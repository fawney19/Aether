"""Account Pool health policy: error code classification and key state management.

Maps upstream HTTP status codes to pool-level actions:

| Code         | Action                                                     |
|--------------|------------------------------------------------------------|
| 401          | Invalidate OAuth token cache; permanent (deactivated) 1h, else no cooldown |
| 402          | Long cooldown (payment issue)                              |
| 403          | Graded cooldown: severe (suspended/banned) 1h, else 300s+  |
| 400          | Check body for "organization has been disabled" -> cooldown |
| 429          | Cooldown (retry-after or rate_limit_cooldown_seconds)      |
| 529          | Cooldown (overload_cooldown_seconds)                       |
| *            | Check unschedulable_rules keyword matching                 |
| 408/5xx/etc  | Transient cooldown (overload_cooldown_seconds)             |
"""

from __future__ import annotations

import json
from typing import TYPE_CHECKING

from src.core.logger import logger
from src.services.provider.pool import redis_ops

if TYPE_CHECKING:
    from src.services.provider.pool.config import PoolConfig

# Patterns in 400 error body that indicate account-level issues.
_ACCOUNT_DISABLE_PATTERNS = (
    "organization has been disabled",
    "organization_disabled",
    "account has been disabled",
    "account_disabled",
    "account has been deactivated",
    "account_deactivated",
    "account deactivated",
)

# 需要更长冷却的账号异常语义（403 body 关键字）。
_FORBIDDEN_ACCOUNT_PATTERNS = (
    "account suspended",
    "account banned",
    "account deactivated",
    "subscription inactive",
    "suspended",
    "banned",
    "deactivated",
)

_TRANSIENT_STATUS_COOLDOWN_REASON: dict[int, str] = {
    408: "request_timeout_408",
    409: "conflict_409",
    423: "locked_423",
    425: "too_early_425",
    500: "server_error_500",
    502: "bad_gateway_502",
    503: "service_unavailable_503",
    504: "gateway_timeout_504",
}


def _parse_retry_after(headers: dict[str, str] | None) -> int | None:
    """Extract retry-after seconds from response headers."""
    if not headers:
        return None
    raw = headers.get("retry-after") or headers.get("Retry-After")
    if not raw:
        return None
    try:
        val = int(raw)
        return max(1, min(val, 3600))
    except (ValueError, TypeError):
        return None


def _extract_error_message(error_body: str | None) -> str:
    """Best-effort extraction of error message from JSON body."""
    if not error_body:
        return ""
    try:
        data = json.loads(error_body)
        if isinstance(data, dict):
            error_obj = data.get("error")
            if isinstance(error_obj, dict):
                return str(error_obj.get("message", ""))
            if isinstance(error_obj, str):
                return error_obj
            return str(data.get("message", ""))
    except (json.JSONDecodeError, TypeError):
        pass
    return error_body[:500]


def _resolve_transient_cooldown_ttl(
    *,
    status_code: int,
    retry_after_seconds: int | None,
    config: PoolConfig,
) -> int:
    """Resolve cooldown ttl for transient upstream status codes."""
    if status_code in (429, 503):
        if retry_after_seconds is not None:
            return retry_after_seconds
    if status_code == 429:
        return config.rate_limit_cooldown_seconds
    # 408/409/423/425/5xx: 统一走短时过载冷却，避免雪崩重试。
    return config.overload_cooldown_seconds


def _parse_google_quota_cooldown(error_body: str | None) -> int | None:
    """Parse Google-specific quota cooldown from error body.

    Safe to call for any provider: returns None unless the error body
    contains Google-specific fields (quotaResetTimeStamp / quotaResetDelay /
    "reset after" message pattern).
    """
    if not error_body:
        return None
    try:
        from src.services.provider.adapters.gemini_cli.quota import extract_quota_cooldown_seconds

        return extract_quota_cooldown_seconds(error_body)
    except Exception:
        return None


async def apply_health_policy(
    *,
    provider_id: str,
    key_id: str,
    status_code: int,
    error_body: str | None,
    response_headers: dict[str, str] | None,
    config: PoolConfig,
) -> None:
    """Apply health policy for an upstream error.

    This is fire-and-forget; exceptions are caught and logged.
    """
    if not config.health_policy_enabled:
        return

    try:
        await _apply(
            provider_id=provider_id,
            key_id=key_id,
            status_code=status_code,
            error_body=error_body,
            response_headers=response_headers,
            config=config,
        )
    except Exception as exc:
        logger.warning(
            "Pool health policy failed for key {}: {}",
            key_id[:8],
            str(exc),
        )


