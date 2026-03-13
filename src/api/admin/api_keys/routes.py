"""管理员独立余额 API Key 管理路由。

独立余额Key：不关联用户配额，可配置独立余额限制或无限额度，用于给非注册用户使用。
"""

from __future__ import annotations

import os
from datetime import datetime, timedelta, timezone
from typing import Any, Literal
from zoneinfo import ZoneInfo

from fastapi import APIRouter, Depends, HTTPException, Query, Request
from fastapi.concurrency import run_in_threadpool
from sqlalchemy import func
from sqlalchemy.orm import Session

from src.api.base.admin_adapter import AdminApiAdapter
from src.api.base.context import ApiRequestContext
from src.api.base.pipeline import ApiRequestPipeline
from src.core.exceptions import InvalidRequestException, NotFoundException
from src.core.logger import logger
from src.database import get_db, get_db_context
from src.models.api import CreateApiKeyRequest
from src.models.database import ApiKey, Usage, Wallet
from src.services.user.apikey import ApiKeyService
from src.services.user.bulk_cleanup import pre_clean_api_key
from src.services.wallet import WalletService

# 应用时区配置，默认为 Asia/Shanghai
APP_TIMEZONE = ZoneInfo(os.getenv("APP_TIMEZONE", "Asia/Shanghai"))


def parse_expiry_date(date_str: str | None) -> datetime | None:
    """解析过期日期字符串为 datetime 对象。

    Args:
        date_str: 日期字符串，支持 "YYYY-MM-DD" 或 ISO 格式

    Returns:
        datetime 对象（当天 23:59:59.999999，应用时区），或 None 如果输入为空

    Raises:
        BadRequestException: 日期格式无效
    """
    if not date_str or not date_str.strip():
        return None

    date_str = date_str.strip()

    # 尝试 YYYY-MM-DD 格式
    try:
        parsed_date = datetime.strptime(date_str, "%Y-%m-%d")
        # 设置为当天结束时间 (23:59:59.999999，应用时区)
        return parsed_date.replace(
            hour=23, minute=59, second=59, microsecond=999999, tzinfo=APP_TIMEZONE
        )
    except ValueError:
        pass

    # 尝试完整 ISO 格式
    try:
        return datetime.fromisoformat(date_str.replace("Z", "+00:00"))
    except ValueError:
        pass

    raise InvalidRequestException(f"无效的日期格式: {date_str}，请使用 YYYY-MM-DD 格式")


router = APIRouter(prefix="/api/admin/api-keys", tags=["Admin - API Keys (Standalone)"])
pipeline = ApiRequestPipeline()


def _serialize_standalone_key_item(api_key: ApiKey, *, total_tokens: int | None = None) -> dict[str, Any]:
    return {
        "id": api_key.id,
        "user_id": api_key.user_id,
        "name": api_key.name,
        "key_display": api_key.get_display_key(),
        "is_active": api_key.is_active,
        "is_standalone": api_key.is_standalone,
        "total_requests": api_key.total_requests,
        "total_tokens": int(total_tokens or 0),
        "total_cost_usd": float(api_key.total_cost_usd or 0),
        "rate_limit": api_key.rate_limit,
        "allowed_providers": api_key.allowed_providers,
        "allowed_api_formats": api_key.allowed_api_formats,
        "allowed_models": api_key.allowed_models,
        "last_used_at": api_key.last_used_at.isoformat() if api_key.last_used_at else None,
        "expires_at": api_key.expires_at.isoformat() if api_key.expires_at else None,
        "created_at": api_key.created_at.isoformat(),
        "updated_at": api_key.updated_at.isoformat() if api_key.updated_at else None,
        "auto_delete_on_expiry": api_key.auto_delete_on_expiry,
    }


