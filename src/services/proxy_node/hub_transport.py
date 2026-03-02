"""
Hub 模式 tunnel transport

Worker 通过单条到 aether-hub 的 WebSocket 长连接转发 tunnel 帧。
"""

from __future__ import annotations

import asyncio
import gzip
import json
from datetime import datetime, timezone
from typing import TYPE_CHECKING, Any

import aiohttp
import httpx
from aiohttp import WSMsgType

from src.core.logger import logger

from .hub_config import HubConfig, get_hub_config
from .tunnel_manager import TunnelStreamError, _StreamState
from .tunnel_protocol import Frame, FrameFlags, MsgType, normalize_heartbeat_id

if TYPE_CHECKING:
    from collections.abc import AsyncGenerator, Coroutine


_TUNNEL_COMPRESS_MIN_SIZE = 512
_RECONNECT_DELAYS_SECONDS: tuple[float, ...] = (0.0, 0.05, 0.1, 0.2, 0.5, 1.0, 2.0, 5.0)
_HEARTBEAT_DEDUP_TTL_SECONDS = 600

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


class HubConnectionManager:
    """Worker 进程级 Hub 连接管理器（单例）。"""

    def __init__(self, config: HubConfig | None = None) -> None:
        self._config = config or get_hub_config()
        self._session: aiohttp.ClientSession | None = None
        self._ws: aiohttp.ClientWebSocketResponse | None = None

        self._connect_lock = asyncio.Lock()
        self._write_lock = asyncio.Lock()

        self._next_stream_id = 2
        self._pending_streams: dict[int, _StreamState] = {}

        self._reader_task: asyncio.Task[None] | None = None
        self._ping_task: asyncio.Task[None] | None = None
        self._reconnect_task: asyncio.Task[None] | None = None
        self._background_tasks: set[asyncio.Task[None]] = set()

        self._closing = False

    @property
    def is_connected(self) -> bool:
        ws = self._ws
        return ws is not None and not ws.closed

    def _background(self, coro: Coroutine[Any, Any, None]) -> None:
        task = asyncio.create_task(coro)
        self._background_tasks.add(task)
        task.add_done_callback(self._background_tasks.discard)

    async def ensure_connected(self) -> None:
        if self._closing:
            raise TunnelStreamError("hub connection manager is shutting down")
        if self.is_connected:
            return

        async with self._connect_lock:
            if self._closing:
                raise TunnelStreamError("hub connection manager is shutting down")
            if self.is_connected:
                return
            try:
                await self._connect_once()
            except Exception as e:
                self._start_reconnect_loop()
                raise TunnelStreamError(f"failed to connect hub worker channel: {e}") from e

    async def _ensure_session(self) -> aiohttp.ClientSession:
        if self._session is None or self._session.closed:
            self._session = aiohttp.ClientSession()
        return self._session

    async def _connect_once(self) -> None:
        session = await self._ensure_session()

        ws = await session.ws_connect(
            self._config.worker_ws_url,
            timeout=self._config.connect_timeout_seconds,
            autoping=False,
            heartbeat=None,
            max_msg_size=self._config.max_frame_size,
        )

        old_ws = self._ws
        self._ws = ws
        if old_ws is not None and not old_ws.closed:
            try:
                await old_ws.close()
            except Exception:
                pass

        if self._reader_task is not None:
            self._reader_task.cancel()
        if self._ping_task is not None:
            self._ping_task.cancel()

        self._reader_task = asyncio.create_task(self._reader_loop(ws))
        self._ping_task = asyncio.create_task(self._ping_loop(ws))
        logger.info("Hub worker channel connected: {}", self._config.worker_ws_url)

    def _start_reconnect_loop(self) -> None:
        if self._closing:
            return
        if self._reconnect_task is not None and not self._reconnect_task.done():
            return
        self._reconnect_task = asyncio.create_task(self._reconnect_loop())

    async def _reconnect_loop(self) -> None:
        attempt = 0
        while not self._closing and not self.is_connected:
            delay = _RECONNECT_DELAYS_SECONDS[min(attempt, len(_RECONNECT_DELAYS_SECONDS) - 1)]
            if delay > 0:
                await asyncio.sleep(delay)
            try:
                async with self._connect_lock:
                    if self._closing or self.is_connected:
                        break
                    await self._connect_once()
                if self.is_connected:
                    logger.info("Hub worker channel reconnected")
                    break
            except Exception as e:
                attempt += 1
                logger.debug("Hub reconnect attempt {} failed: {}", attempt, e)

    async def _handle_disconnect(
        self,
        reason: str,
        *,
        ws: aiohttp.ClientWebSocketResponse | None = None,
    ) -> None:
        current: aiohttp.ClientWebSocketResponse | None = None
        async with self._connect_lock:
            if self._ws is None:
                return
            if ws is not None and self._ws is not ws:
                return
            current = self._ws
            self._ws = None

        if current is not None and not current.closed:
            try:
                await current.close()
            except Exception:
                pass

        if self._pending_streams:
            for state in self._pending_streams.values():
                state.set_error("hub disconnected")
            self._pending_streams.clear()

        if not self._closing:
            logger.warning("Hub worker channel disconnected: {}", reason)
            self._start_reconnect_loop()

    async def _send_frame(self, frame: Frame) -> None:
        ws = self._ws
        if ws is None or ws.closed:
            raise TunnelStreamError("hub not connected")

        try:
            async with asyncio.timeout(self._config.send_timeout_seconds):
                async with self._write_lock:
                    await ws.send_bytes(frame.encode())
        except TimeoutError as e:
            await self._handle_disconnect("send timeout", ws=ws)
            raise TunnelStreamError("hub frame send timeout") from e
        except Exception as e:
            await self._handle_disconnect(f"send failed: {e}", ws=ws)
            raise TunnelStreamError(f"hub frame send failed: {e}") from e

    async def _reader_loop(self, ws: aiohttp.ClientWebSocketResponse) -> None:
        try:
            while not self._closing:
                msg = await ws.receive()

                if msg.type == WSMsgType.BINARY:
                    raw = msg.data
                    if isinstance(raw, memoryview):
                        raw = raw.tobytes()
                    elif isinstance(raw, bytearray):
                        raw = bytes(raw)
                    if not isinstance(raw, bytes):
                        continue
                    try:
                        frame = Frame.decode(raw)
                    except Exception as e:
                        logger.debug("invalid frame from hub: {}", e)
                        continue
                    await self._handle_incoming_frame(frame)
                    continue

                if msg.type == WSMsgType.CLOSE or msg.type == WSMsgType.CLOSED:
                    break

                if msg.type == WSMsgType.ERROR:
                    logger.debug("hub ws reader error: {}", ws.exception())
                    break

                if msg.type == WSMsgType.PING:
                    payload = msg.data if isinstance(msg.data, bytes) else b""
                    self._background(self._send_pong(payload))
                    continue

                # TEXT / PONG / 其他类型直接忽略
        except asyncio.CancelledError:
            return
        except Exception as e:
            logger.debug("hub reader loop aborted: {}", e)
        finally:
            await self._handle_disconnect("reader ended", ws=ws)

    async def _ping_loop(self, ws: aiohttp.ClientWebSocketResponse) -> None:
        try:
            while not self._closing:
                await asyncio.sleep(self._config.ping_interval_seconds)
                if self._ws is not ws or ws.closed:
                    break
                try:
                    await self._send_frame(Frame(0, MsgType.PING, 0, b""))
                except TunnelStreamError:
                    break
        except asyncio.CancelledError:
            return

    async def _send_pong(self, payload: bytes) -> None:
        try:
            await self._send_frame(Frame(0, MsgType.PONG, 0, payload))
        except TunnelStreamError:
            pass

    async def _handle_incoming_frame(self, frame: Frame) -> None:
        match frame.msg_type:
            # -- stream-level frames --
            case MsgType.RESPONSE_HEADERS:
                stream = self._pending_streams.get(frame.stream_id)
                if not stream:
                    return
                try:
                    payload = _decompress_frame_payload(frame)
                    meta = json.loads(payload)
                    stream.set_response_headers(meta["status"], meta.get("headers", []))
                except Exception as e:
                    stream.set_error(f"invalid response headers: {e}")
                    self._pending_streams.pop(frame.stream_id, None)

            case MsgType.RESPONSE_BODY:
                stream = self._pending_streams.get(frame.stream_id)
                if stream:
                    stream.push_body_chunk(_decompress_frame_payload(frame))

            case MsgType.STREAM_END:
                stream = self._pending_streams.pop(frame.stream_id, None)
                if stream:
                    stream.set_done()

            case MsgType.STREAM_ERROR:
                stream = self._pending_streams.pop(frame.stream_id, None)
                if stream:
                    message = (
                        frame.payload.decode(errors="replace") if frame.payload else "stream error"
                    )
                    stream.set_error(message)

            # -- connection-level frames --
            case MsgType.PING:
                self._background(self._send_pong(frame.payload))

            case MsgType.PONG:
                pass

            case MsgType.GOAWAY:
                await self._handle_disconnect("received GOAWAY")

            case MsgType.HEARTBEAT_DATA:
                self._background(self._handle_heartbeat(frame))

            case MsgType.HEARTBEAT_ACK:
                pass

            case MsgType.NODE_STATUS:
                self._background(self._handle_node_status(frame.payload))

    async def _handle_heartbeat(self, frame: Frame) -> None:
        try:
            data = json.loads(frame.payload) if frame.payload else {}
        except Exception:
            data = {}

        node_id = str(data.get("node_id") or "").strip()
        heartbeat_session_id = str(data.get("heartbeat_session_id") or "").strip()
        if len(heartbeat_session_id) > 128:
            heartbeat_session_id = heartbeat_session_id[:128]
        heartbeat_id = normalize_heartbeat_id(data.get("heartbeat_id"))
        ack: dict[str, object] = {}
        if heartbeat_id is not None:
            ack["heartbeat_id"] = heartbeat_id

        should_process = True
        if node_id and heartbeat_id is not None:
            if heartbeat_session_id:
                dedup_key = f"hub:heartbeat:{node_id}:{heartbeat_session_id}:{heartbeat_id}"
            else:
                dedup_key = f"hub:heartbeat:{node_id}:{heartbeat_id}"
            try:
                from src.clients import get_redis_client

                redis = await get_redis_client()
                if redis:
                    acquired = await redis.set(
                        dedup_key,
                        "1",
                        ex=_HEARTBEAT_DEDUP_TTL_SECONDS,
                        nx=True,
                    )
                    if not acquired:
                        should_process = False
            except Exception:
                # Redis 不可用时降级为不去重，避免心跳链路阻塞
                pass

        def _sync_heartbeat() -> dict[str, object]:
            from src.database import create_session
            from src.services.proxy_node.service import ProxyNodeService, build_heartbeat_ack

            if not node_id:
                return {}

            db = create_session()
            try:
                node = ProxyNodeService.heartbeat(
                    db,
                    node_id=node_id,
                    active_connections=data.get("active_connections"),
                    total_requests=data.get("total_requests"),
                    avg_latency_ms=data.get("avg_latency_ms"),
                    failed_requests=data.get("failed_requests"),
                    dns_failures=data.get("dns_failures"),
                    stream_errors=data.get("stream_errors"),
                    proxy_metadata=data.get("proxy_metadata"),
                    proxy_version=data.get("proxy_version"),
                )
                return build_heartbeat_ack(node)
            finally:
                db.close()

        if should_process:
            try:
                ack.update(await asyncio.to_thread(_sync_heartbeat))
            except Exception as e:
                logger.warning("hub heartbeat DB update failed: {}", e)

        try:
            await self._send_frame(
                Frame(
                    frame.stream_id,
                    MsgType.HEARTBEAT_ACK,
                    0,
                    json.dumps(ack, ensure_ascii=False).encode("utf-8"),
                )
            )
        except TunnelStreamError:
            logger.debug("hub heartbeat ACK send failed")

    async def _handle_node_status(self, payload: bytes) -> None:
        try:
            data = json.loads(payload) if payload else {}
        except Exception:
            return

        node_id = str(data.get("node_id") or "").strip()
        if not node_id:
            return

        connected = bool(data.get("connected"))
        conn_count = int(data.get("conn_count") or 0)

        # 所有 worker 都需要立即失效本地缓存，保证请求路由正确
        try:
            from src.services.proxy_node.resolver import invalidate_proxy_node_cache

            invalidate_proxy_node_cache(node_id)
        except Exception:
            pass

        # 使用 Redis SETNX 去重：同一次 NODE_STATUS 广播只有一个 worker 执行 DB 写入，
        # 避免 N 个 worker 并发写同一行并产生 N 条重复事件记录。
        dedup_key = f"hub:node_status:{node_id}:{connected}:{conn_count}"
        try:
            from src.clients import get_redis_client

            redis = await get_redis_client()
            if redis:
                acquired = await redis.set(dedup_key, "1", ex=10, nx=True)
                if not acquired:
                    return
        except Exception:
            # Redis 不可用时不去重，允许重复写入（幂等）
            pass

        def _sync_update() -> None:
            from src.database import create_session
            from src.models.database import ProxyNode, ProxyNodeEvent, ProxyNodeStatus

            db = create_session()
            try:
                node = db.query(ProxyNode).filter(ProxyNode.id == node_id).first()
                if not node:
                    return

                now = datetime.now(timezone.utc)

                node.tunnel_connected = connected
                if connected:
                    node.tunnel_connected_at = now
                node.status = ProxyNodeStatus.ONLINE if connected else ProxyNodeStatus.OFFLINE
                node.updated_at = now

                event = ProxyNodeEvent(
                    node_id=node_id,
                    event_type="connected" if connected else "disconnected",
                    detail=f"[hub_node_status] conn_count={conn_count}",
                )
                db.add(event)
                db.commit()
            except Exception:
                try:
                    db.rollback()
                except Exception:
                    pass
                raise
            finally:
                db.close()

        try:
            await asyncio.to_thread(_sync_update)
        except Exception as e:
            logger.warning("hub NODE_STATUS DB update failed: node_id={}, error={}", node_id, e)

    async def send_request(
        self,
        node_id: str,
        *,
        method: str,
        url: str,
        headers: dict[str, str],
        body: bytes | None = None,
        timeout: float = 60.0,
    ) -> _StreamState:
        await self.ensure_connected()

        if len(self._pending_streams) >= self._config.max_streams:
            raise TunnelStreamError(
                f"hub stream limit reached ({self._config.max_streams}) for node {node_id}"
            )
        stream_id = self._alloc_stream_id()
        stream_state = _StreamState(stream_id)
        self._pending_streams[stream_id] = stream_state

        try:
            meta = json.dumps(
                {
                    "node_id": node_id,
                    "method": method,
                    "url": url,
                    "headers": headers,
                    "timeout": int(timeout),
                },
                ensure_ascii=False,
                separators=(",", ":"),
            ).encode("utf-8")

            meta_payload, meta_flags = _compress_frame_payload(meta)
            await self._send_frame(
                Frame(stream_id, MsgType.REQUEST_HEADERS, meta_flags, meta_payload)
            )

            body_data = body or b""
            if body_data:
                body_payload, body_flags = _compress_frame_payload(body_data)
            else:
                body_payload, body_flags = body_data, 0
            body_flags |= FrameFlags.END_STREAM
            await self._send_frame(Frame(stream_id, MsgType.REQUEST_BODY, body_flags, body_payload))
        except Exception:
            self._pending_streams.pop(stream_id, None)
            raise

        return stream_state

    def remove_stream(self, stream_id: int) -> None:
        self._pending_streams.pop(stream_id, None)

    def _alloc_stream_id(self) -> int:
        sid = self._next_stream_id
        self._next_stream_id = sid + 2 if sid < 0xFFFF_FFFE else 2
        return sid

    async def shutdown(self) -> None:
        self._closing = True

        if self._reconnect_task is not None:
            self._reconnect_task.cancel()
        if self._reader_task is not None:
            self._reader_task.cancel()
        if self._ping_task is not None:
            self._ping_task.cancel()

        tasks = list(self._background_tasks)
        for task in tasks:
            task.cancel()
        if tasks:
            await asyncio.gather(*tasks, return_exceptions=True)

        if self._ws is not None and not self._ws.closed:
            try:
                await self._ws.close()
            except Exception:
                pass
        self._ws = None

        if self._session is not None and not self._session.closed:
            try:
                await self._session.close()
            except Exception:
                pass
        self._session = None

        if self._pending_streams:
            for state in self._pending_streams.values():
                state.set_error("hub connection manager shutdown")
            self._pending_streams.clear()

        logger.info("Hub connection manager shutdown completed")


