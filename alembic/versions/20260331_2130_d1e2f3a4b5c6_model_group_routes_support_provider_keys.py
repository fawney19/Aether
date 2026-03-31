"""model group routes support provider api keys

Revision ID: d1e2f3a4b5c6
Revises: aa31bc42de55
Create Date: 2026-03-31 21:30:00.000000+00:00
"""

from __future__ import annotations

from collections.abc import Sequence

import sqlalchemy as sa
from sqlalchemy import inspect

from alembic import op

# revision identifiers, used by Alembic.
revision: str = "d1e2f3a4b5c6"
down_revision: str | None = "aa31bc42de55"
branch_labels: str | Sequence[str] | None = None
depends_on: str | Sequence[str] | None = None


def _table_exists(table_name: str) -> bool:
    bind = op.get_bind()
    inspector = inspect(bind)
    return table_name in inspector.get_table_names()


def _column_exists(table_name: str, column_name: str) -> bool:
    bind = op.get_bind()
    inspector = inspect(bind)
    return any(column["name"] == column_name for column in inspector.get_columns(table_name))


def _index_exists(table_name: str, index_name: str) -> bool:
    bind = op.get_bind()
    inspector = inspect(bind)
    return any(index["name"] == index_name for index in inspector.get_indexes(table_name))


def upgrade() -> None:
    if not _table_exists("model_group_routes"):
        return

    if not _column_exists("model_group_routes", "provider_api_key_id"):
        op.add_column(
            "model_group_routes",
            sa.Column(
                "provider_api_key_id",
                sa.String(length=36),
                sa.ForeignKey("provider_api_keys.id", ondelete="CASCADE"),
                nullable=True,
            ),
        )

    if not _index_exists("model_group_routes", "ix_model_group_routes_provider_api_key_id"):
        op.create_index(
            "ix_model_group_routes_provider_api_key_id",
            "model_group_routes",
            ["provider_api_key_id"],
            unique=False,
        )


def downgrade() -> None:
    if not _table_exists("model_group_routes"):
        return

    if _index_exists("model_group_routes", "ix_model_group_routes_provider_api_key_id"):
        op.drop_index("ix_model_group_routes_provider_api_key_id", table_name="model_group_routes")

    if _column_exists("model_group_routes", "provider_api_key_id"):
        op.drop_column("model_group_routes", "provider_api_key_id")
