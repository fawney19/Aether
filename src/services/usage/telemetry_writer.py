"""
Telemetry writer abstraction for stream usage.
"""

from __future__ import annotations

import json
from abc import ABC, abstractmethod
from typing import Any

from src.clients.redis_client import get_usage_queue_redis_client as get_redis_client
from src.config.settings import config
from src.core.logger import logger
from src.services.usage.events import UsageEventType, build_usage_event
from src.services.usage.telemetry import MessageTelemetry


class TelemetryWriter(ABC):
    @abstractmethod
    async def record_success(self, **kwargs: Any) -> None:
        raise NotImplementedError

    @abstractmethod
    async def record_failure(self, **kwargs: Any) -> None:
        raise NotImplementedError

    @abstractmethod
    async def record_cancelled(self, **kwargs: Any) -> None:
        raise NotImplementedError


class DbTelemetryWriter(TelemetryWriter):
    """通过 MessageTelemetry 写入数据库的 Writer"""

    # MessageTelemetry 不支持的参数，需要过滤掉
    # - request_type: MessageTelemetry 内部固定为 "chat"，无需外部传入
    # - metadata: 由本 writer 映射到 request_metadata（用于落库追踪信息）
    _IGNORED_KWARGS = frozenset({"request_type"})

    def __init__(self, telemetry: MessageTelemetry) -> None:
        self._telemetry = telemetry

    def _filter_kwargs(self, kwargs: dict[str, Any]) -> dict[str, Any]:
        """过滤掉 MessageTelemetry 不支持的参数"""
        out = {k: v for k, v in kwargs.items() if k not in self._IGNORED_KWARGS}
        # 兼容 stream 侧传入的 metadata 字段：映射到 MessageTelemetry 的 request_metadata
        if "metadata" in out and "request_metadata" not in out:
            out["request_metadata"] = out.get("metadata")
        out.pop("metadata", None)
        return out

    async def record_success(self, **kwargs: Any) -> None:
        await self._telemetry.record_success(**self._filter_kwargs(kwargs))

    async def record_failure(self, **kwargs: Any) -> None:
        await self._telemetry.record_failure(**self._filter_kwargs(kwargs))

    async def record_cancelled(self, **kwargs: Any) -> None:
        await self._telemetry.record_cancelled(**self._filter_kwargs(kwargs))


