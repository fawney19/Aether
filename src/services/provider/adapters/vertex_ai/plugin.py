"""Vertex AI provider plugin — 统一注册入口。

注册 Vertex AI 对各通用 registry / capability registry 的 hooks：
- Transport Hook (URL 构建：Gemini 走 Express mode，Claude 走 Service Account)
- Model Fetcher (专用上游模型获取链路)
- Provider Format Capability（跨格式支持：同一 Provider 可配置 Gemini / Claude）
"""

from __future__ import annotations

from typing import Any

import httpx

from src.core.logger import logger
from src.core.vertex_auth import VertexAuthError, VertexAuthService
from src.services.provider.adapters.vertex_ai.transport import get_effective_format

# Vertex AI 公共 API 根
_VERTEX_API_BASE = "https://aiplatform.googleapis.com"

_MODEL_PAGE_SIZE = 100
_MODEL_MAX_PAGES = 20


def _normalize_extra_headers(raw: Any) -> dict[str, str]:
    if not isinstance(raw, dict):
        return {}
    return {str(k): str(v) for k, v in raw.items() if k and v is not None}


def _looks_like_service_account(auth_config: dict[str, Any] | None) -> bool:
    if not isinstance(auth_config, dict):
        return False
    return all(
        isinstance(auth_config.get(k), str) and str(auth_config.get(k)).strip()
        for k in ("client_email", "private_key", "project_id")
    )


def _extract_model_id(raw_name: str) -> str:
    name = str(raw_name or "").strip()
    if not name:
        return ""
    if "/models/" in name:
        return name.split("/models/", 1)[-1].strip()
    if name.startswith("models/"):
        return name.split("models/", 1)[-1].strip()
    return name


def _extract_publisher(item: dict[str, Any], fallback: str | None = None) -> str | None:
    publisher = item.get("publisher")
    if isinstance(publisher, str) and publisher.strip():
        return publisher.strip()

    raw_name = item.get("name")
    if isinstance(raw_name, str) and "/publishers/" in raw_name:
        try:
            after = raw_name.split("/publishers/", 1)[1]
            candidate = after.split("/", 1)[0].strip()
            if candidate:
                return candidate
        except Exception:
            pass

    return fallback


def _extract_items(data: Any) -> list[dict[str, Any]]:
    if isinstance(data, list):
        return [item for item in data if isinstance(item, dict)]
    if not isinstance(data, dict):
        return []

    for key in ("publisherModels", "models", "data", "items"):
        value = data.get(key)
        if isinstance(value, list):
            return [item for item in value if isinstance(item, dict)]
    return []


def _parse_models_payload(
    data: Any,
    *,
    auth_config: dict[str, Any] | None,
    fallback_publisher: str | None = None,
) -> list[dict[str, Any]]:
    models: list[dict[str, Any]] = []
    for item in _extract_items(data):
        raw_name = item.get("id") or item.get("name") or item.get("model")
        if not isinstance(raw_name, str):
            continue

        model_id = _extract_model_id(raw_name)
        if not model_id:
            continue

        display_name_raw = (
            item.get("displayName") or item.get("display_name") or item.get("title") or model_id
        )
        display_name = (
            str(display_name_raw).strip() if isinstance(display_name_raw, str) else model_id
        )
        if not display_name:
            display_name = model_id

        models.append(
            {
                "id": model_id,
                "owned_by": _extract_publisher(item, fallback=fallback_publisher),
                "display_name": display_name,
                "api_format": get_effective_format(model_id, auth_config),
            }
        )

    return models


def _build_google_publisher_list_url(base_url: str) -> str:
    from src.utils.url_utils import join_url

    base = (base_url or "").strip().rstrip("/") or _VERTEX_API_BASE
    return join_url(base, "/v1/publishers/google/models")


def _iter_endpoint_base_urls(ctx: Any) -> list[str]:
    seen: set[str] = set()
    urls: list[str] = []
    for cfg in (ctx.format_to_endpoint or {}).values():
        base_url = str(getattr(cfg, "base_url", "") or "").strip()
        if not base_url:
            continue
        norm = base_url.rstrip("/")
        if norm in seen:
            continue
        seen.add(norm)
        urls.append(norm)

    if _VERTEX_API_BASE not in seen:
        urls.append(_VERTEX_API_BASE)
    return urls


def _get_endpoint_headers(ctx: Any, api_format: str) -> dict[str, str]:
    cfg = (ctx.format_to_endpoint or {}).get(api_format)
    return _normalize_extra_headers(getattr(cfg, "extra_headers", None))


