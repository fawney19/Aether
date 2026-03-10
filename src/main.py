"""
主应用入口
采用模块化架构设计
"""

from __future__ import annotations

from contextlib import asynccontextmanager
from dataclasses import dataclass, field
from typing import TYPE_CHECKING, Any

import uvicorn
from fastapi import FastAPI, HTTPException
from fastapi.middleware.cors import CORSMiddleware

from src.api.admin import router as admin_router
from src.api.announcements import router as announcement_router

# API路由
from src.api.auth import router as auth_router
from src.api.dashboard import router as dashboard_router
from src.api.monitoring import router as monitoring_router
from src.api.payment import router as payment_router
from src.api.public import router as public_router
from src.api.user_me import router as me_router
from src.api.wallet import router as wallet_router
from src.clients.http_client import HTTPClientPool, close_http_clients

# 核心模块
from src.config import config
from src.core.exceptions import ExceptionHandlers, ProxyException
from src.core.logger import logger
from src.core.modules import get_module_registry
from src.database import init_db
from src.middleware.plugin_middleware import PluginMiddleware
from src.plugins.manager import get_plugin_manager

if TYPE_CHECKING:
    from redis.asyncio.client import Redis

    from src.core.modules.base import ModuleDefinition
    from src.plugins.manager import PluginManager
    from src.services.model.fetch_scheduler import ModelFetchScheduler
    from src.services.provider_keys.pool_quota_probe_scheduler import PoolQuotaProbeScheduler
    from src.services.rate_limit.concurrency_manager import ConcurrencyManager
    from src.services.system.maintenance_scheduler import MaintenanceScheduler
    from src.services.system.scheduler import TaskScheduler
    from src.services.task.polling.task_poller import TaskPollerService
    from src.services.usage.quota_scheduler import QuotaScheduler
    from src.utils.task_coordinator import StartupTaskCoordinator


async def initialize_providers() -> None:
    """从数据库初始化提供商（仅用于日志记录，使用轻量查询）"""
    from sqlalchemy import func
    from sqlalchemy.orm import Session

    from src.database.database import create_session
    from src.models.database import Provider, ProviderEndpoint

    try:
        db: Session = create_session()

        try:
            # 使用聚合查询代替全量加载 ORM 对象，大幅减少内存占用
            results = (
                db.query(
                    Provider.name,
                    func.count(ProviderEndpoint.id).label("total"),
                    func.count(func.nullif(ProviderEndpoint.is_active, False)).label("active"),
                )
                .outerjoin(ProviderEndpoint, Provider.id == ProviderEndpoint.provider_id)
                .filter(Provider.is_active.is_(True))
                .group_by(Provider.id, Provider.name, Provider.provider_priority)
                .order_by(Provider.provider_priority.asc())
                .all()
            )

            if not results:
                logger.warning("数据库中未找到活跃的提供商")
                return

            logger.info(f"从数据库加载了 {len(results)} 个活跃提供商")
            for name, total, active in results:
                logger.info(f"提供商: {name} (端点: {active}/{total})")

        finally:
            db.close()

    except Exception:
        logger.exception("从数据库初始化提供商失败")


@dataclass
class LifecycleState:
    """应用生命周期阶段共享的运行时状态。"""

    redis_client: Redis | None = None
    concurrency_manager: ConcurrencyManager | None = None
    plugin_manager: PluginManager | None = None
    available_modules: list[ModuleDefinition] = field(default_factory=list)
    task_coordinator: StartupTaskCoordinator | None = None
    quota_scheduler: QuotaScheduler | None = None
    maintenance_scheduler: MaintenanceScheduler | None = None
    model_fetch_scheduler: ModelFetchScheduler | None = None
    pool_quota_probe_scheduler: PoolQuotaProbeScheduler | None = None
    task_poller: TaskPollerService | None = None
    task_scheduler: TaskScheduler | None = None


