"""
CLI Message Handler 通用基类

将 CLI 格式处理器的通用逻辑（HTTP 请求、SSE 解析、统计记录）抽取到基类，
子类只需实现格式特定的事件解析逻辑。

设计目标：
1. 减少代码重复 - 原来每个 CLI Handler 900+ 行，抽取后子类只需 ~100 行
2. 统一错误处理 - 超时、空流、故障转移等逻辑集中管理
3. 简化新格式接入 - 只需实现 ResponseParser 和少量钩子方法
"""

import asyncio
import codecs
import json
import time
from typing import (
    Any,
    AsyncGenerator,
    Callable,
    Dict,
    Optional,
)

import httpx
from fastapi import BackgroundTasks
from fastapi.responses import JSONResponse, StreamingResponse
from sqlalchemy.orm import Session

from src.api.handlers.base.base_handler import (
    BaseMessageHandler,
    MessageTelemetry,
)
from src.api.handlers.base.parsers import get_parser_for_format
from src.api.handlers.base.request_builder import PassthroughRequestBuilder

# 直接从具体模块导入，避免循环依赖
from src.api.handlers.base.response_parser import (
    ResponseParser,
    StreamStats,
)
from src.api.handlers.base.stream_context import StreamContext
from src.api.handlers.base.utils import (
    build_sse_headers,
    check_html_response,
    check_prefetched_response_error,
)
from src.config.constants import StreamDefaults
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
from src.database import get_db
from src.models.database import (
    ApiKey,
    Provider,
    ProviderAPIKey,
    ProviderEndpoint,
    User,
)
from src.services.cache.aware_scheduler import ProviderCandidate
from src.services.provider.transport import build_provider_url
from src.utils.sse_parser import SSEEventParser
from src.utils.timeout import read_first_chunk_with_ttfb_timeout


