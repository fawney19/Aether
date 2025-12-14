"""
公共模型查询服务

为 Claude/OpenAI/Gemini 的 /models 端点提供统一的查询逻辑

查询逻辑:
1. 找到指定 api_format 的活跃端点
2. 端点下有活跃的 Key
3. Provider 关联了该模型（Model 表）
4. Key 的 allowed_models 允许该模型（null = 允许所有）
"""

from dataclasses import asdict, dataclass
from typing import Any, Optional

from sqlalchemy.orm import Session, joinedload

from src.config.constants import CacheTTL
from src.core.cache_service import CacheService
from src.core.logger import logger
from src.models.database import GlobalModel, Model, Provider, ProviderAPIKey, ProviderEndpoint

# 缓存 key 前缀
_CACHE_KEY_PREFIX = "models:list"
_CACHE_TTL = CacheTTL.MODEL  # 300 秒


def _get_cache_key(api_formats: list[str]) -> str:
    """生成缓存 key"""
    formats_str = ",".join(sorted(api_formats))
    return f"{_CACHE_KEY_PREFIX}:{formats_str}"


async def _get_cached_models(api_formats: list[str]) -> Optional[list["ModelInfo"]]:
    """从缓存获取模型列表"""
    cache_key = _get_cache_key(api_formats)
    try:
        cached = await CacheService.get(cache_key)
        if cached:
            logger.debug(f"[ModelsService] 缓存命中: {cache_key}, {len(cached)} 个模型")
            return [ModelInfo(**item) for item in cached]
    except Exception as e:
        logger.warning(f"[ModelsService] 缓存读取失败: {e}")
    return None


async def _set_cached_models(api_formats: list[str], models: list["ModelInfo"]) -> None:
    """将模型列表写入缓存"""
    cache_key = _get_cache_key(api_formats)
    try:
        data = [asdict(m) for m in models]
        await CacheService.set(cache_key, data, ttl_seconds=_CACHE_TTL)
        logger.debug(f"[ModelsService] 已缓存: {cache_key}, {len(models)} 个模型, TTL={_CACHE_TTL}s")
    except Exception as e:
        logger.warning(f"[ModelsService] 缓存写入失败: {e}")


@dataclass
class ModelInfo:
    """统一的模型信息结构"""

    id: str  # 模型 ID (GlobalModel.name 或 provider_model_name)
    display_name: str
    description: Optional[str]
    created_at: Optional[str]  # ISO 格式
    created_timestamp: int  # Unix 时间戳
    provider_name: str


def get_available_provider_ids(db: Session, api_formats: list[str]) -> set[str]:
    """
    返回有可用端点的 Provider IDs

    条件:
    - 端点 api_format 匹配
    - 端点是活跃的
    - 端点下有活跃的 Key
    """
    rows = (
        db.query(ProviderEndpoint.provider_id)
        .join(ProviderAPIKey, ProviderAPIKey.endpoint_id == ProviderEndpoint.id)
        .filter(
            ProviderEndpoint.api_format.in_(api_formats),
            ProviderEndpoint.is_active.is_(True),
            ProviderAPIKey.is_active.is_(True),
        )
        .distinct()
        .all()
    )
    return {row[0] for row in rows}


def _get_available_model_ids_for_format(db: Session, api_formats: list[str]) -> set[str]:
    """
    获取指定格式下真正可用的模型 ID 集合

    一个模型可用需满足:
    1. 端点 api_format 匹配且活跃
    2. 端点下有活跃的 Key
    3. **该端点的 Provider 关联了该模型**
    4. Key 的 allowed_models 允许该模型（null = 允许该 Provider 关联的所有模型）
    """
    # 查询所有匹配格式的活跃端点及其活跃 Key，同时获取 endpoint_id
    endpoint_keys = (
        db.query(
            ProviderEndpoint.id.label("endpoint_id"),
            ProviderEndpoint.provider_id,
            ProviderAPIKey.allowed_models,
        )
        .join(ProviderAPIKey, ProviderAPIKey.endpoint_id == ProviderEndpoint.id)
        .filter(
            ProviderEndpoint.api_format.in_(api_formats),
            ProviderEndpoint.is_active.is_(True),
            ProviderAPIKey.is_active.is_(True),
        )
        .all()
    )

    if not endpoint_keys:
        return set()

    # 收集每个 (provider_id, endpoint_id) 对应的 allowed_models
    # 使用 provider_id 作为 key，因为模型是关联到 Provider 的
    provider_allowed_models: dict[str, list[Optional[list[str]]]] = {}
    provider_ids_with_format: set[str] = set()

    for endpoint_id, provider_id, allowed_models in endpoint_keys:
        provider_ids_with_format.add(provider_id)
        if provider_id not in provider_allowed_models:
            provider_allowed_models[provider_id] = []
        provider_allowed_models[provider_id].append(allowed_models)

    # 只查询那些有匹配格式端点的 Provider 下的模型
    models = (
        db.query(Model)
        .options(joinedload(Model.global_model))
        .join(Provider)
        .filter(
            Model.provider_id.in_(provider_ids_with_format),
            Model.is_active.is_(True),
            Provider.is_active.is_(True),
        )
        .all()
    )

    available_model_ids: set[str] = set()

    for model in models:
        model_provider_id = model.provider_id
        global_model = model.global_model
        model_id = global_model.name if global_model else model.provider_model_name  # type: ignore

        if not model_provider_id or not model_id:
            continue

        # 该模型的 Provider 必须有匹配格式的端点
        if model_provider_id not in provider_ids_with_format:
            continue

        # 检查该 provider 下是否有 Key 允许这个模型
        allowed_lists = provider_allowed_models.get(model_provider_id, [])
        for allowed_models in allowed_lists:
            if allowed_models is None:
                # null = 允许该 Provider 关联的所有模型（已通过上面的查询限制）
                available_model_ids.add(model_id)
                break
            elif model_id in allowed_models:
                # 明确在允许列表中
                available_model_ids.add(model_id)
                break
            elif global_model and model.provider_model_name in allowed_models:
                # 也检查 provider_model_name
                available_model_ids.add(model_id)
                break

    return available_model_ids


