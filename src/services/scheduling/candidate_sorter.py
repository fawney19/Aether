"""
候选排序器 (CandidateSorter)

从 CacheAwareScheduler 拆分出的候选排序逻辑，负责:
- 模型分组路由优先级排序
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
    """候选排序器，负责路由优先级排序、负载均衡排序和 Key 内部打乱。"""

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

    def _sort_by_route_priority(
        self,
        candidates: list[ProviderCandidate],
        db: Session,
        affinity_key: str | None = None,
        api_format: str | None = None,
    ) -> list[ProviderCandidate]:
        """
        根据模型分组路由优先级对候选列表排序（数字越小越优先）

        排序规则：
        0. 按 capability_miss_count 分组：完全匹配(0)在前，部分匹配(>0)在后
        1. 如果全局配置 keep_priority_on_conversion=True，所有候选保持原优先级
        2. 否则，按 needs_conversion 和 provider.keep_priority_on_conversion 分组：
           - 保持优先级的候选（exact 或 provider.keep_priority_on_conversion=True）按原优先级排序
           - 需要降级的候选（convertible 且 provider.keep_priority_on_conversion=False）整体排在后面
        3. 在同一组内，按候选的有效路由优先级排序；
           若优先级相同，再按 Key.internal_priority 和稳定哈希打散。
        """
        if not candidates:
            return candidates

        return self._with_capability_split(
            candidates, self._sort_by_route_priority_inner, db, affinity_key, api_format
        )

    def _sort_by_route_priority_inner(
        self,
        candidates: list[ProviderCandidate],
        db: Session,
        affinity_key: str | None = None,
        api_format: str | None = None,
    ) -> list[ProviderCandidate]:
        """模型组路由优先级排序的内部实现（不含 capability_miss_count 分组）"""
        if not candidates:
            return candidates

        # 全局配置：如果开启，所有候选保持原优先级分组，只应用新的路由排序
        global_keep_priority = SystemConfigService.is_keep_priority_on_conversion(db)

        if global_keep_priority:
            return self._sort_by_effective_priority_with_hash(candidates, affinity_key, api_format)

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

        sorted_keep = self._sort_by_effective_priority_with_hash(
            keep_priority_candidates, affinity_key, api_format
        )
        sorted_demote = self._sort_by_effective_priority_with_hash(
            demote_candidates, affinity_key, api_format
        )
        return sorted_keep + sorted_demote

    @staticmethod
    def _candidate_priority_tuple(candidate: ProviderCandidate) -> tuple[int, int, int, str]:
        binding_priority_raw = candidate.user_group_binding_priority
        try:
            binding_priority = (
                int(binding_priority_raw) if binding_priority_raw is not None else 999999
            )
        except Exception:
            binding_priority = 999999

        effective_priority_raw = (
            candidate.effective_provider_priority
            if candidate.effective_provider_priority is not None
            else getattr(candidate.provider, "provider_priority", None)
        )
        try:
            effective_priority = (
                int(effective_priority_raw) if effective_priority_raw is not None else 999999
            )
        except Exception:
            effective_priority = 999999

        if isinstance(candidate, PoolCandidate):
            secondary = 0
            identifier = str(getattr(candidate.provider, "id", "") or "")
        else:
            internal_priority_raw = candidate.key.internal_priority if candidate.key else None
            try:
                secondary = (
                    int(internal_priority_raw) if internal_priority_raw is not None else 999999
                )
            except Exception:
                secondary = 999999
            identifier = candidate.key.id if candidate.key else ""

        return binding_priority, effective_priority, secondary, identifier

    def _sort_by_effective_priority_with_hash(
        self,
        candidates: list[ProviderCandidate],
        affinity_key: str | None = None,
        api_format: str | None = None,
    ) -> list[ProviderCandidate]:
        del api_format
        if not candidates:
            return candidates

        priority_groups: dict[tuple[int, int, int], list[ProviderCandidate]] = defaultdict(list)
        for candidate in candidates:
            binding_priority, effective_priority, secondary, _identifier = (
                self._candidate_priority_tuple(candidate)
            )
            priority_groups[(binding_priority, effective_priority, secondary)].append(candidate)

        result: list[ProviderCandidate] = []
        for group_key in sorted(priority_groups.keys()):
            group = priority_groups[group_key]

            if len(group) > 1 and affinity_key:
                scored_candidates = []
                for candidate in group:
                    _, _, _, identifier = self._candidate_priority_tuple(candidate)
                    scored_candidates.append((affinity_hash(affinity_key, identifier), candidate))
                result.extend(candidate for _, candidate in sorted(scored_candidates, key=lambda x: x[0]))
            else:
                result.extend(
                    sorted(group, key=lambda candidate: self._candidate_priority_tuple(candidate))
                )

        return result

    def _apply_load_balance(
        self, candidates: list[ProviderCandidate], api_format: str | None = None
    ) -> list[ProviderCandidate]:
        """
        负载均衡模式：同优先级内随机轮换

        排序逻辑：
        0. 按 capability_miss_count 分组：完全匹配(0)在前，部分匹配(>0)在后
        1. 按有效路由优先级分组
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

        del api_format

        priority_groups: dict[tuple[int, int, int], list[ProviderCandidate]] = defaultdict(list)
        for candidate in candidates:
            binding_priority, effective_priority, secondary, _identifier = (
                self._candidate_priority_tuple(candidate)
            )
            priority_groups[(binding_priority, effective_priority, secondary)].append(candidate)

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
