"""
请求构建器 - 透传模式

透传模式 (Passthrough): CLI 和 Chat 等场景，原样转发请求体和头部
- 清理敏感头部：authorization, x-api-key, host, content-length 等
- 保留所有其他头部和请求体字段
- 适用于：Claude CLI、OpenAI CLI、Chat API 等场景

使用方式：
    builder = PassthroughRequestBuilder()
    payload, headers = builder.build(original_body, original_headers, endpoint, key)
"""

from __future__ import annotations

import copy
import re
from abc import ABC, abstractmethod
from dataclasses import dataclass
from typing import Any

from src.core.api_format import (
    UPSTREAM_DROP_HEADERS,
    HeaderBuilder,
    get_auth_config_for_endpoint,
    make_signature_key,
    resolve_header_name_case,
)
from src.core.crypto import crypto_service
from src.core.logger import logger
from src.models.endpoint_models import _CONDITION_OPS, _TYPE_IS_VALUES, parse_re_flags
from src.services.provider.auth import get_provider_auth  # noqa: F401
from src.services.provider.envelope import ProviderEnvelope


def _payload_item_count(value: Any) -> int | None:
    """统计顶层 prompt-bearing 容器项数量；标量按 1 处理。"""
    if isinstance(value, list):
        return len(value)
    if value is None:
        return None
    if isinstance(value, (str, bytes)):
        return 1 if value else 0
    if isinstance(value, dict):
        return 1 if value else 0
    return 1


def summarize_request_payload_shape(
    payload: dict[str, Any],
    *,
    provider_api_format: str | None,
    protected_body_keys: frozenset[str] | None,
    body_rules: Any,
) -> dict[str, Any]:
    """生成最终出站 payload 的结构化摘要，避免记录正文。"""
    tools = payload.get("tools")
    tool_count = len(tools) if isinstance(tools, list) else None

    function_declaration_count = 0
    if isinstance(tools, list):
        for tool in tools:
            if not isinstance(tool, dict):
                continue
            decls = tool.get("function_declarations") or tool.get("functionDeclarations")
            if isinstance(decls, list):
                function_declaration_count += len(decls)

    return {
        "format": str(provider_api_format or "").strip().lower() or None,
        "top_level_keys": sorted(payload.keys()),
        "message_count": _payload_item_count(payload.get("messages")),
        "input_count": _payload_item_count(payload.get("input")),
        "contents_count": _payload_item_count(payload.get("contents")),
        "tool_count": tool_count,
        "function_declaration_count": function_declaration_count,
        "has_system": any(
            k in payload for k in ("system", "system_instruction", "systemInstruction")
        ),
        "has_instructions": "instructions" in payload,
        "has_tool_choice": any(
            k in payload for k in ("tool_choice", "toolChoice", "tool_config", "toolConfig")
        ),
        "has_generation_config": any(
            k in payload for k in ("generation_config", "generationConfig")
        ),
        "has_prompt_cache_key": bool(str(payload.get("prompt_cache_key") or "").strip()),
        "protected_body_keys_enabled": bool(protected_body_keys),
        "protected_body_keys": sorted(protected_body_keys or ()),
        "body_rule_count": len(body_rules) if isinstance(body_rules, list) else 0,
    }


# ==============================================================================
# 统一的头部配置常量
# ==============================================================================

# 兼容别名：历史代码使用 SENSITIVE_HEADERS 命名
SENSITIVE_HEADERS: frozenset[str] = UPSTREAM_DROP_HEADERS

# 请求体中受保护的字段（不能被 body_rules 修改）
PROTECTED_BODY_FIELDS: frozenset[str] = frozenset(
    {
        "model",  # 模型名由系统管理
        "stream",  # 流式标志由系统管理
    }
)

# cache-sensitive 顶层字段：用于阻止 endpoint body_rules 在最终出站前
# 二次改写 prompt-bearing 结构，破坏上游 prompt cache 一致性。
_CACHE_SENSITIVE_BODY_FIELDS_BY_FORMAT: dict[str, frozenset[str]] = {
    "openai:chat": frozenset({"messages", "tools", "tool_choice"}),
    "openai:cli": frozenset({"input", "instructions", "tools", "tool_choice", "prompt_cache_key"}),
    "openai:compact": frozenset(
        {"input", "instructions", "tools", "tool_choice", "prompt_cache_key"}
    ),
    "claude:chat": frozenset({"messages", "system", "tools", "tool_choice"}),
    "gemini:chat": frozenset(
        {
            "contents",
            "system_instruction",
            "systemInstruction",
            "tools",
            "tool_config",
            "toolConfig",
            "generation_config",
            "generationConfig",
        }
    ),
}


def get_cache_sensitive_protected_body_keys(provider_api_format: str | None) -> frozenset[str]:
    """根据目标 Provider API 格式返回需要保护的顶层请求字段。"""
    fmt = str(provider_api_format or "").strip().lower()
    extra = _CACHE_SENSITIVE_BODY_FIELDS_BY_FORMAT.get(fmt, frozenset())
    if not extra:
        return PROTECTED_BODY_FIELDS
    return frozenset({*PROTECTED_BODY_FIELDS, *extra})


# ==============================================================================
# 测试请求常量与辅助函数
# ==============================================================================

# 标准测试请求体（OpenAI 格式）
# 用于 check_endpoint 等测试场景，使用简单安全的消息内容避免触发安全过滤
DEFAULT_TEST_REQUEST: dict[str, Any] = {
    "messages": [{"role": "user", "content": "Hi"}],
    "max_tokens": 5,
    "temperature": 0,
}


