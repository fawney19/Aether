"""
Handler 基础工具函数
"""

import json
from typing import Any, Dict, Optional

from src.core.exceptions import EmbeddedErrorException, ProviderNotAvailableException
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

    说明：
    - 只要检测到新格式字段（嵌套/扁平），即视为权威来源：哪怕值为 0 也不回退到旧字段。
    - 仅当新格式字段完全不存在时，才回退到旧字段。
    - 扁平格式和嵌套格式互斥，按顺序检查。

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
            f"Using nested cache_creation: 5m={cache_5m}, 1h={cache_1h}, total={total}"
        )
        return total

    # 2. 检查扁平新格式
    has_flat_format = (
        "claude_cache_creation_5_m_tokens" in usage
        or "claude_cache_creation_1_h_tokens" in usage
    )

    if has_flat_format:
        cache_5m = int(usage.get("claude_cache_creation_5_m_tokens", 0))
        cache_1h = int(usage.get("claude_cache_creation_1_h_tokens", 0))
        total = cache_5m + cache_1h

        logger.debug(
            f"Using flat new format: 5m={cache_5m}, 1h={cache_1h}, total={total}"
        )
        return total

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


def check_html_response(line: str) -> bool:
    """
    检查行是否为 HTML 响应（base_url 配置错误的常见症状）

    Args:
        line: 要检查的行内容

    Returns:
        True 如果检测到 HTML 响应
    """
    lower_line = line.lstrip().lower()
    return lower_line.startswith("<!doctype") or lower_line.startswith("<html")


def check_prefetched_response_error(
    prefetched_chunks: list,
    parser: Any,
    request_id: str,
    provider_name: str,
    endpoint_id: Optional[str],
    base_url: Optional[str],
) -> None:
    """
    检查预读的响应是否为非 SSE 格式的错误响应（HTML 或纯 JSON 错误）

    某些代理可能返回：
    1. HTML 页面（base_url 配置错误）
    2. 纯 JSON 错误（无换行或多行 JSON）

    Args:
        prefetched_chunks: 预读的字节块列表
        parser: 响应解析器（需要有 is_error_response 和 parse_response 方法）
        request_id: 请求 ID（用于日志）
        provider_name: Provider 名称
        endpoint_id: Endpoint ID
        base_url: Endpoint 的 base_url

    Raises:
        ProviderNotAvailableException: 如果检测到 HTML 响应
        EmbeddedErrorException: 如果检测到 JSON 错误响应
    """
    if not prefetched_chunks:
        return

    try:
        prefetched_bytes = b"".join(prefetched_chunks)
        stripped = prefetched_bytes.lstrip()

        # 去除 BOM
        if stripped.startswith(b"\xef\xbb\xbf"):
            stripped = stripped[3:]

        # HTML 响应（通常是 base_url 配置错误导致返回网页）
        lower_prefix = stripped[:32].lower()
        if lower_prefix.startswith(b"<!doctype") or lower_prefix.startswith(b"<html"):
            endpoint_short = endpoint_id[:8] + "..." if endpoint_id else "N/A"
            logger.error(
                f"  [{request_id}] 检测到 HTML 响应，可能是 base_url 配置错误: "
                f"Provider={provider_name}, Endpoint={endpoint_short}, "
                f"base_url={base_url}"
            )
            raise ProviderNotAvailableException(
                "上游服务返回了非预期的响应格式",
                provider_name=provider_name,
                upstream_status=200,
                upstream_response=stripped.decode("utf-8", errors="replace")[:500],
            )

        # 纯 JSON（可能无换行/多行 JSON）
        if stripped.startswith(b"{") or stripped.startswith(b"["):
            payload_str = stripped.decode("utf-8", errors="replace").strip()
            data = json.loads(payload_str)
            if isinstance(data, dict) and parser.is_error_response(data):
                parsed = parser.parse_response(data, 200)
                logger.warning(
                    f"  [{request_id}] 检测到 JSON 错误响应: "
                    f"Provider={provider_name}, "
                    f"error_type={parsed.error_type}, "
                    f"message={parsed.error_message}"
                )
                raise EmbeddedErrorException(
                    provider_name=provider_name,
                    error_code=(
                        int(parsed.error_type)
                        if parsed.error_type and parsed.error_type.isdigit()
                        else None
                    ),
                    error_message=parsed.error_message,
                    error_status=parsed.error_type,
                )
    except json.JSONDecodeError:
        pass
