"""
模型权限工具

支持两种 allowed_models 格式：
1. 简单模式（列表）: ["claude-sonnet-4", "gpt-4o"]
2. 按格式模式（字典）: {"OPENAI": ["gpt-4o"], "CLAUDE": ["claude-sonnet-4"]}

使用 None/null 表示不限制（允许所有模型）

支持模型别名匹配：
- GlobalModel.config.model_aliases 定义别名模式
- 别名模式支持正则表达式语法
- 例如：claude-haiku-.* 可匹配 claude-haiku-4.5, claude-haiku-last
- 使用 regex 库的原生超时保护（100ms）防止 ReDoS
"""

import re
from functools import lru_cache
from typing import Dict, List, Optional, Set, Tuple, Union

import regex

from src.core.logger import logger

# 别名规则限制
MAX_ALIASES_PER_MODEL = 50
MAX_ALIAS_LENGTH = 200
MAX_MODEL_NAME_LENGTH = 200  # 与 MAX_ALIAS_LENGTH 保持一致
REGEX_MATCH_TIMEOUT_MS = 100  # 正则匹配超时（毫秒）

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


def parse_allowed_models_to_list(allowed_models: AllowedModels) -> List[str]:
    """
    解析 allowed_models（支持 list 和 dict 格式）为统一的列表

    与 convert_to_simple_mode 的区别：
    - 本函数返回空列表而非 None（用于 UI 展示）
    - convert_to_simple_mode 返回 None 表示不限制

    Args:
        allowed_models: 允许的模型配置（列表或字典）

    Returns:
        模型名称列表（可能为空）
    """
    if allowed_models is None:
        return []

    if isinstance(allowed_models, list):
        return allowed_models

    if isinstance(allowed_models, dict):
        all_models: Set[str] = set()
        for models in allowed_models.values():
            if isinstance(models, list):
                all_models.update(models)
        return sorted(all_models)

    return []


def validate_alias_pattern(pattern: str) -> Tuple[bool, Optional[str]]:
    """
    验证别名模式是否安全

    Args:
        pattern: 待验证的正则模式

    Returns:
        (is_valid, error_message)
    """
    if not pattern or not pattern.strip():
        return False, "别名规则不能为空"

    if len(pattern) > MAX_ALIAS_LENGTH:
        return False, f"别名规则过长 (最大 {MAX_ALIAS_LENGTH} 字符)"

    # 尝试编译验证语法
    try:
        re.compile(f"^{pattern}$", re.IGNORECASE)
    except re.error as e:
        return False, f"正则表达式语法错误: {e}"

    return True, None


def validate_model_aliases(aliases: Optional[List[str]]) -> Tuple[bool, Optional[str]]:
    """
    验证别名列表是否合法

    Args:
        aliases: 别名列表

    Returns:
        (is_valid, error_message)
    """
    if not aliases:
        return True, None

    if len(aliases) > MAX_ALIASES_PER_MODEL:
        return False, f"别名规则数量超限 (最大 {MAX_ALIASES_PER_MODEL} 条)"

    for i, alias in enumerate(aliases):
        is_valid, error = validate_alias_pattern(alias)
        if not is_valid:
            return False, f"第 {i + 1} 条规则无效: {error}"

    return True, None


def validate_and_extract_model_aliases(
    config: Optional[dict],
) -> Tuple[bool, Optional[str], Optional[List[str]]]:
    """
    从 config 中验证并提取 model_aliases

    用于 GlobalModel 创建/更新时的统一验证

    Args:
        config: GlobalModel 的 config 字典

    Returns:
        (is_valid, error_message, aliases):
        - is_valid: 验证是否通过
        - error_message: 错误信息（验证失败时）
        - aliases: 提取的别名列表（验证成功时）
    """
    if not config or "model_aliases" not in config:
        return True, None, None

    aliases = config.get("model_aliases")

    # 允许显式设置为 None（表示清除别名）
    if aliases is None:
        return True, None, None

    # 类型验证：必须是列表
    if not isinstance(aliases, list):
        return False, "model_aliases 必须是数组类型", None

    # 元素类型验证：必须是字符串
    if not all(isinstance(a, str) for a in aliases):
        return False, "model_aliases 数组元素必须是字符串", None

    # 业务规则验证
    is_valid, error = validate_model_aliases(aliases)
    if not is_valid:
        return False, error, None

    return True, None, aliases


@lru_cache(maxsize=2000)
def _compile_pattern_cached(pattern: str) -> Optional[regex.Pattern]:
    """
    编译正则模式（带 LRU 缓存）

    Args:
        pattern: 正则模式字符串

    Returns:
        编译后的正则对象，如果无效则返回 None
    """
    try:
        return regex.compile(f"^{pattern}$", regex.IGNORECASE)
    except regex.error as e:
        logger.debug(f"正则编译失败: pattern={pattern}, error={e}")
        return None


def clear_regex_cache() -> None:
    """
    清空正则缓存

    在 GlobalModel 别名更新时调用此函数以确保缓存一致性
    """
    _compile_pattern_cached.cache_clear()
    logger.debug("[RegexCache] 缓存已清空")


