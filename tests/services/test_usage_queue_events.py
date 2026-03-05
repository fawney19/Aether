import asyncio
import json
from typing import Any
from unittest.mock import AsyncMock, MagicMock, patch

import pytest
from redis.exceptions import ResponseError

from src.config.settings import config
from src.services.usage.consumer_streams import (
    UsageQueueConsumer,
    _consumer_name,
    ensure_usage_stream_group,
)
from src.services.usage.events import (
    UsageEvent,
    UsageEventType,
    build_usage_event,
    sanitize_payload,
)
from src.services.usage.telemetry_writer import (
    DbTelemetryWriter,
    QueueTelemetryWriter,
)


class DummyRedis:
    def __init__(self) -> None:
        self.calls: list[tuple[str, dict[str, str], int | None, bool | None]] = []
        self.xadd_error: Exception | None = None

    async def xadd(
        self,
        key: str,
        fields: dict[str, str],
        maxlen: int | None = None,
        approximate: bool | None = None,
    ) -> str:
        if self.xadd_error:
            raise self.xadd_error
        self.calls.append((key, fields, maxlen, approximate))
        return "1-0"


@pytest.mark.asyncio
async def test_usage_event_roundtrip() -> None:
    event = build_usage_event(
        event_type=UsageEventType.COMPLETED,
        request_id="req-1",
        data={"foo": "bar"},
        timestamp_ms=123,
    )
    fields = event.to_stream_fields()
    restored = UsageEvent.from_stream_fields(fields)
    assert restored.event_type == UsageEventType.COMPLETED
    assert restored.request_id == "req-1"
    assert restored.timestamp_ms == 123
    assert restored.data["foo"] == "bar"


@pytest.mark.asyncio
async def test_queue_writer_publishes_event(monkeypatch: Any) -> None:
    dummy = DummyRedis()

    async def _get_redis_client(require_redis: bool = False) -> Any:
        return dummy

    monkeypatch.setattr("src.services.usage.telemetry_writer.get_redis_client", _get_redis_client)

    old_stream_key = config.usage_queue_stream_key
    old_maxlen = config.usage_queue_stream_maxlen
    try:
        config.usage_queue_stream_key = "usage:events:test"
        config.usage_queue_stream_maxlen = 0

        writer = QueueTelemetryWriter(
            request_id="req-2",
            user_id="user-1",
            api_key_id="key-1",
            log_level="basic",
        )
        await writer.record_success(
            provider="test",
            model="model",
            input_tokens=1,
            output_tokens=2,
            response_time_ms=10,
            status_code=200,
        )
    finally:
        config.usage_queue_stream_key = old_stream_key
        config.usage_queue_stream_maxlen = old_maxlen

    assert dummy.calls
    key, fields, _, _ = dummy.calls[0]
    assert key == "usage:events:test"
    event = UsageEvent.from_stream_fields(fields)
    assert event.data["user_id"] == "user-1"
    assert event.data["api_key_id"] == "key-1"


# ============ events.py 测试 ============


@pytest.mark.asyncio
async def test_usage_event_all_types() -> None:
    """测试所有事件类型的序列化/反序列化"""
    for event_type in UsageEventType:
        event = build_usage_event(
            event_type=event_type,
            request_id=f"req-{event_type.value}",
            data={"type": event_type.value},
        )
        fields = event.to_stream_fields()
        restored = UsageEvent.from_stream_fields(fields)
        assert restored.event_type == event_type
        assert restored.data["type"] == event_type.value


@pytest.mark.asyncio
async def test_usage_event_bytes_payload() -> None:
    """测试 bytes 类型 payload 的反序列化"""
    event = build_usage_event(
        event_type=UsageEventType.COMPLETED,
        request_id="req-bytes",
        data={"key": "value"},
    )
    fields = event.to_stream_fields()
    # 模拟 Redis 返回 bytes
    fields["payload"] = fields["payload"].encode("utf-8")  # type: ignore[assignment]
    restored = UsageEvent.from_stream_fields(fields)
    assert restored.request_id == "req-bytes"


@pytest.mark.asyncio
async def test_usage_event_missing_payload() -> None:
    """测试缺少 payload 字段时抛出异常"""
    with pytest.raises(ValueError, match="Missing payload"):
        UsageEvent.from_stream_fields({})


def test_sanitize_payload_nested() -> None:
    """测试 sanitize_payload 处理嵌套结构"""
    data = {
        "str": "hello",
        "int": 42,
        "float": 3.14,
        "bool": True,
        "none": None,
        "list": [1, "two", {"nested": True}],
        "dict": {"a": 1, "b": [2, 3]},
        "custom": object(),  # 非基础类型，应转为 str
    }
    result = sanitize_payload(data)
    assert result["str"] == "hello"
    assert result["int"] == 42
    assert result["list"] == [1, "two", {"nested": True}]
    assert isinstance(result["custom"], str)


def test_parse_body_json_string() -> None:
    """测试 _parse_body 正确反序列化 JSON 字符串"""
    from src.services.usage.consumer_streams import _parse_body

    # JSON 字符串应被解析为 dict
    json_str = '{"messages": [{"role": "user", "content": "hello"}]}'
    result = _parse_body(json_str)
    assert isinstance(result, dict)
    assert result["messages"][0]["content"] == "hello"


def test_parse_body_dict_passthrough() -> None:
    """测试 _parse_body 直接返回 dict"""
    from src.services.usage.consumer_streams import _parse_body

    data = {"key": "value"}
    result = _parse_body(data)
    assert result is data  # 应该是同一个对象


def test_parse_body_none() -> None:
    """测试 _parse_body 处理 None"""
    from src.services.usage.consumer_streams import _parse_body

    assert _parse_body(None) is None


