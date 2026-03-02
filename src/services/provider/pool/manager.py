"""Account Pool Manager (provider-agnostic).

Stateless facade that coordinates pool operations for any Provider with
pool configuration enabled.  All state lives in Redis via :mod:`redis_ops`.

Usage::

    mgr = PoolManager(provider_id, pool_config)
    reordered = await mgr.reorder_candidates(session_uuid, candidates)
    # ... execute request ...
    await mgr.on_request_success(session_uuid=..., key_id=..., tokens_used=...)
"""

from __future__ import annotations

import asyncio
import random
from collections.abc import Callable
from typing import TYPE_CHECKING, Any, TypeVar

from src.core.logger import logger
from src.services.provider.pool import redis_ops
from src.services.provider.pool.config import PoolConfig
from src.services.provider.pool.trace import PoolCandidateTrace, PoolSchedulingTrace
from src.services.provider_keys.quota_cooldown import resolve_effective_cooldown_reason

if TYPE_CHECKING:
    from src.models.database import ProviderAPIKey
    from src.services.scheduling.schemas import ProviderCandidate


class PoolManager:
    """Coordinate pool-level scheduling for a single Provider."""

    __slots__ = ("provider_id", "config", "provider_type")

    def __init__(
        self,
        provider_id: str,
        config: PoolConfig,
        provider_type: str | None = None,
    ) -> None:
        self.provider_id = provider_id
        self.config = config
        self.provider_type = str(provider_type or "").strip().lower() or None

    # ------------------------------------------------------------------
    # Core scheduling: reorder candidate list for pool-aware selection
    # ------------------------------------------------------------------

    async def reorder_candidates(
        self,
        session_uuid: str | None,
        candidates: list[ProviderCandidate],
    ) -> list[ProviderCandidate]:
        """Reorder *candidates* according to pool rules.

        The returned list keeps the same elements but in a new order:

        1. **Sticky session hit** -- if the session is already bound to a key
           and that key appears in *candidates* and is not in cooldown, move it
           to position 0.
        2. **Filter** out keys in cooldown or cost-exhausted state (mark
           ``is_skipped``).
        3. **LRU sort** -- among remaining candidates at the same priority
           level, sort by least-recently-used.
        4. **Random tiebreak** -- among candidates with identical LRU score.

        Also builds a :class:`PoolSchedulingTrace` and attaches per-candidate
        trace data via ``_pool_extra_data`` / ``_pool_scheduling_trace``
        attributes on candidate objects.
        """
        if not candidates:
            return candidates

        pid = self.provider_id

        # Build trace
        trace = PoolSchedulingTrace(
            provider_id=pid,
            total_keys=len(candidates),
            session_uuid=session_uuid[:8] if session_uuid else None,
        )

        # --- Strategy: before_select ----------------------------------
        strategies = _get_active_strategies(self.config)
        key_ids = [str(c.key.id) for c in candidates]
        strategy_context: dict[str, Any] = {"session_uuid": session_uuid}
        for strategy in strategies:
            if hasattr(strategy, "on_before_select"):
                try:
                    filtered = strategy.on_before_select(
                        provider_id=pid,
                        key_ids=key_ids,
                        config=self.config,
                        context=strategy_context,
                    )
                    if filtered is not None:
                        key_ids = filtered
                except Exception:
                    logger.opt(exception=True).debug(
                        "Pool[{}]: strategy before_select failed", pid[:8]
                    )

        # --- 1. Sticky session ----------------------------------------
        sticky_key_id: str | None = None
        if session_uuid and self.config.sticky_session_ttl_seconds > 0:
            sticky_key_id = await redis_ops.get_sticky_binding(
                pid, session_uuid, self.config.sticky_session_ttl_seconds
            )

        # --- 2. Batch fetch pool state (parallel) ---------------------
        all_key_ids = [str(c.key.id) for c in candidates]

        # Fire independent Redis queries concurrently.
        _cooldown_coro = redis_ops.batch_get_cooldowns(pid, all_key_ids, include_ttl=True)
        _cost_coro = (
            redis_ops.batch_get_cost_totals(pid, all_key_ids, self.config.cost_window_seconds)
            if self.config.cost_limit_per_key_tokens is not None
            else None
        )
        _lru_coro = redis_ops.get_lru_scores(pid, all_key_ids) if self.config.lru_enabled else None

        # Gather all non-None coroutines in parallel.
        coros: list[Any] = [_cooldown_coro]
        _cost_idx = -1
        _lru_idx = -1
        if _cost_coro is not None:
            _cost_idx = len(coros)
            coros.append(_cost_coro)
        if _lru_coro is not None:
            _lru_idx = len(coros)
            coros.append(_lru_coro)

        gathered = await asyncio.gather(*coros)

        cooldowns_raw = gathered[0]
        # cooldowns_raw: dict[str, tuple[str | None, int | None]]
        cooldowns: dict[str, str | None] = {}
        cooldown_ttls: dict[str, int | None] = {}
        for kid, val in cooldowns_raw.items():
            if isinstance(val, tuple):
                cooldowns[kid] = val[0]
                cooldown_ttls[kid] = val[1]
            else:
                cooldowns[kid] = val
                cooldown_ttls[kid] = None

        # Cost check
        cost_exhausted: set[str] = set()
        cost_soft: set[str] = set()
        cost_totals: dict[str, int] = {}
        if _cost_idx >= 0:
            cost_totals = gathered[_cost_idx]
            limit = self.config.cost_limit_per_key_tokens
            assert limit is not None  # guarded by _cost_idx >= 0
            for kid, total in cost_totals.items():
                if total >= limit:
                    cost_exhausted.add(kid)
                elif total >= limit * self.config.cost_soft_threshold_percent / 100:
                    cost_soft.add(kid)

        # LRU scores
        lru_scores: dict[str, float] = {}
        if _lru_idx >= 0:
            lru_scores = gathered[_lru_idx]

        # --- Strategy: compute_score ----------------------------------
        for strategy in strategies:
            if hasattr(strategy, "compute_score"):
                for kid in all_key_ids:
                    try:
                        custom = strategy.compute_score(
                            key_id=kid,
                            config=self.config,
                            context=strategy_context,
                        )
                        if custom is not None:
                            lru_scores[kid] = custom
                    except Exception:
                        pass

        # --- 3. Classify candidates -----------------------------------
        sticky_candidate: ProviderCandidate | None = None
        available: list[ProviderCandidate] = []
        skipped: list[ProviderCandidate] = []

        for c in candidates:
            kid = str(c.key.id)
            ct = PoolCandidateTrace(key_id=kid)

            # Already skipped upstream?
            if c.is_skipped:
                skipped.append(c)
                ct.skipped = True
                ct.skip_type = "upstream"
                trace.candidate_traces[kid] = ct
                continue

            # Cooldown?
            redis_cd_reason = cooldowns.get(kid)
            candidate_provider_type = self.provider_type or (
                str(getattr(getattr(c, "provider", None), "provider_type", "") or "").strip()
                or None
            )
            cd_reason = resolve_effective_cooldown_reason(
                provider_type=candidate_provider_type,
                key=c.key,
                redis_reason=redis_cd_reason,
            )
            if cd_reason is not None:
                c.is_skipped = True
                c.skip_reason = f"pool cooldown: {cd_reason}"
                skipped.append(c)
                ct.skipped = True
                ct.skip_type = "cooldown"
                ct.cooldown_reason = cd_reason
                ct.cooldown_ttl = cooldown_ttls.get(kid) if redis_cd_reason is not None else None
                _attach_pool_extra(c, ct)
                trace.candidate_traces[kid] = ct
                continue

            # Cost exhausted?
            if kid in cost_exhausted:
                c.is_skipped = True
                c.skip_reason = "pool cost limit reached"
                skipped.append(c)
                ct.skipped = True
                ct.skip_type = "cost_exhausted"
                ct.cost_window_usage = cost_totals.get(kid, 0)
                ct.cost_limit = self.config.cost_limit_per_key_tokens
                _attach_pool_extra(c, ct)
                trace.candidate_traces[kid] = ct
                continue

            # Sticky hit?
            if sticky_key_id and kid == sticky_key_id:
                sticky_candidate = c
                ct.reason = "sticky"
                ct.sticky_hit = True
                trace.sticky_session_used = True
            else:
                available.append(c)
                ct.reason = "lru" if lru_scores.get(kid, 0) > 0 else "random"

            ct.lru_score = lru_scores.get(kid, 0.0)
            ct.cost_window_usage = cost_totals.get(kid, 0)
            ct.cost_limit = self.config.cost_limit_per_key_tokens
            if kid in cost_soft:
                ct.cost_soft_threshold = True
            _attach_pool_extra(c, ct)
            trace.candidate_traces[kid] = ct

        # --- 4. Sort available by LRU ---------------------------------
        if lru_scores and available:
            available.sort(key=lambda c: lru_scores.get(str(c.key.id), 0.0))

        # Random tiebreak among candidates with the same LRU score
        if len(available) > 1 and lru_scores:
            _shuffle_same_score_groups(available, lru_scores)

        # --- 5. Assemble final order ----------------------------------
        result: list[ProviderCandidate] = []
        if sticky_candidate is not None:
            result.append(sticky_candidate)
        result.extend(available)
        result.extend(skipped)

        if sticky_candidate:
            logger.debug(
                "Pool[{}]: sticky hit key={}",
                pid[:8],
                sticky_key_id and sticky_key_id[:8],
            )

        # --- Strategy: after_select -----------------------------------
        if result:
            first_kid = str(result[0].key.id)
            first_trace = trace.candidate_traces.get(first_kid)
            for strategy in strategies:
                if hasattr(strategy, "on_after_select") and first_trace:
                    try:
                        strategy.on_after_select(
                            provider_id=pid,
                            selected_key_id=first_kid,
                            trace=first_trace,
                            config=self.config,
                            context=strategy_context,
                        )
                    except Exception:
                        pass

        # Attach the full trace to the first candidate for downstream use.
        if result:
            setattr(result[0], "_pool_scheduling_trace", trace)

        return result

    # ------------------------------------------------------------------
    # Single-key selection (used by CandidateBuilder for pooled providers)
    # ------------------------------------------------------------------

    async def select_key(
        self,
        session_uuid: str | None,
        keys: list[ProviderAPIKey],
    ) -> ProviderAPIKey | None:
        """Select the best key from *keys* according to pool rules.

        Same logic as :meth:`reorder_candidates` but operates directly on
        :class:`ProviderAPIKey` objects instead of candidates:

        1. Sticky session hit (if bound and still healthy).
        2. Filter out keys in cooldown or cost-exhausted.
        3. LRU sort among remaining keys.
        4. Random tiebreak for identical LRU scores.
        5. Return the first available key, or ``None``.
        """
        if not keys:
            return None

        pid = self.provider_id

        # --- 1. Sticky session ------------------------------------------------
        sticky_key_id: str | None = None
        if session_uuid and self.config.sticky_session_ttl_seconds > 0:
            sticky_key_id = await redis_ops.get_sticky_binding(
                pid, session_uuid, self.config.sticky_session_ttl_seconds
            )

        # --- 2. Batch fetch pool state (parallel) -----------------------------
        key_ids = [str(k.id) for k in keys]

        _cooldown_coro = redis_ops.batch_get_cooldowns(pid, key_ids)
        _cost_coro = (
            redis_ops.batch_get_cost_totals(pid, key_ids, self.config.cost_window_seconds)
            if self.config.cost_limit_per_key_tokens is not None
            else None
        )
        _lru_coro = redis_ops.get_lru_scores(pid, key_ids) if self.config.lru_enabled else None

        coros_sk: list[Any] = [_cooldown_coro]
        _cost_idx_sk = -1
        _lru_idx_sk = -1
        if _cost_coro is not None:
            _cost_idx_sk = len(coros_sk)
            coros_sk.append(_cost_coro)
        if _lru_coro is not None:
            _lru_idx_sk = len(coros_sk)
            coros_sk.append(_lru_coro)

        gathered_sk = await asyncio.gather(*coros_sk)

        cooldowns = gathered_sk[0]

        cost_exhausted: set[str] = set()
        if _cost_idx_sk >= 0:
            cost_totals = gathered_sk[_cost_idx_sk]
            for kid, total in cost_totals.items():
                if total >= self.config.cost_limit_per_key_tokens:  # type: ignore[operator]
                    cost_exhausted.add(kid)

        lru_scores: dict[str, float] = {}
        if _lru_idx_sk >= 0:
            lru_scores = gathered_sk[_lru_idx_sk]

        # --- Strategy: compute_score ------------------------------------------
        strategies = _get_active_strategies(self.config)
        strategy_context: dict[str, Any] = {"session_uuid": session_uuid}
        for strategy in strategies:
            if hasattr(strategy, "compute_score"):
                for kid in key_ids:
                    try:
                        custom = strategy.compute_score(
                            key_id=kid,
                            config=self.config,
                            context=strategy_context,
                        )
                        if custom is not None:
                            lru_scores[kid] = custom
                    except Exception:
                        pass

        # --- 3. Classify keys -------------------------------------------------
        sticky_key: ProviderAPIKey | None = None
        available: list[ProviderAPIKey] = []

        for k in keys:
            kid = str(k.id)
            key_provider_type = self.provider_type or (
                str(getattr(getattr(k, "provider", None), "provider_type", "") or "").strip()
                or None
            )
            cd_reason = resolve_effective_cooldown_reason(
                provider_type=key_provider_type,
                key=k,
                redis_reason=cooldowns.get(kid),
            )
            if cd_reason is not None:
                continue
            if kid in cost_exhausted:
                continue

            if sticky_key_id and kid == sticky_key_id:
                sticky_key = k
                continue

            available.append(k)

        # --- 4. Sort by LRU ---------------------------------------------------
        if lru_scores and available:
            available.sort(key=lambda k: lru_scores.get(str(k.id), 0.0))

        # Random tiebreak within same-score groups
        if len(available) > 1 and lru_scores:
            _shuffle_same_score_keys(available, lru_scores)

        # --- 5. Pick the winner -----------------------------------------------
        if sticky_key is not None:
            logger.debug(
                "Pool[{}]: sticky select key={}",
                pid[:8],
                sticky_key_id and sticky_key_id[:8],
            )
            return sticky_key

        return available[0] if available else None

    # ------------------------------------------------------------------
    # Post-request hooks
    # ------------------------------------------------------------------

    async def on_request_success(
        self,
        *,
        session_uuid: str | None,
        key_id: str,
        tokens_used: int = 0,
    ) -> None:
        """Called after a successful upstream request."""
        pid = self.provider_id

        # Bind sticky session
        if session_uuid and self.config.sticky_session_ttl_seconds > 0:
            await redis_ops.set_sticky_binding(
                pid, session_uuid, key_id, self.config.sticky_session_ttl_seconds
            )

        # Touch LRU
        if self.config.lru_enabled:
            await redis_ops.touch_lru(pid, key_id)

        # Record cost
        if tokens_used > 0 and self.config.cost_limit_per_key_tokens is not None:
            await redis_ops.add_cost_entry(
                pid, key_id, tokens_used, self.config.cost_window_seconds
            )

    async def on_request_error(
        self,
        *,
        key_id: str,
        status_code: int,
        error_body: str | None = None,
        response_headers: dict[str, str] | None = None,
    ) -> None:
        """Called after an upstream error. Delegates to health policy."""
        # Import lazily to avoid circular deps
        from src.services.provider.pool.health_policy import apply_health_policy

        await apply_health_policy(
            provider_id=self.provider_id,
            key_id=key_id,
            status_code=status_code,
            error_body=error_body,
            response_headers=response_headers,
            config=self.config,
        )

    # ------------------------------------------------------------------
    # Key schedulability check (used by candidate_builder)
    # ------------------------------------------------------------------

    async def is_key_schedulable(self, key_id: str) -> tuple[bool, str | None]:
        """Check if *key_id* is currently schedulable (not in cooldown, not
        cost-exhausted).  Returns ``(True, None)`` or ``(False, reason)``.
        """
        pid = self.provider_id

        # Cooldown check
        cd = await redis_ops.get_cooldown(pid, key_id)
        if cd is not None:
            return False, f"pool cooldown: {cd}"

        # Cost check
        if self.config.cost_limit_per_key_tokens is not None:
            total = await redis_ops.get_cost_window_total(
                pid, key_id, self.config.cost_window_seconds
            )
            if total >= self.config.cost_limit_per_key_tokens:
                return False, "pool cost limit reached"

        return True, None


