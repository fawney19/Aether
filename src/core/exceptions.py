"""
统一的异常处理和错误响应定义

安全说明:
- 生产环境不返回详细错误信息，避免信息泄露
- 使用错误 ID 关联日志，便于排查问题
- 开发环境可返回详细信息用于调试
"""

import asyncio
import re
import traceback
import uuid
from typing import Any, Dict, List, Optional

import httpx
from fastapi import HTTPException, status
from fastapi.responses import JSONResponse

from ..config import config
from src.core.logger import logger


# Pydantic 错误消息中英文翻译映射
PYDANTIC_ERROR_TRANSLATIONS = {
    # 字符串验证
    r"String should have at least (\d+) characters?": r"字符串长度至少需要 \1 个字符",
    r"String should have at most (\d+) characters?": r"字符串长度最多 \1 个字符",
    r"string_too_short": "字符串长度不足",
    r"string_too_long": "字符串长度超出限制",
    # 必填字段
    r"Field required": "此字段为必填项",
    r"field required": "此字段为必填项",
    r"missing": "缺少必填字段",
    # 类型错误
    r"Input should be a valid string": "输入应为有效的字符串",
    r"Input should be a valid integer": "输入应为有效的整数",
    r"Input should be a valid number": "输入应为有效的数字",
    r"Input should be a valid boolean": "输入应为布尔值",
    r"Input should be a valid email address": "输入应为有效的邮箱地址",
    r"Input should be a valid list": "输入应为有效的列表",
    r"Input should be a valid dictionary": "输入应为有效的字典",
    # 数值验证
    r"Input should be greater than (\d+)": r"数值应大于 \1",
    r"Input should be greater than or equal to (\d+)": r"数值应大于或等于 \1",
    r"Input should be less than (\d+)": r"数值应小于 \1",
    r"Input should be less than or equal to (\d+)": r"数值应小于或等于 \1",
    # 枚举验证
    r"Input should be (.+)": r"输入应为 \1",
    # 其他
    r"value is not a valid email address": "邮箱地址格式无效",
    r"invalid.*email": "邮箱地址格式无效",
    r"Extra inputs are not permitted": "不允许额外的字段",
    r"Value error, (.+)": r"\1",  # 自定义验证器的错误直接使用
}

# 字段名中英文翻译映射
FIELD_NAME_TRANSLATIONS = {
    "password": "密码",
    "username": "用户名",
    "email": "邮箱",
    "role": "角色",
    "quota_usd": "配额",
    "name": "名称",
    "title": "标题",
    "content": "内容",
    "ip_address": "IP地址",
    "reason": "原因",
    "ttl": "过期时间",
    "enabled": "启用状态",
    "fixed_limit": "固定限制",
    "old_password": "旧密码",
    "new_password": "新密码",
    "allowed_providers": "允许的提供商",
    "allowed_models": "允许的模型",
    "rate_limit": "速率限制",
    "expire_days": "过期天数",
    "priority": "优先级",
    "type": "类型",
    "is_active": "激活状态",
    "is_pinned": "置顶状态",
    "start_time": "开始时间",
    "end_time": "结束时间",
}


def translate_pydantic_error(error: Dict[str, Any]) -> str:
    """
    将 Pydantic 验证错误翻译为中文

    Args:
        error: Pydantic 错误字典，包含 loc, msg, type 等字段

    Returns:
        翻译后的中文错误消息
    """
    # 获取字段名
    loc = error.get("loc", [])
    field = str(loc[0]) if loc else ""
    field_zh = FIELD_NAME_TRANSLATIONS.get(field, field)

    # 获取错误消息
    msg = error.get("msg", "验证失败")

    # 尝试翻译错误消息
    translated_msg = msg
    for pattern, replacement in PYDANTIC_ERROR_TRANSLATIONS.items():
        if re.search(pattern, msg, re.IGNORECASE):
            translated_msg = re.sub(pattern, replacement, msg, flags=re.IGNORECASE)
            break

    # 组合字段名和错误消息
    if field_zh:
        return f"{field_zh}: {translated_msg}"
    return translated_msg


def translate_pydantic_errors(errors: List[Dict[str, Any]]) -> str:
    """
    翻译多个 Pydantic 验证错误

    Args:
        errors: Pydantic 错误列表

    Returns:
        翻译后的错误消息，多个错误用分号分隔
    """
    if not errors:
        return "请求数据验证失败"

    translated = [translate_pydantic_error(e) for e in errors]
    return "; ".join(translated)



