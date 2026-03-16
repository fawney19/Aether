"""
Output schema validators for format-specific payloads.

Validates that normalizer output (request_from_internal / response_from_internal /
stream_event_from_internal) conforms to the target API's structural requirements:
required fields, correct types, and valid enum values.

This prevents regressions where a code change silently drops a required field
(e.g. missing "object" in response, or "index" leak in non-streaming tool_calls).
"""

from __future__ import annotations

from collections.abc import Callable
from typing import Any

ValidatorFunc = Callable[[dict[str, Any]], list[str]]

# ===================================================================
# OpenAI Chat Completions schema
# ===================================================================

_OPENAI_VALID_FINISH_REASONS = {"stop", "length", "tool_calls", "function_call", "content_filter"}
_OPENAI_VALID_ROLES = {"system", "developer", "user", "assistant", "tool"}
_OPENAI_VALID_CONTENT_PART_TYPES = {"text", "image_url", "file", "input_audio"}


def validate_openai_request(payload: dict[str, Any]) -> list[str]:
    """Validate OpenAI Chat Completions request structure. Returns list of errors."""
    errors: list[str] = []

    # Required top-level fields
    if "model" not in payload:
        errors.append("request: missing required field 'model'")
    elif not isinstance(payload["model"], str):
        errors.append(f"request: 'model' must be str, got {type(payload['model']).__name__}")

    if "messages" not in payload:
        errors.append("request: missing required field 'messages'")
    elif not isinstance(payload["messages"], list):
        errors.append("request: 'messages' must be list")
    else:
        for i, msg in enumerate(payload["messages"]):
            errors.extend(_validate_openai_request_message(msg, i))

    # Optional typed fields
    if "max_tokens" in payload and payload["max_tokens"] is not None:
        if not isinstance(payload["max_tokens"], int):
            errors.append(
                f"request: 'max_tokens' must be int, got {type(payload['max_tokens']).__name__}"
            )

    if "temperature" in payload and payload["temperature"] is not None:
        if not isinstance(payload["temperature"], (int, float)):
            errors.append("request: 'temperature' must be numeric")

    if "stream" in payload:
        if not isinstance(payload["stream"], bool):
            errors.append(f"request: 'stream' must be bool, got {type(payload['stream']).__name__}")
        if payload["stream"] and "stream_options" in payload:
            so = payload["stream_options"]
            if not isinstance(so, dict):
                errors.append("request: 'stream_options' must be dict")

    if "tools" in payload:
        if not isinstance(payload["tools"], list):
            errors.append("request: 'tools' must be list")
        else:
            for j, tool in enumerate(payload["tools"]):
                errors.extend(_validate_openai_tool_def(tool, j))

    if "tool_choice" in payload:
        tc = payload["tool_choice"]
        if isinstance(tc, str):
            if tc not in ("auto", "none", "required"):
                errors.append(f"request: 'tool_choice' invalid string value: {tc!r}")
        elif isinstance(tc, dict):
            if tc.get("type") != "function":
                errors.append(f"request: 'tool_choice' dict must have type='function'")
            fn = tc.get("function")
            if not isinstance(fn, dict) or "name" not in fn:
                errors.append("request: 'tool_choice.function' must have 'name'")
        else:
            errors.append(f"request: 'tool_choice' must be str or dict, got {type(tc).__name__}")

    if "response_format" in payload:
        rf = payload["response_format"]
        if not isinstance(rf, dict) or "type" not in rf:
            errors.append("request: 'response_format' must be dict with 'type'")

    return errors


def validate_openai_response(payload: dict[str, Any]) -> list[str]:
    """Validate OpenAI Chat Completions response structure. Returns list of errors."""
    errors: list[str] = []

    # Required top-level fields
    for field in ("id", "object", "model", "choices"):
        if field not in payload:
            errors.append(f"response: missing required field '{field}'")

    if payload.get("object") != "chat.completion":
        errors.append(
            f"response: 'object' must be 'chat.completion', got {payload.get('object')!r}"
        )

    if "created" in payload:
        if not isinstance(payload["created"], int):
            errors.append(
                f"response: 'created' must be int, got {type(payload.get('created')).__name__}"
            )

    choices = payload.get("choices")
    if isinstance(choices, list):
        for i, choice in enumerate(choices):
            errors.extend(_validate_openai_response_choice(choice, i))
    elif choices is not None:
        errors.append(f"response: 'choices' must be list, got {type(choices).__name__}")

    if "usage" in payload:
        errors.extend(_validate_openai_usage(payload["usage"]))

    return errors


def validate_openai_stream_chunk(payload: dict[str, Any]) -> list[str]:
    """Validate OpenAI streaming chunk structure. Returns list of errors."""
    errors: list[str] = []

    for field in ("id", "object", "choices"):
        if field not in payload:
            errors.append(f"chunk: missing required field '{field}'")

    if payload.get("object") != "chat.completion.chunk":
        errors.append(
            f"chunk: 'object' must be 'chat.completion.chunk', got {payload.get('object')!r}"
        )

    choices = payload.get("choices")
    if isinstance(choices, list):
        for i, choice in enumerate(choices):
            errors.extend(_validate_openai_stream_choice(choice, i))

    return errors


def validate_openai_error(payload: dict[str, Any]) -> list[str]:
    """Validate OpenAI error response structure. Returns list of errors."""
    errors: list[str] = []

    if "error" not in payload:
        errors.append("error response: missing 'error' field")
        return errors

    err = payload["error"]
    if not isinstance(err, dict):
        errors.append(f"error response: 'error' must be dict, got {type(err).__name__}")
        return errors

    if "message" not in err:
        errors.append("error: missing required field 'message'")
    if "type" not in err:
        errors.append("error: missing required field 'type'")

    return errors


# ===================================================================
# Internal validators
# ===================================================================


