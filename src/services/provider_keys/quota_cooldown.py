"""配额冷却判定工具。"""

from __future__ import annotations

from typing import Any

from src.services.scheduling.quota_skipper import is_key_quota_exhausted


def resolve_effective_cooldown_reason(
    *,
    provider_type: str | None,
    key: Any,
    redis_reason: str | None,
) -> str | None:
    """返回 Key 的有效冷却原因。

    规则：
    - Redis 冷却存在时，优先返回 Redis 原因（429/403/quota_exhausted 等）。
    - Redis 冷却不存在时，回退到 upstream_metadata 配额判断：
      若账号级配额耗尽（Codex/Kiro），返回 ``quota_exhausted``。
    """
    if redis_reason:
        return redis_reason

    try:
        exhausted, _ = is_key_quota_exhausted(provider_type, key, model_name="")
    except Exception:
        return None
    return "quota_exhausted" if exhausted else None
