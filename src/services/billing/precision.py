"""
Precision helpers for billing calculations.

We standardize money arithmetic with `Decimal` and quantize to stable precisions.

Notes:
- `Decimal` context precision (`DECIMAL_CONTEXT_PRECISION`) is **significant digits**,
  not "decimal places".
- We keep these as constants (not runtime-configurable) to avoid drift between
  environments during billing reconciliation.
"""

from __future__ import annotations

from decimal import ROUND_HALF_UP, Decimal, localcontext

# Decimal context precision (significant digits)
DECIMAL_CONTEXT_PRECISION = 28

# Money precisions
BILLING_STORAGE_PRECISION = 8  # persisted to DB / metadata
BILLING_DISPLAY_PRECISION = 6  # UI display


def to_decimal(value: float | int | str | Decimal | None) -> Decimal:
    """Convert values to Decimal safely (float via str to avoid binary artifacts)."""
    if value is None:
        return Decimal("0")
    if isinstance(value, Decimal):
        return value
    return Decimal(str(value))


def to_money_decimal(value: float | int | str | Decimal | None) -> Decimal:
    """Convert values to Decimal and quantize to billing storage precision."""
    return quantize_cost(to_decimal(value))


def quantize_decimal(value: Decimal, *, precision: int) -> Decimal:
    """Quantize a Decimal to the given number of decimal places (ROUND_HALF_UP)."""
    quantizer = Decimal(10) ** -precision
    with localcontext() as ctx:
        ctx.prec = DECIMAL_CONTEXT_PRECISION
        return value.quantize(quantizer, rounding=ROUND_HALF_UP)


def quantize_cost(value: Decimal) -> Decimal:
    """Quantize to storage precision."""
    return quantize_decimal(value, precision=BILLING_STORAGE_PRECISION)


def quantize_display(value: Decimal) -> Decimal:
    """Quantize to display precision."""
    return quantize_decimal(value, precision=BILLING_DISPLAY_PRECISION)
