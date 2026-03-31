from __future__ import annotations

from decimal import Decimal
from unittest.mock import MagicMock

from src.models.database import RechargePackage
from src.services.payment import RechargePackageService


def test_calculate_pay_amount_uses_credit_ratio() -> None:
    pay_amount = RechargePackageService.calculate_pay_amount(
        recharge_amount_usd="500",
        credit_ratio="2",
    )

    assert pay_amount == Decimal("250.00")


def test_serialize_package_marks_unavailable_when_outside_range() -> None:
    package = RechargePackage(
        id="pkg-1",
        name="标准 500",
        recharge_amount_usd=Decimal("500"),
        bonus_amount_usd=Decimal("50"),
        sort_order=10,
        is_active=True,
    )

    payload = RechargePackageService.serialize_package(
        package,
        credit_ratio=2,
        min_amount=300,
        max_amount=1000,
    )

    assert payload["pay_amount"] == 250.0
    assert payload["total_amount_usd"] == 550.0
    assert payload["available"] is False
    assert payload["availability_message"] == "当前套餐低于系统最小充值金额"


def test_create_package_flushes_normalized_values() -> None:
    db = MagicMock()

    package = RechargePackageService.create_package(
        db,
        name="  入门套餐  ",
        recharge_amount_usd="100",
        bonus_amount_usd="10",
        sort_order=3,
        description="  首充推荐  ",
        is_active=True,
    )

    db.add.assert_called_once()
    db.flush.assert_called_once()
    assert package.name == "入门套餐"
    assert package.description == "首充推荐"
    assert package.recharge_amount_usd == Decimal("100.00000000")
    assert package.bonus_amount_usd == Decimal("10.00000000")
    assert package.sort_order == 3
    assert package.is_active is True
