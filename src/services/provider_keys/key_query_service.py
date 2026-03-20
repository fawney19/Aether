"""
Provider Key 查询服务。
"""

from __future__ import annotations

import json
from datetime import datetime, timezone
from typing import Any

from sqlalchemy.orm import Session

from src.core.api_format.signature import normalize_signature_key
from src.core.crypto import crypto_service
from src.core.exceptions import InvalidRequestException, NotFoundException
from src.core.key_capabilities import get_capability
from src.core.logger import logger
from src.models.database import Provider, ProviderAPIKey, ProviderEndpoint
from src.models.endpoint_models import EndpointAPIKeyResponse
from src.services.provider.pool.config import parse_pool_config
from src.services.provider_keys.auth_type import normalize_auth_type
from src.services.provider_keys.response_builder import build_key_response

_LEGACY_API_FORMAT_MAP: dict[str, str] = {
    "CLAUDE": "claude:chat",
    "CLAUDE_CLI": "claude:cli",
    "OPENAI": "openai:chat",
    "OPENAI_CLI": "openai:cli",
    "OPENAI_COMPACT": "openai:compact",
    "OPENAI_VIDEO": "openai:video",
    "GEMINI": "gemini:chat",
    "GEMINI_CLI": "gemini:cli",
    "GEMINI_VIDEO": "gemini:video",
}


def _normalize_api_format_key(raw_format: Any) -> str | None:
    """Normalize api_format to canonical signature key; keeps legacy import compatibility."""
    text = str(raw_format or "").strip()
    if not text:
        return None

    try:
        return normalize_signature_key(text)
    except Exception:
        pass

    legacy = text.upper().replace("-", "_")
    return _LEGACY_API_FORMAT_MAP.get(legacy)


def _normalize_format_dict(raw_dict: Any) -> dict[str, Any]:
    """Normalize dict keys from any format aliases to canonical api_format."""
    if not isinstance(raw_dict, dict):
        return {}

    normalized: dict[str, Any] = {}
    for raw_key, value in raw_dict.items():
        format_key = _normalize_api_format_key(raw_key)
        if not format_key or format_key in normalized:
            continue
        normalized[format_key] = value
    return normalized


