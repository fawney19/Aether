"""管理员"清空请求体"异步任务。

旧实现在 `src/api/admin/system.py` 里用单个裸 UPDATE 清空整张 usage 表的 12 个大列，
在几十万行以上的数据量下必然 HTTP 超时并长时间锁表。

本模块把它拆成：
- Redis 全局锁 + 任务状态
- 后台协程分批游标清理（每批独立事务）
- 前端通过 task_id 轮询进度
"""

from __future__ import annotations

import asyncio
import json
import time
import uuid
from datetime import datetime, timedelta, timezone
from typing import Any, Callable

from sqlalchemy import null, update

from src.clients import get_redis_client
from src.core.logger import logger
from src.database import create_session
from src.models.database import Usage

_LOCK_KEY = "purge:request_bodies:lock"
_TASK_KEY_PREFIX = "purge:request_bodies:task:"
_LOCK_TTL_SECONDS = 3600
_TASK_TTL_SECONDS = 86400
# 心跳独立协程每 10s 写一次 Redis；超时阈值给 10 分钟缓冲，
# 避免事件循环偶发拥塞被误判为僵尸。
_HEARTBEAT_INTERVAL_SECONDS = 10
_HEARTBEAT_TIMEOUT_SECONDS = 600

STATE_PENDING = "pending"
STATE_RUNNING = "running"
STATE_COMPLETED = "completed"
STATE_FAILED = "failed"

DEFAULT_BATCH_SIZE = 2000


def _task_key(task_id: str) -> str:
    return f"{_TASK_KEY_PREFIX}{task_id}"


def _now_ts() -> float:
    return time.time()


def _now_iso() -> str:
    return datetime.now(timezone.utc).isoformat()


async def _save_state(task_id: str, state: dict[str, Any]) -> None:
    r = await get_redis_client(require_redis=False)
    if r is None:
        return
    state["heartbeat_at"] = _now_ts()
    await r.set(_task_key(task_id), json.dumps(state), ex=_TASK_TTL_SECONDS)


async def _load_state(task_id: str) -> dict[str, Any] | None:
    r = await get_redis_client(require_redis=False)
    if r is None:
        return None
    raw = await r.get(_task_key(task_id))
    if not raw:
        return None
    if isinstance(raw, bytes):
        raw = raw.decode("utf-8", errors="replace")
    try:
        return json.loads(raw)
    except json.JSONDecodeError:
        return None


async def _acquire_lock(task_id: str) -> bool:
    r = await get_redis_client(require_redis=False)
    if r is None:
        # Redis 不可用时不强求锁；用户能用按钮比锁保护更重要
        return True
    ok = await r.set(_LOCK_KEY, task_id, ex=_LOCK_TTL_SECONDS, nx=True)
    return bool(ok)


async def _release_lock(task_id: str) -> None:
    r = await get_redis_client(require_redis=False)
    if r is None:
        return
    holder = await r.get(_LOCK_KEY)
    if isinstance(holder, bytes):
        holder = holder.decode("utf-8", errors="replace")
    if holder == task_id:
        await r.delete(_LOCK_KEY)


async def _get_lock_holder() -> str | None:
    r = await get_redis_client(require_redis=False)
    if r is None:
        return None
    holder = await r.get(_LOCK_KEY)
    if isinstance(holder, bytes):
        holder = holder.decode("utf-8", errors="replace")
    return holder or None


def _nonempty_filter():
    """"body/header 任一列非空"——用于过滤掉已清理/本来就空的行。

    PG 没有单列非空谓词索引时这是非索引 filter，但放在主键游标扫描之上，
    每批最多扫 batch_size 行主键就停，代价可控。若数据规模继续增长，
    可以考虑加 partial index：
      CREATE INDEX CONCURRENTLY idx_usage_has_body ON usage(id)
      WHERE request_body IS NOT NULL OR response_body IS NOT NULL OR ...;
    """
    return (
        (Usage.request_body.isnot(None))
        | (Usage.response_body.isnot(None))
        | (Usage.provider_request_body.isnot(None))
        | (Usage.client_response_body.isnot(None))
        | (Usage.request_body_compressed.isnot(None))
        | (Usage.response_body_compressed.isnot(None))
        | (Usage.provider_request_body_compressed.isnot(None))
        | (Usage.client_response_body_compressed.isnot(None))
        | (Usage.request_headers.isnot(None))
        | (Usage.response_headers.isnot(None))
        | (Usage.provider_request_headers.isnot(None))
        | (Usage.client_response_headers.isnot(None))
    )


