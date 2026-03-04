from __future__ import annotations

from enum import Enum


class TaskStatus(str, Enum):
    """Generic task status (progress)."""

    PENDING = "pending"
    STREAMING = "streaming"

    SUBMITTED = "submitted"
    QUEUED = "queued"
    PROCESSING = "processing"

    COMPLETED = "completed"
    FAILED = "failed"
    CANCELLED = "cancelled"
    EXPIRED = "expired"


class BillingStatus(str, Enum):
    """Billing settlement status (Usage.billing_status)."""

    PENDING = "pending"
    SETTLED = "settled"
    VOID = "void"