def _configure_uvicorn_access_log() -> None:
    """禁用 uvicorn access 日志（在子进程中执行）。"""
    import logging

    logging.getLogger("uvicorn.access").setLevel(logging.CRITICAL)
    logging.getLogger("uvicorn.access").disabled = True


def _log_startup_banner() -> None:
    logger.info("=" * 60)
    from src import __version__

    logger.info(f"AI Proxy v{__version__} - GlobalModel Architecture")
    logger.info("=" * 60)


def _validate_security_or_raise() -> None:
    """启动前安全配置校验。"""
    security_errors = config.validate_security_config()
    if security_errors:
        for error in security_errors:
            logger.error(f"[SECURITY] {error}")
        if config.environment == "production":
            raise RuntimeError(
                "Security configuration errors detected. "
                "Please fix the following issues before starting in production:\n"
                + "\n".join(f"  - {e}" for e in security_errors)
            )


async def _initialize_core_infrastructure(state: LifecycleState) -> None:
    """初始化数据库、缓存、并发与基础后台组件。"""
    # 记录启动警告（密码、连接池、JWT 等）
    config.log_startup_warnings()

    # 初始化数据库
    logger.info("初始化数据库...")
    init_db()

    # 从数据库初始化提供商
    await initialize_providers()

    # 初始化全局HTTP客户端池
    logger.info("初始化全局HTTP客户端池...")
    HTTPClientPool.get_default_client()  # 预创建默认客户端

    # 初始化全局Redis客户端（可根据配置降级为内存模式）
    logger.info("初始化全局Redis客户端...")
    from src.clients.redis_client import get_redis_client

    try:
        state.redis_client = await get_redis_client(require_redis=config.require_redis)
        if state.redis_client:
            logger.info("[OK] Redis客户端初始化成功，缓存亲和性功能已启用")
        else:
            logger.warning(
                "[WARN] Redis未启用或连接失败，将使用内存缓存亲和性（仅适用于单实例/开发环境）"
            )
    except RuntimeError as e:
        if config.require_redis:
            logger.exception("[ERROR] Redis连接失败，应用启动中止")
            raise
        logger.warning(f"Redis连接失败，但配置允许降级，将继续使用内存模式: {e}")
        state.redis_client = None

    # 初始化并发管理器（内部会使用Redis）
    logger.info("初始化并发管理器...")
    from src.services.rate_limit.concurrency_manager import get_concurrency_manager

    state.concurrency_manager = await get_concurrency_manager()

    # 初始化批量提交器（提升数据库并发能力）
    logger.info("初始化批量提交器...")
    from src.core.batch_committer import init_batch_committer

    await init_batch_committer()
    logger.info("[OK] 批量提交器已启动，数据库写入性能优化已启用")

    # 初始化 Codex 配额异步同步器（请求路径仅投递事件）
    logger.info("初始化 Codex 配额异步同步器...")
    from src.services.provider_keys.codex_quota_sync_dispatcher import (
        init_codex_quota_sync_dispatcher,
    )

    await init_codex_quota_sync_dispatcher()
    logger.info("[OK] Codex 配额异步同步器已启动")

    # 初始化 Usage 队列消费者（可选）
    if config.usage_queue_enabled:
        logger.info("初始化 Usage 队列消费者...")
        from src.services.usage.consumer_streams import start_usage_queue_consumer

        await start_usage_queue_consumer()


