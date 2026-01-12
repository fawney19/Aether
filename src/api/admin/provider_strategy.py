"""
提供商策略管理 API 端点
"""

from datetime import datetime, timedelta, timezone
from typing import Optional

from fastapi import APIRouter, Depends, HTTPException, Request
from fastapi.responses import JSONResponse
from pydantic import BaseModel, Field, ValidationError
from sqlalchemy.orm import Session

from src.api.base.admin_adapter import AdminApiAdapter
from src.api.base.pipeline import ApiRequestPipeline
from src.core.enums import ProviderBillingType
from src.core.exceptions import InvalidRequestException, translate_pydantic_error
from src.core.logger import logger
from src.database import get_db
from src.models.database import Provider
from src.models.database_extensions import ProviderUsageTracking

router = APIRouter(prefix="/api/admin/provider-strategy", tags=["Provider Strategy"])
pipeline = ApiRequestPipeline()


class ProviderBillingUpdate(BaseModel):
    billing_type: ProviderBillingType
    monthly_quota_usd: Optional[float] = None
    quota_reset_day: int = Field(default=30, ge=1, le=365)  # 重置周期（天数）
    quota_last_reset_at: Optional[str] = None  # 当前周期开始时间
    quota_expires_at: Optional[str] = None
    rpm_limit: Optional[int] = Field(default=None, ge=0)
    provider_priority: int = Field(default=100, ge=0, le=200)


