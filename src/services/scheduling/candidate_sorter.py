"""
候选排序器 (CandidateSorter)

从 CacheAwareScheduler 拆分出的候选排序逻辑，负责:
- 优先级模式排序（provider / global_key）
- 负载均衡模式排序
- Key 内部按优先级分组打乱
"""

from __future__ import annotations

import random
from collections import defaultdict
from typing import TYPE_CHECKING

from src.services.scheduling.scheduling_config import SchedulingConfig
from src.services.scheduling.schemas import PoolCandidate
from src.services.scheduling.utils import affinity_hash
from src.services.system.config import SystemConfigService

if TYPE_CHECKING:
    from collections.abc import Callable

    from sqlalchemy.orm import Session

    from src.models.database import ProviderAPIKey
    from src.services.scheduling.schemas import ProviderCandidate


class CandidateSorter:
    """候选排序器，负责优先级模式排序、负载均衡排序和 Key 内部打乱。"""

    def __init__(self, config: SchedulingConfig) -> None:
        self._config = config

    @staticmethod
    def _split_by_capability_match(
        candidates: list[ProviderCandidate],
    ) -> tuple[list[ProviderCandidate], list[ProviderCandidate]]:
        """按 capability_miss_count 分组：完全匹配(0)在前，部分匹配(>0)在后"""
        full_match = [c for c in candidates if c.capability_miss_count == 0]
        partial_match = [c for c in candidates if c.capability_miss_count > 0]
        return full_match, partial_match

    def _with_capability_split(
        self,
        candidates: list[ProviderCandidate],
        sort_fn: Callable[..., list[ProviderCandidate]],
        *args: object,
        **kwargs: object,
    ) -> list[ProviderCandidate]:
        """通用包装：先按 capability_miss_count 分组，再分别排序后合并"""
        if not candidates:
            return candidates
        full_match, partial_match = self._split_by_capability_match(candidates)
        return sort_fn(full_match, *args, **kwargs) + sort_fn(partial_match, *args, **kwargs)

    def _apply_priority_mode_sort(
        self,
        candidates: list[ProviderCandidate],
        db: Session,
        affinity_key: str | None = None,
        api_format: str | None = None,
    ) -> list[ProviderCandidate]:
        """
        根据优先级模式对候选列表排序（数字越小越优先）

        排序规则：
        0. 按 capability_miss_count 分组：完全匹配(0)在前，部分匹配(>0)在后
        1. 如果全局配置 keep_priority_on_conversion=True，所有候选保持原优先级
        2. 否则，按 needs_conversion 和 provider.keep_priority_on_conversion 分组：
           - 保持优先级的候选（exact 或 provider.keep_priority_on_conversion=True）按原优先级排序
           - 需要降级的候选（convertible 且 provider.keep_priority_on_conversion=False）整体排在后面
        3. 在同一组内，按优先级模式排序：
           - provider: 按 Provider.provider_priority -> Key.internal_priority 排序
           - global_key: 按 Key.global_priority_by_format 排序
        """
        if not candidates:
            return candidates

        return self._with_capability_split(
            candidates, self._apply_priority_mode_sort_inner, db, affinity_key, api_format
        )

    def _apply_priority_mode_sort_inner(
        self,
        candidates: list[ProviderCandidate],
        db: Session,
        affinity_key: str | None = None,
        api_format: str | None = None,
    ) -> list[ProviderCandidate]:
        """优先级模式排序的内部实现（不含 capability_miss_count 分组）"""
        if not candidates:
            return candidates

        # 全局配置：如果开启，所有候选保持原优先级
        global_keep_priority = SystemConfigService.is_keep_priority_on_conversion(db)

        if global_keep_priority:
            # 全局开启：不分组，直接按优先级模式排序
            if self._config.priority_mode == SchedulingConfig.PRIORITY_MODE_GLOBAL_KEY:
                return self._sort_by_global_priority_with_hash(candidates, affinity_key, api_format)
            # 提供商优先模式：保持构建时的顺序（已按 provider_priority 排序）
            return candidates

        # 全局未开启：按是否需要降级分组
        # - 不需要降级：exact 候选 或 provider.keep_priority_on_conversion=True 的 convertible 候选
        # - 需要降级：convertible 且 provider.keep_priority_on_conversion=False
        keep_priority_candidates: list[ProviderCandidate] = []
        demote_candidates: list[ProviderCandidate] = []

        for c in candidates:
            if not c.needs_conversion:
                # exact 候选：不需要降级
                keep_priority_candidates.append(c)
            elif getattr(c.provider, "keep_priority_on_conversion", False):
                # convertible 但提供商配置了保持优先级
                keep_priority_candidates.append(c)
            else:
                # convertible 且未配置保持优先级：降级
                demote_candidates.append(c)

        if self._config.priority_mode == SchedulingConfig.PRIORITY_MODE_GLOBAL_KEY:
            # 全局 Key 优先模式：分别对两组排序后合并
            sorted_keep = self._sort_by_global_priority_with_hash(
                keep_priority_candidates, affinity_key, api_format
            )
            sorted_demote = self._sort_by_global_priority_with_hash(
                demote_candidates, affinity_key, api_format
            )
            return sorted_keep + sorted_demote

        # 提供商优先模式：保持优先级的在前，降级的在后（各组内部顺序已由构建时保证）
        return keep_priority_candidates + demote_candidates

    def _sort_by_global_priority_with_hash(
        self,
        candidates: list[ProviderCandidate],
        affinity_key: str | None = None,
        api_format: str | None = None,
    ) -> list[ProviderCandidate]:
        """
        按 global_priority_by_format 分组排序，同优先级内通过哈希分散实现负载均衡

        排序逻辑：
        1. 按 global_priority_by_format[api_format] 分组（数字小的优先，NULL 排后面）
        2. 同优先级组内，使用 affinity_key 哈希分散
        3. 确保同一用户请求稳定选择同一个 Key（缓存亲和性）
        """

        def get_priority(candidate: ProviderCandidate) -> int:
            """获取候选的优先级"""
            if isinstance(candidate, PoolCandidate):
                return int(getattr(candidate, "pool_priority", 999999) or 999999)
            if not candidate.key:
                return 999999
            priority_by_format = candidate.key.global_priority_by_format or {}
            if api_format and api_format in priority_by_format:
                return priority_by_format[api_format]
            return 999999  # NULL 排在后面

        # 按优先级分组
        priority_groups: dict[int, list[ProviderCandidate]] = defaultdict(list)
        for candidate in candidates:
            priority = get_priority(candidate)
            priority_groups[priority].append(candidate)

        result = []
        for priority in sorted(priority_groups.keys()):  # 数字小的优先级高
            group = priority_groups[priority]

            if len(group) > 1 and affinity_key:
                # 同优先级内哈希分散负载均衡
                scored_candidates = []
                for candidate in group:
                    if isinstance(candidate, PoolCandidate):
                        hash_id = str(getattr(candidate.provider, "id", "") or "")
                    else:
                        hash_id = candidate.key.id if candidate.key else ""
                    hash_value = affinity_hash(affinity_key, hash_id)
                    scored_candidates.append((hash_value, candidate))

                # 按哈希值排序
                sorted_group = [c for _, c in sorted(scored_candidates, key=lambda x: x[0])]
                result.extend(sorted_group)
            else:
                # 单个候选或没有 affinity_key，按次要排序条件排序
                def secondary_sort(c: ProviderCandidate) -> tuple[int, int, str]:
                    pp = (
                        c.effective_provider_priority
                        if c.effective_provider_priority is not None
                        else c.provider.provider_priority
                    )
                    if isinstance(c, PoolCandidate):
                        ip = int(getattr(c, "pool_priority", 999999) or 999999)
                        key_id = str(getattr(c.provider, "id", "") or "")
                    else:
                        ip = c.key.internal_priority if c.key else None
                        key_id = c.key.id if c.key else ""
                    return (
                        pp if pp is not None else 999999,
                        ip if ip is not None else 999999,
                        key_id,
                    )

                result.extend(sorted(group, key=secondary_sort))

        return result

    def _apply_load_balance(
        self, candidates: list[ProviderCandidate], api_format: str | None = None
    ) -> list[ProviderCandidate]:
        """
        负载均衡模式：同优先级内随机轮换

        排序逻辑：
        0. 按 capability_miss_count 分组：完全匹配(0)在前，部分匹配(>0)在后
        1. 按优先级分组（provider_priority, internal_priority 或 global_priority_by_format）
        2. 同优先级组内随机打乱
        3. 不考虑缓存亲和性
        """
        if not candidates:
            return candidates

        return self._with_capability_split(candidates, self._apply_load_balance_inner, api_format)

    def _apply_load_balance_inner(
        self, candidates: list[ProviderCandidate], api_format: str | None = None
    ) -> list[ProviderCandidate]:
        """负载均衡排序的内部实现（不含 capability_miss_count 分组）"""
        if not candidates:
            return candidates

        priority_groups: dict[tuple, list[ProviderCandidate]] = defaultdict(list)

        # 根据优先级模式选择分组方式
        if self._config.priority_mode == SchedulingConfig.PRIORITY_MODE_GLOBAL_KEY:
            # 全局 Key 优先模式：按格式特定优先级分组
            for candidate in candidates:
                if isinstance(candidate, PoolCandidate):
                    priority = int(getattr(candidate, "pool_priority", 999999) or 999999)
                    # -1 使号池候选独立成组，不与普通 key 候选 (0) 混组打乱
                    priority_groups[(priority, -1)].append(candidate)
                    continue
                else:
                    priority = 999999
                if candidate.key:
                    priority_by_format = candidate.key.global_priority_by_format or {}
                    if api_format and api_format in priority_by_format:
                        priority = priority_by_format[api_format]
                priority_groups[(priority, 0)].append(candidate)
        else:
            # 提供商优先模式：按 (provider_priority, internal_priority) 分组
            for candidate in candidates:
                pp = (
                    candidate.effective_provider_priority
                    if candidate.effective_provider_priority is not None
                    else candidate.provider.provider_priority
                )
                if isinstance(candidate, PoolCandidate):
                    # 号池候选独立成组，不与普通 key 候选混组打乱。
                    ip = -1
                else:
                    ip = candidate.key.internal_priority if candidate.key else None
                key = (
                    pp if pp is not None else 999999,
                    ip if ip is not None else 999999,
                )
                priority_groups[key].append(candidate)

        result: list[ProviderCandidate] = []
        for priority in sorted(priority_groups.keys()):
            group = priority_groups[priority]
            if len(group) > 1:
                # 同优先级内随机打乱
                shuffled = list(group)
                random.shuffle(shuffled)
                result.extend(shuffled)
            else:
                result.extend(group)

        return result

    def shuffle_keys_by_internal_priority(
        self,
        keys: list[ProviderAPIKey],
        affinity_key: str | None = None,
        use_random: bool = False,
    ) -> list[ProviderAPIKey]:
        return self._shuffle_keys_by_internal_priority(keys, affinity_key, use_random)

    def _shuffle_keys_by_internal_priority(
        self,
        keys: list[ProviderAPIKey],
        affinity_key: str | None = None,
        use_random: bool = False,
    ) -> list[ProviderAPIKey]:
        """
        对 API Key 按 internal_priority 分组，同优先级内部基于 affinity_key 进行确定性打乱

        目的：
        - 数字越小越优先使用
        - 同优先级 Key 之间实现负载均衡
        - 使用 affinity_key 哈希确保同一请求 Key 的请求稳定（避免破坏缓存亲和性）
        - 当 use_random=True 时，使用随机排序实现轮换（用于 TTL=0 的场景）

        Args:
            keys: API Key 列表
            affinity_key: 亲和性标识符（通常为 API Key ID，用于确定性打乱）
            use_random: 是否使用随机排序（TTL=0 时为 True）

        Returns:
            排序后的 Key 列表
        """
        if not keys:
            return []

        # 按 internal_priority 分组
        priority_groups: dict[int, list[ProviderAPIKey]] = defaultdict(list)

        for key in keys:
            priority = key.internal_priority if key.internal_priority is not None else 999999
            priority_groups[priority].append(key)

        # 对每个优先级组内的 Key 进行打乱
        result = []
        for priority in sorted(priority_groups.keys()):  # 数字小的优先级高，排前面
            group_keys = priority_groups[priority]

            if len(group_keys) > 1:
                should_randomize = (
                    use_random
                    or self._config.scheduling_mode == SchedulingConfig.SCHEDULING_MODE_LOAD_BALANCE
                    or not affinity_key
                )

                if should_randomize:
                    # 随机排序：TTL=0 / 负载均衡模式 / 无 affinity_key
                    shuffled = list(group_keys)
                    random.shuffle(shuffled)
                    result.extend(shuffled)
                else:
                    # 缓存亲和模式：使用哈希确定性排序（should_randomize=False 蕴含 affinity_key 非空）
                    key_scores = []
                    for key in group_keys:
                        hash_value = affinity_hash(affinity_key, key.id)  # type: ignore[arg-type]
                        key_scores.append((hash_value, key))

                    sorted_group = [key for _, key in sorted(key_scores, key=lambda x: x[0])]
                    result.extend(sorted_group)
            else:
                # 单个 Key 直接添加
                result.extend(group_keys)

        return result
