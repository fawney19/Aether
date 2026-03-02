"""add_proxy_metadata_to_proxy_nodes

Revision ID: 1d2e3f4a5b6c
Revises: f0c3a7b9d1e2
Create Date: 2026-03-02 13:00:00.000000+00:00
"""

from __future__ import annotations

from collections.abc import Sequence

import sqlalchemy as sa
from sqlalchemy import inspect

from alembic import op

# revision identifiers, used by Alembic.
revision: str = "1d2e3f4a5b6c"
down_revision: str | None = "f0c3a7b9d1e2"
branch_labels: str | Sequence[str] | None = None
depends_on: str | Sequence[str] | None = None


def _column_exists(table_name: str, column_name: str) -> bool:
    bind = op.get_bind()
    insp = inspect(bind)
    columns = [c["name"] for c in insp.get_columns(table_name)]
    return column_name in columns


def upgrade() -> None:
    if not _column_exists("proxy_nodes", "proxy_metadata"):
        op.add_column(
            "proxy_nodes",
            sa.Column(
                "proxy_metadata",
                sa.JSON(),
                nullable=True,
                comment="aether-proxy 上报元数据（版本等）",
            ),
        )


def downgrade() -> None:
    if _column_exists("proxy_nodes", "proxy_metadata"):
        op.drop_column("proxy_nodes", "proxy_metadata")
