from __future__ import annotations

from typing import Any

from src.models.database import ApiKey
from src.services.task.submit.submit_service import AsyncTaskSubmitService


class TaskSubmitFacadeService:
    """任务提交门面服务（向后兼容 TaskService.submit_with_failover）。"""

    def __init__(self, submit_ops: AsyncTaskSubmitService) -> None:
        self._submit_ops = submit_ops

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
        return await self._submit_ops.submit_with_failover(
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
