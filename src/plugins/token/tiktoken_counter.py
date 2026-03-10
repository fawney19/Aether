"""
Tiktoken Token计数插件
支持OpenAI和其他使用tiktoken的模型
"""

from __future__ import annotations

from functools import lru_cache
from typing import Any

from src.core.logger import logger

from .base import TokenCounterPlugin

# 尝试导入tiktoken
try:
    import tiktoken

    TIKTOKEN_AVAILABLE = True
except ImportError:  # pragma: no cover
    TIKTOKEN_AVAILABLE = False
    tiktoken = None


@lru_cache(maxsize=4)
def _get_encoder_cached(model: str) -> Any:
    """全局编码器缓存。

    目的：避免在多实例/多请求场景下重复初始化 tiktoken 编码器。
    实际只有 cl100k_base / o200k_base / p50k_base 等少数几种编码，4 个足够。
    """
    if not TIKTOKEN_AVAILABLE:
        raise RuntimeError("tiktoken not installed")

    mapping = TiktokenCounterPlugin.MODEL_ENCODINGS

    # 1) 完全匹配
    if model in mapping:
        return tiktoken.get_encoding(mapping[model])

    # 2) 前缀匹配（按前缀长度从长到短，避免短前缀抢先匹配）
    for model_prefix, enc_name in TiktokenCounterPlugin.MODEL_ENCODINGS_PREFIXES:
        if model.startswith(model_prefix):
            return tiktoken.get_encoding(enc_name)

    # 3) 尝试使用模型名称
    try:
        return tiktoken.encoding_for_model(model)
    except Exception:
        # 默认使用 cl100k_base
        return tiktoken.get_encoding("cl100k_base")


