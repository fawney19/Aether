from __future__ import annotations

from src.api.admin.providers.summary import _extract_claude_code_advanced_from_config
from src.models.admin_requests import ClaudeCodeAdvancedConfig


def test_extract_claude_code_advanced_valid_dict() -> None:
    config = {"claude_code_advanced": {"max_sessions": 12}}

    parsed = _extract_claude_code_advanced_from_config(config, provider_id="provider-1")

    assert isinstance(parsed, ClaudeCodeAdvancedConfig)
    assert parsed.max_sessions == 12
    assert parsed.session_idle_timeout_minutes == 5


def test_extract_claude_code_advanced_invalid_type_returns_none() -> None:
    config = {"claude_code_advanced": "not-a-dict"}

    parsed = _extract_claude_code_advanced_from_config(config, provider_id="provider-1")

    assert parsed is None


def test_extract_claude_code_advanced_invalid_payload_returns_none() -> None:
    config = {"claude_code_advanced": {"max_sessions": 0}}

    parsed = _extract_claude_code_advanced_from_config(config, provider_id="provider-1")

    assert parsed is None
