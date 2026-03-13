"""管理员 Provider 管理路由。"""

from __future__ import annotations

import asyncio
import uuid
from datetime import datetime, timezone
from typing import Any

from fastapi import APIRouter, Depends, Query, Request
from pydantic import BaseModel, ConfigDict, Field, ValidationError
from sqlalchemy import func
from sqlalchemy.orm import Session, load_only

from src.api.base.admin_adapter import AdminApiAdapter
from src.api.base.context import ApiRequestContext
from src.api.base.models_service import invalidate_models_list_cache
from src.api.base.pipeline import ApiRequestPipeline
from src.config.constants import CacheTTL
from src.core.enums import ProviderBillingType
from src.core.exceptions import InvalidRequestException, NotFoundException
from src.core.logger import logger
from src.core.model_permissions import match_model_with_pattern, parse_allowed_models_to_list
from src.core.provider_templates.fixed_providers import FIXED_PROVIDERS
from src.core.provider_templates.types import ProviderType
from src.database import get_db
from src.models.admin_requests import CreateProviderRequest, UpdateProviderRequest
from src.models.database import GlobalModel, Provider, ProviderAPIKey, ProviderEndpoint
from src.models.endpoint_models import ProviderWithEndpointsSummary
from src.services.cache.model_cache import ModelCacheService
from src.services.cache.provider_cache import ProviderCacheService
from src.services.provider.delete_task import get_provider_delete_task, submit_provider_delete
from src.utils.cache_decorator import cache_result

from .summary import _build_provider_summary

router = APIRouter(tags=["Provider CRUD"])
pipeline = ApiRequestPipeline()


# 映射预览配置（管理后台功能，限制宽松）
MAPPING_PREVIEW_MAX_KEYS = 200
MAPPING_PREVIEW_MAX_MODELS = 500
MAPPING_PREVIEW_TIMEOUT_SECONDS = 10.0


def _should_enable_format_conversion_by_default(provider_type: str | None) -> bool:
    """固定类型 Provider 默认是否开启格式转换。"""
    pt = (provider_type or "custom").strip().lower()
    envelope_provider_types = {
        ProviderType.ANTIGRAVITY.value,
        ProviderType.CLAUDE_CODE.value,
        ProviderType.CODEX.value,
        ProviderType.KIRO.value,
        ProviderType.VERTEX_AI.value,
    }
    return pt in envelope_provider_types


def _normalize_provider_type(provider_type: str | None) -> str:
    return (provider_type or "custom").strip().lower()


def _get_fixed_provider_template(provider_type: str | None) -> Any | None:
    """Return fixed-provider template when provider_type is managed by FIXED_PROVIDERS."""
    normalized = _normalize_provider_type(provider_type)
    try:
        return FIXED_PROVIDERS.get(ProviderType(normalized))
    except Exception:
        return None


def _resolve_new_provider_priority(
    current_min_priority: int | None, requested_priority: int | None
) -> tuple[int, bool]:
    """Resolve insertion priority for a newly created provider.

    Returns ``(priority, needs_shift)``.  When the caller explicitly specifies
    a priority we need to shift existing rows. For auto-top insertion we prefer
    ``min - 1`` when that still stays non-negative; otherwise we clamp to ``0``
    and shift existing rows down to preserve ordering.
    """
    if requested_priority is not None:
        return int(requested_priority), True
    if current_min_priority is not None:
        current_min = int(current_min_priority)
        if current_min <= 0:
            return 0, True
        return current_min - 1, False
    return 100, False


def _merge_pool_advanced_config(
    *,
    provider_config: dict[str, Any] | None,
    pool_advanced: dict[str, Any] | None,
    pool_advanced_in_payload: bool,
) -> tuple[dict[str, Any] | None, bool]:
    """合并 pool_advanced 到 provider.config（任何 provider_type 均可使用）。"""
    merged_config = dict(provider_config or {})
    config_changed = False

    if not pool_advanced_in_payload:
        return merged_config or None, config_changed

    if pool_advanced is None:
        if "pool_advanced" in merged_config:
            merged_config.pop("pool_advanced", None)
            config_changed = True
    else:
        next_value = dict(pool_advanced)
        if merged_config.get("pool_advanced") != next_value:
            merged_config["pool_advanced"] = next_value
            config_changed = True

    return merged_config or None, config_changed


def _merge_failover_rules_config(
    *,
    provider_config: dict[str, Any] | None,
    failover_rules: dict[str, Any] | None,
    failover_rules_in_payload: bool,
) -> tuple[dict[str, Any] | None, bool]:
    """合并 failover_rules 到 provider.config。"""
    merged_config = dict(provider_config or {})
    config_changed = False

    if not failover_rules_in_payload:
        return merged_config or None, config_changed

    if failover_rules is None:
        if "failover_rules" in merged_config:
            merged_config.pop("failover_rules", None)
            config_changed = True
    else:
        next_value = dict(failover_rules)
        if merged_config.get("failover_rules") != next_value:
            merged_config["failover_rules"] = next_value
            config_changed = True

    return merged_config or None, config_changed


