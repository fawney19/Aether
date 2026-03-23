from __future__ import annotations

from collections import defaultdict
from dataclasses import dataclass, replace
from datetime import datetime, timedelta, timezone
from decimal import Decimal
import math
from typing import Any, Literal

from sqlalchemy import and_, case, func, or_
from sqlalchemy.orm import Query, Session

from src.core.enums import ErrorCategory, UserRole
from src.core.model_permissions import match_model_with_pattern
from src.models.database import (
    ApiKey,
    GlobalModel,
    Model,
    Provider,
    ProviderAPIKey,
    RequestCandidate,
    Usage,
    User,
)
from src.services.system.time_range import TimeRangeParams
from src.utils.database_helpers import escape_like_pattern, safe_truncate_escaped

TERMINAL_STATUSES = ("completed", "failed", "cancelled")
EXCLUDED_ANALYTICS_REQUEST_TYPES = ("endpoint_test",)
ScopeKind = Literal["global", "me", "user", "api_key"]
BreakdownDimension = Literal["model", "provider", "api_format", "api_key", "user"]
BreakdownMetric = Literal["requests_total", "total_tokens", "total_cost_usd", "actual_total_cost_usd"]
LeaderboardEntity = Literal["user", "api_key"]
LeaderboardMetric = Literal["requests_total", "total_tokens", "total_cost_usd"]
DELETED_USER_FILTER = "__deleted_user__"
DELETED_API_KEY_FILTER = "__deleted_api_key__"
DELETED_USER_LABEL = "已删除用户"
DELETED_API_KEY_LABEL = "已删除Key"
PENDING_PROVIDER_LABEL = "待分配提供商"
UNKNOWN_PROVIDER_LABEL = "未识别提供商"
ERROR_CATEGORY_LABELS = {
    ErrorCategory.RATE_LIMIT.value: "频率限制",
    ErrorCategory.AUTH.value: "认证失败",
    ErrorCategory.INVALID_REQUEST.value: "请求无效",
    ErrorCategory.NOT_FOUND.value: "资源不存在",
    ErrorCategory.CONTENT_FILTER.value: "内容过滤",
    ErrorCategory.CONTEXT_LENGTH.value: "上下文过长",
    ErrorCategory.SERVER_ERROR.value: "服务端错误",
    ErrorCategory.TIMEOUT.value: "请求超时",
    ErrorCategory.NETWORK.value: "网络错误",
    ErrorCategory.CANCELLED.value: "已取消",
    ErrorCategory.UNKNOWN.value: "未知错误",
}
STATUS_OPTION_ORDER = (
    "pending",
    "streaming",
    "completed",
    "failed",
    "cancelled",
    "active",
    "stream",
    "standard",
    "has_retry",
    "has_fallback",
)


@dataclass(slots=True)
class AnalyticsFilters:
    user_ids: list[str]
    provider_names: list[str]
    models: list[str]
    target_models: list[str]
    api_key_ids: list[str]
    api_formats: list[str]
    request_types: list[str]
    statuses: list[str]
    error_categories: list[str]
    is_stream: bool | None
    has_format_conversion: bool | None


@dataclass(slots=True)
class AnalyticsSearch:
    text: str | None
    request_id: str | None


def _to_float(value: Decimal | float | int | None) -> float:
    if value is None:
        return 0.0
    return float(value)


def _to_int(value: Any) -> int:
    return int(value or 0)


def _round2(value: float) -> float:
    return round(value, 2)


def _cache_hit_rate(input_context_tokens: int, cache_read_tokens: int) -> float:
    if input_context_tokens <= 0:
        return 0.0
    return round(cache_read_tokens / input_context_tokens * 100, 2)


def _provider_display_label(value: Any) -> str:
    normalized = str(value or "").strip()
    if normalized == "pending":
        return PENDING_PROVIDER_LABEL
    if normalized == "unknown":
        return UNKNOWN_PROVIDER_LABEL
    return normalized


def _error_category_display_label(value: Any) -> str:
    normalized = str(value or "").strip().lower()
    if not normalized:
        return ERROR_CATEGORY_LABELS[ErrorCategory.UNKNOWN.value]
    return ERROR_CATEGORY_LABELS.get(normalized, normalized.replace("_", " "))


def _percentile(values: list[float], q: float) -> int | None:
    if not values:
        return None
    sorted_values = sorted(values)
    if len(sorted_values) == 1:
        return int(round(sorted_values[0]))
    position = (len(sorted_values) - 1) * q
    lower = math.floor(position)
    upper = math.ceil(position)
    if lower == upper:
        return int(round(sorted_values[lower]))
    weight = position - lower
    interpolated = sorted_values[lower] * (1 - weight) + sorted_values[upper] * weight
    return int(round(interpolated))


def _bucket_tz(params: TimeRangeParams) -> timezone:
    if params.timezone:
        from zoneinfo import ZoneInfo

        try:
            return ZoneInfo(params.timezone)  # type: ignore[return-value]
        except Exception:
            pass
    return timezone(timedelta(minutes=params.tz_offset_minutes))


def _bucket_start(value: datetime, params: TimeRangeParams) -> datetime:
    tzinfo = _bucket_tz(params)
    local = value.astimezone(tzinfo)

    if params.granularity == "hour":
        return local.replace(minute=0, second=0, microsecond=0)
    if params.granularity == "week":
        start = local - timedelta(days=local.weekday())
        return start.replace(hour=0, minute=0, second=0, microsecond=0)
    if params.granularity == "month":
        return local.replace(day=1, hour=0, minute=0, second=0, microsecond=0)
    return local.replace(hour=0, minute=0, second=0, microsecond=0)


def _bucket_end(start: datetime, granularity: str) -> datetime:
    if granularity == "hour":
        return start + timedelta(hours=1)
    if granularity == "week":
        return start + timedelta(days=7)
    if granularity == "month":
        if start.month == 12:
            return start.replace(year=start.year + 1, month=1)
        return start.replace(month=start.month + 1)
    return start + timedelta(days=1)


