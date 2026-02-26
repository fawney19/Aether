"""
SSL 工具函数
提供统一的 SSL 上下文创建功能
"""

import ssl

from loguru import logger


def _create_default_ssl_context() -> ssl.SSLContext:
    try:
        import certifi

        return ssl.create_default_context(cafile=certifi.where())
    except ImportError:
        return ssl.create_default_context()


try:
    _SSL_CONTEXT = _create_default_ssl_context()
except Exception:
    _SSL_CONTEXT = ssl.create_default_context()

_PROXY_SSL_CONTEXT: ssl.SSLContext | None = None
_PROFILE_SSL_CONTEXTS: dict[str, ssl.SSLContext] = {}


def get_ssl_context() -> ssl.SSLContext:
    """
    获取 SSL 上下文

    优先使用 certifi 证书包，如果未安装则使用系统默认证书。
    返回模块级缓存的 SSL 上下文实例。

    Returns:
        ssl.SSLContext: SSL 上下文
    """
    return _SSL_CONTEXT


def get_proxy_ssl_context(expected_fingerprint: str | None = None) -> ssl.SSLContext:
    """
    获取用于代理连接的 SSL 上下文（连接 aether-proxy TLS 端口）

    当前使用 CERT_NONE（不验证证书），因为 aether-proxy 使用自签名证书。
    expected_fingerprint 参数预留供未来实现指纹校验。

    Args:
        expected_fingerprint: 预期的证书 SHA-256 指纹（hex，预留参数）

    Returns:
        ssl.SSLContext: 代理专用 SSL 上下文
    """
    global _PROXY_SSL_CONTEXT

    if expected_fingerprint:
        logger.warning("TLS 证书指纹校验尚未实现, fingerprint={} 被忽略", expected_fingerprint)

    if _PROXY_SSL_CONTEXT is None:
        ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
        ctx.check_hostname = False
        ctx.verify_mode = ssl.CERT_NONE
        _PROXY_SSL_CONTEXT = ctx
    # TODO: 实现基于 expected_fingerprint 的证书指纹校验
    return _PROXY_SSL_CONTEXT


def _build_claude_code_ssl_context() -> ssl.SSLContext:
    """构建 Claude Code best-effort TLS 配置。

    说明：Python/OpenSSL 无法完整模拟 Node.js ClientHello。
    这里仅做可控项的尽力对齐（ALPN/TLS 版本/常见 cipher 偏好）。
    """
    ctx = _create_default_ssl_context()
    ctx.minimum_version = ssl.TLSVersion.TLSv1_2
    try:
        ctx.maximum_version = ssl.TLSVersion.TLSv1_3
    except Exception:
        pass
    try:
        ctx.set_alpn_protocols(["h2", "http/1.1"])
    except Exception:
        pass
    try:
        ctx.set_ciphers(
            "ECDHE-ECDSA-AES128-GCM-SHA256:"
            "ECDHE-RSA-AES128-GCM-SHA256:"
            "ECDHE-ECDSA-AES256-GCM-SHA384:"
            "ECDHE-RSA-AES256-GCM-SHA384:"
            "ECDHE-ECDSA-CHACHA20-POLY1305:"
            "ECDHE-RSA-CHACHA20-POLY1305"
        )
    except Exception:
        pass
    return ctx


def get_ssl_context_for_profile(tls_profile: str | None = None) -> ssl.SSLContext:
    """按 profile 返回 SSL 上下文。"""
    profile = str(tls_profile or "").strip().lower()
    if not profile:
        return get_ssl_context()

    if profile in _PROFILE_SSL_CONTEXTS:
        logger.debug("复用 TLS profile SSL context: {}", profile)
        return _PROFILE_SSL_CONTEXTS[profile]

    if profile == "claude_code_nodejs":
        logger.info("启用 TLS profile: {}（best-effort）", profile)
        ctx = _build_claude_code_ssl_context()
    else:
        logger.warning("未知 TLS profile: {}，回退默认 SSL context", profile)
        ctx = get_ssl_context()

    _PROFILE_SSL_CONTEXTS[profile] = ctx
    return ctx
