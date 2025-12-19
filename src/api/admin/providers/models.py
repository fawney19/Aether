"""
Provider 模型管理 API
"""

from dataclasses import dataclass
from typing import Any, Dict, List, Optional

from fastapi import APIRouter, Depends, Request
from sqlalchemy.orm import Session, joinedload

from src.api.base.admin_adapter import AdminApiAdapter
from src.api.base.models_service import invalidate_models_list_cache
from src.api.base.pipeline import ApiRequestPipeline
from src.core.exceptions import InvalidRequestException, NotFoundException
from src.core.logger import logger
from src.database import get_db
from src.models.api import (
    ModelCreate,
    ModelResponse,
    ModelUpdate,
)
from src.models.pydantic_models import (
    BatchAssignModelsToProviderRequest,
    BatchAssignModelsToProviderResponse,
)
from src.models.database import (
    GlobalModel,
    Model,
    Provider,
)
from src.models.pydantic_models import (
    ProviderAvailableSourceModel,
    ProviderAvailableSourceModelsResponse,
)
from src.services.model.service import ModelService

router = APIRouter(tags=["Model Management"])
pipeline = ApiRequestPipeline()


@router.get("/{provider_id}/models", response_model=List[ModelResponse])
async def list_provider_models(
    provider_id: str,
    request: Request,
    is_active: Optional[bool] = None,
    skip: int = 0,
    limit: int = 100,
    db: Session = Depends(get_db),
) -> List[ModelResponse]:
    """获取提供商的所有模型（管理员）"""
    adapter = AdminListProviderModelsAdapter(
        provider_id=provider_id,
        is_active=is_active,
        skip=skip,
        limit=limit,
    )
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.post("/{provider_id}/models", response_model=ModelResponse)
async def create_provider_model(
    provider_id: str,
    model_data: ModelCreate,
    request: Request,
    db: Session = Depends(get_db),
) -> ModelResponse:
    """创建模型（管理员）"""
    adapter = AdminCreateProviderModelAdapter(provider_id=provider_id, model_data=model_data)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/{provider_id}/models/{model_id}", response_model=ModelResponse)
