"""
消息遥测记录器。

从 api/handlers/base/base_handler.py 迁移到 services 层，
消除 services→api 的反向依赖。
"""

from __future__ import annotations

from collections.abc import Awaitable, Callable
from typing import Any, TypeVar

from sqlalchemy.orm import Session

from src.core.logger import logger
from src.database import create_session
from src.services.system.audit import audit_service
from src.services.usage._db_retry import run_async_db_retry
from src.services.usage.service import UsageService

T = TypeVar("T")


class MessageTelemetry:
    """
    负责记录 Usage/Audit，避免处理器里重复代码。
    """

    def __init__(
        self, db: Session, user: Any, api_key: Any, request_id: str, client_ip: str
    ) -> None:
        self.db = db
        self.user = user
        self.api_key = api_key
        self.request_id = request_id
        self.client_ip = client_ip

    def _build_usage_metadata(
        self,
        *,
        request_metadata: dict[str, Any] | None = None,
        response_metadata: dict[str, Any] | None = None,
    ) -> dict[str, Any] | None:
        metadata: dict[str, Any] | None = None

        if request_metadata:
            metadata = dict(request_metadata)
            if response_metadata:
                metadata.setdefault("response", response_metadata)
        elif response_metadata:
            metadata = dict(response_metadata)

        return metadata

    async def _run_in_isolated_session(
        self,
        operation: Callable[[Session], Awaitable[T]],
        *,
        context: str,
    ) -> T:
        async def _attempt() -> T:
            bg_db = create_session()
            try:
                return await operation(bg_db)
            except Exception:
                bg_db.rollback()
                raise
            finally:
                bg_db.close()

        return await run_async_db_retry(_attempt, context=context)

    async def calculate_cost(
        self,
        provider: str,
        model: str,
        *,
        input_tokens: int,
        output_tokens: int,
        cache_creation_tokens: int = 0,
        cache_read_tokens: int = 0,
    ) -> float:
        input_price, output_price = await UsageService.get_model_price_async(
            self.db, provider, model
        )
        _, _, _, _, _, _, total_cost = UsageService.calculate_cost(
            input_tokens,
            output_tokens,
            input_price,
            output_price,
            cache_creation_tokens,
            cache_read_tokens,
            *await UsageService.get_cache_prices_async(self.db, provider, model, input_price),
        )
        return total_cost

    async def record_success(
        self,
        *,
        provider: str,
        model: str,
        input_tokens: int,
        output_tokens: int,
        response_time_ms: int,
        status_code: int,
        request_body: dict[str, Any],
        request_headers: dict[str, Any],
        response_body: Any,
        response_headers: dict[str, Any],
        client_response_headers: dict[str, Any] | None = None,
        cache_creation_tokens: int = 0,
        cache_read_tokens: int = 0,
        cache_creation_tokens_5m: int = 0,
        cache_creation_tokens_1h: int = 0,
        is_stream: bool = False,
        provider_request_headers: dict[str, Any] | None = None,
        provider_request_body: Any | None = None,
        client_response_body: Any | None = None,
        # 时间指标
        first_byte_time_ms: int | None = None,  # 首字时间/TTFB
        # Provider 侧追踪信息（用于记录真实成本）
        provider_id: str | None = None,
        provider_endpoint_id: str | None = None,
        provider_api_key_id: str | None = None,
        api_format: str | None = None,
        # 结构化格式维度（从 Adapter 层透传）
        api_family: str | None = None,
        endpoint_kind: str | None = None,
        # 格式转换追踪
        endpoint_api_format: str | None = None,  # 端点原生 API 格式
        has_format_conversion: bool = False,  # 是否发生了格式转换
        # 模型映射信息
        target_model: str | None = None,
        # Provider 响应元数据（如 Gemini 的 modelVersion）
        response_metadata: dict[str, Any] | None = None,
        # 请求元数据（用于性能与调试记录）
        request_metadata: dict[str, Any] | None = None,
    ) -> float:
        metadata = self._build_usage_metadata(
            request_metadata=request_metadata,
            response_metadata=response_metadata,
        )

        async def _write(bg_db: Session) -> float:
            usage = await UsageService.record_usage(
                db=bg_db,
                user=self.user,
                api_key=self.api_key,
                provider=provider,
                model=model,
                input_tokens=input_tokens,
                output_tokens=output_tokens,
                cache_creation_input_tokens=cache_creation_tokens,
                cache_read_input_tokens=cache_read_tokens,
                cache_creation_input_tokens_5m=cache_creation_tokens_5m,
                cache_creation_input_tokens_1h=cache_creation_tokens_1h,
                request_type="chat",
                api_format=api_format,
                api_family=api_family,
                endpoint_kind=endpoint_kind,
                endpoint_api_format=endpoint_api_format,
                has_format_conversion=has_format_conversion,
                is_stream=is_stream,
                response_time_ms=response_time_ms,
                first_byte_time_ms=first_byte_time_ms,
                status_code=status_code,
                request_headers=request_headers,
                request_body=request_body,
                provider_request_headers=provider_request_headers or {},
                provider_request_body=provider_request_body,
                response_headers=response_headers,
                client_response_headers=client_response_headers,
                response_body=response_body,
                client_response_body=client_response_body,
                request_id=self.request_id,
                provider_id=provider_id,
                provider_endpoint_id=provider_endpoint_id,
                provider_api_key_id=provider_api_key_id,
                target_model=target_model,
                metadata=metadata,
            )

            total_cost = float(getattr(usage, "total_cost_usd", 0.0) or 0.0)
            user_id = getattr(self.user, "id", None)
            api_key_id = getattr(self.api_key, "id", None)
            if user_id and api_key_id:
                audit_service.log_api_request(
                    db=bg_db,
                    user_id=str(user_id),
                    api_key_id=str(api_key_id),
                    request_id=self.request_id,
                    model=model,
                    provider=provider,
                    success=True,
                    ip_address=self.client_ip,
                    status_code=status_code,
                    input_tokens=getattr(usage, "input_tokens", input_tokens),
                    output_tokens=getattr(usage, "output_tokens", output_tokens),
                    cost_usd=total_cost,
                )
                bg_db.commit()

            return total_cost

        return await self._run_in_isolated_session(
            _write,
            context=f"telemetry.record_success:{self.request_id}",
        )

    async def record_failure(
        self,
        *,
        provider: str,
        model: str,
        response_time_ms: int,
        status_code: int,
        error_message: str,
        request_body: dict[str, Any],
        request_headers: dict[str, Any],
        is_stream: bool,
        api_format: str | None = None,
        api_family: str | None = None,
        endpoint_kind: str | None = None,
        provider_request_headers: dict[str, Any] | None = None,
        provider_request_body: Any | None = None,
        # 预估 token 信息（来自 message_start 事件，用于中断请求的成本估算）
        input_tokens: int = 0,
        output_tokens: int = 0,
        cache_creation_tokens: int = 0,
        cache_read_tokens: int = 0,
        cache_creation_tokens_5m: int = 0,
        cache_creation_tokens_1h: int = 0,
        response_body: dict[str, Any] | None = None,
        response_headers: dict[str, Any] | None = None,
        client_response_headers: dict[str, Any] | None = None,
        client_response_body: Any | None = None,
        # Provider 侧追踪信息（用于 curl 复现等场景）
        provider_id: str | None = None,
        provider_endpoint_id: str | None = None,
        provider_api_key_id: str | None = None,
        # 格式转换追踪
        endpoint_api_format: str | None = None,
        has_format_conversion: bool = False,
        # 模型映射信息
        target_model: str | None = None,
        # 请求元数据（用于性能与调试记录）
        request_metadata: dict[str, Any] | None = None,
    ) -> None:
        """
        记录失败请求

        Args:
            input_tokens: 预估输入 tokens（来自 message_start，用于中断请求的成本估算）
            output_tokens: 预估输出 tokens（来自已收到的内容）
            cache_creation_tokens: 缓存创建 tokens
            cache_read_tokens: 缓存读取 tokens
            response_body: 响应体（如果有部分响应）
            response_headers: 响应头（Provider 返回的原始响应头）
            client_response_headers: 返回给客户端的响应头
            target_model: 映射后的目标模型名（如果发生了映射）
        """
        provider_name = provider or "unknown"
        if provider_name == "unknown":
            logger.warning(
                "[Telemetry] Recording failure with unknown provider (request_id={})",
                self.request_id,
            )

        metadata = self._build_usage_metadata(
            request_metadata=request_metadata,
        )

        async def _write(bg_db: Session) -> None:
            await UsageService.record_usage(
                db=bg_db,
                user=self.user,
                api_key=self.api_key,
                provider=provider_name,
                model=model,
                input_tokens=input_tokens,
                output_tokens=output_tokens,
                cache_creation_input_tokens=cache_creation_tokens,
                cache_read_input_tokens=cache_read_tokens,
                cache_creation_input_tokens_5m=cache_creation_tokens_5m,
                cache_creation_input_tokens_1h=cache_creation_tokens_1h,
                request_type="chat",
                api_format=api_format,
                api_family=api_family,
                endpoint_kind=endpoint_kind,
                endpoint_api_format=endpoint_api_format,
                has_format_conversion=has_format_conversion,
                is_stream=is_stream,
                response_time_ms=response_time_ms,
                status_code=status_code,
                error_message=error_message,
                request_headers=request_headers,
                request_body=request_body,
                provider_request_headers=provider_request_headers or {},
                provider_request_body=provider_request_body,
                response_headers=response_headers or {},
                client_response_headers=client_response_headers,
                response_body=response_body or {"error": error_message},
                client_response_body=client_response_body,
                request_id=self.request_id,
                provider_id=provider_id,
                provider_endpoint_id=provider_endpoint_id,
                provider_api_key_id=provider_api_key_id,
                target_model=target_model,
                metadata=metadata,
            )

        await self._run_in_isolated_session(
            _write,
            context=f"telemetry.record_failure:{self.request_id}",
        )

    async def record_cancelled(
        self,
        *,
        provider: str,
        model: str,
        response_time_ms: int,
        first_byte_time_ms: int | None,
        status_code: int,
        request_body: dict[str, Any],
        request_headers: dict[str, Any],
        is_stream: bool,
        api_format: str | None = None,
        api_family: str | None = None,
        endpoint_kind: str | None = None,
        provider_request_headers: dict[str, Any] | None = None,
        provider_request_body: Any | None = None,
        input_tokens: int = 0,
        output_tokens: int = 0,
        cache_creation_tokens: int = 0,
        cache_read_tokens: int = 0,
        cache_creation_tokens_5m: int = 0,
        cache_creation_tokens_1h: int = 0,
        response_body: dict[str, Any] | None = None,
        response_headers: dict[str, Any] | None = None,
        client_response_headers: dict[str, Any] | None = None,
        client_response_body: Any | None = None,
        # Provider 侧追踪信息
        provider_id: str | None = None,
        provider_endpoint_id: str | None = None,
        provider_api_key_id: str | None = None,
        # 格式转换追踪
        endpoint_api_format: str | None = None,
        has_format_conversion: bool = False,
        target_model: str | None = None,
        # 请求元数据（用于性能与调试记录）
        request_metadata: dict[str, Any] | None = None,
    ) -> None:
        """
        记录客户端取消的请求

        客户端主动断开连接不算系统失败，使用 cancelled 状态。
        """
        provider_name = provider or "unknown"
        metadata = self._build_usage_metadata(
            request_metadata=request_metadata,
        )

        async def _write(bg_db: Session) -> None:
            await UsageService.record_usage(
                db=bg_db,
                user=self.user,
                api_key=self.api_key,
                provider=provider_name,
                model=model,
                input_tokens=input_tokens,
                output_tokens=output_tokens,
                cache_creation_input_tokens=cache_creation_tokens,
                cache_read_input_tokens=cache_read_tokens,
                cache_creation_input_tokens_5m=cache_creation_tokens_5m,
                cache_creation_input_tokens_1h=cache_creation_tokens_1h,
                request_type="chat",
                api_format=api_format,
                api_family=api_family,
                endpoint_kind=endpoint_kind,
                endpoint_api_format=endpoint_api_format,
                has_format_conversion=has_format_conversion,
                is_stream=is_stream,
                response_time_ms=response_time_ms,
                first_byte_time_ms=first_byte_time_ms,
                status_code=status_code,
                status="cancelled",
                request_headers=request_headers,
                request_body=request_body,
                provider_request_headers=provider_request_headers or {},
                provider_request_body=provider_request_body,
                response_headers=response_headers or {},
                client_response_headers=client_response_headers,
                response_body=response_body or {},
                client_response_body=client_response_body,
                request_id=self.request_id,
                provider_id=provider_id,
                provider_endpoint_id=provider_endpoint_id,
                provider_api_key_id=provider_api_key_id,
                target_model=target_model,
                metadata=metadata,
            )

        await self._run_in_isolated_session(
            _write,
            context=f"telemetry.record_cancelled:{self.request_id}",
        )
