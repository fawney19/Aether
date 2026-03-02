"""
WebSocket 隧道管理器

管理所有活跃的 aether-proxy tunnel 连接，提供通过隧道发送 HTTP 请求的能力。
每个 proxy node 可持有多条 tunnel 连接（连接池），请求按 least-loaded 策略分配。
"""

from __future__ import annotations

import asyncio
import gzip
import json
import time
from typing import TYPE_CHECKING, Any

if TYPE_CHECKING:
    from collections.abc import AsyncGenerator

from starlette.websockets import WebSocket, WebSocketState

from src.core.logger import logger

from .tunnel_protocol import Frame, FrameFlags, MsgType, normalize_heartbeat_id

# 隧道帧压缩的最小 payload 大小（字节）
# 小于此值的帧压缩收益不大，反而增加 CPU 开销
_TUNNEL_COMPRESS_MIN_SIZE = 512


class TunnelConnection:
    """单条 tunnel 连接"""

    __slots__ = (
        "node_id",
        "node_name",
        "ws",
        "connected_at",
        "max_streams",
        "_pending_streams",
        "_write_lock",
        "_next_stream_id",
    )

    def __init__(
        self,
        node_id: str,
        node_name: str,
        ws: WebSocket,
        max_streams: int | None = None,
    ) -> None:
        self.node_id = node_id
        self.node_name = node_name
        self.ws = ws
        self.connected_at = time.time()
        # Per-connection max concurrent streams: use proxy-advertised value
        # (from X-Tunnel-Max-Streams header), clamped to [64, 2048].
        # Falls back to TunnelManager.MAX_STREAMS_PER_CONN if not provided.
        if max_streams is not None:
            self.max_streams = max(64, min(max_streams, 2048))
        else:
            self.max_streams = TunnelManager.MAX_STREAMS_PER_CONN
        self._pending_streams: dict[int, _StreamState] = {}
        self._write_lock = asyncio.Lock()
        # Per-connection stream ID 分配器（Aether 端使用偶数，从 2 开始）
        self._next_stream_id: int = 2

    @property
    def is_alive(self) -> bool:
        return self.ws.client_state == WebSocketState.CONNECTED

    async def send_frame(self, frame: Frame, timeout: float = 10.0) -> None:
        """发送帧到 WebSocket，带超时保护防止写阻塞。

        在高丢包网络下 TCP 写缓冲区可能满，send_bytes 会长时间阻塞。
        加超时避免所有协程在 _write_lock 上排队导致级联失败。
        """
        try:
            async with asyncio.timeout(timeout):
                async with self._write_lock:
                    await self.ws.send_bytes(frame.encode())
        except TimeoutError:
            raise TunnelStreamError("frame send timeout (writer congested)")

    def create_stream(self, stream_id: int) -> _StreamState:
        state = _StreamState(stream_id, conn=self)
        self._pending_streams[stream_id] = state
        return state

    def get_stream(self, stream_id: int) -> _StreamState | None:
        return self._pending_streams.get(stream_id)

    def remove_stream(self, stream_id: int) -> None:
        self._pending_streams.pop(stream_id, None)

    @property
    def stream_count(self) -> int:
        return len(self._pending_streams)

    def has_stream(self, stream_id: int) -> bool:
        return stream_id in self._pending_streams

    def alloc_stream_id(self, max_streams: int) -> int:
        """分配一个未被占用的偶数 stream_id，回绕时跳过飞行中的 ID"""
        # 最多尝试 max_streams + 16 次（飞行中的 stream 数量不超过 max_streams）
        for _ in range(max_streams + 16):
            sid = self._next_stream_id
            self._next_stream_id += 2
            if self._next_stream_id > 0xFFFF_FFFE:
                self._next_stream_id = 2
            if sid not in self._pending_streams:
                return sid
        raise TunnelStreamError("stream ID space exhausted")

    def cancel_all_streams(self) -> None:
        for state in self._pending_streams.values():
            state.set_error("tunnel disconnected")
        self._pending_streams.clear()


