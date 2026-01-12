"""
用量统计和配额管理服务
"""

import uuid
from dataclasses import dataclass
from datetime import datetime, timedelta, timezone
from typing import Any, Dict, List, Optional, Tuple

from sqlalchemy import func
from sqlalchemy.orm import Session

from src.core.logger import logger
from src.models.database import ApiKey, Provider, ProviderAPIKey, Usage, User, UserRole
from src.services.model.cost import ModelCostService
from src.services.system.config import SystemConfigService


@dataclass
class UsageRecordParams:
    """用量记录参数数据类，用于在内部方法间传递数据"""
    db: Session
    user: Optional[User]
    api_key: Optional[ApiKey]
    provider: str
    model: str
    input_tokens: int
    output_tokens: int
    cache_creation_input_tokens: int
    cache_read_input_tokens: int
    request_type: str
    api_format: Optional[str]
    is_stream: bool
    response_time_ms: Optional[int]
    first_byte_time_ms: Optional[int]
    status_code: int
    error_message: Optional[str]
    metadata: Optional[Dict[str, Any]]
    request_headers: Optional[Dict[str, Any]]
    request_body: Optional[Any]
    provider_request_headers: Optional[Dict[str, Any]]
    response_headers: Optional[Dict[str, Any]]
    client_response_headers: Optional[Dict[str, Any]]
    response_body: Optional[Any]
    request_id: str
    provider_id: Optional[str]
    provider_endpoint_id: Optional[str]
    provider_api_key_id: Optional[str]
    status: str
    cache_ttl_minutes: Optional[int]
    use_tiered_pricing: bool
    target_model: Optional[str]

    def __post_init__(self) -> None:
        """验证关键字段，确保数据完整性"""
        # Token 数量不能为负数
        if self.input_tokens < 0:
            raise ValueError(f"input_tokens 不能为负数: {self.input_tokens}")
        if self.output_tokens < 0:
            raise ValueError(f"output_tokens 不能为负数: {self.output_tokens}")
        if self.cache_creation_input_tokens < 0:
            raise ValueError(
                f"cache_creation_input_tokens 不能为负数: {self.cache_creation_input_tokens}"
            )
        if self.cache_read_input_tokens < 0:
            raise ValueError(
                f"cache_read_input_tokens 不能为负数: {self.cache_read_input_tokens}"
            )

        # 响应时间不能为负数
        if self.response_time_ms is not None and self.response_time_ms < 0:
            raise ValueError(f"response_time_ms 不能为负数: {self.response_time_ms}")
        if self.first_byte_time_ms is not None and self.first_byte_time_ms < 0:
            raise ValueError(f"first_byte_time_ms 不能为负数: {self.first_byte_time_ms}")

        # HTTP 状态码范围校验
        if not (100 <= self.status_code <= 599):
            raise ValueError(f"无效的 HTTP 状态码: {self.status_code}")

        # 状态值校验
        valid_statuses = {"pending", "streaming", "completed", "failed"}
        if self.status not in valid_statuses:
            raise ValueError(f"无效的状态值: {self.status}，有效值: {valid_statuses}")


