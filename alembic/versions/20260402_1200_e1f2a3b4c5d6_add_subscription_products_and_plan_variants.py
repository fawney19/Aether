"""add subscription products and plan variants

Revision ID: e1f2a3b4c5d6
Revises: d9e0f1a2b3c4
Create Date: 2026-04-02 12:00:00.000000+00:00
"""

from __future__ import annotations

from collections.abc import Sequence
from datetime import datetime, timezone
import uuid

import sqlalchemy as sa
from sqlalchemy import inspect

from alembic import op

# revision identifiers, used by Alembic.
revision: str = "e1f2a3b4c5d6"
down_revision: str | None = "d9e0f1a2b3c4"
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
    if not _table_exists("subscription_products"):
        op.create_table(
            "subscription_products",
            sa.Column("id", sa.String(length=36), nullable=False),
            sa.Column("code", sa.String(length=100), nullable=False),
            sa.Column("name", sa.String(length=100), nullable=False),
            sa.Column("description", sa.String(length=500), nullable=True),
            sa.Column("user_group_id", sa.String(length=36), nullable=False),
            sa.Column("plan_level", sa.Integer(), nullable=False, server_default="0"),
            sa.Column(
                "overage_policy",
                sa.String(length=30),
                nullable=False,
                server_default="block",
            ),
            sa.Column("is_active", sa.Boolean(), nullable=False, server_default=sa.true()),
            sa.Column("created_at", sa.DateTime(timezone=True), nullable=False),
            sa.Column("updated_at", sa.DateTime(timezone=True), nullable=False),
            sa.ForeignKeyConstraint(
                ["user_group_id"],
                ["user_groups.id"],
                ondelete="RESTRICT",
            ),
            sa.PrimaryKeyConstraint("id"),
            sa.UniqueConstraint("code", name="uq_subscription_products_code"),
        )
        op.create_index(
            "ix_subscription_products_code",
            "subscription_products",
            ["code"],
            unique=False,
        )
        op.create_index(
            "ix_subscription_products_user_group_id",
            "subscription_products",
            ["user_group_id"],
            unique=False,
        )

    if _table_exists("subscription_plans"):
        with op.batch_alter_table("subscription_plans") as batch_op:
            if not _column_exists("subscription_plans", "product_id"):
                batch_op.add_column(sa.Column("product_id", sa.String(length=36), nullable=True))
            if not _column_exists("subscription_plans", "variant_rank"):
                batch_op.add_column(
                    sa.Column("variant_rank", sa.Integer(), nullable=False, server_default="100")
                )
            if not _column_exists("subscription_plans", "is_default_variant"):
                batch_op.add_column(
                    sa.Column(
                        "is_default_variant",
                        sa.Boolean(),
                        nullable=False,
                        server_default=sa.false(),
                    )
                )

        bind = op.get_bind()
        plan_rows = bind.execute(
            sa.text(
                """
                SELECT id, code, name, description, user_group_id, plan_level, overage_policy, is_active, created_at, updated_at
                FROM subscription_plans
                ORDER BY created_at ASC, id ASC
                """
            )
        ).mappings().all()
        now = datetime.now(timezone.utc)
        for row in plan_rows:
            product_id = str(uuid.uuid4())
            bind.execute(
                sa.text(
                    """
                    INSERT INTO subscription_products (
                        id, code, name, description, user_group_id, plan_level, overage_policy, is_active, created_at, updated_at
                    ) VALUES (
                        :id, :code, :name, :description, :user_group_id, :plan_level, :overage_policy, :is_active, :created_at, :updated_at
                    )
                    """
                ),
                {
                    "id": product_id,
                    "code": row["code"],
                    "name": row["name"],
                    "description": row["description"],
                    "user_group_id": row["user_group_id"],
                    "plan_level": row["plan_level"] or 0,
                    "overage_policy": row["overage_policy"] or "block",
                    "is_active": bool(row["is_active"]),
                    "created_at": row["created_at"] or now,
                    "updated_at": row["updated_at"] or now,
                },
            )
            bind.execute(
                sa.text(
                    """
                    UPDATE subscription_plans
                    SET product_id = :product_id,
                        variant_rank = COALESCE(variant_rank, 100),
                        is_default_variant = TRUE
                    WHERE id = :plan_id
                    """
                ),
                {
                    "product_id": product_id,
                    "plan_id": row["id"],
                },
            )

        with op.batch_alter_table("subscription_plans") as batch_op:
            batch_op.alter_column("product_id", existing_type=sa.String(length=36), nullable=False)
            batch_op.create_foreign_key(
                "fk_subscription_plans_product_id_subscription_products",
                "subscription_products",
                ["product_id"],
                ["id"],
                ondelete="CASCADE",
            )

        if not _index_exists("subscription_plans", "ix_subscription_plans_product_id"):
            op.create_index(
                "ix_subscription_plans_product_id",
                "subscription_plans",
                ["product_id"],
                unique=False,
            )


def downgrade() -> None:
    if _table_exists("subscription_plans"):
        with op.batch_alter_table("subscription_plans") as batch_op:
            try:
                batch_op.drop_constraint(
                    "fk_subscription_plans_product_id_subscription_products",
                    type_="foreignkey",
                )
            except Exception:
                pass
        if _index_exists("subscription_plans", "ix_subscription_plans_product_id"):
            op.drop_index("ix_subscription_plans_product_id", table_name="subscription_plans")
        with op.batch_alter_table("subscription_plans") as batch_op:
            if _column_exists("subscription_plans", "is_default_variant"):
                batch_op.drop_column("is_default_variant")
            if _column_exists("subscription_plans", "variant_rank"):
                batch_op.drop_column("variant_rank")
            if _column_exists("subscription_plans", "product_id"):
                batch_op.drop_column("product_id")

    if _table_exists("subscription_products"):
        if _index_exists("subscription_products", "ix_subscription_products_user_group_id"):
            op.drop_index(
                "ix_subscription_products_user_group_id",
                table_name="subscription_products",
            )
        if _index_exists("subscription_products", "ix_subscription_products_code"):
            op.drop_index("ix_subscription_products_code", table_name="subscription_products")
        op.drop_table("subscription_products")
