import os

import pytest

os.environ.setdefault("JWT_SECRET_KEY", "test-secret-key")

from src.plugins.manager import PluginManager


def _build_manager() -> PluginManager:
    return PluginManager(
        config={
            "auth": {"api_key": False},
            "cache": {"memory": False},
            "monitor": {"prometheus": False},
            "token": {"claude": False},
            "load_balancer": {"sticky_priority": False},
        }
    )


def test_rate_limit_defaults_to_token_bucket_when_unconfigured() -> None:
    manager = _build_manager()

    plugin = manager.get_plugin("rate_limit")

    assert plugin is not None
    assert plugin.name == "token_bucket"


@pytest.mark.asyncio
async def test_default_rate_limit_plugin_honors_dynamic_rate_limit() -> None:
    manager = _build_manager()

    plugin = manager.get_plugin("rate_limit")

    assert plugin is not None

    first = await plugin.check_limit("public_ip:test", rate_limit=2)
    assert first.allowed is True
    await plugin.consume("public_ip:test", amount=1, rate_limit=2)

    second = await plugin.check_limit("public_ip:test", rate_limit=2)
    assert second.allowed is True
    await plugin.consume("public_ip:test", amount=1, rate_limit=2)

    third = await plugin.check_limit("public_ip:test", rate_limit=2)
    assert third.allowed is False
    assert third.remaining == 0
    assert third.retry_after is not None
