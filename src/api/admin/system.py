"""系统设置API端点。"""

from __future__ import annotations

import copy
import json
from dataclasses import dataclass
from typing import Any

from fastapi import APIRouter, Depends, HTTPException, Request, status
from pydantic import ValidationError
from sqlalchemy import case, func
from sqlalchemy.orm import Session

from src.api.base.admin_adapter import AdminApiAdapter
from src.api.base.context import ApiRequestContext
from src.api.base.pipeline import ApiRequestPipeline
from src.config.constants import CacheTTL
from src.core.exceptions import InvalidRequestException, NotFoundException, translate_pydantic_error
from src.core.logger import logger
from src.database import get_db
from src.models.api import SystemSettingsRequest, SystemSettingsResponse
from src.models.database import ApiKey, Provider, Usage, User
from src.services.email.email_template import EmailTemplate
from src.services.provider_ops.types import SENSITIVE_CREDENTIAL_FIELDS
from src.services.system.config import SystemConfigService
from src.services.wallet import WalletService
from src.utils.cache_decorator import cache_result

router = APIRouter(prefix="/api/admin/system", tags=["Admin - System"])

CONFIG_EXPORT_VERSION = "2.2"
CONFIG_SUPPORTED_VERSIONS = ("2.0", "2.1", "2.2")
MAX_IMPORT_SIZE = 10 * 1024 * 1024  # 10MB


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
async def get_system_version() -> Any:
    """
    获取系统版本信息

    获取当前系统的版本号。优先从 git describe 获取，回退到静态版本文件。

    **返回字段**:
    - `version`: 版本号字符串
    """
    return {"version": _get_current_version()}


@router.get("/check-update")
async def check_update() -> Any:
    """
    检查系统更新

    从 GitHub Tags 获取最新版本并与当前版本对比。
    更新内容从 annotated tag 的 message 中获取。

    **返回字段**:
    - `current_version`: 当前版本号
    - `latest_version`: 最新版本号
    - `has_update`: 是否有更新可用
    - `release_url`: 最新版本的 GitHub 页面链接
    - `release_notes`: 更新日志 (Markdown 格式，来自 tag message)
    - `published_at`: 发布时间 (ISO 8601 格式)
    """
    import httpx

    from src.clients.http_client import HTTPClientPool

    current_version = _get_current_version()
    github_repo = "Aethersailor/Aether"
    github_tags_url = f"https://api.github.com/repos/{github_repo}/tags"

    def _make_empty_response(error: str | None = None) -> None:
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
            # 获取 tags 列表
            response = await client.get(
                github_tags_url,
                headers={
                    "Accept": "application/vnd.github.v3+json",
                    "User-Agent": f"Aether/{current_version}",
                },
                params={"per_page": 20},
            )

            if response.status_code != 200:
                return _make_empty_response(f"GitHub API 返回错误: {response.status_code}")

            tags = response.json()
            if not tags:
                return _make_empty_response()

            # 筛选版本号格式的 tags 并排序
            valid_tags = []
            for tag in tags:
                tag_name = tag.get("name", "")
                if tag_name.startswith("v") or (tag_name and tag_name[0].isdigit()):
                    valid_tags.append((tag, _parse_version(tag_name)))

            if not valid_tags:
                return _make_empty_response()

            # 按版本号排序，取最大的
            valid_tags.sort(key=lambda x: x[1], reverse=True)
            latest_tag_info = valid_tags[0][0]
            latest_tag_name = latest_tag_info.get("name", "")
            latest_version = latest_tag_name.lstrip("v")

            current_tuple = _parse_version(current_version)
            latest_tuple = _parse_version(latest_version)
            has_update = latest_tuple > current_tuple

            # 获取 tag 的详细信息（包含 message 和时间）
            release_notes = None
            published_at = None

            # 获取 tag 对应的 commit sha
            tag_commit_sha = latest_tag_info.get("commit", {}).get("sha")
            if tag_commit_sha:
                # 尝试获取 annotated tag 的信息
                tag_ref_url = (
                    f"https://api.github.com/repos/{github_repo}/git/refs/tags/{latest_tag_name}"
                )
                ref_response = await client.get(
                    tag_ref_url,
                    headers={
                        "Accept": "application/vnd.github.v3+json",
                        "User-Agent": f"Aether/{current_version}",
                    },
                )

                if ref_response.status_code == 200:
                    ref_data = ref_response.json()
                    # 检查是否是 annotated tag（type 为 "tag"）
                    if ref_data.get("object", {}).get("type") == "tag":
                        tag_object_url = ref_data.get("object", {}).get("url")
                        if tag_object_url:
                            tag_obj_response = await client.get(
                                tag_object_url,
                                headers={
                                    "Accept": "application/vnd.github.v3+json",
                                    "User-Agent": f"Aether/{current_version}",
                                },
                            )
                            if tag_obj_response.status_code == 200:
                                tag_obj_data = tag_obj_response.json()
                                release_notes = tag_obj_data.get("message")
                                # 获取 tagger 的时间
                                tagger = tag_obj_data.get("tagger", {})
                                published_at = tagger.get("date")

                # 如果没有获取到时间，从 commit 获取
                if not published_at:
                    commit_url = (
                        f"https://api.github.com/repos/{github_repo}/commits/{tag_commit_sha}"
                    )
                    commit_response = await client.get(
                        commit_url,
                        headers={
                            "Accept": "application/vnd.github.v3+json",
                            "User-Agent": f"Aether/{current_version}",
                        },
                    )
                    if commit_response.status_code == 200:
                        commit_data = commit_response.json()
                        published_at = (
                            commit_data.get("commit", {}).get("committer", {}).get("date")
                        )

            return {
                "current_version": current_version,
                "latest_version": latest_version,
                "has_update": has_update,
                "release_url": f"https://github.com/{github_repo}/releases/tag/{latest_tag_name}",
                "release_notes": release_notes,
                "published_at": published_at,
                "error": None,
            }

    except httpx.TimeoutException:
        return _make_empty_response("检查更新超时")
    except Exception as e:
        return _make_empty_response(f"检查更新失败: {str(e)}")


pipeline = ApiRequestPipeline()


@router.get("/settings")
async def get_system_settings(request: Request, db: Session = Depends(get_db)) -> Any:
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
async def update_system_settings(http_request: Request, db: Session = Depends(get_db)) -> None:
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
async def get_all_system_configs(request: Request, db: Session = Depends(get_db)) -> Any:
    """
    获取所有系统配置

    获取系统中所有的配置项。需要管理员权限。

    **返回字段**:
    - 配置项的键值对字典
    """

    adapter = AdminGetAllConfigsAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/configs/{key}")
async def get_system_config(key: str, request: Request, db: Session = Depends(get_db)) -> Any:
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
) -> Any:
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
async def delete_system_config(key: str, request: Request, db: Session = Depends(get_db)) -> None:
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
async def get_system_stats(request: Request, db: Session = Depends(get_db)) -> Any:
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
async def trigger_cleanup(request: Request, db: Session = Depends(get_db)) -> Any:
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
async def get_api_formats(request: Request, db: Session = Depends(get_db)) -> Any:
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
async def export_config(request: Request, db: Session = Depends(get_db)) -> Any:
    """导出提供商和模型配置（管理员）"""
    adapter = AdminExportConfigAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.post("/config/import")
async def import_config(request: Request, db: Session = Depends(get_db)) -> Any:
    """导入提供商和模型配置（管理员）"""
    adapter = AdminImportConfigAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/users/export")
async def export_users(request: Request, db: Session = Depends(get_db)) -> Any:
    """导出用户数据（管理员）"""
    adapter = AdminExportUsersAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.post("/users/import")
async def import_users(request: Request, db: Session = Depends(get_db)) -> Any:
    """导入用户数据（管理员）"""
    adapter = AdminImportUsersAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.post("/smtp/test")
async def test_smtp(request: Request, db: Session = Depends(get_db)) -> Any:
    """测试 SMTP 连接（管理员）"""
    adapter = AdminTestSmtpAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


# -------- 邮件模板 API --------


@router.get("/email/templates")
async def get_email_templates(request: Request, db: Session = Depends(get_db)) -> Any:
    """获取所有邮件模板（管理员）"""
    adapter = AdminGetEmailTemplatesAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/email/templates/{template_type}")
async def get_email_template(
    template_type: str, request: Request, db: Session = Depends(get_db)
) -> Any:
    """获取指定类型的邮件模板（管理员）"""
    adapter = AdminGetEmailTemplateAdapter(template_type=template_type)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.put("/email/templates/{template_type}")
async def update_email_template(
    template_type: str, request: Request, db: Session = Depends(get_db)
) -> Any:
    """更新邮件模板（管理员）"""
    adapter = AdminUpdateEmailTemplateAdapter(template_type=template_type)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.post("/email/templates/{template_type}/preview")
async def preview_email_template(
    template_type: str, request: Request, db: Session = Depends(get_db)
) -> Any:
    """预览邮件模板（管理员）"""
    adapter = AdminPreviewEmailTemplateAdapter(template_type=template_type)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.post("/email/templates/{template_type}/reset")
async def reset_email_template(
    template_type: str, request: Request, db: Session = Depends(get_db)
) -> Any:
    """重置邮件模板为默认值（管理员）"""
    adapter = AdminResetEmailTemplateAdapter(template_type=template_type)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


# -------- 数据清空 API --------


@router.post("/purge/config")
async def purge_config(request: Request, db: Session = Depends(get_db)) -> Any:
    """清空所有提供商配置（管理员）"""
    adapter = AdminPurgeConfigAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.post("/purge/users")
async def purge_users(request: Request, db: Session = Depends(get_db)) -> Any:
    """清空所有非管理员用户（管理员）"""
    adapter = AdminPurgeUsersAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.post("/purge/usage")
async def purge_usage(request: Request, db: Session = Depends(get_db)) -> Any:
    """清空全部使用记录（管理员）"""
    adapter = AdminPurgeUsageAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.post("/purge/audit-logs")
async def purge_audit_logs(request: Request, db: Session = Depends(get_db)) -> Any:
    """清空全部审计日志（管理员）"""
    adapter = AdminPurgeAuditLogsAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.post("/purge/request-bodies")
