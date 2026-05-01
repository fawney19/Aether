"""Account Pool configuration (provider-agnostic)."""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any

from src.core.logger import logger
from src.services.provider.pool.dimensions import get_preset_dimension, get_preset_names


@dataclass(frozen=True, slots=True)
class ScoringWeights:
    """Weights used by multi-score scheduling."""

    lru: float = 0.3
    latency: float = 0.25
    health: float = 0.2
    cost_remaining: float = 0.25


@dataclass(frozen=True, slots=True)
class SchedulingPreset:
    """Single scheduling preset item with enable/disable and optional sub-config."""

    preset: str
    enabled: bool = True
    mode: str | None = None


@dataclass(frozen=True, slots=True)
class UnschedulableRule:
    """Keyword-based temporary unschedule rule."""

    keyword: str
    duration_minutes: int = 5


@dataclass(frozen=True, slots=True)
class PoolConfig:
    """Parsed pool configuration for any Provider.

    All transient state lives in Redis; this dataclass only holds
    the *configuration* that controls pool behaviour.
    """

    # -- Sticky Session -------------------------------------------------------
    sticky_session_ttl_seconds: int = 3600  # 1 hour
    # -- Load-Aware Selection -------------------------------------------------
    load_threshold_percent: int = 80

    # -- Scheduling (unified preset list) -------------------------------------
    scheduling_presets: tuple[SchedulingPreset, ...] = (
        SchedulingPreset(preset="cache_affinity", enabled=True),
    )
    # Derived from scheduling_presets at parse time (backward compat for consumers)
    lru_enabled: bool = True
    scheduling_mode: str = "lru"  # lru | multi_score

    scoring_weights: ScoringWeights = field(default_factory=ScoringWeights)
    latency_window_seconds: int = 3600
    latency_sample_limit: int = 50

    # -- Rolling-Window Cost Tracking -----------------------------------------
    cost_window_seconds: int = 18000  # 5 hours
    cost_limit_per_key_tokens: int | None = None  # None = unlimited
    cost_soft_threshold_percent: int = 80

    # -- Cooldown Defaults ----------------------------------------------------
    rate_limit_cooldown_seconds: int = 300  # 429
    overload_cooldown_seconds: int = 30  # 529

    # -- OAuth Proactive Refresh ----------------------------------------------
    proactive_refresh_seconds: int = 180  # 3 minutes before expiry

    # -- Health Policy --------------------------------------------------------
    health_policy_enabled: bool = True

    # -- Temporary Unschedulable Rules ----------------------------------------
    unschedulable_rules: list[UnschedulableRule] = field(default_factory=list)

    # -- Batch Operations -----------------------------------------------------
    batch_concurrency: int = 8

    # -- Quota Probing --------------------------------------------------------
    probing_enabled: bool = False
    probing_interval_minutes: int = 10
    auto_remove_banned_keys: bool = False

    # -- Stream Timeout Auto-Pause --------------------------------------------
    stream_timeout_threshold: int = 3  # N timeouts within window trigger cooldown
    stream_timeout_window_seconds: int = 1800  # 30 min counting window
    stream_timeout_cooldown_seconds: int = 300  # 5 min cooldown

    # -- Pluggable Strategies -------------------------------------------------
    strategies: tuple[str, ...] = ()


