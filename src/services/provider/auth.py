"""
Provider 认证逻辑（OAuth / Service Account / Vertex AI）。

从 api/handlers/base/request_builder.py 迁移到 services 层，
消除 services→api 的反向依赖。
"""

from __future__ import annotations

import json
import time
from typing import TYPE_CHECKING, Any

from sqlalchemy.orm import object_session

from src.clients.redis_client import get_redis_client
from src.core.crypto import crypto_service
from src.core.logger import logger
from src.core.provider_auth_types import ProviderAuthInfo
from src.core.provider_oauth_utils import enrich_auth_config, post_oauth_token

if TYPE_CHECKING:
    from src.models.database import ProviderAPIKey, ProviderEndpoint


# ==============================================================================
# OAuth Token Refresh helpers
# ==============================================================================


async def _acquire_refresh_lock(key_id: str) -> tuple[Any, bool]:
    """尝试获取 OAuth refresh 分布式锁。

    返回 ``(redis_client | None, got_lock)``。调用方在刷新完成后
    必须调用 :func:`_release_refresh_lock` 释放锁。
    """
    redis = await get_redis_client(require_redis=False)
    lock_key = f"provider_oauth_refresh_lock:{key_id}"
    got_lock = False
    if redis is not None:
        try:
            got_lock = bool(await redis.set(lock_key, "1", ex=30, nx=True))
        except Exception:
            got_lock = False
    return redis, got_lock


async def _release_refresh_lock(redis: Any, key_id: str) -> None:
    """释放 OAuth refresh 分布式锁（best-effort）。"""
    if redis is not None:
        try:
            await redis.delete(f"provider_oauth_refresh_lock:{key_id}")
        except Exception:
            pass


def _persist_refreshed_token(
    key: Any,
    access_token: str,
    token_meta: dict[str, Any],
) -> None:
    """将刷新后的 access_token 和 auth_config 持久化到数据库。"""
    key.api_key = crypto_service.encrypt(access_token)
    key.auth_config = crypto_service.encrypt(json.dumps(token_meta))

    sess = object_session(key)
    if sess is not None:
        sess.add(key)
        sess.commit()
    else:
        logger.warning(
            "[OAUTH_REFRESH] key {} refreshed but cannot persist (no session); "
            "next request will refresh again",
            key.id,
        )


def _get_proxy_config(key: Any, endpoint: Any = None) -> Any:
    """获取有效代理配置（Key 级别优先于 Provider 级别）。"""
    try:
        from src.services.proxy_node.resolver import resolve_effective_proxy

        provider = getattr(key, "provider", None) or (
            getattr(endpoint, "provider", None) if endpoint else None
        )
        provider_proxy = getattr(provider, "proxy", None)
        key_proxy = getattr(key, "proxy", None)
        return resolve_effective_proxy(provider_proxy, key_proxy)
    except Exception:
        return None


# ==============================================================================
# Provider-specific refresh implementations
# ==============================================================================


async def _refresh_kiro_token(
    key: Any,
    endpoint: Any,
    token_meta: dict[str, Any],
) -> dict[str, Any]:
    """Kiro OAuth refresh: validate + call Kiro-specific refresh endpoint."""
    from src.core.exceptions import InvalidRequestException
    from src.services.provider.adapters.kiro.models.credentials import KiroAuthConfig
    from src.services.provider.adapters.kiro.token_manager import (
        refresh_access_token,
        validate_refresh_token,
    )

    cfg = KiroAuthConfig.from_dict(token_meta or {})
    if not (cfg.refresh_token or "").strip():
        raise InvalidRequestException(
            "Kiro auth_config missing refresh_token; please re-import credentials."
        )

    proxy_config = _get_proxy_config(key, endpoint)

    validate_refresh_token(cfg.refresh_token)
    access_token, new_cfg = await refresh_access_token(
        cfg,
        proxy_config=proxy_config,
    )
    new_meta = new_cfg.to_dict()
    new_meta["updated_at"] = int(time.time())

    _persist_refreshed_token(key, access_token, new_meta)
    return new_meta


