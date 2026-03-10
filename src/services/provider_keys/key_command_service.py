"""
Provider Key 写操作命令服务。
"""

from __future__ import annotations

import asyncio
import json
import uuid
from dataclasses import dataclass
from datetime import datetime, timezone
from typing import Any

from sqlalchemy import delete as sa_delete
from sqlalchemy.orm import Session

from src.core.crypto import crypto_service
from src.core.exceptions import InvalidRequestException, NotFoundException
from src.core.logger import logger
from src.core.provider_types import ProviderType
from src.models.database import (
    Provider,
    ProviderAPIKey,
)
from src.models.endpoint_models import (
    EndpointAPIKeyCreate,
    EndpointAPIKeyResponse,
    EndpointAPIKeyUpdate,
)
from src.services.provider.fingerprint import generate_fingerprint, normalize_fingerprint
from src.services.provider_keys.auth_type import normalize_auth_type
from src.services.provider_keys.duplicate_check import check_duplicate_key
from src.services.provider_keys.key_side_effects import (
    run_create_key_side_effects,
    run_delete_key_side_effects,
    run_update_key_side_effects,
)
from src.services.provider_keys.response_builder import build_key_response


def _validate_vertex_api_formats(
    provider_type: str | None,
    auth_type: str,
    api_formats: list[str] | None,
) -> None:
    """校验 Vertex Provider 的 key.api_formats 与 auth_type 是否匹配。"""
    if str(provider_type or "").strip().lower() != ProviderType.VERTEX_AI.value:
        return

    formats = [
        str(fmt or "").strip().lower() for fmt in (api_formats or []) if str(fmt or "").strip()
    ]
    if not formats:
        return

    if auth_type == "api_key":
        allowed = {"gemini:chat"}
    elif auth_type in {"service_account", "vertex_ai"}:
        allowed = {"gemini:chat", "claude:chat"}
    else:
        return

    invalid = sorted({fmt for fmt in formats if fmt not in allowed})
    if invalid:
        allowed_text = ", ".join(sorted(allowed))
        invalid_text = ", ".join(invalid)
        raise InvalidRequestException(
            f"Vertex {auth_type} 不支持以下 API 格式: {invalid_text}；允许: {allowed_text}"
        )


@dataclass
class _UpdateKeyPreparation:
    """更新 Key 前置准备结果。"""

    update_data: dict[str, Any]
    auto_fetch_enabled_before: bool
    auto_fetch_enabled_after: bool
    allowed_models_before: set[str]
    include_patterns_before: list[str] | None
    exclude_patterns_before: list[str] | None


@dataclass
class _DeleteKeyResult:
    """删除 Key 的执行结果。"""

    provider_id: str | None
    deleted_key_allowed_models: list[str] | None


def _run_async_with_fallback(coro: Any) -> None:
    """在同步上下文中执行异步任务（有事件循环则调度，无则阻塞执行）。"""
    try:
        asyncio.get_running_loop()
    except RuntimeError:
        asyncio.run(coro)
        return

    from src.utils.async_utils import safe_create_task

    safe_create_task(coro)


async def _invalidate_cache_after_clear_oauth_invalid(key_id: str) -> None:
    """清除 OAuth 失效标记后同步失效相关缓存。"""
    from src.services.cache.model_list_cache import invalidate_models_list_cache
    from src.services.cache.provider_cache import ProviderCacheService

    await ProviderCacheService.invalidate_provider_api_key_cache(key_id)
    await invalidate_models_list_cache()


def _clear_oauth_invalid_marker(db: Session, key_id: str) -> dict[str, str]:
    """清除 Key 的 OAuth 失效标记。"""
    key = db.query(ProviderAPIKey).filter(ProviderAPIKey.id == key_id).first()
    if not key:
        raise NotFoundException(f"Key {key_id} 不存在")

    if not key.oauth_invalid_at:
        return {"message": "该 Key 当前无失效标记，无需清除"}

    old_reason = key.oauth_invalid_reason
    key.oauth_invalid_at = None
    key.oauth_invalid_reason = None
    key.is_active = True
    db.commit()
    _run_async_with_fallback(_invalidate_cache_after_clear_oauth_invalid(key_id))

    logger.info(
        "[OK] 手动清除 Key {}... 的 OAuth 失效标记并自动启用 (原因: {})", key_id[:8], old_reason
    )
    return {"message": "已清除 OAuth 失效标记，Key 已自动启用"}


