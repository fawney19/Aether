"""Pool management admin API routes.

Provides endpoints for managing account pools at scale:
- Overview of all pool-enabled providers
- Paginated key listing with search/filter
- Batch import / batch actions
"""

from __future__ import annotations

import asyncio
import json
import re
import time
import uuid
from dataclasses import dataclass, field
from datetime import datetime, timezone
from typing import Any, cast

from fastapi import APIRouter, Depends, Query, Request
from sqlalchemy import case
from sqlalchemy import delete as sa_delete
from sqlalchemy import func
from sqlalchemy.orm import Session, load_only

from src.api.base.admin_adapter import AdminApiAdapter
from src.api.base.context import ApiRequestContext
from src.api.base.pipeline import get_pipeline
from src.core.crypto import crypto_service
from src.core.exceptions import NotFoundException
from src.core.logger import logger
from src.core.provider_oauth_utils import normalize_oauth_organizations
from src.database import get_db
from src.models.database import Provider, ProviderAPIKey
from src.services.billing.precision import to_money_decimal
from src.services.provider.fingerprint import generate_fingerprint
from src.services.provider.pool import redis_ops as pool_redis
from src.services.provider.pool.account_state import resolve_pool_account_state
from src.services.provider.pool.config import parse_pool_config
from src.services.provider.pool.dimensions import get_preset_dimension_metas
from src.services.provider.pool.scheduling_dimensions import (
    PoolSchedulingSnapshot,
    evaluate_pool_scheduling_dimensions,
    summarize_pool_scheduling_dimensions,
)
from src.services.provider_keys.key_side_effects import cleanup_key_references
from src.services.provider_keys.quota_reader import get_quota_reader

from .schemas import (
    BatchActionRequest,
    BatchActionResponse,
    BatchDeleteTaskResponse,
    BatchImportError,
    BatchImportRequest,
    BatchImportResponse,
    OAuthOrganizationSummary,
    PoolKeyDetail,
    PoolKeySelectionItem,
    PoolKeySelectionRequest,
    PoolKeySelectionResponse,
    PoolKeysPageResponse,
    PoolOverviewItem,
    PoolOverviewResponse,
    PoolSchedulingReason,
    PresetDimensionMetaResponse,
    PresetModeMetaResponse,
)

router = APIRouter(prefix="/api/admin/pool", tags=["pool-management"])
pipeline = get_pipeline()


# ---------------------------------------------------------------------------
# GET /api/admin/pool/overview
# ---------------------------------------------------------------------------


@router.get("/overview", response_model=PoolOverviewResponse)
async def pool_overview(
    request: Request,
    db: Session = Depends(get_db),
) -> PoolOverviewResponse:
    """Return all pool-enabled providers with summary stats."""
    adapter = AdminPoolOverviewAdapter()
    return await pipeline.run(
        adapter=adapter, http_request=request, db=db, mode=adapter.mode
    )


# ---------------------------------------------------------------------------
# GET /api/admin/pool/scheduling-presets
# ---------------------------------------------------------------------------


def _preset_mode_label(mode: str) -> str:
    mapping = {
        "free_only": "Free",
        "team_only": "Team",
        "both": "全部",
    }
    return mapping.get(mode, mode)


@router.get("/scheduling-presets", response_model=list[PresetDimensionMetaResponse])
async def list_scheduling_presets(
    request: Request,
    db: Session = Depends(get_db),
) -> list[PresetDimensionMetaResponse]:
    """Return scheduling preset definitions for frontend rendering."""

    adapter = AdminListSchedulingPresetsAdapter()
    return await pipeline.run(
        adapter=adapter, http_request=request, db=db, mode=adapter.mode
    )


# ---------------------------------------------------------------------------
# GET /api/admin/pool/{provider_id}/keys
# ---------------------------------------------------------------------------


@router.get("/{provider_id}/keys", response_model=PoolKeysPageResponse)
async def list_pool_keys(
    provider_id: str,
    request: Request,
    page: int = Query(1, ge=1),
    page_size: int = Query(50, ge=1, le=200),
    search: str = Query("", description="Search by key name"),
    status: str = Query("all", description="all/active/cooldown/inactive"),
    quick_selectors: str = Query(
        "", description="Comma-separated quick selectors for batch dialog"
    ),
    search_scope: str = Query("name", description="Search scope: name/full"),
    db: Session = Depends(get_db),
) -> PoolKeysPageResponse:
    """Server-side paginated account list for a pool-enabled provider."""
    adapter = AdminListPoolKeysAdapter(
        provider_id=provider_id,
        page=page,
        page_size=page_size,
        search=search,
        status=status,
        quick_selectors=quick_selectors.split(",") if quick_selectors else [],
        search_scope=search_scope,
    )
    return await pipeline.run(
        adapter=adapter, http_request=request, db=db, mode=adapter.mode
    )


# ---------------------------------------------------------------------------
# POST /api/admin/pool/{provider_id}/keys/batch-import
# ---------------------------------------------------------------------------


@router.post("/{provider_id}/keys/batch-import", response_model=BatchImportResponse)
async def batch_import_keys(
    provider_id: str,
    body: BatchImportRequest,
    request: Request,
    db: Session = Depends(get_db),
) -> BatchImportResponse:
    """Batch import keys into a provider's pool."""
    adapter = AdminBatchImportKeysAdapter(provider_id=provider_id, body=body)
    return await pipeline.run(
        adapter=adapter, http_request=request, db=db, mode=adapter.mode
    )


# ---------------------------------------------------------------------------
# POST /api/admin/pool/{provider_id}/keys/batch-action
# ---------------------------------------------------------------------------

ALLOWED_ACTIONS = {
    "enable",
    "disable",
    "delete",
    "clear_cooldown",
    "reset_cost",
    "regenerate_fingerprint",
    "clear_proxy",
    "set_proxy",
}

_SQLITE_DELETE_BATCH_SIZE = 900
_DEFAULT_DELETE_BATCH_SIZE = 2000


def _iter_batches(items: list[str], batch_size: int) -> list[list[str]]:
    if batch_size <= 0:
        return [items]
    return [items[i : i + batch_size] for i in range(0, len(items), batch_size)]


def _resolve_delete_batch_size(db: Session) -> int:
    try:
        bind = db.get_bind()
        dialect_name = str(
            getattr(getattr(bind, "dialect", None), "name", "") or ""
        ).lower()
    except Exception:
        dialect_name = ""

    if dialect_name == "sqlite":
        return _SQLITE_DELETE_BATCH_SIZE
    return _DEFAULT_DELETE_BATCH_SIZE


_COOLDOWN_REASON_LABELS: dict[str, str] = {
    "rate_limited_429": "429 限流",
    "forbidden_403": "403 禁止",
    "overloaded_529": "529 过载",
    "auth_failed_401": "401 认证失败",
    "payment_required_402": "402 欠费",
    "server_error_500": "500 错误",
    "request_timeout_408": "408 超时",
    "conflict_409": "409 冲突",
    "locked_423": "423 锁定",
    "too_early_425": "425 Too Early",
    "bad_gateway_502": "502 网关错误",
    "service_unavailable_503": "503 服务不可用",
    "gateway_timeout_504": "504 网关超时",
}


