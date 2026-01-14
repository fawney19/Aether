"""
模型自动同步 API 端点

提供手动触发模型同步的管理接口
"""

from dataclasses import dataclass
from typing import List, Optional

from fastapi import APIRouter, Depends, Request
from pydantic import BaseModel
from sqlalchemy.orm import Session

from src.api.base.admin_adapter import AdminApiAdapter
from src.api.base.pipeline import ApiRequestPipeline
from src.database import get_db
from src.services.model.auto_sync_service import ModelAutoSyncService

router = APIRouter(tags=["Model Auto Sync"])
pipeline = ApiRequestPipeline()


class SyncResult(BaseModel):
    """同步结果响应"""

    provider_id: Optional[str] = None
    providers_scanned: Optional[int] = None
    models_created: int
    errors: List[str]


@dataclass
class SyncProviderModelsAdapter(AdminApiAdapter):
    """同步单个 Provider 的模型"""

    provider_id: str

    async def handle(self, context):  # type: ignore[override]
        db = context.db

        result = ModelAutoSyncService.sync_models_for_provider(db, self.provider_id)

        return SyncResult(
            provider_id=result["provider_id"],
            models_created=result["models_created"],
            errors=result["errors"],
        )


@dataclass
class SyncAllModelsAdapter(AdminApiAdapter):
    """同步所有 Provider 的模型"""

    async def handle(self, context):  # type: ignore[override]
        db = context.db

        result = ModelAutoSyncService.sync_all_provider_models(db)

        return SyncResult(
            providers_scanned=result["providers_scanned"],
            models_created=result["models_created"],
            errors=result["errors"],
        )


@router.post("/{provider_id}/sync-models", response_model=SyncResult)
async def sync_provider_models(
    provider_id: str,
    request: Request,
    db: Session = Depends(get_db),
) -> SyncResult:
    """
    同步单个 Provider 的模型

    根据该 Provider 的所有 API Key 的 allowed_models 白名单,
    自动添加匹配的全局模型到该 Provider 的模型列表。

    **权限要求**: Admin

    **路径参数**:
    - `provider_id`: Provider ID

    **返回**:
    - `provider_id`: Provider ID
    - `models_created`: 新创建的模型数量
    - `errors`: 错误列表
    """
    adapter = SyncProviderModelsAdapter(provider_id=provider_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.post("/sync-all-models", response_model=SyncResult)
async def sync_all_models(
    request: Request,
    db: Session = Depends(get_db),
) -> SyncResult:
    """
    同步所有 Provider 的模型

    遍历所有活跃的 Provider,根据其 API Key 的 allowed_models 白名单,
    自动添加匹配的全局模型。

    **权限要求**: Admin

    **返回**:
    - `providers_scanned`: 扫描的 Provider 数量
    - `models_created`: 新创建的模型总数
    - `errors`: 错误列表
    """
    adapter = SyncAllModelsAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)
