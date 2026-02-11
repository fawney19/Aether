"""
CLI Message Handler 通用基类

将 CLI 格式处理器的通用逻辑（HTTP 请求、SSE 解析、统计记录）抽取到基类，
子类只需实现格式特定的事件解析逻辑。

设计目标：
1. 减少代码重复 - 原来每个 CLI Handler 900+ 行，抽取后子类只需 ~100 行
2. 统一错误处理 - 超时、空流、故障转移等逻辑集中管理
3. 简化新格式接入 - 只需实现 ResponseParser 和少量钩子方法
"""

from __future__ import annotations

import asyncio
import codecs
import time
from collections.abc import AsyncGenerator, Callable
from typing import (
    TYPE_CHECKING,
    Any,
)

import httpx
from fastapi import BackgroundTasks, Request
from fastapi.responses import JSONResponse, StreamingResponse
from sqlalchemy.orm import Session

from src.utils import json_helper as json

if TYPE_CHECKING:
    from src.core.api_format import EndpointDefinition

from src.api.handlers.base.base_handler import (
    BaseMessageHandler,
    ClientDisconnectedException,
    MessageTelemetry,
    wait_for_with_disconnect_detection,
)
from src.api.handlers.base.parsers import get_parser_for_format
from src.api.handlers.base.request_builder import PassthroughRequestBuilder, get_provider_auth

