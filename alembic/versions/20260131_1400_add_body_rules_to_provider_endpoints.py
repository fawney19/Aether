"""Add body_rules column to provider_endpoints table

Revision ID: d9e3f5a7b2c4
Revises: c8d2e4f6a1b3
Create Date: 2026-01-31 14:00:00.000000

"""

from __future__ import annotations

from typing import Sequence, Union

import sqlalchemy as sa
from sqlalchemy import inspect

from alembic import op

# revision identifiers, used by Alembic.
revision: str = "d9e3f5a7b2c4"
down_revision: Union[str, None] = "c8d2e4f6a1b3"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def column_exists(table_name: str, column_name: str) -> bool:
    """检查列是否存在"""
    bind = op.get_bind()
    inspector = inspect(bind)
    columns = [col["name"] for col in inspector.get_columns(table_name)]
    return column_name in columns


def upgrade() -> None:
    # 添加 body_rules 列到 provider_endpoints 表
    # 请求体规则支持三种操作：
    # - set: 设置/覆盖字段 {"action": "set", "path": "metadata", "value": {"custom": "val"}}
    # - drop: 删除字段 {"action": "drop", "path": "unwanted_field"}
    # - rename: 重命名字段 {"action": "rename", "from": "old_key", "to": "new_key"}
    if not column_exists("provider_endpoints", "body_rules"):
        op.add_column(
            "provider_endpoints",
            sa.Column("body_rules", sa.JSON(), nullable=True),
        )


def downgrade() -> None:
    # 删除 body_rules 列
    if column_exists("provider_endpoints", "body_rules"):
        op.drop_column("provider_endpoints", "body_rules")