def get_test_request_data(request_data: dict[str, Any] | None = None) -> dict[str, Any]:
    """获取测试请求数据

    如果传入 request_data，则合并到默认测试请求中；
    否则使用默认测试请求。

    Args:
        request_data: 用户提供的请求数据（会覆盖默认值）

    Returns:
        合并后的测试请求数据（OpenAI 格式）
    """
    if request_data:
        merged = DEFAULT_TEST_REQUEST.copy()
        merged.update(request_data)
        return merged
    return DEFAULT_TEST_REQUEST.copy()


def build_test_request_body(
    format_id: str,
    request_data: dict[str, Any] | None = None,
    *,
    target_variant: str | None = None,
) -> dict[str, Any]:
    """构建测试请求体，自动处理格式转换

    使用格式转换注册表将 OpenAI 格式的测试请求转换为目标格式。

    Args:
        format_id: 目标 endpoint signature（如 "claude:chat", "gemini:chat", "openai:cli"）
        request_data: 可选的请求数据，会与默认测试请求合并
        target_variant: 目标变体（如 "codex"），用于同格式但有细微差异的上游

    Returns:
        转换为目标 API 格式的请求体
    """
    from src.core.api_format.conversion import (
        format_conversion_registry,
        register_default_normalizers,
    )

    register_default_normalizers()

    # 获取测试请求数据（OpenAI 格式）
    source_data = get_test_request_data(request_data)

    # 直接使用目标格式进行转换，不再转换为基础格式
    # 这样 openai:cli 会正确转换为 Responses API 格式
    return format_conversion_registry.convert_request(
        source_data,
        make_signature_key("openai", "chat"),
        format_id,
        target_variant=target_variant,
    )


# ==============================================================================
# 请求体规则应用
# ==============================================================================


@dataclass(frozen=True, slots=True)
class _WildcardSlice:
    """表示数组通配符路径段: [*] 或 [start-end]"""

    start: int | None  # None 表示 [*]
    end: int | None

    def resolve(self, length: int) -> range:
        """根据实际数组长度返回索引 range"""
        if self.start is None:
            return range(length)
        s = max(0, self.start)
        e = min(length - 1, self.end if self.end is not None else length - 1)
        return range(s, e + 1) if s <= e else range(0)


# 路径段类型：str 表示 dict key，int 表示数组索引，_WildcardSlice 表示通配
PathSegment = str | int | _WildcardSlice

_RANGE_RE = re.compile(r"^(\d+)\s*-\s*(\d+)$")


def _parse_path(path: str) -> list[PathSegment]:
    """
    解析路径，支持点号分隔、转义、数组索引、通配符和范围。

    Examples:
        "metadata.user.name"       -> ["metadata", "user", "name"]
        "config\\.v1.enabled"      -> ["config.v1", "enabled"]
        "messages[0].content"      -> ["messages", 0, "content"]
        "data[0].items[2].name"    -> ["data", 0, "items", 2, "name"]
        "messages[-1]"             -> ["messages", -1]
        "matrix[0][1]"             -> ["matrix", 0, 1]
        "tools[*].name"            -> ["tools", _WildcardSlice(None, None), "name"]
        "tools[0-4].name"          -> ["tools", _WildcardSlice(0, 4), "name"]

    约束：
        - 不允许空段（例如：".a" / "a." / "a..b"），遇到则返回空列表表示无效路径。
        - 仅对 "\\." 做特殊处理；其他反斜杠组合按字面量保留。
        - 数组索引必须是整数（支持负数索引）。
        - [*] 表示遍历数组所有元素。
        - [N-M] 表示遍历数组索引 N 到 M（含两端）。
    """
    raw = (path or "").strip()
    if not raw:
        return []

    parts: list[PathSegment] = []
    current: list[str] = []
    expect_key = True  # 是否期望下一个片段是 dict key

    i = 0
    while i < len(raw):
        ch = raw[i]

        # 转义点号：\\.
        if ch == "\\" and i + 1 < len(raw) and raw[i + 1] == ".":
            current.append(".")
            expect_key = False
            i += 2
            continue

        # 点号分隔符
        if ch == ".":
            if current:
                parts.append("".join(current))
                current = []
            elif expect_key:
                # 空段（如 ".a" 或 "a..b"）
                return []
            expect_key = True
            i += 1
            continue

        # 数组索引：[N] / [*] / [N-M]
        if ch == "[":
            # 先将当前累积的 key 入栈
            if current:
                parts.append("".join(current))
                current = []

            # 查找闭合括号
            j = i + 1
            while j < len(raw) and raw[j] != "]":
                j += 1
            if j >= len(raw):
                return []  # 未闭合的括号

            index_str = raw[i + 1 : j].strip()
            if not index_str:
                return []  # 空索引

            # [*] 通配符
            if index_str == "*":
                parts.append(_WildcardSlice(None, None))
            else:
                # [N-M] 范围
                m = _RANGE_RE.match(index_str)
                if m:
                    parts.append(_WildcardSlice(int(m.group(1)), int(m.group(2))))
                else:
                    # 普通整数索引
                    try:
                        idx = int(index_str)
                    except ValueError:
                        return []  # 非整数索引
                    parts.append(idx)

            expect_key = False
            i = j + 1
            continue

        current.append(ch)
        expect_key = False
        i += 1

    # 收尾：将剩余的 key 入栈
    if current:
        parts.append("".join(current))
    elif expect_key:
        # 尾部悬挂的点号（如 "a."）
        return []

    return parts if parts else []


def _has_wildcard(parts: list[PathSegment]) -> bool:
    """检查路径段列表中是否包含通配符"""
    return any(isinstance(p, _WildcardSlice) for p in parts)


