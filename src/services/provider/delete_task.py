"""Provider 异步删除任务。

接口提交后立即返回 task_id，后台分阶段删除 provider 及其子资源。
任务状态存储在 Redis 中，支持多 worker 进程共享。
"""

from __future__ import annotations

import asyncio
import json
import time
import uuid
from collections.abc import Callable, Sequence
from concurrent.futures import Future
from typing import Any

import redis.asyncio as aioredis
from sqlalchemy import text

from src.clients.redis_client import get_redis_client
from src.core.logger import logger
from src.database import create_session
from src.models.database import (
    ApiKey,
    Model,
    Provider,
    ProviderAPIKey,
    ProviderEndpoint,
    RequestCandidate,
    Usage,
    UserGroup,
    UserPreference,
    VideoTask,
)
from src.models.database_extensions import ApiKeyProviderMapping, ProviderUsageTracking
from src.services.cache.model_cache import ModelCacheService
from src.services.cache.model_list_cache import invalidate_models_list_cache
from src.services.cache.provider_cache import ProviderCacheService
from src.services.provider.delete_cleanup import prune_allowed_provider_refs
from src.services.provider_keys.key_side_effects import cleanup_key_references

STATUS_PENDING = "pending"
STATUS_RUNNING = "running"
STATUS_COMPLETED = "completed"
STATUS_FAILED = "failed"

_TASK_RETAIN_SECONDS = 600
_KEY_BATCH_SIZE = 50
_ENDPOINT_BATCH_SIZE = 200
_BATCH_STATEMENT_TIMEOUT_S = 30
_BATCH_LOCK_TIMEOUT_S = 5
_TASK_TIMEOUT_S = 1800
_REDIS_KEY_PREFIX = "provider_delete_task"

_running_tasks: set[asyncio.Task[None]] = set()


def _task_key(task_id: str) -> str:
    return f"{_REDIS_KEY_PREFIX}:{task_id}"


def _provider_lock_key(provider_id: str) -> str:
    return f"{_REDIS_KEY_PREFIX}:provider:{provider_id}"


class ProviderDeleteTaskInfo:
    __slots__ = (
        "task_id",
        "provider_id",
        "status",
        "stage",
        "total_keys",
        "deleted_keys",
        "total_endpoints",
        "deleted_endpoints",
        "message",
    )

    def __init__(
        self,
        task_id: str,
        provider_id: str,
        status: str = STATUS_PENDING,
        stage: str = "queued",
        total_keys: int = 0,
        deleted_keys: int = 0,
        total_endpoints: int = 0,
        deleted_endpoints: int = 0,
        message: str = "",
    ) -> None:
        self.task_id = task_id
        self.provider_id = provider_id
        self.status = status
        self.stage = stage
        self.total_keys = total_keys
        self.deleted_keys = deleted_keys
        self.total_endpoints = total_endpoints
        self.deleted_endpoints = deleted_endpoints
        self.message = message

    def to_dict(self) -> dict[str, Any]:
        return {
            "task_id": self.task_id,
            "provider_id": self.provider_id,
            "status": self.status,
            "stage": self.stage,
            "total_keys": self.total_keys,
            "deleted_keys": self.deleted_keys,
            "total_endpoints": self.total_endpoints,
            "deleted_endpoints": self.deleted_endpoints,
            "message": self.message,
        }

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "ProviderDeleteTaskInfo":
        return cls(
            task_id=str(data["task_id"]),
            provider_id=str(data["provider_id"]),
            status=str(data.get("status", STATUS_PENDING)),
            stage=str(data.get("stage", "queued")),
            total_keys=int(data.get("total_keys", 0)),
            deleted_keys=int(data.get("deleted_keys", 0)),
            total_endpoints=int(data.get("total_endpoints", 0)),
            deleted_endpoints=int(data.get("deleted_endpoints", 0)),
            message=str(data.get("message", "")),
        )


async def _save_task(
    task: ProviderDeleteTaskInfo,
    ttl: int = _TASK_RETAIN_SECONDS,
    r: aioredis.Redis | None = None,
) -> None:
    if r is None:
        r = await get_redis_client(require_redis=False)
    if not r:
        return
    try:
        await r.setex(_task_key(task.task_id), ttl, json.dumps(task.to_dict()))
        await r.setex(_provider_lock_key(task.provider_id), ttl, task.task_id)
    except Exception as exc:
        logger.warning("Failed to save provider delete task: {}", exc)


