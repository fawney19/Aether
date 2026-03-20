"""
Endpoint 健康监控 API
"""

from __future__ import annotations

import asyncio
from collections import defaultdict
from dataclasses import dataclass
from datetime import datetime, timedelta, timezone
from typing import Any

from fastapi import APIRouter, Depends, Query, Request
from sqlalchemy import func
from sqlalchemy.orm import Session

from src.api.base.admin_adapter import AdminApiAdapter
from src.api.base.context import ApiRequestContext
from src.api.base.pipeline import get_pipeline
from src.core.exceptions import NotFoundException
from src.core.logger import logger
from src.database import get_db
from src.models.database import Provider, ProviderAPIKey, ProviderEndpoint, RequestCandidate
from src.models.endpoint_models import (
    ApiFormatHealthMonitor,
    ApiFormatHealthMonitorResponse,
    EndpointHealthEvent,
    HealthStatusResponse,
    HealthSummaryResponse,
)
from src.services.health.endpoint import EndpointHealthService
from src.services.health.monitor import HealthMonitor, get_health_monitor

router = APIRouter(tags=["Endpoint Health"])


def _recover_key_health_sync(db: Session, key_id: str, api_format: str | None) -> dict[str, Any]:
    key = db.query(ProviderAPIKey).filter(ProviderAPIKey.id == key_id).first()
    if not key:
        raise NotFoundException(f"Key {key_id} 不存在")

    success = get_health_monitor().reset_health(db, key_id=key_id, api_format=api_format)
    if not success:
        raise Exception("重置健康度失败")

    if not key.is_active:
        key.is_active = True  # type: ignore[assignment]

    db.commit()
    return {
        "is_active": bool(key.is_active),
        "api_format": api_format,
    }


def _recover_all_keys_health_sync(db: Session) -> list[dict[str, Any]]:
    candidates = (
        db.query(ProviderAPIKey)
        .filter(
            ProviderAPIKey.circuit_breaker_by_format.isnot(None),
            ProviderAPIKey.circuit_breaker_by_format != "{}",
        )
        .all()
    )

    circuit_open_keys = [
        key
        for key in candidates
        if any(cb.get("open") for cb in (key.circuit_breaker_by_format or {}).values())
    ]

    recovered_keys: list[dict[str, Any]] = []
    for key in circuit_open_keys:
        key.health_by_format = {}  # type: ignore[assignment]
        key.circuit_breaker_by_format = {}  # type: ignore[assignment]
        recovered_keys.append(
            {
                "key_id": key.id,
                "key_name": key.name,
                "provider_id": key.provider_id,
                "api_formats": key.api_formats,
            }
        )

    if recovered_keys:
        db.commit()

    return recovered_keys


def _format_str(api_format_enum: Any) -> str:
    """将 DB 查询返回的 api_format（可能是 enum 或 str）统一转为 str。"""
    return api_format_enum.value if hasattr(api_format_enum, "value") else str(api_format_enum)


def _fetch_recent_attempts_for_api_format(
    db: Session,
    *,
    api_format: str,
    since: datetime,
    per_format_limit: int,
) -> list[RequestCandidate]:
    """获取单个 API 格式最近的最终态请求，用于事件展示。"""
    final_statuses = ["success", "failed", "skipped"]
    return (
        db.query(RequestCandidate)
        .join(ProviderEndpoint, RequestCandidate.endpoint_id == ProviderEndpoint.id)
        .join(Provider, ProviderEndpoint.provider_id == Provider.id)
        .filter(
            ProviderEndpoint.is_active.is_(True),
            Provider.is_active.is_(True),
            ProviderEndpoint.api_format == api_format,
            RequestCandidate.created_at >= since,
            RequestCandidate.status.in_(final_statuses),
        )
        .order_by(RequestCandidate.created_at.desc())
        .limit(per_format_limit)
        .all()
    )


pipeline = get_pipeline()


