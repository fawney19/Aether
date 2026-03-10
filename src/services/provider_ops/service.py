"""
Provider 操作服务

提供操作执行、凭据管理、缓存等业务逻辑。
"""

from __future__ import annotations

import asyncio
import os
from dataclasses import asdict
from datetime import datetime, timezone
from typing import Any

from sqlalchemy.orm import Session
from sqlalchemy.orm.attributes import flag_modified

from src.config import config
from src.core.cache_service import CacheService
from src.core.crypto import CryptoService
from src.core.logger import logger
from src.database import create_session
from src.models.database import Provider
from src.services.provider_ops.architectures import ProviderConnector
from src.services.provider_ops.registry import get_registry
from src.services.provider_ops.types import (
    SENSITIVE_CREDENTIAL_FIELDS,
    ActionResult,
    ActionStatus,
    BalanceInfo,
    ConnectorAuthType,
    ConnectorState,
    ConnectorStatus,
    ProviderActionType,
    ProviderOpsConfig,
)

# 余额缓存 TTL（24 小时）
BALANCE_CACHE_TTL = 86400
# 认证失败缓存 TTL（60 秒，避免频繁重试但允许用户修正后快速重试）
AUTH_FAILED_CACHE_TTL = 60

# 后台余额刷新并发限制（避免启动时耗尽连接池）
# 使用较小的值（3）确保不会对连接池造成过大压力
_balance_refresh_semaphore: asyncio.Semaphore | None = None

# 正在异步刷新余额的 provider 集合（per-provider 防重入）
_refreshing_providers: set[str] = set()


def _get_balance_refresh_semaphore() -> asyncio.Semaphore:
    """获取余额刷新信号量（延迟初始化）"""
    global _balance_refresh_semaphore
    if _balance_refresh_semaphore is None:
        # 限制为 3 个并发，确保后台任务不会占用太多连接
        _balance_refresh_semaphore = asyncio.Semaphore(3)
    return _balance_refresh_semaphore


def _get_batch_balance_concurrency() -> int:
    """
    动态计算批量余额查询的并发限制

    计算逻辑：
    1. 优先使用环境变量 BATCH_BALANCE_CONCURRENCY
    2. 否则根据连接池大小自动计算（取 40% 的连接池容量）
    3. 限制在 [3, 15] 范围内

    Returns:
        并发限制数
    """
    # 优先使用环境变量
    env_value = os.getenv("BATCH_BALANCE_CONCURRENCY")
    if env_value:
        try:
            return max(1, int(env_value))
        except ValueError:
            pass

    # 根据连接池大小自动计算
    # 连接池容量 = pool_size + max_overflow
    pool_capacity = config.db_pool_size + config.db_max_overflow

    # 取 40% 的连接池容量，保留 60% 给其他请求
    # 最小 3（保证基本并发），最大 15（避免过度并发）
    calculated = int(pool_capacity * 0.4)
    return max(3, min(calculated, 15))