async def purge_request_bodies(request: Request, db: Session = Depends(get_db)) -> Any:
    """清空全部请求体（管理员）"""
    adapter = AdminPurgeRequestBodiesAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.post("/purge/stats")
async def purge_stats(request: Request, db: Session = Depends(get_db)) -> Any:
    """清空全部聚合统计数据（管理员）"""
    adapter = AdminPurgeStatsAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


# -------- 系统设置适配器 --------


class AdminGetSystemSettingsAdapter(AdminApiAdapter):
    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
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
    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
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
    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        return SystemConfigService.get_all_configs(context.db)


@dataclass
class AdminGetSystemConfigAdapter(AdminApiAdapter):
    key: str

    # 敏感配置项，不返回实际值
    SENSITIVE_KEYS = {"smtp_password"}

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        value = SystemConfigService.get_config(context.db, self.key)
        if value is None and self.key not in SystemConfigService.DEFAULT_CONFIGS:
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

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
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

        # 如果更新的是签到任务时间，动态更新调度器
        if self.key == "provider_checkin_time" and value:
            try:
                from src.services.system.maintenance_scheduler import get_maintenance_scheduler

                scheduler = get_maintenance_scheduler()
                scheduler.update_checkin_time(value)
            except Exception as e:
                logger.warning(f"更新签到任务时间失败: {e}")

        # 如果更新的是调度模式或优先级模式，立即更新当前 Worker 的 Scheduler 单例
        if self.key in ("scheduling_mode", "provider_priority_mode"):
            try:
                from src.clients.redis_client import get_redis_client_sync
                from src.services.scheduling.aware_scheduler import get_cache_aware_scheduler

                redis_client = get_redis_client_sync()
                # 从数据库读取两个调度配置的最新值，确保一致性
                priority_mode = SystemConfigService.get_config(
                    context.db,
                    "provider_priority_mode",
                    "provider",
                )
                scheduling_mode = SystemConfigService.get_config(
                    context.db,
                    "scheduling_mode",
                    "cache_affinity",
                )
                await get_cache_aware_scheduler(
                    redis_client,
                    priority_mode=priority_mode,
                    scheduling_mode=scheduling_mode,
                )
                logger.info(
                    "[AdminSetSystemConfig] 已同步更新 Scheduler: "
                    "priority_mode={}, scheduling_mode={}",
                    priority_mode,
                    scheduling_mode,
                )
            except Exception as e:
                logger.warning("同步更新 Scheduler 失败: {}", e)

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

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        deleted = SystemConfigService.delete_config(context.db, self.key)
        if not deleted:
            raise NotFoundException(f"配置项 '{self.key}' 不存在")
        return {"message": f"配置项 '{self.key}' 已删除"}


class AdminSystemStatsAdapter(AdminApiAdapter):
    @cache_result(
        key_prefix="admin:system:stats",
        ttl=CacheTTL.DASHBOARD_STATS,
        user_specific=False,
    )
    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        db = context.db
        user_stats = db.query(
            func.count(User.id).label("total"),
            func.sum(case((User.is_active.is_(True), 1), else_=0)).label("active"),
        ).first()
        provider_stats = db.query(
            func.count(Provider.id).label("total"),
            func.sum(case((Provider.is_active.is_(True), 1), else_=0)).label("active"),
        ).first()
        total_api_keys = int(db.query(func.count(ApiKey.id)).scalar() or 0)
        total_requests = int(db.query(func.count(Usage.id)).scalar() or 0)
        total_users = int(user_stats.total or 0) if user_stats else 0
        active_users = int(user_stats.active or 0) if user_stats else 0
        total_providers = int(provider_stats.total or 0) if provider_stats else 0
        active_providers = int(provider_stats.active or 0) if provider_stats else 0

        return {
            "users": {"total": total_users, "active": active_users},
            "providers": {"total": total_providers, "active": active_providers},
            "api_keys": total_api_keys,
            "requests": total_requests,
        }


