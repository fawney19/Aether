"""Shared helpers for pool preset dimensions."""

from __future__ import annotations

import math
import time
from typing import Any

from src.core.provider_types import ProviderType, normalize_provider_type
from src.services.provider_keys.quota_reader import get_quota_reader


def safe_float(value: Any) -> float | None:
    try:
        parsed = float(value)
    except (TypeError, ValueError):
        return None
    if math.isnan(parsed) or math.isinf(parsed):
        return None
    return parsed


def safe_metadata(key_obj: Any) -> dict[str, Any]:
    raw = getattr(key_obj, "upstream_metadata", None)
    return raw if isinstance(raw, dict) else {}


def normalize_plan(value: Any) -> str | None:
    if not isinstance(value, str):
        return None
    normalized = value.strip().lower()
    return normalized or None


def rank_ascending(key_id: str, scores: dict[str, float], all_ids: list[str]) -> float:
    """Rank score within all IDs; lower value means better rank."""

    if not all_ids:
        return 0.0

    valid_count = sum(1 for kid in all_ids if safe_float(scores.get(kid)) is not None)
    if valid_count <= 0:
        return 0.5

    decorated: list[tuple[int, float, int, str]] = []
    for idx, kid in enumerate(all_ids):
        score_raw = safe_float(scores.get(kid))
        if score_raw is None:
            decorated.append((1, float("inf"), idx, kid))
        else:
            decorated.append((0, score_raw, idx, kid))

    decorated.sort(key=lambda item: (item[0], item[1], item[2]))
    rank_idx = 0
    for idx, (_missing, _value, _order, kid) in enumerate(decorated):
        if kid == key_id:
            rank_idx = idx
            break

    n = len(all_ids)
    if n <= 1:
        return 0.0
    return rank_idx / float(n - 1)


def rank_descending(key_id: str, scores: dict[str, float], all_ids: list[str]) -> float:
    """Rank score within all IDs; higher value means better rank."""

    if not all_ids:
        return 0.0

    valid_count = sum(1 for kid in all_ids if safe_float(scores.get(kid)) is not None)
    if valid_count <= 0:
        return 0.5

    decorated: list[tuple[int, float, int, str]] = []
    for idx, kid in enumerate(all_ids):
        score_raw = safe_float(scores.get(kid))
        if score_raw is None:
            decorated.append((1, float("inf"), idx, kid))
        else:
            # 排序时取负值，使分值越大排名越靠前（rank 越小）
            decorated.append((0, -score_raw, idx, kid))

    decorated.sort(key=lambda item: (item[0], item[1], item[2]))
    rank_idx = 0
    for idx, (_missing, _value, _order, kid) in enumerate(decorated):
        if kid == key_id:
            rank_idx = idx
            break

    n = len(all_ids)
    if n <= 1:
        return 0.0
    return rank_idx / float(n - 1)


def extract_plan_type(key_obj: Any) -> str | None:
    direct = normalize_plan(getattr(key_obj, "oauth_plan_type", None))
    if direct:
        return direct

    metadata = safe_metadata(key_obj)
    for provider_type in (ProviderType.CODEX, ProviderType.KIRO, ProviderType.ANTIGRAVITY):
        plan_type = get_quota_reader(provider_type, metadata).plan_type()
        if plan_type:
            return plan_type

    return None


def _resolve_key_provider_type(key_obj: Any, provider_type: str | None = None) -> str | None:
    explicit = normalize_provider_type(provider_type)
    if explicit:
        return explicit

    direct = normalize_provider_type(getattr(key_obj, "provider_type", None))
    if direct:
        return direct

    provider = getattr(key_obj, "provider", None)
    related = normalize_provider_type(getattr(provider, "provider_type", None))
    if related:
        return related

    metadata = safe_metadata(key_obj)
    candidates = [
        provider.value
        for provider in (ProviderType.CODEX, ProviderType.KIRO, ProviderType.ANTIGRAVITY)
        if isinstance(metadata.get(provider.value), dict)
    ]
    if len(candidates) == 1:
        return candidates[0]

    return None