class HubTunnelTransport(httpx.AsyncBaseTransport):
    """通过 aether-hub 转发请求的 httpx transport。"""

    def __init__(self, node_id: str, timeout: float = 60.0) -> None:
        self._node_id = node_id
        self._timeout = timeout

    async def handle_async_request(self, request: httpx.Request) -> httpx.Response:
        manager = get_hub_connection_manager()

        headers: dict[str, str] = {}
        for key, value in request.headers.raw:
            if key not in _HOP_BY_HOP_HEADERS_BYTES:
                headers[key.decode("latin-1")] = value.decode("latin-1")

        body = request.content or await request.aread() or None

        stream_state: _StreamState | None = None
        try:
            stream_state = await manager.send_request(
                self._node_id,
                method=request.method,
                url=str(request.url),
                headers=headers,
                body=body,
                timeout=self._timeout,
            )
            await stream_state.wait_headers(timeout=self._timeout)

            return httpx.Response(
                status_code=stream_state.status,
                headers=httpx.Headers(stream_state.headers),
                stream=HubResponseStream(manager, stream_state, timeout=self._timeout),
            )
        except TunnelStreamError as e:
            self._cleanup_stream(manager, stream_state)
            if stream_state and stream_state.status > 0:
                raise httpx.ReadError(str(e)) from e
            raise httpx.ConnectError(str(e)) from e
        except asyncio.TimeoutError:
            self._cleanup_stream(manager, stream_state)
            raise httpx.ReadTimeout("hub tunnel request timeout") from None

    def _cleanup_stream(
        self,
        manager: HubConnectionManager,
        stream_state: _StreamState | None,
    ) -> None:
        if stream_state is None:
            return
        manager.remove_stream(stream_state.stream_id)


