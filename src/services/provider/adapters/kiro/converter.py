"""Claude Messages -> Kiro ConversationState converter (best-effort).

This mirrors `kiro.rs/src/anthropic/converter.rs` but focuses on the fields
needed by generateAssistantResponse.
"""

from __future__ import annotations

import uuid
from typing import Any

from src.core.logger import logger
from src.services.provider.adapters.kiro.constants import (
    SYSTEM_CHUNKED_POLICY as _SYSTEM_CHUNKED_POLICY,
)
from src.services.provider.adapters.kiro.constants import (
    TOOL_DESCRIPTION_SUFFIXES as _TOOL_DESCRIPTION_SUFFIXES,
)
from src.utils import json_helper as json


def map_model(model: str) -> str | None:
    """Map an Anthropic model name to a Kiro-compatible model ID.

    Kiro upstream expects specific model IDs (e.g. ``claude-sonnet-4.5``).
    If *model* already matches a Kiro ID it is returned as-is; otherwise we
    attempt a best-effort fuzzy mapping.  Returns ``None`` when the model is
    unrecognised.
    """
    raw = str(model or "").strip()
    if not raw:
        return None

    model_lower = raw.lower()

    # Already a Kiro-native ID?
    _KIRO_NATIVE_IDS = {
        "claude-sonnet-4.5",
        "claude-opus-4.5",
        "claude-opus-4.6",
        "claude-haiku-4.5",
    }
    if model_lower in _KIRO_NATIVE_IDS:
        return model_lower

    # Fuzzy mapping from Anthropic-style model names
    if "sonnet" in model_lower:
        return "claude-sonnet-4.5"
    if "opus" in model_lower:
        if "4-5" in model_lower or "4.5" in model_lower:
            return "claude-opus-4.5"
        return "claude-opus-4.6"
    if "haiku" in model_lower:
        return "claude-haiku-4.5"

    # Unrecognised — pass through as-is (let the upstream decide)
    return raw


def _extract_session_id(user_id: str) -> str | None:
    text = str(user_id or "")
    pos = text.find("session_")
    if pos < 0:
        return None
    session_part = text[pos + 8 :]
    if len(session_part) < 36:
        return None
    candidate = session_part[:36]
    if candidate.count("-") != 4:
        return None
    return candidate


def _generate_thinking_prefix(request_body: dict[str, Any]) -> str | None:
    thinking = request_body.get("thinking")
    if not isinstance(thinking, dict):
        return None

    thinking_type = str(thinking.get("type") or "").strip()
    if thinking_type == "enabled":
        budget = thinking.get("budget_tokens")
        try:
            budget_i = int(budget) if budget is not None else 0
        except Exception:
            budget_i = 0
        return (
            f"<thinking_mode>enabled</thinking_mode>"
            f"<max_thinking_length>{budget_i}</max_thinking_length>"
        )

    if thinking_type == "adaptive":
        output_cfg = request_body.get("output_config")
        effort = "high"
        if isinstance(output_cfg, dict):
            eff = output_cfg.get("effort")
            if isinstance(eff, str) and eff.strip():
                effort = eff.strip()
        return (
            f"<thinking_mode>adaptive</thinking_mode>"
            f"<thinking_effort>{effort}</thinking_effort>"
        )

    return None


def _has_thinking_tags(content: str) -> bool:
    return "<thinking_mode>" in content or "<max_thinking_length>" in content


def _system_to_text(system: Any) -> str:
    if system is None:
        return ""
    if isinstance(system, str):
        return system
    if isinstance(system, list):
        parts: list[str] = []
        for item in system:
            if isinstance(item, dict):
                t = item.get("text")
                if isinstance(t, str) and t:
                    parts.append(t)
            else:
                # best-effort
                try:
                    parts.append(str(item))
                except Exception:
                    pass
        return "\n".join([p for p in parts if p])
    return ""


def _get_image_format(media_type: str | None) -> str | None:
    if not isinstance(media_type, str) or "/" not in media_type:
        return None
    prefix, suffix = media_type.split("/", 1)
    if prefix != "image":
        return None
    suffix = suffix.strip().lower()
    if suffix in {"jpeg", "png", "gif", "webp"}:
        return suffix
    if suffix == "jpg":
        return "jpeg"
    return None


