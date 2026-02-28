"""
请求链路追踪 API 端点
"""

from __future__ import annotations

import json
from dataclasses import dataclass
from datetime import datetime
from typing import Any

from fastapi import APIRouter, Depends, HTTPException, Query, Request
from pydantic import BaseModel, ConfigDict
from sqlalchemy.orm import Session

from src.api.base.admin_adapter import AdminApiAdapter
from src.api.base.context import ApiRequestContext
from src.api.base.pipeline import ApiRequestPipeline
from src.core.crypto import crypto_service
from src.database import get_db
from src.models.database import Provider, ProviderAPIKey, ProviderEndpoint
from src.services.request.candidate import RequestCandidateService

router = APIRouter(prefix="/api/admin/monitoring/trace", tags=["Admin - Monitoring: Trace"])
pipeline = ApiRequestPipeline()


class CandidateResponse(BaseModel):
    """候选记录响应"""

    id: str
    request_id: str
    candidate_index: int
    retry_index: int = 0  # 重试序号（从0开始）
    provider_id: str | None = None
    provider_name: str | None = None
    provider_website: str | None = None  # Provider 官网
    endpoint_id: str | None = None
    endpoint_name: str | None = None  # 端点显示名称（api_format）
    key_id: str | None = None
    key_name: str | None = None  # 密钥名称
    key_preview: str | None = None  # 密钥脱敏预览（如 sk-***abc），OAuth 类型不返回
    key_auth_type: str | None = None  # 密钥认证类型（api_key, service_account, oauth）
    key_oauth_plan_type: str | None = None  # OAuth 账号套餐类型（free/plus/team/enterprise）
    key_capabilities: dict | None = None  # Key 支持的能力
    required_capabilities: dict | None = None  # 请求实际需要的能力标签
    status: str  # 'pending', 'success', 'failed', 'skipped'
    skip_reason: str | None = None
    is_cached: bool = False
    # 执行结果字段
    status_code: int | None = None
    error_type: str | None = None
    error_message: str | None = None
    latency_ms: int | None = None
    concurrent_requests: int | None = None
    extra_data: dict | None = None
    created_at: datetime
    started_at: datetime | None = None
    finished_at: datetime | None = None

    model_config = ConfigDict(from_attributes=True)


class RequestTraceResponse(BaseModel):
    """请求追踪完整响应"""

    request_id: str
    total_candidates: int
    final_status: str  # 'success', 'failed', 'cancelled', 'streaming', 'pending'
    total_latency_ms: int
    candidates: list[CandidateResponse]


