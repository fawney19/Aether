from __future__ import annotations

from datetime import datetime, timedelta, timezone
from decimal import Decimal
from types import SimpleNamespace

import pytest
from sqlalchemy import create_engine
from sqlalchemy.dialects import postgresql
from sqlalchemy.exc import IntegrityError
from sqlalchemy.orm import Session, sessionmaker

from src.core.enums import AuthSource, UserRole
from src.core.user_access import get_user_group
from src.models.database import (
    ApiKey,
    Base,
    SubscriptionPlan,
    SubscriptionProduct,
    User,
    UserGroup,
    UserSubscription,
    Wallet,
)
from src.services.subscription import (
    SUBSCRIPTION_END_REASON_UPGRADE,
    SUBSCRIPTION_OVERAGE_POLICY_BLOCK,
    SUBSCRIPTION_OVERAGE_POLICY_USE_WALLET,
    SUBSCRIPTION_STATUS_ACTIVE,
    SUBSCRIPTION_STATUS_CANCELED,
    SUBSCRIPTION_STATUS_PENDING_PAYMENT,
    SubscriptionService,
)
from src.services.wallet import WalletService


def _make_db_session() -> Session:
    engine = create_engine("sqlite:///:memory:")
    Base.metadata.create_all(
        engine,
        tables=[
            UserGroup.__table__,
            User.__table__,
            ApiKey.__table__,
            Wallet.__table__,
            SubscriptionProduct.__table__,
            SubscriptionPlan.__table__,
            UserSubscription.__table__,
        ],
    )
    session_factory = sessionmaker(bind=engine)
    db = session_factory()
    db.info["test_engine"] = engine
    return db


def _close_db_session(db: Session) -> None:
    engine = db.info.pop("test_engine", None)
    try:
        db.close()
    finally:
        if engine is not None:
            engine.dispose()


def _create_group(db: Session, *, group_id: str, name: str) -> UserGroup:
    group = UserGroup(
        id=group_id,
        name=name,
        description=f"{name} group",
        is_default=False,
        created_at=datetime.now(timezone.utc),
        updated_at=datetime.now(timezone.utc),
    )
    db.add(group)
    db.commit()
    return group


def _create_user(db: Session, *, user_id: str, group: UserGroup) -> User:
    user = User(
        id=user_id,
        email=f"{user_id}@example.com",
        email_verified=True,
        username=user_id,
        password_hash=None,
        role=UserRole.USER,
        auth_source=AuthSource.LOCAL,
        group_id=group.id,
        group=group,
        is_active=True,
        is_deleted=False,
        created_at=datetime.now(timezone.utc),
        updated_at=datetime.now(timezone.utc),
    )
    db.add(user)
    db.commit()
    db.refresh(user)
    return user


def _create_plan(
    db: Session,
    *,
    plan_id: str,
    code: str,
    name: str,
    group: UserGroup,
    plan_level: int,
    monthly_price: Decimal,
    monthly_quota: Decimal,
    overage_policy: str,
) -> SubscriptionPlan:
    product = SubscriptionProduct(
        id=f"product-{plan_id}",
        code=f"product-{code}",
        name=name,
        description=None,
        user_group_id=group.id,
        user_group=group,
        plan_level=plan_level,
        overage_policy=overage_policy,
        is_active=True,
        created_at=datetime.now(timezone.utc),
        updated_at=datetime.now(timezone.utc),
    )
    db.add(product)
    db.flush()
    plan = SubscriptionPlan(
        id=plan_id,
        product_id=product.id,
        product=product,
        code=code,
        name=name,
        user_group_id=group.id,
        user_group=group,
        plan_level=plan_level,
        monthly_price_usd=monthly_price,
        monthly_quota_usd=monthly_quota,
        variant_rank=100,
        is_default_variant=True,
        overage_policy=overage_policy,
        term_discounts_json=[
            {"months": 1, "discount_factor": 1.0},
            {"months": 12, "discount_factor": 1.0},
        ],
        is_active=True,
        created_at=datetime.now(timezone.utc),
        updated_at=datetime.now(timezone.utc),
    )
    db.add(plan)
    db.commit()
    db.refresh(plan)
    return plan


