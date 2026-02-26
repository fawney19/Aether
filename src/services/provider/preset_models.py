"""预设模型管理模块

为不支持自动获取模型的反代提供商（如 Kiro、Codex）提供统一的预设模型管理。

使用方式：
1. 在 PRESET_MODELS 中定义各 provider_type 的预设模型列表
2. 在 plugin.py 中调用 create_preset_models_fetcher() 创建 fetcher 函数
3. 将 fetcher 注册到 UpstreamModelsFetcherRegistry
"""

from __future__ import annotations

from collections.abc import Awaitable, Callable
from typing import Any

# Fetcher 函数签名：与 UpstreamModelsFetcherRegistry 中的 _ModelsFetcher 保持一致
# (ctx, timeout_seconds) -> (models, errors, has_success, upstream_metadata)
ModelsFetcherFunc = Callable[
    [Any, float],
    Awaitable[tuple[list[dict], list[str], bool, dict[str, Any] | None]],
]

# ---------------------------------------------------------------------------
# 预设模型定义
# ---------------------------------------------------------------------------
# 各 provider_type 对应的预设模型列表
# 格式: provider_type -> list of model dicts

PRESET_MODELS: dict[str, list[dict[str, Any]]] = {
    # Kiro (Claude CLI 反代)
    "kiro": [
        {
            "id": "claude-sonnet-4.5",
            "object": "model",
            "owned_by": "anthropic",
            "display_name": "Claude Sonnet 4.5",
        },
        {
            "id": "claude-sonnet-4.6",
            "object": "model",
            "owned_by": "anthropic",
            "display_name": "Claude Sonnet 4.6",
        },
        {
            "id": "claude-opus-4.5",
            "object": "model",
            "owned_by": "anthropic",
            "display_name": "Claude Opus 4.5",
        },
        {
            "id": "claude-opus-4.6",
            "object": "model",
            "owned_by": "anthropic",
            "display_name": "Claude Opus 4.6",
        },
        {
            "id": "claude-haiku-4.5",
            "object": "model",
            "owned_by": "anthropic",
            "display_name": "Claude Haiku 4.5",
        },
    ],
    # Claude Code (Claude CLI OAuth 反代)
    "claude_code": [
        {
            "id": "claude-opus-4-5-20251101",
            "object": "model",
            "owned_by": "anthropic",
            "display_name": "Claude Opus 4.5",
        },
        {
            "id": "claude-opus-4-6",
            "object": "model",
            "owned_by": "anthropic",
            "display_name": "Claude Opus 4.6",
        },
        {
            "id": "claude-sonnet-4-6",
            "object": "model",
            "owned_by": "anthropic",
            "display_name": "Claude Sonnet 4.6",
        },
        {
            "id": "claude-sonnet-4-5-20250929",
            "object": "model",
            "owned_by": "anthropic",
            "display_name": "Claude Sonnet 4.5",
        },
        {
            "id": "claude-haiku-4-5-20251001",
            "object": "model",
            "owned_by": "anthropic",
            "display_name": "Claude Haiku 4.5",
        },
    ],
    # Codex (OpenAI CLI 反代)
    "codex": [
        {
            "id": "gpt-5",
            "object": "model",
            "owned_by": "openai",
            "display_name": "GPT-5",
        },
        {
            "id": "gpt-5-codex",
            "object": "model",
            "owned_by": "openai",
            "display_name": "GPT-5 Codex",
        },
        {
            "id": "gpt-5-codex-mini",
            "object": "model",
            "owned_by": "openai",
            "display_name": "GPT-5 Codex Mini",
        },
        {
            "id": "gpt-5.1",
            "object": "model",
            "owned_by": "openai",
            "display_name": "GPT-5.1",
        },
        {
            "id": "gpt-5.1-codex",
            "object": "model",
            "owned_by": "openai",
            "display_name": "GPT-5.1 Codex",
        },
        {
            "id": "gpt-5.1-codex-mini",
            "object": "model",
            "owned_by": "openai",
            "display_name": "GPT-5.1 Codex Mini",
        },
        {
            "id": "gpt-5.1-codex-max",
            "object": "model",
            "owned_by": "openai",
            "display_name": "GPT-5.1 Codex Max",
        },
        {
            "id": "gpt-5.2",
            "object": "model",
            "owned_by": "openai",
            "display_name": "GPT-5.2",
        },
        {
            "id": "gpt-5.2-codex",
            "object": "model",
            "owned_by": "openai",
            "display_name": "GPT-5.2 Codex",
        },
        {
            "id": "gpt-5.3-codex",
            "object": "model",
            "owned_by": "openai",
            "display_name": "GPT-5.3 Codex",
        },
    ],
}


# ---------------------------------------------------------------------------
# Fetcher 工厂函数
# ---------------------------------------------------------------------------


def get_preset_models(provider_type: str) -> list[dict[str, Any]]:
    """获取指定 provider_type 的预设模型列表。"""
    return list(PRESET_MODELS.get(provider_type.lower(), []))


def create_preset_models_fetcher(
    provider_type: str,
) -> ModelsFetcherFunc:
    """创建一个返回预设模型列表的 fetcher 函数。

    Args:
        provider_type: 提供商类型（如 "kiro", "codex"）

    Returns:
        符合 UpstreamModelsFetcherRegistry 签名的 async fetcher 函数
    """
    models = get_preset_models(provider_type)

    async def fetch_preset_models(
        _ctx: Any,
        _timeout_seconds: float,
    ) -> tuple[list[dict], list[str], bool, dict[str, Any] | None]:
        """Return preset model catalog.

        This provider does not expose a /v1/models endpoint, so we skip the
        HTTP call entirely and return a hardcoded list.
        """
        return list(models), [], True, None

    return fetch_preset_models


__all__ = [
    "ModelsFetcherFunc",
    "PRESET_MODELS",
    "create_preset_models_fetcher",
    "get_preset_models",
]
