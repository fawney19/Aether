"""
基础消息处理器，封装通用的编排、转换、遥测逻辑。

接口约定：
- process_stream: 处理流式请求，返回 StreamingResponse
- process_sync: 处理非流式请求，返回 JSONResponse

签名规范（推荐）：
    async def process_stream(
        self,
        request: Any,                           # 解析后的请求模型
        http_request: Request,                  # FastAPI Request 对象
        original_headers: Dict[str, str],       # 原始请求头
        original_request_body: Dict[str, Any],  # 原始请求体
        query_params: Optional[Dict[str, str]] = None,  # 查询参数
    ) -> StreamingResponse: ...

    async def process_sync(
        self,
        request: Any,
        http_request: Request,
        original_headers: Dict[str, str],
        original_request_body: Dict[str, Any],
        query_params: Optional[Dict[str, str]] = None,
    ) -> JSONResponse: ...
"""

from __future__ import annotations

import time
from typing import Any, Callable, Dict, Optional, Protocol, runtime_checkable

from fastapi import Request
from fastapi.responses import JSONResponse, StreamingResponse
from sqlalchemy.orm import Session

from src.clients.redis_client import get_redis_client_sync
from src.core.api_format_metadata import resolve_api_format
from src.core.enums import APIFormat
from src.core.logger import logger
from src.services.orchestration.fallback_orchestrator import FallbackOrchestrator
from src.services.provider.format import normalize_api_format
from src.services.system.audit import audit_service
from src.services.usage.service import UsageService



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
        request_body: Dict[str, Any],
        request_headers: Dict[str, Any],
        response_body: Any,
        response_headers: Dict[str, Any],
        cache_creation_tokens: int = 0,
        cache_read_tokens: int = 0,
        is_stream: bool = False,
        provider_request_headers: Optional[Dict[str, Any]] = None,
        # Provider 侧追踪信息（用于记录真实成本）
        provider_id: Optional[str] = None,
        provider_endpoint_id: Optional[str] = None,
        provider_api_key_id: Optional[str] = None,
        api_format: Optional[str] = None,
        # 模型映射信息
        target_model: Optional[str] = None,
        # Provider 响应元数据（如 Gemini 的 modelVersion）
        response_metadata: Optional[Dict[str, Any]] = None,
    ) -> float:
        total_cost = await self.calculate_cost(
            provider,
            model,
            input_tokens=input_tokens,
            output_tokens=output_tokens,
            cache_creation_tokens=cache_creation_tokens,
            cache_read_tokens=cache_read_tokens,
        )

        await UsageService.record_usage(
            db=self.db,
            user=self.user,
            api_key=self.api_key,
            provider=provider,
            model=model,
            input_tokens=input_tokens,
            output_tokens=output_tokens,
            cache_creation_input_tokens=cache_creation_tokens,
            cache_read_input_tokens=cache_read_tokens,
            request_type="chat",
            api_format=api_format,
            is_stream=is_stream,
            response_time_ms=response_time_ms,
            status_code=status_code,
            request_headers=request_headers,
            request_body=request_body,
            provider_request_headers=provider_request_headers or {},
            response_headers=response_headers,
            response_body=response_body,
            request_id=self.request_id,
            # Provider 侧追踪信息（用于记录真实成本）
            provider_id=provider_id,
            provider_endpoint_id=provider_endpoint_id,
            provider_api_key_id=provider_api_key_id,
            # 模型映射信息
            target_model=target_model,
            # Provider 响应元数据
            metadata=response_metadata,
        )

        if self.user and self.api_key:
            audit_service.log_api_request(
                db=self.db,
                user_id=self.user.id,
                api_key_id=self.api_key.id,
                request_id=self.request_id,
                model=model,
                provider=provider,
                success=True,
                ip_address=self.client_ip,
                status_code=status_code,
                input_tokens=input_tokens,
                output_tokens=output_tokens,
                cost_usd=total_cost,
            )

        return total_cost

    async def record_failure(
        self,
        *,
        provider: str,
        model: str,
        response_time_ms: int,
        status_code: int,
        error_message: str,
        request_body: Dict[str, Any],
        request_headers: Dict[str, Any],
        is_stream: bool,
        api_format: Optional[str] = None,
        provider_request_headers: Optional[Dict[str, Any]] = None,
        # 预估 token 信息（来自 message_start 事件，用于中断请求的成本估算）
        input_tokens: int = 0,
        output_tokens: int = 0,
        cache_creation_tokens: int = 0,
        cache_read_tokens: int = 0,
        response_body: Optional[Dict[str, Any]] = None,
        # 模型映射信息
        target_model: Optional[str] = None,
    ) -> None:
        """
        记录失败请求

        注意：Provider 链路信息（provider_id, endpoint_id, key_id）不在此处记录，
        因为 RequestCandidate 表已经记录了完整的请求链路追踪信息。

        Args:
            input_tokens: 预估输入 tokens（来自 message_start，用于中断请求的成本估算）
            output_tokens: 预估输出 tokens（来自已收到的内容）
            cache_creation_tokens: 缓存创建 tokens
            cache_read_tokens: 缓存读取 tokens
            response_body: 响应体（如果有部分响应）
            target_model: 映射后的目标模型名（如果发生了映射）
        """
        provider_name = provider or "unknown"
        if provider_name == "unknown":
            logger.warning(f"[Telemetry] Recording failure with unknown provider (request_id={self.request_id})")

        await UsageService.record_usage(
            db=self.db,
            user=self.user,
            api_key=self.api_key,
            provider=provider_name,
            model=model,
            input_tokens=input_tokens,
            output_tokens=output_tokens,
            cache_creation_input_tokens=cache_creation_tokens,
            cache_read_input_tokens=cache_read_tokens,
            request_type="chat",
            api_format=api_format,
            is_stream=is_stream,
            response_time_ms=response_time_ms,
            status_code=status_code,
            error_message=error_message,
            request_headers=request_headers,
            request_body=request_body,
            provider_request_headers=provider_request_headers or {},
            response_headers={},
            response_body=response_body or {"error": error_message},
            request_id=self.request_id,
            # 模型映射信息
            target_model=target_model,
        )


