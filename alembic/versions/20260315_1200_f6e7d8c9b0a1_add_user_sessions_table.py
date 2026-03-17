"""add user sessions table for device-level auth

Revision ID: f6e7d8c9b0a1
Revises: b7e8f9a0c1d2
Create Date: 2026-03-15 12:00:00.000000+00:00

"""

from __future__ import annotations

import sqlalchemy as sa

from alembic import op

# revision identifiers, used by Alembic.
revision = "f6e7d8c9b0a1"
down_revision = "b7e8f9a0c1d2"
branch_labels = None
depends_on = None


def upgrade() -> None:
    bind = op.get_bind()
    inspector = sa.inspect(bind)
    if "user_sessions" in inspector.get_table_names():
        return

    op.create_table(
        "user_sessions",
        sa.Column("id", sa.String(length=36), nullable=False),
        sa.Column("user_id", sa.String(length=36), nullable=False),
        sa.Column("client_device_id", sa.String(length=128), nullable=False),
        sa.Column("device_label", sa.String(length=120), nullable=True),
        sa.Column("device_type", sa.String(length=20), nullable=False, server_default="unknown"),
        sa.Column("browser_name", sa.String(length=50), nullable=True),
        sa.Column("browser_version", sa.String(length=50), nullable=True),
        sa.Column("os_name", sa.String(length=50), nullable=True),
        sa.Column("os_version", sa.String(length=50), nullable=True),
        sa.Column("device_model", sa.String(length=100), nullable=True),
        sa.Column("ip_address", sa.String(length=45), nullable=True),
        sa.Column("user_agent", sa.String(length=1000), nullable=True),
        sa.Column("client_hints", sa.JSON(), nullable=True),
        sa.Column("refresh_token_hash", sa.String(length=64), nullable=False),
        sa.Column("prev_refresh_token_hash", sa.String(length=64), nullable=True),
        sa.Column("rotated_at", sa.DateTime(timezone=True), nullable=True),
        sa.Column(
            "last_seen_at", sa.DateTime(timezone=True), nullable=False, server_default=sa.func.now()
        ),
        sa.Column("expires_at", sa.DateTime(timezone=True), nullable=False),
        sa.Column("revoked_at", sa.DateTime(timezone=True), nullable=True),
        sa.Column("revoke_reason", sa.String(length=100), nullable=True),
        sa.Column(
            "created_at", sa.DateTime(timezone=True), nullable=False, server_default=sa.func.now()
        ),
        sa.Column(
            "updated_at", sa.DateTime(timezone=True), nullable=False, server_default=sa.func.now()
        ),
        sa.ForeignKeyConstraint(["user_id"], ["users.id"], ondelete="CASCADE"),
        sa.PrimaryKeyConstraint("id"),
    )
    op.create_index("ix_user_sessions_user_id", "user_sessions", ["user_id"], unique=False)
    op.create_index(
        "ix_user_sessions_client_device_id",
        "user_sessions",
        ["client_device_id"],
        unique=False,
    )
    op.create_index(
        "idx_user_sessions_user_active",
        "user_sessions",
        ["user_id", "revoked_at", "expires_at"],
        unique=False,
    )
    op.create_index(
        "idx_user_sessions_user_device",
        "user_sessions",
        ["user_id", "client_device_id"],
        unique=False,
    )


def downgrade() -> None:
    op.drop_index("idx_user_sessions_user_device", table_name="user_sessions")
    op.drop_index("idx_user_sessions_user_active", table_name="user_sessions")
    op.drop_index("ix_user_sessions_client_device_id", table_name="user_sessions")
    op.drop_index("ix_user_sessions_user_id", table_name="user_sessions")
    op.drop_table("user_sessions")
