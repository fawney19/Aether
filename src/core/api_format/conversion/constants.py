"""格式转换层常量定义 & 跨格式工具转换函数。

将跨层共享的常量集中在 core 层，避免 core -> services 的反向依赖。
OpenAI Chat <-> Responses API 的工具 / tool_choice / web_search 双向转换
由 openai.py 和 openai_cli.py 共享，避免两端维护不一致。
"""

from __future__ import annotations

import json
from typing import Any


def stable_json_dumps(value: Any) -> str:
    """Serialize JSON deterministically for cache-sensitive fallback generation."""
    return json.dumps(value, ensure_ascii=False, sort_keys=True)


# Thinking 签名验证的跳过标记
# 当无法获取真实签名时，使用此值作为占位符
DUMMY_THOUGHT_SIGNATURE = "skip_thought_signature_validator"

# ---------------------------------------------------------------------------
# OpenAI Chat / Responses API 跨格式透传字段白名单
# 由 openai.py 和 openai_cli.py 共享，避免两端维护不一致。
# ---------------------------------------------------------------------------

# 已由 normalizer 显式处理的字段 — 不需要从 extra 还原
OPENAI_HANDLED_KEYS: frozenset[str] = frozenset(
    {
        "messages",
        "model",
        "max_tokens",
        "max_completion_tokens",
        "temperature",
        "top_p",
        "stop",
        "stream",
        "tools",
        "tool_choice",
        "parallel_tool_calls",
        "reasoning",
        "reasoning_effort",
        "n",
        "presence_penalty",
        "frequency_penalty",
        "seed",
        "logprobs",
        "top_logprobs",
        "response_format",
        "verbosity",
        "text",
        "input",
        "instructions",
        "max_output_tokens",
        "web_search_options",
        "stream_options",
        # 已废弃的 Chat API 字段，不需要透传
        "function_call",
        "functions",
    }
)

# Chat Completions 允许透传的字段
OPENAI_CHAT_PASSTHROUGH_KEYS: frozenset[str] = frozenset(
    {
        "metadata",
        "user",
        "safety_identifier",
        "prompt_cache_key",
        "service_tier",
        "prompt_cache_retention",
        "modalities",
        "audio",
        "store",
        "prediction",
        "logit_bias",
    }
)

# Responses API 允许透传的字段
OPENAI_RESPONSES_PASSTHROUGH_KEYS: frozenset[str] = frozenset(
    {
        "include",
        "conversation",
        "context_management",
        "previous_response_id",
        "background",
        "max_tool_calls",
        "prompt",
        "truncation",
        "metadata",
        "user",
        "safety_identifier",
        "prompt_cache_key",
        "service_tier",
        "prompt_cache_retention",
        "store",
    }
)


# ---------------------------------------------------------------------------
# OpenAI Chat <-> Responses API 工具 / tool_choice / web_search 双向转换
# ---------------------------------------------------------------------------


def responses_tool_to_chat_tool(tool: dict[str, Any]) -> dict[str, Any] | None:
    """Responses API tool -> Chat Completions tool (嵌套结构)。"""
    tool_type = str(tool.get("type") or "")
    if tool_type == "function":
        name = str(tool.get("name") or "")
        if not name:
            return None
        function: dict[str, Any] = {"name": name}
        if isinstance(tool.get("description"), str):
            function["description"] = tool["description"]
        if isinstance(tool.get("parameters"), dict):
            function["parameters"] = tool["parameters"]
        if tool.get("strict") is not None:
            function["strict"] = tool.get("strict")
        return {"type": "function", "function": function}
    if tool_type == "custom":
        name = str(tool.get("name") or "")
        if not name:
            return None
        custom: dict[str, Any] = {"name": name}
        if isinstance(tool.get("description"), str):
            custom["description"] = tool["description"]
        if isinstance(tool.get("format"), dict):
            custom["format"] = tool["format"]
        return {"type": "custom", "custom": custom}
    return None