def clear_oauth_invalid_response(db: Session, key_id: str) -> dict[str, str]:
    """清除 OAuth 失效标记并返回统一响应。"""
    return _clear_oauth_invalid_marker(db=db, key_id=key_id)


def _prepare_update_key_payload(
    db: Session,
    key: ProviderAPIKey,
    key_id: str,
    key_data: EndpointAPIKeyUpdate,
) -> _UpdateKeyPreparation:
    """准备更新 Key 的数据，并执行规则校验。"""
    # 检查是否开启了 auto_fetch_models（用于后续立即获取模型）
    auto_fetch_enabled_before = key.auto_fetch_models
    auto_fetch_enabled_after = (
        key_data.auto_fetch_models
        if "auto_fetch_models" in key_data.model_fields_set
        else auto_fetch_enabled_before
    )

    # 记录 allowed_models 变化前的值
    allowed_models_before = set(key.allowed_models or [])

    # 记录过滤规则变化前的值（用于检测是否需要重新应用过滤）
    include_patterns_before = key.model_include_patterns
    exclude_patterns_before = key.model_exclude_patterns

    update_data = key_data.model_dump(exclude_unset=True)
    # 显式传 null 等价于“不更新 auth_type”，避免写入 NULL 触发数据库约束错误。
    if update_data.get("auth_type") is None:
        update_data.pop("auth_type", None)
    if "api_key" in update_data and isinstance(update_data["api_key"], str):
        update_data["api_key"] = update_data["api_key"].strip()

    # 验证 auth_type
    current_auth_type = normalize_auth_type(getattr(key, "auth_type", "api_key"))
    target_auth_type = normalize_auth_type(update_data.get("auth_type", current_auth_type))
    is_auth_type_switch = "auth_type" in update_data and target_auth_type != current_auth_type
    api_key_in_payload = "api_key" in update_data
    api_key_value = update_data.get("api_key")

    if api_key_in_payload and api_key_value == "":
        raise InvalidRequestException("api_key 不能为空")

    # auth_type 校验 + 字段归一化
    if target_auth_type == "api_key":
        if is_auth_type_switch and (not api_key_value or api_key_value == "__placeholder__"):
            raise InvalidRequestException("切换到 API Key 认证模式时，必须提供新的 API Key")
        if api_key_in_payload and (api_key_value is None or api_key_value == "__placeholder__"):
            raise InvalidRequestException("API Key 认证模式下 api_key 不能为空")
        # 切换回 API Key：清理非本模式配置
        update_data["auth_config"] = None
    elif target_auth_type == "service_account":
        if is_auth_type_switch and not update_data.get("auth_config"):
            raise InvalidRequestException(
                "切换到 Service Account 认证模式时，必须提供 Service Account JSON"
            )
        # Service Account 不允许手工写入 api_key，仅保留占位符
        if api_key_in_payload and api_key_value not in {None, "__placeholder__"}:
            raise InvalidRequestException("Service Account 认证模式下不允许直接填写 api_key")
        if is_auth_type_switch or api_key_in_payload:
            update_data["api_key"] = "__placeholder__"
    elif target_auth_type == "oauth":
        # OAuth 的 token 不允许在 key 更新接口里手工写入
        if api_key_in_payload and api_key_value not in {None, "__placeholder__"}:
            raise InvalidRequestException("OAuth 认证模式下不允许直接填写 api_key")
        if is_auth_type_switch:
            update_data["api_key"] = "__placeholder__"
            # 从非 OAuth 切换到 OAuth 时，清理旧认证配置（如 Vertex SA 凭证）。
            update_data["auth_config"] = None
        elif api_key_in_payload:
            # 避免把 null 写入 DB 或意外覆盖现有 OAuth token。
            update_data.pop("api_key", None)

    # 检查密钥是否与其他现有密钥重复（排除当前正在更新的密钥）
    check_duplicate_key(
        db=db,
        provider_id=key.provider_id,
        auth_type=target_auth_type,
        new_api_key=update_data.get("api_key"),
        new_auth_config=update_data.get("auth_config"),
        exclude_key_id=key_id,
    )

    # Vertex Provider: auth_type 与 api_formats 的组合必须合法
    provider = getattr(key, "provider", None)
    effective_api_formats = update_data.get("api_formats", key.api_formats)
    _validate_vertex_api_formats(
        getattr(provider, "provider_type", None),
        target_auth_type,
        effective_api_formats,
    )

    if "api_key" in update_data:
        api_key_raw = update_data["api_key"]
        if api_key_raw is None:
            # 防御式处理：避免将 NULL 写入 NOT NULL 字段导致 500。
            update_data.pop("api_key", None)
        else:
            update_data["api_key"] = crypto_service.encrypt(api_key_raw)

    # 加密 auth_config（包含敏感凭证）；即便是 {} 也必须加密存储。
    if "auth_config" in update_data:
        auth_config_raw = update_data["auth_config"]
        if auth_config_raw is None:
            pass
        elif isinstance(auth_config_raw, dict):
            update_data["auth_config"] = crypto_service.encrypt(json.dumps(auth_config_raw))
        else:
            raise InvalidRequestException("auth_config 必须是 JSON 对象")

    # 特殊处理 rpm_limit：需要区分"未提供"和"显式设置为 null"
    if "rpm_limit" in key_data.model_fields_set:
        update_data["rpm_limit"] = key_data.rpm_limit
        if key_data.rpm_limit is None:
            update_data["learned_rpm_limit"] = None
            logger.info("Key {} 切换为自适应 RPM 模式", key_id)

    # 统一处理 allowed_models：空列表 -> None（表示不限制）
    if "allowed_models" in update_data:
        am = update_data["allowed_models"]
        if isinstance(am, list) and len(am) == 0:
            update_data["allowed_models"] = None

    # 统一处理 locked_models：空列表 -> None
    if "locked_models" in update_data:
        lm = update_data["locked_models"]
        if isinstance(lm, list) and len(lm) == 0:
            update_data["locked_models"] = None

    # 处理模型过滤规则：空字符串 -> None
    if "model_include_patterns" in update_data:
        patterns = update_data["model_include_patterns"]
        if isinstance(patterns, list) and len(patterns) == 0:
            update_data["model_include_patterns"] = None

    if "model_exclude_patterns" in update_data:
        patterns = update_data["model_exclude_patterns"]
        if isinstance(patterns, list) and len(patterns) == 0:
            update_data["model_exclude_patterns"] = None

    # 处理 proxy：将 ProxyConfig 转换为 dict 存储，null 清除代理
    if "proxy" in key_data.model_fields_set:
        if key_data.proxy is None:
            update_data["proxy"] = None
        else:
            update_data["proxy"] = key_data.proxy.model_dump(exclude_none=True)

    if "fingerprint" in key_data.model_fields_set:
        if key_data.fingerprint is None:
            update_data["fingerprint"] = None
        else:
            update_data["fingerprint"] = normalize_fingerprint(key_data.fingerprint, key_id)

    return _UpdateKeyPreparation(
        update_data=update_data,
        auto_fetch_enabled_before=auto_fetch_enabled_before,
        auto_fetch_enabled_after=auto_fetch_enabled_after,
        allowed_models_before=allowed_models_before,
        include_patterns_before=include_patterns_before,
        exclude_patterns_before=exclude_patterns_before,
    )