def _validate_openai_request_message(msg: Any, index: int) -> list[str]:
    errors: list[str] = []
    prefix = f"request.messages[{index}]"

    if not isinstance(msg, dict):
        return [f"{prefix}: must be dict, got {type(msg).__name__}"]

    role = msg.get("role")
    if role not in _OPENAI_VALID_ROLES:
        errors.append(f"{prefix}: invalid role {role!r}")

    content = msg.get("content")
    if content is not None:
        if isinstance(content, list):
            for j, part in enumerate(content):
                errors.extend(_validate_openai_content_part(part, f"{prefix}.content[{j}]"))
        elif not isinstance(content, str):
            errors.append(
                f"{prefix}: 'content' must be str, list, or null, got {type(content).__name__}"
            )

    if role == "assistant" and "tool_calls" in msg:
        tcs = msg["tool_calls"]
        if not isinstance(tcs, list):
            errors.append(f"{prefix}: 'tool_calls' must be list")
        else:
            for j, tc in enumerate(tcs):
                errors.extend(
                    _validate_openai_tool_call(tc, f"{prefix}.tool_calls[{j}]", streaming=False)
                )

    if role == "tool":
        if "tool_call_id" not in msg:
            errors.append(f"{prefix}: tool message missing 'tool_call_id'")

    return errors


def _validate_openai_content_part(part: Any, prefix: str) -> list[str]:
    errors: list[str] = []
    if not isinstance(part, dict):
        return [f"{prefix}: must be dict"]

    ptype = part.get("type")
    if ptype not in _OPENAI_VALID_CONTENT_PART_TYPES:
        errors.append(f"{prefix}: unknown type {ptype!r}")
        return errors

    if ptype == "text":
        if "text" not in part:
            errors.append(f"{prefix}: text part missing 'text' field")

    if ptype == "image_url":
        iu = part.get("image_url")
        if not isinstance(iu, dict) or "url" not in iu:
            errors.append(f"{prefix}: image_url part must have 'image_url.url'")

    if ptype == "file":
        f_obj = part.get("file")
        if not isinstance(f_obj, dict):
            errors.append(f"{prefix}: file part must have nested 'file' object")
        else:
            has_data = "file_data" in f_obj
            has_id = "file_id" in f_obj
            if not has_data and not has_id:
                errors.append(f"{prefix}: file part must have 'file.file_data' or 'file.file_id'")

    if ptype == "input_audio":
        ia = part.get("input_audio")
        if not isinstance(ia, dict):
            errors.append(f"{prefix}: input_audio part must have 'input_audio' object")
        else:
            if "data" not in ia:
                errors.append(f"{prefix}: input_audio missing 'data'")
            if "format" not in ia:
                errors.append(f"{prefix}: input_audio missing 'format'")

    return errors


def _validate_openai_tool_def(tool: Any, index: int) -> list[str]:
    errors: list[str] = []
    prefix = f"request.tools[{index}]"

    if not isinstance(tool, dict):
        return [f"{prefix}: must be dict"]

    if tool.get("type") != "function":
        errors.append(f"{prefix}: 'type' must be 'function', got {tool.get('type')!r}")

    fn = tool.get("function")
    if not isinstance(fn, dict):
        errors.append(f"{prefix}: missing 'function' object")
    else:
        if "name" not in fn:
            errors.append(f"{prefix}: function missing 'name'")

    return errors


def _validate_openai_tool_call(tc: Any, prefix: str, *, streaming: bool) -> list[str]:
    """Validate a single tool_call object.

    In non-streaming (message.tool_calls), 'index' must NOT be present.
    In streaming (delta.tool_calls), 'index' is required.
    """
    errors: list[str] = []
    if not isinstance(tc, dict):
        return [f"{prefix}: must be dict"]

    if streaming:
        if "index" not in tc:
            errors.append(f"{prefix}: streaming tool_call must have 'index'")
    else:
        if "index" in tc:
            errors.append(f"{prefix}: non-streaming tool_call must NOT have 'index'")
        if "id" not in tc:
            errors.append(f"{prefix}: tool_call missing 'id'")
        if tc.get("type") != "function":
            errors.append(f"{prefix}: 'type' must be 'function', got {tc.get('type')!r}")

    fn = tc.get("function")
    if isinstance(fn, dict):
        if not streaming and "name" not in fn:
            errors.append(f"{prefix}: function missing 'name'")
        if "arguments" in fn and not isinstance(fn["arguments"], str):
            errors.append(f"{prefix}: function.arguments must be str")

    return errors


def _validate_openai_response_choice(choice: Any, index: int) -> list[str]:
    errors: list[str] = []
    prefix = f"response.choices[{index}]"

    if not isinstance(choice, dict):
        return [f"{prefix}: must be dict"]

    if "index" not in choice:
        errors.append(f"{prefix}: missing 'index'")

    fr = choice.get("finish_reason")
    if fr is not None and fr not in _OPENAI_VALID_FINISH_REASONS:
        errors.append(f"{prefix}: invalid finish_reason {fr!r}")

    msg = choice.get("message")
    if msg is None:
        errors.append(f"{prefix}: missing 'message'")
    elif isinstance(msg, dict):
        if msg.get("role") != "assistant":
            errors.append(f"{prefix}: message.role must be 'assistant', got {msg.get('role')!r}")

        content = msg.get("content")
        if content is not None and not isinstance(content, (str, list)):
            errors.append(f"{prefix}: message.content must be str, list, or null")

        if "tool_calls" in msg:
            tcs = msg["tool_calls"]
            if not isinstance(tcs, list):
                errors.append(f"{prefix}: message.tool_calls must be list")
            else:
                for j, tc in enumerate(tcs):
                    errors.extend(
                        _validate_openai_tool_call(
                            tc, f"{prefix}.message.tool_calls[{j}]", streaming=False
                        )
                    )

    return errors


