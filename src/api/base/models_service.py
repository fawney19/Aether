"""
公共模型查询服务

为 Claude/OpenAI/Gemini 的 /models 端点提供统一的查询逻辑

查询逻辑:
1. 找到指定 api_format 的活跃端点
2. 端点下有活跃的 Key
3. Provider 关联了该模型（Model 表）
4. Key 的 allowed_models 允许该模型（null = 允许所有）
"""

from __future__ import annotations

from dataclasses import asdict, dataclass
from typing import Any

from sqlalchemy import tuple_
from sqlalchemy.orm import Session

from src.config.constants import CacheTTL
from src.core.access_restrictions import AccessRestrictions
from src.core.api_format.conversion.compatibility import is_format_compatible
from src.core.cache_service import CacheService
from src.core.logger import logger
from src.models.database import Model, Provider, ProviderEndpoint
from src.services.cache.model_list_cache import MODELS_LIST_CACHE_PREFIX as _CACHE_KEY_PREFIX
from src.services.cache.model_list_cache import (
    invalidate_models_list_cache,
)
from src.services.model.availability import ModelAvailabilityQuery
from src.services.provider.format import normalize_endpoint_signature

_CACHE_TTL = CacheTTL.MODEL  # 300 秒


def _get_cache_key(api_formats: list[str], client_format: str | None = None) -> str:
    """生成缓存 key"""
    formats_str = ",".join(sorted(api_formats))
    format_key = (client_format or "any").lower()
    return f"{_CACHE_KEY_PREFIX}:{format_key}:{formats_str}"


async def _get_cached_models(
    api_formats: list[str], client_format: str | None = None
) -> list[ModelInfo] | None:
    """从缓存获取模型列表"""
    cache_key = _get_cache_key(api_formats, client_format)
    try:
        cached = await CacheService.get(cache_key)
        if cached:
            logger.debug(f"[ModelsService] 缓存命中: {cache_key}, {len(cached)} 个模型")
            return [ModelInfo(**item) for item in cached]
    except Exception as e:
        logger.warning(f"[ModelsService] 缓存读取失败: {e}")
    return None


async def _set_cached_models(
    api_formats: list[str],
    models: list[ModelInfo],
    client_format: str | None = None,
) -> None:
    """将模型列表写入缓存"""
    cache_key = _get_cache_key(api_formats, client_format)
    try:
        data = [asdict(m) for m in models]
        await CacheService.set(cache_key, data, ttl_seconds=_CACHE_TTL)
        logger.debug(
            f"[ModelsService] 已缓存: {cache_key}, {len(models)} 个模型, TTL={_CACHE_TTL}s"
        )
    except Exception as e:
        logger.warning(f"[ModelsService] 缓存写入失败: {e}")


_PUBLIC_GLOBAL_MODEL_CONFIG_KEYS = (
    "description",
    "icon_url",
    "streaming",
    "vision",
    "function_calling",
    "extended_thinking",
    "image_generation",
    "structured_output",
    "family",
    "knowledge_cutoff",
    "input_modalities",
    "output_modalities",
    "context_limit",
    "output_limit",
)


def sanitize_public_global_model_config(raw_config: dict[str, Any] | None) -> dict[str, Any] | None:
    """Return only user-safe GlobalModel config keys.

    Internal routing fields such as ``model_mappings`` should never be exposed
    through public or user-facing model catalog APIs.
    """
    if not isinstance(raw_config, dict):
        return None

    sanitized = {
        key: raw_config[key]
        for key in _PUBLIC_GLOBAL_MODEL_CONFIG_KEYS
        if key in raw_config
    }
    return sanitized or None


__all__ = [
    "AccessRestrictions",
    "invalidate_models_list_cache",
    "ModelInfo",
    "sanitize_public_global_model_config",
]


@dataclass
class ModelInfo:
    """统一的模型信息结构"""

    id: str  # 模型 ID (GlobalModel.name 或 provider_model_name)
    display_name: str
    description: str | None
    created_at: str | None  # ISO 格式
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
    context_limit: int | None = None
    output_limit: int | None = None
    # 元信息
    family: str | None = None
    knowledge_cutoff: str | None = None
    input_modalities: list[str] | None = None
    output_modalities: list[str] | None = None