class AdminTriggerCleanupAdapter(AdminApiAdapter):
    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        """手动触发清理任务"""
        from datetime import datetime, timezone

        from src.services.system.maintenance_scheduler import get_maintenance_scheduler

        db = context.db

        # 获取清理前的统计信息
        total_before = int(db.query(func.count(Usage.id)).scalar() or 0)
        with_body_before = (
            db.query(func.count(Usage.id))
            .filter((Usage.request_body.isnot(None)) | (Usage.response_body.isnot(None)))
            .scalar()
            or 0
        )
        with_headers_before = (
            db.query(func.count(Usage.id))
            .filter((Usage.request_headers.isnot(None)) | (Usage.response_headers.isnot(None)))
            .scalar()
            or 0
        )

        # 触发清理
        maintenance_scheduler = get_maintenance_scheduler()
        await maintenance_scheduler._perform_cleanup()

        # 获取清理后的统计信息
        total_after = int(db.query(func.count(Usage.id)).scalar() or 0)
        with_body_after = (
            db.query(func.count(Usage.id))
            .filter((Usage.request_body.isnot(None)) | (Usage.response_body.isnot(None)))
            .scalar()
            or 0
        )
        with_headers_after = (
            db.query(func.count(Usage.id))
            .filter((Usage.request_headers.isnot(None)) | (Usage.response_headers.isnot(None)))
            .scalar()
            or 0
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
    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        """获取所有可用的API格式"""
        from src.core.api_format import list_endpoint_definitions

        _ = context  # 参数保留以符合接口规范

        def _label_for(sig: str) -> str:
            fam, kind = (sig.split(":", 1) + [""])[:2]
            fam_title = {"claude": "Claude", "openai": "OpenAI", "gemini": "Gemini"}.get(fam, fam)
            kind_title = {
                "chat": "Chat",
                "cli": "CLI",
                "compact": "Compact",
                "video": "Video",
                "image": "Image",
            }.get(kind, kind)
            return f"{fam_title} {kind_title}".strip()

        endpoint_defs = list_endpoint_definitions()
        preferred_order = [
            "openai:chat",
            "openai:cli",
            "openai:compact",
            "openai:video",
            "claude:chat",
            "claude:cli",
            "gemini:chat",
            "gemini:cli",
            "gemini:video",
        ]
        order_map = {key: i for i, key in enumerate(preferred_order)}
        endpoint_defs.sort(key=lambda d: order_map.get(d.signature_key, 999))

        formats = [
            {
                "value": d.signature_key,
                "label": _label_for(d.signature_key),
                "default_path": d.default_path,
                "aliases": list(d.aliases or []),
            }
            for d in endpoint_defs
        ]

        return {"formats": formats}


class AdminExportConfigAdapter(AdminApiAdapter):
    """导出提供商和模型配置"""

    # Provider Ops 中需要解密的敏感字段
    SENSITIVE_CREDENTIALS = SENSITIVE_CREDENTIAL_FIELDS

    @staticmethod
    def _normalize_api_formats(raw_formats: Any) -> list[str]:
        """规范化 api_formats 为 endpoint signature 列表。"""
        from src.core.api_format.signature import normalize_signature_key

        if not isinstance(raw_formats, list):
            return []

        normalized: list[str] = []
        seen: set[str] = set()
        for raw in raw_formats:
            if not isinstance(raw, str):
                continue
            value = raw.strip()
            if not value:
                continue
            try:
                fmt = normalize_signature_key(value)
            except Exception:
                continue
            if fmt in seen:
                continue
            seen.add(fmt)
            normalized.append(fmt)
        return normalized

    def _resolve_export_key_api_formats(
        self, raw_formats: Any, provider_endpoint_formats: list[str]
    ) -> list[str]:
        """导出 Key 时解析支持端点：

        - 优先使用 Key 自身 api_formats（规范化后）
        - 当 api_formats 为 None（历史语义：全支持）时回退为 Provider 端点列表
        - 当 api_formats 显式为空列表时保留空列表
        """
        normalized = self._normalize_api_formats(raw_formats)
        if normalized:
            return normalized

        if raw_formats is None:
            return list(provider_endpoint_formats)
        return []

    def _collect_provider_endpoint_formats(self, endpoints: list[Any]) -> list[str]:
        """收集 Provider 下所有 endpoint signature（去重后排序）。"""
        normalized: list[str] = []
        seen: set[str] = set()
        for ep in endpoints:
            raw = getattr(ep, "api_format", None)
            if hasattr(raw, "value"):
                raw = raw.value
            fmt_list = self._normalize_api_formats([raw])
            if not fmt_list:
                continue
            fmt = fmt_list[0]
            if fmt in seen:
                continue
            seen.add(fmt)
            normalized.append(fmt)
        return sorted(normalized)

    def _decrypt_provider_config(self, config: dict, crypto_service: Any) -> dict:
        """解密 Provider config 中的 provider_ops credentials"""
        if not config:
            return config

        decrypted_config = copy.deepcopy(config)

        # 解密 provider_ops.connector.credentials 中的敏感字段
        provider_ops = decrypted_config.get("provider_ops")
        if provider_ops and isinstance(provider_ops, dict):
            connector = provider_ops.get("connector")
            if connector and isinstance(connector, dict):
                credentials = connector.get("credentials")
                if credentials and isinstance(credentials, dict):
                    for field in self.SENSITIVE_CREDENTIALS:
                        if field in credentials and isinstance(credentials[field], str):
                            try:
                                credentials[field] = crypto_service.decrypt(credentials[field])
                            except Exception as e:
                                # 解密失败保持原值（可能本来就是明文）
                                logger.debug(f"解密 provider_ops credential '{field}' 失败: {e}")

        return decrypted_config

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        """导出提供商和模型配置（解密数据）"""
        from datetime import datetime, timezone

        from src.core.crypto import crypto_service
        from src.models.database import (
            GlobalModel,
            Model,
            ProviderAPIKey,
            ProviderEndpoint,
            ProxyNode,
        )

        db = context.db

        # 导出 GlobalModels
        global_models = db.query(GlobalModel).all()
        global_models_data = [gm.to_export_dict() for gm in global_models]

        # 预建 global_model_id -> name 映射，避免导出 Model 时 N+1 查询
        gm_name_map: dict[str, str] = {gm.id: gm.name for gm in global_models}

        # 导出 Providers 及其关联数据
        providers = db.query(Provider).all()
        providers_data = []
        for provider in providers:
            # 导出 Endpoints
            endpoints = (
                db.query(ProviderEndpoint).filter(ProviderEndpoint.provider_id == provider.id).all()
            )
            endpoints_data = [ep.to_export_dict() for ep in endpoints]
            provider_endpoint_formats = self._collect_provider_endpoint_formats(endpoints)

            # 导出 Provider Keys（按 provider_id 归属，包含 api_formats）
            keys = (
                db.query(ProviderAPIKey)
                .filter(ProviderAPIKey.provider_id == provider.id)
                .order_by(ProviderAPIKey.internal_priority.asc(), ProviderAPIKey.created_at.asc())
                .all()
            )
            keys_data = []
            for key in keys:
                key_data = key.to_export_dict()
                key_formats = self._resolve_export_key_api_formats(
                    key_data.get("api_formats"),
                    provider_endpoint_formats,
                )
                # 保持现有字段名 api_formats，并补充可读别名 supported_endpoints。
                key_data["api_formats"] = key_formats
                key_data["supported_endpoints"] = list(key_formats)
                # 解密 API Key
                try:
                    key_data["api_key"] = crypto_service.decrypt(key.api_key)
                except Exception:
                    logger.warning(
                        "API Key 解密失败: provider={}, key_id={}, api_formats={}",
                        provider.name,
                        key.id,
                        key.api_formats,
                    )
                    key_data["api_key"] = ""
                # 解密 auth_config（OAuth 等认证配置）
                # 导出值为解密后的 JSON 字符串（非 dict），导入时需按字符串重新加密
                if key.auth_config:
                    try:
                        key_data["auth_config"] = crypto_service.decrypt(key.auth_config)
                    except Exception:
                        logger.warning(
                            "auth_config 解密失败: provider={}, key_id={}",
                            provider.name,
                            key.id,
                        )
                        pass  # 解密失败则不导出 auth_config
                keys_data.append(key_data)

            # 导出 Provider Models
            # 注意：提供商模型（Model）必须关联全局模型（GlobalModel）才能参与路由
            # 导入时未关联 GlobalModel 的模型会被跳过，这是业务规则而非 bug
            models = db.query(Model).filter(Model.provider_id == provider.id).all()
            models_data = []
            for model in models:
                model_data = model.to_export_dict()
                # 追加关联的 GlobalModel 名称（导入时通过名称查找）
                model_data["global_model_name"] = gm_name_map.get(model.global_model_id)
                models_data.append(model_data)

            # 解密 Provider config 中的 credentials
            provider_data = provider.to_export_dict()
            provider_data["config"] = self._decrypt_provider_config(provider.config, crypto_service)
            provider_data["endpoints"] = endpoints_data
            provider_data["api_keys"] = keys_data
            provider_data["models"] = models_data
            providers_data.append(provider_data)

        # 导出 LDAP 配置
        from src.models.database import LDAPConfig

        ldap_config = db.query(LDAPConfig).first()
        ldap_data = None
        if ldap_config:
            # 解密绑定密码
            bind_password = ""
            if ldap_config.bind_password_encrypted:
                try:
                    bind_password = crypto_service.decrypt(ldap_config.bind_password_encrypted)
                except Exception as e:
                    logger.debug(f"解密 LDAP bind_password 失败: {e}")

            ldap_data = {
                "server_url": ldap_config.server_url,
                "bind_dn": ldap_config.bind_dn,
                "bind_password": bind_password,
                "base_dn": ldap_config.base_dn,
                "user_search_filter": ldap_config.user_search_filter,
                "username_attr": ldap_config.username_attr,
                "email_attr": ldap_config.email_attr,
                "display_name_attr": ldap_config.display_name_attr,
                "is_enabled": ldap_config.is_enabled,
                "is_exclusive": ldap_config.is_exclusive,
                "use_starttls": ldap_config.use_starttls,
                "connect_timeout": ldap_config.connect_timeout,
            }

        # 导出 SystemConfig 配置
        from src.models.database import SystemConfig

        # 敏感配置项需要解密导出
        SENSITIVE_CONFIG_KEYS = {"smtp_password"}
        system_configs = db.query(SystemConfig).all()
        system_configs_data = []
        for cfg in system_configs:
            cfg_data = {
                "key": cfg.key,
                "value": cfg.value,
                "description": cfg.description,
            }
            # 解密敏感配置
            if cfg.key in SENSITIVE_CONFIG_KEYS and cfg.value:
                try:
                    cfg_data["value"] = crypto_service.decrypt(cfg.value)
                except Exception as e:
                    logger.debug(f"解密 SystemConfig '{cfg.key}' 失败: {e}")
            system_configs_data.append(cfg_data)

        # 导出 OAuth Providers 配置
        from src.models.database import OAuthProvider

        oauth_providers = db.query(OAuthProvider).all()
        oauth_data = []
        for oauth in oauth_providers:
            # 解密 client secret
            client_secret = ""
            if oauth.client_secret_encrypted:
                try:
                    client_secret = crypto_service.decrypt(oauth.client_secret_encrypted)
                except Exception as e:
                    logger.debug(f"解密 OAuth '{oauth.provider_type}' client_secret 失败: {e}")

            oauth_data.append(
                {
                    "provider_type": oauth.provider_type,
                    "display_name": oauth.display_name,
                    "client_id": oauth.client_id,
                    "client_secret": client_secret,
                    "authorization_url_override": oauth.authorization_url_override,
                    "token_url_override": oauth.token_url_override,
                    "userinfo_url_override": oauth.userinfo_url_override,
                    "scopes": oauth.scopes,
                    "redirect_uri": oauth.redirect_uri,
                    "frontend_callback_url": oauth.frontend_callback_url,
                    "attribute_mapping": oauth.attribute_mapping,
                    "extra_config": oauth.extra_config,
                    "is_enabled": oauth.is_enabled,
                }
            )

        # 导出 ProxyNode（手动节点 + 隧道节点，不含运行时状态）
        proxy_nodes = db.query(ProxyNode).all()
        proxy_nodes_data = []
        for node in proxy_nodes:
            proxy_nodes_data.append(
                {
                    "id": node.id,
                    "name": node.name,
                    "ip": node.ip,
                    "port": node.port,
                    "region": node.region,
                    "is_manual": node.is_manual,
                    "proxy_url": node.proxy_url,
                    "proxy_username": node.proxy_username,
                    "proxy_password": node.proxy_password,
                    "tunnel_mode": node.tunnel_mode,
                    "heartbeat_interval": node.heartbeat_interval,
                    "remote_config": node.remote_config,
                    "config_version": node.config_version,
                }
            )

        return {
            "version": CONFIG_EXPORT_VERSION,
            "exported_at": datetime.now(timezone.utc).isoformat(),
            "global_models": global_models_data,
            "providers": providers_data,
            "proxy_nodes": proxy_nodes_data,
            "ldap_config": ldap_data,
            "oauth_providers": oauth_data,
            "system_configs": system_configs_data,
        }


class AdminImportConfigAdapter(AdminApiAdapter):
    """导入提供商和模型配置"""

    # Provider Ops 中需要加密的敏感字段
    SENSITIVE_CREDENTIALS = SENSITIVE_CREDENTIAL_FIELDS

    @staticmethod
    def _remap_proxy_node_id(
        proxy: dict[str, Any] | None,
        node_id_map: dict[str, str],
    ) -> dict[str, Any] | None:
        """替换 proxy 配置中的 node_id 为新实例的 ID。

        - node_id 在映射表中：替换为新 ID
        - node_id 不在映射表中（节点未导入）：清除 proxy 配置
        - 无 node_id（手动 URL 模式）：原样返回
        """
        if not proxy or not isinstance(proxy, dict):
            return proxy

        old_node_id = proxy.get("node_id")
        if not old_node_id or not isinstance(old_node_id, str):
            return proxy  # 手动 URL 模式，无需映射

        new_node_id = node_id_map.get(old_node_id)
        if new_node_id is None:
            return None  # 节点未导入，清除代理配置

        remapped = dict(proxy)
        remapped["node_id"] = new_node_id
        return remapped

    @staticmethod
    def _extract_import_key_api_formats(
        key_data: dict[str, Any], endpoint_formats: set[str]
    ) -> list[str]:
        """导入 Key 时提取 api_formats（兼容历史字段与旧语义）。"""
        raw_formats = key_data.get("api_formats")
        if isinstance(raw_formats, list):
            if raw_formats:
                return raw_formats
            legacy_formats = key_data.get("supported_endpoints")
            if isinstance(legacy_formats, list) and legacy_formats:
                return legacy_formats
            return []

        legacy_formats = key_data.get("supported_endpoints")
        if isinstance(legacy_formats, list) and legacy_formats:
            return legacy_formats

        # 兼容历史数据：api_formats=None 代表支持 Provider 的全部端点。
        if raw_formats is None and endpoint_formats:
            return sorted(endpoint_formats)
        return []

    def _encrypt_provider_config(self, config: dict, crypto_service: Any) -> dict:
        """加密 Provider config 中的 provider_ops credentials"""
        if not config:
            return config

        encrypted_config = copy.deepcopy(config)

        # 加密 provider_ops.connector.credentials 中的敏感字段
        provider_ops = encrypted_config.get("provider_ops")
        if provider_ops and isinstance(provider_ops, dict):
            connector = provider_ops.get("connector")
            if connector and isinstance(connector, dict):
                credentials = connector.get("credentials")
                if credentials and isinstance(credentials, dict):
                    for field in self.SENSITIVE_CREDENTIALS:
                        if field in credentials and isinstance(credentials[field], str):
                            value = credentials[field]
                            if value:  # 只加密非空值
                                credentials[field] = crypto_service.encrypt(value)

        return encrypted_config

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        """导入提供商和模型配置"""
        import uuid
        from datetime import datetime, timezone

        from src.core.crypto import crypto_service
        from src.core.enums import ProviderBillingType
        from src.models.database import (
            GlobalModel,
            Model,
            ProviderAPIKey,
            ProviderEndpoint,
            ProxyNode,
        )

        # 检查请求体大小
        if context.raw_body and len(context.raw_body) > MAX_IMPORT_SIZE:
            raise InvalidRequestException("请求体大小不能超过 10MB")

        db = context.db
        payload = context.ensure_json_body()

        # 验证配置版本
        version = payload.get("version")
        if version not in CONFIG_SUPPORTED_VERSIONS:
            raise InvalidRequestException(
                f"不支持的配置版本: {version}，支持的版本: {', '.join(CONFIG_SUPPORTED_VERSIONS)}"
            )

        # 获取导入选项
        merge_mode = payload.get("merge_mode", "skip")  # skip, overwrite, error
        global_models_data = payload.get("global_models", [])
        providers_data = payload.get("providers", [])
        proxy_nodes_data = payload.get("proxy_nodes", [])
        ldap_data = payload.get("ldap_config")  # 2.1 新增
        oauth_data = payload.get("oauth_providers", [])  # 2.1 新增
        system_configs_data = payload.get("system_configs", [])  # 2.2 新增

        stats = {
            "global_models": {"created": 0, "updated": 0, "skipped": 0},
            "proxy_nodes": {"created": 0, "updated": 0, "skipped": 0},
            "providers": {"created": 0, "updated": 0, "skipped": 0},
            "endpoints": {"created": 0, "updated": 0, "skipped": 0},
            "keys": {"created": 0, "updated": 0, "skipped": 0},
            "models": {"created": 0, "updated": 0, "skipped": 0},
            "ldap": {"created": 0, "updated": 0, "skipped": 0},
            "oauth": {"created": 0, "updated": 0, "skipped": 0},
            "system_configs": {"created": 0, "updated": 0, "skipped": 0},  # 2.2 新增
            "errors": [],
        }

        try:
            # 导入 GlobalModels
            global_model_map = {}  # name -> id 映射
            for gm_data in global_models_data:
                existing = db.query(GlobalModel).filter(GlobalModel.name == gm_data["name"]).first()

                if existing:
                    global_model_map[gm_data["name"]] = existing.id
                    if merge_mode == "skip":
                        stats["global_models"]["skipped"] += 1
                        continue
                    elif merge_mode == "error":
                        raise InvalidRequestException(f"GlobalModel '{gm_data['name']}' 已存在")
                    elif merge_mode == "overwrite":
                        # 更新现有记录
                        existing.display_name = gm_data.get("display_name", existing.display_name)
                        existing.default_price_per_request = gm_data.get(
                            "default_price_per_request"
                        )
                        existing.default_tiered_pricing = gm_data.get(
                            "default_tiered_pricing", existing.default_tiered_pricing
                        )
                        existing.supported_capabilities = gm_data.get("supported_capabilities")
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
                            {
                                "tiers": [
                                    {
                                        "up_to": None,
                                        "input_price_per_1m": 0,
                                        "output_price_per_1m": 0,
                                    }
                                ]
                            },
                        ),
                        supported_capabilities=gm_data.get("supported_capabilities"),
                        config=gm_data.get("config"),
                        is_active=gm_data.get("is_active", True),
                    )
                    db.add(new_gm)
                    db.flush()
                    global_model_map[gm_data["name"]] = new_gm.id
                    stats["global_models"]["created"] += 1

            # 导入 ProxyNodes（在 Providers 之前，建立 old_id -> new_id 映射）
            proxy_node_id_map: dict[str, str] = {}  # old_node_id -> new_node_id
            for node_data in proxy_nodes_data:
                old_id = node_data.get("id", "")
                ip = node_data.get("ip", "")
                port = node_data.get("port", 0)

                if not ip or not port:
                    stats["errors"].append(f"跳过无效的代理节点: {node_data.get('name', '?')}")
                    continue

                existing_node = (
                    db.query(ProxyNode).filter(ProxyNode.ip == ip, ProxyNode.port == port).first()
                )

                if existing_node:
                    proxy_node_id_map[old_id] = existing_node.id
                    if merge_mode == "skip":
                        stats["proxy_nodes"]["skipped"] += 1
                    elif merge_mode == "error":
                        raise InvalidRequestException(
                            f"代理节点 '{node_data.get('name')}' ({ip}:{port}) 已存在"
                        )
                    elif merge_mode == "overwrite":
                        existing_node.name = node_data.get("name", existing_node.name)
                        existing_node.region = node_data.get("region", existing_node.region)
                        existing_node.is_manual = node_data.get(
                            "is_manual", existing_node.is_manual
                        )
                        existing_node.proxy_url = node_data.get(
                            "proxy_url", existing_node.proxy_url
                        )
                        existing_node.proxy_username = node_data.get(
                            "proxy_username", existing_node.proxy_username
                        )
                        existing_node.proxy_password = node_data.get(
                            "proxy_password", existing_node.proxy_password
                        )
                        existing_node.tunnel_mode = node_data.get(
                            "tunnel_mode", existing_node.tunnel_mode
                        )
                        existing_node.remote_config = node_data.get(
                            "remote_config", existing_node.remote_config
                        )
                        existing_node.updated_at = datetime.now(timezone.utc)
                        stats["proxy_nodes"]["updated"] += 1
                else:
                    from src.models.database import ProxyNodeStatus

                    is_manual = node_data.get("is_manual", False)
                    new_node = ProxyNode(
                        id=str(uuid.uuid4()),
                        name=node_data.get("name", "Imported Node"),
                        ip=ip,
                        port=port,
                        region=node_data.get("region"),
                        is_manual=is_manual,
                        proxy_url=node_data.get("proxy_url"),
                        proxy_username=node_data.get("proxy_username"),
                        proxy_password=node_data.get("proxy_password"),
                        tunnel_mode=node_data.get("tunnel_mode", False),
                        heartbeat_interval=node_data.get("heartbeat_interval", 0),
                        remote_config=node_data.get("remote_config"),
                        config_version=node_data.get("config_version", 0),
                        status=ProxyNodeStatus.ONLINE if is_manual else ProxyNodeStatus.OFFLINE,
                    )
                    db.add(new_node)
                    db.flush()
                    proxy_node_id_map[old_id] = new_node.id
                    stats["proxy_nodes"]["created"] += 1

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
                        raise InvalidRequestException(f"Provider '{prov_data['name']}' 已存在")
                    elif merge_mode == "overwrite":
                        # 更新现有记录
                        existing_provider.name = prov_data.get("name", existing_provider.name)
                        existing_provider.provider_type = prov_data.get(
                            "provider_type", existing_provider.provider_type
                        )
                        existing_provider.description = prov_data.get("description")
                        existing_provider.website = prov_data.get("website")
                        if prov_data.get("billing_type"):
                            existing_provider.billing_type = ProviderBillingType(
                                prov_data["billing_type"]
                            )
                        existing_provider.monthly_quota_usd = prov_data.get("monthly_quota_usd")
                        existing_provider.quota_reset_day = prov_data.get("quota_reset_day", 30)
                        existing_provider.provider_priority = prov_data.get(
                            "provider_priority", 100
                        )
                        existing_provider.keep_priority_on_conversion = prov_data.get(
                            "keep_priority_on_conversion",
                            existing_provider.keep_priority_on_conversion,
                        )
                        existing_provider.enable_format_conversion = prov_data.get(
                            "enable_format_conversion",
                            existing_provider.enable_format_conversion,
                        )
                        existing_provider.is_active = prov_data.get("is_active", True)
                        existing_provider.concurrent_limit = prov_data.get("concurrent_limit")
                        existing_provider.max_retries = prov_data.get(
                            "max_retries", existing_provider.max_retries
                        )
                        existing_provider.stream_first_byte_timeout = prov_data.get(
                            "stream_first_byte_timeout",
                            existing_provider.stream_first_byte_timeout,
                        )
                        existing_provider.request_timeout = prov_data.get(
                            "request_timeout", existing_provider.request_timeout
                        )
                        if "proxy" in prov_data:
                            existing_provider.proxy = self._remap_proxy_node_id(
                                prov_data["proxy"],
                                proxy_node_id_map,
                            )
                        # 未提供 proxy 字段时保留现有配置
                        # 加密 provider_ops credentials 后再保存
                        existing_provider.config = self._encrypt_provider_config(
                            prov_data.get("config"), crypto_service
                        )
                        existing_provider.updated_at = datetime.now(timezone.utc)
                        stats["providers"]["updated"] += 1
                else:
                    # 创建新 Provider
                    billing_type = ProviderBillingType.PAY_AS_YOU_GO
                    if prov_data.get("billing_type"):
                        billing_type = ProviderBillingType(prov_data["billing_type"])

                    # 加密 provider_ops credentials 后再保存
                    encrypted_config = self._encrypt_provider_config(
                        prov_data.get("config"), crypto_service
                    )

                    new_provider = Provider(
                        id=str(uuid.uuid4()),
                        name=prov_data["name"],
                        provider_type=prov_data.get("provider_type", "custom"),
                        description=prov_data.get("description"),
                        website=prov_data.get("website"),
                        billing_type=billing_type,
                        monthly_quota_usd=prov_data.get("monthly_quota_usd"),
                        quota_reset_day=prov_data.get("quota_reset_day", 30),
                        provider_priority=prov_data.get("provider_priority", 100),
                        keep_priority_on_conversion=prov_data.get(
                            "keep_priority_on_conversion", False
                        ),
                        enable_format_conversion=prov_data.get("enable_format_conversion", False),
                        is_active=prov_data.get("is_active", True),
                        concurrent_limit=prov_data.get("concurrent_limit"),
                        max_retries=prov_data.get("max_retries"),
                        stream_first_byte_timeout=prov_data.get("stream_first_byte_timeout"),
                        request_timeout=prov_data.get("request_timeout"),
                        proxy=self._remap_proxy_node_id(prov_data.get("proxy"), proxy_node_id_map),
                        config=encrypted_config,
                    )
                    db.add(new_provider)
                    db.flush()
                    provider_id = new_provider.id
                    stats["providers"]["created"] += 1

                # 导入 Endpoints
                for ep_data in prov_data.get("endpoints", []):
                    from src.core.api_format.signature import (
                        normalize_signature_key,
                        parse_signature_key,
                    )

                    ep_format = normalize_signature_key(ep_data["api_format"])
                    existing_ep = (
                        db.query(ProviderEndpoint)
                        .filter(
                            ProviderEndpoint.provider_id == provider_id,
                            ProviderEndpoint.api_format == ep_format,
                        )
                        .first()
                    )

                    if existing_ep:
                        if merge_mode == "skip":
                            stats["endpoints"]["skipped"] += 1
                        elif merge_mode == "error":
                            raise InvalidRequestException(
                                f"Endpoint '{ep_format}' 已存在于 Provider '{prov_data['name']}'"
                            )
                        elif merge_mode == "overwrite":
                            existing_ep.base_url = ep_data.get("base_url", existing_ep.base_url)
                            existing_ep.header_rules = ep_data.get("header_rules")
                            existing_ep.body_rules = ep_data.get("body_rules")
                            existing_ep.max_retries = ep_data.get("max_retries", 2)
                            existing_ep.is_active = ep_data.get("is_active", True)
                            existing_ep.custom_path = ep_data.get("custom_path")
                            existing_ep.config = ep_data.get("config")
                            existing_ep.format_acceptance_config = ep_data.get(
                                "format_acceptance_config"
                            )
                            existing_ep.proxy = self._remap_proxy_node_id(
                                ep_data.get("proxy"), proxy_node_id_map
                            )
                            sig = parse_signature_key(ep_format)
                            existing_ep.api_format = sig.key  # 使用归一化后的格式
                            existing_ep.api_family = sig.api_family.value
                            existing_ep.endpoint_kind = sig.endpoint_kind.value
                            existing_ep.updated_at = datetime.now(timezone.utc)
                            stats["endpoints"]["updated"] += 1
                    else:
                        sig = parse_signature_key(ep_format)
                        api_family = sig.api_family.value
                        endpoint_kind = sig.endpoint_kind.value
                        new_ep = ProviderEndpoint(
                            id=str(uuid.uuid4()),
                            provider_id=provider_id,
                            api_format=sig.key,  # 使用归一化后的格式
                            api_family=api_family,
                            endpoint_kind=endpoint_kind,
                            base_url=ep_data["base_url"],
                            header_rules=ep_data.get("header_rules"),
                            body_rules=ep_data.get("body_rules"),
                            max_retries=ep_data.get("max_retries", 2),
                            is_active=ep_data.get("is_active", True),
                            custom_path=ep_data.get("custom_path"),
                            config=ep_data.get("config"),
                            format_acceptance_config=ep_data.get("format_acceptance_config"),
                            proxy=self._remap_proxy_node_id(
                                ep_data.get("proxy"), proxy_node_id_map
                            ),
                        )
                        db.add(new_ep)
                        db.flush()
                        stats["endpoints"]["created"] += 1

                # 导入 Provider Keys（按 provider_id 归属）
                from src.core.api_format.signature import normalize_signature_key

                endpoint_format_rows = (
                    db.query(ProviderEndpoint.api_format)
                    .filter(ProviderEndpoint.provider_id == provider_id)
                    .all()
                )
                endpoint_formats: set[str] = set()
                for (api_format,) in endpoint_format_rows:
                    fmt = api_format.value if hasattr(api_format, "value") else str(api_format)
                    endpoint_formats.add(normalize_signature_key(fmt))
                existing_keys = (
                    db.query(ProviderAPIKey).filter(ProviderAPIKey.provider_id == provider_id).all()
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
                        stats["errors"].append(f"跳过空 API Key (Provider: {prov_data['name']})")
                        continue

                    plaintext_key = key_data["api_key"]
                    if plaintext_key in existing_key_values:
                        stats["keys"]["skipped"] += 1
                        continue

                    raw_formats = self._extract_import_key_api_formats(key_data, endpoint_formats)
                    if len(raw_formats) == 0:
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
                        fmt_stripped = fmt.strip()
                        if not fmt_stripped:
                            continue
                        # 使用 normalize_signature_key 归一化，与 endpoint_formats 保持一致
                        try:
                            fmt_normalized = normalize_signature_key(fmt_stripped)
                        except (ValueError, KeyError):
                            # 无效的格式字符串，跳过
                            missing_formats.append(fmt_stripped.upper())
                            continue
                        if fmt_normalized in seen:
                            continue
                        seen.add(fmt_normalized)
                        if endpoint_formats and fmt_normalized not in endpoint_formats:
                            missing_formats.append(fmt_normalized.upper())
                            continue
                        normalized_formats.append(fmt_normalized)

                    if missing_formats:
                        stats["errors"].append(
                            f"Key (Provider: {prov_data['name']}) 的 api_formats 未配置对应 Endpoint，已跳过: {missing_formats}"
                        )

                    if len(normalized_formats) == 0:
                        stats["keys"]["skipped"] += 1
                        continue

                    encrypted_key = crypto_service.encrypt(plaintext_key)

                    # 加密 auth_config（如果有）
                    encrypted_auth_config = None
                    raw_auth_config = key_data.get("auth_config")
                    if raw_auth_config:
                        # auth_config 导出时是解密后的 JSON 字符串，需要重新加密
                        auth_config_str = (
                            raw_auth_config
                            if isinstance(raw_auth_config, str)
                            else json.dumps(raw_auth_config)
                        )
                        encrypted_auth_config = crypto_service.encrypt(auth_config_str)

                    from src.services.provider.fingerprint import generate_fingerprint

                    new_key_id = str(uuid.uuid4())
                    new_key = ProviderAPIKey(
                        id=new_key_id,
                        provider_id=provider_id,
                        api_formats=normalized_formats,
                        auth_type=key_data.get("auth_type", "api_key"),
                        api_key=encrypted_key,
                        auth_config=encrypted_auth_config,
                        name=key_data.get("name") or "Imported Key",
                        note=key_data.get("note"),
                        rate_multipliers=key_data.get("rate_multipliers"),
                        internal_priority=key_data.get("internal_priority", 50),
                        global_priority_by_format=key_data.get("global_priority_by_format"),
                        rpm_limit=key_data.get("rpm_limit"),
                        allowed_models=key_data.get("allowed_models"),
                        capabilities=key_data.get("capabilities"),
                        cache_ttl_minutes=key_data.get("cache_ttl_minutes", 5),
                        max_probe_interval_minutes=key_data.get("max_probe_interval_minutes", 32),
                        auto_fetch_models=key_data.get("auto_fetch_models", False),
                        locked_models=key_data.get("locked_models"),
                        model_include_patterns=key_data.get("model_include_patterns"),
                        model_exclude_patterns=key_data.get("model_exclude_patterns"),
                        is_active=key_data.get("is_active", True),
                        proxy=self._remap_proxy_node_id(key_data.get("proxy"), proxy_node_id_map),
                        fingerprint=generate_fingerprint(seed=new_key_id),
                        health_by_format={},
                        circuit_breaker_by_format={},
                    )
                    db.add(new_key)
                    existing_key_values.add(plaintext_key)
                    stats["keys"]["created"] += 1

                    # 如果开启了 auto_fetch_models，记录需要触发获取的 Key ID
                    if key_data.get("auto_fetch_models", False):
                        if "keys_to_fetch" not in stats:
                            stats["keys_to_fetch"] = []
                        stats["keys_to_fetch"].append(new_key.id)

                # 导入 Models
                # 注意：提供商模型（Model）必须关联全局模型（GlobalModel）才能参与路由
                # 未关联 GlobalModel 的模型会被跳过，这是业务规则而非 bug
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
                            existing_model.price_per_request = model_data.get("price_per_request")
                            existing_model.tiered_pricing = model_data.get("tiered_pricing")
                            existing_model.supports_vision = model_data.get("supports_vision")
                            existing_model.supports_function_calling = model_data.get(
                                "supports_function_calling"
                            )
                            existing_model.supports_streaming = model_data.get("supports_streaming")
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
                            provider_model_mappings=model_data.get("provider_model_mappings"),
                            price_per_request=model_data.get("price_per_request"),
                            tiered_pricing=model_data.get("tiered_pricing"),
                            supports_vision=model_data.get("supports_vision"),
                            supports_function_calling=model_data.get("supports_function_calling"),
                            supports_streaming=model_data.get("supports_streaming"),
                            supports_extended_thinking=model_data.get("supports_extended_thinking"),
                            supports_image_generation=model_data.get("supports_image_generation"),
                            is_active=model_data.get("is_active", True),
                            config=model_data.get("config"),
                        )
                        db.add(new_model)
                        stats["models"]["created"] += 1

            # 导入 LDAP 配置（2.1 新增）
            if ldap_data:
                from src.models.database import LDAPConfig

                # 校验必填字段
                required_ldap_fields = ["server_url", "bind_dn", "base_dn"]
                missing = [f for f in required_ldap_fields if not ldap_data.get(f)]
                if missing:
                    raise InvalidRequestException(f"LDAP 配置缺少必填字段: {', '.join(missing)}")

                existing_ldap = db.query(LDAPConfig).first()

                if existing_ldap:
                    if merge_mode == "skip":
                        stats["ldap"]["skipped"] += 1
                    elif merge_mode == "error":
                        raise InvalidRequestException("LDAP 配置已存在")
                    elif merge_mode == "overwrite":
                        existing_ldap.server_url = ldap_data.get(
                            "server_url", existing_ldap.server_url
                        )
                        existing_ldap.bind_dn = ldap_data.get("bind_dn", existing_ldap.bind_dn)
                        # 加密绑定密码
                        if ldap_data.get("bind_password"):
                            existing_ldap.bind_password_encrypted = crypto_service.encrypt(
                                ldap_data["bind_password"]
                            )
                        existing_ldap.base_dn = ldap_data.get("base_dn", existing_ldap.base_dn)
                        existing_ldap.user_search_filter = ldap_data.get(
                            "user_search_filter", existing_ldap.user_search_filter
                        )
                        existing_ldap.username_attr = ldap_data.get(
                            "username_attr", existing_ldap.username_attr
                        )
                        existing_ldap.email_attr = ldap_data.get(
                            "email_attr", existing_ldap.email_attr
                        )
                        existing_ldap.display_name_attr = ldap_data.get(
                            "display_name_attr", existing_ldap.display_name_attr
                        )
                        existing_ldap.is_enabled = ldap_data.get(
                            "is_enabled", existing_ldap.is_enabled
                        )
                        existing_ldap.is_exclusive = ldap_data.get(
                            "is_exclusive", existing_ldap.is_exclusive
                        )
                        existing_ldap.use_starttls = ldap_data.get(
                            "use_starttls", existing_ldap.use_starttls
                        )
                        existing_ldap.connect_timeout = ldap_data.get(
                            "connect_timeout", existing_ldap.connect_timeout
                        )
                        existing_ldap.updated_at = datetime.now(timezone.utc)
                        stats["ldap"]["updated"] += 1
                else:
                    # 创建新的 LDAP 配置
                    new_ldap = LDAPConfig(
                        server_url=ldap_data["server_url"],
                        bind_dn=ldap_data["bind_dn"],
                        bind_password_encrypted=(
                            crypto_service.encrypt(ldap_data["bind_password"])
                            if ldap_data.get("bind_password")
                            else None
                        ),
                        base_dn=ldap_data["base_dn"],
                        user_search_filter=ldap_data.get("user_search_filter", "(uid={username})"),
                        username_attr=ldap_data.get("username_attr", "uid"),
                        email_attr=ldap_data.get("email_attr", "mail"),
                        display_name_attr=ldap_data.get("display_name_attr", "cn"),
                        is_enabled=ldap_data.get("is_enabled", False),
                        is_exclusive=ldap_data.get("is_exclusive", False),
                        use_starttls=ldap_data.get("use_starttls", False),
                        connect_timeout=ldap_data.get("connect_timeout", 10),
                    )
                    db.add(new_ldap)
                    stats["ldap"]["created"] += 1

            # 导入 OAuth Providers（2.1 新增）
            if oauth_data:
                from src.models.database import OAuthProvider

                for oauth_item in oauth_data:
                    provider_type = oauth_item.get("provider_type")
                    if not provider_type:
                        stats["errors"].append("跳过无 provider_type 的 OAuth 配置")
                        continue

                    existing_oauth = (
                        db.query(OAuthProvider)
                        .filter(OAuthProvider.provider_type == provider_type)
                        .first()
                    )

                    if existing_oauth:
                        if merge_mode == "skip":
                            stats["oauth"]["skipped"] += 1
                        elif merge_mode == "error":
                            raise InvalidRequestException(
                                f"OAuth Provider '{provider_type}' 已存在"
                            )
                        elif merge_mode == "overwrite":
                            existing_oauth.display_name = oauth_item.get(
                                "display_name", existing_oauth.display_name
                            )
                            existing_oauth.client_id = oauth_item.get(
                                "client_id", existing_oauth.client_id
                            )
                            # 加密 client_secret
                            if oauth_item.get("client_secret"):
                                existing_oauth.client_secret_encrypted = crypto_service.encrypt(
                                    oauth_item["client_secret"]
                                )
                            existing_oauth.authorization_url_override = oauth_item.get(
                                "authorization_url_override"
                            )
                            existing_oauth.token_url_override = oauth_item.get("token_url_override")
                            existing_oauth.userinfo_url_override = oauth_item.get(
                                "userinfo_url_override"
                            )
                            existing_oauth.scopes = oauth_item.get("scopes")
                            existing_oauth.redirect_uri = oauth_item.get(
                                "redirect_uri", existing_oauth.redirect_uri
                            )
                            existing_oauth.frontend_callback_url = oauth_item.get(
                                "frontend_callback_url", existing_oauth.frontend_callback_url
                            )
                            existing_oauth.attribute_mapping = oauth_item.get("attribute_mapping")
                            existing_oauth.extra_config = oauth_item.get("extra_config")
                            existing_oauth.is_enabled = oauth_item.get(
                                "is_enabled", existing_oauth.is_enabled
                            )
                            existing_oauth.updated_at = datetime.now(timezone.utc)
                            stats["oauth"]["updated"] += 1
                    else:
                        # 创建新的 OAuth Provider - 校验必填字段
                        required_oauth_fields = [
                            "client_id",
                            "redirect_uri",
                            "frontend_callback_url",
                        ]
                        missing = [f for f in required_oauth_fields if not oauth_item.get(f)]
                        if missing:
                            stats["errors"].append(
                                f"OAuth Provider '{provider_type}' 缺少必填字段: {', '.join(missing)}"
                            )
                            continue

                        new_oauth = OAuthProvider(
                            provider_type=provider_type,
                            display_name=oauth_item.get("display_name", provider_type),
                            client_id=oauth_item["client_id"],
                            client_secret_encrypted=(
                                crypto_service.encrypt(oauth_item["client_secret"])
                                if oauth_item.get("client_secret")
                                else None
                            ),
                            authorization_url_override=oauth_item.get("authorization_url_override"),
                            token_url_override=oauth_item.get("token_url_override"),
                            userinfo_url_override=oauth_item.get("userinfo_url_override"),
                            scopes=oauth_item.get("scopes"),
                            redirect_uri=oauth_item["redirect_uri"],
                            frontend_callback_url=oauth_item["frontend_callback_url"],
                            attribute_mapping=oauth_item.get("attribute_mapping"),
                            extra_config=oauth_item.get("extra_config"),
                            is_enabled=oauth_item.get("is_enabled", False),
                        )
                        db.add(new_oauth)
                        stats["oauth"]["created"] += 1

            # 导入 SystemConfig（2.2 新增）
            if system_configs_data:
                from src.models.database import SystemConfig

                # 敏感配置项需要加密存储
                SENSITIVE_CONFIG_KEYS = {"smtp_password"}

                for cfg_item in system_configs_data:
                    cfg_key = cfg_item.get("key")
                    if not cfg_key:
                        stats["errors"].append("跳过无 key 的 SystemConfig 配置")
                        continue

                    existing_cfg = (
                        db.query(SystemConfig).filter(SystemConfig.key == cfg_key).first()
                    )

                    cfg_value = cfg_item.get("value")
                    # 加密敏感配置
                    if cfg_key in SENSITIVE_CONFIG_KEYS and cfg_value:
                        cfg_value = crypto_service.encrypt(cfg_value)

                    if existing_cfg:
                        if merge_mode == "skip":
                            stats["system_configs"]["skipped"] += 1
                        elif merge_mode == "error":
                            raise InvalidRequestException(f"SystemConfig '{cfg_key}' 已存在")
                        elif merge_mode == "overwrite":
                            existing_cfg.value = cfg_value
                            existing_cfg.description = cfg_item.get(
                                "description", existing_cfg.description
                            )
                            existing_cfg.updated_at = datetime.now(timezone.utc)
                            stats["system_configs"]["updated"] += 1
                    else:
                        new_cfg = SystemConfig(
                            key=cfg_key,
                            value=cfg_value,
                            description=cfg_item.get("description"),
                        )
                        db.add(new_cfg)
                        stats["system_configs"]["created"] += 1

            db.commit()

            # 失效缓存
            from src.services.cache.invalidation import get_cache_invalidation_service

            cache_service = get_cache_invalidation_service()
            cache_service.clear_all_caches()

            # 触发开启了 auto_fetch_models 的 Key 的模型获取
            keys_to_fetch = stats.get("keys_to_fetch", [])
            if keys_to_fetch:
                logger.info(
                    f"[AUTO_FETCH] 导入了 {len(keys_to_fetch)} 个开启自动获取模型的 Key，触发模型获取"
                )
                try:
                    import asyncio

                    from src.services.model.fetch_scheduler import get_model_fetch_scheduler
                    from src.utils.async_utils import safe_create_task

                    scheduler = get_model_fetch_scheduler()
                    for key_id in keys_to_fetch:
                        safe_create_task(scheduler._fetch_models_for_key_by_id(key_id))
                except Exception as e:
                    logger.error(f"触发模型获取失败: {e}")
                    # 不影响导入成功的返回
                # 从统计信息中移除内部字段
                stats.pop("keys_to_fetch", None)

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
    @staticmethod
    def _serialize_api_key(
        key: ApiKey,
        include_is_standalone: bool = False,
        db: Any = None,
    ) -> dict[str, Any]:
        """序列化用户 API Key 为导出格式。"""
        from src.core.crypto import crypto_service

        wallet = None
        if db is not None and key.is_standalone:
            wallet = WalletService.get_wallet(db, api_key_id=key.id)

        data: dict[str, Any] = {
            "key_hash": key.key_hash,
            "name": key.name,
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
            "wallet": WalletService.serialize_wallet_summary(wallet) if wallet else None,
        }

        if key.key_encrypted:
            try:
                data["key"] = crypto_service.decrypt(key.key_encrypted, silent=True)
            except Exception:
                logger.warning(
                    "[USERS_EXPORT] API Key 解密失败，回退为 legacy 密文字段: key_id={}", key.id
                )
                data["key_encrypted"] = key.key_encrypted

        if include_is_standalone:
            data["is_standalone"] = key.is_standalone

        return data

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        """导出用户数据（优先导出解密后的完整 Key，排除管理员）"""
        from datetime import datetime, timezone

        from src.core.enums import UserRole
        from src.models.database import ApiKey, User

        db = context.db

        # 导出 Users（排除管理员）
        users = db.query(User).filter(User.is_deleted.is_(False), User.role != UserRole.ADMIN).all()
        users_data = []
        for user in users:
            wallet = WalletService.get_wallet(db, user_id=user.id)
            # 导出用户的 API Keys（排除独立余额Key，独立Key单独导出）
            api_keys = (
                db.query(ApiKey)
                .filter(ApiKey.user_id == user.id, ApiKey.is_standalone.is_(False))
                .all()
            )
            api_keys_data = [
                self._serialize_api_key(key, include_is_standalone=True) for key in api_keys
            ]

            users_data.append(
                {
                    "email": user.email,
                    "email_verified": user.email_verified,
                    "username": user.username,
                    "password_hash": user.password_hash,
                    "role": user.role.value if user.role else "user",
                    "allowed_providers": user.allowed_providers,
                    "allowed_api_formats": user.allowed_api_formats,
                    "allowed_models": user.allowed_models,
                    "model_capability_settings": user.model_capability_settings,
                    "unlimited": WalletService.is_unlimited_wallet(wallet),
                    "wallet": WalletService.serialize_wallet_summary(wallet) if wallet else None,
                    "is_active": user.is_active,
                    "api_keys": api_keys_data,
                }
            )

        # 导出独立余额 Keys（管理员创建的，不属于普通用户）
        standalone_keys = db.query(ApiKey).filter(ApiKey.is_standalone.is_(True)).all()
        standalone_keys_data = [self._serialize_api_key(key, db=db) for key in standalone_keys]

        return {
            "version": "1.2",
            "exported_at": datetime.now(timezone.utc).isoformat(),
            "users": users_data,
            "standalone_keys": standalone_keys_data,
        }


