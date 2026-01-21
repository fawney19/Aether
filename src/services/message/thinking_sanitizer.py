"""
Thinking 块清洗服务（Legacy）

注意：此类已被 ThinkingRectifier 取代，不再用于预清洗流程。
保留此类仅供参考和向后兼容。

原用途：处理跨供应商场景下的 thinking 块签名兼容性问题。
新方案：使用 ThinkingRectifier 在遇到签名/结构错误时自动整流。

支持的清洗模式：
- convert_to_text: 将 thinking 块转换为普通文本块（保留内容）
- remove: 完全移除 thinking 块

清洗策略：
- STRATEGY_PRESERVE_LAST: 保留最后一条 assistant 消息的首个 thinking 块
- STRATEGY_FORCE_CLEAN: 强制清洗所有 thinking 块
"""

import copy
from typing import TYPE_CHECKING, Any, Dict, List, Optional, Tuple

from src.core.logger import logger

if TYPE_CHECKING:
    from sqlalchemy.orm import Session


class ThinkingSanitizer:
    """
    Thinking 块清洗器

    负责检测和清洗消息中的 thinking 块，解决多供应商签名兼容性问题。
    """

    # 支持的清洗模式
    MODE_CONVERT_TO_TEXT = "convert_to_text"
    MODE_REMOVE = "remove"

    # 清洗策略
    STRATEGY_PRESERVE_LAST = "preserve_last"  # 保留最后一条 assistant 的首个 thinking
    STRATEGY_FORCE_CLEAN = "force_clean"      # 强制清洗所有 thinking

    def __init__(
        self,
        mode: Optional[str] = None,
        text_prefix: Optional[str] = None,
        text_suffix: Optional[str] = None,
        db: Optional["Session"] = None,
    ):
        """
        初始化清洗器

        Args:
            mode: 清洗模式，默认为 "remove"
            text_prefix: 转换为文本时的前缀，默认为 "[Thinking] "
            text_suffix: 转换为文本时的后缀，默认为空字符串
            db: 数据库会话（已废弃，保留参数签名向后兼容）
        """
        # 使用硬编码默认值，不再从配置读取
        self.mode = mode if mode is not None else self.MODE_REMOVE
        self.text_prefix = text_prefix if text_prefix is not None else "[Thinking] "
        self.text_suffix = text_suffix if text_suffix is not None else ""

    def has_thinking_blocks(self, messages: List[Dict[str, Any]]) -> bool:
        """
        检测消息列表中是否包含 thinking 或 redacted_thinking 块

        Args:
            messages: 消息列表

        Returns:
            是否包含 thinking 或 redacted_thinking 块
        """
        for message in messages:
            content = message.get("content")
            if isinstance(content, list):
                for block in content:
                    if isinstance(block, dict) and block.get("type") in ("thinking", "redacted_thinking"):
                        return True
        return False

    def sanitize_messages(
        self,
        messages: List[Dict[str, Any]],
        request_body: Optional[Dict[str, Any]] = None,
        strategy: Optional[str] = None,
    ) -> Tuple[List[Dict[str, Any]], int, bool, bool]:
        """
        清洗消息列表中的 thinking 块

        Args:
            messages: 原始消息列表
            request_body: 请求体，用于检测是否启用 thinking
            strategy: 清洗策略，默认根据 thinking 是否启用自动选择
                - STRATEGY_PRESERVE_LAST: 保留最后一条 assistant 的首个 thinking
                - STRATEGY_FORCE_CLEAN: 强制清洗所有 thinking

        Returns:
            Tuple[清洗后的消息列表, 清洗的 thinking 块数量, 是否需要禁用 thinking 参数, 是否有消息被修改]
        """
        if not messages:
            return messages, 0, False, False

        # 确定策略
        thinking_enabled = False
        if strategy is None:
            # 默认策略：启用 thinking 时保留，否则强制清洗
            thinking_enabled = self._is_thinking_enabled(request_body)
            strategy = self.STRATEGY_PRESERVE_LAST if thinking_enabled else self.STRATEGY_FORCE_CLEAN
        else:
            # 如果手动指定了策略，也需要检测 thinking 是否启用
            thinking_enabled = self._is_thinking_enabled(request_body)

        # 找到最后一条 assistant 消息的索引，并检测是否有 thinking 块
        # 注意：无论策略如何，都需要检测，用于判断是否需要禁用 thinking 参数
        last_assistant_idx = -1
        last_assistant_has_thinking = False
        should_disable_thinking = False  # 默认不禁用，在需要时设为 True

        for i in range(len(messages) - 1, -1, -1):
            if messages[i].get("role") == "assistant":
                last_assistant_idx = i
                # 检测该消息是否包含 thinking 或 redacted_thinking 块
                content = messages[i].get("content")
                if isinstance(content, list):
                    for block in content:
                        if isinstance(block, dict) and block.get("type") in ("thinking", "redacted_thinking"):
                            last_assistant_has_thinking = True
                            break
                break

        # 判断是否需要禁用 thinking 参数
        # 注意：只在 STRATEGY_FORCE_CLEAN（签名错误重试）时禁用 thinking
        # STRATEGY_PRESERVE_LAST 时不自动禁用，避免"死循环"（一旦禁用就永远无法恢复）
        #
        # 设计思路：
        # - FORCE_CLEAN（签名错误重试时显式传入）：清洗所有 thinking 块并禁用 thinking 参数
        # - PRESERVE_LAST + 最后一条有 thinking：保留它，尝试请求（可能签名错误，会触发重试）
        # - PRESERVE_LAST + 最后一条无 thinking：清洗中间的 thinking 块，但不禁用 thinking
        #   让请求正常发出，如果报 "Expected thinking" 错误，作为普通错误处理
        #   这样后续有机会恢复 thinking（当上游返回带有效 thinking 块的响应时）
        if thinking_enabled and last_assistant_idx >= 0:
            if strategy == self.STRATEGY_FORCE_CLEAN:
                # FORCE_CLEAN 会清洗所有 thinking 块（包括最后一条的）
                # 清洗后最后一条 assistant 必然没有 thinking 块，必须禁用 thinking
                should_disable_thinking = True
                logger.info("FORCE_CLEAN 策略将清洗所有 thinking 块，禁用 thinking 参数")
            elif not last_assistant_has_thinking:
                # PRESERVE_LAST 策略下，如果最后一条本身就没有 thinking 块
                # 切换为 FORCE_CLEAN 清洗中间的 thinking 块，但【不禁用 thinking】
                # 让请求尝试发出，避免"死循环"导致永远无法恢复 thinking
                strategy = self.STRATEGY_FORCE_CLEAN
                logger.info("最后一条 assistant 消息无 thinking 块，切换为强制清洗策略（不禁用 thinking，尝试恢复）")
            # else: PRESERVE_LAST 且最后一条有 thinking → 保留它，不禁用 thinking

        sanitized_count = 0
        any_reordered = False  # 新增：跟踪是否有任何消息发生重排
        result_messages = []

        for i, message in enumerate(messages):
            preserve_first = (strategy == self.STRATEGY_PRESERVE_LAST and i == last_assistant_idx)
            sanitized_message, count, reordered = self._sanitize_message(
                message,
                preserve_first_thinking=preserve_first,
                ensure_thinking_first=preserve_first,  # 确保 thinking 在首位
            )
            result_messages.append(sanitized_message)
            sanitized_count += count
            if reordered:
                any_reordered = True

        # 计算 messages_changed
        messages_changed = sanitized_count > 0 or should_disable_thinking or any_reordered

        if sanitized_count > 0 or should_disable_thinking or any_reordered or last_assistant_idx >= 0:
            logger.info(
                f"Thinking 清洗完成: mode={self.mode}, strategy={strategy}, "
                f"sanitized_count={sanitized_count}, "
                f"should_disable_thinking={should_disable_thinking}, "
                f"any_reordered={any_reordered}, "
                f"preserved_last_assistant={strategy == self.STRATEGY_PRESERVE_LAST and last_assistant_idx >= 0}"
            )

        return result_messages, sanitized_count, should_disable_thinking, messages_changed

    @staticmethod
    def _is_thinking_enabled(request_body: Optional[Dict[str, Any]]) -> bool:
        """
        检测请求是否启用了 Claude extended thinking

        Claude API 启用 thinking 的请求体格式：
        {
            "thinking": {
                "type": "enabled",
                "budget_tokens": ...
            }
        }

        Args:
            request_body: 请求体

        Returns:
            是否启用了 extended thinking
        """
        if not request_body or not isinstance(request_body, dict):
            return False
        thinking = request_body.get("thinking")
        if not isinstance(thinking, dict):
            return False
        return thinking.get("type") == "enabled"

    def _sanitize_message(
        self,
        message: Dict[str, Any],
        preserve_first_thinking: bool = False,
        ensure_thinking_first: bool = False,
    ) -> Tuple[Dict[str, Any], int, bool]:
        """
        清洗单条消息中的 thinking 块

        Args:
            message: 原始消息
            preserve_first_thinking: 是否保留第一个 thinking 块不清洗
            ensure_thinking_first: 是否确保 thinking 块在内容首位

        Returns:
            Tuple[清洗后的消息, 清洗的 thinking 块数量, 是否发生重排]
        """
        content = message.get("content")

        # 如果 content 不是列表，无需处理
        if not isinstance(content, list):
            return message, 0, False

        # 深拷贝以避免修改原始数据
        new_message = copy.deepcopy(message)
        new_content = []
        sanitized_count = 0
        first_thinking_preserved = False
        preserved_thinking_block = None  # 暂存需要移到首位的 thinking 块

        for block in new_message["content"]:
            if isinstance(block, dict) and block.get("type") in ("thinking", "redacted_thinking"):
                # 如果需要保留第一个 thinking/redacted_thinking 块且尚未保留
                if preserve_first_thinking and not first_thinking_preserved:
                    # 保留第一个 thinking 块
                    preserved_thinking_block = block
                    first_thinking_preserved = True
                    if not ensure_thinking_first:
                        # 如果不需要确保首位，直接添加
                        new_content.append(block)
                    continue

                sanitized_count += 1

                if self.mode == self.MODE_REMOVE:
                    # 移除模式：直接跳过此块
                    continue
                elif self.mode == self.MODE_CONVERT_TO_TEXT:
                    # 转换模式：转为文本块
                    # thinking 块内容在 "thinking" 字段，redacted_thinking 块内容在 "data" 字段
                    thinking_content = block.get("thinking", block.get("data", ""))
                    converted_block = {
                        "type": "text",
                        "text": f"{self.text_prefix}{thinking_content}{self.text_suffix}",
                    }
                    new_content.append(converted_block)
                else:
                    # 未知模式，默认保留原样（不清洗）
                    logger.warning(
                        f"未知的 thinking 清洗模式: {self.mode}，保留原始块"
                    )
                    new_content.append(block)
                    sanitized_count -= 1  # 撤销计数
            else:
                # 非 thinking 块，保留
                new_content.append(block)

        # 检测是否发生重排
        reordered = False
        if ensure_thinking_first and preserved_thinking_block is not None:
            # 检查原始消息中 thinking/redacted_thinking 块是否在首位
            original_first_is_thinking = (
                len(message.get("content", [])) > 0 and
                isinstance(message["content"][0], dict) and
                message["content"][0].get("type") in ("thinking", "redacted_thinking")
            )
            if not original_first_is_thinking:
                reordered = True
            new_content.insert(0, preserved_thinking_block)

        new_message["content"] = new_content
        return new_message, sanitized_count, reordered

    @staticmethod
    def disable_thinking_in_request(request_body: Dict[str, Any]) -> Dict[str, Any]:
        """
        从请求体中移除 thinking 参数

        用于签名错误 fallback 时，临时禁用 thinking 功能

        Args:
            request_body: 原始请求体

        Returns:
            移除 thinking 参数后的请求体（浅拷贝）
        """
        if not request_body or "thinking" not in request_body:
            return request_body

        new_body = dict(request_body)
        del new_body["thinking"]
        return new_body

    @staticmethod
    def should_sanitize(
        headers: Dict[str, str],
        body: Optional[Dict[str, Any]] = None,
        db: Optional["Session"] = None,
    ) -> bool:
        """
        判断是否需要清洗 thinking 块

        注意：此方法已废弃，预清洗功能已移除。
        新方案使用 ThinkingRectifier 在遇到错误时自动整流。

        此方法现在总是返回 False。

        Args:
            headers: 请求头
            body: 请求体（可选）
            db: 数据库会话（可选）

        Returns:
            总是返回 False（预清洗已禁用）
        """
        # 预清洗功能已移除，总是返回 False
        # 保留此方法签名以向后兼容
        return False


# 全局单例（使用默认配置）
default_sanitizer = ThinkingSanitizer()
