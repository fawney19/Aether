from __future__ import annotations

from typing import Any

from src.core.logger import logger


class TaskPoolOperationsService:
    """任务池化相关操作（重排、展开、健康回写）。"""

    def extract_session_uuid(
        self,
        provider_type: str,
        request_body: dict[str, Any] | None,
    ) -> str | None:
        """Extract a session UUID from the request body (provider-type aware)."""
        if not isinstance(request_body, dict):
            return None
        from src.services.provider.pool.hooks import get_pool_hook

        hook = get_pool_hook(provider_type)
        if hook is not None:
            return hook.extract_session_uuid(request_body)
        return None

    async def apply_pool_reorder(
        self,
        candidates: list[Any],
        request_body: dict[str, Any] | None,
    ) -> tuple[list[Any], list[Any]]:
        """Apply pool key ordering for PoolCandidate objects."""
        if not candidates:
            return candidates, []

        pool_traces: list[Any] = []

        try:
            from src.services.provider.pool.config import parse_pool_config
            from src.services.provider.pool.manager import PoolManager
            from src.services.scheduling.schemas import PoolCandidate

            for candidate in candidates:
                if not isinstance(candidate, PoolCandidate):
                    continue

                provider = candidate.provider
                provider_id = str(getattr(provider, "id", "") or "")
                if not provider_id:
                    continue

                pool_cfg = candidate.pool_config or parse_pool_config(
                    getattr(provider, "config", None)
                )
                if pool_cfg is None:
                    continue
                candidate.pool_config = pool_cfg

                provider_type = str(getattr(provider, "provider_type", "") or "")
                session_uuid = self.extract_session_uuid(provider_type, request_body)
                manager = PoolManager(provider_id, pool_cfg)

                candidate_keys = list(candidate.pool_keys or [])
                if not candidate_keys and getattr(candidate, "key", None) is not None:
                    candidate_keys = [candidate.key]

                ordered_keys, trace = await manager.select_pool_keys(session_uuid, candidate_keys)
                candidate.pool_keys = ordered_keys

                selected_key_index = 0
                selected_key = None
                for idx, pool_key in enumerate(ordered_keys):
                    if not bool(getattr(pool_key, "_pool_skipped", False)):
                        selected_key = pool_key
                        selected_key_index = idx
                        break

                if selected_key is not None:
                    candidate.key = selected_key
                    candidate._pool_key_index = selected_key_index
                    candidate.mapping_matched_model = getattr(
                        selected_key, "_pool_mapping_matched_model", None
                    )
                    candidate.is_skipped = False
                    candidate.skip_reason = None
                else:
                    candidate.is_skipped = True
                    candidate.skip_reason = "pool: all keys unavailable"

                if trace is not None:
                    pool_traces.append(trace)

            return candidates, pool_traces
        except Exception:
            logger.opt(exception=True).debug("Pool reorder failed, using original order")
            return candidates, []

    @staticmethod
    def expand_pool_candidates_for_async_submit(candidates: list[Any]) -> list[Any]:
        """Expand PoolCandidate to key-level candidates for async submit traversal."""
        from src.services.scheduling.schemas import PoolCandidate, ProviderCandidate

        expanded: list[Any] = []
        for candidate in candidates:
            if not isinstance(candidate, PoolCandidate):
                expanded.append(candidate)
                continue

            pool_keys = list(candidate.pool_keys or [])
            if not pool_keys:
                expanded.append(candidate)
                continue

            for key_index, pool_key in enumerate(pool_keys):
                key_skipped = bool(getattr(pool_key, "_pool_skipped", False))
                key_skip_reason = (
                    str(getattr(pool_key, "_pool_skip_reason", "") or "") or candidate.skip_reason
                )
                key_extra = (
                    getattr(pool_key, "_pool_extra_data", None)
                    if isinstance(getattr(pool_key, "_pool_extra_data", None), dict)
                    else {}
                )

                key_candidate = ProviderCandidate(
                    provider=candidate.provider,
                    endpoint=candidate.endpoint,
                    key=pool_key,
                    is_cached=candidate.is_cached,
                    is_skipped=bool(candidate.is_skipped) or key_skipped,
                    skip_reason=(
                        key_skip_reason if (bool(candidate.is_skipped) or key_skipped) else None
                    ),
                    mapping_matched_model=getattr(pool_key, "_pool_mapping_matched_model", None)
                    or candidate.mapping_matched_model,
                    needs_conversion=candidate.needs_conversion,
                    provider_api_format=candidate.provider_api_format,
                    output_limit=candidate.output_limit,
                    capability_miss_count=candidate.capability_miss_count,
                )
                setattr(
                    key_candidate,
                    "_pool_extra_data",
                    {
                        "pool_group_id": str(candidate.provider.id),
                        "pool_key_index": key_index,
                        **key_extra,
                    },
                )
                expanded.append(key_candidate)

        return expanded

    async def pool_on_success(
        self,
        candidate: Any,
        request_body: dict[str, Any] | None,
    ) -> None:
        """Notify the pool manager about a successful request (sticky + LRU)."""
        try:
            from src.services.provider.pool.config import parse_pool_config
            from src.services.provider.pool.manager import PoolManager

            provider = candidate.provider
            provider_config = getattr(provider, "config", None)
            pool_cfg = parse_pool_config(provider_config)
            if pool_cfg is None:
                return

            provider_id = str(getattr(provider, "id", "") or "")
            key_id = str(getattr(candidate.key, "id", "") or "")
            if not provider_id or not key_id:
                return

            provider_type = str(getattr(provider, "provider_type", "") or "")
            session_uuid = self.extract_session_uuid(provider_type, request_body)

            mgr = PoolManager(provider_id, pool_cfg)
            await mgr.on_request_success(
                session_uuid=session_uuid,
                key_id=key_id,
            )
        except Exception:
            logger.opt(exception=True).debug("Pool on_request_success failed (non-blocking)")

    @staticmethod
    async def pool_on_error(
        provider: Any,
        key: Any,
        status_code: int,
        cause: Any,
    ) -> None:
        """Notify the pool manager about an upstream error (health policy)."""
        try:
            from src.services.provider.pool.config import parse_pool_config
            from src.services.provider.pool.health_policy import apply_health_policy

            pool_cfg = parse_pool_config(getattr(provider, "config", None))
            if pool_cfg is None:
                return

            error_text = ""
            resp_headers: dict[str, str] = {}
            if getattr(cause, "response", None) is not None:
                try:
                    error_text = (cause.response.text or "")[:4000]
                except Exception:
                    pass
                try:
                    resp_headers = dict(cause.response.headers)
                except Exception:
                    pass

            await apply_health_policy(
                provider_id=str(provider.id),
                key_id=str(key.id),
                status_code=status_code,
                error_body=error_text,
                response_headers=resp_headers,
                config=pool_cfg,
            )
        except Exception:
            pass
