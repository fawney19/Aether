from __future__ import annotations

import json
from typing import Any

from src.core.crypto import crypto_service


def normalize_oauth_plan_type(plan_type: Any) -> str | None:
    if not isinstance(plan_type, str):
        return None
    normalized = plan_type.strip().lower()
    if not normalized:
        return None
    return normalized


def extract_oauth_plan_type_from_auth_config_data(auth_config: Any) -> str | None:
    if not isinstance(auth_config, dict):
        return None

    # Codex: plan_type (free/plus/team/enterprise)
    plan_type = normalize_oauth_plan_type(auth_config.get("plan_type"))
    if plan_type:
        return plan_type

    # Antigravity: tier (PAID/FREE/...)
    tier = normalize_oauth_plan_type(auth_config.get("tier"))
    if tier:
        return tier

    return None


def decrypt_auth_config_to_dict(
    encrypted_auth_config: str | None,
    *,
    silent: bool = True,
) -> dict[str, Any] | None:
    if not encrypted_auth_config:
        return None
    try:
        decrypted = crypto_service.decrypt(encrypted_auth_config, silent=silent)
        parsed = json.loads(decrypted)
        if isinstance(parsed, dict):
            return parsed
    except Exception:
        return None
    return None


def _strip_provider_prefix(value: str, provider_type: str | None = None) -> str:
    normalized = value.strip()
    if not normalized:
        return ""

    prefixes: list[str] = []
    if isinstance(provider_type, str) and provider_type.strip():
        prefixes.append(provider_type.strip())
    # Kiro 目前将套餐信息记录为 "KIRO FREE" / "KIRO PRO+"。
    if "kiro" not in {p.lower() for p in prefixes}:
        prefixes.append("kiro")

    upper = normalized.upper()
    for prefix in prefixes:
        prefix_upper = prefix.upper()
        if upper == prefix_upper:
            return ""
        if upper.startswith(f"{prefix_upper} "):
            return normalized[len(prefix) :].strip()

    return normalized


def extract_oauth_plan_type_from_upstream_metadata(
    upstream_metadata: Any,
    *,
    provider_type: str | None = None,
) -> str | None:
    if not isinstance(upstream_metadata, dict):
        return None

    kiro_meta = upstream_metadata.get("kiro")
    if isinstance(kiro_meta, dict):
        subscription_title = kiro_meta.get("subscription_title")
        if isinstance(subscription_title, str):
            normalized = _strip_provider_prefix(subscription_title, provider_type=provider_type)
            return normalize_oauth_plan_type(normalized)

    return None


def extract_oauth_plan_type(
    encrypted_auth_config: str | None,
    *,
    upstream_metadata: Any = None,
    provider_type: str | None = None,
    silent: bool = True,
) -> str | None:
    auth_config = decrypt_auth_config_to_dict(encrypted_auth_config, silent=silent)
    plan_type = extract_oauth_plan_type_from_auth_config_data(auth_config)
    if plan_type:
        return plan_type
    return extract_oauth_plan_type_from_upstream_metadata(
        upstream_metadata, provider_type=provider_type
    )


def normalize_antigravity_tier(raw_tier: Any) -> str | None:
    normalized = normalize_oauth_plan_type(raw_tier)
    if not normalized:
        return None
    if "ultra" in normalized:
        return "ultra"
    if "pro" in normalized or "paid" in normalized:
        return "pro"
    if "free" in normalized or "legacy" in normalized:
        return "free"
    return normalized


def _extract_antigravity_tier_raw(tier_obj: Any) -> str | None:
    if isinstance(tier_obj, str):
        stripped = tier_obj.strip()
        return stripped or None
    if isinstance(tier_obj, dict):
        for key in ("id", "tierType"):
            value = tier_obj.get(key)
            if isinstance(value, str):
                stripped = value.strip()
                if stripped:
                    return stripped
    return None


def _format_antigravity_tier_label(
    normalized_tier: str,
    *,
    fallback_raw: str | None = None,
) -> str:
    if normalized_tier == "ultra":
        return "Ultra"
    if normalized_tier == "pro":
        return "Pro"
    if normalized_tier == "free":
        return "Free"
    if fallback_raw:
        return fallback_raw
    return normalized_tier


def extract_antigravity_tier_from_code_assist(code_assist: Any) -> str:
    if not isinstance(code_assist, dict):
        return "Free"

    paid_tier_raw = _extract_antigravity_tier_raw(code_assist.get("paidTier"))
    paid_tier = normalize_antigravity_tier(paid_tier_raw)
    if paid_tier:
        return _format_antigravity_tier_label(paid_tier, fallback_raw=paid_tier_raw)

    current_tier_raw = _extract_antigravity_tier_raw(code_assist.get("currentTier"))
    current_tier = normalize_antigravity_tier(current_tier_raw)
    if current_tier:
        return _format_antigravity_tier_label(current_tier, fallback_raw=current_tier_raw)

    return "Free"