def _list_standalone_api_keys_sync(
    skip: int,
    limit: int,
    is_active: bool | None,
) -> dict[str, Any]:
    with get_db_context() as db:
        query = db.query(ApiKey).filter(ApiKey.is_standalone == True)
        if is_active is not None:
            query = query.filter(ApiKey.is_active == is_active)

        total = int(query.with_entities(func.count(ApiKey.id)).scalar() or 0)
        api_keys = query.order_by(ApiKey.created_at.desc()).offset(skip).limit(limit).all()

        wallet_initialized = False
        for api_key in api_keys:
            wallet = WalletService.get_wallet(db, api_key_id=api_key.id)
            if wallet is None:
                _ensure_standalone_wallet(db, api_key)
                wallet_initialized = True
        if wallet_initialized:
            db.commit()
            for api_key in api_keys:
                db.refresh(api_key)

        token_map: dict[str, int] = {}
        if api_keys:
            stats_rows = (
                db.query(
                    Usage.api_key_id,
                    func.sum(Usage.total_tokens).label("total_tokens"),
                )
                .filter(Usage.api_key_id.in_([api_key.id for api_key in api_keys]))
                .group_by(Usage.api_key_id)
                .all()
            )
            token_map = {
                row.api_key_id: int(row.total_tokens or 0)
                for row in stats_rows
            }

        return {
            "api_keys": [
                _serialize_standalone_key_item(
                    api_key,
                    total_tokens=token_map.get(api_key.id, 0),
                )
                for api_key in api_keys
            ],
            "total": total,
            "limit": limit,
            "skip": skip,
        }


def _create_standalone_api_key_sync(
    admin_user_id: str,
    key_data: CreateApiKeyRequest,
) -> tuple[dict[str, Any], dict[str, Any]]:
    with get_db_context() as db:
        if key_data.initial_balance_usd is not None and key_data.initial_balance_usd <= 0:
            raise HTTPException(
                status_code=400,
                detail="创建独立余额Key时，初始余额必须大于 0（或设置为 null 表示无限制）",
            )

        expires_at_dt = parse_expiry_date(key_data.expires_at)
        api_key, plain_key = ApiKeyService.create_api_key(
            db=db,
            user_id=admin_user_id,
            name=key_data.name,
            allowed_providers=key_data.allowed_providers,
            allowed_api_formats=key_data.allowed_api_formats,
            allowed_models=key_data.allowed_models,
            rate_limit=key_data.rate_limit,
            expire_days=key_data.expire_days,
            expires_at=expires_at_dt,
            is_standalone=True,
            auto_delete_on_expiry=key_data.auto_delete_on_expiry,
        )

        wallet = WalletService.initialize_api_key_wallet(
            db,
            api_key=api_key,
            initial_balance_usd=key_data.initial_balance_usd,
            unlimited=key_data.initial_balance_usd is None,
            operator_id=admin_user_id,
            description="独立密钥初始调账",
        )
        if wallet is None:
            raise InvalidRequestException("独立密钥钱包初始化失败")

        db.commit()
        db.refresh(api_key)
        wallet_summary = WalletService.serialize_wallet_summary(wallet)
        return (
            {
                "id": api_key.id,
                "key": plain_key,
                "name": api_key.name,
                "key_display": api_key.get_display_key(),
                "is_standalone": True,
                "rate_limit": api_key.rate_limit,
                "expires_at": api_key.expires_at.isoformat() if api_key.expires_at else None,
                "created_at": api_key.created_at.isoformat(),
                "wallet": wallet_summary,
                "message": "独立余额Key创建成功，请妥善保存完整密钥，后续将无法查看",
            },
            {
                "action": "create_standalone_api_key",
                "key_id": api_key.id,
                "initial_balance_usd": key_data.initial_balance_usd,
            },
        )


