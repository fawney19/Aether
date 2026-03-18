"""CLI Handler - 流式处理核心 Mixin"""

from __future__ import annotations

import asyncio
import codecs
import json
import time
from collections.abc import AsyncGenerator
from typing import TYPE_CHECKING, Any

import httpx
from fastapi import BackgroundTasks, Request
from fastapi.responses import StreamingResponse

from src.api.handlers.base.base_handler import (
    ClientDisconnectedException,
    wait_for_with_disconnect_detection,
)
from src.api.handlers.base.parsers import get_parser_for_format
from src.api.handlers.base.request_builder import get_provider_auth
from src.api.handlers.base.stream_context import StreamContext
from src.api.handlers.base.utils import (
    build_sse_headers,
    ensure_stream_buffer_limit,
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
)
from src.core.logger import logger
from src.services.provider.behavior import get_provider_behavior
from src.services.scheduling.aware_scheduler import ProviderCandidate
from src.services.system.config import SystemConfigService
from src.services.task.request_state import MutableRequestBodyState
from src.utils.sse_parser import SSEEventParser

from .cli_sse_helpers import _format_converted_events_to_sse

if TYPE_CHECKING:
    from src.api.handlers.base.cli_protocol import CliHandlerProtocol
    from src.models.database import Provider, ProviderAPIKey, ProviderEndpoint


