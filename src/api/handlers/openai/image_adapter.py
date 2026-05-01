"""
OpenAI Image Adapter - 基于 ImageAdapterBase 的 OpenAI Images Generations 适配器
"""

from __future__ import annotations

from src.api.handlers.base.image_adapter_base import (
    ImageAdapterBase,
    register_image_adapter,
)
from src.api.handlers.base.image_handler_base import ImageHandlerBase
from src.core.api_format import ApiFamily


@register_image_adapter
class OpenAIImageAdapter(ImageAdapterBase):
    FORMAT_ID = "openai:image"
    API_FAMILY = ApiFamily.OPENAI
    name = "openai.image"

    @property
    def HANDLER_CLASS(self) -> type[ImageHandlerBase]:
        from src.api.handlers.openai.image_handler import OpenAIImageHandler

        return OpenAIImageHandler


__all__ = ["OpenAIImageAdapter"]
