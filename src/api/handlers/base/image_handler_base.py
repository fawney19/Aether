"""
Image Handler 基类 — 对齐 chat handler 基础设施。

设计要点：
- 继承 :class:`BaseMessageHandler`，复用 ``self.telemetry`` / ``self.redis`` /
  ``_create_pending_usage`` / ``_log_request_error``，不再各自手写 Usage 落库。
- 所有请求统一走 ``TaskService.execute(task_mode=SYNC, is_stream=False)``：
  FailoverEngine 负责候选遍历、ErrorClassifier 分类、RequestCandidate 写入、
  OAuth 401 force_refresh 重试、连续失败退避与客户端轮换。
- Codex 分支在 ``request_func`` 内做流式聚合（上游 stream=true → 聚合成 dict），
  对 FailoverEngine 层表现为同步。
- 客户端 ``stream=true`` 时把聚合 JSON 包成单帧 ``image_generation.completed`` SSE，
  这样无论 Codex 还是透传都走同一条路径。
- multipart ``/v1/images/edits`` 与 JSON ``/v1/images/generations`` 共用同一管线，
  仅在上游请求阶段按 ``raw_body`` / ``raw_content_type`` 切换字节转发。

成功/失败的 Usage 记录：
- 成功：``self.telemetry.record_success(request_type="image", provider=ctx.provider_name,
  provider_id/endpoint_id/key_id=ctx.*, target_model=ctx.mapped_model,
  endpoint_api_format=ctx.provider_api_format, has_format_conversion=..., ...)``
- 失败：``self.telemetry.record_failure(..., provider=ctx.provider_name or "unknown",
  target_model=ctx.mapped_model, ...)`` —— 只在真正拿不到候选时才会落到 "unknown"。
"""

from __future__ import annotations

import json
from abc import ABC, abstractmethod
from collections.abc import AsyncIterator
from dataclasses import dataclass, field
from typing import Any, ClassVar

import httpx
from fastapi import HTTPException, Request
from fastapi.responses import JSONResponse, StreamingResponse

from src.api.handlers.base.base_handler import BaseMessageHandler
from src.api.handlers.base.request_builder import evaluate_condition
from src.clients.http_client import HTTPClientPool
from src.core.api_format import (
    ApiFamily,
    EndpointKind,
    build_upstream_headers_for_endpoint,
    get_extra_headers_from_endpoint,
    make_signature_key,
)
from src.core.api_format.headers import HOP_BY_HOP_HEADERS
from src.core.crypto import crypto_service
from src.core.exceptions import ProviderNotAvailableException, UpstreamClientException
from src.core.logger import logger
from src.core.provider_types import ProviderType
from src.models.database import ProviderAPIKey, ProviderEndpoint
from src.services.provider.auth import get_provider_auth
from src.services.provider.adapters.codex.image_transform import (
    _CodexImageStreamTranslator,
    build_codex_stream_error_frame,
    iter_codex_image_stream_frames,
)
from src.services.provider.adapters.codex.sse_events import (
    CodexStreamError,
    extract_codex_image_usage,
)
from src.services.provider.body_transformer import (
    get_body_transformer,
    transform_request_body,
)
from src.services.provider.transport import build_provider_url
from src.services.provider.upstream_headers import build_upstream_extra_headers
from src.services.proxy_node.resolver import (
    get_proxy_label,
    resolve_delegate_config_async,
    resolve_effective_proxy,
    resolve_proxy_info_async,
)
from src.services.scheduling.aware_scheduler import ProviderCandidate
from src.services.usage.service import UsageService


def _sanitize_headers(headers: dict[str, str] | None) -> dict[str, str]:
    """脱敏请求头用于审计。"""
    if not headers:
        return {}
    drop = {"authorization", "x-api-key", "cookie", "x-goog-api-key"}
    return {k: v for k, v in headers.items() if k.lower() not in drop}


def _is_codex_candidate(candidate: ProviderCandidate | None) -> bool:
    if candidate is None:
        return False
    pt = str(getattr(candidate.provider, "provider_type", "") or "").strip().lower()
    return pt == ProviderType.CODEX.value


def _strip_internal_fields(body: dict[str, Any]) -> dict[str, Any]:
    """聚合 JSON 过滤掉下划线前缀的内部元数据（如 ``_codex_image_gen``）。

    这些字段供 handler 内部做 usage metadata / 观察性统计使用，不应透出给客户端。
    """
    if not isinstance(body, dict):
        return body
    return {k: v for k, v in body.items() if not str(k).startswith("_")}


@dataclass
class ImageRequestContext:
    """单次图像请求的上下文，跨 request_func / telemetry 共享。"""

    provider_name: str | None = None
    provider_id: str | None = None
    endpoint_id: str | None = None
    key_id: str | None = None
    candidate: ProviderCandidate | None = None
    mapped_model: str | None = None
    provider_api_format: str | None = None
    provider_request_headers: dict[str, str] = field(default_factory=dict)
    provider_request_body: Any = None
    has_format_conversion: bool = False
    model_group_id: str | None = None
    model_group_route_id: str | None = None
    user_billing_multiplier: float = 1.0
    first_byte_time_ms: int | None = None
    # scheduling 元信息（从 ExecutionResult 回填，供 telemetry request_metadata）
    candidate_keys: list[Any] = field(default_factory=list)
    scheduling_audit: dict[str, Any] | None = None
    pool_summary: dict[str, Any] | None = None


