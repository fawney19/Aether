"""
缓存监控端点

提供缓存亲和性统计、管理和监控功能
"""

from dataclasses import dataclass
from typing import Any, Dict, List, Optional, Tuple

from fastapi import APIRouter, Depends, HTTPException, Query, Request
from fastapi.responses import PlainTextResponse
from sqlalchemy.orm import Session

from src.api.base.admin_adapter import AdminApiAdapter
from src.api.base.context import ApiRequestContext
from src.api.base.pagination import PaginationMeta, build_pagination_payload, paginate_sequence
from src.api.base.pipeline import ApiRequestPipeline
from src.clients.redis_client import get_redis_client_sync
from src.core.crypto import crypto_service
from src.core.logger import logger
from src.database import get_db
from src.models.database import ApiKey, User
from src.services.cache.affinity_manager import get_affinity_manager
from src.services.cache.aware_scheduler import CacheAwareScheduler, get_cache_aware_scheduler
from src.services.system.config import SystemConfigService

router = APIRouter(prefix="/api/admin/monitoring/cache", tags=["Admin - Monitoring: Cache"])
pipeline = ApiRequestPipeline()


def mask_api_key(api_key: Optional[str], prefix_len: int = 8, suffix_len: int = 4) -> Optional[str]:
    """
    脱敏 API Key，显示前缀 + 星号 + 后缀
    例如: sk-jhiId-xxxxxxxxxxxAABB -> sk-jhiId-********AABB

    Args:
        api_key: 原始 API Key
        prefix_len: 显示的前缀长度，默认 8
        suffix_len: 显示的后缀长度，默认 4
    """
    if not api_key:
        return None
    total_visible = prefix_len + suffix_len
    if len(api_key) <= total_visible:
        # Key 太短，直接返回部分内容 + 星号
        return api_key[:prefix_len] + "********"
    return f"{api_key[:prefix_len]}********{api_key[-suffix_len:]}"


def decrypt_and_mask(encrypted_key: Optional[str], prefix_len: int = 8) -> Optional[str]:
    """
    解密 API Key 后脱敏显示

    Args:
        encrypted_key: 加密后的 API Key
        prefix_len: 显示的前缀长度
    """
    if not encrypted_key:
        return None
    try:
        decrypted = crypto_service.decrypt(encrypted_key)
        return mask_api_key(decrypted, prefix_len)
    except Exception:
        # 解密失败时返回 None
        return None


def resolve_user_identifier(db: Session, identifier: str) -> Optional[str]:
    """
    将用户标识符（username/email/user_id/api_key_id）解析为 user_id

    支持的输入格式：
    1. User UUID (36位，带横杠)
    2. Username (用户名)
    3. Email (邮箱)
    4. API Key ID (36位UUID)

    返回：
    - user_id (UUID字符串) 或 None
    """
    identifier = identifier.strip()

    # 1. 先尝试作为 User UUID 查询
    user = db.query(User).filter(User.id == identifier).first()
    if user:
        logger.debug(f"通过User ID解析: {identifier[:8]}... -> {user.username}")
        return user.id

    # 2. 尝试作为 Username 查询
    user = db.query(User).filter(User.username == identifier).first()
    if user:
        logger.debug(f"通过Username解析: {identifier} -> {user.id[:8]}...")  # type: ignore[index]
        return user.id

    # 3. 尝试作为 Email 查询
    user = db.query(User).filter(User.email == identifier).first()
    if user:
        logger.debug(f"通过Email解析: {identifier} -> {user.id[:8]}...")  # type: ignore[index]
        return user.id

    # 4. 尝试作为 API Key ID 查询
    api_key = db.query(ApiKey).filter(ApiKey.id == identifier).first()
    if api_key:
        logger.debug(f"通过API Key ID解析: {identifier[:8]}... -> User ID: {api_key.user_id[:8]}...")  # type: ignore[index]
        return api_key.user_id

    # 无法识别
    logger.debug(f"无法识别的用户标识符: {identifier}")
    return None


@router.get("/stats")
async def get_cache_stats(
    request: Request,
    db: Session = Depends(get_db),
) -> Any:
    """
    获取缓存亲和性统计信息

    获取缓存调度器的运行统计数据，包括命中率、切换次数、调度器配置等。
    用于监控缓存亲和性功能的运行状态和性能指标。

    **返回字段**:
    - `status`: 状态（ok）
    - `data`: 统计数据对象
      - `scheduler`: 调度器名称（cache_aware 或 random）
      - `total_affinities`: 总缓存亲和性数量
      - `cache_hit_rate`: 缓存命中率（0.0-1.0）
      - `provider_switches`: Provider 切换次数
      - `key_switches`: Key 切换次数
      - `cache_hits`: 缓存命中次数
      - `cache_misses`: 缓存未命中次数
      - `scheduler_metrics`: 调度器详细指标
      - `affinity_stats`: 亲和性统计数据
    """
    adapter = AdminCacheStatsAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/affinity/{user_identifier}")
async def get_user_affinity(
    user_identifier: str,
    request: Request,
    db: Session = Depends(get_db),
) -> Any:
    """
    查询指定用户的所有缓存亲和性

    根据用户标识符查询该用户在各个端点上的缓存亲和性记录。
    支持多种标识符格式的自动识别和解析。

    **路径参数**:
    - `user_identifier`: 用户标识符，支持以下格式：
      - 用户名（username），如：yuanhonghu
      - 邮箱（email），如：user@example.com
      - 用户 UUID（user_id），如：550e8400-e29b-41d4-a716-446655440000
      - API Key ID，如：660e8400-e29b-41d4-a716-446655440000

    **返回字段**:
    - `status`: 状态（ok 或 not_found）
    - `message`: 提示消息（当无缓存时）
    - `user_info`: 用户信息
      - `user_id`: 用户 ID
      - `username`: 用户名
      - `email`: 邮箱
    - `affinities`: 缓存亲和性列表
      - `provider_id`: Provider ID
      - `endpoint_id`: Endpoint ID
      - `key_id`: Key ID
      - `api_format`: API 格式
      - `model_name`: 模型名称（global_model_id）
      - `created_at`: 创建时间
      - `expire_at`: 过期时间
      - `request_count`: 请求计数
    - `total_endpoints`: 缓存的端点数量
    """
    adapter = AdminGetUserAffinityAdapter(user_identifier=user_identifier)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/affinities")
