"""Helpers for building Kiro generateAssistantResponse requests."""

from __future__ import annotations

from typing import Any

from src.services.provider.adapters.kiro.constants import KIRO_GENERATE_ASSISTANT_PATH
from src.services.provider.adapters.kiro.context import KiroRequestContext
from src.services.provider.adapters.kiro.converter import (
    convert_claude_messages_to_conversation_state,
)
from src.services.provider.adapters.kiro.headers import build_generate_assistant_headers
from src.services.provider.adapters.kiro.models.credentials import KiroAuthConfig
from src.services.provider.adapters.kiro.token_manager import generate_machine_id


def is_kiro_thinking_enabled(request_body: dict[str, Any]) -> bool:
    thinking = request_body.get("thinking")
    if not isinstance(thinking, dict):
        return False
    ttype = str(thinking.get("type") or "").strip().lower()
    return ttype in {"enabled", "adaptive"}


def build_kiro_request_context(
    request_body: dict[str, Any],
    *,
    cfg: KiroAuthConfig,
) -> KiroRequestContext:
    return KiroRequestContext(
        region=cfg.effective_api_region(),
        machine_id=generate_machine_id(cfg),
        kiro_version=cfg.kiro_version,
        system_version=cfg.system_version,
        node_version=cfg.node_version,
        thinking_enabled=is_kiro_thinking_enabled(request_body),
    )


def build_kiro_request_headers(
    cfg: KiroAuthConfig,
    *,
    access_token: str | None = None,
) -> dict[str, str]:
    region = cfg.effective_api_region()
    host = f"q.{region}.amazonaws.com"
    return build_generate_assistant_headers(
        host=host,
        access_token=access_token,
        machine_id=generate_machine_id(cfg),
        kiro_version=cfg.kiro_version,
        system_version=cfg.system_version,
        node_version=cfg.node_version,
    )


def resolve_kiro_base_url(base_url: str, *, cfg: KiroAuthConfig) -> str:
    resolved = str(base_url or "").rstrip("/")
    region = cfg.effective_api_region()
    if "{region}" in resolved:
        resolved = resolved.replace("{region}", region)
    return resolved


def build_kiro_generate_assistant_url(base_url: str, *, cfg: KiroAuthConfig) -> str:
    from src.utils.url_utils import join_url

    resolved = resolve_kiro_base_url(base_url, cfg=cfg)
    return join_url(resolved, KIRO_GENERATE_ASSISTANT_PATH)


def build_kiro_inference_config(request_body: dict[str, Any]) -> dict[str, Any] | None:
    inference_config: dict[str, Any] = {}

    max_tokens = request_body.get("max_tokens")
    try:
        max_tokens_i = int(max_tokens) if max_tokens is not None else 0
    except Exception:
        max_tokens_i = 0
    if max_tokens_i > 0:
        inference_config["maxTokens"] = max_tokens_i

    temperature = request_body.get("temperature")
    try:
        temperature_f = float(temperature) if temperature is not None else None
    except Exception:
        temperature_f = None
    if temperature_f is not None and temperature_f >= 0:
        inference_config["temperature"] = temperature_f

    top_p = request_body.get("top_p")
    try:
        top_p_f = float(top_p) if top_p is not None else None
    except Exception:
        top_p_f = None
    if top_p_f is not None and top_p_f > 0:
        inference_config["topP"] = top_p_f

    return inference_config or None


def get_profile_arn_for_payload(cfg: KiroAuthConfig) -> str | None:
    profile_arn = str(cfg.profile_arn or "").strip()
    if not profile_arn:
        return None

    from src.services.provider.adapters.kiro.models.credentials import _normalize_auth_method

    if _normalize_auth_method(cfg.auth_method) == "idc":
        return None

    return profile_arn


def build_kiro_request_payload(
    request_body: dict[str, Any],
    *,
    model: str,
    cfg: KiroAuthConfig,
) -> dict[str, Any]:
    payload: dict[str, Any] = {
        "conversationState": convert_claude_messages_to_conversation_state(
            request_body,
            model=model,
        )
    }

    inference_config = build_kiro_inference_config(request_body)
    if inference_config:
        payload["inferenceConfig"] = inference_config

    profile_arn = get_profile_arn_for_payload(cfg)
    if profile_arn:
        payload["profileArn"] = profile_arn

    return payload


__all__ = [
    "build_kiro_generate_assistant_url",
    "build_kiro_inference_config",
    "build_kiro_request_context",
    "build_kiro_request_headers",
    "build_kiro_request_payload",
    "get_profile_arn_for_payload",
    "is_kiro_thinking_enabled",
    "resolve_kiro_base_url",
]
