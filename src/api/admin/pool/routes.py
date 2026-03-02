"""Pool management admin API routes.

Provides endpoints for managing account pools at scale:
- Overview of all pool-enabled providers
- Paginated key listing with search/filter
- Batch import / batch actions
"""

from __future__ import annotations

import asyncio
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
from src.core.oauth_plan import extract_oauth_plan_type
from src.database import get_db
from src.models.database import Provider, ProviderAPIKey, Usage
from src.services.provider.pool import redis_ops as pool_redis
from src.services.provider.pool.config import parse_pool_config
from src.services.provider_keys.quota_cooldown import resolve_effective_cooldown_reason

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

ALLOWED_ACTIONS = {"enable", "disable", "delete", "clear_cooldown", "reset_cost"}


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


def _format_percent(value: float) -> str:
    clamped = max(0.0, min(value, 100.0))
    return f"{clamped:.1f}%"


def _format_quota_value(value: float) -> str:
    rounded = round(value)
    if abs(value - rounded) < 1e-6:
        return str(rounded)
    return f"{value:.1f}"


def _format_reset_time(seconds: int | float | None) -> str | None:
    if seconds is None or seconds <= 0:
        return None
    total = int(seconds)
    days = total // 86400
    hours = (total % 86400) // 3600
    minutes = (total % 3600) // 60
    if days > 0:
        return f"{days}天{hours}小时后重置"
    if hours > 0:
        return f"{hours}小时{minutes}分钟后重置"
    return f"{minutes}分钟后重置"


def _build_codex_account_quota(upstream_metadata: dict[str, Any]) -> str | None:
    codex = upstream_metadata.get("codex")
    if not isinstance(codex, dict):
        return None

    parts: list[str] = []

    primary_used = _to_float(codex.get("primary_used_percent"))
    if primary_used is not None:
        reset_text = _format_reset_time(codex.get("primary_reset_seconds"))
        suffix = f" ({reset_text})" if reset_text else ""
        parts.append(f"周剩余 {_format_percent(100.0 - primary_used)}{suffix}")

    secondary_used = _to_float(codex.get("secondary_used_percent"))
    if secondary_used is not None:
        reset_text = _format_reset_time(codex.get("secondary_reset_seconds"))
        suffix = f" ({reset_text})" if reset_text else ""
        parts.append(f"5H剩余 {_format_percent(100.0 - secondary_used)}{suffix}")

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


@router.post("/{provider_id}/keys/batch-action", response_model=BatchActionResponse)
async def batch_action_keys(
    provider_id: str,
    body: BatchActionRequest,
    request: Request,
    db: Session = Depends(get_db),
) -> BatchActionResponse:
    """Batch enable/disable/delete/clear_cooldown/reset_cost on pool keys."""
    adapter = AdminBatchActionKeysAdapter(provider_id=provider_id, body=body)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


# ---------------------------------------------------------------------------
# Adapters
# ---------------------------------------------------------------------------


