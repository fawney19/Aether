"""Antigravity provider plugin — 统一注册入口。

将 Antigravity 对各通用 registry 的注册集中在一个文件中：
- Envelope (v1internal 信封)
- Transport Hook (URL 构建)
- Auth Enricher (OAuth enrichment)
- Model Fetcher (模型获取)
- Behavior Variants (格式变体)

新增 provider 时参照此文件创建对应的 plugin.py 即可。
"""

from __future__ import annotations

import time
from typing import Any
from urllib.parse import urlencode

from src.core.logger import logger
from src.services.provider.adapters.antigravity.constants import V1INTERNAL_PATH_TEMPLATE
from src.services.provider.adapters.antigravity.url_availability import url_availability
from src.services.provider.request_context import set_selected_base_url

# ---------------------------------------------------------------------------
# Transport Hook
# ---------------------------------------------------------------------------


def build_antigravity_url(
    endpoint: Any,
    *,
    is_stream: bool,
    effective_query_params: dict[str, Any],
) -> str:
    """构建 Antigravity v1internal URL。

    使用 url_availability 选择最优端点，构建 v1internal:generateContent 或
    v1internal:streamGenerateContent URL。
    """
    ordered_urls = url_availability.get_ordered_urls(prefer_daily=True)
    base_url = ordered_urls[0] if ordered_urls else endpoint.base_url

    # 存入 contextvars（供后续 Handler 层 envelope 获取）
    set_selected_base_url(str(base_url) if base_url is not None else None)

    action = "streamGenerateContent" if is_stream else "generateContent"
    path = V1INTERNAL_PATH_TEMPLATE.format(action=action)

    # v1internal 流式请求同样支持 ?alt=sse
    if is_stream:
        effective_query_params.setdefault("alt", "sse")

    url = f"{str(base_url).rstrip('/')}{path}"
    if effective_query_params:
        query_string = urlencode(effective_query_params, doseq=True)
        if query_string:
            url = f"{url}?{query_string}"

    return url


# ---------------------------------------------------------------------------
# Auth Enricher
# ---------------------------------------------------------------------------


async def enrich_antigravity(
    auth_config: dict[str, Any],
    token_response: dict[str, Any],
    access_token: str,
    proxy_config: dict[str, Any] | None,
) -> dict[str, Any]:
    """Antigravity auth_config enrichment.

    1. 通过 Google userinfo API 获取 email
    2. 通过 loadCodeAssist 获取 project_id / tier
    3. 未激活账号尝试 onboardUser
    4. fallback 到随机 project_id
    """
    from src.core.provider_oauth_utils import fetch_google_email
    from src.services.provider.adapters.antigravity.client import (
        extract_project_id,
        extract_tier_id,
        generate_fallback_project_id,
        load_code_assist,
        onboard_user,
    )

    # Email（仅在缺失时获取）
    if not auth_config.get("email"):
        email = await fetch_google_email(
            access_token,
            proxy_config=proxy_config,
            timeout_seconds=10.0,
        )
        if email:
            auth_config["email"] = email

    # Project ID + Tier（需要 loadCodeAssist 时一起获取）
    need_project = not auth_config.get("project_id")
    # Tier 每次 enrich 都重新获取（确保归一化为 Free/Pro/Ultra）
    need_tier = True

    if need_project or need_tier:
        project_id = auth_config.get("project_id", "")
        try:
            code_assist = await load_code_assist(access_token, proxy_config=proxy_config)

            # 提取 tier 信息（对齐 sub2api：优先 paidTier，fallback currentTier）
            tier_str = _extract_tier_from_code_assist(code_assist)
            if tier_str:
                auth_config["tier"] = tier_str
                logger.info("[enrich] Antigravity tier: {}", tier_str)

            if need_project:
                project_id = extract_project_id(code_assist)

                # 未激活：尝试 onboardUser
                if not project_id and code_assist.get("allowedTiers"):
                    tier_id = extract_tier_id(code_assist)
                    logger.info("[enrich] Antigravity onboardUser tier={}", tier_id)
                    project_id = await onboard_user(
                        access_token,
                        tier_id=tier_id,
                        proxy_config=proxy_config,
                    )
        except Exception as e:
            logger.warning("[enrich] Antigravity loadCodeAssist/onboardUser 失败: {}", e)

        if need_project:
            if project_id:
                auth_config["project_id"] = project_id
                logger.info("[enrich] Antigravity project_id: {}", project_id[:8] + "...")
            else:
                fallback = generate_fallback_project_id()
                auth_config["project_id"] = fallback
                logger.info("[enrich] Antigravity 随机 project_id fallback: {}", fallback)

    return auth_config


