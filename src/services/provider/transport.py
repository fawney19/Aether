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
from urllib.parse import urlencode, urlparse

from src.core.api_format import (
    EndpointKind,
    get_default_path_for_endpoint,
    make_signature_key,
)
from src.core.logger import logger
from src.core.provider_types import ProviderType, normalize_provider_type
from src.services.provider.format import normalize_endpoint_signature
from src.services.provider.provider_context import resolve_provider_type
from src.services.provider.request_context import (
    get_selected_base_url,
    set_selected_base_url,
)
from src.utils.url_utils import join_url

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


# Vertex AI host 白名单：aiplatform.googleapis.com 及其区域子域名
_VERTEX_AI_HOST = "aiplatform.googleapis.com"
_VERTEX_AI_ENDPOINT_SIGS = {"gemini:chat", "claude:chat"}
_VERTEX_AI_AUTH_TYPES = {"api_key", "service_account", "vertex_ai"}


def looks_like_vertex_ai_host(
    base_url: str,
    endpoint_sig: str = "",
    auth_type: str = "",
) -> bool:
    """根据 base_url + endpoint_sig/auth_type 判断是否属于 Vertex AI。

    用于历史数据兼容：部分旧记录缺少 provider_type，但 base_url 已固定到 aiplatform。
    """
    try:
        host = (urlparse(base_url).netloc or "").strip().lower()
    except Exception:
        return False
    if not host:
        return False
    if host != _VERTEX_AI_HOST and not host.endswith(f".{_VERTEX_AI_HOST}"):
        return False
    sig = endpoint_sig.strip().lower()
    at = auth_type.strip().lower()
    return sig in _VERTEX_AI_ENDPOINT_SIGS or at in _VERTEX_AI_AUTH_TYPES


def _normalize_base_url(base_url: str, path: str) -> str:
    """规范化 base_url：仅 strip 前后空白并去除末尾 "/"。

    不再做版本前缀嗅探——由调用方保证 base_url 和 path 的语义边界
    （base_url 只含协议+域名+反代前缀，path 含完整业务路径）。
    path 参数保留以兼容旧调用签名，但在此不再使用。
    """
    _ = path
    return (base_url or "").strip().rstrip("/")


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
    1. endpoint.provider_type（如果调用方已做扁平化注入）
    2. key.provider.provider_type
    3. decrypted_auth_config["provider_type"]（OAuth 导入的凭证）
    4. provider_id 对应的 Provider 记录
    """
    resolved = resolve_provider_type(
        endpoint=endpoint,
        key=key,
        decrypted_auth_config=decrypted_auth_config,
    )
    if resolved:
        return resolved

    # Fallback: 历史 Vertex 数据可能缺少 provider_type，但 base_url 已固定到 aiplatform。
    try:
        base_url = str(getattr(endpoint, "base_url", "") or "").strip()
        endpoint_sig = str(getattr(endpoint, "api_format", "") or "").strip()
        auth_type = str(getattr(key, "auth_type", "") or "").strip() if key else ""
        if base_url and looks_like_vertex_ai_host(base_url, endpoint_sig, auth_type):
            return ProviderType.VERTEX_AI.value
    except Exception:
        pass

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

    ensure_providers_bootstrapped(provider_types=[provider_type] if provider_type else None)
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
        # 仅按 provider_type 判定，不再嗅探 URL
        if endpoint_sig in {"openai:cli", "openai:compact"} and provider_type == ProviderType.CODEX:
            path = "/responses/compact" if endpoint_sig == "openai:compact" else "/responses"
        if effective_path_params:
            try:
                path = path.format(**effective_path_params)
            except KeyError:
                # 如果模板变量不匹配，保持原路径
                pass

    # 统一走 join_url：仅处理边界斜杠，不做版本嗅探
    # base_url 在数据库中是 NOT NULL，类型标注为 Optional 是 SQLAlchemy 限制
    url = join_url(endpoint.base_url, path)  # type: ignore[arg-type]

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
