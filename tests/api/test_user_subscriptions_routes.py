from __future__ import annotations

import json
from contextlib import contextmanager
from datetime import datetime, timezone
from decimal import Decimal
from types import SimpleNamespace
from typing import Any
from unittest.mock import MagicMock

import pytest
from fastapi import FastAPI
from fastapi.testclient import TestClient

from src.api.subscription.routes import router as user_subscriptions_router
from src.database import get_db
from src.models.database import PaymentOrder


def _build_user_subscriptions_app(
    db: MagicMock,
    monkeypatch: pytest.MonkeyPatch,
) -> TestClient:
    app = FastAPI()
    app.include_router(user_subscriptions_router)
    app.dependency_overrides[get_db] = lambda: db

    async def _fake_pipeline_run(
        *,
        adapter: Any,
        http_request: object,
        db: MagicMock,
        mode: object,
    ) -> Any:
        _ = mode
        request_body = await http_request.body()
        payload = json.loads(request_body) if request_body else {}
        context = SimpleNamespace(
            db=db,
            request=SimpleNamespace(state=SimpleNamespace()),
            user=SimpleNamespace(id="user-1"),
            ensure_json_body=lambda: payload,
            add_audit_metadata=lambda **_: None,
        )
        return await adapter.handle(context)

    monkeypatch.setattr("src.api.subscription.routes.pipeline.run", _fake_pipeline_run)
    return TestClient(app)


def _make_subscription() -> SimpleNamespace:
    now = datetime.now(timezone.utc)
    return SimpleNamespace(
        id="sub-1",
        user_id="user-1",
        user=SimpleNamespace(username="alice", email="alice@example.com"),
        plan_id="plan-pro",
        plan=SimpleNamespace(
            id="plan-pro",
            code="pro",
            name="专业版",
            user_group_id="group-pro",
            user_group=SimpleNamespace(name="高级用户"),
        ),
        status="pending_payment",
        end_reason=None,
        purchased_months=12,
        discount_factor=1.0,
        monthly_price_usd_snapshot=20,
        total_price_usd=180,
        started_at=now,
        ends_at=now,
        current_cycle_start=now,
        current_cycle_end=now,
        cycle_quota_usd=60,
        cycle_used_usd=0,
        cancel_at_period_end=False,
        canceled_at=None,
        ended_at=None,
        upgraded_from_subscription_id=None,
        created_at=now,
        updated_at=now,
    )


def test_build_dashboard_sync_uses_display_end(monkeypatch: pytest.MonkeyPatch) -> None:
    from src.api.subscription.routes import _build_dashboard_sync

    db = MagicMock()
    subscription = _make_subscription()
    display_end = datetime.now(timezone.utc)

    user_query = MagicMock()
    user_query.filter.return_value.first.return_value = SimpleNamespace(id="user-1")
    db.query.return_value = user_query

    @contextmanager
    def _fake_db_context() -> Any:
        yield db

    monkeypatch.setattr("src.api.subscription.routes.get_db_context", _fake_db_context)
    monkeypatch.setattr(
        "src.api.subscription.routes.SubscriptionService.get_user_current_subscription",
        lambda *_a, **_k: subscription,
    )
    monkeypatch.setattr(
        "src.api.subscription.routes.SubscriptionService.get_subscription_display_end",
        lambda *_a, **_k: display_end,
    )

    payload = _build_dashboard_sync("user-1")

    assert payload["current_subscription"]["ends_at"] == display_end.isoformat().replace("+00:00", "Z")


def test_list_subscription_plans_returns_payload(monkeypatch: pytest.MonkeyPatch) -> None:
    db = MagicMock()
    client = _build_user_subscriptions_app(db, monkeypatch)

    monkeypatch.setattr(
        "src.api.subscription.routes._list_active_plans_sync",
        lambda: {
            "plans": [
                {
                    "id": "plan-pro",
                    "code": "pro",
                    "name": "专业版",
                    "description": "高阶套餐",
                    "user_group_id": "group-pro",
                    "user_group_name": "高级用户",
                    "plan_level": 10,
                    "monthly_price_usd": 20,
                    "monthly_quota_usd": 60,
                    "overage_policy": "use_wallet_balance",
                    "term_discounts_json": [{"months": 12, "discount_factor": 0.75}],
                    "is_active": True,
                    "active_subscription_count": 0,
                    "created_at": datetime.now(timezone.utc).isoformat(),
                    "updated_at": datetime.now(timezone.utc).isoformat(),
                }
            ],
            "total": 1,
        },
    )

    response = client.get("/api/subscriptions/plans")

    assert response.status_code == 200
    payload = response.json()
    assert payload["total"] == 1
    assert payload["plans"][0]["code"] == "pro"


