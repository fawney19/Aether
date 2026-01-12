"""
统一的 Usage 记录器

设计原则：
1. 单一入口：所有 Usage 记录都通过 UsageRecorder
2. 自动处理：根据 RequestResult 自动判断成功/失败
3. 完整记录：确保所有必要字段都被记录
4. 异步友好：支持后台异步记录，不阻塞主流程

使用方式：
```python
recorder = UsageRecorder(db, user, api_key)

# 记录成功请求
await recorder.record_success(result)

# 记录失败请求
await recorder.record_failure(result)

# 或者自动判断
await recorder.record(result)
```
"""

from typing import Any, Dict, Optional

from sqlalchemy.orm import Session

from src.core.logger import logger
from src.models.database import ApiKey, User
from src.services.request.result import RequestResult
from src.services.system.audit import audit_service
from src.services.usage.service import UsageService



class UsageRecorder:
    """
    统一的 Usage 记录器

    职责：
    1. 记录成功请求的 Usage（包含 token 使用量和费用）
    2. 记录失败请求的 Usage（token=0，记录错误信息）
    3. 记录审计日志
    4. 更新用户配额
    """

    def __init__(
        self,
        db: Session,
        user: User,
        api_key: ApiKey,
        client_ip: str = "unknown",
        request_id: Optional[str] = None,
    ):
        self.db = db
        self.user = user
        self.api_key = api_key
        self.client_ip = client_ip
        self.request_id = request_id

    async def record(self, result: RequestResult) -> None:
        """
        根据 RequestResult 自动判断并记录 Usage

        Args:
            result: 请求结果
        """
        if result.is_success:
            await self.record_success(result)
        else:
            await self.record_failure(result)

    async def record_success(
        self,
        result: RequestResult,
        request_headers: Optional[Dict[str, str]] = None,
        request_body: Optional[Dict[str, Any]] = None,
    ) -> None:
        """
        记录成功请求的 Usage

        Args:
            result: 成功的请求结果
            request_headers: 原始请求头（可选，用于调试）
            request_body: 原始请求体（可选，用于调试）
        """
        metadata = result.metadata
        usage = result.usage

        # 确定 target_model：当存在 original_model 且与 model 不同时，说明发生了映射
        target_model = None
        if metadata.original_model and metadata.original_model != metadata.model:
            target_model = metadata.model

        # 非流式成功时，返回给客户端的是提供商响应头（透传）+ content-type
        client_response_headers = dict(metadata.provider_response_headers) if metadata.provider_response_headers else {}
        client_response_headers["content-type"] = "application/json"

        await UsageService.record_usage(
            db=self.db,
            user=self.user,
            api_key=self.api_key,
            provider=metadata.provider,
            model=metadata.original_model or metadata.model,
            target_model=target_model,
            input_tokens=usage.input_tokens,
            output_tokens=usage.output_tokens,
            cache_creation_input_tokens=usage.cache_creation_input_tokens,
            cache_read_input_tokens=usage.cache_read_input_tokens,
            request_type="chat",
            api_format=metadata.api_format,
            is_stream=result.is_stream,
            response_time_ms=result.response_time_ms,
            status_code=200,
            error_message=None,
            metadata=metadata.response_metadata if metadata.response_metadata else None,
            request_headers=request_headers or result.request_headers,
            request_body=request_body or result.request_body,
            provider_request_headers=metadata.provider_request_headers,
            response_headers=metadata.provider_response_headers,
            client_response_headers=client_response_headers,
            response_body=result.response_data if isinstance(result.response_data, dict) else {},
            request_id=self.request_id,
            provider_id=metadata.provider_id,
            provider_endpoint_id=metadata.provider_endpoint_id,
            provider_api_key_id=metadata.provider_api_key_id,
            status="completed",  # 成功请求
        )

        # 记录审计日志
        audit_service.log_api_request(
            db=self.db,
            user_id=self.user.id,
            api_key_id=self.api_key.id,
            request_id=self.request_id or "",
            model=metadata.original_model or metadata.model,
            provider=metadata.provider,
            success=True,
            ip_address=self.client_ip,
            status_code=200,
        )

        logger.debug(f"[UsageRecorder] 成功记录: provider={metadata.provider}, "
            f"model={metadata.model}, api_format={metadata.api_format}, "
            f"tokens={usage.input_tokens}+{usage.output_tokens}")

    async def record_failure(
        self,
        result: RequestResult,
        request_headers: Optional[Dict[str, str]] = None,
        request_body: Optional[Dict[str, Any]] = None,
    ) -> None:
        """
        记录失败请求的 Usage

        Args:
            result: 失败的请求结果
            request_headers: 原始请求头
            request_body: 原始请求体
        """
        metadata = result.metadata

        # 确定 target_model：当存在 original_model 且与 model 不同时，说明发生了映射
        target_model = None
        if metadata.original_model and metadata.original_model != metadata.model:
            target_model = metadata.model

        await UsageService.record_usage(
            db=self.db,
            user=self.user,
            api_key=self.api_key,
            provider=metadata.provider,
            model=metadata.original_model or metadata.model,
            target_model=target_model,
            input_tokens=0,
            output_tokens=0,
            request_type="chat",
            api_format=metadata.api_format,
            is_stream=result.is_stream,
            response_time_ms=result.response_time_ms,
            status_code=result.status_code,
            error_message=result.error_message,
            metadata=metadata.response_metadata if metadata.response_metadata else None,
            request_headers=request_headers or result.request_headers,
            request_body=request_body or result.request_body,
            provider_request_headers=metadata.provider_request_headers,
            response_headers={},
            # 失败请求返回给客户端的是 JSON 错误响应
            client_response_headers={"content-type": "application/json"},
            response_body={"error": result.error_message} if result.error_message else {},
            request_id=self.request_id,
            provider_id=metadata.provider_id,
            provider_endpoint_id=metadata.provider_endpoint_id,
            provider_api_key_id=metadata.provider_api_key_id,
            status="failed",  # 失败请求
        )

        # 记录审计日志
        audit_service.log_api_request(
            db=self.db,
            user_id=self.user.id,
            api_key_id=self.api_key.id,
            request_id=self.request_id or "",
            model=metadata.original_model or metadata.model,
            provider=metadata.provider,
            success=False,
            ip_address=self.client_ip,
            status_code=result.status_code,
            error_message=result.error_message,
        )

        logger.debug(f"[UsageRecorder] 失败记录: provider={metadata.provider}, "
            f"model={metadata.model}, api_format={metadata.api_format}, "
            f"status={result.status_code}, error={result.error_message[:100] if result.error_message else 'N/A'}")

    async def record_from_exception(
        self,
        exception: Exception,
        api_format: str,
        model: str,
        response_time_ms: int,
        is_stream: bool = False,
        request_headers: Optional[Dict[str, str]] = None,
        request_body: Optional[Dict[str, Any]] = None,
    ) -> None:
        """
        从异常创建 RequestResult 并记录失败

        这是一个便捷方法，用于在异常处理中快速记录失败请求。

        Args:
            exception: 捕获的异常
            api_format: API 格式（必须提供，确保始终有值）
            model: 模型名称
            response_time_ms: 响应时间
            is_stream: 是否流式请求
            request_headers: 原始请求头
            request_body: 原始请求体
        """
        result = RequestResult.from_exception(
            exception=exception,
            api_format=api_format,
            model=model,
            response_time_ms=response_time_ms,
            is_stream=is_stream,
        )
        result.request_headers = request_headers or {}
        result.request_body = request_body or {}

        await self.record_failure(result, request_headers, request_body)