class AdminImportUsersAdapter(AdminApiAdapter):
    @staticmethod
    def _resolve_api_key_material(key_data: dict[str, Any]) -> tuple[str | None, str | None]:
        """解析用户 API Key 导入材料，优先使用明文 key。"""
        from src.core.crypto import crypto_service
        from src.models.database import ApiKey

        plaintext_key = key_data.get("key")
        if isinstance(plaintext_key, str):
            normalized = plaintext_key.strip()
            if normalized:
                return ApiKey.hash_key(normalized), crypto_service.encrypt(normalized)

        key_hash = str(key_data.get("key_hash") or "").strip() or None
        key_encrypted = key_data.get("key_encrypted")
        return key_hash, key_encrypted

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
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
            key_hash, key_encrypted = self._resolve_api_key_material(key_data)
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

            return (
                ApiKey(
                    id=str(uuid.uuid4()),
                    user_id=owner_id,
                    key_hash=key_hash,
                    key_encrypted=key_encrypted,
                    name=key_data.get("name"),
                    is_standalone=is_standalone or key_data.get("is_standalone", False),
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
                ),
                "created",
            )

        try:
            for user_data in users_data:
                # 跳过管理员角色的导入（不区分大小写）
                role_str = str(user_data.get("role", "")).lower()
                if role_str == "admin":
                    stats["errors"].append(f"跳过管理员用户: {user_data.get('email')}")
                    stats["users"]["skipped"] += 1
                    continue

                # 导入必须有邮箱（email 是导入的主键）
                import_email = user_data.get("email")
                if not import_email:
                    stats["errors"].append(f"跳过无邮箱用户: {user_data.get('username', '未知')}")
                    stats["users"]["skipped"] += 1
                    continue

                existing_user = db.query(User).filter(User.email == import_email).first()
                wallet_payload = (
                    user_data.get("wallet") if isinstance(user_data.get("wallet"), dict) else None
                )
                wallet_limit_mode = (
                    str(wallet_payload.get("limit_mode"))
                    if wallet_payload
                    and wallet_payload.get("limit_mode") in {"finite", "unlimited"}
                    else ("unlimited" if user_data.get("unlimited") else "finite")
                )

                if existing_user:
                    user_id = existing_user.id
                    if merge_mode == "skip":
                        stats["users"]["skipped"] += 1
                    elif merge_mode == "error":
                        raise InvalidRequestException(f"用户 '{import_email}' 已存在")
                    elif merge_mode == "overwrite":
                        # 更新现有用户
                        existing_user.username = user_data.get("username", existing_user.username)
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
                        existing_user.is_active = user_data.get("is_active", True)
                        existing_user.updated_at = datetime.now(timezone.utc)
                        wallet = WalletService.get_or_create_wallet(db, user=existing_user)
                        if wallet is not None:
                            wallet.limit_mode = wallet_limit_mode
                            if wallet_payload:
                                wallet.balance = wallet_payload.get("recharge_balance", 0) or 0
                                wallet.gift_balance = wallet_payload.get("gift_balance", 0) or 0
                                wallet.total_recharged = (
                                    wallet_payload.get("total_recharged", 0) or 0
                                )
                                wallet.total_consumed = wallet_payload.get("total_consumed", 0) or 0
                                wallet.total_refunded = wallet_payload.get("total_refunded", 0) or 0
                                wallet.total_adjusted = wallet_payload.get("total_adjusted", 0) or 0
                                wallet.status = wallet_payload.get("status", "active") or "active"
                            wallet.updated_at = datetime.now(timezone.utc)
                        stats["users"]["updated"] += 1
                else:
                    # 创建新用户
                    role = UserRole.USER
                    if user_data.get("role"):
                        role = UserRole(user_data["role"])

                    new_user = User(
                        id=str(uuid.uuid4()),
                        email=import_email,
                        email_verified=user_data.get("email_verified", True),
                        username=user_data.get("username") or import_email.split("@")[0],
                        password_hash=user_data.get("password_hash", ""),
                        role=role,
                        allowed_providers=user_data.get("allowed_providers"),
                        allowed_api_formats=user_data.get("allowed_api_formats"),
                        allowed_models=user_data.get("allowed_models"),
                        model_capability_settings=user_data.get("model_capability_settings"),
                        is_active=user_data.get("is_active", True),
                    )
                    db.add(new_user)
                    db.flush()
                    wallet = WalletService.get_or_create_wallet(db, user=new_user)
                    if wallet is not None:
                        wallet.limit_mode = wallet_limit_mode
                        if wallet_payload:
                            wallet.balance = wallet_payload.get("recharge_balance", 0) or 0
                            wallet.gift_balance = wallet_payload.get("gift_balance", 0) or 0
                            wallet.total_recharged = wallet_payload.get("total_recharged", 0) or 0
                            wallet.total_consumed = wallet_payload.get("total_consumed", 0) or 0
                            wallet.total_refunded = wallet_payload.get("total_refunded", 0) or 0
                            wallet.total_adjusted = wallet_payload.get("total_adjusted", 0) or 0
                            wallet.status = wallet_payload.get("status", "active") or "active"
                        wallet.updated_at = datetime.now(timezone.utc)
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
                            db.flush()
                            wallet = WalletService.get_or_create_wallet(db, api_key=new_key)
                            wallet_payload = (
                                key_data.get("wallet")
                                if isinstance(key_data.get("wallet"), dict)
                                else None
                            )
                            if wallet is not None:
                                wallet.limit_mode = (
                                    str(wallet_payload.get("limit_mode"))
                                    if wallet_payload
                                    and wallet_payload.get("limit_mode") in {"finite", "unlimited"}
                                    else ("unlimited" if key_data.get("unlimited") else "finite")
                                )
                                if wallet_payload:
                                    wallet.balance = wallet_payload.get("recharge_balance", 0) or 0
                                    wallet.gift_balance = wallet_payload.get("gift_balance", 0) or 0
                                    wallet.total_recharged = (
                                        wallet_payload.get("total_recharged", 0) or 0
                                    )
                                    wallet.total_consumed = (
                                        wallet_payload.get("total_consumed", 0) or 0
                                    )
                                    wallet.total_refunded = (
                                        wallet_payload.get("total_refunded", 0) or 0
                                    )
                                    wallet.total_adjusted = (
                                        wallet_payload.get("total_adjusted", 0) or 0
                                    )
                                    wallet.status = (
                                        wallet_payload.get("status", "active") or "active"
                                    )
                                wallet.updated_at = datetime.now(timezone.utc)
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
    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        """测试 SMTP 连接"""
        from src.core.crypto import crypto_service
        from src.services.email.email_sender import EmailSenderService
        from src.services.system.config import SystemConfigService

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
            "smtp_host": payload.get("smtp_host")
            or SystemConfigService.get_config(db, "smtp_host"),
            "smtp_port": payload.get("smtp_port")
            or SystemConfigService.get_config(db, "smtp_port", default=587),
            "smtp_user": payload.get("smtp_user")
            or SystemConfigService.get_config(db, "smtp_user"),
            "smtp_password": smtp_password,
            "smtp_use_tls": (
                payload.get("smtp_use_tls")
                if payload.get("smtp_use_tls") is not None
                else SystemConfigService.get_config(db, "smtp_use_tls", default=True)
            ),
            "smtp_use_ssl": (
                payload.get("smtp_use_ssl")
                if payload.get("smtp_use_ssl") is not None
                else SystemConfigService.get_config(db, "smtp_use_ssl", default=False)
            ),
            "smtp_from_email": payload.get("smtp_from_email")
            or SystemConfigService.get_config(db, "smtp_from_email"),
            "smtp_from_name": payload.get("smtp_from_name")
            or SystemConfigService.get_config(db, "smtp_from_name", default="Aether"),
        }

        # 验证必要配置
        missing_fields = [
            field
            for field in ["smtp_host", "smtp_user", "smtp_password", "smtp_from_email"]
            if not config.get(field)
        ]
        if missing_fields:
            return {
                "success": False,
                "message": f"SMTP 配置不完整，请检查 {', '.join(missing_fields)}",
            }

        # 测试连接
        try:
            success, error_msg = await EmailSenderService.test_smtp_connection(
                db=db, override_config=config
            )

            if success:
                return {"success": True, "message": "SMTP 连接测试成功"}
            else:
                return {"success": False, "message": error_msg}
        except Exception as e:
            return {"success": False, "message": str(e)}


