"""
Provider API Keys 管理
"""

from __future__ import annotations

import json
import time
import uuid
from dataclasses import dataclass
from datetime import datetime, timezone
from typing import Any

from fastapi import APIRouter, Depends, Query, Request
from sqlalchemy.orm import Session

from src.api.base.admin_adapter import AdminApiAdapter
from src.api.base.context import ApiRequestContext
from src.api.base.models_service import invalidate_models_list_cache
from src.api.base.pipeline import ApiRequestPipeline
from src.core.crypto import crypto_service
from src.core.exceptions import InvalidRequestException, NotFoundException
from src.core.key_capabilities import get_capability
from src.core.logger import logger
from src.core.provider_types import ProviderType
from src.database import get_db
from src.models.database import Provider, ProviderAPIKey, ProviderEndpoint, User
from src.models.endpoint_models import (
    EndpointAPIKeyCreate,
    EndpointAPIKeyResponse,
    EndpointAPIKeyUpdate,
)
from src.services.cache.provider_cache import ProviderCacheService
from src.services.model.upstream_fetcher import merge_upstream_metadata
from src.utils.auth_utils import require_admin

router = APIRouter(tags=["Provider Keys"])
pipeline = ApiRequestPipeline()


@router.put("/keys/{key_id}", response_model=EndpointAPIKeyResponse)
async def update_endpoint_key(
    key_id: str,
    key_data: EndpointAPIKeyUpdate,
    request: Request,
    db: Session = Depends(get_db),
) -> EndpointAPIKeyResponse:
    """
    更新 Provider Key

    更新指定 Key 的配置，支持修改并发限制、速率倍数、优先级、
    配额限制、能力限制等。支持部分更新。

    **路径参数**:
    - `key_id`: Key ID

    **请求体字段**（均为可选）:
    - `api_key`: 新的 API Key 原文
    - `name`: Key 名称
    - `note`: 备注
    - `rate_multipliers`: 按 API 格式的成本倍率
    - `internal_priority`: 内部优先级
    - `rpm_limit`: RPM 限制（设置为 null 可切换到自适应模式）
    - `allowed_models`: 允许的模型列表
    - `capabilities`: 能力配置
    - `is_active`: 是否活跃

    **返回字段**:
    - 包含更新后的完整 Key 信息
    """
    adapter = AdminUpdateEndpointKeyAdapter(key_id=key_id, key_data=key_data)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/keys/grouped-by-format")
async def get_keys_grouped_by_format(
    request: Request,
    db: Session = Depends(get_db),
) -> dict:
    """
    获取按 API 格式分组的所有 Keys

    获取所有活跃的 Key，按 API 格式分组返回，用于全局优先级管理。
    每个 Key 包含基本信息、健康度指标、能力标签等。

    **返回字段**:
    - 返回一个字典，键为 API 格式，值为该格式下的 Key 列表
    - 每个 Key 包含：
      - `id`: Key ID
      - `name`: Key 名称
      - `api_key_masked`: 脱敏后的 API Key
      - `internal_priority`: 内部优先级
      - `global_priority_by_format`: 按 API 格式的全局优先级
      - `format_priority`: 当前格式的优先级
      - `rate_multipliers`: 按 API 格式的成本倍率
      - `is_active`: 是否活跃
      - `circuit_breaker_open`: 熔断器状态
      - `provider_name`: Provider 名称
      - `endpoint_base_url`: Endpoint 基础 URL
      - `api_format`: API 格式
      - `capabilities`: 能力简称列表
      - `success_rate`: 成功率
      - `avg_response_time_ms`: 平均响应时间
      - `request_count`: 请求总数
    """
    adapter = AdminGetKeysGroupedByFormatAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/keys/{key_id}/reveal")
