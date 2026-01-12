"""
模型权限工具

支持两种 allowed_models 格式：
1. 简单模式（列表）: ["claude-sonnet-4", "gpt-4o"]
2. 按格式模式（字典）: {"OPENAI": ["gpt-4o"], "CLAUDE": ["claude-sonnet-4"]}

使用 None/null 表示不限制（允许所有模型）
"""

from typing import Any, Dict, List, Optional, Set, Union

# 类型别名
AllowedModels = Optional[Union[List[str], Dict[str, List[str]]]]


def normalize_allowed_models(
    allowed_models: AllowedModels,
    api_format: Optional[str] = None,
) -> Optional[Set[str]]:
    """
    将 allowed_models 规范化为模型名称集合

    Args:
        allowed_models: 允许的模型配置（列表或字典）
        api_format: 当前请求的 API 格式（用于字典模式）

    Returns:
        - None: 不限制（允许所有模型）
        - Set[str]: 允许的模型名称集合（可能为空集，表示拒绝所有）
    """
    if allowed_models is None:
        return None

    # 简单模式：直接是列表
    if isinstance(allowed_models, list):
        return set(allowed_models)

    # 按格式模式：字典
    if isinstance(allowed_models, dict):
        if api_format is None:
            # 没有指定格式，合并所有格式的模型
            all_models: Set[str] = set()
            for models in allowed_models.values():
                if isinstance(models, list):
                    all_models.update(models)
            return all_models if all_models else None

        # 查找指定格式的模型列表
        api_format_upper = api_format.upper()
        models = allowed_models.get(api_format_upper)
        if models is None:
            # 该格式未配置，检查是否有通配符 "*"
            models = allowed_models.get("*")

        if models is None:
            # 字典模式下未配置的格式 = 不限制该格式
            return None

        return set(models) if isinstance(models, list) else None

    # 未知类型，视为不限制
    return None


def check_model_allowed(
    model_name: str,
    allowed_models: AllowedModels,
    api_format: Optional[str] = None,
    resolved_model_name: Optional[str] = None,
) -> bool:
    """
    检查模型是否被允许

    Args:
        model_name: 请求的模型名称
        allowed_models: 允许的模型配置
        api_format: 当前请求的 API 格式
        resolved_model_name: 解析后的 GlobalModel.name（可选）

    Returns:
        True: 允许使用该模型
        False: 不允许使用该模型
    """
    allowed_set = normalize_allowed_models(allowed_models, api_format)

    if allowed_set is None:
        # 不限制
        return True

    if len(allowed_set) == 0:
        # 空集合 = 拒绝所有
        return False

    # 检查请求的模型名或解析后的名称是否在白名单中
    if model_name in allowed_set:
        return True

    if resolved_model_name and resolved_model_name in allowed_set:
        return True

    return False


