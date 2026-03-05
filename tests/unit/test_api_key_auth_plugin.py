from __future__ import annotations

from types import SimpleNamespace
from unittest.mock import MagicMock, patch

import pytest

from src.plugins.auth.api_key import ApiKeyAuthPlugin


def _build_request() -> SimpleNamespace:
    return SimpleNamespace(
        headers={"x-api-key": "test-key"},
        client=SimpleNamespace(host="127.0.0.1"),
    )


@pytest.mark.asyncio
async def test_api_key_auth_plugin_uses_standalone_wallet_for_billing() -> None:
    plugin = ApiKeyAuthPlugin()
    request = _build_request()
    db = MagicMock()

    user = SimpleNamespace(id="user-1", username="alice", is_admin=False)
    api_key = SimpleNamespace(id="key-1", name="standalone-key", is_standalone=True)
    wallet = SimpleNamespace(id="wallet-key")

    with (
        patch(
            "src.plugins.auth.api_key.AuthService.authenticate_api_key",
            return_value=(user, api_key),
        ),
        patch(
            "src.plugins.auth.api_key.UsageService.check_request_balance",
            return_value=(True, "OK"),
        ),
        patch("src.plugins.auth.api_key.WalletService.get_wallet", return_value=wallet) as get_wallet,
        patch(
            "src.plugins.auth.api_key.WalletService.serialize_wallet_summary",
            return_value={"id": "wallet-key"},
        ),
    ):
        context = await plugin.authenticate(request, db)

    assert context is not None
    assert context.billing_info["billing"]["id"] == "wallet-key"
    get_wallet.assert_called_once_with(db, api_key_id="key-1")


@pytest.mark.asyncio
async def test_api_key_auth_plugin_uses_user_wallet_for_normal_key_billing() -> None:
    plugin = ApiKeyAuthPlugin()
    request = _build_request()
    db = MagicMock()

    user = SimpleNamespace(id="user-2", username="bob", is_admin=False)
    api_key = SimpleNamespace(id="key-2", name="normal-key", is_standalone=False)
    wallet = SimpleNamespace(id="wallet-user")

    with (
        patch(
            "src.plugins.auth.api_key.AuthService.authenticate_api_key",
            return_value=(user, api_key),
        ),
        patch(
            "src.plugins.auth.api_key.UsageService.check_request_balance",
            return_value=(True, "OK"),
        ),
        patch("src.plugins.auth.api_key.WalletService.get_wallet", return_value=wallet) as get_wallet,
        patch(
            "src.plugins.auth.api_key.WalletService.serialize_wallet_summary",
            return_value={"id": "wallet-user"},
        ),
    ):
        context = await plugin.authenticate(request, db)

    assert context is not None
    assert context.billing_info["billing"]["id"] == "wallet-user"
    get_wallet.assert_called_once_with(db, user_id="user-2")
