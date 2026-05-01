"""
调度器核心数据类型

从 CacheAwareScheduler 提取的共享数据结构，被 24+ 个模块使用。
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import TYPE_CHECKING, Any

from src.models.database import (
    Provider,
    ProviderAPIKey,
    ProviderEndpoint,
)

if TYPE_CHECKING:
    from src.services.provider.pool.config import PoolConfig


@dataclass
class ProviderCandidate:
    """候选 provider 组合及是否命中缓存"""

    provider: Provider
    endpoint: ProviderEndpoint
    key: ProviderAPIKey
    is_cached: bool = False
    is_skipped: bool = False  # 是否被跳过
    skip_reason: str | None = None  # 跳过原因
    mapping_matched_model: str | None = None  # 通过映射匹配到的模型名（用于实际请求）
    needs_conversion: bool = False  # 是否需要格式转换
    provider_api_format: str = ""  # Provider 端点实际格式（用于健康度/熔断 bucket）
    output_limit: int | None = None  # GlobalModel 配置的模型输出上限
    capability_miss_count: int = 0  # COMPATIBLE 能力不匹配数（0=完全匹配，用于排序）
    effective_provider_priority: int | None = None  # 模型分组覆盖后的 Provider 优先级
    model_group_id: str | None = None
    model_group_name: str | None = None
    model_group_route_id: str | None = None
    user_billing_multiplier: float = 1.0
    # 用户分组绑定该模型分组的 priority（数字越小越靠前）；跨分组聚合后
    # _candidate_priority_tuple 以此为首要排序键，保证分组间不交错。
    user_group_binding_priority: int | None = None
    cache_affinity_degraded: bool = False

    def _stable_order_key(self) -> tuple[int, int, str, str, str]:
        """
        为排序/优先队列提供稳定的比较键。

        说明：
        - 运行时偶发会出现对 ProviderCandidate 做 tuple 排序/heap 排序的场景；
          当主键相同需要比较候选本身时，若候选不可比较会触发：
          TypeError: '<' not supported between instances of 'ProviderCandidate' and 'ProviderCandidate'
        - 这里提供一个与调度逻辑无关、但足够稳定且可比的兜底顺序。
        """
        provider_priority_raw = (
            self.effective_provider_priority
            if self.effective_provider_priority is not None
            else getattr(self.provider, "provider_priority", None)
        )
        internal_priority_raw = getattr(self.key, "internal_priority", None)

        try:
            provider_priority = (
                int(provider_priority_raw) if provider_priority_raw is not None else 999999
            )
        except Exception:
            provider_priority = 999999

        try:
            internal_priority = (
                int(internal_priority_raw) if internal_priority_raw is not None else 999999
            )
        except Exception:
            internal_priority = 999999

        provider_id = str(getattr(self.provider, "id", "") or "")
        endpoint_id = str(getattr(self.endpoint, "id", "") or "")
        key_id = str(getattr(self.key, "id", "") or "")
        return (provider_priority, internal_priority, provider_id, endpoint_id, key_id)

    def __lt__(self, other: object) -> bool:
        if not isinstance(other, ProviderCandidate):
            return NotImplemented
        return self._stable_order_key() < other._stable_order_key()


@dataclass
class PoolCandidate(ProviderCandidate):
    """号池候选。

    排序阶段作为单个候选参与；执行阶段再在 pool_keys 内部选择/切换 key。
    """

    pool_keys: list[ProviderAPIKey] = field(default_factory=list)
    pool_config: PoolConfig | None = None
    pool_priority: int = 999999
    _pool_key_index: int = 0
    # 延迟可用性检查参数（号池优化：先排序再分页检查）
    _deferred_check_params: dict[str, Any] | None = field(default=None, repr=False)


@dataclass
class ConcurrencySnapshot:
    key_current: int
    key_limit: int | None
    is_cached_user: bool = False
    # 动态预留信息
    reservation_ratio: float = 0.0
    reservation_phase: str = "unknown"
    reservation_confidence: float = 0.0
    load_factor: float = 0.0

    def describe(self) -> str:
        key_limit_text = str(self.key_limit) if self.key_limit is not None else "inf"
        reservation_text = f"{self.reservation_ratio:.0%}" if self.reservation_ratio > 0 else "N/A"
        return (
            f"key={self.key_current}/{key_limit_text}, "
            f"cached={self.is_cached_user}, "
            f"reserve={reservation_text}({self.reservation_phase})"
        )
