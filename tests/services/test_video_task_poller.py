from types import SimpleNamespace
from typing import cast
from unittest.mock import AsyncMock, MagicMock

import pytest

from src.core.api_format.conversion.internal_video import InternalVideoPollResult, VideoStatus
from src.models.database import VideoTask
from src.services.task.video.poller_adapter import VideoTaskPollerAdapter


@pytest.mark.asyncio
async def test_poll_task_status_routes_gemini_video_to_gemini(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    adapter = VideoTaskPollerAdapter()

    task = cast(
        VideoTask,
        SimpleNamespace(
            endpoint_id="e1",
            key_id="k1",
            provider_api_format="gemini:video",
            external_task_id="operations/123",
        ),
    )
    endpoint = SimpleNamespace(id="e1", base_url="https://example.com", api_format="gemini:video")
    key = SimpleNamespace(id="k1", api_key="enc")

    monkeypatch.setattr(adapter, "_get_endpoint", lambda _db, _id: endpoint)
    monkeypatch.setattr(adapter, "_get_key", lambda _db, _id: key)
    monkeypatch.setattr(
        "src.services.task.video.poller_adapter.crypto_service.decrypt", lambda _v: "decrypted"
    )

    auth_info = SimpleNamespace(auth_header="authorization", auth_value="Bearer x")
    monkeypatch.setattr(
        "src.services.task.video.poller_adapter.get_provider_auth",
        AsyncMock(return_value=auth_info),
    )

    poll_gemini = AsyncMock(return_value=InternalVideoPollResult(status=VideoStatus.PROCESSING))
    poll_openai = AsyncMock(return_value=InternalVideoPollResult(status=VideoStatus.PROCESSING))
    monkeypatch.setattr(adapter, "_poll_gemini", poll_gemini)
    monkeypatch.setattr(adapter, "_poll_openai", poll_openai)

    result = await adapter._poll_task_status(MagicMock(), task)
    assert result.status == VideoStatus.PROCESSING
    assert poll_gemini.await_count == 1
    assert poll_openai.await_count == 0
