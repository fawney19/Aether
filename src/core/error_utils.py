"""
错误消息处理工具函数
"""

from typing import Optional


def extract_error_message(error: Exception, status_code: Optional[int] = None) -> str:
    """
    从异常中提取错误消息，优先使用上游原始响应（用于链路追踪/调试）

    此函数用于 RequestCandidate 表的 error_message 字段，
    用于请求链路追踪中显示原始 Provider 响应。

    Args:
        error: 异常对象
        status_code: 可选的 HTTP 状态码，用于构建更详细的错误消息

    Returns:
        错误消息字符串（原始 Provider 响应）
    """
    # 优先使用 upstream_response 属性（包含上游 Provider 的原始错误，用于调试）
    upstream_response = getattr(error, "upstream_response", None)
    if upstream_response and isinstance(upstream_response, str) and upstream_response.strip():
        return str(upstream_response)

    # 回退到异常的字符串表示（str 可能为空，如 httpx 超时异常）
    error_str = str(error) or repr(error)
    if status_code is not None:
        return f"HTTP {status_code}: {error_str}"
    return error_str


def extract_client_error_message(error: Exception) -> str:
    """
    从异常中提取客户端友好的错误消息（用于返回给客户端/Usage 记录）

    此函数用于 Usage 表的 error_message 字段，
    用于显示给最终用户的友好错误消息。

    Args:
        error: 异常对象

    Returns:
        友好的错误消息字符串
    """
    # 优先使用 message 属性（已经是友好处理过的消息）
    message = getattr(error, "message", None)
    if message and isinstance(message, str) and message.strip():
        return message

    # 回退到异常的字符串表示
    return str(error) or repr(error)
