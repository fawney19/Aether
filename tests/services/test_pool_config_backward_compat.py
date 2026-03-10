"""Tests for pool config backward compatibility.

Verifies that:
1. Only ``pool_advanced`` key activates pool mode
2. ``claude_code_advanced`` alone does NOT activate pool mode
3. Old import paths via shim modules still resolve correctly
"""

from __future__ import annotations

from src.services.provider.pool.config import PoolConfig, parse_pool_config


def test_pool_advanced_key_activates_pool() -> None:
    """pool_advanced key activates pool mode."""
    cfg = parse_pool_config({"pool_advanced": {"sticky_session_ttl_seconds": 999}})
    assert cfg is not None
    assert cfg.sticky_session_ttl_seconds == 999


def test_claude_code_advanced_alone_does_not_activate_pool() -> None:
    """claude_code_advanced alone does NOT activate pool mode."""
    cfg = parse_pool_config({"claude_code_advanced": {"lru_enabled": False}})
    assert cfg is None


def test_no_config_returns_none() -> None:
    cfg = parse_pool_config({"other": 1})
    assert cfg is None


def test_empty_pool_advanced_returns_defaults() -> None:
    cfg = parse_pool_config({"pool_advanced": {}})
    assert cfg is not None
    defaults = PoolConfig()
    assert cfg.sticky_session_ttl_seconds == defaults.sticky_session_ttl_seconds
    # Default with empty pool_advanced: cache_affinity preset, no LRU
    assert cfg.lru_enabled is False


def test_shim_imports_resolve() -> None:
    """Old import paths through shim modules still work."""
    from src.services.provider.adapters.claude_code.pool_config import PoolConfig as ShimPoolConfig
    from src.services.provider.adapters.claude_code.pool_config import (
        parse_pool_config as shim_parse,
    )
    from src.services.provider.adapters.claude_code.pool_manager import (
        ClaudeCodePoolManager,
    )
    from src.services.provider.pool.manager import PoolManager

    # ShimPoolConfig should be the same class
    assert ShimPoolConfig is PoolConfig
    assert shim_parse is parse_pool_config
    assert ClaudeCodePoolManager is PoolManager
