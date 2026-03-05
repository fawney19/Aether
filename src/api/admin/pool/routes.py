"""Pool management admin API routes.

Provides endpoints for managing account pools at scale:
- Overview of all pool-enabled providers
- Paginated key listing with search/filter
- Batch import / batch actions
"""

from __future__ import annotations

import asyncio
import json
import uuid
from dataclasses import dataclass, field
from datetime import datetime, timezone
from typing import Any

from fastapi import APIRouter, Depends, Query, Request
from sqlalchemy import func
from sqlalchemy.orm import Session

from src.api.base.admin_adapter import AdminApiAdapter
from src.api.base.context import ApiRequestContext
from src.api.base.pipeline import ApiRequestPipeline
from src.core.crypto import crypto_service
from src.core.exceptions import NotFoundException
from src.core.logger import logger
from src.database import get_db
from src.models.database import Provider, ProviderAPIKey, Usage
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

from .schemas import (
    BatchActionRequest,
    BatchActionResponse,
    BatchImportError,
    BatchImportRequest,
    BatchImportResponse,
    PoolKeyDetail,
    PoolKeysPageResponse,
    PoolOverviewItem,
    PoolOverviewResponse,
    PoolSchedulingReason,
    PresetDimensionMetaResponse,
    PresetModeMetaResponse,
)

router = APIRouter(prefix="/api/admin/pool", tags=["pool-management"])
pipeline = ApiRequestPipeline()


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
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


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
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


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
    db: Session = Depends(get_db),
) -> PoolKeysPageResponse:
    """Server-side paginated account list for a pool-enabled provider."""
    adapter = AdminListPoolKeysAdapter(
        provider_id=provider_id,
        page=page,
        page_size=page_size,
        search=search,
        status=status,
    )
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


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
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


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
}