def _match_with_timeout(
    compiled_regex: regex.Pattern, text: str, timeout_ms: int = REGEX_MATCH_TIMEOUT_MS
) -> Optional[bool]:
    """
    带超时的正则匹配（使用 regex 库的原生超时支持）

    相比 ThreadPoolExecutor 方案的优势：
    - C 层面中断匹配，不会留下僵尸线程
    - 更低的性能开销
    - 更精确的超时控制

    Args:
        compiled_regex: 编译后的 regex.Pattern 对象
        text: 待匹配的文本
        timeout_ms: 超时时间（毫秒）

    Returns:
        True: 匹配成功
        False: 匹配失败
        None: 超时或异常
    """
    try:
        # regex 库的 timeout 参数单位是秒
        result = compiled_regex.match(text, timeout=timeout_ms / 1000.0)
        return result is not None
    except TimeoutError:
        logger.warning(
            f"正则匹配超时 ({timeout_ms}ms): pattern={compiled_regex.pattern[:50]}..., text={text[:50]}..."
        )
        return None
    except Exception as e:
        logger.warning(f"正则匹配异常: {e}")
        return None


def match_model_with_pattern(pattern: str, model_name: str) -> bool:
    """
    检查模型名是否匹配别名模式（支持正则表达式）

    安全特性：
    - 长度限制检查
    - 正则编译缓存
    - 正则匹配超时保护（100ms，使用 regex 库原生超时）

    Args:
        pattern: 别名模式，支持正则表达式语法
        model_name: 被检查的模型名（来自 Key 的 allowed_models）

    Returns:
        True 如果匹配

    示例:
        match_model_with_pattern("claude-haiku-.*", "claude-haiku-4.5") -> True
        match_model_with_pattern("gpt-4o", "gpt-4o") -> True
        match_model_with_pattern("gpt-4o", "gpt-4") -> False
    """
    # 快速路径：精确匹配
    if pattern.lower() == model_name.lower():
        return True

    # 长度检查
    if len(pattern) > MAX_ALIAS_LENGTH or len(model_name) > MAX_MODEL_NAME_LENGTH:
        return False

    # 使用缓存的编译结果
    compiled = _compile_pattern_cached(pattern)
    if compiled is None:
        return False

    # 使用带超时的匹配（regex 库原生支持）
    result = _match_with_timeout(compiled, model_name)
    return result is True


def check_model_allowed_with_aliases(
    model_name: str,
    allowed_models: AllowedModels,
    api_format: Optional[str] = None,
    resolved_model_name: Optional[str] = None,
    model_aliases: Optional[List[str]] = None,
) -> tuple[bool, Optional[str]]:
    """
    检查模型是否被允许（支持别名通配符匹配）

    匹配优先级：
    1. 精确匹配 model_name（用户请求的模型名）
    2. 精确匹配 resolved_model_name（GlobalModel.name）
    3. 遍历 model_aliases，检查每个别名是否匹配 allowed_models 中的任一项

    别名匹配顺序说明：
    - 按 allowed_models 集合的迭代顺序遍历（通常为字母顺序，因为内部使用 set）
    - 对于每个 allowed_model，按 model_aliases 数组顺序依次尝试匹配
    - 返回第一个成功匹配的 allowed_model
    - 如需确定性行为，请确保 model_aliases 中的规则从最具体到最通用排序

    Args:
        model_name: 请求的模型名称
        allowed_models: 允许的模型配置（来自 Provider Key）
        api_format: 当前请求的 API 格式
        resolved_model_name: 解析后的 GlobalModel.name
        model_aliases: GlobalModel 的别名列表（来自 config.model_aliases）

    Returns:
        (is_allowed, matched_model_name):
        - is_allowed: 是否允许使用该模型
        - matched_model_name: 通过别名匹配到的模型名（仅别名匹配时有值，精确匹配时为 None）
    """
    # 先尝试精确匹配（使用原有逻辑）
    if check_model_allowed(model_name, allowed_models, api_format, resolved_model_name):
        return True, None

    # 如果精确匹配失败且有别名配置，尝试别名匹配
    if not model_aliases:
        return False, None

    # 获取 allowed_models 的集合
    allowed_set = normalize_allowed_models(allowed_models, api_format)
    if allowed_set is None:
        # 不限制，已在 check_model_allowed 中返回 True
        return True, None

    if len(allowed_set) == 0:
        # 空集合 = 拒绝所有
        return False, None

    # 遍历 allowed_models 中的每个模型名，检查是否有别名能匹配
    # 注意：返回第一个匹配的模型名，匹配顺序由 allowed_set 迭代顺序和 model_aliases 数组顺序决定
    for allowed_model in allowed_set:
        for alias_pattern in model_aliases:
            if match_model_with_pattern(alias_pattern, allowed_model):
                # 返回匹配到的模型名，用于实际请求
                return True, allowed_model

    return False, None