# -------- 邮件模板适配器 --------


class AdminGetEmailTemplatesAdapter(AdminApiAdapter):
    """获取所有邮件模板"""

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
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

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
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

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
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

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
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

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
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


# -------- 数据清空适配器 --------


class AdminPurgeConfigAdapter(AdminApiAdapter):
    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        """清空所有提供商配置（Provider、Endpoint、API Key、Model、GlobalModel）"""
        from src.models.database import (
            GeminiFileMapping,
            GlobalModel,
            Model,
            ProviderAPIKey,
            ProviderEndpoint,
            UserPreference,
            VideoTask,
        )
        from src.models.database_extensions import ApiKeyProviderMapping, ProviderUsageTracking

        db = context.db

        # 统计
        providers_count = int(db.query(func.count(Provider.id)).scalar() or 0)
        endpoints_count = int(db.query(func.count(ProviderEndpoint.id)).scalar() or 0)
        keys_count = int(db.query(func.count(ProviderAPIKey.id)).scalar() or 0)
        models_count = int(db.query(func.count(Model.id)).scalar() or 0)
        global_models_count = int(db.query(func.count(GlobalModel.id)).scalar() or 0)

        # VideoTask 的 provider_id/endpoint_id/key_id 无 ondelete，置 NULL 保留任务记录
        db.query(VideoTask).filter(
            (VideoTask.provider_id.isnot(None))
            | (VideoTask.endpoint_id.isnot(None))
            | (VideoTask.key_id.isnot(None))
        ).update(
            {
                VideoTask.provider_id: None,
                VideoTask.endpoint_id: None,
                VideoTask.key_id: None,
            },
            synchronize_session=False,
        )

        # 先清理有外键引用的关联表
        db.query(GeminiFileMapping).delete()
        db.query(ApiKeyProviderMapping).delete()
        db.query(ProviderUsageTracking).delete()

        # 清空 UserPreference 中的 default_provider_id（无 ondelete 设置）
        db.query(UserPreference).filter(UserPreference.default_provider_id.isnot(None)).update(
            {UserPreference.default_provider_id: None}, synchronize_session=False
        )

        # 按依赖顺序删除配置
        db.query(Model).delete()
        db.query(ProviderAPIKey).delete()
        db.query(ProviderEndpoint).delete()
        db.query(Provider).delete()
        db.query(GlobalModel).delete()
        db.commit()

        return {
            "message": "配置已清空",
            "deleted": {
                "providers": providers_count,
                "endpoints": endpoints_count,
                "api_keys": keys_count,
                "models": models_count,
                "global_models": global_models_count,
            },
        }


