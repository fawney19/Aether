"""测试 handler 基础工具函数"""

import pytest

from src.api.handlers.base.utils import build_sse_headers, extract_cache_creation_tokens


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
        usage = {}
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


class TestBuildSSEHeaders:
    def test_default_headers(self) -> None:
        headers = build_sse_headers()
        assert headers["Cache-Control"] == "no-cache, no-transform"
        assert headers["X-Accel-Buffering"] == "no"

    def test_merge_extra_headers(self) -> None:
        headers = build_sse_headers({"X-Test": "1", "Cache-Control": "custom"})
        assert headers["X-Test"] == "1"
        assert headers["Cache-Control"] == "custom"
