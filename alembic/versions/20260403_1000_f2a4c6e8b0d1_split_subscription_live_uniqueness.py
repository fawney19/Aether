"""split subscription live uniqueness

Revision ID: f2a4c6e8b0d1
Revises: e1f2a3b4c5d6
Create Date: 2026-04-03 10:00:00.000000+00:00
"""

from __future__ import annotations

from collections.abc import Sequence

from sqlalchemy import inspect

from alembic import op

# revision identifiers, used by Alembic.
revision: str = "f2a4c6e8b0d1"
down_revision: str | None = "e1f2a3b4c5d6"
branch_labels: str | Sequence[str] | None = None
depends_on: str | Sequence[str] | None = None


def _table_exists(table_name: str) -> bool:
    bind = op.get_bind()
    inspector = inspect(bind)
    return table_name in inspector.get_table_names()


def _index_exists(table_name: str, index_name: str) -> bool:
    bind = op.get_bind()
    inspector = inspect(bind)
    return any(index["name"] == index_name for index in inspector.get_indexes(table_name))


def upgrade() -> None:
    if not _table_exists("user_subscriptions"):
        return

    if _index_exists("user_subscriptions", "uq_user_subscriptions_single_live"):
        op.execute("DROP INDEX IF EXISTS uq_user_subscriptions_single_live")

    if _index_exists("user_subscriptions", "uq_user_subscriptions_single_active"):
        op.execute("DROP INDEX IF EXISTS uq_user_subscriptions_single_active")

    if not _index_exists("user_subscriptions", "uq_user_subscriptions_single_pending"):
        op.execute(
            """
            CREATE UNIQUE INDEX uq_user_subscriptions_single_pending
            ON user_subscriptions (user_id)
            WHERE status = 'pending_payment'
            """
        )


def downgrade() -> None:
    if not _table_exists("user_subscriptions"):
        return

    if _index_exists("user_subscriptions", "uq_user_subscriptions_single_pending"):
        op.execute("DROP INDEX IF EXISTS uq_user_subscriptions_single_pending")

    if not _index_exists("user_subscriptions", "uq_user_subscriptions_single_live"):
        op.execute(
            """
            CREATE UNIQUE INDEX uq_user_subscriptions_single_live
            ON user_subscriptions (user_id)
            WHERE status IN ('pending_payment', 'active')
            """
        )
