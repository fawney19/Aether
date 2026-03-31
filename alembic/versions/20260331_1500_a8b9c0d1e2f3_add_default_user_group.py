"""add default user group

Revision ID: a8b9c0d1e2f3
Revises: f9a1c2e3b4d5
Create Date: 2026-03-31 15:00:00.000000+00:00
"""

from __future__ import annotations

import uuid
from collections.abc import Sequence
from datetime import datetime, timezone

import sqlalchemy as sa
from sqlalchemy import inspect

from alembic import op

# revision identifiers, used by Alembic.
revision: str = "a8b9c0d1e2f3"
down_revision: str | None = "f9a1c2e3b4d5"
branch_labels: str | Sequence[str] | None = None
depends_on: str | Sequence[str] | None = None

DEFAULT_USER_GROUP_NAME = "默认分组"
DEFAULT_USER_GROUP_DESCRIPTION = "系统默认分组，所有未显式指定分组的用户都会归入此组。"
DEFAULT_GROUP_INDEX_NAME = "ux_user_groups_single_default"


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


def upgrade() -> None:
    if not table_exists("user_groups") or not table_exists("users"):
        return

    if not column_exists("user_groups", "is_default"):
        op.add_column(
            "user_groups",
            sa.Column("is_default", sa.Boolean(), nullable=False, server_default=sa.false()),
        )
        op.alter_column("user_groups", "is_default", server_default=None)

    bind = op.get_bind()
    metadata = sa.MetaData()
    user_groups = sa.Table("user_groups", metadata, autoload_with=bind)
    users = sa.Table("users", metadata, autoload_with=bind)

    default_group = bind.execute(
        sa.select(
            user_groups.c.id,
            user_groups.c.name,
            user_groups.c.description,
        )
        .where(user_groups.c.is_default.is_(True))
        .order_by(user_groups.c.created_at.asc(), user_groups.c.id.asc())
    ).mappings().first()

    if default_group is None:
        named_group = bind.execute(
            sa.select(
                user_groups.c.id,
                user_groups.c.name,
                user_groups.c.description,
            )
            .where(user_groups.c.name == DEFAULT_USER_GROUP_NAME)
            .order_by(user_groups.c.created_at.asc(), user_groups.c.id.asc())
        ).mappings().first()

        if named_group is not None:
            default_group_id = str(named_group["id"])
            bind.execute(
                user_groups.update()
                .where(user_groups.c.id == default_group_id)
                .values(
                    is_default=True,
                    description=named_group["description"] or DEFAULT_USER_GROUP_DESCRIPTION,
                    updated_at=datetime.now(timezone.utc),
                )
            )
        else:
            now = datetime.now(timezone.utc)
            default_group_id = str(uuid.uuid4())
            bind.execute(
                user_groups.insert().values(
                    id=default_group_id,
                    name=DEFAULT_USER_GROUP_NAME,
                    description=DEFAULT_USER_GROUP_DESCRIPTION,
                    is_default=True,
                    allowed_providers=None,
                    allowed_api_formats=None,
                    allowed_models=None,
                    rate_limit=None,
                    created_at=now,
                    updated_at=now,
                )
            )
    else:
        default_group_id = str(default_group["id"])
        if not default_group["description"]:
            bind.execute(
                user_groups.update()
                .where(user_groups.c.id == default_group_id)
                .values(
                    description=DEFAULT_USER_GROUP_DESCRIPTION,
                    updated_at=datetime.now(timezone.utc),
                )
            )

    bind.execute(
        user_groups.update()
        .where(user_groups.c.id != default_group_id)
        .values(is_default=False)
    )
    bind.execute(
        users.update()
        .where(users.c.group_id.is_(None))
        .values(group_id=default_group_id)
    )

    if not index_exists("user_groups", DEFAULT_GROUP_INDEX_NAME):
        op.create_index(
            DEFAULT_GROUP_INDEX_NAME,
            "user_groups",
            ["is_default"],
            unique=True,
            postgresql_where=sa.text("is_default"),
        )


def downgrade() -> None:
    if table_exists("user_groups") and index_exists("user_groups", DEFAULT_GROUP_INDEX_NAME):
        op.drop_index(DEFAULT_GROUP_INDEX_NAME, table_name="user_groups")

    if table_exists("user_groups") and column_exists("user_groups", "is_default"):
        op.drop_column("user_groups", "is_default")