class HubResponseStream(httpx.AsyncByteStream):
    def __init__(
        self,
        manager: HubConnectionManager,
        stream_state: _StreamState,
        timeout: float = 60.0,
    ) -> None:
        self._manager = manager
        self._stream_state = stream_state
        self._timeout = timeout

    async def __aiter__(self) -> AsyncGenerator[bytes, None]:
        async for chunk in self._stream_state.iter_body(chunk_timeout=self._timeout):
            yield chunk

    async def aclose(self) -> None:
        self._manager.remove_stream(self._stream_state.stream_id)


def _compress_frame_payload(data: bytes) -> tuple[bytes, int]:
    if len(data) >= _TUNNEL_COMPRESS_MIN_SIZE:
        compressed = gzip.compress(data, compresslevel=6)
        if len(compressed) < len(data):
            return compressed, FrameFlags.GZIP_COMPRESSED
    return data, 0


def _decompress_frame_payload(frame: Frame) -> bytes:
    if frame.is_gzip:
        return gzip.decompress(frame.payload)
    return frame.payload


_hub_connection_manager: HubConnectionManager | None = None


def get_hub_connection_manager() -> HubConnectionManager:
    global _hub_connection_manager
    if _hub_connection_manager is None:
        _hub_connection_manager = HubConnectionManager()
    return _hub_connection_manager


async def shutdown_hub_connection_manager() -> None:
    global _hub_connection_manager
    if _hub_connection_manager is None:
        return
    await _hub_connection_manager.shutdown()
    _hub_connection_manager = None