def _expand_wildcard_paths(
    obj: Any, parts: list[PathSegment], *, require_leaf: bool = False
) -> list[list[str | int]]:
    """
    将含通配符的路径段展开为具体的路径段列表。

    遍历 obj 结构，遇到 _WildcardSlice 时根据实际数组长度展开为具体索引。
    返回的每条路径都是纯 str|int 段，不含通配符。

    Args:
        obj: 要遍历的数据结构
        parts: 含通配符的路径段列表
        require_leaf: 是否要求叶子节点存在（False 时只要父级存在即可，适用于 set）
    """
    result: list[list[str | int]] = []

    def _recurse(current: Any, idx: int, prefix: list[str | int]) -> None:
        if idx == len(parts):
            result.append(prefix[:])
            return

        seg = parts[idx]
        is_last = idx == len(parts) - 1

        if isinstance(seg, _WildcardSlice):
            if not isinstance(current, list):
                return
            for i in seg.resolve(len(current)):
                prefix.append(i)
                try:
                    _recurse(current[i], idx + 1, prefix)
                except IndexError:
                    pass
                prefix.pop()
        elif isinstance(seg, int):
            if isinstance(current, list):
                try:
                    prefix.append(seg)
                    _recurse(current[seg], idx + 1, prefix)
                    prefix.pop()
                except IndexError:
                    prefix.pop()
        else:
            # str key
            if isinstance(current, dict):
                if seg in current:
                    prefix.append(seg)
                    _recurse(current[seg], idx + 1, prefix)
                    prefix.pop()
                elif is_last and not require_leaf:
                    # 叶子节点不存在但允许创建（set 场景）
                    prefix.append(seg)
                    result.append(prefix[:])
                    prefix.pop()

    _recurse(obj, 0, [])
    return result


def _segments_to_path(segments: list[str | int]) -> str:
    """将路径段列表转回路径字符串（用于调用现有的 _set/_get/_delete 函数）"""
    parts: list[str] = []
    for seg in segments:
        if isinstance(seg, int):
            parts.append(f"[{seg}]")
        else:
            # 转义字面量点号
            escaped = seg.replace(".", "\\.")
            if parts and not parts[-1].endswith("]"):
                parts.append(f".{escaped}")
            else:
                parts.append(escaped)
    return "".join(parts)


def _get_nested_value(obj: Any, path: str) -> tuple[bool, Any]:
    """
    获取嵌套值，支持 dict 和 list 混合遍历

    Returns:
        (found, value) - found 为 True 时 value 有效
    """
    parts = _parse_path(path)
    if not parts:
        return False, None

    current: Any = obj
    for segment in parts:
        if isinstance(segment, int):
            if isinstance(current, list):
                try:
                    current = current[segment]
                except IndexError:
                    return False, None
            else:
                return False, None
        else:
            if isinstance(current, dict) and segment in current:
                current = current[segment]
            else:
                return False, None
    return True, current


def _set_nested_value(obj: dict[str, Any], path: str, value: Any) -> bool:
    """
    设置嵌套值，支持 dict 和 list 混合遍历。

    - dict 中间层：下一段为 str key 时自动创建（覆写语义）；下一段为 int 时要求已存在 list。
    - list 中间层：必须已存在且索引有效。
    - list 元素赋值：要求索引在范围内。

    Returns:
        True: 写入成功
        False: 路径无效或结构不匹配
    """
    parts = _parse_path(path)
    if not parts:
        return False

    current: Any = obj
    for i in range(len(parts) - 1):
        segment = parts[i]
        next_segment = parts[i + 1]

        if isinstance(segment, int):
            # 遍历数组元素
            if not isinstance(current, list):
                return False
            try:
                current = current[segment]
            except IndexError:
                return False
        else:
            # 遍历 dict key
            if not isinstance(current, dict):
                return False
            child = current.get(segment)

            if isinstance(next_segment, int):
                # 下一段是数组索引 → child 必须已经是 list
                if not isinstance(child, list):
                    return False
                current = child
            else:
                # 下一段是 dict key → 自动创建 dict（覆写语义）
                if not isinstance(child, dict):
                    child = {}
                    current[segment] = child
                current = child

    # 写入最终值
    last = parts[-1]
    if isinstance(last, int):
        if not isinstance(current, list):
            return False
        try:
            current[last] = value
            return True
        except IndexError:
            return False
    else:
        if not isinstance(current, dict):
            return False
        current[last] = value
        return True


def _delete_nested_value(obj: dict[str, Any], path: str) -> bool:
    """
    删除嵌套值，支持 dict 和 list 混合遍历

    对于 list 元素，使用 del 删除（会移动后续元素的索引）。

    Returns:
        True: 删除成功
        False: 路径不存在或无效
    """
    parts = _parse_path(path)
    if not parts:
        return False

    current: Any = obj
    for segment in parts[:-1]:
        if isinstance(segment, int):
            if isinstance(current, list):
                try:
                    current = current[segment]
                except IndexError:
                    return False
            else:
                return False
        else:
            if isinstance(current, dict) and segment in current:
                current = current[segment]
            else:
                return False

    last = parts[-1]
    if isinstance(last, int):
        if isinstance(current, list):
            try:
                del current[last]
                return True
            except IndexError:
                return False
        return False
    else:
        if isinstance(current, dict) and last in current:
            del current[last]
            return True
        return False


def _rename_nested_value(obj: dict[str, Any], from_path: str, to_path: str) -> bool:
    """
    重命名嵌套值（移动到新路径），支持 dict 和 list 混合遍历

    Returns:
        True: 重命名成功
        False: 源路径不存在或路径无效
    """
    src = (from_path or "").strip()
    dst = (to_path or "").strip()
    if not src or not dst:
        return False
    if src == dst:
        found, _ = _get_nested_value(obj, src)
        return found

    found, value = _get_nested_value(obj, src)
    if not found:
        return False

    # 先 set 再 delete，避免 set 失败时源值已被删除导致数据丢失
    if not _set_nested_value(obj, dst, value):
        return False
    _delete_nested_value(obj, src)
    return True


