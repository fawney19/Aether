"""用户端订阅展示与购买接口。"""

from __future__ import annotations

from dataclasses import dataclass
from decimal import Decimal
from typing import Any

from fastapi import APIRouter, Depends, Query, Request
from fastapi.concurrency import run_in_threadpool
from pydantic import ValidationError
from sqlalchemy.orm import Session

from src.api.base.authenticated_adapter import AuthenticatedApiAdapter
from src.api.base.context import ApiRequestContext
from src.api.base.pipeline import get_pipeline
from src.api.serializers import safe_gateway_response, serialize_payment_order
from src.core.exceptions import InvalidRequestException, NotFoundException, translate_pydantic_error
from src.database import get_db, get_db_context
from src.models.api import (
    CreateSubscriptionCheckoutRequest,
    SubscriptionCheckoutPublicResponse,
    SubscriptionPlanPublicResponse,
    SubscriptionProductPublicListResponse,
    SubscriptionProductPublicResponse,
    SubscriptionVariantPublicResponse,
    UpgradeSubscriptionCheckoutRequest,
    UserSubscriptionDashboardPublicResponse,
    UserSubscriptionPublicResponse,
)
from src.models.database import SubscriptionPlan, SubscriptionProduct, User, UserSubscription
from src.services.payment import PaymentService
from src.services.subscription import SubscriptionService

router = APIRouter(prefix="/api/subscriptions", tags=["Subscriptions"])
pipeline = get_pipeline()


def _parse_payload(model_cls: type[Any], payload: dict[str, Any]) -> Any:
    try:
        return model_cls.model_validate(payload)
    except ValidationError as exc:
        errors = exc.errors()
        if errors:
            raise InvalidRequestException(translate_pydantic_error(errors[0]))
        raise InvalidRequestException("请求数据验证失败")


def _serialize_plan(plan: SubscriptionPlan) -> SubscriptionPlanPublicResponse:
    return SubscriptionPlanPublicResponse(
        id=str(plan.id),
        code=str(plan.code),
        name=str(plan.name),
        description=plan.description,
        plan_level=int(plan.plan_level or 0),
        monthly_price_usd=float(plan.monthly_price_usd or 0),
        monthly_quota_usd=float(plan.monthly_quota_usd or 0),
        overage_policy=str(plan.overage_policy),
        term_discounts_json=list(getattr(plan, "term_discounts_json", None) or []),
        is_active=bool(plan.is_active),
        active_subscription_count=0,
        created_at=plan.created_at,
        updated_at=plan.updated_at,
    )


def _serialize_variant(variant: SubscriptionPlan) -> SubscriptionVariantPublicResponse:
    return SubscriptionVariantPublicResponse(
        id=str(variant.id),
        product_id=str(variant.product_id),
        code=str(variant.code),
        name=str(variant.name),
        description=variant.description,
        monthly_price_usd=float(variant.monthly_price_usd or 0),
        monthly_quota_usd=float(variant.monthly_quota_usd or 0),
        variant_rank=int(variant.variant_rank or 0),
        term_discounts_json=list(getattr(variant, "term_discounts_json", None) or []),
        is_active=bool(variant.is_active),
        is_default_variant=bool(variant.is_default_variant),
        active_subscription_count=0,
        created_at=variant.created_at,
        updated_at=variant.updated_at,
    )


def _collect_product_model_names(product: SubscriptionProduct) -> list[str]:
    names: set[str] = set()
    user_group = getattr(product, "user_group", None)
    for binding in list(getattr(user_group, "model_group_links", None) or []):
        if not bool(getattr(binding, "is_active", False)):
            continue
        model_group = getattr(binding, "model_group", None)
        if model_group is None or not bool(getattr(model_group, "is_active", False)):
            continue
        for model_link in list(getattr(model_group, "model_links", None) or []):
            global_model = getattr(model_link, "global_model", None)
            if global_model is None or not bool(getattr(global_model, "is_active", False)):
                continue
            label = getattr(global_model, "display_name", None) or getattr(global_model, "name", None)
            if label:
                names.add(str(label))
    return sorted(names, key=lambda item: item.lower())


