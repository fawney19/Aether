"""usage token semantics v2

Revision ID: c3d4e5f6a7b8
Revises: b7e8f9a0c1d2
Create Date: 2026-03-17 14:00:00.000000+00:00
"""

from __future__ import annotations

from collections.abc import Sequence

import sqlalchemy as sa
from sqlalchemy import inspect

from alembic import op

# revision identifiers, used by Alembic.
revision: str = "c3d4e5f6a7b8"
down_revision: str | None = "b7e8f9a0c1d2"
branch_labels: str | Sequence[str] | None = None
depends_on: str | Sequence[str] | None = None


def column_exists(table_name: str, column_name: str) -> bool:
    bind = op.get_bind()
    inspector = inspect(bind)
    columns = [c["name"] for c in inspector.get_columns(table_name)]
    return column_name in columns


def upgrade() -> None:
    if column_exists("usage", "total_tokens") and not column_exists("usage", "input_output_total_tokens"):
        with op.batch_alter_table("usage") as batch_op:
            batch_op.alter_column(
                "total_tokens",
                new_column_name="input_output_total_tokens",
                existing_type=sa.Integer(),
                existing_nullable=True,
            )

    with op.batch_alter_table("usage") as batch_op:
        if not column_exists("usage", "input_context_tokens"):
            batch_op.add_column(sa.Column("input_context_tokens", sa.Integer(), nullable=False, server_default="0"))
        if not column_exists("usage", "total_tokens"):
            batch_op.add_column(sa.Column("total_tokens", sa.Integer(), nullable=False, server_default="0"))
        if not column_exists("usage", "cache_creation_cost_usd_5m"):
            batch_op.add_column(sa.Column("cache_creation_cost_usd_5m", sa.Numeric(20, 8), nullable=False, server_default="0"))
        if not column_exists("usage", "cache_creation_cost_usd_1h"):
            batch_op.add_column(sa.Column("cache_creation_cost_usd_1h", sa.Numeric(20, 8), nullable=False, server_default="0"))
        if not column_exists("usage", "actual_cache_creation_cost_usd_5m"):
            batch_op.add_column(sa.Column("actual_cache_creation_cost_usd_5m", sa.Numeric(20, 8), nullable=False, server_default="0"))
        if not column_exists("usage", "actual_cache_creation_cost_usd_1h"):
            batch_op.add_column(sa.Column("actual_cache_creation_cost_usd_1h", sa.Numeric(20, 8), nullable=False, server_default="0"))
        if not column_exists("usage", "actual_cache_cost_usd"):
            batch_op.add_column(sa.Column("actual_cache_cost_usd", sa.Numeric(20, 8), nullable=False, server_default="0"))
        if not column_exists("usage", "cache_creation_price_per_1m_5m"):
            batch_op.add_column(sa.Column("cache_creation_price_per_1m_5m", sa.Numeric(20, 8), nullable=True))
        if not column_exists("usage", "cache_creation_price_per_1m_1h"):
            batch_op.add_column(sa.Column("cache_creation_price_per_1m_1h", sa.Numeric(20, 8), nullable=True))

    op.execute(
        sa.text(
            """
            UPDATE usage
            SET
                input_output_total_tokens = COALESCE(input_output_total_tokens, COALESCE(input_tokens, 0) + COALESCE(output_tokens, 0)),
                input_context_tokens = COALESCE(input_tokens, 0) + COALESCE(cache_read_input_tokens, 0),
                total_tokens = COALESCE(input_output_total_tokens, 0)
                    + COALESCE(cache_creation_input_tokens, 0)
                    + COALESCE(cache_read_input_tokens, 0),
                cache_creation_cost_usd_5m = CASE
                    WHEN COALESCE(cache_creation_input_tokens_5m, 0) > 0
                         AND COALESCE(cache_creation_input_tokens_1h, 0) = 0
                    THEN COALESCE(cache_creation_cost_usd, 0)
                    WHEN COALESCE(cache_creation_input_tokens_5m, 0) > 0
                         AND COALESCE(cache_creation_input_tokens, 0) > 0
                    THEN COALESCE(cache_creation_cost_usd, 0)
                         * (COALESCE(cache_creation_input_tokens_5m, 0) * 1.0 / COALESCE(cache_creation_input_tokens, 1))
                    ELSE 0
                END,
                cache_creation_cost_usd_1h = CASE
                    WHEN COALESCE(cache_creation_input_tokens_1h, 0) > 0
                         AND COALESCE(cache_creation_input_tokens_5m, 0) = 0
                    THEN COALESCE(cache_creation_cost_usd, 0)
                    WHEN COALESCE(cache_creation_input_tokens_1h, 0) > 0
                         AND COALESCE(cache_creation_input_tokens, 0) > 0
                    THEN COALESCE(cache_creation_cost_usd, 0)
                         * (COALESCE(cache_creation_input_tokens_1h, 0) * 1.0 / COALESCE(cache_creation_input_tokens, 1))
                    ELSE 0
                END,
                actual_cache_creation_cost_usd_5m = CASE
                    WHEN COALESCE(cache_creation_input_tokens_5m, 0) > 0
                         AND COALESCE(cache_creation_input_tokens_1h, 0) = 0
                    THEN COALESCE(actual_cache_creation_cost_usd, 0)
                    WHEN COALESCE(cache_creation_input_tokens_5m, 0) > 0
                         AND COALESCE(cache_creation_input_tokens, 0) > 0
                    THEN COALESCE(actual_cache_creation_cost_usd, 0)
                         * (COALESCE(cache_creation_input_tokens_5m, 0) * 1.0 / COALESCE(cache_creation_input_tokens, 1))
                    ELSE 0
                END,
                actual_cache_creation_cost_usd_1h = CASE
                    WHEN COALESCE(cache_creation_input_tokens_1h, 0) > 0
                         AND COALESCE(cache_creation_input_tokens_5m, 0) = 0
                    THEN COALESCE(actual_cache_creation_cost_usd, 0)
                    WHEN COALESCE(cache_creation_input_tokens_1h, 0) > 0
                         AND COALESCE(cache_creation_input_tokens, 0) > 0
                    THEN COALESCE(actual_cache_creation_cost_usd, 0)
                         * (COALESCE(cache_creation_input_tokens_1h, 0) * 1.0 / COALESCE(cache_creation_input_tokens, 1))
                    ELSE 0
                END,
                actual_cache_cost_usd = COALESCE(actual_cache_creation_cost_usd, 0) + COALESCE(actual_cache_read_cost_usd, 0),
                cache_creation_price_per_1m_5m = CASE
                    WHEN COALESCE(cache_creation_input_tokens_5m, 0) > 0
                         AND COALESCE(cache_creation_input_tokens_1h, 0) = 0
                    THEN cache_creation_price_per_1m
                    ELSE NULL
                END,
                cache_creation_price_per_1m_1h = CASE
                    WHEN COALESCE(cache_creation_input_tokens_1h, 0) > 0
                         AND COALESCE(cache_creation_input_tokens_5m, 0) = 0
                    THEN cache_creation_price_per_1m
                    ELSE NULL
                END,
                cache_cost_usd = COALESCE(cache_creation_cost_usd, 0) + COALESCE(cache_read_cost_usd, 0)
            """
        )
    )


