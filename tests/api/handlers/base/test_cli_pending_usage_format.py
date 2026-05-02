from __future__ import annotations

import json
from types import SimpleNamespace

import pytest

import src.api.handlers.base.cli_sync_mixin as cli_sync_mod
from src.api.handlers.base.cli_stream_mixin import CliStreamMixin
from src.api.handlers.base.cli_sync_mixin import CliSyncMixin


class _StopExecution(Exception):
    pass


class _DummySyncHandler(CliSyncMixin):
    FORMAT_ID = "openai:cli"

    def __init__(self) -> None:
        self.allowed_api_formats = ["openai:compact"]
        self.primary_api_format = "openai:compact"
        self.pending_calls: list[dict[str, object]] = []

    def extract_model_from_request(
        self, request_body: dict[str, object], path_params: dict[str, object] | None
    ) -> str:
        return str(request_body.get("model") or "unknown")

    def _create_pending_usage(self, **kwargs: object) -> bool:
        self.pending_calls.append(kwargs)
        raise _StopExecution()


class _DummyParser:
    def extract_usage_from_response(self, response: dict[str, object]) -> dict[str, int]:
        usage = response.get("usage")
        return usage if isinstance(usage, dict) else {}

    def extract_text_content(self, response: dict[str, object]) -> str:
        content = response.get("content")
        if not isinstance(content, list):
            return ""
        return "".join(
            item.get("text", "") for item in content if isinstance(item, dict)
        )


class _DummyTelemetry:
    def __init__(self) -> None:
        self.success_kwargs: dict[str, object] | None = None

    async def record_success(self, **kwargs: object) -> float:
        self.success_kwargs = kwargs
        return 0.0

    async def record_failure(self, **kwargs: object) -> None:
        raise AssertionError(f"unexpected failure telemetry: {kwargs}")


class _AlreadyConvertedSyncHandler(CliSyncMixin):
    FORMAT_ID = "claude:cli"

    def __init__(self) -> None:
        self.allowed_api_formats = ["claude:cli"]
        self.primary_api_format = "claude:cli"
        self.api_family = "claude"
        self.endpoint_kind = "cli"
        self.api_key = object()
        self.db = None
        self.redis = None
        self.request_id = "req_force_stream"
        self.parser = _DummyParser()
        self.telemetry = _DummyTelemetry()

    def extract_model_from_request(
        self, request_body: dict[str, object], path_params: dict[str, object] | None
    ) -> str:
        return str(request_body.get("model") or "unknown")

    def _create_pending_usage(self, **kwargs: object) -> bool:
        return True

    async def _get_mapped_model(self, **kwargs: object) -> str | None:
        return None

    def apply_mapped_model(self, request_body: dict[str, object], mapped_model: str) -> dict:
        return {**request_body, "model": mapped_model}

    async def _build_upstream_request(self, **kwargs: object) -> SimpleNamespace:
        return SimpleNamespace(
            headers={},
            payload=kwargs["request_body"],
            url="https://upstream.test/responses",
            envelope=None,
            upstream_is_stream=True,
            tls_profile=None,
            selected_base_url="https://upstream.test",
            url_model=kwargs.get("mapped_model") or kwargs.get("fallback_model"),
        )

    def _extract_response_metadata(self, response: dict[str, object]) -> dict[str, object]:
        return {}

    def _resolve_capability_requirements(self, **kwargs: object) -> None:
        return None

    async def _resolve_preferred_key_ids(self, **kwargs: object) -> None:
        return None

    def _build_request_metadata(self) -> dict[str, object]:
        return {}

    def _merge_scheduling_metadata(
        self, request_metadata: dict[str, object] | None, **kwargs: object
    ) -> dict[str, object]:
        return request_metadata or {}


class _DummyStreamHandler(CliStreamMixin):
    FORMAT_ID = "openai:cli"

    def __init__(self) -> None:
        self.allowed_api_formats = ["openai:compact"]
        self.primary_api_format = "openai:compact"
        self.pending_calls: list[dict[str, object]] = []

    def extract_model_from_request(
        self, request_body: dict[str, object], path_params: dict[str, object] | None
    ) -> str:
        return str(request_body.get("model") or "unknown")

    def _create_pending_usage(self, **kwargs: object) -> bool:
        self.pending_calls.append(kwargs)
        raise _StopExecution()


@pytest.mark.asyncio
async def test_sync_pending_usage_uses_primary_api_format() -> None:
    handler = _DummySyncHandler()

    with pytest.raises(_StopExecution):
        await handler.process_sync(  # type: ignore[misc]
            original_request_body={"model": "gpt-5.3-codex"},
            original_headers={},
        )

    assert handler.pending_calls
    assert handler.pending_calls[0]["api_format"] == "openai:compact"