def _create_subscription(
    db: Session,
    *,
    subscription_id: str,
    user: User,
    plan: SubscriptionPlan,
    started_at: datetime,
    ends_at: datetime,
    current_cycle_start: datetime,
    current_cycle_end: datetime,
    cycle_quota: Decimal,
    cycle_used: Decimal,
    total_price: Decimal,
    purchased_months: int,
) -> UserSubscription:
    subscription = UserSubscription(
        id=subscription_id,
        user_id=user.id,
        user=user,
        plan_id=plan.id,
        plan=plan,
        status=SUBSCRIPTION_STATUS_ACTIVE,
        purchased_months=purchased_months,
        discount_factor=Decimal("1.0"),
        monthly_price_usd_snapshot=plan.monthly_price_usd,
        total_price_usd=total_price,
        started_at=started_at,
        ends_at=ends_at,
        current_cycle_start=current_cycle_start,
        current_cycle_end=current_cycle_end,
        cycle_quota_usd=cycle_quota,
        cycle_used_usd=cycle_used,
        cancel_at_period_end=False,
        created_at=datetime.now(timezone.utc),
        updated_at=datetime.now(timezone.utc),
    )
    db.add(subscription)
    db.commit()
    db.refresh(subscription)
    return subscription


def _create_wallet(db: Session, *, user: User, balance: Decimal) -> Wallet:
    wallet = Wallet(
        user_id=user.id,
        balance=balance,
        gift_balance=Decimal("0"),
        total_recharged=balance,
        total_consumed=Decimal("0"),
        total_refunded=Decimal("0"),
        total_adjusted=Decimal("0"),
        limit_mode="finite",
        currency="USD",
        status="active",
        created_at=datetime.now(timezone.utc),
        updated_at=datetime.now(timezone.utc),
    )
    db.add(wallet)
    db.commit()
    db.refresh(wallet)
    return wallet


def test_user_access_prefers_active_subscription_group() -> None:
    db = _make_db_session()
    try:
        base_group = _create_group(db, group_id="group-base", name="base")
        sub_group = _create_group(db, group_id="group-pro", name="pro")
        user = _create_user(db, user_id="user-1", group=base_group)
        plan = _create_plan(
            db,
            plan_id="plan-pro",
            code="pro",
            name="Pro",
            group=sub_group,
            plan_level=10,
            monthly_price=Decimal("20"),
            monthly_quota=Decimal("100"),
            overage_policy=SUBSCRIPTION_OVERAGE_POLICY_BLOCK,
        )
        now = datetime.now(timezone.utc)
        _create_subscription(
            db,
            subscription_id="sub-1",
            user=user,
            plan=plan,
            started_at=now - timedelta(days=1),
            ends_at=now + timedelta(days=29),
            current_cycle_start=now - timedelta(days=1),
            current_cycle_end=now + timedelta(days=29),
            cycle_quota=Decimal("100"),
            cycle_used=Decimal("10"),
            total_price=Decimal("20"),
            purchased_months=1,
        )

        db.refresh(user)
        assert get_user_group(user).id == sub_group.id
    finally:
        _close_db_session(db)


def test_wallet_access_allows_subscription_quota_without_wallet_balance() -> None:
    db = _make_db_session()
    try:
        base_group = _create_group(db, group_id="group-base", name="base")
        plan_group = _create_group(db, group_id="group-pro", name="pro")
        user = _create_user(db, user_id="user-1", group=base_group)
        plan = _create_plan(
            db,
            plan_id="plan-pro",
            code="pro",
            name="Pro",
            group=plan_group,
            plan_level=10,
            monthly_price=Decimal("20"),
            monthly_quota=Decimal("20"),
            overage_policy=SUBSCRIPTION_OVERAGE_POLICY_BLOCK,
        )
        now = datetime.now(timezone.utc)
        _create_subscription(
            db,
            subscription_id="sub-1",
            user=user,
            plan=plan,
            started_at=now - timedelta(days=1),
            ends_at=now + timedelta(days=29),
            current_cycle_start=now - timedelta(days=1),
            current_cycle_end=now + timedelta(days=29),
            cycle_quota=Decimal("20"),
            cycle_used=Decimal("5"),
            total_price=Decimal("20"),
            purchased_months=1,
        )

        access = WalletService.check_request_allowed(db, user=user)

        assert access.allowed is True
        assert access.subscription_id == "sub-1"
        assert access.subscription_remaining == Decimal("15")
        assert access.remaining == Decimal("15")
    finally:
        _close_db_session(db)


