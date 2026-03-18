from __future__ import annotations

from types import SimpleNamespace
from unittest.mock import AsyncMock, MagicMock

import pytest

from src.core.exceptions import EmbeddedErrorException
from src.services.candidate.schema import CandidateKey
from src.services.candidate.submit import SubmitOutcome
from src.services.task.core.context import TaskMode
from src.services.task.core.protocol import AttemptKind
from src.services.task.request_state import MutableRequestBodyState
from src.services.task.service import TaskService, pool_on_error


@pytest.mark.asyncio
async def test_task_service_execute_async_requires_extract_external_task_id() -> None:
    svc = TaskService(MagicMock())
    with pytest.raises(ValueError):
        await svc.execute(
            task_type="video",
            task_mode=TaskMode.ASYNC,
            api_format="openai:video",
            model_name="m",
            user_api_key=MagicMock(id="u", user_id="user"),
            request_func=AsyncMock(),
            request_id="rid",
        )


@pytest.mark.asyncio
async def test_task_service_execute_async_returns_execution_result() -> None:
    db = MagicMock()
    svc = TaskService(db)

    candidate = SimpleNamespace(
        provider=SimpleNamespace(id="p1", name="prov"),
        endpoint=SimpleNamespace(id="e1"),
        key=SimpleNamespace(id="k1"),
    )
    outcome = SubmitOutcome(
        candidate=candidate,  # type: ignore[arg-type]
        candidate_keys=[{"index": 0, "provider_id": "p1"}],
        external_task_id="task_123",
        rule_lookup=None,
        upstream_payload={"id": "x"},
        upstream_headers={"x-test": "1"},
        upstream_status_code=200,
    )

    svc.submit_with_failover = AsyncMock(return_value=outcome)  # type: ignore[method-assign]
    svc._execute_facade_ops._get_candidate_keys = MagicMock(  # type: ignore[attr-defined, method-assign]
        return_value=[
            CandidateKey(candidate_index=0, retry_index=0, status="success", provider_id="p1")
        ]
    )

    result = await svc.execute(
        task_type="video",
        task_mode=TaskMode.ASYNC,
        api_format="openai:video",
        model_name="m",
        user_api_key=MagicMock(id="u", user_id="user"),
        request_func=AsyncMock(),
        request_id="rid",
        extract_external_task_id=MagicMock(),
        allow_format_conversion=True,
    )

    assert result.success is True
    assert result.attempt_result is not None
    assert result.attempt_result.kind == AttemptKind.ASYNC_SUBMIT
    assert result.provider_task_id == "task_123"
    assert result.provider_id == "p1"
    assert result.candidate_index == 0


@pytest.mark.asyncio
async def test_task_service_execute_sync_passes_request_headers_and_body() -> None:
    db = MagicMock()
    svc = TaskService(db)
    sentinel_result = object()
    svc._sync_ops.execute_sync_unified = AsyncMock(  # type: ignore[attr-defined, method-assign]
        return_value=sentinel_result
    )

    request_headers = {"authorization": "Bearer test", "x-trace-id": "abc123"}
    request_body = {"model": "gpt-4o-mini", "messages": [{"role": "user", "content": "hi"}]}
    request_body_state = MutableRequestBodyState(request_body)

    result = await svc.execute(
        task_type="chat",
        task_mode=TaskMode.SYNC,
        api_format="openai:chat",
        model_name="gpt-4o-mini",
        user_api_key=MagicMock(id="u", user_id="user"),
        request_func=AsyncMock(),
        request_id="rid-sync",
        is_stream=True,
        request_headers=request_headers,
        request_body=request_body,
        request_body_state=request_body_state,
    )

    assert result is sentinel_result
    svc._sync_ops.execute_sync_unified.assert_awaited_once()  # type: ignore[attr-defined]
    kwargs = svc._sync_ops.execute_sync_unified.await_args.kwargs  # type: ignore[attr-defined, union-attr]
    assert kwargs["request_headers"] == request_headers
    assert kwargs["request_body"] == request_body
    assert kwargs["request_body_state"] is request_body_state


