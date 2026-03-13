from __future__ import annotations

import pytest

from src.api.admin.providers.routes import (
    _merge_claude_code_advanced_config,
    _resolve_new_provider_priority,
    _should_enable_format_conversion_by_default,
)
from src.core.exceptions import InvalidRequestException


def test_claude_code_defaults_format_conversion_enabled() -> None:
    assert _should_enable_format_conversion_by_default("claude_code") is True


def test_custom_defaults_format_conversion_disabled() -> None:
    assert _should_enable_format_conversion_by_default("custom") is False


def test_merge_claude_code_advanced_clears_stale_config_for_non_claude_provider() -> None:
    merged, changed = _merge_claude_code_advanced_config(
        provider_type="custom",
        provider_config={"foo": "bar", "claude_code_advanced": {"max_sessions": 9}},
        claude_code_advanced=None,
        claude_advanced_in_payload=False,
    )

    assert merged == {"foo": "bar"}
    assert changed is True


def test_merge_claude_code_advanced_rejects_non_claude_payload() -> None:
    with pytest.raises(InvalidRequestException):
        _merge_claude_code_advanced_config(
            provider_type="custom",
            provider_config={},
            claude_code_advanced={"max_sessions": 9},
            claude_advanced_in_payload=True,
        )


def test_new_provider_priority_defaults_to_current_top() -> None:
    priority, needs_shift = _resolve_new_provider_priority(
        current_min_priority=3, requested_priority=None
    )
    assert priority == 2
    assert needs_shift is False


def test_new_provider_priority_clamps_at_zero_when_already_topmost() -> None:
    priority, needs_shift = _resolve_new_provider_priority(
        current_min_priority=0, requested_priority=None
    )
    assert priority == 0
    assert needs_shift is True


def test_new_provider_priority_defaults_to_100_when_empty() -> None:
    priority, needs_shift = _resolve_new_provider_priority(
        current_min_priority=None, requested_priority=None
    )
    assert priority == 100
    assert needs_shift is False


def test_new_provider_priority_keeps_explicit_value() -> None:
    priority, needs_shift = _resolve_new_provider_priority(
        current_min_priority=3, requested_priority=8
    )
    assert priority == 8
    assert needs_shift is True
