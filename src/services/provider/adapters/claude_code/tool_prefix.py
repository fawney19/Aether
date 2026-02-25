"""Claude Code tool-name prefixing (request ↔ response)."""

from __future__ import annotations

import copy
from typing import Any

CLAUDE_TOOL_PREFIX = "proxy_"

_BUILTIN_TOOL_NAMES: frozenset[str] = frozenset(
    {
        "web_search",
        "code_execution",
        "text_editor",
        "computer",
    }
)


def _collect_builtin_tool_names(payload: dict[str, Any]) -> set[str]:
    names = set(_BUILTIN_TOOL_NAMES)
    tools = payload.get("tools")
    if not isinstance(tools, list):
        return names
    for tool in tools:
        if not isinstance(tool, dict):
            continue
        tool_name = tool.get("name")
        tool_type = str(tool.get("type") or "").strip()
        if isinstance(tool_name, str) and tool_name.strip() and tool_type:
            names.add(tool_name.strip())
    return names


def _prefix_tool_field(
    container: dict[str, Any],
    field: str,
    *,
    prefix: str,
    builtin_names: set[str],
) -> None:
    value = container.get(field)
    if not isinstance(value, str):
        return
    if not value or value.startswith(prefix) or value in builtin_names:
        return
    container[field] = f"{prefix}{value}"


def apply_claude_tool_prefix(
    request_body: dict[str, Any],
    *,
    prefix: str = CLAUDE_TOOL_PREFIX,
) -> dict[str, Any]:
    """Add *prefix* to every custom tool name in *request_body* (deep-copy)."""
    if not prefix:
        return request_body

    copied = copy.deepcopy(request_body)
    builtin_names = _collect_builtin_tool_names(copied)

    tools = copied.get("tools")
    if isinstance(tools, list):
        for tool in tools:
            if not isinstance(tool, dict):
                continue
            if str(tool.get("type") or "").strip():
                continue
            _prefix_tool_field(tool, "name", prefix=prefix, builtin_names=builtin_names)

    tool_choice = copied.get("tool_choice")
    if isinstance(tool_choice, dict) and str(tool_choice.get("type") or "").strip().lower() == "tool":
        _prefix_tool_field(tool_choice, "name", prefix=prefix, builtin_names=builtin_names)

    messages = copied.get("messages")
    if isinstance(messages, list):
        for message in messages:
            if not isinstance(message, dict):
                continue
            content = message.get("content")
            if not isinstance(content, list):
                continue
            for block in content:
                if not isinstance(block, dict):
                    continue
                block_type = str(block.get("type") or "").strip().lower()
                if block_type == "tool_use":
                    _prefix_tool_field(block, "name", prefix=prefix, builtin_names=builtin_names)
                elif block_type == "tool_reference":
                    _prefix_tool_field(
                        block,
                        "tool_name",
                        prefix=prefix,
                        builtin_names=builtin_names,
                    )
                elif block_type == "tool_result":
                    nested_content = block.get("content")
                    if not isinstance(nested_content, list):
                        continue
                    for nested_block in nested_content:
                        if not isinstance(nested_block, dict):
                            continue
                        if str(nested_block.get("type") or "").strip().lower() != "tool_reference":
                            continue
                        _prefix_tool_field(
                            nested_block,
                            "tool_name",
                            prefix=prefix,
                            builtin_names=builtin_names,
                        )

    return copied


def _strip_tool_field(container: dict[str, Any], field: str, *, prefix: str) -> None:
    value = container.get(field)
    if not isinstance(value, str) or not value.startswith(prefix):
        return
    container[field] = value[len(prefix) :]


def _strip_tool_prefix_from_content(content: list[Any], *, prefix: str) -> None:
    for block in content:
        if not isinstance(block, dict):
            continue
        block_type = str(block.get("type") or "").strip().lower()
        if block_type == "tool_use":
            _strip_tool_field(block, "name", prefix=prefix)
        elif block_type == "tool_reference":
            _strip_tool_field(block, "tool_name", prefix=prefix)
        elif block_type == "tool_result":
            nested = block.get("content")
            if isinstance(nested, list):
                _strip_tool_prefix_from_content(nested, prefix=prefix)


def strip_claude_tool_prefix_from_response(
    data: Any,
    *,
    prefix: str = CLAUDE_TOOL_PREFIX,
) -> Any:
    """Remove *prefix* from tool names in an upstream response payload."""
    if not prefix or not isinstance(data, dict):
        return data

    content = data.get("content")
    if isinstance(content, list):
        _strip_tool_prefix_from_content(content, prefix=prefix)

    content_block = data.get("content_block")
    if isinstance(content_block, dict):
        _strip_tool_prefix_from_content([content_block], prefix=prefix)

    return data


__all__ = [
    "CLAUDE_TOOL_PREFIX",
    "apply_claude_tool_prefix",
    "strip_claude_tool_prefix_from_response",
]