def _to_float(value: Any) -> float | None:
    if isinstance(value, bool):
        return None
    if isinstance(value, (int, float)):
        return float(value)
    if isinstance(value, str):
        raw = value.strip()
        if not raw:
            return None
        try:
            return float(raw)
        except ValueError:
            return None
    return None


def _serialize_money(value: Any) -> str:
    return format(to_money_decimal(value), "f")


def _is_known_banned_key(key: ProviderAPIKey, provider_type: str) -> bool:
    from src.services.provider.pool.account_state import resolve_pool_account_state

    state = resolve_pool_account_state(
        provider_type=provider_type,
        upstream_metadata=getattr(key, "upstream_metadata", None),
        oauth_invalid_reason=getattr(key, "oauth_invalid_reason", None),
    )
    return state.blocked


def _build_account_quota(provider_type: str, upstream_metadata: Any) -> str | None:
    return get_quota_reader(provider_type, upstream_metadata).display_summary()


def _extract_quota_updated_at(provider_type: str, upstream_metadata: Any) -> int | None:
    return get_quota_reader(provider_type, upstream_metadata).updated_at()


def _normalize_oauth_plan_type(plan_type: Any, provider_type: str) -> str | None:
    if not isinstance(plan_type, str):
        return None
    text = plan_type.strip()
    if not text:
        return None

    ptype = provider_type.strip().lower()
    if ptype and text.lower().startswith(ptype):
        trimmed = text[len(ptype) :].strip(" :-_")
        if trimmed:
            text = trimmed

    return text or None


def _extract_oauth_auth_config(key: ProviderAPIKey) -> dict[str, Any] | None:
    if str(getattr(key, "auth_type", "") or "").strip().lower() != "oauth":
        return None

    auth_config_raw = getattr(key, "auth_config", None)
    if not auth_config_raw:
        return None

    try:
        decrypted = crypto_service.decrypt(auth_config_raw)
        parsed = json.loads(decrypted)
        if isinstance(parsed, dict):
            return parsed
    except Exception:
        return None

    return None


def _normalize_oauth_expires_at(raw: Any) -> int | None:
    value = _to_float(raw)
    if value is None or value <= 0:
        return None
    # 兼容毫秒时间戳
    if value > 1_000_000_000_000:
        value /= 1000
    return int(value)


def _derive_oauth_expires_at(
    key: ProviderAPIKey, auth_config: dict[str, Any] | None = None
) -> int | None:
    if str(getattr(key, "auth_type", "") or "").strip().lower() != "oauth":
        return None

    cfg = (
        auth_config
        if isinstance(auth_config, dict)
        else _extract_oauth_auth_config(key)
    )
    if cfg:
        for field in ("expires_at", "expiresAt", "expiry", "exp"):
            expires_at = _normalize_oauth_expires_at(cfg.get(field))
            if expires_at is not None:
                return expires_at

    # 兼容历史字段
    expires_dt = getattr(key, "expires_at", None)
    if isinstance(expires_dt, datetime):
        return int(expires_dt.timestamp())
    return None


def _derive_oauth_plan_type(
    key: ProviderAPIKey,
    provider_type: str,
    auth_config: dict[str, Any] | None = None,
) -> str | None:
    # Prefer persisted normalized field
    persisted = _normalize_oauth_plan_type(
        getattr(key, "oauth_plan_type", None), provider_type
    )
    if persisted:
        return persisted

    if str(getattr(key, "auth_type", "") or "").strip().lower() != "oauth":
        return None

    # Fallback 1: encrypted auth_config (common for Codex/Antigravity)
    cfg = (
        auth_config
        if isinstance(auth_config, dict)
        else _extract_oauth_auth_config(key)
    )
    if cfg:
        for plan_key in ("plan_type", "tier", "plan", "subscription_plan"):
            normalized = _normalize_oauth_plan_type(cfg.get(plan_key), provider_type)
            if normalized:
                return normalized

    # Fallback 2: upstream_metadata
    upstream_metadata = getattr(key, "upstream_metadata", None)
    if not isinstance(upstream_metadata, dict):
        return None

    provider_bucket = upstream_metadata.get(provider_type.strip().lower())
    candidates: list[dict[str, Any]] = []
    if isinstance(provider_bucket, dict):
        candidates.append(provider_bucket)
    candidates.append(upstream_metadata)

    for source in candidates:
        for plan_key in (
            "plan_type",
            "tier",
            "subscription_title",
            "subscription_plan",
        ):
            normalized = _normalize_oauth_plan_type(source.get(plan_key), provider_type)
            if normalized:
                return normalized
    return None


def _derive_oauth_account_id(auth_config: dict[str, Any] | None = None) -> str | None:
    if not isinstance(auth_config, dict):
        return None
    raw = auth_config.get("account_id")
    if not isinstance(raw, str):
        return None
    normalized = raw.strip()
    return normalized or None


def _derive_oauth_account_name(auth_config: dict[str, Any] | None = None) -> str | None:
    if not isinstance(auth_config, dict):
        return None
    raw = auth_config.get("account_name")
    if not isinstance(raw, str):
        return None
    normalized = raw.strip()
    return normalized or None


def _derive_oauth_account_user_id(
    auth_config: dict[str, Any] | None = None,
) -> str | None:
    if not isinstance(auth_config, dict):
        return None
    raw = auth_config.get("account_user_id")
    if not isinstance(raw, str):
        return None
    normalized = raw.strip()
    return normalized or None


def _derive_oauth_organizations(
    auth_config: dict[str, Any] | None = None,
) -> list[OAuthOrganizationSummary]:
    if not isinstance(auth_config, dict):
        return []
    return [
        OAuthOrganizationSummary(**item)
        for item in normalize_oauth_organizations(auth_config.get("organizations"))
    ]


def _compute_health_aggregate(
    health_by_format: Any, circuit_breaker_by_format: Any
) -> tuple[float, bool]:
    """从按格式健康数据聚合出列表展示字段。"""
    health_map = health_by_format if isinstance(health_by_format, dict) else {}
    circuit_map = (
        circuit_breaker_by_format if isinstance(circuit_breaker_by_format, dict) else {}
    )

    if health_map:
        scores = [
            float(item.get("health_score") or 1.0)
            for item in health_map.values()
            if isinstance(item, dict)
        ]
        health_score = min(scores) if scores else 1.0
    else:
        health_score = 1.0

    any_circuit_open = any(
        bool(item.get("open", False))
        for item in circuit_map.values()
        if isinstance(item, dict)
    )

    return health_score, any_circuit_open


def _format_cooldown_detail(raw: str | None) -> str | None:
    if not raw:
        return None
    return _COOLDOWN_REASON_LABELS.get(raw, raw)


