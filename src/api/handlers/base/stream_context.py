"""
流式处理上下文 - 类型安全的数据类替代 dict

提供流式请求处理过程中的状态跟踪，包括：
- Provider/Endpoint/Key 信息
- Token 统计
- 响应状态
- 请求/响应数据
"""

from __future__ import annotations

import json
import time
from contextlib import contextmanager
from dataclasses import dataclass, field
from typing import TYPE_CHECKING, Any, Iterator

if TYPE_CHECKING:
    from src.core.api_format.conversion.stream_state import StreamState


def extract_proxy_timing(proxy_info: dict[str, Any] | None, headers: dict[str, str]) -> None:
    """从响应头中提取代理分阶段耗时（X-Proxy-Timing）写入 proxy_info"""
    if proxy_info is None:
        return
    timing_raw = headers.get("x-proxy-timing")
    if not timing_raw:
        return
    try:
        timing = json.loads(timing_raw)
        if isinstance(timing, dict):
            proxy_info["timing"] = timing
    except (json.JSONDecodeError, TypeError):
        pass


def is_format_converted(
    provider_api_format: str | None,
    client_api_format: str | None,
) -> bool:
    """client 与 provider 的 api_format 是否真正不同（用于 usage 展示层）"""
    return bool(
        provider_api_format
        and client_api_format
        and provider_api_format.strip().lower() != client_api_format.strip().lower()
    )


_MAX_COLLECTED_TEXT_CHARS = 16 * 1024


@dataclass
class RecordedStreamBodies:
    """统一封装 telemetry/usage 使用的流式响应体引用。"""

    response_body: dict[str, Any] | None
    client_response_body: dict[str, Any] | None

    def ensure_populated(self, ctx: StreamContext, response_time_ms: int) -> None:
        """在 fallback 到需要 body 的路径时按需补建响应体。"""
        if self.response_body is None:
            self.response_body = ctx.build_response_body(response_time_ms)
        if self.client_response_body is None:
            self.client_response_body = ctx.build_client_response_body(response_time_ms)


