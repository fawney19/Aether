"""
Claude Token计数插件
专门为Claude模型设计的Token计数器
"""

from __future__ import annotations

import re
from typing import Any

from src.utils import json_helper as json

from .base import TokenCounterPlugin

_CJK_PATTERN = re.compile(r"[\u4e00-\u9fff\u3040-\u30ff\uac00-\ud7af]")


class ClaudeTokenCounterPlugin(TokenCounterPlugin):
    """
    Claude专用Token计数插件
    使用简化的估算方法
    """

    # Claude模型信息
    CLAUDE_MODELS = {
        "claude-3-5-sonnet-20241022": {
            "max_tokens": 200000,
            "max_output": 8192,
            "chars_per_token": 3.5,  # 平均字符/token比例
        },
        "claude-3-5-haiku-20241022": {
            "max_tokens": 200000,
            "max_output": 8192,
            "chars_per_token": 3.5,
        },
        "claude-3-opus-20240229": {
            "max_tokens": 200000,
            "max_output": 4096,
            "chars_per_token": 3.5,
        },
        "claude-3-sonnet-20240229": {
            "max_tokens": 200000,
            "max_output": 4096,
            "chars_per_token": 3.5,
        },
        "claude-3-haiku-20240307": {
            "max_tokens": 200000,
            "max_output": 4096,
            "chars_per_token": 3.5,
        },
        # 旧版模型
        "claude-2.1": {
            "max_tokens": 100000,
            "max_output": 4096,
            "chars_per_token": 4,
        },
        "claude-2.0": {
            "max_tokens": 100000,
            "max_output": 4096,
            "chars_per_token": 4,
        },
        "claude-instant-1.2": {
            "max_tokens": 100000,
            "max_output": 4096,
            "chars_per_token": 4,
        },
    }

    def __init__(self, name: str = "claude", config: dict[str, Any] | None = None):
        super().__init__(name, config)

        # 价格表（每1M tokens的价格 USD）
        default_pricing = {
            "claude-3-5-sonnet": {
                "input": 3,
                "output": 15,
                "cache_write": 3.75,  # 缓存写入
                "cache_read": 0.30,  # 缓存读取
            },
            "claude-3-5-haiku": {
                "input": 0.8,
                "output": 4,
                "cache_write": 1,
                "cache_read": 0.08,
            },
            "claude-3-opus": {
                "input": 15,
                "output": 75,
                "cache_write": 18.75,
                "cache_read": 1.50,
            },
            "claude-3-sonnet": {
                "input": 3,
                "output": 15,
                "cache_write": 3.75,
                "cache_read": 0.30,
            },
            "claude-3-haiku": {
                "input": 0.25,
                "output": 1.25,
                "cache_write": 0.30,
                "cache_read": 0.03,
            },
            "claude-2.1": {
                "input": 8,
                "output": 24,
            },
            "claude-2.0": {
                "input": 8,
                "output": 24,
            },
            "claude-instant": {
                "input": 0.8,
                "output": 2.4,
            },
        }
        self.config["pricing"] = (
            config.get("pricing", default_pricing) if config else default_pricing
        )

    def supports_model(self, model: str) -> bool:
        """检查是否支持指定模型"""
        # 支持所有Claude模型
        return "claude" in model.lower()

    def _estimate_tokens_from_text(self, text: str, model: str) -> int:
        """从文本估算Token数量"""
        # 获取模型信息
        model_info = None
        for model_name, info in self.CLAUDE_MODELS.items():
            if model.startswith(model_name.split("-20")[0]):  # 匹配基本名称
                model_info = info
                break

        if not model_info:
            # 默认值
            model_info = {"chars_per_token": 3.5}

        # 基本估算
        chars_per_token = model_info["chars_per_token"]

        # 考虑不同语言的特点
        # 检测是否包含中文/日文/韩文
        cjk_count = len(_CJK_PATTERN.findall(text))

        if cjk_count > len(text) * 0.3:  # 超过30%是CJK字符
            # CJK字符通常每个字符1-2个token
            return int(len(text) / 1.5)
        else:
            # 英文和其他语言
            # 考虑空格和标点
            word_count = len(text.split())
            # 平均每个单词1.3个token
            token_by_words = int(word_count * 1.3)
            # 平均每个字符chars_per_token
            token_by_chars = int(len(text) / chars_per_token)
            # 取两者的平均
            return (token_by_words + token_by_chars) // 2

    async def count_tokens(self, text: str, model: str | None = None) -> int:
        """计算文本的Token数量"""
        if not self.enabled:
            return 0

        model = model or self.default_model or "claude-3-5-sonnet-20241022"
        return self._estimate_tokens_from_text(text, model)

    async def count_messages(self, messages: list[dict[str, Any]], model: str | None = None) -> int:
        """计算消息列表的Token数量"""
        if not self.enabled:
            return 0

        model = model or self.default_model or "claude-3-5-sonnet-20241022"
        total_tokens = 0

        for message in messages:
            # 角色token（约3 tokens）
            total_tokens += 3

            # 内容token
            content = message.get("content")
            if content:
                if isinstance(content, str):
                    total_tokens += self._estimate_tokens_from_text(content, model)
                elif isinstance(content, list):
                    # 处理多模态内容
                    for item in content:
                        if item.get("type") == "text":
                            text = item.get("text", "")
                            total_tokens += self._estimate_tokens_from_text(text, model)
                        elif item.get("type") == "image":
                            # Claude图像处理
                            # 基础: 1,600 tokens
                            # 每个256x256的tile: 280 tokens
                            # 简化估算
                            total_tokens += 2000  # 平均估算
                        elif item.get("type") == "tool_use":
                            # 工具使用
                            tool_name = item.get("name", "")
                            tool_input = item.get("input", {})
                            total_tokens += self._estimate_tokens_from_text(tool_name, model)
                            total_tokens += self._estimate_tokens_from_text(
                                json.dumps(tool_input), model
                            )
                        elif item.get("type") == "tool_result":
                            # 工具结果
                            tool_content = item.get("content", "")
                            if isinstance(tool_content, str):
                                total_tokens += self._estimate_tokens_from_text(tool_content, model)

        # 添加系统提示的token（如果有）
        if messages and messages[0].get("role") == "system":
            # 系统提示通常会有额外的开销
            total_tokens += 10

        return total_tokens

    async def count_request(self, request: dict[str, Any], model: str | None = None) -> int:
        """计算请求的Token数量"""
        model = model or request.get("model") or self.default_model
        messages = request.get("messages", [])
        total = await self.count_messages(messages, model)

        # 考虑系统提示
        system = request.get("system")
        if system:
            total += self._estimate_tokens_from_text(system, model)
            total += 5  # 系统提示的额外开销

        return total

    async def get_model_info(self, model: str) -> dict[str, Any]:
        """获取模型信息"""
        info = {"model": model, "supported": self.supports_model(model)}

        if self.supports_model(model):
            # 查找匹配的模型信息
            model_info = None
            for model_name, m_info in self.CLAUDE_MODELS.items():
                if model.startswith(model_name.split("-20")[0]):
                    model_info = m_info
                    info["model_name"] = model_name
                    break

            if model_info:
                info.update(
                    {
                        "max_tokens": model_info["max_tokens"],
                        "max_output": model_info["max_output"],
                        "chars_per_token": model_info["chars_per_token"],
                        "supports_vision": "claude-3" in model,
                        "supports_tools": "claude-3" in model,
                        "supports_cache": "claude-3" in model,
                    }
                )

            # 添加价格信息
            pricing = self.config.get("pricing", {})
            for price_key in pricing:
                if model.startswith(price_key):
                    info["pricing"] = pricing[price_key]
                    break

        return info

    async def get_stats(self) -> dict[str, Any]:
        """获取统计信息"""
        stats = await super().get_stats()
        stats.update(
            {
                "estimation_method": "character_based",
                "supported_models_count": len(self.CLAUDE_MODELS),
            }
        )
        return stats