def _build_pool_scheduling_state(
    *,
    is_active: bool,
    account_blocked: bool,
    account_block_label: str | None,
    account_block_reason: str | None,
    latency_avg_ms: float | None,
    cooldown_reason: str | None,
    cooldown_ttl_seconds: int | None,
    circuit_breaker_open: bool,
    cost_window_usage: int,
    cost_limit: int | None,
    cost_soft_threshold_percent: int,
    health_score: float,
) -> tuple[str, str, str, list[PoolSchedulingReason]]:
    """Build unified scheduling state for frontend display."""
    snapshot = PoolSchedulingSnapshot(
        is_active=is_active,
        account_blocked=account_blocked,
        account_block_label=account_block_label,
        account_block_reason=account_block_reason,
        latency_avg_ms=latency_avg_ms,
        cooldown_reason=cooldown_reason,
        cooldown_ttl_seconds=cooldown_ttl_seconds,
        circuit_breaker_open=circuit_breaker_open,
        cost_window_usage=cost_window_usage,
        cost_limit=cost_limit,
        cost_soft_threshold_percent=cost_soft_threshold_percent,
        health_score=health_score,
    )
    dimensions_raw = evaluate_pool_scheduling_dimensions(snapshot)
    summary = summarize_pool_scheduling_dimensions(dimensions_raw)

    scheduling_reasons: list[PoolSchedulingReason] = []
    for item in dimensions_raw:
        if item.status != "ok":
            detail = item.detail
            if item.code == "cooldown":
                detail = _format_cooldown_detail(detail)
            scheduling_reasons.append(
                PoolSchedulingReason(
                    code=item.code,
                    label=item.label,
                    blocking=bool(item.blocking or item.status == "blocked"),
                    source=item.source,
                    ttl_seconds=item.ttl_seconds,
                    detail=detail,
                )
            )

    return (
        summary.status,
        summary.reason,
        summary.label,
        scheduling_reasons,
    )


def _mask_proxy_password(proxy_config: Any) -> dict[str, Any] | None:
    if not isinstance(proxy_config, dict):
        return None
    masked = dict(proxy_config)
    password = masked.get("password")
    if isinstance(password, str) and password:
        masked["password"] = "******"
    return masked


@router.post("/{provider_id}/keys/batch-action", response_model=BatchActionResponse)
async def batch_action_keys(
    provider_id: str,
    body: BatchActionRequest,
    request: Request,
    db: Session = Depends(get_db),
) -> BatchActionResponse:
    """Batch enable/disable/delete/clear_cooldown/reset_cost/regenerate_fingerprint on pool keys."""
    adapter = AdminBatchActionKeysAdapter(provider_id=provider_id, body=body)
    return await pipeline.run(
        adapter=adapter, http_request=request, db=db, mode=adapter.mode
    )


@router.post(
    "/{provider_id}/keys/resolve-selection", response_model=PoolKeySelectionResponse
)
async def resolve_pool_key_selection(
    provider_id: str,
    body: PoolKeySelectionRequest,
    request: Request,
    db: Session = Depends(get_db),
) -> PoolKeySelectionResponse:
    """Resolve all key ids matching the current batch dialog filters."""
    adapter = AdminResolvePoolKeySelectionAdapter(provider_id=provider_id, body=body)
    return await pipeline.run(
        adapter=adapter, http_request=request, db=db, mode=adapter.mode
    )


@router.get(
    "/{provider_id}/keys/batch-delete-task/{task_id}",
    response_model=BatchDeleteTaskResponse,
)
async def get_batch_delete_task_status(
    provider_id: str,
    task_id: str,
    request: Request,
    db: Session = Depends(get_db),
) -> BatchDeleteTaskResponse:
    """Query the progress of an async batch-delete task."""
    adapter = AdminBatchDeleteTaskStatusAdapter(
        provider_id=provider_id, task_id=task_id
    )
    return await pipeline.run(
        adapter=adapter, http_request=request, db=db, mode=adapter.mode
    )


@router.post("/{provider_id}/keys/cleanup-banned", response_model=BatchActionResponse)
async def cleanup_banned_keys(
    provider_id: str,
    request: Request,
    db: Session = Depends(get_db),
) -> BatchActionResponse:
    """Delete known banned/suspended accounts for the provider."""
    adapter = AdminCleanupBannedKeysAdapter(provider_id=provider_id)
    return await pipeline.run(
        adapter=adapter, http_request=request, db=db, mode=adapter.mode
    )


# ---------------------------------------------------------------------------
# Adapters
# ---------------------------------------------------------------------------


@dataclass
class AdminBatchDeleteTaskStatusAdapter(AdminApiAdapter):
    provider_id: str = ""
    task_id: str = ""

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        from fastapi import HTTPException

        from src.services.provider_keys.batch_delete_task import get_batch_delete_task

        task = await get_batch_delete_task(self.task_id)
        if task is None or task.provider_id != self.provider_id:
            raise HTTPException(status_code=404, detail="Task not found")
        return BatchDeleteTaskResponse(
            task_id=task.task_id,
            status=task.status,
            total=task.total,
            deleted=task.deleted,
            message=task.message,
        )


class AdminListSchedulingPresetsAdapter(AdminApiAdapter):
    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        items: list[PresetDimensionMetaResponse] = [
            PresetDimensionMetaResponse(
                name="lru",
                label="LRU 轮转",
                description="最久未使用的 Key 优先",
                providers=[],
                modes=None,
                default_mode=None,
                mutex_group="distribution_mode",
                evidence_hint="依据 LRU 时间戳（最近未使用优先）",
            )
        ]

        for meta in get_preset_dimension_metas():
            modes = None
            if meta.modes:
                modes = [
                    PresetModeMetaResponse(value=mode, label=_preset_mode_label(mode))
                    for mode in meta.modes
                ]
            items.append(
                PresetDimensionMetaResponse(
                    name=meta.name,
                    label=meta.label,
                    description=meta.description,
                    providers=list(meta.providers),
                    modes=modes,
                    default_mode=meta.default_mode,
                    mutex_group=meta.mutex_group,
                    evidence_hint=meta.evidence_hint,
                )
            )
        return items


