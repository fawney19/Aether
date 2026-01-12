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
from src.models.database import (
    ApiKey,
    GlobalModel,
    Model,
    Provider,
    ProviderAPIKey,
    ProviderEndpoint,
    User,
)

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


async def invalidate_models_list_cache() -> None:
    """
    清除所有 /v1/models 列表缓存

    在模型创建、更新、删除时调用，确保模型列表实时更新
    """
    # 清除所有格式的缓存
    all_formats = ["CLAUDE", "OPENAI", "GEMINI"]
    for fmt in all_formats:
        cache_key = f"{_CACHE_KEY_PREFIX}:{fmt}"
        try:
            await CacheService.delete(cache_key)
            logger.debug(f"[ModelsService] 已清除缓存: {cache_key}")
        except Exception as e:
            logger.warning(f"[ModelsService] 清除缓存失败 {cache_key}: {e}")


@dataclass
class ModelInfo:
    """统一的模型信息结构"""

    id: str  # 模型 ID (GlobalModel.name 或 provider_model_name)
    display_name: str
    description: Optional[str]
    created_at: Optional[str]  # ISO 格式
    created_timestamp: int  # Unix 时间戳
    provider_name: str
    provider_id: str = ""  # Provider ID，用于权限过滤
    # 能力配置
    streaming: bool = True
    vision: bool = False
    function_calling: bool = False
    extended_thinking: bool = False
    image_generation: bool = False
    structured_output: bool = False
    # 规格参数
    context_limit: Optional[int] = None
    output_limit: Optional[int] = None
    # 元信息
    family: Optional[str] = None
    knowledge_cutoff: Optional[str] = None
    input_modalities: Optional[list[str]] = None
    output_modalities: Optional[list[str]] = None


@dataclass
class AccessRestrictions:
    """API Key 或 User 的访问限制"""

    allowed_providers: Optional[list[str]] = None  # 允许的 Provider ID 列表
    allowed_models: Optional[list[str]] = None  # 允许的模型名称列表
    allowed_api_formats: Optional[list[str]] = None  # 允许的 API 格式列表

    @classmethod
    def from_api_key_and_user(
        cls, api_key: Optional[ApiKey], user: Optional[User]
    ) -> "AccessRestrictions":
        """
        从 API Key 和 User 合并访问限制

        限制逻辑:
        - API Key 的限制优先于 User 的限制
        - 如果 API Key 有限制，使用 API Key 的限制
        - 如果 API Key 无限制但 User 有限制，使用 User 的限制
        - 两者都无限制则返回空限制
        """
        allowed_providers: Optional[list[str]] = None
        allowed_models: Optional[list[str]] = None
        allowed_api_formats: Optional[list[str]] = None

        # 优先使用 API Key 的限制
        if api_key:
            if api_key.allowed_providers is not None:
                allowed_providers = api_key.allowed_providers
            if api_key.allowed_models is not None:
                allowed_models = api_key.allowed_models
            if api_key.allowed_api_formats is not None:
                allowed_api_formats = api_key.allowed_api_formats

        # 如果 API Key 没有限制，检查 User 的限制
        if user:
            if allowed_providers is None and user.allowed_providers is not None:
                allowed_providers = user.allowed_providers
            if allowed_models is None and user.allowed_models is not None:
                allowed_models = user.allowed_models
            if allowed_api_formats is None and user.allowed_api_formats is not None:
                allowed_api_formats = user.allowed_api_formats

        return cls(
            allowed_providers=allowed_providers,
            allowed_models=allowed_models,
            allowed_api_formats=allowed_api_formats,
        )

    def is_api_format_allowed(self, api_format: str) -> bool:
        """
        检查 API 格式是否被允许

        Args:
            api_format: API 格式 (如 "OPENAI", "CLAUDE", "GEMINI")

        Returns:
            True 如果格式被允许，False 否则
        """
        if self.allowed_api_formats is None:
            return True
        return api_format in self.allowed_api_formats

    def is_model_allowed(self, model_id: str, provider_id: str) -> bool:
        """
        检查模型是否被允许访问

        Args:
            model_id: 模型 ID
            provider_id: Provider ID

        Returns:
            True 如果模型被允许，False 否则
        """
        # 检查 Provider 限制
        if self.allowed_providers is not None:
            if provider_id not in self.allowed_providers:
                return False

        # 检查模型限制
        if self.allowed_models is not None:
            if model_id not in self.allowed_models:
                return False

        return True


