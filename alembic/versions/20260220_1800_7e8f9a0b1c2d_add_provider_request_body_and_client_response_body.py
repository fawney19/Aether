"""Add provider_request_body and client_response_body columns to usage table

Revision ID: 7e8f9a0b1c2d
Revises: 6d7e8f9a0b1c
Create Date: 2026-02-20 18:00:00.000000

"""

from __future__ import annotations

from collections.abc import Sequence

from sqlalchemy import text

from alembic import op

# revision identifiers, used by Alembic.
revision: str = "7e8f9a0b1c2d"
down_revision: str | None = "6d7e8f9a0b1c"
branch_labels: str | Sequence[str] | None = None
depends_on: str | Sequence[str] | None = None


def upgrade() -> None:
    conn = op.get_bind()
    # Use PostgreSQL native IF NOT EXISTS to avoid duplicate-column races
    # when migrations are triggered concurrently (e.g. startup + manual run).
    conn.execute(text("ALTER TABLE usage ADD COLUMN IF NOT EXISTS provider_request_body JSON"))
    conn.execute(
        text("ALTER TABLE usage ADD COLUMN IF NOT EXISTS provider_request_body_compressed BYTEA")
    )
    conn.execute(text("ALTER TABLE usage ADD COLUMN IF NOT EXISTS client_response_body JSON"))
    conn.execute(
        text("ALTER TABLE usage ADD COLUMN IF NOT EXISTS client_response_body_compressed BYTEA")
    )


def downgrade() -> None:
    conn = op.get_bind()
    for col in (
        "client_response_body_compressed",
        "client_response_body",
        "provider_request_body_compressed",
        "provider_request_body",
    ):
        conn.execute(text(f"ALTER TABLE usage DROP COLUMN IF EXISTS {col}"))
