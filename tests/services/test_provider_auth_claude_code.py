from __future__ import annotations

from types import SimpleNamespace
from unittest.mock import patch

import pytest

from src.services.provider.auth import get_provider_auth


@pytest.mark.asyncio
async def test_get_provider_auth_claude_code_api_key_uses_x_api_key_header() -> None:
    endpoint = SimpleNamespace(provider=SimpleNamespace(provider_type="claude_code"))
    key = SimpleNamespace(
        auth_type="api_key",
        api_key="enc_key",
        auth_config=None,
        provider=SimpleNamespace(provider_type="claude_code"),
    )

    with patch("src.services.provider.auth.crypto_service.decrypt", return_value="sk-ant-test"):
        auth = await get_provider_auth(endpoint, key)  # type: ignore[arg-type]

    assert auth is not None
    assert auth.auth_header == "x-api-key"
    assert auth.auth_value == "sk-ant-test"
    assert auth.decrypted_auth_config == {
        "provider_type": "claude_code",
        "auth_method": "api_key",
    }


@pytest.mark.asyncio
async def test_get_provider_auth_non_claude_code_api_key_returns_none() -> None:
    endpoint = SimpleNamespace(provider=SimpleNamespace(provider_type="custom"))
    key = SimpleNamespace(
        auth_type="api_key",
        api_key="enc_key",
        auth_config=None,
        provider=SimpleNamespace(provider_type="custom"),
    )

    auth = await get_provider_auth(endpoint, key)  # type: ignore[arg-type]
    assert auth is None
