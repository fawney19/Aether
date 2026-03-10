"""Tests for built-in multi-score pool strategy."""

from __future__ import annotations

from types import SimpleNamespace

from src.services.provider.pool.config import PoolConfig, SchedulingPreset, ScoringWeights
from src.services.provider.pool.strategies.multi_score import MultiScoreStrategy
from src.services.provider.pool.strategy import get_pool_strategy, register_pool_strategy


def _context() -> dict:
    return {
        "all_key_ids": ["k1", "k2", "k3"],
        "lru_scores": {"k1": 100.0, "k2": 200.0, "k3": 300.0},
        "latency_avgs": {"k1": 120.0, "k2": 300.0, "k3": 600.0},
        "health_scores": {"k1": 0.95, "k2": 0.7, "k3": 0.4},
        "cost_totals": {"k1": 100, "k2": 300, "k3": 900},
    }


def _key_with_metadata(metadata: dict, **kwargs: object) -> SimpleNamespace:
    return SimpleNamespace(upstream_metadata=metadata, **kwargs)


def test_multi_score_returns_none_when_mode_not_enabled() -> None:
    strategy = MultiScoreStrategy()
    cfg = PoolConfig(scheduling_mode="lru")
    score = strategy.compute_score(key_id="k1", config=cfg, context=_context())
    assert score is None


def test_multi_score_prefers_low_latency_when_latency_weight_is_high() -> None:
    strategy = MultiScoreStrategy()
    cfg = PoolConfig(
        scheduling_mode="multi_score",
        scheduling_presets=(),
        scoring_weights=ScoringWeights(lru=0.0, latency=1.0, health=0.0, cost_remaining=0.0),
    )
    ctx = _context()
    s1 = strategy.compute_score(key_id="k1", config=cfg, context=ctx)
    s2 = strategy.compute_score(key_id="k2", config=cfg, context=ctx)
    s3 = strategy.compute_score(key_id="k3", config=cfg, context=ctx)
    assert s1 is not None and s2 is not None and s3 is not None
    assert s1 < s2 < s3


def test_multi_score_combines_health_and_cost() -> None:
    strategy = MultiScoreStrategy()
    cfg = PoolConfig(
        scheduling_mode="multi_score",
        scheduling_presets=(),
        scoring_weights=ScoringWeights(lru=0.0, latency=0.0, health=0.5, cost_remaining=0.5),
        cost_limit_per_key_tokens=1000,
    )
    ctx = _context()
    s1 = strategy.compute_score(key_id="k1", config=cfg, context=ctx)
    s3 = strategy.compute_score(key_id="k3", config=cfg, context=ctx)
    assert s1 is not None and s3 is not None
    assert s1 < s3


def test_multi_score_strategy_is_registered() -> None:
    register_pool_strategy("multi_score", MultiScoreStrategy())
    registered = get_pool_strategy("multi_score")
    assert registered is not None


def test_multi_score_preset_free_team_first_prefers_free_or_team() -> None:
    strategy = MultiScoreStrategy()
    cfg = PoolConfig(
        scheduling_mode="multi_score",
        scheduling_presets=(SchedulingPreset(preset="free_team_first", enabled=True, mode="both"),),
    )
    ctx = {
        "all_key_ids": ["k1", "k2", "k3"],
        "lru_scores": {"k1": 100.0, "k2": 100.0, "k3": 100.0},
        "keys_by_id": {
            "k1": _key_with_metadata({"codex": {"plan_type": "plus"}}),
            "k2": _key_with_metadata({"codex": {"plan_type": "free"}}),
            "k3": _key_with_metadata({"codex": {"plan_type": "team"}}),
        },
    }
    s1 = strategy.compute_score(key_id="k1", config=cfg, context=ctx)
    s2 = strategy.compute_score(key_id="k2", config=cfg, context=ctx)
    s3 = strategy.compute_score(key_id="k3", config=cfg, context=ctx)
    assert s1 is not None and s2 is not None and s3 is not None
    assert s2 < s1
    assert s3 < s1


def test_multi_score_preset_free_team_first_free_only_mode() -> None:
    """free_only mode: free is preferred, team is mid-priority."""
    strategy = MultiScoreStrategy()
    cfg = PoolConfig(
        scheduling_mode="multi_score",
        scheduling_presets=(
            SchedulingPreset(preset="free_team_first", enabled=True, mode="free_only"),
        ),
    )
    ctx = {
        "all_key_ids": ["k1", "k2", "k3"],
        "lru_scores": {"k1": 100.0, "k2": 100.0, "k3": 100.0},
        "keys_by_id": {
            "k1": _key_with_metadata({"codex": {"plan_type": "plus"}}),
            "k2": _key_with_metadata({"codex": {"plan_type": "free"}}),
            "k3": _key_with_metadata({"codex": {"plan_type": "team"}}),
        },
    }
    s1 = strategy.compute_score(key_id="k1", config=cfg, context=ctx)
    s2 = strategy.compute_score(key_id="k2", config=cfg, context=ctx)
    s3 = strategy.compute_score(key_id="k3", config=cfg, context=ctx)
    assert s1 is not None and s2 is not None and s3 is not None
    # free < team < plus
    assert s2 < s3 < s1


