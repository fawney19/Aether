"""
候选构建器 (CandidateBuilder)

从 CacheAwareScheduler 拆分出的候选构建逻辑，负责:
- 查询活跃 Provider
- 检查模型支持
- 检查 Key 可用性
- 构建候选列表
"""

from __future__ import annotations

import re
from collections.abc import Sequence
from typing import TYPE_CHECKING

from sqlalchemy.orm import Session, selectinload

from src.core.api_format.conversion.compatibility import is_format_compatible
from src.core.api_format.enums import EndpointKind
from src.core.api_format.signature import make_signature_key, parse_signature_key
from src.core.key_capabilities import (
    CapabilityMatchMode,
    check_capability_match,
    compute_capability_score,
    get_capability,
)
from src.core.logger import logger
from src.core.model_permissions import check_model_allowed_with_mappings
from src.models.database import (
    Model,
    Provider,
    ProviderAPIKey,
    ProviderEndpoint,
)
from src.services.health.monitor import health_monitor
from src.services.provider.format import normalize_endpoint_signature
from src.services.provider.pool.account_state import (
    resolve_pool_account_state as _resolve_pool_account_state,
)
from src.services.scheduling.quota_skipper import is_key_quota_exhausted
from src.services.scheduling.utils import release_db_connection_before_await

if TYPE_CHECKING:
    from src.models.database import GlobalModel
    from src.services.provider.pool.config import PoolConfig
    from src.services.scheduling.protocols import CandidateSorterProtocol
    from src.services.scheduling.schemas import ProviderCandidate

from src.services.cache.model_cache import ModelCacheService


def _get_pool_config(provider: Provider) -> "PoolConfig | None":
    """Return parsed PoolConfig if the provider has pool enabled, else None."""
    from src.services.provider.pool.config import PoolConfig, parse_pool_config

    return parse_pool_config(getattr(provider, "config", None))


def _sort_endpoints_by_family_priority(
    eps: Sequence[ProviderEndpoint],
) -> list[ProviderEndpoint]:
    """按 ApiFamily 优先级对端点排序（同分组内使用）。"""
    from src.core.api_format.enums import ApiFamily

    def sort_key(ep: ProviderEndpoint) -> int:
        family_str = str(getattr(ep, "api_family", "") or "").strip().lower()
        try:
            return ApiFamily(family_str).priority
        except ValueError:
            return 99

    return sorted(eps, key=sort_key)


