from __future__ import annotations

import re
from collections.abc import Callable
from typing import Any
from uuid import uuid4

import httpx
from sqlalchemy.orm import Session

from src.config.settings import config
from src.core.error_utils import extract_error_message
from src.core.exceptions import (
    ConcurrencyLimitError,
    EmbeddedErrorException,
    ProviderNotAvailableException,
    ProxyNodeUnavailableError,
    ThinkingSignatureException,
    UpstreamClientException,
)
from src.core.logger import logger
from src.core.provider_types import ProviderType
from src.models.database import (
    ApiKey,
    Provider,
    ProviderAPIKey,
    ProviderEndpoint,
    RequestCandidate,
    Usage,
    User,
    VideoTask,
)
from src.services.candidate.failover import FailoverEngine
from src.services.candidate.policy import RetryPolicy, SkipPolicy
from src.services.candidate.recorder import CandidateRecorder
from src.services.candidate.resolver import CandidateResolver
from src.services.orchestration.error_classifier import ErrorClassifier
from src.services.orchestration.request_dispatcher import RequestDispatcher
from src.services.provider.format import normalize_endpoint_signature
from src.services.request.candidate import RequestCandidateService
from src.services.request.result import RequestMetadata
from src.services.scheduling.aware_scheduler import (
    CacheAwareScheduler,
    get_cache_aware_scheduler,
)
from src.services.system.config import SystemConfigService
from src.services.task.context import TaskMode
from src.services.task.exceptions import TaskNotFoundError
from src.services.task.protocol import AttemptKind, AttemptResult
from src.services.task.schema import ExecutionResult, TaskStatusResult
from src.services.usage.service import UsageService

_SENSITIVE_PATTERN = re.compile(
    r"(api[_-]?key|token|bearer|authorization)[=:\s]+\S+",
    re.IGNORECASE,
)


