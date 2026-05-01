"""remove user level access limits

Revision ID: f9a1c2e3b4d5
Revises: e5f6a7b8c9d0
Create Date: 2026-03-31 09:00:00.000000+00:00
"""

from __future__ import annotations

import json
import uuid
from collections.abc import Sequence
from datetime import datetime, timezone
from typing import Any

import sqlalchemy as sa
from sqlalchemy import inspect

from alembic import op

# revision identifiers, used by Alembic.
revision: str = "f9a1c2e3b4d5"
down_revision: str | None = "e5f6a7b8c9d0"
branch_labels: str | Sequence[str] | None = None
depends_on: str | Sequence[str] | None = None

_LEGACY_USER_COLUMNS = (
    "allowed_providers",
    "allowed_api_formats",
    "allowed_models",
    "rate_limit",
    "inherit_group_allowed_providers",
    "inherit_group_allowed_api_formats",
    "inherit_group_allowed_models",
    "inherit_group_rate_limit",
)


def table_exists(table_name: str) -> bool:
    bind = op.get_bind()
    inspector = inspect(bind)
    return table_name in inspector.get_table_names()


def column_exists(table_name: str, column_name: str) -> bool:
    bind = op.get_bind()
    inspector = inspect(bind)
    columns = [column["name"] for column in inspector.get_columns(table_name)]
    return column_name in columns


def _config_signature(
    allowed_providers: Any,
    allowed_api_formats: Any,
    allowed_models: Any,
    rate_limit: Any,
) -> str:
    return json.dumps(
        {
            "allowed_providers": allowed_providers,
            "allowed_api_formats": allowed_api_formats,
            "allowed_models": allowed_models,
            "rate_limit": rate_limit,
        },
        ensure_ascii=False,
        sort_keys=True,
    )


def _next_migrated_group_name(existing_names: set[str]) -> str:
    index = 1
    while True:
        candidate = f"迁移分组 {index}"
        if candidate not in existing_names:
            existing_names.add(candidate)
            return candidate
        index += 1


def _migrate_effective_user_limits_to_groups() -> None:
    bind = op.get_bind()
    if not table_exists("users") or not table_exists("user_groups"):
        return
    if not all(column_exists("users", column_name) for column_name in _LEGACY_USER_COLUMNS):
        return

    metadata = sa.MetaData()
    users = sa.Table("users", metadata, autoload_with=bind)
    user_groups = sa.Table("user_groups", metadata, autoload_with=bind)

    group_rows = bind.execute(
        sa.select(
            user_groups.c.id,
            user_groups.c.name,
            user_groups.c.description,
            user_groups.c.allowed_providers,
            user_groups.c.allowed_api_formats,
            user_groups.c.allowed_models,
            user_groups.c.rate_limit,
            user_groups.c.created_at,
            user_groups.c.updated_at,
        )
    ).mappings().all()

    groups_by_id: dict[str, dict[str, Any]] = {}
    groups_by_signature: dict[str, dict[str, Any]] = {}
    existing_names: set[str] = set()

    for row in group_rows:
        item = dict(row)
        groups_by_id[item["id"]] = item
        groups_by_signature[
            _config_signature(
                item.get("allowed_providers"),
                item.get("allowed_api_formats"),
                item.get("allowed_models"),
                item.get("rate_limit"),
            )
        ] = item
        existing_names.add(str(item["name"]))

    user_rows = bind.execute(
        sa.select(
            users.c.id,
            users.c.group_id,
            users.c.allowed_providers,
            users.c.allowed_api_formats,
            users.c.allowed_models,
            users.c.rate_limit,
            users.c.inherit_group_allowed_providers,
            users.c.inherit_group_allowed_api_formats,
            users.c.inherit_group_allowed_models,
            users.c.inherit_group_rate_limit,
        )
    ).mappings().all()

    for row in user_rows:
        current_group = groups_by_id.get(row["group_id"])

        effective_allowed_providers = (
            current_group.get("allowed_providers")
            if row["inherit_group_allowed_providers"] and current_group is not None
            else row["allowed_providers"]
        )
        effective_allowed_api_formats = (
            current_group.get("allowed_api_formats")
            if row["inherit_group_allowed_api_formats"] and current_group is not None
            else row["allowed_api_formats"]
        )
        effective_allowed_models = (
            current_group.get("allowed_models")
            if row["inherit_group_allowed_models"] and current_group is not None
            else row["allowed_models"]
        )
        effective_rate_limit = (
            current_group.get("rate_limit")
            if row["inherit_group_rate_limit"] and current_group is not None
            else row["rate_limit"]
        )

        effective_signature = _config_signature(
            effective_allowed_providers,
            effective_allowed_api_formats,
            effective_allowed_models,
            effective_rate_limit,
        )

        current_signature = (
            _config_signature(
                current_group.get("allowed_providers"),
                current_group.get("allowed_api_formats"),
                current_group.get("allowed_models"),
                current_group.get("rate_limit"),
            )
            if current_group is not None
            else None
        )

        if current_signature == effective_signature:
            continue

        if (
            row["group_id"] is None
            and effective_allowed_providers is None
            and effective_allowed_api_formats is None
            and effective_allowed_models is None
            and effective_rate_limit is None
        ):
            continue

        target_group = groups_by_signature.get(effective_signature)
        if target_group is None:
            now = datetime.now(timezone.utc)
            target_group = {
                "id": str(uuid.uuid4()),
                "name": _next_migrated_group_name(existing_names),
                "description": "由用户级访问限制自动迁移生成",
                "allowed_providers": effective_allowed_providers,
                "allowed_api_formats": effective_allowed_api_formats,
                "allowed_models": effective_allowed_models,
                "rate_limit": effective_rate_limit,
                "created_at": now,
                "updated_at": now,
            }
            bind.execute(user_groups.insert().values(**target_group))
            groups_by_id[target_group["id"]] = target_group
            groups_by_signature[effective_signature] = target_group

        if row["group_id"] != target_group["id"]:
            bind.execute(
                users.update().where(users.c.id == row["id"]).values(group_id=target_group["id"])
            )


