"""用户钱包与退款接口。"""

from __future__ import annotations

from dataclasses import dataclass
from datetime import date, datetime, timezone
from typing import Any
from uuid import uuid4
from zoneinfo import ZoneInfo

from fastapi import APIRouter, Depends, Query, Request
from fastapi.concurrency import run_in_threadpool
from pydantic import BaseModel, Field, ValidationError
from sqlalchemy.exc import IntegrityError
from sqlalchemy.orm import Session

from src.api.base.authenticated_adapter import AuthenticatedApiAdapter
from src.api.base.context import ApiRequestContext
from src.api.base.pipeline import get_pipeline
from src.api.serializers import (
    safe_gateway_response,
    serialize_payment_order,
    serialize_wallet_daily_usage,
    serialize_wallet_payload,
    serialize_wallet_refund,
    serialize_wallet_transaction,
)
from src.core.exceptions import InvalidRequestException, NotFoundException, translate_pydantic_error
from src.database import get_db, get_db_context
from src.models.database import (
    PaymentOrder,
    RefundRequest,
    User,
    Wallet,
    WalletDailyUsageLedger,
    WalletTransaction,
)
from src.services.payment import PaymentService
from src.services.wallet import WalletDailyUsageLedgerService, WalletService

router = APIRouter(prefix="/api/wallet", tags=["Wallet"])
pipeline = get_pipeline()


def _create_recharge_order_sync(user_id: str, req: CreateRechargePayload) -> dict[str, Any]:
    with get_db_context() as db:
        user = db.query(User).filter(User.id == user_id).first()
        if user is None:
            raise InvalidRequestException("未登录")

        try:
            order = PaymentService.create_recharge_order(
                db,
                user=user,
                amount_usd=req.amount_usd,
                payment_method=req.payment_method,
                pay_amount=req.pay_amount,
                pay_currency=req.pay_currency,
                exchange_rate=req.exchange_rate,
            )
        except ValueError as exc:
            raise InvalidRequestException(str(exc)) from exc

        db.commit()
        db.refresh(order)
        return {
            "order": serialize_payment_order(order, sanitize_gateway_response=True),
            "payment_instructions": safe_gateway_response(order.gateway_response),
        }


def _list_recharge_orders_sync(user_id: str, limit: int, offset: int) -> dict[str, Any]:
    with get_db_context() as db:
        items, total, _changed = PaymentService.list_user_orders(
            db,
            user_id=user_id,
            limit=limit,
            offset=offset,
            order_types="topup",
        )
        return {
            "items": [
                serialize_payment_order(item, sanitize_gateway_response=True) for item in items
            ],
            "total": total,
            "limit": limit,
            "offset": offset,
        }


def _get_recharge_order_sync(user_id: str, order_id: str) -> dict[str, Any]:
    with get_db_context() as db:
        order = PaymentService.get_user_order(db, user_id=user_id, order_id=order_id)
        if order is None or str(getattr(order, "order_type", "")) != "topup":
            raise NotFoundException("充值订单不存在", "payment_order")
        PaymentService.refresh_order_status(order)
        return {"order": serialize_payment_order(order, sanitize_gateway_response=True)}


def _cancel_recharge_order_sync(user_id: str, order_id: str) -> dict[str, Any]:
    with get_db_context() as db:
        order = PaymentService.get_user_order(db, user_id=user_id, order_id=order_id)
        if order is None or str(getattr(order, "order_type", "")) != "topup":
            raise NotFoundException("充值订单不存在", "payment_order")
        if str(getattr(order, "status", "")) not in {"pending", "pending_approval"}:
            raise InvalidRequestException("仅待支付充值订单支持取消")

        try:
            updated, _changed = PaymentService.expire_order(
                db,
                order=order,
                reason="user_cancelled",
            )
        except ValueError as exc:
            raise InvalidRequestException(str(exc)) from exc

        db.commit()
        db.refresh(updated)
        return {"order": serialize_payment_order(updated, sanitize_gateway_response=True)}


