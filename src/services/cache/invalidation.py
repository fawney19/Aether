"""
缓存失效服务

统一管理各种缓存的失效逻辑
"""

from typing import Optional

from src.core.logger import logger


class CacheInvalidationService:
    """缓存失效服务"""

    def __init__(self):
        self._model_mappers = []

    def register_model_mapper(self, model_mapper):
        """注册 ModelMapper 实例"""
        if model_mapper not in self._model_mappers:
            self._model_mappers.append(model_mapper)

    async def on_global_model_changed(
        self, model_name: str, global_model_id: Optional[str] = None
    ) -> None:
        """
        GlobalModel 变更时的缓存失效

        Args:
            model_name: 变更的 GlobalModel.name
            global_model_id: GlobalModel ID（可选）
        """
        logger.info(f"[CacheInvalidation] GlobalModel 变更: {model_name}")

        # 1. 清空正则缓存
        from src.core.model_permissions import clear_regex_cache

        clear_regex_cache()

        # 2. 清空 ModelMapper 缓存
        for mapper in self._model_mappers:
            mapper.clear_cache()

        # 3. 清空 ModelCacheService 缓存
        from src.services.cache.model_cache import ModelCacheService

        try:
            await ModelCacheService.invalidate_global_model_cache(
                global_model_id=global_model_id or "", name=model_name
            )
        except Exception as e:
            logger.error(f"[CacheInvalidation] 失效 ModelCacheService 缓存失败: {e}")

    def on_model_changed(self, provider_id: str, global_model_id: str):
        """Model 变更时的缓存失效"""
        for mapper in self._model_mappers:
            mapper.refresh_cache(provider_id)

    def clear_all_caches(self):
        """清空所有缓存"""
        for mapper in self._model_mappers:
            mapper.clear_cache()


# 全局单例
_cache_invalidation_service: Optional[CacheInvalidationService] = None


def get_cache_invalidation_service() -> CacheInvalidationService:
    """获取全局缓存失效服务实例"""
    global _cache_invalidation_service

    if _cache_invalidation_service is None:
        _cache_invalidation_service = CacheInvalidationService()

    return _cache_invalidation_service