def test_multi_score_preset_free_team_first_team_only_mode() -> None:
    """team_only mode: team is preferred, free is mid-priority."""
    strategy = MultiScoreStrategy()
    cfg = PoolConfig(
        scheduling_mode="multi_score",
        scheduling_presets=(
            SchedulingPreset(preset="free_team_first", enabled=True, mode="team_only"),
        ),
    )
    ctx = {
        "all_key_ids": ["k1", "k2", "k3"],
        "lru_scores": {"k1": 100.0, "k2": 100.0, "k3": 100.0},
        "keys_by_id": {
            "k1": _key_with_metadata({"codex": {"plan_type": "plus"}}),
            "k2": _key_with_metadata({"codex": {"plan_type": "free"}}),
            "k3": _key_with_metadata({"codex": {"plan_type": "team"}}),
        },
    }
    s1 = strategy.compute_score(key_id="k1", config=cfg, context=ctx)
    s2 = strategy.compute_score(key_id="k2", config=cfg, context=ctx)
    s3 = strategy.compute_score(key_id="k3", config=cfg, context=ctx)
    assert s1 is not None and s2 is not None and s3 is not None
    # team < free < plus
    assert s3 < s2 < s1


def test_multi_score_preset_recent_refresh_prefers_nearer_reset() -> None:
    strategy = MultiScoreStrategy()
    cfg = PoolConfig(
        scheduling_mode="multi_score",
        scheduling_presets=(SchedulingPreset(preset="recent_refresh", enabled=True),),
    )
    ctx = {
        "all_key_ids": ["k1", "k2"],
        "lru_scores": {"k1": 100.0, "k2": 100.0},
        "keys_by_id": {
            "k1": _key_with_metadata({"codex": {"primary_reset_seconds": 600}}),
            "k2": _key_with_metadata({"codex": {"primary_reset_seconds": 120}}),
        },
    }
    s1 = strategy.compute_score(key_id="k1", config=cfg, context=ctx)
    s2 = strategy.compute_score(key_id="k2", config=cfg, context=ctx)
    assert s1 is not None and s2 is not None
    assert s2 < s1


def test_multi_score_preset_single_account_prefers_internal_priority_then_reverse_lru() -> None:
    strategy = MultiScoreStrategy()
    cfg = PoolConfig(
        scheduling_mode="multi_score",
        scheduling_presets=(SchedulingPreset(preset="single_account", enabled=True),),
    )
    ctx = {
        "all_key_ids": ["k1", "k2", "k3"],
        "lru_scores": {"k1": 100.0, "k2": 900.0, "k3": 400.0},
        "keys_by_id": {
            "k1": _key_with_metadata({}, internal_priority=30),
            "k2": _key_with_metadata({}, internal_priority=1),
            "k3": _key_with_metadata({}, internal_priority=10),
        },
    }
    s1 = strategy.compute_score(key_id="k1", config=cfg, context=ctx)
    s2 = strategy.compute_score(key_id="k2", config=cfg, context=ctx)
    s3 = strategy.compute_score(key_id="k3", config=cfg, context=ctx)
    assert s1 is not None and s2 is not None and s3 is not None
    assert s2 < s3 < s1


def test_multi_score_preset_priority_first_prefers_low_internal_priority() -> None:
    strategy = MultiScoreStrategy()
    cfg = PoolConfig(
        scheduling_mode="multi_score",
        scheduling_presets=(SchedulingPreset(preset="priority_first", enabled=True),),
    )
    ctx = {
        "all_key_ids": ["k1", "k2", "k3"],
        "lru_scores": {"k1": 100.0, "k2": 100.0, "k3": 100.0},
        "keys_by_id": {
            "k1": _key_with_metadata({}, internal_priority=20),
            "k2": _key_with_metadata({}, internal_priority=3),
            "k3": _key_with_metadata({}, internal_priority=11),
        },
    }
    s1 = strategy.compute_score(key_id="k1", config=cfg, context=ctx)
    s2 = strategy.compute_score(key_id="k2", config=cfg, context=ctx)
    s3 = strategy.compute_score(key_id="k3", config=cfg, context=ctx)
    assert s1 is not None and s2 is not None and s3 is not None
    assert s2 < s3 < s1


