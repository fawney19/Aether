"""GeminiCLI upstream client helpers."""

from __future__ import annotations

from typing import Any

from src.clients.http_client import HTTPClientPool
from src.core.logger import logger
from src.services.provider.adapters.gemini_cli.constants import (
    PROD_BASE_URL,
    get_v1internal_extra_headers,
)

_CODE_ASSIST_METADATA = {
    "ideType": "ANTIGRAVITY",
    "platform": "PLATFORM_UNSPECIFIED",
    "pluginType": "GEMINI",
}


def _extract_tier_raw(tier_obj: Any) -> str:
    """Extract raw tier string from loadCodeAssist response objects."""
    if isinstance(tier_obj, str) and tier_obj.strip():
        return tier_obj.strip()
    if isinstance(tier_obj, dict):
        for key in ("id", "tierType"):
            value = tier_obj.get(key)
            if isinstance(value, str) and value.strip():
                return value.strip()
    return ""


def extract_plan_type(data: dict[str, Any]) -> str | None:
    """Best-effort normalized plan type for GeminiCLI OAuth accounts."""
    from src.core.oauth_plan import normalize_oauth_plan_type

    for key in ("paidTier", "currentTier"):
        raw = _extract_tier_raw(data.get(key))
        normalized = normalize_oauth_plan_type(raw)
        if normalized:
            return normalized
    return None


def extract_project_id(data: dict[str, Any]) -> str:
    """Extract project_id from loadCodeAssist/onboardUser responses."""
    raw = data.get("cloudaicompanionProject")
    if isinstance(raw, str) and raw.strip():
        return raw.strip()
    if isinstance(raw, dict):
        project_id = raw.get("id", "")
        if isinstance(project_id, str) and project_id.strip():
            return project_id.strip()
    return ""


def extract_tier_id(data: dict[str, Any]) -> str:
    """Choose a tier ID for onboarding when the account is not activated."""
    allowed_tiers = data.get("allowedTiers")
    if not isinstance(allowed_tiers, list):
        return ""

    for tier in allowed_tiers:
        if isinstance(tier, dict) and tier.get("isDefault") is True:
            tier_id = tier.get("id", "")
            if isinstance(tier_id, str) and tier_id.strip():
                return tier_id.strip()

    for tier in allowed_tiers:
        if isinstance(tier, dict):
            tier_id = tier.get("id", "")
            if isinstance(tier_id, str) and tier_id.strip():
                return tier_id.strip()

    return ""


async def load_code_assist(
    access_token: str,
    proxy_config: dict[str, Any] | None = None,
    *,
    timeout_seconds: float = 10.0,
) -> dict[str, Any]:
    """Load GeminiCLI account metadata from v1internal:loadCodeAssist."""
    if not access_token:
        raise ValueError("missing access_token")

    client = await HTTPClientPool.get_proxy_client(proxy_config)
    headers = {
        "Authorization": f"Bearer {access_token}",
        "Content-Type": "application/json",
        **get_v1internal_extra_headers(),
    }
    resp = await client.post(
        f"{PROD_BASE_URL.rstrip('/')}/v1internal:loadCodeAssist",
        json={"metadata": _CODE_ASSIST_METADATA},
        headers=headers,
        timeout=timeout_seconds,
    )
    if 200 <= resp.status_code < 300:
        data = resp.json()
        return data if isinstance(data, dict) else {}
    raise RuntimeError(
        f"loadCodeAssist failed: status={resp.status_code} body={resp.text[:200] if resp.text else ''}"
    )


async def onboard_user(
    access_token: str,
    *,
    tier_id: str,
    proxy_config: dict[str, Any] | None = None,
    timeout_seconds: float = 30.0,
    max_attempts: int = 5,
    poll_interval: float = 2.0,
) -> str:
    """Activate GeminiCLI user and fetch project_id via v1internal:onboardUser."""
    if not access_token:
        raise ValueError("missing access_token")
    if not tier_id:
        raise ValueError("missing tier_id")

    client = await HTTPClientPool.get_proxy_client(proxy_config)
    headers = {
        "Authorization": f"Bearer {access_token}",
        "Content-Type": "application/json",
        **get_v1internal_extra_headers(),
    }
    body = {
        "tierId": tier_id,
        "metadata": _CODE_ASSIST_METADATA,
    }
    url = f"{PROD_BASE_URL.rstrip('/')}/v1internal:onboardUser"

    for attempt in range(1, max_attempts + 1):
        resp = await client.post(url, json=body, headers=headers, timeout=timeout_seconds)
        if not (200 <= resp.status_code < 300):
            raise RuntimeError(
                f"onboardUser failed: status={resp.status_code} body={resp.text[:200] if resp.text else ''}"
            )

        data = resp.json()
        if not isinstance(data, dict):
            raise RuntimeError(f"onboardUser: unexpected response type: {type(data)}")

        if data.get("done") is True:
            response_data = data.get("response")
            if isinstance(response_data, dict):
                return extract_project_id(response_data)
            return ""

        if attempt < max_attempts:
            import asyncio

            await asyncio.sleep(poll_interval)

    raise RuntimeError(f"onboardUser: timeout after {max_attempts} attempts")


async def enrich_project_id(
    access_token: str,
    proxy_config: dict[str, Any] | None = None,
) -> str | None:
    """Best-effort project_id resolution for GeminiCLI OAuth keys."""
    code_assist = await load_code_assist(access_token, proxy_config=proxy_config)
    project_id = extract_project_id(code_assist)
    if project_id:
        return project_id

    tier_id = extract_tier_id(code_assist)
    if tier_id:
        try:
            project_id = await onboard_user(
                access_token,
                tier_id=tier_id,
                proxy_config=proxy_config,
            )
            if project_id:
                return project_id
        except Exception as exc:
            logger.warning("GeminiCLI onboardUser failed: {}", exc)

    return None


__all__ = [
    "enrich_project_id",
    "extract_plan_type",
    "extract_project_id",
    "extract_tier_id",
    "load_code_assist",
    "onboard_user",
]
