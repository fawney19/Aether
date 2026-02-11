"""
Thinking 整流器（Rectifier）

采用 cc-switch 的"错误触发"模式，在遇到 Thinking 签名/结构错误时触发整流。

核心功能：
1. 移除所有 thinking 和 redacted_thinking 块
2. 移除非 thinking 块上的 signature 字段
3. 条件删除顶层 thinking 参数

使用场景：
当遇到 ThinkingSignatureException 时，调用 rectify() 整流请求体后重试一次。
"""

import copy
from typing import Any

from src.core.logger import logger
from src.utils import json_helper as json


class ThinkingRectifier:
    """
    Thinking 整流器

    在遇到 Thinking 签名/结构错误时，整流请求体以便重试。
    采用"彻底清洗 + 条件禁用 thinking"策略。
    """

    @staticmethod
    def rectify(request_body: dict[str, Any]) -> tuple[dict[str, Any], bool]:
        """
        整流请求体

        执行以下操作：
        1. 移除所有 thinking 和 redacted_thinking 块
        2. 移除非 thinking 块上的 signature 字段
        3. 条件删除顶层 thinking 参数

        Args:
            request_body: 原始请求体

        Returns:
            Tuple[整流后的请求体, 是否有修改]
        """
        if not request_body:
            return request_body, False

        # 深拷贝以避免修改原始数据
        rectified_body = copy.deepcopy(request_body)
        modified = False

        # 1. 整流 messages
        messages = rectified_body.get("messages", [])
        if messages:
            rectified_messages, messages_modified = ThinkingRectifier._rectify_messages(messages)
            if messages_modified:
                rectified_body["messages"] = rectified_messages
                modified = True

        # 2. 条件删除顶层 thinking 参数（使用整流后的 messages 判断）
        # 与 cc-switch 行为一致：在整流 messages 之后获取快照进行判断
        if ThinkingRectifier._should_remove_top_level_thinking(rectified_body):
            if "thinking" in rectified_body:
                del rectified_body["thinking"]
                modified = True
                logger.info("ThinkingRectifier: 已移除顶层 thinking 参数")

        return rectified_body, modified

    @staticmethod
    def rectify_signature_sensitive_blocks(
        request_body: dict[str, Any],
    ) -> tuple[dict[str, Any], bool]:
        """Second-stage rectification for signature-related failures.

        This is a more aggressive fallback than `rectify()`:
        - Removes all thinking/redacted_thinking blocks
        - Removes signature fields on remaining blocks
        - Degrades tool_use/tool_result blocks into plain text blocks
        - Disables top-level `thinking` when enabled
        """
        if not request_body:
            return request_body, False

        rectified_body = copy.deepcopy(request_body)
        modified = False

        messages = rectified_body.get("messages", [])
        if isinstance(messages, list) and messages:
            new_messages: list[Any] = []
            for message in messages:
                if not isinstance(message, dict):
                    new_messages.append(message)
                    continue

                new_message = dict(message)
                content = message.get("content")
                if isinstance(content, list):
                    new_content: list[Any] = []
                    for block in content:
                        if not isinstance(block, dict):
                            new_content.append(block)
                            continue

                        block_type = block.get("type")

                        if block_type in ("thinking", "redacted_thinking"):
                            modified = True
                            continue

                        if block_type == "tool_use":
                            # Degrade into text to avoid strict structure/signature validation.
                            name = block.get("name")
                            inp = block.get("input")
                            try:
                                inp_text = json.dumps(inp, ensure_ascii=False)
                            except Exception:
                                inp_text = str(inp)
                            new_content.append(
                                {
                                    "type": "text",
                                    "text": f"[tool_use] name={name} input={inp_text}",
                                }
                            )
                            modified = True
                            continue

                        if block_type == "tool_result":
                            raw = block.get("content")
                            try:
                                raw_text = json.dumps(raw, ensure_ascii=False)
                            except Exception:
                                raw_text = str(raw)
                            new_content.append(
                                {
                                    "type": "text",
                                    "text": f"[tool_result] {raw_text}",
                                }
                            )
                            modified = True
                            continue

                        # Remove signature field (for any non-thinking block).
                        if "signature" in block:
                            new_block = {k: v for k, v in block.items() if k != "signature"}
                            new_content.append(new_block)
                            modified = True
                            continue

                        new_content.append(block)

                    new_message["content"] = new_content

                new_messages.append(new_message)

            rectified_body["messages"] = new_messages

        # Stage-2: disable top-level thinking unconditionally when enabled.
        thinking_param = rectified_body.get("thinking")
        if isinstance(thinking_param, dict) and thinking_param.get("type") == "enabled":
            del rectified_body["thinking"]
            modified = True
            logger.info("ThinkingRectifier(stage2): 已移除顶层 thinking 参数")

        return rectified_body, modified

    @staticmethod
    def _rectify_messages(messages: list[dict[str, Any]]) -> tuple[list[dict[str, Any]], bool]:
        """
        整流消息列表

        移除所有 thinking/redacted_thinking 块和 signature 字段

        Args:
            messages: 原始消息列表

        Returns:
            Tuple[整流后的消息列表, 是否有修改]
        """
        if not messages:
            return messages, False

        modified = False
        result_messages: list[dict[str, Any]] = []
        thinking_removed = 0
        signature_removed = 0

        for message in messages:
            # 类型保护：跳过非 dict 消息
            if not isinstance(message, dict):
                result_messages.append(message)
                continue

            # 消息级浅拷贝：外层 rectify() 已深拷贝整个 request_body
            # content 会被重建为新列表，不会影响原始数据
            new_message = dict(message)
            content = message.get("content")

            if isinstance(content, list):
                new_content = []
                for block in content:
                    if isinstance(block, dict):
                        block_type = block.get("type")

                        # 移除 thinking 和 redacted_thinking 块
                        if block_type in ("thinking", "redacted_thinking"):
                            thinking_removed += 1
                            modified = True
                            continue

                        # 移除非 thinking 块上的 signature 字段
                        if "signature" in block:
                            new_block = {k: v for k, v in block.items() if k != "signature"}
                            new_content.append(new_block)
                            signature_removed += 1
                            modified = True
                            continue

                        new_content.append(block)
                    else:
                        new_content.append(block)

                # 更新 content
                new_message["content"] = new_content

            # 如果整流后 assistant 消息的 content 为空，记录警告
            # （空 content 本身不是"修改"，只是检测到的状态，不设置 modified）
            # 保留消息是必要的：跳过会破坏对话结构（后续 tool_result 消息需要前置 assistant 消息）
            if new_message.get("role") == "assistant":
                effective_content = new_message.get("content")
                is_empty = not effective_content or (
                    isinstance(effective_content, list) and len(effective_content) == 0
                )
                if is_empty:
                    msg_idx = len(result_messages)
                    logger.warning(
                        f"ThinkingRectifier: assistant 消息整流后 content 为空 (message_index={msg_idx})"
                    )

            result_messages.append(new_message)

        if thinking_removed > 0 or signature_removed > 0:
            logger.info(
                f"ThinkingRectifier: 移除了 {thinking_removed} 个 thinking 块, "
                f"{signature_removed} 个 signature 字段"
            )

        return result_messages, modified

    @staticmethod
    def _should_remove_top_level_thinking(body: dict[str, Any]) -> bool:
        """
        判断是否应该删除顶层 thinking 参数

        与 cc-switch 行为一致：只检查最后一条 assistant 消息

        设计思路：
        - body 中的 messages 是整流后的状态，thinking 块已被移除
        - Claude API 只校验最后一条 assistant 消息的结构
        - 如果最后一条有 tool_use 但首块不是 thinking，需要禁用 thinking 参数

        Args:
            body: 整流后的请求体

        Returns:
            是否应该删除顶层 thinking 参数
        """
        # 条件 1: thinking 参数存在且已启用
        thinking_param = body.get("thinking")
        if not isinstance(thinking_param, dict) or thinking_param.get("type") != "enabled":
            return False

        # 从 body 中获取 messages
        messages = body.get("messages", [])

        # 类型保护：确保 messages 是 list
        if not isinstance(messages, list) or not messages:
            return False

        # 条件 2: 找到最后一条 assistant 消息
        last_assistant = None
        for message in reversed(messages):
            if isinstance(message, dict) and message.get("role") == "assistant":
                last_assistant = message
                break

        if not last_assistant:
            return False

        content = last_assistant.get("content")
        if not isinstance(content, list) or not content:
            return False

        # 注意：传入的 messages 是整流后的状态，thinking 块已被移除
        # 因此只需检查是否有 tool_use，如果有则需要禁用 thinking 参数
        # （因为整流后的 assistant 消息不再以 thinking 块开头）

        # 检查是否有 tool_use
        has_tool_use = any(
            isinstance(block, dict) and block.get("type") == "tool_use" for block in content
        )

        # 整流后 assistant 消息不再以 thinking 块开头，如果有 tool_use 则需要禁用 thinking 参数
        # （Claude API 要求：启用 thinking 时，有 tool_use 的 assistant 消息必须以 thinking 块开头）
        if has_tool_use:
            logger.info(
                "ThinkingRectifier: 整流后 assistant 消息有 tool_use 但无 thinking 前缀，"
                "禁用 thinking 参数以通过 API 校验"
            )
            return True

        logger.debug("ThinkingRectifier: 整流后 assistant 消息无 tool_use，保留 thinking 参数")
        return False