# AccessRestrictions -- re-export from src.core.access_restrictions (see __all__)


def _normalize_api_formats(
    api_formats: list[str] | None,
    provider_to_formats: dict[str, set[str]] | None = None,
) -> list[str]:
    """规范化 API 格式列表（endpoint signature，小写 canonical），必要时从 provider_to_formats 兜底"""
    if api_formats:
        return [normalize_endpoint_signature(str(fmt)) for fmt in api_formats if fmt]
    if not provider_to_formats:
        return []
    all_formats: set[str] = set()
    for formats in provider_to_formats.values():
        all_formats.update(normalize_endpoint_signature(str(fmt)) for fmt in formats if fmt)
    return list(all_formats)


def _get_provider_model_names_for_formats(
    model: Model, usable_formats: set[str] | None = None
) -> set[str]:
    """
    获取模型在指定格式下支持的 Provider 模型名称集合

    用于 check_model_allowed_with_mappings 的 candidate_models 参数，
    确保权限检查时只考虑当前格式支持的模型名。
    """
    names: set[str] = {model.provider_model_name}
    raw_mappings = model.provider_model_mappings
    if not isinstance(raw_mappings, list):
        return names

    usable_formats_norm = (
        {normalize_endpoint_signature(f) for f in usable_formats} if usable_formats else None
    )

    for raw in raw_mappings:
        if not isinstance(raw, dict):
            continue
        name = raw.get("name")
        if not isinstance(name, str) or not name.strip():
            continue

        mapping_api_formats = raw.get("api_formats")
        if usable_formats_norm and mapping_api_formats and isinstance(mapping_api_formats, list):
            mapping_formats = {
                normalize_endpoint_signature(str(fmt)) for fmt in mapping_api_formats if fmt
            }
            if not mapping_formats & usable_formats_norm:
                continue

        names.add(name.strip())

    return names


def get_compatible_provider_formats(
    db: Session,
    client_format: str,
    api_formats: list[str],
    global_conversion_enabled: bool,
) -> dict[str, set[str]]:
    """
    获取与客户端格式兼容的 Provider -> formats 映射

    兼容性基于端点格式、format_acceptance_config 与全局格式转换开关。
    """
    normalized_formats = _normalize_api_formats(api_formats)
    if not normalized_formats:
        return {}

    target_pairs: list[tuple[str, str]] = []
    for fmt in normalized_formats:
        try:
            fam, kind = fmt.split(":", 1)
        except ValueError:
            continue
        if fam and kind:
            target_pairs.append((fam, kind))
    if not target_pairs:
        return {}

    client_format_norm = normalize_endpoint_signature(client_format)

    endpoint_rows = (
        db.query(
            ProviderEndpoint.provider_id,
            ProviderEndpoint.api_family,
            ProviderEndpoint.endpoint_kind,
            ProviderEndpoint.format_acceptance_config,
            Provider.enable_format_conversion,
        )
        .join(Provider, ProviderEndpoint.provider_id == Provider.id)
        .filter(
            Provider.is_active.is_(True),
            ProviderEndpoint.is_active.is_(True),
            ProviderEndpoint.api_family.isnot(None),
            ProviderEndpoint.endpoint_kind.isnot(None),
            tuple_(ProviderEndpoint.api_family, ProviderEndpoint.endpoint_kind).in_(target_pairs),
        )
        .all()
    )

    provider_to_formats: dict[str, set[str]] = {}
    for (
        provider_id,
        api_family,
        endpoint_kind,
        format_acceptance_config,
        provider_conversion_enabled,
    ) in endpoint_rows:
        if not provider_id or not api_family or not endpoint_kind:
            continue
        endpoint_format = normalize_endpoint_signature(f"{api_family}:{endpoint_kind}")
        skip_endpoint_check = global_conversion_enabled or bool(provider_conversion_enabled)
        is_compatible, _needs_conversion, _reason = is_format_compatible(
            client_format_norm,
            endpoint_format,
            format_acceptance_config,
            is_stream=False,
            effective_conversion_enabled=global_conversion_enabled,
            skip_endpoint_check=skip_endpoint_check,
        )
        if not is_compatible:
            continue
        provider_to_formats.setdefault(provider_id, set()).add(endpoint_format)

    return provider_to_formats