async def _load_task(
    task_id: str,
    r: aioredis.Redis | None = None,
) -> ProviderDeleteTaskInfo | None:
    if r is None:
        r = await get_redis_client(require_redis=False)
    if not r:
        return None
    try:
        data = await r.get(_task_key(task_id))
        if data is None:
            return None
        return ProviderDeleteTaskInfo.from_dict(json.loads(data))
    except Exception as exc:
        logger.warning("Failed to load provider delete task: {}", exc)
        return None


async def _update_task_field(
    task_id: str,
    r: aioredis.Redis | None = None,
    **fields: object,
) -> None:
    if r is None:
        r = await get_redis_client(require_redis=False)
    if not r:
        return
    task = await _load_task(task_id, r=r)
    if task is None:
        return
    for key, value in fields.items():
        setattr(task, key, value)
    ttl = (
        _TASK_RETAIN_SECONDS
        if task.status in (STATUS_COMPLETED, STATUS_FAILED)
        else _TASK_RETAIN_SECONDS * 2
    )
    await _save_task(task, ttl=ttl, r=r)


async def submit_provider_delete(provider_id: str) -> str:
    r = await get_redis_client(require_redis=False)
    if not r:
        raise RuntimeError("Redis is required for provider delete tasks but is not available")

    existing_task_id = await r.get(_provider_lock_key(provider_id))
    if isinstance(existing_task_id, bytes):
        existing_task_id = existing_task_id.decode()
    if existing_task_id:
        existing_task = await _load_task(str(existing_task_id), r=r)
        if existing_task and existing_task.status in (STATUS_PENDING, STATUS_RUNNING):
            return existing_task.task_id

    task_id = uuid.uuid4().hex[:16]
    task = ProviderDeleteTaskInfo(
        task_id=task_id,
        provider_id=provider_id,
        message="delete task submitted",
    )
    await _save_task(task, ttl=_TASK_RETAIN_SECONDS * 2, r=r)

    def _on_task_done(task: asyncio.Task[None]) -> None:
        _running_tasks.discard(task)
        if not task.cancelled() and task.exception():
            logger.error("[PROVIDER_DELETE_TASK] unhandled error: {}", task.exception())

    bg = asyncio.create_task(
        _run_provider_delete(task_id, provider_id),
        name=f"provider-delete-{task_id}",
    )
    _running_tasks.add(bg)
    bg.add_done_callback(_on_task_done)
    return task_id


async def get_provider_delete_task(task_id: str) -> ProviderDeleteTaskInfo | None:
    return await _load_task(task_id)


def _iter_batches(items: Sequence[str], batch_size: int) -> list[list[str]]:
    if not items:
        return []
    if batch_size <= 0:
        return [list(items)]
    return [list(items[i : i + batch_size]) for i in range(0, len(items), batch_size)]


def _apply_statement_timeouts(db: Any) -> None:
    db.execute(text(f"SET LOCAL statement_timeout = '{_BATCH_STATEMENT_TIMEOUT_S * 1000}'"))
    db.execute(text(f"SET LOCAL lock_timeout = '{_BATCH_LOCK_TIMEOUT_S * 1000}'"))


def _collect_ids(db: Any, provider_id: str) -> tuple[list[str], list[str]]:
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