def _validate_openai_stream_choice(choice: Any, index: int) -> list[str]:
    errors: list[str] = []
    prefix = f"chunk.choices[{index}]"

    if not isinstance(choice, dict):
        return [f"{prefix}: must be dict"]

    if "index" not in choice:
        errors.append(f"{prefix}: missing 'index'")

    if "delta" not in choice:
        errors.append(f"{prefix}: missing 'delta'")
    elif not isinstance(choice["delta"], dict):
        errors.append(f"{prefix}: 'delta' must be dict")
    else:
        delta = choice["delta"]
        if "tool_calls" in delta:
            tcs = delta["tool_calls"]
            if isinstance(tcs, list):
                for j, tc in enumerate(tcs):
                    errors.extend(
                        _validate_openai_tool_call(
                            tc, f"{prefix}.delta.tool_calls[{j}]", streaming=True
                        )
                    )

    fr = choice.get("finish_reason")
    if fr is not None and fr not in _OPENAI_VALID_FINISH_REASONS:
        errors.append(f"{prefix}: invalid finish_reason {fr!r}")

    return errors


def _validate_openai_usage(usage: Any) -> list[str]:
    errors: list[str] = []
    if not isinstance(usage, dict):
        return [f"response.usage: must be dict, got {type(usage).__name__}"]

    for field in ("prompt_tokens", "completion_tokens", "total_tokens"):
        if field not in usage:
            errors.append(f"response.usage: missing required field '{field}'")
        elif not isinstance(usage[field], int):
            errors.append(f"response.usage: '{field}' must be int")

    return errors


# ===================================================================
# OpenAI CLI / Responses API schema
# ===================================================================

_OPENAI_CLI_VALID_INPUT_ITEM_TYPES = {
    "message",
    "function_call",
    "function_call_output",
    "reasoning",
}
_OPENAI_CLI_VALID_ROLES = {"user", "assistant", "system", "developer"}
_OPENAI_CLI_VALID_CONTENT_TYPES = {"input_text", "output_text", "input_image", "input_file"}
_OPENAI_CLI_VALID_OUTPUT_ITEM_TYPES = {"message", "function_call", "reasoning"}
_OPENAI_CLI_VALID_OUTPUT_CONTENT_TYPES = {"output_text", "refusal"}


def validate_openai_cli_request(payload: dict[str, Any]) -> list[str]:
    """Validate OpenAI Responses API request structure."""
    errors: list[str] = []

    if "model" not in payload:
        errors.append("request: missing required field 'model'")
    elif not isinstance(payload["model"], str):
        errors.append(f"request: 'model' must be str, got {type(payload['model']).__name__}")

    if "input" not in payload:
        errors.append("request: missing required field 'input'")
    else:
        inp = payload["input"]
        if isinstance(inp, list):
            for i, item in enumerate(inp):
                errors.extend(_validate_cli_input_item(item, i))
        elif not isinstance(inp, str):
            errors.append(f"request: 'input' must be str or list, got {type(inp).__name__}")

    if "instructions" in payload and payload["instructions"] is not None:
        if not isinstance(payload["instructions"], str):
            errors.append("request: 'instructions' must be str")

    if "max_output_tokens" in payload and payload["max_output_tokens"] is not None:
        if not isinstance(payload["max_output_tokens"], int):
            errors.append("request: 'max_output_tokens' must be int")

    if "stream" in payload:
        if not isinstance(payload["stream"], bool):
            errors.append("request: 'stream' must be bool")

    if "tools" in payload:
        if not isinstance(payload["tools"], list):
            errors.append("request: 'tools' must be list")
        else:
            for j, tool in enumerate(payload["tools"]):
                errors.extend(_validate_cli_tool_def(tool, j))

    if "tool_choice" in payload:
        tc = payload["tool_choice"]
        if isinstance(tc, str):
            if tc not in ("auto", "none", "required"):
                errors.append(f"request: invalid tool_choice string: {tc!r}")
        elif isinstance(tc, dict):
            tc_type = tc.get("type")
            if tc_type == "function":
                if "name" not in tc:
                    errors.append("request: tool_choice function must have 'name'")
        else:
            errors.append(f"request: 'tool_choice' must be str or dict, got {type(tc).__name__}")

    return errors


def validate_openai_cli_response(payload: dict[str, Any]) -> list[str]:
    """Validate OpenAI Responses API response structure."""
    errors: list[str] = []

    for field in ("id", "object", "model", "output", "status"):
        if field not in payload:
            errors.append(f"response: missing required field '{field}'")

    if payload.get("object") != "response":
        errors.append(f"response: 'object' must be 'response', got {payload.get('object')!r}")

    status = payload.get("status")
    if status is not None and status not in ("completed", "in_progress", "incomplete", "failed"):
        errors.append(f"response: invalid status {status!r}")

    output = payload.get("output")
    if isinstance(output, list):
        for i, item in enumerate(output):
            errors.extend(_validate_cli_output_item(item, i))
    elif output is not None:
        errors.append(f"response: 'output' must be list, got {type(output).__name__}")

    if "usage" in payload:
        errors.extend(_validate_cli_usage(payload["usage"]))

    return errors


