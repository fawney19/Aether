"""
统一的 Provider 请求构建工具。

负责:
- 根据 API 格式或端点配置生成请求 URL
- URL 脱敏（用于日志记录）
- Provider transport hook 路由
"""

from __future__ import annotations

import re
from typing import TYPE_CHECKING, Any, Callable
from urllib.parse import urlencode

from src.core.api_format import (
    EndpointKind,
    get_default_path_for_endpoint,
    make_signature_key,
)
from src.core.logger import logger
from src.core.provider_types import ProviderType, normalize_provider_type
from src.services.provider.format import normalize_endpoint_signature
from src.services.provider.request_context import (
    get_selected_base_url,
    set_selected_base_url,
)
from src.utils.url_utils import is_codex_url

if TYPE_CHECKING:
    from src.models.database import ProviderAPIKey, ProviderEndpoint

# ---------------------------------------------------------------------------
# Transport Hook Registry
# ---------------------------------------------------------------------------
# key: (provider_type, endpoint_sig)
# value: Callable(endpoint, *, is_stream, effective_query_params) -> str
_TransportHookFn = Callable[..., str]
_transport_hooks: dict[tuple[str, str], _TransportHookFn] = {}


def register_transport_hook(
    provider_type: str,
    endpoint_sig: str,
    hook: _TransportHookFn,
) -> None:
    """注册 provider 特有的 URL 构建 hook。"""
    pt = normalize_provider_type(provider_type)
    sig = str(endpoint_sig or "").strip().lower()
    _transport_hooks[(pt, sig)] = hook


# URL 中需要脱敏的查询参数（正则模式）
_SENSITIVE_QUERY_PARAMS_PATTERN = re.compile(
    r"([?&])(key|api_key|apikey|token|secret|password|credential)=([^&]*)",
    re.IGNORECASE,
)


def redact_url_for_log(url: str) -> str:
    """
    对 URL 中的敏感查询参数进行脱敏，用于日志记录

    将 ?key=xxx 替换为 ?key=***

    Args:
        url: 原始 URL

    Returns:
        脱敏后的 URL
    """
    return _SENSITIVE_QUERY_PARAMS_PATTERN.sub(r"\1\2=***", url)


def _normalize_base_url(base_url: str, path: str) -> str:
    """
    规范化 base_url，去除末尾的斜杠和可能与 path 重复的版本前缀。

    只有当 path 以版本前缀开头时，才从 base_url 中移除该前缀，
    避免拼接出 /v1/v1/messages 这样的重复路径。

    兼容用户填写的各种格式：
    - https://api.example.com
    - https://api.example.com/
    - https://api.example.com/v1
    - https://api.example.com/v1/
    """
    base = base_url.rstrip("/")
    # 只在 path 以版本前缀开头时才去除 base_url 中的该前缀
    # 例如：base="/v1", path="/v1/messages" -> 去除 /v1
    # 例如：base="/v1", path="/chat/completions" -> 不去除（用户可能期望保留）
    for suffix in ("/v1beta", "/v1", "/v2", "/v3"):
        if base.endswith(suffix) and path.startswith(suffix):
            base = base[: -len(suffix)]
            break
    return base


def get_antigravity_base_url() -> str | None:
    """Backward-compat alias for `get_selected_base_url()`."""
    return get_selected_base_url()


def _get_provider_type(
    endpoint: Any,
    key: "ProviderAPIKey" | None = None,
    decrypted_auth_config: dict[str, Any] | None = None,
) -> str | None:
    """尽力获取 Provider.provider_type（用于 Antigravity 等 Provider 特判）。

    优先级:
    1. endpoint.provider.provider_type
    2. key.provider.provider_type
    3. decrypted_auth_config["provider_type"]（OAuth 导入的凭证）
    """
    try:
        provider = getattr(endpoint, "provider", None)
        if provider is not None:
            pt = getattr(provider, "provider_type", None)
            if pt:
                return str(pt).lower()
    except Exception:
        pass

    try:
        if key is not None:
            provider = getattr(key, "provider", None)
            if provider is not None:
                pt = getattr(provider, "provider_type", None)
                if pt:
                    return str(pt).lower()
    except Exception:
        pass

    # Fallback: OAuth 导入的凭证可能包含 provider_type（如 Kiro）
    if decrypted_auth_config:
        pt = decrypted_auth_config.get("provider_type")
        if isinstance(pt, str) and pt.strip():
            return pt.strip().lower()

    return None


