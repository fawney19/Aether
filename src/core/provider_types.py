"""Provider type 枚举与工具函数。

所有 provider_type 相关的比较、判断应使用此模块中的 ProviderType 枚举，
避免到处散落字符串字面量。

由于 ProviderType 继承自 str，``ProviderType.ANTIGRAVITY == "antigravity"``
始终为 True，因此与已有数据库值、序列化格式完全兼容。
"""

from __future__ import annotations

from enum import Enum


class ProviderType(str, Enum):
    """已支持的 Provider 类型。"""

    CUSTOM = "custom"
    CLAUDE_CODE = "claude_code"
    KIRO = "kiro"
    CODEX = "codex"
    GEMINI_CLI = "gemini_cli"
    ANTIGRAVITY = "antigravity"
    VERTEX_AI = "vertex_ai"


# 所有有效 provider_type 值的集合（用于校验）
VALID_PROVIDER_TYPES: frozenset[str] = frozenset(pt.value for pt in ProviderType)


def normalize_provider_type(value: object) -> str:
    """将任意输入规范化为小写 provider_type 字符串。

    统一处理 ``str(getattr(provider, "provider_type", "") or "").strip().lower()``
    这类散落在各处的 normalize 逻辑。
    """
    if isinstance(value, ProviderType):
        return value.value
    return str(value or "").strip().lower()


__all__ = [
    "VALID_PROVIDER_TYPES",
    "ProviderType",
    "normalize_provider_type",
]
