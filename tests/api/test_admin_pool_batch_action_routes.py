from __future__ import annotations

from types import SimpleNamespace
from unittest.mock import AsyncMock, MagicMock

import pytest

from src.api.admin.pool.routes import AdminBatchActionKeysAdapter
from src.api.admin.pool.schemas import BatchActionRequest


def _build_context(db: MagicMock) -> SimpleNamespace:
    return SimpleNamespace(
        db=db,
        user=SimpleNamespace(username="admin-1"),
        add_audit_metadata=lambda **_: None,
    )


def _mock_provider_lookup(db: MagicMock, provider_id: str) -> None:
    db.query.return_value.filter.return_value.first.return_value = SimpleNamespace(id=provider_id)


@pytest.mark.asyncio
async def test_batch_delete_submits_async_task(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """Delete action now submits an async batch-delete task instead of
    executing SQL synchronously."""
    db = MagicMock()
    provider_id = "provider-1"
    _mock_provider_lookup(db, provider_id)

    mock_submit = AsyncMock(return_value="task-abc-123")
    monkeypatch.setattr(
        "src.services.provider_keys.batch_delete_task.submit_batch_delete",
        mock_submit,
    )

    key_ids = [f"key-{idx}" for idx in range(1200)]
    adapter = AdminBatchActionKeysAdapter(
        provider_id=provider_id,
        body=BatchActionRequest(
            key_ids=key_ids,
            action="delete",
        ),
    )

    result = await adapter.handle(_build_context(db))

    # Async task returns affected=0 and a task_id
    assert result.affected == 0
    assert result.task_id == "task-abc-123"
    assert "1200" in result.message
    mock_submit.assert_awaited_once_with(provider_id, list(dict.fromkeys(key_ids)))


@pytest.mark.asyncio
async def test_batch_delete_deduplicates_key_ids(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """Duplicate key IDs should be deduplicated before submission."""
    db = MagicMock()
    provider_id = "provider-2"
    _mock_provider_lookup(db, provider_id)

    mock_submit = AsyncMock(return_value="task-xyz-456")
    monkeypatch.setattr(
        "src.services.provider_keys.batch_delete_task.submit_batch_delete",
        mock_submit,
    )

    adapter = AdminBatchActionKeysAdapter(
        provider_id=provider_id,
        body=BatchActionRequest(
            key_ids=["key-1", "key-2", "key-1", "key-3", "key-2"],
            action="delete",
        ),
    )

    result = await adapter.handle(_build_context(db))

    assert result.affected == 0
    assert result.task_id == "task-xyz-456"
    # Deduplicated: 3 unique keys
    submitted_ids = mock_submit.call_args[0][1]
    assert len(submitted_ids) == 3
    assert submitted_ids == ["key-1", "key-2", "key-3"]
