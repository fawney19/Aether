from __future__ import annotations

import copy
from dataclasses import dataclass, field
from typing import Any, Protocol


class RequestBodyState(Protocol):
    @property
    def current_body(self) -> dict[str, Any]: ...

    def build_attempt_body(self) -> dict[str, Any]: ...

    def is_rectified(self) -> bool: ...

    def rectify_stage(self) -> int: ...

    def mark_rectified(self, body: dict[str, Any], *, stage: int) -> None: ...

    def consume_rectified_this_turn(self) -> bool: ...


@dataclass(slots=True)
class MutableRequestBodyState:
    """Owns the mutable working request body used across retries."""

    original_body: dict[str, Any]
    _current_body: dict[str, Any] = field(init=False, repr=False)
    _rectified: bool = field(default=False, init=False, repr=False)
    _rectified_this_turn: bool = field(default=False, init=False, repr=False)
    _rectify_stage: int = field(default=0, init=False, repr=False)

    def __post_init__(self) -> None:
        # Attempts already deep-copy per dispatch, and rectification paths clone before
        # rewriting. Keep the initial working body as a direct view to avoid an eager copy.
        self._current_body = self.original_body

    @property
    def current_body(self) -> dict[str, Any]:
        return self._current_body

    def build_attempt_body(self) -> dict[str, Any]:
        return copy.deepcopy(self._current_body)

    def is_rectified(self) -> bool:
        return self._rectified

    def rectify_stage(self) -> int:
        return self._rectify_stage

    def mark_rectified(self, body: dict[str, Any], *, stage: int) -> None:
        self._current_body = body
        self._rectified = True
        self._rectified_this_turn = True
        self._rectify_stage = stage

    def consume_rectified_this_turn(self) -> bool:
        if not self._rectified_this_turn:
            return False
        self._rectified_this_turn = False
        return True


__all__ = ["MutableRequestBodyState", "RequestBodyState"]