def _extract_codex_weekly_reset_seconds(metadata: dict[str, Any]) -> float | None:
    codex = metadata.get(ProviderType.CODEX.value)
    if not isinstance(codex, dict):
        return None

    weekly_used_percent = safe_float(codex.get("primary_used_percent"))
    if weekly_used_percent is not None:
        clamped_used = max(0.0, min(weekly_used_percent, 100.0))
        if clamped_used <= 1e-6:
            # 周额度仍为满额时，不启用周窗口重置倒计时。
            return None

    now = time.time()

    # 优先绝对时间戳，避免 reset_seconds 快照随时间漂移。
    reset_at = safe_float(codex.get("primary_reset_at"))
    if reset_at is not None and reset_at > 0:
        remaining = reset_at - now
        return remaining if remaining > 0 else 0.0

    reset_seconds = safe_float(codex.get("primary_reset_seconds"))
    if reset_seconds is None or reset_seconds < 0:
        return None

    updated_at = safe_float(codex.get("updated_at"))
    if updated_at is not None and updated_at > 0:
        # 时钟偏移下 updated_at 可能晚于当前时间，elapsed 需要下限钳制到 0。
        elapsed = max(now - updated_at, 0.0)
        return max(reset_seconds - elapsed, 0.0)

    return reset_seconds


def extract_reset_seconds(key_obj: Any, provider_type: str | None = None) -> float | None:
    metadata = safe_metadata(key_obj)
    resolved_provider_type = _resolve_key_provider_type(key_obj, provider_type)

    if resolved_provider_type == ProviderType.CODEX:
        # Codex metadata 已统一约定：primary_* 表示周限额，secondary_* 表示 5H 限额。
        return _extract_codex_weekly_reset_seconds(metadata)

    if resolved_provider_type in (ProviderType.KIRO, ProviderType.ANTIGRAVITY):
        return get_quota_reader(resolved_provider_type, metadata).reset_seconds()

    candidates: list[float] = []

    for provider_type in (ProviderType.CODEX, ProviderType.KIRO, ProviderType.ANTIGRAVITY):
        reset_seconds = get_quota_reader(provider_type, metadata).reset_seconds()
        if reset_seconds is None:
            continue
        candidates.append(reset_seconds)

    if not candidates:
        return None
    return min(candidates)


def extract_usage_ratio(key_obj: Any) -> float | None:
    metadata = safe_metadata(key_obj)

    for provider_type in (ProviderType.CODEX, ProviderType.KIRO, ProviderType.ANTIGRAVITY):
        usage_ratio = get_quota_reader(provider_type, metadata).usage_ratio()
        if usage_ratio is not None:
            return usage_ratio

    return None


def extract_internal_priority(key_obj: Any) -> int:
    raw = getattr(key_obj, "internal_priority", None)
    parsed = safe_float(raw)
    if parsed is None:
        return 999999
    return max(0, int(parsed))


def extract_health_score(key_obj: Any) -> float | None:
    direct = safe_float(getattr(key_obj, "health_score", None))
    if direct is not None:
        return max(0.0, min(direct, 1.0))

    health_by_format = getattr(key_obj, "health_by_format", None)
    if not isinstance(health_by_format, dict) or not health_by_format:
        return None

    scores: list[float] = []
    for payload in health_by_format.values():
        if not isinstance(payload, dict):
            continue
        score = safe_float(payload.get("health_score"))
        if score is None:
            continue
        scores.append(max(0.0, min(score, 1.0)))

    if not scores:
        return None
    return min(scores)


def plan_priority_score(plan_type: str | None, mode: str | None = None) -> float:
    """Score a key based on plan type and scheduling mode.

    Lower score = higher priority.
    """

    effective_mode = (mode or "both").strip().lower()
    if effective_mode == "free_only":
        if plan_type == "free":
            return 0.0
        if plan_type == "team":
            return 0.5
    elif effective_mode == "team_only":
        if plan_type == "team":
            return 0.0
        if plan_type == "free":
            return 0.5
    elif effective_mode == "plus_only":
        if plan_type in {"plus", "pro"}:
            return 0.0
        if plan_type in {"enterprise", "business"}:
            return 0.3
    else:
        # "both" or unrecognized -> original behavior
        if plan_type in {"free", "team"}:
            return 0.0
    if plan_type in {"enterprise", "business"}:
        return 0.2
    if plan_type in {"plus", "pro"}:
        return 0.6
    if plan_type:
        return 0.7
    return 0.8


__all__ = [
    "extract_health_score",
    "extract_internal_priority",
    "extract_plan_type",
    "extract_reset_seconds",
    "extract_usage_ratio",
    "normalize_plan",
    "plan_priority_score",
    "rank_ascending",
    "rank_descending",
    "safe_float",
    "safe_metadata",
]
