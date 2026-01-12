"""consolidated schema updates

Revision ID: m4n5o6p7q8r9
Revises: 02a45b66b7c4
Create Date: 2026-01-10 20:00:00.000000

This migration consolidates all schema changes from 2026-01-08 to 2026-01-10:

1. provider_api_keys: Key 直接关联 Provider (provider_id, api_formats)
2. provider_api_keys: 添加 rate_multipliers JSON 字段（按格式费率）
3. models: global_model_id 改为可空（支持独立 ProviderModel）
4. providers: 添加 timeout, max_retries, proxy（从 endpoint 迁移）
5. providers: display_name 重命名为 name，删除原 name
6. provider_api_keys: max_concurrent -> rpm_limit（并发改 RPM）
7. provider_api_keys: 健康度改为按格式存储（health_by_format, circuit_breaker_by_format）
8. provider_endpoints: 删除废弃的 rate_limit 列
9. usage: 添加 client_response_headers 字段
10. provider_api_keys: 删除 endpoint_id（Key 不再与 Endpoint 绑定）
11. provider_endpoints: 删除废弃的 max_concurrent 列
12. providers: 删除废弃的 rpm_limit, rpm_used, rpm_reset_at 列
"""

import logging
from alembic import op
import sqlalchemy as sa
from sqlalchemy.dialects import postgresql
from sqlalchemy import inspect


# 配置日志
alembic_logger = logging.getLogger("alembic.runtime.migration")

revision = "m4n5o6p7q8r9"
down_revision = "02a45b66b7c4"
branch_labels = None
depends_on = None


def _column_exists(table_name: str, column_name: str) -> bool:
    """Check if a column exists in the table"""
    bind = op.get_bind()
    inspector = inspect(bind)
    columns = [col["name"] for col in inspector.get_columns(table_name)]
    return column_name in columns


def _constraint_exists(table_name: str, constraint_name: str) -> bool:
    """Check if a constraint exists"""
    bind = op.get_bind()
    inspector = inspect(bind)
    fks = inspector.get_foreign_keys(table_name)
    return any(fk.get("name") == constraint_name for fk in fks)


def _index_exists(table_name: str, index_name: str) -> bool:
    """Check if an index exists"""
    bind = op.get_bind()
    inspector = inspect(bind)
    indexes = inspector.get_indexes(table_name)
    return any(idx.get("name") == index_name for idx in indexes)