async def _initialize_plugins_and_modules(app: FastAPI, state: LifecycleState) -> None:
    """初始化插件系统、模块系统，并注册路由与钩子。"""
    # 初始化插件系统
    logger.info("初始化插件系统...")
    state.plugin_manager = get_plugin_manager()
    init_results = await state.plugin_manager.initialize_all()
    successful = sum(1 for success in init_results.values() if success)
    logger.info(f"插件初始化完成: {successful}/{len(init_results)} 个插件成功启动")

    # 注册格式转换器
    logger.info("注册格式转换器...")
    from src.core.api_format.conversion.registry import register_default_normalizers

    register_default_normalizers()

    # 初始化功能模块系统
    logger.info("初始化功能模块系统...")
    from src.modules import ALL_MODULES

    module_registry = get_module_registry()

    # 注入配置后端，消除 core/modules→services 的运行时 lazy import
    from src.services.system.config import SystemConfigService

    module_registry.set_config_backend(SystemConfigService)  # type: ignore[arg-type]

    for module in ALL_MODULES:
        module_registry.register(module)

    # 注册模块钩子
    from src.core.modules.hooks import get_hook_dispatcher

    hook_dispatcher = get_hook_dispatcher()
    for module in ALL_MODULES:
        for hook_name, handler in module.hooks.items():
            hook_dispatcher.register(hook_name, module.metadata.name, handler)

    # 注册可用模块的路由
    # 注意：模块的 router 自带 prefix，api_prefix 字段仅用于日志和文档
    state.available_modules = module_registry.get_available_modules()
    for module in state.available_modules:
        if module.router_factory:
            router = module.router_factory()
            app.include_router(router)
            prefix = module.metadata.api_prefix or "(default)"
            logger.info(f"模块 [{module.metadata.name}] 路由已注册: {prefix}")

        # 执行启动钩子
        if module.on_startup:
            await module.on_startup()

    logger.info(f"功能模块初始化完成: {len(state.available_modules)}/{len(ALL_MODULES)} 个模块可用")

    # 显式 bootstrap provider plugins（注册 envelope/enricher 等）
    # 使 core/provider_oauth_utils 不需要在运行时 lazy import services 层
    from src.services.provider.envelope import ensure_providers_bootstrapped

    ensure_providers_bootstrapped()

    # 显式触发 parsers 注册（使 core/stream_types 不需要 lazy import api 层）
    from src.api.handlers.base.parsers import register_default_parsers

    register_default_parsers()


async def _start_background_services(state: LifecycleState) -> None:
    """启动调度器与后台轮询服务。"""
    # 启动月卡额度重置调度器（仅一个 worker 执行）
    logger.info("启动月卡额度重置调度器...")
    from src.services.model.fetch_scheduler import get_model_fetch_scheduler
    from src.services.provider_keys.pool_quota_probe_scheduler import (
        get_pool_quota_probe_scheduler,
    )
    from src.services.system.maintenance_scheduler import get_maintenance_scheduler
    from src.services.task.polling.task_poller import get_task_poller
    from src.services.usage.quota_scheduler import get_quota_scheduler
    from src.utils.task_coordinator import StartupTaskCoordinator

    state.quota_scheduler = get_quota_scheduler()
    state.maintenance_scheduler = get_maintenance_scheduler()
    state.model_fetch_scheduler = get_model_fetch_scheduler()
    state.pool_quota_probe_scheduler = get_pool_quota_probe_scheduler()
    state.task_poller = get_task_poller()
    state.task_coordinator = StartupTaskCoordinator(state.redis_client)

    # 启动额度调度器
    quota_scheduler_active = await state.task_coordinator.acquire("quota_scheduler")
    if quota_scheduler_active:
        await state.quota_scheduler.start()
    else:
        logger.info("检测到其他 worker 已运行额度调度器，本实例跳过")
        state.quota_scheduler = None

    # 启动维护调度器
    maintenance_scheduler_active = await state.task_coordinator.acquire("maintenance_scheduler")
    if maintenance_scheduler_active:
        logger.info("启动系统维护调度器...")
        await state.maintenance_scheduler.start()
    else:
        logger.info("检测到其他 worker 已运行维护调度器，本实例跳过")
        state.maintenance_scheduler = None

    # 启动模型自动获取调度器
    model_fetch_scheduler_active = await state.task_coordinator.acquire("model_fetch_scheduler")
    if model_fetch_scheduler_active:
        logger.info("启动模型自动获取调度器...")
        await state.model_fetch_scheduler.start()
    else:
        logger.info("检测到其他 worker 已运行模型获取调度器，本实例跳过")
        state.model_fetch_scheduler = None

    # 启动号池额度主动探测调度器
    pool_quota_probe_scheduler_active = await state.task_coordinator.acquire(
        "pool_quota_probe_scheduler"
    )
    if pool_quota_probe_scheduler_active:
        logger.info("启动号池额度主动探测调度器...")
        await state.pool_quota_probe_scheduler.start()
    else:
        logger.info("检测到其他 worker 已运行号池额度主动探测调度器，本实例跳过")
        state.pool_quota_probe_scheduler = None

    # 启动异步任务轮询服务（当前仅视频）
    task_poller_active = await state.task_coordinator.acquire("task_poller:video")
    if task_poller_active:
        logger.info("启动 TaskPoller（video）...")
        await state.task_poller.start()
    else:
        logger.info("检测到其他 worker 已运行 TaskPoller（video），本实例跳过")
        state.task_poller = None

    # 启动统一的定时任务调度器
    from src.services.system.scheduler import get_scheduler

    state.task_scheduler = get_scheduler()
    state.task_scheduler.start()