class AdminPoolOverviewAdapter(AdminApiAdapter):
    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        db = context.db
        providers = (
            db.query(Provider)
            .options(
                load_only(*cast(tuple[Any, ...], _PROVIDER_OVERVIEW_LOAD_ONLY_ATTRS))
            )
            .order_by(Provider.provider_priority.asc())
            .all()
        )

        # 仅保留号池调度已开启的 Provider。
        enabled_providers: list[Provider] = []
        pool_provider_ids: list[str] = []
        for p in providers:
            if parse_pool_config(getattr(p, "config", None)) is None:
                continue
            enabled_providers.append(p)
            pool_provider_ids.append(str(p.id))

        key_stats_by_provider: dict[str, dict[str, int]] = {}
        if pool_provider_ids:
            key_rows = (
                db.query(
                    ProviderAPIKey.provider_id,
                    func.count(ProviderAPIKey.id).label("total"),
                    func.coalesce(
                        func.sum(
                            case((ProviderAPIKey.is_active.is_(True), 1), else_=0)
                        ),
                        0,
                    ).label("active"),
                )
                .filter(ProviderAPIKey.provider_id.in_(pool_provider_ids))
                .group_by(ProviderAPIKey.provider_id)
                .all()
            )
            for provider_id, total, active in key_rows:
                pid = str(provider_id)
                key_stats_by_provider[pid] = {
                    "total": int(total or 0),
                    "active": int(active or 0),
                }

        # Redis 冷却状态按 Provider 统计，避免先拉取全量 key_id 再逐个检查。
        cooldown_count_by_provider: dict[str, int] = {}
        cooldown_targets = [
            pid
            for pid in pool_provider_ids
            if key_stats_by_provider.get(pid, {}).get("total", 0) > 0
        ]
        if cooldown_targets:
            cooldown_count_by_provider = (
                await pool_redis.batch_count_provider_cooldowns(cooldown_targets)
            )

        items: list[PoolOverviewItem] = []
        for p in enabled_providers:
            pid = str(p.id)
            key_stats = key_stats_by_provider.get(pid, {"total": 0, "active": 0})

            items.append(
                PoolOverviewItem(
                    provider_id=pid,
                    provider_name=str(getattr(p, "name", "") or ""),
                    provider_type=str(
                        getattr(p, "provider_type", "custom") or "custom"
                    ),
                    total_keys=key_stats["total"],
                    active_keys=key_stats["active"],
                    cooldown_count=cooldown_count_by_provider.get(pid, 0),
                    pool_enabled=True,
                )
            )

        return PoolOverviewResponse(items=items)


_FULL_SEARCH_SCOPE = "full"
_ALLOWED_POOL_KEY_QUICK_SELECTORS = frozenset(
    {
        "banned",
        "no_5h_limit",
        "no_weekly_limit",
        "plan_free",
        "plan_team",
        "oauth_invalid",
        "proxy_unset",
        "proxy_set",
        "disabled",
        "enabled",
    }
)
_ACCOUNT_BANNED_CODES = frozenset(
    {"account_banned", "account_forbidden", "account_blocked"}
)
_BANNED_REASON_PATTERN = re.compile(r"(banned|forbidden|blocked|suspend|封|禁|受限)")

_PROVIDER_OVERVIEW_LOAD_ONLY_ATTRS: tuple[Any, ...] = (
    cast(Any, Provider.id),
    cast(Any, Provider.name),
    cast(Any, Provider.provider_type),
    cast(Any, Provider.provider_priority),
    cast(Any, Provider.config),
)

_POOL_KEY_LOAD_ONLY_ATTRS: tuple[Any, ...] = (
    cast(Any, ProviderAPIKey.id),
    cast(Any, ProviderAPIKey.provider_id),
    cast(Any, ProviderAPIKey.name),
    cast(Any, ProviderAPIKey.auth_type),
    cast(Any, ProviderAPIKey.auth_config),
    cast(Any, ProviderAPIKey.is_active),
    cast(Any, ProviderAPIKey.expires_at),
    cast(Any, ProviderAPIKey.oauth_invalid_at),
    cast(Any, ProviderAPIKey.oauth_invalid_reason),
    cast(Any, ProviderAPIKey.api_formats),
    cast(Any, ProviderAPIKey.rate_multipliers),
    cast(Any, ProviderAPIKey.internal_priority),
    cast(Any, ProviderAPIKey.rpm_limit),
    cast(Any, ProviderAPIKey.cache_ttl_minutes),
    cast(Any, ProviderAPIKey.max_probe_interval_minutes),
    cast(Any, ProviderAPIKey.note),
    cast(Any, ProviderAPIKey.allowed_models),
    cast(Any, ProviderAPIKey.capabilities),
    cast(Any, ProviderAPIKey.auto_fetch_models),
    cast(Any, ProviderAPIKey.locked_models),
    cast(Any, ProviderAPIKey.model_include_patterns),
    cast(Any, ProviderAPIKey.model_exclude_patterns),
    cast(Any, ProviderAPIKey.proxy),
    cast(Any, ProviderAPIKey.fingerprint),
    cast(Any, ProviderAPIKey.health_by_format),
    cast(Any, ProviderAPIKey.circuit_breaker_by_format),
    cast(Any, ProviderAPIKey.request_count),
    cast(Any, ProviderAPIKey.total_tokens),
    cast(Any, ProviderAPIKey.total_cost_usd),
    cast(Any, ProviderAPIKey.last_used_at),
    cast(Any, ProviderAPIKey.created_at),
    cast(Any, ProviderAPIKey.upstream_metadata),
)


def _normalize_batch_text(value: Any) -> str:
    return str(value or "").strip().lower()


def _normalize_pool_search_scope(value: Any) -> str:
    return (
        _FULL_SEARCH_SCOPE
        if _normalize_batch_text(value) == _FULL_SEARCH_SCOPE
        else "name"
    )


def _normalize_pool_quick_selectors(values: Any) -> list[str]:
    if values is None:
        return []
    if isinstance(values, str):
        raw_items = values.split(",")
    elif isinstance(values, (list, tuple, set)):
        raw_items = [str(item) for item in values]
    else:
        return []

    normalized: list[str] = []
    seen: set[str] = set()
    for raw in raw_items:
        item = _normalize_batch_text(raw)
        if not item or item not in _ALLOWED_POOL_KEY_QUICK_SELECTORS or item in seen:
            continue
        seen.add(item)
        normalized.append(item)
    return normalized


def _normalize_quota_segment(value: Any) -> str:
    return str(value or "").strip().lower().replace("％", "%")


def _get_quota_segments(account_quota: Any) -> list[str]:
    return [
        segment
        for segment in (
            _normalize_quota_segment(part)
            for part in str(account_quota or "").split("|")
        )
        if segment
    ]


def _quota_segment_has_depleted_keyword(segment: str) -> bool:
    return bool(
        re.search(
            r"(无额度|额度不足|已耗尽|耗尽|depleted|exhausted|insufficient)", segment
        )
    )


def _quota_segment_has_zero_remaining_text(segment: str) -> bool:
    return bool(re.search(r"剩余\s*0(?:\.0+)?(?!\d)", segment))


def _quota_segment_has_zero_ratio(segment: str) -> bool:
    for match in re.finditer(r"(\d+(?:\.\d+)?)\s*/\s*(\d+(?:\.\d+)?)", segment):
        numerator = float(match.group(1))
        denominator = float(match.group(2))
        if numerator == 0 and denominator > 0:
            return True
    return False


def _quota_segment_has_zero_percent(segment: str) -> bool:
    for match in re.finditer(r"(\d+(?:\.\d+)?)\s*%", segment):
        if float(match.group(1)) == 0:
            return True
    return False


def _is_depleted_quota_segment(segment: str) -> bool:
    return (
        _quota_segment_has_depleted_keyword(segment)
        or _quota_segment_has_zero_remaining_text(segment)
        or _quota_segment_has_zero_ratio(segment)
        or _quota_segment_has_zero_percent(segment)
    )


