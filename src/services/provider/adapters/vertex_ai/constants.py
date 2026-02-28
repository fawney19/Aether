"""Vertex AI 常量配置。

从 transport.py 迁移，集中管理 Vertex AI 模型格式映射和 region 配置。
"""

from __future__ import annotations

# Vertex AI 模型前缀到 API 格式的映射
# 用于 provider_type=vertex_ai 时，根据模型名动态确定实际的请求/响应格式
# 格式：前缀 -> endpoint signature（family:kind）
MODEL_FORMAT_MAPPING: dict[str, str] = {
    "claude-": "claude:chat",  # Anthropic Claude 模型
    "gemini-": "gemini:chat",  # Google Gemini 模型
    "imagen-": "gemini:chat",  # Google Imagen 模型（使用 Gemini chat 格式）
}

# Vertex AI 默认 endpoint signature（当模型前缀不匹配时）
DEFAULT_FORMAT: str = "gemini:chat"

# Vertex AI 模型默认 region 映射
# 用户可以通过 auth_config.model_regions 覆盖
DEFAULT_MODEL_REGIONS: dict[str, str] = {
    # Gemini 3 系列（使用 global）
    "gemini-3-pro-image-preview": "global",
    # Gemini 2.0 系列
    "gemini-2.0-flash": "us-central1",
    "gemini-2.0-flash-exp": "us-central1",
    "gemini-2.0-flash-001": "us-central1",
    "gemini-2.0-pro-exp": "us-central1",
    "gemini-2.0-flash-exp-image-generation": "us-central1",
    # Gemini 1.5 系列
    "gemini-1.5-pro": "us-central1",
    "gemini-1.5-pro-001": "us-central1",
    "gemini-1.5-pro-002": "us-central1",
    "gemini-1.5-flash": "us-central1",
    "gemini-1.5-flash-001": "us-central1",
    "gemini-1.5-flash-002": "us-central1",
    # Imagen 系列
    "imagen-3.0-generate-001": "us-central1",
    "imagen-3.0-fast-generate-001": "us-central1",
}

# API Key 认证的全局端点
API_KEY_BASE_URL = "https://aiplatform.googleapis.com"