_COOLDOWN_REASON_LABELS: dict[str, str] = {
    "rate_limited_429": "429 限流",
    "forbidden_403": "403 禁止",
    "overloaded_529": "529 过载",
    "auth_failed_401": "401 认证失败",
    "payment_required_402": "402 欠费",
    "server_error_500": "500 错误",
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


def _is_known_banned_key(key: ProviderAPIKey, provider_type: str) -> bool:
    from src.services.provider.pool.account_state import resolve_pool_account_state

    state = resolve_pool_account_state(
        provider_type=provider_type,
        upstream_metadata=getattr(key, "upstream_metadata", None),
        oauth_invalid_reason=getattr(key, "oauth_invalid_reason", None),
    )
    return state.blocked


def _format_percent(value: float) -> str:
    clamped = max(0.0, min(value, 100.0))
    return f"{clamped:.1f}%"


def _format_quota_value(value: float) -> str:
    rounded = round(value)
    if abs(value - rounded) < 1e-6:
        return str(rounded)
    return f"{value:.1f}"


def _format_reset_after(seconds_raw: Any) -> str | None:
    seconds = _to_float(seconds_raw)
    if seconds is None:
        return None

    total_seconds = int(seconds)
    if total_seconds <= 0:
        return "已重置"

    days = total_seconds // 86400
    hours = (total_seconds % 86400) // 3600
    minutes = (total_seconds % 3600) // 60

    if days > 0:
        return f"{days}天{hours}小时后重置"
    if hours > 0:
        return f"{hours}小时{minutes}分钟后重置"
    if minutes > 0:
        return f"{minutes}分钟后重置"
    return "即将重置"


def _build_codex_account_quota(upstream_metadata: dict[str, Any]) -> str | None:
    codex = upstream_metadata.get("codex")
    if not isinstance(codex, dict):
        return None

    parts: list[str] = []

    primary_used = _to_float(codex.get("primary_used_percent"))
    if primary_used is not None:
        part = f"周剩余 {_format_percent(100.0 - primary_used)}"
        reset_text = _format_reset_after(codex.get("primary_reset_seconds"))
        if reset_text:
            part = f"{part} ({reset_text})"
        parts.append(part)

    secondary_used = _to_float(codex.get("secondary_used_percent"))
    if secondary_used is not None:
        part = f"5H剩余 {_format_percent(100.0 - secondary_used)}"
        reset_text = _format_reset_after(codex.get("secondary_reset_seconds"))
        if reset_text:
            part = f"{part} ({reset_text})"
        parts.append(part)

    if parts:
        return " | ".join(parts)

    has_credits = codex.get("has_credits")
    credits_balance = _to_float(codex.get("credits_balance"))
    if has_credits is True and credits_balance is not None:
        return f"积分 {credits_balance:.2f}"
    if has_credits is True:
        return "有积分"
    return None


def _build_kiro_account_quota(upstream_metadata: dict[str, Any]) -> str | None:
    kiro = upstream_metadata.get("kiro")
    if not isinstance(kiro, dict):
        return None

    if kiro.get("is_banned") is True:
        return "账号已封禁"

    usage_percentage = _to_float(kiro.get("usage_percentage"))
    if usage_percentage is not None:
        remaining = 100.0 - usage_percentage
        current_usage = _to_float(kiro.get("current_usage"))
        usage_limit = _to_float(kiro.get("usage_limit"))
        if current_usage is not None and usage_limit is not None and usage_limit > 0:
            return (
                f"剩余 {_format_percent(remaining)} "
                f"({_format_quota_value(current_usage)}/{_format_quota_value(usage_limit)})"
            )
        return f"剩余 {_format_percent(remaining)}"

    remaining = _to_float(kiro.get("remaining"))
    usage_limit = _to_float(kiro.get("usage_limit"))
    if remaining is not None and usage_limit is not None and usage_limit > 0:
        return f"剩余 {_format_quota_value(remaining)}/{_format_quota_value(usage_limit)}"
    return None


def _build_antigravity_account_quota(upstream_metadata: dict[str, Any]) -> str | None:
    antigravity = upstream_metadata.get("antigravity")
    if not isinstance(antigravity, dict):
        return None

    if antigravity.get("is_forbidden") is True:
        return "访问受限"

    quota_by_model = antigravity.get("quota_by_model")
    if not isinstance(quota_by_model, dict) or not quota_by_model:
        return None

    remaining_list: list[float] = []
    for raw_info in quota_by_model.values():
        if not isinstance(raw_info, dict):
            continue

        used_percent = _to_float(raw_info.get("used_percent"))
        if used_percent is None:
            remaining_fraction = _to_float(raw_info.get("remaining_fraction"))
            if remaining_fraction is not None:
                used_percent = (1.0 - remaining_fraction) * 100.0

        if used_percent is None:
            continue

        remaining = max(0.0, min(100.0 - used_percent, 100.0))
        remaining_list.append(remaining)

    if not remaining_list:
        return None

    min_remaining = min(remaining_list)
    if len(remaining_list) == 1:
        return f"剩余 {_format_percent(min_remaining)}"
    return f"最低剩余 {_format_percent(min_remaining)} ({len(remaining_list)} 模型)"


def _build_account_quota(provider_type: str, upstream_metadata: Any) -> str | None:
    if not isinstance(upstream_metadata, dict):
        return None

    normalized_type = provider_type.strip().lower()
    if normalized_type == "codex":
        return _build_codex_account_quota(upstream_metadata)
    if normalized_type == "kiro":
        return _build_kiro_account_quota(upstream_metadata)
    if normalized_type == "antigravity":
        return _build_antigravity_account_quota(upstream_metadata)
    return None


def _extract_quota_updated_at(provider_type: str, upstream_metadata: Any) -> int | None:
    if not isinstance(upstream_metadata, dict):
        return None

    normalized_type = provider_type.strip().lower()
    if normalized_type == "codex":
        source = upstream_metadata.get("codex")
    elif normalized_type == "antigravity":
        source = upstream_metadata.get("antigravity")
    elif normalized_type == "kiro":
        source = upstream_metadata.get("kiro")
    else:
        return None

    if not isinstance(source, dict):
        return None

    updated_at = _to_float(source.get("updated_at"))
    if updated_at is None or updated_at <= 0:
        return None

    # 部分上游可能返回毫秒时间戳，统一转换为秒
    if updated_at > 1_000_000_000_000:
        updated_at /= 1000

    return int(updated_at)


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

    cfg = auth_config if isinstance(auth_config, dict) else _extract_oauth_auth_config(key)
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
    persisted = _normalize_oauth_plan_type(getattr(key, "oauth_plan_type", None), provider_type)
    if persisted:
        return persisted

    if str(getattr(key, "auth_type", "") or "").strip().lower() != "oauth":
        return None

    # Fallback 1: encrypted auth_config (common for Codex/Antigravity)
    cfg = auth_config if isinstance(auth_config, dict) else _extract_oauth_auth_config(key)
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
        for plan_key in ("plan_type", "tier", "subscription_title", "subscription_plan"):
            normalized = _normalize_oauth_plan_type(source.get(plan_key), provider_type)
            if normalized:
                return normalized
    return None


def _compute_health_aggregate(
    health_by_format: Any, circuit_breaker_by_format: Any
) -> tuple[float, bool]:
    """从按格式健康数据聚合出列表展示字段。"""
    health_map = health_by_format if isinstance(health_by_format, dict) else {}
    circuit_map = circuit_breaker_by_format if isinstance(circuit_breaker_by_format, dict) else {}

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
        bool(item.get("open", False)) for item in circuit_map.values() if isinstance(item, dict)
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
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.post("/{provider_id}/keys/cleanup-banned", response_model=BatchActionResponse)
async def cleanup_banned_keys(
    provider_id: str,
    request: Request,
    db: Session = Depends(get_db),
) -> BatchActionResponse:
    """Delete known banned/suspended accounts for the provider."""
    adapter = AdminCleanupBannedKeysAdapter(provider_id=provider_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


# ---------------------------------------------------------------------------
# Adapters
# ---------------------------------------------------------------------------


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
                )
            )
        return items


