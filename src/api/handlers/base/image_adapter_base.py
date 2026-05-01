"""
Image Adapter 基类 — 与 chat / cli adapter 共享 :class:`HandlerAdapterBase`
提供的通用能力（异常转换、endpoint 测试、header/body 规则）。

支持两个端点：
- ``POST /v1/images/generations`` (application/json)
- ``POST /v1/images/edits``       (multipart/form-data)

multipart 路径下 adapter 会保留原始字节用于透传，同时解出结构化字段
（``parse_multipart_image_edit``）让 Codex 分支的 body transformer 使用。

``check_endpoint`` 复用 :func:`run_endpoint_check`（与 chat 一样），
Codex 差异通过覆盖 :meth:`build_endpoint_url` / :meth:`build_request_body`
实现，不再自实现测试流程。
"""

from __future__ import annotations

from typing import Any, ClassVar

from fastapi import HTTPException, Request
from fastapi.responses import JSONResponse, Response, StreamingResponse

from src.api.base.adapter import ApiMode
from src.api.base.context import ApiRequestContext
from src.api.handlers.base.handler_adapter_base import HandlerAdapterBase
from src.api.handlers.base.image_handler_base import ImageHandlerBase
from src.core.api_format import ApiFamily, EndpointKind
from src.core.logger import logger


