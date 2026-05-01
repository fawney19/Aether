from __future__ import annotations

from contextlib import contextmanager
from datetime import datetime, timezone
from decimal import Decimal
from types import SimpleNamespace
from unittest.mock import MagicMock

import pytest
from fastapi import FastAPI
from fastapi.testclient import TestClient

from src.api.wallet.routes import router as wallet_router
from src.database import get_db
from src.models.database import PaymentOrder


def _build_wallet_app(
    db: MagicMock,
    monkeypatch: pytest.MonkeyPatch,
    *,
    payload: dict[str, object],
    user_id: str = "user-1",
) -> TestClient:
    app = FastAPI()
    app.include_router(wallet_router)
    app.dependency_overrides[get_db] = lambda: db

    @contextmanager
    def _fake_get_db_context() -> MagicMock:
        yield db

    async def _fake_pipeline_run(*, adapter: object, http_request: object, db: MagicMock, mode: object) -> object:
        _ = http_request, mode
        context = SimpleNamespace(
            db=db,
            user=SimpleNamespace(id=user_id),
            ensure_json_body=lambda: payload,
            add_audit_metadata=lambda **_: None,
        )
        return await adapter.handle(context)

    monkeypatch.setattr("src.api.wallet.routes.get_db_context", _fake_get_db_context)
    monkeypatch.setattr("src.api.wallet.routes.pipeline.run", _fake_pipeline_run)
    return TestClient(app)


