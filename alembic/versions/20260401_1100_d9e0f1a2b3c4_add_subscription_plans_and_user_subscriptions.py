"""add subscription plans and user subscriptions

Revision ID: d9e0f1a2b3c4
Revises: c4f5a6b7d8e9
Create Date: 2026-04-01 11:00:00.000000+00:00
"""

from __future__ import annotations

from collections.abc import Sequence

import sqlalchemy as sa
from sqlalchemy import inspect
from sqlalchemy.dialects import postgresql

from alembic import op

# revision identifiers, used by Alembic.
revision: str = "d9e0f1a2b3c4"
down_revision: str | None = "c4f5a6b7d8e9"
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
    if not _table_exists("subscription_plans"):
        op.create_table(
            "subscription_plans",
            sa.Column("id", sa.String(length=36), nullable=False),
            sa.Column("code", sa.String(length=100), nullable=False),
            sa.Column("name", sa.String(length=100), nullable=False),
            sa.Column("description", sa.String(length=500), nullable=True),
            sa.Column("user_group_id", sa.String(length=36), nullable=False),
            sa.Column("plan_level", sa.Integer(), nullable=False, server_default="0"),
            sa.Column(
                "monthly_price_usd",
                sa.Numeric(20, 8),
                nullable=False,
                server_default="0",
            ),
            sa.Column(
                "monthly_quota_usd",
                sa.Numeric(20, 8),
                nullable=False,
                server_default="0",
            ),
            sa.Column(
                "overage_policy",
                sa.String(length=30),
                nullable=False,
                server_default="block",
            ),
            sa.Column(
                "term_discounts_json",
                postgresql.JSONB(astext_type=sa.Text()),
                nullable=False,
                server_default=sa.text("'[]'::jsonb"),
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
            sa.UniqueConstraint("code", name="uq_subscription_plans_code"),
        )
        op.create_index(
            "ix_subscription_plans_code",
            "subscription_plans",
            ["code"],
            unique=False,
        )
        op.create_index(
            "ix_subscription_plans_user_group_id",
            "subscription_plans",
            ["user_group_id"],
            unique=False,
        )

    if not _table_exists("user_subscriptions"):
        op.create_table(
            "user_subscriptions",
            sa.Column("id", sa.String(length=36), nullable=False),
            sa.Column("user_id", sa.String(length=36), nullable=False),
            sa.Column("plan_id", sa.String(length=36), nullable=False),
            sa.Column("status", sa.String(length=20), nullable=False, server_default="pending_payment"),
            sa.Column("end_reason", sa.String(length=40), nullable=True),
            sa.Column("purchased_months", sa.Integer(), nullable=False),
            sa.Column(
                "discount_factor",
                sa.Numeric(10, 4),
                nullable=False,
                server_default="1.0",
            ),
            sa.Column(
                "monthly_price_usd_snapshot",
                sa.Numeric(20, 8),
                nullable=False,
                server_default="0",
            ),
            sa.Column(
                "total_price_usd",
                sa.Numeric(20, 8),
                nullable=False,
                server_default="0",
            ),
            sa.Column("started_at", sa.DateTime(timezone=True), nullable=False),
            sa.Column("ends_at", sa.DateTime(timezone=True), nullable=False),
            sa.Column("current_cycle_start", sa.DateTime(timezone=True), nullable=False),
            sa.Column("current_cycle_end", sa.DateTime(timezone=True), nullable=False),
            sa.Column(
                "cycle_quota_usd",
                sa.Numeric(20, 8),
                nullable=False,
                server_default="0",
            ),
            sa.Column(
                "cycle_used_usd",
                sa.Numeric(20, 8),
                nullable=False,
                server_default="0",
            ),
            sa.Column(
                "cancel_at_period_end",
                sa.Boolean(),
                nullable=False,
                server_default=sa.false(),
            ),
            sa.Column("canceled_at", sa.DateTime(timezone=True), nullable=True),
            sa.Column("ended_at", sa.DateTime(timezone=True), nullable=True),
            sa.Column("upgraded_from_subscription_id", sa.String(length=36), nullable=True),
            sa.Column("created_at", sa.DateTime(timezone=True), nullable=False),
            sa.Column("updated_at", sa.DateTime(timezone=True), nullable=False),
            sa.ForeignKeyConstraint(["user_id"], ["users.id"], ondelete="CASCADE"),
            sa.ForeignKeyConstraint(["plan_id"], ["subscription_plans.id"], ondelete="RESTRICT"),
            sa.ForeignKeyConstraint(
                ["upgraded_from_subscription_id"],
                ["user_subscriptions.id"],
                ondelete="SET NULL",
            ),
            sa.PrimaryKeyConstraint("id"),
        )
        op.create_index(
            "ix_user_subscriptions_user_id",
            "user_subscriptions",
            ["user_id"],
            unique=False,
        )
        op.create_index(
            "ix_user_subscriptions_plan_id",
            "user_subscriptions",
            ["plan_id"],
            unique=False,
        )
        op.create_index(
            "ix_user_subscriptions_ends_at",
            "user_subscriptions",
            ["ends_at"],
            unique=False,
        )
        op.create_index(
            "ix_user_subscriptions_current_cycle_end",
            "user_subscriptions",
            ["current_cycle_end"],
            unique=False,
        )
        op.create_index(
            "idx_user_subscriptions_user_status",
            "user_subscriptions",
            ["user_id", "status"],
            unique=False,
        )
        op.create_index(
            "idx_user_subscriptions_plan_status",
            "user_subscriptions",
            ["plan_id", "status"],
            unique=False,
        )
        op.execute(
            """
            CREATE UNIQUE INDEX uq_user_subscriptions_single_live
            ON user_subscriptions (user_id)
            WHERE status IN ('pending_payment', 'active')
            """
        )

    if _table_exists("payment_orders"):
        with op.batch_alter_table("payment_orders") as batch_op:
            if not _column_exists("payment_orders", "subscription_id"):
                batch_op.add_column(
                    sa.Column("subscription_id", sa.String(length=36), nullable=True)
                )
            if not _column_exists("payment_orders", "order_type"):
                batch_op.add_column(
                    sa.Column(
                        "order_type",
                        sa.String(length=30),
                        nullable=False,
                        server_default="topup",
                    )
                )
            batch_op.create_foreign_key(
                "fk_payment_orders_subscription_id_user_subscriptions",
                "user_subscriptions",
                ["subscription_id"],
                ["id"],
                ondelete="SET NULL",
            )
        if not _index_exists("payment_orders", "ix_payment_orders_subscription_id"):
            op.create_index(
                "ix_payment_orders_subscription_id",
                "payment_orders",
                ["subscription_id"],
                unique=False,
            )

    if _table_exists("usage"):
        with op.batch_alter_table("usage") as batch_op:
            if not _column_exists("usage", "subscription_id"):
                batch_op.add_column(sa.Column("subscription_id", sa.String(length=36), nullable=True))
            if not _column_exists("usage", "subscription_quota_before_usd"):
                batch_op.add_column(
                    sa.Column("subscription_quota_before_usd", sa.Numeric(20, 8), nullable=True)
                )
            if not _column_exists("usage", "subscription_quota_after_usd"):
                batch_op.add_column(
                    sa.Column("subscription_quota_after_usd", sa.Numeric(20, 8), nullable=True)
                )
            if not _column_exists("usage", "billing_source"):
                batch_op.add_column(sa.Column("billing_source", sa.String(length=30), nullable=True))
            batch_op.create_foreign_key(
                "fk_usage_subscription_id_user_subscriptions",
                "user_subscriptions",
                ["subscription_id"],
                ["id"],
                ondelete="SET NULL",
            )
        if not _index_exists("usage", "ix_usage_subscription_id"):
            op.create_index("ix_usage_subscription_id", "usage", ["subscription_id"], unique=False)


def downgrade() -> None:
    if _table_exists("usage"):
        if _index_exists("usage", "ix_usage_subscription_id"):
            op.drop_index("ix_usage_subscription_id", table_name="usage")
        with op.batch_alter_table("usage") as batch_op:
            if _column_exists("usage", "subscription_id"):
                batch_op.drop_constraint(
                    "fk_usage_subscription_id_user_subscriptions",
                    type_="foreignkey",
                )
            if _column_exists("usage", "billing_source"):
                batch_op.drop_column("billing_source")
            if _column_exists("usage", "subscription_quota_after_usd"):
                batch_op.drop_column("subscription_quota_after_usd")
            if _column_exists("usage", "subscription_quota_before_usd"):
                batch_op.drop_column("subscription_quota_before_usd")
            if _column_exists("usage", "subscription_id"):
                batch_op.drop_column("subscription_id")

    if _table_exists("payment_orders"):
        if _index_exists("payment_orders", "ix_payment_orders_subscription_id"):
            op.drop_index("ix_payment_orders_subscription_id", table_name="payment_orders")
        with op.batch_alter_table("payment_orders") as batch_op:
            if _column_exists("payment_orders", "subscription_id"):
                batch_op.drop_constraint(
                    "fk_payment_orders_subscription_id_user_subscriptions",
                    type_="foreignkey",
                )
            if _column_exists("payment_orders", "order_type"):
                batch_op.drop_column("order_type")
            if _column_exists("payment_orders", "subscription_id"):
                batch_op.drop_column("subscription_id")

    if _table_exists("user_subscriptions"):
        if _index_exists("user_subscriptions", "idx_user_subscriptions_plan_status"):
            op.drop_index("idx_user_subscriptions_plan_status", table_name="user_subscriptions")
        if _index_exists("user_subscriptions", "idx_user_subscriptions_user_status"):
            op.drop_index("idx_user_subscriptions_user_status", table_name="user_subscriptions")
        if _index_exists("user_subscriptions", "ix_user_subscriptions_current_cycle_end"):
            op.drop_index("ix_user_subscriptions_current_cycle_end", table_name="user_subscriptions")
        if _index_exists("user_subscriptions", "ix_user_subscriptions_ends_at"):
            op.drop_index("ix_user_subscriptions_ends_at", table_name="user_subscriptions")
        if _index_exists("user_subscriptions", "ix_user_subscriptions_plan_id"):
            op.drop_index("ix_user_subscriptions_plan_id", table_name="user_subscriptions")
        if _index_exists("user_subscriptions", "ix_user_subscriptions_user_id"):
            op.drop_index("ix_user_subscriptions_user_id", table_name="user_subscriptions")
        op.execute("DROP INDEX IF EXISTS uq_user_subscriptions_single_live")
        op.drop_table("user_subscriptions")

    if _table_exists("subscription_plans"):
        if _index_exists("subscription_plans", "ix_subscription_plans_user_group_id"):
            op.drop_index("ix_subscription_plans_user_group_id", table_name="subscription_plans")
        if _index_exists("subscription_plans", "ix_subscription_plans_code"):
            op.drop_index("ix_subscription_plans_code", table_name="subscription_plans")
        op.drop_table("subscription_plans")
