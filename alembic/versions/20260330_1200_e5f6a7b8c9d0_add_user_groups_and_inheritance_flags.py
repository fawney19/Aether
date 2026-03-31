"""add user groups and inheritance flags

Revision ID: e5f6a7b8c9d0
Revises: c3d4e5f6a7b8
Create Date: 2026-03-30 12:00:00.000000+00:00
"""

from __future__ import annotations

from collections.abc import Sequence

import sqlalchemy as sa
from sqlalchemy import inspect

from alembic import op

# revision identifiers, used by Alembic.
revision: str = "e5f6a7b8c9d0"
down_revision: str | None = "c3d4e5f6a7b8"
branch_labels: str | Sequence[str] | None = None
depends_on: str | Sequence[str] | None = None


def table_exists(table_name: str) -> bool:
    bind = op.get_bind()
    inspector = inspect(bind)
    return table_name in inspector.get_table_names()


def column_exists(table_name: str, column_name: str) -> bool:
    bind = op.get_bind()
    inspector = inspect(bind)
    columns = [c["name"] for c in inspector.get_columns(table_name)]
    return column_name in columns


def index_exists(table_name: str, index_name: str) -> bool:
    bind = op.get_bind()
    inspector = inspect(bind)
    indexes = [idx["name"] for idx in inspector.get_indexes(table_name)]
    return index_name in indexes


def foreign_key_exists(table_name: str, constraint_name: str) -> bool:
    bind = op.get_bind()
    inspector = inspect(bind)
    fks = [fk["name"] for fk in inspector.get_foreign_keys(table_name)]
    return constraint_name in fks


def upgrade() -> None:
    if not table_exists("user_groups"):
        op.create_table(
            "user_groups",
            sa.Column("id", sa.String(length=36), nullable=False),
            sa.Column("name", sa.String(length=100), nullable=False),
            sa.Column("description", sa.String(length=500), nullable=True),
            sa.Column("allowed_providers", sa.JSON(), nullable=True),
            sa.Column("allowed_api_formats", sa.JSON(), nullable=True),
            sa.Column("allowed_models", sa.JSON(), nullable=True),
            sa.Column("rate_limit", sa.Integer(), nullable=True),
            sa.Column("created_at", sa.DateTime(timezone=True), nullable=False),
            sa.Column("updated_at", sa.DateTime(timezone=True), nullable=False),
            sa.PrimaryKeyConstraint("id"),
            sa.UniqueConstraint("name", name="uq_user_groups_name"),
        )

    if not index_exists("user_groups", "ix_user_groups_name"):
        op.create_index("ix_user_groups_name", "user_groups", ["name"], unique=False)

    if not column_exists("users", "group_id"):
        op.add_column("users", sa.Column("group_id", sa.String(length=36), nullable=True))
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

    if not index_exists("users", "ix_users_group_id"):
        op.create_index("ix_users_group_id", "users", ["group_id"], unique=False)

    if not foreign_key_exists("users", "fk_users_group_id_user_groups"):
        op.create_foreign_key(
            "fk_users_group_id_user_groups",
            "users",
            "user_groups",
            ["group_id"],
            ["id"],
            ondelete="SET NULL",
        )

    op.alter_column("users", "inherit_group_allowed_providers", server_default=None)
    op.alter_column("users", "inherit_group_allowed_api_formats", server_default=None)
    op.alter_column("users", "inherit_group_allowed_models", server_default=None)
    op.alter_column("users", "inherit_group_rate_limit", server_default=None)


def downgrade() -> None:
    if foreign_key_exists("users", "fk_users_group_id_user_groups"):
        op.drop_constraint("fk_users_group_id_user_groups", "users", type_="foreignkey")

    if index_exists("users", "ix_users_group_id"):
        op.drop_index("ix_users_group_id", table_name="users")

    for column_name in (
        "inherit_group_rate_limit",
        "inherit_group_allowed_models",
        "inherit_group_allowed_api_formats",
        "inherit_group_allowed_providers",
        "group_id",
    ):
        if column_exists("users", column_name):
            op.drop_column("users", column_name)

    if table_exists("user_groups"):
        if index_exists("user_groups", "ix_user_groups_name"):
            op.drop_index("ix_user_groups_name", table_name="user_groups")
        op.drop_table("user_groups")
