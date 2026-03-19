from __future__ import annotations

from types import SimpleNamespace

import httpx
import pytest

from src.services.provider.adapters.kiro.context import (
    KiroRequestContext,
    get_kiro_request_context,
    set_kiro_request_context,
)
from src.services.provider.adapters.kiro.envelope import kiro_envelope
from src.services.provider.adapters.kiro.error_enhancer import (
    build_kiro_network_diagnostic,
    classify_kiro_http_status,
    enhance_kiro_http_error_text,
)
from src.services.provider.adapters.kiro.plugin import build_kiro_url
from src.services.provider.request_context import get_selected_base_url, set_selected_base_url

_REFRESH_TOKEN = "r" * 120


@pytest.fixture(autouse=True)
def _reset_kiro_context() -> None:  # type: ignore[misc]
    set_kiro_request_context(None)
    set_selected_base_url(None)
    yield
    set_kiro_request_context(None)
    set_selected_base_url(None)


def test_kiro_http_status_classification() -> None:
    assert classify_kiro_http_status(200) == "ok"
    assert classify_kiro_http_status(401) == "auth_error"
    assert classify_kiro_http_status(429) == "rate_limited"
    assert classify_kiro_http_status(503) == "upstream_server_error"


def test_kiro_envelope_records_http_status() -> None:
    set_kiro_request_context(KiroRequestContext(region="us-east-1", machine_id="mid-1"))

    kiro_envelope.on_http_status(base_url="https://q.us-east-1.amazonaws.com", status_code=429)

    ctx = get_kiro_request_context()
    assert ctx is not None
    assert ctx.last_http_status == 429
    assert ctx.last_http_error_category == "rate_limited"


def test_kiro_envelope_records_connection_error_summary() -> None:
    set_kiro_request_context(KiroRequestContext(region="us-east-1", machine_id="mid-1"))

    kiro_envelope.on_connection_error(
        base_url="https://q.us-east-1.amazonaws.com",
        exc=httpx.ConnectTimeout("dial timed out"),
    )

    ctx = get_kiro_request_context()
    assert ctx is not None
    assert ctx.last_connection_error_category == "connect_timeout"
    assert ctx.last_connection_error_summary is not None
    assert "ConnectTimeout" in ctx.last_connection_error_summary


def test_kiro_envelope_capture_selected_base_url() -> None:
    set_selected_base_url("https://q.us-west-2.amazonaws.com")
    assert kiro_envelope.capture_selected_base_url() == "https://q.us-west-2.amazonaws.com"


def test_build_kiro_url_sets_selected_base_url_and_applies_region() -> None:
    set_kiro_request_context(KiroRequestContext(region="eu-west-1", machine_id="mid-2"))
    endpoint = SimpleNamespace(base_url="https://q.{region}.amazonaws.com")

    url = build_kiro_url(
        endpoint,
        is_stream=True,
        effective_query_params={"alt": "sse"},
    )

    assert url.startswith("https://q.eu-west-1.amazonaws.com/generateAssistantResponse")
    assert get_selected_base_url() == "https://q.eu-west-1.amazonaws.com"


def test_build_kiro_network_diagnostic_prefers_connection_summary() -> None:
    diag = build_kiro_network_diagnostic(
        http_status=503,
        http_category="upstream_server_error",
        connection_summary="connect_timeout: ConnectTimeout: dial timed out",
    )
    assert diag == "network=connect_timeout: ConnectTimeout: dial timed out"


def test_kiro_envelope_wrap_request_adds_inference_config() -> None:
    wrapped, _ = kiro_envelope.wrap_request(
        {
            "model": "claude-sonnet-4-5",
            "max_tokens": 2048,
            "temperature": 0.3,
            "top_p": 0.8,
            "messages": [{"role": "user", "content": "hello"}],
        },
        model="claude-sonnet-4-5",
        url_model=None,
        decrypted_auth_config={
            "auth_method": "social",
            "refreshToken": _REFRESH_TOKEN,
            "profileArn": "arn:aws:iam::1:role/x",
        },
    )

    assert wrapped["conversationState"]["currentMessage"]["userInputMessage"]["modelId"] == (
        "claude-sonnet-4-5"
    )
    assert wrapped["inferenceConfig"] == {
        "maxTokens": 2048,
        "temperature": 0.3,
        "topP": 0.8,
    }
    assert wrapped["profileArn"] == "arn:aws:iam::1:role/x"


def test_kiro_envelope_omits_profile_arn_for_idc_auth() -> None:
    wrapped, _ = kiro_envelope.wrap_request(
        {
            "messages": [{"role": "user", "content": "hello"}],
        },
        model="claude-sonnet-4-5",
        url_model=None,
        decrypted_auth_config={
            "auth_method": "identity_center",
            "refreshToken": _REFRESH_TOKEN,
            "profileArn": "arn:aws:iam::1:role/x",
            "clientId": "cid",
            "clientSecret": "secret",
        },
    )

    assert "profileArn" not in wrapped


def test_enhance_kiro_http_error_text_maps_known_reason() -> None:
    text = enhance_kiro_http_error_text(
        '{"message":"Input is too long.","reason":"CONTENT_LENGTH_EXCEEDS_THRESHOLD"}',
        status_code=400,
    )

    assert "[CONTENT_LENGTH_EXCEEDS_THRESHOLD]" in text
    assert "输入超过模型上下文限制" in text