def _extract_model_info(model: Any) -> ModelInfo:
    """从 Model 对象提取 ModelInfo"""
    global_model = model.global_model
    model_id: str = global_model.name if global_model else model.provider_model_name
    display_name: str = global_model.display_name if global_model else model.provider_model_name
    description: Optional[str] = global_model.description if global_model else None
    created_at: Optional[str] = (
        model.created_at.strftime("%Y-%m-%dT%H:%M:%SZ") if model.created_at else None
    )
    created_timestamp: int = int(model.created_at.timestamp()) if model.created_at else 0
    provider_name: str = model.provider.name if model.provider else "unknown"

    return ModelInfo(
        id=model_id,
        display_name=display_name,
        description=description,
        created_at=created_at,
        created_timestamp=created_timestamp,
        provider_name=provider_name,
    )


async def list_available_models(
    db: Session,
    available_provider_ids: set[str],
    api_formats: Optional[list[str]] = None,
) -> list[ModelInfo]:
    """
    获取可用模型列表（已去重，带缓存）

    Args:
        db: 数据库会话
        available_provider_ids: 有可用端点的 Provider ID 集合
        api_formats: API 格式列表，用于检查 Key 的 allowed_models

    Returns:
        去重后的 ModelInfo 列表，按创建时间倒序
    """
    if not available_provider_ids:
        return []

    # 尝试从缓存获取
    if api_formats:
        cached = await _get_cached_models(api_formats)
        if cached is not None:
            return cached

    # 如果提供了 api_formats，获取真正可用的模型 ID
    available_model_ids: Optional[set[str]] = None
    if api_formats:
        available_model_ids = _get_available_model_ids_for_format(db, api_formats)
        if not available_model_ids:
            return []

    query = (
        db.query(Model)
        .options(joinedload(Model.global_model), joinedload(Model.provider))
        .join(Provider)
        .filter(
            Model.is_active.is_(True),
            Provider.is_active.is_(True),
            Model.provider_id.in_(available_provider_ids),
        )
        .order_by(Model.created_at.desc())
    )
    all_models = query.all()

    result: list[ModelInfo] = []
    seen_model_ids: set[str] = set()

    for model in all_models:
        info = _extract_model_info(model)

        # 如果有 available_model_ids 限制，检查是否在其中
        if available_model_ids is not None and info.id not in available_model_ids:
            continue

        if info.id in seen_model_ids:
            continue
        seen_model_ids.add(info.id)

        result.append(info)

    # 写入缓存
    if api_formats:
        await _set_cached_models(api_formats, result)

    return result


def find_model_by_id(
    db: Session,
    model_id: str,
    available_provider_ids: set[str],
    api_formats: Optional[list[str]] = None,
) -> Optional[ModelInfo]:
    """
    按 ID 查找模型

    查找顺序：
    1. 先按 GlobalModel.name 查找
    2. 如果没找到任何候选，再按 provider_model_name 查找
    3. 如果有候选但都不可用，返回 None（不回退）

    Args:
        db: 数据库会话
        model_id: 模型 ID
        available_provider_ids: 有可用端点的 Provider ID 集合
        api_formats: API 格式列表，用于检查 Key 的 allowed_models

    Returns:
        ModelInfo 或 None
    """
    if not available_provider_ids:
        return None

    # 如果提供了 api_formats，获取真正可用的模型 ID
    available_model_ids: Optional[set[str]] = None
    if api_formats:
        available_model_ids = _get_available_model_ids_for_format(db, api_formats)
        # 快速检查：如果目标模型不在可用列表中，直接返回 None
        if available_model_ids is not None and model_id not in available_model_ids:
            return None

    # 先按 GlobalModel.name 查找
    models_by_global = (
        db.query(Model)
        .options(joinedload(Model.global_model), joinedload(Model.provider))
        .join(Provider)
        .join(GlobalModel, Model.global_model_id == GlobalModel.id)
        .filter(
            GlobalModel.name == model_id,
            Model.is_active.is_(True),
            Provider.is_active.is_(True),
        )
        .order_by(Model.created_at.desc())
        .all()
    )

    model = next(
        (m for m in models_by_global if m.provider_id in available_provider_ids),
        None,
    )

    # 如果有候选但都不可用，直接返回 None（不回退 provider_model_name）
    if not model and models_by_global:
        return None

    # 如果找不到任何候选，按 provider_model_name 查找
    if not model:
        models_by_provider_name = (
            db.query(Model)
            .options(joinedload(Model.global_model), joinedload(Model.provider))
            .join(Provider)
            .filter(
                Model.provider_model_name == model_id,
                Model.is_active.is_(True),
                Provider.is_active.is_(True),
            )
            .order_by(Model.created_at.desc())
            .all()
        )

        model = next(
            (m for m in models_by_provider_name if m.provider_id in available_provider_ids),
            None,
        )

    if not model:
        return None

    return _extract_model_info(model)