def _process_message_content(
    content: Any,
) -> tuple[str, list[dict[str, Any]], list[dict[str, Any]]]:
    """Extract text/images/tool_results from a Claude content field."""
    text_parts: list[str] = []
    images: list[dict[str, Any]] = []
    tool_results: list[dict[str, Any]] = []

    if isinstance(content, str):
        if content:
            text_parts.append(content)
        return "".join(text_parts), images, tool_results

    if not isinstance(content, list):
        return "".join(text_parts), images, tool_results

    for block in content:
        if not isinstance(block, dict):
            continue

        btype = str(block.get("type") or "").strip()

        if btype == "text":
            text = block.get("text")
            if isinstance(text, str) and text:
                text_parts.append(text)
            continue

        if btype == "image":
            source = block.get("source")
            if not isinstance(source, dict):
                continue
            media_type = source.get("media_type") or source.get("mediaType")
            fmt = _get_image_format(media_type if isinstance(media_type, str) else None)
            data = source.get("data")
            if fmt and isinstance(data, str) and data:
                images.append({"format": fmt, "source": {"bytes": data}})
            continue

        if btype == "tool_result":
            tool_use_id = block.get("tool_use_id") or block.get("toolUseId")
            if not isinstance(tool_use_id, str) or not tool_use_id.strip():
                continue

            raw_content = block.get("content")
            if isinstance(raw_content, str):
                text = raw_content
            elif isinstance(raw_content, list):
                # Claude tool_result content blocks; keep only text parts.
                parts: list[str] = []
                for item in raw_content:
                    if isinstance(item, dict) and item.get("type") == "text":
                        t = item.get("text")
                        if isinstance(t, str) and t:
                            parts.append(t)
                text = "\n".join(parts)
            else:
                try:
                    text = json.dumps(raw_content, ensure_ascii=False)
                except Exception:
                    text = str(raw_content)

            is_error = bool(block.get("is_error") or block.get("isError") or False)
            status = "error" if is_error else "success"

            tool_results.append(
                {
                    "toolUseId": tool_use_id.strip(),
                    "content": [{"text": text or ""}],
                    "status": status,
                    "isError": bool(is_error),
                }
            )
            continue

    return "".join(text_parts), images, tool_results


def _convert_tools(tools: Any) -> list[dict[str, Any]]:
    if not isinstance(tools, list):
        return []

    out: list[dict[str, Any]] = []
    for t in tools:
        if not isinstance(t, dict):
            continue
        name = t.get("name")
        if not isinstance(name, str) or not name.strip():
            continue

        description = t.get("description")
        description_str = description if isinstance(description, str) else ""

        # Inject chunked-write instructions for Write/Edit tools.
        suffix = _TOOL_DESCRIPTION_SUFFIXES.get(name.strip())
        if suffix:
            description_str = f"{description_str}\n{suffix}" if description_str else suffix

        if len(description_str) > 10000:
            description_str = description_str[:10000]

        input_schema = t.get("input_schema") or t.get("inputSchema") or {}
        if not isinstance(input_schema, dict):
            input_schema = {}

        out.append(
            {
                "toolSpecification": {
                    "name": name.strip(),
                    "description": description_str,
                    "inputSchema": {"json": input_schema},
                }
            }
        )

    return out


def _create_placeholder_tool(name: str) -> dict[str, Any]:
    return {
        "toolSpecification": {
            "name": name,
            "description": "Tool used in conversation history",
            "inputSchema": {
                "json": {
                    "$schema": "http://json-schema.org/draft-07/schema#",
                    "type": "object",
                    "properties": {},
                    "required": [],
                    "additionalProperties": True,
                }
            },
        }
    }


def _convert_assistant_message(message: dict[str, Any]) -> dict[str, Any] | None:
    content = message.get("content")

    tool_uses: list[dict[str, Any]] = []
    thinking_parts: list[str] = []
    text_parts: list[str] = []

    if isinstance(content, str):
        if content:
            text_parts.append(content)
    elif isinstance(content, list):
        for block in content:
            if not isinstance(block, dict):
                continue
            btype = str(block.get("type") or "")
            if btype == "thinking":
                # Preserve thinking content so multi-turn context is not lost.
                t = block.get("thinking")
                if isinstance(t, str) and t:
                    thinking_parts.append(t)
            elif btype == "text":
                t = block.get("text")
                if isinstance(t, str) and t:
                    text_parts.append(t)
            elif btype == "tool_use":
                tool_use_id = block.get("id")
                name = block.get("name")
                if not isinstance(tool_use_id, str) or not tool_use_id.strip():
                    continue
                if not isinstance(name, str) or not name.strip():
                    continue
                inp = block.get("input")
                if not isinstance(inp, dict):
                    inp = {}
                tool_uses.append(
                    {
                        "toolUseId": tool_use_id.strip(),
                        "name": name.strip(),
                        "input": inp,
                    }
                )

    # Combine thinking + text into final content.
    # Format: <thinking>...</thinking>\n\ntext
    thinking_str = "".join(thinking_parts)
    text_str = "".join(text_parts)

    if thinking_str:
        if text_str:
            content_str = f"<thinking>{thinking_str}</thinking>\n\n{text_str}"
        else:
            content_str = f"<thinking>{thinking_str}</thinking>"
    else:
        content_str = text_str

    if not content_str and tool_uses:
        content_str = " "  # Kiro API requires non-empty content.

    if not content_str and not tool_uses:
        return None

    out: dict[str, Any] = {"content": content_str}
    if tool_uses:
        out["toolUses"] = tool_uses
    return out


