"""Antigravity thinking block signature cache (triple-layer).

与 Antigravity-Manager 对齐的三层缓存设计：
  Layer 1: tool_use_id → thoughtSignature  （工具调用签名恢复）
  Layer 2: signature  → model_family       （跨模型兼容校验）
  Layer 3: session_id → latest signature   （会话级签名追踪 + rewind 检测）

同时保留原有的 model:text → signature 兼容层。
"""

from __future__ import annotations

import hashlib
import threading
import time
from typing import Any

from src.services.provider.adapters.antigravity.constants import (
    DUMMY_THOUGHT_SIGNATURE,
    MIN_SIGNATURE_LENGTH,
)

# TTL: 2 小时（与 Antigravity-Manager 对齐）
_SIGNATURE_TTL_SECONDS = 2 * 60 * 60

# 各层缓存上限
_TOOL_CACHE_LIMIT = 500
_FAMILY_CACHE_LIMIT = 200
_SESSION_CACHE_LIMIT = 1000
_TEXT_CACHE_LIMIT = 1000


class _CacheEntry:
    """带时间戳的缓存条目，支持 TTL 过期。"""

    __slots__ = ("data", "created_at")

    def __init__(self, data: Any) -> None:
        self.data = data
        self.created_at: float = time.monotonic()

    def is_expired(self, now: float | None = None) -> bool:
        return ((now or time.monotonic()) - self.created_at) > _SIGNATURE_TTL_SECONDS


class _SessionEntry:
    """Session 层缓存数据，包含消息计数用于 rewind 检测。"""

    __slots__ = ("signature", "message_count")

    def __init__(self, signature: str, message_count: int) -> None:
        self.signature = signature
        self.message_count = message_count


