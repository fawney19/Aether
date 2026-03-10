"""
系统韧性和风险管控模块
提供全局的错误处理、自动恢复、降级策略和用户友好的错误体验
"""

from __future__ import annotations

import asyncio
import functools
import threading
import time
import traceback
import uuid
from collections import deque
from collections.abc import Callable
from contextlib import asynccontextmanager
from datetime import datetime, timezone
from enum import Enum
from typing import Any

from src.core.logger import logger

from ..core.exceptions import ProxyException


class ErrorSeverity(Enum):
    """错误严重程度"""

    LOW = "low"  # 低级错误，不影响核心功能
    MEDIUM = "medium"  # 中级错误，影响部分功能
    HIGH = "high"  # 高级错误，影响主要功能
    CRITICAL = "critical"  # 严重错误，影响系统可用性


class RecoveryStrategy(Enum):
    """恢复策略"""

    RETRY = "retry"  # 重试
    FALLBACK = "fallback"  # 降级
    CIRCUIT_BREAKER = "circuit_breaker"  # 熔断
    GRACEFUL_DEGRADE = "graceful_degrade"  # 优雅降级
    USER_NOTIFY = "user_notify"  # 通知用户


class ErrorPattern:
    """错误模式定义"""

    def __init__(
        self,
        error_types: list[type[Exception]],
        severity: ErrorSeverity,
        recovery_strategy: RecoveryStrategy,
        user_message: str,
        auto_recover: bool = True,
        max_retries: int = 3,
        retry_delay: float = 1.0,
        circuit_threshold: int = 5,
    ):
        self.error_types = error_types
        self.severity = severity
        self.recovery_strategy = recovery_strategy
        self.user_message = user_message
        self.auto_recover = auto_recover
        self.max_retries = max_retries
        self.retry_delay = retry_delay
        self.circuit_threshold = circuit_threshold


class CircuitBreaker:
    """熔断器"""

    def __init__(self, failure_threshold: int = 5, timeout: int = 60):
        self.failure_threshold = failure_threshold
        self.timeout = timeout
        self.failure_count = 0
        self.last_failure_time = None
        self.state = "closed"  # closed, open, half-open
        self._lock = threading.Lock()

    def call(self, func: Callable[..., Any], *args: Any, **kwargs: Any) -> Any:
        """执行函数调用，应用熔断逻辑"""
        with self._lock:
            if self.state == "open":
                if self._should_attempt_reset():
                    self.state = "half-open"
                else:
                    raise Exception("服务暂时不可用，请稍后重试")

            try:
                result = func(*args, **kwargs)
                self._on_success()
                return result
            except Exception as e:
                self._on_failure()
                raise

    def _should_attempt_reset(self) -> bool:
        """检查是否应该尝试重置熔断器"""
        if self.last_failure_time is None:
            return True
        return time.time() - self.last_failure_time >= self.timeout

    def _on_success(self) -> None:
        """成功时重置计数器"""
        self.failure_count = 0
        self.state = "closed"

    def _on_failure(self) -> None:
        """失败时增加计数器"""
        self.failure_count += 1
        self.last_failure_time = time.time()
        if self.failure_count >= self.failure_threshold:
            self.state = "open"


