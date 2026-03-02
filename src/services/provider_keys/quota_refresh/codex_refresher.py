"""
Codex 配额刷新策略。
"""

from __future__ import annotations

import json
from typing import Any

import httpx
from sqlalchemy.orm import Session

from src.api.handlers.base.request_builder import get_provider_auth
from src.core.crypto import crypto_service
from src.models.database import Provider, ProviderAPIKey, ProviderEndpoint
from src.services.provider_keys.auth_type import normalize_auth_type
from src.services.provider_keys.codex_usage_parser import (
    parse_codex_usage_headers,
    parse_codex_wham_usage_response,
)


def _normalize_plan_type(value: Any) -> str | None:
    if not isinstance(value, str):
        return None
    normalized = value.strip().lower()
    return normalized or None


async def refresh_codex_key_quota(
    *,
    db: Session,
    provider: Provider,
    key: ProviderAPIKey,
    endpoint: ProviderEndpoint | None,
    codex_wham_usage_url: str,
    metadata_updates: dict[str, dict],
    state_updates: dict[str, dict],
) -> dict:
    """刷新单个 Codex Key 的限额信息。"""
    _ = db
    if endpoint is None:
        return {
            "key_id": key.id,
            "key_name": key.name,
            "status": "error",
            "message": "找不到有效的 openai:cli 端点",
        }

    # 获取认证信息（用于刷新 OAuth token）
    auth_info = await get_provider_auth(endpoint, key)

    # 构建请求头
    headers: dict[str, Any] = {
        "Accept": "application/json",
    }
    if auth_info:
        headers[auth_info.auth_header] = auth_info.auth_value
    else:
        # 标准 API Key
        decrypted_key = crypto_service.decrypt(key.api_key)
        headers["Authorization"] = f"Bearer {decrypted_key}"

    # 从 auth_config 中解密获取 plan_type 和 account_id
    oauth_plan_type = None
    oauth_account_id = None
    auth_type = normalize_auth_type(getattr(key, "auth_type", "api_key"))
    if auth_type == "oauth" and key.auth_config:
        try:
            decrypted_config = crypto_service.decrypt(key.auth_config)
            auth_config_data = json.loads(decrypted_config)
            if isinstance(auth_config_data, dict):
                oauth_plan_type = _normalize_plan_type(auth_config_data.get("plan_type"))
                raw_account_id = auth_config_data.get("account_id")
                if isinstance(raw_account_id, str):
                    oauth_account_id = raw_account_id.strip() or None
        except Exception:
            pass

    # 如果有 account_id 且不是 free 账号（plan_type 缺失时默认携带，增强兼容性）
    if oauth_account_id and oauth_plan_type != "free":
        headers["chatgpt-account-id"] = oauth_account_id

    # 解析代理配置（key 级别 > provider 级别 > 系统默认）
    from src.services.proxy_node.resolver import (
        build_proxy_client_kwargs,
        resolve_effective_proxy,
    )

    effective_proxy = resolve_effective_proxy(
        getattr(provider, "proxy", None),
        getattr(key, "proxy", None),
    )

    # 使用 wham/usage API 获取限额信息
    async with httpx.AsyncClient(
        **build_proxy_client_kwargs(effective_proxy, timeout=30.0)
    ) as client:
        response = await client.get(codex_wham_usage_url, headers=headers)

    if response.status_code != 200:
        return {
            "key_id": key.id,
            "key_name": key.name,
            "status": "error",
            "message": f"wham/usage API 返回状态码 {response.status_code}",
            "status_code": response.status_code,
        }

    # 解析 JSON 响应
    try:
        data = response.json()
    except Exception:
        return {
            "key_id": key.id,
            "key_name": key.name,
            "status": "error",
            "message": "无法解析 wham/usage API 响应",
        }

    # 解析限额信息
    try:
        metadata = parse_codex_wham_usage_response(data)
    except Exception as exc:
        return {
            "key_id": key.id,
            "key_name": key.name,
            "status": "error",
            "message": f"wham/usage 响应结构异常: {exc}",
            "status_code": response.status_code,
        }

    # 兜底：部分上游场景 body 不含 rate_limit，但响应头携带 x-codex-* 配额字段。
    if not metadata:
        try:
            metadata = parse_codex_usage_headers(response.headers)
        except Exception:
            metadata = None

    if metadata:
        # 收集元数据，稍后统一更新数据库（存储到 codex 子对象）
        metadata_updates[key.id] = {"codex": metadata}
        state_updates[key.id] = {
            "oauth_invalid_at": None,
            "oauth_invalid_reason": None,
        }

        # 判断配额是否耗尽：周限额(primary) 或 5H限额(secondary) 任一剩余为 0
        exhausted_parts: list[str] = []
        weekly_used = metadata.get("primary_used_percent")
        five_hour_used = metadata.get("secondary_used_percent")
        has_quota_signal = isinstance(weekly_used, (int, float)) or isinstance(
            five_hour_used, (int, float)
        )
        if isinstance(weekly_used, (int, float)) and weekly_used >= 100.0 - 1e-6:
            exhausted_parts.append("周限额剩余 0%")
        if isinstance(five_hour_used, (int, float)) and five_hour_used >= 100.0 - 1e-6:
            exhausted_parts.append("5H 限额剩余 0%")

        result: dict[str, Any] = {
            "key_id": key.id,
            "key_name": key.name,
            "status": "success",
            "metadata": metadata,
        }
        if exhausted_parts:
            result["quota_exhausted"] = True
            result["quota_exhausted_reason"] = "Codex " + "，".join(exhausted_parts)
        elif has_quota_signal:
            result["quota_exhausted"] = False

        return result

    # 响应成功但没有限额信息
    return {
        "key_id": key.id,
        "key_name": key.name,
        "status": "no_metadata",
        "message": "响应中未包含限额信息",
        "status_code": response.status_code,
    }