def validate_openai_cli_stream_event(payload: dict[str, Any]) -> list[str]:
    """Validate OpenAI Responses API stream event structure."""
    errors: list[str] = []

    if "type" not in payload:
        errors.append("stream event: missing 'type'")
        return errors

    etype = payload["type"]
    if not isinstance(etype, str) or not etype.startswith("response."):
        errors.append(f"stream event: 'type' must start with 'response.', got {etype!r}")
        return errors

    # sequence_number is required for all events
    if "sequence_number" not in payload:
        errors.append(f"stream event [{etype}]: missing 'sequence_number'")

    # Validate specific event types
    if etype in ("response.created", "response.in_progress", "response.completed"):
        if "response" not in payload:
            errors.append(f"stream event [{etype}]: missing 'response' object")
        elif isinstance(payload["response"], dict):
            if payload["response"].get("object") != "response":
                errors.append(f"stream event [{etype}]: response.object must be 'response'")

    if etype == "response.output_item.added":
        if "output_index" not in payload:
            errors.append(f"stream event [{etype}]: missing 'output_index'")
        if "item" not in payload:
            errors.append(f"stream event [{etype}]: missing 'item'")

    if etype == "response.output_item.done":
        if "output_index" not in payload:
            errors.append(f"stream event [{etype}]: missing 'output_index'")
        if "item" not in payload:
            errors.append(f"stream event [{etype}]: missing 'item'")

    if etype == "response.output_text.delta":
        for req_field in ("item_id", "output_index", "content_index", "delta"):
            if req_field not in payload:
                errors.append(f"stream event [{etype}]: missing '{req_field}'")

    if etype == "response.output_text.done":
        for req_field in ("item_id", "output_index", "content_index", "text"):
            if req_field not in payload:
                errors.append(f"stream event [{etype}]: missing '{req_field}'")

    if etype == "response.content_part.added":
        for req_field in ("item_id", "output_index", "content_index", "part"):
            if req_field not in payload:
                errors.append(f"stream event [{etype}]: missing '{req_field}'")

    if etype == "response.content_part.done":
        for req_field in ("item_id", "output_index", "content_index", "part"):
            if req_field not in payload:
                errors.append(f"stream event [{etype}]: missing '{req_field}'")

    if etype == "response.function_call_arguments.delta":
        for req_field in ("item_id", "output_index", "delta"):
            if req_field not in payload:
                errors.append(f"stream event [{etype}]: missing '{req_field}'")

    if etype == "response.function_call_arguments.done":
        for req_field in ("item_id", "output_index", "arguments"):
            if req_field not in payload:
                errors.append(f"stream event [{etype}]: missing '{req_field}'")

    return errors


def validate_openai_cli_error(payload: dict[str, Any]) -> list[str]:
    """Validate OpenAI Responses API error structure."""
    errors: list[str] = []

    if "error" not in payload:
        errors.append("error response: missing 'error' field")
        return errors

    err = payload["error"]
    if not isinstance(err, dict):
        errors.append(f"error response: 'error' must be dict, got {type(err).__name__}")
        return errors

    if "message" not in err:
        errors.append("error: missing required field 'message'")
    if "type" not in err:
        errors.append("error: missing required field 'type'")

    return errors


# --- Internal validators for CLI ---


def _validate_cli_input_item(item: Any, index: int) -> list[str]:
    errors: list[str] = []
    prefix = f"request.input[{index}]"

    if not isinstance(item, dict):
        return [f"{prefix}: must be dict"]

    item_type = item.get("type")
    if item_type == "message":
        role = item.get("role")
        if role not in _OPENAI_CLI_VALID_ROLES:
            errors.append(f"{prefix}: invalid role {role!r}")
        content = item.get("content")
        if isinstance(content, list):
            for j, part in enumerate(content):
                errors.extend(_validate_cli_content_part(part, f"{prefix}.content[{j}]"))
        elif content is not None and not isinstance(content, str):
            errors.append(f"{prefix}: 'content' must be str, list, or null")
    elif item_type == "function_call":
        if "call_id" not in item and "id" not in item:
            errors.append(f"{prefix}: function_call must have 'call_id' or 'id'")
        if "name" not in item:
            errors.append(f"{prefix}: function_call must have 'name'")
        args = item.get("arguments")
        if args is not None and not isinstance(args, str):
            errors.append(f"{prefix}: function_call.arguments must be str")
    elif item_type == "function_call_output":
        if "call_id" not in item and "id" not in item:
            errors.append(f"{prefix}: function_call_output must have 'call_id' or 'id'")
        if "output" not in item:
            errors.append(f"{prefix}: function_call_output must have 'output'")
        elif not isinstance(item["output"], str):
            errors.append(f"{prefix}: function_call_output.output must be str")
    elif item_type == "reasoning":
        pass  # reasoning items are opaque
    elif item_type is not None:
        pass  # unknown types are allowed (forward compat)
    else:
        errors.append(f"{prefix}: missing 'type'")

    return errors


def _validate_cli_content_part(part: Any, prefix: str) -> list[str]:
    errors: list[str] = []
    if not isinstance(part, dict):
        return [f"{prefix}: must be dict"]

    ptype = part.get("type")
    if ptype == "input_text":
        if "text" not in part:
            errors.append(f"{prefix}: input_text missing 'text'")
    elif ptype == "output_text":
        if "text" not in part:
            errors.append(f"{prefix}: output_text missing 'text'")
    elif ptype == "input_image":
        if "image_url" not in part and "file_id" not in part:
            errors.append(f"{prefix}: input_image must have 'image_url' or 'file_id'")
    elif ptype == "input_file":
        if "file_data" not in part and "file_id" not in part:
            errors.append(f"{prefix}: input_file must have 'file_data' or 'file_id'")
    elif ptype is not None:
        pass  # unknown types allowed
    else:
        errors.append(f"{prefix}: missing 'type'")

    return errors


def _validate_cli_tool_def(tool: Any, index: int) -> list[str]:
    errors: list[str] = []
    prefix = f"request.tools[{index}]"

    if not isinstance(tool, dict):
        return [f"{prefix}: must be dict"]

    tool_type = tool.get("type")
    if tool_type == "function":
        if "name" not in tool:
            errors.append(f"{prefix}: function tool must have 'name'")
    # file_search, web_search_preview, computer_use_preview are also valid
    elif tool_type in (
        "file_search",
        "web_search_preview",
        "web_search_preview_2025_03_11",
        "computer_use_preview",
    ):
        pass
    elif tool_type is not None:
        pass  # unknown tool types allowed
    else:
        errors.append(f"{prefix}: missing 'type'")

    return errors