class ImageAdapterBase(HandlerAdapterBase):
    """图像生成适配器基类。"""

    FORMAT_ID: str = "UNKNOWN"
    HANDLER_CLASS: type[ImageHandlerBase]

    API_FAMILY: ClassVar[ApiFamily | None] = None
    ENDPOINT_KIND: ClassVar[EndpointKind] = EndpointKind.IMAGE

    name: str = "image.base"
    mode = ApiMode.STANDARD
    # /edits 路径是 multipart，不能 eager 反序列化；/generations 是 JSON。
    # 统一由 handle() 内部决定如何读 body。
    eager_request_body = False

    # 指示这是哪个端点：由具体路由适配器覆盖。
    # "generations" | "edits"
    ENDPOINT_OPERATION: ClassVar[str] = "generations"

    def __init__(
        self,
        allowed_api_formats: list[str] | None = None,
        *,
        endpoint_operation: str | None = None,
    ) -> None:
        super().__init__(allowed_api_formats=allowed_api_formats or [self.FORMAT_ID])
        self.endpoint_operation = (
            (endpoint_operation or self.ENDPOINT_OPERATION).strip().lower()
            or "generations"
        )

    # ------------------------------------------------------------------ #
    # 主入口
    # ------------------------------------------------------------------ #

    async def handle(
        self, context: ApiRequestContext
    ) -> Response | StreamingResponse | JSONResponse:
        http_request = context.request
        if context.api_key is None or context.user is None:
            raise HTTPException(status_code=401, detail="Unauthorized")

        if http_request.method.upper() != "POST":
            raise HTTPException(status_code=405, detail="Method Not Allowed")

        content_type = str(
            context.original_headers.get("content-type")
            or context.original_headers.get("Content-Type")
            or ""
        )
        content_type_lower = content_type.lower()
        is_multipart = content_type_lower.startswith("multipart/form-data")

        if self.endpoint_operation == "edits" and not is_multipart:
            return JSONResponse(
                status_code=400,
                content={
                    "error": {
                        "type": "invalid_request_error",
                        "message": "/v1/images/edits requires multipart/form-data body",
                    }
                },
            )

        if is_multipart:
            return await self._handle_multipart(context, http_request, content_type)
        return await self._handle_json(context, http_request)

    async def _handle_json(
        self,
        context: ApiRequestContext,
        http_request: Request,
    ) -> Response | StreamingResponse | JSONResponse:
        original_request_body = await context.ensure_json_body_async()
        if not isinstance(original_request_body, dict):
            return JSONResponse(
                status_code=400,
                content={
                    "error": {
                        "type": "invalid_request_error",
                        "message": "Request body must be a JSON object",
                    }
                },
            )

        missing = [f for f in ("model", "prompt") if not original_request_body.get(f)]
        if missing:
            return JSONResponse(
                status_code=400,
                content={
                    "error": {
                        "type": "invalid_request_error",
                        "message": f"Missing required fields: {', '.join(missing)}",
                    }
                },
            )

        stream = bool(original_request_body.get("stream"))
        model = str(original_request_body.get("model") or "unknown")
        self._record_audit(context, model, stream, original_request_body)

        handler = self._create_handler(context)

        params: dict[str, Any] = {
            "http_request": http_request,
            "original_headers": context.original_headers,
            "original_request_body": original_request_body,
            "query_params": context.query_params,
            "upstream_path_suffix": self.endpoint_operation,
        }
        if stream:
            return await handler.process_stream(**params)
        return await handler.process_sync(**params)

    async def _handle_multipart(
        self,
        context: ApiRequestContext,
        http_request: Request,
        content_type: str,
    ) -> Response | StreamingResponse | JSONResponse:
        raw_body = await http_request.body()
        if not raw_body:
            return JSONResponse(
                status_code=400,
                content={
                    "error": {
                        "type": "invalid_request_error",
                        "message": "Empty multipart body",
                    }
                },
            )

        from src.services.provider.adapters.codex.image_transform import (
            parse_multipart_image_edit,
        )

        parsed = parse_multipart_image_edit(raw_body, content_type) or {}

        if self.endpoint_operation == "edits":
            images = parsed.get("images") or []
            prompt = (
                (parsed.get("prompt") or "").strip()
                if isinstance(parsed.get("prompt"), str)
                else ""
            )
            if not images:
                return JSONResponse(
                    status_code=400,
                    content={
                        "error": {
                            "type": "invalid_request_error",
                            "message": "/v1/images/edits requires at least one image",
                        }
                    },
                )
            if not prompt:
                return JSONResponse(
                    status_code=400,
                    content={
                        "error": {
                            "type": "invalid_request_error",
                            "message": "/v1/images/edits requires prompt",
                        }
                    },
                )

        stream_raw = str(parsed.get("stream") or "").strip().lower()
        stream = stream_raw in {"true", "1", "yes"}

        model = str(parsed.get("model") or "unknown")
        surface_body: dict[str, Any] = {
            "model": model,
            "prompt": parsed.get("prompt") or "",
        }
        for key in ("n", "size", "quality", "response_format", "user", "background"):
            if key in parsed:
                surface_body[key] = parsed[key]

        self._record_audit(context, model, stream, surface_body)

        handler = self._create_handler(context)
        params: dict[str, Any] = {
            "http_request": http_request,
            "original_headers": context.original_headers,
            "original_request_body": surface_body,
            "query_params": context.query_params,
            "multipart_context": parsed,
            "raw_body": raw_body,
            "raw_content_type": content_type,
            "upstream_path_suffix": self.endpoint_operation,
        }
        if stream:
            return await handler.process_stream(**params)
        return await handler.process_sync(**params)

    def _record_audit(
        self,
        context: ApiRequestContext,
        model: str,
        stream: bool,
        body_view: dict[str, Any],
    ) -> None:
        context.add_audit_metadata(
            action=f"{self.FORMAT_ID.lower()}_{self.endpoint_operation}",
            model=model,
            stream=stream,
            image_count=body_view.get("n", 1),
            size=body_view.get("size"),
            quality=body_view.get("quality"),
        )
        balance_remaining = getattr(context, "balance_remaining", None)
        balance_display = (
            "unlimited" if balance_remaining is None else f"${balance_remaining:.2f}"
        )
        logger.info(
            "[REQ] {} | {} | {} | {} | {} | {} | balance:{}",
            context.request_id[:8],
            self.FORMAT_ID,
            getattr(context.api_key, "name", "unknown"),
            self.endpoint_operation,
            model,
            "stream" if stream else "sync",
            balance_display,
        )

    def _create_handler(self, context: ApiRequestContext) -> ImageHandlerBase:
        perf_metrics = None
        try:
            perf_metrics = context.extra.get("perf") if isinstance(context.extra, dict) else None
        except Exception:
            perf_metrics = None
        return self.HANDLER_CLASS(
            db=context.db,
            user=context.user,
            api_key=context.api_key,
            request_id=context.request_id,
            client_ip=context.client_ip,
            user_agent=context.user_agent,
            start_time=context.start_time,
            allowed_api_formats=self.allowed_api_formats,
            perf_metrics=perf_metrics,
            api_family=self.API_FAMILY.value if self.API_FAMILY else None,
            endpoint_kind=self.ENDPOINT_KIND.value,
        )

    # ------------------------------------------------------------------ #
    # Endpoint 测试（管理面“测试模型”）
    # ------------------------------------------------------------------ #
    #
    # 继承 ``HandlerAdapterBase.check_endpoint``（统一走 ``run_endpoint_check`` →
    # 写入 ``Usage request_type="endpoint_test"`` + ``RequestCandidate``）。
    # Codex 差异仅靠覆盖下面两个类方法即可。

    @classmethod
    def build_endpoint_url(
        cls,
        base_url: str,
        request_data: dict[str, Any] | None = None,
        model_name: str | None = None,
        *,
        provider_type: str | None = None,
    ) -> str:
        """Image 测试 URL：Codex 反代 → /responses；其它 → /v1/images/generations。"""
        from src.utils.url_utils import join_url

        _ = (request_data, model_name)
        pt = (provider_type or "").strip().lower()
        if pt == "codex":
            return join_url(base_url, "/responses")
        return join_url(base_url, "/v1/images/generations")

    @classmethod
    def build_request_body(
        cls,
        request_data: dict[str, Any] | None = None,
        *,
        base_url: str | None = None,
        provider_type: str | None = None,
    ) -> dict[str, Any]:
        """构建图像端点的测试 body。

        **Codex 反代协议约束**（与 sub2api 最新实现严格对齐，不要擅改）：
        https://github.com/Wei-Shaw/sub2api/commit/eea6f38881896ed8f78a8b340ee3a5b6223dbe74

        - outer ``model`` = ``OPENAI_IMAGE_ROUTING_MODEL``（Codex 内部路由模型）
        - ``tool.model`` = 管理员配置的真实图像模型
        - ``instructions`` = ``""``；``tool_choice`` = ``{"type":"image_generation"}``
        - 顶层带 ``reasoning`` / ``parallel_tool_calls`` / ``include``
        """
        _ = base_url
        data = dict(request_data or {})
        model = str(data.get("model") or "").strip() or "gpt-image-2"

        pt = (provider_type or "").strip().lower()
        if pt == "codex":
            from src.services.provider.adapters.codex.image_transform import (
                OPENAI_IMAGE_ROUTING_MODEL,
            )

            return {
                "model": OPENAI_IMAGE_ROUTING_MODEL,
                "input": [
                    {
                        "type": "message",
                        "role": "user",
                        "content": [
                            {
                                "type": "input_text",
                                "text": (
                                    "Generate a small test image: a single "
                                    "white dot on black background."
                                ),
                            }
                        ],
                    }
                ],
                "tools": [
                    {
                        "type": "image_generation",
                        "action": "generate",
                        "model": model,
                        "size": "1024x1024",
                        "quality": "low",
                    }
                ],
                "tool_choice": {"type": "image_generation"},
                "instructions": "",
                "stream": True,
                "store": False,
                "reasoning": {"effort": "medium", "summary": "auto"},
                "parallel_tool_calls": True,
                "include": ["reasoning.encrypted_content"],
            }

        prompt = str(data.get("prompt") or "ping").strip() or "ping"
        return {
            "model": model,
            "prompt": prompt,
            "n": 1,
        }