def _serialize_product(product: SubscriptionProduct) -> SubscriptionProductPublicResponse:
    active_variants = [
        variant for variant in list(getattr(product, "variants", None) or []) if bool(variant.is_active)
    ]
    return SubscriptionProductPublicResponse(
        id=str(product.id),
        code=str(product.code),
        name=str(product.name),
        description=product.description,
        plan_level=int(product.plan_level or 0),
        overage_policy=str(product.overage_policy),
        is_active=bool(product.is_active),
        variant_count=len(active_variants),
        available_model_names=_collect_product_model_names(product),
        variants=[_serialize_variant(variant) for variant in active_variants],
        created_at=product.created_at,
        updated_at=product.updated_at,
    )


def _serialize_subscription(
    subscription: UserSubscription,
    *,
    display_ends_at: Any | None = None,
) -> UserSubscriptionPublicResponse:
    plan = getattr(subscription, "plan", None)
    product = getattr(plan, "product", None) if plan is not None else None
    remaining_quota = SubscriptionService.get_remaining_quota_value(subscription)
    return UserSubscriptionPublicResponse(
        id=str(subscription.id),
        product_id=(str(getattr(product, "id")) if getattr(product, "id", None) else None),
        product_code=getattr(product, "code", None),
        product_name=getattr(product, "name", None),
        plan_id=str(subscription.plan_id),
        plan_code=getattr(plan, "code", None),
        plan_name=getattr(plan, "name", None),
        variant_id=(str(getattr(plan, "id")) if getattr(plan, "id", None) else None),
        variant_code=getattr(plan, "code", None),
        variant_name=getattr(plan, "name", None),
        variant_rank=(int(getattr(plan, "variant_rank", 0)) if plan is not None else None),
        status=str(subscription.status),
        end_reason=subscription.end_reason,
        purchased_months=int(subscription.purchased_months or 0),
        discount_factor=float(subscription.discount_factor or 0),
        monthly_price_usd_snapshot=float(subscription.monthly_price_usd_snapshot or 0),
        total_price_usd=float(subscription.total_price_usd or 0),
        started_at=subscription.started_at,
        ends_at=display_ends_at if display_ends_at is not None else subscription.ends_at,
        current_cycle_start=subscription.current_cycle_start,
        current_cycle_end=subscription.current_cycle_end,
        cycle_quota_usd=float(subscription.cycle_quota_usd or 0),
        cycle_used_usd=float(subscription.cycle_used_usd or 0),
        remaining_quota_usd=float(remaining_quota),
        cancel_at_period_end=bool(subscription.cancel_at_period_end),
        canceled_at=subscription.canceled_at,
        ended_at=subscription.ended_at,
        upgraded_from_subscription_id=(
            str(subscription.upgraded_from_subscription_id)
            if subscription.upgraded_from_subscription_id
            else None
        ),
        created_at=subscription.created_at,
        updated_at=subscription.updated_at,
    )


def _serialize_subscription_order(order: Any) -> dict[str, Any]:
    payload = serialize_payment_order(order, sanitize_gateway_response=True)
    subscription = getattr(order, "subscription", None)
    plan = getattr(subscription, "plan", None) if subscription is not None else None
    product = getattr(plan, "product", None) if plan is not None else None
    payload.update(
        {
            "subscription_status": getattr(subscription, "status", None),
            "product_id": (str(getattr(product, "id")) if getattr(product, "id", None) else None),
            "product_name": getattr(product, "name", None),
            "plan_id": (str(getattr(plan, "id")) if getattr(plan, "id", None) else None),
            "plan_name": getattr(plan, "name", None),
            "variant_name": getattr(plan, "name", None),
            "purchased_months": getattr(subscription, "purchased_months", None),
            "upgraded_from_subscription_id": (
                str(getattr(subscription, "upgraded_from_subscription_id"))
                if getattr(subscription, "upgraded_from_subscription_id", None)
                else None
            ),
        }
    )
    return payload


