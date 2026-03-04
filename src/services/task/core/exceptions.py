from __future__ import annotations


class StreamProbeError(RuntimeError):
    """Streaming probe failed before first chunk (eligible for failover)."""

    def __init__(
        self,
        message: str,
        *,
        http_status: int,
        original_exception: Exception | None = None,
    ) -> None:
        super().__init__(message)
        self.http_status = http_status
        self.original_exception = original_exception


class TaskNotFoundError(LookupError):
    """Task not found (by internal id or external id)."""

    def __init__(self, task_id: str) -> None:
        super().__init__(f"Task not found: {task_id}")
        self.task_id = task_id
