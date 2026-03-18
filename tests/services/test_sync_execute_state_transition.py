from __future__ import annotations

from types import SimpleNamespace
from unittest.mock import MagicMock

import pytest

from src.services.candidate.policy import FailoverAction
from src.services.task.execute.exception_classification import (
    CandidateErrorAction,
    classify_candidate_error_action,
)
from src.services.task.execute.state_transition import (
    SyncExecutionState,
    resolve_execution_error_transition,
)
from src.services.task.request_state import MutableRequestBodyState


def _make_candidate() -> SimpleNamespace:
    return SimpleNamespace(
        provider=SimpleNamespace(id="p1", name="provider-1"),
        endpoint=SimpleNamespace(id="e1"),
        key=SimpleNamespace(id="k1"),
    )


@pytest.mark.parametrize(
    ("raw_action", "expected"),
    [
        ("continue", CandidateErrorAction.RETRY_CURRENT),
        ("break", CandidateErrorAction.NEXT_CANDIDATE),
        ("raise", CandidateErrorAction.RAISE_ERROR),
        ("unexpected", CandidateErrorAction.NEXT_CANDIDATE),
        (None, CandidateErrorAction.NEXT_CANDIDATE),
    ],
)
def test_classify_candidate_error_action(
    raw_action: str | None, expected: CandidateErrorAction
) -> None:
    assert classify_candidate_error_action(raw_action) == expected


def test_resolve_execution_error_transition_retry_and_consume_rectify_flag() -> None:
    request_body_state = MutableRequestBodyState({})
    request_body_state.mark_rectified({}, stage=1)
    state = SyncExecutionState(
        candidate_record_map={},
        request_body_state=request_body_state,
    )

    transition = resolve_execution_error_transition(
        action=CandidateErrorAction.RETRY_CURRENT,
        state=state,
        max_retries_for_candidate=2,
        retry_index=1,
    )

    assert transition.failover_action == FailoverAction.RETRY
    assert transition.max_retries == 3
    assert request_body_state.consume_rectified_this_turn() is False


def test_resolve_execution_error_transition_next_candidate() -> None:
    request_body_state = MutableRequestBodyState({})
    request_body_state.mark_rectified({}, stage=1)
    state = SyncExecutionState(
        candidate_record_map={},
        request_body_state=request_body_state,
    )

    transition = resolve_execution_error_transition(
        action=CandidateErrorAction.NEXT_CANDIDATE,
        state=state,
        max_retries_for_candidate=2,
        retry_index=0,
    )

    assert transition.failover_action == FailoverAction.CONTINUE
    assert transition.max_retries is None
    assert state.request_body_state is request_body_state
    assert request_body_state.consume_rectified_this_turn() is True


def test_sync_execution_state_resolve_candidate_record_id_fallback() -> None:
    state = SyncExecutionState(
        candidate_record_map={(2, 0): "r20"},
        request_body_state=None,
    )

    assert state.resolve_candidate_record_id(candidate_index=2, record_id=None) == "r20"
    assert state.resolve_candidate_record_id(candidate_index=2, record_id="r22") == "r22"


def test_sync_execution_state_raise_classified_error_uses_last_error() -> None:
    err = ValueError("boom")
    candidate = _make_candidate()
    state = SyncExecutionState(
        candidate_record_map={},
        request_body_state=None,
        last_error=err,
        last_candidate=candidate,
    )
    failure_ops = MagicMock()

    with pytest.raises(ValueError, match="boom"):
        state.raise_classified_error(
            fallback_error=RuntimeError("fallback"),
            failure_ops=failure_ops,
            model_name="gpt-4.1",
            api_format="openai_chat",
        )

    failure_ops.attach_metadata_to_error.assert_called_once_with(
        err,
        candidate,
        "gpt-4.1",
        "openai_chat",
    )


def test_sync_execution_state_raise_classified_error_fallback_error() -> None:
    state = SyncExecutionState(
        candidate_record_map={},
        request_body_state=None,
    )
    failure_ops = MagicMock()

    with pytest.raises(RuntimeError, match="fallback"):
        state.raise_classified_error(
            fallback_error=RuntimeError("fallback"),
            failure_ops=failure_ops,
            model_name="gpt-4.1",
            api_format="openai_chat",
        )

    failure_ops.attach_metadata_to_error.assert_not_called()
