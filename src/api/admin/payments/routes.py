"""管理员支付订单管理接口。"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any

from fastapi import APIRouter, Depends, Query, Request
from pydantic import BaseModel, Field, ValidationError
from sqlalchemy.orm import Session

from src.api.base.admin_adapter import AdminApiAdapter
from src.api.base.context import ApiRequestContext
from src.api.base.pipeline import ApiRequestPipeline
from src.api.serializers import serialize_payment_callback, serialize_payment_order
from src.core.exceptions import InvalidRequestException, NotFoundException, translate_pydantic_error
from src.database import get_db
from src.services.payment import PaymentService

router = APIRouter(prefix="/api/admin/payments", tags=["Admin - Payments"])
pipeline = ApiRequestPipeline()


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


@router.post("/orders/{order_id}/fail")
async def fail_payment_order(
    order_id: str,
    request: Request,
    db: Session = Depends(get_db),
) -> Any:
    adapter = AdminPaymentOrderFailAdapter(order_id=order_id)
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
        items, total, changed = PaymentService.list_orders(
            context.db,
            status=self.status,
            payment_method=self.payment_method,
            limit=self.limit,
            offset=self.offset,
        )
        if changed:
            context.db.commit()
        return {
            "items": [serialize_payment_order(item) for item in items],
            "total": total,
            "limit": self.limit,
            "offset": self.offset,
        }


@dataclass
class AdminPaymentOrderDetailAdapter(AdminApiAdapter):
    order_id: str

    async def handle(self, context: ApiRequestContext) -> dict[str, Any]:
        order = PaymentService.get_order(context.db, order_id=self.order_id)
        if order is None:
            raise NotFoundException("Payment order not found")
        if PaymentService.refresh_order_status(order):
            context.db.commit()
        return {"order": serialize_payment_order(order)}


@dataclass
class AdminPaymentOrderExpireAdapter(AdminApiAdapter):
    order_id: str

    async def handle(self, context: ApiRequestContext) -> dict[str, Any]:
        order = PaymentService.get_order(context.db, order_id=self.order_id)
        if order is None:
            raise NotFoundException("Payment order not found")
        try:
            updated, expired = PaymentService.expire_order(
                context.db,
                order=order,
                reason="admin_mark_expired",
            )
        except ValueError as exc:
            raise InvalidRequestException(str(exc))
        context.db.commit()
        return {"order": serialize_payment_order(updated), "expired": expired}


@dataclass
class AdminPaymentOrderCreditAdapter(AdminApiAdapter):
    order_id: str

    async def handle(self, context: ApiRequestContext) -> dict[str, Any]:
        order = PaymentService.get_order(context.db, order_id=self.order_id)
        if order is None:
            raise NotFoundException("Payment order not found")

        raw_payload = context.ensure_json_body() if context.raw_body else {}
        req = _parse_payload(AdminPaymentOrderCreditPayload, raw_payload)

        gateway_response = dict(order.gateway_response or {})
        if req.gateway_response:
            gateway_response.update(req.gateway_response)
        gateway_response["manual_credit"] = True
        gateway_response["credited_by"] = context.user.id if context.user else None

        try:
            updated, credited = PaymentService.credit_order(
                context.db,
                order=order,
                gateway_order_id=req.gateway_order_id,
                gateway_response=gateway_response,
                pay_amount=req.pay_amount,
                pay_currency=req.pay_currency,
                exchange_rate=req.exchange_rate,
            )
        except ValueError as exc:
            raise InvalidRequestException(str(exc))
        context.db.commit()
        return {"order": serialize_payment_order(updated), "credited": credited}


@dataclass
class AdminPaymentOrderFailAdapter(AdminApiAdapter):
    order_id: str

    async def handle(self, context: ApiRequestContext) -> dict[str, Any]:
        order = PaymentService.get_order(context.db, order_id=self.order_id)
        if order is None:
            raise NotFoundException("Payment order not found")
        try:
            updated = PaymentService.fail_order(
                context.db,
                order=order,
                reason="admin_mark_failed",
            )
        except ValueError as exc:
            raise InvalidRequestException(str(exc))
        context.db.commit()
        return {"order": serialize_payment_order(updated)}


@dataclass
class AdminPaymentCallbackListAdapter(AdminApiAdapter):
    payment_method: str | None
    limit: int
    offset: int

    async def handle(self, context: ApiRequestContext) -> dict[str, Any]:
        items, total = PaymentService.list_callbacks(
            context.db,
            payment_method=self.payment_method,
            limit=self.limit,
            offset=self.offset,
        )
        return {
            "items": [serialize_payment_callback(item) for item in items],
            "total": total,
            "limit": self.limit,
            "offset": self.offset,
        }
