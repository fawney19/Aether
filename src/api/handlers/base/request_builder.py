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
import time
from abc import ABC, abstractmethod
from dataclasses import dataclass
from typing import TYPE_CHECKING, Any

import httpx
from sqlalchemy.orm import object_session

from src.clients.redis_client import get_redis_client
from src.core.api_format import (
    UPSTREAM_DROP_HEADERS,
    HeaderBuilder,
    get_auth_config_for_endpoint,
    make_signature_key,
)
from src.core.crypto import crypto_service
from src.core.logger import logger
from src.core.provider_oauth_utils import enrich_auth_config, post_oauth_token
from src.models.endpoint_models import _CONDITION_OPS, _TYPE_IS_VALUES, parse_re_flags
from src.utils import json_helper as json

if TYPE_CHECKING:
    from src.models.database import ProviderAPIKey, ProviderEndpoint


# ==============================================================================
# Service Account 认证结果类型
# ==============================================================================


@dataclass
class ProviderAuthInfo:
    """Provider 认证信息（用于 Service Account 等异步认证场景）"""

    auth_header: str
    auth_value: str
    # 解密后的认证配置（用于 URL 构建等场景，避免重复解密）
    decrypted_auth_config: dict[str, Any] | None = None

    def as_tuple(self) -> tuple[str, str]:
        """返回 (auth_header, auth_value) 元组"""
        return (self.auth_header, self.auth_value)


# ==============================================================================
# OAuth Token Refresh helpers
# ==============================================================================


async def _acquire_refresh_lock(key_id: str) -> tuple[Any, bool]:
    """尝试获取 OAuth refresh 分布式锁。

    返回 ``(redis_client | None, got_lock)``。调用方在刷新完成后
    必须调用 :func:`_release_refresh_lock` 释放锁。
    """
    redis = await get_redis_client(require_redis=False)
    lock_key = f"provider_oauth_refresh_lock:{key_id}"
    got_lock = False
    if redis is not None:
        try:
            got_lock = bool(await redis.set(lock_key, "1", ex=30, nx=True))
        except Exception:
            got_lock = False
    return redis, got_lock


async def _release_refresh_lock(redis: Any, key_id: str) -> None:
    """释放 OAuth refresh 分布式锁（best-effort）。"""
    if redis is not None:
        try:
            await redis.delete(f"provider_oauth_refresh_lock:{key_id}")
        except Exception:
            pass


def _persist_refreshed_token(
    key: Any,
    access_token: str,
    token_meta: dict[str, Any],
) -> None:
    """将刷新后的 access_token 和 auth_config 持久化到数据库。"""
    key.api_key = crypto_service.encrypt(access_token)
    key.auth_config = crypto_service.encrypt(json.dumps(token_meta))

    sess = object_session(key)
    if sess is not None:
        sess.add(key)
        sess.commit()
    else:
        logger.warning(
            "[OAUTH_REFRESH] key {} refreshed but cannot persist (no session); "
            "next request will refresh again",
            key.id,
        )


def _get_proxy_config(key: Any, endpoint: Any = None) -> Any:
    """获取有效代理配置（Key 级别优先于 Provider 级别）。"""
    try:
        from src.services.proxy_node.resolver import resolve_effective_proxy

        provider = getattr(key, "provider", None) or (
            getattr(endpoint, "provider", None) if endpoint else None
        )
        provider_proxy = getattr(provider, "proxy", None)
        key_proxy = getattr(key, "proxy", None)
        return resolve_effective_proxy(provider_proxy, key_proxy)
    except Exception:
        return None


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


# 路径段类型：str 表示 dict key，int 表示数组索引
PathSegment = str | int