def _build_dashboard_sync(user_id: str) -> dict[str, Any]:
    with get_db_context() as db:
        user = db.query(User).filter(User.id == user_id).first()
        if user is None:
            raise InvalidRequestException("未登录")

        current_subscription = SubscriptionService.get_user_current_subscription(db, user.id)
        display_ends_at = SubscriptionService.get_subscription_display_end(db, current_subscription)
        payload = UserSubscriptionDashboardPublicResponse(
            current_subscription=(
                _serialize_subscription(
                    current_subscription,
                    display_ends_at=display_ends_at,
                )
                if current_subscription is not None
                else None
            ),
        )
        return payload.model_dump(mode="json")


def _list_active_plans_sync() -> dict[str, Any]:
    with get_db_context() as db:
        plans = SubscriptionService.list_active_plans(db)
        return {
            "plans": [_serialize_plan(plan).model_dump(mode="json") for plan in plans],
            "total": len(plans),
        }


def _list_active_products_sync() -> dict[str, Any]:
    with get_db_context() as db:
        products = SubscriptionService.list_active_products(db)
        return {
            "products": [_serialize_product(product).model_dump(mode="json") for product in products],
            "total": len(products),
        }


def _list_subscription_orders_sync(user_id: str, limit: int, offset: int) -> dict[str, Any]:
    with get_db_context() as db:
        items, total, _changed = PaymentService.list_user_orders(
            db,
            user_id=user_id,
            limit=limit,
            offset=offset,
            order_types=[
                "subscription_initial",
                "subscription_upgrade",
                "subscription_renewal",
            ],
        )
        return {
            "items": [_serialize_subscription_order(item) for item in items],
            "total": total,
            "limit": limit,
            "offset": offset,
        }


def _cancel_subscription_order_sync(user_id: str, order_id: str) -> dict[str, Any]:
    with get_db_context() as db:
        order = PaymentService.get_user_order(db, user_id=user_id, order_id=order_id)
        if order is None or str(getattr(order, "order_type", "")) not in {
            "subscription_initial",
            "subscription_upgrade",
            "subscription_renewal",
        }:
            raise NotFoundException("订阅订单不存在", "subscription_order")
        if str(getattr(order, "status", "")) not in {"pending", "pending_approval"}:
            raise InvalidRequestException("仅待支付订阅订单支持取消")

        try:
            updated, _changed = PaymentService.expire_order(
                db,
                order=order,
                reason="user_cancelled",
            )
        except ValueError as exc:
            raise InvalidRequestException(str(exc)) from exc

        db.commit()
        reloaded = PaymentService.get_user_order(db, user_id=user_id, order_id=order_id)
        return {"order": _serialize_subscription_order(reloaded or updated)}


