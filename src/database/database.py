"""
数据库连接和初始化
"""

import time
from typing import AsyncGenerator, Generator, Optional

from starlette.requests import Request
from sqlalchemy import create_engine, event
from sqlalchemy.engine import Engine
from sqlalchemy.ext.asyncio import (
    AsyncEngine,
    AsyncSession,
    async_sessionmaker,
    create_async_engine,
)
from sqlalchemy.orm import Session, sessionmaker
from sqlalchemy.pool import Pool, QueuePool

from ..config import config
from src.core.logger import logger
from ..models.database import Base, SystemConfig, User, UserRole


# 延迟初始化的数据库引擎和会话工厂
_engine: Optional[Engine] = None
_SessionLocal: Optional[sessionmaker] = None
_async_engine: Optional[AsyncEngine] = None
_AsyncSessionLocal: Optional[async_sessionmaker] = None

# 连接池监控
_last_pool_warning: float = 0.0
POOL_WARNING_INTERVAL = 60  # 每60秒最多警告一次


def _setup_pool_monitoring(engine: Engine):
    """设置连接池监控事件"""

    @event.listens_for(engine, "connect")
    def receive_connect(dbapi_conn, connection_record):
        """连接创建时的监控"""
        pass

    @event.listens_for(engine, "checkout")
    def receive_checkout(dbapi_conn, connection_record, connection_proxy):
        """从连接池检出连接时的监控"""
        global _last_pool_warning

        pool = engine.pool
        # 获取连接池状态
        checked_out = pool.checkedout()
        pool_size = pool.size()
        overflow = pool.overflow()
        max_capacity = config.db_pool_size + config.db_max_overflow

        # 计算使用率
        usage_rate = (checked_out / max_capacity) * 100 if max_capacity > 0 else 0

        # 如果使用率超过阈值，发出警告
        if usage_rate >= config.db_pool_warn_threshold:
            current_time = time.time()
            # 避免频繁警告
            if current_time - _last_pool_warning > POOL_WARNING_INTERVAL:
                _last_pool_warning = current_time
                logger.warning(
                    f"数据库连接池使用率过高: checked_out={checked_out}, "
                    f"pool_size={pool_size}, overflow={overflow}, "
                    f"max_capacity={max_capacity}, usage_rate={usage_rate:.1f}%, "
                    f"threshold={config.db_pool_warn_threshold}%"
                )


def get_pool_status() -> dict:
    """获取连接池状态"""
    engine = _ensure_engine()
    pool = engine.pool

    return {
        "checked_out": pool.checkedout(),
        "pool_size": pool.size(),
        "overflow": pool.overflow(),
        "max_capacity": config.db_pool_size + config.db_max_overflow,
        "pool_timeout": config.db_pool_timeout,
    }


def log_pool_status():
    """记录连接池状态到日志（用于监控）"""
    try:
        status = get_pool_status()
        usage_rate = (
            (status["checked_out"] / status["max_capacity"] * 100)
            if status["max_capacity"] > 0
            else 0
        )

        logger.info(
            f"数据库连接池状态: checked_out={status['checked_out']}, "
            f"pool_size={status['pool_size']}, overflow={status['overflow']}, "
            f"max_capacity={status['max_capacity']}, usage_rate={usage_rate:.1f}%"
        )
    except Exception as e:
        logger.error(f"获取连接池状态失败: {e}")


def dispose_engines():
    """释放连接池，供 Gunicorn post_fork 钩子调用"""
    global _engine, _async_engine, _SessionLocal, _AsyncSessionLocal
    
    if _engine:
        logger.info("释放同步数据库连接池")
        _engine.dispose()
        _engine = None
        _SessionLocal = None
    
    if _async_engine:
        logger.info("释放异步数据库连接池")
        _async_engine = None
        _AsyncSessionLocal = None