def upgrade() -> None:
    _migrate_effective_user_limits_to_groups()

    for column_name in _LEGACY_USER_COLUMNS:
        if column_exists("users", column_name):
            op.drop_column("users", column_name)


def downgrade() -> None:
    if not column_exists("users", "allowed_providers"):
        op.add_column("users", sa.Column("allowed_providers", sa.JSON(), nullable=True))
    if not column_exists("users", "allowed_api_formats"):
        op.add_column("users", sa.Column("allowed_api_formats", sa.JSON(), nullable=True))
    if not column_exists("users", "allowed_models"):
        op.add_column("users", sa.Column("allowed_models", sa.JSON(), nullable=True))
    if not column_exists("users", "rate_limit"):
        op.add_column("users", sa.Column("rate_limit", sa.Integer(), nullable=True))
    if not column_exists("users", "inherit_group_allowed_providers"):
        op.add_column(
            "users",
            sa.Column(
                "inherit_group_allowed_providers",
                sa.Boolean(),
                nullable=False,
                server_default=sa.false(),
            ),
        )
    if not column_exists("users", "inherit_group_allowed_api_formats"):
        op.add_column(
            "users",
            sa.Column(
                "inherit_group_allowed_api_formats",
                sa.Boolean(),
                nullable=False,
                server_default=sa.false(),
            ),
        )
    if not column_exists("users", "inherit_group_allowed_models"):
        op.add_column(
            "users",
            sa.Column(
                "inherit_group_allowed_models",
                sa.Boolean(),
                nullable=False,
                server_default=sa.false(),
            ),
        )
    if not column_exists("users", "inherit_group_rate_limit"):
        op.add_column(
            "users",
            sa.Column(
                "inherit_group_rate_limit",
                sa.Boolean(),
                nullable=False,
                server_default=sa.false(),
            ),
        )

    bind = op.get_bind()
    if table_exists("users") and column_exists("users", "group_id") and table_exists("user_groups"):
        metadata = sa.MetaData()
        users = sa.Table("users", metadata, autoload_with=bind)
        bind.execute(
            users.update()
            .where(users.c.group_id.is_not(None))
            .values(
                inherit_group_allowed_providers=True,
                inherit_group_allowed_api_formats=True,
                inherit_group_allowed_models=True,
                inherit_group_rate_limit=True,
            )
        )

    op.alter_column("users", "inherit_group_allowed_providers", server_default=None)
    op.alter_column("users", "inherit_group_allowed_api_formats", server_default=None)
    op.alter_column("users", "inherit_group_allowed_models", server_default=None)
    op.alter_column("users", "inherit_group_rate_limit", server_default=None)
