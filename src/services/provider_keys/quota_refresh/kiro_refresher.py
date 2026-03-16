"""
Kiro 配额刷新策略。
"""

from __future__ import annotations

import json
import time
from datetime import datetime, timezone

from sqlalchemy.orm import Session

from src.core.crypto import crypto_service
from src.core.logger import logger
from src.models.database import Provider, ProviderAPIKey, ProviderEndpoint


async def refresh_kiro_key_quota(
    *,
    db: Session,
    provider: Provider,
    key: ProviderAPIKey,
    endpoint: ProviderEndpoint | None,
    codex_wham_usage_url: str,
    metadata_updates: dict[str, dict],
    state_updates: dict[str, dict],
) -> dict:
    """刷新单个 Kiro Key 的配额信息。"""
    _ = db
    _ = endpoint
    _ = codex_wham_usage_url

    from src.services.provider.adapters.kiro.usage import (
        KiroAccountBannedException,
    )
    from src.services.provider.adapters.kiro.usage import (
        fetch_kiro_usage_limits as _fetch_kiro_usage_limits,
    )
    from src.services.provider.adapters.kiro.usage import (
        parse_kiro_usage_response as _parse_kiro_usage_response,
    )

    # Kiro: 直接使用 auth_config 调用 getUsageLimits API
    if not key.auth_config:
        return {
            "key_id": key.id,
            "key_name": key.name,
            "status": "error",
            "message": "缺少 Kiro 认证配置 (auth_config)",
        }

    # 解密 auth_config
    try:
        decrypted_config = crypto_service.decrypt(key.auth_config)
        auth_config_data = json.loads(decrypted_config)
    except Exception:
        return {
            "key_id": key.id,
            "key_name": key.name,
            "status": "error",
            "message": "无法解密 auth_config，可能是加密密钥已更改",
        }

    # 获取代理配置（key 级别 > provider 级别）
    from src.services.proxy_node.resolver import resolve_effective_proxy

    proxy_config = resolve_effective_proxy(
        getattr(provider, "proxy", None),
        getattr(key, "proxy", None),
    )

    # 调用 Kiro getUsageLimits API
    try:
        result = await _fetch_kiro_usage_limits(
            auth_config=auth_config_data,
            proxy_config=proxy_config,
        )
    except KiroAccountBannedException as e:
        # 账户被封禁，自动停用并标记
        state_updates[key.id] = {
            "is_active": False,
            "oauth_invalid_at": datetime.now(timezone.utc),
            "oauth_invalid_reason": f"账户已封禁: {e.reason or e.message}",
        }
        # 更新 upstream_metadata 标记封禁状态
        metadata_updates[key.id] = {
            "kiro": {
                "is_banned": True,
                "ban_reason": e.reason or e.message,
                "banned_at": int(time.time()),
                "updated_at": int(time.time()),
            }
        }
        logger.warning(
            "[KIRO_QUOTA] Key {} 账户已封禁，已自动停用: {}",
            key.id,
            e.reason or e.message,
        )
        return {
            "key_id": key.id,
            "key_name": key.name,
            "status": "banned",
            "message": f"账户已封禁: {e.reason or e.message}",
            "is_banned": True,
            "auto_disabled": True,
        }
    except RuntimeError as e:
        error_msg = str(e)
        # 检查是否需要标记账号异常
        if "401" in error_msg or "认证失败" in error_msg:
            state_updates[key.id] = {
                "is_active": False,
                "oauth_invalid_at": datetime.now(timezone.utc),
                "oauth_invalid_reason": "Kiro Token 无效或已过期",
            }
            logger.warning("[KIRO_QUOTA] Key {} Token 无效，已标记为异常并自动停用", key.id)
        return {
            "key_id": key.id,
            "key_name": key.name,
            "status": "error",
            "message": error_msg,
        }

    usage_data = result.get("usage_data")
    updated_auth_config = result.get("updated_auth_config")

    # 解析限额信息
    metadata = _parse_kiro_usage_response(usage_data)

    if metadata:
        # 刷新成功时清除之前的封禁标记（如果账户已恢复）
        metadata["is_banned"] = False
        metadata["ban_reason"] = None
        metadata["banned_at"] = None
        # 收集元数据，稍后统一更新数据库（存储到 kiro 子对象）
        metadata_updates[key.id] = {"kiro": metadata}
        state_updates[key.id] = {
            "oauth_invalid_at": None,
            "oauth_invalid_reason": None,
            "is_active": True,
        }

        # 如果 auth_config 有更新（例如 token 刷新），也需要更新
        if updated_auth_config:
            try:
                new_auth_config_json = json.dumps(updated_auth_config)
                state_updates[key.id]["auth_config"] = crypto_service.encrypt(new_auth_config_json)
            except Exception as exc:
                logger.warning("更新 auth_config 失败 (key={}): {}", key.id, exc)

        return {
            "key_id": key.id,
            "key_name": key.name,
            "status": "success",
            "metadata": metadata,
        }

    # 响应成功但没有限额信息
    return {
        "key_id": key.id,
        "key_name": key.name,
        "status": "no_metadata",
        "message": "响应中未包含限额信息",
    }