def get_available_provider_ids(db: Session, api_formats: list[str]) -> set[str]:
    """
    返回有可用端点的 Provider IDs

    条件:
    - 端点 api_format 匹配
    - 端点是活跃的
    - Provider 下有活跃的 Key 且支持该 api_format（Key 直属 Provider，通过 api_formats 过滤）
    """
    target_formats = {f.upper() for f in api_formats}

    # 1) 先找出有活跃端点的 Provider（记录每个 Provider 支持的格式集合）
    endpoint_rows = (
        db.query(ProviderEndpoint.provider_id, ProviderEndpoint.api_format)
        .filter(
            ProviderEndpoint.api_format.in_(list(target_formats)),
            ProviderEndpoint.is_active.is_(True),
        )
        .all()
    )

    if not endpoint_rows:
        return set()

    provider_to_formats: dict[str, set[str]] = {}
    for provider_id, fmt in endpoint_rows:
        if not provider_id or not fmt:
            continue
        provider_to_formats.setdefault(provider_id, set()).add(str(fmt).upper())

    provider_ids_with_endpoints = set(provider_to_formats.keys())
    if not provider_ids_with_endpoints:
        return set()

    # 2) 再检查这些 Provider 是否至少有一个活跃 Key 支持对应格式
    key_rows = (
        db.query(ProviderAPIKey.provider_id, ProviderAPIKey.api_formats)
        .filter(
            ProviderAPIKey.provider_id.in_(provider_ids_with_endpoints),
            ProviderAPIKey.is_active.is_(True),
        )
        .all()
    )

    available_provider_ids: set[str] = set()
    for provider_id, key_formats in key_rows:
        if not provider_id:
            continue
        endpoint_formats = provider_to_formats.get(provider_id)
        if not endpoint_formats:
            continue

        formats_list = key_formats if isinstance(key_formats, list) else []
        key_formats_upper = {str(f).upper() for f in formats_list}

        # 只有同时满足：请求格式 ∩ Provider 端点格式 ∩ Key 支持格式 非空，才算可用
        if key_formats_upper & endpoint_formats & target_formats:
            available_provider_ids.add(provider_id)

    return available_provider_ids


def _get_available_model_ids_for_format(db: Session, api_formats: list[str]) -> set[str]:
    """
    获取指定格式下真正可用的模型 ID 集合

    一个模型可用需满足:
    1. 端点 api_format 匹配且活跃
    2. 端点下有活跃的 Key
    3. **该端点的 Provider 关联了该模型**
    4. Key 的 allowed_models 允许该模型（null = 允许该 Provider 关联的所有模型）
    """
    target_formats = {f.upper() for f in api_formats}

    # 1) 找出有活跃端点的 Provider（记录每个 Provider 支持的格式集合）
    endpoint_rows = (
        db.query(ProviderEndpoint.provider_id, ProviderEndpoint.api_format)
        .filter(
            ProviderEndpoint.api_format.in_(list(target_formats)),
            ProviderEndpoint.is_active.is_(True),
        )
        .all()
    )

    if not endpoint_rows:
        return set()

    provider_to_formats: dict[str, set[str]] = {}
    for provider_id, fmt in endpoint_rows:
        if not provider_id or not fmt:
            continue
        provider_to_formats.setdefault(provider_id, set()).add(str(fmt).upper())

    provider_ids_with_endpoints = set(provider_to_formats.keys())
    if not provider_ids_with_endpoints:
        return set()

    # 2) 收集每个 Provider 下「支持对应格式」的活跃 Key 的 allowed_models
    # Key 直属 Provider，通过 key.api_formats 与 Provider 端点格式交集筛选
    key_rows = (
        db.query(ProviderAPIKey.provider_id, ProviderAPIKey.allowed_models, ProviderAPIKey.api_formats)
        .filter(
            ProviderAPIKey.provider_id.in_(provider_ids_with_endpoints),
            ProviderAPIKey.is_active.is_(True),
        )
        .all()
    )

    # provider_id -> list[(allowed_models, usable_formats)]
    provider_key_rules: dict[str, list[tuple[object, set[str]]]] = {}
    for provider_id, allowed_models, key_formats in key_rows:
        if not provider_id:
            continue

        endpoint_formats = provider_to_formats.get(provider_id)
        if not endpoint_formats:
            continue

        formats_list = key_formats if isinstance(key_formats, list) else []
        key_formats_upper = {str(f).upper() for f in formats_list}
        usable_formats = key_formats_upper & endpoint_formats & target_formats
        if not usable_formats:
            continue

        provider_key_rules.setdefault(provider_id, []).append((allowed_models, usable_formats))

    provider_ids_with_format = set(provider_key_rules.keys())
    if not provider_ids_with_format:
        return set()

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

        # 检查该 provider 下是否有 Key 允许这个模型（支持 list/dict 两种 allowed_models）
        from src.core.model_permissions import check_model_allowed

        rules = provider_key_rules.get(model_provider_id, [])
        for allowed_models, usable_formats in rules:
            # None = 不限制
            if allowed_models is None:
                available_model_ids.add(model_id)
                break

            # 对于支持多个格式的 Key：任意一个可用格式允许即可
            for fmt in usable_formats:
                if check_model_allowed(
                    model_name=model_id,
                    allowed_models=allowed_models,  # type: ignore[arg-type]
                    api_format=fmt,
                    resolved_model_name=(model.provider_model_name if global_model else None),
                ):
                    available_model_ids.add(model_id)
                    break
            else:
                continue
            break

    return available_model_ids


