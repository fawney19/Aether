"""Tests for pool management scheduling state assembly."""

from __future__ import annotations

from src.api.admin.pool.routes import _build_pool_scheduling_state
from src.services.provider.pool.account_state import resolve_pool_account_state


def test_pool_scheduling_state_manual_disabled_is_blocked() -> None:
    (
        status,
        reason,
        _label,
        reasons,
    ) = _build_pool_scheduling_state(
        is_active=False,
        account_blocked=False,
        account_block_label=None,
        account_block_reason=None,
        latency_avg_ms=None,
        cooldown_reason=None,
        cooldown_ttl_seconds=None,
        circuit_breaker_open=False,
        cost_window_usage=0,
        cost_limit=None,
        cost_soft_threshold_percent=80,
        health_score=1.0,
    )

    assert status == "blocked"
    assert reason == "manual_disabled"
    assert any(item.code == "manual_disabled" for item in reasons)


def test_pool_scheduling_state_cooldown_detail_is_mapped() -> None:
    (
        _status,
        reason,
        _label,
        reasons,
    ) = _build_pool_scheduling_state(
        is_active=True,
        account_blocked=False,
        account_block_label=None,
        account_block_reason=None,
        latency_avg_ms=None,
        cooldown_reason="rate_limited_429",
        cooldown_ttl_seconds=180,
        circuit_breaker_open=False,
        cost_window_usage=0,
        cost_limit=None,
        cost_soft_threshold_percent=80,
        health_score=1.0,
    )

    assert reason == "cooldown"
    cooldown_reason = next(item for item in reasons if item.code == "cooldown")
    assert cooldown_reason.detail == "429 限流"


def test_pool_scheduling_state_cost_soft_is_degraded() -> None:
    (
        status,
        reason,
        _label,
        _reasons,
    ) = _build_pool_scheduling_state(
        is_active=True,
        account_blocked=False,
        account_block_label=None,
        account_block_reason=None,
        latency_avg_ms=None,
        cooldown_reason=None,
        cooldown_ttl_seconds=None,
        circuit_breaker_open=False,
        cost_window_usage=85,
        cost_limit=100,
        cost_soft_threshold_percent=80,
        health_score=1.0,
    )

    assert status == "degraded"
    assert reason == "cost_soft"


def test_known_banned_reason_account_block_prefix() -> None:
    state = resolve_pool_account_state(
        provider_type="codex",
        upstream_metadata={},
        oauth_invalid_reason="[ACCOUNT_BLOCK] Google 要求验证账号",
    )
    assert state.blocked is True


def test_known_banned_key_detects_kiro_banned_metadata() -> None:
    state = resolve_pool_account_state(
        provider_type="kiro",
        upstream_metadata={"kiro": {"is_banned": True}},
        oauth_invalid_reason=None,
    )
    assert state.blocked is True


def test_known_banned_key_detects_reason_keywords() -> None:
    state = resolve_pool_account_state(
        provider_type="antigravity",
        upstream_metadata={},
        oauth_invalid_reason="AWS account temporarily suspended",
    )
    assert state.blocked is True


def test_known_banned_key_does_not_treat_token_expired_as_banned() -> None:
    state = resolve_pool_account_state(
        provider_type="kiro",
        upstream_metadata={"kiro": {"is_banned": False}},
        oauth_invalid_reason="access token expired",
    )
    assert state.blocked is False