def test_parse_body_truncated_string() -> None:
    """测试 _parse_body 处理被截断的 JSON 字符串"""
    from src.services.usage.consumer_streams import _parse_body

    # 被截断的字符串无法解析，应原样返回
    truncated = '{"content": "x...[truncated]'
    result = _parse_body(truncated)
    assert result == truncated


# ============ telemetry_writer.py 测试 ============


@pytest.mark.asyncio
async def test_db_telemetry_writer_filters_kwargs() -> None:
    """测试 DbTelemetryWriter 过滤不支持的参数"""
    mock_telemetry = MagicMock()
    mock_telemetry.record_success = AsyncMock()
    mock_telemetry.record_failure = AsyncMock()
    mock_telemetry.record_cancelled = AsyncMock()

    writer = DbTelemetryWriter(mock_telemetry)

    # 调用 record_success，包含应被过滤的参数
    await writer.record_success(
        provider="test",
        model="gpt-4",
        request_type="chat",  # 应被过滤
        metadata={"foo": "bar"},  # 应被过滤
        input_tokens=100,
    )

    mock_telemetry.record_success.assert_called_once()
    call_kwargs = mock_telemetry.record_success.call_args.kwargs
    assert "request_type" not in call_kwargs
    assert "metadata" not in call_kwargs
    assert call_kwargs["provider"] == "test"
    assert call_kwargs["input_tokens"] == 100


@pytest.mark.asyncio
async def test_db_telemetry_writer_all_methods() -> None:
    """测试 DbTelemetryWriter 的所有方法"""
    mock_telemetry = MagicMock()
    mock_telemetry.record_success = AsyncMock()
    mock_telemetry.record_failure = AsyncMock()
    mock_telemetry.record_cancelled = AsyncMock()

    writer = DbTelemetryWriter(mock_telemetry)

    await writer.record_success(provider="p1")
    await writer.record_failure(provider="p2")
    await writer.record_cancelled(provider="p3")

    mock_telemetry.record_success.assert_called_once()
    mock_telemetry.record_failure.assert_called_once()
    mock_telemetry.record_cancelled.assert_called_once()


@pytest.mark.asyncio
async def test_queue_writer_record_failure(monkeypatch: Any) -> None:
    """测试 QueueTelemetryWriter.record_failure"""
    dummy = DummyRedis()

    async def _get_redis_client(require_redis: bool = False) -> Any:
        return dummy

    monkeypatch.setattr("src.services.usage.telemetry_writer.get_redis_client", _get_redis_client)

    old_maxlen = config.usage_queue_stream_maxlen
    try:
        config.usage_queue_stream_maxlen = 100

        writer = QueueTelemetryWriter(
            request_id="req-fail",
            user_id="user-1",
            api_key_id="key-1",
        )
        await writer.record_failure(
            provider="test",
            model="model",
            error_message="something went wrong",
            status_code=500,
        )
    finally:
        config.usage_queue_stream_maxlen = old_maxlen

    assert len(dummy.calls) == 1
    _, fields, maxlen, approx = dummy.calls[0]
    assert maxlen == 100
    assert approx is True
    event = UsageEvent.from_stream_fields(fields)
    assert event.event_type == UsageEventType.FAILED
    assert event.data["error_message"] == "something went wrong"


@pytest.mark.asyncio
async def test_queue_writer_record_cancelled(monkeypatch: Any) -> None:
    """测试 QueueTelemetryWriter.record_cancelled"""
    dummy = DummyRedis()

    async def _get_redis_client(require_redis: bool = False) -> Any:
        return dummy

    monkeypatch.setattr("src.services.usage.telemetry_writer.get_redis_client", _get_redis_client)

    writer = QueueTelemetryWriter(
        request_id="req-cancel",
        user_id="user-1",
        api_key_id="key-1",
    )
    await writer.record_cancelled(provider="test", model="model")

    event = UsageEvent.from_stream_fields(dummy.calls[0][1])
    assert event.event_type == UsageEventType.CANCELLED


@pytest.mark.asyncio
async def test_queue_writer_include_headers_bodies(monkeypatch: Any) -> None:
    """测试 include_headers 和 include_bodies 配置"""
    dummy = DummyRedis()

    async def _get_redis_client(require_redis: bool = False) -> Any:
        return dummy

    monkeypatch.setattr("src.services.usage.telemetry_writer.get_redis_client", _get_redis_client)

    writer = QueueTelemetryWriter(
        request_id="req-full",
        user_id="user-1",
        api_key_id="key-1",
        log_level="full",
        sensitive_headers=["authorization"],
        max_request_body_size=0,
        max_response_body_size=0,
    )
    await writer.record_success(
        provider="test",
        model="model",
        request_headers={"Authorization": "Bearer xxx"},
        response_headers={"Content-Type": "application/json"},
        request_body={"messages": [{"role": "user", "content": "hi"}]},
        response_body={"choices": [{"message": {"content": "hello"}}]},
    )

    event = UsageEvent.from_stream_fields(dummy.calls[0][1])
    # Sensitive header should be masked before going into Redis
    assert event.data["request_headers"]["Authorization"].startswith("Bear")
    assert "****" in event.data["request_headers"]["Authorization"]
    assert "request_body" in event.data
    assert "response_body" in event.data


@pytest.mark.asyncio
async def test_queue_writer_failure_preserves_empty_request_headers(monkeypatch: Any) -> None:
    """失败事件在传入空请求头时也应保留 request_headers 字段。"""
    dummy = DummyRedis()

    async def _get_redis_client(require_redis: bool = False) -> Any:
        return dummy

    monkeypatch.setattr("src.services.usage.telemetry_writer.get_redis_client", _get_redis_client)

    writer = QueueTelemetryWriter(
        request_id="req-empty-hdr",
        user_id="user-1",
        api_key_id="key-1",
        log_level="full",
    )
    await writer.record_failure(
        provider="test",
        model="model",
        status_code=500,
        error_message="boom",
        request_headers={},
        request_body={"message": "x"},
    )

    event = UsageEvent.from_stream_fields(dummy.calls[0][1])
    assert "request_headers" in event.data
    assert event.data["request_headers"] == {}
    assert event.data["request_body"] == {"message": "x"}