def downgrade() -> None:
    op.execute(
        sa.text(
            """
            UPDATE usage
            SET total_tokens = COALESCE(input_output_total_tokens, COALESCE(input_tokens, 0) + COALESCE(output_tokens, 0))
            """
        )
    )

    with op.batch_alter_table("usage") as batch_op:
        if column_exists("usage", "cache_creation_price_per_1m_1h"):
            batch_op.drop_column("cache_creation_price_per_1m_1h")
        if column_exists("usage", "cache_creation_price_per_1m_5m"):
            batch_op.drop_column("cache_creation_price_per_1m_5m")
        if column_exists("usage", "actual_cache_cost_usd"):
            batch_op.drop_column("actual_cache_cost_usd")
        if column_exists("usage", "actual_cache_creation_cost_usd_1h"):
            batch_op.drop_column("actual_cache_creation_cost_usd_1h")
        if column_exists("usage", "actual_cache_creation_cost_usd_5m"):
            batch_op.drop_column("actual_cache_creation_cost_usd_5m")
        if column_exists("usage", "cache_creation_cost_usd_1h"):
            batch_op.drop_column("cache_creation_cost_usd_1h")
        if column_exists("usage", "cache_creation_cost_usd_5m"):
            batch_op.drop_column("cache_creation_cost_usd_5m")
        if column_exists("usage", "input_context_tokens"):
            batch_op.drop_column("input_context_tokens")
        if column_exists("usage", "total_tokens"):
            batch_op.drop_column("total_tokens")
        if column_exists("usage", "input_output_total_tokens"):
            batch_op.alter_column(
                "input_output_total_tokens",
                new_column_name="total_tokens",
                existing_type=sa.Integer(),
                existing_nullable=True,
            )