def parse_pool_config(provider_config: Any) -> PoolConfig | None:
    """Parse PoolConfig from ``Provider.config``.

    Only looks for the explicit ``pool_advanced`` key.  Returns ``None``
    when the provider has no pool section configured, meaning the caller
    should use the normal (non-pool) scheduling path.
    """
    config_dict = provider_config if isinstance(provider_config, dict) else {}

    raw_advanced = config_dict.get("pool_advanced")
    if raw_advanced is None:
        return None

    if not isinstance(raw_advanced, dict):
        # Could be a pre-validated Pydantic model; grab its dict.
        try:
            raw_advanced = raw_advanced.model_dump()  # type: ignore[union-attr]
        except Exception:
            logger.warning(
                "PoolConfig: advanced config type invalid ({}), falling back to defaults",
                type(raw_advanced).__name__,
            )
            return PoolConfig()

    rules: list[UnschedulableRule] = []
    raw_rules = raw_advanced.get("unschedulable_rules")
    if isinstance(raw_rules, list):
        for r in raw_rules:
            if isinstance(r, dict) and isinstance(r.get("keyword"), str):
                rules.append(
                    UnschedulableRule(
                        keyword=r["keyword"],
                        duration_minutes=int(r.get("duration_minutes", 5)),
                    )
                )

    def _int_or(key: str, default: int) -> int:
        v = raw_advanced.get(key)
        if v is None:
            return default
        try:
            return int(v)
        except (TypeError, ValueError):
            return default

    def _bool_or(key: str, default: bool) -> bool:
        v = raw_advanced.get(key)
        if v is None:
            return default
        return bool(v)

    def _opt_int(key: str) -> int | None:
        v = raw_advanced.get(key)
        if v is None:
            return None
        try:
            return int(v)
        except (TypeError, ValueError):
            return None

    scoring_weights = _parse_scoring_weights(raw_advanced.get("scoring_weights"))

    # Parse scheduling presets (new object-list format or legacy string-list)
    presets = _parse_scheduling_presets_v2(
        raw_advanced.get("scheduling_presets"),
        legacy_mode=raw_advanced.get("scheduling_mode"),
        legacy_lru=raw_advanced.get("lru_enabled"),
    )

    # Derive scheduling_mode and lru_enabled from the presets list
    enabled = [p for p in presets if p.enabled]
    lru_enabled = any(p.preset == "lru" for p in enabled)
    non_lru_enabled = [p for p in enabled if p.preset != "lru"]
    scheduling_mode = "multi_score" if non_lru_enabled else "lru"

    strategies = list(_parse_strategies(raw_advanced.get("strategies")))
    if scheduling_mode == "multi_score" and "multi_score" not in strategies:
        strategies.append("multi_score")

    return PoolConfig(
        sticky_session_ttl_seconds=_int_or("sticky_session_ttl_seconds", 3600),
        load_threshold_percent=_int_or("load_threshold_percent", 80),
        scheduling_presets=presets,
        lru_enabled=lru_enabled,
        scheduling_mode=scheduling_mode,
        scoring_weights=scoring_weights,
        latency_window_seconds=_int_or("latency_window_seconds", 3600),
        latency_sample_limit=_int_or("latency_sample_limit", 50),
        cost_window_seconds=_int_or("cost_window_seconds", 18000),
        cost_limit_per_key_tokens=_opt_int("cost_limit_per_key_tokens"),
        cost_soft_threshold_percent=_int_or("cost_soft_threshold_percent", 80),
        rate_limit_cooldown_seconds=_int_or("rate_limit_cooldown_seconds", 300),
        overload_cooldown_seconds=_int_or("overload_cooldown_seconds", 30),
        proactive_refresh_seconds=_int_or("proactive_refresh_seconds", 180),
        health_policy_enabled=_bool_or("health_policy_enabled", True),
        unschedulable_rules=rules,
        batch_concurrency=max(1, min(_int_or("batch_concurrency", 8), 32)),
        probing_enabled=_bool_or("probing_enabled", False),
        probing_interval_minutes=max(1, min(_int_or("probing_interval_minutes", 10), 1440)),
        auto_remove_banned_keys=_bool_or("auto_remove_banned_keys", False),
        stream_timeout_threshold=_int_or("stream_timeout_threshold", 3),
        stream_timeout_window_seconds=_int_or("stream_timeout_window_seconds", 1800),
        stream_timeout_cooldown_seconds=_int_or("stream_timeout_cooldown_seconds", 300),
        strategies=tuple(strategies),
    )


# ---------------------------------------------------------------------------
# Internal parsers
# ---------------------------------------------------------------------------


def _allowed_preset_names() -> set[str]:
    return get_preset_names() | {"lru"}


def _get_preset_mode_meta(preset_name: str) -> tuple[tuple[str, ...], str | None]:
    dim = get_preset_dimension(preset_name)
    if dim is None or not dim.modes:
        return (), None

    modes = tuple(str(mode).strip().lower() for mode in dim.modes if str(mode).strip())
    if not modes:
        return (), None

    raw_default = str(dim.default_mode or "").strip().lower()
    default_mode = raw_default if raw_default in modes else modes[0]
    return modes, default_mode


