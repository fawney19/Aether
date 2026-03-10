"""Tests for pool_config.py — configuration parsing and defaults."""

from __future__ import annotations

from src.services.provider.pool.config import (
    PoolConfig,
    SchedulingPreset,
    ScoringWeights,
    UnschedulableRule,
    parse_pool_config,
)


def test_parse_pool_config_returns_none_when_no_advanced_section() -> None:
    assert parse_pool_config({}) is None
    assert parse_pool_config(None) is None
    assert parse_pool_config({"other_key": 1}) is None


def test_parse_pool_config_returns_defaults_for_empty_advanced() -> None:
    cfg = parse_pool_config({"pool_advanced": {}})
    assert cfg is not None
    assert cfg.sticky_session_ttl_seconds == 3600
    assert cfg.load_threshold_percent == 80
    assert cfg.lru_enabled is False
    assert cfg.scheduling_mode == "multi_score"
    assert cfg.scoring_weights == ScoringWeights()
    # Default: only cache_affinity preset enabled
    assert len(cfg.scheduling_presets) == 1
    assert cfg.scheduling_presets[0].preset == "cache_affinity"
    assert cfg.scheduling_presets[0].enabled is True
    assert cfg.latency_window_seconds == 3600
    assert cfg.latency_sample_limit == 50
    assert cfg.cost_window_seconds == 18000
    assert cfg.cost_limit_per_key_tokens is None
    assert cfg.cost_soft_threshold_percent == 80
    assert cfg.rate_limit_cooldown_seconds == 300
    assert cfg.overload_cooldown_seconds == 30
    assert cfg.proactive_refresh_seconds == 180
    assert cfg.health_policy_enabled is True
    assert cfg.unschedulable_rules == []
    assert cfg.batch_concurrency == 8
    assert cfg.probing_enabled is False
    assert cfg.probing_interval_minutes == 10
    assert cfg.auto_remove_banned_keys is False


def test_parse_pool_config_overrides_values_legacy_string_list() -> None:
    """Legacy string-list format with scheduling_mode/lru_enabled."""
    cfg = parse_pool_config(
        {
            "pool_advanced": {
                "sticky_session_ttl_seconds": 7200,
                "load_threshold_percent": 90,
                "lru_enabled": False,
                "scheduling_mode": "multi_score",
                "scheduling_presets": ["free_team_first", "recent_refresh", "free_team_first"],
                "scoring_weights": {
                    "lru": 0.1,
                    "latency": 0.5,
                    "health": 0.2,
                    "cost_remaining": 0.2,
                },
                "latency_window_seconds": 7200,
                "latency_sample_limit": 80,
                "cost_window_seconds": 36000,
                "cost_limit_per_key_tokens": 100000,
                "cost_soft_threshold_percent": 70,
                "rate_limit_cooldown_seconds": 600,
                "overload_cooldown_seconds": 60,
                "proactive_refresh_seconds": 300,
                "health_policy_enabled": False,
                "probing_enabled": True,
                "probing_interval_minutes": 15,
                "auto_remove_banned_keys": True,
            }
        }
    )
    assert cfg is not None
    assert cfg.sticky_session_ttl_seconds == 7200
    assert cfg.load_threshold_percent == 90
    assert cfg.lru_enabled is False
    assert cfg.scheduling_mode == "multi_score"
    # Legacy string list → SchedulingPreset objects, deduped
    preset_names = tuple(p.preset for p in cfg.scheduling_presets)
    assert "free_team_first" in preset_names
    assert "recent_refresh" in preset_names
    assert cfg.scoring_weights == ScoringWeights(
        lru=0.1,
        latency=0.5,
        health=0.2,
        cost_remaining=0.2,
    )
    assert cfg.latency_window_seconds == 7200
    assert cfg.latency_sample_limit == 80
    assert "multi_score" in cfg.strategies
    assert cfg.cost_window_seconds == 36000
    assert cfg.cost_limit_per_key_tokens == 100000
    assert cfg.cost_soft_threshold_percent == 70
    assert cfg.rate_limit_cooldown_seconds == 600
    assert cfg.overload_cooldown_seconds == 60
    assert cfg.proactive_refresh_seconds == 300
    assert cfg.health_policy_enabled is False
    assert cfg.batch_concurrency == 8
    assert cfg.probing_enabled is True
    assert cfg.probing_interval_minutes == 15
    assert cfg.auto_remove_banned_keys is True


def test_parse_pool_config_parses_batch_concurrency() -> None:
    cfg = parse_pool_config({"pool_advanced": {"batch_concurrency": 12}})
    assert cfg is not None
    assert cfg.batch_concurrency == 12


def test_parse_pool_config_clamps_batch_concurrency() -> None:
    cfg = parse_pool_config({"pool_advanced": {"batch_concurrency": 99}})
    assert cfg is not None
    assert cfg.batch_concurrency == 32


def test_parse_pool_config_new_object_list_format() -> None:
    """New object-list format: [{preset, enabled, mode}]."""
    cfg = parse_pool_config(
        {
            "pool_advanced": {
                "scheduling_presets": [
                    {"preset": "lru", "enabled": True},
                    {"preset": "free_team_first", "enabled": True, "mode": "free_only"},
                    {"preset": "quota_balanced", "enabled": False},
                    {"preset": "recent_refresh", "enabled": True},
                ],
            }
        }
    )
    assert cfg is not None
    assert len(cfg.scheduling_presets) == 4

    lru = cfg.scheduling_presets[0]
    assert lru.preset == "lru"
    assert lru.enabled is True

    ftf = cfg.scheduling_presets[1]
    assert ftf.preset == "free_team_first"
    assert ftf.enabled is True
    assert ftf.mode == "free_only"

    qb = cfg.scheduling_presets[2]
    assert qb.preset == "quota_balanced"
    assert qb.enabled is False

    rr = cfg.scheduling_presets[3]
    assert rr.preset == "recent_refresh"
    assert rr.enabled is True

    # Derived fields: lru enabled, non-lru enabled → multi_score
    assert cfg.lru_enabled is True
    assert cfg.scheduling_mode == "multi_score"


