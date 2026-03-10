"""request_candidates/video_tasks retention: SET NULL and add snapshots

Revision ID: 13a4c8f6d9e0
Revises: 45b118150a78
Create Date: 2026-03-08 12:15:00.000000+00:00

"""

from __future__ import annotations

import sqlalchemy as sa

from alembic import op

# revision identifiers, used by Alembic.
revision = "13a4c8f6d9e0"
down_revision = "45b118150a78"
branch_labels = None
depends_on = None

_TABLES = ["request_candidates", "video_tasks"]


# ---------------------------------------------------------------------------
# Inline helpers
# ---------------------------------------------------------------------------


class _SchemaCache:
    def __init__(self) -> None:
        self._columns: dict[str, dict[str, str]] = {}
        self._fk_rules: dict[tuple[str, str], str] = {}
        self._fk_loaded_tables: set[str] = set()

    def load_columns(self, tables: list[str]) -> None:
        need = [t for t in tables if t not in self._columns]
        if not need:
            return
        bind = op.get_bind()
        rows = bind.execute(
            sa.text(
                "SELECT table_name, column_name, data_type "
                "FROM information_schema.columns "
                "WHERE table_name = ANY(:tables) "
                "  AND table_schema = current_schema()"
            ),
            {"tables": need},
        ).fetchall()
        for t in need:
            self._columns.setdefault(t, {})
        for table, col, dtype in rows:
            self._columns[table][col] = dtype

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

    def column_exists(self, table: str, column: str) -> bool:
        return column in self._columns.get(table, {})

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
    c.load_columns(_TABLES)
    c.load_fk_rules(_TABLES)

    # --- request_candidates: add snapshot columns ---
    if not c.column_exists("request_candidates", "username"):
        op.add_column(
            "request_candidates",
            sa.Column("username", sa.String(length=100), nullable=True, comment="用户名快照"),
        )
    if not c.column_exists("request_candidates", "api_key_name"):
        op.add_column(
            "request_candidates",
            sa.Column(
                "api_key_name",
                sa.String(length=200),
                nullable=True,
                comment="API Key 名称快照",
            ),
        )

    # --- request_candidates: CASCADE -> SET NULL ---
    _replace_fk_if_needed(
        c,
        "request_candidates_user_id_fkey",
        "request_candidates",
        "users",
        ["user_id"],
        ["id"],
        "SET NULL",
    )
    _replace_fk_if_needed(
        c,
        "request_candidates_api_key_id_fkey",
        "request_candidates",
        "api_keys",
        ["api_key_id"],
        ["id"],
        "SET NULL",
    )

    # --- video_tasks: add snapshot columns ---
    if not c.column_exists("video_tasks", "username"):
        op.add_column(
            "video_tasks",
            sa.Column("username", sa.String(length=100), nullable=True, comment="用户名快照"),
        )
    if not c.column_exists("video_tasks", "api_key_name"):
        op.add_column(
            "video_tasks",
            sa.Column(
                "api_key_name",
                sa.String(length=200),
                nullable=True,
                comment="API Key 名称快照",
            ),
        )

    # --- video_tasks: CASCADE -> SET NULL, user_id nullable ---
    op.alter_column("video_tasks", "user_id", existing_type=sa.String(length=36), nullable=True)
    _replace_fk_if_needed(
        c,
        "video_tasks_user_id_fkey",
        "video_tasks",
        "users",
        ["user_id"],
        ["id"],
        "SET NULL",
    )
    _replace_fk_if_needed(
        c,
        "video_tasks_api_key_id_fkey",
        "video_tasks",
        "api_keys",
        ["api_key_id"],
        ["id"],
        "SET NULL",
    )


def downgrade() -> None:
    c = _SchemaCache()
    c.load_columns(_TABLES)
    c.load_fk_rules(_TABLES)

    # --- video_tasks: SET NULL -> default (no action), restore NOT NULL ---
    _replace_fk_if_needed(
        c,
        "video_tasks_api_key_id_fkey",
        "video_tasks",
        "api_keys",
        ["api_key_id"],
        ["id"],
        "NO ACTION",
    )
    _replace_fk_if_needed(
        c,
        "video_tasks_user_id_fkey",
        "video_tasks",
        "users",
        ["user_id"],
        ["id"],
        "NO ACTION",
    )
    op.alter_column("video_tasks", "user_id", existing_type=sa.String(length=36), nullable=False)
    if c.column_exists("video_tasks", "api_key_name"):
        op.drop_column("video_tasks", "api_key_name")
    if c.column_exists("video_tasks", "username"):
        op.drop_column("video_tasks", "username")

    # --- request_candidates: SET NULL -> CASCADE ---
    _replace_fk_if_needed(
        c,
        "request_candidates_api_key_id_fkey",
        "request_candidates",
        "api_keys",
        ["api_key_id"],
        ["id"],
        "CASCADE",
    )
    _replace_fk_if_needed(
        c,
        "request_candidates_user_id_fkey",
        "request_candidates",
        "users",
        ["user_id"],
        ["id"],
        "CASCADE",
    )
    if c.column_exists("request_candidates", "api_key_name"):
        op.drop_column("request_candidates", "api_key_name")
    if c.column_exists("request_candidates", "username"):
        op.drop_column("request_candidates", "username")