def get_keys_grouped_by_format(db: Session) -> dict:
    """查询所有 Key，并按 API 格式分组返回。"""
    # Key 属于 Provider：按 key.api_formats 分组展示
    # 包含所有 Key（含停用的 Key 和停用的 Provider），前端可显示停用标签和快捷开关
    keys = (
        db.query(ProviderAPIKey, Provider)
        .join(Provider, ProviderAPIKey.provider_id == Provider.id)
        .order_by(
            ProviderAPIKey.internal_priority.asc(),
        )
        .all()
    )

    provider_ids = {str(provider.id) for _key, provider in keys}
    endpoints = (
        db.query(
            ProviderEndpoint.provider_id,
            ProviderEndpoint.api_format,
            ProviderEndpoint.base_url,
        )
        .filter(
            ProviderEndpoint.provider_id.in_(provider_ids),
            ProviderEndpoint.is_active.is_(True),
        )
        .all()
    )
    endpoint_base_url_map: dict[tuple[str, str], str] = {}
    for provider_id, api_format, base_url in endpoints:
        fmt = api_format.value if hasattr(api_format, "value") else str(api_format)
        normalized_fmt = _normalize_api_format_key(fmt)
        if not normalized_fmt:
            continue
        endpoint_base_url_map[(str(provider_id), normalized_fmt)] = base_url

    grouped: dict[str, list[dict]] = {}
    for key, provider in keys:
        pool_enabled = parse_pool_config(getattr(provider, "config", None)) is not None
        raw_api_formats = key.api_formats or []
        api_formats: list[str] = []
        seen_formats: set[str] = set()
        for raw_format in raw_api_formats:
            normalized_format = _normalize_api_format_key(raw_format)
            if not normalized_format or normalized_format in seen_formats:
                continue
            seen_formats.add(normalized_format)
            api_formats.append(normalized_format)

        if not api_formats:
            continue  # 跳过没有 API 格式的 Key

        auth_type = normalize_auth_type(getattr(key, "auth_type", "api_key"))
        if auth_type in ("service_account", "vertex_ai"):
            masked_key = "[Service Account]"
        elif auth_type == "oauth":
            masked_key = "[OAuth Token]"
        else:
            try:
                decrypted_key = crypto_service.decrypt(key.api_key)
                masked_key = f"{decrypted_key[:8]}***{decrypted_key[-4:]}"
            except Exception as e:
                logger.error(f"解密 Key 失败: key_id={key.id}, error={e}")
                masked_key = "***ERROR***"

        # 计算健康度指标
        success_rate = key.success_count / key.request_count if key.request_count > 0 else None
        avg_response_time_ms = (
            round(key.total_response_time_ms / key.success_count, 2)
            if key.success_count > 0
            else None
        )

        # 将 capabilities dict 转换为启用的能力简短名称列表
        caps_list = []
        if key.capabilities:
            for cap_name, enabled in key.capabilities.items():
                if enabled:
                    cap_def = get_capability(cap_name)
                    caps_list.append(cap_def.short_name if cap_def else cap_name)

        # 构建 Key 信息（基础数据）
        normalized_rate_multipliers = _normalize_format_dict(key.rate_multipliers)
        normalized_priority_by_format = _normalize_format_dict(key.global_priority_by_format)
        key_info = {
            "id": key.id,
            "provider_id": str(provider.id),
            "name": key.name,
            "auth_type": auth_type,
            "api_key_masked": masked_key,
            "internal_priority": key.internal_priority,
            "global_priority_by_format": normalized_priority_by_format,
            "rate_multipliers": normalized_rate_multipliers or None,
            "is_active": key.is_active,
            "provider_active": provider.is_active,
            "provider_name": provider.name,
            "pool_enabled": pool_enabled,
            "api_formats": api_formats,
            "capabilities": caps_list,
            "success_rate": success_rate,
            "avg_response_time_ms": avg_response_time_ms,
            "request_count": key.request_count,
        }

        # 将 Key 添加到每个支持的格式分组中，并附加格式特定的数据
        health_by_format = _normalize_format_dict(key.health_by_format)
        circuit_by_format = _normalize_format_dict(key.circuit_breaker_by_format)
        priority_by_format: dict[str, int] = {}
        for k, v in normalized_priority_by_format.items():
            try:
                priority_by_format[k] = int(v)
            except Exception:
                continue
        provider_id = str(provider.id)
        for api_format in api_formats:
            if api_format not in grouped:
                grouped[api_format] = []
            # 为每个格式创建副本，设置当前格式
            format_key_info = key_info.copy()
            format_key_info["api_format"] = api_format
            format_key_info["endpoint_base_url"] = endpoint_base_url_map.get(
                (provider_id, api_format)
            )
            # 添加格式特定的优先级
            format_key_info["format_priority"] = priority_by_format.get(api_format)
            # 添加格式特定的健康度数据
            format_health = health_by_format.get(api_format, {})
            format_circuit = circuit_by_format.get(api_format, {})
            format_key_info["health_score"] = float(format_health.get("health_score") or 1.0)
            format_key_info["circuit_breaker_open"] = bool(format_circuit.get("open", False))
            grouped[api_format].append(format_key_info)

    # 直接返回分组对象，供前端使用
    return grouped


def list_provider_keys_responses(
    db: Session,
    provider_id: str,
    skip: int,
    limit: int,
) -> list[EndpointAPIKeyResponse]:
    """查询 Provider 下的 Key 列表并构建响应。"""
    provider = db.query(Provider).filter(Provider.id == provider_id).first()
    if not provider:
        raise NotFoundException(f"Provider {provider_id} 不存在")
    provider_type = (
        str(
            getattr(provider, "provider_type", None) or getattr(provider, "type", None) or ""
        ).strip()
        or None
    )

    keys = (
        db.query(ProviderAPIKey)
        .filter(ProviderAPIKey.provider_id == provider_id)
        .order_by(ProviderAPIKey.internal_priority.asc(), ProviderAPIKey.created_at.asc())
        .offset(skip)
        .limit(limit)
        .all()
    )
    return [build_key_response(key, provider_type=provider_type) for key in keys]


