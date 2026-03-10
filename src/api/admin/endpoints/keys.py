"""
Provider API Keys 管理
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any

from fastapi import APIRouter, Body, Depends, Query, Request
from pydantic import BaseModel, Field
from sqlalchemy.orm import Session

from src.api.base.admin_adapter import AdminApiAdapter
from src.api.base.context import ApiRequestContext
from src.api.base.pipeline import ApiRequestPipeline
from src.database import get_db
from src.models.database import User
from src.models.endpoint_models import (
    EndpointAPIKeyCreate,
    EndpointAPIKeyResponse,
    EndpointAPIKeyUpdate,
)
from src.services.provider_keys import (
    batch_delete_endpoint_keys_response,
    clear_oauth_invalid_response,
    create_provider_key_response,
    delete_endpoint_key_response,
    export_oauth_key_data,
)
from src.services.provider_keys import get_keys_grouped_by_format as query_keys_grouped_by_format
from src.services.provider_keys import (
    list_provider_keys_responses,
    refresh_provider_quota_for_provider,
    reveal_endpoint_key_payload,
    update_endpoint_key_response,
)
from src.services.provider_keys.key_quota_service import (
    CODEX_WHAM_USAGE_URL as _CODEX_WHAM_USAGE_URL,
)
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


@router.post("/keys/batch-delete")
async def batch_delete_endpoint_keys(
    request: Request,
    ids: list[str] = Body(..., embed=True, max_length=100),
    db: Session = Depends(get_db),
) -> dict:
    """
    批量删除 Provider Keys

    一次性删除多个 Key，按 Provider 聚合执行副作用（缓存失效、模型关联检查），
    避免逐个删除导致的重复 Redis 操作和性能问题。

    **请求体字段**:
    - `ids`: Key ID 列表（最多 100 个）

    **返回字段**:
    - `success_count`: 成功删除的数量
    - `failed_count`: 失败的数量
    - `failed`: 失败的详情列表
    """
    adapter = AdminBatchDeleteEndpointKeysAdapter(ids=ids)
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
    adapter = AdminClearOAuthInvalidAdapter(key_id=key_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


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


@dataclass
class AdminUpdateEndpointKeyAdapter(AdminApiAdapter):
    key_id: str
    key_data: EndpointAPIKeyUpdate

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        return await update_endpoint_key_response(
            db=context.db,
            key_id=self.key_id,
            key_data=self.key_data,
        )


@dataclass
class AdminRevealEndpointKeyAdapter(AdminApiAdapter):
    """获取完整的 API Key 或 Auth Config（用于查看和复制）"""

    key_id: str

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        return reveal_endpoint_key_payload(context.db, self.key_id)


@dataclass
class AdminExportKeyAdapter(AdminApiAdapter):
    """导出 OAuth Key 凭据：解密 auth_config，委托 provider-specific builder 构建导出数据。"""

    key_id: str

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        return export_oauth_key_data(context.db, self.key_id)


@dataclass
class AdminDeleteEndpointKeyAdapter(AdminApiAdapter):
    key_id: str

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        return await delete_endpoint_key_response(db=context.db, key_id=self.key_id)


@dataclass
class AdminBatchDeleteEndpointKeysAdapter(AdminApiAdapter):
    """批量删除多个 Provider Key"""

    ids: list[str]

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        return await batch_delete_endpoint_keys_response(db=context.db, key_ids=self.ids)


@dataclass
class AdminClearOAuthInvalidAdapter(AdminApiAdapter):
    """清除 Key 的 OAuth 失效标记。"""

    key_id: str

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        return clear_oauth_invalid_response(context.db, self.key_id)


class AdminGetKeysGroupedByFormatAdapter(AdminApiAdapter):
    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        return query_keys_grouped_by_format(context.db)


# ========== Adapters ==========


@dataclass
class AdminListProviderKeysAdapter(AdminApiAdapter):
    """获取 Provider 的所有 Keys"""

    provider_id: str
    skip: int
    limit: int

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        return list_provider_keys_responses(context.db, self.provider_id, self.skip, self.limit)


@dataclass
class AdminCreateProviderKeyAdapter(AdminApiAdapter):
    """为 Provider 添加 Key"""

    provider_id: str
    key_data: EndpointAPIKeyCreate

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        return await create_provider_key_response(
            db=context.db,
            provider_id=self.provider_id,
            key_data=self.key_data,
        )


# ========== Quota Refresh API ==========


class RefreshProviderQuotaRequest(BaseModel):
    key_ids: list[str] | None = Field(default=None, description="仅刷新指定 Key 列表（可选）")


@router.post("/providers/{provider_id}/refresh-quota")
async def refresh_provider_quota(
    provider_id: str,
    request: Request,
    payload: RefreshProviderQuotaRequest | None = None,
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
    **请求体**（可选）:
    - `key_ids`: 仅刷新指定 Key 列表，不传时刷新所有活跃 Key

    **返回字段**:
    - `success`: 成功刷新的 Key 数量
    - `failed`: 失败的 Key 数量
    - `results`: 每个 Key 的刷新结果
    """
    adapter = AdminRefreshProviderQuotaAdapter(
        provider_id=provider_id,
        key_ids=payload.key_ids if payload else None,
    )
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@dataclass
class AdminRefreshProviderQuotaAdapter(AdminApiAdapter):
    """刷新 Provider 所有 Keys 的限额信息"""

    provider_id: str
    key_ids: list[str] | None = None

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        return await refresh_provider_quota_for_provider(
            db=context.db,
            provider_id=self.provider_id,
            codex_wham_usage_url=_CODEX_WHAM_USAGE_URL,
            key_ids=self.key_ids,
        )