def _ensure_engine() -> Engine:
    """
    确保数据库引擎已创建（延迟加载）

    这允许测试和 CLI 工具在导入模块时不会立即连接数据库
    """
    global _engine, _SessionLocal

    if _engine is not None:
        return _engine

    # 获取数据库配置
    DATABASE_URL = config.database_url

    # 验证数据库类型（生产环境要求 PostgreSQL，但允许测试环境使用其他数据库）
    is_production = config.environment == "production"
    if is_production and not DATABASE_URL.startswith("postgresql://"):
        raise ValueError("生产环境只支持 PostgreSQL 数据库，请配置正确的 DATABASE_URL")

    # 创建引擎
    _engine = create_engine(
        DATABASE_URL,
        poolclass=QueuePool,  # 使用队列连接池
        pool_size=config.db_pool_size,  # 连接池大小
        max_overflow=config.db_max_overflow,  # 最大溢出连接数
        pool_timeout=config.db_pool_timeout,  # 连接超时（秒）
        pool_recycle=config.db_pool_recycle,  # 连接回收时间（秒）
        pool_pre_ping=True,  # 检查连接活性
        echo=False,  # 关闭SQL日志输出（太冗长）
    )

    # 设置连接池监控
    _setup_pool_monitoring(_engine)

    # 创建会话工厂
    _SessionLocal = sessionmaker(autocommit=False, autoflush=False, bind=_engine)

    _log_pool_capacity()

    logger.debug(f"数据库引擎已初始化: {DATABASE_URL.split('@')[-1] if '@' in DATABASE_URL else 'local'}")

    return _engine


def _log_pool_capacity():
    theoretical = config.db_pool_size + config.db_max_overflow
    workers = max(1, config.worker_processes)
    total_estimated = theoretical * workers
    safe_limit = config.pg_max_connections - config.pg_reserved_connections
    logger.info(
        "数据库连接池配置: pool_size={}, max_overflow={}, workers={}, total_estimated={}, safe_limit={}",
        config.db_pool_size,
        config.db_max_overflow,
        workers,
        total_estimated,
        safe_limit,
    )
    if total_estimated > safe_limit:
        logger.warning(
            "数据库连接池总需求可能超过 PostgreSQL 限制: {} > {} (pg_max_connections - reserved)，"
            "建议调整 DB_POOL_SIZE/DB_MAX_OVERFLOW 或减少 worker 数",
            total_estimated,
            safe_limit,
        )


def _ensure_async_engine() -> AsyncEngine:
    """
    确保异步数据库引擎已创建（延迟加载）

    这允许异步路由使用非阻塞的数据库访问
    """
    global _async_engine, _AsyncSessionLocal

    if _async_engine is not None:
        return _async_engine

    # 获取数据库配置并转换为异步URL
    DATABASE_URL = config.database_url

    # 转换同步URL为异步URL（postgresql:// -> postgresql+asyncpg://）
    if DATABASE_URL.startswith("postgresql://"):
        ASYNC_DATABASE_URL = DATABASE_URL.replace("postgresql://", "postgresql+asyncpg://", 1)
    elif DATABASE_URL.startswith("sqlite:///"):
        ASYNC_DATABASE_URL = DATABASE_URL.replace("sqlite:///", "sqlite+aiosqlite:///", 1)
    else:
        raise ValueError(f"不支持的数据库类型: {DATABASE_URL}")

    # 验证数据库类型（生产环境要求 PostgreSQL）
    is_production = config.environment == "production"
    if is_production and not ASYNC_DATABASE_URL.startswith("postgresql+asyncpg://"):
        raise ValueError("生产环境只支持 PostgreSQL 数据库，请配置正确的 DATABASE_URL")

    # 创建异步引擎
    _async_engine = create_async_engine(
        ASYNC_DATABASE_URL,
        # AsyncEngine 不能使用 QueuePool；默认使用 AsyncAdaptedQueuePool
        pool_size=config.db_pool_size,
        max_overflow=config.db_max_overflow,
        pool_timeout=config.db_pool_timeout,
        pool_recycle=config.db_pool_recycle,
        pool_pre_ping=True,
        echo=False,
    )

    # 创建异步会话工厂
    _AsyncSessionLocal = async_sessionmaker(
        _async_engine,
        class_=AsyncSession,
        expire_on_commit=False,
        autocommit=False,
        autoflush=False,
    )

    logger.debug(f"异步数据库引擎已初始化: {ASYNC_DATABASE_URL.split('@')[-1] if '@' in ASYNC_DATABASE_URL else 'local'}")

    return _async_engine


