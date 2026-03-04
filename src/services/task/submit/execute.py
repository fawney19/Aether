from __future__ import annotations

from collections.abc import Callable
from typing import Any

import httpx
from sqlalchemy.orm import Session

from src.config.settings import config
from src.services.candidate.submit import AllCandidatesFailedError, SubmitOutcome
from src.services.task.submit.attempt import AsyncSubmitAttemptService
from src.services.task.submit.filter import AsyncSubmitFilterService


class AsyncSubmitExecutionService:
    """异步提交候选执行编排服务。"""

    def __init__(
        self,
        db: Session,
        *,
        sanitize: Callable[[str], str],
        extract_response_text: Callable[[httpx.Response], str],
        match_provider_failover_rule: Callable[..., str | None],
    ) -> None:
        self.db = db
        self._filter_ops = AsyncSubmitFilterService(db)
        self._attempt_ops = AsyncSubmitAttemptService(
            db,
            sanitize=sanitize,
            extract_response_text=extract_response_text,
            match_provider_failover_rule=match_provider_failover_rule,
        )

    async def execute_submit_loop(
        self,
        *,
        candidates: list[Any],
        record_map: dict[tuple[int, int], str],
        task_type: str,
        model_name: str,
        submit_func: Any,
        extract_external_task_id: Any,
        supported_auth_types: set[str] | None,
        allow_format_conversion: bool,
    ) -> SubmitOutcome:
        candidate_keys: list[dict[str, Any]] = []
        eligible_count = 0
        last_status_code: int | None = None

        for idx, cand in enumerate(candidates):
            candidate_info = self._filter_ops.build_candidate_info(idx=idx, candidate=cand)
            candidate_keys.append(candidate_info)

            attempt_plan = self._filter_ops.prepare_candidate_for_attempt(
                idx=idx,
                candidate=cand,
                record_map=record_map,
                candidate_info=candidate_info,
                task_type=task_type,
                model_name=model_name,
                supported_auth_types=supported_auth_types,
                allow_format_conversion=allow_format_conversion,
            )
            if attempt_plan is None:
                continue

            eligible_count += 1
            outcome, status_code = await self._attempt_ops.submit_candidate(
                candidate=cand,
                record_id=attempt_plan.record_id,
                candidate_info=candidate_info,
                candidate_keys=candidate_keys,
                rule_lookup=attempt_plan.rule_lookup,
                submit_func=submit_func,
                extract_external_task_id=extract_external_task_id,
            )

            if status_code is not None:
                last_status_code = status_code
            if outcome is not None:
                return outcome

        # Persist candidate records before raising.
        try:
            self.db.commit()
        except Exception:
            self.db.rollback()

        if eligible_count == 0:
            reason = "no_eligible_candidates"
            if config.billing_require_rule:
                reason = "no_candidate_with_billing_rule"
            raise AllCandidatesFailedError(
                reason=reason,
                candidate_keys=candidate_keys,
                last_status_code=last_status_code,
            )

        raise AllCandidatesFailedError(
            reason="all_candidates_failed",
            candidate_keys=candidate_keys,
            last_status_code=last_status_code,
        )