class AdminPoolOverviewAdapter(AdminApiAdapter):
    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        db = context.db
        providers = (
            db.query(Provider)
            .filter(Provider.is_active.is_(True))
            .order_by(Provider.provider_priority.asc())
            .all()
        )

        items: list[PoolOverviewItem] = []
        for p in providers:
            pid = str(p.id)
            pcfg = parse_pool_config(getattr(p, "config", None))
            provider_type = str(getattr(p, "provider_type", "custom") or "custom")

            # Non-pool providers: skip Redis + key queries entirely.
            if pcfg is None:
                items.append(
                    PoolOverviewItem(
                        provider_id=pid,
                        provider_name=p.name,
                        provider_type=provider_type,
                        pool_enabled=False,
                    )
                )
                continue

            keys = db.query(ProviderAPIKey).filter(ProviderAPIKey.provider_id == pid).all()
            key_ids = [str(k.id) for k in keys]

            cooldown_count = 0
            if key_ids:
                cooldowns = await pool_redis.batch_get_cooldowns(pid, key_ids)
                cooldown_count = sum(
                    1
                    for k in keys
                    if resolve_effective_cooldown_reason(
                        provider_type=provider_type,
                        key=k,
                        redis_reason=cooldowns.get(str(k.id)),
                    )
                    is not None
                )

            items.append(
                PoolOverviewItem(
                    provider_id=pid,
                    provider_name=p.name,
                    provider_type=provider_type,
                    total_keys=len(keys),
                    active_keys=sum(1 for k in keys if k.is_active),
                    cooldown_count=cooldown_count,
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
        # "cooldown" filtering is done post-query (Redis + 配额元数据兜底)

        total = q.count()

        # For cooldown filtering we need to fetch all, then filter, then paginate.
        # Limit scan range to avoid loading the entire table into memory.
        if self.status == "cooldown":
            _max_scan = 2000
            all_keys = q.order_by(ProviderAPIKey.created_at.desc()).limit(_max_scan).all()
            key_ids = [str(k.id) for k in all_keys]
            cooldowns = await pool_redis.batch_get_cooldowns(pid, key_ids) if key_ids else {}
            all_keys = [
                k
                for k in all_keys
                if resolve_effective_cooldown_reason(
                    provider_type=provider_type,
                    key=k,
                    redis_reason=cooldowns.get(str(k.id)),
                )
                is not None
            ]
            total = len(all_keys)
            offset = (self.page - 1) * self.page_size
            keys = all_keys[offset : offset + self.page_size]
        else:
            offset = (self.page - 1) * self.page_size
            keys = (
                q.order_by(ProviderAPIKey.created_at.desc())
                .offset(offset)
                .limit(self.page_size)
                .all()
            )

        # Batch fetch Redis state (parallel where possible)
        key_ids = [str(k.id) for k in keys]
        # last_used_at 按 Usage 实时聚合，避免依赖 ProviderAPIKey 冗余字段
        last_used_map: dict[str, datetime] = {}
        if key_ids:
            usage_rows = (
                db.query(
                    Usage.provider_api_key_id.label("key_id"),
                    func.max(Usage.created_at).label("last_used_at"),
                )
                .filter(Usage.provider_api_key_id.in_(key_ids))
                .group_by(Usage.provider_api_key_id)
                .all()
            )
            last_used_map = {
                str(row.key_id): row.last_used_at
                for row in usage_rows
                if row.key_id is not None and row.last_used_at is not None
            }

        if key_ids:
            _lru_coro = (
                pool_redis.get_lru_scores(pid, key_ids)
                if pcfg and pcfg.lru_enabled
                else asyncio.sleep(0, result={})
            )
            _cost_coro = (
                pool_redis.batch_get_cost_totals(pid, key_ids, pcfg.cost_window_seconds)
                if pcfg
                else asyncio.sleep(0, result={})
            )
            cooldowns, cooldown_ttls, lru_scores, cost_totals = await asyncio.gather(
                pool_redis.batch_get_cooldowns(pid, key_ids),
                pool_redis.batch_get_cooldown_ttls(pid, key_ids),
                _lru_coro,
                _cost_coro,
            )
        else:
            cooldowns, cooldown_ttls, lru_scores, cost_totals = {}, {}, {}, {}

        # Sticky session count per key is expensive (SCAN+MGET per key).
        # Only compute when the page is small enough to avoid timeout.
        sticky_counts: dict[str, int] = {}
        if key_ids and len(key_ids) <= 30:
            counts = await asyncio.gather(
                *(pool_redis.get_key_sticky_count(pid, kid) for kid in key_ids)
            )
            sticky_counts = dict(zip(key_ids, counts))

        key_details: list[PoolKeyDetail] = []
        for k in keys:
            kid = str(k.id)
            upstream_metadata = getattr(k, "upstream_metadata", None)
            last_used_dt = last_used_map.get(kid)
            redis_cd_reason = cooldowns.get(kid)
            cd_reason = resolve_effective_cooldown_reason(
                provider_type=provider_type,
                key=k,
                redis_reason=redis_cd_reason,
            )
            cd_ttl = cooldown_ttls.get(kid) if redis_cd_reason else None

            key_details.append(
                PoolKeyDetail(
                    key_id=kid,
                    key_name=k.name or "",
                    is_active=bool(k.is_active),
                    auth_type=str(getattr(k, "auth_type", "api_key") or "api_key"),
                    oauth_plan_type=extract_oauth_plan_type(
                        getattr(k, "auth_config", None),
                        upstream_metadata=upstream_metadata,
                        provider_type=provider_type,
                    ),
                    account_quota=_build_account_quota(
                        provider_type,
                        upstream_metadata,
                    ),
                    cooldown_reason=cd_reason,
                    cooldown_ttl_seconds=cd_ttl,
                    cost_window_usage=cost_totals.get(kid, 0),
                    cost_limit=pcfg.cost_limit_per_key_tokens if pcfg else None,
                    sticky_sessions=sticky_counts.get(kid, 0),
                    lru_score=lru_scores.get(kid),
                    created_at=(
                        k.created_at.isoformat() if getattr(k, "created_at", None) else None
                    ),
                    last_used_at=(last_used_dt.isoformat() if last_used_dt else None),
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
                new_key = ProviderAPIKey(
                    id=str(uuid.uuid4()),
                    provider_id=self.provider_id,
                    name=item.name or f"imported-{idx}",
                    api_key=encrypted_key,
                    auth_type=item.auth_type or "api_key",
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

        if self.body.action in {"enable", "disable", "delete"}:
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