async def _apply(
    *,
    provider_id: str,
    key_id: str,
    status_code: int,
    error_body: str | None,
    response_headers: dict[str, str] | None,
    config: PoolConfig,
) -> None:
    error_msg = _extract_error_message(error_body)

    # --- 401 Unauthorized ---------------------------------------------------
    if status_code == 401:
        await redis_ops.invalidate_oauth_token_cache(key_id)
        # Check if the 401 body indicates a permanent account-level deactivation
        # (e.g. OpenAI "account_deactivated"). These deserve a long cooldown.
        error_lower = error_msg.lower()
        is_permanent = any(p in error_lower for p in _ACCOUNT_DISABLE_PATTERNS)
        if is_permanent:
            await redis_ops.set_cooldown(provider_id, key_id, "account_deactivated_401", ttl=3600)
            logger.warning(
                "Pool[{}]: key {} got 401 with account deactivation, cooldown 1h",
                provider_id[:8],
                key_id[:8],
            )
        else:
            # Transient auth failure (e.g. expired token) — token cache already
            # invalidated above; the next request will trigger a token refresh.
            # No cooldown needed: the key should be retried immediately after refresh.
            logger.info(
                "Pool[{}]: key {} got 401, token cache invalidated (no cooldown)",
                provider_id[:8],
                key_id[:8],
            )
        return

    # --- 402 Payment Required ------------------------------------------------
    if status_code == 402:
        await redis_ops.set_cooldown(provider_id, key_id, "payment_required_402", ttl=3600)
        logger.warning(
            "Pool[{}]: key {} got 402 (payment required), cooldown 1h",
            provider_id[:8],
            key_id[:8],
        )
        return

    # --- 403 Forbidden -------------------------------------------------------
    if status_code == 403:
        error_lower = error_msg.lower()
        severe = any(pattern in error_lower for pattern in _FORBIDDEN_ACCOUNT_PATTERNS)
        ttl = 3600 if severe else max(config.rate_limit_cooldown_seconds, 300)
        await redis_ops.set_cooldown(provider_id, key_id, "forbidden_403", ttl=ttl)
        logger.warning(
            "Pool[{}]: key {} got 403 (forbidden), cooldown {}s",
            provider_id[:8],
            key_id[:8],
            ttl,
        )
        return

    # --- 400 with account-disable pattern ------------------------------------
    if status_code == 400:
        error_lower = error_msg.lower()
        for pattern in _ACCOUNT_DISABLE_PATTERNS:
            if pattern in error_lower:
                await redis_ops.set_cooldown(
                    provider_id, key_id, f"account_disabled_400:{pattern}", ttl=3600
                )
                logger.warning(
                    "Pool[{}]: key {} got 400 with '{}', cooldown 1h",
                    provider_id[:8],
                    key_id[:8],
                    pattern,
                )
                return

    # --- 429 Rate Limited ----------------------------------------------------
    if status_code == 429:
        retry_after = _parse_retry_after(response_headers)
        if retry_after is None:
            retry_after = _parse_google_quota_cooldown(error_body)
        ttl = _resolve_transient_cooldown_ttl(
            status_code=status_code,
            retry_after_seconds=retry_after,
            config=config,
        )
        await redis_ops.set_cooldown(provider_id, key_id, "rate_limited_429", ttl=ttl)
        logger.info(
            "Pool[{}]: key {} got 429, cooldown {}s",
            provider_id[:8],
            key_id[:8],
            ttl,
        )
        return

    # --- 529 Overloaded ------------------------------------------------------
    if status_code == 529:
        ttl = config.overload_cooldown_seconds
        await redis_ops.set_cooldown(provider_id, key_id, "overloaded_529", ttl=ttl)
        logger.info(
            "Pool[{}]: key {} got 529, cooldown {}s",
            provider_id[:8],
            key_id[:8],
            ttl,
        )
        return

    # --- Keyword-based unschedulable rules -----------------------------------
    if config.unschedulable_rules and error_msg:
        error_lower = error_msg.lower()
        for rule in config.unschedulable_rules:
            if rule.keyword.lower() in error_lower:
                ttl = max(60, rule.duration_minutes * 60)
                await redis_ops.set_cooldown(
                    provider_id,
                    key_id,
                    f"rule:{rule.keyword}",
                    ttl=ttl,
                )
                logger.info(
                    "Pool[{}]: key {} matched rule '{}', cooldown {}m",
                    provider_id[:8],
                    key_id[:8],
                    rule.keyword,
                    rule.duration_minutes,
                )
                return

    # --- Transient status bucket (408/409/423/425/5xx) ----------------------
    reason = _TRANSIENT_STATUS_COOLDOWN_REASON.get(status_code)
    if reason:
        retry_after = _parse_retry_after(response_headers)
        ttl = _resolve_transient_cooldown_ttl(
            status_code=status_code,
            retry_after_seconds=retry_after,
            config=config,
        )
        await redis_ops.set_cooldown(provider_id, key_id, reason, ttl=ttl)
        logger.info(
            "Pool[{}]: key {} got {}, cooldown {}s ({})",
            provider_id[:8],
            key_id[:8],
            status_code,
            ttl,
            reason,
        )
        return


async def apply_stream_timeout_policy(
    *,
    provider_id: str,
    key_id: str,
    config: PoolConfig,
) -> None:
    """Record a stream timeout event and apply cooldown if threshold is reached.

    Called when an upstream stream response times out (no data within the
    configured interval). Increments a per-key counter in Redis and sets
    a cooldown if the count reaches the configured threshold.
    """
    if not config.health_policy_enabled:
        return

    try:
        count = await redis_ops.incr_stream_timeout_count(
            provider_id,
            key_id,
            config.stream_timeout_window_seconds,
        )
        if count >= config.stream_timeout_threshold:
            ttl = config.stream_timeout_cooldown_seconds
            await redis_ops.set_cooldown(
                provider_id,
                key_id,
                f"stream_timeout_x{count}",
                ttl=ttl,
            )
            logger.warning(
                "Pool[{}]: key {} stream timeout count {} >= threshold {}, cooldown {}s",
                provider_id[:8],
                key_id[:8],
                count,
                config.stream_timeout_threshold,
                ttl,
            )
        else:
            logger.info(
                "Pool[{}]: key {} stream timeout count {}/{}",
                provider_id[:8],
                key_id[:8],
                count,
                config.stream_timeout_threshold,
            )
    except Exception as exc:
        logger.warning(
            "Pool stream timeout policy failed for key {}: {}",
            key_id[:8],
            str(exc),
        )
