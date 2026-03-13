"""
限流服务模块

包含自适应 RPM 控制、并发管理、IP限流等功能。
"""

from src.services.rate_limit.adaptive_rpm import AdaptiveConcurrencyManager  # 向后兼容别名
from src.services.rate_limit.adaptive_rpm import (
    AdaptiveRPMManager,
    get_adaptive_rpm_manager,
)
from src.services.rate_limit.concurrency_manager import ConcurrencyManager
from src.services.rate_limit.detector import RateLimitDetector
from src.services.rate_limit.ip_limiter import IPRateLimiter
from src.services.rate_limit.user_rpm_limiter import UserRpmLimiter, get_user_rpm_limiter

__all__ = [
    "AdaptiveConcurrencyManager",  # 向后兼容
    "AdaptiveRPMManager",
    "ConcurrencyManager",
    "IPRateLimiter",
    "RateLimitDetector",
    "UserRpmLimiter",
    "get_adaptive_rpm_manager",
    "get_user_rpm_limiter",
]