@router.get("/health/summary", response_model=HealthSummaryResponse)
async def get_health_summary(
    request: Request,
    db: Session = Depends(get_db),
) -> HealthSummaryResponse:
    """
    获取健康状态摘要

    获取系统整体健康状态摘要，包括所有 Provider、Endpoint 和 Key 的健康状态统计。

    **返回字段**:
    - `total_providers`: Provider 总数
    - `active_providers`: 活跃 Provider 数量
    - `total_endpoints`: Endpoint 总数
    - `active_endpoints`: 活跃 Endpoint 数量
    - `total_keys`: Key 总数
    - `active_keys`: 当前可调度 Key 数量（Provider 活跃 + Key 启用 + 存在活跃 Endpoint + 非熔断）
    - `circuit_breaker_open_keys`: 熔断的 Key 数量
    """
    adapter = AdminHealthSummaryAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/health/status")
async def get_endpoint_health_status(
    request: Request,
    lookback_hours: int = Query(6, ge=1, le=72, description="回溯的小时数"),
    db: Session = Depends(get_db),
) -> Any:
    """
    获取端点健康状态（简化视图，与用户端点统一）

    获取按 API 格式聚合的端点健康状态时间线，基于 Usage 表统计，
    返回 50 个时间段的聚合状态，适用于快速查看整体健康趋势。

    与 /health/api-formats 的区别：
    - /health/status: 返回聚合的时间线状态（50个时间段），基于 Usage 表
    - /health/api-formats: 返回详细的事件列表，基于 RequestCandidate 表

    **查询参数**:
    - `lookback_hours`: 回溯的小时数（1-72），默认 6

    **返回字段**:
    - `api_format`: API 格式名称
    - `timeline`: 时间线数据（50个时间段）
    - `time_range_start`: 时间范围起始
    - `time_range_end`: 时间范围结束
    """
    adapter = AdminEndpointHealthStatusAdapter(lookback_hours=lookback_hours)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/health/api-formats", response_model=ApiFormatHealthMonitorResponse)
async def get_api_format_health_monitor(
    request: Request,
    lookback_hours: int = Query(6, ge=1, le=72, description="回溯的小时数"),
    per_format_limit: int = Query(60, ge=10, le=200, description="每个 API 格式的事件数量"),
    db: Session = Depends(get_db),
) -> ApiFormatHealthMonitorResponse:
    """
    获取按 API 格式聚合的健康监控时间线（详细事件列表）

    获取每个 API 格式的详细健康监控数据，包括请求事件列表、成功率统计、
    时间线数据等，基于 RequestCandidate 表查询，适用于详细分析。

    **查询参数**:
    - `lookback_hours`: 回溯的小时数（1-72），默认 6
    - `per_format_limit`: 每个 API 格式返回的事件数量（10-200），默认 60

    **返回字段**:
    - `generated_at`: 数据生成时间
    - `formats`: API 格式健康监控数据列表
      - `api_format`: API 格式名称
      - `total_attempts`: 总请求数
      - `success_count`: 成功请求数
      - `failed_count`: 失败请求数
      - `skipped_count`: 跳过请求数
      - `success_rate`: 成功率
      - `provider_count`: Provider 数量
      - `key_count`: Key 数量
      - `last_event_at`: 最后事件时间
      - `events`: 事件列表
      - `timeline`: 时间线数据
      - `time_range_start`: 时间范围起始
      - `time_range_end`: 时间范围结束
    """
    adapter = AdminApiFormatHealthMonitorAdapter(
        lookback_hours=lookback_hours,
        per_format_limit=per_format_limit,
    )
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/health/key/{key_id}", response_model=HealthStatusResponse)
async def get_key_health(
    key_id: str,
    request: Request,
    api_format: str | None = Query(None, description="API 格式（可选，如 CLAUDE、OPENAI）"),
    db: Session = Depends(get_db),
) -> HealthStatusResponse:
    """
    获取 Key 健康状态

    获取指定 API Key 的健康状态详情，包括健康分数、连续失败次数、
    熔断器状态等信息。支持按 API 格式查询。

    **路径参数**:
    - `key_id`: API Key ID

    **查询参数**:
    - `api_format`: 可选，指定 API 格式（如 CLAUDE、OPENAI）。
      - 指定时返回该格式的健康度详情
      - 不指定时返回所有格式的健康度摘要

    **返回字段**:
    - `key_id`: API Key ID
    - `key_health_score`: 健康分数（0.0-1.0）
    - `key_is_active`: 是否活跃
    - `key_statistics`: 统计信息
    - `health_by_format`: 按格式的健康度数据（无 api_format 参数时）
    - `circuit_breaker_open`: 熔断器是否打开（有 api_format 参数时）
    """
    adapter = AdminKeyHealthAdapter(key_id=key_id, api_format=api_format)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.patch("/health/keys/{key_id}")
