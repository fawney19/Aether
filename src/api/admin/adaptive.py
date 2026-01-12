"""
自适应 RPM 管理 API 端点

设计原则：
- 自适应模式由 rpm_limit 字段决定：
  - rpm_limit = NULL：启用自适应模式，系统自动学习并调整 RPM 限制
  - rpm_limit = 数字：固定限制模式，使用用户指定的 RPM 限制
- learned_rpm_limit：自适应模式下学习到的 RPM 限制值
- adaptive_mode 是计算字段，基于 rpm_limit 是否为 NULL
"""

from dataclasses import dataclass
from typing import List, Optional

from fastapi import APIRouter, Depends, HTTPException, Query, Request
from pydantic import BaseModel, Field, ValidationError
from sqlalchemy.orm import Session

from src.api.base.admin_adapter import AdminApiAdapter
from src.api.base.pipeline import ApiRequestPipeline
from src.config.constants import RPMDefaults
from src.core.exceptions import InvalidRequestException, translate_pydantic_error
from src.database import get_db
from src.models.database import ProviderAPIKey
from src.services.rate_limit.adaptive_rpm import get_adaptive_rpm_manager

router = APIRouter(prefix="/api/admin/adaptive", tags=["Adaptive RPM"])
pipeline = ApiRequestPipeline()


# ==================== Pydantic Models ====================


class EnableAdaptiveRequest(BaseModel):
    """启用自适应模式请求"""

    enabled: bool = Field(..., description="是否启用自适应模式（true=自适应，false=固定限制）")
    fixed_limit: Optional[int] = Field(
        None, ge=1, le=100, description="固定 RPM 限制（仅当 enabled=false 时生效，1-100）"
    )


class AdaptiveStatsResponse(BaseModel):
    """自适应统计响应"""

    adaptive_mode: bool = Field(..., description="是否为自适应模式（rpm_limit=NULL）")
    rpm_limit: Optional[int] = Field(None, description="用户配置的固定限制（NULL=自适应）")
    effective_limit: Optional[int] = Field(
        None, description="当前有效限制（自适应使用学习值，固定使用配置值）"
    )
    learned_limit: Optional[int] = Field(None, description="学习到的 RPM 限制")
    concurrent_429_count: int
    rpm_429_count: int
    last_429_at: Optional[str]
    last_429_type: Optional[str]
    adjustment_count: int
    recent_adjustments: List[dict]


class KeyListItem(BaseModel):
    """Key 列表项"""

    id: str
    name: Optional[str]
    provider_id: str
    api_formats: List[str] = Field(default_factory=list)
    is_adaptive: bool = Field(..., description="是否为自适应模式（rpm_limit=NULL）")
    rpm_limit: Optional[int] = Field(None, description="固定 RPM 限制（NULL=自适应）")
    effective_limit: Optional[int] = Field(None, description="当前有效限制")
    learned_rpm_limit: Optional[int] = Field(None, description="学习到的 RPM 限制")
    concurrent_429_count: int
    rpm_429_count: int


# ==================== API Endpoints ====================


