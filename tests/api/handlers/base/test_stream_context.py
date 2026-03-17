from src.api.handlers.base import stream_context
from src.api.handlers.base.stream_context import StreamContext


def test_collected_text_append_and_property() -> None:
    ctx = StreamContext(model="test-model", api_format="openai:chat")
    assert ctx.collected_text == ""
    assert ctx.collected_text_length == 0

    ctx.append_text("hello")
    ctx.append_text(" ")
    ctx.append_text("world")
    assert ctx.collected_text == "hello world"
    assert ctx.collected_text_length == len("hello world")


def test_collected_text_is_capped_but_total_length_is_preserved() -> None:
    ctx = StreamContext(model="test-model", api_format="openai:chat")
    cap = stream_context._MAX_COLLECTED_TEXT_CHARS

    ctx.append_text("a" * (cap - 4))
    ctx.append_text("b" * 10)

    assert len(ctx.collected_text) == cap
    assert ctx.collected_text == ("a" * (cap - 4)) + ("b" * 4)
    assert ctx.collected_text_length == cap + 6


def test_reset_for_retry_clears_state() -> None:
    ctx = StreamContext(model="test-model", api_format="openai:chat")
    ctx.append_text("x")
    ctx.update_usage(input_tokens=10, output_tokens=5)
    ctx.parsed_chunks.append({"type": "chunk"})
    ctx.chunk_count = 3
    ctx.data_count = 2
    ctx.has_completion = True
    ctx.status_code = 418
    ctx.error_message = "boom"

    ctx.reset_for_retry()

    assert ctx.collected_text == ""
    assert ctx.input_tokens == 0
    assert ctx.output_tokens == 0
    assert ctx.parsed_chunks == []
    assert ctx.chunk_count == 0
    assert ctx.data_count == 0
    assert ctx.has_completion is False
    assert ctx.status_code == 200
    assert ctx.error_message is None


def test_record_first_byte_time(monkeypatch) -> None:
    """测试记录首字时间"""
    ctx = StreamContext(model="claude-3", api_format="claude_messages")
    start_time = 100.0
    monkeypatch.setattr(stream_context.time, "time", lambda: 100.0123)  # 12.3ms

    # 记录首字时间
    ctx.record_first_byte_time(start_time)

    # 验证首字时间已记录
    assert ctx.first_byte_time_ms == 12


def test_record_first_byte_time_idempotent(monkeypatch) -> None:
    """测试首字时间只记录一次"""
    ctx = StreamContext(model="claude-3", api_format="claude_messages")
    start_time = 100.0

    # 第一次记录
    monkeypatch.setattr(stream_context.time, "time", lambda: 100.010)
    ctx.record_first_byte_time(start_time)
    first_value = ctx.first_byte_time_ms

    # 第二次记录（应该被忽略）
    monkeypatch.setattr(stream_context.time, "time", lambda: 100.020)
    ctx.record_first_byte_time(start_time)
    second_value = ctx.first_byte_time_ms

    # 验证值没有改变
    assert first_value == second_value


def test_reset_for_retry_clears_first_byte_time(monkeypatch) -> None:
    """测试重试时清除首字时间"""
    ctx = StreamContext(model="claude-3", api_format="claude_messages")
    start_time = 100.0

    # 记录首字时间
    monkeypatch.setattr(stream_context.time, "time", lambda: 100.010)
    ctx.record_first_byte_time(start_time)
    assert ctx.first_byte_time_ms is not None

    # 重置
    ctx.reset_for_retry()

    # 验证首字时间已清除
    assert ctx.first_byte_time_ms is None


def test_get_log_summary_with_first_byte_time() -> None:
    """测试日志摘要包含首字时间"""
    ctx = StreamContext(model="claude-3", api_format="claude_messages")
    ctx.provider_name = "anthropic"
    ctx.input_tokens = 100
    ctx.output_tokens = 50
    ctx.first_byte_time_ms = 123

    summary = ctx.get_log_summary("request-id-123", 456)

    # 验证包含首字时间和总时间（大写格式）
    assert "TTFB: 123ms" in summary
    assert "Total: 456ms" in summary
    assert "in:100 out:50" in summary


def test_get_log_summary_without_first_byte_time() -> None:
    """测试日志摘要在没有首字时间时的格式"""
    ctx = StreamContext(model="claude-3", api_format="claude_messages")
    ctx.provider_name = "anthropic"
    ctx.input_tokens = 100
    ctx.output_tokens = 50
    # first_byte_time_ms 保持为 None

    summary = ctx.get_log_summary("request-id-123", 456)

    # 验证不包含首字时间标记,但有总时间（使用大写 TTFB 和 Total）
    assert "TTFB:" not in summary
    assert "Total: 456ms" in summary
    assert "in:100 out:50" in summary


def test_ensure_estimated_output_tokens_uses_collected_text() -> None:
    ctx = StreamContext(model="test-model", api_format="openai:chat")
    ctx.append_text("partial output")

    changed = ctx.ensure_estimated_output_tokens()

    assert changed is True
    assert ctx.output_tokens == max(1, len("partial output") // 4)


def test_should_estimate_incomplete_tokens_for_interrupted_partial_stream() -> None:
    ctx = StreamContext(model="test-model", api_format="openai:chat")
    ctx.status_code = 503
    ctx.chunk_count = 3

    assert ctx.should_estimate_incomplete_tokens() is True


def test_should_estimate_incomplete_tokens_when_output_already_estimated() -> None:
    """ensure_estimated_output_tokens 已补了 output，但 input 仍为 0 时仍需估算。"""
    ctx = StreamContext(model="test-model", api_format="openai:chat")
    ctx.status_code = 503
    ctx.chunk_count = 3
    ctx.append_text("partial")
    ctx.ensure_estimated_output_tokens()

    assert ctx.output_tokens > 0
    assert ctx.input_tokens == 0
    assert ctx.should_estimate_incomplete_tokens() is True
