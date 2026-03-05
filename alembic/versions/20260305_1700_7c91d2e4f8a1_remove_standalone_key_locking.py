"""remove standalone api key locking

Revision ID: 7c91d2e4f8a1
Revises: 6f7a8b9c0d1e
Create Date: 2026-03-05 17:00:00.000000+00:00
"""

from __future__ import annotations

from collections.abc import Sequence

import sqlalchemy as sa
from sqlalchemy import inspect

from alembic import op

# revision identifiers, used by Alembic.
revision: str = "7c91d2e4f8a1"
down_revision: str | None = "6f7a8b9c0d1e"
branch_labels: str | Sequence[str] | None = None
depends_on: str | Sequence[str] | None = None

_CONSTRAINT_NAME = "ck_api_keys_standalone_not_locked"


def _column_exists(table_name: str, column_name: str) -> bool:
    bind = op.get_bind()
    insp = inspect(bind)
    insp.clear_cache()
    return column_name in [c["name"] for c in insp.get_columns(table_name)]


def _check_constraint_exists(table_name: str, constraint_name: str) -> bool:
    bind = op.get_bind()
    insp = inspect(bind)
    insp.clear_cache()
    return any(c.get("name") == constraint_name for c in insp.get_check_constraints(table_name))


def upgrade() -> None:
    if not (
        _column_exists("api_keys", "is_standalone")
        and _column_exists("api_keys", "is_locked")
        and _column_exists("api_keys", "is_active")
    ):
        return

    op.execute(
        sa.text(
            """
            UPDATE api_keys
            SET is_active = FALSE,
                is_locked = FALSE
            WHERE is_standalone IS TRUE AND is_locked IS TRUE
            """
        )
    )

    if not _check_constraint_exists("api_keys", _CONSTRAINT_NAME):
        op.create_check_constraint(
            _CONSTRAINT_NAME,
            "api_keys",
            "(NOT is_standalone) OR (NOT is_locked)",
        )


def downgrade() -> None:
    if _check_constraint_exists("api_keys", _CONSTRAINT_NAME):
        op.drop_constraint(_CONSTRAINT_NAME, "api_keys", type_="check")
