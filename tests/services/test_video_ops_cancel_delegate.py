from __future__ import annotations

from types import SimpleNamespace
from unittest.mock import AsyncMock, MagicMock

import pytest
from fastapi import HTTPException

from src.services.task.core.exceptions import TaskNotFoundError
from src.services.task.video.operations import VideoTaskOperationsService


@pytest.mark.asyncio
async def test_video_ops_cancel_delegates_to_cancel_ops() -> None:
    svc = VideoTaskOperationsService(MagicMock())
    task = SimpleNamespace(id="task-1")

    svc._get_video_task_for_user = MagicMock(return_value=task)  # type: ignore[attr-defined, method-assign]
    svc._cancel_ops.cancel_task = AsyncMock(return_value={"ok": True})  # type: ignore[attr-defined, method-assign]

    result = await svc.cancel(
        "task-1",
        user_id="user-1",
        original_headers={"x-test": "1"},
    )

    assert result == {"ok": True}
    svc._cancel_ops.cancel_task.assert_awaited_once_with(  # type: ignore[attr-defined]
        task=task,
        task_id="task-1",
        original_headers={"x-test": "1"},
    )


@pytest.mark.asyncio
async def test_video_ops_cancel_maps_not_found_to_http_404() -> None:
    svc = VideoTaskOperationsService(MagicMock())
    svc._get_video_task_for_user = MagicMock(side_effect=TaskNotFoundError("missing"))  # type: ignore[attr-defined, method-assign]

    with pytest.raises(HTTPException) as excinfo:
        await svc.cancel("missing", user_id="user-1")

    assert excinfo.value.status_code == 404
    assert excinfo.value.detail == "Video task not found"
