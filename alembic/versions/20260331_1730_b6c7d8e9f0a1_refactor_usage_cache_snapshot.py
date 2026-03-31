"""refactor usage cache snapshot schema

Revision ID: b6c7d8e9f0a1
Revises: a8b9c0d1e2f3
Create Date: 2026-03-31 17:30:00.000000+00:00
"""

from __future__ import annotations

from collections.abc import Sequence

import sqlalchemy as sa
from sqlalchemy import inspect
from sqlalchemy.dialects import postgresql

from alembic import op

# revision identifiers, used by Alembic.
revision: str = "b6c7d8e9f0a1"
down_revision: str | None = "a8b9c0d1e2f3"
branch_labels: str | Sequence[str] | None = None
depends_on: str | Sequence[str] | None = None


def _column_exists(table_name: str, column_name: str) -> bool:
    bind = op.get_bind()
    inspector = inspect(bind)
    return any(col["name"] == column_name for col in inspector.get_columns(table_name))


def upgrade() -> None:
    if not _column_exists("usage", "cache_ttl_minutes"):
        with op.batch_alter_table("usage") as batch_op:
            batch_op.add_column(
                sa.Column("cache_ttl_minutes", sa.Integer(), nullable=False, server_default="5")
            )

    if not _column_exists("usage", "upstream_usage_snapshot"):
        with op.batch_alter_table("usage") as batch_op:
            batch_op.add_column(
                sa.Column(
                    "upstream_usage_snapshot",
                    postgresql.JSONB(astext_type=sa.Text()),
                    nullable=True,
                )
            )

    op.execute(
        sa.text(
            """
            UPDATE usage
            SET cache_ttl_minutes = CASE
                WHEN COALESCE(
                    NULLIF(request_metadata->'billing_snapshot'->'resolved_dimensions'->>'cache_ttl_minutes', ''),
                    NULLIF(request_metadata->'billing_snapshot'->'dimensions_used'->>'cache_ttl_minutes', '')
                ) IS NOT NULL THEN COALESCE(
                    NULLIF(request_metadata->'billing_snapshot'->'resolved_dimensions'->>'cache_ttl_minutes', ''),
                    NULLIF(request_metadata->'billing_snapshot'->'dimensions_used'->>'cache_ttl_minutes', '')
                )::integer
                WHEN COALESCE(cache_creation_input_tokens_1h, 0) > 0 THEN 60
                WHEN COALESCE(cache_creation_cost_usd_1h, 0) > 0 THEN 60
                WHEN cache_creation_price_per_1m_1h IS NOT NULL THEN 60
                WHEN COALESCE(cache_creation_input_tokens, 0) > 0
                     OR COALESCE(cache_read_input_tokens, 0) > 0 THEN 5
                ELSE 5
            END
            """
        )
    )

    op.execute(
        sa.text(
            """
            UPDATE usage
            SET upstream_usage_snapshot = jsonb_build_object(
                'snapshot_type', 'response_usage',
                'api_family', NULLIF(LOWER(COALESCE(provider_api_family, api_family, '')), ''),
                'usage', COALESCE(
                    response_body::jsonb -> 'usage',
                    response_body::jsonb -> 'message' -> 'usage',
                    response_body::jsonb -> 'usageMetadata'
                )
            )
            WHERE upstream_usage_snapshot IS NULL
              AND response_body IS NOT NULL
              AND (
                    response_body::jsonb ? 'usage'
                    OR (
                        response_body::jsonb ? 'message'
                        AND jsonb_typeof(response_body::jsonb -> 'message') = 'object'
                        AND (response_body::jsonb -> 'message') ? 'usage'
                    )
                    OR response_body::jsonb ? 'usageMetadata'
              )
            """
        )
    )

    op.execute(
        sa.text(
            """
            UPDATE usage
            SET upstream_usage_snapshot = jsonb_build_object(
                'snapshot_type', 'parsed_stream_usage_events',
                'api_family', NULLIF(LOWER(COALESCE(provider_api_family, api_family, '')), ''),
                'events', response_body::jsonb -> 'chunks'
            )
            WHERE upstream_usage_snapshot IS NULL
              AND response_body IS NOT NULL
              AND jsonb_typeof(response_body::jsonb) = 'object'
              AND jsonb_typeof(response_body::jsonb -> 'chunks') = 'array'
            """
        )
    )

    with op.batch_alter_table("usage") as batch_op:
        if _column_exists("usage", "cache_creation_input_tokens_5m"):
            batch_op.drop_column("cache_creation_input_tokens_5m")
        if _column_exists("usage", "cache_creation_input_tokens_1h"):
            batch_op.drop_column("cache_creation_input_tokens_1h")
        if _column_exists("usage", "cache_creation_cost_usd_5m"):
            batch_op.drop_column("cache_creation_cost_usd_5m")
        if _column_exists("usage", "cache_creation_cost_usd_1h"):
            batch_op.drop_column("cache_creation_cost_usd_1h")
        if _column_exists("usage", "actual_cache_creation_cost_usd_5m"):
            batch_op.drop_column("actual_cache_creation_cost_usd_5m")
        if _column_exists("usage", "actual_cache_creation_cost_usd_1h"):
            batch_op.drop_column("actual_cache_creation_cost_usd_1h")
        if _column_exists("usage", "cache_creation_price_per_1m_5m"):
            batch_op.drop_column("cache_creation_price_per_1m_5m")
        if _column_exists("usage", "cache_creation_price_per_1m_1h"):
            batch_op.drop_column("cache_creation_price_per_1m_1h")


