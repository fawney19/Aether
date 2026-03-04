from __future__ import annotations

from collections.abc import Callable
from dataclasses import dataclass
from typing import Any

import httpx

from src.services.billing.rule_service import BillingRuleLookupResult
from src.services.candidate.submit import SubmitOutcome


@dataclass(slots=True)
class SubmitPayloadParseResult:
    """提交响应解析结果。"""

    payload: dict[str, Any] | None
    error_type: str | None = None
    error_message: str | None = None


class AsyncSubmitOutcomeBuilderService:
    """异步提交结果构建服务。"""

    def __init__(self, *, sanitize: Callable[[str], str]) -> None:
        self._sanitize = sanitize

    def parse_payload(self, *, response: httpx.Response) -> SubmitPayloadParseResult:
        payload: dict[str, Any] | None = None
        try:
            data = response.json()
            if isinstance(data, dict):
                payload = data
        except Exception as exc:
            return SubmitPayloadParseResult(
                payload=None,
                error_type=type(exc).__name__,
                error_message=self._sanitize(str(exc)),
            )
        return SubmitPayloadParseResult(payload=payload)

    @staticmethod
    def build_success_outcome(
        *,
        candidate: Any,
        candidate_keys: list[dict[str, Any]],
        external_task_id: str,
        rule_lookup: BillingRuleLookupResult | None,
        payload: dict[str, Any] | None,
        response: httpx.Response,
    ) -> SubmitOutcome:
        return SubmitOutcome(
            candidate=candidate,
            candidate_keys=candidate_keys,
            external_task_id=external_task_id,
            rule_lookup=rule_lookup,
            upstream_payload=payload,
            upstream_headers=dict(response.headers),
            upstream_status_code=response.status_code,
        )