def _extract_tier_from_code_assist(code_assist: dict[str, Any]) -> str:
    """从 loadCodeAssist 响应中提取用户层级，返回 Free/Pro/Ultra。

    对齐 sub2api/CLIProxyAPI：优先 paidTier，fallback currentTier。
    兼容 tier 为字符串或 {"id": "...", "tierType": "..."} 两种格式。
    """
    # 优先 paidTier（付费订阅级别）
    paid_tier = code_assist.get("paidTier")
    tier = _normalize_tier(_extract_tier_raw(paid_tier))
    if tier:
        return tier

    # fallback currentTier
    current_tier = code_assist.get("currentTier")
    tier = _normalize_tier(_extract_tier_raw(current_tier))
    if tier:
        return tier

    return "Free"


def _extract_tier_raw(tier_obj: Any) -> str:
    """从 tier 对象中提取原始标识，兼容字符串和字典两种格式。"""
    if isinstance(tier_obj, str) and tier_obj.strip():
        return tier_obj.strip()
    if isinstance(tier_obj, dict):
        for key in ("id", "tierType"):
            val = tier_obj.get(key)
            if isinstance(val, str) and val.strip():
                return val.strip()
    return ""


def _normalize_tier(raw: str) -> str:
    """将上游 tier 标识归一化为 Free/Pro/Ultra。

    上游格式示例：
    - id: "free-tier", "g1-pro-tier", "g1-ultra-tier"
    - tierType: "FREE", "PAID"
    """
    if not raw:
        return ""
    lower = raw.lower()
    if "ultra" in lower:
        return "Ultra"
    if "pro" in lower or "paid" in lower:
        return "Pro"
    if "free" in lower or "legacy" in lower:
        return "Free"
    return raw


# ---------------------------------------------------------------------------
# Model Fetcher
# ---------------------------------------------------------------------------

# Antigravity 内部测试/调试模型，不应暴露给用户
BLOCKED_MODELS: frozenset[str] = frozenset({"chat_23310", "chat_20706"})


