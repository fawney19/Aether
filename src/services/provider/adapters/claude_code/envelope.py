"""Claude Code upstream envelope hooks."""

from __future__ import annotations

import threading
import time
import uuid
from dataclasses import replace
from typing import Any

from src.clients.redis_client import get_redis_client, get_redis_client_sync
from src.config.settings import config
from src.core.exceptions import ConcurrencyLimitError
from src.core.logger import logger
from src.services.provider.adapters.claude_code.constants import (
    BETA_CONTEXT_1M,
    CLAUDE_CODE_DEFAULT_HEADERS,
    CLAUDE_CODE_REQUIRED_BETA_TOKENS,
    DEFAULT_ACCEPT,
    DEFAULT_ANTHROPIC_VERSION,
    SESSION_ID_MASKING_TTL_SECONDS,
    STREAM_HELPER_METHOD,
)
from src.services.provider.adapters.claude_code.context import (
    ClaudeCodeRequestContext,
    get_claude_code_request_context,
    set_claude_code_request_context,
)
from src.services.provider.request_context import get_current_fingerprint

_SESSION_MARKER = "_session_"
_DUMMY_THINKING_SIGNATURE = "skip_thought_signature_validator"
_session_runtime_lock = threading.Lock()
# key: scope_key -> {session_id -> last_seen_monotonic}
_active_sessions: dict[str, dict[str, float]] = {}
# key: scope_key -> (masked_session_uuid, expire_at_monotonic)
_masked_sessions: dict[str, tuple[str, float]] = {}
# 上限保护：scope_key 总数超过此值时触发全局清理
_MAX_SCOPE_KEYS = 5000
# 全局清理间隔（monotonic 秒），避免高频请求时每次都做全局扫描
_LAST_GLOBAL_CLEANUP: float = 0.0
_GLOBAL_CLEANUP_INTERVAL = 300.0  # 5 分钟
_REDIS_SESSION_KEY_PREFIX = "claude_code:sessions"
_REDIS_SESSION_RESERVE_LUA = """
local key = KEYS[1]
local sid = ARGV[1]
local now = tonumber(ARGV[2])
local expire_before = tonumber(ARGV[3])
local max_sessions = tonumber(ARGV[4])
local ttl_seconds = tonumber(ARGV[5])

redis.call("ZREMRANGEBYSCORE", key, "-inf", expire_before)

local exists = redis.call("ZSCORE", key, sid)
if exists then
    redis.call("ZADD", key, now, sid)
    redis.call("EXPIRE", key, ttl_seconds)
    return {1, redis.call("ZCARD", key)}
end

local active = redis.call("ZCARD", key)
if active >= max_sessions then
    return {0, active}
end

redis.call("ZADD", key, now, sid)
redis.call("EXPIRE", key, ttl_seconds)
return {1, active + 1}
"""


def merge_anthropic_beta_tokens(
    incoming: str | None,
    *,
    required: tuple[str, ...] = CLAUDE_CODE_REQUIRED_BETA_TOKENS,
) -> str:
    """Merge required beta tokens and incoming anthropic-beta with deduplication."""
    seen: set[str] = set()
    merged: list[str] = []

    def _append(token: str) -> None:
        token = token.strip()
        if not token or token in seen:
            return
        seen.add(token)
        merged.append(token)

    for token in required:
        _append(token)
    for token in str(incoming or "").split(","):
        _append(token)

    return ",".join(merged)


def _parse_stream_flag(raw_stream: Any) -> bool:
    if isinstance(raw_stream, bool):
        return raw_stream
    return str(raw_stream).strip().lower() in {"1", "true", "yes", "on"}


def _get_metadata_user_id(request_body: dict[str, Any]) -> str | None:
    metadata = request_body.get("metadata")
    if not isinstance(metadata, dict):
        return None
    user_id = metadata.get("user_id")
    if not isinstance(user_id, str):
        return None
    text = user_id.strip()
    return text or None


def _set_metadata_user_id(request_body: dict[str, Any], user_id: str) -> None:
    metadata = request_body.get("metadata")
    if not isinstance(metadata, dict):
        metadata = {}
        request_body["metadata"] = metadata
    metadata["user_id"] = user_id


def _extract_session_id(user_id: str) -> str | None:
    idx = user_id.rfind(_SESSION_MARKER)
    if idx == -1:
        return None
    session_id = user_id[idx + len(_SESSION_MARKER) :].strip()
    return session_id or None