def _extract_model_info(model: Any) -> ModelInfo:
    """从 Model 对象提取 ModelInfo"""
    global_model = model.global_model
    model_id: str = global_model.name if global_model else model.provider_model_name
    display_name: str = global_model.display_name if global_model else model.provider_model_name
    created_at: Optional[str] = (
        model.created_at.strftime("%Y-%m-%dT%H:%M:%SZ") if model.created_at else None
    )
    created_timestamp: int = int(model.created_at.timestamp()) if model.created_at else 0
    provider_name: str = model.provider.name if model.provider else "unknown"
    provider_id: str = model.provider_id or ""

    # 从 GlobalModel.config 提取配置信息
    config: dict = {}
    description: Optional[str] = None
    if global_model:
        config = global_model.config or {}
        description = config.get("description")

    return ModelInfo(
        id=model_id,
        display_name=display_name,
        description=description,
        created_at=created_at,
        created_timestamp=created_timestamp,
        provider_name=provider_name,
        provider_id=provider_id,
        # 能力配置
        streaming=config.get("streaming", True),
        vision=config.get("vision", False),
        function_calling=config.get("function_calling", False),
        extended_thinking=config.get("extended_thinking", False),
        image_generation=config.get("image_generation", False),
        structured_output=config.get("structured_output", False),
        # 规格参数
        context_limit=config.get("context_limit"),
        output_limit=config.get("output_limit"),
        # 元信息
        family=config.get("family"),
        knowledge_cutoff=config.get("knowledge_cutoff"),
        input_modalities=config.get("input_modalities"),
        output_modalities=config.get("output_modalities"),
    )


async def list_available_models(
    db: Session,
    available_provider_ids: set[str],
    api_formats: Optional[list[str]] = None,
    restrictions: Optional[AccessRestrictions] = None,
) -> list[ModelInfo]:
    """
    获取可用模型列表（已去重，带缓存）

    Args:
        db: 数据库会话
        available_provider_ids: 有可用端点的 Provider ID 集合
        api_formats: API 格式列表，用于检查 Key 的 allowed_models
        restrictions: API Key/User 的访问限制

    Returns:
        去重后的 ModelInfo 列表，按创建时间倒序
    """
    if not available_provider_ids:
        return []

    # 缓存策略：只有完全无访问限制时才使用缓存
    # - restrictions is None: 未传入限制对象
    # - restrictions 的两个字段都为 None: 传入了限制对象但无实际限制
    # 以上两种情况返回的结果相同，可以共享全局缓存
    use_cache = restrictions is None or (
        restrictions.allowed_providers is None and restrictions.allowed_models is None
    )

    # 尝试从缓存获取
    if api_formats and use_cache:
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

        # 检查 API Key/User 访问限制
        if restrictions is not None:
            if not restrictions.is_model_allowed(info.id, info.provider_id):
                continue

        if info.id in seen_model_ids:
            continue
        seen_model_ids.add(info.id)

        result.append(info)

    # 只有无限制的情况才写入缓存
    if api_formats and use_cache:
        await _set_cached_models(api_formats, result)

    return result


def find_model_by_id(
    db: Session,
    model_id: str,
    available_provider_ids: set[str],
    api_formats: Optional[list[str]] = None,
    restrictions: Optional[AccessRestrictions] = None,
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
        restrictions: API Key/User 的访问限制

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

    # 快速检查：如果 restrictions 明确限制了模型列表且目标模型不在其中，直接返回 None
    if restrictions is not None and restrictions.allowed_models is not None:
        if model_id not in restrictions.allowed_models:
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

    def is_model_accessible(m: Model) -> bool:
        """检查模型是否可访问"""
        if m.provider_id not in available_provider_ids:
            return False
        # 检查 API Key/User 访问限制
        if restrictions is not None:
            provider_id = m.provider_id or ""
            if not restrictions.is_model_allowed(model_id, provider_id):
                return False
        return True

    model = next(
        (m for m in models_by_global if is_model_accessible(m)),
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
            (m for m in models_by_provider_name if is_model_accessible(m)),
            None,
        )

    if not model:
        return None

    return _extract_model_info(model)
