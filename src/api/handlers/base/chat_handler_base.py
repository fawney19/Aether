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
"""

import asyncio
import json
from abc import ABC, abstractmethod
from typing import Any, AsyncGenerator, Callable, Dict, Optional

import httpx
from fastapi import BackgroundTasks, Request
from fastapi.responses import JSONResponse, StreamingResponse
from sqlalchemy.orm import Session

from src.api.handlers.base.base_handler import (
    BaseMessageHandler,
    MessageTelemetry,
)
from src.api.handlers.base.parsers import get_parser_for_format
from src.api.handlers.base.request_builder import PassthroughRequestBuilder
from src.api.handlers.base.response_parser import ResponseParser
from src.core.exceptions import (
    EmbeddedErrorException,
    ProviderAuthException,
    ProviderNotAvailableException,
    ProviderRateLimitException,
    ProviderTimeoutException,
)
from src.core.logger import logger
from src.database import get_db
from src.models.database import (
    ApiKey,
    Provider,
    ProviderAPIKey,
    ProviderEndpoint,
    User,
)
from src.services.provider.transport import build_provider_url
from src.utils.sse_parser import SSEEventParser



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
        response_normalizer: Optional[Any] = None,
        enable_response_normalization: bool = False,
        adapter_detector: Optional[Callable[[Dict[str, str], Optional[Dict[str, Any]]], Dict[str, bool]]] = None,
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
        self.response_normalizer = response_normalizer
        self.enable_response_normalization = enable_response_normalization

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
            mapped_name = str(mapping.model.provider_model_name)
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

        # 用于跟踪的上下文
        ctx = {
            "model": model,
            "api_format": api_format,
            "provider_name": None,
            "provider_id": None,
            "endpoint_id": None,
            "key_id": None,
            "attempt_id": None,
            "provider_api_format": None,  # Provider 的响应格式（用于选择解析器）
            "input_tokens": 0,
            "output_tokens": 0,
            "cached_tokens": 0,
            "cache_creation_tokens": 0,
            "collected_text": "",
            "status_code": 200,
            "response_headers": {},
            "provider_request_headers": {},
            "provider_request_body": None,
            "data_count": 0,
            "chunk_count": 0,
            "has_completion": False,
            "parsed_chunks": [],  # 收集解析后的 chunks
        }

        # 定义请求函数
        async def stream_request_func(
            provider: Provider,
            endpoint: ProviderEndpoint,
            key: ProviderAPIKey,
        ) -> AsyncGenerator[bytes, None]:
            return await self._execute_stream_request(
                ctx,
                provider,
                endpoint,
                key,
                original_request_body,
                original_headers,
                query_params,
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
            ctx["attempt_id"] = attempt_id
            ctx["provider_name"] = provider_name
            ctx["provider_id"] = provider_id
            ctx["endpoint_id"] = endpoint_id
            ctx["key_id"] = key_id

            # 创建后台任务记录统计
            background_tasks = BackgroundTasks()
            background_tasks.add_task(
                self._record_stream_stats,
                ctx,
                original_headers,
                original_request_body,
            )

            # 创建监控流
            monitored_stream = self._create_monitored_stream(ctx, stream_generator, http_request)

            return StreamingResponse(
                monitored_stream,
                media_type="text/event-stream",
                background=background_tasks,
            )

        except Exception as e:
            logger.exception(f"流式请求失败: {e}")
            await self._record_stream_failure(ctx, e, original_headers, original_request_body)
            raise

    async def _execute_stream_request(
        self,
        ctx: Dict,
        provider: Provider,
        endpoint: ProviderEndpoint,
        key: ProviderAPIKey,
        original_request_body: Dict[str, Any],
        original_headers: Dict[str, str],
        query_params: Optional[Dict[str, str]] = None,
    ) -> AsyncGenerator[bytes, None]:
        """执行流式请求并返回流生成器"""
        # 重置上下文状态（重试时清除之前的数据，避免累积）
        ctx["parsed_chunks"] = []
        ctx["chunk_count"] = 0
        ctx["data_count"] = 0
        ctx["has_completion"] = False
        ctx["collected_text"] = ""
        ctx["input_tokens"] = 0
        ctx["output_tokens"] = 0
        ctx["cached_tokens"] = 0
        ctx["cache_creation_tokens"] = 0

        ctx["provider_name"] = str(provider.name)
        ctx["provider_id"] = str(provider.id)
        ctx["endpoint_id"] = str(endpoint.id)
        ctx["key_id"] = str(key.id)
        ctx["provider_api_format"] = str(endpoint.api_format) if endpoint.api_format else ""

        # 获取模型映射
        mapped_model = await self._get_mapped_model(
            source_model=ctx["model"],
            provider_id=str(provider.id),
        )

        # 应用模型映射到请求体
        if mapped_model:
            ctx["mapped_model"] = mapped_model  # 保存映射后的模型名，用于 Usage 记录
            request_body = self.apply_mapped_model(original_request_body, mapped_model)
        else:
            request_body = dict(original_request_body)

        # 准备发送给 Provider 的请求体（子类可覆盖以移除不需要的字段）
        request_body = self.prepare_provider_request_body(request_body)

        # 构建请求
        provider_payload, provider_headers = self._request_builder.build(
            request_body,
            original_headers,
            endpoint,
            key,
            is_stream=True,
        )

        ctx["provider_request_headers"] = provider_headers
        ctx["provider_request_body"] = provider_payload

        # 获取 URL 模型名（兜底使用 ctx 中的 model，确保 Gemini 等格式能正确构建 URL）
        url_model = self.get_model_for_url(request_body, mapped_model) or ctx["model"]

        url = build_provider_url(
            endpoint,
            query_params=query_params,
            path_params={"model": url_model},
            is_stream=True,
        )

        logger.debug(f"  [{self.request_id}] 发送流式请求: Provider={provider.name}, "
            f"模型={ctx['model']} -> {mapped_model or '无映射'}")

        # 发送请求
        timeout_config = httpx.Timeout(
            connect=10.0,
            read=float(endpoint.timeout),
            write=10.0,
            pool=10.0,
        )

        http_client = httpx.AsyncClient(timeout=timeout_config, follow_redirects=True)
        try:
            response_ctx = http_client.stream(
                "POST", url, json=provider_payload, headers=provider_headers
            )
            stream_response = await response_ctx.__aenter__()

            ctx["status_code"] = stream_response.status_code
            ctx["response_headers"] = dict(stream_response.headers)

            stream_response.raise_for_status()

            # 创建行迭代器（只创建一次，后续会继续使用）
            line_iterator = stream_response.aiter_lines()

            # 预读第一个数据块，检测嵌套错误（HTTP 200 但响应体包含错误）
            prefetched_lines = await self._prefetch_and_check_embedded_error(
                line_iterator, provider, endpoint, ctx
            )

        except httpx.HTTPStatusError as e:
            error_text = await self._extract_error_text(e)
            logger.error(f"Provider 返回错误: {e.response.status_code}\n  Response: {error_text}")
            await http_client.aclose()
            raise

        except EmbeddedErrorException:
            # 嵌套错误需要触发重试，关闭连接后重新抛出
            try:
                await response_ctx.__aexit__(None, None, None)
            except Exception:
                pass
            await http_client.aclose()
            raise

        except Exception:
            await http_client.aclose()
            raise

        # 创建流生成器（带预读数据，使用同一个迭代器）
        return self._create_response_stream_with_prefetch(
            ctx,
            line_iterator,
            response_ctx,
            http_client,
            prefetched_lines,
        )

    async def _create_response_stream(
        self,
        ctx: Dict,
        stream_response: httpx.Response,
        response_ctx: Any,
        http_client: httpx.AsyncClient,
    ) -> AsyncGenerator[bytes, None]:
        """创建响应流生成器"""
        try:
            sse_parser = SSEEventParser()
            streaming_status_updated = False

            async for line in stream_response.aiter_lines():
                # 在第一次输出数据前更新状态为 streaming
                if not streaming_status_updated:
                    self._update_usage_to_streaming()
                    streaming_status_updated = True

                normalized_line = line.rstrip("\r")
                events = sse_parser.feed_line(normalized_line)

                if normalized_line == "":
                    for event in events:
                        self._handle_sse_event(ctx, event.get("event"), event.get("data") or "")
                    yield b"\n"
                    continue

                ctx["chunk_count"] += 1

                yield (line + "\n").encode("utf-8")

                for event in events:
                    self._handle_sse_event(ctx, event.get("event"), event.get("data") or "")

            # 处理剩余事件
            for event in sse_parser.flush():
                self._handle_sse_event(ctx, event.get("event"), event.get("data") or "")

        except GeneratorExit:
            raise
        finally:
            try:
                await response_ctx.__aexit__(None, None, None)
            except Exception:
                pass
            try:
                await http_client.aclose()
            except Exception:
                pass

    async def _prefetch_and_check_embedded_error(
        self,
        line_iterator: Any,
        provider: Provider,
        endpoint: ProviderEndpoint,
        ctx: Dict,
    ) -> list:
        """
        预读流的前几行，检测嵌套错误

        某些 Provider（如 Gemini）可能返回 HTTP 200，但在响应体中包含错误信息。
        这种情况需要在流开始输出之前检测，以便触发重试逻辑。

        Args:
            line_iterator: 行迭代器（aiter_lines() 返回的迭代器）
            provider: Provider 对象
            endpoint: Endpoint 对象
            ctx: 上下文字典

        Returns:
            预读的行列表（需要在后续流中先输出）

        Raises:
            EmbeddedErrorException: 如果检测到嵌套错误
        """
        prefetched_lines: list = []
        max_prefetch_lines = 5  # 最多预读5行来检测错误

        try:
            # 获取对应格式的解析器
            provider_parser = self._get_provider_parser(ctx)

            line_count = 0
            async for line in line_iterator:
                prefetched_lines.append(line)
                line_count += 1

                # 解析数据
                normalized_line = line.rstrip("\r")
                if not normalized_line or normalized_line.startswith(":"):
                    # 空行或注释行，继续预读
                    if line_count >= max_prefetch_lines:
                        break
                    continue

                # 尝试解析 SSE 数据
                data_str = normalized_line
                if normalized_line.startswith("data: "):
                    data_str = normalized_line[6:]

                if data_str == "[DONE]":
                    break

                try:
                    data = json.loads(data_str)
                except json.JSONDecodeError:
                    # 不是有效 JSON，可能是部分数据，继续
                    if line_count >= max_prefetch_lines:
                        break
                    continue

                # 使用解析器检查是否为错误响应
                if isinstance(data, dict) and provider_parser.is_error_response(data):
                    # 提取错误信息
                    parsed = provider_parser.parse_response(data, 200)
                    logger.warning(f"  [{self.request_id}] 检测到嵌套错误: "
                        f"Provider={provider.name}, "
                        f"error_type={parsed.error_type}, "
                        f"message={parsed.error_message}")
                    raise EmbeddedErrorException(
                        provider_name=str(provider.name),
                        error_code=(
                            int(parsed.error_type)
                            if parsed.error_type and parsed.error_type.isdigit()
                            else None
                        ),
                        error_message=parsed.error_message,
                        error_status=parsed.error_type,
                    )

                # 预读到有效数据，没有错误，停止预读
                break

        except EmbeddedErrorException:
            # 重新抛出嵌套错误
            raise
        except Exception as e:
            # 其他异常（如网络错误）在预读阶段发生，记录日志但不中断
            logger.debug(f"  [{self.request_id}] 预读流时发生异常: {e}")

        return prefetched_lines

    async def _create_response_stream_with_prefetch(
        self,
        ctx: Dict,
        line_iterator: Any,
        response_ctx: Any,
        http_client: httpx.AsyncClient,
        prefetched_lines: list,
    ) -> AsyncGenerator[bytes, None]:
        """创建响应流生成器（带预读数据）"""
        try:
            sse_parser = SSEEventParser()

            # 在第一次输出数据前更新状态为 streaming
            if prefetched_lines:
                self._update_usage_to_streaming()

            # 先输出预读的数据
            for line in prefetched_lines:
                normalized_line = line.rstrip("\r")
                events = sse_parser.feed_line(normalized_line)

                if normalized_line == "":
                    for event in events:
                        self._handle_sse_event(ctx, event.get("event"), event.get("data") or "")
                    yield b"\n"
                    continue

                ctx["chunk_count"] += 1
                yield (line + "\n").encode("utf-8")

                for event in events:
                    self._handle_sse_event(ctx, event.get("event"), event.get("data") or "")

            # 继续输出剩余的流数据（使用同一个迭代器）
            async for line in line_iterator:
                normalized_line = line.rstrip("\r")
                events = sse_parser.feed_line(normalized_line)

                if normalized_line == "":
                    for event in events:
                        self._handle_sse_event(ctx, event.get("event"), event.get("data") or "")
                    yield b"\n"
                    continue

                ctx["chunk_count"] += 1
                yield (line + "\n").encode("utf-8")

                for event in events:
                    self._handle_sse_event(ctx, event.get("event"), event.get("data") or "")

            # 处理剩余事件
            for event in sse_parser.flush():
                self._handle_sse_event(ctx, event.get("event"), event.get("data") or "")

        except GeneratorExit:
            raise
        finally:
            try:
                await response_ctx.__aexit__(None, None, None)
            except Exception:
                pass
            try:
                await http_client.aclose()
            except Exception:
                pass

    def _get_provider_parser(self, ctx: Dict) -> ResponseParser:
        """
        获取 Provider 格式的解析器

        根据 Provider 的 API 格式选择正确的解析器，
        而不是根据请求格式选择。
        """
        provider_format = ctx.get("provider_api_format")
        if provider_format:
            try:
                return get_parser_for_format(provider_format)
            except KeyError:
                pass
        # 回退到默认解析器
        return self.parser

    def _handle_sse_event(
        self,
        ctx: Dict,
        event_name: Optional[str],
        data_str: str,
    ) -> None:
        """处理 SSE 事件"""
        if not data_str:
            return

        if data_str == "[DONE]":
            ctx["has_completion"] = True
            return

        try:
            data = json.loads(data_str)
        except json.JSONDecodeError:
            return

        ctx["data_count"] += 1

        if not isinstance(data, dict):
            return

        # 收集原始 chunk 数据
        ctx["parsed_chunks"].append(data)

        # 根据 Provider 格式选择解析器
        provider_parser = self._get_provider_parser(ctx)

        # 使用解析器提取 usage
        usage = provider_parser.extract_usage_from_response(data)
        if usage:
            ctx["input_tokens"] = usage.get("input_tokens", ctx["input_tokens"])
            ctx["output_tokens"] = usage.get("output_tokens", ctx["output_tokens"])
            ctx["cached_tokens"] = usage.get("cache_read_tokens", ctx["cached_tokens"])
            ctx["cache_creation_tokens"] = usage.get(
                "cache_creation_tokens", ctx["cache_creation_tokens"]
            )

        # 提取文本
        text = provider_parser.extract_text_content(data)
        if text:
            ctx["collected_text"] += text

        # 检查完成
        event_type = event_name or data.get("type", "")
        if event_type in ("response.completed", "message_stop"):
            ctx["has_completion"] = True

    async def _create_monitored_stream(
        self,
        ctx: Dict,
        stream_generator: AsyncGenerator[bytes, None],
        http_request: Request,
    ) -> AsyncGenerator[bytes, None]:
        """创建带监控的流生成器"""
        try:
            async for chunk in stream_generator:
                if await http_request.is_disconnected():
                    logger.warning(f"ID:{self.request_id} | Client disconnected")
                    # 客户端断开时设置 499 状态码（Client Closed Request）
                    # 注意：Provider 可能已经成功返回数据，但客户端未完整接收
                    ctx["status_code"] = 499
                    break
                yield chunk
        except asyncio.CancelledError:
            ctx["status_code"] = 499
            raise
        except Exception:
            ctx["status_code"] = 500
            raise

    async def _record_stream_stats(
        self,
        ctx: Dict,
        original_headers: Dict[str, str],
        original_request_body: Dict[str, Any],
    ) -> None:
        """记录流式统计信息"""
        response_time_ms = self.elapsed_ms()
        bg_db = None

        try:
            await asyncio.sleep(0.1)

            if not ctx["provider_name"]:
                # 即使没有 provider_name，也要尝试更新状态为 failed
                await self._update_usage_status_on_error(
                    response_time_ms=response_time_ms,
                    error_message="Provider name not available",
                )
                return

            db_gen = get_db()
            bg_db = next(db_gen)

            try:
                from src.models.database import ApiKey as ApiKeyModel

                user = bg_db.query(User).filter(User.id == self.user.id).first()
                api_key_obj = (
                    bg_db.query(ApiKeyModel).filter(ApiKeyModel.id == self.api_key.id).first()
                )

                if not user or not api_key_obj:
                    logger.warning(f"[{self.request_id}] User or ApiKey not found, updating status directly")
                    await self._update_usage_status_directly(
                        bg_db,
                        status="completed" if ctx["status_code"] == 200 else "failed",
                        response_time_ms=response_time_ms,
                        status_code=ctx["status_code"],
                    )
                    return

                bg_telemetry = MessageTelemetry(
                    bg_db, user, api_key_obj, self.request_id, self.client_ip
                )

                actual_request_body = ctx["provider_request_body"] or original_request_body

                # 构建响应体（与 CLI 模式一致）
                response_body = {
                    "chunks": ctx["parsed_chunks"],
                    "metadata": {
                        "stream": True,
                        "total_chunks": len(ctx["parsed_chunks"]),
                        "data_count": ctx["data_count"],
                        "has_completion": ctx["has_completion"],
                        "response_time_ms": response_time_ms,
                    },
                }

                # 根据状态码决定记录成功还是失败
                # 499 = 客户端断开连接，503 = 服务不可用（如流中断）
                status_code: int = ctx.get("status_code") or 200
                if status_code >= 400:
                    # 记录失败的 Usage，但使用已收到的预估 token 信息（来自 message_start）
                    # 这样即使请求中断，也能记录预估成本
                    await bg_telemetry.record_failure(
                        provider=ctx.get("provider_name") or "unknown",
                        model=ctx["model"],
                        response_time_ms=response_time_ms,
                        status_code=status_code,
                        error_message=ctx.get("error_message") or f"HTTP {status_code}",
                        request_headers=original_headers,
                        request_body=actual_request_body,
                        is_stream=True,
                        api_format=ctx["api_format"],
                        provider_request_headers=ctx["provider_request_headers"],
                        # 预估 token 信息（来自 message_start 事件）
                        input_tokens=ctx.get("input_tokens", 0),
                        output_tokens=ctx.get("output_tokens", 0),
                        cache_creation_tokens=ctx.get("cache_creation_tokens", 0),
                        cache_read_tokens=ctx.get("cached_tokens", 0),
                        response_body=response_body,
                        # 模型映射信息
                        target_model=ctx.get("mapped_model"),
                    )
                    logger.debug(f"{self.FORMAT_ID} 流式响应中断")
                    # 简洁的请求失败摘要（包含预估 token 信息）
                    logger.info(f"[FAIL] {self.request_id[:8]} | {ctx['model']} | {ctx.get('provider_name', 'unknown')} | {response_time_ms}ms | "
                        f"{status_code} | in:{ctx.get('input_tokens', 0)} out:{ctx.get('output_tokens', 0)} cache:{ctx.get('cached_tokens', 0)}")
                else:
                    await bg_telemetry.record_success(
                        provider=ctx["provider_name"],
                        model=ctx["model"],
                        input_tokens=ctx["input_tokens"],
                        output_tokens=ctx["output_tokens"],
                        response_time_ms=response_time_ms,
                        status_code=status_code,
                        request_headers=original_headers,
                        request_body=actual_request_body,
                        response_headers=ctx["response_headers"],
                        response_body=response_body,
                        cache_creation_tokens=ctx["cache_creation_tokens"],
                        cache_read_tokens=ctx["cached_tokens"],
                        is_stream=True,
                        provider_request_headers=ctx["provider_request_headers"],
                        api_format=ctx["api_format"],
                        provider_id=ctx["provider_id"],
                        provider_endpoint_id=ctx["endpoint_id"],
                        provider_api_key_id=ctx["key_id"],
                        # 模型映射信息
                        target_model=ctx.get("mapped_model"),
                    )
                    logger.debug(f"{self.FORMAT_ID} 流式响应完成")
                    # 简洁的请求完成摘要
                    logger.info(f"[OK] {self.request_id[:8]} | {ctx['model']} | {ctx.get('provider_name', 'unknown')} | {response_time_ms}ms | "
                        f"in:{ctx.get('input_tokens', 0) or 0} out:{ctx.get('output_tokens', 0) or 0}")

                # 更新候选记录的最终状态和延迟时间
                # 注意：RequestExecutor 会在流开始时过早地标记成功（只记录了连接建立的时间）
                # 这里用流传输完成后的实际时间覆盖
                if ctx.get("attempt_id"):
                    from src.services.request.candidate import RequestCandidateService

                    # 根据状态码决定是成功还是失败（复用上面已定义的 status_code）
                    # 499 = 客户端断开连接，应标记为失败
                    # 503 = 服务不可用（如流中断），应标记为失败
                    if status_code and status_code >= 400:
                        RequestCandidateService.mark_candidate_failed(
                            db=bg_db,
                            candidate_id=ctx["attempt_id"],
                            error_type="client_disconnected" if status_code == 499 else "stream_error",
                            error_message=ctx.get("error_message") or f"HTTP {status_code}",
                            status_code=status_code,
                            latency_ms=response_time_ms,
                            extra_data={
                                "stream_completed": False,
                                "data_count": ctx.get("data_count", 0),
                            },
                        )
                    else:
                        RequestCandidateService.mark_candidate_success(
                            db=bg_db,
                            candidate_id=ctx["attempt_id"],
                            status_code=status_code,
                            latency_ms=response_time_ms,
                            extra_data={
                                "stream_completed": True,
                                "data_count": ctx.get("data_count", 0),
                            },
                        )

            finally:
                if bg_db:
                    bg_db.close()

        except Exception as e:
            logger.exception("记录流式统计信息时出错")
            # 确保即使出错也要更新状态，避免 pending 状态卡住
            await self._update_usage_status_on_error(
                response_time_ms=response_time_ms,
                error_message=f"记录统计信息失败: {str(e)[:200]}",
            )

    # _update_usage_to_streaming 方法已移至基类 BaseMessageHandler

    async def _update_usage_status_on_error(
        self,
        response_time_ms: int,
        error_message: str,
    ) -> None:
        """在记录失败时更新 Usage 状态，避免卡在 pending"""
        try:
            db_gen = get_db()
            error_db = next(db_gen)
            try:
                await self._update_usage_status_directly(
                    error_db,
                    status="failed",
                    response_time_ms=response_time_ms,
                    status_code=500,
                    error_message=error_message,
                )
            finally:
                error_db.close()
        except Exception as inner_e:
            logger.error(f"[{self.request_id}] 更新 Usage 状态失败: {inner_e}")

    async def _update_usage_status_directly(
        self,
        db: Session,
        status: str,
        response_time_ms: int,
        status_code: int = 200,
        error_message: Optional[str] = None,
    ) -> None:
        """直接更新 Usage 表的状态字段"""
        try:
            from src.models.database import Usage

            usage = db.query(Usage).filter(Usage.request_id == self.request_id).first()
            if usage:
                setattr(usage, "status", status)
                setattr(usage, "status_code", status_code)
                setattr(usage, "response_time_ms", response_time_ms)
                if error_message:
                    setattr(usage, "error_message", error_message)
                db.commit()
                logger.debug(f"[{self.request_id}] Usage 状态已更新: {status}")
        except Exception as e:
            logger.error(f"[{self.request_id}] 直接更新 Usage 状态失败: {e}")

    async def _record_stream_failure(
        self,
        ctx: Dict,
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

        actual_request_body = ctx.get("provider_request_body") or original_request_body

        await self.telemetry.record_failure(
            provider=ctx.get("provider_name") or "unknown",
            model=ctx["model"],
            response_time_ms=response_time_ms,
            status_code=status_code,
            error_message=str(error),
            request_headers=original_headers,
            request_body=actual_request_body,
            is_stream=True,
            api_format=ctx["api_format"],
            provider_request_headers=ctx.get("provider_request_headers") or {},
            # 模型映射信息
            target_model=ctx.get("mapped_model"),
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
        ) -> Dict[str, Any]:
            nonlocal provider_name, response_json, status_code, response_headers
            nonlocal provider_request_headers, provider_request_body, mapped_model_result

            provider_name = str(provider.name)

            # 获取模型映射
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

            logger.info(f"  [{self.request_id}] 发送非流式请求: Provider={provider.name}, "
                f"模型={model} -> {mapped_model or '无映射'}")

            async with httpx.AsyncClient(
                timeout=float(endpoint.timeout),
                follow_redirects=True,
            ) as http_client:
                resp = await http_client.post(url, json=provider_payload, headers=provider_hdrs)

                status_code = resp.status_code
                response_headers = dict(resp.headers)

                if resp.status_code == 401:
                    raise ProviderAuthException(f"提供商认证失败: {provider.name}")
                elif resp.status_code == 429:
                    raise ProviderRateLimitException(
                        f"提供商速率限制: {provider.name}",
                        provider_name=str(provider.name),
                        response_headers=response_headers,
                    )
                elif resp.status_code >= 500:
                    raise ProviderNotAvailableException(f"提供商服务不可用: {provider.name}")
                elif resp.status_code != 200:
                    raise ProviderNotAvailableException(
                        f"提供商返回错误: {provider.name}, 状态: {resp.status_code}"
                    )

                response_json = resp.json()
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
            logger.info(f"[OK] {self.request_id[:8]} | {model} | {provider_name or 'unknown'} | {response_time_ms}ms | "
                f"in:{input_tokens or 0} out:{output_tokens or 0}")

            return JSONResponse(status_code=status_code, content=response_json)

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

            await self.telemetry.record_failure(
                provider=provider_name or "unknown",
                model=model,
                response_time_ms=response_time_ms,
                status_code=status_code,
                error_message=str(e),
                request_headers=original_headers,
                request_body=actual_request_body,
                is_stream=False,
                api_format=api_format,
                provider_request_headers=provider_request_headers,
                # 模型映射信息
                target_model=mapped_model_result,
            )

            raise

    async def _extract_error_text(self, e: httpx.HTTPStatusError) -> str:
        """从 HTTP 错误中提取错误文本"""
        try:
            if hasattr(e.response, "is_stream_consumed") and not e.response.is_stream_consumed:
                error_bytes = await e.response.aread()
                return error_bytes.decode("utf-8", errors="replace")[:500]
            else:
                return (
                    e.response.text[:500] if hasattr(e.response, "_content") else "Unable to read"
                )
        except Exception as decode_error:
            return f"Unable to read error: {decode_error}"
