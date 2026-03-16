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

# 上游返回 "token 已失效" 语义的关键词（小写匹配）。
# 被 codex_refresher 前向分类和 oauth_token 回溯清理共用。
TOKEN_INVALIDATED_KEYWORDS: tuple[str, ...] = (
    "authentication token has been invalidated",
    "token has been invalidated",
)

# 回溯清理专用：历史写入的中文 reason 也需匹配
_LEGACY_TOKEN_INVALID_KEYWORDS: tuple[str, ...] = (
    *TOKEN_INVALIDATED_KEYWORDS,
    "codex token 无效或已过期",
)


def looks_like_token_invalidated(message: str | None) -> bool:
    """判断上游错误消息是否表示 access token 已失效/被轮换。"""
    lowered = str(message or "").strip().lower()
    return any(keyword in lowered for keyword in TOKEN_INVALIDATED_KEYWORDS)


def _is_refresh_recoverable_account_block(reason: str | None) -> bool:
    """历史兼容：部分 token 级异常曾被错误写成 [ACCOUNT_BLOCK]。

    这类原因在手动刷新成功后应自动清除，否则前端会继续展示
    "Token 失效/账号异常"，并阻止 Key 恢复调度。
    """
    if not reason:
        return False
    text = str(reason)
    if not text.startswith(OAUTH_ACCOUNT_BLOCK_PREFIX):
        return False
    lowered = text[len(OAUTH_ACCOUNT_BLOCK_PREFIX) :].strip().lower()
    return any(keyword in lowered for keyword in _LEGACY_TOKEN_INVALID_KEYWORDS)


def is_account_level_block(reason: str | None) -> bool:
    """判断 oauth_invalid_reason 是否属于账号级别的 block（刷新 token 无法修复）。"""
    if not reason:
        return False
    text = str(reason)
    return text.startswith(
        OAUTH_ACCOUNT_BLOCK_PREFIX
    ) and not _is_refresh_recoverable_account_block(text)


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
    from src.services.provider.auth import get_provider_auth

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
                    # Refresh succeeded => clear all invalid markers (including
                    # account-level blocks). A successful token refresh proves the
                    # account is usable; stale block marks should not persist.
                    if row.oauth_invalid_at is not None:
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


__all__ = [
    "OAuthAccessTokenResult",
    "TOKEN_INVALIDATED_KEYWORDS",
    "looks_like_token_invalidated",
    "resolve_oauth_access_token",
]
