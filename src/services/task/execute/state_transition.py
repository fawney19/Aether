from __future__ import annotations

from dataclasses import dataclass, field
from typing import TYPE_CHECKING, Any

from src.services.candidate.policy import FailoverAction
from src.services.task.execute.exception_classification import CandidateErrorAction

if TYPE_CHECKING:
    from src.services.task.execute.failure import TaskFailureOperationsService


@dataclass(slots=True)
class ExecutionErrorTransition:
    """执行异常后的状态流转决策。"""

    failover_action: FailoverAction
    max_retries: int | None = None

    def as_failover_tuple(self) -> tuple[FailoverAction, int | None]:
        return (self.failover_action, self.max_retries)


@dataclass(slots=True)
class SyncExecutionState:
    """同步执行阶段状态容器（候选上下文 + 异常上下文）。"""

    candidate_record_map: dict[tuple[int, int], str]
    request_body_ref: dict[str, Any] | None
    last_error: Exception | None = None
    last_candidate: Any | None = None
    _rectify_flag_key: str = field(default="_rectified_this_turn", repr=False)

    def touch_candidate(self, candidate: Any) -> None:
        self.last_candidate = candidate

    def track_execution_error(self, *, exec_err: Any, candidate: Any) -> None:
        self.last_candidate = candidate
        cause = getattr(exec_err, "cause", None)
        self.last_error = cause if isinstance(cause, Exception) else None

    def resolve_candidate_record_id(self, *, candidate_index: int, record_id: str | None) -> str:
        if record_id:
            return str(record_id)
        return str(self.candidate_record_map.get((candidate_index, 0), "") or "")

    def consume_rectify_retry_extension(
        self, *, max_retries_for_candidate: int, retry_index: int
    ) -> int | None:
        if not self.request_body_ref:
            return None
        if not self.request_body_ref.get(self._rectify_flag_key, False):
            return None

        self.request_body_ref[self._rectify_flag_key] = False
        return max(max_retries_for_candidate, retry_index + 2)

    def raise_classified_error(
        self,
        *,
        fallback_error: Any,
        failure_ops: TaskFailureOperationsService,
        model_name: str,
        api_format: str,
    ) -> None:
        if self.last_error is not None:
            failure_ops.attach_metadata_to_error(
                self.last_error,
                self.last_candidate,
                model_name,
                api_format,
            )
            raise self.last_error

        if isinstance(fallback_error, Exception):
            raise fallback_error

        raise RuntimeError("execution_error_handler requested raise without exception context")


def resolve_execution_error_transition(
    *,
    action: CandidateErrorAction,
    state: SyncExecutionState,
    max_retries_for_candidate: int,
    retry_index: int,
) -> ExecutionErrorTransition:
    """根据异常动作分类，返回 FailoverEngine 可消费的状态流转结果。"""
    if action == CandidateErrorAction.RETRY_CURRENT:
        return ExecutionErrorTransition(
            failover_action=FailoverAction.RETRY,
            max_retries=state.consume_rectify_retry_extension(
                max_retries_for_candidate=max_retries_for_candidate,
                retry_index=retry_index,
            ),
        )

    return ExecutionErrorTransition(
        failover_action=FailoverAction.CONTINUE,
        max_retries=None,
    )