async def _refresh_generic_oauth_token(
    key: Any,
    endpoint: Any,
    template: Any,
    provider_type: str,
    refresh_token: str,
    token_meta: dict[str, Any],
) -> dict[str, Any]:
    """Generic OAuth refresh via template (Codex, Antigravity, ClaudeCode, etc.)."""
    token_url = template.oauth.token_url
    is_json = "anthropic.com" in token_url

    scopes = getattr(template.oauth, "scopes", None) or []
    scope_str = " ".join(scopes) if scopes else ""

    if is_json:
        body: dict[str, Any] = {
            "grant_type": "refresh_token",
            "client_id": template.oauth.client_id,
            "refresh_token": str(refresh_token),
        }
        if scope_str:
            body["scope"] = scope_str
        headers = {
            "Content-Type": "application/json",
            "Accept": "application/json",
        }
        data = None
        json_body = body
    else:
        form: dict[str, str] = {
            "grant_type": "refresh_token",
            "client_id": template.oauth.client_id,
            "refresh_token": str(refresh_token),
        }
        if scope_str:
            form["scope"] = scope_str
        if template.oauth.client_secret:
            form["client_secret"] = template.oauth.client_secret
        headers = {
            "Content-Type": "application/x-www-form-urlencoded",
            "Accept": "application/json",
        }
        data = form
        json_body = None

    proxy_config = _get_proxy_config(key, endpoint)

    resp = await post_oauth_token(
        provider_type=provider_type,
        token_url=token_url,
        headers=headers,
        data=data,
        json_body=json_body,
        proxy_config=proxy_config,
        timeout_seconds=30.0,
    )

    if 200 <= resp.status_code < 300:
        token = resp.json()
        access_token = str(token.get("access_token") or "")
        new_refresh_token = str(token.get("refresh_token") or "")
        expires_in = token.get("expires_in")
        new_expires_at: int | None = None
        try:
            if expires_in is not None:
                new_expires_at = int(time.time()) + int(expires_in)
        except Exception:
            new_expires_at = None

        if access_token:
            token_meta["token_type"] = token.get("token_type")
            if new_refresh_token:
                token_meta["refresh_token"] = new_refresh_token
            token_meta["expires_at"] = new_expires_at
            token_meta["scope"] = token.get("scope")
            token_meta["updated_at"] = int(time.time())

            token_meta = await enrich_auth_config(
                provider_type=provider_type,
                auth_config=token_meta,
                token_response=token,
                access_token=access_token,
                proxy_config=proxy_config,
            )

            _persist_refreshed_token(key, access_token, token_meta)
    else:
        logger.warning(
            "OAuth token refresh failed: provider={}, key_id={}, status={}",
            provider_type,
            getattr(key, "id", "?"),
            resp.status_code,
        )

    return token_meta


# ==============================================================================
# Service Account 认证支持
# ==============================================================================


