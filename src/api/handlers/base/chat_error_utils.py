"""
Chat Error Utils - Chat Handler 错误处理工具函数

从 chat_handler_base.py 提取的模块级工具函数，用于错误响应的构建和转换。
"""

from __future__ import annotations

import json
from typing import Any

from src.api.handlers.base.utils import get_format_converter_registry
from src.core.exceptions import ThinkingSignatureException, UpstreamClientException
from src.core.logger import logger
from src.models.database import ProviderAPIKey
from src.services.provider.adapters.vertex_ai.transport import get_effective_format as get_vertex_ai_effective_format
from src.services.scheduling.aware_scheduler import ProviderCandidate


def _get_error_status_code(e: Exception, default: int = 400) -> int:
    """从异常中提取 HTTP 状态码"""
    code = getattr(e, "status_code", None)
    return code if isinstance(code, int) and code > 0 else default


def _resolve_dynamic_format(
    key: ProviderAPIKey,
    auth_info: Any,
    model: str,
    provider_api_format: str,
    client_api_format: str,
    candidate: ProviderCandidate | None,
) -> tuple[str, bool]:
    """
    解析动态格式并计算 needs_conversion

    对于 Vertex AI 等跨格式 Provider，同一个项目可以访问 Gemini 和 Claude，
    但它们的请求/响应格式不同，需要根据模型名动态选择。
    用户可通过 auth_config.model_format_mapping 配置自定义映射。

    Args:
        key: Provider API Key
        auth_info: 认证信息（包含 decrypted_auth_config）
        model: 模型名
        provider_api_format: 当前 provider API 格式
        client_api_format: 客户端 API 格式
        candidate: Provider 候选（用于获取原始 needs_conversion）

    Returns:
        (effective_provider_format, needs_conversion) 元组
    """
    from src.core.provider_types import ProviderType

    # 判断是否为 Vertex AI provider（基于 provider_type 而非 auth_type）
    provider = getattr(key, "provider", None)
    provider_type = getattr(provider, "provider_type", None) if provider else None

    if provider_type == ProviderType.VERTEX_AI:
        vertex_auth_config = auth_info.decrypted_auth_config if auth_info else None
        effective_format = get_vertex_ai_effective_format(model, vertex_auth_config)
        if effective_format.upper() != provider_api_format.upper():
            logger.debug(
                f"Vertex AI 动态格式切换: {provider_api_format} -> {effective_format} "
                f"(model={model})"
            )
            provider_api_format = effective_format
        # Vertex AI 模式下，根据动态格式与客户端格式比较确定是否需要转换
        needs_conversion = provider_api_format.upper() != client_api_format.upper()
    else:
        # 非 Vertex AI：使用 candidate 的 needs_conversion
        needs_conversion = (
            bool(getattr(candidate, "needs_conversion", False)) if candidate else False
        )

    return provider_api_format, needs_conversion


def _convert_error_response_best_effort(
    error_response: dict[str, Any],
    source_format: str,
    target_format: str,
) -> dict[str, Any]:
    """
    将上游错误响应 best-effort 转换为客户端格式。

    说明：错误转换走 Canonical registry。转换失败时构造安全的通用错误响应，
    避免泄露上游原始错误详情。
    """
    try:
        registry = get_format_converter_registry()
        return registry.convert_error_response(error_response, source_format, target_format)
    except Exception as e:
        logger.debug(f"错误响应转换失败 ({source_format} -> {target_format}): {e}")
        # 转换失败时构造安全的通用错误，避免泄露上游详情
        return _build_client_error_response_best_effort("upstream error", target_format)


def _build_client_error_response_best_effort(
    message: str,
    target_format: str,
) -> dict[str, Any]:
    """
    当无法解析上游错误 body 时，构造一个目标格式的错误响应（best-effort）。
    """
    try:
        from src.core.api_format.conversion.internal import ErrorType, InternalError

        registry = get_format_converter_registry()
        normalizer = registry.get_normalizer(target_format)
        if normalizer and normalizer.capabilities.supports_error_conversion:
            return normalizer.error_from_internal(
                InternalError(type=ErrorType.INVALID_REQUEST, message=message, retryable=False)
            )
    except Exception as e:
        logger.debug(f"构建客户端错误响应失败 (target={target_format}): {e}")

    return {"error": {"type": "upstream_client_error", "message": message}}


def _build_error_json_payload(
    e: ThinkingSignatureException | UpstreamClientException,
    client_format: str,
    provider_format: str,
    needs_conversion: bool = True,
) -> dict[str, Any]:
    """
    构建错误 JSON 响应 payload（公共逻辑）。

    从异常中提取上游错误信息，尝试转换为客户端格式。

    Args:
        e: ThinkingSignatureException 或 UpstreamClientException
        client_format: 客户端 API 格式
        provider_format: Provider API 格式
        needs_conversion: 是否需要格式转换

    Returns:
        格式化的错误响应字典
    """
    raw = getattr(e, "upstream_error", None)
    message = getattr(e, "message", str(e))

    if isinstance(raw, str) and raw:
        try:
            parsed = json.loads(raw)
        except json.JSONDecodeError:
            parsed = None

        if isinstance(parsed, dict):
            if needs_conversion:
                return _convert_error_response_best_effort(parsed, provider_format, client_format)
            return parsed

    return _build_client_error_response_best_effort(message, client_format)