def _is_thinking_enabled(request_body: dict[str, Any]) -> bool:
    thinking = request_body.get("thinking")
    if not isinstance(thinking, dict):
        return False
    thinking_type = str(thinking.get("type") or "").strip().lower()
    return thinking_type in {"enabled", "adaptive"}


def _sanitize_thinking_blocks(request_body: dict[str, Any]) -> None:
    """过滤可能导致 Claude Code 400 的无效 thinking 块。"""
    messages = request_body.get("messages")
    if not isinstance(messages, list) or not messages:
        return

    thinking_enabled = _is_thinking_enabled(request_body)
    filtered_messages = 0
    filtered_blocks = 0

    for message in messages:
        if not isinstance(message, dict):
            continue

        role = str(message.get("role") or "")
        content = message.get("content")
        if not isinstance(content, list):
            continue

        new_content: list[Any] = []
        modified = False
        for block in content:
            if not isinstance(block, dict):
                new_content.append(block)
                continue

            block_type = str(block.get("type") or "")
            if block_type in {"thinking", "redacted_thinking"}:
                keep = False
                # 仅保留 assistant 且带真实 signature 的 thinking 块。
                if thinking_enabled and role == "assistant":
                    signature = str(block.get("signature") or "").strip()
                    keep = bool(signature and signature != _DUMMY_THINKING_SIGNATURE)
                if keep:
                    new_content.append(block)
                else:
                    modified = True
                    filtered_blocks += 1
                continue

            # 兼容无 type 但带 thinking 字段的历史块，直接移除。
            if not block_type and "thinking" in block:
                modified = True
                filtered_blocks += 1
                continue

            new_content.append(block)

        if modified:
            message["content"] = new_content
            filtered_messages += 1

    if filtered_blocks:
        logger.info(
            "Claude Code thinking 预过滤: messages={}, blocks={}, thinking_enabled={}",
            filtered_messages,
            filtered_blocks,
            thinking_enabled,
        )


def _get_or_create_masked_session(scope_key: str) -> str:
    now = time.monotonic()
    with _session_runtime_lock:
        existing = _masked_sessions.get(scope_key)
        if existing and existing[1] > now:
            masked_session_id = existing[0]
        else:
            masked_session_id = str(uuid.uuid4())
        _masked_sessions[scope_key] = (
            masked_session_id,
            now + SESSION_ID_MASKING_TTL_SECONDS,
        )
        return masked_session_id


def _apply_session_id_masking(request_body: dict[str, Any], *, scope_key: str) -> None:
    user_id = _get_metadata_user_id(request_body)
    if not user_id:
        return
    idx = user_id.rfind(_SESSION_MARKER)
    if idx == -1:
        return
    masked_session_id = _get_or_create_masked_session(scope_key)
    _set_metadata_user_id(
        request_body,
        user_id[: idx + len(_SESSION_MARKER)] + masked_session_id,
    )


# -- Cache TTL Override -------------------------------------------------------

_VALID_CACHE_TTL_TARGETS = {"ephemeral", "1h"}


def _override_cache_control_in_blocks(blocks: list[Any], target: str) -> int:
    """Override cache_control TTL in a list of content blocks. Returns count of overrides.

    According to Anthropic API docs, cache_control format is:
        {"type": "ephemeral", "ttl": "5m" | "1h"}
    ``type`` is always "ephemeral"; the ``ttl`` field controls the actual duration.
    When ttl is absent, the default is 5m (ephemeral).
    """
    count = 0
    for block in blocks:
        if not isinstance(block, dict):
            continue
        cc = block.get("cache_control")
        if not isinstance(cc, dict):
            continue
        # Ensure type is always "ephemeral"
        if cc.get("type") != "ephemeral":
            cc["type"] = "ephemeral"
        if target == "ephemeral":
            # Target is 5m (default) -- remove explicit ttl so it falls back to default
            if "ttl" in cc:
                del cc["ttl"]
                count += 1
        else:
            # Target is "1h" -- set ttl explicitly
            if cc.get("ttl") != target:
                cc["ttl"] = target
                count += 1
    return count


