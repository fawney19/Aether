"""
端点健康状态服务

提供统一的端点健康监控功能，支持：
1. 按 API 格式聚合的健康状态
2. 基于时间窗口的状态追踪
3. 管理员和普通用户的差异化视图
4. Redis 缓存优化
"""

from __future__ import annotations

import json
from collections import defaultdict
from datetime import datetime, timedelta, timezone
from typing import Any

from sqlalchemy import case, func
from sqlalchemy.orm import Session, load_only

from src.core.logger import logger
from src.models.database import Provider, ProviderAPIKey, ProviderEndpoint, RequestCandidate

# 缓存配置
CACHE_TTL_SECONDS = 30  # 缓存 30 秒
CACHE_KEY_PREFIX = "health:endpoint:"


def _get_redis_client() -> Any:
    """获取 Redis 客户端，失败返回 None"""
    try:
        from src.clients.redis_client import redis_client

        return redis_client
    except Exception:
        return None


class EndpointHealthService:
    """端点健康状态服务"""

    @staticmethod
    def get_endpoint_health_by_format(
        db: Session,
        lookback_hours: int = 6,
        include_admin_fields: bool = False,
        use_cache: bool = True,
    ) -> list[dict[str, Any]]:
        """
        获取按 API 格式聚合的端点健康状态

        Args:
            db: 数据库会话
            lookback_hours: 回溯小时数
            include_admin_fields: 是否包含管理员字段（provider_count, key_count等）
            use_cache: 是否使用缓存（仅对普通用户视图有效）

        Returns:
            按 API 格式聚合的健康状态列表
        """
        # 尝试从缓存获取
        cache_key = f"{CACHE_KEY_PREFIX}format:{lookback_hours}:{include_admin_fields}"
        if use_cache:
            cached = EndpointHealthService._get_from_cache(cache_key)
            if cached is not None:
                return cached

        now = datetime.now(timezone.utc)

        # 查询所有活跃的端点（一次性获取所有需要的数据）
        endpoints = (
            db.query(ProviderEndpoint)
            .join(Provider)
            .filter(
                ProviderEndpoint.is_active.is_(True),
                Provider.is_active.is_(True),
            )
            .all()
        )

        # 收集所有 provider_ids
        all_provider_ids = list({ep.provider_id for ep in endpoints})

        # 批量查询所有密钥（通过 provider_id 关联，只加载分组和统计所需列）
        all_keys = (
            (
                db.query(ProviderAPIKey)
                .filter(ProviderAPIKey.provider_id.in_(all_provider_ids))
                .options(
                    load_only(
                        ProviderAPIKey.id,
                        ProviderAPIKey.provider_id,
                        ProviderAPIKey.is_active,
                        ProviderAPIKey.api_formats,
                        ProviderAPIKey.health_by_format,
                        ProviderAPIKey.circuit_breaker_by_format,
                    )
                )
                .all()
            )
            if all_provider_ids
            else []
        )

        # 按 api_format 分组密钥（通过 api_formats 字段）
        keys_by_format: dict[str, list[ProviderAPIKey]] = defaultdict(list)
        for key in all_keys:
            for fmt in key.api_formats or []:
                keys_by_format[fmt].append(key)

        # 按 API 格式聚合
        format_stats = defaultdict(
            lambda: {
                "total_endpoints": 0,
                "total_keys": 0,
                "active_keys": 0,
                "health_scores": [],
                "endpoint_ids": [],
                "provider_ids": set(),
                "key_ids": [],
            }
        )

        for ep in endpoints:
            api_format = ep.api_format if ep.api_format else "UNKNOWN"

            # 统计端点数
            format_stats[api_format]["total_endpoints"] += 1
            format_stats[api_format]["endpoint_ids"].append(ep.id)
            format_stats[api_format]["provider_ids"].add(ep.provider_id)

        # 统计每个格式的密钥（直接从 keys_by_format 获取）
        for api_format, keys in keys_by_format.items():
            if api_format not in format_stats:
                # 如果有 Key 但没有对应的 Endpoint，跳过
                continue

            # 去重（同一个 Key 可能支持多个格式）
            seen_key_ids = set()
            unique_keys = []
            for key in keys:
                if key.id not in seen_key_ids:
                    seen_key_ids.add(key.id)
                    unique_keys.append(key)

            format_stats[api_format]["total_keys"] = len(unique_keys)

            for key in unique_keys:
                format_stats[api_format]["key_ids"].append(key.id)
                # 检查该格式的熔断器状态
                circuit_by_format = key.circuit_breaker_by_format or {}
                format_circuit = circuit_by_format.get(api_format, {})
                is_circuit_open = format_circuit.get("open", False)

                if key.is_active and not is_circuit_open:
                    format_stats[api_format]["active_keys"] += 1
                    # 获取该格式的健康度
                    health_by_format = key.health_by_format or {}
                    format_health = health_by_format.get(api_format, {})
                    health_score = float(format_health.get("health_score") or 1.0)
                    format_stats[api_format]["health_scores"].append(health_score)

        # 批量生成所有格式的时间线数据
        format_endpoint_mapping: dict[str, list[str]] = {}
        for api_format, stats in format_stats.items():
            format_endpoint_mapping[api_format] = stats["endpoint_ids"]

        # 一次性查询所有时间线数据
        timeline_data_map = EndpointHealthService._generate_timeline_batch(
            db, format_endpoint_mapping, now, lookback_hours
        )

        # 生成结果
        result = []

        for api_format, stats in format_stats.items():
            timeline_data = timeline_data_map.get(
                api_format,
                {
                    "timeline": ["unknown"] * 100,
                    "time_range_start": None,
                    "time_range_end": None,
                },
            )
            timeline = timeline_data["timeline"]
            time_range_start = timeline_data.get("time_range_start")
            time_range_end = timeline_data.get("time_range_end")

            # 基于时间线计算实际健康度
            if timeline:
                healthy_count = sum(1 for status in timeline if status == "healthy")
                warning_count = sum(1 for status in timeline if status == "warning")
                unhealthy_count = sum(1 for status in timeline if status == "unhealthy")
                known_count = healthy_count + warning_count + unhealthy_count

                if known_count > 0:
                    avg_health = (healthy_count * 1.0 + warning_count * 0.8) / known_count
                else:
                    if stats["health_scores"]:
                        avg_health = sum(stats["health_scores"]) / len(stats["health_scores"])
                    elif stats["total_keys"] == 0:
                        avg_health = 0.0
                    else:
                        avg_health = 0.1
            else:
                avg_health = 0.0

            item = {
                "api_format": api_format,
                "display_name": EndpointHealthService._format_display_name(api_format),
                "health_score": avg_health,
                "timeline": timeline,
                "time_range_start": time_range_start.isoformat() if time_range_start else None,
                "time_range_end": time_range_end.isoformat() if time_range_end else None,
            }

            if include_admin_fields:
                item.update(
                    {
                        "total_endpoints": stats["total_endpoints"],
                        "total_keys": stats["total_keys"],
                        "active_keys": stats["active_keys"],
                        "provider_count": len(stats["provider_ids"]),
                    }
                )

            result.append(item)

        result.sort(key=lambda x: x["health_score"], reverse=True)

        # 写入缓存
        if use_cache:
            EndpointHealthService._set_to_cache(cache_key, result)

        return result

    @staticmethod
    def _generate_timeline_batch(
        db: Session,
        format_endpoint_mapping: dict[str, list[str]],
        now: datetime,
        lookback_hours: int,
        segments: int = 100,
    ) -> dict[str, dict[str, Any]]:
        """
        批量生成多个 API 格式的时间线数据（基于 RequestCandidate 表）

        使用 RequestCandidate 表可以：
        1. 记录所有尝试（包括 fallback 中失败的尝试）
        2. 准确反映每个 Provider/Key 的真实健康状态
        3. 失败的请求会显示为红色节点

        Args:
            db: 数据库会话
            format_endpoint_mapping: API格式 -> endpoint_ids 的映射
            now: 当前时间
            lookback_hours: 回溯小时数
            segments: 时间段数量

        Returns:
            API格式 -> 时间线数据的映射
        """
        # 收集所有 endpoint_ids
        all_endpoint_ids = []
        for endpoint_ids in format_endpoint_mapping.values():
            all_endpoint_ids.extend(endpoint_ids)

        if not all_endpoint_ids:
            return {
                api_format: {
                    "timeline": ["unknown"] * 100,
                    "time_range_start": None,
                    "time_range_end": None,
                }
                for api_format in format_endpoint_mapping.keys()
            }

        # 参数校验（API 层已通过 Query(ge=1) 保证，这里做防御性检查）
        if lookback_hours <= 0 or segments <= 0:
            raise ValueError(
                f"lookback_hours and segments must be positive, "
                f"got lookback_hours={lookback_hours}, segments={segments}"
            )

        # 计算时间范围
        segment_seconds = (lookback_hours * 3600) / segments
        start_time = now - timedelta(hours=lookback_hours)

        # 使用 RequestCandidate 表查询所有尝试记录
        # 只统计最终状态：success, failed, skipped
        final_statuses = ["success", "failed", "skipped"]

        segment_expr = func.floor(
            func.extract("epoch", RequestCandidate.created_at - start_time) / segment_seconds
        ).label("segment_idx")

        candidate_stats = (
            db.query(
                RequestCandidate.endpoint_id,
                segment_expr,
                func.count(RequestCandidate.id).label("total_count"),
                func.sum(case((RequestCandidate.status == "success", 1), else_=0)).label(
                    "success_count"
                ),
                func.sum(case((RequestCandidate.status == "failed", 1), else_=0)).label(
                    "failed_count"
                ),
                func.min(RequestCandidate.created_at).label("min_time"),
                func.max(RequestCandidate.created_at).label("max_time"),
            )
            .filter(
                RequestCandidate.endpoint_id.in_(all_endpoint_ids),
                RequestCandidate.created_at >= start_time,
                RequestCandidate.created_at <= now,
                RequestCandidate.status.in_(final_statuses),
            )
            .group_by(RequestCandidate.endpoint_id, segment_expr)
            .all()
        )

        # 构建 endpoint_id -> api_format 的反向映射
        endpoint_to_format: dict[str, str] = {}
        for api_format, endpoint_ids in format_endpoint_mapping.items():
            for endpoint_id in endpoint_ids:
                endpoint_to_format[endpoint_id] = api_format

        # 按 api_format 和 segment 聚合数据
        format_segment_data: dict[str, dict[int, dict]] = defaultdict(
            lambda: defaultdict(
                lambda: {
                    "total": 0,
                    "success": 0,
                    "failed": 0,
                    "min_time": None,
                    "max_time": None,
                }
            )
        )

        for row in candidate_stats:
            endpoint_id = row.endpoint_id
            segment_idx = int(row.segment_idx) if row.segment_idx is not None else 0
            api_format = endpoint_to_format.get(endpoint_id)

            if api_format and 0 <= segment_idx < segments:
                seg_data = format_segment_data[api_format][segment_idx]
                seg_data["total"] += row.total_count or 0
                seg_data["success"] += row.success_count or 0
                seg_data["failed"] += row.failed_count or 0

                if row.min_time:
                    if seg_data["min_time"] is None or row.min_time < seg_data["min_time"]:
                        seg_data["min_time"] = row.min_time
                if row.max_time:
                    if seg_data["max_time"] is None or row.max_time > seg_data["max_time"]:
                        seg_data["max_time"] = row.max_time

        # 生成各格式的时间线
        result: dict[str, dict[str, Any]] = {}

        for api_format in format_endpoint_mapping.keys():
            timeline = []
            earliest_time = None
            latest_time = None

            segment_data = format_segment_data.get(api_format, {})

            for i in range(segments):
                seg = segment_data.get(i)
                if not seg or seg["total"] == 0:
                    timeline.append("unknown")
                else:
                    # 更新时间范围
                    if seg["min_time"]:
                        if earliest_time is None or seg["min_time"] < earliest_time:
                            earliest_time = seg["min_time"]
                    if seg["max_time"]:
                        if latest_time is None or seg["max_time"] > latest_time:
                            latest_time = seg["max_time"]

                    # 计算成功率 = success / (success + failed)
                    # skipped 不算失败，不影响成功率
                    actual_completed = seg["success"] + seg["failed"]
                    if actual_completed > 0:
                        success_rate = seg["success"] / actual_completed
                    else:
                        # 只有 skipped，视为健康
                        success_rate = 1.0

                    if success_rate >= 0.95:
                        timeline.append("healthy")
                    elif success_rate >= 0.7:
                        timeline.append("warning")
                    else:
                        timeline.append("unhealthy")

            result[api_format] = {
                "timeline": timeline,
                "time_range_start": earliest_time,
                "time_range_end": latest_time if latest_time else now,
            }

        return result

    @staticmethod
    def _generate_timeline_from_usage(
        db: Session,
        endpoint_ids: list[str],
        now: datetime,
        lookback_hours: int,
        segments: int = 100,
    ) -> dict[str, Any]:
        """
        从真实使用记录生成时间线数据（使用批量查询优化）

        Args:
            db: 数据库会话
            endpoint_ids: 端点ID列表
            now: 当前时间
            lookback_hours: 回溯小时数
            segments: 时间段数量

        Returns:
            包含时间线和时间范围的字典
        """
        if not endpoint_ids:
            return {
                "timeline": ["unknown"] * 100,
                "time_range_start": None,
                "time_range_end": None,
            }

        # 直接按 endpoint_id 聚合，避免共享 Key 时把不同 API 格式串桶。
        format_endpoint_mapping = {"_single": endpoint_ids}
        result = EndpointHealthService._generate_timeline_batch(
            db, format_endpoint_mapping, now, lookback_hours, segments
        )

        return result.get(
            "_single",
            {
                "timeline": ["unknown"] * 100,
                "time_range_start": None,
                "time_range_end": None,
            },
        )

    @staticmethod
    def _format_display_name(api_format: str) -> str:
        """格式化 API 格式的显示名称"""
        raw = str(api_format or "").strip()
        normalized = raw.lower()
        if ":" not in normalized:
            return raw or api_format

        fam, kind = normalized.split(":", 1)
        fam_label = {"claude": "Claude", "openai": "OpenAI", "gemini": "Gemini"}.get(fam, fam)
        kind_label = {
            "chat": "Chat",
            "cli": "CLI",
            "compact": "Compact",
            "video": "Video",
            "image": "Image",
        }.get(kind, kind)
        return f"{fam_label} {kind_label}"

    @staticmethod
    def _get_from_cache(key: str) -> list[dict[str, Any]] | None:
        """从 Redis 缓存获取数据"""
        redis_client = _get_redis_client()
        if not redis_client:
            return None

        try:
            data = redis_client.get(key)
            if data:
                return json.loads(data)
        except Exception as e:
            logger.warning(f"Failed to get from cache: {e}")
        return None

    @staticmethod
    def _set_to_cache(key: str, data: list[dict[str, Any]]) -> None:
        """写入 Redis 缓存"""
        redis_client = _get_redis_client()
        if not redis_client:
            return

        try:
            redis_client.setex(key, CACHE_TTL_SECONDS, json.dumps(data, default=str))
        except Exception as e:
            logger.warning(f"Failed to set cache: {e}")