def _validate_cli_output_item(item: Any, index: int) -> list[str]:
    errors: list[str] = []
    prefix = f"response.output[{index}]"

    if not isinstance(item, dict):
        return [f"{prefix}: must be dict"]

    item_type = item.get("type")
    if item_type == "message":
        if item.get("role") != "assistant":
            errors.append(f"{prefix}: message.role must be 'assistant'")
        content = item.get("content")
        if isinstance(content, list):
            for j, part in enumerate(content):
                if isinstance(part, dict):
                    ptype = part.get("type")
                    if ptype == "output_text":
                        if "text" not in part:
                            errors.append(f"{prefix}.content[{j}]: output_text missing 'text'")
                    elif ptype == "refusal":
                        if "refusal" not in part:
                            errors.append(f"{prefix}.content[{j}]: refusal missing 'refusal'")
    elif item_type == "function_call":
        if "call_id" not in item and "id" not in item:
            errors.append(f"{prefix}: function_call must have 'call_id' or 'id'")
        if "name" not in item:
            errors.append(f"{prefix}: function_call must have 'name'")
        args = item.get("arguments")
        if args is not None and not isinstance(args, str):
            errors.append(f"{prefix}: function_call.arguments must be str")
    elif item_type == "reasoning":
        summary = item.get("summary")
        if summary is not None and not isinstance(summary, list):
            errors.append(f"{prefix}: reasoning.summary must be list")
    elif item_type is not None:
        pass  # unknown types allowed
    else:
        errors.append(f"{prefix}: missing 'type'")

    return errors


def _validate_cli_usage(usage: Any) -> list[str]:
    errors: list[str] = []
    if not isinstance(usage, dict):
        return [f"response.usage: must be dict, got {type(usage).__name__}"]

    for field in ("input_tokens", "output_tokens", "total_tokens"):
        if field not in usage:
            errors.append(f"response.usage: missing required field '{field}'")
        elif not isinstance(usage[field], int):
            errors.append(f"response.usage: '{field}' must be int")

    return errors


# ===================================================================
# Claude Messages API schema
# ===================================================================

_CLAUDE_VALID_ROLES = {"user", "assistant"}
_CLAUDE_VALID_STOP_REASONS = {"end_turn", "max_tokens", "stop_sequence", "tool_use"}
_CLAUDE_VALID_CONTENT_BLOCK_TYPES = {
    "text",
    "image",
    "document",
    "tool_use",
    "tool_result",
    "thinking",
}
_CLAUDE_VALID_STREAM_EVENT_TYPES = {
    "message_start",
    "content_block_start",
    "content_block_delta",
    "content_block_stop",
    "message_delta",
    "message_stop",
    "ping",
    "error",
}


def validate_claude_request(payload: dict[str, Any]) -> list[str]:
    """Validate Claude Messages API request structure."""
    errors: list[str] = []

    if "model" not in payload:
        errors.append("request: missing required field 'model'")
    elif not isinstance(payload["model"], str):
        errors.append(f"request: 'model' must be str, got {type(payload['model']).__name__}")

    if "max_tokens" not in payload:
        errors.append("request: missing required field 'max_tokens'")
    elif not isinstance(payload["max_tokens"], int):
        errors.append(
            f"request: 'max_tokens' must be int, got {type(payload['max_tokens']).__name__}"
        )

    if "messages" not in payload:
        errors.append("request: missing required field 'messages'")
    elif not isinstance(payload["messages"], list):
        errors.append("request: 'messages' must be list")
    else:
        for i, msg in enumerate(payload["messages"]):
            errors.extend(_validate_claude_request_message(msg, i))

    # system can be string or list of content blocks
    if "system" in payload:
        sys_val = payload["system"]
        if sys_val is not None and not isinstance(sys_val, (str, list)):
            errors.append(f"request: 'system' must be str or list, got {type(sys_val).__name__}")
        if isinstance(sys_val, list):
            for j, block in enumerate(sys_val):
                if isinstance(block, dict):
                    btype = block.get("type")
                    if btype == "text" and "text" not in block:
                        errors.append(f"request.system[{j}]: text block missing 'text'")

    if "temperature" in payload and payload["temperature"] is not None:
        if not isinstance(payload["temperature"], (int, float)):
            errors.append("request: 'temperature' must be numeric")

    if "stream" in payload:
        if not isinstance(payload["stream"], bool):
            errors.append(f"request: 'stream' must be bool, got {type(payload['stream']).__name__}")

    if "tools" in payload:
        if not isinstance(payload["tools"], list):
            errors.append("request: 'tools' must be list")
        else:
            for j, tool in enumerate(payload["tools"]):
                errors.extend(_validate_claude_tool_def(tool, j))

    if "tool_choice" in payload:
        tc = payload["tool_choice"]
        if isinstance(tc, dict):
            tc_type = tc.get("type")
            if tc_type not in ("auto", "any", "tool", "none"):
                errors.append(f"request: invalid tool_choice.type: {tc_type!r}")
            if tc_type == "tool" and "name" not in tc:
                errors.append("request: tool_choice type='tool' must have 'name'")
        else:
            errors.append(f"request: 'tool_choice' must be dict, got {type(tc).__name__}")

    if "thinking" in payload:
        th = payload["thinking"]
        if isinstance(th, dict):
            if "type" not in th:
                errors.append("request: thinking must have 'type'")
            if th.get("type") == "enabled" and "budget_tokens" not in th:
                errors.append("request: thinking type='enabled' must have 'budget_tokens'")

    return errors


def validate_claude_response(payload: dict[str, Any]) -> list[str]:
    """Validate Claude Messages API response structure."""
    errors: list[str] = []

    for field in ("id", "type", "role", "model", "content", "stop_reason", "usage"):
        if field not in payload:
            errors.append(f"response: missing required field '{field}'")

    if payload.get("type") != "message":
        errors.append(f"response: 'type' must be 'message', got {payload.get('type')!r}")

    if payload.get("role") != "assistant":
        errors.append(f"response: 'role' must be 'assistant', got {payload.get('role')!r}")

    sr = payload.get("stop_reason")
    if sr is not None and sr not in _CLAUDE_VALID_STOP_REASONS:
        errors.append(f"response: invalid stop_reason {sr!r}")

    content = payload.get("content")
    if isinstance(content, list):
        for i, block in enumerate(content):
            errors.extend(_validate_claude_content_block(block, f"response.content[{i}]"))
    elif content is not None:
        errors.append(f"response: 'content' must be list, got {type(content).__name__}")

    if "usage" in payload:
        errors.extend(_validate_claude_usage(payload["usage"]))

    return errors


