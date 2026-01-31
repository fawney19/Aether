"""merge heads

Revision ID: 5a9c2f40f54b
Revises: d9e3f5a7b2c4, a2f1b3c4d5e6
Create Date: 2026-02-02 13:59:51.523892+00:00

"""
from alembic import op
import sqlalchemy as sa


# revision identifiers, used by Alembic.
revision = '5a9c2f40f54b'
down_revision = ('d9e3f5a7b2c4', 'a2f1b3c4d5e6')
branch_labels = None
depends_on = None


def upgrade() -> None:
    """应用迁移：升级到新版本"""
    pass


def downgrade() -> None:
    """回滚迁移：降级到旧版本"""
    pass
