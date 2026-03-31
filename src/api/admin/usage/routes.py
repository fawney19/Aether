"""管理员使用情况统计路由。"""

from __future__ import annotations

from dataclasses import dataclass
from datetime import datetime, timezone
from typing import Any

from fastapi import APIRouter, Depends, HTTPException, Query, Request
from sqlalchemy import case, func
from sqlalchemy.orm import Session, defer

from src.api.base.admin_adapter import AdminApiAdapter
from src.api.base.context import ApiRequestContext
from src.api.base.pipeline import get_pipeline
from src.core.logger import logger
from src.database import get_db
from src.models.database import (
    ApiKey,
    Provider,
    ProviderAPIKey,
    ProviderEndpoint,
    Usage,
    User,
)

router = APIRouter(prefix="/api/admin/usage", tags=["Admin - Usage"])
pipeline = get_pipeline()



@router.get("/{usage_id}/curl")
async def get_usage_curl_data(
    usage_id: str,
    request: Request,
    db: Session = Depends(get_db),
) -> Any:
    """
    获取使用记录的 cURL 命令数据

    返回重建 cURL 命令所需的 URL、请求头（含明文 API Key）和请求体。

    **路径参数**:
    - `usage_id`: 使用记录 ID

    **返回字段**:
    - `url`: 提供商请求 URL
    - `method`: HTTP 方法
    - `headers`: 提供商请求头（含明文 API Key）
    - `body`: 请求体
    - `curl`: 生成的 cURL 命令字符串
    """
    adapter = AdminUsageCurlAdapter(usage_id=usage_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.post("/{usage_id}/replay")
async def replay_usage_request(
    usage_id: str,
    request: Request,
    db: Session = Depends(get_db),
) -> Any:
    """
    回放使用记录请求

    将原始请求重新发送到原始或指定的提供商，并返回响应结果。

    **路径参数**:
    - `usage_id`: 使用记录 ID

    **请求体**:
    - `provider_id`: 可选，目标提供商 ID（不指定则使用原始提供商）
    - `endpoint_id`: 可选，目标端点 ID（不指定则使用原始端点）
    - `body_override`: 可选，覆盖原始请求体

    **返回字段**:
    - `url`: 请求 URL
    - `status_code`: HTTP 状态码
    - `response_headers`: 响应头
    - `response_body`: 响应体
    - `response_time_ms`: 响应时间（毫秒）
    """
    # 从 JSON body 中解析参数
    try:
        json_body = await request.json()
    except Exception:
        json_body = {}

    adapter = AdminUsageReplayAdapter(
        usage_id=usage_id,
        target_provider_id=json_body.get("provider_id"),
        target_endpoint_id=json_body.get("endpoint_id"),
        target_api_key_id=json_body.get("api_key_id"),
        body_override=json_body.get("body_override"),
    )
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


# NOTE: This route must be defined AFTER all other routes to avoid matching
# routes like /curl and /replay.
@router.get("/{usage_id}")
async def get_usage_detail(
    usage_id: str,
    request: Request,
    include_bodies: bool = Query(True, description="是否返回请求/响应 body 内容"),
    db: Session = Depends(get_db),
) -> Any:
    """
    获取使用记录详情

    获取指定使用记录的详细信息，包括请求/响应的头部和正文。

    **路径参数**:
    - `usage_id`: 使用记录 ID

    **返回字段**:
    - `id`: 记录 ID
    - `request_id`: 请求 ID
    - `user`: 用户信息（id, username, email）
    - `api_key`: API Key 信息（id, name, display）
    - `provider`: 提供商名称
    - `api_format`: API 格式
    - `model`: 请求的模型名称
    - `target_model`: 映射后的目标模型名称
    - `tokens`: Token 统计（含 input/output/cache_creation/cache_read/total）
    - `cost`: 成本统计（含 input/output/cache_creation/cache_read/request/total）
    - `pricing`: 历史价格快照
    - `cache`: 缓存账单结构（ttl_minutes/creation_input_tokens/read_input_tokens/...）
    - `request_type`: 请求类型
    - `is_stream`: 是否为流式请求
    - `status_code`: HTTP 状态码
    - `error_message`: 错误信息
    - `response_time_ms`: 响应时间（毫秒）
    - `first_byte_time_ms`: 首字节时间（TTFB，毫秒）
    - `created_at`: 创建时间
    - `request_headers`: 请求头
    - `request_body`: 请求体
    - `provider_request_headers`: 提供商请求头
    - `response_headers`: 提供商响应头
    - `client_response_headers`: 返回给客户端的响应头
    - `response_body`: 响应体
    - `metadata`: 提供商响应元数据
    - `tiered_pricing`: 阶梯计费信息（如适用）
    """
    adapter = AdminUsageDetailAdapter(usage_id=usage_id, include_bodies=include_bodies)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@dataclass
class AdminUsageDetailAdapter(AdminApiAdapter):
    """Get detailed usage record with request/response body"""

    usage_id: str
    include_bodies: bool = True

    def _build_usage_detail_query(self, db: Session) -> Any:
        query = db.query(
            Usage,
            case(
                (
                    (Usage.request_body.isnot(None)) | (Usage.request_body_compressed.isnot(None)),
                    True,
                ),
                else_=False,
            ).label("has_request_body"),
            case(
                (
                    (Usage.provider_request_body.isnot(None))
                    | (Usage.provider_request_body_compressed.isnot(None)),
                    True,
                ),
                else_=False,
            ).label("has_provider_request_body"),
            case(
                (
                    (Usage.response_body.isnot(None))
                    | (Usage.response_body_compressed.isnot(None)),
                    True,
                ),
                else_=False,
            ).label("has_response_body"),
            case(
                (
                    (Usage.client_response_body.isnot(None))
                    | (Usage.client_response_body_compressed.isnot(None)),
                    True,
                ),
                else_=False,
            ).label("has_client_response_body"),
        )

        if not self.include_bodies:
            query = query.options(
                defer(Usage.request_body),
                defer(Usage.provider_request_body),
                defer(Usage.response_body),
                defer(Usage.client_response_body),
                defer(Usage.request_body_compressed),
                defer(Usage.provider_request_body_compressed),
                defer(Usage.response_body_compressed),
                defer(Usage.client_response_body_compressed),
            )

        return query

    def _load_usage_detail_row(self, db: Session) -> Any:
        usage_row = self._build_usage_detail_query(db).filter(Usage.id == self.usage_id).first()
        if usage_row:
            return usage_row
        return self._build_usage_detail_query(db).filter(Usage.request_id == self.usage_id).first()

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        db = context.db
        # 先通过主键 id 查找，如果找不到再尝试通过 request_id 查找
        usage_row = self._load_usage_detail_row(db)
        if not usage_row:
            raise HTTPException(status_code=404, detail="Usage record not found")

        (
            usage_record,
            has_request_body,
            has_provider_request_body,
            has_response_body,
            has_client_response_body,
        ) = usage_row

        user = (
            db.query(User).filter(User.id == usage_record.user_id).first()
            if usage_record.user_id
            else None
        )
        api_key = (
            db.query(ApiKey).filter(ApiKey.id == usage_record.api_key_id).first()
            if usage_record.api_key_id
            else None
        )
        provider = (
            db.query(Provider).filter(Provider.id == getattr(usage_record, "provider_id")).first()
            if getattr(usage_record, "provider_id", None)
            else None
        )
        provider_api_key = (
            db.query(ProviderAPIKey)
            .filter(ProviderAPIKey.id == usage_record.provider_api_key_id)
            .first()
            if getattr(usage_record, "provider_api_key_id", None)
            else None
        )

        # 获取阶梯计费信息
        tiered_pricing_info = await self._get_tiered_pricing_info(db, usage_record)

        context.add_audit_metadata(
            action="usage_detail",
            usage_id=self.usage_id,
        )

        # 提取视频/图像/音频计费信息
        video_billing_info = self._extract_video_billing_info(usage_record)

        request_body = usage_record.get_request_body() if self.include_bodies else None
        provider_request_body = (
            usage_record.get_provider_request_body() if self.include_bodies else None
        )
        response_body = usage_record.get_response_body() if self.include_bodies else None
        client_response_body = (
            usage_record.get_client_response_body() if self.include_bodies else None
        )

        return {
            "id": usage_record.id,
            "request_id": usage_record.request_id,
            "user": {
                "id": user.id if user else usage_record.user_id,
                "username": (
                    user.username
                    if user
                    else getattr(usage_record, "username", None) or "已删除用户"
                ),
                "email": user.email if user else None,
            },
            "api_key": {
                "id": api_key.id if api_key else usage_record.api_key_id,
                "name": (
                    api_key.name
                    if api_key
                    else getattr(usage_record, "api_key_name", None)
                    or ("已删除Key" if usage_record.api_key_id is None else None)
                ),
                "display": api_key.get_display_key() if api_key else None,
            },
            "provider": provider.name if provider else usage_record.provider_name,
            "provider_api_key": {
                "id": (
                    provider_api_key.id
                    if provider_api_key
                    else getattr(usage_record, "provider_api_key_id", None)
                ),
                "name": provider_api_key.name if provider_api_key else None,
            },
            "api_format": usage_record.api_format,
            "model": usage_record.model,
            "target_model": usage_record.target_model,
            "tokens": {
                "input": usage_record.input_tokens,
                "output": usage_record.output_tokens,
                "input_output_total": usage_record.input_output_total_tokens,
                "input_context": usage_record.input_context_tokens,
                "cache_creation": usage_record.cache_creation_input_tokens,
                "cache_read": usage_record.cache_read_input_tokens,
                "total": usage_record.total_tokens,
            },
            "cost": {
                "input": float(usage_record.input_cost_usd or 0),
                "output": float(usage_record.output_cost_usd or 0),
                "cache_creation": float(getattr(usage_record, "cache_creation_cost_usd", 0) or 0),
                "cache_read": float(getattr(usage_record, "cache_read_cost_usd", 0) or 0),
                "request": float(getattr(usage_record, "request_cost_usd", 0) or 0),
                "total": float(usage_record.total_cost_usd or 0),
            },
            "pricing": {
                "input_price_per_1m": (
                    float(usage_record.input_price_per_1m)
                    if usage_record.input_price_per_1m is not None
                    else None
                ),
                "output_price_per_1m": (
                    float(usage_record.output_price_per_1m)
                    if usage_record.output_price_per_1m is not None
                    else None
                ),
                "cache_creation_price_per_1m": (
                    float(usage_record.cache_creation_price_per_1m)
                    if usage_record.cache_creation_price_per_1m is not None
                    else None
                ),
                "cache_read_price_per_1m": (
                    float(usage_record.cache_read_price_per_1m)
                    if usage_record.cache_read_price_per_1m is not None
                    else None
                ),
                "price_per_request": (
                    float(usage_record.price_per_request)
                    if usage_record.price_per_request is not None
                    else None
                ),
            },
            "cache": {
                "ttl_minutes": (
                    int(getattr(usage_record, "cache_ttl_minutes", 5) or 5)
                    if (
                        (usage_record.cache_creation_input_tokens or 0) > 0
                        or (usage_record.cache_read_input_tokens or 0) > 0
                    )
                    else None
                ),
                "creation_input_tokens": usage_record.cache_creation_input_tokens or 0,
                "read_input_tokens": usage_record.cache_read_input_tokens or 0,
                "creation_cost": float(getattr(usage_record, "cache_creation_cost_usd", 0) or 0),
                "read_cost": float(getattr(usage_record, "cache_read_cost_usd", 0) or 0),
                "creation_price_per_1m": (
                    float(usage_record.cache_creation_price_per_1m)
                    if usage_record.cache_creation_price_per_1m is not None
                    else None
                ),
                "read_price_per_1m": (
                    float(usage_record.cache_read_price_per_1m)
                    if usage_record.cache_read_price_per_1m is not None
                    else None
                ),
            },
            "request_type": usage_record.request_type,
            "is_stream": usage_record.is_stream,
            "status_code": usage_record.status_code,
            "error_message": usage_record.error_message,
            "status": usage_record.status,
            "response_time_ms": usage_record.response_time_ms,
            "first_byte_time_ms": usage_record.first_byte_time_ms,  # 首字时间 (TTFB)
            "created_at": usage_record.created_at.isoformat() if usage_record.created_at else None,
            "has_request_body": has_request_body,
            "has_provider_request_body": has_provider_request_body,
            "has_response_body": has_response_body,
            "has_client_response_body": has_client_response_body,
            "request_headers": usage_record.request_headers,
            "request_body": request_body,
            "provider_request_headers": usage_record.provider_request_headers,
            "provider_request_body": provider_request_body,
            "response_headers": usage_record.response_headers,
            "client_response_headers": usage_record.client_response_headers,
            "response_body": response_body,
            "client_response_body": client_response_body,
            "metadata": usage_record.request_metadata,
            "tiered_pricing": tiered_pricing_info,
            "video_billing": video_billing_info,
        }

    async def _get_tiered_pricing_info(self, db: Session, usage_record: Any) -> dict | None:
        """获取阶梯计费信息"""
        from src.services.model.cost import ModelCostService

        # 计算总输入上下文（用于阶梯判定）：输入 + 缓存创建 + 缓存读取
        input_tokens = usage_record.input_tokens or 0
        cache_creation_tokens = usage_record.cache_creation_input_tokens or 0
        cache_read_tokens = usage_record.cache_read_input_tokens or 0
        total_input_context = input_tokens + cache_creation_tokens + cache_read_tokens

        # 尝试获取模型的阶梯配置（带来源信息）
        cost_service = ModelCostService(db)
        pricing_result = await cost_service.get_tiered_pricing_with_source_async(
            usage_record.provider_name, usage_record.model
        )

        if not pricing_result:
            return None

        tiered_pricing = pricing_result.get("pricing")
        pricing_source = pricing_result.get("source")  # 'provider' 或 'global'

        if not tiered_pricing or not tiered_pricing.get("tiers"):
            return None

        tiers = tiered_pricing.get("tiers", [])
        if not tiers:
            return None

        # 找到命中的阶梯
        tier_index = None
        matched_tier = None
        for i, tier in enumerate(tiers):
            up_to = tier.get("up_to")
            if up_to is None or total_input_context <= up_to:
                tier_index = i
                matched_tier = tier
                break

        # 如果都没匹配，使用最后一个阶梯
        if tier_index is None and tiers:
            tier_index = len(tiers) - 1
            matched_tier = tiers[-1]

        return {
            "total_input_context": total_input_context,
            "tier_index": tier_index,
            "tier_count": len(tiers),
            "current_tier": matched_tier,
            "tiers": tiers,
            "source": pricing_source,  # 定价来源: 'provider' 或 'global'
        }

    def _extract_video_billing_info(self, usage_record: Any) -> dict | None:
        """
        从 request_metadata.billing_snapshot 和 dimensions 中提取视频/图像/音频计费信息。

        返回结构:
        {
            "task_type": "video" | "image" | "audio",
            "duration_seconds": 10.5,  # 视频时长（秒）
            "resolution": "1080p",     # 分辨率
            "video_price_per_second": 0.1,  # 每秒单价
            "video_cost": 1.05,        # 视频费用
            "rule_name": "...",        # 计费规则名称
            "expression": "...",       # 计费公式
            "status": "complete",      # 计费状态
        }
        """
        request_type = getattr(usage_record, "request_type", None)
        if request_type not in {"video", "image", "audio"}:
            return None

        metadata = getattr(usage_record, "request_metadata", None)
        if not metadata:
            return None

        billing_snapshot = metadata.get("billing_snapshot") if isinstance(metadata, dict) else None
        dimensions = metadata.get("dimensions") if isinstance(metadata, dict) else None

        result: dict = {
            "task_type": request_type,
        }

        # 从 billing_snapshot 中提取计费规则信息
        if billing_snapshot and isinstance(billing_snapshot, dict):
            result["rule_name"] = billing_snapshot.get("rule_name")
            result["expression"] = billing_snapshot.get("expression")
            result["status"] = billing_snapshot.get("status")
            result["cost"] = billing_snapshot.get("cost")

            # 从 dimensions_used 中提取维度
            dims_used = billing_snapshot.get("dimensions_used")
            if dims_used and isinstance(dims_used, dict):
                if "duration_seconds" in dims_used:
                    result["duration_seconds"] = dims_used["duration_seconds"]
                if "video_resolution_key" in dims_used:
                    result["resolution"] = dims_used["video_resolution_key"]
                if "video_price_per_second" in dims_used:
                    result["video_price_per_second"] = dims_used["video_price_per_second"]
                if "video_cost" in dims_used:
                    result["video_cost"] = dims_used["video_cost"]

        # 补充从 dimensions 中提取（备用）
        if dimensions and isinstance(dimensions, dict):
            if "duration_seconds" not in result and "duration_seconds" in dimensions:
                result["duration_seconds"] = dimensions["duration_seconds"]
            if "resolution" not in result and "video_resolution_key" in dimensions:
                result["resolution"] = dimensions["video_resolution_key"]

        # 如果没有有意义的视频计费信息，返回 None
        has_video_info = (
            result.get("duration_seconds")
            or result.get("resolution")
            or result.get("video_cost")
            or result.get("cost")
        )
        if not has_video_info:
            return None

        return result


# ==================== cURL 导出 & 请求回放 ====================


def _find_usage_record(db: Session, usage_id: str) -> Usage:
    """按 id 或 request_id 查找 Usage 记录，找不到则抛 404。"""
    record = db.query(Usage).filter(Usage.id == usage_id).first()
    if not record:
        record = db.query(Usage).filter(Usage.request_id == usage_id).first()
    if not record:
        raise HTTPException(status_code=404, detail="Usage record not found")
    return record


def _build_provider_url_safe(
    endpoint: ProviderEndpoint,
    model_name: str | None,
    is_stream: bool,
    provider_key: ProviderAPIKey | None,
    decrypted_auth_config: dict[str, Any] | None = None,
) -> str:
    """构建 Provider URL，build_provider_url 失败时回退到 base_url + custom_path/默认路径。"""
    from src.services.provider.transport import build_provider_url

    try:
        return build_provider_url(
            endpoint,
            path_params={"model": model_name} if model_name else None,
            is_stream=is_stream,
            key=provider_key,
            decrypted_auth_config=decrypted_auth_config,
        )
    except Exception:
        base = (endpoint.base_url or "").rstrip("/")
        if endpoint.custom_path:
            return f"{base}{endpoint.custom_path}"
        # 尝试使用 API 格式的默认路径
        try:
            from src.core.api_format.metadata import get_default_path_for_endpoint

            ep_sig = (getattr(endpoint, "api_format", "") or "").strip().lower()
            if ep_sig:
                path = get_default_path_for_endpoint(ep_sig)
                if model_name:
                    # Gemini 路径含 {action}，回退时固定用非流式操作
                    action = "streamGenerateContent" if is_stream else "generateContent"
                    try:
                        path = path.format(model=model_name, action=action)
                    except KeyError:
                        pass
                elif "{model}" in path:
                    # model 为空且路径含模板变量，无法构造有效路径，回退到纯 base URL
                    return base
                return f"{base}{path}"
        except Exception:
            pass
        return base


def _build_fresh_headers(
    auth_headers: dict[str, str],
    endpoint: ProviderEndpoint,
) -> dict[str, str]:
    """从零构建请求头：Content-Type + 认证头 + endpoint header_rules 额外头。"""
    from src.core.api_format.headers import get_extra_headers_from_endpoint

    headers: dict[str, str] = {"Content-Type": "application/json"}
    headers.update(auth_headers)
    extra = get_extra_headers_from_endpoint(endpoint)
    if extra:
        headers.update(extra)
    return headers


async def _resolve_provider_auth(
    provider_key: ProviderAPIKey,
    endpoint: ProviderEndpoint,
    db: Session,
) -> tuple[dict[str, str], dict[str, Any] | None]:
    """解析 Provider Key 的认证信息，返回 (认证头字典, 解密后的 auth_config)。

    支持: api_key / oauth / vertex_ai 三种 auth_type。
    """
    from src.core.api_format.metadata import get_auth_config_for_endpoint
    from src.core.crypto import crypto_service

    auth_type = str(getattr(provider_key, "auth_type", "api_key") or "api_key").lower()
    auth_headers: dict[str, str] = {}
    decrypted_auth_config: dict[str, Any] | None = None

    if auth_type == "oauth":
        from src.services.provider.oauth_token import resolve_oauth_access_token
        from src.services.proxy_node.resolver import resolve_effective_proxy

        # 获取 Provider 对象以读取 proxy 和 provider_type
        provider_obj = db.query(Provider).filter(Provider.id == provider_key.provider_id).first()
        provider_type = (
            str(getattr(provider_obj, "provider_type", "") or "").lower() if provider_obj else ""
        )

        # Antigravity 使用 gemini:chat 端点格式
        ep_format = str(getattr(endpoint, "api_format", "") or "")
        if provider_type == "antigravity" and not ep_format:
            ep_format = "gemini:chat"

        resolved = await resolve_oauth_access_token(
            key_id=str(provider_key.id),
            encrypted_api_key=str(provider_key.api_key or ""),
            encrypted_auth_config=(
                str(provider_key.auth_config)
                if getattr(provider_key, "auth_config", None) is not None
                else None
            ),
            provider_proxy_config=(
                resolve_effective_proxy(
                    getattr(provider_obj, "proxy", None),
                    getattr(provider_key, "proxy", None),
                )
                if provider_obj
                else None
            ),
            endpoint_api_format=ep_format,
        )
        access_token = resolved.access_token or ""
        auth_headers["Authorization"] = f"Bearer {access_token}"
        decrypted_auth_config = resolved.decrypted_auth_config

        # Codex 等需要 account_id
        if decrypted_auth_config:
            account_id = decrypted_auth_config.get("account_id")
            if account_id:
                auth_headers["chatgpt-account-id"] = str(account_id)

    elif auth_type in ("service_account", "vertex_ai"):
        from src.api.handlers.base.request_builder import get_provider_auth

        auth_info = await get_provider_auth(endpoint, provider_key)
        if auth_info:
            auth_headers[auth_info.auth_header] = auth_info.auth_value
        else:
            # 回退
            decrypted_key = crypto_service.decrypt(provider_key.api_key)
            auth_headers["Authorization"] = f"Bearer {decrypted_key}"

    else:
        # 标准 API Key
        decrypted_key = crypto_service.decrypt(provider_key.api_key)

        # 根据 endpoint signature 确定认证头名称和类型
        api_family = str(getattr(endpoint, "api_family", "") or "").lower()
        api_kind = str(getattr(endpoint, "endpoint_kind", "") or "").lower()
        if api_family and api_kind:
            endpoint_sig = f"{api_family}:{api_kind}"
        else:
            endpoint_sig = str(getattr(endpoint, "api_format", "") or "") or "openai:chat"

        auth_header, auth_type_cfg = get_auth_config_for_endpoint(endpoint_sig)
        auth_value = f"Bearer {decrypted_key}" if auth_type_cfg == "bearer" else decrypted_key
        auth_headers[auth_header] = auth_value

    return auth_headers, decrypted_auth_config


@dataclass
class AdminUsageCurlAdapter(AdminApiAdapter):
    """Generate cURL command data from a usage record."""

    usage_id: str

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        import json as _json
        import shlex

        db = context.db
        usage_record = _find_usage_record(db, self.usage_id)

        # 获取端点和密钥
        endpoint_id = usage_record.provider_endpoint_id
        key_id = usage_record.provider_api_key_id

        # 兜底：Usage 记录缺少 provider 信息时，从 RequestCandidate 表查找
        if not endpoint_id or not key_id:
            from src.models.database import RequestCandidate as RC

            candidate = (
                db.query(RC)
                .filter(
                    RC.request_id == usage_record.request_id,
                    RC.status.in_(["success", "failed", "streaming"]),
                )
                .order_by(RC.candidate_index.desc(), RC.retry_index.desc())
                .first()
            )
            if candidate:
                endpoint_id = endpoint_id or candidate.endpoint_id
                key_id = key_id or candidate.key_id

        endpoint = None
        if endpoint_id:
            endpoint = db.query(ProviderEndpoint).filter(ProviderEndpoint.id == endpoint_id).first()
        provider_key = None
        if key_id:
            provider_key = db.query(ProviderAPIKey).filter(ProviderAPIKey.id == key_id).first()

        # 解析认证信息
        stored_headers = usage_record.provider_request_headers or {}
        headers: dict[str, str] = {}
        auth_headers: dict[str, str] = {}
        decrypted_auth_config: dict[str, Any] | None = None

        if provider_key and endpoint:
            try:
                auth_headers, decrypted_auth_config = await _resolve_provider_auth(
                    provider_key, endpoint, db
                )

                if stored_headers:
                    # 有存储的请求头：替换被脱敏的认证头为真实值
                    headers = dict(stored_headers)
                    auth_lower_keys = {k.lower() for k in auth_headers}
                    for key_name in list(headers.keys()):
                        if key_name.lower() in auth_lower_keys:
                            del headers[key_name]
                    headers.update(auth_headers)
                else:
                    headers = _build_fresh_headers(auth_headers, endpoint)
            except Exception:
                headers = dict(stored_headers)
        else:
            headers = dict(stored_headers)

        # 重建请求 URL（在认证解析之后，以便传递 decrypted_auth_config 给 Vertex AI 等场景）
        url: str | None = None
        if endpoint:
            model_name = usage_record.target_model or usage_record.model
            url = _build_provider_url_safe(
                endpoint,
                model_name,
                usage_record.is_stream or False,
                provider_key,
                decrypted_auth_config,
            )

        # 确保始终有 Content-Type
        if not any(k.lower() == "content-type" for k in headers):
            headers["Content-Type"] = "application/json"

        # 获取请求体
        body = usage_record.get_request_body()

        # 生成 cURL 命令
        curl_parts = ["curl"]
        if url:
            curl_parts.append(shlex.quote(url))
        curl_parts.append("-X POST")

        for h_key, h_value in headers.items():
            curl_parts.append(f"-H {shlex.quote(f'{h_key}: {h_value}')}")

        if body:
            body_str = _json.dumps(body, ensure_ascii=False)
            curl_parts.append(f"-d {shlex.quote(body_str)}")

        curl_command = " \\\n  ".join(curl_parts)

        context.add_audit_metadata(
            action="usage_curl",
            usage_id=self.usage_id,
        )

        return {
            "url": url,
            "method": "POST",
            "headers": headers,
            "body": body,
            "curl": curl_command,
        }


def _resolve_replay_mode(same_provider: bool, same_endpoint: bool) -> str:
    if same_provider and same_endpoint:
        return "same_endpoint_reuse"
    if same_provider:
        return "same_provider_remap"
    return "cross_provider_remap"


async def _resolve_replay_model_name(
    db: Session,
    *,
    source_model: str,
    target_provider: Provider,
    target_endpoint: ProviderEndpoint,
    target_api_key: ProviderAPIKey | None,
) -> tuple[str, str]:
    """按当前 replay 目标重新解析模型名，并返回 mapping_source。"""
    from src.services.model.mapper import ModelMapperMiddleware

    target_api_format = (getattr(target_endpoint, "api_format", "") or "").strip().lower()

    mapper = ModelMapperMiddleware(db)
    mapping = await mapper.get_mapping(source_model, str(target_provider.id))

    if mapping and mapping.model:
        affinity_key = target_api_key.id if target_api_key else None
        mapped_name = mapping.model.select_provider_model_name(
            affinity_key, api_format=target_api_format
        )
        return mapped_name, "model_mapping"

    logger.debug(
        "[replay] No explicit model mapping for '{}' on provider '{}' (endpoint={}, api_format={}); "
        "forwarding original source model",
        source_model,
        target_provider.name or str(target_provider.id),
        str(getattr(target_endpoint, "id", "") or "unknown"),
        target_api_format or "unknown",
    )

    # Keep replay aligned with the normal request path: if no global-model mapping exists,
    # forward the original source model name and let the target provider validate it.
    return source_model, "none"


def _apply_replay_model_to_body(
    body: dict[str, Any],
    resolved_model: str,
    target_api_format: str | None,
) -> None:
    """根据目标格式决定是否写入 body.model。"""
    from src.core.api_format.metadata import resolve_endpoint_definition

    target_meta = resolve_endpoint_definition(target_api_format) if target_api_format else None
    if target_meta is not None and not target_meta.model_in_body:
        body.pop("model", None)
        return

    body["model"] = resolved_model


@dataclass
class AdminUsageReplayAdapter(AdminApiAdapter):
    """Replay a usage record request to the same or a different provider."""

    usage_id: str
    target_provider_id: str | None = None
    target_endpoint_id: str | None = None
    target_api_key_id: str | None = None
    body_override: dict | None = None

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        import time

        import httpx

        db = context.db
        usage_record = _find_usage_record(db, self.usage_id)

        # 确定原始请求的 API 格式（用于端点/Key 匹配）
        original_api_format = (
            (usage_record.endpoint_api_format or usage_record.api_format or "").strip().lower()
        )
        original_api_family = (
            original_api_format.split(":")[0] if ":" in original_api_format else ""
        )
        original_request_body = usage_record.get_request_body()
        body_override_payload = self.body_override if isinstance(self.body_override, dict) else None
        override_model: str | None = None
        if isinstance(body_override_payload, dict):
            override_val = body_override_payload.get("model")
            if isinstance(override_val, str) and override_val.strip():
                override_model = override_val.strip()

        # 解析源模型名：优先 request_body.model，缺失时回退 usage_record.model
        source_model = usage_record.model
        if override_model:
            source_model = override_model
        elif isinstance(original_request_body, dict):
            raw_model = original_request_body.get("model")
            if isinstance(raw_model, str) and raw_model.strip():
                source_model = raw_model.strip()

        original_target_model = usage_record.target_model

        # 确定目标端点
        target_pid = self.target_provider_id
        target_provider_obj: Provider | None = None
        if self.target_endpoint_id:
            endpoint = (
                db.query(ProviderEndpoint)
                .filter(ProviderEndpoint.id == self.target_endpoint_id)
                .first()
            )
            if not endpoint:
                raise HTTPException(status_code=404, detail="Target endpoint not found")
            target_pid = str(endpoint.provider_id)
        elif target_pid:
            target_provider_obj = db.query(Provider).filter(Provider.id == target_pid).first()
            if not target_provider_obj:
                raise HTTPException(status_code=404, detail="Target provider not found")
            # 优先匹配相同 api_format 的端点，其次匹配同 family，最后取任意 active 端点
            active_endpoints = (
                db.query(ProviderEndpoint)
                .filter(
                    ProviderEndpoint.provider_id == target_pid,
                    ProviderEndpoint.is_active == True,  # noqa: E712
                )
                .all()
            )
            endpoint = None
            if active_endpoints and original_api_format:
                # 精确匹配 api_format
                for ep in active_endpoints:
                    ep_fmt = (getattr(ep, "api_format", "") or "").strip().lower()
                    if ep_fmt == original_api_format:
                        endpoint = ep
                        break
                # 同 family 匹配
                if not endpoint and original_api_family:
                    for ep in active_endpoints:
                        ep_family = (getattr(ep, "api_family", "") or "").strip().lower()
                        if ep_family == original_api_family:
                            endpoint = ep
                            break
            if not endpoint and active_endpoints:
                endpoint = active_endpoints[0]
            if not endpoint:
                raise HTTPException(
                    status_code=404, detail="No active endpoint found for target provider"
                )
        else:
            endpoint = None
            if usage_record.provider_endpoint_id:
                endpoint = (
                    db.query(ProviderEndpoint)
                    .filter(ProviderEndpoint.id == usage_record.provider_endpoint_id)
                    .first()
                )
            if not endpoint:
                raise HTTPException(
                    status_code=404,
                    detail="Original endpoint not found, specify target_endpoint_id",
                )

        if not target_provider_obj:
            target_provider_obj = (
                db.query(Provider).filter(Provider.id == endpoint.provider_id).first()
            )
            if not target_provider_obj:
                raise HTTPException(status_code=404, detail="Target provider not found")

        if not target_pid:
            target_pid = str(target_provider_obj.id)

        # 确定 API Key
        target_ep_format = (getattr(endpoint, "api_format", "") or "").strip().lower()
        provider_key = None
        if self.target_api_key_id:
            provider_key = (
                db.query(ProviderAPIKey).filter(ProviderAPIKey.id == self.target_api_key_id).first()
            )
        elif target_pid:
            # 优先选择 api_formats 包含目标端点格式的 Key
            active_keys = (
                db.query(ProviderAPIKey)
                .filter(
                    ProviderAPIKey.provider_id == target_pid,
                    ProviderAPIKey.is_active == True,  # noqa: E712
                )
                .all()
            )
            if active_keys and target_ep_format:
                for k in active_keys:
                    k_formats = getattr(k, "api_formats", None)
                    if k_formats is None:
                        # None 表示支持所有格式，直接选中
                        provider_key = k
                        break
                    if isinstance(k_formats, list) and target_ep_format in [
                        f.strip().lower() for f in k_formats
                    ]:
                        provider_key = k
                        break
            if not provider_key and active_keys:
                provider_key = active_keys[0]
        else:
            if usage_record.provider_api_key_id:
                provider_key = (
                    db.query(ProviderAPIKey)
                    .filter(ProviderAPIKey.id == usage_record.provider_api_key_id)
                    .first()
                )

        if not provider_key:
            raise HTTPException(status_code=404, detail="No API key available for replay")

        # 判定 replay 模式
        original_provider_id = usage_record.provider_id
        if not original_provider_id and usage_record.provider_endpoint_id:
            original_endpoint = (
                db.query(ProviderEndpoint)
                .filter(ProviderEndpoint.id == usage_record.provider_endpoint_id)
                .first()
            )
            if original_endpoint:
                original_provider_id = str(original_endpoint.provider_id)

        same_provider = bool(
            original_provider_id and target_pid and str(original_provider_id) == str(target_pid)
        )
        same_endpoint = bool(
            usage_record.provider_endpoint_id
            and str(usage_record.provider_endpoint_id) == str(endpoint.id)
        )
        replay_mode = _resolve_replay_mode(same_provider, same_endpoint)

        # 解析目标模型
        resolved_model_name, mapping_source = await _resolve_replay_model_name(
            db,
            source_model=source_model,
            target_provider=target_provider_obj,
            target_endpoint=endpoint,
            target_api_key=provider_key,
        )
        mapping_applied = mapping_source != "none"

        mapping_info = {
            "source_model": source_model,
            "original_target_model": original_target_model,
            "resolved_model": resolved_model_name,
            "target_provider_id": str(target_provider_obj.id),
            "target_provider": target_provider_obj.name,
            "target_endpoint_id": str(endpoint.id),
            "target_api_format": target_ep_format,
            "replay_mode": replay_mode,
            "mapping_applied": mapping_applied,
            "mapping_source": mapping_source,
        }

        # 根据 auth_type 正确解析认证（支持 OAuth / Vertex AI / API Key）
        try:
            auth_headers, decrypted_auth_config = await _resolve_provider_auth(
                provider_key, endpoint, db
            )
        except Exception as e:
            logger.error("[replay] Failed to resolve auth for key {}: {}", provider_key.id, e)
            raise HTTPException(status_code=500, detail="Failed to resolve provider authentication")

        # 构建 URL
        url = _build_provider_url_safe(
            endpoint, resolved_model_name, False, provider_key, decrypted_auth_config
        )

        # 构建请求头（Content-Type + 认证头 + 端点额外头）
        headers = _build_fresh_headers(auth_headers, endpoint)

        # 使用覆盖体或原始请求体
        body = body_override_payload or original_request_body or {}

        # 格式转换：如果存储的请求体格式与目标端点格式不同，需要转换
        if isinstance(body, dict):
            target_format = str(getattr(endpoint, "api_format", "") or "").strip().lower()

            if original_api_format and target_format and original_api_format != target_format:
                try:
                    from src.core.api_format.conversion import format_conversion_registry

                    body = format_conversion_registry.convert_request(
                        body,
                        source_format=original_api_format,
                        target_format=target_format,
                    )
                except Exception as conv_err:
                    logger.warning(
                        "[replay] Format conversion {} -> {} failed: {}",
                        original_api_format,
                        target_format,
                        conv_err,
                    )
                    # 转换失败仍发送原始体，让用户看到上游的实际报错

            _apply_replay_model_to_body(body, resolved_model_name, target_format)

        # 强制非流式以获取完整响应
        # Gemini 格式通过 URL 控制流式（streamGenerateContent vs generateContent），
        # 不支持 body 中的 stream 字段，设置会导致 400 错误
        if isinstance(body, dict):
            target_family = str(getattr(endpoint, "api_family", "") or "").lower()
            if target_family == "gemini":
                body.pop("stream", None)
            else:
                body["stream"] = False

        # 反代提供商 envelope 包装：kiro/codex/antigravity 等需要特殊的请求体格式和额外请求头
        if not target_provider_obj:
            target_provider_obj = (
                db.query(Provider).filter(Provider.id == endpoint.provider_id).first()
            )
        if isinstance(body, dict):
            try:
                from src.services.provider.envelope import get_provider_envelope

                target_provider_type = (
                    str(getattr(target_provider_obj, "provider_type", "") or "").lower()
                    if target_provider_obj
                    else None
                )
                target_ep_sig = (getattr(endpoint, "api_format", "") or "").strip().lower()

                envelope = get_provider_envelope(
                    provider_type=target_provider_type,
                    endpoint_sig=target_ep_sig,
                )
                if envelope:
                    body, _ = envelope.wrap_request(
                        body,
                        model=resolved_model_name or "",
                        url_model=resolved_model_name,
                        decrypted_auth_config=decrypted_auth_config,
                    )
                    # envelope 可能注入额外请求头（如 Kiro 的 AWS 签名头、Codex 的 OAuth 头）
                    extra_envelope_headers = envelope.extra_headers()
                    if extra_envelope_headers:
                        headers.update(extra_envelope_headers)
            except Exception as env_err:
                logger.warning("[replay] Envelope wrap failed: {}", env_err)
                # envelope 失败仍发送原始体

        # 获取提供商名称
        provider_name = (
            target_provider_obj.name if target_provider_obj else usage_record.provider_name
        )

        # 发送请求
        try:
            from src.services.proxy_node.resolver import (
                build_proxy_client_kwargs,
                resolve_effective_proxy,
            )

            # 解析代理（key > provider > 系统默认）
            eff_proxy = resolve_effective_proxy(
                getattr(target_provider_obj, "proxy", None) if target_provider_obj else None,
                getattr(provider_key, "proxy", None) if provider_key else None,
            )

            start_time = time.monotonic()
            async with httpx.AsyncClient(
                **build_proxy_client_kwargs(eff_proxy, timeout=60.0)
            ) as client:
                response = await client.post(
                    url,
                    headers=headers,
                    json=body,
                )
            elapsed_ms = int((time.monotonic() - start_time) * 1000)

            # 解析响应体
            try:
                response_body = response.json()
            except Exception:
                response_body = {"raw": response.text[:10000]}

            response_headers = dict(response.headers)

        except httpx.TimeoutException:
            raise HTTPException(status_code=504, detail="Request to provider timed out")
        except Exception as e:
            logger.error("[replay] Failed to connect to provider at {}: {}", url, e)
            raise HTTPException(status_code=502, detail="Failed to connect to provider")

        context.add_audit_metadata(
            action="usage_replay",
            usage_id=self.usage_id,
            target_provider=provider_name,
            target_url=url,
            source_model=source_model,
            original_target_model=original_target_model,
            resolved_model=resolved_model_name,
            target_provider_id=str(target_provider_obj.id),
            target_endpoint_id=str(endpoint.id),
            target_api_format=target_ep_format,
            replay_mode=replay_mode,
            mapping_applied=mapping_applied,
            mapping_source=mapping_source,
        )

        return {
            "url": url,
            "provider": provider_name,
            "status_code": response.status_code,
            "response_headers": response_headers,
            "response_body": response_body,
            "response_time_ms": elapsed_ms,
            "mapping": mapping_info,
        }
