"""管理员钱包与退款处理接口。"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any

from fastapi import APIRouter, Depends, Query, Request
from pydantic import BaseModel, Field, ValidationError
from sqlalchemy.orm import Session, joinedload

from src.api.base.admin_adapter import AdminApiAdapter
from src.api.base.context import ApiRequestContext
from src.api.base.pipeline import ApiRequestPipeline
from src.api.serializers import (
    serialize_admin_wallet,
    serialize_admin_wallet_refund,
    serialize_admin_wallet_transaction,
)
from src.core.exceptions import InvalidRequestException, NotFoundException, translate_pydantic_error
from src.database import get_db
from src.models.database import RefundRequest, Wallet, WalletTransaction
from src.services.wallet import WalletService

router = APIRouter(prefix="/api/admin/wallets", tags=["Admin - Wallets"])
pipeline = ApiRequestPipeline()


class ManualRechargePayload(BaseModel):
    amount_usd: float = Field(..., gt=0, allow_inf_nan=False)
    payment_method: str = Field(default="admin_manual", max_length=30)
    description: str | None = Field(default=None, max_length=500)


class WalletAdjustPayload(BaseModel):
    amount_usd: float = Field(..., allow_inf_nan=False)
    balance_type: str = Field(default="recharge", pattern="^(recharge|gift)$")
    description: str | None = Field(default=None, max_length=500)


class RefundFailPayload(BaseModel):
    reason: str = Field(..., min_length=1, max_length=500)


class RefundCompletePayload(BaseModel):
    gateway_refund_id: str | None = Field(default=None, max_length=128)
    payout_reference: str | None = Field(default=None, max_length=255)
    payout_proof: dict[str, Any] | None = None


def _get_wallet_or_raise(db: Session, wallet_id: str) -> Wallet:
    wallet = (
        db.query(Wallet)
        .options(joinedload(Wallet.user), joinedload(Wallet.api_key))
        .filter(Wallet.id == wallet_id)
        .first()
    )
    if wallet is None:
        raise NotFoundException("Wallet not found")
    return wallet


def _get_refund_or_raise(db: Session, wallet_id: str, refund_id: str) -> RefundRequest:
    refund = (
        db.query(RefundRequest)
        .filter(RefundRequest.id == refund_id, RefundRequest.wallet_id == wallet_id)
        .first()
    )
    if refund is None:
        raise NotFoundException("Refund request not found")
    return refund


def _ensure_user_wallet_for_refund(wallet: Wallet) -> None:
    if wallet.api_key_id is not None:
        raise InvalidRequestException("独立密钥钱包不支持退款审批")


def _ensure_api_key_wallet_manual_recharge(wallet: Wallet, payment_method: str) -> None:
    if wallet.api_key_id is not None:
        raise InvalidRequestException("独立密钥钱包不支持充值，请使用调账")


def _parse_payload(model_cls: type[BaseModel], payload: dict[str, Any]) -> BaseModel:
    try:
        return model_cls.model_validate(payload)
    except ValidationError as exc:
        errors = exc.errors()
        if errors:
            raise InvalidRequestException(translate_pydantic_error(errors[0]))
        raise InvalidRequestException("请求数据验证失败")


@router.get("")
async def list_wallets(
    request: Request,
    status: str | None = Query(None),
    limit: int = Query(50, ge=1, le=200),
    offset: int = Query(0, ge=0, le=5000),
    db: Session = Depends(get_db),
) -> Any:
    adapter = AdminWalletListAdapter(status=status, limit=limit, offset=offset)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/ledger")
async def list_wallet_ledger(
    request: Request,
    category: str | None = Query(None),
    reason_code: str | None = Query(None),
    owner_type: str | None = Query(None),
    limit: int = Query(50, ge=1, le=200),
    offset: int = Query(0, ge=0, le=5000),
    db: Session = Depends(get_db),
) -> Any:
    adapter = AdminWalletLedgerAdapter(
        category=category,
        reason_code=reason_code,
        owner_type=owner_type,
        limit=limit,
        offset=offset,
    )
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/refund-requests")
async def list_global_refunds(
    request: Request,
    status: str | None = Query(None),
    owner_type: str | None = Query(None),
    limit: int = Query(50, ge=1, le=200),
    offset: int = Query(0, ge=0, le=5000),
    db: Session = Depends(get_db),
) -> Any:
    adapter = AdminWalletGlobalRefundsAdapter(
        status=status,
        owner_type=owner_type,
        limit=limit,
        offset=offset,
    )
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/{wallet_id}")
async def get_wallet_detail(wallet_id: str, request: Request, db: Session = Depends(get_db)) -> Any:
    adapter = AdminWalletDetailAdapter(wallet_id=wallet_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/{wallet_id}/transactions")
async def get_wallet_transactions(
    wallet_id: str,
    request: Request,
    limit: int = Query(50, ge=1, le=200),
    offset: int = Query(0, ge=0, le=5000),
    db: Session = Depends(get_db),
) -> Any:
    adapter = AdminWalletTransactionsAdapter(wallet_id=wallet_id, limit=limit, offset=offset)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/{wallet_id}/refunds")
async def get_wallet_refunds(
    wallet_id: str,
    request: Request,
    limit: int = Query(50, ge=1, le=200),
    offset: int = Query(0, ge=0, le=5000),
    db: Session = Depends(get_db),
) -> Any:
    adapter = AdminWalletRefundsAdapter(wallet_id=wallet_id, limit=limit, offset=offset)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.post("/{wallet_id}/recharge")
async def recharge_wallet(wallet_id: str, request: Request, db: Session = Depends(get_db)) -> Any:
    adapter = AdminWalletRechargeAdapter(wallet_id=wallet_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.post("/{wallet_id}/adjust")
async def adjust_wallet(wallet_id: str, request: Request, db: Session = Depends(get_db)) -> Any:
    adapter = AdminWalletAdjustAdapter(wallet_id=wallet_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.post("/{wallet_id}/refunds/{refund_id}/process")
async def process_refund(
    wallet_id: str,
    refund_id: str,
    request: Request,
    db: Session = Depends(get_db),
) -> Any:
    adapter = AdminWalletRefundProcessAdapter(wallet_id=wallet_id, refund_id=refund_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.post("/{wallet_id}/refunds/{refund_id}/fail")
async def fail_refund(
    wallet_id: str,
    refund_id: str,
    request: Request,
    db: Session = Depends(get_db),
) -> Any:
    adapter = AdminWalletRefundFailAdapter(wallet_id=wallet_id, refund_id=refund_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.post("/{wallet_id}/refunds/{refund_id}/complete")
async def complete_refund(
    wallet_id: str,
    refund_id: str,
    request: Request,
    db: Session = Depends(get_db),
) -> Any:
    adapter = AdminWalletRefundCompleteAdapter(wallet_id=wallet_id, refund_id=refund_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@dataclass
class AdminWalletListAdapter(AdminApiAdapter):
    status: str | None
    limit: int
    offset: int

    async def handle(self, context: ApiRequestContext) -> dict[str, Any]:
        q = context.db.query(Wallet).options(joinedload(Wallet.user), joinedload(Wallet.api_key))
        if self.status:
            q = q.filter(Wallet.status == self.status)
        total = q.count()
        items = q.order_by(Wallet.updated_at.desc()).offset(self.offset).limit(self.limit).all()
        return {
            "items": [serialize_admin_wallet(item) for item in items],
            "total": total,
            "limit": self.limit,
            "offset": self.offset,
        }


@dataclass
class AdminWalletDetailAdapter(AdminApiAdapter):
    wallet_id: str

    async def handle(self, context: ApiRequestContext) -> dict[str, Any]:
        wallet = _get_wallet_or_raise(context.db, self.wallet_id)
        pending_refunds = (
            context.db.query(RefundRequest)
            .filter(
                RefundRequest.wallet_id == wallet.id,
                RefundRequest.status.in_(["pending_approval", "processing"]),
            )
            .count()
        )
        return {
            **serialize_admin_wallet(wallet),
            "pending_refund_count": pending_refunds,
        }


@dataclass
class AdminWalletLedgerAdapter(AdminApiAdapter):
    category: str | None
    reason_code: str | None
    owner_type: str | None
    limit: int
    offset: int

    async def handle(self, context: ApiRequestContext) -> dict[str, Any]:
        q = (
            context.db.query(WalletTransaction)
            .join(Wallet, WalletTransaction.wallet_id == Wallet.id)
            .options(
                joinedload(WalletTransaction.wallet).joinedload(Wallet.user),
                joinedload(WalletTransaction.wallet).joinedload(Wallet.api_key),
                joinedload(WalletTransaction.operator),
            )
        )

        if self.category:
            q = q.filter(WalletTransaction.category == self.category)
        if self.reason_code:
            q = q.filter(WalletTransaction.reason_code == self.reason_code)

        if self.owner_type == "user":
            q = q.filter(Wallet.user_id.isnot(None))
        elif self.owner_type == "api_key":
            q = q.filter(Wallet.api_key_id.isnot(None))

        total = q.count()
        items = q.order_by(WalletTransaction.created_at.desc()).offset(self.offset).limit(self.limit).all()
        return {
            "items": [serialize_admin_wallet_transaction(item) for item in items],
            "total": total,
            "limit": self.limit,
            "offset": self.offset,
        }


@dataclass
class AdminWalletTransactionsAdapter(AdminApiAdapter):
    wallet_id: str
    limit: int
    offset: int

    async def handle(self, context: ApiRequestContext) -> dict[str, Any]:
        wallet = _get_wallet_or_raise(context.db, self.wallet_id)
        q = (
            context.db.query(WalletTransaction)
            .options(joinedload(WalletTransaction.operator))
            .filter(WalletTransaction.wallet_id == wallet.id)
        )
        total = q.count()
        items = q.order_by(WalletTransaction.created_at.desc()).offset(self.offset).limit(self.limit).all()
        return {
            "wallet": serialize_admin_wallet(wallet),
            "items": [serialize_admin_wallet_transaction(item) for item in items],
            "total": total,
            "limit": self.limit,
            "offset": self.offset,
        }


@dataclass
class AdminWalletRefundsAdapter(AdminApiAdapter):
    wallet_id: str
    limit: int
    offset: int

    async def handle(self, context: ApiRequestContext) -> dict[str, Any]:
        wallet = _get_wallet_or_raise(context.db, self.wallet_id)
        _ensure_user_wallet_for_refund(wallet)
        q = context.db.query(RefundRequest).filter(RefundRequest.wallet_id == wallet.id)
        total = q.count()
        items = q.order_by(RefundRequest.created_at.desc()).offset(self.offset).limit(self.limit).all()
        return {
            "wallet": serialize_admin_wallet(wallet),
            "items": [serialize_admin_wallet_refund(item) for item in items],
            "total": total,
            "limit": self.limit,
            "offset": self.offset,
        }


@dataclass
class AdminWalletGlobalRefundsAdapter(AdminApiAdapter):
    status: str | None
    owner_type: str | None
    limit: int
    offset: int

    async def handle(self, context: ApiRequestContext) -> dict[str, Any]:
        if self.owner_type == "api_key":
            raise InvalidRequestException("独立密钥钱包不支持退款审批")

        q = (
            context.db.query(RefundRequest)
            .join(Wallet, RefundRequest.wallet_id == Wallet.id)
            .options(
                joinedload(RefundRequest.wallet).joinedload(Wallet.user),
                joinedload(RefundRequest.wallet).joinedload(Wallet.api_key),
            )
        )

        if self.status:
            q = q.filter(RefundRequest.status == self.status)

        q = q.filter(Wallet.user_id.isnot(None))

        total = q.count()
        items = q.order_by(RefundRequest.created_at.desc()).offset(self.offset).limit(self.limit).all()
        return {
            "items": [serialize_admin_wallet_refund(item) for item in items],
            "total": total,
            "limit": self.limit,
            "offset": self.offset,
        }


@dataclass
class AdminWalletRechargeAdapter(AdminApiAdapter):
    wallet_id: str

    async def handle(self, context: ApiRequestContext) -> dict[str, Any]:
        payload = _parse_payload(ManualRechargePayload, context.ensure_json_body())
        assert isinstance(payload, ManualRechargePayload)

        wallet = _get_wallet_or_raise(context.db, self.wallet_id)
        _ensure_api_key_wallet_manual_recharge(wallet, payload.payment_method)
        try:
            order = WalletService.create_manual_recharge_order(
                context.db,
                wallet=wallet,
                amount_usd=payload.amount_usd,
                payment_method=payload.payment_method,
                operator_id=context.user.id if context.user else None,
                description=payload.description,
            )
        except ValueError as exc:
            raise InvalidRequestException(str(exc))
        context.db.commit()
        context.db.refresh(wallet)
        return {
            "wallet": serialize_admin_wallet(wallet),
            "payment_order": {
                "id": order.id,
                "order_no": order.order_no,
                "amount_usd": float(order.amount_usd or 0),
                "payment_method": order.payment_method,
                "status": order.status,
                "created_at": order.created_at,
                "credited_at": order.credited_at,
            },
        }


@dataclass
class AdminWalletAdjustAdapter(AdminApiAdapter):
    wallet_id: str

    async def handle(self, context: ApiRequestContext) -> dict[str, Any]:
        payload = _parse_payload(WalletAdjustPayload, context.ensure_json_body())
        assert isinstance(payload, WalletAdjustPayload)

        wallet = _get_wallet_or_raise(context.db, self.wallet_id)
        if wallet.api_key_id is not None and payload.balance_type == "gift":
            raise InvalidRequestException("独立密钥钱包不支持赠款调账")
        try:
            tx = WalletService.admin_adjust_balance(
                context.db,
                wallet=wallet,
                amount_usd=payload.amount_usd,
                balance_type=payload.balance_type,  # recharge | gift
                operator_id=context.user.id if context.user else None,
                description=payload.description,
            )
        except ValueError as exc:
            raise InvalidRequestException(str(exc))

        context.db.commit()
        context.db.refresh(wallet)
        return {
            "wallet": serialize_admin_wallet(wallet),
            "transaction": serialize_admin_wallet_transaction(tx),
        }


@dataclass
class AdminWalletRefundProcessAdapter(AdminApiAdapter):
    wallet_id: str
    refund_id: str

    async def handle(self, context: ApiRequestContext) -> dict[str, Any]:
        wallet = _get_wallet_or_raise(context.db, self.wallet_id)
        _ensure_user_wallet_for_refund(wallet)
        refund = _get_refund_or_raise(context.db, self.wallet_id, self.refund_id)
        try:
            tx = WalletService.move_refund_to_processing(
                context.db,
                refund=refund,
                operator_id=context.user.id if context.user else None,
            )
        except ValueError as exc:
            raise InvalidRequestException(str(exc))

        context.db.commit()
        context.db.refresh(wallet)
        context.db.refresh(refund)
        return {
            "wallet": serialize_admin_wallet(wallet),
            "refund": serialize_admin_wallet_refund(refund),
            "transaction": serialize_admin_wallet_transaction(tx),
        }


@dataclass
class AdminWalletRefundFailAdapter(AdminApiAdapter):
    wallet_id: str
    refund_id: str

    async def handle(self, context: ApiRequestContext) -> dict[str, Any]:
        payload = _parse_payload(RefundFailPayload, context.ensure_json_body())
        assert isinstance(payload, RefundFailPayload)

        wallet = _get_wallet_or_raise(context.db, self.wallet_id)
        _ensure_user_wallet_for_refund(wallet)
        refund = _get_refund_or_raise(context.db, self.wallet_id, self.refund_id)
        try:
            tx = WalletService.fail_refund(
                context.db,
                refund=refund,
                reason=payload.reason,
                operator_id=context.user.id if context.user else None,
            )
        except ValueError as exc:
            raise InvalidRequestException(str(exc))
        context.db.commit()
        context.db.refresh(wallet)
        context.db.refresh(refund)
        return {
            "wallet": serialize_admin_wallet(wallet),
            "refund": serialize_admin_wallet_refund(refund),
            "transaction": serialize_admin_wallet_transaction(tx) if tx is not None else None,
        }


@dataclass
class AdminWalletRefundCompleteAdapter(AdminApiAdapter):
    wallet_id: str
    refund_id: str

    async def handle(self, context: ApiRequestContext) -> dict[str, Any]:
        payload = _parse_payload(RefundCompletePayload, context.ensure_json_body())
        assert isinstance(payload, RefundCompletePayload)

        wallet = _get_wallet_or_raise(context.db, self.wallet_id)
        _ensure_user_wallet_for_refund(wallet)
        refund = _get_refund_or_raise(context.db, self.wallet_id, self.refund_id)
        if refund.status != "processing":
            raise InvalidRequestException("只有 processing 状态的退款可以标记完成")

        try:
            updated = WalletService.complete_refund(
                context.db,
                refund=refund,
                gateway_refund_id=payload.gateway_refund_id,
                payout_reference=payload.payout_reference,
                payout_proof=payload.payout_proof,
            )
        except ValueError as exc:
            raise InvalidRequestException(str(exc))
        context.db.commit()
        context.db.refresh(updated)
        return {"refund": serialize_admin_wallet_refund(updated)}
