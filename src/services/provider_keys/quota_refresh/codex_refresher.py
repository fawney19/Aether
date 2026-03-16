"""
Codex 配额刷新策略。
"""

from __future__ import annotations

import json
import time
from datetime import datetime, timezone
from typing import Any

import httpx
from sqlalchemy.orm import Session

from src.core.crypto import crypto_service
from src.models.database import Provider, ProviderAPIKey, ProviderEndpoint
from src.services.provider.auth import get_provider_auth
from src.services.provider.oauth_token import looks_like_token_invalidated
from src.services.provider.pool.account_state import (
    OAUTH_ACCOUNT_BLOCK_PREFIX,
    OAUTH_EXPIRED_PREFIX,
    OAUTH_REQUEST_FAILED_PREFIX,
)
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


def _build_quota_exhausted_fallback_metadata(plan_type: str | None) -> dict[str, Any]:
    """Build conservative Codex quota metadata when wham/usage returns 402."""
    normalized_plan = _normalize_plan_type(plan_type)
    metadata: dict[str, Any] = {"updated_at": int(time.time())}
    if normalized_plan:
        metadata["plan_type"] = normalized_plan
    # primary_* = weekly, secondary_* = 5H (aligned with parser semantics)
    metadata["primary_used_percent"] = 100.0
    if normalized_plan != "free":
        metadata["secondary_used_percent"] = 100.0
    return metadata


def _extract_error_message_from_response(response: httpx.Response) -> str:
    """Best-effort extraction of upstream error message for diagnostics."""
    try:
        payload = response.json()
        if isinstance(payload, dict):
            err = payload.get("error")
            if isinstance(err, dict):
                message = str(err.get("message", "")).strip()
                if message:
                    return message
            if isinstance(err, str) and err.strip():
                return err.strip()
            message = str(payload.get("message", "")).strip()
            if message:
                return message
    except Exception:
        pass
    text = str(getattr(response, "text", "") or "").strip()
    return text[:300] if text else ""


def _looks_like_account_deactivated(message: str | None) -> bool:
    lowered = str(message or "").strip().lower()
    return "account has been deactivated" in lowered or "account deactivated" in lowered


def _looks_like_workspace_deactivated(message: str | None) -> bool:
    lowered = str(message or "").strip().lower()
    return "deactivated_workspace" in lowered or (
        "workspace" in lowered and "deactivated" in lowered
    )


def _build_structured_invalid_reason(*, status_code: int, upstream_message: str | None) -> str:
    message = str(upstream_message or "").strip()

    if status_code == 402 and _looks_like_workspace_deactivated(message):
        return f"{OAUTH_ACCOUNT_BLOCK_PREFIX}工作区已停用 (deactivated_workspace)"

    if _looks_like_account_deactivated(message):
        detail = message or "OpenAI 账号已停用"
        return f"{OAUTH_ACCOUNT_BLOCK_PREFIX}{detail}"

    # Codex 某些场景会返回 403，但语义仍是 access token 已失效/被轮换。
    # 这类异常可通过 refresh_token 恢复，不应落成账号级 block。
    if looks_like_token_invalidated(message):
        detail = message or "Codex Token 无效或已过期"
        return f"{OAUTH_EXPIRED_PREFIX}{detail}"

    if status_code == 401:
        detail = message or "Codex Token 无效或已过期 (401)"
        return f"{OAUTH_EXPIRED_PREFIX}{detail}"

    if status_code == 403:
        detail = message or "Codex 账户访问受限 (403)"
        return f"{OAUTH_ACCOUNT_BLOCK_PREFIX}{detail}"

    return message


def _build_soft_request_failure_reason(*, status_code: int, upstream_message: str | None) -> str:
    detail = str(upstream_message or "").strip() or f"Codex 请求失败 ({status_code})"
    return f"{OAUTH_REQUEST_FAILED_PREFIX}{detail}"


def _get_current_invalid_reason(key: ProviderAPIKey) -> str:
    return str(getattr(key, "oauth_invalid_reason", None) or "").strip()


def _merge_invalid_reason(current: str, candidate_reason: str) -> str:
    if not current:
        return candidate_reason
    if current.startswith(OAUTH_ACCOUNT_BLOCK_PREFIX):
        return current
    if current.startswith(OAUTH_EXPIRED_PREFIX) and candidate_reason.startswith(
        OAUTH_REQUEST_FAILED_PREFIX
    ):
        return current
    return candidate_reason