@pytest.mark.asyncio
async def test_event_to_record_body_deserialization(monkeypatch: Any) -> None:
    """测试 _event_to_record 正确反序列化 body 字符串为 dict"""
    from src.services.usage.consumer_streams import _event_to_record

    # 模拟 QueueTelemetryWriter 产生的事件（body 被序列化为 JSON 字符串）
    event = build_usage_event(
        event_type=UsageEventType.COMPLETED,
        request_id="req-body-test",
        data={
            "user_id": "user-1",
            "api_key_id": "key-1",
            "provider": "test",
            "model": "gpt-4",
            # body 是 JSON 字符串（QueueTelemetryWriter._truncate_body 的输出）
            "request_body": '{"messages": [{"role": "user", "content": "hello"}]}',
            "response_body": '{"choices": [{"message": {"content": "hi"}}]}',
        },
    )

    record = _event_to_record(event)

    # 验证 body 被正确反序列化为 dict
    assert isinstance(record["request_body"], dict)
    assert record["request_body"]["messages"][0]["content"] == "hello"
    assert isinstance(record["response_body"], dict)
    assert record["response_body"]["choices"][0]["message"]["content"] == "hi"


@pytest.mark.asyncio
async def test_queue_writer_body_truncation(monkeypatch: Any) -> None:
    """测试 body 超长截断"""
    dummy = DummyRedis()

    async def _get_redis_client(require_redis: bool = False) -> Any:
        return dummy

    monkeypatch.setattr("src.services.usage.telemetry_writer.get_redis_client", _get_redis_client)

    writer = QueueTelemetryWriter(
        request_id="req-trunc",
        user_id="user-1",
        api_key_id="key-1",
        log_level="full",
        max_request_body_size=50,
        max_response_body_size=0,
    )
    long_body = {"content": "x" * 1000}
    await writer.record_success(
        provider="test",
        model="model",
        request_body=long_body,
    )

    event = UsageEvent.from_stream_fields(dummy.calls[0][1])
    body = event.data["request_body"]
    assert isinstance(body, dict)
    assert body.get("_truncated") is True
    assert len(body.get("_content") or "") <= 50


@pytest.mark.asyncio
async def test_queue_writer_redis_unavailable(monkeypatch: Any) -> None:
    """测试 Redis 不可用时抛出异常"""

    async def _get_redis_client(require_redis: bool = False) -> Any:
        return None

    monkeypatch.setattr("src.services.usage.telemetry_writer.get_redis_client", _get_redis_client)

    writer = QueueTelemetryWriter(
        request_id="req-no-redis",
        user_id="user-1",
        api_key_id="key-1",
    )
    with pytest.raises(RuntimeError, match="Redis unavailable"):
        await writer.record_success(provider="test", model="model")


@pytest.mark.asyncio
async def test_queue_writer_xadd_error(monkeypatch: Any) -> None:
    """测试 XADD 失败时抛出异常"""
    dummy = DummyRedis()
    dummy.xadd_error = Exception("Redis connection lost")

    async def _get_redis_client(require_redis: bool = False) -> Any:
        return dummy

    monkeypatch.setattr("src.services.usage.telemetry_writer.get_redis_client", _get_redis_client)

    writer = QueueTelemetryWriter(
        request_id="req-xadd-fail",
        user_id="user-1",
        api_key_id="key-1",
    )
    with pytest.raises(Exception, match="Redis connection lost"):
        await writer.record_success(provider="test", model="model")


# ============ consumer_streams.py 测试 ============


class MockRedisPipeline:
    """模拟 Redis Pipeline"""

    def __init__(self, parent: "MockRedisForConsumer") -> None:
        self._parent = parent
        self._commands: list[tuple[str, ...]] = []

    def xack(self, key: str, group: str, message_id: str) -> Any:
        self._commands.append(("xack", key, group, message_id))
        return self

    async def execute(self) -> Any:
        results = []
        for cmd in self._commands:
            if cmd[0] == "xack":
                _, key, group, message_id = cmd
                self._parent.xack_calls.append((key, group, message_id))
                results.append(1)
        return results


class MockRedisForConsumer:
    """模拟 Redis 客户端用于消费者测试"""

    def __init__(self) -> None:
        self.xgroup_create_calls: list[tuple[str, str, str, bool]] = []
        self.xreadgroup_results: list[Any] = []
        self.xautoclaim_results: list[Any] = []
        self.xack_calls: list[tuple[str, str, str]] = []
        self.xadd_calls: list[tuple[str, dict[str, str], int | None, bool | None]] = []
        self.xpending_range_results: list[Any] = []
        self.xlen_result: int = 0
        self.xpending_result: dict[str, int] | tuple[int, str, str, list[Any]] = {"pending": 0}
        self.xgroup_create_error: Exception | None = None

    async def xgroup_create(self, key: str, group: str, id: str, mkstream: bool = False) -> Any:
        self.xgroup_create_calls.append((key, group, id, mkstream))
        if self.xgroup_create_error:
            raise self.xgroup_create_error

    async def xreadgroup(
        self,
        groupname: str,
        consumername: str,
        streams: dict[str, str],
        count: int,
        block: int,
    ) -> Any:
        if self.xreadgroup_results:
            return self.xreadgroup_results.pop(0)
        return None

    async def xautoclaim(
        self, key: str, group: str, consumer: str, min_idle_time: int, start_id: str, count: int
    ) -> Any:
        if self.xautoclaim_results:
            return self.xautoclaim_results.pop(0)
        return None

    async def xack(self, key: str, group: str, message_id: str) -> Any:
        self.xack_calls.append((key, group, message_id))

    async def xadd(
        self,
        key: str,
        fields: dict[str, str],
        maxlen: int | None = None,
        approximate: bool | None = None,
    ) -> Any:
        self.xadd_calls.append((key, fields, maxlen, approximate))
        return "dlq-1-0"

    async def xpending_range(self, key: str, group: str, min: str, max: str, count: int) -> Any:
        if self.xpending_range_results:
            return self.xpending_range_results.pop(0)
        return []

    async def xlen(self, key: str) -> Any:
        return self.xlen_result

    async def xpending(self, key: str, group: str) -> Any:
        return self.xpending_result

    def pipeline(self) -> Any:
        return MockRedisPipeline(self)