def merge_allowed_models(
    allowed_models_1: AllowedModels,
    allowed_models_2: AllowedModels,
) -> AllowedModels:
    """
    合并两个 allowed_models 配置，取交集

    规则：
    - 如果任一为 None，返回另一个
    - 如果都有值，取交集
    - 如果都是列表，取列表交集
    - 如果有字典，按 API 格式分别取交集（保持字典语义，不丢失格式区分信息）

    Args:
        allowed_models_1: 第一个配置
        allowed_models_2: 第二个配置

    Returns:
        合并后的配置
    """
    if allowed_models_1 is None:
        return allowed_models_2
    if allowed_models_2 is None:
        return allowed_models_1

    # 两个都是简单列表：直接取交集（返回确定性顺序）
    if isinstance(allowed_models_1, list) and isinstance(allowed_models_2, list):
        intersection = set(allowed_models_1) & set(allowed_models_2)
        return sorted(intersection) if intersection else []

    # 任一为字典模式：按 API 格式分别取交集，避免把 dict 合并成 list 导致权限过宽
    from src.core.enums import APIFormat

    def merge_sets(a: Optional[Set[str]], b: Optional[Set[str]]) -> Optional[Set[str]]:
        # None 表示不限制：交集规则下等价于“只受另一方限制”
        if a is None:
            return b
        if b is None:
            return a
        return a & b

    known_formats = [fmt.value for fmt in APIFormat]

    per_format: Dict[str, Optional[Set[str]]] = {}
    for fmt in known_formats:
        s1 = normalize_allowed_models(allowed_models_1, api_format=fmt)
        s2 = normalize_allowed_models(allowed_models_2, api_format=fmt)
        per_format[fmt] = merge_sets(s1, s2)

    # 计算默认（未知格式）的交集，用 "*" 作为默认值以覆盖未枚举的格式
    default_s1 = normalize_allowed_models(allowed_models_1, api_format="__DEFAULT__")
    default_s2 = normalize_allowed_models(allowed_models_2, api_format="__DEFAULT__")
    default_set = merge_sets(default_s1, default_s2)

    # 如果 default_set 非 None 且不存在“某些格式不限制”的情况，可用 "*" 作为默认规则并按需覆盖
    can_use_wildcard = default_set is not None and all(v is not None for v in per_format.values())

    merged_dict: Dict[str, List[str]] = {}

    if can_use_wildcard and default_set is not None:
        merged_dict["*"] = sorted(default_set)
        for fmt, s in per_format.items():
            # can_use_wildcard 保证 s 非 None
            if s is not None and s != default_set:
                merged_dict[fmt] = sorted(s)
    else:
        for fmt, s in per_format.items():
            if s is None:
                continue
            merged_dict[fmt] = sorted(s)

    if not merged_dict:
        # 全部不限制
        return None

    return merged_dict


def get_allowed_models_preview(
    allowed_models: AllowedModels,
    max_items: int = 3,
) -> str:
    """
    获取 allowed_models 的预览字符串（用于日志和错误消息）

    Args:
        allowed_models: 允许的模型配置
        max_items: 最多显示的模型数

    Returns:
        预览字符串，如 "gpt-4o, claude-sonnet-4, ..."
    """
    if allowed_models is None:
        return "(不限制)"

    all_models: Set[str] = set()

    if isinstance(allowed_models, list):
        all_models = set(allowed_models)
    elif isinstance(allowed_models, dict):
        for models in allowed_models.values():
            if isinstance(models, list):
                all_models.update(models)

    if not all_models:
        return "(无)"

    sorted_models = sorted(all_models)
    preview = ", ".join(sorted_models[:max_items])
    if len(sorted_models) > max_items:
        preview += f", ...共{len(sorted_models)}个"

    return preview


def is_format_mode(allowed_models: AllowedModels) -> bool:
    """
    判断 allowed_models 是否为按格式模式

    Args:
        allowed_models: 允许的模型配置

    Returns:
        True: 按格式模式（字典）
        False: 简单模式（列表或 None）
    """
    return isinstance(allowed_models, dict)


def convert_to_format_mode(
    allowed_models: AllowedModels,
    api_formats: Optional[List[str]] = None,
) -> Dict[str, List[str]]:
    """
    将 allowed_models 转换为按格式模式

    Args:
        allowed_models: 原始配置
        api_formats: 要应用的 API 格式列表

    Returns:
        按格式模式的配置
    """
    if allowed_models is None:
        return {}

    if isinstance(allowed_models, dict):
        return allowed_models

    # 简单列表模式 -> 按格式模式
    if isinstance(allowed_models, list):
        if not api_formats:
            return {"*": allowed_models}
        return {fmt.upper(): list(allowed_models) for fmt in api_formats}

    return {}


def convert_to_simple_mode(allowed_models: AllowedModels) -> Optional[List[str]]:
    """
    将 allowed_models 转换为简单列表模式

    Args:
        allowed_models: 原始配置

    Returns:
        简单列表或 None
    """
    if allowed_models is None:
        return None

    if isinstance(allowed_models, list):
        return allowed_models

    if isinstance(allowed_models, dict):
        all_models: Set[str] = set()
        for models in allowed_models.values():
            if isinstance(models, list):
                all_models.update(models)
        return sorted(all_models) if all_models else None

    return None
