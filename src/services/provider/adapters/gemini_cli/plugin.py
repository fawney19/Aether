"""GeminiCLI provider plugin — unified registration entry."""

from __future__ import annotations

import time
from typing import Any
from urllib.parse import urlencode

from src.core.logger import logger
from src.services.provider.adapters.gemini_cli.constants import V1INTERNAL_PATH_TEMPLATE
from src.services.provider.preset_models import get_preset_models
from src.services.provider.request_context import set_selected_base_url


async def fetch_models_gemini_cli(
    ctx: Any,
    timeout_seconds: float,
) -> tuple[list[dict], list[str], bool, dict[str, Any] | None]:
    """GeminiCLI model fetcher.

    Upstream does not expose a stable public models endpoint for OAuth CLI access,
    so we currently return a curated preset model catalog and enrich account metadata
    from loadCodeAssist when possible.
    """
    from src.services.provider.adapters.gemini_cli.client import (
        extract_plan_type,
        load_code_assist,
    )

    models = get_preset_models("gemini_cli")
    upstream_metadata: dict[str, Any] | None = None

    access_token = str(getattr(ctx, "api_key_value", "") or "").strip()
    if access_token:
        try:
            code_assist = await load_code_assist(
                access_token,
                proxy_config=getattr(ctx, "proxy_config", None),
                timeout_seconds=timeout_seconds,
            )
            provider_meta: dict[str, Any] = {"updated_at": int(time.time())}
            plan_type = extract_plan_type(code_assist)
            if plan_type:
                provider_meta["plan_type"] = plan_type
            project_id = (getattr(ctx, "auth_config", None) or {}).get("project_id")
            if isinstance(project_id, str) and project_id:
                provider_meta["project_id"] = project_id
            upstream_metadata = {"gemini_cli": provider_meta}
        except Exception as exc:
            logger.debug("GeminiCLI model metadata fetch failed: {}", exc)

    return models, [], True, upstream_metadata


def build_gemini_cli_url(
    endpoint: Any,
    *,
    is_stream: bool,
    effective_query_params: dict[str, Any],
    **_kwargs: Any,
) -> str:
    """Build GeminiCLI v1internal URL."""
    from src.utils.url_utils import join_url

    base_url = (getattr(endpoint, "base_url", "") or "").strip().rstrip("/")
    set_selected_base_url(base_url)

    action = "streamGenerateContent" if is_stream else "generateContent"
    path = V1INTERNAL_PATH_TEMPLATE.format(action=action)
    url = join_url(base_url, path)
    if is_stream:
        effective_query_params.setdefault("alt", "sse")
    if effective_query_params:
        query_string = urlencode(effective_query_params, doseq=True)
        if query_string:
            url = f"{url}?{query_string}"
    return url


async def enrich_gemini_cli(
    auth_config: dict[str, Any],
    token_response: dict[str, Any],
    access_token: str,
    proxy_config: dict[str, Any] | None,
) -> dict[str, Any]:
    """GeminiCLI auth_config enrichment: email + project_id."""
    from src.core.provider_oauth_utils import fetch_google_email
    from src.services.provider.adapters.gemini_cli.client import (
        enrich_project_id,
        extract_plan_type,
        load_code_assist,
    )

    if not auth_config.get("email"):
        email = await fetch_google_email(
            access_token,
            proxy_config=proxy_config,
            timeout_seconds=10.0,
        )
        if email:
            auth_config["email"] = email

    try:
        code_assist = await load_code_assist(access_token, proxy_config=proxy_config)
    except Exception as exc:
        code_assist = None
        logger.warning("[enrich] GeminiCLI loadCodeAssist failed: {}", exc)

    if code_assist and not auth_config.get("tier"):
        plan_type = extract_plan_type(code_assist)
        if plan_type:
            auth_config["tier"] = plan_type

    if not auth_config.get("project_id"):
        try:
            project_id = (code_assist and code_assist.get("cloudaicompanionProject")) or None
            if isinstance(project_id, dict):
                project_id = project_id.get("id")
            if isinstance(project_id, str) and project_id.strip():
                auth_config["project_id"] = project_id.strip()
            else:
                project_id = await enrich_project_id(access_token, proxy_config=proxy_config)
                if project_id:
                    auth_config["project_id"] = project_id
                    logger.info("[enrich] GeminiCLI project_id: {}", project_id[:8] + "...")
        except Exception as exc:
            logger.warning("[enrich] GeminiCLI project_id enrichment failed: {}", exc)

    return auth_config


def register_all() -> None:
    """Register all GeminiCLI hooks into shared registries."""
    from src.core.provider_oauth_utils import register_auth_enricher
    from src.services.model.upstream_fetcher import UpstreamModelsFetcherRegistry
    from src.services.provider.adapters.gemini_cli.envelope import gemini_cli_v1internal_envelope
    from src.services.provider.envelope import register_envelope
    from src.services.provider.transport import register_transport_hook

    register_envelope("gemini_cli", "gemini:cli", gemini_cli_v1internal_envelope)
    register_envelope("gemini_cli", "gemini:chat", gemini_cli_v1internal_envelope)
    register_envelope("gemini_cli", "", gemini_cli_v1internal_envelope)

    register_transport_hook("gemini_cli", "gemini:cli", build_gemini_cli_url)
    register_transport_hook("gemini_cli", "gemini:chat", build_gemini_cli_url)

    register_auth_enricher("gemini_cli", enrich_gemini_cli)

    UpstreamModelsFetcherRegistry.register(
        provider_types=["gemini_cli"],
        fetcher=fetch_models_gemini_cli,
    )


__all__ = [
    "build_gemini_cli_url",
    "enrich_gemini_cli",
    "fetch_models_gemini_cli",
    "register_all",
]
