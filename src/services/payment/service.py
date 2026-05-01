from __future__ import annotations

import hashlib
import json
from datetime import datetime, timedelta, timezone
from decimal import Decimal
from typing import Any
from uuid import uuid4

from sqlalchemy.orm import Session

from src.models.database import PaymentCallback, PaymentOrder, User, Wallet
from src.services.billing.precision import to_money_decimal
from src.services.payment.gateway import get_payment_gateway
from src.services.wallet import WalletService


class PaymentService:
    """支付订单与回调处理服务。

    当前实现目标：
    - 打通充值订单创建
    - 打通支付回调幂等到账
    - 真实网关签名/SDK 留给后续渠道适配层
    """

    @staticmethod
    def _build_order_no() -> str:
        ts = datetime.now(timezone.utc).strftime("%Y%m%d%H%M%S%f")
        return f"po_{ts}_{uuid4().hex[:12]}"

    @staticmethod
    def _build_payload_hash(payload: dict[str, Any] | None) -> str | None:
        if payload is None:
            return None
        encoded = json.dumps(payload, sort_keys=True, ensure_ascii=False, default=str).encode(
            "utf-8"
        )
        return hashlib.sha256(encoded).hexdigest()

    @staticmethod
    def _is_manual_review_method(payment_method: str | None) -> bool:
        normalized = str(payment_method or "").strip().lower()
        return normalized in {"manual", "manual_review"}

    @classmethod
    def create_recharge_order(
        cls,
        db: Session,
        *,
        user: User,
        amount_usd: Decimal | float | int | str,
        payment_method: str,
        pay_amount: Decimal | float | int | str | None = None,
        pay_currency: str | None = None,
        exchange_rate: Decimal | float | int | str | None = None,
        expires_in_minutes: int = 30,
        gateway_order_id: str | None = None,
        gateway_response: dict[str, Any] | None = None,
    ) -> PaymentOrder:
        amount = to_money_decimal(amount_usd)
        if amount <= Decimal("0"):
            raise ValueError("recharge amount must be positive")
        if not payment_method:
            raise ValueError("payment_method is required")
        if payment_method == "admin_manual":
            raise ValueError("admin_manual is reserved for admin recharge")
        gateway = get_payment_gateway(payment_method)

        wallet = WalletService.get_or_create_wallet(db, user=user)
        if wallet is None:
            raise ValueError("wallet not available")
        if wallet.status != "active":
            raise ValueError("wallet is not active")

        now = datetime.now(timezone.utc)
        is_manual_review = cls._is_manual_review_method(payment_method)
        order = PaymentOrder(
            order_no=cls._build_order_no(),
            wallet_id=wallet.id,
            user_id=user.id,
            order_type="topup",
            amount_usd=amount,
            pay_amount=to_money_decimal(pay_amount) if pay_amount is not None else None,
            pay_currency=pay_currency,
            exchange_rate=to_money_decimal(exchange_rate) if exchange_rate is not None else None,
            refunded_amount_usd=Decimal("0"),
            refundable_amount_usd=Decimal("0"),
            payment_method=payment_method,
            gateway_order_id=gateway_order_id,
            gateway_response=gateway_response,
            status="pending_approval" if is_manual_review else "pending",
            expires_at=(
                None
                if is_manual_review
                else now + timedelta(minutes=max(expires_in_minutes, 1))
            ),
        )
        db.add(order)
        db.flush()
        checkout = gateway.create_checkout_payload(order=order)
        order.gateway_order_id = order.gateway_order_id or checkout.get("gateway_order_id")
        order.gateway_response = gateway_response if gateway_response is not None else checkout
        return order

    @classmethod
    def create_subscription_order(
        cls,
        db: Session,
        *,
        user: User,
        subscription_id: str,
        amount_usd: Decimal | float | int | str,
        payment_method: str,
        order_type: str,
        pay_amount: Decimal | float | int | str | None = None,
        pay_currency: str | None = None,
        exchange_rate: Decimal | float | int | str | None = None,
        expires_in_minutes: int = 30,
        gateway_order_id: str | None = None,
        gateway_response: dict[str, Any] | None = None,
    ) -> PaymentOrder:
        amount = to_money_decimal(amount_usd)
        if amount <= Decimal("0"):
            raise ValueError("subscription amount must be positive")
        if not subscription_id:
            raise ValueError("subscription_id is required")
        if order_type not in {
            "subscription_initial",
            "subscription_upgrade",
            "subscription_renewal",
        }:
            raise ValueError("unsupported subscription order type")
        if not payment_method:
            raise ValueError("payment_method is required")
        if payment_method == "admin_manual":
            raise ValueError("admin_manual is reserved for admin recharge")
        gateway = get_payment_gateway(payment_method)

        wallet = WalletService.get_or_create_wallet(db, user=user)
        if wallet is None:
            raise ValueError("wallet not available")
        if wallet.status != "active":
            raise ValueError("wallet is not active")

        now = datetime.now(timezone.utc)
        is_manual_review = cls._is_manual_review_method(payment_method)
        order = PaymentOrder(
            order_no=cls._build_order_no(),
            wallet_id=wallet.id,
            user_id=user.id,
            subscription_id=subscription_id,
            order_type=order_type,
            amount_usd=amount,
            pay_amount=to_money_decimal(pay_amount) if pay_amount is not None else None,
            pay_currency=pay_currency,
            exchange_rate=to_money_decimal(exchange_rate) if exchange_rate is not None else None,
            refunded_amount_usd=Decimal("0"),
            refundable_amount_usd=Decimal("0"),
            payment_method=payment_method,
            gateway_order_id=gateway_order_id,
            gateway_response=gateway_response,
            status="pending_approval" if is_manual_review else "pending",
            expires_at=(
                None
                if is_manual_review
                else now + timedelta(minutes=max(expires_in_minutes, 1))
            ),
        )
        db.add(order)
        db.flush()
        checkout = gateway.create_checkout_payload(order=order)
        order.gateway_order_id = order.gateway_order_id or checkout.get("gateway_order_id")
        order.gateway_response = gateway_response if gateway_response is not None else checkout
        return order

    @classmethod
    def create_settled_subscription_order(
        cls,
        db: Session,
        *,
        user: User,
        subscription_id: str,
        amount_usd: Decimal | float | int | str,
        payment_method: str,
        order_type: str,
        gateway_response: dict[str, Any] | None = None,
    ) -> PaymentOrder:
        amount = to_money_decimal(amount_usd)
        if amount < Decimal("0"):
            raise ValueError("subscription amount cannot be negative")
        if not subscription_id:
            raise ValueError("subscription_id is required")
        if order_type not in {
            "subscription_initial",
            "subscription_upgrade",
            "subscription_renewal",
        }:
            raise ValueError("unsupported subscription order type")
        if not payment_method:
            raise ValueError("payment_method is required")

        wallet = WalletService.get_or_create_wallet(db, user=user)
        if wallet is None:
            raise ValueError("wallet not available")
        if wallet.status != "active":
            raise ValueError("wallet is not active")

        now = datetime.now(timezone.utc)
        order = PaymentOrder(
            order_no=cls._build_order_no(),
            wallet_id=wallet.id,
            user_id=user.id,
            subscription_id=subscription_id,
            order_type=order_type,
            amount_usd=amount,
            refunded_amount_usd=Decimal("0"),
            refundable_amount_usd=amount,
            payment_method=payment_method,
            gateway_response=(
                gateway_response
                if gateway_response is not None
                else {"gateway": payment_method, "manual_credit": True}
            ),
            status="credited",
            paid_at=now,
            credited_at=now,
            expires_at=None,
        )
        db.add(order)
        db.flush()
        return order

    @classmethod
    def refresh_order_status(cls, order: PaymentOrder | None) -> bool:
        if order is None:
            return False
        if order.status != "pending":
            return False
        now = datetime.now(timezone.utc)
        if order.expires_at is not None and order.expires_at < now:
            order.status = "expired"
            return True
        return False

    @staticmethod
    def get_order(
        db: Session,
        *,
        order_id: str | None = None,
        order_no: str | None = None,
        gateway_order_id: str | None = None,
    ) -> PaymentOrder | None:
        if order_id:
            return db.query(PaymentOrder).filter(PaymentOrder.id == order_id).first()
        if order_no:
            return db.query(PaymentOrder).filter(PaymentOrder.order_no == order_no).first()
        if gateway_order_id:
            return (
                db.query(PaymentOrder)
                .filter(PaymentOrder.gateway_order_id == gateway_order_id)
                .first()
            )
        return None

    @classmethod
    def list_user_orders(
        cls,
        db: Session,
        *,
        user_id: str,
        limit: int,
        offset: int,
        order_types: str | list[str] | tuple[str, ...] | None = None,
    ) -> tuple[list[PaymentOrder], int, bool]:
        expired_count = cls.expire_overdue_pending_orders(db, user_id=user_id)
        q = db.query(PaymentOrder).filter(PaymentOrder.user_id == user_id)
        if order_types:
            if isinstance(order_types, str):
                q = q.filter(PaymentOrder.order_type == order_types)
            else:
                q = q.filter(PaymentOrder.order_type.in_(list(order_types)))
        total = q.count()
        items = q.order_by(PaymentOrder.created_at.desc()).offset(offset).limit(limit).all()
        return items, total, expired_count > 0

    @classmethod
    def list_orders(
        cls,
        db: Session,
        *,
        status: str | None = None,
        payment_method: str | None = None,
        order_types: str | list[str] | tuple[str, ...] | None = None,
        limit: int = 50,
        offset: int = 0,
    ) -> tuple[list[PaymentOrder], int, bool]:
        expired_count = 0
        if status in {None, "pending", "expired"}:
            expired_count = cls.expire_overdue_pending_orders(
                db,
                payment_method=payment_method,
                order_types=order_types,
            )

        q = db.query(PaymentOrder)
        if status:
            q = q.filter(PaymentOrder.status == status)
        if payment_method:
            q = q.filter(PaymentOrder.payment_method == payment_method)
        if order_types:
            if isinstance(order_types, str):
                q = q.filter(PaymentOrder.order_type == order_types)
            else:
                q = q.filter(PaymentOrder.order_type.in_(list(order_types)))
        total = q.count()
        items = q.order_by(PaymentOrder.created_at.desc()).offset(offset).limit(limit).all()
        return items, total, expired_count > 0

    @staticmethod
    def expire_overdue_pending_orders(
        db: Session,
        *,
        user_id: str | None = None,
        payment_method: str | None = None,
        order_types: str | list[str] | tuple[str, ...] | None = None,
    ) -> int:
        now = datetime.now(timezone.utc)
        q = db.query(PaymentOrder).filter(
            PaymentOrder.status == "pending",
            PaymentOrder.expires_at.isnot(None),
            PaymentOrder.expires_at < now,
        )
        if user_id:
            q = q.filter(PaymentOrder.user_id == user_id)
        if payment_method:
            q = q.filter(PaymentOrder.payment_method == payment_method)
        if order_types:
            if isinstance(order_types, str):
                q = q.filter(PaymentOrder.order_type == order_types)
            else:
                q = q.filter(PaymentOrder.order_type.in_(list(order_types)))
        orders = q.all()
        count = 0
        for order in orders:
            _updated, changed = PaymentService.expire_order(
                db,
                order=order,
                reason="payment order expired",
            )
            if changed:
                count += 1
        return count

    @staticmethod
    def list_callbacks(
        db: Session,
        *,
        payment_method: str | None = None,
        order_types: str | list[str] | tuple[str, ...] | None = None,
        limit: int = 50,
        offset: int = 0,
    ) -> tuple[list[PaymentCallback], int]:
        q = db.query(PaymentCallback)
        if order_types:
            q = q.join(PaymentOrder, PaymentCallback.payment_order_id == PaymentOrder.id)
            if isinstance(order_types, str):
                q = q.filter(PaymentOrder.order_type == order_types)
            else:
                q = q.filter(PaymentOrder.order_type.in_(list(order_types)))
        if payment_method:
            q = q.filter(PaymentCallback.payment_method == payment_method)
        total = q.count()
        items = (
            q.order_by(PaymentCallback.created_at.desc(), PaymentCallback.id.desc())
            .offset(offset)
            .limit(limit)
            .all()
        )
        return items, total

    @staticmethod
    def get_user_order(
        db: Session,
        *,
        user_id: str,
        order_id: str,
    ) -> PaymentOrder | None:
        return (
            db.query(PaymentOrder)
            .filter(PaymentOrder.id == order_id, PaymentOrder.user_id == user_id)
            .first()
        )

    @classmethod
    def fail_order(
        cls,
        db: Session,
        *,
        order: PaymentOrder,
        reason: str | None = None,
    ) -> PaymentOrder:
        locked_order = (
            db.query(PaymentOrder)
            .filter(PaymentOrder.id == order.id)
            .with_for_update()
            .one_or_none()
        )
        if locked_order is None:
            raise ValueError("payment order not found")
        if locked_order.status == "failed":
            return locked_order
        if locked_order.status == "credited":
            raise ValueError("credited order cannot be failed")
        if locked_order.status in {"expired", "refunded"}:
            raise ValueError(f"payment order cannot be marked failed: {locked_order.status}")
        if locked_order.status not in {"pending", "pending_approval", "paid"}:
            raise ValueError(f"payment order is not fail-markable: {locked_order.status}")
        locked_order.status = "failed"
        payload = dict(locked_order.gateway_response or {})
        if reason:
            payload["failure_reason"] = reason
        payload["failed_at"] = datetime.now(timezone.utc).isoformat()
        locked_order.gateway_response = payload
        if locked_order.subscription_id and locked_order.order_type in {
            "subscription_initial",
            "subscription_upgrade",
            "subscription_renewal",
        }:
            from src.services.subscription import SubscriptionService

            SubscriptionService.cancel_pending_subscription(
                db,
                locked_order.subscription_id,
                reason="payment_failed",
                commit=False,
            )
        return locked_order

    @classmethod
    def expire_order(
        cls,
        db: Session,
        *,
        order: PaymentOrder,
        reason: str | None = None,
    ) -> tuple[PaymentOrder, bool]:
        locked_order = (
            db.query(PaymentOrder)
            .filter(PaymentOrder.id == order.id)
            .with_for_update()
            .one_or_none()
        )
        if locked_order is None:
            raise ValueError("payment order not found")
        if locked_order.status == "credited":
            raise ValueError("credited order cannot be expired")
        if locked_order.status == "expired":
            return locked_order, False
        if locked_order.status not in {"pending", "pending_approval"}:
            raise ValueError(f"only pending order can be expired: {locked_order.status}")

        locked_order.status = "expired"
        payload = dict(locked_order.gateway_response or {})
        if reason:
            payload["expire_reason"] = reason
        payload["expired_at"] = datetime.now(timezone.utc).isoformat()
        locked_order.gateway_response = payload
        if locked_order.subscription_id and locked_order.order_type in {
            "subscription_initial",
            "subscription_upgrade",
            "subscription_renewal",
        }:
            from src.services.subscription import SubscriptionService

            SubscriptionService.cancel_pending_subscription(
                db,
                locked_order.subscription_id,
                reason="user_cancelled" if reason == "user_cancelled" else "payment_failed",
                commit=False,
            )
        return locked_order, True

    @classmethod
    def log_callback(
        cls,
        db: Session,
        *,
        payment_method: str,
        callback_key: str,
        order_no: str | None = None,
        gateway_order_id: str | None = None,
        payload: dict[str, Any] | None = None,
        signature_valid: bool = False,
        status: str = "received",
        payment_order: PaymentOrder | None = None,
        error_message: str | None = None,
    ) -> tuple[PaymentCallback, bool]:
        existing = (
            db.query(PaymentCallback).filter(PaymentCallback.callback_key == callback_key).first()
        )
        if existing is not None:
            return existing, False

        callback = PaymentCallback(
            payment_order_id=payment_order.id if payment_order else None,
            payment_method=payment_method,
            callback_key=callback_key,
            order_no=order_no,
            gateway_order_id=gateway_order_id,
            payload_hash=cls._build_payload_hash(payload),
            signature_valid=signature_valid,
            status=status,
            payload=payload,
            error_message=error_message,
        )
        db.add(callback)
        db.flush()
        return callback, True

    @classmethod
    def credit_order(
        cls,
        db: Session,
        *,
        order: PaymentOrder,
        gateway_order_id: str | None = None,
        gateway_response: dict[str, Any] | None = None,
        pay_amount: Decimal | float | int | str | None = None,
        pay_currency: str | None = None,
        exchange_rate: Decimal | float | int | str | None = None,
        operator_id: str | None = None,
    ) -> tuple[PaymentOrder, bool]:
        locked_order = (
            db.query(PaymentOrder)
            .filter(PaymentOrder.id == order.id)
            .with_for_update()
            .one_or_none()
        )
        if locked_order is None:
            raise ValueError("payment order not found")

        if locked_order.status == "credited":
            return locked_order, False
        if locked_order.status in {"failed", "expired", "refunded"}:
            raise ValueError(f"payment order is not creditable: {locked_order.status}")

        now = datetime.now(timezone.utc)
        if locked_order.expires_at is not None and locked_order.expires_at < now:
            locked_order.status = "expired"
            raise ValueError("payment order expired")

        if gateway_order_id:
            locked_order.gateway_order_id = gateway_order_id
        if gateway_response is not None:
            locked_order.gateway_response = gateway_response
        if pay_amount is not None:
            locked_order.pay_amount = to_money_decimal(pay_amount)
        if pay_currency is not None:
            locked_order.pay_currency = pay_currency
        if exchange_rate is not None:
            locked_order.exchange_rate = to_money_decimal(exchange_rate)

        locked_order.status = "paid"
        locked_order.paid_at = locked_order.paid_at or now

        order_type = str(locked_order.order_type or "topup")

        if order_type == "topup":
            wallet = db.query(Wallet).filter(Wallet.id == locked_order.wallet_id).first()
            if wallet is None:
                raise ValueError("wallet not found")
            if wallet.status != "active":
                raise ValueError("wallet is not active")

            is_manual_topup = str(locked_order.payment_method or "") in {"manual", "manual_review"}
            locked_order.refundable_amount_usd = to_money_decimal(locked_order.amount_usd)
            WalletService.create_wallet_transaction(
                db,
                wallet=wallet,
                category="recharge",
                reason_code="topup_admin_manual" if is_manual_topup else "topup_gateway",
                amount=locked_order.amount_usd,
                balance_type="recharge",
                link_type="payment_order",
                link_id=locked_order.id,
                operator_id=operator_id,
                description=(
                    "充值到账(人工充值)"
                    if is_manual_topup
                    else f"充值到账({locked_order.payment_method})"
                ),
            )
        elif order_type in {
            "subscription_initial",
            "subscription_upgrade",
            "subscription_renewal",
        }:
            if not locked_order.subscription_id:
                raise ValueError("subscription order missing subscription_id")
            from src.services.subscription import SubscriptionService

            SubscriptionService.activate_pending_subscription(
                db,
                locked_order.subscription_id,
                commit=False,
            )
        else:
            raise ValueError(f"unsupported payment order type: {locked_order.order_type}")

        locked_order.status = "credited"
        locked_order.credited_at = now
        return locked_order, True

    @classmethod
    def handle_callback(
        cls,
        db: Session,
        *,
        payment_method: str,
        callback_key: str,
        payload: dict[str, Any] | None,
        callback_signature: str | None,
        callback_secret: str | None,
        order_no: str | None = None,
        gateway_order_id: str | None = None,
        amount_usd: Decimal | float | int | str | None = None,
        pay_amount: Decimal | float | int | str | None = None,
        pay_currency: str | None = None,
        exchange_rate: Decimal | float | int | str | None = None,
    ) -> dict[str, Any]:
        gateway = get_payment_gateway(payment_method)
        verified = gateway.verify_callback_payload(
            payload=payload,
            callback_signature=callback_signature,
            callback_secret=callback_secret,
        )
        callback, created = cls.log_callback(
            db,
            payment_method=payment_method,
            callback_key=callback_key,
            order_no=order_no,
            gateway_order_id=gateway_order_id,
            payload=payload,
            signature_valid=verified,
        )
        if not created and callback.status == "processed":
            return {
                "ok": True,
                "duplicate": True,
                "credited": False,
                "order_id": callback.payment_order_id,
            }
        if not verified:
            callback.status = "failed"
            callback.error_message = "invalid callback signature"
            callback.processed_at = datetime.now(timezone.utc)
            return {"ok": False, "duplicate": not created, "error": callback.error_message}

        order = cls.get_order(
            db,
            order_no=order_no or callback.order_no,
            gateway_order_id=gateway_order_id or callback.gateway_order_id,
        )
        if order is None:
            callback.status = "failed"
            callback.error_message = "payment order not found"
            callback.processed_at = datetime.now(timezone.utc)
            return {"ok": False, "duplicate": not created, "error": callback.error_message}

        callback.payment_order_id = order.id
        callback.order_no = order.order_no
        callback.gateway_order_id = gateway_order_id or order.gateway_order_id

        if amount_usd is None:
            callback.status = "failed"
            callback.error_message = "callback amount is required"
            callback.processed_at = datetime.now(timezone.utc)
            return {"ok": False, "duplicate": not created, "error": callback.error_message}

        expected = to_money_decimal(order.amount_usd)
        actual = to_money_decimal(amount_usd)
        if actual != expected:
            callback.status = "failed"
            callback.error_message = "callback amount mismatch"
            callback.processed_at = datetime.now(timezone.utc)
            return {"ok": False, "duplicate": not created, "error": callback.error_message}

        try:
            updated_order, credited = cls.credit_order(
                db,
                order=order,
                gateway_order_id=gateway_order_id,
                gateway_response=payload,
                pay_amount=pay_amount,
                pay_currency=pay_currency,
                exchange_rate=exchange_rate,
            )
        except ValueError as exc:
            callback.status = "failed"
            callback.error_message = str(exc)
            callback.processed_at = datetime.now(timezone.utc)
            return {"ok": False, "duplicate": not created, "error": callback.error_message}

        callback.status = "processed"
        callback.error_message = None
        callback.processed_at = datetime.now(timezone.utc)
        return {
            "ok": True,
            "duplicate": not created,
            "credited": credited,
            "order_id": updated_order.id,
            "order_no": updated_order.order_no,
            "status": updated_order.status,
            "wallet_id": updated_order.wallet_id,
        }
