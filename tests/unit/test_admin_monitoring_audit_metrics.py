from __future__ import annotations

from types import SimpleNamespace

import pytest

from src.api.admin.monitoring import audit


def test_build_cpu_metric_without_psutil(monkeypatch: object) -> None:
    monkeypatch.setattr(audit, "psutil", None)
    monkeypatch.setattr(audit.os, "cpu_count", lambda: 8)
    monkeypatch.setattr(audit.os, "getloadavg", lambda: (1.6, 0.0, 0.0), raising=False)

    metric = audit._build_cpu_metric()

    assert metric["status"] == "ok"
    assert metric["usage_percent"] is None
    assert metric["load_percent"] == 20.0
    assert metric["core_count"] == 8
    assert metric["message"] == "psutil is not installed"


def test_build_memory_metric_without_psutil(monkeypatch: object) -> None:
    monkeypatch.setattr(audit, "psutil", None)

    metric = audit._build_memory_metric()

    assert metric["status"] == "unknown"
    assert metric["used_percent"] is None
    assert metric["used_bytes"] is None
    assert metric["available_bytes"] is None
    assert metric["total_bytes"] is None
    assert metric["message"] == "psutil is not installed"


def test_get_manual_capacity_bytes_ignores_non_positive_values(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(audit.SystemConfigService, "get_config", lambda *_args, **_kwargs: 0)

    assert audit._get_manual_capacity_bytes(object(), "redis_memory_total_bytes") is None


def test_get_manual_capacity_bytes_returns_positive_int(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(audit.SystemConfigService, "get_config", lambda *_args, **_kwargs: "2048")

    assert audit._get_manual_capacity_bytes(object(), "redis_memory_total_bytes") == 2048


class _RedisStub:
    def __init__(self, info_payload: dict[str, object]) -> None:
        self.info_payload = info_payload
        self.sections: list[str] = []
        self.ping_called = False

    async def ping(self) -> bool:
        self.ping_called = True
        return True

    async def info(self, *, section: str) -> dict[str, object]:
        self.sections.append(section)
        return self.info_payload


class _ResultStub:
    def __init__(self, row: dict[str, object]) -> None:
        self.row = row

    def mappings(self) -> _ResultStub:
        return self

    def one(self) -> dict[str, object]:
        return self.row


class _SessionStub:
    def __init__(self, *, dialect_name: str, row: dict[str, object] | None = None) -> None:
        self.bind = SimpleNamespace(dialect=SimpleNamespace(name=dialect_name))
        self.row = row
        self.executed_sql: list[str] = []

    def get_bind(self) -> object:
        return self.bind

    def execute(self, statement: object) -> _ResultStub:
        self.executed_sql.append(str(statement))
        if self.row is None:
            raise AssertionError("execute should not be called without a row stub")
        return _ResultStub(self.row)


@pytest.mark.asyncio
async def test_build_redis_metric_uses_server_reported_info(monkeypatch: pytest.MonkeyPatch) -> None:
    redis = _RedisStub(
        {
            "used_memory": 3072,
            "used_memory_peak": 3584,
            "maxmemory": 4096,
            "total_system_memory": 8192,
        }
    )

    async def _fake_get_redis_client(*, require_redis: bool = False) -> _RedisStub:
        assert require_redis is False
        return redis

    monkeypatch.setattr(audit, "get_redis_client", _fake_get_redis_client)
    monkeypatch.setattr(audit, "_get_manual_capacity_bytes", lambda *_args, **_kwargs: None)

    metric = await audit._build_redis_metric(object())

    assert redis.ping_called is True
    assert redis.sections == ["memory"]
    assert metric["status"] == "ok"
    assert metric["memory_status"] == "warning"
    assert metric["used_memory_bytes"] == 3072
    assert metric["peak_memory_bytes"] == 3584
    assert metric["maxmemory_bytes"] == 4096
    assert metric["memory_ceiling_bytes"] == 4096
    assert metric["memory_source"] == "maxmemory"
    assert metric["available_memory_bytes"] == 1024
    assert metric["memory_percent"] == 75.0
    assert metric["message"] is None


@pytest.mark.asyncio
async def test_build_redis_metric_prefers_manual_total_memory(monkeypatch: pytest.MonkeyPatch) -> None:
    redis = _RedisStub(
        {
            "used_memory": 3072,
            "used_memory_peak": 3584,
            "maxmemory": 4096,
            "total_system_memory": 8192,
        }
    )

    async def _fake_get_redis_client(*, require_redis: bool = False) -> _RedisStub:
        assert require_redis is False
        return redis

    monkeypatch.setattr(audit, "get_redis_client", _fake_get_redis_client)
    monkeypatch.setattr(audit, "_get_manual_capacity_bytes", lambda *_args, **_kwargs: 8192)

    metric = await audit._build_redis_metric(object())

    assert metric["memory_ceiling_bytes"] == 8192
    assert metric["memory_source"] == "configured"
    assert metric["available_memory_bytes"] == 5120
    assert metric["memory_percent"] == 37.5


def test_build_postgres_metric_uses_native_sql_stats(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(
        audit,
        "get_pool_status",
        lambda: {
            "checked_out": 4,
            "pool_size": 8,
            "overflow": 1,
            "max_capacity": 20,
            "pool_timeout": 30,
        },
    )
    monkeypatch.setattr(audit, "_get_manual_capacity_bytes", lambda *_args, **_kwargs: None)
    db = _SessionStub(
        dialect_name="postgresql",
        row={
            "database_size_bytes": 123456789,
            "server_max_connections": 200,
            "server_connections": 25,
        },
    )

    metric = audit._build_postgres_metric(db)

    assert db.executed_sql
    assert "pg_database_size(current_database())" in db.executed_sql[0]
    assert "pg_stat_database" in db.executed_sql[0]
    assert metric["status"] == "ok"
    assert metric["usage_percent"] == 12.5
    assert metric["pool_usage_percent"] == 20.0
    assert metric["checked_out"] == 4
    assert metric["server_connections"] == 25
    assert metric["server_max_connections"] == 200
    assert metric["server_usage_percent"] == 12.5
    assert metric["database_size_bytes"] == 123456789
    assert metric["storage_status"] == "unknown"
    assert metric["storage_message"] == "请在系统设置中填写 PostgreSQL 总空间"
    assert metric["message"] is None


def test_build_postgres_metric_falls_back_without_local_storage_assumptions(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(
        audit,
        "get_pool_status",
        lambda: {
            "checked_out": 2,
            "pool_size": 5,
            "overflow": 0,
            "max_capacity": 10,
            "pool_timeout": 15,
        },
    )
    monkeypatch.setattr(audit, "_get_manual_capacity_bytes", lambda *_args, **_kwargs: None)

    class _FailingSession(_SessionStub):
        def execute(self, statement: object) -> _ResultStub:
            self.executed_sql.append(str(statement))
            raise RuntimeError("permission denied for pg_stat_database")

    db = _FailingSession(dialect_name="postgresql", row=None)

    metric = audit._build_postgres_metric(db)

    assert db.executed_sql
    assert metric["status"] == "ok"
    assert metric["usage_percent"] == 20.0
    assert metric["pool_usage_percent"] == 20.0
    assert metric["server_connections"] is None
    assert metric["server_max_connections"] is None
    assert metric["database_size_bytes"] is None
    assert metric["storage_message"] == "请在系统设置中填写 PostgreSQL 总空间"
    assert "pg_stat_database" in (metric["message"] or "")


def test_build_postgres_metric_skips_storage_probe_for_non_postgres(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(
        audit,
        "get_pool_status",
        lambda: {
            "checked_out": 3,
            "pool_size": 6,
            "overflow": 0,
            "max_capacity": 12,
            "pool_timeout": 20,
        },
    )
    monkeypatch.setattr(audit, "_get_manual_capacity_bytes", lambda *_args, **_kwargs: None)
    db = _SessionStub(dialect_name="sqlite", row=None)

    metric = audit._build_postgres_metric(db)

    assert db.executed_sql == []
    assert metric["status"] == "ok"
    assert metric["usage_percent"] == 25.0
    assert metric["pool_usage_percent"] == 25.0
    assert metric["storage_status"] == "unknown"
    assert metric["storage_message"] == "当前数据库不是 PostgreSQL"
    assert metric["message"] is None


def test_build_postgres_metric_uses_manual_total_storage(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(
        audit,
        "get_pool_status",
        lambda: {
            "checked_out": 2,
            "pool_size": 6,
            "overflow": 0,
            "max_capacity": 12,
            "pool_timeout": 20,
        },
    )
    monkeypatch.setattr(
        audit,
        "_get_manual_capacity_bytes",
        lambda _db, key: 200_000_000 if key == "postgres_storage_total_bytes" else None,
    )
    db = _SessionStub(
        dialect_name="postgresql",
        row={
            "database_size_bytes": 50_000_000,
            "server_max_connections": 200,
            "server_connections": 20,
        },
    )

    metric = audit._build_postgres_metric(db)

    assert metric["storage_total_bytes"] == 200_000_000
    assert metric["storage_free_bytes"] == 150_000_000
    assert metric["storage_free_percent"] == 75.0
    assert metric["storage_status"] == "ok"
    assert metric["storage_message"] is None