async def recover_key_health(
    key_id: str,
    request: Request,
    api_format: str | None = Query(None, description="API 格式（可选，不指定则恢复所有格式）"),
    db: Session = Depends(get_db),
) -> dict:
    """
    恢复 Key 健康状态

    手动恢复指定 Key 的健康状态，将健康分数重置为 1.0，关闭熔断器，
    取消自动禁用，并重置所有失败计数。支持按 API 格式恢复。

    **路径参数**:
    - `key_id`: API Key ID

    **查询参数**:
    - `api_format`: 可选，指定 API 格式（如 CLAUDE、OPENAI）
      - 指定时仅恢复该格式的健康度
      - 不指定时恢复所有格式

    **返回字段**:
    - `message`: 操作结果消息
    - `details`: 详细信息
      - `health_score`: 健康分数
      - `circuit_breaker_open`: 熔断器状态
      - `is_active`: 是否活跃
    """
    adapter = AdminRecoverKeyHealthAdapter(key_id=key_id, api_format=api_format)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.patch("/health/keys")
async def recover_all_keys_health(
    request: Request,
    db: Session = Depends(get_db),
) -> dict:
    """
    批量恢复所有熔断 Key 的健康状态

    查找所有处于熔断状态的 Key（circuit_breaker_open=True），
    并批量执行以下操作：
    1. 将健康分数重置为 1.0
    2. 关闭熔断器
    3. 重置失败计数

    **返回字段**:
    - `message`: 操作结果消息
    - `recovered_count`: 恢复的 Key 数量
    - `recovered_keys`: 恢复的 Key 列表
      - `key_id`: Key ID
      - `key_name`: Key 名称
      - `endpoint_id`: Endpoint ID
    """
    adapter = AdminRecoverAllKeysHealthAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


# -------- Adapters --------


class AdminHealthSummaryAdapter(AdminApiAdapter):
    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        summary = get_health_monitor().get_all_health_status(context.db)
        return HealthSummaryResponse(**summary)


@dataclass
class AdminEndpointHealthStatusAdapter(AdminApiAdapter):
    """管理员端点健康状态适配器（与用户端点统一，但包含管理员字段）"""

    lookback_hours: int

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        db = context.db

        # 使用共享服务获取健康状态（管理员视图）
        result = EndpointHealthService.get_endpoint_health_by_format(
            db=db,
            lookback_hours=self.lookback_hours,
            include_admin_fields=True,  # 包含管理员字段
            use_cache=False,  # 管理员不使用缓存，确保实时性
        )

        context.add_audit_metadata(
            action="endpoint_health_status",
            format_count=len(result),
            lookback_hours=self.lookback_hours,
        )

        return result


