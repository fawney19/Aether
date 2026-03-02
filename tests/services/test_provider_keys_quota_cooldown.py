from __future__ import annotations

from types import SimpleNamespace
from typing import Any, cast

from src.core.provider_types import ProviderType
from src.services.provider_keys.quota_cooldown import resolve_effective_cooldown_reason


def _key_with_metadata(upstream_metadata: dict[str, Any]) -> Any:
    return cast(Any, SimpleNamespace(upstream_metadata=upstream_metadata))


def test_resolve_effective_cooldown_reason_prefers_redis_reason() -> None:
    key = _key_with_metadata({"codex": {"primary_used_percent": 100.0}})

    reason = resolve_effective_cooldown_reason(
        provider_type=ProviderType.CODEX,
        key=key,
        redis_reason="rate_limited_429",
    )

    assert reason == "rate_limited_429"


def test_resolve_effective_cooldown_reason_fallbacks_to_codex_quota_exhausted() -> None:
    key = _key_with_metadata({"codex": {"primary_used_percent": 100.0}})

    reason = resolve_effective_cooldown_reason(
        provider_type=ProviderType.CODEX,
        key=key,
        redis_reason=None,
    )

    assert reason == "quota_exhausted"


def test_resolve_effective_cooldown_reason_fallbacks_to_kiro_quota_exhausted() -> None:
    key = _key_with_metadata({"kiro": {"remaining": 0}})

    reason = resolve_effective_cooldown_reason(
        provider_type=ProviderType.KIRO,
        key=key,
        redis_reason=None,
    )

    assert reason == "quota_exhausted"


def test_resolve_effective_cooldown_reason_returns_none_when_not_exhausted() -> None:
    key = _key_with_metadata({"codex": {"primary_used_percent": 12.0}})

    reason = resolve_effective_cooldown_reason(
        provider_type=ProviderType.CODEX,
        key=key,
        redis_reason=None,
    )

    assert reason is None
