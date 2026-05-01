"""管理员支付订单管理接口。"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any

from fastapi import APIRouter, Depends, Query, Request
from fastapi.concurrency import run_in_threadpool
from pydantic import BaseModel, Field, ValidationError
from sqlalchemy.orm import Session

from src.api.base.admin_adapter import AdminApiAdapter
from src.api.base.context import ApiRequestContext
from src.api.base.pipeline import get_pipeline
from src.api.serializers import serialize_payment_callback, serialize_payment_order
from src.core.exceptions import InvalidRequestException, NotFoundException, translate_pydantic_error
from src.database import get_db, get_db_context
from src.services.payment import PaymentService

router = APIRouter(prefix="/api/admin/payments", tags=["Admin - Payments"])
pipeline = get_pipeline()
RECHARGE_ORDER_TYPE = "topup"


class AdminPaymentOrderCreditPayload(BaseModel):
    gateway_order_id: str | None = Field(default=None, max_length=128)
    pay_amount: float | None = Field(default=None, gt=0)
    pay_currency: str | None = Field(default=None, min_length=3, max_length=3)
    exchange_rate: float | None = Field(default=None, gt=0)
    gateway_response: dict[str, Any] | None = None


def _parse_payload(model_cls: type[BaseModel], payload: dict[str, Any]) -> BaseModel:
    try:
        return model_cls.model_validate(payload)
    except ValidationError as exc:
        errors = exc.errors()
        if errors:
            raise InvalidRequestException(translate_pydantic_error(errors[0]))
        raise InvalidRequestException("请求数据验证失败")


def _list_payment_orders_sync(
    status: str | None,
    payment_method: str | None,
    limit: int,
    offset: int,
) -> dict[str, Any]:
    with get_db_context() as db:
        items, total, _changed = PaymentService.list_orders(
            db,
            status=status,
            payment_method=payment_method,
            order_types=RECHARGE_ORDER_TYPE,
            limit=limit,
            offset=offset,
        )
        return {
            "items": [serialize_payment_order(item) for item in items],
            "total": total,
            "limit": limit,
            "offset": offset,
        }


def _get_recharge_payment_order_or_raise(db: Session, order_id: str) -> Any:
    order = PaymentService.get_order(db, order_id=order_id)
    if order is None or str(getattr(order, "order_type", "")) != RECHARGE_ORDER_TYPE:
        raise NotFoundException("充值订单不存在", "payment_order")
    return order


def _get_payment_order_sync(order_id: str) -> dict[str, Any]:
    with get_db_context() as db:
        order = _get_recharge_payment_order_or_raise(db, order_id)
        PaymentService.refresh_order_status(order)
        return {"order": serialize_payment_order(order)}


def _expire_payment_order_sync(order_id: str) -> dict[str, Any]:
    with get_db_context() as db:
        order = _get_recharge_payment_order_or_raise(db, order_id)
        try:
            updated, expired = PaymentService.expire_order(
                db,
                order=order,
                reason="admin_mark_expired",
            )
        except ValueError as exc:
            raise InvalidRequestException(str(exc)) from exc
        return {"order": serialize_payment_order(updated), "expired": expired}


def _credit_payment_order_sync(
    order_id: str,
    payload: AdminPaymentOrderCreditPayload,
    operator_id: str | None,
) -> dict[str, Any]:
    with get_db_context() as db:
        order = _get_recharge_payment_order_or_raise(db, order_id)

        gateway_response = dict(order.gateway_response or {})
        if payload.gateway_response:
            gateway_response.update(payload.gateway_response)
        gateway_response["manual_credit"] = True
        gateway_response["credited_by"] = operator_id

        try:
            updated, credited = PaymentService.credit_order(
                db,
                order=order,
                gateway_order_id=payload.gateway_order_id,
                gateway_response=gateway_response,
                pay_amount=payload.pay_amount,
                pay_currency=payload.pay_currency,
                exchange_rate=payload.exchange_rate,
                operator_id=operator_id,
            )
        except ValueError as exc:
            raise InvalidRequestException(str(exc)) from exc
        return {"order": serialize_payment_order(updated), "credited": credited}


def _approve_payment_order_sync(
    order_id: str,
    payload: AdminPaymentOrderCreditPayload,
    operator_id: str | None,
) -> dict[str, Any]:
    with get_db_context() as db:
        order = _get_recharge_payment_order_or_raise(db, order_id)
        if str(order.payment_method or "") not in {"manual", "manual_review"}:
            raise InvalidRequestException("仅人工充值订单支持审批")
        if str(order.status or "") != "pending_approval":
            raise InvalidRequestException("当前订单不处于待审核状态")

        gateway_response = dict(order.gateway_response or {})
        if payload.gateway_response:
            gateway_response.update(payload.gateway_response)
        gateway_response["manual_credit"] = True
        gateway_response["credited_by"] = operator_id
        gateway_response["approved_by"] = operator_id

        try:
            updated, _credited = PaymentService.credit_order(
                db,
                order=order,
                gateway_order_id=payload.gateway_order_id,
                gateway_response=gateway_response,
                pay_amount=payload.pay_amount,
                pay_currency=payload.pay_currency,
                exchange_rate=payload.exchange_rate,
                operator_id=operator_id,
            )
        except ValueError as exc:
            raise InvalidRequestException(str(exc)) from exc
        return {"order": serialize_payment_order(updated)}


def _fail_payment_order_sync(order_id: str) -> dict[str, Any]:
    with get_db_context() as db:
        order = _get_recharge_payment_order_or_raise(db, order_id)
        try:
            updated = PaymentService.fail_order(
                db,
                order=order,
                reason="admin_mark_failed",
            )
        except ValueError as exc:
            raise InvalidRequestException(str(exc)) from exc
        return {"order": serialize_payment_order(updated)}


def _reject_payment_order_sync(order_id: str, operator_id: str | None) -> dict[str, Any]:
    with get_db_context() as db:
        order = _get_recharge_payment_order_or_raise(db, order_id)
        if str(order.payment_method or "") not in {"manual", "manual_review"}:
            raise InvalidRequestException("仅人工充值订单支持审批")
        if str(order.status or "") != "pending_approval":
            raise InvalidRequestException("当前订单不处于待审核状态")

        try:
            updated = PaymentService.fail_order(
                db,
                order=order,
                reason="admin_rejected",
            )
        except ValueError as exc:
            raise InvalidRequestException(str(exc)) from exc

        gateway_response = dict(updated.gateway_response or {})
        gateway_response["rejected_by"] = operator_id
        updated.gateway_response = gateway_response
        return {"order": serialize_payment_order(updated)}


@router.get("/orders")
async def list_payment_orders(
    request: Request,
    status: str | None = Query(None),
    payment_method: str | None = Query(None),
    limit: int = Query(50, ge=1, le=200),
    offset: int = Query(0, ge=0, le=5000),
    db: Session = Depends(get_db),
) -> Any:
    adapter = AdminPaymentOrderListAdapter(
        status=status,
        payment_method=payment_method,
        limit=limit,
        offset=offset,
    )
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/orders/{order_id}")
async def get_payment_order(
    order_id: str,
    request: Request,
    db: Session = Depends(get_db),
) -> Any:
    adapter = AdminPaymentOrderDetailAdapter(order_id=order_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.post("/orders/{order_id}/expire")
async def expire_payment_order(
    order_id: str,
    request: Request,
    db: Session = Depends(get_db),
) -> Any:
    adapter = AdminPaymentOrderExpireAdapter(order_id=order_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.post("/orders/{order_id}/credit")
async def credit_payment_order(
    order_id: str,
    request: Request,
    db: Session = Depends(get_db),
) -> Any:
    adapter = AdminPaymentOrderCreditAdapter(order_id=order_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.post("/orders/{order_id}/approve")
async def approve_payment_order(
    order_id: str,
    request: Request,
    db: Session = Depends(get_db),
) -> Any:
    adapter = AdminPaymentOrderApproveAdapter(order_id=order_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.post("/orders/{order_id}/fail")
async def fail_payment_order(
    order_id: str,
    request: Request,
    db: Session = Depends(get_db),
) -> Any:
    adapter = AdminPaymentOrderFailAdapter(order_id=order_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.post("/orders/{order_id}/reject")
async def reject_payment_order(
    order_id: str,
    request: Request,
    db: Session = Depends(get_db),
) -> Any:
    adapter = AdminPaymentOrderRejectAdapter(order_id=order_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/callbacks")
async def list_payment_callbacks(
    request: Request,
    payment_method: str | None = Query(None),
    limit: int = Query(50, ge=1, le=200),
    offset: int = Query(0, ge=0, le=5000),
    db: Session = Depends(get_db),
) -> Any:
    adapter = AdminPaymentCallbackListAdapter(
        payment_method=payment_method,
        limit=limit,
        offset=offset,
    )
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@dataclass
class AdminPaymentOrderListAdapter(AdminApiAdapter):
    status: str | None
    payment_method: str | None
    limit: int
    offset: int

    async def handle(self, context: ApiRequestContext) -> dict[str, Any]:
        return await run_in_threadpool(
            _list_payment_orders_sync,
            self.status,
            self.payment_method,
            self.limit,
            self.offset,
        )


@dataclass
class AdminPaymentOrderDetailAdapter(AdminApiAdapter):
    order_id: str

    async def handle(self, context: ApiRequestContext) -> dict[str, Any]:
        return await run_in_threadpool(_get_payment_order_sync, self.order_id)


@dataclass
class AdminPaymentOrderExpireAdapter(AdminApiAdapter):
    order_id: str

    async def handle(self, context: ApiRequestContext) -> dict[str, Any]:
        return await run_in_threadpool(_expire_payment_order_sync, self.order_id)


@dataclass
class AdminPaymentOrderCreditAdapter(AdminApiAdapter):
    order_id: str

    async def handle(self, context: ApiRequestContext) -> dict[str, Any]:
        raw_payload = context.ensure_json_body() if context.raw_body else {}
        req = _parse_payload(AdminPaymentOrderCreditPayload, raw_payload)

        return await run_in_threadpool(
            _credit_payment_order_sync,
            self.order_id,
            req,
            context.user.id if context.user else None,
        )


@dataclass
class AdminPaymentOrderApproveAdapter(AdminApiAdapter):
    order_id: str

    async def handle(self, context: ApiRequestContext) -> dict[str, Any]:
        raw_payload = context.ensure_json_body() if context.raw_body else {}
        req = _parse_payload(AdminPaymentOrderCreditPayload, raw_payload)

        return await run_in_threadpool(
            _approve_payment_order_sync,
            self.order_id,
            req,
            context.user.id if context.user else None,
        )


@dataclass
class AdminPaymentOrderFailAdapter(AdminApiAdapter):
    order_id: str

    async def handle(self, context: ApiRequestContext) -> dict[str, Any]:
        return await run_in_threadpool(_fail_payment_order_sync, self.order_id)


@dataclass
class AdminPaymentOrderRejectAdapter(AdminApiAdapter):
    order_id: str

    async def handle(self, context: ApiRequestContext) -> dict[str, Any]:
        return await run_in_threadpool(
            _reject_payment_order_sync,
            self.order_id,
            context.user.id if context.user else None,
        )


@dataclass
class AdminPaymentCallbackListAdapter(AdminApiAdapter):
    payment_method: str | None
    limit: int
    offset: int

    async def handle(self, context: ApiRequestContext) -> dict[str, Any]:
        items, total = PaymentService.list_callbacks(
            context.db,
            payment_method=self.payment_method,
            order_types=RECHARGE_ORDER_TYPE,
            limit=self.limit,
            offset=self.offset,
        )
        return {
            "items": [serialize_payment_callback(item) for item in items],
            "total": total,
            "limit": self.limit,
            "offset": self.offset,
        }