class AdminPoolOverviewAdapter(AdminApiAdapter):
    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        db = context.db
        providers = (
            db.query(Provider)
            .filter(Provider.is_active.is_(True))
            .order_by(Provider.provider_priority.asc())
            .all()
        )

        # 先批量计算池化 Provider 的 Key 总数/启用数，避免每个 Provider 单独查询（N+1）。
        pool_provider_ids: list[str] = []
        pool_enabled_map: dict[str, bool] = {}
        for p in providers:
            pid = str(p.id)
            enabled = parse_pool_config(getattr(p, "config", None)) is not None
            pool_enabled_map[pid] = enabled
            if enabled:
                pool_provider_ids.append(pid)

        key_ids_by_provider: dict[str, list[str]] = {pid: [] for pid in pool_provider_ids}
        key_stats_by_provider: dict[str, dict[str, int]] = {
            pid: {"total": 0, "active": 0} for pid in pool_provider_ids
        }
        if pool_provider_ids:
            key_rows = (
                db.query(
                    ProviderAPIKey.provider_id,
                    ProviderAPIKey.id,
                    ProviderAPIKey.is_active,
                )
                .filter(ProviderAPIKey.provider_id.in_(pool_provider_ids))
                .all()
            )
            for provider_id, key_id, is_active in key_rows:
                pid = str(provider_id)
                kid = str(key_id)
                key_ids_by_provider.setdefault(pid, []).append(kid)
                stats = key_stats_by_provider.setdefault(pid, {"total": 0, "active": 0})
                stats["total"] += 1
                if is_active:
                    stats["active"] += 1

        # Redis 冷却状态并发获取，避免逐 Provider 串行等待。
        cooldown_count_by_provider: dict[str, int] = {}
        cooldown_targets = [
            (pid, key_ids) for pid, key_ids in key_ids_by_provider.items() if key_ids
        ]
        if cooldown_targets:
            cooldown_results = await asyncio.gather(
                *[
                    pool_redis.batch_get_cooldowns(pid, key_ids)
                    for pid, key_ids in cooldown_targets
                ],
                return_exceptions=True,
            )
            for (pid, _key_ids), result in zip(cooldown_targets, cooldown_results, strict=False):
                if isinstance(result, Exception):
                    logger.warning(
                        "池管理概览读取冷却状态失败",
                        extra={"provider_id": pid, "error": str(result)},
                    )
                    cooldown_count_by_provider[pid] = 0
                else:
                    cooldown_count_by_provider[pid] = sum(
                        1 for value in result.values() if value is not None
                    )

        items: list[PoolOverviewItem] = []
        for p in providers:
            pid = str(p.id)
            if not pool_enabled_map.get(pid, False):
                items.append(
                    PoolOverviewItem(
                        provider_id=pid,
                        provider_name=p.name,
                        provider_type=str(getattr(p, "provider_type", "custom") or "custom"),
                        pool_enabled=False,
                    )
                )
                continue

            key_stats = key_stats_by_provider.get(pid, {"total": 0, "active": 0})

            items.append(
                PoolOverviewItem(
                    provider_id=pid,
                    provider_name=p.name,
                    provider_type=str(getattr(p, "provider_type", "custom") or "custom"),
                    total_keys=key_stats["total"],
                    active_keys=key_stats["active"],
                    cooldown_count=cooldown_count_by_provider.get(pid, 0),
                    pool_enabled=True,
                )
            )

        return PoolOverviewResponse(items=items)


