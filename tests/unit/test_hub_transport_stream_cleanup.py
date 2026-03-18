from __future__ import annotations

import pytest

from src.services.proxy_node.hub_transport import HubRelayResponseStream


class _FakeResponse:
    def __init__(self, chunks: list[bytes]) -> None:
        self._chunks = chunks
        self.closed = False

    async def aiter_raw(self):  # type: ignore[override]
        for chunk in self._chunks:
            yield chunk

    async def aclose(self) -> None:
        self.closed = True


@pytest.mark.asyncio
async def test_hub_relay_response_stream_closes_after_iteration() -> None:
    response = _FakeResponse([b"hello", b"world"])
    stream = HubRelayResponseStream(response)  # type: ignore[arg-type]

    chunks = []
    async for chunk in stream:
        chunks.append(chunk)

    assert chunks == [b"hello", b"world"]
    assert response.closed is True


@pytest.mark.asyncio
async def test_hub_relay_response_stream_aclose_closes_response() -> None:
    response = _FakeResponse([])
    stream = HubRelayResponseStream(response)  # type: ignore[arg-type]

    await stream.aclose()

    assert response.closed is True
