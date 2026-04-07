from __future__ import annotations

from datetime import date
from typing import Any, Literal

from fastapi import APIRouter, Depends, HTTPException
from pydantic import BaseModel, Field
from sqlalchemy.orm import Session

from src.core.enums import UserRole
from src.database import get_db
from src.models.database import User
from src.services.analytics import AnalyticsQueryService
from src.services.analytics.query_service import (
    DELETED_API_KEY_FILTER,
    DELETED_USER_FILTER,
    AnalyticsFilters,
    AnalyticsSearch,
    BreakdownMetric,
    LeaderboardEntity,
    LeaderboardMetric,
)
from src.services.system.time_range import TimeRangeParams
from src.services.usage.service import UsageService
from src.utils.auth_utils import get_current_user

router = APIRouter(prefix="/api/analytics", tags=["Analytics"])


class AnalyticsScopePayload(BaseModel):
    kind: Literal["global", "me", "user", "api_key"] = "me"
    user_id: str | None = None
    api_key_id: str | None = None


class AnalyticsTimeRangePayload(BaseModel):
    start_date: date | None = None
    end_date: date | None = None
    preset: (
        Literal[
            "today",
            "last7days",
            "last30days",
            "last180days",
            "last1year",
        ]
        | None
    ) = None
    granularity: Literal["hour", "day", "week", "month"] = "day"
    timezone: str | None = "UTC"
    tz_offset_minutes: int = 0


class AnalyticsFiltersPayload(BaseModel):
    user_ids: list[str] = Field(default_factory=list)
    provider_names: list[str] = Field(default_factory=list)
    models: list[str] = Field(default_factory=list)
    target_models: list[str] = Field(default_factory=list)
    api_key_ids: list[str] = Field(default_factory=list)
    api_formats: list[str] = Field(default_factory=list)
    request_types: list[str] = Field(default_factory=list)
    statuses: list[str] = Field(default_factory=list)
    error_categories: list[str] = Field(default_factory=list)
    is_stream: bool | None = None
    has_format_conversion: bool | None = None


class AnalyticsBaseRequest(BaseModel):
    scope: AnalyticsScopePayload = Field(default_factory=AnalyticsScopePayload)
    time_range: AnalyticsTimeRangePayload = Field(
        default_factory=lambda: AnalyticsTimeRangePayload(preset="today")
    )
    filters: AnalyticsFiltersPayload = Field(default_factory=AnalyticsFiltersPayload)


class AnalyticsBreakdownRequest(AnalyticsBaseRequest):
    dimension: Literal["model", "provider", "api_format", "api_key", "user"] = "model"
    metric: BreakdownMetric = "total_cost_usd"
    limit: int = Field(default=50, ge=1, le=200)


class AnalyticsSearchPayload(BaseModel):
    text: str | None = None
    request_id: str | None = None


class AnalyticsPaginationPayload(BaseModel):
    limit: int = Field(default=100, ge=1, le=5000)
    offset: int = Field(default=0, ge=0)


class AnalyticsRecordsRequest(AnalyticsBaseRequest):
    search: AnalyticsSearchPayload = Field(default_factory=AnalyticsSearchPayload)
    pagination: AnalyticsPaginationPayload = Field(default_factory=AnalyticsPaginationPayload)


class AnalyticsLeaderboardRequest(AnalyticsBaseRequest):
    entity: LeaderboardEntity = "user"
    metric: LeaderboardMetric = "total_cost_usd"
    limit: int = Field(default=20, ge=1, le=100)


class AnalyticsHeatmapRequest(BaseModel):
    scope: AnalyticsScopePayload = Field(default_factory=AnalyticsScopePayload)
    user_id: str | None = None
    api_key_id: str | None = None


class AnalyticsActiveRequestsRequest(BaseModel):
    scope: AnalyticsScopePayload = Field(default_factory=AnalyticsScopePayload)
    ids: list[str] = Field(default_factory=list)


class AnalyticsIntervalTimelineRequest(BaseModel):
    scope: AnalyticsScopePayload = Field(default_factory=AnalyticsScopePayload)
    user_id: str | None = None
    hours: int = Field(default=24, ge=1, le=720)
    limit: int = Field(default=2000, ge=100, le=50000)
    include_user_info: bool = False


class AnalyticsCacheAffinityRequest(BaseModel):
    scope: AnalyticsScopePayload = Field(default_factory=AnalyticsScopePayload)
    user_id: str | None = None
    api_key_id: str | None = None
    hours: int = Field(default=168, ge=1, le=720)