def reveal_endpoint_key_payload(
    db: Session,
    key_id: str,
) -> dict[str, Any]:
    """获取完整的 API Key 或 Auth Config（用于查看和复制）。"""
    key = db.query(ProviderAPIKey).filter(ProviderAPIKey.id == key_id).first()
    if not key:
        raise NotFoundException(f"Key {key_id} 不存在")

    auth_type = normalize_auth_type(getattr(key, "auth_type", "api_key"))

    # Service Account 类型返回 auth_config（需要解密）
    if auth_type in ("service_account", "vertex_ai"):
        encrypted_auth_config = getattr(key, "auth_config", None)
        if encrypted_auth_config:
            try:
                decrypted_config = crypto_service.decrypt(encrypted_auth_config)
                auth_config = json.loads(decrypted_config)
                logger.info(f"[REVEAL] 查看 Auth Config: ID={key_id}, Name={key.name}")
                return {"auth_type": auth_type, "auth_config": auth_config}
            except Exception as e:
                logger.error(f"解密 Auth Config 失败: ID={key_id}, Error={e}")
                raise InvalidRequestException(
                    "无法解密认证配置，可能是加密密钥已更改。请重新添加该密钥。"
                )

        # 兼容：auth_config 为空时尝试从 api_key 解密（仅对迁移前的旧数据有效）
        try:
            decrypted_key = crypto_service.decrypt(key.api_key)
            if decrypted_key == "__placeholder__":
                logger.error(f"Service Account Key 缺少 auth_config: ID={key_id}")
                raise InvalidRequestException("认证配置丢失，请重新添加该密钥。")
            logger.info(f"[REVEAL] 查看完整 Key (legacy SA): ID={key_id}, Name={key.name}")
            return {"auth_type": auth_type, "auth_config": decrypted_key}
        except InvalidRequestException:
            raise
        except Exception as e:
            logger.error(f"解密 Key 失败: ID={key_id}, Error={e}")
            raise InvalidRequestException(
                "无法解密认证配置，可能是加密密钥已更改。请重新添加该密钥。"
            )

    # OAuth 类型：返回 access_token（导出走 /export 端点）
    if auth_type == "oauth":
        try:
            decrypted_key = crypto_service.decrypt(key.api_key)
        except Exception as e:
            logger.error(f"解密 Key 失败: ID={key_id}, Error={e}")
            raise InvalidRequestException(
                "无法解密 API Key，可能是加密密钥已更改。请重新添加该密钥。"
            )
        logger.info(f"[REVEAL] 查看 OAuth Key: ID={key_id}, Name={key.name}")
        return {"auth_type": "oauth", "api_key": decrypted_key}

    # API Key 类型返回 api_key
    try:
        decrypted_key = crypto_service.decrypt(key.api_key)
    except Exception as e:
        logger.error(f"解密 Key 失败: ID={key_id}, Error={e}")
        raise InvalidRequestException("无法解密 API Key，可能是加密密钥已更改。请重新添加该密钥。")

    logger.info(f"[REVEAL] 查看完整 Key: ID={key_id}, Name={key.name}")
    return {"auth_type": "api_key", "api_key": decrypted_key}


def export_oauth_key_data(
    db: Session,
    key_id: str,
) -> dict[str, Any]:
    """导出 OAuth Key 凭据。"""
    from src.services.provider.export import build_export_data

    key = db.query(ProviderAPIKey).filter(ProviderAPIKey.id == key_id).first()
    if not key:
        raise NotFoundException(f"Key {key_id} 不存在")

    auth_type = normalize_auth_type(getattr(key, "auth_type", "api_key"))
    if auth_type != "oauth":
        raise InvalidRequestException("仅 OAuth 类型的 Key 支持导出")

    encrypted_auth_config = getattr(key, "auth_config", None)
    if not encrypted_auth_config:
        raise InvalidRequestException("缺少认证配置，无法导出")

    try:
        auth_config: dict[str, Any] = json.loads(crypto_service.decrypt(encrypted_auth_config))
    except Exception:
        raise InvalidRequestException("无法解密认证配置")

    if not auth_config.get("refresh_token"):
        raise InvalidRequestException("缺少 refresh_token，无法导出")

    provider_type = str(auth_config.get("provider_type") or "").strip()
    upstream = getattr(key, "upstream_metadata", None)

    export_data = build_export_data(provider_type, auth_config, upstream)
    export_data["name"] = key.name or ""
    export_data["exported_at"] = datetime.now(timezone.utc).isoformat()

    logger.info("[EXPORT] Key {}... 导出成功", key_id[:8])
    return export_data