class ProviderOpsService:
    """
    Provider 操作服务

    提供：
    - 凭据管理（加密存储、读取）
    - 连接管理（建立、断开、状态检查）
    - 操作执行（余额查询、签到等）
    """

    # 凭据中需要加密的字段
    SENSITIVE_FIELDS = SENSITIVE_CREDENTIAL_FIELDS

    def __init__(self, db: Session):
        self.db = db
        self.crypto = CryptoService()

        # 连接器缓存 {provider_id: ProviderConnector}
        self._connectors: dict[str, ProviderConnector] = {}

    def _release_db_connection_before_await(self) -> None:
        """
        Release pooled DB connection before long awaits (network/Redis).

        SQLAlchemy Session will keep a connection checked out while a transaction is open,
        even for read-only queries. In async code, this can exhaust the pool if we `await`
        network I/O while holding that transaction.

        Safety:
        - Only commits when the session has no pending changes (new/dirty/deleted).
        - Temporarily disables expire_on_commit to avoid unexpected lazy reloads.
        """
        try:
            has_pending_changes = bool(self.db.new) or bool(self.db.dirty) or bool(self.db.deleted)
        except Exception:
            has_pending_changes = False

        if has_pending_changes:
            return

        try:
            if not self.db.in_transaction():
                return
        except Exception:
            return

        original_expire_on_commit = getattr(self.db, "expire_on_commit", True)
        self.db.expire_on_commit = False
        try:
            self.db.commit()
        except Exception:
            try:
                self.db.rollback()
            except Exception:
                pass
        finally:
            self.db.expire_on_commit = original_expire_on_commit

    # ==================== 配置管理 ====================

    def get_config(self, provider_id: str) -> ProviderOpsConfig | None:
        """
        获取 Provider 的操作配置

        Args:
            provider_id: Provider ID

        Returns:
            配置对象，未配置则返回 None
        """
        provider = self._get_provider(provider_id)
        if not provider:
            return None

        config_data = (provider.config or {}).get("provider_ops")
        if not config_data:
            return None

        return ProviderOpsConfig.from_dict(config_data)

    def save_config(
        self,
        provider_id: str,
        config: ProviderOpsConfig,
    ) -> bool:
        """
        保存 Provider 的操作配置

        Args:
            provider_id: Provider ID
            config: 配置对象

        Returns:
            是否保存成功
        """
        provider = self._get_provider(provider_id)
        if not provider:
            return False

        # 加密敏感凭据
        encrypted_credentials = self._encrypt_credentials(config.connector_credentials)
        logger.debug(
            "加密凭据: provider_id={}, input_keys={}, output_keys={}, has_api_key={}",
            provider_id,
            list(config.connector_credentials.keys()),
            list(encrypted_credentials.keys()),
            bool(config.connector_credentials.get("api_key")),
        )

        # 构建配置
        config_dict = config.to_dict()
        config_dict["connector"]["credentials"] = encrypted_credentials

        # 更新 Provider 配置
        provider_config = dict(provider.config or {})
        provider_config["provider_ops"] = config_dict
        provider.config = provider_config

        self.db.commit()

        # 清除连接器缓存
        if provider_id in self._connectors:
            del self._connectors[provider_id]

        logger.info("保存 Provider 操作配置: provider_id={}", provider_id)
        return True

    def delete_config(self, provider_id: str) -> bool:
        """
        删除 Provider 的操作配置

        Args:
            provider_id: Provider ID

        Returns:
            是否删除成功
        """
        provider = self._get_provider(provider_id)
        if not provider:
            return False

        provider_config = dict(provider.config or {})
        if "provider_ops" in provider_config:
            del provider_config["provider_ops"]
            provider.config = provider_config
            self.db.commit()

        # 清除连接器缓存
        if provider_id in self._connectors:
            del self._connectors[provider_id]

        return True

    # ==================== 连接管理 ====================

    async def connect(
        self,
        provider_id: str,
        credentials: dict[str, Any] | None = None,
    ) -> tuple[bool, str]:
        """
        建立与 Provider 的连接

        Args:
            provider_id: Provider ID
            credentials: 凭据（如果为 None 则使用已保存的凭据）

        Returns:
            (是否成功, 消息)
        """
        provider = self._get_provider(provider_id)
        if not provider:
            return False, "Provider 不存在"

        config = self.get_config(provider_id)
        if not config:
            return False, "未配置操作设置"

        # 获取架构
        registry = get_registry()
        architecture = registry.get_or_default(config.architecture_id)

        # 获取 base_url：优先从 config 读取
        base_url = config.base_url or self._get_provider_base_url(provider)
        if not base_url:
            return False, "Provider 未配置 base_url"

        # 创建连接器
        try:
            connector = architecture.get_connector(
                base_url=base_url,
                auth_type=config.connector_auth_type,
                config=config.connector_config,
            )
        except ValueError as e:
            return False, str(e)

        # 使用提供的凭据或已保存的凭据
        if credentials:
            actual_credentials = credentials
        else:
            actual_credentials = self._decrypt_credentials(config.connector_credentials)

        if not actual_credentials:
            return False, "未提供凭据"

        # Avoid holding a DB connection while awaiting network I/O.
        self._release_db_connection_before_await()

        # 建立连接
        logger.info(
            "尝试连接: provider_id={}, credentials_keys={}",
            provider_id,
            list(actual_credentials.keys()),
        )
        # 注册凭据更新回调（Token Rotation 场景持久化新 refresh_token）
        # 必须在 connect 之前注册，因为 connect 内部可能已经触发 Token Rotation
        connector._on_credentials_updated = (
            lambda updated, pid=provider_id: self._persist_updated_credentials(pid, updated)
        )
        success = await connector.connect(actual_credentials)
        if success:
            self._connectors[provider_id] = connector
            return True, "连接成功"
        else:
            state = connector.get_state()
            return False, state.last_error or "连接失败"

    async def disconnect(self, provider_id: str) -> bool:
        """
        断开与 Provider 的连接

        Args:
            provider_id: Provider ID

        Returns:
            是否成功
        """
        connector = self._connectors.get(provider_id)
        if connector:
            await connector.disconnect()
            del self._connectors[provider_id]
        return True

    def get_connection_status(self, provider_id: str) -> ConnectorState:
        """
        获取连接状态

        Args:
            provider_id: Provider ID

        Returns:
            连接器状态
        """
        connector = self._connectors.get(provider_id)
        if connector:
            return connector.get_state()

        # 未连接
        config = self.get_config(provider_id)
        return ConnectorState(
            status=ConnectorStatus.DISCONNECTED,
            auth_type=config.connector_auth_type if config else ConnectorAuthType.NONE,
        )

    # ==================== 操作执行 ====================

    async def execute_action(
        self,
        provider_id: str,
        action_type: ProviderActionType,
        action_config: dict[str, Any] | None = None,
    ) -> ActionResult:
        """
        执行操作

        Args:
            provider_id: Provider ID
            action_type: 操作类型
            action_config: 操作配置（覆盖默认配置）

        Returns:
            操作结果
        """
        # 检查连接状态
        connector = self._connectors.get(provider_id)
        if not connector:
            # 尝试自动连接
            success, message = await self.connect(provider_id)
            if not success:
                return ActionResult(
                    status=ActionStatus.AUTH_FAILED,
                    action_type=action_type,
                    message=f"连接失败: {message}",
                )
            connector = self._connectors.get(provider_id)

        # Avoid holding a DB connection while awaiting authentication checks.
        self._release_db_connection_before_await()
        if not connector or not await connector.is_authenticated():
            return ActionResult(
                status=ActionStatus.AUTH_EXPIRED,
                action_type=action_type,
                message="认证已过期，请重新连接",
            )

        # 获取配置
        config = self.get_config(provider_id)
        if not config:
            return ActionResult(
                status=ActionStatus.NOT_CONFIGURED,
                action_type=action_type,
                message="未配置操作设置",
            )

        # 获取架构
        registry = get_registry()
        architecture = registry.get_or_default(config.architecture_id)

        # 检查是否支持该操作
        if not architecture.supports_action(action_type):
            return ActionResult(
                status=ActionStatus.NOT_SUPPORTED,
                action_type=action_type,
                message=f"架构 {architecture.architecture_id} 不支持 {action_type.value} 操作",
            )

        # 合并操作配置
        saved_action_config = config.actions.get(action_type.value, {}).get("config", {})
        merged_config = {**saved_action_config, **(action_config or {})}

        # 注入 credentials 元信息（如是否配置了 Cookie），供 Action 使用
        decrypted_credentials = self._decrypt_credentials(config.connector_credentials)
        if decrypted_credentials.get("cookie"):
            merged_config["_has_cookie"] = True

        # 创建操作实例
        action = architecture.get_action(action_type, merged_config)

        # Avoid holding a DB connection while awaiting the upstream action.
        self._release_db_connection_before_await()

        # 执行操作
        async with connector.get_client() as client:
            result = await action.execute(client)

        return result

    async def query_balance(
        self,
        provider_id: str,
        config: dict[str, Any] | None = None,
    ) -> ActionResult:
        """
        查询余额（快捷方法）

        Args:
            provider_id: Provider ID
            config: 操作配置

        Returns:
            操作结果
        """
        result = await self.execute_action(provider_id, ProviderActionType.QUERY_BALANCE, config)

        # 成功或 auth_expired 时缓存（auth_expired 带有 cookie_expired 信息供前端显示警告）
        if result.status in (ActionStatus.SUCCESS, ActionStatus.AUTH_EXPIRED) and result.data:
            await self._cache_balance(provider_id, result)
        # auth_failed 时也缓存（使用较短 TTL），避免前端无限显示"加载中..."
        elif result.status == ActionStatus.AUTH_FAILED:
            await self._cache_auth_failed(provider_id, result)

        return result

    async def query_balance_with_cache(
        self,
        provider_id: str,
        trigger_refresh: bool = True,
        allow_sync_query: bool = True,
    ) -> ActionResult:
        """
        查询余额（优先返回缓存，可触发异步刷新）

        Args:
            provider_id: Provider ID
            trigger_refresh: 是否触发后台异步刷新
            allow_sync_query: 缓存未命中时是否允许同步查询（False 时仅返回缓存或触发异步刷新）

        Returns:
            操作结果（可能是缓存的）
        """
        # Avoid holding a DB connection while awaiting Redis/cache I/O.
        self._release_db_connection_before_await()

        # 尝试从缓存获取
        cached = await self._get_cached_balance(provider_id)

        if cached:
            # 有缓存，可选触发后台刷新
            if trigger_refresh:
                # 后台任务内部已处理异常并记录日志，无需额外回调
                from src.utils.async_utils import safe_create_task

                safe_create_task(self._refresh_balance_async(provider_id))
            return cached

        # 没有缓存
        if allow_sync_query:
            # 同步查询一次（首次访问）
            logger.info("余额缓存未命中，同步查询: provider_id={}", provider_id)
            return await self.query_balance(provider_id)
        else:
            # 仅触发异步刷新，立即返回
            logger.debug("余额缓存未命中，触发异步刷新: provider_id={}", provider_id)
            from src.utils.async_utils import safe_create_task

            safe_create_task(self._refresh_balance_async(provider_id))
            return ActionResult(
                status=ActionStatus.PENDING,
                action_type=ProviderActionType.QUERY_BALANCE,
                message="余额数据加载中，请稍后刷新",
            )

    async def _refresh_balance_async(self, provider_id: str) -> None:
        """
        后台异步刷新余额（使用独立的数据库 session）

        注意：这是一个后台任务，使用独立的短生命周期 session，
        避免长时间占用连接池资源。

        使用信号量限制并发数，避免启动时多个刷新任务同时运行导致连接池耗尽。
        使用 _refreshing_providers 集合防止同一 provider 被并发刷新。
        """
        # per-provider 防重入
        if provider_id in _refreshing_providers:
            logger.debug("异步刷新余额跳过（已在刷新中）: provider_id={}", provider_id)
            return
        _refreshing_providers.add(provider_id)

        semaphore = _get_balance_refresh_semaphore()

        # 尝试获取信号量，如果无法立即获取则跳过本次刷新
        # 这样可以避免在连接池紧张时阻塞
        try:
            # 使用 wait_for 设置超时，避免无限等待
            await asyncio.wait_for(semaphore.acquire(), timeout=5.0)
        except asyncio.TimeoutError:
            logger.debug("异步刷新余额跳过（并发限制）: provider_id={}", provider_id)
            _refreshing_providers.discard(provider_id)
            return

        db = None
        try:
            # 后台任务需要创建独立的 session，因为原请求的 session 可能已关闭
            db = create_session()
            service = ProviderOpsService(db)
            await service.query_balance(provider_id)
        except Exception as e:
            logger.warning("异步刷新余额失败: provider_id={}, error={}", provider_id, e)
        finally:
            # 确保 session 被关闭，归还连接到连接池
            if db is not None:
                try:
                    db.close()
                except Exception:
                    pass
            # 释放信号量并移除防重入标记
            semaphore.release()
            _refreshing_providers.discard(provider_id)

    async def _clear_balance_cache(self, provider_id: str) -> None:
        """清除余额缓存"""
        cache_key = f"provider_ops:balance:{provider_id}"
        await CacheService.delete(cache_key)
        logger.info("余额缓存已清除: provider_id={}", provider_id)

    async def _cache_auth_failed(self, provider_id: str, result: ActionResult) -> None:
        """
        缓存认证失败结果（使用较短 TTL）

        这样前端可以立即显示错误信息，而不是无限显示"加载中..."。
        用户修正配置后，等待 60 秒或手动刷新即可重试。
        """
        cache_key = f"provider_ops:balance:{provider_id}"
        cache_data = {
            "status": result.status.value,
            "data": None,
            "message": result.message,
            "executed_at": result.executed_at.isoformat() if result.executed_at else None,
            "response_time_ms": result.response_time_ms,
        }
        await CacheService.set(cache_key, cache_data, AUTH_FAILED_CACHE_TTL)
        logger.info(
            "余额缓存已写入（认证失败）: provider_id={}, message={}",
            provider_id,
            result.message,
        )

    async def _cache_balance(self, provider_id: str, result: ActionResult) -> None:
        """缓存余额结果"""
        cache_key = f"provider_ops:balance:{provider_id}"

        # 序列化 BalanceInfo
        data = result.data
        if isinstance(data, BalanceInfo):
            data = asdict(data)

        cache_data = {
            "status": result.status.value,
            "data": data,
            "executed_at": result.executed_at.isoformat(),
            "response_time_ms": result.response_time_ms,
        }

        await CacheService.set(cache_key, cache_data, BALANCE_CACHE_TTL)

    async def _cache_balance_from_verify(
        self,
        provider_id: str,
        quota_usd: float,
        extra: dict[str, Any] | None = None,
    ) -> None:
        """
        从验证结果缓存余额

        Args:
            provider_id: Provider ID
            quota_usd: 已转换为美元的余额值
            extra: 额外信息（如窗口限额）
        """
        cache_key = f"provider_ops:balance:{provider_id}"

        # 构建与 BalanceAction 兼容的缓存数据
        # 注意：验证接口只返回 quota，没有 total_granted/total_used
        cache_data = {
            "status": "success",
            "data": {
                "total_granted": None,
                "total_used": None,
                "total_available": quota_usd,
                "currency": "USD",
                "extra": extra or {},
            },
            "executed_at": datetime.now(timezone.utc).isoformat(),
            "response_time_ms": None,
        }

        await CacheService.set(cache_key, cache_data, BALANCE_CACHE_TTL)
        logger.debug("验证成功，缓存余额: provider_id={}, quota_usd={}", provider_id, quota_usd)

    async def _get_cached_balance(self, provider_id: str) -> ActionResult | None:
        """获取缓存的余额"""
        cache_key = f"provider_ops:balance:{provider_id}"
        cached = await CacheService.get(cache_key)

        if not cached:
            return None

        # 反序列化
        try:
            data = cached.get("data")
            if data and isinstance(data, dict):
                # 转回 BalanceInfo
                data = BalanceInfo(
                    total_granted=data.get("total_granted"),
                    total_used=data.get("total_used"),
                    total_available=data.get("total_available"),
                    currency=data.get("currency", "USD"),
                    extra=data.get("extra", {}),
                )

            executed_at_str = cached.get("executed_at")
            executed_at = (
                datetime.fromisoformat(executed_at_str)
                if executed_at_str
                else datetime.now(timezone.utc)
            )

            status = ActionStatus(cached.get("status", "success"))
            # 认证失败使用较短的缓存 TTL
            ttl = AUTH_FAILED_CACHE_TTL if status == ActionStatus.AUTH_FAILED else BALANCE_CACHE_TTL
            return ActionResult(
                status=status,
                action_type=ProviderActionType.QUERY_BALANCE,
                data=data,
                message=cached.get("message"),
                executed_at=executed_at,
                response_time_ms=cached.get("response_time_ms"),
                cache_ttl_seconds=ttl,
            )
        except Exception as e:
            logger.warning("解析缓存余额失败: provider_id={}, error={}", provider_id, e)
            return None

    async def checkin(
        self,
        provider_id: str,
        config: dict[str, Any] | None = None,
    ) -> ActionResult:
        """
        签到（快捷方法）

        Args:
            provider_id: Provider ID
            config: 操作配置

        Returns:
            操作结果
        """
        return await self.execute_action(provider_id, ProviderActionType.CHECKIN, config)

    # ==================== 辅助方法 ====================

    def _get_provider(self, provider_id: str) -> Provider | None:
        """获取 Provider"""
        return self.db.query(Provider).filter(Provider.id == provider_id).first()

    def _get_provider_base_url(self, provider: Provider) -> str | None:
        """从 Provider 获取 base_url"""
        # 优先从第一个 endpoint 获取
        if provider.endpoints:
            for endpoint in provider.endpoints:
                if endpoint.base_url:
                    return endpoint.base_url

        # 从 config 获取
        config = provider.config or {}
        if "base_url" in config:
            return config["base_url"]

        # 从 website 获取
        if provider.website:
            return provider.website

        return None

    def _persist_updated_credentials(self, provider_id: str, updated: dict[str, Any]) -> None:
        """
        持久化连接器运行时更新的凭据（如 Token Rotation 后的新 refresh_token）

        通过 run_in_executor 将同步 DB 操作 offload 到线程池，避免在异步事件循环中阻塞。
        """
        try:
            loop = asyncio.get_running_loop()
            future = loop.run_in_executor(
                None, self._persist_updated_credentials_sync, provider_id, updated
            )

            def _log_persist_error(f: asyncio.Future) -> None:  # type: ignore[type-arg]
                if not f.cancelled() and f.exception():
                    logger.warning("异步持久化凭据失败: {}", f.exception())

            future.add_done_callback(_log_persist_error)
        except RuntimeError:
            # 没有运行中的事件循环（测试等场景），直接同步执行
            self._persist_updated_credentials_sync(provider_id, updated)

    def _persist_updated_credentials_sync(self, provider_id: str, updated: dict[str, Any]) -> None:
        """同步执行凭据持久化（在线程池中运行）"""
        import copy

        db = None
        try:
            db = create_session()
            provider = db.query(Provider).filter(Provider.id == provider_id).first()
            if not provider:
                return

            # 深拷贝整个 config，避免原地修改导致 SQLAlchemy 变更检测失败
            provider_config = copy.deepcopy(dict(provider.config or {}))

            config_data = provider_config.get("provider_ops")
            if not config_data:
                return

            credentials = config_data.get("connector", {}).get("credentials", {})

            # 更新凭据（敏感字段加密）
            for key, value in updated.items():
                if key in self.SENSITIVE_FIELDS and isinstance(value, str) and value:
                    credentials[key] = self.crypto.encrypt(value)
                else:
                    credentials[key] = value

            config_data["connector"]["credentials"] = credentials
            provider.config = provider_config
            flag_modified(provider, "config")
            db.commit()

            logger.info(
                "凭据已持久化更新: provider_id={}, updated_keys={}",
                provider_id,
                list(updated.keys()),
            )
        except Exception as e:
            logger.warning("持久化凭据更新失败: provider_id={}, error={}", provider_id, e)
            if db:
                try:
                    db.rollback()
                except Exception:
                    pass
        finally:
            if db:
                try:
                    db.close()
                except Exception:
                    pass

    def _encrypt_credentials(self, credentials: dict[str, Any]) -> dict[str, Any]:
        """加密凭据中的敏感字段"""
        encrypted = {}
        for key, value in credentials.items():
            if key in self.SENSITIVE_FIELDS and isinstance(value, str):
                if value:  # 只加密非空值
                    encrypted[key] = self.crypto.encrypt(value)
                    logger.debug(
                        "加密字段 {}: 原始长度={}, 加密后长度={}",
                        key,
                        len(value),
                        len(encrypted[key]),
                    )
                else:
                    logger.warning("跳过空值字段 {}", key)
                    encrypted[key] = value
            else:
                encrypted[key] = value
        return encrypted

    def _decrypt_credentials(self, credentials: dict[str, Any]) -> dict[str, Any]:
        """解密凭据中的敏感字段"""
        decrypted = {}
        for key, value in credentials.items():
            if key in self.SENSITIVE_FIELDS and isinstance(value, str):
                try:
                    decrypted[key] = self.crypto.decrypt(value)
                except Exception as e:
                    logger.warning("解密字段 {} 失败: {}", key, e)
                    decrypted[key] = value  # 解密失败则保持原值
            else:
                decrypted[key] = value
        return decrypted

    # 密码类字段：脱敏时全部遮盖，不显示任何明文字符
    FULLY_MASKED_FIELDS = {"password"}

    def get_masked_credentials(self, credentials: dict[str, Any]) -> dict[str, Any]:
        """
        获取脱敏后的凭据

        解密凭据并对敏感字段进行脱敏处理。
        - 密码类字段：全部遮盖为 ********
        - 其他敏感字段：显示部分字符（如 sk-x****a12k）

        Args:
            credentials: 加密的凭据

        Returns:
            脱敏后的凭据
        """
        decrypted = self._decrypt_credentials(credentials)

        for field in self.SENSITIVE_FIELDS:
            if field in decrypted and decrypted[field]:
                value = str(decrypted[field])
                if field in self.FULLY_MASKED_FIELDS:
                    decrypted[field] = "********"
                elif len(value) > 12:
                    # 显示前4位和后4位，中间固定4个 *（如 sk-x****a12k）
                    decrypted[field] = value[:4] + "****" + value[-4:]
                elif len(value) > 8:
                    decrypted[field] = value[:2] + "****" + value[-2:]
                else:
                    decrypted[field] = "*" * len(value)

        return decrypted

    def merge_credentials_with_saved(
        self,
        provider_id: str,
        credentials: dict[str, Any],
    ) -> dict[str, Any]:
        """
        合并凭据：如果请求中的敏感字段为空，使用已保存的凭据

        用于验证和保存配置时，当用户未重新输入敏感字段时保留原有值。

        Args:
            provider_id: Provider ID
            credentials: 请求中的凭据

        Returns:
            合并后的凭据
        """
        merged = dict(credentials)
        saved_config = self.get_config(provider_id)

        if saved_config:
            saved_credentials = self._decrypt_credentials(saved_config.connector_credentials)
            sensitive_fields = [
                "api_key",
                "password",
                "refresh_token",
                "session_token",
                "cookie_string",
                "cookie",
                "token_cookie",
                "auth_cookie",
                "session_cookie",  # Cookie 认证字段
            ]

            for field in sensitive_fields:
                # 如果请求中该字段为空或只包含星号（脱敏值），使用已保存的值
                req_value = merged.get(field, "")
                if not req_value or (isinstance(req_value, str) and set(req_value) <= {"*"}):
                    if field in saved_credentials:
                        merged[field] = saved_credentials[field]
                        logger.debug("合并凭据 - 使用已保存的 {}", field)

            # 保留内部缓存字段（如 _cached_access_token），前端不感知这些字段
            for key, value in saved_credentials.items():
                if key.startswith("_") and key not in merged:
                    merged[key] = value

        return merged

    # ==================== 批量操作 ====================

    async def batch_query_balance(
        self, provider_ids: list[str] | None = None
    ) -> dict[str, ActionResult]:
        """
        批量查询余额（优先返回缓存，后台异步刷新）

        使用信号量限制并发数，避免数据库连接池耗尽。

        Args:
            provider_ids: Provider ID 列表，None 表示查询所有已配置的

        Returns:
            {provider_id: result}
        """
        if provider_ids is None:
            # 查询所有已配置的 Provider
            providers = self.db.query(Provider).filter(Provider.is_active.is_(True)).all()
            provider_ids = [p.id for p in providers if p.config and p.config.get("provider_ops")]

        if not provider_ids:
            return {}

        # Release the DB connection before awaiting many async cache refreshes.
        self._release_db_connection_before_await()

        # 使用信号量限制并发数，避免同时发起过多请求耗尽连接池
        concurrency = _get_batch_balance_concurrency()
        semaphore = asyncio.Semaphore(concurrency)

        async def _query_with_limit(provider_id: str) -> tuple[str, ActionResult]:
            async with semaphore:
                try:
                    # 批量查询时禁用同步查询，避免阻塞请求
                    # 缓存未命中时会触发异步刷新，前端可稍后重试
                    result = await self.query_balance_with_cache(
                        provider_id, trigger_refresh=True, allow_sync_query=False
                    )
                    return provider_id, result
                except Exception as e:
                    logger.warning("查询余额失败: provider_id={}, error={}", provider_id, e)
                    return provider_id, ActionResult(
                        status=ActionStatus.UNKNOWN_ERROR,
                        action_type=ProviderActionType.QUERY_BALANCE,
                        message=str(e),
                    )

        # 并行查询，但受信号量限制
        tasks = [_query_with_limit(provider_id) for provider_id in provider_ids]
        results_list = await asyncio.gather(*tasks)

        return dict(results_list)

    # ==================== 认证验证 ====================

    async def verify_auth(
        self,
        base_url: str,
        architecture_id: str,
        auth_type: ConnectorAuthType,
        config: dict[str, Any],
        credentials: dict[str, Any],
        provider_id: str | None = None,
    ) -> dict[str, Any]:
        """
        验证认证配置

        在保存前测试认证是否有效。
        认证逻辑委托给对应的 Architecture 实现。

        Args:
            base_url: API 基础地址
            architecture_id: 架构 ID
            auth_type: 认证类型
            config: 连接器配置
            credentials: 凭据
            provider_id: Provider ID（可选，用于缓存余额）

        Returns:
            验证结果
        """
        import httpx

        from src.utils.ssl_utils import get_ssl_context

        # Avoid holding a DB connection while awaiting verify pre-processing / network.
        self._release_db_connection_before_await()

        # 移除 base_url 末尾的斜杠
        base_url = base_url.rstrip("/")

        # 获取架构实例
        registry = get_registry()
        architecture = registry.get_or_default(architecture_id)

        # 使用架构的方法构建请求
        verify_endpoint = f"{base_url}{architecture.get_verify_endpoint()}"

        # 执行异步预处理（如获取动态 Cookie、登录获取 Token）
        # 返回值可以是 dict（仅额外配置）或 tuple[dict, dict]（额外配置 + 凭据更新）
        prepare_result = await architecture.prepare_verify_config(base_url, config, credentials)
        if isinstance(prepare_result, tuple):
            extra_config, updated_creds = prepare_result
        else:
            extra_config = prepare_result
            updated_creds = {}
        merged_config = {**config, **extra_config}

        # Token Rotation: prepare_verify_config 可能已消耗旧 refresh_token 并获取新值，
        # 无论后续验证是否成功都需要立即持久化，否则旧 token 已失效但数据库未更新。
        if updated_creds and provider_id:
            logger.info(
                "验证过程检测到凭据变更: provider_id={}, updated_keys={}",
                provider_id,
                list(updated_creds.keys()),
            )
            self._persist_updated_credentials(provider_id, updated_creds)

        headers = architecture.build_verify_headers(merged_config, credentials)

        logger.debug(
            "验证认证: architecture={}, endpoint={}, headers={}",
            architecture_id,
            verify_endpoint,
            list(headers.keys()),
        )

        # 获取代理配置（支持 proxy_node_id、tunnel 模式和旧的 proxy URL）
        from src.services.proxy_node.resolver import resolve_ops_proxy_config

        proxy, tunnel_node_id = resolve_ops_proxy_config(config)

        try:
            # 构建 httpx client 参数
            client_kwargs: dict[str, Any] = {
                "timeout": 30.0,
                "verify": get_ssl_context(),
            }
            if tunnel_node_id:
                from src.services.proxy_node.tunnel_transport import create_tunnel_transport

                client_kwargs["transport"] = create_tunnel_transport(tunnel_node_id, timeout=30.0)
                logger.debug("使用 tunnel 代理: node_id={}", tunnel_node_id)
            elif proxy:
                client_kwargs["proxy"] = proxy
                logger.debug("使用代理: {}", proxy)

            async with httpx.AsyncClient(**client_kwargs) as client:
                response = await client.get(verify_endpoint, headers=headers)

                logger.debug(
                    "验证响应: status={}, content_type={}",
                    response.status_code,
                    response.headers.get("content-type"),
                )

                # 尝试解析 JSON
                try:
                    data = response.json()
                except Exception:
                    data = {}

                # 将预处理获取的额外数据合并到响应中
                if "_combined_data" in merged_config:
                    data["_combined_data"] = merged_config["_combined_data"]
                elif "_balance_data" in merged_config:
                    data["_balance_data"] = merged_config["_balance_data"]

                # 使用架构的方法解析响应
                result = architecture.parse_verify_response(response.status_code, data)
                result_dict = result.to_dict()

                # 将凭据更新信息附加到响应，供前端同步更新表单
                # 过滤掉内部缓存字段（以 _ 开头），前端不需要这些
                frontend_creds = {k: v for k, v in updated_creds.items() if not k.startswith("_")}
                if frontend_creds:
                    result_dict["updated_credentials"] = frontend_creds

                # 验证成功且有 provider_id 时，缓存余额
                if result.success and provider_id and result.quota is not None:
                    # 从架构的默认配置获取 quota_divisor
                    balance_config = architecture.default_action_configs.get(
                        ProviderActionType.QUERY_BALANCE, {}
                    )
                    quota_divisor = balance_config.get("quota_divisor", 1)
                    # 转换为美元值后缓存
                    quota_usd = result.quota / quota_divisor
                    # 传入 extra 信息（如窗口限额）
                    await self._cache_balance_from_verify(provider_id, quota_usd, result.extra)

                return result_dict

        except httpx.TimeoutException:
            return {"success": False, "message": "连接超时"}
        except httpx.ConnectError as e:
            return {"success": False, "message": f"连接失败: {str(e)}"}
        except Exception as e:
            logger.error("验证认证失败: {}", e)
            return {"success": False, "message": f"验证失败: {str(e)}"}