def _is_protected_path(parts: list[PathSegment], protected_lower: frozenset[str]) -> bool:
    """检查路径的顶层 key 是否为受保护字段（int 索引不可能是受保护字段）"""
    if not parts:
        return False
    first = parts[0]
    return isinstance(first, str) and first.lower() in protected_lower


def _extract_path(
    rule: dict[str, Any],
    protected_lower: frozenset[str],
    key: str = "path",
) -> str | None:
    """从规则中提取并校验 path 字段，返回 strip 后的路径或 None（无效/受保护时）。"""
    raw = rule.get(key, "")
    if not isinstance(raw, str):
        return None
    path = raw.strip()
    parts = _parse_path(path)
    if not parts:
        return None
    if _is_protected_path(parts, protected_lower):
        return None
    return path


_ORIGINAL_PLACEHOLDER = "{{$original}}"

# ==============================================================================
# 命名风格转换
# ==============================================================================

_NAME_STYLE_VALUES = frozenset(
    {"snake_case", "camelCase", "PascalCase", "kebab-case", "capitalize"}
)

# 拆分标识符为单词列表（支持 camelCase / PascalCase / snake_case / kebab-case / 混合）
_WORD_SPLIT_RE = re.compile(r"[A-Z]?[a-z]+|[A-Z]+(?=[A-Z][a-z]|\d|\b)|[A-Z]|[0-9]+")


def _split_identifier(name: str) -> list[str]:
    """将标识符拆分为小写单词列表"""
    # 先把常见分隔符替换为空格
    normalized = name.replace("_", " ").replace("-", " ")
    words = _WORD_SPLIT_RE.findall(normalized)
    return [w.lower() for w in words if w]


def _convert_name_style(name: str, style: str) -> str:
    """将标识符转换为指定命名风格"""
    words = _split_identifier(name)
    if not words:
        return name

    if style == "snake_case":
        return "_".join(words)
    elif style == "camelCase":
        return words[0] + "".join(w.capitalize() for w in words[1:])
    elif style == "PascalCase":
        return "".join(w.capitalize() for w in words)
    elif style == "kebab-case":
        return "-".join(words)
    elif style == "capitalize":
        # 仅首字母大写，保留其余部分不变
        return name[0].upper() + name[1:] if name else name
    return name


def _contains_original_placeholder(value: Any) -> bool:
    """递归检查 value 中是否包含 {{$original}} 占位符"""
    if isinstance(value, str):
        return _ORIGINAL_PLACEHOLDER in value
    if isinstance(value, dict):
        return any(_contains_original_placeholder(v) for v in value.values())
    if isinstance(value, list):
        return any(_contains_original_placeholder(item) for item in value)
    return False


def _resolve_original_placeholder(template: Any, original: Any) -> Any:
    """递归解析模板中的 {{$original}} 占位符。

    - 字符串完全匹配 {{$original}} → 直接返回原值（保留原始类型）
    - 字符串部分包含 {{$original}} → str(original) 插值
    - dict → 递归处理每个 value
    - list → 递归处理每个元素
    - 其他 → 原样返回
    """
    if isinstance(template, str):
        if template == _ORIGINAL_PLACEHOLDER:
            return original
        if _ORIGINAL_PLACEHOLDER in template:
            return template.replace(_ORIGINAL_PLACEHOLDER, str(original))
        return template
    if isinstance(template, dict):
        return {k: _resolve_original_placeholder(v, original) for k, v in template.items()}
    if isinstance(template, list):
        return [_resolve_original_placeholder(item, original) for item in template]
    return template


_ITEM_PREFIX = "$item."
_ITEM_EXACT = "$item"


def _get_condition_children(
    condition: dict[str, Any],
) -> tuple[str, list[Any]] | None:
    """如果 condition 是 all/any 组合节点，返回 (key, children)；否则返回 None。"""
    for key in ("all", "any"):
        children = condition.get(key)
        if isinstance(children, list):
            return key, children
    return None


def _has_item_ref(condition: dict[str, Any] | None) -> bool:
    """检查 condition 的 path 是否包含 $item 引用"""
    if not condition or not isinstance(condition, dict):
        return False
    group = _get_condition_children(condition)
    if group is not None:
        return any(_has_item_ref(c) for c in group[1] if isinstance(c, dict))
    path = condition.get("path", "")
    return isinstance(path, str) and (
        path.strip().startswith(_ITEM_PREFIX) or path.strip() == _ITEM_EXACT
    )


def _resolve_item_condition(
    condition: dict[str, Any],
    item_path_prefix: str,
) -> dict[str, Any]:
    """将 condition 中的 $item 引用替换为具体的元素路径前缀。

    例如：
        condition = {"path": "$item.name", "op": "in", "value": ["writer"]}
        item_path_prefix = "tools[0]"
        -> {"path": "tools[0].name", "op": "in", "value": ["writer"]}

        condition = {"path": "$item", "op": "type_is", "value": "object"}
        item_path_prefix = "tools[0]"
        -> {"path": "tools[0]", "op": "type_is", "value": "object"}
    """
    group = _get_condition_children(condition)
    if group is not None:
        key, children = group
        return {
            key: [
                _resolve_item_condition(c, item_path_prefix) if isinstance(c, dict) else c
                for c in children
            ]
        }

    resolved = dict(condition)
    raw_path = resolved.get("path", "").strip()
    if raw_path == _ITEM_EXACT:
        resolved["path"] = item_path_prefix
    elif raw_path.startswith(_ITEM_PREFIX):
        suffix = raw_path[len(_ITEM_PREFIX) :]
        resolved["path"] = f"{item_path_prefix}.{suffix}"
    return resolved