@runtime_checkable
class MessageHandlerProtocol(Protocol):
    """
    消息处理器协议 - 定义标准接口

    ChatHandlerBase 使用完整签名（含 request, http_request）。
    CliMessageHandlerBase 使用简化签名（仅 original_request_body, original_headers）。
    """

    async def process_stream(
        self,
        request: Any,
        http_request: Request,
        original_headers: Dict[str, str],
        original_request_body: Dict[str, Any],
        query_params: Optional[Dict[str, str]] = None,
    ) -> StreamingResponse:
        """处理流式请求"""
        ...

    async def process_sync(
        self,
        request: Any,
        http_request: Request,
        original_headers: Dict[str, str],
        original_request_body: Dict[str, Any],
        query_params: Optional[Dict[str, str]] = None,
    ) -> JSONResponse:
        """处理非流式请求"""
        ...


class BaseMessageHandler:
    """
    消息处理器基类，所有具体格式的 handler 可以继承它。

    子类需要实现：
    - process_stream: 处理流式请求
    - process_sync: 处理非流式请求

    推荐使用 MessageHandlerProtocol 中定义的签名。
    """

    # Adapter 检测器类型
    AdapterDetectorType = Callable[[Dict[str, str], Optional[Dict[str, Any]]], Dict[str, bool]]

    def __init__(
        self,
        *,
        db: Session,
        user: Any,
        api_key: Any,
        request_id: str,
        client_ip: str,
        user_agent: str,
        start_time: float,
        allowed_api_formats: Optional[list[str]] = None,
        adapter_detector: Optional[AdapterDetectorType] = None,
    ) -> None:
        self.db = db
        self.user = user
        self.api_key = api_key
        self.request_id = request_id
        self.client_ip = client_ip
        self.user_agent = user_agent
        self.start_time = start_time
        self.allowed_api_formats = allowed_api_formats or [APIFormat.CLAUDE.value]
        self.primary_api_format = normalize_api_format(self.allowed_api_formats[0])
        self.adapter_detector = adapter_detector

        redis_client = get_redis_client_sync()
        self.orchestrator = FallbackOrchestrator(db, redis_client)  # type: ignore[arg-type]
        self.telemetry = MessageTelemetry(db, user, api_key, request_id, client_ip)

    def elapsed_ms(self) -> int:
        return int((time.time() - self.start_time) * 1000)

    def _resolve_capability_requirements(
        self,
        model_name: str,
        request_headers: Optional[Dict[str, str]] = None,
        request_body: Optional[Dict[str, Any]] = None,
    ) -> Dict[str, bool]:
        """
        解析请求的能力需求

        来源:
        1. 用户模型级配置 (User.model_capability_settings)
        2. 用户 API Key 强制配置 (ApiKey.force_capabilities)
        3. 请求头 X-Require-Capability
        4. Adapter 的 detect_capability_requirements（如 Claude 的 anthropic-beta）

        Args:
            model_name: 模型名称
            request_headers: 请求头
            request_body: 请求体（可选）

        Returns:
            能力需求字典
        """
        from src.services.capability.resolver import CapabilityResolver

        return CapabilityResolver.resolve_requirements(
            user=self.user,
            user_api_key=self.api_key,
            model_name=model_name,
            request_headers=request_headers,
            request_body=request_body,
            adapter_detector=self.adapter_detector,
        )

    def get_api_format(self, provider_type: Optional[str] = None) -> APIFormat:
        """根据 provider_type 解析 API 格式，未知类型默认 OPENAI"""
        if provider_type:
            result = resolve_api_format(provider_type, default=APIFormat.OPENAI)
            return result or APIFormat.OPENAI
        return self.primary_api_format

    def build_provider_payload(
        self,
        original_body: Dict[str, Any],
        *,
        mapped_model: Optional[str] = None,
    ) -> Dict[str, Any]:
        """构建发送给 Provider 的请求体，替换 model 名称"""
        payload = dict(original_body)
        if mapped_model:
            payload["model"] = mapped_model
        return payload

    def _update_usage_to_streaming(self, request_id: Optional[str] = None) -> None:
        """更新 Usage 状态为 streaming（流式传输开始时调用）

        使用 asyncio 后台任务执行数据库更新，避免阻塞流式传输

        Args:
            request_id: 请求 ID，如果不传则使用 self.request_id
        """
        import asyncio
        from src.database.database import get_db

        target_request_id = request_id or self.request_id

        async def _do_update() -> None:
            try:
                db_gen = get_db()
                db = next(db_gen)
                try:
                    UsageService.update_usage_status(
                        db=db,
                        request_id=target_request_id,
                        status="streaming",
                    )
                finally:
                    db.close()
            except Exception as e:
                logger.warning(f"[{target_request_id}] 更新 Usage 状态为 streaming 失败: {e}")

        # 创建后台任务，不阻塞当前流
        asyncio.create_task(_do_update())