def get_available_provider_ids(
    db: Session,
    api_formats: list[str],
    provider_to_formats: dict[str, set[str]] | None = None,
) -> set[str]:
    """
    返回有可用端点的 Provider IDs

    条件:
    - 端点 api_format 匹配
    - 端点是活跃的
    - Provider 下有活跃的 Key 且支持该 api_format（Key 直属 Provider，通过 api_formats 过滤）
    """
    normalized_formats = _normalize_api_formats(api_formats, provider_to_formats)
    if provider_to_formats is None:
        provider_to_formats = ModelAvailabilityQuery.get_providers_with_active_endpoints(
            db, normalized_formats
        )
    if not provider_to_formats:
        return set()

    return ModelAvailabilityQuery.get_providers_with_active_keys(
        db,
        set(provider_to_formats.keys()),
        normalized_formats,
        provider_to_formats,
    )


def _get_available_model_ids_for_format(
    db: Session,
    api_formats: list[str],
    provider_to_formats: dict[str, set[str]] | None = None,
) -> set[str]:
    """
    获取指定格式下真正可用的模型 ID 集合

    一个模型可用需满足:
    1. 端点 api_format 匹配且活跃
    2. 端点下有活跃的 Key
    3. **该端点的 Provider 关联了该模型**
    4. Key 的 allowed_models 允许该模型（null = 允许该 Provider 关联的所有模型）
    """
    normalized_formats = _normalize_api_formats(api_formats, provider_to_formats)
    if provider_to_formats is None:
        provider_to_formats = ModelAvailabilityQuery.get_providers_with_active_endpoints(
            db, normalized_formats
        )
    if not provider_to_formats:
        return set()

    provider_key_rules = ModelAvailabilityQuery.get_provider_key_rules(
        db,
        provider_ids=set(provider_to_formats.keys()),
        api_formats=normalized_formats,
        provider_to_endpoint_formats=provider_to_formats,
    )

    provider_ids_with_format = set(provider_key_rules.keys())
    if not provider_ids_with_format:
        return set()

    models = (
        ModelAvailabilityQuery.base_active_models(db, eager_load=True)
        .filter(Model.provider_id.in_(provider_ids_with_format))
        .all()
    )

    available_model_ids: set[str] = set()

    for model in models:
        model_provider_id = model.provider_id
        global_model = model.global_model
        if not model_provider_id or not global_model or not global_model.name:
            continue

        # 该模型的 Provider 必须有匹配格式的端点
        if model_provider_id not in provider_ids_with_format:
            continue

        # 检查该 provider 下是否有 Key 允许这个模型
        from src.core.model_permissions import check_model_allowed_with_mappings

        model_id = global_model.name
        model_mappings = (global_model.config or {}).get("model_mappings")

        rules = provider_key_rules.get(model_provider_id, [])
        for allowed_models, usable_formats in rules:
            # None = 不限制
            if allowed_models is None:
                available_model_ids.add(model_id)
                break

            # 检查是否允许该模型（支持 model_mappings 正则匹配）
            candidate_models = _get_provider_model_names_for_formats(model, usable_formats)
            is_allowed, _ = check_model_allowed_with_mappings(
                model_name=model_id,
                allowed_models=allowed_models,
                model_mappings=model_mappings,
                candidate_models=candidate_models,
            )
            if is_allowed:
                available_model_ids.add(model_id)
                break

    return available_model_ids