class ImageHandlerBase(BaseMessageHandler, ABC):
    """图像生成处理器基类。

    子类必须定义：
    - ``FORMAT_ID`` / ``API_FAMILY`` / ``ENDPOINT_KIND``
    - ``_build_upstream_url`` —— 透传路径的 URL 构建（Codex 走 transport hook 不经此）
    """

    FORMAT_ID: str = "UNKNOWN"
    API_FAMILY: ClassVar[ApiFamily | None] = None
    ENDPOINT_KIND: ClassVar[EndpointKind] = EndpointKind.IMAGE

    # Codex 上游 /responses 流式读取握手窗口（秒）
    STREAM_HANDSHAKE_TIMEOUT: ClassVar[float] = 30.0
    # Codex 生图总 deadline（秒）—— 防止上游卡住无限吊死。
    # 生图单张一般 15-60s，给到 180s 足够容纳重图 + 网络抖动。
    STREAM_READ_TIMEOUT: ClassVar[float] = 180.0

    def __init__(
        self,
        *,
        db: Any,
        user: Any,
        api_key: Any,
        request_id: str,
        client_ip: str,
        user_agent: str,
        start_time: float,
        allowed_api_formats: list[str] | None = None,
        adapter_detector: Any | None = None,
        perf_metrics: dict[str, Any] | None = None,
        api_family: str | None = None,
        endpoint_kind: str | None = None,
    ) -> None:
        super().__init__(
            db=db,
            user=user,
            api_key=api_key,
            request_id=request_id,
            client_ip=client_ip,
            user_agent=user_agent,
            start_time=start_time,
            allowed_api_formats=allowed_api_formats or [self.FORMAT_ID],
            adapter_detector=adapter_detector,
            perf_metrics=perf_metrics,
            api_family=api_family or (self.API_FAMILY.value if self.API_FAMILY else None),
            endpoint_kind=endpoint_kind or self.ENDPOINT_KIND.value,
        )

    # ------------------------------------------------------------------ #
    # 子类抽象接口
    # ------------------------------------------------------------------ #

    @abstractmethod
    def _build_upstream_url(
        self, base_url: str | None, *, path_suffix: str | None = None
    ) -> str:
        """透传路径的 URL 构建。

        path_suffix: "generations" | "edits"，由 adapter 根据路由传入。
        """

    # ------------------------------------------------------------------ #
    # 公共编排入口
    # ------------------------------------------------------------------ #

    async def process_sync(
        self,
        *,
        http_request: Request,
        original_headers: dict[str, str],
        original_request_body: dict[str, Any],
        query_params: dict[str, str] | None = None,
        multipart_context: dict[str, Any] | None = None,
        raw_body: bytes | None = None,
        raw_content_type: str | None = None,
        upstream_path_suffix: str | None = None,
    ) -> JSONResponse:
        """同步图像生成。"""
        _ = (http_request, query_params)

        ctx, response_body, status_code, _response_headers = (
            await self._execute_image_request(
                original_headers=original_headers,
                original_request_body=original_request_body,
                multipart_context=multipart_context,
                raw_body=raw_body,
                raw_content_type=raw_content_type,
                upstream_path_suffix=upstream_path_suffix,
                client_is_stream=False,
            )
        )

        self._log_ok(ctx, status_code)
        return JSONResponse(response_body, status_code=status_code)

    async def process_stream(
        self,
        *,
        http_request: Request,
        original_headers: dict[str, str],
        original_request_body: dict[str, Any],
        query_params: dict[str, str] | None = None,
        multipart_context: dict[str, Any] | None = None,
        raw_body: bytes | None = None,
        raw_content_type: str | None = None,
        upstream_path_suffix: str | None = None,
    ) -> StreamingResponse | JSONResponse:
        """真流式图像生成：Codex 候选实时翻译 SSE 推给客户端；非 Codex 候选
        退回到聚合+单帧合成（因为 OpenAI /v1/images/generations 原生不 stream）。

        与 sub2api ``handleOpenAIImagesOAuthStreamingResponse`` 行为对齐。
        """
        _ = (http_request, query_params)

        return await self._execute_image_stream_request(
            original_headers=original_headers,
            original_request_body=original_request_body,
            multipart_context=multipart_context,
            raw_body=raw_body,
            raw_content_type=raw_content_type,
            upstream_path_suffix=upstream_path_suffix,
        )

    # ------------------------------------------------------------------ #
    # 统一执行入口：走 TaskService.execute + self.telemetry
    # ------------------------------------------------------------------ #

    async def _execute_image_request(
        self,
        *,
        original_headers: dict[str, str],
        original_request_body: dict[str, Any],
        multipart_context: dict[str, Any] | None,
        raw_body: bytes | None,
        raw_content_type: str | None,
        upstream_path_suffix: str | None,
        client_is_stream: bool,
    ) -> tuple[ImageRequestContext, dict[str, Any], int, dict[str, str]]:
        """运行一次图像请求的完整流程：

        1. 创建 pending Usage；
        2. 构造 request_func，内部按候选类型（Codex / 透传）发 HTTP，捕获 ctx；
        3. 走 ``TaskService.execute`` 让 FailoverEngine 处理候选遍历与错误分类；
        4. 根据成功/失败走 ``self.telemetry`` 写 Usage（带完整归因字段）；
        5. 返回 ``(ctx, response_body, status_code, response_headers)``。
        """
        model = str(original_request_body.get("model") or "unknown")

        pending_created = self._create_pending_usage(
            model=model,
            is_stream=client_is_stream,
            request_type="image",
            api_format=self.FORMAT_ID,
            request_headers=_sanitize_headers(original_headers),
            request_body=original_request_body,
        )

        ctx = ImageRequestContext()
        ctx.mapped_model = None  # 在 request_func 内填入
        response_ref: dict[str, Any] = {
            "body": None,
            "status_code": 200,
            "headers": {},
        }

        async def request_func(
            provider: Any, endpoint: Any, key: Any, candidate: ProviderCandidate
        ) -> Any:
            # 切候选前清空 response_ref，避免上一次候选的残留污染归因
            response_ref["body"] = None
            response_ref["status_code"] = 200
            response_ref["headers"] = {}

            # 一次候选的归因信息先全部记到 ctx，即使后面抛错也能用
            ctx.candidate = candidate
            ctx.provider_name = str(getattr(provider, "name", "") or "") or ctx.provider_name
            ctx.provider_id = str(getattr(provider, "id", "") or "") or ctx.provider_id
            ctx.endpoint_id = str(getattr(endpoint, "id", "") or "") or ctx.endpoint_id
            ctx.key_id = str(getattr(key, "id", "") or "") or ctx.key_id
            fam = str(getattr(endpoint, "api_family", "")).strip().lower()
            kind = str(getattr(endpoint, "endpoint_kind", "")).strip().lower()
            ctx.provider_api_format = make_signature_key(fam, kind) or self.FORMAT_ID
            ctx.model_group_id = getattr(candidate, "model_group_id", None)
            ctx.model_group_route_id = getattr(candidate, "model_group_route_id", None)
            ctx.user_billing_multiplier = float(
                getattr(candidate, "user_billing_multiplier", None) or 1.0
            )
            ctx.has_format_conversion = _is_codex_candidate(candidate)
            ctx.mapped_model = await self._resolve_mapped_model(
                candidate, original_request_body
            )

            try:
                proxy_info = await resolve_proxy_info_async(
                    self._resolve_effective_proxy(candidate)
                )
                logger.debug(
                    "  [{}] 图像请求 provider={} 模型={} -> {} 代理={}",
                    self.request_id,
                    ctx.provider_name or "?",
                    model,
                    ctx.mapped_model or "无映射",
                    get_proxy_label(proxy_info),
                )
            except Exception:
                pass

            # 立刻把 pending 行推进成 streaming + 回填 provider/key/target_model。
            # 对齐 chat 的 _update_usage_to_streaming_with_ctx —— 让管理面"使用记录"页
            # 能实时看到当前哪个 provider 在处理，不再卡在"待分配提供商/等待中"。
            # Codex 生图可能 30-60s，不推进就永远看不到归因。
            self._update_image_usage_to_streaming(ctx)

            return await self._invoke_upstream(
                candidate=candidate,
                original_headers=original_headers,
                original_request_body=original_request_body,
                multipart_context=multipart_context,
                raw_body=raw_body,
                raw_content_type=raw_content_type,
                upstream_path_suffix=upstream_path_suffix,
                ctx=ctx,
                response_ref=response_ref,
            )

        from src.services.task import TaskService
        from src.services.task.core.context import TaskMode

        try:
            exec_result = await TaskService(self.db, self.redis).execute(
                task_type="image",
                task_mode=TaskMode.SYNC,
                api_format=self.FORMAT_ID,
                model_name=model,
                user_api_key=self.api_key,
                request_func=request_func,
                request_id=self.request_id,
                is_stream=False,
                capability_requirements=None,
                request_headers=original_headers,
                request_body=original_request_body,
                supported_auth_types={"api_key", "oauth"},
                allow_format_conversion=False,
                max_candidates=10,
                create_pending_usage=not pending_created,
            )
        except Exception as exc:  # noqa: BLE001 — 需要统一结算失败 Usage
            await self._record_failure(
                ctx=ctx,
                model=model,
                original_headers=original_headers,
                original_request_body=original_request_body,
                exc=exc,
                is_stream=client_is_stream,
            )
            raise self._to_http_exception(exc) from exc

        # ExecutionResult 是最终权威：回填 ctx
        if exec_result.candidate:
            ctx.candidate = exec_result.candidate
        if exec_result.provider_name:
            ctx.provider_name = exec_result.provider_name
        if exec_result.provider_id:
            ctx.provider_id = exec_result.provider_id
        if exec_result.endpoint_id:
            ctx.endpoint_id = exec_result.endpoint_id
        if exec_result.key_id:
            ctx.key_id = exec_result.key_id
        if exec_result.candidate_keys:
            ctx.candidate_keys = list(exec_result.candidate_keys)
        if exec_result.pool_summary:
            ctx.pool_summary = exec_result.pool_summary

        response_body = response_ref.get("body")
        if not isinstance(response_body, dict):
            candidate_body = exec_result.response
            if isinstance(candidate_body, dict):
                response_body = candidate_body
            else:
                # request_func 正常返回但 response_ref 未填（不该发生）——
                # 把 exec_result 里能拿到的任何痕迹吐出来，避免客户端拿到空 {}
                logger.warning(
                    "[ImageHandler] response_ref missing body request_id={} candidate={}",
                    self.request_id,
                    getattr(ctx.candidate, "provider", None)
                    and getattr(ctx.candidate.provider, "name", "?"),
                )
                raise HTTPException(
                    status_code=502,
                    detail="Upstream returned no response body",
                )
        status_code = int(response_ref.get("status_code") or 200)
        response_headers = response_ref.get("headers") or {}

        # Usage 记账走聚合后的完整 body（含 _codex_image_gen 等内部 metadata）
        await self._record_success(
            ctx=ctx,
            model=model,
            original_headers=original_headers,
            original_request_body=original_request_body,
            response_body=response_body,
            response_headers=response_headers,
            status_code=status_code,
            is_stream=client_is_stream,
        )

        # 返回给客户端前过滤 _codex_* 内部元数据（不能泄漏给调用方）
        client_body = _strip_internal_fields(response_body)

        return ctx, client_body, status_code, response_headers

    # ------------------------------------------------------------------ #
    # 真流式入口（client stream=true）
    # ------------------------------------------------------------------ #

    async def _execute_image_stream_request(
        self,
        *,
        original_headers: dict[str, str],
        original_request_body: dict[str, Any],
        multipart_context: dict[str, Any] | None,
        raw_body: bytes | None,
        raw_content_type: str | None,
        upstream_path_suffix: str | None,
    ) -> StreamingResponse | JSONResponse:
        """真流式生图执行链 —— 手动候选迭代 + Codex SSE 实时翻译。

        路由策略：
        - **Codex 候选** → 打开 /responses 流、用 ``_CodexImageStreamTranslator``
          实时翻译 SSE 事件，把 ``image_generation.partial_image`` /
          ``image_generation.completed`` 帧推给客户端
        - **非 Codex 候选** → OpenAI 原生 ``/v1/images/generations`` 没有稳定的
          流式协议（gpt-image-1 公版 stream=true 行为因模型而异），为稳妥退回到
          聚合 JSON 再用 ``_synthesize_stream_response`` 合成单帧
        - **故意不走 TaskService** —— Codex 首字节可能 10-30s（内部路由 +
          partial_image 生成），FailoverEngine 的 first-chunk probe 默认 30s 容易
          误报；sub2api 也是自己迭代候选，不用通用 failover。

        与 ``sub2api`` 的 ``handleOpenAIImagesOAuthStreamingResponse`` 行为严格对齐。
        """
        model = str(original_request_body.get("model") or "unknown")
        pending_created = self._create_pending_usage(
            model=model,
            is_stream=True,
            request_type="image",
            api_format=self.FORMAT_ID,
            request_headers=_sanitize_headers(original_headers),
            request_body=original_request_body,
        )
        _ = pending_created

        candidates = await self._fetch_stream_candidates(
            model=model, request_body=original_request_body
        )
        ctx = ImageRequestContext()

        if not candidates:
            exc = HTTPException(
                status_code=503,
                detail="No available provider for image generation",
            )
            await self._record_failure(
                ctx=ctx,
                model=model,
                original_headers=original_headers,
                original_request_body=original_request_body,
                exc=exc,
                is_stream=True,
            )
            raise exc

        last_exc: Exception | None = None
        for candidate in candidates[:10]:
            # Codex 只支持 OAuth；非 OAuth 的 codex provider 直接跳
            if _is_codex_candidate(candidate):
                auth_type = str(
                    getattr(candidate.key, "auth_type", "") or ""
                ).lower()
                if auth_type != "oauth":
                    last_exc = UpstreamClientException(
                        "Codex provider requires OAuth",
                        provider_name=str(getattr(candidate.provider, "name", "") or ""),
                        status_code=400,
                    )
                    continue

            # 归因信息写入 ctx
            self._populate_ctx_from_candidate(ctx, candidate)
            try:
                ctx.mapped_model = await self._resolve_mapped_model(
                    candidate, original_request_body
                )
            except Exception:  # noqa: BLE001
                ctx.mapped_model = str(
                    original_request_body.get("model") or ""
                ) or None

            is_codex = _is_codex_candidate(candidate)
            ctx.has_format_conversion = is_codex

            try:
                if is_codex:
                    upstream_response = await self._open_codex_upstream_stream(
                        candidate=candidate,
                        original_headers=original_headers,
                        original_request_body=original_request_body,
                        multipart_context=multipart_context,
                        ctx=ctx,
                    )
                else:
                    # 非 Codex 一律走聚合 + 单帧（保守策略）
                    upstream_response = None
            except Exception as exc:  # noqa: BLE001
                last_exc = exc
                logger.warning(
                    "[ImageHandler] stream open failed provider={}: {}",
                    ctx.provider_name or "?",
                    exc,
                )
                continue

            # ---- 非 Codex fallback：直接 sync POST + 聚合 + 单帧 ----
            if not is_codex:
                try:
                    response_ref: dict[str, Any] = {
                        "body": None,
                        "status_code": 200,
                        "headers": {},
                    }
                    body = await self._invoke_passthrough_upstream(
                        candidate=candidate,
                        original_headers=original_headers,
                        original_request_body=original_request_body,
                        raw_body=raw_body,
                        raw_content_type=raw_content_type,
                        upstream_path_suffix=upstream_path_suffix,
                        ctx=ctx,
                        response_ref=response_ref,
                    )
                except Exception as exc:  # noqa: BLE001
                    last_exc = exc
                    continue

                self._update_image_usage_to_streaming(ctx)
                await self._record_success(
                    ctx=ctx,
                    model=model,
                    original_headers=original_headers,
                    original_request_body=original_request_body,
                    response_body=body,
                    response_headers=response_ref.get("headers") or {},
                    status_code=int(response_ref.get("status_code") or 200),
                    is_stream=True,
                )
                self._log_ok(ctx, int(response_ref.get("status_code") or 200))
                return self._synthesize_stream_response(
                    _strip_internal_fields(body),
                    int(response_ref.get("status_code") or 200),
                )

            # ---- Codex 路径：检查上游状态 + 真流式翻译 ----
            assert upstream_response is not None
            if upstream_response.status_code >= 400:
                err_bytes = b""
                try:
                    err_bytes = await upstream_response.aread()
                except Exception:  # noqa: BLE001
                    pass
                await upstream_response.aclose()
                last_exc = self._build_status_error(upstream_response, err_bytes)
                continue

            # 上游 200：commit to this candidate，开始真流式
            self._update_image_usage_to_streaming(ctx)

            operation = "edit" if multipart_context else "generate"
            response_format = str(
                original_request_body.get("response_format") or "b64_json"
            ).strip().lower() or "b64_json"

            self._log_ok(ctx, 200)
            return self._build_codex_streaming_response(
                upstream_response=upstream_response,
                ctx=ctx,
                model=model,
                original_headers=original_headers,
                original_request_body=original_request_body,
                operation=operation,
                response_format=response_format,
            )

        # 所有候选失败
        final_exc = last_exc or HTTPException(
            status_code=502, detail="All candidates failed"
        )
        await self._record_failure(
            ctx=ctx,
            model=model,
            original_headers=original_headers,
            original_request_body=original_request_body,
            exc=final_exc,
            is_stream=True,
        )
        raise self._to_http_exception(final_exc)

    def _build_codex_streaming_response(
        self,
        *,
        upstream_response: httpx.Response,
        ctx: ImageRequestContext,
        model: str,
        original_headers: dict[str, str],
        original_request_body: dict[str, Any],
        operation: str,
        response_format: str,
    ) -> StreamingResponse:
        """把上游 Codex SSE 实时翻译成客户端 SSE，结束后记 Usage。"""
        translator = _CodexImageStreamTranslator(
            operation=operation, response_format=response_format
        )

        async def _pump() -> AsyncIterator[bytes]:
            stream_error: Exception | None = None
            try:
                async for frame in iter_codex_image_stream_frames(
                    upstream_response.aiter_bytes(),
                    translator=translator,
                ):
                    yield frame
            except CodexStreamError as err:
                stream_error = err
                logger.warning(
                    "[ImageHandler] codex stream error request_id={}: {}",
                    self.request_id,
                    err,
                )
                yield build_codex_stream_error_frame(str(err))
            except Exception as err:  # noqa: BLE001
                stream_error = err
                logger.warning(
                    "[ImageHandler] stream interrupted request_id={}: {}",
                    self.request_id,
                    err,
                )
                yield build_codex_stream_error_frame(str(err))
            finally:
                # 所有分支都以 [DONE] 收尾（OpenAI SSE 惯例）
                yield b"data: [DONE]\n\n"
                try:
                    await upstream_response.aclose()
                except Exception:  # noqa: BLE001
                    pass

            # 流结束后记账 —— 此时 client 可能已断开，只影响服务端 Usage 行
            try:
                if stream_error is None and translator.state.images:
                    response_body: dict[str, Any] = {
                        "created": int(translator.state.created_at or 0),
                        "data": list(translator.state.images),
                    }
                    billing_usage, tool_usage_meta = extract_codex_image_usage(
                        translator.state.completed_response
                    )
                    if billing_usage:
                        response_body["usage"] = billing_usage
                    if tool_usage_meta:
                        response_body["_codex_image_gen"] = tool_usage_meta
                    if translator.state.event_count:
                        response_body["_codex_event_count"] = translator.state.event_count
                    await self._record_success(
                        ctx=ctx,
                        model=model,
                        original_headers=original_headers,
                        original_request_body=original_request_body,
                        response_body=response_body,
                        response_headers={},
                        status_code=200,
                        is_stream=True,
                    )
                else:
                    exc = stream_error or HTTPException(
                        status_code=502,
                        detail="Stream ended without images",
                    )
                    await self._record_failure(
                        ctx=ctx,
                        model=model,
                        original_headers=original_headers,
                        original_request_body=original_request_body,
                        exc=exc,
                        is_stream=True,
                    )
            except Exception as rec_err:  # noqa: BLE001
                logger.warning(
                    "[ImageHandler] stream usage record failed request_id={}: {}",
                    self.request_id,
                    rec_err,
                )

        safe_headers: dict[str, str] = {
            k: v
            for k, v in upstream_response.headers.items()
            if k.lower() not in HOP_BY_HOP_HEADERS
        }
        safe_headers["content-type"] = "text/event-stream"

        return StreamingResponse(
            _pump(),
            status_code=upstream_response.status_code or 200,
            headers=safe_headers,
            media_type="text/event-stream",
        )

    async def _fetch_stream_candidates(
        self,
        *,
        model: str,
        request_body: dict[str, Any],
    ) -> list[ProviderCandidate]:
        """手动拉候选列表（stream 入口不走 TaskService）。"""
        from src.services.candidate.resolver import CandidateResolver
        from src.services.scheduling.aware_scheduler import (
            get_cache_aware_scheduler,
        )
        from src.services.user.group_service import UserGroupService

        try:
            scheduling_mode = UserGroupService.resolve_effective_scheduling_mode(
                self.db, self.api_key.user
            )
            scheduler = await get_cache_aware_scheduler(
                self.redis, scheduling_mode=scheduling_mode
            )
            resolver = CandidateResolver(db=self.db, cache_scheduler=scheduler)
            candidates, _ = await resolver.fetch_candidates(
                api_format=self.FORMAT_ID,
                model_name=model,
                affinity_key=str(self.api_key.id),
                user_api_key=self.api_key,
                request_id=self.request_id,
                is_stream=True,
                capability_requirements=None,
                request_body=request_body,
            )
            return list(candidates or [])
        except Exception as exc:  # noqa: BLE001
            logger.warning(
                "[ImageHandler] fetch_stream_candidates failed: {}", exc
            )
            return []

    def _populate_ctx_from_candidate(
        self,
        ctx: ImageRequestContext,
        candidate: ProviderCandidate,
    ) -> None:
        """从候选的 provider/endpoint/key 对象填 ctx 的归因字段。"""
        provider = candidate.provider
        endpoint = candidate.endpoint
        key = candidate.key

        ctx.candidate = candidate
        ctx.provider_name = (
            str(getattr(provider, "name", "") or "") or ctx.provider_name
        )
        ctx.provider_id = (
            str(getattr(provider, "id", "") or "") or ctx.provider_id
        )
        ctx.endpoint_id = (
            str(getattr(endpoint, "id", "") or "") or ctx.endpoint_id
        )
        ctx.key_id = str(getattr(key, "id", "") or "") or ctx.key_id
        fam = str(getattr(endpoint, "api_family", "")).strip().lower()
        kind = str(getattr(endpoint, "endpoint_kind", "")).strip().lower()
        ctx.provider_api_format = make_signature_key(fam, kind) or self.FORMAT_ID
        ctx.model_group_id = getattr(candidate, "model_group_id", None)
        ctx.model_group_route_id = getattr(candidate, "model_group_route_id", None)
        ctx.user_billing_multiplier = float(
            getattr(candidate, "user_billing_multiplier", None) or 1.0
        )

    async def _open_codex_upstream_stream(
        self,
        *,
        candidate: ProviderCandidate,
        original_headers: dict[str, str],
        original_request_body: dict[str, Any],
        multipart_context: dict[str, Any] | None,
        ctx: ImageRequestContext,
    ) -> httpx.Response:
        """只打开 Codex /responses 流式上行 —— 不消费不聚合。

        - OAuth 401 force_refresh 单次重试
        - 返回处于 open 状态的 httpx.Response，调用方负责 aclose()
        - 如果 force_refresh 后仍 401，抛 ``UpstreamClientException``
        - 不做 HTTP 4xx/5xx 判断（调用方用 ``response.status_code`` 自己决定）
        """
        auth_type = str(getattr(candidate.key, "auth_type", "") or "").lower()
        is_oauth = auth_type == "oauth"

        provider_body: dict[str, Any] | None = None
        stream_response: httpx.Response | None = None

        for attempt in range(2 if is_oauth else 1):
            upstream_key, endpoint, provider_key, decrypted_auth_config = (
                await self._resolve_upstream_key(
                    candidate, force_refresh=attempt > 0
                )
            )

            if provider_body is None:
                provider_body = await self._build_codex_request_body(
                    candidate=candidate,
                    original_request_body=original_request_body,
                    multipart_context=multipart_context,
                )
                ctx.provider_request_body = provider_body

            upstream_url = build_provider_url(
                endpoint,
                is_stream=True,
                key=provider_key,
                decrypted_auth_config=decrypted_auth_config,
            )
            headers = self._build_upstream_headers(
                original_headers,
                upstream_key,
                endpoint,
                body=provider_body,
                original_body=original_request_body,
                provider_type="codex",
                decrypted_auth_config=decrypted_auth_config,
            )
            ctx.provider_request_headers = _sanitize_headers(headers)

            client = await self._get_upstream_client(candidate)
            request = client.build_request(
                "POST",
                upstream_url,
                headers=headers,
                json=provider_body,
                timeout=httpx.Timeout(
                    self.STREAM_HANDSHAKE_TIMEOUT,
                    read=self.STREAM_READ_TIMEOUT,
                ),
            )
            stream_response = await client.send(request, stream=True)

            if (
                is_oauth
                and attempt == 0
                and stream_response.status_code == 401
            ):
                await stream_response.aclose()
                logger.warning(
                    "  [{}] Codex image OAuth 401 key_id={}，force_refresh 后重试",
                    self.request_id,
                    getattr(candidate.key, "id", "?"),
                )
                continue

            return stream_response

        # force_refresh 后仍然 401
        raise UpstreamClientException(
            "Codex upstream returned 401 after force_refresh",
            provider_name=ctx.provider_name,
            status_code=401,
        )

    # ------------------------------------------------------------------ #
    # 上游调用（Codex 聚合 / 透传）
    # ------------------------------------------------------------------ #

    async def _invoke_upstream(
        self,
        *,
        candidate: ProviderCandidate,
        original_headers: dict[str, str],
        original_request_body: dict[str, Any],
        multipart_context: dict[str, Any] | None,
        raw_body: bytes | None,
        raw_content_type: str | None,
        upstream_path_suffix: str | None,
        ctx: ImageRequestContext,
        response_ref: dict[str, Any],
    ) -> dict[str, Any]:
        """对一个候选发起上游调用，捕获原始响应填入 ``response_ref``。

        HTTP 4xx/5xx 以 ``httpx.HTTPStatusError`` 抛出，
        让 ErrorClassifier 统一分类（OAuth 401 force_refresh / 限流 / 超时等）。
        Codex 聚合异常包成 ``UpstreamClientException``。
        """
        if _is_codex_candidate(candidate):
            return await self._invoke_codex_upstream(
                candidate=candidate,
                original_headers=original_headers,
                original_request_body=original_request_body,
                multipart_context=multipart_context,
                ctx=ctx,
                response_ref=response_ref,
            )

        return await self._invoke_passthrough_upstream(
            candidate=candidate,
            original_headers=original_headers,
            original_request_body=original_request_body,
            raw_body=raw_body,
            raw_content_type=raw_content_type,
            upstream_path_suffix=upstream_path_suffix,
            ctx=ctx,
            response_ref=response_ref,
        )

    async def _invoke_codex_upstream(
        self,
        *,
        candidate: ProviderCandidate,
        original_headers: dict[str, str],
        original_request_body: dict[str, Any],
        multipart_context: dict[str, Any] | None,
        ctx: ImageRequestContext,
        response_ref: dict[str, Any],
    ) -> dict[str, Any]:
        """Codex 反代：/responses + image_generation tool，内部聚合 SSE。

        - OAuth 401：force_refresh token 后单次重试（同 chat handler 行为）
        - CodexStreamError：上游 SSE 里带错误事件（failed / content_filter 等）
          转为 ``UpstreamClientException`` 进 ErrorClassifier
        """
        auth_type = str(getattr(candidate.key, "auth_type", "") or "").lower()
        is_oauth = auth_type == "oauth"

        provider_body: dict[str, Any] | None = None
        last_exc: Exception | None = None

        # OAuth 路径允许一次 force_refresh 重试；其它 auth_type 只试一次
        for attempt in range(2 if is_oauth else 1):
            upstream_key, endpoint, provider_key, decrypted_auth_config = (
                await self._resolve_upstream_key(candidate, force_refresh=attempt > 0)
            )

            if provider_body is None:
                provider_body = await self._build_codex_request_body(
                    candidate=candidate,
                    original_request_body=original_request_body,
                    multipart_context=multipart_context,
                )
                ctx.provider_request_body = provider_body

            upstream_url = build_provider_url(
                endpoint,
                is_stream=True,
                key=provider_key,
                decrypted_auth_config=decrypted_auth_config,
            )
            headers = self._build_upstream_headers(
                original_headers,
                upstream_key,
                endpoint,
                body=provider_body,
                original_body=original_request_body,
                provider_type="codex",
                decrypted_auth_config=decrypted_auth_config,
            )
            ctx.provider_request_headers = _sanitize_headers(headers)

            client = await self._get_upstream_client(candidate)
            request = client.build_request(
                "POST",
                upstream_url,
                headers=headers,
                json=provider_body,
                # 不再用 read=None —— Codex 生图可能 30-60s 但必须有 deadline。
                # STREAM_READ_TIMEOUT 给 SSE 聚合一个宽限值，避免上游卡住无限吊死。
                timeout=httpx.Timeout(
                    self.STREAM_HANDSHAKE_TIMEOUT,
                    read=self.STREAM_READ_TIMEOUT,
                ),
            )
            stream_response = await client.send(request, stream=True)

            # OAuth 401：force_refresh 一次重试
            if (
                is_oauth
                and attempt == 0
                and stream_response.status_code == 401
            ):
                await stream_response.aclose()
                logger.warning(
                    "  [{}] Codex image OAuth 401 key_id={}，force_refresh 后重试",
                    self.request_id,
                    getattr(candidate.key, "id", "?"),
                )
                continue

            try:
                if stream_response.status_code >= 400:
                    err_bytes = await stream_response.aread()
                    response_ref["status_code"] = stream_response.status_code
                    response_ref["headers"] = dict(stream_response.headers)
                    raise self._build_status_error(stream_response, err_bytes)

                try:
                    aggregated = await self._consume_codex_stream_to_dict(
                        stream_response
                    )
                except CodexStreamError as exc:
                    # 上游吐 response.failed / content_filter / incomplete 等致命事件
                    # → 当成 400/502 让 ErrorClassifier 分类
                    response_ref["status_code"] = 502
                    raise UpstreamClientException(
                        f"Codex stream error: {exc}",
                        provider_name=ctx.provider_name,
                        status_code=502,
                        error_type=exc.event_type,
                        upstream_error=str(exc)[:500],
                    ) from exc
                except HTTPException:
                    raise
                except Exception as exc:
                    raise UpstreamClientException(
                        f"Codex SSE aggregation failed: {exc}",
                        provider_name=ctx.provider_name,
                        status_code=502,
                        upstream_error=str(exc)[:500],
                    ) from exc
            except Exception as exc:
                last_exc = exc
                raise
            finally:
                await stream_response.aclose()

            response_ref["body"] = aggregated
            response_ref["status_code"] = stream_response.status_code or 200
            response_ref["headers"] = {
                k: v
                for k, v in stream_response.headers.items()
                if k.lower() not in HOP_BY_HOP_HEADERS
            }
            return aggregated

        # force_refresh 后仍然 401：抛出最后一次状态供 ErrorClassifier 分类
        if last_exc is not None:
            raise last_exc
        raise UpstreamClientException(
            "Codex upstream returned 401 after force_refresh",
            provider_name=ctx.provider_name,
            status_code=401,
        )

    async def _invoke_passthrough_upstream(
        self,
        *,
        candidate: ProviderCandidate,
        original_headers: dict[str, str],
        original_request_body: dict[str, Any],
        raw_body: bytes | None,
        raw_content_type: str | None,
        upstream_path_suffix: str | None,
        ctx: ImageRequestContext,
        response_ref: dict[str, Any],
    ) -> dict[str, Any]:
        """透传路径：标准 OpenAI /v1/images/{generations,edits}。

        OAuth 401：force_refresh token 后单次重试（与 Codex 分支同策略）。
        """
        auth_type = str(getattr(candidate.key, "auth_type", "") or "").lower()
        is_oauth = auth_type == "oauth"
        provider_type = (
            str(getattr(candidate.provider, "provider_type", "") or "").lower() or None
        )

        response: httpx.Response | None = None
        for attempt in range(2 if is_oauth else 1):
            upstream_key, endpoint, _provider_key, decrypted_auth_config = (
                await self._resolve_upstream_key(candidate, force_refresh=attempt > 0)
            )
            upstream_url = self._build_upstream_url(
                endpoint.base_url, path_suffix=upstream_path_suffix
            )

            if raw_body is not None:
                headers = self._build_upstream_headers_raw(
                    original_headers,
                    upstream_key,
                    endpoint,
                    content_type=raw_content_type,
                    provider_type=provider_type,
                    decrypted_auth_config=decrypted_auth_config,
                )
                ctx.provider_request_headers = _sanitize_headers(headers)
                ctx.provider_request_body = {
                    "__multipart__": True,
                    "size_bytes": len(raw_body),
                }
                client = await self._get_upstream_client(candidate)
                response = await client.post(
                    upstream_url, headers=headers, content=raw_body
                )
            else:
                headers = self._build_upstream_headers(
                    original_headers,
                    upstream_key,
                    endpoint,
                    body=original_request_body,
                    original_body=original_request_body,
                    provider_type=provider_type,
                    decrypted_auth_config=decrypted_auth_config,
                )
                ctx.provider_request_headers = _sanitize_headers(headers)
                ctx.provider_request_body = original_request_body
                client = await self._get_upstream_client(candidate)
                response = await client.post(
                    upstream_url, headers=headers, json=original_request_body
                )

            # OAuth 401: force_refresh retry once
            if is_oauth and attempt == 0 and response.status_code == 401:
                logger.warning(
                    "  [{}] Passthrough image OAuth 401 key_id={}，force_refresh 后重试",
                    self.request_id,
                    getattr(candidate.key, "id", "?"),
                )
                continue
            break

        assert response is not None  # 循环至少跑一次

        response_ref["status_code"] = response.status_code
        response_ref["headers"] = {
            k: v
            for k, v in response.headers.items()
            if k.lower() not in HOP_BY_HOP_HEADERS
        }

        if response.status_code >= 400:
            try:
                err_bytes = response.content
            except Exception:
                err_bytes = b""
            raise self._build_status_error(response, err_bytes)

        try:
            body = response.json()
        except (UnicodeDecodeError, json.JSONDecodeError) as exc:
            raw_text = ""
            try:
                raw_text = (response.text or "")[:500]
            except Exception:
                raw_text = ""
            raise ProviderNotAvailableException(
                "上游服务返回了无效的响应",
                provider_name=ctx.provider_name,
                upstream_status=response.status_code,
                upstream_response=raw_text,
            ) from exc

        if not isinstance(body, dict):
            body = {"data": body}

        response_ref["body"] = body
        return body

    @staticmethod
    def _build_status_error(
        response: httpx.Response, body_bytes: bytes
    ) -> httpx.HTTPStatusError:
        text = ""
        try:
            text = body_bytes.decode("utf-8", errors="ignore")[:4000]
        except Exception:
            text = ""
        request = getattr(response, "request", None) or httpx.Request(
            "POST", str(response.url or "")
        )
        # httpx.Response 只读 stream；重建一个可读 response 供 raise_for_status
        err = httpx.HTTPStatusError(
            f"Upstream returned {response.status_code}",
            request=request,
            response=response,
        )
        err.upstream_response = text  # type: ignore[attr-defined]
        return err

    @staticmethod
    def _to_http_exception(exc: Exception) -> HTTPException:
        if isinstance(exc, HTTPException):
            return exc
        status = int(getattr(exc, "status_code", 0) or 0)
        if not status:
            status = int(getattr(exc, "http_status", 0) or 0)
        if not status:
            status = 503
        detail = str(getattr(exc, "detail", "") or exc)[:500] or "Image generation failed"
        return HTTPException(status_code=status, detail=detail)

    # ------------------------------------------------------------------ #
    # Codex 相关
    # ------------------------------------------------------------------ #

    async def _build_codex_request_body(
        self,
        *,
        candidate: ProviderCandidate,
        original_request_body: dict[str, Any],
        multipart_context: dict[str, Any] | None,
    ) -> dict[str, Any]:
        """调用注册的 body transformer 构造 Codex Responses payload。

        ``mapped_model`` 不再通过 context 传入 —— Codex 反代要求 outer model
        固定为路由模型（``gpt-5.4-mini``），``tool.model`` 直接用 ``body["model"]``
        （用户后台配置的真实图像模型），映射不在此处生效。
        """
        transformer = get_body_transformer("codex", self.FORMAT_ID)
        if transformer is None:
            raise HTTPException(
                status_code=500,
                detail="Codex image body transformer not registered",
            )
        _ = candidate
        operation = "edit" if multipart_context else "generate"
        context = {
            "operation": operation,
            "multipart": multipart_context,
        }
        return transform_request_body(
            provider_type="codex",
            endpoint_sig=self.FORMAT_ID,
            request_body=original_request_body,
            context=context,
        )

    async def _resolve_mapped_model(
        self,
        candidate: ProviderCandidate,
        request_body: dict[str, Any],
    ) -> str:
        """解析候选对应的 mapped_model，fallback 链与 chat 对齐。"""
        mapped = getattr(candidate, "mapping_matched_model", None)
        if isinstance(mapped, str) and mapped.strip():
            return mapped.strip()

        source = str(request_body.get("model") or "").strip()
        provider_id = str(getattr(candidate.provider, "id", "") or "").strip()
        if source and provider_id:
            try:
                from src.services.model.mapper import ModelMapperMiddleware

                mapper = ModelMapperMiddleware(self.db)
                mapping = await mapper.get_mapping(source, provider_id)
                if mapping and getattr(mapping, "model", None) is not None:
                    affinity_key = self.api_key.id if self.api_key else None
                    picked = mapping.model.select_provider_model_name(
                        affinity_key, api_format=self.FORMAT_ID
                    )
                    if isinstance(picked, str) and picked.strip():
                        return picked.strip()
            except Exception as exc:
                logger.warning(
                    "[ImageHandler] model mapping lookup failed for {}: {}",
                    source,
                    exc,
                )

        return source or "unknown"

    async def _consume_codex_stream_to_dict(
        self,
        stream_response: httpx.Response,
    ) -> dict[str, Any]:
        from src.services.provider.adapters.codex.image_transform import (
            aggregate_codex_image_sse,
        )

        return await aggregate_codex_image_sse(stream_response.aiter_bytes())

    def _synthesize_stream_response(
        self,
        aggregated: dict[str, Any],
        upstream_status_code: int,
    ) -> StreamingResponse:
        from src.services.provider.adapters.codex.image_transform import (
            synthesize_stream_completed_frames,
        )

        payload = synthesize_stream_completed_frames(aggregated)

        async def _iter() -> AsyncIterator[bytes]:
            yield payload

        return StreamingResponse(
            _iter(),
            status_code=upstream_status_code if upstream_status_code < 400 else 200,
            media_type="text/event-stream",
            headers={"content-type": "text/event-stream"},
        )

    # ------------------------------------------------------------------ #
    # 代理 / HTTP client
    # ------------------------------------------------------------------ #

    def _resolve_effective_proxy(
        self, candidate: ProviderCandidate
    ) -> dict[str, Any] | None:
        return resolve_effective_proxy(
            getattr(candidate.provider, "proxy", None),
            getattr(candidate.key, "proxy", None),
        )

    async def _get_upstream_client(
        self, candidate: ProviderCandidate
    ) -> httpx.AsyncClient:
        """返回代理感知的上游 HTTP client（与 chat handler 一致语义）。"""
        effective_proxy = self._resolve_effective_proxy(candidate)
        delegate_cfg = await resolve_delegate_config_async(effective_proxy)
        return await HTTPClientPool.get_upstream_client(
            delegate_cfg,
            proxy_config=effective_proxy,
            tls_profile=None,
        )

    # ------------------------------------------------------------------ #
    # 凭证 / 头部构建
    # ------------------------------------------------------------------ #

    async def _resolve_upstream_key(
        self,
        candidate: ProviderCandidate,
        *,
        force_refresh: bool = False,
    ) -> tuple[str, ProviderEndpoint, ProviderAPIKey, dict[str, Any] | None]:
        """解析上游凭证。

        ``force_refresh=True`` 时穿透 Redis OAuth token 缓存，强制从 refresh_token
        换新的 access_token。用于 401 单次重试。
        """
        auth_type = str(getattr(candidate.key, "auth_type", "api_key") or "api_key").lower()

        # Codex 反代只支持 OAuth（需要 chatgpt-account-id）。API key 模式下不会注入
        # 该 header，上游只会一路 401；这里前置校验，让 scheduler 直接切候选并
        # 在日志里留痕，避免烧掉一次候选配额。
        if _is_codex_candidate(candidate) and auth_type != "oauth":
            raise UpstreamClientException(
                "Codex provider requires OAuth key; api_key credentials are not supported",
                provider_name=str(getattr(candidate.provider, "name", "") or ""),
                status_code=400,
                error_type="invalid_auth_type_for_codex",
            )

        if auth_type == "oauth":
            try:
                auth_info = await get_provider_auth(
                    candidate.endpoint, candidate.key, force_refresh=force_refresh
                )
            except Exception as exc:
                logger.error(
                    "[ImageHandler] OAuth token resolve failed key_id={}: {}",
                    candidate.key.id,
                    exc,
                )
                raise HTTPException(
                    status_code=500, detail="Failed to resolve OAuth access token"
                ) from exc
            if auth_info is None or not auth_info.auth_value:
                raise HTTPException(
                    status_code=500, detail="OAuth access token unavailable"
                )
            token = auth_info.auth_value
            if token.lower().startswith("bearer "):
                token = token[7:].strip()
            return (
                token,
                candidate.endpoint,
                candidate.key,
                auth_info.decrypted_auth_config,
            )

        try:
            upstream_key = crypto_service.decrypt(candidate.key.api_key)
        except Exception as exc:
            logger.error(
                "[ImageHandler] Failed to decrypt provider key id={}: {}",
                candidate.key.id,
                exc,
            )
            raise HTTPException(
                status_code=500, detail="Failed to decrypt provider key"
            ) from exc
        return upstream_key, candidate.endpoint, candidate.key, None

    def _build_upstream_headers(
        self,
        original_headers: dict[str, str],
        upstream_key: str,
        endpoint: ProviderEndpoint,
        *,
        body: dict[str, Any] | None = None,
        original_body: dict[str, Any] | None = None,
        provider_type: str | None = None,
        decrypted_auth_config: dict[str, Any] | None = None,
    ) -> dict[str, str]:
        extra_headers = dict(get_extra_headers_from_endpoint(endpoint) or {})
        endpoint_sig = make_signature_key(
            str(getattr(endpoint, "api_family", "")).strip().lower(),
            str(getattr(endpoint, "endpoint_kind", "")).strip().lower(),
        )

        if provider_type:
            try:
                provider_extra = build_upstream_extra_headers(
                    provider_type=provider_type,
                    endpoint_sig=endpoint_sig,
                    request_body=body,
                    original_headers=original_headers,
                    decrypted_auth_config=decrypted_auth_config,
                )
                if provider_extra:
                    extra_headers.update(provider_extra)
            except Exception as exc:
                logger.warning(
                    "[ImageHandler] provider header hook failed pt={} sig={}: {}",
                    provider_type,
                    endpoint_sig,
                    exc,
                )

        headers = build_upstream_headers_for_endpoint(
            original_headers,
            endpoint_sig,
            upstream_key,
            endpoint_headers=extra_headers,
            header_rules=getattr(endpoint, "header_rules", None),
            body=body,
            original_body=original_body,
            condition_evaluator=evaluate_condition,
        )
        headers["content-type"] = "application/json"
        return headers

    def _build_upstream_headers_raw(
        self,
        original_headers: dict[str, str],
        upstream_key: str,
        endpoint: ProviderEndpoint,
        *,
        content_type: str | None,
        provider_type: str | None = None,
        decrypted_auth_config: dict[str, Any] | None = None,
    ) -> dict[str, str]:
        extra_headers = dict(get_extra_headers_from_endpoint(endpoint) or {})
        endpoint_sig = make_signature_key(
            str(getattr(endpoint, "api_family", "")).strip().lower(),
            str(getattr(endpoint, "endpoint_kind", "")).strip().lower(),
        )

        if provider_type:
            try:
                provider_extra = build_upstream_extra_headers(
                    provider_type=provider_type,
                    endpoint_sig=endpoint_sig,
                    request_body=None,
                    original_headers=original_headers,
                    decrypted_auth_config=decrypted_auth_config,
                )
                if provider_extra:
                    extra_headers.update(provider_extra)
            except Exception as exc:
                logger.warning(
                    "[ImageHandler] provider header hook failed pt={} sig={}: {}",
                    provider_type,
                    endpoint_sig,
                    exc,
                )

        headers = build_upstream_headers_for_endpoint(
            original_headers,
            endpoint_sig,
            upstream_key,
            endpoint_headers=extra_headers,
            header_rules=getattr(endpoint, "header_rules", None),
            body=None,
            original_body=None,
            condition_evaluator=evaluate_condition,
        )
        if content_type:
            headers["content-type"] = content_type
        return headers

    # ------------------------------------------------------------------ #
    # Usage 记录
    # ------------------------------------------------------------------ #

    def _update_image_usage_to_streaming(self, ctx: ImageRequestContext) -> None:
        """把 pending 行推进成 ``streaming`` 并回填 provider/key/target_model。

        异步后台执行（仿 :meth:`BaseMessageHandler._update_usage_to_streaming_with_ctx`），
        不阻塞主请求链路。失败只 warn。

        这一步是管理面"使用记录"页实时展示归因的前提 —— 没有它，Codex 生图的
        30-60 秒里 pending 行会一直显示"待分配提供商 / 等待中"。
        """
        import asyncio

        from src.database.database import get_db

        target_request_id = self.request_id
        provider = ctx.provider_name
        provider_id = ctx.provider_id
        endpoint_id = ctx.endpoint_id
        key_id = ctx.key_id
        target_model = ctx.mapped_model
        api_format = self.FORMAT_ID
        endpoint_api_format = ctx.provider_api_format
        has_format_conversion = ctx.has_format_conversion
        provider_request_headers = ctx.provider_request_headers or None
        provider_request_body = ctx.provider_request_body

        if not provider:
            logger.warning(
                "[ImageHandler] 推进 streaming 时 provider 为空: request_id={}",
                target_request_id,
            )
            return

        def _sync_update() -> None:
            db_gen = get_db()
            db = next(db_gen)
            try:
                UsageService.update_usage_status(
                    db=db,
                    request_id=target_request_id,
                    status="streaming",
                    provider=provider,
                    target_model=target_model,
                    provider_id=provider_id,
                    provider_endpoint_id=endpoint_id,
                    provider_api_key_id=key_id,
                    api_format=api_format,
                    endpoint_api_format=endpoint_api_format,
                    has_format_conversion=has_format_conversion,
                    provider_request_headers=provider_request_headers,
                    provider_request_body=provider_request_body,
                )
            finally:
                db.close()

        async def _do_update() -> None:
            try:
                await asyncio.to_thread(_sync_update)
            except Exception as exc:
                logger.warning(
                    "[ImageHandler] 推进 streaming 失败 request_id={}: {}",
                    target_request_id,
                    exc,
                )

        from src.utils.async_utils import safe_create_task

        safe_create_task(_do_update())

    async def _record_success(
        self,
        *,
        ctx: ImageRequestContext,
        model: str,
        original_headers: dict[str, str],
        original_request_body: dict[str, Any],
        response_body: dict[str, Any],
        response_headers: dict[str, str],
        status_code: int,
        is_stream: bool,
    ) -> None:
        response_time_ms = self.elapsed_ms()
        usage = response_body.get("usage") if isinstance(response_body, dict) else None
        input_tokens = 0
        output_tokens = 0
        if isinstance(usage, dict):
            input_tokens = int(usage.get("input_tokens") or usage.get("prompt_tokens") or 0)
            output_tokens = int(
                usage.get("output_tokens") or usage.get("completion_tokens") or 0
            )

        request_metadata = self._build_image_request_metadata(ctx)

        try:
            await self.telemetry.record_success(
                provider=ctx.provider_name or "unknown",
                model=model,
                target_model=ctx.mapped_model,
                input_tokens=input_tokens,
                output_tokens=output_tokens,
                response_time_ms=response_time_ms,
                status_code=status_code,
                request_body=original_request_body,
                request_headers=_sanitize_headers(original_headers),
                response_body=response_body,
                response_headers=response_headers or {},
                provider_request_headers=ctx.provider_request_headers,
                provider_request_body=ctx.provider_request_body,
                is_stream=is_stream,
                provider_id=ctx.provider_id,
                provider_endpoint_id=ctx.endpoint_id,
                provider_api_key_id=ctx.key_id,
                model_group_id=ctx.model_group_id,
                model_group_route_id=ctx.model_group_route_id,
                user_billing_multiplier=ctx.user_billing_multiplier,
                api_format=self.FORMAT_ID,
                api_family=self.api_family,
                endpoint_kind=self.endpoint_kind,
                endpoint_api_format=ctx.provider_api_format,
                has_format_conversion=ctx.has_format_conversion,
                request_metadata=request_metadata,
                request_type="image",
            )
        except Exception as exc:
            logger.warning(
                "[ImageHandler] record_success failed request_id={}: {}",
                self.request_id,
                exc,
            )

    async def _record_failure(
        self,
        *,
        ctx: ImageRequestContext,
        model: str,
        original_headers: dict[str, str],
        original_request_body: dict[str, Any],
        exc: Exception,
        is_stream: bool,
    ) -> None:
        response_time_ms = self.elapsed_ms()
        status_code = self._extract_error_status_code(exc)
        error_message = self._extract_error_message(exc)

        request_metadata = self._build_image_request_metadata(ctx)

        try:
            await self.telemetry.record_failure(
                provider=ctx.provider_name or "unknown",
                model=model,
                target_model=ctx.mapped_model,
                response_time_ms=response_time_ms,
                status_code=status_code,
                error_message=error_message,
                request_body=original_request_body,
                request_headers=_sanitize_headers(original_headers),
                is_stream=is_stream,
                api_format=self.FORMAT_ID,
                api_family=self.api_family,
                endpoint_kind=self.endpoint_kind,
                endpoint_api_format=ctx.provider_api_format,
                has_format_conversion=ctx.has_format_conversion,
                provider_request_headers=ctx.provider_request_headers,
                provider_request_body=ctx.provider_request_body,
                provider_id=ctx.provider_id,
                provider_endpoint_id=ctx.endpoint_id,
                provider_api_key_id=ctx.key_id,
                model_group_id=ctx.model_group_id,
                model_group_route_id=ctx.model_group_route_id,
                user_billing_multiplier=ctx.user_billing_multiplier,
                request_metadata=request_metadata,
                request_type="image",
            )
        except Exception as inner:
            logger.warning(
                "[ImageHandler] record_failure failed request_id={}: {}",
                self.request_id,
                inner,
            )

    def _build_image_request_metadata(
        self, ctx: ImageRequestContext
    ) -> dict[str, Any] | None:
        """把 perf + scheduling 信息打包为 request_metadata（参考 chat）。"""
        meta = dict(self._build_request_metadata() or {})
        if ctx.candidate_keys:
            meta["candidate_keys"] = [
                getattr(ck, "__dict__", {}) if hasattr(ck, "__dict__") else ck
                for ck in ctx.candidate_keys
            ]
        if ctx.scheduling_audit:
            meta["scheduling_audit"] = ctx.scheduling_audit
        if ctx.pool_summary:
            meta["pool_summary"] = ctx.pool_summary
        return meta or None

    @staticmethod
    def _extract_error_status_code(exc: Exception) -> int:
        status = int(getattr(exc, "status_code", 0) or 0)
        if status:
            return status
        status = int(getattr(exc, "http_status", 0) or 0)
        if status:
            return status
        resp = getattr(exc, "response", None)
        if resp is not None:
            status = int(getattr(resp, "status_code", 0) or 0)
            if status:
                return status
        if isinstance(exc, HTTPException):
            return int(exc.status_code or 503)
        return 503

    @staticmethod
    def _extract_error_message(exc: Exception) -> str:
        msg = str(getattr(exc, "detail", "") or "") or str(exc)
        upstream = getattr(exc, "upstream_response", None) or getattr(
            exc, "upstream_error", None
        )
        if upstream:
            msg = f"{msg} | upstream={str(upstream)[:200]}"
        return msg[:500]

    # ------------------------------------------------------------------ #
    # 日志
    # ------------------------------------------------------------------ #

    def _log_ok(self, ctx: ImageRequestContext, status_code: int) -> None:
        model = "?"
        try:
            model = str(ctx.candidate.provider.name) if ctx.candidate else "?"
        except Exception:
            pass
        logger.info(
            "[OK] {} | {} -> {} | provider={} | {}ms | status={}",
            self.request_id[:8],
            ctx.mapped_model or "-",
            ctx.provider_api_format or self.FORMAT_ID,
            ctx.provider_name or "unknown",
            self.elapsed_ms(),
            status_code,
        )
        _ = model
