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

import time
from typing import TYPE_CHECKING, Any

if TYPE_CHECKING:
    from src.services.scheduling.protocols import (
        CacheAffinityManagerProtocol,
        CandidateBuilderProtocol,
        CandidateSorterProtocol,
        ConcurrencyCheckerProtocol,
    )

from sqlalchemy.orm import Session

from src.core.exceptions import ModelNotSupportedException, ProviderNotAvailableException
from src.core.logger import logger
from src.core.model_permissions import (
    check_model_allowed,
    get_allowed_models_preview,
)
from src.models.database import (
    ApiKey,
    Provider,
    ProviderAPIKey,
    ProviderEndpoint,
)
from src.services.cache.model_cache import ModelCacheService
from src.services.provider.format import normalize_endpoint_signature
from src.services.rate_limit.adaptive_reservation import (
    get_adaptive_reservation_manager,
)
from src.services.rate_limit.concurrency_manager import get_concurrency_manager
from src.services.scheduling.affinity_manager import (
    CacheAffinityManager,
    get_affinity_manager,
)
from src.services.scheduling.candidate_builder import (
    CandidateBuilder,
)
from src.services.scheduling.candidate_builder import (
    _sort_endpoints_by_family_priority as _sort_endpoints_by_family_priority,
)
from src.services.scheduling.candidate_sorter import CandidateSorter
from src.services.scheduling.concurrency_checker import ConcurrencyChecker
from src.services.scheduling.restriction_checker import get_effective_restrictions
from src.services.scheduling.scheduling_config import SchedulingConfig
from src.services.scheduling.schemas import ConcurrencySnapshot as ConcurrencySnapshot  # re-export
from src.services.scheduling.schemas import PoolCandidate as PoolCandidate  # re-export
from src.services.scheduling.schemas import ProviderCandidate as ProviderCandidate  # re-export
from src.services.scheduling.utils import affinity_hash as _affinity_hash  # re-export compat
from src.services.scheduling.utils import (
    release_db_connection_before_await,
)
from src.services.system.config import SystemConfigService


