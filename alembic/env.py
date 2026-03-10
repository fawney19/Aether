"""
Alembic 环境配置
用于数据库迁移的运行时环境设置
"""

import os
import sys
from logging.config import fileConfig
from pathlib import Path

from sqlalchemy import engine_from_config, pool, text

from alembic import context

# 添加项目根目录到 Python 路径
sys.path.insert(0, os.path.dirname(os.path.dirname(__file__)))

# 加载 .env 文件（本地开发时需要）
try:
    from dotenv import load_dotenv

    env_file = Path(__file__).parent.parent / ".env"
    if env_file.exists():
        load_dotenv(env_file)
except ImportError:
    pass

# 导入所有数据库模型（确保 Alembic 能检测到所有表）
from src.models.database import Base

# Alembic Config 对象
config = context.config

# 从环境变量获取数据库 URL
# 优先使用 DATABASE_URL，否则从 DB_PASSWORD 自动构建（与 docker compose 保持一致）
database_url = os.getenv("DATABASE_URL")
if not database_url:
    db_password = os.getenv("DB_PASSWORD", "")
    db_host = os.getenv("DB_HOST", "localhost")
    db_port = os.getenv("DB_PORT", "5432")
    db_name = os.getenv("DB_NAME", "aether")
    db_user = os.getenv("DB_USER", "postgres")
    database_url = f"postgresql://{db_user}:{db_password}@{db_host}:{db_port}/{db_name}"
config.set_main_option("sqlalchemy.url", database_url)

# 配置日志
if config.config_file_name is not None:
    fileConfig(config.config_file_name)

# 目标元数据（包含所有表定义）
target_metadata = Base.metadata

# PostgreSQL 全局迁移锁，避免多进程并发执行 Alembic 导致竞态（重复加列/索引等）
# 使用会话级 advisory lock（pg_advisory_lock），在迁移完成后手动释放。
# ID 由 crc32("aether-alembic-migration") 拼接生成，仅需全局唯一即可。
MIGRATION_ADVISORY_LOCK_ID = 582694137405821


def run_migrations_offline() -> None:
    """
    离线模式运行迁移

    在离线模式下，不需要连接数据库，
    只生成 SQL 脚本
    """
    url = config.get_main_option("sqlalchemy.url")
    context.configure(
        url=url,
        target_metadata=target_metadata,
        literal_binds=True,
        dialect_opts={"paramstyle": "named"},
        compare_type=True,  # 比较列类型变更
        compare_server_default=True,  # 比较默认值变更
    )

    with context.begin_transaction():
        context.run_migrations()


def run_migrations_online() -> None:
    """
    在线模式运行迁移

    在线模式下，直接连接数据库执行迁移
    """
    connectable = engine_from_config(
        config.get_section(config.config_ini_section, {}),
        prefix="sqlalchemy.",
        poolclass=pool.NullPool,
    )

    with connectable.connect() as connection:
        try:
            # 使用会话级 advisory lock（非事务级），避免干扰 Alembic 的事务管理。
            # pg_advisory_lock 在会话结束时自动释放，不受 COMMIT/ROLLBACK 影响。
            if connection.dialect.name == "postgresql":
                connection.execute(
                    text("SELECT pg_advisory_lock(:lock_id)"),
                    {"lock_id": MIGRATION_ADVISORY_LOCK_ID},
                )
                connection.commit()

            context.configure(
                connection=connection,
                target_metadata=target_metadata,
                compare_type=True,
                compare_server_default=True,
                transaction_per_migration=True,  # 每个迁移文件独立事务，完成即提交
            )

            with context.begin_transaction():
                context.run_migrations()
        except Exception:
            raise


# 根据模式选择运行方式
if context.is_offline_mode():
    run_migrations_offline()
else:
    run_migrations_online()