class CliStreamMixin:
    """流式处理核心方法的 Mixin"""

    async def process_stream(
        self: CliHandlerProtocol,
        original_request_body: dict[str, Any],
        original_headers: dict[str, str],
        query_params: dict[str, str] | None = None,
        path_params: dict[str, Any] | None = None,
        http_request: Request | None = None,
        client_content_encoding: str | None = None,
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
        logger.debug("开始流式响应处理 ({})", self.FORMAT_ID)
        effective_client_content_encoding = resolve_client_content_encoding(
            original_headers,
            client_content_encoding,
        )

        request_state = MutableRequestBodyState(original_request_body)

        # 使用子类实现的方法提取 model（不同 API 格式的 model 位置不同）
        # 注意：使用 original_request_body，因为整流只修改 messages，不影响 model 字段
        model = self.extract_model_from_request(original_request_body, path_params)
        client_api_format = self.primary_api_format

        # 提前创建 pending 记录，让前端可以立即看到"处理中"
        pending_usage_created = self._create_pending_usage(
            model=model,
            is_stream=True,
            request_type="chat",
            api_format=client_api_format,
            request_headers=original_headers,
            request_body=original_request_body,
        )

        # 创建流上下文
        ctx = StreamContext(
            model=model,
            api_format=client_api_format,
            api_family=self.api_family,
            endpoint_kind=self.endpoint_kind,
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
            provider: "Provider",
            endpoint: "ProviderEndpoint",
            key: "ProviderAPIKey",
            candidate: ProviderCandidate,
        ) -> AsyncGenerator[bytes]:
            return await self._execute_stream_request(
                ctx,
                provider,
                endpoint,
                key,
                request_state.build_attempt_body(),
                original_headers,
                query_params,
                candidate,
                http_request,  # 传递 http_request 用于断连检测
                effective_client_content_encoding,
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
            from src.services.task.core.context import TaskMode

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

        except Exception as e:
            from src.core.exceptions import ThinkingSignatureException

            if isinstance(e, ThinkingSignatureException):
                # Thinking 签名错误：TaskService 层已处理整流重试但仍失败
                # 记录 original_request_body（客户端原始请求），便于排查问题根因
                self._log_request_error("流式请求失败（签名错误）", e)
            else:
                self._log_request_error("流式请求失败", e)
            await self._record_stream_failure(ctx, e, original_headers, original_request_body)
            raise

    async def _execute_stream_request(
        self,
        ctx: StreamContext,
        provider: "Provider",
        endpoint: "ProviderEndpoint",
        key: "ProviderAPIKey",
        working_request_body: dict[str, Any],
        original_headers: dict[str, str],
        query_params: dict[str, str] | None = None,
        candidate: ProviderCandidate | None = None,
        http_request: Request | None = None,
        client_content_encoding: str | None = None,
    ) -> AsyncGenerator[bytes]:
        """执行流式请求并返回流生成器"""
        # 重置上下文状态（重试时清除之前的数据，避免累积）
        ctx.release_recorded_chunks()
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

        # `working_request_body` is already isolated per attempt.
        request_body = working_request_body
        if mapped_model:
            ctx.mapped_model = mapped_model  # 保存映射后的模型名，用于 Usage 记录
            request_body = self.apply_mapped_model(request_body, mapped_model)

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

        upstream_request = await self._build_upstream_request(
            provider=provider,
            endpoint=endpoint,
            key=key,
            request_body=request_body,
            original_headers=original_headers,
            query_params=query_params,
            client_api_format=client_api_format,
            provider_api_format=provider_api_format,
            fallback_model=ctx.model,
            mapped_model=mapped_model,
            client_is_stream=True,
            needs_conversion=needs_conversion,
            output_limit=candidate.output_limit if candidate else None,
        )
        provider_headers = upstream_request.headers
        provider_payload = upstream_request.payload
        url = upstream_request.url
        envelope = upstream_request.envelope
        upstream_is_stream = upstream_request.upstream_is_stream
        envelope_tls_profile = upstream_request.tls_profile

        # 保存发送给 Provider 的请求信息（用于调试和统计）
        ctx.provider_request_headers = provider_headers
        ctx.provider_request_body = provider_payload
        ctx.selected_base_url = upstream_request.selected_base_url

        # 解析有效代理（Key 级别优先于 Provider 级别）
        from src.services.proxy_node.resolver import get_proxy_label as _gpl
        from src.services.proxy_node.resolver import resolve_effective_proxy as _rep
        from src.services.proxy_node.resolver import resolve_proxy_info_async as _rpi_async

        effective_proxy = _rep(provider.proxy, getattr(key, "proxy", None))
        ctx.proxy_info = await _rpi_async(effective_proxy)

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
                tls_profile=envelope_tls_profile,
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
                    _pkw = await build_post_kwargs_async(
                        delegate_cfg,
                        url=url,
                        headers=provider_headers,
                        payload=provider_payload,
                        timeout=request_timeout_sync,
                        client_content_encoding=client_content_encoding,
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
            f"原始模型={ctx.model}, 映射后={mapped_model or '无映射'}, URL模型={upstream_request.url_model}, "
            f"timeout={request_timeout}s, 代理={_proxy_label}"
        )

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
            tls_profile=envelope_tls_profile,
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
            _connect_start = time.monotonic()
            response_ctx = http_client.stream(**_skw)
            stream_response = await response_ctx.__aenter__()
            ctx.set_ttfb_ms(int((time.monotonic() - _connect_start) * 1000))

            ctx.status_code = stream_response.status_code
            ctx.response_headers = dict(stream_response.headers)
            ctx.set_proxy_timing(ctx.response_headers)

            logger.debug("  └─ 收到响应: status={}", stream_response.status_code)

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
                logger.warning("  [{}] 客户端在等待首字节时断开连接", self.request_id)
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

                try:
                    if response_ctx is not None:
                        await response_ctx.__aexit__(None, None, None)
                except Exception:
                    pass
                finally:
                    response_ctx = None
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

        # 创建流生成器（带预读数据，使用同一个迭代器）
        return self._create_response_stream_with_prefetch(
            ctx,
            byte_iterator,
            response_ctx,
            prefetched_chunks,
        )

    @staticmethod
    def _fire_stream_timeout_policy(ctx: StreamContext) -> None:
        """Fire-and-forget: record stream timeout for pool health policy."""
        if not ctx.provider_id or not ctx.key_id:
            return
        try:
            from src.services.provider.adapters.claude_code.context import (
                get_claude_code_request_context,
            )

            cc_ctx = get_claude_code_request_context()
            pool_cfg = cc_ctx.pool_config if cc_ctx else None
            if not pool_cfg:
                return

            from src.services.provider.pool.health_policy import apply_stream_timeout_policy

            task = asyncio.create_task(
                apply_stream_timeout_policy(
                    provider_id=ctx.provider_id,
                    key_id=ctx.key_id,
                    config=pool_cfg,
                )
            )
            task.add_done_callback(lambda t: t.exception() if not t.cancelled() else None)
        except Exception as exc:
            logger.debug("Stream timeout policy trigger failed: {}", exc)

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
            _sample_lines: list[str] = []  # 采集前几行原始内容，用于空流诊断
            _MAX_SAMPLE_LINES = 5
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
                ensure_stream_buffer_limit(
                    buffer,
                    request_id=self.request_id,
                    provider_name=ctx.provider_name,
                )
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
                    if len(_sample_lines) < _MAX_SAMPLE_LINES:
                        _sample_lines.append(normalized_line[:200])

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

                            self._fire_stream_timeout_policy(ctx)

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

            # flush 字节 buffer 残余数据 + SSE parser 内部缓冲区
            for chunk in self._flush_buffer_with_conversion(
                ctx, buffer, decoder, sse_parser, needs_conversion
            ):
                yield chunk

            # 检查是否收到数据
            if ctx.data_count == 0:
                sample_info = f", 前几行内容: {_sample_lines!r}" if _sample_lines else ""
                logger.warning(
                    "Provider '{}' 返回空流式响应{}",
                    ctx.provider_name,
                    sample_info,
                )
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
                logger.warning("Provider '{}' 流连接关闭且无数据", ctx.provider_name)
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
