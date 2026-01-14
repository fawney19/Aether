"""
模型自动同步服务

根据 Provider Key 的 allowed_models 白名单和全局模型的别名规则,
自动将匹配的全局模型添加到 Provider 的模型列表中。
"""

import uuid
from typing import Dict, List, Optional, Set

from sqlalchemy.orm import Session

from src.core.logger import logger
from src.core.model_permissions import match_model_with_pattern, normalize_allowed_models
from src.models.database import GlobalModel, Model, Provider, ProviderAPIKey


class ModelAutoSyncService:
    """模型自动同步服务"""

    @staticmethod
    def sync_models_for_global_model(db: Session, global_model_id: str) -> Dict:
        """
        为特定 GlobalModel 同步所有 Provider 的模型

        当 GlobalModel 的别名规则变更时，重新扫描所有 Provider 的 allowed_models，
        如果有新的匹配项则自动添加。

        Args:
            db: 数据库会话
            global_model_id: GlobalModel ID

        Returns:
            同步结果统计:
            {
                "global_model_id": GlobalModel ID,
                "providers_scanned": 扫描的 Provider 数量,
                "models_created": 新创建的 Model 数量,
                "errors": 错误列表
            }
        """
        # 获取 GlobalModel
        global_model = db.query(GlobalModel).filter(GlobalModel.id == global_model_id).first()
        if not global_model:
            raise ValueError(f"GlobalModel {global_model_id} 不存在")

        logger.info(f"开始为 GlobalModel '{global_model.name}' 同步所有 Provider 的模型...")

        # 获取所有活跃的 Provider
        providers = db.query(Provider).filter(Provider.is_active.is_(True)).all()

        total_created = 0
        errors = []

        for provider in providers:
            try:
                result = ModelAutoSyncService._sync_single_global_model_for_provider(
                    db, provider, global_model
                )
                total_created += result["models_created"]
                if result["errors"]:
                    errors.extend(result["errors"])
            except Exception as e:
                error_msg = f"同步 Provider {provider.name} (ID: {provider.id}) 失败: {str(e)}"
                logger.warning(error_msg)
                errors.append(error_msg)
                try:
                    db.rollback()
                except Exception:
                    pass

        logger.info(
            f"GlobalModel '{global_model.name}' 同步完成: 扫描 {len(providers)} 个 Provider, "
            f"新增 {total_created} 个模型"
        )

        return {
            "global_model_id": global_model_id,
            "providers_scanned": len(providers),
            "models_created": total_created,
            "errors": errors,
        }

    @staticmethod
    def sync_all_provider_models(db: Session) -> Dict:
        """
        为所有 Provider 同步模型

        Args:
            db: 数据库会话

        Returns:
            同步结果统计:
            {
                "providers_scanned": 扫描的 Provider 数量,
                "models_created": 新创建的 Model 数量,
                "errors": 错误列表
            }
        """
        logger.info("开始执行模型自动同步...")

        # 获取所有活跃的 Provider
        providers = db.query(Provider).filter(Provider.is_active.is_(True)).all()

        total_created = 0
        errors = []

        for provider in providers:
            try:
                result = ModelAutoSyncService.sync_models_for_provider(db, provider.id)
                total_created += result["models_created"]
                if result["errors"]:
                    errors.extend(result["errors"])
            except Exception as e:
                error_msg = f"同步 Provider {provider.name} (ID: {provider.id}) 失败: {str(e)}"
                logger.warning(error_msg)
                errors.append(error_msg)
                try:
                    db.rollback()
                except Exception:
                    pass

        logger.info(
            f"模型自动同步完成: 扫描 {len(providers)} 个 Provider, "
            f"新增 {total_created} 个模型"
        )

        return {
            "providers_scanned": len(providers),
            "models_created": total_created,
            "errors": errors,
        }

    @staticmethod
    def sync_models_for_provider(db: Session, provider_id: str) -> Dict:
        """
        为单个 Provider 同步模型

        Args:
            db: 数据库会话
            provider_id: Provider ID

        Returns:
            同步结果:
            {
                "provider_id": Provider ID,
                "models_created": 新创建的 Model 数量,
                "errors": 错误列表
            }
        """
        # 获取 Provider
        provider = db.query(Provider).filter(Provider.id == provider_id).first()
        if not provider:
            raise ValueError(f"Provider {provider_id} 不存在")

        logger.debug(f"检查 Provider: {provider.name} (ID: {provider_id})")

        # 获取所有活跃的全局模型(一次性加载,避免 N+1 查询)
        all_global_models = (
            db.query(GlobalModel).filter(GlobalModel.is_active.is_(True)).all()
        )

        # 收集该 Provider 下所有 API Key 的 allowed_models
        all_allowed_model_names: Set[str] = set()

        for key in provider.api_keys:
            if key.allowed_models is None:
                # None 表示不限制,跳过
                continue

            # 处理简单列表和按格式字典两种情况
            # 由于我们不知道具体的 api_format,这里需要收集所有可能的模型名
            if isinstance(key.allowed_models, list):
                # 简单列表格式
                allowed_set = normalize_allowed_models(key.allowed_models, None)
                if allowed_set:
                    all_allowed_model_names.update(allowed_set)
            elif isinstance(key.allowed_models, dict):
                # 按格式字典,遍历所有格式
                for api_format, model_list in key.allowed_models.items():
                    if model_list is None:
                        continue
                    allowed_set = normalize_allowed_models(
                        key.allowed_models, api_format
                    )
                    if allowed_set:
                        all_allowed_model_names.update(allowed_set)

        if not all_allowed_model_names:
            logger.debug(f"Provider {provider.name} 没有配置 allowed_models 白名单")
            return {"provider_id": provider_id, "models_created": 0, "errors": []}

        # 查找匹配的全局模型
        matched_global_models = ModelAutoSyncService._find_matching_global_models(
            all_global_models, all_allowed_model_names
        )

        # 创建缺失的 Model 记录
        models_created = 0
        errors = []

        for global_model in matched_global_models:
            try:
                # 检查是否已存在
                existing = (
                    db.query(Model)
                    .filter(
                        Model.provider_id == provider_id,
                        Model.global_model_id == global_model.id,
                    )
                    .first()
                )

                if existing:
                    logger.debug(
                        f"Model 已存在: {global_model.name} for provider {provider.name}"
                    )
                    continue

                # 创建新的 Model 记录
                new_model = Model(
                    id=str(uuid.uuid4()),
                    provider_id=provider_id,
                    global_model_id=global_model.id,
                    provider_model_name=global_model.name,
                    # 价格和能力字段设为 None,继承 GlobalModel 默认值
                    price_per_request=None,
                    tiered_pricing=None,
                    supports_vision=None,
                    supports_function_calling=None,
                    supports_streaming=None,
                    supports_extended_thinking=None,
                    supports_image_generation=None,
                    is_active=True,
                    is_available=True,
                )
                db.add(new_model)
                db.commit()

                models_created += 1
                logger.info(
                    f"为 Provider '{provider.name}' 创建模型: {global_model.name}"
                )

            except Exception as e:
                error_msg = f"创建模型 {global_model.name} 失败: {str(e)}"
                logger.warning(error_msg)
                errors.append(error_msg)
                try:
                    db.rollback()
                except Exception:
                    pass

        return {
            "provider_id": provider_id,
            "models_created": models_created,
            "errors": errors,
        }

    @staticmethod
    def _sync_single_global_model_for_provider(
        db: Session, provider: Provider, global_model: GlobalModel
    ) -> Dict:
        """
        为单个 Provider 同步单个 GlobalModel

        Args:
            db: 数据库会话
            provider: Provider 实例
            global_model: GlobalModel 实例

        Returns:
            同步结果:
            {
                "models_created": 新创建的 Model 数量,
                "errors": 错误列表
            }
        """
        logger.debug(
            f"检查 Provider '{provider.name}' 是否匹配 GlobalModel '{global_model.name}'"
        )

        # 检查全局模型是否配置了别名
        if not global_model.config or "model_aliases" not in global_model.config:
            logger.debug(f"GlobalModel '{global_model.name}' 没有配置别名规则")
            return {"models_created": 0, "errors": []}

        model_aliases = global_model.config.get("model_aliases", [])
        if not isinstance(model_aliases, list) or not model_aliases:
            logger.debug(f"GlobalModel '{global_model.name}' 的别名规则为空")
            return {"models_created": 0, "errors": []}

        # 收集该 Provider 下所有 API Key 的 allowed_models
        all_allowed_model_names: Set[str] = set()

        for key in provider.api_keys:
            if key.allowed_models is None:
                # None 表示不限制，跳过
                continue

            # 处理简单列表和按格式字典两种情况
            if isinstance(key.allowed_models, list):
                # 简单列表格式
                allowed_set = normalize_allowed_models(key.allowed_models, None)
                if allowed_set:
                    all_allowed_model_names.update(allowed_set)
            elif isinstance(key.allowed_models, dict):
                # 按格式字典，遍历所有格式
                for api_format, model_list in key.allowed_models.items():
                    if model_list is None:
                        continue
                    allowed_set = normalize_allowed_models(key.allowed_models, api_format)
                    if allowed_set:
                        all_allowed_model_names.update(allowed_set)

        if not all_allowed_model_names:
            logger.debug(f"Provider '{provider.name}' 没有配置 allowed_models 白名单")
            return {"models_created": 0, "errors": []}

        # 检查 GlobalModel 的别名是否匹配 Provider 的白名单
        matched = False
        for allowed_model_name in all_allowed_model_names:
            for alias_pattern in model_aliases:
                if match_model_with_pattern(alias_pattern, allowed_model_name):
                    logger.debug(
                        f"发现匹配: GlobalModel '{global_model.name}' "
                        f"(别名: '{alias_pattern}') 匹配白名单 '{allowed_model_name}' "
                        f"in Provider '{provider.name}'"
                    )
                    matched = True
                    break
            if matched:
                break

        if not matched:
            logger.debug(
                f"GlobalModel '{global_model.name}' 的别名规则不匹配 Provider '{provider.name}' 的白名单"
            )
            return {"models_created": 0, "errors": []}

        # 检查是否已存在
        existing = (
            db.query(Model)
            .filter(
                Model.provider_id == provider.id,
                Model.global_model_id == global_model.id,
            )
            .first()
        )

        if existing:
            logger.debug(
                f"Model 已存在: {global_model.name} for provider {provider.name}"
            )
            return {"models_created": 0, "errors": []}

        # 创建新的 Model 记录
        errors = []
        try:
            new_model = Model(
                id=str(uuid.uuid4()),
                provider_id=provider.id,
                global_model_id=global_model.id,
                provider_model_name=global_model.name,
                # 价格和能力字段设为 None，继承 GlobalModel 默认值
                price_per_request=None,
                tiered_pricing=None,
                supports_vision=None,
                supports_function_calling=None,
                supports_streaming=None,
                supports_extended_thinking=None,
                supports_image_generation=None,
                is_active=True,
                is_available=True,
            )
            db.add(new_model)
            db.commit()

            logger.info(
                f"为 Provider '{provider.name}' 创建模型: {global_model.name} (自动同步)"
            )
            return {"models_created": 1, "errors": []}

        except Exception as e:
            error_msg = f"创建模型 {global_model.name} 失败: {str(e)}"
            logger.warning(error_msg)
            errors.append(error_msg)
            try:
                db.rollback()
            except Exception:
                pass
            return {"models_created": 0, "errors": errors}

    @staticmethod
    def _find_matching_global_models(
        all_global_models: List[GlobalModel], allowed_model_names: Set[str]
    ) -> List[GlobalModel]:
        """
        根据白名单查找匹配的全局模型

        Args:
            all_global_models: 所有活跃的全局模型列表
            allowed_model_names: 白名单中的模型名集合

        Returns:
            匹配的全局模型列表
        """
        matched_models = []

        for allowed_model_name in allowed_model_names:
            for global_model in all_global_models:
                # 检查全局模型是否配置了别名
                if not global_model.config or "model_aliases" not in global_model.config:
                    continue

                model_aliases = global_model.config.get("model_aliases", [])
                if not isinstance(model_aliases, list):
                    continue

                # 检查白名单模型名是否匹配全局模型的任一别名
                for alias_pattern in model_aliases:
                    if match_model_with_pattern(alias_pattern, allowed_model_name):
                        # 匹配成功
                        logger.debug(
                            f"发现匹配: GlobalModel '{global_model.name}' "
                            f"(别名: '{alias_pattern}') 匹配白名单 '{allowed_model_name}'"
                        )
                        matched_models.append(global_model)
                        break  # 匹配成功后跳出别名循环

        # 去重(同一个全局模型可能匹配多个白名单项)
        unique_models = list({model.id: model for model in matched_models}.values())

        return unique_models