async def get_async_db() -> AsyncGenerator[AsyncSession, None]:
    """获取异步数据库会话

    .. deprecated::
        此方法已废弃，项目统一使用同步 Session。
        未来版本可能移除此方法。请使用 get_db() 代替。
    """
    import warnings
    warnings.warn(
        "get_async_db() 已废弃，项目统一使用同步 Session。请使用 get_db() 代替。",
        DeprecationWarning,
        stacklevel=2,
    )
    # 确保异步引擎已初始化
    _ensure_async_engine()

    async with _AsyncSessionLocal() as session:
        try:
            yield session
        finally:
            await session.close()


def get_db(request: Request = None) -> Generator[Session, None, None]:  # type: ignore[assignment]
    """获取数据库会话

    事务策略说明
    ============
    本项目采用**混合事务管理**策略：

    1. **LLM 请求路径**：
       - 由 PluginMiddleware 统一管理事务
       - Service 层使用 db.flush() 使更改可见，但不提交
       - 请求结束时由中间件统一 commit 或 rollback
       - 例外：UsageService.record_usage() 会显式 commit，因为使用记录需要立即持久化

    2. **管理后台 API**：
       - 路由层显式调用 db.commit()
       - 提交后设置 request.state.tx_committed_by_route = True
       - 中间件看到此标志后跳过 commit，只负责 close

    3. **后台任务/调度器**：
       - 使用独立 Session（通过 create_session() 或 next(get_db())）
       - 自行管理事务生命周期

    使用方式
    ========
    - FastAPI 请求：通过 Depends(get_db) 注入，支持中间件管理的 session 复用
    - 非请求上下文：直接调用 get_db()，退化为独立 session 模式

    路由层提交事务示例
    ==================
    ```python
    @router.post("/example")
    async def example(request: Request, db: Session = Depends(get_db)):
        # ... 业务逻辑 ...
        db.commit()
        request.state.tx_committed_by_route = True  # 告知中间件已提交
        return {"message": "success"}
    ```

    注意事项
    ========
    - 本函数不自动提交事务
    - 异常时会自动回滚
    - 中间件管理模式下，session 关闭由中间件负责
    """
    # FastAPI 请求上下文：优先复用中间件绑定的 request.state.db
    if request is not None:
        existing_db = getattr(getattr(request, "state", None), "db", None)
        if isinstance(existing_db, Session):
            yield existing_db
            return

    # 确保引擎已初始化
    _ensure_engine()

    db = _SessionLocal()

    # 如果中间件声明会统一管理会话生命周期，则把 session 绑定到 request.state，
    # 并由中间件负责 commit/rollback/close（这里不关闭，避免流式响应提前释放会话）。
    managed_by_middleware = bool(
        request is not None
        and hasattr(request, "state")
        and getattr(request.state, "db_managed_by_middleware", False)
    )
    if managed_by_middleware:
        request.state.db = db
        db.info["managed_by_middleware"] = True

    try:
        yield db
        # 不再自动 commit，由业务代码显式管理事务
    except Exception:
        try:
            db.rollback()  # 失败时回滚未提交的事务
        except Exception as rollback_error:
            # 记录回滚错误（可能是 commit 正在进行中）
            logger.debug(f"回滚事务时出错（可忽略）: {rollback_error}")
        raise
    finally:
        if not managed_by_middleware:
            try:
                db.close()  # 确保连接返回池
            except Exception as close_error:
                # 记录关闭错误（如 IllegalStateChangeError）
                # 连接池会处理连接的回收
                logger.debug(f"关闭数据库连接时出错（可忽略）: {close_error}")


