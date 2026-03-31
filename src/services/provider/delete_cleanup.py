from __future__ import annotations

from collections.abc import Iterable, Sequence
from typing import Any

from sqlalchemy.orm import Session

from src.core.logger import logger
from src.models.database import (
    ApiKey,
    Model,
    Provider,
    ProviderAPIKey,
    ProviderEndpoint,
    RequestCandidate,
    Usage,
    UserPreference,
    VideoTask,
)
from src.models.database_extensions import ApiKeyProviderMapping, ProviderUsageTracking
from src.services.provider_keys.key_side_effects import cleanup_key_references

_BATCH_SIZE = 2000


def _empty_cleanup_stats() -> dict[str, int]:
    return {
        "api_keys": 0,
        "user_preferences": 0,
        "usage_provider": 0,
        "usage_endpoint": 0,
        "video_tasks_provider": 0,
        "video_tasks_endpoint": 0,
        "request_candidates_provider": 0,
        "request_candidates_endpoint": 0,
    }


def _empty_delete_stats() -> dict[str, int]:
    return {
        "api_key_mappings": 0,
        "usage_tracking": 0,
        "models": 0,
        "api_keys": 0,
        "endpoints": 0,
        "providers": 0,
    }


def _iter_batches(items: Sequence[str], batch_size: int = _BATCH_SIZE) -> list[list[str]]:
    if not items:
        return []
    if batch_size <= 0:
        return [list(items)]
    return [list(items[i : i + batch_size]) for i in range(0, len(items), batch_size)]


def _collect_provider_child_ids(db: Session, provider_id: str) -> tuple[list[str], list[str]]:
    endpoint_ids = [
        endpoint_id
        for endpoint_id, in db.query(ProviderEndpoint.id)
        .filter(ProviderEndpoint.provider_id == provider_id)
        .all()
    ]
    key_ids = [
        key_id
        for key_id, in db.query(ProviderAPIKey.id)
        .filter(ProviderAPIKey.provider_id == provider_id)
        .all()
    ]
    return endpoint_ids, key_ids


def prune_allowed_provider_list(
    allowed_providers: Any, provider_id: str
) -> tuple[list[str] | None | Any, bool]:
    """从访问限制列表中移除指定 Provider ID。"""
    if not isinstance(allowed_providers, list):
        return allowed_providers, False
    if provider_id not in allowed_providers:
        return allowed_providers, False

    next_allowed = [value for value in allowed_providers if value != provider_id]
    return next_allowed, True


def prune_allowed_provider_refs(records: Iterable[Any], provider_id: str) -> int:
    """批量移除记录中的 allowed_providers 引用。"""
    updated = 0
    for record in records:
        next_allowed, changed = prune_allowed_provider_list(
            getattr(record, "allowed_providers", None),
            provider_id,
        )
        if not changed:
            continue
        record.allowed_providers = next_allowed
        updated += 1
    return updated


