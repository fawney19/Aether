"""
API 格式检测

提供从请求头、响应内容等检测 API 格式的函数。
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from starlette.requests import Request

from src.core.api_format.enums import ApiFamily, AuthMethod, EndpointKind, EndpointType
from src.core.api_format.signature import EndpointSignature, make_signature_key


@dataclass(frozen=True)
class RequestContext:
    """请求上下文 - 三维度信息"""

    endpoint: EndpointSignature
    endpoint_type: EndpointType
    auth_method: AuthMethod
    credentials: str | None


def _detect_endpoint_type(path: str) -> EndpointType:
    normalized = path.lower()

    if normalized.startswith("/upload/v1beta/files") or normalized.startswith("/v1beta/files"):
        return EndpointType.FILES
    if normalized.startswith("/v1/videos") or (
        normalized.startswith("/v1beta/") and "predictlongrunning" in normalized
    ):
        return EndpointType.VIDEO
    # Gemini operations (视频轮询) 也归类为 VIDEO
    if normalized.startswith("/v1beta/operations"):
        return EndpointType.VIDEO
    if normalized.startswith("/v1/models"):
        return EndpointType.MODELS
    if "/embeddings" in normalized:
        return EndpointType.EMBEDDING
    if "/images" in normalized:
        return EndpointType.IMAGE
    if "/audio" in normalized:
        return EndpointType.AUDIO
    return EndpointType.CHAT


def _detect_data_format(
    path: str, headers: dict[str, str], query_params: dict[str, str] | None
) -> EndpointSignature:
    normalized = path.lower()
    endpoint_type = _detect_endpoint_type(path)

    # Claude: /v1/messages（chat/cli 共用路径，按认证头区分）
    if normalized.startswith("/v1/messages"):
        auth_header = headers.get("authorization", "")
        if auth_header.lower().startswith("bearer "):
            return EndpointSignature(api_family=ApiFamily.CLAUDE, endpoint_kind=EndpointKind.CLI)
        return EndpointSignature(api_family=ApiFamily.CLAUDE, endpoint_kind=EndpointKind.CHAT)

    # OpenAI compact: /responses/compact
    if "/responses/compact" in normalized:
        return EndpointSignature(api_family=ApiFamily.OPENAI, endpoint_kind=EndpointKind.COMPACT)

    # OpenAI CLI: /responses
    if "/responses" in normalized:
        return EndpointSignature(api_family=ApiFamily.OPENAI, endpoint_kind=EndpointKind.CLI)

    # Gemini family
    if normalized.startswith("/v1beta/") or normalized.startswith("/upload/v1beta/"):
        kind = EndpointKind.CHAT
        if endpoint_type == EndpointType.VIDEO:
            kind = EndpointKind.VIDEO
        return EndpointSignature(api_family=ApiFamily.GEMINI, endpoint_kind=kind)

    # OpenAI family
    if normalized.startswith("/v1/videos"):
        return EndpointSignature(api_family=ApiFamily.OPENAI, endpoint_kind=EndpointKind.VIDEO)
    if normalized.startswith("/v1/images"):
        return EndpointSignature(api_family=ApiFamily.OPENAI, endpoint_kind=EndpointKind.IMAGE)
    if normalized.startswith("/v1/chat/completions"):
        return EndpointSignature(api_family=ApiFamily.OPENAI, endpoint_kind=EndpointKind.CHAT)

    # Fallback: 基于认证方式猜测协议族（主要用于 /v1/models）
    sig, _api_key, _auth_source = detect_format_from_request(headers, query_params)
    return sig


def _detect_auth_method(
    headers: dict[str, str], query_params: dict[str, str] | None
) -> tuple[AuthMethod, str | None]:
    # Query key (Gemini) has highest priority
    query_key = query_params.get("key") if query_params else None
    if query_key:
        return AuthMethod.QUERY_KEY, query_key

    x_goog_key = headers.get("x-goog-api-key")
    if x_goog_key:
        return AuthMethod.GOOG_API_KEY, x_goog_key

    x_api_key = headers.get("x-api-key")
    if x_api_key:
        return AuthMethod.API_KEY, x_api_key

    auth_header = headers.get("authorization", "")
    if auth_header.lower().startswith("bearer "):
        return AuthMethod.BEARER, auth_header[7:].strip()

    return AuthMethod.BEARER, None


def detect_format_from_request(
    headers: dict[str, str],
    query_params: dict[str, str] | None = None,
) -> tuple[EndpointSignature, str | None, str]:
    """
    从请求头检测 API 格式和 API Key

    检测优先级：
    1. x-api-key + anthropic-version -> Claude
    2. x-goog-api-key 或 ?key= -> Gemini
    3. Authorization: Bearer -> OpenAI (默认)

    Args:
        headers: 请求头字典（key 应为小写）
        query_params: 查询参数字典（可选）

    Returns:
        (endpoint_signature, api_key, auth_source) 元组
        - endpoint_signature: EndpointSignature(api_family, endpoint_kind)
        - auth_source: 认证来源 ("header" 或 "query")
    """
    # Claude: x-api-key + anthropic-version (必须同时存在)
    if headers.get("x-api-key") and headers.get("anthropic-version"):
        return (
            EndpointSignature(api_family=ApiFamily.CLAUDE, endpoint_kind=EndpointKind.CHAT),
            headers.get("x-api-key"),
            "header",
        )

    # Gemini: query 参数优先（与 Google SDK 行为一致）
    query_key = query_params.get("key") if query_params else None
    if query_key:
        return (
            EndpointSignature(api_family=ApiFamily.GEMINI, endpoint_kind=EndpointKind.CHAT),
            query_key,
            "query",
        )
    x_goog_key = headers.get("x-goog-api-key")
    if x_goog_key:
        return (
            EndpointSignature(api_family=ApiFamily.GEMINI, endpoint_kind=EndpointKind.CHAT),
            x_goog_key,
            "header",
        )

    # OpenAI: Authorization: Bearer (默认)
    auth_header = headers.get("authorization", "")
    if auth_header.lower().startswith("bearer "):
        return (
            EndpointSignature(api_family=ApiFamily.OPENAI, endpoint_kind=EndpointKind.CHAT),
            auth_header[7:].strip(),
            "header",
        )

    # 兜底：兼容部分客户端用 x-api-key 携带 OpenAI token 的情况
    if headers.get("x-api-key"):
        return (
            EndpointSignature(api_family=ApiFamily.OPENAI, endpoint_kind=EndpointKind.CHAT),
            headers.get("x-api-key"),
            "header",
        )

    return (
        EndpointSignature(api_family=ApiFamily.OPENAI, endpoint_kind=EndpointKind.CHAT),
        None,
        "header",
    )


def detect_format_and_key_from_starlette(
    request: Request,
) -> tuple[str, str | None, str]:
    """
    从 Starlette Request 对象检测 API 格式和 API Key

    这是一个便捷函数，用于直接处理 Starlette/FastAPI 请求对象。

    Args:
        request: Starlette Request 对象

    Returns:
        (format_name, api_key, auth_method) 元组
        - format_name: 为小写字符串
        - auth_method: 认证方式 ("header" 或 "query")
    """
    # 规范化 headers 为小写
    headers = {k.lower(): v for k, v in request.headers.items()}
    query_params = dict(request.query_params)

    api_format, api_key, auth_method = detect_format_from_request(headers, query_params)

    # 返回小写格式名
    return api_format.key, api_key, auth_method


def detect_request_context(request: Request) -> RequestContext:
    """
    从 Request 中检测三维度信息

    Returns:
        RequestContext(data_format, endpoint_type, auth_method, credentials)
    """
    headers = {k.lower(): v for k, v in request.headers.items()}
    query_params = dict(request.query_params)

    endpoint_type = _detect_endpoint_type(request.url.path)
    data_format = _detect_data_format(request.url.path, headers, query_params)
    auth_method, credentials = _detect_auth_method(headers, query_params)

    return RequestContext(
        endpoint=data_format,
        endpoint_type=endpoint_type,
        auth_method=auth_method,
        credentials=credentials,
    )


def detect_format_from_response(
    response_data: dict,
) -> str | None:
    """
    从响应内容检测 API 格式

    Args:
        response_data: 响应 JSON 字典

    Returns:
        检测到的格式，或 None
    """
    # Claude: 有 type="message" 或特定的 content 结构
    if response_data.get("type") == "message":
        return make_signature_key(ApiFamily.CLAUDE, EndpointKind.CHAT)
    if "content" in response_data and isinstance(response_data["content"], list):
        first_content = response_data["content"][0] if response_data["content"] else {}
        if first_content.get("type") in ("text", "tool_use"):
            return make_signature_key(ApiFamily.CLAUDE, EndpointKind.CHAT)

    # OpenAI: 有 choices 数组
    if "choices" in response_data:
        return make_signature_key(ApiFamily.OPENAI, EndpointKind.CHAT)

    # Gemini: 有 candidates 数组
    if "candidates" in response_data:
        return make_signature_key(ApiFamily.GEMINI, EndpointKind.CHAT)

    return None


def detect_cli_format_from_path(
    path: str,
    base_signature: str,
) -> bool:
    """
    根据请求路径检测是否为 CLI 模式

    CLI 模式的特征：
    - OpenAI CLI: 请求 /responses 路径
    - Claude CLI: 有特定的路径模式
    - Gemini CLI: 有特定的路径模式

    Args:
        path: 请求路径
        base_format: 基础格式

    Returns:
        True 如果是 CLI 模式
    """
    # OpenAI CLI 特征: /v1/responses 路径
    if str(base_signature).lower().startswith("openai:") and "/responses" in path.lower():
        return True

    # 其他 CLI 模式通常由 Adapter 层根据具体业务逻辑判断
    return False


__all__ = [
    "detect_format_from_request",
    "detect_format_and_key_from_starlette",
    "detect_format_from_response",
    "detect_cli_format_from_path",
    "detect_request_context",
    "RequestContext",
]
