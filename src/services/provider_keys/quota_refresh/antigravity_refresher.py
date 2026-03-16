"""
Antigravity 配额刷新策略。
"""

from __future__ import annotations

import time
from datetime import datetime, timezone

from sqlalchemy.orm import Session

from src.core.logger import logger
from src.models.database import Provider, ProviderAPIKey, ProviderEndpoint
from src.services.provider.auth import get_provider_auth


async def refresh_antigravity_key_quota(
    *,
    db: Session,
    provider: Provider,
    key: ProviderAPIKey,
    endpoint: ProviderEndpoint | None,
    codex_wham_usage_url: str,
    metadata_updates: dict[str, dict],
    state_updates: dict[str, dict],
) -> dict:
    """刷新单个 Antigravity Key 的配额信息。"""
    _ = db
    _ = codex_wham_usage_url
    if endpoint is None:
        return {
            "key_id": key.id,
            "key_name": key.name,
            "status": "error",
            "message": "找不到有效的 gemini:chat/gemini:cli 端点",
        }

    # 直接调用 /v1internal:fetchAvailableModels 获取 quotaInfo，无需发送真实对话请求
    auth_info = await get_provider_auth(endpoint, key)
    if not auth_info:
        return {
            "key_id": key.id,
            "key_name": key.name,
            "status": "error",
            "message": "缺少 OAuth 认证信息，请先授权/刷新 Token",
        }

    access_token = str(auth_info.auth_value).removeprefix("Bearer ").strip()

    from src.services.model.upstream_fetcher import (
        UpstreamModelsFetchContext,
        fetch_models_for_key,
    )
    from src.services.provider.adapters.antigravity.client import (
        AntigravityAccountForbiddenException,
    )
    from src.services.proxy_node.resolver import resolve_effective_proxy

    effective_proxy = resolve_effective_proxy(
        getattr(provider, "proxy", None),
        getattr(key, "proxy", None),
    )

    fetch_ctx = UpstreamModelsFetchContext(
        provider_type="antigravity",
        api_key_value=access_token,
        # antigravity fetcher 不依赖 endpoint mapping
        format_to_endpoint={},
        proxy_config=effective_proxy,
        auth_config=auth_info.decrypted_auth_config,
    )

    try:
        _models, errors, ok, upstream_meta = await fetch_models_for_key(
            fetch_ctx, timeout_seconds=10.0
        )
    except AntigravityAccountForbiddenException as e:
        # 对齐 AM：所有 403 一律标记 is_forbidden 并停用
        state_updates[key.id] = {
            "is_active": False,
            "oauth_invalid_at": datetime.now(timezone.utc),
            "oauth_invalid_reason": f"账户访问被禁止: {e.reason or e.message}",
        }
        # 更新 upstream_metadata 标记封禁状态
        metadata_updates[key.id] = {
            "antigravity": {
                "is_forbidden": True,
                "forbidden_reason": e.reason or e.message,
                "forbidden_at": int(time.time()),
                "updated_at": int(time.time()),
            }
        }
        logger.warning(
            "[ANTIGRAVITY_QUOTA] Key {} 账户访问被禁止，已自动停用: {}",
            key.id,
            e.reason or e.message,
        )
        return {
            "key_id": key.id,
            "key_name": key.name,
            "status": "forbidden",
            "message": f"账户访问被禁止: {e.reason or e.message}",
            "is_forbidden": True,
            "auto_disabled": True,
        }

    if ok and upstream_meta:
        # 刷新成功时清除之前的封禁标记（如果账户已恢复）
        if "antigravity" in upstream_meta:
            upstream_meta["antigravity"]["is_forbidden"] = False
            upstream_meta["antigravity"]["forbidden_reason"] = None
            upstream_meta["antigravity"]["forbidden_at"] = None
        metadata_updates[key.id] = upstream_meta
        state_updates[key.id] = {
            "oauth_invalid_at": None,
            "oauth_invalid_reason": None,
            "is_active": True,
        }
        return {
            "key_id": key.id,
            "key_name": key.name,
            "status": "success",
            "metadata": upstream_meta,
        }

    if ok and not upstream_meta:
        return {
            "key_id": key.id,
            "key_name": key.name,
            "status": "no_metadata",
            "message": "响应中未包含配额信息",
        }

    error_msg = "; ".join(errors) if errors else "fetchAvailableModels failed"
    return {
        "key_id": key.id,
        "key_name": key.name,
        "status": "error",
        "message": error_msg,
    }
