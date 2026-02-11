"""
错误分类器

负责错误分类和处理策略决定
"""

from __future__ import annotations

from enum import Enum
from typing import Any

import httpx
from sqlalchemy.orm import Session

from src.core.api_format.signature import make_signature_key
from src.core.crypto import CryptoService
from src.core.exceptions import (
    ConcurrencyLimitError,
    ProviderAuthException,
    ProviderCompatibilityException,
    ProviderException,
    ProviderNotAvailableException,
    ProviderRateLimitException,
    ThinkingSignatureException,
    UpstreamClientException,
)
from src.core.logger import logger
from src.models.database import Provider, ProviderAPIKey, ProviderEndpoint
from src.services.cache.aware_scheduler import CacheAwareScheduler
from src.services.health.monitor import health_monitor
from src.services.provider.format import normalize_endpoint_signature
from src.services.rate_limit.adaptive_rpm import get_adaptive_rpm_manager
from src.services.rate_limit.detector import RateLimitType, detect_rate_limit_type
from src.utils import json_helper as json


class ErrorAction(Enum):
    """错误处理动作"""

    CONTINUE = "continue"  # 继续重试当前候选
    BREAK = "break"  # 跳到下一个候选
    RAISE = "raise"  # 直接抛出异常


class ErrorClassifier:
    """
    错误分类器 - 负责错误分类和处理策略

    职责：
    1. 将错误分类为可重试/不可重试
    2. 决定错误后的处理动作（重试/切换/放弃）
    3. 处理特定类型的错误（如 429 限流）
    4. 更新健康状态和缓存亲和性
    """

    # 需要触发故障转移的错误类型
    RETRIABLE_ERRORS: tuple[type, ...] = (
        ProviderException,  # 包含所有 Provider 异常子类
        ConnectionError,  # Python 标准连接错误
        TimeoutError,  # Python 标准超时错误
        httpx.TransportError,  # HTTPX 传输错误
    )

    # 不可重试的错误类型（直接抛出）
    NON_RETRIABLE_ERRORS: tuple[type, ...] = (
        ValueError,  # 参数错误
        TypeError,  # 类型错误
        KeyError,  # 键错误
        UpstreamClientException,  # 上游客户端错误
    )

    # 表示客户端请求错误的关键词（不区分大小写）
    # 这些错误是由用户请求本身导致的，换 Provider 也无济于事
    # 注意：标准 API 返回的 error.type 已在 CLIENT_ERROR_TYPES 中处理
    # 这里主要用于匹配非标准格式或第三方代理的错误消息
    #
    # 重要：不要在此列表中包含 Provider Key 配置问题（如 invalid_api_key）
    # 这类错误应该触发故障转移，而不是直接返回给用户
    CLIENT_ERROR_PATTERNS: tuple[str, ...] = (
        "could not process image",  # 图片处理失败
        "image too large",  # 图片过大
        "invalid image",  # 无效图片
        "unsupported image",  # 不支持的图片格式
        "content_policy_violation",  # 内容违规
        "context_length_exceeded",  # 上下文长度超限
        "content_length_limit",  # 请求内容长度超限 (Claude API)
        "content_length_exceeds",  # 内容长度超限变体 (AWS CodeWhisperer)
        # 注意：移除了 "max_tokens"，因为 max_tokens 相关错误可能是 Provider 兼容性问题
        # 如 "Unsupported parameter: 'max_tokens' is not supported with this model"
        # 这类错误应由 COMPATIBILITY_ERROR_PATTERNS 处理
        "invalid_prompt",  # 无效的提示词
        "content too long",  # 内容过长
        "input is too long",  # 输入过长 (AWS)
        "message is too long",  # 消息过长
        "prompt is too long",  # Prompt 超长（第三方代理常见格式）
        "image exceeds",  # 图片超出限制
        "pdf too large",  # PDF 过大
        "file too large",  # 文件过大
        "tool_use_id",  # tool_result 引用了不存在的 tool_use（兼容非标准代理）
        "validationexception",  # AWS 验证异常
    )

    def __init__(
        self,
        db: Session,
        adaptive_manager: Any | None = None,
        cache_scheduler: CacheAwareScheduler | None = None,
    ) -> None:
        """
        初始化错误分类器

        Args:
            db: 数据库会话
            adaptive_manager: 自适应并发管理器
            cache_scheduler: 缓存调度器（可选）
        """
        self.db = db
        self.adaptive_manager = adaptive_manager or get_adaptive_rpm_manager()
        self.cache_scheduler = cache_scheduler

    def _extract_oauth_email(self, key: ProviderAPIKey | None) -> str | None:
        if not key or str(getattr(key, "auth_type", "") or "").lower() != "oauth":
            return None
        encrypted_auth_config = getattr(key, "auth_config", None)
        if not encrypted_auth_config:
            return None
        try:
            decrypted = CryptoService().decrypt(encrypted_auth_config, silent=True)
            auth_config = json.loads(decrypted) if decrypted else {}
        except Exception:
            return None
        email = auth_config.get("email")
        if isinstance(email, str):
            email = email.strip()
            if email:
                return email
        return None

    def _format_key_display(self, key: ProviderAPIKey | None) -> str:
        if not key:
            return "key=unknown"
        key_id = str(getattr(key, "id", "") or "")[:8] or "unknown"
        name = str(getattr(key, "name", "") or "").strip()
        email = self._extract_oauth_email(key)
        parts = [f"key={key_id}"]
        if email:
            parts.append(f"email={email}")
        if name and name != email:
            parts.append(f"name={name}")
        return " ".join(parts)

    # 表示客户端错误的 error type（不区分大小写）
    # 这些 type 表明是请求本身的问题，不应重试
    CLIENT_ERROR_TYPES: tuple[str, ...] = (
        # Claude/OpenAI 标准
        "invalid_request_error",
        # Gemini
        "invalid_argument",
        "failed_precondition",
        # AWS
        "validationexception",
        # 通用
        "validation_error",
        "bad_request",
    )

    # 表示客户端错误的 reason/code 字段值
    CLIENT_ERROR_REASONS: tuple[str, ...] = (
        "CONTENT_LENGTH_EXCEEDS_THRESHOLD",
        "CONTEXT_LENGTH_EXCEEDED",
        "MAX_TOKENS_EXCEEDED",
        "INVALID_CONTENT",
        "CONTENT_POLICY_VIOLATION",
    )

    # Provider 兼容性错误模式 - 这类错误应该触发故障转移
    # 因为换一个 Provider 可能就能成功
    COMPATIBILITY_ERROR_PATTERNS: tuple[str, ...] = (
        "unsupported parameter",  # 不支持的参数
        "unsupported model",  # 不支持的模型
        "unsupported feature",  # 不支持的功能
        "not supported with this model",  # 此模型不支持
        "model does not support",  # 模型不支持
        "parameter is not supported",  # 参数不支持
        "feature is not supported",  # 功能不支持
        "not available for this model",  # 此模型不可用
    )

    # Thinking 块相关错误模式 - 这类错误需要清洗 thinking 块或调整请求
    # 场景：多供应商环境下，Provider A 生成的 thinking 块被发送到 Provider B 时签名验证失败
    THINKING_ERROR_PATTERNS: tuple[str, ...] = (
        # 签名错误：跨 Provider 发送 thinking 块时，签名无法被目标 Provider 验证
        # 例: "invalid `signature` in `thinking` block: signature is for a different request"
        "invalid `signature` in `thinking` block",
        "invalid signature in thinking block",
        # 签名字段缺失或格式错误
        # 例: "messages.0.content.0.thinking.signature: field required"
        "thinking.signature: field required",
        "thinking.signature:",  # 匹配路径模式 messages.X.content.X.thinking.signature: xxx
        "signature verification failed",
        # 结构错误：启用 thinking 时，有 tool_use 的 assistant 消息必须以 thinking 块开头
        # 例: "when `thinking` is enabled, the first content block ... must start with a `thinking` block"
        "must start with a thinking block",
        # 例: "expected thinking or redacted_thinking, found tool_use"
        "expected thinking or redacted_thinking",
        "expected `thinking`",
        "expected thinking, found",  # 统一匹配 "found tool_use/text" 等变体
        "expected `thinking`, found",  # 带反引号变体
        "expected redacted_thinking, found",
        "expected `redacted_thinking`, found",
        # Antigravity / Gemini-internal: thought signature validation
        "thoughtsignature",
        "thought_signature",
    )

    def _parse_error_response(self, error_text: str | None) -> dict[str, Any]:
        """
        解析错误响应为结构化数据

        支持多种格式:
        - {"error": {"type": "...", "message": "..."}}  (Claude/OpenAI)
        - {"error": {"message": "...", "__type": "..."}}  (AWS)
        - {"errorMessage": "..."}  (Lambda)
        - {"error": "..."}
        - {"message": "...", "reason": "..."}

        Returns:
            结构化的错误信息: {
                "type": str,      # 错误类型
                "message": str,   # 错误消息
                "reason": str,    # 错误原因/代码
                "raw": str,       # 原始文本
            }
        """
        result = {"type": "", "message": "", "reason": "", "raw": error_text or ""}

        if not error_text:
            return result

        try:
            data = json.loads(error_text)

            # 格式 1: {"error": {"type": "...", "message": "..."}}
            if isinstance(data.get("error"), dict):
                error_obj = data["error"]
                result["type"] = str(error_obj.get("type", ""))
                result["message"] = str(error_obj.get("message", ""))

                # AWS 格式: {"error": {"__type": "...", "message": "...", "reason": "..."}}
                # __type 直接在 error 对象中，而不是嵌套在 message 里
                if "__type" in error_obj:
                    result["type"] = result["type"] or str(error_obj.get("__type", ""))
                if "reason" in error_obj:
                    result["reason"] = str(error_obj.get("reason", ""))
                if "code" in error_obj:
                    result["reason"] = result["reason"] or str(error_obj.get("code", ""))

                # 嵌套 JSON 格式: message 字段本身是 JSON 字符串
                # 支持多种嵌套格式：
                # - AWS: {"__type": "...", "message": "...", "reason": "..."}
                # - 第三方代理: {"error": {"type": "...", "message": "..."}}
                if result["message"].startswith("{"):
                    try:
                        nested = json.loads(result["message"])
                        if isinstance(nested, dict):
                            # AWS 格式
                            if "__type" in nested:
                                result["type"] = result["type"] or str(nested.get("__type", ""))
                                result["message"] = str(nested.get("message", result["message"]))
                                result["reason"] = str(nested.get("reason", ""))
                            # 第三方代理格式: {"error": {"message": "..."}}
                            elif isinstance(nested.get("error"), dict):
                                inner_error = nested["error"]
                                inner_msg = str(inner_error.get("message", ""))
                                if inner_msg:
                                    result["message"] = inner_msg
                            # 简单格式: {"message": "..."}
                            elif "message" in nested:
                                result["message"] = str(nested["message"])
                    except json.JSONDecodeError:
                        pass

            # 格式 2: {"error": "..."}
            elif isinstance(data.get("error"), str):
                result["message"] = str(data["error"])

            # 格式 3: {"errorMessage": "..."}  (Lambda)
            elif "errorMessage" in data:
                result["message"] = str(data["errorMessage"])

            # 格式 4: {"message": "...", "reason": "..."}
            elif "message" in data:
                result["message"] = str(data["message"])
                result["reason"] = str(data.get("reason", ""))

            # 提取顶层的 reason/code
            if not result["reason"]:
                result["reason"] = str(data.get("reason", data.get("code", "")))

        except (json.JSONDecodeError, TypeError, KeyError):
            result["message"] = error_text

        return result

    def is_client_error(self, error_text: str | None) -> bool:
        """
        检测错误响应是否为客户端错误（不应重试）

        判断逻辑（按优先级）：
        1. 检查 error.type 是否为已知的客户端错误类型
        2. 检查 reason/code 是否为已知的客户端错误原因
        3. 回退到关键词匹配

        Args:
            error_text: 错误响应文本

        Returns:
            是否为客户端错误
        """
        if not error_text:
            return False

        parsed = self._parse_error_response(error_text)

        # 1. 检查 error type
        if parsed["type"]:
            error_type_lower = parsed["type"].lower()
            if any(t.lower() in error_type_lower for t in self.CLIENT_ERROR_TYPES):
                return True

        # 2. 检查 reason/code
        if parsed["reason"]:
            reason_upper = parsed["reason"].upper()
            if any(r in reason_upper for r in self.CLIENT_ERROR_REASONS):
                return True

        # 3. 回退到关键词匹配（合并 message 和 raw）
        search_text = f"{parsed['message']} {parsed['raw']}".lower()
        return any(pattern.lower() in search_text for pattern in self.CLIENT_ERROR_PATTERNS)

    def _is_compatibility_error(self, error_text: str | None) -> bool:
        """
        检测错误响应是否为 Provider 兼容性错误（应触发故障转移）

        这类错误是因为 Provider 不支持某些参数或功能导致的，
        换一个 Provider 可能就能成功。

        Args:
            error_text: 错误响应文本

        Returns:
            是否为兼容性错误
        """
        if not error_text:
            return False

        search_text = error_text.lower()
        return any(pattern.lower() in search_text for pattern in self.COMPATIBILITY_ERROR_PATTERNS)

    def _is_thinking_error(self, error_text: str | None) -> bool:
        """
        检测错误响应是否为 Thinking 块相关错误（签名错误或结构错误）

        这类错误通常发生在：
        1. 多供应商场景下，当一个供应商生成的 thinking 块被发送到另一个供应商时，签名验证会失败
        2. 请求体中有 tool_use 但没有以 thinking 块开头时，Claude 会报结构错误

        Args:
            error_text: 错误响应文本

        Returns:
            是否为 Thinking 相关错误
        """
        if not error_text:
            return False
        search_text = error_text.lower()
        return any(p.lower() in search_text for p in self.THINKING_ERROR_PATTERNS)

    def _is_account_validation_required(self, error_text: str | None) -> bool:
        """
        检测 403 错误是否为 Google 账号验证要求 (VALIDATION_REQUIRED)

        Google 会在某些情况下要求账号所有者手动完成人机验证，
        此时所有 API 请求都会返回 403 + VALIDATION_REQUIRED。
        这是账号级别的永久性错误，重试无法修复，需要人工干预。

        匹配条件（满足任一即可）：
        - error.details 中包含 reason=VALIDATION_REQUIRED
        - error.status 为 PERMISSION_DENIED 且 message 包含 "verify your account"
        - error.message 包含 "verify your account"

        Args:
            error_text: 错误响应文本

        Returns:
            是否为账号验证要求错误
        """
        if not error_text:
            return False

        search_text = error_text.lower()

        # 快速路径：关键词匹配
        if "validation_required" in search_text:
            return True
        if "verify your account" in search_text and "permission_denied" in search_text:
            return True

        return False

    def _extract_error_message(self, error_text: str | None) -> str | None:
        """
        从错误响应中提取错误消息

        Args:
            error_text: 错误响应文本

        Returns:
            提取的错误消息
        """
        if not error_text:
            return None

        parsed = self._parse_error_response(error_text)

        # 构建可读的错误消息
        parts = []
        if parsed["type"]:
            parts.append(parsed["type"])
        if parsed["reason"]:
            parts.append(f"[{parsed['reason']}]")
        if parsed["message"]:
            parts.append(parsed["message"])

        if parts:
            return ": ".join(parts) if len(parts) > 1 else parts[0]

        # 无法解析，返回原始文本
        return parsed["raw"]

    def classify(
        self,
        error: Exception,
        has_retry_left: bool = False,
    ) -> ErrorAction:
        """
        分类错误，返回处理动作

        Args:
            error: 异常对象
            has_retry_left: 当前候选是否还有重试次数

        Returns:
            ErrorAction: 处理动作
        """
        if isinstance(error, ConcurrencyLimitError):
            return ErrorAction.BREAK

        if isinstance(error, httpx.HTTPStatusError):
            status_code = int(getattr(error.response, "status_code", 0) or 0)
            # 401/403 是认证/权限错误，在同一个 key 上重试无意义，直接跳到下一个候选
            if status_code in (401, 403):
                return ErrorAction.BREAK
            return ErrorAction.CONTINUE if has_retry_left else ErrorAction.BREAK

        if isinstance(error, self.RETRIABLE_ERRORS):
            return ErrorAction.CONTINUE if has_retry_left else ErrorAction.BREAK

        if isinstance(error, self.NON_RETRIABLE_ERRORS):
            return ErrorAction.RAISE

        # 未知错误，直接抛出
        return ErrorAction.RAISE

    async def handle_rate_limit(
        self,
        key: ProviderAPIKey,
        provider_name: str,
        current_rpm: int | None,
        exception: ProviderRateLimitException,
        request_id: str | None = None,
    ) -> str:
        """
        处理 429 速率限制错误的自适应调整

        Args:
            key: API Key 对象
            provider_name: 提供商名称
            current_rpm: 当前分钟内的请求数
            exception: 速率限制异常
            request_id: 请求 ID（用于日志）

        Returns:
            限制类型: "concurrent" 或 "rpm" 或 "unknown"
        """
        try:
            # 提取响应头（如果有）
            response_headers = {}
            if hasattr(exception, "response_headers"):
                response_headers = exception.response_headers or {}

            # 检测速率限制类型
            rate_limit_info = detect_rate_limit_type(
                headers=response_headers,
                provider_name=provider_name,
                current_usage=current_rpm,
            )

            logger.info(
                f"  [{request_id}] 429错误分析: "
                f"类型={rate_limit_info.limit_type}, "
                f"retry_after={rate_limit_info.retry_after}s, "
                f"当前RPM={current_rpm}"
            )

            # 调用自适应管理器处理
            new_limit = self.adaptive_manager.handle_429_error(
                db=self.db,
                key=key,
                rate_limit_info=rate_limit_info,
                current_rpm=current_rpm,
            )

            if rate_limit_info.limit_type == RateLimitType.CONCURRENT:
                logger.warning(f"  [{request_id}] 并发限制触发（不调整RPM）")
                return "concurrent"
            elif rate_limit_info.limit_type == RateLimitType.RPM:
                if new_limit is not None:
                    logger.warning(
                        f"  [{request_id}] 自适应调整: Key {key.id[:8]}... RPM限制 -> {new_limit}"
                    )
                else:
                    logger.info(
                        f"  [{request_id}] 学习中: Key {key.id[:8]}... 观察已记录，暂不设限"
                    )
                return "rpm"
            else:
                return "unknown"

        except Exception as e:
            logger.exception(f"  [{request_id}] 处理429错误时异常: {e}")
            return "unknown"

    def convert_http_error(
        self,
        error: httpx.HTTPStatusError,
        provider_name: str,
        error_response_text: str | None = None,
    ) -> ProviderException | UpstreamClientException:
        """
        转换 HTTP 错误为 Provider 异常

        Args:
            error: HTTP 状态错误
            provider_name: Provider 名称
            error_response_text: 错误响应文本（可选）

        Returns:
            ProviderException 或 UpstreamClientException: 转换后的异常
        """
        status = error.response.status_code if error.response else None

        # 提取可读的错误消息
        extracted_message = self._extract_error_message(error_response_text)

        # 构建详细错误信息（仅用于日志，不暴露给客户端）
        if extracted_message:
            detailed_message = f"上游服务返回错误 {status}: {extracted_message}"
        else:
            detailed_message = f"上游服务返回错误: {status}"

        if status == 401:
            return ProviderAuthException(provider_name=provider_name)

        # 403: 检查是否为 Google VALIDATION_REQUIRED（账号需要手动验证）
        # 这类错误是永久性的，重试同一个 key 无意义，应视为认证错误
        if status == 403 and self._is_account_validation_required(error_response_text):
            logger.warning("检测到 Google 账号验证要求 (VALIDATION_REQUIRED): {}", provider_name)
            return ProviderAuthException(provider_name=provider_name)

        if status == 429:
            return ProviderRateLimitException(
                message="请求过于频繁，请稍后重试",
                provider_name=provider_name,
                response_headers=dict(error.response.headers) if error.response else None,
                retry_after=(
                    int(error.response.headers.get("retry-after", 0))
                    if error.response and error.response.headers.get("retry-after")
                    else None
                ),
            )

        # 400 错误：检查是否为 Thinking 块签名错误
        if status == 400 and self._is_thinking_error(error_response_text):
            logger.info(f"检测到 Thinking 块错误: {extracted_message}")
            return ThinkingSignatureException(
                message=extracted_message or "Thinking block signature validation failed",
                provider_name=provider_name,
                upstream_error=error_response_text,
            )

        # 400 错误：先检查是否为 Provider 兼容性错误（应触发故障转移）
        if status == 400 and self._is_compatibility_error(error_response_text):
            logger.info(f"检测到 Provider 兼容性错误，将触发故障转移: {extracted_message}")
            return ProviderCompatibilityException(
                message=extracted_message or "Provider 不支持此请求",
                provider_name=provider_name,
                status_code=400,
                upstream_error=error_response_text,
            )

        # 400 错误：检查是否为客户端请求错误（不应重试）
        if status == 400 and self.is_client_error(error_response_text):
            logger.info(f"检测到客户端请求错误，不进行重试: {extracted_message}")
            return UpstreamClientException(
                message=extracted_message or "请求无效",
                provider_name=provider_name,
                status_code=400,
                upstream_error=error_response_text,
            )

        if status and status >= 500:
            return ProviderNotAvailableException(
                message=detailed_message,
                provider_name=provider_name,
                upstream_status=status,
                upstream_response=error_response_text,
            )

        return ProviderNotAvailableException(
            message=detailed_message,
            provider_name=provider_name,
            upstream_status=status,
            upstream_response=error_response_text,
        )

    async def handle_http_error(
        self,
        http_error: httpx.HTTPStatusError,
        *,
        provider: Provider,
        endpoint: ProviderEndpoint,
        key: ProviderAPIKey,
        affinity_key: str,
        api_format: str,
        global_model_id: str,
        request_id: str | None,
        captured_key_concurrent: int | None,
        elapsed_ms: int | None,
        attempt: int,
        max_attempts: int,
    ) -> dict[str, Any]:
        """
        处理 HTTP 错误，返回 extra_data

        Args:
            http_error: HTTP 状态错误
            provider: Provider 对象
            endpoint: Endpoint 对象
            key: API Key 对象
            affinity_key: 亲和性标识符（通常为 API Key ID）
            api_format: API 格式
            global_model_id: GlobalModel ID（规范化的模型标识）
            request_id: 请求 ID
            captured_key_concurrent: 捕获的并发数
            elapsed_ms: 耗时（毫秒）
            attempt: 当前尝试次数
            max_attempts: 最大尝试次数

        Returns:
            Dict[str, Any]: 额外数据，包含：
                - error_response: 错误响应文本（如有）
                - converted_error: 转换后的异常对象（用于判断是否应该重试）
        """
        provider_name = str(provider.name)

        # 尝试读取错误响应内容
        # 优先使用 handler 附加的 upstream_response 属性（流式请求中 response.text 可能为空）
        error_response_text = getattr(http_error, "upstream_response", None)
        if not error_response_text:
            try:
                if http_error.response and hasattr(http_error.response, "text"):
                    error_response_text = http_error.response.text
            except Exception:
                pass

        logger.warning(
            f"  [{request_id}] HTTP错误 (attempt={attempt}/{max_attempts}): "
            f"{http_error.response.status_code if http_error.response else 'unknown'}"
        )

        converted_error = self.convert_http_error(http_error, provider_name, error_response_text)

        # 构建 extra_data，包含转换后的异常
        extra_data: dict[str, Any] = {
            "converted_error": converted_error,
        }
        if error_response_text:
            extra_data["error_response"] = error_response_text

        # client_format：用于缓存亲和性/缓存失效（用户视角）
        client_format_str = normalize_endpoint_signature(api_format)
        # provider_format：用于健康度/熔断 bucket（Provider 真实端点格式）
        fam = str(getattr(endpoint, "api_family", "")).strip().lower()
        kind = str(getattr(endpoint, "endpoint_kind", "")).strip().lower()
        provider_format_str = make_signature_key(fam, kind) if fam and kind else client_format_str

        # 处理客户端请求错误（不应重试，不失效缓存，不记录健康失败）
        if isinstance(converted_error, UpstreamClientException):
            logger.warning(
                f"  [{request_id}] 客户端请求错误，不进行重试: {converted_error.message}"
            )
            return extra_data

        # 处理认证错误
        if isinstance(converted_error, ProviderAuthException):
            if endpoint and key and self.cache_scheduler is not None:
                await self.cache_scheduler.invalidate_cache(
                    affinity_key=affinity_key,
                    api_format=client_format_str,
                    global_model_id=global_model_id,
                    endpoint_id=str(endpoint.id),
                    key_id=str(key.id),
                )
            if key:
                health_monitor.record_failure(
                    db=self.db,
                    key_id=str(key.id),
                    api_format=provider_format_str,
                    error_type="ProviderAuthException",
                )
            # 403 VALIDATION_REQUIRED → 标记 OAuth key 为账号级别封禁
            # 这与 test-model 端点的行为对齐（provider_query.py 第 669-690 行）
            status_code = http_error.response.status_code if http_error.response else None
            if (
                status_code == 403
                and key
                and str(getattr(key, "auth_type", "") or "").lower() == "oauth"
                and self._is_account_validation_required(error_response_text)
            ):
                try:
                    from datetime import datetime, timezone

                    from src.services.provider.oauth_token import (
                        OAUTH_ACCOUNT_BLOCK_PREFIX,
                    )

                    key.oauth_invalid_at = datetime.now(timezone.utc)
                    key.oauth_invalid_reason = f"{OAUTH_ACCOUNT_BLOCK_PREFIX}Google 要求验证账号"
                    key.is_active = False
                    self.db.commit()
                    logger.warning(
                        "  [{}] {} 因 403 VALIDATION_REQUIRED 已标记为账号异常并自动停用",
                        request_id,
                        self._format_key_display(key),
                    )
                except Exception as mark_exc:
                    logger.debug("  [{}] 标记 oauth_invalid 失败: {}", request_id, mark_exc)
            return extra_data

        # 处理限流错误
        if isinstance(converted_error, ProviderRateLimitException) and key:
            await self.handle_rate_limit(
                key=key,
                provider_name=provider_name,
                current_rpm=captured_key_concurrent,
                exception=converted_error,
                request_id=request_id,
            )
            if endpoint and self.cache_scheduler is not None:
                await self.cache_scheduler.invalidate_cache(
                    affinity_key=affinity_key,
                    api_format=client_format_str,
                    global_model_id=global_model_id,
                    endpoint_id=str(endpoint.id),
                    key_id=str(key.id),
                )
        else:
            # 其他错误也失效缓存
            if endpoint and key and self.cache_scheduler is not None:
                await self.cache_scheduler.invalidate_cache(
                    affinity_key=affinity_key,
                    api_format=client_format_str,
                    global_model_id=global_model_id,
                    endpoint_id=str(endpoint.id),
                    key_id=str(key.id),
                )

        # 记录健康失败
        if key:
            health_monitor.record_failure(
                db=self.db,
                key_id=str(key.id),
                api_format=provider_format_str,
                error_type=type(converted_error).__name__,
            )

        return extra_data

    async def handle_retriable_error(
        self,
        error: Exception,
        *,
        provider: Provider,
        endpoint: ProviderEndpoint,
        key: ProviderAPIKey,
        affinity_key: str,
        api_format: str,
        global_model_id: str,
        captured_key_concurrent: int | None,
        elapsed_ms: int | None,
        request_id: str | None,
        attempt: int,
        max_attempts: int,
    ) -> None:
        """
        处理可重试错误

        Args:
            error: 异常对象
            provider: Provider 对象
            endpoint: Endpoint 对象
            key: API Key 对象
            affinity_key: 亲和性标识符（通常为 API Key ID）
            api_format: API 格式
            global_model_id: GlobalModel ID（规范化的模型标识，用于缓存亲和性）
            captured_key_concurrent: 捕获的并发数
            elapsed_ms: 耗时（毫秒）
            request_id: 请求 ID
            attempt: 当前尝试次数
            max_attempts: 最大尝试次数
        """
        provider_name = str(provider.name)

        logger.warning(
            f"  [{request_id}] 请求失败 (attempt={attempt}/{max_attempts}): "
            f"{type(error).__name__}: {str(error)}"
        )

        # client_format：用于缓存亲和性/缓存失效（用户视角）
        client_format_str = normalize_endpoint_signature(api_format)
        # provider_format：用于健康度/熔断 bucket（Provider 真实端点格式）
        fam = str(getattr(endpoint, "api_family", "")).strip().lower()
        kind = str(getattr(endpoint, "endpoint_kind", "")).strip().lower()
        provider_format_str = make_signature_key(fam, kind) if fam and kind else client_format_str

        # 处理限流错误
        if isinstance(error, ProviderRateLimitException) and key:
            await self.handle_rate_limit(
                key=key,
                provider_name=provider_name,
                current_rpm=captured_key_concurrent,
                exception=error,
                request_id=request_id,
            )
            if endpoint and self.cache_scheduler is not None:
                await self.cache_scheduler.invalidate_cache(
                    affinity_key=affinity_key,
                    api_format=client_format_str,
                    global_model_id=global_model_id,
                    endpoint_id=str(endpoint.id),
                    key_id=str(key.id),
                )
        elif endpoint and key and self.cache_scheduler is not None:
            # 其他错误也失效缓存
            await self.cache_scheduler.invalidate_cache(
                affinity_key=affinity_key,
                api_format=client_format_str,
                global_model_id=global_model_id,
                endpoint_id=str(endpoint.id),
                key_id=str(key.id),
            )

        # 记录健康失败
        if key:
            health_monitor.record_failure(
                db=self.db,
                key_id=str(key.id),
                api_format=provider_format_str,
                error_type=type(error).__name__,
            )
