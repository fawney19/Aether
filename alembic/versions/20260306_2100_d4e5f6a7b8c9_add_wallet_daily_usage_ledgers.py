"""add wallet daily usage ledgers

Revision ID: d4e5f6a7b8c9
Revises: 9e4f1a2b3c4d
Create Date: 2026-03-06 21:00:00.000000+00:00
"""

from __future__ import annotations

from collections.abc import Sequence

import sqlalchemy as sa
from sqlalchemy import inspect

from alembic import op

# revision identifiers, used by Alembic.
revision: str = "d4e5f6a7b8c9"
down_revision: str | None = "9e4f1a2b3c4d"
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


def _index_exists(table_name: str, index_name: str) -> bool:
    bind = op.get_bind()
    insp = inspect(bind)
    insp.clear_cache()
    return any(idx["name"] == index_name for idx in insp.get_indexes(table_name))


def upgrade() -> None:
    if not _table_exists("wallet_daily_usage_ledgers"):
        op.create_table(
            "wallet_daily_usage_ledgers",
            sa.Column("id", sa.String(length=36), nullable=False),
            sa.Column("wallet_id", sa.String(length=36), nullable=False),
            sa.Column("billing_date", sa.Date(), nullable=False),
            sa.Column("billing_timezone", sa.String(length=64), nullable=False),
            sa.Column("total_cost_usd", sa.Numeric(20, 8), nullable=False, server_default="0"),
            sa.Column("total_requests", sa.Integer(), nullable=False, server_default="0"),
            sa.Column("input_tokens", sa.BigInteger(), nullable=False, server_default="0"),
            sa.Column("output_tokens", sa.BigInteger(), nullable=False, server_default="0"),
            sa.Column("cache_creation_tokens", sa.BigInteger(), nullable=False, server_default="0"),
            sa.Column("cache_read_tokens", sa.BigInteger(), nullable=False, server_default="0"),
            sa.Column("first_finalized_at", sa.DateTime(timezone=True), nullable=True),
            sa.Column("last_finalized_at", sa.DateTime(timezone=True), nullable=True),
            sa.Column("aggregated_at", sa.DateTime(timezone=True), nullable=False),
            sa.Column("created_at", sa.DateTime(timezone=True), nullable=False),
            sa.Column("updated_at", sa.DateTime(timezone=True), nullable=False),
            sa.ForeignKeyConstraint(["wallet_id"], ["wallets.id"], ondelete="CASCADE"),
            sa.PrimaryKeyConstraint("id"),
            sa.UniqueConstraint(
                "wallet_id",
                "billing_date",
                "billing_timezone",
                name="uq_wallet_daily_usage_ledgers_wallet_date_tz",
            ),
        )

    if not _index_exists("wallet_daily_usage_ledgers", "idx_wallet_daily_usage_wallet_date"):
        op.create_index(
            "idx_wallet_daily_usage_wallet_date",
            "wallet_daily_usage_ledgers",
            ["wallet_id", "billing_date"],
        )
    if not _index_exists("wallet_daily_usage_ledgers", "idx_wallet_daily_usage_date"):
        op.create_index(
            "idx_wallet_daily_usage_date",
            "wallet_daily_usage_ledgers",
            ["billing_date"],
        )

    if _table_exists("usage") and all(
        _column_exists("usage", col)
        for col in ["billing_status", "finalized_at", "wallet_id"]
    ) and not _index_exists("usage", "idx_usage_billing_finalized_wallet"):
        op.create_index(
            "idx_usage_billing_finalized_wallet",
            "usage",
            ["billing_status", "finalized_at", "wallet_id"],
        )


def downgrade() -> None:
    if _table_exists("usage") and _index_exists("usage", "idx_usage_billing_finalized_wallet"):
        op.drop_index("idx_usage_billing_finalized_wallet", table_name="usage")

    if _table_exists("wallet_daily_usage_ledgers"):
        if _index_exists("wallet_daily_usage_ledgers", "idx_wallet_daily_usage_date"):
            op.drop_index("idx_wallet_daily_usage_date", table_name="wallet_daily_usage_ledgers")
        if _index_exists("wallet_daily_usage_ledgers", "idx_wallet_daily_usage_wallet_date"):
            op.drop_index(
                "idx_wallet_daily_usage_wallet_date",
                table_name="wallet_daily_usage_ledgers",
            )
        op.drop_table("wallet_daily_usage_ledgers")
