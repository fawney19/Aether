"""
缓存感知调度器 (Cache-Aware Scheduler)

职责:
1. 统一管理Provider/Endpoint/Key的选择逻辑
2. 集成缓存亲和性管理，优先使用有缓存的Provider+Key
3. 协调并发控制和缓存优先级
4. 实现故障转移机制（同Endpoint内优先，跨Provider按优先级）

核心设计思想:
===============
1. 用户首次请求: 按 provider_priority 选择最优 Provider+Endpoint+Key
2. 用户后续请求:
   - 优先使用缓存的Endpoint+Key (利用Prompt Caching)
   - 如果缓存的Key并发满，尝试同Endpoint其他Key
   - 如果Endpoint不可用，按 provider_priority 切换到其他Provider

3. 并发控制（动态预留机制）:
   - 探测阶段：使用低预留（10%），让系统快速学习真实并发限制
   - 稳定阶段：根据置信度和负载动态调整预留比例（10%-35%）
   - 置信度因素：连续成功次数、429冷却时间、调整历史稳定性
   - 缓存用户可使用全部槽位，新用户只能用 (1-预留比例) 的槽位

4. 故障转移:
   - Key故障: 同Endpoint内切换其他Key（检查模型支持）
   - Endpoint故障: 按 provider_priority 切换到其他Provider
   - 注意：不同Endpoint的协议完全不兼容，不能在同Provider内切换Endpoint
   - 失效缓存亲和性，避免重复选择故障资源
"""

from __future__ import annotations

import hashlib
import random
import re
import time
from dataclasses import dataclass
from typing import TYPE_CHECKING, Any, Dict, List, Optional, Tuple, Union

from sqlalchemy.orm import Session, selectinload

from src.core.enums import APIFormat
from src.core.exceptions import ModelNotSupportedException, ProviderNotAvailableException
from src.core.logger import logger
from src.models.database import (
    ApiKey,
    Model,
    Provider,
    ProviderAPIKey,
    ProviderEndpoint,
)

if TYPE_CHECKING:
    from src.models.database import GlobalModel

from src.services.cache.affinity_manager import (
    CacheAffinityManager,
    get_affinity_manager,
)
from src.services.cache.model_cache import ModelCacheService
from src.services.health.monitor import health_monitor
from src.services.provider.format import normalize_api_format
from src.services.rate_limit.adaptive_reservation import (
    AdaptiveReservationManager,
    get_adaptive_reservation_manager,
)
from src.services.rate_limit.concurrency_manager import get_concurrency_manager


@dataclass
class ProviderCandidate:
    """候选 provider 组合及是否命中缓存"""

    provider: Provider
    endpoint: ProviderEndpoint
    key: ProviderAPIKey
    is_cached: bool = False
    is_skipped: bool = False  # 是否被跳过
    skip_reason: Optional[str] = None  # 跳过原因
    alias_matched_model: Optional[str] = None  # 通过别名匹配到的模型名（用于实际请求）


@dataclass
class ConcurrencySnapshot:
    key_current: int
    key_limit: Optional[int]
    is_cached_user: bool = False
    # 动态预留信息
    reservation_ratio: float = 0.0
    reservation_phase: str = "unknown"
    reservation_confidence: float = 0.0

    def describe(self) -> str:
        key_limit_text = str(self.key_limit) if self.key_limit is not None else "inf"
        reservation_text = f"{self.reservation_ratio:.0%}" if self.reservation_ratio > 0 else "N/A"
        return (
            f"key={self.key_current}/{key_limit_text}, "
            f"cached={self.is_cached_user}, "
            f"reserve={reservation_text}({self.reservation_phase})"
        )


