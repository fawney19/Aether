"""video_tasks.key_id: add ondelete SET NULL

Revision ID: d7649c1f8e21
Revises: 2053ab8ed764
Create Date: 2026-03-09 01:00:00.000000+00:00

"""

from __future__ import annotations

import sqlalchemy as sa

from alembic import op

# revision identifiers, used by Alembic.
revision = "d7649c1f8e21"
down_revision = "2053ab8ed764"
branch_labels = None
depends_on = None

_TABLE = "video_tasks"
_FK_NAME = "video_tasks_key_id_fkey"


# ---------------------------------------------------------------------------
# Inline helpers
# ---------------------------------------------------------------------------


class _SchemaCache:
    def __init__(self) -> None:
        self._fk_rules: dict[tuple[str, str], str] = {}
        self._fk_loaded_tables: set[str] = set()

    def load_fk_rules(self, tables: list[str]) -> None:
        need = [t for t in tables if t not in self._fk_loaded_tables]
        if not need:
            return
        bind = op.get_bind()
        rows = bind.execute(
            sa.text(
                "SELECT tc.table_name, tc.constraint_name, rc.delete_rule "
                "FROM information_schema.referential_constraints rc "
                "JOIN information_schema.table_constraints tc "
                "  ON rc.constraint_name = tc.constraint_name "
                " AND rc.constraint_schema = tc.constraint_schema "
                "WHERE tc.table_name = ANY(:tables) "
                "  AND tc.table_schema = current_schema()"
            ),
            {"tables": need},
        ).fetchall()
        for table, name, rule in rows:
            self._fk_rules[(table, name)] = rule
        self._fk_loaded_tables.update(need)

    def fk_ondelete(self, table: str, constraint: str) -> str | None:
        return self._fk_rules.get((table, constraint))


def _fk_exists(constraint_name: str, table_name: str) -> bool:
    bind = op.get_bind()
    result = bind.execute(
        sa.text(
            "SELECT 1 FROM pg_constraint c "
            "JOIN pg_class r ON c.conrelid = r.oid "
            "JOIN pg_namespace n ON r.relnamespace = n.oid "
            "WHERE c.conname = :name AND r.relname = :table "
            "  AND n.nspname = current_schema() AND c.contype = 'f'"
        ),
        {"name": constraint_name, "table": table_name},
    )
    return result.scalar() is not None


def _replace_fk_if_needed(
    cache: _SchemaCache,
    constraint_name: str,
    table_name: str,
    ref_table: str,
    local_cols: list[str],
    remote_cols: list[str],
    desired_ondelete: str,
) -> None:
    current = cache.fk_ondelete(table_name, constraint_name)
    if current and current.upper() == desired_ondelete.upper():
        return
    if current or _fk_exists(constraint_name, table_name):
        op.drop_constraint(constraint_name, table_name, type_="foreignkey")
    op.create_foreign_key(
        constraint_name,
        table_name,
        ref_table,
        local_cols,
        remote_cols,
        ondelete=desired_ondelete,
    )


# ---------------------------------------------------------------------------


def upgrade() -> None:
    c = _SchemaCache()
    c.load_fk_rules([_TABLE])
    _replace_fk_if_needed(
        c,
        _FK_NAME,
        _TABLE,
        "provider_api_keys",
        ["key_id"],
        ["id"],
        "SET NULL",
    )


def downgrade() -> None:
    c = _SchemaCache()
    c.load_fk_rules([_TABLE])
    _replace_fk_if_needed(
        c,
        _FK_NAME,
        _TABLE,
        "provider_api_keys",
        ["key_id"],
        ["id"],
        "NO ACTION",
    )
