"""Pydantic schemas for Pool management API."""

from __future__ import annotations

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
# Paginated key list
# ---------------------------------------------------------------------------


class PoolKeyDetail(BaseModel):
    """Detailed status of a single pool key."""

    key_id: str
    key_name: str
    is_active: bool
    auth_type: str = "api_key"
    oauth_plan_type: str | None = None
    account_quota: str | None = None
    cooldown_reason: str | None = None
    cooldown_ttl_seconds: int | None = None
    cost_window_usage: int = 0
    cost_limit: int | None = None
    sticky_sessions: int = 0
    lru_score: float | None = None
    created_at: str | None = None
    last_used_at: str | None = None

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
    key_ids: list[str] = Field(..., max_length=500)
    action: str  # enable / disable / delete / clear_cooldown / reset_cost


class BatchActionResponse(BaseModel):
    affected: int = 0
    message: str = ""