@router.get(
    "/keys",
    response_model=List[KeyListItem],
    summary="获取所有启用自适应模式的Key",
)
async def list_adaptive_keys(
    request: Request,
    provider_id: Optional[str] = Query(None, description="按 Provider 过滤"),
    db: Session = Depends(get_db),
):
    """
    获取所有启用自适应模式的Key列表

    可选参数：
    - provider_id: 按 Provider 过滤
    """
    adapter = ListAdaptiveKeysAdapter(provider_id=provider_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.patch(
    "/keys/{key_id}/mode",
    summary="Toggle key's RPM control mode",
)
async def toggle_adaptive_mode(
    key_id: str,
    request: Request,
    db: Session = Depends(get_db),
):
    """
    Toggle the RPM control mode for a specific key

    Parameters:
    - enabled: true=adaptive mode (rpm_limit=NULL), false=fixed limit mode
    - fixed_limit: fixed limit value (required when enabled=false)
    """
    adapter = ToggleAdaptiveModeAdapter(key_id=key_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get(
    "/keys/{key_id}/stats",
    response_model=AdaptiveStatsResponse,
    summary="获取Key的自适应统计",
)
async def get_adaptive_stats(
    key_id: str,
    request: Request,
    db: Session = Depends(get_db),
):
    """
    获取指定Key的自适应 RPM 统计信息

    包括：
    - 当前配置
    - 学习到的限制
    - 429错误统计
    - 调整历史
    """
    adapter = GetAdaptiveStatsAdapter(key_id=key_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.delete(
    "/keys/{key_id}/learning",
    summary="Reset key's learning state",
)
async def reset_adaptive_learning(
    key_id: str,
    request: Request,
    db: Session = Depends(get_db),
):
    """
    Reset the adaptive learning state for a specific key

    Clears:
    - Learned RPM limit (learned_rpm_limit)
    - 429 error counts
    - Adjustment history

    Does not change:
    - rpm_limit config (determines adaptive mode)
    """
    adapter = ResetAdaptiveLearningAdapter(key_id=key_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.patch(
    "/keys/{key_id}/limit",
    summary="Set key to fixed RPM limit mode",
)
async def set_rpm_limit(
    key_id: str,
    request: Request,
    limit: int = Query(..., ge=1, le=100, description="RPM limit value (1-100)"),
    db: Session = Depends(get_db),
):
    """
    Set key to fixed RPM limit mode

    Note:
    - After setting this value, key switches to fixed limit mode and won't auto-adjust
    - To restore adaptive mode, use PATCH /keys/{key_id}/mode
    """
    adapter = SetRPMLimitAdapter(key_id=key_id, limit=limit)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get(
    "/summary",
    summary="获取自适应 RPM 的全局统计",
)
async def get_adaptive_summary(
    request: Request,
    db: Session = Depends(get_db),
):
    """
    获取自适应 RPM 的全局统计摘要

    包括：
    - 启用自适应模式的Key数量
    - 总429错误数
    - RPM 限制调整次数
    """
    adapter = AdaptiveSummaryAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


# ==================== Pipeline 适配器 ====================


@dataclass
class ListAdaptiveKeysAdapter(AdminApiAdapter):
    provider_id: Optional[str] = None

    async def handle(self, context):  # type: ignore[override]
        # 自适应模式：rpm_limit = NULL
        query = context.db.query(ProviderAPIKey).filter(ProviderAPIKey.rpm_limit.is_(None))
        if self.provider_id:
            query = query.filter(ProviderAPIKey.provider_id == self.provider_id)

        keys = query.all()
        return [
            KeyListItem(
                id=key.id,
                name=key.name,
                provider_id=key.provider_id,
                api_formats=key.api_formats or [],
                is_adaptive=key.rpm_limit is None,
                rpm_limit=key.rpm_limit,
                effective_limit=(
                    (key.learned_rpm_limit if key.learned_rpm_limit is not None else RPMDefaults.INITIAL_LIMIT)
                    if key.rpm_limit is None
                    else key.rpm_limit
                ),
                learned_rpm_limit=key.learned_rpm_limit,
                concurrent_429_count=key.concurrent_429_count or 0,
                rpm_429_count=key.rpm_429_count or 0,
            )
            for key in keys
        ]


@dataclass
class ToggleAdaptiveModeAdapter(AdminApiAdapter):
    key_id: str

    async def handle(self, context):  # type: ignore[override]
        key = context.db.query(ProviderAPIKey).filter(ProviderAPIKey.id == self.key_id).first()
        if not key:
            raise HTTPException(status_code=404, detail="Key not found")

        payload = context.ensure_json_body()
        try:
            body = EnableAdaptiveRequest.model_validate(payload)
        except ValidationError as e:
            errors = e.errors()
            if errors:
                raise InvalidRequestException(translate_pydantic_error(errors[0]))
            raise InvalidRequestException("请求数据验证失败")

        if body.enabled:
            # 启用自适应模式：将 rpm_limit 设为 NULL
            key.rpm_limit = None
            message = "已切换为自适应模式，系统将自动学习并调整 RPM 限制"
        else:
            # 禁用自适应模式：设置固定限制
            if body.fixed_limit is None:
                raise HTTPException(
                    status_code=400, detail="禁用自适应模式时必须提供 fixed_limit 参数"
                )
            key.rpm_limit = body.fixed_limit
            message = f"已切换为固定限制模式，RPM 限制设为 {body.fixed_limit}"

        context.db.commit()
        context.db.refresh(key)

        is_adaptive = key.rpm_limit is None
        return {
            "message": message,
            "key_id": key.id,
            "is_adaptive": is_adaptive,
            "rpm_limit": key.rpm_limit,
            "effective_limit": (
                (key.learned_rpm_limit if key.learned_rpm_limit is not None else RPMDefaults.INITIAL_LIMIT)
                if is_adaptive
                else key.rpm_limit
            ),
        }


@dataclass
class GetAdaptiveStatsAdapter(AdminApiAdapter):
    key_id: str

    async def handle(self, context):  # type: ignore[override]
        key = context.db.query(ProviderAPIKey).filter(ProviderAPIKey.id == self.key_id).first()
        if not key:
            raise HTTPException(status_code=404, detail="Key not found")

        adaptive_manager = get_adaptive_rpm_manager()
        stats = adaptive_manager.get_adjustment_stats(key)

        # 转换字段名以匹配响应模型
        return AdaptiveStatsResponse(
            adaptive_mode=stats["adaptive_mode"],
            rpm_limit=stats["rpm_limit"],
            effective_limit=stats["effective_limit"],
            learned_limit=stats["learned_limit"],
            concurrent_429_count=stats["concurrent_429_count"],
            rpm_429_count=stats["rpm_429_count"],
            last_429_at=stats["last_429_at"],
            last_429_type=stats["last_429_type"],
            adjustment_count=stats["adjustment_count"],
            recent_adjustments=stats["recent_adjustments"],
        )


@dataclass
class ResetAdaptiveLearningAdapter(AdminApiAdapter):
    key_id: str

    async def handle(self, context):  # type: ignore[override]
        key = context.db.query(ProviderAPIKey).filter(ProviderAPIKey.id == self.key_id).first()
        if not key:
            raise HTTPException(status_code=404, detail="Key not found")

        adaptive_manager = get_adaptive_rpm_manager()
        adaptive_manager.reset_learning(context.db, key)
        return {"message": "学习状态已重置", "key_id": key.id}


@dataclass
class SetRPMLimitAdapter(AdminApiAdapter):
    key_id: str
    limit: int

    async def handle(self, context):  # type: ignore[override]
        key = context.db.query(ProviderAPIKey).filter(ProviderAPIKey.id == self.key_id).first()
        if not key:
            raise HTTPException(status_code=404, detail="Key not found")

        was_adaptive = key.rpm_limit is None
        key.rpm_limit = self.limit
        context.db.commit()
        context.db.refresh(key)

        return {
            "message": f"已设置为固定限制模式，RPM 限制为 {self.limit}",
            "key_id": key.id,
            "is_adaptive": False,
            "rpm_limit": key.rpm_limit,
            "previous_mode": "adaptive" if was_adaptive else "fixed",
        }


class AdaptiveSummaryAdapter(AdminApiAdapter):
    async def handle(self, context):  # type: ignore[override]
        # 自适应模式：rpm_limit = NULL
        adaptive_keys = (
            context.db.query(ProviderAPIKey).filter(ProviderAPIKey.rpm_limit.is_(None)).all()
        )

        total_keys = len(adaptive_keys)
        total_concurrent_429 = sum(key.concurrent_429_count or 0 for key in adaptive_keys)
        total_rpm_429 = sum(key.rpm_429_count or 0 for key in adaptive_keys)
        total_adjustments = sum(len(key.adjustment_history or []) for key in adaptive_keys)

        recent_adjustments = []
        for key in adaptive_keys:
            if key.adjustment_history:
                for adj in key.adjustment_history[-3:]:
                    recent_adjustments.append(
                        {
                            "key_id": key.id,
                            "key_name": key.name,
                            **adj,
                        }
                    )

        recent_adjustments.sort(key=lambda item: item.get("timestamp", ""), reverse=True)

        return {
            "total_adaptive_keys": total_keys,
            "total_concurrent_429_errors": total_concurrent_429,
            "total_rpm_429_errors": total_rpm_429,
            "total_adjustments": total_adjustments,
            "recent_adjustments": recent_adjustments[:10],
        }