def validate_claude_stream_chunk(payload: dict[str, Any]) -> list[str]:
    """Validate Claude Messages API stream event structure."""
    errors: list[str] = []

    if "type" not in payload:
        errors.append("stream event: missing 'type'")
        return errors

    etype = payload["type"]
    if etype not in _CLAUDE_VALID_STREAM_EVENT_TYPES:
        errors.append(f"stream event: unknown type {etype!r}")
        return errors

    if etype == "message_start":
        if "message" not in payload:
            errors.append("stream event [message_start]: missing 'message'")
        elif isinstance(payload["message"], dict):
            msg = payload["message"]
            if msg.get("type") != "message":
                errors.append("stream event [message_start]: message.type must be 'message'")
            if msg.get("role") != "assistant":
                errors.append("stream event [message_start]: message.role must be 'assistant'")

    if etype == "content_block_start":
        if "index" not in payload:
            errors.append("stream event [content_block_start]: missing 'index'")
        if "content_block" not in payload:
            errors.append("stream event [content_block_start]: missing 'content_block'")
        elif isinstance(payload["content_block"], dict):
            if "type" not in payload["content_block"]:
                errors.append("stream event [content_block_start]: content_block missing 'type'")

    if etype == "content_block_delta":
        if "index" not in payload:
            errors.append("stream event [content_block_delta]: missing 'index'")
        if "delta" not in payload:
            errors.append("stream event [content_block_delta]: missing 'delta'")
        elif isinstance(payload["delta"], dict):
            if "type" not in payload["delta"]:
                errors.append("stream event [content_block_delta]: delta missing 'type'")

    if etype == "content_block_stop":
        if "index" not in payload:
            errors.append("stream event [content_block_stop]: missing 'index'")

    if etype == "message_delta":
        if "delta" not in payload:
            errors.append("stream event [message_delta]: missing 'delta'")

    return errors


def validate_claude_error(payload: dict[str, Any]) -> list[str]:
    """Validate Claude Messages API error structure."""
    errors: list[str] = []

    if payload.get("type") != "error":
        errors.append(f"error response: 'type' must be 'error', got {payload.get('type')!r}")

    if "error" not in payload:
        errors.append("error response: missing 'error' field")
        return errors

    err = payload["error"]
    if not isinstance(err, dict):
        errors.append(f"error response: 'error' must be dict, got {type(err).__name__}")
        return errors

    if "type" not in err:
        errors.append("error: missing required field 'type'")
    if "message" not in err:
        errors.append("error: missing required field 'message'")

    return errors


# --- Internal validators for Claude ---


def _validate_claude_request_message(msg: Any, index: int) -> list[str]:
    errors: list[str] = []
    prefix = f"request.messages[{index}]"

    if not isinstance(msg, dict):
        return [f"{prefix}: must be dict, got {type(msg).__name__}"]

    role = msg.get("role")
    if role not in _CLAUDE_VALID_ROLES:
        errors.append(f"{prefix}: invalid role {role!r}")

    content = msg.get("content")
    if content is None:
        errors.append(f"{prefix}: missing 'content'")
    elif isinstance(content, list):
        for j, block in enumerate(content):
            errors.extend(_validate_claude_content_block(block, f"{prefix}.content[{j}]"))
    elif not isinstance(content, str):
        errors.append(f"{prefix}: 'content' must be str or list, got {type(content).__name__}")

    return errors


def _validate_claude_content_block(block: Any, prefix: str) -> list[str]:
    errors: list[str] = []
    if not isinstance(block, dict):
        return [f"{prefix}: must be dict"]

    btype = block.get("type")
    if btype is None:
        errors.append(f"{prefix}: missing 'type'")
        return errors

    if btype == "text":
        if "text" not in block:
            errors.append(f"{prefix}: text block missing 'text'")
    elif btype == "image":
        source = block.get("source")
        if not isinstance(source, dict):
            errors.append(f"{prefix}: image block must have 'source' dict")
        else:
            if "type" not in source:
                errors.append(f"{prefix}: image source missing 'type'")
            if source.get("type") == "base64":
                if "media_type" not in source:
                    errors.append(f"{prefix}: base64 image source missing 'media_type'")
                if "data" not in source:
                    errors.append(f"{prefix}: base64 image source missing 'data'")
            elif source.get("type") == "url":
                if "url" not in source:
                    errors.append(f"{prefix}: url image source missing 'url'")
    elif btype == "document":
        source = block.get("source")
        if not isinstance(source, dict):
            errors.append(f"{prefix}: document block must have 'source' dict")
    elif btype == "tool_use":
        if "id" not in block:
            errors.append(f"{prefix}: tool_use missing 'id'")
        if "name" not in block:
            errors.append(f"{prefix}: tool_use missing 'name'")
        if "input" not in block:
            errors.append(f"{prefix}: tool_use missing 'input'")
    elif btype == "tool_result":
        if "tool_use_id" not in block:
            errors.append(f"{prefix}: tool_result missing 'tool_use_id'")
    elif btype == "thinking":
        if "thinking" not in block:
            errors.append(f"{prefix}: thinking block missing 'thinking'")

    return errors


def _validate_claude_tool_def(tool: Any, index: int) -> list[str]:
    errors: list[str] = []
    prefix = f"request.tools[{index}]"

    if not isinstance(tool, dict):
        return [f"{prefix}: must be dict"]

    if "name" not in tool:
        errors.append(f"{prefix}: tool missing 'name'")
    tool_type = tool.get("type")
    if isinstance(tool_type, str) and tool_type.startswith("web_search_"):
        max_uses = tool.get("max_uses")
        if max_uses is not None and not isinstance(max_uses, int):
            errors.append(f"{prefix}: web_search 'max_uses' must be int")
        user_location = tool.get("user_location")
        if user_location is not None and not isinstance(user_location, dict):
            errors.append(f"{prefix}: web_search 'user_location' must be dict")
        return errors

    if "input_schema" not in tool:
        errors.append(f"{prefix}: tool missing 'input_schema'")

    return errors


