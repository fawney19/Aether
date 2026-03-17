"""
Usage 相关的 token 解析工具。

该模块用于从不同上游的 usage 结构中提取缓存 token 信息（兼容多种字段命名）。
放在 core 层，便于 services/api 共用，避免跨层反向依赖。
"""

from __future__ import annotations

from typing import Any

from src.core.logger import logger


def extract_cache_creation_tokens(usage: dict[str, Any]) -> int:
    """
    提取缓存创建 tokens（兼容三种格式）

    根据 Anthropic API 文档，支持三种格式（按优先级）：

    1. **嵌套格式（优先级最高）**：
       usage.cache_creation.ephemeral_5m_input_tokens
       usage.cache_creation.ephemeral_1h_input_tokens

    2. **扁平新格式（优先级第二）**：
       usage.claude_cache_creation_5_m_tokens
       usage.claude_cache_creation_1_h_tokens

    3. **旧格式（优先级第三）**：
       usage.cache_creation_input_tokens

    说明：
    - 只要检测到新格式字段（嵌套/扁平），即视为权威来源：哪怕值为 0 也不回退到旧字段。
    - 仅当新格式字段完全不存在时，才回退到旧字段。

    Args:
        usage: API 响应中的 usage 字典

    Returns:
        缓存创建 tokens 总数
    """
    # 1. 检查嵌套格式（最新格式）
    cache_creation = usage.get("cache_creation")
    has_nested_format = isinstance(cache_creation, dict) and (
        "ephemeral_5m_input_tokens" in cache_creation
        or "ephemeral_1h_input_tokens" in cache_creation
    )

    if has_nested_format:
        cache_5m = int(cache_creation.get("ephemeral_5m_input_tokens", 0))
        cache_1h = int(cache_creation.get("ephemeral_1h_input_tokens", 0))
        total = cache_5m + cache_1h

        logger.debug(
            "Using nested cache_creation: 5m={}, 1h={}, total={}",
            cache_5m,
            cache_1h,
            total,
        )
        return total

    # 2. 检查扁平新格式
    has_flat_format = (
        "claude_cache_creation_5_m_tokens" in usage or "claude_cache_creation_1_h_tokens" in usage
    )

    if has_flat_format:
        cache_5m = int(usage.get("claude_cache_creation_5_m_tokens", 0))
        cache_1h = int(usage.get("claude_cache_creation_1_h_tokens", 0))
        total = cache_5m + cache_1h

        logger.debug("Using flat new format: 5m={}, 1h={}, total={}", cache_5m, cache_1h, total)
        return total

    # 3. 回退到旧格式
    old_format = int(usage.get("cache_creation_input_tokens", 0))
    if old_format > 0:
        logger.debug("Using old format: cache_creation_input_tokens={}", old_format)
    return old_format


def extract_cache_creation_tokens_detail(usage: dict[str, Any]) -> tuple[int, int, int]:
    """
    提取缓存创建 tokens 细分（区分 5m 和 1h）

    返回 (total, tokens_5m, tokens_1h) 三元组。
    当无法区分时，tokens_5m 和 tokens_1h 均为 0，total 为合计值。
    """
    # 1. 嵌套格式
    cache_creation = usage.get("cache_creation")
    if isinstance(cache_creation, dict) and (
        "ephemeral_5m_input_tokens" in cache_creation
        or "ephemeral_1h_input_tokens" in cache_creation
    ):
        t5m = int(cache_creation.get("ephemeral_5m_input_tokens", 0))
        t1h = int(cache_creation.get("ephemeral_1h_input_tokens", 0))
        return t5m + t1h, t5m, t1h

    # 2. 扁平新格式
    if "claude_cache_creation_5_m_tokens" in usage or "claude_cache_creation_1_h_tokens" in usage:
        t5m = int(usage.get("claude_cache_creation_5_m_tokens", 0))
        t1h = int(usage.get("claude_cache_creation_1_h_tokens", 0))
        return t5m + t1h, t5m, t1h

    # 3. 旧格式：无法区分
    old = int(usage.get("cache_creation_input_tokens", 0))
    return old, 0, 0


def extract_cache_read_tokens(usage: dict[str, Any]) -> int:
    """
    提取缓存读取 tokens（兼容多种 OpenAI / Claude / Gemini 字段命名）。

    优先级：
    1. 直接字段：cache_read_input_tokens / cache_read_tokens
    2. OpenAI Responses: input_tokens_details.cached_tokens
    3. OpenAI Chat: prompt_tokens_details.cached_tokens
    4. 通用回退：cached_tokens

    说明：
    - 只要检测到更高优先级字段存在，即便值为 0 也不继续回退，
      避免被较低优先级字段覆盖。
    """
    if "cache_read_input_tokens" in usage:
        return int(usage.get("cache_read_input_tokens", 0) or 0)

    if "cache_read_tokens" in usage:
        return int(usage.get("cache_read_tokens", 0) or 0)

    input_details = usage.get("input_tokens_details")
    if isinstance(input_details, dict) and "cached_tokens" in input_details:
        return int(input_details.get("cached_tokens", 0) or 0)

    prompt_details = usage.get("prompt_tokens_details")
    if isinstance(prompt_details, dict) and "cached_tokens" in prompt_details:
        return int(prompt_details.get("cached_tokens", 0) or 0)

    return int(usage.get("cached_tokens", 0) or 0)


__all__ = [
    "extract_cache_creation_tokens",
    "extract_cache_creation_tokens_detail",
    "extract_cache_read_tokens",
]