def _merge_claude_code_advanced_config(
    *,
    provider_type: str | None,
    provider_config: dict[str, Any] | None,
    claude_code_advanced: dict[str, Any] | None,
    claude_advanced_in_payload: bool,
) -> tuple[dict[str, Any] | None, bool]:
    """合并并规范 claude_code_advanced，确保仅在 claude_code 下保留。"""
    normalized_provider_type = _normalize_provider_type(provider_type)
    merged_config = dict(provider_config or {})
    config_changed = False

    if normalized_provider_type != ProviderType.CLAUDE_CODE.value:
        if claude_advanced_in_payload and claude_code_advanced is not None:
            raise InvalidRequestException("claude_code_advanced 仅适用于 provider_type=claude_code")
        if "claude_code_advanced" in merged_config:
            merged_config.pop("claude_code_advanced", None)
            config_changed = True
        return merged_config or None, config_changed

    if not claude_advanced_in_payload:
        return merged_config or None, config_changed

    if claude_code_advanced is None:
        if "claude_code_advanced" in merged_config:
            merged_config.pop("claude_code_advanced", None)
            config_changed = True
    else:
        next_value = dict(claude_code_advanced)
        if merged_config.get("claude_code_advanced") != next_value:
            merged_config["claude_code_advanced"] = next_value
            config_changed = True

    return merged_config or None, config_changed


# ========== Response Models ==========


class MappingMatchedModel(BaseModel):
    """匹配到的模型名称"""

    allowed_model: str = Field(..., description="Key 白名单中匹配到的模型名")
    mapping_pattern: str = Field(..., description="匹配的映射规则")


class MappingMatchingGlobalModel(BaseModel):
    """有映射匹配的 GlobalModel"""

    global_model_id: str
    global_model_name: str
    display_name: str
    is_active: bool
    matched_models: list[MappingMatchedModel] = Field(
        default_factory=list, description="匹配到的模型列表"
    )

    model_config = ConfigDict(from_attributes=True)


class MappingMatchingKey(BaseModel):
    """有映射匹配的 Key"""

    key_id: str
    key_name: str
    masked_key: str
    is_active: bool
    allowed_models: list[str] = Field(default_factory=list, description="Key 的模型白名单")
    matching_global_models: list[MappingMatchingGlobalModel] = Field(
        default_factory=list, description="匹配到的 GlobalModel 列表"
    )

    model_config = ConfigDict(from_attributes=True)


class ProviderMappingPreviewResponse(BaseModel):
    """Provider 映射预览响应"""

    provider_id: str
    provider_name: str
    keys: list[MappingMatchingKey] = Field(
        default_factory=list, description="有白名单配置且匹配到映射的 Key 列表"
    )
    total_keys: int = Field(0, description="有匹配结果的 Key 数量")
    total_matches: int = Field(
        0, description="匹配到的 GlobalModel 数量（同一 GlobalModel 被多个 Key 匹配会重复计数）"
    )
    # 截断提示字段
    truncated: bool = Field(False, description="是否因限制而截断结果")
    truncated_keys: int = Field(0, description="被截断的 Key 数量")
    truncated_models: int = Field(0, description="被截断的 GlobalModel 数量")

    model_config = ConfigDict(from_attributes=True)


class ProviderDeleteSubmitResponse(BaseModel):
    task_id: str
    status: str = "pending"
    message: str = ""


class ProviderDeleteTaskResponse(BaseModel):
    task_id: str
    provider_id: str
    status: str
    stage: str = "queued"
    total_keys: int = 0
    deleted_keys: int = 0
    total_endpoints: int = 0
    deleted_endpoints: int = 0
    message: str = ""


@router.get("/")
async def list_providers(
    request: Request,
    skip: int = Query(0, ge=0),
    limit: int = Query(100, ge=1, le=500),
    is_active: bool | None = None,
    db: Session = Depends(get_db),
) -> Any:
    """
    获取提供商列表

    获取所有提供商的基本信息列表，支持分页和状态过滤。

    **查询参数**:
    - `skip`: 跳过的记录数，用于分页，默认为 0
    - `limit`: 返回的最大记录数，范围 1-500，默认为 100
    - `is_active`: 可选的活跃状态过滤，true 仅返回活跃提供商，false 返回禁用提供商，不传则返回全部

    **返回字段**:
    - `id`: 提供商 ID
    - `name`: 提供商名称（唯一）
    - `api_format`: API 格式（如 claude、openai、gemini 等）
    - `base_url`: API 基础 URL
    - `api_key`: API 密钥（脱敏显示）
    - `priority`: 优先级
    - `is_active`: 是否活跃
    - `created_at`: 创建时间
    - `updated_at`: 更新时间
    """
    adapter = AdminListProvidersAdapter(skip=skip, limit=limit, is_active=is_active)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.post("/")
