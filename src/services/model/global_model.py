"""
GlobalModel 服务层

提供 GlobalModel 的 CRUD 操作、查询和统计功能
"""

from typing import Dict, List, Optional, Set

from sqlalchemy import and_, func
from sqlalchemy.orm import Session, joinedload

from src.core.exceptions import InvalidRequestException, NotFoundException
from src.core.logger import logger
from src.models.database import GlobalModel, Model
from src.models.pydantic_models import GlobalModelUpdate


async def on_key_allowed_models_changed(
    db: Session,
    provider_id: str,
    allowed_models: List[str],
) -> None:
    """
    Key 的 allowed_models 变更后的统一处理

    包括：
    1. 触发缓存失效（包括 /v1/models 列表缓存）
    2. 检查并自动关联匹配的 GlobalModel
    3. 检查并自动解除不再匹配的 GlobalModel 关联

    Args:
        db: 数据库 Session
        provider_id: Provider ID
        allowed_models: 更新后的 allowed_models 列表
    """
    from src.services.cache.invalidation import get_cache_invalidation_service

    # 1. 触发缓存失效
    cache_service = get_cache_invalidation_service()
    await cache_service.on_key_allowed_models_changed(provider_id)

    # 2. 检查并自动关联 GlobalModel
    if allowed_models:
        GlobalModelService.auto_associate_provider_by_key_whitelist(
            db=db,
            provider_id=provider_id,
            allowed_models=allowed_models,
        )

    # 3. 检查并自动解除不再匹配的 GlobalModel 关联
    GlobalModelService.auto_disassociate_provider_by_key_whitelist(
        db=db,
        provider_id=provider_id,
    )


