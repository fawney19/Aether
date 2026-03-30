"""add payment order status check fields

Revision ID: f1a2b3c4d5e6
Revises: e2f4a6b8c9d0
Create Date: 2026-03-31 02:00:00.000000+00:00
"""

from __future__ import annotations

from collections.abc import Sequence

import sqlalchemy as sa

from alembic import op

# revision identifiers, used by Alembic.
revision: str = "f1a2b3c4d5e6"
down_revision: str | None = "e2f4a6b8c9d0"
branch_labels: str | Sequence[str] | None = None
depends_on: str | Sequence[str] | None = None


def upgrade() -> None:
    op.add_column(
        "payment_orders",
        sa.Column("status_check_attempts", sa.Integer(), nullable=False, server_default="0"),
    )
    op.add_column(
        "payment_orders",
        sa.Column("last_status_check_at", sa.DateTime(timezone=True), nullable=True),
    )
    op.add_column(
        "payment_orders",
        sa.Column("next_status_check_at", sa.DateTime(timezone=True), nullable=True),
    )
    op.add_column(
        "payment_orders",
        sa.Column("last_status_check_result", sa.String(length=64), nullable=True),
    )
    op.add_column(
        "payment_orders",
        sa.Column("last_status_check_error", sa.Text(), nullable=True),
    )
    op.create_index(
        "idx_payment_orders_status_check_due",
        "payment_orders",
        ["payment_method", "status", "next_status_check_at"],
        unique=False,
    )
    op.execute(
        """
        UPDATE payment_orders
        SET next_status_check_at = NOW()
        WHERE payment_method = 'alipay' AND status = 'pending'
        """
    )


def downgrade() -> None:
    op.drop_index("idx_payment_orders_status_check_due", table_name="payment_orders")
    op.drop_column("payment_orders", "last_status_check_error")
    op.drop_column("payment_orders", "last_status_check_result")
    op.drop_column("payment_orders", "next_status_check_at")
    op.drop_column("payment_orders", "last_status_check_at")
    op.drop_column("payment_orders", "status_check_attempts")
