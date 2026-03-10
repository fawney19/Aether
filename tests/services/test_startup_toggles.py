from __future__ import annotations

import inspect
from types import SimpleNamespace

import pytest

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