def _get_item_prefix_from_concrete(
    concrete_segs: list[str | int],
    wildcard_parts: list[PathSegment],
) -> str:
    """从展开后的具体路径段中，提取通配符所在层级的元素路径前缀。

    例如：
        concrete_segs = ["tools", 0, "name"]
        wildcard_parts = ["tools", _WildcardSlice, "name"]
        -> "tools[0]"  (通配符在 index 1，取 concrete_segs[:2])

        concrete_segs = ["data", 1, "items", 2, "name"]
        wildcard_parts = ["data", _WildcardSlice, "items", _WildcardSlice, "name"]
        -> "data[1].items[2]"  (取到最后一个通配符位置+1)
    """
    # 找到最后一个通配符在 wildcard_parts 中的位置
    last_wc_idx = 0
    for i, seg in enumerate(wildcard_parts):
        if isinstance(seg, _WildcardSlice):
            last_wc_idx = i

    # concrete_segs 中对应位置 +1 就是元素前缀的结束
    prefix_segs = concrete_segs[: last_wc_idx + 1]
    return _segments_to_path(prefix_segs)


def _iter_wildcard_targets(
    result: dict[str, Any],
    path: str,
    parts: list[PathSegment],
    condition: dict[str, Any] | None,
    item_condition: bool,
    *,
    original_body: dict[str, Any] | None = None,
    require_leaf: bool = False,
    reverse: bool = False,
) -> list[str]:
    """通配符路径展开 + $item 条件过滤的通用逻辑。

    如果路径不含通配符，返回 [path] 本身（单元素列表）。
    如果含通配符，展开后逐条评估 $item 条件，返回通过条件的具体路径列表。

    Args:
        result: 当前请求体（用于展开和条件评估）
        path: 原始路径字符串（不含通配符时直接返回）
        parts: 已解析的路径段列表
        condition: 规则的 condition 字典
        item_condition: condition 是否包含 $item 引用
        require_leaf: 是否要求叶子节点存在
        reverse: 是否倒序返回（drop 场景需要倒序避免索引偏移）
    """
    if not _has_wildcard(parts):
        return [path]

    expanded = _expand_wildcard_paths(result, parts, require_leaf=require_leaf)
    if reverse:
        expanded = list(reversed(expanded))

    targets: list[str] = []
    for concrete_segs in expanded:
        if item_condition:
            prefix = _get_item_prefix_from_concrete(concrete_segs, parts)
            resolved = _resolve_item_condition(condition, prefix)  # type: ignore[arg-type]
            if not evaluate_condition(result, resolved, original_body=original_body):
                continue
        targets.append(_segments_to_path(concrete_segs))
    return targets


# ==============================================================================
# 条件评估器
# ==============================================================================

# _CONDITION_OPS / _TYPE_IS_VALUES 从 endpoint_models 导入，避免重复定义

_SIMPLE_TYPE_MAP: dict[str, type] = {
    "string": str,
    "array": list,
    "object": dict,
}


def evaluate_condition(
    body: dict[str, Any],
    condition: dict[str, Any],
    original_body: dict[str, Any] | None = None,
) -> bool:
    """
    评估单个条件表达式，决定规则是否应该执行。

    条件格式: {"path": "model", "op": "starts_with", "value": "claude"}

    条件无效时返回 False（跳过该规则，fail-closed）。
    """
    if not isinstance(condition, dict):
        return False

    if "all" in condition:
        children = condition.get("all")
        if not isinstance(children, list) or not children:
            return False
        return all(
            isinstance(child, dict) and evaluate_condition(body, child, original_body=original_body)
            for child in children
        )

    if "any" in condition:
        children = condition.get("any")
        if not isinstance(children, list) or not children:
            return False
        return any(
            isinstance(child, dict) and evaluate_condition(body, child, original_body=original_body)
            for child in children
        )

    op = condition.get("op")
    if not isinstance(op, str) or op not in _CONDITION_OPS:
        return False

    path = condition.get("path")
    if not isinstance(path, str) or not path.strip():
        return False

    source = condition.get("source", "current")
    if not isinstance(source, str) or source not in {"current", "original"}:
        return False
    target = original_body if source == "original" and original_body is not None else body

    found, current_val = _get_nested_value(target, path.strip())

    # 存在性检查：不需要 value
    if op == "exists":
        return found
    if op == "not_exists":
        return not found

    # 其他操作符要求字段存在
    if not found:
        return False

    expected = condition.get("value")

    # 相等/不等
    if op == "eq":
        return current_val == expected
    if op == "neq":
        return current_val != expected

    # 数值比较
    if op in ("gt", "lt", "gte", "lte"):
        if not isinstance(current_val, (int, float)) or not isinstance(expected, (int, float)):
            return False
        if op == "gt":
            return current_val > expected
        if op == "lt":
            return current_val < expected
        if op == "gte":
            return current_val >= expected
        return current_val <= expected  # lte

    # 字符串操作
    if op == "starts_with":
        return (
            isinstance(current_val, str)
            and isinstance(expected, str)
            and current_val.startswith(expected)
        )
    if op == "ends_with":
        return (
            isinstance(current_val, str)
            and isinstance(expected, str)
            and current_val.endswith(expected)
        )
    if op == "contains":
        if isinstance(current_val, str) and isinstance(expected, str):
            return expected in current_val
        if isinstance(current_val, list):
            return expected in current_val
        return False
    if op == "matches":
        if not isinstance(current_val, str) or not isinstance(expected, str):
            return False
        try:
            return re.search(expected, current_val) is not None
        except re.error:
            return False

    # 列表包含
    if op == "in":
        return isinstance(expected, list) and current_val in expected

    # 类型判断
    if op == "type_is":
        if not isinstance(expected, str) or expected not in _TYPE_IS_VALUES:
            return False
        # bool 是 int 的子类，需要特殊处理
        if expected == "number":
            return isinstance(current_val, (int, float)) and not isinstance(current_val, bool)
        if expected == "boolean":
            return isinstance(current_val, bool)
        if expected == "null":
            return current_val is None
        return isinstance(current_val, _SIMPLE_TYPE_MAP[expected])

    return False


