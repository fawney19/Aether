"""
集中维护 API 格式的元数据，避免新增格式时到处修改常量。

此模块与 src/formats/ 的 FormatProtocol 系统配合使用：
- api_format_metadata: 定义格式的元数据（别名、默认路径）
- src/formats/: 定义格式的协议实现（解析、转换、验证）

使用方式：
    # 解析格式别名
    from src.core.api_format_metadata import resolve_api_format
    api_format = resolve_api_format("claude")  # -> APIFormat.CLAUDE

    # 获取格式协议
    from src.core.api_format_metadata import get_format_protocol
    protocol = get_format_protocol(APIFormat.CLAUDE)  # -> ClaudeProtocol
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from functools import lru_cache
from types import MappingProxyType
from typing import Dict, Iterable, List, Mapping, MutableMapping, Optional, Sequence, Union

from .enums import APIFormat


@dataclass(frozen=True)
class ApiFormatDefinition:
    """
    描述一个 API 格式的所有通用信息。

    - aliases: 用于 detect_api_format 的 provider 别名或快捷名称
    - default_path: 上游默认请求路径（如 /v1/messages），可通过 Endpoint.custom_path 覆盖
    - path_prefix: 本站路径前缀（如 /claude, /openai），为空表示无前缀
    - auth_header: 认证头名称 (如 "x-api-key", "x-goog-api-key")
    - auth_type: 认证类型 ("header" 直接放值, "bearer" 加 Bearer 前缀)
    """

    api_format: APIFormat
    aliases: Sequence[str] = field(default_factory=tuple)
    default_path: str = "/"  # 上游默认请求路径
    path_prefix: str = ""  # 本站路径前缀，为空表示无前缀
    auth_header: str = "Authorization"
    auth_type: str = "bearer"  # "bearer" or "header"

    def iter_aliases(self) -> Iterable[str]:
        """返回大小写统一后的别名集合，包含枚举名本身。"""
        yield normalize_alias_value(self.api_format.value)
        for alias in self.aliases:
            normalized = normalize_alias_value(alias)
            if normalized:
                yield normalized


_DEFINITIONS: Dict[APIFormat, ApiFormatDefinition] = {
    APIFormat.CLAUDE: ApiFormatDefinition(
        api_format=APIFormat.CLAUDE,
        aliases=("claude", "anthropic", "claude_compatible"),
        default_path="/v1/messages",
        path_prefix="",  # 通过请求头区分格式，不使用路径前缀
        auth_header="x-api-key",
        auth_type="header",
    ),
    APIFormat.CLAUDE_CLI: ApiFormatDefinition(
        api_format=APIFormat.CLAUDE_CLI,
        aliases=("claude_cli", "claude-cli"),
        default_path="/v1/messages",
        path_prefix="",  # 与 CLAUDE 共享入口，通过 header 区分
        auth_header="authorization",
        auth_type="bearer",
    ),
    APIFormat.OPENAI: ApiFormatDefinition(
        api_format=APIFormat.OPENAI,
        aliases=(
            "openai",
            "deepseek",
            "grok",
            "moonshot",
            "zhipu",
            "qwen",
            "baichuan",
            "minimax",
            "openai_compatible",
        ),
        default_path="/v1/chat/completions",
        path_prefix="",  # 默认格式
        auth_header="Authorization",
        auth_type="bearer",
    ),
    APIFormat.OPENAI_CLI: ApiFormatDefinition(
        api_format=APIFormat.OPENAI_CLI,
        aliases=("openai_cli", "responses"),
        default_path="/responses",
        path_prefix="",  # 与 OPENAI 共享入口
        auth_header="Authorization",
        auth_type="bearer",
    ),
    APIFormat.GEMINI: ApiFormatDefinition(
        api_format=APIFormat.GEMINI,
        aliases=("gemini", "google", "vertex"),
        default_path="/v1beta/models/{model}:{action}",
        path_prefix="",  # 通过请求头区分格式
        auth_header="x-goog-api-key",
        auth_type="header",
    ),
    APIFormat.GEMINI_CLI: ApiFormatDefinition(
        api_format=APIFormat.GEMINI_CLI,
        aliases=("gemini_cli", "gemini-cli"),
        default_path="/v1beta/models/{model}:{action}",
        path_prefix="",  # 与 GEMINI 共享入口
        auth_header="x-goog-api-key",
        auth_type="header",
    ),
}

# 对外只暴露只读视图，避免被随意修改
API_FORMAT_DEFINITIONS: Mapping[APIFormat, ApiFormatDefinition] = MappingProxyType(_DEFINITIONS)


def get_api_format_definition(api_format: APIFormat) -> ApiFormatDefinition:
    """获取指定格式的定义，不存在时抛出 KeyError。"""
    return API_FORMAT_DEFINITIONS[api_format]


def list_api_format_definitions() -> List[ApiFormatDefinition]:
    """返回所有定义的浅拷贝列表，供遍历使用。"""
    return list(API_FORMAT_DEFINITIONS.values())


def build_alias_lookup() -> Dict[str, APIFormat]:
    """
    构建 alias -> APIFormat 的查找表。
    每次调用都会返回新的 dict，避免可变全局引发并发问题。
    """
    lookup: MutableMapping[str, APIFormat] = {}
    for definition in API_FORMAT_DEFINITIONS.values():
        for alias in definition.iter_aliases():
            lookup.setdefault(alias, definition.api_format)
    return dict(lookup)


def get_default_path(api_format: APIFormat) -> str:
    """
    获取该格式的上游默认请求路径。

    可通过 Endpoint.custom_path 覆盖。
    """
    definition = API_FORMAT_DEFINITIONS.get(api_format)
    return definition.default_path if definition else "/"


def get_local_path(api_format: APIFormat) -> str:
    """
    获取该格式的本站入口路径。

    本站入口路径 = path_prefix + default_path
    例如：path_prefix="/openai" + default_path="/v1/chat/completions" -> "/openai/v1/chat/completions"
    """
    definition = API_FORMAT_DEFINITIONS.get(api_format)
    if definition:
        prefix = definition.path_prefix or ""
        return prefix + definition.default_path
    return "/"


def get_auth_config(api_format: APIFormat) -> tuple[str, str]:
    """
    获取该格式的认证配置。

    Returns:
        (auth_header, auth_type) 元组
        - auth_header: 认证头名称
        - auth_type: "bearer" 或 "header"
    """
    definition = API_FORMAT_DEFINITIONS.get(api_format)
    if definition:
        return definition.auth_header, definition.auth_type
    return "Authorization", "bearer"


@lru_cache(maxsize=1)
def _alias_lookup_cache() -> Dict[str, APIFormat]:
    """缓存 alias -> APIFormat 查找表，减少重复构建。"""
    return build_alias_lookup()


def resolve_api_format_alias(value: str) -> Optional[APIFormat]:
    """根据别名查找 APIFormat，找不到时返回 None。"""
    if not value:
        return None
    normalized = normalize_alias_value(value)
    if not normalized:
        return None
    return _alias_lookup_cache().get(normalized)


def resolve_api_format(
    value: Union[str, APIFormat, None],
    default: Optional[APIFormat] = None,
) -> Optional[APIFormat]:
    """
    将任意字符串/枚举值解析为 APIFormat。

    Args:
        value: 可以是 APIFormat 或任意字符串/别名
        default: 未解析成功时返回的默认值
    """
    if isinstance(value, APIFormat):
        return value
    if isinstance(value, str):
        stripped = value.strip()
        if not stripped:
            return default
        upper = stripped.upper()
        if upper in APIFormat.__members__:
            return APIFormat[upper]
        alias = resolve_api_format_alias(stripped)
        if alias:
            return alias
    return default


def register_api_format_definition(definition: ApiFormatDefinition, *, override: bool = False):
    """
    注册或覆盖 API 格式定义，允许运行时扩展。

    Args:
        definition: 要注册的定义
        override: 若目标枚举已存在，是否允许覆盖
    """
    existing = _DEFINITIONS.get(definition.api_format)
    if existing and not override:
        raise ValueError(f"{definition.api_format.value} 已存在，如需覆盖请设置 override=True")
    _DEFINITIONS[definition.api_format] = definition
    _refresh_metadata_cache()


def _refresh_metadata_cache():
    """更新别名缓存，供注册函数调用。"""
    _alias_lookup_cache.cache_clear()


def normalize_alias_value(value: str) -> str:
    """统一别名格式：去空白、转小写，并将非字母数字转为单个下划线。"""
    if value is None:
        return ""
    text = value.strip().lower()
    # 将所有非字母数字字符替换为下划线，并折叠连续的下划线
    text = re.sub(r"[^a-z0-9]+", "_", text)
    return text.strip("_")


# =============================================================================
# 格式判断工具
# =============================================================================


def is_cli_api_format(api_format: APIFormat) -> bool:
    """
    判断是否为 CLI 透传格式。

    Args:
        api_format: APIFormat 枚举值

    Returns:
        True 如果是 CLI 格式
    """
    from src.api.handlers.base.parsers import is_cli_format

    return is_cli_format(api_format.value)