def test_create_pending_subscription_then_activate_resets_cycle() -> None:
    db = _make_db_session()
    try:
        base_group = _create_group(db, group_id="group-base", name="base")
        plan_group = _create_group(db, group_id="group-pro", name="pro")
        user = _create_user(db, user_id="user-1", group=base_group)
        plan = _create_plan(
            db,
            plan_id="plan-pro",
            code="pro",
            name="Pro",
            group=plan_group,
            plan_level=10,
            monthly_price=Decimal("20"),
            monthly_quota=Decimal("80"),
            overage_policy=SUBSCRIPTION_OVERAGE_POLICY_BLOCK,
        )

        pending = SubscriptionService.create_pending_subscription(
            db,
            user_id=user.id,
            plan_id=plan.id,
            purchased_months=12,
        )
        assert pending.status == SUBSCRIPTION_STATUS_PENDING_PAYMENT

        activated = SubscriptionService.activate_pending_subscription(db, pending.id)

        assert activated.status == SUBSCRIPTION_STATUS_ACTIVE
        assert activated.started_at < activated.ends_at
        assert activated.current_cycle_start == activated.started_at
        assert activated.current_cycle_end <= activated.ends_at
        assert Decimal(activated.cycle_quota_usd) == Decimal("80")
        assert Decimal(activated.cycle_used_usd) == Decimal("0")
    finally:
        _close_db_session(db)


def test_create_pending_subscription_allows_same_plan_renewal() -> None:
    db = _make_db_session()
    try:
        base_group = _create_group(db, group_id="group-base", name="base")
        plan_group = _create_group(db, group_id="group-pro", name="pro")
        user = _create_user(db, user_id="user-1", group=base_group)
        plan = _create_plan(
            db,
            plan_id="plan-pro",
            code="pro",
            name="Pro",
            group=plan_group,
            plan_level=10,
            monthly_price=Decimal("20"),
            monthly_quota=Decimal("80"),
            overage_policy=SUBSCRIPTION_OVERAGE_POLICY_BLOCK,
        )
        now = datetime.now(timezone.utc)
        current = _create_subscription(
            db,
            subscription_id="sub-current",
            user=user,
            plan=plan,
            started_at=now - timedelta(days=1),
            ends_at=now + timedelta(days=29),
            current_cycle_start=now - timedelta(days=1),
            current_cycle_end=now + timedelta(days=29),
            cycle_quota=Decimal("80"),
            cycle_used=Decimal("10"),
            total_price=Decimal("20"),
            purchased_months=1,
        )

        pending = SubscriptionService.create_pending_subscription(
            db,
            user_id=user.id,
            plan_id=plan.id,
            purchased_months=3,
            upgraded_from_subscription_id=current.id,
        )

        assert pending.status == SUBSCRIPTION_STATUS_PENDING_PAYMENT
        assert pending.upgraded_from_subscription_id == current.id
        assert Decimal(pending.total_price_usd) == Decimal("60")
    finally:
        _close_db_session(db)


def test_activate_pending_subscription_for_renewal_starts_after_current_end() -> None:
    db = _make_db_session()
    try:
        base_group = _create_group(db, group_id="group-base", name="base")
        plan_group = _create_group(db, group_id="group-pro", name="pro")
        user = _create_user(db, user_id="user-1", group=base_group)
        plan = _create_plan(
            db,
            plan_id="plan-pro",
            code="pro",
            name="Pro",
            group=plan_group,
            plan_level=10,
            monthly_price=Decimal("20"),
            monthly_quota=Decimal("80"),
            overage_policy=SUBSCRIPTION_OVERAGE_POLICY_BLOCK,
        )
        now = datetime.now(timezone.utc)
        current = _create_subscription(
            db,
            subscription_id="sub-current",
            user=user,
            plan=plan,
            started_at=now - timedelta(days=1),
            ends_at=now + timedelta(days=29),
            current_cycle_start=now - timedelta(days=1),
            current_cycle_end=now + timedelta(days=29),
            cycle_quota=Decimal("80"),
            cycle_used=Decimal("10"),
            total_price=Decimal("20"),
            purchased_months=1,
        )
        pending = SubscriptionService.create_pending_subscription(
            db,
            user_id=user.id,
            plan_id=plan.id,
            purchased_months=2,
            upgraded_from_subscription_id=current.id,
        )

        renewed = SubscriptionService.activate_pending_subscription(db, pending.id)

        db.refresh(current)
        assert current.status == SUBSCRIPTION_STATUS_ACTIVE
        assert SubscriptionService._ensure_utc(renewed.started_at) == SubscriptionService._ensure_utc(
            current.ends_at
        )
        assert SubscriptionService.get_active_subscription(db, user_id=user.id).id == current.id
    finally:
        _close_db_session(db)