def _build_time_range(payload: AnalyticsTimeRangePayload) -> TimeRangeParams:
    try:
        return TimeRangeParams(
            start_date=payload.start_date,
            end_date=payload.end_date,
            preset=payload.preset,
            granularity=payload.granularity,
            timezone=payload.timezone,
            tz_offset_minutes=payload.tz_offset_minutes,
        ).validate_and_resolve()
    except Exception as exc:
        raise HTTPException(status_code=400, detail=str(exc)) from exc


def _build_filters(payload: AnalyticsFiltersPayload) -> AnalyticsFilters:
    return AnalyticsFilters(
        user_ids=payload.user_ids,
        provider_names=payload.provider_names,
        models=payload.models,
        target_models=payload.target_models,
        api_key_ids=payload.api_key_ids,
        api_formats=payload.api_formats,
        request_types=payload.request_types,
        statuses=payload.statuses,
        error_categories=payload.error_categories,
        is_stream=payload.is_stream,
        has_format_conversion=payload.has_format_conversion,
    )


def _build_search(payload: AnalyticsSearchPayload) -> AnalyticsSearch:
    return AnalyticsSearch(
        text=payload.text.strip() if payload.text else None,
        request_id=payload.request_id.strip() if payload.request_id else None,
    )


def _validate_scope(current_user: User, scope: AnalyticsScopePayload) -> None:
    if current_user.role != UserRole.ADMIN and scope.kind != "me":
        raise HTTPException(status_code=403, detail="Only admin can access non-personal analytics scope")

    if scope.kind == "user" and not scope.user_id:
        raise HTTPException(status_code=400, detail="scope.user_id is required for user scope")

    if scope.kind == "api_key" and not scope.api_key_id:
        raise HTTPException(status_code=400, detail="scope.api_key_id is required for api_key scope")


def _redact_sensitive_cost_fields(payload: object) -> object:
    if isinstance(payload, list):
        return [_redact_sensitive_cost_fields(item) for item in payload]

    if isinstance(payload, dict):
        redacted = {
            key: _redact_sensitive_cost_fields(value)
            for key, value in payload.items()
        }
        if "providers" in redacted:
            redacted["providers"] = []
        if "provider_health" in redacted:
            redacted["provider_health"] = []
        if "provider_name" in redacted:
            redacted["provider_name"] = None
        if "provider_api_key_name" in redacted:
            redacted["provider_api_key_name"] = None
        if "rate_multiplier" in redacted:
            redacted["rate_multiplier"] = 1.0
        for key in list(redacted.keys()):
            if key.startswith("actual_") and key.endswith("_usd"):
                redacted[key] = 0.0
        return redacted

    return payload


def _ensure_non_admin_breakdown_allowed(
    current_user: User,
    body: AnalyticsBreakdownRequest,
) -> None:
    if current_user.role == UserRole.ADMIN:
        return
    if body.dimension == "provider":
        raise HTTPException(status_code=403, detail="Only admin can access provider breakdown")
    if body.metric == "actual_total_cost_usd":
        raise HTTPException(
            status_code=403,
            detail="Only admin can access actual cost breakdown",
        )


def _resolve_scope_targets(
    current_user: User,
    scope: AnalyticsScopePayload,
    *,
    user_id: str | None = None,
    api_key_id: str | None = None,
) -> tuple[str | None, str | None]:
    _validate_scope(current_user, scope)

    resolved_user_id = user_id
    resolved_api_key_id = api_key_id

    if scope.kind == "me":
        current_user_id = str(current_user.id)
        if resolved_user_id not in {None, current_user_id}:
            raise HTTPException(status_code=400, detail="scope.kind=me cannot target another user")
        resolved_user_id = current_user_id
    elif scope.kind == "user":
        if resolved_user_id not in {None, scope.user_id}:
            raise HTTPException(status_code=400, detail="scope.user_id conflicts with user_id")
        resolved_user_id = scope.user_id
    elif scope.kind == "api_key":
        if resolved_api_key_id not in {None, scope.api_key_id}:
            raise HTTPException(status_code=400, detail="scope.api_key_id conflicts with api_key_id")
        resolved_api_key_id = scope.api_key_id

    return resolved_user_id, resolved_api_key_id


def _normalize_optional_ids(ids: list[str]) -> list[str]:
    normalized = [item.strip() for item in ids if item and item.strip()]
    return list(dict.fromkeys(normalized))