# 延迟导入韧性管理器，避免循环导入
def get_resilience_manager():
    try:
        from ..core.resilience import resilience_manager

        return resilience_manager
    except ImportError:
        return None


class ProxyException(HTTPException):
    """代理服务基础异常"""

    def __init__(
        self,
        status_code: int,
        error_type: str,
        message: str,
        details: Optional[Dict[str, Any]] = None,
    ):
        self.error_type = error_type
        self.message = message
        self.details = details or {}
        super().__init__(status_code=status_code, detail=message)


class ProviderException(ProxyException):
    """提供商相关异常"""

    def __init__(
        self,
        message: str,
        provider_name: Optional[str] = None,
        request_metadata: Optional[Any] = None,
        **kwargs,
    ):
        self.request_metadata = request_metadata  # 保存元数据以便传递
        details = {"provider": provider_name} if provider_name else {}
        details.update(kwargs)
        super().__init__(
            status_code=status.HTTP_503_SERVICE_UNAVAILABLE,
            error_type="provider_error",
            message=message,
            details=details,
        )


class ProviderNotAvailableException(ProviderException):
    """提供商不可用"""

    def __init__(
        self,
        message: str,
        provider_name: Optional[str] = None,
        request_metadata: Optional[Any] = None,
        upstream_status: Optional[int] = None,
        upstream_response: Optional[str] = None,
    ):
        super().__init__(
            message=message,
            provider_name=provider_name,
            request_metadata=request_metadata,
        )
        self.upstream_status = upstream_status
        self.upstream_response = upstream_response


class ProviderTimeoutException(ProviderException):
    """提供商请求超时"""

    def __init__(self, provider_name: str, timeout: int, request_metadata: Optional[Any] = None):
        super().__init__(
            message=f"请求超时（{timeout}秒）",
            provider_name=provider_name,
            request_metadata=request_metadata,
            timeout=timeout,
        )


class ProviderAuthException(ProviderException):
    """提供商认证失败"""

    def __init__(self, provider_name: str, request_metadata: Optional[Any] = None):
        super().__init__(
            message="上游服务认证失败",
            provider_name=provider_name,
            request_metadata=request_metadata,
        )


class ProviderRateLimitException(ProviderException):
    """提供商限流"""

    def __init__(
        self,
        message: str,
        provider_name: Optional[str] = None,
        request_metadata: Optional[Any] = None,
        response_headers: Optional[Dict[str, str]] = None,  # 添加响应头
        retry_after: Optional[int] = None,  # 添加重试时间
    ):
        self.response_headers = response_headers or {}  # 保存响应头
        self.retry_after = retry_after  # 保存重试时间
        super().__init__(
            message=message, provider_name=provider_name, request_metadata=request_metadata
        )


class QuotaExceededException(ProxyException):
    """配额超限"""

    def __init__(self, quota_type: str = "tokens", remaining: Optional[float] = None):
        message = f"{quota_type}配额已用尽"
        if remaining is not None:
            message += f"（剩余: {remaining}）"
        super().__init__(
            status_code=status.HTTP_429_TOO_MANY_REQUESTS,
            error_type="quota_exceeded",
            message=message,
            details={"quota_type": quota_type, "remaining": remaining},
        )


class RateLimitException(ProxyException):
    """速率限制"""

    def __init__(self, limit: int, window: str = "minute"):
        super().__init__(
            status_code=status.HTTP_429_TOO_MANY_REQUESTS,
            error_type="rate_limit",
            message=f"请求过于频繁，限制为每{window} {limit}次",
            details={"limit": limit, "window": window},
        )


class ConcurrencyLimitError(ProxyException):
    """并发限制异常"""

    def __init__(
        self, message: str, endpoint_id: Optional[str] = None, key_id: Optional[str] = None
    ):
        details = {}
        if endpoint_id:
            details["endpoint_id"] = endpoint_id
        if key_id:
            details["key_id"] = key_id

        super().__init__(
            status_code=status.HTTP_429_TOO_MANY_REQUESTS,
            error_type="concurrency_limit",
            message=message,
            details=details,
        )