def downgrade() -> None:
    with op.batch_alter_table("usage") as batch_op:
        if not _column_exists("usage", "cache_creation_input_tokens_5m"):
            batch_op.add_column(
                sa.Column("cache_creation_input_tokens_5m", sa.Integer(), nullable=False, server_default="0")
            )
        if not _column_exists("usage", "cache_creation_input_tokens_1h"):
            batch_op.add_column(
                sa.Column("cache_creation_input_tokens_1h", sa.Integer(), nullable=False, server_default="0")
            )
        if not _column_exists("usage", "cache_creation_cost_usd_5m"):
            batch_op.add_column(
                sa.Column("cache_creation_cost_usd_5m", sa.Numeric(20, 8), nullable=False, server_default="0")
            )
        if not _column_exists("usage", "cache_creation_cost_usd_1h"):
            batch_op.add_column(
                sa.Column("cache_creation_cost_usd_1h", sa.Numeric(20, 8), nullable=False, server_default="0")
            )
        if not _column_exists("usage", "actual_cache_creation_cost_usd_5m"):
            batch_op.add_column(
                sa.Column("actual_cache_creation_cost_usd_5m", sa.Numeric(20, 8), nullable=False, server_default="0")
            )
        if not _column_exists("usage", "actual_cache_creation_cost_usd_1h"):
            batch_op.add_column(
                sa.Column("actual_cache_creation_cost_usd_1h", sa.Numeric(20, 8), nullable=False, server_default="0")
            )
        if not _column_exists("usage", "cache_creation_price_per_1m_5m"):
            batch_op.add_column(
                sa.Column("cache_creation_price_per_1m_5m", sa.Numeric(20, 8), nullable=True)
            )
        if not _column_exists("usage", "cache_creation_price_per_1m_1h"):
            batch_op.add_column(
                sa.Column("cache_creation_price_per_1m_1h", sa.Numeric(20, 8), nullable=True)
            )

    op.execute(
        sa.text(
            """
            UPDATE usage
            SET
                cache_creation_input_tokens_5m = CASE
                    WHEN COALESCE(cache_creation_input_tokens, 0) > 0
                         AND COALESCE(cache_ttl_minutes, 5) < 60
                    THEN cache_creation_input_tokens
                    ELSE 0
                END,
                cache_creation_input_tokens_1h = CASE
                    WHEN COALESCE(cache_creation_input_tokens, 0) > 0
                         AND COALESCE(cache_ttl_minutes, 5) >= 60
                    THEN cache_creation_input_tokens
                    ELSE 0
                END,
                cache_creation_cost_usd_5m = CASE
                    WHEN COALESCE(cache_creation_cost_usd, 0) > 0
                         AND COALESCE(cache_ttl_minutes, 5) < 60
                    THEN cache_creation_cost_usd
                    ELSE 0
                END,
                cache_creation_cost_usd_1h = CASE
                    WHEN COALESCE(cache_creation_cost_usd, 0) > 0
                         AND COALESCE(cache_ttl_minutes, 5) >= 60
                    THEN cache_creation_cost_usd
                    ELSE 0
                END,
                actual_cache_creation_cost_usd_5m = CASE
                    WHEN COALESCE(actual_cache_creation_cost_usd, 0) > 0
                         AND COALESCE(cache_ttl_minutes, 5) < 60
                    THEN actual_cache_creation_cost_usd
                    ELSE 0
                END,
                actual_cache_creation_cost_usd_1h = CASE
                    WHEN COALESCE(actual_cache_creation_cost_usd, 0) > 0
                         AND COALESCE(cache_ttl_minutes, 5) >= 60
                    THEN actual_cache_creation_cost_usd
                    ELSE 0
                END,
                cache_creation_price_per_1m_5m = CASE
                    WHEN COALESCE(cache_creation_input_tokens, 0) > 0
                         AND COALESCE(cache_ttl_minutes, 5) < 60
                    THEN cache_creation_price_per_1m
                    ELSE NULL
                END,
                cache_creation_price_per_1m_1h = CASE
                    WHEN COALESCE(cache_creation_input_tokens, 0) > 0
                         AND COALESCE(cache_ttl_minutes, 5) >= 60
                    THEN cache_creation_price_per_1m
                    ELSE NULL
                END
            """
        )
    )

    with op.batch_alter_table("usage") as batch_op:
        if _column_exists("usage", "upstream_usage_snapshot"):
            batch_op.drop_column("upstream_usage_snapshot")
        if _column_exists("usage", "cache_ttl_minutes"):
            batch_op.drop_column("cache_ttl_minutes")