def create_session() -> Session:
    """
    创建一个新的数据库会话

    注意：调用者必须负责关闭会话
    推荐在 with 语句中使用或手动调用 session.close()

    示例:
        db = create_session()
        try:
            # 使用 db
        finally:
            db.close()
    """
    _ensure_engine()
    return _SessionLocal()


def get_db_url() -> str:
    """返回当前配置的数据库连接字符串（供脚本/测试使用）。"""
    return config.database_url


def init_db():
    """初始化数据库

    注意：数据库表结构由 Alembic 管理，部署时请运行 ./migrate.sh
    """
    logger.info("初始化数据库...")

    # 确保引擎已创建
    _ensure_engine()

    # 数据库表结构由 Alembic 迁移管理
    # 首次部署或更新后请运行: ./migrate.sh

    db = _SessionLocal()
    try:
        # 创建管理员账户（如果环境变量中配置了）
        init_admin_user(db)

        # 添加默认模型配置
        init_default_models(db)

        # 添加系统配置
        init_system_configs(db)

        db.commit()
        logger.info("数据库初始化完成")

    except Exception as e:
        logger.error(f"数据库初始化失败: {e}")
        db.rollback()
        raise
    finally:
        db.close()


def init_admin_user(db: Session):
    """从环境变量创建管理员账户"""
    # 检查是否使用默认凭据
    if config.admin_email == "admin@localhost" and config.admin_password == "admin123":
        logger.warning("使用默认管理员账户配置，建议修改为安全的凭据")

    # 检查是否已存在管理员
    existing_admin = (
        db.query(User)
        .filter((User.email == config.admin_email) | (User.username == config.admin_username))
        .first()
    )

    if existing_admin:
        logger.info(f"管理员账户已存在: {existing_admin.email}")
        return

    try:
        # 创建管理员账户
        admin = User(
            email=config.admin_email,
            username=config.admin_username,
            role=UserRole.ADMIN,
            quota_usd=1000.0,
            is_active=True,
        )
        admin.set_password(config.admin_password)

        db.add(admin)
        db.flush()  # 分配ID，但不提交事务（由外层 init_db 统一 commit）

        logger.info(f"创建管理员账户成功: {admin.email} ({admin.username})")
    except Exception as e:
        logger.error(f"创建管理员账户失败: {e}")
        raise


def init_default_models(db: Session):
    """初始化默认模型配置"""

    # 注意：作为中转代理服务，不再预设模型配置
    # 模型配置应该通过 GlobalModel 和 Model 表动态管理
    # 这个函数保留用于未来可能的默认模型初始化
    pass


def init_system_configs(db: Session):
    """初始化系统配置"""

    configs = [
        {"key": "default_user_quota_usd", "value": 10.0, "description": "新用户默认美元配额"},
        {"key": "rate_limit_per_minute", "value": 60, "description": "每分钟请求限制"},
        {"key": "enable_registration", "value": False, "description": "是否开放用户注册"},
        {"key": "require_email_verification", "value": False, "description": "是否需要邮箱验证"},
        {"key": "api_key_expire_days", "value": 365, "description": "API密钥过期天数"},
    ]

    for config_data in configs:
        existing = db.query(SystemConfig).filter_by(key=config_data["key"]).first()
        if not existing:
            config = SystemConfig(**config_data)
            db.add(config)
            logger.info(f"添加系统配置: {config_data['key']}")


def reset_db():
    """重置数据库（仅用于开发）"""
    logger.warning("重置数据库...")

    # 确保引擎已创建
    engine = _ensure_engine()

    Base.metadata.drop_all(bind=engine)
    init_db()
