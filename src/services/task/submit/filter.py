from __future__ import annotations

from dataclasses import dataclass
from typing import Any

from sqlalchemy import update
from sqlalchemy.orm import Session

from src.config.settings import config
from src.models.database import RequestCandidate
from src.services.billing.rule_service import BillingRuleLookupResult, BillingRuleService
from src.services.system.config import SystemConfigService


@dataclass(slots=True)
class CandidateAttemptPlan:
    """候选尝试计划（通过过滤后可进入提交阶段）。"""

    record_id: str | None
    rule_lookup: BillingRuleLookupResult | None


class AsyncSubmitFilterService:
    """异步提交候选过滤服务。"""

    def __init__(self, db: Session) -> None:
        self.db = db

    @staticmethod
    def build_candidate_info(*, idx: int, candidate: Any) -> dict[str, Any]:
        auth_type = getattr(candidate.key, "auth_type", "api_key") or "api_key"
        return {
            "index": idx,
            "provider_id": candidate.provider.id,
            "provider_name": candidate.provider.name,
            "endpoint_id": candidate.endpoint.id,
            "key_id": candidate.key.id,
            "key_name": getattr(candidate.key, "name", None),
            "auth_type": auth_type,
            "priority": getattr(candidate.key, "priority", 0) or 0,
            "is_cached": bool(getattr(candidate, "is_cached", False)),
        }

    def prepare_candidate_for_attempt(
        self,
        *,
        idx: int,
        candidate: Any,
        record_map: dict[tuple[int, int], str],
        candidate_info: dict[str, Any],
        task_type: str,
        model_name: str,
        supported_auth_types: set[str] | None,
        allow_format_conversion: bool,
    ) -> CandidateAttemptPlan | None:
        record_id = record_map.get((idx, 0))
        auth_type = candidate_info.get("auth_type", "api_key")

        # Scheduler marked skip
        if getattr(candidate, "is_skipped", False):
            skip_reason = getattr(candidate, "skip_reason", None) or "skipped"
            self._mark_skip(
                record_id=record_id, candidate_info=candidate_info, skip_reason=skip_reason
            )
            return None

        # Format conversion checks
        needs_conversion = bool(getattr(candidate, "needs_conversion", False))
        if needs_conversion:
            # 1. handler-level switch
            if not allow_format_conversion:
                self._mark_skip(
                    record_id=record_id,
                    candidate_info=candidate_info,
                    skip_reason="format_conversion_not_supported",
                )
                return None

            # 2. global switch (from database config)
            if not SystemConfigService.is_format_conversion_enabled(self.db):
                self._mark_skip(
                    record_id=record_id,
                    candidate_info=candidate_info,
                    skip_reason="format_conversion_disabled",
                    extra_info={"format_conversion_enabled": False},
                )
                return None

        # auth_type filter
        if supported_auth_types is not None and auth_type not in supported_auth_types:
            self._mark_skip(
                record_id=record_id,
                candidate_info=candidate_info,
                skip_reason=f"unsupported_auth_type:{auth_type}",
            )
            return None

        # billing rule filter
        rule_lookup: BillingRuleLookupResult | None = None
        has_billing_rule = True
        if config.billing_require_rule:
            rule_lookup = BillingRuleService.find_rule(
                self.db,
                provider_id=candidate.provider.id,
                model_name=model_name,
                task_type=task_type,
            )
            has_billing_rule = rule_lookup is not None
            if not has_billing_rule:
                self._mark_skip(
                    record_id=record_id,
                    candidate_info=candidate_info,
                    skip_reason="billing_rule_missing",
                    extra_info={"has_billing_rule": False},
                )
                return None
        candidate_info["has_billing_rule"] = has_billing_rule

        return CandidateAttemptPlan(record_id=record_id, rule_lookup=rule_lookup)

    def _mark_skip(
        self,
        *,
        record_id: str | None,
        candidate_info: dict[str, Any],
        skip_reason: str,
        extra_info: dict[str, Any] | None = None,
    ) -> None:
        candidate_info.update({"skipped": True, "skip_reason": skip_reason})
        if extra_info:
            candidate_info.update(extra_info)
        if record_id:
            self.db.execute(
                update(RequestCandidate)
                .where(RequestCandidate.id == record_id)
                .values(status="skipped", skip_reason=skip_reason)
            )
