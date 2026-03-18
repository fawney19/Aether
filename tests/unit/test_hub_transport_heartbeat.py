from __future__ import annotations

import json
import struct
from typing import Any

import httpx
import pytest

from src.services.proxy_node.hub_config import HubConfig
from src.services.proxy_node.hub_transport import HubTunnelTransport


class _FakeRelayClient:
    def __init__(self, response: httpx.Response) -> None:
        self.response = response
        self.sent_request: httpx.Request | None = None
        self.sent_body: bytes | None = None

    def build_request(self, method: str, url: str, **kwargs: Any) -> httpx.Request:
        return httpx.Request(method, url, **kwargs)

    async def send(self, request: httpx.Request, *, stream: bool = False) -> httpx.Response:
        _ = stream
        self.sent_request = request
        self.sent_body = await request.aread()
        self.response.request = request
        return self.response

    async def aclose(self) -> None:
        return None


def _relay_config() -> HubConfig:
    return HubConfig(
        enabled=True,
        url="http://127.0.0.1:8085",
        connect_timeout_seconds=1.0,
    )


@pytest.mark.asyncio
async def test_transport_encodes_local_relay_envelope(monkeypatch: pytest.MonkeyPatch) -> None:
    transport = HubTunnelTransport("node-1", timeout=12.0)
    fake_client = _FakeRelayClient(httpx.Response(200, content=b"ok"))
    monkeypatch.setattr("src.services.proxy_node.hub_transport.get_hub_config", _relay_config)
    monkeypatch.setattr(transport, "_relay_client", fake_client)

    request = httpx.Request(
        "POST",
        "https://example.com/v1/chat/completions",
        headers={"content-type": "application/json", "connection": "keep-alive"},
        content=b'{"hello":"world"}',
    )

    response = await transport.handle_async_request(request)
    assert response.status_code == 200
    await response.aclose()

    assert fake_client.sent_request is not None
    payload = fake_client.sent_body
    assert payload is not None
    meta_len = struct.unpack("!I", payload[:4])[0]
    meta = json.loads(payload[4 : 4 + meta_len].decode("utf-8"))
    assert meta == {
        "method": "POST",
        "url": "https://example.com/v1/chat/completions",
        "headers": {"content-type": "application/json"},
        "timeout": 12,
    }
    assert payload[4 + meta_len :] == b'{"hello":"world"}'


@pytest.mark.asyncio
async def test_transport_maps_relay_timeout_to_read_timeout(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    transport = HubTunnelTransport("node-1", timeout=12.0)
    fake_client = _FakeRelayClient(
        httpx.Response(
            504,
            headers={"x-aether-tunnel-error": "timeout"},
            content=b"relay timed out",
        )
    )
    monkeypatch.setattr("src.services.proxy_node.hub_transport.get_hub_config", _relay_config)
    monkeypatch.setattr(transport, "_relay_client", fake_client)

    request = httpx.Request("GET", "https://example.com")

    with pytest.raises(httpx.ReadTimeout, match="relay timed out"):
        await transport.handle_async_request(request)


@pytest.mark.asyncio
async def test_transport_streams_request_body_from_async_generator(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    transport = HubTunnelTransport("node-1", timeout=12.0)
    fake_client = _FakeRelayClient(httpx.Response(200, content=b"ok"))
    monkeypatch.setattr("src.services.proxy_node.hub_transport.get_hub_config", _relay_config)
    monkeypatch.setattr(transport, "_relay_client", fake_client)

    async def body() -> Any:
        yield b'{"hello":'
        yield b'"world"}'

    request = httpx.Request(
        "POST",
        "https://example.com/v1/chat/completions",
        headers={"content-type": "application/json"},
        content=body(),
    )

    response = await transport.handle_async_request(request)
    assert response.status_code == 200
    await response.aclose()

    payload = fake_client.sent_body
    assert payload is not None
    meta_len = struct.unpack("!I", payload[:4])[0]
    assert payload[4 + meta_len :] == b'{"hello":"world"}'
