"""drop model group route endpoint signature

Revision ID: e6f7a8b9c0d1
Revises: d1e2f3a4b5c6
Create Date: 2026-03-31 22:00:00.000000+00:00
"""

from __future__ import annotations

from collections import defaultdict
from collections.abc import Sequence

import sqlalchemy as sa
from sqlalchemy import inspect

from alembic import op

# revision identifiers, used by Alembic.
revision: str = "e6f7a8b9c0d1"
down_revision: str | None = "d1e2f3a4b5c6"
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


def _deduplicate_model_group_routes() -> None:
    bind = op.get_bind()
    rows = bind.execute(
        sa.text(
            """
            SELECT id, model_group_id, provider_id, provider_api_key_id, priority, updated_at, created_at
            FROM model_group_routes
            ORDER BY priority ASC, updated_at ASC NULLS LAST, created_at ASC NULLS LAST, id ASC
            """
        )
    ).fetchall()

    grouped: dict[tuple[str, str, str], list[str]] = defaultdict(list)
    for row in rows:
        grouped[
            (
                str(row.model_group_id),
                str(row.provider_id),
                str(row.provider_api_key_id or ""),
            )
        ].append(str(row.id))

    duplicate_ids = [route_id for ids in grouped.values() for route_id in ids[1:]]
    if duplicate_ids:
        model_group_routes = sa.table("model_group_routes", sa.column("id", sa.String(length=36)))
        bind.execute(sa.delete(model_group_routes).where(model_group_routes.c.id.in_(duplicate_ids)))


def upgrade() -> None:
    if not _table_exists("model_group_routes"):
        return

    _deduplicate_model_group_routes()

    with op.batch_alter_table("model_group_routes") as batch_op:
        batch_op.drop_constraint("uq_model_group_route", type_="unique")
        batch_op.create_unique_constraint(
            "uq_model_group_route",
            ["model_group_id", "provider_id", "provider_api_key_id"],
        )
        if _column_exists("model_group_routes", "endpoint_signature"):
            batch_op.drop_column("endpoint_signature")


def downgrade() -> None:
    if not _table_exists("model_group_routes"):
        return

    with op.batch_alter_table("model_group_routes") as batch_op:
        if not _column_exists("model_group_routes", "endpoint_signature"):
            batch_op.add_column(sa.Column("endpoint_signature", sa.String(length=50), nullable=True))
        batch_op.drop_constraint("uq_model_group_route", type_="unique")
        batch_op.create_unique_constraint(
            "uq_model_group_route",
            ["model_group_id", "provider_id", "endpoint_signature"],
        )
