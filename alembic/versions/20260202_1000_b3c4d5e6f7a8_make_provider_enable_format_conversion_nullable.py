"""Make provider enable_format_conversion nullable for inheritance support

Revision ID: b3c4d5e6f7a8
Revises: a2f1b3c4d5e6
Create Date: 2026-02-02 10:00:00+00:00

Changes:
1. providers 表: 修改 enable_format_conversion 字段为可空
   - True: 明确开启格式转换
   - False: 明确禁用格式转换
   - None/NULL: 继承父级设置

这支持三级继承逻辑：端点 > 提供商 > 全局
"""

from alembic import op
import sqlalchemy as sa
from sqlalchemy import inspect


# revision identifiers, used by Alembic.
revision = "b3c4d5e6f7a8"
down_revision = "a2f1b3c4d5e6"
branch_labels = None
depends_on = None


def table_exists(table_name: str) -> bool:
    bind = op.get_bind()
    inspector = inspect(bind)
    return table_name in inspector.get_table_names()


def column_exists(table_name: str, column_name: str) -> bool:
    bind = op.get_bind()
    inspector = inspect(bind)
    columns = [col["name"] for col in inspector.get_columns(table_name)]
    return column_name in columns


def upgrade() -> None:
    # 修改 enable_format_conversion 为可空
    # 这允许 NULL 值表示"继承父级设置"
    if table_exists("providers") and column_exists("providers", "enable_format_conversion"):
        # 1. 移除 server_default
        op.alter_column(
            "providers",
            "enable_format_conversion",
            existing_type=sa.Boolean(),
            server_default=None,
        )
        # 2. 设置为可空
        op.alter_column(
            "providers",
            "enable_format_conversion",
            existing_type=sa.Boolean(),
            nullable=True,
        )
        # 3. 保留现有数据语义：True 和 False 都保持原值
        # 只有新建的 Provider 才会使用 NULL（继承父级设置）
        # 这避免了迁移意外改变已明确禁用格式转换的 Provider 的行为


def downgrade() -> None:
    # 回滚：将 NULL 值改为 False，然后设置为非空
    if table_exists("providers") and column_exists("providers", "enable_format_conversion"):
        # 1. 将 NULL 值改为 False
        bind = op.get_bind()
        bind.execute(
            sa.text(
                "UPDATE providers SET enable_format_conversion = false WHERE enable_format_conversion IS NULL"
            )
        )
        # 2. 设置为非空
        op.alter_column(
            "providers",
            "enable_format_conversion",
            existing_type=sa.Boolean(),
            nullable=False,
        )
        # 3. 恢复 server_default
        op.alter_column(
            "providers",
            "enable_format_conversion",
            existing_type=sa.Boolean(),
            server_default=sa.text("false"),
        )