async def list_affinities(
    request: Request,
    keyword: Optional[str] = None,
    limit: int = Query(100, ge=1, le=1000, description="返回数量限制"),
    offset: int = Query(0, ge=0, description="偏移量"),
    db: Session = Depends(get_db),
) -> Any:
    """
    获取所有缓存亲和性列表

    查询系统中所有的缓存亲和性记录，支持按关键词过滤和分页。
    返回详细的用户、Provider、Endpoint、Key 信息。

    **查询参数**:
    - `keyword`: 可选，支持以下过滤方式（可选）
      - 用户名/邮箱/User ID/API Key ID（精确匹配）
      - 任意字段的模糊匹配（affinity_key、user_id、username、email、provider_id、key_id）
    - `limit`: 返回数量限制（1-1000，默认 100）
    - `offset`: 偏移量（用于分页，默认 0）

    **返回字段**:
    - `status`: 状态（ok）
    - `data`: 分页数据对象
      - `items`: 缓存亲和性列表
        - `affinity_key`: API Key ID（用于缓存键）
        - `user_api_key_name`: 用户 API Key 名称
        - `user_api_key_prefix`: 脱敏后的用户 API Key
        - `is_standalone`: 是否为独立 API Key
        - `user_id`: 用户 ID
        - `username`: 用户名
        - `email`: 邮箱
        - `provider_id`: Provider ID
        - `provider_name`: Provider 显示名称
        - `endpoint_id`: Endpoint ID
        - `endpoint_api_format`: Endpoint API 格式
        - `endpoint_url`: Endpoint 基础 URL
        - `key_id`: Key ID
        - `key_name`: Key 名称
        - `key_prefix`: 脱敏后的 Provider Key
        - `rate_multiplier`: 速率倍数
        - `global_model_id`: GlobalModel ID
        - `model_name`: 模型名称
        - `model_display_name`: 模型显示名称
        - `api_format`: API 格式
        - `created_at`: 创建时间
        - `expire_at`: 过期时间
        - `request_count`: 请求计数
      - `meta`: 分页元数据
        - `count`: 总数量
        - `limit`: 每页数量
        - `offset`: 当前偏移量
      - `matched_user_id`: 匹配到的用户 ID（当关键词为用户标识时）
    """
    adapter = AdminListAffinitiesAdapter(keyword=keyword, limit=limit, offset=offset)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.delete("/users/{user_identifier}")
async def clear_user_cache(
    user_identifier: str,
    request: Request,
    db: Session = Depends(get_db),
) -> Any:
    """
    清除指定用户的缓存亲和性

    清除指定用户或 API Key 的所有缓存亲和性记录。
    支持按用户维度或单个 API Key 维度清除。

    **路径参数**:
    - `user_identifier`: 用户标识符，支持以下格式：
      - 用户名（username）
      - 邮箱（email）
      - 用户 UUID（user_id）
      - API Key ID（清除该 API Key 的缓存）

    **返回字段**:
    - `status`: 状态（ok）
    - `message`: 操作结果消息
    - `user_info`: 用户信息
      - `user_id`: 用户 ID
      - `username`: 用户名
      - `email`: 邮箱
      - `api_key_id`: API Key ID（当清除单个 API Key 时）
      - `api_key_name`: API Key 名称（当清除单个 API Key 时）
    """
    adapter = AdminClearUserCacheAdapter(user_identifier=user_identifier)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.delete("/affinity/{affinity_key}/{endpoint_id}/{model_id}/{api_format}")
async def clear_single_affinity(
    affinity_key: str,
    endpoint_id: str,
    model_id: str,
    api_format: str,
    request: Request,
    db: Session = Depends(get_db),
) -> Any:
    """
    清除单条缓存亲和性记录

    根据精确的缓存键（affinity_key + endpoint_id + model_id + api_format）
    清除单条缓存亲和性记录。用于精确控制缓存清除。

    **路径参数**:
    - `affinity_key`: API Key ID（用于缓存的键）
    - `endpoint_id`: Endpoint ID
    - `model_id`: GlobalModel ID
    - `api_format`: API 格式（如：claude、openai、gemini）

    **返回字段**:
    - `status`: 状态（ok）
    - `message`: 操作结果消息
    - `affinity_key`: API Key ID
    - `endpoint_id`: Endpoint ID
    - `model_id`: GlobalModel ID
    """
    adapter = AdminClearSingleAffinityAdapter(
        affinity_key=affinity_key, endpoint_id=endpoint_id, model_id=model_id, api_format=api_format
    )
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.delete("")
async def clear_all_cache(
    request: Request,
    db: Session = Depends(get_db),
) -> Any:
    """
    清除所有缓存亲和性

    清除系统中所有用户的缓存亲和性记录。此操作会影响所有用户，
    下次请求时将重新建立缓存亲和性。请谨慎使用。

    **警告**: 此操作影响所有用户，使用前请确认

    **返回字段**:
    - `status`: 状态（ok）
    - `message`: 操作结果消息
    - `count`: 清除的缓存数量
    """
    adapter = AdminClearAllCacheAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.delete("/providers/{provider_id}")