def _extract_model_info(model: Any) -> ModelInfo | None:
    """
    从 Model 对象提取 ModelInfo

    前置条件：model 必须关联 GlobalModel（由 base_active_models 内连接保证）
    如果 global_model 为 None（不应发生），返回 None 并记录日志。
    """
    global_model = model.global_model
    if global_model is None:
        logger.warning(
            f"[ModelService] Model {getattr(model, 'id', 'unknown')} 缺少 global_model，跳过"
        )
        return None

    model_id: str = global_model.name
    display_name: str = global_model.display_name
    created_at: str | None = (
        model.created_at.strftime("%Y-%m-%dT%H:%M:%SZ") if model.created_at else None
    )
    created_timestamp: int = int(model.created_at.timestamp()) if model.created_at else 0
    provider_name: str = model.provider.name if model.provider else "unknown"
    provider_id: str = model.provider_id or ""

    # 从 GlobalModel.config 提取配置信息
    config: dict = global_model.config or {}
    description: str | None = config.get("description")

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
    api_formats: list[str] | None = None,
    restrictions: AccessRestrictions | None = None,
    provider_to_formats: dict[str, set[str]] | None = None,
    client_format: str | None = None,
) -> list[ModelInfo]:
    """
    获取可用模型列表（已去重，带缓存）

    Args:
        db: 数据库会话
        available_provider_ids: 有可用端点的 Provider ID 集合
        api_formats: API 格式列表，用于检查 Key 的 allowed_models
        restrictions: API Key/User 的访问限制
        provider_to_formats: Provider -> formats 映射（兼容转换过滤用）
        client_format: 客户端格式（用于缓存隔离）

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

    normalized_formats = _normalize_api_formats(api_formats, provider_to_formats)

    # 尝试从缓存获取
    if normalized_formats and use_cache:
        cached = await _get_cached_models(normalized_formats, client_format)
        if cached is not None:
            return cached

    # 如果提供了 api_formats，获取真正可用的模型 ID
    available_model_ids: set[str] | None = None
    if normalized_formats:
        available_model_ids = _get_available_model_ids_for_format(
            db, normalized_formats, provider_to_formats
        )
        if not available_model_ids:
            return []

    all_models = (
        ModelAvailabilityQuery.base_active_models(db, eager_load=True)
        .filter(Model.provider_id.in_(available_provider_ids))
        .order_by(Model.created_at.desc())
        .all()
    )

    result: list[ModelInfo] = []
    seen_model_ids: set[str] = set()

    for model in all_models:
        info = _extract_model_info(model)
        if info is None:
            continue

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
    if normalized_formats and use_cache:
        await _set_cached_models(normalized_formats, result, client_format)

    return result


def find_model_by_id(
    db: Session,
    model_id: str,
    available_provider_ids: set[str],
    api_formats: list[str] | None = None,
    restrictions: AccessRestrictions | None = None,
    provider_to_formats: dict[str, set[str]] | None = None,
) -> ModelInfo | None:
    """
    按 ID 查找模型（仅支持 GlobalModel.name）

    Args:
        db: 数据库会话
        model_id: 模型 ID
        available_provider_ids: 有可用端点的 Provider ID 集合
        api_formats: API 格式列表，用于检查 Key 的 allowed_models
        restrictions: API Key/User 的访问限制
        provider_to_formats: Provider -> formats 映射（兼容转换过滤用）

    Returns:
        ModelInfo 或 None
    """
    if not available_provider_ids:
        return None

    normalized_formats = _normalize_api_formats(api_formats, provider_to_formats)

    # 如果提供了 api_formats，获取真正可用的模型 ID
    available_model_ids: set[str] | None = None
    if normalized_formats:
        available_model_ids = _get_available_model_ids_for_format(
            db, normalized_formats, provider_to_formats
        )
        # 快速检查：如果目标模型不在可用列表中，直接返回 None
        if available_model_ids is not None and model_id not in available_model_ids:
            return None

    # 快速检查：如果 restrictions 明确限制了模型列表且目标模型不在其中，直接返回 None
    if restrictions is not None and restrictions.allowed_models is not None:
        if model_id not in restrictions.allowed_models:
            return None

    models_by_global = (
        ModelAvailabilityQuery.find_by_global_model_name(db, model_id, eager_load=True)
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

    model = next((m for m in models_by_global if is_model_accessible(m)), None)

    if not model:
        return None

    return _extract_model_info(model)
