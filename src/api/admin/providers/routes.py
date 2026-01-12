"""管理员 Provider 管理路由。"""

from datetime import datetime, timezone
from typing import Optional

from fastapi import APIRouter, Depends, Query, Request
from pydantic import ValidationError
from sqlalchemy.orm import Session

from src.api.base.admin_adapter import AdminApiAdapter
from src.api.base.pipeline import ApiRequestPipeline
from src.core.enums import ProviderBillingType
from src.core.exceptions import InvalidRequestException, NotFoundException
from src.core.logger import logger
from src.database import get_db
from src.models.admin_requests import CreateProviderRequest, UpdateProviderRequest
from src.models.database import Provider
from src.services.cache.provider_cache import ProviderCacheService

router = APIRouter(tags=["Provider CRUD"])
pipeline = ApiRequestPipeline()


@router.get("/")
async def list_providers(
    request: Request,
    skip: int = Query(0, ge=0),
    limit: int = Query(100, ge=1, le=500),
    is_active: Optional[bool] = None,
    db: Session = Depends(get_db),
):
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
async def create_provider(request: Request, db: Session = Depends(get_db)):
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
    - `provider_priority`: 提供商优先级（数字越小优先级越高，默认 100）
    - `is_active`: 是否启用（默认 true）
    - `concurrent_limit`: 并发限制（可选）
    - `timeout`: 请求超时（秒，可选）
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


@router.put("/{provider_id}")
async def update_provider(provider_id: str, request: Request, db: Session = Depends(get_db)):
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
    - `timeout`: 请求超时（秒）
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


@router.delete("/{provider_id}")
async def delete_provider(provider_id: str, request: Request, db: Session = Depends(get_db)):
    """
    删除提供商

    删除指定的提供商。注意：此操作会级联删除关联的端点、密钥和模型配置。

    **路径参数**:
    - `provider_id`: 提供商 ID

    **返回字段**:
    - `message`: 删除成功提示信息
    """
    adapter = AdminDeleteProviderAdapter(provider_id=provider_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


class AdminListProvidersAdapter(AdminApiAdapter):
    def __init__(self, skip: int, limit: int, is_active: Optional[bool]):
        self.skip = skip
        self.limit = limit
        self.is_active = is_active

    async def handle(self, context):  # type: ignore[override]
        db = context.db
        query = db.query(Provider)
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
    async def handle(self, context):  # type: ignore[override]
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

            # 创建 Provider 对象
            provider = Provider(
                name=validated_data.name,
                description=validated_data.description,
                website=validated_data.website,
                billing_type=billing_type,
                monthly_quota_usd=validated_data.monthly_quota_usd,
                quota_reset_day=validated_data.quota_reset_day,
                quota_last_reset_at=validated_data.quota_last_reset_at,
                quota_expires_at=validated_data.quota_expires_at,
                provider_priority=validated_data.provider_priority,
                is_active=validated_data.is_active,
                concurrent_limit=validated_data.concurrent_limit,
                timeout=validated_data.timeout,
                max_retries=validated_data.max_retries,
                proxy=validated_data.proxy.model_dump() if validated_data.proxy else None,
                config=validated_data.config,
            )

            db.add(provider)
            db.commit()
            db.refresh(provider)

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

    async def handle(self, context):  # type: ignore[override]
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

            for field, value in update_data.items():
                if field == "billing_type" and value is not None:
                    # billing_type 需要转换为枚举
                    setattr(provider, field, ProviderBillingType(value))
                elif field == "proxy" and value is not None:
                    # proxy 需要转换为 dict（如果是 Pydantic 模型）
                    setattr(provider, field, value if isinstance(value, dict) else value.model_dump())
                else:
                    setattr(provider, field, value)

            provider.updated_at = datetime.now(timezone.utc)
            db.commit()
            db.refresh(provider)

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

            return {
                "id": provider.id,
                "name": provider.name,
                "is_active": provider.is_active,
                "message": "提供商更新成功",
            }
        except InvalidRequestException:
            db.rollback()
            raise
        except Exception:
            db.rollback()
            raise


class AdminDeleteProviderAdapter(AdminApiAdapter):
    def __init__(self, provider_id: str):
        self.provider_id = provider_id

    async def handle(self, context):  # type: ignore[override]
        db = context.db
        provider = db.query(Provider).filter(Provider.id == self.provider_id).first()
        if not provider:
            raise NotFoundException("提供商不存在", "provider")

        context.add_audit_metadata(
            action="delete_provider",
            provider_id=provider.id,
            provider_name=provider.name,
        )
        db.delete(provider)
        db.commit()
        return {"message": "提供商已删除"}