def _apply_cache_ttl_override(request_body: dict[str, Any], target: str) -> None:
    """Force all cache_control entries to use a unified TTL type.

    Prevents multi-user behavioral fingerprinting when sharing an OAuth account.
    """
    if target not in _VALID_CACHE_TTL_TARGETS:
        return

    overridden = 0

    # system prompt (can be string or list of blocks)
    system = request_body.get("system")
    if isinstance(system, list):
        overridden += _override_cache_control_in_blocks(system, target)

    # messages
    messages = request_body.get("messages")
    if isinstance(messages, list):
        for msg in messages:
            if not isinstance(msg, dict):
                continue
            content = msg.get("content")
            if isinstance(content, list):
                overridden += _override_cache_control_in_blocks(content, target)

    # tools
    tools = request_body.get("tools")
    if isinstance(tools, list):
        overridden += _override_cache_control_in_blocks(tools, target)

    if overridden:
        logger.debug("Cache TTL override: {} block(s) -> {}", overridden, target)


def _cleanup_stale_scope_keys(now: float, idle_seconds: int) -> None:
    """清理空 bucket 和过期的 _masked_sessions 条目（调用方须持有锁）。"""
    global _LAST_GLOBAL_CLEANUP

    if now - _LAST_GLOBAL_CLEANUP < _GLOBAL_CLEANUP_INTERVAL:
        return
    _LAST_GLOBAL_CLEANUP = now

    # 清理所有 scope_key 下的过期 session，并删除空 bucket
    stale_keys = []
    for sk, bucket in _active_sessions.items():
        expired = [sid for sid, ts in bucket.items() if now - ts > idle_seconds]
        for sid in expired:
            bucket.pop(sid, None)
        if not bucket:
            stale_keys.append(sk)
    for sk in stale_keys:
        _active_sessions.pop(sk, None)

    # 清理过期的 masked session 条目
    expired_masked = [sk for sk, (_, exp) in _masked_sessions.items() if exp <= now]
    for sk in expired_masked:
        _masked_sessions.pop(sk, None)

    total = len(_active_sessions) + len(_masked_sessions)
    if total > 0 and (stale_keys or expired_masked):
        logger.debug(
            "Session 全局清理: 移除 {} 个 active scope + {} 个 masked scope, 剩余 {}",
            len(stale_keys),
            len(expired_masked),
            total,
        )


def _register_or_reject_session(
    *,
    scope_key: str,
    session_id: str,
    max_sessions: int,
    idle_timeout_minutes: int,
) -> tuple[bool, int]:
    now = time.monotonic()
    idle_seconds = max(60, int(idle_timeout_minutes * 60))

    with _session_runtime_lock:
        # scope_key 总数超限或达到清理间隔时，执行全局清理
        if (
            len(_active_sessions) + len(_masked_sessions) > _MAX_SCOPE_KEYS
            or now - _LAST_GLOBAL_CLEANUP >= _GLOBAL_CLEANUP_INTERVAL
        ):
            _cleanup_stale_scope_keys(now, idle_seconds)

        bucket = _active_sessions.setdefault(scope_key, {})

        # 先清理当前 bucket 的过期会话，避免误判占用。
        expired = [sid for sid, last_seen in bucket.items() if now - last_seen > idle_seconds]
        for sid in expired:
            bucket.pop(sid, None)

        if session_id in bucket:
            bucket[session_id] = now
            return True, len(bucket)

        if len(bucket) >= max_sessions:
            return False, len(bucket)

        bucket[session_id] = now
        return True, len(bucket)


def _build_session_limit_error(
    *,
    max_sessions: int,
    active_count: int,
    key_id: str | None,
) -> ConcurrencyLimitError:
    return ConcurrencyLimitError(
        message=(f"Claude Code 活跃会话数已达上限（{max_sessions}）。当前活跃会话: {active_count}"),
        key_id=key_id,
    )


def _redis_session_key(scope_key: str) -> str:
    return f"{_REDIS_SESSION_KEY_PREFIX}:{scope_key}"


def _parse_redis_session_result(raw: Any) -> tuple[bool, int] | None:
    if not isinstance(raw, (list, tuple)) or len(raw) < 2:
        return None
    try:
        allowed = int(raw[0]) == 1
        active_count = int(raw[1])
    except Exception:
        return None
    return allowed, active_count


