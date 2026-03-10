"""Strip request_results_window from health_by_format JSON.

This data is now maintained in process memory only, no longer persisted to DB.

Revision ID: a3f1b7c9d2e4
Revises: d7649c1f8e21
Create Date: 2026-03-10 12:00:00.000000+00:00

"""

from alembic import op

# revision identifiers, used by Alembic.
revision = "a3f1b7c9d2e4"
down_revision = "d7649c1f8e21"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.execute("""
        UPDATE provider_api_keys
        SET health_by_format = (
            SELECT jsonb_object_agg(
                fmt_key,
                fmt_value - 'request_results_window'
            )
            FROM jsonb_each(health_by_format) AS x(fmt_key, fmt_value)
        )
        WHERE health_by_format IS NOT NULL
          AND health_by_format != '{}'::jsonb
          AND EXISTS (
              SELECT 1
              FROM jsonb_each(health_by_format) AS x(fmt_key, fmt_value)
              WHERE fmt_value ? 'request_results_window'
          )
    """)


def downgrade() -> None:
    # No-op: window data is rebuilt from scratch on process start
    pass