@router.post("/overview")
async def analytics_overview(
    body: AnalyticsBaseRequest,
    current_user: User = Depends(get_current_user),
    db: Session = Depends(get_db),
) -> dict:
    _validate_scope(current_user, body.scope)
    time_range = _build_time_range(body.time_range)
    result = AnalyticsQueryService.overview(
        db,
        current_user,
        time_range=time_range,
        scope_kind=body.scope.kind,
        scope_user_id=body.scope.user_id,
        scope_api_key_id=body.scope.api_key_id,
        filters=_build_filters(body.filters),
    )
    result = {
        "query_context": {
            "scope": body.scope.model_dump(),
            "time_range": body.time_range.model_dump(),
        },
        **result,
    }
    return (
        result
        if current_user.role == UserRole.ADMIN
        else _redact_sensitive_cost_fields(result)
    )


@router.post("/timeseries")
async def analytics_timeseries(
    body: AnalyticsBaseRequest,
    current_user: User = Depends(get_current_user),
    db: Session = Depends(get_db),
) -> dict:
    _validate_scope(current_user, body.scope)
    time_range = _build_time_range(body.time_range)
    result = AnalyticsQueryService.timeseries(
        db,
        current_user,
        time_range=time_range,
        scope_kind=body.scope.kind,
        scope_user_id=body.scope.user_id,
        scope_api_key_id=body.scope.api_key_id,
        filters=_build_filters(body.filters),
    )
    return (
        result
        if current_user.role == UserRole.ADMIN
        else _redact_sensitive_cost_fields(result)
    )


@router.post("/breakdown")
async def analytics_breakdown(
    body: AnalyticsBreakdownRequest,
    current_user: User = Depends(get_current_user),
    db: Session = Depends(get_db),
) -> dict:
    _validate_scope(current_user, body.scope)
    _ensure_non_admin_breakdown_allowed(current_user, body)
    time_range = _build_time_range(body.time_range)
    result = AnalyticsQueryService.breakdown(
        db,
        current_user,
        time_range=time_range,
        scope_kind=body.scope.kind,
        scope_user_id=body.scope.user_id,
        scope_api_key_id=body.scope.api_key_id,
        filters=_build_filters(body.filters),
        dimension=body.dimension,
        metric=body.metric,
        limit=body.limit,
    )
    return (
        result
        if current_user.role == UserRole.ADMIN
        else _redact_sensitive_cost_fields(result)
    )


@router.post("/records")
async def analytics_records(
    body: AnalyticsRecordsRequest,
    current_user: User = Depends(get_current_user),
    db: Session = Depends(get_db),
) -> dict:
    _validate_scope(current_user, body.scope)
    time_range = _build_time_range(body.time_range)
    result = AnalyticsQueryService.records(
        db,
        current_user,
        time_range=time_range,
        scope_kind=body.scope.kind,
        scope_user_id=body.scope.user_id,
        scope_api_key_id=body.scope.api_key_id,
        filters=_build_filters(body.filters),
        search=_build_search(body.search),
        limit=body.pagination.limit,
        offset=body.pagination.offset,
    )
    return (
        result
        if current_user.role == UserRole.ADMIN
        else _redact_sensitive_cost_fields(result)
    )


@router.post("/filter-options")
async def analytics_filter_options(
    body: AnalyticsBaseRequest,
    current_user: User = Depends(get_current_user),
    db: Session = Depends(get_db),
) -> dict:
    _validate_scope(current_user, body.scope)
    time_range = _build_time_range(body.time_range)
    result = AnalyticsQueryService.filter_options(
        db,
        current_user,
        time_range=time_range,
        scope_kind=body.scope.kind,
        scope_user_id=body.scope.user_id,
        scope_api_key_id=body.scope.api_key_id,
        filters=_build_filters(body.filters),
    )
    return (
        result
        if current_user.role == UserRole.ADMIN
        else _redact_sensitive_cost_fields(result)
    )


@router.post("/heatmap")
async def analytics_heatmap(
    body: AnalyticsHeatmapRequest,
    current_user: User = Depends(get_current_user),
    db: Session = Depends(get_db),
) -> dict[str, Any]:
    resolved_user_id, resolved_api_key_id = _resolve_scope_targets(
        current_user,
        body.scope,
        user_id=body.user_id,
        api_key_id=body.api_key_id,
    )
    result = await UsageService.get_cached_heatmap(
        db=db,
        user_id=None if resolved_user_id == DELETED_USER_FILTER else resolved_user_id,
        api_key_id=None if resolved_api_key_id == DELETED_API_KEY_FILTER else resolved_api_key_id,
        deleted_user_only=resolved_user_id == DELETED_USER_FILTER,
        deleted_api_key_only=resolved_api_key_id == DELETED_API_KEY_FILTER,
        include_actual_cost=current_user.role == UserRole.ADMIN,
    )
    return result if current_user.role == UserRole.ADMIN else _redact_sensitive_cost_fields(result)