async def _run_startup(app: FastAPI) -> LifecycleState:
    """执行完整启动流程并返回生命周期状态。"""
    _configure_uvicorn_access_log()
    _log_startup_banner()
    _validate_security_or_raise()

    state = LifecycleState()
    await _initialize_core_infrastructure(state)
    await _initialize_plugins_and_modules(app, state)

    logger.info(f"服务启动成功: http://{config.host}:{config.port}")
    logger.info("=" * 60)

    await _start_background_services(state)
    return state


async def _run_shutdown(state: LifecycleState) -> None:
    """执行完整关闭流程。"""
    logger.info("正在关闭服务...")

    # 停止 Codex 配额异步同步器（停止前会 flush 待同步事件）
    logger.info("停止 Codex 配额异步同步器...")
    from src.services.provider_keys.codex_quota_sync_dispatcher import (
        shutdown_codex_quota_sync_dispatcher,
    )

    await shutdown_codex_quota_sync_dispatcher()
    logger.info("[OK] Codex 配额异步同步器已停止")

    # 停止批量提交器（确保所有待提交的数据都被保存）
    logger.info("停止批量提交器...")
    from src.core.batch_committer import shutdown_batch_committer

    await shutdown_batch_committer()
    logger.info("[OK] 批量提交器已停止，所有待提交数据已保存")

    # 停止 Usage 队列消费者
    if config.usage_queue_enabled:
        logger.info("停止 Usage 队列消费者...")
        from src.services.usage.consumer_streams import stop_usage_queue_consumer

        await stop_usage_queue_consumer()

    # 停止维护调度器
    if state.maintenance_scheduler:
        logger.info("停止系统维护调度器...")
        await state.maintenance_scheduler.stop()
        if state.task_coordinator:
            await state.task_coordinator.release("maintenance_scheduler")

    # 停止月卡额度重置调度器，并释放分布式锁
    if state.quota_scheduler:
        logger.info("停止月卡额度重置调度器...")
        await state.quota_scheduler.stop()
        if state.task_coordinator:
            await state.task_coordinator.release("quota_scheduler")

    # 停止模型自动获取调度器
    if state.model_fetch_scheduler:
        logger.info("停止模型自动获取调度器...")
        await state.model_fetch_scheduler.stop()
        if state.task_coordinator:
            await state.task_coordinator.release("model_fetch_scheduler")

    if state.pool_quota_probe_scheduler:
        logger.info("停止号池额度主动探测调度器...")
        await state.pool_quota_probe_scheduler.stop()
        if state.task_coordinator:
            await state.task_coordinator.release("pool_quota_probe_scheduler")

    if state.task_poller:
        logger.info("停止 TaskPoller（video）...")
        await state.task_poller.stop()
        if state.task_coordinator:
            await state.task_coordinator.release("task_poller:video")

    # 停止统一的定时任务调度器
    logger.info("停止定时任务调度器...")
    if state.task_scheduler:
        state.task_scheduler.stop()

    # 关闭插件系统
    logger.info("关闭插件系统...")
    if state.plugin_manager:
        await state.plugin_manager.shutdown_all()

    # 关闭功能模块
    logger.info("关闭功能模块...")
    for module in state.available_modules:
        if module.on_shutdown:
            await module.on_shutdown()

    # 关闭并发管理器
    logger.info("关闭并发管理器...")
    if state.concurrency_manager:
        await state.concurrency_manager.close()

    # 关闭全局Redis客户端
    logger.info("关闭全局Redis客户端...")
    from src.clients.redis_client import close_redis_client

    await close_redis_client()

    # 关闭HTTP客户端池
    logger.info("关闭HTTP客户端池...")
    await close_http_clients()

    logger.info("服务已关闭")