async def create_provider(request: Request, db: Session = Depends(get_db)) -> Any:
    """
    创建新提供商

    创建一个新的 AI 模型提供商配置。

    **请求体字段**:
    - `name`: 提供商名称（必填，唯一）
    - `description`: 描述信息（可选）
    - `website`: 官网地址（可选）
    - `billing_type`: 计费类型（可选，pay_as_you_go/subscription/prepaid，默认 pay_as_you_go）
    - `monthly_quota_usd`: 月度配额（美元）（可选）
    - `quota_reset_day`: 配额重置日期（1-31）（可选）
    - `quota_last_reset_at`: 上次配额重置时间（可选）
    - `quota_expires_at`: 配额过期时间（可选）
    - `provider_priority`: 提供商优先级（数字越小优先级越高；不传时自动置顶，并将原有提供商顺延一位）
    - `is_active`: 是否启用（默认 true）
    - `concurrent_limit`: 并发限制（可选）
    - `max_retries`: 最大重试次数（可选）
    - `proxy`: 代理配置（可选）
    - `config`: 额外配置信息（JSON，可选）

    **返回字段**:
    - `id`: 新创建的提供商 ID
    - `name`: 提供商名称
    - `message`: 成功提示信息
    """
    adapter = AdminCreateProviderAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.patch("/{provider_id}")