def _list_refunds_sync(user_id: str, limit: int, offset: int) -> dict[str, Any]:
    with get_db_context() as db:
        user = db.query(User).filter(User.id == user_id).first()
        if user is None:
            raise InvalidRequestException("未登录")

        existing_wallet = WalletService.get_wallet(db, user_id=user.id)
        wallet = existing_wallet or WalletService.get_or_create_wallet(db, user=user)
        if wallet is None:
            return {"items": [], "total": 0, "limit": limit, "offset": offset}

        if existing_wallet is None:
            db.commit()
            db.refresh(wallet)

        base_query = db.query(RefundRequest).filter(RefundRequest.wallet_id == wallet.id)
        total = base_query.count()
        items = (
            base_query.order_by(RefundRequest.created_at.desc()).offset(offset).limit(limit).all()
        )
        return {
            "items": [serialize_wallet_refund(item) for item in items],
            "total": total,
            "limit": limit,
            "offset": offset,
        }


def _create_refund_sync(user_id: str, req: CreateRefundPayload) -> dict[str, Any]:
    with get_db_context() as db:
        user = db.query(User).filter(User.id == user_id).first()
        if user is None:
            raise InvalidRequestException("未登录")

        wallet = WalletService.get_or_create_wallet(db, user=user)
        if wallet is None:
            raise InvalidRequestException("当前账户尚未开通钱包，无法申请退款")

        payment_order = None
        source_type = req.source_type or "wallet_balance"
        source_id = req.source_id
        refund_mode = req.refund_mode or "offline_payout"

        if req.payment_order_id:
            payment_order = (
                db.query(PaymentOrder)
                .filter(
                    PaymentOrder.id == req.payment_order_id, PaymentOrder.wallet_id == wallet.id
                )
                .first()
            )
            if payment_order is None:
                raise NotFoundException("Payment order not found")
            source_type = "payment_order"
            source_id = payment_order.id
            refund_mode = req.refund_mode or _default_refund_mode_for_order(payment_order)

        try:
            refund = WalletService.create_refund_request(
                db,
                wallet=wallet,
                user_id=user.id,
                amount_usd=req.amount_usd,
                refund_no=_build_refund_no(),
                source_type=source_type,
                source_id=source_id,
                refund_mode=refund_mode,
                payment_order=payment_order,
                reason=req.reason,
                requested_by=user.id,
                idempotency_key=req.idempotency_key,
            )
            db.commit()
            db.refresh(refund)
            return serialize_wallet_refund(refund)
        except ValueError as exc:
            db.rollback()
            raise InvalidRequestException(str(exc)) from exc
        except IntegrityError:
            db.rollback()
            if req.idempotency_key:
                existing = (
                    db.query(RefundRequest)
                    .filter(
                        RefundRequest.idempotency_key == req.idempotency_key,
                        RefundRequest.user_id == user.id,
                    )
                    .first()
                )
                if existing is not None:
                    return serialize_wallet_refund(existing)
            raise InvalidRequestException("退款申请重复，请勿重复提交")


class CreateRefundPayload(BaseModel):
    amount_usd: float = Field(..., gt=0, allow_inf_nan=False)
    payment_order_id: str | None = None
    source_type: str | None = Field(default=None, max_length=30)
    source_id: str | None = Field(default=None, max_length=100)
    refund_mode: str | None = Field(default=None, max_length=30)
    reason: str | None = Field(default=None, max_length=500)
    idempotency_key: str | None = Field(default=None, max_length=128)


class CreateRechargePayload(BaseModel):
    amount_usd: float = Field(..., gt=0, allow_inf_nan=False)
    payment_method: str = Field(..., min_length=1, max_length=30)
    pay_amount: float | None = Field(default=None, gt=0, allow_inf_nan=False)
    pay_currency: str | None = Field(default=None, min_length=3, max_length=3)
    exchange_rate: float | None = Field(default=None, gt=0, allow_inf_nan=False)


