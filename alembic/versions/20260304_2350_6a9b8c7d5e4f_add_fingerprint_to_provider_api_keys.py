"""add fingerprint column to provider_api_keys

Revision ID: 6a9b8c7d5e4f
Revises: 5f1d2e3c4b5a
Create Date: 2026-03-04 23:50:00.000000
"""

from __future__ import annotations

from collections.abc import Sequence

import sqlalchemy as sa
from sqlalchemy import inspect

from alembic import op

# revision identifiers, used by Alembic.
revision: str = "6a9b8c7d5e4f"
down_revision: str | None = "5f1d2e3c4b5a"
branch_labels: str | Sequence[str] | None = None
depends_on: str | Sequence[str] | None = None


def column_exists(table_name: str, column_name: str) -> bool:
    bind = op.get_bind()
    inspector = inspect(bind)
    columns = [c["name"] for c in inspector.get_columns(table_name)]
    return column_name in columns


def upgrade() -> None:
    if not column_exists("provider_api_keys", "fingerprint"):
        op.add_column(
            "provider_api_keys",
            sa.Column("fingerprint", sa.JSON(), nullable=True),
        )


def downgrade() -> None:
    if column_exists("provider_api_keys", "fingerprint"):
        op.drop_column("provider_api_keys", "fingerprint")