def _dedupe_models(models: list[dict[str, Any]]) -> list[dict[str, Any]]:
    seen: set[str] = set()
    result: list[dict[str, Any]] = []
    for model in models:
        model_id = str(model.get("id", "")).strip()
        api_format = str(model.get("api_format", "")).strip()
        if not model_id:
            continue
        unique_key = f"{model_id}:{api_format}"
        if unique_key in seen:
            continue
        seen.add(unique_key)
        result.append(model)
    return result


def _is_soft_not_found(error: str) -> bool:
    return str(error).strip().startswith("HTTP 404:")


def _iter_regions(auth_config: dict[str, Any] | None) -> list[str]:
    seen: set[str] = set()
    regions: list[str] = []

    def _add(raw: Any) -> None:
        if not isinstance(raw, str):
            return
        region = raw.strip()
        if not region or region in seen:
            return
        seen.add(region)
        regions.append(region)

    if isinstance(auth_config, dict):
        _add(auth_config.get("region"))
        model_regions = auth_config.get("model_regions")
        if isinstance(model_regions, dict):
            for region in model_regions.values():
                _add(region)

    _add("global")
    _add("us-central1")
    return regions


async def _fetch_models_from_url(
    client: httpx.AsyncClient,
    *,
    url: str,
    headers: dict[str, str],
    params: dict[str, Any],
    auth_config: dict[str, Any] | None,
    fallback_publisher: str | None = None,
) -> tuple[list[dict[str, Any]], str | None, bool]:
    all_models: list[dict[str, Any]] = []
    next_page_token: str | None = None
    has_success = False

    for _ in range(_MODEL_MAX_PAGES):
        req_params = dict(params)
        if next_page_token:
            req_params["pageToken"] = next_page_token

        try:
            resp = await client.get(url, headers=headers, params=req_params)
        except httpx.TimeoutException:
            return [], "timeout", has_success
        except Exception as exc:
            return [], f"request error: {exc}", has_success

        if resp.status_code != 200:
            body = resp.text[:500] if resp.text else "(empty)"
            return [], f"HTTP {resp.status_code}: {body}", has_success

        has_success = True

        try:
            payload = resp.json()
        except Exception:
            body = resp.text[:500] if resp.text else "(empty)"
            return [], f"invalid json body: {body}", has_success

        all_models.extend(
            _parse_models_payload(
                payload,
                auth_config=auth_config,
                fallback_publisher=fallback_publisher,
            )
        )

        if not isinstance(payload, dict):
            break

        token = payload.get("nextPageToken")
        next_page_token = str(token).strip() if isinstance(token, str) else None
        if not next_page_token:
            break

    return all_models, None, has_success


async def _fetch_models_vertex_api_key(
    client: httpx.AsyncClient,
    *,
    ctx: Any,
    auth_config: dict[str, Any] | None,
) -> tuple[list[dict[str, Any]], list[str], bool]:
    """API Key 仅抓取 Vertex AI Express mode 的 Google publisher models。"""
    api_key = str(ctx.api_key_value or "").strip()
    if not api_key or api_key == "__placeholder__":
        return [], ["vertex_ai(api_key): missing api key"], False

    all_models: list[dict[str, Any]] = []
    hard_errors: list[str] = []
    soft_errors: list[str] = []
    has_success = False

    endpoint_headers = _get_endpoint_headers(ctx, "gemini:chat")
    vertex_list_urls = [
        _build_google_publisher_list_url(base) for base in _iter_endpoint_base_urls(ctx)
    ]

    # Vertex Express mode list (publisher=google)
    for url in vertex_list_urls:
        headers = {"Accept": "application/json", **endpoint_headers}
        models, err, success = await _fetch_models_from_url(
            client,
            url=url,
            headers=headers,
            params={"key": api_key, "pageSize": _MODEL_PAGE_SIZE},
            auth_config=auth_config,
            fallback_publisher="google",
        )
        if success:
            has_success = True
        if err:
            labeled = f"{url}: {err}"
            if _is_soft_not_found(err):
                soft_errors.append(labeled)
            else:
                hard_errors.append(labeled)
            continue
        all_models.extend(models)

    deduped = _dedupe_models(all_models)
    if deduped:
        return deduped, hard_errors, has_success or True

    if hard_errors:
        return [], hard_errors, has_success
    if soft_errors:
        return [], [soft_errors[0]], has_success
    return [], [], has_success


