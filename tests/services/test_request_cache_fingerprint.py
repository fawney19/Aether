from __future__ import annotations

from types import SimpleNamespace
from typing import Any

import pytest

from src.config.settings import config
from src.services.provider.cache_fingerprint import build_request_cache_fingerprint
from src.services.usage._recording_helpers import sanitize_request_metadata
from src.services.usage.service import UsageService
from src.services.usage.telemetry import MessageTelemetry


def _build_openai_cli_body() -> dict[str, Any]:
    return {
        "model": "gpt-5.4",
        "instructions": "You are precise.",
        "input": [{"role": "user", "content": [{"type": "input_text", "text": "hello"}]}],
        "tools": [
            {
                "type": "function",
                "name": "lookup_weather",
                "parameters": {
                    "type": "object",
                    "required": ["city", "country"],
                    "properties": {
                        "country": {"type": "string"},
                        "city": {"type": "string"},
                    },
                },
            }
        ],
        "temperature": 0.2,
        "prompt_cache_key": "pcache-123",
    }


def test_build_request_cache_fingerprint_is_stable_for_dict_key_reordering() -> None:
    body_a = _build_openai_cli_body()
    body_b = {
        "prompt_cache_key": "pcache-123",
        "temperature": 0.2,
        "tools": [
            {
                "parameters": {
                    "properties": {
                        "city": {"type": "string"},
                        "country": {"type": "string"},
                    },
                    "required": ["city", "country"],
                    "type": "object",
                },
                "name": "lookup_weather",
                "type": "function",
            }
        ],
        "input": [{"content": [{"text": "hello", "type": "input_text"}], "role": "user"}],
        "instructions": "You are precise.",
        "model": "gpt-5.4",
    }

    fingerprint_a = build_request_cache_fingerprint(body_a, provider_api_format="openai:cli")
    fingerprint_b = build_request_cache_fingerprint(body_b, provider_api_format="openai:cli")

    assert fingerprint_a is not None
    assert fingerprint_b is not None
    assert fingerprint_a["version"] == 2
    assert fingerprint_a["payload_sha256"] == fingerprint_b["payload_sha256"]
    assert fingerprint_a["cache_relevant_sha256"] == fingerprint_b["cache_relevant_sha256"]
    assert fingerprint_a["field_fingerprints"] == fingerprint_b["field_fingerprints"]
    assert fingerprint_a["prompt_cache_key"] == "pcache-123"
    assert fingerprint_a["cache_relevant_keys"] == [
        "input",
        "instructions",
        "model",
        "prompt_cache_key",
        "tools",
    ]
    assert fingerprint_a["field_fingerprints"]["instructions"]["bytes"] > 0
    assert fingerprint_a["field_fingerprints"]["tools"]["bytes"] > 0


def test_build_request_cache_fingerprint_ignores_non_prompt_fields_in_cache_hash() -> None:
    body_a = _build_openai_cli_body()
    body_b = _build_openai_cli_body()
    body_b["temperature"] = 0.9

    fingerprint_a = build_request_cache_fingerprint(body_a, provider_api_format="openai:cli")
    fingerprint_b = build_request_cache_fingerprint(body_b, provider_api_format="openai:cli")

    assert fingerprint_a is not None
    assert fingerprint_b is not None
    assert fingerprint_a["payload_sha256"] != fingerprint_b["payload_sha256"]
    assert fingerprint_a["cache_relevant_sha256"] == fingerprint_b["cache_relevant_sha256"]
    assert fingerprint_a["field_fingerprints"] == fingerprint_b["field_fingerprints"]
    assert "temperature" not in fingerprint_a["field_fingerprints"]


def test_build_request_cache_fingerprint_tracks_prompt_changes() -> None:
    body_a = _build_openai_cli_body()
    body_b = _build_openai_cli_body()
    body_b["instructions"] = "You are terse."

    fingerprint_a = build_request_cache_fingerprint(body_a, provider_api_format="openai:cli")
    fingerprint_b = build_request_cache_fingerprint(body_b, provider_api_format="openai:cli")

    assert fingerprint_a is not None
    assert fingerprint_b is not None
    assert fingerprint_a["cache_relevant_sha256"] != fingerprint_b["cache_relevant_sha256"]
    assert (
        fingerprint_a["field_fingerprints"]["instructions"]["sha256"]
        != fingerprint_b["field_fingerprints"]["instructions"]["sha256"]
    )
    assert (
        fingerprint_a["field_fingerprints"]["input"]["sha256"]
        == fingerprint_b["field_fingerprints"]["input"]["sha256"]
    )
    assert (
        fingerprint_a["field_fingerprints"]["tools"]["sha256"]
        == fingerprint_b["field_fingerprints"]["tools"]["sha256"]
    )