def _prepare_create_key_payload(
    db: Session,
    provider_id: str,
    key_data: EndpointAPIKeyCreate,
) -> tuple[str, ProviderAPIKey]:
    """准备创建 Key 的认证类型校验、重复校验与实体构造。"""
    auth_type = key_data.auth_type or "api_key"
    if auth_type == "api_key":
        if not key_data.api_key:
            raise InvalidRequestException("API Key 认证模式下 api_key 为必填字段")
    elif auth_type == "service_account":
        if not key_data.auth_config:
            raise InvalidRequestException("Service Account 认证模式下 auth_config 为必填字段")
    elif auth_type == "oauth":
        # OAuth key 的 token 通过 provider-oauth 授权流程写入（此处不允许手填）
        if key_data.api_key:
            raise InvalidRequestException("OAuth 认证模式下不允许直接填写 api_key")

    # 检查密钥是否已存在（防止重复添加）
    check_duplicate_key(
        db=db,
        provider_id=provider_id,
        auth_type=auth_type,
        new_api_key=key_data.api_key,
        new_auth_config=key_data.auth_config,
    )

    # 加密 API Key（如果有）
    encrypted_key = (
        crypto_service.encrypt(key_data.api_key)
        if key_data.api_key
        else crypto_service.encrypt("__placeholder__")  # 占位符，保持 NOT NULL 约束
    )
    # OAuth 类型 key 初始写入占位符（token 由 provider-oauth 流程写入）
    if auth_type == "oauth":
        encrypted_key = crypto_service.encrypt("__placeholder__")
    now = datetime.now(timezone.utc)

    # 加密 auth_config（包含敏感的 Service Account 凭证）
    encrypted_auth_config = None
    if key_data.auth_config:
        encrypted_auth_config = crypto_service.encrypt(json.dumps(key_data.auth_config))

    new_key_id = str(uuid.uuid4())

    new_key = ProviderAPIKey(
        id=new_key_id,
        provider_id=provider_id,
        api_formats=key_data.api_formats,
        auth_type=auth_type,
        api_key=encrypted_key,
        auth_config=encrypted_auth_config,
        name=key_data.name,
        note=key_data.note,
        rate_multipliers=key_data.rate_multipliers,
        internal_priority=key_data.internal_priority,
        rpm_limit=key_data.rpm_limit,
        allowed_models=key_data.allowed_models if key_data.allowed_models else None,
        capabilities=key_data.capabilities if key_data.capabilities else None,
        cache_ttl_minutes=key_data.cache_ttl_minutes,
        max_probe_interval_minutes=key_data.max_probe_interval_minutes,
        auto_fetch_models=key_data.auto_fetch_models,
        locked_models=key_data.locked_models if key_data.locked_models else None,
        model_include_patterns=(
            key_data.model_include_patterns if key_data.model_include_patterns else None
        ),
        model_exclude_patterns=(
            key_data.model_exclude_patterns if key_data.model_exclude_patterns else None
        ),
        fingerprint=generate_fingerprint(seed=new_key_id),
        request_count=0,
        success_count=0,
        error_count=0,
        total_response_time_ms=0,
        health_by_format={},  # 按格式存储健康度
        circuit_breaker_by_format={},  # 按格式存储熔断器状态
        is_active=True,
        last_used_at=None,
        created_at=now,
        updated_at=now,
    )
    return auth_type, new_key


