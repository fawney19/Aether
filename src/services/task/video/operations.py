from __future__ import annotations

from typing import Any

from sqlalchemy.orm import Session

from src.models.database import VideoTask
from src.services.task.core.exceptions import TaskNotFoundError
from src.services.task.core.schema import TaskStatusResult
from src.services.task.video.billing import VideoTaskBillingService
from src.services.task.video.cancel import VideoTaskCancelService


class VideoTaskOperationsService:
    """视频任务相关应用服务（轮询/取消/终态结算）。"""

    def __init__(self, db: Session, redis_client: Any | None = None) -> None:
        self.db = db
        self.redis = redis_client
        self._billing_ops = VideoTaskBillingService(db)
        self._cancel_ops = VideoTaskCancelService(db)

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
        from src.services.task.video.poller_adapter import VideoTaskPollerAdapter

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
        from fastapi import HTTPException

        try:
            task = self._get_video_task_for_user(task_id, user_id=user_id)
        except TaskNotFoundError:
            raise HTTPException(status_code=404, detail="Video task not found")
        return await self._cancel_ops.cancel_task(
            task=task,
            task_id=task_id,
            original_headers=original_headers,
        )

    async def finalize_video_task(self, task: Any) -> bool:
        return await self._billing_ops.finalize_video_task(task)

    async def finalize(self, task_id: str) -> bool:
        """Finalize a task by internal id (best-effort)."""
        task = self.db.query(VideoTask).filter(VideoTask.id == task_id).first()
        if not task:
            return False
        return await self.finalize_video_task(task)
