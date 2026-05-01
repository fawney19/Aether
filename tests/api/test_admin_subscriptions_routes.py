from __future__ import annotations

import json
from datetime import datetime, timezone
from decimal import Decimal
from types import SimpleNamespace
from typing import Any
from unittest.mock import MagicMock

import pytest
from fastapi import FastAPI
from fastapi.testclient import TestClient

from src.api.admin.subscriptions.routes import router as admin_subscriptions_router
from src.database import get_db


def _build_admin_subscriptions_app(
    db: MagicMock,
    monkeypatch: pytest.MonkeyPatch,
) -> TestClient:
    app = FastAPI()
    app.include_router(admin_subscriptions_router)
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
            user=SimpleNamespace(id="admin-1"),
            ensure_json_body=lambda: payload,
            add_audit_metadata=lambda **_: None,
        )
        return await adapter.handle(context)

    monkeypatch.setattr("src.api.admin.subscriptions.routes.pipeline.run", _fake_pipeline_run)
    return TestClient(app)


def _make_plan(*, plan_id: str = "plan-1") -> SimpleNamespace:
    now = datetime.now(timezone.utc)
    return SimpleNamespace(
        id=plan_id,
        code="pro",
        name="专业版",
        description="高阶套餐",
        user_group_id="group-pro",
        user_group=SimpleNamespace(name="高级用户"),
        plan_level=20,
        monthly_price_usd=Decimal("20"),
        monthly_quota_usd=Decimal("50"),
        overage_policy="use_wallet_balance",
        term_discounts_json=[
            {"months": 1, "discount_factor": 1.0},
            {"months": 12, "discount_factor": 0.75},
        ],
        is_active=True,
        created_at=now,
        updated_at=now,
    )


def _make_subscription(*, subscription_id: str = "sub-1") -> SimpleNamespace:
    now = datetime.now(timezone.utc)
    return SimpleNamespace(
        id=subscription_id,
        user_id="user-1",
        user=SimpleNamespace(username="alice", email="alice@example.com"),
        plan_id="plan-pro",
        plan=SimpleNamespace(
            id="plan-pro",
            code="pro",
            name="专业版",
            user_group_id="group-pro",
            user_group=SimpleNamespace(name="高级用户"),
            overage_policy="use_wallet_balance",
        ),
        status="active",
        end_reason=None,
        purchased_months=12,
        discount_factor=Decimal("0.75"),
        monthly_price_usd_snapshot=Decimal("20"),
        total_price_usd=Decimal("180"),
        started_at=now,
        ends_at=now,
        current_cycle_start=now,
        current_cycle_end=now,
        cycle_quota_usd=Decimal("50"),
        cycle_used_usd=Decimal("12"),
        cancel_at_period_end=False,
        canceled_at=None,
        ended_at=None,
        upgraded_from_subscription_id=None,
        created_at=now,
        updated_at=now,
    )


def _make_subscription_order(*, order_id: str = "order-1", status: str = "pending_approval") -> SimpleNamespace:
    now = datetime.now(timezone.utc)
    subscription = _make_subscription(subscription_id="sub-1")
    subscription.plan.product = SimpleNamespace(id="product-pro", name="专业版")
    return SimpleNamespace(
        id=order_id,
        order_no=f"po-{order_id}",
        wallet_id="wallet-1",
        user_id="user-1",
        user=SimpleNamespace(username="alice", email="alice@example.com"),
        subscription_id=subscription.id,
        subscription=subscription,
        amount_usd=Decimal("180"),
        pay_amount=None,
        pay_currency=None,
        exchange_rate=None,
        refunded_amount_usd=Decimal("0"),
        refundable_amount_usd=Decimal("0"),
        payment_method="manual_review",
        order_type="subscription_initial",
        gateway_order_id=f"gw-{order_id}",
        gateway_response={"gateway": "manual_review", "instructions": "wait"},
        status=status,
        created_at=now,
        paid_at=None,
        credited_at=None,
        expires_at=None,
    )


def _make_payment_callback(*, callback_id: str = "cb-1") -> SimpleNamespace:
    now = datetime.now(timezone.utc)
    return SimpleNamespace(
        id=callback_id,
        payment_order_id="order-1",
        payment_method="alipay",
        callback_key=f"key-{callback_id}",
        order_no="po-order-1",
        gateway_order_id=f"gw-{callback_id}",
        payload_hash="hash-1",
        signature_valid=True,
        status="processed",
        payload={"foo": "bar"},
        error_message=None,
        created_at=now,
        processed_at=now,
    )


