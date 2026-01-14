"""系统设置API端点。"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Optional

from fastapi import APIRouter, Depends, HTTPException, Request, status
from pydantic import ValidationError
from sqlalchemy.orm import Session

from src.api.base.admin_adapter import AdminApiAdapter
from src.api.base.pipeline import ApiRequestPipeline
from src.core.exceptions import InvalidRequestException, NotFoundException, translate_pydantic_error
from src.database import get_db
from src.models.api import SystemSettingsRequest, SystemSettingsResponse
from src.models.database import ApiKey, Provider, Usage, User
from src.services.email.email_template import EmailTemplate
from src.services.system.config import SystemConfigService

router = APIRouter(prefix="/api/admin/system", tags=["Admin - System"])


def _get_version_from_git() -> str | None:
    """从 git describe 获取版本号"""
    import subprocess

    try:
        result = subprocess.run(
            ["git", "describe", "--tags", "--always"],
            capture_output=True,
            text=True,
            timeout=5,
        )
        if result.returncode == 0:
            version = result.stdout.strip()
            if version.startswith("v"):
                version = version[1:]
            return version
    except Exception:
        pass
    return None


def _get_current_version() -> str:
    """获取当前版本号"""
    version = _get_version_from_git()
    if version:
        return version
    try:
        from src._version import __version__

        return __version__
    except ImportError:
        return "unknown"


def _parse_version(version_str: str) -> tuple:
    """解析版本号为可比较的元组，支持 3-4 段版本号

    例如:
    - '0.2.5' -> (0, 2, 5, 0)
    - '0.2.5.1' -> (0, 2, 5, 1)
    - 'v0.2.5-4-g1234567' -> (0, 2, 5, 0)
    """
    import re

    version_str = version_str.lstrip("v")
    main_version = re.split(r"[-+]", version_str)[0]
    try:
        parts = main_version.split(".")
        # 标准化为 4 段，便于比较
        int_parts = [int(p) for p in parts]
        while len(int_parts) < 4:
            int_parts.append(0)
        return tuple(int_parts[:4])
    except ValueError:
        return (0, 0, 0, 0)


@router.get("/version")
async def get_system_version():
    """
    获取系统版本信息

    获取当前系统的版本号。优先从 git describe 获取，回退到静态版本文件。

    **返回字段**:
    - `version`: 版本号字符串
    """
    return {"version": _get_current_version()}


@router.get("/check-update")
async def check_update():
    """
    检查系统更新

    从 GitHub Releases 获取最新版本并与当前版本对比。

    **返回字段**:
    - `current_version`: 当前版本号
    - `latest_version`: 最新版本号
    - `has_update`: 是否有更新可用
    - `release_url`: 最新版本的 GitHub 页面链接
    - `release_notes`: 更新日志 (Markdown 格式)
    - `published_at`: 发布时间 (ISO 8601 格式)
    """
    import httpx

    from src.clients.http_client import HTTPClientPool

    current_version = _get_current_version()
    github_repo = "Aethersailor/Aether"
    github_releases_url = f"https://api.github.com/repos/{github_repo}/releases"

    def _make_empty_response(error: str | None = None):
        return {
            "current_version": current_version,
            "latest_version": None,
            "has_update": False,
            "release_url": None,
            "release_notes": None,
            "published_at": None,
            "error": error,
        }

    try:
        async with HTTPClientPool.get_temp_client(
            timeout=httpx.Timeout(connect=5.0, read=10.0, write=5.0, pool=5.0)
        ) as client:
            response = await client.get(
                github_releases_url,
                headers={
                    "Accept": "application/vnd.github.v3+json",
                    "User-Agent": f"Aether/{current_version}",
                },
                params={"per_page": 10},
            )

            if response.status_code != 200:
                return _make_empty_response(f"GitHub API 返回错误: {response.status_code}")

            releases = response.json()
            if not releases:
                return _make_empty_response()

            # 找到最新的正式 release（排除 prerelease 和 draft，按版本号排序）
            valid_releases = []
            for release in releases:
                if release.get("prerelease") or release.get("draft"):
                    continue
                tag_name = release.get("tag_name", "")
                if tag_name.startswith("v") or (tag_name and tag_name[0].isdigit()):
                    valid_releases.append((release, _parse_version(tag_name)))

            if not valid_releases:
                return _make_empty_response()

            # 按版本号排序，取最大的
            valid_releases.sort(key=lambda x: x[1], reverse=True)
            latest_release = valid_releases[0][0]

            latest_tag = latest_release.get("tag_name", "")
            latest_version = latest_tag.lstrip("v")

            current_tuple = _parse_version(current_version)
            latest_tuple = _parse_version(latest_version)
            has_update = latest_tuple > current_tuple

            return {
                "current_version": current_version,
                "latest_version": latest_version,
                "has_update": has_update,
                "release_url": latest_release.get("html_url")
                or f"https://github.com/{github_repo}/releases/tag/{latest_tag}",
                "release_notes": latest_release.get("body"),
                "published_at": latest_release.get("published_at"),
                "error": None,
            }

    except httpx.TimeoutException:
        return _make_empty_response("检查更新超时")
    except Exception as e:
        return _make_empty_response(f"检查更新失败: {str(e)}")


pipeline = ApiRequestPipeline()


@router.get("/settings")
async def get_system_settings(request: Request, db: Session = Depends(get_db)):
    """
    获取系统设置

    获取系统的全局设置信息。需要管理员权限。

    **返回字段**:
    - `default_provider`: 默认提供商名称
    - `default_model`: 默认模型名称
    - `enable_usage_tracking`: 是否启用使用情况追踪
    """

    adapter = AdminGetSystemSettingsAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.put("/settings")
async def update_system_settings(http_request: Request, db: Session = Depends(get_db)):
    """
    更新系统设置

    更新系统的全局设置。需要管理员权限。

    **请求体字段**:
    - `default_provider`: 可选，默认提供商名称（空字符串表示清除设置）
    - `default_model`: 可选，默认模型名称（空字符串表示清除设置）
    - `enable_usage_tracking`: 可选，是否启用使用情况追踪

    **返回字段**:
    - `message`: 操作结果信息
    """

    adapter = AdminUpdateSystemSettingsAdapter()
    return await pipeline.run(adapter=adapter, http_request=http_request, db=db, mode=adapter.mode)


@router.get("/configs")
async def get_all_system_configs(request: Request, db: Session = Depends(get_db)):
    """
    获取所有系统配置

    获取系统中所有的配置项。需要管理员权限。

    **返回字段**:
    - 配置项的键值对字典
    """

    adapter = AdminGetAllConfigsAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/configs/{key}")
async def get_system_config(key: str, request: Request, db: Session = Depends(get_db)):
    """
    获取特定系统配置

    获取指定配置项的值。需要管理员权限。

    **路径参数**:
    - `key`: 配置项键名

    **返回字段**:
    - `key`: 配置项键名
    - `value`: 配置项的值（敏感配置项不返回实际值）
    - `is_set`: 可选，对于敏感配置项，指示是否已设置
    """

    adapter = AdminGetSystemConfigAdapter(key=key)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.put("/configs/{key}")
async def set_system_config(
    key: str,
    request: Request,
    db: Session = Depends(get_db),
):
    """
    设置系统配置

    设置或更新指定配置项的值。需要管理员权限。

    **路径参数**:
    - `key`: 配置项键名

    **请求体字段**:
    - `value`: 配置项的值
    - `description`: 可选，配置项描述

    **返回字段**:
    - `key`: 配置项键名
    - `value`: 配置项的值（敏感配置项显示为 ********）
    - `description`: 配置项描述
    - `updated_at`: 更新时间
    """

    adapter = AdminSetSystemConfigAdapter(key=key)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.delete("/configs/{key}")
async def delete_system_config(key: str, request: Request, db: Session = Depends(get_db)):
    """
    删除系统配置

    删除指定的配置项。需要管理员权限。

    **路径参数**:
    - `key`: 配置项键名

    **返回字段**:
    - `message`: 操作结果信息
    """

    adapter = AdminDeleteSystemConfigAdapter(key=key)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/stats")
async def get_system_stats(request: Request, db: Session = Depends(get_db)):
    """
    获取系统统计信息

    获取系统的整体统计数据。需要管理员权限。

    **返回字段**:
    - `users`: 用户统计（total: 总用户数, active: 活跃用户数）
    - `providers`: 提供商统计（total: 总提供商数, active: 活跃提供商数）
    - `api_keys`: API Key 总数
    - `requests`: 请求总数
    """
    adapter = AdminSystemStatsAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.post("/cleanup")
async def trigger_cleanup(request: Request, db: Session = Depends(get_db)):
    """
    手动触发清理任务

    手动触发使用记录清理任务，清理过期的请求/响应数据。需要管理员权限。

    **返回字段**:
    - `message`: 操作结果信息
    - `stats`: 清理统计信息
      - `total_records`: 总记录数统计（before, after, deleted）
      - `body_fields`: 请求/响应体字段清理统计（before, after, cleaned）
      - `header_fields`: 请求/响应头字段清理统计（before, after, cleaned）
    - `timestamp`: 清理完成时间
    """
    adapter = AdminTriggerCleanupAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/api-formats")
async def get_api_formats(request: Request, db: Session = Depends(get_db)):
    """
    获取所有可用的 API 格式列表

    获取系统支持的所有 API 格式及其元数据。需要管理员权限。

    **返回字段**:
    - `formats`: API 格式列表，每个格式包含：
      - `value`: 格式值
      - `label`: 显示名称
      - `default_path`: 默认路径
      - `aliases`: 别名列表
    """
    adapter = AdminGetApiFormatsAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/config/export")
async def export_config(request: Request, db: Session = Depends(get_db)):
    """导出提供商和模型配置（管理员）"""
    adapter = AdminExportConfigAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.post("/config/import")
async def import_config(request: Request, db: Session = Depends(get_db)):
    """导入提供商和模型配置（管理员）"""
    adapter = AdminImportConfigAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/users/export")
async def export_users(request: Request, db: Session = Depends(get_db)):
    """导出用户数据（管理员）"""
    adapter = AdminExportUsersAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.post("/users/import")
async def import_users(request: Request, db: Session = Depends(get_db)):
    """导入用户数据（管理员）"""
    adapter = AdminImportUsersAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.post("/smtp/test")
async def test_smtp(request: Request, db: Session = Depends(get_db)):
    """测试 SMTP 连接（管理员）"""
    adapter = AdminTestSmtpAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


# -------- 邮件模板 API --------


@router.get("/email/templates")
async def get_email_templates(request: Request, db: Session = Depends(get_db)):
    """获取所有邮件模板（管理员）"""
    adapter = AdminGetEmailTemplatesAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/email/templates/{template_type}")
async def get_email_template(
    template_type: str, request: Request, db: Session = Depends(get_db)
):
    """获取指定类型的邮件模板（管理员）"""
    adapter = AdminGetEmailTemplateAdapter(template_type=template_type)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.put("/email/templates/{template_type}")
async def update_email_template(
    template_type: str, request: Request, db: Session = Depends(get_db)
):
    """更新邮件模板（管理员）"""
    adapter = AdminUpdateEmailTemplateAdapter(template_type=template_type)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.post("/email/templates/{template_type}/preview")
async def preview_email_template(
    template_type: str, request: Request, db: Session = Depends(get_db)
):
    """预览邮件模板（管理员）"""
    adapter = AdminPreviewEmailTemplateAdapter(template_type=template_type)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.post("/email/templates/{template_type}/reset")
async def reset_email_template(
    template_type: str, request: Request, db: Session = Depends(get_db)
):
    """重置邮件模板为默认值（管理员）"""
    adapter = AdminResetEmailTemplateAdapter(template_type=template_type)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


# -------- 系统设置适配器 --------


class AdminGetSystemSettingsAdapter(AdminApiAdapter):
    async def handle(self, context):  # type: ignore[override]
        db = context.db
        default_provider = SystemConfigService.get_default_provider(db)
        default_model = SystemConfigService.get_config(db, "default_model")
        enable_usage_tracking = (
            SystemConfigService.get_config(db, "enable_usage_tracking", "true") == "true"
        )

        return SystemSettingsResponse(
            default_provider=default_provider,
            default_model=default_model,
            enable_usage_tracking=enable_usage_tracking,
        )


class AdminUpdateSystemSettingsAdapter(AdminApiAdapter):
    async def handle(self, context):  # type: ignore[override]
        db = context.db
        payload = context.ensure_json_body()
        try:
            settings_request = SystemSettingsRequest.model_validate(payload)
        except ValidationError as e:
            errors = e.errors()
            if errors:
                raise InvalidRequestException(translate_pydantic_error(errors[0]))
            raise InvalidRequestException("请求数据验证失败")

        if settings_request.default_provider is not None:
            provider = (
                db.query(Provider)
                .filter(
                    Provider.name == settings_request.default_provider,
                    Provider.is_active.is_(True),
                )
                .first()
            )

            if not provider and settings_request.default_provider != "":
                raise HTTPException(
                    status_code=status.HTTP_400_BAD_REQUEST,
                    detail=f"提供商 '{settings_request.default_provider}' 不存在或未启用",
                )

            if settings_request.default_provider:
                SystemConfigService.set_default_provider(db, settings_request.default_provider)
            else:
                SystemConfigService.delete_config(db, "default_provider")

        if settings_request.default_model is not None:
            if settings_request.default_model:
                SystemConfigService.set_config(db, "default_model", settings_request.default_model)
            else:
                SystemConfigService.delete_config(db, "default_model")

        if settings_request.enable_usage_tracking is not None:
            SystemConfigService.set_config(
                db,
                "enable_usage_tracking",
                str(settings_request.enable_usage_tracking).lower(),
            )

        return {"message": "系统设置更新成功"}


class AdminGetAllConfigsAdapter(AdminApiAdapter):
    async def handle(self, context):  # type: ignore[override]
        return SystemConfigService.get_all_configs(context.db)


@dataclass
class AdminGetSystemConfigAdapter(AdminApiAdapter):
    key: str

    # 敏感配置项，不返回实际值
    SENSITIVE_KEYS = {"smtp_password"}

    async def handle(self, context):  # type: ignore[override]
        value = SystemConfigService.get_config(context.db, self.key)
        if value is None:
            raise NotFoundException(f"配置项 '{self.key}' 不存在")
        # 对敏感配置，只返回是否已设置的标志，不返回实际值
        if self.key in self.SENSITIVE_KEYS:
            return {"key": self.key, "value": None, "is_set": bool(value)}
        return {"key": self.key, "value": value}


@dataclass
class AdminSetSystemConfigAdapter(AdminApiAdapter):
    key: str

    # 需要加密存储的配置项
    ENCRYPTED_KEYS = {"smtp_password"}

    async def handle(self, context):  # type: ignore[override]
        payload = context.ensure_json_body()
        value = payload.get("value")

        # 对敏感配置进行加密
        if self.key in self.ENCRYPTED_KEYS and value:
            from src.core.crypto import crypto_service
            value = crypto_service.encrypt(value)

        config = SystemConfigService.set_config(
            context.db,
            self.key,
            value,
            payload.get("description"),
        )

        # 返回时不暴露加密后的值
        display_value = "********" if self.key in self.ENCRYPTED_KEYS else config.value

        return {
            "key": config.key,
            "value": display_value,
            "description": config.description,
            "updated_at": config.updated_at.isoformat(),
        }


@dataclass
class AdminDeleteSystemConfigAdapter(AdminApiAdapter):
    key: str

    async def handle(self, context):  # type: ignore[override]
        deleted = SystemConfigService.delete_config(context.db, self.key)
        if not deleted:
            raise NotFoundException(f"配置项 '{self.key}' 不存在")
        return {"message": f"配置项 '{self.key}' 已删除"}


class AdminSystemStatsAdapter(AdminApiAdapter):
    async def handle(self, context):  # type: ignore[override]
        db = context.db
        total_users = db.query(User).count()
        active_users = db.query(User).filter(User.is_active.is_(True)).count()
        total_providers = db.query(Provider).count()
        active_providers = db.query(Provider).filter(Provider.is_active.is_(True)).count()
        total_api_keys = db.query(ApiKey).count()
        total_requests = db.query(Usage).count()

        return {
            "users": {"total": total_users, "active": active_users},
            "providers": {"total": total_providers, "active": active_providers},
            "api_keys": total_api_keys,
            "requests": total_requests,
        }


class AdminTriggerCleanupAdapter(AdminApiAdapter):
    async def handle(self, context):  # type: ignore[override]
        """手动触发清理任务"""
        from datetime import datetime, timedelta, timezone

        from sqlalchemy import func

        from src.services.system.cleanup_scheduler import get_cleanup_scheduler

        db = context.db

        # 获取清理前的统计信息
        total_before = db.query(Usage).count()
        with_body_before = (
            db.query(Usage)
            .filter((Usage.request_body.isnot(None)) | (Usage.response_body.isnot(None)))
            .count()
        )
        with_headers_before = (
            db.query(Usage)
            .filter((Usage.request_headers.isnot(None)) | (Usage.response_headers.isnot(None)))
            .count()
        )

        # 触发清理
        cleanup_scheduler = get_cleanup_scheduler()
        await cleanup_scheduler._perform_cleanup()

        # 获取清理后的统计信息
        total_after = db.query(Usage).count()
        with_body_after = (
            db.query(Usage)
            .filter((Usage.request_body.isnot(None)) | (Usage.response_body.isnot(None)))
            .count()
        )
        with_headers_after = (
            db.query(Usage)
            .filter((Usage.request_headers.isnot(None)) | (Usage.response_headers.isnot(None)))
            .count()
        )

        return {
            "message": "清理任务执行完成",
            "stats": {
                "total_records": {
                    "before": total_before,
                    "after": total_after,
                    "deleted": total_before - total_after,
                },
                "body_fields": {
                    "before": with_body_before,
                    "after": with_body_after,
                    "cleaned": with_body_before - with_body_after,
                },
                "header_fields": {
                    "before": with_headers_before,
                    "after": with_headers_after,
                    "cleaned": with_headers_before - with_headers_after,
                },
            },
            "timestamp": datetime.now(timezone.utc).isoformat(),
        }


class AdminGetApiFormatsAdapter(AdminApiAdapter):
    async def handle(self, context):  # type: ignore[override]
        """获取所有可用的API格式"""
        from src.core.api_format_metadata import API_FORMAT_DEFINITIONS
        from src.core.enums import APIFormat

        _ = context  # 参数保留以符合接口规范

        formats = []
        for api_format in APIFormat:
            definition = API_FORMAT_DEFINITIONS.get(api_format)
            formats.append(
                {
                    "value": api_format.value,
                    "label": api_format.value,
                    "default_path": definition.default_path if definition else "/",
                    "aliases": list(definition.aliases) if definition else [],
                }
            )

        return {"formats": formats}


class AdminExportConfigAdapter(AdminApiAdapter):
    async def handle(self, context):  # type: ignore[override]
        """导出提供商和模型配置（解密数据）"""
        from datetime import datetime, timezone

        from src.core.crypto import crypto_service
        from src.models.database import GlobalModel, Model, ProviderAPIKey, ProviderEndpoint

        db = context.db

        # 导出 GlobalModels
        global_models = db.query(GlobalModel).all()
        global_models_data = []
        for gm in global_models:
            global_models_data.append(
                {
                    "name": gm.name,
                    "display_name": gm.display_name,
                    "default_price_per_request": gm.default_price_per_request,
                    "default_tiered_pricing": gm.default_tiered_pricing,
                    "supported_capabilities": gm.supported_capabilities,
                    "config": gm.config,
                    "is_active": gm.is_active,
                }
            )

        # 导出 Providers 及其关联数据
        providers = db.query(Provider).all()
        providers_data = []
        for provider in providers:
            # 导出 Endpoints
            endpoints = (
                db.query(ProviderEndpoint)
                .filter(ProviderEndpoint.provider_id == provider.id)
                .all()
            )
            endpoints_data = []
            for ep in endpoints:
                endpoints_data.append(
                    {
                        "api_format": ep.api_format,
                        "base_url": ep.base_url,
                        "headers": ep.headers,
                        "timeout": ep.timeout,
                        "max_retries": ep.max_retries,
                        "is_active": ep.is_active,
                        "custom_path": ep.custom_path,
                        "config": ep.config,
                        "proxy": ep.proxy,
                    }
                )

            # 导出 Provider Keys（按 provider_id 归属，包含 api_formats）
            keys = (
                db.query(ProviderAPIKey)
                .filter(ProviderAPIKey.provider_id == provider.id)
                .order_by(ProviderAPIKey.internal_priority.asc(), ProviderAPIKey.created_at.asc())
                .all()
            )
            keys_data = []
            for key in keys:
                # 解密 API Key
                try:
                    decrypted_key = crypto_service.decrypt(key.api_key)
                except Exception:
                    decrypted_key = ""

                keys_data.append(
                    {
                        "api_key": decrypted_key,
                        "name": key.name,
                        "note": key.note,
                        "api_formats": key.api_formats or [],
                        "rate_multiplier": key.rate_multiplier,
                        "rate_multipliers": key.rate_multipliers,
                        "internal_priority": key.internal_priority,
                        "global_priority": key.global_priority,
                        "rpm_limit": key.rpm_limit,
                        "allowed_models": key.allowed_models,
                        "capabilities": key.capabilities,
                        "cache_ttl_minutes": key.cache_ttl_minutes,
                        "max_probe_interval_minutes": key.max_probe_interval_minutes,
                        "is_active": key.is_active,
                    }
                )

            # 导出 Provider Models
            models = db.query(Model).filter(Model.provider_id == provider.id).all()
            models_data = []
            for model in models:
                # 获取关联的 GlobalModel 名称
                global_model = (
                    db.query(GlobalModel).filter(GlobalModel.id == model.global_model_id).first()
                )
                models_data.append(
                    {
                        "global_model_name": global_model.name if global_model else None,
                        "provider_model_name": model.provider_model_name,
                        "provider_model_mappings": model.provider_model_mappings,
                        "price_per_request": model.price_per_request,
                        "tiered_pricing": model.tiered_pricing,
                        "supports_vision": model.supports_vision,
                        "supports_function_calling": model.supports_function_calling,
                        "supports_streaming": model.supports_streaming,
                        "supports_extended_thinking": model.supports_extended_thinking,
                        "supports_image_generation": model.supports_image_generation,
                        "is_active": model.is_active,
                        "config": model.config,
                    }
                )

            providers_data.append(
                {
                    "name": provider.name,
                    "description": provider.description,
                    "website": provider.website,
                    "billing_type": provider.billing_type.value if provider.billing_type else None,
                    "monthly_quota_usd": provider.monthly_quota_usd,
                    "quota_reset_day": provider.quota_reset_day,
                    "provider_priority": provider.provider_priority,
                    "is_active": provider.is_active,
                    "concurrent_limit": provider.concurrent_limit,
                    "timeout": provider.timeout,
                    "max_retries": provider.max_retries,
                    "proxy": provider.proxy,
                    "config": provider.config,
                    "endpoints": endpoints_data,
                    "api_keys": keys_data,
                    "models": models_data,
                }
            )

        return {
            "version": "2.0",
            "exported_at": datetime.now(timezone.utc).isoformat(),
            "global_models": global_models_data,
            "providers": providers_data,
        }


MAX_IMPORT_SIZE = 10 * 1024 * 1024  # 10MB


class AdminImportConfigAdapter(AdminApiAdapter):
    async def handle(self, context):  # type: ignore[override]
        """导入提供商和模型配置"""
        import uuid
        from datetime import datetime, timezone

        from src.core.crypto import crypto_service
        from src.core.enums import ProviderBillingType
        from src.models.database import GlobalModel, Model, ProviderAPIKey, ProviderEndpoint

        # 检查请求体大小
        if context.raw_body and len(context.raw_body) > MAX_IMPORT_SIZE:
            raise InvalidRequestException("请求体大小不能超过 10MB")

        db = context.db
        payload = context.ensure_json_body()

        # 验证配置版本
        version = payload.get("version")
        if version != "2.0":
            raise InvalidRequestException(f"不支持的配置版本: {version}")

        # 获取导入选项
        merge_mode = payload.get("merge_mode", "skip")  # skip, overwrite, error
        global_models_data = payload.get("global_models", [])
        providers_data = payload.get("providers", [])

        stats = {
            "global_models": {"created": 0, "updated": 0, "skipped": 0},
            "providers": {"created": 0, "updated": 0, "skipped": 0},
            "endpoints": {"created": 0, "updated": 0, "skipped": 0},
            "keys": {"created": 0, "updated": 0, "skipped": 0},
            "models": {"created": 0, "updated": 0, "skipped": 0},
            "errors": [],
        }

        try:
            # 导入 GlobalModels
            global_model_map = {}  # name -> id 映射
            for gm_data in global_models_data:
                existing = (
                    db.query(GlobalModel).filter(GlobalModel.name == gm_data["name"]).first()
                )

                if existing:
                    global_model_map[gm_data["name"]] = existing.id
                    if merge_mode == "skip":
                        stats["global_models"]["skipped"] += 1
                        continue
                    elif merge_mode == "error":
                        raise InvalidRequestException(
                            f"GlobalModel '{gm_data['name']}' 已存在"
                        )
                    elif merge_mode == "overwrite":
                        # 更新现有记录
                        existing.display_name = gm_data.get(
                            "display_name", existing.display_name
                        )
                        existing.default_price_per_request = gm_data.get(
                            "default_price_per_request"
                        )
                        existing.default_tiered_pricing = gm_data.get(
                            "default_tiered_pricing", existing.default_tiered_pricing
                        )
                        existing.supported_capabilities = gm_data.get(
                            "supported_capabilities"
                        )
                        existing.config = gm_data.get("config")
                        existing.is_active = gm_data.get("is_active", True)
                        existing.updated_at = datetime.now(timezone.utc)
                        stats["global_models"]["updated"] += 1
                else:
                    # 创建新记录
                    new_gm = GlobalModel(
                        id=str(uuid.uuid4()),
                        name=gm_data["name"],
                        display_name=gm_data.get("display_name", gm_data["name"]),
                        default_price_per_request=gm_data.get("default_price_per_request"),
                        default_tiered_pricing=gm_data.get(
                            "default_tiered_pricing",
                            {"tiers": [{"up_to": None, "input_price_per_1m": 0, "output_price_per_1m": 0}]},
                        ),
                        supported_capabilities=gm_data.get("supported_capabilities"),
                        config=gm_data.get("config"),
                        is_active=gm_data.get("is_active", True),
                    )
                    db.add(new_gm)
                    db.flush()
                    global_model_map[gm_data["name"]] = new_gm.id
                    stats["global_models"]["created"] += 1

            # 导入 Providers
            for prov_data in providers_data:
                existing_provider = (
                    db.query(Provider).filter(Provider.name == prov_data["name"]).first()
                )

                if existing_provider:
                    provider_id = existing_provider.id
                    if merge_mode == "skip":
                        stats["providers"]["skipped"] += 1
                        # 仍然需要处理 endpoints 和 models（如果存在）
                    elif merge_mode == "error":
                        raise InvalidRequestException(
                            f"Provider '{prov_data['name']}' 已存在"
                        )
                    elif merge_mode == "overwrite":
                        # 更新现有记录
                        existing_provider.name = prov_data.get(
                            "name", existing_provider.name
                        )
                        existing_provider.description = prov_data.get("description")
                        existing_provider.website = prov_data.get("website")
                        if prov_data.get("billing_type"):
                            existing_provider.billing_type = ProviderBillingType(
                                prov_data["billing_type"]
                            )
                        existing_provider.monthly_quota_usd = prov_data.get(
                            "monthly_quota_usd"
                        )
                        existing_provider.quota_reset_day = prov_data.get(
                            "quota_reset_day", 30
                        )
                        existing_provider.provider_priority = prov_data.get(
                            "provider_priority", 100
                        )
                        existing_provider.is_active = prov_data.get("is_active", True)
                        existing_provider.concurrent_limit = prov_data.get(
                            "concurrent_limit"
                        )
                        existing_provider.timeout = prov_data.get("timeout", existing_provider.timeout)
                        existing_provider.max_retries = prov_data.get(
                            "max_retries", existing_provider.max_retries
                        )
                        existing_provider.proxy = prov_data.get("proxy", existing_provider.proxy)
                        existing_provider.config = prov_data.get("config")
                        existing_provider.updated_at = datetime.now(timezone.utc)
                        stats["providers"]["updated"] += 1
                else:
                    # 创建新 Provider
                    billing_type = ProviderBillingType.PAY_AS_YOU_GO
                    if prov_data.get("billing_type"):
                        billing_type = ProviderBillingType(prov_data["billing_type"])

                    new_provider = Provider(
                        id=str(uuid.uuid4()),
                        name=prov_data["name"],
                        description=prov_data.get("description"),
                        website=prov_data.get("website"),
                        billing_type=billing_type,
                        monthly_quota_usd=prov_data.get("monthly_quota_usd"),
                        quota_reset_day=prov_data.get("quota_reset_day", 30),
                        provider_priority=prov_data.get("provider_priority", 100),
                        is_active=prov_data.get("is_active", True),
                        concurrent_limit=prov_data.get("concurrent_limit"),
                        timeout=prov_data.get("timeout"),
                        max_retries=prov_data.get("max_retries"),
                        proxy=prov_data.get("proxy"),
                        config=prov_data.get("config"),
                    )
                    db.add(new_provider)
                    db.flush()
                    provider_id = new_provider.id
                    stats["providers"]["created"] += 1

                # 导入 Endpoints
                for ep_data in prov_data.get("endpoints", []):
                    existing_ep = (
                        db.query(ProviderEndpoint)
                        .filter(
                            ProviderEndpoint.provider_id == provider_id,
                            ProviderEndpoint.api_format == ep_data["api_format"],
                        )
                        .first()
                    )

                    if existing_ep:
                        if merge_mode == "skip":
                            stats["endpoints"]["skipped"] += 1
                        elif merge_mode == "error":
                            raise InvalidRequestException(
                                f"Endpoint '{ep_data['api_format']}' 已存在于 Provider '{prov_data['name']}'"
                            )
                        elif merge_mode == "overwrite":
                            existing_ep.base_url = ep_data.get(
                                "base_url", existing_ep.base_url
                            )
                            existing_ep.headers = ep_data.get("headers")
                            existing_ep.timeout = ep_data.get("timeout", 300)
                            existing_ep.max_retries = ep_data.get("max_retries", 2)
                            existing_ep.is_active = ep_data.get("is_active", True)
                            existing_ep.custom_path = ep_data.get("custom_path")
                            existing_ep.config = ep_data.get("config")
                            existing_ep.proxy = ep_data.get("proxy")
                            existing_ep.updated_at = datetime.now(timezone.utc)
                            stats["endpoints"]["updated"] += 1
                    else:
                        new_ep = ProviderEndpoint(
                            id=str(uuid.uuid4()),
                            provider_id=provider_id,
                            api_format=ep_data["api_format"],
                            base_url=ep_data["base_url"],
                            headers=ep_data.get("headers"),
                            timeout=ep_data.get("timeout", 300),
                            max_retries=ep_data.get("max_retries", 2),
                            is_active=ep_data.get("is_active", True),
                            custom_path=ep_data.get("custom_path"),
                            config=ep_data.get("config"),
                            proxy=ep_data.get("proxy"),
                        )
                        db.add(new_ep)
                        db.flush()
                        stats["endpoints"]["created"] += 1

                # 导入 Provider Keys（按 provider_id 归属）
                endpoint_format_rows = (
                    db.query(ProviderEndpoint.api_format)
                    .filter(ProviderEndpoint.provider_id == provider_id)
                    .all()
                )
                endpoint_formats: set[str] = set()
                for (api_format,) in endpoint_format_rows:
                    fmt = api_format.value if hasattr(api_format, "value") else str(api_format)
                    endpoint_formats.add(fmt.strip().upper())
                existing_keys = (
                    db.query(ProviderAPIKey)
                    .filter(ProviderAPIKey.provider_id == provider_id)
                    .all()
                )
                existing_key_values = set()
                for ek in existing_keys:
                    try:
                        decrypted = crypto_service.decrypt(ek.api_key)
                        existing_key_values.add(decrypted)
                    except Exception:
                        pass

                for key_data in prov_data.get("api_keys", []):
                    if not key_data.get("api_key"):
                        stats["errors"].append(
                            f"跳过空 API Key (Provider: {prov_data['name']})"
                        )
                        continue

                    plaintext_key = key_data["api_key"]
                    if plaintext_key in existing_key_values:
                        stats["keys"]["skipped"] += 1
                        continue

                    raw_formats = key_data.get("api_formats") or []
                    if not isinstance(raw_formats, list) or len(raw_formats) == 0:
                        stats["errors"].append(
                            f"跳过无 api_formats 的 Key (Provider: {prov_data['name']})"
                        )
                        continue

                    normalized_formats: list[str] = []
                    seen: set[str] = set()
                    missing_formats: list[str] = []
                    for fmt in raw_formats:
                        if not isinstance(fmt, str):
                            continue
                        fmt_upper = fmt.strip().upper()
                        if not fmt_upper or fmt_upper in seen:
                            continue
                        seen.add(fmt_upper)
                        if endpoint_formats and fmt_upper not in endpoint_formats:
                            missing_formats.append(fmt_upper)
                            continue
                        normalized_formats.append(fmt_upper)

                    if missing_formats:
                        stats["errors"].append(
                            f"Key (Provider: {prov_data['name']}) 的 api_formats 未配置对应 Endpoint，已跳过: {missing_formats}"
                        )

                    if len(normalized_formats) == 0:
                        stats["keys"]["skipped"] += 1
                        continue

                    encrypted_key = crypto_service.encrypt(plaintext_key)

                    new_key = ProviderAPIKey(
                        id=str(uuid.uuid4()),
                        provider_id=provider_id,
                        api_formats=normalized_formats,
                        api_key=encrypted_key,
                        name=key_data.get("name") or "Imported Key",
                        note=key_data.get("note"),
                        rate_multiplier=key_data.get("rate_multiplier", 1.0),
                        rate_multipliers=key_data.get("rate_multipliers"),
                        internal_priority=key_data.get("internal_priority", 50),
                        global_priority=key_data.get("global_priority"),
                        rpm_limit=key_data.get("rpm_limit"),
                        allowed_models=key_data.get("allowed_models"),
                        capabilities=key_data.get("capabilities"),
                        cache_ttl_minutes=key_data.get("cache_ttl_minutes", 5),
                        max_probe_interval_minutes=key_data.get("max_probe_interval_minutes", 32),
                        is_active=key_data.get("is_active", True),
                        health_by_format={},
                        circuit_breaker_by_format={},
                    )
                    db.add(new_key)
                    existing_key_values.add(plaintext_key)
                    stats["keys"]["created"] += 1

                # 导入 Models
                for model_data in prov_data.get("models", []):
                    global_model_name = model_data.get("global_model_name")
                    if not global_model_name:
                        stats["errors"].append(
                            f"跳过无 global_model_name 的模型 (Provider: {prov_data['name']})"
                        )
                        continue

                    global_model_id = global_model_map.get(global_model_name)
                    if not global_model_id:
                        # 尝试从数据库查找
                        existing_gm = (
                            db.query(GlobalModel)
                            .filter(GlobalModel.name == global_model_name)
                            .first()
                        )
                        if existing_gm:
                            global_model_id = existing_gm.id
                        else:
                            stats["errors"].append(
                                f"GlobalModel '{global_model_name}' 不存在，跳过模型"
                            )
                            continue

                    existing_model = (
                        db.query(Model)
                        .filter(
                            Model.provider_id == provider_id,
                            Model.provider_model_name == model_data["provider_model_name"],
                        )
                        .first()
                    )

                    if existing_model:
                        if merge_mode == "skip":
                            stats["models"]["skipped"] += 1
                        elif merge_mode == "error":
                            raise InvalidRequestException(
                                f"Model '{model_data['provider_model_name']}' 已存在于 Provider '{prov_data['name']}'"
                            )
                        elif merge_mode == "overwrite":
                            existing_model.global_model_id = global_model_id
                            existing_model.provider_model_mappings = model_data.get(
                                "provider_model_mappings"
                            )
                            existing_model.price_per_request = model_data.get(
                                "price_per_request"
                            )
                            existing_model.tiered_pricing = model_data.get(
                                "tiered_pricing"
                            )
                            existing_model.supports_vision = model_data.get(
                                "supports_vision"
                            )
                            existing_model.supports_function_calling = model_data.get(
                                "supports_function_calling"
                            )
                            existing_model.supports_streaming = model_data.get(
                                "supports_streaming"
                            )
                            existing_model.supports_extended_thinking = model_data.get(
                                "supports_extended_thinking"
                            )
                            existing_model.supports_image_generation = model_data.get(
                                "supports_image_generation"
                            )
                            existing_model.is_active = model_data.get("is_active", True)
                            existing_model.config = model_data.get("config")
                            existing_model.updated_at = datetime.now(timezone.utc)
                            stats["models"]["updated"] += 1
                    else:
                        new_model = Model(
                            id=str(uuid.uuid4()),
                            provider_id=provider_id,
                            global_model_id=global_model_id,
                            provider_model_name=model_data["provider_model_name"],
                            provider_model_mappings=model_data.get(
                                "provider_model_mappings"
                            ),
                            price_per_request=model_data.get("price_per_request"),
                            tiered_pricing=model_data.get("tiered_pricing"),
                            supports_vision=model_data.get("supports_vision"),
                            supports_function_calling=model_data.get(
                                "supports_function_calling"
                            ),
                            supports_streaming=model_data.get("supports_streaming"),
                            supports_extended_thinking=model_data.get(
                                "supports_extended_thinking"
                            ),
                            supports_image_generation=model_data.get(
                                "supports_image_generation"
                            ),
                            is_active=model_data.get("is_active", True),
                            config=model_data.get("config"),
                        )
                        db.add(new_model)
                        stats["models"]["created"] += 1

            db.commit()

            # 失效缓存
            from src.services.cache.invalidation import get_cache_invalidation_service

            cache_service = get_cache_invalidation_service()
            cache_service.clear_all_caches()

            return {
                "message": "配置导入成功",
                "stats": stats,
            }

        except InvalidRequestException:
            db.rollback()
            raise
        except Exception as e:
            db.rollback()
            raise InvalidRequestException(f"导入失败: {str(e)}")


class AdminExportUsersAdapter(AdminApiAdapter):
    async def handle(self, context):  # type: ignore[override]
        """导出用户数据（保留加密数据，排除管理员）"""
        from datetime import datetime, timezone

        from src.core.enums import UserRole
        from src.models.database import ApiKey, User

        db = context.db

        def _serialize_api_key(key: ApiKey, include_is_standalone: bool = False) -> dict:
            """序列化 API Key 为导出格式"""
            data = {
                "key_hash": key.key_hash,
                "key_encrypted": key.key_encrypted,
                "name": key.name,
                "balance_used_usd": key.balance_used_usd,
                "current_balance_usd": key.current_balance_usd,
                "allowed_providers": key.allowed_providers,
                "allowed_api_formats": key.allowed_api_formats,
                "allowed_models": key.allowed_models,
                "rate_limit": key.rate_limit,
                "concurrent_limit": key.concurrent_limit,
                "force_capabilities": key.force_capabilities,
                "is_active": key.is_active,
                "expires_at": key.expires_at.isoformat() if key.expires_at else None,
                "auto_delete_on_expiry": key.auto_delete_on_expiry,
                "total_requests": key.total_requests,
                "total_cost_usd": key.total_cost_usd,
            }
            if include_is_standalone:
                data["is_standalone"] = key.is_standalone
            return data

        # 导出 Users（排除管理员）
        users = db.query(User).filter(
            User.is_deleted.is_(False),
            User.role != UserRole.ADMIN
        ).all()
        users_data = []
        for user in users:
            # 导出用户的 API Keys（排除独立余额Key，独立Key单独导出）
            api_keys = db.query(ApiKey).filter(
                ApiKey.user_id == user.id,
                ApiKey.is_standalone.is_(False)
            ).all()
            api_keys_data = [_serialize_api_key(key, include_is_standalone=True) for key in api_keys]

            users_data.append(
                {
                    "email": user.email,
                    "username": user.username,
                    "password_hash": user.password_hash,
                    "role": user.role.value if user.role else "user",
                    "allowed_providers": user.allowed_providers,
                    "allowed_api_formats": user.allowed_api_formats,
                    "allowed_models": user.allowed_models,
                    "model_capability_settings": user.model_capability_settings,
                    "quota_usd": user.quota_usd,
                    "used_usd": user.used_usd,
                    "total_usd": user.total_usd,
                    "is_active": user.is_active,
                    "api_keys": api_keys_data,
                }
            )

        # 导出独立余额 Keys（管理员创建的，不属于普通用户）
        standalone_keys = db.query(ApiKey).filter(ApiKey.is_standalone.is_(True)).all()
        standalone_keys_data = [_serialize_api_key(key) for key in standalone_keys]

        return {
            "version": "1.1",
            "exported_at": datetime.now(timezone.utc).isoformat(),
            "users": users_data,
            "standalone_keys": standalone_keys_data,
        }


class AdminImportUsersAdapter(AdminApiAdapter):
    async def handle(self, context):  # type: ignore[override]
        """导入用户数据"""
        import uuid
        from datetime import datetime, timezone

        from src.core.enums import UserRole
        from src.models.database import ApiKey, User

        # 检查请求体大小
        if context.raw_body and len(context.raw_body) > MAX_IMPORT_SIZE:
            raise InvalidRequestException("请求体大小不能超过 10MB")

        db = context.db
        payload = context.ensure_json_body()

        # 获取导入选项
        merge_mode = payload.get("merge_mode", "skip")  # skip, overwrite, error
        users_data = payload.get("users", [])
        standalone_keys_data = payload.get("standalone_keys", [])

        stats = {
            "users": {"created": 0, "updated": 0, "skipped": 0},
            "api_keys": {"created": 0, "skipped": 0},
            "standalone_keys": {"created": 0, "skipped": 0},
            "errors": [],
        }

        def _create_api_key_from_data(
            key_data: dict,
            owner_id: str,
            is_standalone: bool = False,
        ) -> tuple[ApiKey | None, str]:
            """从导入数据创建 ApiKey 对象

            Returns:
                (ApiKey, "created"): 成功创建
                (None, "skipped"): key 已存在，跳过
                (None, "invalid"): 数据无效，跳过
            """
            key_hash = key_data.get("key_hash", "").strip()
            if not key_hash:
                return None, "invalid"

            # 检查是否已存在
            existing = db.query(ApiKey).filter(ApiKey.key_hash == key_hash).first()
            if existing:
                return None, "skipped"

            # 解析 expires_at
            expires_at = None
            if key_data.get("expires_at"):
                try:
                    expires_at = datetime.fromisoformat(key_data["expires_at"])
                except ValueError:
                    stats["errors"].append(
                        f"API Key '{key_data.get('name', key_hash[:8])}' 的 expires_at 格式无效"
                    )

            return ApiKey(
                id=str(uuid.uuid4()),
                user_id=owner_id,
                key_hash=key_hash,
                key_encrypted=key_data.get("key_encrypted"),
                name=key_data.get("name"),
                is_standalone=is_standalone or key_data.get("is_standalone", False),
                balance_used_usd=key_data.get("balance_used_usd", 0.0),
                current_balance_usd=key_data.get("current_balance_usd"),
                allowed_providers=key_data.get("allowed_providers"),
                allowed_api_formats=key_data.get("allowed_api_formats"),
                allowed_models=key_data.get("allowed_models"),
                rate_limit=key_data.get("rate_limit"),
                concurrent_limit=key_data.get("concurrent_limit", 5),
                force_capabilities=key_data.get("force_capabilities"),
                is_active=key_data.get("is_active", True),
                expires_at=expires_at,
                auto_delete_on_expiry=key_data.get("auto_delete_on_expiry", False),
                total_requests=key_data.get("total_requests", 0),
                total_cost_usd=key_data.get("total_cost_usd", 0.0),
            ), "created"

        try:
            for user_data in users_data:
                # 跳过管理员角色的导入（不区分大小写）
                role_str = str(user_data.get("role", "")).lower()
                if role_str == "admin":
                    stats["errors"].append(f"跳过管理员用户: {user_data.get('email')}")
                    stats["users"]["skipped"] += 1
                    continue

                existing_user = (
                    db.query(User).filter(User.email == user_data["email"]).first()
                )

                if existing_user:
                    user_id = existing_user.id
                    if merge_mode == "skip":
                        stats["users"]["skipped"] += 1
                    elif merge_mode == "error":
                        raise InvalidRequestException(
                            f"用户 '{user_data['email']}' 已存在"
                        )
                    elif merge_mode == "overwrite":
                        # 更新现有用户
                        existing_user.username = user_data.get(
                            "username", existing_user.username
                        )
                        if user_data.get("password_hash"):
                            existing_user.password_hash = user_data["password_hash"]
                        if user_data.get("role"):
                            existing_user.role = UserRole(user_data["role"])
                        existing_user.allowed_providers = user_data.get("allowed_providers")
                        existing_user.allowed_api_formats = user_data.get("allowed_api_formats")
                        existing_user.allowed_models = user_data.get("allowed_models")
                        existing_user.model_capability_settings = user_data.get(
                            "model_capability_settings"
                        )
                        existing_user.quota_usd = user_data.get("quota_usd")
                        existing_user.used_usd = user_data.get("used_usd", 0.0)
                        existing_user.total_usd = user_data.get("total_usd", 0.0)
                        existing_user.is_active = user_data.get("is_active", True)
                        existing_user.updated_at = datetime.now(timezone.utc)
                        stats["users"]["updated"] += 1
                else:
                    # 创建新用户
                    role = UserRole.USER
                    if user_data.get("role"):
                        role = UserRole(user_data["role"])

                    new_user = User(
                        id=str(uuid.uuid4()),
                        email=user_data["email"],
                        username=user_data.get("username", user_data["email"].split("@")[0]),
                        password_hash=user_data.get("password_hash", ""),
                        role=role,
                        allowed_providers=user_data.get("allowed_providers"),
                        allowed_api_formats=user_data.get("allowed_api_formats"),
                        allowed_models=user_data.get("allowed_models"),
                        model_capability_settings=user_data.get("model_capability_settings"),
                        quota_usd=user_data.get("quota_usd"),
                        used_usd=user_data.get("used_usd", 0.0),
                        total_usd=user_data.get("total_usd", 0.0),
                        is_active=user_data.get("is_active", True),
                    )
                    db.add(new_user)
                    db.flush()
                    user_id = new_user.id
                    stats["users"]["created"] += 1

                # 导入 API Keys
                for key_data in user_data.get("api_keys", []):
                    new_key, status = _create_api_key_from_data(key_data, user_id)
                    if new_key:
                        db.add(new_key)
                        stats["api_keys"]["created"] += 1
                    elif status == "skipped":
                        stats["api_keys"]["skipped"] += 1
                    # invalid 数据不计入统计

            # 导入独立余额 Keys（需要找一个管理员用户作为 owner）
            if standalone_keys_data:
                # 查找一个管理员用户作为独立Key的owner
                admin_user = db.query(User).filter(User.role == UserRole.ADMIN).first()
                if not admin_user:
                    stats["errors"].append("无法导入独立余额Key: 系统中没有管理员用户")
                else:
                    for key_data in standalone_keys_data:
                        new_key, status = _create_api_key_from_data(
                            key_data, admin_user.id, is_standalone=True
                        )
                        if new_key:
                            db.add(new_key)
                            stats["standalone_keys"]["created"] += 1
                        elif status == "skipped":
                            stats["standalone_keys"]["skipped"] += 1
                        # invalid 数据不计入统计

            db.commit()

            return {
                "message": "用户数据导入成功",
                "stats": stats,
            }

        except InvalidRequestException:
            db.rollback()
            raise
        except Exception as e:
            db.rollback()
            raise InvalidRequestException(f"导入失败: {str(e)}")


class AdminTestSmtpAdapter(AdminApiAdapter):
    async def handle(self, context):  # type: ignore[override]
        """测试 SMTP 连接"""
        from src.core.crypto import crypto_service
        from src.services.system.config import SystemConfigService
        from src.services.email.email_sender import EmailSenderService

        db = context.db
        payload = context.ensure_json_body() or {}

        # 获取密码：优先使用前端传入的明文密码，否则从数据库获取并解密
        smtp_password = payload.get("smtp_password")
        if not smtp_password:
            encrypted_password = SystemConfigService.get_config(db, "smtp_password")
            if encrypted_password:
                try:
                    smtp_password = crypto_service.decrypt(encrypted_password, silent=True)
                except Exception:
                    # 解密失败，可能是旧的未加密密码
                    smtp_password = encrypted_password

        # 前端可传入未保存的配置，优先使用前端值，否则回退数据库
        config = {
            "smtp_host": payload.get("smtp_host") or SystemConfigService.get_config(db, "smtp_host"),
            "smtp_port": payload.get("smtp_port") or SystemConfigService.get_config(db, "smtp_port", default=587),
            "smtp_user": payload.get("smtp_user") or SystemConfigService.get_config(db, "smtp_user"),
            "smtp_password": smtp_password,
            "smtp_use_tls": payload.get("smtp_use_tls")
            if payload.get("smtp_use_tls") is not None
            else SystemConfigService.get_config(db, "smtp_use_tls", default=True),
            "smtp_use_ssl": payload.get("smtp_use_ssl")
            if payload.get("smtp_use_ssl") is not None
            else SystemConfigService.get_config(db, "smtp_use_ssl", default=False),
            "smtp_from_email": payload.get("smtp_from_email")
            or SystemConfigService.get_config(db, "smtp_from_email"),
            "smtp_from_name": payload.get("smtp_from_name")
            or SystemConfigService.get_config(db, "smtp_from_name", default="Aether"),
        }

        # 验证必要配置
        missing_fields = [
            field for field in ["smtp_host", "smtp_user", "smtp_password", "smtp_from_email"] if not config.get(field)
        ]
        if missing_fields:
            return {
                "success": False,
                "message": f"SMTP 配置不完整，请检查 {', '.join(missing_fields)}"
            }

        # 测试连接
        try:
            success, error_msg = await EmailSenderService.test_smtp_connection(
                db=db, override_config=config
            )

            if success:
                return {
                    "success": True,
                    "message": "SMTP 连接测试成功"
                }
            else:
                return {
                    "success": False,
                    "message": error_msg
                }
        except Exception as e:
            return {
                "success": False,
                "message": str(e)
            }


# -------- 邮件模板适配器 --------


class AdminGetEmailTemplatesAdapter(AdminApiAdapter):
    """获取所有邮件模板"""

    async def handle(self, context):  # type: ignore[override]
        db = context.db
        templates = []

        for template_type, type_info in EmailTemplate.TEMPLATE_TYPES.items():
            # 获取自定义模板或默认模板
            template = EmailTemplate.get_template(db, template_type)
            default_template = EmailTemplate.get_default_template(template_type)

            # 检查是否使用了自定义模板
            is_custom = (
                template["subject"] != default_template["subject"]
                or template["html"] != default_template["html"]
            )

            templates.append(
                {
                    "type": template_type,
                    "name": type_info["name"],
                    "variables": type_info["variables"],
                    "subject": template["subject"],
                    "html": template["html"],
                    "is_custom": is_custom,
                }
            )

        return {"templates": templates}


@dataclass
class AdminGetEmailTemplateAdapter(AdminApiAdapter):
    """获取指定类型的邮件模板"""

    template_type: str

    async def handle(self, context):  # type: ignore[override]
        # 验证模板类型
        if self.template_type not in EmailTemplate.TEMPLATE_TYPES:
            raise NotFoundException(f"模板类型 '{self.template_type}' 不存在")

        db = context.db
        type_info = EmailTemplate.TEMPLATE_TYPES[self.template_type]
        template = EmailTemplate.get_template(db, self.template_type)
        default_template = EmailTemplate.get_default_template(self.template_type)

        is_custom = (
            template["subject"] != default_template["subject"]
            or template["html"] != default_template["html"]
        )

        return {
            "type": self.template_type,
            "name": type_info["name"],
            "variables": type_info["variables"],
            "subject": template["subject"],
            "html": template["html"],
            "is_custom": is_custom,
            "default_subject": default_template["subject"],
            "default_html": default_template["html"],
        }


@dataclass
class AdminUpdateEmailTemplateAdapter(AdminApiAdapter):
    """更新邮件模板"""

    template_type: str

    async def handle(self, context):  # type: ignore[override]
        # 验证模板类型
        if self.template_type not in EmailTemplate.TEMPLATE_TYPES:
            raise NotFoundException(f"模板类型 '{self.template_type}' 不存在")

        db = context.db
        payload = context.ensure_json_body()

        subject = payload.get("subject")
        html = payload.get("html")

        # 至少需要提供一个字段
        if subject is None and html is None:
            raise InvalidRequestException("请提供 subject 或 html")

        # 保存模板
        subject_key = f"email_template_{self.template_type}_subject"
        html_key = f"email_template_{self.template_type}_html"

        if subject is not None:
            if subject:
                SystemConfigService.set_config(db, subject_key, subject)
            else:
                # 空字符串表示删除自定义值，恢复默认
                SystemConfigService.delete_config(db, subject_key)

        if html is not None:
            if html:
                SystemConfigService.set_config(db, html_key, html)
            else:
                SystemConfigService.delete_config(db, html_key)

        return {"message": "模板保存成功"}


@dataclass
class AdminPreviewEmailTemplateAdapter(AdminApiAdapter):
    """预览邮件模板"""

    template_type: str

    async def handle(self, context):  # type: ignore[override]
        # 验证模板类型
        if self.template_type not in EmailTemplate.TEMPLATE_TYPES:
            raise NotFoundException(f"模板类型 '{self.template_type}' 不存在")

        db = context.db
        payload = context.ensure_json_body() or {}

        # 获取模板 HTML（优先使用请求体中的，否则使用数据库中的）
        html = payload.get("html")
        if not html:
            template = EmailTemplate.get_template(db, self.template_type)
            html = template["html"]

        # 获取预览变量
        type_info = EmailTemplate.TEMPLATE_TYPES[self.template_type]

        # 构建预览变量，使用请求中的值或默认示例值
        preview_variables = {}
        default_values = {
            "app_name": SystemConfigService.get_config(db, "email_app_name")
            or SystemConfigService.get_config(db, "smtp_from_name", default="Aether"),
            "code": "123456",
            "expire_minutes": "30",
            "email": "example@example.com",
            "reset_link": "https://example.com/reset?token=abc123",
        }

        for var in type_info["variables"]:
            preview_variables[var] = payload.get(var, default_values.get(var, f"{{{{{var}}}}}"))

        # 渲染模板
        rendered_html = EmailTemplate.render_template(html, preview_variables)

        return {
            "html": rendered_html,
            "variables": preview_variables,
        }


@dataclass
class AdminResetEmailTemplateAdapter(AdminApiAdapter):
    """重置邮件模板为默认值"""

    template_type: str

    async def handle(self, context):  # type: ignore[override]
        # 验证模板类型
        if self.template_type not in EmailTemplate.TEMPLATE_TYPES:
            raise NotFoundException(f"模板类型 '{self.template_type}' 不存在")

        db = context.db

        # 删除自定义模板
        subject_key = f"email_template_{self.template_type}_subject"
        html_key = f"email_template_{self.template_type}_html"

        SystemConfigService.delete_config(db, subject_key)
        SystemConfigService.delete_config(db, html_key)

        # 返回默认模板
        default_template = EmailTemplate.get_default_template(self.template_type)
        type_info = EmailTemplate.TEMPLATE_TYPES[self.template_type]

        return {
            "message": "模板已重置为默认值",
            "template": {
                "type": self.template_type,
                "name": type_info["name"],
                "subject": default_template["subject"],
                "html": default_template["html"],
            },
        }