def _sync_delete_provider(
    provider_id: str,
    progress_callback: Callable[[dict[str, object]], None] | None = None,
) -> dict[str, Any]:
    db = create_session()
    try:
        task_start = time.monotonic()
        provider = db.query(Provider).filter(Provider.id == provider_id).first()
        if not provider:
            raise RuntimeError("provider not found")

        _apply_statement_timeouts(db)
        endpoint_ids, key_ids = _collect_ids(db, provider_id)
        if progress_callback is not None:
            progress_callback(
                {
                    "stage": "preparing",
                    "total_keys": len(key_ids),
                    "total_endpoints": len(endpoint_ids),
                    "message": f"preparing delete for {len(key_ids)} keys and {len(endpoint_ids)} endpoints",
                }
            )

        if getattr(provider, "is_active", False):
            provider.is_active = False
            db.commit()
            if progress_callback is not None:
                progress_callback(
                    {
                        "stage": "disabling",
                        "message": "provider disabled; starting cleanup",
                    }
                )

        _apply_statement_timeouts(db)
        updated_user_groups = prune_allowed_provider_refs(
            db.query(UserGroup).filter(UserGroup.allowed_providers.isnot(None)).all(),
            provider_id,
        )
        updated_api_keys = prune_allowed_provider_refs(
            db.query(ApiKey).filter(ApiKey.allowed_providers.isnot(None)).all(),
            provider_id,
        )
        db.commit()
        if progress_callback is not None:
            progress_callback(
                {
                    "stage": "cleaning_restrictions",
                    "message": (
                        "cleaned access restrictions "
                        f"(user_groups={updated_user_groups}, api_keys={updated_api_keys})"
                    ),
                }
            )

        _apply_statement_timeouts(db)
        db.query(UserPreference).filter(UserPreference.default_provider_id == provider_id).update(
            {UserPreference.default_provider_id: None},
            synchronize_session=False,
        )
        db.query(Usage).filter(Usage.provider_id == provider_id).update(
            {Usage.provider_id: None},
            synchronize_session=False,
        )
        db.query(VideoTask).filter(VideoTask.provider_id == provider_id).update(
            {VideoTask.provider_id: None},
            synchronize_session=False,
        )
        db.query(RequestCandidate).filter(RequestCandidate.provider_id == provider_id).delete(
            synchronize_session=False,
        )
        db.commit()
        if progress_callback is not None:
            progress_callback(
                {
                    "stage": "cleaning_provider_refs",
                    "message": "cleaned provider-wide history references",
                }
            )

        deleted_keys = 0
        key_batches = _iter_batches(key_ids, _KEY_BATCH_SIZE)
        for index, batch in enumerate(key_batches, start=1):
            if time.monotonic() - task_start > _TASK_TIMEOUT_S:
                raise RuntimeError(f"task timeout after {_TASK_TIMEOUT_S}s")
            _apply_statement_timeouts(db)
            cleanup_key_references(db, batch)
            deleted_batch = int(
                db.query(ProviderAPIKey)
                .filter(ProviderAPIKey.provider_id == provider_id, ProviderAPIKey.id.in_(batch))
                .delete(synchronize_session=False)
                or 0
            )
            db.commit()
            deleted_keys += deleted_batch
            if progress_callback is not None:
                progress_callback(
                    {
                        "stage": "deleting_keys",
                        "deleted_keys": deleted_keys,
                        "message": f"deleted key batch {index}/{max(len(key_batches), 1)}",
                    }
                )

        deleted_endpoints = 0
        endpoint_batches = _iter_batches(endpoint_ids, _ENDPOINT_BATCH_SIZE)
        for index, batch in enumerate(endpoint_batches, start=1):
            if time.monotonic() - task_start > _TASK_TIMEOUT_S:
                raise RuntimeError(f"task timeout after {_TASK_TIMEOUT_S}s")
            _apply_statement_timeouts(db)
            db.query(Usage).filter(Usage.provider_endpoint_id.in_(batch)).update(
                {Usage.provider_endpoint_id: None},
                synchronize_session=False,
            )
            db.query(VideoTask).filter(VideoTask.endpoint_id.in_(batch)).update(
                {VideoTask.endpoint_id: None},
                synchronize_session=False,
            )
            db.query(RequestCandidate).filter(RequestCandidate.endpoint_id.in_(batch)).delete(
                synchronize_session=False,
            )
            deleted_batch = int(
                db.query(ProviderEndpoint)
                .filter(ProviderEndpoint.provider_id == provider_id, ProviderEndpoint.id.in_(batch))
                .delete(synchronize_session=False)
                or 0
            )
            db.commit()
            deleted_endpoints += deleted_batch
            if progress_callback is not None:
                progress_callback(
                    {
                        "stage": "deleting_endpoints",
                        "deleted_endpoints": deleted_endpoints,
                        "message": f"deleted endpoint batch {index}/{max(len(endpoint_batches), 1)}",
                    }
                )

        _apply_statement_timeouts(db)
        deleted_models = int(
            db.query(Model)
            .filter(Model.provider_id == provider_id)
            .delete(synchronize_session=False)
            or 0
        )
        deleted_mappings = int(
            db.query(ApiKeyProviderMapping)
            .filter(ApiKeyProviderMapping.provider_id == provider_id)
            .delete(synchronize_session=False)
            or 0
        )
        deleted_usage_tracking = int(
            db.query(ProviderUsageTracking)
            .filter(ProviderUsageTracking.provider_id == provider_id)
            .delete(synchronize_session=False)
            or 0
        )
        deleted_provider = int(
            db.query(Provider).filter(Provider.id == provider_id).delete(synchronize_session=False)
            or 0
        )
        db.commit()

        return {
            "provider_id": provider_id,
            "total_keys": len(key_ids),
            "deleted_keys": deleted_keys,
            "total_endpoints": len(endpoint_ids),
            "deleted_endpoints": deleted_endpoints,
            "deleted_models": deleted_models,
            "deleted_mappings": deleted_mappings,
            "deleted_usage_tracking": deleted_usage_tracking,
            "deleted_provider": deleted_provider,
            "elapsed_seconds": time.monotonic() - task_start,
        }
    except Exception:
        try:
            db.rollback()
        except Exception:
            pass
        raise
    finally:
        try:
            db.close()
        except Exception:
            pass