@pytest.mark.asyncio
async def test_sync_forced_stream_response_skips_second_format_conversion(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    handler = _AlreadyConvertedSyncHandler()
    client_response = {
        "id": "msg_force_stream",
        "type": "message",
        "role": "assistant",
        "model": "claude-opus-4-7",
        "content": [
            {
                "type": "tool_use",
                "id": "call_123",
                "name": "Read",
                "input": {"file_path": "README.md"},
            }
        ],
        "stop_reason": "tool_use",
        "usage": {"input_tokens": 10, "output_tokens": 4},
    }

    class _FakeRegistry:
        def __init__(self) -> None:
            self.convert_response_calls = 0

        def get_normalizer(self, api_format: str) -> object:
            assert api_format == "claude:cli"

            class _Normalizer:
                def response_from_internal(
                    self, internal_response: object, **kwargs: object
                ) -> dict:
                    return client_response

            return _Normalizer()

        def convert_response(self, *args: object, **kwargs: object) -> dict[str, object]:
            self.convert_response_calls += 1
            return {
                "id": "msg_double_converted",
                "type": "message",
                "role": "assistant",
                "model": "claude-opus-4-7",
                "content": [],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 10, "output_tokens": 4},
            }

    class _FakeStreamResponse:
        status_code = 200
        headers = {"content-type": "text/event-stream"}

        async def __aenter__(self) -> "_FakeStreamResponse":
            return self

        async def __aexit__(self, *args: object) -> None:
            return None

        def raise_for_status(self) -> None:
            return None

        async def aiter_bytes(self):
            yield b"data: {}\n"

    class _FakeHTTPClient:
        def stream(self, **kwargs: object) -> _FakeStreamResponse:
            return _FakeStreamResponse()

    registry = _FakeRegistry()

    async def _aggregate_stream(*args: object, **kwargs: object) -> object:
        return object()

    async def _get_upstream_client(*args: object, **kwargs: object) -> _FakeHTTPClient:
        return _FakeHTTPClient()

    async def _resolve_proxy_info_async(*args: object, **kwargs: object) -> dict[str, object]:
        return {}

    async def _resolve_delegate_config_async(*args: object, **kwargs: object) -> None:
        return None

    async def _build_stream_kwargs_async(*args: object, **kwargs: object) -> dict[str, object]:
        return {}

    class _FakeTaskService:
        def __init__(self, *args: object, **kwargs: object) -> None:
            pass

        async def execute(self, **kwargs: object) -> SimpleNamespace:
            request_func = kwargs["request_func"]
            response = await request_func(
                SimpleNamespace(id="provider_id", name="main", proxy=None, request_timeout=None),
                SimpleNamespace(id="endpoint_id", api_format="openai:cli"),
                SimpleNamespace(id="key_id", api_key="sk-test", proxy=None),
                SimpleNamespace(
                    mapping_matched_model="gpt-5.5",
                    needs_conversion=True,
                    output_limit=None,
                ),
            )
            return SimpleNamespace(
                response=response,
                provider_name="main",
                request_candidate_id="attempt_id",
                provider_id="provider_id",
                endpoint_id="endpoint_id",
                key_id="key_id",
            )

    monkeypatch.setattr(cli_sync_mod, "get_format_converter_registry", lambda: registry)
    monkeypatch.setattr(
        cli_sync_mod,
        "aggregate_upstream_stream_to_internal_response",
        _aggregate_stream,
    )
    monkeypatch.setattr(
        "src.clients.http_client.HTTPClientPool.get_upstream_client",
        _get_upstream_client,
    )
    monkeypatch.setattr(
        "src.services.proxy_node.resolver.resolve_effective_proxy",
        lambda *args, **kwargs: None,
    )
    monkeypatch.setattr(
        "src.services.proxy_node.resolver.resolve_proxy_info_async",
        _resolve_proxy_info_async,
    )
    monkeypatch.setattr(
        "src.services.proxy_node.resolver.get_proxy_label",
        lambda *args, **kwargs: "direct",
    )
    monkeypatch.setattr(
        "src.services.proxy_node.resolver.resolve_delegate_config_async",
        _resolve_delegate_config_async,
    )
    monkeypatch.setattr(
        "src.services.proxy_node.resolver.build_stream_kwargs_async",
        _build_stream_kwargs_async,
    )
    monkeypatch.setattr("src.services.task.TaskService", _FakeTaskService)

    response = await handler.process_sync(
        original_request_body={"model": "claude-opus-4-7"},
        original_headers={},
    )

    body = json.loads(response.body)
    assert registry.convert_response_calls == 0
    assert body["content"] == client_response["content"]
    assert body["stop_reason"] == "tool_use"
    assert handler.telemetry.success_kwargs is not None
    assert handler.telemetry.success_kwargs["response_body"] == client_response
    assert handler.telemetry.success_kwargs["client_response_body"] is None


@pytest.mark.asyncio
async def test_stream_pending_usage_uses_primary_api_format() -> None:
    handler = _DummyStreamHandler()

    with pytest.raises(_StopExecution):
        await handler.process_stream(  # type: ignore[misc]
            original_request_body={"model": "gpt-5.3-codex"},
            original_headers={},
        )

    assert handler.pending_calls
    assert handler.pending_calls[0]["api_format"] == "openai:compact"
