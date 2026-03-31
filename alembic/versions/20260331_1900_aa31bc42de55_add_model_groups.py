"""add model groups

Revision ID: aa31bc42de55
Revises: a8b9c0d1e2f3
Create Date: 2026-03-31 19:00:00.000000+00:00
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
revision: str = "aa31bc42de55"
down_revision: str | None = "a8b9c0d1e2f3"
branch_labels: str | Sequence[str] | None = None
depends_on: str | Sequence[str] | None = None

DEFAULT_MODEL_GROUP_NAME = "default"
DEFAULT_MODEL_GROUP_DISPLAY_NAME = "默认模型分组"
DEFAULT_MODEL_GROUP_DESCRIPTION = "系统默认模型分组，供默认用户分组与新建分组按需绑定。"
DEFAULT_MODEL_GROUP_INDEX_NAME = "ux_model_groups_single_default"


def table_exists(table_name: str) -> bool:
    bind = op.get_bind()
    inspector = inspect(bind)
    return table_name in inspector.get_table_names()


def column_exists(table_name: str, column_name: str) -> bool:
    bind = op.get_bind()
    inspector = inspect(bind)
    columns = [column["name"] for column in inspector.get_columns(table_name)]
    return column_name in columns


def index_exists(table_name: str, index_name: str) -> bool:
    bind = op.get_bind()
    inspector = inspect(bind)
    indexes = [index["name"] for index in inspector.get_indexes(table_name)]
    return index_name in indexes


def _normalize_list(values: Any) -> Any:
    if values is None or not isinstance(values, list):
        return values
    normalized = sorted({str(item).strip() for item in values if str(item).strip()})
    return normalized


def _legacy_signature(allowed_providers: Any, allowed_models: Any) -> str:
    return json.dumps(
        {
            "allowed_providers": _normalize_list(allowed_providers),
            "allowed_models": _normalize_list(allowed_models),
        },
        ensure_ascii=False,
        sort_keys=True,
    )


def _next_migrated_model_group_name(existing_names: set[str]) -> str:
    index = 1
    while True:
        candidate = f"迁移模型分组 {index}"
        if candidate not in existing_names:
            existing_names.add(candidate)
            return candidate
        index += 1


def _create_model_group_tables() -> None:
    if not table_exists("model_groups"):
        op.create_table(
            "model_groups",
            sa.Column("id", sa.String(length=36), nullable=False),
            sa.Column("name", sa.String(length=100), nullable=False),
            sa.Column("display_name", sa.String(length=100), nullable=False),
            sa.Column("description", sa.String(length=500), nullable=True),
            sa.Column(
                "default_user_billing_multiplier",
                sa.Numeric(10, 4),
                nullable=False,
                server_default="1.0",
            ),
            sa.Column("routing_mode", sa.String(length=20), nullable=False, server_default="inherit"),
            sa.Column("is_default", sa.Boolean(), nullable=False, server_default=sa.false()),
            sa.Column("is_active", sa.Boolean(), nullable=False, server_default=sa.true()),
            sa.Column("sort_order", sa.Integer(), nullable=False, server_default="100"),
            sa.Column("created_at", sa.DateTime(timezone=True), nullable=False),
            sa.Column("updated_at", sa.DateTime(timezone=True), nullable=False),
            sa.PrimaryKeyConstraint("id"),
            sa.UniqueConstraint("name", name="uq_model_groups_name"),
        )
        op.create_index("ix_model_groups_name", "model_groups", ["name"], unique=False)
        op.create_index("ix_model_groups_is_active", "model_groups", ["is_active"], unique=False)

    if not index_exists("model_groups", DEFAULT_MODEL_GROUP_INDEX_NAME):
        op.create_index(
            DEFAULT_MODEL_GROUP_INDEX_NAME,
            "model_groups",
            ["is_default"],
            unique=True,
            postgresql_where=sa.text("is_default"),
        )

    if not table_exists("model_group_models"):
        op.create_table(
            "model_group_models",
            sa.Column("id", sa.String(length=36), nullable=False),
            sa.Column("model_group_id", sa.String(length=36), nullable=False),
            sa.Column("global_model_id", sa.String(length=36), nullable=False),
            sa.Column("created_at", sa.DateTime(timezone=True), nullable=False),
            sa.Column("updated_at", sa.DateTime(timezone=True), nullable=False),
            sa.ForeignKeyConstraint(["global_model_id"], ["global_models.id"], ondelete="CASCADE"),
            sa.ForeignKeyConstraint(["model_group_id"], ["model_groups.id"], ondelete="CASCADE"),
            sa.PrimaryKeyConstraint("id"),
            sa.UniqueConstraint("model_group_id", "global_model_id", name="uq_model_group_model"),
        )
        op.create_index(
            "ix_model_group_models_model_group_id",
            "model_group_models",
            ["model_group_id"],
            unique=False,
        )
        op.create_index(
            "ix_model_group_models_global_model_id",
            "model_group_models",
            ["global_model_id"],
            unique=False,
        )

    if not table_exists("model_group_routes"):
        op.create_table(
            "model_group_routes",
            sa.Column("id", sa.String(length=36), nullable=False),
            sa.Column("model_group_id", sa.String(length=36), nullable=False),
            sa.Column("provider_id", sa.String(length=36), nullable=False),
            sa.Column("endpoint_signature", sa.String(length=50), nullable=True),
            sa.Column("priority", sa.Integer(), nullable=False, server_default="50"),
            sa.Column("user_billing_multiplier_override", sa.Numeric(10, 4), nullable=True),
            sa.Column("is_active", sa.Boolean(), nullable=False, server_default=sa.true()),
            sa.Column("notes", sa.String(length=500), nullable=True),
            sa.Column("created_at", sa.DateTime(timezone=True), nullable=False),
            sa.Column("updated_at", sa.DateTime(timezone=True), nullable=False),
            sa.ForeignKeyConstraint(["model_group_id"], ["model_groups.id"], ondelete="CASCADE"),
            sa.ForeignKeyConstraint(["provider_id"], ["providers.id"], ondelete="CASCADE"),
            sa.PrimaryKeyConstraint("id"),
            sa.UniqueConstraint(
                "model_group_id",
                "provider_id",
                "endpoint_signature",
                name="uq_model_group_route",
            ),
        )
        op.create_index(
            "ix_model_group_routes_model_group_id",
            "model_group_routes",
            ["model_group_id"],
            unique=False,
        )
        op.create_index(
            "ix_model_group_routes_provider_id",
            "model_group_routes",
            ["provider_id"],
            unique=False,
        )
        op.create_index(
            "idx_model_group_routes_priority",
            "model_group_routes",
            ["model_group_id", "priority"],
            unique=False,
        )

    if not table_exists("user_group_model_groups"):
        op.create_table(
            "user_group_model_groups",
            sa.Column("id", sa.String(length=36), nullable=False),
            sa.Column("user_group_id", sa.String(length=36), nullable=False),
            sa.Column("model_group_id", sa.String(length=36), nullable=False),
            sa.Column("priority", sa.Integer(), nullable=False, server_default="100"),
            sa.Column("is_active", sa.Boolean(), nullable=False, server_default=sa.true()),
            sa.Column("created_at", sa.DateTime(timezone=True), nullable=False),
            sa.Column("updated_at", sa.DateTime(timezone=True), nullable=False),
            sa.ForeignKeyConstraint(["model_group_id"], ["model_groups.id"], ondelete="CASCADE"),
            sa.ForeignKeyConstraint(["user_group_id"], ["user_groups.id"], ondelete="CASCADE"),
            sa.PrimaryKeyConstraint("id"),
            sa.UniqueConstraint("user_group_id", "model_group_id", name="uq_user_group_model_group"),
        )
        op.create_index(
            "ix_user_group_model_groups_user_group_id",
            "user_group_model_groups",
            ["user_group_id"],
            unique=False,
        )
        op.create_index(
            "ix_user_group_model_groups_model_group_id",
            "user_group_model_groups",
            ["model_group_id"],
            unique=False,
        )


def _add_usage_columns() -> None:
    if table_exists("usage") and not column_exists("usage", "model_group_id"):
        op.add_column(
            "usage",
            sa.Column(
                "model_group_id",
                sa.String(length=36),
                sa.ForeignKey("model_groups.id", ondelete="SET NULL"),
                nullable=True,
            ),
        )
        op.create_index("ix_usage_model_group_id", "usage", ["model_group_id"], unique=False)

    if table_exists("usage") and not column_exists("usage", "model_group_route_id"):
        op.add_column(
            "usage",
            sa.Column(
                "model_group_route_id",
                sa.String(length=36),
                sa.ForeignKey("model_group_routes.id", ondelete="SET NULL"),
                nullable=True,
            ),
        )
        op.create_index(
            "ix_usage_model_group_route_id",
            "usage",
            ["model_group_route_id"],
            unique=False,
        )

    if table_exists("usage") and not column_exists("usage", "user_billing_multiplier"):
        op.add_column(
            "usage",
            sa.Column(
                "user_billing_multiplier",
                sa.Numeric(10, 4),
                nullable=False,
                server_default="1.0",
            ),
        )
        op.alter_column("usage", "user_billing_multiplier", server_default=None)


def _ensure_default_model_group(
    bind: sa.engine.Connection,
    model_groups: sa.Table,
) -> str:
    existing = bind.execute(
        sa.select(model_groups.c.id, model_groups.c.description)
        .where(model_groups.c.is_default.is_(True))
        .order_by(model_groups.c.sort_order.asc(), model_groups.c.created_at.asc(), model_groups.c.id.asc())
    ).mappings().first()

    if existing is None:
        named = bind.execute(
            sa.select(model_groups.c.id, model_groups.c.description)
            .where(model_groups.c.name == DEFAULT_MODEL_GROUP_NAME)
            .order_by(model_groups.c.created_at.asc(), model_groups.c.id.asc())
        ).mappings().first()
        if named is not None:
            default_group_id = str(named["id"])
            bind.execute(
                model_groups.update()
                .where(model_groups.c.id == default_group_id)
                .values(
                    display_name=DEFAULT_MODEL_GROUP_DISPLAY_NAME,
                    description=named["description"] or DEFAULT_MODEL_GROUP_DESCRIPTION,
                    routing_mode="inherit",
                    default_user_billing_multiplier=1.0,
                    is_default=True,
                    is_active=True,
                    sort_order=0,
                    updated_at=datetime.now(timezone.utc),
                )
            )
        else:
            now = datetime.now(timezone.utc)
            default_group_id = str(uuid.uuid4())
            bind.execute(
                model_groups.insert().values(
                    id=default_group_id,
                    name=DEFAULT_MODEL_GROUP_NAME,
                    display_name=DEFAULT_MODEL_GROUP_DISPLAY_NAME,
                    description=DEFAULT_MODEL_GROUP_DESCRIPTION,
                    default_user_billing_multiplier=1.0,
                    routing_mode="inherit",
                    is_default=True,
                    is_active=True,
                    sort_order=0,
                    created_at=now,
                    updated_at=now,
                )
            )
    else:
        default_group_id = str(existing["id"])
        if not existing["description"]:
            bind.execute(
                model_groups.update()
                .where(model_groups.c.id == default_group_id)
                .values(
                    description=DEFAULT_MODEL_GROUP_DESCRIPTION,
                    updated_at=datetime.now(timezone.utc),
                )
            )

    bind.execute(
        model_groups.update()
        .where(model_groups.c.id != default_group_id)
        .values(is_default=False)
    )
    return default_group_id


def _migrate_user_groups_to_model_groups() -> None:
    if not table_exists("user_groups") or not table_exists("global_models"):
        return
    if not column_exists("user_groups", "allowed_providers") or not column_exists(
        "user_groups", "allowed_models"
    ):
        return

    bind = op.get_bind()
    metadata = sa.MetaData()
    user_groups = sa.Table("user_groups", metadata, autoload_with=bind)
    users = sa.Table("users", metadata, autoload_with=bind)
    global_models = sa.Table("global_models", metadata, autoload_with=bind)
    providers = sa.Table("providers", metadata, autoload_with=bind)
    model_groups = sa.Table("model_groups", metadata, autoload_with=bind)
    model_group_models = sa.Table("model_group_models", metadata, autoload_with=bind)
    model_group_routes = sa.Table("model_group_routes", metadata, autoload_with=bind)
    user_group_model_groups = sa.Table("user_group_model_groups", metadata, autoload_with=bind)
    user_counts = (
        sa.select(
            users.c.group_id.label("group_id"),
            sa.func.count(users.c.id).label("user_count"),
        )
        .group_by(users.c.group_id)
        .subquery()
    )

    default_model_group_id = _ensure_default_model_group(bind, model_groups)

    active_model_rows = bind.execute(
        sa.select(global_models.c.id, global_models.c.name).where(global_models.c.is_active.is_(True))
    ).mappings().all()
    all_active_model_ids = [str(row["id"]) for row in active_model_rows]
    model_name_to_id = {str(row["name"]): str(row["id"]) for row in active_model_rows}
    provider_priorities = {
        str(row["id"]): int(row["provider_priority"] or 100)
        for row in bind.execute(
            sa.select(providers.c.id, providers.c.provider_priority)
        ).mappings().all()
    }

    existing_model_group_names = {
        str(name)
        for (name,) in bind.execute(sa.select(model_groups.c.name)).all()
    }
    signature_to_model_group_id: dict[str, str] = {}

    group_rows = bind.execute(
        sa.select(
            user_groups.c.id,
            user_groups.c.name,
            user_groups.c.description,
            user_groups.c.allowed_providers,
            user_groups.c.allowed_models,
            user_groups.c.is_default,
            user_groups.c.created_at,
            user_groups.c.updated_at,
            sa.func.coalesce(user_counts.c.user_count, 0).label("user_count"),
        )
        .outerjoin(user_counts, user_counts.c.group_id == user_groups.c.id)
    ).mappings().all()

    for row in group_rows:
        allowed_providers = row["allowed_providers"]
        allowed_models = row["allowed_models"]
        user_count = int(row["user_count"] or 0)

        if (
            bool(row["is_default"])
            and allowed_providers is None
            and allowed_models is None
            and user_count == 0
        ):
            target_model_group_id = default_model_group_id
        else:
            signature = _legacy_signature(allowed_providers, allowed_models)
            target_model_group_id = signature_to_model_group_id.get(signature)
            if target_model_group_id is None:
                now = datetime.now(timezone.utc)
                target_model_group_id = str(uuid.uuid4())
                routing_mode = "custom" if isinstance(allowed_providers, list) else "inherit"
                bind.execute(
                    model_groups.insert().values(
                        id=target_model_group_id,
                        name=_next_migrated_model_group_name(existing_model_group_names),
                        display_name=f"{row['name']} 迁移模型分组",
                        description="由旧版用户分组访问限制迁移生成",
                        default_user_billing_multiplier=1.0,
                        routing_mode=routing_mode,
                        is_default=False,
                        is_active=True,
                        sort_order=999,
                        created_at=now,
                        updated_at=now,
                    )
                )

                if allowed_models is None:
                    model_ids = list(all_active_model_ids)
                else:
                    model_ids = []
                    for model_name in allowed_models if isinstance(allowed_models, list) else []:
                        normalized_name = str(model_name or "").strip()
                        model_id = model_name_to_id.get(normalized_name)
                        if model_id is not None:
                            model_ids.append(model_id)

                for model_id in model_ids:
                    bind.execute(
                        model_group_models.insert().values(
                            id=str(uuid.uuid4()),
                            model_group_id=target_model_group_id,
                            global_model_id=model_id,
                            created_at=now,
                            updated_at=now,
                        )
                    )

                if isinstance(allowed_providers, list):
                    for raw_provider_id in allowed_providers:
                        provider_id = str(raw_provider_id or "").strip()
                        if not provider_id or provider_id not in provider_priorities:
                            continue
                        bind.execute(
                            model_group_routes.insert().values(
                                id=str(uuid.uuid4()),
                                model_group_id=target_model_group_id,
                                provider_id=provider_id,
                                endpoint_signature=None,
                                priority=int(provider_priorities[provider_id]),
                                user_billing_multiplier_override=None,
                                is_active=True,
                                notes="由旧版用户分组访问限制迁移生成",
                                created_at=now,
                                updated_at=now,
                            )
                        )

                signature_to_model_group_id[signature] = target_model_group_id

        existing_binding = bind.execute(
            sa.select(user_group_model_groups.c.id)
            .where(
                user_group_model_groups.c.user_group_id == row["id"],
                user_group_model_groups.c.model_group_id == target_model_group_id,
            )
        ).first()
        if existing_binding is None:
            bind.execute(
                user_group_model_groups.insert().values(
                    id=str(uuid.uuid4()),
                    user_group_id=row["id"],
                    model_group_id=target_model_group_id,
                    priority=100,
                    is_active=True,
                    created_at=datetime.now(timezone.utc),
                    updated_at=datetime.now(timezone.utc),
                )
            )

    unbound_group_ids = [
        str(group_id)
        for (group_id,) in bind.execute(
            sa.select(user_groups.c.id)
            .where(
                ~sa.exists(
                    sa.select(user_group_model_groups.c.id).where(
                        user_group_model_groups.c.user_group_id == user_groups.c.id
                    )
                )
            )
        ).all()
    ]
    for group_id in unbound_group_ids:
        bind.execute(
            user_group_model_groups.insert().values(
                id=str(uuid.uuid4()),
                user_group_id=group_id,
                model_group_id=default_model_group_id,
                priority=100,
                is_active=True,
                created_at=datetime.now(timezone.utc),
                updated_at=datetime.now(timezone.utc),
            )
        )


def upgrade() -> None:
    _create_model_group_tables()
    _add_usage_columns()
    _migrate_user_groups_to_model_groups()

    if table_exists("user_groups") and column_exists("user_groups", "allowed_providers"):
        op.drop_column("user_groups", "allowed_providers")
    if table_exists("user_groups") and column_exists("user_groups", "allowed_models"):
        op.drop_column("user_groups", "allowed_models")


def downgrade() -> None:
    if table_exists("user_groups") and not column_exists("user_groups", "allowed_providers"):
        op.add_column("user_groups", sa.Column("allowed_providers", sa.JSON(), nullable=True))
    if table_exists("user_groups") and not column_exists("user_groups", "allowed_models"):
        op.add_column("user_groups", sa.Column("allowed_models", sa.JSON(), nullable=True))

    bind = op.get_bind()
    if table_exists("user_groups") and table_exists("model_groups") and table_exists("user_group_model_groups"):
        metadata = sa.MetaData()
        user_groups = sa.Table("user_groups", metadata, autoload_with=bind)
        global_models = sa.Table("global_models", metadata, autoload_with=bind)
        model_group_models = sa.Table("model_group_models", metadata, autoload_with=bind)
        model_group_routes = sa.Table("model_group_routes", metadata, autoload_with=bind)
        user_group_model_groups = sa.Table("user_group_model_groups", metadata, autoload_with=bind)

        bindings = bind.execute(
            sa.select(
                user_group_model_groups.c.user_group_id,
                user_group_model_groups.c.model_group_id,
                user_group_model_groups.c.priority,
            )
            .order_by(
                user_group_model_groups.c.user_group_id.asc(),
                user_group_model_groups.c.priority.asc(),
                user_group_model_groups.c.created_at.asc(),
            )
        ).mappings().all()

        first_binding_by_group: dict[str, str] = {}
        for row in bindings:
            first_binding_by_group.setdefault(str(row["user_group_id"]), str(row["model_group_id"]))

        for user_group_id, model_group_id in first_binding_by_group.items():
            model_names = [
                str(name)
                for (name,) in bind.execute(
                    sa.select(global_models.c.name)
                    .join(model_group_models, model_group_models.c.global_model_id == global_models.c.id)
                    .where(model_group_models.c.model_group_id == model_group_id)
                    .order_by(global_models.c.name.asc())
                ).all()
            ]
            provider_ids = [
                str(provider_id)
                for (provider_id,) in bind.execute(
                    sa.select(model_group_routes.c.provider_id)
                    .where(model_group_routes.c.model_group_id == model_group_id)
                    .order_by(model_group_routes.c.priority.asc(), model_group_routes.c.provider_id.asc())
                ).all()
            ]
            bind.execute(
                user_groups.update()
                .where(user_groups.c.id == user_group_id)
                .values(
                    allowed_models=model_names or None,
                    allowed_providers=provider_ids or None,
                )
            )

    if table_exists("usage") and column_exists("usage", "user_billing_multiplier"):
        op.drop_column("usage", "user_billing_multiplier")
    if table_exists("usage") and column_exists("usage", "model_group_route_id"):
        op.drop_index("ix_usage_model_group_route_id", table_name="usage")
        op.drop_column("usage", "model_group_route_id")
    if table_exists("usage") and column_exists("usage", "model_group_id"):
        op.drop_index("ix_usage_model_group_id", table_name="usage")
        op.drop_column("usage", "model_group_id")

    if table_exists("user_group_model_groups"):
        op.drop_index("ix_user_group_model_groups_model_group_id", table_name="user_group_model_groups")
        op.drop_index("ix_user_group_model_groups_user_group_id", table_name="user_group_model_groups")
        op.drop_table("user_group_model_groups")

    if table_exists("model_group_routes"):
        op.drop_index("idx_model_group_routes_priority", table_name="model_group_routes")
        op.drop_index("ix_model_group_routes_provider_id", table_name="model_group_routes")
        op.drop_index("ix_model_group_routes_model_group_id", table_name="model_group_routes")
        op.drop_table("model_group_routes")

    if table_exists("model_group_models"):
        op.drop_index("ix_model_group_models_global_model_id", table_name="model_group_models")
        op.drop_index("ix_model_group_models_model_group_id", table_name="model_group_models")
        op.drop_table("model_group_models")

    if table_exists("model_groups"):
        if index_exists("model_groups", DEFAULT_MODEL_GROUP_INDEX_NAME):
            op.drop_index(DEFAULT_MODEL_GROUP_INDEX_NAME, table_name="model_groups")
        if index_exists("model_groups", "ix_model_groups_is_active"):
            op.drop_index("ix_model_groups_is_active", table_name="model_groups")
        if index_exists("model_groups", "ix_model_groups_name"):
            op.drop_index("ix_model_groups_name", table_name="model_groups")
        op.drop_table("model_groups")