def test_get_subscription_display_end_extends_to_latest_renewal() -> None:
    db = _make_db_session()
    try:
        base_group = _create_group(db, group_id="group-base", name="base")
        plan_group = _create_group(db, group_id="group-pro", name="pro")
        user = _create_user(db, user_id="user-1", group=base_group)
        plan = _create_plan(
            db,
            plan_id="plan-pro",
            code="pro",
            name="Pro",
            group=plan_group,
            plan_level=10,
            monthly_price=Decimal("20"),
            monthly_quota=Decimal("80"),
            overage_policy=SUBSCRIPTION_OVERAGE_POLICY_BLOCK,
        )
        now = datetime.now(timezone.utc)
        current = _create_subscription(
            db,
            subscription_id="sub-current",
            user=user,
            plan=plan,
            started_at=now - timedelta(days=1),
            ends_at=now + timedelta(days=29),
            current_cycle_start=now - timedelta(days=1),
            current_cycle_end=now + timedelta(days=29),
            cycle_quota=Decimal("80"),
            cycle_used=Decimal("10"),
            total_price=Decimal("20"),
            purchased_months=1,
        )
        pending = SubscriptionService.create_pending_subscription(
            db,
            user_id=user.id,
            plan_id=plan.id,
            purchased_months=2,
            upgraded_from_subscription_id=current.id,
        )

        renewed = SubscriptionService.activate_pending_subscription(db, pending.id)

        assert SubscriptionService.get_subscription_display_end(db, current) == SubscriptionService._ensure_utc(
            renewed.ends_at
        )
        assert SubscriptionService.get_subscription_display_end(db, renewed) == SubscriptionService._ensure_utc(
            renewed.ends_at
        )
    finally:
        _close_db_session(db)


def test_multiple_renewals_chain_from_latest_renewal_end() -> None:
    db = _make_db_session()
    try:
        base_group = _create_group(db, group_id="group-base", name="base")
        plan_group = _create_group(db, group_id="group-pro", name="pro")
        user = _create_user(db, user_id="user-1", group=base_group)
        plan = _create_plan(
            db,
            plan_id="plan-pro",
            code="pro",
            name="Pro",
            group=plan_group,
            plan_level=10,
            monthly_price=Decimal("20"),
            monthly_quota=Decimal("80"),
            overage_policy=SUBSCRIPTION_OVERAGE_POLICY_BLOCK,
        )
        now = datetime.now(timezone.utc)
        current = _create_subscription(
            db,
            subscription_id="sub-current",
            user=user,
            plan=plan,
            started_at=now - timedelta(days=1),
            ends_at=now + timedelta(days=29),
            current_cycle_start=now - timedelta(days=1),
            current_cycle_end=now + timedelta(days=29),
            cycle_quota=Decimal("80"),
            cycle_used=Decimal("10"),
            total_price=Decimal("20"),
            purchased_months=1,
        )

        first_pending = SubscriptionService.create_pending_subscription(
            db,
            user_id=user.id,
            plan_id=plan.id,
            purchased_months=2,
            upgraded_from_subscription_id=current.id,
        )
        first_renewed = SubscriptionService.activate_pending_subscription(db, first_pending.id)

        second_pending = SubscriptionService.create_pending_subscription(
            db,
            user_id=user.id,
            plan_id=plan.id,
            purchased_months=3,
            upgraded_from_subscription_id=current.id,
        )

        assert second_pending.upgraded_from_subscription_id == first_renewed.id

        second_renewed = SubscriptionService.activate_pending_subscription(db, second_pending.id)

        assert SubscriptionService._ensure_utc(second_renewed.started_at) == SubscriptionService._ensure_utc(
            first_renewed.ends_at
        )
        assert SubscriptionService.get_subscription_display_end(
            db, current
        ) == SubscriptionService._ensure_utc(second_renewed.ends_at)
    finally:
        _close_db_session(db)


