"""管理员订阅计划与用户订阅接口。"""

from __future__ import annotations

from dataclasses import dataclass
from decimal import Decimal
from typing import Any

from fastapi import APIRouter, Depends, Query, Request, Response
from pydantic import ValidationError
from sqlalchemy import func
from sqlalchemy.orm import Session, joinedload

from src.api.base.admin_adapter import AdminApiAdapter
from src.api.base.context import ApiRequestContext
from src.api.base.pipeline import get_pipeline
from src.api.serializers import serialize_payment_callback, serialize_payment_order
from src.core.exceptions import InvalidRequestException, NotFoundException, translate_pydantic_error
from src.database import get_db
from src.models.api import (
    CancelUserSubscriptionRequest,
    CreateSubscriptionPlanRequest,
    CreateSubscriptionProductRequest,
    CreateUserSubscriptionRequest,
    SubscriptionProductResponse,
    SubscriptionPlanResponse,
    SubscriptionVariantResponse,
    UpdateSubscriptionPlanRequest,
    UpdateSubscriptionProductRequest,
    UpgradeUserSubscriptionRequest,
    UserSubscriptionResponse,
)
from src.models.database import PaymentOrder, SubscriptionPlan, SubscriptionProduct, User, UserSubscription
from src.services.payment import PaymentService
from src.services.subscription import (
    SUBSCRIPTION_STATUS_ACTIVE,
    SUBSCRIPTION_STATUS_PENDING_PAYMENT,
    SubscriptionService,
)

router = APIRouter(prefix="/api/admin/subscriptions", tags=["Admin - Subscriptions"])
pipeline = get_pipeline()
SUBSCRIPTION_ORDER_TYPES = {
    "subscription_initial",
    "subscription_upgrade",
    "subscription_renewal",
}


def _parse_payload(model_cls: type[Any], payload: dict[str, Any]) -> Any:
    try:
        return model_cls.model_validate(payload)
    except ValidationError as exc:
        errors = exc.errors()
        if errors:
            raise InvalidRequestException(translate_pydantic_error(errors[0]))
        raise InvalidRequestException("请求数据验证失败")


def _serialize_plan(
    db: Session,
    plan: SubscriptionPlan,
    *,
    active_subscription_count: int = 0,
) -> SubscriptionPlanResponse:
    raw_discounts = list(getattr(plan, "term_discounts_json", None) or [])
    return SubscriptionPlanResponse(
        id=str(plan.id),
        code=str(plan.code),
        name=str(plan.name),
        description=plan.description,
        user_group_id=str(plan.user_group_id),
        user_group_name=getattr(getattr(plan, "user_group", None), "name", None),
        plan_level=int(plan.plan_level or 0),
        monthly_price_usd=float(plan.monthly_price_usd or 0),
        monthly_quota_usd=float(plan.monthly_quota_usd or 0),
        overage_policy=str(plan.overage_policy),
        term_discounts_json=raw_discounts,
        is_active=bool(plan.is_active),
        active_subscription_count=int(active_subscription_count),
        created_at=plan.created_at,
        updated_at=plan.updated_at,
    )


def _serialize_variant(
    variant: SubscriptionPlan,
    *,
    active_subscription_count: int = 0,
) -> SubscriptionVariantResponse:
    return SubscriptionVariantResponse(
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
        active_subscription_count=int(active_subscription_count),
        created_at=variant.created_at,
        updated_at=variant.updated_at,
    )