def test_parse_pool_config_new_format_lru_only() -> None:
    """When only LRU is enabled, scheduling_mode should be 'lru'."""
    cfg = parse_pool_config(
        {
            "pool_advanced": {
                "scheduling_presets": [
                    {"preset": "lru", "enabled": True},
                    {"preset": "quota_balanced", "enabled": False},
                ],
            }
        }
    )
    assert cfg is not None
    assert cfg.lru_enabled is True
    assert cfg.scheduling_mode == "lru"


def test_parse_pool_config_new_format_lru_disabled() -> None:
    """LRU disabled, other presets enabled → multi_score + lru_enabled=False."""
    cfg = parse_pool_config(
        {
            "pool_advanced": {
                "scheduling_presets": [
                    {"preset": "lru", "enabled": False},
                    {"preset": "quota_balanced", "enabled": True},
                ],
            }
        }
    )
    assert cfg is not None
    assert cfg.lru_enabled is False
    assert cfg.scheduling_mode == "multi_score"


def test_parse_pool_config_new_format_free_team_mode_validation() -> None:
    """Invalid mode falls back to 'both'."""
    cfg = parse_pool_config(
        {
            "pool_advanced": {
                "scheduling_presets": [
                    {"preset": "free_team_first", "enabled": True, "mode": "invalid_mode"},
                ],
            }
        }
    )
    assert cfg is not None
    ftf = [p for p in cfg.scheduling_presets if p.preset == "free_team_first"][0]
    assert ftf.mode == "both"


def test_parse_pool_config_new_format_dedup_presets() -> None:
    """Duplicate presets in object list should be deduplicated."""
    cfg = parse_pool_config(
        {
            "pool_advanced": {
                "scheduling_presets": [
                    {"preset": "lru", "enabled": True},
                    {"preset": "lru", "enabled": False},
                    {"preset": "quota_balanced", "enabled": True},
                ],
            }
        }
    )
    assert cfg is not None
    lru_presets = [p for p in cfg.scheduling_presets if p.preset == "lru"]
    assert len(lru_presets) == 1
    assert lru_presets[0].enabled is True  # first occurrence wins


def test_parse_pool_config_parses_unschedulable_rules() -> None:
    cfg = parse_pool_config(
        {
            "pool_advanced": {
                "unschedulable_rules": [
                    {"keyword": "rate_limit", "duration_minutes": 10},
                    {"keyword": "overloaded"},
                    {"invalid": "entry"},  # should be skipped
                    "not_a_dict",  # should be skipped
                ]
            }
        }
    )
    assert cfg is not None
    assert len(cfg.unschedulable_rules) == 2
    assert cfg.unschedulable_rules[0] == UnschedulableRule(
        keyword="rate_limit", duration_minutes=10
    )
    assert cfg.unschedulable_rules[1] == UnschedulableRule(keyword="overloaded", duration_minutes=5)


def test_parse_pool_config_handles_invalid_types_gracefully() -> None:
    # Invalid int values should fall back to defaults
    cfg = parse_pool_config(
        {
            "pool_advanced": {
                "sticky_session_ttl_seconds": "not_a_number",
                "cost_limit_per_key_tokens": "bad",
            }
        }
    )
    assert cfg is not None
    assert cfg.sticky_session_ttl_seconds == 3600  # default
    assert cfg.cost_limit_per_key_tokens is None  # default for opt_int


def test_parse_pool_config_invalid_scheduling_mode_falls_back_to_cache_affinity() -> None:
    cfg = parse_pool_config({"pool_advanced": {"scheduling_mode": "unknown"}})
    assert cfg is not None
    # Default with no explicit presets: cache_affinity -> multi_score
    assert cfg.scheduling_mode == "multi_score"


def test_parse_pool_config_scoring_weights_invalid_values_are_clamped() -> None:
    cfg = parse_pool_config(
        {
            "pool_advanced": {
                "scheduling_mode": "multi_score",
                "scoring_weights": {
                    "lru": 2.0,
                    "latency": -1.0,
                    "health": "bad",
                },
            }
        }
    )
    assert cfg is not None
    assert cfg.scoring_weights.lru == 1.0
    assert cfg.scoring_weights.latency == 0.0
    assert cfg.scoring_weights.health == 0.2


def test_parse_pool_config_invalid_scheduling_presets_are_ignored() -> None:
    cfg = parse_pool_config(
        {
            "pool_advanced": {
                "scheduling_mode": "multi_score",
                "scheduling_presets": ["quota_balanced", "unknown", 123, "single_account"],
            }
        }
    )
    assert cfg is not None
    preset_names = tuple(p.preset for p in cfg.scheduling_presets if p.preset != "lru")
    assert "quota_balanced" in preset_names
    assert "single_account" in preset_names
    assert "unknown" not in preset_names


def test_pool_config_is_frozen() -> None:
    cfg = PoolConfig()
    try:
        cfg.sticky_session_ttl_seconds = 999  # type: ignore[misc]
        assert False, "Should have raised FrozenInstanceError"
    except AttributeError:
        pass


def test_scheduling_preset_is_frozen() -> None:
    preset = SchedulingPreset(preset="lru", enabled=True)
    try:
        preset.enabled = False  # type: ignore[misc]
        assert False, "Should have raised FrozenInstanceError"
    except AttributeError:
        pass