def _create_purchase_sync(
    user_id: str,
    req: CreateSubscriptionCheckoutRequest,
) -> dict[str, Any]:
    with get_db_context() as db:
        user = db.query(User).filter(User.id == user_id).first()
        if user is None:
            raise InvalidRequestException("未登录")

        try:
            subscription = SubscriptionService.create_pending_subscription(
                db,
                user_id=user.id,
                plan_id=req.plan_id,
                purchased_months=req.purchased_months,
                commit=False,
            )
            payable_amount = Decimal(str(subscription.total_price_usd or 0))
            order_payload: dict[str, Any] | None = None
            payment_instructions: dict[str, Any] = {}
            if payable_amount <= Decimal("0"):
                subscription = SubscriptionService.activate_pending_subscription(
                    db,
                    subscription.id,
                    commit=False,
                )
                order = PaymentService.create_settled_subscription_order(
                    db,
                    user=user,
                    subscription_id=str(subscription.id),
                    amount_usd=payable_amount,
                    payment_method="system",
                    order_type=SubscriptionService.get_transition_order_type(None, subscription.plan),
                    gateway_response={"gateway": "system", "manual_credit": True},
                )
                order_payload = serialize_payment_order(order, sanitize_gateway_response=True)
            else:
                order = PaymentService.create_subscription_order(
                    db,
                    user=user,
                    subscription_id=str(subscription.id),
                    amount_usd=payable_amount,
                    payment_method=req.payment_method,
                    order_type=SubscriptionService.get_transition_order_type(None, subscription.plan),
                )
                order_payload = serialize_payment_order(order, sanitize_gateway_response=True)
                payment_instructions = safe_gateway_response(order.gateway_response)

            db.flush()
            db.refresh(subscription)
            payload = SubscriptionCheckoutPublicResponse(
                subscription=_serialize_subscription(subscription),
                payable_amount_usd=float(payable_amount),
                order=order_payload,
                payment_instructions=payment_instructions,
            )
            return payload.model_dump(mode="json")
        except ValueError as exc:
            raise InvalidRequestException(str(exc)) from exc


def _create_upgrade_sync(
    user_id: str,
    subscription_id: str,
    req: UpgradeSubscriptionCheckoutRequest,
) -> dict[str, Any]:
    with get_db_context() as db:
        user = db.query(User).filter(User.id == user_id).first()
        if user is None:
            raise InvalidRequestException("未登录")

        current_subscription = SubscriptionService.get_subscription(db, subscription_id, for_update=True)
        if current_subscription is None or str(current_subscription.user_id) != str(user.id):
            raise NotFoundException("用户订阅不存在", "user_subscription")

        try:
            current_plan = current_subscription.plan
            if current_plan is None:
                raise InvalidRequestException("原订阅计划不存在")
            pending_upgrade = SubscriptionService.create_pending_subscription(
                db,
                user_id=user.id,
                plan_id=req.new_plan_id,
                purchased_months=req.purchased_months,
                upgraded_from_subscription_id=current_subscription.id,
                commit=False,
            )
            target_plan = pending_upgrade.plan
            if target_plan is None:
                raise InvalidRequestException("目标订阅计划不存在")
            order_type = SubscriptionService.get_transition_order_type(current_plan, target_plan)
            payable_amount = Decimal(str(pending_upgrade.total_price_usd or 0))
            order_payload: dict[str, Any] | None = None
            payment_instructions: dict[str, Any] = {}
            if payable_amount <= Decimal("0"):
                pending_upgrade = SubscriptionService.activate_pending_subscription(
                    db,
                    pending_upgrade.id,
                    commit=False,
                )
                order = PaymentService.create_settled_subscription_order(
                    db,
                    user=user,
                    subscription_id=str(pending_upgrade.id),
                    amount_usd=payable_amount,
                    payment_method="system",
                    order_type=order_type,
                    gateway_response={"gateway": "system", "manual_credit": True},
                )
                order_payload = serialize_payment_order(order, sanitize_gateway_response=True)
            else:
                order = PaymentService.create_subscription_order(
                    db,
                    user=user,
                    subscription_id=str(pending_upgrade.id),
                    amount_usd=payable_amount,
                    payment_method=req.payment_method,
                    order_type=order_type,
                )
                order_payload = serialize_payment_order(order, sanitize_gateway_response=True)
                payment_instructions = safe_gateway_response(order.gateway_response)

            db.flush()
            db.refresh(pending_upgrade)
            payload = SubscriptionCheckoutPublicResponse(
                subscription=_serialize_subscription(pending_upgrade),
                payable_amount_usd=float(payable_amount),
                order=order_payload,
                payment_instructions=payment_instructions,
            )
            return payload.model_dump(mode="json")
        except ValueError as exc:
            raise InvalidRequestException(str(exc)) from exc


@router.get("/dashboard", response_model=UserSubscriptionDashboardPublicResponse)
async def get_subscription_dashboard(request: Request, db: Session = Depends(get_db)) -> Any:
    adapter = UserSubscriptionDashboardAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/plans")
