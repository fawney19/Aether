from __future__ import annotations

from dataclasses import dataclass
from datetime import datetime, timezone
from decimal import Decimal
from typing import Literal
from uuid import uuid4

from sqlalchemy import func
from sqlalchemy.exc import IntegrityError
from sqlalchemy.orm import Session

from src.core.enums import UserRole
from src.models.database import (
    ApiKey,
    PaymentOrder,
    RefundRequest,
    Usage,
    User,
    Wallet,
    WalletTransaction,
)
from src.services.billing.precision import to_money_decimal

WalletCategory = Literal["recharge", "gift", "adjust", "refund"]
WalletBalanceBucket = Literal["recharge", "gift"]

REASON_TOPUP_ADMIN_MANUAL = "topup_admin_manual"
REASON_TOPUP_GATEWAY = "topup_gateway"
REASON_TOPUP_CARD_CODE = "topup_card_code"
REASON_GIFT_INITIAL = "gift_initial"
REASON_GIFT_CAMPAIGN = "gift_campaign"
REASON_GIFT_EXPIRE_RECLAIM = "gift_expire_reclaim"
REASON_ADJUST_ADMIN = "adjust_admin"
REASON_ADJUST_SYSTEM = "adjust_system"
REASON_REFUND_OUT = "refund_out"
REASON_REFUND_REVERT = "refund_revert"


@dataclass(slots=True)
class WalletAccessResult:
    allowed: bool
    remaining: Decimal | None
    message: str
    wallet: Wallet | None = None
    balance_snapshot: Decimal | None = None
    subscription_id: str | None = None
    subscription_remaining: Decimal | None = None


