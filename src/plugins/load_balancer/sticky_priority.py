"""
粘性优先级负载均衡策略
正常情况下始终选择同一个提供商（优先级最高+权重最大），只在故障时切换

WARNING: 多进程环境注意事项
=============================
此插件的健康状态和粘性缓存存储在进程内存中。如果使用 Gunicorn/uvicorn 多 worker 模式，
每个 worker 进程有独立的状态，可能导致：
- 不同 worker 看到的提供商健康状态不同
- 粘性路由在不同 worker 间不一致
- 统计数据分散在各个 worker 中

解决方案：
1. 单 worker 模式：适用于低流量场景
2. Redis 共享状态：将 _provider_health 和 _sticky_providers 迁移到 Redis
3. 使用独立的健康检查服务：所有 worker 共享同一个健康状态源

目前项目已有 Redis 依赖，建议在高可用场景下将状态迁移到 Redis。
参考：src/services/health_monitor.py 中的实现
"""

from __future__ import annotations

import time
from collections import defaultdict
from typing import Any

from src.core.logger import logger

from .base import LoadBalancerStrategy, ProviderCandidate, SelectionResult


class StickyPriorityStrategy(LoadBalancerStrategy):
    """
    粘性优先级策略

    选择逻辑：
    1. 在最高优先级组中，选择权重最大的提供商作为"粘性"提供商
    2. 正常情况下，始终选择该粘性提供商
    3. 只有在粘性提供商失败时，才切换到同优先级的其他提供商
    4. 当粘性提供商恢复后，自动切回

    特点：
    - 最小化提供商切换，流量集中在单一提供商
    - 自动故障转移和恢复
    - 适合需要集中使用某个API Key的场景

    Note:
        状态存储在进程内存中，多进程部署时各 worker 状态独立。
        详见模块文档说明。
    """

    def __init__(self, config: dict[str, Any] | None = None):
        config = config or {}  # 确保 config 不为 None
        super().__init__(
            name="sticky_priority",
            priority=110,  # 比默认的 priority_weighted 更高
            version="1.0.0",
            author="System",
            description="粘性优先级负载均衡策略，正常时始终使用同一提供商",
            api_version="1.0",
            provides=["load_balancer"],
            config=config,
        )

        # 配置参数
        self.failure_threshold = config.get("failure_threshold", 3)  # 连续失败阈值
        self.recovery_delay = config.get("recovery_delay", 30)  # 恢复延迟（秒）
        self.enable_auto_recovery = config.get("enable_auto_recovery", True)  # 是否自动恢复

        # 提供商健康状态追踪 {provider_id: health_info}
        self._provider_health: dict[str, dict[str, Any]] = defaultdict(
            lambda: {
                "consecutive_failures": 0,
                "last_failure_time": None,
                "is_healthy": True,
                "total_requests": 0,
                "total_failures": 0,
            }
        )
        self._max_provider_health_entries: int = 200  # 上限，防止无界增长

        # 当前粘性提供商缓存 {cache_key: provider_id}
        # cache_key 可以是 api_key_id 或者其他标识
        self._sticky_providers: dict[str, str] = {}
        self._max_sticky_entries: int = 2000  # 上限，防止无界增长

        # 统计信息
        self._stats = {
            "total_selections": 0,
            "provider_selections": {},
            "sticky_hits": 0,  # 选择粘性提供商的次数
            "failovers": 0,  # 故障切换次数
            "auto_recoveries": 0,  # 自动恢复次数
        }

    async def select(
        self, candidates: list[ProviderCandidate], context: dict[str, Any] | None = None
    ) -> SelectionResult | None:
        """
        从候选提供商中选择一个

        Args:
            candidates: 候选提供商列表
            context: 上下文信息（包含 api_key_id 等）

        Returns:
            选择结果
        """
        if not candidates:
            logger.warning("No candidates available for selection")
            return None

        if len(candidates) == 1:
            candidate = candidates[0]
            self._record_selection(candidate.provider, is_sticky=True)
            return SelectionResult(
                provider=candidate.provider,
                priority=candidate.priority,
                weight=candidate.weight,
                selection_metadata={"strategy": "single_candidate"},
            )

        # 获取缓存键（用于识别同一请求源）
        cache_key = self._get_cache_key(context)

        # 按优先级分组
        priority_groups = self._group_by_priority(candidates)
        highest_priority = max(priority_groups.keys())
        highest_group = priority_groups[highest_priority]

        # 确定粘性提供商
        sticky_candidate = self._determine_sticky_provider(highest_group, cache_key, context)

        # 检查粘性提供商是否健康
        provider_id = str(sticky_candidate.provider.id)
        health_info = self._provider_health[provider_id]

        # 如果粘性提供商健康，直接使用
        if health_info["is_healthy"]:
            self._record_selection(sticky_candidate.provider, is_sticky=True)

            logger.info(f"Selected sticky provider {sticky_candidate.provider.name}")

            return SelectionResult(
                provider=sticky_candidate.provider,
                priority=sticky_candidate.priority,
                weight=sticky_candidate.weight,
                selection_metadata={
                    "strategy": "sticky_priority",
                    "is_sticky": True,
                    "cache_key": cache_key,
                    "health_status": "healthy",
                },
            )

        # 粘性提供商不健康，选择备用提供商
        logger.warning(
            f"Sticky provider {sticky_candidate.provider.name} is unhealthy, selecting backup"
        )

        # 从同一优先级组中选择健康的备用提供商
        backup_candidate = self._select_backup_provider(highest_group)

        if not backup_candidate:
            # 如果没有健康的备用，降级使用不健康的粘性提供商
            logger.warning(
                "No healthy backup provider available, falling back to unhealthy sticky provider"
            )
            backup_candidate = sticky_candidate

        self._record_selection(backup_candidate.provider, is_sticky=False)
        self._stats["failovers"] += 1

        logger.info(f"Selected backup provider {backup_candidate.provider.name}")

        return SelectionResult(
            provider=backup_candidate.provider,
            priority=backup_candidate.priority,
            weight=backup_candidate.weight,
            selection_metadata={
                "strategy": "sticky_priority",
                "is_sticky": False,
                "is_failover": True,
                "original_provider_id": provider_id,
                "health_status": "backup",
            },
        )

    def _get_cache_key(self, context: dict[str, Any] | None) -> str:
        """
        生成缓存键，用于识别同一请求源

        Args:
            context: 上下文信息

        Returns:
            缓存键
        """
        if not context:
            return "default"

        # 优先使用 api_key_id
        if "api_key_id" in context:
            return f"api_key_{context['api_key_id']}"

        # 其他标识
        if "user_id" in context:
            return f"user_{context['user_id']}"

        return "default"

    def _group_by_priority(
        self, candidates: list[ProviderCandidate]
    ) -> dict[int, list[ProviderCandidate]]:
        """按优先级分组候选提供商"""
        groups: dict[int, list[ProviderCandidate]] = {}
        for candidate in candidates:
            priority = candidate.priority
            if priority not in groups:
                groups[priority] = []
            groups[priority].append(candidate)
        return groups

    def _determine_sticky_provider(
        self,
        candidates: list[ProviderCandidate],
        cache_key: str,
        context: dict[str, Any] | None = None,
    ) -> ProviderCandidate:
        """
        确定粘性提供商

        策略：
        1. 如果已有缓存的粘性提供商，检查是否仍在候选列表中
        2. 如果没有或已失效，选择权重最大的作为新的粘性提供商

        Args:
            candidates: 同一优先级的候选列表
            cache_key: 缓存键
            context: 上下文信息

        Returns:
            粘性提供商候选
        """
        # 检查缓存的粘性提供商
        if cache_key in self._sticky_providers:
            cached_provider_id = self._sticky_providers[cache_key]

            # 查找是否仍在候选列表中
            for candidate in candidates:
                if str(candidate.provider.id) == cached_provider_id:
                    # 检查是否可以自动恢复
                    if self._can_auto_recover(cached_provider_id):
                        logger.info(f"Auto-recovering sticky provider {candidate.provider.name}")
                        self._stats["auto_recoveries"] += 1
                        # 重置健康状态
                        self._provider_health[cached_provider_id]["is_healthy"] = True
                        self._provider_health[cached_provider_id]["consecutive_failures"] = 0

                    return candidate

        # 没有缓存或缓存失效，选择权重最大的
        sticky_candidate = max(candidates, key=lambda c: c.weight)
        # 超出上限时清理不在当前候选列表中的旧条目
        if len(self._sticky_providers) >= self._max_sticky_entries:
            active_ids = {str(c.provider.id) for c in candidates}
            stale = [k for k, v in self._sticky_providers.items() if v not in active_ids]
            for k in stale[: len(stale) // 2 or len(stale)]:
                del self._sticky_providers[k]
        self._sticky_providers[cache_key] = str(sticky_candidate.provider.id)

        logger.info(f"Set new sticky provider {sticky_candidate.provider.name}")

        return sticky_candidate

    def _can_auto_recover(self, provider_id: str) -> bool:
        """
        检查提供商是否可以自动恢复

        Args:
            provider_id: 提供商ID

        Returns:
            是否可以恢复
        """
        if not self.enable_auto_recovery:
            return False

        health_info = self._provider_health[provider_id]

        # 如果已经是健康状态，直接返回 True
        if health_info["is_healthy"]:
            return True

        # 检查是否超过恢复延迟
        if health_info["last_failure_time"]:
            time_since_failure = time.time() - health_info["last_failure_time"]
            if time_since_failure >= self.recovery_delay:
                return True

        return False

    def _select_backup_provider(
        self, candidates: list[ProviderCandidate]
    ) -> ProviderCandidate | None:
        """
        从候选列表中选择健康的备用提供商

        优先选择权重最大且健康的提供商

        Args:
            candidates: 候选提供商列表

        Returns:
            备用提供商，如果没有健康的则返回 None
        """
        healthy_candidates = []

        for candidate in candidates:
            provider_id = str(candidate.provider.id)
            health_info = self._provider_health[provider_id]

            # 检查是否可以自动恢复
            if health_info["is_healthy"] or self._can_auto_recover(provider_id):
                if not health_info["is_healthy"]:
                    # 自动恢复
                    health_info["is_healthy"] = True
                    health_info["consecutive_failures"] = 0
                    self._stats["auto_recoveries"] += 1

                healthy_candidates.append(candidate)

        if not healthy_candidates:
            return None

        # 选择权重最大的健康提供商
        return max(healthy_candidates, key=lambda c: c.weight)

    def _record_selection(self, provider: Any, is_sticky: bool = True) -> None:
        """记录选择统计"""
        self._stats["total_selections"] += 1
        provider_id = str(provider.id)

        if provider_id not in self._stats["provider_selections"]:
            self._stats["provider_selections"][provider_id] = 0
        self._stats["provider_selections"][provider_id] += 1

        if is_sticky:
            self._stats["sticky_hits"] += 1

    async def record_result(
        self,
        provider: Any,
        success: bool,
        response_time: float | None = None,
        error: Exception | None = None,
    ) -> Any:
        """
        记录请求结果，更新健康状态

        Args:
            provider: 提供商对象
            success: 是否成功
            response_time: 响应时间（秒）
            error: 错误信息（如果失败）
        """
        provider_id = str(provider.id)
        # 超出上限时淘汰健康且请求量最少的条目
        if (
            provider_id not in self._provider_health
            and len(self._provider_health) >= self._max_provider_health_entries
        ):
            healthy_keys = [
                k for k, v in self._provider_health.items() if v["is_healthy"] and k != provider_id
            ]
            if healthy_keys:
                victim = min(healthy_keys, key=lambda k: self._provider_health[k]["total_requests"])
                del self._provider_health[victim]
        health_info = self._provider_health[provider_id]

        health_info["total_requests"] += 1

        if success:
            # 成功，重置连续失败计数
            health_info["consecutive_failures"] = 0
            health_info["is_healthy"] = True

            logger.debug(f"Recorded successful result for provider {provider.name}")
        else:
            # 失败，增加连续失败计数
            health_info["consecutive_failures"] += 1
            health_info["total_failures"] += 1
            health_info["last_failure_time"] = time.time()

            # 检查是否达到失败阈值
            if health_info["consecutive_failures"] >= self.failure_threshold:
                health_info["is_healthy"] = False

                logger.warning(f"Provider {provider.name} marked as unhealthy")
            else:
                logger.debug(f"Recorded failed result for provider {provider.name}")

    async def get_stats(self) -> dict[str, Any]:
        """获取统计信息"""
        # 计算健康状态
        healthy_count = sum(1 for info in self._provider_health.values() if info["is_healthy"])

        total_providers = len(self._provider_health)

        # 计算粘性命中率
        sticky_hit_rate = 0.0
        if self._stats["total_selections"] > 0:
            sticky_hit_rate = self._stats["sticky_hits"] / self._stats["total_selections"]

        return {
            "strategy": "sticky_priority",
            "total_selections": self._stats["total_selections"],
            "provider_selections": self._stats["provider_selections"],
            "sticky_hits": self._stats["sticky_hits"],
            "sticky_hit_rate": sticky_hit_rate,
            "failovers": self._stats["failovers"],
            "auto_recoveries": self._stats["auto_recoveries"],
            "healthy_providers": healthy_count,
            "total_providers": total_providers,
            "provider_health": {
                provider_id: {
                    "is_healthy": info["is_healthy"],
                    "consecutive_failures": info["consecutive_failures"],
                    "total_requests": info["total_requests"],
                    "total_failures": info["total_failures"],
                    "failure_rate": (
                        info["total_failures"] / info["total_requests"]
                        if info["total_requests"] > 0
                        else 0
                    ),
                    "last_failure_time": info["last_failure_time"],
                }
                for provider_id, info in self._provider_health.items()
            },
            "sticky_providers": self._sticky_providers,
            "config": {
                "failure_threshold": self.failure_threshold,
                "recovery_delay": self.recovery_delay,
                "enable_auto_recovery": self.enable_auto_recovery,
            },
        }

    async def reset_provider_health(self, provider_id: str) -> None:
        """重置指定提供商的健康状态"""
        if provider_id in self._provider_health:
            self._provider_health[provider_id] = {
                "consecutive_failures": 0,
                "last_failure_time": None,
                "is_healthy": True,
                "total_requests": 0,
                "total_failures": 0,
            }
            logger.info(f"Reset health status for provider {provider_id}")

    async def clear_sticky_cache(self, cache_key: str | None = None) -> None:
        """
        清除粘性提供商缓存

        Args:
            cache_key: 指定要清除的缓存键，None 则清除全部
        """
        if cache_key:
            if cache_key in self._sticky_providers:
                del self._sticky_providers[cache_key]
                logger.info(f"Cleared sticky provider cache for key: {cache_key}")
        else:
            self._sticky_providers.clear()
            logger.info("Cleared all sticky provider cache")