def test_purchase_route_forwards_validated_payload(monkeypatch: pytest.MonkeyPatch) -> None:
    db = MagicMock()
    client = _build_user_subscriptions_app(db, monkeypatch)
    captured: dict[str, Any] = {}
    subscription = _make_subscription()

    def _fake_create_purchase_sync(user_id: str, req: Any) -> dict[str, Any]:
        captured["user_id"] = user_id
        captured["request"] = req
        return {
            "subscription": {
                "id": subscription.id,
                "user_id": subscription.user_id,
                "username": subscription.user.username,
                "email": subscription.user.email,
                "plan_id": subscription.plan_id,
                "plan_code": subscription.plan.code,
                "plan_name": subscription.plan.name,
                "user_group_id": subscription.plan.user_group_id,
                "user_group_name": subscription.plan.user_group.name,
                "status": subscription.status,
                "end_reason": subscription.end_reason,
                "purchased_months": subscription.purchased_months,
                "discount_factor": subscription.discount_factor,
                "monthly_price_usd_snapshot": subscription.monthly_price_usd_snapshot,
                "total_price_usd": subscription.total_price_usd,
                "started_at": subscription.started_at.isoformat(),
                "ends_at": subscription.ends_at.isoformat(),
                "current_cycle_start": subscription.current_cycle_start.isoformat(),
                "current_cycle_end": subscription.current_cycle_end.isoformat(),
                "cycle_quota_usd": subscription.cycle_quota_usd,
                "cycle_used_usd": subscription.cycle_used_usd,
                "remaining_quota_usd": subscription.cycle_quota_usd,
                "cancel_at_period_end": subscription.cancel_at_period_end,
                "canceled_at": subscription.canceled_at,
                "ended_at": subscription.ended_at,
                "upgraded_from_subscription_id": subscription.upgraded_from_subscription_id,
                "created_at": subscription.created_at.isoformat(),
                "updated_at": subscription.updated_at.isoformat(),
            },
            "payable_amount_usd": 180,
            "order": {
                "id": "order-1",
                "order_no": "po_1",
                "wallet_id": "wallet-1",
                "user_id": "user-1",
                "subscription_id": subscription.id,
                "amount_usd": 180,
                "pay_amount": None,
                "pay_currency": None,
                "exchange_rate": None,
                "refunded_amount_usd": 0,
                "refundable_amount_usd": 0,
                "payment_method": "alipay",
                "order_type": "subscription_initial",
                "gateway_order_id": "ali_1",
                "gateway_response": {"payment_url": "https://example.com/pay"},
                "status": "pending",
                "created_at": subscription.created_at.isoformat(),
                "paid_at": None,
                "credited_at": None,
                "expires_at": subscription.created_at.isoformat(),
            },
            "payment_instructions": {"payment_url": "https://example.com/pay"},
        }

    monkeypatch.setattr(
        "src.api.subscription.routes._create_purchase_sync",
        _fake_create_purchase_sync,
    )

    response = client.post(
        "/api/subscriptions/purchase",
        json={
            "plan_id": "plan-pro",
            "purchased_months": 12,
            "payment_method": "alipay",
        },
    )

    assert response.status_code == 200
    assert captured["user_id"] == "user-1"
    assert captured["request"].plan_id == "plan-pro"
    assert captured["request"].purchased_months == 12
    assert captured["request"].payment_method == "alipay"
    assert response.json()["order"]["order_type"] == "subscription_initial"
    assert "user_group_name" not in response.json()["subscription"]
    assert "user_group_id" not in response.json()["subscription"]


