"""
Thinking 整流器（Rectifier）

采用 cc-switch 的"错误触发"模式，在遇到 Thinking 签名/结构错误时触发整流。

核心功能：
1. 移除所有 thinking 和 redacted_thinking 块
2. 移除非 thinking 块上的 signature 字段
3. 条件删除顶层 thinking 参数

与 ThinkingSanitizer 的区别：
- ThinkingSanitizer: 预清洗（全局配置启用时执行），可选择保留或清洗
- ThinkingRectifier: 错误触发整流（遇到签名/结构错误时执行），彻底清洗

使用场景：
当遇到 ThinkingSignatureException 时，调用 rectify() 整流请求体后重试一次。
"""

import copy
from typing import Any, Dict, List, Tuple

from src.core.logger import logger


class ThinkingRectifier:
    """
    Thinking 整流器

    在遇到 Thinking 签名/结构错误时，整流请求体以便重试。
    采用"彻底清洗 + 条件禁用 thinking"策略。
    """

    @staticmethod
    def rectify(request_body: Dict[str, Any]) -> Tuple[Dict[str, Any], bool]:
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
        if ThinkingRectifier._should_remove_top_level_thinking(rectified_body, rectified_body.get("messages", [])):
            if "thinking" in rectified_body:
                del rectified_body["thinking"]
                modified = True
                logger.info("ThinkingRectifier: 已移除顶层 thinking 参数")

        return rectified_body, modified

    @staticmethod
    def _rectify_messages(messages: List[Dict[str, Any]]) -> Tuple[List[Dict[str, Any]], bool]:
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
        result_messages = []
        thinking_removed = 0
        signature_removed = 0

        for message in messages:
            # 类型保护：跳过非 dict 消息
            if not isinstance(message, dict):
                result_messages.append(message)
                continue

            new_message = dict(message)  # 浅拷贝
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

                new_message["content"] = new_content

            result_messages.append(new_message)

        if thinking_removed > 0 or signature_removed > 0:
            logger.info(
                f"ThinkingRectifier: 移除了 {thinking_removed} 个 thinking 块, "
                f"{signature_removed} 个 signature 字段"
            )

        return result_messages, modified

    @staticmethod
    def _should_remove_top_level_thinking(body: Dict[str, Any], messages: List[Dict[str, Any]]) -> bool:
        """
        判断是否应该删除顶层 thinking 参数

        与 cc-switch 行为一致：只检查最后一条 assistant 消息

        设计思路：
        - 传入的 messages 是整流后的状态，thinking 块已被移除
        - Claude API 只校验最后一条 assistant 消息的结构
        - 如果最后一条有 tool_use 但首块不是 thinking，需要禁用 thinking 参数

        Args:
            body: 请求体
            messages: 消息列表（整流后的）

        Returns:
            是否应该删除顶层 thinking 参数
        """
        # 条件 1: thinking 参数存在且已启用
        thinking_param = body.get("thinking")
        if not isinstance(thinking_param, dict) or thinking_param.get("type") != "enabled":
            return False

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

        # 检查首块是否为 thinking/redacted_thinking
        first_block = content[0]
        first_is_thinking = (
            isinstance(first_block, dict) and
            first_block.get("type") in ("thinking", "redacted_thinking")
        )

        # 如果首块是 thinking，不需要禁用（结构正确）
        if first_is_thinking:
            return False

        # 检查是否有 tool_use
        has_tool_use = any(
            isinstance(block, dict) and block.get("type") == "tool_use"
            for block in content
        )

        # 最后一条 assistant 有 tool_use 但首块不是 thinking → 需要禁用 thinking
        if has_tool_use:
            logger.info(
                "ThinkingRectifier: 最后一条 assistant 消息有 tool_use 但无 thinking 前缀，"
                "需要禁用 thinking 参数"
            )
            return True

        logger.debug("ThinkingRectifier: 最后一条 assistant 消息结构正常，保留 thinking 参数")
        return False