def test_sanitize_request_metadata_preserves_cache_fingerprint(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(config, "usage_metadata_max_bytes", 120, raising=False)

    metadata = {
        "trace": {"payload": "x" * 400},
        "debug": {"payload": "y" * 400},
        "cache_fingerprint": {
            "payload_sha256": "a" * 64,
            "cache_relevant_sha256": "b" * 64,
        },
    }

    sanitized = sanitize_request_metadata(metadata)

    assert sanitized["_metadata_truncated"] is True
    assert sanitized["cache_fingerprint"]["payload_sha256"] == "a" * 64


@pytest.mark.asyncio
async def test_message_telemetry_record_success_keeps_response_shape_and_adds_fingerprint(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    captured: dict[str, Any] = {}

    async def _fake_record_usage(**kwargs: Any) -> Any:
        captured.update(kwargs)
        return SimpleNamespace(total_cost_usd=0.0, input_tokens=1, output_tokens=2)

    monkeypatch.setattr(UsageService, "record_usage", _fake_record_usage)

    telemetry = MessageTelemetry(
        db=SimpleNamespace(),  # type: ignore[arg-type]
        user=None,
        api_key=None,
        request_id="req-cache-fingerprint",
        client_ip="127.0.0.1",
    )

    await telemetry.record_success(
        provider="openai",
        model="gpt-5.4",
        input_tokens=1,
        output_tokens=2,
        response_time_ms=10,
        status_code=200,
        request_body={"messages": [{"role": "user", "content": "hello"}]},
        request_headers={"user-agent": "codex desktop"},
        response_body={"id": "resp-1"},
        response_headers={"x-test": "1"},
        provider_request_body=_build_openai_cli_body(),
        response_metadata={"model_version": "gpt-5.4-2026-03-01"},
        endpoint_api_format="openai:cli",
    )

    metadata = captured["metadata"]
    assert metadata["model_version"] == "gpt-5.4-2026-03-01"
    assert "response" not in metadata
    assert metadata["cache_fingerprint"]["version"] == 2
    assert metadata["cache_fingerprint"]["provider_api_format"] == "openai:cli"
    assert metadata["cache_fingerprint"]["prompt_cache_key"] == "pcache-123"
    assert "instructions" in metadata["cache_fingerprint"]["field_fingerprints"]


@pytest.mark.asyncio
async def test_message_telemetry_record_failure_keeps_request_metadata_and_adds_fingerprint(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    captured: dict[str, Any] = {}

    async def _fake_record_usage(**kwargs: Any) -> Any:
        captured.update(kwargs)
        return SimpleNamespace()

    monkeypatch.setattr(UsageService, "record_usage", _fake_record_usage)

    telemetry = MessageTelemetry(
        db=SimpleNamespace(),  # type: ignore[arg-type]
        user=None,
        api_key=None,
        request_id="req-cache-fingerprint-fail",
        client_ip="127.0.0.1",
    )

    await telemetry.record_failure(
        provider="openai",
        model="gpt-5.4",
        response_time_ms=10,
        status_code=502,
        error_message="upstream failed",
        request_body={"messages": [{"role": "user", "content": "hello"}]},
        request_headers={"user-agent": "codex desktop"},
        is_stream=False,
        provider_request_body=_build_openai_cli_body(),
        request_metadata={"perf": {"ttfb_ms": 12}},
        endpoint_api_format="openai:cli",
    )

    metadata = captured["metadata"]
    assert metadata["perf"]["ttfb_ms"] == 12
    assert metadata["cache_fingerprint"]["version"] == 2
    assert metadata["cache_fingerprint"]["provider_api_format"] == "openai:cli"
    assert metadata["cache_fingerprint"]["prompt_cache_key"] == "pcache-123"
    assert "input" in metadata["cache_fingerprint"]["field_fingerprints"]