def _has_no_five_hour_limit(account_quota: Any) -> bool:
    return any(
        _is_depleted_quota_segment(segment)
        for segment in _get_quota_segments(account_quota)
        if "5h" in segment or "5小时" in segment
    )


def _has_no_weekly_limit(account_quota: Any) -> bool:
    return any(
        _is_depleted_quota_segment(segment)
        for segment in _get_quota_segments(account_quota)
        if "周" in segment or "weekly" in segment or "week" in segment
    )


def _detail_is_oauth_invalid(detail: PoolKeyDetail) -> bool:
    if _normalize_batch_text(detail.auth_type) != "oauth":
        return False
    if detail.oauth_invalid_at is not None or _normalize_batch_text(
        detail.oauth_invalid_reason
    ):
        return True
    expires_at = detail.oauth_expires_at
    return (
        isinstance(expires_at, int)
        and expires_at > 0
        and expires_at <= int(time.time())
    )


def _detail_is_banned(detail: PoolKeyDetail) -> bool:
    reason = _normalize_batch_text(detail.oauth_invalid_reason)
    if reason and _BANNED_REASON_PATTERN.search(reason):
        return True
    for item in detail.scheduling_reasons or []:
        code = _normalize_batch_text(getattr(item, "code", ""))
        if code in _ACCOUNT_BANNED_CODES:
            return True
    return False


def _detail_has_proxy(detail: PoolKeyDetail) -> bool:
    proxy = detail.proxy if isinstance(detail.proxy, dict) else None
    return bool(_normalize_batch_text((proxy or {}).get("node_id")))


def _matches_pool_key_search(
    detail: PoolKeyDetail,
    search: str,
    *,
    search_scope: str = _FULL_SEARCH_SCOPE,
) -> bool:
    keyword = _normalize_batch_text(search)
    if not keyword:
        return True
    if search_scope != _FULL_SEARCH_SCOPE:
        return keyword in _normalize_batch_text(detail.key_name)

    parts = [
        detail.key_name,
        detail.auth_type,
        detail.oauth_plan_type,
        detail.account_quota,
        "独立代理" if _detail_has_proxy(detail) else "未配置代理",
        "已启用" if detail.is_active else "已禁用",
        detail.oauth_invalid_reason,
    ]
    return any(keyword in _normalize_batch_text(part) for part in parts)


def _matches_pool_key_quick_selector(detail: PoolKeyDetail, selector: str) -> bool:
    if selector == "banned":
        return _detail_is_banned(detail)
    if selector == "no_5h_limit":
        return _has_no_five_hour_limit(detail.account_quota)
    if selector == "no_weekly_limit":
        return _has_no_weekly_limit(detail.account_quota)
    if selector == "plan_free":
        return "free" in _normalize_batch_text(detail.oauth_plan_type)
    if selector == "plan_team":
        return "team" in _normalize_batch_text(detail.oauth_plan_type)
    if selector == "oauth_invalid":
        return _detail_is_oauth_invalid(detail)
    if selector == "proxy_unset":
        return not _detail_has_proxy(detail)
    if selector == "proxy_set":
        return _detail_has_proxy(detail)
    if selector == "disabled":
        return not detail.is_active
    if selector == "enabled":
        return detail.is_active
    return False


def _filter_pool_key_details(
    details: list[PoolKeyDetail],
    *,
    search: str = "",
    quick_selectors: list[str] | None = None,
    search_scope: str = _FULL_SEARCH_SCOPE,
    require_cooldown: bool = False,
) -> list[PoolKeyDetail]:
    normalized_selectors = _normalize_pool_quick_selectors(quick_selectors)
    normalized_search_scope = _normalize_pool_search_scope(search_scope)
    filtered: list[PoolKeyDetail] = []
    for detail in details:
        if require_cooldown and not detail.cooldown_reason:
            continue
        if not _matches_pool_key_search(
            detail, search, search_scope=normalized_search_scope
        ):
            continue
        if normalized_selectors and not any(
            _matches_pool_key_quick_selector(detail, selector)
            for selector in normalized_selectors
        ):
            continue
        filtered.append(detail)
    return filtered


def _build_pool_keys_base_query(db: Session, provider_id: str) -> Any:
    return (
        db.query(ProviderAPIKey)
        .options(load_only(*_POOL_KEY_LOAD_ONLY_ATTRS))
        .filter(ProviderAPIKey.provider_id == provider_id)
    )


def _apply_pool_key_order(query: Any) -> Any:
    return query.order_by(
        ProviderAPIKey.internal_priority.asc(),
        ProviderAPIKey.created_at.asc(),
    )


