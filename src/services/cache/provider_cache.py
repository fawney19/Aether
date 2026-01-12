"""
Provider 缓存服务 - 减少 Provider 和 ProviderAPIKey 查询

用于缓存 Provider 的 billing_type 和 ProviderAPIKey 的 rate_multiplier，
这些数据在 UsageService.record_usage() 中被频繁查询但变化不频繁。
"""

from typing import Optional, Tuple

from sqlalchemy.orm import Session

from src.config.constants import CacheTTL
from src.core.cache_service import CacheService
from src.core.enums import ProviderBillingType
from src.core.logger import logger
from src.models.database import Provider, ProviderAPIKey


class ProviderCacheService:
    """Provider 缓存服务

    提供 Provider 和 ProviderAPIKey 的缓存查询功能，减少数据库访问。
    主要用于 UsageService 中获取费率倍数和计费类型。
    """

    CACHE_TTL = CacheTTL.PROVIDER  # 5 分钟

    @staticmethod
    async def get_provider_api_key_rate_multiplier(
        db: Session, provider_api_key_id: str, api_format: Optional[str] = None
    ) -> Optional[float]:
        """
        获取 ProviderAPIKey 的 rate_multiplier（带缓存）

        优先返回指定 API 格式的倍率，如果没有则返回默认倍率。

        Args:
            db: 数据库会话
            provider_api_key_id: ProviderAPIKey ID
            api_format: API 格式（可选），如 "CLAUDE"、"OPENAI"

        Returns:
            rate_multiplier 或 None（如果找不到）
        """
        # 缓存键包含 api_format
        format_suffix = api_format.upper() if api_format else "default"
        cache_key = f"provider_api_key:rate_multiplier:{provider_api_key_id}:{format_suffix}"

        # 1. 尝试从缓存获取
        cached_data = await CacheService.get(cache_key)
        if cached_data is not None:
            logger.debug(f"ProviderAPIKey rate_multiplier 缓存命中: {provider_api_key_id[:8]}... format={format_suffix}")
            # 缓存的 "NOT_FOUND" 表示数据库中不存在
            if cached_data == "NOT_FOUND":
                return None
            return float(cached_data)

        # 2. 缓存未命中，查询数据库
        provider_key = (
            db.query(ProviderAPIKey.rate_multiplier, ProviderAPIKey.rate_multipliers)
            .filter(ProviderAPIKey.id == provider_api_key_id)
            .first()
        )

        # 3. 计算倍率并写入缓存
        if provider_key:
            # 优先使用 rate_multipliers[api_format]，回退到 rate_multiplier
            rate_multiplier = provider_key.rate_multiplier or 1.0
            if api_format and provider_key.rate_multipliers:
                format_upper = api_format.upper()
                if format_upper in provider_key.rate_multipliers:
                    rate_multiplier = provider_key.rate_multipliers[format_upper]

            await CacheService.set(
                cache_key, rate_multiplier, ttl_seconds=ProviderCacheService.CACHE_TTL
            )
            logger.debug(f"ProviderAPIKey rate_multiplier 已缓存: {provider_api_key_id[:8]}... format={format_suffix} value={rate_multiplier}")
            return rate_multiplier
        else:
            # 缓存负结果
            await CacheService.set(
                cache_key, "NOT_FOUND", ttl_seconds=ProviderCacheService.CACHE_TTL
            )
            return None

    @staticmethod
    async def get_provider_billing_type(
        db: Session, provider_id: str
    ) -> Optional[ProviderBillingType]:
        """
        获取 Provider 的 billing_type（带缓存）

        Args:
            db: 数据库会话
            provider_id: Provider ID

        Returns:
            billing_type 或 None（如果找不到）
        """
        cache_key = f"provider:billing_type:{provider_id}"

        # 1. 尝试从缓存获取
        cached_data = await CacheService.get(cache_key)
        if cached_data is not None:
            logger.debug(f"Provider billing_type 缓存命中: {provider_id[:8]}...")
            if cached_data == "NOT_FOUND":
                return None
            try:
                return ProviderBillingType(cached_data)
            except ValueError:
                # 缓存值无效，删除并重新查询
                await CacheService.delete(cache_key)

        # 2. 缓存未命中，查询数据库
        provider = (
            db.query(Provider.billing_type).filter(Provider.id == provider_id).first()
        )

        # 3. 写入缓存
        if provider:
            billing_type = provider.billing_type
            await CacheService.set(
                cache_key, billing_type.value, ttl_seconds=ProviderCacheService.CACHE_TTL
            )
            logger.debug(f"Provider billing_type 已缓存: {provider_id[:8]}...")
            return billing_type
        else:
            # 缓存负结果
            await CacheService.set(
                cache_key, "NOT_FOUND", ttl_seconds=ProviderCacheService.CACHE_TTL
            )
            return None

    @staticmethod
    async def get_rate_multiplier_and_free_tier(
        db: Session,
        provider_api_key_id: Optional[str],
        provider_id: Optional[str],
        api_format: Optional[str] = None,
    ) -> Tuple[float, bool]:
        """
        获取费率倍数和是否免费套餐（带缓存）

        这是 UsageService._get_rate_multiplier_and_free_tier 的缓存版本。

        Args:
            db: 数据库会话
            provider_api_key_id: ProviderAPIKey ID（可选）
            provider_id: Provider ID（可选）
            api_format: API 格式（可选），用于获取按格式配置的倍率

        Returns:
            (rate_multiplier, is_free_tier) 元组
        """
        actual_rate_multiplier = 1.0
        is_free_tier = False

        # 获取费率倍数（支持按 API 格式查询）
        if provider_api_key_id:
            rate_multiplier = await ProviderCacheService.get_provider_api_key_rate_multiplier(
                db, provider_api_key_id, api_format
            )
            if rate_multiplier is not None:
                actual_rate_multiplier = rate_multiplier

        # 获取计费类型
        if provider_id:
            billing_type = await ProviderCacheService.get_provider_billing_type(db, provider_id)
            if billing_type == ProviderBillingType.FREE_TIER:
                is_free_tier = True

        return actual_rate_multiplier, is_free_tier

    @staticmethod
    async def invalidate_provider_api_key_cache(provider_api_key_id: str) -> None:
        """清除 ProviderAPIKey 缓存（包括所有 API 格式的缓存）"""
        # 使用模式匹配删除所有格式的缓存
        await CacheService.delete_pattern(f"provider_api_key:rate_multiplier:{provider_api_key_id}:*")
        logger.debug(f"ProviderAPIKey 缓存已清除: {provider_api_key_id[:8]}...")

    @staticmethod
    async def invalidate_provider_cache(provider_id: str) -> None:
        """清除 Provider 缓存"""
        await CacheService.delete(f"provider:billing_type:{provider_id}")
        logger.debug(f"Provider 缓存已清除: {provider_id[:8]}...")