def _build_invalid_state_update(
    key: ProviderAPIKey,
    *,
    candidate_reason: str,
) -> dict[str, Any]:
    current_reason = _get_current_invalid_reason(key)
    merged_reason = _merge_invalid_reason(current_reason, candidate_reason)
    if merged_reason == current_reason:
        return {
            "oauth_invalid_at": getattr(key, "oauth_invalid_at", None),
            "oauth_invalid_reason": merged_reason,
        }
    return {
        "oauth_invalid_at": datetime.now(timezone.utc),
        "oauth_invalid_reason": merged_reason,
    }


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
        status_code = int(response.status_code)
        err_msg = _extract_error_message_from_response(response)

        header_quota = parse_codex_usage_headers(dict(response.headers) if response.headers else {})
        if isinstance(header_quota, dict) and header_quota:
            metadata_updates[key.id] = {"codex": header_quota}

        if status_code == 401:
            state_updates[key.id] = _build_invalid_state_update(
                key,
                candidate_reason=_build_structured_invalid_reason(
                    status_code=401,
                    upstream_message=err_msg,
                ),
            )
            return {
                "key_id": key.id,
                "key_name": key.name,
                "status": "auth_invalid",
                "message": f"wham/usage API 返回状态码 401{f': {err_msg}' if err_msg else ''}",
                "status_code": 401,
                "auto_disabled": False,
            }

        if status_code == 402:
            if _looks_like_workspace_deactivated(err_msg):
                codex_meta = metadata_updates.get(key.id, {}).get("codex")
                if not isinstance(codex_meta, dict):
                    codex_meta = {}
                codex_meta = {
                    **codex_meta,
                    "updated_at": int(time.time()),
                    "account_disabled": True,
                    "reason": "deactivated_workspace",
                    "message": err_msg or "deactivated_workspace",
                }
                if oauth_plan_type and not codex_meta.get("plan_type"):
                    codex_meta["plan_type"] = oauth_plan_type
                metadata_updates[key.id] = {"codex": codex_meta}
                state_updates[key.id] = _build_invalid_state_update(
                    key,
                    candidate_reason=_build_structured_invalid_reason(
                        status_code=402,
                        upstream_message=err_msg,
                    ),
                )
                return {
                    "key_id": key.id,
                    "key_name": key.name,
                    "status": "workspace_deactivated",
                    "message": f"wham/usage API 返回状态码 402{f': {err_msg}' if err_msg else ''}",
                    "status_code": 402,
                }

            if key.id not in metadata_updates:
                metadata_updates[key.id] = {
                    "codex": _build_quota_exhausted_fallback_metadata(oauth_plan_type)
                }
            state_updates[key.id] = {
                "oauth_invalid_at": None,
                "oauth_invalid_reason": None,
                "is_active": True,
            }
            return {
                "key_id": key.id,
                "key_name": key.name,
                "status": "quota_exhausted",
                "message": f"wham/usage API 返回状态码 402{f': {err_msg}' if err_msg else ''}",
                "status_code": 402,
            }

        if status_code == 403:
            candidate_reason = _build_structured_invalid_reason(
                status_code=403,
                upstream_message=err_msg,
            )
            if not looks_like_token_invalidated(err_msg):
                candidate_reason = _build_soft_request_failure_reason(
                    status_code=403,
                    upstream_message=err_msg,
                )
            state_updates[key.id] = _build_invalid_state_update(
                key,
                candidate_reason=candidate_reason,
            )
            return {
                "key_id": key.id,
                "key_name": key.name,
                "status": "forbidden",
                "message": f"wham/usage API 返回状态码 403{f': {err_msg}' if err_msg else ''}",
                "status_code": 403,
                "auto_disabled": False,
            }

        return {
            "key_id": key.id,
            "key_name": key.name,
            "status": "error",
            "message": (
                f"wham/usage API 返回状态码 {status_code}{f': {err_msg}' if err_msg else ''}"
            ),
            "status_code": status_code,
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

    if metadata:
        # 收集元数据，稍后统一更新数据库（存储到 codex 子对象）
        metadata_updates[key.id] = {"codex": metadata}
        state_updates[key.id] = {
            "oauth_invalid_at": None,
            "oauth_invalid_reason": None,
            "is_active": True,
        }
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
        "status_code": response.status_code,
    }