async def _serialize_pool_key_details(
    *,
    keys: list[ProviderAPIKey],
    pid: str,
    provider_type: str,
    pcfg: Any,
) -> tuple[list[PoolKeyDetail], float, float]:
    redis_state_ms = 0.0
    key_ids = [str(k.id) for k in keys]
    sticky_counts: dict[str, int] = {kid: 0 for kid in key_ids}

    if key_ids:
        _lru_coro = (
            pool_redis.get_lru_scores(pid, key_ids)
            if pcfg and pcfg.lru_enabled
            else asyncio.sleep(0, result={})
        )
        _latency_coro = (
            pool_redis.batch_get_latency_avgs(pid, key_ids, pcfg.latency_window_seconds)
            if pcfg and pcfg.scheduling_mode == "multi_score"
            else asyncio.sleep(0, result={})
        )
        _cost_coro = (
            pool_redis.batch_get_cost_totals(pid, key_ids, pcfg.cost_window_seconds)
            if pcfg
            else asyncio.sleep(0, result={})
        )
        redis_started_at = time.perf_counter()
        (
            cooldowns,
            cooldown_ttls,
            lru_scores,
            latency_avgs,
            cost_totals,
        ) = await asyncio.gather(
            pool_redis.batch_get_cooldowns(pid, key_ids),
            pool_redis.batch_get_cooldown_ttls(pid, key_ids),
            _lru_coro,
            _latency_coro,
            _cost_coro,
        )
        redis_state_ms += (time.perf_counter() - redis_started_at) * 1000.0
    else:
        cooldowns, cooldown_ttls, lru_scores, latency_avgs, cost_totals = (
            {},
            {},
            {},
            {},
            {},
        )

    cooldowns_map = cast(dict[str, str | None], cooldowns)

    key_details: list[PoolKeyDetail] = []
    serialize_started_at = time.perf_counter()
    for k in keys:
        kid = str(k.id)
        cd_reason = cooldowns_map.get(kid)
        cd_ttl = cooldown_ttls.get(kid) if cd_reason else None
        health_score, any_circuit_open = _compute_health_aggregate(
            getattr(k, "health_by_format", None),
            getattr(k, "circuit_breaker_by_format", None),
        )
        cost_usage = int(cost_totals.get(kid, 0) or 0)
        cost_limit = pcfg.cost_limit_per_key_tokens if pcfg else None
        latency_avg_raw = latency_avgs.get(kid)
        latency_avg_ms = float(latency_avg_raw) if latency_avg_raw is not None else None
        account_state = resolve_pool_account_state(
            provider_type=provider_type,
            upstream_metadata=getattr(k, "upstream_metadata", None),
            oauth_invalid_reason=getattr(k, "oauth_invalid_reason", None),
        )
        (
            scheduling_status,
            scheduling_reason,
            scheduling_label,
            scheduling_reasons,
        ) = _build_pool_scheduling_state(
            is_active=bool(k.is_active),
            account_blocked=account_state.blocked,
            account_block_label=account_state.label,
            account_block_reason=account_state.reason,
            latency_avg_ms=latency_avg_ms,
            cooldown_reason=cd_reason,
            cooldown_ttl_seconds=cd_ttl,
            circuit_breaker_open=any_circuit_open,
            cost_window_usage=cost_usage,
            cost_limit=cost_limit,
            cost_soft_threshold_percent=(
                pcfg.cost_soft_threshold_percent if pcfg else 80
            ),
            health_score=health_score,
        )

        raw_allowed_models = getattr(k, "allowed_models", None)
        allowed_models = (
            [str(item) for item in raw_allowed_models]
            if isinstance(raw_allowed_models, list)
            else None
        )
        raw_locked_models = getattr(k, "locked_models", None)
        locked_models = (
            [str(item) for item in raw_locked_models]
            if isinstance(raw_locked_models, list)
            else None
        )
        raw_include_patterns = getattr(k, "model_include_patterns", None)
        include_patterns = (
            [str(item) for item in raw_include_patterns]
            if isinstance(raw_include_patterns, list)
            else None
        )
        raw_exclude_patterns = getattr(k, "model_exclude_patterns", None)
        exclude_patterns = (
            [str(item) for item in raw_exclude_patterns]
            if isinstance(raw_exclude_patterns, list)
            else None
        )
        capabilities = (
            {str(name): bool(enabled) for name, enabled in k.capabilities.items()}
            if isinstance(getattr(k, "capabilities", None), dict)
            else None
        )
        rate_multipliers: dict[str, float] | None = None
        if isinstance(getattr(k, "rate_multipliers", None), dict):
            converted: dict[str, float] = {}
            for fmt, raw_val in k.rate_multipliers.items():
                num_val = _to_float(raw_val)
                if num_val is None:
                    continue
                converted[str(fmt)] = num_val
            rate_multipliers = converted or None
        api_formats = (
            [str(fmt) for fmt in getattr(k, "api_formats", []) if isinstance(fmt, str)]
            if isinstance(getattr(k, "api_formats", None), list)
            else []
        )
        key_request_count = int(getattr(k, "request_count", 0) or 0)
        key_total_tokens = int(getattr(k, "total_tokens", 0) or 0)
        key_total_cost_usd = _serialize_money(getattr(k, "total_cost_usd", 0.0))
        key_last_used_at = getattr(k, "last_used_at", None)
        oauth_auth_config = _extract_oauth_auth_config(k)

        key_details.append(
            PoolKeyDetail(
                key_id=kid,
                key_name=str(getattr(k, "name", "") or ""),
                is_active=bool(k.is_active),
                auth_type=str(getattr(k, "auth_type", "api_key") or "api_key"),
                oauth_expires_at=_derive_oauth_expires_at(
                    k, auth_config=oauth_auth_config
                ),
                oauth_invalid_at=(
                    int(k.oauth_invalid_at.timestamp())
                    if getattr(k, "oauth_invalid_at", None)
                    else None
                ),
                oauth_invalid_reason=getattr(k, "oauth_invalid_reason", None),
                oauth_plan_type=_derive_oauth_plan_type(
                    k, provider_type, auth_config=oauth_auth_config
                ),
                oauth_account_id=_derive_oauth_account_id(oauth_auth_config),
                oauth_account_name=_derive_oauth_account_name(oauth_auth_config),
                oauth_account_user_id=_derive_oauth_account_user_id(oauth_auth_config),
                oauth_organizations=_derive_oauth_organizations(oauth_auth_config),
                quota_updated_at=_extract_quota_updated_at(
                    provider_type,
                    getattr(k, "upstream_metadata", None),
                ),
                health_score=health_score,
                circuit_breaker_open=any_circuit_open,
                api_formats=api_formats,
                rate_multipliers=rate_multipliers,
                internal_priority=int(getattr(k, "internal_priority", 50) or 50),
                rpm_limit=getattr(k, "rpm_limit", None),
                cache_ttl_minutes=(
                    v if (v := getattr(k, "cache_ttl_minutes", None)) is not None else 5
                ),
                max_probe_interval_minutes=(
                    v
                    if (v := getattr(k, "max_probe_interval_minutes", None)) is not None
                    else 32
                ),
                note=getattr(k, "note", None),
                allowed_models=allowed_models,
                capabilities=capabilities,
                auto_fetch_models=bool(getattr(k, "auto_fetch_models", False)),
                locked_models=locked_models,
                model_include_patterns=include_patterns,
                model_exclude_patterns=exclude_patterns,
                proxy=_mask_proxy_password(getattr(k, "proxy", None)),
                fingerprint=(
                    getattr(k, "fingerprint", None)
                    if isinstance(getattr(k, "fingerprint", None), dict)
                    else None
                ),
                account_quota=_build_account_quota(
                    provider_type,
                    getattr(k, "upstream_metadata", None),
                ),
                cooldown_reason=cd_reason,
                cooldown_ttl_seconds=cd_ttl,
                cost_window_usage=cost_usage,
                cost_limit=cost_limit,
                request_count=key_request_count,
                total_tokens=key_total_tokens,
                total_cost_usd=key_total_cost_usd,
                sticky_sessions=sticky_counts.get(kid, 0),
                lru_score=lru_scores.get(kid),
                created_at=(
                    k.created_at.isoformat() if getattr(k, "created_at", None) else None
                ),
                last_used_at=(
                    key_last_used_at.isoformat() if key_last_used_at else None
                ),
                scheduling_status=scheduling_status,
                scheduling_reason=scheduling_reason,
                scheduling_label=scheduling_label,
                scheduling_reasons=scheduling_reasons,
            )
        )
    serialize_ms = (time.perf_counter() - serialize_started_at) * 1000.0
    return key_details, redis_state_ms, serialize_ms


_DEFAULT_POOL_KEY_SCAN_LIMIT = 5000
_RESOLVE_SELECTION_SCAN_LIMIT = 10000


