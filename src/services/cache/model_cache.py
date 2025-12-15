"""
Model 映射缓存服务 - 减少模型查询
"""

import json
import time
from typing import Optional

from sqlalchemy.exc import OperationalError, ProgrammingError
from sqlalchemy.orm import Session

from src.config.constants import CacheTTL
from src.core.cache_service import CacheService
from src.core.logger import logger
from src.core.metrics import (
    model_mapping_conflict_total,
    model_mapping_resolution_duration_seconds,
    model_mapping_resolution_total,
)
from src.models.database import GlobalModel, Model


class ModelCacheService:
    """Model 映射缓存服务"""

    # 缓存 TTL（秒）- 使用统一常量
    CACHE_TTL = CacheTTL.MODEL

    @staticmethod
    async def get_model_by_id(db: Session, model_id: str) -> Optional[Model]:
        """
        获取 Model（带缓存）

        Args:
            db: 数据库会话
            model_id: Model ID

        Returns:
            Model 对象或 None
        """
        cache_key = f"model:id:{model_id}"

        # 1. 尝试从缓存获取
        cached_data = await CacheService.get(cache_key)
        if cached_data:
            logger.debug(f"Model 缓存命中: {model_id}")
            return ModelCacheService._dict_to_model(cached_data)

        # 2. 缓存未命中，查询数据库
        model = db.query(Model).filter(Model.id == model_id).first()

        # 3. 写入缓存
        if model:
            model_dict = ModelCacheService._model_to_dict(model)
            await CacheService.set(cache_key, model_dict, ttl_seconds=ModelCacheService.CACHE_TTL)
            logger.debug(f"Model 已缓存: {model_id}")

        return model

    @staticmethod
    async def get_global_model_by_id(db: Session, global_model_id: str) -> Optional[GlobalModel]:
        """
        获取 GlobalModel（带缓存）

        Args:
            db: 数据库会话
            global_model_id: GlobalModel ID

        Returns:
            GlobalModel 对象或 None
        """
        cache_key = f"global_model:id:{global_model_id}"

        # 1. 尝试从缓存获取
        cached_data = await CacheService.get(cache_key)
        if cached_data:
            logger.debug(f"GlobalModel 缓存命中: {global_model_id}")
            return ModelCacheService._dict_to_global_model(cached_data)

        # 2. 缓存未命中，查询数据库
        global_model = db.query(GlobalModel).filter(GlobalModel.id == global_model_id).first()

        # 3. 写入缓存
        if global_model:
            global_model_dict = ModelCacheService._global_model_to_dict(global_model)
            await CacheService.set(
                cache_key, global_model_dict, ttl_seconds=ModelCacheService.CACHE_TTL
            )
            logger.debug(f"GlobalModel 已缓存: {global_model_id}")

        return global_model

    @staticmethod
    async def get_model_by_provider_and_global_model(
        db: Session, provider_id: str, global_model_id: str
    ) -> Optional[Model]:
        """
        通过 Provider ID 和 GlobalModel ID 获取 Model（带缓存）

        Args:
            db: 数据库会话
            provider_id: Provider ID
            global_model_id: GlobalModel ID

        Returns:
            Model 对象或 None
        """
        cache_key = f"model:provider_global:{provider_id}:{global_model_id}"

        # 1. 尝试从缓存获取
        cached_data = await CacheService.get(cache_key)
        if cached_data:
            logger.debug(
                f"Model 缓存命中(provider+global): {provider_id[:8]}...+{global_model_id[:8]}..."
            )
            return ModelCacheService._dict_to_model(cached_data)

        # 2. 缓存未命中，查询数据库
        model = (
            db.query(Model)
            .filter(
                Model.provider_id == provider_id,
                Model.global_model_id == global_model_id,
                Model.is_active == True,
            )
            .first()
        )

        # 3. 写入缓存
        if model:
            model_dict = ModelCacheService._model_to_dict(model)
            await CacheService.set(cache_key, model_dict, ttl_seconds=ModelCacheService.CACHE_TTL)
            logger.debug(
                f"Model 已缓存(provider+global): {provider_id[:8]}...+{global_model_id[:8]}..."
            )

        return model

    @staticmethod
    async def get_global_model_by_name(db: Session, name: str) -> Optional[GlobalModel]:
        """
        通过名称获取 GlobalModel（带缓存）

        Args:
            db: 数据库会话
            name: GlobalModel 名称

        Returns:
            GlobalModel 对象或 None
        """
        cache_key = f"global_model:name:{name}"

        # 1. 尝试从缓存获取
        cached_data = await CacheService.get(cache_key)
        if cached_data:
            logger.debug(f"GlobalModel 缓存命中(名称): {name}")
            return ModelCacheService._dict_to_global_model(cached_data)

        # 2. 缓存未命中，查询数据库
        global_model = db.query(GlobalModel).filter(GlobalModel.name == name).first()

        # 3. 写入缓存
        if global_model:
            global_model_dict = ModelCacheService._global_model_to_dict(global_model)
            await CacheService.set(
                cache_key, global_model_dict, ttl_seconds=ModelCacheService.CACHE_TTL
            )
            logger.debug(f"GlobalModel 已缓存(名称): {name}")

        return global_model

    @staticmethod
    async def invalidate_model_cache(
        model_id: str,
        provider_id: Optional[str] = None,
        global_model_id: Optional[str] = None,
        provider_model_name: Optional[str] = None,
        provider_model_aliases: Optional[list] = None,
    ) -> None:
        """清除 Model 缓存

        Args:
            model_id: Model ID
            provider_id: Provider ID（用于清除 provider_global 缓存）
            global_model_id: GlobalModel ID（用于清除 provider_global 缓存）
            provider_model_name: provider_model_name（用于清除 resolve 缓存）
            provider_model_aliases: 映射名称列表（用于清除 resolve 缓存）
        """
        # 清除 model:id 缓存
        await CacheService.delete(f"model:id:{model_id}")

        # 清除 provider_global 缓存（如果提供了必要参数）
        if provider_id and global_model_id:
            await CacheService.delete(f"model:provider_global:{provider_id}:{global_model_id}")
            logger.debug(
                f"Model 缓存已清除: {model_id}, provider_global:{provider_id[:8]}...:{global_model_id[:8]}..."
            )
        else:
            logger.debug(f"Model 缓存已清除: {model_id}")

        # 清除 resolve 缓存（provider_model_name 和 aliases 可能都被用作解析 key）
        resolve_keys_to_clear = []
        if provider_model_name:
            resolve_keys_to_clear.append(provider_model_name)
        if provider_model_aliases:
            for alias_entry in provider_model_aliases:
                if isinstance(alias_entry, dict):
                    alias_name = alias_entry.get("name", "").strip()
                    if alias_name:
                        resolve_keys_to_clear.append(alias_name)

        for key in resolve_keys_to_clear:
            await CacheService.delete(f"global_model:resolve:{key}")

        if resolve_keys_to_clear:
            logger.debug(f"Model resolve 缓存已清除: {resolve_keys_to_clear}")

    @staticmethod
    async def invalidate_global_model_cache(global_model_id: str, name: Optional[str] = None) -> None:
        """清除 GlobalModel 缓存"""
        await CacheService.delete(f"global_model:id:{global_model_id}")
        if name:
            await CacheService.delete(f"global_model:name:{name}")
            # 同时清除 resolve 缓存，因为 GlobalModel.name 也是一个 resolve key
            await CacheService.delete(f"global_model:resolve:{name}")
        logger.debug(f"GlobalModel 缓存已清除: {global_model_id}")

    @staticmethod
    async def resolve_global_model_by_name_or_alias(
        db: Session, model_name: str
    ) -> Optional[GlobalModel]:
        """
        通过名称或映射解析 GlobalModel（带缓存，支持映射匹配）

        查找顺序：
        1. 检查缓存
        2. 通过映射匹配（查询 Model 表的 provider_model_name 和 provider_model_aliases）
        3. 直接匹配 GlobalModel.name（兜底）

        Args:
            db: 数据库会话
            model_name: 模型名称（可以是 GlobalModel.name 或映射名称）

        Returns:
            GlobalModel 对象或 None
        """
        start_time = time.time()
        resolution_method = "not_found"
        cache_hit = False

        normalized_name = model_name.strip()
        if not normalized_name:
            return None

        cache_key = f"global_model:resolve:{normalized_name}"

        try:
            # 1. 尝试从缓存获取
            cached_data = await CacheService.get(cache_key)
            if cached_data:
                if cached_data == "NOT_FOUND":
                    # 缓存的负结果
                    cache_hit = True
                    resolution_method = "not_found"
                    logger.debug(f"GlobalModel 缓存命中(映射解析-未找到): {normalized_name}")
                    return None
                if isinstance(cached_data, dict) and "supported_capabilities" not in cached_data:
                    # 兼容旧缓存：字段不全时视为未命中，走 DB 刷新
                    logger.debug(f"GlobalModel 缓存命中但 schema 过旧，刷新: {normalized_name}")
                else:
                    cache_hit = True
                    resolution_method = "direct_match"  # 缓存命中时无法区分原始解析方式
                    logger.debug(f"GlobalModel 缓存命中(映射解析): {normalized_name}")
                    return ModelCacheService._dict_to_global_model(cached_data)

            # 2. 优先通过 provider_model_name 和映射名称匹配（Provider 配置优先级最高）
            from sqlalchemy import or_

            from src.models.database import Provider

            # 构建精确的映射匹配条件
            # 注意：provider_model_aliases 是 JSONB 数组，需要使用 PostgreSQL 的 JSONB 操作符
            # 对于 SQLite，会在 Python 层面进行过滤
            try:
                # 尝试使用 PostgreSQL 的 JSONB 查询（更高效）
                # 使用 json.dumps 确保正确转义特殊字符，避免 SQL 注入
                jsonb_pattern = json.dumps([{"name": normalized_name}])
                models_with_global = (
                    db.query(Model, GlobalModel)
                    .join(Provider, Model.provider_id == Provider.id)
                    .join(GlobalModel, Model.global_model_id == GlobalModel.id)
                    .filter(
                        Provider.is_active == True,
                        Model.is_active == True,
                        GlobalModel.is_active == True,
                        or_(
                            Model.provider_model_name == normalized_name,
                            # PostgreSQL JSONB 查询：检查数组中是否有包含 {"name": "xxx"} 的元素
                            Model.provider_model_aliases.op("@>")(jsonb_pattern),
                        ),
                    )
                    .all()
                )
            except (OperationalError, ProgrammingError) as e:
                # JSONB 操作符不支持（如 SQLite），回退到加载匹配 provider_model_name 的 Model
                # 并在 Python 层过滤 aliases
                logger.debug(
                    f"JSONB 查询失败，回退到 Python 过滤: {e}",
                )
                # 优化：先用 provider_model_name 缩小范围，再加载其他可能匹配的记录
                models_with_global = (
                    db.query(Model, GlobalModel)
                    .join(Provider, Model.provider_id == Provider.id)
                    .join(GlobalModel, Model.global_model_id == GlobalModel.id)
                    .filter(
                        Provider.is_active == True,
                        Model.is_active == True,
                        GlobalModel.is_active == True,
                    )
                    .all()
                )

            # 用于存储匹配结果：{(model_id, global_model_id): (GlobalModel, match_type, priority)}
            # 使用字典去重，同一个 Model 只保留优先级最高的匹配
            matched_models_dict = {}

            # 遍历查询结果进行匹配
            for model, gm in models_with_global:
                key = (model.id, gm.id)

                # 检查 provider_model_aliases 是否匹配（优先级更高）
                if model.provider_model_aliases:
                    for alias_entry in model.provider_model_aliases:
                        if isinstance(alias_entry, dict):
                            alias_name = alias_entry.get("name", "").strip()
                            if alias_name == normalized_name:
                                # alias 优先级为 0（最高），覆盖任何已存在的匹配
                                matched_models_dict[key] = (gm, "alias", 0)
                                logger.debug(
                                    f"模型名称 '{normalized_name}' 通过映射名称匹配到 "
                                    f"GlobalModel: {gm.name} (Model: {model.id[:8]}...)"
                                )
                                break

                # 如果还没有匹配（或只有 provider_model_name 匹配），检查 provider_model_name
                if key not in matched_models_dict or matched_models_dict[key][1] != "alias":
                    if model.provider_model_name == normalized_name:
                        # provider_model_name 优先级为 1（兜底），只在没有 alias 匹配时使用
                        if key not in matched_models_dict:
                            matched_models_dict[key] = (gm, "provider_model_name", 1)
                            logger.debug(
                                f"模型名称 '{normalized_name}' 通过 provider_model_name 匹配到 "
                                f"GlobalModel: {gm.name} (Model: {model.id[:8]}...)"
                            )

            # 如果通过 provider_model_name/alias 找到了，直接返回
            if matched_models_dict:
                # 转换为列表并排序：按 priority（alias=0 优先）、然后按 GlobalModel.name
                matched_global_models = [
                    (gm, match_type) for gm, match_type, priority in matched_models_dict.values()
                ]
                matched_global_models.sort(
                    key=lambda item: (
                        0 if item[1] == "alias" else 1,  # alias 优先
                        item[0].name  # 同优先级按名称排序（确定性）
                    )
                )

                # 记录解析方式
                resolution_method = matched_global_models[0][1]

                if len(matched_global_models) > 1:
                    # 检测到冲突
                    unique_models = {gm.id: gm for gm, _ in matched_global_models}
                    if len(unique_models) > 1:
                        model_names = [gm.name for gm in unique_models.values()]
                        logger.warning(
                            f"模型映射冲突: 名称 '{normalized_name}' 匹配到多个不同的 GlobalModel: "
                            f"{', '.join(model_names)}，使用第一个匹配结果"
                        )
                        # 记录冲突指标
                        model_mapping_conflict_total.inc()

                # 返回第一个匹配的 GlobalModel
                result_global_model: GlobalModel = matched_global_models[0][0]
                global_model_dict = ModelCacheService._global_model_to_dict(result_global_model)
                await CacheService.set(
                    cache_key, global_model_dict, ttl_seconds=ModelCacheService.CACHE_TTL
                )
                logger.debug(
                    f"GlobalModel 已缓存(映射解析-{resolution_method}): {normalized_name} -> {result_global_model.name}"
                )
                return result_global_model

            # 3. 如果通过 provider 映射没找到，最后尝试直接通过 GlobalModel.name 查找
            global_model = (
                db.query(GlobalModel)
                .filter(GlobalModel.name == normalized_name, GlobalModel.is_active == True)
                .first()
            )

            if global_model:
                resolution_method = "direct_match"
                # 缓存结果
                global_model_dict = ModelCacheService._global_model_to_dict(global_model)
                await CacheService.set(
                    cache_key, global_model_dict, ttl_seconds=ModelCacheService.CACHE_TTL
                )
                logger.debug(f"GlobalModel 已缓存(映射解析-直接匹配): {normalized_name}")
                return global_model

            # 4. 完全未找到
            resolution_method = "not_found"
            # 未找到匹配，缓存负结果
            await CacheService.set(
                cache_key, "NOT_FOUND", ttl_seconds=ModelCacheService.CACHE_TTL
            )
            logger.debug(f"GlobalModel 未找到(映射解析): {normalized_name}")
            return None

        finally:
            # 记录监控指标
            duration = time.time() - start_time
            model_mapping_resolution_total.labels(
                method=resolution_method, cache_hit=str(cache_hit).lower()
            ).inc()
            model_mapping_resolution_duration_seconds.labels(method=resolution_method).observe(
                duration
            )

    @staticmethod
    def _model_to_dict(model: Model) -> dict:
        """将 Model 对象转换为字典"""
        return {
            "id": model.id,
            "provider_id": model.provider_id,
            "global_model_id": model.global_model_id,
            "provider_model_name": model.provider_model_name,
            "provider_model_aliases": getattr(model, "provider_model_aliases", None),
            "is_active": model.is_active,
            "is_available": model.is_available if hasattr(model, "is_available") else True,
            "price_per_request": (
                float(model.price_per_request) if model.price_per_request else None
            ),
            "tiered_pricing": model.tiered_pricing,
            "supports_vision": model.supports_vision,
            "supports_function_calling": model.supports_function_calling,
            "supports_streaming": model.supports_streaming,
            "supports_extended_thinking": model.supports_extended_thinking,
            "supports_image_generation": getattr(model, "supports_image_generation", None),
            "config": model.config,
        }

    @staticmethod
    def _dict_to_model(model_dict: dict) -> Model:
        """从字典重建 Model 对象"""
        model = Model(
            id=model_dict["id"],
            provider_id=model_dict["provider_id"],
            global_model_id=model_dict["global_model_id"],
            provider_model_name=model_dict["provider_model_name"],
            provider_model_aliases=model_dict.get("provider_model_aliases"),
            is_active=model_dict["is_active"],
            is_available=model_dict.get("is_available", True),
            price_per_request=model_dict.get("price_per_request"),
            tiered_pricing=model_dict.get("tiered_pricing"),
            supports_vision=model_dict.get("supports_vision"),
            supports_function_calling=model_dict.get("supports_function_calling"),
            supports_streaming=model_dict.get("supports_streaming"),
            supports_extended_thinking=model_dict.get("supports_extended_thinking"),
            supports_image_generation=model_dict.get("supports_image_generation"),
            config=model_dict.get("config"),
        )
        return model

    @staticmethod
    def _global_model_to_dict(global_model: GlobalModel) -> dict:
        """将 GlobalModel 对象转换为字典"""
        return {
            "id": global_model.id,
            "name": global_model.name,
            "display_name": global_model.display_name,
            "default_supports_vision": global_model.default_supports_vision,
            "default_supports_function_calling": global_model.default_supports_function_calling,
            "default_supports_streaming": global_model.default_supports_streaming,
            "default_supports_extended_thinking": global_model.default_supports_extended_thinking,
            "default_supports_image_generation": global_model.default_supports_image_generation,
            "supported_capabilities": global_model.supported_capabilities,
            "is_active": global_model.is_active,
            "description": global_model.description,
        }

    @staticmethod
    def _dict_to_global_model(global_model_dict: dict) -> GlobalModel:
        """从字典重建 GlobalModel 对象"""
        global_model = GlobalModel(
            id=global_model_dict["id"],
            name=global_model_dict["name"],
            display_name=global_model_dict.get("display_name"),
            default_supports_vision=global_model_dict.get("default_supports_vision", False),
            default_supports_function_calling=global_model_dict.get(
                "default_supports_function_calling", False
            ),
            default_supports_streaming=global_model_dict.get("default_supports_streaming", True),
            default_supports_extended_thinking=global_model_dict.get(
                "default_supports_extended_thinking", False
            ),
            default_supports_image_generation=global_model_dict.get(
                "default_supports_image_generation", False
            ),
            supported_capabilities=global_model_dict.get("supported_capabilities") or [],
            is_active=global_model_dict.get("is_active", True),
            description=global_model_dict.get("description"),
        )
        return global_model
