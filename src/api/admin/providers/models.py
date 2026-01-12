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
    ImportFromUpstreamRequest,
    ImportFromUpstreamResponse,
    ImportFromUpstreamSuccessItem,
    ImportFromUpstreamErrorItem,
    ProviderAvailableSourceModel,
    ProviderAvailableSourceModelsResponse,
)
from src.models.database import (
    GlobalModel,
    Model,
    Provider,
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
    """
    获取提供商的所有模型

    获取指定提供商的模型列表，支持分页和状态过滤。

    **路径参数**:
    - `provider_id`: 提供商 ID

    **查询参数**:
    - `is_active`: 可选的活跃状态过滤，true 仅返回活跃模型，false 返回禁用模型，不传则返回全部
    - `skip`: 跳过的记录数，默认为 0
    - `limit`: 返回的最大记录数，默认为 100

    **返回字段**（数组，每项包含）:
    - `id`: 模型 ID
    - `provider_id`: 提供商 ID
    - `global_model_id`: 全局模型 ID
    - `provider_model_name`: 提供商模型名称
    - `is_active`: 是否启用
    - `input_price_per_1m`: 输入价格（每百万 token）
    - `output_price_per_1m`: 输出价格（每百万 token）
    - `cache_creation_price_per_1m`: 缓存创建价格（每百万 token）
    - `cache_read_price_per_1m`: 缓存读取价格（每百万 token）
    - `price_per_request`: 每次请求价格
    - `supports_vision`: 是否支持视觉
    - `supports_function_calling`: 是否支持函数调用
    - `supports_streaming`: 是否支持流式输出
    - `created_at`: 创建时间
    - `updated_at`: 更新时间
    """
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
    """
    创建模型

    为指定提供商创建一个新的模型配置。

    **路径参数**:
    - `provider_id`: 提供商 ID

    **请求体字段**:
    - `provider_model_name`: 提供商模型名称（必填）
    - `global_model_id`: 全局模型 ID（可选，关联到全局模型）
    - `is_active`: 是否启用（默认 true）
    - `input_price_per_1m`: 输入价格（每百万 token）（可选）
    - `output_price_per_1m`: 输出价格（每百万 token）（可选）
    - `cache_creation_price_per_1m`: 缓存创建价格（每百万 token）（可选）
    - `cache_read_price_per_1m`: 缓存读取价格（每百万 token）（可选）
    - `price_per_request`: 每次请求价格（可选）
    - `supports_vision`: 是否支持视觉（可选）
    - `supports_function_calling`: 是否支持函数调用（可选）
    - `supports_streaming`: 是否支持流式输出（可选）

    **返回字段**: 返回创建的模型详细信息（与 GET 单个模型接口返回格式相同）
    """
    adapter = AdminCreateProviderModelAdapter(provider_id=provider_id, model_data=model_data)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/{provider_id}/models/{model_id}", response_model=ModelResponse)
async def get_provider_model(
    provider_id: str,
    model_id: str,
    request: Request,
    db: Session = Depends(get_db),
) -> ModelResponse:
    """
    获取模型详情

    获取指定模型的详细配置信息。

    **路径参数**:
    - `provider_id`: 提供商 ID
    - `model_id`: 模型 ID

    **返回字段**:
    - `id`: 模型 ID
    - `provider_id`: 提供商 ID
    - `global_model_id`: 全局模型 ID
    - `provider_model_name`: 提供商模型名称
    - `is_active`: 是否启用
    - `input_price_per_1m`: 输入价格（每百万 token）
    - `output_price_per_1m`: 输出价格（每百万 token）
    - `cache_creation_price_per_1m`: 缓存创建价格（每百万 token）
    - `cache_read_price_per_1m`: 缓存读取价格（每百万 token）
    - `price_per_request`: 每次请求价格
    - `supports_vision`: 是否支持视觉
    - `supports_function_calling`: 是否支持函数调用
    - `supports_streaming`: 是否支持流式输出
    - `created_at`: 创建时间
    - `updated_at`: 更新时间
    """
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
    """
    更新模型配置

    更新指定模型的配置信息。只需传入需要更新的字段，未传入的字段保持不变。

    **路径参数**:
    - `provider_id`: 提供商 ID
    - `model_id`: 模型 ID

    **请求体字段**（所有字段可选）:
    - `provider_model_name`: 提供商模型名称
    - `global_model_id`: 全局模型 ID
    - `is_active`: 是否启用
    - `input_price_per_1m`: 输入价格（每百万 token）
    - `output_price_per_1m`: 输出价格（每百万 token）
    - `cache_creation_price_per_1m`: 缓存创建价格（每百万 token）
    - `cache_read_price_per_1m`: 缓存读取价格（每百万 token）
    - `price_per_request`: 每次请求价格
    - `supports_vision`: 是否支持视觉
    - `supports_function_calling`: 是否支持函数调用
    - `supports_streaming`: 是否支持流式输出

    **返回字段**: 返回更新后的模型详细信息（与 GET 单个模型接口返回格式相同）
    """
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
    """
    删除模型

    删除指定的模型配置。注意：此操作不可逆。

    **路径参数**:
    - `provider_id`: 提供商 ID
    - `model_id`: 模型 ID

    **返回字段**:
    - `message`: 删除成功提示信息
    """
    adapter = AdminDeleteProviderModelAdapter(provider_id=provider_id, model_id=model_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.post("/{provider_id}/models/batch", response_model=List[ModelResponse])
async def batch_create_provider_models(
    provider_id: str,
    models_data: List[ModelCreate],
    request: Request,
    db: Session = Depends(get_db),
) -> List[ModelResponse]:
    """
    批量创建模型

    为指定提供商批量创建多个模型配置。

    **路径参数**:
    - `provider_id`: 提供商 ID

    **请求体**: 模型数据数组，每项包含：
    - `provider_model_name`: 提供商模型名称（必填）
    - `global_model_id`: 全局模型 ID（可选）
    - `is_active`: 是否启用（默认 true）
    - `input_price_per_1m`: 输入价格（每百万 token）（可选）
    - `output_price_per_1m`: 输出价格（每百万 token）（可选）
    - `cache_creation_price_per_1m`: 缓存创建价格（每百万 token）（可选）
    - `cache_read_price_per_1m`: 缓存读取价格（每百万 token）（可选）
    - `price_per_request`: 每次请求价格（可选）
    - `supports_vision`: 是否支持视觉（可选）
    - `supports_function_calling`: 是否支持函数调用（可选）
    - `supports_streaming`: 是否支持流式输出（可选）

    **返回字段**: 返回创建的模型列表（与 GET 模型列表接口返回格式相同）
    """
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
    获取提供商支持的可用源模型

    获取该提供商支持的所有统一模型名（source_model），包含价格和能力信息。

    **路径参数**:
    - `provider_id`: 提供商 ID

    **返回字段**:
    - `models`: 可用源模型数组，每项包含：
      - `global_model_name`: 全局模型名称
      - `display_name`: 显示名称
      - `provider_model_name`: 提供商模型名称
      - `model_id`: 模型 ID
      - `price`: 价格信息（包含 input_price_per_1m, output_price_per_1m, cache_creation_price_per_1m, cache_read_price_per_1m, price_per_request）
      - `capabilities`: 能力信息（包含 supports_vision, supports_function_calling, supports_streaming）
      - `is_active`: 是否启用
    - `total`: 总数
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
    """
    批量关联全局模型

    批量为提供商关联全局模型，自动继承全局模型的价格和能力配置。

    **路径参数**:
    - `provider_id`: 提供商 ID

    **请求体字段**:
    - `global_model_ids`: 全局模型 ID 数组（必填）

    **返回字段**:
    - `success`: 成功关联的模型数组，每项包含：
      - `global_model_id`: 全局模型 ID
      - `global_model_name`: 全局模型名称
      - `model_id`: 新创建的模型 ID
    - `errors`: 失败的模型数组，每项包含：
      - `global_model_id`: 全局模型 ID
      - `global_model_name`: 全局模型名称（如果可用）
      - `error`: 错误信息
    """
    adapter = AdminBatchAssignModelsToProviderAdapter(
        provider_id=provider_id, payload=payload
    )
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.post(
    "/{provider_id}/import-from-upstream",
    response_model=ImportFromUpstreamResponse,
)
async def import_models_from_upstream(
    provider_id: str,
    payload: ImportFromUpstreamRequest,
    request: Request,
    db: Session = Depends(get_db),
) -> ImportFromUpstreamResponse:
    """
    从上游提供商导入模型

    从上游提供商导入模型列表。导入的模型作为独立的 ProviderModel 存储，
    不会自动创建 GlobalModel。后续需要手动关联 GlobalModel 才能参与路由。

    **流程说明**:
    1. 检查模型是否已存在于当前 Provider（按 provider_model_name 匹配）
    2. 创建新的 ProviderModel（global_model_id = NULL）
    3. 支持设置价格覆盖（tiered_pricing, price_per_request）

    **路径参数**:
    - `provider_id`: 提供商 ID

    **请求体字段**:
    - `model_ids`: 模型 ID 数组（必填，每个 ID 长度 1-100 字符）
    - `tiered_pricing`: 可选的阶梯计费配置（应用于所有导入的模型）
    - `price_per_request`: 可选的按次计费价格（应用于所有导入的模型）

    **返回字段**:
    - `success`: 成功导入的模型数组，每项包含：
      - `model_id`: 模型 ID
      - `provider_model_id`: 提供商模型 ID
      - `global_model_id`: 全局模型 ID（如果已关联）
      - `global_model_name`: 全局模型名称（如果已关联）
      - `created_global_model`: 是否新创建了全局模型（始终为 false）
    - `errors`: 失败的模型数组，每项包含：
      - `model_id`: 模型 ID
      - `error`: 错误信息
    """
    adapter = AdminImportFromUpstreamAdapter(provider_id=provider_id, payload=payload)
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


@dataclass
class AdminImportFromUpstreamAdapter(AdminApiAdapter):
    """从上游提供商导入模型（不创建 GlobalModel，作为独立 ProviderModel）"""

    provider_id: str
    payload: ImportFromUpstreamRequest

    async def handle(self, context):  # type: ignore[override]
        db = context.db
        provider = db.query(Provider).filter(Provider.id == self.provider_id).first()
        if not provider:
            raise NotFoundException("Provider not found", "provider")

        success: list[ImportFromUpstreamSuccessItem] = []
        errors: list[ImportFromUpstreamErrorItem] = []

        # 获取价格覆盖配置
        tiered_pricing = None
        price_per_request = None
        if hasattr(self.payload, 'tiered_pricing') and self.payload.tiered_pricing:
            tiered_pricing = self.payload.tiered_pricing
        if hasattr(self.payload, 'price_per_request') and self.payload.price_per_request is not None:
            price_per_request = self.payload.price_per_request

        for model_id in self.payload.model_ids:
            # 输入验证：检查 model_id 长度
            if not model_id or len(model_id) > 100:
                errors.append(
                    ImportFromUpstreamErrorItem(
                        model_id=model_id[:50] + "..." if model_id and len(model_id) > 50 else model_id or "<empty>",
                        error="Invalid model_id: must be 1-100 characters",
                    )
                )
                continue

            try:
                # 使用 savepoint 确保单个模型导入的原子性
                savepoint = db.begin_nested()
                try:
                    # 1. 检查是否已存在同名的 ProviderModel
                    existing = (
                        db.query(Model)
                        .filter(
                            Model.provider_id == self.provider_id,
                            Model.provider_model_name == model_id,
                        )
                        .first()
                    )
                    if existing:
                        # 已存在，提交 savepoint 并记录成功
                        savepoint.commit()
                        success.append(
                            ImportFromUpstreamSuccessItem(
                                model_id=model_id,
                                global_model_id=existing.global_model_id or "",
                                global_model_name=existing.global_model.name if existing.global_model else "",
                                provider_model_id=existing.id,
                                created_global_model=False,
                            )
                        )
                        continue

                    # 2. 创建新的 Model 记录（不关联 GlobalModel）
                    new_model = Model(
                        provider_id=self.provider_id,
                        global_model_id=None,  # 独立模型，不关联 GlobalModel
                        provider_model_name=model_id,
                        is_active=True,
                        tiered_pricing=tiered_pricing,
                        price_per_request=price_per_request,
                    )
                    db.add(new_model)
                    db.flush()

                    # 提交 savepoint
                    savepoint.commit()
                    success.append(
                        ImportFromUpstreamSuccessItem(
                            model_id=model_id,
                            global_model_id="",  # 未关联
                            global_model_name="",  # 未关联
                            provider_model_id=new_model.id,
                            created_global_model=False,
                        )
                    )
                    logger.info(
                        f"Created independent ProviderModel: {model_id} for provider {provider.name}"
                    )
                except Exception as e:
                    # 回滚到 savepoint
                    savepoint.rollback()
                    raise e
            except Exception as e:
                logger.error(f"Error importing model {model_id}: {e}")
                errors.append(ImportFromUpstreamErrorItem(model_id=model_id, error=str(e)))

        db.commit()
        logger.info(
            f"Imported {len(success)} independent models to provider {provider.name} by {context.user.username}"
        )

        # 不需要清除 /v1/models 缓存，因为独立模型不参与路由

        return ImportFromUpstreamResponse(success=success, errors=errors)
