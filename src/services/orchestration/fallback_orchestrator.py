"""
故障转移编排器（预取+顺序遍历策略）

功能：
1. 预先获取所有可用的 Provider/Endpoint/Key 组合
2. 按优先级顺序遍历组合（每个只尝试一次）
3. 集成 HealthMonitor 记录成功/失败
4. 集成 ConcurrencyManager 管理并发（支持缓存用户优先级）
5. 缓存亲和性管理（自动失效失败的Key）

优化亮点：
- 避免运行时重复查询数据库
- 精确控制重试次数（=实际组合数）
- 清晰的故障转移逻辑，易于维护和调试

重构说明：
- 职责已拆分到独立组件（src/services/orchestration/）：
  - CandidateResolver: 候选解析器，负责获取和排序可用的 Provider 组合
  - RequestDispatcher: 请求分发器，负责执行单个候选请求
  - ErrorClassifier: 错误分类器，负责错误分类和处理策略
- 本类作为协调者，组合使用上述组件
"""

from __future__ import annotations

from typing import Any, Callable, Dict, List, NoReturn, Optional, Tuple, Union

import httpx
from redis import Redis
from sqlalchemy.orm import Session

from src.core.enums import APIFormat
from src.core.error_utils import extract_error_message
from src.core.exceptions import (
    ConcurrencyLimitError,
    ProviderNotAvailableException,
    UpstreamClientException,
)
from src.core.logger import logger
from src.models.database import ApiKey, Provider, ProviderAPIKey, ProviderEndpoint
from src.services.cache.aware_scheduler import (
    CacheAwareScheduler,
    ProviderCandidate,
    get_cache_aware_scheduler,
)
from src.services.provider.format import normalize_api_format
from src.services.rate_limit.adaptive_rpm import get_adaptive_rpm_manager
from src.services.rate_limit.concurrency_manager import get_concurrency_manager
from src.services.request.candidate import RequestCandidateService
from src.services.request.executor import ExecutionError, RequestExecutor
from src.services.system.config import SystemConfigService

from .candidate_resolver import CandidateResolver
from .error_classifier import ErrorClassifier
from .request_dispatcher import RequestDispatcher