def cleanup_deleted_provider_references(
    db: Session,
    provider_id: str,
    *,
    endpoint_ids: Sequence[str] | None = None,
    key_ids: Sequence[str] | None = None,
) -> dict[str, int]:
    """清理 Provider 删除时的大扇出引用，避免依赖数据库级联导致慢删。"""
    if not provider_id:
        return _empty_cleanup_stats()

    if endpoint_ids is None or key_ids is None:
        resolved_endpoint_ids, resolved_key_ids = _collect_provider_child_ids(db, provider_id)
        endpoint_ids = resolved_endpoint_ids if endpoint_ids is None else list(endpoint_ids)
        key_ids = resolved_key_ids if key_ids is None else list(key_ids)
    else:
        endpoint_ids = list(endpoint_ids)
        key_ids = list(key_ids)

    updated_api_keys = prune_allowed_provider_refs(
        db.query(ApiKey).filter(ApiKey.allowed_providers.isnot(None)).all(),
        provider_id,
    )

    cleared_preferences = int(
        db.query(UserPreference)
        .filter(UserPreference.default_provider_id == provider_id)
        .update({UserPreference.default_provider_id: None}, synchronize_session=False)
        or 0
    )
    cleared_usage_providers = int(
        db.query(Usage)
        .filter(Usage.provider_id == provider_id)
        .update({Usage.provider_id: None}, synchronize_session=False)
        or 0
    )
    cleared_video_task_providers = int(
        db.query(VideoTask)
        .filter(VideoTask.provider_id == provider_id)
        .update({VideoTask.provider_id: None}, synchronize_session=False)
        or 0
    )

    if key_ids:
        cleanup_key_references(db, list(key_ids))

    cleared_usage_endpoints = 0
    cleared_video_task_endpoints = 0
    deleted_request_candidates_endpoints = 0
    for batch in _iter_batches(endpoint_ids):
        cleared_usage_endpoints += int(
            db.query(Usage)
            .filter(Usage.provider_endpoint_id.in_(batch))
            .update({Usage.provider_endpoint_id: None}, synchronize_session=False)
            or 0
        )
        cleared_video_task_endpoints += int(
            db.query(VideoTask)
            .filter(VideoTask.endpoint_id.in_(batch))
            .update({VideoTask.endpoint_id: None}, synchronize_session=False)
            or 0
        )
        deleted_request_candidates_endpoints += int(
            db.query(RequestCandidate)
            .filter(RequestCandidate.endpoint_id.in_(batch))
            .delete(synchronize_session=False)
            or 0
        )

    deleted_request_candidates_provider = int(
        db.query(RequestCandidate)
        .filter(RequestCandidate.provider_id == provider_id)
        .delete(synchronize_session=False)
        or 0
    )

    stats = {
        "api_keys": updated_api_keys,
        "user_preferences": cleared_preferences,
        "usage_provider": cleared_usage_providers,
        "usage_endpoint": cleared_usage_endpoints,
        "video_tasks_provider": cleared_video_task_providers,
        "video_tasks_endpoint": cleared_video_task_endpoints,
        "request_candidates_provider": deleted_request_candidates_provider,
        "request_candidates_endpoint": deleted_request_candidates_endpoints,
    }

    if any(stats.values()) or key_ids:
        logger.info(
            "Provider 删除引用清理: provider_id={}, key_refs={}, api_keys={}, "
            "user_preferences={}, usage_provider={}, usage_endpoint={}, "
            "video_tasks_provider={}, video_tasks_endpoint={}, "
            "request_candidates_provider={}, request_candidates_endpoint={}",
            provider_id,
            len(key_ids),
            stats["api_keys"],
            stats["user_preferences"],
            stats["usage_provider"],
            stats["usage_endpoint"],
            stats["video_tasks_provider"],
            stats["video_tasks_endpoint"],
            stats["request_candidates_provider"],
            stats["request_candidates_endpoint"],
        )

    return stats


def delete_provider_tree(db: Session, provider_id: str) -> dict[str, Any]:
    """分阶段删除 Provider 及其子资源，降低 ORM/FK 级联导致的超时风险。"""
    if not provider_id:
        return {
            "cleanup": _empty_cleanup_stats(),
            "deleted": _empty_delete_stats(),
            "key_count": 0,
            "endpoint_count": 0,
        }

    endpoint_ids, key_ids = _collect_provider_child_ids(db, provider_id)
    cleanup_stats = cleanup_deleted_provider_references(
        db,
        provider_id,
        endpoint_ids=endpoint_ids,
        key_ids=key_ids,
    )

    deleted_stats = {
        "api_key_mappings": int(
            db.query(ApiKeyProviderMapping)
            .filter(ApiKeyProviderMapping.provider_id == provider_id)
            .delete(synchronize_session=False)
            or 0
        ),
        "usage_tracking": int(
            db.query(ProviderUsageTracking)
            .filter(ProviderUsageTracking.provider_id == provider_id)
            .delete(synchronize_session=False)
            or 0
        ),
        "models": int(
            db.query(Model)
            .filter(Model.provider_id == provider_id)
            .delete(synchronize_session=False)
            or 0
        ),
        "api_keys": int(
            db.query(ProviderAPIKey)
            .filter(ProviderAPIKey.provider_id == provider_id)
            .delete(synchronize_session=False)
            or 0
        ),
        "endpoints": int(
            db.query(ProviderEndpoint)
            .filter(ProviderEndpoint.provider_id == provider_id)
            .delete(synchronize_session=False)
            or 0
        ),
        "providers": int(
            db.query(Provider).filter(Provider.id == provider_id).delete(synchronize_session=False)
            or 0
        ),
    }

    logger.info(
        "Provider 分阶段删除: provider_id={}, key_count={}, endpoint_count={}, "
        "deleted_mappings={}, deleted_usage_tracking={}, deleted_models={}, "
        "deleted_api_keys={}, deleted_endpoints={}, deleted_providers={}",
        provider_id,
        len(key_ids),
        len(endpoint_ids),
        deleted_stats["api_key_mappings"],
        deleted_stats["usage_tracking"],
        deleted_stats["models"],
        deleted_stats["api_keys"],
        deleted_stats["endpoints"],
        deleted_stats["providers"],
    )

    return {
        "cleanup": cleanup_stats,
        "deleted": deleted_stats,
        "key_count": len(key_ids),
        "endpoint_count": len(endpoint_ids),
    }