@asynccontextmanager
async def lifespan(app: FastAPI) -> Any:
    """应用生命周期管理"""
    state = await _run_startup(app)
    try:
        yield  # 应用运行期间
    finally:
        await _run_shutdown(state)


from src import __version__ as app_version

# OpenAPI Tags 元数据定义
openapi_tags = [
    {
        "name": "Authentication",
        "description": "用户认证相关接口，包括登录、注册、令牌刷新等",
    },
    {
        "name": "User Profile",
        "description": "用户个人信息管理，包括 API 密钥、使用统计、偏好设置等",
    },
    {
        "name": "Management Tokens",
        "description": "管理令牌，用于 CLI 工具等外部应用的认证",
    },
    {
        "name": "Dashboard",
        "description": "仪表盘统计数据，包括请求量、Token 用量、成本等概览信息",
    },
    {
        "name": "Announcements",
        "description": "系统公告管理",
    },
    {
        "name": "Monitoring",
        "description": "用户监控与审计日志查询",
    },
    {
        "name": "Admin - Users",
        "description": "用户管理（管理员）",
    },
    {
        "name": "Admin - Providers",
        "description": "提供商管理（管理员）",
    },
    {
        "name": "Admin - Endpoints",
        "description": "端点管理（管理员）",
    },
    {
        "name": "Admin - Models",
        "description": "模型管理（管理员）",
    },
    {
        "name": "Admin - API Keys",
        "description": "API 密钥管理（管理员）",
    },
    {
        "name": "Admin - Usage",
        "description": "使用统计管理（管理员）",
    },
    {
        "name": "Admin - Monitoring",
        "description": "系统监控（管理员）",
    },
    {
        "name": "Admin - Security",
        "description": "安全配置管理（管理员）",
    },
    {
        "name": "Admin - System",
        "description": "系统配置管理（管理员）",
    },
    {
        "name": "Claude API",
        "description": "Claude API 代理接口，兼容 Anthropic Claude API 格式",
    },
    {
        "name": "OpenAI API",
        "description": "OpenAI API 代理接口，兼容 OpenAI Chat Completions API 格式",
    },
    {
        "name": "Gemini API",
        "description": "Gemini API 代理接口，兼容 Google Gemini API 格式",
    },
    {
        "name": "Gemini Files API",
        "description": "Gemini Files API 代理接口，支持文件上传、查询、删除等操作",
    },
    {
        "name": "System Catalog",
        "description": "系统目录接口，用于获取可用模型列表等",
    },
]

app = FastAPI(
    title="Aether AI Gateway",
    version=app_version,
    lifespan=lifespan,
    docs_url="/docs" if config.docs_enabled else None,
    redoc_url="/redoc" if config.docs_enabled else None,
    openapi_url="/openapi.json" if config.docs_enabled else None,
    openapi_tags=openapi_tags,
)

# 注册全局异常处理器
# 注意：异常处理器的注册顺序很重要，必须先注册更通用的异常类型，再注册具体的
# ProxyException 处理器的启用由配置控制：
# - propagate_provider_exceptions=True (默认): 不注册，让异常传播到路由层以记录 provider_request_headers
# - propagate_provider_exceptions=False: 注册全局处理器统一处理
if not config.propagate_provider_exceptions:
    app.add_exception_handler(ProxyException, ExceptionHandlers.handle_proxy_exception)  # type: ignore[arg-type]
