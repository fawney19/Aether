from __future__ import annotations

from typing import Any

from sqlalchemy.orm import Session

from src.models.database import ApiKey
from src.services.orchestration.error_classifier import ErrorClassifier
from src.services.scheduling.aware_scheduler import ProviderCandidate, get_cache_aware_scheduler
from src.services.user.group_service import UserGroupService

from .recorder import CandidateRecorder
from .resolver import CandidateResolver
from .schema import CandidateKey


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

        scheduling_mode = UserGroupService.resolve_effective_scheduling_mode(self.db, None)
        self._cache_scheduler = await get_cache_aware_scheduler(
            self.redis,
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
        user = getattr(user_api_key, "user", None) if user_api_key is not None else None
        self._cache_scheduler = await get_cache_aware_scheduler(
            self.redis,
            scheduling_mode=UserGroupService.resolve_effective_scheduling_mode(self.db, user),
        )
        self._resolver = CandidateResolver(db=self.db, cache_scheduler=self._cache_scheduler)
        self._error_classifier = ErrorClassifier(db=self.db, cache_scheduler=self._cache_scheduler)
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