# =========================================================================
# Image Adapter 注册表
# =========================================================================
#
# 与 chat / cli 的 ``_ADAPTER_REGISTRY`` 并列独立；``provider_query._get_adapter_for_format``
# 按 chat→cli→image 顺序 fallback。保留独立注册表以避免 FORMAT_ID 碰撞时
# chat 优先，image/cli 同名错误命中。

_IMAGE_ADAPTER_REGISTRY: dict[str, type[ImageAdapterBase]] = {}
_IMAGE_ADAPTERS_LOADED = False


def register_image_adapter(
    adapter_class: type[ImageAdapterBase],
) -> type[ImageAdapterBase]:
    format_id = getattr(adapter_class, "FORMAT_ID", None)
    if format_id and format_id != "UNKNOWN":
        _IMAGE_ADAPTER_REGISTRY[format_id.upper()] = adapter_class
    return adapter_class


def _ensure_image_adapters_loaded() -> None:
    global _IMAGE_ADAPTERS_LOADED
    if _IMAGE_ADAPTERS_LOADED:
        return
    try:
        from src.api.handlers.openai import image_adapter as _  # noqa: F401
    except ImportError:
        pass
    _IMAGE_ADAPTERS_LOADED = True


def get_image_adapter_class(
    api_format: str,
) -> type[ImageAdapterBase] | None:
    _ensure_image_adapters_loaded()
    return _IMAGE_ADAPTER_REGISTRY.get(api_format.upper()) if api_format else None


__all__ = [
    "ImageAdapterBase",
    "register_image_adapter",
    "get_image_adapter_class",
]