@dataclass
class AdminApiFormatHealthMonitorAdapter(AdminApiAdapter):
    lookback_hours: int
    per_format_limit: int

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        db = context.db
        now = datetime.now(timezone.utc)
        since = now - timedelta(hours=self.lookback_hours)

        # 1. 单次查询获取所有活跃 endpoint 行，在内存中聚合 provider_count / endpoint_map
        endpoint_rows = (
            db.query(ProviderEndpoint.api_format, ProviderEndpoint.id, ProviderEndpoint.provider_id)
            .join(Provider, ProviderEndpoint.provider_id == Provider.id)
            .filter(
                ProviderEndpoint.is_active.is_(True),
                Provider.is_active.is_(True),
            )
            .all()
        )

        all_formats: dict[str, int] = {}  # api_format -> distinct provider count
        endpoint_map: dict[str, list[str]] = defaultdict(list)
        active_provider_formats: set[tuple[str, str]] = set()
        _provider_sets: dict[str, set[str]] = defaultdict(set)

        for api_format_enum, endpoint_id, provider_id in endpoint_rows:
            fmt = _format_str(api_format_enum)
            endpoint_map[fmt].append(endpoint_id)
            _provider_sets[fmt].add(str(provider_id))
            active_provider_formats.add((str(provider_id), fmt))

        for fmt, pids in _provider_sets.items():
            all_formats[fmt] = len(pids)

        # 1.2 统计每个 API 格式可用的活跃 Key 数量（Key 属于 Provider，通过 api_formats 关联格式）
        key_counts: dict[str, int] = {}
        if active_provider_formats:
            active_provider_keys = (
                db.query(ProviderAPIKey.provider_id, ProviderAPIKey.api_formats)
                .join(Provider, ProviderAPIKey.provider_id == Provider.id)
                .filter(
                    Provider.is_active.is_(True),
                    ProviderAPIKey.is_active.is_(True),
                )
                .all()
            )
            for provider_id, api_formats in active_provider_keys:
                pid = str(provider_id)
                for fmt in api_formats or []:
                    if (pid, fmt) not in active_provider_formats:
                        continue
                    key_counts[fmt] = key_counts.get(fmt, 0) + 1

        # 2. 统计窗口内每个 API 格式的请求状态分布（真实统计）
        # 只统计最终状态：success, failed, skipped
        final_statuses = ["success", "failed", "skipped"]
        status_counts_query = (
            db.query(
                ProviderEndpoint.api_format,
                RequestCandidate.status,
                func.count(RequestCandidate.id).label("count"),
            )
            .join(RequestCandidate, ProviderEndpoint.id == RequestCandidate.endpoint_id)
            .join(Provider, ProviderEndpoint.provider_id == Provider.id)
            .filter(
                ProviderEndpoint.is_active.is_(True),
                Provider.is_active.is_(True),
                RequestCandidate.created_at >= since,
                RequestCandidate.status.in_(final_statuses),
            )
            .group_by(ProviderEndpoint.api_format, RequestCandidate.status)
            .all()
        )

        # 构建每个格式的状态统计
        status_counts: dict[str, dict[str, int]] = {}
        for api_format_enum, status, count in status_counts_query:
            fmt = _format_str(api_format_enum)
            if fmt not in status_counts:
                status_counts[fmt] = {"success": 0, "failed": 0, "skipped": 0}
            status_counts[fmt][status] = count

        # 3. 为所有活跃格式生成监控数据（包括没有请求记录的）
        monitors: list[ApiFormatHealthMonitor] = []
        for api_format in all_formats:
            attempts = _fetch_recent_attempts_for_api_format(
                db=db,
                api_format=api_format,
                since=since,
                per_format_limit=self.per_format_limit,
            )
            # 获取窗口内的真实统计数据
            # 只统计最终状态：success, failed, skipped
            # 中间状态（available, pending, used, started）不计入统计
            format_stats = status_counts.get(api_format, {"success": 0, "failed": 0, "skipped": 0})
            real_success_count = format_stats.get("success", 0)
            real_failed_count = format_stats.get("failed", 0)
            real_skipped_count = format_stats.get("skipped", 0)
            # total_attempts 只包含最终状态的请求数
            total_attempts = real_success_count + real_failed_count + real_skipped_count

            # 时间线按时间正序
            attempts_sorted = list(reversed(attempts))
            events: list[EndpointHealthEvent] = []
            for attempt in attempts_sorted:
                event_timestamp = attempt.finished_at or attempt.started_at or attempt.created_at
                events.append(
                    EndpointHealthEvent(
                        timestamp=event_timestamp,
                        status=attempt.status,
                        status_code=attempt.status_code,
                        latency_ms=attempt.latency_ms,
                        error_type=attempt.error_type,
                        error_message=attempt.error_message,
                    )
                )

            # 成功率 = success / (success + failed)
            # skipped 不算失败，不计入成功率分母
            # 无实际完成请求时成功率为 1.0（灰色状态）
            actual_completed = real_success_count + real_failed_count
            success_rate = real_success_count / actual_completed if actual_completed > 0 else 1.0
            last_event_at = events[-1].timestamp if events else None

            # 生成 Usage 基于时间窗口的健康时间线
            timeline_data = EndpointHealthService._generate_timeline_from_usage(
                db=db,
                endpoint_ids=endpoint_map.get(api_format, []),
                now=now,
                lookback_hours=self.lookback_hours,
            )

            monitors.append(
                ApiFormatHealthMonitor(
                    api_format=api_format,
                    total_attempts=total_attempts,  # 真实总请求数
                    success_count=real_success_count,  # 真实成功数
                    failed_count=real_failed_count,  # 真实失败数
                    skipped_count=real_skipped_count,  # 真实跳过数
                    success_rate=success_rate,  # 基于真实统计的成功率
                    provider_count=all_formats[api_format],
                    key_count=key_counts.get(api_format, 0),
                    last_event_at=last_event_at,
                    events=events,  # 限制为 per_format_limit 条（用于时间线显示）
                    timeline=timeline_data.get("timeline", []),
                    time_range_start=timeline_data.get("time_range_start"),
                    time_range_end=timeline_data.get("time_range_end"),
                )
            )

        response = ApiFormatHealthMonitorResponse(
            generated_at=now,
            formats=monitors,
        )
        context.add_audit_metadata(
            action="api_format_health_monitor",
            format_count=len(monitors),
            lookback_hours=self.lookback_hours,
            per_format_limit=self.per_format_limit,
        )
        return response