# 直接从具体模块导入，避免循环依赖
from src.api.handlers.base.response_parser import (
    ResponseParser,
)
from src.api.handlers.base.stream_context import (
    StreamContext,
    extract_proxy_timing,
    is_format_converted,
)
from src.api.handlers.base.upstream_stream_bridge import (
    aggregate_upstream_stream_to_internal_response,
)
from src.api.handlers.base.utils import (
    build_sse_headers,
    check_html_response,
    check_prefetched_response_error,
    filter_proxy_response_headers,
    get_format_converter_registry,
)
from src.config.constants import StreamDefaults
from src.config.settings import config
from src.core.api_format.conversion.stream_bridge import (
    iter_internal_response_as_stream_events,
)
from src.core.error_utils import extract_client_error_message
from src.core.exceptions import (
    EmbeddedErrorException,
    ProviderAuthException,
    ProviderNotAvailableException,
    ProviderRateLimitException,
    ProviderTimeoutException,
    ThinkingSignatureException,
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
from src.services.provider.behavior import get_provider_behavior
from src.services.provider.stream_policy import (
    enforce_stream_mode_for_upstream,
    get_upstream_stream_policy,
    resolve_upstream_is_stream,
)
from src.services.provider.transport import build_provider_url
from src.services.system.config import SystemConfigService
from src.utils.sse_parser import SSEEventParser
from src.utils.timeout import read_first_chunk_with_ttfb_timeout

# ==============================================================================
# 辅助函数
# ==============================================================================


def _parse_sse_data_line(line: str) -> tuple[Any | None, str]:
    """
    解析标准 SSE data 行

    Args:
        line: 以 "data:" 开头的 SSE 行

    Returns:
        (parsed_json, status) 元组：
        - (parsed_dict, "ok") - 解析成功
        - (None, "empty") - 内容为空
        - (None, "invalid") - JSON 解析失败，调用方应透传原始行
    """
    data_content = line[5:].strip()
    if not data_content:
        return None, "empty"
    try:
        return json.loads(data_content), "ok"
    except json.JSONDecodeError:
        return None, "invalid"


def _parse_sse_event_data_line(line: str) -> tuple[Any | None, str]:
    """
    解析 event + data 同行格式（如 "event: xxx data: {...}"）

    Args:
        line: 以 "event:" 开头且包含 " data:" 的 SSE 行

    Returns:
        (parsed_json, status) 元组
    """
    _event_part, data_part = line.split(" data:", 1)
    data_content = data_part.strip()
    try:
        return json.loads(data_content), "ok"
    except json.JSONDecodeError:
        return None, "invalid"


def _parse_gemini_json_array_line(line: str) -> tuple[Any | None, str]:
    """
    解析 Gemini JSON-array 格式的裸 JSON 行

    Gemini 流式响应可能是 JSON 数组格式，每行是数组元素。

    Args:
        line: 原始行（可能是 "[", "]", ",", 或 JSON 对象）

    Returns:
        (parsed_json, status) 元组
    """
    stripped = line.strip()
    if stripped in ("", "[", "]", ","):
        return None, "skip"

    candidate = stripped.lstrip(",").rstrip(",").strip()
    try:
        return json.loads(candidate), "ok"
    except json.JSONDecodeError:
        logger.debug(f"Gemini JSON-array line skip: {stripped[:50]}")
        return None, "invalid"


def _format_converted_events_to_sse(
    converted_events: list[dict[str, Any]],
    client_format: str,
) -> list[str]:
    """
    将转换后的事件格式化为 SSE 行

    Args:
        converted_events: 转换后的事件列表
        client_format: 客户端 API 格式

    Returns:
        SSE 行列表（每个元素是完整的 SSE 事件，包含尾部空行）
    """
    result: list[str] = []
    needs_event_line = str(client_format or "").strip().lower().startswith("claude:")

    for evt in converted_events:
        payload = json.dumps(evt, ensure_ascii=False)
        if needs_event_line:
            evt_type = evt.get("type") if isinstance(evt, dict) else None
            if isinstance(evt_type, str) and evt_type:
                # Claude 格式：event + data + 空行
                result.append(f"event: {evt_type}\ndata: {payload}\n")
            else:
                result.append(f"data: {payload}\n")
        else:
            # OpenAI 格式：data + 空行
            result.append(f"data: {payload}\n")

    return result


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
        allowed_api_formats: list | None = None,
        adapter_detector: None | (
            Callable[[dict[str, str], dict[str, Any] | None], dict[str, bool]]
        ) = None,
        perf_metrics: dict[str, Any] | None = None,
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
        )
        self._parser: ResponseParser | None = None
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
    ) -> str | None:
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

    @staticmethod
    def _get_format_metadata(format_id: str) -> "EndpointDefinition | None":
        """获取 endpoint 元数据（解析失败返回 None）"""
        from src.core.api_format.metadata import resolve_endpoint_definition

        return resolve_endpoint_definition(format_id)

    def _finalize_converted_request(
        self,
        request_body: dict[str, Any],
        client_api_format: str,
        provider_api_format: str,
        mapped_model: str | None,
        fallback_model: str,
        is_stream: bool,
    ) -> None:
        """
        跨格式转换后统一设置并清理 model/stream 字段（原地修改）

        处理逻辑：
        1. 根据目标格式决定是否在 body 中设置 model
        2. 若客户端格式不含 stream 字段但 Provider 需要，则显式设置
        3. 移除目标格式不允许在 body 中携带的字段（如 Gemini 的 model/stream）

        Args:
            request_body: 转换后的请求体（会被原地修改）
            client_api_format: 客户端 API 格式
            provider_api_format: Provider API 格式
            mapped_model: 映射后的模型名
            fallback_model: 备用模型名
            is_stream: 是否流式请求
        """
        client_meta = self._get_format_metadata(client_api_format)
        provider_meta = self._get_format_metadata(provider_api_format)

        # 默认：model_in_body=True, stream_in_body=True（如 OpenAI/Claude）
        client_uses_stream = client_meta.stream_in_body if client_meta else True
        provider_model_in_body = provider_meta.model_in_body if provider_meta else True
        provider_stream_in_body = provider_meta.stream_in_body if provider_meta else True

        # 设置 model（仅当 Provider 允许且 body 中需要）
        if provider_model_in_body:
            request_body["model"] = mapped_model or fallback_model
        else:
            request_body.pop("model", None)

        # 设置 stream（客户端不带但 Provider 需要时显式设置；Provider 不需要时移除）
        if provider_stream_in_body:
            if not client_uses_stream:
                request_body["stream"] = is_stream
        else:
            request_body.pop("stream", None)

        # OpenAI Chat Completions: request usage in streaming mode.
        provider_fmt = str(provider_api_format or "").strip().lower()
        if is_stream and provider_fmt == "openai:chat":
            stream_options = request_body.get("stream_options")
            if not isinstance(stream_options, dict):
                stream_options = {}
            stream_options["include_usage"] = True
            request_body["stream_options"] = stream_options

    def _convert_request_for_cross_format(
        self,
        request_body: dict[str, Any],
        client_api_format: str,
        provider_api_format: str,
        mapped_model: str | None,
        fallback_model: str,
        is_stream: bool,
        *,
        target_variant: str | None = None,
    ) -> tuple[dict[str, Any], str]:
        """
        跨格式请求转换的公共逻辑

        将客户端格式的请求体转换为 Provider 格式，并处理 model/stream 字段的补齐和清理。

        Args:
            request_body: 原始请求体（会被修改）
            client_api_format: 客户端 API 格式
            provider_api_format: Provider API 格式
            mapped_model: 映射后的模型名
            fallback_model: 备用模型名（通常是原始请求的 model）
            is_stream: 是否流式请求
            target_variant: 目标变体（如 "codex"），用于同格式但有细微差异的上游

        Returns:
            (转换后的请求体, 用于 URL 的模型名)
        """
        registry = get_format_converter_registry()
        converted_body = registry.convert_request(
            request_body,
            str(client_api_format),
            str(provider_api_format),
            target_variant=target_variant,
        )

        # 先计算 URL 模型（在清理 body 中的 model 字段之前）
        url_model = (
            self.get_model_for_url(converted_body, mapped_model) or mapped_model or fallback_model
        )

        # 统一设置并清理 model/stream 字段
        self._finalize_converted_request(
            converted_body,
            str(client_api_format),
            str(provider_api_format),
            mapped_model,
            fallback_model,
            is_stream,
        )

        return converted_body, url_model

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
            用于 URL 路径的模型名，默认优先使用映射后的名称
        """
        return mapped_model or request_body.get("model")

    def _extract_response_metadata(
        self,
        response: dict[str, Any],
    ) -> dict[str, Any]:
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
        original_request_body: dict[str, Any],
        original_headers: dict[str, str],
        query_params: dict[str, str] | None = None,
        path_params: dict[str, Any] | None = None,
        http_request: Request | None = None,
    ) -> StreamingResponse:
        """
        处理流式请求

        通用流程：
        1. 创建流上下文
        2. 定义请求函数（供 TaskService/FailoverEngine 调用）
        3. 执行请求并返回 StreamingResponse
        4. 后台任务记录统计信息

        Args:
            original_request_body: 原始请求体
            original_headers: 原始请求头
            query_params: 查询参数
            path_params: 路径参数
            http_request: FastAPI Request 对象，用于检测客户端断连
        """
        logger.debug(f"开始流式响应处理 ({self.FORMAT_ID})")

        # 可变请求体容器：允许 TaskService 在遇到 Thinking 签名错误时整流请求体后重试
        # 结构: {"body": 实际请求体, "_rectified": 是否已整流, "_rectified_this_turn": 本轮是否整流}
        request_body_ref: dict[str, Any] = {"body": original_request_body}

        # 使用子类实现的方法提取 model（不同 API 格式的 model 位置不同）
        # 注意：使用 original_request_body，因为整流只修改 messages，不影响 model 字段
        model = self.extract_model_from_request(original_request_body, path_params)

        # 提前创建 pending 记录，让前端可以立即看到"处理中"
        self._create_pending_usage(
            model=model,
            is_stream=True,
            request_type="chat",
            api_format=self.FORMAT_ID,
            request_headers=original_headers,
            request_body=original_request_body,
        )

        # 创建流上下文
        ctx = StreamContext(
            model=model,
            api_format=self.allowed_api_formats[0],
            request_id=self.request_id,
            user_id=self.user.id,
            api_key_id=self.api_key.id,
        )
        # 仅在 FULL 级别才需要保留 parsed_chunks，避免长流式响应导致的内存占用
        ctx.record_parsed_chunks = SystemConfigService.should_log_body(self.db)
        request_metadata = self._build_request_metadata(http_request)
        if request_metadata and isinstance(request_metadata.get("perf"), dict):
            ctx.perf_sampled = True
            ctx.perf_metrics.update(request_metadata["perf"])

        # 定义请求函数
        async def stream_request_func(
            provider: Provider,
            endpoint: ProviderEndpoint,
            key: ProviderAPIKey,
            candidate: ProviderCandidate,
        ) -> AsyncGenerator[bytes]:
            return await self._execute_stream_request(
                ctx,
                provider,
                endpoint,
                key,
                request_body_ref["body"],  # 使用容器中的请求体
                original_headers,
                query_params,
                candidate,
                http_request,  # 传递 http_request 用于断连检测
            )

        try:
            # 解析能力需求
            capability_requirements = self._resolve_capability_requirements(
                model_name=ctx.model,
                request_headers=original_headers,
                request_body=original_request_body,
            )
            preferred_key_ids = await self._resolve_preferred_key_ids(
                model_name=ctx.model,
                request_body=original_request_body,
            )

            # 统一入口：总是通过 TaskService
            from src.services.task import TaskService
            from src.services.task.context import TaskMode

            exec_result = await TaskService(self.db, self.redis).execute(
                task_type="cli",
                task_mode=TaskMode.SYNC,
                api_format=ctx.api_format,
                model_name=ctx.model,
                user_api_key=self.api_key,
                request_func=stream_request_func,
                request_id=self.request_id,
                is_stream=True,
                capability_requirements=capability_requirements or None,
                preferred_key_ids=preferred_key_ids or None,
                request_body_ref=request_body_ref,
            )
            stream_generator = exec_result.response
            provider_name = exec_result.provider_name or "unknown"
            attempt_id = exec_result.request_candidate_id
            provider_id = exec_result.provider_id
            endpoint_id = exec_result.endpoint_id
            key_id = exec_result.key_id

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
            # 同步整流状态（如果请求体被整流过）
            ctx.rectified = request_body_ref.get("_rectified", False)

            # 创建后台任务记录统计
            background_tasks = BackgroundTasks()
            background_tasks.add_task(
                self._record_stream_stats,
                ctx,
                original_headers,
                original_request_body,
            )

            # 创建监控流（传递 http_request 用于断连检测）
            monitored_stream = self._create_monitored_stream(ctx, stream_generator, http_request)

            # 透传提供商的响应头给客户端
            # 同时添加必要的 SSE 头以确保流式传输正常工作
            client_headers = filter_proxy_response_headers(ctx.response_headers)
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

        except ThinkingSignatureException as e:
            # Thinking 签名错误：TaskService 层已处理整流重试但仍失败
            # 记录 original_request_body（客户端原始请求），便于排查问题根因
            self._log_request_error("流式请求失败（签名错误）", e)
            await self._record_stream_failure(ctx, e, original_headers, original_request_body)
            raise

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
        original_request_body: dict[str, Any],
        original_headers: dict[str, str],
        query_params: dict[str, str] | None = None,
        candidate: ProviderCandidate | None = None,
        http_request: Request | None = None,
    ) -> AsyncGenerator[bytes]:
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
        ctx.selected_base_url = None  # 重置本次请求选用的 base_url（重试时避免污染）

        # 记录 Provider 信息
        ctx.provider_name = str(provider.name)
        ctx.provider_id = str(provider.id)
        ctx.provider_type = str(getattr(provider, "provider_type", "") or "")
        ctx.endpoint_id = str(endpoint.id)
        ctx.key_id = str(key.id)

        # 记录格式转换信息
        ctx.provider_api_format = str(endpoint.api_format) if endpoint.api_format else ""
        ctx.client_api_format = ctx.api_format  # 已在 process_stream 中设置

        # 获取模型映射（优先使用映射匹配到的模型，其次是 Provider 级别的映射）
        mapped_model = candidate.mapping_matched_model if candidate else None
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

        client_api_format = (
            ctx.client_api_format.value
            if hasattr(ctx.client_api_format, "value")
            else str(ctx.client_api_format)
        )
        provider_api_format = str(ctx.provider_api_format or "")
        needs_conversion = (
            bool(getattr(candidate, "needs_conversion", False)) if candidate else False
        )
        ctx.needs_conversion = needs_conversion

        provider_type = str(getattr(provider, "provider_type", "") or "").lower()
        behavior = get_provider_behavior(
            provider_type=provider_type,
            endpoint_sig=provider_api_format,
        )
        envelope = behavior.envelope
        target_variant = behavior.same_format_variant
        # 跨格式转换也允许变体（Antigravity 需要保留/翻译 Claude thinking 块）
        conversion_variant = behavior.cross_format_variant

        # Upstream streaming policy (per-endpoint): may force upstream to sync/stream mode.
        upstream_policy = get_upstream_stream_policy(
            endpoint,
            provider_type=provider_type,
            endpoint_sig=provider_api_format,
        )
        upstream_is_stream = resolve_upstream_is_stream(
            client_is_stream=True,
            policy=upstream_policy,
        )

        # 跨格式：先做请求体转换（失败触发 failover）
        if needs_conversion and provider_api_format:
            request_body, url_model = self._convert_request_for_cross_format(
                request_body,
                client_api_format,
                provider_api_format,
                mapped_model,
                ctx.model,
                is_stream=upstream_is_stream,
                target_variant=conversion_variant,
            )
        else:
            # 同格式：按原逻辑做轻量清理（子类可覆盖）
            request_body = self.prepare_provider_request_body(request_body)
            url_model = (
                self.get_model_for_url(request_body, mapped_model) or mapped_model or ctx.model
            )
            # 同格式时也需要应用 target_variant 转换（如 Codex）
            if target_variant and provider_api_format:
                registry = get_format_converter_registry()
                request_body = registry.convert_request(
                    request_body,
                    provider_api_format,
                    provider_api_format,
                    target_variant=target_variant,
                )

        # 模型感知的请求后处理（如图像生成模型移除不兼容字段）
        request_body = self.finalize_provider_request(
            request_body,
            mapped_model=mapped_model,
            provider_api_format=provider_api_format,
        )

        # Force upstream stream/sync mode in request body (best-effort).
        if provider_api_format:
            enforce_stream_mode_for_upstream(
                request_body,
                provider_api_format=provider_api_format,
                upstream_is_stream=upstream_is_stream,
            )

        # 获取认证信息（处理 Service Account 等异步认证场景）
        auth_info = await get_provider_auth(endpoint, key)

        # Provider envelope: wrap request after auth is available and before RequestBuilder.build().
        if envelope:
            request_body, url_model = envelope.wrap_request(
                request_body,
                model=url_model or ctx.model or "",
                url_model=url_model,
                decrypted_auth_config=auth_info.decrypted_auth_config if auth_info else None,
            )

        # Provider envelope: extra upstream headers (e.g. dedicated User-Agent).
        extra_headers: dict[str, str] = {}
        if envelope:
            extra_headers.update(envelope.extra_headers() or {})

        # 使用 RequestBuilder 构建请求体和请求头
        # 注意：mapped_model 已经应用到 request_body，这里不再传递
        # 上游始终使用 header 认证，不跟随客户端的 query 方式
        provider_payload, provider_headers = self._request_builder.build(
            request_body,
            original_headers,
            endpoint,
            key,
            is_stream=upstream_is_stream,
            extra_headers=extra_headers if extra_headers else None,
            pre_computed_auth=auth_info.as_tuple() if auth_info else None,
        )
        if upstream_is_stream:
            from src.core.api_format.headers import set_accept_if_absent

            set_accept_if_absent(provider_headers)

        # 保存发送给 Provider 的请求信息（用于调试和统计）
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
        from src.services.proxy_node.resolver import get_proxy_label as _gpl
        from src.services.proxy_node.resolver import resolve_effective_proxy as _rep
        from src.services.proxy_node.resolver import resolve_proxy_info as _rpi

        effective_proxy = _rep(provider.proxy, getattr(key, "proxy", None))
        ctx.proxy_info = _rpi(effective_proxy)

        # If upstream is forced to non-stream mode, we execute a sync request and then
        # simulate streaming to the client (sync -> stream bridge).
        if not upstream_is_stream:
            from src.clients.http_client import HTTPClientPool
            from src.services.proxy_node.resolver import build_post_kwargs, resolve_delegate_config

            request_timeout_sync = provider.request_timeout or config.http_request_timeout
            delegate_cfg = resolve_delegate_config(effective_proxy)
            http_client = await HTTPClientPool.get_upstream_client(
                delegate_cfg, proxy_config=effective_proxy
            )

            try:
                _pkw = build_post_kwargs(
                    delegate_cfg,
                    url=url,
                    headers=provider_headers,
                    payload=provider_payload,
                    timeout=request_timeout_sync,
                )
                _connect_start = time.monotonic()
                resp = await http_client.post(**_pkw)
                ctx.set_ttfb_ms(int((time.monotonic() - _connect_start) * 1000))
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
                    _pkw = build_post_kwargs(
                        delegate_cfg,
                        url=url,
                        headers=provider_headers,
                        payload=provider_payload,
                        timeout=request_timeout_sync,
                        refresh_auth=True,
                    )
                    _connect_start = time.monotonic()
                    resp = await http_client.post(**_pkw)
                    ctx.set_ttfb_ms(int((time.monotonic() - _connect_start) * 1000))
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
            if isinstance(response_json, dict) and provider_api_format:
                parser = get_parser_for_format(provider_api_format)
                if parser.is_error_response(response_json):
                    parsed = parser.parse_response(response_json, 200)
                    raise EmbeddedErrorException(
                        provider_name=str(provider.name),
                        error_code=parsed.embedded_status_code,
                        error_message=parsed.error_message,
                        error_status=parsed.error_type,
                    )

            # Extract Provider response metadata (best-effort).
            if isinstance(response_json, dict):
                ctx.response_metadata = self._extract_response_metadata(response_json)

            # Convert sync JSON -> InternalResponse, then InternalResponse -> client stream events.
            registry = get_format_converter_registry()
            src_norm = registry.get_normalizer(provider_api_format) if provider_api_format else None
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

            tgt_norm = registry.get_normalizer(client_api_format) if client_api_format else None
            if tgt_norm is None:
                raise RuntimeError(f"未注册 Normalizer: {client_api_format}")

            state = StreamState(
                model=str(ctx.model or ""),
                message_id=str(ctx.response_id or ctx.request_id or self.request_id or ""),
            )
            output_state = {"first_yield": True, "streaming_updated": False}

            async def _streamified() -> AsyncGenerator[bytes]:
                for ev in iter_internal_response_as_stream_events(internal_resp):
                    converted_events = tgt_norm.stream_event_from_internal(ev, state)
                    if not converted_events:
                        continue
                    self._record_converted_chunks(ctx, converted_events)
                    for sse_line in _format_converted_events_to_sse(
                        converted_events, client_api_format
                    ):
                        if not sse_line:
                            continue
                        ctx.chunk_count += 1
                        self._mark_first_output(ctx, output_state)
                        yield (sse_line + "\n").encode("utf-8")

                # OpenAI chat clients expect a final [DONE] marker.
                if str(client_api_format or "").strip().lower() == "openai:chat":
                    ctx.chunk_count += 1
                    self._mark_first_output(ctx, output_state)
                    yield b"data: [DONE]\n\n"
                    ctx.has_completion = True

            return _streamified()

        # 流式请求使用 stream_first_byte_timeout 作为首字节超时
        # 优先使用 Provider 配置，否则使用全局配置
        request_timeout = provider.stream_first_byte_timeout or config.stream_first_byte_timeout

        _proxy_label = _gpl(ctx.proxy_info)

        logger.debug(
            f"  └─ [{self.request_id}] 发送流式请求: "
            f"Provider={provider.name}, Endpoint={endpoint.id[:8] if endpoint.id else 'N/A'}..., "
            f"Key=***{key.api_key[-4:] if key.api_key else 'N/A'}, "
            f"原始模型={ctx.model}, 映射后={mapped_model or '无映射'}, URL模型={url_model}, "
            f"timeout={request_timeout}s, 代理={_proxy_label}"
        )

        # 获取 HTTP 客户端（支持代理配置，Key 级别优先于 Provider 级别）
        # 使用连接池复用客户端，避免每次流式请求都新建 TCP/TLS 连接
        from src.clients.http_client import HTTPClientPool
        from src.services.proxy_node.resolver import build_stream_kwargs, resolve_delegate_config

        delegate_cfg = resolve_delegate_config(effective_proxy)
        http_client = await HTTPClientPool.get_upstream_client(
            delegate_cfg, proxy_config=effective_proxy
        )

        # 用于存储内部函数的结果（必须在函数定义前声明，供 nonlocal 使用）
        byte_iterator: Any = None
        prefetched_chunks: Any = None
        response_ctx: Any = None

        async def _connect_and_prefetch() -> None:
            """建立连接并预读首字节（受整体超时控制）"""
            nonlocal byte_iterator, prefetched_chunks, response_ctx
            _skw = build_stream_kwargs(
                delegate_cfg,
                url=url,
                headers=provider_headers,
                payload=provider_payload,
                timeout=(
                    provider.request_timeout or config.http_request_timeout
                    if delegate_cfg
                    else None
                ),
            )
            _connect_start = time.monotonic()
            response_ctx = http_client.stream(**_skw)
            stream_response = await response_ctx.__aenter__()
            ctx.set_ttfb_ms(int((time.monotonic() - _connect_start) * 1000))

            ctx.status_code = stream_response.status_code
            ctx.response_headers = dict(stream_response.headers)
            ctx.set_proxy_timing(ctx.response_headers)

            logger.debug(f"  └─ 收到响应: status={stream_response.status_code}")

            if envelope:
                envelope.on_http_status(
                    base_url=ctx.selected_base_url,
                    status_code=ctx.status_code,
                )

            stream_response.raise_for_status()

            # 使用字节流迭代器（避免 aiter_lines 的性能问题, aiter_bytes 会自动解压 gzip/deflate）
            byte_iterator = stream_response.aiter_bytes()

            # 预读第一个数据块，检测嵌套错误（HTTP 200 但响应体包含错误）
            prefetched_chunks = await self._prefetch_and_check_embedded_error(
                byte_iterator, provider, endpoint, ctx
            )

        for attempt in range(2):
            try:
                # 使用 asyncio.wait_for 包裹整个"建立连接 + 获取首字节"阶段
                # stream_first_byte_timeout 控制首字节超时，避免上游长时间无响应
                # 同时检测客户端断连，避免客户端已断开但服务端仍在等待上游响应
                if http_request is not None:
                    await wait_for_with_disconnect_detection(
                        _connect_and_prefetch(),
                        timeout=request_timeout,
                        is_disconnected=http_request.is_disconnected,
                        request_id=self.request_id,
                    )
                else:
                    await asyncio.wait_for(_connect_and_prefetch(), timeout=request_timeout)
                break

            except TimeoutError as e:
                # 整体请求超时（建立连接 + 获取首字节）
                # 清理可能已建立的连接上下文（不关闭池中复用的客户端）
                if response_ctx is not None:
                    try:
                        await response_ctx.__aexit__(None, None, None)
                    except Exception:
                        pass
                if envelope:
                    envelope.on_connection_error(base_url=ctx.selected_base_url, exc=e)
                logger.warning(
                    f"  [{self.request_id}] 请求超时: Provider={provider.name}, timeout={request_timeout}s"
                )
                raise ProviderTimeoutException(
                    provider_name=str(provider.name),
                    timeout=int(request_timeout),
                )

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

            except (httpx.ConnectError, httpx.ConnectTimeout, httpx.TimeoutException) as e:
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

                error_text = await self._extract_error_text(e)
                logger.error(
                    f"Provider 返回错误状态: {e.response.status_code}\n  Response: {error_text}"
                )
                # 将上游错误信息附加到异常，以便故障转移时能够返回给客户端
                e.upstream_response = error_text  # type: ignore[attr-defined]
                raise

            except EmbeddedErrorException:
                # 嵌套错误需要触发重试，关闭连接上下文后重新抛出
                try:
                    if response_ctx is not None:
                        await response_ctx.__aexit__(None, None, None)
                except Exception:
                    pass
                raise

            except Exception:
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
            prefetched_chunks,
        )

    async def _create_response_stream(
        self,
        ctx: StreamContext,
        stream_response: httpx.Response,
        response_ctx: Any,
    ) -> AsyncGenerator[bytes]:
        """创建响应流生成器（使用字节流）"""
        try:
            sse_parser = SSEEventParser()
            last_data_time = time.time()
            buffer = b""
            output_state = {"first_yield": True, "streaming_updated": False}
            # 使用增量解码器处理跨 chunk 的 UTF-8 字符
            decoder = codecs.getincrementaldecoder("utf-8")(errors="replace")

            # 使用已设置的 ctx.needs_conversion（由候选筛选阶段根据端点配置判断）
            # 不再调用 _needs_format_conversion，它只检查格式差异，不检查端点配置
            needs_conversion = ctx.needs_conversion
            behavior = get_provider_behavior(
                provider_type=ctx.provider_type,
                endpoint_sig=ctx.provider_api_format,
            )
            envelope = behavior.envelope
            if envelope and envelope.force_stream_rewrite():
                needs_conversion = True
                ctx.needs_conversion = True

            # Kiro 特殊处理：AWS Event Stream 二进制流需要重写为 SSE
            ctx_provider_type = str(ctx.provider_type or "").strip().lower()
            if ctx_provider_type == "kiro" and envelope and envelope.force_stream_rewrite():
                from src.services.provider.adapters.kiro.eventstream_rewriter import (
                    apply_kiro_stream_rewrite,
                )

                chunk_source: AsyncGenerator[bytes, None] = apply_kiro_stream_rewrite(
                    stream_response.aiter_bytes(),
                    model=str(ctx.model or ""),
                    input_tokens=int(ctx.input_tokens or 0),
                )
                # Kiro 重写后输出的是 Claude SSE 格式，不需要再进行格式转换
                needs_conversion = False
                ctx.needs_conversion = False
            else:
                chunk_source = stream_response.aiter_bytes()

            async for chunk in chunk_source:
                buffer += chunk
                # 处理缓冲区中的完整行
                while b"\n" in buffer:
                    line_bytes, buffer = buffer.split(b"\n", 1)
                    try:
                        # 使用增量解码器，可以正确处理跨 chunk 的多字节字符
                        line = decoder.decode(line_bytes + b"\n", False).rstrip("\n")
                    except Exception as e:
                        logger.warning(
                            "[{}] UTF-8 解码失败: {}, bytes={!r}",
                            self.request_id,
                            e,
                            line_bytes[:50],
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
                                record_chunk=not needs_conversion,
                            )
                        self._mark_first_output(ctx, output_state)
                        yield b"\n"
                        continue

                    ctx.chunk_count += 1

                    # 空流检测：超过阈值且无数据，发送错误事件并结束
                    if ctx.chunk_count > self.EMPTY_CHUNK_THRESHOLD and ctx.data_count == 0:
                        elapsed = time.time() - last_data_time
                        if elapsed > self.DATA_TIMEOUT:
                            logger.warning("Provider '{}' 流超时且无数据", ctx.provider_name)
                            ctx.status_code = 504
                            ctx.error_message = "流式响应超时，未收到有效数据"
                            ctx.upstream_response = (
                                f"流超时: Provider={ctx.provider_name}, "
                                f"elapsed={elapsed:.1f}s, "
                                f"chunk_count={ctx.chunk_count}, data_count=0"
                            )
                            error_event = {
                                "type": "error",
                                "error": {
                                    "type": "empty_stream_timeout",
                                    "message": ctx.error_message,
                                },
                            }
                            self._mark_first_output(ctx, output_state)
                            yield f"event: error\ndata: {json.dumps(error_event)}\n\n".encode()
                            return  # 结束生成器

                    # 格式转换或直接透传
                    if needs_conversion:
                        converted_lines, converted_events = self._convert_sse_line(
                            ctx, line, events
                        )
                        # 记录转换后的数据到 parsed_chunks
                        self._record_converted_chunks(ctx, converted_events)
                        for converted_line in converted_lines:
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
                            record_chunk=not needs_conversion,
                        )

                    if ctx.data_count > 0:
                        last_data_time = time.time()

            # 处理剩余事件
            for event in sse_parser.flush():
                self._handle_sse_event(
                    ctx,
                    event.get("event"),
                    event.get("data") or "",
                    record_chunk=not needs_conversion,
                )

            # 检查是否收到数据
            if ctx.data_count == 0:
                logger.warning("Provider '{}' 返回空流式响应", ctx.provider_name)
                ctx.status_code = 503
                ctx.error_message = "上游服务返回了空的流式响应"
                ctx.upstream_response = (
                    f"空流式响应: Provider={ctx.provider_name}, "
                    f"chunk_count={ctx.chunk_count}, data_count=0"
                )
                error_event = {
                    "type": "error",
                    "error": {
                        "type": "empty_response",
                        "message": ctx.error_message,
                    },
                }
                yield f"event: error\ndata: {json.dumps(error_event)}\n\n".encode()
            else:
                logger.debug("流式数据转发完成")
                # 为 OpenAI 客户端补齐 [DONE] 标记（非 CLI 格式）
                client_fmt = (ctx.client_api_format or "").strip().lower()
                if needs_conversion and client_fmt == "openai:chat":
                    yield b"data: [DONE]\n\n"

        except GeneratorExit:
            raise
        except httpx.StreamClosed:
            # 连接关闭前 flush 残余数据，尝试捕获尾部事件（如 response.completed 中的 usage）
            self._flush_remaining_sse_data(
                ctx, buffer, decoder, sse_parser, record_chunk=not needs_conversion
            )
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
                yield f"event: error\ndata: {json.dumps(error_event)}\n\n".encode()
        except httpx.RemoteProtocolError:
            # 连接异常关闭前 flush 残余数据，尝试捕获尾部事件（如 response.completed 中的 usage）
            self._flush_remaining_sse_data(
                ctx, buffer, decoder, sse_parser, record_chunk=not needs_conversion
            )
            if ctx.data_count > 0:
                error_event = {
                    "type": "error",
                    "error": {
                        "type": "connection_error",
                        "message": "上游连接意外关闭，部分响应已成功传输",
                    },
                }
                yield f"event: error\ndata: {json.dumps(error_event)}\n\n".encode()
            else:
                raise
        except httpx.ReadError:
            # 代理/上游连接读取失败（如 aether-proxy 中断），与 RemoteProtocolError 处理逻辑一致
            self._flush_remaining_sse_data(
                ctx, buffer, decoder, sse_parser, record_chunk=not needs_conversion
            )
            if ctx.data_count > 0:
                error_event = {
                    "type": "error",
                    "error": {
                        "type": "connection_error",
                        "message": "代理或上游连接读取失败，部分响应已成功传输",
                    },
                }
                yield f"event: error\ndata: {json.dumps(error_event)}\n\n".encode()
            else:
                raise
        finally:
            try:
                await response_ctx.__aexit__(None, None, None)
            except Exception:
                pass

    def _flush_remaining_sse_data(
        self,
        ctx: StreamContext,
        buffer: bytes,
        decoder: codecs.IncrementalDecoder,
        sse_parser: SSEEventParser,
        *,
        record_chunk: bool = True,
    ) -> None:
        """
        异常发生时 flush 残留的字节 buffer 和 SSE parser 内部缓冲区。

        用于 StreamClosed / RemoteProtocolError 等场景：
        连接断开可能恰好发生在最后一个 SSE 事件（如 response.completed）
        的 data 行已收到、但终止空行尚未到达之时。此方法确保这些事件仍能被处理，
        从而正确捕获 usage 等关键信息。
        """
        try:
            # 1) flush 字节 buffer 中的残余行
            if buffer:
                remaining = decoder.decode(buffer, True)
                for line in remaining.split("\n"):
                    stripped = line.rstrip("\r")
                    events = sse_parser.feed_line(stripped)
                    for event in events:
                        self._handle_sse_event(
                            ctx,
                            event.get("event"),
                            event.get("data") or "",
                            record_chunk=record_chunk,
                        )
            # 2) flush SSE parser 内部累积的未完成事件
            for event in sse_parser.flush():
                self._handle_sse_event(
                    ctx,
                    event.get("event"),
                    event.get("data") or "",
                    record_chunk=record_chunk,
                )
        except Exception:
            # best-effort: 不应因 flush 失败影响后续流程
            pass

    def _estimate_tokens_for_incomplete_stream(
        self,
        ctx: StreamContext,
        request_body: dict[str, Any],
    ) -> None:
        """
        流未正常完成（无 response.completed）且 token 均为 0 时的兜底估算。

        从已收集的输出文本和请求体粗略估算 token 数，确保 usage 记录不为 0。
        估算采用 ~4 字符/token 的保守比例。
        """
        # 输出 tokens：从已收集的文本估算
        collected = ctx.collected_text
        if collected:
            ctx.output_tokens = max(1, len(collected) // 4)

        # 输入 tokens：从请求体文本内容估算
        try:
            total_input_len = 0
            instructions = request_body.get("instructions")
            if isinstance(instructions, str):
                total_input_len += len(instructions)
            # OpenAI Responses API 使用 input 字段；Claude 使用 messages
            input_items = request_body.get("input") or request_body.get("messages") or []
            if isinstance(input_items, list):
                for item in input_items:
                    if isinstance(item, str):
                        total_input_len += len(item)
                    elif isinstance(item, dict):
                        content = item.get("content", "")
                        if isinstance(content, str):
                            total_input_len += len(content)
                        elif isinstance(content, list):
                            for block in content:
                                if isinstance(block, dict):
                                    text = block.get("text", "")
                                    if isinstance(text, str):
                                        total_input_len += len(text)
            if total_input_len > 0:
                ctx.input_tokens = max(1, total_input_len // 4)
            else:
                # fallback: 整个请求体 JSON 大小
                body_str = json.dumps(request_body, ensure_ascii=False)
                ctx.input_tokens = max(1, len(body_str) // 4)
        except Exception:
            pass

        if ctx.input_tokens > 0 or ctx.output_tokens > 0:
            logger.warning(
                "[{}] 流未正常完成 (has_completion=False, data_count={}), "
                "使用估算 tokens: in={}, out={}",
                ctx.request_id,
                ctx.data_count,
                ctx.input_tokens,
                ctx.output_tokens,
            )

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
            # 优先使用 Provider 配置，否则使用全局配置
            ttfb_timeout = provider.stream_first_byte_timeout or config.stream_first_byte_timeout
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
                            upstream_response=(
                                normalized_line[:500] if normalized_line else "(empty)"
                            ),
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
        except OSError as e:
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
        prefetched_chunks: list,
    ) -> AsyncGenerator[bytes]:
        """创建响应流生成器（带预读数据，使用字节流）"""
        try:
            sse_parser = SSEEventParser()
            last_data_time = time.time()
            buffer = b""
            output_state = {"first_yield": True, "streaming_updated": False}
            # 使用增量解码器处理跨 chunk 的 UTF-8 字符
            decoder = codecs.getincrementaldecoder("utf-8")(errors="replace")

            # 使用已设置的 ctx.needs_conversion（由候选筛选阶段根据端点配置判断）
            # 不再调用 _needs_format_conversion，它只检查格式差异，不检查端点配置
            needs_conversion = ctx.needs_conversion
            behavior = get_provider_behavior(
                provider_type=ctx.provider_type,
                endpoint_sig=ctx.provider_api_format,
            )
            envelope = behavior.envelope
            if envelope and envelope.force_stream_rewrite():
                needs_conversion = True
                ctx.needs_conversion = True

            # Kiro 特殊处理：AWS Event Stream 二进制流需要重写为 SSE
            ctx_provider_type = str(ctx.provider_type or "").strip().lower()
            if ctx_provider_type == "kiro" and envelope and envelope.force_stream_rewrite():
                from src.services.provider.adapters.kiro.eventstream_rewriter import (
                    apply_kiro_stream_rewrite,
                )

                byte_iterator = apply_kiro_stream_rewrite(
                    byte_iterator,
                    model=str(ctx.model or ""),
                    input_tokens=int(ctx.input_tokens or 0),
                    prefetched_chunks=list(prefetched_chunks) if prefetched_chunks else None,
                )
                prefetched_chunks = []

                # Kiro 重写后输出的是 Claude SSE 格式
                # 客户端也是 Claude CLI，不需要再进行格式转换
                needs_conversion = False
                ctx.needs_conversion = False

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
                                record_chunk=not needs_conversion,
                            )
                        self._mark_first_output(ctx, output_state)
                        yield b"\n"
                        continue

                    ctx.chunk_count += 1

                    # 格式转换或直接透传
                    if needs_conversion:
                        converted_lines, converted_events = self._convert_sse_line(
                            ctx, line, events
                        )
                        # 记录转换后的数据到 parsed_chunks
                        self._record_converted_chunks(ctx, converted_events)
                        for converted_line in converted_lines:
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
                            record_chunk=not needs_conversion,
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
                                record_chunk=not needs_conversion,
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
                            yield f"event: error\ndata: {json.dumps(error_event)}\n\n".encode()
                            return

                    # 格式转换或直接透传
                    if needs_conversion:
                        converted_lines, converted_events = self._convert_sse_line(
                            ctx, line, events
                        )
                        # 记录转换后的数据到 parsed_chunks
                        self._record_converted_chunks(ctx, converted_events)
                        for converted_line in converted_lines:
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
                            record_chunk=not needs_conversion,
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
                    record_chunk=not needs_conversion,
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
                yield f"event: error\ndata: {json.dumps(error_event)}\n\n".encode()
            else:
                logger.debug("流式数据转发完成")
                # 为 OpenAI 客户端补齐 [DONE] 标记（非 CLI 格式）
                client_fmt = (ctx.client_api_format or "").strip().lower()
                if needs_conversion and client_fmt == "openai:chat":
                    yield b"data: [DONE]\n\n"

        except GeneratorExit:
            raise
        except httpx.StreamClosed:
            # 连接关闭前 flush 残余数据，尝试捕获尾部事件（如 response.completed 中的 usage）
            self._flush_remaining_sse_data(
                ctx, buffer, decoder, sse_parser, record_chunk=not needs_conversion
            )
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
                yield f"event: error\ndata: {json.dumps(error_event)}\n\n".encode()
        except httpx.RemoteProtocolError:
            # 连接异常关闭前 flush 残余数据，尝试捕获尾部事件（如 response.completed 中的 usage）
            self._flush_remaining_sse_data(
                ctx, buffer, decoder, sse_parser, record_chunk=not needs_conversion
            )
            if ctx.data_count > 0:
                error_event = {
                    "type": "error",
                    "error": {
                        "type": "connection_error",
                        "message": "上游连接意外关闭，部分响应已成功传输",
                    },
                }
                yield f"event: error\ndata: {json.dumps(error_event)}\n\n".encode()
            else:
                raise
        except httpx.ReadError:
            # 代理/上游连接读取失败（如 aether-proxy 中断），与 RemoteProtocolError 处理逻辑一致
            self._flush_remaining_sse_data(
                ctx, buffer, decoder, sse_parser, record_chunk=not needs_conversion
            )
            if ctx.data_count > 0:
                error_event = {
                    "type": "error",
                    "error": {
                        "type": "connection_error",
                        "message": "代理或上游连接读取失败，部分响应已成功传输",
                    },
                }
                yield f"event: error\ndata: {json.dumps(error_event)}\n\n".encode()
            else:
                raise
        finally:
            try:
                await response_ctx.__aexit__(None, None, None)
            except Exception:
                pass

    def _handle_sse_event(
        self,
        ctx: StreamContext,
        event_name: str | None,
        data_str: str,
        record_chunk: bool = False,
    ) -> None:
        """
        处理 SSE 事件

        通用框架：解析 JSON、更新计数器
        子类可覆盖 _process_event_data() 实现格式特定逻辑

        Args:
            ctx: 流上下文
            event_name: 事件名称（如 message_start, content_block_delta 等）
            data_str: 事件数据字符串（JSON 格式）
            record_chunk: 是否记录到 parsed_chunks（不需要格式转换时应为 True）
                          当为 True 时，同时更新 data_count；
                          当为 False 时，data_count 由 _record_converted_chunks 更新
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

        if not isinstance(data, dict):
            return

        behavior = get_provider_behavior(
            provider_type=ctx.provider_type,
            endpoint_sig=ctx.provider_api_format,
        )
        envelope = behavior.envelope
        if envelope:
            data = envelope.unwrap_response(data)
            if not isinstance(data, dict):
                return

        # 当不需要格式转换时，更新 data_count；需要记录时再写入 parsed_chunks。
        # 当需要格式转换时（record_chunk=False），data_count 由 _record_converted_chunks 更新
        if record_chunk:
            ctx.data_count += 1
            if ctx.record_parsed_chunks:
                ctx.parsed_chunks.append(data)

        event_type = event_name or data.get("type", "")

        if envelope:
            envelope.postprocess_unwrapped_response(model=ctx.model, data=data)

        # 调用格式特定的处理逻辑
        # 注意：跨格式转换时，_process_event_data 会自动选择正确的 Provider 解析器
        self._process_event_data(ctx, event_type, data)

    def _process_event_data(
        self,
        ctx: StreamContext,
        event_type: str,
        data: dict[str, Any],
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
        # Claude/CLI 流式响应的 usage 可能在首个 chunk 或最后一个 chunk 中
        # 首个 chunk 可能部分为 0，最后一个 chunk 包含完整值，因此取最大值确保正确计费
        #
        # 重要：当跨格式转换时，收到的数据是 Provider 格式，需要使用 Provider 格式的解析器
        # 而不是客户端格式的解析器（self.parser）
        parser = self.parser
        if ctx.provider_api_format and ctx.provider_api_format != ctx.client_api_format:
            # 跨格式转换：使用 Provider 格式的解析器
            try:
                provider_parser = get_parser_for_format(ctx.provider_api_format)
                if provider_parser:
                    parser = provider_parser
                    logger.debug(
                        f"[{getattr(ctx, 'request_id', 'unknown')}] 使用 Provider 解析器: "
                        f"{ctx.provider_api_format} (client={ctx.client_api_format})"
                    )
            except KeyError:
                logger.debug(
                    f"[{getattr(ctx, 'request_id', 'unknown')}] 未找到 Provider 格式解析器: "
                    f"{ctx.provider_api_format}, 回退使用客户端格式解析器"
                )

        usage = parser.extract_usage_from_response(data)
        if usage:
            new_input = usage.get("input_tokens", 0)
            new_output = usage.get("output_tokens", 0)
            new_cached = usage.get("cache_read_tokens", 0)
            new_cache_creation = usage.get("cache_creation_tokens", 0)

            # 取最大值更新
            if new_input > ctx.input_tokens:
                ctx.input_tokens = new_input
            if new_output > ctx.output_tokens:
                ctx.output_tokens = new_output
            if new_cached > ctx.cached_tokens:
                ctx.cached_tokens = new_cached
            if new_cache_creation > ctx.cache_creation_tokens:
                ctx.cache_creation_tokens = new_cache_creation

            # 保存最后一个非空 usage 作为 final_usage
            if any([new_input, new_output, new_cached, new_cache_creation]):
                ctx.final_usage = usage

        # 提取文本内容（同样使用正确的解析器）
        text = parser.extract_text_content(data)
        if text:
            ctx.append_text(text)

        # 检查完成事件
        if event_type in ("response.completed", "message_stop"):
            ctx.has_completion = True
            response_obj = data.get("response")
            if isinstance(response_obj, dict):
                ctx.final_response = response_obj

    def _record_converted_chunks(
        self,
        ctx: StreamContext,
        converted_events: list[dict[str, Any]],
    ) -> None:
        """
        记录转换后的 chunk 数据到 parsed_chunks，并更新统计信息

        当需要格式转换时，记录的是转换后的数据（客户端实际收到的格式）；
        同时更新 data_count、has_completion 等统计信息。

        重要：此方法也从转换后的事件中提取 usage 信息，作为 _process_event_data
        从原始数据提取的补充。这确保即使原始 Provider 数据中没有 usage（如 OpenAI
        未设置 stream_options），也能从转换后的格式中获取。

        Args:
            ctx: 流上下文
            converted_events: 转换后的事件列表
        """
        for evt in converted_events:
            if isinstance(evt, dict):
                ctx.data_count += 1
                if ctx.record_parsed_chunks:
                    ctx.parsed_chunks.append(evt)

                # 检测完成事件（根据客户端格式判断）
                # OpenAI 格式: choices[].finish_reason
                # Claude 格式: type == "message_stop" 或 stop_reason
                event_type = evt.get("type", "")
                if event_type == "message_stop":
                    ctx.has_completion = True
                elif event_type == "response.completed":
                    ctx.has_completion = True
                elif "choices" in evt:
                    choices = evt.get("choices", [])
                    for choice in choices:
                        if isinstance(choice, dict) and choice.get("finish_reason"):
                            ctx.has_completion = True
                            break

                # 从转换后的事件中提取 usage（补充 _process_event_data 的提取）
                # Claude 格式: message_delta.usage 或 message_start.message.usage
                # OpenAI 格式: chunk.usage
                self._extract_usage_from_converted_event(ctx, evt, event_type)

    def _extract_usage_from_converted_event(
        self,
        ctx: StreamContext,
        evt: dict[str, Any],
        event_type: str,
    ) -> None:
        """
        从转换后的事件中提取 usage 信息

        支持多种格式：
        - Claude: message_delta.usage, message_start.message.usage
        - OpenAI: chunk.usage
        - Gemini: usageMetadata

        Args:
            ctx: 流上下文
            evt: 转换后的事件
            event_type: 事件类型
        """
        usage: dict[str, Any] | None = None

        # Claude 格式: message_delta 或 message_start
        if event_type == "message_delta":
            usage = evt.get("usage")
        elif event_type == "message_start":
            message = evt.get("message", {})
            if isinstance(message, dict):
                usage = message.get("usage")
        # OpenAI Responses API (openai:cli) 格式: response.completed 中 usage 嵌套在 response 对象内
        elif event_type == "response.completed":
            resp_obj = evt.get("response")
            if isinstance(resp_obj, dict):
                usage = resp_obj.get("usage")
            # 兼容: 部分实现可能在顶层也有 usage
            if not usage:
                usage = evt.get("usage")
        # OpenAI Chat 格式: 直接在 chunk 中
        elif "usage" in evt:
            usage = evt.get("usage")
        # Gemini 格式: usageMetadata
        elif "usageMetadata" in evt:
            meta = evt.get("usageMetadata", {})
            if isinstance(meta, dict):
                usage = {
                    "input_tokens": meta.get("promptTokenCount", 0),
                    "output_tokens": meta.get("candidatesTokenCount", 0),
                    "cache_read_tokens": meta.get("cachedContentTokenCount", 0),
                    "cache_creation_tokens": 0,  # Gemini 目前不支持缓存创建
                }

        if usage and isinstance(usage, dict):
            new_input = usage.get("input_tokens", 0) or 0
            new_output = usage.get("output_tokens", 0) or 0
            new_cached = usage.get("cache_read_tokens") or usage.get("cache_read_input_tokens") or 0
            new_cache_creation = (
                usage.get("cache_creation_tokens") or usage.get("cache_creation_input_tokens") or 0
            )

            # 取最大值更新（与 _process_event_data 相同的策略）
            if new_input > ctx.input_tokens:
                ctx.input_tokens = new_input
                logger.debug(f"[{ctx.request_id}] 从转换后事件更新 input_tokens: {new_input}")
            if new_output > ctx.output_tokens:
                ctx.output_tokens = new_output
                logger.debug(f"[{ctx.request_id}] 从转换后事件更新 output_tokens: {new_output}")
            if new_cached > ctx.cached_tokens:
                ctx.cached_tokens = new_cached
            if new_cache_creation > ctx.cache_creation_tokens:
                ctx.cache_creation_tokens = new_cache_creation

            # 保存最后一个非空 usage
            if any([new_input, new_output, new_cached, new_cache_creation]):
                ctx.final_usage = usage

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
        stream_generator: AsyncGenerator[bytes],
        http_request: Request | None = None,
    ) -> AsyncGenerator[bytes]:
        """
        创建带监控的流生成器

        支持两种断连检测方式：
        1. 如果提供了 http_request，使用后台任务主动检测客户端断连
        2. 如果未提供，仅依赖 asyncio.CancelledError 被动检测

        Args:
            ctx: 流上下文
            stream_generator: 底层流生成器
            http_request: FastAPI Request 对象，用于检测客户端断连
        """
        import time as time_module

        last_chunk_time = time_module.time()
        chunk_count = 0

        try:
            if http_request is not None:
                # 使用后台任务检测断连，完全不阻塞流式传输
                disconnected = False

                async def check_disconnect_background() -> None:
                    nonlocal disconnected
                    while not disconnected and not ctx.has_completion:
                        await asyncio.sleep(0.5)
                        try:
                            if await http_request.is_disconnected():
                                disconnected = True
                                break
                        except Exception as e:
                            # 检测失败时不中断流，继续传输
                            logger.debug(f"ID:{ctx.request_id} | 断连检测异常: {e}")

                # 启动后台检查任务
                check_task = asyncio.create_task(check_disconnect_background())

                try:
                    async for chunk in stream_generator:
                        if disconnected:
                            # 如果响应已完成，客户端断开不算失败
                            if ctx.has_completion:
                                logger.info(
                                    f"ID:{ctx.request_id} | Client disconnected after completion"
                                )
                            else:
                                logger.warning(f"ID:{ctx.request_id} | Client disconnected")
                                ctx.status_code = 499
                                ctx.error_message = "client_disconnected"
                            break
                        last_chunk_time = time_module.time()
                        chunk_count += 1
                        yield chunk
                finally:
                    check_task.cancel()
                    try:
                        await check_task
                    except asyncio.CancelledError:
                        pass
            else:
                # 无 http_request，仅被动监控
                async for chunk in stream_generator:
                    last_chunk_time = time_module.time()
                    chunk_count += 1
                    yield chunk

        except asyncio.CancelledError:
            # 注意：CancelledError 不等于“用户手动取消”，它既可能是客户端断连触发，
            # 也可能是服务端（重载/关停/内部取消）导致的协程取消。
            # 这里尽量做一次“断连归因”：仅当能确认客户端已断开时才记为 499 cancelled。
            time_since_last_chunk = time_module.time() - last_chunk_time

            is_client_disconnected = False
            if http_request is not None:
                try:
                    # shield + timeout: 避免在取消态下二次被 CancelledError 打断，尽力取到断连状态
                    # 限时 0.5s 防止极端情况下的阻塞
                    is_client_disconnected = await asyncio.wait_for(
                        asyncio.shield(http_request.is_disconnected()),
                        timeout=0.5,
                    )
                except (asyncio.CancelledError, asyncio.TimeoutError):
                    # 无法在取消态/超时下完成断连检查，保守视为未知（不强行归因为客户端）
                    is_client_disconnected = False
                except Exception as e:
                    logger.debug(f"ID:{ctx.request_id} | cancel 断连检测失败: {e}")
                    is_client_disconnected = False

            # 如果响应已完成，不标记为失败/取消
            if not ctx.has_completion:
                if is_client_disconnected:
                    ctx.status_code = 499
                    ctx.error_message = "client_disconnected"
                    logger.warning(
                        f"ID:{ctx.request_id} | Stream cancelled by client: "
                        f"chunks={chunk_count}, "
                        f"has_completion={ctx.has_completion}, "
                        f"time_since_last_chunk={time_since_last_chunk:.2f}s, "
                        f"output_tokens={ctx.output_tokens}"
                    )
                else:
                    # 服务端中断（例如重载/关停/内部取消）— 不应伪装成客户端取消
                    ctx.status_code = 503
                    ctx.error_message = "server_cancelled"
                    logger.error(
                        f"ID:{ctx.request_id} | Stream interrupted by server: "
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
        original_headers: dict[str, str],
        original_request_body: dict[str, Any],
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

            behavior = get_provider_behavior(
                provider_type=ctx.provider_type,
                endpoint_sig=ctx.provider_api_format,
            )
            envelope = behavior.envelope
            if envelope:
                envelope.on_http_status(
                    base_url=ctx.selected_base_url,
                    status_code=ctx.status_code,
                )

            # 获取新的 DB session
            db_gen = get_db()
            bg_db = next(db_gen)

            try:
                from src.models.database import ApiKey as ApiKeyModel

                user = bg_db.query(User).filter(User.id == ctx.user_id).first()
                api_key = bg_db.query(ApiKeyModel).filter(ApiKeyModel.id == ctx.api_key_id).first()

                if not user or not api_key:
                    logger.warning(
                        "[{}] 无法记录统计: user={} api_key={}",
                        ctx.request_id,
                        user is not None,
                        api_key is not None,
                    )
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
                # 499 = 客户端取消（不算系统失败）；其他 4xx/5xx 视为失败
                if ctx.status_code and ctx.status_code >= 400:
                    client_response_headers = ctx.client_response_headers or {
                        "content-type": "application/json"
                    }

                    if ctx.is_client_disconnected():
                        # 客户端取消：记录为 cancelled（不算系统失败）
                        request_metadata = {"perf": ctx.perf_metrics} if ctx.perf_metrics else None
                        await bg_telemetry.record_cancelled(
                            provider=ctx.provider_name or "unknown",
                            model=ctx.model,
                            response_time_ms=response_time_ms,
                            first_byte_time_ms=ctx.first_byte_time_ms,
                            status_code=ctx.status_code,
                            request_headers=original_headers,
                            request_body=actual_request_body,
                            is_stream=True,
                            api_format=ctx.api_format,
                            provider_request_headers=ctx.provider_request_headers,
                            input_tokens=ctx.input_tokens,
                            output_tokens=ctx.output_tokens,
                            cache_creation_tokens=ctx.cache_creation_tokens,
                            cache_read_tokens=ctx.cached_tokens,
                            response_body=response_body,
                            response_headers=ctx.response_headers,
                            client_response_headers=client_response_headers,
                            endpoint_api_format=ctx.provider_api_format or None,
                            has_format_conversion=ctx.has_format_conversion,
                            target_model=ctx.mapped_model,
                            request_metadata=request_metadata,
                        )
                        logger.debug(f"{self.FORMAT_ID} 流式响应被客户端取消")
                        logger.info(
                            f"[CANCEL] {self.request_id[:8]} | {ctx.model} | {ctx.provider_name} | {response_time_ms}ms | "
                            f"{ctx.status_code} | in:{ctx.input_tokens} out:{ctx.output_tokens} cache:{ctx.cached_tokens}"
                        )
                    else:
                        # 服务端/上游异常：记录为失败
                        request_metadata = {"perf": ctx.perf_metrics} if ctx.perf_metrics else None
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
                            input_tokens=ctx.input_tokens,
                            output_tokens=ctx.output_tokens,
                            cache_creation_tokens=ctx.cache_creation_tokens,
                            cache_read_tokens=ctx.cached_tokens,
                            response_body=response_body,
                            response_headers=ctx.response_headers,
                            client_response_headers=client_response_headers,
                            # 格式转换追踪
                            endpoint_api_format=ctx.provider_api_format or None,
                            has_format_conversion=ctx.has_format_conversion,
                            # 模型映射信息
                            target_model=ctx.mapped_model,
                            request_metadata=request_metadata,
                        )
                        logger.debug(f"{self.FORMAT_ID} 流式响应中断")
                        logger.info(
                            f"[FAIL] {self.request_id[:8]} | {ctx.model} | {ctx.provider_name} | {response_time_ms}ms | "
                            f"{ctx.status_code} | in:{ctx.input_tokens} out:{ctx.output_tokens} cache:{ctx.cached_tokens}"
                        )
                else:
                    # 在记录统计前，允许子类从 parsed_chunks 中提取额外的元数据
                    self._finalize_stream_metadata(ctx)

                    # 流未正常完成（如上游截断/连接中断）且无 token 数据时，
                    # 从已收集的文本和请求体估算 tokens，避免 usage 记录为 0
                    if (
                        not ctx.has_completion
                        and ctx.data_count > 0
                        and ctx.input_tokens == 0
                        and ctx.output_tokens == 0
                    ):
                        self._estimate_tokens_for_incomplete_stream(ctx, actual_request_body)

                    # 流式成功时，返回给客户端的是提供商响应头 + SSE 必需头
                    client_response_headers = filter_proxy_response_headers(ctx.response_headers)
                    client_response_headers.update(
                        {
                            "Cache-Control": "no-cache, no-transform",
                            "X-Accel-Buffering": "no",
                            "content-type": "text/event-stream",
                        }
                    )

                    logger.debug(
                        f"[{ctx.request_id}] 开始记录 Usage: "
                        f"provider={ctx.provider_name}, model={ctx.model}, "
                        f"in={ctx.input_tokens}, out={ctx.output_tokens}"
                    )
                    request_metadata = {"perf": ctx.perf_metrics} if ctx.perf_metrics else None
                    total_cost = await bg_telemetry.record_success(
                        provider=ctx.provider_name,
                        model=ctx.model,
                        input_tokens=ctx.input_tokens,
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
                        # 格式转换追踪
                        endpoint_api_format=ctx.provider_api_format or None,
                        has_format_conversion=ctx.has_format_conversion,
                        # Provider 侧追踪信息（用于记录真实成本）
                        provider_id=ctx.provider_id,
                        provider_endpoint_id=ctx.endpoint_id,
                        provider_api_key_id=ctx.key_id,
                        # 模型映射信息
                        target_model=ctx.mapped_model,
                        # Provider 响应元数据（如 Gemini 的 modelVersion）
                        response_metadata=ctx.response_metadata if ctx.response_metadata else None,
                        request_metadata=request_metadata,
                    )
                    logger.debug(f"[{ctx.request_id}] Usage 记录完成: cost=${total_cost:.6f}")
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

                    # 计算候选自身的 TTFB
                    candidate_first_byte_time_ms: int | None = None
                    if ctx.first_byte_time_ms is not None:
                        candidate_first_byte_time_ms = (
                            RequestCandidateService.calculate_candidate_ttfb(
                                db=bg_db,
                                candidate_id=ctx.attempt_id,
                                request_start_time=self.start_time,
                                global_first_byte_time_ms=ctx.first_byte_time_ms,
                            )
                        )

                    # 根据状态码决定是成功还是失败
                    # 499 = 客户端断开连接，应标记为失败
                    # 503 = 服务不可用（如流中断），应标记为失败
                    if ctx.status_code and ctx.status_code >= 400:
                        # 请求链路追踪使用 upstream_response（原始响应），回退到 error_message（友好消息）
                        trace_error_message = (
                            ctx.upstream_response or ctx.error_message or f"HTTP {ctx.status_code}"
                        )
                        extra_data = {
                            "stream_completed": False,
                            "chunk_count": ctx.chunk_count,
                            "data_count": ctx.data_count,
                        }
                        if ctx.proxy_info:
                            extra_data["proxy"] = ctx.proxy_info
                        if candidate_first_byte_time_ms is not None:
                            extra_data["first_byte_time_ms"] = candidate_first_byte_time_ms
                        if ctx.is_client_disconnected():
                            RequestCandidateService.mark_candidate_cancelled(
                                db=bg_db,
                                candidate_id=ctx.attempt_id,
                                status_code=ctx.status_code,
                                latency_ms=response_time_ms,
                                extra_data=extra_data,
                            )
                        else:
                            RequestCandidateService.mark_candidate_failed(
                                db=bg_db,
                                candidate_id=ctx.attempt_id,
                                error_type="stream_error",
                                error_message=trace_error_message,
                                status_code=ctx.status_code,
                                latency_ms=response_time_ms,
                                extra_data=extra_data,
                            )
                    else:
                        extra_data = {
                            "stream_completed": True,
                            "chunk_count": ctx.chunk_count,
                            "data_count": ctx.data_count,
                        }
                        if ctx.proxy_info:
                            extra_data["proxy"] = ctx.proxy_info
                        if ctx.rectified:
                            extra_data["rectified"] = True
                        if candidate_first_byte_time_ms is not None:
                            extra_data["first_byte_time_ms"] = candidate_first_byte_time_ms
                        RequestCandidateService.mark_candidate_success(
                            db=bg_db,
                            candidate_id=ctx.attempt_id,
                            status_code=ctx.status_code,
                            latency_ms=response_time_ms,
                            extra_data=extra_data,
                        )

            finally:
                bg_db.close()

        except Exception as e:
            logger.exception("记录流式统计信息时出错")

    async def _record_stream_failure(
        self,
        ctx: StreamContext,
        error: Exception,
        original_headers: dict[str, str],
        original_request_body: dict[str, Any],
    ) -> None:
        """记录流式请求失败"""
        # 使用 self.start_time 作为时间基准，与首字时间保持一致
        response_time_ms = int((time.time() - self.start_time) * 1000)

        status_code = 503
        if isinstance(error, ThinkingSignatureException):
            status_code = 400
        elif isinstance(error, ProviderAuthException):
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

        request_metadata = {"perf": ctx.perf_metrics} if ctx.perf_metrics else None
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
            # 格式转换追踪
            endpoint_api_format=ctx.provider_api_format or None,
            has_format_conversion=ctx.has_format_conversion,
            # 模型映射信息
            target_model=ctx.mapped_model,
            request_metadata=request_metadata,
        )

    # _update_usage_to_streaming 方法已移至基类 BaseMessageHandler

    async def process_sync(
        self,
        original_request_body: dict[str, Any],
        original_headers: dict[str, str],
        query_params: dict[str, str] | None = None,
        path_params: dict[str, Any] | None = None,
    ) -> JSONResponse:
        """
        处理非流式请求

        通用流程：
        1. 构建请求
        2. 通过 TaskService/FailoverEngine 执行
        3. 解析响应并记录统计
        """
        logger.debug(f"开始非流式响应处理 ({self.FORMAT_ID})")

        # 使用子类实现的方法提取 model（不同 API 格式的 model 位置不同）
        model = self.extract_model_from_request(original_request_body, path_params)
        api_format = self.allowed_api_formats[0]
        sync_start_time = time.time()

        # 提前创建 pending 记录，让前端可以立即看到"处理中"
        self._create_pending_usage(
            model=model,
            is_stream=False,
            request_type="chat",
            api_format=self.FORMAT_ID,
            request_headers=original_headers,
            request_body=original_request_body,
        )

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
        response_metadata_result: dict[str, Any] = {}  # Provider 响应元数据
        needs_conversion = False  # 是否需要格式转换（由 candidate 决定）
        sync_proxy_info: dict[str, Any] | None = None  # 代理信息

        # 可变请求体容器：允许 TaskService 在遇到 Thinking 签名错误时整流请求体后重试
        # 结构: {"body": 实际请求体, "_rectified": 是否已整流, "_rectified_this_turn": 本轮是否整流}
        request_body_ref: dict[str, Any] = {"body": original_request_body}

        async def sync_request_func(
            provider: Provider,
            endpoint: ProviderEndpoint,
            key: ProviderAPIKey,
            candidate: ProviderCandidate,
        ) -> dict[str, Any]:
            nonlocal provider_name, response_json, status_code, response_headers, provider_api_format, provider_request_headers, provider_request_body, mapped_model_result, response_metadata_result, needs_conversion, sync_proxy_info
            provider_name = str(provider.name)
            provider_api_format = str(endpoint.api_format) if endpoint.api_format else ""

            # 获取模型映射（优先使用映射匹配到的模型，其次是 Provider 级别的映射）
            mapped_model = candidate.mapping_matched_model if candidate else None
            if not mapped_model:
                mapped_model = await self._get_mapped_model(
                    source_model=model,
                    provider_id=str(provider.id),
                )

            # 应用模型映射到请求体（子类可覆盖此方法处理不同格式）
            if mapped_model:
                mapped_model_result = mapped_model  # 保存映射后的模型名，用于 Usage 记录
                request_body = self.apply_mapped_model(request_body_ref["body"], mapped_model)
            else:
                request_body = dict(request_body_ref["body"])

            client_api_format = (
                api_format.value if hasattr(api_format, "value") else str(api_format)
            )
            needs_conversion = bool(getattr(candidate, "needs_conversion", False))

            provider_type = str(getattr(provider, "provider_type", "") or "").lower()
            behavior = get_provider_behavior(
                provider_type=provider_type,
                endpoint_sig=provider_api_format,
            )
            envelope = behavior.envelope
            target_variant = behavior.same_format_variant
            # 跨格式转换也允许变体（Antigravity 需要保留/翻译 Claude thinking 块）
            conversion_variant = behavior.cross_format_variant

            # Upstream streaming policy (per-endpoint).
            upstream_policy = get_upstream_stream_policy(
                endpoint,
                provider_type=provider_type,
                endpoint_sig=provider_api_format,
            )
            upstream_is_stream = resolve_upstream_is_stream(
                client_is_stream=False,
                policy=upstream_policy,
            )

            # 跨格式：先做请求体转换（失败触发 failover）
            if needs_conversion and provider_api_format:
                request_body, url_model = self._convert_request_for_cross_format(
                    request_body,
                    client_api_format,
                    provider_api_format,
                    mapped_model,
                    model,
                    is_stream=upstream_is_stream,
                    target_variant=conversion_variant,
                )
            else:
                # 同格式：按原逻辑做轻量清理（子类可覆盖）
                request_body = self.prepare_provider_request_body(request_body)
                url_model = (
                    self.get_model_for_url(request_body, mapped_model) or mapped_model or model
                )
                # 同格式时也需要应用 target_variant 转换（如 Codex）
                if target_variant and provider_api_format:
                    registry = get_format_converter_registry()
                    request_body = registry.convert_request(
                        request_body,
                        provider_api_format,
                        provider_api_format,
                        target_variant=target_variant,
                    )

            # 模型感知的请求后处理（如图像生成模型移除不兼容字段）
            request_body = self.finalize_provider_request(
                request_body,
                mapped_model=mapped_model,
                provider_api_format=provider_api_format,
            )

            # Force upstream stream/sync mode in request body (best-effort).
            if provider_api_format:
                enforce_stream_mode_for_upstream(
                    request_body,
                    provider_api_format=provider_api_format,
                    upstream_is_stream=upstream_is_stream,
                )

            # 获取认证信息（处理 Service Account 等异步认证场景）
            auth_info = await get_provider_auth(endpoint, key)

            # Provider envelope: wrap request after auth is available and before RequestBuilder.build().
            if envelope:
                request_body, url_model = envelope.wrap_request(
                    request_body,
                    model=url_model or model or "",
                    url_model=url_model,
                    decrypted_auth_config=auth_info.decrypted_auth_config if auth_info else None,
                )

            # Provider envelope: extra upstream headers (e.g. dedicated User-Agent).
            extra_headers: dict[str, str] = {}
            if envelope:
                extra_headers.update(envelope.extra_headers() or {})

            # 使用 RequestBuilder 构建请求体和请求头
            # 注意：mapped_model 已经应用到 request_body，这里不再传递
            # 上游始终使用 header 认证，不跟随客户端的 query 方式
            provider_payload, provider_headers = self._request_builder.build(
                request_body,
                original_headers,
                endpoint,
                key,
                is_stream=upstream_is_stream,
                extra_headers=extra_headers if extra_headers else None,
                pre_computed_auth=auth_info.as_tuple() if auth_info else None,
            )
            if upstream_is_stream:
                from src.core.api_format.headers import set_accept_if_absent

                set_accept_if_absent(provider_headers)

            # 保存发送给 Provider 的请求信息（用于调试和统计）
            provider_request_headers = provider_headers
            provider_request_body = provider_payload

            url = build_provider_url(
                endpoint,
                query_params=query_params,
                path_params={"model": url_model},
                is_stream=upstream_is_stream,  # sync handler may still force upstream streaming
                key=key,
                decrypted_auth_config=auth_info.decrypted_auth_config if auth_info else None,
            )
            # 非流式：必须在 build_provider_url 调用后立即缓存（避免 contextvar 被后续调用覆盖）
            selected_base_url_cached = envelope.capture_selected_base_url() if envelope else None

            # 解析有效代理（Key 级别优先于 Provider 级别）
            from src.services.proxy_node.resolver import (
                get_proxy_label,
                resolve_effective_proxy,
                resolve_proxy_info,
            )

            _effective_proxy = resolve_effective_proxy(provider.proxy, getattr(key, "proxy", None))
            sync_proxy_info = resolve_proxy_info(_effective_proxy)
            _proxy_label = get_proxy_label(sync_proxy_info)

            logger.info(
                f"  └─ [{self.request_id}] 发送{'上游流式(聚合)' if upstream_is_stream else '非流式'}请求: "
                f"Provider={provider.name}, Endpoint={endpoint.id[:8] if endpoint.id else 'N/A'}..., "
                f"Key=***{key.api_key[-4:] if key.api_key else 'N/A'}, "
                f"原始模型={model}, 映射后={mapped_model or '无映射'}, URL模型={url_model}, "
                f"代理={_proxy_label}"
            )

            # 获取复用的 HTTP 客户端（支持代理配置，Key 级别优先于 Provider 级别）
            # 注意：使用 get_proxy_client 复用连接池，不再每次创建新客户端
            from src.clients.http_client import HTTPClientPool
            from src.services.proxy_node.resolver import (
                build_post_kwargs,
                build_stream_kwargs,
                resolve_delegate_config,
            )

            # 非流式请求使用 http_request_timeout 作为整体超时
            # 优先使用 Provider 配置，否则使用全局配置
            request_timeout = provider.request_timeout or config.http_request_timeout

            delegate_cfg = resolve_delegate_config(_effective_proxy)
            http_client = await HTTPClientPool.get_upstream_client(
                delegate_cfg, proxy_config=_effective_proxy
            )

            # 注意：不使用 async with，因为复用的客户端不应该被关闭
            # 超时通过 timeout 参数控制
            resp: httpx.Response | None = None
            if not upstream_is_stream:
                try:
                    _pkw = build_post_kwargs(
                        delegate_cfg,
                        url=url,
                        headers=provider_headers,
                        payload=provider_payload,
                        timeout=request_timeout,
                    )
                    resp = await http_client.post(**_pkw)
                except (httpx.ConnectError, httpx.ConnectTimeout, httpx.TimeoutException) as e:
                    if envelope:
                        envelope.on_connection_error(base_url=selected_base_url_cached, exc=e)
                        if selected_base_url_cached:
                            logger.warning(
                                f"[{envelope.name}] Connection error: {selected_base_url_cached} ({e})"
                            )
                    raise
            else:
                # Forced upstream streaming: aggregate SSE to a sync JSON response.
                registry = get_format_converter_registry()
                provider_parser = (
                    get_parser_for_format(provider_api_format) if provider_api_format else None
                )

                try:
                    _stream_args = build_stream_kwargs(
                        delegate_cfg,
                        url=url,
                        headers=provider_headers,
                        payload=provider_payload,
                        timeout=request_timeout,
                    )
                    async with http_client.stream(**_stream_args) as stream_resp:
                        resp = stream_resp

                        status_code = stream_resp.status_code
                        response_headers = dict(stream_resp.headers)
                        extract_proxy_timing(sync_proxy_info, response_headers)

                        if envelope:
                            envelope.on_http_status(
                                base_url=selected_base_url_cached,
                                status_code=status_code,
                            )

                        stream_resp.raise_for_status()

                        byte_iter = stream_resp.aiter_bytes()
                        if provider_type == "kiro" and envelope and envelope.force_stream_rewrite():
                            from src.services.provider.adapters.kiro.eventstream_rewriter import (
                                apply_kiro_stream_rewrite,
                            )

                            byte_iter = apply_kiro_stream_rewrite(byte_iter, model=str(model or ""))

                        internal_resp = await aggregate_upstream_stream_to_internal_response(
                            byte_iter,
                            provider_api_format=provider_api_format,
                            provider_name=str(provider.name),
                            model=str(model or ""),
                            request_id=str(self.request_id or ""),
                            envelope=envelope,
                            provider_parser=provider_parser,
                        )

                        tgt_norm = (
                            registry.get_normalizer(client_api_format)
                            if client_api_format
                            else None
                        )
                        if tgt_norm is None:
                            raise RuntimeError(f"未注册 Normalizer: {client_api_format}")

                        response_json = tgt_norm.response_from_internal(
                            internal_resp,
                            requested_model=model,
                        )
                        response_json = response_json if isinstance(response_json, dict) else {}

                except (httpx.ConnectError, httpx.ConnectTimeout, httpx.TimeoutException) as e:
                    if envelope:
                        envelope.on_connection_error(base_url=selected_base_url_cached, exc=e)
                        if selected_base_url_cached:
                            logger.warning(
                                f"[{envelope.name}] Connection error: {selected_base_url_cached} ({e})"
                            )
                    raise

            status_code = resp.status_code
            response_headers = dict(resp.headers)
            extract_proxy_timing(sync_proxy_info, response_headers)

            if envelope:
                envelope.on_http_status(base_url=selected_base_url_cached, status_code=status_code)

            # Forced upstream streaming already built response_json via aggregator.
            if upstream_is_stream:
                response_metadata_result = self._extract_response_metadata(response_json or {})
                return response_json if isinstance(response_json, dict) else {}

            # Reuse HTTPStatusError classification path (handled by TaskService/error_classifier).
            try:
                resp.raise_for_status()
            except httpx.HTTPStatusError as e:
                error_body = ""
                try:
                    error_body = resp.text[:4000] if resp.text else ""
                except Exception:
                    error_body = ""
                e.upstream_response = error_body  # type: ignore[attr-defined]
                raise

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

            if envelope:
                response_json = envelope.unwrap_response(response_json)
                envelope.postprocess_unwrapped_response(model=model, data=response_json)

            # 提取 Provider 响应元数据（子类可覆盖）
            response_metadata_result = self._extract_response_metadata(response_json)

            return response_json if isinstance(response_json, dict) else {}

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
            from src.services.task.context import TaskMode

            exec_result = await TaskService(self.db, self.redis).execute(
                task_type="cli",
                task_mode=TaskMode.SYNC,
                api_format=api_format,
                model_name=model,
                user_api_key=self.api_key,
                request_func=sync_request_func,
                request_id=self.request_id,
                is_stream=False,
                capability_requirements=capability_requirements or None,
                preferred_key_ids=preferred_key_ids or None,
                request_body_ref=request_body_ref,
            )
            result = exec_result.response
            actual_provider_name = exec_result.provider_name or "unknown"
            attempt_id = exec_result.request_candidate_id
            provider_id = exec_result.provider_id
            endpoint_id = exec_result.endpoint_id
            key_id = exec_result.key_id

            provider_name = actual_provider_name
            response_time_ms = int((time.time() - sync_start_time) * 1000)

            # 确保 response_json 不为 None
            if response_json is None:
                response_json = {}

            # 跨格式：响应转换回 client_format（失败不触发 failover，保守回退为原始响应）
            if (
                needs_conversion
                and provider_api_format
                and api_format
                and isinstance(response_json, dict)
            ):
                try:
                    registry = get_format_converter_registry()
                    response_json = registry.convert_response(
                        response_json,
                        provider_api_format,
                        api_format,
                        requested_model=model,  # 使用用户请求的原始模型名
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

            output_text = self.parser.extract_text_content(response_json)[:200]

            # 使用实际发送给 Provider 的请求体（如果有），否则用原始请求体
            actual_request_body = provider_request_body or original_request_body

            # 非流式成功时，返回给客户端的是提供商响应头（透传）
            client_response_headers = filter_proxy_response_headers(response_headers)
            client_response_headers["content-type"] = "application/json"

            request_metadata = self._build_request_metadata() or {}
            if sync_proxy_info:
                request_metadata["proxy"] = sync_proxy_info
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
                # 格式转换追踪
                endpoint_api_format=provider_api_format or None,
                has_format_conversion=is_format_converted(provider_api_format, str(api_format)),
                # Provider 侧追踪信息（用于记录真实成本）
                provider_id=provider_id,
                provider_endpoint_id=endpoint_id,
                provider_api_key_id=key_id,
                # 模型映射信息
                target_model=mapped_model_result,
                # Provider 响应元数据（如 Gemini 的 modelVersion）
                response_metadata=response_metadata_result if response_metadata_result else None,
                request_metadata=request_metadata or None,
            )

            logger.info(f"{self.FORMAT_ID} 非流式响应处理完成")

            # 透传提供商的响应头
            return JSONResponse(
                status_code=status_code,
                content=response_json,
                headers=client_response_headers,
            )

        except ThinkingSignatureException as e:
            # Thinking 签名错误：TaskService 层已处理整流重试但仍失败
            # 记录实际发送给 Provider 的请求体，便于排查问题根因
            response_time_ms = int((time.time() - sync_start_time) * 1000)
            actual_request_body = provider_request_body or original_request_body
            request_metadata = self._build_request_metadata() or {}
            if sync_proxy_info:
                request_metadata["proxy"] = sync_proxy_info
            await self.telemetry.record_failure(
                provider=provider_name or "unknown",
                model=model,
                response_time_ms=response_time_ms,
                status_code=e.status_code or 400,
                request_headers=original_headers,
                request_body=actual_request_body,
                error_message=str(e),
                is_stream=False,
                api_format=api_format,
                request_metadata=request_metadata or None,
            )
            raise

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
            error_response_headers: dict[str, str] = {}
            if isinstance(e, ProviderRateLimitException) and e.response_headers:
                error_response_headers = e.response_headers
            elif isinstance(e, httpx.HTTPStatusError) and hasattr(e, "response"):
                error_response_headers = dict(e.response.headers)

            request_metadata = self._build_request_metadata() or {}
            if sync_proxy_info:
                request_metadata["proxy"] = sync_proxy_info
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
                # 格式转换追踪
                endpoint_api_format=provider_api_format or None,
                has_format_conversion=is_format_converted(provider_api_format, str(api_format)),
                # 模型映射信息
                target_model=mapped_model_result,
                request_metadata=request_metadata or None,
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
        [已废弃] 仅根据格式差异判断是否需要转换

        警告：此方法只检查格式是否不同，不检查端点的 format_acceptance_config 配置！
        正确的判断应使用候选筛选阶段的结果（ctx.needs_conversion），该结果由
        is_format_compatible() 函数根据全局开关和端点配置计算得出。

        此方法保留仅供调试和日志输出使用，流生成器中不应调用此方法。

        当 Provider 的 API 格式与客户端请求的 API 格式不同时，需要转换响应。
        例如：客户端请求 Claude 格式，但 Provider 返回 OpenAI 格式。

        注意：
        - CLAUDE 和 CLAUDE_CLI、GEMINI 和 GEMINI_CLI：格式相同，只是认证不同，可透传
        - OPENAI 和 OPENAI_CLI：格式不同（Chat Completions vs Responses API），需要转换
        """
        from src.core.api_format.metadata import can_passthrough_endpoint
        from src.core.api_format.signature import normalize_signature_key

        if not ctx.provider_api_format or not ctx.client_api_format:
            logger.debug(
                f"[{getattr(ctx, 'request_id', 'unknown')}] _needs_format_conversion: "
                f"provider_api_format={ctx.provider_api_format!r}, client_api_format={ctx.client_api_format!r} -> False (missing)"
            )
            return False

        provider_format = normalize_signature_key(str(ctx.provider_api_format))
        client_format = normalize_signature_key(str(ctx.client_api_format))

        # 1. 格式完全匹配 -> 不需要转换
        if provider_format == client_format:
            logger.debug(
                f"[{getattr(ctx, 'request_id', 'unknown')}] _needs_format_conversion: "
                f"provider={provider_format}, client={client_format} -> False (exact match)"
            )
            return False

        # 2. 根据 data_format_id 判断是否可透传（可透传则不需要转换）
        if can_passthrough_endpoint(client_format, provider_format):
            logger.debug(
                f"[{getattr(ctx, 'request_id', 'unknown')}] _needs_format_conversion: "
                f"provider={provider_format}, client={client_format} -> False (passthroughable)"
            )
            return False

        # 3. 其他情况 -> 需要转换
        logger.debug(
            f"[{getattr(ctx, 'request_id', 'unknown')}] _needs_format_conversion: "
            f"provider={provider_format}, client={client_format} -> True"
        )
        return True

    def _mark_first_output(self, ctx: StreamContext, state: dict[str, bool]) -> None:
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
                # 优先使用当前请求的 DB 会话同步更新，避免状态延迟或丢失
                try:
                    from src.services.usage import UsageService

                    UsageService.update_usage_status(
                        db=self.db,
                        request_id=self.request_id,
                        status="streaming",
                        provider=ctx.provider_name,
                        target_model=ctx.mapped_model,
                        provider_id=ctx.provider_id,
                        provider_endpoint_id=ctx.endpoint_id,
                        provider_api_key_id=ctx.key_id,
                        first_byte_time_ms=ctx.first_byte_time_ms,
                        api_format=ctx.api_format,
                        endpoint_api_format=ctx.provider_api_format or None,
                        has_format_conversion=ctx.has_format_conversion,
                    )
                except Exception as e:
                    logger.warning(f"[{self.request_id}] 同步更新 streaming 状态失败: {e}")
                    # 回退到后台任务更新
                    self._update_usage_to_streaming_with_ctx(ctx)
                state["streaming_updated"] = True

    def _convert_sse_line(
        self,
        ctx: StreamContext,
        line: str,
        events: list,  # noqa: ARG002 - 预留给上下文感知转换
    ) -> tuple[list[str], list[dict[str, Any]]]:
        """
        将 SSE 行从 Provider 格式转换为客户端格式

        Args:
            ctx: 流上下文
            line: 原始 SSE 行
            events: 当前累积的事件列表（预留参数，用于未来上下文感知转换如合并相邻事件）

        Returns:
            (sse_lines, converted_events) 元组：
            - sse_lines: 转换后的 SSE 行列表（一入多出），空列表表示跳过该行
            - converted_events: 转换后的事件对象列表（用于记录到 parsed_chunks）
        """
        # 空行直接返回
        if not line or line.strip() == "":
            return ([line] if line else [], [])

        client_format = (ctx.client_api_format or "").strip().lower()

        # [DONE] 标记处理：只有 OpenAI 客户端需要，Claude 客户端不需要
        if line == "data: [DONE]":
            if client_format.startswith("openai"):
                return [line], []
            else:
                # Claude/Gemini 客户端不需要 [DONE] 标记
                return [], []

        provider_format = (ctx.provider_api_format or "").strip().lower()

        # 过滤上游控制行（id/retry），避免与目标格式混淆
        if line.startswith(("id:", "retry:")):
            return [], []

        # 解析 SSE 行为 JSON 对象
        data_obj, status = self._parse_sse_line_to_json(line, provider_format)

        # 根据解析状态决定行为
        if status == "empty" or status == "skip":
            return [], []
        if status == "invalid" or status == "passthrough":
            return [line], []

        behavior = get_provider_behavior(
            provider_type=ctx.provider_type,
            endpoint_sig=ctx.provider_api_format,
        )
        envelope = behavior.envelope
        if envelope:
            data_obj = envelope.unwrap_response(data_obj)
            envelope.postprocess_unwrapped_response(model=ctx.model, data=data_obj)

        # 初始化流式转换状态
        if ctx.stream_conversion_state is None:
            from src.core.api_format.conversion.stream_state import StreamState

            # 使用客户端请求的模型（ctx.model），而非映射后的上游模型（ctx.mapped_model）
            init_model = ctx.model or ""
            logger.debug(
                f"[{ctx.request_id}] StreamState init: ctx.model={ctx.model!r}, "
                f"mapped_model={ctx.mapped_model!r}, using={init_model!r}"
            )
            ctx.stream_conversion_state = StreamState(
                model=init_model,
                message_id=ctx.response_id or ctx.request_id or "",
            )

        # 执行格式转换
        try:
            registry = get_format_converter_registry()
            # status == "ok" 时 data_obj 必定是有效的 dict（防御性检查）
            if data_obj is None:
                return [], []
            converted_events = registry.convert_stream_chunk(
                data_obj,
                provider_format,
                client_format,
                state=ctx.stream_conversion_state,
            )
            result = _format_converted_events_to_sse(converted_events, client_format)
            if result:
                logger.debug(
                    f"[{getattr(ctx, 'request_id', 'unknown')}] 流式转换: "
                    f"{provider_format}->{client_format}, events={len(converted_events)}, "
                    f"first_output={result[0][:100] if result else 'empty'}..."
                )
            return result, converted_events

        except Exception as e:
            logger.warning(f"格式转换失败，透传原始数据: {e}")
            return [line], []

    def _parse_sse_line_to_json(self, line: str, provider_format: str) -> tuple[Any | None, str]:
        """
        解析 SSE 行为 JSON 对象

        支持多种格式：
        - 标准 SSE: "data: {...}"
        - event+data 同行: "event: xxx data: {...}"
        - Gemini JSON-array: 裸 JSON 行

        Args:
            line: 原始 SSE 行
            provider_format: Provider API 格式

        Returns:
            (parsed_json, status) 元组：
            - (obj, "ok") - 解析成功
            - (None, "empty") - 内容为空，应跳过
            - (None, "invalid") - JSON 解析失败，应透传原始行
            - (None, "skip") - 应跳过（如纯 event 行）
            - (None, "passthrough") - 无法识别，应透传原始行
        """
        # 标准 SSE: data: {...}
        if line.startswith("data:"):
            return _parse_sse_data_line(line)

        # event + data 同行: event: xxx data: {...}
        if line.startswith("event:") and " data:" in line:
            return _parse_sse_event_data_line(line)

        # 纯 event 行不参与转换
        if line.startswith("event:"):
            return None, "skip"

        # Gemini JSON-array 格式
        if provider_format.startswith("gemini"):
            return _parse_gemini_json_array_line(line)

        # 其他格式：无法识别，透传
        return None, "passthrough"
