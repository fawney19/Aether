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

from __future__ import annotations

import asyncio
import json
from abc import ABC, abstractmethod
from collections.abc import AsyncGenerator, Awaitable, Callable
from dataclasses import dataclass, field
from typing import Any

import httpx
from fastapi import BackgroundTasks, Request
from fastapi.responses import JSONResponse, StreamingResponse
from sqlalchemy.orm import Session

from src.api.handlers.base.base_handler import (
    BaseMessageHandler,
    ClientDisconnectedException,
    wait_for_with_disconnect_detection,
)
from src.api.handlers.base.chat_error_utils import (
    _build_error_json_payload,
    _get_error_status_code,
    _resolve_dynamic_format,
)
from src.api.handlers.base.parsers import get_parser_for_format
from src.api.handlers.base.request_builder import (
    PassthroughRequestBuilder,
    get_provider_auth,
)
from src.api.handlers.base.response_parser import ResponseParser
from src.api.handlers.base.stream_context import (
    StreamContext,
)
from src.api.handlers.base.stream_processor import StreamProcessor
from src.api.handlers.base.stream_telemetry import StreamTelemetryRecorder
from src.api.handlers.base.utils import (
    build_sse_headers,
    filter_proxy_response_headers,
    get_format_converter_registry,
    resolve_client_content_encoding,
)
from src.config.settings import config
from src.core.api_format.conversion.stream_bridge import (
    iter_internal_response_as_stream_events,
)
from src.core.exceptions import (
    EmbeddedErrorException,
    ProviderNotAvailableException,
    ProviderTimeoutException,
    ThinkingSignatureException,
    UpstreamClientException,
)
from src.core.logger import logger
from src.models.database import (
    ApiKey,
    Provider,
    ProviderAPIKey,
    ProviderEndpoint,
    User,
)
from src.services.provider.behavior import get_provider_behavior
from src.services.provider.prompt_cache import (
    maybe_patch_request_with_prompt_cache_key,
)
from src.services.provider.stream_policy import (
    enforce_stream_mode_for_upstream,
    get_upstream_stream_policy,
    resolve_upstream_is_stream,
)
from src.services.provider.transport import (
    build_provider_url,
)
from src.services.provider.upstream_headers import build_upstream_extra_headers
from src.services.scheduling.aware_scheduler import ProviderCandidate
from src.services.system.config import SystemConfigService
from src.services.task.request_state import MutableRequestBodyState


@dataclass
class ProviderRequestResult:
    """_prepare_provider_request() 的返回结果，封装请求构建阶段的所有产出。"""

    request_body: dict[str, Any]
    url_model: str
    mapped_model: str | None
    envelope: Any  # ProviderEnvelope | None
    extra_headers: dict[str, str] = field(default_factory=dict)
    upstream_is_stream: bool = True
    needs_conversion: bool = False
    provider_api_format: str = ""
    client_api_format: str = ""
    auth_info: Any = None
    tls_profile: str | None = None


