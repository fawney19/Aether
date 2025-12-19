"""
模型管理服务
"""

import asyncio
from typing import List, Optional

from sqlalchemy import and_
from sqlalchemy.exc import IntegrityError
from sqlalchemy.orm import Session

from src.core.exceptions import InvalidRequestException, NotFoundException
from src.core.logger import logger
from src.models.api import ModelCreate, ModelResponse, ModelUpdate
from src.models.database import Model, Provider
from src.api.base.models_service import invalidate_models_list_cache
from src.services.cache.invalidation import get_cache_invalidation_service
from src.services.cache.model_cache import ModelCacheService



class ModelService:
    """模型管理服务"""

    @staticmethod
    def create_model(db: Session, provider_id: str, model_data: ModelCreate) -> Model:
        """创建模型"""
        # 检查提供商是否存在
        provider = db.query(Provider).filter(Provider.id == provider_id).first()
        if not provider:
            raise NotFoundException(f"提供商 {provider_id} 不存在")

        # 检查同一提供商下是否已存在同名模型
        existing = (
            db.query(Model)
            .filter(
                and_(
                    Model.provider_id == provider_id,
                    Model.provider_model_name == model_data.provider_model_name,
                )
            )
            .first()
        )
        if existing:
            raise InvalidRequestException(
                f"提供商 {provider.name} 下已存在模型 {model_data.provider_model_name}"
            )

        try:
            model = Model(
                provider_id=provider_id,
                global_model_id=model_data.global_model_id,
                provider_model_name=model_data.provider_model_name,
                provider_model_aliases=model_data.provider_model_aliases,
                price_per_request=model_data.price_per_request,
                tiered_pricing=model_data.tiered_pricing,
                supports_vision=model_data.supports_vision,
                supports_function_calling=model_data.supports_function_calling,
                supports_streaming=model_data.supports_streaming,
                supports_extended_thinking=model_data.supports_extended_thinking,
                is_active=model_data.is_active if model_data.is_active is not None else True,
                config=model_data.config,
            )
            db.add(model)
            db.commit()
            db.refresh(model)
            # 显式加载 global_model 关系
            if model.global_model_id:
                from sqlalchemy.orm import joinedload

                model = (
                    db.query(Model)
                    .options(joinedload(Model.global_model))
                    .filter(Model.id == model.id)
                    .first()
                )

            logger.info(f"创建模型成功: provider={provider.name}, model={model.provider_model_name}, global_model_id={model.global_model_id}")

            # 清除 /v1/models 列表缓存
            asyncio.create_task(invalidate_models_list_cache())

            return model

        except IntegrityError as e:
            db.rollback()
            logger.error(f"创建模型失败: {str(e)}")
            raise InvalidRequestException("创建模型失败，请检查输入数据")

    @staticmethod
    def get_model(db: Session, model_id: str) -> Model:  # UUID
        """获取模型详情"""
        from sqlalchemy.orm import joinedload

        model = (
            db.query(Model)
            .options(joinedload(Model.global_model))
            .filter(Model.id == model_id)
            .first()
        )
        if not model:
            raise NotFoundException(f"模型 {model_id} 不存在")
        return model

    @staticmethod
    def get_models_by_provider(
        db: Session,
        provider_id: str,  # UUID
        skip: int = 0,
        limit: int = 100,
        is_active: Optional[bool] = None,
    ) -> List[Model]:
        """获取提供商的模型列表"""
        from sqlalchemy.orm import joinedload

        query = (
            db.query(Model)
            .options(joinedload(Model.global_model))
            .filter(Model.provider_id == provider_id)
        )

        if is_active is not None:
            query = query.filter(Model.is_active == is_active)

        # 按创建时间排序
        query = query.order_by(Model.created_at.desc())

        return query.offset(skip).limit(limit).all()

    @staticmethod
    def get_all_models(
        db: Session,
        skip: int = 0,
        limit: int = 100,
        is_active: Optional[bool] = None,
        category: Optional[str] = None,
    ) -> List[Model]:
        """获取所有模型列表"""
        query = db.query(Model)

        if is_active is not None:
            query = query.filter(Model.is_active == is_active)

        # 按提供商和创建时间排序
        query = query.order_by(Model.provider_id, Model.created_at.desc())

        return query.offset(skip).limit(limit).all()

    @staticmethod
    def update_model(db: Session, model_id: str, model_data: ModelUpdate) -> Model:  # UUID
        """更新模型"""
        model = db.query(Model).filter(Model.id == model_id).first()
        if not model:
            raise NotFoundException(f"模型 {model_id} 不存在")

        # 保存旧的别名，用于清除缓存
        old_provider_model_name = model.provider_model_name
        old_provider_model_aliases = model.provider_model_aliases

        # 更新字段
        update_data = model_data.model_dump(exclude_unset=True)

        # 添加调试日志
        logger.debug(f"更新模型 {model_id} 收到的数据: {update_data}")
        logger.debug(f"更新前的 supports_vision: {model.supports_vision}, supports_function_calling: {model.supports_function_calling}, supports_extended_thinking: {model.supports_extended_thinking}")

        for field, value in update_data.items():
            setattr(model, field, value)

        logger.debug(f"更新后的 supports_vision: {model.supports_vision}, supports_function_calling: {model.supports_function_calling}, supports_extended_thinking: {model.supports_extended_thinking}")

        try:
            db.commit()
            db.refresh(model)

            # 清除 Redis 缓存（异步执行，不阻塞返回）
            # 先清除旧的别名缓存
            asyncio.create_task(
                ModelCacheService.invalidate_model_cache(
                    model_id=model.id,
                    provider_id=model.provider_id,
                    global_model_id=model.global_model_id,
                    provider_model_name=old_provider_model_name,
                    provider_model_aliases=old_provider_model_aliases,
                )
            )
            # 再清除新的别名缓存（如果有变化）
            if (model.provider_model_name != old_provider_model_name or
                model.provider_model_aliases != old_provider_model_aliases):
                asyncio.create_task(
                    ModelCacheService.invalidate_model_cache(
                        model_id=model.id,
                        provider_id=model.provider_id,
                        global_model_id=model.global_model_id,
                        provider_model_name=model.provider_model_name,
                        provider_model_aliases=model.provider_model_aliases,
                    )
                )

            # 清除内存缓存（ModelMapperMiddleware 实例）
            if model.provider_id and model.global_model_id:
                cache_service = get_cache_invalidation_service()
                cache_service.on_model_changed(model.provider_id, model.global_model_id)

            # 清除 /v1/models 列表缓存
            asyncio.create_task(invalidate_models_list_cache())

            logger.info(f"更新模型成功: id={model_id}, 最终 supports_vision: {model.supports_vision}, supports_function_calling: {model.supports_function_calling}, supports_extended_thinking: {model.supports_extended_thinking}")
            return model
        except IntegrityError as e:
            db.rollback()
            logger.error(f"更新模型失败: {str(e)}")
            raise InvalidRequestException("更新模型失败，请检查输入数据")

    @staticmethod
    def delete_model(db: Session, model_id: str):  # UUID
        """删除模型

        新架构删除逻辑：
        - Model 只是 Provider 对 GlobalModel 的实现，删除不影响 GlobalModel
        - 检查是否是该 GlobalModel 的最后一个实现（如果是，警告但允许删除）
        """
        model = db.query(Model).filter(Model.id == model_id).first()
        if not model:
            raise NotFoundException(f"模型 {model_id} 不存在")

        # 检查这是否是该 GlobalModel 的最后一个关联提供商
        if model.global_model_id:
            other_implementations = (
                db.query(Model)
                .filter(
                    Model.global_model_id == model.global_model_id,
                    Model.id != model_id,
                    Model.is_active == True,
                )
                .count()
            )

            if other_implementations == 0:
                logger.warning(f"警告：删除模型 {model_id}（Provider: {model.provider_id[:8]}...）后，"
                    f"GlobalModel '{model.global_model_id}' 将没有任何活跃的关联提供商")

        # 保存缓存清除所需的信息（删除后无法访问）
        cache_info = {
            "model_id": model.id,
            "provider_id": model.provider_id,
            "global_model_id": model.global_model_id,
            "provider_model_name": model.provider_model_name,
            "provider_model_aliases": model.provider_model_aliases,
        }

        try:
            db.delete(model)
            db.commit()

            # 清除 Redis 缓存
            asyncio.create_task(
                ModelCacheService.invalidate_model_cache(
                    model_id=cache_info["model_id"],
                    provider_id=cache_info["provider_id"],
                    global_model_id=cache_info["global_model_id"],
                    provider_model_name=cache_info["provider_model_name"],
                    provider_model_aliases=cache_info["provider_model_aliases"],
                )
            )

            # 清除内存缓存
            if cache_info["provider_id"] and cache_info["global_model_id"]:
                cache_service = get_cache_invalidation_service()
                cache_service.on_model_changed(cache_info["provider_id"], cache_info["global_model_id"])

            # 清除 /v1/models 列表缓存
            asyncio.create_task(invalidate_models_list_cache())

            logger.info(f"删除模型成功: id={model_id}, provider_model_name={cache_info['provider_model_name']}, "
                f"global_model_id={cache_info['global_model_id'][:8] if cache_info['global_model_id'] else 'None'}...")
        except Exception as e:
            db.rollback()
            logger.error(f"删除模型失败: {str(e)}")
            raise InvalidRequestException("删除模型失败")

    @staticmethod
    def toggle_model_availability(db: Session, model_id: str, is_available: bool) -> Model:  # UUID
        """切换模型可用状态"""
        model = db.query(Model).filter(Model.id == model_id).first()
        if not model:
            raise NotFoundException(f"模型 {model_id} 不存在")

        model.is_available = is_available
        db.commit()
        db.refresh(model)

        # 清除 Redis 缓存
        asyncio.create_task(
            ModelCacheService.invalidate_model_cache(
                model_id=model.id,
                provider_id=model.provider_id,
                global_model_id=model.global_model_id,
                provider_model_name=model.provider_model_name,
                provider_model_aliases=model.provider_model_aliases,
            )
        )

        # 清除内存缓存（ModelMapperMiddleware 实例）
        if model.provider_id and model.global_model_id:
            cache_service = get_cache_invalidation_service()
            cache_service.on_model_changed(model.provider_id, model.global_model_id)

        # 清除 /v1/models 列表缓存
        asyncio.create_task(invalidate_models_list_cache())

        status = "可用" if is_available else "不可用"
        logger.info(f"更新模型可用状态: id={model_id}, status={status}")
        return model

    @staticmethod
    def get_model_by_name(db: Session, provider_id: str, model_name: str) -> Optional[Model]:
        """根据 provider_model_name 获取模型"""
        return (
            db.query(Model)
            .filter(and_(Model.provider_id == provider_id, Model.provider_model_name == model_name))
            .first()
        )

    @staticmethod
    def batch_create_models(
        db: Session, provider_id: str, models_data: List[ModelCreate]
    ) -> List[Model]:  # UUID
        """批量创建模型"""
        # 检查提供商是否存在
        provider = db.query(Provider).filter(Provider.id == provider_id).first()
        if not provider:
            raise NotFoundException(f"提供商 {provider_id} 不存在")

        created_models = []
        for model_data in models_data:
            # 检查是否已存在
            existing = (
                db.query(Model)
                .filter(
                    and_(
                        Model.provider_id == provider_id,
                        Model.provider_model_name == model_data.provider_model_name,
                    )
                )
                .first()
            )

            if existing:
                logger.warning(f"模型 {model_data.provider_model_name} 已存在，跳过创建")
                continue

            model = Model(
                provider_id=provider_id,
                global_model_id=model_data.global_model_id,
                provider_model_name=model_data.provider_model_name,
                price_per_request=model_data.price_per_request,
                tiered_pricing=model_data.tiered_pricing,
                supports_vision=model_data.supports_vision,
                supports_function_calling=model_data.supports_function_calling,
                supports_streaming=model_data.supports_streaming,
                supports_extended_thinking=model_data.supports_extended_thinking,
                is_active=model_data.is_active,
                config=model_data.config,
            )
            db.add(model)
            created_models.append(model)

        if created_models:
            try:
                db.commit()
                for model in created_models:
                    db.refresh(model)
                logger.info(f"批量创建 {len(created_models)} 个模型成功")

                # 清除 /v1/models 列表缓存
                asyncio.create_task(invalidate_models_list_cache())
            except IntegrityError as e:
                db.rollback()
                logger.error(f"批量创建模型失败: {str(e)}")
                raise InvalidRequestException("批量创建模型失败")

        return created_models

    @staticmethod
    def convert_to_response(model: Model) -> ModelResponse:
        """转换为响应模型（新架构：从 GlobalModel 获取显示信息和默认值）"""
        return ModelResponse(
            id=model.id,
            provider_id=model.provider_id,
            global_model_id=model.global_model_id,
            provider_model_name=model.provider_model_name,
            provider_model_aliases=model.provider_model_aliases,
            # 原始配置值（可能为空）
            price_per_request=model.price_per_request,
            tiered_pricing=model.tiered_pricing,
            supports_vision=model.supports_vision,
            supports_function_calling=model.supports_function_calling,
            supports_streaming=model.supports_streaming,
            supports_extended_thinking=model.supports_extended_thinking,
            supports_image_generation=model.supports_image_generation,
            # 有效值（合并 Model 和 GlobalModel 默认值）
            effective_tiered_pricing=model.get_effective_tiered_pricing(),
            effective_input_price=model.get_effective_input_price(),
            effective_output_price=model.get_effective_output_price(),
            effective_price_per_request=model.get_effective_price_per_request(),
            effective_supports_vision=model.get_effective_supports_vision(),
            effective_supports_function_calling=model.get_effective_supports_function_calling(),
            effective_supports_streaming=model.get_effective_supports_streaming(),
            effective_supports_extended_thinking=model.get_effective_supports_extended_thinking(),
            effective_supports_image_generation=model.get_effective_supports_image_generation(),
            is_active=model.is_active,
            is_available=model.is_available if model.is_available is not None else True,
            created_at=model.created_at,
            updated_at=model.updated_at,
            # GlobalModel 信息（如果存在）
            global_model_name=model.global_model.name if model.global_model else None,
            global_model_display_name=(
                model.global_model.display_name if model.global_model else None
            ),
        )