class CandidateBuilder:
    """候选构建器，负责查询 Provider、检查模型支持和 Key 可用性、构建候选列表。"""

    def __init__(self, candidate_sorter: CandidateSorterProtocol) -> None:
        self._sorter = candidate_sorter

    def _query_providers(
        self,
        db: Session,
        provider_offset: int = 0,
        provider_limit: int | None = None,
    ) -> list[Provider]:
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
                # defer 排除调度热路径不需要的大 JSON 字段，减少号池场景内存占用
                # - 凭证类: api_key/auth_config 在执行阶段由 get_provider_auth() 按需加载
                # - adjustment_history/utilization_samples: 仅 AdaptiveReservationManager
                #   在并发检查时读取单个 key，号池/非号池均可 lazy load
                # - upstream_metadata: 号池模式在 _build_candidates 中预计算账号封禁
                #   状态并挂到 key._pool_account_state，排序阶段不再需要原始 JSON；
                #   非号池模式 key 少，lazy load 可忽略
                selectinload(Provider.api_keys)
                .defer(ProviderAPIKey.api_key)
                .defer(ProviderAPIKey.auth_config)
                .defer(ProviderAPIKey.note)
                .defer(ProviderAPIKey.last_error_msg)
                .defer(ProviderAPIKey.auto_fetch_models)
                .defer(ProviderAPIKey.locked_models)
                .defer(ProviderAPIKey.model_include_patterns)
                .defer(ProviderAPIKey.model_exclude_patterns)
                .defer(ProviderAPIKey.last_models_fetch_at)
                .defer(ProviderAPIKey.last_models_fetch_error)
                .defer(ProviderAPIKey.max_probe_interval_minutes)
                .defer(ProviderAPIKey.expires_at)
                .defer(ProviderAPIKey.adjustment_history)
                .defer(ProviderAPIKey.utilization_samples)
                .defer(ProviderAPIKey.upstream_metadata),
                # 预加载 endpoints（用于按 api_format 选择请求配置）
                selectinload(Provider.endpoints),
                # 同时加载 models 和 global_model 关系
                selectinload(Provider.models).selectinload(Model.global_model),
            )
            .filter(Provider.is_active.is_(True))
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
        api_format: str | None = None,
        is_stream: bool = False,
        capability_requirements: dict[str, bool] | None = None,
    ) -> tuple[bool, str | None, list[str] | None, set[str] | None]:
        """
        检查 Provider 是否支持指定模型（可选检查流式支持和能力需求）

        模型能力检查在这里进行（而不是在 Key 级别），因为：
        - 模型支持的能力是全局的，与具体的 Key 无关
        - 如果模型不支持某能力，整个 Provider 的所有 Key 都应该被跳过

        仅支持直接匹配 GlobalModel.name（外部请求不接受映射名）

        Args:
            db: 数据库会话
            provider: Provider 对象
            model_name: 模型名称（必须是 GlobalModel.name）
            is_stream: 是否是流式请求，如果为 True 则同时检查流式支持
            capability_requirements: 能力需求（可选），用于检查模型是否支持所需能力

        Returns:
            (is_supported, skip_reason, supported_capabilities, provider_model_names)
            - is_supported: 是否支持
            - skip_reason: 跳过原因
            - supported_capabilities: 模型支持的能力列表
            - provider_model_names: Provider 侧可用的模型名称集合（主名称 + 映射名称，按 api_format 过滤）
        """
        # Avoid holding a DB connection while awaiting cache/Redis inside ModelCacheService.
        release_db_connection_before_await(db)

        # 仅接受 GlobalModel.name（不允许映射名）
        normalized_name = model_name.strip() if isinstance(model_name, str) else ""
        if not normalized_name:
            return False, "模型不存在或名称无效", None, None

        global_model = await ModelCacheService.get_global_model_by_name(db, normalized_name)
        if not global_model or not global_model.is_active:
            return False, "模型不存在或已停用", None, None

        # 找到 GlobalModel 后，检查当前 Provider 是否支持
        is_supported, skip_reason, caps, provider_model_names = (
            await self._check_model_support_for_global_model(
                db,
                provider,
                global_model,
                model_name,
                api_format,
                is_stream,
                capability_requirements,
            )
        )
        return is_supported, skip_reason, caps, provider_model_names

    async def _check_model_support_for_global_model(
        self,
        db: Session,
        provider: Provider,
        global_model: GlobalModel,
        model_name: str,
        api_format: str | None = None,
        is_stream: bool = False,
        capability_requirements: dict[str, bool] | None = None,
    ) -> tuple[bool, str | None, list[str] | None, set[str] | None]:
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
            (is_supported, skip_reason, supported_capabilities, provider_model_names)
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
        model_supported_capabilities: list[str] = list(global_model.supported_capabilities or [])

        # 查询该 Provider 是否有实现这个 GlobalModel
        for model in provider.models:
            if model.global_model_id == global_model.id and model.is_active:
                # 检查流式支持
                if is_stream:
                    supports_streaming = model.get_effective_supports_streaming()
                    if not supports_streaming:
                        return False, f"模型 {model_name} 在此 Provider 不支持流式", None, None

                # 检查模型是否支持所需的能力（在 Provider 级别检查，而不是 Key 级别）
                # 只有当 model_supported_capabilities 非空时才进行检查
                # 空列表意味着模型没有配置能力限制，默认支持所有能力
                # COMPATIBLE 能力跳过模型级硬过滤（交由排序阶段处理）
                if capability_requirements and model_supported_capabilities:
                    for cap_name, is_required in capability_requirements.items():
                        if is_required and cap_name not in model_supported_capabilities:
                            cap_def = get_capability(cap_name)
                            if cap_def and cap_def.match_mode == CapabilityMatchMode.COMPATIBLE:
                                continue
                            return (
                                False,
                                f"模型 {model_name} 不支持能力: {cap_name}",
                                list(model_supported_capabilities),
                                None,
                            )

                provider_model_names: set[str] = {model.provider_model_name}
                raw_mappings = model.provider_model_mappings
                if isinstance(raw_mappings, list):
                    for raw in raw_mappings:
                        if not isinstance(raw, dict):
                            continue
                        name = raw.get("name")
                        if not isinstance(name, str) or not name.strip():
                            continue

                        mapping_api_formats = raw.get("api_formats")
                        if api_format and mapping_api_formats:
                            # 新模式：endpoint signature（family:kind），按小写 canonical 比较
                            if isinstance(mapping_api_formats, list):
                                target = str(api_format).strip().lower()
                                allowed = {
                                    str(fmt).strip().lower() for fmt in mapping_api_formats if fmt
                                }
                                if target not in allowed:
                                    continue

                        provider_model_names.add(name.strip())

                return True, None, list(model_supported_capabilities), provider_model_names

        return False, "Provider 未实现此模型", None, None

    def _check_key_availability(
        self,
        key: ProviderAPIKey,
        api_format: str | None,
        model_name: str,
        capability_requirements: dict[str, bool] | None = None,
        model_mappings: list[str] | None = None,
        candidate_models: set[str] | None = None,
        *,
        provider_type: str | None = None,
    ) -> tuple[bool, str | None, str | None]:
        """
        检查 API Key 的可用性

        注意：模型能力检查已移到 _check_model_support 中进行（Provider 级别），
        这里只检查 Key 级别的能力匹配。

        Args:
            key: API Key 对象
            model_name: 模型名称（GlobalModel.name）
            capability_requirements: 能力需求（可选）
            model_mappings: GlobalModel 的映射列表（用于通配符匹配）
            candidate_models: Provider 侧可用的模型名称集合（用于限制映射匹配范围）

        Returns:
            (is_available, skip_reason, mapping_matched_model)
            - is_available: Key 是否可用
            - skip_reason: 不可用时的原因
            - mapping_matched_model: 通过映射匹配到的模型名（用于实际请求）
        """
        # 检查熔断器状态（使用详细状态方法获取更丰富的跳过原因，按 API 格式）
        is_available, circuit_reason = health_monitor.get_circuit_breaker_status(
            key, api_format=api_format
        )
        if not is_available:
            return False, circuit_reason or "熔断器已打开", None

        # 模型权限检查：使用 allowed_models 白名单
        # None = 允许所有模型，[] = 拒绝所有模型，["a","b"] = 只允许指定模型
        # 支持通配符映射匹配（通过 model_mappings）
        try:
            is_allowed, mapping_matched_model = check_model_allowed_with_mappings(
                model_name=model_name,
                allowed_models=key.allowed_models,
                model_mappings=model_mappings,
                candidate_models=candidate_models,
            )
            if mapping_matched_model:
                logger.debug(
                    "[Scheduler] Key {}... 模型名匹配: model={} -> {}, allowed_models={}",
                    key.id[:8],
                    model_name,
                    mapping_matched_model,
                    key.allowed_models,
                )
        except TimeoutError:
            # 正则匹配超时（可能是 ReDoS 攻击或复杂模式）
            logger.warning("映射匹配超时: key_id={}, model={}", key.id, model_name)
            return False, "映射匹配超时，请简化配置", None
        except re.error as e:
            # 正则语法错误（配置问题）
            logger.warning("映射规则无效: key_id={}, model={}, error={}", key.id, model_name, e)
            return False, f"映射规则无效: {str(e)}", None
        except Exception as e:
            # 其他未知异常
            logger.error(
                "映射匹配异常: key_id={}, model={}, error={}", key.id, model_name, e, exc_info=True
            )
            # 异常时保守处理：不允许使用该 Key
            return False, "映射匹配失败", None

        if not is_allowed:
            return (
                False,
                f"Key 不支持 {model_name}",
                None,
            )

        # Key 级别的能力匹配检查
        # 注意：模型级别的能力检查已在 _check_model_support 中完成
        # 始终执行检查，即使 capability_requirements 为空
        # 因为 check_capability_match 会检查 Key 的 EXCLUSIVE 能力是否被浪费
        key_caps: dict[str, bool] = dict(key.capabilities or {})
        is_match, skip_reason = check_capability_match(key_caps, capability_requirements)
        if not is_match:
            return False, skip_reason, None

        effective_model_name = mapping_matched_model or model_name

        quota_exhausted, quota_reason = is_key_quota_exhausted(
            provider_type,
            key,
            model_name=effective_model_name,
        )
        if quota_exhausted:
            return False, quota_reason, mapping_matched_model

        return True, None, mapping_matched_model

    async def _build_candidates(
        self,
        db: Session,
        providers: list[Provider],
        client_format: str,
        model_name: str,
        affinity_key: str | None,
        model_mappings: list[str] | None = None,
        max_candidates: int | None = None,
        is_stream: bool = False,
        capability_requirements: dict[str, bool] | None = None,
        global_conversion_enabled: bool = True,
        request_body: dict | None = None,
    ) -> "list[ProviderCandidate]":
        """
        构建候选列表

        Key 直属 Provider，通过 api_formats 筛选符合端点格式的 Key。

        Args:
            db: 数据库会话
            providers: Provider 列表
            client_format: 客户端请求的 API 格式
            model_name: 模型名称（GlobalModel.name）
            affinity_key: 亲和性标识符（通常为API Key ID）
            model_mappings: GlobalModel 的映射列表（用于 Key.allowed_models 通配符匹配）
            max_candidates: 最大候选数
            is_stream: 是否是流式请求，如果为 True 则过滤不支持流式的 Provider
            capability_requirements: 能力需求（可选）
            global_conversion_enabled: 格式转换全局开关（数据库配置），关闭时回退到 Provider/Endpoint 精细化配置

        Returns:
            候选列表
        """
        from src.services.scheduling.schemas import PoolCandidate, ProviderCandidate

        candidates: list[ProviderCandidate] = []
        client_format_str = normalize_endpoint_signature(client_format)
        client_sig = parse_signature_key(client_format_str)
        client_family, client_kind = client_sig.api_family, client_sig.endpoint_kind

        # 提取 GlobalModel 配置的 output_limit（用于跨格式转换时的 max_tokens 默认值）
        output_limit: int | None = None
        normalized_name = model_name.strip() if isinstance(model_name, str) else ""
        if normalized_name:
            gm = await ModelCacheService.get_global_model_by_name(db, normalized_name)
            if gm and isinstance(gm.config, dict):
                raw = gm.config.get("output_limit")
                if isinstance(raw, int) and raw > 0:
                    output_limit = raw

        # chat/cli 互相可回退（用于同协议族下的端点变体），compact 可回退到 cli。
        # video/image 等不跨类回退。
        if client_kind in {EndpointKind.CHAT, EndpointKind.CLI}:
            allowed_kinds = {EndpointKind.CHAT, EndpointKind.CLI}
        elif client_kind == EndpointKind.COMPACT:
            allowed_kinds = {EndpointKind.COMPACT, EndpointKind.CLI}
        else:
            allowed_kinds = {client_kind}

        for provider in providers:
            # 按端点格式分别判断兼容性与模型/Key 可用性：
            # - 同格式端点优先（needs_conversion=False）
            # - 跨格式端点次之（needs_conversion=True）
            model_support_cache: dict[
                str, tuple[bool, str | None, list[str] | None, set[str] | None]
            ] = {}
            exact_candidates: list[ProviderCandidate] = []
            convertible_candidates: list[ProviderCandidate] = []
            pool_cfg = _get_pool_config(provider)

            # 使用新架构字段 (api_family, endpoint_kind) 进行预过滤与排序：
            # - family/kind 匹配的 endpoint 排在前面（但不做硬过滤，避免破坏格式转换路径）
            # - chat/cli 请求允许互相回退（优先同 kind）
            # - video 等请求只允许同 kind
            endpoints = list(provider.endpoints or [])
            allowed_kind_values = {k.value for k in allowed_kinds}
            preferred: list[ProviderEndpoint] = []
            preferred_other_family: list[ProviderEndpoint] = []
            fallback: list[ProviderEndpoint] = []
            fallback_other_family: list[ProviderEndpoint] = []

            for ep in endpoints:
                if not getattr(ep, "is_active", False):
                    continue

                raw_family = getattr(ep, "api_family", None)
                raw_kind = getattr(ep, "endpoint_kind", None)
                if not isinstance(raw_family, str) or not raw_family.strip():
                    continue
                if not isinstance(raw_kind, str) or not raw_kind.strip():
                    continue

                ep_family = raw_family.strip().lower()
                ep_kind = raw_kind.strip().lower()

                if allowed_kind_values and ep_kind not in allowed_kind_values:
                    continue

                same_family = ep_family == client_family.value
                same_kind = ep_kind == client_kind.value
                if same_kind and same_family:
                    preferred.append(ep)
                elif same_kind:
                    preferred_other_family.append(ep)
                elif same_family:
                    fallback.append(ep)
                else:
                    fallback_other_family.append(ep)

            endpoints = (
                _sort_endpoints_by_family_priority(preferred)
                + _sort_endpoints_by_family_priority(preferred_other_family)
                + _sort_endpoints_by_family_priority(fallback)
                + _sort_endpoints_by_family_priority(fallback_other_family)
            )

            for endpoint in endpoints:
                if not endpoint.is_active:
                    continue

                endpoint_format_str = make_signature_key(
                    str(getattr(endpoint, "api_family", "")).strip().lower(),
                    str(getattr(endpoint, "endpoint_kind", "")).strip().lower(),
                )

                # 格式转换开关（从高到低）：
                # 1) 全局开关 enable_format_conversion=ON -> 允许跨格式（跳过端点检查）
                # 2) 全局开关 OFF -> Provider.enable_format_conversion=ON -> 允许跨格式（跳过端点检查）
                # 3) 否则 -> 需 Endpoint.format_acceptance_config 显式允许
                provider_conversion_enabled = bool(
                    getattr(provider, "enable_format_conversion", False)
                )
                skip_endpoint_check = global_conversion_enabled or provider_conversion_enabled

                is_compatible, needs_conversion, _compat_reason = is_format_compatible(
                    client_format_str,
                    endpoint_format_str,
                    getattr(endpoint, "format_acceptance_config", None),
                    is_stream,
                    global_conversion_enabled,
                    skip_endpoint_check=skip_endpoint_check,
                )
                if not is_compatible:
                    continue

                # 检查模型支持（按端点格式过滤 provider_model_mappings）
                if endpoint_format_str not in model_support_cache:
                    model_support_cache[endpoint_format_str] = await self._check_model_support(
                        db,
                        provider,
                        model_name,
                        api_format=endpoint_format_str,
                        is_stream=is_stream,
                        capability_requirements=capability_requirements,
                    )
                supports_model, skip_reason, _model_caps, provider_model_names = (
                    model_support_cache[endpoint_format_str]
                )
                if not supports_model:
                    continue

                # Key 直属 Provider，通过 api_formats 按端点格式筛选
                # api_formats=None 视为"全支持"（兼容历史数据）
                active_keys = [
                    key
                    for key in provider.api_keys
                    if key.is_active
                    and (key.api_formats is None or endpoint_format_str in key.api_formats)
                ]
                if not active_keys:
                    continue

                use_random = all((key.cache_ttl_minutes or 0) == 0 for key in active_keys)
                if pool_cfg is not None:
                    use_random = False
                elif use_random and len(active_keys) > 1:
                    logger.debug(
                        "  Provider {} 启用 Key 轮换模式 (endpoint_format={}, {} keys)",
                        provider.name,
                        endpoint_format_str,
                        len(active_keys),
                    )
                keys_to_check = self._sorter.shuffle_keys_by_internal_priority(
                    active_keys, affinity_key, use_random
                )

                if pool_cfg is not None:
                    # 号池优化：跳过逐 key 的 _check_key_availability 检查，
                    # 直接收集全部 active key，将检查推迟到 PoolManager 排序后分页执行。

                    pool_keys = list(keys_to_check)

                    if not pool_keys:
                        continue

                    # 在释放 DB 连接前预计算账号封禁状态并挂到 Key 对象上。
                    # upstream_metadata 是 deferred 字段，逐条 lazy load 会产生 N+1 查询；
                    # 这里集中触发后，PoolManager 排序时直接读取 _pool_account_state 即可。
                    provider_type_str = (
                        str(getattr(provider, "provider_type", "") or "").strip().lower() or None
                    )
                    for pk in pool_keys:
                        setattr(
                            pk,
                            "_pool_account_state",
                            _resolve_pool_account_state(
                                provider_type=provider_type_str,
                                upstream_metadata=getattr(pk, "upstream_metadata", None),
                                oauth_invalid_reason=getattr(pk, "oauth_invalid_reason", None),
                            ),
                        )

                    # 释放 DB 连接，因为后续的 PoolManager 排序涉及大量 Redis 操作，
                    # 避免在 Redis I/O 期间长时间占用 DB 连接池。
                    release_db_connection_before_await(db)

                    provider_priority_raw = getattr(provider, "provider_priority", None)
                    try:
                        provider_priority = (
                            int(provider_priority_raw)
                            if provider_priority_raw is not None
                            else 999999
                        )
                    except Exception:
                        provider_priority = 999999
                    try:
                        pool_priority = (
                            int(pool_cfg.global_priority)
                            if pool_cfg.global_priority is not None
                            else provider_priority
                        )
                    except Exception:
                        pool_priority = provider_priority

                    pool_candidate = PoolCandidate(
                        provider=provider,
                        endpoint=endpoint,
                        key=pool_keys[0],
                        pool_keys=pool_keys,
                        pool_config=pool_cfg,
                        pool_priority=pool_priority,
                        needs_conversion=needs_conversion,
                        provider_api_format=str(endpoint_format_str or ""),
                        output_limit=output_limit,
                        capability_miss_count=0,
                    )

                    # 打包延迟检查参数，供 PoolManager 排序后分页调用
                    pool_candidate._deferred_check_params = {
                        "endpoint_format": endpoint_format_str,
                        "model_name": model_name,
                        "capability_requirements": capability_requirements,
                        "model_mappings": model_mappings,
                        "candidate_models": provider_model_names,
                        "provider_type": getattr(provider, "provider_type", None),
                    }

                    if needs_conversion:
                        convertible_candidates.append(pool_candidate)
                    else:
                        exact_candidates.append(pool_candidate)
                    break

                for key in keys_to_check:
                    # Key 级别检查（健康度/熔断按 provider_format bucket）
                    # 传入 provider_model_names 作为 candidate_models，
                    # 用于检查 Key 的 allowed_models 是否支持 Provider 定义的模型名称
                    is_available, key_skip_reason, mapping_matched_model = (
                        self._check_key_availability(
                            key,
                            endpoint_format_str,
                            model_name,
                            capability_requirements,
                            model_mappings=model_mappings,
                            candidate_models=provider_model_names,
                            provider_type=getattr(provider, "provider_type", None),
                        )
                    )

                    candidate = ProviderCandidate(
                        provider=provider,
                        endpoint=endpoint,
                        key=key,
                        is_skipped=not is_available,
                        skip_reason=key_skip_reason,
                        mapping_matched_model=mapping_matched_model,
                        needs_conversion=needs_conversion,
                        provider_api_format=str(endpoint_format_str or ""),
                        output_limit=output_limit,
                        # is_skipped 候选不参与排序，miss_count 无意义，置 0 避免干扰
                        capability_miss_count=(
                            compute_capability_score(
                                key.capabilities or {},
                                capability_requirements,
                            )
                            if is_available
                            else 0
                        ),
                    )

                    if needs_conversion:
                        convertible_candidates.append(candidate)
                    else:
                        exact_candidates.append(candidate)

            candidates.extend(exact_candidates)
            candidates.extend(convertible_candidates)

        # max_candidates 截断应在所有候选收集完成后统一处理，确保优先级排序正确
        if max_candidates and len(candidates) > max_candidates:
            candidates = candidates[:max_candidates]

        return candidates