def test_consumer_name() -> None:
    """测试消费者名称生成"""
    name = _consumer_name()
    assert ":" in name
    # 应包含 PID
    import os

    assert str(os.getpid()) in name


@pytest.mark.asyncio
async def test_ensure_stream_group_creates_group(monkeypatch: Any) -> None:
    """测试创建消费者组"""
    mock_redis = MockRedisForConsumer()

    async def _get_redis_client(require_redis: bool = False) -> Any:
        return mock_redis

    monkeypatch.setattr("src.services.usage.consumer_streams.get_redis_client", _get_redis_client)

    await ensure_usage_stream_group()

    assert len(mock_redis.xgroup_create_calls) == 1
    key, group, id_, mkstream = mock_redis.xgroup_create_calls[0]
    assert key == config.usage_queue_stream_key
    assert group == config.usage_queue_stream_group
    assert mkstream is True


@pytest.mark.asyncio
async def test_ensure_stream_group_handles_busygroup(monkeypatch: Any) -> None:
    """测试消费者组已存在时不抛异常"""
    mock_redis = MockRedisForConsumer()
    mock_redis.xgroup_create_error = ResponseError("BUSYGROUP Consumer Group name already exists")

    async def _get_redis_client(require_redis: bool = False) -> Any:
        return mock_redis

    monkeypatch.setattr("src.services.usage.consumer_streams.get_redis_client", _get_redis_client)

    # 不应抛出异常
    await ensure_usage_stream_group()


@pytest.mark.asyncio
async def test_ensure_stream_group_raises_other_errors(monkeypatch: Any) -> None:
    """测试其他 Redis 错误时抛出异常"""
    mock_redis = MockRedisForConsumer()
    mock_redis.xgroup_create_error = ResponseError("SOME OTHER ERROR")

    async def _get_redis_client(require_redis: bool = False) -> Any:
        return mock_redis

    monkeypatch.setattr("src.services.usage.consumer_streams.get_redis_client", _get_redis_client)

    with pytest.raises(ResponseError, match="SOME OTHER ERROR"):
        await ensure_usage_stream_group()


@pytest.mark.asyncio
async def test_ensure_stream_group_no_redis(monkeypatch: Any) -> None:
    """测试 Redis 不可用时直接返回"""

    async def _get_redis_client(require_redis: bool = False) -> Any:
        return None

    monkeypatch.setattr("src.services.usage.consumer_streams.get_redis_client", _get_redis_client)

    # 不应抛出异常
    await ensure_usage_stream_group()


@pytest.mark.asyncio
async def test_consumer_start_stop() -> None:
    """测试消费者启动和停止"""
    consumer = UsageQueueConsumer()
    assert not consumer._running

    # Mock _run 避免真正执行
    consumer._run = AsyncMock()  # type: ignore[method-assign]

    await consumer.start()
    assert consumer._running
    assert consumer._task is not None

    # 重复启动应该是幂等的
    await consumer.start()
    assert consumer._running

    await consumer.stop()
    assert not consumer._running

    # 重复停止应该是幂等的
    await consumer.stop()
    assert not consumer._running


@pytest.mark.asyncio
async def test_consumer_process_messages_success(monkeypatch: Any) -> None:
    """测试成功处理消息"""
    mock_redis = MockRedisForConsumer()

    # 创建测试事件
    event = build_usage_event(
        event_type=UsageEventType.COMPLETED,
        request_id="req-test-1",
        data={
            "user_id": None,
            "api_key_id": None,
            "provider": "test-provider",
            "model": "test-model",
            "input_tokens": 10,
            "output_tokens": 20,
        },
    )
    message_id = "1-0"
    messages = [(message_id, event.to_stream_fields())]

    consumer = UsageQueueConsumer()

    # Mock 批量处理方法
    consumer._process_record_batch = AsyncMock()  # type: ignore[method-assign]

    await consumer._process_messages(mock_redis, messages)

    # 验证批量处理被调用
    consumer._process_record_batch.assert_called_once()
    call_messages = consumer._process_record_batch.call_args[0][1]
    assert len(call_messages) == 1
    assert call_messages[0][2].request_id == "req-test-1"


@pytest.mark.asyncio
async def test_consumer_process_messages_error_retry(monkeypatch: Any) -> None:
    """测试处理消息失败时 STREAMING 事件的重试行为"""
    mock_redis = MockRedisForConsumer()
    # 返回重试次数小于 max_retries
    mock_redis.xpending_range_results = [[{"times_delivered": 2}]]

    # 使用 STREAMING 事件测试单条处理失败
    event = build_usage_event(
        event_type=UsageEventType.STREAMING,
        request_id="req-err",
        data={"provider": "test"},
    )
    message_id = "err-1-0"
    messages = [(message_id, event.to_stream_fields())]

    old_max_retries = config.usage_queue_max_retries
    try:
        config.usage_queue_max_retries = 5

        # 在设置 config 后创建 consumer，以便缓存正确的配置值
        consumer = UsageQueueConsumer()
        # 模拟 STREAMING 事件处理失败
        consumer._apply_streaming_event = AsyncMock(side_effect=ValueError("Processing error"))  # type: ignore[method-assign]

        await consumer._process_messages(mock_redis, messages)

        # 消息不应该被 ack（还能重试）
        assert len(mock_redis.xack_calls) == 0
        # 不应该移入 DLQ（重试次数未达上限）
        assert len(mock_redis.xadd_calls) == 0
    finally:
        config.usage_queue_max_retries = old_max_retries