def upgrade() -> None:
    """Apply all consolidated schema changes"""
    bind = op.get_bind()

    # ========== 1. provider_api_keys: 添加 provider_id 和 api_formats ==========
    if not _column_exists("provider_api_keys", "provider_id"):
        op.add_column("provider_api_keys", sa.Column("provider_id", sa.String(36), nullable=True))

        # 数据迁移：从 endpoint 获取 provider_id
        op.execute("""
            UPDATE provider_api_keys k
            SET provider_id = e.provider_id
            FROM provider_endpoints e
            WHERE k.endpoint_id = e.id AND k.provider_id IS NULL
        """)

        # 检查无法关联的孤儿 Key
        result = bind.execute(sa.text(
            "SELECT COUNT(*) FROM provider_api_keys WHERE provider_id IS NULL"
        ))
        orphan_count = result.scalar() or 0
        if orphan_count > 0:
            # 使用 logger 记录更明显的告警
            alembic_logger.warning("=" * 60)
            alembic_logger.warning(f"[MIGRATION WARNING] 发现 {orphan_count} 个无法关联 Provider 的孤儿 Key")
            alembic_logger.warning("=" * 60)
            alembic_logger.info("正在备份孤儿 Key 到 _orphan_api_keys_backup 表...")

            # 先备份孤儿数据到临时表，避免数据丢失
            op.execute("""
                CREATE TABLE IF NOT EXISTS _orphan_api_keys_backup AS
                SELECT *, NOW() as backup_at
                FROM provider_api_keys
                WHERE provider_id IS NULL
            """)

            # 记录备份的 Key ID
            orphan_ids = bind.execute(sa.text(
                "SELECT id, name FROM provider_api_keys WHERE provider_id IS NULL"
            )).fetchall()
            alembic_logger.info("备份的孤儿 Key 列表：")
            for key_id, key_name in orphan_ids:
                alembic_logger.info(f"  - Key: {key_name} (ID: {key_id})")

            # 删除孤儿数据
            op.execute("DELETE FROM provider_api_keys WHERE provider_id IS NULL")
            alembic_logger.info(f"已备份并删除 {orphan_count} 个孤儿 Key")

            # 提供恢复指南
            alembic_logger.warning("-" * 60)
            alembic_logger.warning("[恢复指南] 如需恢复孤儿 Key：")
            alembic_logger.warning("  1. 查询备份表: SELECT * FROM _orphan_api_keys_backup;")
            alembic_logger.warning("  2. 确定正确的 provider_id")
            alembic_logger.warning("  3. 执行恢复:")
            alembic_logger.warning("     INSERT INTO provider_api_keys (...)")
            alembic_logger.warning("     SELECT ... FROM _orphan_api_keys_backup WHERE ...;")
            alembic_logger.warning("-" * 60)

        # 设置 NOT NULL 并创建外键
        op.alter_column("provider_api_keys", "provider_id", nullable=False)

        if not _constraint_exists("provider_api_keys", "fk_provider_api_keys_provider"):
            op.create_foreign_key(
                "fk_provider_api_keys_provider",
                "provider_api_keys",
                "providers",
                ["provider_id"],
                ["id"],
                ondelete="CASCADE",
            )

        if not _index_exists("provider_api_keys", "idx_provider_api_keys_provider_id"):
            op.create_index("idx_provider_api_keys_provider_id", "provider_api_keys", ["provider_id"])

    if not _column_exists("provider_api_keys", "api_formats"):
        op.add_column("provider_api_keys", sa.Column("api_formats", sa.JSON(), nullable=True))

        # 数据迁移：从 endpoint 获取 api_format
        op.execute("""
            UPDATE provider_api_keys k
            SET api_formats = json_build_array(e.api_format)
            FROM provider_endpoints e
            WHERE k.endpoint_id = e.id AND k.api_formats IS NULL
        """)

        op.alter_column("provider_api_keys", "api_formats", nullable=False, server_default="[]")

    # 修改 endpoint_id 为可空，外键改为 SET NULL
    if _constraint_exists("provider_api_keys", "provider_api_keys_endpoint_id_fkey"):
        op.drop_constraint("provider_api_keys_endpoint_id_fkey", "provider_api_keys", type_="foreignkey")
        op.alter_column("provider_api_keys", "endpoint_id", nullable=True)
        # 不再重建外键，因为后面会删除这个字段

    # ========== 2. provider_api_keys: 添加 rate_multipliers ==========
    if not _column_exists("provider_api_keys", "rate_multipliers"):
        op.add_column(
            "provider_api_keys",
            sa.Column("rate_multipliers", postgresql.JSON(astext_type=sa.Text()), nullable=True),
        )

        # 数据迁移：将 rate_multiplier 按 api_formats 转换
        op.execute("""
            UPDATE provider_api_keys
            SET rate_multipliers = (
                SELECT jsonb_object_agg(elem, rate_multiplier)
                FROM jsonb_array_elements_text(api_formats::jsonb) AS elem
            )
            WHERE api_formats IS NOT NULL
              AND api_formats::text != '[]'
              AND api_formats::text != 'null'
              AND rate_multipliers IS NULL
        """)

    # ========== 3. models: global_model_id 改为可空 ==========
    op.alter_column("models", "global_model_id", existing_type=sa.String(36), nullable=True)

    # ========== 4. providers: 添加 timeout, max_retries, proxy ==========
    if not _column_exists("providers", "timeout"):
        op.add_column(
            "providers",
            sa.Column("timeout", sa.Integer(), nullable=True, comment="请求超时（秒）"),
        )

    if not _column_exists("providers", "max_retries"):
        op.add_column(
            "providers",
            sa.Column("max_retries", sa.Integer(), nullable=True, comment="最大重试次数"),
        )

    if not _column_exists("providers", "proxy"):
        op.add_column(
            "providers",
            sa.Column("proxy", postgresql.JSONB(), nullable=True, comment="代理配置"),
        )

    # 从端点迁移数据到 provider
    op.execute("""
        UPDATE providers p
        SET
            timeout = COALESCE(
                p.timeout,
                (SELECT MAX(e.timeout) FROM provider_endpoints e WHERE e.provider_id = p.id AND e.timeout IS NOT NULL),
                300
            ),
            max_retries = COALESCE(
                p.max_retries,
                (SELECT MAX(e.max_retries) FROM provider_endpoints e WHERE e.provider_id = p.id AND e.max_retries IS NOT NULL),
                2
            ),
            proxy = COALESCE(
                p.proxy,
                (SELECT e.proxy FROM provider_endpoints e WHERE e.provider_id = p.id AND e.proxy IS NOT NULL ORDER BY e.created_at LIMIT 1)
            )
        WHERE p.timeout IS NULL OR p.max_retries IS NULL
    """)

    # ========== 5. providers: display_name -> name ==========
    # 注意：这里假设 display_name 已经被重命名为 name
    # 如果 display_name 仍然存在，则需要执行重命名
    if _column_exists("providers", "display_name"):
        # 删除旧的 name 索引
        if _index_exists("providers", "ix_providers_name"):
            op.drop_index("ix_providers_name", table_name="providers")

        # 如果存在旧的 name 列，先删除
        if _column_exists("providers", "name"):
            op.drop_column("providers", "name")

        # 重命名 display_name 为 name
        op.alter_column("providers", "display_name", new_column_name="name")

        # 创建新索引
        op.create_index("ix_providers_name", "providers", ["name"], unique=True)

    # ========== 6. provider_api_keys: max_concurrent -> rpm_limit ==========
    if _column_exists("provider_api_keys", "max_concurrent"):
        op.alter_column("provider_api_keys", "max_concurrent", new_column_name="rpm_limit")

    if _column_exists("provider_api_keys", "learned_max_concurrent"):
        op.alter_column("provider_api_keys", "learned_max_concurrent", new_column_name="learned_rpm_limit")

    if _column_exists("provider_api_keys", "last_concurrent_peak"):
        op.alter_column("provider_api_keys", "last_concurrent_peak", new_column_name="last_rpm_peak")

    # 删除废弃字段
    for col in ["rate_limit", "daily_limit", "monthly_limit"]:
        if _column_exists("provider_api_keys", col):
            op.drop_column("provider_api_keys", col)

    # ========== 7. provider_api_keys: 健康度改为按格式存储 ==========
    if not _column_exists("provider_api_keys", "health_by_format"):
        op.add_column(
            "provider_api_keys",
            sa.Column(
                "health_by_format",
                postgresql.JSONB(astext_type=sa.Text()),
                nullable=True,
                comment="按API格式存储的健康度数据",
            ),
        )

    if not _column_exists("provider_api_keys", "circuit_breaker_by_format"):
        op.add_column(
            "provider_api_keys",
            sa.Column(
                "circuit_breaker_by_format",
                postgresql.JSONB(astext_type=sa.Text()),
                nullable=True,
                comment="按API格式存储的熔断器状态",
            ),
        )

    # 数据迁移：如果存在旧字段，迁移数据到新结构
    if _column_exists("provider_api_keys", "health_score"):
        op.execute("""
            UPDATE provider_api_keys
            SET health_by_format = (
                SELECT jsonb_object_agg(
                    elem,
                    jsonb_build_object(
                        'health_score', COALESCE(health_score, 1.0),
                        'consecutive_failures', COALESCE(consecutive_failures, 0),
                        'last_failure_at', last_failure_at,
                        'request_results_window', COALESCE(request_results_window::jsonb, '[]'::jsonb)
                    )
                )
                FROM jsonb_array_elements_text(api_formats::jsonb) AS elem
            )
            WHERE api_formats IS NOT NULL
              AND api_formats::text != '[]'
              AND health_by_format IS NULL
        """)

    # Circuit Breaker 迁移策略：
    # 不复制旧的 circuit_breaker_open 状态到所有 format，而是全部重置为 closed
    # 原因：旧的单一 circuit breaker 状态可能因某一个 format 失败而打开，
    #       如果复制到所有 format，会导致其他正常工作的 format 被错误标记为不可用
    if _column_exists("provider_api_keys", "circuit_breaker_open"):
        op.execute("""
            UPDATE provider_api_keys
            SET circuit_breaker_by_format = (
                SELECT jsonb_object_agg(
                    elem,
                    jsonb_build_object(
                        'open', false,
                        'open_at', NULL,
                        'next_probe_at', NULL,
                        'half_open_until', NULL,
                        'half_open_successes', 0,
                        'half_open_failures', 0
                    )
                )
                FROM jsonb_array_elements_text(api_formats::jsonb) AS elem
            )
            WHERE api_formats IS NOT NULL
              AND api_formats::text != '[]'
              AND circuit_breaker_by_format IS NULL
        """)

    # 设置默认空对象
    op.execute("""
        UPDATE provider_api_keys
        SET health_by_format = '{}'::jsonb
        WHERE health_by_format IS NULL
    """)
    op.execute("""
        UPDATE provider_api_keys
        SET circuit_breaker_by_format = '{}'::jsonb
        WHERE circuit_breaker_by_format IS NULL
    """)

    # 创建 GIN 索引
    if not _index_exists("provider_api_keys", "ix_provider_api_keys_health_by_format"):
        op.create_index(
            "ix_provider_api_keys_health_by_format",
            "provider_api_keys",
            ["health_by_format"],
            postgresql_using="gin",
        )
    if not _index_exists("provider_api_keys", "ix_provider_api_keys_circuit_breaker_by_format"):
        op.create_index(
            "ix_provider_api_keys_circuit_breaker_by_format",
            "provider_api_keys",
            ["circuit_breaker_by_format"],
            postgresql_using="gin",
        )

    # 删除旧字段
    old_health_columns = [
        "health_score",
        "consecutive_failures",
        "last_failure_at",
        "request_results_window",
        "circuit_breaker_open",
        "circuit_breaker_open_at",
        "next_probe_at",
        "half_open_until",
        "half_open_successes",
        "half_open_failures",
    ]
    for col in old_health_columns:
        if _column_exists("provider_api_keys", col):
            op.drop_column("provider_api_keys", col)

    # ========== 8. provider_endpoints: 删除废弃的 rate_limit 列 ==========
    if _column_exists("provider_endpoints", "rate_limit"):
        op.drop_column("provider_endpoints", "rate_limit")

    # ========== 9. usage: 添加 client_response_headers ==========
    if not _column_exists("usage", "client_response_headers"):
        op.add_column(
            "usage",
            sa.Column("client_response_headers", sa.JSON(), nullable=True),
        )

    # ========== 10. provider_api_keys: 删除 endpoint_id ==========
    # Key 不再与 Endpoint 绑定，通过 provider_id + api_formats 关联
    if _column_exists("provider_api_keys", "endpoint_id"):
        # 确保外键已删除（前面可能已经删除）
        try:
            bind = op.get_bind()
            inspector = inspect(bind)
            for fk in inspector.get_foreign_keys("provider_api_keys"):
                constrained = fk.get("constrained_columns") or []
                if "endpoint_id" in constrained:
                    name = fk.get("name")
                    if name:
                        op.drop_constraint(name, "provider_api_keys", type_="foreignkey")
        except Exception:
            pass  # 外键可能已经不存在
        op.drop_column("provider_api_keys", "endpoint_id")

    # ========== 11. provider_endpoints: 删除废弃的 max_concurrent 列 ==========
    if _column_exists("provider_endpoints", "max_concurrent"):
        op.drop_column("provider_endpoints", "max_concurrent")

    # ========== 12. providers: 删除废弃的 RPM 相关字段 ==========
    if _column_exists("providers", "rpm_limit"):
        op.drop_column("providers", "rpm_limit")
    if _column_exists("providers", "rpm_used"):
        op.drop_column("providers", "rpm_used")
    if _column_exists("providers", "rpm_reset_at"):
        op.drop_column("providers", "rpm_reset_at")

    alembic_logger.info("[OK] Consolidated migration completed successfully")


