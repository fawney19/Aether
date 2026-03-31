from __future__ import annotations

import asyncio
import inspect
from collections.abc import Callable
from datetime import datetime, timedelta, timezone
from types import SimpleNamespace
from typing import Any, cast
from unittest.mock import AsyncMock, MagicMock

import pytest

import src.main as main_module
import src.services.system.maintenance_scheduler as maintenance_scheduler_module
from src.config.settings import config
from src.services.system.maintenance_scheduler import MaintenanceScheduler


@pytest.mark.asyncio
async def test_maintenance_scheduler_start_skips_startup_task_when_disabled(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(config, "maintenance_startup_tasks_enabled", False)

    scheduler = MaintenanceScheduler()

    created = False

    def fake_create_task(coro):  # type: ignore[no-untyped-def]
        nonlocal created
        created = True
        if inspect.iscoroutine(coro):
            coro.close()
        return object()

    monkeypatch.setattr(maintenance_scheduler_module.asyncio, "create_task", fake_create_task)
    monkeypatch.setattr(scheduler, "_get_checkin_time", lambda: (1, 5))
    monkeypatch.setattr(
        maintenance_scheduler_module,
        "get_scheduler",
        lambda: SimpleNamespace(
            add_cron_job=lambda *args, **kwargs: None,
            add_interval_job=lambda *args, **kwargs: None,
        ),
    )

    await scheduler.start()

    assert created is False


@pytest.mark.asyncio
async def test_maintenance_scheduler_stop_cancels_startup_task_and_removes_jobs(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    scheduler = MaintenanceScheduler()
    scheduler.running = True
    scheduler._startup_task = asyncio.create_task(asyncio.sleep(3600))

    expected_job_ids = [
        "stats_aggregation",
        "stats_hourly_aggregation",
        "wallet_daily_usage_aggregation",
        "usage_cleanup",
        "pool_monitor",
        "http_client_idle_cleanup",
        "pending_cleanup",
        "audit_cleanup",
        "gemini_file_mapping_cleanup",
        "candidate_cleanup",
        "db_maintenance",
        "antigravity_ua_refresh",
        scheduler.CHECKIN_JOB_ID,
    ]
    scheduler._registered_job_ids = list(expected_job_ids)
    removed_jobs: list[str] = []

    monkeypatch.setattr(
        maintenance_scheduler_module,
        "get_scheduler",
        lambda: SimpleNamespace(remove_job=lambda job_id: removed_jobs.append(job_id)),
    )

    await scheduler.stop()

    assert scheduler.running is False
    assert scheduler._startup_task is None
    assert set(removed_jobs) == set(expected_job_ids)
    assert scheduler._registered_job_ids == []


@pytest.mark.asyncio
async def test_run_startup_tasks_starts_historical_rebilling_task(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    scheduler = MaintenanceScheduler()
    created: list[Any] = []

    async def fake_sleep(_seconds: float) -> None:
        return None

    async def fake_pending_cleanup() -> None:
        return None

    def fake_safe_create_task(coro: Any) -> object:
        created.append(coro)
        if inspect.iscoroutine(coro):
            coro.close()
        return object()

    monkeypatch.setattr(maintenance_scheduler_module.asyncio, "sleep", fake_sleep)
    monkeypatch.setattr(scheduler, "_perform_pending_cleanup", fake_pending_cleanup)
    monkeypatch.setattr(
        "src.services.provider.adapters.antigravity.client.refresh_user_agent",
        AsyncMock(),
    )
    monkeypatch.setattr("src.utils.async_utils.safe_create_task", fake_safe_create_task)

    await scheduler._run_startup_tasks()

    assert len(created) == 1
    assert scheduler._historical_rebilling_task is not None


@pytest.mark.asyncio
async def test_stop_service_on_lock_lost_keeps_state_when_stop_fails() -> None:
    state = main_module.LifecycleState()
    service = SimpleNamespace()
    state.quota_scheduler = cast(Any, service)

    async def fail_stop() -> None:
        raise RuntimeError("boom")

    await main_module._stop_service_on_lock_lost(
        state,
        lock_name="quota_scheduler",
        service_name="月卡额度重置调度器",
        state_attr="quota_scheduler",
        stop=fail_stop,
    )

    assert state.quota_scheduler is service


@pytest.mark.asyncio
async def test_stop_service_on_lock_lost_clears_state_after_success() -> None:
    state = main_module.LifecycleState()
    service = SimpleNamespace()
    state.quota_scheduler = cast(Any, service)
    stopped = False

    async def stop() -> None:
        nonlocal stopped
        stopped = True

    await main_module._stop_service_on_lock_lost(
        state,
        lock_name="quota_scheduler",
        service_name="月卡额度重置调度器",
        state_attr="quota_scheduler",
        stop=stop,
    )

    assert stopped is True
    assert state.quota_scheduler is None


def test_http_client_idle_cleanup_interval_env_invalid(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("HTTP_CLIENT_IDLE_CLEANUP_INTERVAL_MINUTES", "bad")
    assert MaintenanceScheduler._get_http_client_idle_cleanup_interval_minutes() == 5


@pytest.mark.asyncio
async def test_candidate_cleanup_uses_dedicated_retention_and_batch_settings(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    scheduler = MaintenanceScheduler()

    class _FakeLoop:
        async def run_in_executor(self, _executor, func):  # type: ignore[no-untyped-def]
            return func()

    class _ConfigSession:
        def close(self) -> None:
            return None

    class _BatchSession:
        def __init__(self, ids: list[str]) -> None:
            self.ids = ids
            self.closed = False
            self.committed = False
            self.query_obj = MagicMock()
            filtered = self.query_obj.filter.return_value
            filtered.order_by.return_value.limit.return_value.all.return_value = [
                SimpleNamespace(id=value) for value in ids
            ]

        def query(self, _model):  # type: ignore[no-untyped-def]
            return self.query_obj

        def execute(self, _statement):  # type: ignore[no-untyped-def]
            return SimpleNamespace(rowcount=len(self.ids))

        def commit(self) -> None:
            self.committed = True

        def rollback(self) -> None:
            raise AssertionError("rollback should not be called")

        def close(self) -> None:
            self.closed = True

    config_session = _ConfigSession()
    batch_one = _BatchSession(["candidate-1", "candidate-2"])
    batch_two = _BatchSession([])
    sessions = iter([config_session, batch_one, batch_two])

    def fake_create_session():  # type: ignore[no-untyped-def]
        return next(sessions)

    config_values = {
        "enable_auto_cleanup": True,
        "request_candidates_retention_days": 21,
        "request_candidates_cleanup_batch_size": 2,
    }

    monkeypatch.setattr(maintenance_scheduler_module, "create_session", fake_create_session)
    monkeypatch.setattr(
        maintenance_scheduler_module.SystemConfigService,
        "get_config",
        lambda _db, key, default=None: config_values.get(key, default),
    )
    monkeypatch.setattr(
        maintenance_scheduler_module.asyncio, "get_running_loop", lambda: _FakeLoop()
    )

    await scheduler._perform_candidate_cleanup()

    batch_one.query_obj.filter.return_value.order_by.return_value.limit.return_value.all.assert_called_once()
    batch_one.query_obj.filter.return_value.order_by.return_value.limit.assert_called_once_with(2)
    assert batch_one.committed is True
    assert batch_one.closed is True
    assert batch_two.closed is True


def test_cleanup_body_fields_batches_records_with_single_commit(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    scheduler = MaintenanceScheduler()

    class _BatchSession:
        def __init__(self, records: list[SimpleNamespace]) -> None:
            self.records = records
            self.closed = False
            self.committed = False
            self.executed = 0
            self.query_obj = MagicMock()
            filtered = self.query_obj.filter.return_value
            filtered.filter.return_value.order_by.return_value.limit.return_value.all.return_value = (
                records
            )

        def query(self, *args):  # type: ignore[no-untyped-def]
            self.query_args = args
            return self.query_obj

        def execute(self, _statement):  # type: ignore[no-untyped-def]
            self.executed += 1
            return SimpleNamespace(rowcount=1)

        def commit(self) -> None:
            self.committed = True

        def rollback(self) -> None:
            raise AssertionError("rollback should not be called")

        def close(self) -> None:
            self.closed = True

    batch_one = _BatchSession(
        [
            SimpleNamespace(
                id="usage-1",
                request_body={"hello": "world"},
                response_body=None,
                provider_request_body=None,
                client_response_body=None,
            ),
            SimpleNamespace(
                id="usage-2",
                request_body=None,
                response_body={"ok": True},
                provider_request_body=None,
                client_response_body=None,
            ),
        ]
    )
    batch_two = _BatchSession([])
    sessions = iter([batch_one, batch_two])

    monkeypatch.setattr(
        maintenance_scheduler_module,
        "create_session",
        lambda: next(sessions),
    )
    monkeypatch.setattr(
        maintenance_scheduler_module,
        "compress_json",
        lambda payload: f"compressed:{payload}".encode(),
    )

    compressed = scheduler._cleanup_body_fields(
        cutoff_time=SimpleNamespace(),  # type: ignore[arg-type]
        batch_size=1000,
    )

    assert compressed == 2
    assert len(batch_one.query_args) == 5
    assert batch_one.executed == 2
    assert batch_one.committed is True
    assert batch_one.closed is True
    assert batch_two.closed is True


def test_cleanup_header_fields_clears_client_response_headers(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    scheduler = MaintenanceScheduler()

    class _BatchSession:
        def __init__(self, ids: list[str]) -> None:
            self.ids = ids
            self.closed = False
            self.committed = False
            self.query_obj = MagicMock()
            self.filtered_by_time = MagicMock()
            self.filtered_by_headers = MagicMock()
            self.query_obj.filter.return_value = self.filtered_by_time
            self.filtered_by_time.filter.return_value = self.filtered_by_headers
            self.filtered_by_headers.order_by.return_value.limit.return_value.all.return_value = [
                SimpleNamespace(id=value) for value in ids
            ]
            self.executed_statements: list[str] = []

        def query(self, *args):  # type: ignore[no-untyped-def]
            self.query_args = args
            return self.query_obj

        def execute(self, statement):  # type: ignore[no-untyped-def]
            self.executed_statements.append(str(statement))
            return SimpleNamespace(rowcount=len(self.ids))

        def commit(self) -> None:
            self.committed = True

        def rollback(self) -> None:
            raise AssertionError("rollback should not be called")

        def close(self) -> None:
            self.closed = True

    batch_one = _BatchSession(["usage-1"])
    batch_two = _BatchSession([])
    sessions = iter([batch_one, batch_two])

    monkeypatch.setattr(
        maintenance_scheduler_module,
        "create_session",
        lambda: next(sessions),
    )

    cleaned = scheduler._cleanup_header_fields(
        cutoff_time=SimpleNamespace(),  # type: ignore[arg-type]
        batch_size=1000,
    )

    header_filter = str(batch_one.filtered_by_time.filter.call_args.args[0])

    assert cleaned == 1
    assert batch_one.query_args == (maintenance_scheduler_module.Usage.id,)
    assert "client_response_headers" in header_filter
    assert "client_response_headers" in batch_one.executed_statements[0]
    assert batch_one.committed is True
    assert batch_one.closed is True
    assert batch_two.closed is True


@pytest.mark.asyncio
async def test_perform_cleanup_deletes_first_and_uses_non_overlapping_windows(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    scheduler = MaintenanceScheduler()
    fixed_now = datetime(2026, 3, 18, 3, 0, 0, tzinfo=timezone.utc)

    class _FakeDateTime(datetime):
        @classmethod
        def now(cls, tz: timezone | None = None) -> datetime:  # type: ignore[override]
            if tz is None:
                return fixed_now.replace(tzinfo=None)
            return fixed_now.astimezone(tz)

    class _FakeLoop:
        async def run_in_executor(self, _executor, func):  # type: ignore[no-untyped-def]
            return func()

    class _ConfigSession:
        def close(self) -> None:
            return None

    calls: list[tuple[str, datetime, int, datetime | None]] = []

    def _record(name: str, count: int) -> Callable[..., int]:
        def _inner(
            cutoff_time: datetime,
            batch_size: int,
            *,
            newer_than: datetime | None = None,
        ) -> int:
            calls.append((name, cutoff_time, batch_size, newer_than))
            return count

        return _inner

    config_values = {
        "enable_auto_cleanup": True,
        "detail_log_retention_days": 7,
        "compressed_log_retention_days": 30,
        "header_retention_days": 90,
        "log_retention_days": 365,
        "cleanup_batch_size": 123,
        "auto_delete_expired_keys": False,
    }

    def _delete_old_records(cutoff_time: datetime, batch_size: int) -> int:
        calls.append(("delete", cutoff_time, batch_size, None))
        return 5

    monkeypatch.setattr(maintenance_scheduler_module, "datetime", _FakeDateTime)
    monkeypatch.setattr(
        maintenance_scheduler_module.asyncio, "get_running_loop", lambda: _FakeLoop()
    )
    monkeypatch.setattr(
        maintenance_scheduler_module,
        "create_session",
        lambda: _ConfigSession(),
    )
    monkeypatch.setattr(
        maintenance_scheduler_module.SystemConfigService,
        "get_config",
        lambda _db, key, default=None: config_values.get(key, default),
    )
    monkeypatch.setattr(
        scheduler,
        "_delete_old_records",
        _delete_old_records,
    )
    monkeypatch.setattr(
        scheduler,
        "_cleanup_header_fields",
        _record("header", 4),
    )
    monkeypatch.setattr(
        scheduler,
        "_cleanup_stale_body_fields",
        _record("body", 3),
    )
    monkeypatch.setattr(
        scheduler,
        "_cleanup_body_fields",
        _record("compress", 2),
    )
    monkeypatch.setattr(
        maintenance_scheduler_module.ApiKeyService,
        "cleanup_expired_keys",
        lambda _db, auto_delete=False: 0,
    )

    await scheduler._perform_cleanup()

    detail_cutoff = fixed_now - timedelta(days=7)
    compressed_cutoff = fixed_now - timedelta(days=30)
    header_cutoff = fixed_now - timedelta(days=90)
    log_cutoff = fixed_now - timedelta(days=365)

    assert calls == [
        ("delete", log_cutoff, 123, None),
        ("header", header_cutoff, 123, log_cutoff),
        ("body", compressed_cutoff, 123, log_cutoff),
        ("compress", detail_cutoff, 123, compressed_cutoff),
    ]