@pytest.mark.asyncio
async def test_task_service_execute_delegates_to_execute_facade_ops() -> None:
    svc = TaskService(MagicMock())
    sentinel = object()
    svc._execute_facade_ops.execute = AsyncMock(return_value=sentinel)  # type: ignore[attr-defined, method-assign]

    result = await svc.execute(
        task_type="chat",
        task_mode=TaskMode.SYNC,
        api_format="openai:chat",
        model_name="m",
        user_api_key=MagicMock(id="u", user_id="user"),
        request_func=AsyncMock(),
        request_id="rid",
    )

    assert result is sentinel
    svc._execute_facade_ops.execute.assert_awaited_once()  # type: ignore[attr-defined]


@pytest.mark.asyncio
async def test_task_service_submit_with_failover_delegates_to_submit_facade_ops() -> None:
    svc = TaskService(MagicMock())
    sentinel = object()
    svc._submit_facade_ops.submit_with_failover = AsyncMock(  # type: ignore[attr-defined, method-assign]
        return_value=sentinel
    )

    result = await svc.submit_with_failover(
        api_format="openai:video",
        model_name="sora",
        affinity_key="a1",
        user_api_key=MagicMock(id="u", user_id="user"),
        request_id="rid",
        task_type="video",
        submit_func=AsyncMock(),
        extract_external_task_id=lambda payload: payload.get("id"),
    )

    assert result is sentinel
    svc._submit_facade_ops.submit_with_failover.assert_awaited_once()  # type: ignore[attr-defined]


@pytest.mark.asyncio
async def test_task_service_poll_delegates_to_video_facade_ops() -> None:
    svc = TaskService(MagicMock())
    sentinel = object()
    svc._video_facade_ops.poll = AsyncMock(return_value=sentinel)  # type: ignore[attr-defined, method-assign]

    result = await svc.poll("task-1", user_id="user-1")

    assert result is sentinel
    svc._video_facade_ops.poll.assert_awaited_once_with("task-1", user_id="user-1")  # type: ignore[attr-defined]


@pytest.mark.asyncio
async def test_task_service_cancel_delegates_to_video_facade_ops() -> None:
    svc = TaskService(MagicMock())
    sentinel = object()
    svc._video_facade_ops.cancel = AsyncMock(return_value=sentinel)  # type: ignore[attr-defined, method-assign]

    result = await svc.cancel(
        "task-1",
        user_id="user-1",
        original_headers={"x-test": "1"},
    )

    assert result is sentinel
    svc._video_facade_ops.cancel.assert_awaited_once_with(  # type: ignore[attr-defined]
        "task-1",
        user_id="user-1",
        original_headers={"x-test": "1"},
    )


@pytest.mark.asyncio
async def test_pool_on_error_uses_embedded_error_message_fallback(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    parsed_pool_cfg = object()
    apply_health_policy = AsyncMock()
    monkeypatch.setattr(
        "src.services.provider.pool.config.parse_pool_config", lambda _cfg: parsed_pool_cfg
    )
    monkeypatch.setattr(
        "src.services.provider.pool.health_policy.apply_health_policy",
        apply_health_policy,
    )

    provider = SimpleNamespace(id="p1", config={})
    key = SimpleNamespace(id="k1")
    cause = EmbeddedErrorException(
        provider_name="prov",
        error_code=429,
        error_message="usage_limit_reached",
    )

    await pool_on_error(provider, key, 429, cause)

    apply_health_policy.assert_awaited_once()
    kwargs = apply_health_policy.await_args.kwargs
    assert kwargs["provider_id"] == "p1"
    assert kwargs["key_id"] == "k1"
    assert kwargs["status_code"] == 429
    assert kwargs["error_body"] == "usage_limit_reached"
    assert kwargs["response_headers"] == {}
    assert kwargs["config"] is parsed_pool_cfg