@dataclass
class AdminListPoolKeysAdapter(AdminApiAdapter):
    provider_id: str = ""
    page: int = 1
    page_size: int = 50
    search: str = ""
    status: str = "all"

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        db = context.db
        provider = db.query(Provider).filter(Provider.id == self.provider_id).first()
        if not provider:
            raise NotFoundException("Provider not found", "provider")

        pcfg = parse_pool_config(getattr(provider, "config", None))
        pid = str(provider.id)
        provider_type = str(getattr(provider, "provider_type", "custom") or "custom")

        # Base query
        q = db.query(ProviderAPIKey).filter(ProviderAPIKey.provider_id == pid)

        if self.search:
            escaped = self.search.replace("\\", "\\\\").replace("%", "\\%").replace("_", "\\_")
            q = q.filter(ProviderAPIKey.name.ilike(f"%{escaped}%"))

        if self.status == "active":
            q = q.filter(ProviderAPIKey.is_active.is_(True))
        elif self.status == "inactive":
            q = q.filter(ProviderAPIKey.is_active.is_(False))
        # "cooldown" filtering is done post-query (Redis state)

        total = 0

        # For cooldown filtering we need to fetch all, then filter, then paginate.
        # Limit scan range to avoid loading the entire table into memory.
        if self.status == "cooldown":
            _max_scan = 2000
            all_keys = q.order_by(ProviderAPIKey.created_at.desc()).limit(_max_scan).all()
            key_ids = [str(k.id) for k in all_keys]
            cooldowns = await pool_redis.batch_get_cooldowns(pid, key_ids) if key_ids else {}
            all_keys = [k for k in all_keys if cooldowns.get(str(k.id)) is not None]
            total = len(all_keys)
            offset = (self.page - 1) * self.page_size
            keys = all_keys[offset : offset + self.page_size]
        else:
            total = int(q.with_entities(func.count(ProviderAPIKey.id)).scalar() or 0)
            offset = (self.page - 1) * self.page_size
            keys = (
                q.order_by(ProviderAPIKey.created_at.desc())
                .offset(offset)
                .limit(self.page_size)
                .all()
            )

        # Batch fetch Redis state (parallel where possible)
        key_ids = [str(k.id) for k in keys]
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
            (
                cooldowns,
                cooldown_ttls,
                lru_scores,
                latency_avgs,
                cost_totals,
                sticky_counts,
            ) = await asyncio.gather(
                pool_redis.batch_get_cooldowns(pid, key_ids),
                pool_redis.batch_get_cooldown_ttls(pid, key_ids),
                _lru_coro,
                _latency_coro,
                _cost_coro,
                pool_redis.batch_get_key_sticky_counts(pid, key_ids),
            )
        else:
            cooldowns, cooldown_ttls, lru_scores, latency_avgs, cost_totals, sticky_counts = (
                {},
                {},
                {},
                {},
                {},
                {},
            )

        usage_stats_by_key: dict[str, dict[str, Any]] = {}
        if key_ids:
            usage_rows = (
                db.query(
                    Usage.provider_api_key_id.label("key_id"),
                    func.count(Usage.id).label("request_count"),
                    func.coalesce(func.sum(Usage.total_tokens), 0).label("total_tokens"),
                    func.coalesce(func.sum(Usage.total_cost_usd), 0.0).label("total_cost_usd"),
                    func.max(Usage.created_at).label("last_used_at"),
                )
                .filter(
                    Usage.provider_id == pid,
                    Usage.provider_api_key_id.in_(key_ids),
                    Usage.status.notin_(["pending", "streaming"]),
                )
                .group_by(Usage.provider_api_key_id)
                .all()
            )
            usage_stats_by_key = {
                str(row.key_id): {
                    "request_count": int(row.request_count or 0),
                    "total_tokens": int(row.total_tokens or 0),
                    "total_cost_usd": float(row.total_cost_usd or 0.0),
                    "last_used_at": getattr(row, "last_used_at", None),
                }
                for row in usage_rows
                if getattr(row, "key_id", None)
            }

        key_details: list[PoolKeyDetail] = []
        for k in keys:
            kid = str(k.id)
            cd_reason = cooldowns.get(kid)
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
                cost_soft_threshold_percent=(pcfg.cost_soft_threshold_percent if pcfg else 80),
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
            key_usage_stats = usage_stats_by_key.get(kid, {})
            key_request_count = int(
                key_usage_stats.get("request_count") or getattr(k, "request_count", 0) or 0
            )
            key_total_tokens = int(key_usage_stats.get("total_tokens") or 0)
            key_total_cost_usd = float(key_usage_stats.get("total_cost_usd") or 0.0)
            key_last_used_at = getattr(k, "last_used_at", None) or key_usage_stats.get(
                "last_used_at"
            )
            oauth_auth_config = _extract_oauth_auth_config(k)

            key_details.append(
                PoolKeyDetail(
                    key_id=kid,
                    key_name=k.name or "",
                    is_active=bool(k.is_active),
                    auth_type=str(getattr(k, "auth_type", "api_key") or "api_key"),
                    oauth_expires_at=_derive_oauth_expires_at(k, auth_config=oauth_auth_config),
                    oauth_invalid_at=(
                        int(k.oauth_invalid_at.timestamp())
                        if getattr(k, "oauth_invalid_at", None)
                        else None
                    ),
                    oauth_invalid_reason=getattr(k, "oauth_invalid_reason", None),
                    oauth_plan_type=_derive_oauth_plan_type(
                        k, provider_type, auth_config=oauth_auth_config
                    ),
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
                    cache_ttl_minutes=int(getattr(k, "cache_ttl_minutes", 5) or 5),
                    max_probe_interval_minutes=int(
                        getattr(k, "max_probe_interval_minutes", 32) or 32
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
                    last_used_at=(key_last_used_at.isoformat() if key_last_used_at else None),
                    scheduling_status=scheduling_status,
                    scheduling_reason=scheduling_reason,
                    scheduling_label=scheduling_label,
                    scheduling_reasons=scheduling_reasons,
                )
            )

        return PoolKeysPageResponse(
            total=total,
            page=self.page,
            page_size=self.page_size,
            keys=key_details,
        )


@dataclass
class AdminBatchImportKeysAdapter(AdminApiAdapter):
    provider_id: str = ""
    body: BatchImportRequest = field(default_factory=lambda: BatchImportRequest(keys=[]))

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
                new_key = ProviderAPIKey(
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

        db = context.db
        provider = db.query(Provider).filter(Provider.id == self.provider_id).first()
        if not provider:
            raise NotFoundException("Provider not found", "provider")

        pid = str(provider.id)
        affected = 0

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

            if self.body.action == "enable":
                key.is_active = True
                affected += 1

            elif self.body.action == "disable":
                key.is_active = False
                affected += 1

            elif self.body.action == "delete":
                db.delete(key)
                affected += 1

            elif self.body.action == "clear_cooldown":
                await pool_redis.clear_cooldown(pid, kid)
                affected += 1

            elif self.body.action == "reset_cost":
                await pool_redis.clear_cost(pid, kid)
                affected += 1

            elif self.body.action == "regenerate_fingerprint":
                key.fingerprint = generate_fingerprint(seed=None)
                affected += 1

        if self.body.action in {"enable", "disable", "delete", "regenerate_fingerprint"}:
            try:
                db.commit()
            except Exception as exc:
                db.rollback()
                logger.error("batch action commit failed: {}", exc)
                return BatchActionResponse(affected=0, message=f"commit failed: {exc}")

        action_labels = {
            "enable": "enabled",
            "disable": "disabled",
            "delete": "deleted",
            "clear_cooldown": "cooldown cleared",
            "reset_cost": "cost reset",
            "regenerate_fingerprint": "fingerprint regenerated",
        }

        admin_name = context.user.username if context.user else "admin"
        affected_ids = [str(k.id)[:8] for k in keys]
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
        provider_type = str(getattr(provider, "provider_type", "") or "").strip().lower()

        keys = db.query(ProviderAPIKey).filter(ProviderAPIKey.provider_id == pid).all()
        banned_keys = [key for key in keys if _is_known_banned_key(key, provider_type)]

        if not banned_keys:
            return BatchActionResponse(affected=0, message="未发现已知封号账号")

        banned_key_ids = [str(key.id) for key in banned_keys]
        for key in banned_keys:
            db.delete(key)

        try:
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
