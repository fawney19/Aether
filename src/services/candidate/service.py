from __future__ import annotations

import re
from datetime import datetime, timezone
from typing import Any

import httpx
from sqlalchemy import update
from sqlalchemy.orm import Session

from src.config.settings import config
from src.core.exceptions import ProviderNotAvailableException
from src.core.logger import logger
from src.models.database import ApiKey, RequestCandidate
from src.services.billing.rule_service import BillingRuleLookupResult, BillingRuleService
from src.services.cache.aware_scheduler import ProviderCandidate, get_cache_aware_scheduler
from src.services.candidate.submit import (
    AllCandidatesFailedError,
    SubmitOutcome,
    UpstreamClientRequestError,
)
from src.services.orchestration.error_classifier import ErrorClassifier
from src.services.system.config import SystemConfigService

from .recorder import CandidateRecorder
from .resolver import CandidateResolver
from .schema import CandidateKey

_SENSITIVE_PATTERN = re.compile(
    r"(api[_-]?key|token|bearer|authorization)[=:\s]+\S+",
    re.IGNORECASE,
)


def _sanitize(message: str, max_length: int = 200) -> str:
    if not message:
        return "request_failed"
    return _SENSITIVE_PATTERN.sub("[REDACTED]", message)[:max_length]


class CandidateService:
    """
    CandidateService (Facade).

    Phase2 note: this is introduced as a new domain entrypoint. Legacy orchestrators
    still exist and will be migrated gradually to use this service.
    """

    def __init__(self, db: Session, redis_client: Any | None = None) -> None:
        self.db = db
        self.redis = redis_client
        self._cache_scheduler = None
        self._resolver: CandidateResolver | None = None
        self._error_classifier: ErrorClassifier | None = None
        self._recorder = CandidateRecorder(db)

    async def _ensure_initialized(self) -> None:
        if self._cache_scheduler is not None:
            return

        priority_mode = SystemConfigService.get_config(
            self.db,
            "provider_priority_mode",
            "provider",
        )
        scheduling_mode = SystemConfigService.get_config(
            self.db,
            "scheduling_mode",
            "cache_affinity",
        )
        self._cache_scheduler = await get_cache_aware_scheduler(
            self.redis,
            priority_mode=priority_mode,
            scheduling_mode=scheduling_mode,
        )
        self._resolver = CandidateResolver(db=self.db, cache_scheduler=self._cache_scheduler)
        self._error_classifier = ErrorClassifier(db=self.db, cache_scheduler=self._cache_scheduler)

    async def resolve(
        self,
        *,
        api_format: str,
        model_name: str,
        affinity_key: str,
        user_api_key: ApiKey | None = None,
        request_id: str | None = None,
        is_stream: bool = False,
        capability_requirements: dict[str, bool] | None = None,
        preferred_key_ids: list[str] | None = None,
    ) -> tuple[list[ProviderCandidate], str]:
        await self._ensure_initialized()
        assert self._resolver is not None
        return await self._resolver.fetch_candidates(
            api_format=api_format,
            model_name=model_name,
            affinity_key=affinity_key,
            user_api_key=user_api_key,
            request_id=request_id,
            is_stream=is_stream,
            capability_requirements=capability_requirements,
            preferred_key_ids=preferred_key_ids,
        )

    def _should_stop_on_http_error(self, *, status_code: int, error_text: str) -> bool:
        """
        Decide whether an upstream HTTP error is a client error (no failover).

        Rules:
        - 401/403/429 are usually key/permission/ratelimit issues -> allow failover
        - other 4xx: stop only if ErrorClassifier says it's a client error
        """
        if status_code in (401, 403, 429):
            return False
        if 400 <= status_code < 500:
            assert self._error_classifier is not None
            return self._error_classifier.is_client_error(error_text)
        return False

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
    ) -> SubmitOutcome:
        """
        Submit async task with failover, returning the selected candidate + external_task_id.

        Phase2 submit entrypoint (replaces legacy submit orchestrator).
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
        await self._ensure_initialized()
        assert self._resolver is not None
        try:
            candidates, _global_model_id = await self._resolver.fetch_candidates(
                api_format=api_format,
                model_name=model_name,
                affinity_key=affinity_key,
                user_api_key=user_api_key,
                request_id=request_id,
                is_stream=False,
                capability_requirements=capability_requirements,
            )

            if not candidates:
                raise ProviderNotAvailableException("No candidates available")

            if max_candidates is not None and max_candidates > 0:
                candidates = candidates[:max_candidates]

            # Pre-create RequestCandidate records (no retry expand for async submit stage)
            record_map: dict[tuple[int, int], str] = {}
            if request_id:
                try:
                    record_map = self.create_candidate_records(
                        candidates=candidates,
                        request_id=request_id,
                        user_api_key=user_api_key,
                        required_capabilities=capability_requirements,
                        expand_retries=False,
                    )
                except Exception as exc:
                    logger.warning(
                        "[CandidateService] Failed to create candidate records: %s",
                        _sanitize(str(exc)),
                    )
                    record_map = {}

            candidate_keys: list[dict[str, Any]] = []
            eligible_count = 0
            last_status_code: int | None = None

            for idx, cand in enumerate(candidates):
                now = datetime.now(timezone.utc)
                auth_type = getattr(cand.key, "auth_type", "api_key") or "api_key"

                candidate_info: dict[str, Any] = {
                    "index": idx,
                    "provider_id": cand.provider.id,
                    "provider_name": cand.provider.name,
                    "endpoint_id": cand.endpoint.id,
                    "key_id": cand.key.id,
                    "key_name": getattr(cand.key, "name", None),
                    "auth_type": auth_type,
                    "priority": getattr(cand.key, "priority", 0) or 0,
                    "is_cached": bool(getattr(cand, "is_cached", False)),
                }
                candidate_keys.append(candidate_info)

                record_id = record_map.get((idx, 0))

                # Scheduler marked skip
                if getattr(cand, "is_skipped", False):
                    skip_reason = getattr(cand, "skip_reason", None) or "skipped"
                    candidate_info.update({"skipped": True, "skip_reason": skip_reason})
                    if record_id:
                        # record is usually already skipped, but keep it consistent
                        self.db.execute(
                            update(RequestCandidate)
                            .where(RequestCandidate.id == record_id)
                            .values(status="skipped", skip_reason=skip_reason)
                        )
                    continue

                # Format conversion checks
                # 优先级：全局开关 ON 强制允许，全局开关 OFF 看提供商开关
                needs_conversion = bool(getattr(cand, "needs_conversion", False))
                if needs_conversion:
                    # 1. Check handler-level switch (handler 不支持则直接跳过)
                    if not allow_format_conversion:
                        skip_reason = "format_conversion_not_supported"
                        candidate_info.update({"skipped": True, "skip_reason": skip_reason})
                        if record_id:
                            self.db.execute(
                                update(RequestCandidate)
                                .where(RequestCandidate.id == record_id)
                                .values(status="skipped", skip_reason=skip_reason)
                            )
                        continue

                    # 2. Check global + provider switches (三态继承：提供商 > 全局)
                    # provider.enable_format_conversion:
                    #   - True: 明确启用
                    #   - False: 明确禁用
                    #   - None: 继承全局设置
                    from src.services.system.config import SystemConfigService

                    global_enabled = SystemConfigService.is_format_conversion_enabled(self.db)
                    provider_conversion_setting = getattr(
                        cand.provider, "enable_format_conversion", None
                    )
                    if provider_conversion_setting is not None:
                        # 提供商有明确设置，使用提供商设置
                        effective_enabled = bool(provider_conversion_setting)
                    else:
                        # 提供商未设置（None），继承全局设置
                        effective_enabled = global_enabled

                    if not effective_enabled:
                        skip_reason = "format_conversion_disabled"
                        candidate_info.update(
                            {
                                "skipped": True,
                                "skip_reason": skip_reason,
                                "global_conversion_enabled": global_enabled,
                                "provider_conversion_setting": provider_conversion_setting,
                                "effective_enabled": effective_enabled,
                            }
                        )
                        if record_id:
                            self.db.execute(
                                update(RequestCandidate)
                                .where(RequestCandidate.id == record_id)
                                .values(status="skipped", skip_reason=skip_reason)
                            )
                        continue

                # auth_type filter
                if supported_auth_types is not None and auth_type not in supported_auth_types:
                    skip_reason = f"unsupported_auth_type:{auth_type}"
                    candidate_info.update({"skipped": True, "skip_reason": skip_reason})
                    if record_id:
                        self.db.execute(
                            update(RequestCandidate)
                            .where(RequestCandidate.id == record_id)
                            .values(status="skipped", skip_reason=skip_reason)
                        )
                    continue

                # billing rule filter
                rule_lookup: BillingRuleLookupResult | None = None
                has_billing_rule = True
                if config.billing_require_rule:
                    rule_lookup = BillingRuleService.find_rule(
                        self.db,
                        provider_id=cand.provider.id,
                        model_name=model_name,
                        task_type=task_type,
                    )
                    has_billing_rule = rule_lookup is not None
                    if not has_billing_rule:
                        skip_reason = "billing_rule_missing"
                        candidate_info.update(
                            {"has_billing_rule": False, "skipped": True, "skip_reason": skip_reason}
                        )
                        if record_id:
                            self.db.execute(
                                update(RequestCandidate)
                                .where(RequestCandidate.id == record_id)
                                .values(status="skipped", skip_reason=skip_reason)
                            )
                        continue
                candidate_info["has_billing_rule"] = has_billing_rule

                eligible_count += 1

                # Mark pending
                if record_id:
                    self.db.execute(
                        update(RequestCandidate)
                        .where(RequestCandidate.id == record_id)
                        .values(status="pending", started_at=now)
                    )

                # Flush/commit BEFORE awaiting upstream submit to avoid holding DB connections
                # during potentially slow network operations.
                if self.db.in_transaction():
                    try:
                        self.db.commit()
                    except Exception:
                        self.db.rollback()
                        raise

                # Attempt submit (upstream HTTP)
                try:
                    response: httpx.Response = await submit_func(cand)
                except Exception as exc:
                    finished_at = datetime.now(timezone.utc)
                    error_msg = _sanitize(str(exc))
                    candidate_info.update(
                        {
                            "attempt_status": "exception",
                            "error_type": type(exc).__name__,
                            "error_message": error_msg,
                        }
                    )
                    if record_id:
                        self.db.execute(
                            update(RequestCandidate)
                            .where(RequestCandidate.id == record_id)
                            .values(
                                status="failed",
                                error_type=type(exc).__name__,
                                error_message=error_msg,
                                finished_at=finished_at,
                            )
                        )
                    continue

                last_status_code = int(getattr(response, "status_code", 0) or 0)

                if response.status_code >= 400:
                    finished_at = datetime.now(timezone.utc)
                    try:
                        error_text = response.text or ""
                    except Exception:
                        error_text = ""
                    error_msg = _sanitize(error_text)
                    candidate_info.update(
                        {
                            "attempt_status": "http_error",
                            "status_code": response.status_code,
                            "error_message": error_msg,
                        }
                    )
                    if record_id:
                        self.db.execute(
                            update(RequestCandidate)
                            .where(RequestCandidate.id == record_id)
                            .values(
                                status="failed",
                                status_code=response.status_code,
                                error_type="http_error",
                                error_message=error_msg,
                                finished_at=finished_at,
                            )
                        )

                    if self._should_stop_on_http_error(
                        status_code=response.status_code, error_text=error_text
                    ):
                        try:
                            self.db.commit()
                        except Exception:
                            self.db.rollback()
                        raise UpstreamClientRequestError(
                            response=response,
                            candidate_keys=candidate_keys,
                        )
                    continue

                # Parse JSON
                payload: dict[str, Any] | None = None
                try:
                    data = response.json()
                    if isinstance(data, dict):
                        payload = data
                except Exception as exc:
                    finished_at = datetime.now(timezone.utc)
                    error_msg = _sanitize(str(exc))
                    candidate_info.update(
                        {
                            "attempt_status": "invalid_json",
                            "error_type": type(exc).__name__,
                            "error_message": error_msg,
                        }
                    )
                    if record_id:
                        self.db.execute(
                            update(RequestCandidate)
                            .where(RequestCandidate.id == record_id)
                            .values(
                                status="failed",
                                status_code=response.status_code,
                                error_type="invalid_json",
                                error_message=error_msg,
                                finished_at=finished_at,
                            )
                        )
                    continue

                external_task_id = extract_external_task_id(payload or {})
                if not external_task_id:
                    finished_at = datetime.now(timezone.utc)
                    candidate_info.update(
                        {
                            "attempt_status": "empty_task_id",
                            "error_message": "Upstream returned empty task id",
                        }
                    )
                    if record_id:
                        self.db.execute(
                            update(RequestCandidate)
                            .where(RequestCandidate.id == record_id)
                            .values(
                                status="failed",
                                status_code=response.status_code,
                                error_type="empty_task_id",
                                error_message="Upstream returned empty task id",
                                finished_at=finished_at,
                            )
                        )
                    continue

                # Success
                finished_at = datetime.now(timezone.utc)
                candidate_info.update({"attempt_status": "success", "selected": True})
                if record_id:
                    self.db.execute(
                        update(RequestCandidate)
                        .where(RequestCandidate.id == record_id)
                        .values(
                            status="success",
                            status_code=response.status_code,
                            finished_at=finished_at,
                        )
                    )
                try:
                    self.db.commit()
                except Exception:
                    self.db.rollback()

                return SubmitOutcome(
                    candidate=cand,
                    candidate_keys=candidate_keys,
                    external_task_id=str(external_task_id),
                    rule_lookup=rule_lookup,
                    upstream_payload=payload,
                    upstream_headers=dict(response.headers),
                    upstream_status_code=response.status_code,
                )

            # Persist candidate records before raising
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
        finally:
            # Restore Session behavior for the rest of the request lifecycle.
            self.db.expire_on_commit = original_expire_on_commit

    def create_candidate_records(
        self,
        *,
        candidates: list[ProviderCandidate],
        request_id: str,
        user_api_key: ApiKey,
        required_capabilities: dict[str, bool] | None = None,
        expand_retries: bool = True,
    ) -> dict[tuple[int, int], str]:
        # CandidateResolver.create_candidate_records is currently the canonical implementation
        assert self._resolver is not None, "Call resolve() once before create_candidate_records()"
        return self._resolver.create_candidate_records(
            all_candidates=candidates,
            request_id=request_id,
            user_id=str(user_api_key.user_id),
            user_api_key=user_api_key,
            required_capabilities=required_capabilities,
            expand_retries=expand_retries,
        )

    def get_candidate_keys(self, request_id: str) -> list["CandidateKey"]:
        return self._recorder.get_candidate_keys(request_id)