async def _run_provider_delete(task_id: str, provider_id: str) -> None:
    r = await get_redis_client(require_redis=False)
    await _update_task_field(
        task_id,
        r=r,
        status=STATUS_RUNNING,
        stage="queued",
        message="delete task started",
    )

    loop = asyncio.get_running_loop()
    progress_futures: list[Future[object]] = []

    def on_progress(fields: dict[str, object]) -> None:
        try:
            future = asyncio.run_coroutine_threadsafe(
                _update_task_field(task_id, r=r, **fields),
                loop,
            )
            progress_futures.append(future)
        except RuntimeError:
            pass

    async def _drain_progress() -> None:
        if progress_futures:
            await asyncio.gather(
                *(asyncio.wrap_future(f) for f in progress_futures),
                return_exceptions=True,
            )

    try:
        summary = await asyncio.wait_for(
            asyncio.to_thread(_sync_delete_provider, provider_id, on_progress),
            timeout=_TASK_TIMEOUT_S + 60,
        )

        await _drain_progress()

        try:
            await invalidate_models_list_cache()
            await ModelCacheService.invalidate_all_resolve_cache()
            await ProviderCacheService.invalidate_provider_cache(provider_id)
        except Exception as exc:
            logger.error("provider delete cache invalidation failed: {}", exc)

        await _update_task_field(
            task_id,
            r=r,
            status=STATUS_COMPLETED,
            stage="completed",
            total_keys=int(summary.get("total_keys", 0)),
            deleted_keys=int(summary.get("deleted_keys", 0)),
            total_endpoints=int(summary.get("total_endpoints", 0)),
            deleted_endpoints=int(summary.get("deleted_endpoints", 0)),
            message=(
                f"provider deleted: keys={summary.get('deleted_keys', 0)}, "
                f"endpoints={summary.get('deleted_endpoints', 0)}"
            ),
        )
        logger.info(
            "[PROVIDER_DELETE_TASK] completed task={} provider={} keys={}/{} endpoints={}/{} elapsed={:.1f}s",
            task_id,
            provider_id[:8],
            summary.get("deleted_keys", 0),
            summary.get("total_keys", 0),
            summary.get("deleted_endpoints", 0),
            summary.get("total_endpoints", 0),
            float(summary.get("elapsed_seconds", 0.0) or 0.0),
        )
    except asyncio.TimeoutError:
        await _drain_progress()
        msg = f"task timeout after {_TASK_TIMEOUT_S + 60}s"
        await _update_task_field(task_id, r=r, status=STATUS_FAILED, stage="failed", message=msg)
        logger.error("[PROVIDER_DELETE_TASK] {} task={} provider={}", msg, task_id, provider_id[:8])
    except asyncio.CancelledError:
        await _drain_progress()
        await _update_task_field(
            task_id,
            r=r,
            status=STATUS_FAILED,
            stage="failed",
            message="task cancelled (shutdown)",
        )
        logger.warning(
            "[PROVIDER_DELETE_TASK] cancelled task={} provider={}",
            task_id,
            provider_id[:8],
        )
    except Exception as exc:
        await _drain_progress()
        await _update_task_field(
            task_id,
            r=r,
            status=STATUS_FAILED,
            stage="failed",
            message=str(exc),
        )
        logger.error(
            "[PROVIDER_DELETE_TASK] failed task={} provider={} error={}",
            task_id,
            provider_id[:8],
            exc,
        )
