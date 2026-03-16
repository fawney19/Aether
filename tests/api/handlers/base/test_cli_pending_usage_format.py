from __future__ import annotations

import pytest

from src.api.handlers.base.cli_stream_mixin import CliStreamMixin
from src.api.handlers.base.cli_sync_mixin import CliSyncMixin


class _StopExecution(Exception):
    pass


class _DummySyncHandler(CliSyncMixin):
    FORMAT_ID = "openai:cli"

    def __init__(self) -> None:
        self.allowed_api_formats = ["openai:compact"]
        self.primary_api_format = "openai:compact"
        self.pending_calls: list[dict[str, object]] = []

    def extract_model_from_request(
        self, request_body: dict[str, object], path_params: dict[str, object] | None
    ) -> str:
        return str(request_body.get("model") or "unknown")

    def _create_pending_usage(self, **kwargs: object) -> bool:
        self.pending_calls.append(kwargs)
        raise _StopExecution()


class _DummyStreamHandler(CliStreamMixin):
    FORMAT_ID = "openai:cli"

    def __init__(self) -> None:
        self.allowed_api_formats = ["openai:compact"]
        self.primary_api_format = "openai:compact"
        self.pending_calls: list[dict[str, object]] = []

    def extract_model_from_request(
        self, request_body: dict[str, object], path_params: dict[str, object] | None
    ) -> str:
        return str(request_body.get("model") or "unknown")

    def _create_pending_usage(self, **kwargs: object) -> bool:
        self.pending_calls.append(kwargs)
        raise _StopExecution()


@pytest.mark.asyncio
async def test_sync_pending_usage_uses_primary_api_format() -> None:
    handler = _DummySyncHandler()

    with pytest.raises(_StopExecution):
        await handler.process_sync(  # type: ignore[misc]
            original_request_body={"model": "gpt-5.3-codex"},
            original_headers={},
        )

    assert handler.pending_calls
    assert handler.pending_calls[0]["api_format"] == "openai:compact"


@pytest.mark.asyncio
async def test_stream_pending_usage_uses_primary_api_format() -> None:
    handler = _DummyStreamHandler()

    with pytest.raises(_StopExecution):
        await handler.process_stream(  # type: ignore[misc]
            original_request_body={"model": "gpt-5.3-codex"},
            original_headers={},
        )

    assert handler.pending_calls
    assert handler.pending_calls[0]["api_format"] == "openai:compact"