class WalletService:
    """统一钱包服务。"""

    @staticmethod
    def get_limit_mode(wallet: Wallet | None) -> str:
        if wallet is None:
            return "finite"
        limit_mode = getattr(wallet, "limit_mode", None)
        if limit_mode in {"finite", "unlimited"}:
            return str(limit_mode)
        return "finite"

    @classmethod
    def is_unlimited_wallet(cls, wallet: Wallet | None) -> bool:
        return cls.get_limit_mode(wallet) == "unlimited"

    @classmethod
    def get_recharge_balance_value(cls, wallet: Wallet | None) -> Decimal:
        if wallet is None:
            return Decimal("0")
        return to_money_decimal(wallet.balance)

    @classmethod
    def get_gift_balance_value(cls, wallet: Wallet | None) -> Decimal:
        if wallet is None:
            return Decimal("0")
        return to_money_decimal(getattr(wallet, "gift_balance", None))

    @classmethod
    def get_spendable_balance_value(cls, wallet: Wallet | None) -> Decimal:
        return cls.get_recharge_balance_value(wallet) + cls.get_gift_balance_value(wallet)

    @classmethod
    def get_refundable_balance_value(cls, wallet: Wallet | None) -> Decimal:
        # 赠款余额不可退款，仅充值余额可退。
        return cls.get_recharge_balance_value(wallet)

    @classmethod
    def serialize_wallet_summary(cls, wallet: Wallet | None) -> dict[str, object]:
        recharge_balance = cls.get_recharge_balance_value(wallet)
        gift_balance = cls.get_gift_balance_value(wallet)
        spendable_balance = recharge_balance + gift_balance
        limit_mode = cls.get_limit_mode(wallet)
        return {
            "id": wallet.id if wallet else None,
            "balance": float(spendable_balance),
            "recharge_balance": float(recharge_balance),
            "gift_balance": float(gift_balance),
            "refundable_balance": float(recharge_balance),
            "currency": wallet.currency if wallet else "USD",
            "status": wallet.status if wallet else "active",
            "limit_mode": limit_mode,
            "unlimited": limit_mode == "unlimited",
            "total_recharged": float(wallet.total_recharged or 0) if wallet else 0.0,
            "total_consumed": float(wallet.total_consumed or 0) if wallet else 0.0,
            "total_refunded": float(wallet.total_refunded or 0) if wallet else 0.0,
            "total_adjusted": float(wallet.total_adjusted or 0) if wallet else 0.0,
            "updated_at": wallet.updated_at if wallet else None,
        }

    @staticmethod
    def _build_order_no(prefix: str) -> str:
        ts = datetime.now(timezone.utc).strftime("%Y%m%d%H%M%S%f")
        return f"{prefix}_{ts}_{uuid4().hex[:12]}"

    @classmethod
    def initialize_user_wallet(
        cls,
        db: Session,
        *,
        user: User,
        initial_gift_usd: Decimal | float | int | str | None,
        unlimited: bool = False,
        description: str = "用户初始赠款",
    ) -> Wallet | None:
        """初始化用户钱包，并按需要写入初始赠款。"""
        if not user.id:
            return None

        wallet = cls.get_wallet(db, user_id=user.id)
        if wallet is None:
            wallet = Wallet(
                user_id=user.id,
                balance=Decimal("0"),
                gift_balance=Decimal("0"),
                total_recharged=Decimal("0"),
                total_consumed=Decimal("0"),
                total_refunded=Decimal("0"),
                total_adjusted=Decimal("0"),
                limit_mode="unlimited" if unlimited else "finite",
                currency="USD",
                status="active",
            )
            db.add(wallet)
            db.flush()
        else:
            wallet.limit_mode = "unlimited" if unlimited else "finite"

        gift_amount = to_money_decimal(initial_gift_usd)
        if not unlimited and gift_amount > Decimal("0"):
            cls.create_wallet_transaction(
                db,
                wallet=wallet,
                category="gift",
                reason_code=REASON_GIFT_INITIAL,
                amount=gift_amount,
                balance_type="gift",
                link_type="system_task",
                link_id=user.id,
                description=description,
            )
        return wallet

    @classmethod
    def initialize_api_key_wallet(
        cls,
        db: Session,
        *,
        api_key: ApiKey,
        initial_balance_usd: Decimal | float | int | str | None,
        unlimited: bool = False,
        operator_id: str | None = None,
        description: str = "初始调账",
    ) -> Wallet | None:
        """初始化独立 Key 钱包，并按需执行初始调账。

        设计目标：
        - 初始化语义与用户钱包保持一致（均由 WalletService 统一入口完成）
        - 独立 Key 不支持充值，余额变动统一通过调账流水实现
        """
        if not api_key.id:
            return None

        wallet = cls.get_wallet(db, api_key_id=api_key.id)
        if wallet is None:
            wallet = Wallet(
                api_key_id=api_key.id,
                balance=Decimal("0"),
                gift_balance=Decimal("0"),
                total_recharged=Decimal("0"),
                total_consumed=Decimal("0"),
                total_refunded=Decimal("0"),
                total_adjusted=Decimal("0"),
                limit_mode="unlimited" if unlimited else "finite",
                currency="USD",
                status="active",
            )
            db.add(wallet)
            db.flush()
        else:
            wallet.limit_mode = "unlimited" if unlimited else "finite"

        initial_amount = to_money_decimal(initial_balance_usd)
        if not unlimited and initial_amount > Decimal("0"):
            cls.create_wallet_transaction(
                db,
                wallet=wallet,
                category="adjust",
                reason_code=REASON_ADJUST_SYSTEM,
                amount=initial_amount,
                balance_type="recharge",
                link_type="system_task",
                link_id=api_key.id,
                operator_id=operator_id,
                description=description,
            )
        return wallet

    @classmethod
    def get_wallet(
        cls,
        db: Session,
        *,
        user_id: str | None = None,
        api_key_id: str | None = None,
    ) -> Wallet | None:
        if api_key_id:
            wallet = db.query(Wallet).filter(Wallet.api_key_id == api_key_id).first()
            if wallet is not None:
                return wallet
        if user_id:
            return db.query(Wallet).filter(Wallet.user_id == user_id).first()
        return None

    @classmethod
    def get_wallets_by_user_ids(
        cls,
        db: Session,
        user_ids: list[str],
    ) -> dict[str, Wallet]:
        if not user_ids:
            return {}
        wallets = db.query(Wallet).filter(Wallet.user_id.in_(user_ids)).all()
        return {wallet.user_id: wallet for wallet in wallets if wallet.user_id is not None}

    @classmethod
    def get_or_create_wallet(
        cls,
        db: Session,
        *,
        user: User | None = None,
        api_key: ApiKey | None = None,
        user_id: str | None = None,
        api_key_id: str | None = None,
    ) -> Wallet | None:
        if user is None and user_id:
            user = db.query(User).filter(User.id == user_id).first()
        if api_key is None and api_key_id:
            api_key = db.query(ApiKey).filter(ApiKey.id == api_key_id).first()

        owner_user_id = user.id if user else user_id
        owner_api_key_id = api_key.id if api_key else api_key_id

        # owner 解析规则：
        # - 独立 Key: 归属 API Key 钱包
        # - 普通 Key + 用户: 归属用户钱包（避免 user_id/api_key_id 同时写入）
        # - 仅提供 API Key: 归属 API Key 钱包
        api_key_is_standalone = bool(getattr(api_key, "is_standalone", False)) if api_key else False
        if owner_user_id is not None and not api_key_is_standalone:
            owner_api_key_id = None
        elif owner_api_key_id is not None:
            owner_user_id = None

        wallet = cls.get_wallet(db, user_id=owner_user_id, api_key_id=owner_api_key_id)
        if wallet:
            return wallet

        if owner_user_id is None and owner_api_key_id is None:
            return None

        bootstrap = Wallet(
            user_id=owner_user_id,
            api_key_id=owner_api_key_id,
            balance=Decimal("0"),
            gift_balance=Decimal("0"),
            total_recharged=Decimal("0"),
            total_consumed=Decimal("0"),
            total_refunded=Decimal("0"),
            total_adjusted=Decimal("0"),
            limit_mode="finite",
            currency="USD",
            status="active",
            created_at=datetime.now(timezone.utc),
            updated_at=datetime.now(timezone.utc),
        )
        try:
            with db.begin_nested():
                db.add(bootstrap)
                db.flush()
            return bootstrap
        except IntegrityError:
            # 并发创建时可能触发唯一约束，回查已创建的钱包并复用。
            wallet = cls.get_wallet(db, user_id=owner_user_id, api_key_id=owner_api_key_id)
            if wallet is not None:
                return wallet
            raise

    @classmethod
    def _get_balance_snapshot_from_wallet(cls, wallet: Wallet | None) -> Decimal | None:
        if wallet is None:
            return None

        recharge_balance = cls.get_recharge_balance_value(wallet)
        if recharge_balance < Decimal("0"):
            return recharge_balance
        if cls.is_unlimited_wallet(wallet):
            return None
        return cls.get_spendable_balance_value(wallet)

    @classmethod
    def _check_wallet_only_access(cls, wallet: Wallet | None) -> WalletAccessResult:
        balance_snapshot = cls._get_balance_snapshot_from_wallet(wallet)
        if wallet is None:
            return WalletAccessResult(False, Decimal("0"), "钱包不存在", None, None)

        remaining = cls.get_spendable_balance_value(wallet)
        recharge_balance = cls.get_recharge_balance_value(wallet)
        if wallet.status != "active":
            return WalletAccessResult(False, remaining, "钱包不可用", wallet, balance_snapshot)
        if recharge_balance < Decimal("0"):
            return WalletAccessResult(
                False,
                recharge_balance,
                "钱包欠费，请先充值",
                wallet,
                balance_snapshot,
            )
        if cls.is_unlimited_wallet(wallet):
            return WalletAccessResult(True, None, "OK", wallet, balance_snapshot)
        if remaining <= Decimal("0"):
            return WalletAccessResult(False, remaining, "钱包余额不足", wallet, balance_snapshot)
        return WalletAccessResult(True, remaining, "OK", wallet, balance_snapshot)

    @classmethod
    def check_request_allowed(
        cls,
        db: Session,
        *,
        user: User | None,
        api_key: ApiKey | None = None,
    ) -> WalletAccessResult:
        wallet = cls.get_or_create_wallet(db, user=user, api_key=api_key)
        balance_snapshot = cls._get_balance_snapshot_from_wallet(wallet)

        if user and user.role == UserRole.ADMIN:
            return WalletAccessResult(True, None, "OK", wallet, balance_snapshot)

        if user is not None and not (api_key and api_key.is_standalone):
            from src.services.subscription import (
                SUBSCRIPTION_OVERAGE_POLICY_USE_WALLET,
                SubscriptionService,
            )

            subscription = SubscriptionService.get_active_subscription(db, user_id=str(user.id))
            if subscription is not None and subscription.plan is not None:
                remaining_quota = SubscriptionService.get_remaining_quota_value(subscription)
                subscription_id = str(subscription.id)

                if (
                    str(subscription.plan.overage_policy)
                    != SUBSCRIPTION_OVERAGE_POLICY_USE_WALLET
                ):
                    return WalletAccessResult(
                        remaining_quota > Decimal("0"),
                        remaining_quota,
                        "OK" if remaining_quota > Decimal("0") else "订阅额度已用尽",
                        wallet,
                        remaining_quota,
                        subscription_id,
                        remaining_quota,
                    )

                if remaining_quota > Decimal("0"):
                    combined_snapshot: Decimal | None
                    if (
                        wallet is not None
                        and wallet.status == "active"
                        and cls.get_recharge_balance_value(wallet) >= Decimal("0")
                        and cls.is_unlimited_wallet(wallet)
                    ):
                        combined_snapshot = None
                    elif wallet is not None and wallet.status == "active":
                        combined_snapshot = remaining_quota + cls.get_spendable_balance_value(wallet)
                    else:
                        combined_snapshot = remaining_quota

                    return WalletAccessResult(
                        True,
                        combined_snapshot,
                        "OK",
                        wallet,
                        combined_snapshot,
                        subscription_id,
                        remaining_quota,
                    )

                wallet_access = cls._check_wallet_only_access(wallet)
                if wallet_access.allowed:
                    return WalletAccessResult(
                        True,
                        wallet_access.remaining,
                        "OK",
                        wallet_access.wallet,
                        wallet_access.balance_snapshot,
                        subscription_id,
                        Decimal("0"),
                    )

                message_map = {
                    "钱包不存在": "订阅额度已用尽，且钱包不存在",
                    "钱包不可用": "订阅额度已用尽，且钱包不可用",
                    "钱包欠费，请先充值": "订阅额度已用尽，且钱包欠费，请先充值",
                    "钱包余额不足": "订阅额度已用尽，且钱包余额不足",
                }
                return WalletAccessResult(
                    False,
                    wallet_access.remaining,
                    message_map.get(wallet_access.message, "订阅额度已用尽"),
                    wallet_access.wallet,
                    wallet_access.balance_snapshot,
                    subscription_id,
                    Decimal("0"),
                )

        return cls._check_wallet_only_access(wallet)

    @classmethod
    def get_balance_snapshot(
        cls,
        db: Session,
        *,
        user: User | None,
        api_key: ApiKey | None = None,
    ) -> Decimal | None:
        access = cls.check_request_allowed(db, user=user, api_key=api_key)
        return access.balance_snapshot if access.balance_snapshot is not None else access.remaining

    @classmethod
    def _resolve_wallet_for_usage(
        cls, db: Session, usage: Usage, *, for_update: bool = False
    ) -> Wallet | None:
        """解析 Usage 对应的钱包。for_update=True 时直接返回加锁的钱包，避免二次查询。"""
        wallet_id = None
        if usage.wallet_id:
            wallet_id = usage.wallet_id
        else:
            api_key = None
            if usage.api_key_id:
                api_key = db.query(ApiKey).filter(ApiKey.id == usage.api_key_id).first()
                if api_key and api_key.is_standalone:
                    wallet = cls.get_or_create_wallet(db, api_key=api_key)
                    wallet_id = wallet.id if wallet else None
            if wallet_id is None and usage.user_id:
                user = db.query(User).filter(User.id == usage.user_id).first()
                wallet = cls.get_or_create_wallet(db, user=user, api_key=api_key)
                wallet_id = wallet.id if wallet else None

        if wallet_id is None:
            return None

        query = db.query(Wallet).filter(Wallet.id == wallet_id)
        if for_update:
            query = query.with_for_update()
        return query.first()

    @classmethod
    def apply_usage_charge(
        cls,
        db: Session,
        *,
        usage: Usage,
        amount_usd: Decimal | float | int | str,
    ) -> tuple[Decimal | None, Decimal | None]:
        amount = to_money_decimal(amount_usd)
        if amount <= Decimal("0"):
            return None, None

        from src.services.subscription import SubscriptionService

        subscription_charge = SubscriptionService.apply_usage_charge(
            db,
            user_id=usage.user_id,
            amount_usd=amount,
        )
        wallet_charge_amount = amount
        consumed_from_subscription = Decimal("0")

        if subscription_charge.subscription is not None:
            usage.subscription_id = subscription_charge.subscription.id
            usage.subscription_quota_before_usd = subscription_charge.quota_before
            usage.subscription_quota_after_usd = subscription_charge.quota_after
            wallet_charge_amount = subscription_charge.wallet_charge_amount
            consumed_from_subscription = subscription_charge.consumed_from_quota
            if wallet_charge_amount <= Decimal("0"):
                usage.billing_source = "subscription"
                return None, None
        else:
            usage.subscription_id = None
            usage.subscription_quota_before_usd = None
            usage.subscription_quota_after_usd = None

        locked_wallet = cls._resolve_wallet_for_usage(db, usage, for_update=True)
        if locked_wallet is None:
            if consumed_from_subscription > Decimal("0"):
                usage.billing_source = "subscription"
            return None, None

        before_recharge = cls.get_recharge_balance_value(locked_wallet)
        before_gift = cls.get_gift_balance_value(locked_wallet)
        before_total = before_recharge + before_gift

        if cls.is_unlimited_wallet(locked_wallet):
            locked_wallet.total_consumed = (
                to_money_decimal(locked_wallet.total_consumed) + wallet_charge_amount
            )
            locked_wallet.updated_at = datetime.now(timezone.utc)
            db.flush()

            usage.wallet_id = locked_wallet.id
            usage.wallet_balance_before = before_total
            usage.wallet_balance_after = before_total
            usage.wallet_recharge_balance_before = before_recharge
            usage.wallet_recharge_balance_after = before_recharge
            usage.wallet_gift_balance_before = before_gift
            usage.wallet_gift_balance_after = before_gift
            usage.billing_source = (
                "mixed" if consumed_from_subscription > Decimal("0") else "unlimited_wallet"
            )
            return before_total, before_total

        # 赠款优先扣减：赠款不可退款，优先消耗可避免与充值余额混淆。
        gift_deduction = min(max(before_gift, Decimal("0")), wallet_charge_amount)
        recharge_deduction = wallet_charge_amount - gift_deduction

        after_gift = before_gift - gift_deduction
        after_recharge = before_recharge - recharge_deduction
        after_total = after_recharge + after_gift

        locked_wallet.balance = after_recharge
        locked_wallet.gift_balance = after_gift
        locked_wallet.total_consumed = (
            to_money_decimal(locked_wallet.total_consumed) + wallet_charge_amount
        )
        locked_wallet.updated_at = datetime.now(timezone.utc)
        db.flush()
        usage.wallet_id = locked_wallet.id
        usage.wallet_balance_before = before_total
        usage.wallet_balance_after = after_total
        usage.wallet_recharge_balance_before = before_recharge
        usage.wallet_recharge_balance_after = after_recharge
        usage.wallet_gift_balance_before = before_gift
        usage.wallet_gift_balance_after = after_gift
        usage.billing_source = "mixed" if consumed_from_subscription > Decimal("0") else "wallet"
        return before_total, after_total

    @classmethod
    def reconcile_usage_charge_delta(
        cls,
        db: Session,
        *,
        usage: Usage,
        previous_amount_usd: Decimal | float | int | str,
        next_amount_usd: Decimal | float | int | str,
    ) -> tuple[Decimal | None, Decimal | None]:
        """按差额修正已结算 Usage 对钱包的影响。

        设计目标：
        - 只做短事务、短锁，适合在线后台维护任务增量修正历史账单
        - 正向差额（新费用更高）沿用当前钱包的 gift -> recharge 扣减规则
        - 负向差额（新费用更低）优先按原始扣减的反向顺序返还：recharge -> gift
        - 若该 Usage 历史上按 unlimited 语义结算，则仅修正 total_consumed，不改余额
        """

        previous_amount = to_money_decimal(previous_amount_usd)
        next_amount = to_money_decimal(next_amount_usd)
        delta = next_amount - previous_amount
        if delta == Decimal("0"):
            return None, None

        locked_wallet = cls._resolve_wallet_for_usage(db, usage, for_update=True)
        if locked_wallet is None:
            return None, None

        before_recharge = cls.get_recharge_balance_value(locked_wallet)
        before_gift = cls.get_gift_balance_value(locked_wallet)
        before_total = before_recharge + before_gift

        usage_before_total = to_money_decimal(getattr(usage, "wallet_balance_before", None))
        usage_after_total = to_money_decimal(getattr(usage, "wallet_balance_after", None))
        usage_before_recharge = to_money_decimal(
            getattr(usage, "wallet_recharge_balance_before", None)
        )
        usage_after_recharge = to_money_decimal(getattr(usage, "wallet_recharge_balance_after", None))
        usage_before_gift = to_money_decimal(getattr(usage, "wallet_gift_balance_before", None))
        usage_after_gift = to_money_decimal(getattr(usage, "wallet_gift_balance_after", None))

        historical_unlimited = (
            previous_amount > Decimal("0")
            and usage.wallet_balance_before is not None
            and usage.wallet_balance_after is not None
            and usage_before_total == usage_after_total
            and usage.wallet_recharge_balance_before is not None
            and usage.wallet_recharge_balance_after is not None
            and usage_before_recharge == usage_after_recharge
            and usage.wallet_gift_balance_before is not None
            and usage.wallet_gift_balance_after is not None
            and usage_before_gift == usage_after_gift
        )

        if historical_unlimited:
            locked_wallet.total_consumed = to_money_decimal(locked_wallet.total_consumed) + delta
            locked_wallet.updated_at = datetime.now(timezone.utc)
            return before_total, before_total

        if delta > Decimal("0"):
            additional = delta
            gift_deduction = min(max(before_gift, Decimal("0")), additional)
            recharge_deduction = additional - gift_deduction

            after_gift = before_gift - gift_deduction
            after_recharge = before_recharge - recharge_deduction
        else:
            refund = -delta

            original_recharge_deduction = max(usage_before_recharge - usage_after_recharge, Decimal("0"))
            original_gift_deduction = max(usage_before_gift - usage_after_gift, Decimal("0"))

            recharge_refund = min(refund, original_recharge_deduction)
            remaining_refund = refund - recharge_refund
            gift_refund = min(remaining_refund, original_gift_deduction)
            residual_refund = remaining_refund - gift_refund

            # 历史快照缺失时兜底返还到 recharge，避免吞掉差额。
            recharge_refund += residual_refund

            after_recharge = before_recharge + recharge_refund
            after_gift = before_gift + gift_refund

        after_total = after_recharge + after_gift

        locked_wallet.balance = after_recharge
        locked_wallet.gift_balance = after_gift
        locked_wallet.total_consumed = to_money_decimal(locked_wallet.total_consumed) + delta
        locked_wallet.updated_at = datetime.now(timezone.utc)

        return before_total, after_total

    @classmethod
    def set_wallet_limit_mode(
        cls,
        db: Session,
        *,
        wallet: Wallet,
        limit_mode: Literal["finite", "unlimited"],
    ) -> Wallet:
        if limit_mode not in {"finite", "unlimited"}:
            raise ValueError("limit_mode must be finite or unlimited")

        locked_wallet = (
            db.query(Wallet).filter(Wallet.id == wallet.id).with_for_update().one_or_none()
        )
        if locked_wallet is None:
            raise ValueError("wallet not found")

        locked_wallet.limit_mode = limit_mode
        locked_wallet.updated_at = datetime.now(timezone.utc)
        db.flush()
        return locked_wallet

    @classmethod
    def create_wallet_transaction(
        cls,
        db: Session,
        *,
        wallet: Wallet,
        category: WalletCategory,
        reason_code: str,
        amount: Decimal | float | int | str,
        balance_type: WalletBalanceBucket | None = None,
        link_type: str | None = None,
        link_id: str | None = None,
        operator_id: str | None = None,
        description: str | None = None,
    ) -> WalletTransaction:
        if category not in {"recharge", "gift", "adjust", "refund"}:
            raise ValueError("category must be recharge/gift/adjust/refund")
        if not reason_code:
            raise ValueError("reason_code is required")

        locked_wallet = (
            db.query(Wallet).filter(Wallet.id == wallet.id).with_for_update().one_or_none()
        )
        if locked_wallet is None:
            raise ValueError("wallet not found")

        delta = to_money_decimal(amount)
        bucket = balance_type
        if bucket is None:
            bucket = "gift" if category == "gift" else "recharge"

        before_recharge = cls.get_recharge_balance_value(locked_wallet)
        before_gift = cls.get_gift_balance_value(locked_wallet)
        before_total = before_recharge + before_gift

        after_recharge = before_recharge
        after_gift = before_gift
        if bucket == "recharge":
            after_recharge = before_recharge + delta
        else:
            after_gift = before_gift + delta
        after_total = after_recharge + after_gift

        if category == "refund" and bucket != "recharge":
            raise ValueError("refund transaction must use recharge balance")
        if category == "refund" and delta < Decimal("0") and after_recharge < Decimal("0"):
            raise ValueError("refund amount exceeds refundable recharge balance")
        if bucket == "gift" and delta < Decimal("0") and after_gift < Decimal("0"):
            raise ValueError("gift balance cannot be negative")
        if bucket == "gift" and locked_wallet.api_key_id is not None:
            raise ValueError("api key wallet does not support gift balance")

        locked_wallet.balance = after_recharge
        locked_wallet.gift_balance = after_gift
        locked_wallet.updated_at = datetime.now(timezone.utc)

        if category == "recharge":
            locked_wallet.total_recharged = to_money_decimal(locked_wallet.total_recharged) + delta
        elif category == "refund":
            # refund_out 为负值（累计退款增加）；refund_revert 为正值（累计退款回退）。
            next_total_refunded = to_money_decimal(locked_wallet.total_refunded) - delta
            locked_wallet.total_refunded = max(next_total_refunded, Decimal("0"))
        elif category in {"gift", "adjust"}:
            locked_wallet.total_adjusted = to_money_decimal(locked_wallet.total_adjusted) + delta

        tx = WalletTransaction(
            wallet_id=locked_wallet.id,
            category=category,
            reason_code=reason_code,
            amount=delta,
            balance_before=before_total,
            balance_after=after_total,
            recharge_balance_before=before_recharge,
            recharge_balance_after=after_recharge,
            gift_balance_before=before_gift,
            gift_balance_after=after_gift,
            link_type=link_type,
            link_id=link_id,
            operator_id=operator_id,
            description=description,
        )
        db.add(tx)
        db.flush()
        return tx

    @classmethod
    def create_manual_recharge_order(
        cls,
        db: Session,
        *,
        wallet: Wallet,
        amount_usd: Decimal | float | int | str,
        payment_method: str = "admin_manual",
        operator_id: str | None = None,
        description: str | None = None,
        reason_code: str | None = None,
        link_type: str = "payment_order",
        link_id: str | None = None,
    ) -> PaymentOrder:
        amount = to_money_decimal(amount_usd)
        if amount <= Decimal("0"):
            raise ValueError("recharge amount must be positive")
        if wallet.api_key_id is not None:
            raise ValueError("api key wallet does not support recharge, use adjust instead")

        now = datetime.now(timezone.utc)
        order = PaymentOrder(
            order_no=cls._build_order_no("po"),
            wallet_id=wallet.id,
            user_id=wallet.user_id,
            order_type="topup",
            amount_usd=amount,
            refunded_amount_usd=Decimal("0"),
            refundable_amount_usd=amount,
            payment_method=payment_method,
            status="credited",
            paid_at=now,
            credited_at=now,
            gateway_response={
                "source": "manual",
                "operator_id": operator_id,
                "description": description,
            },
        )
        db.add(order)
        db.flush()

        tx_reason = reason_code
        if tx_reason is None:
            if payment_method in {"card_code", "gift_code", "card_recharge"}:
                tx_reason = REASON_TOPUP_CARD_CODE
            else:
                tx_reason = REASON_TOPUP_ADMIN_MANUAL

        cls.create_wallet_transaction(
            db,
            wallet=wallet,
            category="recharge",
            reason_code=tx_reason,
            amount=amount,
            balance_type="recharge",
            link_type=link_type,
            link_id=link_id or order.id,
            operator_id=operator_id,
            description=description or "管理员充值",
        )
        return order

    @classmethod
    def admin_adjust_balance(
        cls,
        db: Session,
        *,
        wallet: Wallet,
        amount_usd: Decimal | float | int | str,
        balance_type: Literal["recharge", "gift"] = "recharge",
        operator_id: str | None = None,
        description: str | None = None,
    ) -> WalletTransaction:
        amount = to_money_decimal(amount_usd)
        if amount == Decimal("0"):
            raise ValueError("adjust amount must not be zero")
        if balance_type not in {"recharge", "gift"}:
            raise ValueError("balance_type must be recharge or gift")
        if balance_type == "gift" and wallet.api_key_id is not None:
            raise ValueError("api key wallet does not support gift balance")
        # 正向调账：加给谁就加给谁，不做抵充。
        if amount > Decimal("0"):
            return cls.create_wallet_transaction(
                db,
                wallet=wallet,
                category="adjust",
                reason_code=REASON_ADJUST_ADMIN,
                amount=amount,
                balance_type=balance_type,
                link_type="admin_action",
                link_id=wallet.id,
                operator_id=operator_id,
                description=description or "管理员调账",
            )

        # 负向调账：先扣所选账户，再扣另一账户；若仍不足，继续计入充值余额（可为负）。
        locked_wallet = (
            db.query(Wallet).filter(Wallet.id == wallet.id).with_for_update().one_or_none()
        )
        if locked_wallet is None:
            raise ValueError("wallet not found")

        before_recharge = cls.get_recharge_balance_value(locked_wallet)
        before_gift = cls.get_gift_balance_value(locked_wallet)
        before_total = before_recharge + before_gift

        after_recharge = before_recharge
        after_gift = before_gift
        remaining = -amount

        def consume_positive_bucket(
            balance: Decimal, to_consume: Decimal
        ) -> tuple[Decimal, Decimal]:
            if to_consume <= Decimal("0"):
                return balance, Decimal("0")
            available = max(balance, Decimal("0"))
            consumed = min(available, to_consume)
            return balance - consumed, to_consume - consumed

        if balance_type == "gift":
            after_gift, remaining = consume_positive_bucket(after_gift, remaining)
            after_recharge, remaining = consume_positive_bucket(after_recharge, remaining)
        else:
            after_recharge, remaining = consume_positive_bucket(after_recharge, remaining)
            after_gift, remaining = consume_positive_bucket(after_gift, remaining)

        if remaining > Decimal("0"):
            after_recharge = after_recharge - remaining

        if after_gift < Decimal("0"):
            raise ValueError("gift balance cannot be negative")

        after_total = after_recharge + after_gift

        locked_wallet.balance = after_recharge
        locked_wallet.gift_balance = after_gift
        locked_wallet.updated_at = datetime.now(timezone.utc)
        locked_wallet.total_adjusted = to_money_decimal(locked_wallet.total_adjusted) + amount

        tx = WalletTransaction(
            wallet_id=locked_wallet.id,
            category="adjust",
            reason_code=REASON_ADJUST_ADMIN,
            amount=amount,
            balance_before=before_total,
            balance_after=after_total,
            recharge_balance_before=before_recharge,
            recharge_balance_after=after_recharge,
            gift_balance_before=before_gift,
            gift_balance_after=after_gift,
            link_type="admin_action",
            link_id=wallet.id,
            operator_id=operator_id,
            description=description or "管理员调账",
        )
        db.add(tx)
        db.flush()
        return tx

    @classmethod
    def _get_pending_refund_reserved_amount(
        cls,
        db: Session,
        *,
        wallet_id: str | None = None,
        payment_order_id: str | None = None,
    ) -> Decimal:
        query = db.query(func.coalesce(func.sum(RefundRequest.amount_usd), 0)).filter(
            RefundRequest.status.in_(["pending_approval", "approved"])
        )
        if wallet_id is not None:
            query = query.filter(RefundRequest.wallet_id == wallet_id)
        if payment_order_id is not None:
            query = query.filter(RefundRequest.payment_order_id == payment_order_id)
        return to_money_decimal(query.scalar() or 0)

    @classmethod
    def create_refund_request(
        cls,
        db: Session,
        *,
        wallet: Wallet,
        user_id: str | None,
        amount_usd: Decimal | float | int | str,
        refund_no: str,
        source_type: str,
        source_id: str | None,
        refund_mode: str,
        payment_order: PaymentOrder | None = None,
        reason: str | None = None,
        requested_by: str | None = None,
        idempotency_key: str | None = None,
    ) -> RefundRequest:
        amount = to_money_decimal(amount_usd)
        if amount <= Decimal("0"):
            raise ValueError("refund amount must be positive")

        locked_wallet = (
            db.query(Wallet).filter(Wallet.id == wallet.id).with_for_update().one_or_none()
        )
        if locked_wallet is None:
            raise ValueError("wallet not found")

        refundable_balance = cls.get_refundable_balance_value(locked_wallet)
        reserved_wallet_amount = cls._get_pending_refund_reserved_amount(
            db,
            wallet_id=locked_wallet.id,
        )
        available_refundable_balance = refundable_balance - reserved_wallet_amount
        if amount > available_refundable_balance:
            raise ValueError("refund amount exceeds available refundable recharge balance")

        locked_payment_order = None
        if payment_order is not None:
            locked_payment_order = (
                db.query(PaymentOrder)
                .filter(PaymentOrder.id == payment_order.id)
                .with_for_update()
                .one_or_none()
            )
            if locked_payment_order is None:
                raise ValueError("payment order not found")
            if locked_payment_order.wallet_id != locked_wallet.id:
                raise ValueError("payment order does not belong to wallet")
            if locked_payment_order.status != "credited":
                raise ValueError("payment order is not refundable")

            refundable_amount = to_money_decimal(locked_payment_order.refundable_amount_usd)
            reserved_order_amount = cls._get_pending_refund_reserved_amount(
                db,
                payment_order_id=locked_payment_order.id,
            )
            available_refundable_amount = refundable_amount - reserved_order_amount
            if amount > available_refundable_amount:
                raise ValueError("refund amount exceeds available refundable amount")

        refund = RefundRequest(
            refund_no=refund_no,
            wallet_id=locked_wallet.id,
            user_id=user_id,
            payment_order_id=locked_payment_order.id if locked_payment_order else None,
            source_type=source_type,
            source_id=source_id,
            refund_mode=refund_mode,
            amount_usd=amount,
            status="pending_approval",
            reason=reason,
            requested_by=requested_by,
            idempotency_key=idempotency_key,
        )
        db.add(refund)
        db.flush()
        return refund

    @classmethod
    def move_refund_to_processing(
        cls,
        db: Session,
        *,
        refund: RefundRequest,
        operator_id: str | None = None,
    ) -> WalletTransaction:
        locked_refund = (
            db.query(RefundRequest)
            .filter(RefundRequest.id == refund.id)
            .with_for_update()
            .one_or_none()
        )
        if locked_refund is None:
            raise ValueError("refund not found")

        if locked_refund.status not in {"approved", "pending_approval"}:
            raise ValueError("refund status is not approvable")

        locked_wallet = (
            db.query(Wallet)
            .filter(Wallet.id == locked_refund.wallet_id)
            .with_for_update()
            .one_or_none()
        )
        if locked_wallet is None:
            raise ValueError("wallet not found")

        payment_order = None
        if locked_refund.payment_order_id:
            payment_order = (
                db.query(PaymentOrder)
                .filter(PaymentOrder.id == locked_refund.payment_order_id)
                .with_for_update()
                .one_or_none()
            )
            if payment_order is None:
                raise ValueError("payment order not found")

            refund_amount = to_money_decimal(locked_refund.amount_usd)
            refundable_amount = to_money_decimal(payment_order.refundable_amount_usd)
            if refund_amount > refundable_amount:
                raise ValueError("refund amount exceeds refundable amount")

        tx = cls.create_wallet_transaction(
            db,
            wallet=locked_wallet,
            category="refund",
            reason_code=REASON_REFUND_OUT,
            amount=-to_money_decimal(locked_refund.amount_usd),
            balance_type="recharge",
            link_type="refund_request",
            link_id=locked_refund.id,
            operator_id=operator_id,
            description="退款占款",
        )

        if payment_order is not None:
            delta = to_money_decimal(locked_refund.amount_usd)
            payment_order.refunded_amount_usd = (
                to_money_decimal(payment_order.refunded_amount_usd) + delta
            )
            payment_order.refundable_amount_usd = (
                to_money_decimal(payment_order.refundable_amount_usd) - delta
            )

        locked_refund.status = "processing"
        locked_refund.approved_by = operator_id
        locked_refund.processed_by = operator_id
        locked_refund.processed_at = datetime.now(timezone.utc)
        locked_refund.updated_at = datetime.now(timezone.utc)
        return tx

    @classmethod
    def fail_refund(
        cls,
        db: Session,
        *,
        refund: RefundRequest,
        reason: str,
        operator_id: str | None = None,
    ) -> WalletTransaction | None:
        locked_refund = (
            db.query(RefundRequest)
            .filter(RefundRequest.id == refund.id)
            .with_for_update()
            .one_or_none()
        )
        if locked_refund is None:
            raise ValueError("refund not found")

        if locked_refund.status in {"pending_approval", "approved"}:
            locked_refund.status = "failed"
            locked_refund.failure_reason = reason
            locked_refund.updated_at = datetime.now(timezone.utc)
            return None
        if locked_refund.status != "processing":
            raise ValueError(f"cannot fail refund in status: {locked_refund.status}")

        wallet = db.query(Wallet).filter(Wallet.id == locked_refund.wallet_id).first()
        if wallet is None:
            raise ValueError("wallet not found")

        tx = cls.create_wallet_transaction(
            db,
            wallet=wallet,
            category="refund",
            reason_code=REASON_REFUND_REVERT,
            amount=to_money_decimal(locked_refund.amount_usd),
            balance_type="recharge",
            link_type="refund_request",
            link_id=locked_refund.id,
            operator_id=operator_id,
            description="退款失败回补",
        )

        if locked_refund.payment_order_id:
            payment_order = (
                db.query(PaymentOrder)
                .filter(PaymentOrder.id == locked_refund.payment_order_id)
                .with_for_update()
                .one_or_none()
            )
            if payment_order is not None:
                delta = to_money_decimal(locked_refund.amount_usd)
                payment_order.refunded_amount_usd = (
                    to_money_decimal(payment_order.refunded_amount_usd) - delta
                )
                payment_order.refundable_amount_usd = (
                    to_money_decimal(payment_order.refundable_amount_usd) + delta
                )

        locked_refund.status = "failed"
        locked_refund.failure_reason = reason
        locked_refund.updated_at = datetime.now(timezone.utc)
        return tx

    @classmethod
    def complete_refund(
        cls,
        db: Session,
        *,
        refund: RefundRequest,
        gateway_refund_id: str | None = None,
        payout_reference: str | None = None,
        payout_proof: dict | None = None,
    ) -> RefundRequest:
        locked_refund = (
            db.query(RefundRequest)
            .filter(RefundRequest.id == refund.id)
            .with_for_update()
            .one_or_none()
        )
        if locked_refund is None:
            raise ValueError("refund not found")
        if locked_refund.status != "processing":
            raise ValueError("refund status must be processing before completion")

        locked_refund.status = "succeeded"
        locked_refund.gateway_refund_id = gateway_refund_id
        locked_refund.payout_reference = payout_reference
        locked_refund.payout_proof = payout_proof
        locked_refund.completed_at = datetime.now(timezone.utc)
        locked_refund.updated_at = datetime.now(timezone.utc)
        return locked_refund