async def get_provider_model(
    provider_id: str,
    model_id: str,
    request: Request,
    db: Session = Depends(get_db),
) -> ModelResponse:
    """获取模型详情（管理员）"""
    adapter = AdminGetProviderModelAdapter(provider_id=provider_id, model_id=model_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.patch("/{provider_id}/models/{model_id}", response_model=ModelResponse)
async def update_provider_model(
    provider_id: str,
    model_id: str,
    model_data: ModelUpdate,
    request: Request,
    db: Session = Depends(get_db),
) -> ModelResponse:
    """更新模型（管理员）"""
    adapter = AdminUpdateProviderModelAdapter(
        provider_id=provider_id,
        model_id=model_id,
        model_data=model_data,
    )
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.delete("/{provider_id}/models/{model_id}")
async def delete_provider_model(
    provider_id: str,
    model_id: str,
    request: Request,
    db: Session = Depends(get_db),
):
    """删除模型（管理员）"""
    adapter = AdminDeleteProviderModelAdapter(provider_id=provider_id, model_id=model_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.post("/{provider_id}/models/batch", response_model=List[ModelResponse])
async def batch_create_provider_models(
    provider_id: str,
    models_data: List[ModelCreate],
    request: Request,
    db: Session = Depends(get_db),
) -> List[ModelResponse]:
    """批量创建模型（管理员）"""
    adapter = AdminBatchCreateModelsAdapter(provider_id=provider_id, models_data=models_data)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get(
    "/{provider_id}/available-source-models",
    response_model=ProviderAvailableSourceModelsResponse,
)
async def get_provider_available_source_models(
    provider_id: str,
    request: Request,
    db: Session = Depends(get_db),
):
    """
    获取该 Provider 支持的所有统一模型名（source_model）

    包括：
    1. 直连模型（Model.provider_model_name 直接作为统一模型名）
    """
    adapter = AdminGetProviderAvailableSourceModelsAdapter(provider_id=provider_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.post(
    "/{provider_id}/assign-global-models",
    response_model=BatchAssignModelsToProviderResponse,
)
async def batch_assign_global_models_to_provider(
    provider_id: str,
    payload: BatchAssignModelsToProviderRequest,
    request: Request,
    db: Session = Depends(get_db),
) -> BatchAssignModelsToProviderResponse:
    """批量为 Provider 关联 GlobalModels（自动继承价格和能力配置）"""
    adapter = AdminBatchAssignModelsToProviderAdapter(
        provider_id=provider_id, payload=payload
    )
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


# -------- Adapters --------


@dataclass
class AdminListProviderModelsAdapter(AdminApiAdapter):
    provider_id: str
    is_active: Optional[bool]
    skip: int
    limit: int

    async def handle(self, context):  # type: ignore[override]
        db = context.db
        provider = db.query(Provider).filter(Provider.id == self.provider_id).first()
        if not provider:
            raise NotFoundException("Provider not found", "provider")

        models = ModelService.get_models_by_provider(
            db, self.provider_id, self.skip, self.limit, self.is_active
        )
        return [ModelService.convert_to_response(model) for model in models]


@dataclass
class AdminCreateProviderModelAdapter(AdminApiAdapter):
    provider_id: str
    model_data: ModelCreate

    async def handle(self, context):  # type: ignore[override]
        db = context.db
        provider = db.query(Provider).filter(Provider.id == self.provider_id).first()
        if not provider:
            raise NotFoundException("Provider not found", "provider")

        try:
            model = ModelService.create_model(db, self.provider_id, self.model_data)
            logger.info(f"Model created: {model.provider_model_name} for provider {provider.name} by {context.user.username}")
            return ModelService.convert_to_response(model)
        except Exception as exc:
            raise InvalidRequestException(str(exc))


@dataclass
class AdminGetProviderModelAdapter(AdminApiAdapter):
    provider_id: str
    model_id: str

    async def handle(self, context):  # type: ignore[override]
        db = context.db
        model = (
            db.query(Model)
            .filter(Model.id == self.model_id, Model.provider_id == self.provider_id)
            .first()
        )
        if not model:
            raise NotFoundException("Model not found", "model")

        return ModelService.convert_to_response(model)


@dataclass
class AdminUpdateProviderModelAdapter(AdminApiAdapter):
    provider_id: str
    model_id: str
    model_data: ModelUpdate

    async def handle(self, context):  # type: ignore[override]
        db = context.db
        model = (
            db.query(Model)
            .filter(Model.id == self.model_id, Model.provider_id == self.provider_id)
            .first()
        )
        if not model:
            raise NotFoundException("Model not found", "model")

        try:
            updated_model = ModelService.update_model(db, self.model_id, self.model_data)
            logger.info(f"Model updated: {updated_model.provider_model_name} by {context.user.username}")
            return ModelService.convert_to_response(updated_model)
        except Exception as exc:
            raise InvalidRequestException(str(exc))


@dataclass
class AdminDeleteProviderModelAdapter(AdminApiAdapter):
    provider_id: str
    model_id: str

    async def handle(self, context):  # type: ignore[override]
        db = context.db
        model = (
            db.query(Model)
            .filter(Model.id == self.model_id, Model.provider_id == self.provider_id)
            .first()
        )
        if not model:
            raise NotFoundException("Model not found", "model")

        model_name = model.provider_model_name
        try:
            ModelService.delete_model(db, self.model_id)
            logger.info(f"Model deleted: {model_name} by {context.user.username}")
            return {"message": f"Model '{model_name}' deleted successfully"}
        except Exception as exc:
            raise InvalidRequestException(str(exc))


@dataclass
class AdminBatchCreateModelsAdapter(AdminApiAdapter):
    provider_id: str
    models_data: List[ModelCreate]

    async def handle(self, context):  # type: ignore[override]
        db = context.db
        provider = db.query(Provider).filter(Provider.id == self.provider_id).first()
        if not provider:
            raise NotFoundException("Provider not found", "provider")

        try:
            models = ModelService.batch_create_models(db, self.provider_id, self.models_data)
            logger.info(f"Batch created {len(models)} models for provider {provider.name} by {context.user.username}")
            return [ModelService.convert_to_response(model) for model in models]
        except Exception as exc:
            raise InvalidRequestException(str(exc))


@dataclass
class AdminGetProviderAvailableSourceModelsAdapter(AdminApiAdapter):
    provider_id: str

    async def handle(self, context):  # type: ignore[override]
        """
        返回 Provider 支持的所有 GlobalModel

        逻辑：
        1. 查询该 Provider 的所有 Model
        2. 通过 Model.global_model_id 获取 GlobalModel
        """
        db = context.db
        provider = db.query(Provider).filter(Provider.id == self.provider_id).first()
        if not provider:
            raise NotFoundException("Provider not found", "provider")

        # 1. 查询该 Provider 的所有活跃 Model（预加载 GlobalModel）
        models = (
            db.query(Model)
            .options(joinedload(Model.global_model))
            .filter(Model.provider_id == self.provider_id, Model.is_active == True)
            .all()
        )

        # 2. 构建以 GlobalModel 为主键的字典
        global_models_dict: Dict[str, Dict[str, Any]] = {}

        for model in models:
            global_model = model.global_model
            if not global_model or not global_model.is_active:
                continue

            global_model_name = global_model.name

            # 如果该 GlobalModel 还未处理，初始化
            if global_model_name not in global_models_dict:
                global_models_dict[global_model_name] = {
                    "global_model_name": global_model_name,
                    "display_name": global_model.display_name,
                    "provider_model_name": model.provider_model_name,
                    "model_id": model.id,
                    "price": {
                        "input_price_per_1m": model.get_effective_input_price(),
                        "output_price_per_1m": model.get_effective_output_price(),
                        "cache_creation_price_per_1m": model.get_effective_cache_creation_price(),
                        "cache_read_price_per_1m": model.get_effective_cache_read_price(),
                        "price_per_request": model.get_effective_price_per_request(),
                    },
                    "capabilities": {
                        "supports_vision": bool(model.supports_vision),
                        "supports_function_calling": bool(model.supports_function_calling),
                        "supports_streaming": bool(model.supports_streaming),
                    },
                    "is_active": bool(model.is_active),
                }

        models_list = [
            ProviderAvailableSourceModel(**global_models_dict[name])
            for name in sorted(global_models_dict.keys())
        ]

        return ProviderAvailableSourceModelsResponse(models=models_list, total=len(models_list))


@dataclass
class AdminBatchAssignModelsToProviderAdapter(AdminApiAdapter):
    """批量为 Provider 关联 GlobalModels"""

    provider_id: str
    payload: BatchAssignModelsToProviderRequest

    async def handle(self, context):  # type: ignore[override]
        db = context.db
        provider = db.query(Provider).filter(Provider.id == self.provider_id).first()
        if not provider:
            raise NotFoundException("Provider not found", "provider")

        success = []
        errors = []

        for global_model_id in self.payload.global_model_ids:
            try:
                global_model = (
                    db.query(GlobalModel).filter(GlobalModel.id == global_model_id).first()
                )
                if not global_model:
                    errors.append(
                        {"global_model_id": global_model_id, "error": "GlobalModel not found"}
                    )
                    continue

                # 检查是否已存在关联
                existing = (
                    db.query(Model)
                    .filter(
                        Model.provider_id == self.provider_id,
                        Model.global_model_id == global_model_id,
                    )
                    .first()
                )
                if existing:
                    errors.append(
                        {
                            "global_model_id": global_model_id,
                            "global_model_name": global_model.name,
                            "error": "Already associated",
                        }
                    )
                    continue

                # 创建新的 Model 记录，继承 GlobalModel 的配置
                new_model = Model(
                    provider_id=self.provider_id,
                    global_model_id=global_model_id,
                    provider_model_name=global_model.name,
                    is_active=True,
                )
                db.add(new_model)
                db.flush()

                success.append(
                    {
                        "global_model_id": global_model_id,
                        "global_model_name": global_model.name,
                        "model_id": new_model.id,
                    }
                )
            except Exception as e:
                errors.append({"global_model_id": global_model_id, "error": str(e)})

        db.commit()
        logger.info(
            f"Batch assigned {len(success)} GlobalModels to provider {provider.name} by {context.user.username}"
        )

        # 清除 /v1/models 列表缓存
        if success:
            await invalidate_models_list_cache()

        return BatchAssignModelsToProviderResponse(success=success, errors=errors)
