"""测试 handler 基础工具函数"""

import gzip
import json

import pytest

from src.api.handlers.base.utils import (
    build_json_response_for_client,
    build_sse_headers,
    filter_proxy_response_headers,
    resolve_client_accept_encoding,
    resolve_client_content_encoding,
)
from src.core.usage_tokens import extract_cache_creation_tokens, extract_cache_ttl_minutes


class TestExtractCacheCreationTokens:
    """测试 extract_cache_creation_tokens 函数"""

    def test_new_format_only(self) -> None:
        """测试只有新格式字段"""
        usage = {
            "claude_cache_creation_5_m_tokens": 100,
            "claude_cache_creation_1_h_tokens": 200,
        }
        assert extract_cache_creation_tokens(usage) == 300

    def test_new_format_5m_only(self) -> None:
        """测试只有 5 分钟缓存"""
        usage = {
            "claude_cache_creation_5_m_tokens": 150,
            "claude_cache_creation_1_h_tokens": 0,
        }
        assert extract_cache_creation_tokens(usage) == 150

    def test_new_format_1h_only(self) -> None:
        """测试只有 1 小时缓存"""
        usage = {
            "claude_cache_creation_5_m_tokens": 0,
            "claude_cache_creation_1_h_tokens": 250,
        }
        assert extract_cache_creation_tokens(usage) == 250

    def test_old_format_only(self) -> None:
        """测试只有旧格式字段"""
        usage = {
            "cache_creation_input_tokens": 500,
        }
        assert extract_cache_creation_tokens(usage) == 500

    def test_both_formats_prefers_new(self) -> None:
        """测试同时存在时优先使用新格式"""
        usage = {
            "claude_cache_creation_5_m_tokens": 100,
            "claude_cache_creation_1_h_tokens": 200,
            "cache_creation_input_tokens": 999,  # 应该被忽略
        }
        assert extract_cache_creation_tokens(usage) == 300

    def test_empty_usage(self) -> None:
        """测试空字典"""
        usage: dict[str, int] = {}
        assert extract_cache_creation_tokens(usage) == 0

    def test_all_zeros(self) -> None:
        """测试所有字段都为 0"""
        usage = {
            "claude_cache_creation_5_m_tokens": 0,
            "claude_cache_creation_1_h_tokens": 0,
            "cache_creation_input_tokens": 0,
        }
        assert extract_cache_creation_tokens(usage) == 0

    def test_partial_new_format_with_old_format_fallback(self) -> None:
        """测试新格式字段不存在时回退到旧格式"""
        usage = {
            "cache_creation_input_tokens": 123,
        }
        assert extract_cache_creation_tokens(usage) == 123

    def test_new_format_zero_should_not_fallback(self) -> None:
        """测试新格式字段存在但为 0 时，不应 fallback 到旧格式"""
        usage = {
            "claude_cache_creation_5_m_tokens": 0,
            "claude_cache_creation_1_h_tokens": 0,
            "cache_creation_input_tokens": 456,
        }
        # 新格式字段存在，即使值为 0 也应该使用新格式（返回 0）
        # 而不是 fallback 到旧格式（返回 456）
        assert extract_cache_creation_tokens(usage) == 0

    def test_unrelated_fields_ignored(self) -> None:
        """测试忽略无关字段"""
        usage = {
            "input_tokens": 1000,
            "output_tokens": 2000,
            "cache_read_input_tokens": 300,
            "claude_cache_creation_5_m_tokens": 50,
            "claude_cache_creation_1_h_tokens": 75,
        }
        assert extract_cache_creation_tokens(usage) == 125


