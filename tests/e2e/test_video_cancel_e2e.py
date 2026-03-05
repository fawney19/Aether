from __future__ import annotations

from types import SimpleNamespace
from unittest.mock import AsyncMock, MagicMock, patch

import httpx
import pytest

from src.api.base.pipeline import ApiRequestPipeline
from src.api.handlers.gemini.video_adapter import GeminiVeoAdapter
from src.api.handlers.openai.video_adapter import OpenAIVideoAdapter
from src.core.api_format.conversion.internal_video import VideoStatus


def _make_request(
    *,
    method: str,
    path: str,
    headers: dict[str, str],
    body: bytes,
) -> MagicMock:
    req = MagicMock()
    req.method = method
    req.url = SimpleNamespace(path=path)
    req.headers = headers
    req.query_params = {}
    req.client = None
    req.state = SimpleNamespace()
    req.body = AsyncMock(return_value=body)
    return req


@pytest.mark.asyncio
async def test_video_cancel_openai_route_end_to_end(monkeypatch: pytest.MonkeyPatch) -> None:
    """
    End-to-end-ish test:
    ApiRequestPipeline -> OpenAIVideoAdapter -> OpenAIVideoHandler -> TaskService.cancel
    """
    pipeline = ApiRequestPipeline()

    # Pipeline auth/balance/audit shortcuts
    user = SimpleNamespace(id="u1", username="u1", role="user")
    api_key = SimpleNamespace(id="ak1", user_id="u1", is_standalone=False)
    monkeypatch.setattr(
        pipeline.auth_service, "authenticate_api_key", lambda _db, _k: (user, api_key)
    )
    monkeypatch.setattr(
        pipeline.usage_service, "check_request_balance", lambda *_args, **_kwargs: (True, "ok")
    )
    monkeypatch.setattr(pipeline, "_calculate_balance_remaining", lambda *_args, **_kwargs: None)
    monkeypatch.setattr(pipeline.audit_service, "log_event", MagicMock())

    # DB stubs used by TaskService.cancel
    task = SimpleNamespace(
        id="t1",
        short_id="s1",
        user_id="u1",
        request_id="r1",
        external_task_id="ext-1",
        endpoint_id="e1",
        key_id="k1",
        status=VideoStatus.SUBMITTED.value,
        updated_at=None,
        request_metadata={},
    )
    endpoint = SimpleNamespace(
        id="e1",
        base_url="https://upstream.example.com",
        api_format="openai:video",
        api_family="openai",
        endpoint_kind="video",
        header_rules=None,
    )
    provider_key = SimpleNamespace(
        id="k1",
        api_key="encrypted",
        auth_type="api_key",
    )

    q_task = MagicMock()
    q_task.filter.return_value.first.return_value = task
    q_endpoint = MagicMock()
    q_endpoint.filter.return_value.first.return_value = endpoint
    q_key = MagicMock()
    q_key.filter.return_value.first.return_value = provider_key

    db = MagicMock()

    def _query(model: type) -> MagicMock:
        name = getattr(model, "__name__", "")
        if name == "VideoTask":
            return q_task
        if name == "ProviderEndpoint":
            return q_endpoint
        if name == "ProviderAPIKey":
            return q_key
        return MagicMock()

    db.query.side_effect = _query

    # Upstream call stubs
    upstream = SimpleNamespace(
        delete=AsyncMock(return_value=httpx.Response(200, json={"ok": True})),
        post=AsyncMock(),  # not used for openai cancel
    )

    with (
        patch(
            "src.clients.http_client.HTTPClientPool.get_default_client_async",
            AsyncMock(return_value=upstream),
        ),
        patch("src.core.crypto.crypto_service.decrypt", lambda _v: "upstream-key"),
        patch(
            "src.services.provider.transport.build_provider_url",
            lambda _endpoint, **_kwargs: "https://upstream.example.com/v1/videos",
        ),
        patch(
            "src.services.usage.service.UsageService.finalize_void", MagicMock(return_value=True)
        ),
        patch("src.services.usage.service.UsageService.void_settled", MagicMock()),
    ):
        request = _make_request(
            method="POST",
            path="/v1/videos/t1/cancel",
            headers={
                "authorization": "Bearer sk-test",
                "x-real-ip": "127.0.0.1",
                "user-agent": "pytest",
            },
            body=b"",
        )
        adapter = OpenAIVideoAdapter()
        resp = await pipeline.run(
            adapter=adapter,
            http_request=request,
            db=db,
            mode=adapter.mode,
            api_format_hint=adapter.allowed_api_formats[0],
            path_params={"task_id": "t1"},
        )

    assert getattr(resp, "status_code", None) == 200
    assert upstream.delete.await_count == 1
    assert upstream.delete.call_args.args[0] == "https://upstream.example.com/v1/videos/ext-1"
    assert task.status == VideoStatus.CANCELLED.value


