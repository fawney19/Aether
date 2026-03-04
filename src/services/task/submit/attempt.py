from __future__ import annotations

from collections.abc import Callable
from typing import Any

import httpx
from sqlalchemy.orm import Session

from src.services.billing.rule_service import BillingRuleLookupResult
from src.services.candidate.submit import SubmitOutcome
from src.services.task.submit.record import AsyncSubmitRecordService
from src.services.task.submit.response import AsyncSubmitResponseService


class AsyncSubmitAttemptService:
    """异步提交单候选执行编排服务。"""

    def __init__(
        self,
        db: Session,
        *,
        sanitize: Callable[[str], str],
        extract_response_text: Callable[[httpx.Response], str],
        match_provider_failover_rule: Callable[..., str | None],
    ) -> None:
        self.db = db
        self._record_ops = AsyncSubmitRecordService(db)
        self._response_ops = AsyncSubmitResponseService(
            db,
            record_ops=self._record_ops,
            sanitize=sanitize,
            extract_response_text=extract_response_text,
            match_provider_failover_rule=match_provider_failover_rule,
        )

    async def submit_candidate(
        self,
        *,
        candidate: Any,
        record_id: str | None,
        candidate_info: dict[str, Any],
        candidate_keys: list[dict[str, Any]],
        rule_lookup: BillingRuleLookupResult | None,
        submit_func: Any,
        extract_external_task_id: Any,
    ) -> tuple[SubmitOutcome | None, int | None]:
        self._record_ops.mark_pending(record_id=record_id)

        # Flush/commit BEFORE awaiting upstream submit to avoid holding DB connections.
        if self.db.in_transaction():
            try:
                self.db.commit()
            except Exception:
                self.db.rollback()
                raise

        # Attempt submit (upstream HTTP)
        try:
            response: httpx.Response = await submit_func(candidate)
        except Exception as exc:
            return self._response_ops.handle_submit_exception(
                record_id=record_id,
                candidate_info=candidate_info,
                exc=exc,
            )

        return self._response_ops.handle_submit_response(
            candidate=candidate,
            record_id=record_id,
            candidate_info=candidate_info,
            candidate_keys=candidate_keys,
            rule_lookup=rule_lookup,
            response=response,
            extract_external_task_id=extract_external_task_id,
        )
