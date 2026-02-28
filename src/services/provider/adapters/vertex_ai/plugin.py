"""Vertex AI provider plugin — 统一注册入口。

注册 Vertex AI 对各通用 registry 的 hooks：
- Transport Hook (URL 构建，支持 API Key / Service Account 双策略)
- Behavior Variants (跨格式支持：同一 Provider 同时访问 Gemini 和 Claude)
"""

from __future__ import annotations


def register_all() -> None:
    """一次性注册 Vertex AI 的所有 hooks 到各通用 registry。"""
    from src.services.provider.adapters.vertex_ai.transport import build_vertex_ai_url
    from src.services.provider.behavior import register_behavior_variant
    from src.services.provider.transport import register_transport_hook

    # Transport: Vertex AI 同时支持 gemini:chat 和 claude:chat 格式
    register_transport_hook("vertex_ai", "gemini:chat", build_vertex_ai_url)
    register_transport_hook("vertex_ai", "claude:chat", build_vertex_ai_url)

    # Behavior: 跨格式支持（同一 Vertex AI Provider 可同时访问 Gemini 和 Claude 模型）
    register_behavior_variant("vertex_ai", cross_format=True)
