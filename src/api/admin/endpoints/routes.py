"""
ProviderEndpoint CRUD 管理 API
"""

from __future__ import annotations

import uuid
from dataclasses import dataclass
from datetime import datetime, timezone
from typing import Any

from fastapi import APIRouter, Depends, Query, Request
from sqlalchemy import and_
from sqlalchemy.orm import Session
from sqlalchemy.orm.attributes import flag_modified

from src.api.base.admin_adapter import AdminApiAdapter
from src.api.base.context import ApiRequestContext
from src.api.base.models_service import invalidate_models_list_cache
from src.api.base.pipeline import ApiRequestPipeline
from src.core.api_format.signature import parse_signature_key
from src.core.exceptions import InvalidRequestException, NotFoundException
from src.core.logger import logger
from src.database import get_db
from src.models.database import Provider, ProviderAPIKey, ProviderEndpoint
from src.models.endpoint_models import (
    ProviderEndpointCreate,
    ProviderEndpointResponse,
    ProviderEndpointUpdate,
)
from src.services.system.config import SystemConfigService

router = APIRouter(tags=["Endpoint Management"])
pipeline = ApiRequestPipeline()


def mask_proxy_password(proxy_config: dict | None) -> dict | None:
    """对代理配置中的密码进行脱敏处理"""
    if not proxy_config:
        return None
    masked = dict(proxy_config)
    if masked.get("password"):
        masked["password"] = "***"
    return masked


def _compute_endpoint_format_conversion(
    endpoint: ProviderEndpoint, provider: Provider, db: Session
) -> tuple[bool, str | None]:
    """计算端点格式转换的有效值和继承来源

    继承链：端点 > 提供商 > 全局
    Returns:
        (effective_value, inherited_from)
        inherited_from: None=自身设置, 'provider'=继承提供商, 'global'=继承全局
    """
    # 端点自身配置
    endpoint_config = endpoint.format_acceptance_config
    if endpoint_config is not None and endpoint_config.get("enabled") is not None:
        enabled = endpoint_config.get("enabled")
        if enabled is True or enabled is False:
            return bool(enabled), None

    # 继承提供商设置
    if provider.enable_format_conversion is not None:
        return bool(provider.enable_format_conversion), "provider"

    # 继承全局设置
    global_value = SystemConfigService.get_config(db, "enable_format_conversion", False)
    return bool(global_value), "global"


