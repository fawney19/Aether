"""
Layer 4: Stream conversion tests.

Verifies that each normalizer correctly converts format-specific stream
chunks to/from internal stream events.
"""

from __future__ import annotations

import pytest

from src.core.api_format.conversion.internal import (
    ContentType,
    InternalResponse,
    StopReason,
    TextBlock,
    ThinkingBlock,
)
from src.core.api_format.conversion.normalizers.openai import OpenAINormalizer
from src.core.api_format.conversion.registry import (
    format_conversion_registry,
    register_default_normalizers,
)
from src.core.api_format.conversion.stream_bridge import (
    InternalStreamAggregator,
    iter_internal_response_as_stream_events,
)
from src.core.api_format.conversion.stream_events import (
    ContentBlockStartEvent,
    ContentBlockStopEvent,
    ContentDeltaEvent,
    MessageStartEvent,
    MessageStopEvent,
)
from src.core.api_format.conversion.stream_state import StreamState

from .fixtures.assertions import (
    assert_stream_has_tool_call,
    assert_stream_stop_reason_matches,
    assert_stream_text_matches,
)
from .fixtures.schema_validators import get_stream_chunk_validator
from .fixtures.stream_fixtures import (
    STREAM_ALL_FORMATS,
    STREAM_FIXTURE_IDS,
    STREAM_FIXTURES,
)


@pytest.fixture(autouse=True, scope="module")
def _ensure_normalizers_registered() -> None:
    register_default_normalizers()


def _stream_combos() -> list[tuple[str, str]]:
    combos = []
    for fmt in STREAM_ALL_FORMATS:
        for fid in STREAM_FIXTURE_IDS:
            if fid in STREAM_FIXTURES.get(fmt, {}):
                combos.append((fmt, fid))
    return combos


_COMBOS = _stream_combos()


class TestStreamToInternal:
    """Verify format-specific stream chunks -> InternalStreamEvent sequence."""

    @pytest.mark.parametrize("format_id,fixture_id", _COMBOS)
    def test_stream_to_internal(self, format_id: str, fixture_id: str) -> None:
        normalizer = format_conversion_registry.get_normalizer(format_id)
        assert normalizer is not None

        fixture = STREAM_FIXTURES[format_id][fixture_id]
        state = StreamState(model=fixture.chunks[0].get("model", ""))

        all_events = []
        for chunk in fixture.chunks:
            events = normalizer.stream_chunk_to_internal(chunk, state)
            all_events.extend(events)

        assert_stream_text_matches(all_events, fixture.expected_text)
        assert_stream_stop_reason_matches(all_events, fixture.expected_stop_reason)

        if fixture_id == "stream_tool_call":
            assert_stream_has_tool_call(all_events, "get_weather")


class TestStreamFromInternalSchema:
    """Verify internal events -> format-specific chunks conform to API schema."""

    @pytest.mark.parametrize("format_id,fixture_id", _COMBOS)
    def test_stream_roundtrip_schema(self, format_id: str, fixture_id: str) -> None:
        """Parse chunks -> internal events -> reconstruct chunks, validate schema."""
        validator = get_stream_chunk_validator(format_id)
        if validator is None:
            pytest.skip(f"No stream chunk schema validator for {format_id}")

        normalizer = format_conversion_registry.get_normalizer(format_id)
        assert normalizer is not None

        fixture = STREAM_FIXTURES[format_id][fixture_id]

        # Phase 1: chunks -> internal events
        in_state = StreamState(model=fixture.chunks[0].get("model", ""))
        all_events = []
        for chunk in fixture.chunks:
            events = normalizer.stream_chunk_to_internal(chunk, in_state)
            all_events.extend(events)

        # Phase 2: internal events -> output chunks, validate each
        out_state = StreamState(
            message_id=in_state.message_id or "chatcmpl-test",
            model=in_state.model or "test-model",
        )
        all_errors: list[str] = []
        for event in all_events:
            output_chunks = normalizer.stream_event_from_internal(event, out_state)
            for out_chunk in output_chunks:
                errors = validator(out_chunk)
                if errors:
                    all_errors.extend(f"[{type(event).__name__}] {e}" for e in errors)

        assert (
            not all_errors
        ), f"Stream schema validation failed for {format_id} ({fixture_id}):\n" + "\n".join(
            f"  - {e}" for e in all_errors
        )


def test_sync_response_stream_bridge_preserves_openai_reasoning_content() -> None:
    internal = InternalResponse(
        id="resp_reasoning",
        model="reasoner",
        content=[
            ThinkingBlock(thinking="hidden reasoning"),
            TextBlock(text="visible answer"),
        ],
        stop_reason=StopReason.END_TURN,
    )
    normalizer = OpenAINormalizer()
    state = StreamState(model="reasoner", message_id="resp_reasoning")

    chunks = []
    for event in iter_internal_response_as_stream_events(internal):
        chunks.extend(normalizer.stream_event_from_internal(event, state))

    reasoning = ""
    content = ""
    for chunk in chunks:
        choices = chunk.get("choices") or []
        if not choices:
            continue
        delta = choices[0].get("delta") or {}
        reasoning += delta.get("reasoning_content") or ""
        content += delta.get("content") or ""

    assert reasoning == "hidden reasoning"
    assert content == "visible answer"


def test_internal_stream_aggregator_preserves_thinking_block() -> None:
    aggregator = InternalStreamAggregator(fallback_id="resp_reasoning", fallback_model="reasoner")

    aggregator.feed(
        [
            MessageStartEvent(message_id="resp_reasoning", model="reasoner"),
            ContentBlockStartEvent(block_index=0, block_type=ContentType.THINKING),
            ContentDeltaEvent(block_index=0, text_delta="hidden reasoning"),
            ContentDeltaEvent(block_index=0, extra={"thought_signature": "sig_reasoning"}),
            ContentBlockStopEvent(block_index=0),
            ContentBlockStartEvent(block_index=1, block_type=ContentType.TEXT),
            ContentDeltaEvent(block_index=1, text_delta="visible answer"),
            ContentBlockStopEvent(block_index=1),
            MessageStopEvent(stop_reason=StopReason.END_TURN),
        ]
    )

    internal = aggregator.build()

    assert len(internal.content) == 2
    assert isinstance(internal.content[0], ThinkingBlock)
    assert internal.content[0].thinking == "hidden reasoning"
    assert internal.content[0].signature == "sig_reasoning"
    assert isinstance(internal.content[1], TextBlock)
    assert internal.content[1].text == "visible answer"
