from __future__ import annotations

from dataclasses import dataclass, field
from enum import Enum


class TaskMode(str, Enum):
    SYNC = "sync"
    ASYNC = "async"


@dataclass(slots=True)
class TaskContext:
    """
    TaskContext (pure DTO)

    - Only primitive types / IDs
    - Serializable & safe to pass across processes
    """

    request_id: str
    task_type: str  # chat/cli/video/image/audio
    task_mode: TaskMode

    user_id: str
    api_key_id: str

    client_ip: str = ""
    user_agent: str = ""
    start_time: float = 0.0

    api_format: str | None = None
    model: str | None = None
    mapped_model: str | None = None

    capability_requirements: dict[str, bool] = field(default_factory=dict)
