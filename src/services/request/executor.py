"""
封装请求执行逻辑，包含并发控制与链路追踪。
"""

from __future__ import annotations

import asyncio
import math
import time
from collections.abc import Callable
from contextlib import AsyncExitStack
from dataclasses import dataclass
from typing import Any

from sqlalchemy.orm import Session

from src.core.api_format.signature import make_signature_key
from src.core.exceptions import ConcurrencyLimitError
from src.core.logger import logger
from src.services.health.monitor import health_monitor
from src.services.provider.format import normalize_endpoint_signature
from src.services.rate_limit.adaptive_reservation import get_adaptive_reservation_manager
from src.services.rate_limit.adaptive_rpm import get_adaptive_rpm_manager
from src.services.request.candidate import RequestCandidateService


@dataclass
class ExecutionContext:
    candidate_id: str
    candidate_index: int
    provider_id: str
    endpoint_id: str
    key_id: str
    user_id: str | None
    api_key_id: str | None
    is_cached_user: bool
    start_time: float | None = None
    elapsed_ms: int | None = None
    concurrent_requests: int | None = None
    rpm_current: int | None = None
    rpm_limit: int | None = None
    rpm_available_for_new: int | None = None
    reservation_ratio: float | None = None
    reservation_phase: str | None = None
    reservation_confidence: float | None = None
    reservation_load_factor: float | None = None
    # 用户级别 RPM 跟踪
    user_rpm_current: int | None = None
    user_rpm_limit: int | None = None
    user_rpm_available_for_new: int | None = None


@dataclass
class ExecutionResult:
    response: Any
    context: ExecutionContext


class ExecutionError(Exception):
    def __init__(self, cause: Exception, context: ExecutionContext):
        super().__init__(str(cause))
        self.cause = cause
        self.context = context


