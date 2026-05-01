"""调度/候选子组件的协议接口。

目的：
- 用 `Protocol` 固化 CacheAwareScheduler 的子组件契约
- 便于单测注入 stub/mocks，减少对具体实现类的耦合

说明：这里的协议面向“调度器内部协作”，因此保留了部分 `_` 前缀方法。
后续如果要对外暴露更稳定的 API，可再抽出无下划线的 facade。
"""

from __future__ import annotations

from typing import TYPE_CHECKING, Any, Protocol

if TYPE_CHECKING:
    from sqlalchemy.orm import Session

    from src.models.database import GlobalModel, Provider, ProviderAPIKey
    from src.services.scheduling.affinity_manager import CacheAffinity
    from src.services.scheduling.schemas import ConcurrencySnapshot, ProviderCandidate


class CandidateSorterProtocol(Protocol):
    def _sort_by_route_priority(
        self,
        candidates: list[ProviderCandidate],
        db: Session,
        affinity_key: str | None = None,
        api_format: str | None = None,
    ) -> list[ProviderCandidate]: ...

    def _apply_load_balance(
        self, candidates: list[ProviderCandidate], api_format: str | None = None
    ) -> list[ProviderCandidate]: ...

    def shuffle_keys_by_internal_priority(
        self,
        keys: list[ProviderAPIKey],
        affinity_key: str | None = None,
        use_random: bool = False,
    ) -> list[ProviderAPIKey]: ...


class CandidateBuilderProtocol(Protocol):
    def _query_provider_refs(
        self,
        db: Session,
        provider_offset: int = 0,
        provider_limit: int | None = None,
    ) -> list[tuple[str, str]]: ...

    def _query_providers(
        self,
        db: Session,
        provider_offset: int = 0,
        provider_limit: int | None = None,
        allowed_providers: list[str] | None = None,
        provider_ids: list[str] | None = None,
    ) -> list[Provider]: ...

    async def _build_candidates(
        self,
        db: Session,
        providers: list[Provider],
        client_format: str,
        model_name: str,
        affinity_key: str | None,
        model_mappings: list[str] | None = None,
        max_candidates: int | None = None,
        is_stream: bool = False,
        capability_requirements: dict[str, bool] | None = None,
        global_conversion_enabled: bool = True,
    ) -> list[ProviderCandidate]: ...

    async def _check_model_support(
        self,
        db: Session,
        provider: Provider,
        model_name: str,
        api_format: str | None = None,
        is_stream: bool = False,
        capability_requirements: dict[str, bool] | None = None,
    ) -> tuple[bool, str | None, list[str] | None, set[str] | None]: ...

    async def _check_model_support_for_global_model(
        self,
        db: Session,
        provider: Provider,
        global_model: GlobalModel,
        model_name: str,
        api_format: str | None = None,
        is_stream: bool = False,
        capability_requirements: dict[str, bool] | None = None,
    ) -> tuple[bool, str | None, list[str] | None, set[str] | None]: ...

    def _check_key_availability(
        self,
        key: ProviderAPIKey,
        api_format: str | None,
        model_name: str,
        capability_requirements: dict[str, bool] | None = None,
        model_mappings: list[str] | None = None,
        candidate_models: set[str] | None = None,
        *,
        provider_type: str | None = None,
    ) -> tuple[bool, str | None, str | None]: ...


class ConcurrencyCheckerProtocol(Protocol):
    async def check_available(
        self,
        key: ProviderAPIKey,
        is_cached_user: bool = False,
    ) -> tuple[bool, ConcurrencySnapshot]: ...

    def get_reservation_stats(self) -> dict[str, Any]: ...


class CacheAffinityManagerProtocol(Protocol):
    async def get_affinity(
        self, affinity_key: str, api_format: str, model_name: str
    ) -> CacheAffinity | None: ...

    async def set_affinity(
        self,
        affinity_key: str,
        provider_id: str,
        endpoint_id: str,
        key_id: str,
        api_format: str,
        model_name: str,
        supports_caching: bool = True,
        ttl: int | None = None,
    ) -> None: ...

    async def invalidate_affinity(
        self,
        affinity_key: str,
        api_format: str,
        model_name: str,
        key_id: str | None = None,
        provider_id: str | None = None,
        endpoint_id: str | None = None,
    ) -> None: ...

    def get_stats(self) -> dict[str, Any]: ...