def test_upgrade_rejects_when_future_renewal_exists() -> None:
    db = _make_db_session()
    try:
        base_group = _create_group(db, group_id="group-base", name="base")
        group_pro = _create_group(db, group_id="group-pro", name="pro")
        group_enterprise = _create_group(db, group_id="group-enterprise", name="enterprise")
        user = _create_user(db, user_id="user-1", group=base_group)
        pro_plan = _create_plan(
            db,
            plan_id="plan-pro",
            code="pro",
            name="Pro",
            group=group_pro,
            plan_level=10,
            monthly_price=Decimal("20"),
            monthly_quota=Decimal("80"),
            overage_policy=SUBSCRIPTION_OVERAGE_POLICY_BLOCK,
        )
        enterprise_plan = _create_plan(
            db,
            plan_id="plan-enterprise",
            code="enterprise",
            name="Enterprise",
            group=group_enterprise,
            plan_level=20,
            monthly_price=Decimal("50"),
            monthly_quota=Decimal("200"),
            overage_policy=SUBSCRIPTION_OVERAGE_POLICY_BLOCK,
        )
        now = datetime.now(timezone.utc)
        current = _create_subscription(
            db,
            subscription_id="sub-current",
            user=user,
            plan=pro_plan,
            started_at=now - timedelta(days=1),
            ends_at=now + timedelta(days=29),
            current_cycle_start=now - timedelta(days=1),
            current_cycle_end=now + timedelta(days=29),
            cycle_quota=Decimal("80"),
            cycle_used=Decimal("10"),
            total_price=Decimal("20"),
            purchased_months=1,
        )
        pending = SubscriptionService.create_pending_subscription(
            db,
            user_id=user.id,
            plan_id=pro_plan.id,
            purchased_months=2,
            upgraded_from_subscription_id=current.id,
        )
        SubscriptionService.activate_pending_subscription(db, pending.id)

        with pytest.raises(ValueError, match="后续续期安排"):
            SubscriptionService.create_pending_subscription(
                db,
                user_id=user.id,
                plan_id=enterprise_plan.id,
                purchased_months=1,
                upgraded_from_subscription_id=current.id,
            )
    finally:
        _close_db_session(db)


def test_subscription_schema_allows_active_plus_one_pending_upgrade_only() -> None:
    db = _make_db_session()
    try:
        base_group = _create_group(db, group_id="group-base", name="base")
        plan_group = _create_group(db, group_id="group-pro", name="pro")
        user = _create_user(db, user_id="user-1", group=base_group)
        plan = _create_plan(
            db,
            plan_id="plan-pro",
            code="pro",
            name="Pro",
            group=plan_group,
            plan_level=10,
            monthly_price=Decimal("20"),
            monthly_quota=Decimal("80"),
            overage_policy=SUBSCRIPTION_OVERAGE_POLICY_BLOCK,
        )
        now = datetime.now(timezone.utc)
        current = _create_subscription(
            db,
            subscription_id="sub-current",
            user=user,
            plan=plan,
            started_at=now - timedelta(days=1),
            ends_at=now + timedelta(days=29),
            current_cycle_start=now - timedelta(days=1),
            current_cycle_end=now + timedelta(days=29),
            cycle_quota=Decimal("80"),
            cycle_used=Decimal("10"),
            total_price=Decimal("20"),
            purchased_months=1,
        )

        pending = UserSubscription(
            id="sub-pending-1",
            user_id=user.id,
            plan_id=plan.id,
            status=SUBSCRIPTION_STATUS_PENDING_PAYMENT,
            purchased_months=1,
            discount_factor=Decimal("1.0"),
            monthly_price_usd_snapshot=Decimal("20"),
            total_price_usd=Decimal("20"),
            started_at=now,
            ends_at=now + timedelta(days=30),
            current_cycle_start=now,
            current_cycle_end=now + timedelta(days=30),
            cycle_quota_usd=Decimal("80"),
            cycle_used_usd=Decimal("0"),
            cancel_at_period_end=False,
            upgraded_from_subscription_id=current.id,
            created_at=now,
            updated_at=now,
        )
        db.add(pending)
        db.commit()

        duplicate_pending = UserSubscription(
            id="sub-pending-2",
            user_id=user.id,
            plan_id=plan.id,
            status=SUBSCRIPTION_STATUS_PENDING_PAYMENT,
            purchased_months=1,
            discount_factor=Decimal("1.0"),
            monthly_price_usd_snapshot=Decimal("20"),
            total_price_usd=Decimal("20"),
            started_at=now,
            ends_at=now + timedelta(days=31),
            current_cycle_start=now,
            current_cycle_end=now + timedelta(days=31),
            cycle_quota_usd=Decimal("80"),
            cycle_used_usd=Decimal("0"),
            cancel_at_period_end=False,
            upgraded_from_subscription_id=current.id,
            created_at=now + timedelta(seconds=1),
            updated_at=now + timedelta(seconds=1),
        )
        db.add(duplicate_pending)
        with pytest.raises(IntegrityError):
            db.commit()
        db.rollback()
    finally:
        _close_db_session(db)


