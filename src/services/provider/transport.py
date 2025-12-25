"""
统一的 Provider 请求构建工具。

负责:
- 根据 endpoint/key 构建标准请求头
- 根据 API 格式或端点配置生成请求 URL
"""

from typing import TYPE_CHECKING, Any, Dict, Optional
from urllib.parse import urlencode

from src.core.api_format_metadata import get_auth_config, get_default_path, resolve_api_format
from src.core.crypto import crypto_service
from src.core.enums import APIFormat
from src.core.logger import logger

if TYPE_CHECKING:
    from src.models.database import ProviderAPIKey, ProviderEndpoint



def build_provider_headers(
    endpoint: "ProviderEndpoint",
    key: "ProviderAPIKey",
    original_headers: Optional[Dict[str, str]] = None,
    *,
    extra_headers: Optional[Dict[str, str]] = None,
) -> Dict[str, str]:
    """
    根据 endpoint/key 构建请求头，并透传客户端自定义头。

    支持高级 headers 规则处理（从 endpoint.headers 读取）：
    - add: 新增固定的参数和值
    - remove: 删除指定的参数
    - replace_name: 替换参数名
    - replace_value: 替换参数值
    """
    headers: Dict[str, str] = {}

    # api_key 在数据库中是 NOT NULL，类型标注为 Optional 是 SQLAlchemy 限制
    decrypted_key = crypto_service.decrypt(key.api_key)  # type: ignore[arg-type]

    # 根据 API 格式自动选择认证头
    api_format = getattr(endpoint, "api_format", None)
    resolved_format = resolve_api_format(api_format)
    auth_header, auth_type = (
        get_auth_config(resolved_format) if resolved_format else ("Authorization", "bearer")
    )

    if auth_type == "bearer":
        headers[auth_header] = f"Bearer {decrypted_key}"
    else:
        headers[auth_header] = decrypted_key

    # 处理 endpoint.headers（可能是规则或直接的 headers）
    if endpoint.headers:
        from src.core.header_rules import apply_header_rules

        # 检查是否是新格式的规则（包含规则键）
        rule_keys = {"add", "remove", "replace_name", "replace_value"}
        if isinstance(endpoint.headers, dict) and any(
            key in endpoint.headers for key in rule_keys
        ):
            # 新格式：应用规则
            headers = apply_header_rules(headers, endpoint.headers)
        else:
            # 旧格式：直接合并 headers
            headers.update(endpoint.headers)

    excluded_headers = {
        "host",
        "authorization",
        "x-api-key",
        "x-goog-api-key",
        "content-length",
        "transfer-encoding",
    }

    if original_headers:
        for name, value in original_headers.items():
            if name.lower() not in excluded_headers:
                headers[name] = value

    if extra_headers:
        headers.update(extra_headers)

    if "Content-Type" not in headers and "content-type" not in headers:
        headers["Content-Type"] = "application/json"

    return headers


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


def build_provider_url(
    endpoint: "ProviderEndpoint",
    *,
    query_params: Optional[Dict[str, Any]] = None,
    path_params: Optional[Dict[str, Any]] = None,
    is_stream: bool = False,
) -> str:
    """
    根据 endpoint 配置生成请求 URL

    优先级：
    1. endpoint.custom_path - 自定义路径（支持模板变量如 {model}）
    2. API 格式默认路径 - 根据 api_format 自动选择

    Args:
        endpoint: 端点配置
        query_params: 查询参数
        path_params: 路径模板参数 (如 {model})
        is_stream: 是否为流式请求，用于 Gemini API 选择正确的操作方法
    """
    # 准备路径参数，添加 Gemini API 所需的 action 参数
    effective_path_params = dict(path_params) if path_params else {}

    # 为 Gemini API 格式自动添加 action 参数
    resolved_format = resolve_api_format(endpoint.api_format)
    if resolved_format in (APIFormat.GEMINI, APIFormat.GEMINI_CLI):
        if "action" not in effective_path_params:
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
        path = _resolve_default_path(endpoint.api_format)
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

    # 添加查询参数
    if query_params:
        query_string = urlencode(query_params, doseq=True)
        if query_string:
            url = f"{url}?{query_string}"

    return url


def _resolve_default_path(api_format: Optional[str]) -> str:
    """
    根据 API 格式返回默认路径
    """
    resolved = resolve_api_format(api_format)
    if resolved:
        return get_default_path(resolved)

    logger.warning(f"Unknown api_format '{api_format}' for endpoint, fallback to '/'")
    return "/"