def apply_body_rules(
    body: dict[str, Any],
    rules: list[dict[str, Any]],
    protected_keys: frozenset[str] | None = None,
    original_body: dict[str, Any] | None = None,
) -> dict[str, Any]:
    """
    应用请求体规则

    路径语法：
    - 使用点号分隔层级：metadata.user.name
    - 转义字面量点号：config\\.v1.enabled -> key "config.v1" 下的 "enabled"
    - 使用方括号访问数组元素：messages[0].content
    - 支持多层嵌套：data[0].items[2].name
    - 支持负数索引：messages[-1]
    - 支持连续数组索引：matrix[0][1]
    - 通配符 [*]：遍历数组所有元素，如 tools[*].name
    - 范围 [N-M]：遍历数组索引 N 到 M（含两端），如 tools[0-4].name

    支持的规则类型：
    - set: 设置/覆盖字段 {"action": "set", "path": "metadata.user_id", "value": 123}
        value 中的字符串 {{$original}} 会被替换为该路径的原值（完全匹配时保留类型）
    - drop: 删除字段 {"action": "drop", "path": "unwanted_field"}
    - rename: 重命名字段 {"action": "rename", "from": "old.key", "to": "new.key"}
    - append: 向数组追加元素 {"action": "append", "path": "messages", "value": {...}}
    - insert: 在数组指定位置插入元素 {"action": "insert", "path": "messages", "index": 0, "value": {...}}
    - regex_replace: 正则替换字符串值 {"action": "regex_replace", "path": "messages[0].content",
        "pattern": "\\bfoo\\b", "replacement": "bar", "flags": "i", "count": 0}
    - name_style: 转换字符串命名风格 {"action": "name_style", "path": "tools[*].name",
        "style": "camelCase"}
        支持的风格: snake_case, camelCase, PascalCase, kebab-case, capitalize

    Args:
        body: 原始请求体
        rules: 规则列表
        protected_keys: 受保护的字段（不能被 set/drop/rename 修改）
        original_body: 条件评估使用的原始请求体；未提供时回退到当前 body

    Returns:
        应用规则后的请求体
    """
    if not rules:
        return body

    # 深拷贝，避免修改原始数据（尤其是嵌套 dict/list）
    result = copy.deepcopy(body)
    protected = protected_keys or PROTECTED_BODY_FIELDS
    protected_lower = frozenset(str(k).lower() for k in protected)

    for rule in rules:
        if not isinstance(rule, dict):
            continue

        # 条件触发：
        # - $item 引用的 condition 延迟到通配符循环内逐元素评估
        # - 普通 condition 在此全局评估，不满足则跳过整条规则
        condition = rule.get("condition")
        item_condition = _has_item_ref(condition)
        if condition is not None and not item_condition:
            if not evaluate_condition(result, condition, original_body=original_body):
                continue

        action = rule.get("action")
        if not isinstance(action, str):
            continue
        action = action.strip().lower()

        if action == "set":
            path = _extract_path(rule, protected_lower)
            if not path:
                continue
            parts = _parse_path(path)
            for target_path in _iter_wildcard_targets(
                result, path, parts, condition, item_condition, original_body=original_body
            ):
                value = rule.get("value")
                if _contains_original_placeholder(value):
                    found, original = _get_nested_value(result, target_path)
                    value = _resolve_original_placeholder(value, original if found else None)
                _set_nested_value(result, target_path, value)

        elif action == "drop":
            path = _extract_path(rule, protected_lower)
            if not path:
                continue
            parts = _parse_path(path)
            for target_path in _iter_wildcard_targets(
                result,
                path,
                parts,
                condition,
                item_condition,
                original_body=original_body,
                require_leaf=True,
                reverse=True,
            ):
                _delete_nested_value(result, target_path)

        elif action == "rename":
            raw_from = rule.get("from", "")
            raw_to = rule.get("to", "")
            if not isinstance(raw_from, str) or not isinstance(raw_to, str):
                continue
            from_path = raw_from.strip()
            to_path = raw_to.strip()
            if not from_path or not to_path:
                continue
            from_parts = _parse_path(from_path)
            to_parts = _parse_path(to_path)
            if not from_parts or not to_parts:
                continue

            # 受保护字段只检查顶层 key
            if _is_protected_path(from_parts, protected_lower) or _is_protected_path(
                to_parts, protected_lower
            ):
                continue

            # rename 不支持通配符（语义不明确）
            if _has_wildcard(from_parts) or _has_wildcard(to_parts):
                continue

            _rename_nested_value(result, from_path, to_path)

        elif action == "append":
            path = _extract_path(rule, protected_lower)
            if not path:
                continue
            parts = _parse_path(path)
            for target_path in _iter_wildcard_targets(
                result,
                path,
                parts,
                condition,
                item_condition,
                original_body=original_body,
                require_leaf=True,
            ):
                found, target = _get_nested_value(result, target_path)
                if found and isinstance(target, list):
                    target.append(rule.get("value"))

        elif action == "insert":
            path = _extract_path(rule, protected_lower)
            if not path:
                continue
            index = rule.get("index")
            if not isinstance(index, int):
                continue
            # insert 不支持通配符（索引语义冲突）
            found, target = _get_nested_value(result, path)
            if not found or not isinstance(target, list):
                continue
            target.insert(index, rule.get("value"))

        elif action == "regex_replace":
            path = _extract_path(rule, protected_lower)
            if not path:
                continue
            pattern = rule.get("pattern")
            replacement = rule.get("replacement", "")
            if not isinstance(pattern, str) or not isinstance(replacement, str):
                continue
            if not pattern:
                continue

            flags_raw = rule.get("flags", "")
            re_flags = parse_re_flags(flags_raw if isinstance(flags_raw, str) else "")

            count = rule.get("count", 0)
            if not isinstance(count, int) or count < 0:
                count = 0

            try:
                compiled = re.compile(pattern, re_flags)
            except re.error:
                continue

            parts = _parse_path(path)
            for target_path in _iter_wildcard_targets(
                result,
                path,
                parts,
                condition,
                item_condition,
                original_body=original_body,
                require_leaf=True,
            ):
                found, current_val = _get_nested_value(result, target_path)
                if found and isinstance(current_val, str):
                    new_val = compiled.sub(replacement, current_val, count=count)
                    _set_nested_value(result, target_path, new_val)

        elif action == "name_style":
            path = _extract_path(rule, protected_lower)
            if not path:
                continue
            style = rule.get("style")
            if not isinstance(style, str) or style not in _NAME_STYLE_VALUES:
                continue
            parts = _parse_path(path)
            for target_path in _iter_wildcard_targets(
                result,
                path,
                parts,
                condition,
                item_condition,
                original_body=original_body,
                require_leaf=True,
            ):
                found, current_val = _get_nested_value(result, target_path)
                if found and isinstance(current_val, str):
                    _set_nested_value(result, target_path, _convert_name_style(current_val, style))

    return result


