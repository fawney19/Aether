"""
请求构建器 - 透传模式

透传模式 (Passthrough): CLI 和 Chat 等场景，原样转发请求体和头部
- 清理敏感头部：authorization, x-api-key, host, content-length 等
- 保留所有其他头部和请求体字段
- 适用于：Claude CLI、OpenAI CLI、Chat API 等场景

使用方式：
    builder = PassthroughRequestBuilder()
    payload, headers = builder.build(original_body, original_headers, endpoint, key)
"""

from __future__ import annotations

from abc import ABC, abstractmethod
from typing import Any, Dict, FrozenSet, Optional, Tuple

from src.core.crypto import crypto_service

# ==============================================================================
# 统一的头部配置常量
# ==============================================================================

# 敏感头部 - 透传时需要清理（黑名单）
# 这些头部要么包含认证信息，要么由代理层重新生成
SENSITIVE_HEADERS: FrozenSet[str] = frozenset(
    {
        "authorization",
        "x-api-key",
        "x-goog-api-key",  # Gemini API 认证头
        "host",
        "content-length",
        "transfer-encoding",
        "connection",
        # 不透传 accept-encoding，让 httpx 自己协商压缩格式
        # 避免客户端请求 brotli/zstd 但 httpx 不支持解压的问题
        "accept-encoding",
    }
)

# 代理痕迹/客户端来源相关头部
#
# 目标：尽量不让上游感知请求在到达上游前被代理/转发过（例如 X-Forwarded-*、Via、Envoy/CDN 相关头等）。
# 当 Provider 配置 strip_proxy_headers=True 时，这些头部会被过滤。
PROXY_TRACE_HEADERS: FrozenSet[str] = frozenset(
    {
        "x-real-ip",
        "forwarded",
        "via",
        "cf-connecting-ip",  # Cloudflare
        "true-client-ip",  # Cloudflare/Akamai
        "client-ip",
        "x-client-ip",
        "x-cluster-client-ip",
        "x-original-forwarded-for",
        "x-originating-ip",
        "proxy-client-ip",
        "wl-proxy-client-ip",  # WebLogic
        # 一些网关/环境变量风格的注入字段（非标准 HTTP Header，但在透传模式下也可能出现）
        "http_x_forwarded_for",
        "http_x_forwarded",
        "http_x_cluster_client_ip",
        "http_client_ip",
        "http_forwarded_for",
        "http_forwarded",
        "http_via",
        "remote_addr",
    }
)

# 前缀匹配：全量过滤常见转发/代理链路头，避免漏掉变体（如 x-forwarded-prefix / x-envoy-external-address）
PROXY_TRACE_PREFIXES: Tuple[str, ...] = (
    "x-forwarded-",
    "x-envoy-",
)


def _should_strip_proxy_trace_header(lower_name: str) -> bool:
    return lower_name in PROXY_TRACE_HEADERS or lower_name.startswith(PROXY_TRACE_PREFIXES)


# ==============================================================================
# 请求构建器
# ==============================================================================


class RequestBuilder(ABC):
    """请求构建器抽象基类"""

    @abstractmethod
    def build_payload(
        self,
        original_body: Dict[str, Any],
        *,
        mapped_model: Optional[str] = None,
        is_stream: bool = False,
    ) -> Dict[str, Any]:
        """构建请求体"""
        pass

    @abstractmethod
    def build_headers(
        self,
        original_headers: Dict[str, str],
        endpoint: Any,
        key: Any,
        *,
        extra_headers: Optional[Dict[str, str]] = None,
    ) -> Dict[str, str]:
        """构建请求头"""
        pass

    def build(
        self,
        original_body: Dict[str, Any],
        original_headers: Dict[str, str],
        endpoint: Any,
        key: Any,
        *,
        mapped_model: Optional[str] = None,
        is_stream: bool = False,
        extra_headers: Optional[Dict[str, str]] = None,
    ) -> Tuple[Dict[str, Any], Dict[str, str]]:
        """
        构建完整的请求（请求体 + 请求头）

        Returns:
            Tuple[payload, headers]
        """
        payload = self.build_payload(
            original_body,
            mapped_model=mapped_model,
            is_stream=is_stream,
        )
        headers = self.build_headers(
            original_headers,
            endpoint,
            key,
            extra_headers=extra_headers,
        )
        return payload, headers