async def update_endpoint_key_response(
    db: Session,
    key_id: str,
    key_data: EndpointAPIKeyUpdate,
) -> EndpointAPIKeyResponse:
    """更新 Key 并返回响应对象。"""
    key = db.query(ProviderAPIKey).filter(ProviderAPIKey.id == key_id).first()
    if not key:
        raise NotFoundException(f"Key {key_id} 不存在")

    prepared = _prepare_update_key_payload(
        db=db,
        key=key,
        key_id=key_id,
        key_data=key_data,
    )

    for field, value in prepared.update_data.items():
        setattr(key, field, value)
    key.updated_at = datetime.now(timezone.utc)

    db.commit()
    db.refresh(key)

    await run_update_key_side_effects(
        db=db,
        key=key,
        key_id=key_id,
        auto_fetch_enabled_before=prepared.auto_fetch_enabled_before,
        auto_fetch_enabled_after=prepared.auto_fetch_enabled_after,
        include_patterns_before=prepared.include_patterns_before,
        exclude_patterns_before=prepared.exclude_patterns_before,
        allowed_models_before=prepared.allowed_models_before,
    )

    logger.info("[OK] 更新 Key: ID={}, Updates={}", key_id, list(prepared.update_data.keys()))
    return build_key_response(key)


async def create_provider_key_response(
    db: Session,
    provider_id: str,
    key_data: EndpointAPIKeyCreate,
) -> EndpointAPIKeyResponse:
    """创建 Provider Key 并返回响应对象。"""
    provider = db.query(Provider).filter(Provider.id == provider_id).first()
    if not provider:
        raise NotFoundException(f"Provider {provider_id} 不存在")

    if not key_data.api_formats:
        raise InvalidRequestException("api_formats 为必填字段")

    auth_type, new_key = _prepare_create_key_payload(
        db=db,
        provider_id=provider_id,
        key_data=key_data,
    )

    # Vertex Provider: auth_type 与 api_formats 的组合必须合法
    _validate_vertex_api_formats(
        getattr(provider, "provider_type", None),
        auth_type,
        key_data.api_formats,
    )

    db.add(new_key)
    db.commit()
    db.refresh(new_key)

    key_tail = (key_data.api_key or "")[-4:]
    logger.info(
        "[OK] 添加 Key: Provider={}, Formats={}, Key=***{}, ID={}",
        provider_id,
        key_data.api_formats,
        key_tail,
        new_key.id,
    )

    await run_create_key_side_effects(db=db, provider_id=provider_id, key=new_key)
    return build_key_response(new_key, api_key_plain=key_data.api_key)


