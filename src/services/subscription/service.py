"""订阅计划与用户订阅服务。"""

from __future__ import annotations

import calendar
from dataclasses import dataclass
from datetime import datetime, timezone
from decimal import Decimal
from typing import Any

from sqlalchemy.orm import Session, joinedload, selectinload

from src.models.database import (
    GlobalModel,
    ModelGroup,
    ModelGroupModel,
    SubscriptionPlan,
    SubscriptionProduct,
    User,
    UserGroup,
    UserGroupModelGroup,
    UserSubscription,
)
from src.services.billing.precision import to_money_decimal
from src.services.user.group_service import UserGroupService
from src.utils.transaction_manager import retry_on_database_error, transactional

SUBSCRIPTION_STATUS_PENDING_PAYMENT = "pending_payment"
SUBSCRIPTION_STATUS_ACTIVE = "active"
SUBSCRIPTION_STATUS_CANCELED = "canceled"
SUBSCRIPTION_STATUS_EXPIRED = "expired"

SUBSCRIPTION_TRANSITION_RENEWAL = "renewal"
SUBSCRIPTION_TRANSITION_UPGRADE = "upgrade"

SUBSCRIPTION_ORDER_TYPE_INITIAL = "subscription_initial"
SUBSCRIPTION_ORDER_TYPE_UPGRADE = "subscription_upgrade"
SUBSCRIPTION_ORDER_TYPE_RENEWAL = "subscription_renewal"

SUBSCRIPTION_END_REASON_UPGRADE = "upgrade"
SUBSCRIPTION_END_REASON_USER_CANCEL = "user_cancel"
SUBSCRIPTION_END_REASON_ADMIN_CANCEL = "admin_cancel"
SUBSCRIPTION_END_REASON_PAYMENT_FAILED = "payment_failed"
SUBSCRIPTION_END_REASON_EXPIRED = "expired"

SUBSCRIPTION_OVERAGE_POLICY_BLOCK = "block"
SUBSCRIPTION_OVERAGE_POLICY_USE_WALLET = "use_wallet_balance"


@dataclass(slots=True, frozen=True)
class SubscriptionChargeResult:
    subscription: UserSubscription | None
    quota_before: Decimal | None
    quota_after: Decimal | None
    consumed_from_quota: Decimal
    wallet_charge_amount: Decimal


