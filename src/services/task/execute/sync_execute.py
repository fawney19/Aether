from __future__ import annotations

from collections.abc import Callable
from typing import Any
from uuid import uuid4

from src.core.logger import logger
from src.models.database import ApiKey, User
from src.services.candidate.failover import FailoverEngine
from src.services.candidate.policy import FailoverAction, RetryPolicy, SkipPolicy
from src.services.candidate.resolver import CandidateResolver
from src.services.orchestration.error_classifier import ErrorClassifier
from src.services.orchestration.request_dispatcher import RequestDispatcher
from src.services.provider.format import normalize_endpoint_signature
from src.services.request.candidate import RequestCandidateService
from src.services.scheduling.aware_scheduler import (
    CacheAwareScheduler,
    get_cache_aware_scheduler,
)
from src.services.system.config import SystemConfigService
from src.services.task.core.protocol import AttemptKind, AttemptResult
from src.services.task.core.schema import ExecutionResult
from src.services.task.execute.error_handler import TaskErrorOperationsService
from src.services.task.execute.exception_classification import (
    CandidateErrorAction,
    classify_candidate_error_action,
)
from src.services.task.execute.failure import TaskFailureOperationsService
from src.services.task.execute.pool import TaskPoolOperationsService
from src.services.task.execute.state_transition import (
    SyncExecutionState,
    resolve_execution_error_transition,
)
from src.services.task.request_state import RequestBodyState
from src.services.usage.service import UsageService


class SyncTaskExecutionService:
    """同步任务执行服务（候选遍历 + 错误处理 + 结果聚合）。"""

    def __init__(
        self,
        db: Any,
        redis_client: Any | None,
        *,
        recorder: Any,
        pool_ops: TaskPoolOperationsService,
        error_ops: TaskErrorOperationsService,
        failure_ops: TaskFailureOperationsService,
    ) -> None:
        self.db = db
        self.redis = redis_client
        self._recorder = recorder
        self._pool_ops = pool_ops
        self._error_ops = error_ops
        self._failure_ops = failure_ops

    async def execute_sync_unified(
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
        request_body_state: RequestBodyState | None,
        request_headers: dict[str, Any] | None,
        request_body: dict[str, Any] | None,
        create_pending_usage: bool = True,
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

        # IMPORTANT:
        # This SYNC path awaits upstream HTTP work while the failover engine may commit
        # candidate audit rows between attempts. SQLAlchemy's default
        # expire_on_commit=True would expire provider/endpoint/key ORM objects and can
        # trigger an unexpected lazy DB reload later in error handling (for example when
        # reading candidate.provider.config for failover_rules after a timeout).
        #
        # Keep already-loaded candidate objects resident in memory for the duration of
        # the request, mirroring the async submit path.
        original_expire_on_commit = getattr(self.db, "expire_on_commit", True)
        self.db.expire_on_commit = False

        try:

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
            user: User | None = None
            username_snapshot = None
            api_key_name_snapshot = getattr(user_api_key, "name", None)
            try:
                user = self.db.query(User).filter(User.id == user_api_key.user_id).first()
                username_snapshot = getattr(user, "username", None) if user else None
            except Exception as exc:
                # username 仅用于审计快照，不应阻塞主请求链路。
                logger.warning("查询用户快照失败: {}", str(exc))

            # 默认由 TaskService 创建 pending 使用记录；已预创建的调用方可关闭。
            if create_pending_usage:
                try:
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
                request_body=request_body,
            )

            # 号池排序涉及大量 Redis 操作，提前释放 DB 连接避免连接池压力
            from src.services.scheduling.utils import release_db_connection_before_await

            release_db_connection_before_await(self.db)

            # Account Pool: reorder candidates for claude_code providers.
            all_candidates, pool_traces = await self._pool_ops.apply_pool_reorder(
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
            # Keep behavior consistent with previous behavior: last_candidate is updated even if skipped.
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

                # Safety net: if record_id missing, create an "available" record on-demand.
                if not candidate_record_id:
                    from src.services.scheduling.schemas import PoolCandidate

                    pool_extra = (
                        getattr(candidate.key, "_pool_extra_data", None)
                        if isinstance(getattr(candidate.key, "_pool_extra_data", None), dict)
                        else {}
                    )
                    extra_data: dict[str, Any] = {
                        "needs_conversion": bool(getattr(candidate, "needs_conversion", False)),
                        "provider_api_format": getattr(candidate, "provider_api_format", None)
                        or None,
                        "mapping_matched_model": getattr(candidate, "mapping_matched_model", None)
                        or None,
                        **pool_extra,
                    }
                    if isinstance(candidate, PoolCandidate):
                        extra_data["pool_group_id"] = str(candidate.provider.id)
                        extra_data["pool_key_index"] = int(
                            getattr(candidate, "_pool_key_index", 0) or 0
                        )
                    created = RequestCandidateService.create_candidate(
                        db=self.db,
                        request_id=request_id,
                        candidate_index=candidate_index,
                        retry_index=retry_index,
                        user_id=user_id,
                        api_key_id=str(user_api_key.id),
                        username=username_snapshot,
                        api_key_name=api_key_name_snapshot,
                        provider_id=str(candidate.provider.id),
                        endpoint_id=str(candidate.endpoint.id),
                        key_id=str(candidate.key.id),
                        status="available",
                        is_cached=bool(getattr(candidate, "is_cached", False)),
                        extra_data=extra_data,
                    )
                    candidate_record_id = str(created.id)
                    execution_state.candidate_record_map[(candidate_index, retry_index)] = (
                        candidate_record_id
                    )

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
                    affinity_key=affinity_key,
                    global_model_id=global_model_id,
                    attempt_counter=attempt_counter,
                    max_attempts=max_attempts_local,
                    is_stream=is_stream,
                )
                _ = (
                    attempt_id,
                    _provider_name,
                    _provider_id,
                    _endpoint_id,
                    _key_id,
                    _first_byte_time_ms,
                )

                # Account Pool: on success, update sticky binding + LRU.
                await self._pool_ops.pool_on_success(candidate, request_body)

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
            ) -> tuple[FailoverAction, int | None]:
                execution_state.track_execution_error(exec_err=exec_err, candidate=candidate)
                # Fall back to retry 0 record if needed (rectify may extend retries).
                candidate_record_id = execution_state.resolve_candidate_record_id(
                    candidate_index=candidate_index,
                    record_id=record_id,
                )

                raw_action = await self._error_ops.handle_candidate_error(
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
                    request_body_state=request_body_state,
                    error_classifier=error_classifier,
                )
                action = classify_candidate_error_action(raw_action)

                if action == CandidateErrorAction.RAISE_ERROR:
                    execution_state.raise_classified_error(
                        fallback_error=exec_err,
                        failure_ops=self._failure_ops,
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
                username=username_snapshot,
                api_key_name=api_key_name_snapshot,
                candidate_record_map=candidate_record_map,
                max_attempts=max_attempts,
                execution_error_handler=_handle_exec_err,
            )

            if result.success:
                # Build pool scheduling summary from traces collected during reorder.
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

            self._failure_ops.raise_all_failed_exception(
                request_id,
                max_attempts,
                execution_state.last_candidate,
                model_name,
                api_format_norm,
                execution_state.last_error,
            )
        finally:
            self.db.expire_on_commit = original_expire_on_commit