class CacheAwareScheduler:
    """
    缓存感知调度器 - 薄协调层

    编排以下子组件:
    - SchedulingConfig: 调度模式和优先级模式管理
    - CandidateBuilder: 候选构建（查询 Provider/Endpoint/Key）
    - CandidateSorter: 候选排序（优先级/负载均衡）
    - ConcurrencyChecker: 并发控制（RPM + 动态预留）
    - CacheAffinityManager: 缓存亲和性管理
    """

    # 类常量 re-export（保持外部访问兼容性）
    PRIORITY_MODE_PROVIDER = SchedulingConfig.PRIORITY_MODE_PROVIDER
    PRIORITY_MODE_GLOBAL_KEY = SchedulingConfig.PRIORITY_MODE_GLOBAL_KEY
    ALLOWED_PRIORITY_MODES = SchedulingConfig.ALLOWED_PRIORITY_MODES
    SCHEDULING_MODE_FIXED_ORDER = SchedulingConfig.SCHEDULING_MODE_FIXED_ORDER
    SCHEDULING_MODE_CACHE_AFFINITY = SchedulingConfig.SCHEDULING_MODE_CACHE_AFFINITY
    SCHEDULING_MODE_LOAD_BALANCE = SchedulingConfig.SCHEDULING_MODE_LOAD_BALANCE
    ALLOWED_SCHEDULING_MODES = SchedulingConfig.ALLOWED_SCHEDULING_MODES

    def __init__(
        self,
        redis_client: Any | None = None,
        priority_mode: str | None = None,
        scheduling_mode: str | None = None,
        *,
        candidate_builder: CandidateBuilderProtocol | None = None,
        candidate_sorter: CandidateSorterProtocol | None = None,
        concurrency_checker: ConcurrencyCheckerProtocol | None = None,
        affinity_manager: CacheAffinityManagerProtocol | None = None,
    ) -> None:
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
        self._config = SchedulingConfig(priority_mode, scheduling_mode)

        # 异步子组件（将在第一次使用时初始化，可通过构造函数注入）
        self._affinity_manager: CacheAffinityManagerProtocol | None = affinity_manager
        self._concurrency_checker: ConcurrencyCheckerProtocol | None = concurrency_checker

        self._metrics: dict[str, Any] = {
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

        # 初始化子模块（不传 self，解除反向引用，可通过构造函数注入）
        self._candidate_sorter: CandidateSorterProtocol = candidate_sorter or CandidateSorter(
            self._config
        )
        self._candidate_builder: CandidateBuilderProtocol = candidate_builder or CandidateBuilder(
            self._candidate_sorter
        )

    # ── 属性代理（保持外部访问兼容性）──────────────────────────

    @property
    def priority_mode(self) -> str:
        return self._config.priority_mode

    @priority_mode.setter
    def priority_mode(self, value: str) -> None:
        self._config.priority_mode = value

    @property
    def scheduling_mode(self) -> str:
        return self._config.scheduling_mode

    @scheduling_mode.setter
    def scheduling_mode(self, value: str) -> None:
        self._config.scheduling_mode = value

    def set_priority_mode(self, mode: str | None) -> None:
        """运行时更新候选排序策略"""
        self._config.set_priority_mode(mode)

    def set_scheduling_mode(self, mode: str | None) -> None:
        """运行时更新调度模式"""
        self._config.set_scheduling_mode(mode)

    # ── 静态方法兼容壳 ───────────────────────────────────────

    @staticmethod
    def _release_db_connection_before_await(db: Session) -> None:
        release_db_connection_before_await(db)

    @staticmethod
    def _affinity_hash(affinity_key: str, identifier: str) -> int:
        return _affinity_hash(affinity_key, identifier)

    # ── 异步初始化 ───────────────────────────────────────────

    async def _ensure_initialized(self) -> None:
        """确保所有异步组件已初始化"""
        if self._affinity_manager is None:
            self._affinity_manager = await get_affinity_manager(self.redis)

        if self._concurrency_checker is None:
            concurrency_manager = await get_concurrency_manager()
            reservation_manager = get_adaptive_reservation_manager()
            self._concurrency_checker = ConcurrencyChecker(
                concurrency_manager=concurrency_manager,
                reservation_manager=reservation_manager,
            )

    # ── 核心编排方法 ─────────────────────────────────────────

    async def select_with_cache_affinity(
        self,
        db: Session,
        affinity_key: str,
        api_format: str,
        model_name: str,
        excluded_endpoints: list[str] | None = None,
        excluded_keys: list[str] | None = None,
        provider_batch_size: int = 20,
        max_candidates_per_batch: int | None = None,
    ) -> tuple[Provider, ProviderEndpoint, ProviderAPIKey]:
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

        normalized_format = normalize_endpoint_signature(api_format)

        logger.debug(
            "[CacheAwareScheduler] select_with_cache_affinity: "
            "affinity_key={}..., api_format={}, model={}",
            affinity_key[:8],
            normalized_format,
            model_name,
        )

        self._metrics["last_api_format"] = normalized_format
        self._metrics["last_model_name"] = model_name

        provider_offset = 0

        global_model_id = None  # 用于缓存亲和性

        while True:
            candidates, resolved_global_model_id, provider_batch_count = (
                await self.list_all_candidates(
                    db=db,
                    api_format=normalized_format,
                    model_name=model_name,
                    affinity_key=affinity_key,
                    provider_offset=provider_offset,
                    provider_limit=provider_batch_size,
                    max_candidates=max_candidates_per_batch,
                )
            )

            if resolved_global_model_id and global_model_id is None:
                global_model_id = resolved_global_model_id

            if provider_batch_count == 0:
                if provider_offset == 0:
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
                    logger.debug("  └─ Endpoint {}... 在排除列表，跳过", endpoint.id[:8])
                    continue

                if key.id in excluded_keys_set:
                    logger.debug("  └─ Key {}... 在排除列表，跳过", key.id[:8])
                    continue

                is_cached_user = bool(candidate.is_cached)
                can_use, snapshot = await self._concurrency_checker.check_available(
                    key,
                    is_cached_user=is_cached_user,
                )

                # 更新预留指标
                self._update_reservation_metrics(snapshot)

                if not can_use:
                    logger.debug("  └─ Key {}... 并发已满 ({})", key.id[:8], snapshot.describe())
                    self._metrics["concurrency_denied"] += 1
                    continue

                logger.debug(
                    "  └─ 选择 Provider={}, Endpoint={}..., " "Key={}, 缓存命中={}, 并发状态[{}]",
                    provider.name,
                    endpoint.id[:8],
                    key.name,
                    is_cached_user,
                    snapshot.describe(),
                )

                if key.cache_ttl_minutes > 0 and global_model_id:
                    ttl = key.cache_ttl_minutes * 60
                    await self.set_cache_affinity(
                        affinity_key=affinity_key,
                        provider_id=str(provider.id),
                        endpoint_id=str(endpoint.id),
                        key_id=str(key.id),
                        api_format=normalized_format,
                        global_model_id=global_model_id,
                        ttl=ttl,
                    )

                if is_cached_user:
                    self._metrics["cache_hits"] += 1
                else:
                    self._metrics["cache_misses"] += 1

                return provider, endpoint, key

            provider_offset += provider_batch_size
            if provider_batch_count < provider_batch_size:
                break

        raise ProviderNotAvailableException("服务暂时繁忙，请稍后重试")

    async def list_all_candidates(
        self,
        db: Session,
        api_format: str,
        model_name: str,
        affinity_key: str | None = None,
        user_api_key: ApiKey | None = None,
        provider_offset: int = 0,
        provider_limit: int | None = None,
        max_candidates: int | None = None,
        is_stream: bool = False,
        capability_requirements: dict[str, bool] | None = None,
        request_body: dict | None = None,
    ) -> tuple[list[ProviderCandidate], str, int]:
        """
        预先获取所有可用的 Provider/Endpoint/Key 组合

        编排流程：
        1. 解析 GlobalModel
        2. 检查访问限制
        3. 查询 Providers（委托给 CandidateBuilder）
        4. 构建候选列表（委托给 CandidateBuilder）
        5. 应用排序和缓存亲和性

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
            (候选列表, global_model_id, provider_batch_count)
            - global_model_id 用于缓存亲和性
            - provider_batch_count 表示本次查询到的 Provider 数量（未应用 allowed_providers 过滤前）
        """
        # If the caller already touched the DB, release the connection before we do async work.
        release_db_connection_before_await(db)
        await self._ensure_initialized()

        target_format = normalize_endpoint_signature(api_format)

        logger.debug(
            "[Scheduler] list_all_candidates: model={}, api_format={}",
            model_name,
            target_format,
        )

        # 0. 解析 model_name 到 GlobalModel（仅接受 GlobalModel.name）
        normalized_name = model_name.strip() if isinstance(model_name, str) else ""
        if not normalized_name:
            logger.warning("GlobalModel not found: <empty model name>")
            raise ModelNotSupportedException(model=model_name)

        global_model = await ModelCacheService.get_global_model_by_name(db, normalized_name)
        if not global_model or not global_model.is_active:
            logger.warning("GlobalModel not found or inactive: {}", normalized_name)
            raise ModelNotSupportedException(model=model_name)

        logger.debug(
            "[Scheduler] GlobalModel resolved: id={}, name={}",
            global_model.id,
            global_model.name,
        )

        # 使用 GlobalModel.id 作为缓存亲和性的模型标识，确保映射名和规范名都能命中同一个缓存
        global_model_id: str = str(global_model.id)

        queried_provider_count = 0

        # 提取模型映射（用于 Provider Key 的 allowed_models 匹配）
        model_mappings: list[str] = (global_model.config or {}).get("model_mappings", [])
        if model_mappings:
            logger.debug(
                "[Scheduler] GlobalModel={} 配置了映射规则: {}",
                global_model.name,
                model_mappings,
            )

        # 获取合并后的访问限制（ApiKey + User）
        restrictions = get_effective_restrictions(user_api_key)
        allowed_api_formats = restrictions["allowed_api_formats"]
        allowed_providers = restrictions["allowed_providers"]
        allowed_models = restrictions["allowed_models"]

        # 0.1 检查 API 格式是否被允许
        if allowed_api_formats is not None:
            allowed_norm = {normalize_endpoint_signature(f) for f in allowed_api_formats if f}
            if target_format not in allowed_norm:
                logger.debug(
                    "API Key {}... 不允许使用 API 格式 {}, 允许的格式: {}",
                    user_api_key.id[:8] if user_api_key else "N/A",
                    target_format,
                    allowed_api_formats,
                )
                return [], global_model_id, queried_provider_count

        # 0.2 检查模型是否被允许
        if not check_model_allowed(
            model_name=model_name,
            allowed_models=allowed_models,
        ):
            logger.debug(
                "用户/API Key 不允许使用模型 {}, 允许的模型: {}",
                model_name,
                get_allowed_models_preview(allowed_models),
            )
            return [], global_model_id, queried_provider_count

        # 1. 查询 Providers（委托给 CandidateBuilder）
        providers = self._candidate_builder._query_providers(
            db=db,
            provider_offset=provider_offset,
            provider_limit=provider_limit,
        )
        queried_provider_count = len(providers)

        # Provider query starts a transaction; release connection before entering async candidate build.
        release_db_connection_before_await(db)

        logger.debug(
            "[Scheduler] Found {} active providers: {}",
            len(providers),
            ", ".join(p.name for p in providers),
        )

        if not providers:
            return [], global_model_id, queried_provider_count

        # 1.5 根据 allowed_providers 过滤（合并 ApiKey 和 User 的限制）
        if allowed_providers is not None:
            original_count = len(providers)
            # 同时支持 provider id 和 name 匹配
            providers = [
                p for p in providers if p.id in allowed_providers or p.name in allowed_providers
            ]
            if original_count != len(providers):
                logger.debug("用户/API Key 过滤 Provider: {} -> {}", original_count, len(providers))

        if not providers:
            return [], global_model_id, queried_provider_count

        # 2. 构建候选列表（委托给 CandidateBuilder）

        # 格式转换总开关（数据库配置）：关闭时禁止任何跨格式候选进入队列
        global_conversion_enabled = SystemConfigService.is_format_conversion_enabled(db)

        candidates = await self._candidate_builder._build_candidates(
            db=db,
            providers=providers,
            client_format=target_format,
            model_name=model_name,
            model_mappings=model_mappings,
            affinity_key=affinity_key,
            max_candidates=max_candidates,
            is_stream=is_stream,
            capability_requirements=capability_requirements,
            global_conversion_enabled=global_conversion_enabled,
            request_body=request_body,
        )

        # 3. 应用优先级模式排序 + 调度模式排序
        candidates = await self.reorder_candidates(
            candidates=candidates,
            db=db,
            affinity_key=affinity_key,
            api_format=target_format,
            global_model_id=global_model_id,
        )

        # 更新指标
        self._metrics["total_candidates"] += len(candidates)
        self._metrics["last_candidate_count"] = len(candidates)

        logger.debug(
            "预先获取到 {} 个可用组合 (api_format={}, model={})",
            len(candidates),
            target_format,
            model_name,
        )

        return candidates, global_model_id, queried_provider_count

    async def reorder_candidates(
        self,
        candidates: list[ProviderCandidate],
        db: Session,
        affinity_key: str | None = None,
        api_format: str | None = None,
        global_model_id: str | None = None,
    ) -> list[ProviderCandidate]:
        """对候选列表应用优先级模式排序和调度模式排序。

        在分页汇总后调用此方法可修正跨页排序失真。

        Args:
            candidates: 候选列表
            db: 数据库会话
            affinity_key: 亲和性标识符
            api_format: API 格式
            global_model_id: GlobalModel ID（缓存亲和模式需要）

        Returns:
            重排序后的候选列表
        """
        if not candidates:
            return candidates

        # 1. 优先级模式排序（委托给 CandidateSorter）
        candidates = self._candidate_sorter._apply_priority_mode_sort(
            candidates, db, affinity_key, api_format
        )

        # 排序完成后释放 DB 连接，避免后续 Redis 操作期间占用连接
        release_db_connection_before_await(db)

        # 2. 调度模式排序
        if self.scheduling_mode == self.SCHEDULING_MODE_CACHE_AFFINITY:
            if affinity_key and candidates and global_model_id:
                candidates = await self._apply_cache_affinity(
                    candidates=candidates,
                    db=db,
                    affinity_key=affinity_key,
                    api_format=api_format or "",
                    global_model_id=global_model_id,
                )
        elif self.scheduling_mode == self.SCHEDULING_MODE_LOAD_BALANCE:
            candidates = self._candidate_sorter._apply_load_balance(candidates, api_format)
            for candidate in candidates:
                candidate.is_cached = False
        else:
            for candidate in candidates:
                candidate.is_cached = False

        return candidates

    async def _apply_cache_affinity(
        self,
        candidates: list[ProviderCandidate],
        db: Session,
        affinity_key: str,
        api_format: str,
        global_model_id: str,
    ) -> list[ProviderCandidate]:
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
            api_format_str = str(api_format)
            affinity = await self._affinity_manager.get_affinity(
                affinity_key, api_format_str, global_model_id
            )

            if not affinity:
                # 没有缓存亲和性，所有候选都标记为非缓存
                for candidate in candidates:
                    candidate.is_cached = False
                return candidates

            # 判断候选是否应该被降级（用于分组）
            global_keep_priority = SystemConfigService.is_keep_priority_on_conversion(db)

            def should_demote(c: ProviderCandidate) -> bool:
                """判断候选是否应该被降级"""
                if global_keep_priority:
                    return False  # 全局开启时，所有候选都不降级
                if not c.needs_conversion:
                    return False  # exact 候选不降级
                if getattr(c.provider, "keep_priority_on_conversion", False):
                    return False  # 提供商配置了保持优先级
                return True  # 需要降级

            # 按是否匹配缓存亲和性分类候选，同时记录是否降级
            matched_candidate: ProviderCandidate | None = None
            matched = False

            for candidate in candidates:
                provider = candidate.provider
                endpoint = candidate.endpoint
                key = candidate.key

                is_pool_candidate = isinstance(candidate, PoolCandidate)
                pool_matched = (
                    is_pool_candidate
                    and provider.id == affinity.provider_id
                    and endpoint.id == affinity.endpoint_id
                )
                key_matched = (
                    (not is_pool_candidate)
                    and provider.id == affinity.provider_id
                    and endpoint.id == affinity.endpoint_id
                    and key.id == affinity.key_id
                )

                if pool_matched or key_matched:
                    candidate.is_cached = True
                    matched_candidate = candidate
                    matched = True
                    logger.debug(
                        "检测到缓存亲和性: affinity_key={}..., "
                        "api_format={}, global_model_id={}..., "
                        "provider={}, endpoint={}..., "
                        "provider_key={}, 使用次数={}",
                        affinity_key[:8],
                        api_format_str,
                        global_model_id[:8],
                        provider.name,
                        endpoint.id[:8],
                        key.name,
                        affinity.request_count,
                    )
                else:
                    candidate.is_cached = False

            if not matched:
                logger.debug("API格式 {} 的缓存亲和性存在但组合不可用", api_format_str)
                return candidates

            # 缓存亲和性命中且该候选可用（未被跳过）时，无条件优先使用
            # 理由：1) 它之前成功过；2) 它有 prompt cache 优势
            # 只有当缓存亲和性的候选被跳过（健康度太低/熔断）时，才按 exact 优先排序
            assert matched_candidate is not None  # guaranteed by matched=True

            if not matched_candidate.is_skipped:
                # 缓存命中且健康，无条件提升到最前面
                other_candidates = [c for c in candidates if c is not matched_candidate]
                result = [matched_candidate] + other_candidates
                logger.debug(
                    "缓存亲和性命中且健康，无条件优先使用 (needs_conversion={})",
                    matched_candidate.needs_conversion,
                )
                return result

            # 缓存命中但被跳过（不健康），按 exact 优先排序
            # 缓存候选在其所属类别内提升到最前面
            logger.debug(
                "缓存亲和性命中但不健康 (skip_reason={})，按 exact 优先排序",
                matched_candidate.skip_reason,
            )
            matched_should_demote = should_demote(matched_candidate)

            # 分组：非降级类 和 降级类
            keep_priority_candidates: list[ProviderCandidate] = []
            demote_candidates: list[ProviderCandidate] = []

            for c in candidates:
                if c is matched_candidate:
                    continue  # 先跳过缓存命中的候选
                if should_demote(c):
                    demote_candidates.append(c)
                else:
                    keep_priority_candidates.append(c)

            # 将缓存命中的候选插入到其所属类别的最前面
            if matched_should_demote:
                # 缓存命中的是降级类，插入到降级类最前面
                demote_candidates.insert(0, matched_candidate)
            else:
                # 缓存命中的是非降级类，插入到非降级类最前面
                keep_priority_candidates.insert(0, matched_candidate)

            result = keep_priority_candidates + demote_candidates
            logger.debug("缓存组合已提升至其类别内优先级 (demote={})", matched_should_demote)
            return result

        except Exception as e:
            logger.warning("检查缓存亲和性失败: {}，继续使用默认排序", e)
            return candidates

    # ── 委托方法（外部 API 兼容）──────────────────────────────

    async def invalidate_cache(
        self,
        affinity_key: str,
        api_format: str,
        global_model_id: str,
        endpoint_id: str | None = None,
        key_id: str | None = None,
        provider_id: str | None = None,
    ) -> Any:
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
        ttl: int | None = None,
    ) -> Any:
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

    # ── 指标 ────────────────────────────────────────────────

    def _update_reservation_metrics(self, snapshot: ConcurrencySnapshot) -> None:
        """根据并发检查结果更新预留相关指标"""
        if snapshot.reservation_phase == "probe":
            self._metrics["reservation_probe_count"] += 1
        elif snapshot.reservation_phase != "unknown":
            self._metrics["reservation_stable_count"] += 1

        # 计算移动平均预留比例
        total_reservations = (
            self._metrics["reservation_probe_count"] + self._metrics["reservation_stable_count"]
        )
        if total_reservations > 0:
            alpha = 0.1
            self._metrics["avg_reservation_ratio"] = (
                alpha * snapshot.reservation_ratio
                + (1 - alpha) * self._metrics["avg_reservation_ratio"]
            )

        self._metrics["last_reservation_result"] = {
            "ratio": snapshot.reservation_ratio,
            "phase": snapshot.reservation_phase,
            "confidence": snapshot.reservation_confidence,
            "load_factor": snapshot.load_factor,
        }

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
        reservation_stats = self._concurrency_checker.get_reservation_stats()
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
_scheduler: CacheAwareScheduler | None = None


async def get_cache_aware_scheduler(
    redis_client: Any | None = None,
    priority_mode: str | None = None,
    scheduling_mode: str | None = None,
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