@router.get("/providers/{provider_id}/endpoints", response_model=list[ProviderEndpointResponse])
async def list_provider_endpoints(
    provider_id: str,
    request: Request,
    skip: int = Query(0, ge=0, description="跳过的记录数"),
    limit: int = Query(100, ge=1, le=1000, description="返回的最大记录数"),
    db: Session = Depends(get_db),
) -> list[ProviderEndpointResponse]:
    """
    获取指定 Provider 的所有 Endpoints

    获取指定 Provider 下的所有 Endpoint 列表，包括配置、统计信息等。
    结果按创建时间倒序排列。

    **路径参数**:
    - `provider_id`: Provider ID

    **查询参数**:
    - `skip`: 跳过的记录数，用于分页（默认 0）
    - `limit`: 返回的最大记录数（1-1000，默认 100）

    **返回字段**:
    - `id`: Endpoint ID
    - `provider_id`: Provider ID
    - `provider_name`: Provider 名称
    - `api_format`: API 格式
    - `base_url`: 基础 URL
    - `custom_path`: 自定义路径
    - `max_retries`: 最大重试次数
    - `is_active`: 是否活跃
    - `total_keys`: Key 总数
    - `active_keys`: 活跃 Key 数量
    - `proxy`: 代理配置（密码已脱敏）
    - 其他配置字段
    """
    adapter = AdminListProviderEndpointsAdapter(
        provider_id=provider_id,
        skip=skip,
        limit=limit,
    )
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.post("/providers/{provider_id}/endpoints", response_model=ProviderEndpointResponse)
async def create_provider_endpoint(
    provider_id: str,
    endpoint_data: ProviderEndpointCreate,
    request: Request,
    db: Session = Depends(get_db),
) -> ProviderEndpointResponse:
    """
    为 Provider 创建新的 Endpoint

    为指定 Provider 创建新的 Endpoint，每个 Provider 的每种 API 格式
    只能创建一个 Endpoint。

    **路径参数**:
    - `provider_id`: Provider ID

    **请求体字段**:
    - `provider_id`: Provider ID（必须与路径参数一致）
    - `api_format`: API 格式（如 claude、openai、gemini 等）
    - `base_url`: 基础 URL
    - `custom_path`: 自定义路径（可选）
    - `header_rules`: 请求头规则列表（可选，支持 set/drop/rename 操作）
    - `max_retries`: 最大重试次数（默认 2）
    - `config`: 额外配置（可选）
    - `proxy`: 代理配置（可选）

    **返回字段**:
    - 包含完整的 Endpoint 信息
    """
    adapter = AdminCreateProviderEndpointAdapter(
        provider_id=provider_id,
        endpoint_data=endpoint_data,
    )
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/{endpoint_id}", response_model=ProviderEndpointResponse)
async def get_endpoint(
    endpoint_id: str,
    request: Request,
    db: Session = Depends(get_db),
) -> ProviderEndpointResponse:
    """
    获取 Endpoint 详情

    获取指定 Endpoint 的详细信息，包括配置、统计信息等。

    **路径参数**:
    - `endpoint_id`: Endpoint ID

    **返回字段**:
    - `id`: Endpoint ID
    - `provider_id`: Provider ID
    - `provider_name`: Provider 名称
    - `api_format`: API 格式
    - `base_url`: 基础 URL
    - `custom_path`: 自定义路径
    - `max_retries`: 最大重试次数
    - `is_active`: 是否活跃
    - `total_keys`: Key 总数
    - `active_keys`: 活跃 Key 数量
    - `proxy`: 代理配置（密码已脱敏）
    - 其他配置字段
    """
    adapter = AdminGetProviderEndpointAdapter(endpoint_id=endpoint_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.put("/{endpoint_id}", response_model=ProviderEndpointResponse)
async def update_endpoint(
    endpoint_id: str,
    endpoint_data: ProviderEndpointUpdate,
    request: Request,
    db: Session = Depends(get_db),
) -> ProviderEndpointResponse:
    """
    更新 Endpoint

    更新指定 Endpoint 的配置。支持部分更新。

    **路径参数**:
    - `endpoint_id`: Endpoint ID

    **请求体字段**（均为可选）:
    - `base_url`: 基础 URL
    - `custom_path`: 自定义路径
    - `header_rules`: 请求头规则列表
    - `max_retries`: 最大重试次数
    - `is_active`: 是否活跃
    - `config`: 额外配置
    - `proxy`: 代理配置（设置为 null 可清除代理）

    **返回字段**:
    - 包含更新后的完整 Endpoint 信息
    """
    adapter = AdminUpdateProviderEndpointAdapter(
        endpoint_id=endpoint_id,
        endpoint_data=endpoint_data,
    )
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.delete("/{endpoint_id}")
async def delete_endpoint(
    endpoint_id: str,
    request: Request,
    db: Session = Depends(get_db),
) -> dict:
    """
    删除 Endpoint

    删除指定的 Endpoint，会影响该 Provider 在该 API 格式下的路由能力。
    Key 不会被删除，但包含该 API 格式的 Key 将无法被调度使用（直到重新创建该格式的 Endpoint）。

    **路径参数**:
    - `endpoint_id`: Endpoint ID

    **返回字段**:
    - `message`: 操作结果消息
    - `affected_keys_count`: 受影响的 Key 数量（包含该 API 格式）
    """
    adapter = AdminDeleteProviderEndpointAdapter(endpoint_id=endpoint_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


# -------- Adapters --------


@dataclass
class AdminListProviderEndpointsAdapter(AdminApiAdapter):
    provider_id: str
    skip: int
    limit: int

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        db = context.db
        provider = db.query(Provider).filter(Provider.id == self.provider_id).first()
        if not provider:
            raise NotFoundException(f"Provider {self.provider_id} 不存在")

        endpoints = (
            db.query(ProviderEndpoint)
            .filter(ProviderEndpoint.provider_id == self.provider_id)
            .order_by(ProviderEndpoint.created_at.desc())
            .offset(self.skip)
            .limit(self.limit)
            .all()
        )

        # Key 是 Provider 级别资源：按 key.api_formats 归类到各 Endpoint.api_format 下
        keys = (
            db.query(ProviderAPIKey.api_formats, ProviderAPIKey.is_active)
            .filter(ProviderAPIKey.provider_id == self.provider_id)
            .all()
        )
        total_keys_map: dict[str, int] = {}
        active_keys_map: dict[str, int] = {}
        for api_formats, is_active in keys:
            for fmt in api_formats or []:
                total_keys_map[fmt] = total_keys_map.get(fmt, 0) + 1
                if is_active:
                    active_keys_map[fmt] = active_keys_map.get(fmt, 0) + 1

        result: list[ProviderEndpointResponse] = []
        for endpoint in endpoints:
            endpoint_format = (
                endpoint.api_format
                if isinstance(endpoint.api_format, str)
                else endpoint.api_format.value
            )
            # 计算格式转换的有效值和继承来源
            effective, inherited_from = _compute_endpoint_format_conversion(
                endpoint, provider, db
            )
            endpoint_dict = {
                **endpoint.__dict__,
                "provider_name": provider.name,
                "api_format": endpoint.api_format,
                "total_keys": total_keys_map.get(endpoint_format, 0),
                "active_keys": active_keys_map.get(endpoint_format, 0),
                "proxy": mask_proxy_password(endpoint.proxy),
                "format_conversion_effective": effective,
                "format_conversion_inherited_from": inherited_from,
            }
            endpoint_dict.pop("_sa_instance_state", None)
            result.append(ProviderEndpointResponse(**endpoint_dict))

        return result


@dataclass
class AdminCreateProviderEndpointAdapter(AdminApiAdapter):
    provider_id: str
    endpoint_data: ProviderEndpointCreate

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        db = context.db
        provider = db.query(Provider).filter(Provider.id == self.provider_id).first()
        if not provider:
            raise NotFoundException(f"Provider {self.provider_id} 不存在")

        if self.endpoint_data.provider_id != self.provider_id:
            raise InvalidRequestException("provider_id 不匹配")

        existing = (
            db.query(ProviderEndpoint)
            .filter(
                and_(
                    ProviderEndpoint.provider_id == self.provider_id,
                    ProviderEndpoint.api_format == self.endpoint_data.api_format,
                )
            )
            .first()
        )
        if existing:
            raise InvalidRequestException(
                f"Provider {provider.name} 已存在 {self.endpoint_data.api_format} 格式的 Endpoint"
            )

        now = datetime.now(timezone.utc)
        sig = parse_signature_key(self.endpoint_data.api_format)
        api_family = sig.api_family.value
        endpoint_kind = sig.endpoint_kind.value
        # 使用归一化后的 signature key，确保格式一致性
        normalized_api_format = sig.key

        new_endpoint = ProviderEndpoint(
            id=str(uuid.uuid4()),
            provider_id=self.provider_id,
            api_format=normalized_api_format,
            api_family=api_family,
            endpoint_kind=endpoint_kind,
            base_url=self.endpoint_data.base_url,
            custom_path=self.endpoint_data.custom_path,
            header_rules=self.endpoint_data.header_rules,
            max_retries=self.endpoint_data.max_retries,
            is_active=True,
            config=self.endpoint_data.config,
            proxy=self.endpoint_data.proxy.model_dump() if self.endpoint_data.proxy else None,
            format_acceptance_config=self.endpoint_data.format_acceptance_config,
            created_at=now,
            updated_at=now,
        )

        db.add(new_endpoint)
        db.commit()
        db.refresh(new_endpoint)

        # 清除 /v1/models 列表缓存
        await invalidate_models_list_cache()

        logger.info(
            f"[OK] 创建 Endpoint: Provider={provider.name}, Format={self.endpoint_data.api_format}, ID={new_endpoint.id}"
        )

        endpoint_dict = {
            k: v
            for k, v in new_endpoint.__dict__.items()
            if k not in {"api_format", "_sa_instance_state", "proxy"}
        }
        effective, inherited_from = _compute_endpoint_format_conversion(
            new_endpoint, provider, db
        )
        return ProviderEndpointResponse(
            **endpoint_dict,
            provider_name=provider.name,
            api_format=new_endpoint.api_format,
            proxy=mask_proxy_password(new_endpoint.proxy),
            total_keys=0,
            active_keys=0,
            format_conversion_effective=effective,
            format_conversion_inherited_from=inherited_from,
        )


@dataclass
class AdminGetProviderEndpointAdapter(AdminApiAdapter):
    endpoint_id: str

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        db = context.db
        endpoint = (
            db.query(ProviderEndpoint, Provider)
            .join(Provider, ProviderEndpoint.provider_id == Provider.id)
            .filter(ProviderEndpoint.id == self.endpoint_id)
            .first()
        )
        if not endpoint:
            raise NotFoundException(f"Endpoint {self.endpoint_id} 不存在")

        endpoint_obj, provider = endpoint
        endpoint_format = (
            endpoint_obj.api_format
            if isinstance(endpoint_obj.api_format, str)
            else endpoint_obj.api_format.value
        )
        keys = (
            db.query(ProviderAPIKey.api_formats, ProviderAPIKey.is_active)
            .filter(ProviderAPIKey.provider_id == endpoint_obj.provider_id)
            .all()
        )
        total_keys = 0
        active_keys = 0
        for api_formats, is_active in keys:
            if endpoint_format in (api_formats or []):
                total_keys += 1
                if is_active:
                    active_keys += 1

        endpoint_dict = {
            k: v
            for k, v in endpoint_obj.__dict__.items()
            if k not in {"api_format", "_sa_instance_state", "proxy"}
        }
        effective, inherited_from = _compute_endpoint_format_conversion(
            endpoint_obj, provider, db
        )
        return ProviderEndpointResponse(
            **endpoint_dict,
            provider_name=provider.name,
            api_format=endpoint_obj.api_format,
            proxy=mask_proxy_password(endpoint_obj.proxy),
            total_keys=total_keys,
            active_keys=active_keys,
            format_conversion_effective=effective,
            format_conversion_inherited_from=inherited_from,
        )


@dataclass
class AdminUpdateProviderEndpointAdapter(AdminApiAdapter):
    endpoint_id: str
    endpoint_data: ProviderEndpointUpdate

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        db = context.db
        endpoint = (
            db.query(ProviderEndpoint).filter(ProviderEndpoint.id == self.endpoint_id).first()
        )
        if not endpoint:
            raise NotFoundException(f"Endpoint {self.endpoint_id} 不存在")

        update_data = self.endpoint_data.model_dump(exclude_unset=True)

        # 把 proxy 转换为 dict 存储，支持显式设置为 None 清除代理
        if "proxy" in update_data:
            if update_data["proxy"] is not None:
                new_proxy = dict(update_data["proxy"])
                # 只有当密码字段未提供时才保留原密码（空字符串视为显式清除）
                if "password" not in new_proxy and endpoint.proxy:
                    old_password = endpoint.proxy.get("password")
                    if old_password:
                        new_proxy["password"] = old_password
                update_data["proxy"] = new_proxy
            # proxy 为 None 时保留，用于清除代理配置
        for field, value in update_data.items():
            setattr(endpoint, field, value)

        # Phase 3/4: 自动维护新架构字段，确保新增/历史数据都能被调度器按 family/kind 查询
        sig = parse_signature_key(endpoint.api_format)
        endpoint.api_family = sig.api_family.value
        endpoint.endpoint_kind = sig.endpoint_kind.value
        endpoint.updated_at = datetime.now(timezone.utc)

        db.commit()
        db.refresh(endpoint)

        # 清除 /v1/models 列表缓存（is_active 变更会影响模型可用性）
        await invalidate_models_list_cache()

        provider = db.query(Provider).filter(Provider.id == endpoint.provider_id).first()
        logger.info(
            f"[OK] 更新 Endpoint: ID={self.endpoint_id}, Updates={list(update_data.keys())}"
        )

        endpoint_format = (
            endpoint.api_format
            if isinstance(endpoint.api_format, str)
            else endpoint.api_format.value
        )
        keys = (
            db.query(ProviderAPIKey.api_formats, ProviderAPIKey.is_active)
            .filter(ProviderAPIKey.provider_id == endpoint.provider_id)
            .all()
        )
        total_keys = 0
        active_keys = 0
        for api_formats, is_active in keys:
            if endpoint_format in (api_formats or []):
                total_keys += 1
                if is_active:
                    active_keys += 1

        endpoint_dict = {
            k: v
            for k, v in endpoint.__dict__.items()
            if k not in {"api_format", "_sa_instance_state", "proxy"}
        }
        effective, inherited_from = _compute_endpoint_format_conversion(
            endpoint, provider, db
        ) if provider else (False, None)
        return ProviderEndpointResponse(
            **endpoint_dict,
            provider_name=provider.name if provider else "Unknown",
            api_format=endpoint.api_format,
            proxy=mask_proxy_password(endpoint.proxy),
            total_keys=total_keys,
            active_keys=active_keys,
            format_conversion_effective=effective,
            format_conversion_inherited_from=inherited_from,
        )


@dataclass
class AdminDeleteProviderEndpointAdapter(AdminApiAdapter):
    endpoint_id: str

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        db = context.db
        endpoint = (
            db.query(ProviderEndpoint).filter(ProviderEndpoint.id == self.endpoint_id).first()
        )
        if not endpoint:
            raise NotFoundException(f"Endpoint {self.endpoint_id} 不存在")

        endpoint_format = (
            endpoint.api_format
            if isinstance(endpoint.api_format, str)
            else endpoint.api_format.value
        )

        # 查询包含该格式的所有 Key，并从 api_formats 中移除该格式
        keys = (
            db.query(ProviderAPIKey)
            .filter(ProviderAPIKey.provider_id == endpoint.provider_id)
            .all()
        )
        affected_keys_count = 0
        for key in keys:
            if key.api_formats and endpoint_format in key.api_formats:
                affected_keys_count += 1
                # 移除该格式
                new_formats = [f for f in key.api_formats if f != endpoint_format]
                key.api_formats = new_formats if new_formats else []
                flag_modified(key, "api_formats")

        db.delete(endpoint)
        db.commit()

        # 清除 /v1/models 列表缓存
        await invalidate_models_list_cache()

        logger.warning(
            f"[DELETE] 删除 Endpoint: ID={self.endpoint_id}, Format={endpoint_format}, "
            f"AffectedKeys={affected_keys_count}"
        )

        return {
            "message": f"Endpoint {self.endpoint_id} 已删除",
            "affected_keys_count": affected_keys_count,
        }