@router.put("/providers/{provider_id}/billing")
async def update_provider_billing(
    provider_id: str,
    request: Request,
    db: Session = Depends(get_db),
):
    """
    更新提供商计费配置

    更新指定提供商的计费策略、配额设置和优先级配置。

    **路径参数**:
    - `provider_id`: 提供商 ID

    **请求体字段**:
    - `billing_type`: 计费类型（pay_as_you_go、subscription、prepaid、monthly_quota）
    - `monthly_quota_usd`: 月度配额（美元），可选
    - `quota_reset_day`: 配额重置周期（天数，1-365），默认 30
    - `quota_last_reset_at`: 当前周期开始时间，可选（设置后会自动同步该周期内的历史使用量）
    - `quota_expires_at`: 配额过期时间，可选
    - `rpm_limit`: 每分钟请求数限制，可选
    - `provider_priority`: 提供商优先级（0-200），默认 100

    **返回字段**:
    - `message`: 操作结果信息
    - `provider`: 更新后的提供商信息
      - `id`: 提供商 ID
      - `name`: 提供商名称
      - `billing_type`: 计费类型
      - `provider_priority`: 提供商优先级
    """
    adapter = AdminProviderBillingAdapter(provider_id=provider_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/providers/{provider_id}/stats")
async def get_provider_stats(
    provider_id: str,
    request: Request,
    hours: int = 24,
    db: Session = Depends(get_db),
):
    """
    获取提供商统计数据

    获取指定提供商的计费信息和使用统计数据。

    **路径参数**:
    - `provider_id`: 提供商 ID

    **查询参数**:
    - `hours`: 统计时间范围（小时），默认 24

    **返回字段**:
    - `provider_id`: 提供商 ID
    - `provider_name`: 提供商名称
    - `period_hours`: 统计时间范围
    - `billing_info`: 计费信息
      - `billing_type`: 计费类型
      - `monthly_quota_usd`: 月度配额
      - `monthly_used_usd`: 月度已使用
      - `quota_remaining_usd`: 剩余配额
      - `quota_expires_at`: 配额过期时间
    - `usage_stats`: 使用统计
      - `total_requests`: 总请求数
      - `successful_requests`: 成功请求数
      - `failed_requests`: 失败请求数
      - `success_rate`: 成功率
      - `avg_response_time_ms`: 平均响应时间（毫秒）
      - `total_cost_usd`: 总成本（美元）
    """
    adapter = AdminProviderStatsAdapter(provider_id=provider_id, hours=hours)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.delete("/providers/{provider_id}/quota")
async def reset_provider_quota(
    provider_id: str,
    request: Request,
    db: Session = Depends(get_db),
):
    """Reset provider quota usage to zero"""
    adapter = AdminProviderResetQuotaAdapter(provider_id=provider_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/strategies")
async def list_available_strategies(request: Request, db: Session = Depends(get_db)):
    """
    获取可用负载均衡策略列表

    列出系统中所有已注册的负载均衡策略插件。

    **返回字段**:
    - `strategies`: 策略列表
      - `name`: 策略名称
      - `priority`: 策略优先级
      - `version`: 策略版本
      - `description`: 策略描述
      - `author`: 策略作者
    - `total`: 策略总数
    """
    adapter = AdminListStrategiesAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


class AdminProviderBillingAdapter(AdminApiAdapter):
    def __init__(self, provider_id: str):
        self.provider_id = provider_id

    async def handle(self, context):
        db = context.db
        payload = context.ensure_json_body()
        try:
            config = ProviderBillingUpdate.model_validate(payload)
        except ValidationError as e:
            errors = e.errors()
            if errors:
                raise InvalidRequestException(translate_pydantic_error(errors[0]))
            raise InvalidRequestException("请求数据验证失败")

        provider = db.query(Provider).filter(Provider.id == self.provider_id).first()
        if not provider:
            raise HTTPException(status_code=404, detail="Provider not found")

        provider.billing_type = config.billing_type
        provider.monthly_quota_usd = config.monthly_quota_usd
        provider.quota_reset_day = config.quota_reset_day
        provider.provider_priority = config.provider_priority

        from dateutil import parser
        from sqlalchemy import func

        from src.models.database import Usage

        if config.quota_last_reset_at:
            new_reset_at = parser.parse(config.quota_last_reset_at)
            # 确保有时区信息，如果没有则假设为 UTC
            if new_reset_at.tzinfo is None:
                new_reset_at = new_reset_at.replace(tzinfo=timezone.utc)
            provider.quota_last_reset_at = new_reset_at

            # 自动同步该周期内的历史使用量
            period_usage = (
                db.query(func.coalesce(func.sum(Usage.total_cost_usd), 0))
                .filter(
                    Usage.provider_id == self.provider_id,
                    Usage.created_at >= new_reset_at,
                )
                .scalar()
            )
            provider.monthly_used_usd = float(period_usage or 0)
            logger.info(f"Synced usage for provider {provider.name}: ${period_usage:.4f} since {new_reset_at}")

        if config.quota_expires_at:
            expires_at = parser.parse(config.quota_expires_at)
            # 确保有时区信息，如果没有则假设为 UTC
            if expires_at.tzinfo is None:
                expires_at = expires_at.replace(tzinfo=timezone.utc)
            provider.quota_expires_at = expires_at

        db.commit()
        db.refresh(provider)

        logger.info(f"Updated billing config for provider {provider.name}")

        return JSONResponse(
            {
                "message": "Provider billing config updated successfully",
                "provider": {
                    "id": provider.id,
                    "name": provider.name,
                    "billing_type": provider.billing_type.value,
                    "provider_priority": provider.provider_priority,
                },
            }
        )


class AdminProviderStatsAdapter(AdminApiAdapter):
    def __init__(self, provider_id: str, hours: int):
        self.provider_id = provider_id
        self.hours = hours

    async def handle(self, context):
        db = context.db
        provider = db.query(Provider).filter(Provider.id == self.provider_id).first()
        if not provider:
            raise HTTPException(status_code=404, detail="Provider not found")

        since = datetime.now(timezone.utc) - timedelta(hours=self.hours)
        stats = (
            db.query(ProviderUsageTracking)
            .filter(
                ProviderUsageTracking.provider_id == self.provider_id,
                ProviderUsageTracking.window_start >= since,
            )
            .all()
        )

        total_requests = sum(s.total_requests for s in stats)
        total_success = sum(s.successful_requests for s in stats)
        total_failures = sum(s.failed_requests for s in stats)
        avg_response_time = sum(s.avg_response_time_ms for s in stats) / len(stats) if stats else 0
        total_cost = sum(s.total_cost_usd for s in stats)

        return JSONResponse(
            {
                "provider_id": self.provider_id,
                "provider_name": provider.name,
                "period_hours": self.hours,
                "billing_info": {
                    "billing_type": provider.billing_type.value,
                    "monthly_quota_usd": provider.monthly_quota_usd,
                    "monthly_used_usd": provider.monthly_used_usd,
                    "quota_remaining_usd": (
                        provider.monthly_quota_usd - provider.monthly_used_usd
                        if provider.monthly_quota_usd is not None
                        else None
                    ),
                    "quota_expires_at": (
                        provider.quota_expires_at.isoformat() if provider.quota_expires_at else None
                    ),
                },
                "usage_stats": {
                    "total_requests": total_requests,
                    "successful_requests": total_success,
                    "failed_requests": total_failures,
                    "success_rate": total_success / total_requests if total_requests > 0 else 0,
                    "avg_response_time_ms": round(avg_response_time, 2),
                    "total_cost_usd": round(total_cost, 4),
                },
            }
        )


class AdminProviderResetQuotaAdapter(AdminApiAdapter):
    def __init__(self, provider_id: str):
        self.provider_id = provider_id

    async def handle(self, context):
        db = context.db
        provider = db.query(Provider).filter(Provider.id == self.provider_id).first()
        if not provider:
            raise HTTPException(status_code=404, detail="Provider not found")

        if provider.billing_type != ProviderBillingType.MONTHLY_QUOTA:
            raise HTTPException(status_code=400, detail="Only monthly quota providers can be reset")

        old_used = provider.monthly_used_usd
        provider.monthly_used_usd = 0.0
        db.commit()

        logger.info(f"Manually reset quota for provider {provider.name}")

        return JSONResponse(
            {
                "message": "Provider quota reset successfully",
                "provider_name": provider.name,
                "previous_used": old_used,
                "current_used": 0.0,
            }
        )


class AdminListStrategiesAdapter(AdminApiAdapter):
    async def handle(self, context):
        from src.plugins.manager import get_plugin_manager

        plugin_manager = get_plugin_manager()
        lb_plugins = plugin_manager.plugins.get("load_balancer", {})

        strategies = []
        for name, plugin in lb_plugins.items():
            try:
                strategies.append(
                    {
                        "name": getattr(plugin, "name", name),
                        "priority": getattr(plugin, "priority", 0),
                        "version": (
                            getattr(plugin.metadata, "version", "1.0.0")
                            if hasattr(plugin, "metadata")
                            else "1.0.0"
                        ),
                        "description": (
                            getattr(plugin.metadata, "description", "")
                            if hasattr(plugin, "metadata")
                            else ""
                        ),
                        "author": (
                            getattr(plugin.metadata, "author", "Unknown")
                            if hasattr(plugin, "metadata")
                            else "Unknown"
                        ),
                    }
                )
            except Exception as exc:  # pragma: no cover
                logger.error(f"Error accessing plugin {name}: {exc}")
                continue

        strategies.sort(key=lambda x: x["priority"], reverse=True)
        return JSONResponse({"strategies": strategies, "total": len(strategies)})