def convert_claude_messages_to_conversation_state(
    request_body: dict[str, Any],
    *,
    model: str,
) -> dict[str, Any]:
    model_id = map_model(model)
    if not model_id:
        raise ValueError(f"kiro: model is required (got {model!r})")

    messages = request_body.get("messages")
    if not isinstance(messages, list) or not messages:
        raise ValueError("kiro: empty messages")

    conversation_id = None
    metadata = request_body.get("metadata")
    if isinstance(metadata, dict):
        user_id = metadata.get("user_id") or metadata.get("userId")
        if isinstance(user_id, str) and user_id:
            conversation_id = _extract_session_id(user_id)

    if not conversation_id:
        conversation_id = str(uuid.uuid4())

    agent_continuation_id = str(uuid.uuid4())

    thinking_prefix = _generate_thinking_prefix(request_body)

    history: list[dict[str, Any]] = []

    # System injection: add as (user, assistant) pair.
    system_text = _system_to_text(request_body.get("system"))
    if system_text:
        # Append chunked-write policy so the model silently obeys tool limits.
        final_system = f"{system_text}\n{_SYSTEM_CHUNKED_POLICY}"
        if thinking_prefix and not _has_thinking_tags(system_text):
            final_system = f"{thinking_prefix}\n{final_system}"
        history.append(
            {
                "userInputMessage": {
                    "content": final_system,
                    "modelId": model_id,
                    "origin": "AI_EDITOR",
                }
            }
        )
        history.append(
            {"assistantResponseMessage": {"content": "I will follow these instructions."}}
        )
    elif thinking_prefix:
        history.append(
            {
                "userInputMessage": {
                    "content": thinking_prefix,
                    "modelId": model_id,
                    "origin": "AI_EDITOR",
                }
            }
        )
        history.append(
            {"assistantResponseMessage": {"content": "I will follow these instructions."}}
        )

    # Build history from messages.
    # If the last message is assistant, include it in history (Kiro currentMessage
    # must be user; we synthesise one).  Otherwise the last user message becomes
    # currentMessage and everything before it goes into history.
    last_msg = messages[-1]
    last_is_assistant = (
        isinstance(last_msg, dict) and str(last_msg.get("role") or "") == "assistant"
    )

    if last_is_assistant:
        # All messages go into history; we'll synthesise a currentMessage later.
        history_end_index = len(messages)
    else:
        history_end_index = max(len(messages) - 1, 0)

    user_buffer: list[dict[str, Any]] = []

    def _flush_user_buffer() -> dict[str, Any] | None:
        nonlocal user_buffer
        if not user_buffer:
            return None

        parts: list[str] = []
        images: list[dict[str, Any]] = []
        tool_results: list[dict[str, Any]] = []

        for msg in user_buffer:
            text, imgs, results = _process_message_content(msg.get("content"))
            if text:
                parts.append(text)
            images.extend(imgs)
            tool_results.extend(results)

        user_buffer = []

        payload: dict[str, Any] = {
            "content": "\n".join(parts),
            "modelId": model_id,
            "origin": "AI_EDITOR",
        }

        if images:
            payload["images"] = images

        if tool_results:
            payload["userInputMessageContext"] = {"toolResults": tool_results}

        return {"userInputMessage": payload}

    for i in range(history_end_index):
        msg = messages[i]
        if not isinstance(msg, dict):
            continue
        role = str(msg.get("role") or "")
        if role == "user":
            user_buffer.append(msg)
            continue
        if role == "assistant":
            user_item = _flush_user_buffer()
            if user_item is not None:
                history.append(user_item)
                assistant_item = _convert_assistant_message(msg)
                if assistant_item is not None:
                    history.append({"assistantResponseMessage": assistant_item})
            continue

    # trailing unpaired user messages in history
    tail_user = _flush_user_buffer()
    if tail_user is not None:
        history.append(tail_user)
        history.append({"assistantResponseMessage": {"content": "OK"}})

    # Current message: last message as user input.
    if last_is_assistant:
        # Synthesise a minimal user continuation message.
        text_content = "Continue."
        images: list[dict[str, Any]] = []
        tool_results: list[dict[str, Any]] = []
    else:
        last = messages[-1]
        if not isinstance(last, dict) or str(last.get("role") or "") != "user":
            raise ValueError("kiro: last message must be user")
        text_content, images, tool_results = _process_message_content(last.get("content"))

    tools = _convert_tools(request_body.get("tools"))

    # Ensure tools referenced in history assistant toolUses are defined.
    # Also collect ids for tool_use / tool_result pairing validation.
    history_tool_names: set[str] = set()
    history_tool_results_ids: set[str] = set()
    history_tool_use_ids: set[str] = set()

    for item in history:
        if not isinstance(item, dict):
            continue
        u = item.get("userInputMessage")
        if isinstance(u, dict):
            ctx = u.get("userInputMessageContext")
            if isinstance(ctx, dict):
                results = ctx.get("toolResults")
                if isinstance(results, list):
                    for r in results:
                        if isinstance(r, dict):
                            tid = r.get("toolUseId")
                            if isinstance(tid, str) and tid:
                                history_tool_results_ids.add(tid)
        a = item.get("assistantResponseMessage")
        if isinstance(a, dict):
            uses = a.get("toolUses")
            if isinstance(uses, list):
                for tu in uses:
                    if not isinstance(tu, dict):
                        continue
                    nm = tu.get("name")
                    if isinstance(nm, str) and nm:
                        history_tool_names.add(nm)
                    tid = tu.get("toolUseId")
                    if isinstance(tid, str) and tid:
                        history_tool_use_ids.add(tid)

    existing_tool_names = {
        str(t.get("toolSpecification", {}).get("name", "")).lower() for t in tools
    }

    for tool_name in sorted(history_tool_names):
        if tool_name.lower() not in existing_tool_names:
            tools.append(_create_placeholder_tool(tool_name))

    # Filter tool_results: only keep those with matching tool_use in history, and not duplicated.
    validated_tool_results: list[dict[str, Any]] = []
    current_tool_result_ids: set[str] = set()
    for r in tool_results:
        if not isinstance(r, dict):
            continue
        tid = r.get("toolUseId")
        if not isinstance(tid, str) or not tid:
            continue
        if tid not in history_tool_use_ids:
            continue
        if tid in history_tool_results_ids:
            continue
        validated_tool_results.append(r)
        current_tool_result_ids.add(tid)

    # Remove orphaned tool_uses from history.
    # Kiro API requires every tool_use to have a matching tool_result; otherwise
    # it returns 400 Bad Request.
    orphaned_tool_use_ids = (
        history_tool_use_ids - history_tool_results_ids - current_tool_result_ids
    )
    if orphaned_tool_use_ids:
        logger.warning(
            "kiro: removing {} orphaned tool_use(s) from history: {}",
            len(orphaned_tool_use_ids),
            orphaned_tool_use_ids,
        )
        for item in history:
            if not isinstance(item, dict):
                continue
            a = item.get("assistantResponseMessage")
            if not isinstance(a, dict):
                continue
            uses = a.get("toolUses")
            if not isinstance(uses, list):
                continue
            filtered = [
                u
                for u in uses
                if not (
                    isinstance(u, dict)
                    and isinstance(u.get("toolUseId"), str)
                    and u["toolUseId"] in orphaned_tool_use_ids
                )
            ]
            if not filtered:
                a.pop("toolUses", None)
            elif len(filtered) != len(uses):
                a["toolUses"] = filtered

    user_ctx: dict[str, Any] = {}
    if tools:
        user_ctx["tools"] = tools
    if validated_tool_results:
        user_ctx["toolResults"] = validated_tool_results

    user_input: dict[str, Any] = {
        "userInputMessageContext": user_ctx,
        "content": text_content,
        "modelId": model_id,
        "origin": "AI_EDITOR",
    }
    if images:
        user_input["images"] = images

    conversation_state = {
        "agentContinuationId": agent_continuation_id,
        "agentTaskType": "vibe",
        "chatTriggerType": "MANUAL",
        "currentMessage": {"userInputMessage": user_input},
        "conversationId": conversation_id,
        "history": history,
    }

    return conversation_state


__all__ = [
    "convert_claude_messages_to_conversation_state",
    "map_model",
]
