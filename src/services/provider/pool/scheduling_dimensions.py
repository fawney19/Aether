"""Pool scheduling dimension registry and evaluation helpers.

This module keeps pool scheduling scoring isolated from API layer code.
Callers build a :class:`PoolSchedulingSnapshot` and evaluate it against
registered dimensions to obtain a normalized summary.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Protocol

PoolDimensionStatus = str  # ok / degraded / blocked


@dataclass(frozen=True, slots=True)
class PoolSchedulingSnapshot:
    """Point-in-time scheduling inputs for one key."""

    is_active: bool
    cooldown_reason: str | None
    cooldown_ttl_seconds: int | None
    circuit_breaker_open: bool
    cost_window_usage: int
    cost_limit: int | None
    cost_soft_threshold_percent: int = 80
    health_score: float = 1.0
    latency_avg_ms: float | None = None
    account_blocked: bool = False
    account_block_label: str | None = None
    account_block_reason: str | None = None


@dataclass(frozen=True, slots=True)
class PoolSchedulingDimensionResult:
    """Evaluation output for one scheduling dimension."""

    code: str
    label: str
    status: PoolDimensionStatus = "ok"
    blocking: bool = False
    source: str = "pool"
    weight: int = 1
    score: float = 1.0
    detail: str | None = None
    ttl_seconds: int | None = None


@dataclass(frozen=True, slots=True)
class PoolSchedulingSummary:
    """Merged scheduling state across all dimensions."""

    status: str  # available / degraded / blocked
    reason: str
    label: str
    candidate_eligible: bool
    blocked_count: int
    degraded_count: int


class PoolSchedulingDimension(Protocol):
    """Dimension evaluator protocol."""

    code: str
    label: str
    source: str
    weight: int

    def evaluate(self, snapshot: PoolSchedulingSnapshot) -> PoolSchedulingDimensionResult:
        """Evaluate one dimension from snapshot."""


@dataclass(frozen=True, slots=True)
class _AccountStateDimension:
    code: str = "account_state"
    label: str = "账号状态"
    source: str = "policy"
    weight: int = 10

    def evaluate(self, snapshot: PoolSchedulingSnapshot) -> PoolSchedulingDimensionResult:
        if not snapshot.account_blocked:
            return PoolSchedulingDimensionResult(
                code=self.code,
                label=self.label,
                source=self.source,
                weight=self.weight,
                status="ok",
                score=1.0,
            )

        blocked_label = snapshot.account_block_label or "账号异常"
        if blocked_label == "账号封禁":
            blocked_code = "account_banned"
        elif blocked_label == "访问受限":
            blocked_code = "account_forbidden"
        else:
            blocked_code = "account_blocked"

        return PoolSchedulingDimensionResult(
            code=blocked_code,
            label=blocked_label,
            source=self.source,
            weight=self.weight,
            status="blocked",
            blocking=True,
            score=0.0,
            detail=snapshot.account_block_reason or blocked_label,
        )


@dataclass(frozen=True, slots=True)
class _ManualEnableDimension:
    code: str = "manual_disabled"
    label: str = "禁用"
    source: str = "manual"
    weight: int = 8

    def evaluate(self, snapshot: PoolSchedulingSnapshot) -> PoolSchedulingDimensionResult:
        if snapshot.is_active:
            return PoolSchedulingDimensionResult(
                code=self.code,
                label=self.label,
                source=self.source,
                weight=self.weight,
                status="ok",
                score=1.0,
            )
        return PoolSchedulingDimensionResult(
            code=self.code,
            label=self.label,
            source=self.source,
            weight=self.weight,
            status="blocked",
            blocking=True,
            score=0.0,
            detail="账号被手动禁用",
        )


@dataclass(frozen=True, slots=True)
class _CooldownDimension:
    code: str = "cooldown"
    label: str = "冷却中"
    source: str = "pool"
    weight: int = 7

    def evaluate(self, snapshot: PoolSchedulingSnapshot) -> PoolSchedulingDimensionResult:
        if not snapshot.cooldown_reason:
            return PoolSchedulingDimensionResult(
                code=self.code,
                label=self.label,
                source=self.source,
                weight=self.weight,
                status="ok",
                score=1.0,
            )
        return PoolSchedulingDimensionResult(
            code=self.code,
            label=self.label,
            source=self.source,
            weight=self.weight,
            status="blocked",
            blocking=True,
            score=0.0,
            detail=snapshot.cooldown_reason,
            ttl_seconds=snapshot.cooldown_ttl_seconds,
        )


@dataclass(frozen=True, slots=True)
class _CircuitBreakerDimension:
    code: str = "circuit_open"
    label: str = "熔断中"
    source: str = "health"
    weight: int = 6

    def evaluate(self, snapshot: PoolSchedulingSnapshot) -> PoolSchedulingDimensionResult:
        if not snapshot.circuit_breaker_open:
            return PoolSchedulingDimensionResult(
                code=self.code,
                label=self.label,
                source=self.source,
                weight=self.weight,
                status="ok",
                score=1.0,
            )
        return PoolSchedulingDimensionResult(
            code=self.code,
            label=self.label,
            source=self.source,
            weight=self.weight,
            status="blocked",
            blocking=True,
            score=0.0,
        )


@dataclass(frozen=True, slots=True)
class _CostDimension:
    code: str = "cost"
    label: str = "成本"
    source: str = "pool"
    weight: int = 5

    def evaluate(self, snapshot: PoolSchedulingSnapshot) -> PoolSchedulingDimensionResult:
        limit = snapshot.cost_limit
        usage = max(snapshot.cost_window_usage, 0)
        if limit is None or limit <= 0:
            return PoolSchedulingDimensionResult(
                code=self.code,
                label=self.label,
                source=self.source,
                weight=self.weight,
                status="ok",
                score=1.0,
                detail=f"{usage}/-",
            )

        ratio = usage / limit
        detail = f"{usage}/{limit}"
        if ratio >= 1.0:
            return PoolSchedulingDimensionResult(
                code="cost_exhausted",
                label="成本超限",
                source=self.source,
                weight=self.weight,
                status="blocked",
                blocking=True,
                score=0.0,
                detail=detail,
            )

        soft_threshold = max(1, min(snapshot.cost_soft_threshold_percent, 100))
        if ratio * 100 >= soft_threshold:
            return PoolSchedulingDimensionResult(
                code="cost_soft",
                label="成本接近上限",
                source=self.source,
                weight=self.weight,
                status="degraded",
                score=0.45,
                detail=detail,
            )

        if ratio >= 0.6:
            return PoolSchedulingDimensionResult(
                code=self.code,
                label=self.label,
                source=self.source,
                weight=self.weight,
                status="degraded",
                score=0.72,
                detail=detail,
            )

        return PoolSchedulingDimensionResult(
            code=self.code,
            label=self.label,
            source=self.source,
            weight=self.weight,
            status="ok",
            score=1.0,
            detail=detail,
        )


@dataclass(frozen=True, slots=True)
class _HealthDimension:
    code: str = "health"
    label: str = "健康度"
    source: str = "health"
    weight: int = 4

    def evaluate(self, snapshot: PoolSchedulingSnapshot) -> PoolSchedulingDimensionResult:
        score = max(0.0, min(snapshot.health_score, 1.0))
        detail = f"{score:.2f}"
        if score < 0.5:
            return PoolSchedulingDimensionResult(
                code="health_low",
                label="健康度过低",
                source=self.source,
                weight=self.weight,
                status="degraded",
                score=0.3,
                detail=detail,
            )
        if score < 0.8:
            return PoolSchedulingDimensionResult(
                code="health_degraded",
                label="健康度下降",
                source=self.source,
                weight=self.weight,
                status="degraded",
                score=0.65,
                detail=detail,
            )
        return PoolSchedulingDimensionResult(
            code=self.code,
            label=self.label,
            source=self.source,
            weight=self.weight,
            status="ok",
            score=1.0,
            detail=detail,
        )


@dataclass(frozen=True, slots=True)
class _LatencyDimension:
    code: str = "latency"
    label: str = "延迟"
    source: str = "runtime"
    weight: int = 3

    def evaluate(self, snapshot: PoolSchedulingSnapshot) -> PoolSchedulingDimensionResult:
        latency = snapshot.latency_avg_ms
        if latency is None:
            return PoolSchedulingDimensionResult(
                code=self.code,
                label=self.label,
                source=self.source,
                weight=self.weight,
                status="ok",
                score=1.0,
                detail="-",
            )

        value = max(float(latency), 0.0)
        detail = f"{value:.0f}ms"
        if value >= 3000:
            return PoolSchedulingDimensionResult(
                code="latency_high",
                label="延迟偏高",
                source=self.source,
                weight=self.weight,
                status="degraded",
                score=0.5,
                detail=detail,
            )
        if value >= 1200:
            return PoolSchedulingDimensionResult(
                code="latency_slow",
                label="延迟较慢",
                source=self.source,
                weight=self.weight,
                status="degraded",
                score=0.72,
                detail=detail,
            )
        return PoolSchedulingDimensionResult(
            code=self.code,
            label=self.label,
            source=self.source,
            weight=self.weight,
            status="ok",
            score=1.0,
            detail=detail,
        )


_POOL_DIMENSION_REGISTRY: dict[str, PoolSchedulingDimension] = {}
_POOL_DIMENSION_ORDER: list[str] = []


def register_pool_scheduling_dimension(name: str, dimension: PoolSchedulingDimension) -> None:
    """Register a dimension evaluator by name."""
    normalized = name.strip()
    if not normalized:
        return
    if normalized not in _POOL_DIMENSION_ORDER:
        _POOL_DIMENSION_ORDER.append(normalized)
    _POOL_DIMENSION_REGISTRY[normalized] = dimension


def get_pool_scheduling_dimension(name: str) -> PoolSchedulingDimension | None:
    """Fetch a registered dimension evaluator."""
    return _POOL_DIMENSION_REGISTRY.get(name.strip())


def list_pool_scheduling_dimensions() -> tuple[str, ...]:
    """List registered dimension names in evaluation order."""
    return tuple(_POOL_DIMENSION_ORDER)


def evaluate_pool_scheduling_dimensions(
    snapshot: PoolSchedulingSnapshot,
    *,
    dimension_names: tuple[str, ...] | None = None,
) -> list[PoolSchedulingDimensionResult]:
    """Evaluate snapshot across all registered dimensions."""
    names = dimension_names or list_pool_scheduling_dimensions()
    results: list[PoolSchedulingDimensionResult] = []
    for name in names:
        dimension = get_pool_scheduling_dimension(name)
        if dimension is None:
            continue
        results.append(dimension.evaluate(snapshot))
    return results


def summarize_pool_scheduling_dimensions(
    dimensions: list[PoolSchedulingDimensionResult],
) -> PoolSchedulingSummary:
    """Summarize dimension outputs into a unified scheduling state."""
    if not dimensions:
        return PoolSchedulingSummary(
            status="available",
            reason="available",
            label="可用",
            candidate_eligible=True,
            blocked_count=0,
            degraded_count=0,
        )

    blocked = [item for item in dimensions if item.status == "blocked" or item.blocking]
    degraded = [item for item in dimensions if item.status == "degraded"]

    if blocked:
        primary = blocked[0]
        return PoolSchedulingSummary(
            status="blocked",
            reason=primary.code,
            label=primary.label,
            candidate_eligible=False,
            blocked_count=len(blocked),
            degraded_count=len(degraded),
        )

    if degraded:
        primary = degraded[0]
        return PoolSchedulingSummary(
            status="degraded",
            reason=primary.code,
            label=primary.label,
            candidate_eligible=True,
            blocked_count=0,
            degraded_count=len(degraded),
        )

    return PoolSchedulingSummary(
        status="available",
        reason="available",
        label="可用",
        candidate_eligible=True,
        blocked_count=0,
        degraded_count=0,
    )


def _register_default_dimensions() -> None:
    register_pool_scheduling_dimension("account_state", _AccountStateDimension())
    register_pool_scheduling_dimension("manual", _ManualEnableDimension())
    register_pool_scheduling_dimension("cooldown", _CooldownDimension())
    register_pool_scheduling_dimension("circuit", _CircuitBreakerDimension())
    register_pool_scheduling_dimension("cost", _CostDimension())
    register_pool_scheduling_dimension("latency", _LatencyDimension())
    register_pool_scheduling_dimension("health", _HealthDimension())


_register_default_dimensions()
