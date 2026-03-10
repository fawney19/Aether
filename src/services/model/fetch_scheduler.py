"""
模型自动获取调度器

定时从上游 API 获取可用模型列表，并更新 ProviderAPIKey 的 allowed_models。

功能:
- 扫描所有启用了 auto_fetch_models 的 ProviderAPIKey
- 调用 core.api_format 注册表获取模型列表
- 更新 Key 的 allowed_models（保留 locked_models 中的模型）
- 支持包含/排除规则过滤模型
- 记录获取结果和错误信息
"""

import asyncio
import fnmatch
import json
import os
from collections.abc import Awaitable, Callable
from dataclasses import dataclass
from datetime import datetime, timezone
from typing import Any

from sqlalchemy.orm import defer, joinedload, load_only

from src.core.cache_service import CacheService
from src.core.crypto import crypto_service
from src.core.logger import logger
from src.core.provider_types import ProviderType
from src.database import create_session
from src.models.database import Provider, ProviderAPIKey
from src.services.model.upstream_fetcher import (
    EndpointFetchConfig,
    UpstreamModelsFetchContext,
    build_format_to_config,
    fetch_models_for_key,
    merge_upstream_metadata,
)
from src.services.provider.oauth_token import resolve_oauth_access_token
from src.services.proxy_node.resolver import resolve_effective_proxy
from src.services.system.scheduler import get_scheduler

# 从环境变量读取间隔，默认 1440 分钟（1 天），限制在 60-10080 分钟之间
_interval_env = int(os.getenv("MODEL_FETCH_INTERVAL_MINUTES", "1440"))
MODEL_FETCH_INTERVAL_MINUTES = max(60, min(10080, _interval_env))

# 并发请求限制
MAX_CONCURRENT_REQUESTS = 5

# 单个 Key 处理的超时时间（秒）
KEY_FETCH_TIMEOUT_SECONDS = 120

# 模型获取 HTTP 请求超时时间（秒）
# 使用较短的超时（10秒），避免不支持 /models 端点的提供商长时间阻塞
MODEL_FETCH_HTTP_TIMEOUT = 10.0

# 启动时首次自动获取开关与延迟
MODEL_FETCH_STARTUP_ENABLED = (
    os.getenv("MODEL_FETCH_STARTUP_ENABLED", "true").lower() == "true"
)
MODEL_FETCH_STARTUP_DELAY_SECONDS = max(
    0,
    int(os.getenv("MODEL_FETCH_STARTUP_DELAY_SECONDS", "10")),
)

# 单批扫描的 Key 数量（仅拉取 ID，避免一次性扫描整个大号池）
AUTO_FETCH_KEY_BATCH_SIZE = max(MAX_CONCURRENT_REQUESTS, 100)

# 上游模型缓存 TTL（与定时任务间隔保持一致）
UPSTREAM_MODELS_CACHE_TTL_SECONDS = MODEL_FETCH_INTERVAL_MINUTES * 60


@dataclass(frozen=True, slots=True)
class PreparedModelsFetchContext:
    key_id: str
    provider_id: str
    provider_name: str
    provider_type: str
    auth_type: str
    encrypted_api_key: str
    encrypted_auth_config: str | None
    format_to_endpoint: dict[str, EndpointFetchConfig]
    proxy_config: dict[str, Any] | None


def _match_pattern(model_id: str, pattern: str) -> bool:
    """
    检查模型 ID 是否匹配模式

    支持的通配符:
    - * 匹配任意字符（包括空）
    - ? 匹配单个字符

    Args:
        model_id: 模型 ID
        pattern: 匹配模式

    Returns:
        是否匹配
    """
    return fnmatch.fnmatch(model_id.lower(), pattern.lower())


def _filter_models_by_patterns(
    model_ids: set[str],
    include_patterns: list[str] | None,
    exclude_patterns: list[str] | None,
) -> set[str]:
    """
    根据包含/排除规则过滤模型列表

    规则优先级:
    1. 如果 include_patterns 为空或 None，则包含所有模型
    2. 如果 include_patterns 不为空，则只包含匹配的模型
    3. exclude_patterns 总是会排除匹配的模型（优先级高于 include）

    Args:
        model_ids: 原始模型 ID 集合
        include_patterns: 包含规则列表（支持 * 和 ? 通配符）
        exclude_patterns: 排除规则列表（支持 * 和 ? 通配符）

    Returns:
        过滤后的模型 ID 集合
    """
    result = set()

    for model_id in model_ids:
        # 步骤1: 检查是否应该包含
        should_include = True
        if include_patterns:
            # 有包含规则时，必须匹配至少一个规则
            should_include = any(_match_pattern(model_id, p) for p in include_patterns)

        if not should_include:
            continue

        # 步骤2: 检查是否应该排除
        should_exclude = False
        if exclude_patterns:
            should_exclude = any(_match_pattern(model_id, p) for p in exclude_patterns)

        if not should_exclude:
            result.add(model_id)

    return result