class AdminPurgeUsersAdapter(AdminApiAdapter):
    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        """清空所有非管理员用户及其关联数据"""
        from src.core.enums import UserRole

        db = context.db

        user_ids = [uid for (uid,) in db.query(User.id).filter(User.role != UserRole.ADMIN).all()]
        users_count = len(user_ids)

        if user_ids:
            from src.models.database import VideoTask

            # 删除关联的 VideoTask（user_id 无 ondelete 设置）
            db.query(VideoTask).filter(VideoTask.user_id.in_(user_ids)).delete(
                synchronize_session=False
            )

            # 统计关联 API Keys 数量（DB 级别 CASCADE 会随 User 自动删除）
            keys_count = int(
                db.query(func.count(ApiKey.id)).filter(ApiKey.user_id.in_(user_ids)).scalar() or 0
            )

            # 将使用记录的 user_id 置空（保留记录）
            db.query(Usage).filter(Usage.user_id.in_(user_ids)).update(
                {Usage.user_id: None}, synchronize_session=False
            )

            # 删除用户
            db.query(User).filter(User.id.in_(user_ids)).delete(synchronize_session=False)
            db.commit()
        else:
            keys_count = 0

        return {
            "message": "非管理员用户已清空",
            "deleted": {
                "users": users_count,
                "api_keys": keys_count,
            },
        }


