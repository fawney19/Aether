"""
候选解析器

负责获取和排序可用的 Provider/Endpoint/Key 组合
"""

import asyncio
import uuid
from datetime import datetime, timezone
from typing import Any

from sqlalchemy import tuple_
from sqlalchemy.orm import Session

from src.core.exceptions import ProviderNotAvailableException
from src.core.logger import logger
from src.models.database import ApiKey
from src.services.provider.format import normalize_endpoint_signature
from src.services.scheduling.aware_scheduler import CacheAwareScheduler
from src.services.scheduling.schemas import PoolCandidate, ProviderCandidate


class CandidateResolver:
    """
    候选解析器 - 负责获取和排序可用的 Provider 组合

    职责：
    1. 从 CacheAwareScheduler 获取所有可用候选
    2. 创建候选记录（用于追踪）
    3. 提供候选的迭代和过滤功能
    """

    def __init__(
        self,
        db: Session,
        cache_scheduler: CacheAwareScheduler,
    ) -> None:
        """
        初始化候选解析器

        Args:
            db: 数据库会话
            cache_scheduler: 缓存感知调度器
        """
        self.db = db
        self.cache_scheduler = cache_scheduler

    async def fetch_candidates(
        self,
        api_format: str,
        model_name: str,
        affinity_key: str,
        user_api_key: ApiKey | None = None,
        request_id: str | None = None,
        is_stream: bool = False,
        capability_requirements: dict[str, bool] | None = None,
        preferred_key_ids: list[str] | None = None,
        request_body: dict | None = None,
    ) -> tuple[list[ProviderCandidate], str]:
        """
        获取所有可用候选

        Args:
            api_format: API 格式
            model_name: 模型名称
            affinity_key: 亲和性标识符（通常为API Key ID，用于缓存亲和性）
            user_api_key: 用户 API Key（用于 allowed_providers/allowed_api_formats 过滤）
            request_id: 请求 ID（用于日志）
            is_stream: 是否是流式请求，如果为 True 则过滤不支持流式的 Provider
            capability_requirements: 能力需求（用于过滤不满足能力要求的 Key）
            preferred_key_ids: 优先使用的 Provider Key ID 列表（匹配则置顶）

        Returns:
            (所有候选组合的列表, global_model_id)

        Raises:
            ProviderNotAvailableException: 没有找到任何可用候选时
        """
        api_format_norm = normalize_endpoint_signature(api_format)

        logger.debug(
            "[CandidateResolver] fetch_candidates starting: model={}, api_format={}",
            model_name,
            api_format_norm,
        )

        # 一次性全量获取：分页后仍要走 reorder_candidates 全局重排，
        # 分页不会降低工作量，只会让 DB 查询翻倍（Provider × UserGroup 每页都查）。
        all_candidates, global_model_id, _ = await self.cache_scheduler.list_all_candidates(
            db=self.db,
            api_format=api_format_norm,
            model_name=model_name,
            affinity_key=affinity_key,
            user_api_key=user_api_key,
            provider_offset=0,
            provider_limit=None,
            is_stream=is_stream,
            capability_requirements=capability_requirements,
            request_body=request_body,
        )

        logger.debug(
            "[CandidateResolver] fetch_candidates completed: total={} candidates",
            len(all_candidates),
        )

        if not all_candidates:
            logger.error(f"  [{request_id}] 没有找到任何可用的 Provider/Endpoint/Key 组合")
            request_type = "流式" if is_stream else "非流式"
            raise ProviderNotAvailableException(
                f"没有可用的 Provider 支持模型 {model_name} 的{request_type}请求"
            )

        logger.debug(f"  [{request_id}] 获取到 {len(all_candidates)} 个候选组合")

        # Provider 分页会导致候选在全局维度上排序失真（尤其是路由降级分组 / cache_affinity）。
        # 这里在汇总后再次应用全局排序规则，保证遍历顺序符合当前调度配置。
        try:
            all_candidates = await self.cache_scheduler.reorder_candidates(
                candidates=all_candidates,
                db=self.db,
                affinity_key=affinity_key,
                api_format=api_format_norm,
                global_model_id=global_model_id,
            )
        except Exception as exc:
            logger.warning(
                "[CandidateResolver] global reorder failed, keep paged order: {}",
                exc,
            )

        if preferred_key_ids:
            preferred_set = {str(kid) for kid in preferred_key_ids if kid}
            if preferred_set:

                def _is_preferred_candidate(c: ProviderCandidate) -> bool:
                    if c.key and str(c.key.id) in preferred_set:
                        return True
                    if isinstance(c, PoolCandidate):
                        return any(str(pk.id) in preferred_set for pk in (c.pool_keys or []))
                    return False

                preferred_candidates = [c for c in all_candidates if _is_preferred_candidate(c)]
                other_candidates = [c for c in all_candidates if not _is_preferred_candidate(c)]
                if preferred_candidates:
                    matched_key_ids: list[str] = []
                    for candidate in preferred_candidates:
                        if isinstance(candidate, PoolCandidate):
                            matched_key_ids.extend(
                                str(pk.id)
                                for pk in (candidate.pool_keys or [])
                                if str(pk.id) in preferred_set
                            )
                        elif candidate.key:
                            matched_key_ids.append(str(candidate.key.id))
                    logger.debug(
                        f"  [{request_id}] 优先候选命中: {len(preferred_candidates)} 个 "
                        f"(key_ids={matched_key_ids[:3]}{'...' if len(matched_key_ids) > 3 else ''})"
                    )
                else:
                    logger.debug(
                        f"  [{request_id}] 优先候选未命中: 请求的 key_ids={list(preferred_set)[:3]} "
                        "不在可用候选中，将使用普通优先级"
                    )
                all_candidates = preferred_candidates + other_candidates

        # 如果没有解析到 global_model_id，使用原始 model_name 作为后备
        return all_candidates, global_model_id or model_name

    def create_candidate_records(
        self,
        all_candidates: list[ProviderCandidate],
        request_id: str | None,
        user_id: str | None,
        user_api_key: ApiKey | None,
        required_capabilities: dict[str, bool] | None = None,
        *,
        expand_retries: bool = True,
    ) -> dict[tuple[int, int], str]:
        """
        为所有候选预先创建 available 状态记录（批量插入优化）

        Args:
            all_candidates: 所有候选组合
            request_id: 请求 ID
            user_id: 用户 ID
            user_api_key: 用户 API Key 对象
            required_capabilities: 请求需要的能力标签

        Returns:
            candidate_record_map: {(candidate_index, retry_index): candidate_record_id}
        """
        from src.models.database import RequestCandidate

        candidate_records_to_insert: list[dict[str, Any]] = []
        candidate_record_map: dict[tuple[int, int], str] = {}
        username = None
        api_key_name = getattr(user_api_key, "name", None) if user_api_key else None

        if user_api_key is not None:
            try:
                user = getattr(user_api_key, "user", None)
            except Exception:
                user = None
            username = getattr(user, "username", None) if user is not None else None

        # 只保存启用的能力（值为 True 的）
        active_capabilities = None
        if required_capabilities:
            active_capabilities = {k: v for k, v in required_capabilities.items() if v}
            if not active_capabilities:
                active_capabilities = None

        def _retry_slots_for_candidate(candidate: ProviderCandidate) -> int:
            if not expand_retries:
                return 1
            return int(candidate.provider.max_retries or 2) if candidate.is_cached else 1

        for candidate_index, candidate in enumerate(all_candidates):
            provider = candidate.provider
            endpoint = candidate.endpoint
            key = candidate.key
            pool_extra = (
                getattr(candidate, "_pool_extra_data", None)
                if isinstance(getattr(candidate, "_pool_extra_data", None), dict)
                else {}
            )
            base_extra = {
                "needs_conversion": candidate.needs_conversion,
                "provider_api_format": candidate.provider_api_format or None,
                "mapping_matched_model": candidate.mapping_matched_model or None,
                **pool_extra,
            }

            if isinstance(candidate, PoolCandidate) and candidate.pool_keys:
                retry_slots = _retry_slots_for_candidate(candidate)
                for key_idx, pool_key in enumerate(candidate.pool_keys):
                    key_id = str(pool_key.id)
                    key_pool_extra = (
                        getattr(pool_key, "_pool_extra_data", None)
                        if isinstance(getattr(pool_key, "_pool_extra_data", None), dict)
                        else {}
                    )
                    key_skipped = candidate.is_skipped or bool(
                        getattr(pool_key, "_pool_skipped", False)
                    )
                    key_skip_reason_raw = (
                        getattr(pool_key, "_pool_skip_reason", None) if key_skipped else None
                    )
                    key_skip_reason = (
                        str(key_skip_reason_raw)
                        if key_skip_reason_raw
                        else (candidate.skip_reason if key_skipped else None)
                    )
                    mapping_model = getattr(pool_key, "_pool_mapping_matched_model", None)
                    extra_data = {
                        **base_extra,
                        "mapping_matched_model": (
                            mapping_model
                            if mapping_model
                            else base_extra.get("mapping_matched_model")
                        ),
                        "pool_group_id": str(provider.id),
                        "pool_key_index": key_idx,
                        **key_pool_extra,
                    }

                    for retry_in_key in range(retry_slots):
                        retry_index = key_idx * retry_slots + retry_in_key
                        status = "skipped" if key_skipped else "available"
                        record_id = str(uuid.uuid4())
                        candidate_records_to_insert.append(
                            {
                                "id": record_id,
                                "request_id": request_id,
                                "candidate_index": candidate_index,
                                "retry_index": retry_index,
                                "user_id": user_id,
                                "api_key_id": user_api_key.id if user_api_key else None,
                                "username": username,
                                "api_key_name": api_key_name,
                                "provider_id": provider.id,
                                "endpoint_id": endpoint.id,
                                "key_id": key_id,
                                "status": status,
                                "skip_reason": key_skip_reason if key_skipped else None,
                                "is_cached": candidate.is_cached,
                                "extra_data": extra_data,
                                "required_capabilities": active_capabilities,
                                "created_at": datetime.now(timezone.utc),
                            }
                        )
                        candidate_record_map[(candidate_index, retry_index)] = record_id
                continue

            if candidate.is_skipped:
                record_id = str(uuid.uuid4())
                candidate_records_to_insert.append(
                    {
                        "id": record_id,
                        "request_id": request_id,
                        "candidate_index": candidate_index,
                        "retry_index": 0,
                        "user_id": user_id,
                        "api_key_id": user_api_key.id if user_api_key else None,
                        "username": username,
                        "api_key_name": api_key_name,
                        "provider_id": provider.id,
                        "endpoint_id": endpoint.id,
                        "key_id": key.id,
                        "status": "skipped",
                        "skip_reason": candidate.skip_reason,
                        "is_cached": candidate.is_cached,
                        "extra_data": base_extra,
                        "required_capabilities": active_capabilities,
                        "created_at": datetime.now(timezone.utc),
                    }
                )
                candidate_record_map[(candidate_index, 0)] = record_id
            else:
                max_retries_for_candidate = _retry_slots_for_candidate(candidate)

                for retry_index in range(max_retries_for_candidate):
                    record_id = str(uuid.uuid4())
                    candidate_records_to_insert.append(
                        {
                            "id": record_id,
                            "request_id": request_id,
                            "candidate_index": candidate_index,
                            "retry_index": retry_index,
                            "user_id": user_id,
                            "api_key_id": user_api_key.id if user_api_key else None,
                            "username": username,
                            "api_key_name": api_key_name,
                            "provider_id": provider.id,
                            "endpoint_id": endpoint.id,
                            "key_id": key.id,
                            "status": "available",
                            "skip_reason": None,
                            "is_cached": candidate.is_cached,
                            "extra_data": base_extra,
                            "required_capabilities": active_capabilities,
                            "created_at": datetime.now(timezone.utc),
                        }
                    )
                    candidate_record_map[(candidate_index, retry_index)] = record_id

        if candidate_records_to_insert:
            bind = self.db.get_bind()
            dialect_name = getattr(bind.dialect, "name", "")

            if request_id and dialect_name in {"postgresql", "sqlite"}:
                if dialect_name == "postgresql":
                    from sqlalchemy.dialects.postgresql import insert as dialect_insert

                    stmt = dialect_insert(RequestCandidate).values(candidate_records_to_insert)
                    stmt = stmt.on_conflict_do_nothing(
                        constraint="uq_request_candidate_with_retry"
                    )
                else:
                    from sqlalchemy.dialects.sqlite import insert as dialect_insert

                    stmt = dialect_insert(RequestCandidate).values(candidate_records_to_insert)
                    stmt = stmt.on_conflict_do_nothing(
                        index_elements=["request_id", "candidate_index", "retry_index"]
                    )

                self.db.execute(stmt)
                self.db.flush()

                expected_slots = sorted(candidate_record_map.keys())
                actual_rows = (
                    self.db.query(
                        RequestCandidate.id,
                        RequestCandidate.candidate_index,
                        RequestCandidate.retry_index,
                    )
                    .filter(RequestCandidate.request_id == request_id)
                    .filter(
                        tuple_(
                            RequestCandidate.candidate_index,
                            RequestCandidate.retry_index,
                        ).in_(expected_slots)
                    )
                    .all()
                )
                candidate_record_map = {
                    (int(row.candidate_index), int(row.retry_index)): str(row.id)
                    for row in actual_rows
                }

                if len(candidate_record_map) != len(expected_slots):
                    raise RuntimeError(
                        "candidate records incomplete after idempotent insert: "
                        f"expected={len(expected_slots)}, actual={len(candidate_record_map)}, "
                        f"request_id={request_id}"
                    )
            else:
                self.db.bulk_insert_mappings(
                    RequestCandidate, candidate_records_to_insert  # type: ignore
                )
                self.db.flush()

            logger.debug(
                f"  [{request_id}] 批量插入完成: {len(candidate_records_to_insert)} 条记录"
            )

        return candidate_record_map

    async def create_candidate_records_async(
        self,
        all_candidates: list[ProviderCandidate],
        request_id: str | None,
        user_id: str | None,
        user_api_key: ApiKey | None,
        required_capabilities: dict[str, bool] | None = None,
        *,
        expand_retries: bool = True,
    ) -> dict[tuple[int, int], str]:
        """异步版本的 create_candidate_records，将同步 DB 操作放到线程池执行，
        避免阻塞 asyncio 事件循环。"""
        return await asyncio.to_thread(
            self.create_candidate_records,
            all_candidates,
            request_id,
            user_id,
            user_api_key,
            required_capabilities,
            expand_retries=expand_retries,
        )

    def get_active_candidates(
        self,
        all_candidates: list[ProviderCandidate],
    ) -> list[tuple[int, ProviderCandidate]]:
        """
        获取所有非跳过的候选（带索引）

        Args:
            all_candidates: 所有候选组合

        Returns:
            List of (index, candidate) for non-skipped candidates
        """
        return [(i, c) for i, c in enumerate(all_candidates) if not c.is_skipped]

    def count_total_attempts(
        self,
        all_candidates: list[ProviderCandidate],
    ) -> int:
        """
        计算总尝试次数

        Args:
            all_candidates: 所有候选组合

        Returns:
            总尝试次数
        """
        total = 0
        for candidate in all_candidates:
            if not candidate.is_skipped:
                retries_per_slot = (
                    int(candidate.provider.max_retries or 2) if candidate.is_cached else 1
                )
                if isinstance(candidate, PoolCandidate):
                    schedulable_keys = [
                        k
                        for k in (candidate.pool_keys or [])
                        if not bool(getattr(k, "_pool_skipped", False))
                    ]
                    if schedulable_keys:
                        total += len(schedulable_keys) * retries_per_slot
                    elif candidate.pool_keys:
                        # 兜底：尚未附加 _pool_skipped 标记时，按 key 数估算。
                        total += len(candidate.pool_keys) * retries_per_slot
                    else:
                        total += retries_per_slot
                else:
                    total += retries_per_slot
        return total
