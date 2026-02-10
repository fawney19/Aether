"""Provider OAuth token helpers.

These helpers are for *upstream Provider* OAuth keys (ProviderAPIKey.auth_type == "oauth"),
not for user-login OAuth.

Why:
- Request path uses `get_provider_auth()` which may refresh the access_token lazily.
- Some background/admin paths (model fetch/query, etc.) need the same behavior but must
  avoid sharing a SQLAlchemy Session across concurrent async tasks.

Strategy:
- Run `get_provider_auth()` on a detached key-like object (no DB session held during HTTP).
- If refresh updated encrypted fields, persist them back to DB in a short transaction.
"""

from __future__ import annotations

from dataclasses import dataclass
from types import SimpleNamespace
from typing import Any

from src.core.logger import logger
from src.database import create_session
from src.models.database import ProviderAPIKey

# ---------------------------------------------------------------------------
# Account-level block 结构化标记
# ---------------------------------------------------------------------------
# oauth_invalid_reason 以此前缀开头的，属于"账号级别"异常（如 Google 要求验证账号）；
# 刷新 token 无法修复，必须由用户手动解决后再由管理员手动清除。
# 其余 reason 属于 token 级别异常，成功刷新 token 后自动清除。
OAUTH_ACCOUNT_BLOCK_PREFIX = "[ACCOUNT_BLOCK] "


def is_account_level_block(reason: str | None) -> bool:
    """判断 oauth_invalid_reason 是否属于账号级别的 block（刷新 token 无法修复）。"""
    if not reason:
        return False
    return str(reason).startswith(OAUTH_ACCOUNT_BLOCK_PREFIX)


@dataclass(frozen=True, slots=True)
class OAuthAccessTokenResult:
    access_token: str
    decrypted_auth_config: dict[str, Any] | None
    refreshed: bool


async def resolve_oauth_access_token(
    *,
    key_id: str,
    encrypted_api_key: str,
    encrypted_auth_config: str | None,
    provider_proxy_config: dict[str, Any] | None = None,
    endpoint_api_format: str | None = None,
) -> OAuthAccessTokenResult:
    """Resolve (and lazily refresh) OAuth access_token for a ProviderAPIKey.

    This helper is safe to call from concurrent async tasks because it does not
    rely on the caller's SQLAlchemy Session:
    - It runs refresh logic without an ORM session.
    - If refresh succeeded (encrypted fields changed), it persists the new encrypted
      values to DB using a short, independent session.
    """

    # Local import to avoid circular imports during app startup.
    from src.api.handlers.base.request_builder import get_provider_auth

    # Build detached key-like objects for get_provider_auth().
    provider_obj = (
        SimpleNamespace(proxy=provider_proxy_config) if provider_proxy_config is not None else None
    )
    endpoint_obj = SimpleNamespace(api_format=str(endpoint_api_format or ""))
    key_obj = SimpleNamespace(
        id=str(key_id),
        auth_type="oauth",
        api_key=encrypted_api_key,
        auth_config=encrypted_auth_config,
        provider=provider_obj,
    )

    orig_api_key = key_obj.api_key
    orig_auth_config = key_obj.auth_config

    auth_info = await get_provider_auth(endpoint_obj, key_obj)  # type: ignore[arg-type]
    if auth_info is None:
        # Should not happen for auth_type="oauth", but keep defensive.
        return OAuthAccessTokenResult(access_token="", decrypted_auth_config=None, refreshed=False)

    access_token = str(auth_info.auth_value or "").removeprefix("Bearer ").strip()
    refreshed = (key_obj.api_key != orig_api_key) or (key_obj.auth_config != orig_auth_config)

    if refreshed:
        # Persist refreshed token/config back to DB.
        try:
            with create_session() as db:
                row = db.query(ProviderAPIKey).filter(ProviderAPIKey.id == str(key_id)).first()
                if row is not None:
                    row.api_key = key_obj.api_key
                    row.auth_config = key_obj.auth_config
                    # Refresh succeeded => clear token-level invalid markers.
                    # Preserve account-level blocks (刷新 token 无法修复).
                    if not is_account_level_block(getattr(row, "oauth_invalid_reason", None)):
                        row.oauth_invalid_at = None
                        row.oauth_invalid_reason = None
                        row.is_active = True
                    db.commit()
        except Exception as e:
            # Don't fail caller path; token is still usable for this request.
            logger.debug("[OAUTH_REFRESH] persist refreshed token failed for key {}: {}", key_id, e)

    return OAuthAccessTokenResult(
        access_token=access_token,
        decrypted_auth_config=auth_info.decrypted_auth_config,
        refreshed=refreshed,
    )


__all__ = ["OAuthAccessTokenResult", "resolve_oauth_access_token"]
