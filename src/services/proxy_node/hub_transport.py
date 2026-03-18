"""
Hub 模式 tunnel transport

Worker 通过本机 aether-hub 的 HTTP relay 访问 tunnel 数据面，不再维护 /worker WebSocket。
"""

from __future__ import annotations

import json
import struct
from typing import TYPE_CHECKING

import httpx

from .hub_config import get_hub_config

if TYPE_CHECKING:
    from collections.abc import AsyncGenerator


_HOP_BY_HOP_HEADERS = frozenset(
    {
        "host",
        "transfer-encoding",
        "content-length",
        "connection",
        "upgrade",
        "keep-alive",
        "proxy-authorization",
        "proxy-connection",
        "te",
        "trailer",
    }
)
_HOP_BY_HOP_HEADERS_BYTES = frozenset(h.encode("ascii") for h in _HOP_BY_HOP_HEADERS)
_RELAY_CONTENT_TYPE = "application/vnd.aether.tunnel-envelope"
_TUNNEL_ERROR_HEADER = "x-aether-tunnel-error"


class HubTunnelTransport(httpx.AsyncBaseTransport):
    """通过本机 aether-hub relay 转发请求的 httpx transport。"""

    def __init__(self, node_id: str, timeout: float = 60.0) -> None:
        self._node_id = node_id
        self._timeout = timeout

        config = get_hub_config()
        relay_timeout = max(timeout + 5.0, config.connect_timeout_seconds)
        self._relay_client = httpx.AsyncClient(
            transport=httpx.AsyncHTTPTransport(retries=0),
            timeout=httpx.Timeout(
                connect=config.connect_timeout_seconds,
                read=relay_timeout,
                write=relay_timeout,
                pool=relay_timeout,
            ),
        )

    async def handle_async_request(self, request: httpx.Request) -> httpx.Response:
        config = get_hub_config()
        if not config.enabled:
            raise httpx.ConnectError("hub local relay is unavailable outside docker runtime")

        headers: dict[str, str] = {}
        for key, value in request.headers.raw:
            if key.lower() not in _HOP_BY_HOP_HEADERS_BYTES:
                headers[key.decode("latin-1")] = value.decode("latin-1")

        relay_content = _iter_relay_envelope(
            {
                "method": request.method,
                "url": str(request.url),
                "headers": headers,
                "timeout": int(self._timeout),
            },
            request,
        )

        relay_request = self._relay_client.build_request(
            "POST",
            config.local_relay_url(self._node_id),
            headers={"content-type": _RELAY_CONTENT_TYPE},
            content=relay_content,
        )

        try:
            relay_response = await self._relay_client.send(relay_request, stream=True)
        except httpx.ConnectError as exc:
            raise httpx.ConnectError(f"hub local relay connect failed: {exc}") from exc
        except httpx.TimeoutException as exc:
            raise httpx.ConnectError(f"hub local relay timeout: {exc}") from exc

        tunnel_error = relay_response.headers.get(_TUNNEL_ERROR_HEADER)
        if tunnel_error:
            message = await _read_error_message(relay_response)
            if tunnel_error == "timeout":
                raise httpx.ReadTimeout(message or "hub relay timed out")
            raise httpx.ConnectError(message or f"hub relay error: {tunnel_error}")

        return httpx.Response(
            status_code=relay_response.status_code,
            headers=httpx.Headers(relay_response.headers),
            stream=HubRelayResponseStream(relay_response),
            request=request,
        )

    async def aclose(self) -> None:
        await self._relay_client.aclose()


class HubRelayResponseStream(httpx.AsyncByteStream):
    def __init__(self, response: httpx.Response) -> None:
        self._response = response

    async def __aiter__(self) -> AsyncGenerator[bytes, None]:
        try:
            async for chunk in self._response.aiter_raw():
                yield chunk
        finally:
            await self._response.aclose()

    async def aclose(self) -> None:
        await self._response.aclose()


async def _iter_relay_envelope(
    meta: dict[str, object],
    request: httpx.Request,
) -> AsyncGenerator[bytes, None]:
    meta_json = json.dumps(meta, ensure_ascii=False, separators=(",", ":")).encode("utf-8")
    yield struct.pack("!I", len(meta_json)) + meta_json
    async for chunk in _iter_request_body(request):
        if chunk:
            yield chunk


async def _iter_request_body(request: httpx.Request) -> AsyncGenerator[bytes, None]:
    stream = request.stream
    try:
        if hasattr(stream, "__aiter__"):
            async for chunk in stream:
                if chunk:
                    yield bytes(chunk)
            return

        if hasattr(stream, "__iter__"):
            for chunk in stream:
                if chunk:
                    yield bytes(chunk)
            return

        body = request.content
        if body:
            yield body
    finally:
        aclose = getattr(stream, "aclose", None)
        if callable(aclose):
            await aclose()


async def _read_error_message(response: httpx.Response) -> str:
    try:
        payload = await response.aread()
        return payload.decode("utf-8", errors="replace").strip()
    finally:
        await response.aclose()
