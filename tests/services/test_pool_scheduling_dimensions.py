"""Tests for pool scheduling dimension evaluation."""

from __future__ import annotations

from src.services.provider.pool.scheduling_dimensions import (
    PoolSchedulingDimensionResult,
    PoolSchedulingSnapshot,
    evaluate_pool_scheduling_dimensions,
    list_pool_scheduling_dimensions,
    summarize_pool_scheduling_dimensions,
)


def _snapshot(**overrides: object) -> PoolSchedulingSnapshot:
    base = {
        "is_active": True,
        "cooldown_reason": None,
        "cooldown_ttl_seconds": None,
        "circuit_breaker_open": False,
        "cost_window_usage": 1200,
        "cost_limit": 10000,
        "cost_soft_threshold_percent": 80,
        "health_score": 0.95,
    }
    base.update(overrides)
    return PoolSchedulingSnapshot(**base)  # type: ignore[arg-type]


def test_default_dimension_registry_contains_core_dimensions() -> None:
    names = list_pool_scheduling_dimensions()
    assert names == ("account_state", "manual", "cooldown", "circuit", "cost", "latency", "health")


def test_summary_available_when_all_dimensions_ok() -> None:
    dimensions = evaluate_pool_scheduling_dimensions(_snapshot())
    summary = summarize_pool_scheduling_dimensions(dimensions)

    assert summary.status == "available"
    assert summary.reason == "available"
    assert summary.candidate_eligible is True
    assert summary.blocked_count == 0
    assert summary.degraded_count == 0


def test_summary_blocked_when_manual_disabled() -> None:
    dimensions = evaluate_pool_scheduling_dimensions(_snapshot(is_active=False))
    summary = summarize_pool_scheduling_dimensions(dimensions)

    assert summary.status == "blocked"
    assert summary.reason == "manual_disabled"
    assert summary.candidate_eligible is False
    assert summary.blocked_count >= 1


def test_summary_blocked_when_account_state_blocked() -> None:
    dimensions = evaluate_pool_scheduling_dimensions(
        _snapshot(
            account_blocked=True,
            account_block_label="账号封禁",
            account_block_reason="account suspended",
        )
    )
    summary = summarize_pool_scheduling_dimensions(dimensions)

    assert summary.status == "blocked"
    assert summary.reason == "account_banned"
    assert summary.candidate_eligible is False
    assert summary.blocked_count >= 1


def test_account_state_takes_priority_over_manual_disabled() -> None:
    dimensions = evaluate_pool_scheduling_dimensions(
        _snapshot(
            is_active=False,
            account_blocked=True,
            account_block_label="访问受限",
            account_block_reason="forbidden",
        )
    )
    summary = summarize_pool_scheduling_dimensions(dimensions)

    assert summary.status == "blocked"
    assert summary.reason == "account_forbidden"


def test_summary_degraded_when_cost_reaches_soft_threshold() -> None:
    dimensions = evaluate_pool_scheduling_dimensions(
        _snapshot(cost_window_usage=8200, cost_limit=10000, cost_soft_threshold_percent=80)
    )
    summary = summarize_pool_scheduling_dimensions(dimensions)

    assert summary.status == "degraded"
    assert summary.reason == "cost_soft"
    assert summary.candidate_eligible is True
    assert summary.blocked_count == 0
    assert summary.degraded_count >= 1


def test_summary_blocked_on_cooldown_even_if_other_dimensions_ok() -> None:
    dimensions = evaluate_pool_scheduling_dimensions(
        _snapshot(cooldown_reason="rate_limited_429", cooldown_ttl_seconds=120)
    )
    summary = summarize_pool_scheduling_dimensions(dimensions)

    assert summary.status == "blocked"
    assert summary.reason == "cooldown"
    assert summary.candidate_eligible is False
    assert summary.blocked_count >= 1


def test_empty_summary_defaults_to_available() -> None:
    summary = summarize_pool_scheduling_dimensions([])
    assert summary.status == "available"
    assert summary.reason == "available"


def test_dimension_result_keeps_degraded_health_details() -> None:
    dimensions = evaluate_pool_scheduling_dimensions(_snapshot(health_score=0.65))
    health = next((item for item in dimensions if item.code == "health_degraded"), None)
    assert isinstance(health, PoolSchedulingDimensionResult)
    assert health.status == "degraded"
    assert health.detail == "0.65"


def test_latency_dimension_degraded_when_latency_high() -> None:
    dimensions = evaluate_pool_scheduling_dimensions(_snapshot(latency_avg_ms=3200))
    latency = next((item for item in dimensions if item.code == "latency_high"), None)
    assert isinstance(latency, PoolSchedulingDimensionResult)
    assert latency.status == "degraded"
    assert latency.detail == "3200ms"