def _update_standalone_api_key_sync(
    key_id: str,
    key_data: CreateApiKeyRequest,
) -> tuple[dict[str, Any], list[str]]:
    with get_db_context() as db:
        api_key = db.query(ApiKey).filter(ApiKey.id == key_id).first()
        if not api_key:
            raise NotFoundException("API密钥不存在", "api_key")
        if not api_key.is_standalone:
            raise InvalidRequestException("仅支持更新独立密钥")

        update_data: dict[str, Any] = {}
        if key_data.name is not None:
            update_data["name"] = key_data.name
        if "rate_limit" in key_data.model_fields_set:
            update_data["rate_limit"] = key_data.rate_limit
        if (
            hasattr(key_data, "auto_delete_on_expiry")
            and key_data.auto_delete_on_expiry is not None
        ):
            update_data["auto_delete_on_expiry"] = key_data.auto_delete_on_expiry
        if hasattr(key_data, "allowed_providers"):
            update_data["allowed_providers"] = key_data.allowed_providers
        if hasattr(key_data, "allowed_api_formats"):
            update_data["allowed_api_formats"] = key_data.allowed_api_formats
        if hasattr(key_data, "allowed_models"):
            update_data["allowed_models"] = key_data.allowed_models

        if key_data.expires_at and key_data.expires_at.strip():
            update_data["expires_at"] = parse_expiry_date(key_data.expires_at)
        elif "expires_at" in key_data.model_fields_set:
            update_data["expires_at"] = None
        elif "expire_days" in key_data.model_fields_set:
            if key_data.expire_days is not None and key_data.expire_days > 0:
                update_data["expires_at"] = datetime.now(timezone.utc) + timedelta(
                    days=key_data.expire_days
                )
            else:
                update_data["expires_at"] = None

        changed_fields = list(update_data.keys())

        if "initial_balance_usd" in key_data.model_fields_set:
            raise InvalidRequestException("编辑独立密钥不支持修改余额，请使用钱包操作")

        if (
            "unlimited_balance" in key_data.model_fields_set
            and key_data.unlimited_balance is not None
        ):
            wallet = _ensure_standalone_wallet(db, api_key)
            desired_mode: Literal["finite", "unlimited"] = (
                "unlimited" if key_data.unlimited_balance else "finite"
            )
            if wallet.limit_mode != desired_mode:
                WalletService.set_wallet_limit_mode(db, wallet=wallet, limit_mode=desired_mode)
            changed_fields.append("unlimited_balance")

        updated_key = ApiKeyService.update_api_key(db, key_id, **update_data)
        if not updated_key:
            raise NotFoundException("更新失败", "api_key")

        wallet = _ensure_standalone_wallet(db, updated_key)
        wallet_summary = WalletService.serialize_wallet_summary(wallet)
        return (
            {
                "id": updated_key.id,
                "name": updated_key.name,
                "key_display": updated_key.get_display_key(),
                "is_active": updated_key.is_active,
                "rate_limit": updated_key.rate_limit,
                "expires_at": (
                    updated_key.expires_at.isoformat() if updated_key.expires_at else None
                ),
                "updated_at": (
                    updated_key.updated_at.isoformat() if updated_key.updated_at else None
                ),
                "wallet": wallet_summary,
                "message": "API密钥已更新",
            },
            changed_fields,
        )


def _toggle_standalone_api_key_sync(key_id: str) -> tuple[dict[str, Any], dict[str, Any]]:
    with get_db_context() as db:
        api_key = db.query(ApiKey).filter(ApiKey.id == key_id).first()
        if not api_key:
            raise NotFoundException("API密钥不存在", "api_key")
        if not api_key.is_standalone:
            raise InvalidRequestException("仅支持操作独立密钥")

        api_key.is_active = not api_key.is_active
        api_key.updated_at = datetime.now(timezone.utc)
        db.commit()
        db.refresh(api_key)
        return (
            {
                "id": api_key.id,
                "is_active": api_key.is_active,
                "message": f"API密钥已{'启用' if api_key.is_active else '禁用'}",
            },
            {
                "action": "toggle_api_key",
                "target_key_id": api_key.id,
                "user_id": api_key.user_id,
                "new_status": "enabled" if api_key.is_active else "disabled",
            },
        )


def _delete_standalone_api_key_sync(
    key_id: str,
) -> tuple[dict[str, str], dict[str, Any], str | None]:
    with get_db_context() as db:
        api_key = db.query(ApiKey).filter(ApiKey.id == key_id).first()
        if not api_key:
            raise HTTPException(status_code=404, detail="API密钥不存在")
        if not api_key.is_standalone:
            raise InvalidRequestException("仅支持删除独立密钥")

        user = api_key.user
        pre_clean_api_key(db, api_key.id)
        db.delete(api_key)
        return (
            {"message": "API密钥已删除"},
            {
                "action": "delete_api_key",
                "target_key_id": key_id,
                "user_id": user.id if user else None,
                "user_email": user.email if user else None,
            },
            user.email if user else None,
        )


