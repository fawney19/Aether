from __future__ import annotations

import hashlib
import json
from datetime import datetime, timedelta, timezone
from decimal import Decimal
from typing import Any
from uuid import uuid4

from sqlalchemy.orm import Session

from src.config import config
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

    _STATUS_CHECK_BACKOFF_MINUTES = (1, 2, 5, 10, 20, 30)

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
    def _clear_status_check_schedule(order: PaymentOrder) -> None:
        order.next_status_check_at = None
        order.last_status_check_error = None

    @staticmethod
    def _get_locked_order(db: Session, *, order_id: str) -> PaymentOrder | None:
        return (
            db.query(PaymentOrder)
            .filter(PaymentOrder.id == order_id)
            .with_for_update()
            .one_or_none()
        )

    @staticmethod
    def _get_locked_wallet(db: Session, *, wallet_id: str) -> Wallet | None:
        return (
            db.query(Wallet)
            .filter(Wallet.id == wallet_id)
            .with_for_update()
            .one_or_none()
        )

    @staticmethod
    def _merge_gateway_response(
        existing: dict[str, Any] | None,
        updates: dict[str, Any] | None,
    ) -> dict[str, Any]:
        payload = dict(existing or {})
        if updates:
            payload.update(updates)
        return payload

    @classmethod
    def calculate_next_status_check_at(
        cls,
        *,
        order: PaymentOrder,
        now: datetime | None = None,
        attempts: int | None = None,
    ) -> datetime | None:
        if order.status != "pending":
            return None

        now = now or datetime.now(timezone.utc)
        attempt_index = max(0, attempts if attempts is not None else int(order.status_check_attempts or 0))
        if attempt_index < len(cls._STATUS_CHECK_BACKOFF_MINUTES):
            next_run = now + timedelta(minutes=cls._STATUS_CHECK_BACKOFF_MINUTES[attempt_index])
        else:
            next_run = now + timedelta(hours=1)

        max_age_hours = max(1, config.payment_status_check_max_age_hours)
        created_ref = order.created_at or now
        expiry_ref = order.expires_at or created_ref
        stop_at = min(
            created_ref + timedelta(hours=max_age_hours),
            expiry_ref + timedelta(hours=max_age_hours),
        )
        if next_run > stop_at:
            return None
        return next_run

    @classmethod
    def record_status_check_result(
        cls,
        *,
        order: PaymentOrder,
        checked_at: datetime,
        result: str,
        error: str | None = None,
        next_check_at: datetime | None,
    ) -> None:
        order.status_check_attempts = int(order.status_check_attempts or 0) + 1
        order.last_status_check_at = checked_at
        order.last_status_check_result = result
        order.last_status_check_error = error
        order.next_status_check_at = next_check_at

    @classmethod
    def _build_query_gateway_response(
        cls,
        *,
        order: PaymentOrder,
        query_response: dict[str, Any],
        checked_at: datetime,
    ) -> dict[str, Any]:
        return cls._merge_gateway_response(
            order.gateway_response if isinstance(order.gateway_response, dict) else None,
            {
                "gateway_trade_no": query_response.get("trade_no"),
                "gateway_trade_status": query_response.get("trade_status"),
                "last_gateway_query_at": checked_at.isoformat(),
                "last_gateway_query_response": query_response,
            },
        )

    @classmethod
    def _build_status_snapshot_gateway_response(
        cls,
        *,
        order: PaymentOrder,
        checked_at: datetime,
        channel_status: str | None,
        gateway_order_id: str | None,
        query_response: dict[str, Any] | None,
        error: str | None = None,
    ) -> dict[str, Any]:
        payload = {
            "gateway_trade_no": gateway_order_id,
            "gateway_trade_status": channel_status,
            "last_gateway_query_at": checked_at.isoformat(),
            "last_gateway_query_response": query_response or {},
        }
        if error:
            payload["last_gateway_query_error"] = error
        return cls._merge_gateway_response(
            order.gateway_response if isinstance(order.gateway_response, dict) else None,
            payload,
        )

    @classmethod
    def get_expected_pay_amount(cls, *, order: PaymentOrder) -> Decimal:
        pay_amount = getattr(order, "pay_amount", None)
        expected = pay_amount if pay_amount is not None else order.amount_usd
        return to_money_decimal(expected)

    @classmethod
    def count_active_pending_orders(
        cls,
        db: Session,
        *,
        user_id: str,
        refresh_expired: bool = True,
    ) -> int:
        if refresh_expired:
            cls.expire_overdue_pending_orders(db, user_id=user_id)
        return int(
            db.query(PaymentOrder)
            .filter(
                PaymentOrder.user_id == user_id,
                PaymentOrder.status == "pending",
            )
            .count()
            or 0
        )

    @classmethod
    def create_recharge_order(
        cls,
        db: Session,
        *,
        user: User,
        amount_usd: Decimal | float | int | str,
        bonus_amount_usd: Decimal | float | int | str = 0,
        payment_method: str,
        pay_amount: Decimal | float | int | str | None = None,
        pay_currency: str | None = None,
        exchange_rate: Decimal | float | int | str | None = None,
        expires_in_minutes: int = 15,
        gateway_order_id: str | None = None,
        gateway_response: dict[str, Any] | None = None,
        max_pending_orders: int | None = None,
    ) -> PaymentOrder:
        amount = to_money_decimal(amount_usd)
        bonus_amount = to_money_decimal(bonus_amount_usd)
        if amount <= Decimal("0"):
            raise ValueError("recharge amount must be positive")
        if bonus_amount < Decimal("0"):
            raise ValueError("bonus amount must not be negative")
        if not payment_method:
            raise ValueError("payment_method is required")
        if payment_method == "admin_manual":
            raise ValueError("admin_manual is reserved for admin recharge")
        gateway = get_payment_gateway(payment_method)

        wallet = WalletService.get_or_create_wallet(db, user=user)
        if wallet is None:
            raise ValueError("wallet not available")

        if max_pending_orders is not None and max_pending_orders > 0:
            locked_wallet = cls._get_locked_wallet(db, wallet_id=wallet.id)
            if locked_wallet is None:
                raise ValueError("wallet not found")
            if locked_wallet.status != "active":
                raise ValueError("wallet is not active")
            wallet = locked_wallet
            pending_count = cls.count_active_pending_orders(db, user_id=user.id, refresh_expired=True)
            if pending_count >= max_pending_orders:
                raise ValueError(
                    f"当前待支付订单已达上限（{max_pending_orders}笔），请先完成支付或等待旧订单过期后再试"
                )
        elif wallet.status != "active":
            raise ValueError("wallet is not active")

        now = datetime.now(timezone.utc)
        order = PaymentOrder(
            order_no=cls._build_order_no(),
            wallet_id=wallet.id,
            user_id=user.id,
            amount_usd=amount,
            bonus_amount_usd=bonus_amount,
            pay_amount=to_money_decimal(pay_amount) if pay_amount is not None else None,
            pay_currency=pay_currency,
            exchange_rate=to_money_decimal(exchange_rate) if exchange_rate is not None else None,
            refunded_amount_usd=Decimal("0"),
            refundable_amount_usd=Decimal("0"),
            payment_method=payment_method,
            gateway_order_id=gateway_order_id,
            gateway_response=gateway_response,
            status="pending",
            expires_at=now + timedelta(minutes=max(expires_in_minutes, 1)),
        )
        if gateway.capabilities.supports_active_query:
            order.next_status_check_at = cls.calculate_next_status_check_at(order=order, now=now, attempts=0)
            order.last_status_check_result = "scheduled"
        db.add(order)
        db.flush()
        checkout = gateway.create_checkout_payload(order=order)
        order.gateway_order_id = order.gateway_order_id or checkout.get("gateway_order_id")
        
        merged_response = dict(gateway_response or {})
        merged_response.update(checkout)
        order.gateway_response = merged_response
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
            cls._clear_status_check_schedule(order)
            order.last_status_check_result = "local_expired"
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
    ) -> tuple[list[PaymentOrder], int, bool]:
        expired_count = cls.expire_overdue_pending_orders(db, user_id=user_id)
        q = db.query(PaymentOrder).filter(PaymentOrder.user_id == user_id)
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
        limit: int = 50,
        offset: int = 0,
    ) -> tuple[list[PaymentOrder], int, bool]:
        expired_count = 0
        if status in {None, "pending", "expired"}:
            expired_count = cls.expire_overdue_pending_orders(
                db,
                payment_method=payment_method,
            )

        q = db.query(PaymentOrder)
        if status:
            q = q.filter(PaymentOrder.status == status)
        if payment_method:
            q = q.filter(PaymentOrder.payment_method == payment_method)
        total = q.count()
        items = q.order_by(PaymentOrder.created_at.desc()).offset(offset).limit(limit).all()
        return items, total, expired_count > 0

    @staticmethod
    def expire_overdue_pending_orders(
        db: Session,
        *,
        user_id: str | None = None,
        payment_method: str | None = None,
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
        return int(q.update({PaymentOrder.status: "expired"}, synchronize_session=False) or 0)

    @staticmethod
    def list_callbacks(
        db: Session,
        *,
        payment_method: str | None = None,
        limit: int = 50,
        offset: int = 0,
    ) -> tuple[list[PaymentCallback], int]:
        q = db.query(PaymentCallback)
        if payment_method:
            q = q.filter(PaymentCallback.payment_method == payment_method)
        total = q.count()
        items = q.order_by(PaymentCallback.created_at.desc()).offset(offset).limit(limit).all()
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
        locked_order = cls._get_locked_order(db, order_id=order.id)
        if locked_order is None:
            raise ValueError("payment order not found")
        if locked_order.status == "credited":
            raise ValueError("credited order cannot be failed")
        locked_order.status = "failed"
        cls._clear_status_check_schedule(locked_order)
        payload = dict(locked_order.gateway_response or {})
        if reason:
            payload["failure_reason"] = reason
        payload["failed_at"] = datetime.now(timezone.utc).isoformat()
        locked_order.gateway_response = payload
        return locked_order

    @classmethod
    def mark_order_manual_review(
        cls,
        db: Session,
        *,
        order: PaymentOrder,
        reason: str | None = None,
        gateway_response: dict[str, Any] | None = None,
    ) -> tuple[PaymentOrder, bool]:
        locked_order = cls._get_locked_order(db, order_id=order.id)
        if locked_order is None:
            raise ValueError("payment order not found")
        if locked_order.status in {"credited", "refunded"}:
            raise ValueError(f"payment order cannot enter manual review: {locked_order.status}")
        if locked_order.status == "manual_review":
            if gateway_response is not None:
                locked_order.gateway_response = cls._merge_gateway_response(
                    locked_order.gateway_response
                    if isinstance(locked_order.gateway_response, dict)
                    else None,
                    gateway_response,
                )
            return locked_order, False

        locked_order.status = "manual_review"
        cls._clear_status_check_schedule(locked_order)
        payload = dict(locked_order.gateway_response or {})
        if gateway_response is not None:
            payload = cls._merge_gateway_response(payload, gateway_response)
        if reason:
            payload["manual_review_reason"] = reason
        payload["manual_review_marked_at"] = datetime.now(timezone.utc).isoformat()
        locked_order.gateway_response = payload
        return locked_order, True

    @classmethod
    def expire_order(
        cls,
        db: Session,
        *,
        order: PaymentOrder,
        reason: str | None = None,
    ) -> tuple[PaymentOrder, bool]:
        locked_order = cls._get_locked_order(db, order_id=order.id)
        if locked_order is None:
            raise ValueError("payment order not found")
        if locked_order.status == "credited":
            raise ValueError("credited order cannot be expired")
        if locked_order.status == "expired":
            return locked_order, False
        if locked_order.status != "pending":
            raise ValueError(f"only pending order can be expired: {locked_order.status}")

        locked_order.status = "expired"
        cls._clear_status_check_schedule(locked_order)
        payload = dict(locked_order.gateway_response or {})
        if reason:
            payload["expire_reason"] = reason
        payload["expired_at"] = datetime.now(timezone.utc).isoformat()
        locked_order.gateway_response = payload
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
    ) -> tuple[PaymentOrder, bool]:
        locked_order = cls._get_locked_order(db, order_id=order.id)
        if locked_order is None:
            raise ValueError("payment order not found")

        if locked_order.status == "credited":
            return locked_order, False
        if locked_order.status in {"failed", "refunded"}:
            raise ValueError(f"payment order is not creditable: {locked_order.status}")
        now = datetime.now(timezone.utc)

        wallet = db.query(Wallet).filter(Wallet.id == locked_order.wallet_id).first()
        if wallet is None:
            raise ValueError("wallet not found")
        if wallet.status != "active":
            raise ValueError("wallet is not active")

        if gateway_order_id:
            locked_order.gateway_order_id = gateway_order_id
        if gateway_response is not None:
            locked_order.gateway_response = cls._merge_gateway_response(
                locked_order.gateway_response if isinstance(locked_order.gateway_response, dict) else None,
                gateway_response,
            )
        if pay_amount is not None:
            locked_order.pay_amount = to_money_decimal(pay_amount)
        if pay_currency is not None:
            locked_order.pay_currency = pay_currency
        if exchange_rate is not None:
            locked_order.exchange_rate = to_money_decimal(exchange_rate)

        locked_order.status = "paid"
        locked_order.paid_at = locked_order.paid_at or now
        locked_order.refundable_amount_usd = to_money_decimal(locked_order.amount_usd)
        cls._clear_status_check_schedule(locked_order)

        WalletService.create_wallet_transaction(
            db,
            wallet=wallet,
            category="recharge",
            reason_code="topup_gateway",
            amount=locked_order.amount_usd,
            balance_type="recharge",
            link_type="payment_order",
            link_id=locked_order.id,
            description=f"充值到账({locked_order.payment_method})",
        )
        bonus_amount = to_money_decimal(getattr(locked_order, "bonus_amount_usd", None))
        if bonus_amount > Decimal("0"):
            WalletService.create_wallet_transaction(
                db,
                wallet=wallet,
                category="gift",
                reason_code="gift_recharge_bonus",
                amount=bonus_amount,
                balance_type="gift",
                link_type="payment_order",
                link_id=locked_order.id,
                description=f"充值赠送({locked_order.payment_method})",
            )

        locked_order.status = "credited"
        locked_order.credited_at = now
        return locked_order, True

    @classmethod
    def sync_order_status(
        cls,
        db: Session,
        *,
        order: PaymentOrder,
        query_response: dict[str, Any],
        checked_at: datetime | None = None,
    ) -> dict[str, Any]:
        checked_at = checked_at or datetime.now(timezone.utc)
        locked_order = cls._get_locked_order(db, order_id=order.id)
        if locked_order is None:
            raise ValueError("payment order not found")
        gateway = get_payment_gateway(locked_order.payment_method)
        if not gateway.capabilities.supports_active_query:
            raise ValueError(
                f"payment method does not support active status query: {locked_order.payment_method}"
            )

        if locked_order.status == "credited":
            cls._clear_status_check_schedule(locked_order)
            return {"ok": True, "noop": True, "status": locked_order.status}
        if locked_order.status in {"failed", "refunded", "manual_review"}:
            cls._clear_status_check_schedule(locked_order)
            return {"ok": True, "noop": True, "status": locked_order.status}

        snapshot = gateway.normalize_query_response(order=locked_order, query_response=query_response)
        trade_status = snapshot.channel_status

        if snapshot.error:
            next_check_at = cls.calculate_next_status_check_at(
                order=locked_order,
                now=checked_at,
                attempts=int(locked_order.status_check_attempts or 0) + 1,
            )
            locked_order.gateway_response = cls._build_status_snapshot_gateway_response(
                order=locked_order,
                checked_at=checked_at,
                channel_status=snapshot.channel_status,
                gateway_order_id=snapshot.gateway_order_id,
                query_response=snapshot.raw,
                error=snapshot.error,
            )
            cls.record_status_check_result(
                order=locked_order,
                checked_at=checked_at,
                result=trade_status or "query_error",
                error=snapshot.error,
                next_check_at=next_check_at,
            )
            return {
                "ok": False,
                "status": locked_order.status,
                "trade_status": trade_status,
                "error": snapshot.error,
                "next_check_at": next_check_at,
            }

        if snapshot.order_status in {"paid", "credited"}:
            if snapshot.pay_amount is None:
                next_check_at = cls.calculate_next_status_check_at(
                    order=locked_order,
                    now=checked_at,
                    attempts=int(locked_order.status_check_attempts or 0) + 1,
                )
                locked_order.gateway_response = cls._build_status_snapshot_gateway_response(
                    order=locked_order,
                    checked_at=checked_at,
                    channel_status=snapshot.channel_status,
                    gateway_order_id=snapshot.gateway_order_id,
                    query_response=snapshot.raw,
                    error="missing pay_amount in payment query response",
                )
                cls.record_status_check_result(
                    order=locked_order,
                    checked_at=checked_at,
                    result=trade_status or "missing_pay_amount",
                    error="missing pay_amount in payment query response",
                    next_check_at=next_check_at,
                )
                return {
                    "ok": False,
                    "status": locked_order.status,
                    "trade_status": trade_status,
                    "error": "missing pay_amount in payment query response",
                    "next_check_at": next_check_at,
                }

            expected_amount = cls.get_expected_pay_amount(order=locked_order)
            actual_amount = to_money_decimal(snapshot.pay_amount)
            if actual_amount != expected_amount:
                next_check_at = cls.calculate_next_status_check_at(
                    order=locked_order,
                    now=checked_at,
                    attempts=int(locked_order.status_check_attempts or 0) + 1,
                )
                locked_order.gateway_response = cls._build_status_snapshot_gateway_response(
                    order=locked_order,
                    checked_at=checked_at,
                    channel_status=snapshot.channel_status,
                    gateway_order_id=snapshot.gateway_order_id,
                    query_response=snapshot.raw,
                    error="payment query amount mismatch",
                )
                cls.record_status_check_result(
                    order=locked_order,
                    checked_at=checked_at,
                    result=trade_status or "amount_mismatch",
                    error="payment query amount mismatch",
                    next_check_at=next_check_at,
                )
                return {
                    "ok": False,
                    "status": locked_order.status,
                    "trade_status": trade_status,
                    "error": "payment query amount mismatch",
                    "next_check_at": next_check_at,
                }

            updated_order, credited = cls.credit_order(
                db,
                order=locked_order,
                gateway_order_id=snapshot.gateway_order_id,
                gateway_response=cls._build_status_snapshot_gateway_response(
                    order=locked_order,
                    checked_at=checked_at,
                    channel_status=snapshot.channel_status,
                    gateway_order_id=snapshot.gateway_order_id,
                    query_response=snapshot.raw,
                ),
                pay_amount=actual_amount,
                pay_currency=snapshot.pay_currency,
                exchange_rate=snapshot.exchange_rate,
            )
            cls.record_status_check_result(
                order=updated_order,
                checked_at=checked_at,
                result=trade_status or "paid",
                error=None,
                next_check_at=None,
            )
            return {
                "ok": True,
                "credited": credited,
                "status": updated_order.status,
                "trade_status": trade_status,
            }

        if snapshot.order_status == "expired":
            locked_order.gateway_response = cls._build_status_snapshot_gateway_response(
                order=locked_order,
                checked_at=checked_at,
                channel_status=snapshot.channel_status,
                gateway_order_id=snapshot.gateway_order_id,
                query_response=snapshot.raw,
            )
            locked_order.status = "expired"
            cls.record_status_check_result(
                order=locked_order,
                checked_at=checked_at,
                result=trade_status or "expired",
                error=None,
                next_check_at=None,
            )
            return {
                "ok": True,
                "credited": False,
                "status": locked_order.status,
                "trade_status": trade_status,
            }

        if snapshot.order_status == "failed":
            updated_order = cls.fail_order(
                db,
                order=locked_order,
                reason=snapshot.error or snapshot.channel_status or "gateway_reported_failed",
            )
            updated_order.gateway_response = cls._build_status_snapshot_gateway_response(
                order=updated_order,
                checked_at=checked_at,
                channel_status=snapshot.channel_status,
                gateway_order_id=snapshot.gateway_order_id,
                query_response=snapshot.raw,
                error=snapshot.error,
            )
            cls.record_status_check_result(
                order=updated_order,
                checked_at=checked_at,
                result=trade_status or "failed",
                error=snapshot.error,
                next_check_at=None,
            )
            return {
                "ok": True,
                "credited": False,
                "status": updated_order.status,
                "trade_status": trade_status,
            }

        if locked_order.expires_at is not None and locked_order.expires_at < checked_at:
            locked_order.status = "expired"
            cls.record_status_check_result(
                order=locked_order,
                checked_at=checked_at,
                result=trade_status or "local_expired",
                error=None,
                next_check_at=None,
            )
            return {
                "ok": True,
                "credited": False,
                "status": locked_order.status,
                "trade_status": trade_status or None,
            }

        next_check_at = cls.calculate_next_status_check_at(
            order=locked_order,
            now=checked_at,
            attempts=int(locked_order.status_check_attempts or 0) + 1,
        )
        locked_order.gateway_response = cls._build_status_snapshot_gateway_response(
            order=locked_order,
            checked_at=checked_at,
            channel_status=snapshot.channel_status,
            gateway_order_id=snapshot.gateway_order_id,
            query_response=snapshot.raw,
        )
        cls.record_status_check_result(
            order=locked_order,
            checked_at=checked_at,
            result=trade_status or "unknown",
            error=None,
            next_check_at=next_check_at,
        )
        return {
            "ok": True,
            "credited": False,
            "status": locked_order.status,
            "trade_status": trade_status or None,
            "next_check_at": next_check_at,
        }

    @classmethod
    def sync_alipay_order_status(
        cls,
        db: Session,
        *,
        order: PaymentOrder,
        query_response: dict[str, Any],
        checked_at: datetime | None = None,
    ) -> dict[str, Any]:
        return cls.sync_order_status(
            db,
            order=order,
            query_response=query_response,
            checked_at=checked_at,
        )

    @classmethod
    def query_and_sync_order_status(
        cls,
        db: Session,
        *,
        order: PaymentOrder,
        checked_at: datetime | None = None,
    ) -> dict[str, Any]:
        gateway = get_payment_gateway(order.payment_method)
        if not gateway.capabilities.supports_active_query:
            raise ValueError(
                f"payment method does not support active status query: {order.payment_method}"
            )
        query_response = gateway.query_order_status(
            order_no=order.order_no,
            gateway_order_id=order.gateway_order_id,
        )
        return cls.sync_order_status(
            db,
            order=order,
            query_response=query_response,
            checked_at=checked_at,
        )

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
        parsed = gateway.parse_callback_payload(payload=payload)
        resolved_callback_key = callback_key or parsed.callback_key
        resolved_order_no = order_no or parsed.order_no
        resolved_gateway_order_id = gateway_order_id or parsed.gateway_order_id
        resolved_order_status = parsed.order_status or "paid"
        resolved_pay_amount = (
            pay_amount
            if pay_amount is not None
            else amount_usd
            if amount_usd is not None
            else parsed.pay_amount
        )
        resolved_pay_currency = pay_currency or parsed.pay_currency
        resolved_exchange_rate = exchange_rate if exchange_rate is not None else parsed.exchange_rate

        verified = gateway.verify_callback_payload(
            payload=payload,
            callback_signature=callback_signature,
            callback_secret=callback_secret,
        )
        callback, created = cls.log_callback(
            db,
            payment_method=payment_method,
            callback_key=resolved_callback_key,
            order_no=resolved_order_no,
            gateway_order_id=resolved_gateway_order_id,
            payload=payload,
            signature_valid=verified,
        )
        if not created and callback.status in {"processed", "ignored"}:
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
            order_no=resolved_order_no or callback.order_no,
            gateway_order_id=resolved_gateway_order_id or callback.gateway_order_id,
        )
        if order is None:
            callback.status = "failed"
            callback.error_message = "payment order not found"
            callback.processed_at = datetime.now(timezone.utc)
            return {"ok": False, "duplicate": not created, "error": callback.error_message}

        callback.payment_order_id = order.id
        callback.order_no = order.order_no
        callback.gateway_order_id = resolved_gateway_order_id or order.gateway_order_id

        if resolved_order_status == "expired":
            updated_order, _changed = cls.expire_order(
                db,
                order=order,
                reason="gateway_callback_expired",
            )
            callback.status = "processed"
            callback.error_message = None
            callback.processed_at = datetime.now(timezone.utc)
            return {
                "ok": True,
                "duplicate": not created,
                "credited": False,
                "order_id": updated_order.id,
                "order_no": updated_order.order_no,
                "status": updated_order.status,
                "wallet_id": updated_order.wallet_id,
            }

        if resolved_order_status not in {"paid", "credited"}:
            callback.status = "ignored"
            callback.error_message = None
            callback.processed_at = datetime.now(timezone.utc)
            return {
                "ok": True,
                "duplicate": not created,
                "credited": False,
                "order_id": order.id,
                "order_no": order.order_no,
                "status": order.status,
                "wallet_id": order.wallet_id,
            }

        if resolved_pay_amount is None:
            callback.status = "failed"
            callback.error_message = "callback amount is required"
            callback.processed_at = datetime.now(timezone.utc)
            return {"ok": False, "duplicate": not created, "error": callback.error_message}

        expected = cls.get_expected_pay_amount(order=order)
        actual = to_money_decimal(resolved_pay_amount)
        if actual != expected:
            callback.status = "failed"
            callback.error_message = "callback amount mismatch"
            callback.processed_at = datetime.now(timezone.utc)
            return {"ok": False, "duplicate": not created, "error": callback.error_message}

        if order.status in {"failed", "manual_review"}:
            updated_order, _changed = cls.mark_order_manual_review(
                db,
                order=order,
                reason=(
                    "paid_callback_after_failed_order"
                    if order.status == "failed"
                    else "paid_callback_requires_manual_review"
                ),
                gateway_response=payload,
            )
            callback.status = "processed"
            callback.error_message = None
            callback.processed_at = datetime.now(timezone.utc)
            return {
                "ok": True,
                "duplicate": not created,
                "credited": False,
                "requires_manual_review": True,
                "order_id": updated_order.id,
                "order_no": updated_order.order_no,
                "status": updated_order.status,
                "wallet_id": updated_order.wallet_id,
            }

        try:
            updated_order, credited = cls.credit_order(
                db,
                order=order,
                gateway_order_id=resolved_gateway_order_id,
                gateway_response=payload,
                pay_amount=resolved_pay_amount,
                pay_currency=resolved_pay_currency,
                exchange_rate=resolved_exchange_rate,
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
