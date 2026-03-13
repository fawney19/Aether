"""add user rpm_limit

Revision ID: a1b2c3d4e5f6
Revises: c1d2e3f4a5b6
Create Date: 2026-03-12 09:00:00.000000+00:00

"""

from __future__ import annotations

import sqlalchemy as sa
from alembic import op
from sqlalchemy import inspect

# revision identifiers, used by Alembic.
revision = "a1b2c3d4e5f6"
down_revision = "c1d2e3f4a5b6"
branch_labels = None
depends_on = None


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
    if not _table_exists("users"):
        return

    if not _column_exists("users", "rpm_limit"):
        op.add_column(
            "users",
            sa.Column(
                "rpm_limit",
                sa.Integer(),
                nullable=True,
                comment="用户级别 RPM 限制（每分钟最大请求数，NULL = 不限制）",
            ),
        )


def downgrade() -> None:
    if not _table_exists("users"):
        return

    if _column_exists("users", "rpm_limit"):
        op.drop_column("users", "rpm_limit")