@router.post("/active-requests")
async def analytics_active_requests(
    body: AnalyticsActiveRequestsRequest,
    current_user: User = Depends(get_current_user),
    db: Session = Depends(get_db),
) -> dict[str, Any]:
    resolved_user_id, resolved_api_key_id = _resolve_scope_targets(current_user, body.scope)
    requests = UsageService.get_active_requests_status(
        db=db,
        ids=_normalize_optional_ids(body.ids) or None,
        user_id=resolved_user_id,
        api_key_id=resolved_api_key_id,
        include_admin_fields=current_user.role == UserRole.ADMIN,
        maintain_status=True,
    )
    payload = {"requests": requests}
    return payload if current_user.role == UserRole.ADMIN else _redact_sensitive_cost_fields(payload)


@router.post("/interval-timeline")
async def analytics_interval_timeline(
    body: AnalyticsIntervalTimelineRequest,
    current_user: User = Depends(get_current_user),
    db: Session = Depends(get_db),
) -> dict[str, Any]:
    resolved_user_id, resolved_api_key_id = _resolve_scope_targets(
        current_user,
        body.scope,
        user_id=body.user_id,
    )
    if resolved_api_key_id:
        raise HTTPException(status_code=400, detail="interval timeline does not support api_key scope")
    if body.include_user_info and current_user.role != UserRole.ADMIN:
        raise HTTPException(status_code=403, detail="Only admin can request user timeline labels")
    return UsageService.get_interval_timeline(
        db=db,
        hours=body.hours,
        limit=body.limit,
        user_id=resolved_user_id,
        include_user_info=body.include_user_info and resolved_user_id is None,
    )


@router.post("/cache-affinity/ttl-analysis")
async def analytics_cache_affinity_ttl_analysis(
    body: AnalyticsCacheAffinityRequest,
    current_user: User = Depends(get_current_user),
    db: Session = Depends(get_db),
) -> dict[str, Any]:
    resolved_user_id, resolved_api_key_id = _resolve_scope_targets(
        current_user,
        body.scope,
        user_id=body.user_id,
        api_key_id=body.api_key_id,
    )
    if current_user.role != UserRole.ADMIN:
        raise HTTPException(status_code=403, detail="Only admin can access cache affinity analysis")
    return UsageService.analyze_cache_affinity_ttl(
        db=db,
        user_id=resolved_user_id,
        api_key_id=resolved_api_key_id,
        hours=body.hours,
    )


@router.post("/cache-affinity/hit-analysis")
async def analytics_cache_affinity_hit_analysis(
    body: AnalyticsCacheAffinityRequest,
    current_user: User = Depends(get_current_user),
    db: Session = Depends(get_db),
) -> dict[str, Any]:
    resolved_user_id, resolved_api_key_id = _resolve_scope_targets(
        current_user,
        body.scope,
        user_id=body.user_id,
        api_key_id=body.api_key_id,
    )
    if current_user.role != UserRole.ADMIN:
        raise HTTPException(status_code=403, detail="Only admin can access cache affinity analysis")
    return UsageService.get_cache_hit_analysis(
        db=db,
        user_id=resolved_user_id,
        api_key_id=resolved_api_key_id,
        hours=body.hours,
    )


@router.post("/leaderboard")
async def analytics_leaderboard(
    body: AnalyticsLeaderboardRequest,
    current_user: User = Depends(get_current_user),
    db: Session = Depends(get_db),
) -> dict:
    _validate_scope(current_user, body.scope)
    time_range = _build_time_range(body.time_range)
    result = AnalyticsQueryService.leaderboard(
        db,
        current_user,
        time_range=time_range,
        scope_kind=body.scope.kind,
        scope_user_id=body.scope.user_id,
        scope_api_key_id=body.scope.api_key_id,
        filters=_build_filters(body.filters),
        entity=body.entity,
        metric=body.metric,
        limit=body.limit,
    )
    return (
        result
        if current_user.role == UserRole.ADMIN
        else _redact_sensitive_cost_fields(result)
    )


@router.post("/performance")
async def analytics_performance(
    body: AnalyticsBaseRequest,
    current_user: User = Depends(get_current_user),
    db: Session = Depends(get_db),
) -> dict:
    _validate_scope(current_user, body.scope)
    time_range = _build_time_range(body.time_range)
    result = AnalyticsQueryService.performance(
        db,
        current_user,
        time_range=time_range,
        scope_kind=body.scope.kind,
        scope_user_id=body.scope.user_id,
        scope_api_key_id=body.scope.api_key_id,
        filters=_build_filters(body.filters),
    )
    return (
        result
        if current_user.role == UserRole.ADMIN
        else _redact_sensitive_cost_fields(result)
    )
