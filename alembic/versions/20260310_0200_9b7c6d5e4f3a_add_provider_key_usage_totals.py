"""add provider_api_keys usage total columns

Revision ID: 9b7c6d5e4f3a
Revises: d7649c1f8e21
Create Date: 2026-03-10 02:00:00.000000+00:00
"""

from __future__ import annotations

from collections.abc import Sequence

import sqlalchemy as sa
from sqlalchemy import inspect

from alembic import op

# revision identifiers, used by Alembic.
revision: str = "9b7c6d5e4f3a"
down_revision: str | None = "d7649c1f8e21"
branch_labels: str | Sequence[str] | None = None
depends_on: str | Sequence[str] | None = None


def column_exists(table_name: str, column_name: str) -> bool:
    bind = op.get_bind()
    inspector = inspect(bind)
    columns = [c["name"] for c in inspector.get_columns(table_name)]
    return column_name in columns


def upgrade() -> None:
    if not column_exists("provider_api_keys", "total_tokens"):
        op.add_column(
            "provider_api_keys",
            sa.Column("total_tokens", sa.BigInteger(), nullable=False, server_default="0"),
        )
    if column_exists("provider_api_keys", "total_tokens"):
        op.alter_column("provider_api_keys", "total_tokens", server_default=None)

    if not column_exists("provider_api_keys", "total_cost_usd"):
        op.add_column(
            "provider_api_keys",
            sa.Column(
                "total_cost_usd",
                sa.Numeric(20, 8),
                nullable=False,
                server_default="0.0",
            ),
        )
    if column_exists("provider_api_keys", "total_cost_usd"):
        op.alter_column("provider_api_keys", "total_cost_usd", server_default=None)


def downgrade() -> None:
    if column_exists("provider_api_keys", "total_cost_usd"):
        op.drop_column("provider_api_keys", "total_cost_usd")

    if column_exists("provider_api_keys", "total_tokens"):
        op.drop_column("provider_api_keys", "total_tokens")