def _get_upstream_models_cache_key(provider_id: str, api_key_id: str) -> str:
    """生成上游模型缓存的 key"""
    return f"upstream_models:{provider_id}:{api_key_id}"


async def get_upstream_models_from_cache(provider_id: str, api_key_id: str) -> list[dict] | None:
    """从缓存获取上游模型列表"""
    cache_key = _get_upstream_models_cache_key(provider_id, api_key_id)
    cached = await CacheService.get(cache_key)
    if cached is not None:
        logger.debug(f"上游模型缓存命中: {cache_key}")
        return cached  # type: ignore[no-any-return]
    return None


async def set_upstream_models_to_cache(
    provider_id: str, api_key_id: str, models: list[dict]
) -> None:
    """将上游模型列表写入缓存"""
    cache_key = _get_upstream_models_cache_key(provider_id, api_key_id)
    await CacheService.set(cache_key, models, UPSTREAM_MODELS_CACHE_TTL_SECONDS)
    logger.debug(f"上游模型已缓存: {cache_key}, 数量={len(models)}")


def _aggregate_models_for_cache(models: list[dict]) -> list[dict]:
    """聚合缓存模型，按 model id 合并 api_formats，减少 Redis 占用。"""
    aggregated: dict[str, dict[str, Any]] = {}
    ordered_ids: list[str] = []

    for model in models:
        if not isinstance(model, dict):
            continue

        model_id = str(model.get("id") or "").strip()
        if not model_id:
            continue

        api_format = str(model.get("api_format") or "").strip()
        existing = aggregated.get(model_id)

        if existing is None:
            payload = dict(model)
            payload.pop("api_format", None)
            payload["api_formats"] = [api_format] if api_format else []
            aggregated[model_id] = payload
            ordered_ids.append(model_id)
            continue

        api_formats = existing.setdefault("api_formats", [])
        if not isinstance(api_formats, list):
            api_formats = []
            existing["api_formats"] = api_formats

        if api_format and api_format not in api_formats:
            api_formats.append(api_format)

        for key, value in model.items():
            if key not in existing and key != "api_format":
                existing[key] = value

    for model_id in ordered_ids:
        api_formats = aggregated[model_id].get("api_formats")
        if isinstance(api_formats, list):
            aggregated[model_id]["api_formats"] = sorted(
                {str(fmt) for fmt in api_formats if str(fmt).strip()}
            )

    return [aggregated[model_id] for model_id in ordered_ids]


async def _run_key_fetch_workers(
    key_ids: list[str],
    *,
    max_concurrent: int,
    timeout_seconds: float,
    running_predicate: Callable[[], bool],
    fetch_one: Callable[[str], Awaitable[str]],
    on_timeout: Callable[[str], None],
    on_error: Callable[[str, str], None],
) -> tuple[int, int, int]:
    """用固定 worker 数处理 key，避免一次性创建大量协程导致内存峰值过高。"""
    if not key_ids:
        return 0, 0, 0

    worker_count = max(1, min(max_concurrent, len(key_ids)))
    key_queue: asyncio.Queue[str] = asyncio.Queue()
    for key_id in key_ids:
        key_queue.put_nowait(key_id)

    async def _worker() -> tuple[int, int, int]:
        success_count = 0
        error_count = 0
        skip_count = 0

        while True:
            if not running_predicate():
                break

            try:
                key_id = key_queue.get_nowait()
            except asyncio.QueueEmpty:
                break

            result = "error"
            try:
                if not running_predicate():
                    result = "skip"
                else:
                    result = await asyncio.wait_for(fetch_one(key_id), timeout=timeout_seconds)
            except TimeoutError:
                logger.error(f"处理 Key {key_id} 超时（{timeout_seconds}s）")
                on_timeout(key_id)
                result = "error"
            except Exception as exc:
                logger.exception(f"处理 Key {key_id} 时出错")
                on_error(key_id, str(exc))
                result = "error"
            finally:
                key_queue.task_done()

            if result == "success":
                success_count += 1
            elif result == "skip":
                skip_count += 1
            else:
                error_count += 1

        return success_count, error_count, skip_count

    results = await asyncio.gather(*[asyncio.create_task(_worker()) for _ in range(worker_count)])

    success_count = sum(success for success, _, _ in results)
    error_count = sum(error for _, error, _ in results)
    skip_count = sum(skip for _, _, skip in results)

    # 停止过程中尚未消费的队列项统一计为 skip。
    skip_count += key_queue.qsize()
    return success_count, error_count, skip_count


