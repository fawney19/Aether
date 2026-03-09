from __future__ import annotations

from typing import Any

from src.models.database import (
    PaymentCallback,
    PaymentOrder,
    RefundRequest,
    Wallet,
    WalletDailyUsageLedger,
    WalletTransaction,
)
from src.services.wallet import WalletDailyUsageSnapshot, WalletService


def safe_gateway_response(raw: dict[str, Any] | None) -> dict[str, Any]:
    if not isinstance(raw, dict):
        return {}
    allowed_keys = {
        "gateway",
        "display_name",
        "gateway_order_id",
        "payment_url",
        "qr_code",
        "expires_at",
        "manual_credit",
    }
    return {key: raw[key] for key in allowed_keys if key in raw}


def serialize_payment_order(
    order: PaymentOrder,
    *,
    sanitize_gateway_response: bool = False,
) -> dict[str, Any]:
    return {
        "id": order.id,
        "order_no": order.order_no,
        "wallet_id": order.wallet_id,
        "user_id": order.user_id,
        "amount_usd": float(order.amount_usd or 0),
        "pay_amount": float(order.pay_amount or 0) if order.pay_amount is not None else None,
        "pay_currency": order.pay_currency,
        "exchange_rate": (
            float(order.exchange_rate or 0) if order.exchange_rate is not None else None
        ),
        "refunded_amount_usd": float(order.refunded_amount_usd or 0),
        "refundable_amount_usd": float(order.refundable_amount_usd or 0),
        "payment_method": order.payment_method,
        "gateway_order_id": order.gateway_order_id,
        "gateway_response": (
            safe_gateway_response(order.gateway_response)
            if sanitize_gateway_response
            else order.gateway_response
        ),
        "status": order.status,
        "created_at": order.created_at,
        "paid_at": order.paid_at,
        "credited_at": order.credited_at,
        "expires_at": order.expires_at,
    }


def serialize_payment_callback(callback: PaymentCallback) -> dict[str, Any]:
    return {
        "id": callback.id,
        "payment_order_id": callback.payment_order_id,
        "payment_method": callback.payment_method,
        "callback_key": callback.callback_key,
        "order_no": callback.order_no,
        "gateway_order_id": callback.gateway_order_id,
        "payload_hash": callback.payload_hash,
        "signature_valid": callback.signature_valid,
        "status": callback.status,
        "payload": callback.payload,
        "error_message": callback.error_message,
        "created_at": callback.created_at,
        "processed_at": callback.processed_at,
    }


def serialize_wallet_payload(wallet: Wallet | None) -> dict[str, Any]:
    if wallet is None:
        return {
            "wallet": None,
            "unlimited": False,
            "limit_mode": "finite",
            "balance": 0.0,
            "recharge_balance": 0.0,
            "gift_balance": 0.0,
            "refundable_balance": 0.0,
            "currency": "USD",
        }

    summary = WalletService.serialize_wallet_summary(wallet)
    return {
        "wallet": summary,
        "unlimited": bool(summary["unlimited"]),
        "limit_mode": summary["limit_mode"],
        "balance": summary["balance"],
        "recharge_balance": summary["recharge_balance"],
        "gift_balance": summary["gift_balance"],
        "refundable_balance": summary["refundable_balance"],
        "currency": summary["currency"],
    }


def serialize_wallet_transaction(tx: WalletTransaction) -> dict[str, Any]:
    return {
        "id": tx.id,
        "category": tx.category,
        "reason_code": tx.reason_code,
        "amount": float(tx.amount or 0),
        "balance_before": float(tx.balance_before or 0),
        "balance_after": float(tx.balance_after or 0),
        "recharge_balance_before": float(tx.recharge_balance_before),
        "recharge_balance_after": float(tx.recharge_balance_after),
        "gift_balance_before": float(tx.gift_balance_before),
        "gift_balance_after": float(tx.gift_balance_after),
        "link_type": tx.link_type,
        "link_id": tx.link_id,
        "operator_id": tx.operator_id,
        "description": tx.description,
        "created_at": tx.created_at,
    }


def serialize_wallet_daily_usage(
    ledger: WalletDailyUsageLedger | WalletDailyUsageSnapshot,
) -> dict[str, Any]:
    billing_date = getattr(ledger, "billing_date", None)
    return {
        "id": getattr(ledger, "id", None),
        "date": billing_date.isoformat() if billing_date is not None else None,
        "timezone": getattr(ledger, "billing_timezone", None),
        "total_cost": float(getattr(ledger, "total_cost_usd", 0) or 0),
        "total_requests": int(getattr(ledger, "total_requests", 0) or 0),
        "input_tokens": int(getattr(ledger, "input_tokens", 0) or 0),
        "output_tokens": int(getattr(ledger, "output_tokens", 0) or 0),
        "cache_creation_tokens": int(getattr(ledger, "cache_creation_tokens", 0) or 0),
        "cache_read_tokens": int(getattr(ledger, "cache_read_tokens", 0) or 0),
        "first_finalized_at": getattr(ledger, "first_finalized_at", None),
        "last_finalized_at": getattr(ledger, "last_finalized_at", None),
        "aggregated_at": getattr(ledger, "aggregated_at", None),
        "is_today": bool(getattr(ledger, "is_today", False)),
    }