class ModelNotSupportedException(ProxyException):
    """模型不支持"""

    def __init__(self, model: str, provider_name: Optional[str] = None):
        # 客户端消息不暴露提供商信息
        message = f"模型 '{model}' 不受支持"
        super().__init__(
            status_code=status.HTTP_400_BAD_REQUEST,
            error_type="model_not_supported",
            message=message,
            details={"model": model, "provider": provider_name},
        )


class StreamingNotSupportedException(ProxyException):
    """流式请求不支持"""

    def __init__(self, model: str, provider_name: Optional[str] = None):
        # 客户端消息不暴露提供商信息
        message = f"模型 '{model}' 不支持流式请求"
        super().__init__(
            status_code=status.HTTP_400_BAD_REQUEST,
            error_type="streaming_not_supported",
            message=message,
            details={"model": model, "provider": provider_name},
        )


class InvalidRequestException(ProxyException):
    """无效请求"""

    def __init__(self, message: str, field: Optional[str] = None):
        super().__init__(
            status_code=status.HTTP_400_BAD_REQUEST,
            error_type="invalid_request",
            message=message,
            details={"field": field} if field else {},
        )


class NotFoundException(ProxyException):
    """资源未找到"""

    def __init__(self, message: str, resource_type: Optional[str] = None):
        super().__init__(
            status_code=status.HTTP_404_NOT_FOUND,
            error_type="not_found",
            message=message,
            details={"resource_type": resource_type} if resource_type else {},
        )


class ForbiddenException(ProxyException):
    """权限不足"""

    def __init__(self, message: str, required_role: Optional[str] = None):
        super().__init__(
            status_code=status.HTTP_403_FORBIDDEN,
            error_type="forbidden",
            message=message,
            details={"required_role": required_role} if required_role else {},
        )


class DecryptionException(ProxyException):
    """解密失败异常 - 已知的配置问题，不需要打印堆栈"""

    def __init__(self, message: str, details: Optional[Dict[str, Any]] = None):
        super().__init__(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            error_type="decryption_error",
            message=message,
            details=details or {},
        )


class JSONParseException(ProviderException):
    """JSON解析错误"""

    def __init__(
        self,
        provider_name: str,
        original_error: str,
        response_content: Optional[str] = None,
        content_type: Optional[str] = None,
        request_metadata: Optional[Any] = None,
    ):
        details = {
            "original_error": original_error,
            "content_type": content_type,
        }
        if response_content and len(response_content) > 500:
            # 截断长内容，但保留头尾
            details["response_preview"] = f"{response_content[:200]}...{response_content[-200:]}"
        elif response_content:
            details["response_content"] = response_content

        super().__init__(
            message="上游服务返回了无效的响应",
            provider_name=provider_name,
            request_metadata=request_metadata,
            **details,
        )


class EmptyStreamException(ProviderException):
    """流式响应为空异常 - 上游返回200但没有发送任何数据"""

    def __init__(
        self,
        provider_name: str,
        chunk_count: int = 0,
        request_metadata: Optional[Any] = None,
    ):
        super().__init__(
            message="上游服务返回了空的流式响应",
            provider_name=provider_name,
            request_metadata=request_metadata,
            chunk_count=chunk_count,
        )


class EmbeddedErrorException(ProviderException):
    """响应体内嵌套错误异常 - HTTP 状态码正常但响应体包含错误信息

    用于处理某些 Provider（如 Gemini）返回 HTTP 200 但在响应体中包含错误的情况。
    这类错误需要触发重试逻辑。
    """

    def __init__(
        self,
        provider_name: str,
        error_code: Optional[int] = None,
        error_message: Optional[str] = None,
        error_status: Optional[str] = None,
        request_metadata: Optional[Any] = None,
    ):
        # 客户端消息不暴露提供商信息
        message = "上游服务返回了错误"
        if error_code:
            message += f" (code={error_code})"

        super().__init__(
            message=message,
            provider_name=provider_name,
            request_metadata=request_metadata,
            error_code=error_code,
            error_status=error_status,
        )
        self.error_code = error_code
        self.error_message = error_message
        self.error_status = error_status


