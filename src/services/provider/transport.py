"""
统一的 Provider 请求构建工具。

负责:
- 根据 API 格式或端点配置生成请求 URL
- URL 脱敏（用于日志记录）
- Vertex AI URL 自动构建
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
    1. Vertex AI 自动构建 - 当 key.auth_type == "vertex_ai" 时
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

    # 检查是否为 Vertex AI 认证类型
    auth_type = getattr(key, "auth_type", "api_key") if key else "api_key"
    if auth_type == "vertex_ai":
        return _build_vertex_ai_url(
            key=key,
            path_params=path_params,
            query_params=query_params,
            is_stream=is_stream,
            decrypted_auth_config=decrypted_auth_config,
        )

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


# ==============================================================================
# Vertex AI 配置
# ==============================================================================

# Vertex AI 模型前缀到 API 格式的映射
# 用于 auth_type=vertex_ai 时，根据模型名动态确定实际的请求/响应格式
# 格式：前缀 -> endpoint signature（family:kind）
VERTEX_AI_MODEL_FORMAT_MAPPING: dict[str, str] = {
    "claude-": "claude:chat",  # Anthropic Claude 模型
    "gemini-": "gemini:chat",  # Google Gemini 模型
    "imagen-": "gemini:chat",  # Google Imagen 模型（使用 Gemini chat 格式）
}

# Vertex AI 默认 endpoint signature（当模型前缀不匹配时）
VERTEX_AI_DEFAULT_FORMAT: str = "gemini:chat"


def get_vertex_ai_effective_format(
    model: str,
    auth_config: dict[str, Any] | None = None,
) -> str:
    """
    获取 Vertex AI 模式下模型的实际 API 格式

    优先级：
    1. auth_config.model_format_mapping 中的精确匹配
    2. auth_config.model_format_mapping 中的前缀匹配
    3. 内置 VERTEX_AI_MODEL_FORMAT_MAPPING 前缀匹配
    4. auth_config.default_format
    5. 内置 VERTEX_AI_DEFAULT_FORMAT

    auth_config 配置示例::

        {
            "project_id": "your-gcp-project-id",
            "model_format_mapping": {
                "claude-": "CLAUDE",           # 前缀匹配
                "my-custom-model": "OPENAI"    # 精确匹配
            },
            "default_format": "GEMINI"
        }

    Args:
        model: 模型名称
        auth_config: 解密后的认证配置（可选），可包含 model_format_mapping 和 default_format

    Returns:
        实际应使用的 endpoint signature（如 "claude:chat", "gemini:chat"）
    """
    # 用户配置的模型-格式映射
    user_format_mapping: dict[str, str] = {}
    user_default_format: str | None = None

    if auth_config:
        user_format_mapping = auth_config.get("model_format_mapping", {})
        user_default_format = auth_config.get("default_format")

    # 1. 用户配置：精确匹配
    if model in user_format_mapping:
        try:
            return normalize_endpoint_signature(user_format_mapping[model])
        except Exception:
            logger.warning(
                "Invalid vertex_ai model_format_mapping value for model '{}': {!r}",
                model,
                user_format_mapping[model],
            )

    # 2. 用户配置：前缀匹配
    for prefix, api_format in user_format_mapping.items():
        if prefix.endswith("-") and model.startswith(prefix):
            try:
                return normalize_endpoint_signature(api_format)
            except Exception:
                logger.warning(
                    "Invalid vertex_ai model_format_mapping value for prefix '{}': {!r}",
                    prefix,
                    api_format,
                )
                break

    # 3. 内置配置：前缀匹配
    for prefix, api_format in VERTEX_AI_MODEL_FORMAT_MAPPING.items():
        if model.startswith(prefix):
            return normalize_endpoint_signature(api_format)

    # 4. 用户默认格式
    if user_default_format:
        try:
            return normalize_endpoint_signature(user_default_format)
        except Exception:
            logger.warning("Invalid vertex_ai default_format: {!r}", user_default_format)

    # 5. 内置默认格式
    return VERTEX_AI_DEFAULT_FORMAT


# Vertex AI 模型默认 region 映射
# 用户可以通过 auth_config.model_regions 覆盖
VERTEX_AI_DEFAULT_MODEL_REGIONS: dict[str, str] = {
    # Gemini 3 系列（使用 global）
    "gemini-3-pro-image-preview": "global",
    # Gemini 2.0 系列
    "gemini-2.0-flash": "us-central1",
    "gemini-2.0-flash-exp": "us-central1",
    "gemini-2.0-flash-001": "us-central1",
    "gemini-2.0-pro-exp": "us-central1",
    "gemini-2.0-flash-exp-image-generation": "us-central1",
    # Gemini 1.5 系列
    "gemini-1.5-pro": "us-central1",
    "gemini-1.5-pro-001": "us-central1",
    "gemini-1.5-pro-002": "us-central1",
    "gemini-1.5-flash": "us-central1",
    "gemini-1.5-flash-001": "us-central1",
    "gemini-1.5-flash-002": "us-central1",
    # Imagen 系列
    "imagen-3.0-generate-001": "us-central1",
    "imagen-3.0-fast-generate-001": "us-central1",
}


def _build_vertex_ai_url(
    key: "ProviderAPIKey",
    *,
    path_params: dict[str, Any] | None = None,
    query_params: dict[str, Any] | None = None,
    is_stream: bool = False,
    decrypted_auth_config: dict[str, Any] | None = None,
) -> str:
    """
    构建 Vertex AI URL

    Vertex AI URL 格式:
    - Gemini: https://{region}-aiplatform.googleapis.com/v1/projects/{project_id}/locations/{region}/publishers/google/models/{model}:{action}
    - Claude: https://{region}-aiplatform.googleapis.com/v1/projects/{project_id}/locations/{region}/publishers/anthropic/models/{model}:{action}

    从 auth_config 中读取:
    - project_id: GCP 项目 ID（必需）
    - region: 默认 GCP 区域（覆盖内置默认值）
    - model_regions: 模型到区域的映射（可选），覆盖内置和默认配置

    Region 优先级:
    1. auth_config.model_regions[model] - 用户为该模型指定的区域
    2. VERTEX_AI_DEFAULT_MODEL_REGIONS[model] - 内置的模型默认区域
    3. auth_config.region - 用户配置的默认区域
    4. global - 最终兜底

    Args:
        key: Provider API Key（包含 auth_config）
        path_params: 路径参数（需要 model）
        query_params: 查询参数
        is_stream: 是否为流式请求
        decrypted_auth_config: 已解密的认证配置（由 get_provider_auth 提供，避免重复解密）

    Returns:
        完整的 Vertex AI URL
    """
    from src.core.crypto import crypto_service
    from src.utils import json_helper as json

    # 优先使用传入的已解密配置，避免重复解密
    auth_config: dict[str, Any] = {}
    if decrypted_auth_config:
        auth_config = decrypted_auth_config
    else:
        # 兜底：从 key.auth_config 解密（理论上不应走到这里）
        raw_auth_config = getattr(key, "auth_config", None)
        if raw_auth_config:
            try:
                # auth_config 可能是加密字符串或未加密的 dict
                if isinstance(raw_auth_config, dict):
                    auth_config = raw_auth_config
                else:
                    decrypted_config = crypto_service.decrypt(raw_auth_config)
                    auth_config = json.loads(decrypted_config)
            except Exception as e:
                logger.error(f"解密 Vertex AI auth_config 失败: {e}")
                auth_config = {}

    from src.core.exceptions import InvalidRequestException

    # 获取必需的配置
    project_id = auth_config.get("project_id")
    if not project_id:
        raise InvalidRequestException(
            "Vertex AI 配置缺少 project_id（请在 Key 的 auth_config 中提供）"
        )

    # 获取模型名
    model = (path_params or {}).get("model", "")
    if not model:
        raise InvalidRequestException("Vertex AI 请求缺少 model 参数")

    # 确定 region（优先级：用户配置 > 内置默认 > 用户默认 > 兜底）
    user_model_regions = auth_config.get("model_regions", {})
    user_default_region = auth_config.get("region")

    if model in user_model_regions:
        region = user_model_regions[model]
    elif model in VERTEX_AI_DEFAULT_MODEL_REGIONS:
        region = VERTEX_AI_DEFAULT_MODEL_REGIONS[model]
    elif user_default_region:
        region = user_default_region
    else:
        region = "global"

    # 判断是 Claude 还是 Gemini 模型
    is_claude_model = model.startswith("claude-")

    # 根据模型类型确定 publisher 和 action
    if is_claude_model:
        # Claude 模型使用 Anthropic publisher
        publisher = "anthropic"
        action = "streamRawPredict" if is_stream else "rawPredict"
    else:
        # Gemini 模型使用 Google publisher
        publisher = "google"
        action = "streamGenerateContent" if is_stream else "generateContent"

    # 构建 URL（global region 使用不同的 URL 格式）
    if region == "global":
        base_url = "https://aiplatform.googleapis.com"
    else:
        base_url = f"https://{region}-aiplatform.googleapis.com"
    path = f"/v1/projects/{project_id}/locations/{region}/publishers/{publisher}/models/{model}:{action}"
    url = f"{base_url}{path}"

    # 添加查询参数
    effective_query_params = dict(query_params) if query_params else {}
    # Gemini 流式请求使用 SSE 格式，Claude 不需要
    if is_stream and not is_claude_model:
        effective_query_params.setdefault("alt", "sse")
    # 移除不适用于 Vertex AI 的参数
    effective_query_params.pop("beta", None)

    if effective_query_params:
        query_string = urlencode(effective_query_params, doseq=True)
        if query_string:
            url = f"{url}?{query_string}"

    logger.debug(f"Vertex AI URL: {redact_url_for_log(url)} (region={region})")
    return url
