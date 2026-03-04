from __future__ import annotations

import asyncio
import json
import re
from collections.abc import Awaitable, Callable
from dataclasses import dataclass
from datetime import datetime, timezone
from typing import Any, AsyncIterator

from sqlalchemy import update
from sqlalchemy.orm import Session

from src.core.logger import logger
from src.models.database import RequestCandidate
from src.services.orchestration.error_classifier import ErrorAction, ErrorClassifier
from src.services.request.candidate import RequestCandidateService
from src.services.scheduling.schemas import PoolCandidate, ProviderCandidate
from src.services.task.core.exceptions import StreamProbeError
from src.services.task.core.protocol import AttemptFunc, AttemptKind, AttemptResult
from src.services.task.core.schema import ExecutionResult

from .policy import FailoverAction, RetryMode, RetryPolicy, SkipPolicy
from .recorder import CandidateRecorder
from .schema import CandidateKey

_SENSITIVE_PATTERN = re.compile(
    r"(api[_-]?key|token|bearer|authorization)[=:\s]+\S+",
    re.IGNORECASE,
)


@dataclass
class AttemptErrorOutcome:
    """_handle_attempt_error 的返回结果"""

    action: FailoverAction
    last_status_code: int | None
    max_retries: int
    stop_result: ExecutionResult | None = None