def _serialize_product(
    product: SubscriptionProduct,
    *,
    active_subscription_counts: dict[str, int] | None = None,
) -> SubscriptionProductResponse:
    counts = active_subscription_counts or {}
    variants = list(getattr(product, "variants", None) or [])
    return SubscriptionProductResponse(
        id=str(product.id),
        code=str(product.code),
        name=str(product.name),
        description=product.description,
        user_group_id=str(product.user_group_id),
        user_group_name=getattr(getattr(product, "user_group", None), "name", None),
        plan_level=int(product.plan_level or 0),
        overage_policy=str(product.overage_policy),
        is_active=bool(product.is_active),
        active_subscription_count=sum(counts.get(str(variant.id), 0) for variant in variants),
        variant_count=len(variants),
        variants=[
            _serialize_variant(
                variant,
                active_subscription_count=counts.get(str(variant.id), 0),
            )
            for variant in variants
        ],
        created_at=product.created_at,
        updated_at=product.updated_at,
    )


def _serialize_subscription(
    subscription: UserSubscription,
    *,
    display_ends_at: Any | None = None,
) -> UserSubscriptionResponse:
    plan = getattr(subscription, "plan", None)
    product = getattr(plan, "product", None) if plan is not None else None
    user = getattr(subscription, "user", None)
    remaining_quota = SubscriptionService.get_remaining_quota_value(subscription)
    return UserSubscriptionResponse(
        id=str(subscription.id),
        user_id=str(subscription.user_id),
        username=getattr(user, "username", None),
        email=getattr(user, "email", None),
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
        user_group_id=(str(getattr(plan, "user_group_id")) if getattr(plan, "user_group_id", None) else None),
        user_group_name=getattr(getattr(plan, "user_group", None), "name", None),
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


def _serialize_subscription_order(order: PaymentOrder) -> dict[str, Any]:
    payload = serialize_payment_order(order, sanitize_gateway_response=True)
    user = getattr(order, "user", None)
    subscription = getattr(order, "subscription", None)
    plan = getattr(subscription, "plan", None) if subscription is not None else None
    product = getattr(plan, "product", None) if plan is not None else None
    payload.update(
        {
            "username": getattr(user, "username", None),
            "email": getattr(user, "email", None),
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


@router.get("/plans")
async def list_subscription_plans(request: Request, db: Session = Depends(get_db)) -> Any:
    adapter = AdminListSubscriptionPlansAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/products")
async def list_subscription_products(request: Request, db: Session = Depends(get_db)) -> Any:
    adapter = AdminListSubscriptionProductsAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.post("/products", response_model=SubscriptionProductResponse, status_code=201)
async def create_subscription_product(
    request: Request,
    db: Session = Depends(get_db),
) -> Any:
    adapter = AdminCreateSubscriptionProductAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/products/{product_id}", response_model=SubscriptionProductResponse)
async def get_subscription_product(
    product_id: str,
    request: Request,
    db: Session = Depends(get_db),
) -> Any:
    adapter = AdminGetSubscriptionProductAdapter(product_id=product_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.patch("/products/{product_id}", response_model=SubscriptionProductResponse)
async def update_subscription_product(
    product_id: str,
    request: Request,
    db: Session = Depends(get_db),
) -> Any:
    adapter = AdminUpdateSubscriptionProductAdapter(product_id=product_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.delete("/products/{product_id}", status_code=204, response_class=Response)
async def delete_subscription_product(
    product_id: str,
    request: Request,
    db: Session = Depends(get_db),
) -> Response:
    adapter = AdminDeleteSubscriptionProductAdapter(product_id=product_id)
    await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)
    return Response(status_code=204)


@router.post("/plans", response_model=SubscriptionPlanResponse, status_code=201)
async def create_subscription_plan(
    request: Request,
    db: Session = Depends(get_db),
) -> Any:
    adapter = AdminCreateSubscriptionPlanAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/plans/{plan_id}", response_model=SubscriptionPlanResponse)
async def get_subscription_plan(
    plan_id: str,
    request: Request,
    db: Session = Depends(get_db),
) -> Any:
    adapter = AdminGetSubscriptionPlanAdapter(plan_id=plan_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.patch("/plans/{plan_id}", response_model=SubscriptionPlanResponse)
async def update_subscription_plan(
    plan_id: str,
    request: Request,
    db: Session = Depends(get_db),
) -> Any:
    adapter = AdminUpdateSubscriptionPlanAdapter(plan_id=plan_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.delete("/plans/{plan_id}", status_code=204, response_class=Response)
async def delete_subscription_plan(
    plan_id: str,
    request: Request,
    db: Session = Depends(get_db),
) -> Response:
    adapter = AdminDeleteSubscriptionPlanAdapter(plan_id=plan_id)
    await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)
    return Response(status_code=204)


@router.get("")
async def list_user_subscriptions(
    request: Request,
    status: str | None = Query(None),
    user_id: str | None = Query(None),
    plan_id: str | None = Query(None),
    product_id: str | None = Query(None),
    db: Session = Depends(get_db),
) -> Any:
    adapter = AdminListUserSubscriptionsAdapter(
        status=status,
        user_id=user_id,
        plan_id=plan_id,
        product_id=product_id,
    )
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/orders")
async def list_subscription_orders(
    request: Request,
    status: str | None = Query(None),
    payment_method: str | None = Query(None),
    user_id: str | None = Query(None),
    db: Session = Depends(get_db),
) -> Any:
    adapter = AdminListSubscriptionOrdersAdapter(
        status=status,
        payment_method=payment_method,
        user_id=user_id,
    )
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/callbacks")
async def list_subscription_callbacks(
    request: Request,
    payment_method: str | None = Query(None),
    limit: int = Query(50, ge=1, le=200),
    offset: int = Query(0, ge=0, le=5000),
    db: Session = Depends(get_db),
) -> Any:
    adapter = AdminListSubscriptionCallbacksAdapter(
        payment_method=payment_method,
        limit=limit,
        offset=offset,
    )
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.post("/orders/{order_id}/approve")
async def approve_subscription_order(
    order_id: str,
    request: Request,
    db: Session = Depends(get_db),
) -> Any:
    adapter = AdminApproveSubscriptionOrderAdapter(order_id=order_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.post("/orders/{order_id}/reject")
async def reject_subscription_order(
    order_id: str,
    request: Request,
    db: Session = Depends(get_db),
) -> Any:
    adapter = AdminRejectSubscriptionOrderAdapter(order_id=order_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/users/{user_id}/current", response_model=UserSubscriptionResponse | None)
async def get_user_current_subscription(
    user_id: str,
    request: Request,
    db: Session = Depends(get_db),
) -> Any:
    adapter = AdminGetUserCurrentSubscriptionAdapter(user_id=user_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/{subscription_id}", response_model=UserSubscriptionResponse)
async def get_user_subscription(
    subscription_id: str,
    request: Request,
    db: Session = Depends(get_db),
) -> Any:
    adapter = AdminGetUserSubscriptionAdapter(subscription_id=subscription_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.post("/users/{user_id}", response_model=UserSubscriptionResponse, status_code=201)
async def create_user_subscription(
    user_id: str,
    request: Request,
    db: Session = Depends(get_db),
) -> Any:
    adapter = AdminCreateUserSubscriptionAdapter(user_id=user_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.post("/{subscription_id}/cancel", response_model=UserSubscriptionResponse)
async def cancel_user_subscription(
    subscription_id: str,
    request: Request,
    db: Session = Depends(get_db),
) -> Any:
    adapter = AdminCancelUserSubscriptionAdapter(subscription_id=subscription_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.post("/{subscription_id}/upgrade", response_model=UserSubscriptionResponse)
async def upgrade_user_subscription(
    subscription_id: str,
    request: Request,
    db: Session = Depends(get_db),
) -> Any:
    adapter = AdminUpgradeUserSubscriptionAdapter(subscription_id=subscription_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@dataclass
class AdminListSubscriptionPlansAdapter(AdminApiAdapter):
    async def handle(self, context: ApiRequestContext) -> Any:
        plans = SubscriptionService.list_plans(context.db)
        count_rows = (
            context.db.query(UserSubscription.plan_id, func.count(UserSubscription.id))
            .filter(
                UserSubscription.status.in_(
                    [SUBSCRIPTION_STATUS_PENDING_PAYMENT, SUBSCRIPTION_STATUS_ACTIVE]
                )
            )
            .group_by(UserSubscription.plan_id)
            .all()
        )
        counts = {str(plan_id): int(count) for plan_id, count in count_rows}
        return {
            "plans": [
                _serialize_plan(
                    context.db,
                    plan,
                    active_subscription_count=counts.get(str(plan.id), 0),
                ).model_dump(mode="json")
                for plan in plans
            ],
            "total": len(plans),
        }


@dataclass
class AdminListSubscriptionProductsAdapter(AdminApiAdapter):
    async def handle(self, context: ApiRequestContext) -> Any:
        products = SubscriptionService.list_products(context.db)
        count_rows = (
            context.db.query(UserSubscription.plan_id, func.count(UserSubscription.id))
            .filter(
                UserSubscription.status.in_(
                    [SUBSCRIPTION_STATUS_PENDING_PAYMENT, SUBSCRIPTION_STATUS_ACTIVE]
                )
            )
            .group_by(UserSubscription.plan_id)
            .all()
        )
        counts = {str(plan_id): int(count) for plan_id, count in count_rows}
        return {
            "products": [
                _serialize_product(
                    product,
                    active_subscription_counts=counts,
                ).model_dump(mode="json")
                for product in products
            ],
            "total": len(products),
        }


@dataclass
class AdminCreateSubscriptionPlanAdapter(AdminApiAdapter):
    async def handle(self, context: ApiRequestContext) -> Any:
        payload = _parse_payload(
            CreateSubscriptionPlanRequest,
            context.ensure_json_body(),
        )
        plan = SubscriptionService.create_plan(
            context.db,
            code=payload.code,
            name=payload.name,
            description=payload.description,
            user_group_id=payload.user_group_id,
            plan_level=payload.plan_level,
            monthly_price_usd=payload.monthly_price_usd,
            monthly_quota_usd=payload.monthly_quota_usd,
            overage_policy=payload.overage_policy,
            term_discounts_json=[item.model_dump() for item in payload.term_discounts_json],
        )
        context.add_audit_metadata(action="create_subscription_plan", plan_id=plan.id, plan_code=plan.code)
        return _serialize_plan(context.db, plan).model_dump(mode="json")


@dataclass
class AdminCreateSubscriptionProductAdapter(AdminApiAdapter):
    async def handle(self, context: ApiRequestContext) -> Any:
        payload = _parse_payload(
            CreateSubscriptionProductRequest,
            context.ensure_json_body(),
        )
        product = SubscriptionService.create_product(
            context.db,
            code=payload.code,
            name=payload.name,
            description=payload.description,
            user_group_id=payload.user_group_id,
            plan_level=payload.plan_level,
            overage_policy=payload.overage_policy,
            variants=[item.model_dump() for item in payload.variants],
            is_active=payload.is_active,
        )
        context.add_audit_metadata(
            action="create_subscription_product",
            product_id=product.id,
            product_code=product.code,
        )
        return _serialize_product(product).model_dump(mode="json")


@dataclass
class AdminGetSubscriptionPlanAdapter(AdminApiAdapter):
    plan_id: str

    async def handle(self, context: ApiRequestContext) -> Any:
        plan = SubscriptionService.get_plan(context.db, self.plan_id)
        if plan is None:
            raise NotFoundException("订阅计划不存在", "subscription_plan")
        active_count = int(
            context.db.query(func.count(UserSubscription.id))
            .filter(
                UserSubscription.plan_id == plan.id,
                UserSubscription.status.in_(
                    [SUBSCRIPTION_STATUS_PENDING_PAYMENT, SUBSCRIPTION_STATUS_ACTIVE]
                ),
            )
            .scalar()
            or 0
        )
        return _serialize_plan(
            context.db,
            plan,
            active_subscription_count=active_count,
        ).model_dump(mode="json")


@dataclass
class AdminGetSubscriptionProductAdapter(AdminApiAdapter):
    product_id: str

    async def handle(self, context: ApiRequestContext) -> Any:
        product = SubscriptionService.get_product(context.db, self.product_id)
        if product is None:
            raise NotFoundException("订阅产品不存在", "subscription_product")
        count_rows = (
            context.db.query(UserSubscription.plan_id, func.count(UserSubscription.id))
            .filter(
                UserSubscription.plan_id.in_([variant.id for variant in list(product.variants or [])]),
                UserSubscription.status.in_(
                    [SUBSCRIPTION_STATUS_PENDING_PAYMENT, SUBSCRIPTION_STATUS_ACTIVE]
                ),
            )
            .group_by(UserSubscription.plan_id)
            .all()
        )
        counts = {str(plan_id): int(count) for plan_id, count in count_rows}
        return _serialize_product(
            product,
            active_subscription_counts=counts,
        ).model_dump(mode="json")


@dataclass
class AdminUpdateSubscriptionPlanAdapter(AdminApiAdapter):
    plan_id: str

    async def handle(self, context: ApiRequestContext) -> Any:
        payload = _parse_payload(
            UpdateSubscriptionPlanRequest,
            context.ensure_json_body(),
        )
        update_data = payload.model_dump(exclude_unset=True)
        plan = SubscriptionService.update_plan(context.db, self.plan_id, **update_data)
        if plan is None:
            raise NotFoundException("订阅计划不存在", "subscription_plan")
        context.add_audit_metadata(action="update_subscription_plan", plan_id=plan.id, plan_code=plan.code)
        return _serialize_plan(context.db, plan).model_dump(mode="json")


@dataclass
class AdminUpdateSubscriptionProductAdapter(AdminApiAdapter):
    product_id: str

    async def handle(self, context: ApiRequestContext) -> Any:
        payload = _parse_payload(
            UpdateSubscriptionProductRequest,
            context.ensure_json_body(),
        )
        update_data = payload.model_dump(exclude_unset=True)
        if "variants" in update_data and update_data["variants"] is not None:
            update_data["variants"] = [item for item in update_data["variants"]]
        product = SubscriptionService.update_product(context.db, self.product_id, **update_data)
        if product is None:
            raise NotFoundException("订阅产品不存在", "subscription_product")
        context.add_audit_metadata(
            action="update_subscription_product",
            product_id=product.id,
            product_code=product.code,
        )
        return _serialize_product(product).model_dump(mode="json")


@dataclass
class AdminDeleteSubscriptionPlanAdapter(AdminApiAdapter):
    plan_id: str

    async def handle(self, context: ApiRequestContext) -> Any:
        plan = SubscriptionService.get_plan(context.db, self.plan_id)
        if plan is None:
            raise NotFoundException("订阅计划不存在", "subscription_plan")
        try:
            deleted = SubscriptionService.delete_plan(context.db, self.plan_id)
        except ValueError as exc:
            raise InvalidRequestException(str(exc)) from exc
        if not deleted:
            raise NotFoundException("订阅计划不存在", "subscription_plan")
        context.add_audit_metadata(action="delete_subscription_plan", plan_id=plan.id, plan_code=plan.code)
        return {"message": "订阅计划删除成功"}


@dataclass
class AdminDeleteSubscriptionProductAdapter(AdminApiAdapter):
    product_id: str

    async def handle(self, context: ApiRequestContext) -> Any:
        product = SubscriptionService.get_product(context.db, self.product_id)
        if product is None:
            raise NotFoundException("订阅产品不存在", "subscription_product")
        try:
            deleted = SubscriptionService.delete_product(context.db, self.product_id)
        except ValueError as exc:
            raise InvalidRequestException(str(exc)) from exc
        if not deleted:
            raise NotFoundException("订阅产品不存在", "subscription_product")
        context.add_audit_metadata(
            action="delete_subscription_product",
            product_id=product.id,
            product_code=product.code,
        )
        return {"message": "订阅产品删除成功"}


@dataclass
class AdminListUserSubscriptionsAdapter(AdminApiAdapter):
    status: str | None = None
    user_id: str | None = None
    plan_id: str | None = None
    product_id: str | None = None

    async def handle(self, context: ApiRequestContext) -> Any:
        subscriptions = SubscriptionService.list_subscriptions(
            context.db,
            status=self.status,
            user_id=self.user_id,
            plan_id=self.plan_id,
            product_id=self.product_id,
        )
        return {
            "subscriptions": [
                _serialize_subscription(subscription).model_dump(mode="json")
                for subscription in subscriptions
            ],
            "total": len(subscriptions),
        }


@dataclass
class AdminListSubscriptionOrdersAdapter(AdminApiAdapter):
    status: str | None = None
    payment_method: str | None = None
    user_id: str | None = None

    async def handle(self, context: ApiRequestContext) -> Any:
        PaymentService.expire_overdue_pending_orders(
            context.db,
            user_id=self.user_id,
            payment_method=self.payment_method,
            order_types=tuple(SUBSCRIPTION_ORDER_TYPES),
        )
        orders = (
            context.db.query(PaymentOrder)
            .options(
                joinedload(PaymentOrder.user),
                joinedload(PaymentOrder.subscription)
                .joinedload(UserSubscription.plan)
                .joinedload(SubscriptionPlan.product),
            )
            .filter(PaymentOrder.order_type.in_(tuple(SUBSCRIPTION_ORDER_TYPES)))
        )
        if self.status:
            orders = orders.filter(PaymentOrder.status == self.status)
        if self.payment_method:
            orders = orders.filter(PaymentOrder.payment_method == self.payment_method)
        if self.user_id:
            orders = orders.filter(PaymentOrder.user_id == self.user_id)
        items = orders.order_by(PaymentOrder.created_at.desc(), PaymentOrder.id.desc()).all()
        return {
            "orders": [_serialize_subscription_order(order) for order in items],
            "total": len(items),
        }


@dataclass
class AdminListSubscriptionCallbacksAdapter(AdminApiAdapter):
    payment_method: str | None = None
    limit: int = 50
    offset: int = 0

    async def handle(self, context: ApiRequestContext) -> Any:
        items, total = PaymentService.list_callbacks(
            context.db,
            payment_method=self.payment_method,
            order_types=tuple(SUBSCRIPTION_ORDER_TYPES),
            limit=self.limit,
            offset=self.offset,
        )
        return {
            "items": [serialize_payment_callback(item) for item in items],
            "total": total,
            "limit": self.limit,
            "offset": self.offset,
        }


@dataclass
class AdminApproveSubscriptionOrderAdapter(AdminApiAdapter):
    order_id: str

    async def handle(self, context: ApiRequestContext) -> Any:
        order = PaymentService.get_order(context.db, order_id=self.order_id)
        if order is None or str(order.order_type or "") not in SUBSCRIPTION_ORDER_TYPES:
            raise NotFoundException("订阅订单不存在", "subscription_order")
        if str(order.payment_method or "") not in {"manual", "manual_review"}:
            raise InvalidRequestException("仅人工充值订单支持审批")
        if str(order.status or "") != "pending_approval":
            raise InvalidRequestException("当前订单不处于待审批状态")
        try:
            updated, _credited = PaymentService.credit_order(
                context.db,
                order=order,
                gateway_response={
                    **dict(order.gateway_response or {}),
                    "approved_by": context.user.id if context.user else None,
                },
            )
        except ValueError as exc:
            raise InvalidRequestException(str(exc)) from exc
        context.add_audit_metadata(
            action="approve_subscription_order",
            order_id=updated.id,
            subscription_id=updated.subscription_id,
        )
        reloaded = (
            context.db.query(PaymentOrder)
            .options(
                joinedload(PaymentOrder.user),
                joinedload(PaymentOrder.subscription)
                .joinedload(UserSubscription.plan)
                .joinedload(SubscriptionPlan.product),
            )
            .filter(PaymentOrder.id == updated.id)
            .first()
        )
        return {"order": _serialize_subscription_order(reloaded or updated)}


@dataclass
class AdminRejectSubscriptionOrderAdapter(AdminApiAdapter):
    order_id: str

    async def handle(self, context: ApiRequestContext) -> Any:
        order = PaymentService.get_order(context.db, order_id=self.order_id)
        if order is None or str(order.order_type or "") not in SUBSCRIPTION_ORDER_TYPES:
            raise NotFoundException("订阅订单不存在", "subscription_order")
        if str(order.payment_method or "") not in {"manual", "manual_review"}:
            raise InvalidRequestException("仅人工充值订单支持审批")
        if str(order.status or "") != "pending_approval":
            raise InvalidRequestException("当前订单不处于待审批状态")
        try:
            updated = PaymentService.fail_order(
                context.db,
                order=order,
                reason="admin_rejected",
            )
        except ValueError as exc:
            raise InvalidRequestException(str(exc)) from exc
        context.add_audit_metadata(
            action="reject_subscription_order",
            order_id=updated.id,
            subscription_id=updated.subscription_id,
        )
        reloaded = (
            context.db.query(PaymentOrder)
            .options(
                joinedload(PaymentOrder.user),
                joinedload(PaymentOrder.subscription)
                .joinedload(UserSubscription.plan)
                .joinedload(SubscriptionPlan.product),
            )
            .filter(PaymentOrder.id == updated.id)
            .first()
        )
        return {"order": _serialize_subscription_order(reloaded or updated)}


@dataclass
class AdminGetUserSubscriptionAdapter(AdminApiAdapter):
    subscription_id: str

    async def handle(self, context: ApiRequestContext) -> Any:
        subscription = SubscriptionService.get_subscription(context.db, self.subscription_id)
        if subscription is None:
            raise NotFoundException("用户订阅不存在", "user_subscription")
        return _serialize_subscription(subscription).model_dump(mode="json")


@dataclass
class AdminGetUserCurrentSubscriptionAdapter(AdminApiAdapter):
    user_id: str

    async def handle(self, context: ApiRequestContext) -> Any:
        user = context.db.query(User).filter(User.id == self.user_id).first()
        if user is None:
            raise NotFoundException("用户不存在", "user")
        subscription = SubscriptionService.get_user_current_subscription(context.db, self.user_id)
        if subscription is None:
            return None
        display_ends_at = SubscriptionService.get_subscription_display_end(context.db, subscription)
        return _serialize_subscription(
            subscription,
            display_ends_at=display_ends_at,
        ).model_dump(mode="json")


@dataclass
class AdminCreateUserSubscriptionAdapter(AdminApiAdapter):
    user_id: str

    async def handle(self, context: ApiRequestContext) -> Any:
        payload = _parse_payload(CreateUserSubscriptionRequest, context.ensure_json_body())
        try:
            user = context.db.query(User).filter(User.id == self.user_id).first()
            if user is None:
                raise NotFoundException("用户不存在", "user")
            subscription = SubscriptionService.create_subscription(
                context.db,
                user_id=self.user_id,
                plan_id=payload.plan_id,
                purchased_months=payload.purchased_months,
                started_at=payload.started_at,
            )
            PaymentService.create_settled_subscription_order(
                context.db,
                user=user,
                subscription_id=str(subscription.id),
                amount_usd=subscription.total_price_usd,
                payment_method="admin_subscription",
                order_type="subscription_initial",
                gateway_response={"gateway": "admin_subscription", "manual_credit": True},
            )
        except ValueError as exc:
            raise InvalidRequestException(str(exc)) from exc
        context.add_audit_metadata(
            action="create_user_subscription",
            subscription_id=subscription.id,
            user_id=self.user_id,
            plan_id=payload.plan_id,
        )
        subscription = SubscriptionService.get_subscription(context.db, subscription.id)
        if subscription is None:
            raise NotFoundException("用户订阅不存在", "user_subscription")
        return _serialize_subscription(subscription).model_dump(mode="json")


@dataclass
class AdminCancelUserSubscriptionAdapter(AdminApiAdapter):
    subscription_id: str

    async def handle(self, context: ApiRequestContext) -> Any:
        payload = _parse_payload(CancelUserSubscriptionRequest, context.ensure_json_body())
        try:
            subscription = SubscriptionService.cancel_subscription(
                context.db,
                self.subscription_id,
                immediate=payload.immediate,
            )
        except ValueError as exc:
            raise InvalidRequestException(str(exc)) from exc
        if subscription is None:
            raise NotFoundException("用户订阅不存在", "user_subscription")
        context.add_audit_metadata(
            action="cancel_user_subscription",
            subscription_id=subscription.id,
            immediate=payload.immediate,
        )
        subscription = SubscriptionService.get_subscription(context.db, subscription.id)
        if subscription is None:
            raise NotFoundException("用户订阅不存在", "user_subscription")
        return _serialize_subscription(subscription).model_dump(mode="json")


@dataclass
class AdminUpgradeUserSubscriptionAdapter(AdminApiAdapter):
    subscription_id: str

    async def handle(self, context: ApiRequestContext) -> Any:
        payload = _parse_payload(UpgradeUserSubscriptionRequest, context.ensure_json_body())
        try:
            current_subscription = SubscriptionService.get_subscription(
                context.db,
                self.subscription_id,
                for_update=True,
            )
            if current_subscription is None:
                raise NotFoundException("用户订阅不存在", "user_subscription")
            current_plan = current_subscription.plan
            if current_plan is None:
                raise InvalidRequestException("原订阅计划不存在")
            user = current_subscription.user
            if user is None:
                raise InvalidRequestException("订阅所属用户不存在")
            subscription = SubscriptionService.create_pending_subscription(
                context.db,
                user_id=str(current_subscription.user_id),
                plan_id=payload.new_plan_id,
                purchased_months=payload.purchased_months,
                upgraded_from_subscription_id=current_subscription.id,
                commit=False,
            )
            target_plan = subscription.plan
            if target_plan is None:
                raise InvalidRequestException("目标订阅计划不存在")
            payable_amount = Decimal(str(subscription.total_price_usd or 0))
            order_type = SubscriptionService.get_transition_order_type(current_plan, target_plan)
            subscription = SubscriptionService.activate_pending_subscription(
                context.db,
                subscription.id,
                commit=False,
            )
            PaymentService.create_settled_subscription_order(
                context.db,
                user=user,
                subscription_id=str(subscription.id),
                amount_usd=payable_amount,
                payment_method="admin_subscription",
                order_type=order_type,
                gateway_response={"gateway": "admin_subscription", "manual_credit": True},
            )
        except ValueError as exc:
            raise InvalidRequestException(str(exc)) from exc
        context.add_audit_metadata(
            action="upgrade_user_subscription",
            subscription_id=subscription.id,
            upgraded_from_subscription_id=subscription.upgraded_from_subscription_id,
            payable_amount_usd=float(payable_amount),
        )
        subscription = SubscriptionService.get_subscription(context.db, subscription.id)
        if subscription is None:
            raise NotFoundException("用户订阅不存在", "user_subscription")
        return _serialize_subscription(subscription).model_dump(mode="json")
