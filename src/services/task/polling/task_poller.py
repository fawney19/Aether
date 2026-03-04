"""
Task poller (Phase2)

Provides a generic polling skeleton for async tasks.
Currently wired with a video poller adapter.

优化：HTTP 请求期间不持有数据库连接，避免阻塞其他请求。
采用三阶段处理：准备数据 -> HTTP 请求 -> 更新数据库。
"""

from __future__ import annotations

import asyncio
from datetime import datetime, timezone
from typing import Any, Protocol, runtime_checkable
from uuid import uuid4

from sqlalchemy.orm import Session

from src.core.api_format.conversion.internal_video import InternalVideoPollResult
from src.core.logger import logger
from src.database import create_session
from src.services.system.scheduler import get_scheduler
from src.services.task.video.poller_adapter import VideoPollContext, VideoTaskPollerAdapter


@runtime_checkable
class TaskPollerAdapter(Protocol):
    task_type: str

    # scheduler
    job_id: str
    job_name: str
    interval_seconds: int

    # distributed lock (optional, best-effort)
    lock_key: str
    lock_ttl: int

    # execution
    batch_size: int
    concurrency: int
    consecutive_failure_alert_threshold: int

    def list_due_task_ids(self, db: Session, *, now: datetime, limit: int) -> list[str]: ...

    def get_task(self, db: Session, task_id: str) -> Any | None: ...

    # 分阶段处理方法（推荐使用）
    async def prepare_poll_context(
        self, db: Session, task: Any
    ) -> Any: ...  # Returns context or error result

    async def poll_task_http(self, ctx: Any) -> Any: ...  # Returns poll result

    async def update_task_after_poll(
        self,
        task_id: str,
        result: Any,
        ctx: Any,
        redis_client: Any | None,
        error_exception: Exception | None = None,
    ) -> None: ...

    # 旧版方法（保留兼容性）
    async def poll_single_task(
        self, db: Session, task: Any, *, redis_client: Any | None
    ) -> None: ...

    def sanitize_error_message(self, message: str) -> str: ...


class TaskPollerService:
    """Generic background poller for async tasks."""

    def __init__(self, adapter: TaskPollerAdapter) -> None:
        self.adapter = adapter
        self._lock = asyncio.Lock()
        self.redis: Any | None = None
        self._semaphore: asyncio.Semaphore | None = None
        self._consecutive_failures = 0

    async def start(self) -> None:
        if self._semaphore is None:
            self._semaphore = asyncio.Semaphore(self.adapter.concurrency)

        # lazy import to avoid redis hard dependency in local runs
        from src.clients.redis_client import get_redis_client

        if self.redis is None:
            self.redis = await get_redis_client(require_redis=False)

        scheduler = get_scheduler()
        scheduler.add_interval_job(
            self.poll_pending_tasks,
            seconds=self.adapter.interval_seconds,
            job_id=self.adapter.job_id,
            name=self.adapter.job_name,
        )

    async def stop(self) -> None:
        scheduler = get_scheduler()
        scheduler.remove_job(self.adapter.job_id)

    async def poll_pending_tasks(self) -> None:
        try:
            await self._do_poll()
        except asyncio.CancelledError:
            logger.debug("[{}] poll_pending_tasks cancelled (shutdown?)", self.adapter.task_type)
            return

    async def _do_poll(self) -> None:
        async with self._lock:
            token = await self._acquire_redis_lock()
            if token is None:
                return

            try:
                with create_session() as db:
                    now = datetime.now(timezone.utc)
                    task_ids = self.adapter.list_due_task_ids(
                        db, now=now, limit=self.adapter.batch_size
                    )

                if not task_ids:
                    self._consecutive_failures = 0
                    return

                poll_results: list[bool] = []

                if self._semaphore is None:
                    self._semaphore = asyncio.Semaphore(self.adapter.concurrency)
                semaphore = self._semaphore

                async def poll_with_semaphore(task_id: str) -> None:
                    async with semaphore:
                        try:
                            # ========== 阶段 1：准备数据（短暂持有连接）==========
                            with create_session() as task_db:
                                task_obj = self.adapter.get_task(task_db, task_id)
                                if not task_obj:
                                    logger.warning(
                                        "[{}] Task {} disappeared during poll",
                                        self.adapter.task_type,
                                        task_id,
                                    )
                                    poll_results.append(True)
                                    return

                                ctx_or_result = await self.adapter.prepare_poll_context(
                                    task_db, task_obj
                                )

                            # 检查是否是错误结果（而非上下文）
                            if isinstance(ctx_or_result, InternalVideoPollResult):
                                # 准备阶段就失败了，直接更新任务状态
                                await self.adapter.update_task_after_poll(
                                    task_id=task_id,
                                    result=ctx_or_result,
                                    ctx=None,  # type: ignore[arg-type]
                                    redis_client=self.redis,
                                )
                                poll_results.append(True)
                                return

                            ctx: VideoPollContext = ctx_or_result

                            # ========== 阶段 2：HTTP 请求（不持有数据库连接）==========
                            error_exception: Exception | None = None
                            try:
                                result = await self.adapter.poll_task_http(ctx)
                            except Exception as http_exc:
                                # HTTP 请求失败，记录异常以便后续处理
                                error_exception = http_exc
                                result = InternalVideoPollResult(
                                    status=None,  # type: ignore[arg-type]
                                    error_message=str(http_exc),
                                )

                            # ========== 阶段 3：更新数据库（获取新连接）==========
                            await self.adapter.update_task_after_poll(
                                task_id=task_id,
                                result=result,
                                ctx=ctx,
                                redis_client=self.redis,
                                error_exception=error_exception,
                            )
                            poll_results.append(True)
                        except Exception as exc:
                            logger.exception(
                                "[{}] Unexpected error polling task {}: {}",
                                self.adapter.task_type,
                                task_id,
                                self.adapter.sanitize_error_message(str(exc)),
                            )
                            poll_results.append(False)

                async with asyncio.TaskGroup() as tg:
                    for tid in task_ids:
                        tg.create_task(poll_with_semaphore(tid))

                batch_failures = sum(1 for r in poll_results if r is False)
                if batch_failures == len(task_ids):
                    self._consecutive_failures += 1
                    if (
                        self._consecutive_failures
                        >= self.adapter.consecutive_failure_alert_threshold
                    ):
                        logger.error(
                            "[ALERT] {} poller: {} consecutive batches failed.",
                            self.adapter.task_type,
                            self._consecutive_failures,
                        )
                else:
                    self._consecutive_failures = 0
            finally:
                await self._release_redis_lock(token)

    async def _acquire_redis_lock(self) -> str | None:
        if not self.redis:
            return "no_redis"
        token = str(uuid4())
        acquired = await self.redis.set(
            self.adapter.lock_key, token, nx=True, ex=self.adapter.lock_ttl
        )
        return token if acquired else None

    async def _release_redis_lock(self, token: str) -> None:
        if not self.redis or token == "no_redis":
            return
        script = """
        if redis.call('GET', KEYS[1]) == ARGV[1] then
            return redis.call('DEL', KEYS[1])
        end
        return 0
        """
        await self.redis.eval(script, 1, self.adapter.lock_key, token)


_task_poller: TaskPollerService | None = None


def get_task_poller() -> TaskPollerService:
    global _task_poller
    if _task_poller is None:
        _task_poller = TaskPollerService(VideoTaskPollerAdapter())
    return _task_poller