def _enforce_session_controls(
    request_body: dict[str, Any],
    ctx: ClaudeCodeRequestContext,
    *,
    enforce_max_sessions: bool = True,
) -> None:
    """同步执行会话限制 + masking。

    当 ``enforce_max_sessions=False``（将由 ``enforce_distributed_session_controls``
    异步接管）时仅做 masking；masking 始终在会话限制检查之后执行，避免
    用被伪装后的 session_id 做计数。
    """
    scope_key = str(ctx.scope_key or "").strip()
    if not scope_key:
        return

    # 先基于真实 session_id 做会话限制检查。
    if enforce_max_sessions and ctx.max_sessions and ctx.max_sessions > 0:
        user_id = _get_metadata_user_id(request_body)
        if user_id:
            session_id = _extract_session_id(user_id) or user_id
            allowed, active_count = _register_or_reject_session(
                scope_key=scope_key,
                session_id=session_id,
                max_sessions=ctx.max_sessions,
                idle_timeout_minutes=ctx.session_idle_timeout_minutes,
            )
            if not allowed:
                raise _build_session_limit_error(
                    max_sessions=ctx.max_sessions,
                    active_count=active_count,
                    key_id=ctx.key_id,
                )

    if not enforce_max_sessions:
        # 分布式模式下 masking 延迟到 enforce_distributed_session_controls 中执行，
        # 避免 wrap_request 提前改写 user_id 导致分布式检查拿到伪装后的 session_id。
        return

    # 仅在本地模式下立即 masking。
    if ctx.session_id_masking_enabled:
        _apply_session_id_masking(request_body, scope_key=scope_key)


def _is_distributed_session_control_available() -> bool:
    try:
        return get_redis_client_sync() is not None
    except Exception:
        return False


async def enforce_distributed_session_controls(
    request_body: dict[str, Any],
    ctx: ClaudeCodeRequestContext | None,
) -> None:
    """异步执行会话限制 + masking。

    优先使用 Redis（多实例共享）；Redis 不可用时回退到进程内计数。
    masking 在会话限制检查通过后执行，确保计数使用真实 session_id。
    """
    if ctx is None:
        return

    scope_key = str(ctx.scope_key or "").strip()
    if not scope_key:
        # 即使无 scope_key 也无法做 masking（需要 scope_key 作为 key），直接返回。
        return

    if not ctx.max_sessions or ctx.max_sessions <= 0:
        # 无会话限制，仅做 masking。
        if ctx.session_id_masking_enabled:
            _apply_session_id_masking(request_body, scope_key=scope_key)
        return

    # 基于真实 user_id 提取 session_id 做限制检查。
    user_id = _get_metadata_user_id(request_body)
    if not user_id:
        return
    session_id = _extract_session_id(user_id) or user_id

    idle_seconds = max(60, int(ctx.session_idle_timeout_minutes * 60))
    redis_ttl = idle_seconds + 300
    now = int(time.time())
    expire_before = now - idle_seconds

    redis_client = await get_redis_client(require_redis=False)
    if redis_client is not None:
        try:
            raw_result = await redis_client.eval(
                _REDIS_SESSION_RESERVE_LUA,
                1,
                _redis_session_key(scope_key),
                session_id,
                str(now),
                str(expire_before),
                str(ctx.max_sessions),
                str(redis_ttl),
            )
            parsed = _parse_redis_session_result(raw_result)
            if parsed is None:
                raise ValueError(f"invalid redis eval result: {raw_result!r}")

            allowed, active_count = parsed
            if not allowed:
                raise _build_session_limit_error(
                    max_sessions=ctx.max_sessions,
                    active_count=active_count,
                    key_id=ctx.key_id,
                )
            # 会话限制通过后再做 masking。
            if ctx.session_id_masking_enabled:
                _apply_session_id_masking(request_body, scope_key=scope_key)
            return
        except ConcurrencyLimitError:
            raise
        except Exception as exc:
            logger.warning("Claude Code 分布式会话控制失败，回退本地计数: {}", str(exc))

    allowed, active_count = _register_or_reject_session(
        scope_key=scope_key,
        session_id=session_id,
        max_sessions=ctx.max_sessions,
        idle_timeout_minutes=ctx.session_idle_timeout_minutes,
    )
    if not allowed:
        raise _build_session_limit_error(
            max_sessions=ctx.max_sessions,
            active_count=active_count,
            key_id=ctx.key_id,
        )

    # 会话限制通过后再做 masking。
    if ctx.session_id_masking_enabled:
        _apply_session_id_masking(request_body, scope_key=scope_key)