class _StreamState:
    """跟踪单个 stream 的响应状态"""

    __slots__ = (
        "stream_id",
        "status",
        "headers",
        "_header_event",
        "_body_chunks",
        "_done_event",
        "_error",
        "_conn",
    )

    def __init__(self, stream_id: int, conn: TunnelConnection | None = None) -> None:
        self.stream_id = stream_id
        self.status: int = 0
        self.headers: list[list[str]] = []
        self._header_event = asyncio.Event()
        self._body_chunks: asyncio.Queue[bytes | None] = asyncio.Queue()
        self._done_event = asyncio.Event()
        self._error: str | None = None
        self._conn = conn

    def set_response_headers(self, status: int, headers: list[list[str]] | dict[str, str]) -> None:
        self.status = status
        # headers 可能是 [[k, v], ...] (多值) 或 {k: v} (旧格式兼容)
        if isinstance(headers, list):
            self.headers = headers  # type: ignore[assignment]
        else:
            self.headers = list(headers.items())  # type: ignore[assignment]
        self._header_event.set()

    def push_body_chunk(self, data: bytes) -> None:
        self._body_chunks.put_nowait(data)

    def set_done(self) -> None:
        self._body_chunks.put_nowait(None)  # sentinel
        self._done_event.set()

    def set_error(self, msg: str) -> None:
        self._error = msg
        self._header_event.set()
        self._body_chunks.put_nowait(None)
        self._done_event.set()

    async def wait_headers(self, timeout: float = 60.0) -> None:
        await asyncio.wait_for(self._header_event.wait(), timeout=timeout)
        if self._error:
            raise TunnelStreamError(self._error)

    async def iter_body(self, chunk_timeout: float = 60.0) -> AsyncGenerator[bytes, None]:
        while True:
            try:
                chunk = await asyncio.wait_for(self._body_chunks.get(), timeout=chunk_timeout)
            except asyncio.TimeoutError:
                self._error = "body chunk timeout"
                self._done_event.set()
                raise TunnelStreamError("body chunk timeout")
            if chunk is None:
                if self._error:
                    raise TunnelStreamError(self._error)
                return
            yield chunk


class TunnelStreamError(Exception):
    pass


# ---------------------------------------------------------------------------
# 全局 TunnelManager 单例
# ---------------------------------------------------------------------------