class AnalyticsQueryService:
    @staticmethod
    def _successful_request_clause() -> Any:
        return and_(
            Usage.status != "failed",
            or_(Usage.status_code.is_(None), Usage.status_code < 400),
            Usage.error_message.is_(None),
        )

    @staticmethod
    def _retry_request_ids_subquery(db: Session) -> Any:
        return (
            db.query(RequestCandidate.request_id)
            .filter(
                RequestCandidate.status.in_(["success", "failed"]),
                RequestCandidate.retry_index > 0,
            )
            .distinct()
            .subquery()
        )

    @staticmethod
    def _fallback_request_ids_subquery(db: Session) -> Any:
        return (
            db.query(RequestCandidate.request_id)
            .filter(RequestCandidate.status.in_(["success", "failed"]))
            .group_by(RequestCandidate.request_id)
            .having(func.count(func.distinct(RequestCandidate.candidate_index)) > 1)
            .subquery()
        )

    @classmethod
    def _apply_status_filters(
        cls,
        db: Session,
        query: Query[Any],
        filters: AnalyticsFilters,
        *,
        include_non_terminal: bool,
    ) -> Query[Any]:
        if not filters.statuses:
            if include_non_terminal:
                return query
            return query.filter(Usage.status.in_(TERMINAL_STATUSES))

        clauses = [
            clause
            for status in filters.statuses
            if (clause := cls._status_clause(db, status)) is not None
        ]

        if not clauses:
            return query

        return query.filter(or_(*clauses))

    @classmethod
    def _status_clause(cls, db: Session, status: str) -> Any | None:
        if not status:
            return None
        if status == "stream":
            return and_(
                Usage.is_stream.is_(True),
                cls._successful_request_clause(),
            )
        if status == "standard":
            return and_(
                Usage.is_stream.is_(False),
                cls._successful_request_clause(),
            )
        if status == "active":
            return Usage.status.in_(["pending", "streaming"])
        if status == "failed":
            return or_(
                Usage.status == "failed",
                Usage.status_code >= 400,
                Usage.error_message.isnot(None),
            )
        if status == "has_retry":
            return Usage.request_id.in_(cls._retry_request_ids_subquery(db))
        if status == "has_fallback":
            return Usage.request_id.in_(cls._fallback_request_ids_subquery(db))
        return Usage.status == status

    @staticmethod
    def _compose_status_options(
        *,
        raw_statuses: set[str],
        has_active: bool,
        has_stream: bool,
        has_standard: bool,
        has_retry: bool,
        has_fallback: bool,
    ) -> list[dict[str, str]]:
        present_statuses = {status for status in raw_statuses if status}
        if has_active:
            present_statuses.add("active")
        if has_stream:
            present_statuses.add("stream")
        if has_standard:
            present_statuses.add("standard")
        if has_retry:
            present_statuses.add("has_retry")
        if has_fallback:
            present_statuses.add("has_fallback")

        return [
            {"value": status, "label": status}
            for status in STATUS_OPTION_ORDER
            if status in present_statuses
        ]

    @staticmethod
    def _load_request_execution_flags(
        db: Session,
        request_ids: list[str],
    ) -> tuple[dict[str, bool], dict[str, bool]]:
        if not request_ids:
            return {}, {}

        executed_candidates = (
            db.query(
                RequestCandidate.request_id,
                RequestCandidate.candidate_index,
                RequestCandidate.retry_index,
            )
            .filter(
                RequestCandidate.request_id.in_(request_ids),
                RequestCandidate.status.in_(["success", "failed"]),
            )
            .all()
        )

        fallback_map: dict[str, bool] = {}
        retry_map: dict[str, bool] = {}
        request_candidates: dict[str, list[tuple[int | None, int | None]]] = defaultdict(list)

        for request_id, candidate_index, retry_index in executed_candidates:
            request_candidates[str(request_id)].append((candidate_index, retry_index))

        for request_id, candidates in request_candidates.items():
            unique_candidates = {candidate_index for candidate_index, _ in candidates}
            fallback_map[request_id] = len(unique_candidates) > 1
            retry_map[request_id] = any((retry_index or 0) > 0 for _, retry_index in candidates)

        return fallback_map, retry_map

    @staticmethod
    def _resolve_model_display_names(db: Session, model_names: list[str]) -> dict[str, str]:
        normalized_names = sorted({str(name).strip() for name in model_names if str(name).strip()})
        if not normalized_names:
            return {}

        resolved: dict[str, str] = {}

        direct_rows = (
            db.query(GlobalModel.name, GlobalModel.display_name)
            .filter(GlobalModel.is_active.is_(True), GlobalModel.name.in_(normalized_names))
            .all()
        )
        for raw_name, display_name in direct_rows:
            key = str(raw_name or "").strip()
            if key:
                resolved[key] = str(display_name or raw_name)

        unresolved_names = [name for name in normalized_names if name not in resolved]
        if unresolved_names:
            provider_rows = (
                db.query(Model.provider_model_name, GlobalModel.display_name, GlobalModel.name)
                .join(Provider, Model.provider_id == Provider.id)
                .join(GlobalModel, Model.global_model_id == GlobalModel.id)
                .filter(
                    Provider.is_active.is_(True),
                    Model.is_active.is_(True),
                    GlobalModel.is_active.is_(True),
                    Model.provider_model_name.in_(unresolved_names),
                )
                .order_by(GlobalModel.name.asc())
                .all()
            )
            for provider_model_name, display_name, global_model_name in provider_rows:
                key = str(provider_model_name or "").strip()
                if key and key not in resolved:
                    resolved[key] = str(display_name or global_model_name or key)

        unresolved_set = {name for name in normalized_names if name not in resolved}
        if unresolved_set:
            provider_mapping_rows = (
                db.query(Model.provider_model_mappings, GlobalModel.display_name, GlobalModel.name)
                .join(Provider, Model.provider_id == Provider.id)
                .join(GlobalModel, Model.global_model_id == GlobalModel.id)
                .filter(
                    Provider.is_active.is_(True),
                    Model.is_active.is_(True),
                    GlobalModel.is_active.is_(True),
                    Model.provider_model_mappings.isnot(None),
                )
                .order_by(GlobalModel.name.asc())
                .all()
            )
            for provider_model_mappings, display_name, global_model_name in provider_mapping_rows:
                if not isinstance(provider_model_mappings, list):
                    continue
                resolved_label = str(display_name or global_model_name or "").strip()
                if not resolved_label:
                    continue
                for mapping in provider_model_mappings:
                    if not isinstance(mapping, dict):
                        continue
                    raw_name = mapping.get("name")
                    if not isinstance(raw_name, str):
                        continue
                    key = raw_name.strip()
                    if key in unresolved_set and key not in resolved:
                        resolved[key] = resolved_label

        unresolved_set = {name for name in normalized_names if name not in resolved}
        if unresolved_set:
            model_mapping_rows = (
                db.query(GlobalModel.name, GlobalModel.display_name, GlobalModel.config)
                .filter(GlobalModel.is_active.is_(True))
                .order_by(GlobalModel.name.asc())
                .all()
            )
            for global_model_name, display_name, config in model_mapping_rows:
                patterns = config.get("model_mappings") if isinstance(config, dict) else None
                if not isinstance(patterns, list):
                    continue
                resolved_label = str(display_name or global_model_name or "").strip()
                if not resolved_label:
                    continue
                for unresolved_name in list(unresolved_set):
                    if unresolved_name in resolved:
                        continue
                    if any(
                        isinstance(pattern, str) and match_model_with_pattern(pattern, unresolved_name)
                        for pattern in patterns
                    ):
                        resolved[unresolved_name] = resolved_label
                        unresolved_set.discard(unresolved_name)

        return resolved

    @staticmethod
    def _apply_scope(
        query: Query[Any],
        current_user: User,
        scope_kind: ScopeKind,
        scope_user_id: str | None = None,
        scope_api_key_id: str | None = None,
    ) -> Query[Any]:
        if scope_kind == "me":
            return query.filter(Usage.user_id == current_user.id)

        if current_user.role != UserRole.ADMIN:
            raise PermissionError("Only admin can access non-personal analytics scope")

        if scope_kind == "user":
            if not scope_user_id:
                raise ValueError("scope.user_id is required for user scope")
            if scope_user_id == DELETED_USER_FILTER:
                return query.filter(Usage.user_id.is_(None))
            return query.filter(Usage.user_id == scope_user_id)

        if scope_kind == "api_key":
            if not scope_api_key_id:
                raise ValueError("scope.api_key_id is required for api_key scope")
            return query.filter(Usage.api_key_id == scope_api_key_id)

        return query

    @staticmethod
    def _apply_filters(
        query: Query[Any],
        filters: AnalyticsFilters,
        *,
        allow_provider_filters: bool,
    ) -> Query[Any]:
        if filters.user_ids:
            wants_deleted_users = DELETED_USER_FILTER in filters.user_ids
            live_user_ids = [
                user_id
                for user_id in filters.user_ids
                if user_id != DELETED_USER_FILTER
            ]

            if wants_deleted_users and live_user_ids:
                query = query.filter(
                    or_(
                        Usage.user_id.in_(live_user_ids),
                        Usage.user_id.is_(None),
                    )
                )
            elif wants_deleted_users:
                query = query.filter(Usage.user_id.is_(None))
            elif live_user_ids:
                query = query.filter(Usage.user_id.in_(live_user_ids))
        if allow_provider_filters and filters.provider_names:
            query = query.filter(Usage.provider_name.in_(filters.provider_names))
        if filters.models:
            query = query.filter(Usage.model.in_(filters.models))
        if filters.target_models:
            query = query.filter(Usage.target_model.in_(filters.target_models))
        if filters.api_key_ids:
            wants_deleted_api_keys = DELETED_API_KEY_FILTER in filters.api_key_ids
            live_api_key_ids = [
                api_key_id
                for api_key_id in filters.api_key_ids
                if api_key_id != DELETED_API_KEY_FILTER
            ]

            if wants_deleted_api_keys and live_api_key_ids:
                query = query.filter(
                    or_(
                        Usage.api_key_id.in_(live_api_key_ids),
                        Usage.api_key_id.is_(None),
                    )
                )
            elif wants_deleted_api_keys:
                query = query.filter(Usage.api_key_id.is_(None))
            elif live_api_key_ids:
                query = query.filter(Usage.api_key_id.in_(live_api_key_ids))
        if filters.api_formats:
            query = query.filter(Usage.api_format.in_(filters.api_formats))
        if filters.request_types:
            query = query.filter(Usage.request_type.in_(filters.request_types))
        if filters.error_categories:
            query = query.filter(Usage.error_category.in_(filters.error_categories))
        if filters.is_stream is not None:
            query = query.filter(Usage.is_stream.is_(filters.is_stream))
        if filters.has_format_conversion is not None:
            query = query.filter(Usage.has_format_conversion.is_(filters.has_format_conversion))
        return query

    @classmethod
    def build_usage_query(
        cls,
        db: Session,
        current_user: User,
        *,
        time_range: TimeRangeParams,
        scope_kind: ScopeKind,
        scope_user_id: str | None,
        scope_api_key_id: str | None,
        filters: AnalyticsFilters,
        include_non_terminal: bool = False,
        include_pending_providers: bool = False,
    ) -> Query[Any]:
        start_utc, end_utc = time_range.to_utc_datetime_range()
        query: Query[Any] = db.query(Usage).filter(
            Usage.created_at >= start_utc,
            Usage.created_at < end_utc,
        )
        query = query.filter(
            or_(
                Usage.request_type.is_(None),
                Usage.request_type.notin_(EXCLUDED_ANALYTICS_REQUEST_TYPES),
            )
        )
        query = cls._apply_scope(query, current_user, scope_kind, scope_user_id, scope_api_key_id)
        query = cls._apply_filters(
            query,
            filters,
            allow_provider_filters=current_user.role == UserRole.ADMIN,
        )
        query = cls._apply_status_filters(
            db,
            query,
            filters,
            include_non_terminal=include_non_terminal,
        )
        if not include_pending_providers:
            query = query.filter(Usage.provider_name.notin_(["pending", "unknown"]))
        return query

    @staticmethod
    def _summary_columns() -> list[Any]:
        success_case = case((Usage.status == "completed", 1), else_=0)
        error_case = case((Usage.status == "failed", 1), else_=0)
        stream_case = case((Usage.is_stream.is_(True), 1), else_=0)
        conversion_case = case((Usage.has_format_conversion.is_(True), 1), else_=0)
        model_name_expr = func.coalesce(func.nullif(Usage.model, ""), func.nullif(Usage.target_model, ""))

        return [
            func.count(Usage.id).label("requests_total"),
            func.coalesce(func.sum(success_case), 0).label("requests_success"),
            func.coalesce(func.sum(error_case), 0).label("requests_error"),
            func.coalesce(func.sum(stream_case), 0).label("requests_stream"),
            func.coalesce(func.sum(Usage.input_tokens), 0).label("input_tokens"),
            func.coalesce(func.sum(Usage.output_tokens), 0).label("output_tokens"),
            func.coalesce(func.sum(Usage.input_output_total_tokens), 0).label("input_output_total_tokens"),
            func.coalesce(func.sum(Usage.cache_creation_input_tokens), 0).label("cache_creation_input_tokens"),
            func.coalesce(func.sum(Usage.cache_creation_input_tokens_5m), 0).label("cache_creation_input_tokens_5m"),
            func.coalesce(func.sum(Usage.cache_creation_input_tokens_1h), 0).label("cache_creation_input_tokens_1h"),
            func.coalesce(func.sum(Usage.cache_read_input_tokens), 0).label("cache_read_input_tokens"),
            func.coalesce(func.sum(Usage.input_context_tokens), 0).label("input_context_tokens"),
            func.coalesce(func.sum(Usage.total_tokens), 0).label("total_tokens"),
            func.coalesce(func.sum(Usage.input_cost_usd), 0).label("input_cost_usd"),
            func.coalesce(func.sum(Usage.output_cost_usd), 0).label("output_cost_usd"),
            func.coalesce(func.sum(Usage.cache_creation_cost_usd), 0).label("cache_creation_cost_usd"),
            func.coalesce(func.sum(Usage.cache_creation_cost_usd_5m), 0).label("cache_creation_cost_usd_5m"),
            func.coalesce(func.sum(Usage.cache_creation_cost_usd_1h), 0).label("cache_creation_cost_usd_1h"),
            func.coalesce(func.sum(Usage.cache_read_cost_usd), 0).label("cache_read_cost_usd"),
            func.coalesce(func.sum(Usage.cache_cost_usd), 0).label("cache_cost_usd"),
            func.coalesce(func.sum(Usage.request_cost_usd), 0).label("request_cost_usd"),
            func.coalesce(func.sum(Usage.total_cost_usd), 0).label("total_cost_usd"),
            func.coalesce(func.sum(Usage.actual_total_cost_usd), 0).label("actual_total_cost_usd"),
            func.coalesce(func.sum(Usage.actual_cache_cost_usd), 0).label("actual_cache_cost_usd"),
            func.coalesce(func.avg(Usage.response_time_ms), 0).label("avg_response_time_ms"),
            func.coalesce(func.avg(Usage.first_byte_time_ms), 0).label("avg_first_byte_time_ms"),
            func.coalesce(func.sum(conversion_case), 0).label("format_conversion_count"),
            func.count(func.distinct(model_name_expr)).label("models_used_count"),
        ]

    @staticmethod
    def _breakdown_metric_expression(metric: BreakdownMetric) -> Any:
        return {
            "requests_total": func.count(Usage.id),
            "total_tokens": func.coalesce(func.sum(Usage.total_tokens), 0),
            "total_cost_usd": func.coalesce(func.sum(Usage.total_cost_usd), 0),
            "actual_total_cost_usd": func.coalesce(func.sum(Usage.actual_total_cost_usd), 0),
        }[metric]

    @classmethod
    def overview(
        cls,
        db: Session,
        current_user: User,
        *,
        time_range: TimeRangeParams,
        scope_kind: ScopeKind,
        scope_user_id: str | None,
        scope_api_key_id: str | None,
        filters: AnalyticsFilters,
    ) -> dict[str, Any]:
        query = cls.build_usage_query(
            db,
            current_user,
            time_range=time_range,
            scope_kind=scope_kind,
            scope_user_id=scope_user_id,
            scope_api_key_id=scope_api_key_id,
            filters=filters,
        )
        row = query.with_entities(*cls._summary_columns()).first()
        summary = cls._serialize_summary_row(row)
        return {
            "summary": summary,
            "composition": {
                "token_segments": cls._composition_segments(summary, token_mode=True),
                "cost_segments": cls._composition_segments(summary, token_mode=False),
            },
        }

    @classmethod
    def timeseries(
        cls,
        db: Session,
        current_user: User,
        *,
        time_range: TimeRangeParams,
        scope_kind: ScopeKind,
        scope_user_id: str | None,
        scope_api_key_id: str | None,
        filters: AnalyticsFilters,
    ) -> dict[str, Any]:
        query = cls.build_usage_query(
            db,
            current_user,
            time_range=time_range,
            scope_kind=scope_kind,
            scope_user_id=scope_user_id,
            scope_api_key_id=scope_api_key_id,
            filters=filters,
        )
        rows = query.with_entities(
            Usage.created_at,
            Usage.model,
            Usage.target_model,
            Usage.status,
            Usage.is_stream,
            Usage.has_format_conversion,
            Usage.input_tokens,
            Usage.output_tokens,
            Usage.input_output_total_tokens,
            Usage.cache_creation_input_tokens,
            Usage.cache_creation_input_tokens_5m,
            Usage.cache_creation_input_tokens_1h,
            Usage.cache_read_input_tokens,
            Usage.input_context_tokens,
            Usage.total_tokens,
            Usage.input_cost_usd,
            Usage.output_cost_usd,
            Usage.cache_creation_cost_usd,
            Usage.cache_creation_cost_usd_5m,
            Usage.cache_creation_cost_usd_1h,
            Usage.cache_read_cost_usd,
            Usage.cache_cost_usd,
            Usage.request_cost_usd,
            Usage.total_cost_usd,
            Usage.actual_total_cost_usd,
            Usage.actual_cache_cost_usd,
            Usage.response_time_ms,
            Usage.first_byte_time_ms,
        ).all()

        buckets: dict[str, dict[str, Any]] = {}
        for row in rows:
            created_at = row.created_at
            if created_at is None:
                continue
            bucket_local = _bucket_start(created_at, time_range)
            bucket_end = _bucket_end(bucket_local, time_range.granularity)
            key = bucket_local.isoformat()
            bucket = buckets.setdefault(
                key,
                {
                    "bucket_start": bucket_local.isoformat(),
                    "bucket_end": bucket_end.isoformat(),
                    "requests_total": 0,
                    "requests_success": 0,
                    "requests_error": 0,
                    "requests_stream": 0,
                    "input_tokens": 0,
                    "output_tokens": 0,
                    "input_output_total_tokens": 0,
                    "cache_creation_input_tokens": 0,
                    "cache_creation_input_tokens_5m": 0,
                    "cache_creation_input_tokens_1h": 0,
                    "cache_read_input_tokens": 0,
                    "input_context_tokens": 0,
                    "total_tokens": 0,
                    "input_cost_usd": 0.0,
                    "output_cost_usd": 0.0,
                    "cache_creation_cost_usd": 0.0,
                    "cache_creation_cost_usd_5m": 0.0,
                    "cache_creation_cost_usd_1h": 0.0,
                    "cache_read_cost_usd": 0.0,
                    "cache_cost_usd": 0.0,
                    "request_cost_usd": 0.0,
                    "total_cost_usd": 0.0,
                    "actual_total_cost_usd": 0.0,
                    "actual_cache_cost_usd": 0.0,
                    "_response_time_sum": 0.0,
                    "_response_time_count": 0,
                    "_first_byte_sum": 0.0,
                    "_first_byte_count": 0,
                    "format_conversion_count": 0,
                    "_models_used": set(),
                },
            )
            model_name = str(row.model or row.target_model or "").strip()
            if model_name:
                bucket["_models_used"].add(model_name)
            bucket["requests_total"] += 1
            if row.status == "completed":
                bucket["requests_success"] += 1
            if row.status == "failed":
                bucket["requests_error"] += 1
            if row.is_stream:
                bucket["requests_stream"] += 1
            if row.has_format_conversion:
                bucket["format_conversion_count"] += 1
            bucket["input_tokens"] += _to_int(row.input_tokens)
            bucket["output_tokens"] += _to_int(row.output_tokens)
            bucket["input_output_total_tokens"] += _to_int(row.input_output_total_tokens)
            bucket["cache_creation_input_tokens"] += _to_int(row.cache_creation_input_tokens)
            bucket["cache_creation_input_tokens_5m"] += _to_int(row.cache_creation_input_tokens_5m)
            bucket["cache_creation_input_tokens_1h"] += _to_int(row.cache_creation_input_tokens_1h)
            bucket["cache_read_input_tokens"] += _to_int(row.cache_read_input_tokens)
            bucket["input_context_tokens"] += _to_int(row.input_context_tokens)
            bucket["total_tokens"] += _to_int(row.total_tokens)
            bucket["input_cost_usd"] += _to_float(row.input_cost_usd)
            bucket["output_cost_usd"] += _to_float(row.output_cost_usd)
            bucket["cache_creation_cost_usd"] += _to_float(row.cache_creation_cost_usd)
            bucket["cache_creation_cost_usd_5m"] += _to_float(row.cache_creation_cost_usd_5m)
            bucket["cache_creation_cost_usd_1h"] += _to_float(row.cache_creation_cost_usd_1h)
            bucket["cache_read_cost_usd"] += _to_float(row.cache_read_cost_usd)
            bucket["cache_cost_usd"] += _to_float(row.cache_cost_usd)
            bucket["request_cost_usd"] += _to_float(row.request_cost_usd)
            bucket["total_cost_usd"] += _to_float(row.total_cost_usd)
            bucket["actual_total_cost_usd"] += _to_float(row.actual_total_cost_usd)
            bucket["actual_cache_cost_usd"] += _to_float(row.actual_cache_cost_usd)
            if row.response_time_ms is not None:
                bucket["_response_time_sum"] += _to_float(row.response_time_ms)
                bucket["_response_time_count"] += 1
            if row.first_byte_time_ms is not None:
                bucket["_first_byte_sum"] += _to_float(row.first_byte_time_ms)
                bucket["_first_byte_count"] += 1

        items = []
        for key in sorted(buckets):
            bucket = buckets[key]
            response_count = bucket.pop("_response_time_count")
            response_sum = bucket.pop("_response_time_sum")
            first_byte_count = bucket.pop("_first_byte_count")
            first_byte_sum = bucket.pop("_first_byte_sum")
            models_used = bucket.pop("_models_used")
            bucket["avg_response_time_ms"] = _round2(response_sum / response_count) if response_count else 0.0
            bucket["avg_first_byte_time_ms"] = _round2(first_byte_sum / first_byte_count) if first_byte_count else 0.0
            bucket["cache_hit_rate"] = _cache_hit_rate(
                int(bucket["input_context_tokens"]),
                int(bucket["cache_read_input_tokens"]),
            )
            bucket["models_used_count"] = len(models_used)
            items.append(bucket)
        return {"buckets": items}

    @classmethod
    def breakdown(
        cls,
        db: Session,
        current_user: User,
        *,
        time_range: TimeRangeParams,
        scope_kind: ScopeKind,
        scope_user_id: str | None,
        scope_api_key_id: str | None,
        filters: AnalyticsFilters,
        dimension: BreakdownDimension,
        metric: BreakdownMetric = "total_cost_usd",
        limit: int = 50,
    ) -> dict[str, Any]:
        query = cls.build_usage_query(
            db,
            current_user,
            time_range=time_range,
            scope_kind=scope_kind,
            scope_user_id=scope_user_id,
            scope_api_key_id=scope_api_key_id,
            filters=filters,
        )

        unknown_api_key_group = "__unknown_api_key__"
        unknown_user_group = "__unknown_user__"
        order_metric = cls._breakdown_metric_expression(metric)

        if dimension == "api_key":
            dimension_key_column = case(
                (Usage.api_key_id.isnot(None), Usage.api_key_id),
                else_=unknown_api_key_group,
            )
            rows = (
                query.with_entities(
                    dimension_key_column.label("dimension_key"),
                    *cls._summary_columns(),
                )
                .group_by(dimension_key_column)
                .order_by(order_metric.desc())
                .limit(limit)
                .all()
            )
        elif dimension == "user":
            dimension_key_column = case(
                (Usage.user_id.isnot(None), Usage.user_id),
                else_=unknown_user_group,
            )
            rows = (
                query.with_entities(
                    dimension_key_column.label("dimension_key"),
                    *cls._summary_columns(),
                )
                .group_by(dimension_key_column)
                .order_by(order_metric.desc())
                .limit(limit)
                .all()
            )
        else:
            dimension_column = {
                "model": Usage.model,
                "provider": Usage.provider_name,
                "api_format": Usage.api_format,
            }[dimension]

            rows = (
                query.with_entities(
                    dimension_column.label("dimension_key"),
                    *cls._summary_columns(),
                )
                .group_by(dimension_column)
                .order_by(order_metric.desc())
                .limit(limit)
                .all()
            )

        total_summary = cls.overview(
            db,
            current_user,
            time_range=time_range,
            scope_kind=scope_kind,
            scope_user_id=scope_user_id,
            scope_api_key_id=scope_api_key_id,
            filters=filters,
        )["summary"]

        model_display_names = (
            cls._resolve_model_display_names(
                db,
                [str(getattr(row, "dimension_key", "") or "").strip() for row in rows],
            )
            if dimension == "model"
            else {}
        )
        user_display_names = (
            {
                option["value"]: option["label"]
                for option in cls._resolve_current_user_options(
                    db,
                    [
                        str(getattr(row, "dimension_key"))
                        for row in rows
                        if getattr(row, "dimension_key", None) not in {None, unknown_user_group}
                    ],
                )
            }
            if dimension == "user"
            else {}
        )
        api_key_display_names = (
            {
                option["value"]: option["label"]
                for option in cls._resolve_current_api_key_options(
                    db,
                    [
                        str(getattr(row, "dimension_key"))
                        for row in rows
                        if getattr(row, "dimension_key", None) not in {None, unknown_api_key_group}
                    ],
                )
            }
            if dimension == "api_key"
            else {}
        )
        total_metric_value = _to_float(total_summary.get(metric, 0))

        items = []
        for row in rows:
            item = cls._serialize_summary_row(row)
            raw_key = getattr(row, "dimension_key", None)
            if dimension == "api_key":
                normalized_key = None if raw_key == unknown_api_key_group else raw_key
                fallback_label = DELETED_API_KEY_LABEL
                resolved_label = (
                    api_key_display_names.get(str(normalized_key))
                    if normalized_key is not None
                    else None
                )
            elif dimension == "user":
                normalized_key = None if raw_key == unknown_user_group else raw_key
                fallback_label = DELETED_USER_LABEL
                resolved_label = (
                    user_display_names.get(str(normalized_key))
                    if normalized_key is not None
                    else None
                )
            elif dimension == "model":
                normalized_key = raw_key
                fallback_label = "未知"
                resolved_label = model_display_names.get(str(normalized_key or "").strip())
            elif dimension == "provider":
                normalized_key = raw_key
                fallback_label = UNKNOWN_PROVIDER_LABEL
                resolved_label = _provider_display_label(normalized_key)
            else:
                normalized_key = raw_key
                fallback_label = "未知"
                resolved_label = normalized_key

            item["key"] = str(normalized_key or fallback_label)
            item["label"] = str(resolved_label or normalized_key or fallback_label)
            item["share_of_total_cost"] = _round2(
                item["total_cost_usd"] / total_summary["total_cost_usd"] * 100
            ) if total_summary["total_cost_usd"] > 0 else 0.0
            item["share_of_total_tokens"] = _round2(
                item["total_tokens"] / total_summary["total_tokens"] * 100
            ) if total_summary["total_tokens"] > 0 else 0.0
            item["share_of_selected_metric"] = _round2(
                item[metric] / total_metric_value * 100
            ) if total_metric_value > 0 else 0.0
            items.append(item)

        return {"dimension": dimension, "metric": metric, "rows": items}

    @classmethod
    def records(
        cls,
        db: Session,
        current_user: User,
        *,
        time_range: TimeRangeParams,
        scope_kind: ScopeKind,
        scope_user_id: str | None,
        scope_api_key_id: str | None,
        filters: AnalyticsFilters,
        search: AnalyticsSearch,
        limit: int,
        offset: int,
    ) -> dict[str, Any]:
        query = cls.build_usage_query(
            db,
            current_user,
            time_range=time_range,
            scope_kind=scope_kind,
            scope_user_id=scope_user_id,
            scope_api_key_id=scope_api_key_id,
            filters=filters,
            include_non_terminal=True,
            include_pending_providers=True,
        )

        if search.request_id:
            query = query.filter(Usage.request_id == search.request_id)

        if search.text:
            escaped = safe_truncate_escaped(escape_like_pattern(search.text.strip()), 128)
            pattern = f"%{escaped}%"
            search_clauses = [
                Usage.username.ilike(pattern, escape="\\"),
                Usage.api_key_name.ilike(pattern, escape="\\"),
                Usage.model.ilike(pattern, escape="\\"),
                Usage.target_model.ilike(pattern, escape="\\"),
                Usage.request_id.ilike(pattern, escape="\\"),
                Usage.error_message.ilike(pattern, escape="\\"),
            ]
            if current_user.role == UserRole.ADMIN:
                search_clauses.append(Usage.provider_name.ilike(pattern, escape="\\"))
            query = query.filter(or_(*search_clauses))

        total = int(query.with_entities(func.count(Usage.id)).scalar() or 0)
        rows = (
            query.order_by(Usage.created_at.desc())
            .offset(offset)
            .limit(limit)
            .all()
        )
        request_ids = [
            str(usage.request_id)
            for usage in rows
            if getattr(usage, "request_id", None)
        ]
        fallback_map, retry_map = cls._load_request_execution_flags(db, request_ids)
        user_label_map = {
            option["value"]: option["label"]
            for option in cls._resolve_current_user_options(
                db,
                [
                    str(usage.user_id)
                    for usage in rows
                    if getattr(usage, "user_id", None)
                ],
            )
        }
        api_key_label_map = {
            option["value"]: option["label"]
            for option in cls._resolve_current_api_key_options(
                db,
                [
                    str(usage.api_key_id)
                    for usage in rows
                    if getattr(usage, "api_key_id", None)
                ],
            )
        }
        provider_api_key_label_map = (
            {
                option["value"]: option["label"]
                for option in cls._resolve_current_provider_api_key_options(
                    db,
                    [
                        str(usage.provider_api_key_id)
                        for usage in rows
                        if getattr(usage, "provider_api_key_id", None)
                    ],
                )
            }
            if current_user.role == UserRole.ADMIN
            else {}
        )

        items = []
        for usage in rows:
            resolved_username = user_label_map.get(str(usage.user_id)) if usage.user_id else None
            resolved_api_key_name = (
                api_key_label_map.get(str(usage.api_key_id))
                if usage.api_key_id
                else None
            )
            resolved_provider_api_key_name = (
                provider_api_key_label_map.get(str(usage.provider_api_key_id))
                if current_user.role == UserRole.ADMIN
                and getattr(usage, "provider_api_key_id", None)
                else None
            )
            items.append(
                {
                    "id": usage.id,
                    "request_id": usage.request_id,
                    "created_at": usage.created_at.isoformat() if usage.created_at else None,
                    "user_id": usage.user_id,
                    "username": resolved_username
                    or usage.username
                    or (DELETED_USER_LABEL if usage.user_id is None else None),
                    "api_key_id": usage.api_key_id,
                    "api_key_name": resolved_api_key_name
                    or usage.api_key_name
                    or (DELETED_API_KEY_LABEL if usage.api_key_id is None else None),
                    "provider_api_key_name": (
                        resolved_provider_api_key_name if current_user.role == UserRole.ADMIN else None
                    ),
                    "provider_name": usage.provider_name if current_user.role == UserRole.ADMIN else None,
                    "model": usage.model,
                    "target_model": usage.target_model,
                    "api_format": usage.api_format,
                    "request_type": usage.request_type,
                    "status": usage.status,
                    "billing_status": usage.billing_status,
                    "is_stream": usage.is_stream,
                    "has_format_conversion": usage.has_format_conversion,
                    "has_fallback": fallback_map.get(str(usage.request_id), False),
                    "has_retry": retry_map.get(str(usage.request_id), False),
                    "status_code": usage.status_code,
                    "error_message": usage.error_message,
                    "error_category": usage.error_category,
                    "response_time_ms": usage.response_time_ms,
                    "first_byte_time_ms": usage.first_byte_time_ms,
                    "input_tokens": _to_int(usage.input_tokens),
                    "output_tokens": _to_int(usage.output_tokens),
                    "input_output_total_tokens": _to_int(usage.input_output_total_tokens),
                    "cache_creation_input_tokens": _to_int(usage.cache_creation_input_tokens),
                    "cache_creation_input_tokens_5m": _to_int(usage.cache_creation_input_tokens_5m),
                    "cache_creation_input_tokens_1h": _to_int(usage.cache_creation_input_tokens_1h),
                    "cache_read_input_tokens": _to_int(usage.cache_read_input_tokens),
                    "input_context_tokens": _to_int(usage.input_context_tokens),
                    "total_tokens": _to_int(usage.total_tokens),
                    "input_cost_usd": _round2(_to_float(usage.input_cost_usd)),
                    "output_cost_usd": _round2(_to_float(usage.output_cost_usd)),
                    "cache_creation_cost_usd": _round2(_to_float(usage.cache_creation_cost_usd)),
                    "cache_creation_cost_usd_5m": _round2(_to_float(usage.cache_creation_cost_usd_5m)),
                    "cache_creation_cost_usd_1h": _round2(_to_float(usage.cache_creation_cost_usd_1h)),
                    "cache_read_cost_usd": _round2(_to_float(usage.cache_read_cost_usd)),
                    "cache_cost_usd": _round2(_to_float(usage.cache_cost_usd)),
                    "request_cost_usd": _round2(_to_float(usage.request_cost_usd)),
                    "total_cost_usd": _round2(_to_float(usage.total_cost_usd)),
                    "actual_total_cost_usd": _round2(_to_float(usage.actual_total_cost_usd)),
                    "actual_cache_cost_usd": _round2(_to_float(usage.actual_cache_cost_usd)),
                    "rate_multiplier": _to_float(usage.rate_multiplier),
                }
            )

        return {
            "total": total,
            "limit": limit,
            "offset": offset,
            "records": items,
        }

    @staticmethod
    def _serialize_option_rows(rows: list[tuple[Any, Any]]) -> list[dict[str, str]]:
        return [
            {"value": str(value), "label": str(label or value)}
            for value, label in rows
            if value not in ("", "None")
        ]

    @classmethod
    def _resolve_current_user_options(
        cls,
        db: Session,
        user_ids: list[str],
    ) -> list[dict[str, str]]:
        if not user_ids:
            return []
        rows = (
            db.query(User.id, User.username)
            .filter(User.id.in_(user_ids))
            .order_by(func.coalesce(User.username, User.id).asc())
            .all()
        )
        return cls._serialize_option_rows(rows)

    @classmethod
    def _resolve_current_api_key_options(
        cls,
        db: Session,
        api_key_ids: list[str],
    ) -> list[dict[str, str]]:
        if not api_key_ids:
            return []
        rows = (
            db.query(ApiKey.id, ApiKey.name)
            .filter(ApiKey.id.in_(api_key_ids))
            .order_by(func.coalesce(ApiKey.name, ApiKey.id).asc())
            .all()
        )
        return cls._serialize_option_rows(rows)

    @classmethod
    def _resolve_current_provider_api_key_options(
        cls,
        db: Session,
        api_key_ids: list[str],
    ) -> list[dict[str, str]]:
        if not api_key_ids:
            return []
        rows = (
            db.query(ProviderAPIKey.id, ProviderAPIKey.name)
            .filter(ProviderAPIKey.id.in_(api_key_ids))
            .order_by(func.coalesce(ProviderAPIKey.name, ProviderAPIKey.id).asc())
            .all()
        )
        return cls._serialize_option_rows(rows)

    @classmethod
    def filter_options(
        cls,
        db: Session,
        current_user: User,
        *,
        time_range: TimeRangeParams,
        scope_kind: ScopeKind,
        scope_user_id: str | None,
        scope_api_key_id: str | None,
        filters: AnalyticsFilters,
    ) -> dict[str, Any]:
        def _build_option_query(option_filters: AnalyticsFilters) -> Query[Any]:
            return cls.build_usage_query(
                db,
                current_user,
                time_range=time_range,
                scope_kind=scope_kind,
                scope_user_id=scope_user_id,
                scope_api_key_id=scope_api_key_id,
                filters=option_filters,
                include_non_terminal=True,
                include_pending_providers=True,
            )

        def _pairs(
            query: Query[Any],
            value_column: Any,
            label_column: Any | None = None,
            *,
            label_formatter: Any | None = None,
        ) -> list[dict[str, str]]:
            rows = (
                query.with_entities(value_column, label_column or value_column)
                .filter(value_column.isnot(None))
                .distinct()
                .order_by((label_column or value_column).asc())
                .all()
            )
            return [
                {
                    "value": str(value),
                    "label": (
                        label_formatter(str(label or value))
                        if label_formatter
                        else str(label or value)
                    ),
                }
                for value, label in rows
                if value not in ("", "None")
            ]

        def _distinct_values(query: Query[Any], value_column: Any) -> list[str]:
            rows = (
                query.with_entities(value_column)
                .filter(value_column.isnot(None))
                .distinct()
                .all()
            )
            return [
                str(value)
                for (value,) in rows
                if value not in ("", "None")
            ]

        def _user_pairs(query: Query[Any]) -> list[dict[str, str]]:
            user_ids = _distinct_values(query, Usage.user_id)
            return cls._resolve_current_user_options(db, user_ids)

        def _api_key_pairs(query: Query[Any]) -> list[dict[str, str]]:
            api_key_ids = _distinct_values(query, Usage.api_key_id)
            return cls._resolve_current_api_key_options(db, api_key_ids)

        providers_query = _build_option_query(replace(filters, provider_names=[]))
        models_query = _build_option_query(replace(filters, models=[]))
        target_models_query = _build_option_query(replace(filters, target_models=[]))
        api_formats_query = _build_option_query(replace(filters, api_formats=[]))
        request_types_query = _build_option_query(replace(filters, request_types=[]))
        error_categories_query = _build_option_query(replace(filters, error_categories=[]))
        statuses_query = _build_option_query(replace(filters, statuses=[]))

        response: dict[str, Any] = {
            "providers": (
                _pairs(
                    providers_query,
                    Usage.provider_name,
                    label_formatter=_provider_display_label,
                )
                if current_user.role == UserRole.ADMIN
                else []
            ),
            "models": _pairs(models_query, Usage.model),
            "target_models": _pairs(target_models_query, Usage.target_model),
            "api_formats": _pairs(api_formats_query, Usage.api_format),
            "request_types": _pairs(request_types_query, Usage.request_type),
            "error_categories": _pairs(error_categories_query, Usage.error_category),
            "statuses": [],
        }

        raw_status_rows = (
            statuses_query.with_entities(Usage.status)
            .filter(Usage.status.isnot(None))
            .distinct()
            .all()
        )
        raw_statuses = {
            str(status or "").strip()
            for (status,) in raw_status_rows
            if str(status or "").strip()
        }

        def _has_status_option(status_value: str) -> bool:
            clause = cls._status_clause(db, status_value)
            if clause is None:
                return False
            return (
                statuses_query.with_entities(Usage.id)
                .filter(clause)
                .first()
                is not None
            )

        response["statuses"] = cls._compose_status_options(
            raw_statuses=raw_statuses,
            has_active=_has_status_option("active"),
            has_stream=_has_status_option("stream"),
            has_standard=_has_status_option("standard"),
            has_retry=_has_status_option("has_retry"),
            has_fallback=_has_status_option("has_fallback"),
        )

        if current_user.role == UserRole.ADMIN:
            users_query = _build_option_query(replace(filters, user_ids=[]))
            response["users"] = _user_pairs(users_query)
            has_deleted_user = (
                users_query.with_entities(Usage.id)
                .filter(Usage.user_id.is_(None))
                .first()
                is not None
            )
            if has_deleted_user:
                response["users"].append(
                    {"value": DELETED_USER_FILTER, "label": DELETED_USER_LABEL}
                )

        api_keys_query = _build_option_query(replace(filters, api_key_ids=[]))
        response["api_keys"] = _api_key_pairs(api_keys_query)

        has_deleted_api_key = (
            api_keys_query.with_entities(Usage.id)
            .filter(Usage.api_key_id.is_(None))
            .first()
            is not None
        )
        if has_deleted_api_key:
            response["api_keys"].append(
                {"value": DELETED_API_KEY_FILTER, "label": DELETED_API_KEY_LABEL}
            )

        return response

    @classmethod
    def leaderboard(
        cls,
        db: Session,
        current_user: User,
        *,
        time_range: TimeRangeParams,
        scope_kind: ScopeKind,
        scope_user_id: str | None,
        scope_api_key_id: str | None,
        filters: AnalyticsFilters,
        entity: LeaderboardEntity,
        metric: LeaderboardMetric,
        limit: int,
    ) -> dict[str, Any]:
        query = cls.build_usage_query(
            db,
            current_user,
            time_range=time_range,
            scope_kind=scope_kind,
            scope_user_id=scope_user_id,
            scope_api_key_id=scope_api_key_id,
            filters=filters,
            include_non_terminal=False,
            include_pending_providers=False,
        )

        if entity == "user":
            key_column = Usage.user_id
            label_column = Usage.username
        else:
            key_column = Usage.api_key_id
            label_column = Usage.api_key_name

        rows = (
            query.with_entities(
                key_column.label("entity_id"),
                func.max(label_column).label("entity_label"),
                *cls._summary_columns(),
            )
            .filter(key_column.isnot(None))
            .group_by(key_column)
            .all()
        )

        items = []
        for row in rows:
            item = cls._serialize_summary_row(row)
            item["id"] = row.entity_id
            item["label"] = row.entity_label or row.entity_id
            items.append(item)

        items.sort(key=lambda item: item[metric], reverse=True)
        ranked = []
        for index, item in enumerate(items[:limit], start=1):
            ranked.append(
                {
                    "rank": index,
                    "id": item["id"],
                    "label": item["label"],
                    "requests_total": item["requests_total"],
                    "total_tokens": item["total_tokens"],
                    "total_cost_usd": item["total_cost_usd"],
                    "actual_total_cost_usd": item["actual_total_cost_usd"],
                    "metric_value": item[metric],
                }
            )

        return {
            "entity": entity,
            "metric": metric,
            "items": ranked,
        }

    @classmethod
    def performance(
        cls,
        db: Session,
        current_user: User,
        *,
        time_range: TimeRangeParams,
        scope_kind: ScopeKind,
        scope_user_id: str | None,
        scope_api_key_id: str | None,
        filters: AnalyticsFilters,
    ) -> dict[str, Any]:
        query = cls.build_usage_query(
            db,
            current_user,
            time_range=time_range,
            scope_kind=scope_kind,
            scope_user_id=scope_user_id,
            scope_api_key_id=scope_api_key_id,
            filters=filters,
            include_non_terminal=False,
            include_pending_providers=False,
        )

        rows = query.with_entities(
            Usage.created_at,
            Usage.provider_name,
            Usage.error_category,
            Usage.status,
            Usage.response_time_ms,
            Usage.first_byte_time_ms,
        ).all()

        response_values: list[float] = []
        ttfb_values: list[float] = []
        distribution: dict[str, int] = defaultdict(int)
        trend: dict[str, int] = defaultdict(int)
        provider_metrics: dict[str, dict[str, float]] = {}
        percentiles_by_bucket: dict[str, dict[str, list[float]]] = defaultdict(
            lambda: {"response": [], "ttfb": []}
        )

        for row in rows:
            if row.response_time_ms is not None:
                value = _to_float(row.response_time_ms)
                response_values.append(value)
            if row.first_byte_time_ms is not None:
                ttfb_value = _to_float(row.first_byte_time_ms)
                ttfb_values.append(ttfb_value)

            bucket_local = _bucket_start(row.created_at, time_range)
            bucket_key = bucket_local.date().isoformat()
            if row.response_time_ms is not None:
                percentiles_by_bucket[bucket_key]["response"].append(_to_float(row.response_time_ms))
            if row.first_byte_time_ms is not None:
                percentiles_by_bucket[bucket_key]["ttfb"].append(_to_float(row.first_byte_time_ms))

            if row.status == "failed":
                category = row.error_category or "unknown"
                distribution[category] += 1
                trend[bucket_key] += 1

            provider = row.provider_name or "unknown"
            metrics = provider_metrics.setdefault(
                provider,
                {
                    "requests_total": 0,
                    "requests_success": 0,
                    "requests_error": 0,
                    "response_sum": 0.0,
                    "response_count": 0,
                    "ttfb_sum": 0.0,
                    "ttfb_count": 0,
                },
            )
            metrics["requests_total"] += 1
            if row.status == "completed":
                metrics["requests_success"] += 1
            if row.status == "failed":
                metrics["requests_error"] += 1
            if row.response_time_ms is not None:
                metrics["response_sum"] += _to_float(row.response_time_ms)
                metrics["response_count"] += 1
            if row.first_byte_time_ms is not None:
                metrics["ttfb_sum"] += _to_float(row.first_byte_time_ms)
                metrics["ttfb_count"] += 1

        percentile_items = []
        for date_key in sorted(percentiles_by_bucket.keys()):
            bucket = percentiles_by_bucket[date_key]
            percentile_items.append(
                {
                    "date": date_key,
                    "p50_response_time_ms": _percentile(bucket["response"], 0.50),
                    "p90_response_time_ms": _percentile(bucket["response"], 0.90),
                    "p99_response_time_ms": _percentile(bucket["response"], 0.99),
                    "p50_first_byte_time_ms": _percentile(bucket["ttfb"], 0.50),
                    "p90_first_byte_time_ms": _percentile(bucket["ttfb"], 0.90),
                    "p99_first_byte_time_ms": _percentile(bucket["ttfb"], 0.99),
                }
            )

        distribution_items = [
            {
                "category": category,
                "label": _error_category_display_label(category),
                "count": count,
            }
            for category, count in sorted(distribution.items(), key=lambda item: item[1], reverse=True)
        ]
        trend_items = [
            {"date": day, "total": count}
            for day, count in sorted(trend.items(), key=lambda item: item[0])
        ]
        provider_health = []
        if current_user.role == UserRole.ADMIN:
            for provider_name, metrics in sorted(
                provider_metrics.items(),
                key=lambda item: item[1]["requests_total"],
                reverse=True,
            ):
                requests_total = int(metrics["requests_total"])
                requests_success = int(metrics["requests_success"])
                requests_error = int(metrics["requests_error"])
                success_rate = (
                    _round2(requests_success / requests_total * 100)
                    if requests_total > 0
                    else 0.0
                )
                provider_health.append(
                    {
                        "provider_name": provider_name,
                        "requests_total": requests_total,
                        "success_rate": success_rate,
                        "error_rate": (
                            _round2(requests_error / requests_total * 100)
                            if requests_total > 0
                            else 0.0
                        ),
                        "avg_response_time_ms": (
                            _round2(metrics["response_sum"] / metrics["response_count"])
                            if metrics["response_count"] > 0
                            else 0.0
                        ),
                        "avg_first_byte_time_ms": (
                            _round2(metrics["ttfb_sum"] / metrics["ttfb_count"])
                            if metrics["ttfb_count"] > 0
                            else 0.0
                        ),
                    }
                )

        return {
            "latency": {
                "response_time_ms": {
                    "avg": _round2(sum(response_values) / len(response_values)) if response_values else 0.0,
                    "p50": _percentile(response_values, 0.50),
                    "p90": _percentile(response_values, 0.90),
                    "p99": _percentile(response_values, 0.99),
                },
                "first_byte_time_ms": {
                    "avg": _round2(sum(ttfb_values) / len(ttfb_values)) if ttfb_values else 0.0,
                    "p50": _percentile(ttfb_values, 0.50),
                    "p90": _percentile(ttfb_values, 0.90),
                    "p99": _percentile(ttfb_values, 0.99),
                },
            },
            "percentiles": percentile_items,
            "errors": {
                "total": sum(distribution.values()),
                "rate": _round2(sum(distribution.values()) / len(rows) * 100) if rows else 0.0,
                "categories": distribution_items,
                "trend": trend_items,
            },
            "provider_health": provider_health,
        }

    @staticmethod
    def _serialize_summary_row(row: Any) -> dict[str, Any]:
        summary = {
            "requests_total": _to_int(getattr(row, "requests_total", 0)),
            "requests_success": _to_int(getattr(row, "requests_success", 0)),
            "requests_error": _to_int(getattr(row, "requests_error", 0)),
            "requests_stream": _to_int(getattr(row, "requests_stream", 0)),
            "input_tokens": _to_int(getattr(row, "input_tokens", 0)),
            "output_tokens": _to_int(getattr(row, "output_tokens", 0)),
            "input_output_total_tokens": _to_int(getattr(row, "input_output_total_tokens", 0)),
            "cache_creation_input_tokens": _to_int(getattr(row, "cache_creation_input_tokens", 0)),
            "cache_creation_input_tokens_5m": _to_int(getattr(row, "cache_creation_input_tokens_5m", 0)),
            "cache_creation_input_tokens_1h": _to_int(getattr(row, "cache_creation_input_tokens_1h", 0)),
            "cache_read_input_tokens": _to_int(getattr(row, "cache_read_input_tokens", 0)),
            "input_context_tokens": _to_int(getattr(row, "input_context_tokens", 0)),
            "total_tokens": _to_int(getattr(row, "total_tokens", 0)),
            "input_cost_usd": _round2(_to_float(getattr(row, "input_cost_usd", 0))),
            "output_cost_usd": _round2(_to_float(getattr(row, "output_cost_usd", 0))),
            "cache_creation_cost_usd": _round2(_to_float(getattr(row, "cache_creation_cost_usd", 0))),
            "cache_creation_cost_usd_5m": _round2(_to_float(getattr(row, "cache_creation_cost_usd_5m", 0))),
            "cache_creation_cost_usd_1h": _round2(_to_float(getattr(row, "cache_creation_cost_usd_1h", 0))),
            "cache_read_cost_usd": _round2(_to_float(getattr(row, "cache_read_cost_usd", 0))),
            "cache_cost_usd": _round2(_to_float(getattr(row, "cache_cost_usd", 0))),
            "request_cost_usd": _round2(_to_float(getattr(row, "request_cost_usd", 0))),
            "total_cost_usd": _round2(_to_float(getattr(row, "total_cost_usd", 0))),
            "actual_total_cost_usd": _round2(_to_float(getattr(row, "actual_total_cost_usd", 0))),
            "actual_cache_cost_usd": _round2(_to_float(getattr(row, "actual_cache_cost_usd", 0))),
            "avg_response_time_ms": _round2(_to_float(getattr(row, "avg_response_time_ms", 0))),
            "avg_first_byte_time_ms": _round2(_to_float(getattr(row, "avg_first_byte_time_ms", 0))),
            "format_conversion_count": _to_int(getattr(row, "format_conversion_count", 0)),
            "models_used_count": _to_int(getattr(row, "models_used_count", 0)),
        }
        summary["success_rate"] = _round2(
            summary["requests_success"] / summary["requests_total"] * 100
        ) if summary["requests_total"] > 0 else 0.0
        summary["cache_hit_rate"] = _cache_hit_rate(
            summary["input_context_tokens"],
            summary["cache_read_input_tokens"],
        )
        return summary

    @staticmethod
    def _composition_segments(summary: dict[str, Any], *, token_mode: bool) -> list[dict[str, Any]]:
        if token_mode:
            segments = [
                ("input", summary["input_tokens"]),
                ("output", summary["output_tokens"]),
                ("cache_creation", summary["cache_creation_input_tokens"]),
                ("cache_read", summary["cache_read_input_tokens"]),
            ]
            total = summary["total_tokens"]
        else:
            segments = [
                ("input", summary["input_cost_usd"]),
                ("output", summary["output_cost_usd"]),
                ("cache_creation", summary["cache_creation_cost_usd"]),
                ("cache_read", summary["cache_read_cost_usd"]),
            ]
            total = summary["total_cost_usd"]

        result = []
        for key, value in segments:
            percentage = _round2(value / total * 100) if total > 0 else 0.0
            result.append({"key": key, "value": value, "percentage": percentage})
        return result
