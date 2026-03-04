from __future__ import annotations

import re
from typing import Any

import httpx
from sqlalchemy.orm import Session

from src.models.database import ApiKey
from src.services.candidate.failover import FailoverEngine
from src.services.candidate.submit import SubmitOutcome
from src.services.task.submit import ApplyPoolReorderFn, ExpandPoolCandidatesFn
from src.services.task.submit.execute import AsyncSubmitExecutionService
from src.services.task.submit.prepare import AsyncSubmitPreparationService

_SENSITIVE_PATTERN = re.compile(
    r"(api[_-]?key|token|bearer|authorization)[=:\s]+\S+",
    re.IGNORECASE,
)


class AsyncTaskSubmitService:
    """异步任务提交应用服务（候选选择 + 故障转移）。"""

    def __init__(
        self,
        db: Session,
        redis_client: Any | None,
        *,
        apply_pool_reorder: ApplyPoolReorderFn,
        expand_pool_candidates_for_async_submit: ExpandPoolCandidatesFn,
    ) -> None:
        self.db = db
        self.redis = redis_client
        self._apply_pool_reorder = apply_pool_reorder
        self._expand_pool_candidates_for_async_submit = expand_pool_candidates_for_async_submit
        self._prepare_ops = AsyncSubmitPreparationService(
            db,
            redis_client,
            sanitize=self._sanitize,
        )
        self._execute_ops = AsyncSubmitExecutionService(
            db,
            sanitize=self._sanitize,
            extract_response_text=self._extract_response_text,
            match_provider_failover_rule=self._match_provider_failover_rule,
        )

    @staticmethod
    def _sanitize(message: str, max_length: int = 200) -> str:
        if not message:
            return "request_failed"
        return _SENSITIVE_PATTERN.sub("[REDACTED]", message)[:max_length]

    @staticmethod
    def _extract_response_text(response: httpx.Response) -> str:
        try:
            return response.text or ""
        except Exception:
            return ""

    @staticmethod
    def _match_provider_failover_rule(
        candidate: Any,
        *,
        is_success: bool,
        response_text: str,
        status_code: int | None = None,
    ) -> str | None:
        provider_config = getattr(candidate.provider, "config", None) or {}
        rules = provider_config.get("failover_rules")
        if not rules or not isinstance(rules, dict):
            return None

        compiled = FailoverEngine._get_compiled_patterns(rules)
        key = "success" if is_success else "error"

        for regex, rule in compiled.get(key, []):
            if not is_success:
                rule_status_codes = rule.get("status_codes")
                if rule_status_codes and status_code not in rule_status_codes:
                    continue
            if regex.search(response_text):
                return rule.get("pattern", "")

        return None

    async def submit_with_failover(
        self,
        *,
        api_format: str,
        model_name: str,
        affinity_key: str,
        user_api_key: ApiKey,
        request_id: str | None,
        task_type: str,
        submit_func: Any,
        extract_external_task_id: Any,
        supported_auth_types: set[str] | None = None,
        allow_format_conversion: bool = False,
        capability_requirements: dict[str, bool] | None = None,
        max_candidates: int | None = None,
        request_body: dict[str, Any] | None = None,
    ) -> SubmitOutcome:
        """
        异步提交入口。

        行为保持与原 TaskService.submit_with_failover 一致：
        - 按候选顺序依次尝试（提交阶段不做单候选重试）
        - 记录 RequestCandidate 审计行
        - 命中 error_stop_patterns 时立即停止并抛出上游错误
        - 命中 success_failover_patterns 时继续尝试下一个候选
        """
        # IMPORTANT:
        # This method awaits upstream HTTP calls. If we have an open DB transaction before awaiting,
        # the connection can be held for a long time (pool exhaustion under concurrency).
        #
        # Also note SQLAlchemy's default expire_on_commit=True would expire ORM objects and may
        # trigger unexpected lazy DB loads after we commit (potentially during the await).
        # We disable it temporarily to keep candidate/provider/key objects in-memory.
        original_expire_on_commit = getattr(self.db, "expire_on_commit", True)
        self.db.expire_on_commit = False

        try:
            prepared = await self._prepare_ops.prepare_candidates(
                api_format=api_format,
                model_name=model_name,
                affinity_key=affinity_key,
                user_api_key=user_api_key,
                request_id=request_id,
                capability_requirements=capability_requirements,
                request_body=request_body,
                max_candidates=max_candidates,
                apply_pool_reorder=self._apply_pool_reorder,
                expand_pool_candidates_for_async_submit=(
                    self._expand_pool_candidates_for_async_submit
                ),
            )

            return await self._execute_ops.execute_submit_loop(
                candidates=prepared.candidates,
                record_map=prepared.record_map,
                task_type=task_type,
                model_name=model_name,
                submit_func=submit_func,
                extract_external_task_id=extract_external_task_id,
                supported_auth_types=supported_auth_types,
                allow_format_conversion=allow_format_conversion,
            )
        finally:
            # Restore Session behavior for the rest of the request lifecycle.
            self.db.expire_on_commit = original_expire_on_commit
