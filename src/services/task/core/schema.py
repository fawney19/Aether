from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any

from src.services.candidate.schema import CandidateKey
from src.services.scheduling.aware_scheduler import ProviderCandidate

from .protocol import AttemptKind, AttemptResult


@dataclass(slots=True)
class ExecutionResult:
    """FailoverEngine.execute() unified result."""

    success: bool

    # payload (filled based on AttemptKind)
    attempt_result: AttemptResult | None = None

    # selected candidate
    candidate: ProviderCandidate | None = None
    candidate_index: int = -1
    retry_index: int = 0

    provider_id: str | None = None
    provider_name: str | None = None
    endpoint_id: str | None = None
    key_id: str | None = None

    # audit
    candidate_keys: list[CandidateKey] = field(default_factory=list)
    attempt_count: int = 0
    request_candidate_id: str | None = None

    # pool scheduling summary (populated when pool mode is active)
    pool_summary: dict[str, Any] | None = None

    # failure
    error_type: str | None = None
    error_message: str | None = None
    last_status_code: int | None = None

    @property
    def response(self) -> Any:
        """Compatibility accessor: returns response body or stream iterator."""
        if not self.attempt_result:
            return None
        if self.attempt_result.kind == AttemptKind.STREAM:
            return self.attempt_result.stream_iterator
        return self.attempt_result.response_body

    @property
    def provider_task_id(self) -> str | None:
        if self.attempt_result and self.attempt_result.kind == AttemptKind.ASYNC_SUBMIT:
            return self.attempt_result.provider_task_id
        return None


@dataclass(slots=True)
class TaskStatusResult:
    """Generic task status payload returned by TaskService.poll()."""

    task_id: str
    status: str

    progress_percent: int | None = None
    result_url: str | None = None
    error_message: str | None = None

    # optional metadata (best-effort)
    provider_id: str | None = None
    provider_name: str | None = None
    endpoint_id: str | None = None
    key_id: str | None = None