class ProviderCompatibilityException(ProviderException):
    """Provider 兼容性错误异常 - 应该触发故障转移

    用于处理因 Provider 不支持某些参数或功能导致的错误。
    这类错误不是用户请求本身的问题，换一个 Provider 可能就能成功，应该触发故障转移。

    常见场景：
    - Unsupported parameter（不支持的参数）
    - Unsupported model（不支持的模型）
    - Unsupported feature（不支持的功能）
    """

    def __init__(
        self,
        message: str,
        provider_name: Optional[str] = None,
        status_code: int = 400,
        upstream_error: Optional[str] = None,
        request_metadata: Optional[Any] = None,
    ):
        self.upstream_error = upstream_error
        super().__init__(
            message=message,
            provider_name=provider_name,
            request_metadata=request_metadata,
        )
        # 覆盖状态码为 400（保持与上游一致）
        self.status_code = status_code


class UpstreamClientException(ProxyException):
    """上游返回的客户端错误异常 - HTTP 4xx 错误，不应该重试

    用于处理上游 Provider 返回的客户端错误（如图片处理失败、无效请求等）。
    这类错误是由用户请求本身的问题导致的，换 Provider 也无济于事，不应该重试。

    常见场景：
    - 图片处理失败（图片过大、格式不支持等）
    - 请求参数无效
    - 消息内容违规
    """

    def __init__(
        self,
        message: str,
        provider_name: Optional[str] = None,
        status_code: int = 400,
        error_type: Optional[str] = None,
        upstream_error: Optional[str] = None,
        request_metadata: Optional[Any] = None,
    ):
        self.upstream_error = upstream_error
        self.request_metadata = request_metadata
        details = {}
        if provider_name:
            details["provider"] = provider_name
        if error_type:
            details["upstream_error_type"] = error_type
        if upstream_error:
            details["upstream_error"] = upstream_error

        super().__init__(
            status_code=status_code,
            error_type="upstream_client_error",
            message=message,
            details=details,
        )


class ErrorResponse:
    """统一的错误响应格式化器"""

    @staticmethod
    def create(
        error_type: str,
        message: str,
        status_code: int = 500,
        details: Optional[Dict[str, Any]] = None,
    ) -> JSONResponse:
        """创建标准错误响应"""
        error_body = {"error": {"type": error_type, "message": message}}

        if details:
            error_body["error"]["details"] = details

        # 记录错误日志
        logger.error(f"Error response: {error_type} - {message}")

        return JSONResponse(status_code=status_code, content=error_body)

    @staticmethod
    def from_exception(e: Exception) -> JSONResponse:
        """
        从异常创建错误响应

        安全说明:
        - 生产环境只返回错误 ID，不暴露详细信息
        - 开发环境返回完整错误信息用于调试
        - 所有错误都记录到日志，通过错误 ID 关联
        """
        if isinstance(e, ProxyException):
            details = e.details.copy() if e.details else {}
            status_code = e.status_code
            message = e.message  # 使用友好的错误消息
            # 如果是 ProviderNotAvailableException 且有上游错误信息
            if isinstance(e, ProviderNotAvailableException):
                if e.upstream_status:
                    status_code = e.upstream_status
                # upstream_response 存入 details 供请求链路追踪使用，不作为客户端消息
                if e.upstream_response:
                    details["upstream_response"] = e.upstream_response
            return ErrorResponse.create(
                error_type=e.error_type,
                message=message,
                status_code=status_code,
                details=details if details else None,
            )
        elif isinstance(e, HTTPException):
            return ErrorResponse.create(
                error_type="http_error", message=str(e.detail), status_code=e.status_code
            )
        else:
            # 未知异常，使用错误 ID 机制
            error_id = str(uuid.uuid4())[:8]  # 短 ID，便于用户报告
            error_type_name = type(e).__name__
            error_message = str(e)

            # 始终记录完整错误到日志
            logger.error(f"[{error_id}] Unexpected error: {error_type_name}: {error_message}")

            # 根据环境决定返回的详细程度
            is_development = config.environment in ("development", "test", "testing")

            if is_development:
                # 开发环境：返回完整错误信息
                return ErrorResponse.create(
                    error_type="internal_error",
                    message=f"内部服务器错误: {error_type_name}: {error_message}",
                    status_code=500,
                    details={
                        "error_id": error_id,
                        "error_type": error_type_name,
                        "error": error_message,
                        "traceback": traceback.format_exc().split("\n"),
                    },
                )
            else:
                # 生产环境：只返回错误 ID
                return ErrorResponse.create(
                    error_type="internal_error",
                    message="内部服务器错误",
                    status_code=500,
                    details={
                        "error_id": error_id,
                        "support_info": "请联系管理员并提供此错误 ID",
                    },
                )

    @staticmethod
    def provider_error(provider_name: str, error: Exception) -> JSONResponse:
        """提供商错误响应 - 基于异常类型判断"""
        # 基于异常类型判断，更可靠
        if isinstance(error, (asyncio.TimeoutError, httpx.TimeoutException)):
            return ErrorResponse.from_exception(ProviderTimeoutException(provider_name, 60))
        elif isinstance(error, (httpx.HTTPStatusError,)):
            if error.response.status_code == 401:
                return ErrorResponse.from_exception(ProviderAuthException(provider_name))
            elif error.response.status_code == 429:
                return ErrorResponse.from_exception(
                    ProviderRateLimitException(
                        message=f"提供商 '{provider_name}' 速率限制",
                        provider_name=provider_name,
                    )
                )
        elif isinstance(error, (httpx.ConnectError, httpx.NetworkError)):
            return ErrorResponse.create(
                error_type="provider_connection_error",
                message=f"无法连接到提供商 {provider_name}",
                status_code=503,
                details={"provider": provider_name, "error": "Connection failed"},
            )
        # 如果异常类型无法判断，再通过字符串匹配作为备用
        elif "auth" in str(error).lower() or "401" in str(error):
            return ErrorResponse.from_exception(ProviderAuthException(provider_name))
        elif "rate limit" in str(error).lower() or "429" in str(error):
            return ErrorResponse.from_exception(
                ProviderRateLimitException(
                    message=f"提供商 '{provider_name}' 速率限制",
                    provider_name=provider_name,
                )
            )
        else:
            return ErrorResponse.create(
                error_type="provider_error",
                message=f"提供商请求失败: {str(error)}",
                status_code=503,
                details={
                    "provider": provider_name,
                    "error": str(error),
                    "error_type": type(error).__name__,
                },
            )


