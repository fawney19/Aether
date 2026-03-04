from __future__ import annotations

from collections.abc import Callable
from typing import Any

import httpx
from sqlalchemy.orm import Session

from src.core.logger import logger
from src.services.billing.rule_service import BillingRuleLookupResult
from src.services.candidate.submit import SubmitOutcome, UpstreamClientRequestError
from src.services.task.submit.outcome_builder import (
    AsyncSubmitOutcomeBuilderService,
)
from src.services.task.submit.record import AsyncSubmitRecordService
from src.services.task.submit.rule_decider import AsyncSubmitRuleDeciderService


class AsyncSubmitResponseService:
    """异步提交响应判定服务。"""

    def __init__(
        self,
        db: Session,
        *,
        record_ops: AsyncSubmitRecordService,
        sanitize: Callable[[str], str],
        extract_response_text: Callable[[httpx.Response], str],
        match_provider_failover_rule: Callable[..., str | None],
    ) -> None:
        self.db = db
        self._record_ops = record_ops
        self._sanitize = sanitize
        self._extract_response_text = extract_response_text
        self._rule_decider = AsyncSubmitRuleDeciderService(
            match_provider_failover_rule=match_provider_failover_rule
        )
        self._outcome_builder = AsyncSubmitOutcomeBuilderService(sanitize=sanitize)

    def handle_submit_exception(
        self,
        *,
        record_id: str | None,
        candidate_info: dict[str, Any],
        exc: Exception,
    ) -> tuple[None, None]:
        error_type = type(exc).__name__
        error_msg = self._sanitize(str(exc))
        candidate_info.update(
            {
                "attempt_status": "exception",
                "error_type": error_type,
                "error_message": error_msg,
            }
        )
        self._record_ops.mark_failed(
            record_id=record_id,
            error_type=error_type,
            error_message=error_msg,
        )
        return None, None

    def handle_submit_response(
        self,
        *,
        candidate: Any,
        record_id: str | None,
        candidate_info: dict[str, Any],
        candidate_keys: list[dict[str, Any]],
        rule_lookup: BillingRuleLookupResult | None,
        response: httpx.Response,
        extract_external_task_id: Any,
    ) -> tuple[SubmitOutcome | None, int | None]:
        last_status_code = int(getattr(response, "status_code", 0) or 0)

        if response.status_code >= 400:
            error_text = self._extract_response_text(response)
            error_msg = self._sanitize(error_text)
            candidate_info.update(
                {
                    "attempt_status": "http_error",
                    "status_code": response.status_code,
                    "error_message": error_msg,
                }
            )
            self._record_ops.mark_failed(
                record_id=record_id,
                status_code=response.status_code,
                error_type="http_error",
                error_message=error_msg,
            )

            stop_pattern = self._rule_decider.detect_error_stop_pattern(
                candidate=candidate,
                response_text=error_text,
                status_code=response.status_code,
            )
            if stop_pattern:
                logger.info(
                    "[TaskService] 错误终止规则命中: pattern={}, status_code={}, provider={}",
                    stop_pattern,
                    response.status_code,
                    candidate.provider.name,
                )
                candidate_info["stop_rule_pattern"] = stop_pattern
                try:
                    self.db.commit()
                except Exception:
                    self.db.rollback()
                raise UpstreamClientRequestError(
                    response=response,
                    candidate_keys=candidate_keys,
                )
            return None, last_status_code

        success_text = self._extract_response_text(response)
        success_continue_pattern = self._rule_decider.detect_success_failover_pattern(
            candidate=candidate,
            response_text=success_text,
            status_code=response.status_code,
        )
        if success_continue_pattern:
            logger.info(
                "[TaskService] 成功转移规则命中: pattern={}, status_code={}, provider={}",
                success_continue_pattern,
                response.status_code,
                candidate.provider.name,
            )
            failover_reason = f"success_failover_rule_matched:{success_continue_pattern}"
            candidate_info.update(
                {
                    "attempt_status": "success_failover",
                    "status_code": response.status_code,
                    "error_message": failover_reason,
                    "success_rule_pattern": success_continue_pattern,
                }
            )
            self._record_ops.mark_failed(
                record_id=record_id,
                status_code=response.status_code,
                error_type="success_failover_pattern",
                error_message=failover_reason,
            )
            return None, last_status_code

        parse_result = self._outcome_builder.parse_payload(response=response)
        if parse_result.error_type:
            candidate_info.update(
                {
                    "attempt_status": "invalid_json",
                    "error_type": parse_result.error_type,
                    "error_message": parse_result.error_message,
                }
            )
            self._record_ops.mark_failed(
                record_id=record_id,
                status_code=response.status_code,
                error_type="invalid_json",
                error_message=parse_result.error_message or "invalid_json",
            )
            return None, last_status_code

        payload = parse_result.payload
        external_task_id = extract_external_task_id(payload or {})
        if not external_task_id:
            candidate_info.update(
                {
                    "attempt_status": "empty_task_id",
                    "error_message": "Upstream returned empty task id",
                }
            )
            self._record_ops.mark_failed(
                record_id=record_id,
                status_code=response.status_code,
                error_type="empty_task_id",
                error_message="Upstream returned empty task id",
            )
            return None, last_status_code

        # Success
        candidate_info.update({"attempt_status": "success", "selected": True})
        self._record_ops.mark_success(
            record_id=record_id,
            status_code=response.status_code,
        )
        try:
            self.db.commit()
        except Exception:
            self.db.rollback()

        return (
            self._outcome_builder.build_success_outcome(
                candidate=candidate,
                candidate_keys=candidate_keys,
                external_task_id=str(external_task_id),
                rule_lookup=rule_lookup,
                payload=payload,
                response=response,
            ),
            last_status_code,
        )
