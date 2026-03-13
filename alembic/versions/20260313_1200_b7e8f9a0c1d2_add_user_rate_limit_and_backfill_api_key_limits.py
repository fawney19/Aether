"""add user rate_limit and backfill normal api key limits

Revision ID: b7e8f9a0c1d2
Revises: c1d2e3f4a5b6
Create Date: 2026-03-13 12:00:00.000000+00:00
"""

from __future__ import annotations

from collections.abc import Sequence

import sqlalchemy as sa
from sqlalchemy import inspect

from alembic import op

# revision identifiers, used by Alembic.
revision: str = "b7e8f9a0c1d2"
down_revision: str | None = "c1d2e3f4a5b6"
branch_labels: str | Sequence[str] | None = None
depends_on: str | Sequence[str] | None = None


def column_exists(table_name: str, column_name: str) -> bool:
    bind = op.get_bind()
    inspector = inspect(bind)
    columns = [c["name"] for c in inspector.get_columns(table_name)]
    return column_name in columns


def upgrade() -> None:
    if not column_exists("users", "rate_limit"):
        op.add_column("users", sa.Column("rate_limit", sa.Integer(), nullable=True))

    # 普通 Key 新语义不再允许 NULL；存量 NULL 统一回填为 0（不限制）。
    op.execute(
        sa.text(
            """
            UPDATE api_keys
            SET rate_limit = 0
            WHERE is_standalone = FALSE
              AND rate_limit IS NULL
            """
        )
    )


def downgrade() -> None:
    if column_exists("users", "rate_limit"):
        op.drop_column("users", "rate_limit")
