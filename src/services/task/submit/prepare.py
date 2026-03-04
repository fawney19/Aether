from __future__ import annotations

from collections.abc import Callable
from dataclasses import dataclass
from typing import Any

from sqlalchemy.orm import Session

from src.core.logger import logger
from src.models.database import ApiKey
from src.services.candidate.resolver import CandidateResolver
from src.services.candidate.submit import AllCandidatesFailedError
from src.services.scheduling.aware_scheduler import get_cache_aware_scheduler
from src.services.system.config import SystemConfigService
from src.services.task.submit import ApplyPoolReorderFn, ExpandPoolCandidatesFn


@dataclass(slots=True)
class PreparedSubmitCandidates:
    """异步提交前的候选准备结果。"""

    candidates: list[Any]
    record_map: dict[tuple[int, int], str]


class AsyncSubmitPreparationService:
    """异步提交候选准备服务。"""

    def __init__(
        self,
        db: Session,
        redis_client: Any | None,
        *,
        sanitize: Callable[[str], str],
    ) -> None:
        self.db = db
        self.redis = redis_client
        self._sanitize = sanitize

    async def prepare_candidates(
        self,
        *,
        api_format: str,
        model_name: str,
        affinity_key: str,
        user_api_key: ApiKey,
        request_id: str | None,
        capability_requirements: dict[str, bool] | None,
        request_body: dict[str, Any] | None,
        max_candidates: int | None,
        apply_pool_reorder: ApplyPoolReorderFn,
        expand_pool_candidates_for_async_submit: ExpandPoolCandidatesFn,
    ) -> PreparedSubmitCandidates:
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
        cache_scheduler = await get_cache_aware_scheduler(
            self.redis,
            priority_mode=priority_mode,
            scheduling_mode=scheduling_mode,
        )
        resolver = CandidateResolver(db=self.db, cache_scheduler=cache_scheduler)

        candidates, _global_model_id = await resolver.fetch_candidates(
            api_format=api_format,
            model_name=model_name,
            affinity_key=affinity_key,
            user_api_key=user_api_key,
            request_id=request_id,
            is_stream=False,
            capability_requirements=capability_requirements,
            request_body=request_body,
        )
        _ = _global_model_id

        if not candidates:
            raise AllCandidatesFailedError(
                reason="no_candidates",
                candidate_keys=[],
                last_status_code=None,
            )

        # Account Pool: keep internal key failover order/skip behavior
        # consistent with the SYNC path.
        candidates, _pool_traces = await apply_pool_reorder(
            candidates,
            request_body=request_body,
        )
        _ = _pool_traces
        candidates = expand_pool_candidates_for_async_submit(candidates)

        if max_candidates is not None and max_candidates > 0:
            candidates = candidates[:max_candidates]

        # Pre-create RequestCandidate records (no retry expand for async submit stage)
        record_map: dict[tuple[int, int], str] = {}
        if request_id:
            try:
                record_map = resolver.create_candidate_records(
                    all_candidates=candidates,
                    request_id=request_id,
                    user_id=str(user_api_key.user_id),
                    user_api_key=user_api_key,
                    required_capabilities=capability_requirements,
                    expand_retries=False,
                )
            except Exception as exc:
                logger.warning(
                    "[TaskService] Failed to create candidate records: {}",
                    self._sanitize(str(exc)),
                )
                record_map = {}

        return PreparedSubmitCandidates(candidates=candidates, record_map=record_map)
