from __future__ import annotations

import asyncio
from collections.abc import AsyncGenerator

import pytest

from src.api.handlers.base.cli_monitor_mixin import CliMonitorMixin
from src.api.handlers.base.stream_context import StreamContext


class _DummyMonitor(CliMonitorMixin):
    pass


class _RequestStub:
    def __init__(self, responses: list[bool | Exception]):
        self._responses = responses

    async def is_disconnected(self) -> bool:
        if self._responses:
            value = self._responses.pop(0)
        else:
            value = False
        if isinstance(value, Exception):
            raise value
        return value


async def _cancel_immediately() -> AsyncGenerator[bytes, None]:
    if False:
        yield b""
    raise asyncio.CancelledError()


async def _yield_once_then_cancel(ctx: StreamContext) -> AsyncGenerator[bytes, None]:
    ctx.append_text("partial output")
    yield b"data: chunk\n\n"
    raise asyncio.CancelledError()


@pytest.mark.asyncio
async def test_create_monitored_stream_marks_client_disconnected_when_confirmed() -> None:
    monitor = _DummyMonitor()
    monitor.CANCEL_DISCONNECT_RETRY_DELAYS_SECONDS = ()
    ctx = StreamContext(model="test-model", api_format="openai:cli", request_id="req-client")

    request = _RequestStub([True])
    monitored = monitor._create_monitored_stream(ctx, _cancel_immediately(), request)

    with pytest.raises(asyncio.CancelledError):
        async for _ in monitored:
            pass

    assert ctx.status_code == 499
    assert ctx.error_message == "client_disconnected"
    assert "cancel_origin=client_disconnected" in (ctx.upstream_response or "")


@pytest.mark.asyncio
async def test_create_monitored_stream_marks_server_cancelled_when_confirmed_connected() -> None:
    monitor = _DummyMonitor()
    monitor.CANCEL_DISCONNECT_RETRY_DELAYS_SECONDS = ()
    ctx = StreamContext(model="test-model", api_format="openai:cli", request_id="req-server")

    request = _RequestStub([False])
    monitored = monitor._create_monitored_stream(ctx, _cancel_immediately(), request)

    with pytest.raises(asyncio.CancelledError):
        async for _ in monitored:
            pass

    assert ctx.status_code == 503
    assert ctx.error_message == "server_cancelled"
    assert "cancel_origin=server_cancelled" in (ctx.upstream_response or "")


@pytest.mark.asyncio
async def test_create_monitored_stream_marks_cancelled_unknown_when_disconnect_check_uncertain() -> (
    None
):
    monitor = _DummyMonitor()
    monitor.CANCEL_DISCONNECT_RETRY_DELAYS_SECONDS = ()
    ctx = StreamContext(model="test-model", api_format="openai:cli", request_id="req-unknown")

    request = _RequestStub([asyncio.TimeoutError()])
    monitored = monitor._create_monitored_stream(ctx, _cancel_immediately(), request)

    with pytest.raises(asyncio.CancelledError):
        async for _ in monitored:
            pass

    assert ctx.status_code == 503
    assert ctx.error_message == "cancelled_unknown"
    assert "cancel_origin=cancelled_unknown" in (ctx.upstream_response or "")


@pytest.mark.asyncio
async def test_create_monitored_stream_estimates_output_tokens_before_unknown_cancel_log() -> None:
    monitor = _DummyMonitor()
    monitor.CANCEL_DISCONNECT_RETRY_DELAYS_SECONDS = ()
    ctx = StreamContext(model="test-model", api_format="openai:cli", request_id="req-estimate")

    request = _RequestStub([asyncio.TimeoutError()])
    monitored = monitor._create_monitored_stream(ctx, _yield_once_then_cancel(ctx), request)

    with pytest.raises(asyncio.CancelledError):
        async for _ in monitored:
            pass

    expected_output_tokens = max(1, len("partial output") // 4)
    assert ctx.status_code == 503
    assert ctx.error_message == "cancelled_unknown"
    assert ctx.output_tokens == expected_output_tokens
    assert f"output_tokens={expected_output_tokens}" in (ctx.upstream_response or "")