def _clear_values() -> dict[Any, Any]:
    return {
        Usage.request_body: null(),
        Usage.response_body: null(),
        Usage.provider_request_body: null(),
        Usage.client_response_body: null(),
        Usage.request_body_compressed: null(),
        Usage.response_body_compressed: null(),
        Usage.provider_request_body_compressed: null(),
        Usage.client_response_body_compressed: null(),
        Usage.request_headers: null(),
        Usage.response_headers: null(),
        Usage.provider_request_headers: null(),
        Usage.client_response_headers: null(),
    }


def _run_purge_sync(
    cutoff: datetime | None,
    batch_size: int,
    progress_cb: Callable[[int], None],
) -> int:
    """在线程池中执行的分批清理主循环。每批独立事务，避免长事务锁。

    策略（针对 PostgreSQL 优化）：
    - 按主键 id 游标分页：`WHERE id > last_id ORDER BY id LIMIT N` 走 PK 索引，
      避免非索引 ORDER BY / 全表排序
    - 再加 body 非空 filter：让已为 NULL 的行不进入 UPDATE，避免 MVCC 写放大
      （PG 即便 NULL→NULL 也会写新 heap tuple + 产生 dead tuple，增加 VACUUM 压力）
    - rowcount 在 PG 上等于 WHERE 匹配行数，可精确反映"本批清了多少条"
    - 游标推进用本批最大 id，而不是重新从 0 开始，避免反复扫描前缀
    """
    total_cleaned = 0
    last_id: str | None = None
    values = _clear_values()
    nonempty = _nonempty_filter()
    batch_no = 0

    while True:
        batch_db = create_session()
        try:
            select_start = time.monotonic()
            query = batch_db.query(Usage.id).filter(nonempty)
            if cutoff is not None:
                query = query.filter(Usage.created_at < cutoff)
            if last_id is not None:
                query = query.filter(Usage.id > last_id)
            rows = query.order_by(Usage.id.asc()).limit(batch_size).all()
            ids = [r.id for r in rows]
            select_ms = int((time.monotonic() - select_start) * 1000)
            if not ids:
                logger.info(
                    "清空请求体扫描完毕：共 {} 批，累计 {} 条（末批 SELECT 耗时 {}ms）",
                    batch_no,
                    total_cleaned,
                    select_ms,
                )
                break

            # 推进游标
            last_id = ids[-1]

            update_start = time.monotonic()
            result = batch_db.execute(
                update(Usage).where(Usage.id.in_(ids)).values(values)
            )
            batch_db.commit()
            update_ms = int((time.monotonic() - update_start) * 1000)
            updated = result.rowcount or 0

            total_cleaned += updated
            batch_no += 1
            logger.info(
                "清空请求体 batch#{} select={}ms update+commit={}ms rowcount={} total={}",
                batch_no,
                select_ms,
                update_ms,
                updated,
                total_cleaned,
            )
            try:
                progress_cb(total_cleaned)
            except Exception as cb_exc:
                logger.warning("清空请求体进度回调失败: {}", cb_exc)
        except Exception:
            try:
                batch_db.rollback()
            except Exception:
                pass
            raise
        finally:
            batch_db.close()

    return total_cleaned


async def _heartbeat_loop(task_id: str, state: dict[str, Any]) -> None:
    """独立心跳：与业务循环解耦，周期性刷新 Redis 的 heartbeat_at。

    这样即便某一批 SELECT/UPDATE 在大表上跑好几分钟，状态检测也不会
    把活着的任务误判为僵尸。
    """
    try:
        while True:
            await asyncio.sleep(_HEARTBEAT_INTERVAL_SECONDS)
            try:
                await _save_state(task_id, dict(state))
            except Exception as exc:
                logger.debug("清空请求体心跳写 Redis 失败: {}", exc)
    except asyncio.CancelledError:
        return


