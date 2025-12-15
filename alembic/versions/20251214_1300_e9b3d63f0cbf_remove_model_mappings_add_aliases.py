"""remove_model_mappings_add_aliases

合并迁移：
1. 添加 provider_model_aliases 字段到 models 表
2. 迁移 model_mappings 数据到 provider_model_aliases
3. 删除 model_mappings 表
4. 添加索引优化别名解析性能

Revision ID: e9b3d63f0cbf
Revises: 20251210_baseline
Create Date: 2025-12-14 13:00:22.828183+00:00

"""
import json
from datetime import datetime, timezone

import sqlalchemy as sa
from alembic import op
from sqlalchemy.orm import Session


# revision identifiers, used by Alembic.
revision = 'e9b3d63f0cbf'
down_revision = '20251210_baseline'
branch_labels = None
depends_on = None


def upgrade() -> None:
    """添加 provider_model_aliases 字段，迁移数据，删除 model_mappings 表"""
    # 1. 添加 provider_model_aliases 字段
    op.add_column(
        'models',
        sa.Column('provider_model_aliases', sa.JSON(), nullable=True)
    )

    # 2. 迁移 model_mappings 数据
    bind = op.get_bind()
    session = Session(bind=bind)

    model_mappings_table = sa.table(
        "model_mappings",
        sa.column("source_model", sa.String),
        sa.column("target_global_model_id", sa.String),
        sa.column("provider_id", sa.String),
        sa.column("mapping_type", sa.String),
        sa.column("is_active", sa.Boolean),
    )

    models_table = sa.table(
        "models",
        sa.column("id", sa.String),
        sa.column("provider_id", sa.String),
        sa.column("global_model_id", sa.String),
        sa.column("provider_model_aliases", sa.JSON),
        sa.column("updated_at", sa.DateTime(timezone=True)),
    )

    def normalize_alias_list(value) -> list[dict]:
        """将 DB 返回的 JSON 值规范化为 list[{'name': str, 'priority': int}]"""
        if value is None:
            return []

        if isinstance(value, str):
            try:
                value = json.loads(value) if value else []
            except Exception:
                return []

        if not isinstance(value, list):
            return []

        normalized: list[dict] = []
        for item in value:
            if not isinstance(item, dict):
                continue

            raw_name = item.get("name")
            if not isinstance(raw_name, str):
                continue
            name = raw_name.strip()
            if not name:
                continue

            raw_priority = item.get("priority", 1)
            try:
                priority = int(raw_priority)
            except Exception:
                priority = 1
            if priority < 1:
                priority = 1

            normalized.append({"name": name, "priority": priority})

        return normalized

    # 查询所有活跃的 provider 级别 alias（只迁移 is_active=True 且 mapping_type='alias' 的）
    # 全局别名/映射不迁移（新架构不再支持 source_model -> GlobalModel.name 的解析）
    mappings = session.execute(
        sa.select(
            model_mappings_table.c.source_model,
            model_mappings_table.c.target_global_model_id,
            model_mappings_table.c.provider_id,
        )
        .where(
            model_mappings_table.c.is_active.is_(True),
            model_mappings_table.c.provider_id.isnot(None),
            model_mappings_table.c.mapping_type == "alias",
        )
        .order_by(model_mappings_table.c.provider_id, model_mappings_table.c.source_model)
    ).all()

    # 按 (provider_id, target_global_model_id) 分组，收集别名
    alias_groups: dict = {}
    for source_model, target_global_model_id, provider_id in mappings:
        if not isinstance(source_model, str):
            continue
        source_model = source_model.strip()
        if not source_model:
            continue
        if not isinstance(provider_id, str) or not provider_id:
            continue
        if not isinstance(target_global_model_id, str) or not target_global_model_id:
            continue

        key = (provider_id, target_global_model_id)
        if key not in alias_groups:
            alias_groups[key] = []
        priority = len(alias_groups[key]) + 1
        alias_groups[key].append({"name": source_model, "priority": priority})

    # 更新对应的 models 记录
    for (provider_id, global_model_id), aliases in alias_groups.items():
        model_row = session.execute(
            sa.select(models_table.c.id, models_table.c.provider_model_aliases)
            .where(
                models_table.c.provider_id == provider_id,
                models_table.c.global_model_id == global_model_id,
            )
            .limit(1)
        ).first()

        if model_row:
            model_id = model_row[0]
            existing_aliases = normalize_alias_list(model_row[1])

            existing_names = {a["name"] for a in existing_aliases}
            merged_aliases = list(existing_aliases)
            for alias in aliases:
                name = alias.get("name")
                if not isinstance(name, str):
                    continue
                name = name.strip()
                if not name or name in existing_names:
                    continue

                merged_aliases.append(
                    {
                        "name": name,
                        "priority": len(merged_aliases) + 1,
                    }
                )
                existing_names.add(name)

            session.execute(
                models_table.update()
                .where(models_table.c.id == model_id)
                .values(
                    provider_model_aliases=merged_aliases if merged_aliases else None,
                    updated_at=datetime.now(timezone.utc),
                )
            )

    session.commit()

    # 3. 删除 model_mappings 表
    op.drop_table('model_mappings')

    # 4. 添加索引优化别名解析性能
    # provider_model_name 索引（支持精确匹配）
    op.create_index(
        "idx_model_provider_model_name",
        "models",
        ["provider_model_name"],
        unique=False,
        postgresql_where=sa.text("is_active = true"),
    )

    # provider_model_aliases GIN 索引（支持 JSONB 查询，仅 PostgreSQL）
    if bind.dialect.name == "postgresql":
        # 将 json 列转为 jsonb（jsonb 性能更好且支持 GIN 索引）
        op.execute(
            """
            ALTER TABLE models
            ALTER COLUMN provider_model_aliases TYPE jsonb
            USING provider_model_aliases::jsonb
            """
        )
        # 创建 GIN 索引
        op.execute(
            """
            CREATE INDEX IF NOT EXISTS idx_model_provider_model_aliases_gin
            ON models USING gin(provider_model_aliases jsonb_path_ops)
            WHERE is_active = true
            """
        )