class TiktokenCounterPlugin(TokenCounterPlugin):
    """
    使用tiktoken库计算Token数量
    支持OpenAI模型和其他兼容模型
    """

    # 模型编码映射
    MODEL_ENCODINGS = {
        # GPT-4 系列
        "gpt-4": "cl100k_base",
        "gpt-4-32k": "cl100k_base",
        "gpt-4-turbo": "cl100k_base",
        "gpt-4-turbo-preview": "cl100k_base",
        "gpt-4o": "o200k_base",
        "gpt-4o-mini": "o200k_base",
        # GPT-3.5 系列
        "gpt-3.5-turbo": "cl100k_base",
        "gpt-3.5-turbo-16k": "cl100k_base",
        # 旧模型
        "text-davinci-003": "p50k_base",
        "text-davinci-002": "p50k_base",
        "code-davinci-002": "p50k_base",
        # Embeddings
        "text-embedding-ada-002": "cl100k_base",
        "text-embedding-3-small": "cl100k_base",
        "text-embedding-3-large": "cl100k_base",
    }

    # 前缀匹配顺序（从长到短）
    MODEL_ENCODINGS_PREFIXES = sorted(
        MODEL_ENCODINGS.items(),
        key=lambda kv: len(kv[0]),
        reverse=True,
    )

    # 每个消息的额外Token数
    MESSAGE_OVERHEAD = {
        "gpt-3.5-turbo": 4,  # 每条消息
        "gpt-4": 3,
        "gpt-4-turbo": 3,
        "gpt-4o": 3,
        "gpt-4o-mini": 3,
    }

    def __init__(self, name: str = "tiktoken", config: dict[str, Any] | None = None):
        super().__init__(name, config)

        if not TIKTOKEN_AVAILABLE:
            self.enabled = False
            logger.warning("tiktoken not installed, plugin disabled")
            return

        # 缓存编码器
        self._encoders = {}

        # 价格表（每1M tokens的价格 USD）
        default_pricing = {
            "gpt-4o": {"input": 2.5, "output": 10},
            "gpt-4o-mini": {"input": 0.15, "output": 0.6},
            "gpt-4-turbo": {"input": 10, "output": 30},
            "gpt-4": {"input": 30, "output": 60},
            "gpt-3.5-turbo": {"input": 0.5, "output": 1.5},
            "o1-preview": {"input": 15, "output": 60, "reasoning": 60},
            "o1-mini": {"input": 3, "output": 12, "reasoning": 12},
        }
        self.config["pricing"] = (
            config.get("pricing", default_pricing) if config else default_pricing
        )

    def _get_encoder(self, model: str) -> Any:
        """获取模型的编码器（全局缓存）"""
        return _get_encoder_cached(model)

    def supports_model(self, model: str) -> bool:
        """检查是否支持指定模型"""
        # 支持所有OpenAI模型和一些兼容模型
        openai_models = [
            "gpt-4",
            "gpt-3.5",
            "text-davinci",
            "text-embedding",
            "code-davinci",
            "o1",
        ]
        return any(model.startswith(prefix) for prefix in openai_models)

    async def count_tokens(self, text: str, model: str | None = None) -> int:
        """计算文本的Token数量"""
        if not self.enabled:
            return 0

        model = model or self.default_model or "gpt-3.5-turbo"
        encoder = self._get_encoder(model)

        try:
            tokens = encoder.encode(text)
            return len(tokens)
        except Exception as e:
            logger.warning(f"Error counting tokens: {e}")
            # 简单估算: 平均每个字符0.75个token
            return int(len(text) * 0.75)

    async def count_messages(self, messages: list[dict[str, Any]], model: str | None = None) -> int:
        """计算消息列表的Token数量"""
        if not self.enabled:
            return 0

        model = model or self.default_model or "gpt-3.5-turbo"
        encoder = self._get_encoder(model)

        # 获取每条消息的额外token数
        msg_overhead = self.MESSAGE_OVERHEAD.get(model, 3)

        total_tokens = 0

        for message in messages:
            # 每条消息的基本token
            total_tokens += msg_overhead

            # 角色token
            role = message.get("role", "")
            if role:
                total_tokens += len(encoder.encode(role))

            # 内容token
            content = message.get("content")
            if content:
                if isinstance(content, str):
                    total_tokens += len(encoder.encode(content))
                elif isinstance(content, list):
                    # 处理多模态内容
                    for item in content:
                        if item.get("type") == "text":
                            text = item.get("text", "")
                            total_tokens += len(encoder.encode(text))
                        elif item.get("type") == "image_url":
                            # 图像的token计算更复杂，这里简化处理
                            # 低分辨率: 85 tokens, 高分辨率: 170 tokens
                            detail = item.get("image_url", {}).get("detail", "auto")
                            total_tokens += 170 if detail == "high" else 85

            # 名称token
            name = message.get("name")
            if name:
                total_tokens += len(encoder.encode(name)) - 1  # name会减去1个token

            # 工具调用
            tool_calls = message.get("tool_calls")
            if tool_calls:
                for tool_call in tool_calls:
                    # 工具ID
                    if "id" in tool_call:
                        total_tokens += len(encoder.encode(tool_call["id"]))

                    # 函数信息
                    function = tool_call.get("function", {})
                    if "name" in function:
                        total_tokens += len(encoder.encode(function["name"]))
                    if "arguments" in function:
                        total_tokens += len(encoder.encode(function["arguments"]))

        # 添加固定的结束标记
        total_tokens += 3

        return total_tokens

    async def get_model_info(self, model: str) -> dict[str, Any]:
        """获取模型信息"""
        info = {"model": model, "supported": self.supports_model(model)}

        if self.supports_model(model):
            # 获取编码信息
            encoder = self._get_encoder(model)
            encoding_name = None

            # 找到编码名称
            for m, enc in self.MODEL_ENCODINGS.items():
                if model.startswith(m):
                    encoding_name = enc
                    break

            info.update(
                {
                    "encoding": encoding_name or "unknown",
                    "vocab_size": encoder.n_vocab if hasattr(encoder, "n_vocab") else None,
                    "max_tokens": self._get_max_tokens(model),
                    "message_overhead": self.MESSAGE_OVERHEAD.get(model, 3),
                }
            )

            # 添加价格信息
            pricing = self.config.get("pricing", {})
            if model in pricing:
                info["pricing"] = pricing[model]

        return info

    def _get_max_tokens(self, model: str) -> int:
        """获取模型的最大token数"""
        max_tokens_map = {
            "gpt-4": 8192,
            "gpt-4-32k": 32768,
            "gpt-4-turbo": 128000,
            "gpt-4o": 128000,
            "gpt-4o-mini": 128000,
            "gpt-3.5-turbo": 4096,
            "gpt-3.5-turbo-16k": 16384,
            "o1-preview": 128000,
            "o1-mini": 128000,
        }

        # 完全匹配
        if model in max_tokens_map:
            return max_tokens_map[model]

        # 前缀匹配
        for model_prefix, max_tokens in max_tokens_map.items():
            if model.startswith(model_prefix):
                return max_tokens

        # 默认值
        return 4096

    async def get_stats(self) -> dict[str, Any]:
        """获取统计信息"""
        stats = await super().get_stats()
        stats.update(
            {
                "encoders_cached": _get_encoder_cached.cache_info().currsize,
                "tiktoken_available": TIKTOKEN_AVAILABLE,
            }
        )
        return stats
