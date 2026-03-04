from __future__ import annotations

from dataclasses import dataclass
from enum import Enum
from typing import Any, AsyncIterator, Protocol, runtime_checkable

import httpx

from src.services.scheduling.aware_scheduler import ProviderCandidate


class AttemptKind(str, Enum):
    """`attempt_func` return kind."""

    SYNC_RESPONSE = "sync_response"
    STREAM = "stream"
    ASYNC_SUBMIT = "async_submit"


@dataclass(slots=True)
class AttemptResult:
    """
    Unified attempt result returned by `AttemptFunc`.

    Notes:
    - `http_status` / `http_headers` MUST be filled for all kinds (for audit/classification).
    - The payload fields are filled depending on `kind`.
    """

    kind: AttemptKind

    # HTTP meta (always filled)
    http_status: int
    http_headers: dict[str, str]

    # SYNC_RESPONSE
    response_body: Any = None

    # STREAM
    stream_iterator: AsyncIterator[bytes] | None = None

    # ASYNC_SUBMIT
    provider_task_id: str | None = None

    # Raw response reference (optional, for audit/debugging)
    raw_response: httpx.Response | None = None


@runtime_checkable
class AttemptFunc(Protocol):
    async def __call__(self, candidate: ProviderCandidate) -> AttemptResult: ...
