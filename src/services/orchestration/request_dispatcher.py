"""
请求分发器

负责执行单个候选请求
"""

from collections.abc import Callable
from typing import Any

from sqlalchemy.orm import Session

from src.core.logger import logger
from src.models.database import ApiKey
from src.services.request.candidate import RequestCandidateService
from src.services.request.executor import RequestExecutor
from src.services.scheduling.aware_scheduler import CacheAwareScheduler, ProviderCandidate


class RequestDispatcher:
    """
    请求分发器 - 负责执行单个候选请求

    职责：
    1. 执行请求并返回结果
    2. 更新候选状态（pending -> success/failed）
    3. 设置缓存亲和性（成功时）
    """

    def __init__(
        self,
        db: Session,
        request_executor: RequestExecutor,
        cache_scheduler: CacheAwareScheduler | None = None,
    ) -> None:
        """
        初始化请求分发器

        Args:
            db: 数据库会话
            request_executor: 请求执行器
            cache_scheduler: 缓存调度器（可选）
        """
        self.db = db
        self.request_executor = request_executor
        self.cache_scheduler = cache_scheduler

    async def dispatch(
        self,
        candidate: ProviderCandidate,
        candidate_index: int,
        retry_index: int,
        candidate_record_id: str,
        user_api_key: ApiKey | None,
        user_id: str | None,
        request_func: Callable[..., Any],
        request_id: str | None,
        api_format: str,
        model_name: str,
        affinity_key: str,
        global_model_id: str,
        attempt_counter: int,
        max_attempts: int,
        is_stream: bool = False,
    ) -> tuple[Any, str, str, str, str, str, int | None]:
        """
        执行请求并返回结果

        Args:
            candidate: 候选对象
            candidate_index: 候选索引
            retry_index: 重试索引
            candidate_record_id: 候选记录 ID
            user_api_key: 用户 API Key
            request_func: 请求函数
            request_id: 请求 ID
            api_format: API 格式
            model_name: 模型名称
            affinity_key: 亲和性标识符（通常为API Key ID）
            global_model_id: GlobalModel ID（规范化的模型标识，用于缓存亲和性）
            attempt_counter: 尝试计数
            max_attempts: 最大尝试次数
            is_stream: 是否为流式请求

        Returns:
            (response, provider_name, candidate_record_id, provider_id, endpoint_id, key_id, ttfb_ms)

        Raises:
            ExecutionError: 执行失败时
        """
        provider = candidate.provider
        endpoint = candidate.endpoint
        key = candidate.key

        # 显式转换为 str
        provider_id = str(provider.id)
        provider_name = str(provider.name)
        endpoint_id = str(endpoint.id)
        key_id = str(key.id)
        cache_ttl_minutes = int(key.cache_ttl_minutes or 0)
        provider_supports_caching = cache_ttl_minutes > 0
        provider_cache_ttl_seconds: int | None = (
            cache_ttl_minutes * 60 if cache_ttl_minutes > 0 else None
        )

        # 更新状态为 pending
        RequestCandidateService.update_candidate_status(
            db=self.db, candidate_id=candidate_record_id, status="pending"
        )

        # 执行请求
        execution_result = await self.request_executor.execute(
            candidate=candidate,
            candidate_id=candidate_record_id,
            candidate_index=candidate_index,
            user_api_key=user_api_key,
            user_id=user_id,
            request_func=request_func,
            request_id=request_id,
            api_format=api_format,
            model_name=model_name,
            is_stream=is_stream,
        )

        context = execution_result.context
        elapsed_ms = context.elapsed_ms or 0

        # 流式请求：标记为 streaming 状态（请求尚未完成）
        # 非流式请求：标记为 success 状态
        # 注意：executor.execute() 内部已经处理了状态标记，这里不再重复
        # 流式请求的 success 状态会在流完成后由 _record_stream_stats 方法标记

        # 设置缓存亲和性
        should_refresh_cache_affinity = not bool(
            getattr(candidate, "cache_affinity_degraded", False)
        )
        if (
            provider_supports_caching
            and self.cache_scheduler is not None
            and should_refresh_cache_affinity
        ):
            try:
                await self.cache_scheduler.set_cache_affinity(
                    affinity_key=affinity_key,
                    provider_id=provider_id,
                    endpoint_id=endpoint_id,
                    key_id=key_id,
                    api_format=api_format,
                    global_model_id=global_model_id,
                    ttl=provider_cache_ttl_seconds,
                )
            except Exception as cache_exc:
                logger.warning(f"  [{request_id}] 设置缓存亲和性失败: {cache_exc}")
        elif provider_supports_caching and not should_refresh_cache_affinity:
            logger.debug(
                "  [{}] 跳过降级缓存亲和续期: Provider={}, Endpoint={}..., Key={}...",
                request_id,
                provider_name,
                endpoint_id[:8],
                key_id[:8],
            )

        logger.debug(f"  [{request_id}] 请求成功: Provider={provider_name}, 耗时={elapsed_ms}ms")

        # Non-stream requests don't have first-byte telemetry in this path.
        # Use elapsed latency as a conservative fallback for pool latency sampling.
        ttfb_ms: int | None = None
        if not is_stream:
            raw_ttfb = getattr(execution_result.response, "first_byte_time_ms", None)
            try:
                if raw_ttfb is not None:
                    ttfb_ms = max(int(raw_ttfb), 0)
            except (TypeError, ValueError):
                ttfb_ms = None
            if ttfb_ms is None and elapsed_ms >= 0:
                ttfb_ms = int(elapsed_ms)

        return (
            execution_result.response,
            provider_name,
            candidate_record_id,
            provider_id,
            endpoint_id,
            key_id,
            ttfb_ms,
        )
