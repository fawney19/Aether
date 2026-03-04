from __future__ import annotations

from collections.abc import Callable
from typing import Any


class AsyncSubmitRuleDeciderService:
    """异步提交故障转移规则判定服务。"""

    def __init__(
        self,
        *,
        match_provider_failover_rule: Callable[..., str | None],
    ) -> None:
        self._match_provider_failover_rule = match_provider_failover_rule

    def detect_error_stop_pattern(
        self,
        *,
        candidate: Any,
        response_text: str,
        status_code: int,
    ) -> str | None:
        return self._match_provider_failover_rule(
            candidate,
            is_success=False,
            response_text=response_text,
            status_code=status_code,
        )

    def detect_success_failover_pattern(
        self,
        *,
        candidate: Any,
        response_text: str,
        status_code: int,
    ) -> str | None:
        return self._match_provider_failover_rule(
            candidate,
            is_success=True,
            response_text=response_text,
            status_code=status_code,
        )