def test_compute_total_price_supports_custom_months_with_threshold_discount() -> None:
    db = _make_db_session()
    try:
        group = _create_group(db, group_id="group-pro", name="pro")
        plan = _create_plan(
            db,
            plan_id="plan-pro",
            code="pro",
            name="Pro",
            group=group,
            plan_level=10,
            monthly_price=Decimal("20"),
            monthly_quota=Decimal("80"),
            overage_policy=SUBSCRIPTION_OVERAGE_POLICY_BLOCK,
        )
        plan.term_discounts_json = [
            {"months": 1, "discount_factor": 1.0},
            {"months": 3, "discount_factor": 0.9},
            {"months": 12, "discount_factor": 0.75},
        ]
        db.commit()
        db.refresh(plan)

        assert SubscriptionService.resolve_discount_factor(plan, 2) == Decimal("1.0")
        assert SubscriptionService.resolve_discount_factor(plan, 3) == Decimal("0.9")
        assert SubscriptionService.resolve_discount_factor(plan, 5) == Decimal("0.9")
        assert SubscriptionService.resolve_discount_factor(plan, 18) == Decimal("0.75")

        monthly_price, discount_factor, total_price = SubscriptionService.compute_total_price(
            plan,
            18,
        )

        assert monthly_price == Decimal("20")
        assert discount_factor == Decimal("0.75")
        assert total_price == Decimal("270")
    finally:
        _close_db_session(db)


def test_wallet_usage_charge_splits_subscription_and_wallet(monkeypatch: pytest.MonkeyPatch) -> None:
    db = _make_db_session()
    try:
        base_group = _create_group(db, group_id="group-base", name="base")
        plan_group = _create_group(db, group_id="group-pro", name="pro")
        user = _create_user(db, user_id="user-1", group=base_group)
        plan = _create_plan(
            db,
            plan_id="plan-pro",
            code="pro",
            name="Pro",
            group=plan_group,
            plan_level=10,
            monthly_price=Decimal("20"),
            monthly_quota=Decimal("20"),
            overage_policy=SUBSCRIPTION_OVERAGE_POLICY_USE_WALLET,
        )
        now = datetime.now(timezone.utc)
        _create_subscription(
            db,
            subscription_id="sub-1",
            user=user,
            plan=plan,
            started_at=now - timedelta(days=1),
            ends_at=now + timedelta(days=29),
            current_cycle_start=now - timedelta(days=1),
            current_cycle_end=now + timedelta(days=29),
            cycle_quota=Decimal("20"),
            cycle_used=Decimal("15"),
            total_price=Decimal("20"),
            purchased_months=1,
        )
        wallet = _create_wallet(db, user=user, balance=Decimal("10"))
        usage = SimpleNamespace(
            user_id=user.id,
            api_key_id=None,
            wallet_id=wallet.id,
            subscription_id=None,
            subscription_quota_before_usd=None,
            subscription_quota_after_usd=None,
            billing_source=None,
            wallet_balance_before=None,
            wallet_balance_after=None,
            wallet_recharge_balance_before=None,
            wallet_recharge_balance_after=None,
            wallet_gift_balance_before=None,
            wallet_gift_balance_after=None,
        )

        monkeypatch.setattr(
            "src.services.wallet.service.WalletService._resolve_wallet_for_usage",
            lambda *_a, **_k: wallet,
        )

        before_total, after_total = WalletService.apply_usage_charge(
            db,
            usage=usage,
            amount_usd=Decimal("8"),
        )

        db.refresh(wallet)
        subscription = SubscriptionService.get_subscription(db, "sub-1")

        assert before_total == Decimal("10")
        assert after_total == Decimal("7")
        assert usage.subscription_id == "sub-1"
        assert usage.subscription_quota_before_usd == Decimal("5")
        assert usage.subscription_quota_after_usd == Decimal("0")
        assert usage.billing_source == "mixed"
        assert wallet.balance == Decimal("7")
        assert wallet.total_consumed == Decimal("3")
        assert subscription is not None
        assert subscription.cycle_used_usd == Decimal("20")
    finally:
        _close_db_session(db)