def _ensure_standalone_wallet(
    db: Session,
    api_key: ApiKey,
    *,
    limit_mode: Literal["finite", "unlimited"] | None = None,
) -> Wallet:
    """确保独立 Key 已绑定钱包，并可选同步额度模式。"""
    wallet = WalletService.get_or_create_wallet(db, api_key=api_key)
    if wallet is None:
        raise InvalidRequestException("独立密钥钱包初始化失败")

    if limit_mode is not None and wallet.limit_mode != limit_mode:
        wallet = WalletService.set_wallet_limit_mode(db, wallet=wallet, limit_mode=limit_mode)

    return wallet


@router.get("")
async def list_standalone_api_keys(
    request: Request,
    skip: int = Query(0, ge=0),
    limit: int = Query(100, ge=1, le=500),
    is_active: bool | None = None,
    db: Session = Depends(get_db),
) -> Any:
    """
    列出所有独立余额 API Keys

    获取系统中所有独立余额 API Key 的列表。独立余额 Key 不关联用户配额，
    有独立的余额限制，主要用于给非注册用户使用。

    **查询参数**:
    - `skip`: 跳过的记录数（分页偏移量），默认 0
    - `limit`: 返回的记录数（分页限制），默认 100，最大 500
    - `is_active`: 可选，根据启用状态筛选（true/false）

    **返回字段**:
    - `api_keys`: API Key 列表，包含 id, name, key_display, is_active, is_standalone,
      total_requests, total_cost_usd, rate_limit, allowed_providers, allowed_api_formats,
      allowed_models, last_used_at, expires_at, created_at, updated_at, auto_delete_on_expiry 等字段
    - `total`: 符合条件的总记录数
    - `limit`: 当前分页限制
    - `skip`: 当前分页偏移量
    """
    adapter = AdminListStandaloneKeysAdapter(skip=skip, limit=limit, is_active=is_active)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.post("")
async def create_standalone_api_key(
    request: Request,
    key_data: CreateApiKeyRequest,
    db: Session = Depends(get_db),
) -> Any:
    """
    创建独立余额 API Key

    创建一个新的独立余额 API Key。独立余额 Key 可设置初始余额，或使用无限额度。

    **请求体字段**:
    - `name`: API Key 的名称
    - `initial_balance_usd`: 可选，初始余额（美元），null 表示无限制额度
    - `allowed_providers`: 可选，允许使用的提供商列表
    - `allowed_api_formats`: 可选，允许使用的 API 格式列表
    - `allowed_models`: 可选，允许使用的模型列表
    - `rate_limit`: 可选，每分钟请求限制（null 表示跟随系统默认，0 表示不限制）
    - `expire_days`: 可选，过期天数（与 expires_at 二选一）
    - `expires_at`: 可选，过期时间（ISO 格式或 YYYY-MM-DD 格式，优先级高于 expire_days）
    - `auto_delete_on_expiry`: 可选，过期后是否自动删除

    **返回字段**:
    - `id`: API Key ID
    - `key`: 完整的 API Key（仅在创建时返回一次）
    - `name`: API Key 名称
    - `key_display`: 脱敏显示的 Key
    - `is_standalone`: 是否为独立余额 Key（始终为 true）
    - `wallet`: 钱包摘要（总余额、充值余额、赠款余额、额度模式等）
    - `rate_limit`: 速率限制配置
    - `expires_at`: 过期时间
    - `created_at`: 创建时间
    - `message`: 提示信息
    """
    adapter = AdminCreateStandaloneKeyAdapter(key_data=key_data)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.put("/{key_id}")
async def update_api_key(
    key_id: str, request: Request, key_data: CreateApiKeyRequest, db: Session = Depends(get_db)
) -> Any:
    """
    更新独立余额 API Key

    更新指定 ID 的独立余额 API Key 的配置信息。

    **路径参数**:
    - `key_id`: API Key ID

    **请求体字段**:
    - `name`: 可选，API Key 的名称
    - `unlimited_balance`: 可选，是否无限余额（true=无限，false=有限，不修改余额数值）
    - `rate_limit`: 可选，每分钟请求限制（null 表示跟随系统默认，0 表示不限制）
    - `allowed_providers`: 可选，允许使用的提供商列表
    - `allowed_api_formats`: 可选，允许使用的 API 格式列表
    - `allowed_models`: 可选，允许使用的模型列表
    - `expire_days`: 可选，过期天数（与 expires_at 二选一）
    - `expires_at`: 可选，过期时间（ISO 格式或 YYYY-MM-DD 格式，优先级高于 expire_days，null 或空字符串表示永不过期）
    - `auto_delete_on_expiry`: 可选，过期后是否自动删除

    **返回字段**:
    - `id`: API Key ID
    - `name`: API Key 名称
    - `key_display`: 脱敏显示的 Key
    - `is_active`: 是否启用
    - `wallet`: 钱包摘要（总余额、充值余额、赠款余额、额度模式等）
    - `rate_limit`: 速率限制配置
    - `expires_at`: 过期时间
    - `updated_at`: 更新时间
    - `message`: 提示信息
    """
    adapter = AdminUpdateApiKeyAdapter(key_id=key_id, key_data=key_data)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.patch("/{key_id}")
