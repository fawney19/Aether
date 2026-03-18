import asyncio

import pytest

from src.clients import redis_client as redis_client_module
from src.clients.redis_client import RedisClientManager, RedisState


def _seed_open_circuit(manager: RedisClientManager) -> None:
    manager._circuit_open_until = 9999999999.0
    manager._consecutive_failures = 5
    manager._last_error = "boom"


def test_reset_redis_circuit_breaker_resets_both_clients() -> None:
    old_global = redis_client_module._redis_manager
    old_usage = redis_client_module._usage_queue_redis_manager
    try:
        global_manager = RedisClientManager(client_name="global")
        usage_manager = RedisClientManager(client_name="usage")
        _seed_open_circuit(global_manager)
        _seed_open_circuit(usage_manager)

        redis_client_module._redis_manager = global_manager
        redis_client_module._usage_queue_redis_manager = usage_manager

        assert redis_client_module.reset_redis_circuit_breaker() is True
        assert global_manager.get_state() == RedisState.NOT_INITIALIZED
        assert usage_manager.get_state() == RedisState.NOT_INITIALIZED
    finally:
        redis_client_module._redis_manager = old_global
        redis_client_module._usage_queue_redis_manager = old_usage


def test_reset_redis_circuit_breaker_returns_false_when_uninitialized() -> None:
    old_global = redis_client_module._redis_manager
    old_usage = redis_client_module._usage_queue_redis_manager
    try:
        redis_client_module._redis_manager = None
        redis_client_module._usage_queue_redis_manager = None

        assert redis_client_module.reset_redis_circuit_breaker() is False
    finally:
        redis_client_module._redis_manager = old_global
        redis_client_module._usage_queue_redis_manager = old_usage


@pytest.mark.asyncio
async def test_get_redis_client_isolated_per_event_loop(monkeypatch: pytest.MonkeyPatch) -> None:
    old_global = redis_client_module._redis_manager
    try:
        redis_client_module._redis_manager = RedisClientManager(client_name="global")

        class DummyRedis:
            def __init__(self, name: str) -> None:
                self.name = name
                self.closed = False

            async def ping(self) -> bool:
                return True

            async def close(self) -> None:
                self.closed = True

        created_clients: list[DummyRedis] = []

        async def fake_from_url(*args: object, **kwargs: object) -> DummyRedis:
            client = DummyRedis(f"client-{len(created_clients)}")
            created_clients.append(client)
            return client

        monkeypatch.setattr(redis_client_module.aioredis, "from_url", fake_from_url)

        main_client = await redis_client_module.get_redis_client()

        def _get_client_from_thread() -> object:
            return asyncio.run(redis_client_module.get_redis_client())

        thread_client = await asyncio.to_thread(_get_client_from_thread)

        assert main_client is created_clients[0]
        assert thread_client is created_clients[1]
        assert thread_client is not main_client
        assert redis_client_module.get_redis_client_sync() is main_client

        await redis_client_module.close_redis_client()

        assert created_clients[0].closed is True
        assert created_clients[1].closed is True
    finally:
        redis_client_module._redis_manager = old_global