class QueueTelemetryWriter(TelemetryWriter):
    def __init__(
        self,
        *,
        request_id: str,
        user_id: str,
        api_key_id: str,
        log_level: str = "basic",
        sensitive_headers: list[str] | None = None,
        max_request_body_size: int = 0,
        max_response_body_size: int = 0,
    ) -> None:
        self.request_id = request_id
        self.user_id = user_id
        self.api_key_id = api_key_id
        self.log_level = (log_level or "basic").strip().lower()
        self._sensitive_headers = sensitive_headers or [
            "authorization",
            "x-api-key",
            "api-key",
            "cookie",
            "set-cookie",
        ]
        self._max_request_body_size = int(max_request_body_size or 0)
        self._max_response_body_size = int(max_response_body_size or 0)

    @property
    def include_headers(self) -> bool:
        return self.log_level in {"headers", "full"}

    @property
    def include_bodies(self) -> bool:
        return self.log_level == "full"

    async def record_success(self, **kwargs: Any) -> None:
        await self._publish_event(UsageEventType.COMPLETED, **kwargs)

    async def record_failure(self, **kwargs: Any) -> None:
        await self._publish_event(UsageEventType.FAILED, **kwargs)

    async def record_cancelled(self, **kwargs: Any) -> None:
        await self._publish_event(UsageEventType.CANCELLED, **kwargs)

    async def _publish_event(self, event_type: UsageEventType, **kwargs: Any) -> None:
        redis_client = await get_redis_client(require_redis=False)
        if not redis_client:
            raise RuntimeError("Redis unavailable for usage queue")

        data = self._build_event_data(**kwargs)
        event = build_usage_event(
            event_type=event_type,
            request_id=self.request_id,
            data=data,
        )
        maxlen = config.usage_queue_stream_maxlen
        try:
            if maxlen > 0:
                await redis_client.xadd(
                    config.usage_queue_stream_key,
                    event.to_stream_fields(),
                    maxlen=maxlen,
                    approximate=True,
                )
            else:
                await redis_client.xadd(config.usage_queue_stream_key, event.to_stream_fields())
        except Exception as exc:
            logger.error("[usage-queue] XADD failed: {}", exc)
            raise

    def _mask_headers(self, headers: Any) -> Any:
        """Mask sensitive headers before putting them into Redis."""
        if not isinstance(headers, dict) or not headers:
            return headers
        sensitive = {h.lower() for h in self._sensitive_headers if isinstance(h, str) and h}
        if not sensitive:
            return headers
        out: dict[str, Any] = {}
        for k, v in headers.items():
            key = str(k)
            if key.lower() in sensitive:
                s = str(v)
                if len(s) > 8:
                    out[key] = s[:4] + "****" + s[-4:]
                else:
                    out[key] = "****"
            else:
                out[key] = v
        return out

    def _truncate_body(self, value: Any, *, max_size: int, is_request: bool) -> Any:
        """Best-effort truncate body based on SystemConfigService max_*_body_size."""
        if value is None:
            return None
        limit = int(max_size or 0)
        if limit <= 0:
            return value

        body_str = json.dumps(value) if isinstance(value, (dict, list)) else str(value)
        if len(body_str) <= limit:
            return value

        # Match SystemConfigService.truncate_body contract.
        if isinstance(value, (dict, list)):
            return {
                "_truncated": True,
                "_original_size": len(body_str),
                "_content": body_str[:limit],
            }
        kind = "request" if is_request else "response"
        return (
            body_str[:limit]
            + f"\n... (truncated {kind} body, original size: {len(body_str)} bytes)"
        )

    def _build_event_data(self, **kwargs: Any) -> dict[str, Any]:
        # 必需字段
        data: dict[str, Any] = {
            "request_id": self.request_id,
            "user_id": self.user_id,
            "api_key_id": self.api_key_id,
        }

        # 可选字段 - 只添加非 None/非默认值，减少 payload 大小
        # 注意：消费者端需要处理缺失字段的默认值
        if kwargs.get("provider"):
            data["provider"] = kwargs["provider"]
        if kwargs.get("model"):
            data["model"] = kwargs["model"]
        if kwargs.get("target_model"):
            data["target_model"] = kwargs["target_model"]

        # Token 计数 - 0 是常见值，但仍需传递
        input_tokens = kwargs.get("input_tokens", 0)
        output_tokens = kwargs.get("output_tokens", 0)
        if input_tokens:
            data["input_tokens"] = input_tokens
        if output_tokens:
            data["output_tokens"] = output_tokens

        # 缓存 token（cache_creation_tokens -> cache_creation_input_tokens 映射）
        cache_creation = kwargs.get("cache_creation_tokens", 0)
        cache_read = kwargs.get("cache_read_tokens", 0)
        if cache_creation:
            data["cache_creation_input_tokens"] = cache_creation
        if cache_read:
            data["cache_read_input_tokens"] = cache_read

        cache_ttl_minutes = kwargs.get("cache_ttl_minutes")
        if cache_ttl_minutes is not None:
            data["cache_ttl_minutes"] = cache_ttl_minutes

        # 时间指标
        if kwargs.get("response_time_ms") is not None:
            data["response_time_ms"] = kwargs["response_time_ms"]
        if kwargs.get("first_byte_time_ms") is not None:
            data["first_byte_time_ms"] = kwargs["first_byte_time_ms"]

        # 状态信息
        status_code = kwargs.get("status_code", 200)
        if status_code != 200:
            data["status_code"] = status_code
        if kwargs.get("error_message"):
            data["error_message"] = kwargs["error_message"]

        # 格式信息
        request_type = kwargs.get("request_type", "chat")
        if request_type != "chat":
            data["request_type"] = request_type
        if kwargs.get("api_format"):
            data["api_format"] = kwargs["api_format"]
        if kwargs.get("api_family"):
            data["api_family"] = kwargs["api_family"]
        if kwargs.get("endpoint_kind"):
            data["endpoint_kind"] = kwargs["endpoint_kind"]
        if kwargs.get("endpoint_api_format"):
            data["endpoint_api_format"] = kwargs["endpoint_api_format"]
        if kwargs.get("has_format_conversion"):
            data["has_format_conversion"] = True

        # 流式标记 - 默认 True，只记录 False
        if not kwargs.get("is_stream", True):
            data["is_stream"] = False

        # Provider 追踪
        if kwargs.get("provider_id"):
            data["provider_id"] = kwargs["provider_id"]
        if kwargs.get("provider_endpoint_id"):
            data["provider_endpoint_id"] = kwargs["provider_endpoint_id"]
        if kwargs.get("provider_api_key_id"):
            data["provider_api_key_id"] = kwargs["provider_api_key_id"]

        # 元数据
        if kwargs.get("metadata"):
            data["metadata"] = kwargs["metadata"]

        # Optional: Headers (masked)
        if self.include_headers:
            for _hdr_key in (
                "request_headers",
                "provider_request_headers",
                "response_headers",
                "client_response_headers",
            ):
                if kwargs.get(_hdr_key) is not None:
                    data[_hdr_key] = self._mask_headers(kwargs[_hdr_key])

        # Optional: Bodies (truncated)
        if self.include_bodies:
            request_body = self._truncate_body(
                kwargs.get("request_body"),
                max_size=self._max_request_body_size,
                is_request=True,
            )
            provider_request_body = self._truncate_body(
                kwargs.get("provider_request_body"),
                max_size=self._max_request_body_size,
                is_request=True,
            )
            response_body = self._truncate_body(
                kwargs.get("response_body"),
                max_size=self._max_response_body_size,
                is_request=False,
            )
            client_response_body = self._truncate_body(
                kwargs.get("client_response_body"),
                max_size=self._max_response_body_size,
                is_request=False,
            )
            if request_body is not None:
                data["request_body"] = request_body
            if provider_request_body is not None:
                data["provider_request_body"] = provider_request_body
            if response_body is not None:
                data["response_body"] = response_body
            if client_response_body is not None:
                data["client_response_body"] = client_response_body

        return data
