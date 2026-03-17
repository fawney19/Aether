from __future__ import annotations

import copy
from typing import Any

from src.core.api_format.conversion.internal import (
    FormatCapabilities,
    InternalMessage,
    InternalRequest,
    InternalResponse,
    Role,
    TextBlock,
)
from src.core.api_format.conversion.normalizer import FormatNormalizer
from src.core.api_format.conversion.registry import FormatConversionRegistry


class _BaseTestNormalizer(FormatNormalizer):
    capabilities = FormatCapabilities()

    def response_to_internal(self, response: dict[str, Any]) -> InternalResponse:
        return InternalResponse(id=str(response.get("id") or ""), model="", content=[])

    def response_from_internal(
        self,
        internal: InternalResponse,
        *,
        requested_model: str | None = None,
    ) -> dict[str, Any]:
        return {
            "id": internal.id,
            "model": requested_model or internal.model,
        }


class _SameFormatNormalizer(_BaseTestNormalizer):
    FORMAT_ID = "TEST:SAME"

    def request_to_internal(self, request: dict[str, Any]) -> InternalRequest:
        return InternalRequest(model=str(request.get("model") or ""), messages=[])

    def request_from_internal(
        self,
        internal: InternalRequest,
        *,
        target_variant: str | None = None,
    ) -> dict[str, Any]:
        return {"model": internal.model, "variant": target_variant}


class _MutatingSourceNormalizer(_BaseTestNormalizer):
    FORMAT_ID = "TEST:MUTSRC"

    def request_to_internal(self, request: dict[str, Any]) -> InternalRequest:
        request.pop("ephemeral", None)
        request["messages"][0]["content"][0]["text"] = "mutated"
        return InternalRequest(
            model=str(request.get("model") or ""),
            messages=[
                InternalMessage(
                    role=Role.USER,
                    content=[TextBlock(text=str(request["messages"][0]["content"][0]["text"]))],
                )
            ],
        )

    def request_from_internal(
        self,
        internal: InternalRequest,
        *,
        target_variant: str | None = None,
    ) -> dict[str, Any]:
        return {"model": internal.model, "variant": target_variant}


class _TargetNormalizer(_BaseTestNormalizer):
    FORMAT_ID = "TEST:MUTTGT"

    def request_to_internal(self, request: dict[str, Any]) -> InternalRequest:
        return InternalRequest(model=str(request.get("model") or ""), messages=[])

    def request_from_internal(
        self,
        internal: InternalRequest,
        *,
        target_variant: str | None = None,
    ) -> dict[str, Any]:
        text = ""
        if internal.messages and internal.messages[0].content:
            first = internal.messages[0].content[0]
            if isinstance(first, TextBlock):
                text = first.text
        return {"model": internal.model, "text": text, "variant": target_variant}


def test_convert_request_same_format_returns_detached_copy() -> None:
    registry = FormatConversionRegistry()
    registry.register(_SameFormatNormalizer())

    original = {
        "model": "gpt-test",
        "messages": [{"role": "user", "content": [{"type": "text", "text": "hello"}]}],
    }

    out = registry.convert_request(original, "test:same", "test:same")

    assert out == original
    assert out is not original
    assert out["messages"] is not original["messages"]

    out["messages"][0]["content"][0]["text"] = "changed"

    assert original["messages"][0]["content"][0]["text"] == "hello"


def test_convert_request_cross_format_does_not_mutate_original_input() -> None:
    registry = FormatConversionRegistry()
    registry.register(_MutatingSourceNormalizer())
    registry.register(_TargetNormalizer())

    original = {
        "model": "gpt-test",
        "ephemeral": "keep-me",
        "messages": [{"role": "user", "content": [{"type": "text", "text": "hello"}]}],
    }
    snapshot = copy.deepcopy(original)

    out = registry.convert_request(original, "test:mutsrc", "test:muttgt")

    assert out["model"] == "gpt-test"
    assert out["text"] == "mutated"
    assert original == snapshot
