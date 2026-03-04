from __future__ import annotations

from datetime import datetime, timezone

from sqlalchemy import update
from sqlalchemy.orm import Session

from src.models.database import RequestCandidate


class AsyncSubmitRecordService:
    """异步提交阶段的 RequestCandidate 落库服务。"""

    def __init__(self, db: Session) -> None:
        self.db = db

    def mark_pending(self, *, record_id: str | None) -> None:
        if not record_id:
            return
        started_at = datetime.now(timezone.utc)
        self.db.execute(
            update(RequestCandidate)
            .where(RequestCandidate.id == record_id)
            .values(status="pending", started_at=started_at)
        )

    def mark_failed(
        self,
        *,
        record_id: str | None,
        error_type: str,
        error_message: str,
        status_code: int | None = None,
    ) -> None:
        if not record_id:
            return

        values: dict[str, object] = {
            "status": "failed",
            "error_type": error_type,
            "error_message": error_message,
            "finished_at": datetime.now(timezone.utc),
        }
        if status_code is not None:
            values["status_code"] = status_code

        self.db.execute(
            update(RequestCandidate).where(RequestCandidate.id == record_id).values(**values)
        )

    def mark_success(
        self,
        *,
        record_id: str | None,
        status_code: int,
    ) -> None:
        if not record_id:
            return
        self.db.execute(
            update(RequestCandidate)
            .where(RequestCandidate.id == record_id)
            .values(
                status="success",
                status_code=status_code,
                finished_at=datetime.now(timezone.utc),
            )
        )