app.add_exception_handler(Exception, ExceptionHandlers.handle_generic_exception)  # type: ignore[arg-type]
app.add_exception_handler(HTTPException, ExceptionHandlers.handle_http_exception)  # type: ignore[arg-type]

# 添加插件中间件（包含认证、审计、速率限制等功能）
app.add_middleware(PluginMiddleware)

# CORS配置 - 使用环境变量配置允许的域名
# 生产环境必须通过 CORS_ORIGINS 环境变量显式指定允许的域名
# 开发环境默认允许本地前端访问
if config.cors_origins:
    # CORS_ORIGINS=* 时自动禁用 credentials（浏览器规范要求）
    allow_credentials = config.cors_allow_credentials and "*" not in config.cors_origins
    app.add_middleware(
        CORSMiddleware,
        allow_origins=config.cors_origins,  # 使用配置的白名单
        allow_credentials=allow_credentials,
        allow_methods=["GET", "POST", "PUT", "DELETE", "PATCH", "OPTIONS"],
        allow_headers=["*"],
        expose_headers=["*"],
    )
    logger.info(f"CORS已启用,允许的源: {config.cors_origins}, credentials: {allow_credentials}")
else:
    # 没有配置CORS源,不允许跨域
    logger.warning(
        f"CORS未配置,不允许跨域请求。如需启用CORS,请设置 CORS_ORIGINS 环境变量(当前环境: {config.environment})"
    )

# 注册路由
app.include_router(auth_router)  # 认证相关
app.include_router(admin_router)  # 管理员端点
app.include_router(me_router)  # 用户个人端点
app.include_router(wallet_router)  # 钱包端点
app.include_router(payment_router)  # 支付回调端点
app.include_router(announcement_router)  # 公告系统
app.include_router(dashboard_router)  # 仪表盘端点
app.include_router(public_router)  # 公开API端点（用户可查看提供商和模型）
app.include_router(monitoring_router)  # 监控端点


def main() -> Any:
    # 初始化新日志系统
    debug_mode = config.environment == "development"
    # 日志系统已在导入时自动初始化

    # Parse log level
    log_level = config.log_level.split()[0].lower()
    if log_level not in ["debug", "info", "warning", "error", "critical"]:
        log_level = "info"

    # 自定义uvicorn日志配置,完全禁用access日志
    uvicorn_log_config = {
        "version": 1,
        "disable_existing_loggers": False,
        "formatters": {
            "default": {
                "format": "%(levelprefix)s %(message)s",
                "use_colors": True,
            },
        },
        "handlers": {
            "default": {
                "formatter": "default",
                "class": "logging.StreamHandler",
                "stream": "ext://sys.stderr",
            },
        },
        "loggers": {
            "uvicorn": {"handlers": ["default"], "level": log_level.upper()},
            "uvicorn.error": {"level": log_level.upper()},
            "uvicorn.access": {"handlers": [], "level": "CRITICAL"},  # 禁用access日志
        },
    }

    # Start server
    # 根据环境设置热重载
    is_dev = config.environment == "development"
    uvicorn.run(
        "src.main:app",
        host=config.host,
        port=config.port,
        log_level=log_level,
        reload=is_dev,
        reload_dirs=["src"] if is_dev else None,
        access_log=False,  # 禁用 uvicorn 访问日志，使用自定义中间件
        log_config=uvicorn_log_config,  # 使用自定义日志配置
    )


if __name__ == "__main__":
    # 使用安全的方式清屏，避免命令注入风险
    try:
        import os

        if os.name == "nt":  # Windows
            os.system("cls")
        else:  # Unix/Linux/MacOS
            print("\033[2J\033[H", end="")  # ANSI escape sequence
    except:
        pass  # 清屏失败不影响程序运行

    main()