async def _resolve_filtered_pool_key_details(
    *,
    query: Any,
    pid: str,
    provider_type: str,
    pcfg: Any,
    search: str,
    quick_selectors: list[str],
    search_scope: str,
    require_cooldown: bool,
    max_scan: int = _DEFAULT_POOL_KEY_SCAN_LIMIT,
) -> tuple[list[PoolKeyDetail], float, float, float]:
    keys_query_started_at = time.perf_counter()
    ordered = _apply_pool_key_order(query)
    keys = ordered.limit(max_scan).all() if max_scan > 0 else ordered.all()
    keys_query_ms = (time.perf_counter() - keys_query_started_at) * 1000.0
    key_details, redis_state_ms, serialize_ms = await _serialize_pool_key_details(
        keys=keys,
        pid=pid,
        provider_type=provider_type,
        pcfg=pcfg,
    )
    filtered_details = _filter_pool_key_details(
        key_details,
        search=search,
        quick_selectors=quick_selectors,
        search_scope=search_scope,
        require_cooldown=require_cooldown,
    )
    return filtered_details, keys_query_ms, redis_state_ms, serialize_ms


@dataclass
class AdminListPoolKeysAdapter(AdminApiAdapter):
    provider_id: str = ""
    page: int = 1
    page_size: int = 50
    search: str = ""
    status: str = "all"
    quick_selectors: list[str] = field(default_factory=list)
    search_scope: str = "name"

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        started_at = time.perf_counter()
        count_query_ms = 0.0
        keys_query_ms = 0.0
        redis_state_ms = 0.0
        serialize_ms = 0.0

        db = context.db
        provider = db.query(Provider).filter(Provider.id == self.provider_id).first()
        if not provider:
            raise NotFoundException("Provider not found", "provider")

        pcfg = parse_pool_config(getattr(provider, "config", None))
        pid = str(provider.id)
        provider_type = str(getattr(provider, "provider_type", "custom") or "custom")
        normalized_quick_selectors = _normalize_pool_quick_selectors(
            self.quick_selectors
        )
        normalized_search_scope = _normalize_pool_search_scope(self.search_scope)

        q = _build_pool_keys_base_query(db, pid)
        if self.search and normalized_search_scope != _FULL_SEARCH_SCOPE:
            escaped = (
                self.search.replace("\\", "\\\\")
                .replace("%", "\\%")
                .replace("_", "\\_")
            )
            q = q.filter(ProviderAPIKey.name.ilike(f"%{escaped}%"))

        if self.status == "active":
            q = q.filter(ProviderAPIKey.is_active.is_(True))
        elif self.status == "inactive":
            q = q.filter(ProviderAPIKey.is_active.is_(False))

        total = 0
        if (
            normalized_quick_selectors
            or self.status == "cooldown"
            or (bool(self.search) and normalized_search_scope == _FULL_SEARCH_SCOPE)
        ):
            (
                filtered_details,
                keys_query_ms,
                redis_state_ms,
                serialize_ms,
            ) = await _resolve_filtered_pool_key_details(
                query=q,
                pid=pid,
                provider_type=provider_type,
                pcfg=pcfg,
                search=self.search,
                quick_selectors=normalized_quick_selectors,
                search_scope=normalized_search_scope,
                require_cooldown=self.status == "cooldown",
            )
            total = len(filtered_details)
            offset = (self.page - 1) * self.page_size
            key_details = filtered_details[offset : offset + self.page_size]
        else:
            count_query_started_at = time.perf_counter()
            total = int(q.with_entities(func.count(ProviderAPIKey.id)).scalar() or 0)
            count_query_ms = (time.perf_counter() - count_query_started_at) * 1000.0
            offset = (self.page - 1) * self.page_size
            keys_query_started_at = time.perf_counter()
            keys = _apply_pool_key_order(q).offset(offset).limit(self.page_size).all()
            keys_query_ms = (time.perf_counter() - keys_query_started_at) * 1000.0
            (
                key_details,
                extra_redis_ms,
                serialize_ms,
            ) = await _serialize_pool_key_details(
                keys=keys,
                pid=pid,
                provider_type=provider_type,
                pcfg=pcfg,
            )
            redis_state_ms += extra_redis_ms

        total_ms = (time.perf_counter() - started_at) * 1000.0
        logger.info(
            "[POOL_KEYS_TIMING] provider={} page={} page_size={} status={} search={} total={} count_ms={:.2f} fetch_ms={:.2f} redis_ms={:.2f} serialize_ms={:.2f} total_ms={:.2f}",
            pid[:8],
            self.page,
            self.page_size,
            self.status,
            bool(self.search),
            total,
            count_query_ms,
            keys_query_ms,
            redis_state_ms,
            serialize_ms,
            total_ms,
        )

        return PoolKeysPageResponse(
            total=total,
            page=self.page,
            page_size=self.page_size,
            keys=key_details,
        )


@dataclass
class AdminResolvePoolKeySelectionAdapter(AdminApiAdapter):
    provider_id: str = ""
    body: PoolKeySelectionRequest = field(default_factory=PoolKeySelectionRequest)

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        db = context.db
        provider = db.query(Provider).filter(Provider.id == self.provider_id).first()
        if not provider:
            raise NotFoundException("Provider not found", "provider")

        pcfg = parse_pool_config(getattr(provider, "config", None))
        pid = str(provider.id)
        provider_type = str(getattr(provider, "provider_type", "custom") or "custom")
        q = _build_pool_keys_base_query(db, pid)

        filtered_details, _, _, _ = await _resolve_filtered_pool_key_details(
            query=q,
            pid=pid,
            provider_type=provider_type,
            pcfg=pcfg,
            search=self.body.search,
            quick_selectors=_normalize_pool_quick_selectors(self.body.quick_selectors),
            search_scope=_FULL_SEARCH_SCOPE,
            require_cooldown=False,
            max_scan=_RESOLVE_SELECTION_SCAN_LIMIT,
        )

        return PoolKeySelectionResponse(
            total=len(filtered_details),
            items=[
                PoolKeySelectionItem(
                    key_id=detail.key_id,
                    key_name=detail.key_name,
                    auth_type=detail.auth_type,
                )
                for detail in filtered_details
            ],
        )


@dataclass
class AdminBatchImportKeysAdapter(AdminApiAdapter):
    provider_id: str = ""
    body: BatchImportRequest = field(
        default_factory=lambda: BatchImportRequest(keys=[])
    )

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        db = context.db
        provider = db.query(Provider).filter(Provider.id == self.provider_id).first()
        if not provider:
            raise NotFoundException("Provider not found", "provider")
        key_proxy: dict[str, Any] | None = None
        if self.body.proxy_node_id and self.body.proxy_node_id.strip():
            key_proxy = {"node_id": self.body.proxy_node_id.strip(), "enabled": True}

        imported = 0
        skipped = 0
        errors: list[BatchImportError] = []
        now = datetime.now(timezone.utc)

        for idx, item in enumerate(self.body.keys):
            if not item.api_key.strip():
                errors.append(BatchImportError(index=idx, reason="api_key is empty"))
                continue

            try:
                encrypted_key = crypto_service.encrypt(item.api_key)
                new_key_id = str(uuid.uuid4())
                provider_api_key_cls = cast(Any, ProviderAPIKey)
                new_key = provider_api_key_cls(
                    id=new_key_id,
                    provider_id=self.provider_id,
                    name=item.name or f"imported-{idx}",
                    api_key=encrypted_key,
                    auth_type=item.auth_type or "api_key",
                    proxy=key_proxy,
                    fingerprint=generate_fingerprint(seed=new_key_id),
                    is_active=True,
                    created_at=now,
                    updated_at=now,
                )
                db.add(new_key)
                imported += 1
            except Exception as exc:
                logger.warning("batch import key #{} failed: {}", idx, exc)
                errors.append(BatchImportError(index=idx, reason=str(exc)))

        if imported > 0:
            try:
                db.commit()
            except Exception as exc:
                db.rollback()
                logger.error("batch import commit failed: {}", exc)
                return BatchImportResponse(
                    imported=0,
                    skipped=skipped,
                    errors=[BatchImportError(index=-1, reason=f"commit failed: {exc}")],
                )

        admin_name = context.user.username if context.user else "admin"
        logger.info(
            "Pool batch import by {}: provider={}, imported={}, skipped={}, errors={}",
            admin_name,
            self.provider_id[:8],
            imported,
            skipped,
            len(errors),
        )

        return BatchImportResponse(imported=imported, skipped=skipped, errors=errors)