def chat_tool_to_responses_tool(tool: dict[str, Any]) -> dict[str, Any] | None:
    """Chat Completions tool (嵌套结构) -> Responses API tool (扁平结构)。"""
    tool_type = str(tool.get("type") or "")
    if tool_type == "function" and isinstance(tool.get("function"), dict):
        function = tool["function"]
        name = str(function.get("name") or "")
        if not name:
            return None
        translated: dict[str, Any] = {"type": "function", "name": name}
        if isinstance(function.get("description"), str):
            translated["description"] = function["description"]
        if isinstance(function.get("parameters"), dict):
            translated["parameters"] = function["parameters"]
        if function.get("strict") is not None:
            translated["strict"] = function.get("strict")
        return translated
    if tool_type == "custom" and isinstance(tool.get("custom"), dict):
        custom = tool["custom"]
        name = str(custom.get("name") or "")
        if not name:
            return None
        translated = {"type": "custom", "name": name}
        if isinstance(custom.get("description"), str):
            translated["description"] = custom["description"]
        if isinstance(custom.get("format"), dict):
            translated["format"] = custom["format"]
        return translated
    return None


def responses_web_search_tool_to_chat_options(
    tool: dict[str, Any],
) -> dict[str, Any] | None:
    """Responses API web_search tool -> Chat Completions web_search_options。"""
    tool_type = str(tool.get("type") or "")
    if not tool_type.startswith("web_search"):
        return None
    options: dict[str, Any] = {}
    user_location = tool.get("user_location")
    if isinstance(user_location, dict):
        approximate = dict(user_location)
        approximate.pop("type", None)
        options["user_location"] = {"type": "approximate", "approximate": approximate}
    search_context_size = tool.get("search_context_size")
    if isinstance(search_context_size, str) and search_context_size:
        options["search_context_size"] = search_context_size
    return options or None


def chat_web_search_options_to_responses_tools(
    web_search_options: Any,
) -> list[dict[str, Any]] | None:
    """Chat Completions web_search_options -> Responses API web_search tool 列表。"""
    if not isinstance(web_search_options, dict):
        return None
    tool: dict[str, Any] = {"type": "web_search"}
    user_location = web_search_options.get("user_location")
    if isinstance(user_location, dict):
        approximate = user_location.get("approximate")
        if isinstance(approximate, dict):
            tool["user_location"] = {"type": "approximate", **approximate}
    search_context_size = web_search_options.get("search_context_size")
    if isinstance(search_context_size, str) and search_context_size:
        tool["search_context_size"] = search_context_size
    return [tool]


def responses_tool_choice_to_chat(
    tool_choice: dict[str, Any],
) -> dict[str, Any] | None:
    """Responses API tool_choice dict -> Chat Completions tool_choice dict。"""
    choice_type = str(tool_choice.get("type") or "")
    if choice_type == "allowed_tools":
        mode = tool_choice.get("mode")
        tools = tool_choice.get("tools")
        if isinstance(mode, str) and isinstance(tools, list):
            return {"type": "allowed_tools", "allowed_tools": {"mode": mode, "tools": tools}}
    if choice_type == "function":
        fn = tool_choice.get("function")
        name = str(
            tool_choice.get("name") or (fn.get("name") if isinstance(fn, dict) else "") or ""
        )
        if name:
            return {"type": "function", "function": {"name": name}}
    if choice_type == "custom":
        custom = tool_choice.get("custom")
        name = str(
            tool_choice.get("name")
            or (custom.get("name") if isinstance(custom, dict) else "")
            or ""
        )
        if name:
            return {"type": "custom", "custom": {"name": name}}
    return None


def chat_tool_choice_to_responses(
    tool_choice: dict[str, Any],
) -> dict[str, Any] | None:
    """Chat Completions tool_choice dict -> Responses API tool_choice dict。"""
    choice_type = str(tool_choice.get("type") or "")
    if choice_type == "allowed_tools" and isinstance(tool_choice.get("allowed_tools"), dict):
        allowed_tools = tool_choice["allowed_tools"]
        mode = allowed_tools.get("mode")
        tools = allowed_tools.get("tools")
        if isinstance(mode, str) and isinstance(tools, list):
            return {"type": "allowed_tools", "mode": mode, "tools": tools}
    if choice_type == "function" and isinstance(tool_choice.get("function"), dict):
        name = str(tool_choice["function"].get("name") or "")
        if name:
            return {"type": "function", "name": name}
    if choice_type == "custom" and isinstance(tool_choice.get("custom"), dict):
        name = str(tool_choice["custom"].get("name") or "")
        if name:
            return {"type": "custom", "name": name}
    return None