def test_create_refund_route_maps_uncredited_order_to_400(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    db = MagicMock()
    payload = {"amount_usd": 2.0, "payment_order_id": "order-1"}
    client = _build_wallet_app(db, monkeypatch, payload=payload)
    wallet = SimpleNamespace(id="wallet-1")
    payment_order = SimpleNamespace(id="order-1", wallet_id="wallet-1", payment_method="alipay")

    monkeypatch.setattr(
        "src.api.wallet.routes.WalletService.get_or_create_wallet",
        lambda _db, user: wallet,
    )
    db.query.return_value.filter.return_value.first.return_value = payment_order

    def _raise(*args: object, **kwargs: object) -> object:
        raise ValueError("payment order is not refundable")

    monkeypatch.setattr("src.api.wallet.routes.WalletService.create_refund_request", _raise)

    response = client.post("/api/wallet/refunds", json=payload)

    assert response.status_code == 400
    assert "not refundable" in response.json()["detail"]
    db.rollback.assert_called_once()
    db.commit.assert_not_called()


def test_create_refund_route_maps_reserved_wallet_amount_to_400(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    db = MagicMock()
    payload = {"amount_usd": 2.0}
    client = _build_wallet_app(db, monkeypatch, payload=payload)
    wallet = SimpleNamespace(id="wallet-1")

    monkeypatch.setattr(
        "src.api.wallet.routes.WalletService.get_or_create_wallet",
        lambda _db, user: wallet,
    )

    def _raise(*args: object, **kwargs: object) -> object:
        raise ValueError("refund amount exceeds available refundable recharge balance")

    monkeypatch.setattr("src.api.wallet.routes.WalletService.create_refund_request", _raise)

    response = client.post("/api/wallet/refunds", json=payload)

    assert response.status_code == 400
    assert "available refundable recharge balance" in response.json()["detail"]
    db.rollback.assert_called_once()
    db.commit.assert_not_called()


def test_create_refund_route_passes_default_order_refund_mode_and_commits(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    db = MagicMock()
    payload = {"amount_usd": 2.0, "payment_order_id": "order-1", "reason": "test"}
    client = _build_wallet_app(db, monkeypatch, payload=payload)
    wallet = SimpleNamespace(id="wallet-1")
    payment_order = SimpleNamespace(id="order-1", wallet_id="wallet-1", payment_method="alipay")
    refund = SimpleNamespace(
        id="refund-1",
        refund_no="rf-1",
        payment_order_id="order-1",
        source_type="payment_order",
        source_id="order-1",
        refund_mode="original_channel",
        amount_usd=Decimal("2.00000000"),
        status="pending_approval",
        reason="test",
        failure_reason=None,
        gateway_refund_id=None,
        payout_method=None,
        payout_reference=None,
        payout_proof=None,
        created_at="2026-03-07T00:00:00Z",
        updated_at="2026-03-07T00:00:00Z",
        processed_at=None,
        completed_at=None,
    )

    monkeypatch.setattr(
        "src.api.wallet.routes.WalletService.get_or_create_wallet",
        lambda _db, user: wallet,
    )
    db.query.return_value.filter.return_value.first.return_value = payment_order
    captured: dict[str, object] = {}

    def _create_refund_request(_db: MagicMock, **kwargs: object) -> object:
        captured.update(kwargs)
        return refund

    monkeypatch.setattr(
        "src.api.wallet.routes.WalletService.create_refund_request",
        _create_refund_request,
    )

    response = client.post("/api/wallet/refunds", json=payload)

    assert response.status_code == 200
    body = response.json()
    assert body["id"] == "refund-1"
    assert body["status"] == "pending_approval"
    assert captured["refund_mode"] == "original_channel"
    assert captured["source_type"] == "payment_order"
    assert captured["source_id"] == "order-1"
    assert captured["payment_order"] is payment_order
    db.commit.assert_called_once()
    db.refresh.assert_called_once_with(refund)
    db.rollback.assert_not_called()


def test_create_refund_route_uses_offline_payout_for_manual_recharge(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    db = MagicMock()
    payload = {"amount_usd": 2.0, "payment_order_id": "order-2"}
    client = _build_wallet_app(db, monkeypatch, payload=payload)
    wallet = SimpleNamespace(id="wallet-1")
    payment_order = SimpleNamespace(id="order-2", wallet_id="wallet-1", payment_method="manual_review")
    refund = SimpleNamespace(
        id="refund-2",
        refund_no="rf-2",
        payment_order_id="order-2",
        source_type="payment_order",
        source_id="order-2",
        refund_mode="offline_payout",
        amount_usd=Decimal("2.00000000"),
        status="pending_approval",
        reason=None,
        failure_reason=None,
        gateway_refund_id=None,
        payout_method=None,
        payout_reference=None,
        payout_proof=None,
        created_at="2026-03-07T00:00:00Z",
        updated_at="2026-03-07T00:00:00Z",
        processed_at=None,
        completed_at=None,
    )

    monkeypatch.setattr(
        "src.api.wallet.routes.WalletService.get_or_create_wallet",
        lambda _db, user: wallet,
    )
    db.query.return_value.filter.return_value.first.return_value = payment_order
    captured: dict[str, object] = {}

    def _create_refund_request(_db: MagicMock, **kwargs: object) -> object:
        captured.update(kwargs)
        return refund

    monkeypatch.setattr(
        "src.api.wallet.routes.WalletService.create_refund_request",
        _create_refund_request,
    )

    response = client.post("/api/wallet/refunds", json=payload)

    assert response.status_code == 200
    assert captured["refund_mode"] == "offline_payout"


def test_create_recharge_route_supports_manual_review(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    db = MagicMock()
    payload = {"amount_usd": 20.0, "payment_method": "manual_review"}
    client = _build_wallet_app(db, monkeypatch, payload=payload)
    user = SimpleNamespace(id="user-1")
    order = PaymentOrder(
        id="order-manual-1",
        order_no="po-order-manual-1",
        wallet_id="wallet-1",
        user_id="user-1",
        amount_usd=Decimal("20.00000000"),
        refunded_amount_usd=Decimal("0"),
        refundable_amount_usd=Decimal("0"),
        payment_method="manual_review",
        order_type="topup",
        gateway_response={"gateway": "manual_review", "instructions": "请联系管理员审核充值"},
        status="pending_approval",
        created_at=datetime.now(timezone.utc),
        paid_at=None,
        credited_at=None,
        expires_at=None,
    )
    captured: dict[str, object] = {}

    @contextmanager
    def _fake_get_db_context() -> MagicMock:
        yield db

    def _create_recharge_order(_db: MagicMock, **kwargs: object) -> PaymentOrder:
        captured.update(kwargs)
        return order

    db.query.return_value.filter.return_value.first.return_value = user
    monkeypatch.setattr("src.api.wallet.routes.get_db_context", _fake_get_db_context)
    monkeypatch.setattr("src.api.wallet.routes.PaymentService.create_recharge_order", _create_recharge_order)

    response = client.post("/api/wallet/recharge", json=payload)

    assert response.status_code == 200
    body = response.json()
    assert body["order"]["status"] == "pending_approval"
    assert body["order"]["payment_method"] == "manual_review"
    assert body["payment_instructions"]["gateway"] == "manual_review"
    assert "instructions" in body["payment_instructions"]
    assert captured["payment_method"] == "manual_review"
    db.commit.assert_called_once()
    db.refresh.assert_called_once_with(order)


def test_cancel_recharge_route_expires_pending_approval_order(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    db = MagicMock()
    client = _build_wallet_app(db, monkeypatch, payload={})
    now = datetime.now(timezone.utc)
    order = PaymentOrder(
        id="order-manual-2",
        order_no="po-order-manual-2",
        wallet_id="wallet-1",
        user_id="user-1",
        amount_usd=Decimal("20.00000000"),
        refunded_amount_usd=Decimal("0"),
        refundable_amount_usd=Decimal("0"),
        payment_method="manual_review",
        order_type="topup",
        gateway_response={"gateway": "manual_review"},
        status="pending_approval",
        created_at=now,
        paid_at=None,
        credited_at=None,
        expires_at=None,
    )
    captured: dict[str, object] = {}

    monkeypatch.setattr(
        "src.api.wallet.routes.PaymentService.get_user_order",
        lambda _db, *, user_id, order_id: order,
    )

    def _expire_order(_db: MagicMock, *, order: PaymentOrder, reason: str | None = None) -> tuple[PaymentOrder, bool]:
        captured["order"] = order
        captured["reason"] = reason
        order.status = "expired"
        order.gateway_response = {"expire_reason": reason}
        return order, True

    monkeypatch.setattr("src.api.wallet.routes.PaymentService.expire_order", _expire_order)

    response = client.post("/api/wallet/recharge/order-manual-2/cancel")

    assert response.status_code == 200
    body = response.json()
    assert body["order"]["status"] == "expired"
    assert captured["order"] is order
    assert captured["reason"] == "user_cancelled"
    db.commit.assert_called_once()
    db.refresh.assert_called_once_with(order)
