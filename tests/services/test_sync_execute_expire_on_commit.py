from __future__ import annotations

from types import SimpleNamespace
from unittest.mock import AsyncMock, MagicMock

import pytest

from src.services.task.core.schema import ExecutionResult
from src.services.task.execute.sync_execute import SyncTaskExecutionService


@pytest.mark.asyncio
async def test_execute_sync_unified_temporarily_disables_expire_on_commit(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    db = MagicMock()
    db.expire_on_commit = True
    db.query.return_value.filter.return_value.first.return_value = SimpleNamespace(username="alice")

    pool_ops = MagicMock()
    pool_ops.apply_pool_reorder = AsyncMock(return_value=([], []))

    service = SyncTaskExecutionService(
        db,
        None,
        recorder=MagicMock(),
        pool_ops=pool_ops,
        error_ops=MagicMock(),
        failure_ops=MagicMock(),
    )

    cache_scheduler = SimpleNamespace(_ensure_initialized=AsyncMock())

    class _StubCandidateResolver:
        def __init__(self, db: object, cache_scheduler: object) -> None:
            self.db = db
            self.cache_scheduler = cache_scheduler

        async def fetch_candidates(self, **kwargs: object) -> tuple[list[object], str]:
            return [], "gpt-4.1"

        async def create_candidate_records_async(
            self, **kwargs: object
        ) -> dict[tuple[int, int], str]:
            return {}

        def count_total_attempts(self, _all_candidates: list[object]) -> int:
            return 0

    class _StubFailoverEngine:
        def __init__(self, db: object, **kwargs: object) -> None:
            self.db = db

        async def execute(self, **kwargs: object) -> ExecutionResult:
            assert getattr(self.db, "expire_on_commit") is False
            return ExecutionResult(success=True)

    monkeypatch.setattr(
        "src.services.task.execute.sync_execute.SystemConfigService.get_config",
        lambda _db, _key, default=None: default,
    )
    monkeypatch.setattr(
        "src.services.task.execute.sync_execute.get_cache_aware_scheduler",
        AsyncMock(return_value=cache_scheduler),
    )
    monkeypatch.setattr(
        "src.services.task.execute.sync_execute.CandidateResolver",
        _StubCandidateResolver,
    )
    monkeypatch.setattr(
        "src.services.task.execute.sync_execute.ErrorClassifier",
        lambda **kwargs: MagicMock(),
    )
    monkeypatch.setattr(
        "src.services.task.execute.sync_execute.RequestDispatcher",
        lambda **kwargs: MagicMock(),
    )
    monkeypatch.setattr(
        "src.services.task.execute.sync_execute.FailoverEngine",
        _StubFailoverEngine,
    )
    monkeypatch.setattr(
        "src.services.task.execute.sync_execute.UsageService.create_pending_usage",
        lambda **kwargs: None,
    )
    monkeypatch.setattr(
        "src.services.scheduling.utils.release_db_connection_before_await",
        lambda _db: None,
    )
    monkeypatch.setattr(
        "src.services.rate_limit.concurrency_manager.get_concurrency_manager",
        AsyncMock(return_value=MagicMock()),
    )
    monkeypatch.setattr(
        "src.services.rate_limit.adaptive_rpm.get_adaptive_rpm_manager",
        lambda: MagicMock(),
    )
    monkeypatch.setattr(
        "src.services.request.executor.RequestExecutor",
        lambda **kwargs: MagicMock(),
    )

    result = await service.execute_sync_unified(
        api_format="openai_chat",
        model_name="gpt-4.1",
        user_api_key=SimpleNamespace(id="key-1", user_id="user-1", name="default-key"),
        request_func=AsyncMock(),
        request_id="req-1",
        is_stream=True,
        capability_requirements=None,
        preferred_key_ids=None,
        request_body_ref=None,
        request_headers=None,
        request_body=None,
    )

    assert result.success is True
    assert db.expire_on_commit is True
