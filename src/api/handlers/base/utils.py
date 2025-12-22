"""
Handler 基础工具函数
"""

from typing import Any, Dict, Optional

from src.core.logger import logger


def extract_cache_creation_tokens(usage: Dict[str, Any]) -> int:
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

    优先使用嵌套格式，如果嵌套格式字段存在但值为 0，则智能 fallback 到旧格式。
    扁平格式和嵌套格式互斥，按顺序检查。

    Args:
        usage: API 响应中的 usage 字典

    Returns:
        缓存创建 tokens 总数
    """
    # 1. 检查嵌套格式（最新格式）
    cache_creation = usage.get("cache_creation")
    if isinstance(cache_creation, dict):
        cache_5m = int(cache_creation.get("ephemeral_5m_input_tokens", 0))
        cache_1h = int(cache_creation.get("ephemeral_1h_input_tokens", 0))
        total = cache_5m + cache_1h

        if total > 0:
            logger.debug(
                f"Using nested cache_creation: 5m={cache_5m}, 1h={cache_1h}, total={total}"
            )
            return total

        # 嵌套格式存在但为 0，fallback 到旧格式
        old_format = int(usage.get("cache_creation_input_tokens", 0))
        if old_format > 0:
            logger.debug(
                f"Nested cache_creation is 0, using old format: {old_format}"
            )
            return old_format

        # 都是 0，返回 0
        return 0

    # 2. 检查扁平新格式
    has_flat_format = (
        "claude_cache_creation_5_m_tokens" in usage
        or "claude_cache_creation_1_h_tokens" in usage
    )

    if has_flat_format:
        cache_5m = int(usage.get("claude_cache_creation_5_m_tokens", 0))
        cache_1h = int(usage.get("claude_cache_creation_1_h_tokens", 0))
        total = cache_5m + cache_1h

        if total > 0:
            logger.debug(
                f"Using flat new format: 5m={cache_5m}, 1h={cache_1h}, total={total}"
            )
            return total

        # 扁平格式存在但为 0，fallback 到旧格式
        old_format = int(usage.get("cache_creation_input_tokens", 0))
        if old_format > 0:
            logger.debug(
                f"Flat cache_creation is 0, using old format: {old_format}"
            )
            return old_format

        # 都是 0，返回 0
        return 0

    # 3. 回退到旧格式
    old_format = int(usage.get("cache_creation_input_tokens", 0))
    if old_format > 0:
        logger.debug(f"Using old format: cache_creation_input_tokens={old_format}")
    return old_format


def build_sse_headers(extra_headers: Optional[Dict[str, str]] = None) -> Dict[str, str]:
    """
    构建 SSE（text/event-stream）推荐响应头，用于减少代理缓冲带来的卡顿/成段输出。

    说明：
    - Cache-Control: no-transform 可避免部分代理对流做压缩/改写导致缓冲
    - X-Accel-Buffering: no 可显式提示 Nginx 关闭缓冲（即使全局已关闭也无害）
    """
    headers: Dict[str, str] = {
        "Cache-Control": "no-cache, no-transform",
        "X-Accel-Buffering": "no",
    }
    if extra_headers:
        headers.update(extra_headers)
    return headers
