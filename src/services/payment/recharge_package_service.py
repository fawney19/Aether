from __future__ import annotations

from decimal import Decimal, ROUND_HALF_UP
from typing import Any

from sqlalchemy.orm import Session

from src.models.database import RechargePackage
from src.services.billing.precision import to_money_decimal

PAYMENT_AMOUNT_QUANT = Decimal("0.01")


class RechargePackageService:
    """充值套餐管理与金额换算。"""

    @staticmethod
    def list_packages(
        db: Session,
        *,
        active_only: bool | None = None,
    ) -> list[RechargePackage]:
        query = db.query(RechargePackage)
        if active_only is True:
            query = query.filter(RechargePackage.is_active.is_(True))
        elif active_only is False:
            query = query.filter(RechargePackage.is_active.is_(False))
        return (
            query.order_by(
                RechargePackage.sort_order.asc(),
                RechargePackage.created_at.asc(),
            ).all()
        )

    @staticmethod
    def get_package(db: Session, package_id: str) -> RechargePackage | None:
        return db.query(RechargePackage).filter(RechargePackage.id == package_id).first()

    @classmethod
    def create_package(
        cls,
        db: Session,
        *,
        name: str,
        recharge_amount_usd: Decimal | float | int | str,
        bonus_amount_usd: Decimal | float | int | str = 0,
        sort_order: int = 0,
        description: str | None = None,
        is_active: bool = True,
    ) -> RechargePackage:
        normalized_name = name.strip()
        if not normalized_name:
            raise ValueError("package name is required")
        recharge_amount = to_money_decimal(recharge_amount_usd)
        bonus_amount = to_money_decimal(bonus_amount_usd)
        if recharge_amount <= Decimal("0"):
            raise ValueError("recharge amount must be positive")
        if bonus_amount < Decimal("0"):
            raise ValueError("bonus amount must not be negative")

        package = RechargePackage(
            name=normalized_name,
            description=description.strip() if isinstance(description, str) else None,
            recharge_amount_usd=recharge_amount,
            bonus_amount_usd=bonus_amount,
            sort_order=int(sort_order),
            is_active=bool(is_active),
        )
        db.add(package)
        db.flush()
        return package

    @classmethod
    def update_package(
        cls,
        db: Session,
        *,
        package: RechargePackage,
        name: str,
        recharge_amount_usd: Decimal | float | int | str,
        bonus_amount_usd: Decimal | float | int | str = 0,
        sort_order: int = 0,
        description: str | None = None,
        is_active: bool = True,
    ) -> RechargePackage:
        normalized_name = name.strip()
        if not normalized_name:
            raise ValueError("package name is required")
        recharge_amount = to_money_decimal(recharge_amount_usd)
        bonus_amount = to_money_decimal(bonus_amount_usd)
        if recharge_amount <= Decimal("0"):
            raise ValueError("recharge amount must be positive")
        if bonus_amount < Decimal("0"):
            raise ValueError("bonus amount must not be negative")

        package.name = normalized_name
        package.description = description.strip() if isinstance(description, str) else None
        package.recharge_amount_usd = recharge_amount
        package.bonus_amount_usd = bonus_amount
        package.sort_order = int(sort_order)
        package.is_active = bool(is_active)
        db.flush()
        return package

    @staticmethod
    def delete_package(db: Session, *, package: RechargePackage) -> None:
        db.delete(package)
        db.flush()

    @staticmethod
    def get_total_amount(package: RechargePackage) -> Decimal:
        return to_money_decimal(package.recharge_amount_usd) + to_money_decimal(package.bonus_amount_usd)

    @staticmethod
    def calculate_pay_amount(
        *,
        recharge_amount_usd: Decimal | float | int | str,
        credit_ratio: Decimal | float | int | str,
    ) -> Decimal:
        ratio = Decimal(str(credit_ratio))
        if ratio <= Decimal("0"):
            raise ValueError("credit_ratio must be positive")
        recharge_amount = to_money_decimal(recharge_amount_usd)
        if recharge_amount <= Decimal("0"):
            raise ValueError("recharge amount must be positive")
        return (recharge_amount / ratio).quantize(PAYMENT_AMOUNT_QUANT, rounding=ROUND_HALF_UP)

    @classmethod
    def serialize_package(
        cls,
        package: RechargePackage,
        *,
        credit_ratio: Decimal | float | int | str,
        min_amount: Decimal | float | int | str | None = None,
        max_amount: Decimal | float | int | str | None = None,
    ) -> dict[str, Any]:
        pay_amount = cls.calculate_pay_amount(
            recharge_amount_usd=package.recharge_amount_usd,
            credit_ratio=credit_ratio,
        )
        available = True
        availability_message: str | None = None
        if min_amount is not None and pay_amount < Decimal(str(min_amount)):
            available = False
            availability_message = "当前套餐低于系统最小充值金额"
        if max_amount is not None and pay_amount > Decimal(str(max_amount)):
            available = False
            availability_message = "当前套餐超过系统最大充值金额"
        return {
            "id": package.id,
            "name": package.name,
            "description": package.description,
            "recharge_amount_usd": float(package.recharge_amount_usd or 0),
            "bonus_amount_usd": float(package.bonus_amount_usd or 0),
            "total_amount_usd": float(cls.get_total_amount(package)),
            "pay_amount": float(pay_amount),
            "pay_currency": "CNY",
            "sort_order": int(package.sort_order or 0),
            "is_active": bool(package.is_active),
            "available": available,
            "availability_message": availability_message,
            "created_at": package.created_at,
            "updated_at": package.updated_at,
        }
