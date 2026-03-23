from __future__ import annotations

from datetime import datetime, timedelta, timezone
from typing import Any

from sqlalchemy import func
from sqlalchemy.orm import Session

from src.core.api_format.metadata import can_passthrough_endpoint
from src.core.api_format.signature import normalize_signature_key
from src.core.logger import logger
from src.models.database import RequestCandidate, Usage


class UsageActiveRequestsMixin:
    """活跃请求管理方法"""

    @staticmethod
    def _find_completed_request_ids(
        db: Session,
        request_ids: list[str],
    ) -> set[str]:
        """查询已成功完成的 request_id 集合

        通过 RequestCandidate 表判断哪些请求实际已成功完成：
        1. status='success' 且 stream_completed=True（正常完成）
        2. status='streaming'（Provider 已返回成功响应头，流因重启中断）
        """
        if not request_ids:
            return set()

        from sqlalchemy import or_

        candidates = (
            db.query(
                RequestCandidate.request_id,
                RequestCandidate.status,
                RequestCandidate.extra_data,
            )
            .filter(
                RequestCandidate.request_id.in_(request_ids),
                or_(
                    RequestCandidate.status == "success",
                    RequestCandidate.status == "streaming",
                ),
            )
            .all()
        )
        completed: set[str] = set()
        for c in candidates:
            extra_data = c.extra_data or {}
            if c.status == "success" and extra_data.get("stream_completed", False):
                completed.add(c.request_id)
            elif c.status == "streaming":
                completed.add(c.request_id)
        return completed

    @staticmethod
    def _sync_candidate_status_to_success(
        db: Session,
        request_ids: list[str],
        now: datetime | None = None,
    ) -> None:
        """将指定请求的 streaming candidate 同步更新为 success"""
        if not request_ids:
            return
        if now is None:
            now = datetime.now(timezone.utc)
        db.query(RequestCandidate).filter(
            RequestCandidate.request_id.in_(request_ids),
            RequestCandidate.status == "streaming",
        ).update(
            {"status": "success", "finished_at": now},
            synchronize_session=False,
        )

    @classmethod
    def get_active_requests(
        cls,
        db: Session,
        user_id: str | None = None,
        limit: int = 50,
    ) -> list[Usage]:
        """
        获取活跃的请求（pending 或 streaming 状态）

        Args:
            db: 数据库会话
            user_id: 用户ID（可选，用于过滤）
            limit: 最大返回数量

        Returns:
            活跃请求的 Usage 列表
        """
        query = db.query(Usage).filter(Usage.status.in_(["pending", "streaming"]))

        if user_id:
            query = query.filter(Usage.user_id == user_id)

        return query.order_by(Usage.created_at.desc()).limit(limit).all()

    @classmethod
    def cleanup_stale_pending_requests(
        cls,
        db: Session,
        timeout_minutes: int = 10,
        batch_size: int = 200,
    ) -> int:
        """
        清理超时的 pending/streaming 请求

        将超过指定时间仍处于 pending 或 streaming 状态的请求标记为 failed 或恢复为 completed。
        会检查 RequestCandidate 表，如果 Provider 已返回成功响应（status=streaming 或 stream_completed），
        则恢复为 completed 而非标记为 failed，同时同步更新 candidate 状态。

        Args:
            db: 数据库会话
            timeout_minutes: 超时时间（分钟），默认 10 分钟
            batch_size: 每次处理的记录数，限制在 1-200 之间

        Returns:
            清理的记录数
        """
        now = datetime.now(timezone.utc)
        cutoff_time = now - timedelta(minutes=timeout_minutes)

        batch_size = max(1, batch_size)
        failed_count = 0
        recovered_count = 0

        while True:
            stale_requests = (
                db.query(Usage.id, Usage.request_id, Usage.status, Usage.billing_status)
                .filter(
                    Usage.status.in_(["pending", "streaming"]),
                    Usage.created_at < cutoff_time,
                )
                .order_by(Usage.created_at.asc(), Usage.id.asc())
                .limit(batch_size)
                .all()
            )

            if not stale_requests:
                break

            stale_request_ids = [request_id for _, request_id, _, _ in stale_requests if request_id]
            completed_request_ids = cls._find_completed_request_ids(db, stale_request_ids)

            usage_updates = []
            failed_request_ids: list[str] = []

            for usage_id, request_id, old_status, billing_status in stale_requests:
                if request_id and request_id in completed_request_ids:
                    usage_updates.append(
                        {
                            "id": usage_id,
                            "status": "completed",
                            "status_code": 200,
                            "error_message": None,
                        }
                    )
                    recovered_count += 1
                else:
                    entry: dict[str, Any] = {
                        "id": usage_id,
                        "status": "failed",
                        "status_code": 504,
                        "error_message": (
                            f"请求超时: 状态 '{old_status}' 超过 {timeout_minutes} 分钟未完成"
                        ),
                    }
                    if billing_status == "pending":
                        entry["billing_status"] = "void"
                        entry["finalized_at"] = now
                        entry["total_cost_usd"] = 0.0
                        entry["request_cost_usd"] = 0.0
                        entry["actual_total_cost_usd"] = 0.0
                        entry["actual_request_cost_usd"] = 0.0
                    usage_updates.append(entry)
                    failed_count += 1
                    if request_id:
                        failed_request_ids.append(request_id)

            if usage_updates:
                db.bulk_update_mappings(Usage, usage_updates)

            cls._sync_candidate_status_to_success(db, list(completed_request_ids), now)

            if failed_request_ids:
                db.query(RequestCandidate).filter(
                    RequestCandidate.request_id.in_(failed_request_ids),
                    RequestCandidate.status.in_(["streaming", "pending"]),
                ).update(
                    {
                        "status": "failed",
                        "finished_at": now,
                        "error_message": "请求超时（服务器可能已重启）",
                    },
                    synchronize_session=False,
                )

            db.commit()
            db.expunge_all()

        total = failed_count + recovered_count
        if total > 0:
            parts = []
            if failed_count:
                parts.append(f"{failed_count} 条标记为 failed")
            if recovered_count:
                parts.append(f"{recovered_count} 条恢复为 completed")
            logger.info(
                f"清理超时请求: 超过 {timeout_minutes} 分钟的 pending/streaming 请求 - "
                + ", ".join(parts)
            )

        return total

    @classmethod
    def get_stale_pending_count(
        cls,
        db: Session,
        timeout_minutes: int = 10,
    ) -> int:
        """
        获取超时的 pending/streaming 请求数量（用于监控）

        Args:
            db: 数据库会话
            timeout_minutes: 超时时间（分钟）

        Returns:
            超时请求数量
        """
        cutoff_time = datetime.now(timezone.utc) - timedelta(minutes=timeout_minutes)

        return int(
            db.query(func.count(Usage.id))
            .filter(
                Usage.status.in_(["pending", "streaming"]),
                Usage.created_at < cutoff_time,
            )
            .scalar()
            or 0
        )

    @classmethod
    def get_active_requests_status(
        cls,
        db: Session,
        ids: list[str] | None = None,
        user_id: str | None = None,
        api_key_id: str | None = None,
        default_timeout_seconds: int = 300,
        *,
        include_admin_fields: bool = False,
        maintain_status: bool | None = None,
    ) -> list[dict[str, Any]]:
        """
        获取活跃请求状态（用于前端轮询）。

        与 get_active_requests 不同，此方法：
        1. 返回轻量级的状态字典而非完整 Usage 对象
        2. 可选地检测并清理超时的 pending/streaming 请求
        3. 支持按 ID 列表查询特定请求

        Args:
            db: 数据库会话
            ids: 指定要查询的请求 ID 列表（可选）
            user_id: 限制只查询该用户的请求（可选，用于普通用户接口）
            api_key_id: 限制只查询该 API Key 的请求（可选）
            default_timeout_seconds: 默认超时时间（秒），当端点未配置时使用
            maintain_status: 是否执行超时修复与状态回写；默认仅在全量活跃请求查询时执行

        Returns:
            请求状态列表
        """
        now = datetime.now(timezone.utc)

        # 构建基础查询
        query = db.query(
            Usage.id,
            Usage.status,
            Usage.input_tokens,
            Usage.output_tokens,
            Usage.cache_creation_input_tokens,
            Usage.cache_read_input_tokens,
            Usage.total_cost_usd,
            Usage.actual_total_cost_usd,
            Usage.rate_multiplier,
            Usage.response_time_ms,
            Usage.first_byte_time_ms,  # 首字时间 (TTFB)
            Usage.created_at,
            Usage.provider_endpoint_id,
            # API 格式 / 格式转换（streaming 状态时已可确定）
            Usage.api_format,
            Usage.endpoint_api_format,
            Usage.has_format_conversion,
            # 模型映射（streaming 时已可确定）
            Usage.target_model,
        )

        # 管理员轮询：可附带 provider 与上游 key 名称（注意：不要在普通用户接口暴露上游 key 信息）
        if include_admin_fields:
            from src.models.database import ProviderAPIKey

            query = query.add_columns(
                Usage.provider_name,
                ProviderAPIKey.name.label("api_key_name"),
            ).outerjoin(ProviderAPIKey, Usage.provider_api_key_id == ProviderAPIKey.id)

        if ids:
            query = query.filter(Usage.id.in_(ids))
            if user_id:
                query = query.filter(Usage.user_id == user_id)
            if api_key_id:
                query = query.filter(Usage.api_key_id == api_key_id)
        else:
            # 查询所有活跃请求
            query = query.filter(Usage.status.in_(["pending", "streaming"]))
            if user_id:
                query = query.filter(Usage.user_id == user_id)
            if api_key_id:
                query = query.filter(Usage.api_key_id == api_key_id)
            query = query.order_by(Usage.created_at.desc()).limit(50)

        records = query.all()
        should_maintain_status = maintain_status if maintain_status is not None else not ids

        # 检查超时的 pending/streaming 请求
        # 收集可能超时的 usage_id 列表
        timeout_candidates: list[str] = []
        if should_maintain_status:
            for r in records:
                if r.status in ("pending", "streaming") and r.created_at:
                    timeout_seconds = default_timeout_seconds

                    created_at = r.created_at
                    if created_at.tzinfo is None:
                        created_at = created_at.replace(tzinfo=timezone.utc)
                    elapsed = (now - created_at).total_seconds()
                    if elapsed > timeout_seconds:
                        timeout_candidates.append(r.id)

        # 批量更新超时的请求（排除已有成功完成记录的请求）
        timeout_ids = []
        if should_maintain_status and timeout_candidates:
            # 先获取这些 Usage 的 request_id
            usage_request_ids = (
                db.query(Usage.id, Usage.request_id).filter(Usage.id.in_(timeout_candidates)).all()
            )
            usage_id_to_request_id = {u.id: u.request_id for u in usage_request_ids}
            request_id_to_usage_id = {u.request_id: u.id for u in usage_request_ids}
            request_ids = list(request_id_to_usage_id.keys())

            # 查询已成功完成的 request_id
            completed_rids = cls._find_completed_request_ids(db, request_ids)
            completed_usage_ids = {
                request_id_to_usage_id[rid]
                for rid in completed_rids
                if rid in request_id_to_usage_id
            }

            # 只对没有成功完成记录的请求标记超时
            timeout_ids = [uid for uid in timeout_candidates if uid not in completed_usage_ids]

            if timeout_ids:
                db.query(Usage).filter(Usage.id.in_(timeout_ids)).update(
                    {"status": "failed", "error_message": "请求超时（服务器可能已重启）"},
                    synchronize_session=False,
                )
                db.commit()

            # 对于已完成但状态未更新的请求，主动恢复状态为 completed
            if completed_usage_ids:
                db.query(Usage).filter(Usage.id.in_(list(completed_usage_ids))).update(
                    {"status": "completed"},
                    synchronize_session=False,
                )
                # 同步更新 candidate 状态：streaming -> success
                completed_request_ids = [
                    usage_id_to_request_id[uid]
                    for uid in completed_usage_ids
                    if uid in usage_id_to_request_id
                ]
                cls._sync_candidate_status_to_success(db, completed_request_ids)
                db.commit()
                logger.info(
                    "[Usage] 恢复 {} 个已完成请求的状态（遥测回调丢失）",
                    len(completed_usage_ids),
                )

        result: list[dict[str, Any]] = []
        for r in records:
            api_format = getattr(r, "api_format", None)
            endpoint_api_format = getattr(r, "endpoint_api_format", None)
            has_format_conversion = getattr(r, "has_format_conversion", None)

            # 兼容历史数据：当 streaming 状态已拿到两个格式但 has_format_conversion 为空时，回填推断结果
            if has_format_conversion is None and api_format and endpoint_api_format:
                client_raw = str(api_format).strip()
                endpoint_raw = str(endpoint_api_format).strip()
                if ":" in client_raw and ":" in endpoint_raw:
                    client_fmt = normalize_signature_key(client_raw)
                    endpoint_fmt = normalize_signature_key(endpoint_raw)
                    has_format_conversion = not can_passthrough_endpoint(client_fmt, endpoint_fmt)

            item: dict[str, Any] = {
                "id": r.id,
                "status": "failed" if r.id in timeout_ids else r.status,
                "input_tokens": r.input_tokens,
                "output_tokens": r.output_tokens,
                "cache_creation_input_tokens": r.cache_creation_input_tokens,
                "cache_read_input_tokens": r.cache_read_input_tokens,
                "cost": float(r.total_cost_usd) if r.total_cost_usd else 0,
                "actual_cost": (
                    float(r.actual_total_cost_usd) if r.actual_total_cost_usd is not None else None
                ),
                "rate_multiplier": (
                    float(r.rate_multiplier) if r.rate_multiplier is not None else None
                ),
                "response_time_ms": r.response_time_ms,
                "first_byte_time_ms": r.first_byte_time_ms,  # 首字时间 (TTFB)
            }
            if api_format:
                item["api_format"] = api_format
            if endpoint_api_format:
                item["endpoint_api_format"] = endpoint_api_format
            if has_format_conversion is not None:
                item["has_format_conversion"] = bool(has_format_conversion)
            # 模型映射（streaming 时已可确定）
            if r.target_model:
                item["target_model"] = r.target_model
            if include_admin_fields:
                item["provider"] = r.provider_name
                item["api_key_name"] = r.api_key_name
            result.append(item)

        return result
