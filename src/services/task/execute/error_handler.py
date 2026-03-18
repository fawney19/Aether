from __future__ import annotations

from typing import Any

import httpx
from sqlalchemy.orm import Session

from src.config.settings import config
from src.core.error_utils import extract_error_message
from src.core.exceptions import (
    ConcurrencyLimitError,
    EmbeddedErrorException,
    ProxyNodeUnavailableError,
    ThinkingSignatureException,
    UpstreamClientException,
)
from src.core.logger import logger
from src.core.provider_types import ProviderType
from src.services.request.candidate import RequestCandidateService
from src.services.task.execute.pool import TaskPoolOperationsService
from src.services.task.request_state import RequestBodyState


class TaskErrorOperationsService:
    """任务执行错误处理服务（候选失败分类、整流与状态回写）。"""

    def __init__(self, db: Session, *, pool_ops: TaskPoolOperationsService) -> None:
        self.db = db
        self._pool_ops = pool_ops

    def mark_thinking_error_failed(
        self,
        candidate_record_id: str,
        error: Any,
        elapsed_ms: int,
        captured_key_concurrent: int | None,
        extra_data: dict[str, Any],
    ) -> None:
        """Mark ThinkingSignatureException as failed for the candidate."""
        if not isinstance(error, ThinkingSignatureException):
            return

        RequestCandidateService.mark_candidate_failed(
            db=self.db,
            candidate_id=candidate_record_id,
            error_type="ThinkingSignatureException",
            error_message=str(error),
            status_code=400,
            latency_ms=elapsed_ms,
            concurrent_requests=captured_key_concurrent,
            extra_data=extra_data,
        )

    def handle_thinking_signature_error(
        self,
        *,
        converted_error: Any,
        provider_type: str | None,
        request_id: str | None,
        candidate_record_id: str,
        elapsed_ms: int,
        captured_key_concurrent: int | None,
        serializable_extra_data: dict[str, Any],
        request_body_state: RequestBodyState | None,
    ) -> str:
        """Try to rectify thinking signature errors and request a retry."""
        from src.services.message.thinking_rectifier import ThinkingRectifier

        if not isinstance(converted_error, ThinkingSignatureException):
            raise converted_error

        if not config.thinking_rectifier_enabled:
            logger.info("  [{}] Thinking 错误：整流器已禁用，终止重试", request_id)
            self.mark_thinking_error_failed(
                candidate_record_id,
                converted_error,
                elapsed_ms,
                captured_key_concurrent,
                serializable_extra_data,
            )
            raise converted_error

        if request_body_state is None:
            logger.warning("  [{}] Thinking 错误：无法获取请求体引用，终止重试", request_id)
            self.mark_thinking_error_failed(
                candidate_record_id,
                converted_error,
                elapsed_ms,
                captured_key_concurrent,
                serializable_extra_data,
            )
            raise converted_error

        provider_type_norm = str(provider_type or "").lower()

        # Rectification may have multiple stages (Antigravity only).
        stage = request_body_state.rectify_stage()
        if stage <= 0 and request_body_state.is_rectified():
            stage = 1

        if stage >= 2 or (stage >= 1 and provider_type_norm != ProviderType.ANTIGRAVITY):
            logger.warning("  [{}] Thinking 错误：已整流仍失败，终止重试", request_id)
            self.mark_thinking_error_failed(
                candidate_record_id,
                converted_error,
                elapsed_ms,
                captured_key_concurrent,
                {**serializable_extra_data, "rectified": True, "rectify_stage": stage},
            )
            raise converted_error

        request_body = request_body_state.current_body

        stage_label = "thinking_only"
        next_stage = 1
        if stage == 0:
            rectified_body, modified = ThinkingRectifier.rectify(request_body)
            stage_label = "thinking_only"
            next_stage = 1
        else:
            # Stage 2 only applies to Antigravity.
            rectified_body, modified = ThinkingRectifier.rectify_signature_sensitive_blocks(
                request_body
            )
            stage_label = "thinking_and_tools"
            next_stage = 2

        if modified:
            request_body_state.mark_rectified(rectified_body, stage=next_stage)

            if provider_type_norm == ProviderType.ANTIGRAVITY:
                try:
                    from src.core.metrics import antigravity_degradation_total

                    antigravity_degradation_total.labels(
                        stage=stage_label,
                    ).inc()
                except Exception:
                    pass

            logger.info(
                "  [{}] 请求已整流(stage={})，在当前候选上重试",
                request_id,
                next_stage,
            )
            self.mark_thinking_error_failed(
                candidate_record_id,
                converted_error,
                elapsed_ms,
                captured_key_concurrent,
                {
                    **serializable_extra_data,
                    "rectified": True,
                    "rectify_stage": next_stage,
                    "rectify_stage_label": stage_label,
                },
            )
            return "continue"

        logger.warning("  [{}] Thinking 错误：无可整流内容", request_id)
        self.mark_thinking_error_failed(
            candidate_record_id,
            converted_error,
            elapsed_ms,
            captured_key_concurrent,
            serializable_extra_data,
        )
        raise converted_error

    async def handle_candidate_error(
        self,
        *,
        exec_err: Any,
        candidate: Any,
        candidate_record_id: str,
        retry_index: int,
        max_retries_for_candidate: int,
        affinity_key: str,
        api_format: str,
        global_model_id: str,
        request_id: str | None,
        attempt: int,
        max_attempts: int,
        error_classifier: Any,
        request_body_state: RequestBodyState | None = None,
    ) -> str:
        """
        Handle an execution error for a candidate.

        Returns:
        - "continue": retry current candidate
        - "break": move to next candidate
        - "raise": raise the underlying exception
        """
        from src.core.api_format.conversion.exceptions import FormatConversionError
        from src.services.proxy_node.resolver import (
            resolve_effective_proxy,
            resolve_proxy_info_async,
        )
        from src.services.request.executor import ExecutionError

        # 提前解析代理信息，写入候选记录的 extra_data（用于链路追踪展示）
        _eff_proxy = resolve_effective_proxy(
            getattr(candidate.provider, "proxy", None),
            getattr(candidate.key, "proxy", None),
        )
        _proxy_info = await resolve_proxy_info_async(_eff_proxy)
        _proxy_extra: dict[str, Any] | None = {"proxy": _proxy_info} if _proxy_info else None

        if not isinstance(exec_err, ExecutionError):
            RequestCandidateService.mark_candidate_failed(
                db=self.db,
                candidate_id=candidate_record_id,
                error_type=type(exec_err).__name__,
                error_message=str(exec_err),
                extra_data=_proxy_extra,
            )
            return "break"

        provider = candidate.provider
        endpoint = candidate.endpoint
        key = candidate.key

        context = exec_err.context
        captured_key_concurrent = context.concurrent_requests
        elapsed_ms = context.elapsed_ms
        cause = exec_err.cause

        has_retry_left = retry_index < (max_retries_for_candidate - 1)

        if isinstance(cause, ConcurrencyLimitError):
            rpm_current = context.rpm_current
            if rpm_current is None:
                rpm_current = captured_key_concurrent

            rpm_limit = context.rpm_limit
            rpm_available_for_new = context.rpm_available_for_new
            reservation_ratio = context.reservation_ratio
            reservation_phase = context.reservation_phase or "unknown"
            reservation_confidence = context.reservation_confidence
            reservation_load_factor = context.reservation_load_factor

            reason_code = "unknown"
            if rpm_limit is not None and rpm_current is not None:
                if context.is_cached_user:
                    if rpm_current >= rpm_limit:
                        reason_code = "total_limit"
                else:
                    if rpm_available_for_new is not None and rpm_current >= rpm_available_for_new:
                        reason_code = (
                            "reserved_for_cached" if rpm_current < rpm_limit else "total_limit"
                        )
                    elif rpm_current >= rpm_limit:
                        reason_code = "total_limit"

            reason_text = "并发限制"
            if reason_code == "reserved_for_cached":
                reason_text = "并发限制: 新用户配额已满（预留给缓存用户）"
            elif reason_code == "total_limit":
                reason_text = "并发限制: 总配额已满"

            parts: list[str] = []
            if rpm_current is not None:
                parts.append(f"current={rpm_current}")
            if rpm_limit is not None:
                parts.append(f"limit={rpm_limit}")
            if rpm_available_for_new is not None and not context.is_cached_user:
                parts.append(f"new={rpm_available_for_new}")
            if reservation_ratio is not None:
                parts.append(f"reserve={reservation_ratio:.0%}")
            if reservation_phase:
                parts.append(f"phase={reservation_phase}")

            skip_reason = reason_text
            if parts:
                skip_reason = f"{reason_text} ({', '.join(parts)})"

            logger.warning(
                "  [{}] 并发限制 (attempt={}/{}): provider={}, key={}, cached={}, reason={}, {}",
                request_id,
                attempt,
                max_attempts,
                provider.name,
                str(key.id)[:8],
                bool(context.is_cached_user),
                reason_code,
                ", ".join(parts) if parts else "N/A",
            )

            extra_data: dict[str, Any] = {
                "concurrency_denied": True,
                "concurrency_reason": reason_code,
                "rpm_current": rpm_current,
                "rpm_limit": rpm_limit,
                "rpm_available_for_new": rpm_available_for_new,
                "reservation_ratio": reservation_ratio,
                "reservation_phase": reservation_phase,
                "reservation_confidence": reservation_confidence,
                "reservation_load_factor": reservation_load_factor,
                "attempt": attempt,
                "max_attempts": max_attempts,
            }
            extra_data = {k: v for k, v in extra_data.items() if v is not None}
            if _proxy_extra:
                extra_data = {**_proxy_extra, **extra_data}

            try:
                from src.core.metrics import scheduler_concurrency_denied_total

                scheduler_concurrency_denied_total.labels(
                    is_cached_user=str(bool(context.is_cached_user)).lower(),
                    reason=reason_code,
                    reservation_phase=str(reservation_phase or "unknown"),
                ).inc()
            except Exception:
                pass

            RequestCandidateService.mark_candidate_skipped(
                db=self.db,
                candidate_id=candidate_record_id,
                skip_reason=skip_reason,
                status_code=429,
                concurrent_requests=rpm_current,
                extra_data=extra_data,
            )
            return "break"

        if isinstance(cause, ProxyNodeUnavailableError):
            # ProxyNode 不可用属于"配置明确指定但不可达/不可用"的情况，
            # 在当前候选上重试通常没有意义，直接切换到下一个候选更合理。
            node_id = cause.details.get("proxy_node_id") if cause.details else None
            logger.warning(
                "  [{}] 代理节点不可用 (node_id={})，切换候选: {}",
                request_id,
                node_id or "unknown",
                str(cause),
            )
            RequestCandidateService.mark_candidate_failed(
                db=self.db,
                candidate_id=candidate_record_id,
                error_type=type(cause).__name__,
                error_message=extract_error_message(cause),
                latency_ms=elapsed_ms,
                concurrent_requests=captured_key_concurrent,
                extra_data=_proxy_extra,
            )
            return "break"

        if isinstance(cause, EmbeddedErrorException):
            error_message = cause.error_message or ""
            embedded_status = cause.error_code or 200
            if error_classifier.is_client_error(error_message):
                logger.warning(
                    "  [{}] 嵌入式客户端错误，继续转移: {}",
                    request_id,
                    error_message[:200],
                )
                RequestCandidateService.mark_candidate_failed(
                    db=self.db,
                    candidate_id=candidate_record_id,
                    error_type="UpstreamClientException",
                    error_message=error_message,
                    status_code=embedded_status,
                    latency_ms=elapsed_ms,
                    concurrent_requests=captured_key_concurrent,
                    extra_data=_proxy_extra,
                )
                return "break"

            logger.warning(
                "  [{}] 嵌入式服务端错误，尝试重试: {}",
                request_id,
                error_message[:200],
            )
            RequestCandidateService.mark_candidate_failed(
                db=self.db,
                candidate_id=candidate_record_id,
                error_type="EmbeddedErrorException",
                error_message=error_message,
                status_code=embedded_status,
                latency_ms=elapsed_ms,
                concurrent_requests=captured_key_concurrent,
                extra_data=_proxy_extra,
            )
            return "continue" if has_retry_left else "break"

        if isinstance(cause, httpx.HTTPStatusError):
            status_code = cause.response.status_code
            extra_data = await error_classifier.handle_http_error(
                http_error=cause,
                provider=provider,
                endpoint=endpoint,
                key=key,
                affinity_key=affinity_key,
                api_format=api_format,
                global_model_id=global_model_id,
                request_id=request_id,
                captured_key_concurrent=captured_key_concurrent,
                elapsed_ms=elapsed_ms,
                max_attempts=max_attempts,
                attempt=attempt,
            )

            # Account Pool: apply health policy (cooldown/disable).
            await self._pool_ops.pool_on_error(provider, key, status_code, cause)

            converted_error = extra_data.get("converted_error")
            serializable_extra_data = {
                k: v for k, v in extra_data.items() if k != "converted_error"
            }
            if _proxy_info:
                serializable_extra_data["proxy"] = _proxy_info

            if isinstance(converted_error, ThinkingSignatureException):
                action = self.handle_thinking_signature_error(
                    converted_error=converted_error,
                    provider_type=str(getattr(provider, "provider_type", "") or "").lower(),
                    request_id=request_id,
                    candidate_record_id=candidate_record_id,
                    elapsed_ms=elapsed_ms,
                    captured_key_concurrent=captured_key_concurrent,
                    serializable_extra_data=serializable_extra_data,
                    request_body_state=request_body_state,
                )
                if action == "continue":
                    return "continue"

            if isinstance(converted_error, UpstreamClientException):
                logger.warning(
                    "  [{}] 客户端请求错误，继续转移: {}",
                    request_id,
                    str(converted_error.message),
                )
                RequestCandidateService.mark_candidate_failed(
                    db=self.db,
                    candidate_id=candidate_record_id,
                    error_type="UpstreamClientException",
                    error_message=converted_error.message,
                    status_code=status_code,
                    latency_ms=elapsed_ms,
                    concurrent_requests=captured_key_concurrent,
                    extra_data=serializable_extra_data,
                )
                return "break"

            RequestCandidateService.mark_candidate_failed(
                db=self.db,
                candidate_id=candidate_record_id,
                error_type="HTTPStatusError",
                error_message=extract_error_message(cause, status_code),
                status_code=status_code,
                latency_ms=elapsed_ms,
                concurrent_requests=captured_key_concurrent,
                extra_data=serializable_extra_data,
            )
            return "continue" if has_retry_left else "break"

        if isinstance(cause, error_classifier.RETRIABLE_ERRORS):
            await error_classifier.handle_retriable_error(
                error=cause,
                provider=provider,
                endpoint=endpoint,
                key=key,
                affinity_key=affinity_key,
                api_format=api_format,
                global_model_id=global_model_id,
                captured_key_concurrent=captured_key_concurrent,
                elapsed_ms=elapsed_ms,
                request_id=request_id,
                attempt=attempt,
                max_attempts=max_attempts,
            )
            RequestCandidateService.mark_candidate_failed(
                db=self.db,
                candidate_id=candidate_record_id,
                error_type=type(cause).__name__,
                error_message=extract_error_message(cause),
                latency_ms=elapsed_ms,
                concurrent_requests=captured_key_concurrent,
                extra_data=_proxy_extra,
            )
            return "continue" if has_retry_left else "break"

        if isinstance(cause, FormatConversionError):
            logger.warning("  [{}] 格式转换失败，切换候选: {}", request_id, str(cause))
            RequestCandidateService.mark_candidate_failed(
                db=self.db,
                candidate_id=candidate_record_id,
                error_type="FormatConversionError",
                error_message=str(cause),
                latency_ms=elapsed_ms,
                concurrent_requests=captured_key_concurrent,
                extra_data=_proxy_extra,
            )
            return "break"

        RequestCandidateService.mark_candidate_failed(
            db=self.db,
            candidate_id=candidate_record_id,
            error_type=type(cause).__name__,
            error_message=extract_error_message(cause),
            latency_ms=elapsed_ms,
            concurrent_requests=captured_key_concurrent,
            extra_data=_proxy_extra,
        )
        return "break"
