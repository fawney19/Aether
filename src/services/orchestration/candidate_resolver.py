"""
候选解析器

负责获取和排序可用的 Provider/Endpoint/Key 组合
"""

import uuid
from datetime import datetime, timezone
from typing import Any, Dict, List, Optional, Tuple

from sqlalchemy.orm import Session

from src.core.enums import APIFormat
from src.core.exceptions import ProviderNotAvailableException
from src.core.logger import logger
from src.models.database import ApiKey
from src.services.cache.aware_scheduler import CacheAwareScheduler, ProviderCandidate



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
        api_format: APIFormat,
        model_name: str,
        affinity_key: str,
        user_api_key: Optional[ApiKey] = None,
        request_id: Optional[str] = None,
        is_stream: bool = False,
        capability_requirements: Optional[Dict[str, bool]] = None,
    ) -> Tuple[List[ProviderCandidate], str]:
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

        Returns:
            (所有候选组合的列表, global_model_id)

        Raises:
            ProviderNotAvailableException: 没有找到任何可用候选时
        """
        all_candidates: List[ProviderCandidate] = []
        provider_offset = 0
        provider_batch_size = 20
        global_model_id: Optional[str] = None

        while True:
            candidates, resolved_global_model_id = await self.cache_scheduler.list_all_candidates(
                db=self.db,
                api_format=api_format,
                model_name=model_name,
                affinity_key=affinity_key,
                user_api_key=user_api_key,
                provider_offset=provider_offset,
                provider_limit=provider_batch_size,
                is_stream=is_stream,
                capability_requirements=capability_requirements,
            )

            if resolved_global_model_id and global_model_id is None:
                global_model_id = resolved_global_model_id

            if not candidates:
                break

            all_candidates.extend(candidates)
            provider_offset += provider_batch_size

        if not all_candidates:
            logger.error(f"  [{request_id}] 没有找到任何可用的 Provider/Endpoint/Key 组合")
            request_type = "流式" if is_stream else "非流式"
            raise ProviderNotAvailableException(
                f"没有可用的 Provider 支持模型 {model_name} 的{request_type}请求"
            )

        logger.debug(f"  [{request_id}] 获取到 {len(all_candidates)} 个候选组合")

        # 如果没有解析到 global_model_id，使用原始 model_name 作为后备
        return all_candidates, global_model_id or model_name

    def create_candidate_records(
        self,
        all_candidates: List[ProviderCandidate],
        request_id: Optional[str],
        user_id: str,
        user_api_key: ApiKey,
        required_capabilities: Optional[Dict[str, bool]] = None,
    ) -> Dict[Tuple[int, int], str]:
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

        candidate_records_to_insert: List[Dict[str, Any]] = []
        candidate_record_map: Dict[Tuple[int, int], str] = {}

        # 只保存启用的能力（值为 True 的）
        active_capabilities = None
        if required_capabilities:
            active_capabilities = {k: v for k, v in required_capabilities.items() if v}
            if not active_capabilities:
                active_capabilities = None

        for candidate_index, candidate in enumerate(all_candidates):
            provider = candidate.provider
            endpoint = candidate.endpoint
            key = candidate.key

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
                        "provider_id": provider.id,
                        "endpoint_id": endpoint.id,
                        "key_id": key.id,
                        "status": "skipped",
                        "skip_reason": candidate.skip_reason,
                        "is_cached": candidate.is_cached,
                        "extra_data": {},
                        "required_capabilities": active_capabilities,
                        "created_at": datetime.now(timezone.utc),
                    }
                )
                candidate_record_map[(candidate_index, 0)] = record_id
            else:
                # max_retries 已从 Endpoint 迁移到 Provider（Endpoint 仍可能保留旧字段用于兼容）
                max_retries_for_candidate = int(provider.max_retries or 2) if candidate.is_cached else 1

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
                            "provider_id": provider.id,
                            "endpoint_id": endpoint.id,
                            "key_id": key.id,
                            "status": "available",
                            "is_cached": candidate.is_cached,
                            "extra_data": {},
                            "required_capabilities": active_capabilities,
                            "created_at": datetime.now(timezone.utc),
                        }
                    )
                    candidate_record_map[(candidate_index, retry_index)] = record_id

        if candidate_records_to_insert:
            self.db.bulk_insert_mappings(
                RequestCandidate, candidate_records_to_insert  # type: ignore
            )
            self.db.flush()

            logger.debug(f"  [{request_id}] 批量插入完成: {len(candidate_records_to_insert)} 条记录")

        return candidate_record_map

    def get_active_candidates(
        self,
        all_candidates: List[ProviderCandidate],
    ) -> List[Tuple[int, ProviderCandidate]]:
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
        all_candidates: List[ProviderCandidate],
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
                provider = candidate.provider
                max_retries = int(provider.max_retries or 2) if candidate.is_cached else 1
                total += max_retries
        return total
