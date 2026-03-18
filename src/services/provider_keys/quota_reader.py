"""Unified quota readers for provider key upstream metadata."""

from __future__ import annotations

import math
import time
from abc import ABC, abstractmethod
from dataclasses import dataclass
from typing import Any

from src.core.provider_types import ProviderType, normalize_provider_type


def _to_float(value: Any) -> float | None:
    try:
        parsed = float(value)
    except (TypeError, ValueError):
        return None
    if math.isnan(parsed) or math.isinf(parsed):
        return None
    return parsed


def _normalize_plan(value: Any) -> str | None:
    if not isinstance(value, str):
        return None
    normalized = value.strip().lower()
    return normalized or None


def _is_truthy_flag(value: Any) -> bool:
    if isinstance(value, bool):
        return value
    if isinstance(value, (int, float)):
        return value != 0
    if isinstance(value, str):
        normalized = value.strip().lower()
        return normalized in {"1", "true", "yes", "y"}
    return False


def _extract_reason(source: dict[str, Any], *fields: str) -> str | None:
    for field in fields:
        value = source.get(field)
        if not isinstance(value, str):
            continue
        text = value.strip()
        if text:
            return text
    return None


def _pct_is_exhausted(value: Any) -> bool:
    pct = _to_float(value)
    if pct is None:
        return False
    return pct >= 100.0 - 1e-6


def _format_percent(value: float) -> str:
    clamped = max(0.0, min(value, 100.0))
    return f"{clamped:.1f}%"


def _format_quota_value(value: float) -> str:
    rounded = round(value)
    if abs(value - rounded) < 1e-6:
        return str(rounded)
    return f"{value:.1f}"


def _has_quota_consumption(used_percent_raw: Any) -> bool:
    used = _to_float(used_percent_raw)
    if used is None:
        return False
    clamped_used = max(0.0, min(used, 100.0))
    return clamped_used > 1e-6


def _format_reset_after(seconds_raw: Any) -> str | None:
    seconds = _to_float(seconds_raw)
    if seconds is None:
        return None

    total_seconds = int(seconds)
    if total_seconds <= 0:
        return "已重置"

    days = total_seconds // 86400
    hours = (total_seconds % 86400) // 3600
    minutes = (total_seconds % 3600) // 60

    if days > 0:
        return f"{days}天{hours}小时后重置"
    if hours > 0:
        return f"{hours}小时{minutes}分钟后重置"
    if minutes > 0:
        return f"{minutes}分钟后重置"
    return "即将重置"


@dataclass(frozen=True, slots=True)
class QuotaExhaustedResult:
    exhausted: bool
    reason: str | None = None


@dataclass(frozen=True, slots=True)
class AccountBlockResult:
    blocked: bool
    code: str | None = None
    label: str | None = None
    reason: str | None = None


class PoolQuotaReader(ABC):
    """Read-only view over one provider namespace in upstream_metadata."""

    namespace: str | None = None

    def __init__(self, data: dict[str, Any] | None) -> None:
        self._data: dict[str, Any] = data if isinstance(data, dict) else {}

    @abstractmethod
    def is_exhausted(self, model_name: str | None = None) -> QuotaExhaustedResult:
        """Return whether this key/model should be skipped for quota exhaustion."""

    @abstractmethod
    def usage_ratio(self) -> float | None:
        """Return usage ratio within [0, 1], when available."""

    @abstractmethod
    def plan_type(self) -> str | None:
        """Return normalized plan type, when available."""

    @abstractmethod
    def reset_seconds(self) -> float | None:
        """Return seconds until next reset, when available."""

    @abstractmethod
    def account_block(self) -> AccountBlockResult:
        """Return account-level block state derived from metadata."""

    @abstractmethod
    def display_summary(self) -> str | None:
        """Return admin-facing quota summary string."""

    def updated_at(self) -> int | None:
        updated_at = _to_float(self._data.get("updated_at"))
        if updated_at is None or updated_at <= 0:
            return None
        if updated_at > 1_000_000_000_000:
            updated_at /= 1000
        return int(updated_at)


class NullQuotaReader(PoolQuotaReader):
    def is_exhausted(self, model_name: str | None = None) -> QuotaExhaustedResult:
        _ = model_name
        return QuotaExhaustedResult(exhausted=False)

    def usage_ratio(self) -> float | None:
        return None

    def plan_type(self) -> str | None:
        return None

    def reset_seconds(self) -> float | None:
        return None

    def account_block(self) -> AccountBlockResult:
        return AccountBlockResult(blocked=False)

    def display_summary(self) -> str | None:
        return None


