from __future__ import annotations

from sqlalchemy.orm import Session

from src.models.database import ProviderAPIKey, RequestCandidate

from .schema import CandidateKey


class CandidateRecorder:
    """Read helpers for RequestCandidate audit data."""

    def __init__(self, db: Session):
        self.db = db

    def get_candidate_keys(self, request_id: str) -> list[CandidateKey]:
        rows: list[RequestCandidate] = (
            self.db.query(RequestCandidate)
            .filter(RequestCandidate.request_id == request_id)
            .order_by(RequestCandidate.candidate_index.asc(), RequestCandidate.retry_index.asc())
            .all()
        )

        provider_key_ids = {
            str(row.key_id)
            for row in rows
            if getattr(row, "key_id", None)
        }
        provider_key_name_map: dict[str, str] = {}
        if provider_key_ids:
            provider_key_rows = (
                self.db.query(ProviderAPIKey.id, ProviderAPIKey.name)
                .filter(ProviderAPIKey.id.in_(provider_key_ids))
                .all()
            )
            provider_key_name_map = {
                str(key_id): str(name)
                for key_id, name in provider_key_rows
                if key_id and name
            }

        result: list[CandidateKey] = []
        for row in rows:
            provider_name = None
            if getattr(row, "provider", None) is not None:
                provider_name = getattr(row.provider, "name", None)

            key_name = provider_key_name_map.get(str(row.key_id)) if getattr(row, "key_id", None) else None

            result.append(
                CandidateKey(
                    candidate_index=int(row.candidate_index or 0),
                    retry_index=int(row.retry_index or 0),
                    provider_id=str(row.provider_id) if row.provider_id else None,
                    provider_name=str(provider_name) if provider_name else None,
                    endpoint_id=str(row.endpoint_id) if row.endpoint_id else None,
                    key_id=str(row.key_id) if row.key_id else None,
                    key_name=str(key_name) if key_name else None,
                    is_cached=bool(getattr(row, "is_cached", False)),
                    status=str(getattr(row, "status", "") or "pending"),
                    skip_reason=getattr(row, "skip_reason", None),
                    error_type=getattr(row, "error_type", None),
                    error_message=getattr(row, "error_message", None),
                    status_code=getattr(row, "status_code", None),
                    latency_ms=getattr(row, "latency_ms", None),
                )
            )

        return result