class ThinkingSignatureCache:
    """Triple-layer thinking signature cache.

    Layer 1 (tool): tool_use_id → thoughtSignature
        当客户端(如 OpenCode) 在 tool_result 中丢弃了 signature 时用于恢复。

    Layer 2 (family): signature → model_family
        防止跨模型签名污染（Claude 签名不能用在 Gemini 上）。

    Layer 3 (session): session_id → latest signature + message_count
        会话级追踪，支持 rewind 检测（用户删除消息后不会注入来自"未来"的签名）。

    Legacy (text): SHA256(model + text) → signature
        向后兼容的 get_or_dummy() 接口。
    """

    def __init__(self) -> None:
        self._tool_sigs: dict[str, _CacheEntry] = {}
        self._families: dict[str, _CacheEntry] = {}
        self._sessions: dict[str, _CacheEntry] = {}
        self._text_sigs: dict[str, _CacheEntry] = {}
        self._lock = threading.Lock()

    # ===== Layer 1: Tool Use ID → Signature =====

    def cache_tool_signature(self, tool_use_id: str, signature: str) -> None:
        """缓存工具调用对应的 thinking signature。"""
        if len(signature) < MIN_SIGNATURE_LENGTH:
            return
        with self._lock:
            self._tool_sigs[tool_use_id] = _CacheEntry(signature)
            if len(self._tool_sigs) > _TOOL_CACHE_LIMIT:
                self._prune(self._tool_sigs, limit=_TOOL_CACHE_LIMIT)

    def get_tool_signature(self, tool_use_id: str) -> str | None:
        """查找工具调用对应的 signature。"""
        with self._lock:
            entry = self._tool_sigs.get(tool_use_id)
            if entry is None:
                return None
            if entry.is_expired():
                self._tool_sigs.pop(tool_use_id, None)
                return None
            return entry.data

    # ===== Layer 2: Signature → Model Family =====

    def cache_thinking_family(self, signature: str, family: str) -> None:
        """记录 signature 所属的模型家族。"""
        if len(signature) < MIN_SIGNATURE_LENGTH:
            return
        with self._lock:
            self._families[signature] = _CacheEntry(family)
            if len(self._families) > _FAMILY_CACHE_LIMIT:
                self._prune(self._families, limit=_FAMILY_CACHE_LIMIT)

    def get_signature_family(self, signature: str) -> str | None:
        """查找 signature 所属的模型家族。"""
        with self._lock:
            entry = self._families.get(signature)
            if entry is None:
                return None
            if entry.is_expired():
                self._families.pop(signature, None)
                return None
            return entry.data

    # ===== Layer 3: Session ID → Latest Signature =====

    def cache_session_signature(
        self, session_id: str, signature: str, message_count: int = 0
    ) -> None:
        """存储会话的最新 thinking signature。

        Rewind 检测：当 message_count 小于已缓存值时，说明用户删除了消息，
        强制更新签名以避免注入来自"未来"的签名。
        """
        if len(signature) < MIN_SIGNATURE_LENGTH:
            return

        with self._lock:
            existing = self._sessions.get(session_id)
            should_store = True

            if existing and not existing.is_expired():
                entry: _SessionEntry = existing.data
                if message_count < entry.message_count:
                    # Rewind detected: 用户删除了消息，强制更新
                    pass
                elif message_count == entry.message_count:
                    # 同一轮消息：仅当新签名更长（更完整）时才替换
                    should_store = len(signature) > len(entry.signature)
                # else: 正常递增，更新

            if should_store:
                self._sessions[session_id] = _CacheEntry(_SessionEntry(signature, message_count))
                if len(self._sessions) > _SESSION_CACHE_LIMIT:
                    self._prune(self._sessions, limit=_SESSION_CACHE_LIMIT)

    def get_session_signature(self, session_id: str) -> str | None:
        """获取会话的最新 thinking signature。"""
        with self._lock:
            entry = self._sessions.get(session_id)
            if entry is None:
                return None
            if entry.is_expired():
                self._sessions.pop(session_id, None)
                return None
            return entry.data.signature

    # ===== Legacy: model:text → signature（向后兼容） =====

    def get_or_dummy(self, model: str, thinking_text: str) -> str | None:
        """Legacy: 根据 model + thinking_text 查找 signature。

        Gemini 模型在未命中时返回 DUMMY_THOUGHT_SIGNATURE（跳过验证）。
        """
        key = self._text_key(model, thinking_text)
        with self._lock:
            entry = self._text_sigs.get(key)
            if entry is not None:
                if entry.is_expired():
                    self._text_sigs.pop(key, None)
                else:
                    return entry.data
        if str(model).startswith("gemini-"):
            return DUMMY_THOUGHT_SIGNATURE
        return None

    def cache(self, model: str, thinking_text: str, signature: str) -> None:
        """Legacy: 缓存 model + thinking_text → signature。"""
        if len(signature) < MIN_SIGNATURE_LENGTH:
            return

        key = self._text_key(model, thinking_text)
        with self._lock:
            if key in self._text_sigs:
                self._text_sigs[key] = _CacheEntry(signature)
                return

            if len(self._text_sigs) >= _TEXT_CACHE_LIMIT:
                # FIFO 淘汰 1/4
                evict_n = max(1, _TEXT_CACHE_LIMIT // 4)
                for k in list(self._text_sigs.keys())[:evict_n]:
                    self._text_sigs.pop(k, None)

            self._text_sigs[key] = _CacheEntry(signature)

    # ===== Utilities =====

    @staticmethod
    def _text_key(model: str, thinking_text: str) -> str:
        # 使用 \x00 作为分隔符避免 model 中含 ':' 时的歧义
        content = f"{model}\x00{thinking_text}"
        return hashlib.sha256(content.encode("utf-8")).hexdigest()[:32]

    @staticmethod
    def _prune(d: dict[str, _CacheEntry], *, limit: int | None = None) -> None:
        """Remove expired entries and optionally enforce a size limit."""
        now = time.monotonic()
        expired = [k for k, v in d.items() if v.is_expired(now)]
        for k in expired:
            d.pop(k, None)

        if limit is None or len(d) <= limit:
            return

        # Evict oldest entries by created_at.
        excess = len(d) - limit
        for k, _entry in sorted(d.items(), key=lambda kv: kv[1].created_at)[:excess]:
            d.pop(k, None)


    def clear(self) -> None:
        """清空所有缓存层（用于测试或手动重置）。"""
        with self._lock:
            self._tool_sigs.clear()
            self._families.clear()
            self._sessions.clear()
            self._text_sigs.clear()


signature_cache = ThinkingSignatureCache()

__all__ = ["ThinkingSignatureCache", "signature_cache"]
