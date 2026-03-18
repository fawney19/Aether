"""
CLI Handler Mixin Protocol -- Mixin 隐式依赖的编译时契约

各 Mixin (CliStreamMixin, CliSyncMixin, CliRequestMixin, CliMonitorMixin,
CliPrefetchMixin, CliEventMixin) 通过 duck typing 访问宿主类的属性和方法。
本模块将这些隐式依赖显式声明为 Protocol，使 mypy/pyright 能在编辑期捕获
缺失属性或类型不匹配的错误。

渐进式采用：仅在各 Mixin 的公开方法签名中标注 `self: CliHandlerProtocol`，
不修改方法体或私有 helper。
"""

from __future__ import annotations

from typing import (
    TYPE_CHECKING,
    Any,
    Protocol,
    runtime_checkable,
)

if TYPE_CHECKING:
    from redis import Redis
    from sqlalchemy.orm import Session

    from src.api.handlers.base.base_handler import MessageTelemetry
    from src.api.handlers.base.cli_request_mixin import CliUpstreamRequestResult
    from src.api.handlers.base.request_builder import RequestBuilder
    from src.api.handlers.base.response_parser import ResponseParser
    from src.api.handlers.base.stream_context import StreamContext
    from src.models.database import ApiKey, User


@runtime_checkable
class CliHandlerProtocol(Protocol):
    """CLI Handler Mixin 宿主需要满足的属性/方法契约。

    声明范围仅覆盖 Mixin 实际引用的 self.xxx，不要求宿主实现全部
    BaseMessageHandler 接口。
    """

    # ------------------------------------------------------------------
    # 实例属性 -- 来自 BaseMessageHandler.__init__
    # ------------------------------------------------------------------
    db: Session
    user: User
    api_key: ApiKey
    request_id: str
    client_ip: str
    user_agent: str
    start_time: float
    allowed_api_formats: list[str]
    primary_api_format: str
    redis: Redis  # type: ignore[type-arg]
    telemetry: MessageTelemetry
    perf_metrics: dict[str, Any] | None

    # ------------------------------------------------------------------
    # 类属性 -- 来自 CliMessageHandlerBase
    # ------------------------------------------------------------------
    FORMAT_ID: str
    DATA_TIMEOUT: int
    EMPTY_CHUNK_THRESHOLD: int

    # ------------------------------------------------------------------
    # 属性/方法 -- 来自 CliMessageHandlerBase / BaseMessageHandler
    # ------------------------------------------------------------------
    @property
    def parser(self) -> ResponseParser: ...

    _request_builder: RequestBuilder

    # ------------------------------------------------------------------
    # 方法 -- 来自 BaseMessageHandler (被多个 Mixin 引用)
    # ------------------------------------------------------------------
    def _create_pending_usage(
        self,
        model: str,
        is_stream: bool,
        request_type: str = ...,
        api_format: str | None = ...,
        request_headers: dict[str, Any] | None = ...,
        request_body: dict[str, Any] | None = ...,
    ) -> bool: ...

    def _build_request_metadata(
        self,
        http_request: Any | None = ...,
    ) -> dict[str, Any] | None: ...

    def _merge_scheduling_metadata(
        self,
        request_metadata: dict[str, Any] | None,
        *,
        exec_result: Any | None = ...,
        selected_key_id: str | None = ...,
        candidate_keys: list[Any] | None = ...,
        pool_summary: dict[str, Any] | None = ...,
        fallback_from_request: bool = ...,
    ) -> dict[str, Any] | None: ...

    def _resolve_capability_requirements(
        self,
        model_name: str,
        request_headers: dict[str, str] | None = ...,
        request_body: dict[str, Any] | None = ...,
    ) -> dict[str, bool]: ...

    async def _resolve_preferred_key_ids(
        self,
        model_name: str,
        request_body: dict[str, Any] | None = ...,
    ) -> list[str] | None: ...

    def _update_usage_to_streaming(
        self,
        request_id: str | None = ...,
    ) -> None: ...

    def _update_usage_to_streaming_with_ctx(
        self,
        ctx: StreamContext,
    ) -> None: ...

    def _log_request_error(
        self,
        message: str,
        error: Exception,
    ) -> None: ...

    # ------------------------------------------------------------------
    # 方法 -- 来自 CliRequestMixin (被 CliStreamMixin / CliSyncMixin 引用)
    # ------------------------------------------------------------------
    def extract_model_from_request(
        self,
        request_body: dict[str, Any],
        path_params: dict[str, Any] | None = ...,
    ) -> str: ...

    async def _get_mapped_model(
        self,
        source_model: str,
        provider_id: str,
    ) -> str | None: ...

    def apply_mapped_model(
        self,
        request_body: dict[str, Any],
        mapped_model: str,
    ) -> dict[str, Any]: ...

    def prepare_provider_request_body(
        self,
        request_body: dict[str, Any],
    ) -> dict[str, Any]: ...

    def finalize_provider_request(
        self,
        request_body: dict[str, Any],
        *,
        mapped_model: str | None,
        provider_api_format: str | None,
    ) -> dict[str, Any]: ...

    def get_model_for_url(
        self,
        request_body: dict[str, Any],
        mapped_model: str | None,
    ) -> str | None: ...

    async def _convert_request_for_cross_format(
        self,
        request_body: dict[str, Any],
        client_api_format: str,
        provider_api_format: str,
        mapped_model: str | None,
        fallback_model: str,
        is_stream: bool,
        *,
        target_variant: str | None = ...,
        output_limit: int | None = ...,
    ) -> tuple[dict[str, Any], str]: ...

    async def _build_upstream_request(
        self,
        *,
        provider: Any,
        endpoint: Any,
        key: Any,
        request_body: dict[str, Any],
        original_headers: dict[str, str],
        query_params: dict[str, str] | None,
        client_api_format: str,
        provider_api_format: str,
        fallback_model: str,
        mapped_model: str | None,
        client_is_stream: bool,
        needs_conversion: bool = ...,
        output_limit: int | None = ...,
    ) -> CliUpstreamRequestResult: ...

    def _extract_response_metadata(
        self,
        response: dict[str, Any],
    ) -> dict[str, Any]: ...

    # ------------------------------------------------------------------
    # 方法 -- 来自 CliEventMixin (被 CliStreamMixin / CliPrefetchMixin 引用)
    # ------------------------------------------------------------------
    def _handle_sse_event(
        self,
        ctx: StreamContext,
        event_name: str | None,
        data_str: str,
        record_chunk: bool = ...,
    ) -> None: ...

    def _mark_first_output(
        self,
        ctx: StreamContext,
        state: dict[str, bool],
    ) -> None: ...

    def _convert_sse_line(
        self,
        ctx: StreamContext,
        line: str,
        events: list[Any],
    ) -> tuple[list[str], list[dict[str, Any]]]: ...

    def _record_converted_chunks(
        self,
        ctx: StreamContext,
        converted_events: list[dict[str, Any]],
    ) -> None: ...

    def _flush_buffer_with_conversion(
        self,
        ctx: StreamContext,
        buffer: bytes,
        decoder: Any,
        sse_parser: Any,
        needs_conversion: bool,
    ) -> Any: ...  # Iterator[bytes]

    def _finalize_stream_metadata(
        self,
        ctx: StreamContext,
    ) -> None: ...

    # ------------------------------------------------------------------
    # 方法 -- 来自 CliPrefetchMixin (被 CliStreamMixin 引用)
    # ------------------------------------------------------------------
    def _flush_remaining_sse_data(
        self,
        ctx: StreamContext,
        buffer: bytes,
        decoder: Any,
        sse_parser: Any,
        *,
        record_chunk: bool = ...,
    ) -> None: ...

    def _estimate_tokens_for_incomplete_stream(
        self,
        ctx: StreamContext,
        request_body: dict[str, Any],
    ) -> None: ...

    # ------------------------------------------------------------------
    # 方法 -- 来自 CliMonitorMixin (被 CliStreamMixin 引用)
    # ------------------------------------------------------------------
    async def _create_monitored_stream(
        self,
        ctx: StreamContext,
        stream_generator: Any,
        http_request: Any | None = ...,
    ) -> Any: ...

    async def _record_stream_stats(
        self,
        ctx: StreamContext,
        original_headers: dict[str, str],
        original_request_body: dict[str, Any],
    ) -> None: ...

    async def _record_stream_failure(
        self,
        ctx: StreamContext,
        error: Exception,
        original_headers: dict[str, str],
        original_request_body: dict[str, Any],
    ) -> None: ...
