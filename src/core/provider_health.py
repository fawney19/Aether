"""
提供商健康度管理
基于简单的失败计数和优先级调整
"""

import time
from collections import defaultdict
from typing import Any


class ProviderHealthTracker:
    """
    追踪提供商的健康状态
    根据失败率动态调整优先级
    """

    def __init__(
        self,
        failure_window: int = 300,  # 5分钟时间窗口
        failure_threshold: int = 3,  # 3次失败降低优先级
        recovery_time: int = 600,  # 10分钟后重置
    ):
        self.failure_window = failure_window
        self.failure_threshold = failure_threshold
        self.recovery_time = recovery_time

        # 存储每个提供商的失败记录
        self.failures: dict[str, list] = defaultdict(list)
        # 存储每个提供商的成功记录
        self.successes: dict[str, list] = defaultdict(list)
        # 存储优先级调整
        self.priority_adjustments: dict[str, int] = {}

    def record_success(self, provider_name: str) -> None:
        """记录成功的请求"""
        current_time = time.time()

        # 记录成功时间
        self.successes[provider_name].append(current_time)

        # 清理旧记录
        self._cleanup_old_records(provider_name, current_time)

        # 如果连续成功，可以恢复优先级
        if len(self.successes[provider_name]) >= 5:
            if self.priority_adjustments.get(provider_name, 0) < 0:
                self.priority_adjustments[provider_name] += 1

    def record_failure(self, provider_name: str) -> None:
        """记录失败的请求"""
        current_time = time.time()

        # 记录失败时间
        self.failures[provider_name].append(current_time)

        # 清理旧记录
        self._cleanup_old_records(provider_name, current_time)

        # 检查是否需要降低优先级
        recent_failures = len(self.failures[provider_name])
        if recent_failures >= self.failure_threshold:
            # 降低优先级
            current_adjustment = self.priority_adjustments.get(provider_name, 0)
            self.priority_adjustments[provider_name] = current_adjustment - 1

    def get_priority_adjustment(self, provider_name: str) -> int:
        """
        获取优先级调整值
        负数表示降低优先级，正数表示提高优先级
        """
        return self.priority_adjustments.get(provider_name, 0)

    def get_health_status(self, provider_name: str) -> dict:
        """
        获取提供商的健康状态
        """
        current_time = time.time()
        self._cleanup_old_records(provider_name, current_time)

        recent_failures = len(self.failures[provider_name])
        recent_successes = len(self.successes[provider_name])
        total_requests = recent_failures + recent_successes

        failure_rate = recent_failures / total_requests if total_requests > 0 else 0

        return {
            "provider": provider_name,
            "recent_failures": recent_failures,
            "recent_successes": recent_successes,
            "failure_rate": failure_rate,
            "priority_adjustment": self.get_priority_adjustment(provider_name),
            "status": self._get_status_label(failure_rate, recent_failures),
        }

    def _cleanup_old_records(self, provider_name: str, current_time: float) -> None:
        """清理超出时间窗口的记录"""
        # 清理失败记录
        self.failures[provider_name] = [
            t for t in self.failures[provider_name] if current_time - t < self.failure_window
        ]

        # 清理成功记录
        self.successes[provider_name] = [
            t for t in self.successes[provider_name] if current_time - t < self.failure_window
        ]

        # 如果很久没有失败，重置优先级调整
        if not self.failures[provider_name] and self.priority_adjustments.get(provider_name, 0) < 0:
            # 检查恢复时间
            if all(current_time - t > self.recovery_time for t in self.successes[provider_name]):
                self.priority_adjustments[provider_name] = 0

        # 清理已无记录且无优先级调整的 key，防止 dict 无限增长
        if (
            not self.failures[provider_name]
            and not self.successes[provider_name]
            and self.priority_adjustments.get(provider_name, 0) == 0
        ):
            self.failures.pop(provider_name, None)
            self.successes.pop(provider_name, None)
            self.priority_adjustments.pop(provider_name, None)

    def _get_status_label(self, failure_rate: float, recent_failures: int) -> str:
        """根据失败率返回状态标签"""
        if recent_failures >= self.failure_threshold:
            return "degraded"  # 降级
        elif failure_rate > 0.5:
            return "unstable"  # 不稳定
        elif failure_rate > 0.1:
            return "warning"  # 警告
        else:
            return "healthy"  # 健康

    def should_use_provider(self, provider_name: str) -> bool:
        """
        判断是否应该使用该提供商
        简单的策略：如果优先级调整低于-3，暂时不使用
        """
        adjustment = self.get_priority_adjustment(provider_name)
        return adjustment > -3

    def reset_provider_health(self, provider_name: str) -> None:
        """重置提供商的健康状态（管理员手动操作）"""
        self.failures[provider_name] = []
        self.successes[provider_name] = []
        self.priority_adjustments[provider_name] = 0


class SimpleProviderSelector:
    """
    简单的提供商选择器
    基于优先级和健康状态
    """

    def __init__(self, health_tracker: ProviderHealthTracker):
        self.health_tracker = health_tracker

    def select_provider(self, providers: list, specified_provider: str | None = None) -> Any:
        """
        选择提供商

        Args:
            providers: 可用提供商列表（已按基础优先级排序）
            specified_provider: 用户指定的提供商

        Returns:
            选中的提供商
        """
        # 如果用户指定了提供商，直接使用（不管健康状态）
        if specified_provider:
            return next((p for p in providers if p.name == specified_provider), None)

        # 否则，根据优先级和健康状态选择
        # 对提供商列表进行动态排序
        sorted_providers = sorted(
            providers,
            key=lambda p: (
                p.priority + self.health_tracker.get_priority_adjustment(p.name),
                -p.id,  # 相同优先级时，使用ID作为次要排序
            ),
            reverse=True,  # 优先级高的在前
        )

        # 选择第一个健康的提供商
        for provider in sorted_providers:
            if self.health_tracker.should_use_provider(provider.name):
                return provider

        # 如果都不健康，还是返回第一个（降级策略）
        return sorted_providers[0] if sorted_providers else None

    def get_provider_rankings(self, providers: list) -> list:
        """
        获取提供商的当前排名（用于调试和监控）
        """
        rankings = []
        for provider in providers:
            health_status = self.health_tracker.get_health_status(provider.name)
            effective_priority = provider.priority + health_status["priority_adjustment"]

            rankings.append(
                {
                    "name": provider.name,
                    "base_priority": provider.priority,
                    "adjustment": health_status["priority_adjustment"],
                    "effective_priority": effective_priority,
                    "status": health_status["status"],
                    "failure_rate": health_status["failure_rate"],
                }
            )

        # 按有效优先级排序
        rankings.sort(key=lambda x: x["effective_priority"], reverse=True)
        return rankings
