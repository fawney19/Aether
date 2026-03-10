"""Pydantic schemas for Pool management API."""

from __future__ import annotations

from typing import Any

from pydantic import BaseModel, ConfigDict, Field

# ---------------------------------------------------------------------------
# Overview
# ---------------------------------------------------------------------------


class PoolOverviewItem(BaseModel):
    """One Provider in the overview list."""

    provider_id: str
    provider_name: str
    provider_type: str = "custom"
    total_keys: int = 0
    active_keys: int = 0
    cooldown_count: int = 0
    pool_enabled: bool = False

    model_config = ConfigDict(from_attributes=True)


class PoolOverviewResponse(BaseModel):
    items: list[PoolOverviewItem] = Field(default_factory=list)


# ---------------------------------------------------------------------------
# Scheduling presets metadata
# ---------------------------------------------------------------------------


class PresetModeMetaResponse(BaseModel):
    value: str
    label: str


class PresetDimensionMetaResponse(BaseModel):
    name: str
    label: str
    description: str
    providers: list[str] = Field(default_factory=list)
    modes: list[PresetModeMetaResponse] | None = None
    default_mode: str | None = None
    mutex_group: str | None = None
    evidence_hint: str | None = None


# ---------------------------------------------------------------------------
# Paginated key list
# ---------------------------------------------------------------------------


class PoolSchedulingReason(BaseModel):
    """Structured scheduling reason for a key."""

    code: str
    label: str
    blocking: bool = False
    source: str = "pool"  # manual / pool / health / policy
    ttl_seconds: int | None = None
    detail: str | None = None


class PoolKeyDetail(BaseModel):
    """Detailed status of a single pool key."""

    key_id: str
    key_name: str
    is_active: bool
    auth_type: str = "api_key"
    oauth_expires_at: int | None = None
    oauth_invalid_at: int | None = None
    oauth_invalid_reason: str | None = None
    oauth_plan_type: str | None = None
    quota_updated_at: int | None = None
    # 健康度聚合字段（与 Provider Key 列表口径一致）
    health_score: float = 1.0
    circuit_breaker_open: bool = False
    # 编辑/权限/代理所需字段
    api_formats: list[str] = Field(default_factory=list)
    rate_multipliers: dict[str, float] | None = None
    internal_priority: int = 50
    rpm_limit: int | None = None
    cache_ttl_minutes: int = 5
    max_probe_interval_minutes: int = 32
    note: str | None = None
    allowed_models: list[str] | None = None
    capabilities: dict[str, bool] | None = None
    auto_fetch_models: bool = False
    locked_models: list[str] | None = None
    model_include_patterns: list[str] | None = None
    model_exclude_patterns: list[str] | None = None
    proxy: dict[str, Any] | None = None
    fingerprint: dict[str, Any] | None = None
    account_quota: str | None = None
    cooldown_reason: str | None = None
    cooldown_ttl_seconds: int | None = None
    cost_window_usage: int = 0
    cost_limit: int | None = None
    request_count: int = 0
    total_tokens: int = 0
    total_cost_usd: str = "0.00000000"
    sticky_sessions: int = 0
    lru_score: float | None = None
    created_at: str | None = None
    last_used_at: str | None = None
    scheduling_status: str = "available"  # available / degraded / blocked
    scheduling_reason: str = "available"
    scheduling_label: str = "可用"
    scheduling_reasons: list[PoolSchedulingReason] = Field(default_factory=list)

    model_config = ConfigDict(from_attributes=True)


class PoolKeysPageResponse(BaseModel):
    """Server-side paginated key list."""

    total: int
    page: int
    page_size: int
    keys: list[PoolKeyDetail] = Field(default_factory=list)


# ---------------------------------------------------------------------------
# Batch import
# ---------------------------------------------------------------------------


class PoolKeyImportItem(BaseModel):
    """Single key to import."""

    name: str
    api_key: str
    auth_type: str = "api_key"


class BatchImportRequest(BaseModel):
    keys: list[PoolKeyImportItem] = Field(..., max_length=500)
    proxy_node_id: str | None = Field(
        default=None,
        description="导入时绑定到账号的代理节点 ID（可选）",
    )


class BatchImportError(BaseModel):
    index: int
    reason: str


class BatchImportResponse(BaseModel):
    imported: int = 0
    skipped: int = 0
    errors: list[BatchImportError] = Field(default_factory=list)


# ---------------------------------------------------------------------------
# Batch action
# ---------------------------------------------------------------------------


class BatchActionRequest(BaseModel):
    key_ids: list[str] = Field(..., max_length=2000)
    action: str  # enable / disable / delete / clear_cooldown / reset_cost / regenerate_fingerprint / clear_proxy / set_proxy
    payload: dict[str, Any] | None = None


class BatchActionResponse(BaseModel):
    affected: int = 0
    message: str = ""
    task_id: str | None = None


class BatchDeleteTaskResponse(BaseModel):
    task_id: str
    status: str  # pending / running / completed / failed
    total: int = 0
    deleted: int = 0
    message: str = ""