class CodexQuotaReader(PoolQuotaReader):
    namespace = "codex"

    def is_exhausted(self, model_name: str | None = None) -> QuotaExhaustedResult:
        _ = model_name
        exhausted_parts: list[str] = []
        if _pct_is_exhausted(self._data.get("primary_used_percent")):
            exhausted_parts.append("周限额剩余 0%")
        if _pct_is_exhausted(self._data.get("secondary_used_percent")):
            exhausted_parts.append("5H 限额剩余 0%")
        if exhausted_parts:
            return QuotaExhaustedResult(True, "Codex " + "，".join(exhausted_parts))
        return QuotaExhaustedResult(False)

    def usage_ratio(self) -> float | None:
        values: list[float] = []
        for field in ("primary_used_percent", "secondary_used_percent"):
            parsed = _to_float(self._data.get(field))
            if parsed is None:
                continue
            values.append(max(0.0, min(parsed, 100.0)) / 100.0)
        if not values:
            return None
        return sum(values) / len(values)

    def plan_type(self) -> str | None:
        return _normalize_plan(self._data.get("plan_type"))

    def reset_seconds(self) -> float | None:
        candidates: list[float] = []
        for field in ("secondary_reset_seconds", "primary_reset_seconds"):
            parsed = _to_float(self._data.get(field))
            if parsed is None or parsed < 0:
                continue
            candidates.append(parsed)
        if not candidates:
            return None
        return min(candidates)

    def account_block(self) -> AccountBlockResult:
        if not _is_truthy_flag(self._data.get("account_disabled")):
            return AccountBlockResult(blocked=False)
        reason = _extract_reason(self._data, "forbidden_reason", "ban_reason", "reason", "message")
        return AccountBlockResult(
            blocked=True,
            code="account_forbidden",
            label="访问受限",
            reason=reason or "账号访问受限",
        )

    def display_summary(self) -> str | None:
        parts: list[str] = []

        primary_used = _to_float(self._data.get("primary_used_percent"))
        if primary_used is not None:
            part = f"周剩余 {_format_percent(100.0 - primary_used)}"
            reset_text = (
                _format_reset_after(self._data.get("primary_reset_seconds"))
                if _has_quota_consumption(primary_used)
                else None
            )
            if reset_text:
                part = f"{part} ({reset_text})"
            parts.append(part)

        secondary_used = _to_float(self._data.get("secondary_used_percent"))
        if secondary_used is not None:
            part = f"5H剩余 {_format_percent(100.0 - secondary_used)}"
            reset_text = (
                _format_reset_after(self._data.get("secondary_reset_seconds"))
                if _has_quota_consumption(secondary_used)
                else None
            )
            if reset_text:
                part = f"{part} ({reset_text})"
            parts.append(part)

        if parts:
            return " | ".join(parts)

        has_credits = self._data.get("has_credits")
        credits_balance = _to_float(self._data.get("credits_balance"))
        if has_credits is True and credits_balance is not None:
            return f"积分 {credits_balance:.2f}"
        if has_credits is True:
            return "有积分"
        return None


class KiroQuotaReader(PoolQuotaReader):
    namespace = "kiro"

    def is_exhausted(self, model_name: str | None = None) -> QuotaExhaustedResult:
        _ = model_name
        remaining = _to_float(self._data.get("remaining"))
        if remaining is not None and remaining <= 0.0:
            return QuotaExhaustedResult(True, "Kiro 账号配额剩余 0")
        return QuotaExhaustedResult(False)

    def usage_ratio(self) -> float | None:
        parsed = _to_float(self._data.get("usage_percentage"))
        if parsed is None:
            return None
        return max(0.0, min(parsed, 100.0)) / 100.0

    def plan_type(self) -> str | None:
        subscription_title = _normalize_plan(self._data.get("subscription_title"))
        if not subscription_title:
            return None
        if "team" in subscription_title:
            return "team"
        if "free" in subscription_title:
            return "free"
        if "pro" in subscription_title:
            return "pro"
        if "plus" in subscription_title:
            return "plus"
        return subscription_title

    def reset_seconds(self) -> float | None:
        next_reset_at = _to_float(self._data.get("next_reset_at"))
        if next_reset_at is None or next_reset_at <= 0:
            return None
        return max(0.0, next_reset_at - time.time())

    def account_block(self) -> AccountBlockResult:
        if not _is_truthy_flag(self._data.get("is_banned")):
            return AccountBlockResult(blocked=False)
        reason = _extract_reason(self._data, "ban_reason", "reason", "message")
        return AccountBlockResult(
            blocked=True,
            code="account_banned",
            label="账号封禁",
            reason=reason or "Kiro 账号已封禁",
        )

    def display_summary(self) -> str | None:
        if self._data.get("is_banned") is True:
            return "账号已封禁"

        usage_percentage = _to_float(self._data.get("usage_percentage"))
        if usage_percentage is not None:
            remaining = 100.0 - usage_percentage
            current_usage = _to_float(self._data.get("current_usage"))
            usage_limit = _to_float(self._data.get("usage_limit"))
            if current_usage is not None and usage_limit is not None and usage_limit > 0:
                return (
                    f"剩余 {_format_percent(remaining)} "
                    f"({_format_quota_value(current_usage)}/{_format_quota_value(usage_limit)})"
                )
            return f"剩余 {_format_percent(remaining)}"

        remaining = _to_float(self._data.get("remaining"))
        usage_limit = _to_float(self._data.get("usage_limit"))
        if remaining is not None and usage_limit is not None and usage_limit > 0:
            return f"剩余 {_format_quota_value(remaining)}/{_format_quota_value(usage_limit)}"
        return None