# ==============================================================================
# 请求构建器
# ==============================================================================


class RequestBuilder(ABC):
    """请求构建器抽象基类"""

    @abstractmethod
    def build_payload(
        self,
        original_body: dict[str, Any],
        *,
        mapped_model: str | None = None,
        is_stream: bool = False,
    ) -> dict[str, Any]:
        """构建请求体"""
        pass

    @abstractmethod
    def build_headers(
        self,
        original_headers: dict[str, str],
        endpoint: Any,
        key: Any,
        *,
        extra_headers: dict[str, str] | None = None,
        pre_computed_auth: tuple[str, str] | None = None,
        envelope: ProviderEnvelope | None = None,
        body: dict[str, Any] | None = None,
        original_body: dict[str, Any] | None = None,
    ) -> dict[str, str]:
        """构建请求头"""
        pass

    def build(
        self,
        original_body: dict[str, Any],
        original_headers: dict[str, str],
        endpoint: Any,
        key: Any,
        *,
        mapped_model: str | None = None,
        is_stream: bool = False,
        extra_headers: dict[str, str] | None = None,
        pre_computed_auth: tuple[str, str] | None = None,
        envelope: ProviderEnvelope | None = None,
        protected_body_keys: frozenset[str] | None = None,
        provider_api_format: str | None = None,
    ) -> tuple[dict[str, Any], dict[str, str]]:
        """
        构建完整的请求（请求体 + 请求头）

        Args:
            original_body: 原始请求体
            original_headers: 原始请求头
            endpoint: 端点配置
            key: Provider API Key
            mapped_model: 映射后的模型名
            is_stream: 是否为流式请求
            extra_headers: 额外请求头
            pre_computed_auth: 预先计算的认证信息 (auth_header, auth_value)
            protected_body_keys: body_rules 不允许修改的顶层请求字段
            provider_api_format: 运行时实际生效的 Provider API 格式，用于准确记录摘要日志

        Returns:
            Tuple[payload, headers]
        """
        payload = self.build_payload(
            original_body,
            mapped_model=mapped_model,
            is_stream=is_stream,
        )

        # 应用请求体规则（如果 endpoint 配置了 body_rules）
        body_rules = getattr(endpoint, "body_rules", None)
        if body_rules:
            payload = apply_body_rules(
                payload,
                body_rules,
                protected_keys=protected_body_keys,
                original_body=original_body,
            )

        effective_provider_api_format = provider_api_format or getattr(endpoint, "api_format", None)
        logger.debug(
            "[RequestBuilder] outbound payload summary: {}",
            summarize_request_payload_shape(
                payload,
                provider_api_format=effective_provider_api_format,
                protected_body_keys=protected_body_keys,
                body_rules=body_rules,
            ),
        )

        headers = self.build_headers(
            original_headers,
            endpoint,
            key,
            extra_headers=extra_headers,
            pre_computed_auth=pre_computed_auth,
            envelope=envelope,
            body=payload,
            original_body=original_body,
        )
        return payload, headers