class RequestExecutor:
    def __init__(self, db: Session, concurrency_manager: Any, adaptive_manager: Any) -> None:
        self.db = db
        self.concurrency_manager = concurrency_manager
        self.adaptive_manager = adaptive_manager

    async def execute(
        self,
        *,
        candidate: Any,
        candidate_id: str,
        candidate_index: int,
        user_api_key: Any | None,
        user_id: str | None = None,
        request_func: Callable[..., Any],
        request_id: str | None,
        api_format: str,
        model_name: str,
        is_stream: bool = False,
    ) -> ExecutionResult:
        provider = candidate.provider
        endpoint = candidate.endpoint
        key = candidate.key
        is_cached_user = bool(candidate.is_cached)

        # 标记候选开始执行
        RequestCandidateService.mark_candidate_started(
            db=self.db,
            candidate_id=candidate_id,
        )

        context = ExecutionContext(
            candidate_id=candidate_id,
            candidate_index=candidate_index,
            provider_id=provider.id,
            endpoint_id=endpoint.id,
            key_id=key.id,
            user_id=user_id if user_id is not None else getattr(user_api_key, "user_id", None),
            api_key_id=getattr(user_api_key, "id", None),
            is_cached_user=is_cached_user,
        )

        try:
            # 获取 User 对象（用于用户级 RPM 限制检查）
            user = None
            effective_user_id = context.user_id
            if effective_user_id:
                try:
                    from src.models.database import User

                    user = (
                        self.db.query(User).filter(User.id == effective_user_id).first()
                    )
                except Exception as e:
                    logger.debug("获取 User 对象失败（用于 RPM 检查）: {}", e)

            # 计算动态预留比例
            reservation_manager = get_adaptive_reservation_manager()
            # 获取当前 RPM 计数用于计算负载
            # 注意：key 侧返回的是 RPM 计数（不会在请求结束时减少，靠 TTL 过期）
            try:
                current_key_rpm = await self.concurrency_manager.get_key_rpm_count(
                    key_id=key.id,
                )
            except Exception as e:
                logger.debug("获取 RPM 计数失败（用于预留计算）: {}", e)
                current_key_rpm = 0

            # 在获取 guard 之前记录当前 RPM 计数，便于并发拒绝场景落库
            context.concurrent_requests = current_key_rpm
            context.rpm_current = current_key_rpm

            # 获取有效的 RPM 限制（自适应或固定）
            effective_key_limit = get_adaptive_rpm_manager().get_effective_limit(key)

            reservation_result = reservation_manager.calculate_reservation(
                key=key,
                current_usage=current_key_rpm,
                effective_limit=effective_key_limit,
            )
            dynamic_reservation_ratio = reservation_result.ratio

            context.rpm_limit = effective_key_limit
            context.reservation_ratio = dynamic_reservation_ratio
            context.reservation_phase = reservation_result.phase
            context.reservation_confidence = reservation_result.confidence
            context.reservation_load_factor = reservation_result.load_factor

            if effective_key_limit is not None and not is_cached_user:
                context.rpm_available_for_new = max(
                    1, math.floor(effective_key_limit * (1 - dynamic_reservation_ratio))
                )

            logger.debug(
                "[Executor] 动态预留: key={}..., ratio={:.0%}, phase={}, confidence={:.0%}",
                key.id[:8],
                dynamic_reservation_ratio,
                reservation_result.phase,
                reservation_result.confidence,
            )

            async with AsyncExitStack() as stack:
                # ---- 用户级别 RPM guard（如果用户配置了 rpm_limit）----
                if user is not None and getattr(user, "rpm_limit", None) is not None:
                    await stack.enter_async_context(
                        self.concurrency_manager.user_rpm_guard(
                            user_id=str(user.id),
                            user_rpm_limit=int(user.rpm_limit),
                            is_cached_user=is_cached_user,
                            cache_reservation_ratio=dynamic_reservation_ratio,
                        )
                    )
                    # 记录用户 RPM 统计
                    try:
                        user_rpm_count = await self.concurrency_manager.get_user_rpm_count(
                            user_id=str(user.id),
                        )
                        context.user_rpm_current = user_rpm_count
                        context.user_rpm_limit = int(user.rpm_limit)
                        if not is_cached_user:
                            context.user_rpm_available_for_new = max(
                                1,
                                math.floor(
                                    int(user.rpm_limit) * (1 - dynamic_reservation_ratio)
                                ),
                            )
                    except Exception as e:
                        logger.debug("获取 User RPM 计数失败（guard 内）: {}", e)

                # ---- Key RPM guard（始终执行，已有逻辑）----
                await stack.enter_async_context(
                    self.concurrency_manager.rpm_guard(
                        key_id=key.id,
                        key_rpm_limit=effective_key_limit,
                        is_cached_user=is_cached_user,
                        cache_reservation_ratio=dynamic_reservation_ratio,
                    )
                )

                # 获取当前 RPM 计数（guard 内再次获取以获得最新值）
                try:
                    key_rpm_count = await self.concurrency_manager.get_key_rpm_count(
                        key_id=key.id,
                    )
                except Exception as e:
                    logger.debug("获取 RPM 计数失败（guard 内）: {}", e)
                    key_rpm_count = None

                if key_rpm_count is not None:
                    context.concurrent_requests = key_rpm_count  # 用于记录，实际是 RPM 计数
                context.start_time = time.time()

                response = await request_func(provider, endpoint, key, candidate)

                context.elapsed_ms = int((time.time() - context.start_time) * 1000)

                fam = str(getattr(endpoint, "api_family", "")).strip().lower()
                kind = str(getattr(endpoint, "endpoint_kind", "")).strip().lower()
                provider_format_str = make_signature_key(fam, kind) if fam and kind else ""
                client_format_str = normalize_endpoint_signature(api_format)
                health_format = provider_format_str or client_format_str

                await asyncio.to_thread(
                    health_monitor.record_success,
                    db=self.db,
                    key_id=key.id,
                    api_format=health_format,
                    response_time_ms=context.elapsed_ms,
                )

                # 自适应模式：rpm_limit = NULL
                if key.rpm_limit is None and key_rpm_count is not None:
                    self.adaptive_manager.handle_success(
                        db=self.db,
                        key=key,
                        current_rpm=key_rpm_count,
                    )

                # 根据是否为流式请求，标记不同状态
                if is_stream:
                    # 流式请求：标记为 streaming 状态
                    # 此时连接已建立但流传输尚未完成
                    # success 状态会在流完成后由 _record_stream_stats 方法标记
                    RequestCandidateService.mark_candidate_streaming(
                        db=self.db,
                        candidate_id=candidate_id,
                        concurrent_requests=key_rpm_count,
                    )
                else:
                    # 非流式请求：标记为 success 状态
                    from src.services.proxy_node.resolver import (
                        resolve_effective_proxy,
                        resolve_proxy_info_async,
                    )

                    _eff_proxy = resolve_effective_proxy(
                        getattr(provider, "proxy", None), getattr(key, "proxy", None)
                    )
                    _extra: dict[str, Any] = {
                        "is_cached_user": is_cached_user,
                        "model_name": model_name,
                        "api_format": api_format,
                    }
                    _pi = await resolve_proxy_info_async(_eff_proxy)
                    if _pi:
                        _extra["proxy"] = _pi
                    RequestCandidateService.mark_candidate_success(
                        db=self.db,
                        candidate_id=candidate_id,
                        status_code=200,
                        latency_ms=context.elapsed_ms,
                        concurrent_requests=key_rpm_count,
                        extra_data=_extra,
                    )

                return ExecutionResult(response=response, context=context)
        except ConcurrencyLimitError as exc:
            raise ExecutionError(exc, context) from exc
        except Exception as exc:
            context.elapsed_ms = (
                int((time.time() - context.start_time) * 1000)
                if context.start_time is not None
                else None
            )
            raise ExecutionError(exc, context) from exc
