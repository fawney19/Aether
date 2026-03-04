from __future__ import annotations

from collections.abc import Awaitable, Callable
from typing import Any

ApplyPoolReorderFn = Callable[
    [list[Any], dict[str, Any] | None],
    Awaitable[tuple[list[Any], list[Any]]],
]
ExpandPoolCandidatesFn = Callable[[list[Any]], list[Any]]

__all__ = ["ApplyPoolReorderFn", "ExpandPoolCandidatesFn"]
