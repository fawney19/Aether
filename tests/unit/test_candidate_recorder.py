from __future__ import annotations

from types import SimpleNamespace
from typing import Any

from src.services.candidate.recorder import CandidateRecorder


class _FakeQuery:
    def __init__(self, all_result: list[Any]) -> None:
        self._all_result = all_result

    def filter(self, *_args: Any, **_kwargs: Any) -> "_FakeQuery":
        return self

    def order_by(self, *_args: Any, **_kwargs: Any) -> "_FakeQuery":
        return self

    def all(self) -> list[Any]:
        return self._all_result


class _FakeDb:
    def __init__(self, queries: list[list[Any]]) -> None:
        self._queries = queries
        self.calls = 0

    def query(self, *_args: Any) -> _FakeQuery:
        result = self._queries[self.calls]
        self.calls += 1
        return _FakeQuery(result)


def test_candidate_recorder_prefers_provider_key_name_over_user_api_key_snapshot() -> None:
    request_candidate = SimpleNamespace(
        candidate_index=0,
        retry_index=0,
        provider_id="provider-1",
        provider=SimpleNamespace(name="CRS"),
        endpoint_id="endpoint-1",
        key_id="provider-key-1",
        api_key_name="USER-KEY-NAME",
        is_cached=False,
        status="success",
        skip_reason=None,
        error_type=None,
        error_message=None,
        status_code=200,
        latency_ms=123,
    )
    db = _FakeDb(
        [
            [request_candidate],
            [("provider-key-1", "POOL-KEY-NAME")],
        ]
    )

    result = CandidateRecorder(db).get_candidate_keys("req-1")

    assert len(result) == 1
    assert result[0].provider_name == "CRS"
    assert result[0].key_id == "provider-key-1"
    assert result[0].key_name == "POOL-KEY-NAME"