def test_upgrade_subscription_uses_smaller_of_time_and_quota_ratio(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    db = _make_db_session()
    try:
        base_group = _create_group(db, group_id="group-basic", name="basic")
        pro_group = _create_group(db, group_id="group-pro", name="pro")
        enterprise_group = _create_group(db, group_id="group-enterprise", name="enterprise")
        user = _create_user(db, user_id="user-1", group=base_group)
        basic_plan = _create_plan(
            db,
            plan_id="plan-basic",
            code="basic",
            name="Basic",
            group=pro_group,
            plan_level=10,
            monthly_price=Decimal("10"),
            monthly_quota=Decimal("100"),
            overage_policy=SUBSCRIPTION_OVERAGE_POLICY_BLOCK,
        )
        enterprise_plan = _create_plan(
            db,
            plan_id="plan-enterprise",
            code="enterprise",
            name="Enterprise",
            group=enterprise_group,
            plan_level=20,
            monthly_price=Decimal("20"),
            monthly_quota=Decimal("200"),
            overage_policy=SUBSCRIPTION_OVERAGE_POLICY_BLOCK,
        )
        fixed_now = datetime(2026, 4, 16, 0, 0, tzinfo=timezone.utc)
        old_subscription = _create_subscription(
            db,
            subscription_id="sub-old",
            user=user,
            plan=basic_plan,
            started_at=datetime(2026, 1, 1, 0, 0, tzinfo=timezone.utc),
            ends_at=datetime(2027, 1, 1, 0, 0, tzinfo=timezone.utc),
            current_cycle_start=datetime(2026, 4, 1, 0, 0, tzinfo=timezone.utc),
            current_cycle_end=datetime(2026, 5, 1, 0, 0, tzinfo=timezone.utc),
            cycle_quota=Decimal("100"),
            cycle_used=Decimal("90"),
            total_price=Decimal("120"),
            purchased_months=12,
        )

        monkeypatch.setattr(SubscriptionService, "_utc_now", staticmethod(lambda: fixed_now))

        upgraded, payable_amount = SubscriptionService.upgrade_subscription(
            db,
            old_subscription.id,
            new_plan_id=enterprise_plan.id,
            purchased_months=12,
        )

        db.refresh(old_subscription)

        assert old_subscription.status == SUBSCRIPTION_STATUS_CANCELED
        assert old_subscription.end_reason == SUBSCRIPTION_END_REASON_UPGRADE
        assert SubscriptionService._ensure_utc(old_subscription.ended_at) == fixed_now
        assert upgraded.upgraded_from_subscription_id == old_subscription.id
        assert upgraded.plan_id == enterprise_plan.id
        assert upgraded.total_price_usd == Decimal("159")
        assert payable_amount == Decimal("159")
    finally:
        _close_db_session(db)


def test_locked_subscription_query_targets_only_subscription_table() -> None:
    db = _make_db_session()
    try:
        query = (
            db.query(UserSubscription)
            .options(
                *SubscriptionService._subscription_loader_options(
                    include_user=True,
                    for_update=True,
                )
            )
            .filter(UserSubscription.id == "sub-1")
            .limit(1)
            .with_for_update(of=UserSubscription)
        )

        compiled = str(
            query.statement.compile(
                dialect=postgresql.dialect(),
                compile_kwargs={"literal_binds": True},
            )
        ).upper()

        assert "LEFT OUTER JOIN" not in compiled
        assert "FOR UPDATE OF USER_SUBSCRIPTIONS" in compiled
    finally:
        _close_db_session(db)
