"""add recharge packages and payment order bonus amount

Revision ID: a7b8c9d0e1f2
Revises: f1a2b3c4d5e6
Create Date: 2026-03-31 15:00:00.000000+00:00
"""

from __future__ import annotations

from collections.abc import Sequence

import sqlalchemy as sa

from alembic import op

# revision identifiers, used by Alembic.
revision: str = "a7b8c9d0e1f2"
down_revision: str | None = "f1a2b3c4d5e6"
branch_labels: str | Sequence[str] | None = None
depends_on: str | Sequence[str] | None = None


def upgrade() -> None:
    op.create_table(
        "recharge_packages",
        sa.Column("id", sa.String(length=36), nullable=False),
        sa.Column("name", sa.String(length=100), nullable=False),
        sa.Column("description", sa.Text(), nullable=True),
        sa.Column("recharge_amount_usd", sa.Numeric(20, 8), nullable=False),
        sa.Column("bonus_amount_usd", sa.Numeric(20, 8), nullable=False, server_default="0"),
        sa.Column("sort_order", sa.Integer(), nullable=False, server_default="0"),
        sa.Column("is_active", sa.Boolean(), nullable=False, server_default=sa.true()),
        sa.Column("created_at", sa.DateTime(timezone=True), nullable=False),
        sa.Column("updated_at", sa.DateTime(timezone=True), nullable=False),
        sa.CheckConstraint(
            "recharge_amount_usd > 0",
            name="ck_recharge_packages_recharge_positive",
        ),
        sa.CheckConstraint(
            "bonus_amount_usd >= 0",
            name="ck_recharge_packages_bonus_non_negative",
        ),
        sa.PrimaryKeyConstraint("id"),
    )
    op.create_index(
        "idx_recharge_packages_active_sort",
        "recharge_packages",
        ["is_active", "sort_order", "created_at"],
        unique=False,
    )

    op.add_column(
        "payment_orders",
        sa.Column("bonus_amount_usd", sa.Numeric(20, 8), nullable=False, server_default="0"),
    )

    op.execute(
        sa.text(
            """
            UPDATE payment_orders
            SET bonus_amount_usd = 0
            WHERE bonus_amount_usd IS NULL
            """
        )
    )

    with op.batch_alter_table("recharge_packages") as batch_op:
        batch_op.alter_column("bonus_amount_usd", server_default=None)
        batch_op.alter_column("sort_order", server_default=None)

    with op.batch_alter_table("payment_orders") as batch_op:
        batch_op.alter_column("bonus_amount_usd", server_default=None)


def downgrade() -> None:
    op.drop_column("payment_orders", "bonus_amount_usd")
    op.drop_index("idx_recharge_packages_active_sort", table_name="recharge_packages")
    op.drop_table("recharge_packages")
