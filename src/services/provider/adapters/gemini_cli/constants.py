"""GeminiCLI provider constants."""

from __future__ import annotations

from src.config.settings import config
from src.core.provider_templates.fixed_providers import FIXED_PROVIDERS
from src.core.provider_templates.types import ProviderType

PROD_BASE_URL = FIXED_PROVIDERS[ProviderType.GEMINI_CLI].api_base_url
V1INTERNAL_PATH_TEMPLATE = "/v1internal:{action}"


def get_v1internal_extra_headers() -> dict[str, str]:
    """Headers required by GeminiCLI upstream requests."""
    return {
        "Accept-Encoding": "identity",
        "User-Agent": config.internal_user_agent_gemini_cli,
    }


__all__ = [
    "PROD_BASE_URL",
    "V1INTERNAL_PATH_TEMPLATE",
    "get_v1internal_extra_headers",
]