async def reveal_endpoint_key(
    key_id: str,
    request: Request,
    db: Session = Depends(get_db),
) -> dict:
    """
    获取完整的 API Key

    解密并返回指定 Key 的完整原文，用于查看和复制。
    此操作会被记录到审计日志。

    **路径参数**:
    - `key_id`: Key ID

    **返回字段**:
    - `api_key`: 完整的 API Key 原文
    """
    adapter = AdminRevealEndpointKeyAdapter(key_id=key_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/keys/{key_id}/export")
async def export_key(
    key_id: str,
    request: Request,
    db: Session = Depends(get_db),
    _: User = Depends(require_admin),
) -> dict:
    """
    导出 OAuth Key 凭据（用于跨实例迁移）

    解密 auth_config，返回精简的扁平 JSON，去掉 null 和临时字段。
    所有 OAuth Provider 格式统一。

    **路径参数**:
    - `key_id`: Key ID
    """
    adapter = AdminExportKeyAdapter(key_id=key_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.delete("/keys/{key_id}")
async def delete_endpoint_key(
    key_id: str,
    request: Request,
    db: Session = Depends(get_db),
) -> dict:
    """
    删除 Provider Key

    删除指定的 API Key。此操作不可逆，请谨慎使用。

    **路径参数**:
    - `key_id`: Key ID

    **返回字段**:
    - `message`: 操作结果消息
    """
    adapter = AdminDeleteEndpointKeyAdapter(key_id=key_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.post("/keys/{key_id}/clear-oauth-invalid")
async def clear_oauth_invalid(
    key_id: str,
    request: Request,
    db: Session = Depends(get_db),
    _: User = Depends(require_admin),
) -> dict:
    """
    清除 Key 的 OAuth 失效标记

    手动清除指定 Key 的 oauth_invalid_at / oauth_invalid_reason 状态，
    通常在管理员确认账号已完成验证后使用。

    **路径参数**:
    - `key_id`: Key ID

    **返回字段**:
    - `message`: 操作结果消息
    """
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

    logger.info("[OK] 手动清除 Key {}... 的 OAuth 失效标记并自动启用 (原因: {})", key_id[:8], old_reason)

    return {"message": "已清除 OAuth 失效标记，Key 已自动启用"}


# ========== Provider Keys API ==========


@router.get("/providers/{provider_id}/keys", response_model=list[EndpointAPIKeyResponse])
async def list_provider_keys(
    provider_id: str,
    request: Request,
    skip: int = Query(0, ge=0, description="跳过的记录数"),
    limit: int = Query(100, ge=1, le=1000, description="返回的最大记录数"),
    db: Session = Depends(get_db),
) -> list[EndpointAPIKeyResponse]:
    """
    获取 Provider 的所有 Keys

    获取指定 Provider 下的所有 API Key 列表，支持多 API 格式。
    结果按优先级和创建时间排序。

    **路径参数**:
    - `provider_id`: Provider ID

    **查询参数**:
    - `skip`: 跳过的记录数，用于分页（默认 0）
    - `limit`: 返回的最大记录数（1-1000，默认 100）
    """
    adapter = AdminListProviderKeysAdapter(
        provider_id=provider_id,
        skip=skip,
        limit=limit,
    )
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.post("/providers/{provider_id}/keys", response_model=EndpointAPIKeyResponse)
async def add_provider_key(
    provider_id: str,
    key_data: EndpointAPIKeyCreate,
    request: Request,
    db: Session = Depends(get_db),
) -> EndpointAPIKeyResponse:
    """
    为 Provider 添加 Key

    为指定 Provider 添加新的 API Key，支持配置多个 API 格式。

    **路径参数**:
    - `provider_id`: Provider ID

    **请求体字段**:
    - `api_formats`: 支持的 API 格式列表（必填）
    - `api_key`: API Key 原文（将被加密存储）
    - `name`: Key 名称
    - 其他配置字段同 Key
    """
    adapter = AdminCreateProviderKeyAdapter(provider_id=provider_id, key_data=key_data)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


# -------- Adapters --------


def _normalize_auth_type(raw: str) -> str:
    """将数据库中的 auth_type 归一化为逻辑类型。

    Kiro 在数据库中存储为 ``"kiro"`` 或 ``"oauth"``，统一映射为 ``"oauth"``。
    """
    t = str(raw or "api_key").strip() or "api_key"
    return "oauth" if t == "kiro" else t


def check_duplicate_key(
    db: Session,
    provider_id: str,
    auth_type: str,
    new_api_key: str | None = None,
    new_auth_config: dict | None = None,
    exclude_key_id: str | None = None,
) -> None:
    """
    检查密钥是否与其他现有密钥重复

    对于不同的认证类型，使用不同的比较方式：
    - api_key: 比较 API Key 的哈希值
    - vertex_ai: 比较 Service Account 的 client_email

    Args:
        db: 数据库会话
        provider_id: Provider ID
        auth_type: 认证类型 (api_key, vertex_ai, oauth)
        new_api_key: 新的 API Key（用于 api_key 类型）
        new_auth_config: 新的认证配置（用于 vertex_ai 类型）
        exclude_key_id: 要排除的 Key ID（用于更新场景）
    """
    if auth_type == "api_key" and new_api_key:
        # 跳过占位符
        if new_api_key == "__placeholder__":
            return

        # 仅查询同 auth_type 的 Keys，减少不必要的解密操作
        query = db.query(ProviderAPIKey).filter(
            ProviderAPIKey.provider_id == provider_id,
            ProviderAPIKey.auth_type == "api_key",
        )
        if exclude_key_id:
            query = query.filter(ProviderAPIKey.id != exclude_key_id)

        new_key_hash = crypto_service.hash_api_key(new_api_key)
        for existing_key in query:
            try:
                decrypted_key = crypto_service.decrypt(existing_key.api_key, silent=True)
                if decrypted_key == "__placeholder__":
                    continue
                existing_hash = crypto_service.hash_api_key(decrypted_key)
                if new_key_hash == existing_hash:
                    raise InvalidRequestException(
                        f"该 API Key 已存在于当前 Provider 中（名称: {existing_key.name}）"
                    )
            except InvalidRequestException:
                raise
            except Exception:
                # 解密失败时跳过该 Key
                continue

    elif auth_type == "vertex_ai" and new_auth_config:
        new_client_email = (
            new_auth_config.get("client_email") if isinstance(new_auth_config, dict) else None
        )
        if not new_client_email:
            return

        # 仅查询同 auth_type 且有 auth_config 的 Keys
        query = db.query(ProviderAPIKey).filter(
            ProviderAPIKey.provider_id == provider_id,
            ProviderAPIKey.auth_type == "vertex_ai",
            ProviderAPIKey.auth_config.isnot(None),
        )
        if exclude_key_id:
            query = query.filter(ProviderAPIKey.id != exclude_key_id)

        for existing_key in query:
            try:
                decrypted_config = json.loads(
                    crypto_service.decrypt(existing_key.auth_config, silent=True)
                )
                existing_email = decrypted_config.get("client_email")
                if existing_email and existing_email == new_client_email:
                    raise InvalidRequestException(
                        f"该 Service Account ({new_client_email}) 已存在于当前 Provider 中"
                        f"（名称: {existing_key.name}）"
                    )
            except InvalidRequestException:
                raise
            except Exception:
                # 解密失败时跳过该 Key
                continue


@dataclass
class AdminUpdateEndpointKeyAdapter(AdminApiAdapter):
    key_id: str
    key_data: EndpointAPIKeyUpdate

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        db = context.db
        key = db.query(ProviderAPIKey).filter(ProviderAPIKey.id == self.key_id).first()
        if not key:
            raise NotFoundException(f"Key {self.key_id} 不存在")

        # 检查是否开启了 auto_fetch_models（用于后续立即获取模型）
        auto_fetch_enabled_before = key.auto_fetch_models
        auto_fetch_enabled_after = (
            self.key_data.auto_fetch_models
            if "auto_fetch_models" in self.key_data.model_fields_set
            else auto_fetch_enabled_before
        )

        # 记录 allowed_models 变化前的值
        allowed_models_before = set(key.allowed_models or [])

        # 记录过滤规则变化前的值（用于检测是否需要重新应用过滤）
        include_patterns_before = key.model_include_patterns
        exclude_patterns_before = key.model_exclude_patterns

        update_data = self.key_data.model_dump(exclude_unset=True)

        # 验证 auth_type 切换
        current_auth_type = _normalize_auth_type(getattr(key, "auth_type", "api_key"))
        target_auth_type = update_data.get("auth_type", current_auth_type) or current_auth_type

        # auth_type 切换校验 + 字段归一化
        if "auth_type" in update_data:
            if target_auth_type == "api_key":
                if current_auth_type in {"vertex_ai", "oauth"} and not update_data.get("api_key"):
                    raise InvalidRequestException("切换到 API Key 认证模式时，必须提供新的 API Key")
                # 切换回 API Key：清理非本模式配置
                update_data["auth_config"] = None
            elif target_auth_type == "vertex_ai":
                if current_auth_type != "vertex_ai" and not update_data.get("auth_config"):
                    raise InvalidRequestException(
                        "从 API Key 切换到 Vertex AI 认证模式时，必须提供 Service Account JSON"
                    )
                # Vertex AI 不使用 api_key：写入占位符（若未提供 api_key）
                if "api_key" not in update_data:
                    update_data["api_key"] = "__placeholder__"
            elif target_auth_type == "oauth":
                # OAuth 的 token 不允许在 key 更新接口里手工写入
                if update_data.get("api_key"):
                    raise InvalidRequestException("OAuth 认证模式下不允许直接填写 api_key")
                if "api_key" not in update_data:
                    update_data["api_key"] = "__placeholder__"

        # 检查密钥是否与其他现有密钥重复（排除当前正在更新的密钥）
        check_duplicate_key(
            db=db,
            provider_id=key.provider_id,
            auth_type=target_auth_type,
            new_api_key=update_data.get("api_key"),
            new_auth_config=update_data.get("auth_config"),
            exclude_key_id=self.key_id,
        )

        if "api_key" in update_data and update_data["api_key"] is not None:
            update_data["api_key"] = crypto_service.encrypt(update_data["api_key"])
        # 加密 auth_config（包含敏感的 Service Account 凭证）
        if "auth_config" in update_data and update_data["auth_config"]:
            update_data["auth_config"] = crypto_service.encrypt(
                json.dumps(update_data["auth_config"])
            )

        # 特殊处理 rpm_limit：需要区分"未提供"和"显式设置为 null"
        if "rpm_limit" in self.key_data.model_fields_set:
            update_data["rpm_limit"] = self.key_data.rpm_limit
            if self.key_data.rpm_limit is None:
                update_data["learned_rpm_limit"] = None
                logger.info("Key {} 切换为自适应 RPM 模式", self.key_id)

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
        if "proxy" in self.key_data.model_fields_set:
            if self.key_data.proxy is None:
                update_data["proxy"] = None
            else:
                update_data["proxy"] = self.key_data.proxy.model_dump(exclude_none=True)

        for field, value in update_data.items():
            setattr(key, field, value)
        key.updated_at = datetime.now(timezone.utc)

        db.commit()
        db.refresh(key)

        # 处理 auto_fetch_models 的开启和关闭
        if not auto_fetch_enabled_before and auto_fetch_enabled_after:
            # 刚刚开启了 auto_fetch_models，同步执行模型获取
            logger.info("[AUTO_FETCH] Key {} 开启自动获取模型，同步执行模型获取", self.key_id)
            try:
                from src.services.model.fetch_scheduler import get_model_fetch_scheduler

                scheduler = get_model_fetch_scheduler()
                # 同步等待模型获取完成，确保前端刷新时能看到最新数据
                await scheduler._fetch_models_for_key_by_id(self.key_id)
            except Exception as e:
                logger.error(f"触发模型获取失败: {e}")
                # 不抛出异常，避免影响 Key 更新操作
        elif auto_fetch_enabled_before and not auto_fetch_enabled_after:
            # 关闭了 auto_fetch_models，只保留锁定的模型，清除自动获取的模型
            locked = key.locked_models or []
            if locked:
                key.allowed_models = locked
                logger.info(
                    "[AUTO_FETCH] Key {} 关闭自动获取模型，保留 {} 个锁定模型",
                    self.key_id,
                    len(locked),
                )
            else:
                key.allowed_models = None
                logger.info(
                    "[AUTO_FETCH] Key {} 关闭自动获取模型，无锁定模型，清空 allowed_models",
                    self.key_id,
                )
            db.commit()
            db.refresh(key)
        elif auto_fetch_enabled_after:
            # auto_fetch_models 保持开启状态，检查过滤规则是否变更
            include_patterns_after = key.model_include_patterns
            exclude_patterns_after = key.model_exclude_patterns
            patterns_changed = (
                include_patterns_before != include_patterns_after
                or exclude_patterns_before != exclude_patterns_after
            )
            if patterns_changed:
                # 过滤规则变更，重新应用过滤（使用缓存的上游模型数据）
                logger.info(
                    "[AUTO_FETCH] Key {} 过滤规则变更，重新应用过滤",
                    self.key_id,
                )
                try:
                    from src.services.model.fetch_scheduler import get_model_fetch_scheduler

                    scheduler = get_model_fetch_scheduler()
                    await scheduler._fetch_models_for_key_by_id(self.key_id)
                except Exception as e:
                    logger.error(f"重新应用过滤规则失败: {e}")

        # 任何字段更新都清除缓存，确保缓存一致性
        # 包括 is_active、allowed_models、capabilities 等影响权限和行为的字段
        await ProviderCacheService.invalidate_provider_api_key_cache(self.key_id)

        # 检查 allowed_models 是否有变化，触发缓存失效和自动关联
        allowed_models_after = set(key.allowed_models or [])
        if allowed_models_before != allowed_models_after and key.provider_id:
            from src.services.model.global_model import on_key_allowed_models_changed

            await on_key_allowed_models_changed(
                db=db,
                provider_id=key.provider_id,
                allowed_models=list(key.allowed_models or []),
            )
        else:
            # allowed_models 未变化时，仍需清除 /v1/models 缓存（is_active、api_formats 变更会影响模型可用性）
            await invalidate_models_list_cache()

        logger.info("[OK] 更新 Key: ID={}, Updates={}", self.key_id, list(update_data.keys()))

        return _build_key_response(key)


@dataclass
class AdminRevealEndpointKeyAdapter(AdminApiAdapter):
    """获取完整的 API Key 或 Auth Config（用于查看和复制）"""

    key_id: str

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        db = context.db
        key = db.query(ProviderAPIKey).filter(ProviderAPIKey.id == self.key_id).first()
        if not key:
            raise NotFoundException(f"Key {self.key_id} 不存在")

        auth_type = _normalize_auth_type(getattr(key, "auth_type", "api_key"))

        # Vertex AI 类型返回 auth_config（需要解密）
        if auth_type == "vertex_ai":
            encrypted_auth_config = getattr(key, "auth_config", None)
            if encrypted_auth_config:
                try:
                    decrypted_config = crypto_service.decrypt(encrypted_auth_config)
                    auth_config = json.loads(decrypted_config)
                    logger.info(f"[REVEAL] 查看 Auth Config: ID={self.key_id}, Name={key.name}")
                    return {"auth_type": "vertex_ai", "auth_config": auth_config}
                except Exception as e:
                    logger.error(f"解密 Auth Config 失败: ID={self.key_id}, Error={e}")
                    raise InvalidRequestException(
                        "无法解密认证配置，可能是加密密钥已更改。请重新添加该密钥。"
                    )
            # 兼容：auth_config 为空时尝试从 api_key 解密（仅对迁移前的旧数据有效）
            try:
                decrypted_key = crypto_service.decrypt(key.api_key)
                # 检查是否是新格式的占位符（表示 auth_config 丢失）
                if decrypted_key == "__placeholder__":
                    logger.error(f"Vertex AI Key 缺少 auth_config: ID={self.key_id}")
                    raise InvalidRequestException("认证配置丢失，请重新添加该密钥。")
                logger.info(
                    f"[REVEAL] 查看完整 Key (legacy vertex_ai): ID={self.key_id}, Name={key.name}"
                )
                return {"auth_type": "vertex_ai", "auth_config": decrypted_key}
            except InvalidRequestException:
                raise
            except Exception as e:
                logger.error(f"解密 Key 失败: ID={self.key_id}, Error={e}")
                raise InvalidRequestException(
                    "无法解密认证配置，可能是加密密钥已更改。请重新添加该密钥。"
                )

        # OAuth 类型：返回 access_token（导出走 /export 端点）
        if auth_type == "oauth":
            try:
                decrypted_key = crypto_service.decrypt(key.api_key)
            except Exception as e:
                logger.error(f"解密 Key 失败: ID={self.key_id}, Error={e}")
                raise InvalidRequestException(
                    "无法解密 API Key，可能是加密密钥已更改。请重新添加该密钥。"
                )
            logger.info(f"[REVEAL] 查看 OAuth Key: ID={self.key_id}, Name={key.name}")
            return {"auth_type": "oauth", "api_key": decrypted_key}

        # API Key 类型返回 api_key
        try:
            decrypted_key = crypto_service.decrypt(key.api_key)
        except Exception as e:
            logger.error(f"解密 Key 失败: ID={self.key_id}, Error={e}")
            raise InvalidRequestException(
                "无法解密 API Key，可能是加密密钥已更改。请重新添加该密钥。"
            )

        logger.info(f"[REVEAL] 查看完整 Key: ID={self.key_id}, Name={key.name}")
        return {"auth_type": "api_key", "api_key": decrypted_key}


@dataclass
class AdminExportKeyAdapter(AdminApiAdapter):
    """导出 OAuth Key 凭据：解密 auth_config，委托 provider-specific builder 构建导出数据。"""

    key_id: str

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        from src.services.provider.export import build_export_data

        db = context.db
        key = db.query(ProviderAPIKey).filter(ProviderAPIKey.id == self.key_id).first()
        if not key:
            raise NotFoundException(f"Key {self.key_id} 不存在")

        auth_type = _normalize_auth_type(getattr(key, "auth_type", "api_key"))
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

        logger.info("[EXPORT] Key {}... 导出成功", self.key_id[:8])
        return export_data


@dataclass
class AdminDeleteEndpointKeyAdapter(AdminApiAdapter):
    key_id: str

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        db = context.db
        key = db.query(ProviderAPIKey).filter(ProviderAPIKey.id == self.key_id).first()
        if not key:
            raise NotFoundException(f"Key {self.key_id} 不存在")

        provider_id = key.provider_id
        deleted_key_allowed_models = key.allowed_models  # 保存被删除 Key 的 allowed_models
        try:
            db.delete(key)
            db.commit()
        except Exception as exc:
            db.rollback()
            logger.error(f"删除 Key 失败: ID={self.key_id}, Error={exc}")
            raise

        # 触发缓存失效和自动解除关联检查
        # 注意：只有当被删除的 Key 有具体的 allowed_models 时才触发 disassociate
        # 如果 allowed_models 为 null（允许所有模型），则不需要检查解除关联
        if provider_id:
            from src.services.model.global_model import on_key_allowed_models_changed

            await on_key_allowed_models_changed(
                db=db,
                provider_id=provider_id,
                skip_disassociate=deleted_key_allowed_models is None,
            )
        else:
            # 无 provider_id 时仅清除缓存
            await invalidate_models_list_cache()

        logger.warning(f"[DELETE] 删除 Key: ID={self.key_id}, Provider={provider_id}")
        return {"message": f"Key {self.key_id} 已删除"}


class AdminGetKeysGroupedByFormatAdapter(AdminApiAdapter):
    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        db = context.db

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
            endpoint_base_url_map[(str(provider_id), fmt)] = base_url

        grouped: dict[str, list[dict]] = {}
        for key, provider in keys:
            api_formats = key.api_formats or []

            if not api_formats:
                continue  # 跳过没有 API 格式的 Key

            auth_type = _normalize_auth_type(getattr(key, "auth_type", "api_key"))
            if auth_type == "vertex_ai":
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
            key_info = {
                "id": key.id,
                "name": key.name,
                "auth_type": auth_type,
                "api_key_masked": masked_key,
                "internal_priority": key.internal_priority,
                "global_priority_by_format": key.global_priority_by_format,
                "rate_multipliers": key.rate_multipliers,
                "is_active": key.is_active,
                "provider_active": provider.is_active,
                "provider_name": provider.name,
                "api_formats": api_formats,
                "capabilities": caps_list,
                "success_rate": success_rate,
                "avg_response_time_ms": avg_response_time_ms,
                "request_count": key.request_count,
            }

            # 将 Key 添加到每个支持的格式分组中，并附加格式特定的数据
            health_by_format = key.health_by_format or {}
            circuit_by_format = key.circuit_breaker_by_format or {}
            priority_by_format = key.global_priority_by_format or {}
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


# ========== Adapters ==========


def _build_key_response(
    key: ProviderAPIKey, api_key_plain: str | None = None
) -> EndpointAPIKeyResponse:
    """构建 Key 响应对象的辅助函数"""
    auth_type = _normalize_auth_type(getattr(key, "auth_type", "api_key"))

    if auth_type == "vertex_ai":
        # Vertex AI 使用 Service Account，不显示占位符
        masked_key = "[Service Account]"
    elif auth_type == "oauth":
        masked_key = "[OAuth Token]"
    else:
        try:
            decrypted_key = crypto_service.decrypt(key.api_key)
            masked_key = f"{decrypted_key[:8]}***{decrypted_key[-4:]}"
        except Exception:
            masked_key = "***ERROR***"

    success_rate = key.success_count / key.request_count if key.request_count > 0 else 0.0
    avg_response_time_ms = (
        key.total_response_time_ms / key.success_count if key.success_count > 0 else 0.0
    )

    is_adaptive = key.rpm_limit is None
    key_dict = key.__dict__.copy()
    key_dict.pop("_sa_instance_state", None)
    key_dict.pop("api_key", None)  # 移除敏感字段，避免泄露
    key_dict["auth_type"] = auth_type

    # 提取 OAuth 元数据（如果是 OAuth 类型）
    oauth_expires_at = None
    oauth_email = None
    oauth_plan_type = None
    oauth_account_id = None
    encrypted_auth_config = key_dict.pop("auth_config", None)  # 移除敏感字段，避免泄露
    if auth_type == "oauth" and encrypted_auth_config:
        try:
            decrypted_config = crypto_service.decrypt(encrypted_auth_config)
            auth_config = json.loads(decrypted_config)
            oauth_expires_at = auth_config.get("expires_at")
            oauth_email = auth_config.get("email")
            oauth_plan_type = auth_config.get("plan_type")  # Codex: plus/free/team/enterprise
            # Antigravity 使用 "tier" 字段（如 "PAID"/"FREE"），做小写化 fallback
            if not oauth_plan_type:
                ag_tier = auth_config.get("tier")
                if ag_tier and isinstance(ag_tier, str):
                    oauth_plan_type = ag_tier.lower()
            oauth_account_id = auth_config.get("account_id")  # Codex: chatgpt_account_id
        except Exception as e:
            logger.error("Failed to decrypt auth_config for key {}: {}", key.id, e)

    # 从 health_by_format 计算汇总字段（便于列表展示）
    health_by_format = key.health_by_format or {}
    circuit_by_format = key.circuit_breaker_by_format or {}

    # 计算整体健康度（取所有格式中的最低值）
    if health_by_format:
        health_scores = [float(h.get("health_score") or 1.0) for h in health_by_format.values()]
        min_health_score = min(health_scores) if health_scores else 1.0
        # 取最大的连续失败次数
        max_consecutive = max(
            (int(h.get("consecutive_failures") or 0) for h in health_by_format.values()),
            default=0,
        )
        # 取最近的失败时间
        failure_times = [
            h.get("last_failure_at") for h in health_by_format.values() if h.get("last_failure_at")
        ]
        last_failure = max(failure_times) if failure_times else None
    else:
        min_health_score = 1.0
        max_consecutive = 0
        last_failure = None

    # 检查是否有任何格式的熔断器打开
    any_circuit_open = any(c.get("open", False) for c in circuit_by_format.values())

    key_dict.update(
        {
            "api_key_masked": masked_key,
            "api_key_plain": api_key_plain,
            "success_rate": success_rate,
            "avg_response_time_ms": round(avg_response_time_ms, 2),
            "is_adaptive": is_adaptive,
            "effective_limit": (
                key.learned_rpm_limit  # 自适应模式：使用学习值，未学习时为 None（不限制）
                if is_adaptive
                else key.rpm_limit
            ),
            # 汇总字段
            "health_score": min_health_score,
            "consecutive_failures": max_consecutive,
            "last_failure_at": last_failure,
            "circuit_breaker_open": any_circuit_open,
            # OAuth 相关
            "oauth_expires_at": oauth_expires_at,
            "oauth_email": oauth_email,
            "oauth_plan_type": oauth_plan_type,
            "oauth_account_id": oauth_account_id,
            "oauth_invalid_at": (
                int(key.oauth_invalid_at.timestamp()) if key.oauth_invalid_at else None
            ),
            "oauth_invalid_reason": key.oauth_invalid_reason,
        }
    )

    # 防御性：确保 api_formats 存在（历史数据可能为空/缺失）
    if "api_formats" not in key_dict or key_dict["api_formats"] is None:
        key_dict["api_formats"] = []

    return EndpointAPIKeyResponse(**key_dict)


@dataclass
class AdminListProviderKeysAdapter(AdminApiAdapter):
    """获取 Provider 的所有 Keys"""

    provider_id: str
    skip: int
    limit: int

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        db = context.db
        provider = db.query(Provider).filter(Provider.id == self.provider_id).first()
        if not provider:
            raise NotFoundException(f"Provider {self.provider_id} 不存在")

        keys = (
            db.query(ProviderAPIKey)
            .filter(ProviderAPIKey.provider_id == self.provider_id)
            .order_by(ProviderAPIKey.internal_priority.asc(), ProviderAPIKey.created_at.asc())
            .offset(self.skip)
            .limit(self.limit)
            .all()
        )

        return [_build_key_response(key) for key in keys]


@dataclass
class AdminCreateProviderKeyAdapter(AdminApiAdapter):
    """为 Provider 添加 Key"""

    provider_id: str
    key_data: EndpointAPIKeyCreate

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        db = context.db
        provider = db.query(Provider).filter(Provider.id == self.provider_id).first()
        if not provider:
            raise NotFoundException(f"Provider {self.provider_id} 不存在")

        # 验证 api_formats 必填
        if not self.key_data.api_formats:
            raise InvalidRequestException("api_formats 为必填字段")

        # 验证认证配置
        auth_type = self.key_data.auth_type or "api_key"
        if auth_type == "api_key":
            if not self.key_data.api_key:
                raise InvalidRequestException("API Key 认证模式下 api_key 为必填字段")
        elif auth_type == "vertex_ai":
            if not self.key_data.auth_config:
                raise InvalidRequestException("Service Account 认证模式下 auth_config 为必填字段")
        elif auth_type == "oauth":
            # OAuth key 的 token 通过 provider-oauth 授权流程写入（此处不允许手填）
            if self.key_data.api_key:
                raise InvalidRequestException("OAuth 认证模式下不允许直接填写 api_key")

        # 检查密钥是否已存在（防止重复添加）
        check_duplicate_key(
            db=db,
            provider_id=self.provider_id,
            auth_type=auth_type,
            new_api_key=self.key_data.api_key,
            new_auth_config=self.key_data.auth_config,
        )

        # 加密 API Key（如果有）
        encrypted_key = (
            crypto_service.encrypt(self.key_data.api_key)
            if self.key_data.api_key
            else crypto_service.encrypt("__placeholder__")  # 占位符，保持 NOT NULL 约束
        )
        # OAuth 类型 key 初始写入占位符（token 由 provider-oauth 流程写入）
        if auth_type == "oauth":
            encrypted_key = crypto_service.encrypt("__placeholder__")
        now = datetime.now(timezone.utc)

        # 加密 auth_config（包含敏感的 Service Account 凭证）
        encrypted_auth_config = None
        if self.key_data.auth_config:
            encrypted_auth_config = crypto_service.encrypt(json.dumps(self.key_data.auth_config))

        new_key = ProviderAPIKey(
            id=str(uuid.uuid4()),
            provider_id=self.provider_id,
            api_formats=self.key_data.api_formats,
            auth_type=auth_type,
            api_key=encrypted_key,
            auth_config=encrypted_auth_config,
            name=self.key_data.name,
            note=self.key_data.note,
            rate_multipliers=self.key_data.rate_multipliers,
            internal_priority=self.key_data.internal_priority,
            rpm_limit=self.key_data.rpm_limit,
            allowed_models=self.key_data.allowed_models if self.key_data.allowed_models else None,
            capabilities=self.key_data.capabilities if self.key_data.capabilities else None,
            cache_ttl_minutes=self.key_data.cache_ttl_minutes,
            max_probe_interval_minutes=self.key_data.max_probe_interval_minutes,
            auto_fetch_models=self.key_data.auto_fetch_models,
            locked_models=self.key_data.locked_models if self.key_data.locked_models else None,
            model_include_patterns=(
                self.key_data.model_include_patterns
                if self.key_data.model_include_patterns
                else None
            ),
            model_exclude_patterns=(
                self.key_data.model_exclude_patterns
                if self.key_data.model_exclude_patterns
                else None
            ),
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

        db.add(new_key)
        db.commit()
        db.refresh(new_key)

        key_tail = (self.key_data.api_key or "")[-4:]
        logger.info(
            f"[OK] 添加 Key: Provider={self.provider_id}, "
            f"Formats={self.key_data.api_formats}, Key=***{key_tail}, ID={new_key.id}"
        )

        # 如果开启了 auto_fetch_models，同步执行模型获取
        if self.key_data.auto_fetch_models:
            logger.info("[AUTO_FETCH] 新 Key {} 开启自动获取模型，同步执行模型获取", new_key.id)
            try:
                from src.services.model.fetch_scheduler import get_model_fetch_scheduler

                scheduler = get_model_fetch_scheduler()
                # 同步等待模型获取完成，确保前端刷新时能看到最新数据
                await scheduler._fetch_models_for_key_by_id(new_key.id)
            except Exception as e:
                logger.error(f"触发模型获取失败: {e}")
                # 不抛出异常，避免影响 Key 创建操作

        # 如果创建时指定了 allowed_models，触发自动关联检查（内部会清除 /v1/models 缓存）
        if new_key.allowed_models:
            from src.services.model.global_model import on_key_allowed_models_changed

            await on_key_allowed_models_changed(
                db=db,
                provider_id=self.provider_id,
                allowed_models=list(new_key.allowed_models),
            )
        else:
            # 没有 allowed_models 时，仍需清除 /v1/models 缓存
            await invalidate_models_list_cache()

        return _build_key_response(new_key, api_key_plain=self.key_data.api_key)


# ========== Codex Quota Refresh API ==========

# Codex wham/usage API 地址（用于查询限额信息）
CODEX_WHAM_USAGE_URL = "https://chatgpt.com/backend-api/wham/usage"


def _parse_codex_wham_usage_response(data: dict) -> dict | None:
    """
    解析 Codex wham/usage API 响应，提取限额信息

    Free 账号:
    - rate_limit.primary_window: 周限额
    - code_review_rate_limit.primary_window: 代码审查周限额

    Team/Plus/Enterprise 账号:
    - rate_limit.primary_window: 5H 限额
    - rate_limit.secondary_window: 周限额
    - code_review_rate_limit.primary_window: 代码审查周限额
    """
    if not data:
        return None

    result: dict = {}

    plan_type = data.get("plan_type")
    if plan_type:
        result["plan_type"] = plan_type

    # 解析 rate_limit
    rate_limit = data.get("rate_limit") or {}
    primary_window = rate_limit.get("primary_window") or {}
    secondary_window = rate_limit.get("secondary_window")

    # 根据账号类型解析限额
    # Free 账号: primary_window 是周限额，无 secondary_window
    # Team/Plus/Enterprise: primary_window 是 5H 限额，secondary_window 是周限额
    if plan_type == "free":
        # Free 账号: primary_window 是周限额
        if primary_window:
            used_percent = primary_window.get("used_percent")
            if used_percent is not None:
                result["primary_used_percent"] = float(used_percent)
            reset_seconds = primary_window.get("reset_after_seconds")
            if reset_seconds is not None:
                result["primary_reset_seconds"] = int(reset_seconds)
            reset_at = primary_window.get("reset_at")
            if reset_at is not None:
                result["primary_reset_at"] = int(reset_at)
            limit_window_seconds = primary_window.get("limit_window_seconds")
            if limit_window_seconds is not None:
                result["primary_window_minutes"] = int(limit_window_seconds) // 60
    else:
        # Team/Plus/Enterprise: primary_window 是 5H 限额, secondary_window 是周限额
        if secondary_window:
            # 周限额 (secondary_window)
            used_percent = secondary_window.get("used_percent")
            if used_percent is not None:
                result["primary_used_percent"] = float(used_percent)
            reset_seconds = secondary_window.get("reset_after_seconds")
            if reset_seconds is not None:
                result["primary_reset_seconds"] = int(reset_seconds)
            reset_at = secondary_window.get("reset_at")
            if reset_at is not None:
                result["primary_reset_at"] = int(reset_at)
            limit_window_seconds = secondary_window.get("limit_window_seconds")
            if limit_window_seconds is not None:
                result["primary_window_minutes"] = int(limit_window_seconds) // 60

        if primary_window:
            # 5H 限额 (primary_window)
            used_percent = primary_window.get("used_percent")
            if used_percent is not None:
                result["secondary_used_percent"] = float(used_percent)
            reset_seconds = primary_window.get("reset_after_seconds")
            if reset_seconds is not None:
                result["secondary_reset_seconds"] = int(reset_seconds)
            reset_at = primary_window.get("reset_at")
            if reset_at is not None:
                result["secondary_reset_at"] = int(reset_at)
            limit_window_seconds = primary_window.get("limit_window_seconds")
            if limit_window_seconds is not None:
                result["secondary_window_minutes"] = int(limit_window_seconds) // 60

    # 解析 code_review_rate_limit (代码审查限额)
    code_review_limit = data.get("code_review_rate_limit") or {}
    code_review_primary = code_review_limit.get("primary_window") or {}
    if code_review_primary:
        used_percent = code_review_primary.get("used_percent")
        if used_percent is not None:
            result["code_review_used_percent"] = float(used_percent)
        reset_seconds = code_review_primary.get("reset_after_seconds")
        if reset_seconds is not None:
            result["code_review_reset_seconds"] = int(reset_seconds)
        reset_at = code_review_primary.get("reset_at")
        if reset_at is not None:
            result["code_review_reset_at"] = int(reset_at)
        limit_window_seconds = code_review_primary.get("limit_window_seconds")
        if limit_window_seconds is not None:
            result["code_review_window_minutes"] = int(limit_window_seconds) // 60

    # 解析 credits
    credits = data.get("credits") or {}
    has_credits = credits.get("has_credits")
    if has_credits is not None:
        result["has_credits"] = bool(has_credits)
    balance = credits.get("balance")
    if balance is not None:
        result["credits_balance"] = float(balance)

    # 添加更新时间戳
    if result:
        result["updated_at"] = int(time.time())

    return result if result else None


# ========== Kiro Quota Refresh API ==========


@router.post("/providers/{provider_id}/refresh-quota")
async def refresh_provider_quota(
    provider_id: str,
    request: Request,
    db: Session = Depends(get_db),
) -> dict:
    """
    刷新 Provider 所有 Keys 的限额信息

    支持的 Provider 类型：
    - Codex: 调用 wham/usage API 获取限额
    - Antigravity: 调用 fetchAvailableModels 获取配额
    - Kiro: 调用 getUsageLimits API 获取使用额度

    **路径参数**:
    - `provider_id`: Provider ID

    **返回字段**:
    - `success`: 成功刷新的 Key 数量
    - `failed`: 失败的 Key 数量
    - `results`: 每个 Key 的刷新结果
    """
    adapter = AdminRefreshProviderQuotaAdapter(provider_id=provider_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@dataclass
class AdminRefreshProviderQuotaAdapter(AdminApiAdapter):
    """刷新 Provider 所有 Keys 的限额信息"""

    provider_id: str

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        import asyncio

        import httpx

        from src.api.handlers.base.request_builder import get_provider_auth
        from src.utils.ssl_utils import get_ssl_context

        db = context.db
        provider = db.query(Provider).filter(Provider.id == self.provider_id).first()
        if not provider:
            raise NotFoundException(f"Provider {self.provider_id} 不存在")

        provider_type = str(getattr(provider, "provider_type", "") or "").strip().lower()
        if provider_type not in {ProviderType.CODEX, ProviderType.ANTIGRAVITY, ProviderType.KIRO}:
            raise InvalidRequestException(
                "仅支持 Codex / Antigravity / Kiro 类型的 Provider 刷新限额"
            )

        # 获取所有活跃的 Keys
        keys = (
            db.query(ProviderAPIKey)
            .filter(
                ProviderAPIKey.provider_id == self.provider_id,
                ProviderAPIKey.is_active.is_(True),
            )
            .all()
        )

        if not keys:
            return {
                "success": 0,
                "failed": 0,
                "total": 0,
                "results": [],
                "message": "没有活跃的 Key",
            }

        # 获取端点：
        # - Codex: openai:cli
        # - Antigravity: gemini:chat（用于触发 oauth 刷新 + 提供 auth_config.project_id）
        # - Kiro: 不需要特定端点，直接使用 auth_config 中的凭据
        endpoint = None
        if provider_type == ProviderType.CODEX:
            for ep in provider.endpoints:
                if ep.api_format == "openai:cli" and ep.is_active:
                    endpoint = ep
                    break
            if not endpoint:
                raise InvalidRequestException("找不到有效的 openai:cli 端点")
        elif provider_type == ProviderType.ANTIGRAVITY:
            # Prefer the new signature, but keep backward-compat with existing DB rows.
            for sig in ("gemini:chat", "gemini:cli"):
                for ep in provider.endpoints:
                    if ep.api_format == sig and ep.is_active:
                        endpoint = ep
                        break
                if endpoint is not None:
                    break
            if not endpoint:
                raise InvalidRequestException("找不到有效的 gemini:chat/gemini:cli 端点")
        # Kiro 不需要端点检查，直接使用 auth_config

        results: list[dict] = []
        success_count = 0
        failed_count = 0

        # 用于收集需要更新的 key 元数据（避免在并发任务中直接操作 db session）
        metadata_updates: dict[str, dict] = {}  # key_id -> metadata

        # 单个 Key 刷新函数
        async def refresh_single_key(key: ProviderAPIKey) -> dict:
            try:
                if provider_type == ProviderType.CODEX:
                    # 获取认证信息（用于刷新 OAuth token）
                    auth_info = await get_provider_auth(endpoint, key)

                    # 构建请求头
                    headers = {
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
                    auth_type = _normalize_auth_type(getattr(key, "auth_type", "api_key"))
                    if auth_type == "oauth" and key.auth_config:
                        try:
                            decrypted_config = crypto_service.decrypt(key.auth_config)
                            auth_config_data = json.loads(decrypted_config)
                            oauth_plan_type = auth_config_data.get("plan_type")
                            oauth_account_id = auth_config_data.get("account_id")
                        except Exception:
                            pass

                    # 如果有 account_id 且不是 free 账号，添加 chatgpt-account-id 头
                    if oauth_account_id and oauth_plan_type and oauth_plan_type.lower() != "free":
                        headers["chatgpt-account-id"] = oauth_account_id

                    # 使用 wham/usage API 获取限额信息
                    async with httpx.AsyncClient(timeout=30.0, verify=get_ssl_context()) as client:
                        response = await client.get(CODEX_WHAM_USAGE_URL, headers=headers)

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
                    metadata = _parse_codex_wham_usage_response(data)

                    if metadata:
                        # 收集元数据，稍后统一更新数据库（存储到 codex 子对象）
                        metadata_updates[key.id] = {"codex": metadata}
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

                elif provider_type == ProviderType.ANTIGRAVITY:
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

                    fetch_ctx = UpstreamModelsFetchContext(
                        provider_type="antigravity",
                        api_key_value=access_token,
                        # antigravity fetcher 不依赖 endpoint mapping
                        format_to_endpoint={},
                        proxy_config=getattr(provider, "proxy", None),
                        auth_config=auth_info.decrypted_auth_config,
                    )

                    try:
                        _models, errors, ok, upstream_meta = await fetch_models_for_key(
                            fetch_ctx, timeout_seconds=10.0
                        )
                    except AntigravityAccountForbiddenException as e:
                        # 对齐 AM：所有 403 一律标记 is_forbidden 并停用
                        key.is_active = False
                        key.oauth_invalid_at = datetime.now(timezone.utc)
                        key.oauth_invalid_reason = f"账户访问被禁止: {e.reason or e.message}"
                        # 更新 upstream_metadata 标记封禁状态
                        forbidden_metadata = {
                            "antigravity": {
                                "is_forbidden": True,
                                "forbidden_reason": e.reason or e.message,
                                "forbidden_at": int(time.time()),
                                "updated_at": int(time.time()),
                            }
                        }
                        key.upstream_metadata = merge_upstream_metadata(
                            key.upstream_metadata, forbidden_metadata
                        )
                        db.commit()
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

                elif provider_type == ProviderType.KIRO:
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

                    # 获取代理配置
                    proxy_config = getattr(provider, "proxy", None)

                    # 调用 Kiro getUsageLimits API
                    try:
                        result = await _fetch_kiro_usage_limits(
                            auth_config=auth_config_data,
                            proxy_config=proxy_config,
                        )
                    except KiroAccountBannedException as e:
                        # 账户被封禁，自动停用并标记
                        key.is_active = False
                        key.oauth_invalid_at = datetime.now(timezone.utc)
                        key.oauth_invalid_reason = f"账户已封禁: {e.reason or e.message}"
                        # 更新 upstream_metadata 标记封禁状态
                        ban_metadata = {
                            "kiro": {
                                "is_banned": True,
                                "ban_reason": e.reason or e.message,
                                "banned_at": int(time.time()),
                                "updated_at": int(time.time()),
                            }
                        }
                        key.upstream_metadata = merge_upstream_metadata(
                            key.upstream_metadata, ban_metadata
                        )
                        db.commit()
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
                            key.oauth_invalid_at = datetime.now(timezone.utc)
                            key.oauth_invalid_reason = "Kiro Token 无效或已过期"
                            key.is_active = False
                            db.commit()
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

                        # 如果 auth_config 有更新（例如 token 刷新），也需要更新
                        if updated_auth_config:
                            try:
                                new_auth_config_json = json.dumps(updated_auth_config)
                                key.auth_config = crypto_service.encrypt(new_auth_config_json)
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

            except Exception as e:
                logger.error("刷新 Key {} 限额失败: {}", key.id, e)
                return {
                    "key_id": key.id,
                    "key_name": key.name,
                    "status": "error",
                    "message": str(e),
                }

        # 分批执行，每批最多 5 个并发
        BATCH_SIZE = 5
        for i in range(0, len(keys), BATCH_SIZE):
            batch = keys[i : i + BATCH_SIZE]
            batch_tasks = [refresh_single_key(key) for key in batch]
            batch_results = await asyncio.gather(*batch_tasks)
            results.extend(batch_results)

            # 统计本批次结果
            for r in batch_results:
                if r["status"] == "success":
                    success_count += 1
                else:
                    failed_count += 1

        # 统一更新数据库（避免在并发任务中操作 session）
        if metadata_updates:
            for key in keys:
                if key.id in metadata_updates:
                    updates = metadata_updates[key.id]
                    if isinstance(updates, dict):
                        key.upstream_metadata = merge_upstream_metadata(
                            key.upstream_metadata, updates
                        )
                    db.add(key)

        # 提交数据库更改
        db.commit()

        failed_details = [
            f"{r.get('key_name', r.get('key_id', '?'))}: {r.get('message', 'unknown')}"
            for r in results
            if r["status"] != "success"
        ]
        if failed_details:
            logger.info(
                "[QUOTA_REFRESH] Provider {}: 成功 {}/{}, 失败 {} [{}]",
                self.provider_id,
                success_count,
                len(keys),
                failed_count,
                "; ".join(failed_details),
            )
        else:
            logger.info(
                "[QUOTA_REFRESH] Provider {}: 成功 {}/{}",
                self.provider_id,
                success_count,
                len(keys),
            )

        return {
            "success": success_count,
            "failed": failed_count,
            "total": len(keys),
            "results": results,
        }
