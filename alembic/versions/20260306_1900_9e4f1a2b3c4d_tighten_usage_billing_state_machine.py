"""tighten usage billing state machine

Revision ID: 9e4f1a2b3c4d
Revises: 7c91d2e4f8a1
Create Date: 2026-03-06 19:00:00.000000+00:00

This migration does two things:
1. Change new `usage.billing_status` default from `settled` to `pending`.
2. Repair only the clearly-safe inconsistent historical rows for production:
   - failed/cancelled zero-cost rows that were marked settled are converted to void
   - terminal rows missing finalized_at are backfilled from created_at

Ambiguous positive-cost settled rows are intentionally left untouched for manual audit.
"""

from __future__ import annotations

from collections.abc import Sequence

import sqlalchemy as sa
from sqlalchemy import inspect

from alembic import op

# revision identifiers, used by Alembic.
revision: str = "9e4f1a2b3c4d"
down_revision: str | None = "7c91d2e4f8a1"
branch_labels: str | Sequence[str] | None = None
depends_on: str | Sequence[str] | None = None


def _table_exists(table_name: str) -> bool:
    bind = op.get_bind()
    insp = inspect(bind)
    insp.clear_cache()
    return table_name in insp.get_table_names()


def _column_exists(table_name: str, column_name: str) -> bool:
    bind = op.get_bind()
    insp = inspect(bind)
    insp.clear_cache()
    return column_name in [col["name"] for col in insp.get_columns(table_name)]


def upgrade() -> None:
    if not _table_exists("usage"):
        return

    if _column_exists("usage", "billing_status"):
        op.alter_column(
            "usage",
            "billing_status",
            existing_type=sa.String(length=20),
            server_default="pending",
            existing_nullable=False,
        )

    required_columns = {
        "billing_status",
        "status",
        "total_cost_usd",
        "request_cost_usd",
        "actual_total_cost_usd",
        "actual_request_cost_usd",
        "wallet_balance_after",
        "finalized_at",
        "created_at",
    }
    if not required_columns.issubset(
        {
            col
            for col in required_columns
            if _column_exists("usage", col)
        }
    ):
        return

    op.execute(
        sa.text(
            """
            UPDATE usage
            SET billing_status = 'pending'
            WHERE billing_status IS NULL
            """
        )
    )

    op.execute(
        sa.text(
            """
            UPDATE usage
            SET billing_status = 'void',
                finalized_at = COALESCE(finalized_at, created_at),
                total_cost_usd = 0,
                request_cost_usd = 0,
                actual_total_cost_usd = 0,
                actual_request_cost_usd = 0
            WHERE billing_status = 'settled'
              AND status IN ('failed', 'cancelled')
              AND COALESCE(total_cost_usd, 0) = 0
              AND wallet_balance_after IS NULL
            """
        )
    )

    op.execute(
        sa.text(
            """
            UPDATE usage
            SET finalized_at = COALESCE(finalized_at, created_at)
            WHERE billing_status IN ('settled', 'void')
              AND finalized_at IS NULL
            """
        )
    )


def downgrade() -> None:
    if not _table_exists("usage") or not _column_exists("usage", "billing_status"):
        return

    op.alter_column(
        "usage",
        "billing_status",
        existing_type=sa.String(length=20),
        server_default="settled",
        existing_nullable=False,
    )