# Backward-compatible alias
ClaudeCodePoolManager = PoolManager


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

_T = TypeVar("_T")


def _attach_pool_extra(candidate: Any, ct: PoolCandidateTrace) -> None:
    """Attach pool trace extra_data onto a candidate object."""
    existing = getattr(candidate, "_pool_extra_data", None) or {}
    existing.update(ct.to_extra_data())
    setattr(candidate, "_pool_extra_data", existing)


def _get_active_strategies(config: PoolConfig) -> list[Any]:
    """Get active strategies for the given config (lazy import)."""
    if not config.strategies:
        return []
    try:
        from src.services.provider.pool.strategy import get_active_strategies

        return get_active_strategies(config.strategies)
    except Exception:
        return []


def _shuffle_same_score(
    items: list[_T],
    lru_scores: dict[str, float],
    key_fn: Callable[[_T], str],
) -> None:
    """In-place random shuffle within groups that share the same LRU score."""
    if len(items) <= 1:
        return

    i = 0
    while i < len(items):
        score_i = lru_scores.get(key_fn(items[i]), 0.0)
        j = i + 1
        while j < len(items) and lru_scores.get(key_fn(items[j]), 0.0) == score_i:
            j += 1
        if j - i > 1:
            group = items[i:j]
            random.shuffle(group)
            items[i:j] = group
        i = j


def _shuffle_same_score_groups(
    candidates: list[ProviderCandidate],
    lru_scores: dict[str, float],
) -> None:
    _shuffle_same_score(candidates, lru_scores, lambda c: str(c.key.id))


def _shuffle_same_score_keys(
    keys: list[ProviderAPIKey],
    lru_scores: dict[str, float],
) -> None:
    _shuffle_same_score(keys, lru_scores, lambda k: str(k.id))