def _parse_path(path: str) -> list[PathSegment]:
    """
    解析路径，支持点号分隔、转义和数组索引。

    Examples:
        "metadata.user.name"       -> ["metadata", "user", "name"]
        "config\\.v1.enabled"      -> ["config.v1", "enabled"]
        "messages[0].content"      -> ["messages", 0, "content"]
        "data[0].items[2].name"    -> ["data", 0, "items", 2, "name"]
        "messages[-1]"             -> ["messages", -1]
        "matrix[0][1]"             -> ["matrix", 0, 1]

    约束：
        - 不允许空段（例如：".a" / "a." / "a..b"），遇到则返回空列表表示无效路径。
        - 仅对 "\\." 做特殊处理；其他反斜杠组合按字面量保留。
        - 数组索引必须是整数（支持负数索引）。
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

        # 数组索引：[N]
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


# ==============================================================================
# 条件评估器
# ==============================================================================

# _CONDITION_OPS / _TYPE_IS_VALUES 从 endpoint_models 导入，避免重复定义

_SIMPLE_TYPE_MAP: dict[str, type] = {
    "string": str,
    "array": list,
    "object": dict,
}


def _evaluate_condition(body: dict[str, Any], condition: dict[str, Any]) -> bool:
    """
    评估单个条件表达式，决定规则是否应该执行。

    条件格式: {"path": "model", "op": "starts_with", "value": "claude"}

    条件无效时返回 False（跳过该规则，fail-closed）。
    """
    if not isinstance(condition, dict):
        return False

    op = condition.get("op")
    if not isinstance(op, str) or op not in _CONDITION_OPS:
        return False

    path = condition.get("path")
    if not isinstance(path, str) or not path.strip():
        return False

    found, current_val = _get_nested_value(body, path.strip())

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

    支持的规则类型：
    - set: 设置/覆盖字段 {"action": "set", "path": "metadata.user_id", "value": 123}
        value 中的字符串 {{$original}} 会被替换为该路径的原值（完全匹配时保留类型）
    - drop: 删除字段 {"action": "drop", "path": "unwanted_field"}
    - rename: 重命名字段 {"action": "rename", "from": "old.key", "to": "new.key"}
    - append: 向数组追加元素 {"action": "append", "path": "messages", "value": {...}}
    - insert: 在数组指定位置插入元素 {"action": "insert", "path": "messages", "index": 0, "value": {...}}
    - regex_replace: 正则替换字符串值 {"action": "regex_replace", "path": "messages[0].content",
        "pattern": "\\bfoo\\b", "replacement": "bar", "flags": "i", "count": 0}

    Args:
        body: 原始请求体
        rules: 规则列表
        protected_keys: 受保护的字段（不能被 set/drop/rename 修改）

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

        # 条件触发：condition 存在且不满足时跳过规则
        condition = rule.get("condition")
        if condition is not None and not _evaluate_condition(result, condition):
            continue

        action = rule.get("action")
        if not isinstance(action, str):
            continue
        action = action.strip().lower()

        if action == "set":
            path = _extract_path(rule, protected_lower)
            if not path:
                continue
            value = rule.get("value")
            if _contains_original_placeholder(value):
                found, original = _get_nested_value(result, path)
                value = _resolve_original_placeholder(value, original if found else None)
            _set_nested_value(result, path, value)

        elif action == "drop":
            path = _extract_path(rule, protected_lower)
            if not path:
                continue
            _delete_nested_value(result, path)

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

            _rename_nested_value(result, from_path, to_path)

        elif action == "append":
            path = _extract_path(rule, protected_lower)
            if not path:
                continue
            found, target = _get_nested_value(result, path)
            if not found or not isinstance(target, list):
                continue
            target.append(rule.get("value"))

        elif action == "insert":
            path = _extract_path(rule, protected_lower)
            if not path:
                continue
            index = rule.get("index")
            if not isinstance(index, int):
                continue
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

            found, current_val = _get_nested_value(result, path)
            if not found or not isinstance(current_val, str):
                continue

            try:
                new_val = re.compile(pattern, re_flags).sub(replacement, current_val, count=count)
                _set_nested_value(result, path, new_val)
            except re.error:
                continue  # 正则表达式无效，跳过

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
            payload = apply_body_rules(payload, body_rules)

        headers = self.build_headers(
            original_headers,
            endpoint,
            key,
            extra_headers=extra_headers,
            pre_computed_auth=pre_computed_auth,
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
        mapped_model: str | None = None,  # noqa: ARG002 - 由 apply_mapped_model 处理
        is_stream: bool = False,  # noqa: ARG002 - 保留原始值，不自动添加
    ) -> dict[str, Any]:
        """
        透传请求体 - 原样复制，不做任何修改

        透传模式下：
        - model: 由各 handler 的 apply_mapped_model 方法处理
        - stream: 保留客户端原始值（不同 API 处理方式不同）
        """
        return dict(original_body)

    def build_headers(
        self,
        original_headers: dict[str, str],
        endpoint: Any,
        key: Any,
        *,
        extra_headers: dict[str, str] | None = None,
        pre_computed_auth: tuple[str, str] | None = None,
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
        # 1. 根据 API 格式自动设置认证头
        if pre_computed_auth:
            # 使用预先计算的认证信息（Service Account 等场景）
            auth_header, auth_value = pre_computed_auth
        else:
            # 标准 API Key 认证
            decrypted_key = crypto_service.decrypt(key.api_key)
            raw_family = getattr(endpoint, "api_family", None)
            raw_kind = getattr(endpoint, "endpoint_kind", None)
            endpoint_sig: str | None = None
            if (
                isinstance(raw_family, str)
                and isinstance(raw_kind, str)
                and raw_family
                and raw_kind
            ):
                endpoint_sig = make_signature_key(raw_family, raw_kind)
            else:
                # 兜底：允许 endpoint.api_format 已经是 signature key 的情况
                raw_format = getattr(endpoint, "api_format", None)
                if isinstance(raw_format, str) and ":" in raw_format:
                    endpoint_sig = raw_format

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
            builder.apply_rules(header_rules, protected_keys)

        # 4. 添加额外头部
        if extra_headers:
            builder.add_many(extra_headers)

        # 5. 设置认证头（最高优先级，上游始终使用 header 认证）
        builder.add(auth_header, auth_value)

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


# ==============================================================================
# OAuth Token Refresh logic (Kiro / Generic)
# ==============================================================================


async def _refresh_kiro_token(
    key: Any,
    endpoint: Any,
    token_meta: dict[str, Any],
) -> dict[str, Any]:
    """Kiro OAuth refresh: validate + call Kiro-specific refresh endpoint."""
    from src.core.exceptions import InvalidRequestException
    from src.services.provider.adapters.kiro.models.credentials import KiroAuthConfig
    from src.services.provider.adapters.kiro.token_manager import (
        refresh_access_token,
        validate_refresh_token,
    )

    cfg = KiroAuthConfig.from_dict(token_meta or {})
    if not (cfg.refresh_token or "").strip():
        raise InvalidRequestException(
            "Kiro auth_config missing refresh_token; please re-import credentials."
        )

    proxy_config = _get_proxy_config(key, endpoint)

    validate_refresh_token(cfg.refresh_token)
    access_token, new_cfg = await refresh_access_token(
        cfg,
        proxy_config=proxy_config,
    )
    new_meta = new_cfg.to_dict()
    new_meta["updated_at"] = int(time.time())

    _persist_refreshed_token(key, access_token, new_meta)
    return new_meta


async def _refresh_generic_oauth_token(
    key: Any,
    endpoint: Any,
    template: Any,
    provider_type: str,
    refresh_token: str,
    token_meta: dict[str, Any],
) -> dict[str, Any]:
    """Generic OAuth refresh via template (Codex, Antigravity, ClaudeCode, etc.)."""
    token_url = template.oauth.token_url
    is_json = "anthropic.com" in token_url

    scopes = getattr(template.oauth, "scopes", None) or []
    scope_str = " ".join(scopes) if scopes else ""

    if is_json:
        body: dict[str, Any] = {
            "grant_type": "refresh_token",
            "client_id": template.oauth.client_id,
            "refresh_token": str(refresh_token),
        }
        if scope_str:
            body["scope"] = scope_str
        headers = {
            "Content-Type": "application/json",
            "Accept": "application/json",
        }
        data = None
        json_body = body
    else:
        form: dict[str, str] = {
            "grant_type": "refresh_token",
            "client_id": template.oauth.client_id,
            "refresh_token": str(refresh_token),
        }
        if scope_str:
            form["scope"] = scope_str
        if template.oauth.client_secret:
            form["client_secret"] = template.oauth.client_secret
        headers = {
            "Content-Type": "application/x-www-form-urlencoded",
            "Accept": "application/json",
        }
        data = form
        json_body = None

    proxy_config = _get_proxy_config(key, endpoint)

    resp = await post_oauth_token(
        provider_type=provider_type,
        token_url=token_url,
        headers=headers,
        data=data,
        json_body=json_body,
        proxy_config=proxy_config,
        timeout_seconds=30.0,
    )

    if 200 <= resp.status_code < 300:
        token = resp.json()
        access_token = str(token.get("access_token") or "")
        new_refresh_token = str(token.get("refresh_token") or "")
        expires_in = token.get("expires_in")
        new_expires_at: int | None = None
        try:
            if expires_in is not None:
                new_expires_at = int(time.time()) + int(expires_in)
        except Exception:
            new_expires_at = None

        if access_token:
            token_meta["token_type"] = token.get("token_type")
            if new_refresh_token:
                token_meta["refresh_token"] = new_refresh_token
            token_meta["expires_at"] = new_expires_at
            token_meta["scope"] = token.get("scope")
            token_meta["updated_at"] = int(time.time())

            token_meta = await enrich_auth_config(
                provider_type=provider_type,
                auth_config=token_meta,
                token_response=token,
                access_token=access_token,
                proxy_config=proxy_config,
            )

            _persist_refreshed_token(key, access_token, token_meta)
    else:
        logger.warning(
            "OAuth token refresh failed: provider={}, key_id={}, status={}",
            provider_type,
            getattr(key, "id", "?"),
            resp.status_code,
        )

    return token_meta


# ==============================================================================
# Service Account 认证支持
# ==============================================================================


async def get_provider_auth(
    endpoint: "ProviderEndpoint",
    key: "ProviderAPIKey",
    *,
    force_refresh: bool = False,
) -> ProviderAuthInfo | None:
    """
    获取 Provider 的认证信息

    对于标准 API Key，返回 None（由 build_headers 自动处理）。
    对于 Service Account，异步获取 Access Token 并返回认证信息。

    Args:
        endpoint: 端点配置
        key: Provider API Key

    Returns:
        Service Account 场景: ProviderAuthInfo 对象（包含认证信息和解密后的配置）
        API Key 场景: None（由 build_headers 处理）

    Raises:
        InvalidRequestException: 认证配置无效或认证失败
    """
    from src.core.exceptions import InvalidRequestException

    auth_type = getattr(key, "auth_type", "api_key")

    if auth_type == "oauth":
        # OAuth token 保存在 key.api_key（加密），refresh_token/expires_at 等在 auth_config（加密 JSON）中。
        # 在请求前做一次懒刷新：接近过期时刷新 access_token，并用 Redis lock 避免并发风暴。
        encrypted_auth_config = getattr(key, "auth_config", None)
        if encrypted_auth_config:
            try:
                decrypted_config = crypto_service.decrypt(encrypted_auth_config)
                token_meta = json.loads(decrypted_config)
            except Exception:
                token_meta = {}
        else:
            token_meta = {}

        expires_at = token_meta.get("expires_at")
        refresh_token = token_meta.get("refresh_token")
        provider_type = str(token_meta.get("provider_type") or "")
        cached_access_token = str(token_meta.get("access_token") or "").strip()

        # 120s skew (or force refresh when upstream returns 401)
        should_refresh = False
        try:
            if expires_at is not None:
                should_refresh = int(time.time()) >= int(expires_at) - 120
        except Exception:
            should_refresh = False

        if force_refresh:
            should_refresh = True

        # Kiro 特殊处理：如果没有缓存的 access_token 或 key.api_key 是占位符，强制刷新
        if provider_type == "kiro" and not should_refresh:
            if not cached_access_token:
                should_refresh = True
            elif crypto_service.decrypt(key.api_key) == "__placeholder__":
                should_refresh = True

        if should_refresh and refresh_token and provider_type:
            try:
                from src.core.provider_templates.fixed_providers import FIXED_PROVIDERS
                from src.core.provider_templates.types import ProviderType

                try:
                    template = FIXED_PROVIDERS.get(ProviderType(provider_type))
                except Exception:
                    template = None

                redis, got_lock = await _acquire_refresh_lock(key.id)
                if got_lock or redis is None:
                    try:
                        if provider_type == ProviderType.KIRO.value:
                            token_meta = await _refresh_kiro_token(key, endpoint, token_meta)
                        elif template:
                            token_meta = await _refresh_generic_oauth_token(
                                key, endpoint, template, provider_type, refresh_token, token_meta
                            )
                    finally:
                        if got_lock:
                            await _release_refresh_lock(redis, key.id)
            except Exception:
                # 刷新失败不阻断请求；后续由上游返回 401 再触发管理端处理
                pass

        # 获取最终使用的 access_token
        # Kiro 优先使用 token_meta 中缓存的 access_token（刷新后会更新到 token_meta）
        if provider_type == "kiro":
            refreshed_token = str(token_meta.get("access_token") or "").strip()
            effective_token = refreshed_token or crypto_service.decrypt(key.api_key)
        else:
            effective_token = crypto_service.decrypt(key.api_key)

        decrypted_auth_config: dict[str, Any] | None = None
        if isinstance(token_meta, dict) and token_meta:
            decrypted_auth_config = token_meta

        return ProviderAuthInfo(
            auth_header="Authorization",
            auth_value=f"Bearer {effective_token}",
            decrypted_auth_config=decrypted_auth_config,
        )
    if auth_type == "vertex_ai":
        from src.core.vertex_auth import VertexAuthError, VertexAuthService

        try:
            # 优先从 auth_config 读取，兼容从 api_key 读取（过渡期）
            encrypted_auth_config = getattr(key, "auth_config", None)
            if encrypted_auth_config:
                # auth_config 可能是加密字符串或未加密的 dict
                if isinstance(encrypted_auth_config, dict):
                    # 已经是 dict，直接使用（兼容未加密存储的情况）
                    sa_json = encrypted_auth_config
                else:
                    # 是加密字符串，需要解密
                    decrypted_config = crypto_service.decrypt(encrypted_auth_config)
                    sa_json = json.loads(decrypted_config)
            else:
                # 兼容旧数据：从 api_key 读取
                decrypted_key = crypto_service.decrypt(key.api_key)
                # 检查是否是占位符（表示 auth_config 丢失）
                if decrypted_key == "__placeholder__":
                    raise InvalidRequestException("认证配置丢失，请重新添加该密钥。")
                sa_json = json.loads(decrypted_key)

            if not isinstance(sa_json, dict):
                raise InvalidRequestException("Service Account JSON 无效，请重新添加该密钥。")

            # 获取 Access Token
            service = VertexAuthService(sa_json)
            access_token = await service.get_access_token()

            # Vertex AI 使用 Bearer token
            return ProviderAuthInfo(
                auth_header="Authorization",
                auth_value=f"Bearer {access_token}",
                decrypted_auth_config=sa_json,
            )
        except InvalidRequestException:
            raise
        except VertexAuthError as e:
            raise InvalidRequestException(f"Vertex AI 认证失败：{e}")
        except json.JSONDecodeError:
            raise InvalidRequestException("Service Account JSON 格式无效，请重新添加该密钥。")
        except Exception:
            raise InvalidRequestException("Vertex AI 认证失败，请检查 Key 的 auth_config")

    # 其他认证类型可在此扩展
    # elif auth_type == "oauth2":
    #     ...

    # 标准 API Key：返回 None，由 build_headers 处理
    return None