class ResilienceManager:
    """系统韧性管理器"""

    _MAX_ERROR_STATS = 500
    _MAX_CIRCUIT_BREAKERS = 200
    _MAX_LAST_ERRORS = 100

    def __init__(self) -> None:
        self.error_patterns: list[ErrorPattern] = []
        self.circuit_breakers: dict[str, CircuitBreaker] = {}
        self.error_stats: dict[str, int] = {}
        self.last_errors: deque[dict[str, Any]] = deque(maxlen=self._MAX_LAST_ERRORS)
        self._setup_default_patterns()

    def _setup_default_patterns(self) -> None:
        """设置默认错误处理模式"""

        # 数据库连接错误 - 只捕获特定的数据库相关异常
        try:
            from sqlalchemy.exc import (
                DisconnectionError,
                OperationalError,
                ProgrammingError,
            )
            from sqlalchemy.exc import TimeoutError as SQLTimeoutError

            # SQL/Schema 编程错误（如缺列/缺表）不应误判为“连接异常重试”。
            self.add_error_pattern(
                ErrorPattern(
                    error_types=[ProgrammingError],
                    severity=ErrorSeverity.HIGH,
                    recovery_strategy=RecoveryStrategy.USER_NOTIFY,
                    user_message="数据库结构与当前版本不兼容，请执行 alembic upgrade head 后重试",
                    auto_recover=False,
                )
            )

            db_exceptions = [
                OperationalError,
                DisconnectionError,
                SQLTimeoutError,
            ]
        except ImportError:
            # 如果SQLAlchemy不可用，使用通用异常类型
            db_exceptions = [ConnectionError, OSError]

        self.add_error_pattern(
            ErrorPattern(
                error_types=db_exceptions,
                severity=ErrorSeverity.HIGH,
                recovery_strategy=RecoveryStrategy.RETRY,
                user_message="数据库连接异常，正在重试...",
                max_retries=3,
                retry_delay=1.0,
            )
        )

        # 认证相关错误 - 只捕获特定的认证异常
        try:
            from ..core.exceptions import ForbiddenException, ProviderAuthException

            auth_exceptions = [ProviderAuthException, ForbiddenException]
        except ImportError:
            # 如果无法导入特定异常，使用更保守的方式（不使用通用异常）
            auth_exceptions = []

        if auth_exceptions:
            self.add_error_pattern(
                ErrorPattern(
                    error_types=auth_exceptions,
                    severity=ErrorSeverity.MEDIUM,
                    recovery_strategy=RecoveryStrategy.USER_NOTIFY,
                    user_message="认证失败，请检查API密钥或重新登录",
                    auto_recover=False,
                )
            )

        # 网络请求错误
        self.add_error_pattern(
            ErrorPattern(
                error_types=[ConnectionError, TimeoutError],
                severity=ErrorSeverity.MEDIUM,
                recovery_strategy=RecoveryStrategy.FALLBACK,
                user_message="网络连接异常，正在尝试备用方案...",
                max_retries=2,
            )
        )

    def add_error_pattern(self, pattern: ErrorPattern) -> None:
        """添加错误处理模式"""
        self.error_patterns.append(pattern)

    def get_circuit_breaker(self, key: str) -> CircuitBreaker:
        """获取或创建熔断器"""
        if key not in self.circuit_breakers:
            # 淘汰旧熔断器，防止无界增长
            if len(self.circuit_breakers) >= self._MAX_CIRCUIT_BREAKERS:
                # 优先淘汰已恢复(closed)的
                closed_keys = [k for k, cb in self.circuit_breakers.items() if cb.state == "closed"]
                if closed_keys:
                    for k in closed_keys:
                        del self.circuit_breakers[k]
                else:
                    # 全部处于 open/half-open，按最后失败时间淘汰最旧的一半
                    sorted_keys = sorted(
                        self.circuit_breakers,
                        key=lambda cb_key: self.circuit_breakers[cb_key].last_failure_time or 0,
                    )
                    for k in sorted_keys[: len(sorted_keys) // 2 or 1]:
                        del self.circuit_breakers[k]
            self.circuit_breakers[key] = CircuitBreaker()
        return self.circuit_breakers[key]

    def handle_error(
        self, error: Exception, context: dict[str, Any] = None, operation: str = "unknown"
    ) -> dict[str, Any]:
        """处理错误并返回处理结果"""

        error_id = str(uuid.uuid4())[:8]
        context = context or {}

        # 记录错误
        error_info = {
            "error_id": error_id,
            "error_type": type(error).__name__,
            "error_message": str(error),
            "operation": operation,
            "context": context,
            "timestamp": datetime.now(timezone.utc),
            "traceback": traceback.format_exc(),
        }

        self.last_errors.append(error_info)

        # 更新错误统计（超上限时淘汰计数最低的条目）
        error_key = f"{type(error).__name__}:{operation}"
        self.error_stats[error_key] = self.error_stats.get(error_key, 0) + 1
        if len(self.error_stats) > self._MAX_ERROR_STATS:
            min_key = min(
                (k for k in self.error_stats if k != error_key),
                key=lambda k: self.error_stats[k],
                default=None,
            )
            if min_key is not None:
                del self.error_stats[min_key]

        # 查找匹配的错误处理模式
        pattern = self._find_matching_pattern(error)

        if pattern:
            logger.error(f"错误处理 [{error_id}]: {pattern.user_message}")

            return {
                "error_id": error_id,
                "severity": pattern.severity,
                "recovery_strategy": pattern.recovery_strategy,
                "user_message": pattern.user_message,
                "auto_recover": pattern.auto_recover,
                "pattern": pattern,
            }
        else:
            # 未匹配的错误，使用默认处理
            logger.error(f"未知错误 [{error_id}]: {str(error)}")

            return {
                "error_id": error_id,
                "severity": ErrorSeverity.MEDIUM,
                "recovery_strategy": RecoveryStrategy.USER_NOTIFY,
                "user_message": "系统遇到未知错误，请稍后重试或联系管理员",
                "auto_recover": False,
                "pattern": None,
            }

    def _find_matching_pattern(self, error: Exception) -> ErrorPattern | None:
        """查找匹配的错误处理模式"""
        for pattern in self.error_patterns:
            if any(isinstance(error, error_type) for error_type in pattern.error_types):
                return pattern
        return None

    def get_error_stats(self) -> dict[str, Any]:
        """获取错误统计"""
        return {
            "total_errors": sum(self.error_stats.values()),
            "error_breakdown": self.error_stats.copy(),
            "recent_errors": len(self.last_errors),
            "circuit_breakers": {
                key: {"state": cb.state, "failure_count": cb.failure_count}
                for key, cb in self.circuit_breakers.items()
            },
        }


# 全局韧性管理器实例
resilience_manager = ResilienceManager()


def resilient_operation(
    operation_name: str | None = None,
    max_retries: int | None = None,
    retry_delay: float | None = None,
    circuit_breaker_key: str | None = None,
    context: dict[str, Any] = None,
) -> Any:
    """
    韧性操作装饰器
    自动处理重试、熔断、错误记录等
    """

    def decorator(func: Callable) -> Callable:
        @functools.wraps(func)
        async def async_wrapper(*args: Any, **kwargs: Any) -> Any:
            op_name = operation_name or f"{func.__module__}.{func.__name__}"
            retries = max_retries or 3
            delay = retry_delay or 1.0

            last_error = None

            for attempt in range(retries + 1):
                try:
                    # 如果指定了熔断器，使用熔断逻辑
                    if circuit_breaker_key:
                        cb = resilience_manager.get_circuit_breaker(circuit_breaker_key)
                        if asyncio.iscoroutinefunction(func):
                            return await cb.call(func, *args, **kwargs)
                        else:
                            return cb.call(func, *args, **kwargs)
                    else:
                        if asyncio.iscoroutinefunction(func):
                            return await func(*args, **kwargs)
                        else:
                            return func(*args, **kwargs)

                except Exception as e:
                    last_error = e

                    # 处理错误
                    error_result = resilience_manager.handle_error(
                        error=e,
                        context={**(context or {}), "attempt": attempt + 1, "max_retries": retries},
                        operation=op_name,
                    )

                    # 如果是最后一次尝试，或者不应该自动恢复，直接抛出
                    if attempt == retries or not error_result.get("auto_recover", True):
                        raise ProxyException(
                            status_code=500,
                            error_type="system_error",
                            message=error_result["user_message"],
                            details={
                                "error_id": error_result["error_id"],
                                "original_error": str(e),
                            },
                        )

                    # 等待后重试
                    if attempt < retries:
                        await asyncio.sleep(delay * (attempt + 1))  # 指数退避

            # 这里不应该到达，但作为安全网
            raise last_error

        @functools.wraps(func)
        def sync_wrapper(*args: Any, **kwargs: Any) -> None:
            # 对于同步函数，创建异步包装器并运行
            return asyncio.run(async_wrapper(*args, **kwargs))

        # 根据函数类型返回对应的包装器
        if asyncio.iscoroutinefunction(func):
            return async_wrapper
        else:
            return sync_wrapper

    return decorator


@asynccontextmanager
async def safe_operation(operation_name: str, context: dict[str, Any] = None) -> Any:
    """
    安全操作上下文管理器
    自动处理异常并提供用户友好的错误信息
    """
    try:
        yield
    except Exception as e:
        error_result = resilience_manager.handle_error(
            error=e, context=context or {}, operation=operation_name
        )

        # 根据错误严重程度决定是否抛出异常
        if error_result["severity"] in [ErrorSeverity.HIGH, ErrorSeverity.CRITICAL]:
            raise ProxyException(
                status_code=500,
                error_type="system_error",
                message=error_result["user_message"],
                details={"error_id": error_result["error_id"]},
            )
        else:
            # 记录警告但不中断操作
            logger.warning(f"操作警告 [{error_result['error_id']}]: {error_result['user_message']}")


def graceful_degradation(
    fallback_func: Callable | None = None, fallback_value: Any | None = None
) -> Any:
    """
    优雅降级装饰器
    当主要功能失败时，自动切换到备用方案
    """

    def decorator(func: Callable) -> Callable:
        @functools.wraps(func)
        async def async_wrapper(*args: Any, **kwargs: Any) -> Any:
            try:
                if asyncio.iscoroutinefunction(func):
                    return await func(*args, **kwargs)
                else:
                    return func(*args, **kwargs)
            except Exception as e:
                logger.warning(f"主要功能失败，启用降级模式: {func.__name__}")

                if fallback_func:
                    try:
                        if asyncio.iscoroutinefunction(fallback_func):
                            return await fallback_func(*args, **kwargs)
                        else:
                            return fallback_func(*args, **kwargs)
                    except Exception as fallback_error:
                        logger.exception(f"降级方案也失败了: {fallback_func.__name__}")
                        raise e  # 抛出原始错误
                else:
                    return fallback_value

        if asyncio.iscoroutinefunction(func):
            return async_wrapper
        else:
            return lambda *args, **kwargs: asyncio.run(async_wrapper(*args, **kwargs))

    return decorator


# 导出主要接口
__all__ = [
    "resilience_manager",
    "resilient_operation",
    "safe_operation",
    "graceful_degradation",
    "ErrorSeverity",
    "RecoveryStrategy",
    "ErrorPattern",
]
