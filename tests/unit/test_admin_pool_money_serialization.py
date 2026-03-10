from __future__ import annotations

from decimal import Decimal

from src.api.admin.pool.routes import _serialize_money


def test_serialize_money_preserves_storage_precision() -> None:
    assert _serialize_money(Decimal("12.34")) == "12.34000000"
    assert _serialize_money("0.00000001") == "0.00000001"
    assert _serialize_money(None) == "0.00000000"