def _purge_stats_and_reset_counters(db: Session) -> None:
    """清空预聚合统计表、重置累计计数字段、清除缓存。"""
    from src.models.database import (
        ProviderAPIKey,
        StatsDaily,
        StatsDailyApiKey,
        StatsDailyError,
        StatsDailyModel,
        StatsDailyProvider,
        StatsHourly,
        StatsHourlyModel,
        StatsHourlyProvider,
        StatsHourlyUser,
        StatsSummary,
        StatsUserDaily,
    )
    from src.services.cache.invalidation import get_cache_invalidation_service

    # 清空预聚合统计表
    db.query(StatsHourly).delete()
    db.query(StatsHourlyUser).delete()
    db.query(StatsHourlyModel).delete()
    db.query(StatsHourlyProvider).delete()
    db.query(StatsDaily).delete()
    db.query(StatsDailyModel).delete()
    db.query(StatsDailyProvider).delete()
    db.query(StatsDailyApiKey).delete()
    db.query(StatsDailyError).delete()
    db.query(StatsSummary).delete()
    db.query(StatsUserDaily).delete()

    # 重置 ApiKey 上的缓存统计字段
    db.query(ApiKey).update(
        {
            ApiKey.total_requests: 0,
            ApiKey.total_cost_usd: 0.0,
        },
        synchronize_session=False,
    )

    # 重置 ProviderAPIKey 上的使用统计
    db.query(ProviderAPIKey).update(
        {
            ProviderAPIKey.request_count: 0,
            ProviderAPIKey.success_count: 0,
            ProviderAPIKey.error_count: 0,
            ProviderAPIKey.total_response_time_ms: 0,
            ProviderAPIKey.last_used_at: None,
            ProviderAPIKey.last_error_at: None,
        },
        synchronize_session=False,
    )

    # 清除缓存
    try:
        cache_service = get_cache_invalidation_service()
        cache_service.clear_all_caches()
    except Exception:
        pass