class SubscriptionService:
    """订阅服务。"""

    @classmethod
    def _subscription_loader_options(
        cls,
        *,
        include_user: bool,
        for_update: bool,
    ) -> tuple[Any, ...]:
        if for_update:
            options: list[Any] = [
                selectinload(UserSubscription.plan).selectinload(SubscriptionPlan.user_group),
                selectinload(UserSubscription.plan).selectinload(SubscriptionPlan.product),
            ]
            if include_user:
                options.append(selectinload(UserSubscription.user).selectinload(User.group))
            return tuple(options)

        options = [
            joinedload(UserSubscription.plan).joinedload(SubscriptionPlan.user_group),
            joinedload(UserSubscription.plan).joinedload(SubscriptionPlan.product),
        ]
        if include_user:
            options.append(joinedload(UserSubscription.user).joinedload(User.group))
        return tuple(options)

    @staticmethod
    def _utc_now() -> datetime:
        return datetime.now(timezone.utc)

    @staticmethod
    def _ensure_utc(value: datetime) -> datetime:
        if value.tzinfo is None:
            return value.replace(tzinfo=timezone.utc)
        return value.astimezone(timezone.utc)

    @staticmethod
    def _add_months(value: datetime, months: int) -> datetime:
        normalized = SubscriptionService._ensure_utc(value)
        month_index = normalized.month - 1 + months
        year = normalized.year + month_index // 12
        month = month_index % 12 + 1
        day = min(normalized.day, calendar.monthrange(year, month)[1])
        return normalized.replace(year=year, month=month, day=day)

    @classmethod
    def _build_subscription_periods(
        cls,
        *,
        start: datetime,
        purchased_months: int,
    ) -> tuple[datetime, datetime]:
        end = cls._add_months(start, purchased_months)
        current_cycle_end = cls._add_months(start, 1)
        if current_cycle_end > end:
            current_cycle_end = end
        return end, current_cycle_end

    @staticmethod
    def _normalize_term_discounts(raw: Any) -> list[dict[str, Any]]:
        items = raw if isinstance(raw, list) else []
        normalized: list[dict[str, Any]] = []
        seen: set[int] = set()
        for item in items:
            if not isinstance(item, dict):
                continue
            try:
                months = int(item.get("months") or 0)
                factor = float(item.get("discount_factor") or 0)
            except (TypeError, ValueError):
                continue
            if months <= 0 or factor <= 0 or months in seen:
                continue
            seen.add(months)
            normalized.append(
                {
                    "months": months,
                    "discount_factor": round(factor, 4),
                }
            )
        if 1 not in seen:
            normalized.append({"months": 1, "discount_factor": 1.0})
        normalized.sort(key=lambda item: int(item["months"]))
        return normalized

    @staticmethod
    def _normalize_optional_text(value: Any) -> str | None:
        if value is None:
            return None
        normalized = str(value).strip()
        return normalized or None

    @classmethod
    def _normalize_variant_payloads(
        cls,
        variants: list[dict[str, Any]] | None,
    ) -> list[dict[str, Any]]:
        if not variants:
            raise ValueError("至少需要一个订阅版本")

        normalized: list[dict[str, Any]] = []
        seen_codes: set[str] = set()
        default_count = 0
        for index, raw in enumerate(variants):
            if not isinstance(raw, dict):
                raise ValueError("订阅版本配置格式无效")
            variant_id = cls._normalize_optional_text(raw.get("id"))
            code = cls._normalize_optional_text(raw.get("code"))
            name = cls._normalize_optional_text(raw.get("name"))
            if not code or not name:
                raise ValueError("订阅版本编码和名称不能为空")
            if code in seen_codes:
                raise ValueError(f"订阅版本编码重复: {code}")
            seen_codes.add(code)
            is_default_variant = bool(raw.get("is_default_variant", False))
            if is_default_variant:
                default_count += 1
            normalized.append(
                {
                    "id": variant_id,
                    "code": code,
                    "name": name,
                    "description": cls._normalize_optional_text(raw.get("description")),
                    "monthly_price_usd": to_money_decimal(raw.get("monthly_price_usd", 0)),
                    "monthly_quota_usd": to_money_decimal(raw.get("monthly_quota_usd", 0)),
                    "variant_rank": int(raw.get("variant_rank", 100) or 100),
                    "term_discounts_json": cls._normalize_term_discounts(
                        raw.get("term_discounts_json")
                    ),
                    "is_active": bool(raw.get("is_active", True)),
                    "is_default_variant": is_default_variant,
                    "_position": index,
                }
            )

        if default_count > 1:
            raise ValueError("只能设置一个默认订阅版本")
        if default_count == 0 and normalized:
            normalized.sort(key=lambda item: (int(item["variant_rank"]), int(item["_position"])))
            normalized[0]["is_default_variant"] = True

        normalized.sort(key=lambda item: (int(item["variant_rank"]), int(item["_position"])))
        return normalized

    @classmethod
    def _ensure_variant_codes_available(
        cls,
        db: Session,
        variants: list[dict[str, Any]],
    ) -> None:
        for item in variants:
            existing = cls.get_plan_by_code(db, str(item["code"]))
            if existing is None:
                continue
            if item.get("id") and str(existing.id) == str(item["id"]):
                continue
            raise ValueError(f"订阅版本编码已存在: {item['code']}")

    @staticmethod
    def _sync_variant_shared_fields(
        variant: SubscriptionPlan,
        *,
        product: SubscriptionProduct,
    ) -> None:
        variant.product_id = product.id
        variant.product = product
        variant.user_group_id = product.user_group_id
        variant.user_group = product.user_group
        variant.plan_level = int(product.plan_level or 0)
        variant.overage_policy = str(product.overage_policy or SUBSCRIPTION_OVERAGE_POLICY_BLOCK)

    @classmethod
    def resolve_discount_rule(
        cls,
        plan: SubscriptionPlan,
        purchased_months: int,
    ) -> dict[str, Any]:
        if purchased_months <= 0:
            raise ValueError("购买月数必须大于 0")
        normalized = cls._normalize_term_discounts(getattr(plan, "term_discounts_json", None))
        matched = normalized[0]
        for item in normalized:
            if int(item["months"]) > int(purchased_months):
                break
            matched = item
        return matched

    @classmethod
    def resolve_discount_factor(cls, plan: SubscriptionPlan, purchased_months: int) -> Decimal:
        matched = cls.resolve_discount_rule(plan, purchased_months)
        return Decimal(str(matched["discount_factor"]))

    @classmethod
    def compute_total_price(
        cls,
        plan: SubscriptionPlan,
        purchased_months: int,
    ) -> tuple[Decimal, Decimal, Decimal]:
        if purchased_months <= 0:
            raise ValueError("购买月数必须大于 0")
        monthly_price = to_money_decimal(plan.monthly_price_usd)
        discount_factor = cls.resolve_discount_factor(plan, purchased_months)
        total_price = monthly_price * Decimal(purchased_months) * discount_factor
        return monthly_price, discount_factor, total_price

    @staticmethod
    def get_remaining_quota_value(subscription: UserSubscription | None) -> Decimal:
        if subscription is None:
            return Decimal("0")
        quota = to_money_decimal(getattr(subscription, "cycle_quota_usd", None))
        used = to_money_decimal(getattr(subscription, "cycle_used_usd", None))
        remaining = quota - used
        return remaining if remaining > Decimal("0") else Decimal("0")

    @classmethod
    def _is_effective_subscription(
        cls,
        subscription: UserSubscription | None,
        *,
        now: datetime | None = None,
    ) -> bool:
        if subscription is None:
            return False
        current = now or cls._utc_now()
        status = str(getattr(subscription, "status", "") or "")
        if status != SUBSCRIPTION_STATUS_ACTIVE:
            return False
        started_at = getattr(subscription, "started_at", None)
        ends_at = getattr(subscription, "ends_at", None)
        if not isinstance(started_at, datetime) or not isinstance(ends_at, datetime):
            return False
        return cls._ensure_utc(started_at) <= current < cls._ensure_utc(ends_at)

    @classmethod
    def _load_active_subscription_from_user(
        cls,
        user: User | None,
        *,
        now: datetime | None = None,
    ) -> UserSubscription | None:
        if user is None:
            return None
        subscriptions = list(getattr(user, "subscriptions", None) or [])
        if not subscriptions:
            return None
        current = now or cls._utc_now()
        subscriptions.sort(
            key=lambda item: (
                cls._ensure_utc(getattr(item, "created_at", current))
                if getattr(item, "created_at", None)
                else current
            ),
            reverse=True,
        )
        for subscription in subscriptions:
            if cls._is_effective_subscription(subscription, now=current):
                return subscription
        return None

    @classmethod
    def resolve_effective_user_group_from_user(cls, user: User | None) -> UserGroup | None:
        """从已加载的用户对象解析当前生效用户分组。"""
        active_subscription = cls._load_active_subscription_from_user(user)
        if active_subscription is not None:
            plan = getattr(active_subscription, "plan", None)
            group = getattr(plan, "user_group", None) if plan is not None else None
            if group is not None and bool(getattr(group, "is_active", True)):
                return group
        if user is None:
            return None
        group = getattr(user, "group", None)
        return group if group is not None else None

    @classmethod
    def resolve_effective_user_group(cls, db: Session, user: User | None) -> UserGroup | None:
        """从数据库解析当前生效用户分组。"""
        if user is None:
            return None

        active_subscription = cls.get_active_subscription(
            db,
            user_id=str(user.id),
            for_update=False,
        )
        if active_subscription is not None and active_subscription.plan is not None:
            group = active_subscription.plan.user_group
            if group is not None and bool(getattr(group, "is_active", True)):
                return group

        group = getattr(user, "group", None)
        if group is not None:
            return group
        return UserGroupService.get_default_group(db)

    @classmethod
    def _refresh_subscription_state(
        cls,
        db: Session,
        subscription: UserSubscription,
        *,
        now: datetime | None = None,
    ) -> UserSubscription:
        current = now or cls._utc_now()
        current = cls._ensure_utc(current)

        if subscription.status in {SUBSCRIPTION_STATUS_CANCELED, SUBSCRIPTION_STATUS_EXPIRED}:
            return subscription

        ends_at = cls._ensure_utc(subscription.ends_at)
        if ends_at <= current:
            subscription.status = SUBSCRIPTION_STATUS_EXPIRED
            subscription.end_reason = subscription.end_reason or SUBSCRIPTION_END_REASON_EXPIRED
            subscription.ended_at = subscription.ended_at or ends_at
            subscription.updated_at = current
            db.flush()
            return subscription

        plan = subscription.plan
        monthly_quota = (
            to_money_decimal(plan.monthly_quota_usd)
            if plan is not None
            else to_money_decimal(subscription.cycle_quota_usd)
        )
        cycle_end = cls._ensure_utc(subscription.current_cycle_end)
        rotated = False
        while cycle_end <= current and cycle_end < ends_at:
            next_cycle_start = cycle_end
            next_cycle_end = cls._add_months(next_cycle_start, 1)
            if next_cycle_end > ends_at:
                next_cycle_end = ends_at
            subscription.current_cycle_start = next_cycle_start
            subscription.current_cycle_end = next_cycle_end
            subscription.cycle_quota_usd = monthly_quota
            subscription.cycle_used_usd = Decimal("0")
            cycle_end = next_cycle_end
            rotated = True

        if rotated:
            subscription.updated_at = current
            db.flush()
        return subscription

    @classmethod
    def get_product(cls, db: Session, product_id: str) -> SubscriptionProduct | None:
        return (
            db.query(SubscriptionProduct)
            .options(
                joinedload(SubscriptionProduct.user_group)
                .selectinload(UserGroup.model_group_links)
                .selectinload(UserGroupModelGroup.model_group)
                .selectinload(ModelGroup.model_links)
                .selectinload(ModelGroupModel.global_model),
                selectinload(SubscriptionProduct.variants),
            )
            .filter(SubscriptionProduct.id == product_id)
            .first()
        )

    @classmethod
    def get_product_by_code(cls, db: Session, code: str) -> SubscriptionProduct | None:
        return (
            db.query(SubscriptionProduct)
            .options(
                joinedload(SubscriptionProduct.user_group)
                .selectinload(UserGroup.model_group_links)
                .selectinload(UserGroupModelGroup.model_group)
                .selectinload(ModelGroup.model_links)
                .selectinload(ModelGroupModel.global_model),
                selectinload(SubscriptionProduct.variants),
            )
            .filter(SubscriptionProduct.code == code)
            .first()
        )

    @classmethod
    def list_products(cls, db: Session) -> list[SubscriptionProduct]:
        return (
            db.query(SubscriptionProduct)
            .options(
                joinedload(SubscriptionProduct.user_group)
                .selectinload(UserGroup.model_group_links)
                .selectinload(UserGroupModelGroup.model_group)
                .selectinload(ModelGroup.model_links)
                .selectinload(ModelGroupModel.global_model),
                selectinload(SubscriptionProduct.variants),
            )
            .order_by(
                SubscriptionProduct.plan_level.asc(),
                SubscriptionProduct.created_at.asc(),
                SubscriptionProduct.id.asc(),
            )
            .all()
        )

    @classmethod
    def get_plan(cls, db: Session, plan_id: str) -> SubscriptionPlan | None:
        return (
            db.query(SubscriptionPlan)
            .options(
                joinedload(SubscriptionPlan.user_group),
                joinedload(SubscriptionPlan.product).joinedload(SubscriptionProduct.user_group),
            )
            .filter(SubscriptionPlan.id == plan_id)
            .first()
        )

    @classmethod
    def get_plan_by_code(cls, db: Session, code: str) -> SubscriptionPlan | None:
        return (
            db.query(SubscriptionPlan)
            .options(
                joinedload(SubscriptionPlan.user_group),
                joinedload(SubscriptionPlan.product).joinedload(SubscriptionProduct.user_group),
            )
            .filter(SubscriptionPlan.code == code)
            .first()
        )

    @classmethod
    def list_plans(cls, db: Session) -> list[SubscriptionPlan]:
        return (
            db.query(SubscriptionPlan)
            .options(
                joinedload(SubscriptionPlan.user_group),
                joinedload(SubscriptionPlan.product).joinedload(SubscriptionProduct.user_group),
            )
            .order_by(
                SubscriptionPlan.plan_level.asc(),
                SubscriptionPlan.variant_rank.asc(),
                SubscriptionPlan.created_at.asc(),
                SubscriptionPlan.id.asc(),
            )
            .all()
        )

    @classmethod
    @transactional()
    @retry_on_database_error(max_retries=3)
    def create_product(
        cls,
        db: Session,
        *,
        code: str,
        name: str,
        description: str | None,
        user_group_id: str,
        plan_level: int,
        overage_policy: str,
        variants: list[dict[str, Any]],
        is_active: bool = True,
        commit: bool = True,
    ) -> SubscriptionProduct:
        normalized_code = str(code or "").strip()
        if not normalized_code:
            raise ValueError("订阅产品编码不能为空")
        if cls.get_product_by_code(db, normalized_code):
            raise ValueError(f"订阅产品编码已存在: {normalized_code}")

        user_group = db.query(UserGroup).filter(UserGroup.id == user_group_id).first()
        if user_group is None:
            raise ValueError("订阅产品绑定的用户分组不存在")
        if overage_policy not in {
            SUBSCRIPTION_OVERAGE_POLICY_BLOCK,
            SUBSCRIPTION_OVERAGE_POLICY_USE_WALLET,
        }:
            raise ValueError("不支持的订阅超额策略")

        normalized_variants = cls._normalize_variant_payloads(variants)
        cls._ensure_variant_codes_available(db, normalized_variants)

        product = SubscriptionProduct(
            code=normalized_code,
            name=str(name or "").strip(),
            description=cls._normalize_optional_text(description),
            user_group_id=user_group.id,
            plan_level=int(plan_level),
            overage_policy=overage_policy,
            is_active=bool(is_active),
        )
        db.add(product)
        db.flush()

        for item in normalized_variants:
            variant = SubscriptionPlan(
                product_id=product.id,
                code=str(item["code"]),
                name=str(item["name"]),
                description=item["description"],
                monthly_price_usd=item["monthly_price_usd"],
                monthly_quota_usd=item["monthly_quota_usd"],
                variant_rank=int(item["variant_rank"]),
                is_default_variant=bool(item["is_default_variant"]),
                term_discounts_json=item["term_discounts_json"],
                is_active=bool(item["is_active"]),
            )
            cls._sync_variant_shared_fields(variant, product=product)
            db.add(variant)

        if commit:
            db.commit()
            db.refresh(product)
        else:
            db.flush()
        return cls.get_product(db, str(product.id)) or product

    @classmethod
    @transactional()
    @retry_on_database_error(max_retries=3)
    def update_product(
        cls,
        db: Session,
        product_id: str,
        *,
        commit: bool = True,
        **kwargs: Any,
    ) -> SubscriptionProduct | None:
        product = cls.get_product(db, product_id)
        if product is None:
            return None

        if "code" in kwargs and kwargs["code"] is not None:
            normalized_code = str(kwargs["code"]).strip()
            if not normalized_code:
                raise ValueError("订阅产品编码不能为空")
            existing = cls.get_product_by_code(db, normalized_code)
            if existing is not None and str(existing.id) != str(product.id):
                raise ValueError(f"订阅产品编码已存在: {normalized_code}")
            product.code = normalized_code

        if "user_group_id" in kwargs and kwargs["user_group_id"] is not None:
            user_group = db.query(UserGroup).filter(UserGroup.id == kwargs["user_group_id"]).first()
            if user_group is None:
                raise ValueError("订阅产品绑定的用户分组不存在")
            product.user_group_id = user_group.id
            product.user_group = user_group

        if "overage_policy" in kwargs and kwargs["overage_policy"] is not None:
            overage_policy = str(kwargs["overage_policy"] or "").strip()
            if overage_policy not in {
                SUBSCRIPTION_OVERAGE_POLICY_BLOCK,
                SUBSCRIPTION_OVERAGE_POLICY_USE_WALLET,
            }:
                raise ValueError("不支持的订阅超额策略")
            product.overage_policy = overage_policy

        field_map = {
            "name": lambda value: str(value).strip(),
            "description": cls._normalize_optional_text,
            "plan_level": lambda value: int(value),
            "is_active": bool,
        }
        for field, converter in field_map.items():
            if field not in kwargs or kwargs[field] is None:
                continue
            setattr(product, field, converter(kwargs[field]))

        existing_variants = {str(variant.id): variant for variant in list(product.variants or [])}
        if "variants" in kwargs and kwargs["variants"] is not None:
            normalized_variants = cls._normalize_variant_payloads(kwargs["variants"])
            cls._ensure_variant_codes_available(db, normalized_variants)
            submitted_ids = {str(item["id"]) for item in normalized_variants if item.get("id")}
            existing_ids = set(existing_variants.keys())
            removed_ids = existing_ids - submitted_ids
            if removed_ids:
                removable_count = (
                    db.query(UserSubscription)
                    .filter(UserSubscription.plan_id.in_(list(removed_ids)))
                    .count()
                )
                if removable_count > 0:
                    raise ValueError("存在已使用过的订阅版本，不能删除")
                for removed_id in removed_ids:
                    db.delete(existing_variants[removed_id])

            for item in normalized_variants:
                variant: SubscriptionPlan
                if item.get("id"):
                    variant = existing_variants.get(str(item["id"]))  # type: ignore[assignment]
                    if variant is None:
                        raise ValueError("订阅版本不存在或不属于当前产品")
                else:
                    variant = SubscriptionPlan(product_id=product.id)
                    db.add(variant)
                variant.code = str(item["code"])
                variant.name = str(item["name"])
                variant.description = item["description"]
                variant.monthly_price_usd = item["monthly_price_usd"]
                variant.monthly_quota_usd = item["monthly_quota_usd"]
                variant.variant_rank = int(item["variant_rank"])
                variant.is_default_variant = bool(item["is_default_variant"])
                variant.term_discounts_json = item["term_discounts_json"]
                variant.is_active = bool(item["is_active"])
                cls._sync_variant_shared_fields(variant, product=product)
        else:
            for variant in list(product.variants or []):
                cls._sync_variant_shared_fields(variant, product=product)

        product.updated_at = cls._utc_now()
        if commit:
            db.commit()
            db.refresh(product)
        else:
            db.flush()
        return cls.get_product(db, str(product.id)) or product

    @classmethod
    @transactional()
    @retry_on_database_error(max_retries=3)
    def delete_product(cls, db: Session, product_id: str) -> bool:
        product = cls.get_product(db, product_id)
        if product is None:
            return False
        variant_ids = [str(variant.id) for variant in list(product.variants or [])]
        if variant_ids:
            subscription_count = (
                db.query(UserSubscription)
                .filter(UserSubscription.plan_id.in_(variant_ids))
                .count()
            )
            if subscription_count > 0:
                raise ValueError("该订阅产品存在订阅记录，不能删除")
        db.delete(product)
        db.commit()
        return True

    @classmethod
    @transactional()
    @retry_on_database_error(max_retries=3)
    def create_plan(
        cls,
        db: Session,
        *,
        code: str,
        name: str,
        description: str | None,
        user_group_id: str,
        plan_level: int,
        monthly_price_usd: Decimal | float | int | str,
        monthly_quota_usd: Decimal | float | int | str,
        overage_policy: str,
        term_discounts_json: list[dict[str, Any]] | None,
        is_active: bool = True,
        commit: bool = True,
    ) -> SubscriptionPlan:
        product = cls.create_product(
            db,
            code=code,
            name=name,
            description=description,
            user_group_id=user_group_id,
            plan_level=plan_level,
            overage_policy=overage_policy,
            variants=[
                {
                    "code": code,
                    "name": name,
                    "description": description,
                    "monthly_price_usd": monthly_price_usd,
                    "monthly_quota_usd": monthly_quota_usd,
                    "variant_rank": 100,
                    "term_discounts_json": term_discounts_json or [],
                    "is_active": is_active,
                    "is_default_variant": True,
                }
            ],
            is_active=is_active,
            commit=commit,
        )
        variant = next((item for item in list(product.variants or []) if item.is_default_variant), None)
        if variant is None:
            variant = list(product.variants or [])[0]
        return variant

    @classmethod
    @transactional()
    @retry_on_database_error(max_retries=3)
    def update_plan(
        cls,
        db: Session,
        plan_id: str,
        *,
        commit: bool = True,
        **kwargs: Any,
    ) -> SubscriptionPlan | None:
        plan = cls.get_plan(db, plan_id)
        if plan is None:
            return None
        product = plan.product
        if product is None:
            raise ValueError("订阅产品不存在")
        if len(list(product.variants or [])) > 1:
            raise ValueError("多版本订阅产品请使用订阅产品接口更新")

        updated_product = cls.update_product(
            db,
            str(product.id),
            code=kwargs.get("code", product.code),
            name=kwargs.get("name", product.name),
            description=kwargs.get("description", product.description),
            user_group_id=kwargs.get("user_group_id", product.user_group_id),
            plan_level=kwargs.get("plan_level", product.plan_level),
            overage_policy=kwargs.get("overage_policy", product.overage_policy),
            is_active=kwargs.get("is_active", product.is_active),
            variants=[
                {
                    "id": str(plan.id),
                    "code": kwargs.get("code", plan.code),
                    "name": kwargs.get("name", plan.name),
                    "description": kwargs.get("description", plan.description),
                    "monthly_price_usd": kwargs.get("monthly_price_usd", plan.monthly_price_usd),
                    "monthly_quota_usd": kwargs.get("monthly_quota_usd", plan.monthly_quota_usd),
                    "variant_rank": kwargs.get("variant_rank", plan.variant_rank),
                    "term_discounts_json": kwargs.get(
                        "term_discounts_json", plan.term_discounts_json
                    ),
                    "is_active": kwargs.get("is_active", plan.is_active),
                    "is_default_variant": True,
                }
            ],
            commit=commit,
        )
        if updated_product is None:
            return None
        return next((item for item in list(updated_product.variants or []) if str(item.id) == str(plan.id)), None)

    @classmethod
    @transactional()
    @retry_on_database_error(max_retries=3)
    def delete_plan(cls, db: Session, plan_id: str) -> bool:
        plan = cls.get_plan(db, plan_id)
        if plan is None:
            return False
        product = plan.product
        if product is None:
            raise ValueError("订阅产品不存在")
        if len(list(product.variants or [])) > 1:
            raise ValueError("多版本订阅产品请使用订阅产品接口删除")
        return cls.delete_product(db, str(product.id))

    @classmethod
    def get_subscription(
        cls,
        db: Session,
        subscription_id: str,
        *,
        for_update: bool = False,
    ) -> UserSubscription | None:
        query = (
            db.query(UserSubscription)
            .options(*cls._subscription_loader_options(include_user=True, for_update=for_update))
            .filter(UserSubscription.id == subscription_id)
        )
        if for_update:
            query = query.with_for_update(of=UserSubscription)
        subscription = query.first()
        if subscription is None:
            return None
        return cls._refresh_subscription_state(db, subscription)

    @classmethod
    def get_active_subscription(
        cls,
        db: Session,
        *,
        user_id: str,
        for_update: bool = False,
    ) -> UserSubscription | None:
        query = (
            db.query(UserSubscription)
            .options(*cls._subscription_loader_options(include_user=False, for_update=for_update))
            .filter(
                UserSubscription.user_id == user_id,
                UserSubscription.status == SUBSCRIPTION_STATUS_ACTIVE,
            )
            .order_by(
                UserSubscription.started_at.desc(),
                UserSubscription.created_at.desc(),
                UserSubscription.id.desc(),
            )
        )
        if for_update:
            query = query.with_for_update(of=UserSubscription)
        for subscription in query.all():
            subscription = cls._refresh_subscription_state(db, subscription)
            if cls._is_effective_subscription(subscription):
                return subscription
        return None

    @classmethod
    def list_subscriptions(
        cls,
        db: Session,
        *,
        status: str | None = None,
        user_id: str | None = None,
        plan_id: str | None = None,
        product_id: str | None = None,
    ) -> list[UserSubscription]:
        query = db.query(UserSubscription).options(
            joinedload(UserSubscription.plan).joinedload(SubscriptionPlan.user_group),
            joinedload(UserSubscription.plan).joinedload(SubscriptionPlan.product),
            joinedload(UserSubscription.user).joinedload(User.group),
        )
        if status:
            query = query.filter(UserSubscription.status == status)
        if user_id:
            query = query.filter(UserSubscription.user_id == user_id)
        if plan_id:
            query = query.filter(UserSubscription.plan_id == plan_id)
        if product_id:
            query = query.join(UserSubscription.plan).filter(SubscriptionPlan.product_id == product_id)
        subscriptions = (
            query.order_by(UserSubscription.created_at.desc(), UserSubscription.id.desc()).all()
        )
        for subscription in subscriptions:
            cls._refresh_subscription_state(db, subscription)
        return subscriptions

    @classmethod
    def get_user_current_subscription(cls, db: Session, user_id: str) -> UserSubscription | None:
        subscription = cls.get_active_subscription(db, user_id=user_id)
        if subscription is not None:
            return subscription
        return (
            db.query(UserSubscription)
            .options(
                joinedload(UserSubscription.plan).joinedload(SubscriptionPlan.user_group),
                joinedload(UserSubscription.plan).joinedload(SubscriptionPlan.product),
            )
            .filter(UserSubscription.user_id == user_id)
            .order_by(UserSubscription.created_at.desc(), UserSubscription.id.desc())
            .first()
        )

    @classmethod
    def _list_user_active_subscriptions(
        cls,
        db: Session,
        user_id: str,
    ) -> list[UserSubscription]:
        active_subscriptions: list[UserSubscription] = []
        candidates = (
            db.query(UserSubscription)
            .filter(
                UserSubscription.user_id == user_id,
                UserSubscription.status == SUBSCRIPTION_STATUS_ACTIVE,
            )
            .order_by(UserSubscription.created_at.asc(), UserSubscription.id.asc())
            .all()
        )
        for candidate in candidates:
            candidate = cls._refresh_subscription_state(db, candidate)
            if candidate.status == SUBSCRIPTION_STATUS_ACTIVE:
                active_subscriptions.append(candidate)
        return active_subscriptions

    @classmethod
    def _get_active_subscription_descendants(
        cls,
        db: Session,
        subscription: UserSubscription | None,
        *,
        plan_id: str | None = None,
    ) -> list[UserSubscription]:
        if subscription is None:
            return []
        subscription_id = getattr(subscription, "id", None)
        user_id = getattr(subscription, "user_id", None)
        if not subscription_id or not user_id:
            return []

        children_by_parent: dict[str, list[UserSubscription]] = {}
        for candidate in cls._list_user_active_subscriptions(db, str(user_id)):
            parent_id = getattr(candidate, "upgraded_from_subscription_id", None)
            if not parent_id:
                continue
            children_by_parent.setdefault(str(parent_id), []).append(candidate)

        descendants: list[UserSubscription] = []
        visited: set[str] = set()
        stack = [str(subscription_id)]
        while stack:
            current_id = stack.pop()
            if current_id in visited:
                continue
            visited.add(current_id)
            for child in children_by_parent.get(current_id, []):
                stack.append(str(child.id))
                if plan_id is not None and str(child.plan_id) != str(plan_id):
                    continue
                descendants.append(child)
        return descendants

    @classmethod
    def _resolve_renewal_base_subscription(
        cls,
        db: Session,
        subscription: UserSubscription,
        *,
        target_plan_id: str,
    ) -> UserSubscription:
        latest_subscription = subscription
        latest_end = cls._ensure_utc(subscription.ends_at)
        for candidate in cls._get_active_subscription_descendants(
            db,
            subscription,
            plan_id=target_plan_id,
        ):
            candidate_end = getattr(candidate, "ends_at", None)
            if not isinstance(candidate_end, datetime):
                continue
            normalized_end = cls._ensure_utc(candidate_end)
            if normalized_end > latest_end:
                latest_subscription = candidate
                latest_end = normalized_end
        return latest_subscription

    @classmethod
    def get_subscription_display_end(
        cls,
        db: Session,
        subscription: UserSubscription | None,
    ) -> datetime | None:
        if subscription is None:
            return None
        ends_at = getattr(subscription, "ends_at", None)
        if not isinstance(ends_at, datetime):
            return ends_at

        subscription_id = getattr(subscription, "id", None)
        user_id = getattr(subscription, "user_id", None)
        if not subscription_id or not user_id:
            return ends_at

        latest_end = cls._ensure_utc(ends_at)
        for child in cls._get_active_subscription_descendants(db, subscription):
            child_end = getattr(child, "ends_at", None)
            if not isinstance(child_end, datetime):
                continue
            normalized_end = cls._ensure_utc(child_end)
            if normalized_end > latest_end:
                latest_end = normalized_end
        return latest_end

    @classmethod
    def list_active_plans(cls, db: Session) -> list[SubscriptionPlan]:
        return [
            plan
            for plan in cls.list_plans(db)
            if bool(getattr(plan, "is_active", False))
            and bool(getattr(getattr(plan, "product", None), "is_active", False))
        ]

    @classmethod
    def list_active_products(cls, db: Session) -> list[SubscriptionProduct]:
        return [
            product
            for product in cls.list_products(db)
            if bool(getattr(product, "is_active", False))
            and any(bool(getattr(variant, "is_active", False)) for variant in list(product.variants or []))
        ]

    @classmethod
    def _assert_single_live_subscription(cls, db: Session, user_id: str) -> None:
        live_subscription = (
            db.query(UserSubscription)
            .filter(
                UserSubscription.user_id == user_id,
                UserSubscription.status.in_(
                    [SUBSCRIPTION_STATUS_PENDING_PAYMENT, SUBSCRIPTION_STATUS_ACTIVE]
                ),
            )
            .first()
        )
        if live_subscription is not None:
            raise ValueError("该用户已有生效中的订阅")

    @classmethod
    def _assert_no_pending_subscription(cls, db: Session, user_id: str) -> None:
        pending_subscription = (
            db.query(UserSubscription)
            .filter(
                UserSubscription.user_id == user_id,
                UserSubscription.status == SUBSCRIPTION_STATUS_PENDING_PAYMENT,
            )
            .first()
        )
        if pending_subscription is not None:
            raise ValueError("该用户存在待支付订阅，请先完成或取消当前订单")

    @classmethod
    @transactional()
    @retry_on_database_error(max_retries=3)
    def create_subscription(
        cls,
        db: Session,
        *,
        user_id: str,
        plan_id: str,
        purchased_months: int,
        started_at: datetime | None = None,
        commit: bool = True,
    ) -> UserSubscription:
        user = db.query(User).filter(User.id == user_id).with_for_update(of=User).first()
        if user is None:
            raise ValueError("用户不存在")

        plan = cls.get_plan(db, plan_id)
        if plan is None:
            raise ValueError("订阅计划不存在")
        if not plan.is_active:
            raise ValueError("订阅计划未启用")

        cls._assert_single_live_subscription(db, user_id)
        monthly_price, discount_factor, total_price = cls.compute_total_price(plan, purchased_months)

        start = cls._ensure_utc(started_at or cls._utc_now())
        end, current_cycle_end = cls._build_subscription_periods(
            start=start,
            purchased_months=purchased_months,
        )

        subscription = UserSubscription(
            user_id=user.id,
            plan_id=plan.id,
            status=SUBSCRIPTION_STATUS_ACTIVE,
            end_reason=None,
            purchased_months=int(purchased_months),
            discount_factor=discount_factor,
            monthly_price_usd_snapshot=monthly_price,
            total_price_usd=total_price,
            started_at=start,
            ends_at=end,
            current_cycle_start=start,
            current_cycle_end=current_cycle_end,
            cycle_quota_usd=to_money_decimal(plan.monthly_quota_usd),
            cycle_used_usd=Decimal("0"),
            cancel_at_period_end=False,
        )
        db.add(subscription)
        if commit:
            db.commit()
            db.refresh(subscription)
        else:
            db.flush()
        return subscription

    @classmethod
    @transactional()
    @retry_on_database_error(max_retries=3)
    def create_pending_subscription(
        cls,
        db: Session,
        *,
        user_id: str,
        plan_id: str,
        purchased_months: int,
        upgraded_from_subscription_id: str | None = None,
        commit: bool = True,
    ) -> UserSubscription:
        user = db.query(User).filter(User.id == user_id).with_for_update(of=User).first()
        if user is None:
            raise ValueError("用户不存在")

        plan = cls.get_plan(db, plan_id)
        if plan is None:
            raise ValueError("订阅计划不存在")
        if not plan.is_active:
            raise ValueError("订阅计划未启用")

        cls._assert_no_pending_subscription(db, user_id)

        upgraded_from: UserSubscription | None = None
        total_price_override: Decimal | None = None
        if upgraded_from_subscription_id:
            upgraded_from = cls.get_subscription(db, upgraded_from_subscription_id, for_update=True)
            if upgraded_from is None:
                raise ValueError("原订阅不存在")
            if str(upgraded_from.user_id) != str(user_id):
                raise ValueError("原订阅不属于当前用户")
            if upgraded_from.status != SUBSCRIPTION_STATUS_ACTIVE:
                raise ValueError("只有生效中的订阅才能续期或升级")
            if not cls._is_effective_subscription(upgraded_from):
                raise ValueError("原订阅已失效，不能续期或升级")
            current_plan = upgraded_from.plan
            if current_plan is None:
                raise ValueError("原订阅计划不存在")
            transition_kind = cls.get_plan_transition_kind(current_plan, plan)
            if transition_kind is None:
                raise ValueError("只支持续期当前订阅，或升级到更高版本/更高等级的订阅")
            if transition_kind == SUBSCRIPTION_TRANSITION_RENEWAL:
                upgraded_from = cls._resolve_renewal_base_subscription(
                    db,
                    upgraded_from,
                    target_plan_id=str(plan.id),
                )
            if transition_kind == SUBSCRIPTION_TRANSITION_UPGRADE:
                if cls._get_active_subscription_descendants(db, upgraded_from):
                    raise ValueError("当前订阅已存在后续续期安排，暂不支持直接升级，请先处理已有续期")
                total_price_override = cls.compute_upgrade_payable_amount(
                    db,
                    upgraded_from,
                    new_plan=plan,
                    purchased_months=purchased_months,
                )
        else:
            cls._assert_single_live_subscription(db, user_id)

        monthly_price, discount_factor, total_price = cls.compute_total_price(plan, purchased_months)
        start = cls._utc_now()
        end, current_cycle_end = cls._build_subscription_periods(
            start=start,
            purchased_months=purchased_months,
        )

        subscription = UserSubscription(
            user_id=user.id,
            plan_id=plan.id,
            status=SUBSCRIPTION_STATUS_PENDING_PAYMENT,
            end_reason=None,
            purchased_months=int(purchased_months),
            discount_factor=discount_factor,
            monthly_price_usd_snapshot=monthly_price,
            total_price_usd=total_price_override if total_price_override is not None else total_price,
            started_at=start,
            ends_at=end,
            current_cycle_start=start,
            current_cycle_end=current_cycle_end,
            cycle_quota_usd=to_money_decimal(plan.monthly_quota_usd),
            cycle_used_usd=Decimal("0"),
            cancel_at_period_end=False,
            upgraded_from_subscription_id=upgraded_from.id if upgraded_from is not None else None,
        )
        db.add(subscription)
        if commit:
            db.commit()
            db.refresh(subscription)
        else:
            db.flush()
        return subscription

    @classmethod
    @transactional()
    @retry_on_database_error(max_retries=3)
    def cancel_subscription(
        cls,
        db: Session,
        subscription_id: str,
        *,
        immediate: bool = False,
        reason: str = SUBSCRIPTION_END_REASON_USER_CANCEL,
        commit: bool = True,
    ) -> UserSubscription | None:
        subscription = cls.get_subscription(db, subscription_id, for_update=True)
        if subscription is None:
            return None

        now = cls._utc_now()
        if subscription.status in {SUBSCRIPTION_STATUS_CANCELED, SUBSCRIPTION_STATUS_EXPIRED}:
            return subscription

        if immediate:
            subscription.status = SUBSCRIPTION_STATUS_CANCELED
            subscription.end_reason = reason
            subscription.cancel_at_period_end = False
            subscription.canceled_at = now
            subscription.ended_at = now
            subscription.ends_at = now
        else:
            subscription.cancel_at_period_end = True
            subscription.end_reason = reason
            subscription.canceled_at = now

        subscription.updated_at = now
        if commit:
            db.commit()
            db.refresh(subscription)
        else:
            db.flush()
        return subscription

    @classmethod
    def _count_future_full_months(
        cls,
        subscription: UserSubscription,
    ) -> int:
        current_cycle_end = cls._ensure_utc(subscription.current_cycle_end)
        ends_at = cls._ensure_utc(subscription.ends_at)
        count = 0
        cursor = current_cycle_end
        while cursor < ends_at:
            next_cursor = cls._add_months(cursor, 1)
            if next_cursor > ends_at:
                next_cursor = ends_at
            if cursor < ends_at:
                count += 1
            cursor = next_cursor
        return count

    @classmethod
    def estimate_upgrade_credit(
        cls,
        db: Session,
        subscription: UserSubscription,
    ) -> Decimal:
        subscription = cls._refresh_subscription_state(db, subscription)
        if subscription.purchased_months <= 0:
            return Decimal("0")

        now = cls._utc_now()
        cycle_start = cls._ensure_utc(subscription.current_cycle_start)
        cycle_end = cls._ensure_utc(subscription.current_cycle_end)
        total_cycle_seconds = max((cycle_end - cycle_start).total_seconds(), 0)
        remaining_cycle_seconds = max((cycle_end - now).total_seconds(), 0)
        remaining_time_ratio = (
            Decimal(str(remaining_cycle_seconds / total_cycle_seconds))
            if total_cycle_seconds > 0
            else Decimal("0")
        )

        cycle_quota = to_money_decimal(subscription.cycle_quota_usd)
        remaining_quota = cls.get_remaining_quota_value(subscription)
        remaining_quota_ratio = (
            remaining_quota / cycle_quota if cycle_quota > Decimal("0") else Decimal("0")
        )
        current_cycle_credit_ratio = min(remaining_time_ratio, remaining_quota_ratio)

        effective_month_price = (
            to_money_decimal(subscription.total_price_usd) / Decimal(subscription.purchased_months)
        )
        future_full_months = cls._count_future_full_months(subscription)
        return effective_month_price * (
            Decimal(future_full_months) + current_cycle_credit_ratio
        )

    @classmethod
    def compute_upgrade_payable_amount(
        cls,
        db: Session,
        subscription: UserSubscription,
        *,
        new_plan: SubscriptionPlan,
        purchased_months: int,
    ) -> Decimal:
        _, _, new_total_price = cls.compute_total_price(new_plan, purchased_months)
        upgrade_credit = cls.estimate_upgrade_credit(db, subscription)
        payable_amount = new_total_price - upgrade_credit
        if payable_amount < Decimal("0"):
            payable_amount = Decimal("0")
        return payable_amount

    @staticmethod
    def get_plan_transition_kind(
        current_plan: SubscriptionPlan,
        new_plan: SubscriptionPlan,
    ) -> str | None:
        if str(current_plan.id) == str(new_plan.id):
            return SUBSCRIPTION_TRANSITION_RENEWAL
        if str(current_plan.product_id) == str(new_plan.product_id):
            if int(new_plan.variant_rank or 0) > int(current_plan.variant_rank or 0):
                return SUBSCRIPTION_TRANSITION_UPGRADE
            return None
        if int(new_plan.plan_level or 0) > int(current_plan.plan_level or 0):
            return SUBSCRIPTION_TRANSITION_UPGRADE
        return None

    @classmethod
    def get_transition_order_type(
        cls,
        current_plan: SubscriptionPlan | None,
        new_plan: SubscriptionPlan,
    ) -> str:
        if current_plan is None:
            return SUBSCRIPTION_ORDER_TYPE_INITIAL
        transition_kind = cls.get_plan_transition_kind(current_plan, new_plan)
        if transition_kind == SUBSCRIPTION_TRANSITION_RENEWAL:
            return SUBSCRIPTION_ORDER_TYPE_RENEWAL
        return SUBSCRIPTION_ORDER_TYPE_UPGRADE

    @staticmethod
    def is_upgrade_target(current_plan: SubscriptionPlan, new_plan: SubscriptionPlan) -> bool:
        return (
            SubscriptionService.get_plan_transition_kind(current_plan, new_plan)
            == SUBSCRIPTION_TRANSITION_UPGRADE
        )

    @classmethod
    @transactional()
    @retry_on_database_error(max_retries=3)
    def upgrade_subscription(
        cls,
        db: Session,
        subscription_id: str,
        *,
        new_plan_id: str,
        purchased_months: int,
        commit: bool = True,
    ) -> tuple[UserSubscription, Decimal]:
        subscription = cls.get_subscription(db, subscription_id, for_update=True)
        if subscription is None:
            raise ValueError("订阅不存在")
        if subscription.status != SUBSCRIPTION_STATUS_ACTIVE:
            raise ValueError("只有生效中的订阅才能升级")
        if not cls._is_effective_subscription(subscription):
            raise ValueError("订阅已失效，不能升级")

        current_plan = subscription.plan
        if current_plan is None:
            raise ValueError("订阅计划不存在")
        new_plan = cls.get_plan(db, new_plan_id)
        if new_plan is None:
            raise ValueError("目标订阅计划不存在")
        if not new_plan.is_active:
            raise ValueError("目标订阅计划未启用")
        if not cls.is_upgrade_target(current_plan, new_plan):
            raise ValueError("只支持升级到更高版本或更高等级的订阅")
        if cls._get_active_subscription_descendants(db, subscription):
            raise ValueError("当前订阅已存在后续续期安排，暂不支持直接升级，请先处理已有续期")

        monthly_price, discount_factor, _new_total_price = cls.compute_total_price(
            new_plan, purchased_months
        )
        payable_amount = cls.compute_upgrade_payable_amount(
            db,
            subscription,
            new_plan=new_plan,
            purchased_months=purchased_months,
        )

        now = cls._utc_now()
        subscription.status = SUBSCRIPTION_STATUS_CANCELED
        subscription.end_reason = SUBSCRIPTION_END_REASON_UPGRADE
        subscription.cancel_at_period_end = False
        subscription.canceled_at = now
        subscription.ended_at = now
        subscription.ends_at = now
        subscription.updated_at = now

        new_end, new_cycle_end = cls._build_subscription_periods(
            start=now,
            purchased_months=purchased_months,
        )

        upgraded = UserSubscription(
            user_id=subscription.user_id,
            plan_id=new_plan.id,
            status=SUBSCRIPTION_STATUS_ACTIVE,
            end_reason=None,
            purchased_months=int(purchased_months),
            discount_factor=discount_factor,
            monthly_price_usd_snapshot=monthly_price,
            total_price_usd=payable_amount,
            started_at=now,
            ends_at=new_end,
            current_cycle_start=now,
            current_cycle_end=new_cycle_end,
            cycle_quota_usd=to_money_decimal(new_plan.monthly_quota_usd),
            cycle_used_usd=Decimal("0"),
            cancel_at_period_end=False,
            upgraded_from_subscription_id=subscription.id,
        )
        db.add(upgraded)
        if commit:
            db.commit()
            db.refresh(upgraded)
        else:
            db.flush()
        return upgraded, payable_amount

    @classmethod
    @transactional()
    @retry_on_database_error(max_retries=3)
    def activate_pending_subscription(
        cls,
        db: Session,
        subscription_id: str,
        *,
        commit: bool = True,
    ) -> UserSubscription:
        subscription = cls.get_subscription(db, subscription_id, for_update=True)
        if subscription is None:
            raise ValueError("待激活订阅不存在")
        if subscription.status == SUBSCRIPTION_STATUS_ACTIVE and cls._is_effective_subscription(
            subscription
        ):
            return subscription
        if subscription.status != SUBSCRIPTION_STATUS_PENDING_PAYMENT:
            raise ValueError("当前订阅不处于待支付状态")
        if subscription.plan is None or not bool(subscription.plan.is_active):
            raise ValueError("订阅计划未启用")

        now = cls._utc_now()
        if subscription.upgraded_from_subscription_id:
            upgraded_from = cls.get_subscription(
                db,
                subscription.upgraded_from_subscription_id,
                for_update=True,
            )
            if upgraded_from is None:
                raise ValueError("原订阅不存在")
            current_plan = upgraded_from.plan
            if current_plan is None:
                raise ValueError("原订阅计划不存在")
            transition_kind = cls.get_plan_transition_kind(current_plan, subscription.plan)
            if transition_kind == SUBSCRIPTION_TRANSITION_RENEWAL:
                renewal_start = cls._ensure_utc(upgraded_from.ends_at)
                if renewal_start < now:
                    renewal_start = now
                end, current_cycle_end = cls._build_subscription_periods(
                    start=renewal_start,
                    purchased_months=int(subscription.purchased_months or 0),
                )
                subscription.status = SUBSCRIPTION_STATUS_ACTIVE
                subscription.end_reason = None
                subscription.started_at = renewal_start
                subscription.ends_at = end
                subscription.current_cycle_start = renewal_start
                subscription.current_cycle_end = current_cycle_end
                subscription.cycle_quota_usd = to_money_decimal(subscription.plan.monthly_quota_usd)
                subscription.cycle_used_usd = Decimal("0")
                subscription.cancel_at_period_end = False
                subscription.canceled_at = None
                subscription.ended_at = None
                subscription.updated_at = now
                if commit:
                    db.commit()
                    db.refresh(subscription)
                else:
                    db.flush()
                return subscription
            if upgraded_from.status == SUBSCRIPTION_STATUS_ACTIVE:
                upgraded_from.status = SUBSCRIPTION_STATUS_CANCELED
                upgraded_from.end_reason = SUBSCRIPTION_END_REASON_UPGRADE
                upgraded_from.cancel_at_period_end = False
                upgraded_from.canceled_at = now
                upgraded_from.ended_at = now
                upgraded_from.ends_at = now
                upgraded_from.updated_at = now

        end, current_cycle_end = cls._build_subscription_periods(
            start=now,
            purchased_months=int(subscription.purchased_months or 0),
        )
        subscription.status = SUBSCRIPTION_STATUS_ACTIVE
        subscription.end_reason = None
        subscription.started_at = now
        subscription.ends_at = end
        subscription.current_cycle_start = now
        subscription.current_cycle_end = current_cycle_end
        subscription.cycle_quota_usd = to_money_decimal(subscription.plan.monthly_quota_usd)
        subscription.cycle_used_usd = Decimal("0")
        subscription.cancel_at_period_end = False
        subscription.canceled_at = None
        subscription.ended_at = None
        subscription.updated_at = now
        if commit:
            db.commit()
            db.refresh(subscription)
        else:
            db.flush()
        return subscription

    @classmethod
    @transactional()
    @retry_on_database_error(max_retries=3)
    def cancel_pending_subscription(
        cls,
        db: Session,
        subscription_id: str,
        *,
        reason: str = SUBSCRIPTION_END_REASON_PAYMENT_FAILED,
        commit: bool = True,
    ) -> UserSubscription | None:
        subscription = cls.get_subscription(db, subscription_id, for_update=True)
        if subscription is None:
            return None
        if subscription.status != SUBSCRIPTION_STATUS_PENDING_PAYMENT:
            return subscription

        now = cls._utc_now()
        subscription.status = SUBSCRIPTION_STATUS_CANCELED
        subscription.end_reason = reason
        subscription.cancel_at_period_end = False
        subscription.canceled_at = now
        subscription.ended_at = now
        subscription.updated_at = now
        if commit:
            db.commit()
            db.refresh(subscription)
        else:
            db.flush()
        return subscription

    @classmethod
    def apply_usage_charge(
        cls,
        db: Session,
        *,
        user_id: str | None,
        amount_usd: Decimal | float | int | str,
    ) -> SubscriptionChargeResult:
        amount = to_money_decimal(amount_usd)
        if not user_id or amount <= Decimal("0"):
            return SubscriptionChargeResult(None, None, None, Decimal("0"), amount)

        subscription = cls.get_active_subscription(db, user_id=user_id, for_update=True)
        if subscription is None or subscription.plan is None:
            return SubscriptionChargeResult(None, None, None, Decimal("0"), amount)

        quota_before = cls.get_remaining_quota_value(subscription)
        consumed_from_quota = min(quota_before, amount)
        quota_used_next = to_money_decimal(subscription.cycle_used_usd) + consumed_from_quota
        subscription.cycle_used_usd = quota_used_next
        subscription.updated_at = cls._utc_now()
        db.flush()

        quota_after = cls.get_remaining_quota_value(subscription)
        wallet_charge_amount = Decimal("0")
        if subscription.plan.overage_policy == SUBSCRIPTION_OVERAGE_POLICY_USE_WALLET:
            wallet_charge_amount = amount - consumed_from_quota

        return SubscriptionChargeResult(
            subscription=subscription,
            quota_before=quota_before,
            quota_after=quota_after,
            consumed_from_quota=consumed_from_quota,
            wallet_charge_amount=wallet_charge_amount,
        )