class ChatHandlerBase(BaseMessageHandler, ABC):
    """
    Chat Handler 基类

    主要职责：
    - 通过 TaskService/FailoverEngine 选择 Provider/Endpoint/Key
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
        allowed_api_formats: list | None = None,
        adapter_detector: None | (
            Callable[[dict[str, str], dict[str, Any] | None], dict[str, bool]]
        ) = None,
        perf_metrics: dict[str, Any] | None = None,
        api_family: str | None = None,
        endpoint_kind: str | None = None,
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
            perf_metrics=perf_metrics,
            api_family=api_family,
            endpoint_kind=endpoint_kind,
        )
        self._parser: ResponseParser | None = None
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
    def _extract_usage(self, response: dict) -> dict[str, int]:
        """
        从响应中提取 token 使用情况

        Args:
            response: 响应数据

        Returns:
            Dict with keys: input_tokens, output_tokens,
                           cache_creation_input_tokens, cache_read_input_tokens
        """
        pass

    def _normalize_response(self, response: dict) -> dict:
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
        request_body: dict[str, Any],
        path_params: dict[str, Any] | None = None,  # noqa: ARG002 - 子类使用
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
        request_body: dict[str, Any],
        mapped_model: str,  # noqa: ARG002 - 子类使用
    ) -> dict[str, Any]:
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
        request_body: dict[str, Any],
        mapped_model: str | None,
    ) -> str | None:
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
        request_body: dict[str, Any],
    ) -> dict[str, Any]:
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

    def finalize_provider_request(
        self,
        request_body: dict[str, Any],
        *,
        mapped_model: str | None,
        provider_api_format: str | None,
    ) -> dict[str, Any]:
        """
        格式转换完成后、envelope 之前的模型感知后处理钩子 - 子类可覆盖

        用于根据目标模型的特性对请求体做最终调整，例如：
        - 图像生成模型需要移除不兼容的 tools/system_instruction 并注入 imageConfig
        - 特定模型需要注入/移除某些字段
        - Gemini 格式：清理无效 parts 和合并连续同角色 contents

        此方法在流式和非流式路径中均会被调用，且 mapped_model 已确定。

        Args:
            request_body: 已完成格式转换的请求体
            mapped_model: 映射后的目标模型名
            provider_api_format: Provider 侧 API 格式标识

        Returns:
            调整后的请求体
        """
        # Gemini 格式请求：清理无效 parts 和合并连续同角色 contents
        # 跨格式转换（如 Claude → Gemini）可能产生 thinking 等无法表示的块，
        # 导致 parts 为空或缺少有效 data-oneof 字段，被 Google API 拒绝。
        if provider_api_format and "gemini" in str(provider_api_format).lower():
            contents = request_body.get("contents")
            if isinstance(contents, list):
                from src.core.api_format.conversion.normalizers.gemini import (
                    compact_gemini_contents,
                )

                request_body["contents"] = compact_gemini_contents(contents)

        return request_body

    def _set_model_after_conversion(
        self,
        request_body: dict[str, Any],
        provider_api_format: str,
        mapped_model: str | None,
        fallback_model: str,
    ) -> None:
        """
        跨格式转换后设置 model 字段

        根据目标格式的 model_in_body 属性决定是否在请求体中设置 model 字段。
        Gemini 等格式通过 URL 路径传递模型名，不需要在请求体中设置。

        Args:
            request_body: 请求体字典（会被原地修改）
            provider_api_format: Provider 侧 API 格式
            mapped_model: 映射后的模型名
            fallback_model: 兜底模型名（无映射时使用）
        """
        from src.core.api_format.metadata import resolve_endpoint_definition

        target_meta = resolve_endpoint_definition(provider_api_format)
        if target_meta is None:
            # 未知格式，保守处理：默认设置 model
            request_body["model"] = mapped_model or fallback_model
            return

        if target_meta.model_in_body:
            request_body["model"] = mapped_model or fallback_model
        else:
            request_body.pop("model", None)

    def _set_stream_after_conversion(
        self,
        request_body: dict[str, Any],
        client_api_format: str,
        provider_api_format: str,
        is_stream: bool,
    ) -> None:
        """
        跨格式转换后设置 stream 字段

        当客户端格式不使用 stream 字段（如 Gemini 通过 URL 端点区分流式），
        而 Provider 格式需要 stream 字段（如 OpenAI/Claude）时，需要显式设置。

        Args:
            request_body: 请求体字典（会被原地修改）
            client_api_format: 客户端 API 格式
            provider_api_format: Provider 侧 API 格式
            is_stream: 是否为流式请求
        """
        from src.core.api_format.metadata import resolve_endpoint_definition

        client_meta = resolve_endpoint_definition(client_api_format)
        provider_meta = resolve_endpoint_definition(provider_api_format)

        # 默认：stream_in_body=True（如 OpenAI/Claude）
        client_uses_stream = client_meta.stream_in_body if client_meta else True
        provider_uses_stream = provider_meta.stream_in_body if provider_meta else True

        # Provider 不使用 stream 字段（如 Gemini）：确保移除
        if not provider_uses_stream:
            request_body.pop("stream", None)
            return

        # 如果客户端格式不使用 stream 字段，但 Provider 格式需要：补齐
        if not client_uses_stream and provider_uses_stream:
            request_body["stream"] = is_stream
        elif "stream" not in request_body:
            # 保守兜底：目标需要 stream 且当前缺失时写入
            request_body["stream"] = is_stream

        # OpenAI Chat Completions: request usage in streaming mode.
        # When the client format doesn't carry a `stream` field (e.g. Gemini streaming endpoint),
        # the normalizer won't see internal.stream=True, so we need to add this here.
        provider_fmt = str(provider_api_format or "").strip().lower()
        if is_stream and provider_fmt == "openai:chat":
            stream_options = request_body.get("stream_options")
            if not isinstance(stream_options, dict):
                stream_options = {}
            stream_options["include_usage"] = True
            request_body["stream_options"] = stream_options

    async def _get_mapped_model(
        self,
        source_model: str,
        provider_id: str,
        api_format: str | None = None,
    ) -> str | None:
        """
        获取模型映射后的实际模型名

        Args:
            source_model: 用户请求的模型名
            provider_id: Provider ID
            api_format: Provider 侧 API 格式（用于过滤映射作用域，默认使用 handler FORMAT_ID）

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
            effective_format = api_format or self.FORMAT_ID
            mapped_name = mapping.model.select_provider_model_name(
                affinity_key, api_format=effective_format
            )
            logger.debug(f"[Chat] 模型映射: {source_model} -> {mapped_name}")
            return mapped_name

        return None

    # ==================== 流式处理 ====================

    async def process_stream(
        self,
        request: Any,
        http_request: Request,
        original_headers: dict[str, Any],
        original_request_body: dict[str, Any],
        query_params: dict[str, str] | None = None,
        client_content_encoding: str | None = None,
    ) -> StreamingResponse | JSONResponse:
        """处理流式响应"""
        logger.debug(f"开始流式响应处理 ({self.FORMAT_ID})")
        effective_client_content_encoding = resolve_client_content_encoding(
            original_headers,
            client_content_encoding,
        )

        # 转换请求格式
        converted_request = await self._convert_request(request)
        model = getattr(converted_request, "model", original_request_body.get("model", "unknown"))

        # 提前创建 pending 记录，让前端可以立即看到"处理中"
        pending_usage_created = self._create_pending_usage(
            model=model,
            is_stream=True,
            request_type="chat",
            api_format=self.FORMAT_ID,
            request_headers=original_headers,
            request_body=original_request_body,
        )
        api_format = self.allowed_api_formats[0]

        request_state = MutableRequestBodyState(original_request_body)

        # 创建类型安全的流式上下文
        ctx = StreamContext(
            model=model,
            api_format=api_format,
            api_family=self.api_family,
            endpoint_kind=self.endpoint_kind,
        )
        ctx.request_id = self.request_id
        ctx.client_api_format = (
            api_format.value if hasattr(api_format, "value") else str(api_format)
        )
        # 仅在 FULL 级别才需要保留 parsed_chunks，避免长流式响应导致的内存占用
        ctx.record_parsed_chunks = SystemConfigService.should_log_body(self.db)
        request_metadata = self._build_request_metadata()
        if request_metadata and isinstance(request_metadata.get("perf"), dict):
            ctx.perf_sampled = True
            ctx.perf_metrics.update(request_metadata["perf"])

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
        ) -> AsyncGenerator[bytes]:
            return await self._execute_stream_request(
                ctx,
                stream_processor,
                provider,
                endpoint,
                key,
                request_state.build_attempt_body(),
                original_headers,
                query_params,
                candidate,
                is_disconnected=http_request.is_disconnected,
                client_content_encoding=effective_client_content_encoding,
            )

        try:
            # 解析能力需求
            capability_requirements = self._resolve_capability_requirements(
                model_name=model,
                request_headers=original_headers,
                request_body=original_request_body,
            )
            preferred_key_ids = await self._resolve_preferred_key_ids(
                model_name=model,
                request_body=original_request_body,
            )

            # 统一入口：总是通过 TaskService
            from src.services.task import TaskService
            from src.services.task.core.context import TaskMode

            exec_result = await TaskService(self.db, self.redis).execute(
                task_type="chat",
                task_mode=TaskMode.SYNC,
                api_format=api_format,
                model_name=model,
                user_api_key=self.api_key,
                request_func=stream_request_func,
                request_id=self.request_id,
                is_stream=True,
                capability_requirements=capability_requirements or None,
                preferred_key_ids=preferred_key_ids or None,
                request_body_state=request_state,
                request_headers=original_headers,
                request_body=original_request_body,
                # 预创建失败时，回退到 TaskService 侧创建，避免丢失 pending 状态。
                create_pending_usage=not pending_usage_created,
            )
            stream_generator = exec_result.response
            provider_name = exec_result.provider_name or "unknown"
            attempt_id = exec_result.request_candidate_id
            provider_id = exec_result.provider_id
            endpoint_id = exec_result.endpoint_id
            key_id = exec_result.key_id

            # 更新上下文
            ctx.attempt_id = attempt_id
            ctx.provider_name = provider_name
            ctx.provider_id = provider_id
            ctx.endpoint_id = endpoint_id
            ctx.key_id = key_id
            if getattr(exec_result, "pool_summary", None):
                ctx.pool_summary = exec_result.pool_summary
            scheduling_metadata = (
                self._merge_scheduling_metadata(
                    {},
                    exec_result=exec_result,
                    selected_key_id=key_id,
                    fallback_from_request=False,
                )
                or {}
            )
            candidate_keys = scheduling_metadata.get("candidate_keys")
            if isinstance(candidate_keys, list):
                ctx.candidate_keys = candidate_keys
            scheduling_audit = scheduling_metadata.get("scheduling_audit")
            if isinstance(scheduling_audit, dict):
                ctx.scheduling_audit = scheduling_audit
            # 同步整流状态（如果请求体被整流过）
            ctx.rectified = request_state.is_rectified()

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
            client_headers = filter_proxy_response_headers(ctx.response_headers)
            # 添加/覆盖 SSE 必需的头（所有格式统一使用 SSE）
            client_headers.update(build_sse_headers())
            client_headers["content-type"] = "text/event-stream"

            return StreamingResponse(
                monitored_stream,
                media_type="text/event-stream",
                headers=client_headers,
                background=background_tasks,
            )

        except (ThinkingSignatureException, UpstreamClientException) as e:
            # ThinkingSignatureException: TaskService 层已处理整流重试但仍失败
            # UpstreamClientException: 上游客户端错误（HTTP 4xx），不重试，直接返回给客户端
            error_type = (
                "签名错误" if isinstance(e, ThinkingSignatureException) else "上游客户端错误"
            )
            self._log_request_error(f"流式请求失败（{error_type}）", e)
            from src.api.handlers.base.chat_sync_executor import ChatSyncExecutor

            await ChatSyncExecutor(self)._record_stream_failure(
                ctx, e, original_headers, original_request_body
            )
            client_format = (ctx.client_api_format or "").upper()
            provider_format = (ctx.provider_api_format or client_format).upper()
            payload = _build_error_json_payload(
                e, client_format, provider_format, needs_conversion=ctx.needs_conversion
            )
            return JSONResponse(
                status_code=_get_error_status_code(e),
                content=payload,
            )

        except Exception as e:
            self._log_request_error("流式请求失败", e)
            from src.api.handlers.base.chat_sync_executor import ChatSyncExecutor

            await ChatSyncExecutor(self)._record_stream_failure(
                ctx, e, original_headers, original_request_body
            )
            raise

    async def _prepare_provider_request(
        self,
        *,
        model: str,
        provider: Provider,
        endpoint: ProviderEndpoint,
        key: ProviderAPIKey,
        working_request_body: dict[str, Any],
        original_headers: dict[str, str],
        client_api_format: str,
        provider_api_format: str,
        candidate: ProviderCandidate | None,
        client_is_stream: bool,
    ) -> ProviderRequestResult:
        """
        构建 Provider 请求：模型映射、格式转换、envelope 包装。

        流式和非流式请求共享此逻辑，唯一差异是 client_is_stream 参数。
        """
        # 提前获取认证信息（动态格式判断需要使用 auth_config）
        auth_info = await get_provider_auth(endpoint, key)

        # 解析动态格式并计算 needs_conversion（Vertex AI 等跨格式 Provider）
        provider_api_format, needs_conversion = _resolve_dynamic_format(
            key, auth_info, model, provider_api_format, client_api_format, candidate
        )

        # 获取模型映射（优先使用映射匹配到的模型，其次是 Provider 级别的映射）
        mapped_model = candidate.mapping_matched_model if candidate else None
        if not mapped_model:
            mapped_model = await self._get_mapped_model(
                source_model=model,
                provider_id=str(provider.id),
                api_format=provider_api_format,
            )

        # `working_request_body` is already isolated per attempt.
        request_body = working_request_body
        if mapped_model:
            request_body = self.apply_mapped_model(request_body, mapped_model)

        provider_type = str(getattr(provider, "provider_type", "") or "").lower()
        behavior = get_provider_behavior(
            provider_type=provider_type,
            endpoint_sig=provider_api_format,
        )
        envelope = behavior.envelope
        same_format_variant = behavior.same_format_variant
        cross_format_variant = behavior.cross_format_variant

        # Upstream streaming policy (per-endpoint).
        upstream_policy = get_upstream_stream_policy(
            endpoint,
            provider_type=provider_type,
            endpoint_sig=str(provider_api_format),
        )
        upstream_is_stream = resolve_upstream_is_stream(
            client_is_stream=client_is_stream,
            policy=upstream_policy,
        )

        # Envelope lifecycle: prepare_context (pre-wrap hook).
        envelope_tls_profile: str | None = None
        if envelope and hasattr(envelope, "prepare_context"):
            envelope_tls_profile = envelope.prepare_context(
                provider_config=getattr(provider, "config", None),
                key_id=str(getattr(key, "id", "") or ""),
                user_api_key_id=str(getattr(self.api_key, "id", "") or ""),
                is_stream=upstream_is_stream,
                provider_id=str(getattr(provider, "id", "") or ""),
                key=key,
            )

        # 跨格式：先做请求体转换（失败触发 failover）
        registry = get_format_converter_registry()
        if needs_conversion:
            request_body = await registry.convert_request_async(
                request_body,
                str(client_api_format),
                str(provider_api_format),
                target_variant=cross_format_variant,
                output_limit=candidate.output_limit if candidate else None,
            )
            # 格式转换后，为需要 model 字段的格式设置模型名
            self._set_model_after_conversion(
                request_body,
                str(provider_api_format),
                mapped_model,
                model,
            )
            # 格式转换后，为需要 stream 字段的格式设置流式标志
            self._set_stream_after_conversion(
                request_body,
                str(client_api_format),
                str(provider_api_format),
                is_stream=upstream_is_stream,
            )
        else:
            # 同格式：按原逻辑做轻量清理（子类可覆盖以移除不需要的字段）
            request_body = self.prepare_provider_request_body(request_body)
            # 同格式 Provider 仍可能声明 target_variant
            if same_format_variant:
                request_body = registry.convert_request(
                    request_body,
                    str(provider_api_format),
                    str(provider_api_format),
                    target_variant=same_format_variant,
                )

        # 模型感知的请求后处理（如图像生成模型移除不兼容字段）
        request_body = self.finalize_provider_request(
            request_body,
            mapped_model=mapped_model,
            provider_api_format=str(provider_api_format) if provider_api_format else None,
        )

        # Force upstream stream/sync mode in request body (best-effort).
        if provider_api_format:
            enforce_stream_mode_for_upstream(
                request_body,
                provider_api_format=str(provider_api_format),
                upstream_is_stream=upstream_is_stream,
            )

        request_body = maybe_patch_request_with_prompt_cache_key(
            request_body,
            provider_api_format=str(provider_api_format) if provider_api_format else None,
            provider_type=provider_type,
            base_url=getattr(endpoint, "base_url", None),
            user_api_key_id=str(getattr(self.api_key, "id", "") or ""),
            request_headers=original_headers,
        )

        # 获取 URL 模型名
        url_model = self.get_model_for_url(request_body, mapped_model) or model

        # Provider envelope: wrap request.
        if envelope:
            request_body, url_model = envelope.wrap_request(
                request_body,
                model=url_model or model or "",
                url_model=url_model,
                decrypted_auth_config=auth_info.decrypted_auth_config if auth_info else None,
            )
            # Envelope lifecycle: post_wrap_request (post-wrap hook).
            if hasattr(envelope, "post_wrap_request"):
                await envelope.post_wrap_request(request_body)

        # Provider envelope: extra upstream headers (e.g. dedicated User-Agent).
        extra_headers: dict[str, str] = {}
        if envelope:
            extra_headers.update(envelope.extra_headers() or {})

        hook_headers = build_upstream_extra_headers(
            provider_type=provider_type,
            endpoint_sig=str(provider_api_format) if provider_api_format else None,
            request_body=request_body,
            original_headers=original_headers,
            decrypted_auth_config=auth_info.decrypted_auth_config if auth_info else None,
        )
        if hook_headers:
            extra_headers.update(hook_headers)

        return ProviderRequestResult(
            request_body=request_body,
            url_model=url_model,
            mapped_model=mapped_model,
            envelope=envelope,
            extra_headers=extra_headers,
            upstream_is_stream=upstream_is_stream,
            needs_conversion=needs_conversion,
            provider_api_format=provider_api_format,
            client_api_format=client_api_format,
            auth_info=auth_info,
            tls_profile=envelope_tls_profile,
        )

    async def _execute_stream_request(
        self,
        ctx: StreamContext,
        stream_processor: StreamProcessor,
        provider: Provider,
        endpoint: ProviderEndpoint,
        key: ProviderAPIKey,
        working_request_body: dict[str, Any],
        original_headers: dict[str, str],
        query_params: dict[str, str] | None = None,
        candidate: ProviderCandidate | None = None,
        is_disconnected: Callable[[], Awaitable[bool]] | None = None,
        client_content_encoding: str | None = None,
    ) -> AsyncGenerator[bytes]:
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
        ctx.provider_type = str(getattr(provider, "provider_type", "") or "")

        # ctx.api_format 是枚举，需要取 value 作为字符串
        _api_format_str = (
            ctx.api_format.value if hasattr(ctx.api_format, "value") else str(ctx.api_format)
        )
        provider_api_format = ctx.provider_api_format or _api_format_str
        client_api_format = ctx.client_api_format or _api_format_str

        # 构建 Provider 请求（模型映射、格式转换、envelope 包装）
        prep = await self._prepare_provider_request(
            model=ctx.model,
            provider=provider,
            endpoint=endpoint,
            key=key,
            working_request_body=working_request_body,
            original_headers=original_headers,
            client_api_format=client_api_format,
            provider_api_format=provider_api_format,
            candidate=candidate,
            client_is_stream=True,
        )
        provider_api_format = prep.provider_api_format
        needs_conversion = prep.needs_conversion
        ctx.provider_api_format = provider_api_format
        ctx.needs_conversion = needs_conversion
        mapped_model = prep.mapped_model
        if mapped_model:
            ctx.mapped_model = mapped_model
        request_body = prep.request_body
        url_model = prep.url_model
        envelope = prep.envelope
        upstream_is_stream = prep.upstream_is_stream
        auth_info = prep.auth_info
        tls_profile = prep.tls_profile

        # 构建请求（上游始终使用 header 认证，不跟随客户端的 query 方式）
        provider_payload, provider_headers = self._request_builder.build(
            request_body,
            original_headers,
            endpoint,
            key,
            is_stream=upstream_is_stream,
            extra_headers=prep.extra_headers if prep.extra_headers else None,
            pre_computed_auth=auth_info.as_tuple() if auth_info else None,
            envelope=envelope,
            provider_api_format=prep.provider_api_format,
        )
        if upstream_is_stream:
            from src.core.api_format.headers import set_accept_if_absent

            set_accept_if_absent(provider_headers)

        ctx.provider_request_headers = provider_headers
        ctx.provider_request_body = provider_payload

        url = build_provider_url(
            endpoint,
            query_params=query_params,
            path_params={"model": url_model},
            is_stream=upstream_is_stream,
            key=key,
            decrypted_auth_config=auth_info.decrypted_auth_config if auth_info else None,
        )
        # Capture the selected base_url from transport (used by some envelopes for failover).
        ctx.selected_base_url = envelope.capture_selected_base_url() if envelope else None

        # 解析有效代理（Key 级别优先于 Provider 级别）
        from src.services.proxy_node.resolver import (
            get_proxy_label,
            resolve_effective_proxy,
            resolve_proxy_info_async,
        )

        effective_proxy = resolve_effective_proxy(provider.proxy, getattr(key, "proxy", None))
        ctx.proxy_info = await resolve_proxy_info_async(effective_proxy)
        proxy_label = get_proxy_label(ctx.proxy_info)

        logger.debug(
            f"  [{self.request_id}] 发送流式请求: Provider={provider.name}, "
            f"模型={ctx.model} -> {mapped_model or '无映射'}, 代理={proxy_label}"
        )

        # If upstream is forced to non-stream mode, we execute a sync request and then
        # simulate streaming to the client (sync -> stream bridge).
        if not upstream_is_stream:
            from src.clients.http_client import HTTPClientPool
            from src.services.proxy_node.resolver import (
                build_post_kwargs_async,
                resolve_delegate_config_async,
            )

            request_timeout_sync = provider.request_timeout or config.http_request_timeout
            delegate_cfg = await resolve_delegate_config_async(effective_proxy)
            http_client = await HTTPClientPool.get_upstream_client(
                delegate_cfg,
                proxy_config=effective_proxy,
                tls_profile=tls_profile,
            )

            try:
                _pkw = await build_post_kwargs_async(
                    delegate_cfg,
                    url=url,
                    headers=provider_headers,
                    payload=provider_payload,
                    timeout=request_timeout_sync,
                    client_content_encoding=client_content_encoding,
                )
                resp = await http_client.post(**_pkw)
            except (httpx.ConnectError, httpx.ConnectTimeout, httpx.TimeoutException) as e:
                if envelope:
                    envelope.on_connection_error(base_url=ctx.selected_base_url, exc=e)
                    if ctx.selected_base_url:
                        logger.warning(
                            f"[{envelope.name}] Connection error: {ctx.selected_base_url} ({e})"
                        )
                raise

            ctx.status_code = resp.status_code
            ctx.response_headers = dict(resp.headers)
            ctx.set_proxy_timing(ctx.response_headers)
            if envelope:
                envelope.on_http_status(base_url=ctx.selected_base_url, status_code=ctx.status_code)

            # Reuse HTTPStatusError classification path (handled by TaskService/error_classifier).
            try:
                resp.raise_for_status()
            except httpx.HTTPStatusError as e:
                # OAuth token may be revoked/expired earlier than expires_at indicates.
                # Best-effort: force refresh once on 401 and retry a single time.
                if (
                    resp.status_code == 401
                    and str(getattr(key, "auth_type", "") or "").lower() == "oauth"
                ):
                    refreshed_auth = await get_provider_auth(endpoint, key, force_refresh=True)
                    if refreshed_auth:
                        provider_headers[refreshed_auth.auth_header] = refreshed_auth.auth_value
                        ctx.provider_request_headers = provider_headers

                    # retry once
                    _pkw = await build_post_kwargs_async(
                        delegate_cfg,
                        url=url,
                        headers=provider_headers,
                        payload=provider_payload,
                        timeout=request_timeout_sync,
                        client_content_encoding=client_content_encoding,
                        refresh_auth=True,
                    )
                    resp = await http_client.post(**_pkw)
                    ctx.status_code = resp.status_code
                    ctx.response_headers = dict(resp.headers)
                    ctx.set_proxy_timing(ctx.response_headers)
                    if envelope:
                        envelope.on_http_status(
                            base_url=ctx.selected_base_url, status_code=ctx.status_code
                        )
                    try:
                        resp.raise_for_status()
                    except httpx.HTTPStatusError as e2:
                        error_body = ""
                        try:
                            error_body = resp.text[:4000] if resp.text else ""
                        except Exception:
                            error_body = ""
                        e2.upstream_response = error_body  # type: ignore[attr-defined]
                        raise
                else:
                    error_body = ""
                    try:
                        error_body = resp.text[:4000] if resp.text else ""
                    except Exception:
                        error_body = ""
                    e.upstream_response = error_body  # type: ignore[attr-defined]
                    raise

            # Safe JSON parsing.
            try:
                response_json = resp.json()
            except (UnicodeDecodeError, json.JSONDecodeError) as e:
                raw_content = ""
                try:
                    raw_content = resp.text[:500] if resp.text else "(empty)"
                except Exception:
                    try:
                        raw_content = repr(resp.content[:500]) if resp.content else "(empty)"
                    except Exception:
                        raw_content = "(unable to read)"
                raise ProviderNotAvailableException(
                    "上游服务返回了无效的响应",
                    provider_name=str(provider.name),
                    upstream_status=resp.status_code,
                    upstream_response=f"json_decode_error={type(e).__name__}: {raw_content}",
                )

            if envelope:
                response_json = envelope.unwrap_response(response_json)
                envelope.postprocess_unwrapped_response(model=ctx.model, data=response_json)

            # Embedded error detection (HTTP 200 but error body).
            if isinstance(response_json, dict):
                parser = get_parser_for_format(provider_api_format)
                if parser.is_error_response(response_json):
                    parsed = parser.parse_response(response_json, 200)
                    raise EmbeddedErrorException(
                        provider_name=str(provider.name),
                        error_code=parsed.embedded_status_code,
                        error_message=parsed.error_message,
                        error_status=parsed.error_type,
                    )

            # Convert sync JSON -> InternalResponse, then InternalResponse -> client stream events.
            registry = get_format_converter_registry()
            src_norm = (
                registry.get_normalizer(str(provider_api_format)) if provider_api_format else None
            )
            if src_norm is None:
                raise RuntimeError(f"未注册 Normalizer: {provider_api_format}")

            internal_resp = src_norm.response_to_internal(
                response_json if isinstance(response_json, dict) else {}
            )
            internal_resp.model = str(ctx.model or internal_resp.model or "")
            if internal_resp.id:
                ctx.response_id = internal_resp.id

            if internal_resp.usage:
                ctx.input_tokens = int(internal_resp.usage.input_tokens or 0)
                ctx.output_tokens = int(internal_resp.usage.output_tokens or 0)
                ctx.cached_tokens = int(internal_resp.usage.cache_read_tokens or 0)
                ctx.cache_creation_tokens = int(internal_resp.usage.cache_write_tokens or 0)

            from src.core.api_format.conversion.stream_state import StreamState

            tgt_norm = (
                registry.get_normalizer(str(client_api_format)) if client_api_format else None
            )
            if tgt_norm is None:
                raise RuntimeError(f"未注册 Normalizer: {client_api_format}")

            state = StreamState(
                model=str(ctx.model or ""),
                message_id=str(ctx.response_id or ctx.request_id or self.request_id or ""),
            )

            output_state = {"started": False}

            # 保留提供商原始响应到 provider_parsed_chunks
            if ctx.record_parsed_chunks and isinstance(response_json, dict):
                ctx.provider_parsed_chunks.append(response_json)

            async def _streamified() -> AsyncGenerator[bytes]:
                for ev in iter_internal_response_as_stream_events(internal_resp):
                    converted_events = tgt_norm.stream_event_from_internal(ev, state)
                    if not converted_events:
                        continue
                    for evt in converted_events:
                        if isinstance(evt, dict):
                            ctx.data_count += 1
                            if ctx.record_parsed_chunks:
                                ctx.parsed_chunks.append(evt)
                        payload = json.dumps(evt, ensure_ascii=False)
                        ctx.chunk_count += 1
                        if not output_state["started"]:
                            ctx.record_first_byte_time(self.start_time)
                            if stream_processor.on_streaming_start:
                                stream_processor.on_streaming_start()
                            output_state["started"] = True
                        yield f"data: {payload}\n\n".encode("utf-8")

                # OpenAI chat clients expect a final [DONE] marker.
                if str(client_api_format or "").strip().lower() == "openai:chat":
                    if not output_state["started"]:
                        ctx.record_first_byte_time(self.start_time)
                        if stream_processor.on_streaming_start:
                            stream_processor.on_streaming_start()
                        output_state["started"] = True
                    ctx.chunk_count += 1
                    yield b"data: [DONE]\n\n"
                    ctx.has_completion = True

            return _streamified()

        # 流式请求使用 stream_first_byte_timeout 作为首字节超时
        # 优先使用 Provider 配置，否则使用全局配置
        request_timeout = provider.stream_first_byte_timeout or config.stream_first_byte_timeout

        # 获取 HTTP 客户端（支持代理配置，Key 级别优先于 Provider 级别）
        # 使用连接池复用客户端，避免每次流式请求都新建 TCP/TLS 连接
        from src.clients.http_client import HTTPClientPool
        from src.services.proxy_node.resolver import (
            build_stream_kwargs_async,
            resolve_delegate_config_async,
        )

        delegate_cfg = await resolve_delegate_config_async(effective_proxy)
        http_client = await HTTPClientPool.get_upstream_client(
            delegate_cfg,
            proxy_config=effective_proxy,
            tls_profile=tls_profile,
        )

        # 用于存储内部函数的结果（必须在函数定义前声明，供 nonlocal 使用）
        byte_iterator: Any = None
        prefetched_chunks: Any = None
        response_ctx: Any = None

        async def _connect_and_prefetch() -> None:
            """建立连接并预读首字节（受整体超时控制）"""
            nonlocal byte_iterator, prefetched_chunks, response_ctx
            _skw = await build_stream_kwargs_async(
                delegate_cfg,
                url=url,
                headers=provider_headers,
                payload=provider_payload,
                client_content_encoding=client_content_encoding,
                # 流式请求不应使用 provider.request_timeout 作为“整条流总时长”超时，
                # 否则会在长响应中途被硬切断（常见于 request_timeout=15s 的配置）。
                # 首字节超时由外层 wait_for(stream_first_byte_timeout) 控制；
                # 后续分块读取由 http client 默认 read timeout（长超时）控制。
                timeout=None,
            )
            response_ctx = http_client.stream(**_skw)
            stream_response = await response_ctx.__aenter__()

            ctx.status_code = stream_response.status_code
            ctx.response_headers = dict(stream_response.headers)
            ctx.set_proxy_timing(ctx.response_headers)
            if envelope:
                envelope.on_http_status(
                    base_url=ctx.selected_base_url,
                    status_code=ctx.status_code,
                )

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

        for attempt in range(2):
            try:
                # 使用 asyncio.wait_for 包裹整个"建立连接 + 获取首字节"阶段
                # stream_first_byte_timeout 控制首字节超时，避免上游长时间无响应
                # 同时检测客户端断连，避免客户端已断开但服务端仍在等待上游响应
                if is_disconnected is not None:
                    await wait_for_with_disconnect_detection(
                        _connect_and_prefetch(),
                        timeout=request_timeout,
                        is_disconnected=is_disconnected,
                        request_id=self.request_id,
                    )
                else:
                    await asyncio.wait_for(_connect_and_prefetch(), timeout=request_timeout)
                break

            except ClientDisconnectedException:
                # 客户端断开连接，清理响应上下文（不关闭池中复用的客户端）
                if response_ctx is not None:
                    try:
                        await response_ctx.__aexit__(None, None, None)
                    except Exception:
                        pass
                logger.warning(f"  [{self.request_id}] 客户端在等待首字节时断开连接")
                ctx.status_code = 499
                ctx.error_message = "client_disconnected_during_prefetch"
                raise

            except TimeoutError:
                # 整体请求超时（建立连接 + 获取首字节）
                # 清理可能已建立的连接上下文（不关闭池中复用的客户端）
                if response_ctx is not None:
                    try:
                        await response_ctx.__aexit__(None, None, None)
                    except Exception:
                        pass
                logger.warning(
                    f"  [{self.request_id}] 请求超时: Provider={provider.name}, timeout={request_timeout}s"
                )
                raise ProviderTimeoutException(
                    provider_name=str(provider.name),
                    timeout=int(request_timeout),
                )

            except (httpx.ConnectError, httpx.ConnectTimeout, httpx.TimeoutException) as e:
                # 连接/读写超时：清理可能已建立的连接上下文（不关闭池中复用的客户端）
                if response_ctx is not None:
                    try:
                        await response_ctx.__aexit__(None, None, None)
                    except Exception:
                        pass
                if envelope:
                    envelope.on_connection_error(base_url=ctx.selected_base_url, exc=e)
                    if ctx.selected_base_url:
                        logger.warning(
                            f"[{envelope.name}] Connection error: {ctx.selected_base_url} ({e})"
                        )
                raise

            except httpx.HTTPStatusError as e:
                status = int(getattr(e.response, "status_code", 0) or 0)
                if (
                    attempt == 0
                    and status == 401
                    and str(getattr(key, "auth_type", "") or "").lower() == "oauth"
                ):
                    # OAuth token may be revoked/expired earlier than expires_at indicates.
                    # Best-effort: force refresh once on 401 and retry a single time.
                    try:
                        if response_ctx is not None:
                            await response_ctx.__aexit__(None, None, None)
                    except Exception:
                        pass

                    refreshed_auth = await get_provider_auth(endpoint, key, force_refresh=True)
                    if refreshed_auth:
                        provider_headers[refreshed_auth.auth_header] = refreshed_auth.auth_value
                        ctx.provider_request_headers = provider_headers

                    # Reset state for the next attempt.
                    byte_iterator = None
                    prefetched_chunks = None
                    response_ctx = None
                    continue

                from src.api.handlers.base.chat_sync_executor import ChatSyncExecutor

                error_text = await ChatSyncExecutor(self)._extract_error_text(e)

                try:
                    if response_ctx is not None:
                        await response_ctx.__aexit__(None, None, None)
                except Exception:
                    pass
                finally:
                    response_ctx = None
                logger.error(
                    f"Provider 返回错误: {e.response.status_code}\n  Response: {error_text}"
                )
                # 将上游错误信息附加到异常，以便故障转移时能够返回给客户端
                e.upstream_response = error_text  # type: ignore[attr-defined]
                raise

            except EmbeddedErrorException:
                try:
                    if response_ctx is not None:
                        await response_ctx.__aexit__(None, None, None)
                except Exception:
                    pass
                raise

            except Exception:
                try:
                    if response_ctx is not None:
                        await response_ctx.__aexit__(None, None, None)
                except Exception:
                    pass
                finally:
                    response_ctx = None
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
            prefetched_chunks,
            start_time=self.start_time,
        )

    # ==================== 非流式处理 ====================

    async def process_sync(
        self,
        request: Any,
        http_request: Request,
        original_headers: dict[str, Any],
        original_request_body: dict[str, Any],
        query_params: dict[str, str] | None = None,
        client_content_encoding: str | None = None,
        client_accept_encoding: str | None = None,
    ) -> JSONResponse:
        """处理非流式响应"""
        from src.api.handlers.base.chat_sync_executor import ChatSyncExecutor

        executor = ChatSyncExecutor(self)
        return await executor.execute(
            request,
            http_request,
            original_headers,
            original_request_body,
            query_params,
            client_content_encoding=client_content_encoding,
            client_accept_encoding=client_accept_encoding,
        )
