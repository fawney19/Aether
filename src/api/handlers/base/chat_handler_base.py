"""
Chat Handler Base - Chat API 格式的通用基类

提供 Chat API 格式（Claude Chat、OpenAI Chat）的通用处理逻辑。
与 CliMessageHandlerBase 的区别：
- CLI 模式：透传请求体，直接转发
- Chat 模式：可能需要格式转换（如 OpenAI -> Claude）

两者共享相同的接口：
- process_stream(): 流式请求
- process_sync(): 非流式请求
- apply_mapped_model(): 模型映射
- get_model_for_url(): URL 模型名
- _extract_usage(): 使用量提取

重构说明：
- StreamContext: 类型安全的流式上下文，替代原有的 ctx dict
- StreamProcessor: 流式响应处理（SSE 解析、预读、错误检测）
- StreamTelemetryRecorder: 统计记录（Usage、Audit、Candidate）
"""

import asyncio
from abc import ABC, abstractmethod
from typing import Any, AsyncGenerator, Callable, Dict, Optional

import httpx
from fastapi import BackgroundTasks, Request
from fastapi.responses import JSONResponse, StreamingResponse
from sqlalchemy.orm import Session

from src.api.handlers.base.base_handler import BaseMessageHandler
from src.api.handlers.base.parsers import get_parser_for_format
from src.api.handlers.base.request_builder import PassthroughRequestBuilder
from src.api.handlers.base.response_parser import ResponseParser
from src.api.handlers.base.stream_context import StreamContext
from src.api.handlers.base.stream_processor import StreamProcessor
from src.api.handlers.base.stream_telemetry import StreamTelemetryRecorder
from src.api.handlers.base.utils import build_sse_headers
from src.config.settings import config
from src.core.error_utils import extract_client_error_message
from src.core.exceptions import (
    EmbeddedErrorException,
    ProviderAuthException,
    ProviderNotAvailableException,
    ProviderRateLimitException,
    ProviderTimeoutException,
)
from src.core.logger import logger
from src.models.database import (
    ApiKey,
    Provider,
    ProviderAPIKey,
    ProviderEndpoint,
    User,
)
from src.services.cache.aware_scheduler import ProviderCandidate
from src.services.provider.transport import build_provider_url