class GlobalModelService:
    """GlobalModel 服务"""

    @staticmethod
    def get_global_model(db: Session, global_model_id: str) -> GlobalModel:
        """
        获取单个 GlobalModel

        Args:
            global_model_id: GlobalModel 的 UUID 或 name
        """
        # 先尝试通过 ID 查找
        global_model = db.query(GlobalModel).filter(GlobalModel.id == global_model_id).first()

        # 如果没找到，尝试通过 name 查找
        if not global_model:
            global_model = db.query(GlobalModel).filter(GlobalModel.name == global_model_id).first()

        if not global_model:
            raise NotFoundException(f"GlobalModel {global_model_id} not found")
        return global_model

    @staticmethod
    def get_global_model_by_name(db: Session, name: str) -> Optional[GlobalModel]:
        """通过名称获取 GlobalModel"""
        return db.query(GlobalModel).filter(GlobalModel.name == name).first()

    @staticmethod
    def list_global_models(
        db: Session,
        skip: int = 0,
        limit: int = 100,
        is_active: Optional[bool] = None,
        search: Optional[str] = None,
    ) -> List[GlobalModel]:
        """列出 GlobalModel"""
        query = db.query(GlobalModel)

        if is_active is not None:
            query = query.filter(GlobalModel.is_active == is_active)

        if search:
            search_pattern = f"%{search}%"
            query = query.filter(
                (GlobalModel.name.ilike(search_pattern))
                | (GlobalModel.display_name.ilike(search_pattern))
            )

        # 按名称排序
        query = query.order_by(GlobalModel.name)

        return query.offset(skip).limit(limit).all()

    @staticmethod
    def create_global_model(
        db: Session,
        name: str,
        display_name: str,
        is_active: Optional[bool] = True,
        # 按次计费配置
        default_price_per_request: Optional[float] = None,
        # 阶梯计费配置（必填）
        default_tiered_pricing: dict = None,
        # Key 能力配置
        supported_capabilities: Optional[List[str]] = None,
        # 模型配置（JSON）
        config: Optional[dict] = None,
    ) -> GlobalModel:
        """创建 GlobalModel"""
        # 检查名称是否已存在
        existing = GlobalModelService.get_global_model_by_name(db, name)
        if existing:
            raise InvalidRequestException(f"GlobalModel with name '{name}' already exists")

        global_model = GlobalModel(
            name=name,
            display_name=display_name,
            is_active=is_active,
            # 按次计费配置
            default_price_per_request=default_price_per_request,
            # 阶梯计费配置
            default_tiered_pricing=default_tiered_pricing,
            # Key 能力配置
            supported_capabilities=supported_capabilities,
            # 模型配置（JSON）
            config=config,
        )

        db.add(global_model)
        db.commit()
        db.refresh(global_model)

        return global_model

    @staticmethod
    def update_global_model(
        db: Session,
        global_model_id: str,
        update_data: GlobalModelUpdate,
    ) -> GlobalModel:
        """
        更新 GlobalModel

        使用 exclude_unset=True 来区分"未提供字段"和"显式设置为 None"：
        - 未提供的字段不会被更新
        - 显式设置为 None 的字段会被更新为 None（置空）
        """
        global_model = GlobalModelService.get_global_model(db, global_model_id)

        # 只更新显式设置的字段（包括显式设置为 None 的情况）
        data_dict = update_data.model_dump(exclude_unset=True)

        # 处理阶梯计费配置：如果是 TieredPricingConfig 对象，转换为 dict
        if "default_tiered_pricing" in data_dict:
            tiered_pricing = data_dict["default_tiered_pricing"]
            if tiered_pricing is not None and hasattr(tiered_pricing, "model_dump"):
                data_dict["default_tiered_pricing"] = tiered_pricing.model_dump()

        for field, value in data_dict.items():
            setattr(global_model, field, value)

        db.commit()
        db.refresh(global_model)

        return global_model

    @staticmethod
    def delete_global_model(db: Session, global_model_id: str) -> None:
        """
        删除 GlobalModel

        默认行为: 级联删除所有关联的 Provider 模型实现
        注意: 不清理 API Key 和 User 的 allowed_models 引用，
        保留无效引用可让用户在前端看到"已失效"的模型，便于手动清理或等待重建同名模型
        """
        global_model = GlobalModelService.get_global_model(db, global_model_id)

        # 查找所有关联的 Model（使用 global_model.id，预加载 provider 关联）
        associated_models = (
            db.query(Model)
            .options(joinedload(Model.provider))
            .filter(Model.global_model_id == global_model.id)
            .all()
        )

        # 级联删除所有关联的 Provider 模型实现
        if associated_models:
            logger.info(
                f"删除 GlobalModel {global_model.name} 的 {len(associated_models)} 个关联 Provider 模型"
            )
            for model in associated_models:
                db.delete(model)

        # 删除 GlobalModel
        db.delete(global_model)
        db.commit()

    @staticmethod
    def get_global_model_stats(db: Session, global_model_id: str) -> Dict:
        """获取 GlobalModel 统计信息"""
        global_model = GlobalModelService.get_global_model(db, global_model_id)

        # 统计关联的 Model 数量（使用 global_model.id，预加载 provider 关联）
        models = (
            db.query(Model)
            .options(joinedload(Model.provider))
            .filter(Model.global_model_id == global_model.id)
            .all()
        )

        # 统计支持的 Provider 数量
        provider_ids = set(model.provider_id for model in models)

        # 从阶梯计费中提取价格范围
        input_prices = []
        output_prices = []
        for m in models:
            tiered = m.get_effective_tiered_pricing()
            if tiered and tiered.get("tiers"):
                first_tier = tiered["tiers"][0]
                if first_tier.get("input_price_per_1m") is not None:
                    input_prices.append(first_tier["input_price_per_1m"])
                if first_tier.get("output_price_per_1m") is not None:
                    output_prices.append(first_tier["output_price_per_1m"])

        return {
            "global_model_id": global_model.id,
            "name": global_model.name,
            "total_models": len(models),
            "total_providers": len(provider_ids),
            "price_range": {
                "min_input": min(input_prices) if input_prices else None,
                "max_input": max(input_prices) if input_prices else None,
                "min_output": min(output_prices) if output_prices else None,
                "max_output": max(output_prices) if output_prices else None,
            },
        }

    @staticmethod
    def batch_assign_to_providers(
        db: Session,
        global_model_id: str,
        provider_ids: List[str],
        create_models: bool = False,
    ) -> Dict:
        """批量为多个 Provider 添加 GlobalModel 实现"""
        from .service import ModelService

        global_model = GlobalModelService.get_global_model(db, global_model_id)

        results = {
            "success": [],
            "errors": [],
        }

        for provider_id in provider_ids:
            try:
                # 检查该 Provider 是否已有该 GlobalModel 的实现（使用 global_model.id）
                existing_model = (
                    db.query(Model)
                    .filter(
                        Model.provider_id == provider_id,
                        Model.global_model_id == global_model.id,
                    )
                    .first()
                )

                if existing_model:
                    results["errors"].append(
                        {
                            "provider_id": provider_id,
                            "error": "Model already exists for this provider",
                        }
                    )
                    continue

                if create_models:
                    # 创建新的 Model（价格和能力设为 None，继承 GlobalModel 默认值）
                    model = Model(
                        provider_id=provider_id,
                        global_model_id=global_model.id,
                        provider_model_name=global_model.name,  # 默认使用 GlobalModel name
                        # 计费设为 None，使用 GlobalModel 默认值
                        price_per_request=None,
                        tiered_pricing=None,
                        # 能力设为 None，使用 GlobalModel 默认值
                        supports_vision=None,
                        supports_function_calling=None,
                        supports_streaming=None,
                        supports_extended_thinking=None,
                        is_active=True,
                    )
                    db.add(model)
                    db.commit()

                    results["success"].append(
                        {"provider_id": provider_id, "model_id": model.id, "created": True}
                    )
                else:
                    results["errors"].append(
                        {
                            "provider_id": provider_id,
                            "error": "create_models=False, no existing model found",
                        }
                    )

            except Exception as e:
                db.rollback()
                results["errors"].append({"provider_id": provider_id, "error": str(e)})

        db.commit()
        return results

    @staticmethod
    def auto_associate_provider_by_key_whitelist(
        db: Session,
        provider_id: str,
        allowed_models: List[str],
    ) -> Dict:
        """
        根据 Key 白名单自动关联 Provider 到匹配的 GlobalModel

        当 Key 的 allowed_models 更新后调用此方法，检查所有 GlobalModel 的映射规则，
        如果有映射规则匹配到 Key 白名单中的模型，且 Provider 尚未关联到该 GlobalModel，
        则自动创建关联。

        Args:
            db: 数据库 Session
            provider_id: Provider ID
            allowed_models: Key 的白名单模型列表

        Returns:
            Dict: 包含 success 和 errors 列表
        """
        from src.core.model_permissions import match_model_with_pattern
        from src.models.database import Provider

        results: Dict[str, List[Dict]] = {
            "success": [],
            "errors": [],
        }

        if not allowed_models:
            return results

        # 获取 Provider
        provider = db.query(Provider).filter(Provider.id == provider_id).first()
        if not provider:
            logger.warning(f"Provider {provider_id} not found for auto-association")
            return results

        # 获取该 Provider 已关联的 GlobalModel ID 集合
        existing_associations = (
            db.query(Model.global_model_id, Model.provider_model_name)
            .filter(Model.provider_id == provider_id)
            .all()
        )
        linked_global_model_ids: Set[str] = {row[0] for row in existing_associations if row[0]}
        # 同时获取已存在的 provider_model_name 集合，避免唯一约束冲突
        existing_provider_model_names: Set[str] = {
            row[1] for row in existing_associations if row[1]
        }

        # 获取所有活跃的 GlobalModel（带映射规则）
        global_models = db.query(GlobalModel).filter(GlobalModel.is_active == True).all()

        allowed_models_set = set(allowed_models)

        for global_model in global_models:
            # 跳过已关联的
            if global_model.id in linked_global_model_ids:
                continue

            # 跳过 provider_model_name 已存在的（避免唯一约束冲突）
            if global_model.name in existing_provider_model_names:
                logger.debug(
                    f"Skipping auto-association for GlobalModel {global_model.name}: "
                    f"provider_model_name already exists for Provider {provider.name}"
                )
                continue

            # 提取映射规则
            model_mappings: List[str] = []
            if global_model.config and isinstance(global_model.config, dict):
                mappings = global_model.config.get("model_mappings")
                if isinstance(mappings, list):
                    model_mappings = [m for m in mappings if isinstance(m, str)]

            if not model_mappings:
                continue

            # 检查是否有映射规则匹配到 Key 白名单
            matched = False
            for mapping_pattern in model_mappings:
                for allowed_model in allowed_models_set:
                    if match_model_with_pattern(mapping_pattern, allowed_model):
                        matched = True
                        break
                if matched:
                    break

            if not matched:
                continue

            # 自动创建关联
            try:
                new_model = Model(
                    provider_id=provider_id,
                    global_model_id=global_model.id,
                    provider_model_name=global_model.name,
                    is_active=True,
                )
                db.add(new_model)
                db.flush()

                # 添加到已存在集合，避免后续循环重复创建
                existing_provider_model_names.add(global_model.name)

                results["success"].append(
                    {
                        "global_model_id": global_model.id,
                        "global_model_name": global_model.name,
                        "model_id": new_model.id,
                    }
                )
                logger.info(
                    f"Auto-associated Provider {provider.name} to GlobalModel {global_model.name} "
                    f"via mapping rule match"
                )
            except Exception as e:
                db.rollback()
                logger.error(
                    f"Failed to auto-associate Provider {provider.name} to GlobalModel {global_model.name}: {e}"
                )
                results["errors"].append(
                    {
                        "global_model_id": global_model.id,
                        "global_model_name": global_model.name,
                        "error": str(e),
                    }
                )

        if results["success"]:
            db.commit()

        return results

    @staticmethod
    def auto_disassociate_provider_by_key_whitelist(
        db: Session,
        provider_id: str,
    ) -> Dict:
        """
        根据 Key 白名单自动解除 Provider 与不再匹配的 GlobalModel 的关联

        当 Key 的 allowed_models 更新后调用此方法，检查所有已关联的 GlobalModel，
        如果其映射规则不再匹配任何 Key 白名单中的模型，则自动删除关联。

        注意：只删除通过映射规则自动关联的 Model（即 GlobalModel 有 model_mappings 配置的）

        Args:
            db: 数据库 Session
            provider_id: Provider ID

        Returns:
            Dict: 包含 deleted 和 errors 列表
        """
        from src.core.model_permissions import match_model_with_pattern
        from src.models.database import Provider, ProviderAPIKey

        results: Dict[str, List[Dict]] = {
            "deleted": [],
            "errors": [],
        }

        # 获取 Provider
        provider = db.query(Provider).filter(Provider.id == provider_id).first()
        if not provider:
            logger.warning(f"Provider {provider_id} not found for auto-disassociation")
            return results

        # 1. 获取 Provider 下所有活跃 Key 的 allowed_models 并集
        keys = (
            db.query(ProviderAPIKey)
            .filter(
                ProviderAPIKey.provider_id == provider_id,
                ProviderAPIKey.is_active == True,
            )
            .all()
        )

        # 收集所有 Key 的 allowed_models 并集
        # 注意：allowed_models 为 null 或空列表都表示不支持任何模型
        all_allowed_models: Set[str] = set()

        for key in keys:
            if key.allowed_models:
                all_allowed_models.update(key.allowed_models)

        # 2. 获取 Provider 当前关联的所有 Model（带 GlobalModel 信息）
        models = (
            db.query(Model)
            .options(joinedload(Model.global_model))
            .filter(Model.provider_id == provider_id)
            .all()
        )

        # 3. 检查每个 Model 是否还能匹配
        for model in models:
            # 跳过没有关联 GlobalModel 的
            if not model.global_model_id or not model.global_model:
                continue

            global_model = model.global_model

            # 提取映射规则
            model_mappings: List[str] = []
            if global_model.config and isinstance(global_model.config, dict):
                mappings = global_model.config.get("model_mappings")
                if isinstance(mappings, list):
                    model_mappings = [m for m in mappings if isinstance(m, str)]

            # 如果 GlobalModel 没有 model_mappings，跳过（说明不是通过映射自动关联的）
            if not model_mappings:
                continue

            # 检查是否有映射规则匹配到任一 allowed_models
            matched = False
            for mapping_pattern in model_mappings:
                for allowed_model in all_allowed_models:
                    if match_model_with_pattern(mapping_pattern, allowed_model):
                        matched = True
                        break
                if matched:
                    break

            # 如果不再匹配，删除该 Model
            if not matched:
                try:
                    db.delete(model)
                    results["deleted"].append(
                        {
                            "model_id": model.id,
                            "global_model_id": global_model.id,
                            "global_model_name": global_model.name,
                        }
                    )
                    logger.info(
                        f"Auto-disassociated Provider {provider.name} from GlobalModel {global_model.name} "
                        f"(no matching allowed_models)"
                    )
                except Exception as e:
                    logger.error(
                        f"Failed to auto-disassociate Provider {provider.name} from GlobalModel {global_model.name}: {e}"
                    )
                    results["errors"].append(
                        {
                            "model_id": model.id,
                            "global_model_id": global_model.id,
                            "global_model_name": global_model.name,
                            "error": str(e),
                        }
                    )

        if results["deleted"]:
            db.commit()

        return results