class AntigravityQuotaReader(PoolQuotaReader):
    namespace = "antigravity"

    def _quota_by_model(self) -> dict[str, Any]:
        quota_by_model = self._data.get("quota_by_model")
        if not isinstance(quota_by_model, dict):
            return {}
        return quota_by_model

    def _used_percent(self, model_info: dict[str, Any]) -> float | None:
        used_percent = _to_float(model_info.get("used_percent"))
        if used_percent is not None:
            return max(0.0, min(used_percent, 100.0))
        remaining_fraction = _to_float(model_info.get("remaining_fraction"))
        if remaining_fraction is None:
            return None
        return max(0.0, min((1.0 - remaining_fraction) * 100.0, 100.0))

    def is_exhausted(self, model_name: str | None = None) -> QuotaExhaustedResult:
        if not model_name:
            return QuotaExhaustedResult(False)
        model_quota = self._quota_by_model().get(model_name)
        if not isinstance(model_quota, dict):
            return QuotaExhaustedResult(False)

        remaining_fraction = _to_float(model_quota.get("remaining_fraction"))
        if remaining_fraction is not None and remaining_fraction <= 0.0:
            return QuotaExhaustedResult(True, f"Antigravity 模型 {model_name} 配额剩余 0%")
        if _pct_is_exhausted(model_quota.get("used_percent")):
            return QuotaExhaustedResult(True, f"Antigravity 模型 {model_name} 配额剩余 0%")
        return QuotaExhaustedResult(False)

    def usage_ratio(self) -> float | None:
        usage_values: list[float] = []
        for model_info in self._quota_by_model().values():
            if not isinstance(model_info, dict):
                continue
            used_percent = self._used_percent(model_info)
            if used_percent is None:
                continue
            usage_values.append(used_percent / 100.0)
        if not usage_values:
            return None
        return sum(usage_values) / len(usage_values)

    def plan_type(self) -> str | None:
        return None

    def reset_seconds(self) -> float | None:
        return None

    def account_block(self) -> AccountBlockResult:
        if not _is_truthy_flag(self._data.get("is_forbidden")):
            return AccountBlockResult(blocked=False)
        reason = _extract_reason(self._data, "forbidden_reason", "reason", "message")
        return AccountBlockResult(
            blocked=True,
            code="account_forbidden",
            label="访问受限",
            reason=reason or "Antigravity 账户访问受限",
        )

    def display_summary(self) -> str | None:
        if self._data.get("is_forbidden") is True:
            return "访问受限"

        remaining_list: list[float] = []
        for raw_info in self._quota_by_model().values():
            if not isinstance(raw_info, dict):
                continue
            used_percent = self._used_percent(raw_info)
            if used_percent is None:
                continue
            remaining_list.append(max(0.0, min(100.0 - used_percent, 100.0)))

        if not remaining_list:
            return None

        min_remaining = min(remaining_list)
        if len(remaining_list) == 1:
            return f"剩余 {_format_percent(min_remaining)}"
        return f"最低剩余 {_format_percent(min_remaining)} ({len(remaining_list)} 模型)"