class AdminPurgeUsageAdapter(AdminApiAdapter):
    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        """清空全部使用记录及相关统计数据"""
        from src.models.database import RequestCandidate, UserModelUsageCount

        db = context.db

        usage_count = int(db.query(func.count(Usage.id)).scalar() or 0)
        candidates_count = int(db.query(func.count(RequestCandidate.id)).scalar() or 0)
        usage_counts_count = int(db.query(func.count(UserModelUsageCount.id)).scalar() or 0)

        # 清空使用记录
        db.query(RequestCandidate).delete()
        db.query(Usage).delete()
        db.query(UserModelUsageCount).delete()

        _purge_stats_and_reset_counters(db)
        db.commit()

        return {
            "message": "使用记录已清空",
            "deleted": {
                "usage_records": usage_count,
                "request_candidates": candidates_count,
                "user_model_usage_counts": usage_counts_count,
            },
        }


class AdminPurgeAuditLogsAdapter(AdminApiAdapter):
    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        """清空全部审计日志"""
        from src.models.database import AuditLog

        db = context.db

        count = int(db.query(func.count(AuditLog.id)).scalar() or 0)
        db.query(AuditLog).delete()
        db.commit()

        return {
            "message": "审计日志已清空",
            "deleted": {
                "audit_logs": count,
            },
        }


class AdminPurgeRequestBodiesAdapter(AdminApiAdapter):
    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        """清空全部请求体/响应体（保留使用记录的统计信息）"""
        db = context.db

        # 统计有 body 的记录数
        with_body = int(
            db.query(func.count(Usage.id))
            .filter(
                (Usage.request_body.isnot(None))
                | (Usage.response_body.isnot(None))
                | (Usage.provider_request_body.isnot(None))
                | (Usage.client_response_body.isnot(None))
                | (Usage.request_body_compressed.isnot(None))
                | (Usage.response_body_compressed.isnot(None))
                | (Usage.provider_request_body_compressed.isnot(None))
                | (Usage.client_response_body_compressed.isnot(None))
            )
            .scalar()
            or 0
        )

        # 批量清空所有 body 字段
        db.query(Usage).update(
            {
                Usage.request_body: None,
                Usage.response_body: None,
                Usage.provider_request_body: None,
                Usage.client_response_body: None,
                Usage.request_body_compressed: None,
                Usage.response_body_compressed: None,
                Usage.provider_request_body_compressed: None,
                Usage.client_response_body_compressed: None,
                Usage.request_headers: None,
                Usage.response_headers: None,
                Usage.provider_request_headers: None,
                Usage.client_response_headers: None,
            },
            synchronize_session=False,
        )
        db.commit()

        return {
            "message": "请求体已清空",
            "cleaned": {
                "records_with_body": with_body,
            },
        }


class AdminPurgeStatsAdapter(AdminApiAdapter):
    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        """清空全部聚合统计数据（保留原始使用记录）"""
        db = context.db

        _purge_stats_and_reset_counters(db)
        db.commit()

        return {
            "message": "聚合统计数据已清空",
        }


# ---------------------------------------------------------------------------
# AWS Regions (从 AWS Regional Table API 获取，Redis 缓存 24h)
# ---------------------------------------------------------------------------

_AWS_REGIONS_CACHE_KEY = "aws_regions"
_AWS_REGIONS_CACHE_TTL = 86400  # 24h
_AWS_REGIONAL_TABLE_URL = "https://api.regional-table.region-services.aws.a2z.com"

# 内存级 fallback（进程生命周期内有效，Redis 不可用时兜底）
_aws_regions_mem_cache: list[str] | None = None


async def _fetch_aws_regions() -> list[str]:
    """从 AWS Regional Table API 提取去重排序的 region 列表"""
    import httpx as _httpx

    from src.clients.http_client import HTTPClientPool

    client = await HTTPClientPool.get_default_client_async()
    resp = await client.get(
        _AWS_REGIONAL_TABLE_URL,
        timeout=_httpx.Timeout(connect=10, read=15),
    )
    resp.raise_for_status()
    data = resp.json()
    regions: set[str] = set()
    for item in data.get("prices", []):
        region = item.get("attributes", {}).get("aws:region", "")
        if region:
            regions.add(region)
    return sorted(regions)


@router.get("/aws-regions")
async def get_aws_regions() -> Any:
    """获取 AWS 全部可用 Region 列表（缓存 24h）"""
    global _aws_regions_mem_cache

    # 1. 尝试 Redis 缓存
    from src.core.cache_service import CacheService

    cached = await CacheService.get(_AWS_REGIONS_CACHE_KEY)
    if cached and isinstance(cached, list):
        return {"regions": cached}

    # 2. 尝试内存 fallback
    if _aws_regions_mem_cache:
        return {"regions": _aws_regions_mem_cache}

    # 3. 远程获取
    try:
        regions = await _fetch_aws_regions()
    except Exception as e:
        logger.warning("获取 AWS Regions 失败: {}", e)
        # 返回最基础的 fallback
        return {"regions": ["us-east-1", "us-east-2", "us-west-1", "us-west-2", "eu-north-1"]}

    # 写入缓存
    _aws_regions_mem_cache = regions
    await CacheService.set(_AWS_REGIONS_CACHE_KEY, regions, ttl_seconds=_AWS_REGIONS_CACHE_TTL)

    return {"regions": regions}