@dataclass
class AdminKeyHealthAdapter(AdminApiAdapter):
    key_id: str
    api_format: str | None = None

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        health_data = get_health_monitor().get_key_health(context.db, self.key_id, self.api_format)
        if not health_data:
            raise NotFoundException(f"Key {self.key_id} 不存在")

        # 构建响应
        response_data = {
            "key_id": health_data["key_id"],
            "key_is_active": health_data["is_active"],
            "key_statistics": health_data.get("statistics"),
            "key_health_score": health_data.get("health_score", 1.0),
        }

        if self.api_format:
            # 单格式查询
            response_data["api_format"] = self.api_format
            response_data["key_consecutive_failures"] = health_data.get("consecutive_failures")
            response_data["key_last_failure_at"] = health_data.get("last_failure_at")
            circuit = health_data.get("circuit_breaker", {})
            response_data["circuit_breaker_open"] = circuit.get("open", False)
            response_data["circuit_breaker_open_at"] = circuit.get("open_at")
            response_data["next_probe_at"] = circuit.get("next_probe_at")
            response_data["half_open_until"] = circuit.get("half_open_until")
            response_data["half_open_successes"] = circuit.get("half_open_successes", 0)
            response_data["half_open_failures"] = circuit.get("half_open_failures", 0)
        else:
            # 全格式查询
            response_data["any_circuit_open"] = health_data.get("any_circuit_open", False)
            response_data["health_by_format"] = health_data.get("health_by_format")

        return HealthStatusResponse(**response_data)


@dataclass
class AdminRecoverKeyHealthAdapter(AdminApiAdapter):
    key_id: str
    api_format: str | None = None

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        db = context.db
        await asyncio.to_thread(_recover_key_health_sync, db, self.key_id, self.api_format)

        if self.api_format:
            logger.info(f"管理员恢复Key健康状态: {self.key_id}/{self.api_format}")
            return {
                "message": f"Key 的 {self.api_format} 格式已恢复",
                "details": {
                    "api_format": self.api_format,
                    "health_score": 1.0,
                    "circuit_breaker_open": False,
                    "is_active": True,
                },
            }
        else:
            logger.info(f"管理员恢复Key健康状态: {self.key_id} (所有格式)")
            return {
                "message": "Key 所有格式已恢复",
                "details": {
                    "health_score": 1.0,
                    "circuit_breaker_open": False,
                    "is_active": True,
                },
            }


class AdminRecoverAllKeysHealthAdapter(AdminApiAdapter):
    """批量恢复所有熔断 Key 的健康状态"""

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        db = context.db
        recovered_keys = await asyncio.to_thread(_recover_all_keys_health_sync, db)

        if not recovered_keys:
            return {
                "message": "没有需要恢复的 Key",
                "recovered_count": 0,
                "recovered_keys": [],
            }

        # 重置健康监控器的熔断计数
        HealthMonitor.reset_open_circuit_count()

        logger.info(f"管理员批量恢复 {len(recovered_keys)} 个 Key 的健康状态")

        return {
            "message": f"已恢复 {len(recovered_keys)} 个 Key",
            "recovered_count": len(recovered_keys),
            "recovered_keys": recovered_keys,
        }
