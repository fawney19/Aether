"""
URL 处理工具函数

唯一职责：给定 base_url 和 path，按"边界斜杠 + 去空白"的最小规则拼成一个
URL。不做版本号嗅探、不做路径去重、不改大小写。上层调用方自行决定 path 来源
（custom_path 或 api_format 的 default_path）。
"""

from __future__ import annotations

from urllib.parse import urlparse


def join_url(base_url: str | None, path: str | None) -> str:
    """拼接 base_url 和 path。

    规则：
    1. 去前后空白（strip）
    2. base_url 末尾的 "/" 全部去掉
    3. path 若非空则保证以 "/" 开头
    4. 直接拼接，不做任何路径段去重或版本嗅探

    参数可为 None，被视作空字符串。
    """
    base = (base_url or "").strip().rstrip("/")
    p = (path or "").strip()
    if not p:
        return base
    if not p.startswith("/"):
        p = "/" + p
    return f"{base}{p}"


def is_official_openai_api_url(base_url: str | None) -> bool:
    """判断是否为 OpenAI 官方 API 端点。"""
    value = str(base_url or "").strip()
    if not value:
        return False

    parsed = urlparse(value if "://" in value else f"https://{value}")
    host = str(parsed.hostname or "").strip().lower()
    return host == "api.openai.com"


__all__ = ["join_url", "is_official_openai_api_url"]