class ClaudeCodeEnvelope:
    """Provider envelope hooks for Claude Code OAuth upstream."""

    name = "claude:cli"

    def extra_headers(self) -> dict[str, str] | None:
        ctx = get_claude_code_request_context()
        is_stream = bool(ctx.is_stream) if ctx else False
        fp = get_current_fingerprint()

        headers = dict(CLAUDE_CODE_DEFAULT_HEADERS)
        headers["Accept"] = DEFAULT_ACCEPT
        headers["anthropic-version"] = DEFAULT_ANTHROPIC_VERSION
        headers["anthropic-beta"] = merge_anthropic_beta_tokens(None)
        if is_stream:
            headers["x-stainless-helper-method"] = STREAM_HELPER_METHOD

        if fp:
            headers["X-Stainless-Package-Version"] = fp.stainless_package_version
            headers["X-Stainless-OS"] = fp.stainless_os
            headers["X-Stainless-Arch"] = fp.stainless_arch
            headers["X-Stainless-Runtime-Version"] = fp.stainless_runtime_version
            headers["X-Stainless-Timeout"] = fp.stainless_timeout
            headers["User-Agent"] = fp.user_agent
        else:
            ua = str(getattr(config, "internal_user_agent_claude_cli", "") or "").strip()
            if ua:
                headers["User-Agent"] = ua

        return headers

    def wrap_request(
        self,
        request_body: dict[str, Any],
        *,
        model: str,  # noqa: ARG002
        url_model: str | None,
        decrypted_auth_config: dict[str, Any] | None,  # noqa: ARG002
    ) -> tuple[dict[str, Any], str | None]:
        raw_stream = request_body.get("stream", False)
        is_stream = _parse_stream_flag(raw_stream)

        ctx = get_claude_code_request_context()
        if ctx is None:
            ctx = ClaudeCodeRequestContext()

        # CLI-only restriction: reject non-CLI clients early.
        if ctx.cli_only_enabled:
            from src.services.provider.adapters.claude_code.client_restriction import (
                enforce_cli_only,
            )

            enforce_cli_only(ctx.cli_only_enabled)

        # Extract session_uuid from metadata.user_id for pool sticky session.
        session_uuid: str | None = None
        user_id = _get_metadata_user_id(request_body)
        if user_id:
            session_uuid = _extract_session_id(user_id)
        ctx = replace(ctx, is_stream=is_stream, session_uuid=session_uuid)
        set_claude_code_request_context(ctx)

        _sanitize_thinking_blocks(request_body)

        # Cache TTL override: unify cache_control types to prevent behavioral fingerprinting.
        if ctx.cache_ttl_override_enabled:
            _apply_cache_ttl_override(request_body, ctx.cache_ttl_override_target)

        _enforce_session_controls(
            request_body,
            ctx,
            enforce_max_sessions=not _is_distributed_session_control_available(),
        )
        return request_body, url_model

    def unwrap_response(self, data: Any) -> Any:
        return data

    def postprocess_unwrapped_response(self, *, model: str, data: Any) -> None:  # noqa: ARG002
        return

    def capture_selected_base_url(self) -> str | None:
        return None

    def on_http_status(self, *, base_url: str | None, status_code: int) -> None:  # noqa: ARG002
        return

    def on_connection_error(self, *, base_url: str | None, exc: Exception) -> None:  # noqa: ARG002
        return

    def force_stream_rewrite(self) -> bool:
        return False

    # ------------------------------------------------------------------
    # Optional lifecycle hooks
    # ------------------------------------------------------------------

    def prepare_context(
        self,
        *,
        provider_config: Any,
        key_id: str,
        user_api_key_id: str | None = None,  # noqa: ARG002
        is_stream: bool,
        provider_id: str | None = None,
        key: Any = None,
    ) -> str | None:
        from src.services.provider.adapters.claude_code.context import (
            build_and_set_claude_code_request_context,
        )

        # 在 envelope 层设置指纹 context var（仅 Claude Code 需要指纹注入）
        if key is not None:
            from src.services.provider.fingerprint import ensure_key_fingerprint
            from src.services.provider.request_context import set_current_fingerprint

            set_current_fingerprint(ensure_key_fingerprint(key, persist_if_missing=True))

        build_and_set_claude_code_request_context(
            provider_config=provider_config,
            key_id=key_id,
            is_stream=is_stream,
            provider_id=provider_id,
        )
        fp = get_current_fingerprint()
        if fp:
            return fp.impersonate
        return None

    async def post_wrap_request(self, request_body: dict[str, Any]) -> None:
        await enforce_distributed_session_controls(
            request_body,
            get_claude_code_request_context(),
        )

    def excluded_beta_tokens(self) -> frozenset[str]:
        return frozenset({BETA_CONTEXT_1M})


claude_code_envelope = ClaudeCodeEnvelope()


__all__ = [
    "ClaudeCodeEnvelope",
    "claude_code_envelope",
    "enforce_distributed_session_controls",
    "merge_anthropic_beta_tokens",
]
