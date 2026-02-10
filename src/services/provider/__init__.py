"""
Provider 服务模块

包含 Provider 管理、格式处理、传输层等功能。
"""

from __future__ import annotations

from typing import TYPE_CHECKING, Any

if TYPE_CHECKING:
    from src.services.provider.format import normalize_endpoint_signature
    from src.services.provider.service import ProviderService
    from src.services.provider.transport import build_provider_url

__all__ = [
    "ProviderService",
    "normalize_endpoint_signature",
    "build_provider_url",
]


def __getattr__(name: str) -> Any:
    """Lazy attribute access to avoid import-time side effects.

    Importing `src.services.provider` should not eagerly import the whole provider
    service stack (which can create circular imports during test collection).
    """

    if name == "ProviderService":
        from src.services.provider.service import ProviderService as _ProviderService

        return _ProviderService
    if name == "normalize_endpoint_signature":
        from src.services.provider.format import (
            normalize_endpoint_signature as _normalize_endpoint_signature,
        )

        return _normalize_endpoint_signature
    if name == "build_provider_url":
        from src.services.provider.transport import build_provider_url as _build_provider_url

        return _build_provider_url

    raise AttributeError(f"module {__name__!r} has no attribute {name!r}")