def _delete_endpoint_key(db: Session, key_id: str) -> _DeleteKeyResult:
    """删除指定 Key 并返回删除上下文。"""
    key = db.query(ProviderAPIKey).filter(ProviderAPIKey.id == key_id).first()
    if not key:
        raise NotFoundException(f"Key {key_id} 不存在")

    provider_id = key.provider_id
    deleted_key_allowed_models = key.allowed_models  # 保存被删除 Key 的 allowed_models
    try:
        db.delete(key)
        db.commit()
    except Exception as exc:
        db.rollback()
        logger.error(f"删除 Key 失败: ID={key_id}, Error={exc}")
        raise

    return _DeleteKeyResult(
        provider_id=provider_id,
        deleted_key_allowed_models=deleted_key_allowed_models,
    )


async def delete_endpoint_key_response(db: Session, key_id: str) -> dict[str, str]:
    """删除 Key，执行副作用并返回统一响应。"""
    delete_result = _delete_endpoint_key(db, key_id)
    await run_delete_key_side_effects(
        db=db,
        provider_id=delete_result.provider_id,
        deleted_key_allowed_models=delete_result.deleted_key_allowed_models,
    )
    logger.warning("[DELETE] 删除 Key: ID={}, Provider={}", key_id, delete_result.provider_id)
    return {"message": f"Key {key_id} 已删除"}


async def batch_delete_endpoint_keys_response(db: Session, key_ids: list[str]) -> dict[str, Any]:
    """批量删除 Keys，按 provider_id 聚合后仅执行一次副作用。"""
    if not key_ids:
        return {"success_count": 0, "failed_count": 0, "failed": []}

    # 一次查询所有 Key
    keys = db.query(ProviderAPIKey).filter(ProviderAPIKey.id.in_(key_ids)).all()
    found_ids = {key.id for key in keys}
    not_found_ids = [kid for kid in key_ids if kid not in found_ids]

    failed: list[dict[str, str]] = [{"id": kid, "error": "not found"} for kid in not_found_ids]

    # 收集受影响的 provider_id
    affected_provider_ids = {key.provider_id for key in keys if key.provider_id}

    # 批量 SQL DELETE，依赖数据库 CASCADE/SET NULL 自动清理关联表
    success_count = 0
    try:
        found_id_list = list(found_ids)
        db.execute(sa_delete(ProviderAPIKey).where(ProviderAPIKey.id.in_(found_id_list)))
        db.commit()
        success_count = len(found_ids)
    except Exception as exc:
        db.rollback()
        logger.error("批量删除 Key 提交失败: {}", exc)
        failed.extend({"id": kid, "error": str(exc)} for kid in found_ids)
        return {"success_count": 0, "failed_count": len(failed), "failed": failed}

    # 按 provider_id 聚合，每个 provider 仅执行一次副作用
    for provider_id in affected_provider_ids:
        try:
            await run_delete_key_side_effects(
                db=db,
                provider_id=provider_id,
                deleted_key_allowed_models=None,
            )
        except Exception as exc:
            logger.error("批量删除副作用执行失败: provider_id={}, Error={}", provider_id, exc)

    logger.warning(
        "[BATCH_DELETE] 批量删除 Keys: success={}, failed={}, providers={}",
        success_count,
        len(failed),
        len(affected_provider_ids),
    )
    return {
        "success_count": success_count,
        "failed_count": len(failed),
        "failed": failed,
    }
