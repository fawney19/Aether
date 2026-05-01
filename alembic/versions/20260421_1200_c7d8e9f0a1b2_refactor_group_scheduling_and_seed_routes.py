"""refactor group scheduling and seed routes

Revision ID: c7d8e9f0a1b2
Revises: f2a4c6e8b0d1
Create Date: 2026-04-21 12:00:00.000000+00:00
"""

from __future__ import annotations

import uuid
from collections.abc import Sequence
from datetime import datetime, timezone

import sqlalchemy as sa
from sqlalchemy import inspect

from alembic import op

# revision identifiers, used by Alembic.
revision: str = "c7d8e9f0a1b2"
down_revision: str | None = "f2a4c6e8b0d1"
branch_labels: str | Sequence[str] | None = None
depends_on: str | Sequence[str] | None = None

DEFAULT_SCHEDULING_MODE = "cache_affinity"
ALLOWED_SCHEDULING_MODES = {"cache_affinity", "load_balance", "fixed_order"}


def _table_exists(table_name: str) -> bool:
    bind = op.get_bind()
    inspector = inspect(bind)
    return table_name in inspector.get_table_names()


def _column_exists(table_name: str, column_name: str) -> bool:
    bind = op.get_bind()
    inspector = inspect(bind)
    return any(column["name"] == column_name for column in inspector.get_columns(table_name))


def upgrade() -> None:
    if _table_exists("user_groups") and not _column_exists("user_groups", "scheduling_mode"):
        op.add_column(
            "user_groups",
            sa.Column(
                "scheduling_mode",
                sa.String(length=32),
                nullable=False,
                server_default=DEFAULT_SCHEDULING_MODE,
            ),
        )
        op.alter_column("user_groups", "scheduling_mode", server_default=None)

    bind = op.get_bind()
    metadata = sa.MetaData()

    if _table_exists("user_groups") and _column_exists("user_groups", "scheduling_mode"):
        user_groups = sa.Table("user_groups", metadata, autoload_with=bind)
        global_mode = DEFAULT_SCHEDULING_MODE

        if _table_exists("system_configs"):
            system_configs = sa.Table("system_configs", metadata, autoload_with=bind)
            row = bind.execute(
                sa.select(system_configs.c.value).where(system_configs.c.key == "scheduling_mode")
            ).first()
            if row is not None:
                raw_mode = str(row[0] or "").strip().lower()
                if raw_mode in ALLOWED_SCHEDULING_MODES:
                    global_mode = raw_mode

        bind.execute(
            user_groups.update()
            .where(
                sa.or_(
                    user_groups.c.scheduling_mode.is_(None),
                    user_groups.c.scheduling_mode == "",
                )
            )
            .values(scheduling_mode=global_mode)
        )

    if not (
        _table_exists("model_groups")
        and _table_exists("providers")
        and _table_exists("model_group_routes")
    ):
        return

    model_groups = sa.Table("model_groups", metadata, autoload_with=bind)
    providers = sa.Table("providers", metadata, autoload_with=bind)
    model_group_routes = sa.Table("model_group_routes", metadata, autoload_with=bind)

    provider_rows = bind.execute(
        sa.select(
            providers.c.id,
            providers.c.provider_priority,
        ).order_by(
            sa.func.coalesce(providers.c.provider_priority, 999999).asc(),
            providers.c.id.asc(),
        )
    ).mappings().all()

    if not provider_rows:
        return

    existing_route_group_ids = {
        str(row[0])
        for row in bind.execute(sa.select(sa.distinct(model_group_routes.c.model_group_id))).all()
        if row[0] is not None
    }

    groups_without_routes = [
        str(group_id)
        for group_id, in bind.execute(sa.select(model_groups.c.id)).all()
        if str(group_id) not in existing_route_group_ids
    ]

    if not groups_without_routes:
        return

    now = datetime.now(timezone.utc)
    route_rows: list[dict[str, object]] = []
    for model_group_id in groups_without_routes:
        for index, provider_row in enumerate(provider_rows, start=1):
            route_rows.append(
                {
                    "id": str(uuid.uuid4()),
                    "model_group_id": model_group_id,
                    "provider_id": str(provider_row["id"]),
                    "provider_api_key_id": None,
                    "priority": index * 10,
                    "is_active": True,
                    "created_at": now,
                    "updated_at": now,
                }
            )

    if route_rows:
        bind.execute(model_group_routes.insert(), route_rows)

    # 物理清除已废弃的 SystemConfig 行（原为 HIDDEN_LEGACY_KEYS 软屏蔽目标）。
    if _table_exists("system_configs"):
        system_configs = sa.Table("system_configs", metadata, autoload_with=bind)
        bind.execute(
            system_configs.delete().where(
                system_configs.c.key.in_(("provider_priority_mode", "scheduling_mode"))
            )
        )

    # 删除已废弃列：model_group_routes.notes 与 model_groups.routing_mode。
    if _column_exists("model_group_routes", "notes"):
        op.drop_column("model_group_routes", "notes")
    if _column_exists("model_groups", "routing_mode"):
        op.drop_column("model_groups", "routing_mode")


def downgrade() -> None:
    if _table_exists("model_groups") and not _column_exists("model_groups", "routing_mode"):
        op.add_column(
            "model_groups",
            sa.Column(
                "routing_mode",
                sa.String(length=20),
                nullable=False,
                server_default="custom",
            ),
        )
        op.alter_column("model_groups", "routing_mode", server_default=None)

    if _table_exists("model_group_routes") and not _column_exists("model_group_routes", "notes"):
        op.add_column(
            "model_group_routes",
            sa.Column("notes", sa.String(length=500), nullable=True),
        )

    if _table_exists("user_groups") and _column_exists("user_groups", "scheduling_mode"):
        op.drop_column("user_groups", "scheduling_mode")