@pytest.mark.asyncio
async def test_consumer_process_messages_move_to_dlq(monkeypatch: Any) -> None:
    """测试消息超过最大重试次数后移入 DLQ"""
    mock_redis = MockRedisForConsumer()
    # 返回重试次数 >= max_retries
    mock_redis.xpending_range_results = [[{"times_delivered": 10}]]

    # 测试解析失败的消息会被移入 DLQ
    invalid_fields = {"payload": "invalid json"}
    message_id = "dlq-1-0"
    messages = [(message_id, invalid_fields)]

    old_max_retries = config.usage_queue_max_retries
    old_dlq_maxlen = config.usage_queue_dlq_maxlen
    try:
        config.usage_queue_max_retries = 5
        config.usage_queue_dlq_maxlen = 1000

        # 在设置 config 后创建 consumer，以便缓存正确的配置值
        consumer = UsageQueueConsumer()

        await consumer._process_messages(mock_redis, messages)

        # 应该移入 DLQ（解析失败的消息）
        assert len(mock_redis.xadd_calls) == 1
        dlq_key, dlq_fields, maxlen, approx = mock_redis.xadd_calls[0]
        assert dlq_key == config.usage_queue_dlq_key
        assert dlq_fields["source_id"] == message_id
        assert "error" in dlq_fields
        assert maxlen == 1000
        assert approx is True

        # 消息应该被 ack
        assert len(mock_redis.xack_calls) == 1
    finally:
        config.usage_queue_max_retries = old_max_retries
        config.usage_queue_dlq_maxlen = old_dlq_maxlen


@pytest.mark.asyncio
async def test_consumer_get_delivery_count_dict_format() -> None:
    """测试获取消息投递次数（dict 格式）"""
    mock_redis = MockRedisForConsumer()
    mock_redis.xpending_range_results = [[{"times_delivered": 3}]]

    consumer = UsageQueueConsumer()
    count = await consumer._get_delivery_count(mock_redis, "msg-1")
    assert count == 3


@pytest.mark.asyncio
async def test_consumer_get_delivery_count_tuple_format() -> None:
    """测试获取消息投递次数（tuple 格式）"""
    mock_redis = MockRedisForConsumer()
    # (message_id, consumer, idle_time, times_delivered)
    mock_redis.xpending_range_results = [[("msg-1", "consumer-1", 1000, 5)]]

    consumer = UsageQueueConsumer()
    count = await consumer._get_delivery_count(mock_redis, "msg-1")
    assert count == 5


@pytest.mark.asyncio
async def test_consumer_get_delivery_count_empty() -> None:
    """测试消息不存在时返回 0"""
    mock_redis = MockRedisForConsumer()
    mock_redis.xpending_range_results = [[]]

    consumer = UsageQueueConsumer()
    count = await consumer._get_delivery_count(mock_redis, "nonexistent")
    assert count == 0


@pytest.mark.asyncio
async def test_consumer_read_new_messages(monkeypatch: Any) -> None:
    """测试读取新消息"""
    mock_redis = MockRedisForConsumer()

    event = build_usage_event(
        event_type=UsageEventType.COMPLETED,
        request_id="req-new",
        data={"provider": "test"},
    )
    # xreadgroup 返回格式: [(stream_key, [(msg_id, fields), ...])]
    mock_redis.xreadgroup_results = [
        [(config.usage_queue_stream_key, [("new-1-0", event.to_stream_fields())])]
    ]

    consumer = UsageQueueConsumer()
    consumer._process_messages = AsyncMock()  # type: ignore[method-assign]

    await consumer._read_new(mock_redis)

    consumer._process_messages.assert_called_once()
    _, messages = consumer._process_messages.call_args[0]
    assert len(messages) == 1
    assert messages[0][0] == "new-1-0"


@pytest.mark.asyncio
async def test_consumer_maybe_claim_pending_respects_interval(monkeypatch: Any) -> None:
    """测试 claim 间隔限制"""
    mock_redis = MockRedisForConsumer()

    consumer = UsageQueueConsumer()
    consumer._last_claim = 999999999999.0  # 未来时间

    consumer._process_messages = AsyncMock()  # type: ignore[method-assign]

    await consumer._maybe_claim_pending(mock_redis)

    # 由于间隔未到，不应该处理消息
    consumer._process_messages.assert_not_called()


@pytest.mark.asyncio
async def test_consumer_maybe_claim_pending_xautoclaim_error(monkeypatch: Any) -> None:
    """测试 XAUTOCLAIM 失败时优雅处理"""
    mock_redis = MockRedisForConsumer()

    async def failing_xautoclaim(*args: Any, **kwargs: Any) -> Any:
        raise ResponseError("XAUTOCLAIM error")

    mock_redis.xautoclaim = failing_xautoclaim  # type: ignore[method-assign]

    consumer = UsageQueueConsumer()
    consumer._last_claim = 0  # 确保会尝试 claim

    # 不应抛出异常
    await consumer._maybe_claim_pending(mock_redis)


