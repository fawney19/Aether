from __future__ import annotations

from typing import Any

from src.services.provider.adapters.antigravity.constants import DUMMY_THOUGHT_SIGNATURE
from src.services.provider.adapters.antigravity.signature_cache import ThinkingSignatureCache

# 测试用签名（需 >= MIN_SIGNATURE_LENGTH=50）
_SIG_A = "a" * 60
_SIG_B = "b" * 60
_SIG_C = "c" * 60
_SIG_D = "d" * 60
_SIG_E = "e" * 60


def test_get_or_dummy_returns_dummy_for_gemini_models() -> None:
    cache = ThinkingSignatureCache()
    assert cache.get_or_dummy("gemini-3-pro", "thinking...") == DUMMY_THOUGHT_SIGNATURE


def test_get_or_dummy_returns_none_for_non_gemini_models() -> None:
    cache = ThinkingSignatureCache()
    assert cache.get_or_dummy("claude-sonnet", "thinking...") is None


def test_cached_signature_preferred() -> None:
    cache = ThinkingSignatureCache()
    cache.cache("gemini-3-pro", "thinking-text", _SIG_A)
    assert cache.get_or_dummy("gemini-3-pro", "thinking-text") == _SIG_A


def test_short_signature_ignored() -> None:
    """短于 MIN_SIGNATURE_LENGTH 的签名不会被缓存。"""
    cache = ThinkingSignatureCache()
    cache.cache("gemini-3-pro", "text", "short")
    # 未命中缓存，回退到 DUMMY
    assert cache.get_or_dummy("gemini-3-pro", "text") == DUMMY_THOUGHT_SIGNATURE


# ===== Layer 1: Tool Signatures =====


def test_tool_signature_cache() -> None:
    cache = ThinkingSignatureCache()
    cache.cache_tool_signature("toolu_123", _SIG_A)
    assert cache.get_tool_signature("toolu_123") == _SIG_A
    assert cache.get_tool_signature("toolu_999") is None


def test_tool_signature_short_ignored() -> None:
    cache = ThinkingSignatureCache()
    cache.cache_tool_signature("toolu_123", "short")
    assert cache.get_tool_signature("toolu_123") is None


def test_tool_signature_cache_enforces_limit(monkeypatch: Any) -> None:
    import src.services.provider.adapters.antigravity.signature_cache as sc_mod

    # Use a small limit to make eviction deterministic in tests.
    monkeypatch.setattr(sc_mod, "_TOOL_CACHE_LIMIT", 3)
    cache = sc_mod.ThinkingSignatureCache()
    cache.cache_tool_signature("toolu_1", _SIG_A)
    cache.cache_tool_signature("toolu_2", _SIG_B)
    cache.cache_tool_signature("toolu_3", _SIG_C)
    cache.cache_tool_signature("toolu_4", _SIG_D)

    # Oldest entry should be evicted once the limit is exceeded.
    assert cache.get_tool_signature("toolu_1") is None
    assert cache.get_tool_signature("toolu_4") == _SIG_D


# ===== Layer 2: Thinking Families =====


def test_thinking_family_cache() -> None:
    cache = ThinkingSignatureCache()
    cache.cache_thinking_family(_SIG_A, "claude-3-5-sonnet")
    assert cache.get_signature_family(_SIG_A) == "claude-3-5-sonnet"
    assert cache.get_signature_family(_SIG_B) is None


# ===== Layer 3: Session Signatures =====


def test_session_signature_basic() -> None:
    cache = ThinkingSignatureCache()
    assert cache.get_session_signature("sid-test") is None

    cache.cache_session_signature("sid-test", _SIG_A, 5)
    assert cache.get_session_signature("sid-test") == _SIG_A


def test_session_signature_longer_replaces_same_count() -> None:
    """同一 message_count 下，更长的签名替换更短的。"""
    cache = ThinkingSignatureCache()
    sig_short = "x" * 60
    sig_long = "y" * 80

    cache.cache_session_signature("sid-1", sig_short, 5)
    cache.cache_session_signature("sid-1", sig_long, 5)
    assert cache.get_session_signature("sid-1") == sig_long

    # 更短的不会替换
    cache.cache_session_signature("sid-1", sig_short, 5)
    assert cache.get_session_signature("sid-1") == sig_long


def test_session_signature_rewind_detection() -> None:
    """Rewind: message_count 减少时强制更新签名。"""
    cache = ThinkingSignatureCache()
    cache.cache_session_signature("sid-1", _SIG_A, 10)
    assert cache.get_session_signature("sid-1") == _SIG_A

    # message_count 3 < 10 → rewind detected, force update
    cache.cache_session_signature("sid-1", _SIG_B, 3)
    assert cache.get_session_signature("sid-1") == _SIG_B


def test_session_signature_short_ignored() -> None:
    """短签名即使 rewind 也不会被缓存。"""
    cache = ThinkingSignatureCache()
    cache.cache_session_signature("sid-1", _SIG_A, 5)
    cache.cache_session_signature("sid-1", "short", 1)
    assert cache.get_session_signature("sid-1") == _SIG_A


def test_session_isolation() -> None:
    """不同 session 之间互相隔离。"""
    cache = ThinkingSignatureCache()
    cache.cache_session_signature("sid-1", _SIG_A, 1)
    assert cache.get_session_signature("sid-1") == _SIG_A
    assert cache.get_session_signature("sid-2") is None


# ===== Clear =====


def test_clear_all_layers() -> None:
    cache = ThinkingSignatureCache()
    cache.cache_tool_signature("tool-1", _SIG_A)
    cache.cache_thinking_family(_SIG_A, "claude")
    cache.cache_session_signature("sid-1", _SIG_B, 1)
    cache.cache("gemini-3-pro", "text", _SIG_C)

    cache.clear()

    assert cache.get_tool_signature("tool-1") is None
    assert cache.get_signature_family(_SIG_A) is None
    assert cache.get_session_signature("sid-1") is None
    # Legacy layer: 回退到 DUMMY
    assert cache.get_or_dummy("gemini-3-pro", "text") == DUMMY_THOUGHT_SIGNATURE
