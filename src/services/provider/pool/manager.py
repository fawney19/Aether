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
from src.services.provider.pool.account_state import resolve_pool_account_state
from src.services.provider.pool.config import PoolConfig
from src.services.provider.pool.health_cache import get_health_scores
from src.services.provider.pool.trace import PoolCandidateTrace, PoolSchedulingTrace

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
        2. **Filter** out keys in account-blocked / cooldown / cost-exhausted state (mark
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

        provider_type = self.provider_type
        if provider_type is None and candidates:
            first_provider = getattr(candidates[0], "provider", None)
            provider_type = str(getattr(first_provider, "provider_type", "") or "").strip().lower()
            if not provider_type:
                provider_type = None

        # --- 2. Batch fetch pool state (parallel) ---------------------
        all_key_ids = [str(c.key.id) for c in candidates]

        # Fire independent Redis queries concurrently.
        # Only fetch reason (no TTL) on the scheduling hot path -- TTL is only
        # used for trace display and costs an extra pipeline command per key.
        _cooldown_coro = redis_ops.batch_get_cooldowns(pid, all_key_ids, include_ttl=False)
        _cost_coro = (
            redis_ops.batch_get_cost_totals(pid, all_key_ids, self.config.cost_window_seconds)
            if (
                self.config.cost_limit_per_key_tokens is not None
                or self.config.scheduling_mode == "multi_score"
            )
            else None
        )
        # LRU scores are needed both for plain LRU sorting and for multi_score
        # dimensions (e.g. cache_affinity / single_account) that rely on
        # lru_scores data.
        _need_lru = self.config.lru_enabled or self.config.scheduling_mode == "multi_score"
        _lru_coro = redis_ops.get_lru_scores(pid, all_key_ids) if _need_lru else None
        _latency_coro = (
            redis_ops.batch_get_latency_avgs(pid, all_key_ids, self.config.latency_window_seconds)
            if self.config.scheduling_mode == "multi_score"
            else None
        )

        # Gather all non-None coroutines in parallel.
        coros: list[Any] = [_cooldown_coro]
        _cost_idx = -1
        _lru_idx = -1
        _latency_idx = -1
        if _cost_coro is not None:
            _cost_idx = len(coros)
            coros.append(_cost_coro)
        if _lru_coro is not None:
            _lru_idx = len(coros)
            coros.append(_lru_coro)
        if _latency_coro is not None:
            _latency_idx = len(coros)
            coros.append(_latency_coro)

        gathered = await asyncio.gather(*coros)

        cooldowns: dict[str, str | None] = gathered[0]

        # Cost check
        cost_exhausted: set[str] = set()
        cost_soft: set[str] = set()
        cost_totals: dict[str, int] = {}
        if _cost_idx >= 0:
            cost_totals = gathered[_cost_idx]
            limit = self.config.cost_limit_per_key_tokens
            if limit is not None:
                for kid, total in cost_totals.items():
                    if total >= limit:
                        cost_exhausted.add(kid)
                    elif total >= limit * self.config.cost_soft_threshold_percent / 100:
                        cost_soft.add(kid)

        # LRU scores
        lru_scores: dict[str, float] = {}
        if _lru_idx >= 0:
            lru_scores = gathered[_lru_idx]

        # Latency averages
        latency_avgs: dict[str, float] = {}
        if _latency_idx >= 0:
            latency_avgs = gathered[_latency_idx]

        # Health scores (TTL cached, no Redis round-trip) -- only needed for multi_score
        health_scores: dict[str, float] = {}
        if self.config.scheduling_mode == "multi_score":
            health_scores = get_health_scores(pid, [c.key for c in candidates])

        strategy_context.update(
            {
                "all_key_ids": all_key_ids,
                "lru_scores": lru_scores,
                "cost_totals": cost_totals,
                "cost_limit_per_key_tokens": self.config.cost_limit_per_key_tokens,
                "latency_avgs": latency_avgs,
                "health_scores": health_scores,
                "keys_by_id": {str(c.key.id): c.key for c in candidates},
            }
        )

        # --- Strategy: compute_score ----------------------------------
        custom_scores: dict[str, float] = {}
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
                            custom_scores[kid] = float(custom)
                            lru_scores[kid] = float(custom)
                    except Exception:
                        pass

        # --- 3. Classify candidates -----------------------------------
        # Use precomputed account states from CandidateBuilder when available
        # (upstream_metadata is deferred on pool keys to save memory).
        # Fall back to on-the-fly resolution for non-pool candidates.
        account_states: dict[str, Any] = {}
        for c in candidates:
            kid = str(c.key.id)
            if kid not in account_states:
                precomputed = getattr(c.key, "_pool_account_state", None)
                if precomputed is not None:
                    account_states[kid] = precomputed
                else:
                    account_states[kid] = resolve_pool_account_state(
                        provider_type=provider_type,
                        upstream_metadata=getattr(c.key, "upstream_metadata", None),
                        oauth_invalid_reason=getattr(c.key, "oauth_invalid_reason", None),
                    )

        sticky_candidate: ProviderCandidate | None = None
        available: list[ProviderCandidate] = []
        skipped: list[ProviderCandidate] = []

        for c in candidates:
            kid = str(c.key.id)
            ct = PoolCandidateTrace(key_id=kid)
            ct.scoring_mode = self.config.scheduling_mode
            ct.latency_avg_ms = float(latency_avgs.get(kid, 0.0) or 0.0)
            ct.health_score = float(health_scores.get(kid, 1.0) or 1.0)
            if kid in custom_scores:
                ct.composite_score = float(custom_scores[kid])

            # Already skipped upstream?
            if c.is_skipped:
                skipped.append(c)
                ct.skipped = True
                ct.skip_type = "upstream"
                trace.candidate_traces[kid] = ct
                continue

            # Account blocked?
            account_state = account_states[kid]
            if account_state.blocked:
                c.is_skipped = True
                skip_reason = account_state.reason or account_state.label or "account blocked"
                c.skip_reason = f"pool account blocked: {skip_reason}"
                skipped.append(c)
                ct.skipped = True
                ct.skip_type = "account_blocked"
                ct.account_block_code = account_state.code
                ct.account_block_label = account_state.label
                ct.account_block_reason = account_state.reason
                _attach_pool_extra(c, ct)
                trace.candidate_traces[kid] = ct
                continue

            cd_reason = cooldowns.get(kid)
            if cd_reason is not None:
                c.is_skipped = True
                c.skip_reason = f"pool cooldown: {cd_reason}"
                skipped.append(c)
                ct.skipped = True
                ct.skip_type = "cooldown"
                ct.cooldown_reason = cd_reason
                ct.cooldown_ttl = None  # TTL skipped on hot path for perf
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
                if kid in custom_scores and self.config.scheduling_mode == "multi_score":
                    ct.reason = "multi_score"
                else:
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

    async def select_pool_keys(
        self,
        session_uuid: str | None,
        keys: list[ProviderAPIKey],
        *,
        availability_checker: (
            Callable[[ProviderAPIKey], tuple[bool, str | None, str | None]] | None
        ) = None,
        page_size: int = 50,
    ) -> tuple[list[ProviderAPIKey], PoolSchedulingTrace]:
        """Select and order pool keys with trace output.

        Reuses :meth:`reorder_candidates` logic by adapting keys to lightweight
        candidate-like wrappers, then propagates skip/trace metadata back onto
        each key object for downstream execution/recording.

        When *availability_checker* is provided, post-sort availability checks
        are performed lazily: only the top *page_size* non-skipped keys are
        checked at a time; if all fail, the next page is checked, and so on.
        Keys beyond the last checked page are marked as ``deferred`` (skipped
        without checking) to avoid unnecessary CPU work on large pools.
        """
        if not keys:
            return (
                [],
                PoolSchedulingTrace(
                    provider_id=self.provider_id,
                    total_keys=0,
                    session_uuid=session_uuid[:8] if session_uuid else None,
                ),
            )

        class _KeyCandidate:
            __slots__ = (
                "key",
                "is_skipped",
                "skip_reason",
                "_pool_extra_data",
                "_pool_scheduling_trace",
            )

            def __init__(self, key: ProviderAPIKey) -> None:
                self.key = key
                self.is_skipped = False
                self.skip_reason: str | None = None
                self._pool_extra_data: dict | None = None
                self._pool_scheduling_trace: PoolSchedulingTrace | None = None

        wrappers = [_KeyCandidate(k) for k in keys]
        reordered_wrappers = await self.reorder_candidates(session_uuid, wrappers)  # type: ignore[arg-type]

        trace: PoolSchedulingTrace | None = None
        if reordered_wrappers:
            maybe_trace = getattr(reordered_wrappers[0], "_pool_scheduling_trace", None)
            if isinstance(maybe_trace, PoolSchedulingTrace):
                trace = maybe_trace
        if trace is None:
            trace = PoolSchedulingTrace(
                provider_id=self.provider_id,
                total_keys=len(keys),
                session_uuid=session_uuid[:8] if session_uuid else None,
            )

        ordered_keys: list[ProviderAPIKey] = []
        for order_idx, wrapped in enumerate(reordered_wrappers):
            key = wrapped.key
            is_skipped = bool(getattr(wrapped, "is_skipped", False))
            skip_reason = str(getattr(wrapped, "skip_reason", "") or "")
            setattr(key, "_pool_skipped", is_skipped)
            setattr(key, "_pool_skip_reason", skip_reason if skip_reason else None)
            setattr(key, "_pool_order_index", order_idx)
            pool_extra = getattr(wrapped, "_pool_extra_data", None)
            setattr(
                key, "_pool_extra_data", dict(pool_extra) if isinstance(pool_extra, dict) else {}
            )
            ordered_keys.append(key)

        # -- 分页可用性检查 --
        # 排序后对非 skipped key 分页调用 availability_checker，
        # 找到 page_size 个可用 key 后停止检查，剩余标记 deferred。
        if availability_checker is not None:
            available_count = 0
            found_enough = False
            for key in ordered_keys:
                if getattr(key, "_pool_skipped", False):
                    continue
                if found_enough:
                    setattr(key, "_pool_skipped", True)
                    setattr(key, "_pool_skip_reason", "deferred")
                    continue
                is_available, skip_reason_check, mapping_model = availability_checker(key)
                if not is_available:
                    setattr(key, "_pool_skipped", True)
                    setattr(key, "_pool_skip_reason", skip_reason_check)
                else:
                    if mapping_model:
                        setattr(key, "_pool_mapping_matched_model", mapping_model)
                    available_count += 1
                    if available_count >= page_size:
                        found_enough = True

        return ordered_keys, trace

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
        2. Filter out keys in account-blocked / cooldown / cost-exhausted.
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
            if (
                self.config.cost_limit_per_key_tokens is not None
                or self.config.scheduling_mode == "multi_score"
            )
            else None
        )
        _need_lru_sk = self.config.lru_enabled or self.config.scheduling_mode == "multi_score"
        _lru_coro = redis_ops.get_lru_scores(pid, key_ids) if _need_lru_sk else None
        _latency_coro = (
            redis_ops.batch_get_latency_avgs(pid, key_ids, self.config.latency_window_seconds)
            if self.config.scheduling_mode == "multi_score"
            else None
        )

        coros_sk: list[Any] = [_cooldown_coro]
        _cost_idx_sk = -1
        _lru_idx_sk = -1
        _latency_idx_sk = -1
        if _cost_coro is not None:
            _cost_idx_sk = len(coros_sk)
            coros_sk.append(_cost_coro)
        if _lru_coro is not None:
            _lru_idx_sk = len(coros_sk)
            coros_sk.append(_lru_coro)
        if _latency_coro is not None:
            _latency_idx_sk = len(coros_sk)
            coros_sk.append(_latency_coro)

        gathered_sk = await asyncio.gather(*coros_sk)

        cooldowns = gathered_sk[0]

        cost_exhausted: set[str] = set()
        cost_totals: dict[str, int] = {}
        if _cost_idx_sk >= 0:
            cost_totals = gathered_sk[_cost_idx_sk]
            if self.config.cost_limit_per_key_tokens is not None:
                for kid, total in cost_totals.items():
                    if total >= self.config.cost_limit_per_key_tokens:
                        cost_exhausted.add(kid)

        lru_scores: dict[str, float] = {}
        if _lru_idx_sk >= 0:
            lru_scores = gathered_sk[_lru_idx_sk]

        latency_avgs: dict[str, float] = {}
        if _latency_idx_sk >= 0:
            latency_avgs = gathered_sk[_latency_idx_sk]

        health_scores: dict[str, float] = {}
        if self.config.scheduling_mode == "multi_score":
            health_scores = get_health_scores(pid, keys)

        # --- Strategy: compute_score ------------------------------------------
        strategies = _get_active_strategies(self.config)
        strategy_context: dict[str, Any] = {
            "session_uuid": session_uuid,
            "all_key_ids": key_ids,
            "lru_scores": lru_scores,
            "cost_totals": cost_totals if _cost_idx_sk >= 0 else {},
            "cost_limit_per_key_tokens": self.config.cost_limit_per_key_tokens,
            "latency_avgs": latency_avgs,
            "health_scores": health_scores,
            "keys_by_id": {str(k.id): k for k in keys},
        }
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
                            lru_scores[kid] = float(custom)
                    except Exception:
                        pass

        # --- 3. Classify keys -------------------------------------------------
        # Use precomputed account states when available.
        account_states_sk: dict[str, Any] = {}
        for k in keys:
            kid = str(k.id)
            if kid not in account_states_sk:
                precomputed = getattr(k, "_pool_account_state", None)
                if precomputed is not None:
                    account_states_sk[kid] = precomputed
                else:
                    account_states_sk[kid] = resolve_pool_account_state(
                        provider_type=self.provider_type,
                        upstream_metadata=getattr(k, "upstream_metadata", None),
                        oauth_invalid_reason=getattr(k, "oauth_invalid_reason", None),
                    )

        sticky_key: ProviderAPIKey | None = None
        available: list[ProviderAPIKey] = []

        for k in keys:
            kid = str(k.id)

            if account_states_sk[kid].blocked:
                continue

            if cooldowns.get(kid) is not None:
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
        ttfb_ms: int | None = None,
    ) -> None:
        """Called after a successful upstream request."""
        pid = self.provider_id

        # Bind sticky session
        if session_uuid and self.config.sticky_session_ttl_seconds > 0:
            await redis_ops.set_sticky_binding(
                pid, session_uuid, key_id, self.config.sticky_session_ttl_seconds
            )

        # Touch LRU -- needed for both plain LRU mode and multi_score dimensions
        # (e.g. cache_affinity) that rely on LRU timestamps.
        if self.config.lru_enabled or self.config.scheduling_mode == "multi_score":
            await redis_ops.touch_lru(pid, key_id)

        # Record cost
        if tokens_used > 0 and self.config.cost_limit_per_key_tokens is not None:
            await redis_ops.add_cost_entry(
                pid, key_id, tokens_used, self.config.cost_window_seconds
            )

        # Record latency sample for multi-score scheduling.
        if self.config.scheduling_mode == "multi_score" and ttfb_ms is not None and ttfb_ms >= 0:
            await redis_ops.record_latency(
                pid,
                key_id,
                ttfb_ms,
                self.config.latency_window_seconds,
                self.config.latency_sample_limit,
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
        # Import triggers built-in strategy registration via module-level side effects.
        from src.services.provider.pool import strategies as _builtin_strategies  # noqa: F401
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
