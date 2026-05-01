"""Tests for CreateEndpointRequest.validate_base_url / validate_custom_path.

Locks the contract:
- base_url 只接受 scheme+host(+反代前缀)；带业务路径、query、fragment 一律拒绝。
- custom_path 必须以 `/` 开头；允许 `:`、`{`、`}`、`.` 等模板字符。
"""

from __future__ import annotations

import pytest
from pydantic import ValidationError

from src.models.admin_requests import CreateEndpointRequest


def _make(**overrides):
    base = {
        "provider_id": "prov-test",
        "name": "test-ep",
        "base_url": "https://api.example.com",
        "api_format": "openai:chat",
    }
    base.update(overrides)
    return CreateEndpointRequest(**base)


class TestValidateBaseUrl:
    def test_strips_whitespace(self):
        req = _make(base_url="  https://api.example.com  ")
        assert req.base_url == "https://api.example.com"

    def test_strips_trailing_slash(self):
        req = _make(base_url="https://api.example.com/")
        assert req.base_url == "https://api.example.com"

    def test_lowercases_scheme_and_host(self):
        req = _make(base_url="HTTPS://API.example.COM")
        assert req.base_url == "https://api.example.com"

    def test_preserves_path_case(self):
        req = _make(base_url="https://api.example.com/MyPrefix")
        assert req.base_url == "https://api.example.com/MyPrefix"

    def test_rejects_missing_scheme(self):
        with pytest.raises(ValidationError, match="必须以 http"):
            _make(base_url="api.example.com")

    def test_rejects_query_string(self):
        with pytest.raises(ValidationError, match="查询参数"):
            _make(base_url="https://api.example.com/v1?key=secret")

    def test_rejects_fragment(self):
        with pytest.raises(ValidationError, match="片段"):
            _make(base_url="https://api.example.com#section")

    @pytest.mark.parametrize(
        "bad_path",
        [
            "/v1/chat/completions",
            "/v1/messages",
            "/v1/responses",
            "/v1/images/generations",
            "/v1/videos",
            "/v1beta/models/gemini-pro:generateContent",
            "/v1beta/models/gemini-pro:streamGenerateContent",
        ],
    )
    def test_rejects_business_paths(self, bad_path: str):
        with pytest.raises(ValidationError, match="业务路径"):
            _make(base_url=f"https://api.example.com{bad_path}")

    def test_accepts_proxy_prefix(self):
        req = _make(base_url="https://proxy.example.com/openai")
        assert req.base_url == "https://proxy.example.com/openai"

    def test_accepts_with_port(self):
        req = _make(base_url="https://api.example.com:8443/api")
        assert req.base_url == "https://api.example.com:8443/api"


class TestValidateCustomPath:
    def test_none_passes(self):
        req = _make(custom_path=None)
        assert req.custom_path is None

    def test_empty_string_normalized_to_none(self):
        req = _make(custom_path="")
        assert req.custom_path is None

    def test_basic_path(self):
        req = _make(custom_path="/v1/chat/completions")
        assert req.custom_path == "/v1/chat/completions"

    def test_gemini_colon_template(self):
        # 这是关键回归：原正则禁了 ":" 会卡死 Gemini
        req = _make(custom_path="/v1beta/models/{model}:{action}")
        assert req.custom_path == "/v1beta/models/{model}:{action}"

    def test_dot_in_path(self):
        req = _make(custom_path="/v1.0/chat")
        assert req.custom_path == "/v1.0/chat"

    def test_rejects_missing_leading_slash(self):
        with pytest.raises(ValidationError, match="必须以 /"):
            _make(custom_path="v1/chat")

    def test_rejects_query(self):
        with pytest.raises(ValidationError, match="路径只能包含"):
            _make(custom_path="/v1/chat?key=x")

    def test_rejects_whitespace(self):
        with pytest.raises(ValidationError, match="路径只能包含"):
            _make(custom_path="/v1 chat")
