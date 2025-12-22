"""测试 handler 基础工具函数"""

import pytest

from src.api.handlers.base.utils import build_sse_headers, extract_cache_creation_tokens


class TestExtractCacheCreationTokens:
    """测试 extract_cache_creation_tokens 函数"""

    # === 嵌套格式测试（优先级最高）===

    def test_nested_cache_creation_format(self) -> None:
        """测试嵌套格式正常情况"""
        usage = {
            "cache_creation": {
                "ephemeral_5m_input_tokens": 456,
                "ephemeral_1h_input_tokens": 100,
            }
        }
        assert extract_cache_creation_tokens(usage) == 556

    def test_nested_cache_creation_with_old_format_fallback(self) -> None:
        """测试嵌套格式为 0 时回退到旧格式"""
        usage = {
            "cache_creation": {
                "ephemeral_5m_input_tokens": 0,
                "ephemeral_1h_input_tokens": 0,
            },
            "cache_creation_input_tokens": 549,
        }
        assert extract_cache_creation_tokens(usage) == 549

    def test_nested_has_priority_over_flat(self) -> None:
        """测试嵌套格式优先于扁平格式"""
        usage = {
            "cache_creation": {
                "ephemeral_5m_input_tokens": 100,
                "ephemeral_1h_input_tokens": 200,
            },
            "claude_cache_creation_5_m_tokens": 999,  # 应该被忽略
            "claude_cache_creation_1_h_tokens": 888,  # 应该被忽略
            "cache_creation_input_tokens": 777,  # 应该被忽略
        }
        assert extract_cache_creation_tokens(usage) == 300

    # === 扁平格式测试（优先级第二）===

    def test_flat_new_format_still_works(self) -> None:
        """测试扁平新格式兼容性"""
        usage = {
            "claude_cache_creation_5_m_tokens": 100,
            "claude_cache_creation_1_h_tokens": 200,
        }
        assert extract_cache_creation_tokens(usage) == 300

    def test_flat_new_format_with_old_format_fallback(self) -> None:
        """测试扁平格式为 0 时回退到旧格式"""
        usage = {
            "claude_cache_creation_5_m_tokens": 0,
            "claude_cache_creation_1_h_tokens": 0,
            "cache_creation_input_tokens": 549,
        }
        assert extract_cache_creation_tokens(usage) == 549

    def test_flat_new_format_5m_only(self) -> None:
        """测试只有 5 分钟扁平缓存"""
        usage = {
            "claude_cache_creation_5_m_tokens": 150,
            "claude_cache_creation_1_h_tokens": 0,
        }
        assert extract_cache_creation_tokens(usage) == 150

    def test_flat_new_format_1h_only(self) -> None:
        """测试只有 1 小时扁平缓存"""
        usage = {
            "claude_cache_creation_5_m_tokens": 0,
            "claude_cache_creation_1_h_tokens": 250,
        }
        assert extract_cache_creation_tokens(usage) == 250

    # === 旧格式测试（优先级第三）===

    def test_old_format_only(self) -> None:
        """测试只有旧格式"""
        usage = {
            "cache_creation_input_tokens": 549,
        }
        assert extract_cache_creation_tokens(usage) == 549

    # === 边界情况测试 ===

    def test_no_cache_creation_tokens(self) -> None:
        """测试没有任何缓存字段"""
        usage = {}
        assert extract_cache_creation_tokens(usage) == 0

    def test_all_formats_zero(self) -> None:
        """测试所有格式都为 0"""
        usage = {
            "cache_creation": {
                "ephemeral_5m_input_tokens": 0,
                "ephemeral_1h_input_tokens": 0,
            },
            "claude_cache_creation_5_m_tokens": 0,
            "claude_cache_creation_1_h_tokens": 0,
            "cache_creation_input_tokens": 0,
        }
        assert extract_cache_creation_tokens(usage) == 0

    def test_unrelated_fields_ignored(self) -> None:
        """测试忽略无关字段"""
        usage = {
            "input_tokens": 1000,
            "output_tokens": 2000,
            "cache_read_input_tokens": 300,
            "cache_creation": {
                "ephemeral_5m_input_tokens": 50,
                "ephemeral_1h_input_tokens": 75,
            },
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
