from __future__ import annotations

from collections.abc import Awaitable, Callable
from typing import Any

from src.models.database import ApiKey
from src.services.task.core.context import TaskMode
from src.services.task.core.protocol import AttemptKind, AttemptResult
from src.services.task.core.schema import ExecutionResult


class TaskExecuteFacadeService:
    """任务执行门面服务（SYNC/ASYNC 路由与结果组装）。"""

    def __init__(self, get_candidate_keys_fn: Callable[[str], Any]) -> None:
        self._get_candidate_keys = get_candidate_keys_fn

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
        submit_with_failover_fn: Callable[..., Awaitable[Any]],
        execute_sync_fn: Callable[..., Awaitable[ExecutionResult]],
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

            outcome = await submit_with_failover_fn(
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
                    candidate_keys = self._get_candidate_keys(request_id)
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
        return await execute_sync_fn(
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