async def fetch_models_antigravity(
    ctx: Any,
    timeout_seconds: float,
) -> tuple[list[dict], list[str], bool, dict[str, Any] | None]:
    """Antigravity 模型获取策略。

    调用 v1internal:fetchAvailableModels 获取可用模型列表，
    解析配额信息，过滤黑名单模型。
    """
    from src.services.provider.adapters.antigravity.client import (
        AntigravityAccountForbiddenException,
        fetch_available_models,
    )

    auth_config = ctx.auth_config or {}
    project_id = auth_config.get("project_id")
    if not isinstance(project_id, str) or not project_id.strip():
        return [], ["antigravity: missing auth_config.project_id (please re-auth)"], False, None

    try:
        data = await fetch_available_models(
            ctx.api_key_value,
            project_id=project_id.strip(),
            proxy_config=ctx.proxy_config,
            timeout_seconds=timeout_seconds,
        )
    except AntigravityAccountForbiddenException:
        # 重新抛出，由上层（keys.py）处理自动停用逻辑
        raise
    except Exception as e:
        return [], [f"antigravity: fetchAvailableModels error: {e}"], False, None

    raw_models = data.get("models")
    if not isinstance(raw_models, dict):
        return [], ["antigravity: invalid response (missing models)"], False, None

    models: list[dict] = []
    quota_by_model: dict[str, dict[str, Any]] = {}

    for model_id, model_data in raw_models.items():
        if not isinstance(model_id, str) or not model_id.strip():
            continue
        if model_id.strip() in BLOCKED_MODELS:
            continue
        if not isinstance(model_data, dict):
            model_data = {}

        display_name = model_data.get("displayName")
        if not isinstance(display_name, str) or not display_name:
            display_name = model_id

        models.append(
            {
                "id": model_id,
                "owned_by": "antigravity",
                "display_name": display_name,
                "api_format": "gemini:chat",
            }
        )

        quota_info = model_data.get("quotaInfo")
        if not isinstance(quota_info, dict):
            # 没有 quotaInfo 视为配额耗尽
            quota_by_model[model_id] = {
                "remaining_fraction": 0.0,
                "used_percent": 100.0,
            }
            continue

        remaining = quota_info.get("remainingFraction")
        reset_time = quota_info.get("resetTime")

        remaining_fraction: float | None = None
        try:
            if remaining is not None:
                remaining_fraction = float(remaining)
        except Exception:
            remaining_fraction = None

        if remaining_fraction is None:
            # remainingFraction 缺失视为配额耗尽
            payload: dict[str, Any] = {
                "remaining_fraction": 0.0,
                "used_percent": 100.0,
            }
            if isinstance(reset_time, str) and reset_time.strip():
                payload["reset_time"] = reset_time.strip()
            quota_by_model[model_id] = payload
            continue

        used_percent = (1.0 - remaining_fraction) * 100.0
        if used_percent < 0:
            used_percent = 0.0
        if used_percent > 100:  # noqa: PLR2004
            used_percent = 100.0

        payload: dict[str, Any] = {
            "remaining_fraction": remaining_fraction,
            "used_percent": used_percent,
        }
        if isinstance(reset_time, str) and reset_time.strip():
            payload["reset_time"] = reset_time.strip()
        quota_by_model[model_id] = payload

    upstream_metadata: dict[str, Any] | None = None
    if quota_by_model:
        upstream_metadata = {
            "antigravity": {
                "updated_at": int(time.time()),
                "quota_by_model": quota_by_model,
            }
        }

    return models, [], True, upstream_metadata


# ---------------------------------------------------------------------------
# Export builder
# ---------------------------------------------------------------------------

_AG_SKIP_KEYS = frozenset(
    {
        "access_token",
        "expires_at",
        "updated_at",
        "token_type",
        "scope",
    }
)


def antigravity_export_builder(
    auth_config: dict[str, Any],
    upstream_metadata: dict[str, Any] | None,
) -> dict[str, Any]:
    """Antigravity 导出：保留 refresh_token / email / project_id / tier。"""
    return {
        k: v for k, v in auth_config.items() if k not in _AG_SKIP_KEYS and v is not None and v != ""
    }


# ---------------------------------------------------------------------------
# Unified Registration
# ---------------------------------------------------------------------------


def register_all() -> None:
    """一次性注册 Antigravity 的所有 hooks 到各通用 registry。"""
    from src.core.provider_oauth_utils import register_auth_enricher
    from src.services.model.upstream_fetcher import UpstreamModelsFetcherRegistry
    from src.services.provider.adapters.antigravity.envelope import antigravity_v1internal_envelope
    from src.services.provider.behavior import register_behavior_variant
    from src.services.provider.envelope import register_envelope
    from src.services.provider.export import register_export_builder
    from src.services.provider.transport import register_transport_hook

    # Envelope
    register_envelope("antigravity", "gemini:chat", antigravity_v1internal_envelope)
    # Backward compat: allow existing endpoints that still use the old signature.
    register_envelope("antigravity", "gemini:cli", antigravity_v1internal_envelope)
    register_envelope("antigravity", "", antigravity_v1internal_envelope)

    # Transport
    register_transport_hook("antigravity", "gemini:chat", build_antigravity_url)
    # Backward compat: allow existing endpoints that still use the old signature.
    register_transport_hook("antigravity", "gemini:cli", build_antigravity_url)

    # Auth
    register_auth_enricher("antigravity", enrich_antigravity)

    # Export
    register_export_builder("antigravity", antigravity_export_builder)

    # Model Fetcher
    UpstreamModelsFetcherRegistry.register(
        provider_types=["antigravity"],
        fetcher=fetch_models_antigravity,
    )

    # Behavior
    register_behavior_variant("antigravity", cross_format=True)
