from __future__ import annotations

import asyncio
import time
from collections.abc import Awaitable, Callable
from typing import Any
from typing import TypeVar

from sqlalchemy import text
from sqlalchemy.exc import DBAPIError

from src.core.logger import logger

T = TypeVar("T")

_RETRYABLE_SQLSTATE_CODES = frozenset({"40P01", "40001", "55P03", "57014"})
_RETRYABLE_MESSAGES = (
    "deadlock detected",
    "serialization failure",
    "could not serialize access",
    "querycanceled",
    "statement timeout",
    "canceling statement due to statement timeout",
    "lock timeout",
    "canceling statement due to lock timeout",
)


def apply_postgres_statement_timeouts(
    db: Any,
    *,
    statement_timeout_ms: int = 15000,
    lock_timeout_ms: int = 3000,
) -> None:
    bind = db.get_bind() if hasattr(db, "get_bind") else None
    dialect_name = getattr(getattr(bind, "dialect", None), "name", "")
    if dialect_name != "postgresql":
        return

    db.execute(text(f"SET LOCAL statement_timeout = '{int(statement_timeout_ms)}'"))
    db.execute(text(f"SET LOCAL lock_timeout = '{int(lock_timeout_ms)}'"))


def is_retryable_db_error(exc: BaseException) -> bool:
    queue: list[BaseException] = [exc]
    seen: set[int] = set()

    while queue:
        current = queue.pop(0)
        current_id = id(current)
        if current_id in seen:
            continue
        seen.add(current_id)

        if isinstance(current, DBAPIError):
            orig = getattr(current, "orig", None)
            sqlstate = getattr(orig, "pgcode", None) or getattr(orig, "sqlstate", None)
            if sqlstate in _RETRYABLE_SQLSTATE_CODES:
                return True

        message = str(current).lower()
        if any(token in message for token in _RETRYABLE_MESSAGES):
            return True

        for attr in ("orig", "__cause__", "__context__"):
            nested = getattr(current, attr, None)
            if isinstance(nested, BaseException):
                queue.append(nested)

    return False


async def run_async_db_retry(
    operation: Callable[[], Awaitable[T]],
    *,
    context: str,
    attempts: int = 3,
    base_delay_seconds: float = 0.05,
) -> T:
    for attempt in range(1, attempts + 1):
        try:
            return await operation()
        except Exception as exc:
            if attempt >= attempts or not is_retryable_db_error(exc):
                raise
            delay = base_delay_seconds * attempt
            logger.warning(
                "[db-retry] {} retryable failure on attempt {}/{}: {} (sleep {:.2f}s)",
                context,
                attempt,
                attempts,
                exc,
                delay,
            )
            await asyncio.sleep(delay)

    raise RuntimeError(f"unreachable async db retry state: {context}")


def run_sync_db_retry(
    operation: Callable[[], T],
    *,
    context: str,
    attempts: int = 3,
    base_delay_seconds: float = 0.05,
) -> T:
    for attempt in range(1, attempts + 1):
        try:
            return operation()
        except Exception as exc:
            if attempt >= attempts or not is_retryable_db_error(exc):
                raise
            delay = base_delay_seconds * attempt
            logger.warning(
                "[db-retry] {} retryable failure on attempt {}/{}: {} (sleep {:.2f}s)",
                context,
                attempt,
                attempts,
                exc,
                delay,
            )
            time.sleep(delay)

    raise RuntimeError(f"unreachable sync db retry state: {context}")
