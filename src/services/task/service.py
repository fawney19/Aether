from __future__ import annotations

from collections.abc import Awaitable, Callable
from types import SimpleNamespace
from typing import Any

from sqlalchemy.orm import Session

from src.models.database import ApiKey
from src.services.candidate.recorder import CandidateRecorder
from src.services.task.core.context import TaskMode
from src.services.task.core.protocol import AttemptKind, AttemptResult
from src.services.task.core.schema import ExecutionResult, TaskStatusResult
from src.services.task.execute.error_handler import TaskErrorOperationsService
from src.services.task.execute.failure import TaskFailureOperationsService
from src.services.task.execute.pool import TaskPoolOperationsService
from src.services.task.execute.sync_execute import SyncTaskExecutionService
from src.services.task.request_state import RequestBodyState
from src.services.task.submit.submit_service import AsyncTaskSubmitService
from src.services.task.video.facade import TaskVideoFacadeService
from src.services.task.video.operations import VideoTaskOperationsService


async def pool_on_error(
    provider: Any,
    key: Any,
    status_code: int,
    cause: Any,
) -> None:
    """Notify the pool manager about an upstream error (health policy)."""
    try:
        from src.services.provider.pool.config import parse_pool_config
        from src.services.provider.pool.health_policy import apply_health_policy

        pool_cfg = parse_pool_config(getattr(provider, "config", None))
        if pool_cfg is None:
            return

        error_text = ""
        resp_headers: dict[str, str] = {}
        if getattr(cause, "response", None) is not None:
            try:
                error_text = (cause.response.text or "")[:4000]
            except Exception:
                pass
            try:
                resp_headers = dict(cause.response.headers)
            except Exception:
                pass
        elif isinstance(getattr(cause, "error_message", None), str):
            error_text = str(getattr(cause, "error_message", "") or "")[:4000]

        await apply_health_policy(
            provider_id=str(provider.id),
            key_id=str(key.id),
            status_code=status_code,
            error_body=error_text,
            response_headers=resp_headers,
            config=pool_cfg,
        )
    except Exception:
        pass


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
        self._candidate_recorder = CandidateRecorder(db)
        # 兼容历史注入点：_execute_facade_ops/_submit_facade_ops
        # 不再依赖独立门面类，默认直接绑定 TaskService 内部实现。
        self._execute_facade_ops = SimpleNamespace(
            execute=self._execute_internal,
            _get_candidate_keys=self._candidate_recorder.get_candidate_keys,
        )

        pool_ops = TaskPoolOperationsService()
        error_ops = TaskErrorOperationsService(db, pool_ops=pool_ops)
        failure_ops = TaskFailureOperationsService()

        self._sync_ops = SyncTaskExecutionService(
            db,
            redis_client,
            recorder=self._candidate_recorder,
            pool_ops=pool_ops,
            error_ops=error_ops,
            failure_ops=failure_ops,
        )
        self._submit_ops = AsyncTaskSubmitService(
            db,
            redis_client,
            apply_pool_reorder=pool_ops.apply_pool_reorder,
            expand_pool_candidates_for_async_submit=pool_ops.expand_pool_candidates_for_async_submit,
        )
        self._video_ops = VideoTaskOperationsService(db, redis_client)
        self._submit_facade_ops = self._submit_ops
        self._video_facade_ops = TaskVideoFacadeService(self._video_ops)

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
        request_body_state: RequestBodyState | None = None,
        request_headers: dict[str, Any] | None = None,
        request_body: dict[str, Any] | None = None,
        # ASYNC-only (video submit)
        extract_external_task_id: Any | None = None,
        supported_auth_types: set[str] | None = None,
        allow_format_conversion: bool = False,
        max_candidates: int | None = None,
        create_pending_usage: bool = True,
    ) -> ExecutionResult:
        """兼容入口：默认绑定到 TaskService 内部执行路由。"""
        return await self._execute_facade_ops.execute(
            task_type=task_type,
            task_mode=task_mode,
            api_format=api_format,
            model_name=model_name,
            user_api_key=user_api_key,
            request_func=request_func,
            request_id=request_id,
            is_stream=is_stream,
            capability_requirements=capability_requirements,
            preferred_key_ids=preferred_key_ids,
            request_body_state=request_body_state,
            request_headers=request_headers,
            request_body=request_body,
            extract_external_task_id=extract_external_task_id,
            supported_auth_types=supported_auth_types,
            allow_format_conversion=allow_format_conversion,
            max_candidates=max_candidates,
            create_pending_usage=create_pending_usage,
        )

    async def _execute_internal(
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
        request_body_state: RequestBodyState | None = None,
        request_headers: dict[str, Any] | None = None,
        request_body: dict[str, Any] | None = None,
        extract_external_task_id: Any | None = None,
        supported_auth_types: set[str] | None = None,
        allow_format_conversion: bool = False,
        max_candidates: int | None = None,
        create_pending_usage: bool = True,
    ) -> ExecutionResult:
        if task_mode == TaskMode.ASYNC:
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
                request_body=request_body,
            )

            candidate_keys = []
            if request_id:
                try:
                    candidate_keys = self._execute_facade_ops._get_candidate_keys(request_id)
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

        return await self._sync_ops.execute_sync_unified(
            api_format=api_format,
            model_name=model_name,
            user_api_key=user_api_key,
            request_func=request_func,
            request_id=request_id,
            is_stream=is_stream,
            capability_requirements=capability_requirements,
            preferred_key_ids=preferred_key_ids,
            request_body_state=request_body_state,
            request_headers=request_headers,
            request_body=request_body,
            create_pending_usage=create_pending_usage,
        )

    async def execute_sync_candidates(
        self,
        *,
        api_format: str,
        model_name: str,
        candidates: list[Any],
        request_func: Callable[..., Any],
        request_id: str | None = None,
        current_user: Any | None = None,
        user_api_key: ApiKey | None = None,
        is_stream: bool = False,
        capability_requirements: dict[str, bool] | None = None,
        request_body_state: RequestBodyState | None = None,
        request_headers: dict[str, Any] | None = None,
        request_body: dict[str, Any] | None = None,
        affinity_key: str | None = None,
        create_pending_usage: bool = False,
        enable_cache_affinity: bool = False,
        is_cancelled: Callable[[], Awaitable[bool]] | None = None,
    ) -> ExecutionResult:
        """Execute a pre-built candidate set through the unified SYNC runtime."""
        from uuid import uuid4

        from src.models.database import User
        from src.services.candidate.failover import FailoverEngine
        from src.services.candidate.policy import RetryPolicy, SkipPolicy
        from src.services.candidate.resolver import CandidateResolver
        from src.services.orchestration.error_classifier import ErrorClassifier
        from src.services.orchestration.request_dispatcher import RequestDispatcher
        from src.services.provider.format import normalize_endpoint_signature
        from src.services.rate_limit.adaptive_rpm import get_adaptive_rpm_manager
        from src.services.rate_limit.concurrency_manager import get_concurrency_manager
        from src.services.request.candidate import RequestCandidateService
        from src.services.request.executor import RequestExecutor
        from src.services.scheduling.aware_scheduler import (
            CacheAwareScheduler,
            get_cache_aware_scheduler,
        )
        from src.services.system.config import SystemConfigService
        from src.services.task.execute.exception_classification import (
            CandidateErrorAction,
            classify_candidate_error_action,
        )
        from src.services.task.execute.state_transition import (
            SyncExecutionState,
            resolve_execution_error_transition,
        )
        from src.services.usage.service import UsageService

        if not request_id:
            request_id = str(uuid4())

        api_format_norm = normalize_endpoint_signature(api_format)

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
            cache_scheduler=cache_scheduler if enable_cache_affinity else None,
        )

        pool_ops = self._sync_ops._pool_ops
        error_ops = self._sync_ops._error_ops
        failure_ops = self._sync_ops._failure_ops

        resolved_user = current_user
        if resolved_user is None and user_api_key is not None:
            try:
                resolved_user = user_api_key.user if hasattr(user_api_key, "user") else None
            except Exception:
                resolved_user = None
            if resolved_user is None and getattr(user_api_key, "user_id", None):
                resolved_user = self.db.query(User).filter(User.id == user_api_key.user_id).first()

        user_id: str | None = None
        if resolved_user is not None and getattr(resolved_user, "id", None):
            user_id = str(resolved_user.id)
        elif user_api_key is not None and getattr(user_api_key, "user_id", None):
            user_id = str(user_api_key.user_id)

        username_snapshot = getattr(resolved_user, "username", None) if resolved_user else None
        api_key_name_snapshot = getattr(user_api_key, "name", None) if user_api_key else None

        resolved_affinity_key = affinity_key
        if not resolved_affinity_key:
            api_key_id = getattr(user_api_key, "id", None) if user_api_key is not None else None
            resolved_affinity_key = str(api_key_id) if api_key_id else f"internal-test:{request_id}"

        if create_pending_usage:
            try:
                UsageService.create_pending_usage(
                    db=self.db,
                    request_id=request_id,
                    user=resolved_user,
                    api_key=user_api_key,
                    model=model_name,
                    is_stream=is_stream,
                    api_format=api_format_norm,
                    request_headers=request_headers,
                    request_body=request_body,
                )
            except Exception as exc:
                from src.core.logger import logger as _logger

                _logger.warning("创建 pending 使用记录失败: {}", str(exc))

        all_candidates = list(candidates)

        # 号池排序涉及大量 Redis 操作，提前释放 DB 连接避免连接池压力
        from src.services.scheduling.utils import release_db_connection_before_await

        release_db_connection_before_await(self.db)

        all_candidates, pool_traces = await pool_ops.apply_pool_reorder(
            all_candidates, request_body=request_body
        )

        candidate_record_map = await candidate_resolver.create_candidate_records_async(
            all_candidates=all_candidates,
            request_id=request_id,
            user_id=user_id,
            user_api_key=user_api_key,
            required_capabilities=capability_requirements,
        )

        max_attempts = candidate_resolver.count_total_attempts(all_candidates)
        execution_state = SyncExecutionState(
            candidate_record_map=candidate_record_map,
            request_body_state=request_body_state,
            last_candidate=all_candidates[-1] if all_candidates else None,
        )

        async def _attempt(candidate: Any) -> AttemptResult:
            execution_state.touch_candidate(candidate)

            candidate_index = int(getattr(candidate, "_utf_candidate_index", -1))
            retry_index = int(getattr(candidate, "_utf_retry_index", 0))
            candidate_record_id = str(getattr(candidate, "_utf_candidate_record_id", "") or "")
            attempt_counter = int(getattr(candidate, "_utf_attempt_count", 0))
            max_attempts_local = int(getattr(candidate, "_utf_max_attempts", max_attempts))

            if not candidate_record_id:
                from src.services.scheduling.schemas import PoolCandidate

                pool_extra = (
                    getattr(candidate.key, "_pool_extra_data", None)
                    if isinstance(getattr(candidate.key, "_pool_extra_data", None), dict)
                    else {}
                )
                extra_data: dict[str, Any] = {
                    "needs_conversion": bool(getattr(candidate, "needs_conversion", False)),
                    "provider_api_format": getattr(candidate, "provider_api_format", None) or None,
                    "mapping_matched_model": getattr(candidate, "mapping_matched_model", None)
                    or None,
                    **pool_extra,
                }
                if isinstance(candidate, PoolCandidate):
                    extra_data["pool_group_id"] = str(candidate.provider.id)
                    extra_data["pool_key_index"] = int(
                        getattr(candidate, "_pool_key_index", 0) or 0
                    )

                candidate_record = RequestCandidateService.create_candidate(
                    db=self.db,
                    request_id=request_id,
                    candidate_index=candidate_index,
                    retry_index=retry_index,
                    user_id=user_id,
                    api_key_id=(getattr(user_api_key, "id", None) if user_api_key else None),
                    username=username_snapshot,
                    api_key_name=api_key_name_snapshot,
                    provider_id=str(candidate.provider.id),
                    endpoint_id=str(candidate.endpoint.id),
                    key_id=str(candidate.key.id),
                    status="available",
                    is_cached=bool(getattr(candidate, "is_cached", False)),
                    extra_data=extra_data,
                )
                self.db.flush()
                candidate_record_id = str(candidate_record.id)
                execution_state.candidate_record_map[(candidate_index, retry_index)] = (
                    candidate_record_id
                )
                setattr(candidate, "_utf_candidate_record_id", candidate_record_id)

            (
                response,
                _provider_name,
                attempt_id,
                _provider_id,
                _endpoint_id,
                _key_id,
                _first_byte_time_ms,
            ) = await request_dispatcher.dispatch(
                candidate=candidate,
                candidate_index=candidate_index,
                retry_index=retry_index,
                candidate_record_id=candidate_record_id,
                user_api_key=user_api_key,
                user_id=user_id,
                request_func=request_func,
                request_id=request_id,
                api_format=api_format_norm,
                model_name=model_name,
                affinity_key=resolved_affinity_key,
                global_model_id=model_name,
                attempt_counter=attempt_counter,
                max_attempts=max_attempts_local,
                is_stream=is_stream,
            )
            _ = (attempt_id, _provider_name, _provider_id, _endpoint_id, _key_id)

            await pool_ops.pool_on_success(candidate, request_body)

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
            execution_state.track_execution_error(exec_err=exec_err, candidate=candidate)
            candidate_record_id = execution_state.resolve_candidate_record_id(
                candidate_index=candidate_index,
                record_id=record_id,
            )

            raw_action = await error_ops.handle_candidate_error(
                exec_err=exec_err,
                candidate=candidate,
                candidate_record_id=candidate_record_id,
                retry_index=retry_index,
                max_retries_for_candidate=max_retries_for_candidate,
                affinity_key=resolved_affinity_key,
                api_format=api_format_norm,
                global_model_id=model_name,
                request_id=request_id,
                attempt=attempt_count,
                max_attempts=int(max_attempts or 0),
                request_body_state=request_body_state,
                error_classifier=error_classifier,
            )
            action = classify_candidate_error_action(raw_action)

            if action == CandidateErrorAction.RAISE_ERROR:
                execution_state.raise_classified_error(
                    fallback_error=exec_err,
                    failure_ops=failure_ops,
                    model_name=model_name,
                    api_format=api_format_norm,
                )

            return resolve_execution_error_transition(
                action=action,
                state=execution_state,
                max_retries_for_candidate=max_retries_for_candidate,
                retry_index=retry_index,
            ).as_failover_tuple()

        engine = FailoverEngine(
            self.db,
            error_classifier=error_classifier,
            recorder=self._candidate_recorder,
        )
        result = await engine.execute(
            candidates=all_candidates,
            attempt_func=_attempt,
            retry_policy=RetryPolicy.for_sync_task(),
            skip_policy=SkipPolicy(),
            request_id=request_id,
            user_id=user_id,
            api_key_id=(
                str(user_api_key.id) if user_api_key and getattr(user_api_key, "id", None) else None
            ),
            username=username_snapshot,
            api_key_name=api_key_name_snapshot,
            candidate_record_map=candidate_record_map,
            max_attempts=max_attempts,
            execution_error_handler=_handle_exec_err,
            is_cancelled=is_cancelled,
        )

        if result.success:
            if pool_traces and result.key_id:
                try:
                    attempted_key_ids: set[str] = set()
                    for ck in result.candidate_keys or []:
                        status = str(getattr(ck, "status", "") or "").strip().lower()
                        if status in {"", "available", "pending", "skipped", "unused"}:
                            continue
                        kid = getattr(ck, "key_id", None)
                        if isinstance(kid, str) and kid:
                            attempted_key_ids.add(kid)
                    if not attempted_key_ids:
                        attempted_key_ids.add(str(result.key_id))

                    for pt in pool_traces:
                        summary = pt.build_summary(
                            result.key_id,
                            attempted_key_ids=attempted_key_ids,
                        )
                        if summary:
                            result.pool_summary = summary
                            break
                except Exception:
                    pass
            return result

        failure_ops.raise_all_failed_exception(
            request_id,
            max_attempts,
            execution_state.last_candidate,
            model_name,
            api_format_norm,
            execution_state.last_error,
        )

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
        request_body: dict[str, Any] | None = None,
    ) -> Any:
        """
        Unified ASYNC submit entrypoint (Phase 3.2).

        兼容入口：默认直接绑定 AsyncTaskSubmitService。
        """
        return await self._submit_facade_ops.submit_with_failover(
            api_format=api_format,
            model_name=model_name,
            affinity_key=affinity_key,
            user_api_key=user_api_key,
            request_id=request_id,
            task_type=task_type,
            submit_func=submit_func,
            extract_external_task_id=extract_external_task_id,
            supported_auth_types=supported_auth_types,
            allow_format_conversion=allow_format_conversion,
            capability_requirements=capability_requirements,
            max_candidates=max_candidates,
            request_body=request_body,
        )

    # ====================
    # Phase 3.1: Async task helpers (poll/finalize)
    # ====================

    async def poll(self, task_id: str, *, user_id: str) -> TaskStatusResult:
        return await self._video_facade_ops.poll(task_id, user_id=user_id)

    async def poll_now(self, task_id: str, *, user_id: str) -> TaskStatusResult:
        return await self._video_facade_ops.poll_now(task_id, user_id=user_id)

    async def cancel(
        self,
        task_id: str,
        *,
        user_id: str,
        original_headers: dict[str, str] | None = None,
    ) -> Any:
        return await self._video_facade_ops.cancel(
            task_id,
            user_id=user_id,
            original_headers=original_headers,
        )

    async def finalize_video_task(self, task: Any) -> bool:
        return await self._video_facade_ops.finalize_video_task(task)

    async def finalize(self, task_id: str) -> bool:
        return await self._video_facade_ops.finalize(task_id)
