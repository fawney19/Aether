"""CLI Handler - 同步处理 Mixin"""

from __future__ import annotations

import json
import time
from typing import TYPE_CHECKING, Any

import httpx
from fastapi.responses import JSONResponse

from src.api.handlers.base.parsers import get_parser_for_format
from src.api.handlers.base.stream_context import extract_proxy_timing, is_format_converted
from src.api.handlers.base.upstream_stream_bridge import (
    aggregate_upstream_stream_to_internal_response,
)
from src.api.handlers.base.utils import (
    build_json_response_for_client,
    filter_proxy_response_headers,
    get_format_converter_registry,
    resolve_client_accept_encoding,
    resolve_client_content_encoding,
)
from src.config.settings import config
from src.core.error_utils import extract_client_error_message
from src.core.exceptions import (
    ProviderAuthException,
    ProviderNotAvailableException,
    ProviderRateLimitException,
    ProviderTimeoutException,
    ThinkingSignatureException,
)
from src.core.logger import logger
from src.services.scheduling.aware_scheduler import ProviderCandidate
from src.services.task.request_state import MutableRequestBodyState

if TYPE_CHECKING:
    from src.api.handlers.base.cli_protocol import CliHandlerProtocol
    from src.models.database import Provider, ProviderAPIKey, ProviderEndpoint