@pytest.mark.asyncio
async def test_consumer_log_metrics_dict_pending() -> None:
    """测试 metrics 日志（dict 格式 pending）"""
    mock_redis = MockRedisForConsumer()
    mock_redis.xlen_result = 100
    mock_redis.xpending_result = {"pending": 10}

    consumer = UsageQueueConsumer()
    consumer._last_metrics_log = 0  # 确保会执行

    old_interval = config.usage_queue_metrics_interval_seconds
    try:
        config.usage_queue_metrics_interval_seconds = 0

        await consumer._log_metrics(mock_redis)
        # 不抛出异常即可
    finally:
        config.usage_queue_metrics_interval_seconds = old_interval


@pytest.mark.asyncio
async def test_consumer_log_metrics_tuple_pending() -> None:
    """测试 metrics 日志（tuple 格式 pending）"""
    mock_redis = MockRedisForConsumer()
    mock_redis.xlen_result = 50
    # tuple 格式: (pending_count, min_id, max_id, consumers)
    mock_redis.xpending_result = (5, "1-0", "5-0", [])

    consumer = UsageQueueConsumer()
    consumer._last_metrics_log = 0

    old_interval = config.usage_queue_metrics_interval_seconds
    try:
        config.usage_queue_metrics_interval_seconds = 0

        await consumer._log_metrics(mock_redis)
    finally:
        config.usage_queue_metrics_interval_seconds = old_interval


@pytest.mark.asyncio
async def test_consumer_apply_event_streaming(monkeypatch: Any) -> None:
    """测试处理 STREAMING 事件"""
    mock_db = MagicMock()

    def mock_create_session() -> Any:
        return mock_db

    monkeypatch.setattr("src.services.usage.consumer_streams.create_session", mock_create_session)

    mock_update_status = MagicMock()
    monkeypatch.setattr(
        "src.services.usage.consumer_streams.UsageService.update_usage_status",
        mock_update_status,
    )

    consumer = UsageQueueConsumer()

    event = UsageEvent(
        event_type=UsageEventType.STREAMING,
        request_id="req-stream",
        timestamp_ms=123,
        data={
            "provider": "test",
            "target_model": "gpt-4",
            "first_byte_time_ms": 100,
        },
    )

    await consumer._apply_event(event)

    mock_update_status.assert_called_once()
    call_kwargs = mock_update_status.call_args.kwargs
    assert call_kwargs["request_id"] == "req-stream"
    assert call_kwargs["status"] == "streaming"
    assert call_kwargs["provider"] == "test"
    mock_db.close.assert_called_once()


@pytest.mark.asyncio
async def test_consumer_apply_event_completed(monkeypatch: Any) -> None:
    """测试处理 COMPLETED 事件"""
    mock_db = MagicMock()
    mock_db.query.return_value.filter.return_value.first.return_value = None

    def mock_create_session() -> Any:
        return mock_db

    monkeypatch.setattr("src.services.usage.consumer_streams.create_session", mock_create_session)

    mock_record_usage = AsyncMock()
    monkeypatch.setattr(
        "src.services.usage.consumer_streams.UsageService.record_usage",
        mock_record_usage,
    )

    consumer = UsageQueueConsumer()

    event = UsageEvent(
        event_type=UsageEventType.COMPLETED,
        request_id="req-done",
        timestamp_ms=123,
        data={
            "provider": "openai",
            "model": "gpt-4",
            "input_tokens": 100,
            "output_tokens": 200,
        },
    )

    await consumer._apply_event(event)

    mock_record_usage.assert_called_once()
    call_kwargs = mock_record_usage.call_args.kwargs
    assert call_kwargs["request_id"] == "req-done"
    assert call_kwargs["status"] == "completed"
    assert call_kwargs["provider"] == "openai"
    assert call_kwargs["input_tokens"] == 100


@pytest.mark.asyncio
async def test_consumer_apply_event_failed(monkeypatch: Any) -> None:
    """测试处理 FAILED 事件"""
    mock_db = MagicMock()
    mock_db.query.return_value.filter.return_value.first.return_value = None

    def mock_create_session() -> Any:
        return mock_db

    monkeypatch.setattr("src.services.usage.consumer_streams.create_session", mock_create_session)

    mock_record_usage = AsyncMock()
    monkeypatch.setattr(
        "src.services.usage.consumer_streams.UsageService.record_usage",
        mock_record_usage,
    )

    consumer = UsageQueueConsumer()

    event = UsageEvent(
        event_type=UsageEventType.FAILED,
        request_id="req-fail",
        timestamp_ms=123,
        data={
            "provider": "openai",
            "model": "gpt-4",
            "error_message": "Rate limited",
        },
    )

    await consumer._apply_event(event)

    call_kwargs = mock_record_usage.call_args.kwargs
    assert call_kwargs["status"] == "failed"
    assert call_kwargs["error_message"] == "Rate limited"


@pytest.mark.asyncio
async def test_consumer_apply_event_cancelled(monkeypatch: Any) -> None:
    """测试处理 CANCELLED 事件"""
    mock_db = MagicMock()
    mock_db.query.return_value.filter.return_value.first.return_value = None

    def mock_create_session() -> Any:
        return mock_db

    monkeypatch.setattr("src.services.usage.consumer_streams.create_session", mock_create_session)

    mock_record_usage = AsyncMock()
    monkeypatch.setattr(
        "src.services.usage.consumer_streams.UsageService.record_usage",
        mock_record_usage,
    )

    consumer = UsageQueueConsumer()

    event = UsageEvent(
        event_type=UsageEventType.CANCELLED,
        request_id="req-cancel",
        timestamp_ms=123,
        data={"provider": "openai", "model": "gpt-4"},
    )

    await consumer._apply_event(event)

    call_kwargs = mock_record_usage.call_args.kwargs
    assert call_kwargs["status"] == "cancelled"


# ============ 批量处理测试 ============