def _validate_claude_usage(usage: Any) -> list[str]:
    errors: list[str] = []
    if not isinstance(usage, dict):
        return [f"response.usage: must be dict, got {type(usage).__name__}"]

    for field in ("input_tokens", "output_tokens"):
        if field not in usage:
            errors.append(f"response.usage: missing required field '{field}'")
        elif not isinstance(usage[field], int):
            errors.append(f"response.usage: '{field}' must be int")

    return errors


# ===================================================================
# Gemini GenerateContent API schema
# ===================================================================

_GEMINI_VALID_ROLES = {"user", "model"}
_GEMINI_VALID_FINISH_REASONS = {
    "STOP",
    "MAX_TOKENS",
    "SAFETY",
    "RECITATION",
    "LANGUAGE",
    "BLOCKLIST",
    "PROHIBITED_CONTENT",
    "SPII",
    "IMAGE_SAFETY",
    "MALFORMED_FUNCTION_CALL",
    "OTHER",
}
_GEMINI_VALID_PART_DATA_FIELDS = {
    "text",
    "inlineData",
    "inline_data",
    "fileData",
    "file_data",
    "functionCall",
    "function_call",
    "functionResponse",
    "function_response",
    "executableCode",
    "executable_code",
    "codeExecutionResult",
    "code_execution_result",
}


def validate_gemini_request(payload: dict[str, Any]) -> list[str]:
    """Validate Gemini GenerateContent request structure."""
    errors: list[str] = []

    if "contents" not in payload:
        errors.append("request: missing required field 'contents'")
    elif not isinstance(payload["contents"], list):
        errors.append("request: 'contents' must be list")
    else:
        for i, content in enumerate(payload["contents"]):
            errors.extend(_validate_gemini_content(content, f"request.contents[{i}]"))

    si = payload.get("system_instruction") or payload.get("systemInstruction")
    if si is not None:
        if not isinstance(si, dict):
            errors.append("request: 'systemInstruction' must be dict")
        elif "parts" not in si:
            errors.append("request: 'systemInstruction' must have 'parts'")

    gc = payload.get("generation_config") or payload.get("generationConfig")
    if gc is not None:
        if not isinstance(gc, dict):
            errors.append("request: 'generationConfig' must be dict")
        else:
            errors.extend(_validate_gemini_generation_config(gc))

    tools = payload.get("tools")
    if tools is not None:
        if not isinstance(tools, list):
            errors.append("request: 'tools' must be list")
        else:
            for j, tool in enumerate(tools):
                errors.extend(_validate_gemini_tool(tool, j))

    tc = payload.get("tool_config") or payload.get("toolConfig")
    if tc is not None:
        if not isinstance(tc, dict):
            errors.append("request: 'toolConfig' must be dict")
        else:
            fcc = tc.get("function_calling_config") or tc.get("functionCallingConfig")
            if fcc is not None and not isinstance(fcc, dict):
                errors.append("request: 'functionCallingConfig' must be dict")

    return errors


def validate_gemini_response(payload: dict[str, Any]) -> list[str]:
    """Validate Gemini GenerateContent response structure."""
    errors: list[str] = []

    if "candidates" not in payload:
        errors.append("response: missing required field 'candidates'")
    elif not isinstance(payload["candidates"], list):
        errors.append("response: 'candidates' must be list")
    else:
        for i, cand in enumerate(payload["candidates"]):
            errors.extend(_validate_gemini_candidate(cand, i))

    if "usageMetadata" in payload:
        errors.extend(_validate_gemini_usage(payload["usageMetadata"]))

    return errors


def validate_gemini_stream_chunk(payload: dict[str, Any]) -> list[str]:
    """Validate Gemini streaming chunk structure."""
    errors: list[str] = []

    # Gemini stream chunks are GenerateContentResponse objects
    if "candidates" in payload:
        candidates = payload["candidates"]
        if not isinstance(candidates, list):
            errors.append("chunk: 'candidates' must be list")
        else:
            for i, cand in enumerate(candidates):
                if not isinstance(cand, dict):
                    errors.append(f"chunk.candidates[{i}]: must be dict")
                    continue
                content = cand.get("content")
                if content is not None:
                    if not isinstance(content, dict):
                        errors.append(f"chunk.candidates[{i}]: 'content' must be dict")
                    elif "parts" in content and not isinstance(content["parts"], list):
                        errors.append(f"chunk.candidates[{i}]: 'content.parts' must be list")
                fr = cand.get("finishReason")
                if fr is not None and fr not in _GEMINI_VALID_FINISH_REASONS:
                    errors.append(f"chunk.candidates[{i}]: invalid finishReason {fr!r}")

    if "usageMetadata" in payload:
        errors.extend(_validate_gemini_usage(payload["usageMetadata"]))

    return errors


def validate_gemini_error(payload: dict[str, Any]) -> list[str]:
    """Validate Gemini error response structure."""
    errors: list[str] = []

    if "error" not in payload:
        errors.append("error response: missing 'error' field")
        return errors

    err = payload["error"]
    if not isinstance(err, dict):
        errors.append(f"error response: 'error' must be dict, got {type(err).__name__}")
        return errors

    if "message" not in err:
        errors.append("error: missing required field 'message'")
    if "status" not in err:
        errors.append("error: missing required field 'status'")

    return errors


# --- Internal validators for Gemini ---


def _validate_gemini_content(content: Any, prefix: str) -> list[str]:
    errors: list[str] = []
    if not isinstance(content, dict):
        return [f"{prefix}: must be dict"]

    role = content.get("role")
    if role not in _GEMINI_VALID_ROLES:
        errors.append(f"{prefix}: invalid role {role!r}")

    parts = content.get("parts")
    if parts is None:
        errors.append(f"{prefix}: missing 'parts'")
    elif not isinstance(parts, list):
        errors.append(f"{prefix}: 'parts' must be list")
    else:
        for j, part in enumerate(parts):
            errors.extend(_validate_gemini_part(part, f"{prefix}.parts[{j}]"))

    return errors