class ChatHandlerBase(BaseMessageHandler, ABC):
    """
    Chat Handler 基类

    主要职责：
    - 通过 FallbackOrchestrator 选择 Provider/Endpoint/Key
    - 发送请求并处理响应
    - 记录日志、审计、统计
    - 错误处理

    子类需要实现：
    - FORMAT_ID: API 格式标识
    - _convert_request(): 请求格式转换
    - _extract_usage(): 从响应中提取 token 使用情况
    - _normalize_response(): 响应规范化（可选）

    与 CliMessageHandlerBase 共享的接口：
    - apply_mapped_model(): 模型映射到请求体
    - get_model_for_url(): 获取 URL 中的模型名
    """

    FORMAT_ID: str = ""  # 子类覆盖

    def __init__(
        self,
        db: Session,
        user: User,
        api_key: ApiKey,
        request_id: str,
        client_ip: str,
        user_agent: str,
        start_time: float,
        allowed_api_formats: Optional[list] = None,
        adapter_detector: Optional[
            Callable[[Dict[str, str], Optional[Dict[str, Any]]], Dict[str, bool]]
        ] = None,
    ):
        allowed = allowed_api_formats or [self.FORMAT_ID]
        super().__init__(
            db=db,
            user=user,
            api_key=api_key,
            request_id=request_id,
            client_ip=client_ip,
            user_agent=user_agent,
            start_time=start_time,
            allowed_api_formats=allowed,
            adapter_detector=adapter_detector,
        )
        self._parser: Optional[ResponseParser] = None
        self._request_builder = PassthroughRequestBuilder()

    @property
    def parser(self) -> ResponseParser:
        """获取响应解析器（懒加载）"""
        if self._parser is None:
            self._parser = get_parser_for_format(self.FORMAT_ID)
        return self._parser

    # ==================== 抽象方法 ====================

    @abstractmethod
    async def _convert_request(self, request: Any) -> Any:
        """
        将请求转换为目标格式

        Args:
            request: 原始请求对象

        Returns:
            转换后的请求对象
        """
        pass

    @abstractmethod
    def _extract_usage(self, response: Dict) -> Dict[str, int]:
        """
        从响应中提取 token 使用情况

        Args:
            response: 响应数据

        Returns:
            Dict with keys: input_tokens, output_tokens,
                           cache_creation_input_tokens, cache_read_input_tokens
        """
        pass

    def _normalize_response(self, response: Dict) -> Dict:
        """
        规范化响应（可选覆盖）

        Args:
            response: 原始响应

        Returns:
            规范化后的响应
        """
        return response

    # ==================== 统一接口方法 ====================

    def extract_model_from_request(
        self,
        request_body: Dict[str, Any],
        path_params: Optional[Dict[str, Any]] = None,  # noqa: ARG002 - 子类使用
    ) -> str:
        """
        从请求中提取模型名 - 子类可覆盖

        不同 API 格式的 model 位置不同：
        - OpenAI/Claude: 在请求体中 request_body["model"]
        - Gemini: 在 URL 路径中 path_params["model"]

        子类应覆盖此方法实现各自的提取逻辑。

        Args:
            request_body: 请求体
            path_params: URL 路径参数

        Returns:
            模型名，如果无法提取则返回 "unknown"
        """
        # 默认实现：从请求体获取
        model = request_body.get("model")
        return str(model) if model else "unknown"

    def apply_mapped_model(
        self,
        request_body: Dict[str, Any],
        mapped_model: str,  # noqa: ARG002 - 子类使用
    ) -> Dict[str, Any]:
        """
        将映射后的模型名应用到请求体

        基类默认实现：不修改请求体，保持原样透传。
        子类应覆盖此方法实现各自的模型名替换逻辑。

        Args:
            request_body: 原始请求体
            mapped_model: 映射后的模型名（子类使用）

        Returns:
            请求体（默认不修改）
        """
        # 基类不修改请求体，子类覆盖此方法实现特定格式的处理
        return request_body

    def get_model_for_url(
        self,
        request_body: Dict[str, Any],
        mapped_model: Optional[str],
    ) -> Optional[str]:
        """
        获取用于 URL 路径的模型名

        某些 API 格式（如 Gemini）需要将 model 放入 URL 路径中。
        子类应覆盖此方法返回正确的值。

        Args:
            request_body: 请求体
            mapped_model: 映射后的模型名（如果有）

        Returns:
            用于 URL 路径的模型名
        """
        return mapped_model or request_body.get("model")

    def prepare_provider_request_body(
        self,
        request_body: Dict[str, Any],
    ) -> Dict[str, Any]:
        """
        准备发送给 Provider 的请求体 - 子类可覆盖

        在模型映射之后、发送请求之前调用，用于移除不需要发送给上游的字段。
        例如 Gemini API 需要移除请求体中的 model 字段（因为 model 在 URL 路径中）。

        Args:
            request_body: 经过模型映射处理后的请求体

        Returns:
            准备好的请求体
        """
        return request_body

    async def _get_mapped_model(
        self,
        source_model: str,
        provider_id: str,
    ) -> Optional[str]:
        """
        获取模型映射后的实际模型名

        Args:
            source_model: 用户请求的模型名
            provider_id: Provider ID

        Returns:
            映射后的 provider_model_name，没有映射则返回 None
        """
        from src.services.model.mapper import ModelMapperMiddleware

        mapper = ModelMapperMiddleware(self.db)
        mapping = await mapper.get_mapping(source_model, provider_id)

        if mapping and mapping.model:
            # 使用 select_provider_model_name 支持映射功能
            # 传入 api_key.id 作为 affinity_key，实现相同用户稳定选择同一映射
            # 传入 api_format 用于过滤适用的映射作用域
            affinity_key = self.api_key.id if self.api_key else None
            mapped_name = mapping.model.select_provider_model_name(
                affinity_key, api_format=self.FORMAT_ID
            )
            logger.debug(f"[Chat] 模型映射: {source_model} -> {mapped_name}")
            return mapped_name

        return None

    # ==================== 流式处理 ====================

    async def process_stream(
        self,
        request: Any,
        http_request: Request,
        original_headers: Dict[str, Any],
        original_request_body: Dict[str, Any],
        query_params: Optional[Dict[str, str]] = None,
    ) -> StreamingResponse:
        """处理流式响应"""
        logger.debug(f"开始流式响应处理 ({self.FORMAT_ID})")

        # 转换请求格式
        converted_request = await self._convert_request(request)
        model = getattr(converted_request, "model", original_request_body.get("model", "unknown"))
        api_format = self.allowed_api_formats[0]

        # 创建类型安全的流式上下文
        ctx = StreamContext(model=model, api_format=api_format)

        # 创建更新状态的回调闭包（可以访问 ctx）
        def update_streaming_status() -> None:
            self._update_usage_to_streaming_with_ctx(ctx)

        # 创建流处理器
        stream_processor = StreamProcessor(
            request_id=self.request_id,
            default_parser=self.parser,
            on_streaming_start=update_streaming_status,
        )

        # 定义请求函数
        async def stream_request_func(
            provider: Provider,
            endpoint: ProviderEndpoint,
            key: ProviderAPIKey,
            candidate: ProviderCandidate,
        ) -> AsyncGenerator[bytes, None]:
            return await self._execute_stream_request(
                ctx,
                stream_processor,
                provider,
                endpoint,
                key,
                original_request_body,
                original_headers,
                query_params,
                candidate,
            )

        try:
            # 解析能力需求
            capability_requirements = self._resolve_capability_requirements(
                model_name=model,
                request_headers=original_headers,
            )

            # 执行请求（通过 FallbackOrchestrator）
            (
                stream_generator,
                provider_name,
                attempt_id,
                provider_id,
                endpoint_id,
                key_id,
            ) = await self.orchestrator.execute_with_fallback(
                api_format=api_format,
                model_name=model,
                user_api_key=self.api_key,
                request_func=stream_request_func,
                request_id=self.request_id,
                is_stream=True,
                capability_requirements=capability_requirements or None,
            )

            # 更新上下文
            ctx.attempt_id = attempt_id
            ctx.provider_name = provider_name
            ctx.provider_id = provider_id
            ctx.endpoint_id = endpoint_id
            ctx.key_id = key_id

            # 创建遥测记录器
            telemetry_recorder = StreamTelemetryRecorder(
                request_id=self.request_id,
                user_id=str(self.user.id),
                api_key_id=str(self.api_key.id),
                client_ip=self.client_ip,
                format_id=self.FORMAT_ID,
            )

            # 创建后台任务记录统计
            background_tasks = BackgroundTasks()
            background_tasks.add_task(
                telemetry_recorder.record_stream_stats,
                ctx,
                original_headers,
                original_request_body,
                self.start_time,  # 传入开始时间，让 telemetry 在流结束后计算响应时间
            )

            # 创建监控流
            monitored_stream = stream_processor.create_monitored_stream(
                ctx,
                stream_generator,
                http_request.is_disconnected,
            )

            # 透传提供商的响应头给客户端
            # 同时添加必要的 SSE 头以确保流式传输正常工作
            client_headers = dict(ctx.response_headers) if ctx.response_headers else {}
            # 添加/覆盖 SSE 必需的头
            client_headers.update(build_sse_headers())
            client_headers["content-type"] = "text/event-stream"

            return StreamingResponse(
                monitored_stream,
                media_type="text/event-stream",
                headers=client_headers,
                background=background_tasks,
            )

        except Exception as e:
            self._log_request_error("流式请求失败", e)
            await self._record_stream_failure(ctx, e, original_headers, original_request_body)
            raise

    async def _execute_stream_request(
        self,
        ctx: StreamContext,
        stream_processor: StreamProcessor,
        provider: Provider,
        endpoint: ProviderEndpoint,
        key: ProviderAPIKey,
        original_request_body: Dict[str, Any],
        original_headers: Dict[str, str],
        query_params: Optional[Dict[str, str]] = None,
        candidate: Optional[ProviderCandidate] = None,
    ) -> AsyncGenerator[bytes, None]:
        """执行流式请求并返回流生成器"""
        # 重置上下文状态（重试时清除之前的数据）
        ctx.reset_for_retry()

        # 更新 Provider 信息
        ctx.update_provider_info(
            provider_name=str(provider.name),
            provider_id=str(provider.id),
            endpoint_id=str(endpoint.id),
            key_id=str(key.id),
            provider_api_format=str(endpoint.api_format) if endpoint.api_format else None,
        )

        # 获取模型映射（优先使用别名匹配到的模型，其次是 Provider 级别的映射）
        mapped_model = candidate.alias_matched_model if candidate else None
        if not mapped_model:
            mapped_model = await self._get_mapped_model(
                source_model=ctx.model,
                provider_id=str(provider.id),
            )

        # 应用模型映射到请求体
        if mapped_model:
            ctx.mapped_model = mapped_model
            request_body = self.apply_mapped_model(original_request_body, mapped_model)
        else:
            request_body = dict(original_request_body)

        # 准备发送给 Provider 的请求体
        request_body = self.prepare_provider_request_body(request_body)

        # 构建请求
        provider_payload, provider_headers = self._request_builder.build(
            request_body,
            original_headers,
            endpoint,
            key,
            is_stream=True,
        )

        ctx.provider_request_headers = provider_headers
        ctx.provider_request_body = provider_payload

        # 获取 URL 模型名
        url_model = self.get_model_for_url(request_body, mapped_model) or ctx.model

        url = build_provider_url(
            endpoint,
            query_params=query_params,
            path_params={"model": url_model},
            is_stream=True,
        )

        logger.debug(
            f"  [{self.request_id}] 发送流式请求: Provider={provider.name}, "
            f"模型={ctx.model} -> {mapped_model or '无映射'}"
        )

        # 配置 HTTP 超时
        # 注意：read timeout 用于检测连接断开，不是整体请求超时
        # 整体请求超时由 asyncio.wait_for 控制，使用 provider.timeout
        timeout_config = httpx.Timeout(
            connect=config.http_connect_timeout,
            read=config.http_read_timeout,  # 使用全局配置，用于检测连接断开
            write=config.http_write_timeout,
            pool=config.http_pool_timeout,
        )

        # provider.timeout 作为整体请求超时（建立连接 + 获取首字节）
        request_timeout = float(provider.timeout or 300)

        # 创建 HTTP 客户端（支持代理配置，从 Provider 读取）
        from src.clients.http_client import HTTPClientPool

        http_client = HTTPClientPool.create_client_with_proxy(
            proxy_config=provider.proxy,
            timeout=timeout_config,
        )

        # 用于存储内部函数的结果（必须在函数定义前声明，供 nonlocal 使用）
        byte_iterator: Any = None
        prefetched_chunks: Any = None
        response_ctx: Any = None

        async def _connect_and_prefetch() -> None:
            """建立连接并预读首字节（受整体超时控制）"""
            nonlocal byte_iterator, prefetched_chunks, response_ctx
            response_ctx = http_client.stream(
                "POST", url, json=provider_payload, headers=provider_headers
            )
            stream_response = await response_ctx.__aenter__()

            ctx.status_code = stream_response.status_code
            ctx.response_headers = dict(stream_response.headers)

            stream_response.raise_for_status()

            # 使用字节流迭代器（避免 aiter_lines 的性能问题, aiter_bytes 会自动解压 gzip/deflate）
            byte_iterator = stream_response.aiter_bytes()

            # 预读检测嵌套错误
            prefetched_chunks = await stream_processor.prefetch_and_check_error(
                byte_iterator,
                provider,
                endpoint,
                ctx,
                max_prefetch_lines=config.stream_prefetch_lines,
            )

        try:
            # 使用 asyncio.wait_for 包裹整个"建立连接 + 获取首字节"阶段
            # provider.timeout 控制整体超时，避免上游长时间无响应
            await asyncio.wait_for(_connect_and_prefetch(), timeout=request_timeout)

        except asyncio.TimeoutError:
            # 整体请求超时（建立连接 + 获取首字节）
            # 清理可能已建立的连接上下文
            if response_ctx is not None:
                try:
                    await response_ctx.__aexit__(None, None, None)
                except Exception:
                    pass
            await http_client.aclose()
            logger.warning(
                f"  [{self.request_id}] 请求超时: Provider={provider.name}, timeout={request_timeout}s"
            )
            raise ProviderTimeoutException(
                provider_name=str(provider.name),
                timeout=int(request_timeout),
            )

        except httpx.HTTPStatusError as e:
            error_text = await self._extract_error_text(e)
            logger.error(f"Provider 返回错误: {e.response.status_code}\n  Response: {error_text}")
            await http_client.aclose()
            # 将上游错误信息附加到异常，以便故障转移时能够返回给客户端
            e.upstream_response = error_text  # type: ignore[attr-defined]
            raise

        except EmbeddedErrorException:
            try:
                if response_ctx is not None:
                    await response_ctx.__aexit__(None, None, None)
            except Exception:
                pass
            await http_client.aclose()
            raise

        except Exception:
            await http_client.aclose()
            raise

        # 类型断言：成功执行后这些变量不会为 None
        assert byte_iterator is not None
        assert prefetched_chunks is not None
        assert response_ctx is not None

        # 创建流生成器（传入字节流迭代器）
        return stream_processor.create_response_stream(
            ctx,
            byte_iterator,
            response_ctx,
            http_client,
            prefetched_chunks,
            start_time=self.start_time,
        )

    async def _record_stream_failure(
        self,
        ctx: StreamContext,
        error: Exception,
        original_headers: Dict[str, str],
        original_request_body: Dict[str, Any],
    ) -> None:
        """记录流式请求失败"""
        response_time_ms = self.elapsed_ms()

        status_code = 503
        if isinstance(error, ProviderAuthException):
            status_code = 503
        elif isinstance(error, ProviderRateLimitException):
            status_code = 429
        elif isinstance(error, ProviderTimeoutException):
            status_code = 504

        actual_request_body = ctx.provider_request_body or original_request_body

        # 失败时返回给客户端的是 JSON 错误响应
        client_response_headers = {"content-type": "application/json"}

        await self.telemetry.record_failure(
            provider=ctx.provider_name or "unknown",
            model=ctx.model,
            response_time_ms=response_time_ms,
            status_code=status_code,
            error_message=extract_client_error_message(error),
            request_headers=original_headers,
            request_body=actual_request_body,
            is_stream=True,
            api_format=ctx.api_format,
            provider_request_headers=ctx.provider_request_headers,
            response_headers=ctx.response_headers,
            client_response_headers=client_response_headers,
            target_model=ctx.mapped_model,
        )

    # ==================== 非流式处理 ====================

    async def process_sync(
        self,
        request: Any,
        http_request: Request,
        original_headers: Dict[str, Any],
        original_request_body: Dict[str, Any],
        query_params: Optional[Dict[str, str]] = None,
    ) -> JSONResponse:
        """处理非流式响应"""
        logger.debug(f"开始非流式响应处理 ({self.FORMAT_ID})")

        # 转换请求格式
        converted_request = await self._convert_request(request)
        model = getattr(converted_request, "model", original_request_body.get("model", "unknown"))
        api_format = self.allowed_api_formats[0]

        # 用于跟踪的变量
        provider_name: Optional[str] = None
        response_json: Optional[Dict[str, Any]] = None
        status_code = 200
        response_headers: Dict[str, str] = {}
        provider_request_headers: Dict[str, str] = {}
        provider_request_body: Optional[Dict[str, Any]] = None
        provider_id: Optional[str] = None  # Provider ID（用于失败记录）
        endpoint_id: Optional[str] = None  # Endpoint ID（用于失败记录）
        key_id: Optional[str] = None  # Key ID（用于失败记录）
        mapped_model_result: Optional[str] = None  # 映射后的目标模型名（用于 Usage 记录）

        async def sync_request_func(
            provider: Provider,
            endpoint: ProviderEndpoint,
            key: ProviderAPIKey,
            candidate: ProviderCandidate,
        ) -> Dict[str, Any]:
            nonlocal provider_name, response_json, status_code, response_headers
            nonlocal provider_request_headers, provider_request_body, mapped_model_result

            provider_name = str(provider.name)

            # 获取模型映射（优先使用别名匹配到的模型，其次是 Provider 级别的映射）
            mapped_model = candidate.alias_matched_model if candidate else None
            if not mapped_model:
                mapped_model = await self._get_mapped_model(
                    source_model=model,
                    provider_id=str(provider.id),
                )

            # 应用模型映射
            if mapped_model:
                mapped_model_result = mapped_model  # 保存映射后的模型名，用于 Usage 记录
                request_body = self.apply_mapped_model(original_request_body, mapped_model)
            else:
                request_body = dict(original_request_body)

            # 准备发送给 Provider 的请求体（子类可覆盖以移除不需要的字段）
            request_body = self.prepare_provider_request_body(request_body)

            # 构建请求
            provider_payload, provider_hdrs = self._request_builder.build(
                request_body,
                original_headers,
                endpoint,
                key,
                is_stream=False,
            )

            provider_request_headers = provider_hdrs
            provider_request_body = provider_payload

            # 获取 URL 模型名（兜底使用外层的 model，确保 Gemini 等格式能正确构建 URL）
            url_model = self.get_model_for_url(request_body, mapped_model) or model

            url = build_provider_url(
                endpoint,
                query_params=query_params,
                path_params={"model": url_model},
                is_stream=False,
            )

            logger.info(
                f"  [{self.request_id}] 发送非流式请求: Provider={provider.name}, "
                f"模型={model} -> {mapped_model or '无映射'}"
            )
            logger.debug(f"  [{self.request_id}] 请求URL: {url}")
            logger.debug(
                f"  [{self.request_id}] 请求体stream字段: {provider_payload.get('stream', 'N/A')}"
            )

            # 获取复用的 HTTP 客户端（支持代理配置，从 Provider 读取）
            # 注意：使用 get_proxy_client 复用连接池，不再每次创建新客户端
            from src.clients.http_client import HTTPClientPool

            request_timeout = float(provider.timeout or 300)
            http_client = await HTTPClientPool.get_proxy_client(
                proxy_config=provider.proxy,
            )

            # 注意：不使用 async with，因为复用的客户端不应该被关闭
            # 超时通过 timeout 参数控制
            resp = await http_client.post(
                url,
                json=provider_payload,
                headers=provider_hdrs,
                timeout=httpx.Timeout(request_timeout),
            )

            status_code = resp.status_code
            response_headers = dict(resp.headers)

            if resp.status_code == 401:
                raise ProviderAuthException(str(provider.name))
            elif resp.status_code == 429:
                raise ProviderRateLimitException(
                    "请求过于频繁，请稍后重试",
                    provider_name=str(provider.name),
                    response_headers=response_headers,
                )
            elif resp.status_code >= 500:
                # 记录响应体以便调试
                error_body = ""
                try:
                    error_body = resp.text[:1000]
                    logger.error(
                        f"  [{self.request_id}] 上游返回5xx错误: status={resp.status_code}, body={error_body[:500]}"
                    )
                except Exception:
                    pass
                raise ProviderNotAvailableException(
                    f"上游服务暂时不可用 (HTTP {resp.status_code})",
                    provider_name=str(provider.name),
                    upstream_status=resp.status_code,
                    upstream_response=error_body,
                )
            elif resp.status_code != 200:
                # 记录非200响应以便调试
                error_body = ""
                try:
                    error_body = resp.text[:1000]
                    logger.warning(
                        f"  [{self.request_id}] 上游返回非200: status={resp.status_code}, body={error_body[:500]}"
                    )
                except Exception:
                    pass
                raise ProviderNotAvailableException(
                    f"上游服务返回错误 (HTTP {resp.status_code})",
                    provider_name=str(provider.name),
                    upstream_status=resp.status_code,
                    upstream_response=error_body,
                )

            # 安全解析 JSON 响应，处理可能的编码错误
            try:
                response_json = resp.json()
            except (UnicodeDecodeError, json.JSONDecodeError) as e:
                # 获取原始响应内容用于调试（存入 upstream_response）
                raw_content = ""
                try:
                    raw_content = resp.text[:500] if resp.text else "(empty)"
                except Exception:
                    try:
                        raw_content = repr(resp.content[:500]) if resp.content else "(empty)"
                    except Exception:
                        raw_content = "(unable to read)"
                logger.error(
                    f"[{self.request_id}] 无法解析响应 JSON: {e}, 原始内容: {raw_content}"
                )
                # 判断错误类型，生成友好的客户端错误消息（不暴露提供商信息）
                if raw_content == "(empty)" or not raw_content.strip():
                    client_message = "上游服务返回了空响应"
                elif raw_content.strip().startswith(("<", "<!doctype", "<!DOCTYPE")):
                    client_message = "上游服务返回了非预期的响应格式"
                else:
                    client_message = "上游服务返回了无效的响应"
                raise ProviderNotAvailableException(
                    client_message,
                    provider_name=str(provider.name),
                    upstream_status=resp.status_code,
                    upstream_response=raw_content,
                )
            return response_json if isinstance(response_json, dict) else {}

        try:
            # 解析能力需求
            capability_requirements = self._resolve_capability_requirements(
                model_name=model,
                request_headers=original_headers,
            )

            (
                result,
                actual_provider_name,
                attempt_id,
                provider_id,
                endpoint_id,
                key_id,
            ) = await self.orchestrator.execute_with_fallback(
                api_format=api_format,
                model_name=model,
                user_api_key=self.api_key,
                request_func=sync_request_func,
                request_id=self.request_id,
                capability_requirements=capability_requirements or None,
            )

            provider_name = actual_provider_name
            response_time_ms = self.elapsed_ms()

            # 确保 response_json 不为 None
            if response_json is None:
                response_json = {}

            # 规范化响应
            response_json = self._normalize_response(response_json)

            # 提取 usage
            usage_info = self._extract_usage(response_json)
            input_tokens = usage_info.get("input_tokens", 0)
            output_tokens = usage_info.get("output_tokens", 0)
            cache_creation_tokens = usage_info.get("cache_creation_input_tokens", 0)
            cached_tokens = usage_info.get("cache_read_input_tokens", 0)

            actual_request_body = provider_request_body or original_request_body

            # 非流式成功时，返回给客户端的是提供商响应头（透传）
            # JSONResponse 会自动设置 content-type，但我们记录实际返回的完整头
            client_response_headers = dict(response_headers) if response_headers else {}
            client_response_headers["content-type"] = "application/json"

            total_cost = await self.telemetry.record_success(
                provider=provider_name,
                model=model,
                input_tokens=input_tokens,
                output_tokens=output_tokens,
                response_time_ms=response_time_ms,
                status_code=status_code,
                request_headers=original_headers,
                request_body=actual_request_body,
                response_headers=response_headers,
                client_response_headers=client_response_headers,
                response_body=response_json,
                cache_creation_tokens=cache_creation_tokens,
                cache_read_tokens=cached_tokens,
                is_stream=False,
                provider_request_headers=provider_request_headers,
                api_format=api_format,
                provider_id=provider_id,
                provider_endpoint_id=endpoint_id,
                provider_api_key_id=key_id,
                # 模型映射信息
                target_model=mapped_model_result,
            )

            logger.debug(f"{self.FORMAT_ID} 非流式响应完成")

            # 简洁的请求完成摘要
            logger.info(
                f"[OK] {self.request_id[:8]} | {model} | {provider_name or 'unknown'} | {response_time_ms}ms | "
                f"in:{input_tokens or 0} out:{output_tokens or 0}"
            )

            # 透传提供商的响应头
            return JSONResponse(
                status_code=status_code,
                content=response_json,
                headers=response_headers if response_headers else None,
            )

        except Exception as e:
            response_time_ms = self.elapsed_ms()

            status_code = 503
            if isinstance(e, ProviderAuthException):
                status_code = 503
            elif isinstance(e, ProviderRateLimitException):
                status_code = 429
            elif isinstance(e, ProviderTimeoutException):
                status_code = 504

            actual_request_body = provider_request_body or original_request_body

            # 尝试从异常中提取响应头
            error_response_headers: Dict[str, str] = {}
            if isinstance(e, ProviderRateLimitException) and e.response_headers:
                error_response_headers = e.response_headers
            elif isinstance(e, httpx.HTTPStatusError) and hasattr(e, "response"):
                error_response_headers = dict(e.response.headers)

            await self.telemetry.record_failure(
                provider=provider_name or "unknown",
                model=model,
                response_time_ms=response_time_ms,
                status_code=status_code,
                error_message=extract_client_error_message(e),
                request_headers=original_headers,
                request_body=actual_request_body,
                is_stream=False,
                api_format=api_format,
                provider_request_headers=provider_request_headers,
                response_headers=error_response_headers,
                # 非流式失败返回给客户端的是 JSON 错误响应
                client_response_headers={"content-type": "application/json"},
                # 模型映射信息
                target_model=mapped_model_result,
            )

            raise

    async def _extract_error_text(self, e: httpx.HTTPStatusError) -> str:
        """从 HTTP 错误中提取错误文本"""
        try:
            if hasattr(e.response, "is_stream_consumed") and not e.response.is_stream_consumed:
                error_bytes = await e.response.aread()
                return error_bytes.decode("utf-8", errors="replace")
            else:
                return e.response.text if hasattr(e.response, "_content") else "Unable to read"
        except Exception as decode_error:
            return f"Unable to read error: {decode_error}"
