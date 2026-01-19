"""add_stats_daily_provider_table

Revision ID: c868729753ad
Revises: 33e347f97c0c
Create Date: 2026-01-19 05:19:49.634662+00:00

"""
from alembic import op
import sqlalchemy as sa


# revision identifiers, used by Alembic.
revision = 'c868729753ad'
down_revision = '33e347f97c0c'
branch_labels = None
depends_on = None


def upgrade() -> None:
    """应用迁移：升级到新版本"""
    op.create_table(
        'stats_daily_provider',
        sa.Column('id', sa.String(length=36), nullable=False),
        sa.Column('date', sa.DateTime(timezone=True), nullable=False),
        sa.Column('provider_name', sa.String(length=100), nullable=False),
        sa.Column('total_requests', sa.Integer(), nullable=False),
        sa.Column('input_tokens', sa.BigInteger(), nullable=False),
        sa.Column('output_tokens', sa.BigInteger(), nullable=False),
        sa.Column('cache_creation_tokens', sa.BigInteger(), nullable=False),
        sa.Column('cache_read_tokens', sa.BigInteger(), nullable=False),
        sa.Column('total_cost', sa.Float(), nullable=False),
        sa.Column('created_at', sa.DateTime(timezone=True), nullable=False),
        sa.Column('updated_at', sa.DateTime(timezone=True), nullable=False),
        sa.PrimaryKeyConstraint('id'),
        sa.UniqueConstraint('date', 'provider_name', name='uq_stats_daily_provider')
    )
    op.create_index('idx_stats_daily_provider_date', 'stats_daily_provider', ['date'], unique=False)
    op.create_index('idx_stats_daily_provider_date_provider', 'stats_daily_provider', ['date', 'provider_name'], unique=False)


def downgrade() -> None:
    """回滚迁移：降级到旧版本"""
    op.drop_index('idx_stats_daily_provider_date_provider', table_name='stats_daily_provider')
    op.drop_index('idx_stats_daily_provider_date', table_name='stats_daily_provider')
    op.drop_table('stats_daily_provider')