class CacheAwareScheduler:
    """
    缓存感知调度器

    这是Provider选择的核心组件，整合了:
    - Provider/Endpoint/Key三层架构
    - 缓存亲和性管理
    - 并发控制（动态预留机制）
    - 健康度监控
    """

    # 优先级模式常量
    PRIORITY_MODE_PROVIDER = "provider"  # 提供商优先模式
    PRIORITY_MODE_GLOBAL_KEY = "global_key"  # 全局 Key 优先模式
    ALLOWED_PRIORITY_MODES = {
        PRIORITY_MODE_PROVIDER,
        PRIORITY_MODE_GLOBAL_KEY,
    }
    # 调度模式常量
    SCHEDULING_MODE_FIXED_ORDER = "fixed_order"  # 固定顺序模式：严格按优先级，忽略缓存
    SCHEDULING_MODE_CACHE_AFFINITY = "cache_affinity"  # 缓存亲和模式：优先缓存，同优先级哈希分散
    SCHEDULING_MODE_LOAD_BALANCE = "load_balance"  # 负载均衡模式：忽略缓存，同优先级随机轮换
    ALLOWED_SCHEDULING_MODES = {
        SCHEDULING_MODE_FIXED_ORDER,
        SCHEDULING_MODE_CACHE_AFFINITY,
        SCHEDULING_MODE_LOAD_BALANCE,
    }

    def __init__(
        self, redis_client=None, priority_mode: Optional[str] = None, scheduling_mode: Optional[str] = None
    ):
        """
        初始化调度器

        注意: 不再持久化 db Session,避免跨请求使用已关闭的会话
        每个方法调用时需要传入当前请求的 db Session

        Args:
            redis_client: Redis客户端（可选）
            priority_mode: 候选排序策略（provider | global_key）
            scheduling_mode: 调度模式（fixed_order | cache_affinity）
        """
        self.redis = redis_client
        self.priority_mode = self._normalize_priority_mode(
            priority_mode or self.PRIORITY_MODE_PROVIDER
        )
        self.scheduling_mode = self._normalize_scheduling_mode(
            scheduling_mode or self.SCHEDULING_MODE_CACHE_AFFINITY
        )
        logger.debug(f"[CacheAwareScheduler] 初始化优先级模式: {self.priority_mode}, 调度模式: {self.scheduling_mode}")

        # 初始化子组件（将在第一次使用时异步初始化）
        self._affinity_manager: Optional[CacheAffinityManager] = None
        self._concurrency_manager = None
        # 动态预留管理器（同步初始化）
        self._reservation_manager: AdaptiveReservationManager = get_adaptive_reservation_manager()
        self._metrics = {
            "total_batches": 0,
            "last_batch_size": 0,
            "total_candidates": 0,
            "last_candidate_count": 0,
            "cache_hits": 0,
            "cache_misses": 0,
            "concurrency_denied": 0,
            "last_api_format": None,
            "last_model_name": None,
            "last_updated_at": None,
            # 动态预留相关指标
            "reservation_probe_count": 0,
            "reservation_stable_count": 0,
            "avg_reservation_ratio": 0.0,
            "last_reservation_result": None,
        }

    async def _ensure_initialized(self):
        """确保所有异步组件已初始化"""
        if self._affinity_manager is None:
            self._affinity_manager = await get_affinity_manager(self.redis)

        if self._concurrency_manager is None:
            self._concurrency_manager = await get_concurrency_manager()

    async def select_with_cache_affinity(
        self,
        db: Session,
        affinity_key: str,
        api_format: Union[str, APIFormat],
        model_name: str,
        excluded_endpoints: Optional[List[str]] = None,
        excluded_keys: Optional[List[str]] = None,
        provider_batch_size: int = 20,
        max_candidates_per_batch: Optional[int] = None,
    ) -> Tuple[Provider, ProviderEndpoint, ProviderAPIKey]:
        """
        缓存感知选择 - 核心方法

        逻辑：一次性获取所有候选（缓存命中优先），按顺序检查
        排除列表和并发限制，返回首个可用组合，并在需要时刷新缓存亲和性。

        Args:
            db: 数据库会话
            affinity_key: 亲和性标识符（通常为API Key ID）
            api_format: API格式
            model_name: 模型名称
            excluded_endpoints: 排除的Endpoint ID列表
            excluded_keys: 排除的Provider Key ID列表
            provider_batch_size: Provider批量大小
            max_candidates_per_batch: 每批最大候选数
        """
        await self._ensure_initialized()

        excluded_endpoints_set = set(excluded_endpoints or [])
        excluded_keys_set = set(excluded_keys or [])

        normalized_format = normalize_api_format(api_format)

        logger.debug(
            f"[CacheAwareScheduler] select_with_cache_affinity: "
            f"affinity_key={affinity_key[:8]}..., api_format={normalized_format.value}, model={model_name}"
        )

        self._metrics["last_api_format"] = normalized_format.value
        self._metrics["last_model_name"] = model_name

        provider_offset = 0

        global_model_id = None  # 用于缓存亲和性

        while True:
            candidates, resolved_global_model_id = await self.list_all_candidates(
                db=db,
                api_format=normalized_format,
                model_name=model_name,
                affinity_key=affinity_key,
                provider_offset=provider_offset,
                provider_limit=provider_batch_size,
                max_candidates=max_candidates_per_batch,
            )

            if resolved_global_model_id and global_model_id is None:
                global_model_id = resolved_global_model_id

            if not candidates:
                if provider_offset == 0:
                    # 没有找到任何候选，提供友好的错误提示（不暴露内部信息）
                    raise ProviderNotAvailableException("请求的模型当前不可用")
                break

            self._metrics["total_batches"] += 1
            self._metrics["last_batch_size"] = len(candidates)
            self._metrics["last_updated_at"] = int(time.time())

            for candidate in candidates:
                provider = candidate.provider
                endpoint = candidate.endpoint
                key = candidate.key

                if endpoint.id in excluded_endpoints_set:
                    logger.debug(f"  └─ Endpoint {endpoint.id[:8]}... 在排除列表，跳过")
                    continue

                if key.id in excluded_keys_set:
                    logger.debug(f"  └─ Key {key.id[:8]}... 在排除列表，跳过")
                    continue

                is_cached_user = bool(candidate.is_cached)
                can_use, snapshot = await self._check_concurrent_available(
                    key,
                    is_cached_user=is_cached_user,
                )

                if not can_use:
                    logger.debug(f"  └─ Key {key.id[:8]}... 并发已满 ({snapshot.describe()})")
                    self._metrics["concurrency_denied"] += 1
                    continue

                logger.debug(
                    f"  └─ 选择 Provider={provider.name}, Endpoint={endpoint.id[:8]}..., "
                    f"Key=***{key.api_key[-4:]}, 缓存命中={is_cached_user}, "
                    f"并发状态[{snapshot.describe()}]"
                )

                if key.cache_ttl_minutes > 0 and global_model_id:
                    ttl = key.cache_ttl_minutes * 60 if key.cache_ttl_minutes > 0 else None
                    api_format_str = (
                        normalized_format.value
                        if isinstance(normalized_format, APIFormat)
                        else normalized_format
                    )
                    await self.set_cache_affinity(
                        affinity_key=affinity_key,
                        provider_id=str(provider.id),
                        endpoint_id=str(endpoint.id),
                        key_id=str(key.id),
                        api_format=api_format_str,
                        global_model_id=global_model_id,
                        ttl=int(ttl) if ttl is not None else None,
                    )

                if is_cached_user:
                    self._metrics["cache_hits"] += 1
                else:
                    self._metrics["cache_misses"] += 1

                return provider, endpoint, key

            provider_offset += provider_batch_size

        raise ProviderNotAvailableException("服务暂时繁忙，请稍后重试")

    def _get_effective_rpm_limit(self, key: ProviderAPIKey) -> Optional[int]:
        """
        获取有效的 RPM 限制

        新逻辑：
        - rpm_limit=NULL: 启用自适应，使用 learned_rpm_limit（如无学习记录则使用默认初始值）
        - rpm_limit=数字: 固定限制，直接使用该值

        Args:
            key: API Key对象

        Returns:
            有效的 RPM 限制（None 表示不限制）
        """
        if key.rpm_limit is None:
            # 自适应模式：使用学习到的值
            learned = key.learned_rpm_limit
            if learned is not None:
                return int(learned)

            # 未学习到值时，使用默认初始限制，避免无限制打爆上游
            from src.config.constants import RPMDefaults

            return int(RPMDefaults.INITIAL_LIMIT)
        else:
            # 固定限制模式
            return int(key.rpm_limit)

    async def _check_concurrent_available(
        self,
        key: ProviderAPIKey,
        is_cached_user: bool = False,
    ) -> Tuple[bool, ConcurrencySnapshot]:
        """
        检查 RPM 限制是否可用（使用动态预留机制）

        核心逻辑 - 动态缓存预留机制:
        - 总槽位: 有效 RPM 限制（固定值或学习到的值）
        - 预留比例: 由 AdaptiveReservationManager 根据置信度和负载动态计算
        - 缓存用户可用: 全部槽位
        - 新用户可用: 总槽位 × (1 - 动态预留比例)

        Args:
            key: ProviderAPIKey对象
            is_cached_user: 是否是缓存用户

        Returns:
            (是否可用, 并发快照)
        """
        # 获取有效的并发限制
        effective_key_limit = self._get_effective_rpm_limit(key)

        logger.debug(
            f"            -> 并发检查: _concurrency_manager={self._concurrency_manager is not None}, "
            f"is_cached_user={is_cached_user}, effective_limit={effective_key_limit}"
        )

        if not self._concurrency_manager:
            # 并发管理器不可用，直接返回True
            logger.debug(f"            -> 无并发管理器，直接通过")
            snapshot = ConcurrencySnapshot(
                key_current=0,
                key_limit=effective_key_limit,
                is_cached_user=is_cached_user,
            )
            return True, snapshot

        # 获取当前 RPM 计数
        key_count = await self._concurrency_manager.get_key_rpm_count(
            key_id=str(key.id),
        )

        can_use = True

        # 计算动态预留比例
        reservation_result = self._reservation_manager.calculate_reservation(
            key=key,
            current_usage=key_count,
            effective_limit=effective_key_limit,
        )

        # 更新指标
        if reservation_result.phase == "probe":
            self._metrics["reservation_probe_count"] += 1
        else:
            self._metrics["reservation_stable_count"] += 1

        # 计算移动平均预留比例
        total_reservations = (
            self._metrics["reservation_probe_count"] + self._metrics["reservation_stable_count"]
        )
        if total_reservations > 0:
            # 指数移动平均
            alpha = 0.1
            self._metrics["avg_reservation_ratio"] = (
                alpha * reservation_result.ratio
                + (1 - alpha) * self._metrics["avg_reservation_ratio"]
            )

        self._metrics["last_reservation_result"] = {
            "ratio": reservation_result.ratio,
            "phase": reservation_result.phase,
            "confidence": reservation_result.confidence,
            "load_factor": reservation_result.load_factor,
        }

        available_for_new = None
        reservation_ratio = reservation_result.ratio

        # 检查Key级别限制（使用动态预留比例）
        if effective_key_limit is not None:
            if is_cached_user:
                # 缓存用户: 可以使用全部槽位
                if key_count >= effective_key_limit:
                    can_use = False
            else:
                # 新用户: 只能使用 (1 - 动态预留比例) 的槽位
                # 使用 max 确保至少有 1 个槽位可用
                import math

                # 与 ConcurrencyManager 的 Lua 脚本保持一致：使用 floor 计算新用户可用槽位
                available_for_new = max(1, math.floor(effective_key_limit * (1 - reservation_ratio)))
                if key_count >= available_for_new:
                    logger.debug(
                        f"Key {key.id[:8]}... 新用户配额已满 "
                        f"({key_count}/{available_for_new}, 总{effective_key_limit}, "
                        f"预留{reservation_ratio:.0%}[{reservation_result.phase}])"
                    )
                    can_use = False

        key_limit_for_snapshot: Optional[int]
        if is_cached_user:
            key_limit_for_snapshot = effective_key_limit
        elif effective_key_limit is not None:
            key_limit_for_snapshot = (
                available_for_new if available_for_new is not None else effective_key_limit
            )
        else:
            key_limit_for_snapshot = None

        snapshot = ConcurrencySnapshot(
            key_current=key_count,
            key_limit=key_limit_for_snapshot,
            is_cached_user=is_cached_user,
            reservation_ratio=reservation_ratio,
            reservation_phase=reservation_result.phase,
            reservation_confidence=reservation_result.confidence,
        )

        return can_use, snapshot

    def _get_effective_restrictions(
        self,
        user_api_key: Optional[ApiKey],
    ) -> Dict[str, Any]:
        """
        获取有效的访问限制（合并 ApiKey 和 User 的限制）

        逻辑：
        - 如果 ApiKey 和 User 都有限制，取交集
        - 如果只有一方有限制，使用该方的限制
        - 如果都没有限制，返回 None（表示不限制）

        Args:
            user_api_key: 用户 API Key 对象（可能包含 user relationship）

        Returns:
            包含 allowed_providers, allowed_models, allowed_api_formats 的字典
        """
        result = {
            "allowed_providers": None,
            "allowed_models": None,
            "allowed_api_formats": None,
        }

        if not user_api_key:
            return result

        # 获取 User 的限制
        # 注意：这里可能触发 lazy loading，需要确保 session 仍然有效
        try:
            user = user_api_key.user if hasattr(user_api_key, "user") else None
        except Exception as e:
            logger.warning(f"无法加载 ApiKey 关联的 User: {e}，仅使用 ApiKey 级别的限制")
            user = None

        # 调试日志
        logger.debug(
            f"[_get_effective_restrictions] ApiKey={user_api_key.id[:8]}..., "
            f"User={user.id[:8] if user else 'None'}..., "
            f"ApiKey.allowed_models={user_api_key.allowed_models}, "
            f"User.allowed_models={user.allowed_models if user else 'N/A'}"
        )

        def merge_restrictions(key_restriction, user_restriction):
            """合并两个限制列表，返回有效的限制集合"""
            key_set = set(key_restriction) if key_restriction else None
            user_set = set(user_restriction) if user_restriction else None

            if key_set and user_set:
                # 两者都有限制，取交集
                return key_set & user_set
            elif key_set:
                return key_set
            elif user_set:
                return user_set
            else:
                return None

        # 合并 allowed_providers
        result["allowed_providers"] = merge_restrictions(
            user_api_key.allowed_providers, user.allowed_providers if user else None
        )

        # 合并 allowed_models
        # allowed_models 支持 list/dict 两种结构，不能转成 set 否则会导致权限校验失效
        from src.core.model_permissions import merge_allowed_models

        result["allowed_models"] = merge_allowed_models(
            user_api_key.allowed_models, user.allowed_models if user else None
        )

        # 合并 allowed_api_formats
        result["allowed_api_formats"] = merge_restrictions(
            user_api_key.allowed_api_formats,
            user.allowed_api_formats if user else None
        )

        return result

    async def list_all_candidates(
        self,
        db: Session,
        api_format: Union[str, APIFormat],
        model_name: str,
        affinity_key: Optional[str] = None,
        user_api_key: Optional[ApiKey] = None,
        provider_offset: int = 0,
        provider_limit: Optional[int] = None,
        max_candidates: Optional[int] = None,
        is_stream: bool = False,
        capability_requirements: Optional[Dict[str, bool]] = None,
    ) -> Tuple[List[ProviderCandidate], str]:
        """
        预先获取所有可用的 Provider/Endpoint/Key 组合

        重构后的方法将逻辑拆分为：
        1. _query_providers: 数据库查询逻辑
        2. _build_candidates: 候选构建逻辑
        3. _apply_cache_affinity: 缓存亲和性处理

        Args:
            db: 数据库会话
            api_format: API 格式
            model_name: 模型名称
            affinity_key: 亲和性标识符（通常为API Key ID，用于缓存亲和性）
            user_api_key: 用户 API Key（用于访问限制过滤，同时考虑 User 级别限制）
            provider_offset: Provider 分页偏移
            provider_limit: Provider 分页限制
            max_candidates: 最大候选数量
            is_stream: 是否是流式请求，如果为 True 则过滤不支持流式的 Provider
            capability_requirements: 能力需求（用于过滤不满足能力要求的 Key）

        Returns:
            (候选列表, global_model_id) - global_model_id 用于缓存亲和性
        """
        await self._ensure_initialized()

        target_format = normalize_api_format(api_format)

        # 0. 解析 model_name 到 GlobalModel（支持直接匹配和映射名匹配，使用 ModelCacheService）
        global_model = await ModelCacheService.resolve_global_model_by_name_or_alias(db, model_name)

        if not global_model:
            logger.warning(f"GlobalModel not found: {model_name}")
            raise ModelNotSupportedException(model=model_name)

        # 使用 GlobalModel.id 作为缓存亲和性的模型标识，确保映射名和规范名都能命中同一个缓存
        global_model_id: str = str(global_model.id)
        requested_model_name = model_name
        resolved_model_name = str(global_model.name)

        # 提取模型别名（用于 Provider Key 的 allowed_models 匹配）
        model_aliases: List[str] = (global_model.config or {}).get("model_aliases", [])

        # 获取合并后的访问限制（ApiKey + User）
        restrictions = self._get_effective_restrictions(user_api_key)
        allowed_api_formats = restrictions["allowed_api_formats"]
        allowed_providers = restrictions["allowed_providers"]
        allowed_models = restrictions["allowed_models"]

        # 0.1 检查 API 格式是否被允许
        if allowed_api_formats is not None:
            # 统一转为大写比较，兼容数据库中存储的大小写
            allowed_upper = {f.upper() for f in allowed_api_formats}
            if target_format.value.upper() not in allowed_upper:
                logger.debug(
                    f"API Key {user_api_key.id[:8] if user_api_key else 'N/A'}... 不允许使用 API 格式 {target_format.value}, "
                    f"允许的格式: {allowed_api_formats}"
                )
                return [], global_model_id

        # 0.2 检查模型是否被允许（支持简单列表和按格式字典两种模式）
        from src.core.model_permissions import check_model_allowed, get_allowed_models_preview

        if not check_model_allowed(
            model_name=requested_model_name,
            allowed_models=allowed_models,
            api_format=target_format.value,
            resolved_model_name=resolved_model_name,
        ):
            resolved_note = (
                f" (解析为 {resolved_model_name})"
                if resolved_model_name != requested_model_name
                else ""
            )
            logger.debug(
                f"用户/API Key 不允许使用模型 {requested_model_name}{resolved_note}, "
                f"允许的模型: {get_allowed_models_preview(allowed_models)}"
            )
            return [], global_model_id

        # 1. 查询 Providers
        providers = self._query_providers(
            db=db,
            provider_offset=provider_offset,
            provider_limit=provider_limit,
        )

        if not providers:
            return [], global_model_id

        # 1.5 根据 allowed_providers 过滤（合并 ApiKey 和 User 的限制）
        if allowed_providers is not None:
            original_count = len(providers)
            # 同时支持 provider id 和 name 匹配
            providers = [
                p for p in providers if p.id in allowed_providers or p.name in allowed_providers
            ]
            if original_count != len(providers):
                logger.debug(f"用户/API Key 过滤 Provider: {original_count} -> {len(providers)}")

        if not providers:
            return [], global_model_id

        # 2. 构建候选列表（传入 is_stream 和 capability_requirements 用于过滤）
        candidates = await self._build_candidates(
            db=db,
            providers=providers,
            target_format=target_format,
            model_name=requested_model_name,
            resolved_model_name=resolved_model_name,
            model_aliases=model_aliases,
            affinity_key=affinity_key,
            max_candidates=max_candidates,
            is_stream=is_stream,
            capability_requirements=capability_requirements,
        )

        # 3. 应用优先级模式排序
        candidates = self._apply_priority_mode_sort(candidates, affinity_key)

        # 更新指标
        self._metrics["total_candidates"] += len(candidates)
        self._metrics["last_candidate_count"] = len(candidates)

        logger.debug(
            f"预先获取到 {len(candidates)} 个可用组合 "
            f"(api_format={target_format.value}, model={model_name})"
        )

        # 4. 根据调度模式应用不同的排序策略
        if self.scheduling_mode == self.SCHEDULING_MODE_CACHE_AFFINITY:
            # 缓存亲和模式：优先使用缓存的，同优先级内哈希分散
            if affinity_key and candidates:
                candidates = await self._apply_cache_affinity(
                    candidates=candidates,
                    affinity_key=affinity_key,
                    api_format=target_format,
                    global_model_id=global_model_id,
                )
        elif self.scheduling_mode == self.SCHEDULING_MODE_LOAD_BALANCE:
            # 负载均衡模式：忽略缓存，同优先级内随机轮换
            candidates = self._apply_load_balance(candidates)
            for candidate in candidates:
                candidate.is_cached = False
        else:
            # 固定顺序模式：严格按优先级，忽略缓存
            for candidate in candidates:
                candidate.is_cached = False

        return candidates, global_model_id

    def _query_providers(
        self,
        db: Session,
        provider_offset: int = 0,
        provider_limit: Optional[int] = None,
    ) -> List[Provider]:
        """
        查询活跃的 Providers（带预加载）

        Args:
            db: 数据库会话
            provider_offset: 分页偏移
            provider_limit: 分页限制

        Returns:
            Provider 列表
        """
        provider_query = (
            db.query(Provider)
            .options(
                # 预加载 Provider 级别的 api_keys
                selectinload(Provider.api_keys),
                # 预加载 endpoints（用于按 api_format 选择请求配置）
                selectinload(Provider.endpoints),
                # 同时加载 models 和 global_model 关系
                selectinload(Provider.models).selectinload(Model.global_model),
            )
            .filter(Provider.is_active == True)
            .order_by(Provider.provider_priority.asc())
        )

        if provider_offset:
            provider_query = provider_query.offset(provider_offset)
        if provider_limit:
            provider_query = provider_query.limit(provider_limit)

        return provider_query.all()

    async def _check_model_support(
        self,
        db: Session,
        provider: Provider,
        model_name: str,
        is_stream: bool = False,
        capability_requirements: Optional[Dict[str, bool]] = None,
    ) -> Tuple[bool, Optional[str], Optional[List[str]]]:
        """
        检查 Provider 是否支持指定模型（可选检查流式支持和能力需求）

        模型能力检查在这里进行（而不是在 Key 级别），因为：
        - 模型支持的能力是全局的，与具体的 Key 无关
        - 如果模型不支持某能力，整个 Provider 的所有 Key 都应该被跳过

        支持两种匹配方式：
        1. 直接匹配 GlobalModel.name
        2. 通过 ModelCacheService 匹配映射名（全局查找）

        Args:
            db: 数据库会话
            provider: Provider 对象
            model_name: 模型名称（可以是 GlobalModel.name 或映射名）
            is_stream: 是否是流式请求，如果为 True 则同时检查流式支持
            capability_requirements: 能力需求（可选），用于检查模型是否支持所需能力

        Returns:
            (is_supported, skip_reason, supported_capabilities) - 是否支持、跳过原因、模型支持的能力列表
        """
        # 使用 ModelCacheService 解析模型名称（支持映射名）
        global_model = await ModelCacheService.resolve_global_model_by_name_or_alias(db, model_name)

        if not global_model:
            # 完全未找到匹配
            return False, "模型不存在或 Provider 未配置此模型", None

        # 找到 GlobalModel 后，检查当前 Provider 是否支持
        is_supported, skip_reason, caps = await self._check_model_support_for_global_model(
            db, provider, global_model, model_name, is_stream, capability_requirements
        )
        return is_supported, skip_reason, caps

    async def _check_model_support_for_global_model(
        self,
        db: Session,
        provider: Provider,
        global_model: "GlobalModel",
        model_name: str,
        is_stream: bool = False,
        capability_requirements: Optional[Dict[str, bool]] = None,
    ) -> Tuple[bool, Optional[str], Optional[List[str]]]:
        """
        检查 Provider 是否支持指定的 GlobalModel

        Args:
            db: 数据库会话
            provider: Provider 对象
            global_model: GlobalModel 对象
            model_name: 用户请求的模型名称（用于错误消息）
            is_stream: 是否是流式请求
            capability_requirements: 能力需求

        Returns:
            (is_supported, skip_reason, supported_capabilities)
        """
        # 确保 global_model 附加到当前 Session
        # 注意：从缓存重建的对象是 transient 状态，不能使用 load=False
        # 使用 load=True（默认）允许 SQLAlchemy 正确处理 transient 对象
        from sqlalchemy import inspect
        insp = inspect(global_model)
        if insp.transient or insp.detached:
            # transient/detached 对象：使用默认 merge（会查询 DB 检查是否存在）
            global_model = db.merge(global_model)
        else:
            # persistent 对象：已经附加到 session，无需 merge
            pass

        # 获取模型支持的能力列表
        model_supported_capabilities: List[str] = list(global_model.supported_capabilities or [])

        # 查询该 Provider 是否有实现这个 GlobalModel
        for model in provider.models:
            if model.global_model_id == global_model.id and model.is_active:
                logger.debug(
                    f"[_check_model_support_for_global_model] Provider={provider.name}, "
                    f"GlobalModel={global_model.name}, "
                    f"provider_model_name={model.provider_model_name}"
                )
                # 检查流式支持
                if is_stream:
                    supports_streaming = model.get_effective_supports_streaming()
                    if not supports_streaming:
                        return False, f"模型 {model_name} 在此 Provider 不支持流式", None

                # 检查模型是否支持所需的能力（在 Provider 级别检查，而不是 Key 级别）
                # 只有当 model_supported_capabilities 非空时才进行检查
                # 空列表意味着模型没有配置能力限制，默认支持所有能力
                if capability_requirements and model_supported_capabilities:
                    for cap_name, is_required in capability_requirements.items():
                        if is_required and cap_name not in model_supported_capabilities:
                            return (
                                False,
                                f"模型 {model_name} 不支持能力: {cap_name}",
                                list(model_supported_capabilities),
                            )

                return True, None, list(model_supported_capabilities)

        return False, "Provider 未实现此模型", None

    def _check_key_availability(
        self,
        key: ProviderAPIKey,
        api_format: Optional[str],
        model_name: str,
        capability_requirements: Optional[Dict[str, bool]] = None,
        resolved_model_name: Optional[str] = None,
        model_aliases: Optional[List[str]] = None,
    ) -> Tuple[bool, Optional[str], Optional[str]]:
        """
        检查 API Key 的可用性

        注意：模型能力检查已移到 _check_model_support 中进行（Provider 级别），
        这里只检查 Key 级别的能力匹配。

        Args:
            key: API Key 对象
            model_name: 模型名称
            capability_requirements: 能力需求（可选）
            resolved_model_name: 解析后的 GlobalModel.name（可选）
            model_aliases: GlobalModel 的别名列表（用于通配符匹配）

        Returns:
            (is_available, skip_reason, alias_matched_model)
            - is_available: Key 是否可用
            - skip_reason: 不可用时的原因
            - alias_matched_model: 通过别名匹配到的模型名（用于实际请求）
        """
        # 检查熔断器状态（使用详细状态方法获取更丰富的跳过原因，按 API 格式）
        is_available, circuit_reason = health_monitor.get_circuit_breaker_status(
            key, api_format=api_format
        )
        if not is_available:
            return False, circuit_reason or "熔断器已打开", None

        # 模型权限检查：使用 allowed_models 白名单（支持简单列表和按格式字典两种模式）
        # None = 允许所有模型，[] = 拒绝所有模型，["a","b"] = 只允许指定模型
        # 支持通配符别名匹配（通过 model_aliases）
        from src.core.model_permissions import (
            check_model_allowed_with_aliases,
            get_allowed_models_preview,
        )

        try:
            is_allowed, alias_matched_model = check_model_allowed_with_aliases(
                model_name=model_name,
                allowed_models=key.allowed_models,
                api_format=api_format,
                resolved_model_name=resolved_model_name,
                model_aliases=model_aliases,
            )
        except TimeoutError:
            # 正则匹配超时（可能是 ReDoS 攻击或复杂模式）
            logger.warning(f"别名匹配超时: key_id={key.id}, model={model_name}")
            return False, "别名匹配超时，请简化配置", None
        except re.error as e:
            # 正则语法错误（配置问题）
            logger.warning(f"别名规则无效: key_id={key.id}, model={model_name}, error={e}")
            return False, f"别名规则无效: {str(e)}", None
        except Exception as e:
            # 其他未知异常
            logger.error(f"别名匹配异常: key_id={key.id}, model={model_name}, error={e}", exc_info=True)
            # 异常时保守处理：不允许使用该 Key
            return False, "别名匹配失败", None

        if not is_allowed:
            return False, f"模型权限不匹配(允许: {get_allowed_models_preview(key.allowed_models)})", None

        # Key 级别的能力匹配检查
        # 注意：模型级别的能力检查已在 _check_model_support 中完成
        # 始终执行检查，即使 capability_requirements 为空
        # 因为 check_capability_match 会检查 Key 的 EXCLUSIVE 能力是否被浪费
        from src.core.key_capabilities import check_capability_match

        key_caps: Dict[str, bool] = dict(key.capabilities or {})
        is_match, skip_reason = check_capability_match(key_caps, capability_requirements)
        if not is_match:
            return False, skip_reason, None

        return True, None, alias_matched_model

    async def _build_candidates(
        self,
        db: Session,
        providers: List[Provider],
        target_format: APIFormat,
        model_name: str,
        affinity_key: Optional[str],
        resolved_model_name: Optional[str] = None,
        model_aliases: Optional[List[str]] = None,
        max_candidates: Optional[int] = None,
        is_stream: bool = False,
        capability_requirements: Optional[Dict[str, bool]] = None,
    ) -> List[ProviderCandidate]:
        """
        构建候选列表

        Key 直属 Provider，通过 api_formats 筛选符合目标格式的 Key。

        Args:
            db: 数据库会话
            providers: Provider 列表
            target_format: 目标 API 格式
            model_name: 模型名称（用户请求的名称，可能是映射名）
            affinity_key: 亲和性标识符（通常为API Key ID）
            resolved_model_name: 解析后的 GlobalModel.name（用于 Key.allowed_models 校验）
            model_aliases: GlobalModel 的别名列表（用于 Key.allowed_models 通配符匹配）
            max_candidates: 最大候选数
            is_stream: 是否是流式请求，如果为 True 则过滤不支持流式的 Provider
            capability_requirements: 能力需求（可选）

        Returns:
            候选列表
        """
        candidates: List[ProviderCandidate] = []
        target_format_str = target_format.value

        for provider in providers:
            # 检查模型支持（同时检查流式支持和模型能力需求）
            supports_model, skip_reason, _model_caps = await self._check_model_support(
                db, provider, model_name, is_stream, capability_requirements
            )
            if not supports_model:
                logger.debug(f"Provider {provider.name} 不支持模型 {model_name}: {skip_reason}")
                continue

            # 查找目标格式对应的 Endpoint（获取请求配置）
            target_endpoint = None
            for endpoint in provider.endpoints:
                endpoint_format_str = (
                    endpoint.api_format
                    if isinstance(endpoint.api_format, str)
                    else endpoint.api_format.value
                )
                if endpoint.is_active and endpoint_format_str == target_format_str:
                    target_endpoint = endpoint
                    break

            if not target_endpoint:
                logger.debug(f"Provider {provider.name} 没有活跃的 {target_format_str} 端点")
                continue

            # Key 直属 Provider，通过 api_formats 筛选
            active_keys = [
                key for key in provider.api_keys
                if key.is_active and target_format_str in (key.api_formats or [])
            ]

            if not active_keys:
                logger.debug(f"Provider {provider.name} 没有支持 {target_format_str} 的活跃 Key")
                continue

            # 检查是否所有 Key 都是 TTL=0（轮换模式）
            use_random = all(
                (key.cache_ttl_minutes or 0) == 0 for key in active_keys
            ) if active_keys else False
            if use_random and len(active_keys) > 1:
                logger.debug(
                    f"  Provider {provider.name} 启用 Key 轮换模式 (TTL=0, {len(active_keys)} keys)"
                )
            keys = self._shuffle_keys_by_internal_priority(active_keys, affinity_key, use_random)

            for key in keys:
                # Key 级别的能力检查
                is_available, skip_reason, alias_matched_model = self._check_key_availability(
                    key,
                    target_format_str,
                    model_name,
                    capability_requirements,
                    resolved_model_name=resolved_model_name,
                    model_aliases=model_aliases,
                )

                candidate = ProviderCandidate(
                    provider=provider,
                    endpoint=target_endpoint,
                    key=key,
                    is_skipped=not is_available,
                    skip_reason=skip_reason,
                    alias_matched_model=alias_matched_model,
                )
                candidates.append(candidate)

                if max_candidates and len(candidates) >= max_candidates:
                    return candidates

        return candidates

    async def _apply_cache_affinity(
        self,
        candidates: List[ProviderCandidate],
        affinity_key: str,
        api_format: APIFormat,
        global_model_id: str,
    ) -> List[ProviderCandidate]:
        """
        应用缓存亲和性排序

        缓存命中的候选会被提升到列表前面

        Args:
            candidates: 候选列表
            affinity_key: 亲和性标识符（通常为API Key ID）
            api_format: API 格式
            global_model_id: GlobalModel ID（规范化的模型标识）

        Returns:
            重排序后的候选列表
        """
        try:
            # 查询该亲和性标识符在当前 API 格式和模型下的缓存亲和性
            api_format_str = api_format.value if isinstance(api_format, APIFormat) else api_format
            affinity = await self._affinity_manager.get_affinity(
                affinity_key, api_format_str, global_model_id
            )

            if not affinity:
                # 没有缓存亲和性，所有候选都标记为非缓存
                for candidate in candidates:
                    candidate.is_cached = False
                return candidates

            # 按是否匹配缓存亲和性分类候选
            cached_candidates: List[ProviderCandidate] = []
            other_candidates: List[ProviderCandidate] = []
            matched = False

            for candidate in candidates:
                provider = candidate.provider
                endpoint = candidate.endpoint
                key = candidate.key

                if (
                    provider.id == affinity.provider_id
                    and endpoint.id == affinity.endpoint_id
                    and key.id == affinity.key_id
                ):
                    candidate.is_cached = True
                    cached_candidates.append(candidate)
                    matched = True
                    logger.debug(
                        f"检测到缓存亲和性: affinity_key={affinity_key[:8]}..., "
                        f"api_format={api_format_str}, global_model_id={global_model_id[:8]}..., "
                        f"provider={provider.name}, endpoint={endpoint.id[:8]}..., "
                        f"provider_key=***{key.api_key[-4:]}, "
                        f"使用次数={affinity.request_count}"
                    )
                else:
                    candidate.is_cached = False
                    other_candidates.append(candidate)

            if not matched:
                logger.debug(f"API格式 {api_format_str} 的缓存亲和性存在但组合不可用")

            # 重新排序：缓存候选优先
            if cached_candidates:
                result = cached_candidates + other_candidates
                logger.debug(f"{len(cached_candidates)} 个缓存组合已提升至优先级")
                return result

            return candidates

        except Exception as e:
            logger.warning(f"检查缓存亲和性失败: {e}，继续使用默认排序")
            return candidates

    def _normalize_priority_mode(self, mode: Optional[str]) -> str:
        normalized = (mode or "").strip().lower()
        if normalized not in self.ALLOWED_PRIORITY_MODES:
            if normalized:
                logger.warning(f"[CacheAwareScheduler] 无效的优先级模式 '{mode}'，回退为 provider")
            return self.PRIORITY_MODE_PROVIDER
        return normalized

    def set_priority_mode(self, mode: Optional[str]) -> None:
        """运行时更新候选排序策略"""
        normalized = self._normalize_priority_mode(mode)
        if normalized == self.priority_mode:
            return
        self.priority_mode = normalized
        logger.debug(f"[CacheAwareScheduler] 切换优先级模式为: {self.priority_mode}")

    def _normalize_scheduling_mode(self, mode: Optional[str]) -> str:
        normalized = (mode or "").strip().lower()
        if normalized not in self.ALLOWED_SCHEDULING_MODES:
            if normalized:
                logger.warning(f"[CacheAwareScheduler] 无效的调度模式 '{mode}'，回退为 cache_affinity")
            return self.SCHEDULING_MODE_CACHE_AFFINITY
        return normalized

    def set_scheduling_mode(self, mode: Optional[str]) -> None:
        """运行时更新调度模式"""
        normalized = self._normalize_scheduling_mode(mode)
        if normalized == self.scheduling_mode:
            return
        self.scheduling_mode = normalized
        logger.debug(f"[CacheAwareScheduler] 切换调度模式为: {self.scheduling_mode}")

    def _apply_priority_mode_sort(
        self, candidates: List[ProviderCandidate], affinity_key: Optional[str] = None
    ) -> List[ProviderCandidate]:
        """
        根据优先级模式对候选列表排序（数字越小越优先）

        - provider: 提供商优先模式，保持原有顺序（按 Provider.provider_priority -> Key.internal_priority 排序，已由查询保证）
                   Key.internal_priority 表示 Endpoint 内部优先级，同优先级内通过哈希分散负载均衡
        - global_key: 全局 Key 优先模式，按 Key.global_priority 升序排序（数字小的优先）
                     有 global_priority 的优先，NULL 的排后面
                     同 global_priority 内通过哈希分散实现负载均衡
        """
        if not candidates:
            return candidates

        if self.priority_mode == self.PRIORITY_MODE_GLOBAL_KEY:
            # 全局 Key 优先模式：按 global_priority 分组，同组内哈希分散负载均衡
            return self._sort_by_global_priority_with_hash(candidates, affinity_key)

        # 提供商优先模式：保持原有顺序（provider_priority 排序已经由查询保证）
        return candidates

    def _sort_by_global_priority_with_hash(
        self, candidates: List[ProviderCandidate], affinity_key: Optional[str] = None
    ) -> List[ProviderCandidate]:
        """
        按 global_priority 分组排序，同优先级内通过哈希分散实现负载均衡

        排序逻辑：
        1. 按 global_priority 分组（数字小的优先，NULL 排后面）
        2. 同 global_priority 组内，使用 affinity_key 哈希分散
        3. 确保同一用户请求稳定选择同一个 Key（缓存亲和性）
        """
        import hashlib
        from collections import defaultdict

        # 按 global_priority 分组
        priority_groups: Dict[int, List[ProviderCandidate]] = defaultdict(list)
        for candidate in candidates:
            global_priority = (
                candidate.key.global_priority
                if candidate.key and candidate.key.global_priority is not None
                else 999999  # NULL 排在后面
            )
            priority_groups[global_priority].append(candidate)

        result = []
        for priority in sorted(priority_groups.keys()):  # 数字小的优先级高
            group = priority_groups[priority]

            if len(group) > 1 and affinity_key:
                # 同优先级内哈希分散负载均衡
                scored_candidates = []
                for candidate in group:
                    key_id = candidate.key.id if candidate.key else ""
                    hash_input = f"{affinity_key}:{key_id}"
                    hash_value = int(hashlib.sha256(hash_input.encode()).hexdigest()[:16], 16)
                    scored_candidates.append((hash_value, candidate))

                # 按哈希值排序
                sorted_group = [c for _, c in sorted(scored_candidates)]
                result.extend(sorted_group)
            else:
                # 单个候选或没有 affinity_key，按次要排序条件排序
                def secondary_sort(c: ProviderCandidate):
                    return (
                        c.provider.provider_priority,
                        c.key.internal_priority if c.key else 999999,
                        c.key.id if c.key else "",
                    )

                result.extend(sorted(group, key=secondary_sort))

        return result

    def _apply_load_balance(
        self, candidates: List[ProviderCandidate]
    ) -> List[ProviderCandidate]:
        """
        负载均衡模式：同优先级内随机轮换

        排序逻辑：
        1. 按优先级分组（provider_priority, internal_priority 或 global_priority）
        2. 同优先级组内随机打乱
        3. 不考虑缓存亲和性
        """
        if not candidates:
            return candidates

        from collections import defaultdict

        priority_groups: Dict[tuple, List[ProviderCandidate]] = defaultdict(list)

        # 根据优先级模式选择分组方式
        if self.priority_mode == self.PRIORITY_MODE_GLOBAL_KEY:
            # 全局 Key 优先模式：按 global_priority 分组
            for candidate in candidates:
                global_priority = (
                    candidate.key.global_priority
                    if candidate.key and candidate.key.global_priority is not None
                    else 999999
                )
                priority_groups[(global_priority,)].append(candidate)
        else:
            # 提供商优先模式：按 (provider_priority, internal_priority) 分组
            for candidate in candidates:
                key = (
                    candidate.provider.provider_priority or 999999,
                    candidate.key.internal_priority if candidate.key else 999999,
                )
                priority_groups[key].append(candidate)

        result: List[ProviderCandidate] = []
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

    def _shuffle_keys_by_internal_priority(
        self,
        keys: List[ProviderAPIKey],
        affinity_key: Optional[str] = None,
        use_random: bool = False,
    ) -> List[ProviderAPIKey]:
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
        from collections import defaultdict

        priority_groups: Dict[int, List[ProviderAPIKey]] = defaultdict(list)

        for key in keys:
            priority = key.internal_priority if key.internal_priority is not None else 999999
            priority_groups[priority].append(key)

        # 对每个优先级组内的 Key 进行打乱
        result = []
        for priority in sorted(priority_groups.keys()):  # 数字小的优先级高，排前面
            group_keys = priority_groups[priority]

            if len(group_keys) > 1:
                if use_random:
                    # TTL=0 模式：使用随机排序实现 Key 轮换
                    shuffled = list(group_keys)
                    random.shuffle(shuffled)
                    result.extend(shuffled)
                elif affinity_key:
                    # 正常模式：使用哈希确定性打乱（保持缓存亲和性）
                    key_scores = []
                    for key in group_keys:
                        # 使用 affinity_key + key.id 的组合哈希
                        hash_input = f"{affinity_key}:{key.id}"
                        hash_value = int(hashlib.sha256(hash_input.encode()).hexdigest()[:16], 16)
                        key_scores.append((hash_value, key))

                    # 按哈希值排序
                    sorted_group = [key for _, key in sorted(key_scores)]
                    result.extend(sorted_group)
                else:
                    # 没有 affinity_key 时按 ID 排序保持稳定性
                    result.extend(sorted(group_keys, key=lambda k: k.id))
            else:
                # 单个 Key 直接添加
                result.extend(group_keys)

        return result

    async def invalidate_cache(
        self,
        affinity_key: str,
        api_format: str,
        global_model_id: str,
        endpoint_id: Optional[str] = None,
        key_id: Optional[str] = None,
        provider_id: Optional[str] = None,
    ):
        """
        失效指定亲和性标识符对特定API格式和模型的缓存亲和性

        Args:
            affinity_key: 亲和性标识符（通常为API Key ID）
            api_format: API格式 (claude/openai)
            global_model_id: GlobalModel ID（规范化的模型标识）
            endpoint_id: 端点ID（可选，如果提供则只在Endpoint匹配时失效）
            key_id: Provider Key ID（可选）
            provider_id: Provider ID（可选）
        """
        await self._ensure_initialized()
        await self._affinity_manager.invalidate_affinity(
            affinity_key=affinity_key,
            api_format=api_format,
            model_name=global_model_id,
            endpoint_id=endpoint_id,
            key_id=key_id,
            provider_id=provider_id,
        )

    async def set_cache_affinity(
        self,
        affinity_key: str,
        provider_id: str,
        endpoint_id: str,
        key_id: str,
        api_format: str,
        global_model_id: str,
        ttl: Optional[int] = None,
    ):
        """
        记录缓存亲和性（供编排器调用）

        Args:
            affinity_key: 亲和性标识符（通常为API Key ID）
            provider_id: Provider ID
            endpoint_id: Endpoint ID
            key_id: Provider Key ID
            api_format: API格式
            global_model_id: GlobalModel ID（规范化的模型标识）
            ttl: 缓存TTL（秒）

        注意：每次调用都会刷新过期时间，实现滑动窗口机制
        """
        await self._ensure_initialized()

        await self._affinity_manager.set_affinity(
            affinity_key=affinity_key,
            provider_id=provider_id,
            endpoint_id=endpoint_id,
            key_id=key_id,
            api_format=api_format,
            model_name=global_model_id,
            supports_caching=True,
            ttl=ttl,
        )

    async def get_stats(self) -> dict:
        """获取调度器统计信息"""
        await self._ensure_initialized()

        affinity_stats = self._affinity_manager.get_stats()
        metrics = dict(self._metrics)

        cache_total = metrics["cache_hits"] + metrics["cache_misses"]
        metrics["cache_hit_rate"] = metrics["cache_hits"] / cache_total if cache_total else 0.0
        metrics["avg_candidates_per_batch"] = (
            metrics["total_candidates"] / metrics["total_batches"]
            if metrics["total_batches"]
            else 0.0
        )

        # 动态预留统计
        reservation_stats = self._reservation_manager.get_stats()
        total_reservation_checks = (
            metrics["reservation_probe_count"] + metrics["reservation_stable_count"]
        )
        if total_reservation_checks > 0:
            probe_ratio = metrics["reservation_probe_count"] / total_reservation_checks
        else:
            probe_ratio = 0.0

        return {
            "scheduler": "cache_aware",
            "dynamic_reservation": {
                "enabled": True,
                "config": reservation_stats["config"],
                "current_avg_ratio": round(metrics["avg_reservation_ratio"], 3),
                "probe_phase_ratio": round(probe_ratio, 3),
                "total_checks": total_reservation_checks,
                "last_result": metrics["last_reservation_result"],
            },
            "affinity_stats": affinity_stats,
            "scheduler_metrics": metrics,
        }


# 全局单例
_scheduler: Optional[CacheAwareScheduler] = None


async def get_cache_aware_scheduler(
    redis_client=None,
    priority_mode: Optional[str] = None,
    scheduling_mode: Optional[str] = None,
) -> CacheAwareScheduler:
    """
    获取全局CacheAwareScheduler实例

    注意: 不再接受 db 参数,避免持久化请求级别的 Session
    每次调用 scheduler 方法时需要传入当前请求的 db Session

    Args:
        redis_client: Redis客户端（可选）
        priority_mode: 外部覆盖的优先级模式（provider | global_key）
        scheduling_mode: 外部覆盖的调度模式（fixed_order | cache_affinity）

    Returns:
        CacheAwareScheduler实例
    """
    global _scheduler

    if _scheduler is None:
        _scheduler = CacheAwareScheduler(
            redis_client, priority_mode=priority_mode, scheduling_mode=scheduling_mode
        )
    else:
        if priority_mode:
            _scheduler.set_priority_mode(priority_mode)
        if scheduling_mode:
            _scheduler.set_scheduling_mode(scheduling_mode)

    return _scheduler
