from __future__ import annotations

from collections.abc import Callable
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
from src.services.task.submit.submit_service import AsyncTaskSubmitService
from src.services.task.video.facade import TaskVideoFacadeService
from src.services.task.video.operations import VideoTaskOperationsService


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
        request_body_ref: dict[str, Any] | None = None,
        request_headers: dict[str, Any] | None = None,
        request_body: dict[str, Any] | None = None,
        # ASYNC-only (video submit)
        extract_external_task_id: Any | None = None,
        supported_auth_types: set[str] | None = None,
        allow_format_conversion: bool = False,
        max_candidates: int | None = None,
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
            request_body_ref=request_body_ref,
            request_headers=request_headers,
            request_body=request_body,
            extract_external_task_id=extract_external_task_id,
            supported_auth_types=supported_auth_types,
            allow_format_conversion=allow_format_conversion,
            max_candidates=max_candidates,
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
        request_body_ref: dict[str, Any] | None = None,
        request_headers: dict[str, Any] | None = None,
        request_body: dict[str, Any] | None = None,
        extract_external_task_id: Any | None = None,
        supported_auth_types: set[str] | None = None,
        allow_format_conversion: bool = False,
        max_candidates: int | None = None,
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
            request_body_ref=request_body_ref,
            request_headers=request_headers,
            request_body=request_body,
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