def test_multi_score_lru_disabled_no_blend() -> None:
    """When lru_enabled=False, LRU blend factor should be 0."""
    strategy = MultiScoreStrategy()
    cfg = PoolConfig(
        scheduling_mode="multi_score",
        lru_enabled=False,
        scheduling_presets=(SchedulingPreset(preset="quota_balanced", enabled=True),),
    )
    ctx = {
        "all_key_ids": ["k1", "k2"],
        "lru_scores": {"k1": 100.0, "k2": 200.0},
        "keys_by_id": {
            "k1": _key_with_metadata({"codex": {"primary_used_percent": 80}}),
            "k2": _key_with_metadata({"codex": {"primary_used_percent": 20}}),
        },
    }
    s1 = strategy.compute_score(key_id="k1", config=cfg, context=ctx)
    s2 = strategy.compute_score(key_id="k2", config=cfg, context=ctx)
    assert s1 is not None and s2 is not None
    assert s2 < s1


def test_multi_score_disabled_presets_are_skipped() -> None:
    """Disabled presets should not affect scoring."""
    strategy = MultiScoreStrategy()
    cfg = PoolConfig(
        scheduling_mode="multi_score",
        scheduling_presets=(
            SchedulingPreset(preset="free_team_first", enabled=False),
            SchedulingPreset(preset="quota_balanced", enabled=True),
        ),
    )
    ctx = {
        "all_key_ids": ["k1", "k2"],
        "lru_scores": {"k1": 100.0, "k2": 100.0},
        "keys_by_id": {
            "k1": _key_with_metadata(
                {
                    "codex": {"plan_type": "free", "primary_used_percent": 80},
                }
            ),
            "k2": _key_with_metadata(
                {
                    "codex": {"plan_type": "plus", "primary_used_percent": 20},
                }
            ),
        },
    }
    s1 = strategy.compute_score(key_id="k1", config=cfg, context=ctx)
    s2 = strategy.compute_score(key_id="k2", config=cfg, context=ctx)
    assert s1 is not None and s2 is not None
    # quota_balanced only: k2 (20%) should score lower (better) than k1 (80%)
    # free_team_first is disabled so plan_type should not matter
    assert s2 < s1


def test_multi_score_preset_hard_priority_overrides_later_presets() -> None:
    """Earlier preset should dominate later presets (lexicographic hard priority)."""
    strategy = MultiScoreStrategy()
    cfg = PoolConfig(
        scheduling_mode="multi_score",
        lru_enabled=False,
        scheduling_presets=(
            SchedulingPreset(preset="priority_first", enabled=True),
            SchedulingPreset(preset="quota_balanced", enabled=True),
        ),
    )
    ctx = {
        "all_key_ids": ["k1", "k2", "k3"],
        "lru_scores": {"k1": 100.0, "k2": 100.0, "k3": 100.0},
        "keys_by_id": {
            "k1": _key_with_metadata({"codex": {"primary_used_percent": 90}}, internal_priority=1),
            "k2": _key_with_metadata({"codex": {"primary_used_percent": 10}}, internal_priority=2),
            "k3": _key_with_metadata({"codex": {"primary_used_percent": 50}}, internal_priority=3),
        },
    }
    s1 = strategy.compute_score(key_id="k1", config=cfg, context=ctx)
    s2 = strategy.compute_score(key_id="k2", config=cfg, context=ctx)
    assert s1 is not None and s2 is not None
    # k1 has better priority_first rank even though quota_balanced is worse.
    assert s1 < s2


def test_multi_score_mutex_group_selected_member_uses_group_priority_slot() -> None:
    """Selecting single_account should keep distribution group's first priority slot."""
    strategy = MultiScoreStrategy()
    cfg = PoolConfig(
        scheduling_mode="multi_score",
        lru_enabled=False,
        scheduling_presets=(
            SchedulingPreset(preset="lru", enabled=False),
            SchedulingPreset(preset="quota_balanced", enabled=True),
            SchedulingPreset(preset="single_account", enabled=True),
        ),
    )
    ctx = {
        "all_key_ids": ["k1", "k2", "k3"],
        "lru_scores": {"k1": 100.0, "k2": 100.0, "k3": 100.0},
        "keys_by_id": {
            "k1": _key_with_metadata({"codex": {"primary_used_percent": 90}}, internal_priority=1),
            "k2": _key_with_metadata({"codex": {"primary_used_percent": 10}}, internal_priority=2),
            "k3": _key_with_metadata({"codex": {"primary_used_percent": 20}}, internal_priority=3),
        },
    }
    s1 = strategy.compute_score(key_id="k1", config=cfg, context=ctx)
    s2 = strategy.compute_score(key_id="k2", config=cfg, context=ctx)
    assert s1 is not None and s2 is not None
    # single_account is selected in distribution_mode and should outrank quota_balanced.
    assert s1 < s2