async def _fetch_models_vertex_service_account(
    client: httpx.AsyncClient,
    *,
    ctx: Any,
    auth_config: dict[str, Any] | None,
    client_kwargs: dict[str, Any],
) -> tuple[list[dict[str, Any]], list[str], bool]:
    """Service Account 抓取 Vertex AI Google + Anthropic publisher models。"""
    if not isinstance(auth_config, dict):
        return [], ["vertex_ai(service_account): missing auth_config"], False

    try:
        auth_service = VertexAuthService(auth_config)
        access_token = await auth_service.get_access_token(httpx_client_kwargs=client_kwargs)
    except VertexAuthError as exc:
        return [], [f"vertex_ai(service_account): auth failed: {exc}"], False
    except Exception as exc:
        return [], [f"vertex_ai(service_account): auth failed: {exc}"], False

    project_id = str(auth_config.get("project_id") or "").strip()
    if not project_id:
        return [], ["vertex_ai(service_account): missing project_id"], False

    all_models: list[dict[str, Any]] = []
    hard_errors: list[str] = []
    soft_errors: list[str] = []
    has_success = False

    gemini_headers = {"Accept": "application/json", **_get_endpoint_headers(ctx, "gemini:chat")}
    claude_headers = {"Accept": "application/json", **_get_endpoint_headers(ctx, "claude:chat")}
    gemini_headers["Authorization"] = f"Bearer {access_token}"
    claude_headers["Authorization"] = f"Bearer {access_token}"

    for region in _iter_regions(auth_config):
        base = (
            _VERTEX_API_BASE
            if region == "global"
            else f"https://{region}-aiplatform.googleapis.com"
        )

        requests = [
            (
                "google",
                f"{base}/v1/projects/{project_id}/locations/{region}/publishers/google/models",
                gemini_headers,
            ),
            (
                "anthropic",
                f"{base}/v1/projects/{project_id}/locations/{region}/publishers/anthropic/models",
                claude_headers,
            ),
        ]

        for publisher, url, headers in requests:
            models, err, success = await _fetch_models_from_url(
                client,
                url=url,
                headers=headers,
                params={"pageSize": _MODEL_PAGE_SIZE},
                auth_config=auth_config,
                fallback_publisher=publisher,
            )
            if success:
                has_success = True
            if err:
                labeled = f"{url}: {err}"
                if _is_soft_not_found(err):
                    soft_errors.append(labeled)
                else:
                    hard_errors.append(labeled)
                continue
            all_models.extend(models)

    deduped = _dedupe_models(all_models)
    if deduped:
        return deduped, hard_errors, has_success or True

    if hard_errors:
        return [], hard_errors, has_success
    if soft_errors:
        return [], [soft_errors[0]], has_success
    return [], [], has_success


async def fetch_models_vertex_ai(
    ctx: Any,
    timeout_seconds: float,
) -> tuple[list[dict], list[str], bool, dict[str, Any] | None]:
    """Vertex AI 专用模型获取链路。

    - API Key: 仅请求 Vertex AI Express mode 的 Gemini models
    - Service Account: 使用 SA 凭证换取 Bearer Token，按 region 查询 Gemini + Claude models
    """
    from src.services.proxy_node.resolver import build_proxy_client_kwargs

    auth_config = ctx.auth_config if isinstance(ctx.auth_config, dict) else None
    is_service_account = _looks_like_service_account(auth_config)

    client_kwargs = build_proxy_client_kwargs(ctx.proxy_config, timeout=timeout_seconds)

    async with httpx.AsyncClient(**client_kwargs) as client:
        if is_service_account:
            models, errors, has_success = await _fetch_models_vertex_service_account(
                client,
                ctx=ctx,
                auth_config=auth_config,
                client_kwargs=client_kwargs,
            )
        else:
            models, errors, has_success = await _fetch_models_vertex_api_key(
                client,
                ctx=ctx,
                auth_config=auth_config,
            )

    if not models and errors:
        logger.warning("Vertex 模型获取失败: {}", "; ".join(errors))
    return models, errors, has_success, None


def register_all() -> None:
    """一次性注册 Vertex AI 的所有 hooks 到各通用 registry。"""
    from src.core.api_format.capabilities import register_provider_behavior_variant
    from src.services.model.upstream_fetcher import UpstreamModelsFetcherRegistry
    from src.services.provider.adapters.vertex_ai.transport import build_vertex_ai_url
    from src.services.provider.transport import register_transport_hook

    # Transport: Vertex AI 同时支持 gemini:chat 和 claude:chat 格式
    register_transport_hook("vertex_ai", "gemini:chat", build_vertex_ai_url)
    register_transport_hook("vertex_ai", "claude:chat", build_vertex_ai_url)

    # Model Fetcher: Vertex 走专用模型获取链路
    UpstreamModelsFetcherRegistry.register(
        provider_types=["vertex_ai"],
        fetcher=fetch_models_vertex_ai,
    )

    # Provider Format Capability：跨格式支持（同一 Vertex AI Provider 可同时访问 Gemini 和 Claude 模型）
    register_provider_behavior_variant("vertex_ai", cross_format=True)


__all__ = ["fetch_models_vertex_ai", "register_all"]
