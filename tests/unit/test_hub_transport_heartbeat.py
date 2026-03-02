from __future__ import annotations

import json
from types import SimpleNamespace
from typing import Any

import pytest

from src.services.proxy_node.hub_config import HubConfig
from src.services.proxy_node.hub_transport import HubConnectionManager
from src.services.proxy_node.tunnel_protocol import Frame, MsgType


class _StubRedis:
    def __init__(self, *, set_result: bool = True, set_exc: Exception | None = None) -> None:
        self.set_result = set_result
        self.set_exc = set_exc
        self.calls: list[tuple[str, str, int | None, bool | None]] = []

    async def set(
        self,
        key: str,
        value: str,
        ex: int | None = None,
        nx: bool | None = None,
    ) -> bool:
        self.calls.append((key, value, ex, nx))
        if self.set_exc is not None:
            raise self.set_exc
        return self.set_result


def _build_manager() -> HubConnectionManager:
    return HubConnectionManager(
        HubConfig(
            enabled=True,
            url="ws://127.0.0.1:8085",
            connect_timeout_seconds=1.0,
            ping_interval_seconds=1.0,
            send_timeout_seconds=1.0,
            max_streams=16,
            max_frame_size=1024 * 1024,
        )
    )


def _heartbeat_frame(payload: dict[str, Any]) -> Frame:
    return Frame(0, MsgType.HEARTBEAT_DATA, 0, json.dumps(payload).encode("utf-8"))


def _decode_ack(frame: Frame) -> dict[str, Any]:
    assert frame.msg_type == MsgType.HEARTBEAT_ACK
    if not frame.payload:
        return {}
    return json.loads(frame.payload.decode("utf-8"))


@pytest.mark.asyncio
async def test_handle_heartbeat_normalizes_id_and_updates_db(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager = _build_manager()
    captured_frames: list[Frame] = []
    heartbeat_calls: list[dict[str, Any]] = []

    async def _fake_send_frame(frame: Frame) -> None:
        captured_frames.append(frame)

    class _FakeSession:
        def close(self) -> None:
            return

    async def _fake_get_redis_client(*, require_redis: bool = False) -> _StubRedis:
        _ = require_redis
        return redis

    def _fake_heartbeat(db: Any, **kwargs: Any) -> Any:
        heartbeat_calls.append(kwargs)
        return SimpleNamespace(
            remote_config={"heartbeat_interval": 8, "upgrade_to": "0.2.3"},
            config_version=5,
        )

    redis = _StubRedis(set_result=True)
    monkeypatch.setattr(manager, "_send_frame", _fake_send_frame)
    monkeypatch.setattr("src.database.create_session", lambda: _FakeSession())
    monkeypatch.setattr("src.clients.get_redis_client", _fake_get_redis_client)
    monkeypatch.setattr(
        "src.services.proxy_node.service.ProxyNodeService.heartbeat", _fake_heartbeat
    )

    await manager._handle_heartbeat(
        _heartbeat_frame(
            {
                "node_id": "node-1",
                "heartbeat_session_id": "sess-1",
                "heartbeat_id": 15.0,
                "active_connections": 3,
                "total_requests": 10,
                "failed_requests": 1,
                "dns_failures": 2,
                "stream_errors": 0,
                "proxy_metadata": {"version": "0.2.1"},
            }
        )
    )

    assert len(redis.calls) == 1
    assert redis.calls[0][0] == "hub:heartbeat:node-1:sess-1:15"
    assert heartbeat_calls and heartbeat_calls[0]["node_id"] == "node-1"
    assert heartbeat_calls[0]["proxy_metadata"] == {"version": "0.2.1"}
    assert len(captured_frames) == 1

    ack = _decode_ack(captured_frames[0])
    assert ack["heartbeat_id"] == 15
    assert isinstance(ack["heartbeat_id"], int)
    assert ack["remote_config"] == {"heartbeat_interval": 8, "upgrade_to": "0.2.3"}
    assert ack["config_version"] == 5
    assert ack["upgrade_to"] == "0.2.3"


@pytest.mark.asyncio
async def test_handle_heartbeat_duplicate_skips_db_update(monkeypatch: pytest.MonkeyPatch) -> None:
    manager = _build_manager()
    captured_frames: list[Frame] = []
    heartbeat_called = False

    async def _fake_send_frame(frame: Frame) -> None:
        captured_frames.append(frame)

    async def _fake_get_redis_client(*, require_redis: bool = False) -> _StubRedis:
        _ = require_redis
        return redis

    def _fake_heartbeat(db: Any, **kwargs: Any) -> Any:
        _ = db, kwargs
        nonlocal heartbeat_called
        heartbeat_called = True
        return SimpleNamespace(remote_config={"heartbeat_interval": 8}, config_version=5)

    redis = _StubRedis(set_result=False)
    monkeypatch.setattr(manager, "_send_frame", _fake_send_frame)
    monkeypatch.setattr("src.clients.get_redis_client", _fake_get_redis_client)
    monkeypatch.setattr(
        "src.services.proxy_node.service.ProxyNodeService.heartbeat", _fake_heartbeat
    )

    await manager._handle_heartbeat(
        _heartbeat_frame({"node_id": "node-1", "heartbeat_id": 77, "total_requests": 20})
    )

    assert len(redis.calls) == 1
    assert heartbeat_called is False
    assert len(captured_frames) == 1
    ack = _decode_ack(captured_frames[0])
    assert ack == {"heartbeat_id": 77}


@pytest.mark.asyncio
async def test_handle_heartbeat_redis_error_falls_back_to_db_update(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager = _build_manager()
    captured_frames: list[Frame] = []
    heartbeat_calls: list[dict[str, Any]] = []

    async def _fake_send_frame(frame: Frame) -> None:
        captured_frames.append(frame)

    class _FakeSession:
        def close(self) -> None:
            return

    async def _fake_get_redis_client(*, require_redis: bool = False) -> _StubRedis:
        _ = require_redis
        return redis

    def _fake_heartbeat(db: Any, **kwargs: Any) -> Any:
        heartbeat_calls.append(kwargs)
        return SimpleNamespace(remote_config=None, config_version=0)

    redis = _StubRedis(set_exc=RuntimeError("redis unavailable"))
    monkeypatch.setattr(manager, "_send_frame", _fake_send_frame)
    monkeypatch.setattr("src.database.create_session", lambda: _FakeSession())
    monkeypatch.setattr("src.clients.get_redis_client", _fake_get_redis_client)
    monkeypatch.setattr(
        "src.services.proxy_node.service.ProxyNodeService.heartbeat", _fake_heartbeat
    )

    await manager._handle_heartbeat(
        _heartbeat_frame(
            {
                "node_id": "node-1",
                "heartbeat_id": 99,
                "total_requests": 1,
                "proxy_metadata": {"version": "0.2.2"},
            }
        )
    )

    assert heartbeat_calls and heartbeat_calls[0]["total_requests"] == 1
    assert heartbeat_calls[0]["proxy_metadata"] == {"version": "0.2.2"}
    assert len(captured_frames) == 1
    ack = _decode_ack(captured_frames[0])
    assert ack == {"heartbeat_id": 99}