class PassthroughRequestBuilder(RequestBuilder):
    """
    透传模式请求构建器

    适用于 CLI 等场景，尽量保持请求原样：
    - 请求体：直接复制，只修改必要字段（model, stream）
    - 请求头：清理敏感头部（黑名单），透传其他所有头部
    """

    def build_payload(
        self,
        original_body: dict[str, Any],
        *,
        mapped_model: str | None = None,
        is_stream: bool = False,
    ) -> dict[str, Any]:
        """
        透传请求体 - 原样复制，不做任何修改

        透传模式下：
        - model: 由各 handler 的 apply_mapped_model 方法处理
        - stream: 保留客户端原始值（不同 API 处理方式不同）
        """
        del mapped_model, is_stream
        return dict(original_body)

    @staticmethod
    def _merge_comma_header_values(primary: str, secondary: str) -> str:
        """合并逗号分隔 header 值并去重，保持 primary 在前。"""
        seen: set[str] = set()
        merged: list[str] = []

        def _append(raw: str) -> None:
            for token in str(raw or "").split(","):
                token = token.strip()
                if not token or token in seen:
                    continue
                seen.add(token)
                merged.append(token)

        _append(primary)
        _append(secondary)
        return ",".join(merged)

    @classmethod
    def _drop_beta_token(cls, value: str, token: str) -> str:
        """从逗号分隔 header 中移除指定 token。"""
        if not value or token not in value:
            return value
        return cls._merge_comma_header_values(
            ",".join(p.strip() for p in str(value).split(",") if p.strip() and p.strip() != token),
            "",
        )

    @classmethod
    def _merge_extra_headers_with_original(
        cls,
        original_headers: dict[str, str],
        extra_headers: dict[str, str] | None,
        *,
        envelope: ProviderEnvelope | None = None,
    ) -> dict[str, str] | None:
        """合并 extra_headers 与原始头部中的特定字段。"""
        if not extra_headers:
            return None

        merged_extra = dict(extra_headers)
        beta_extra_key = next((k for k in merged_extra if k.lower() == "anthropic-beta"), None)
        if beta_extra_key is None:
            return merged_extra

        incoming_beta = next(
            (v for k, v in original_headers.items() if k.lower() == "anthropic-beta"),
            "",
        )
        merged_beta = str(merged_extra.get(beta_extra_key) or "")
        if incoming_beta:
            merged_beta = cls._merge_comma_header_values(
                merged_beta,
                str(incoming_beta),
            )

        # 由 envelope 声明需要排除的 beta token（如 Claude Code OAuth 的 context-1m）。
        if envelope and hasattr(envelope, "excluded_beta_tokens"):
            for token in envelope.excluded_beta_tokens():
                merged_beta = cls._drop_beta_token(merged_beta, token)

        merged_extra[beta_extra_key] = merged_beta
        return merged_extra

    def build_headers(
        self,
        original_headers: dict[str, str],
        endpoint: Any,
        key: Any,
        *,
        extra_headers: dict[str, str] | None = None,
        pre_computed_auth: tuple[str, str] | None = None,
        envelope: ProviderEnvelope | None = None,
        body: dict[str, Any] | None = None,
        original_body: dict[str, Any] | None = None,
    ) -> dict[str, str]:
        """
        透传请求头 - 清理敏感头部（黑名单），透传其他所有头部

        Args:
            original_headers: 原始请求头
            endpoint: 端点配置
            key: Provider API Key
            extra_headers: 额外请求头
            pre_computed_auth: 预先计算的认证信息 (auth_header, auth_value)，
                               用于 Service Account 等异步获取 token 的场景
        """
        raw_family = getattr(endpoint, "api_family", None)
        raw_kind = getattr(endpoint, "endpoint_kind", None)
        endpoint_sig: str | None = None
        if isinstance(raw_family, str) and isinstance(raw_kind, str) and raw_family and raw_kind:
            endpoint_sig = make_signature_key(raw_family, raw_kind)
        else:
            # 兜底：允许 endpoint.api_format 已经是 signature key 的情况
            raw_format = getattr(endpoint, "api_format", None)
            if isinstance(raw_format, str) and ":" in raw_format:
                endpoint_sig = raw_format

        # 1. 根据 API 格式自动设置认证头
        if pre_computed_auth:
            # 使用预先计算的认证信息（Service Account 等场景）
            auth_header, auth_value = pre_computed_auth
        else:
            # 标准 API Key 认证
            decrypted_key = crypto_service.decrypt(key.api_key)

            auth_header, auth_type = get_auth_config_for_endpoint(endpoint_sig or "openai:chat")
            auth_value = f"Bearer {decrypted_key}" if auth_type == "bearer" else decrypted_key
        # 认证头始终受保护，防止 header_rules 覆盖
        protected_keys = {auth_header.lower(), "content-type"}

        builder = HeaderBuilder()

        # 2. 透传原始头部（排除默认敏感头部）
        if original_headers:
            for name, value in original_headers.items():
                if name.lower() in SENSITIVE_HEADERS:
                    continue
                builder.add(name, value)

        # 3. 应用 endpoint 的请求头规则（认证头受保护，无法通过 rules 设置）
        header_rules = getattr(endpoint, "header_rules", None)
        if header_rules:
            builder.apply_rules(
                header_rules,
                protected_keys,
                body=body,
                original_body=original_body,
                condition_evaluator=evaluate_condition,
            )

        # 4. 添加额外头部
        effective_extra_headers = self._merge_extra_headers_with_original(
            original_headers,
            extra_headers,
            envelope=envelope,
        )
        if effective_extra_headers:
            builder.add_many(effective_extra_headers)

        # 5. 设置认证头（最高优先级，上游始终使用 header 认证）
        builder.add(resolve_header_name_case(original_headers, auth_header), auth_value)

        # 6. 确保有 Content-Type
        headers = builder.build()
        if not any(k.lower() == "content-type" for k in headers):
            headers["Content-Type"] = "application/json"

        return headers


# ==============================================================================
# 便捷函数
# ==============================================================================


def build_passthrough_request(
    original_body: dict[str, Any],
    original_headers: dict[str, str],
    endpoint: Any,
    key: Any,
) -> tuple[dict[str, Any], dict[str, str]]:
    """
    构建透传模式的请求

    纯透传：原样复制请求体，只处理请求头（认证等）。
    model mapping 和 stream 由调用方自行处理（不同 API 格式处理方式不同）。
    """
    builder = PassthroughRequestBuilder()
    return builder.build(
        original_body,
        original_headers,
        endpoint,
        key,
    )