async def update_provider(
    provider_id: str, request: Request, db: Session = Depends(get_db)
) -> ProviderWithEndpointsSummary:
    """
    更新提供商配置

    更新指定提供商的配置信息。只需传入需要更新的字段，未传入的字段保持不变。

    **路径参数**:
    - `provider_id`: 提供商 ID

    **请求体字段**（所有字段可选）:
    - `name`: 提供商名称
    - `description`: 描述信息
    - `website`: 官网地址
    - `billing_type`: 计费类型（pay_as_you_go/subscription/prepaid）
    - `monthly_quota_usd`: 月度配额（美元）
    - `quota_reset_day`: 配额重置日期（1-31）
    - `quota_last_reset_at`: 上次配额重置时间
    - `quota_expires_at`: 配额过期时间
    - `provider_priority`: 提供商优先级
    - `is_active`: 是否启用
    - `concurrent_limit`: 并发限制
    - `max_retries`: 最大重试次数
    - `proxy`: 代理配置
    - `config`: 额外配置信息（JSON）

    **返回字段**:
    - `id`: 提供商 ID
    - `name`: 提供商名称
    - `is_active`: 是否启用
    - `message`: 成功提示信息
    """
    adapter = AdminUpdateProviderAdapter(provider_id=provider_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.delete("/{provider_id}", response_model=ProviderDeleteSubmitResponse)
async def delete_provider(
    provider_id: str, request: Request, db: Session = Depends(get_db)
) -> ProviderDeleteSubmitResponse:
    """
    删除提供商

    删除指定的提供商。注意：此操作会级联删除关联的端点、密钥和模型配置。

    **路径参数**:
    - `provider_id`: 提供商 ID

    **返回字段**:
    - `task_id`: 后台删除任务 ID
    - `status`: 任务状态
    - `message`: 提交结果提示
    """
    adapter = AdminDeleteProviderAdapter(provider_id=provider_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/{provider_id}/delete-task/{task_id}", response_model=ProviderDeleteTaskResponse)
async def get_delete_provider_task_status(
    provider_id: str,
    task_id: str,
    request: Request,
    db: Session = Depends(get_db),
) -> ProviderDeleteTaskResponse:
    """查询 Provider 删除任务状态。"""
    adapter = AdminProviderDeleteTaskStatusAdapter(provider_id=provider_id, task_id=task_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


class AdminListProvidersAdapter(AdminApiAdapter):
    def __init__(self, skip: int, limit: int, is_active: bool | None):
        self.skip = skip
        self.limit = limit
        self.is_active = is_active

    @cache_result(
        key_prefix="admin:providers:list",
        ttl=CacheTTL.ADMIN_USAGE_RECORDS,
        user_specific=False,
        vary_by=["skip", "limit", "is_active"],
    )
    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        db = context.db
        query = db.query(Provider).options(
            load_only(
                Provider.id,
                Provider.name,
                Provider.provider_priority,
                Provider.is_active,
                Provider.created_at,
                Provider.updated_at,
            )
        )
        if self.is_active is not None:
            query = query.filter(Provider.is_active == self.is_active)
        providers = query.offset(self.skip).limit(self.limit).all()

        data = []
        for provider in providers:
            api_format = getattr(provider, "api_format", None)
            base_url = getattr(provider, "base_url", None)
            api_key = getattr(provider, "api_key", None)
            priority = getattr(provider, "priority", provider.provider_priority)

            data.append(
                {
                    "id": provider.id,
                    "name": provider.name,
                    "api_format": api_format.value if api_format else None,
                    "base_url": base_url,
                    "api_key": "***" if api_key else None,
                    "priority": priority,
                    "is_active": provider.is_active,
                    "created_at": provider.created_at.isoformat(),
                    "updated_at": provider.updated_at.isoformat() if provider.updated_at else None,
                }
            )
        context.add_audit_metadata(
            action="list_providers",
            filter_is_active=self.is_active,
            limit=self.limit,
            skip=self.skip,
            result_count=len(data),
        )
        return data


class AdminCreateProviderAdapter(AdminApiAdapter):
    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        db = context.db
        payload = context.ensure_json_body()

        try:
            # 使用 Pydantic 模型进行验证（自动进行 SQL 注入、XSS、SSRF 检测）
            validated_data = CreateProviderRequest.model_validate(payload)
        except ValidationError as exc:
            # 将 Pydantic 验证错误转换为友好的错误信息
            errors = []
            for error in exc.errors():
                field = " -> ".join(str(x) for x in error["loc"])
                errors.append(f"{field}: {error['msg']}")
            raise InvalidRequestException("输入验证失败: " + "; ".join(errors))

        try:
            # 检查名称是否已存在
            existing = db.query(Provider).filter(Provider.name == validated_data.name).first()
            if existing:
                raise InvalidRequestException(f"提供商名称 '{validated_data.name}' 已存在")

            # 将验证后的数据转换为枚举类型
            billing_type = (
                ProviderBillingType(validated_data.billing_type)
                if validated_data.billing_type
                else ProviderBillingType.PAY_AS_YOU_GO
            )

            # 有 envelope 包装的 Provider 类型（如 ClaudeCode、Antigravity、Codex）需要
            # 格式转换来正确解包上游响应，创建时默认开启 enable_format_conversion。
            pt = _normalize_provider_type(validated_data.provider_type)
            default_enable_format_conversion = _should_enable_format_conversion_by_default(pt)
            provider_config, _ = _merge_claude_code_advanced_config(
                provider_type=pt,
                provider_config=validated_data.config,
                claude_code_advanced=(
                    validated_data.claude_code_advanced.model_dump(exclude_none=True)
                    if validated_data.claude_code_advanced is not None
                    else None
                ),
                claude_advanced_in_payload=validated_data.claude_code_advanced is not None,
            )
            provider_config, _pool_changed = _merge_pool_advanced_config(
                provider_config=provider_config,
                pool_advanced=(
                    validated_data.pool_advanced.model_dump(exclude_none=True)
                    if validated_data.pool_advanced is not None
                    else None
                ),
                pool_advanced_in_payload=validated_data.pool_advanced is not None,
            )
            provider_config, _ = _merge_failover_rules_config(
                provider_config=provider_config,
                failover_rules=(
                    validated_data.failover_rules.model_dump()
                    if validated_data.failover_rules is not None
                    else None
                ),
                failover_rules_in_payload=validated_data.failover_rules is not None,
            )

            current_min_priority = db.query(func.min(Provider.provider_priority)).scalar()
            target_priority, needs_shift = _resolve_new_provider_priority(
                current_min_priority=current_min_priority,
                requested_priority=validated_data.provider_priority,
            )
            if needs_shift:
                db.query(Provider).filter(
                    Provider.provider_priority.isnot(None),
                    Provider.provider_priority >= target_priority,
                ).update(
                    {Provider.provider_priority: Provider.provider_priority + 1},
                    synchronize_session=False,
                )

            # 创建 Provider 对象
            provider = Provider(
                name=validated_data.name,
                provider_type=pt,
                description=validated_data.description,
                website=validated_data.website,
                billing_type=billing_type,
                monthly_quota_usd=validated_data.monthly_quota_usd,
                quota_reset_day=validated_data.quota_reset_day,
                quota_last_reset_at=validated_data.quota_last_reset_at,
                quota_expires_at=validated_data.quota_expires_at,
                provider_priority=target_priority,
                keep_priority_on_conversion=validated_data.keep_priority_on_conversion,
                is_active=validated_data.is_active,
                concurrent_limit=validated_data.concurrent_limit,
                max_retries=validated_data.max_retries,
                proxy=validated_data.proxy.model_dump() if validated_data.proxy else None,
                # 超时配置
                stream_first_byte_timeout=validated_data.stream_first_byte_timeout,
                request_timeout=validated_data.request_timeout,
                config=provider_config or None,
                # 有 envelope 的反代类型默认开启格式转换
                enable_format_conversion=default_enable_format_conversion,
            )

            db.add(provider)
            db.flush()  # flush 获取 ID，但不提交，保持在同一事务中

            # 固定类型 Provider：自动创建并锁定预置 Endpoints（同一事务）
            template = _get_fixed_provider_template(provider.provider_type)
            if template:
                from src.core.api_format.metadata import get_default_body_rules_for_endpoint

                now = datetime.now(timezone.utc)
                for sig in template.endpoint_signatures:
                    endpoint_config: dict[str, str] | None = None
                    if provider.provider_type == ProviderType.CODEX.value and sig == "openai:cli":
                        endpoint_config = {"upstream_stream_policy": "force_stream"}
                    # 获取 provider-scoped 默认 body rules
                    default_body_rules = (
                        get_default_body_rules_for_endpoint(
                            sig, provider_type=provider.provider_type
                        )
                        or None
                    )
                    endpoint = ProviderEndpoint(
                        id=str(uuid.uuid4()),
                        provider_id=provider.id,
                        api_format=sig,
                        api_family=sig.split(":", 1)[0],
                        endpoint_kind=sig.split(":", 1)[1],
                        base_url=template.api_base_url,
                        custom_path=None,
                        header_rules=None,
                        body_rules=default_body_rules,
                        max_retries=provider.max_retries or 2,
                        is_active=True,
                        config=endpoint_config,
                        proxy=None,
                        format_acceptance_config=None,
                        created_at=now,
                        updated_at=now,
                    )
                    db.add(endpoint)

            db.commit()
            db.refresh(provider)

            # 清除 /v1/models 列表缓存
            await invalidate_models_list_cache()

            context.add_audit_metadata(
                action="create_provider",
                provider_id=provider.id,
                provider_name=provider.name,
                billing_type=provider.billing_type.value if provider.billing_type else None,
                is_active=provider.is_active,
                provider_priority=provider.provider_priority,
            )

            return {
                "id": provider.id,
                "name": provider.name,
                "message": "提供商创建成功",
            }
        except InvalidRequestException:
            db.rollback()
            raise
        except Exception:
            db.rollback()
            raise


class AdminUpdateProviderAdapter(AdminApiAdapter):
    def __init__(self, provider_id: str):
        self.provider_id = provider_id

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        db = context.db
        payload = context.ensure_json_body()

        # 查找 Provider
        provider = db.query(Provider).filter(Provider.id == self.provider_id).first()
        if not provider:
            raise NotFoundException("提供商不存在", "provider")

        try:
            # 使用 Pydantic 模型进行验证（自动进行 SQL 注入、XSS、SSRF 检测）
            validated_data = UpdateProviderRequest.model_validate(payload)
        except ValidationError as exc:
            # 将 Pydantic 验证错误转换为友好的错误信息
            errors = []
            for error in exc.errors():
                field = " -> ".join(str(x) for x in error["loc"])
                errors.append(f"{field}: {error['msg']}")
            raise InvalidRequestException("输入验证失败: " + "; ".join(errors))

        try:
            # 更新字段（只更新非 None 的字段）
            update_data = validated_data.model_dump(exclude_unset=True)
            config_in_payload = "config" in update_data
            claude_advanced_in_payload = "claude_code_advanced" in update_data
            pool_advanced_in_payload = "pool_advanced" in update_data
            failover_rules_in_payload = "failover_rules" in update_data
            provider_config = (
                dict(update_data.pop("config") or {})
                if config_in_payload
                else dict(provider.config or {})
            )
            claude_advanced = (
                update_data.pop("claude_code_advanced") if claude_advanced_in_payload else None
            )
            pool_advanced = update_data.pop("pool_advanced") if pool_advanced_in_payload else None
            failover_rules = (
                update_data.pop("failover_rules") if failover_rules_in_payload else None
            )
            target_provider_type = (
                update_data.get("provider_type")
                or getattr(provider, "provider_type", None)
                or "custom"
            )

            provider_config, config_changed_by_claude = _merge_claude_code_advanced_config(
                provider_type=target_provider_type,
                provider_config=provider_config,
                claude_code_advanced=claude_advanced,
                claude_advanced_in_payload=claude_advanced_in_payload,
            )
            provider_config, config_changed_by_pool = _merge_pool_advanced_config(
                provider_config=provider_config,
                pool_advanced=pool_advanced,
                pool_advanced_in_payload=pool_advanced_in_payload,
            )
            provider_config, config_changed_by_failover = _merge_failover_rules_config(
                provider_config=provider_config,
                failover_rules=failover_rules,
                failover_rules_in_payload=failover_rules_in_payload,
            )

            config_touched = (
                config_in_payload
                or claude_advanced_in_payload
                or config_changed_by_claude
                or pool_advanced_in_payload
                or config_changed_by_pool
                or failover_rules_in_payload
                or config_changed_by_failover
            )
            if config_touched:
                update_data["config"] = provider_config

            for field, value in update_data.items():
                if field == "billing_type" and value is not None:
                    # billing_type 需要转换为枚举
                    setattr(provider, field, ProviderBillingType(value))
                elif field == "provider_type" and value is not None:
                    setattr(provider, field, value)
                elif field == "proxy" and value is not None:
                    # proxy 需要转换为 dict（如果是 Pydantic 模型）
                    setattr(
                        provider, field, value if isinstance(value, dict) else value.model_dump()
                    )
                else:
                    setattr(provider, field, value)

            provider.updated_at = datetime.now(timezone.utc)
            db.commit()
            db.refresh(provider)

            # 清除 /v1/models 列表缓存（is_active 变更会影响模型可用性）
            await invalidate_models_list_cache()

            # 如果更新了 is_active，清除 GlobalModel 解析缓存
            # Provider 状态变更会影响模型解析结果
            if "is_active" in update_data:
                await ModelCacheService.invalidate_all_resolve_cache()

            # 如果更新了 billing_type，清除缓存
            if "billing_type" in update_data:
                await ProviderCacheService.invalidate_provider_cache(provider.id)
                logger.debug(f"已清除 Provider 缓存: {provider.id}")

            context.add_audit_metadata(
                action="update_provider",
                provider_id=provider.id,
                changed_fields=list(update_data.keys()),
                is_active=provider.is_active,
                provider_priority=provider.provider_priority,
            )

            return _build_provider_summary(db, provider)
        except InvalidRequestException:
            db.rollback()
            raise
        except Exception:
            db.rollback()
            raise


class AdminDeleteProviderAdapter(AdminApiAdapter):
    def __init__(self, provider_id: str):
        self.provider_id = provider_id

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        db = context.db
        provider = db.query(Provider).filter(Provider.id == self.provider_id).first()
        if not provider:
            raise NotFoundException("提供商不存在", "provider")

        context.add_audit_metadata(
            action="delete_provider",
            provider_id=provider.id,
            provider_name=provider.name,
        )

        task_id = await submit_provider_delete(provider.id)

        provider_was_active = bool(provider.is_active)
        if provider_was_active:
            provider.is_active = False
            db.commit()
            await invalidate_models_list_cache()
            await ModelCacheService.invalidate_all_resolve_cache()
            await ProviderCacheService.invalidate_provider_cache(provider.id)

        context.add_audit_metadata(
            task_id=task_id,
            provider_deactivated=provider_was_active,
        )
        return {
            "task_id": task_id,
            "status": "pending",
            "message": "删除任务已提交，提供商已进入后台删除队列",
        }


class AdminProviderDeleteTaskStatusAdapter(AdminApiAdapter):
    def __init__(self, provider_id: str, task_id: str):
        self.provider_id = provider_id
        self.task_id = task_id

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        from fastapi import HTTPException

        task = await get_provider_delete_task(self.task_id)
        if task is None or task.provider_id != self.provider_id:
            raise HTTPException(status_code=404, detail="Task not found")
        return ProviderDeleteTaskResponse(
            task_id=task.task_id,
            provider_id=task.provider_id,
            status=task.status,
            stage=task.stage,
            total_keys=task.total_keys,
            deleted_keys=task.deleted_keys,
            total_endpoints=task.total_endpoints,
            deleted_endpoints=task.deleted_endpoints,
            message=task.message,
        )


@router.get(
    "/{provider_id}/mapping-preview",
    response_model=ProviderMappingPreviewResponse,
)
async def get_provider_mapping_preview(
    request: Request,
    provider_id: str,
    db: Session = Depends(get_db),
) -> ProviderMappingPreviewResponse:
    """
    获取 Provider 映射预览

    查看该 Provider 的 Key 白名单能够被哪些 GlobalModel 的映射规则匹配。

    **路径参数**:
    - `provider_id`: Provider ID

    **返回字段**:
    - `provider_id`: Provider ID
    - `provider_name`: Provider 名称
    - `keys`: 有白名单配置的 Key 列表，每个包含：
      - `key_id`: Key ID
      - `key_name`: Key 名称
      - `masked_key`: 脱敏的 Key
      - `allowed_models`: Key 的白名单模型列表
      - `matching_global_models`: 匹配到的 GlobalModel 列表
    - `total_keys`: 有白名单配置的 Key 总数
    - `total_matches`: 匹配到的 GlobalModel 总数
    """
    adapter = AdminGetProviderMappingPreviewAdapter(provider_id=provider_id)

    # 添加超时保护，防止复杂匹配导致的 DoS
    try:
        return await asyncio.wait_for(
            pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode),
            timeout=MAPPING_PREVIEW_TIMEOUT_SECONDS,
        )
    except TimeoutError:
        logger.warning(f"映射预览超时: provider_id={provider_id}")
        raise InvalidRequestException("映射预览超时，请简化配置或稍后重试")


class AdminGetProviderMappingPreviewAdapter(AdminApiAdapter):
    """获取 Provider 映射预览"""

    def __init__(self, provider_id: str):
        self.provider_id = provider_id

    @cache_result(
        key_prefix="admin:providers:mapping-preview",
        ttl=CacheTTL.ADMIN_USAGE_RECORDS,
        user_specific=False,
        vary_by=["provider_id"],
    )
    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        db = context.db

        # 获取 Provider
        provider = (
            db.query(Provider)
            .options(load_only(Provider.id, Provider.name))
            .filter(Provider.id == self.provider_id)
            .first()
        )
        if not provider:
            raise NotFoundException("提供商不存在", "provider")

        # 统计截断情况
        truncated_keys = 0
        truncated_models = 0

        # 获取该 Provider 有白名单配置的 Key（只查询需要的字段）
        keys = (
            db.query(
                ProviderAPIKey.id,
                ProviderAPIKey.name,
                ProviderAPIKey.api_key,
                ProviderAPIKey.is_active,
                ProviderAPIKey.allowed_models,
            )
            .filter(
                ProviderAPIKey.provider_id == self.provider_id,
                ProviderAPIKey.allowed_models.isnot(None),
            )
            .limit(MAPPING_PREVIEW_MAX_KEYS + 1)
            .all()
        )

        if len(keys) > MAPPING_PREVIEW_MAX_KEYS:
            keys = keys[:MAPPING_PREVIEW_MAX_KEYS]
            total_keys_with_allowed_models = (
                db.query(func.count(ProviderAPIKey.id))
                .filter(
                    ProviderAPIKey.provider_id == self.provider_id,
                    ProviderAPIKey.allowed_models.isnot(None),
                )
                .scalar()
                or 0
            )
            truncated_keys = total_keys_with_allowed_models - MAPPING_PREVIEW_MAX_KEYS

        # 只查询有 model_mappings 配置的 GlobalModel（使用 SQLAlchemy JSONB 操作符）
        global_models = (
            db.query(
                GlobalModel.id,
                GlobalModel.name,
                GlobalModel.display_name,
                GlobalModel.is_active,
                GlobalModel.config,
            )
            .filter(
                GlobalModel.config.isnot(None),
                GlobalModel.config["model_mappings"].isnot(None),
                func.jsonb_array_length(GlobalModel.config["model_mappings"]) > 0,
            )
            .limit(MAPPING_PREVIEW_MAX_MODELS + 1)
            .all()
        )

        if len(global_models) > MAPPING_PREVIEW_MAX_MODELS:
            global_models = global_models[:MAPPING_PREVIEW_MAX_MODELS]
            total_models_with_mappings = (
                db.query(func.count(GlobalModel.id))
                .filter(
                    GlobalModel.config.isnot(None),
                    GlobalModel.config["model_mappings"].isnot(None),
                    func.jsonb_array_length(GlobalModel.config["model_mappings"]) > 0,
                )
                .scalar()
                or 0
            )
            truncated_models = total_models_with_mappings - MAPPING_PREVIEW_MAX_MODELS

        # 构建有映射配置的 GlobalModel 映射
        models_with_mappings: dict[str, tuple] = {}  # id -> (model_info, mappings)
        for gm in global_models:
            config = gm.config or {}
            mappings = config.get("model_mappings", [])
            if mappings:
                models_with_mappings[gm.id] = (gm, mappings)

        # 如果没有任何带映射的 GlobalModel，直接返回空结果
        if not models_with_mappings:
            return ProviderMappingPreviewResponse(
                provider_id=provider.id,
                provider_name=provider.name,
                keys=[],
                total_keys=0,
                total_matches=0,
                truncated=truncated_keys > 0 or truncated_models > 0,
                truncated_keys=truncated_keys,
                truncated_models=truncated_models,
            ).model_dump()

        key_infos: list[MappingMatchingKey] = []
        total_matches = 0

        # 创建 CryptoService 实例
        from src.core.crypto import CryptoService

        crypto = CryptoService()

        for key in keys:
            allowed_models_list = parse_allowed_models_to_list(key.allowed_models)
            if not allowed_models_list:
                continue

            # 查找匹配的 GlobalModel
            matching_global_models: list[MappingMatchingGlobalModel] = []

            for gm_id, (gm, mappings) in models_with_mappings.items():
                matched_models: list[MappingMatchedModel] = []

                for allowed_model in allowed_models_list:
                    for mapping_pattern in mappings:
                        if match_model_with_pattern(mapping_pattern, allowed_model):
                            matched_models.append(
                                MappingMatchedModel(
                                    allowed_model=allowed_model,
                                    mapping_pattern=mapping_pattern,
                                )
                            )
                            break  # 一个 allowed_model 只需匹配一个映射

                if matched_models:
                    matching_global_models.append(
                        MappingMatchingGlobalModel(
                            global_model_id=gm.id,
                            global_model_name=gm.name,
                            display_name=gm.display_name,
                            is_active=bool(gm.is_active),
                            matched_models=matched_models,
                        )
                    )
                    total_matches += 1

            if matching_global_models:
                # 只有有匹配结果的 key 才做解密脱敏，减少 CPU 开销
                masked_key = "***"
                if key.api_key:
                    try:
                        decrypted_key = crypto.decrypt(key.api_key, silent=True)
                        if len(decrypted_key) > 8:
                            masked_key = f"{decrypted_key[:4]}***{decrypted_key[-4:]}"
                        else:
                            masked_key = f"{decrypted_key[:2]}***"
                    except Exception:
                        pass

                key_infos.append(
                    MappingMatchingKey(
                        key_id=key.id or "",
                        key_name=key.name or "",
                        masked_key=masked_key,
                        is_active=bool(key.is_active),
                        allowed_models=allowed_models_list,
                        matching_global_models=matching_global_models,
                    )
                )

        is_truncated = truncated_keys > 0 or truncated_models > 0

        return ProviderMappingPreviewResponse(
            provider_id=provider.id,
            provider_name=provider.name,
            keys=key_infos,
            total_keys=len(key_infos),
            total_matches=total_matches,
            truncated=is_truncated,
            truncated_keys=truncated_keys,
            truncated_models=truncated_models,
        ).model_dump()


# ========== Claude Code Pool Management ==========


class PoolKeyStatus(BaseModel):
    """Single key's pool status."""

    key_id: str
    key_name: str
    is_active: bool
    cooldown_reason: str | None = None
    cooldown_ttl_seconds: int | None = None
    cost_window_usage: int = 0
    cost_limit: int | None = None
    sticky_sessions: int = 0
    lru_score: float | None = None

    model_config = ConfigDict(from_attributes=True)


class PoolStatusResponse(BaseModel):
    """Pool status for a Provider with pool config."""

    provider_id: str
    provider_name: str
    pool_enabled: bool = False
    total_keys: int = 0
    total_sticky_sessions: int = 0
    keys: list[PoolKeyStatus] = Field(default_factory=list)

    model_config = ConfigDict(from_attributes=True)


@router.get("/{provider_id}/pool-status", response_model=PoolStatusResponse)
async def get_pool_status(
    request: Request,
    provider_id: str,
    db: Session = Depends(get_db),
) -> PoolStatusResponse:
    """获取 Provider 的号池状态。"""
    provider = db.query(Provider).filter(Provider.id == provider_id).first()
    if not provider:
        raise NotFoundException("提供商不存在", "provider")

    from src.services.provider.pool import redis_ops as pool_redis
    from src.services.provider.pool.config import parse_pool_config

    pcfg = parse_pool_config(provider.config)
    if pcfg is None:
        return PoolStatusResponse(
            provider_id=provider.id,
            provider_name=provider.name,
            pool_enabled=False,
        )

    keys = db.query(ProviderAPIKey).filter(ProviderAPIKey.provider_id == provider_id).all()

    key_ids = [str(k.id) for k in keys]
    pid = str(provider.id)

    import asyncio

    # Batch fetch pool state (parallel)
    lru_coro = (
        pool_redis.get_lru_scores(pid, key_ids) if pcfg.lru_enabled else asyncio.sleep(0, result={})
    )
    cooldowns, cooldown_ttls, lru_scores, cost_totals, total_sticky = await asyncio.gather(
        pool_redis.batch_get_cooldowns(pid, key_ids),
        pool_redis.batch_get_cooldown_ttls(pid, key_ids),
        lru_coro,
        pool_redis.batch_get_cost_totals(pid, key_ids, pcfg.cost_window_seconds),
        pool_redis.get_sticky_session_count(pid),
    )

    # Sticky count per key requires SCAN+MGET; batch with gather.
    sticky_counts: dict[str, int] = {}
    if key_ids:
        counts = await asyncio.gather(
            *(pool_redis.get_key_sticky_count(pid, kid) for kid in key_ids)
        )
        sticky_counts = dict(zip(key_ids, counts))

    key_statuses: list[PoolKeyStatus] = []
    for k in keys:
        kid = str(k.id)
        cd_reason = cooldowns.get(kid)

        key_statuses.append(
            PoolKeyStatus(
                key_id=kid,
                key_name=k.name or "",
                is_active=bool(k.is_active),
                cooldown_reason=cd_reason,
                cooldown_ttl_seconds=cooldown_ttls.get(kid) if cd_reason else None,
                cost_window_usage=cost_totals.get(kid, 0),
                cost_limit=pcfg.cost_limit_per_key_tokens,
                sticky_sessions=sticky_counts.get(kid, 0),
                lru_score=lru_scores.get(kid),
            )
        )

    return PoolStatusResponse(
        provider_id=provider.id,
        provider_name=provider.name,
        pool_enabled=True,
        total_keys=len(keys),
        total_sticky_sessions=total_sticky,
        keys=key_statuses,
    )


@router.post("/{provider_id}/pool/clear-cooldown/{key_id}")
async def clear_pool_cooldown(
    request: Request,
    provider_id: str,
    key_id: str,
    db: Session = Depends(get_db),
) -> dict[str, str]:
    """手动清除指定 Key 的号池冷却状态。"""
    provider = db.query(Provider).filter(Provider.id == provider_id).first()
    if not provider:
        raise NotFoundException("提供商不存在", "provider")

    key = (
        db.query(ProviderAPIKey)
        .filter(ProviderAPIKey.id == key_id, ProviderAPIKey.provider_id == provider_id)
        .first()
    )
    if not key:
        raise NotFoundException("密钥不存在", "key")

    from src.services.provider.pool import redis_ops as pool_redis

    await pool_redis.clear_cooldown(str(provider.id), str(key.id))
    return {"message": f"已清除 Key {key.name or key_id} 的冷却状态"}


@router.post("/{provider_id}/pool/reset-cost/{key_id}")
async def reset_pool_cost(
    request: Request,
    provider_id: str,
    key_id: str,
    db: Session = Depends(get_db),
) -> dict[str, str]:
    """重置指定 Key 的号池成本窗口。"""
    provider = db.query(Provider).filter(Provider.id == provider_id).first()
    if not provider:
        raise NotFoundException("提供商不存在", "provider")

    key = (
        db.query(ProviderAPIKey)
        .filter(ProviderAPIKey.id == key_id, ProviderAPIKey.provider_id == provider_id)
        .first()
    )
    if not key:
        raise NotFoundException("密钥不存在", "key")

    from src.services.provider.pool import redis_ops as pool_redis

    await pool_redis.clear_cost(str(provider.id), str(key.id))
    return {"message": f"已重置 Key {key.name or key_id} 的成本窗口"}