async def list_available_plans(request: Request, db: Session = Depends(get_db)) -> Any:
    adapter = UserSubscriptionPlansAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/products", response_model=SubscriptionProductPublicListResponse)
async def list_available_products(request: Request, db: Session = Depends(get_db)) -> Any:
    adapter = UserSubscriptionProductsAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/orders")
async def list_subscription_orders(
    request: Request,
    limit: int = Query(50, ge=1, le=200),
    offset: int = Query(0, ge=0, le=5000),
    db: Session = Depends(get_db),
) -> Any:
    adapter = UserSubscriptionOrdersAdapter(limit=limit, offset=offset)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.post("/orders/{order_id}/cancel")
async def cancel_subscription_order(
    order_id: str,
    request: Request,
    db: Session = Depends(get_db),
) -> Any:
    adapter = UserSubscriptionOrderCancelAdapter(order_id=order_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.post("/purchase", response_model=SubscriptionCheckoutPublicResponse)
async def create_subscription_purchase(request: Request, db: Session = Depends(get_db)) -> Any:
    adapter = UserSubscriptionPurchaseAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.post("/{subscription_id}/upgrade", response_model=SubscriptionCheckoutPublicResponse)
async def create_subscription_upgrade(
    subscription_id: str,
    request: Request,
    db: Session = Depends(get_db),
) -> Any:
    adapter = UserSubscriptionUpgradeAdapter(subscription_id=subscription_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


class UserSubscriptionDashboardAdapter(AuthenticatedApiAdapter):
    async def handle(self, context: ApiRequestContext) -> Any:
        user = context.user
        if user is None:
            raise InvalidRequestException("未登录")
        return await run_in_threadpool(_build_dashboard_sync, user.id)


class UserSubscriptionPlansAdapter(AuthenticatedApiAdapter):
    async def handle(self, context: ApiRequestContext) -> Any:
        _ = context
        return await run_in_threadpool(_list_active_plans_sync)


class UserSubscriptionProductsAdapter(AuthenticatedApiAdapter):
    async def handle(self, context: ApiRequestContext) -> Any:
        _ = context
        return await run_in_threadpool(_list_active_products_sync)


@dataclass
class UserSubscriptionOrdersAdapter(AuthenticatedApiAdapter):
    limit: int
    offset: int

    async def handle(self, context: ApiRequestContext) -> Any:
        user = context.user
        if user is None:
            raise InvalidRequestException("未登录")
        return await run_in_threadpool(
            _list_subscription_orders_sync,
            user.id,
            self.limit,
            self.offset,
        )


@dataclass
class UserSubscriptionOrderCancelAdapter(AuthenticatedApiAdapter):
    order_id: str

    async def handle(self, context: ApiRequestContext) -> Any:
        user = context.user
        if user is None:
            raise InvalidRequestException("未登录")
        return await run_in_threadpool(_cancel_subscription_order_sync, user.id, self.order_id)


class UserSubscriptionPurchaseAdapter(AuthenticatedApiAdapter):
    async def handle(self, context: ApiRequestContext) -> Any:
        user = context.user
        if user is None:
            raise InvalidRequestException("未登录")
        payload = _parse_payload(CreateSubscriptionCheckoutRequest, context.ensure_json_body())
        return await run_in_threadpool(_create_purchase_sync, user.id, payload)


@dataclass
class UserSubscriptionUpgradeAdapter(AuthenticatedApiAdapter):
    subscription_id: str

    async def handle(self, context: ApiRequestContext) -> Any:
        user = context.user
        if user is None:
            raise InvalidRequestException("未登录")
        payload = _parse_payload(UpgradeSubscriptionCheckoutRequest, context.ensure_json_body())
        return await run_in_threadpool(
            _create_upgrade_sync,
            user.id,
            self.subscription_id,
            payload,
        )