class FallbackOrchestrator:
    """
    故障转移编排器（预取+顺序遍历策略）

    负责协调请求的完整生命周期：
    1. 预先获取所有可用的 Provider+Endpoint+Key 组合（按优先级排序）
    2. 按顺序遍历每个组合，获取并发槽位（缓存用户优先）
    3. 发送请求
    4. 记录结果（成功/失败，更新健康度）
    5. 失败时自动切换到下一个组合，直到成功或全部失败

    故障转移策略（V2 - 预取优化）：
    - 启动时预先获取所有符合条件的 Provider/Endpoint/Key 组合
    - 按优先级排序：Provider.provider_priority → Key.internal_priority（Endpoint在Provider内唯一，无需排序）
    - 过滤条件：活跃状态、健康度、熔断器状态、模型支持、API格式匹配
    - 顺序遍历组合列表，每个组合只尝试一次
    - 重试次数 = 实际可用组合数（无固定上限，避免过度重试）
    - 优势：可预测、高效、公平、资源友好
    """

    def __init__(self, db: Session, redis_client: Optional[Redis] = None) -> None:
        """
        初始化编排器

        Args:
            db: 数据库会话
            redis_client: Redis客户端（可选，用于缓存和并发控制）
        """
        self.db = db
        self.redis = redis_client
        self.cache_scheduler: Optional[CacheAwareScheduler] = None
        self.concurrency_manager: Any = None
        self.adaptive_manager = get_adaptive_rpm_manager()  # 自适应 RPM 管理器
        self.request_executor: Optional[RequestExecutor] = None

        # 拆分后的组件（延迟初始化）
        self._candidate_resolver: Optional[CandidateResolver] = None
        self._request_dispatcher: Optional[RequestDispatcher] = None
        self._error_classifier: Optional[ErrorClassifier] = None

    async def _ensure_initialized(self) -> None:
        """确保异步组件已初始化"""
        if self.cache_scheduler is None:
            priority_mode = SystemConfigService.get_config(
                self.db,
                "provider_priority_mode",
                CacheAwareScheduler.PRIORITY_MODE_PROVIDER,
            )
            scheduling_mode = SystemConfigService.get_config(
                self.db,
                "scheduling_mode",
                CacheAwareScheduler.SCHEDULING_MODE_CACHE_AFFINITY,
            )
            self.cache_scheduler = await get_cache_aware_scheduler(
                self.redis,
                priority_mode=priority_mode,
                scheduling_mode=scheduling_mode,
            )
        else:
            # 确保运行时配置变更能生效
            priority_mode = SystemConfigService.get_config(
                self.db,
                "provider_priority_mode",
                CacheAwareScheduler.PRIORITY_MODE_PROVIDER,
            )
            scheduling_mode = SystemConfigService.get_config(
                self.db,
                "scheduling_mode",
                CacheAwareScheduler.SCHEDULING_MODE_CACHE_AFFINITY,
            )
            self.cache_scheduler.set_priority_mode(priority_mode)
            self.cache_scheduler.set_scheduling_mode(scheduling_mode)

        # 确保 cache_scheduler 内部组件也已初始化
        await self.cache_scheduler._ensure_initialized()

        if self.concurrency_manager is None:
            self.concurrency_manager = await get_concurrency_manager()

        if self.request_executor is None and self.concurrency_manager is not None:
            self.request_executor = RequestExecutor(
                db=self.db,
                concurrency_manager=self.concurrency_manager,
                adaptive_manager=self.adaptive_manager,
            )

        # 初始化拆分后的组件
        if self._candidate_resolver is None:
            self._candidate_resolver = CandidateResolver(
                db=self.db,
                cache_scheduler=self.cache_scheduler,
            )

        if self._error_classifier is None:
            self._error_classifier = ErrorClassifier(
                db=self.db,
                cache_scheduler=self.cache_scheduler,
                adaptive_manager=self.adaptive_manager,
            )

        if self._request_dispatcher is None and self.request_executor is not None:
            self._request_dispatcher = RequestDispatcher(
                db=self.db,
                request_executor=self.request_executor,
                cache_scheduler=self.cache_scheduler,
            )

    async def _fetch_all_candidates(
        self,
        api_format: APIFormat,
        model_name: str,
        affinity_key: str,
        user_api_key: Optional[ApiKey] = None,
        request_id: Optional[str] = None,
        is_stream: bool = False,
        capability_requirements: Optional[Dict[str, bool]] = None,
    ) -> Tuple[List[ProviderCandidate], str]:
        """
        收集所有可用的 Provider/Endpoint/Key 候选组合

        委托给 CandidateResolver 处理。

        Args:
            api_format: API 格式
            model_name: 模型名称
            affinity_key: 亲和性标识符（通常为API Key ID，用于缓存亲和性）
            user_api_key: 用户 API Key（用于 allowed_providers/allowed_api_formats 过滤）
            request_id: 请求 ID（用于日志）
            is_stream: 是否是流式请求，如果为 True 则过滤不支持流式的 Provider
            capability_requirements: 能力需求（用于过滤不满足能力要求的 Key）

        Returns:
            (所有候选组合的列表, global_model_id)

        Raises:
            ProviderNotAvailableException: 没有找到任何可用候选时
        """
        assert self._candidate_resolver is not None
        return await self._candidate_resolver.fetch_candidates(
            api_format=api_format,
            model_name=model_name,
            affinity_key=affinity_key,
            user_api_key=user_api_key,
            request_id=request_id,
            is_stream=is_stream,
            capability_requirements=capability_requirements,
        )

    def _create_candidate_records(
        self,
        all_candidates: List[ProviderCandidate],
        request_id: Optional[str],
        user_id: str,
        user_api_key: ApiKey,
        required_capabilities: Optional[Dict[str, bool]] = None,
    ) -> Dict[Tuple[int, int], str]:
        """
        为所有候选预先创建 available 状态记录（批量插入优化）

        委托给 CandidateResolver 处理。

        Args:
            all_candidates: 所有候选组合
            request_id: 请求 ID
            user_id: 用户 ID
            user_api_key: 用户 API Key 对象
            required_capabilities: 请求需要的能力标签

        Returns:
            candidate_record_map: {(candidate_index, retry_index): candidate_record_id}
        """
        assert self._candidate_resolver is not None
        return self._candidate_resolver.create_candidate_records(
            all_candidates=all_candidates,
            request_id=request_id,
            user_id=user_id,
            user_api_key=user_api_key,
            required_capabilities=required_capabilities,
        )

    async def _try_single_candidate(
        self,
        candidate: ProviderCandidate,
        candidate_index: int,
        retry_index: int,
        candidate_record_id: str,
        user_api_key: ApiKey,
        request_func: Callable[..., Any],
        request_id: Optional[str],
        api_format: APIFormat,
        model_name: str,
        affinity_key: str,
        global_model_id: str,
        attempt_counter: int,
        max_attempts: int,
        is_stream: bool = False,
    ) -> Tuple[Any, str, str, str, str, str]:
        """
        尝试单个候选执行请求

        委托给 RequestDispatcher 处理。

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
            (response, provider_name, candidate_record_id, provider_id, endpoint_id, key_id)

        Raises:
            ExecutionError: 执行失败时
        """
        assert self._request_dispatcher is not None
        return await self._request_dispatcher.dispatch(
            candidate=candidate,
            candidate_index=candidate_index,
            retry_index=retry_index,
            candidate_record_id=candidate_record_id,
            user_api_key=user_api_key,
            request_func=request_func,
            request_id=request_id,
            api_format=api_format,
            model_name=model_name,
            affinity_key=affinity_key,
            global_model_id=global_model_id,
            attempt_counter=attempt_counter,
            max_attempts=max_attempts,
            is_stream=is_stream,
        )

    async def _handle_candidate_error(
        self,
        exec_err: ExecutionError,
        candidate: ProviderCandidate,
        candidate_record_id: str,
        retry_index: int,
        max_retries_for_candidate: int,
        affinity_key: str,
        api_format: APIFormat,
        global_model_id: str,
        request_id: Optional[str],
        attempt: int,
        max_attempts: int,
    ) -> str:
        """
        处理候选执行错误

        Args:
            exec_err: 执行错误
            candidate: 候选对象
            candidate_record_id: 候选记录 ID
            retry_index: 当前重试索引
            max_retries_for_candidate: 该候选的最大重试次数
            affinity_key: 亲和性标识符（通常为API Key ID）
            api_format: API 格式
            global_model_id: GlobalModel ID（规范化的模型标识）
            request_id: 请求 ID
            attempt: 当前尝试次数
            max_attempts: 最大尝试次数

        Returns:
            action: "continue" (继续重试), "break" (跳到下一个候选), "raise" (抛出异常)
        """
        provider = candidate.provider
        endpoint = candidate.endpoint
        key = candidate.key

        context = exec_err.context
        captured_key_concurrent = context.concurrent_requests
        elapsed_ms = context.elapsed_ms
        cause = exec_err.cause

        has_retry_left = retry_index < (max_retries_for_candidate - 1)

        # 确保 error_classifier 已初始化
        assert self._error_classifier is not None, "ErrorClassifier not initialized"

        if isinstance(cause, ConcurrencyLimitError):
            logger.warning(f"  [{request_id}] 并发限制 (attempt={attempt}/{max_attempts}): {cause}")
            RequestCandidateService.mark_candidate_skipped(
                db=self.db,
                candidate_id=candidate_record_id,
                skip_reason=f"并发限制: {str(cause)}",
            )
            return "break"

        if isinstance(cause, httpx.HTTPStatusError):
            status_code = cause.response.status_code
            # 使用 ErrorClassifier 处理 HTTP 错误
            extra_data = await self._error_classifier.handle_http_error(
                http_error=cause,
                provider=provider,
                endpoint=endpoint,
                key=key,
                affinity_key=affinity_key,
                api_format=api_format,
                global_model_id=global_model_id,
                request_id=request_id,
                captured_key_concurrent=captured_key_concurrent,
                elapsed_ms=elapsed_ms,
                max_attempts=max_attempts,
                attempt=attempt,
            )

            # 检查是否为客户端请求错误（不应重试）
            converted_error = extra_data.get("converted_error")
            # 从 extra_data 中移除 converted_error，避免序列化问题
            serializable_extra_data = {k: v for k, v in extra_data.items() if k != "converted_error"}

            if isinstance(converted_error, UpstreamClientException):
                logger.warning(f"  [{request_id}] 客户端请求错误，停止重试: {converted_error.message}")
                RequestCandidateService.mark_candidate_failed(
                    db=self.db,
                    candidate_id=candidate_record_id,
                    error_type="UpstreamClientException",
                    error_message=converted_error.message,
                    status_code=status_code,
                    latency_ms=elapsed_ms,
                    concurrent_requests=captured_key_concurrent,
                    extra_data=serializable_extra_data,
                )
                # 重新包装异常，附加 request_metadata 以便记录 usage
                converted_error.request_metadata = {
                    "provider": provider.name,
                    "provider_id": str(provider.id),
                    "provider_endpoint_id": str(endpoint.id),
                    "provider_api_key_id": str(key.id),
                    "api_format": api_format.value if hasattr(api_format, "value") else str(api_format),
                }
                raise converted_error

            RequestCandidateService.mark_candidate_failed(
                db=self.db,
                candidate_id=candidate_record_id,
                error_type="HTTPStatusError",
                error_message=extract_error_message(cause, status_code),
                status_code=status_code,
                latency_ms=elapsed_ms,
                concurrent_requests=captured_key_concurrent,
                extra_data=serializable_extra_data,
            )
            return "continue" if has_retry_left else "break"

        if isinstance(cause, self._error_classifier.RETRIABLE_ERRORS):
            # 使用 ErrorClassifier 处理可重试错误
            await self._error_classifier.handle_retriable_error(
                error=cause,
                provider=provider,
                endpoint=endpoint,
                key=key,
                affinity_key=affinity_key,
                api_format=api_format,
                global_model_id=global_model_id,
                captured_key_concurrent=captured_key_concurrent,
                elapsed_ms=elapsed_ms,
                request_id=request_id,
                attempt=attempt,
                max_attempts=max_attempts,
            )
            RequestCandidateService.mark_candidate_failed(
                db=self.db,
                candidate_id=candidate_record_id,
                error_type=type(cause).__name__,
                error_message=extract_error_message(cause),
                latency_ms=elapsed_ms,
                concurrent_requests=captured_key_concurrent,
            )
            return "continue" if has_retry_left else "break"

        # 未知错误：记录失败并抛出
        RequestCandidateService.mark_candidate_failed(
            db=self.db,
            candidate_id=candidate_record_id,
            error_type=type(cause).__name__,
            error_message=extract_error_message(cause),
            latency_ms=elapsed_ms,
            concurrent_requests=captured_key_concurrent,
        )
        return "raise"

    def _create_pending_usage_record(
        self,
        request_id: Optional[str],
        user_api_key: ApiKey,
        model_name: str,
        is_stream: bool,
        api_format_enum: APIFormat,
    ) -> None:
        """创建 pending 状态的使用记录（用于实时状态追踪）"""
        if not request_id:
            return

        from src.services.usage.service import UsageService

        try:
            from src.models.database import User

            user = self.db.query(User).filter(User.id == user_api_key.user_id).first()
            UsageService.create_pending_usage(
                db=self.db,
                request_id=request_id,
                user=user,
                api_key=user_api_key,
                model=model_name,
                is_stream=is_stream,
                api_format=api_format_enum.value,
            )
        except Exception as e:
            # 创建 pending 记录失败不应阻塞请求
            logger.warning(f"创建 pending 使用记录失败: {e}")

    async def _execute_candidates_loop(
        self,
        all_candidates: List[ProviderCandidate],
        candidate_record_map: Dict[Tuple[int, int], str],
        user_api_key: ApiKey,
        request_func: Callable[..., Any],
        request_id: Optional[str],
        api_format_enum: APIFormat,
        model_name: str,
        affinity_key: str,
        global_model_id: str,
        is_stream: bool = False,
    ) -> Tuple[Any, str, Optional[str], Optional[str], Optional[str], Optional[str]]:
        """遍历所有候选执行请求，返回第一个成功的结果或抛出异常"""
        attempt_counter = 0
        max_attempts = 0
        last_error: Optional[Exception] = None
        last_candidate: Optional[ProviderCandidate] = None

        for candidate_index, candidate in enumerate(all_candidates):
            last_candidate = candidate

            if candidate.is_skipped:
                logger.debug(f"  [{request_id}] 跳过候选: Provider={candidate.provider.name}, "
                    f"Reason={candidate.skip_reason}")
                continue

            result = await self._try_candidate_with_retries(
                candidate=candidate,
                candidate_index=candidate_index,
                candidate_record_map=candidate_record_map,
                user_api_key=user_api_key,
                request_func=request_func,
                request_id=request_id,
                api_format_enum=api_format_enum,
                model_name=model_name,
                affinity_key=affinity_key,
                global_model_id=global_model_id,
                attempt_counter=attempt_counter,
                max_attempts=max_attempts,
                is_stream=is_stream,
            )

            if result["success"]:
                response: Tuple[Any, str, Optional[str], Optional[str], Optional[str], Optional[str]] = result["response"]
                return response

            # 更新计数器和错误信息
            attempt_counter = result["attempt_counter"]
            max_attempts = result["max_attempts"]
            if result.get("error"):
                last_error = result["error"]
            if result.get("should_raise") and last_error is not None:
                self._attach_metadata_to_error(last_error, last_candidate, model_name, api_format_enum)
                raise last_error

        # 所有组合都已尝试完毕，全部失败
        self._raise_all_failed_exception(
            request_id, max_attempts, last_candidate, model_name, api_format_enum, last_error
        )

    async def _try_candidate_with_retries(
        self,
        candidate: ProviderCandidate,
        candidate_index: int,
        candidate_record_map: Dict[Tuple[int, int], str],
        user_api_key: ApiKey,
        request_func: Callable[..., Any],
        request_id: Optional[str],
        api_format_enum: APIFormat,
        model_name: str,
        affinity_key: str,
        global_model_id: str,
        attempt_counter: int,
        max_attempts: int,
        is_stream: bool = False,
    ) -> Dict[str, Any]:
        """尝试单个候选（含重试逻辑），返回执行结果"""
        provider = candidate.provider
        endpoint = candidate.endpoint
        # 从 Provider 读取 max_retries（已从 Endpoint 迁移）
        max_retries_for_candidate = int(provider.max_retries or 2) if candidate.is_cached else 1
        last_error: Optional[Exception] = None

        for retry_index in range(max_retries_for_candidate):
            attempt_counter += 1
            max_attempts = max(max_attempts, attempt_counter)

            if retry_index == 0:
                # 首次尝试该候选
                cache_hint = " (cached)" if candidate.is_cached else ""
                logger.info(f"  [{request_id[:8] if request_id else 'N/A'}] -> {provider.name}{cache_hint}")
            else:
                logger.info(f"  [{request_id[:8] if request_id else 'N/A'}] -> {provider.name} (retry {retry_index})")

            candidate_record_id = candidate_record_map[(candidate_index, retry_index)]

            try:
                response = await self._try_single_candidate(
                    candidate=candidate,
                    candidate_index=candidate_index,
                    retry_index=retry_index,
                    candidate_record_id=candidate_record_id,
                    user_api_key=user_api_key,
                    request_func=request_func,
                    request_id=request_id,
                    api_format=api_format_enum,
                    model_name=model_name,
                    affinity_key=affinity_key,
                    global_model_id=global_model_id,
                    attempt_counter=attempt_counter,
                    max_attempts=max_attempts,
                    is_stream=is_stream,
                )
                return {"success": True, "response": response}

            except ExecutionError as exec_err:
                last_error = exec_err.cause
                action = await self._handle_candidate_error(
                    exec_err=exec_err,
                    candidate=candidate,
                    candidate_record_id=candidate_record_id,
                    retry_index=retry_index,
                    max_retries_for_candidate=max_retries_for_candidate,
                    affinity_key=affinity_key,
                    api_format=api_format_enum,
                    global_model_id=global_model_id,
                    request_id=request_id,
                    attempt=attempt_counter,
                    max_attempts=max_attempts,
                )

                if action == "continue":
                    continue
                elif action == "break":
                    break
                elif action == "raise":
                    return {
                        "success": False,
                        "should_raise": True,
                        "error": exec_err.cause,
                        "attempt_counter": attempt_counter,
                        "max_attempts": max_attempts,
                    }

        return {
            "success": False,
            "attempt_counter": attempt_counter,
            "max_attempts": max_attempts,
            "error": last_error,
        }

    def _attach_metadata_to_error(
        self,
        error: Optional[Exception],
        candidate: Optional[ProviderCandidate],
        model_name: str,
        api_format_enum: APIFormat,
    ) -> None:
        """附加 candidate 信息到异常，以便记录 usage"""
        if not error or not candidate:
            return

        from src.services.request.result import RequestMetadata

        existing_metadata = getattr(error, "request_metadata", None)
        if existing_metadata and getattr(existing_metadata, "api_format", None):
            return  # 已有完整的 metadata

        metadata = RequestMetadata(
            provider_request_headers=(
                getattr(existing_metadata, "provider_request_headers", {})
                if existing_metadata
                else {}
            ),
            provider=getattr(existing_metadata, "provider", None) or str(candidate.provider.name),
            model=getattr(existing_metadata, "model", None) or model_name,
            provider_id=getattr(existing_metadata, "provider_id", None) or str(candidate.provider.id),
            provider_endpoint_id=(
                getattr(existing_metadata, "provider_endpoint_id", None)
                or str(candidate.endpoint.id)
            ),
            provider_api_key_id=(
                getattr(existing_metadata, "provider_api_key_id", None)
                or str(candidate.key.id)
            ),
            api_format=api_format_enum.value,
        )
        # 使用 setattr 避免类型检查错误
        setattr(error, "request_metadata", metadata)

    def _raise_all_failed_exception(
        self,
        request_id: Optional[str],
        max_attempts: int,
        last_candidate: Optional[ProviderCandidate],
        model_name: str,
        api_format_enum: APIFormat,
        last_error: Optional[Exception] = None,
    ) -> NoReturn:
        """所有组合都失败时抛出异常"""
        logger.error(f"  [{request_id}] 所有 {max_attempts} 个组合均失败")

        request_metadata = None
        if last_candidate:
            request_metadata = {
                "provider": last_candidate.provider.name,
                "model": model_name,
                "provider_id": str(last_candidate.provider.id),
                "provider_endpoint_id": str(last_candidate.endpoint.id),
                "provider_api_key_id": str(last_candidate.key.id),
                "api_format": api_format_enum.value,
            }

        # 提取上游错误响应
        upstream_status: Optional[int] = None
        upstream_response: Optional[str] = None
        if last_error:
            # 从 httpx.HTTPStatusError 提取
            if isinstance(last_error, httpx.HTTPStatusError):
                upstream_status = last_error.response.status_code
                # 优先使用我们附加的 upstream_response 属性（流已读取时 response.text 可能为空）
                upstream_response = getattr(last_error, "upstream_response", None)
                if not upstream_response:
                    try:
                        upstream_response = last_error.response.text
                    except Exception:
                        pass
            # 从其他异常属性提取（如 ProviderNotAvailableException）
            else:
                upstream_status = getattr(last_error, "upstream_status", None)
                upstream_response = getattr(last_error, "upstream_response", None)

            # 如果响应为空或无效，使用异常的字符串表示作为 upstream_response
            if (
                not upstream_response
                or not upstream_response.strip()
                or upstream_response.startswith("Unable to read")
            ):
                upstream_response = str(last_error)

        # 构建友好的错误消息（用于返回给客户端，不暴露内部信息）
        # 如果 last_error 有 message 属性，优先使用（已经是友好提示）
        # 否则使用通用提示
        friendly_message = "服务暂时不可用，请稍后重试"
        if last_error:
            last_error_message = getattr(last_error, "message", None)
            if last_error_message and isinstance(last_error_message, str):
                friendly_message = last_error_message

        raise ProviderNotAvailableException(
            friendly_message,
            request_metadata=request_metadata,
            upstream_status=upstream_status,
            upstream_response=upstream_response,
        )

    async def execute_with_fallback(
        self,
        api_format: Union[str, APIFormat],
        model_name: str,
        user_api_key: ApiKey,
        request_func: Callable[[Provider, ProviderEndpoint, ProviderAPIKey], Any],
        request_id: Optional[str] = None,
        is_stream: bool = False,
        capability_requirements: Optional[Dict[str, bool]] = None,
    ) -> Tuple[Any, str, Optional[str], Optional[str], Optional[str], Optional[str]]:
        """
        执行请求，并在失败时自动故障转移（缓存感知）

        Args:
            api_format: API 格式（如 'CLAUDE', 'OPENAI'）
            model_name: 模型名称
            user_api_key: 用户的 API Key对象
            request_func: 请求函数，接收 (provider, endpoint, key) 参数，返回响应
            request_id: 请求 ID（用于日志）
            is_stream: 是否是流式请求，如果为 True 则过滤不支持流式的 Provider
            capability_requirements: 能力需求（用于过滤不满足能力要求的 Key）

        Returns:
            (请求响应, 实际Provider名称, RequestTraceAttempt ID, provider_id, endpoint_id, key_id)

        Raises:
            ProviderNotAvailableException: 所有 Providers 都失败后抛出
        """
        await self._ensure_initialized()

        # 准备执行上下文
        affinity_key = str(user_api_key.id)
        user_id = str(user_api_key.user_id)
        api_format_enum = normalize_api_format(api_format)

        logger.debug(f"[FallbackOrchestrator] execute_with_fallback 被调用: "
            f"api_format={api_format_enum.value}, model_name={model_name}, "
            f"request_id={request_id}, is_stream={is_stream}")

        # 创建 pending 状态的使用记录
        self._create_pending_usage_record(request_id, user_api_key, model_name, is_stream, api_format_enum)

        # 1. 收集所有候选（同时获取规范化的 global_model_id 用于缓存亲和性）
        all_candidates, global_model_id = await self._fetch_all_candidates(
            api_format=api_format_enum,
            model_name=model_name,
            affinity_key=affinity_key,
            user_api_key=user_api_key,
            request_id=request_id,
            is_stream=is_stream,
            capability_requirements=capability_requirements,
        )

        # 2. 批量创建候选记录
        candidate_record_map = self._create_candidate_records(
            all_candidates=all_candidates,
            request_id=request_id,
            user_id=user_id,
            user_api_key=user_api_key,
            required_capabilities=capability_requirements,
        )

        # 3. 遍历候选执行请求（使用 global_model_id 用于缓存亲和性）
        return await self._execute_candidates_loop(
            all_candidates=all_candidates,
            candidate_record_map=candidate_record_map,
            user_api_key=user_api_key,
            request_func=request_func,
            request_id=request_id,
            api_format_enum=api_format_enum,
            model_name=model_name,
            affinity_key=affinity_key,
            global_model_id=global_model_id,
            is_stream=is_stream,
        )