async def _run_task(task_id: str, cutoff: datetime | None, cutoff_days: int | None) -> None:
    state: dict[str, Any] = {
        "task_id": task_id,
        "status": STATE_RUNNING,
        "total_cleaned": 0,
        "started_at": _now_iso(),
        "finished_at": None,
        "error": None,
        "params": {
            "cutoff_days": cutoff_days,
            "cutoff": cutoff.isoformat() if cutoff else None,
        },
    }
    await _save_state(task_id, state)

    loop = asyncio.get_running_loop()

    # 进度回调在工作线程里被调用。不等待写 Redis 完成，避免长时间阻塞工作线程。
    # 真正的"还活着"证明靠独立心跳协程，这里只需尽力更新 total_cleaned。
    def progress_cb(total: int) -> None:
        state["total_cleaned"] = total
        try:
            asyncio.run_coroutine_threadsafe(
                _save_state(task_id, dict(state)), loop
            )
        except Exception as exc:
            logger.debug("清空请求体进度分派失败: {}", exc)

    hb_task = asyncio.create_task(_heartbeat_loop(task_id, state))
    try:
        total = await loop.run_in_executor(
            None,
            _run_purge_sync,
            cutoff,
            DEFAULT_BATCH_SIZE,
            progress_cb,
        )
        state["status"] = STATE_COMPLETED
        state["total_cleaned"] = total
        state["finished_at"] = _now_iso()
        logger.info("清空请求体任务 {} 完成，共清理 {} 条", task_id, total)
    except Exception as exc:
        state["status"] = STATE_FAILED
        state["error"] = str(exc)
        state["finished_at"] = _now_iso()
        logger.exception("清空请求体任务 {} 失败", task_id)
    finally:
        hb_task.cancel()
        try:
            await hb_task
        except (asyncio.CancelledError, Exception):
            pass
        await _save_state(task_id, state)
        await _release_lock(task_id)


async def _mark_zombie_if_needed(task_id: str, state: dict[str, Any]) -> dict[str, Any]:
    """running 状态但心跳超时，则判定为 failed 并释放锁。"""
    if state.get("status") != STATE_RUNNING:
        return state
    hb = state.get("heartbeat_at")
    if hb is None:
        return state
    try:
        hb_value = float(hb)
    except (TypeError, ValueError):
        return state
    if _now_ts() - hb_value <= _HEARTBEAT_TIMEOUT_SECONDS:
        return state

    logger.warning(
        "清空请求体任务 {} 心跳超时 {}s，判定为失败",
        task_id,
        int(_now_ts() - hb_value),
    )
    state["status"] = STATE_FAILED
    state["error"] = "任务心跳超时（进程可能已重启）"
    state["finished_at"] = _now_iso()
    await _save_state(task_id, state)
    await _release_lock(task_id)
    return state


async def start_purge_request_bodies(cutoff_days: int | None) -> dict[str, Any]:
    """启动清理任务。已有在跑任务时返回同一 task_id（reused=True）。"""
    existing_id = await _get_lock_holder()
    if existing_id:
        existing_state = await _load_state(existing_id)
        if existing_state:
            existing_state = await _mark_zombie_if_needed(existing_id, existing_state)
            if existing_state.get("status") == STATE_RUNNING:
                return {
                    "task_id": existing_id,
                    "status": STATE_RUNNING,
                    "reused": True,
                    "total_cleaned": existing_state.get("total_cleaned", 0),
                }
        else:
            await _release_lock(existing_id)

    task_id = uuid.uuid4().hex[:16]
    if not await _acquire_lock(task_id):
        holder = await _get_lock_holder()
        return {
            "task_id": holder or task_id,
            "status": STATE_RUNNING,
            "reused": True,
            "total_cleaned": 0,
        }

    cutoff: datetime | None = None
    if cutoff_days is not None and cutoff_days > 0:
        cutoff = datetime.now(timezone.utc) - timedelta(days=cutoff_days)

    initial_state = {
        "task_id": task_id,
        "status": STATE_PENDING,
        "total_cleaned": 0,
        "started_at": _now_iso(),
        "finished_at": None,
        "error": None,
        "params": {
            "cutoff_days": cutoff_days,
            "cutoff": cutoff.isoformat() if cutoff else None,
        },
    }
    await _save_state(task_id, initial_state)

    asyncio.create_task(_run_task(task_id, cutoff, cutoff_days))

    return {
        "task_id": task_id,
        "status": STATE_PENDING,
        "reused": False,
        "total_cleaned": 0,
    }


async def get_purge_task_status(task_id: str) -> dict[str, Any] | None:
    state = await _load_state(task_id)
    if not state:
        return None
    return await _mark_zombie_if_needed(task_id, state)