@pytest.mark.asyncio
async def test_consumer_process_messages_batch(monkeypatch: Any) -> None:
    """测试批量处理消息"""
    mock_redis = MockRedisForConsumer()

    # 创建多个测试事件
    events = []
    for i in range(3):
        event = build_usage_event(
            event_type=UsageEventType.COMPLETED,
            request_id=f"req-batch-{i}",
            data={
                "user_id": None,
                "api_key_id": None,
                "provider": "test-provider",
                "model": "test-model",
                "input_tokens": 10 * (i + 1),
                "output_tokens": 20 * (i + 1),
            },
        )
        events.append((f"msg-{i}", event.to_stream_fields(), event))

    messages = [(msg_id, fields) for msg_id, fields, _ in events]

    consumer = UsageQueueConsumer()

    # Mock 批量处理
    consumer._process_record_batch = AsyncMock()  # type: ignore[method-assign]

    await consumer._process_messages(mock_redis, messages)

    # 验证批量处理被调用
    consumer._process_record_batch.assert_called_once()


@pytest.mark.asyncio
async def test_consumer_process_messages_separates_streaming(monkeypatch: Any) -> None:
    """测试消息分类：STREAMING 和其他事件分开处理"""
    mock_redis = MockRedisForConsumer()

    # 创建混合事件
    streaming_event = build_usage_event(
        event_type=UsageEventType.STREAMING,
        request_id="req-stream",
        data={"provider": "test"},
    )
    completed_event = build_usage_event(
        event_type=UsageEventType.COMPLETED,
        request_id="req-done",
        data={"provider": "test", "model": "gpt-4"},
    )

    messages = [
        ("msg-stream", streaming_event.to_stream_fields()),
        ("msg-done", completed_event.to_stream_fields()),
    ]

    consumer = UsageQueueConsumer()
    consumer._apply_streaming_event = AsyncMock()  # type: ignore[method-assign]
    consumer._process_record_batch = AsyncMock()  # type: ignore[method-assign]

    await consumer._process_messages(mock_redis, messages)

    # STREAMING 事件单独处理
    consumer._apply_streaming_event.assert_called_once()
    streaming_call_event = consumer._apply_streaming_event.call_args[0][0]
    assert streaming_call_event.request_id == "req-stream"

    # 记录事件批量处理
    consumer._process_record_batch.assert_called_once()
    batch_call_messages = consumer._process_record_batch.call_args[0][1]
    assert len(batch_call_messages) == 1
    assert batch_call_messages[0][2].request_id == "req-done"


@pytest.mark.asyncio
async def test_consumer_process_record_batch_success(monkeypatch: Any) -> None:
    """测试批量记录处理成功"""
    mock_redis = MockRedisForConsumer()
    mock_db = MagicMock()

    def mock_create_session() -> Any:
        return mock_db

    monkeypatch.setattr("src.services.usage.consumer_streams.create_session", mock_create_session)

    mock_record_batch = AsyncMock(return_value=[])
    monkeypatch.setattr(
        "src.services.usage.consumer_streams.UsageService.record_usage_batch",
        mock_record_batch,
    )

    consumer = UsageQueueConsumer()

    # 创建批量消息
    events_data = []
    for i in range(3):
        event = build_usage_event(
            event_type=UsageEventType.COMPLETED,
            request_id=f"req-{i}",
            data={"provider": "test", "model": "gpt-4", "input_tokens": 10, "output_tokens": 20},
        )
        events_data.append((f"msg-{i}", event.to_stream_fields(), event))

    await consumer._process_record_batch(mock_redis, events_data)

    # 验证批量写入被调用
    mock_record_batch.assert_called_once()
    records = mock_record_batch.call_args[0][1]
    assert len(records) == 3

    # 验证所有消息被 ACK
    assert len(mock_redis.xack_calls) == 3


@pytest.mark.asyncio
async def test_consumer_process_record_batch_fallback(monkeypatch: Any) -> None:
    """测试批量处理失败时回退到逐条处理"""
    mock_redis = MockRedisForConsumer()
    mock_db = MagicMock()
    mock_db.query.return_value.filter.return_value.first.return_value = None

    def mock_create_session() -> Any:
        return mock_db

    monkeypatch.setattr("src.services.usage.consumer_streams.create_session", mock_create_session)

    # 批量处理失败
    mock_record_batch = AsyncMock(side_effect=Exception("Batch failed"))
    monkeypatch.setattr(
        "src.services.usage.consumer_streams.UsageService.record_usage_batch",
        mock_record_batch,
    )

    # 单条处理成功
    mock_record_usage = AsyncMock()
    monkeypatch.setattr(
        "src.services.usage.consumer_streams.UsageService.record_usage",
        mock_record_usage,
    )

    consumer = UsageQueueConsumer()

    event = build_usage_event(
        event_type=UsageEventType.COMPLETED,
        request_id="req-fallback",
        data={"provider": "test", "model": "gpt-4"},
    )

    await consumer._process_record_batch(
        mock_redis,
        [("msg-1", event.to_stream_fields(), event)],
    )

    # 验证回退到单条处理
    mock_record_usage.assert_called_once()
    # 消息被 ACK
    assert len(mock_redis.xack_calls) == 1


@pytest.mark.asyncio
async def test_consumer_apply_streaming_event(monkeypatch: Any) -> None:
    """测试 STREAMING 事件处理"""
    mock_db = MagicMock()

    def mock_create_session() -> Any:
        return mock_db

    monkeypatch.setattr("src.services.usage.consumer_streams.create_session", mock_create_session)

    mock_update_status = MagicMock()
    monkeypatch.setattr(
        "src.services.usage.consumer_streams.UsageService.update_usage_status",
        mock_update_status,
    )

    consumer = UsageQueueConsumer()

    event = UsageEvent(
        event_type=UsageEventType.STREAMING,
        request_id="req-streaming",
        timestamp_ms=123,
        data={
            "provider": "openai",
            "target_model": "gpt-4",
            "first_byte_time_ms": 150,
        },
    )

    await consumer._apply_streaming_event(event)

    mock_update_status.assert_called_once()
    call_kwargs = mock_update_status.call_args.kwargs
    assert call_kwargs["request_id"] == "req-streaming"
    assert call_kwargs["status"] == "streaming"
    assert call_kwargs["first_byte_time_ms"] == 150