class PassthroughRequestBuilder(RequestBuilder):
    """
    透传模式请求构建器

    适用于 CLI 等场景，尽量保持请求原样：
    - 请求体：直接复制，只修改必要字段（model, stream）
    - 请求头：清理敏感头部（黑名单），透传其他所有头部
    """

    def build_payload(
        self,
        original_body: Dict[str, Any],
        *,
        mapped_model: Optional[str] = None,  # noqa: ARG002 - 由 apply_mapped_model 处理
        is_stream: bool = False,  # noqa: ARG002 - 保留原始值，不自动添加
    ) -> Dict[str, Any]:
        """
        透传请求体 - 原样复制，不做任何修改

        透传模式下：
        - model: 由各 handler 的 apply_mapped_model 方法处理
        - stream: 保留客户端原始值（不同 API 处理方式不同）
        """
        return dict(original_body)

    def build_headers(
        self,
        original_headers: Dict[str, str],
        endpoint: Any,
        key: Any,
        *,
        extra_headers: Optional[Dict[str, str]] = None,
    ) -> Dict[str, str]:
        """
        透传请求头 - 清理敏感头部（黑名单），透传其他所有头部

        如果 Provider 配置了 strip_proxy_headers=True，则过滤代理痕迹相关头部，
        避免将转发/代理链路信息透传给上游供应商。
        """
        from src.core.api_format_metadata import get_auth_config, resolve_api_format

        headers: Dict[str, str] = {}

        # 检查 Provider 是否配置了过滤代理痕迹头部
        strip_proxy_headers = False
        provider = getattr(endpoint, "provider", None)
        if provider:
            provider_config = getattr(provider, "config", None)
            if isinstance(provider_config, dict):
                strip_proxy_headers = bool(provider_config.get("strip_proxy_headers", False))

        # 1. 根据 API 格式自动设置认证头
        decrypted_key = crypto_service.decrypt(key.api_key)
        api_format = getattr(endpoint, "api_format", None)
        resolved_format = resolve_api_format(api_format)
        auth_header, auth_type = (
            get_auth_config(resolved_format) if resolved_format else ("Authorization", "bearer")
        )

        if auth_type == "bearer":
            headers[auth_header] = f"Bearer {decrypted_key}"
        else:
            headers[auth_header] = decrypted_key

        # 2. 添加 endpoint 配置的额外头部
        if endpoint.headers:
            headers.update(endpoint.headers)

        # 3. 透传原始头部（排除敏感头部 - 黑名单模式）
        if original_headers:
            for name, value in original_headers.items():
                lower_name = name.lower()

                # 跳过敏感头部
                if lower_name in SENSITIVE_HEADERS:
                    continue

                # 如果启用了代理痕迹过滤，跳过相关头部
                if strip_proxy_headers and _should_strip_proxy_trace_header(lower_name):
                    continue

                headers[name] = value

        # 4. 添加额外头部
        if extra_headers:
            headers.update(extra_headers)

        # 5. 确保有 Content-Type
        if "Content-Type" not in headers and "content-type" not in headers:
            headers["Content-Type"] = "application/json"

        return headers


# ==============================================================================
# 便捷函数
# ==============================================================================


def build_passthrough_request(
    original_body: Dict[str, Any],
    original_headers: Dict[str, str],
    endpoint: Any,
    key: Any,
) -> Tuple[Dict[str, Any], Dict[str, str]]:
    """
    构建透传模式的请求

    纯透传：原样复制请求体，只处理请求头（认证等）。
    model mapping 和 stream 由调用方自行处理（不同 API 格式处理方式不同）。
    """
    builder = PassthroughRequestBuilder()
    return builder.build(
        original_body,
        original_headers,
        endpoint,
        key,
    )