async def get_provider_auth(
    endpoint: "ProviderEndpoint",
    key: "ProviderAPIKey",
    *,
    force_refresh: bool = False,
    refresh_skew: int | None = None,
) -> ProviderAuthInfo | None:
    """
    获取 Provider 的认证信息

    对于标准 API Key，返回 None（由 build_headers 自动处理）。
    对于 Service Account，异步获取 Access Token 并返回认证信息。

    Args:
        endpoint: 端点配置
        key: Provider API Key

    Returns:
        Service Account 场景: ProviderAuthInfo 对象（包含认证信息和解密后的配置）
        API Key 场景: None（由 build_headers 处理）

    Raises:
        InvalidRequestException: 认证配置无效或认证失败
    """
    from src.core.exceptions import InvalidRequestException

    auth_type = getattr(key, "auth_type", "api_key")

    if auth_type == "oauth":
        # OAuth token 保存在 key.api_key（加密），refresh_token/expires_at 等在 auth_config（加密 JSON）中。
        # 在请求前做一次懒刷新：接近过期时刷新 access_token，并用 Redis lock 避免并发风暴。

        encrypted_auth_config = getattr(key, "auth_config", None)

        # 先解密 auth_config -- 下游 build_provider_url 等依赖 decrypted_auth_config
        # 中的 provider_type / project_id / region 等元数据，即使 access_token 命中缓存
        # 也不能跳过。
        if encrypted_auth_config:
            try:
                decrypted_config = crypto_service.decrypt(encrypted_auth_config)
                token_meta = json.loads(decrypted_config)
            except Exception:
                token_meta = {}
        else:
            token_meta = {}

        decrypted_auth_config: dict[str, Any] | None = (
            token_meta if isinstance(token_meta, dict) and token_meta else None
        )

        # 快路径：查 Redis token 缓存，命中则跳过 refresh 和 api_key 解密。
        # 注意：token_meta/decrypted_auth_config 已在上方解密，此处只是跳过后续刷新逻辑。
        if not force_refresh and encrypted_auth_config:
            try:
                from src.services.provider.pool.oauth_cache import get_cached_token

                _cached = await get_cached_token(str(key.id))
                if _cached:
                    return ProviderAuthInfo(
                        auth_header="Authorization",
                        auth_value=f"Bearer {_cached}",
                        decrypted_auth_config=decrypted_auth_config,
                    )
            except Exception:
                logger.debug("OAuth token cache lookup failed for key {}", str(key.id)[:8])

        expires_at = token_meta.get("expires_at")
        refresh_token = token_meta.get("refresh_token")
        provider_type = str(token_meta.get("provider_type") or "")
        cached_access_token = str(token_meta.get("access_token") or "").strip()

        # Refresh skew: providers with pool config use configurable
        # proactive_refresh_seconds (default 180 s), others use 120 s.
        # Prefer the caller-supplied value to avoid ORM lazy-load on key.provider.
        _refresh_skew = refresh_skew if refresh_skew is not None else 120
        if refresh_skew is None:
            try:
                from src.services.provider.pool.config import parse_pool_config

                provider_obj = getattr(key, "provider", None)
                pcfg = getattr(provider_obj, "config", None) if provider_obj else None
                pool_cfg = parse_pool_config(pcfg) if pcfg else None
                if pool_cfg is not None:
                    _refresh_skew = pool_cfg.proactive_refresh_seconds
            except Exception:
                pass

        should_refresh = False
        try:
            if expires_at is not None:
                should_refresh = int(time.time()) >= int(expires_at) - _refresh_skew
        except Exception:
            should_refresh = False

        if force_refresh:
            should_refresh = True

        # Kiro 特殊处理：如果没有缓存的 access_token 或 key.api_key 是占位符，强制刷新
        if provider_type == "kiro" and not should_refresh:
            if not cached_access_token:
                should_refresh = True
            elif crypto_service.decrypt(key.api_key) == "__placeholder__":
                should_refresh = True

        _refreshed = False
        if should_refresh and refresh_token and provider_type:
            try:
                from src.core.provider_templates.fixed_providers import FIXED_PROVIDERS
                from src.core.provider_templates.types import ProviderType

                try:
                    template = FIXED_PROVIDERS.get(ProviderType(provider_type))
                except Exception:
                    template = None

                redis, got_lock = await _acquire_refresh_lock(key.id)
                if got_lock or redis is None:
                    try:
                        if provider_type == ProviderType.KIRO.value:
                            token_meta = await _refresh_kiro_token(key, endpoint, token_meta)
                        elif template:
                            token_meta = await _refresh_generic_oauth_token(
                                key, endpoint, template, provider_type, refresh_token, token_meta
                            )
                        _refreshed = True
                    finally:
                        if got_lock:
                            await _release_refresh_lock(redis, key.id)
            except Exception:
                # 刷新失败不阻断请求；后续由上游返回 401 再触发管理端处理
                pass

        # 获取最终使用的 access_token
        # Kiro 优先使用 token_meta 中缓存的 access_token（刷新后会更新到 token_meta）
        if provider_type == "kiro":
            refreshed_token = str(token_meta.get("access_token") or "").strip()
            effective_token = refreshed_token or crypto_service.decrypt(key.api_key)
        else:
            effective_token = crypto_service.decrypt(key.api_key)

        # 刷新成功后写入 Redis token 缓存（所有 OAuth key 均可受益）
        if _refreshed and effective_token:
            try:
                from src.services.provider.pool.oauth_cache import cache_token

                new_expires_at = token_meta.get("expires_at")
                if new_expires_at is not None:
                    remaining = int(new_expires_at) - int(time.time())
                    if remaining > 0:
                        await cache_token(str(key.id), effective_token, remaining)
            except Exception:
                logger.debug("OAuth token cache write failed for key {}", str(key.id)[:8])

        # 刷新可能更新了 token_meta，同步 decrypted_auth_config
        if isinstance(token_meta, dict) and token_meta:
            decrypted_auth_config = token_meta

        return ProviderAuthInfo(
            auth_header="Authorization",
            auth_value=f"Bearer {effective_token}",
            decrypted_auth_config=decrypted_auth_config,
        )
    if auth_type in ("service_account", "vertex_ai"):
        # service_account: GCP Service Account JSON → JWT → Access Token
        # "vertex_ai" 保留为向后兼容（迁移期间旧数据可能仍使用该值）
        from src.services.provider.adapters.vertex_ai.auth import _auth_service_account

        return await _auth_service_account(key)

    # 标准 API Key：返回 None，由 build_headers 处理
    return None