class CliSyncMixin:
    """同步处理相关方法的 Mixin"""

    async def process_sync(
        self: CliHandlerProtocol,
        original_request_body: dict[str, Any],
        original_headers: dict[str, str],
        query_params: dict[str, str] | None = None,
        path_params: dict[str, Any] | None = None,
        client_content_encoding: str | None = None,
        client_accept_encoding: str | None = None,
    ) -> JSONResponse:
        """
        处理非流式请求

        通用流程：
        1. 构建请求
        2. 通过 TaskService/FailoverEngine 执行
        3. 解析响应并记录统计
        """
        logger.debug("开始非流式响应处理 ({})", self.FORMAT_ID)
        effective_client_content_encoding = resolve_client_content_encoding(
            original_headers,
            client_content_encoding,
        )
        effective_client_accept_encoding = resolve_client_accept_encoding(
            original_headers,
            client_accept_encoding,
        )

        # 使用子类实现的方法提取 model（不同 API 格式的 model 位置不同）
        model = self.extract_model_from_request(original_request_body, path_params)
        api_format = self.primary_api_format
        sync_start_time = time.time()

        # 提前创建 pending 记录，让前端可以立即看到"处理中"
        pending_usage_created = self._create_pending_usage(
            model=model,
            is_stream=False,
            request_type="chat",
            api_format=api_format,
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
        exec_result = None
        mapped_model_result = None  # 映射后的目标模型名（用于 Usage 记录）
        response_metadata_result: dict[str, Any] = {}  # Provider 响应元数据
        needs_conversion = False  # 是否需要格式转换（由 candidate 决定）
        sync_proxy_info: dict[str, Any] | None = None  # 代理信息

        request_state = MutableRequestBodyState(original_request_body)

        async def sync_request_func(
            provider: "Provider",
            endpoint: "ProviderEndpoint",
            key: "ProviderAPIKey",
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

            request_body = request_state.build_attempt_body()
            if mapped_model:
                mapped_model_result = mapped_model  # 保存映射后的模型名，用于 Usage 记录
                request_body = self.apply_mapped_model(request_body, mapped_model)

            client_api_format = (
                api_format.value if hasattr(api_format, "value") else str(api_format)
            )
            needs_conversion = bool(getattr(candidate, "needs_conversion", False))

            upstream_request = await self._build_upstream_request(
                provider=provider,
                endpoint=endpoint,
                key=key,
                request_body=request_body,
                original_headers=original_headers,
                query_params=query_params,
                client_api_format=client_api_format,
                provider_api_format=provider_api_format,
                fallback_model=model,
                mapped_model=mapped_model,
                client_is_stream=False,
                needs_conversion=needs_conversion,
                output_limit=candidate.output_limit if candidate else None,
            )
            provider_headers = upstream_request.headers
            provider_payload = upstream_request.payload
            provider_request_headers = provider_headers
            provider_request_body = provider_payload
            url = upstream_request.url
            envelope = upstream_request.envelope
            upstream_is_stream = upstream_request.upstream_is_stream
            envelope_tls_profile = upstream_request.tls_profile
            selected_base_url_cached = upstream_request.selected_base_url

            # 解析有效代理（Key 级别优先于 Provider 级别）
            from src.services.proxy_node.resolver import (
                get_proxy_label,
                resolve_effective_proxy,
                resolve_proxy_info_async,
            )

            _effective_proxy = resolve_effective_proxy(provider.proxy, getattr(key, "proxy", None))
            sync_proxy_info = await resolve_proxy_info_async(_effective_proxy)
            _proxy_label = get_proxy_label(sync_proxy_info)

            logger.info(
                f"  └─ [{self.request_id}] 发送{'上游流式(聚合)' if upstream_is_stream else '非流式'}请求: "
                f"Provider={provider.name}, Endpoint={endpoint.id[:8] if endpoint.id else 'N/A'}..., "
                f"Key=***{key.api_key[-4:] if key.api_key else 'N/A'}, "
                f"原始模型={model}, 映射后={mapped_model or '无映射'}, URL模型={upstream_request.url_model}, "
                f"代理={_proxy_label}"
            )

            # 获取复用的 HTTP 客户端（支持代理配置，Key 级别优先于 Provider 级别）
            # 注意：使用 get_proxy_client 复用连接池，不再每次创建新客户端
            from src.clients.http_client import HTTPClientPool
            from src.services.proxy_node.resolver import (
                build_post_kwargs_async,
                build_stream_kwargs_async,
                resolve_delegate_config_async,
            )

            # 非流式请求使用 http_request_timeout 作为整体超时
            # 优先使用 Provider 配置，否则使用全局配置
            request_timeout = provider.request_timeout or config.http_request_timeout

            delegate_cfg = await resolve_delegate_config_async(_effective_proxy)
            http_client = await HTTPClientPool.get_upstream_client(
                delegate_cfg,
                proxy_config=_effective_proxy,
                tls_profile=envelope_tls_profile,
            )

            # 注意：不使用 async with，因为复用的客户端不应该被关闭
            # 超时通过 timeout 参数控制
            resp: httpx.Response | None = None
            if not upstream_is_stream:
                try:
                    _pkw = await build_post_kwargs_async(
                        delegate_cfg,
                        url=url,
                        headers=provider_headers,
                        payload=provider_payload,
                        timeout=request_timeout,
                        client_content_encoding=effective_client_content_encoding,
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
                    _stream_args = await build_stream_kwargs_async(
                        delegate_cfg,
                        url=url,
                        headers=provider_headers,
                        payload=provider_payload,
                        timeout=request_timeout,
                        client_content_encoding=effective_client_content_encoding,
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
                        _provider_type = str(getattr(provider, "provider_type", "") or "").lower()
                        if (
                            _provider_type == "kiro"
                            and envelope
                            and envelope.force_stream_rewrite()
                        ):
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
            from src.services.task.core.context import TaskMode

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
                request_body_state=request_state,
                request_headers=original_headers,
                request_body=original_request_body,
                # 预创建失败时，回退到 TaskService 侧创建，避免丢失 pending 状态。
                create_pending_usage=not pending_usage_created,
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
            provider_response_json: dict[str, Any] | None = None
            if (
                needs_conversion
                and provider_api_format
                and api_format
                and isinstance(response_json, dict)
            ):
                try:
                    provider_response_json = response_json.copy()
                    registry = get_format_converter_registry()
                    response_json = registry.convert_response(
                        response_json,
                        provider_api_format,
                        api_format,
                        requested_model=model,  # 使用用户请求的原始模型名
                    )
                    logger.debug(
                        "非流式响应格式转换完成: {} -> {}", provider_api_format, api_format
                    )
                except Exception as conv_err:
                    logger.warning("非流式响应格式转换失败，使用原始响应: {}", conv_err)
                    provider_response_json = None

            # 使用解析器提取 usage
            usage = self.parser.extract_usage_from_response(response_json)
            input_tokens = usage.get("input_tokens", 0)
            output_tokens = usage.get("output_tokens", 0)
            cached_tokens = usage.get("cache_read_tokens", 0)
            cache_creation_tokens = usage.get("cache_creation_tokens", 0)

            output_text = self.parser.extract_text_content(response_json)[:200]

            # 非流式成功时，返回给客户端的是提供商响应头（透传）
            client_response_headers = filter_proxy_response_headers(response_headers)
            client_response_headers["content-type"] = "application/json"
            client_response = build_json_response_for_client(
                status_code=status_code,
                content=response_json,
                headers=client_response_headers,
                client_accept_encoding=effective_client_accept_encoding,
            )
            actual_client_response_headers = dict(client_response.headers)

            request_metadata = self._build_request_metadata() or {}
            if sync_proxy_info:
                request_metadata["proxy"] = sync_proxy_info
            request_metadata = self._merge_scheduling_metadata(
                request_metadata,
                exec_result=exec_result,
                selected_key_id=key_id,
            )
            total_cost = await self.telemetry.record_success(
                provider=provider_name,
                model=model,
                input_tokens=input_tokens,
                output_tokens=output_tokens,
                response_time_ms=response_time_ms,
                status_code=status_code,
                request_headers=original_headers,
                request_body=original_request_body,
                response_headers=response_headers,
                client_response_headers=actual_client_response_headers,
                response_body=provider_response_json or response_json,
                client_response_body=response_json if provider_response_json else None,
                provider_request_body=provider_request_body,
                cache_creation_tokens=cache_creation_tokens,
                cache_read_tokens=cached_tokens,
                is_stream=False,
                provider_request_headers=provider_request_headers,
                api_format=api_format,
                api_family=self.api_family,
                endpoint_kind=self.endpoint_kind,
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
                request_metadata=request_metadata,
            )

            logger.info("{} 非流式响应处理完成", self.FORMAT_ID)

            # 透传提供商的响应头
            return client_response

        except ThinkingSignatureException as e:
            # Thinking 签名错误：TaskService 层已处理整流重试但仍失败
            # 记录实际发送给 Provider 的请求体，便于排查问题根因
            response_time_ms = int((time.time() - sync_start_time) * 1000)
            request_metadata = self._build_request_metadata() or {}
            if sync_proxy_info:
                request_metadata["proxy"] = sync_proxy_info
            request_metadata = self._merge_scheduling_metadata(
                request_metadata,
                selected_key_id=key_id,
                pool_summary=getattr(exec_result, "pool_summary", None),
                fallback_from_request=True,
            )
            await self.telemetry.record_failure(
                provider=provider_name or "unknown",
                model=model,
                response_time_ms=response_time_ms,
                status_code=e.status_code or 400,
                request_headers=original_headers,
                request_body=original_request_body,
                provider_request_body=provider_request_body,
                error_message=str(e),
                is_stream=False,
                api_format=api_format,
                api_family=self.api_family,
                endpoint_kind=self.endpoint_kind,
                request_metadata=request_metadata,
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

            # 尝试从异常中提取响应头
            error_response_headers: dict[str, str] = {}
            if isinstance(e, ProviderRateLimitException) and e.response_headers:
                error_response_headers = e.response_headers
            elif isinstance(e, httpx.HTTPStatusError) and hasattr(e, "response"):
                error_response_headers = dict(e.response.headers)

            request_metadata = self._build_request_metadata() or {}
            if sync_proxy_info:
                request_metadata["proxy"] = sync_proxy_info
            request_metadata = self._merge_scheduling_metadata(
                request_metadata,
                selected_key_id=key_id,
                pool_summary=getattr(exec_result, "pool_summary", None),
                fallback_from_request=True,
            )
            await self.telemetry.record_failure(
                provider=provider_name or "unknown",
                model=model,
                response_time_ms=response_time_ms,
                status_code=status_code,
                error_message=extract_client_error_message(e),
                request_headers=original_headers,
                request_body=original_request_body,
                provider_request_body=provider_request_body,
                is_stream=False,
                api_format=api_format,
                api_family=self.api_family,
                endpoint_kind=self.endpoint_kind,
                provider_request_headers=provider_request_headers,
                response_headers=error_response_headers,
                # 非流式失败返回给客户端的是 JSON 错误响应
                client_response_headers={"content-type": "application/json"},
                # 格式转换追踪
                endpoint_api_format=provider_api_format or None,
                has_format_conversion=is_format_converted(provider_api_format, str(api_format)),
                # 模型映射信息
                target_model=mapped_model_result,
                request_metadata=request_metadata,
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