class ExceptionHandlers:
    """FastAPI异常处理器"""

    @staticmethod
    async def handle_proxy_exception(request, exc: ProxyException):
        """处理代理异常"""
        return ErrorResponse.from_exception(exc)

    @staticmethod
    async def handle_http_exception(request, exc: HTTPException):
        """处理HTTP异常"""
        return ErrorResponse.from_exception(exc)

    @staticmethod
    async def handle_generic_exception(request, exc: Exception):
        """处理通用异常 - 集成韧性管理"""

        # 首先检查是否为HTTPException，如果是则委托给HTTP异常处理器
        if isinstance(exc, HTTPException):
            return await ExceptionHandlers.handle_http_exception(request, exc)

        # 获取请求信息用于上下文
        request_info = {
            "method": request.method,
            "url": str(request.url),
            "client_ip": (
                getattr(request.client, "host", "unknown")
                if hasattr(request, "client")
                else "unknown"
            ),
            "user_agent": request.headers.get("user-agent", "unknown"),
        }

        # 使用韧性管理器处理错误
        rm = get_resilience_manager()
        if rm:
            try:
                error_result = rm.handle_error(
                    error=exc,
                    context=request_info,
                    operation=f"{request.method} {request.url.path}",
                )

                # 根据错误处理结果返回适当的响应
                if error_result.get("severity") and error_result["severity"].value == "critical":
                    status_code = status.HTTP_503_SERVICE_UNAVAILABLE
                elif error_result.get("severity") and error_result["severity"].value == "high":
                    status_code = status.HTTP_500_INTERNAL_SERVER_ERROR
                else:
                    status_code = status.HTTP_500_INTERNAL_SERVER_ERROR

                return ErrorResponse.create(
                    status_code=status_code,
                    error_type="system_error",
                    message=error_result.get("user_message", "系统遇到未知错误"),
                    details={
                        "error_id": error_result.get("error_id"),
                        "recovery_info": "请稍后重试，如问题持续请联系管理员",
                    },
                )

            except Exception as resilience_error:
                # 如果韧性管理器本身出错，降级到基本处理
                logger.exception("韧性管理器处理异常时出错")

        # 降级处理：基本的异常响应
        return ErrorResponse.from_exception(exc)
