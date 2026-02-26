"""Claude Code tool-name prefixing (request ↔ response)."""

from __future__ import annotations

import copy
import secrets
import string
from typing import Any

CLAUDE_TOOL_PREFIX = "proxy_"
_RANDOM_SUFFIX_ALPHABET = string.ascii_lowercase + string.digits

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


def _normalize_tool_name(value: Any) -> str:
    if not isinstance(value, str):
        return ""
    return value.strip()


def _generate_random_suffix(length: int = 6) -> str:
    safe_len = max(4, int(length or 6))
    return "".join(secrets.choice(_RANDOM_SUFFIX_ALPHABET) for _ in range(safe_len))


def _transform_tool_field(
    container: dict[str, Any],
    field: str,
    *,
    prefix: str,
    builtin_names: set[str],
    alias_forward: dict[str, str],
    alias_reverse: dict[str, str],
    random_suffix: str,
) -> None:
    raw_value = container.get(field)
    value = _normalize_tool_name(raw_value)
    if not value:
        return
    if value in builtin_names:
        container[field] = value
        return
    if value.startswith(prefix):
        container[field] = value
        return

    aliased = alias_forward.get(value)
    if aliased is None:
        aliased = f"{prefix}{value}"
        if random_suffix:
            aliased = f"{aliased}_{random_suffix}"
        alias_forward[value] = aliased
        alias_reverse[aliased] = value

    container[field] = aliased


def apply_claude_tool_prefix_with_alias_map(
    request_body: dict[str, Any],
    *,
    prefix: str = CLAUDE_TOOL_PREFIX,
    randomize_suffix: bool = False,
    random_suffix: str | None = None,
) -> tuple[dict[str, Any], dict[str, str]]:
    """Add prefix (and optional randomized alias) to custom tool names (deep-copy).

    Returns:
        (transformed_request_body, alias_reverse_map)
        alias_reverse_map 的 key 为上游可见别名，value 为原始工具名。
    """
    if not prefix:
        return request_body, {}

    suffix = ""
    if randomize_suffix:
        token = _normalize_tool_name(random_suffix) if random_suffix is not None else ""
        suffix = token if token else _generate_random_suffix()
    copied = copy.deepcopy(request_body)
    builtin_names = _collect_builtin_tool_names(copied)
    alias_forward: dict[str, str] = {}
    alias_reverse: dict[str, str] = {}

    tools = copied.get("tools")
    if isinstance(tools, list):
        for tool in tools:
            if not isinstance(tool, dict):
                continue
            if str(tool.get("type") or "").strip():
                continue
            _transform_tool_field(
                tool,
                "name",
                prefix=prefix,
                builtin_names=builtin_names,
                alias_forward=alias_forward,
                alias_reverse=alias_reverse,
                random_suffix=suffix,
            )

    tool_choice = copied.get("tool_choice")
    if isinstance(tool_choice, dict) and str(tool_choice.get("type") or "").strip().lower() == "tool":
        _transform_tool_field(
            tool_choice,
            "name",
            prefix=prefix,
            builtin_names=builtin_names,
            alias_forward=alias_forward,
            alias_reverse=alias_reverse,
            random_suffix=suffix,
        )

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
                    _transform_tool_field(
                        block,
                        "name",
                        prefix=prefix,
                        builtin_names=builtin_names,
                        alias_forward=alias_forward,
                        alias_reverse=alias_reverse,
                        random_suffix=suffix,
                    )
                elif block_type == "tool_reference":
                    _transform_tool_field(
                        block,
                        "tool_name",
                        prefix=prefix,
                        builtin_names=builtin_names,
                        alias_forward=alias_forward,
                        alias_reverse=alias_reverse,
                        random_suffix=suffix,
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
                        _transform_tool_field(
                            nested_block,
                            "tool_name",
                            prefix=prefix,
                            builtin_names=builtin_names,
                            alias_forward=alias_forward,
                            alias_reverse=alias_reverse,
                            random_suffix=suffix,
                        )

    return copied, alias_reverse


def apply_claude_tool_prefix(
    request_body: dict[str, Any],
    *,
    prefix: str = CLAUDE_TOOL_PREFIX,
    randomize_suffix: bool = False,
) -> dict[str, Any]:
    """Backward-compatible helper: returns transformed request only."""
    transformed, _ = apply_claude_tool_prefix_with_alias_map(
        request_body,
        prefix=prefix,
        randomize_suffix=randomize_suffix,
    )
    return transformed


def _restore_tool_field(
    container: dict[str, Any],
    field: str,
    *,
    prefix: str,
    alias_to_original: dict[str, str] | None,
) -> None:
    value = container.get(field)
    if not isinstance(value, str):
        return
    if alias_to_original and value in alias_to_original:
        container[field] = alias_to_original[value]
        return
    if value.startswith(prefix):
        container[field] = value[len(prefix) :]


def _strip_tool_prefix_from_content(
    content: list[Any],
    *,
    prefix: str,
    alias_to_original: dict[str, str] | None,
) -> None:
    for block in content:
        if not isinstance(block, dict):
            continue
        block_type = str(block.get("type") or "").strip().lower()
        if block_type == "tool_use":
            _restore_tool_field(
                block,
                "name",
                prefix=prefix,
                alias_to_original=alias_to_original,
            )
        elif block_type == "tool_reference":
            _restore_tool_field(
                block,
                "tool_name",
                prefix=prefix,
                alias_to_original=alias_to_original,
            )
        elif block_type == "tool_result":
            nested = block.get("content")
            if isinstance(nested, list):
                _strip_tool_prefix_from_content(
                    nested,
                    prefix=prefix,
                    alias_to_original=alias_to_original,
                )


def strip_claude_tool_prefix_from_response(
    data: Any,
    *,
    prefix: str = CLAUDE_TOOL_PREFIX,
    alias_to_original: dict[str, str] | None = None,
) -> Any:
    """Remove *prefix* from tool names in an upstream response payload."""
    if not prefix or not isinstance(data, dict):
        return data

    content = data.get("content")
    if isinstance(content, list):
        _strip_tool_prefix_from_content(
            content,
            prefix=prefix,
            alias_to_original=alias_to_original,
        )

    content_block = data.get("content_block")
    if isinstance(content_block, dict):
        _strip_tool_prefix_from_content(
            [content_block],
            prefix=prefix,
            alias_to_original=alias_to_original,
        )

    return data


__all__ = [
    "CLAUDE_TOOL_PREFIX",
    "apply_claude_tool_prefix_with_alias_map",
    "apply_claude_tool_prefix",
    "strip_claude_tool_prefix_from_response",
]
