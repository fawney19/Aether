from __future__ import annotations

from typing import Any

from src.services.task.core.schema import TaskStatusResult
from src.services.task.video.operations import VideoTaskOperationsService


class TaskVideoFacadeService:
    """视频任务门面服务（向后兼容 TaskService 的视频公开方法）。"""

    def __init__(self, video_ops: VideoTaskOperationsService) -> None:
        self._video_ops = video_ops

    async def poll(self, task_id: str, *, user_id: str) -> TaskStatusResult:
        return await self._video_ops.poll(task_id, user_id=user_id)

    async def poll_now(self, task_id: str, *, user_id: str) -> TaskStatusResult:
        return await self._video_ops.poll_now(task_id, user_id=user_id)

    async def cancel(
        self,
        task_id: str,
        *,
        user_id: str,
        original_headers: dict[str, str] | None = None,
    ) -> Any:
        return await self._video_ops.cancel(
            task_id,
            user_id=user_id,
            original_headers=original_headers,
        )

    async def finalize_video_task(self, task: Any) -> bool:
        return await self._video_ops.finalize_video_task(task)

    async def finalize(self, task_id: str) -> bool:
        return await self._video_ops.finalize(task_id)