def downgrade() -> None:
    """
    Downgrade is complex due to data migrations.
    For safety, this only removes new columns without restoring old structure.
    Manual intervention may be required for full rollback.
    """
    bind = op.get_bind()

    # 12. 恢复 providers RPM 相关字段
    if not _column_exists("providers", "rpm_limit"):
        op.add_column("providers", sa.Column("rpm_limit", sa.Integer(), nullable=True))
    if not _column_exists("providers", "rpm_used"):
        op.add_column(
            "providers",
            sa.Column("rpm_used", sa.Integer(), server_default="0", nullable=True),
        )
    if not _column_exists("providers", "rpm_reset_at"):
        op.add_column(
            "providers",
            sa.Column("rpm_reset_at", sa.DateTime(timezone=True), nullable=True),
        )

    # 11. 恢复 provider_endpoints.max_concurrent
    if not _column_exists("provider_endpoints", "max_concurrent"):
        op.add_column("provider_endpoints", sa.Column("max_concurrent", sa.Integer(), nullable=True))

    # 10. 恢复 endpoint_id
    if not _column_exists("provider_api_keys", "endpoint_id"):
        op.add_column("provider_api_keys", sa.Column("endpoint_id", sa.String(36), nullable=True))

    # 9. 删除 client_response_headers
    if _column_exists("usage", "client_response_headers"):
        op.drop_column("usage", "client_response_headers")

    # 8. 恢复 provider_endpoints.rate_limit（如果需要）
    if not _column_exists("provider_endpoints", "rate_limit"):
        op.add_column("provider_endpoints", sa.Column("rate_limit", sa.Integer(), nullable=True))

    # 7. 删除健康度 JSON 字段
    bind.execute(sa.text("DROP INDEX IF EXISTS ix_provider_api_keys_health_by_format"))
    bind.execute(sa.text("DROP INDEX IF EXISTS ix_provider_api_keys_circuit_breaker_by_format"))
    if _column_exists("provider_api_keys", "health_by_format"):
        op.drop_column("provider_api_keys", "health_by_format")
    if _column_exists("provider_api_keys", "circuit_breaker_by_format"):
        op.drop_column("provider_api_keys", "circuit_breaker_by_format")

    # 6. rpm_limit -> max_concurrent（简化版：仅重命名）
    if _column_exists("provider_api_keys", "rpm_limit"):
        op.alter_column("provider_api_keys", "rpm_limit", new_column_name="max_concurrent")
    if _column_exists("provider_api_keys", "learned_rpm_limit"):
        op.alter_column("provider_api_keys", "learned_rpm_limit", new_column_name="learned_max_concurrent")
    if _column_exists("provider_api_keys", "last_rpm_peak"):
        op.alter_column("provider_api_keys", "last_rpm_peak", new_column_name="last_concurrent_peak")

    # 恢复已删除的字段
    if not _column_exists("provider_api_keys", "rate_limit"):
        op.add_column("provider_api_keys", sa.Column("rate_limit", sa.Integer(), nullable=True))
    if not _column_exists("provider_api_keys", "daily_limit"):
        op.add_column("provider_api_keys", sa.Column("daily_limit", sa.Integer(), nullable=True))
    if not _column_exists("provider_api_keys", "monthly_limit"):
        op.add_column("provider_api_keys", sa.Column("monthly_limit", sa.Integer(), nullable=True))

    # 5. name -> display_name (需要先删除索引)
    if _index_exists("providers", "ix_providers_name"):
        op.drop_index("ix_providers_name", table_name="providers")
    op.alter_column("providers", "name", new_column_name="display_name")
    # 重新添加原 name 字段
    op.add_column("providers", sa.Column("name", sa.String(100), nullable=True))
    op.execute("""
        UPDATE providers
        SET name = LOWER(REPLACE(REPLACE(display_name, ' ', '_'), '-', '_'))
    """)
    op.alter_column("providers", "name", nullable=False)
    op.create_index("ix_providers_name", "providers", ["name"], unique=True)

    # 4. 删除 providers 的 timeout, max_retries, proxy
    if _column_exists("providers", "proxy"):
        op.drop_column("providers", "proxy")
    if _column_exists("providers", "max_retries"):
        op.drop_column("providers", "max_retries")
    if _column_exists("providers", "timeout"):
        op.drop_column("providers", "timeout")

    # 3. models: global_model_id 改回 NOT NULL
    result = bind.execute(sa.text(
        "SELECT COUNT(*) FROM models WHERE global_model_id IS NULL"
    ))
    orphan_model_count = result.scalar() or 0
    if orphan_model_count > 0:
        alembic_logger.warning(f"[WARN] 发现 {orphan_model_count} 个无 global_model_id 的独立模型，将被删除")
        op.execute("DELETE FROM models WHERE global_model_id IS NULL")
        alembic_logger.info(f"已删除 {orphan_model_count} 个独立模型")
    op.alter_column("models", "global_model_id", nullable=False)

    # 2. 删除 rate_multipliers
    if _column_exists("provider_api_keys", "rate_multipliers"):
        op.drop_column("provider_api_keys", "rate_multipliers")

    # 1. 删除 provider_id 和 api_formats
    if _index_exists("provider_api_keys", "idx_provider_api_keys_provider_id"):
        op.drop_index("idx_provider_api_keys_provider_id", table_name="provider_api_keys")
    if _constraint_exists("provider_api_keys", "fk_provider_api_keys_provider"):
        op.drop_constraint("fk_provider_api_keys_provider", "provider_api_keys", type_="foreignkey")
    if _column_exists("provider_api_keys", "api_formats"):
        op.drop_column("provider_api_keys", "api_formats")
    if _column_exists("provider_api_keys", "provider_id"):
        op.drop_column("provider_api_keys", "provider_id")

    # 恢复 endpoint_id 外键（简化版：仅创建外键，不强制 NOT NULL）
    if _column_exists("provider_api_keys", "endpoint_id"):
        if not _constraint_exists("provider_api_keys", "provider_api_keys_endpoint_id_fkey"):
            op.create_foreign_key(
                "provider_api_keys_endpoint_id_fkey",
                "provider_api_keys",
                "provider_endpoints",
                ["endpoint_id"],
                ["id"],
                ondelete="SET NULL",
            )

    alembic_logger.info("[OK] Downgrade completed (simplified version)")