def downgrade() -> None:
    """恢复 model_mappings 表，移除 provider_model_aliases 字段和索引"""
    bind = op.get_bind()

    # 1. 删除索引
    op.drop_index("idx_model_provider_model_name", table_name="models")

    if bind.dialect.name == "postgresql":
        op.execute("DROP INDEX IF EXISTS idx_model_provider_model_aliases_gin")
        # 将 jsonb 列还原为 json
        op.execute(
            """
            ALTER TABLE models
            ALTER COLUMN provider_model_aliases TYPE json
            USING provider_model_aliases::json
            """
        )

    # 2. 恢复 model_mappings 表
    op.create_table(
        'model_mappings',
        sa.Column('id', sa.String(36), primary_key=True),
        sa.Column('source_model', sa.String(200), nullable=False),
        sa.Column(
            'target_global_model_id',
            sa.String(36),
            sa.ForeignKey('global_models.id', ondelete='CASCADE'),
            nullable=False,
        ),
        sa.Column('provider_id', sa.String(36), sa.ForeignKey('providers.id'), nullable=True),
        sa.Column('mapping_type', sa.String(20), nullable=False, server_default='alias'),
        sa.Column('is_active', sa.Boolean(), nullable=False, server_default='true'),
        sa.Column('created_at', sa.DateTime(timezone=True), server_default=sa.func.now(), nullable=False),
        sa.Column('updated_at', sa.DateTime(timezone=True), server_default=sa.func.now(), nullable=False),
        sa.UniqueConstraint('source_model', 'provider_id', name='uq_model_mapping_source_provider'),
    )
    op.create_index('ix_model_mappings_source_model', 'model_mappings', ['source_model'])
    op.create_index('ix_model_mappings_target_global_model_id', 'model_mappings', ['target_global_model_id'])
    op.create_index('ix_model_mappings_provider_id', 'model_mappings', ['provider_id'])
    op.create_index('ix_model_mappings_mapping_type', 'model_mappings', ['mapping_type'])

    # 3. 移除 provider_model_aliases 字段
    op.drop_column('models', 'provider_model_aliases')
