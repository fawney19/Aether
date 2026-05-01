"""
API Key / User 访问限制数据类型。

从 api/base/models_service.py 下沉到 core 层，
消除 services→api 的反向依赖。
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import TYPE_CHECKING

from src.core.api_format.signature import normalize_signature_key
from src.core.logger import logger
from src.core.user_access import resolve_user_access_config

if TYPE_CHECKING:
    from src.models.database import ApiKey, User


def _safe_normalize_signature(value: str) -> str:
    """归一化 endpoint signature，解析失败时原样返回（小写）。"""
    try:
        return normalize_signature_key(value)
    except ValueError:
        logger.warning("[AccessRestrictions] 无法归一化 API 格式 '{}', 原样使用小写形式", value)
        return value.strip().lower()


@dataclass
class AccessRestrictions:
    """API Key 或 User 的访问限制"""

    allowed_providers: list[str] | None = None  # 允许的 Provider ID 列表
    allowed_models: list[str] | None = None  # 允许的模型名称列表
    allowed_api_formats: list[str] | None = None  # 允许的 API 格式列表

    @classmethod
    def from_api_key_and_user(cls, api_key: ApiKey | None, user: User | None) -> AccessRestrictions:
        """
        从 API Key 和 User 合并访问限制

        限制逻辑:
        - API Key 的限制优先于 User 的限制
        - 如果 API Key 有限制，使用 API Key 的限制
        - 如果 API Key 无限制但 User 有限制，使用 User 的限制
        - 两者都无限制则返回空限制
        """
        allowed_providers: list[str] | None = None
        allowed_models: list[str] | None = None
        allowed_api_formats: list[str] | None = None

        # 优先使用 API Key 的限制
        if api_key:
            if api_key.allowed_providers is not None:
                allowed_providers = api_key.allowed_providers
            if api_key.allowed_models is not None:
                allowed_models = api_key.allowed_models
            if api_key.allowed_api_formats is not None:
                allowed_api_formats = api_key.allowed_api_formats

        # 如果 API Key 没有限制，检查 User 的限制
        if user:
            user_access = resolve_user_access_config(user)
            if allowed_providers is None and user_access.allowed_providers is not None:
                allowed_providers = user_access.allowed_providers
            if allowed_models is None and user_access.allowed_models is not None:
                allowed_models = user_access.allowed_models
            if allowed_api_formats is None and user_access.allowed_api_formats is not None:
                allowed_api_formats = user_access.allowed_api_formats

        return cls(
            allowed_providers=allowed_providers,
            allowed_models=allowed_models,
            allowed_api_formats=allowed_api_formats,
        )

    def is_api_format_allowed(self, api_format: str) -> bool:
        """
        检查 API 格式是否被允许

        Args:
            api_format: endpoint signature（如 "openai:chat"）

        Returns:
            True 如果格式被允许，False 否则
        """
        if self.allowed_api_formats is None:
            return True
        target = _safe_normalize_signature(api_format)
        allowed = {_safe_normalize_signature(f) for f in self.allowed_api_formats if f}
        return target in allowed

    def is_model_allowed(self, model_id: str, provider_id: str) -> bool:
        """
        检查模型是否被允许访问

        Args:
            model_id: 模型 ID
            provider_id: Provider ID

        Returns:
            True 如果模型被允许，False 否则
        """
        # 检查 Provider 限制
        if self.allowed_providers is not None:
            if provider_id not in self.allowed_providers:
                return False

        # 检查模型限制
        if self.allowed_models is not None:
            if model_id not in self.allowed_models:
                return False

        return True