def _default_refund_mode_for_order(order: PaymentOrder) -> str:
    if order.payment_method in {
        "admin_manual",
        "manual",
        "manual_review",
        "card_recharge",
        "card_code",
        "gift_code",
    }:
        return "offline_payout"
    return "original_channel"


def _build_refund_no() -> str:
    ts = datetime.now(timezone.utc).strftime("%Y%m%d%H%M%S%f")
    return f"rf_{ts}_{uuid4().hex[:8]}"


def _resolve_user_wallet(db: Session, user: Any) -> Wallet | None:
    existing_wallet = WalletService.get_wallet(db, user_id=user.id)
    wallet = existing_wallet or WalletService.get_or_create_wallet(db, user=user)
    if wallet is not None and existing_wallet is None:
        db.commit()
        db.refresh(wallet)
    return wallet


def _flow_sort_key(item: dict[str, Any], billing_tz: ZoneInfo) -> tuple[date, int, datetime]:
    item_type = item.get("type")
    data = item.get("data") or {}
    if item_type == "daily_usage":
        raw_date = data.get("date")
        billing_date = (
            date.fromisoformat(raw_date) if isinstance(raw_date, str) and raw_date else date.min
        )
        sort_dt = (
            data.get("last_finalized_at")
            or data.get("aggregated_at")
            or datetime.min.replace(tzinfo=timezone.utc)
        )
        if isinstance(sort_dt, str):
            sort_dt = datetime.fromisoformat(sort_dt.replace("Z", "+00:00"))
        return billing_date, 1, sort_dt

    created_at = data.get("created_at")
    if isinstance(created_at, str):
        created_at = datetime.fromisoformat(created_at.replace("Z", "+00:00"))
    if not isinstance(created_at, datetime):
        created_at = datetime.min.replace(tzinfo=timezone.utc)
    elif created_at.tzinfo is None:
        created_at = created_at.replace(tzinfo=timezone.utc)
    local_date = created_at.astimezone(billing_tz).date()
    return local_date, 0, created_at


@router.get("/balance")
async def get_wallet_balance(request: Request, db: Session = Depends(get_db)) -> Any:
    adapter = WalletBalanceAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/transactions")
async def list_wallet_transactions(
    request: Request,
    limit: int = Query(50, ge=1, le=200),
    offset: int = Query(0, ge=0, le=5000),
    db: Session = Depends(get_db),
) -> Any:
    adapter = WalletTransactionsAdapter(limit=limit, offset=offset)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/flow")
async def get_wallet_flow(
    request: Request,
    limit: int = Query(50, ge=1, le=200),
    offset: int = Query(0, ge=0, le=5000),
    db: Session = Depends(get_db),
) -> Any:
    adapter = WalletFlowAdapter(limit=limit, offset=offset)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/today-cost")
async def get_wallet_today_cost(request: Request, db: Session = Depends(get_db)) -> Any:
    adapter = WalletTodayCostAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.post("/recharge")
async def create_recharge_order(request: Request, db: Session = Depends(get_db)) -> Any:
    adapter = WalletRechargeCreateAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/recharge")
async def list_recharge_orders(
    request: Request,
    limit: int = Query(50, ge=1, le=200),
    offset: int = Query(0, ge=0, le=5000),
    db: Session = Depends(get_db),
) -> Any:
    adapter = WalletRechargeListAdapter(limit=limit, offset=offset)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/recharge/{order_id}")