@dataclass
class AdminBatchActionKeysAdapter(AdminApiAdapter):
    provider_id: str = ""
    body: BatchActionRequest = field(
        default_factory=lambda: BatchActionRequest(key_ids=[], action="")
    )

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        from fastapi import HTTPException

        if self.body.action not in ALLOWED_ACTIONS:
            raise HTTPException(
                status_code=400,
                detail=(
                    f"Invalid action: {self.body.action}. "
                    f"Allowed: {', '.join(sorted(ALLOWED_ACTIONS))}"
                ),
            )

        if self.body.action == "set_proxy":
            if not isinstance(self.body.payload, dict) or not self.body.payload:
                raise HTTPException(
                    status_code=400,
                    detail="set_proxy action requires a non-empty payload with proxy config",
                )

        db = context.db
        provider = db.query(Provider).filter(Provider.id == self.provider_id).first()
        if not provider:
            raise NotFoundException("Provider not found", "provider")

        pid = str(provider.id)
        affected = 0

        if self.body.action == "delete":
            from src.services.provider_keys.batch_delete_task import submit_batch_delete

            key_ids = list(dict.fromkeys(self.body.key_ids))
            task_id = await submit_batch_delete(pid, key_ids)
            admin_name = context.user.username if context.user else "admin"
            logger.info(
                "Pool batch delete submitted by {}: provider={}, keys={}, task_id={}",
                admin_name,
                pid[:8],
                len(key_ids),
                task_id,
            )
            return BatchActionResponse(
                affected=0,
                message=f"delete task submitted ({len(key_ids)} keys)",
                task_id=task_id,
            )
        else:
            keys = (
                db.query(ProviderAPIKey)
                .filter(
                    ProviderAPIKey.provider_id == pid,
                    ProviderAPIKey.id.in_(self.body.key_ids),
                )
                .all()
            )

            for key in keys:
                kid = str(key.id)
                mutable_key = cast(Any, key)

                if self.body.action == "enable":
                    mutable_key.is_active = True
                    affected += 1

                elif self.body.action == "disable":
                    mutable_key.is_active = False
                    affected += 1

                elif self.body.action == "clear_cooldown":
                    await pool_redis.clear_cooldown(pid, kid)
                    affected += 1

                elif self.body.action == "reset_cost":
                    await pool_redis.clear_cost(pid, kid)
                    affected += 1

                elif self.body.action == "clear_proxy":
                    mutable_key.proxy = None
                    affected += 1

                elif self.body.action == "set_proxy":
                    mutable_key.proxy = self.body.payload
                    affected += 1

                elif self.body.action == "regenerate_fingerprint":
                    mutable_key.fingerprint = generate_fingerprint(seed=None)
                    affected += 1

            if self.body.action in {
                "enable",
                "disable",
                "regenerate_fingerprint",
                "clear_proxy",
                "set_proxy",
            }:
                try:
                    db.commit()
                except Exception as exc:
                    db.rollback()
                    logger.error("batch action commit failed: {}", exc)
                    return BatchActionResponse(
                        affected=0, message=f"commit failed: {exc}"
                    )

            admin_name = context.user.username if context.user else "admin"
            affected_ids = [str(k.id)[:8] for k in keys]

        action_labels = {
            "enable": "enabled",
            "disable": "disabled",
            "delete": "deleted",
            "clear_cooldown": "cooldown cleared",
            "reset_cost": "cost reset",
            "regenerate_fingerprint": "fingerprint regenerated",
            "clear_proxy": "proxy cleared",
            "set_proxy": "proxy set",
        }
        logger.info(
            "Pool batch action by {}: provider={}, action={}, affected={}, key_ids={}",
            admin_name,
            self.provider_id[:8],
            self.body.action,
            affected,
            affected_ids,
        )

        return BatchActionResponse(
            affected=affected,
            message=f"{affected} keys {action_labels.get(self.body.action, self.body.action)}",
        )


@dataclass
class AdminCleanupBannedKeysAdapter(AdminApiAdapter):
    provider_id: str = ""

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        db = context.db
        provider = db.query(Provider).filter(Provider.id == self.provider_id).first()
        if not provider:
            raise NotFoundException("Provider not found", "provider")

        pid = str(provider.id)
        provider_type = (
            str(getattr(provider, "provider_type", "") or "").strip().lower()
        )

        keys = db.query(ProviderAPIKey).filter(ProviderAPIKey.provider_id == pid).all()
        banned_keys = [key for key in keys if _is_known_banned_key(key, provider_type)]

        if not banned_keys:
            return BatchActionResponse(affected=0, message="未发现已知封号账号")

        banned_key_ids = [str(key.id) for key in banned_keys]
        try:
            cleanup_key_references(db, banned_key_ids)
            db.execute(
                sa_delete(ProviderAPIKey).where(
                    ProviderAPIKey.provider_id == pid,
                    ProviderAPIKey.id.in_(banned_key_ids),
                )
            )
            db.commit()
        except Exception as exc:
            db.rollback()
            logger.error("cleanup banned keys commit failed: {}", exc)
            return BatchActionResponse(affected=0, message=f"commit failed: {exc}")

        # 清理 Redis 中可能残留的状态，避免删除后仍有旧状态占用资源。
        cleanup_coros = []
        for kid in banned_key_ids:
            cleanup_coros.append(pool_redis.clear_cooldown(pid, kid))
            cleanup_coros.append(pool_redis.clear_cost(pid, kid))
        if cleanup_coros:
            await asyncio.gather(*cleanup_coros, return_exceptions=True)

        admin_name = context.user.username if context.user else "admin"
        logger.warning(
            "Pool cleanup banned by {}: provider={}, affected={}, key_ids={}",
            admin_name,
            self.provider_id[:8],
            len(banned_key_ids),
            [kid[:8] for kid in banned_key_ids],
        )

        return BatchActionResponse(
            affected=len(banned_key_ids),
            message=f"已清理 {len(banned_key_ids)} 个已知封号账号",
        )