def _parse_strategies(raw: Any) -> tuple[str, ...]:
    """Parse strategy names from config (list[str] -> tuple[str, ...])."""
    if not isinstance(raw, list):
        return ()
    return tuple(str(s) for s in raw if isinstance(s, str) and s)


def _parse_scoring_weights(raw: Any) -> ScoringWeights:
    """Parse scoring weights with graceful fallback."""
    if not isinstance(raw, dict):
        return ScoringWeights()

    def _float_or(value: Any, default: float) -> float:
        try:
            parsed = float(value)
        except (TypeError, ValueError):
            return default
        return max(0.0, min(parsed, 1.0))

    return ScoringWeights(
        lru=_float_or(raw.get("lru"), 0.3),
        latency=_float_or(raw.get("latency"), 0.25),
        health=_float_or(raw.get("health"), 0.2),
        cost_remaining=_float_or(raw.get("cost_remaining"), 0.25),
    )


def _parse_scheduling_presets_v2(
    raw: Any,
    *,
    legacy_mode: Any = None,
    legacy_lru: Any = None,
) -> tuple[SchedulingPreset, ...]:
    """Parse scheduling presets, supporting both new and legacy formats.

    New format::

        [{"preset": "lru", "enabled": true},
         {"preset": "free_team_first", "enabled": true, "mode": "free_only"},
         ...]

    Legacy format::

        ["free_team_first", "recent_refresh"]  (with separate scheduling_mode / lru_enabled)
    """
    if isinstance(raw, list) and raw:
        first = raw[0]
        if isinstance(first, dict):
            return _parse_preset_object_list(raw)
        if isinstance(first, str):
            return _convert_legacy_string_list(raw, legacy_mode, legacy_lru)

    # No presets at all: derive from legacy fields
    return _build_from_legacy_fields(legacy_mode, legacy_lru)


def _parse_preset_object_list(raw: list[Any]) -> tuple[SchedulingPreset, ...]:
    """Parse new-format object list into SchedulingPreset tuple."""
    allowed = _allowed_preset_names()
    ordered: list[SchedulingPreset] = []
    seen: set[str] = set()
    for item in raw:
        if not isinstance(item, dict):
            continue
        name = str(item.get("preset", "")).strip().lower()
        if name not in allowed or name in seen:
            continue
        seen.add(name)
        enabled = bool(item.get("enabled", True))
        mode: str | None = None
        modes, default_mode = _get_preset_mode_meta(name)
        if modes:
            raw_mode = str(item.get("mode", default_mode) or "").strip().lower()
            mode = raw_mode if raw_mode in modes else default_mode
        ordered.append(SchedulingPreset(preset=name, enabled=enabled, mode=mode))
    return tuple(ordered) if ordered else (SchedulingPreset(preset="lru", enabled=True),)


def _convert_legacy_string_list(
    raw: list[Any],
    legacy_mode: Any,
    legacy_lru: Any,
) -> tuple[SchedulingPreset, ...]:
    """Convert legacy string list + mode/lru fields to new format."""
    lru_enabled = legacy_lru if isinstance(legacy_lru, bool) else True

    allowed_non_lru = _allowed_preset_names() - {"lru"}
    items: list[SchedulingPreset] = [SchedulingPreset(preset="lru", enabled=lru_enabled)]
    seen: set[str] = {"lru"}
    for p in raw:
        if not isinstance(p, str):
            continue
        name = p.strip().lower()
        if name not in allowed_non_lru or name in seen:
            continue
        seen.add(name)
        items.append(SchedulingPreset(preset=name, enabled=True))
    return tuple(items)


def _build_from_legacy_fields(legacy_mode: Any, legacy_lru: Any) -> tuple[SchedulingPreset, ...]:
    """Build presets from legacy scheduling_mode / lru_enabled only."""
    # Explicit lru_enabled=True -> LRU; explicit lru_enabled=False -> cache_affinity.
    # No legacy fields at all -> default to cache_affinity.
    if isinstance(legacy_lru, bool):
        if legacy_lru:
            return (SchedulingPreset(preset="lru", enabled=True),)
        return (SchedulingPreset(preset="cache_affinity", enabled=True),)
    return (SchedulingPreset(preset="cache_affinity", enabled=True),)
