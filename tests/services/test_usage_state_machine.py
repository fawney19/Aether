from types import SimpleNamespace
from unittest.mock import MagicMock

import pytest
from fastapi import HTTPException

from src.services.task.video.cancel import VideoTaskCancelService
from src.services.usage.service import UsageService


class _DummyQuery:
    def __init__(self, obj):
        self._obj = obj

    def filter(self, *args, **kwargs):
        return self

    def with_for_update(self):
        return self

    def first(self):
        return self._obj


def test_update_settled_billing_rejects_terminal_usage() -> None:
    usage = SimpleNamespace(
        request_id="req-1",
        billing_status="settled",
        finalized_at="2026-03-06T00:00:00+00:00",
        total_cost_usd=1.23,
    )
    db = MagicMock()
    db.query.return_value = _DummyQuery(usage)

    updated = UsageService.update_settled_billing(
        db,
        request_id="req-1",
        total_cost_usd=2.34,
        status="completed",
    )

    assert updated is False
    assert usage.total_cost_usd == 1.23


def test_update_usage_status_skips_terminal_usage() -> None:
    usage = SimpleNamespace(
        request_id="req-1",
        billing_status="settled",
        finalized_at="2026-03-06T00:00:00+00:00",
        status="completed",
        error_message=None,
        provider_name="demo",
    )
    db = MagicMock()
    db.query.return_value = _DummyQuery(usage)

    result = UsageService.update_usage_status(
        db=db,
        request_id="req-1",
        status="failed",
        error_message="boom",
    )

    assert result is usage
    assert usage.status == "completed"
    assert usage.error_message is None
    db.commit.assert_not_called()


@pytest.mark.asyncio
async def test_task_cancel_rejects_terminal_task() -> None:
    task = SimpleNamespace(
        id="t1",
        user_id="u1",
        status="completed",
        request_id="req-1",
    )
    svc = VideoTaskCancelService(MagicMock())

    with pytest.raises(HTTPException) as exc:
        await svc.cancel_task(task=task, task_id="t1")

    assert exc.value.status_code == 409