class CliMessageHandlerBase(BaseMessageHandler):
    """
    CLI 格式消息处理器基类

    提供 CLI 格式（直接透传请求）的通用处理逻辑：
    - 流式请求的 HTTP 连接管理
    - SSE 事件解析框架
    - 统计信息收集和记录
    - 错误处理和故障转移

    子类需要实现：
    - get_response_parser(): 返回格式特定的响应解析器
    - 可选覆盖 handle_sse_event() 自定义事件处理

    """

    # 子类可覆盖的配置
    FORMAT_ID: str = "UNKNOWN"  # API 格式标识
    DATA_TIMEOUT: int = 30  # 流数据超时时间（秒）
    EMPTY_CHUNK_THRESHOLD: int = 10  # 空流检测的 chunk 阈值

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
            self._parser = self.get_response_parser()
        return self._parser

    def get_response_parser(self) -> ResponseParser:
        """
        获取格式特定的响应解析器

        子类可覆盖此方法提供自定义解析器，
        默认从解析器注册表获取
        """
        return get_parser_for_format(self.FORMAT_ID)

    async def _get_mapped_model(
        self,
        source_model: str,
        provider_id: str,
    ) -> Optional[str]:
        """
        获取模型映射后的实际模型名

        查找逻辑：
        1. 直接通过 GlobalModel.name 匹配
        2. 查找该 Provider 的 Model 实现
        3. 使用 provider_model_name / provider_model_mappings 选择最终名称

        Args:
            source_model: 用户请求的模型名（必须是 GlobalModel.name）
            provider_id: Provider ID

        Returns:
            映射后的 Provider 模型名，如果没有找到映射则返回 None
        """
        from src.services.model.mapper import ModelMapperMiddleware

        mapper = ModelMapperMiddleware(self.db)
        mapping = await mapper.get_mapping(source_model, provider_id)

        logger.debug(
            f"[CLI] _get_mapped_model: source={source_model}, provider={provider_id[:8]}..., mapping={mapping}"
        )

        if mapping and mapping.model:
            # 使用 select_provider_model_name 支持模型映射功能
            # 传入 api_key.id 作为 affinity_key，实现相同用户稳定选择同一映射
            # 传入 api_format 用于过滤适用的映射作用域
            affinity_key = self.api_key.id if self.api_key else None
            mapped_name = mapping.model.select_provider_model_name(
                affinity_key, api_format=self.FORMAT_ID
            )
            logger.debug(
                f"[CLI] 模型映射: {source_model} -> {mapped_name} (provider={provider_id[:8]}...)"
            )
            return mapped_name

        logger.debug(f"[CLI] 无模型映射，使用原始名称: {source_model}")
        return None

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
            用于 URL 路径的模型名，默认优先使用映射后的名称
        """
        return mapped_model or request_body.get("model")

    def _extract_response_metadata(
        self,
        response: Dict[str, Any],
    ) -> Dict[str, Any]:
        """
        从响应中提取 Provider 特有的元数据 - 子类可覆盖

        例如 Gemini 返回的 modelVersion 字段。
        这些元数据会存储到 Usage.request_metadata 中。

        Args:
            response: Provider 返回的响应

        Returns:
            元数据字典，默认为空
        """
        return {}

    async def process_stream(
        self,
        original_request_body: Dict[str, Any],
        original_headers: Dict[str, str],
        query_params: Optional[Dict[str, str]] = None,
        path_params: Optional[Dict[str, Any]] = None,
    ) -> StreamingResponse:
        """
        处理流式请求

        通用流程：
        1. 创建流上下文
        2. 定义请求函数（供 FallbackOrchestrator 调用）
        3. 执行请求并返回 StreamingResponse
        4. 后台任务记录统计信息
        """
        logger.debug(f"开始流式响应处理 ({self.FORMAT_ID})")

        # 使用子类实现的方法提取 model（不同 API 格式的 model 位置不同）
        model = self.extract_model_from_request(original_request_body, path_params)

        # 创建流上下文
        ctx = StreamContext(
            model=model,
            api_format=self.allowed_api_formats[0],
            request_id=self.request_id,
            user_id=self.user.id,
            api_key_id=self.api_key.id,
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
                model_name=ctx.model,
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
                api_format=ctx.api_format,
                model_name=ctx.model,
                user_api_key=self.api_key,
                request_func=stream_request_func,
                request_id=self.request_id,
                is_stream=True,
                capability_requirements=capability_requirements or None,
            )

            # 更新上下文（确保 provider 信息已设置，用于 streaming 状态更新）
            ctx.attempt_id = attempt_id
            if not ctx.provider_name:
                ctx.provider_name = provider_name
            if not ctx.provider_id:
                ctx.provider_id = provider_id
            if not ctx.endpoint_id:
                ctx.endpoint_id = endpoint_id
            if not ctx.key_id:
                ctx.key_id = key_id

            # 创建后台任务记录统计
            background_tasks = BackgroundTasks()
            background_tasks.add_task(
                self._record_stream_stats,
                ctx,
                original_headers,
                original_request_body,
            )

            # 创建监控流
            monitored_stream = self._create_monitored_stream(ctx, stream_generator)

            # 透传提供商的响应头给客户端
            # 同时添加必要的 SSE 头以确保流式传输正常工作
            client_headers = dict(ctx.response_headers) if ctx.response_headers else {}
            # 添加/覆盖 SSE 必需的头
            client_headers.update(build_sse_headers())
            client_headers["content-type"] = "text/event-stream"
            ctx.client_response_headers = client_headers

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
        provider: Provider,
        endpoint: ProviderEndpoint,
        key: ProviderAPIKey,
        original_request_body: Dict[str, Any],
        original_headers: Dict[str, str],
        query_params: Optional[Dict[str, str]] = None,
        candidate: Optional[ProviderCandidate] = None,
    ) -> AsyncGenerator[bytes, None]:
        """执行流式请求并返回流生成器"""
        # 重置上下文状态（重试时清除之前的数据，避免累积）
        ctx.parsed_chunks = []
        ctx.chunk_count = 0
        ctx.data_count = 0
        ctx.has_completion = False
        ctx._collected_text_parts = []  # 重置文本收集
        ctx.input_tokens = 0
        ctx.output_tokens = 0
        ctx.cached_tokens = 0
        ctx.cache_creation_tokens = 0
        ctx.final_usage = None
        ctx.final_response = None
        ctx.response_id = None
        ctx.response_metadata = {}  # 重置 Provider 响应元数据

        # 记录 Provider 信息
        ctx.provider_name = str(provider.name)
        ctx.provider_id = str(provider.id)
        ctx.endpoint_id = str(endpoint.id)
        ctx.key_id = str(key.id)

        # 记录格式转换信息
        ctx.provider_api_format = str(endpoint.api_format) if endpoint.api_format else ""
        ctx.client_api_format = ctx.api_format  # 已在 process_stream 中设置

        # 获取模型映射（优先使用别名匹配到的模型，其次是 Provider 级别的映射）
        mapped_model = candidate.alias_matched_model if candidate else None
        if not mapped_model:
            mapped_model = await self._get_mapped_model(
                source_model=ctx.model,
                provider_id=str(provider.id),
            )

        # 应用模型映射到请求体（子类可覆盖此方法处理不同格式）
        if mapped_model:
            ctx.mapped_model = mapped_model  # 保存映射后的模型名，用于 Usage 记录
            request_body = self.apply_mapped_model(original_request_body, mapped_model)
        else:
            request_body = original_request_body

        # 准备发送给 Provider 的请求体（子类可覆盖以移除不需要的字段）
        request_body = self.prepare_provider_request_body(request_body)

        # 使用 RequestBuilder 构建请求体和请求头
        # 注意：mapped_model 已经应用到 request_body，这里不再传递
        provider_payload, provider_headers = self._request_builder.build(
            request_body,
            original_headers,
            endpoint,
            key,
            is_stream=True,
        )

        # 保存发送给 Provider 的请求信息（用于调试和统计）
        ctx.provider_request_headers = provider_headers
        ctx.provider_request_body = provider_payload

        # 获取用于 URL 的模型名（子类可覆盖此方法，如 Gemini 需要特殊处理）
        # 使用 ctx.model 作为 fallback（它已从 path_params 获取）
        url_model = self.get_model_for_url(request_body, mapped_model) or ctx.model

        url = build_provider_url(
            endpoint,
            query_params=query_params,
            path_params={"model": url_model},
            is_stream=True,  # CLI handler 处理流式请求
        )

        # 配置 HTTP 超时
        # 注意：read timeout 用于检测连接断开，不是整体请求超时
        # 整体请求超时由 _connect_and_prefetch 内部的 asyncio.wait_for 控制
        timeout_config = httpx.Timeout(
            connect=config.http_connect_timeout,
            read=config.http_read_timeout,  # 使用全局配置，用于检测连接断开
            write=config.http_write_timeout,
            pool=config.http_pool_timeout,
        )

        # provider.timeout 作为整体请求超时（建立连接 + 获取首字节）
        request_timeout = float(provider.timeout or 300)

        logger.debug(
            f"  └─ [{self.request_id}] 发送流式请求: "
            f"Provider={provider.name}, Endpoint={endpoint.id[:8] if endpoint.id else 'N/A'}..., "
            f"Key=***{key.api_key[-4:] if key.api_key else 'N/A'}, "
            f"原始模型={ctx.model}, 映射后={mapped_model or '无映射'}, URL模型={url_model}, "
            f"timeout={request_timeout}s"
        )

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

            logger.debug(f"  └─ 收到响应: status={stream_response.status_code}")

            stream_response.raise_for_status()

            # 使用字节流迭代器（避免 aiter_lines 的性能问题, aiter_bytes 会自动解压 gzip/deflate）
            byte_iterator = stream_response.aiter_bytes()

            # 预读第一个数据块，检测嵌套错误（HTTP 200 但响应体包含错误）
            prefetched_chunks = await self._prefetch_and_check_embedded_error(
                byte_iterator, provider, endpoint, ctx
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
            logger.error(
                f"Provider 返回错误状态: {e.response.status_code}\n  Response: {error_text}"
            )
            await http_client.aclose()
            # 将上游错误信息附加到异常，以便故障转移时能够返回给客户端
            e.upstream_response = error_text  # type: ignore[attr-defined]
            raise

        except EmbeddedErrorException:
            # 嵌套错误需要触发重试，关闭连接后重新抛出
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

        # 创建流生成器（带预读数据，使用同一个迭代器）
        return self._create_response_stream_with_prefetch(
            ctx,
            byte_iterator,
            response_ctx,
            http_client,
            prefetched_chunks,
        )

    async def _create_response_stream(
        self,
        ctx: StreamContext,
        stream_response: httpx.Response,
        response_ctx: Any,
        http_client: httpx.AsyncClient,
    ) -> AsyncGenerator[bytes, None]:
        """创建响应流生成器（使用字节流）"""
        try:
            sse_parser = SSEEventParser()
            last_data_time = time.time()
            buffer = b""
            output_state = {"first_yield": True, "streaming_updated": False}
            # 使用增量解码器处理跨 chunk 的 UTF-8 字符
            decoder = codecs.getincrementaldecoder("utf-8")(errors="replace")

            # 检查是否需要格式转换
            needs_conversion = self._needs_format_conversion(ctx)

            async for chunk in stream_response.aiter_bytes():
                buffer += chunk
                # 处理缓冲区中的完整行
                while b"\n" in buffer:
                    line_bytes, buffer = buffer.split(b"\n", 1)
                    try:
                        # 使用增量解码器，可以正确处理跨 chunk 的多字节字符
                        line = decoder.decode(line_bytes + b"\n", False).rstrip("\n")
                    except Exception as e:
                        logger.warning(
                            f"[{self.request_id}] UTF-8 解码失败: {e}, "
                            f"bytes={line_bytes[:50]!r}"
                        )
                        continue

                    normalized_line = line.rstrip("\r")
                    events = sse_parser.feed_line(normalized_line)

                    if normalized_line == "":
                        for event in events:
                            self._handle_sse_event(
                                ctx,
                                event.get("event"),
                                event.get("data") or "",
                            )
                        self._mark_first_output(ctx, output_state)
                        yield b"\n"
                        continue

                    ctx.chunk_count += 1

                    # 空流检测：超过阈值且无数据，发送错误事件并结束
                    if ctx.chunk_count > self.EMPTY_CHUNK_THRESHOLD and ctx.data_count == 0:
                        elapsed = time.time() - last_data_time
                        if elapsed > self.DATA_TIMEOUT:
                            logger.warning(f"Provider '{ctx.provider_name}' 流超时且无数据")
                            # 设置错误状态用于后续记录
                            ctx.status_code = 504
                            ctx.error_message = "流式响应超时，未收到有效数据"
                            ctx.upstream_response = f"流超时: Provider={ctx.provider_name}, elapsed={elapsed:.1f}s, chunk_count={ctx.chunk_count}, data_count=0"
                            error_event = {
                                "type": "error",
                                "error": {
                                    "type": "empty_stream_timeout",
                                    "message": ctx.error_message,
                                },
                            }
                            self._mark_first_output(ctx, output_state)
                            yield f"event: error\ndata: {json.dumps(error_event)}\n\n".encode(
                                "utf-8"
                            )
                            return  # 结束生成器

                    # 格式转换或直接透传
                    if needs_conversion:
                        converted_line = self._convert_sse_line(ctx, line, events)
                        if converted_line:
                            self._mark_first_output(ctx, output_state)
                            yield (converted_line + "\n").encode("utf-8")
                    else:
                        self._mark_first_output(ctx, output_state)
                        yield (line + "\n").encode("utf-8")

                for event in events:
                    self._handle_sse_event(
                        ctx,
                        event.get("event"),
                        event.get("data") or "",
                    )

                if ctx.data_count > 0:
                    last_data_time = time.time()

            # 处理剩余事件
            for event in sse_parser.flush():
                self._handle_sse_event(
                    ctx,
                    event.get("event"),
                    event.get("data") or "",
                )

            # 检查是否收到数据
            if ctx.data_count == 0:
                # 流已开始，无法抛出异常进行故障转移
                # 发送错误事件并记录日志
                logger.warning(f"Provider '{ctx.provider_name}' 返回空流式响应")
                # 设置错误状态用于后续记录
                ctx.status_code = 503
                ctx.error_message = "上游服务返回了空的流式响应"
                ctx.upstream_response = f"空流式响应: Provider={ctx.provider_name}, chunk_count={ctx.chunk_count}, data_count=0"
                error_event = {
                    "type": "error",
                    "error": {
                        "type": "empty_response",
                        "message": ctx.error_message,
                    },
                }
                yield f"event: error\ndata: {json.dumps(error_event)}\n\n".encode("utf-8")
            else:
                logger.debug("流式数据转发完成")

        except GeneratorExit:
            raise
        except httpx.StreamClosed:
            if ctx.data_count == 0:
                # 流已开始，发送错误事件而不是抛出异常
                logger.warning(f"Provider '{ctx.provider_name}' 流连接关闭且无数据")
                # 设置错误状态用于后续记录
                ctx.status_code = 503
                ctx.error_message = "上游服务连接关闭且未返回数据"
                ctx.upstream_response = f"流连接关闭: Provider={ctx.provider_name}, chunk_count={ctx.chunk_count}, data_count=0"
                error_event = {
                    "type": "error",
                    "error": {
                        "type": "stream_closed",
                        "message": ctx.error_message,
                    },
                }
                yield f"event: error\ndata: {json.dumps(error_event)}\n\n".encode("utf-8")
        except httpx.RemoteProtocolError:
            if ctx.data_count > 0:
                error_event = {
                    "type": "error",
                    "error": {
                        "type": "connection_error",
                        "message": "上游连接意外关闭，部分响应已成功传输",
                    },
                }
                yield f"event: error\ndata: {json.dumps(error_event)}\n\n".encode("utf-8")
            else:
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
        byte_iterator: Any,
        provider: Provider,
        endpoint: ProviderEndpoint,
        ctx: StreamContext,
    ) -> list:
        """
        预读流的前几行，检测嵌套错误

        某些 Provider（如 Gemini）可能返回 HTTP 200，但在响应体中包含错误信息。
        这种情况需要在流开始输出之前检测，以便触发重试逻辑。

        同时检测 HTML 响应（通常是 base_url 配置错误导致返回网页）。

        首次读取时会应用 TTFB（首字节超时）检测，超时则触发故障转移。

        Args:
            byte_iterator: 字节流迭代器
            provider: Provider 对象
            endpoint: Endpoint 对象
            ctx: 流上下文

        Returns:
            预读的字节块列表（需要在后续流中先输出）

        Raises:
            EmbeddedErrorException: 如果检测到嵌套错误
            ProviderNotAvailableException: 如果检测到 HTML 响应（配置错误）
            ProviderTimeoutException: 如果首字节超时（TTFB timeout）
        """
        prefetched_chunks: list = []
        max_prefetch_lines = config.stream_prefetch_lines  # 最多预读行数来检测错误
        max_prefetch_bytes = StreamDefaults.MAX_PREFETCH_BYTES  # 避免无换行响应导致 buffer 增长
        total_prefetched_bytes = 0
        buffer = b""
        line_count = 0
        should_stop = False
        # 使用增量解码器处理跨 chunk 的 UTF-8 字符
        decoder = codecs.getincrementaldecoder("utf-8")(errors="replace")

        try:
            # 获取对应格式的解析器
            provider_format = ctx.provider_api_format
            if provider_format:
                try:
                    provider_parser = get_parser_for_format(provider_format)
                except KeyError:
                    provider_parser = self.parser
            else:
                provider_parser = self.parser

            # 使用共享的 TTFB 超时函数读取首字节
            ttfb_timeout = config.stream_first_byte_timeout
            first_chunk, aiter = await read_first_chunk_with_ttfb_timeout(
                byte_iterator,
                timeout=ttfb_timeout,
                request_id=self.request_id,
                provider_name=str(provider.name),
            )
            prefetched_chunks.append(first_chunk)
            total_prefetched_bytes += len(first_chunk)
            buffer += first_chunk

            # 继续读取剩余的预读数据
            async for chunk in aiter:
                prefetched_chunks.append(chunk)
                total_prefetched_bytes += len(chunk)
                buffer += chunk

                # 尝试按行解析缓冲区（SSE 格式）
                while b"\n" in buffer:
                    line_bytes, buffer = buffer.split(b"\n", 1)
                    try:
                        # 使用增量解码器，可以正确处理跨 chunk 的多字节字符
                        line = decoder.decode(line_bytes + b"\n", False).rstrip("\n")
                    except Exception as e:
                        logger.warning(
                            f"[{self.request_id}] 预读时 UTF-8 解码失败: {e}, "
                            f"bytes={line_bytes[:50]!r}"
                        )
                        continue

                    line_count += 1
                    normalized_line = line.rstrip("\r")

                    # 检测 HTML 响应（base_url 配置错误的常见症状）
                    if check_html_response(normalized_line):
                        logger.error(
                            f"  [{self.request_id}] 检测到 HTML 响应，可能是 base_url 配置错误: "
                            f"Provider={provider.name}, Endpoint={endpoint.id[:8]}..., "
                            f"base_url={endpoint.base_url}"
                        )
                        raise ProviderNotAvailableException(
                            "上游服务返回了非预期的响应格式",
                            provider_name=str(provider.name),
                            upstream_status=200,
                            upstream_response=normalized_line[:500] if normalized_line else "(empty)",
                        )

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
                        should_stop = True
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
                        logger.warning(
                            f"  [{self.request_id}] 检测到嵌套错误: "
                            f"Provider={provider.name}, "
                            f"error_type={parsed.error_type}, "
                            f"message={parsed.error_message}"
                        )
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
                    should_stop = True
                    break

                # 达到预读字节上限，停止继续预读（避免无换行响应导致内存增长）
                if not should_stop and total_prefetched_bytes >= max_prefetch_bytes:
                    logger.debug(
                        f"  [{self.request_id}] 预读达到字节上限，停止继续预读: "
                        f"Provider={provider.name}, bytes={total_prefetched_bytes}, "
                        f"max_bytes={max_prefetch_bytes}"
                    )
                    break

                if should_stop or line_count >= max_prefetch_lines:
                    break

            # 预读结束后，检查是否为非 SSE 格式的 HTML/JSON 响应
            # 处理某些代理返回的纯 JSON 错误（可能无换行/多行 JSON）以及 HTML 页面（base_url 配置错误）
            if not should_stop and prefetched_chunks:
                check_prefetched_response_error(
                    prefetched_chunks=prefetched_chunks,
                    parser=provider_parser,
                    request_id=self.request_id,
                    provider_name=str(provider.name),
                    endpoint_id=endpoint.id,
                    base_url=endpoint.base_url,
                )

        except (EmbeddedErrorException, ProviderTimeoutException, ProviderNotAvailableException):
            # 重新抛出可重试的 Provider 异常，触发故障转移
            raise
        except (OSError, IOError) as e:
            # 网络 I/O 异常：记录警告，可能需要重试
            logger.warning(f"  [{self.request_id}] 预读流时发生网络异常: {type(e).__name__}: {e}")
        except Exception as e:
            # 未预期的严重异常：记录错误并重新抛出，避免掩盖问题
            logger.error(
                f"  [{self.request_id}] 预读流时发生严重异常: {type(e).__name__}: {e}",
                exc_info=True,
            )
            raise

        return prefetched_chunks

    async def _create_response_stream_with_prefetch(
        self,
        ctx: StreamContext,
        byte_iterator: Any,
        response_ctx: Any,
        http_client: httpx.AsyncClient,
        prefetched_chunks: list,
    ) -> AsyncGenerator[bytes, None]:
        """创建响应流生成器（带预读数据，使用字节流）"""
        try:
            sse_parser = SSEEventParser()
            last_data_time = time.time()
            buffer = b""
            output_state = {"first_yield": True, "streaming_updated": False}
            # 使用增量解码器处理跨 chunk 的 UTF-8 字符
            decoder = codecs.getincrementaldecoder("utf-8")(errors="replace")

            # 检查是否需要格式转换
            needs_conversion = self._needs_format_conversion(ctx)

            # 先处理预读的字节块
            for chunk in prefetched_chunks:
                buffer += chunk
                # 处理缓冲区中的完整行
                while b"\n" in buffer:
                    line_bytes, buffer = buffer.split(b"\n", 1)
                    try:
                        # 使用增量解码器，可以正确处理跨 chunk 的多字节字符
                        line = decoder.decode(line_bytes + b"\n", False).rstrip("\n")
                    except Exception as e:
                        logger.warning(
                            f"[{self.request_id}] UTF-8 解码失败: {e}, "
                            f"bytes={line_bytes[:50]!r}"
                        )
                        continue

                    normalized_line = line.rstrip("\r")
                    events = sse_parser.feed_line(normalized_line)

                    if normalized_line == "":
                        for event in events:
                            self._handle_sse_event(
                                ctx,
                                event.get("event"),
                                event.get("data") or "",
                            )
                        self._mark_first_output(ctx, output_state)
                        yield b"\n"
                        continue

                    ctx.chunk_count += 1

                    # 格式转换或直接透传
                    if needs_conversion:
                        converted_line = self._convert_sse_line(ctx, line, events)
                        if converted_line:
                            self._mark_first_output(ctx, output_state)
                            yield (converted_line + "\n").encode("utf-8")
                    else:
                        self._mark_first_output(ctx, output_state)
                        yield (line + "\n").encode("utf-8")

                    for event in events:
                        self._handle_sse_event(
                            ctx,
                            event.get("event"),
                            event.get("data") or "",
                        )

                    if ctx.data_count > 0:
                        last_data_time = time.time()

            # 继续处理剩余的流数据（使用同一个迭代器）
            async for chunk in byte_iterator:
                buffer += chunk
                # 处理缓冲区中的完整行
                while b"\n" in buffer:
                    line_bytes, buffer = buffer.split(b"\n", 1)
                    try:
                        # 使用增量解码器，可以正确处理跨 chunk 的多字节字符
                        line = decoder.decode(line_bytes + b"\n", False).rstrip("\n")
                    except Exception as e:
                        logger.warning(
                            f"[{self.request_id}] UTF-8 解码失败: {e}, "
                            f"bytes={line_bytes[:50]!r}"
                        )
                        continue

                    normalized_line = line.rstrip("\r")
                    events = sse_parser.feed_line(normalized_line)

                    if normalized_line == "":
                        for event in events:
                            self._handle_sse_event(
                                ctx,
                                event.get("event"),
                                event.get("data") or "",
                            )
                        self._mark_first_output(ctx, output_state)
                        yield b"\n"
                        continue

                    ctx.chunk_count += 1

                    # 空流检测：超过阈值且无数据，发送错误事件并结束
                    if ctx.chunk_count > self.EMPTY_CHUNK_THRESHOLD and ctx.data_count == 0:
                        elapsed = time.time() - last_data_time
                        if elapsed > self.DATA_TIMEOUT:
                            logger.warning(f"Provider '{ctx.provider_name}' 流超时且无数据")
                            # 设置错误状态用于后续记录
                            ctx.status_code = 504
                            ctx.error_message = "流式响应超时，未收到有效数据"
                            ctx.upstream_response = f"流超时: Provider={ctx.provider_name}, elapsed={elapsed:.1f}s, chunk_count={ctx.chunk_count}, data_count=0"
                            error_event = {
                                "type": "error",
                                "error": {
                                    "type": "empty_stream_timeout",
                                    "message": ctx.error_message,
                                },
                            }
                            self._mark_first_output(ctx, output_state)
                            yield f"event: error\ndata: {json.dumps(error_event)}\n\n".encode(
                                "utf-8"
                            )
                            return

                    # 格式转换或直接透传
                    if needs_conversion:
                        converted_line = self._convert_sse_line(ctx, line, events)
                        if converted_line:
                            self._mark_first_output(ctx, output_state)
                            yield (converted_line + "\n").encode("utf-8")
                    else:
                        self._mark_first_output(ctx, output_state)
                        yield (line + "\n").encode("utf-8")

                    for event in events:
                        self._handle_sse_event(
                            ctx,
                            event.get("event"),
                            event.get("data") or "",
                        )

                    if ctx.data_count > 0:
                        last_data_time = time.time()

            # 处理剩余事件
            flushed_events = sse_parser.flush()
            for event in flushed_events:
                self._handle_sse_event(
                    ctx,
                    event.get("event"),
                    event.get("data") or "",
                )

            # 检查是否收到数据
            if ctx.data_count == 0:
                # 空流通常意味着配置错误（如 base_url 指向了网页而非 API）
                logger.error(
                    f"Provider '{ctx.provider_name}' 返回空流式响应 (收到 {ctx.chunk_count} 个非数据行), "
                    f"可能是 endpoint base_url 配置错误"
                )
                # 设置错误状态用于后续记录
                ctx.status_code = 503
                ctx.error_message = "上游服务返回了空的流式响应"
                ctx.upstream_response = f"空流式响应: Provider={ctx.provider_name}, chunk_count={ctx.chunk_count}, data_count=0, 可能是 base_url 配置错误"
                error_event = {
                    "type": "error",
                    "error": {
                        "type": "empty_response",
                        "message": ctx.error_message,
                    },
                }
                yield f"event: error\ndata: {json.dumps(error_event)}\n\n".encode("utf-8")
            else:
                logger.debug("流式数据转发完成")

        except GeneratorExit:
            raise
        except httpx.StreamClosed:
            if ctx.data_count == 0:
                logger.warning(f"Provider '{ctx.provider_name}' 流连接关闭且无数据")
                # 设置错误状态用于后续记录
                ctx.status_code = 503
                ctx.error_message = "上游服务连接关闭且未返回数据"
                ctx.upstream_response = f"流连接关闭: Provider={ctx.provider_name}, chunk_count={ctx.chunk_count}, data_count=0"
                error_event = {
                    "type": "error",
                    "error": {
                        "type": "stream_closed",
                        "message": ctx.error_message,
                    },
                }
                yield f"event: error\ndata: {json.dumps(error_event)}\n\n".encode("utf-8")
        except httpx.RemoteProtocolError:
            if ctx.data_count > 0:
                error_event = {
                    "type": "error",
                    "error": {
                        "type": "connection_error",
                        "message": "上游连接意外关闭，部分响应已成功传输",
                    },
                }
                yield f"event: error\ndata: {json.dumps(error_event)}\n\n".encode("utf-8")
            else:
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

    def _handle_sse_event(
        self,
        ctx: StreamContext,
        event_name: Optional[str],
        data_str: str,
    ) -> None:
        """
        处理 SSE 事件

        通用框架：解析 JSON、更新计数器
        子类可覆盖 _process_event_data() 实现格式特定逻辑
        """
        if not data_str:
            return

        if data_str == "[DONE]":
            ctx.has_completion = True
            return

        try:
            data = json.loads(data_str)
        except json.JSONDecodeError:
            return

        ctx.data_count += 1
        ctx.parsed_chunks.append(data)

        if not isinstance(data, dict):
            return

        event_type = event_name or data.get("type", "")

        # 调用格式特定的处理逻辑
        self._process_event_data(ctx, event_type, data)

    def _process_event_data(
        self,
        ctx: StreamContext,
        event_type: str,
        data: Dict[str, Any],
    ) -> None:
        """
        处理解析后的事件数据 - 子类应覆盖此方法

        默认实现使用 ResponseParser 提取 usage
        """
        # 提取 response_id
        if not ctx.response_id:
            response_obj = data.get("response")
            if isinstance(response_obj, dict) and response_obj.get("id"):
                ctx.response_id = response_obj["id"]
            elif "id" in data:
                ctx.response_id = data["id"]

        # 使用解析器提取 usage
        usage = self.parser.extract_usage_from_response(data)
        if usage and not ctx.final_usage:
            ctx.input_tokens = usage.get("input_tokens", 0)
            ctx.output_tokens = usage.get("output_tokens", 0)
            ctx.cached_tokens = usage.get("cache_read_tokens", 0)
            ctx.final_usage = usage

        # 提取文本内容
        text = self.parser.extract_text_content(data)
        if text:
            ctx.append_text(text)

        # 检查完成事件
        if event_type in ("response.completed", "message_stop"):
            ctx.has_completion = True
            response_obj = data.get("response")
            if isinstance(response_obj, dict):
                ctx.final_response = response_obj

    def _finalize_stream_metadata(self, ctx: StreamContext) -> None:
        """
        在记录统计前从 parsed_chunks 中提取额外的元数据 - 子类可覆盖

        这是一个后处理钩子，在流传输完成后、记录 Usage 之前调用。
        子类可以覆盖此方法从 ctx.parsed_chunks 中提取格式特定的元数据，
        如 Gemini 的 modelVersion、token 统计等。

        Args:
            ctx: 流上下文，包含 parsed_chunks 和 response_metadata
        """
        pass

    async def _create_monitored_stream(
        self,
        ctx: StreamContext,
        stream_generator: AsyncGenerator[bytes, None],
    ) -> AsyncGenerator[bytes, None]:
        """创建带监控的流生成器"""
        import time as time_module

        last_chunk_time = time_module.time()
        chunk_count = 0
        try:
            async for chunk in stream_generator:
                last_chunk_time = time_module.time()
                chunk_count += 1
                yield chunk
        except asyncio.CancelledError:
            # 计算距离上次收到 chunk 的时间
            time_since_last_chunk = time_module.time() - last_chunk_time
            # 如果响应已完成，不标记为失败
            if not ctx.has_completion:
                ctx.status_code = 499
                ctx.error_message = "Client disconnected"
                logger.warning(
                    f"ID:{ctx.request_id} | Stream cancelled: "
                    f"chunks={chunk_count}, "
                    f"has_completion={ctx.has_completion}, "
                    f"time_since_last_chunk={time_since_last_chunk:.2f}s, "
                    f"output_tokens={ctx.output_tokens}"
                )
            raise
        except httpx.TimeoutException as e:
            ctx.status_code = 504
            ctx.error_message = str(e)
            raise
        except Exception as e:
            ctx.status_code = 500
            ctx.error_message = str(e)
            raise

    async def _record_stream_stats(
        self,
        ctx: StreamContext,
        original_headers: Dict[str, str],
        original_request_body: Dict[str, Any],
    ) -> None:
        """在流完成后记录统计信息"""
        try:
            # 使用 self.start_time 作为时间基准，与首字时间保持一致
            # 注意：不要把统计延迟算进响应时间里
            response_time_ms = int((time.time() - self.start_time) * 1000)

            await asyncio.sleep(0.1)

            if not ctx.provider_name:
                logger.warning(f"[{ctx.request_id}] 流式请求失败，未选中提供商")
                return

            # Claude API 的 input_tokens 已经是非缓存部分，不需要再减去 cached_tokens
            # 实际计费的输入 tokens = input_tokens + cache_creation_tokens（缓存读取免费或折扣）
            actual_input_tokens = ctx.input_tokens

            # 获取新的 DB session
            db_gen = get_db()
            bg_db = next(db_gen)

            try:
                from src.models.database import ApiKey as ApiKeyModel

                user = bg_db.query(User).filter(User.id == ctx.user_id).first()
                api_key = bg_db.query(ApiKeyModel).filter(ApiKeyModel.id == ctx.api_key_id).first()

                if not user or not api_key:
                    return

                bg_telemetry = MessageTelemetry(
                    bg_db, user, api_key, ctx.request_id, self.client_ip
                )

                response_body = {
                    "chunks": ctx.parsed_chunks,
                    "metadata": {
                        "stream": True,
                        "total_chunks": len(ctx.parsed_chunks),
                        "data_count": ctx.data_count,
                        "has_completion": ctx.has_completion,
                        "response_time_ms": response_time_ms,
                    },
                }

                # 使用实际发送给 Provider 的请求体（如果有），否则用原始请求体
                actual_request_body = ctx.provider_request_body or original_request_body

                # 根据状态码决定记录成功还是失败
                # 499 = 客户端断开连接，503 = 服务不可用（如流中断）
                if ctx.status_code and ctx.status_code >= 400:
                    # 记录失败的 Usage，但使用已收到的预估 token 信息（来自 message_start）
                    # 这样即使请求中断，也能记录预估成本
                    # 失败时返回给客户端的是 JSON 错误响应，如果没有设置则使用默认值
                    client_response_headers = ctx.client_response_headers or {"content-type": "application/json"}
                    await bg_telemetry.record_failure(
                        provider=ctx.provider_name or "unknown",
                        model=ctx.model,
                        response_time_ms=response_time_ms,
                        status_code=ctx.status_code,
                        error_message=ctx.error_message or f"HTTP {ctx.status_code}",
                        request_headers=original_headers,
                        request_body=actual_request_body,
                        is_stream=True,
                        api_format=ctx.api_format,
                        provider_request_headers=ctx.provider_request_headers,
                        # 预估 token 信息（来自 message_start 事件）
                        input_tokens=actual_input_tokens,
                        output_tokens=ctx.output_tokens,
                        cache_creation_tokens=ctx.cache_creation_tokens,
                        cache_read_tokens=ctx.cached_tokens,
                        response_body=response_body,
                        response_headers=ctx.response_headers,
                        client_response_headers=client_response_headers,
                        # 模型映射信息
                        target_model=ctx.mapped_model,
                    )
                    logger.debug(f"{self.FORMAT_ID} 流式响应中断")
                    # 简洁的请求失败摘要（包含预估 token 信息）
                    logger.info(
                        f"[FAIL] {self.request_id[:8]} | {ctx.model} | {ctx.provider_name} | {response_time_ms}ms | "
                        f"{ctx.status_code} | in:{actual_input_tokens} out:{ctx.output_tokens} cache:{ctx.cached_tokens}"
                    )
                else:
                    # 在记录统计前，允许子类从 parsed_chunks 中提取额外的元数据
                    self._finalize_stream_metadata(ctx)

                    # 流式成功时，返回给客户端的是提供商响应头 + SSE 必需头
                    client_response_headers = dict(ctx.response_headers) if ctx.response_headers else {}
                    client_response_headers.update({
                        "Cache-Control": "no-cache, no-transform",
                        "X-Accel-Buffering": "no",
                        "content-type": "text/event-stream",
                    })

                    total_cost = await bg_telemetry.record_success(
                        provider=ctx.provider_name,
                        model=ctx.model,
                        input_tokens=actual_input_tokens,
                        output_tokens=ctx.output_tokens,
                        response_time_ms=response_time_ms,
                        first_byte_time_ms=ctx.first_byte_time_ms,  # 传递首字时间
                        status_code=ctx.status_code,
                        request_headers=original_headers,
                        request_body=actual_request_body,
                        response_headers=ctx.response_headers,
                        client_response_headers=client_response_headers,
                        response_body=response_body,
                        cache_creation_tokens=ctx.cache_creation_tokens,
                        cache_read_tokens=ctx.cached_tokens,
                        is_stream=True,
                        provider_request_headers=ctx.provider_request_headers,
                        api_format=ctx.api_format,
                        # Provider 侧追踪信息（用于记录真实成本）
                        provider_id=ctx.provider_id,
                        provider_endpoint_id=ctx.endpoint_id,
                        provider_api_key_id=ctx.key_id,
                        # 模型映射信息
                        target_model=ctx.mapped_model,
                        # Provider 响应元数据（如 Gemini 的 modelVersion）
                        response_metadata=ctx.response_metadata if ctx.response_metadata else None,
                    )
                    logger.debug(f"{self.FORMAT_ID} 流式响应完成")
                    # 简洁的请求完成摘要（两行格式）
                    line1 = f"[OK] {self.request_id[:8]} | {ctx.model} | {ctx.provider_name}"
                    if ctx.first_byte_time_ms:
                        line1 += f" | TTFB: {ctx.first_byte_time_ms}ms"

                    line2 = (
                        f"      Total: {response_time_ms}ms | "
                        f"in:{ctx.input_tokens or 0} out:{ctx.output_tokens or 0}"
                    )
                    logger.info(f"{line1}\n{line2}")

                # 更新候选记录的最终状态和延迟时间
                # 注意：RequestExecutor 会在流开始时过早地标记成功（只记录了连接建立的时间）
                # 这里用流传输完成后的实际时间覆盖
                if ctx.attempt_id:
                    from src.services.request.candidate import RequestCandidateService

                    # 根据状态码决定是成功还是失败
                    # 499 = 客户端断开连接，应标记为失败
                    # 503 = 服务不可用（如流中断），应标记为失败
                    if ctx.status_code and ctx.status_code >= 400:
                        # 请求链路追踪使用 upstream_response（原始响应），回退到 error_message（友好消息）
                        trace_error_message = ctx.upstream_response or ctx.error_message or f"HTTP {ctx.status_code}"
                        RequestCandidateService.mark_candidate_failed(
                            db=bg_db,
                            candidate_id=ctx.attempt_id,
                            error_type=(
                                "client_disconnected" if ctx.status_code == 499 else "stream_error"
                            ),
                            error_message=trace_error_message,
                            status_code=ctx.status_code,
                            latency_ms=response_time_ms,
                            extra_data={
                                "stream_completed": False,
                                "chunk_count": ctx.chunk_count,
                                "data_count": ctx.data_count,
                            },
                        )
                    else:
                        RequestCandidateService.mark_candidate_success(
                            db=bg_db,
                            candidate_id=ctx.attempt_id,
                            status_code=ctx.status_code,
                            latency_ms=response_time_ms,
                            extra_data={
                                "stream_completed": True,
                                "chunk_count": ctx.chunk_count,
                                "data_count": ctx.data_count,
                            },
                        )

            finally:
                bg_db.close()

        except Exception as e:
            logger.exception("记录流式统计信息时出错")

    async def _record_stream_failure(
        self,
        ctx: StreamContext,
        error: Exception,
        original_headers: Dict[str, str],
        original_request_body: Dict[str, Any],
    ) -> None:
        """记录流式请求失败"""
        # 使用 self.start_time 作为时间基准，与首字时间保持一致
        response_time_ms = int((time.time() - self.start_time) * 1000)

        status_code = 503
        if isinstance(error, ProviderAuthException):
            status_code = 503
        elif isinstance(error, ProviderRateLimitException):
            status_code = 429
        elif isinstance(error, ProviderTimeoutException):
            status_code = 504

        ctx.status_code = status_code
        ctx.error_message = str(error)

        # 使用实际发送给 Provider 的请求体（如果有），否则用原始请求体
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
            # 模型映射信息
            target_model=ctx.mapped_model,
        )

    # _update_usage_to_streaming 方法已移至基类 BaseMessageHandler

    async def process_sync(
        self,
        original_request_body: Dict[str, Any],
        original_headers: Dict[str, str],
        query_params: Optional[Dict[str, str]] = None,
        path_params: Optional[Dict[str, Any]] = None,
    ) -> JSONResponse:
        """
        处理非流式请求

        通用流程：
        1. 构建请求
        2. 通过 FallbackOrchestrator 执行
        3. 解析响应并记录统计
        """
        logger.debug(f"开始非流式响应处理 ({self.FORMAT_ID})")

        # 使用子类实现的方法提取 model（不同 API 格式的 model 位置不同）
        model = self.extract_model_from_request(original_request_body, path_params)
        api_format = self.allowed_api_formats[0]
        sync_start_time = time.time()

        provider_name = None
        response_json = None
        status_code = 200
        response_headers = {}
        provider_api_format = ""  # 用于追踪 Provider 的 API 格式
        provider_request_headers = {}  # 发送给 Provider 的请求头
        provider_request_body = None  # 实际发送给 Provider 的请求体
        provider_id = None  # Provider ID（用于失败记录）
        endpoint_id = None  # Endpoint ID（用于失败记录）
        key_id = None  # Key ID（用于失败记录）
        mapped_model_result = None  # 映射后的目标模型名（用于 Usage 记录）
        response_metadata_result: Dict[str, Any] = {}  # Provider 响应元数据

        async def sync_request_func(
            provider: Provider,
            endpoint: ProviderEndpoint,
            key: ProviderAPIKey,
            candidate: ProviderCandidate,
        ) -> Dict[str, Any]:
            nonlocal provider_name, response_json, status_code, response_headers, provider_api_format, provider_request_headers, provider_request_body, mapped_model_result, response_metadata_result
            provider_name = str(provider.name)
            provider_api_format = str(endpoint.api_format) if endpoint.api_format else ""

            # 获取模型映射（优先使用别名匹配到的模型，其次是 Provider 级别的映射）
            mapped_model = candidate.alias_matched_model if candidate else None
            if not mapped_model:
                mapped_model = await self._get_mapped_model(
                    source_model=model,
                    provider_id=str(provider.id),
                )

            # 应用模型映射到请求体（子类可覆盖此方法处理不同格式）
            if mapped_model:
                mapped_model_result = mapped_model  # 保存映射后的模型名，用于 Usage 记录
                request_body = self.apply_mapped_model(original_request_body, mapped_model)
            else:
                request_body = original_request_body

            # 准备发送给 Provider 的请求体（子类可覆盖以移除不需要的字段）
            request_body = self.prepare_provider_request_body(request_body)

            # 使用 RequestBuilder 构建请求体和请求头
            # 注意：mapped_model 已经应用到 request_body，这里不再传递
            provider_payload, provider_headers = self._request_builder.build(
                request_body,
                original_headers,
                endpoint,
                key,
                is_stream=False,
            )

            # 保存发送给 Provider 的请求信息（用于调试和统计）
            provider_request_headers = provider_headers
            provider_request_body = provider_payload

            # 获取用于 URL 的模型名（子类可覆盖此方法，如 Gemini 需要特殊处理）
            url_model = self.get_model_for_url(request_body, mapped_model)

            url = build_provider_url(
                endpoint,
                query_params=query_params,
                path_params={"model": url_model},
                is_stream=False,  # 非流式请求
            )

            logger.info(
                f"  └─ [{self.request_id}] 发送非流式请求: "
                f"Provider={provider.name}, Endpoint={endpoint.id[:8] if endpoint.id else 'N/A'}..., "
                f"Key=***{key.api_key[-4:] if key.api_key else 'N/A'}, "
                f"原始模型={model}, 映射后={mapped_model or '无映射'}, URL模型={url_model}"
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
                headers=provider_headers,
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
                    retry_after=int(resp.headers.get("retry-after", 0)) or None,
                )
            elif resp.status_code >= 500:
                error_text = resp.text
                raise ProviderNotAvailableException(
                    f"上游服务暂时不可用 (HTTP {resp.status_code})",
                    provider_name=str(provider.name),
                    upstream_status=resp.status_code,
                    upstream_response=error_text,
                )
            elif 300 <= resp.status_code < 400:
                redirect_url = resp.headers.get("location", "unknown")
                raise ProviderNotAvailableException(
                    "上游服务返回重定向响应",
                    provider_name=str(provider.name),
                    upstream_status=resp.status_code,
                    upstream_response=f"重定向 {resp.status_code} -> {redirect_url}",
                )
            elif resp.status_code != 200:
                error_text = resp.text
                raise ProviderNotAvailableException(
                    f"上游服务返回错误 (HTTP {resp.status_code})",
                    provider_name=str(provider.name),
                    upstream_status=resp.status_code,
                    upstream_response=error_text,
                )

            # 安全解析 JSON 响应，处理可能的编码错误
            try:
                response_json = resp.json()
            except (UnicodeDecodeError, json.JSONDecodeError) as e:
                # 获取原始响应内容用于调试（存入 upstream_response）
                content_type = resp.headers.get("content-type", "unknown")
                content_encoding = resp.headers.get("content-encoding", "none")
                raw_content = ""
                try:
                    raw_content = resp.text[:500] if resp.text else "(empty)"
                except Exception:
                    try:
                        raw_content = repr(resp.content[:500]) if resp.content else "(empty)"
                    except Exception:
                        raw_content = "(unable to read)"
                logger.error(
                    f"[{self.request_id}] 无法解析响应 JSON: {e}, "
                    f"Content-Type: {content_type}, Content-Encoding: {content_encoding}, "
                    f"响应长度: {len(resp.content)} bytes, 原始内容: {raw_content}"
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

            # 提取 Provider 响应元数据（子类可覆盖）
            response_metadata_result = self._extract_response_metadata(response_json)

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
            response_time_ms = int((time.time() - sync_start_time) * 1000)

            # 确保 response_json 不为 None
            if response_json is None:
                response_json = {}

            # 检查是否需要格式转换
            if (
                provider_api_format
                and api_format
                and provider_api_format.upper() != api_format.upper()
            ):
                from src.api.handlers.base.format_converter_registry import converter_registry

                try:
                    response_json = converter_registry.convert_response(
                        response_json,
                        provider_api_format,
                        api_format,
                    )
                    logger.debug(f"非流式响应格式转换完成: {provider_api_format} -> {api_format}")
                except Exception as conv_err:
                    logger.warning(f"非流式响应格式转换失败，使用原始响应: {conv_err}")

            # 使用解析器提取 usage
            usage = self.parser.extract_usage_from_response(response_json)
            input_tokens = usage.get("input_tokens", 0)
            output_tokens = usage.get("output_tokens", 0)
            cached_tokens = usage.get("cache_read_tokens", 0)
            cache_creation_tokens = usage.get("cache_creation_tokens", 0)
            # Claude API 的 input_tokens 已经是非缓存部分，不需要再减去 cached_tokens
            actual_input_tokens = input_tokens

            output_text = self.parser.extract_text_content(response_json)[:200]

            # 使用实际发送给 Provider 的请求体（如果有），否则用原始请求体
            actual_request_body = provider_request_body or original_request_body

            # 非流式成功时，返回给客户端的是提供商响应头（透传）
            client_response_headers = dict(response_headers) if response_headers else {}
            client_response_headers["content-type"] = "application/json"

            total_cost = await self.telemetry.record_success(
                provider=provider_name,
                model=model,
                input_tokens=actual_input_tokens,
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
                # Provider 侧追踪信息（用于记录真实成本）
                provider_id=provider_id,
                provider_endpoint_id=endpoint_id,
                provider_api_key_id=key_id,
                # 模型映射信息
                target_model=mapped_model_result,
                # Provider 响应元数据（如 Gemini 的 modelVersion）
                response_metadata=response_metadata_result if response_metadata_result else None,
            )

            logger.info(f"{self.FORMAT_ID} 非流式响应处理完成")

            # 透传提供商的响应头
            return JSONResponse(
                status_code=status_code,
                content=response_json,
                headers=response_headers if response_headers else None,
            )

        except Exception as e:
            response_time_ms = int((time.time() - sync_start_time) * 1000)

            status_code = 503
            if isinstance(e, ProviderAuthException):
                status_code = 503
            elif isinstance(e, ProviderRateLimitException):
                status_code = 429
            elif isinstance(e, ProviderTimeoutException):
                status_code = 504

            # 使用实际发送给 Provider 的请求体（如果有），否则用原始请求体
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

                for encoding in ["utf-8", "gbk", "latin1"]:
                    try:
                        return error_bytes.decode(encoding)
                    except (UnicodeDecodeError, LookupError):
                        continue

                return error_bytes.decode("utf-8", errors="replace")
            else:
                return (
                    e.response.text
                    if hasattr(e.response, "_content")
                    else "Unable to read response"
                )
        except Exception as decode_error:
            return f"Unable to read error response: {decode_error}"

    def _needs_format_conversion(self, ctx: StreamContext) -> bool:
        """
        检查是否需要进行格式转换

        当 Provider 的 API 格式与客户端请求的 API 格式不同时，需要转换响应。
        例如：客户端请求 Claude 格式，但 Provider 返回 OpenAI 格式。
        """
        if not ctx.provider_api_format or not ctx.client_api_format:
            return False
        return ctx.provider_api_format.upper() != ctx.client_api_format.upper()

    def _mark_first_output(self, ctx: StreamContext, state: Dict[str, bool]) -> None:
        """
        标记首次输出：记录 TTFB 并更新 streaming 状态

        在第一次 yield 数据前调用，确保：
        1. 首字时间 (TTFB) 已记录到 ctx
        2. Usage 状态已更新为 streaming（包含 provider/key/TTFB 信息）

        Args:
            ctx: 流上下文
            state: 包含 first_yield 和 streaming_updated 的状态字典
        """
        if state["first_yield"]:
            ctx.record_first_byte_time(self.start_time)
            state["first_yield"] = False
            if not state["streaming_updated"]:
                self._update_usage_to_streaming_with_ctx(ctx)
                state["streaming_updated"] = True

    def _convert_sse_line(
        self,
        ctx: StreamContext,
        line: str,
        events: list,
    ) -> Optional[str]:
        """
        将 SSE 行从 Provider 格式转换为客户端格式

        Args:
            ctx: 流上下文
            line: 原始 SSE 行
            events: 解析后的事件列表

        Returns:
            转换后的 SSE 行，如果无法转换则返回 None
        """
        from src.api.handlers.base.format_converter_registry import converter_registry

        # 如果是空行或特殊控制行，直接返回
        if not line or line.strip() == "" or line == "data: [DONE]":
            return line

        # 如果不是 data 行，直接透传
        if not line.startswith("data:"):
            return line

        # 提取 data 内容
        data_content = line[5:].strip()  # 去掉 "data:" 前缀

        # 尝试解析 JSON
        try:
            data_obj = json.loads(data_content)
        except json.JSONDecodeError:
            # 无法解析，直接透传
            return line

        # 使用注册表进行格式转换
        try:
            converted_obj = converter_registry.convert_stream_chunk(
                data_obj,
                ctx.provider_api_format,
                ctx.client_api_format,
            )
            # 重新构建 SSE 行
            return f"data: {json.dumps(converted_obj, ensure_ascii=False)}"
        except Exception as e:
            logger.warning(f"格式转换失败，透传原始数据: {e}")
            return line