def serialize_wallet_refund(refund: RefundRequest) -> dict[str, Any]:
    return {
        "id": refund.id,
        "refund_no": refund.refund_no,
        "payment_order_id": refund.payment_order_id,
        "source_type": refund.source_type,
        "source_id": refund.source_id,
        "refund_mode": refund.refund_mode,
        "amount_usd": float(refund.amount_usd or 0),
        "status": refund.status,
        "reason": refund.reason,
        "failure_reason": refund.failure_reason,
        "gateway_refund_id": refund.gateway_refund_id,
        "payout_method": refund.payout_method,
        "payout_reference": refund.payout_reference,
        "payout_proof": refund.payout_proof,
        "created_at": refund.created_at,
        "updated_at": refund.updated_at,
        "processed_at": refund.processed_at,
        "completed_at": refund.completed_at,
    }


def _wallet_owner(wallet: Wallet | None) -> tuple[str, str | None]:
    if wallet is None:
        return "unknown", None
    owner_name: str | None = None
    if wallet.user_id:
        owner_name = wallet.user.username if wallet.user else None
        return "user", owner_name
    if wallet.api_key_id:
        if wallet.api_key:
            owner_name = wallet.api_key.name or f"Key-{wallet.api_key.id[:8]}"
        else:
            owner_name = f"Key-{wallet.api_key_id[:8]}"
        return "api_key", owner_name
    return "orphaned", None


def serialize_admin_wallet(wallet: Wallet) -> dict[str, Any]:
    owner_type, owner_name = _wallet_owner(wallet)
    summary = WalletService.serialize_wallet_summary(wallet)
    return {
        "id": wallet.id,
        "user_id": wallet.user_id,
        "api_key_id": wallet.api_key_id,
        "owner_type": owner_type,
        "owner_name": owner_name,
        "balance": summary["balance"],
        "recharge_balance": summary["recharge_balance"],
        "gift_balance": summary["gift_balance"],
        "refundable_balance": summary["refundable_balance"],
        "currency": summary["currency"],
        "status": summary["status"],
        "limit_mode": summary["limit_mode"],
        "unlimited": summary["unlimited"],
        "total_recharged": summary["total_recharged"],
        "total_consumed": summary["total_consumed"],
        "total_refunded": summary["total_refunded"],
        "total_adjusted": summary["total_adjusted"],
        "created_at": wallet.created_at,
        "updated_at": summary["updated_at"],
    }


def serialize_admin_wallet_transaction(tx: WalletTransaction) -> dict[str, Any]:
    owner_type, owner_name = _wallet_owner(tx.wallet)
    wallet_status = tx.wallet.status if tx.wallet is not None else None
    return {
        "id": tx.id,
        "wallet_id": tx.wallet_id,
        "owner_type": owner_type,
        "owner_name": owner_name,
        "wallet_status": wallet_status,
        "category": tx.category,
        "reason_code": tx.reason_code,
        "amount": float(tx.amount or 0),
        "balance_before": float(tx.balance_before or 0),
        "balance_after": float(tx.balance_after or 0),
        "recharge_balance_before": float(tx.recharge_balance_before),
        "recharge_balance_after": float(tx.recharge_balance_after),
        "gift_balance_before": float(tx.gift_balance_before),
        "gift_balance_after": float(tx.gift_balance_after),
        "link_type": tx.link_type,
        "link_id": tx.link_id,
        "operator_id": tx.operator_id,
        "operator_name": tx.operator.username if tx.operator else None,
        "operator_email": tx.operator.email if tx.operator else None,
        "description": tx.description,
        "created_at": tx.created_at,
    }


def serialize_admin_wallet_refund(refund: RefundRequest) -> dict[str, Any]:
    owner_type, owner_name = _wallet_owner(refund.wallet)
    wallet_status = refund.wallet.status if refund.wallet is not None else None
    return {
        "id": refund.id,
        "refund_no": refund.refund_no,
        "wallet_id": refund.wallet_id,
        "owner_type": owner_type,
        "owner_name": owner_name,
        "wallet_status": wallet_status,
        "user_id": refund.user_id,
        "payment_order_id": refund.payment_order_id,
        "source_type": refund.source_type,
        "source_id": refund.source_id,
        "refund_mode": refund.refund_mode,
        "amount_usd": float(refund.amount_usd or 0),
        "status": refund.status,
        "reason": refund.reason,
        "failure_reason": refund.failure_reason,
        "gateway_refund_id": refund.gateway_refund_id,
        "payout_method": refund.payout_method,
        "payout_reference": refund.payout_reference,
        "payout_proof": refund.payout_proof,
        "requested_by": refund.requested_by,
        "approved_by": refund.approved_by,
        "processed_by": refund.processed_by,
        "created_at": refund.created_at,
        "updated_at": refund.updated_at,
        "processed_at": refund.processed_at,
        "completed_at": refund.completed_at,
    }