@router.get("/{request_id}", response_model=RequestTraceResponse)
async def get_request_trace(
    request_id: str,
    request: Request,
    db: Session = Depends(get_db),
) -> Any:
    """
    获取请求的完整追踪信息

    获取指定请求的完整链路追踪信息，包括所有候选（candidates）的执行情况。

    **路径参数**:
    - `request_id`: 请求 ID

    **返回字段**:
    - `request_id`: 请求 ID
    - `total_candidates`: 候选总数
    - `final_status`: 最终状态（success: 成功，failed: 失败，streaming: 流式传输中，pending: 等待中）
    - `total_latency_ms`: 总延迟（毫秒）
    - `candidates`: 候选列表，每个候选包含：
      - `id`: 候选 ID
      - `request_id`: 请求 ID
      - `candidate_index`: 候选索引
      - `retry_index`: 重试序号
      - `provider_id`: 提供商 ID
      - `provider_name`: 提供商名称
      - `provider_website`: 提供商官网
      - `endpoint_id`: 端点 ID
      - `endpoint_name`: 端点名称（API 格式）
      - `key_id`: 密钥 ID
      - `key_name`: 密钥名称
      - `key_preview`: 密钥脱敏预览
      - `key_capabilities`: 密钥支持的能力
      - `required_capabilities`: 请求需要的能力标签
      - `status`: 状态（pending, success, failed, skipped）
      - `skip_reason`: 跳过原因
      - `is_cached`: 是否缓存命中
      - `status_code`: HTTP 状态码
      - `error_type`: 错误类型
      - `error_message`: 错误信息
      - `latency_ms`: 延迟（毫秒）
      - `concurrent_requests`: 并发请求数
      - `extra_data`: 额外数据
      - `created_at`: 创建时间
      - `started_at`: 开始时间
      - `finished_at`: 完成时间
    """

    adapter = AdminGetRequestTraceAdapter(request_id=request_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/stats/provider/{provider_id}")
async def get_provider_failure_rate(
    provider_id: str,
    request: Request,
    limit: int = Query(100, ge=1, le=1000, description="统计最近的尝试数量"),
    db: Session = Depends(get_db),
) -> Any:
    """
    获取提供商的失败率统计

    获取指定提供商最近的失败率统计信息。需要管理员权限。

    **路径参数**:
    - `provider_id`: 提供商 ID

    **查询参数**:
    - `limit`: 统计最近的尝试数量，默认 100，最大 1000

    **返回字段**:
    - `provider_id`: 提供商 ID
    - `total_attempts`: 总尝试次数
    - `success_count`: 成功次数
    - `failed_count`: 失败次数
    - `failure_rate`: 失败率（百分比）
    - `avg_latency_ms`: 平均延迟（毫秒）
    """
    adapter = AdminProviderFailureRateAdapter(provider_id=provider_id, limit=limit)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


# -------- 请求追踪适配器 --------


@dataclass
class AdminGetRequestTraceAdapter(AdminApiAdapter):
    request_id: str

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        db = context.db

        # 只查询 candidates
        candidates = RequestCandidateService.get_candidates_by_request_id(db, self.request_id)

        # 如果没有数据，返回 404
        if not candidates:
            raise HTTPException(status_code=404, detail="Request not found")

        # 计算总延迟（只统计已完成的候选：success, failed, cancelled）
        # 使用显式的 is not None 检查，避免过滤掉 0ms 的快速响应
        total_latency = sum(
            c.latency_ms
            for c in candidates
            if c.status in ("success", "failed", "cancelled") and c.latency_ms is not None
        )

        # 判断最终状态：
        # 1. status="success" 即视为成功（无论 status_code 是什么）
        #    - 流式请求即使客户端断开（499），只要 Provider 成功返回数据，也算成功
        # 2. 同时检查 status_code 在 200-299 范围，作为额外的成功判断条件
        #    - 用于兼容非流式请求或未正确设置 status 的旧数据
        # 3. status="streaming" 表示流式请求正在进行中
        # 4. status="pending" 表示请求尚未开始执行
        # 5. status="cancelled" 表示客户端主动断开连接（不算失败）
        has_success = any(
            c.status == "success" or (c.status_code is not None and 200 <= c.status_code < 300)
            for c in candidates
        )
        has_streaming = any(c.status == "streaming" for c in candidates)
        has_pending = any(c.status == "pending" for c in candidates)
        has_cancelled = any(c.status == "cancelled" for c in candidates)
        has_failed = any(c.status == "failed" for c in candidates)

        if has_success:
            final_status = "success"
        elif has_streaming:
            # 有候选正在流式传输中
            final_status = "streaming"
        elif has_pending:
            # 有候选正在等待执行
            final_status = "pending"
        elif has_cancelled and not has_failed:
            # 只有取消没有失败，算作取消
            final_status = "cancelled"
        else:
            final_status = "failed"

        # 批量加载 provider 信息，避免 N+1 查询
        provider_ids = {c.provider_id for c in candidates if c.provider_id}
        provider_map: dict[str, str] = {}
        provider_website_map: dict[str, str | None] = {}
        provider_type_map: dict[str, str] = {}
        if provider_ids:
            providers = db.query(Provider).filter(Provider.id.in_(provider_ids)).all()
            for p in providers:
                provider_map[p.id] = p.name
                provider_website_map[p.id] = p.website
                provider_type_map[p.id] = getattr(p, "provider_type", "custom") or "custom"

        # 批量加载 endpoint 信息
        endpoint_ids = {c.endpoint_id for c in candidates if c.endpoint_id}
        endpoint_map = {}
        if endpoint_ids:
            endpoints = (
                db.query(ProviderEndpoint).filter(ProviderEndpoint.id.in_(endpoint_ids)).all()
            )
            endpoint_map = {e.id: e.api_format for e in endpoints}

        # 批量加载 key 信息
        key_ids = {c.key_id for c in candidates if c.key_id}
        key_map: dict[str, str] = {}
        key_preview_map: dict[str, str] = {}
        key_capabilities_map: dict[str, dict | None] = {}
        key_auth_type_map: dict[str, str] = {}
        key_oauth_plan_map: dict[str, str | None] = {}
        # 建立 key_id -> provider_id 的映射（用于获取 provider_type）
        key_provider_map: dict[str, str | None] = {
            c.key_id: c.provider_id for c in candidates if c.key_id
        }
        if key_ids:
            keys = db.query(ProviderAPIKey).filter(ProviderAPIKey.id.in_(key_ids)).all()
            for k in keys:
                key_map[k.id] = k.name
                key_capabilities_map[k.id] = k.capabilities

                is_oauth = k.auth_type == "oauth"

                if is_oauth:
                    # OAuth: auth_type 使用具体的 provider_type（如 kiro/codex/antigravity）
                    pid = key_provider_map.get(k.id)
                    key_auth_type_map[k.id] = (
                        provider_type_map.get(pid, "oauth") if pid else "oauth"
                    )
                    # 提取 plan_type（不同 provider 存储位置不同）
                    oauth_plan_type = None
                    # 1. Codex: auth_config.plan_type
                    # 2. Antigravity: auth_config.tier
                    if k.auth_config:
                        try:
                            decrypted_config = crypto_service.decrypt(k.auth_config)
                            auth_config = json.loads(decrypted_config)
                            oauth_plan_type = auth_config.get("plan_type")
                            if not oauth_plan_type:
                                ag_tier = auth_config.get("tier")
                                if ag_tier and isinstance(ag_tier, str):
                                    oauth_plan_type = ag_tier.lower()
                        except Exception:
                            pass
                    # 3. Kiro: upstream_metadata.kiro.subscription_title
                    #    subscription_title 通常为 "KIRO FREE" / "KIRO PRO+" 等，
                    #    去掉 provider 名称前缀，只保留等级部分
                    if not oauth_plan_type:
                        um = getattr(k, "upstream_metadata", None) or {}
                        kiro_meta = um.get("kiro") if isinstance(um, dict) else None
                        if isinstance(kiro_meta, dict):
                            sub_title = kiro_meta.get("subscription_title")
                            if sub_title and isinstance(sub_title, str):
                                # "KIRO FREE" -> "Free", "KIRO PRO+" -> "Pro+"
                                ptype = provider_type_map.get(pid, "") if pid else ""
                                if ptype and sub_title.upper().startswith(ptype.upper()):
                                    sub_title = sub_title[len(ptype) :].strip()
                                oauth_plan_type = sub_title
                    key_oauth_plan_map[k.id] = oauth_plan_type
                    continue
                else:
                    key_auth_type_map[k.id] = k.auth_type or "api_key"

                # 非 OAuth：生成脱敏预览
                try:
                    decrypted_key = crypto_service.decrypt(k.api_key)
                    if len(decrypted_key) > 8:
                        # 检测常见前缀模式
                        prefix_end = 0
                        for prefix in ["sk-", "key-", "api-", "ak-"]:
                            if decrypted_key.lower().startswith(prefix):
                                prefix_end = len(prefix)
                                break
                        if prefix_end > 0:
                            key_preview_map[k.id] = (
                                f"{decrypted_key[:prefix_end]}***{decrypted_key[-4:]}"
                            )
                        else:
                            key_preview_map[k.id] = f"{decrypted_key[:4]}***{decrypted_key[-4:]}"
                    elif len(decrypted_key) > 4:
                        key_preview_map[k.id] = f"***{decrypted_key[-4:]}"
                    else:
                        key_preview_map[k.id] = "***"
                except Exception:
                    key_preview_map[k.id] = "***"

        # 构建 candidate 响应列表
        candidate_responses: list[CandidateResponse] = []
        for candidate in candidates:
            provider_name = (
                provider_map.get(candidate.provider_id) if candidate.provider_id else None
            )
            provider_website = (
                provider_website_map.get(candidate.provider_id) if candidate.provider_id else None
            )
            endpoint_name = (
                endpoint_map.get(candidate.endpoint_id) if candidate.endpoint_id else None
            )
            key_name = key_map.get(candidate.key_id) if candidate.key_id else None
            key_preview = key_preview_map.get(candidate.key_id) if candidate.key_id else None
            key_auth_type = key_auth_type_map.get(candidate.key_id) if candidate.key_id else None
            key_oauth_plan_type = (
                key_oauth_plan_map.get(candidate.key_id) if candidate.key_id else None
            )
            key_capabilities = (
                key_capabilities_map.get(candidate.key_id) if candidate.key_id else None
            )

            candidate_responses.append(
                CandidateResponse(
                    id=candidate.id,
                    request_id=candidate.request_id,
                    candidate_index=candidate.candidate_index,
                    retry_index=candidate.retry_index,
                    provider_id=candidate.provider_id,
                    provider_name=provider_name,
                    provider_website=provider_website,
                    endpoint_id=candidate.endpoint_id,
                    endpoint_name=endpoint_name,
                    key_id=candidate.key_id,
                    key_name=key_name,
                    key_preview=key_preview,
                    key_auth_type=key_auth_type,
                    key_oauth_plan_type=key_oauth_plan_type,
                    key_capabilities=key_capabilities,
                    required_capabilities=candidate.required_capabilities,
                    status=candidate.status,
                    skip_reason=candidate.skip_reason,
                    is_cached=candidate.is_cached,
                    status_code=candidate.status_code,
                    error_type=candidate.error_type,
                    error_message=candidate.error_message,
                    latency_ms=candidate.latency_ms,
                    concurrent_requests=candidate.concurrent_requests,
                    extra_data=candidate.extra_data,
                    created_at=candidate.created_at,
                    started_at=candidate.started_at,
                    finished_at=candidate.finished_at,
                )
            )

        response = RequestTraceResponse(
            request_id=self.request_id,
            total_candidates=len(candidates),
            final_status=final_status,
            total_latency_ms=total_latency,
            candidates=candidate_responses,
        )
        context.add_audit_metadata(
            action="trace_request_detail",
            request_id=self.request_id,
            total_candidates=len(candidates),
            final_status=final_status,
            total_latency_ms=total_latency,
        )
        return response


@dataclass
class AdminProviderFailureRateAdapter(AdminApiAdapter):
    provider_id: str
    limit: int

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        result = RequestCandidateService.get_candidate_stats_by_provider(
            db=context.db,
            provider_id=self.provider_id,
            limit=self.limit,
        )
        context.add_audit_metadata(
            action="trace_provider_failure_rate",
            provider_id=self.provider_id,
            limit=self.limit,
            total_attempts=result.get("total_attempts"),
            failure_rate=result.get("failure_rate"),
        )
        return result