def build_provider_url(
    endpoint: ProviderEndpoint,
    *,
    query_params: dict[str, Any] | None = None,
    path_params: dict[str, Any] | None = None,
    is_stream: bool = False,
    key: "ProviderAPIKey" | None = None,
    decrypted_auth_config: dict[str, Any] | None = None,
) -> str:
    """
    根据 endpoint 配置生成请求 URL

    优先级：
    1. Provider transport hook - 如有注册的 hook 则委托处理
    2. endpoint.custom_path - 自定义路径（支持模板变量如 {model}）
    3. API 格式默认路径 - 根据 api_format 自动选择

    Args:
        endpoint: 端点配置
        query_params: 查询参数
        path_params: 路径模板参数 (如 {model})
        is_stream: 是否为流式请求，用于 Gemini API 选择正确的操作方法
        key: Provider API Key（用于 Vertex AI 等需要从密钥配置读取信息的场景）
        decrypted_auth_config: 已解密的认证配置（避免重复解密，由 get_provider_auth 提供）
    """
    # 默认清理，避免上一次请求的 selected_base_url 泄漏到其他请求
    set_selected_base_url(None)

    # endpoint signature（新模式）
    raw_family = getattr(endpoint, "api_family", None)
    raw_kind = getattr(endpoint, "endpoint_kind", None)
    endpoint_sig = ""
    if isinstance(raw_family, str) and isinstance(raw_kind, str) and raw_family and raw_kind:
        endpoint_sig = make_signature_key(raw_family, raw_kind)
    else:
        # 兜底：允许 api_format 已直接存 signature key 的情况
        raw_format = getattr(endpoint, "api_format", None)
        if isinstance(raw_format, str) and ":" in raw_format:
            endpoint_sig = raw_format

    # endpoint_sig 为空时保持为空（更安全：默认路径回退到 "/"，避免误判为 claude:chat）
    endpoint_sig = normalize_endpoint_signature(endpoint_sig) if endpoint_sig else ""

    provider_type = _get_provider_type(endpoint, key, decrypted_auth_config)

    # 合并查询参数（部分逻辑需要先拿到 query_params）
    effective_query_params = dict(query_params) if query_params else {}

    # Gemini family 下清除可能存在的 key 参数（避免客户端传入的认证信息泄露到上游）
    # 上游认证始终使用 header 方式，不使用 URL 参数
    if endpoint_sig.startswith("gemini:"):
        effective_query_params.pop("key", None)

    # Provider transport hook: 如果有注册的 hook 则委托处理
    from src.services.provider.envelope import ensure_providers_bootstrapped

    ensure_providers_bootstrapped()
    if provider_type and endpoint_sig:
        hook = _transport_hooks.get((provider_type, endpoint_sig))
        # Codex hook 仅在无 custom_path 时生效
        if hook and not (provider_type == ProviderType.CODEX and endpoint.custom_path):
            return hook(
                endpoint,
                is_stream=is_stream,
                effective_query_params=effective_query_params,
                path_params=path_params,
                key=key,
                decrypted_auth_config=decrypted_auth_config,
            )

    # 非 hook 路径：清除 contextvar，避免跨请求污染
    set_selected_base_url(None)

    # 准备路径参数（Gemini chat/cli 需要 action）
    effective_path_params = dict(path_params) if path_params else {}
    if endpoint_sig.startswith("gemini:"):
        try:
            kind = EndpointKind(endpoint_sig.split(":", 1)[1])
        except Exception:
            kind = None
        if kind in {EndpointKind.CHAT, EndpointKind.CLI} and "action" not in effective_path_params:
            effective_path_params["action"] = (
                "streamGenerateContent" if is_stream else "generateContent"
            )

    # 优先使用 custom_path 字段
    if endpoint.custom_path:
        path = endpoint.custom_path
        if effective_path_params:
            try:
                path = path.format(**effective_path_params)
            except KeyError:
                # 如果模板变量不匹配，保持原路径
                pass
    else:
        # 使用 API 格式的默认路径
        path = _resolve_default_path(endpoint_sig)
        # Codex OAuth 端点（chatgpt.com/backend-api/codex）使用 /responses 而非 /v1/responses
        base_url = getattr(endpoint, "base_url", "") or ""
        if endpoint_sig == "openai:cli" and is_codex_url(base_url):
            path = "/responses"
        if effective_path_params:
            try:
                path = path.format(**effective_path_params)
            except KeyError:
                # 如果模板变量不匹配，保持原路径
                pass

    if not path.startswith("/"):
        path = f"/{path}"

    # 先确定 path，再根据 path 规范化 base_url
    # base_url 在数据库中是 NOT NULL，类型标注为 Optional 是 SQLAlchemy 限制
    base = _normalize_base_url(endpoint.base_url, path)  # type: ignore[arg-type]
    url = f"{base}{path}"

    # Gemini streamGenerateContent 官方支持 `?alt=sse` 返回 SSE（data: {...}）。
    # 网关侧统一使用 SSE 输出，优先向上游请求 SSE 以减少解析分支；同时保留 JSON-array 兜底解析。
    if endpoint_sig.startswith("gemini:") and is_stream:
        effective_query_params.setdefault("alt", "sse")

    # 添加查询参数
    if effective_query_params:
        query_string = urlencode(effective_query_params, doseq=True)
        if query_string:
            url = f"{url}?{query_string}"

    return url


def _resolve_default_path(endpoint_sig: str | None) -> str:
    """根据 endpoint signature 返回默认路径。"""
    try:
        return get_default_path_for_endpoint(endpoint_sig or "")
    except Exception:
        logger.warning(f"Unknown endpoint signature '{endpoint_sig}' for endpoint, fallback to '/'")
        return "/"
