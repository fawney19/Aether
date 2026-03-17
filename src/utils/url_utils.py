"""
URL 处理工具函数

提供 URL 模式检测和处理功能。
"""

from __future__ import annotations

from urllib.parse import urlparse


def is_official_openai_api_url(base_url: str | None) -> bool:
    """判断是否为 OpenAI 官方 API 端点。"""
    value = str(base_url or "").strip()
    if not value:
        return False

    parsed = urlparse(value if "://" in value else f"https://{value}")
    host = str(parsed.hostname or "").strip().lower()
    return host == "api.openai.com"


def is_codex_url(base_url: str) -> bool:
    """判断是否是 Codex OAuth 端点。

    Codex OAuth 端点（如 chatgpt.com/backend-api/codex）不走标准 /v1 前缀，
    直接使用 /responses 而非 /v1/responses。

    Args:
        base_url: 端点基础 URL

    Returns:
        bool: 是否是 Codex 端点
    """
    url = base_url.rstrip("/")
    return "/backend-api/codex" in url or url.endswith("/codex")
