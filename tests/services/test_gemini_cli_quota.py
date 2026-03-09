from __future__ import annotations

from types import SimpleNamespace
from typing import Any, cast

from src.services.orchestration.error_handler import ErrorHandlerService
from src.services.provider.adapters.gemini_cli.quota import (
    build_quota_exhausted_metadata,
    extract_quota_cooldown_seconds,
)


class _FakeDB:
    def __init__(self) -> None:
        self.added: list[object] = []
        self.commit_count = 0

    def add(self, obj: object) -> None:
        self.added.append(obj)

    def commit(self) -> None:
        self.commit_count += 1


def test_extract_quota_cooldown_seconds_from_message_reset_after() -> None:
    error_text = """
    {
      "error": {
        "code": 429,
        "message": "You have exhausted your capacity on this model. Your quota will reset after 27s.",
        "status": "RESOURCE_EXHAUSTED",
        "details": [
          {
            "@type": "type.googleapis.com/google.rpc.ErrorInfo",
            "reason": "RATE_LIMIT_EXCEEDED",
            "domain": "cloudcode-pa.googleapis.com",
            "metadata": {
              "model": "gemini-3-pro-preview"
            }
          }
        ]
      }
    }
    """

    assert extract_quota_cooldown_seconds(error_text, now_ts=1_700_000_000) == 27


def test_build_quota_exhausted_metadata_skips_unknown_reset_window() -> None:
    error_text = """
    {
      "error": {
        "code": 429,
        "message": "No capacity available for model gemini-3.1-pro-preview on the server",
        "status": "RESOURCE_EXHAUSTED",
        "details": [
          {
            "@type": "type.googleapis.com/google.rpc.ErrorInfo",
            "reason": "MODEL_CAPACITY_EXHAUSTED",
            "domain": "cloudcode-pa.googleapis.com",
            "metadata": {
              "model": "gemini-3.1-pro-preview"
            }
          }
        ]
      }
    }
    """

    assert (
        build_quota_exhausted_metadata(
            model_name="gemini-3.1-pro-preview",
            error_text=error_text,
            current_namespace=None,
            now_ts=1_700_000_000,
        )
        is None
    )


def test_sync_gemini_cli_quota_state_uses_error_model_not_global_model_id() -> None:
    db = _FakeDB()
    service = ErrorHandlerService(db=cast(Any, db))
    key = SimpleNamespace(id="k1", upstream_metadata={})
    provider = SimpleNamespace(provider_type="gemini_cli")
    error_text = """
    {
      "error": {
        "code": 429,
        "message": "You have exhausted your capacity on this model. Your quota will reset after 27s.",
        "status": "RESOURCE_EXHAUSTED",
        "details": [
          {
            "@type": "type.googleapis.com/google.rpc.ErrorInfo",
            "reason": "RATE_LIMIT_EXCEEDED",
            "domain": "cloudcode-pa.googleapis.com",
            "metadata": {
              "model": "gemini-3-pro-preview"
            }
          }
        ]
      }
    }
    """

    service._sync_gemini_cli_quota_state(
        key=cast(Any, key),
        provider=cast(Any, provider),
        model_name="17855939-7164-44b3-9354-f87851b25ec6",
        error_text=error_text,
        request_id="req-1",
    )

    quota_by_model = key.upstream_metadata["gemini_cli"]["quota_by_model"]
    assert "gemini-3-pro-preview" in quota_by_model
    assert "17855939-7164-44b3-9354-f87851b25ec6" not in quota_by_model
    assert db.commit_count == 1