def test_list_subscription_plans_route_returns_serialized_plans(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    db = MagicMock()
    client = _build_admin_subscriptions_app(db, monkeypatch)
    plan = _make_plan()

    monkeypatch.setattr(
        "src.api.admin.subscriptions.routes.SubscriptionService.list_plans",
        lambda *_a, **_k: [plan],
    )

    count_query = MagicMock()
    count_query.filter.return_value.group_by.return_value.all.return_value = [(plan.id, 3)]
    db.query.return_value = count_query

    response = client.get("/api/admin/subscriptions/plans")

    assert response.status_code == 200
    payload = response.json()
    assert payload["total"] == 1
    assert payload["plans"][0]["id"] == plan.id
    assert payload["plans"][0]["active_subscription_count"] == 3


def test_create_subscription_plan_route_forwards_validated_payload(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    db = MagicMock()
    client = _build_admin_subscriptions_app(db, monkeypatch)
    plan = _make_plan()
    captured: dict[str, Any] = {}

    def _fake_create_plan(*_args: Any, **kwargs: Any) -> SimpleNamespace:
        captured.update(kwargs)
        return plan

    monkeypatch.setattr(
        "src.api.admin.subscriptions.routes.SubscriptionService.create_plan",
        _fake_create_plan,
    )

    response = client.post(
        "/api/admin/subscriptions/plans",
        json={
            "code": "pro",
            "name": "专业版",
            "description": "高阶套餐",
            "user_group_id": "group-pro",
            "plan_level": 20,
            "monthly_price_usd": 20,
            "monthly_quota_usd": 50,
            "overage_policy": "use_wallet_balance",
            "term_discounts_json": [
                {"months": 1, "discount_factor": 1},
                {"months": 12, "discount_factor": 0.75},
            ],
        },
    )

    assert response.status_code == 201
    assert captured["code"] == "pro"
    assert captured["term_discounts_json"] == [
        {"months": 1, "discount_factor": 1.0},
        {"months": 12, "discount_factor": 0.75},
    ]


def test_get_current_user_subscription_route_returns_null_when_absent(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    db = MagicMock()
    client = _build_admin_subscriptions_app(db, monkeypatch)

    user_query = MagicMock()
    user_query.filter.return_value.first.return_value = SimpleNamespace(id="user-1")
    db.query.return_value = user_query

    monkeypatch.setattr(
        "src.api.admin.subscriptions.routes.SubscriptionService.get_user_current_subscription",
        lambda *_a, **_k: None,
    )

    response = client.get("/api/admin/subscriptions/users/user-1/current")

    assert response.status_code == 200
    assert response.json() is None


def test_get_current_user_subscription_route_uses_display_end(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    db = MagicMock()
    client = _build_admin_subscriptions_app(db, monkeypatch)
    subscription = _make_subscription()
    display_end = datetime.now(timezone.utc)

    user_query = MagicMock()
    user_query.filter.return_value.first.return_value = SimpleNamespace(id="user-1")
    db.query.return_value = user_query

    monkeypatch.setattr(
        "src.api.admin.subscriptions.routes.SubscriptionService.get_user_current_subscription",
        lambda *_a, **_k: subscription,
    )
    monkeypatch.setattr(
        "src.api.admin.subscriptions.routes.SubscriptionService.get_subscription_display_end",
        lambda *_a, **_k: display_end,
    )

    response = client.get("/api/admin/subscriptions/users/user-1/current")

    assert response.status_code == 200
    assert response.json()["ends_at"] == display_end.isoformat().replace("+00:00", "Z")


def test_upgrade_subscription_route_returns_upgraded_subscription(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    db = MagicMock()
    client = _build_admin_subscriptions_app(db, monkeypatch)
    current_subscription = _make_subscription(subscription_id="sub-old")
    subscription = _make_subscription(subscription_id="sub-new")
    subscription.upgraded_from_subscription_id = "sub-old"
    current_subscription.plan.id = "plan-old"
    current_subscription.plan.product = SimpleNamespace(id="product-old", plan_level=10)
    subscription.plan.id = "plan-new"
    subscription.plan.product = SimpleNamespace(id="product-new", plan_level=20)

    monkeypatch.setattr(
        "src.api.admin.subscriptions.routes.SubscriptionService.get_subscription",
        lambda *_a, **_k: current_subscription if _k.get("subscription_id") == "sub-old" or (_a and _a[1] == "sub-old") else subscription,
    )
    monkeypatch.setattr(
        "src.api.admin.subscriptions.routes.SubscriptionService.create_pending_subscription",
        lambda *_a, **_k: subscription,
    )
    monkeypatch.setattr(
        "src.api.admin.subscriptions.routes.SubscriptionService.activate_pending_subscription",
        lambda *_a, **_k: subscription,
    )
    monkeypatch.setattr(
        "src.api.admin.subscriptions.routes.SubscriptionService.get_transition_order_type",
        lambda *_a, **_k: "subscription_upgrade",
    )
    monkeypatch.setattr(
        "src.api.admin.subscriptions.routes.PaymentService.create_settled_subscription_order",
        lambda *_a, **_k: SimpleNamespace(id="order-1"),
    )

    response = client.post(
        "/api/admin/subscriptions/sub-old/upgrade",
        json={
            "new_plan_id": "plan-enterprise",
            "purchased_months": 12,
        },
    )

    assert response.status_code == 200
    payload = response.json()
    assert payload["id"] == "sub-new"
    assert payload["upgraded_from_subscription_id"] == "sub-old"
    assert payload["remaining_quota_usd"] == 38


def test_create_user_subscription_route_records_credited_order(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    db = MagicMock()
    client = _build_admin_subscriptions_app(db, monkeypatch)
    subscription = _make_subscription(subscription_id="sub-created")
    user = SimpleNamespace(id="user-1")

    user_query = MagicMock()
    user_query.filter.return_value.first.return_value = user
    db.query.return_value = user_query

    monkeypatch.setattr(
        "src.api.admin.subscriptions.routes.SubscriptionService.create_subscription",
        lambda *_a, **_k: subscription,
    )

    captured: dict[str, Any] = {}

    def _fake_record_order(*_args: Any, **kwargs: Any) -> SimpleNamespace:
        captured.update(kwargs)
        return SimpleNamespace(id="order-1")

    monkeypatch.setattr(
        "src.api.admin.subscriptions.routes.PaymentService.create_settled_subscription_order",
        _fake_record_order,
    )
    monkeypatch.setattr(
        "src.api.admin.subscriptions.routes.SubscriptionService.get_subscription",
        lambda *_a, **_k: subscription,
    )

    response = client.post(
        "/api/admin/subscriptions/users/user-1",
        json={
            "plan_id": "plan-pro",
            "purchased_months": 3,
        },
    )

    assert response.status_code == 201
    assert captured["payment_method"] == "admin_subscription"
    assert captured["order_type"] == "subscription_initial"
    assert captured["subscription_id"] == subscription.id


def test_list_subscription_orders_route_returns_serialized_orders(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    db = MagicMock()
    client = _build_admin_subscriptions_app(db, monkeypatch)
    order = _make_subscription_order()

    query = MagicMock()
    query.options.return_value = query
    query.filter.return_value = query
    query.order_by.return_value = query
    query.all.return_value = [order]
    db.query.return_value = query

    monkeypatch.setattr(
        "src.api.admin.subscriptions.routes.PaymentService.expire_overdue_pending_orders",
        lambda *_a, **_k: 0,
    )

    response = client.get("/api/admin/subscriptions/orders")

    assert response.status_code == 200
    payload = response.json()
    assert payload["total"] == 1
    assert payload["orders"][0]["order_no"] == order.order_no
    assert payload["orders"][0]["product_name"] == "专业版"
    assert payload["orders"][0]["username"] == "alice"


def test_list_subscription_callbacks_route_returns_subscription_callbacks(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    db = MagicMock()
    client = _build_admin_subscriptions_app(db, monkeypatch)
    callback = _make_payment_callback()
    captured: dict[str, Any] = {}

    def _fake_list_callbacks(_db: MagicMock, **kwargs: Any) -> tuple[list[SimpleNamespace], int]:
        captured.update(kwargs)
        return [callback], 1

    monkeypatch.setattr(
        "src.api.admin.subscriptions.routes.PaymentService.list_callbacks",
        _fake_list_callbacks,
    )

    response = client.get(
        "/api/admin/subscriptions/callbacks",
        params={"payment_method": "alipay", "limit": 10, "offset": 5},
    )

    assert response.status_code == 200
    assert captured["payment_method"] == "alipay"
    assert set(captured["order_types"]) == {
        "subscription_initial",
        "subscription_upgrade",
        "subscription_renewal",
    }
    assert captured["limit"] == 10
    assert captured["offset"] == 5
    payload = response.json()
    assert payload["total"] == 1
    assert payload["items"][0]["callback_key"] == callback.callback_key


def test_approve_subscription_order_route_credits_manual_review_order(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    db = MagicMock()
    client = _build_admin_subscriptions_app(db, monkeypatch)
    order = _make_subscription_order()
    credited_order = _make_subscription_order(status="credited")
    credited_order.id = order.id
    credited_order.order_no = order.order_no

    monkeypatch.setattr(
        "src.api.admin.subscriptions.routes.PaymentService.get_order",
        lambda *_a, **_k: order,
    )
    monkeypatch.setattr(
        "src.api.admin.subscriptions.routes.PaymentService.credit_order",
        lambda *_a, **_k: (credited_order, True),
    )

    query = MagicMock()
    query.options.return_value = query
    query.filter.return_value = query
    query.first.return_value = credited_order
    db.query.return_value = query

    response = client.post(f"/api/admin/subscriptions/orders/{order.id}/approve")

    assert response.status_code == 200
    payload = response.json()
    assert payload["order"]["id"] == order.id
    assert payload["order"]["status"] == "credited"