async def clear_provider_cache(
    provider_id: str,
    request: Request,
    db: Session = Depends(get_db),
) -> Any:
    """
    清除指定 Provider 的缓存亲和性

    清除与指定 Provider 相关的所有缓存亲和性记录。
    当 Provider 配置变更或下线时使用。

    **路径参数**:
    - `provider_id`: Provider ID

    **返回字段**:
    - `status`: 状态（ok）
    - `message`: 操作结果消息
    - `provider_id`: Provider ID
    - `count`: 清除的缓存数量
    """
    adapter = AdminClearProviderCacheAdapter(provider_id=provider_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/config")
async def get_cache_config(
    request: Request,
    db: Session = Depends(get_db),
) -> Any:
    """
    获取缓存相关配置

    获取缓存亲和性功能的配置参数，包括缓存 TTL、预留比例、
    动态预留机制配置等。

    **返回字段**:
    - `status`: 状态（ok）
    - `data`: 配置数据
      - `cache_ttl_seconds`: 缓存亲和性有效期（秒）
      - `cache_reservation_ratio`: 静态预留比例（已被动态预留替代）
      - `dynamic_reservation`: 动态预留机制配置
        - `enabled`: 是否启用
        - `config`: 配置参数
          - `probe_phase_requests`: 探测阶段请求数阈值
          - `probe_reservation`: 探测阶段预留比例
          - `stable_min_reservation`: 稳定阶段最小预留比例
          - `stable_max_reservation`: 稳定阶段最大预留比例
          - `low_load_threshold`: 低负载阈值
          - `high_load_threshold`: 高负载阈值
        - `description`: 各参数说明
      - `description`: 配置说明
    """
    adapter = AdminCacheConfigAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/metrics", response_class=PlainTextResponse)
async def get_cache_metrics(
    request: Request,
    db: Session = Depends(get_db),
) -> Any:
    """
    获取缓存调度指标（Prometheus 格式）

    以 Prometheus 文本格式输出缓存调度器的监控指标，
    方便接入 Prometheus/Grafana 等监控系统。

    **返回格式**: Prometheus 文本格式（Content-Type: text/plain）

    **指标列表**:
    - `cache_scheduler_total_batches`: 总批次数
    - `cache_scheduler_last_batch_size`: 最后一批候选数
    - `cache_scheduler_total_candidates`: 总候选数
    - `cache_scheduler_last_candidate_count`: 最后一批候选计数
    - `cache_scheduler_cache_hits`: 缓存命中次数
    - `cache_scheduler_cache_misses`: 缓存未命中次数
    - `cache_scheduler_cache_hit_rate`: 缓存命中率
    - `cache_scheduler_concurrency_denied`: 并发拒绝次数
    - `cache_scheduler_avg_candidates_per_batch`: 平均每批候选数
    - `cache_affinity_total`: 总缓存亲和性数量
    - `cache_affinity_hits`: 亲和性命中次数
    - `cache_affinity_misses`: 亲和性未命中次数
    - `cache_affinity_hit_rate`: 亲和性命中率
    - `cache_affinity_invalidations`: 亲和性失效次数
    - `cache_affinity_provider_switches`: Provider 切换次数
    - `cache_affinity_key_switches`: Key 切换次数
    - `cache_scheduler_info`: 调度器信息（label: scheduler）
    """
    adapter = AdminCacheMetricsAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


# -------- 缓存监控适配器 --------


class AdminCacheStatsAdapter(AdminApiAdapter):
    async def handle(self, context: ApiRequestContext) -> Dict[str, Any]:  # type: ignore[override]
        try:
            redis_client = get_redis_client_sync()
            # 读取系统配置，确保监控接口与编排器使用一致的模式
            priority_mode = SystemConfigService.get_config(
                context.db,
                "provider_priority_mode",
                CacheAwareScheduler.PRIORITY_MODE_PROVIDER,
            )
            scheduling_mode = SystemConfigService.get_config(
                context.db,
                "scheduling_mode",
                CacheAwareScheduler.SCHEDULING_MODE_CACHE_AFFINITY,
            )
            scheduler = await get_cache_aware_scheduler(
                redis_client,
                priority_mode=priority_mode,
                scheduling_mode=scheduling_mode,
            )
            stats = await scheduler.get_stats()
            logger.info("缓存统计信息查询成功")
            context.add_audit_metadata(
                action="cache_stats",
                scheduler=stats.get("scheduler"),
                total_affinities=stats.get("total_affinities"),
                cache_hit_rate=stats.get("cache_hit_rate"),
                provider_switches=stats.get("provider_switches"),
            )
            return {"status": "ok", "data": stats}
        except Exception as exc:
            logger.exception(f"获取缓存统计信息失败: {exc}")
            raise HTTPException(status_code=500, detail=f"获取缓存统计失败: {exc}")


class AdminCacheMetricsAdapter(AdminApiAdapter):
    async def handle(self, context: ApiRequestContext) -> PlainTextResponse:
        try:
            redis_client = get_redis_client_sync()
            # 读取系统配置，确保监控接口与编排器使用一致的模式
            priority_mode = SystemConfigService.get_config(
                context.db,
                "provider_priority_mode",
                CacheAwareScheduler.PRIORITY_MODE_PROVIDER,
            )
            scheduling_mode = SystemConfigService.get_config(
                context.db,
                "scheduling_mode",
                CacheAwareScheduler.SCHEDULING_MODE_CACHE_AFFINITY,
            )
            scheduler = await get_cache_aware_scheduler(
                redis_client,
                priority_mode=priority_mode,
                scheduling_mode=scheduling_mode,
            )
            stats = await scheduler.get_stats()
            payload = self._format_prometheus(stats)
            context.add_audit_metadata(
                action="cache_metrics_export",
                scheduler=stats.get("scheduler"),
                metrics_lines=payload.count("\n"),
            )
            return PlainTextResponse(payload)
        except Exception as exc:
            logger.exception(f"导出缓存指标失败: {exc}")
            raise HTTPException(status_code=500, detail=f"导出缓存指标失败: {exc}")

    def _format_prometheus(self, stats: Dict[str, Any]) -> str:
        """
        将 scheduler/affinity 指标转换为 Prometheus 文本格式。
        """
        scheduler_metrics = stats.get("scheduler_metrics", {})
        affinity_stats = stats.get("affinity_stats", {})

        metric_map: List[Tuple[str, str, float]] = [
            (
                "cache_scheduler_total_batches",
                "Total batches pulled from provider list",
                float(scheduler_metrics.get("total_batches", 0)),
            ),
            (
                "cache_scheduler_last_batch_size",
                "Size of the latest candidate batch",
                float(scheduler_metrics.get("last_batch_size", 0)),
            ),
            (
                "cache_scheduler_total_candidates",
                "Total candidates enumerated by scheduler",
                float(scheduler_metrics.get("total_candidates", 0)),
            ),
            (
                "cache_scheduler_last_candidate_count",
                "Number of candidates in the most recent batch",
                float(scheduler_metrics.get("last_candidate_count", 0)),
            ),
            (
                "cache_scheduler_cache_hits",
                "Cache hits counted during scheduling",
                float(scheduler_metrics.get("cache_hits", 0)),
            ),
            (
                "cache_scheduler_cache_misses",
                "Cache misses counted during scheduling",
                float(scheduler_metrics.get("cache_misses", 0)),
            ),
            (
                "cache_scheduler_cache_hit_rate",
                "Cache hit rate during scheduling",
                float(scheduler_metrics.get("cache_hit_rate", 0.0)),
            ),
            (
                "cache_scheduler_concurrency_denied",
                "Times candidate rejected due to concurrency limits",
                float(scheduler_metrics.get("concurrency_denied", 0)),
            ),
            (
                "cache_scheduler_avg_candidates_per_batch",
                "Average candidates per batch",
                float(scheduler_metrics.get("avg_candidates_per_batch", 0.0)),
            ),
        ]

        affinity_map: List[Tuple[str, str, float]] = [
            (
                "cache_affinity_total",
                "Total cache affinities stored",
                float(affinity_stats.get("total_affinities", 0)),
            ),
            (
                "cache_affinity_hits",
                "Affinity cache hits",
                float(affinity_stats.get("cache_hits", 0)),
            ),
            (
                "cache_affinity_misses",
                "Affinity cache misses",
                float(affinity_stats.get("cache_misses", 0)),
            ),
            (
                "cache_affinity_hit_rate",
                "Affinity cache hit rate",
                float(affinity_stats.get("cache_hit_rate", 0.0)),
            ),
            (
                "cache_affinity_invalidations",
                "Affinity invalidations",
                float(affinity_stats.get("cache_invalidations", 0)),
            ),
            (
                "cache_affinity_provider_switches",
                "Affinity provider switches",
                float(affinity_stats.get("provider_switches", 0)),
            ),
            (
                "cache_affinity_key_switches",
                "Affinity key switches",
                float(affinity_stats.get("key_switches", 0)),
            ),
        ]

        lines = []
        for name, help_text, value in metric_map + affinity_map:
            lines.append(f"# HELP {name} {help_text}")
            lines.append(f"# TYPE {name} gauge")
            lines.append(f"{name} {value}")

        scheduler_name = stats.get("scheduler", "cache_aware")
        lines.append(f'cache_scheduler_info{{scheduler="{scheduler_name}"}} 1')

        return "\n".join(lines) + "\n"


@dataclass
class AdminGetUserAffinityAdapter(AdminApiAdapter):
    user_identifier: str

    async def handle(self, context: ApiRequestContext) -> Dict[str, Any]:  # type: ignore[override]
        db = context.db
        try:
            user_id = resolve_user_identifier(db, self.user_identifier)
            if not user_id:
                raise HTTPException(
                    status_code=404,
                    detail=f"无法识别的用户标识符: {self.user_identifier}。支持用户名、邮箱、User ID或API Key ID",
                )

            user = db.query(User).filter(User.id == user_id).first()
            redis_client = get_redis_client_sync()
            affinity_mgr = await get_affinity_manager(redis_client)

            # 获取该用户的所有缓存亲和性
            all_affinities = await affinity_mgr.list_affinities()
            user_affinities = [aff for aff in all_affinities if aff.get("user_id") == user_id]

            if not user_affinities:
                response = {
                    "status": "not_found",
                    "message": f"用户 {user.username} ({user.email}) 没有缓存亲和性",
                    "user_info": {
                        "user_id": user_id,
                        "username": user.username,
                        "email": user.email,
                    },
                    "affinities": [],
                }
                context.add_audit_metadata(
                    action="cache_user_affinity",
                    user_identifier=self.user_identifier,
                    resolved_user_id=user_id,
                    affinity_count=0,
                    status="not_found",
                )
                return response

            response = {
                "status": "ok",
                "user_info": {
                    "user_id": user_id,
                    "username": user.username,
                    "email": user.email,
                },
                "affinities": [
                    {
                        "provider_id": aff["provider_id"],
                        "endpoint_id": aff["endpoint_id"],
                        "key_id": aff["key_id"],
                        "api_format": aff.get("api_format"),
                        "model_name": aff.get("model_name"),
                        "created_at": aff["created_at"],
                        "expire_at": aff["expire_at"],
                        "request_count": aff["request_count"],
                    }
                    for aff in user_affinities
                ],
                "total_endpoints": len(user_affinities),
            }
            context.add_audit_metadata(
                action="cache_user_affinity",
                user_identifier=self.user_identifier,
                resolved_user_id=user_id,
                affinity_count=len(user_affinities),
                status="ok",
            )
            return response
        except HTTPException:
            raise
        except Exception as exc:
            logger.exception(f"查询用户缓存亲和性失败: {exc}")
            raise HTTPException(status_code=500, detail=f"查询失败: {exc}")


@dataclass
class AdminListAffinitiesAdapter(AdminApiAdapter):
    keyword: Optional[str]
    limit: int
    offset: int

    async def handle(self, context: ApiRequestContext) -> Dict[str, Any]:  # type: ignore[override]
        db = context.db
        redis_client = get_redis_client_sync()
        if not redis_client:
            raise HTTPException(status_code=503, detail="Redis未初始化，无法获取缓存亲和性")

        affinity_mgr = await get_affinity_manager(redis_client)
        matched_user_id = None
        matched_api_key_id = None
        raw_affinities: List[Dict[str, Any]] = []

        if self.keyword:
            # 首先检查是否是 API Key ID（affinity_key）
            api_key = db.query(ApiKey).filter(ApiKey.id == self.keyword).first()
            if api_key:
                # 直接通过 affinity_key 过滤
                matched_api_key_id = str(api_key.id)
                matched_user_id = str(api_key.user_id)
                all_affinities = await affinity_mgr.list_affinities()
                raw_affinities = [
                    aff for aff in all_affinities if aff.get("affinity_key") == matched_api_key_id
                ]
            else:
                # 尝试解析为用户标识
                user_id = resolve_user_identifier(db, self.keyword)
                if user_id:
                    matched_user_id = user_id
                    # 获取该用户所有的 API Key ID
                    user_api_keys = db.query(ApiKey).filter(ApiKey.user_id == user_id).all()
                    user_api_key_ids = {str(k.id) for k in user_api_keys}
                    # 过滤出该用户所有 API Key 的亲和性
                    all_affinities = await affinity_mgr.list_affinities()
                    raw_affinities = [
                        aff for aff in all_affinities if aff.get("affinity_key") in user_api_key_ids
                    ]
                else:
                    # 关键词不是有效标识，返回所有亲和性（后续会进行模糊匹配）
                    raw_affinities = await affinity_mgr.list_affinities()
        else:
            raw_affinities = await affinity_mgr.list_affinities()

        # 收集所有 affinity_key (API Key ID)
        affinity_keys = {
            item.get("affinity_key") for item in raw_affinities if item.get("affinity_key")
        }

        # 批量查询用户 API Key 信息
        user_api_key_map: Dict[str, ApiKey] = {}
        if affinity_keys:
            user_api_keys = db.query(ApiKey).filter(ApiKey.id.in_(list(affinity_keys))).all()
            user_api_key_map = {str(k.id): k for k in user_api_keys}

        # 收集所有 user_id
        user_ids = {str(k.user_id) for k in user_api_key_map.values()}
        user_map: Dict[str, User] = {}
        if user_ids:
            users = db.query(User).filter(User.id.in_(list(user_ids))).all()
            user_map = {str(user.id): user for user in users}

        # 收集所有provider_id、endpoint_id、key_id
        provider_ids = {
            item.get("provider_id") for item in raw_affinities if item.get("provider_id")
        }
        endpoint_ids = {
            item.get("endpoint_id") for item in raw_affinities if item.get("endpoint_id")
        }
        key_ids = {item.get("key_id") for item in raw_affinities if item.get("key_id")}

        # 批量查询Provider、Endpoint、Key信息
        from src.models.database import Provider, ProviderAPIKey, ProviderEndpoint

        provider_map = {}
        if provider_ids:
            providers = db.query(Provider).filter(Provider.id.in_(list(provider_ids))).all()
            provider_map = {p.id: p for p in providers}

        endpoint_map = {}
        if endpoint_ids:
            endpoints = (
                db.query(ProviderEndpoint).filter(ProviderEndpoint.id.in_(list(endpoint_ids))).all()
            )
            endpoint_map = {e.id: e for e in endpoints}

        key_map = {}
        if key_ids:
            keys = db.query(ProviderAPIKey).filter(ProviderAPIKey.id.in_(list(key_ids))).all()
            key_map = {k.id: k for k in keys}

        # 收集所有 model_name（实际存储的是 global_model_id）并批量查询 GlobalModel
        from src.models.database import GlobalModel

        global_model_ids = {
            item.get("model_name") for item in raw_affinities if item.get("model_name")
        }
        global_model_map: Dict[str, GlobalModel] = {}
        if global_model_ids:
            # model_name 可能是 UUID 格式的 global_model_id，也可能是原始模型名称
            global_models = db.query(GlobalModel).filter(
                GlobalModel.id.in_(list(global_model_ids))
            ).all()
            global_model_map = {str(gm.id): gm for gm in global_models}

        keyword_lower = self.keyword.lower() if self.keyword else None
        items = []
        for affinity in raw_affinities:
            affinity_key = affinity.get("affinity_key")
            if not affinity_key:
                continue

            # 通过 affinity_key（API Key ID）找到用户 API Key 和用户
            user_api_key = user_api_key_map.get(affinity_key)
            user = user_map.get(str(user_api_key.user_id)) if user_api_key else None
            user_id = str(user_api_key.user_id) if user_api_key else None

            provider_id = affinity.get("provider_id")
            endpoint_id = affinity.get("endpoint_id")
            key_id = affinity.get("key_id")

            provider = provider_map.get(provider_id)
            endpoint = endpoint_map.get(endpoint_id)
            key = key_map.get(key_id)

            # 用户 API Key 脱敏显示（解密 key_encrypted 后脱敏）
            user_api_key_masked = None
            if user_api_key and user_api_key.key_encrypted:
                user_api_key_masked = decrypt_and_mask(user_api_key.key_encrypted)

            # Provider Key 脱敏显示（解密 api_key 后脱敏）
            provider_key_masked = None
            if key and key.api_key:
                provider_key_masked = decrypt_and_mask(key.api_key)

            item = {
                "affinity_key": affinity_key,
                "user_api_key_name": user_api_key.name if user_api_key else None,
                "user_api_key_prefix": user_api_key_masked,
                "is_standalone": user_api_key.is_standalone if user_api_key else False,
                "user_id": user_id,
                "username": user.username if user else None,
                "email": user.email if user else None,
                "provider_id": provider_id,
                "provider_name": provider.name if provider else None,
                "endpoint_id": endpoint_id,
                "endpoint_api_format": (
                    endpoint.api_format if endpoint and endpoint.api_format else None
                ),
                "endpoint_url": endpoint.base_url if endpoint else None,
                "key_id": key_id,
                "key_name": key.name if key else None,
                "key_prefix": provider_key_masked,
                "rate_multiplier": key.rate_multiplier if key else 1.0,
                "global_model_id": affinity.get("model_name"),  # 原始的 global_model_id
                "model_name": (
                    global_model_map.get(affinity.get("model_name")).name
                    if affinity.get("model_name") and global_model_map.get(affinity.get("model_name"))
                    else affinity.get("model_name")  # 如果找不到 GlobalModel，显示原始值
                ),
                "model_display_name": (
                    global_model_map.get(affinity.get("model_name")).display_name
                    if affinity.get("model_name") and global_model_map.get(affinity.get("model_name"))
                    else None
                ),
                "api_format": affinity.get("api_format"),
                "created_at": affinity.get("created_at"),
                "expire_at": affinity.get("expire_at"),
                "request_count": affinity.get("request_count", 0),
            }

            if keyword_lower and not matched_user_id and not matched_api_key_id:
                searchable = [
                    item["affinity_key"],
                    item["user_api_key_name"] or "",
                    item["user_id"] or "",
                    item["username"] or "",
                    item["email"] or "",
                    item["provider_id"] or "",
                    item["key_id"] or "",
                ]
                if not any(keyword_lower in str(value).lower() for value in searchable if value):
                    continue

            items.append(item)

        items.sort(key=lambda x: x.get("expire_at") or 0, reverse=True)
        paged_items, meta = paginate_sequence(items, self.limit, self.offset)
        payload = build_pagination_payload(
            paged_items,
            meta,
            matched_user_id=matched_user_id,
        )
        response = {
            "status": "ok",
            "data": payload,
        }
        result_count = meta.count if hasattr(meta, "count") else len(paged_items)
        context.add_audit_metadata(
            action="cache_affinity_list",
            keyword=self.keyword,
            matched_user_id=matched_user_id,
            matched_api_key_id=matched_api_key_id,
            limit=self.limit,
            offset=self.offset,
            result_count=result_count,
        )
        return response


@dataclass
class AdminClearUserCacheAdapter(AdminApiAdapter):
    user_identifier: str

    async def handle(self, context: ApiRequestContext) -> Dict[str, Any]:  # type: ignore[override]
        db = context.db
        try:
            redis_client = get_redis_client_sync()
            affinity_mgr = await get_affinity_manager(redis_client)

            # 首先检查是否直接是 API Key ID (affinity_key)
            api_key = db.query(ApiKey).filter(ApiKey.id == self.user_identifier).first()
            if api_key:
                # 直接按 affinity_key 清除
                affinity_key = str(api_key.id)
                user = db.query(User).filter(User.id == api_key.user_id).first()

                all_affinities = await affinity_mgr.list_affinities()
                target_affinities = [
                    aff for aff in all_affinities if aff.get("affinity_key") == affinity_key
                ]

                count = 0
                for aff in target_affinities:
                    api_format = aff.get("api_format")
                    model_name = aff.get("model_name")
                    endpoint_id = aff.get("endpoint_id")
                    if api_format and model_name:
                        await affinity_mgr.invalidate_affinity(
                            affinity_key, api_format, model_name, endpoint_id=endpoint_id
                        )
                        count += 1

                logger.info(f"已清除API Key缓存亲和性: api_key_name={api_key.name}, affinity_key={affinity_key[:8]}..., 清除数量={count}")

                response = {
                    "status": "ok",
                    "message": f"已清除 API Key {api_key.name} 的缓存亲和性",
                    "user_info": {
                        "user_id": str(api_key.user_id),
                        "username": user.username if user else None,
                        "email": user.email if user else None,
                        "api_key_id": affinity_key,
                        "api_key_name": api_key.name,
                    },
                }
                context.add_audit_metadata(
                    action="cache_clear_api_key",
                    user_identifier=self.user_identifier,
                    resolved_api_key_id=affinity_key,
                    cleared_count=count,
                )
                return response

            # 如果不是 API Key ID，尝试解析为用户标识
            user_id = resolve_user_identifier(db, self.user_identifier)
            if not user_id:
                raise HTTPException(
                    status_code=404,
                    detail=f"无法识别的标识符: {self.user_identifier}。支持用户名、邮箱、User ID或API Key ID",
                )

            user = db.query(User).filter(User.id == user_id).first()

            # 获取该用户所有的 API Key
            user_api_keys = db.query(ApiKey).filter(ApiKey.user_id == user_id).all()
            user_api_key_ids = {str(k.id) for k in user_api_keys}

            # 获取该用户所有 API Key 的缓存亲和性并逐个失效
            all_affinities = await affinity_mgr.list_affinities()
            user_affinities = [
                aff for aff in all_affinities if aff.get("affinity_key") in user_api_key_ids
            ]

            count = 0
            for aff in user_affinities:
                affinity_key = aff.get("affinity_key")
                api_format = aff.get("api_format")
                model_name = aff.get("model_name")
                endpoint_id = aff.get("endpoint_id")
                if affinity_key and api_format and model_name:
                    await affinity_mgr.invalidate_affinity(
                        affinity_key, api_format, model_name, endpoint_id=endpoint_id
                    )
                    count += 1

            logger.info(f"已清除用户缓存亲和性: username={user.username}, user_id={user_id[:8]}..., 清除数量={count}")

            response = {
                "status": "ok",
                "message": f"已清除用户 {user.username} 的所有缓存亲和性",
                "user_info": {"user_id": user_id, "username": user.username, "email": user.email},
            }
            context.add_audit_metadata(
                action="cache_clear_user",
                user_identifier=self.user_identifier,
                resolved_user_id=user_id,
                cleared_count=count,
            )
            return response
        except HTTPException:
            raise
        except Exception as exc:
            logger.exception(f"清除用户缓存亲和性失败: {exc}")
            raise HTTPException(status_code=500, detail=f"清除失败: {exc}")


@dataclass
class AdminClearSingleAffinityAdapter(AdminApiAdapter):
    affinity_key: str
    endpoint_id: str
    model_id: str
    api_format: str

    async def handle(self, context: ApiRequestContext) -> Dict[str, Any]:  # type: ignore[override]
        db = context.db
        try:
            redis_client = get_redis_client_sync()
            affinity_mgr = await get_affinity_manager(redis_client)

            # 直接获取指定的亲和性记录（无需遍历全部）
            existing_affinity = await affinity_mgr.get_affinity(
                self.affinity_key, self.api_format, self.model_id
            )

            if not existing_affinity:
                raise HTTPException(status_code=404, detail="未找到指定的缓存亲和性记录")

            # 验证 endpoint_id 是否匹配
            if existing_affinity.endpoint_id != self.endpoint_id:
                raise HTTPException(status_code=404, detail="未找到指定的缓存亲和性记录")

            # 失效单条记录
            await affinity_mgr.invalidate_affinity(
                self.affinity_key, self.api_format, self.model_id, endpoint_id=self.endpoint_id
            )

            # 获取用于日志的信息
            api_key = db.query(ApiKey).filter(ApiKey.id == self.affinity_key).first()
            api_key_name = api_key.name if api_key else None

            logger.info(
                f"已清除单条缓存亲和性: affinity_key={self.affinity_key[:8]}..., "
                f"endpoint_id={self.endpoint_id[:8]}..., model_id={self.model_id[:8]}..."
            )

            context.add_audit_metadata(
                action="cache_clear_single",
                affinity_key=self.affinity_key,
                endpoint_id=self.endpoint_id,
                model_id=self.model_id,
            )
            return {
                "status": "ok",
                "message": f"已清除缓存亲和性: {api_key_name or self.affinity_key[:8]}",
                "affinity_key": self.affinity_key,
                "endpoint_id": self.endpoint_id,
                "model_id": self.model_id,
            }
        except HTTPException:
            raise
        except Exception as exc:
            logger.exception(f"清除单条缓存亲和性失败: {exc}")
            raise HTTPException(status_code=500, detail=f"清除失败: {exc}")


class AdminClearAllCacheAdapter(AdminApiAdapter):
    async def handle(self, context: ApiRequestContext) -> Dict[str, Any]:  # type: ignore[override]
        try:
            redis_client = get_redis_client_sync()
            affinity_mgr = await get_affinity_manager(redis_client)
            count = await affinity_mgr.clear_all()
            logger.warning(f"已清除所有缓存亲和性（管理员操作）: {count} 个")
            context.add_audit_metadata(
                action="cache_clear_all",
                cleared_count=count,
            )
            return {"status": "ok", "message": "已清除所有缓存亲和性", "count": count}
        except Exception as exc:
            logger.exception(f"清除所有缓存亲和性失败: {exc}")
            raise HTTPException(status_code=500, detail=f"清除失败: {exc}")


@dataclass
class AdminClearProviderCacheAdapter(AdminApiAdapter):
    provider_id: str

    async def handle(self, context: ApiRequestContext) -> Dict[str, Any]:  # type: ignore[override]
        try:
            redis_client = get_redis_client_sync()
            affinity_mgr = await get_affinity_manager(redis_client)
            count = await affinity_mgr.invalidate_all_for_provider(self.provider_id)
            logger.info(f"已清除Provider缓存亲和性: provider_id={self.provider_id[:8]}..., count={count}")
            context.add_audit_metadata(
                action="cache_clear_provider",
                provider_id=self.provider_id,
                cleared_count=count,
            )
            return {
                "status": "ok",
                "message": "已清除Provider的缓存亲和性",
                "provider_id": self.provider_id,
                "count": count,
            }
        except Exception as exc:
            logger.exception(f"清除Provider缓存亲和性失败: {exc}")
            raise HTTPException(status_code=500, detail=f"清除失败: {exc}")


class AdminCacheConfigAdapter(AdminApiAdapter):
    async def handle(self, context: ApiRequestContext) -> Dict[str, Any]:  # type: ignore[override]
        from src.services.cache.affinity_manager import CacheAffinityManager
        from src.config.constants import ConcurrencyDefaults
        from src.services.rate_limit.adaptive_reservation import get_adaptive_reservation_manager

        # 获取动态预留管理器的配置
        reservation_manager = get_adaptive_reservation_manager()
        reservation_stats = reservation_manager.get_stats()

        response = {
            "status": "ok",
            "data": {
                "cache_ttl_seconds": CacheAffinityManager.DEFAULT_CACHE_TTL,
                "cache_reservation_ratio": ConcurrencyDefaults.CACHE_RESERVATION_RATIO,
                "dynamic_reservation": {
                    "enabled": True,
                    "config": reservation_stats["config"],
                    "description": {
                        "probe_phase_requests": "探测阶段请求数阈值",
                        "probe_reservation": "探测阶段预留比例",
                        "stable_min_reservation": "稳定阶段最小预留比例",
                        "stable_max_reservation": "稳定阶段最大预留比例",
                        "low_load_threshold": "低负载阈值（低于此值使用最小预留）",
                        "high_load_threshold": "高负载阈值（高于此值根据置信度使用较高预留）",
                    },
                },
                "description": {
                    "cache_ttl": "缓存亲和性有效期（秒）",
                    "cache_reservation_ratio": "静态预留比例（已被动态预留替代）",
                    "dynamic_reservation": "动态预留机制配置",
                },
            },
        }
        context.add_audit_metadata(
            action="cache_config",
            cache_ttl_seconds=CacheAffinityManager.DEFAULT_CACHE_TTL,
            cache_reservation_ratio=ConcurrencyDefaults.CACHE_RESERVATION_RATIO,
            dynamic_reservation_enabled=True,
        )
        return response


# ==================== 模型映射缓存管理 ====================


@router.get("/model-mapping/stats")
async def get_model_mapping_cache_stats(
    request: Request,
    db: Session = Depends(get_db),
) -> Any:
    """
    获取模型映射缓存统计信息

    获取模型解析缓存的详细统计信息，包括各类型缓存键数量、
    映射关系列表、Provider 级别的模型映射缓存等。

    **返回字段**:
    - `status`: 状态（ok）
    - `data`: 统计数据
      - `available`: Redis 是否可用
      - `message`: 提示消息（当 Redis 未启用时）
      - `ttl_seconds`: 缓存 TTL（秒）
      - `total_keys`: 总缓存键数量
      - `breakdown`: 各类型缓存键数量分解
        - `model_by_id`: Model ID 缓存数量
        - `model_by_provider_global`: Provider-GlobalModel 缓存数量
        - `global_model_by_id`: GlobalModel ID 缓存数量
        - `global_model_by_name`: GlobalModel 名称缓存数量
        - `global_model_resolve`: GlobalModel 解析缓存数量
      - `mappings`: 模型映射列表（最多 100 条）
        - `mapping_name`: 映射名称（别名）
        - `global_model_name`: GlobalModel 名称
        - `global_model_display_name`: GlobalModel 显示名称
        - `providers`: 使用该映射的 Provider 列表
        - `ttl`: 缓存剩余 TTL（秒）
      - `provider_model_mappings`: Provider 级别的模型映射（最多 100 条）
        - `provider_id`: Provider ID
        - `provider_name`: Provider 名称
        - `global_model_id`: GlobalModel ID
        - `global_model_name`: GlobalModel 名称
        - `global_model_display_name`: GlobalModel 显示名称
        - `provider_model_name`: Provider 侧的模型名称
        - `aliases`: 别名列表
        - `ttl`: 缓存剩余 TTL（秒）
        - `hit_count`: 缓存命中次数
      - `unmapped`: 未映射或无效的缓存条目
    """
    adapter = AdminModelMappingCacheStatsAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.delete("/model-mapping")
async def clear_all_model_mapping_cache(
    request: Request,
    db: Session = Depends(get_db),
) -> Any:
    """
    清除所有模型映射缓存

    清除系统中所有的模型映射缓存，包括 Model、GlobalModel、
    模型解析等所有相关缓存。下次请求时将重新从数据库查询。

    **警告**: 此操作会影响所有模型解析，请谨慎使用

    **返回字段**:
    - `status`: 状态（ok）
    - `message`: 操作结果消息
    - `deleted_count`: 删除的缓存键数量
    """
    adapter = AdminClearAllModelMappingCacheAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.delete("/model-mapping/{model_name}")
async def clear_model_mapping_cache_by_name(
    model_name: str,
    request: Request,
    db: Session = Depends(get_db),
) -> Any:
    """
    清除指定模型名称的映射缓存

    根据模型名称清除相关的映射缓存，包括 resolve 缓存和 name 缓存。
    用于更新单个模型的配置后刷新缓存。

    **路径参数**:
    - `model_name`: 模型名称（可以是 GlobalModel.name 或映射名称）

    **返回字段**:
    - `status`: 状态（ok）
    - `message`: 操作结果消息
    - `model_name`: 模型名称
    - `deleted_keys`: 删除的缓存键列表
    """
    adapter = AdminClearModelMappingCacheByNameAdapter(model_name=model_name)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.delete("/model-mapping/provider/{provider_id}/{global_model_id}")
async def clear_provider_model_mapping_cache(
    provider_id: str,
    global_model_id: str,
    request: Request,
    db: Session = Depends(get_db),
) -> Any:
    """
    清除指定 Provider 和 GlobalModel 的模型映射缓存

    清除特定 Provider 和 GlobalModel 组合的映射缓存及其命中次数统计。
    用于 Provider 模型配置更新后刷新缓存。

    **路径参数**:
    - `provider_id`: Provider ID
    - `global_model_id`: GlobalModel ID

    **返回字段**:
    - `status`: 状态（ok）
    - `message`: 操作结果消息
    - `provider_id`: Provider ID
    - `global_model_id`: GlobalModel ID
    - `deleted_keys`: 删除的缓存键列表
    """
    adapter = AdminClearProviderModelMappingCacheAdapter(
        provider_id=provider_id, global_model_id=global_model_id
    )
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


class AdminModelMappingCacheStatsAdapter(AdminApiAdapter):
    async def handle(self, context: ApiRequestContext) -> Dict[str, Any]:  # type: ignore[override]
        import json

        from src.clients.redis_client import get_redis_client
        from src.config.constants import CacheTTL
        from src.models.database import GlobalModel, Model, Provider

        db = context.db

        try:
            redis = await get_redis_client(require_redis=False)
            if not redis:
                return {
                    "status": "ok",
                    "data": {
                        "available": False,
                        "message": "Redis 未启用，模型映射缓存不可用",
                    },
                }

            # 统计各类型缓存键数量
            model_id_keys = []
            global_model_id_keys = []
            global_model_name_keys = []
            global_model_resolve_keys = []
            provider_global_keys = []

            # 扫描所有模型相关的缓存键
            async for key in redis.scan_iter(match="model:*", count=100):
                key_str = key.decode() if isinstance(key, bytes) else key
                if key_str.startswith("model:id:"):
                    model_id_keys.append(key_str)
                elif key_str.startswith("model:provider_global:"):
                    # 过滤掉 hits 统计键，只保留实际的缓存键
                    if not key_str.startswith("model:provider_global:hits:"):
                        provider_global_keys.append(key_str)

            async for key in redis.scan_iter(match="global_model:*", count=100):
                key_str = key.decode() if isinstance(key, bytes) else key
                if key_str.startswith("global_model:id:"):
                    global_model_id_keys.append(key_str)
                elif key_str.startswith("global_model:name:"):
                    global_model_name_keys.append(key_str)
                elif key_str.startswith("global_model:resolve:"):
                    global_model_resolve_keys.append(key_str)

            total_keys = (
                len(model_id_keys)
                + len(global_model_id_keys)
                + len(global_model_name_keys)
                + len(global_model_resolve_keys)
                + len(provider_global_keys)
            )

            # 解析缓存内容，构建映射列表
            mappings = []
            unmapped_entries = []

            for key in global_model_resolve_keys[:100]:  # 最多处理 100 个
                mapping_name = key.replace("global_model:resolve:", "")
                try:
                    cached_value = await redis.get(key)
                    ttl = await redis.ttl(key)

                    if cached_value:
                        cached_str = (
                            cached_value.decode()
                            if isinstance(cached_value, bytes)
                            else cached_value
                        )

                        if cached_str == "NOT_FOUND":
                            unmapped_entries.append({
                                "mapping_name": mapping_name,
                                "status": "not_found",
                                "ttl": ttl if ttl > 0 else None,
                            })
                        else:
                            try:
                                cached_data = json.loads(cached_str)
                                global_model_id = cached_data.get("id")
                                global_model_name = cached_data.get("name")
                                global_model_display_name = cached_data.get("display_name")

                                # 跳过 mapping_name == global_model_name 的情况（直接匹配，不是映射）
                                if mapping_name == global_model_name:
                                    continue

                                # 查询哪些 Provider 配置了这个映射名称
                                provider_names = []
                                if global_model_id:
                                    models = (
                                        db.query(Model, Provider)
                                        .join(Provider, Model.provider_id == Provider.id)
                                        .filter(
                                            Model.global_model_id == global_model_id,
                                            Model.is_active,
                                            Provider.is_active,
                                        )
                                        .all()
                                    )
                                    # 只显示配置了该映射名称的 Provider
                                    for model, provider in models:
                                        # 检查是否是主模型名称
                                        if model.provider_model_name == mapping_name:
                                            provider_names.append(provider.name)
                                            continue
                                        # 检查是否在映射列表中
                                        if model.provider_model_mappings:
                                            mapping_list = [
                                                a.get("name")
                                                for a in model.provider_model_mappings
                                                if isinstance(a, dict)
                                            ]
                                            if mapping_name in mapping_list:
                                                provider_names.append(provider.name)
                                    provider_names = sorted(list(set(provider_names)))

                                mappings.append({
                                    "mapping_name": mapping_name,
                                    "global_model_name": global_model_name,
                                    "global_model_display_name": global_model_display_name,
                                    "providers": provider_names,
                                    "ttl": ttl if ttl > 0 else None,
                                })

                            except json.JSONDecodeError:
                                unmapped_entries.append({
                                    "mapping_name": mapping_name,
                                    "status": "invalid",
                                    "ttl": ttl if ttl > 0 else None,
                                })
                except Exception as e:
                    logger.warning(f"解析缓存键 {key} 失败: {e}")
                    unmapped_entries.append({
                        "mapping_name": mapping_name,
                        "status": "error",
                        "ttl": None,
                    })

            # 按 mapping_name 排序
            mappings.sort(key=lambda x: x["mapping_name"])

            # 3. 解析 provider_global 缓存（Provider 级别的模型解析缓存）
            provider_model_mappings = []
            # 预加载 Provider 和 GlobalModel 数据
            provider_map = {str(p.id): p for p in db.query(Provider).filter(Provider.is_active.is_(True)).all()}
            global_model_map = {str(gm.id): gm for gm in db.query(GlobalModel).filter(GlobalModel.is_active.is_(True)).all()}

            for key in provider_global_keys[:100]:  # 最多处理 100 个
                # key 格式: model:provider_global:{provider_id}:{global_model_id}
                try:
                    parts = key.replace("model:provider_global:", "").split(":")
                    if len(parts) != 2:
                        continue
                    provider_id, global_model_id = parts

                    cached_value = await redis.get(key)
                    ttl = await redis.ttl(key)

                    # 获取命中次数
                    hit_count_key = f"model:provider_global:hits:{provider_id}:{global_model_id}"
                    hit_count_raw = await redis.get(hit_count_key)
                    hit_count = int(hit_count_raw) if hit_count_raw else 0

                    if cached_value:
                        cached_str = (
                            cached_value.decode()
                            if isinstance(cached_value, bytes)
                            else cached_value
                        )
                        try:
                            cached_data = json.loads(cached_str)
                            provider_model_name = cached_data.get("provider_model_name")
                            cached_model_mappings = cached_data.get("provider_model_mappings", [])

                            # 获取 Provider 和 GlobalModel 信息
                            provider = provider_map.get(provider_id)
                            global_model = global_model_map.get(global_model_id)

                            if provider and global_model:
                                # 提取映射名称
                                mapping_names = []
                                if cached_model_mappings:
                                    for mapping_entry in cached_model_mappings:
                                        if isinstance(mapping_entry, dict) and mapping_entry.get("name"):
                                            mapping_names.append(mapping_entry["name"])

                                # provider_model_name 为空时跳过
                                if not provider_model_name:
                                    continue

                                # 只显示有实际映射的条目：
                                # 1. 全局模型名 != Provider 模型名（模型名称映射）
                                # 2. 或者有映射配置
                                has_name_mapping = global_model.name != provider_model_name
                                has_mappings = len(mapping_names) > 0

                                if has_name_mapping or has_mappings:
                                    # 构建用于展示的映射列表
                                    # 如果只有名称映射没有额外映射，则用 global_model_name 作为"请求名称"
                                    display_mappings = mapping_names if mapping_names else [global_model.name]

                                    provider_model_mappings.append({
                                        "provider_id": provider_id,
                                        "provider_name": provider.name,
                                        "global_model_id": global_model_id,
                                        "global_model_name": global_model.name,
                                        "global_model_display_name": global_model.display_name,
                                        "provider_model_name": provider_model_name,
                                        "aliases": display_mappings,
                                        "ttl": ttl if ttl > 0 else None,
                                        "hit_count": hit_count,
                                    })
                        except json.JSONDecodeError:
                            pass
                except Exception as e:
                    logger.warning(f"解析 provider_global 缓存键 {key} 失败: {e}")

            # 按 provider_name + global_model_name 排序
            provider_model_mappings.sort(key=lambda x: (x["provider_name"], x["global_model_name"]))

            response_data = {
                "available": True,
                "ttl_seconds": CacheTTL.MODEL,
                "total_keys": total_keys,
                "breakdown": {
                    "model_by_id": len(model_id_keys),
                    "model_by_provider_global": len(provider_global_keys),
                    "global_model_by_id": len(global_model_id_keys),
                    "global_model_by_name": len(global_model_name_keys),
                    "global_model_resolve": len(global_model_resolve_keys),
                },
                "mappings": mappings,
                "provider_model_mappings": provider_model_mappings if provider_model_mappings else None,
                "unmapped": unmapped_entries if unmapped_entries else None,
            }

            context.add_audit_metadata(
                action="model_mapping_cache_stats",
                total_keys=total_keys,
            )
            return {"status": "ok", "data": response_data}

        except Exception as exc:
            logger.exception(f"获取模型映射缓存统计失败: {exc}")
            raise HTTPException(status_code=500, detail=f"获取统计失败: {exc}")


class AdminClearAllModelMappingCacheAdapter(AdminApiAdapter):
    async def handle(self, context: ApiRequestContext) -> Dict[str, Any]:  # type: ignore[override]
        from src.clients.redis_client import get_redis_client

        try:
            redis = await get_redis_client(require_redis=False)
            if not redis:
                raise HTTPException(status_code=503, detail="Redis 未启用")

            deleted_count = 0

            # 删除所有模型相关的缓存键
            keys_to_delete = []
            async for key in redis.scan_iter(match="model:*", count=100):
                keys_to_delete.append(key)
            async for key in redis.scan_iter(match="global_model:*", count=100):
                keys_to_delete.append(key)

            if keys_to_delete:
                deleted_count = await redis.delete(*keys_to_delete)

            logger.warning(f"已清除所有模型映射缓存（管理员操作）: {deleted_count} 个键")
            context.add_audit_metadata(
                action="model_mapping_cache_clear_all",
                deleted_count=deleted_count,
            )
            return {
                "status": "ok",
                "message": f"已清除所有模型映射缓存",
                "deleted_count": deleted_count,
            }

        except HTTPException:
            raise
        except Exception as exc:
            logger.exception(f"清除模型映射缓存失败: {exc}")
            raise HTTPException(status_code=500, detail=f"清除失败: {exc}")


@dataclass
class AdminClearModelMappingCacheByNameAdapter(AdminApiAdapter):
    model_name: str

    async def handle(self, context: ApiRequestContext) -> Dict[str, Any]:  # type: ignore[override]
        from src.clients.redis_client import get_redis_client

        try:
            redis = await get_redis_client(require_redis=False)
            if not redis:
                raise HTTPException(status_code=503, detail="Redis 未启用")

            deleted_keys = []

            # 清除 resolve 缓存
            resolve_key = f"global_model:resolve:{self.model_name}"
            if await redis.exists(resolve_key):
                await redis.delete(resolve_key)
                deleted_keys.append(resolve_key)

            # 清除 name 缓存
            name_key = f"global_model:name:{self.model_name}"
            if await redis.exists(name_key):
                await redis.delete(name_key)
                deleted_keys.append(name_key)

            logger.info(f"已清除模型映射缓存: model_name={self.model_name}, 删除键={deleted_keys}")
            context.add_audit_metadata(
                action="model_mapping_cache_clear_by_name",
                model_name=self.model_name,
                deleted_keys=deleted_keys,
            )
            return {
                "status": "ok",
                "message": f"已清除模型 {self.model_name} 的映射缓存",
                "model_name": self.model_name,
                "deleted_keys": deleted_keys,
            }

        except HTTPException:
            raise
        except Exception as exc:
            logger.exception(f"清除模型映射缓存失败: {exc}")
            raise HTTPException(status_code=500, detail=f"清除失败: {exc}")


@dataclass
class AdminClearProviderModelMappingCacheAdapter(AdminApiAdapter):
    provider_id: str
    global_model_id: str

    async def handle(self, context: ApiRequestContext) -> Dict[str, Any]:  # type: ignore[override]
        from src.clients.redis_client import get_redis_client

        try:
            redis = await get_redis_client(require_redis=False)
            if not redis:
                raise HTTPException(status_code=503, detail="Redis 未启用")

            deleted_keys = []

            # 清除 provider_global 缓存
            provider_global_key = f"model:provider_global:{self.provider_id}:{self.global_model_id}"
            if await redis.exists(provider_global_key):
                await redis.delete(provider_global_key)
                deleted_keys.append(provider_global_key)

            # 清除对应的 hit_count 缓存
            hit_count_key = f"model:provider_global:hits:{self.provider_id}:{self.global_model_id}"
            if await redis.exists(hit_count_key):
                await redis.delete(hit_count_key)
                deleted_keys.append(hit_count_key)

            logger.info(
                f"已清除 Provider 模型映射缓存: provider_id={self.provider_id[:8]}..., "
                f"global_model_id={self.global_model_id[:8]}..., 删除键={deleted_keys}"
            )
            context.add_audit_metadata(
                action="provider_model_mapping_cache_clear",
                provider_id=self.provider_id,
                global_model_id=self.global_model_id,
                deleted_keys=deleted_keys,
            )
            return {
                "status": "ok",
                "message": "已清除 Provider 模型映射缓存",
                "provider_id": self.provider_id,
                "global_model_id": self.global_model_id,
                "deleted_keys": deleted_keys,
            }

        except HTTPException:
            raise
        except Exception as exc:
            logger.exception(f"清除 Provider 模型映射缓存失败: {exc}")
            raise HTTPException(status_code=500, detail=f"清除失败: {exc}")