@pytest.mark.asyncio
async def test_video_cancel_gemini_route_end_to_end(monkeypatch: pytest.MonkeyPatch) -> None:
    """
    End-to-end-ish test:
    ApiRequestPipeline -> GeminiVeoAdapter -> GeminiVeoHandler -> TaskService.cancel
    """
    pipeline = ApiRequestPipeline()

    # Pipeline auth/balance/audit shortcuts
    user = SimpleNamespace(id="u1", username="u1", role="user")
    api_key = SimpleNamespace(id="ak1", user_id="u1", is_standalone=False)
    monkeypatch.setattr(
        pipeline.auth_service, "authenticate_api_key", lambda _db, _k: (user, api_key)
    )
    monkeypatch.setattr(
        pipeline.usage_service, "check_request_balance", lambda *_args, **_kwargs: (True, "ok")
    )
    monkeypatch.setattr(pipeline, "_calculate_balance_remaining", lambda *_args, **_kwargs: None)
    monkeypatch.setattr(pipeline.audit_service, "log_event", MagicMock())

    # DB stubs used by TaskService.cancel
    task = SimpleNamespace(
        id="t1",
        short_id="op123",
        user_id="u1",
        request_id="r1",
        external_task_id="op123",
        endpoint_id="e1",
        key_id="k1",
        status=VideoStatus.SUBMITTED.value,
        updated_at=None,
        request_metadata={},
    )
    endpoint = SimpleNamespace(
        id="e1",
        base_url="https://generativelanguage.googleapis.com",
        api_format="gemini:video",
        api_family="gemini",
        endpoint_kind="video",
        header_rules=None,
    )
    provider_key = SimpleNamespace(
        id="k1",
        api_key="encrypted",
        auth_type="api_key",
    )

    q_task_id = MagicMock()
    q_task_id.filter.return_value.first.return_value = None
    q_task_short = MagicMock()
    q_task_short.filter.return_value.first.return_value = task
    q_endpoint = MagicMock()
    q_endpoint.filter.return_value.first.return_value = endpoint
    q_key = MagicMock()
    q_key.filter.return_value.first.return_value = provider_key

    db = MagicMock()
    _video_query_count = {"n": 0}

    def _query(model: type) -> MagicMock:
        name = getattr(model, "__name__", "")
        if name == "VideoTask":
            _video_query_count["n"] += 1
            return q_task_id if _video_query_count["n"] == 1 else q_task_short
        if name == "ProviderEndpoint":
            return q_endpoint
        if name == "ProviderAPIKey":
            return q_key
        return MagicMock()

    db.query.side_effect = _query

    upstream = SimpleNamespace(
        post=AsyncMock(return_value=httpx.Response(200, json={"done": True})),
        delete=AsyncMock(),  # not used for gemini cancel
    )

    with (
        patch(
            "src.clients.http_client.HTTPClientPool.get_default_client_async",
            AsyncMock(return_value=upstream),
        ),
        patch("src.core.crypto.crypto_service.decrypt", lambda _v: "upstream-key"),
        patch(
            "src.api.handlers.base.request_builder.get_provider_auth", AsyncMock(return_value=None)
        ),
        patch(
            "src.services.usage.service.UsageService.finalize_void", MagicMock(return_value=True)
        ),
        patch("src.services.usage.service.UsageService.void_settled", MagicMock()),
    ):
        request = _make_request(
            method="POST",
            path="/v1beta/operations/op123:cancel",
            headers={"x-goog-api-key": "sk-test", "x-real-ip": "127.0.0.1", "user-agent": "pytest"},
            body=b"",
        )
        adapter = GeminiVeoAdapter()
        resp = await pipeline.run(
            adapter=adapter,
            http_request=request,
            db=db,
            mode=adapter.mode,
            api_format_hint=adapter.allowed_api_formats[0],
            path_params={"task_id": "op123", "action": "cancel"},
        )

    assert getattr(resp, "status_code", None) == 200
    assert upstream.post.await_count == 1
    assert upstream.post.call_args.args[0].endswith("/v1beta/operations/op123:cancel")
    assert task.status == VideoStatus.CANCELLED.value