class TunnelManager:
    """管理所有活跃的 tunnel 连接（支持每个 node 多条连接的连接池）"""

    # 单条 tunnel 上允许的最大并发 stream 数（超出时拒绝新请求）
    MAX_STREAMS_PER_CONN = 2048

    def __init__(self) -> None:
        self._connections: dict[str, list[TunnelConnection]] = {}  # node_id -> [conn, ...]
        self._background_tasks: set[asyncio.Task[None]] = set()
        self._draining: bool = False

    def _background(self, coro: Any) -> None:  # noqa: ANN401
        """启动 fire-and-forget task，通过 set 持有引用防止 GC 回收，完成后自动清理"""
        task = asyncio.create_task(coro)
        self._background_tasks.add(task)
        task.add_done_callback(self._background_tasks.discard)

    @property
    def active_count(self) -> int:
        return sum(len(conns) for conns in self._connections.values())

    def get_connection(self, node_id: str) -> TunnelConnection | None:
        """获取负载最低的存活连接，同时清理 dead 连接"""
        conns = self._connections.get(node_id)
        if not conns:
            return None

        # 清理 dead 连接
        alive = [c for c in conns if c.is_alive]
        dead = [c for c in conns if not c.is_alive]
        for c in dead:
            c.cancel_all_streams()

        if not alive:
            self._connections.pop(node_id, None)
            return None

        if len(alive) != len(conns):
            self._connections[node_id] = alive

        # Least-loaded: 选 stream_count 最小的连接
        return min(alive, key=lambda c: c.stream_count)

    def register(self, conn: TunnelConnection) -> None:
        """注册一条新连接到连接池"""
        conns = self._connections.get(conn.node_id)
        if conns is None:
            conns = []
            self._connections[conn.node_id] = conns
        conns.append(conn)
        logger.info(
            "tunnel connected: node_id={}, name={}, pool_size={}",
            conn.node_id,
            conn.node_name,
            len(conns),
        )

    def unregister(self, conn: TunnelConnection) -> bool:
        """
        从连接池中注销指定连接。

        返回 True 表示成功移除，False 表示该连接已不在池中。
        """
        conns = self._connections.get(conn.node_id)
        if not conns:
            return False

        try:
            conns.remove(conn)  # identity comparison via list.remove
        except ValueError:
            return False

        conn.cancel_all_streams()

        if not conns:
            self._connections.pop(conn.node_id, None)

        remaining = len(conns) if conns else 0
        logger.info(
            "tunnel disconnected: node_id={}, name={}, remaining={}",
            conn.node_id,
            conn.node_name,
            remaining,
        )
        return True

    async def shutdown_all(self, drain_timeout: float = 60.0) -> None:
        """优雅关闭所有 tunnel 连接：drain 飞行中请求 -> GoAway -> 关闭 WebSocket。

        在 worker 即将退出时调用。先标记 draining 阻止新请求进入，
        等待飞行中的 stream 完成（最多 drain_timeout 秒），
        然后发送 GoAway 让 proxy 端重连到其他 worker。
        """
        all_conns = [c for conns in self._connections.values() for c in conns]
        if not all_conns:
            return

        # 标记 draining，send_request 将拒绝新请求
        self._draining = True

        total_streams = sum(c.stream_count for c in all_conns)
        if total_streams > 0:
            logger.info(
                "draining {} in-flight streams on {} connections (timeout={}s)",
                total_streams,
                len(all_conns),
                drain_timeout,
            )
            try:
                await asyncio.wait_for(self._wait_streams_drain(all_conns), timeout=drain_timeout)
            except asyncio.TimeoutError:
                remaining = sum(c.stream_count for c in all_conns)
                logger.warning("drain timeout, {} streams still in-flight", remaining)

        logger.info("sending GoAway to {} tunnel connections", len(all_conns))

        async def _close_conn(conn: TunnelConnection) -> None:
            try:
                await asyncio.wait_for(
                    conn.send_frame(Frame(0, MsgType.GOAWAY, 0, b"")),
                    timeout=2.0,
                )
            except Exception:
                pass
            try:
                await conn.ws.close(code=1001, reason="server shutting down")
            except Exception:
                pass

        await asyncio.gather(*(_close_conn(c) for c in all_conns), return_exceptions=True)

    async def _wait_streams_drain(self, conns: list[TunnelConnection]) -> None:
        """轮询等待所有连接的 pending_streams 清空"""
        while any(c.stream_count > 0 for c in conns):
            await asyncio.sleep(0.5)

    def has_tunnel(self, node_id: str) -> bool:
        """检查指定 node 是否有存活的 tunnel 连接（纯检查，无副作用）

        与 get_connection 不同，此方法不会清理 dead 连接，
        避免在 finally 块或 health_scheduler 中误清理刚注册的连接。
        """
        conns = self._connections.get(node_id)
        if not conns:
            return False
        return any(c.is_alive for c in conns)

    def connection_count(self, node_id: str) -> int:
        """返回指定 node 当前存活的连接数"""
        conns = self._connections.get(node_id)
        if not conns:
            return 0
        return sum(1 for c in conns if c.is_alive)

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
        """
        通过 tunnel 发送 HTTP 请求，返回 StreamState 用于读取响应。
        """
        if self._draining:
            raise TunnelStreamError("tunnel manager is draining, rejecting new requests")

        conn = self.get_connection(node_id)
        if not conn:
            raise TunnelStreamError(f"tunnel not connected for node {node_id}")

        if conn.stream_count >= conn.max_streams:
            raise TunnelStreamError(
                f"tunnel stream limit reached ({conn.max_streams}) for node {node_id}"
            )

        stream_id = conn.alloc_stream_id(conn.max_streams)
        stream_state = conn.create_stream(stream_id)

        try:
            # 发送 REQUEST_HEADERS（大元数据帧压缩）
            meta = json.dumps(
                {
                    "method": method,
                    "url": url,
                    "headers": headers,
                    "timeout": int(timeout),
                }
            ).encode()
            meta_payload, meta_flags = _compress_frame_payload(meta)
            await conn.send_frame(
                Frame(stream_id, MsgType.REQUEST_HEADERS, meta_flags, meta_payload)
            )

            # 发送 REQUEST_BODY + END_STREAM（大请求体帧压缩）
            body_data = body or b""
            if body_data:
                body_payload, body_flags = _compress_frame_payload(body_data)
            else:
                body_payload, body_flags = body_data, 0
            body_flags |= FrameFlags.END_STREAM
            await conn.send_frame(Frame(stream_id, MsgType.REQUEST_BODY, body_flags, body_payload))
        except Exception:
            conn.remove_stream(stream_id)
            raise

        return stream_state

    async def handle_incoming_frame(self, conn: TunnelConnection, frame: Frame) -> None:
        """处理从 proxy 收到的响应帧（仅处理当前 active 连接的帧）。

        重要：此方法在 WebSocket 主读循环中被 await 调用，不能长时间阻塞，
        否则会阻止读取后续帧，导致 proxy 端 TCP 缓冲区满而级联失败。
        """
        # 防止已被移除的连接的帧继续被处理
        conns = self._connections.get(conn.node_id)
        if not conns or conn not in conns:
            return

        stream = conn.get_stream(frame.stream_id)

        if frame.msg_type == MsgType.RESPONSE_HEADERS:
            if not stream:
                return
            try:
                payload = _decompress_frame_payload(frame)
                meta = json.loads(payload)
                stream.set_response_headers(meta["status"], meta.get("headers", []))
            except Exception as e:
                stream.set_error(f"invalid response headers: {e}")

        elif frame.msg_type == MsgType.RESPONSE_BODY:
            if stream:
                payload = _decompress_frame_payload(frame)
                stream.push_body_chunk(payload)

        elif frame.msg_type == MsgType.STREAM_END:
            if stream:
                stream.set_done()
                conn.remove_stream(frame.stream_id)

        elif frame.msg_type == MsgType.STREAM_ERROR:
            if stream:
                msg = frame.payload.decode(errors="replace") if frame.payload else "stream error"
                stream.set_error(msg)
                conn.remove_stream(frame.stream_id)

        elif frame.msg_type == MsgType.HEARTBEAT_DATA:
            # fire-and-forget: 不阻塞主读循环
            self._background(self._handle_heartbeat(conn, frame))

        elif frame.msg_type == MsgType.PING:
            # fire-and-forget: pong 回复不阻塞读循环
            self._background(self._send_pong(conn, frame.payload))

    async def _send_pong(self, conn: TunnelConnection, payload: bytes) -> None:
        """发送 PONG 回复（fire-and-forget，不阻塞主读循环）"""
        try:
            await conn.send_frame(Frame(0, MsgType.PONG, 0, payload))
        except TunnelStreamError:
            pass  # best-effort pong

    async def _handle_heartbeat(self, conn: TunnelConnection, frame: Frame) -> None:
        """处理 proxy 上报的心跳数据，更新 DB，返回 ACK"""
        try:
            data = json.loads(frame.payload) if frame.payload else {}
        except Exception:
            data = {}
        heartbeat_id = normalize_heartbeat_id(data.get("heartbeat_id"))
        ack: dict[str, Any] = {}
        if heartbeat_id is not None:
            ack["heartbeat_id"] = heartbeat_id

        def _sync_heartbeat() -> dict[str, Any]:
            from src.database import create_session
            from src.services.proxy_node.service import ProxyNodeService, build_heartbeat_ack

            db = create_session()
            try:
                node = ProxyNodeService.heartbeat(
                    db,
                    node_id=conn.node_id,
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

        try:
            ack.update(await asyncio.to_thread(_sync_heartbeat))
        except Exception as e:
            logger.warning("tunnel heartbeat DB update failed: {}", e)

        try:
            await conn.send_frame(Frame(0, MsgType.HEARTBEAT_ACK, 0, json.dumps(ack).encode()))
        except TunnelStreamError:
            logger.debug("heartbeat ACK send failed for node_id={}", conn.node_id)


# ---------------------------------------------------------------------------
# 隧道帧压缩 / 解压
# ---------------------------------------------------------------------------


def _compress_frame_payload(data: bytes) -> tuple[bytes, int]:
    """按配置对帧 payload 进行 gzip 压缩。

    Returns:
        (payload, flags) — 若压缩则 flags 含 GZIP_COMPRESSED，否则 flags=0。
    """
    if len(data) >= _TUNNEL_COMPRESS_MIN_SIZE:
        compressed = gzip.compress(data, compresslevel=6)
        # 仅在压缩确实缩小时使用
        if len(compressed) < len(data):
            return compressed, FrameFlags.GZIP_COMPRESSED
    return data, 0


def _decompress_frame_payload(frame: Frame) -> bytes:
    """如果帧设置了 GZIP_COMPRESSED 标志则解压，否则原样返回。"""
    if frame.is_gzip:
        return gzip.decompress(frame.payload)
    return frame.payload


# 全局单例
_tunnel_manager: TunnelManager | None = None


def get_tunnel_manager() -> TunnelManager:
    global _tunnel_manager
    if _tunnel_manager is None:
        _tunnel_manager = TunnelManager()
    return _tunnel_manager
