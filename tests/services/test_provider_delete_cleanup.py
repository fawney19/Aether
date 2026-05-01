from __future__ import annotations

from types import SimpleNamespace
from unittest.mock import MagicMock

import pytest

from src.services.provider import delete_cleanup as cleanup_module
from src.services.provider.delete_cleanup import (
    delete_provider_tree,
    prune_allowed_provider_list,
    prune_allowed_provider_refs,
)


def _query_mock(
    *,
    rows: list[object] | None = None,
    update_count: int | None = None,
    delete_count: int | None = None,
) -> MagicMock:
    query = MagicMock()
    filtered = query.filter.return_value
    if rows is not None:
        filtered.all.return_value = rows
    if update_count is not None:
        filtered.update.return_value = update_count
    if delete_count is not None:
        filtered.delete.return_value = delete_count
    return query


def test_prune_allowed_provider_list_removes_target_and_normalizes_empty() -> None:
    next_allowed, changed = prune_allowed_provider_list(["provider-a", "provider-b"], "provider-a")

    assert changed is True
    assert next_allowed == ["provider-b"]

    next_allowed, changed = prune_allowed_provider_list(["provider-a"], "provider-a")

    assert changed is True
    assert next_allowed == []


def test_prune_allowed_provider_refs_updates_matching_records_only() -> None:
    records = [
        SimpleNamespace(allowed_providers=["provider-a", "provider-b"]),
        SimpleNamespace(allowed_providers=["provider-b"]),
        SimpleNamespace(allowed_providers=None),
    ]

    updated = prune_allowed_provider_refs(records, "provider-a")

    assert updated == 1
    assert records[0].allowed_providers == ["provider-b"]
    assert records[1].allowed_providers == ["provider-b"]
    assert records[2].allowed_providers is None


@pytest.mark.asyncio
async def test_cleanup_deleted_provider_references_cleans_large_fanout_tables(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    db = MagicMock()

    api_key_records = [SimpleNamespace(allowed_providers=["provider-a"])]
    key_cleanup_calls: list[list[str]] = []

    monkeypatch.setattr(
        cleanup_module,
        "cleanup_key_references",
        lambda _db, key_ids: key_cleanup_calls.append(list(key_ids)),
    )

    db.query.side_effect = [
        _query_mock(rows=[("endpoint-1",), ("endpoint-2",)]),
        _query_mock(rows=[("key-1",), ("key-2",)]),
        _query_mock(rows=api_key_records),
        _query_mock(update_count=2),
        _query_mock(update_count=3),
        _query_mock(update_count=4),
        _query_mock(update_count=5),
        _query_mock(update_count=6),
        _query_mock(delete_count=7),
        _query_mock(delete_count=8),
    ]

    stats = cleanup_module.cleanup_deleted_provider_references(db, "provider-a")

    assert stats == {
        "api_keys": 1,
        "user_preferences": 2,
        "usage_provider": 3,
        "usage_endpoint": 5,
        "video_tasks_provider": 4,
        "video_tasks_endpoint": 6,
        "request_candidates_provider": 8,
        "request_candidates_endpoint": 7,
    }
    assert api_key_records[0].allowed_providers == []
    assert key_cleanup_calls == [["key-1", "key-2"]]


def test_delete_provider_tree_deletes_children_before_provider(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    db = MagicMock()

    cleanup_mock = MagicMock(
        return_value={
            "api_keys": 2,
            "user_preferences": 3,
            "usage_provider": 4,
            "usage_endpoint": 5,
            "video_tasks_provider": 6,
            "video_tasks_endpoint": 7,
            "request_candidates_provider": 8,
            "request_candidates_endpoint": 9,
        }
    )
    monkeypatch.setattr(cleanup_module, "cleanup_deleted_provider_references", cleanup_mock)

    db.query.side_effect = [
        _query_mock(rows=[("endpoint-1",), ("endpoint-2",)]),
        _query_mock(rows=[("key-1",), ("key-2",), ("key-3",)]),
        _query_mock(delete_count=10),
        _query_mock(delete_count=11),
        _query_mock(delete_count=12),
        _query_mock(delete_count=13),
        _query_mock(delete_count=14),
        _query_mock(delete_count=1),
    ]

    stats = delete_provider_tree(db, "provider-a")

    cleanup_mock.assert_called_once_with(
        db,
        "provider-a",
        endpoint_ids=["endpoint-1", "endpoint-2"],
        key_ids=["key-1", "key-2", "key-3"],
    )
    assert stats == {
        "cleanup": cleanup_mock.return_value,
        "deleted": {
            "api_key_mappings": 10,
            "usage_tracking": 11,
            "models": 12,
            "api_keys": 13,
            "endpoints": 14,
            "providers": 1,
        },
        "key_count": 3,
        "endpoint_count": 2,
    }