@pytest.mark.asyncio
async def test_consumer_apply_record_event(monkeypatch: Any) -> None:
    """测试记录事件单独处理"""
    mock_db = MagicMock()
    mock_db.query.return_value.filter.return_value.first.return_value = None

    def mock_create_session() -> Any:
        return mock_db

    monkeypatch.setattr("src.services.usage.consumer_streams.create_session", mock_create_session)

    mock_record_usage = AsyncMock()
    monkeypatch.setattr(
        "src.services.usage.consumer_streams.UsageService.record_usage",
        mock_record_usage,
    )

    consumer = UsageQueueConsumer()

    event = UsageEvent(
        event_type=UsageEventType.COMPLETED,
        request_id="req-record",
        timestamp_ms=123,
        data={"provider": "openai", "model": "gpt-4", "input_tokens": 100},
    )

    await consumer._apply_record_event(event)

    mock_record_usage.assert_called_once()
    call_kwargs = mock_record_usage.call_args.kwargs
    assert call_kwargs["request_id"] == "req-record"
    assert call_kwargs["input_tokens"] == 100


@pytest.mark.asyncio
async def test_record_usage_batch_updates_when_status_completed_billing_pending(
    monkeypatch: Any,
) -> None:
    """回归测试：
    usage-queue 模式下，handler 可能会先把 Usage.status 直接更新为 completed（为减少 UI 延迟），
    但 billing_status 仍为 pending。此时 completed 事件仍应更新详情字段（如 response_headers/body），
    并将 billing_status 结算为 settled。
    """

    from src.models.database import Usage
    from src.services.usage.service import UsageService

    class DummyQuery:
        def __init__(self, all_result: list[Any]) -> None:
            self._all_result = all_result

        def options(self, *args: Any, **kwargs: Any) -> "DummyQuery":
            return self

        def filter(self, *args: Any, **kwargs: Any) -> "DummyQuery":
            return self

        def with_for_update(self) -> "DummyQuery":
            return self

        def all(self) -> list[Any]:
            return self._all_result

    existing = Usage(
        request_id="req-usage-batch-1",
        provider_name="pending",
        model="gemini-3-pro-image-preview",
        status="completed",
        billing_status="pending",
    )
    assert existing.status == "completed"
    assert existing.billing_status == "pending"
    assert existing.finalized_at is None

    db = MagicMock()
    db.query.side_effect = lambda model: (
        DummyQuery([existing]) if model is Usage else DummyQuery([])
    )

    usage_params = {
        "status": "completed",
        "response_headers": {"content-type": "text/event-stream"},
        "response_body": {"chunks": [{"foo": "bar"}], "metadata": {"stream": True}},
    }

    monkeypatch.setattr(
        UsageService,
        "_prepare_usage_records_batch",
        AsyncMock(return_value=[(usage_params, 0.0, None)]),
    )

    def _fake_update(existing_usage: Any, params: dict[str, Any], _target_model: Any) -> None:
        existing_usage.status = params.get("status", existing_usage.status)
        existing_usage.response_headers = params.get("response_headers")
        existing_usage.response_body = params.get("response_body")

    monkeypatch.setattr(UsageService, "_update_existing_usage", _fake_update)

    result = await UsageService.record_usage_batch(
        db,
        [
            {
                "request_id": "req-usage-batch-1",
                "provider": "Antigravity反代",
                "model": "gemini-3-pro-image-preview",
                "status": "completed",
            }
        ],
    )

    assert result and result[0] is existing
    assert existing.response_headers == usage_params["response_headers"]
    assert existing.response_body == usage_params["response_body"]
    assert existing.billing_status == "settled"
    assert existing.finalized_at is not None


@pytest.mark.asyncio
async def test_record_usage_batch_uses_orm_insert_for_new_records(
    monkeypatch: Any,
) -> None:
    """确保批量新建 Usage 走 ORM add 路径，以便复用统一结算逻辑。"""

    from src.models.database import Usage
    from src.services.usage.service import UsageService

    class DummyQuery:
        def __init__(self, all_result: list[Any]) -> None:
            self._all_result = all_result

        def options(self, *args: Any, **kwargs: Any) -> "DummyQuery":
            return self

        def filter(self, *args: Any, **kwargs: Any) -> "DummyQuery":
            return self

        def with_for_update(self) -> "DummyQuery":
            return self

        def all(self) -> list[Any]:
            return self._all_result

    usage_query_calls = {"count": 0}

    def _query_side_effect(model: Any) -> Any:
        if model is Usage:
            usage_query_calls["count"] += 1
            return DummyQuery([])
        return DummyQuery([])

    db = MagicMock()
    db.query.side_effect = _query_side_effect

    usage_params = {
        "request_id": "req-usage-batch-new",
        "provider_name": "openai",
        "model": "gpt-4",
        "status": "completed",
    }

    monkeypatch.setattr(
        UsageService,
        "_prepare_usage_records_batch",
        AsyncMock(return_value=[(usage_params, 0.0, None)]),
    )

    result = await UsageService.record_usage_batch(
        db,
        [
            {
                "request_id": "req-usage-batch-new",
                "provider": "openai",
                "model": "gpt-4",
                "status": "completed",
            }
        ],
    )

    db.add.assert_called_once()
    db.bulk_insert_mappings.assert_not_called()

    assert result and isinstance(result[0], Usage)
    assert result[0].request_id == "req-usage-batch-new"
    assert result[0].billing_status == "settled"
    assert result[0].finalized_at is not None
