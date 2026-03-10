from __future__ import annotations

import asyncio

import pytest

import src.services.model.fetch_scheduler as fetch_scheduler_module
from src.services.model.fetch_scheduler import (
    ModelFetchScheduler,
    _aggregate_models_for_cache,
    _run_key_fetch_workers,
)


def test_aggregate_models_for_cache_merges_formats_by_model_id() -> None:
    models: list[dict] = [
        {"id": "gpt-4.1", "api_format": "openai:chat", "label": "GPT 4.1"},
        {"id": "gpt-4.1", "api_format": "openai:cli", "extra": {"tier": "pro"}},
        {"id": "claude-sonnet", "api_format": "claude:chat", "label": "Sonnet"},
        {"id": "", "api_format": "ignored"},
    ]

    aggregated = _aggregate_models_for_cache(models)

    assert len(aggregated) == 2

    by_id = {item["id"]: item for item in aggregated}
    assert by_id["gpt-4.1"]["api_formats"] == ["openai:chat", "openai:cli"]
    assert by_id["gpt-4.1"]["label"] == "GPT 4.1"
    assert by_id["gpt-4.1"]["extra"] == {"tier": "pro"}
    assert "api_format" not in by_id["gpt-4.1"]

    assert by_id["claude-sonnet"]["api_formats"] == ["claude:chat"]


@pytest.mark.asyncio
async def test_run_key_fetch_workers_caps_inflight_tasks() -> None:
    inflight = 0
    max_inflight = 0

    async def fetch_one(key_id: str) -> str:
        nonlocal inflight, max_inflight
        inflight += 1
        max_inflight = max(max_inflight, inflight)
        await asyncio.sleep(0.01)
        inflight -= 1
        return "success"

    timed_out: list[str] = []
    failed: list[tuple[str, str]] = []

    result = await _run_key_fetch_workers(
        [f"key-{index}" for index in range(8)],
        max_concurrent=3,
        timeout_seconds=1.0,
        running_predicate=lambda: True,
        fetch_one=fetch_one,
        on_timeout=timed_out.append,
        on_error=lambda key_id, message: failed.append((key_id, message)),
    )

    assert result == (8, 0, 0)
    assert max_inflight <= 3
    assert timed_out == []
    assert failed == []


@pytest.mark.asyncio
async def test_perform_fetch_all_keys_scans_in_batches(monkeypatch: pytest.MonkeyPatch) -> None:
    scheduler = ModelFetchScheduler()
    scheduler._running = True

    batch_requests: list[str | None] = []
    processed_batches: list[list[str]] = []

    pages = {
        None: ["a", "b"],
        "b": ["c"],
        "c": [],
    }

    def fake_list_batch(*, after_id: str | None = None, limit: int = 0) -> list[str]:
        batch_requests.append(after_id)
        return list(pages.get(after_id, []))

    async def fake_run_key_fetch_workers(
        key_ids: list[str],
        *,
        max_concurrent: int,
        timeout_seconds: float,
        running_predicate,
        fetch_one,
        on_timeout,
        on_error,
    ) -> tuple[int, int, int]:
        processed_batches.append(list(key_ids))
        return len(key_ids), 0, 0

    monkeypatch.setattr(fetch_scheduler_module, "AUTO_FETCH_KEY_BATCH_SIZE", 2)
    monkeypatch.setattr(scheduler, "_list_auto_fetch_key_id_batch", fake_list_batch)
    monkeypatch.setattr(
        fetch_scheduler_module,
        "_run_key_fetch_workers",
        fake_run_key_fetch_workers,
    )

    await scheduler._perform_fetch_all_keys()

    assert batch_requests == [None, "b"]
    assert processed_batches == [["a", "b"], ["c"]]