class FailoverEngine:
    """
    FailoverEngine executes candidate attempts under policies.

    Phase3 core: unified failover loop used by TaskService.
    """

    # Hard constraint: streaming first chunk probe timeout
    STREAM_FIRST_CHUNK_TIMEOUT_SECONDS: int = 30

    def __init__(
        self,
        db: Session,
        *,
        error_classifier: ErrorClassifier | None = None,
        recorder: CandidateRecorder | None = None,
    ) -> None:
        self.db = db
        self._error_classifier = error_classifier or ErrorClassifier(db=db)
        self._recorder = recorder or CandidateRecorder(db)

    async def execute(
        self,
        *,
        candidates: list[ProviderCandidate],
        attempt_func: AttemptFunc,
        retry_policy: RetryPolicy,
        skip_policy: SkipPolicy,
        request_id: str | None = None,
        user_id: str | None = None,
        api_key_id: str | None = None,
        candidate_record_map: dict[tuple[int, int], str] | None = None,
        max_candidates: int | None = None,
        max_attempts: int | None = None,
        execution_error_handler: (
            Callable[
                ...,
                Awaitable[tuple[FailoverAction, int | None]],
            ]
            | None
        ) = None,
    ) -> ExecutionResult:
        """
        Execute candidate traversal + retry + failover.

        Notes:
        - For PRE_EXPAND: `candidate_record_map` should be provided (created by CandidateResolver).
        - For ON_DEMAND/DISABLED: records are created when used (and on skip, best-effort).
        """
        candidate_keys_fallback: list[CandidateKey] = []

        if max_candidates is not None and max_candidates > 0:
            candidates = candidates[:max_candidates]

        attempt_count = 0
        last_status_code: int | None = None

        # For logging / dispatcher parity only; callers may pass an exact value.
        if max_attempts is None:
            computed = 0
            for cand in candidates:
                should_skip, _ = self._should_skip(cand, skip_policy)
                if should_skip:
                    continue
                computed += self._get_max_retries(cand, retry_policy)
            max_attempts = computed

        for candidate_index, candidate in enumerate(candidates):
            should_skip, skip_reason = self._should_skip(candidate, skip_policy)
            if should_skip:
                # PRE_EXPAND: mark all retry slots skipped.
                if retry_policy.mode == RetryMode.PRE_EXPAND and candidate_record_map:
                    self._mark_candidate_skipped(
                        candidate_record_map=candidate_record_map,
                        candidate_index=candidate_index,
                        candidate=candidate,
                        retry_policy=retry_policy,
                        skip_reason=skip_reason,
                    )
                else:
                    # ON_DEMAND/DISABLED: create a skipped record for audit (best-effort).
                    if request_id:
                        await self._create_skipped_record(
                            request_id=request_id,
                            candidate=candidate,
                            candidate_index=candidate_index,
                            user_id=user_id,
                            api_key_id=api_key_id,
                            skip_reason=skip_reason,
                        )
                candidate_keys_fallback.append(
                    self._make_candidate_key(
                        candidate=candidate,
                        candidate_index=candidate_index,
                        retry_index=0,
                        status="skipped",
                        skip_reason=skip_reason,
                    )
                )
                continue

            if isinstance(candidate, PoolCandidate):
                pool_result, attempt_count, last_status_code = await self._execute_pool_candidate(
                    candidate=candidate,
                    candidate_index=candidate_index,
                    attempt_func=attempt_func,
                    retry_policy=retry_policy,
                    request_id=request_id,
                    user_id=user_id,
                    api_key_id=api_key_id,
                    candidate_record_map=candidate_record_map,
                    candidate_keys_fallback=candidate_keys_fallback,
                    candidates=candidates,
                    attempt_count=attempt_count,
                    max_attempts=max_attempts,
                    execution_error_handler=execution_error_handler,
                )
                if pool_result is not None:
                    return pool_result
                continue

            max_retries = self._get_max_retries(candidate, retry_policy)
            retry_index = 0
            while retry_index < max_retries:
                attempt_count += 1

                # Resolve/create record_id
                record_id = None
                if candidate_record_map:
                    record_id = candidate_record_map.get((candidate_index, retry_index))
                    if record_id is None:
                        # Rectify may extend retries beyond pre-created range; reuse retry 0 record.
                        record_id = candidate_record_map.get((candidate_index, 0))
                if record_id is None and request_id and retry_policy.mode != RetryMode.PRE_EXPAND:
                    record_id = await self._ensure_record_exists(
                        request_id=request_id,
                        candidate=candidate,
                        candidate_index=candidate_index,
                        retry_index=retry_index,
                        user_id=user_id,
                        api_key_id=api_key_id,
                    )

                self._attach_attempt_context(
                    candidate, candidate_index, retry_index, record_id, attempt_count, max_attempts
                )

                # Mark pending
                now = datetime.now(timezone.utc)
                if record_id:
                    self._update_record(
                        record_id,
                        status="pending",
                        started_at=now,
                    )

                # Commit BEFORE await (avoid holding DB connections during slow upstream calls)
                self._commit_before_await()

                try:
                    attempt_result = await self._execute_attempt(
                        candidate=candidate,
                        record_id=record_id,
                        attempt_func=attempt_func,
                    )
                    last_status_code = int(getattr(attempt_result, "http_status", 0) or 0)

                    # PRE_EXPAND: mark unused slots after request ends (success)
                    if retry_policy.mode == RetryMode.PRE_EXPAND and candidate_record_map:
                        self._mark_remaining_slots_unused(
                            candidate_record_map=candidate_record_map,
                            candidates=candidates,
                            success_candidate_idx=candidate_index,
                            success_retry_idx=retry_index,
                            retry_policy=retry_policy,
                        )

                    return ExecutionResult(
                        success=True,
                        attempt_result=attempt_result,
                        candidate=candidate,
                        candidate_index=candidate_index,
                        retry_index=retry_index,
                        provider_id=str(candidate.provider.id),
                        provider_name=str(candidate.provider.name),
                        endpoint_id=str(candidate.endpoint.id),
                        key_id=str(candidate.key.id),
                        candidate_keys=self._get_candidate_keys(
                            request_id=request_id,
                            fallback=candidate_keys_fallback,
                            candidates=candidates,
                        ),
                        attempt_count=attempt_count,
                        request_candidate_id=record_id,
                    )

                except StreamProbeError as exc:
                    last_status_code = exc.http_status
                    self._record_attempt_failure(record_id, exc, exc.http_status)
                    action = FailoverAction.CONTINUE

                except Exception as exc:
                    outcome = await self._handle_attempt_error(
                        exc,
                        candidate=candidate,
                        candidate_index=candidate_index,
                        retry_index=retry_index,
                        max_retries=max_retries,
                        record_id=record_id,
                        attempt_count=attempt_count,
                        max_attempts=max_attempts,
                        execution_error_handler=execution_error_handler,
                        retry_policy=retry_policy,
                        candidate_record_map=candidate_record_map,
                        candidates=candidates,
                        request_id=request_id,
                        candidate_keys_fallback=candidate_keys_fallback,
                    )
                    action = outcome.action
                    last_status_code = outcome.last_status_code
                    max_retries = outcome.max_retries
                    if outcome.stop_result is not None:
                        return outcome.stop_result

                # action switch: continue/ retry
                if action == FailoverAction.CONTINUE:
                    # PRE_EXPAND: if we break early, mark remaining retries of this candidate unused.
                    if retry_policy.mode == RetryMode.PRE_EXPAND and candidate_record_map:
                        self._mark_candidate_remaining_retries_unused(
                            candidate_record_map=candidate_record_map,
                            candidate_idx=candidate_index,
                            from_retry_idx=retry_index + 1,
                            retry_policy=retry_policy,
                        )
                    break
                if action == FailoverAction.RETRY:
                    retry_index += 1
                    continue

                # Safety: unknown action -> stop retrying this candidate.
                break

        # exhausted: PRE_EXPAND should not leave 'available' records behind
        if retry_policy.mode == RetryMode.PRE_EXPAND and candidate_record_map:
            self._mark_all_remaining_available_unused(candidate_record_map)

        return ExecutionResult(
            success=False,
            error_type="AllCandidatesFailed",
            error_message="All candidates exhausted",
            last_status_code=last_status_code,
            candidate_keys=self._get_candidate_keys(
                request_id=request_id,
                fallback=candidate_keys_fallback,
                candidates=candidates,
            ),
            attempt_count=attempt_count,
        )

    async def _execute_pool_candidate(
        self,
        *,
        candidate: PoolCandidate,
        candidate_index: int,
        attempt_func: AttemptFunc,
        retry_policy: RetryPolicy,
        request_id: str | None,
        user_id: str | None,
        api_key_id: str | None,
        candidate_record_map: dict[tuple[int, int], str] | None,
        candidate_keys_fallback: list[CandidateKey],
        candidates: list[ProviderCandidate],
        attempt_count: int,
        max_attempts: int | None,
        execution_error_handler: Any,
    ) -> tuple[ExecutionResult | None, int, int | None]:
        """Execute a PoolCandidate with in-pool key failover."""
        last_status_code: int | None = None
        retry_slots_per_key = self._get_pool_key_max_retries(candidate, retry_policy)

        for key_index, pool_key in enumerate(candidate.pool_keys or []):
            base_retry_index = key_index * retry_slots_per_key
            candidate.key = pool_key
            candidate._pool_key_index = key_index
            candidate.mapping_matched_model = getattr(pool_key, "_pool_mapping_matched_model", None)

            if bool(getattr(pool_key, "_pool_skipped", False)):
                skip_reason = str(
                    getattr(pool_key, "_pool_skip_reason", None)
                    or getattr(candidate, "skip_reason", None)
                    or "pool_skipped"
                )
                if retry_policy.mode == RetryMode.PRE_EXPAND and candidate_record_map:
                    self._mark_retry_indices_status(
                        candidate_record_map=candidate_record_map,
                        candidate_idx=candidate_index,
                        retry_indices=range(
                            base_retry_index, base_retry_index + retry_slots_per_key
                        ),
                        status="skipped",
                        skip_reason=skip_reason,
                    )
                elif request_id:
                    await self._create_skipped_record(
                        request_id=request_id,
                        candidate=candidate,
                        candidate_index=candidate_index,
                        retry_index=base_retry_index,
                        user_id=user_id,
                        api_key_id=api_key_id,
                        skip_reason=skip_reason,
                    )
                candidate_keys_fallback.append(
                    self._make_candidate_key(
                        candidate=candidate,
                        candidate_index=candidate_index,
                        retry_index=base_retry_index,
                        status="skipped",
                        skip_reason=skip_reason,
                    )
                )
                continue

            max_retries_for_key = retry_slots_per_key
            retry_index = 0
            while retry_index < max_retries_for_key:
                attempt_count += 1
                composite_retry_index = base_retry_index + retry_index

                record_id = None
                if candidate_record_map:
                    record_id = candidate_record_map.get((candidate_index, composite_retry_index))
                    if record_id is None:
                        # Rectify may extend retries beyond pre-created range.
                        record_id = candidate_record_map.get((candidate_index, base_retry_index))
                if record_id is None and request_id and retry_policy.mode != RetryMode.PRE_EXPAND:
                    record_id = await self._ensure_record_exists(
                        request_id=request_id,
                        candidate=candidate,
                        candidate_index=candidate_index,
                        retry_index=composite_retry_index,
                        user_id=user_id,
                        api_key_id=api_key_id,
                    )

                self._attach_attempt_context(
                    candidate,
                    candidate_index,
                    composite_retry_index,
                    record_id,
                    attempt_count,
                    max_attempts,
                )

                now = datetime.now(timezone.utc)
                if record_id:
                    self._update_record(
                        record_id,
                        status="pending",
                        started_at=now,
                    )

                self._commit_before_await()

                try:
                    attempt_result = await self._execute_attempt(
                        candidate=candidate,
                        record_id=record_id,
                        attempt_func=attempt_func,
                    )
                    last_status_code = int(getattr(attempt_result, "http_status", 0) or 0)

                    if retry_policy.mode == RetryMode.PRE_EXPAND and candidate_record_map:
                        self._mark_remaining_slots_unused(
                            candidate_record_map=candidate_record_map,
                            candidates=candidates,
                            success_candidate_idx=candidate_index,
                            success_retry_idx=composite_retry_index,
                            retry_policy=retry_policy,
                        )

                    return (
                        ExecutionResult(
                            success=True,
                            attempt_result=attempt_result,
                            candidate=candidate,
                            candidate_index=candidate_index,
                            retry_index=composite_retry_index,
                            provider_id=str(candidate.provider.id),
                            provider_name=str(candidate.provider.name),
                            endpoint_id=str(candidate.endpoint.id),
                            key_id=str(candidate.key.id),
                            candidate_keys=self._get_candidate_keys(
                                request_id=request_id,
                                fallback=candidate_keys_fallback,
                                candidates=candidates,
                            ),
                            attempt_count=attempt_count,
                            request_candidate_id=record_id,
                        ),
                        attempt_count,
                        last_status_code,
                    )

                except StreamProbeError as exc:
                    last_status_code = exc.http_status
                    self._record_attempt_failure(record_id, exc, exc.http_status)
                    action = FailoverAction.CONTINUE

                except Exception as exc:
                    outcome = await self._handle_pool_attempt_error(
                        exc,
                        candidate=candidate,
                        candidate_index=candidate_index,
                        key_retry_index=retry_index,
                        composite_retry_index=composite_retry_index,
                        max_retries=max_retries_for_key,
                        record_id=record_id,
                        attempt_count=attempt_count,
                        max_attempts=max_attempts,
                        execution_error_handler=execution_error_handler,
                    )
                    action = outcome.action
                    last_status_code = outcome.last_status_code
                    max_retries_for_key = min(outcome.max_retries, retry_slots_per_key)

                if action == FailoverAction.CONTINUE:
                    if retry_policy.mode == RetryMode.PRE_EXPAND and candidate_record_map:
                        # max_retries_for_key may have been shrunk by error handler;
                        # mark unused up to the *original* retry_slots_per_key to cover
                        # all pre-created records.
                        self._mark_retry_indices_status(
                            candidate_record_map=candidate_record_map,
                            candidate_idx=candidate_index,
                            retry_indices=range(
                                composite_retry_index + 1,
                                base_retry_index + retry_slots_per_key,
                            ),
                            status="unused",
                        )
                    break
                if action == FailoverAction.RETRY:
                    retry_index += 1
                    continue

                # STOP: only stop this pool candidate; outer candidate traversal continues.
                # Rationale: pool-internal STOP (from error_stop_patterns on a non-ExecutionError)
                # should not terminate the entire request because other providers may still succeed.
                # When handler_used=True, TaskService raises directly for true STOP semantics.
                if retry_policy.mode == RetryMode.PRE_EXPAND and candidate_record_map:
                    self._mark_candidate_remaining_retries_unused(
                        candidate_record_map=candidate_record_map,
                        candidate_idx=candidate_index,
                        from_retry_idx=composite_retry_index + 1,
                        retry_policy=retry_policy,
                    )
                return None, attempt_count, last_status_code

        return None, attempt_count, last_status_code

    async def _handle_pool_attempt_error(
        self,
        exc: Exception,
        *,
        candidate: ProviderCandidate,
        candidate_index: int,
        key_retry_index: int,
        composite_retry_index: int,
        max_retries: int,
        record_id: str | None,
        attempt_count: int,
        max_attempts: int | None,
        execution_error_handler: Any,
    ) -> AttemptErrorOutcome:
        """Handle pool attempt errors without forcing outer STOP semantics.

        Args:
            key_retry_index: key 内部的重试索引 (用于判断 has_retry_left)
            composite_retry_index: 全局维度的重试索引 (传给 execution_error_handler,
                与 candidate_record_map 对齐)
        """
        return await self._classify_attempt_error(
            exc,
            candidate=candidate,
            candidate_index=candidate_index,
            retry_index=composite_retry_index,
            has_retry_left=key_retry_index + 1 < max_retries,
            max_retries=max_retries,
            record_id=record_id,
            attempt_count=attempt_count,
            max_attempts=max_attempts,
            execution_error_handler=execution_error_handler,
        )

    @staticmethod
    def _attach_attempt_context(
        candidate: ProviderCandidate,
        candidate_index: int,
        retry_index: int,
        record_id: str | None,
        attempt_count: int,
        max_attempts: int | None,
    ) -> None:
        """Attach per-attempt context onto candidate for attempt_func (best-effort)."""
        try:
            setattr(candidate, "_utf_candidate_index", candidate_index)
            setattr(candidate, "_utf_retry_index", retry_index)
            setattr(candidate, "_utf_candidate_record_id", record_id)
            setattr(candidate, "_utf_attempt_count", attempt_count)
            setattr(candidate, "_utf_max_attempts", max_attempts)
        except Exception:
            pass

    async def _execute_attempt(
        self,
        *,
        candidate: ProviderCandidate,
        record_id: str | None,
        attempt_func: AttemptFunc,
    ) -> AttemptResult:
        """Run attempt_func with stream probe and sync failover-pattern checks.

        On success records the attempt; raises StreamProbeError on failover-pattern
        match or stream probe failure so the caller can handle retries uniformly.
        """
        attempt_result = await attempt_func(candidate)

        if attempt_result.kind == AttemptKind.STREAM:
            attempt_result = await self._probe_stream_first_chunk(
                attempt_result=attempt_result,
                record_id=record_id,
                candidate=candidate,
            )

        if attempt_result.kind == AttemptKind.SYNC_RESPONSE:
            body = getattr(attempt_result, "response_body", None)
            if body:
                if isinstance(body, bytes):
                    body_text = body.decode("utf-8", errors="replace")
                elif isinstance(body, (dict, list)):
                    body_text = json.dumps(body, ensure_ascii=False)
                else:
                    body_text = str(body)
                rule_action = self._check_provider_failover_rules(
                    candidate, is_success=True, response_text=body_text
                )
                if rule_action == FailoverAction.CONTINUE:
                    self._record_attempt_failure(
                        record_id,
                        Exception("success_failover_pattern matched"),
                        200,
                    )
                    raise StreamProbeError(
                        "Success failover pattern matched",
                        http_status=200,
                    )

        self._record_attempt_success(record_id, attempt_result)
        return attempt_result

    def _record_attempt_success(self, record_id: str | None, attempt_result: AttemptResult) -> None:
        """Mark attempt record as success/streaming."""
        if not record_id:
            return
        if attempt_result.kind == AttemptKind.STREAM:
            self._update_record(
                record_id,
                status="streaming",
                status_code=attempt_result.http_status,
            )
        else:
            self._update_record(
                record_id,
                status="success",
                status_code=attempt_result.http_status,
                finished_at=datetime.now(timezone.utc),
            )
        self.db.commit()

    def _record_attempt_failure(
        self, record_id: str | None, exc: Exception, status_code: int | None = None
    ) -> None:
        """Mark attempt record as failed."""
        if not record_id:
            return
        self._update_record(
            record_id,
            status="failed",
            status_code=status_code,
            error_type=type(exc).__name__,
            error_message=self._sanitize(str(exc)),
            finished_at=datetime.now(timezone.utc),
        )
        self.db.commit()

    async def _handle_attempt_error(
        self,
        exc: Exception,
        *,
        candidate: ProviderCandidate,
        candidate_index: int,
        retry_index: int,
        max_retries: int,
        record_id: str | None,
        attempt_count: int,
        max_attempts: int | None,
        execution_error_handler: Any,
        retry_policy: RetryPolicy,
        candidate_record_map: dict[tuple[int, int], str] | None,
        candidates: list[ProviderCandidate],
        request_id: str | None,
        candidate_keys_fallback: list[CandidateKey],
    ) -> AttemptErrorOutcome:
        """
        Handle attempt exception: delegate to external/internal handler, update records.

        Returns:
            AttemptErrorOutcome; stop_result is non-None only when action==STOP.
        """
        outcome = await self._classify_attempt_error(
            exc,
            candidate=candidate,
            candidate_index=candidate_index,
            retry_index=retry_index,
            has_retry_left=retry_index + 1 < max_retries,
            max_retries=max_retries,
            record_id=record_id,
            attempt_count=attempt_count,
            max_attempts=max_attempts,
            execution_error_handler=execution_error_handler,
        )

        if outcome.action == FailoverAction.STOP:
            if retry_policy.mode == RetryMode.PRE_EXPAND and candidate_record_map:
                self._mark_remaining_slots_unused(
                    candidate_record_map=candidate_record_map,
                    candidates=candidates,
                    success_candidate_idx=candidate_index,
                    success_retry_idx=retry_index,
                    retry_policy=retry_policy,
                )
            outcome.stop_result = ExecutionResult(
                success=False,
                error_type=type(exc).__name__,
                error_message=self._sanitize(str(exc)),
                last_status_code=outcome.last_status_code or None,
                candidate_keys=self._get_candidate_keys(
                    request_id=request_id,
                    fallback=candidate_keys_fallback,
                    candidates=candidates,
                ),
                attempt_count=attempt_count,
            )

        return outcome

    async def _classify_attempt_error(
        self,
        exc: Exception,
        *,
        candidate: ProviderCandidate,
        candidate_index: int,
        retry_index: int,
        has_retry_left: bool,
        max_retries: int,
        record_id: str | None,
        attempt_count: int,
        max_attempts: int | None,
        execution_error_handler: Any,
    ) -> AttemptErrorOutcome:
        """Classify an attempt error: delegate to external handler or internal classifier.

        Returns a base AttemptErrorOutcome (without stop_result). Callers add
        STOP-specific logic (e.g. PRE_EXPAND cleanup, stop_result construction) as needed.
        """
        handler_used = False
        action = FailoverAction.CONTINUE
        if execution_error_handler is not None:
            try:
                from src.services.request.executor import ExecutionError as _ExecutionError

                if isinstance(exc, _ExecutionError):
                    handler_used = True
                    action, new_max_retries = await execution_error_handler(
                        exec_err=exc,
                        candidate=candidate,
                        candidate_index=candidate_index,
                        retry_index=retry_index,
                        max_retries_for_candidate=max_retries,
                        record_id=record_id,
                        attempt_count=attempt_count,
                        max_attempts=max_attempts,
                    )
                    if new_max_retries is not None:
                        max_retries = max(max_retries, int(new_max_retries))
            except Exception:
                handler_used = False

        last_status_code: int | None = None
        if not handler_used:
            action = await self._handle_error(
                exc,
                candidate=candidate,
                has_retry_left=has_retry_left,
            )
            last_status_code = int(getattr(exc, "status_code", 0) or 0) or int(
                getattr(exc, "http_status", 0) or 0
            )
            self._record_attempt_failure(record_id, exc, last_status_code or None)

        return AttemptErrorOutcome(
            action=action,
            last_status_code=last_status_code,
            max_retries=max_retries,
        )

    def _sanitize(self, message: str, max_length: int = 200) -> str:
        if not message:
            return "request_failed"
        return _SENSITIVE_PATTERN.sub("[REDACTED]", message)[:max_length]

    def _make_candidate_key(
        self,
        *,
        candidate: ProviderCandidate,
        candidate_index: int,
        retry_index: int,
        status: str,
        skip_reason: str | None = None,
        error_type: str | None = None,
        error_message: str | None = None,
        status_code: int | None = None,
    ) -> CandidateKey:
        return CandidateKey(
            candidate_index=candidate_index,
            retry_index=retry_index,
            provider_id=str(candidate.provider.id),
            provider_name=str(candidate.provider.name),
            endpoint_id=str(candidate.endpoint.id),
            key_id=str(candidate.key.id),
            key_name=str(getattr(candidate.key, "name", "") or ""),
            auth_type=str(getattr(candidate.key, "auth_type", "") or ""),
            priority=int(getattr(candidate.key, "priority", 0) or 0),
            is_cached=bool(getattr(candidate, "is_cached", False)),
            status=status,
            skip_reason=skip_reason,
            error_type=error_type,
            error_message=error_message,
            status_code=status_code,
        )

    def _get_candidate_keys(
        self,
        *,
        request_id: str | None,
        fallback: list[CandidateKey],
        candidates: list[ProviderCandidate],
    ) -> list[CandidateKey]:
        if request_id:
            try:
                return self._recorder.get_candidate_keys(request_id)
            except Exception as exc:
                # 降级到 fallback 但记录 warning（影响审计追踪可见性）
                logger.warning(
                    "[FailoverEngine] get_candidate_keys failed, using fallback: {}",
                    self._sanitize(str(exc)),
                )
        if fallback:
            return fallback
        # fallback snapshot (no DB audit)
        result: list[CandidateKey] = []
        for idx, cand in enumerate(candidates):
            result.append(
                self._make_candidate_key(
                    candidate=cand,
                    candidate_index=idx,
                    retry_index=0,
                    status="available",
                )
            )
        return result

    def _commit_before_await(self) -> None:
        if self.db.in_transaction():
            try:
                self.db.commit()
            except Exception:
                self.db.rollback()
                raise

    def _update_record(self, record_id: str, /, **values: Any) -> None:
        self.db.execute(
            update(RequestCandidate).where(RequestCandidate.id == record_id).values(**values)
        )

    async def _ensure_record_exists(
        self,
        *,
        request_id: str,
        candidate: ProviderCandidate,
        candidate_index: int,
        retry_index: int,
        user_id: str | None,
        api_key_id: str | None,
    ) -> str:
        # Create "available" record, then caller will mark pending.
        extra = self._build_pool_extra_data(candidate)
        row = RequestCandidateService.create_candidate(
            db=self.db,
            request_id=request_id,
            candidate_index=candidate_index,
            retry_index=retry_index,
            user_id=user_id,
            api_key_id=api_key_id,
            provider_id=str(candidate.provider.id),
            endpoint_id=str(candidate.endpoint.id),
            key_id=str(candidate.key.id),
            status="available",
            is_cached=bool(getattr(candidate, "is_cached", False)),
            extra_data=extra,
        )
        return str(row.id)

    async def _create_skipped_record(
        self,
        *,
        request_id: str,
        candidate: ProviderCandidate,
        candidate_index: int,
        retry_index: int = 0,
        user_id: str | None,
        api_key_id: str | None,
        skip_reason: str | None,
    ) -> str:
        extra = self._build_pool_extra_data(candidate)
        row = RequestCandidateService.create_candidate(
            db=self.db,
            request_id=request_id,
            candidate_index=candidate_index,
            retry_index=retry_index,
            user_id=user_id,
            api_key_id=api_key_id,
            provider_id=str(candidate.provider.id),
            endpoint_id=str(candidate.endpoint.id),
            key_id=str(candidate.key.id),
            status="skipped",
            skip_reason=skip_reason,
            is_cached=bool(getattr(candidate, "is_cached", False)),
            extra_data=extra,
        )
        # ensure visible for subsequent recorder reads
        if self.db.in_transaction():
            self.db.commit()
        return str(row.id)

    def _build_pool_extra_data(self, candidate: ProviderCandidate) -> dict[str, Any]:
        extra: dict[str, Any] = {}
        pool_extra = getattr(candidate, "_pool_extra_data", None)
        if isinstance(pool_extra, dict):
            extra.update(pool_extra)

        if isinstance(candidate, PoolCandidate):
            extra["pool_group_id"] = str(getattr(candidate.provider, "id", "") or "")
            extra["pool_key_index"] = int(getattr(candidate, "_pool_key_index", 0) or 0)
            key_extra = getattr(candidate.key, "_pool_extra_data", None)
            if isinstance(key_extra, dict):
                extra.update(key_extra)
        return extra

    def _should_skip(
        self, candidate: ProviderCandidate, skip_policy: SkipPolicy
    ) -> tuple[bool, str | None]:
        if bool(getattr(candidate, "is_skipped", False)):
            return True, str(getattr(candidate, "skip_reason", None) or "scheduler_marked")

        auth_type = str(getattr(getattr(candidate, "key", None), "auth_type", "") or "api_key")
        if (
            skip_policy.supported_auth_types is not None
            and auth_type not in skip_policy.supported_auth_types
        ):
            return True, "unsupported_auth_type"

        needs_conversion = bool(getattr(candidate, "needs_conversion", False))
        if needs_conversion and not skip_policy.allow_format_conversion:
            return True, "format_conversion_not_supported"

        return False, None

    def _get_max_retries(self, candidate: ProviderCandidate, retry_policy: RetryPolicy) -> int:
        per_key_retries = self._get_pool_key_max_retries(candidate, retry_policy)
        if isinstance(candidate, PoolCandidate):
            key_count = len(candidate.pool_keys or []) or 1
            return max(1, key_count * per_key_retries)
        return per_key_retries

    def _get_pool_key_max_retries(
        self, candidate: ProviderCandidate, retry_policy: RetryPolicy
    ) -> int:
        if retry_policy.mode == RetryMode.DISABLED:
            return 1
        if retry_policy.retry_on_cached_only and not bool(getattr(candidate, "is_cached", False)):
            return 1
        provider_max = getattr(getattr(candidate, "provider", None), "max_retries", None)
        try:
            value = int(provider_max or retry_policy.max_retries or 1)
        except Exception:
            value = int(retry_policy.max_retries or 1)
        return max(1, value)

    async def _handle_error(
        self,
        error: Exception,
        *,
        candidate: ProviderCandidate,
        has_retry_left: bool,
    ) -> FailoverAction:
        # 检查提供商级别的错误终止规则
        error_text = self._extract_error_text(error)
        status_code = int(getattr(error, "status_code", 0) or 0) or int(
            getattr(error, "http_status", 0) or 0
        )
        # ExecutionError wrapping: check cause for status_code
        if not status_code:
            cause = getattr(error, "cause", None)
            if cause is not None:
                status_code = int(getattr(cause, "status_code", 0) or 0) or int(
                    getattr(cause, "http_status", 0) or 0
                )
        if error_text:
            rule_action = self._check_provider_failover_rules(
                candidate,
                is_success=False,
                response_text=error_text,
                status_code=status_code or None,
            )
            if rule_action is not None:
                return rule_action

        # 默认全部转移: ErrorClassifier 结果统一映射为 CONTINUE/RETRY，不再 STOP
        action = self._error_classifier.classify(error, has_retry_left=has_retry_left)
        if action == ErrorAction.CONTINUE:
            return FailoverAction.RETRY
        return FailoverAction.CONTINUE

    def _check_provider_failover_rules(
        self,
        candidate: ProviderCandidate,
        *,
        is_success: bool,
        response_text: str,
        status_code: int | None = None,
    ) -> FailoverAction | None:
        """检查提供商级别的故障转移规则。返回 None 表示无规则命中，使用默认行为。"""
        config = getattr(candidate.provider, "config", None) or {}
        rules = config.get("failover_rules")
        if not rules or not isinstance(rules, dict):
            return None

        compiled = self._get_compiled_patterns(rules)

        if is_success:
            for regex, rule in compiled.get("success", []):
                if regex.search(response_text):
                    logger.info(
                        "[FailoverEngine] 成功转移规则命中: pattern={}, provider={}",
                        rule.get("pattern", ""),
                        candidate.provider.name,
                    )
                    return FailoverAction.CONTINUE
        else:
            for regex, rule in compiled.get("error", []):
                # 检查状态码过滤
                rule_status_codes = rule.get("status_codes")
                if rule_status_codes and status_code not in rule_status_codes:
                    continue
                if regex.search(response_text):
                    logger.info(
                        "[FailoverEngine] 错误终止规则命中: pattern={}, status_code={}, provider={}",
                        rule.get("pattern", ""),
                        status_code,
                        candidate.provider.name,
                    )
                    return FailoverAction.STOP

        return None

    @staticmethod
    def _get_compiled_patterns(
        rules: dict[str, Any],
    ) -> dict[str, list[tuple[re.Pattern[str], dict[str, Any]]]]:
        """编译 failover_rules 中的正则模式。

        编译结果缓存在 rules dict 的 _compiled 键上，避免每次请求都重复编译。
        """
        cached = rules.get("_compiled")
        if cached is not None:
            return cached

        result: dict[str, list[tuple[re.Pattern[str], dict[str, Any]]]] = {
            "success": [],
            "error": [],
        }
        for rule in rules.get("success_failover_patterns", []):
            pattern = rule.get("pattern", "")
            if pattern:
                try:
                    result["success"].append((re.compile(pattern), rule))
                except re.error:
                    pass
        for rule in rules.get("error_stop_patterns", []):
            pattern = rule.get("pattern", "")
            if pattern:
                try:
                    result["error"].append((re.compile(pattern), rule))
                except re.error:
                    pass
        rules["_compiled"] = result
        return result

    @staticmethod
    def _extract_error_text(error: Exception) -> str:
        """从异常中提取错误响应文本。"""
        # ExecutionError wrapping
        cause = getattr(error, "cause", None)
        if cause is not None:
            error = cause

        # httpx.HTTPStatusError
        response = getattr(error, "response", None)
        if response is not None:
            try:
                return response.text or ""
            except Exception:
                pass

        # upstream_response / upstream_error attribute
        for attr in ("upstream_response", "upstream_error", "error_message"):
            val = getattr(error, attr, None)
            if val:
                return str(val)

        return str(error)

    async def _probe_stream_first_chunk(
        self,
        *,
        attempt_result: AttemptResult,
        record_id: str | None,
        candidate: ProviderCandidate | None = None,
    ) -> AttemptResult:
        """
        Probe first chunk for a streaming response.

        Strong constraints:
        - Must have timeout.
        - Empty stream before first chunk is treated as probe failure (eligible for failover).
        """
        assert attempt_result.kind == AttemptKind.STREAM
        assert attempt_result.stream_iterator is not None

        original_iterator = attempt_result.stream_iterator
        try:
            first_chunk = await asyncio.wait_for(
                original_iterator.__anext__(),
                timeout=self.STREAM_FIRST_CHUNK_TIMEOUT_SECONDS,
            )
        except asyncio.TimeoutError as exc:
            raise StreamProbeError(
                "Timeout waiting for first chunk",
                http_status=attempt_result.http_status,
                original_exception=exc,
            ) from exc
        except StopAsyncIteration as exc:
            raise StreamProbeError(
                "Empty stream: no data received before EOF",
                http_status=attempt_result.http_status,
                original_exception=exc,
            ) from exc
        except Exception as exc:
            raise StreamProbeError(
                f"Failed to read first chunk: {exc}",
                http_status=attempt_result.http_status,
                original_exception=exc,
            ) from exc

        # Check success_failover_patterns on first chunk
        if candidate is not None and first_chunk:
            chunk_text = (
                first_chunk.decode("utf-8", errors="replace")
                if isinstance(first_chunk, bytes)
                else str(first_chunk)
            )
            rule_action = self._check_provider_failover_rules(
                candidate, is_success=True, response_text=chunk_text
            )
            if rule_action == FailoverAction.CONTINUE:
                raise StreamProbeError(
                    "Success failover pattern matched in first chunk",
                    http_status=attempt_result.http_status,
                )

        wrapped = self._wrap_stream_with_finalizer(
            first_chunk=first_chunk,
            original_iterator=original_iterator,
            record_id=record_id,
        )
        return AttemptResult(
            kind=AttemptKind.STREAM,
            http_status=attempt_result.http_status,
            http_headers=attempt_result.http_headers,
            stream_iterator=wrapped,
            raw_response=attempt_result.raw_response,
        )

    def _wrap_stream_with_finalizer(
        self,
        *,
        first_chunk: bytes,
        original_iterator: AsyncIterator[bytes],
        record_id: str | None,
    ) -> AsyncIterator[bytes]:
        async def _gen() -> AsyncIterator[bytes]:
            yield first_chunk
            try:
                async for chunk in original_iterator:
                    yield chunk
            except Exception as exc:
                # Best-effort: mark stream interrupted using a new session (stream may outlive request session).
                if record_id:
                    self._mark_record_stream_interrupted(record_id, exc)
                raise

        return _gen()

    def _mark_record_stream_interrupted(self, record_id: str, exc: Exception) -> None:
        try:
            from src.database import create_session

            with create_session() as db:
                db.execute(
                    update(RequestCandidate)
                    .where(RequestCandidate.id == record_id)
                    .values(
                        status="stream_interrupted",
                        error_type=type(exc).__name__,
                        error_message=self._sanitize(str(exc)),
                        finished_at=datetime.now(timezone.utc),
                    )
                )
                db.commit()
        except Exception as inner:
            logger.debug(
                "[FailoverEngine] Failed to mark stream_interrupted: {}",
                self._sanitize(str(inner)),
            )

    def _mark_candidate_skipped(
        self,
        *,
        candidate_record_map: dict[tuple[int, int], str],
        candidate_index: int,
        candidate: ProviderCandidate,
        retry_policy: RetryPolicy,
        skip_reason: str | None,
    ) -> None:
        max_retries = self._get_max_retries(candidate, retry_policy)
        now = datetime.now(timezone.utc)
        for retry_index in range(max_retries):
            record_id = candidate_record_map.get((candidate_index, retry_index))
            if record_id:
                self._update_record(
                    record_id,
                    status="skipped",
                    skip_reason=skip_reason,
                    finished_at=now,
                )
        self.db.commit()

    def _mark_remaining_slots_unused(
        self,
        *,
        candidate_record_map: dict[tuple[int, int], str],
        candidates: list[ProviderCandidate],
        success_candidate_idx: int,
        success_retry_idx: int,
        retry_policy: RetryPolicy,
    ) -> None:
        now = datetime.now(timezone.utc)
        for candidate_idx, cand in enumerate(candidates):
            max_retries = self._get_max_retries(cand, retry_policy)
            for retry_idx in range(max_retries):
                if candidate_idx < success_candidate_idx:
                    continue
                if candidate_idx == success_candidate_idx and retry_idx <= success_retry_idx:
                    continue
                record_id = candidate_record_map.get((candidate_idx, retry_idx))
                if record_id:
                    self._update_record(
                        record_id,
                        status="unused",
                        finished_at=now,
                    )
        self.db.commit()

    def _mark_candidate_remaining_retries_unused(
        self,
        *,
        candidate_record_map: dict[tuple[int, int], str],
        candidate_idx: int,
        from_retry_idx: int,
        retry_policy: RetryPolicy,
    ) -> None:
        # Only meaningful for PRE_EXPAND.
        # We don't have access to candidate object list here, so infer max_retries from map keys.
        # Fallback to retry_policy.max_retries.
        now = datetime.now(timezone.utc)
        # try best-effort upper bound
        upper = max(
            (ri for (ci, ri) in candidate_record_map.keys() if ci == candidate_idx),
            default=retry_policy.max_retries - 1,
        )
        for retry_idx in range(from_retry_idx, upper + 1):
            record_id = candidate_record_map.get((candidate_idx, retry_idx))
            if record_id:
                self._update_record(record_id, status="unused", finished_at=now)
        self.db.commit()

    def _mark_retry_indices_status(
        self,
        *,
        candidate_record_map: dict[tuple[int, int], str],
        candidate_idx: int,
        retry_indices: range,
        status: str,
        skip_reason: str | None = None,
    ) -> None:
        now = datetime.now(timezone.utc)
        for retry_idx in retry_indices:
            record_id = candidate_record_map.get((candidate_idx, retry_idx))
            if not record_id:
                continue
            values: dict[str, Any] = {"status": status, "finished_at": now}
            if status == "skipped":
                values["skip_reason"] = skip_reason
            self._update_record(record_id, **values)
        self.db.commit()

    def _mark_all_remaining_available_unused(
        self, candidate_record_map: dict[tuple[int, int], str]
    ) -> None:
        # As a safety net: do not leave available records behind in PRE_EXPAND mode.
        try:
            ids = list(candidate_record_map.values())
            if not ids:
                return
            now = datetime.now(timezone.utc)
            self.db.execute(
                update(RequestCandidate)
                .where(RequestCandidate.id.in_(ids))
                .where(RequestCandidate.status == "available")
                .values(status="unused", finished_at=now)
            )
            self.db.commit()
        except Exception:
            self.db.rollback()
            raise