async def toggle_api_key(key_id: str, request: Request, db: Session = Depends(get_db)) -> Any:
    """
    切换 API Key 启用状态

    切换指定 API Key 的启用/禁用状态。

    **路径参数**:
    - `key_id`: API Key ID

    **返回字段**:
    - `id`: API Key ID
    - `is_active`: 新的启用状态
    - `message`: 提示信息
    """
    adapter = AdminToggleApiKeyAdapter(key_id=key_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.delete("/{key_id}")
async def delete_api_key(key_id: str, request: Request, db: Session = Depends(get_db)) -> None:
    """
    删除 API Key

    删除指定的 API Key。此操作不可逆。

    **路径参数**:
    - `key_id`: API Key ID

    **返回字段**:
    - `message`: 提示信息
    """
    adapter = AdminDeleteApiKeyAdapter(key_id=key_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/{key_id}")
async def get_api_key_detail(
    key_id: str,
    request: Request,
    include_key: bool = Query(False, description="Include full decrypted key in response"),
    db: Session = Depends(get_db),
) -> Any:
    """
    获取 API Key 详情

    获取指定 API Key 的详细信息。可选择是否返回完整的解密密钥。

    **路径参数**:
    - `key_id`: API Key ID

    **查询参数**:
    - `include_key`: 是否包含完整的解密密钥，默认 false

    **返回字段**:
    - 当 include_key=false 时，返回基本信息：id, user_id, name, key_display, is_active,
      is_standalone, total_requests, total_cost_usd, rate_limit, allowed_providers,
      allowed_api_formats, allowed_models, last_used_at, expires_at, created_at, updated_at,
      wallet
    - 当 include_key=true 时，返回完整密钥：key
    """
    if include_key:
        adapter = AdminGetFullKeyAdapter(key_id=key_id)
    else:
        # Return basic key info without full key
        adapter = AdminGetKeyDetailAdapter(key_id=key_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


class AdminListStandaloneKeysAdapter(AdminApiAdapter):
    """列出独立余额Keys"""

    def __init__(
        self,
        skip: int,
        limit: int,
        is_active: bool | None,
    ):
        self.skip = skip
        self.limit = limit
        self.is_active = is_active

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        result = await run_in_threadpool(
            _list_standalone_api_keys_sync,
            self.skip,
            self.limit,
            self.is_active,
        )
        context.add_audit_metadata(
            action="list_standalone_api_keys",
            filter_is_active=self.is_active,
            limit=self.limit,
            skip=self.skip,
            total=result["total"],
        )
        return result


class AdminCreateStandaloneKeyAdapter(AdminApiAdapter):
    """创建独立余额Key"""

    def __init__(self, key_data: CreateApiKeyRequest):
        self.key_data = key_data

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        result, audit_meta = await run_in_threadpool(
            _create_standalone_api_key_sync,
            context.user.id,
            self.key_data,
        )
        logger.info(
            f"管理员创建独立余额Key: ID {result['id']}, 初始余额 ${self.key_data.initial_balance_usd}"
        )
        context.add_audit_metadata(**audit_meta)
        return result


class AdminUpdateApiKeyAdapter(AdminApiAdapter):
    """更新独立余额Key"""

    def __init__(self, key_id: str, key_data: CreateApiKeyRequest):
        self.key_id = key_id
        self.key_data = key_data

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        result, changed_fields = await run_in_threadpool(
            _update_standalone_api_key_sync,
            self.key_id,
            self.key_data,
        )
        logger.info(f"管理员更新独立余额Key: ID {self.key_id}, 更新字段 {changed_fields}")
        context.add_audit_metadata(
            action="update_standalone_api_key",
            key_id=self.key_id,
            updated_fields=changed_fields,
        )
        return result


class AdminToggleApiKeyAdapter(AdminApiAdapter):
    def __init__(self, key_id: str):
        self.key_id = key_id

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        result, audit_meta = await run_in_threadpool(_toggle_standalone_api_key_sync, self.key_id)
        logger.info(
            f"管理员切换API密钥状态: Key ID {self.key_id}, 新状态 {'启用' if result['is_active'] else '禁用'}"
        )
        context.add_audit_metadata(**audit_meta)
        return result


class AdminDeleteApiKeyAdapter(AdminApiAdapter):
    def __init__(self, key_id: str):
        self.key_id = key_id

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        result, audit_meta, user_email = await run_in_threadpool(
            _delete_standalone_api_key_sync,
            self.key_id,
        )
        logger.info(f"管理员删除API密钥: Key ID {self.key_id}, 用户 {user_email or '未知'}")
        context.add_audit_metadata(**audit_meta)
        return result


class AdminGetFullKeyAdapter(AdminApiAdapter):
    """获取完整的API密钥"""

    def __init__(self, key_id: str):
        self.key_id = key_id

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        from src.core.crypto import crypto_service

        db = context.db

        # 查找API密钥
        api_key = db.query(ApiKey).filter(ApiKey.id == self.key_id).first()
        if not api_key:
            raise NotFoundException("API密钥不存在", "api_key")
        if not api_key.is_standalone:
            raise InvalidRequestException("仅支持查看独立密钥")

        # 解密完整密钥
        if not api_key.key_encrypted:
            raise HTTPException(status_code=400, detail="该密钥没有存储完整密钥信息")

        try:
            full_key = crypto_service.decrypt(api_key.key_encrypted)
        except Exception as e:
            logger.error(f"解密API密钥失败: Key ID {self.key_id}, 错误: {e}")
            raise HTTPException(status_code=500, detail="解密密钥失败")

        logger.info(f"管理员查看完整API密钥: Key ID {self.key_id}")

        context.add_audit_metadata(
            action="view_full_api_key",
            key_id=self.key_id,
            key_name=api_key.name,
        )

        return {
            "key": full_key,
        }


class AdminGetKeyDetailAdapter(AdminApiAdapter):
    """Get API key detail without full key"""

    def __init__(self, key_id: str):
        self.key_id = key_id

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        db = context.db

        api_key = db.query(ApiKey).filter(ApiKey.id == self.key_id).first()
        if not api_key:
            raise NotFoundException("API密钥不存在", "api_key")
        if not api_key.is_standalone:
            raise InvalidRequestException("仅支持查看独立密钥")

        wallet = WalletService.get_wallet(db, api_key_id=api_key.id)
        wallet_summary = WalletService.serialize_wallet_summary(wallet)

        context.add_audit_metadata(
            action="get_api_key_detail",
            key_id=self.key_id,
        )

        return {
            "id": api_key.id,
            "user_id": api_key.user_id,
            "name": api_key.name,
            "key_display": api_key.get_display_key(),
            "is_active": api_key.is_active,
            "is_standalone": api_key.is_standalone,
            "total_requests": api_key.total_requests,
            "total_tokens": int(
                (
                    db.query(func.sum(Usage.total_tokens))
                    .filter(Usage.api_key_id == api_key.id)
                    .scalar()
                )
                or 0
            ),
            "total_cost_usd": float(api_key.total_cost_usd or 0),
            "rate_limit": api_key.rate_limit,
            "allowed_providers": api_key.allowed_providers,
            "allowed_api_formats": api_key.allowed_api_formats,
            "allowed_models": api_key.allowed_models,
            "last_used_at": api_key.last_used_at.isoformat() if api_key.last_used_at else None,
            "expires_at": api_key.expires_at.isoformat() if api_key.expires_at else None,
            "created_at": api_key.created_at.isoformat(),
            "updated_at": api_key.updated_at.isoformat() if api_key.updated_at else None,
            "wallet": wallet_summary,
        }
