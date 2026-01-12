"""
Key RPM 限制管理 API
"""

from dataclasses import dataclass

from fastapi import APIRouter, Depends, Request
from sqlalchemy.orm import Session

from src.api.base.admin_adapter import AdminApiAdapter
from src.api.base.pipeline import ApiRequestPipeline
from src.core.exceptions import NotFoundException
from src.database import get_db
from src.models.database import ProviderAPIKey
from src.models.endpoint_models import KeyRpmStatusResponse
from src.services.rate_limit.concurrency_manager import get_concurrency_manager

router = APIRouter(tags=["RPM Control"])
pipeline = ApiRequestPipeline()


@router.get("/rpm/key/{key_id}", response_model=KeyRpmStatusResponse)
async def get_key_rpm(
    key_id: str,
    request: Request,
    db: Session = Depends(get_db),
) -> KeyRpmStatusResponse:
    """
    获取 Key 当前 RPM 状态

    查询指定 API Key 的实时 RPM 使用情况，包括当前 RPM 计数和最大 RPM 限制。

    **路径参数**:
    - `key_id`: API Key ID

    **返回字段**:
    - `key_id`: API Key ID
    - `current_rpm`: 当前 RPM 计数
    - `rpm_limit`: RPM 限制
    """
    adapter = AdminKeyRpmAdapter(key_id=key_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.delete("/rpm/key/{key_id}")
async def reset_key_rpm(
    key_id: str,
    http_request: Request,
    db: Session = Depends(get_db),
) -> dict:
    """
    重置 Key RPM 计数器

    重置指定 API Key 的 RPM 计数器，用于解决计数不准确的问题。
    管理员功能，请谨慎使用。

    **路径参数**:
    - `key_id`: API Key ID

    **返回字段**:
    - `message`: 操作结果消息
    """
    adapter = AdminResetKeyRpmAdapter(key_id=key_id)
    return await pipeline.run(adapter=adapter, http_request=http_request, db=db, mode=adapter.mode)


# -------- Adapters --------


@dataclass
class AdminKeyRpmAdapter(AdminApiAdapter):
    key_id: str

    async def handle(self, context):  # type: ignore[override]
        db = context.db
        key = db.query(ProviderAPIKey).filter(ProviderAPIKey.id == self.key_id).first()
        if not key:
            raise NotFoundException(f"Key {self.key_id} 不存在")

        concurrency_manager = await get_concurrency_manager()
        key_count = await concurrency_manager.get_key_rpm_count(key_id=self.key_id)

        return KeyRpmStatusResponse(
            key_id=self.key_id,
            current_rpm=key_count,
            rpm_limit=key.rpm_limit,
        )


@dataclass
class AdminResetKeyRpmAdapter(AdminApiAdapter):
    key_id: str

    async def handle(self, context):  # type: ignore[override]
        concurrency_manager = await get_concurrency_manager()
        await concurrency_manager.reset_key_rpm(key_id=self.key_id)
        return {"message": "RPM 计数已重置"}