class UsageService:
    """用量统计服务"""

    # ==================== 缓存键常量 ====================

    # 热力图缓存键前缀（依赖 TTL 自动过期，用户角色变更时主动清除）
    HEATMAP_CACHE_KEY_PREFIX = "activity_heatmap"

    # ==================== 热力图缓存 ====================

    @classmethod
    def _get_heatmap_cache_key(cls, user_id: Optional[str], include_actual_cost: bool) -> str:
        """生成热力图缓存键"""
        cost_suffix = "with_cost" if include_actual_cost else "no_cost"
        if user_id:
            return f"{cls.HEATMAP_CACHE_KEY_PREFIX}:user:{user_id}:{cost_suffix}"
        else:
            return f"{cls.HEATMAP_CACHE_KEY_PREFIX}:admin:all:{cost_suffix}"

    @classmethod
    async def clear_user_heatmap_cache(cls, user_id: str) -> None:
        """
        清除用户的热力图缓存（用户角色变更时调用）

        Args:
            user_id: 用户ID
        """
        from src.clients.redis_client import get_redis_client

        redis_client = await get_redis_client(require_redis=False)
        if not redis_client:
            return

        # 清除该用户的所有热力图缓存（with_cost 和 no_cost）
        keys_to_delete = [
            cls._get_heatmap_cache_key(user_id, include_actual_cost=True),
            cls._get_heatmap_cache_key(user_id, include_actual_cost=False),
        ]

        for key in keys_to_delete:
            try:
                await redis_client.delete(key)
                logger.debug(f"已清除热力图缓存: {key}")
            except Exception as e:
                logger.warning(f"清除热力图缓存失败: {key}, error={e}")

    @classmethod
    async def get_cached_heatmap(
        cls,
        db: Session,
        user_id: Optional[str] = None,
        include_actual_cost: bool = False,
    ) -> Dict[str, Any]:
        """
        获取带缓存的热力图数据

        缓存策略：
        - TTL: 5分钟（CacheTTL.ACTIVITY_HEATMAP）
        - 仅依赖 TTL 自动过期，新使用记录最多延迟 5 分钟出现
        - 用户角色变更时通过 clear_user_heatmap_cache() 主动清除

        Args:
            db: 数据库会话
            user_id: 用户ID，None 表示获取全局热力图（管理员）
            include_actual_cost: 是否包含实际成本

        Returns:
            热力图数据字典
        """
        from src.clients.redis_client import get_redis_client
        from src.config.constants import CacheTTL
        import json

        cache_key = cls._get_heatmap_cache_key(user_id, include_actual_cost)

        cache_ttl = CacheTTL.ACTIVITY_HEATMAP
        redis_client = await get_redis_client(require_redis=False)

        # 尝试从缓存获取
        if redis_client:
            try:
                cached = await redis_client.get(cache_key)
                if cached:
                    try:
                        return json.loads(cached)  # type: ignore[no-any-return]
                    except json.JSONDecodeError as e:
                        logger.warning(f"热力图缓存解析失败，删除损坏缓存: {cache_key}, error={e}")
                        try:
                            await redis_client.delete(cache_key)
                        except Exception:
                            pass
            except Exception as e:
                logger.error(f"读取热力图缓存出错: {cache_key}, error={e}")

        # 从数据库查询
        result = cls.get_daily_activity(
            db=db,
            user_id=user_id,
            window_days=365,
            include_actual_cost=include_actual_cost,
        )

        # 保存到缓存（失败不影响返回结果）
        if redis_client:
            try:
                await redis_client.setex(
                    cache_key,
                    cache_ttl,
                    json.dumps(result, ensure_ascii=False, default=str),
                )
            except Exception as e:
                logger.warning(f"保存热力图缓存失败: {cache_key}, error={e}")

        return result

    # ==================== 内部数据类 ====================

    @staticmethod
    def _build_usage_params(
        *,
        db: Session,
        user: Optional[User],
        api_key: Optional[ApiKey],
        provider: str,
        model: str,
        input_tokens: int,
        output_tokens: int,
        cache_creation_input_tokens: int,
        cache_read_input_tokens: int,
        request_type: str,
        api_format: Optional[str],
        is_stream: bool,
        response_time_ms: Optional[int],
        first_byte_time_ms: Optional[int],
        status_code: int,
        error_message: Optional[str],
        metadata: Optional[Dict[str, Any]],
        request_headers: Optional[Dict[str, Any]],
        request_body: Optional[Any],
        provider_request_headers: Optional[Dict[str, Any]],
        response_headers: Optional[Dict[str, Any]],
        client_response_headers: Optional[Dict[str, Any]],
        response_body: Optional[Any],
        request_id: str,
        provider_id: Optional[str],
        provider_endpoint_id: Optional[str],
        provider_api_key_id: Optional[str],
        status: str,
        target_model: Optional[str],
        # 成本计算结果
        input_cost: float,
        output_cost: float,
        cache_creation_cost: float,
        cache_read_cost: float,
        cache_cost: float,
        request_cost: float,
        total_cost: float,
        # 价格信息
        input_price: float,
        output_price: float,
        cache_creation_price: Optional[float],
        cache_read_price: Optional[float],
        request_price: Optional[float],
        # 倍率
        actual_rate_multiplier: float,
        is_free_tier: bool,
    ) -> Dict[str, Any]:
        """构建 Usage 记录的参数字典（内部方法，避免代码重复）"""

        # 根据配置决定是否记录请求详情
        should_log_headers = SystemConfigService.should_log_headers(db)
        should_log_body = SystemConfigService.should_log_body(db)

        # 处理请求头（可能需要脱敏）
        processed_request_headers = None
        if should_log_headers and request_headers:
            processed_request_headers = SystemConfigService.mask_sensitive_headers(
                db, request_headers
            )

        # 处理提供商请求头（可能需要脱敏）
        processed_provider_request_headers = None
        if should_log_headers and provider_request_headers:
            processed_provider_request_headers = SystemConfigService.mask_sensitive_headers(
                db, provider_request_headers
            )

        # 处理请求体和响应体（可能需要截断）
        processed_request_body = None
        processed_response_body = None
        if should_log_body:
            if request_body:
                processed_request_body = SystemConfigService.truncate_body(
                    db, request_body, is_request=True
                )
            if response_body:
                processed_response_body = SystemConfigService.truncate_body(
                    db, response_body, is_request=False
                )

        # 处理响应头
        processed_response_headers = None
        if should_log_headers and response_headers:
            processed_response_headers = SystemConfigService.mask_sensitive_headers(
                db, response_headers
            )

        # 处理返回给客户端的响应头
        processed_client_response_headers = None
        if should_log_headers and client_response_headers:
            processed_client_response_headers = SystemConfigService.mask_sensitive_headers(
                db, client_response_headers
            )

        # 计算真实成本（表面成本 * 倍率），免费套餐实际费用为 0
        if is_free_tier:
            actual_input_cost = 0.0
            actual_output_cost = 0.0
            actual_cache_creation_cost = 0.0
            actual_cache_read_cost = 0.0
            actual_request_cost = 0.0
            actual_total_cost = 0.0
        else:
            actual_input_cost = input_cost * actual_rate_multiplier
            actual_output_cost = output_cost * actual_rate_multiplier
            actual_cache_creation_cost = cache_creation_cost * actual_rate_multiplier
            actual_cache_read_cost = cache_read_cost * actual_rate_multiplier
            actual_request_cost = request_cost * actual_rate_multiplier
            actual_total_cost = total_cost * actual_rate_multiplier

        return {
            "user_id": user.id if user else None,
            "api_key_id": api_key.id if api_key else None,
            "request_id": request_id,
            "provider_name": provider,
            "model": model,
            "target_model": target_model,
            "provider_id": provider_id,
            "provider_endpoint_id": provider_endpoint_id,
            "provider_api_key_id": provider_api_key_id,
            "input_tokens": input_tokens,
            "output_tokens": output_tokens,
            "total_tokens": input_tokens + output_tokens,
            "cache_creation_input_tokens": cache_creation_input_tokens,
            "cache_read_input_tokens": cache_read_input_tokens,
            "input_cost_usd": input_cost,
            "output_cost_usd": output_cost,
            "cache_cost_usd": cache_cost,
            "cache_creation_cost_usd": cache_creation_cost,
            "cache_read_cost_usd": cache_read_cost,
            "request_cost_usd": request_cost,
            "total_cost_usd": total_cost,
            "actual_input_cost_usd": actual_input_cost,
            "actual_output_cost_usd": actual_output_cost,
            "actual_cache_creation_cost_usd": actual_cache_creation_cost,
            "actual_cache_read_cost_usd": actual_cache_read_cost,
            "actual_request_cost_usd": actual_request_cost,
            "actual_total_cost_usd": actual_total_cost,
            "rate_multiplier": actual_rate_multiplier,
            "input_price_per_1m": input_price,
            "output_price_per_1m": output_price,
            "cache_creation_price_per_1m": cache_creation_price,
            "cache_read_price_per_1m": cache_read_price,
            "price_per_request": request_price,
            "request_type": request_type,
            "api_format": api_format,
            "is_stream": is_stream,
            "status_code": status_code,
            "error_message": error_message,
            "response_time_ms": response_time_ms,
            "first_byte_time_ms": first_byte_time_ms,
            "status": status,
            "request_metadata": metadata,
            "request_headers": processed_request_headers,
            "request_body": processed_request_body,
            "provider_request_headers": processed_provider_request_headers,
            "response_headers": processed_response_headers,
            "client_response_headers": processed_client_response_headers,
            "response_body": processed_response_body,
        }

    @classmethod
    async def _get_rate_multiplier_and_free_tier(
        cls,
        db: Session,
        provider_api_key_id: Optional[str],
        provider_id: Optional[str],
        api_format: Optional[str] = None,
    ) -> Tuple[float, bool]:
        """获取费率倍数和是否免费套餐（使用缓存）"""
        from src.services.cache.provider_cache import ProviderCacheService

        return await ProviderCacheService.get_rate_multiplier_and_free_tier(
            db, provider_api_key_id, provider_id, api_format
        )

    @classmethod
    async def _calculate_costs(
        cls,
        db: Session,
        provider: str,
        model: str,
        input_tokens: int,
        output_tokens: int,
        cache_creation_input_tokens: int,
        cache_read_input_tokens: int,
        api_format: Optional[str],
        cache_ttl_minutes: Optional[int],
        use_tiered_pricing: bool,
        is_failed_request: bool,
    ) -> Tuple[float, float, float, float, float, float, float, float, float,
               Optional[float], Optional[float], Optional[float], Optional[int]]:
        """计算所有成本相关数据

        Returns:
            (input_price, output_price, cache_creation_price, cache_read_price, request_price,
             input_cost, output_cost, cache_creation_cost, cache_read_cost, cache_cost,
             request_cost, total_cost, tier_index)
        """
        # 获取模型价格信息
        input_price, output_price = await cls.get_model_price_async(db, provider, model)
        cache_creation_price, cache_read_price = await cls.get_cache_prices_async(
            db, provider, model, input_price
        )
        request_price = await cls.get_request_price_async(db, provider, model)
        effective_request_price = None if is_failed_request else request_price

        # 初始化成本变量
        input_cost = 0.0
        output_cost = 0.0
        cache_creation_cost = 0.0
        cache_read_cost = 0.0
        cache_cost = 0.0
        request_cost = 0.0
        total_cost = 0.0
        tier_index = None

        if use_tiered_pricing:
            (
                input_cost,
                output_cost,
                cache_creation_cost,
                cache_read_cost,
                cache_cost,
                request_cost,
                total_cost,
                tier_index,
            ) = await cls.calculate_cost_with_strategy_async(
                db=db,
                provider=provider,
                model=model,
                input_tokens=input_tokens,
                output_tokens=output_tokens,
                cache_creation_input_tokens=cache_creation_input_tokens,
                cache_read_input_tokens=cache_read_input_tokens,
                api_format=api_format,
                cache_ttl_minutes=cache_ttl_minutes,
            )
            if is_failed_request:
                total_cost = total_cost - request_cost
                request_cost = 0.0
        else:
            (
                input_cost,
                output_cost,
                cache_creation_cost,
                cache_read_cost,
                cache_cost,
                request_cost,
                total_cost,
            ) = cls.calculate_cost(
                input_tokens=input_tokens,
                output_tokens=output_tokens,
                input_price_per_1m=input_price,
                output_price_per_1m=output_price,
                cache_creation_input_tokens=cache_creation_input_tokens,
                cache_read_input_tokens=cache_read_input_tokens,
                cache_creation_price_per_1m=cache_creation_price,
                cache_read_price_per_1m=cache_read_price,
                price_per_request=effective_request_price,
            )

        return (
            input_price, output_price, cache_creation_price, cache_read_price, request_price,
            input_cost, output_cost, cache_creation_cost, cache_read_cost, cache_cost,
            request_cost, total_cost, tier_index
        )

    @staticmethod
    def _update_existing_usage(
        existing_usage: Usage,
        usage_params: Dict[str, Any],
        target_model: Optional[str],
    ) -> None:
        """更新已存在的 Usage 记录（内部方法）"""
        # 更新关键字段
        existing_usage.provider_name = usage_params["provider_name"]
        existing_usage.status = usage_params["status"]
        existing_usage.status_code = usage_params["status_code"]
        existing_usage.error_message = usage_params["error_message"]
        existing_usage.response_time_ms = usage_params["response_time_ms"]
        existing_usage.first_byte_time_ms = usage_params["first_byte_time_ms"]

        # 更新请求头和请求体（如果有新值）
        if usage_params["request_headers"] is not None:
            existing_usage.request_headers = usage_params["request_headers"]
        if usage_params["request_body"] is not None:
            existing_usage.request_body = usage_params["request_body"]
        if usage_params["provider_request_headers"] is not None:
            existing_usage.provider_request_headers = usage_params["provider_request_headers"]
        existing_usage.response_body = usage_params["response_body"]
        existing_usage.response_headers = usage_params["response_headers"]
        existing_usage.client_response_headers = usage_params["client_response_headers"]

        # 更新 token 和费用信息
        existing_usage.input_tokens = usage_params["input_tokens"]
        existing_usage.output_tokens = usage_params["output_tokens"]
        existing_usage.total_tokens = usage_params["total_tokens"]
        existing_usage.cache_creation_input_tokens = usage_params["cache_creation_input_tokens"]
        existing_usage.cache_read_input_tokens = usage_params["cache_read_input_tokens"]
        existing_usage.input_cost_usd = usage_params["input_cost_usd"]
        existing_usage.output_cost_usd = usage_params["output_cost_usd"]
        existing_usage.cache_cost_usd = usage_params["cache_cost_usd"]
        existing_usage.cache_creation_cost_usd = usage_params["cache_creation_cost_usd"]
        existing_usage.cache_read_cost_usd = usage_params["cache_read_cost_usd"]
        existing_usage.request_cost_usd = usage_params["request_cost_usd"]
        existing_usage.total_cost_usd = usage_params["total_cost_usd"]
        existing_usage.actual_input_cost_usd = usage_params["actual_input_cost_usd"]
        existing_usage.actual_output_cost_usd = usage_params["actual_output_cost_usd"]
        existing_usage.actual_cache_creation_cost_usd = usage_params["actual_cache_creation_cost_usd"]
        existing_usage.actual_cache_read_cost_usd = usage_params["actual_cache_read_cost_usd"]
        existing_usage.actual_request_cost_usd = usage_params["actual_request_cost_usd"]
        existing_usage.actual_total_cost_usd = usage_params["actual_total_cost_usd"]
        existing_usage.rate_multiplier = usage_params["rate_multiplier"]

        # 更新 Provider 侧追踪信息
        existing_usage.provider_id = usage_params["provider_id"]
        existing_usage.provider_endpoint_id = usage_params["provider_endpoint_id"]
        existing_usage.provider_api_key_id = usage_params["provider_api_key_id"]

        # 更新模型映射信息
        if target_model is not None:
            existing_usage.target_model = target_model

    # ==================== 公开 API ====================

    @classmethod
    async def get_model_price_async(
        cls, db: Session, provider: str, model: str
    ) -> tuple[float, float]:
        """异步获取模型价格（输入价格，输出价格）每1M tokens

        查找逻辑：
        1. 直接通过 GlobalModel.name 匹配
        2. 查找该 Provider 的 Model 实现并获取价格
        3. 如果找不到则使用系统默认价格
        """

        service = ModelCostService(db)
        return await service.get_model_price_async(provider, model)

    @classmethod
    def get_model_price(cls, db: Session, provider: str, model: str) -> tuple[float, float]:
        """获取模型价格（输入价格，输出价格）每1M tokens

        查找逻辑：
        1. 直接通过 GlobalModel.name 匹配
        2. 查找该 Provider 的 Model 实现并获取价格
        3. 如果找不到则使用系统默认价格
        """

        service = ModelCostService(db)
        return service.get_model_price(provider, model)

    @classmethod
    async def get_cache_prices_async(
        cls, db: Session, provider: str, model: str, input_price: float
    ) -> Tuple[Optional[float], Optional[float]]:
        """异步获取模型缓存价格（缓存创建价格，缓存读取价格）每1M tokens"""
        service = ModelCostService(db)
        return await service.get_cache_prices_async(provider, model, input_price)

    @classmethod
    def get_cache_prices(
        cls, db: Session, provider: str, model: str, input_price: float
    ) -> Tuple[Optional[float], Optional[float]]:
        """获取模型缓存价格（缓存创建价格，缓存读取价格）每1M tokens"""
        service = ModelCostService(db)
        return service.get_cache_prices(provider, model, input_price)

    @classmethod
    async def get_request_price_async(
        cls, db: Session, provider: str, model: str
    ) -> Optional[float]:
        """异步获取模型按次计费价格"""
        service = ModelCostService(db)
        return await service.get_request_price_async(provider, model)

    @classmethod
    def get_request_price(cls, db: Session, provider: str, model: str) -> Optional[float]:
        """获取模型按次计费价格"""
        service = ModelCostService(db)
        return service.get_request_price(provider, model)

    @staticmethod
    def calculate_cost(
        input_tokens: int,
        output_tokens: int,
        input_price_per_1m: float,
        output_price_per_1m: float,
        cache_creation_input_tokens: int = 0,
        cache_read_input_tokens: int = 0,
        cache_creation_price_per_1m: Optional[float] = None,
        cache_read_price_per_1m: Optional[float] = None,
        price_per_request: Optional[float] = None,
    ) -> tuple[float, float, float, float, float, float, float]:
        """计算成本（价格是每百万tokens）- 固定价格模式

        Returns:
            Tuple of (input_cost, output_cost, cache_creation_cost,
                     cache_read_cost, cache_cost, request_cost, total_cost)
        """
        return ModelCostService.compute_cost(
            input_tokens=input_tokens,
            output_tokens=output_tokens,
            input_price_per_1m=input_price_per_1m,
            output_price_per_1m=output_price_per_1m,
            cache_creation_input_tokens=cache_creation_input_tokens,
            cache_read_input_tokens=cache_read_input_tokens,
            cache_creation_price_per_1m=cache_creation_price_per_1m,
            cache_read_price_per_1m=cache_read_price_per_1m,
            price_per_request=price_per_request,
        )

    @classmethod
    async def calculate_cost_with_strategy_async(
        cls,
        db: Session,
        provider: str,
        model: str,
        input_tokens: int,
        output_tokens: int,
        cache_creation_input_tokens: int = 0,
        cache_read_input_tokens: int = 0,
        api_format: Optional[str] = None,
        cache_ttl_minutes: Optional[int] = None,
    ) -> tuple[float, float, float, float, float, float, float, Optional[int]]:
        """使用策略模式计算成本（支持阶梯计费）

        根据 api_format 选择对应的计费策略，支持阶梯计费和 TTL 差异化。

        Returns:
            Tuple of (input_cost, output_cost, cache_creation_cost,
                     cache_read_cost, cache_cost, request_cost, total_cost, tier_index)
        """
        service = ModelCostService(db)
        return await service.compute_cost_with_strategy_async(
            provider=provider,
            model=model,
            input_tokens=input_tokens,
            output_tokens=output_tokens,
            cache_creation_input_tokens=cache_creation_input_tokens,
            cache_read_input_tokens=cache_read_input_tokens,
            api_format=api_format,
            cache_ttl_minutes=cache_ttl_minutes,
        )

    @classmethod
    async def _prepare_usage_record(
        cls,
        params: UsageRecordParams,
    ) -> Tuple[Dict[str, Any], float]:
        """准备用量记录的共享逻辑

        此方法提取了 record_usage 和 record_usage_async 的公共处理逻辑：
        - 获取费率倍数
        - 计算成本
        - 构建 Usage 参数

        Args:
            params: 用量记录参数数据类

        Returns:
            (usage_params 字典, total_cost 总成本)
        """
        # 获取费率倍数和是否免费套餐（传递 api_format 支持按格式配置的倍率）
        actual_rate_multiplier, is_free_tier = await cls._get_rate_multiplier_and_free_tier(
            params.db, params.provider_api_key_id, params.provider_id, params.api_format
        )

        # 计算成本
        is_failed_request = params.status_code >= 400 or params.error_message is not None
        (
            input_price, output_price, cache_creation_price, cache_read_price, request_price,
            input_cost, output_cost, cache_creation_cost, cache_read_cost, cache_cost,
            request_cost, total_cost, _tier_index
        ) = await cls._calculate_costs(
            db=params.db,
            provider=params.provider,
            model=params.model,
            input_tokens=params.input_tokens,
            output_tokens=params.output_tokens,
            cache_creation_input_tokens=params.cache_creation_input_tokens,
            cache_read_input_tokens=params.cache_read_input_tokens,
            api_format=params.api_format,
            cache_ttl_minutes=params.cache_ttl_minutes,
            use_tiered_pricing=params.use_tiered_pricing,
            is_failed_request=is_failed_request,
        )

        # 构建 Usage 参数
        usage_params = cls._build_usage_params(
            db=params.db,
            user=params.user,
            api_key=params.api_key,
            provider=params.provider,
            model=params.model,
            input_tokens=params.input_tokens,
            output_tokens=params.output_tokens,
            cache_creation_input_tokens=params.cache_creation_input_tokens,
            cache_read_input_tokens=params.cache_read_input_tokens,
            request_type=params.request_type,
            api_format=params.api_format,
            is_stream=params.is_stream,
            response_time_ms=params.response_time_ms,
            first_byte_time_ms=params.first_byte_time_ms,
            status_code=params.status_code,
            error_message=params.error_message,
            metadata=params.metadata,
            request_headers=params.request_headers,
            request_body=params.request_body,
            provider_request_headers=params.provider_request_headers,
            response_headers=params.response_headers,
            client_response_headers=params.client_response_headers,
            response_body=params.response_body,
            request_id=params.request_id,
            provider_id=params.provider_id,
            provider_endpoint_id=params.provider_endpoint_id,
            provider_api_key_id=params.provider_api_key_id,
            status=params.status,
            target_model=params.target_model,
            input_cost=input_cost,
            output_cost=output_cost,
            cache_creation_cost=cache_creation_cost,
            cache_read_cost=cache_read_cost,
            cache_cost=cache_cost,
            request_cost=request_cost,
            total_cost=total_cost,
            input_price=input_price,
            output_price=output_price,
            cache_creation_price=cache_creation_price,
            cache_read_price=cache_read_price,
            request_price=request_price,
            actual_rate_multiplier=actual_rate_multiplier,
            is_free_tier=is_free_tier,
        )

        return usage_params, total_cost

    @classmethod
    async def record_usage_async(
        cls,
        db: Session,
        user: Optional[User],
        api_key: Optional[ApiKey],
        provider: str,
        model: str,
        input_tokens: int,
        output_tokens: int,
        cache_creation_input_tokens: int = 0,
        cache_read_input_tokens: int = 0,
        request_type: str = "chat",
        api_format: Optional[str] = None,
        is_stream: bool = False,
        response_time_ms: Optional[int] = None,
        first_byte_time_ms: Optional[int] = None,
        status_code: int = 200,
        error_message: Optional[str] = None,
        metadata: Optional[Dict[str, Any]] = None,
        request_headers: Optional[Dict[str, Any]] = None,
        request_body: Optional[Any] = None,
        provider_request_headers: Optional[Dict[str, Any]] = None,
        response_headers: Optional[Dict[str, Any]] = None,
        client_response_headers: Optional[Dict[str, Any]] = None,
        response_body: Optional[Any] = None,
        request_id: Optional[str] = None,
        provider_id: Optional[str] = None,
        provider_endpoint_id: Optional[str] = None,
        provider_api_key_id: Optional[str] = None,
        status: str = "completed",
        cache_ttl_minutes: Optional[int] = None,
        use_tiered_pricing: bool = True,
        target_model: Optional[str] = None,
    ) -> Usage:
        """异步记录使用量（简化版，仅插入新记录）

        此方法用于快速记录使用量，不更新用户/API Key 统计，不支持更新已存在的记录。
        适用于不需要更新统计信息的场景。

        如需完整功能（更新用户统计、支持更新已存在记录），请使用 record_usage()。
        """
        # 生成 request_id
        if request_id is None:
            request_id = str(uuid.uuid4())[:8]

        # 使用共享逻辑准备记录参数
        params = UsageRecordParams(
            db=db, user=user, api_key=api_key, provider=provider, model=model,
            input_tokens=input_tokens, output_tokens=output_tokens,
            cache_creation_input_tokens=cache_creation_input_tokens,
            cache_read_input_tokens=cache_read_input_tokens,
            request_type=request_type, api_format=api_format, is_stream=is_stream,
            response_time_ms=response_time_ms, first_byte_time_ms=first_byte_time_ms,
            status_code=status_code, error_message=error_message, metadata=metadata,
            request_headers=request_headers, request_body=request_body,
            provider_request_headers=provider_request_headers,
            response_headers=response_headers, client_response_headers=client_response_headers,
            response_body=response_body,
            request_id=request_id, provider_id=provider_id,
            provider_endpoint_id=provider_endpoint_id,
            provider_api_key_id=provider_api_key_id, status=status,
            cache_ttl_minutes=cache_ttl_minutes, use_tiered_pricing=use_tiered_pricing,
            target_model=target_model,
        )
        usage_params, _ = await cls._prepare_usage_record(params)

        # 创建 Usage 记录
        usage = Usage(**usage_params)
        db.add(usage)

        # 更新 GlobalModel 使用计数（原子操作）
        from sqlalchemy import update
        from src.models.database import GlobalModel

        db.execute(
            update(GlobalModel)
            .where(GlobalModel.name == model)
            .values(usage_count=GlobalModel.usage_count + 1)
        )

        # 更新 Provider 月度使用量（原子操作）
        if provider_id:
            actual_total_cost = usage_params["actual_total_cost_usd"]
            db.execute(
                update(Provider)
                .where(Provider.id == provider_id)
                .values(monthly_used_usd=Provider.monthly_used_usd + actual_total_cost)
            )

        db.commit()  # 立即提交事务，释放数据库锁
        return usage

    @classmethod
    async def record_usage(
        cls,
        db: Session,
        user: Optional[User],
        api_key: Optional[ApiKey],
        provider: str,
        model: str,
        input_tokens: int,
        output_tokens: int,
        cache_creation_input_tokens: int = 0,
        cache_read_input_tokens: int = 0,
        request_type: str = "chat",
        api_format: Optional[str] = None,
        is_stream: bool = False,
        response_time_ms: Optional[int] = None,
        first_byte_time_ms: Optional[int] = None,
        status_code: int = 200,
        error_message: Optional[str] = None,
        metadata: Optional[Dict[str, Any]] = None,
        request_headers: Optional[Dict[str, Any]] = None,
        request_body: Optional[Any] = None,
        provider_request_headers: Optional[Dict[str, Any]] = None,
        response_headers: Optional[Dict[str, Any]] = None,
        client_response_headers: Optional[Dict[str, Any]] = None,
        response_body: Optional[Any] = None,
        request_id: Optional[str] = None,
        provider_id: Optional[str] = None,
        provider_endpoint_id: Optional[str] = None,
        provider_api_key_id: Optional[str] = None,
        status: str = "completed",
        cache_ttl_minutes: Optional[int] = None,
        use_tiered_pricing: bool = True,
        target_model: Optional[str] = None,
    ) -> Usage:
        """记录使用量（完整版，支持更新已存在记录和用户统计）

        此方法支持：
        - 检查是否已存在相同 request_id 的记录（更新 vs 插入）
        - 更新用户/API Key 使用统计
        - 阶梯计费

        如只需简单插入新记录，可使用 record_usage_async()。
        """
        # 生成 request_id
        if request_id is None:
            request_id = str(uuid.uuid4())[:8]

        # 使用共享逻辑准备记录参数
        params = UsageRecordParams(
            db=db, user=user, api_key=api_key, provider=provider, model=model,
            input_tokens=input_tokens, output_tokens=output_tokens,
            cache_creation_input_tokens=cache_creation_input_tokens,
            cache_read_input_tokens=cache_read_input_tokens,
            request_type=request_type, api_format=api_format, is_stream=is_stream,
            response_time_ms=response_time_ms, first_byte_time_ms=first_byte_time_ms,
            status_code=status_code, error_message=error_message, metadata=metadata,
            request_headers=request_headers, request_body=request_body,
            provider_request_headers=provider_request_headers,
            response_headers=response_headers, client_response_headers=client_response_headers,
            response_body=response_body,
            request_id=request_id, provider_id=provider_id,
            provider_endpoint_id=provider_endpoint_id,
            provider_api_key_id=provider_api_key_id, status=status,
            cache_ttl_minutes=cache_ttl_minutes, use_tiered_pricing=use_tiered_pricing,
            target_model=target_model,
        )
        usage_params, total_cost = await cls._prepare_usage_record(params)

        # 检查是否已存在相同 request_id 的记录
        existing_usage = db.query(Usage).filter(Usage.request_id == request_id).first()
        if existing_usage:
            logger.debug(
                f"request_id {request_id} 已存在，更新现有记录 "
                f"(status: {existing_usage.status} -> {status})"
            )
            cls._update_existing_usage(existing_usage, usage_params, target_model)
            usage = existing_usage
        else:
            usage = Usage(**usage_params)
            db.add(usage)

        # 确保 user 和 api_key 在会话中
        if user and not db.object_session(user):
            user = db.merge(user)
        if api_key and not db.object_session(api_key):
            api_key = db.merge(api_key)

        # 使用原子更新避免并发竞态条件
        from sqlalchemy import func as sql_func, update
        from src.models.database import ApiKey as ApiKeyModel, User as UserModel, GlobalModel

        # 更新用户使用量（独立 Key 不计入创建者的使用记录）
        if user and not (api_key and api_key.is_standalone):
            db.execute(
                update(UserModel)
                .where(UserModel.id == user.id)
                .values(
                    used_usd=UserModel.used_usd + total_cost,
                    total_usd=UserModel.total_usd + total_cost,
                    updated_at=sql_func.now(),
                )
            )

        # 更新 API 密钥使用量
        if api_key:
            if api_key.is_standalone:
                db.execute(
                    update(ApiKeyModel)
                    .where(ApiKeyModel.id == api_key.id)
                    .values(
                        total_requests=ApiKeyModel.total_requests + 1,
                        total_cost_usd=ApiKeyModel.total_cost_usd + total_cost,
                        balance_used_usd=ApiKeyModel.balance_used_usd + total_cost,
                        last_used_at=sql_func.now(),
                        updated_at=sql_func.now(),
                    )
                )
            else:
                db.execute(
                    update(ApiKeyModel)
                    .where(ApiKeyModel.id == api_key.id)
                    .values(
                        total_requests=ApiKeyModel.total_requests + 1,
                        total_cost_usd=ApiKeyModel.total_cost_usd + total_cost,
                        last_used_at=sql_func.now(),
                        updated_at=sql_func.now(),
                    )
                )

        # 更新 GlobalModel 使用计数
        db.execute(
            update(GlobalModel)
            .where(GlobalModel.name == model)
            .values(usage_count=GlobalModel.usage_count + 1)
        )

        # 更新 Provider 月度使用量
        if provider_id:
            actual_total_cost = usage_params["actual_total_cost_usd"]
            db.execute(
                update(Provider)
                .where(Provider.id == provider_id)
                .values(monthly_used_usd=Provider.monthly_used_usd + actual_total_cost)
            )

        # 提交事务
        try:
            db.commit()
        except Exception as e:
            logger.error(f"提交使用记录时出错: {e}")
            db.rollback()
            raise

        return usage

    @staticmethod
    def check_user_quota(
        db: Session,
        user: User,
        estimated_tokens: int = 0,
        estimated_cost: float = 0,
        api_key: Optional[ApiKey] = None,
    ) -> tuple[bool, str]:
        """检查用户配额或独立Key余额

        Args:
            db: 数据库会话
            user: 用户对象
            estimated_tokens: 预估token数
            estimated_cost: 预估费用
            api_key: API Key对象（用于检查独立余额Key）

        Returns:
            (是否通过, 消息)
        """

        # 如果是独立余额Key，检查Key的余额而不是用户配额
        if api_key and api_key.is_standalone:
            # 导入 ApiKeyService 以使用统一的余额计算方法
            from src.services.user.apikey import ApiKeyService

            # NULL 表示无限制
            if api_key.current_balance_usd is None:
                return True, "OK"

            # 使用统一的余额计算方法
            remaining_balance = ApiKeyService.get_remaining_balance(api_key)
            if remaining_balance is None:
                return True, "OK"

            # 检查余额是否充足
            if remaining_balance < estimated_cost:
                return (
                    False,
                    f"Key余额不足（剩余: ${remaining_balance:.2f}，需要: ${estimated_cost:.2f}）",
                )

            return True, "OK"

        # 普通Key：检查用户配额
        # 管理员无限制
        if user.role == UserRole.ADMIN:
            return True, "OK"

        # NULL 表示无限制
        if user.quota_usd is None:
            return True, "OK"

        # 有配额限制，检查是否超额
        used_usd = float(user.used_usd or 0)
        quota_usd = float(user.quota_usd)
        if used_usd + estimated_cost > quota_usd:
            remaining = quota_usd - used_usd
            return False, f"配额不足（剩余: ${remaining:.2f}）"

        return True, "OK"

    @staticmethod
    def get_usage_summary(
        db: Session,
        user_id: Optional[str] = None,
        api_key_id: Optional[str] = None,
        start_date: Optional[datetime] = None,
        end_date: Optional[datetime] = None,
        group_by: str = "day",  # day, week, month
    ) -> List[Dict[str, Any]]:
        """获取使用汇总"""

        query = db.query(Usage)
        # 过滤掉 pending/streaming 状态的请求（尚未完成的请求不应计入统计）
        query = query.filter(Usage.status.notin_(["pending", "streaming"]))

        if user_id:
            query = query.filter(Usage.user_id == user_id)
        if api_key_id:
            query = query.filter(Usage.api_key_id == api_key_id)
        if start_date:
            query = query.filter(Usage.created_at >= start_date)
        if end_date:
            query = query.filter(Usage.created_at <= end_date)

        # 使用跨数据库兼容的日期函数
        from src.utils.database_helpers import date_trunc_portable

        # 检测数据库方言
        bind = db.bind
        dialect = bind.dialect.name if bind is not None else "sqlite"

        # 根据分组类型选择日期函数（兼容多种数据库）
        if group_by == "day":
            date_func = date_trunc_portable(dialect, "day", Usage.created_at)
        elif group_by == "week":
            date_func = date_trunc_portable(dialect, "week", Usage.created_at)
        elif group_by == "month":
            date_func = date_trunc_portable(dialect, "month", Usage.created_at)
        else:
            # 默认按天分组
            date_func = date_trunc_portable(dialect, "day", Usage.created_at)

        # 汇总查询
        summary = db.query(
            date_func.label("period"),
            Usage.provider_name,
            Usage.model,
            func.count(Usage.id).label("requests"),
            func.sum(Usage.input_tokens).label("input_tokens"),
            func.sum(Usage.output_tokens).label("output_tokens"),
            func.sum(Usage.total_tokens).label("total_tokens"),
            func.sum(Usage.total_cost_usd).label("total_cost_usd"),
            func.avg(Usage.response_time_ms).label("avg_response_time"),
        )

        if user_id:
            summary = summary.filter(Usage.user_id == user_id)
        if api_key_id:
            summary = summary.filter(Usage.api_key_id == api_key_id)
        if start_date:
            summary = summary.filter(Usage.created_at >= start_date)
        if end_date:
            summary = summary.filter(Usage.created_at <= end_date)

        summary = summary.group_by(date_func, Usage.provider_name, Usage.model).all()

        return [
            {
                "period": row.period,
                "provider": row.provider_name,
                "model": row.model,
                "requests": row.requests,
                "input_tokens": row.input_tokens,
                "output_tokens": row.output_tokens,
                "total_tokens": row.total_tokens,
                "total_cost_usd": float(row.total_cost_usd),
                "avg_response_time_ms": (
                    float(row.avg_response_time) if row.avg_response_time else 0
                ),
            }
            for row in summary
        ]

    @staticmethod
    def get_daily_activity(
        db: Session,
        user_id: Optional[int] = None,
        start_date: Optional[datetime] = None,
        end_date: Optional[datetime] = None,
        window_days: int = 365,
        include_actual_cost: bool = False,
    ) -> Dict[str, Any]:
        """按天统计请求活跃度，用于渲染热力图。

        优化策略：
        - 历史数据从预计算的 StatsDaily/StatsUserDaily 表读取
        - 只有"今天"的数据才实时查询 Usage 表
        """

        def ensure_timezone(value: datetime) -> datetime:
            if value.tzinfo is None:
                return value.replace(tzinfo=timezone.utc)
            return value.astimezone(timezone.utc)

        # 如果调用方未指定时间范围，则默认统计最近 window_days 天
        now = datetime.now(timezone.utc)
        end_dt = ensure_timezone(end_date) if end_date else now
        start_dt = (
            ensure_timezone(start_date) if start_date else end_dt - timedelta(days=window_days - 1)
        )

        # 对齐到自然日的开始/结束
        start_dt = datetime.combine(start_dt.date(), datetime.min.time(), tzinfo=timezone.utc)
        end_dt = datetime.combine(end_dt.date(), datetime.max.time(), tzinfo=timezone.utc)

        today = now.date()
        today_start_dt = datetime.combine(today, datetime.min.time(), tzinfo=timezone.utc)
        aggregated: Dict[str, Dict[str, Any]] = {}

        # 1. 从预计算表读取历史数据（不包括今天）
        if user_id:
            from src.models.database import StatsUserDaily

            hist_query = db.query(StatsUserDaily).filter(
                StatsUserDaily.user_id == user_id,
                StatsUserDaily.date >= start_dt,
                StatsUserDaily.date < today_start_dt,
            )
            for row in hist_query.all():
                key = (
                    row.date.date().isoformat()
                    if isinstance(row.date, datetime)
                    else str(row.date)[:10]
                )
                aggregated[key] = {
                    "requests": row.total_requests or 0,
                    "total_tokens": (
                        (row.input_tokens or 0)
                        + (row.output_tokens or 0)
                        + (row.cache_creation_tokens or 0)
                        + (row.cache_read_tokens or 0)
                    ),
                    "total_cost_usd": float(row.total_cost or 0.0),
                }
                # StatsUserDaily 没有 actual_total_cost 字段，用户视图不需要倍率成本
        else:
            from src.models.database import StatsDaily

            hist_query = db.query(StatsDaily).filter(
                StatsDaily.date >= start_dt,
                StatsDaily.date < today_start_dt,
            )
            for row in hist_query.all():
                key = (
                    row.date.date().isoformat()
                    if isinstance(row.date, datetime)
                    else str(row.date)[:10]
                )
                aggregated[key] = {
                    "requests": row.total_requests or 0,
                    "total_tokens": (
                        (row.input_tokens or 0)
                        + (row.output_tokens or 0)
                        + (row.cache_creation_tokens or 0)
                        + (row.cache_read_tokens or 0)
                    ),
                    "total_cost_usd": float(row.total_cost or 0.0),
                }
                if include_actual_cost:
                    aggregated[key]["actual_total_cost_usd"] = float(
                        row.actual_total_cost or 0.0  # type: ignore[attr-defined]
                    )

        # 2. 实时查询今天的数据（如果在查询范围内）
        if today >= start_dt.date() and today <= end_dt.date():
            today_start = datetime.combine(today, datetime.min.time(), tzinfo=timezone.utc)
            today_end = datetime.combine(today, datetime.max.time(), tzinfo=timezone.utc)

            if include_actual_cost:
                today_query = db.query(
                    func.count(Usage.id).label("requests"),
                    func.sum(Usage.total_tokens).label("total_tokens"),
                    func.sum(Usage.total_cost_usd).label("total_cost_usd"),
                    func.sum(Usage.actual_total_cost_usd).label("actual_total_cost_usd"),
                ).filter(
                    Usage.created_at >= today_start,
                    Usage.created_at <= today_end,
                )
            else:
                today_query = db.query(
                    func.count(Usage.id).label("requests"),
                    func.sum(Usage.total_tokens).label("total_tokens"),
                    func.sum(Usage.total_cost_usd).label("total_cost_usd"),
                ).filter(
                    Usage.created_at >= today_start,
                    Usage.created_at <= today_end,
                )

            if user_id:
                today_query = today_query.filter(Usage.user_id == user_id)

            today_row = today_query.first()
            if today_row and today_row.requests:
                aggregated[today.isoformat()] = {
                    "requests": int(today_row.requests or 0),
                    "total_tokens": int(today_row.total_tokens or 0),
                    "total_cost_usd": float(today_row.total_cost_usd or 0.0),
                }
                if include_actual_cost:
                    aggregated[today.isoformat()]["actual_total_cost_usd"] = float(
                        today_row.actual_total_cost_usd or 0.0
                    )

        # 3. 构建返回结果
        days: List[Dict[str, Any]] = []
        cursor = start_dt.date()
        end_date_only = end_dt.date()
        max_requests = 0

        while cursor <= end_date_only:
            iso_date = cursor.isoformat()
            stats = aggregated.get(iso_date, {})
            requests = stats.get("requests", 0)
            total_tokens = stats.get("total_tokens", 0)
            total_cost = stats.get("total_cost_usd", 0.0)

            entry: Dict[str, Any] = {
                "date": iso_date,
                "requests": requests,
                "total_tokens": total_tokens,
                "total_cost": total_cost,
            }

            if include_actual_cost:
                entry["actual_total_cost"] = stats.get("actual_total_cost_usd", 0.0)

            days.append(entry)
            max_requests = max(max_requests, requests)
            cursor += timedelta(days=1)

        return {
            "start_date": start_dt.date().isoformat(),
            "end_date": end_dt.date().isoformat(),
            "total_days": len(days),
            "max_requests": max_requests,
            "days": days,
        }

    @staticmethod
    def get_top_users(
        db: Session,
        limit: int = 10,
        start_date: Optional[datetime] = None,
        end_date: Optional[datetime] = None,
        order_by: str = "cost",  # cost, tokens, requests
    ) -> List[Dict[str, Any]]:
        """获取使用量最高的用户"""

        query = (
            db.query(
                User.id,
                User.email,
                User.username,
                func.count(Usage.id).label("requests"),
                func.sum(Usage.total_tokens).label("tokens"),
                func.sum(Usage.total_cost_usd).label("cost_usd"),
            )
            .join(Usage, User.id == Usage.user_id)
            .filter(Usage.user_id.isnot(None))
        )

        if start_date:
            query = query.filter(Usage.created_at >= start_date)
        if end_date:
            query = query.filter(Usage.created_at <= end_date)

        query = query.group_by(User.id, User.email, User.username)

        # 排序
        if order_by == "cost":
            query = query.order_by(func.sum(Usage.total_cost_usd).desc())
        elif order_by == "tokens":
            query = query.order_by(func.sum(Usage.total_tokens).desc())
        else:
            query = query.order_by(func.count(Usage.id).desc())

        results = query.limit(limit).all()

        return [
            {
                "user_id": row.id,
                "email": row.email,
                "username": row.username,
                "requests": row.requests,
                "tokens": row.tokens,
                "cost_usd": float(row.cost_usd),
            }
            for row in results
        ]

    @staticmethod
    def cleanup_old_usage_records(
        db: Session, days_to_keep: int = 90, batch_size: int = 1000
    ) -> int:
        """清理旧的使用记录（分批删除避免长事务锁定）

        Args:
            db: 数据库会话
            days_to_keep: 保留天数，默认 90 天
            batch_size: 每批删除数量，默认 1000 条

        Returns:
            删除的总记录数
        """
        cutoff_date = datetime.now(timezone.utc) - timedelta(days=days_to_keep)
        total_deleted = 0

        while True:
            # 查询待删除的 ID（使用新索引 idx_usage_user_created）
            batch_ids = (
                db.query(Usage.id)
                .filter(Usage.created_at < cutoff_date)
                .limit(batch_size)
                .all()
            )

            if not batch_ids:
                break

            # 批量删除
            deleted_count = (
                db.query(Usage)
                .filter(Usage.id.in_([row.id for row in batch_ids]))
                .delete(synchronize_session=False)
            )
            db.commit()
            total_deleted += deleted_count

            logger.debug(f"清理使用记录: 本批删除 {deleted_count} 条")

        logger.info(f"清理使用记录: 共删除 {total_deleted} 条超过 {days_to_keep} 天的记录")

        return total_deleted

    # ========== 请求状态追踪方法 ==========

    @classmethod
    def create_pending_usage(
        cls,
        db: Session,
        request_id: str,
        user: Optional[User],
        api_key: Optional[ApiKey],
        model: str,
        is_stream: bool = False,
        api_format: Optional[str] = None,
        request_headers: Optional[Dict[str, Any]] = None,
        request_body: Optional[Any] = None,
    ) -> Usage:
        """
        创建 pending 状态的使用记录（在请求开始时调用）

        Args:
            db: 数据库会话
            request_id: 请求ID
            user: 用户对象
            api_key: API Key 对象
            model: 模型名称
            is_stream: 是否流式请求
            api_format: API 格式
            request_headers: 请求头
            request_body: 请求体

        Returns:
            创建的 Usage 记录
        """
        # 根据配置决定是否记录请求详情
        should_log_headers = SystemConfigService.should_log_headers(db)
        should_log_body = SystemConfigService.should_log_body(db)

        # 处理请求头
        processed_request_headers = None
        if should_log_headers and request_headers:
            processed_request_headers = SystemConfigService.mask_sensitive_headers(
                db, request_headers
            )

        # 处理请求体
        processed_request_body = None
        if should_log_body and request_body:
            processed_request_body = SystemConfigService.truncate_body(
                db, request_body, is_request=True
            )

        usage = Usage(
            user_id=user.id if user else None,
            api_key_id=api_key.id if api_key else None,
            request_id=request_id,
            provider_name="pending",  # 尚未确定 provider
            model=model,
            input_tokens=0,
            output_tokens=0,
            total_tokens=0,
            total_cost_usd=0.0,
            request_type="chat",
            api_format=api_format,
            is_stream=is_stream,
            status="pending",
            request_headers=processed_request_headers,
            request_body=processed_request_body,
        )

        db.add(usage)
        db.commit()

        logger.debug(f"创建 pending 使用记录: request_id={request_id}, model={model}")

        return usage

    @classmethod
    def update_usage_status(
        cls,
        db: Session,
        request_id: str,
        status: str,
        error_message: Optional[str] = None,
        provider: Optional[str] = None,
        target_model: Optional[str] = None,
        first_byte_time_ms: Optional[int] = None,
        provider_id: Optional[str] = None,
        provider_endpoint_id: Optional[str] = None,
        provider_api_key_id: Optional[str] = None,
    ) -> Optional[Usage]:
        """
        快速更新使用记录状态

        Args:
            db: 数据库会话
            request_id: 请求ID
            status: 新状态 (pending, streaming, completed, failed)
            error_message: 错误消息（仅在 failed 状态时使用）
            provider: 提供商名称（可选，streaming 状态时更新）
            target_model: 映射后的目标模型名（可选）
            first_byte_time_ms: 首字时间/TTFB（可选，streaming 状态时更新）
            provider_id: Provider ID（可选，streaming 状态时更新）
            provider_endpoint_id: Endpoint ID（可选，streaming 状态时更新）
            provider_api_key_id: Provider API Key ID（可选，streaming 状态时更新）

        Returns:
            更新后的 Usage 记录，如果未找到则返回 None
        """
        usage = db.query(Usage).filter(Usage.request_id == request_id).first()
        if not usage:
            logger.warning(f"未找到 request_id={request_id} 的使用记录，无法更新状态")
            return None

        old_status = usage.status
        usage.status = status
        if error_message:
            usage.error_message = error_message
        if provider:
            usage.provider_name = provider
        elif status == "streaming" and usage.provider_name == "pending":
            # 状态变为 streaming 但 provider_name 仍为 pending，记录警告
            logger.warning(
                f"状态更新为 streaming 但 provider_name 为空: request_id={request_id}, "
                f"当前 provider_name={usage.provider_name}"
            )
        if target_model:
            usage.target_model = target_model
        if first_byte_time_ms is not None:
            usage.first_byte_time_ms = first_byte_time_ms
        if provider_id is not None:
            usage.provider_id = provider_id
        if provider_endpoint_id is not None:
            usage.provider_endpoint_id = provider_endpoint_id
        if provider_api_key_id is not None:
            usage.provider_api_key_id = provider_api_key_id

        db.commit()

        logger.debug(f"更新使用记录状态: request_id={request_id}, {old_status} -> {status}")

        return usage

    @classmethod
    def get_active_requests(
        cls,
        db: Session,
        user_id: Optional[str] = None,
        limit: int = 50,
    ) -> List[Usage]:
        """
        获取活跃的请求（pending 或 streaming 状态）

        Args:
            db: 数据库会话
            user_id: 用户ID（可选，用于过滤）
            limit: 最大返回数量

        Returns:
            活跃请求的 Usage 列表
        """
        query = db.query(Usage).filter(Usage.status.in_(["pending", "streaming"]))

        if user_id:
            query = query.filter(Usage.user_id == user_id)

        return query.order_by(Usage.created_at.desc()).limit(limit).all()

    @classmethod
    def cleanup_stale_pending_requests(
        cls,
        db: Session,
        timeout_minutes: int = 10,
    ) -> int:
        """
        清理超时的 pending/streaming 请求

        将超过指定时间仍处于 pending 或 streaming 状态的请求标记为 failed。
        这些请求可能是由于网络问题、服务重启或其他异常导致未能正常完成。

        Args:
            db: 数据库会话
            timeout_minutes: 超时时间（分钟），默认 10 分钟

        Returns:
            清理的记录数
        """
        cutoff_time = datetime.now(timezone.utc) - timedelta(minutes=timeout_minutes)

        # 查找超时的请求
        stale_requests = (
            db.query(Usage)
            .filter(
                Usage.status.in_(["pending", "streaming"]),
                Usage.created_at < cutoff_time,
            )
            .all()
        )

        count = 0
        for usage in stale_requests:
            old_status = usage.status
            usage.status = "failed"
            usage.error_message = f"请求超时: 状态 '{old_status}' 超过 {timeout_minutes} 分钟未完成"
            usage.status_code = 504  # Gateway Timeout
            count += 1

        if count > 0:
            db.commit()
            logger.info(f"清理超时请求: 将 {count} 条超过 {timeout_minutes} 分钟的 pending/streaming 请求标记为 failed")

        return count

    @classmethod
    def get_stale_pending_count(
        cls,
        db: Session,
        timeout_minutes: int = 10,
    ) -> int:
        """
        获取超时的 pending/streaming 请求数量（用于监控）

        Args:
            db: 数据库会话
            timeout_minutes: 超时时间（分钟）

        Returns:
            超时请求数量
        """
        cutoff_time = datetime.now(timezone.utc) - timedelta(minutes=timeout_minutes)

        return (
            db.query(Usage)
            .filter(
                Usage.status.in_(["pending", "streaming"]),
                Usage.created_at < cutoff_time,
            )
            .count()
        )

    @classmethod
    def get_active_requests_status(
        cls,
        db: Session,
        ids: Optional[List[str]] = None,
        user_id: Optional[str] = None,
        default_timeout_seconds: int = 300,
        *,
        include_admin_fields: bool = False,
    ) -> List[Dict[str, Any]]:
        """
        获取活跃请求状态（用于前端轮询），并自动清理超时的 pending/streaming 请求

        与 get_active_requests 不同，此方法：
        1. 返回轻量级的状态字典而非完整 Usage 对象
        2. 自动检测并清理超时的 pending/streaming 请求
        3. 支持按 ID 列表查询特定请求

        Args:
            db: 数据库会话
            ids: 指定要查询的请求 ID 列表（可选）
            user_id: 限制只查询该用户的请求（可选，用于普通用户接口）
            default_timeout_seconds: 默认超时时间（秒），当端点未配置时使用

        Returns:
            请求状态列表
        """
        from src.models.database import ProviderEndpoint

        now = datetime.now(timezone.utc)

        # 构建基础查询，包含端点的 timeout 配置
        query = db.query(
            Usage.id,
            Usage.status,
            Usage.input_tokens,
            Usage.output_tokens,
            Usage.total_cost_usd,
            Usage.response_time_ms,
            Usage.first_byte_time_ms,  # 首字时间 (TTFB)
            Usage.created_at,
            Usage.provider_endpoint_id,
            ProviderEndpoint.timeout.label("endpoint_timeout"),
        ).outerjoin(ProviderEndpoint, Usage.provider_endpoint_id == ProviderEndpoint.id)

        # 管理员轮询：可附带 provider 与上游 key 名称（注意：不要在普通用户接口暴露上游 key 信息）
        if include_admin_fields:
            from src.models.database import ProviderAPIKey

            query = query.add_columns(
                Usage.provider_name,
                ProviderAPIKey.name.label("api_key_name"),
            ).outerjoin(ProviderAPIKey, Usage.provider_api_key_id == ProviderAPIKey.id)

        if ids:
            query = query.filter(Usage.id.in_(ids))
            if user_id:
                query = query.filter(Usage.user_id == user_id)
        else:
            # 查询所有活跃请求
            query = query.filter(Usage.status.in_(["pending", "streaming"]))
            if user_id:
                query = query.filter(Usage.user_id == user_id)
            query = query.order_by(Usage.created_at.desc()).limit(50)

        records = query.all()

        # 检查超时的 pending/streaming 请求
        timeout_ids = []
        for r in records:
            if r.status in ("pending", "streaming") and r.created_at:
                # 使用端点配置的超时时间，若无则使用默认值
                timeout_seconds = r.endpoint_timeout or default_timeout_seconds

                # 处理时区：如果 created_at 没有时区信息，假定为 UTC
                created_at = r.created_at
                if created_at.tzinfo is None:
                    created_at = created_at.replace(tzinfo=timezone.utc)
                elapsed = (now - created_at).total_seconds()
                if elapsed > timeout_seconds:
                    timeout_ids.append(r.id)

        # 批量更新超时的请求
        if timeout_ids:
            db.query(Usage).filter(Usage.id.in_(timeout_ids)).update(
                {"status": "failed", "error_message": "请求超时（服务器可能已重启）"},
                synchronize_session=False,
            )
            db.commit()

        result: List[Dict[str, Any]] = []
        for r in records:
            item: Dict[str, Any] = {
                "id": r.id,
                "status": "failed" if r.id in timeout_ids else r.status,
                "input_tokens": r.input_tokens,
                "output_tokens": r.output_tokens,
                "cost": float(r.total_cost_usd) if r.total_cost_usd else 0,
                "response_time_ms": r.response_time_ms,
                "first_byte_time_ms": r.first_byte_time_ms,  # 首字时间 (TTFB)
            }
            if include_admin_fields:
                item["provider"] = r.provider_name
                item["api_key_name"] = r.api_key_name
            result.append(item)

        return result

    # ========== 缓存亲和性分析方法 ==========

    @staticmethod
    def analyze_cache_affinity_ttl(
        db: Session,
        user_id: Optional[str] = None,
        api_key_id: Optional[str] = None,
        hours: int = 168,
    ) -> Dict[str, Any]:
        """
        分析用户请求间隔分布，推荐合适的缓存亲和性 TTL

        通过分析同一用户连续请求之间的时间间隔，判断用户的使用模式：
        - 高频用户（间隔短）：5 分钟 TTL 足够
        - 中频用户：15-30 分钟 TTL
        - 低频用户（间隔长）：需要 60 分钟 TTL

        Args:
            db: 数据库会话
            user_id: 指定用户 ID（可选，为空则分析所有用户）
            api_key_id: 指定 API Key ID（可选）
            hours: 分析最近多少小时的数据

        Returns:
            包含分析结果的字典
        """
        from sqlalchemy import text

        # 计算时间范围
        start_date = datetime.now(timezone.utc) - timedelta(hours=hours)

        # 构建 SQL 查询 - 使用窗口函数计算请求间隔
        # 按 user_id 或 api_key_id 分组，计算同一组内连续请求的时间差
        group_by_field = "api_key_id" if api_key_id else "user_id"

        # 构建过滤条件
        filter_clause = ""
        if user_id or api_key_id:
            filter_clause = f"AND {group_by_field} = :filter_id"

        sql = text(f"""
            WITH user_requests AS (
                SELECT
                    {group_by_field} as group_id,
                    created_at,
                    LAG(created_at) OVER (
                        PARTITION BY {group_by_field}
                        ORDER BY created_at
                    ) as prev_request_at
                FROM usage
                WHERE status = 'completed'
                  AND created_at > :start_date
                  AND {group_by_field} IS NOT NULL
                  {filter_clause}
            ),
            intervals AS (
                SELECT
                    group_id,
                    EXTRACT(EPOCH FROM (created_at - prev_request_at)) / 60.0 as interval_minutes
                FROM user_requests
                WHERE prev_request_at IS NOT NULL
            ),
            user_stats AS (
                SELECT
                    group_id,
                    COUNT(*) as request_count,
                    COUNT(*) FILTER (WHERE interval_minutes <= 5) as within_5min,
                    COUNT(*) FILTER (WHERE interval_minutes > 5 AND interval_minutes <= 15) as within_15min,
                    COUNT(*) FILTER (WHERE interval_minutes > 15 AND interval_minutes <= 30) as within_30min,
                    COUNT(*) FILTER (WHERE interval_minutes > 30 AND interval_minutes <= 60) as within_60min,
                    COUNT(*) FILTER (WHERE interval_minutes > 60) as over_60min,
                    PERCENTILE_CONT(0.5) WITHIN GROUP (ORDER BY interval_minutes) as median_interval,
                    PERCENTILE_CONT(0.75) WITHIN GROUP (ORDER BY interval_minutes) as p75_interval,
                    PERCENTILE_CONT(0.90) WITHIN GROUP (ORDER BY interval_minutes) as p90_interval,
                    AVG(interval_minutes) as avg_interval,
                    MIN(interval_minutes) as min_interval,
                    MAX(interval_minutes) as max_interval
                FROM intervals
                GROUP BY group_id
                HAVING COUNT(*) >= 2
            )
            SELECT * FROM user_stats
            ORDER BY request_count DESC
        """)

        params: Dict[str, Any] = {
            "start_date": start_date,
        }
        if user_id:
            params["filter_id"] = user_id
        elif api_key_id:
            params["filter_id"] = api_key_id

        result = db.execute(sql, params)
        rows = result.fetchall()

        # 收集所有 user_id 以便批量查询用户信息
        group_ids = [row[0] for row in rows]

        # 如果是按 user_id 分组，查询用户信息
        user_info_map: Dict[str, Dict[str, str]] = {}
        if group_by_field == "user_id" and group_ids:
            users = db.query(User).filter(User.id.in_(group_ids)).all()
            for user in users:
                user_info_map[str(user.id)] = {
                    "username": str(user.username),
                    "email": str(user.email) if user.email else "",
                }

        # 处理结果
        users_analysis = []
        for row in rows:
            # row 是一个 tuple，按查询顺序访问
            (
                group_id,
                request_count,
                within_5min,
                within_15min,
                within_30min,
                within_60min,
                over_60min,
                median_interval,
                p75_interval,
                p90_interval,
                avg_interval,
                min_interval,
                max_interval,
            ) = row

            # 计算推荐 TTL
            recommended_ttl = UsageService._calculate_recommended_ttl(
                p75_interval, p90_interval
            )

            # 获取用户信息
            user_info = user_info_map.get(str(group_id), {})

            # 计算各区间占比
            total_intervals = request_count
            users_analysis.append({
                "group_id": group_id,
                "username": user_info.get("username"),
                "email": user_info.get("email"),
                "request_count": request_count,
                "interval_distribution": {
                    "within_5min": within_5min,
                    "within_15min": within_15min,
                    "within_30min": within_30min,
                    "within_60min": within_60min,
                    "over_60min": over_60min,
                },
                "interval_percentages": {
                    "within_5min": round(within_5min / total_intervals * 100, 1),
                    "within_15min": round(within_15min / total_intervals * 100, 1),
                    "within_30min": round(within_30min / total_intervals * 100, 1),
                    "within_60min": round(within_60min / total_intervals * 100, 1),
                    "over_60min": round(over_60min / total_intervals * 100, 1),
                },
                "percentiles": {
                    "p50": round(float(median_interval), 2) if median_interval else None,
                    "p75": round(float(p75_interval), 2) if p75_interval else None,
                    "p90": round(float(p90_interval), 2) if p90_interval else None,
                },
                "avg_interval_minutes": round(float(avg_interval), 2) if avg_interval else None,
                "min_interval_minutes": round(float(min_interval), 2) if min_interval else None,
                "max_interval_minutes": round(float(max_interval), 2) if max_interval else None,
                "recommended_ttl_minutes": recommended_ttl,
                "recommendation_reason": UsageService._get_ttl_recommendation_reason(
                    recommended_ttl, p75_interval, p90_interval
                ),
            })

        # 汇总统计
        ttl_distribution = {"5min": 0, "15min": 0, "30min": 0, "60min": 0}
        for analysis in users_analysis:
            ttl = analysis["recommended_ttl_minutes"]
            if ttl <= 5:
                ttl_distribution["5min"] += 1
            elif ttl <= 15:
                ttl_distribution["15min"] += 1
            elif ttl <= 30:
                ttl_distribution["30min"] += 1
            else:
                ttl_distribution["60min"] += 1

        return {
            "analysis_period_hours": hours,
            "total_users_analyzed": len(users_analysis),
            "ttl_distribution": ttl_distribution,
            "users": users_analysis,
        }

    @staticmethod
    def _calculate_recommended_ttl(
        p75_interval: Optional[float],
        p90_interval: Optional[float],
    ) -> int:
        """
        根据请求间隔分布计算推荐的缓存 TTL

        策略：
        - 如果 90% 的请求间隔都在 5 分钟内 → 5 分钟 TTL
        - 如果 75% 的请求间隔在 15 分钟内 → 15 分钟 TTL
        - 如果 75% 的请求间隔在 30 分钟内 → 30 分钟 TTL
        - 否则 → 60 分钟 TTL
        """
        if p90_interval is None or p75_interval is None:
            return 5  # 默认值

        # 如果 90% 的间隔都在 5 分钟内
        if p90_interval <= 5:
            return 5

        # 如果 75% 的间隔在 15 分钟内
        if p75_interval <= 15:
            return 15

        # 如果 75% 的间隔在 30 分钟内
        if p75_interval <= 30:
            return 30

        # 低频用户，需要更长的 TTL
        return 60

    @staticmethod
    def _get_ttl_recommendation_reason(
        ttl: int,
        p75_interval: Optional[float],
        p90_interval: Optional[float],
    ) -> str:
        """生成 TTL 推荐理由"""
        if p75_interval is None or p90_interval is None:
            return "数据不足，使用默认值"

        if ttl == 5:
            return f"高频用户：90% 的请求间隔在 {p90_interval:.1f} 分钟内"
        elif ttl == 15:
            return f"中高频用户：75% 的请求间隔在 {p75_interval:.1f} 分钟内"
        elif ttl == 30:
            return f"中频用户：75% 的请求间隔在 {p75_interval:.1f} 分钟内"
        else:
            return f"低频用户：75% 的请求间隔为 {p75_interval:.1f} 分钟，建议使用长 TTL"

    @staticmethod
    def get_cache_hit_analysis(
        db: Session,
        user_id: Optional[str] = None,
        api_key_id: Optional[str] = None,
        hours: int = 168,
    ) -> Dict[str, Any]:
        """
        分析缓存命中情况

        Args:
            db: 数据库会话
            user_id: 指定用户 ID（可选）
            api_key_id: 指定 API Key ID（可选）
            hours: 分析最近多少小时的数据

        Returns:
            缓存命中分析结果
        """
        start_date = datetime.now(timezone.utc) - timedelta(hours=hours)

        # 基础查询
        query = db.query(
            func.count(Usage.id).label("total_requests"),
            func.sum(Usage.input_tokens).label("total_input_tokens"),
            func.sum(Usage.cache_read_input_tokens).label("total_cache_read_tokens"),
            func.sum(Usage.cache_creation_input_tokens).label("total_cache_creation_tokens"),
            func.sum(Usage.cache_read_cost_usd).label("total_cache_read_cost"),
            func.sum(Usage.cache_creation_cost_usd).label("total_cache_creation_cost"),
        ).filter(
            Usage.status == "completed",
            Usage.created_at >= start_date,
        )

        if user_id:
            query = query.filter(Usage.user_id == user_id)
        if api_key_id:
            query = query.filter(Usage.api_key_id == api_key_id)

        result = query.first()

        if result is None:
            total_requests = 0
            total_input_tokens = 0
            total_cache_read_tokens = 0
            total_cache_creation_tokens = 0
            total_cache_read_cost = 0.0
            total_cache_creation_cost = 0.0
        else:
            total_requests = result.total_requests or 0
            total_input_tokens = result.total_input_tokens or 0
            total_cache_read_tokens = result.total_cache_read_tokens or 0
            total_cache_creation_tokens = result.total_cache_creation_tokens or 0
            total_cache_read_cost = float(result.total_cache_read_cost or 0)
            total_cache_creation_cost = float(result.total_cache_creation_cost or 0)

        # 计算缓存命中率（按 token 数）
        # 总输入上下文 = input_tokens + cache_read_tokens（因为 input_tokens 不含 cache_read）
        # 或者如果 input_tokens 已经包含 cache_read，则直接用 input_tokens
        # 这里假设 cache_read_tokens 是额外的，命中率 = cache_read / (input + cache_read)
        total_context_tokens = total_input_tokens + total_cache_read_tokens
        cache_hit_rate = 0.0
        if total_context_tokens > 0:
            cache_hit_rate = total_cache_read_tokens / total_context_tokens * 100

        # 计算节省的费用
        # 缓存读取价格是正常输入价格的 10%，所以节省了 90%
        # 节省 = cache_read_tokens * (正常价格 - 缓存价格) = cache_read_cost * 9
        # 因为 cache_read_cost 是按 10% 价格算的，如果按 100% 算就是 10 倍
        estimated_savings = total_cache_read_cost * 9  # 节省了 90%

        # 统计有缓存命中的请求数
        requests_with_cache_hit = db.query(func.count(Usage.id)).filter(
            Usage.status == "completed",
            Usage.created_at >= start_date,
            Usage.cache_read_input_tokens > 0,
        )
        if user_id:
            requests_with_cache_hit = requests_with_cache_hit.filter(Usage.user_id == user_id)
        if api_key_id:
            requests_with_cache_hit = requests_with_cache_hit.filter(Usage.api_key_id == api_key_id)
        requests_with_cache_hit_count = int(requests_with_cache_hit.scalar() or 0)

        return {
            "analysis_period_hours": hours,
            "total_requests": total_requests,
            "requests_with_cache_hit": requests_with_cache_hit_count,
            "request_cache_hit_rate": round(requests_with_cache_hit_count / total_requests * 100, 2) if total_requests > 0 else 0,
            "total_input_tokens": total_input_tokens,
            "total_cache_read_tokens": total_cache_read_tokens,
            "total_cache_creation_tokens": total_cache_creation_tokens,
            "token_cache_hit_rate": round(cache_hit_rate, 2),
            "total_cache_read_cost_usd": round(total_cache_read_cost, 4),
            "total_cache_creation_cost_usd": round(total_cache_creation_cost, 4),
            "estimated_savings_usd": round(estimated_savings, 4),
        }

    @staticmethod
    def get_interval_timeline(
        db: Session,
        hours: int = 24,
        limit: int = 10000,
        user_id: Optional[str] = None,
        include_user_info: bool = False,
    ) -> Dict[str, Any]:
        """
        获取请求间隔时间线数据，用于散点图展示

        Args:
            db: 数据库会话
            hours: 分析最近多少小时的数据（默认24小时）
            limit: 最大返回数据点数量（默认10000）
            user_id: 指定用户 ID（可选，为空则返回所有用户）
            include_user_info: 是否包含用户信息（用于管理员多用户视图）

        Returns:
            包含时间线数据点的字典，每个数据点包含 model 字段用于按模型区分颜色
        """
        from sqlalchemy import text

        start_date = datetime.now(timezone.utc) - timedelta(hours=hours)

        # 构建用户过滤条件
        user_filter = "AND u.user_id = :user_id" if user_id else ""

        # 根据是否需要用户信息选择不同的查询
        if include_user_info and not user_id:
            # 管理员视图：返回带用户信息的数据点
            # 使用按比例采样，保持每个用户的数据量比例不变
            sql = text(f"""
                WITH request_intervals AS (
                    SELECT
                        u.created_at,
                        u.user_id,
                        u.model,
                        usr.username,
                        LAG(u.created_at) OVER (
                            PARTITION BY u.user_id
                            ORDER BY u.created_at
                        ) as prev_request_at
                    FROM usage u
                    LEFT JOIN users usr ON u.user_id = usr.id
                    WHERE u.status = 'completed'
                      AND u.created_at > :start_date
                      AND u.user_id IS NOT NULL
                      {user_filter}
                ),
                filtered_intervals AS (
                    SELECT
                        created_at,
                        user_id,
                        model,
                        username,
                        EXTRACT(EPOCH FROM (created_at - prev_request_at)) / 60.0 as interval_minutes,
                        ROW_NUMBER() OVER (PARTITION BY user_id ORDER BY created_at) as rn
                    FROM request_intervals
                    WHERE prev_request_at IS NOT NULL
                      AND EXTRACT(EPOCH FROM (created_at - prev_request_at)) / 60.0 <= 120
                ),
                total_count AS (
                    SELECT COUNT(*) as cnt FROM filtered_intervals
                ),
                user_totals AS (
                    SELECT user_id, COUNT(*) as user_cnt FROM filtered_intervals GROUP BY user_id
                ),
                user_limits AS (
                    SELECT
                        ut.user_id,
                        CASE WHEN tc.cnt <= :limit THEN ut.user_cnt
                             ELSE GREATEST(CEIL(ut.user_cnt::float * :limit / tc.cnt), 1)::int
                        END as user_limit
                    FROM user_totals ut, total_count tc
                )
                SELECT
                    fi.created_at,
                    fi.user_id,
                    fi.model,
                    fi.username,
                    fi.interval_minutes
                FROM filtered_intervals fi
                JOIN user_limits ul ON fi.user_id = ul.user_id
                WHERE fi.rn <= ul.user_limit
                ORDER BY fi.created_at
            """)
        else:
            # 普通视图：返回时间、间隔和模型信息
            sql = text(f"""
                WITH request_intervals AS (
                    SELECT
                        u.created_at,
                        u.user_id,
                        u.model,
                        LAG(u.created_at) OVER (
                            PARTITION BY u.user_id
                            ORDER BY u.created_at
                        ) as prev_request_at
                    FROM usage u
                    WHERE u.status = 'completed'
                      AND u.created_at > :start_date
                      AND u.user_id IS NOT NULL
                      {user_filter}
                )
                SELECT
                    created_at,
                    model,
                    EXTRACT(EPOCH FROM (created_at - prev_request_at)) / 60.0 as interval_minutes
                FROM request_intervals
                WHERE prev_request_at IS NOT NULL
                  AND EXTRACT(EPOCH FROM (created_at - prev_request_at)) / 60.0 <= 120
                ORDER BY created_at
                LIMIT :limit
            """)

        params: Dict[str, Any] = {"start_date": start_date, "limit": limit}
        if user_id:
            params["user_id"] = user_id

        result = db.execute(sql, params)
        rows = result.fetchall()

        # 转换为时间线数据点
        points = []
        users_map: Dict[str, str] = {}  # user_id -> username
        models_set: set = set()  # 收集所有出现的模型

        if include_user_info and not user_id:
            for row in rows:
                created_at, row_user_id, model, username, interval_minutes = row
                point_data: Dict[str, Any] = {
                    "x": created_at.isoformat(),
                    "y": round(float(interval_minutes), 2),
                    "user_id": str(row_user_id),
                }
                if model:
                    point_data["model"] = model
                    models_set.add(model)
                points.append(point_data)
                if row_user_id and username:
                    users_map[str(row_user_id)] = username
        else:
            for row in rows:
                created_at, model, interval_minutes = row
                point_data = {
                    "x": created_at.isoformat(),
                    "y": round(float(interval_minutes), 2)
                }
                if model:
                    point_data["model"] = model
                    models_set.add(model)
                points.append(point_data)

        response: Dict[str, Any] = {
            "analysis_period_hours": hours,
            "total_points": len(points),
            "points": points,
        }

        if include_user_info and not user_id:
            response["users"] = users_map

        # 如果有模型信息，返回模型列表
        if models_set:
            response["models"] = sorted(models_set)

        return response