class GeminiCliQuotaReader(PoolQuotaReader):
    namespace = "gemini_cli"

    def _quota_by_model(self) -> dict[str, Any]:
        quota_by_model = self._data.get("quota_by_model")
        if not isinstance(quota_by_model, dict):
            return {}
        return quota_by_model

    def _reset_at(self, model_info: dict[str, Any]) -> int | None:
        reset_at = _to_float(model_info.get("reset_at"))
        if reset_at is None or reset_at <= 0:
            return None
        if reset_at > 1_000_000_000_000:
            reset_at /= 1000
        return int(reset_at)

    def _is_model_exhausted(self, model_info: dict[str, Any]) -> bool:
        if _is_truthy_flag(model_info.get("is_exhausted")):
            return True
        remaining_fraction = _to_float(model_info.get("remaining_fraction"))
        if remaining_fraction is not None and remaining_fraction <= 0.0:
            return True
        return _pct_is_exhausted(model_info.get("used_percent"))

    def _active_exhausted_models(self) -> list[tuple[str, dict[str, Any], int | None]]:
        now = int(time.time())
        active: list[tuple[str, dict[str, Any], int | None]] = []
        for model_name, raw_info in self._quota_by_model().items():
            if not isinstance(raw_info, dict):
                continue
            if not self._is_model_exhausted(raw_info):
                continue
            reset_at = self._reset_at(raw_info)
            if reset_at is not None and reset_at <= now:
                continue
            active.append((str(model_name), raw_info, reset_at))
        return active

    def is_exhausted(self, model_name: str | None = None) -> QuotaExhaustedResult:
        if not model_name:
            return QuotaExhaustedResult(False)
        model_quota = self._quota_by_model().get(model_name)
        if not isinstance(model_quota, dict) or not self._is_model_exhausted(model_quota):
            return QuotaExhaustedResult(False)

        reset_at = self._reset_at(model_quota)
        if reset_at is not None:
            now = int(time.time())
            if reset_at <= now:
                return QuotaExhaustedResult(False)
            reset_text = _format_reset_after(reset_at - now)
            if reset_text:
                return QuotaExhaustedResult(
                    True, f"Gemini CLI 模型 {model_name} 冷却中（{reset_text}）"
                )
        return QuotaExhaustedResult(True, f"Gemini CLI 模型 {model_name} 配额已耗尽")

    def usage_ratio(self) -> float | None:
        active = self._active_exhausted_models()
        if not active:
            return None
        return 1.0

    def plan_type(self) -> str | None:
        return _normalize_plan(self._data.get("plan_type")) or _normalize_plan(
            self._data.get("tier")
        )

    def reset_seconds(self) -> float | None:
        now = int(time.time())
        reset_values = [
            reset_at - now
            for _, _, reset_at in self._active_exhausted_models()
            if reset_at is not None and reset_at > now
        ]
        if not reset_values:
            return None
        return float(min(reset_values))

    def account_block(self) -> AccountBlockResult:
        return AccountBlockResult(blocked=False)

    def display_summary(self) -> str | None:
        active = self._active_exhausted_models()
        if not active:
            return None

        active_sorted = sorted(
            active,
            key=lambda item: item[2] if item[2] is not None else 2**31 - 1,
        )
        first_model, _, first_reset_at = active_sorted[0]
        if len(active_sorted) == 1:
            if first_reset_at is not None:
                reset_text = _format_reset_after(first_reset_at - int(time.time()))
                if reset_text:
                    return f"{first_model} 冷却中 ({reset_text})"
            return f"{first_model} 冷却中"

        if first_reset_at is not None:
            reset_text = _format_reset_after(first_reset_at - int(time.time()))
            if reset_text:
                return f"{len(active_sorted)} 个模型冷却中（最早 {reset_text}）"
        return f"{len(active_sorted)} 个模型冷却中"


_READER_CLASSES: dict[str, type[PoolQuotaReader]] = {
    ProviderType.CODEX: CodexQuotaReader,
    ProviderType.GEMINI_CLI: GeminiCliQuotaReader,
    ProviderType.KIRO: KiroQuotaReader,
    ProviderType.ANTIGRAVITY: AntigravityQuotaReader,
}


def get_quota_reader(provider_type: str | None, upstream_metadata: Any) -> PoolQuotaReader:
    """Return a quota reader for one provider namespace in upstream_metadata."""

    normalized_type = normalize_provider_type(provider_type)
    reader_cls = _READER_CLASSES.get(normalized_type)
    if reader_cls is None or not isinstance(upstream_metadata, dict):
        return NullQuotaReader(None)

    namespace = reader_cls.namespace
    if not namespace:
        return NullQuotaReader(None)

    data = upstream_metadata.get(namespace)
    if not isinstance(data, dict):
        return NullQuotaReader(None)
    return reader_cls(data)


__all__ = [
    "AccountBlockResult",
    "AntigravityQuotaReader",
    "CodexQuotaReader",
    "GeminiCliQuotaReader",
    "KiroQuotaReader",
    "NullQuotaReader",
    "PoolQuotaReader",
    "QuotaExhaustedResult",
    "get_quota_reader",
]
