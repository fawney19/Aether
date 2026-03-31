"""merge usage cache snapshot and model group heads

Revision ID: c4f5a6b7d8e9
Revises: b6c7d8e9f0a1, e6f7a8b9c0d1
Create Date: 2026-04-01 00:10:00.000000+00:00
"""

from __future__ import annotations

from collections.abc import Sequence

# revision identifiers, used by Alembic.
revision: str = "c4f5a6b7d8e9"
down_revision: tuple[str, str] = ("b6c7d8e9f0a1", "e6f7a8b9c0d1")
branch_labels: str | Sequence[str] | None = None
depends_on: str | Sequence[str] | None = None


def upgrade() -> None:
    pass


def downgrade() -> None:
    pass
