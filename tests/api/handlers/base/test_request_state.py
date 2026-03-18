from __future__ import annotations

from src.services.task.request_state import MutableRequestBodyState


def test_mutable_request_body_state_keeps_original_and_attempts_isolated() -> None:
    original = {
        "model": "gpt-5",
        "input": [{"role": "user", "content": [{"type": "input_text", "text": "hello"}]}],
    }

    state = MutableRequestBodyState(original)

    first_attempt = state.build_attempt_body()
    first_attempt["input"][0]["content"][0]["text"] = "attempt-1"

    assert original["input"][0]["content"][0]["text"] == "hello"
    assert state.current_body["input"][0]["content"][0]["text"] == "hello"

    rectified = state.build_attempt_body()
    rectified["input"][0]["content"][0]["text"] = "rectified"
    state.mark_rectified(rectified, stage=1)

    second_attempt = state.build_attempt_body()
    second_attempt["input"][0]["content"][0]["text"] = "attempt-2"

    assert state.is_rectified() is True
    assert state.rectify_stage() == 1
    assert state.current_body["input"][0]["content"][0]["text"] == "rectified"
    assert original["input"][0]["content"][0]["text"] == "hello"
    assert state.consume_rectified_this_turn() is True
    assert state.consume_rectified_this_turn() is False