def _validate_gemini_part(part: Any, prefix: str) -> list[str]:
    errors: list[str] = []
    if not isinstance(part, dict):
        return [f"{prefix}: must be dict"]

    # A part must have at least one data field
    has_data = bool(part.keys() & _GEMINI_VALID_PART_DATA_FIELDS)
    if not has_data:
        # thought parts have text + thought=True, which is valid
        if "thought" not in part:
            errors.append(f"{prefix}: no recognised data field")

    # Validate specific part types
    inline = part.get("inlineData") or part.get("inline_data")
    if isinstance(inline, dict):
        mime = inline.get("mimeType") or inline.get("mime_type")
        if not mime:
            errors.append(f"{prefix}: inlineData missing 'mimeType'")
        if "data" not in inline:
            errors.append(f"{prefix}: inlineData missing 'data'")

    file_data = part.get("fileData") or part.get("file_data")
    if isinstance(file_data, dict):
        if not (file_data.get("fileUri") or file_data.get("file_uri")):
            errors.append(f"{prefix}: fileData missing 'fileUri'")

    fc = part.get("functionCall") or part.get("function_call")
    if isinstance(fc, dict):
        if "name" not in fc:
            errors.append(f"{prefix}: functionCall missing 'name'")

    fr = part.get("functionResponse") or part.get("function_response")
    if isinstance(fr, dict):
        if "name" not in fr:
            errors.append(f"{prefix}: functionResponse missing 'name'")

    return errors


def _validate_gemini_candidate(cand: Any, index: int) -> list[str]:
    errors: list[str] = []
    prefix = f"response.candidates[{index}]"

    if not isinstance(cand, dict):
        return [f"{prefix}: must be dict"]

    content = cand.get("content")
    if content is not None:
        if not isinstance(content, dict):
            errors.append(f"{prefix}: 'content' must be dict")
        else:
            if "parts" not in content:
                errors.append(f"{prefix}: content missing 'parts'")
            elif not isinstance(content["parts"], list):
                errors.append(f"{prefix}: content.parts must be list")
            role = content.get("role")
            if role is not None and role != "model":
                errors.append(f"{prefix}: content.role should be 'model', got {role!r}")

    fr = cand.get("finishReason")
    if fr is not None and fr not in _GEMINI_VALID_FINISH_REASONS:
        errors.append(f"{prefix}: invalid finishReason {fr!r}")

    return errors


def _validate_gemini_generation_config(gc: dict[str, Any]) -> list[str]:
    errors: list[str] = []

    mot = gc.get("max_output_tokens") or gc.get("maxOutputTokens")
    if mot is not None and not isinstance(mot, int):
        errors.append("generationConfig: 'maxOutputTokens' must be int")

    temp = gc.get("temperature")
    if temp is not None and not isinstance(temp, (int, float)):
        errors.append("generationConfig: 'temperature' must be numeric")

    return errors


def _validate_gemini_tool(tool: Any, index: int) -> list[str]:
    errors: list[str] = []
    prefix = f"request.tools[{index}]"

    if not isinstance(tool, dict):
        return [f"{prefix}: must be dict"]

    decls = tool.get("functionDeclarations") or tool.get("function_declarations")
    if decls is not None:
        if not isinstance(decls, list):
            errors.append(f"{prefix}: 'functionDeclarations' must be list")
        else:
            for j, decl in enumerate(decls):
                if not isinstance(decl, dict):
                    errors.append(f"{prefix}.functionDeclarations[{j}]: must be dict")
                elif "name" not in decl:
                    errors.append(f"{prefix}.functionDeclarations[{j}]: missing 'name'")

    return errors


def _validate_gemini_usage(usage: Any) -> list[str]:
    errors: list[str] = []
    if not isinstance(usage, dict):
        return [f"usageMetadata: must be dict, got {type(usage).__name__}"]

    for field in ("promptTokenCount", "candidatesTokenCount", "totalTokenCount"):
        val = usage.get(field)
        if val is not None and not isinstance(val, int):
            errors.append(f"usageMetadata: '{field}' must be int")

    return errors


_REQUEST_VALIDATORS: dict[str, Any] = {
    "openai:chat": validate_openai_request,
    "openai:cli": validate_openai_cli_request,
    "claude:chat": validate_claude_request,
    "claude:cli": validate_claude_request,
    "gemini:chat": validate_gemini_request,
    "gemini:cli": validate_gemini_request,
}

_RESPONSE_VALIDATORS: dict[str, Any] = {
    "openai:chat": validate_openai_response,
    "openai:cli": validate_openai_cli_response,
    "claude:chat": validate_claude_response,
    "claude:cli": validate_claude_response,
    "gemini:chat": validate_gemini_response,
    "gemini:cli": validate_gemini_response,
}

_STREAM_CHUNK_VALIDATORS: dict[str, Any] = {
    "openai:chat": validate_openai_stream_chunk,
    "openai:cli": validate_openai_cli_stream_event,
    "claude:chat": validate_claude_stream_chunk,
    "claude:cli": validate_claude_stream_chunk,
    "gemini:chat": validate_gemini_stream_chunk,
    "gemini:cli": validate_gemini_stream_chunk,
}

_ERROR_VALIDATORS: dict[str, Any] = {
    "openai:chat": validate_openai_error,
    "openai:cli": validate_openai_cli_error,
    "claude:chat": validate_claude_error,
    "claude:cli": validate_claude_error,
    "gemini:chat": validate_gemini_error,
    "gemini:cli": validate_gemini_error,
}


def get_request_validator(format_id: str) -> ValidatorFunc | None:
    return _REQUEST_VALIDATORS.get(format_id)


def get_response_validator(format_id: str) -> ValidatorFunc | None:
    return _RESPONSE_VALIDATORS.get(format_id)


def get_stream_chunk_validator(format_id: str) -> ValidatorFunc | None:
    return _STREAM_CHUNK_VALIDATORS.get(format_id)


def get_error_validator(format_id: str) -> ValidatorFunc | None:
    return _ERROR_VALIDATORS.get(format_id)