@dataclass
class StreamContext:
    """
    流式处理上下文

    用于在流式请求处理过程中跟踪状态，替代原有的 ctx dict。
    所有字段都有类型注解，提供更好的 IDE 支持和运行时类型安全。
    """

    # 请求基本信息
    model: str
    api_format: str
    api_family: str | None = None  # 协议族（从 Adapter 层透传）
    endpoint_kind: str | None = None  # 端点类型（从 Adapter 层透传）

    # 请求标识信息（CLI handler 需要）
    request_id: str = ""
    user_id: int = 0
    api_key_id: int = 0

    # Provider 信息（在请求执行时填充）
    provider_name: str | None = None
    provider_id: str | None = None
    provider_type: str | None = None  # Provider 类型（如 codex），用于元数据采集
    # Transport 层选中的 base_url（用于 URL 可用性更新/故障转移等场景）
    selected_base_url: str | None = None
    endpoint_id: str | None = None
    key_id: str | None = None
    attempt_id: str | None = None
    attempt_synced: bool = False
    provider_api_format: str | None = None  # Provider 的响应格式

    # 模型映射
    mapped_model: str | None = None

    # Token 统计
    input_tokens: int = 0
    output_tokens: int = 0
    cached_tokens: int = 0
    cache_creation_tokens: int = 0
    cache_creation_tokens_5m: int = 0  # 5min TTL 缓存创建
    cache_creation_tokens_1h: int = 0  # 1h TTL 缓存创建

    # 响应内容
    _collected_text_parts: list[str] = field(default_factory=list, repr=False)
    _collected_text_chars: int = field(default=0, repr=False)
    _stored_collected_text_chars: int = field(default=0, repr=False)
    response_id: str | None = None
    final_usage: dict[str, Any] | None = None
    final_response: dict[str, Any] | None = None

    # 时间指标
    first_byte_time_ms: int | None = None  # 首字时间 (TTFB - Time To First Byte)
    start_time: float = field(default_factory=time.time)

    # 响应状态
    status_code: int = 200
    error_message: str | None = None  # 客户端友好的错误消息
    upstream_response: str | None = None  # 原始 Provider 响应（用于请求链路追踪）
    has_completion: bool = False

    # 请求/响应数据
    response_headers: dict[str, str] = field(default_factory=dict)  # 提供商响应头
    client_response_headers: dict[str, str] = field(default_factory=dict)  # 返回给客户端的响应头
    provider_request_headers: dict[str, str] = field(default_factory=dict)
    provider_request_body: dict[str, Any] | None = None

    # 格式转换信息（CLI handler 需要）
    client_api_format: str = ""
    needs_conversion: bool = False  # 是否需要跨格式转换（由 handler 层设置）

    # Provider 响应元数据（CLI handler 需要）
    response_metadata: dict[str, Any] = field(default_factory=dict)

    # 整流标记（Thinking Rectifier）
    rectified: bool = False  # 请求是否经过整流（移除 thinking 块后重试）

    # 流式处理统计
    data_count: int = 0
    chunk_count: int = 0
    parsed_chunks: list[dict[str, Any]] = field(default_factory=list)
    # 格式转换时保留提供商原始 chunks（转换前的数据）
    provider_parsed_chunks: list[dict[str, Any]] = field(default_factory=list)
    # 是否记录 parsed_chunks（可用于降低高并发/长流式响应的内存占用）
    record_parsed_chunks: bool = True

    # 性能采集（可选）
    perf_sampled: bool = False
    perf_metrics: dict[str, Any] = field(default_factory=dict)

    # 代理信息（用于 usage 记录和日志，含 ttfb_ms）
    proxy_info: dict[str, Any] | None = None

    # 号池调度摘要（来自 ExecutionResult.pool_summary）
    pool_summary: dict[str, Any] | None = None
    # 候选轨迹（来自 ExecutionResult.candidate_keys，写入 usage metadata）
    candidate_keys: list[dict[str, Any]] = field(default_factory=list)
    # 内部调度审计摘要（重试/故障转移/账号使用轨迹）
    scheduling_audit: dict[str, Any] | None = None

    # 流式格式转换状态（跨 chunk 追踪）
    stream_conversion_state: StreamState | None = None
    stream_conversion_event_count: int = 0  # 流式转换成功的 event 计数

    def reset_for_retry(self) -> None:
        """
        重试时重置状态

        在故障转移重试时调用，清除之前的数据避免累积。
        保留 model 和 api_format，重置其他所有状态。
        """
        self.release_recorded_chunks()
        self.chunk_count = 0
        self.data_count = 0
        self.has_completion = False
        self._collected_text_parts = []
        self._collected_text_chars = 0
        self._stored_collected_text_chars = 0
        self.input_tokens = 0
        self.output_tokens = 0
        self.cached_tokens = 0
        self.cache_creation_tokens = 0
        self.cache_creation_tokens_5m = 0
        self.cache_creation_tokens_1h = 0
        self.error_message = None
        self.upstream_response = None
        self.status_code = 200
        self.first_byte_time_ms = None
        self.response_headers = {}
        self.client_response_headers = {}
        self.provider_request_headers = {}
        self.provider_request_body = None
        self.response_id = None
        self.final_usage = None
        self.final_response = None
        self.proxy_info = None
        self.pool_summary = None
        self.candidate_keys = []
        self.scheduling_audit = None
        self.stream_conversion_state = None
        self.stream_conversion_event_count = 0
        self.needs_conversion = False
        self.selected_base_url = None

    def release_recorded_chunks(self) -> None:
        """释放 telemetry/usage 已消费完的 chunk 列表，避免后台任务继续持有大对象。"""
        self.parsed_chunks = []
        self.provider_parsed_chunks = []

    @contextmanager
    def managed_recorded_bodies(
        self,
        response_time_ms: int,
        *,
        include_bodies: bool = True,
    ) -> Iterator[RecordedStreamBodies]:
        """统一管理响应体构建与 chunk 释放，避免 telemetry 路径重复写 finally。"""
        recorded_bodies = RecordedStreamBodies(
            response_body=self.build_response_body(response_time_ms) if include_bodies else None,
            client_response_body=(
                self.build_client_response_body(response_time_ms) if include_bodies else None
            ),
        )
        try:
            yield recorded_bodies
        finally:
            self.release_recorded_chunks()
            recorded_bodies.response_body = None
            recorded_bodies.client_response_body = None

    @property
    def collected_text(self) -> str:
        """已收集的文本内容（按需拼接，避免在流式过程中频繁做字符串拷贝）"""
        return "".join(self._collected_text_parts)

    @property
    def collected_text_length(self) -> int:
        """已收集文本的总字符数（包含未保留到内存的截断部分）"""
        return self._collected_text_chars

    def append_text(self, text: str) -> None:
        """追加文本内容（仅保留有限前缀，避免长流导致内存增长）"""
        if not text:
            return

        text_len = len(text)
        self._collected_text_chars += text_len

        remaining = _MAX_COLLECTED_TEXT_CHARS - self._stored_collected_text_chars
        if remaining <= 0:
            return

        if text_len <= remaining:
            self._collected_text_parts.append(text)
            self._stored_collected_text_chars += text_len
            return

        self._collected_text_parts.append(text[:remaining])
        self._stored_collected_text_chars += remaining

    def update_provider_info(
        self,
        provider_name: str,
        provider_id: str,
        endpoint_id: str,
        key_id: str,
        provider_api_format: str | None = None,
    ) -> None:
        """更新 Provider 信息"""
        self.provider_name = provider_name
        self.provider_id = provider_id
        self.endpoint_id = endpoint_id
        self.key_id = key_id
        self.provider_api_format = provider_api_format

    def update_usage(
        self,
        input_tokens: int | None = None,
        output_tokens: int | None = None,
        cached_tokens: int | None = None,
        cache_creation_tokens: int | None = None,
    ) -> None:
        """
        更新 Token 使用统计

        采用防御性更新策略：只有当新值 > 0 或当前值为 0 时才更新，避免用 0 覆盖已有的正确值。

        设计原理：
        - 在流式响应中，某些事件可能不包含完整的 usage 信息（字段为 0 或不存在）
        - 后续事件可能会提供完整的统计数据
        - 通过这种策略，确保一旦获得非零值就保留它，不会被后续的 0 值覆盖

        示例场景：
        - message_start 事件：input_tokens=100, output_tokens=0
        - message_delta 事件：input_tokens=0, output_tokens=50
        - 最终结果：input_tokens=100, output_tokens=50

        注意事项：
        - 此策略假设初始值为 0 是正确的默认状态
        - 如果需要将已有值重置为 0，请直接修改实例属性（不使用此方法）

        Args:
            input_tokens: 输入 tokens 数量
            output_tokens: 输出 tokens 数量
            cached_tokens: 缓存命中 tokens 数量
            cache_creation_tokens: 缓存创建 tokens 数量
        """
        if input_tokens is not None and (input_tokens > 0 or self.input_tokens == 0):
            self.input_tokens = input_tokens
        if output_tokens is not None and (output_tokens > 0 or self.output_tokens == 0):
            self.output_tokens = output_tokens
        if cached_tokens is not None and (cached_tokens > 0 or self.cached_tokens == 0):
            self.cached_tokens = cached_tokens
        if cache_creation_tokens is not None and (
            cache_creation_tokens > 0 or self.cache_creation_tokens == 0
        ):
            self.cache_creation_tokens = cache_creation_tokens

    def mark_failed(
        self,
        status_code: int,
        error_message: str,
        upstream_response: str | None = None,
    ) -> None:
        """
        标记请求失败

        Args:
            status_code: HTTP 状态码
            error_message: 客户端友好的错误消息
            upstream_response: 原始 Provider 响应（用于请求链路追踪）
        """
        self.status_code = status_code
        self.error_message = error_message
        if upstream_response:
            self.upstream_response = upstream_response

    def record_first_byte_time(self, start_time: float) -> None:
        """
        记录首字时间 (TTFB - Time To First Byte)

        应在第一次向客户端发送数据时调用。
        如果已记录过,则不会覆盖(避免重试时重复记录)。

        Args:
            start_time: 请求开始时间 (time.time())
        """
        if self.first_byte_time_ms is None:
            self.first_byte_time_ms = int((time.time() - start_time) * 1000)

    @property
    def has_format_conversion(self) -> bool:
        """是否发生了真正的格式转换（client 和 provider 的 api_format 不同）

        区别于 needs_conversion：后者包含 envelope rewrite（如 Antigravity v1internal），
        不代表客户端与上游的数据格式真正不同。此属性用于 usage 展示层。
        """
        return is_format_converted(self.provider_api_format, self.client_api_format)

    def is_success(self) -> bool:
        """检查请求是否成功"""
        return self.status_code < 400

    def is_client_disconnected(self) -> bool:
        """检查是否因客户端断开连接而结束"""
        return self.status_code == 499

    def has_partial_response(self) -> bool:
        """是否已收到部分流式响应数据。"""
        return self.data_count > 0 or self.chunk_count > 0 or self.collected_text_length > 0

    def ensure_estimated_output_tokens(self) -> bool:
        """在缺少 usage 时，基于已收集文本补充输出 tokens。"""
        if self.output_tokens > 0 or self.collected_text_length <= 0:
            return False
        self.output_tokens = max(1, self.collected_text_length // 4)
        return True

    def should_estimate_incomplete_tokens(self) -> bool:
        """流异常结束且尚无 usage 时，是否应做兜底 token 估算。

        使用 or 而非 and：CancelledError 路径中 ensure_estimated_output_tokens
        可能已补了 output_tokens，但 input_tokens 仍为 0，此时仍需估算。
        """
        return (
            not self.has_completion
            and (self.input_tokens == 0 or self.output_tokens == 0)
            and self.has_partial_response()
        )

    def set_ttfb_ms(self, ms: int) -> None:
        """将首字节响应耗时（TTFB）注入到 proxy_info 中"""
        if self.proxy_info is not None:
            self.proxy_info["ttfb_ms"] = ms

    def set_proxy_timing(self, headers: dict[str, str]) -> None:
        """从代理响应头中提取分阶段耗时信息（X-Proxy-Timing）"""
        extract_proxy_timing(self.proxy_info, headers)

    def build_response_body(self, response_time_ms: int) -> dict[str, Any]:
        """
        构建响应体元数据

        用于记录到 Usage 表的 response_body 字段。
        当有格式转换时，返回提供商原始 chunks；否则返回 parsed_chunks。
        """
        chunks = self.provider_parsed_chunks if self.provider_parsed_chunks else self.parsed_chunks
        return {
            "chunks": chunks,
            "metadata": {
                "stream": True,
                "total_chunks": len(chunks),
                "data_count": self.data_count,
                "has_completion": self.has_completion,
                "response_time_ms": response_time_ms,
            },
        }

    def build_client_response_body(self, response_time_ms: int) -> dict[str, Any] | None:
        """
        构建客户端侧响应体元数据

        仅当有格式转换时返回（parsed_chunks 是转换后的客户端格式）；
        无格式转换时返回 None（此时 parsed_chunks 已在 response_body 中）。
        """
        if not self.provider_parsed_chunks:
            return None
        return {
            "chunks": self.parsed_chunks,
            "metadata": {
                "stream": True,
                "total_chunks": len(self.parsed_chunks),
                "data_count": self.data_count,
                "has_completion": self.has_completion,
                "response_time_ms": response_time_ms,
            },
        }

    def get_log_summary(self, request_id: str, response_time_ms: int) -> str:
        """
        获取日志摘要

        用于请求完成/失败时的日志输出。
        包含首字时间 (TTFB) 和总响应时间,分两行显示。
        """
        if self.is_success():
            status = "OK"
        elif self.is_client_disconnected():
            status = "CANCEL"
        else:
            status = "FAIL"

        # 第一行:基本信息 + 首字时间
        line1 = (
            f"[{status}] {request_id[:8]} | {self.model} | " f"{self.provider_name or 'unknown'}"
        )
        if self.first_byte_time_ms is not None:
            line1 += f" | TTFB: {self.first_byte_time_ms}ms"

        # 第二行:总响应时间 + tokens
        line2 = (
            f"      Total: {response_time_ms}ms | "
            f"in:{self.input_tokens} out:{self.output_tokens}"
        )

        return f"{line1}\n{line2}"