class TaskService:
    """
    Unified task service facade (Phase 3).

    Phase 3.1 scope:
    - Provide a single entrypoint for SYNC tasks.
    - Keep behavior consistent with the pre-Phase-3 implementation.
    - Return a structured `ExecutionResult` for downstream compatibility.
    """

    def __init__(self, db: Session, redis_client: Any | None = None) -> None:
        self.db = db
        self.redis = redis_client
        self._recorder = CandidateRecorder(db)

    async def execute(
        self,
        *,
        task_type: str,  # chat/cli/video/image
        task_mode: TaskMode,
        api_format: str,
        model_name: str,
        user_api_key: ApiKey,
        request_func: Callable[..., Any],
        request_id: str | None = None,
        is_stream: bool = False,
        capability_requirements: dict[str, bool] | None = None,
        preferred_key_ids: list[str] | None = None,
        request_body_ref: dict[str, Any] | None = None,
        request_headers: dict[str, Any] | None = None,
        request_body: dict[str, Any] | None = None,
        # ASYNC-only (video submit)
        extract_external_task_id: Any | None = None,
        supported_auth_types: set[str] | None = None,
        allow_format_conversion: bool = False,
        max_candidates: int | None = None,
    ) -> ExecutionResult:
        """
        Unified execute entrypoint.

        Currently supports:
        - SYNC (chat/cli): FailoverEngine-driven execution (behavior parity with prior implementation).
        - ASYNC (video): submit via `TaskService.submit_with_failover()` and return task_id.
        - video task poll/finalize helpers.
        """
        if task_mode == TaskMode.ASYNC:
            # Phase 3.2+: unified async submit entrypoint.
            if extract_external_task_id is None:
                raise ValueError("extract_external_task_id is required for task_mode=ASYNC")

            outcome = await self.submit_with_failover(
                api_format=api_format,
                model_name=model_name,
                affinity_key=str(user_api_key.id),
                user_api_key=user_api_key,
                request_id=request_id,
                task_type=task_type,
                submit_func=request_func,
                extract_external_task_id=extract_external_task_id,
                supported_auth_types=supported_auth_types,
                allow_format_conversion=allow_format_conversion,
                capability_requirements=capability_requirements,
                max_candidates=max_candidates,
            )

            candidate_keys = []
            if request_id:
                try:
                    candidate_keys = self._recorder.get_candidate_keys(request_id)
                except Exception:
                    candidate_keys = []

            selected_idx = -1
            if candidate_keys:
                for ck in candidate_keys:
                    if str(getattr(ck, "status", "")) == "success":
                        idx_val = getattr(ck, "candidate_index", -1)
                        selected_idx = int(idx_val) if idx_val is not None else -1
                        break

            attempt_count = 0
            if candidate_keys:
                attempt_count = sum(
                    1
                    for ck in candidate_keys
                    if str(getattr(ck, "status", ""))
                    in {"pending", "success", "failed", "cancelled"}
                )

            attempt_result = AttemptResult(
                kind=AttemptKind.ASYNC_SUBMIT,
                http_status=int(outcome.upstream_status_code or 200),
                http_headers=dict(outcome.upstream_headers or {}),
                provider_task_id=str(outcome.external_task_id),
                response_body=outcome.upstream_payload,
            )

            return ExecutionResult(
                success=True,
                attempt_result=attempt_result,
                candidate=outcome.candidate,
                candidate_index=selected_idx,
                retry_index=0,
                provider_id=str(outcome.candidate.provider.id),
                provider_name=str(outcome.candidate.provider.name),
                endpoint_id=str(outcome.candidate.endpoint.id),
                key_id=str(outcome.candidate.key.id),
                candidate_keys=candidate_keys,
                attempt_count=attempt_count,
                request_candidate_id=None,
            )

        _ = task_type  # reserved for future routing (chat/cli/video/image)

        # Phase 3+: handler 层统一走 TaskService（统一内核）。

        return await self._execute_sync_unified(
            api_format=api_format,
            model_name=model_name,
            user_api_key=user_api_key,
            request_func=request_func,
            request_id=request_id,
            is_stream=is_stream,
            capability_requirements=capability_requirements,
            preferred_key_ids=preferred_key_ids,
            request_body_ref=request_body_ref,
            request_headers=request_headers,
            request_body=request_body,
        )

    async def _execute_sync_unified(
        self,
        *,
        api_format: str,
        model_name: str,
        user_api_key: ApiKey,
        request_func: Callable[..., Any],
        request_id: str | None,
        is_stream: bool,
        capability_requirements: dict[str, bool] | None,
        preferred_key_ids: list[str] | None,
        request_body_ref: dict[str, Any] | None,
        request_headers: dict[str, Any] | None,
        request_body: dict[str, Any] | None,
    ) -> ExecutionResult:
        """
        Unified candidate traversal loop for SYNC.

        This intentionally reuses existing components for parity:
        - CandidateResolver fetch + record creation
        - RequestDispatcher execution
        - Error classification/rectify logic ported from the previous SYNC implementation
        """
        from src.services.rate_limit.adaptive_rpm import get_adaptive_rpm_manager
        from src.services.rate_limit.concurrency_manager import get_concurrency_manager
        from src.services.request.executor import RequestExecutor

        if not request_id:
            request_id = str(uuid4())

        # Build execution components (mirrors pre-Phase-3 initialization)
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
        cache_scheduler = await get_cache_aware_scheduler(
            self.redis,
            priority_mode=priority_mode,
            scheduling_mode=scheduling_mode,
        )
        # Ensure cache_scheduler inner state is ready
        await cache_scheduler._ensure_initialized()

        concurrency_manager = await get_concurrency_manager()
        adaptive_manager = get_adaptive_rpm_manager()
        request_executor = RequestExecutor(
            db=self.db,
            concurrency_manager=concurrency_manager,
            adaptive_manager=adaptive_manager,
        )
        candidate_resolver = CandidateResolver(
            db=self.db,
            cache_scheduler=cache_scheduler,
        )
        error_classifier = ErrorClassifier(
            db=self.db,
            cache_scheduler=cache_scheduler,
            adaptive_manager=adaptive_manager,
        )
        request_dispatcher = RequestDispatcher(
            db=self.db,
            request_executor=request_executor,
            cache_scheduler=cache_scheduler,
        )

        affinity_key = str(user_api_key.id)
        user_id = str(user_api_key.user_id)
        api_format_norm = normalize_endpoint_signature(api_format)

        # Keep pending usage creation behavior consistent with previous behavior
        try:
            user = self.db.query(User).filter(User.id == user_api_key.user_id).first()
            UsageService.create_pending_usage(
                db=self.db,
                request_id=request_id,
                user=user,
                api_key=user_api_key,
                model=model_name,
                is_stream=is_stream,
                api_format=api_format_norm,
                request_headers=request_headers,
                request_body=request_body,
            )
        except Exception as exc:
            logger.warning("创建 pending 使用记录失败: {}", str(exc))

        all_candidates, global_model_id = await candidate_resolver.fetch_candidates(
            api_format=api_format_norm,
            model_name=model_name,
            affinity_key=affinity_key,
            user_api_key=user_api_key,
            request_id=request_id,
            is_stream=is_stream,
            capability_requirements=capability_requirements,
            preferred_key_ids=preferred_key_ids,
        )

        candidate_record_map = candidate_resolver.create_candidate_records(
            all_candidates=all_candidates,
            request_id=request_id,
            user_id=user_id,
            user_api_key=user_api_key,
            required_capabilities=capability_requirements,
        )

        max_attempts = candidate_resolver.count_total_attempts(all_candidates)
        last_error: Exception | None = None
        # Keep behavior consistent with previous behavior: last_candidate is updated even if skipped.
        last_candidate: Any | None = all_candidates[-1] if all_candidates else None

        async def _attempt(candidate: Any) -> AttemptResult:
            nonlocal last_candidate
            last_candidate = candidate

            candidate_index = int(getattr(candidate, "_utf_candidate_index", -1))
            retry_index = int(getattr(candidate, "_utf_retry_index", 0))
            candidate_record_id = str(getattr(candidate, "_utf_candidate_record_id", "") or "")
            attempt_counter = int(getattr(candidate, "_utf_attempt_count", 0))
            max_attempts_local = int(getattr(candidate, "_utf_max_attempts", max_attempts))

            # Safety net: if record_id missing, create an "available" record on-demand.
            if not candidate_record_id:
                created = RequestCandidateService.create_candidate(
                    db=self.db,
                    request_id=request_id,
                    candidate_index=candidate_index,
                    retry_index=retry_index,
                    user_id=user_id,
                    api_key_id=str(user_api_key.id),
                    provider_id=str(candidate.provider.id),
                    endpoint_id=str(candidate.endpoint.id),
                    key_id=str(candidate.key.id),
                    status="available",
                    is_cached=bool(getattr(candidate, "is_cached", False)),
                )
                candidate_record_id = str(created.id)
                candidate_record_map[(candidate_index, retry_index)] = candidate_record_id

            response, _provider_name, attempt_id, _provider_id, _endpoint_id, _key_id = (
                await request_dispatcher.dispatch(
                    candidate=candidate,
                    candidate_index=candidate_index,
                    retry_index=retry_index,
                    candidate_record_id=candidate_record_id,
                    user_api_key=user_api_key,
                    request_func=request_func,
                    request_id=request_id,
                    api_format=api_format_norm,
                    model_name=model_name,
                    affinity_key=affinity_key,
                    global_model_id=global_model_id,
                    attempt_counter=attempt_counter,
                    max_attempts=max_attempts_local,
                    is_stream=is_stream,
                )
            )
            _ = (attempt_id, _provider_name, _provider_id, _endpoint_id, _key_id)

            if is_stream:
                return AttemptResult(
                    kind=AttemptKind.STREAM,
                    http_status=200,
                    http_headers={},
                    stream_iterator=response,
                )
            return AttemptResult(
                kind=AttemptKind.SYNC_RESPONSE,
                http_status=200,
                http_headers={},
                response_body=response,
            )

        async def _handle_exec_err(
            *,
            exec_err: Any,
            candidate: Any,
            candidate_index: int,
            retry_index: int,
            max_retries_for_candidate: int,
            record_id: str | None,
            attempt_count: int,
            max_attempts: int | None,
        ) -> tuple[Any, int | None]:
            nonlocal last_error, last_candidate
            last_candidate = candidate
            last_error = getattr(exec_err, "cause", None)

            # Fall back to retry 0 record if needed (rectify may extend retries).
            candidate_record_id = str(record_id or "") or str(
                candidate_record_map.get((candidate_index, 0), "")
            )

            action = await self._handle_candidate_error(
                exec_err=exec_err,
                candidate=candidate,
                candidate_record_id=candidate_record_id,
                retry_index=retry_index,
                max_retries_for_candidate=max_retries_for_candidate,
                affinity_key=affinity_key,
                api_format=api_format_norm,
                global_model_id=global_model_id,
                request_id=request_id,
                attempt=attempt_count,
                max_attempts=int(max_attempts or 0),
                request_body_ref=request_body_ref,
                error_classifier=error_classifier,
            )

            if action == "continue":
                new_max = None
                # Rectify may extend one more retry (keep previous behavior)
                if request_body_ref and request_body_ref.get("_rectified_this_turn", False):
                    request_body_ref["_rectified_this_turn"] = False
                    new_max = max(max_retries_for_candidate, retry_index + 2)
                return ("retry", new_max)

            if action == "break":
                return ("continue", None)

            if action == "raise":
                if last_error is not None:
                    self._attach_metadata_to_error(
                        last_error, last_candidate, model_name, api_format_norm
                    )
                    raise last_error
                raise

            # Unknown action: fail safe -> continue to next candidate.
            return ("continue", None)

        engine = FailoverEngine(
            self.db,
            error_classifier=error_classifier,
            recorder=self._recorder,
        )
        result = await engine.execute(
            candidates=all_candidates,
            attempt_func=_attempt,
            retry_policy=RetryPolicy.for_sync_task(),
            skip_policy=SkipPolicy(),
            request_id=request_id,
            user_id=user_id,
            api_key_id=str(user_api_key.id),
            candidate_record_map=candidate_record_map,
            max_attempts=max_attempts,
            execution_error_handler=_handle_exec_err,
        )

        if result.success:
            return result

        self._raise_all_failed_exception(
            request_id, max_attempts, last_candidate, model_name, api_format_norm, last_error
        )

    def _attach_metadata_to_error(
        self,
        error: Exception | None,
        candidate: Any | None,
        model_name: str,
        api_format: str,
    ) -> None:
        """Attach candidate metadata onto exception for usage recording."""
        if not error or not candidate:
            return

        existing_metadata = getattr(error, "request_metadata", None)
        if existing_metadata and getattr(existing_metadata, "api_format", None):
            return

        metadata = RequestMetadata(
            provider_request_headers=(
                getattr(existing_metadata, "provider_request_headers", {})
                if existing_metadata
                else {}
            ),
            provider=getattr(existing_metadata, "provider", None) or str(candidate.provider.name),
            model=getattr(existing_metadata, "model", None) or model_name,
            provider_id=getattr(existing_metadata, "provider_id", None)
            or str(candidate.provider.id),
            provider_endpoint_id=(
                getattr(existing_metadata, "provider_endpoint_id", None)
                or str(candidate.endpoint.id)
            ),
            provider_api_key_id=(
                getattr(existing_metadata, "provider_api_key_id", None) or str(candidate.key.id)
            ),
            api_format=api_format,
        )
        setattr(error, "request_metadata", metadata)

    def _raise_all_failed_exception(
        self,
        request_id: str | None,
        max_attempts: int,
        last_candidate: Any | None,
        model_name: str,
        api_format: str,
        last_error: Exception | None = None,
    ) -> None:
        """Raise a unified 'all candidates failed' exception."""
        logger.error("  [{}] 所有 {} 个组合均失败", request_id, max_attempts)

        request_metadata = None
        if last_candidate:
            request_metadata = {
                "provider": last_candidate.provider.name,
                "model": model_name,
                "provider_id": str(last_candidate.provider.id),
                "provider_endpoint_id": str(last_candidate.endpoint.id),
                "provider_api_key_id": str(last_candidate.key.id),
                "api_format": api_format,
            }

        upstream_status: int | None = None
        upstream_response: str | None = None
        if last_error:
            if isinstance(last_error, httpx.HTTPStatusError):
                upstream_status = last_error.response.status_code
                upstream_response = getattr(last_error, "upstream_response", None)
                if not upstream_response:
                    try:
                        upstream_response = last_error.response.text
                    except Exception:
                        pass
            else:
                upstream_status = getattr(last_error, "upstream_status", None)
                upstream_response = getattr(last_error, "upstream_response", None)

            if (
                not upstream_response
                or not upstream_response.strip()
                or upstream_response.startswith("Unable to read")
            ):
                upstream_response = str(last_error)

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

    def _mark_thinking_error_failed(
        self,
        candidate_record_id: str,
        error: Any,
        elapsed_ms: int,
        captured_key_concurrent: int | None,
        extra_data: dict[str, Any],
    ) -> None:
        """Mark ThinkingSignatureException as failed for the candidate."""
        if not isinstance(error, ThinkingSignatureException):
            return

        RequestCandidateService.mark_candidate_failed(
            db=self.db,
            candidate_id=candidate_record_id,
            error_type="ThinkingSignatureException",
            error_message=str(error),
            status_code=400,
            latency_ms=elapsed_ms,
            concurrent_requests=captured_key_concurrent,
            extra_data=extra_data,
        )

    def _handle_thinking_signature_error(
        self,
        *,
        converted_error: Any,
        provider_type: str | None,
        model_name: str | None,
        request_id: str | None,
        candidate_record_id: str,
        elapsed_ms: int,
        captured_key_concurrent: int | None,
        serializable_extra_data: dict[str, Any],
        request_body_ref: dict[str, Any] | None,
    ) -> str:
        """Try to rectify thinking signature errors and request a retry."""
        from src.services.message.thinking_rectifier import ThinkingRectifier

        if not isinstance(converted_error, ThinkingSignatureException):
            raise converted_error

        if not config.thinking_rectifier_enabled:
            logger.info("  [{}] Thinking 错误：整流器已禁用，终止重试", request_id)
            self._mark_thinking_error_failed(
                candidate_record_id,
                converted_error,
                elapsed_ms,
                captured_key_concurrent,
                serializable_extra_data,
            )
            raise converted_error

        if request_body_ref is None:
            logger.warning("  [{}] Thinking 错误：无法获取请求体引用，终止重试", request_id)
            self._mark_thinking_error_failed(
                candidate_record_id,
                converted_error,
                elapsed_ms,
                captured_key_concurrent,
                serializable_extra_data,
            )
            raise converted_error

        provider_type_norm = str(provider_type or "").lower()

        # Rectification may have multiple stages (Antigravity only).
        stage_raw = request_body_ref.get("_rectify_stage", 0)
        try:
            stage = int(stage_raw or 0)
        except Exception:
            stage = 0
        if stage <= 0 and request_body_ref.get("_rectified", False):
            stage = 1

        if stage >= 2 or (stage >= 1 and provider_type_norm != ProviderType.ANTIGRAVITY):
            logger.warning("  [{}] Thinking 错误：已整流仍失败，终止重试", request_id)
            self._mark_thinking_error_failed(
                candidate_record_id,
                converted_error,
                elapsed_ms,
                captured_key_concurrent,
                {**serializable_extra_data, "rectified": True, "rectify_stage": stage},
            )
            raise converted_error

        request_body = request_body_ref.get("body", {})

        stage_label = "thinking_only"
        next_stage = 1
        if stage == 0:
            rectified_body, modified = ThinkingRectifier.rectify(request_body)
            stage_label = "thinking_only"
            next_stage = 1
        else:
            # Stage 2 only applies to Antigravity.
            rectified_body, modified = ThinkingRectifier.rectify_signature_sensitive_blocks(
                request_body
            )
            stage_label = "thinking_and_tools"
            next_stage = 2

        if modified:
            request_body_ref["body"] = rectified_body
            request_body_ref["_rectified"] = True
            request_body_ref["_rectified_this_turn"] = True
            request_body_ref["_rectify_stage"] = next_stage

            if provider_type_norm == ProviderType.ANTIGRAVITY:
                try:
                    from src.core.metrics import antigravity_degradation_total

                    antigravity_degradation_total.labels(
                        stage=stage_label,
                        model=str(model_name or "unknown"),
                    ).inc()
                except Exception:
                    pass

            logger.info(
                "  [{}] 请求已整流(stage={})，在当前候选上重试",
                request_id,
                next_stage,
            )
            self._mark_thinking_error_failed(
                candidate_record_id,
                converted_error,
                elapsed_ms,
                captured_key_concurrent,
                {
                    **serializable_extra_data,
                    "rectified": True,
                    "rectify_stage": next_stage,
                    "rectify_stage_label": stage_label,
                },
            )
            return "continue"

        logger.warning("  [{}] Thinking 错误：无可整流内容", request_id)
        self._mark_thinking_error_failed(
            candidate_record_id,
            converted_error,
            elapsed_ms,
            captured_key_concurrent,
            serializable_extra_data,
        )
        raise converted_error

    async def _handle_candidate_error(
        self,
        *,
        exec_err: Any,
        candidate: Any,
        candidate_record_id: str,
        retry_index: int,
        max_retries_for_candidate: int,
        affinity_key: str,
        api_format: str,
        global_model_id: str,
        request_id: str | None,
        attempt: int,
        max_attempts: int,
        error_classifier: Any,
        request_body_ref: dict[str, Any] | None = None,
    ) -> str:
        """
        Handle an execution error for a candidate.

        Returns:
        - "continue": retry current candidate
        - "break": move to next candidate
        - "raise": raise the underlying exception
        """
        from src.core.api_format.conversion.exceptions import FormatConversionError
        from src.services.proxy_node.resolver import resolve_effective_proxy, resolve_proxy_info
        from src.services.request.executor import ExecutionError

        # 提前解析代理信息，写入候选记录的 extra_data（用于链路追踪展示）
        _eff_proxy = resolve_effective_proxy(
            getattr(candidate.provider, "proxy", None),
            getattr(candidate.key, "proxy", None),
        )
        _proxy_info = resolve_proxy_info(_eff_proxy)
        _proxy_extra: dict[str, Any] | None = {"proxy": _proxy_info} if _proxy_info else None

        if not isinstance(exec_err, ExecutionError):
            RequestCandidateService.mark_candidate_failed(
                db=self.db,
                candidate_id=candidate_record_id,
                error_type=type(exec_err).__name__,
                error_message=str(exec_err),
                extra_data=_proxy_extra,
            )
            return "raise"

        provider = candidate.provider
        endpoint = candidate.endpoint
        key = candidate.key

        context = exec_err.context
        captured_key_concurrent = context.concurrent_requests
        elapsed_ms = context.elapsed_ms
        cause = exec_err.cause

        has_retry_left = retry_index < (max_retries_for_candidate - 1)

        if isinstance(cause, ConcurrencyLimitError):
            rpm_current = context.rpm_current
            if rpm_current is None:
                rpm_current = captured_key_concurrent

            rpm_limit = context.rpm_limit
            rpm_available_for_new = context.rpm_available_for_new
            reservation_ratio = context.reservation_ratio
            reservation_phase = context.reservation_phase or "unknown"
            reservation_confidence = context.reservation_confidence
            reservation_load_factor = context.reservation_load_factor

            reason_code = "unknown"
            if rpm_limit is not None and rpm_current is not None:
                if context.is_cached_user:
                    if rpm_current >= rpm_limit:
                        reason_code = "total_limit"
                else:
                    if rpm_available_for_new is not None and rpm_current >= rpm_available_for_new:
                        reason_code = (
                            "reserved_for_cached" if rpm_current < rpm_limit else "total_limit"
                        )
                    elif rpm_current >= rpm_limit:
                        reason_code = "total_limit"

            reason_text = "并发限制"
            if reason_code == "reserved_for_cached":
                reason_text = "并发限制: 新用户配额已满（预留给缓存用户）"
            elif reason_code == "total_limit":
                reason_text = "并发限制: 总配额已满"

            parts: list[str] = []
            if rpm_current is not None:
                parts.append(f"current={rpm_current}")
            if rpm_limit is not None:
                parts.append(f"limit={rpm_limit}")
            if rpm_available_for_new is not None and not context.is_cached_user:
                parts.append(f"new={rpm_available_for_new}")
            if reservation_ratio is not None:
                parts.append(f"reserve={reservation_ratio:.0%}")
            if reservation_phase:
                parts.append(f"phase={reservation_phase}")

            skip_reason = reason_text
            if parts:
                skip_reason = f"{reason_text} ({', '.join(parts)})"

            logger.warning(
                "  [{}] 并发限制 (attempt={}/{}): provider={}, key={}, cached={}, reason={}, {}",
                request_id,
                attempt,
                max_attempts,
                provider.name,
                str(key.id)[:8],
                bool(context.is_cached_user),
                reason_code,
                ", ".join(parts) if parts else "N/A",
            )

            extra_data: dict[str, Any] = {
                "concurrency_denied": True,
                "concurrency_reason": reason_code,
                "rpm_current": rpm_current,
                "rpm_limit": rpm_limit,
                "rpm_available_for_new": rpm_available_for_new,
                "reservation_ratio": reservation_ratio,
                "reservation_phase": reservation_phase,
                "reservation_confidence": reservation_confidence,
                "reservation_load_factor": reservation_load_factor,
                "attempt": attempt,
                "max_attempts": max_attempts,
            }
            extra_data = {k: v for k, v in extra_data.items() if v is not None}
            if _proxy_extra:
                extra_data = {**_proxy_extra, **extra_data}

            try:
                from src.core.metrics import scheduler_concurrency_denied_total

                scheduler_concurrency_denied_total.labels(
                    is_cached_user=str(bool(context.is_cached_user)).lower(),
                    reason=reason_code,
                    reservation_phase=str(reservation_phase or "unknown"),
                ).inc()
            except Exception:
                pass

            RequestCandidateService.mark_candidate_skipped(
                db=self.db,
                candidate_id=candidate_record_id,
                skip_reason=skip_reason,
                status_code=429,
                concurrent_requests=rpm_current,
                extra_data=extra_data,
            )
            return "break"

        if isinstance(cause, ProxyNodeUnavailableError):
            # ProxyNode 不可用属于"配置明确指定但不可达/不可用"的情况，
            # 在当前候选上重试通常没有意义，直接切换到下一个候选更合理。
            node_id = cause.details.get("proxy_node_id") if cause.details else None
            logger.warning(
                "  [{}] 代理节点不可用 (node_id={})，切换候选: {}",
                request_id,
                node_id or "unknown",
                str(cause),
            )
            RequestCandidateService.mark_candidate_failed(
                db=self.db,
                candidate_id=candidate_record_id,
                error_type=type(cause).__name__,
                error_message=extract_error_message(cause),
                latency_ms=elapsed_ms,
                concurrent_requests=captured_key_concurrent,
                extra_data=_proxy_extra,
            )
            return "break"

        if isinstance(cause, EmbeddedErrorException):
            error_message = cause.error_message or ""
            embedded_status = cause.error_code or 200
            if error_classifier.is_client_error(error_message):
                logger.warning(
                    "  [{}] 嵌入式客户端错误，停止重试: {}",
                    request_id,
                    error_message[:200],
                )
                client_error = UpstreamClientException(
                    message=error_message or "请求无效",
                    provider_name=str(provider.name),
                    status_code=embedded_status,
                    upstream_error=error_message,
                )
                RequestCandidateService.mark_candidate_failed(
                    db=self.db,
                    candidate_id=candidate_record_id,
                    error_type="UpstreamClientException",
                    error_message=error_message,
                    status_code=embedded_status,
                    latency_ms=elapsed_ms,
                    concurrent_requests=captured_key_concurrent,
                    extra_data=_proxy_extra,
                )
                client_error.request_metadata = {
                    "provider": provider.name,
                    "provider_id": str(provider.id),
                    "provider_endpoint_id": str(endpoint.id),
                    "provider_api_key_id": str(key.id),
                    "api_format": str(api_format),
                }
                raise client_error

            logger.warning(
                "  [{}] 嵌入式服务端错误，尝试重试: {}",
                request_id,
                error_message[:200],
            )
            RequestCandidateService.mark_candidate_failed(
                db=self.db,
                candidate_id=candidate_record_id,
                error_type="EmbeddedErrorException",
                error_message=error_message,
                status_code=embedded_status,
                latency_ms=elapsed_ms,
                concurrent_requests=captured_key_concurrent,
                extra_data=_proxy_extra,
            )
            return "continue" if has_retry_left else "break"

        if isinstance(cause, httpx.HTTPStatusError):
            status_code = cause.response.status_code
            extra_data = await error_classifier.handle_http_error(
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

            converted_error = extra_data.get("converted_error")
            serializable_extra_data = {
                k: v for k, v in extra_data.items() if k != "converted_error"
            }
            if _proxy_info:
                serializable_extra_data["proxy"] = _proxy_info

            if isinstance(converted_error, ThinkingSignatureException):
                action = self._handle_thinking_signature_error(
                    converted_error=converted_error,
                    provider_type=str(getattr(provider, "provider_type", "") or "").lower(),
                    model_name=str(global_model_id or ""),
                    request_id=request_id,
                    candidate_record_id=candidate_record_id,
                    elapsed_ms=elapsed_ms,
                    captured_key_concurrent=captured_key_concurrent,
                    serializable_extra_data=serializable_extra_data,
                    request_body_ref=request_body_ref,
                )
                if action == "continue":
                    return "continue"

            if isinstance(converted_error, UpstreamClientException):
                logger.warning(
                    "  [{}] 客户端请求错误，停止重试: {}",
                    request_id,
                    str(converted_error.message),
                )
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
                converted_error.request_metadata = {
                    "provider": provider.name,
                    "provider_id": str(provider.id),
                    "provider_endpoint_id": str(endpoint.id),
                    "provider_api_key_id": str(key.id),
                    "api_format": str(api_format),
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
            if str(getattr(provider, "provider_type", "") or "").strip().lower() == "claude_code":
                from src.services.provider.adapters.claude_code.retry_policy import (
                    should_retry_same_candidate,
                )

                same_candidate_retry = has_retry_left and should_retry_same_candidate(
                    status_code,
                    serializable_extra_data.get("error_response"),
                )
                if has_retry_left and not same_candidate_retry:
                    logger.info(
                        "  [{}] Claude Code 状态码策略: status={}，切换到下一个候选",
                        request_id,
                        status_code,
                    )
                return "continue" if same_candidate_retry else "break"

            return "continue" if has_retry_left else "break"

        if isinstance(cause, error_classifier.RETRIABLE_ERRORS):
            await error_classifier.handle_retriable_error(
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
                extra_data=_proxy_extra,
            )
            return "continue" if has_retry_left else "break"

        if isinstance(cause, FormatConversionError):
            logger.warning("  [{}] 格式转换失败，切换候选: {}", request_id, str(cause))
            RequestCandidateService.mark_candidate_failed(
                db=self.db,
                candidate_id=candidate_record_id,
                error_type="FormatConversionError",
                error_message=str(cause),
                latency_ms=elapsed_ms,
                concurrent_requests=captured_key_concurrent,
                extra_data=_proxy_extra,
            )
            return "break"

        RequestCandidateService.mark_candidate_failed(
            db=self.db,
            candidate_id=candidate_record_id,
            error_type=type(cause).__name__,
            error_message=extract_error_message(cause),
            latency_ms=elapsed_ms,
            concurrent_requests=captured_key_concurrent,
            extra_data=_proxy_extra,
        )
        return "raise"

    async def submit_with_failover(
        self,
        *,
        api_format: str,
        model_name: str,
        affinity_key: str,
        user_api_key: ApiKey,
        request_id: str | None,
        task_type: str,
        submit_func: Any,
        extract_external_task_id: Any,
        supported_auth_types: set[str] | None = None,
        allow_format_conversion: bool = False,
        capability_requirements: dict[str, bool] | None = None,
        max_candidates: int | None = None,
    ) -> Any:
        """
        Unified ASYNC submit entrypoint (Phase 3.2).

        Behavior notes:
        - sequentially try candidates (no per-candidate retries at submit stage)
        - record RequestCandidate audit rows
        - stop on "client error" (raise UpstreamClientRequestError)
        - if all failed, raise AllCandidatesFailedError
        """
        from datetime import datetime, timezone

        from sqlalchemy import update

        from src.services.billing.rule_service import BillingRuleLookupResult, BillingRuleService
        from src.services.candidate.submit import (
            AllCandidatesFailedError,
            SubmitOutcome,
            UpstreamClientRequestError,
        )

        def _sanitize(message: str, max_length: int = 200) -> str:
            if not message:
                return "request_failed"
            return _SENSITIVE_PATTERN.sub("[REDACTED]", message)[:max_length]

        def _should_stop_on_http_error(
            *, status_code: int, error_text: str, classifier: ErrorClassifier
        ) -> bool:
            # Keep rules aligned with previous behavior:
            # - 401/403/429 should not hard-stop the traversal at submit stage
            if status_code in (401, 403, 429):
                return False
            if 400 <= status_code < 500:
                return classifier.is_client_error(error_text)
            return False

        # IMPORTANT:
        # This method awaits upstream HTTP calls. If we have an open DB transaction before awaiting,
        # the connection can be held for a long time (pool exhaustion under concurrency).
        #
        # Also note SQLAlchemy's default expire_on_commit=True would expire ORM objects and may
        # trigger unexpected lazy DB loads after we commit (potentially during the await).
        # We disable it temporarily to keep candidate/provider/key objects in-memory.
        original_expire_on_commit = getattr(self.db, "expire_on_commit", True)
        self.db.expire_on_commit = False

        try:
            priority_mode = SystemConfigService.get_config(
                self.db,
                "provider_priority_mode",
                "provider",
            )
            scheduling_mode = SystemConfigService.get_config(
                self.db,
                "scheduling_mode",
                "cache_affinity",
            )
            cache_scheduler = await get_cache_aware_scheduler(
                self.redis,
                priority_mode=priority_mode,
                scheduling_mode=scheduling_mode,
            )
            resolver = CandidateResolver(db=self.db, cache_scheduler=cache_scheduler)
            error_classifier = ErrorClassifier(db=self.db, cache_scheduler=cache_scheduler)

            candidates, _global_model_id = await resolver.fetch_candidates(
                api_format=api_format,
                model_name=model_name,
                affinity_key=affinity_key,
                user_api_key=user_api_key,
                request_id=request_id,
                is_stream=False,
                capability_requirements=capability_requirements,
            )

            if not candidates:
                raise AllCandidatesFailedError(
                    reason="no_candidates",
                    candidate_keys=[],
                    last_status_code=None,
                )

            if max_candidates is not None and max_candidates > 0:
                candidates = candidates[:max_candidates]

            # Pre-create RequestCandidate records (no retry expand for async submit stage)
            record_map: dict[tuple[int, int], str] = {}
            if request_id:
                try:
                    record_map = resolver.create_candidate_records(
                        all_candidates=candidates,
                        request_id=request_id,
                        user_id=str(user_api_key.user_id),
                        user_api_key=user_api_key,
                        required_capabilities=capability_requirements,
                        expand_retries=False,
                    )
                except Exception as exc:
                    logger.warning(
                        "[TaskService] Failed to create candidate records: {}",
                        _sanitize(str(exc)),
                    )
                    record_map = {}

            candidate_keys: list[dict[str, Any]] = []
            eligible_count = 0
            last_status_code: int | None = None

            for idx, cand in enumerate(candidates):
                now = datetime.now(timezone.utc)
                auth_type = getattr(cand.key, "auth_type", "api_key") or "api_key"

                candidate_info: dict[str, Any] = {
                    "index": idx,
                    "provider_id": cand.provider.id,
                    "provider_name": cand.provider.name,
                    "endpoint_id": cand.endpoint.id,
                    "key_id": cand.key.id,
                    "key_name": getattr(cand.key, "name", None),
                    "auth_type": auth_type,
                    "priority": getattr(cand.key, "priority", 0) or 0,
                    "is_cached": bool(getattr(cand, "is_cached", False)),
                }
                candidate_keys.append(candidate_info)

                record_id = record_map.get((idx, 0))

                # Scheduler marked skip
                if getattr(cand, "is_skipped", False):
                    skip_reason = getattr(cand, "skip_reason", None) or "skipped"
                    candidate_info.update({"skipped": True, "skip_reason": skip_reason})
                    if record_id:
                        # record is usually already skipped, but keep it consistent
                        self.db.execute(
                            update(RequestCandidate)
                            .where(RequestCandidate.id == record_id)
                            .values(status="skipped", skip_reason=skip_reason)
                        )
                    continue

                # Format conversion checks
                needs_conversion = bool(getattr(cand, "needs_conversion", False))
                if needs_conversion:
                    # 1. handler-level switch
                    if not allow_format_conversion:
                        skip_reason = "format_conversion_not_supported"
                        candidate_info.update({"skipped": True, "skip_reason": skip_reason})
                        if record_id:
                            self.db.execute(
                                update(RequestCandidate)
                                .where(RequestCandidate.id == record_id)
                                .values(status="skipped", skip_reason=skip_reason)
                            )
                        continue

                    # 2. global switch (from database config)
                    if not SystemConfigService.is_format_conversion_enabled(self.db):
                        skip_reason = "format_conversion_disabled"
                        candidate_info.update(
                            {
                                "skipped": True,
                                "skip_reason": skip_reason,
                                "format_conversion_enabled": False,
                            }
                        )
                        if record_id:
                            self.db.execute(
                                update(RequestCandidate)
                                .where(RequestCandidate.id == record_id)
                                .values(status="skipped", skip_reason=skip_reason)
                            )
                        continue

                # auth_type filter
                if supported_auth_types is not None and auth_type not in supported_auth_types:
                    skip_reason = f"unsupported_auth_type:{auth_type}"
                    candidate_info.update({"skipped": True, "skip_reason": skip_reason})
                    if record_id:
                        self.db.execute(
                            update(RequestCandidate)
                            .where(RequestCandidate.id == record_id)
                            .values(status="skipped", skip_reason=skip_reason)
                        )
                    continue

                # billing rule filter
                rule_lookup: BillingRuleLookupResult | None = None
                has_billing_rule = True
                if config.billing_require_rule:
                    rule_lookup = BillingRuleService.find_rule(
                        self.db,
                        provider_id=cand.provider.id,
                        model_name=model_name,
                        task_type=task_type,
                    )
                    has_billing_rule = rule_lookup is not None
                    if not has_billing_rule:
                        skip_reason = "billing_rule_missing"
                        candidate_info.update(
                            {"has_billing_rule": False, "skipped": True, "skip_reason": skip_reason}
                        )
                        if record_id:
                            self.db.execute(
                                update(RequestCandidate)
                                .where(RequestCandidate.id == record_id)
                                .values(status="skipped", skip_reason=skip_reason)
                            )
                        continue
                candidate_info["has_billing_rule"] = has_billing_rule

                eligible_count += 1

                # Mark pending
                if record_id:
                    self.db.execute(
                        update(RequestCandidate)
                        .where(RequestCandidate.id == record_id)
                        .values(status="pending", started_at=now)
                    )

                # Flush/commit BEFORE awaiting upstream submit to avoid holding DB connections
                if self.db.in_transaction():
                    try:
                        self.db.commit()
                    except Exception:
                        self.db.rollback()
                        raise

                # Attempt submit (upstream HTTP)
                try:
                    response: httpx.Response = await submit_func(cand)
                except Exception as exc:
                    finished_at = datetime.now(timezone.utc)
                    error_msg = _sanitize(str(exc))
                    candidate_info.update(
                        {
                            "attempt_status": "exception",
                            "error_type": type(exc).__name__,
                            "error_message": error_msg,
                        }
                    )
                    if record_id:
                        self.db.execute(
                            update(RequestCandidate)
                            .where(RequestCandidate.id == record_id)
                            .values(
                                status="failed",
                                error_type=type(exc).__name__,
                                error_message=error_msg,
                                finished_at=finished_at,
                            )
                        )
                    continue

                last_status_code = int(getattr(response, "status_code", 0) or 0)

                if response.status_code >= 400:
                    finished_at = datetime.now(timezone.utc)
                    try:
                        error_text = response.text or ""
                    except Exception:
                        error_text = ""
                    error_msg = _sanitize(error_text)
                    candidate_info.update(
                        {
                            "attempt_status": "http_error",
                            "status_code": response.status_code,
                            "error_message": error_msg,
                        }
                    )
                    if record_id:
                        self.db.execute(
                            update(RequestCandidate)
                            .where(RequestCandidate.id == record_id)
                            .values(
                                status="failed",
                                status_code=response.status_code,
                                error_type="http_error",
                                error_message=error_msg,
                                finished_at=finished_at,
                            )
                        )

                    if _should_stop_on_http_error(
                        status_code=response.status_code,
                        error_text=error_text,
                        classifier=error_classifier,
                    ):
                        try:
                            self.db.commit()
                        except Exception:
                            self.db.rollback()
                        raise UpstreamClientRequestError(
                            response=response,
                            candidate_keys=candidate_keys,
                        )
                    continue

                # Parse JSON
                payload: dict[str, Any] | None = None
                try:
                    data = response.json()
                    if isinstance(data, dict):
                        payload = data
                except Exception as exc:
                    finished_at = datetime.now(timezone.utc)
                    error_msg = _sanitize(str(exc))
                    candidate_info.update(
                        {
                            "attempt_status": "invalid_json",
                            "error_type": type(exc).__name__,
                            "error_message": error_msg,
                        }
                    )
                    if record_id:
                        self.db.execute(
                            update(RequestCandidate)
                            .where(RequestCandidate.id == record_id)
                            .values(
                                status="failed",
                                status_code=response.status_code,
                                error_type="invalid_json",
                                error_message=error_msg,
                                finished_at=finished_at,
                            )
                        )
                    continue

                external_task_id = extract_external_task_id(payload or {})
                if not external_task_id:
                    finished_at = datetime.now(timezone.utc)
                    candidate_info.update(
                        {
                            "attempt_status": "empty_task_id",
                            "error_message": "Upstream returned empty task id",
                        }
                    )
                    if record_id:
                        self.db.execute(
                            update(RequestCandidate)
                            .where(RequestCandidate.id == record_id)
                            .values(
                                status="failed",
                                status_code=response.status_code,
                                error_type="empty_task_id",
                                error_message="Upstream returned empty task id",
                                finished_at=finished_at,
                            )
                        )
                    continue

                # Success
                finished_at = datetime.now(timezone.utc)
                candidate_info.update({"attempt_status": "success", "selected": True})
                if record_id:
                    self.db.execute(
                        update(RequestCandidate)
                        .where(RequestCandidate.id == record_id)
                        .values(
                            status="success",
                            status_code=response.status_code,
                            finished_at=finished_at,
                        )
                    )
                try:
                    self.db.commit()
                except Exception:
                    self.db.rollback()

                return SubmitOutcome(
                    candidate=cand,
                    candidate_keys=candidate_keys,
                    external_task_id=str(external_task_id),
                    rule_lookup=rule_lookup,
                    upstream_payload=payload,
                    upstream_headers=dict(response.headers),
                    upstream_status_code=response.status_code,
                )

            # Persist candidate records before raising
            try:
                self.db.commit()
            except Exception:
                self.db.rollback()

            if eligible_count == 0:
                reason = "no_eligible_candidates"
                if config.billing_require_rule:
                    reason = "no_candidate_with_billing_rule"
                raise AllCandidatesFailedError(
                    reason=reason,
                    candidate_keys=candidate_keys,
                    last_status_code=last_status_code,
                )

            raise AllCandidatesFailedError(
                reason="all_candidates_failed",
                candidate_keys=candidate_keys,
                last_status_code=last_status_code,
            )
        finally:
            # Restore Session behavior for the rest of the request lifecycle.
            self.db.expire_on_commit = original_expire_on_commit

    # ====================
    # Phase 3.1: Async task helpers (poll/finalize)
    # ====================

    def _extract_short_id(self, task_id: str) -> str:
        # Keep the parsing rule consistent with handlers:
        # - models/{model}/operations/{short_id}
        # - operations/{short_id}
        # - {short_id}
        return task_id.rsplit("/", 1)[-1] if "/" in task_id else task_id

    def _get_video_task_for_user(self, task_id: str, *, user_id: str) -> Any:
        """
        Resolve a video task by:
        - internal UUID (VideoTask.id)
        - external operation id (VideoTask.short_id)
        """
        task = (
            self.db.query(VideoTask)
            .filter(VideoTask.id == task_id, VideoTask.user_id == user_id)
            .first()
        )
        if task:
            return task

        short_id = self._extract_short_id(task_id)
        task = (
            self.db.query(VideoTask)
            .filter(VideoTask.short_id == short_id, VideoTask.user_id == user_id)
            .first()
        )
        if not task:
            raise TaskNotFoundError(task_id)
        return task

    async def poll(self, task_id: str, *, user_id: str) -> TaskStatusResult:
        """Read task status from DB (does not trigger polling)."""
        task = self._get_video_task_for_user(task_id, user_id=user_id)

        result_url = None
        if getattr(task, "status", None) == "completed":
            result_url = getattr(task, "video_url", None)

        error_message = None
        if getattr(task, "status", None) == "failed":
            error_message = getattr(task, "error_message", None) or getattr(
                task, "error_code", None
            )

        return TaskStatusResult(
            task_id=str(getattr(task, "id", task_id)),
            status=str(getattr(task, "status", "unknown")),
            progress_percent=int(getattr(task, "progress_percent", 0) or 0),
            result_url=result_url,
            error_message=str(error_message) if error_message else None,
            provider_id=(
                str(getattr(task, "provider_id", None))
                if getattr(task, "provider_id", None)
                else None
            ),
            provider_name=(
                str(getattr(task, "provider_name", None))
                if getattr(task, "provider_name", None)
                else None
            ),
            endpoint_id=(
                str(getattr(task, "endpoint_id", None))
                if getattr(task, "endpoint_id", None)
                else None
            ),
            key_id=str(getattr(task, "key_id", None)) if getattr(task, "key_id", None) else None,
        )

    async def poll_now(self, task_id: str, *, user_id: str) -> TaskStatusResult:
        """
        Trigger a single polling attempt (best-effort), then return latest DB status.

        Note: this uses the poller adapter's single-task method and may hold a DB
        connection during the upstream HTTP request; keep usage low.
        """
        from src.services.task.impl.video_poller import VideoTaskPollerAdapter

        task = self._get_video_task_for_user(task_id, user_id=user_id)
        adapter = VideoTaskPollerAdapter()
        await adapter.poll_single_task(self.db, task, redis_client=self.redis)
        self.db.commit()
        return await self.poll(task_id, user_id=user_id)

    async def cancel(
        self,
        task_id: str,
        *,
        user_id: str,
        original_headers: dict[str, str] | None = None,
    ) -> Any:
        """
        Cancel a video task (best-effort) and void its Usage (no charge).

        Returns:
        - None on success
        - upstream httpx.Response when upstream returns an error (status >= 400)
        """
        from datetime import datetime, timezone

        from fastapi import HTTPException

        from src.clients.http_client import HTTPClientPool
        from src.core.api_format import (
            build_upstream_headers_for_endpoint,
            get_extra_headers_from_endpoint,
            make_signature_key,
        )
        from src.core.api_format.conversion.internal_video import VideoStatus
        from src.core.crypto import crypto_service
        from src.services.provider.auth import get_provider_auth
        from src.services.provider.transport import build_provider_url

        try:
            task = self._get_video_task_for_user(task_id, user_id=user_id)
        except TaskNotFoundError:
            raise HTTPException(status_code=404, detail="Video task not found")

        external_task_id = getattr(task, "external_task_id", None)
        if not external_task_id:
            raise HTTPException(status_code=500, detail="Task missing external_task_id")

        endpoint = (
            self.db.query(ProviderEndpoint).filter(ProviderEndpoint.id == task.endpoint_id).first()
        )
        key = self.db.query(ProviderAPIKey).filter(ProviderAPIKey.id == task.key_id).first()
        if not endpoint or not key:
            raise HTTPException(status_code=500, detail="Provider endpoint or key not found")
        if not getattr(key, "api_key", None):
            raise HTTPException(status_code=500, detail="Provider key not configured")

        upstream_key = crypto_service.decrypt(key.api_key)
        extra_headers = get_extra_headers_from_endpoint(endpoint)

        raw_family = str(getattr(endpoint, "api_family", "") or "").strip().lower()
        raw_kind = str(getattr(endpoint, "endpoint_kind", "") or "").strip().lower()
        provider_format = (
            make_signature_key(raw_family, raw_kind)
            if raw_family and raw_kind
            else str(
                getattr(endpoint, "api_format", "")
                or getattr(task, "provider_api_format", "")
                or ""
            )
        )
        provider_format_norm = provider_format.strip().lower()

        headers = build_upstream_headers_for_endpoint(
            original_headers or {},
            provider_format,
            upstream_key,
            endpoint_headers=extra_headers,
            header_rules=getattr(endpoint, "header_rules", None),
        )

        client = await HTTPClientPool.get_default_client_async()

        if provider_format_norm.startswith("openai:"):
            upstream_url = build_provider_url(endpoint, is_stream=False, key=key)
            upstream_url = f"{upstream_url.rstrip('/')}/{str(external_task_id).lstrip('/')}"
            response = await client.delete(upstream_url, headers=headers)
            if response.status_code >= 400:
                return response

        elif provider_format_norm.startswith("gemini:"):
            # Gemini cancel endpoint supports both:
            # - operations/{id}:cancel
            # - models/{model}/operations/{id}:cancel
            operation_name = str(external_task_id)
            if not (
                operation_name.startswith("operations/") or operation_name.startswith("models/")
            ):
                operation_name = f"operations/{operation_name}"

            base = (
                getattr(endpoint, "base_url", None) or "https://generativelanguage.googleapis.com"
            ).rstrip("/")
            if base.endswith("/v1beta"):
                base = base[: -len("/v1beta")]
            upstream_url = f"{base}/v1beta/{operation_name}:cancel"

            auth_info = await get_provider_auth(endpoint, key)
            if auth_info:
                headers.pop("x-goog-api-key", None)
                headers[auth_info.auth_header] = auth_info.auth_value

            response = await client.post(upstream_url, headers=headers, json={})
            if response.status_code >= 400:
                return response

        else:
            raise HTTPException(
                status_code=400,
                detail=f"Cancel not supported for provider format: {provider_format}",
            )

        task.status = VideoStatus.CANCELLED.value
        task.updated_at = datetime.now(timezone.utc)

        # Void Usage (no charge)
        try:
            voided = UsageService.finalize_void(
                self.db,
                request_id=task.request_id,
                reason="cancelled_by_user",
            )
            if not voided:
                UsageService.void_settled(
                    self.db,
                    request_id=task.request_id,
                    reason="cancelled_by_user",
                )
        except Exception as exc:
            logger.warning(
                "Failed to void usage for cancelled task={}: {}",
                getattr(task, "id", task_id),
                str(exc),
            )

        self.db.commit()
        return None

    async def _create_fallback_usage_for_video_task(self, task: Any, request_id: str) -> bool:
        """
        Fallback: create a Usage row if it's missing (should be rare).

        This keeps behavior compatible with the old Phase2 finalize logic.
        """
        user_obj = self.db.query(User).filter(User.id == task.user_id).first()
        api_key_obj = (
            self.db.query(ApiKey).filter(ApiKey.id == task.api_key_id).first()
            if getattr(task, "api_key_id", None)
            else None
        )
        provider_obj = (
            self.db.query(Provider).filter(Provider.id == task.provider_id).first()
            if getattr(task, "provider_id", None)
            else None
        )
        provider_name = provider_obj.name if provider_obj else "unknown"

        response_time_ms: int | None = None
        if getattr(task, "submitted_at", None) and getattr(task, "completed_at", None):
            delta = task.completed_at - task.submitted_at
            response_time_ms = int(delta.total_seconds() * 1000)

        request_headers: dict[str, Any] | None = None
        if isinstance(getattr(task, "request_metadata", None), dict):
            task_meta = task.request_metadata
            for header_key in ("request_headers", "headers", "original_headers"):
                raw_headers = task_meta.get(header_key)
                if isinstance(raw_headers, dict):
                    request_headers = dict(raw_headers)
                    break

        try:
            await UsageService.record_usage_with_custom_cost(
                db=self.db,
                user=user_obj,
                api_key=api_key_obj,
                provider=provider_name,
                model=task.model,
                request_type="video",
                total_cost_usd=0.0,
                request_cost_usd=0.0,
                input_tokens=0,
                output_tokens=0,
                cache_creation_input_tokens=0,
                cache_read_input_tokens=0,
                api_format=task.client_api_format,
                endpoint_api_format=task.provider_api_format,
                has_format_conversion=bool(getattr(task, "format_converted", False)),
                is_stream=False,
                response_time_ms=response_time_ms,
                first_byte_time_ms=None,
                status_code=200 if task.status == "completed" else 500,
                error_message=(
                    None
                    if task.status == "completed"
                    else (task.error_message or task.error_code or "video_task_failed")
                ),
                metadata={
                    "fallback_created": True,
                    "video_task_id": task.id,
                },
                request_headers=request_headers,
                request_body=getattr(task, "original_request_body", None),
                provider_request_headers=None,
                response_headers=None,
                client_response_headers=None,
                response_body=None,
                request_id=request_id,
                provider_id=getattr(task, "provider_id", None),
                provider_endpoint_id=getattr(task, "endpoint_id", None),
                provider_api_key_id=getattr(task, "key_id", None),
                status="completed" if task.status == "completed" else "failed",
                target_model=None,
            )
            return True
        except Exception as exc:
            logger.exception(
                "Failed to create fallback usage for video task={}: {}",
                task.id,
                str(exc),
            )
            return False

    async def finalize_video_task(self, task: Any) -> bool:
        """
        Update billing/usage for a completed/failed video task (Phase 3.1 facade).

        Async video billing flow:
        - Submit success: Usage is already settled with cost=0
        - Poll completion: update actual cost (success -> bill, failure -> keep 0)

        Returns True when updated, False when skipped (already finalized).
        """
        from datetime import datetime, timezone

        from src.core.api_format.conversion.internal_video import VideoStatus
        from src.services.billing.dimension_collector_service import DimensionCollectorService
        from src.services.billing.formula_engine import BillingIncompleteError, FormulaEngine
        from src.services.billing.rule_service import BillingRuleService

        request_id = getattr(task, "request_id", None) or getattr(task, "id", None)
        if not request_id:
            return False

        existing = self.db.query(Usage).filter(Usage.request_id == request_id).first()
        if not existing:
            logger.warning(
                "Usage not found for video task, creating fallback: task_id={} request_id={}",
                getattr(task, "id", None),
                request_id,
            )
            return await self._create_fallback_usage_for_video_task(task, request_id)

        metadata = existing.request_metadata or {}
        if metadata.get("billing_updated_at"):
            logger.debug(
                "Video task billing already updated: task_id={} request_id={}",
                getattr(task, "id", None),
                request_id,
            )
            return False

        response_time_ms: int | None = None
        if getattr(task, "submitted_at", None) and getattr(task, "completed_at", None):
            delta = task.completed_at - task.submitted_at
            response_time_ms = int(delta.total_seconds() * 1000)

        base_dimensions: dict[str, Any] = {
            "duration_seconds": getattr(task, "duration_seconds", None),
            "resolution": getattr(task, "resolution", None),
            "aspect_ratio": getattr(task, "aspect_ratio", None),
            "size": getattr(task, "size", None) or "",
            "retry_count": getattr(task, "retry_count", 0),
        }

        collector_metadata: dict[str, Any] = {
            "task": {
                "id": getattr(task, "id", None),
                "external_task_id": getattr(task, "external_task_id", None),
                "model": getattr(task, "model", None),
                "duration_seconds": getattr(task, "duration_seconds", None),
                "resolution": getattr(task, "resolution", None),
                "aspect_ratio": getattr(task, "aspect_ratio", None),
                "size": getattr(task, "size", None),
                "retry_count": getattr(task, "retry_count", 0),
                "video_size_bytes": getattr(task, "video_size_bytes", None),
            },
            "result": {
                "video_url": getattr(task, "video_url", None),
                "video_urls": getattr(task, "video_urls", None) or [],
            },
        }

        poll_raw = None
        if isinstance(getattr(task, "request_metadata", None), dict):
            poll_raw = task.request_metadata.get("poll_raw_response")

        dims = DimensionCollectorService(self.db).collect_dimensions(
            api_format=getattr(task, "provider_api_format", None),
            task_type="video",
            request=getattr(task, "original_request_body", None) or {},
            response=poll_raw if isinstance(poll_raw, dict) else None,
            metadata=collector_metadata,
            base_dimensions=base_dimensions,
        )

        # Prefer frozen rule snapshot from submit stage
        rule_snapshot = None
        if isinstance(getattr(task, "request_metadata", None), dict):
            rule_snapshot = task.request_metadata.get("billing_rule_snapshot")

        expression = None
        variables: dict[str, Any] | None = None
        dimension_mappings: dict[str, dict[str, Any]] | None = None
        rule_id = None
        rule_name = None
        rule_scope = None

        if isinstance(rule_snapshot, dict) and rule_snapshot.get("status") == "ok":
            rule_id = rule_snapshot.get("rule_id")
            rule_name = rule_snapshot.get("rule_name")
            rule_scope = rule_snapshot.get("scope")
            expression = rule_snapshot.get("expression")
            variables = rule_snapshot.get("variables") or {}
            dimension_mappings = rule_snapshot.get("dimension_mappings") or {}
        else:
            lookup = BillingRuleService.find_rule(
                self.db,
                provider_id=getattr(task, "provider_id", None),
                model_name=getattr(task, "model", None),
                task_type="video",
            )
            if lookup:
                rule = lookup.rule
                rule_id = getattr(rule, "id", None)
                rule_name = getattr(rule, "name", None)
                rule_scope = getattr(lookup, "scope", None)
                expression = getattr(rule, "expression", None)
                variables = getattr(rule, "variables", None) or {}
                dimension_mappings = getattr(rule, "dimension_mappings", None) or {}

        billing_snapshot: dict[str, Any] = {
            "schema_version": "1.0",
            "rule_id": str(rule_id) if rule_id else None,
            "rule_name": str(rule_name) if rule_name else None,
            "scope": str(rule_scope) if rule_scope else None,
            "expression": str(expression) if expression else None,
            "dimensions_used": dims,
            "missing_required": [],
            "cost": 0.0,
            "status": "no_rule",
            "calculated_at": datetime.now(timezone.utc).isoformat(),
        }

        cost = 0.0
        is_success = str(getattr(task, "status", "")) in {
            VideoStatus.COMPLETED.value,
            "completed",
        }

        if is_success and expression:
            engine = FormulaEngine()
            try:
                result = engine.evaluate(
                    expression=str(expression),
                    variables=variables,
                    dimensions=dims,
                    dimension_mappings=dimension_mappings,
                    strict_mode=config.billing_strict_mode,
                )
                billing_snapshot["status"] = result.status
                billing_snapshot["missing_required"] = result.missing_required
                if result.status == "complete":
                    cost = float(result.cost)
                billing_snapshot["cost"] = cost
            except BillingIncompleteError as exc:
                # strict_mode=true: mark task failed and hide artifacts (avoid free pass)
                task.status = VideoStatus.FAILED.value
                task.error_code = "billing_incomplete"
                task.error_message = f"Missing required dimensions: {exc.missing_required}"
                task.video_url = None
                task.video_urls = None
                billing_snapshot["status"] = "incomplete"
                billing_snapshot["missing_required"] = exc.missing_required
                billing_snapshot["cost"] = 0.0
                cost = 0.0
            except Exception as exc:
                billing_snapshot["status"] = "incomplete"
                billing_snapshot["error"] = str(exc)
                billing_snapshot["cost"] = 0.0
                cost = 0.0

        # Write back to task.request_metadata for audit/recalc
        task_meta = dict(task.request_metadata) if getattr(task, "request_metadata", None) else {}
        task_meta["billing_snapshot"] = billing_snapshot
        task.request_metadata = task_meta

        updated = UsageService.update_settled_billing(
            self.db,
            request_id=request_id,
            total_cost_usd=cost,
            request_cost_usd=cost,
            status="completed" if str(getattr(task, "status", "")) == "completed" else "failed",
            status_code=200 if str(getattr(task, "status", "")) == "completed" else 500,
            error_message=(
                None
                if str(getattr(task, "status", "")) == "completed"
                else (
                    getattr(task, "error_message", None)
                    or getattr(task, "error_code", None)
                    or "video_task_failed"
                )
            ),
            response_time_ms=response_time_ms,
            billing_snapshot=billing_snapshot,
            extra_metadata={
                "dimensions": dims,
                "raw_response_ref": {
                    "video_task_id": getattr(task, "id", None),
                    "field": "video_tasks.request_metadata.poll_raw_response",
                },
            },
        )

        if updated:
            logger.debug(
                "Updated video task billing: task_id={} request_id={} cost={:.6f}",
                getattr(task, "id", None),
                request_id,
                cost,
            )
        else:
            logger.warning(
                "Failed to update video task billing (may already be updated): "
                "task_id={} request_id={}",
                getattr(task, "id", None),
                request_id,
            )

        return bool(updated)

    async def finalize(self, task_id: str) -> bool:
        """Finalize a task by internal id (best-effort)."""
        task = self.db.query(VideoTask).filter(VideoTask.id == task_id).first()
        if not task:
            return False
        return await self.finalize_video_task(task)
