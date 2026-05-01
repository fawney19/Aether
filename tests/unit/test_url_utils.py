"""Tests for src/utils/url_utils.py:join_url.

Lock the contract: only strip whitespace, normalize boundary slashes, and
concatenate. No version sniffing, no path-segment dedup.
"""

from __future__ import annotations

import pytest

from src.utils.url_utils import is_official_openai_api_url, join_url


class TestJoinUrl:
    def test_basic_concat(self):
        assert join_url("https://api.example.com", "/v1/chat/completions") == (
            "https://api.example.com/v1/chat/completions"
        )

    def test_strips_trailing_slashes_from_base(self):
        assert join_url("https://api.example.com/", "/v1/chat/completions") == (
            "https://api.example.com/v1/chat/completions"
        )
        assert join_url("https://api.example.com//", "/v1/chat/completions") == (
            "https://api.example.com/v1/chat/completions"
        )

    def test_adds_leading_slash_to_path(self):
        assert join_url("https://api.example.com", "v1/chat/completions") == (
            "https://api.example.com/v1/chat/completions"
        )

    def test_strips_whitespace(self):
        assert join_url("  https://api.example.com  ", "  /v1/chat/completions  ") == (
            "https://api.example.com/v1/chat/completions"
        )

    def test_empty_path_returns_base_only(self):
        assert join_url("https://api.example.com/", "") == "https://api.example.com"
        assert join_url("https://api.example.com/", None) == "https://api.example.com"

    def test_none_base_returns_path_only(self):
        assert join_url(None, "/v1/chat") == "/v1/chat"
        assert join_url("", "/v1/chat") == "/v1/chat"

    def test_proxy_prefix_in_base_is_preserved(self):
        # base 中间或末尾的反代前缀完全尊重用户输入，不做去重
        assert join_url("https://proxy.example.com/api", "/v1/chat/completions") == (
            "https://proxy.example.com/api/v1/chat/completions"
        )

    def test_full_path_in_base_does_not_dedupe(self):
        # 用户把完整路径塞进 base_url 的"坏输入"——按规则直接拼，不去重
        # 这是预期行为：让用户立刻发现并改配置
        assert join_url("https://api.openai.com/v1", "/v1/chat/completions") == (
            "https://api.openai.com/v1/v1/chat/completions"
        )
        assert join_url("https://api.openai.com/v1/chat/completions", "/v1/chat/completions") == (
            "https://api.openai.com/v1/chat/completions/v1/chat/completions"
        )

    def test_query_in_path_passes_through_unchanged(self):
        # 部分调用方会自己拼 query，join_url 不解析
        assert join_url("https://api.example.com", "/v1beta/models?key=abc") == (
            "https://api.example.com/v1beta/models?key=abc"
        )

    def test_gemini_path_with_colons(self):
        assert join_url(
            "https://generativelanguage.googleapis.com",
            "/v1beta/models/gemini-pro:generateContent",
        ) == (
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-pro:generateContent"
        )


class TestIsOfficialOpenAIApiUrl:
    def test_official(self):
        assert is_official_openai_api_url("https://api.openai.com")
        assert is_official_openai_api_url("https://api.openai.com/v1")

    def test_non_official(self):
        assert not is_official_openai_api_url("https://api.deepseek.com")
        assert not is_official_openai_api_url("")
        assert not is_official_openai_api_url(None)


@pytest.mark.parametrize(
    "base,path,expected",
    [
        ("https://a.com", "/p", "https://a.com/p"),
        ("https://a.com/", "/p", "https://a.com/p"),
        ("https://a.com/x", "/p", "https://a.com/x/p"),
        ("https://a.com/x/", "/p", "https://a.com/x/p"),
        ("https://a.com", "p", "https://a.com/p"),
        ("https://a.com/x", "p", "https://a.com/x/p"),
    ],
)
def test_join_url_table(base: str, path: str, expected: str) -> None:
    assert join_url(base, path) == expected