class ModelFetchScheduler:
    """模型自动获取调度器"""

    def __init__(self) -> None:
        self._running = False
        self._lock = asyncio.Lock()
        self._startup_task: asyncio.Task | None = None

    async def start(self) -> None:
        """启动调度器"""
        if self._running:
            logger.warning("ModelFetchScheduler already running")
            return

        self._running = True
        logger.info(f"模型自动获取调度器已启动，间隔: {MODEL_FETCH_INTERVAL_MINUTES} 分钟")

        scheduler = get_scheduler()
        scheduler.add_interval_job(
            self._scheduled_fetch_models,
            minutes=MODEL_FETCH_INTERVAL_MINUTES,
            job_id="model_auto_fetch",
            name="自动获取模型",
        )

        # 启动时延迟执行一次，保存任务引用
        self._startup_task = asyncio.create_task(self._run_startup_task())

    async def stop(self) -> None:
        """停止调度器"""
        self._running = False

        # 取消并等待启动任务完成
        if self._startup_task and not self._startup_task.done():
            self._startup_task.cancel()
            try:
                await self._startup_task
            except asyncio.CancelledError:
                pass

        logger.info("模型自动获取调度器已停止")

    async def _run_startup_task(self) -> None:
        """启动时执行的初始化任务"""
        try:
            if not MODEL_FETCH_STARTUP_ENABLED:
                logger.info("启动时模型自动获取已禁用（MODEL_FETCH_STARTUP_ENABLED=false）")
                return

            if MODEL_FETCH_STARTUP_DELAY_SECONDS > 0:
                await asyncio.sleep(MODEL_FETCH_STARTUP_DELAY_SECONDS)

            if not self._running:
                return
            logger.info("启动时执行首次模型获取...")
            await self._perform_fetch_all_keys()
        except asyncio.CancelledError:
            logger.debug("启动任务被取消")
            raise
        except Exception:
            logger.exception("启动时模型获取出错")

    async def _scheduled_fetch_models(self) -> None:
        """定时任务入口"""
        async with self._lock:
            await self._perform_fetch_all_keys()

    def _list_auto_fetch_key_id_batch(
        self,
        *,
        after_id: str | None = None,
        limit: int = AUTO_FETCH_KEY_BATCH_SIZE,
    ) -> list[str]:
        """分批返回启用自动获取模型的 Key ID。"""
        with create_session() as db:
            query = db.query(ProviderAPIKey.id).filter(
                ProviderAPIKey.auto_fetch_models == True,  # noqa: E712
                ProviderAPIKey.is_active == True,  # noqa: E712
            )
            if after_id:
                query = query.filter(ProviderAPIKey.id > after_id)
            return [row[0] for row in query.order_by(ProviderAPIKey.id.asc()).limit(limit).all()]

    async def _perform_fetch_all_keys(self) -> None:
        """获取所有启用自动获取的 Key，并以固定批次/并发节奏拉取。"""
        logger.info("开始自动获取模型任务...")

        success_count = 0
        error_count = 0
        skip_count = 0
        total_count = 0
        last_id: str | None = None
        batch_index = 0

        while self._running:
            key_ids = self._list_auto_fetch_key_id_batch(after_id=last_id)
            if not key_ids:
                break

            batch_index += 1
            total_count += len(key_ids)
            last_id = key_ids[-1]
            logger.info(
                "自动获取模型任务处理第 {} 批 Key: {} 个",
                batch_index,
                len(key_ids),
            )

            batch_success, batch_error, batch_skip = await _run_key_fetch_workers(
                key_ids,
                max_concurrent=MAX_CONCURRENT_REQUESTS,
                timeout_seconds=KEY_FETCH_TIMEOUT_SECONDS,
                running_predicate=lambda: self._running,
                fetch_one=self._fetch_models_for_key_by_id,
                on_timeout=lambda key_id: self._update_key_error(
                    key_id, f"Timeout after {KEY_FETCH_TIMEOUT_SECONDS}s"
                ),
                on_error=self._update_key_error,
            )
            success_count += batch_success
            error_count += batch_error
            skip_count += batch_skip

            if len(key_ids) < AUTO_FETCH_KEY_BATCH_SIZE:
                break

            await asyncio.sleep(0)

        if total_count == 0:
            logger.debug("没有启用自动获取模型的 Key")
            return

        logger.info(
            "自动获取模型任务完成: 总数={}, 成功={}, 失败={}, 跳过={}",
            total_count,
            success_count,
            error_count,
            skip_count,
        )

    def _update_key_error(self, key_id: str, error_msg: str) -> None:
        """更新 Key 的错误信息（独立事务）"""
        try:
            with create_session() as db:
                key = (
                    db.query(ProviderAPIKey)
                    .options(
                        defer(ProviderAPIKey.adjustment_history),
                        defer(ProviderAPIKey.utilization_samples),
                        defer(ProviderAPIKey.health_by_format),
                        defer(ProviderAPIKey.circuit_breaker_by_format),
                        defer(ProviderAPIKey.upstream_metadata),
                        defer(ProviderAPIKey.allowed_models),
                    )
                    .filter(ProviderAPIKey.id == key_id)
                    .first()
                )
                if key:
                    key.last_models_fetch_at = datetime.now(timezone.utc)
                    key.last_models_fetch_error = error_msg
                    db.commit()
        except Exception:
            logger.exception(f"更新 Key {key_id} 错误信息失败")

    async def _fetch_models_for_key_by_id(self, key_id: str) -> str:
        """
        根据 Key ID 获取模型并更新，返回结果状态

        优化：分两个阶段处理，HTTP 请求期间不持有数据库连接，避免阻塞其他请求
        """
        # ========== 阶段 1：准备数据（短暂持有连接）==========
        prepared = self._prepare_fetch_context(key_id)
        if prepared is None:
            return "skip"
        if isinstance(prepared, str):
            return prepared  # "error" or "skip"

        # Resolve auth (incl. lazy OAuth refresh) without holding a DB session.
        api_key_value: str = ""
        auth_config: dict[str, Any] | None = None

        if prepared.auth_type == "oauth":
            # Use request_builder's lazy refresh logic and persist refreshed token back to DB.
            # Endpoint signature is only used for tracing/debug; auth logic doesn't depend on it.
            endpoint_api_format = (
                "gemini:chat"
                if prepared.provider_type.lower() == ProviderType.ANTIGRAVITY
                else None
            )
            try:
                resolved = await resolve_oauth_access_token(
                    key_id=prepared.key_id,
                    encrypted_api_key=prepared.encrypted_api_key,
                    encrypted_auth_config=prepared.encrypted_auth_config,
                    provider_proxy_config=prepared.proxy_config,
                    endpoint_api_format=endpoint_api_format,
                )
                api_key_value = resolved.access_token
                auth_config = resolved.decrypted_auth_config
            except Exception as e:
                self._update_key_error(prepared.key_id, f"OAuth token resolution failed: {e}")
                return "error"
        else:
            try:
                api_key_value = crypto_service.decrypt(prepared.encrypted_api_key)
            except Exception:
                self._update_key_error(prepared.key_id, "Decrypt error")
                return "error"

            # Best-effort: decrypt auth_config if present (e.g. Antigravity project_id).
            if prepared.encrypted_auth_config:
                try:
                    parsed = json.loads(crypto_service.decrypt(prepared.encrypted_auth_config))
                    auth_config = parsed if isinstance(parsed, dict) else None
                except Exception:
                    auth_config = None

        fetch_ctx = UpstreamModelsFetchContext(
            provider_type=prepared.provider_type,
            api_key_value=api_key_value,
            format_to_endpoint=prepared.format_to_endpoint,
            proxy_config=prepared.proxy_config,
            auth_config=auth_config,
        )

        # ========== 阶段 2：HTTP 请求（不持有数据库连接）==========
        # 使用较短的超时时间（10秒），避免长时间阻塞
        all_models, errors, has_success, upstream_metadata = await fetch_models_for_key(
            fetch_ctx,
            timeout_seconds=MODEL_FETCH_HTTP_TIMEOUT,
        )

        # ========== 阶段 3：更新数据库（获取新连接）==========
        return await self._update_key_after_fetch(
            key_id=prepared.key_id,
            provider_id=prepared.provider_id,
            provider_name=prepared.provider_name,
            all_models=all_models,
            errors=errors,
            has_success=has_success,
            upstream_metadata=upstream_metadata,
        )

    def _prepare_fetch_context(self, key_id: str) -> PreparedModelsFetchContext | str | None:
        """
        准备获取模型所需的上下文数据

        Returns:
            - PreparedModelsFetchContext: 准备好的上下文（不包含解密后的 token）
            - "skip": 跳过该 Key
            - "error": 出错
            - None: Key 不存在
        """
        with create_session() as db:
            key = (
                db.query(ProviderAPIKey)
                .options(
                    load_only(
                        ProviderAPIKey.id,
                        ProviderAPIKey.is_active,
                        ProviderAPIKey.auto_fetch_models,
                        ProviderAPIKey.api_key,
                        ProviderAPIKey.auth_type,
                        ProviderAPIKey.auth_config,
                        ProviderAPIKey.provider_id,
                        ProviderAPIKey.proxy,
                    ),
                )
                .filter(ProviderAPIKey.id == key_id)
                .first()
            )

            if not key:
                logger.warning(f"Key {key_id} 不存在，跳过")
                return None

            if not key.is_active or not key.auto_fetch_models:
                logger.debug(f"Key {key_id} 已禁用或关闭自动获取，跳过")
                return "skip"

            now = datetime.now(timezone.utc)
            provider_id = key.provider_id

            # 获取 Provider 和 Endpoints
            provider = (
                db.query(Provider)
                .options(joinedload(Provider.endpoints))
                .filter(Provider.id == provider_id)
                .first()
            )

            if not provider:
                logger.warning(f"Provider {provider_id} 不存在，跳过 Key {key.id}")
                key.last_models_fetch_error = "Provider not found"
                key.last_models_fetch_at = now
                db.commit()
                return "error"

            # Service Account 类型不支持自动获取模型（Vertex AI SA / 旧 vertex_ai auth_type）
            auth_type = getattr(key, "auth_type", "api_key") or "api_key"
            if auth_type in ("service_account", "vertex_ai"):
                key.last_models_fetch_error = (
                    "auto_fetch_models 暂不支持 Service Account 类型的 Key"
                )
                key.last_models_fetch_at = now
                db.commit()
                logger.info(f"Key {key.id} 为 Service Account 类型，跳过自动获取模型")
                return "skip"

            # 基础校验：必须有 api_key（OAuth: 加密 access_token；API Key: 加密 key）
            if not key.api_key:
                logger.warning(f"Key {key.id} 没有 API Key，跳过")
                key.last_models_fetch_error = "No API key configured"
                key.last_models_fetch_at = now
                db.commit()
                return "error"

            # 构建 api_format -> EndpointFetchConfig 映射（纯数据，session 无关）
            format_to_endpoint = build_format_to_config(provider.endpoints)  # type: ignore[attr-defined]

            if not format_to_endpoint:
                logger.warning(f"Provider {provider.name} 没有活跃的端点，跳过 Key {key.id}")
                key.last_models_fetch_error = "No active endpoints"
                key.last_models_fetch_at = now
                db.commit()
                return "error"

            encrypted_auth_config = getattr(key, "auth_config", None)
            provider_type = str(getattr(provider, "provider_type", "") or "")

            return PreparedModelsFetchContext(
                key_id=key_id,
                provider_id=provider_id,
                provider_name=provider.name,
                provider_type=provider_type,
                auth_type=auth_type,
                encrypted_api_key=str(key.api_key),
                encrypted_auth_config=(
                    encrypted_auth_config if isinstance(encrypted_auth_config, str) else None
                ),
                format_to_endpoint=format_to_endpoint,
                proxy_config=resolve_effective_proxy(
                    getattr(provider, "proxy", None), getattr(key, "proxy", None)
                ),
            )

    async def _update_key_after_fetch(
        self,
        key_id: str,
        provider_id: str,
        provider_name: str,
        all_models: list[dict],
        errors: list[str],
        has_success: bool,
        upstream_metadata: dict[str, Any] | None = None,
    ) -> str:
        """
        HTTP 请求完成后更新数据库

        使用新的数据库连接来更新 Key 的 allowed_models
        """
        now = datetime.now(timezone.utc)

        with create_session() as db:
            # 重新获取 Key（因为之前的连接已关闭）
            # defer 不需要的大 JSON 字段，减少内存占用
            key = (
                db.query(ProviderAPIKey)
                .options(
                    defer(ProviderAPIKey.adjustment_history),
                    defer(ProviderAPIKey.utilization_samples),
                    defer(ProviderAPIKey.health_by_format),
                    defer(ProviderAPIKey.circuit_breaker_by_format),
                )
                .filter(ProviderAPIKey.id == key_id)
                .first()
            )
            if not key:
                logger.warning(f"Key {key_id} 在更新时不存在")
                return "error"

            # 记录获取时间
            key.last_models_fetch_at = now

            # 如果没有任何成功的响应，不更新 allowed_models（保留旧数据）
            if not has_success:
                error_msg = "; ".join(errors) if errors else "All endpoints failed"
                key.last_models_fetch_error = error_msg
                logger.warning(
                    f"Provider {provider_name} Key {key.id} 所有端点获取失败，保留现有模型列表"
                )
                db.commit()
                return "error"

            # 有成功的响应，清除错误状态
            key.last_models_fetch_error = None

            # 最佳努力：保存上游元数据（如 Antigravity 配额信息）
            if upstream_metadata and isinstance(upstream_metadata, dict):
                key.upstream_metadata = merge_upstream_metadata(
                    key.upstream_metadata, upstream_metadata
                )

            # 去重获取模型 ID 列表
            fetched_model_ids: set[str] = set()
            for model in all_models:
                model_id = model.get("id")
                if model_id:
                    fetched_model_ids.add(model_id)

            logger.info(
                f"Provider {provider_name} Key {key.id} 获取到 {len(fetched_model_ids)} 个唯一模型"
            )

            # 写入上游模型缓存（按 model id 聚合 api_formats，减少 Redis 内存占用）
            unique_models = _aggregate_models_for_cache(all_models)
            await set_upstream_models_to_cache(provider_id, key.id, unique_models)

            # 更新 allowed_models（保留 locked_models）
            has_changed = self._update_key_allowed_models(key, fetched_model_ids)

            db.commit()

            # 如果白名单有变化，触发缓存失效和自动关联检查
            if has_changed and provider_id:
                from src.services.model.global_model import on_key_allowed_models_changed

                # 使用新会话处理后续操作
                with create_session() as db2:
                    await on_key_allowed_models_changed(
                        db=db2,
                        provider_id=provider_id,
                        allowed_models=list(key.allowed_models or []),
                    )

            return "success"

    def _update_key_allowed_models(self, key: ProviderAPIKey, fetched_model_ids: set[str]) -> bool:
        """
        更新 Key 的 allowed_models，保留 locked_models，应用过滤规则

        Returns:
            bool: 是否有变化
        """
        # 获取当前锁定的模型
        locked_models = set(key.locked_models or [])

        # 应用包含/排除过滤规则
        include_patterns = key.model_include_patterns
        exclude_patterns = key.model_exclude_patterns

        filtered_model_ids = _filter_models_by_patterns(
            fetched_model_ids, include_patterns, exclude_patterns
        )

        # 记录过滤结果
        if include_patterns or exclude_patterns:
            filtered_count = len(fetched_model_ids) - len(filtered_model_ids)
            if filtered_count > 0:
                logger.info(
                    f"Key {key.id} 过滤规则生效: 原始 {len(fetched_model_ids)} 个模型, "
                    f"过滤后 {len(filtered_model_ids)} 个 (排除 {filtered_count} 个)"
                )

        # 新的 allowed_models = 过滤后的模型 + 锁定的模型
        # 锁定模型无论上游是否返回都会保留
        new_allowed_models = list(filtered_model_ids | locked_models)
        new_allowed_models.sort()  # 保持顺序稳定

        # 检查是否有变化
        current_allowed = set(key.allowed_models or [])
        new_allowed_set = set(new_allowed_models)

        if current_allowed != new_allowed_set:
            added = new_allowed_set - current_allowed
            removed = current_allowed - new_allowed_set
            if added:
                logger.info(f"Key {key.id} 新增模型: {sorted(added)}")
            if removed:
                logger.info(f"Key {key.id} 移除模型: {sorted(removed)}")

            key.allowed_models = new_allowed_models
            return True
        else:
            logger.debug(f"Key {key.id} 模型列表无变化")
            return False


# 单例模式
_model_fetch_scheduler: ModelFetchScheduler | None = None


def get_model_fetch_scheduler() -> ModelFetchScheduler:
    """获取模型获取调度器单例"""
    global _model_fetch_scheduler
    if _model_fetch_scheduler is None:
        _model_fetch_scheduler = ModelFetchScheduler()
    return _model_fetch_scheduler