def test_list_subscription_products_returns_models_without_group_fields(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    db = MagicMock()
    client = _build_user_subscriptions_app(db, monkeypatch)

    monkeypatch.setattr(
        "src.api.subscription.routes._list_active_products_sync",
        lambda: {
            "products": [
                {
                    "id": "product-pro",
                    "code": "pro",
                    "name": "专业版",
                    "description": "高阶套餐",
                    "user_group_id": "group-pro",
                    "user_group_name": "高级用户",
                    "plan_level": 10,
                    "overage_policy": "use_wallet_balance",
                    "is_active": True,
                    "variant_count": 1,
                    "available_model_names": ["Claude Sonnet 4", "GPT-4.1"],
                    "variants": [
                        {
                            "id": "variant-pro",
                            "product_id": "product-pro",
                            "code": "pro-5x",
                            "name": "5x",
                            "description": None,
                            "monthly_price_usd": 20,
                            "monthly_quota_usd": 60,
                            "variant_rank": 10,
                            "term_discounts_json": [{"months": 12, "discount_factor": 0.75}],
                            "is_active": True,
                            "is_default_variant": True,
                            "created_at": datetime.now(timezone.utc).isoformat(),
                            "updated_at": datetime.now(timezone.utc).isoformat(),
                        }
                    ],
                    "created_at": datetime.now(timezone.utc).isoformat(),
                    "updated_at": datetime.now(timezone.utc).isoformat(),
                }
            ],
            "total": 1,
        },
    )

    response = client.get("/api/subscriptions/products")

    assert response.status_code == 200
    payload = response.json()
    assert payload["total"] == 1
    assert payload["products"][0]["available_model_names"] == ["Claude Sonnet 4", "GPT-4.1"]
    assert "user_group_name" not in payload["products"][0]
    assert "user_group_id" not in payload["products"][0]


def test_list_subscription_orders_sync_includes_plan_fields(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from src.api.subscription.routes import _list_subscription_orders_sync

    db = MagicMock()
    now = datetime.now(timezone.utc)
    order = SimpleNamespace(
        id="order-1",
        order_no="po_1",
        wallet_id="wallet-1",
        user_id="user-1",
        subscription_id="sub-1",
        amount_usd=100,
        pay_amount=None,
        pay_currency=None,
        exchange_rate=None,
        refunded_amount_usd=0,
        refundable_amount_usd=0,
        payment_method="manual_review",
        order_type="subscription_renewal",
        gateway_order_id=None,
        gateway_response={},
        status="pending_approval",
        created_at=now,
        paid_at=None,
        credited_at=None,
        expires_at=None,
        subscription=SimpleNamespace(
            status="pending_payment",
            purchased_months=3,
            upgraded_from_subscription_id="sub-old",
            plan=SimpleNamespace(
                id="plan-pro",
                name="专业版 5x",
                product=SimpleNamespace(
                    id="product-pro",
                    name="专业版",
                ),
            ),
        ),
    )

    @contextmanager
    def _fake_db_context() -> Any:
        yield db

    monkeypatch.setattr("src.api.subscription.routes.get_db_context", _fake_db_context)
    monkeypatch.setattr(
        "src.api.subscription.routes.PaymentService.list_user_orders",
        lambda *_a, **_k: ([order], 1, False),
    )

    payload = _list_subscription_orders_sync("user-1", 20, 0)

    assert payload["total"] == 1
    assert payload["items"][0]["product_name"] == "专业版"
    assert payload["items"][0]["plan_name"] == "专业版 5x"
    assert payload["items"][0]["purchased_months"] == 3


def test_cancel_subscription_order_route_expires_pending_approval_order(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    db = MagicMock()
    client = _build_user_subscriptions_app(db, monkeypatch)
    now = datetime.now(timezone.utc)
    order = SimpleNamespace(
        id="order-sub-2",
        order_no="po_sub_2",
        wallet_id="wallet-1",
        user_id="user-1",
        subscription_id="sub-2",
        amount_usd=Decimal("100.00000000"),
        pay_amount=None,
        pay_currency=None,
        exchange_rate=None,
        refunded_amount_usd=Decimal("0"),
        refundable_amount_usd=Decimal("0"),
        payment_method="manual_review",
        order_type="subscription_renewal",
        gateway_order_id=None,
        gateway_response={"gateway": "manual_review"},
        status="pending_approval",
        created_at=now,
        paid_at=None,
        credited_at=None,
        expires_at=None,
        subscription=SimpleNamespace(
            status="pending_payment",
            purchased_months=1,
            upgraded_from_subscription_id=None,
            plan=SimpleNamespace(
                id="plan-pro",
            name="专业版 5x",
            product=SimpleNamespace(
                id="product-pro",
                    name="专业版",
                ),
            ),
        ),
    )
    captured: dict[str, Any] = {}

    @contextmanager
    def _fake_db_context() -> Any:
        yield db

    monkeypatch.setattr("src.api.subscription.routes.get_db_context", _fake_db_context)
    monkeypatch.setattr(
        "src.api.subscription.routes.PaymentService.get_user_order",
        lambda _db, *, user_id, order_id: order,
    )

    def _expire_order(
        _db: MagicMock, *, order: Any, reason: str | None = None
    ) -> tuple[Any, bool]:
        captured["order"] = order
        captured["reason"] = reason
        order.status = "expired"
        order.gateway_response = {"expire_reason": reason}
        return order, True

    monkeypatch.setattr("src.api.subscription.routes.PaymentService.expire_order", _expire_order)

    response = client.post("/api/subscriptions/orders/order-sub-2/cancel")

    assert response.status_code == 200
    payload = response.json()
    assert payload["order"]["status"] == "expired"
    assert payload["order"]["order_type"] == "subscription_renewal"
    assert captured["order"] is order
    assert captured["reason"] == "user_cancelled"
    db.commit.assert_called_once()