class TestExtractCacheTTLMinutes:
    def test_nested_5m_cache_returns_5(self) -> None:
        usage = {
            "cache_creation_input_tokens": 32760,
            "cache_creation": {
                "ephemeral_5m_input_tokens": 32760,
                "ephemeral_1h_input_tokens": 0,
            },
        }

        assert extract_cache_ttl_minutes(usage) == 5

    def test_nested_1h_cache_returns_60(self) -> None:
        usage = {
            "cache_creation_input_tokens": 62226,
            "cache_creation": {
                "ephemeral_5m_input_tokens": 0,
                "ephemeral_1h_input_tokens": 62226,
            },
        }

        assert extract_cache_ttl_minutes(usage) == 60

    def test_total_only_cache_creation_has_no_explicit_ttl(self) -> None:
        usage = {"cache_creation_input_tokens": 62226}

        assert extract_cache_ttl_minutes(usage) is None


class TestBuildSSEHeaders:
    def test_default_headers(self) -> None:
        headers = build_sse_headers()
        assert headers["Cache-Control"] == "no-cache, no-transform"
        assert headers["X-Accel-Buffering"] == "no"

    def test_merge_extra_headers(self) -> None:
        headers = build_sse_headers({"X-Test": "1", "Cache-Control": "custom"})
        assert headers["X-Test"] == "1"
        assert headers["Cache-Control"] == "custom"


class TestFilterProxyResponseHeaders:
    def test_none_returns_empty(self) -> None:
        assert filter_proxy_response_headers(None) == {}

    def test_filters_blocklisted_headers_case_insensitive(self) -> None:
        headers = {
            "Content-Length": "123",
            "content-encoding": "gzip",
            "Transfer-Encoding": "chunked",
            "Connection": "keep-alive",
            "Keep-Alive": "timeout=5",
            "Content-Type": "application/json",
            "X-Request-Id": "abc",
            "Anthropic-RateLimit-Requests-Remaining": "10",
        }

        result = filter_proxy_response_headers(headers)

        assert "Content-Length" not in result
        assert "content-encoding" not in result
        assert "Transfer-Encoding" not in result
        assert "Connection" not in result
        assert "Keep-Alive" not in result
        assert "Content-Type" not in result

        assert result["X-Request-Id"] == "abc"
        assert result["Anthropic-RateLimit-Requests-Remaining"] == "10"


class TestResolveClientEncoding:
    def test_content_encoding_prefers_hint(self) -> None:
        headers = {"content-encoding": "gzip"}
        result = resolve_client_content_encoding(headers, hinted_content_encoding="br")
        assert result == "br"

    def test_content_encoding_fallback_to_headers(self) -> None:
        headers = {"Content-Encoding": "gzip"}
        result = resolve_client_content_encoding(headers)
        assert result == "gzip"

    def test_accept_encoding_prefers_hint(self) -> None:
        headers = {"accept-encoding": "gzip"}
        result = resolve_client_accept_encoding(headers, hinted_accept_encoding="br")
        assert result == "br"

    def test_accept_encoding_fallback_to_headers(self) -> None:
        headers = {"Accept-Encoding": "gzip, deflate"}
        result = resolve_client_accept_encoding(headers)
        assert result == "gzip, deflate"


class TestBuildJsonResponseForClient:
    def test_returns_gzip_response_when_client_accepts_gzip(self) -> None:
        response = build_json_response_for_client(
            status_code=200,
            content={"ok": True},
            headers={"content-type": "application/json"},
            client_accept_encoding="gzip, deflate",
        )

        assert response.headers.get("content-encoding") == "gzip"
        assert "accept-encoding" in response.headers.get("vary", "").lower()
        decompressed = gzip.decompress(bytes(response.body))
        assert json.loads(decompressed.decode("utf-8")) == {"ok": True}

    def test_returns_plain_json_when_gzip_not_accepted(self) -> None:
        response = build_json_response_for_client(
            status_code=200,
            content={"ok": True},
            headers={"content-type": "application/json"},
            client_accept_encoding="gzip;q=0, deflate",
        )

        assert response.headers.get("content-encoding") is None
        assert json.loads(bytes(response.body).decode("utf-8")) == {"ok": True}