async def get_recharge_order(order_id: str, request: Request, db: Session = Depends(get_db)) -> Any:
    adapter = WalletRechargeDetailAdapter(order_id=order_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.post("/recharge/{order_id}/cancel")
async def cancel_recharge_order(
    order_id: str,
    request: Request,
    db: Session = Depends(get_db),
) -> Any:
    adapter = WalletRechargeCancelAdapter(order_id=order_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/refunds")
async def list_refunds(
    request: Request,
    limit: int = Query(50, ge=1, le=200),
    offset: int = Query(0, ge=0, le=5000),
    db: Session = Depends(get_db),
) -> Any:
    adapter = WalletRefundListAdapter(limit=limit, offset=offset)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/refunds/{refund_id}")
async def get_refund_detail(refund_id: str, request: Request, db: Session = Depends(get_db)) -> Any:
    adapter = WalletRefundDetailAdapter(refund_id=refund_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.post("/refunds")
async def create_refund(request: Request, db: Session = Depends(get_db)) -> Any:
    adapter = WalletRefundCreateAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@dataclass
class WalletTransactionsAdapter(AuthenticatedApiAdapter):
    limit: int
    offset: int

    async def handle(self, context: ApiRequestContext) -> dict[str, Any]:
        db = context.db
        user = context.user
        if user is None:
            raise InvalidRequestException("未登录")

        wallet = _resolve_user_wallet(db, user)
        if wallet is None:
            return {
                "items": [],
                "total": 0,
                "limit": self.limit,
                "offset": self.offset,
                **serialize_wallet_payload(None),
            }

        base_query = db.query(WalletTransaction).filter(WalletTransaction.wallet_id == wallet.id)
        total = base_query.count()
        items = (
            base_query.order_by(WalletTransaction.created_at.desc())
            .offset(self.offset)
            .limit(self.limit)
            .all()
        )

        return {
            "items": [serialize_wallet_transaction(item) for item in items],
            "total": total,
            "limit": self.limit,
            "offset": self.offset,
            **serialize_wallet_payload(wallet),
        }


@dataclass
class WalletFlowAdapter(AuthenticatedApiAdapter):
    limit: int
    offset: int

    async def handle(self, context: ApiRequestContext) -> dict[str, Any]:
        db = context.db
        user = context.user
        if user is None:
            raise InvalidRequestException("未登录")

        wallet = _resolve_user_wallet(db, user)
        if wallet is None:
            return {
                "today_entry": None,
                "items": [],
                "total": 0,
                "limit": self.limit,
                "offset": self.offset,
                **serialize_wallet_payload(None),
            }

        today_entry = WalletDailyUsageLedgerService.get_today_snapshot(db, wallet.id)
        billing_tz = WalletDailyUsageLedgerService.get_timezone(today_entry.billing_timezone)
        fetch_size = min(self.offset + self.limit, 5200)

        tx_query = db.query(WalletTransaction).filter(WalletTransaction.wallet_id == wallet.id)
        tx_total = tx_query.count()
        tx_items = tx_query.order_by(WalletTransaction.created_at.desc()).limit(fetch_size).all()

        daily_query = db.query(WalletDailyUsageLedger).filter(
            WalletDailyUsageLedger.wallet_id == wallet.id,
            WalletDailyUsageLedger.billing_timezone == today_entry.billing_timezone,
            WalletDailyUsageLedger.billing_date < today_entry.billing_date,
        )
        daily_total = daily_query.count()
        daily_items = (
            daily_query.order_by(WalletDailyUsageLedger.billing_date.desc()).limit(fetch_size).all()
        )

        merged = [
            {"type": "transaction", "data": serialize_wallet_transaction(item)} for item in tx_items
        ] + [
            {"type": "daily_usage", "data": serialize_wallet_daily_usage(item)}
            for item in daily_items
        ]
        merged.sort(key=lambda item: _flow_sort_key(item, billing_tz), reverse=True)
        paged = merged[self.offset : self.offset + self.limit]

        return {
            "today_entry": serialize_wallet_daily_usage(today_entry),
            "items": paged,
            "total": tx_total + daily_total,
            "limit": self.limit,
            "offset": self.offset,
            **serialize_wallet_payload(wallet),
        }


class WalletTodayCostAdapter(AuthenticatedApiAdapter):
    async def handle(self, context: ApiRequestContext) -> dict[str, Any]:
        db = context.db
        user = context.user
        if user is None:
            raise InvalidRequestException("未登录")

        wallet = _resolve_user_wallet(db, user)
        snapshot = WalletDailyUsageLedgerService.get_today_snapshot(
            db,
            wallet.id if wallet is not None else None,
        )
        return serialize_wallet_daily_usage(snapshot)


class WalletBalanceAdapter(AuthenticatedApiAdapter):
    async def handle(self, context: ApiRequestContext) -> dict[str, Any]:
        db = context.db
        user = context.user
        if user is None:
            raise InvalidRequestException("未登录")

        wallet = _resolve_user_wallet(db, user)
        if wallet is None:
            return serialize_wallet_payload(None)

        pending_refunds = (
            db.query(RefundRequest)
            .filter(
                RefundRequest.wallet_id == wallet.id,
                RefundRequest.status.in_(["pending_approval", "approved", "processing"]),
            )
            .count()
        )

        payload = serialize_wallet_payload(wallet)
        payload["pending_refund_count"] = pending_refunds
        return payload


class WalletRechargeCreateAdapter(AuthenticatedApiAdapter):
    async def handle(self, context: ApiRequestContext) -> dict[str, Any]:
        db = context.db
        user = context.user
        if user is None:
            raise InvalidRequestException("未登录")

        payload = context.ensure_json_body()
        try:
            req = CreateRechargePayload.model_validate(payload)
        except ValidationError as exc:
            errors = exc.errors()
            if errors:
                raise InvalidRequestException(translate_pydantic_error(errors[0]))
            raise InvalidRequestException("请求数据验证失败")

        return await run_in_threadpool(_create_recharge_order_sync, user.id, req)


@dataclass
class WalletRechargeListAdapter(AuthenticatedApiAdapter):
    limit: int
    offset: int

    async def handle(self, context: ApiRequestContext) -> dict[str, Any]:
        user = context.user
        if user is None:
            raise InvalidRequestException("未登录")

        return await run_in_threadpool(
            _list_recharge_orders_sync,
            user.id,
            self.limit,
            self.offset,
        )


@dataclass
class WalletRechargeDetailAdapter(AuthenticatedApiAdapter):
    order_id: str

    async def handle(self, context: ApiRequestContext) -> dict[str, Any]:
        user = context.user
        if user is None:
            raise InvalidRequestException("未登录")

        return await run_in_threadpool(_get_recharge_order_sync, user.id, self.order_id)


@dataclass
class WalletRechargeCancelAdapter(AuthenticatedApiAdapter):
    order_id: str

    async def handle(self, context: ApiRequestContext) -> dict[str, Any]:
        user = context.user
        if user is None:
            raise InvalidRequestException("未登录")

        return await run_in_threadpool(_cancel_recharge_order_sync, user.id, self.order_id)


@dataclass
class WalletRefundListAdapter(AuthenticatedApiAdapter):
    limit: int
    offset: int

    async def handle(self, context: ApiRequestContext) -> dict[str, Any]:
        db = context.db
        user = context.user
        if user is None:
            raise InvalidRequestException("未登录")

        return await run_in_threadpool(_list_refunds_sync, user.id, self.limit, self.offset)


@dataclass
class WalletRefundDetailAdapter(AuthenticatedApiAdapter):
    refund_id: str

    async def handle(self, context: ApiRequestContext) -> dict[str, Any]:
        db = context.db
        user = context.user
        if user is None:
            raise InvalidRequestException("未登录")

        refund = (
            db.query(RefundRequest)
            .filter(RefundRequest.id == self.refund_id, RefundRequest.user_id == user.id)
            .first()
        )
        if refund is None:
            raise NotFoundException("Refund request not found")
        return serialize_wallet_refund(refund)


class WalletRefundCreateAdapter(AuthenticatedApiAdapter):
    async def handle(self, context: ApiRequestContext) -> dict[str, Any]:
        db = context.db
        user = context.user
        if user is None:
            raise InvalidRequestException("未登录")

        payload = context.ensure_json_body()
        try:
            req = CreateRefundPayload.model_validate(payload)
        except ValidationError as exc:
            errors = exc.errors()
            if errors:
                raise InvalidRequestException(translate_pydantic_error(errors[0]))
            raise InvalidRequestException("请求数据验证失败")

        return await run_in_threadpool(_create_refund_sync, user.id, req)
