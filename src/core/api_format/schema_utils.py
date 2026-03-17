"""JSON Schema cleaning utilities shared across Gemini-compatible providers.

Google Gemini / Antigravity v1internal 的 function declaration API 对 JSON Schema
有严格限制。特别是当目标模型为 Claude 时，Schema 必须严格符合 JSON Schema draft 2020-12。

本模块对齐 Antigravity-Manager common/json_schema.rs 的完整清洗逻辑：
1. $ref / $defs 展开（Schema Flattening）
2. allOf 合并
3. anyOf / oneOf 联合类型折叠（择优保留最复杂的分支）
4. 白名单字段过滤（只保留 Gemini 支持的字段）
5. 约束字段迁移到 description（保留语义信息）
6. 类型数组降级（["string", "null"] → "string"）
7. 类型大小写归一化
8. 隐式类型注入
9. required 字段对齐
"""

from __future__ import annotations

import copy
from typing import Any

# Gemini 白名单：只有这些字段在 Schema 节点中允许存在
_ALLOWED_SCHEMA_FIELDS: frozenset[str] = frozenset(
    {
        "type",
        "description",
        "properties",
        "required",
        "items",
        "enum",
        "title",
    }
)

# 约束字段：删除前将语义信息迁移到 description
_CONSTRAINT_FIELDS: tuple[tuple[str, str], ...] = (
    ("minLength", "minLen"),
    ("maxLength", "maxLen"),
    ("pattern", "pattern"),
    ("minimum", "min"),
    ("maximum", "max"),
    ("multipleOf", "multipleOf"),
    ("exclusiveMinimum", "exclMin"),
    ("exclusiveMaximum", "exclMax"),
    ("minItems", "minItems"),
    ("maxItems", "maxItems"),
    ("format", "format"),
)

# Legacy: 向后兼容的简单禁止列表（不再使用，保留用于其他调用者）
GEMINI_FORBIDDEN_SCHEMA_FIELDS: frozenset[str] = frozenset(
    {
        "$schema",
        "additionalProperties",
        "const",
        "contentEncoding",
        "contentMediaType",
        "default",
        "exclusiveMaximum",
        "exclusiveMinimum",
        "multipleOf",
        "patternProperties",
        "propertyNames",
    }
)


# ---------------------------------------------------------------------------
# Public API
# ---------------------------------------------------------------------------


def clean_gemini_schema(schema: dict[str, Any]) -> None:
    """Recursively clean a JSON Schema for Gemini / Antigravity v1internal.

    对齐 AM common/json_schema.rs clean_json_schema：
    1. 收集并展开 $ref / $defs / definitions
    2. 递归白名单清洗
    """
    if not isinstance(schema, dict):
        return

    # Phase 1: 收集所有 $defs（递归所有层级）
    all_defs: dict[str, Any] = {}
    _collect_all_defs(schema, all_defs)

    # 移除根层级的 $defs / definitions
    schema.pop("$defs", None)
    schema.pop("definitions", None)

    # Phase 2: 展开 $ref（递归替换为实际定义）
    _flatten_refs(schema, all_defs)

    # Phase 3: 递归清洗
    _clean_recursive(schema, is_schema_node=True)


# ---------------------------------------------------------------------------
# Phase 1: $defs 收集
# ---------------------------------------------------------------------------


def _collect_all_defs(value: Any, defs: dict[str, Any]) -> None:
    """递归收集所有层级的 $defs 和 definitions。

    对齐 AM #952：MCP 工具可能在任意嵌套层级定义 $defs。
    """
    if isinstance(value, dict):
        for defs_key in ("$defs", "definitions"):
            d = value.get(defs_key)
            if isinstance(d, dict):
                for k, v in d.items():
                    if k not in defs:
                        defs[k] = v
        for key, v in value.items():
            if key not in ("$defs", "definitions"):
                _collect_all_defs(v, defs)
    elif isinstance(value, list):
        for item in value:
            _collect_all_defs(item, defs)


# ---------------------------------------------------------------------------
# Phase 2: $ref 展开
# ---------------------------------------------------------------------------


def _flatten_refs(obj: dict[str, Any], defs: dict[str, Any], _seen: set[str] | None = None) -> None:
    """递归展开 $ref，用定义内容替换引用。

    对齐 AM flatten_refs：
    - 从 $ref 路径中提取名称 (e.g. #/$defs/MyType → MyType)
    - 合并定义内容到当前节点
    - 无法解析的 $ref 降级为 type: string
    - 使用 _seen 防止循环 $ref 导致无限递归
    """
    if _seen is None:
        _seen = set()

    ref_path = obj.pop("$ref", None)
    if isinstance(ref_path, str):
        ref_name = ref_path.rsplit("/", 1)[-1]

        if ref_name in _seen:
            # 循环引用：降级为 string 类型，避免无限递归
            obj.setdefault("type", "string")
            _append_hint(obj, f"(Circular $ref: {ref_path})")
        else:
            _seen.add(ref_name)
            def_schema = defs.get(ref_name)

            if isinstance(def_schema, dict):
                for k, v in def_schema.items():
                    if k not in obj:
                        # 深拷贝避免共享引用导致后续修改污染
                        obj[k] = copy.deepcopy(v)
                # 递归处理合并后的完整节点（包含所有子节点）
                _flatten_refs(obj, defs, _seen)
            else:
                # 无法解析：降级为 string 类型
                obj.setdefault("type", "string")
                hint = f"(Unresolved $ref: {ref_path})"
                desc = obj.get("description", "")
                if not isinstance(desc, str):
                    desc = ""
                if hint not in desc:
                    obj["description"] = f"{desc} {hint}".strip()
            # 回溯：允许同一 $def 在兄弟节点中再次被引用（菱形引用不是循环）
            _seen.discard(ref_name)
        # $ref 展开后递归调用已处理所有子节点，无需再遍历
        return

    # 仅对非 $ref 节点遍历子节点
    for v in obj.values():
        if isinstance(v, dict):
            _flatten_refs(v, defs, _seen)
        elif isinstance(v, list):
            for item in v:
                if isinstance(item, dict):
                    _flatten_refs(item, defs, _seen)


# ---------------------------------------------------------------------------
# Phase 3: 递归清洗
# ---------------------------------------------------------------------------


def _clean_recursive(value: Any, *, is_schema_node: bool) -> bool:
    """递归清洗 Schema 节点，返回 is_effectively_nullable。

    对齐 AM clean_json_schema_recursive 的完整逻辑。
    """
    if not isinstance(value, dict):
        if isinstance(value, list):
            for item in value:
                _clean_recursive(item, is_schema_node=is_schema_node)
        return False

    is_nullable = False

    # 0. allOf 合并
    _merge_all_of(value)

    # 0.5 结构归一化：type=object 但有 items → 移到 properties
    if (value.get("type") == "object" or "properties" in value) and "items" in value:
        items = value.pop("items")
        if isinstance(items, dict):
            props = value.setdefault("properties", {})
            if isinstance(props, dict):
                props.update({k: v for k, v in items.items() if k not in props})

    # 1. 递归处理 properties
    props = value.get("properties")
    if isinstance(props, dict):
        nullable_keys: set[str] = set()
        for k, v in props.items():
            if isinstance(v, dict):
                if _clean_recursive(v, is_schema_node=True):
                    nullable_keys.add(k)

        # 从 required 中移除 nullable 的键
        if nullable_keys:
            req = value.get("required")
            if isinstance(req, list):
                req[:] = [r for r in req if not (isinstance(r, str) and r in nullable_keys)]
                if not req:
                    value.pop("required", None)

        # 隐式类型注入
        if "type" not in value:
            value["type"] = "object"

    # 处理 items
    items = value.get("items")
    if isinstance(items, dict):
        _clean_recursive(items, is_schema_node=True)
        if "type" not in value:
            value["type"] = "array"

    # Fallback: 对既没 properties 也没 items 的对象递归处理
    if "properties" not in value and "items" not in value:
        skip_keys = {"anyOf", "oneOf", "allOf", "enum", "type"}
        for k, v in value.items():
            if k not in skip_keys and isinstance(v, (dict, list)):
                _clean_recursive(v, is_schema_node=False)

    # 1.5 递归清洗 anyOf / oneOf 分支
    for combo_key in ("anyOf", "oneOf"):
        combo = value.get(combo_key)
        if isinstance(combo, list):
            for branch in combo:
                if isinstance(branch, dict):
                    _clean_recursive(branch, is_schema_node=True)

    # 2. anyOf / oneOf 折叠：选取最佳分支合并到当前节点
    union_to_merge = None
    if value.get("type") is None or value.get("type") == "object":
        for combo_key in ("anyOf", "oneOf"):
            combo = value.get(combo_key)
            if isinstance(combo, list):
                union_to_merge = combo
                break

    if union_to_merge is not None:
        best, all_types = _extract_best_branch(union_to_merge)
        if best is not None and isinstance(best, dict):
            for k, v in best.items():
                if k == "properties":
                    target = value.setdefault("properties", {})
                    if isinstance(target, dict) and isinstance(v, dict):
                        for pk, pv in v.items():
                            if pk not in target:
                                target[pk] = pv
                elif k == "required":
                    target_req = value.setdefault("required", [])
                    if isinstance(target_req, list) and isinstance(v, list):
                        for rv in v:
                            if rv not in target_req:
                                target_req.append(rv)
                elif k not in value:
                    value[k] = v

            # 添加类型提示
            if len(all_types) > 1:
                _append_hint(value, f"Accepts: {' | '.join(all_types)}")

    # 移除 anyOf / oneOf（已合并）
    value.pop("anyOf", None)
    value.pop("oneOf", None)

    # 3. 判断是否为 Schema 节点
    is_not_schema_payload = "functionCall" in value or "functionResponse" in value
    has_standard = any(k in value for k in _ALLOWED_SCHEMA_FIELDS)

    # 3.5 启发式修复：Schema 节点但没有标准关键字 → 把所有 key 移到 properties
    if is_schema_node and not has_standard and value and not is_not_schema_payload:
        all_keys = list(value.keys())
        new_props: dict[str, Any] = {}
        for k in all_keys:
            new_props[k] = value.pop(k)
        value["type"] = "object"
        value["properties"] = new_props
        # 递归清洗刚移入的属性
        for v in new_props.values():
            if isinstance(v, dict):
                _clean_recursive(v, is_schema_node=True)
        has_standard = True

    looks_like_schema = (is_schema_node or has_standard) and not is_not_schema_payload

    if looks_like_schema:
        # 4. 约束迁移到 description
        _move_constraints_to_description(value)

        # 5. 白名单过滤
        keys_to_remove = [k for k in value if k not in _ALLOWED_SCHEMA_FIELDS]
        for k in keys_to_remove:
            del value[k]

        # 6. 空 Object 处理
        if value.get("type") == "object" and "properties" not in value:
            value["properties"] = {}

        # 7. required 字段对齐
        valid_keys = None
        p = value.get("properties")
        if isinstance(p, dict):
            valid_keys = set(p.keys())

        req = value.get("required")
        if isinstance(req, list):
            if valid_keys is not None:
                req[:] = [r for r in req if isinstance(r, str) and r in valid_keys]
            else:
                req.clear()

        # 隐式类型注入（如果白名单过滤后丢失了 type）
        if "type" not in value:
            if "enum" in value:
                value["type"] = "string"
            elif "properties" in value:
                value["type"] = "object"
            elif "items" in value:
                value["type"] = "array"

        # 8. 类型处理：数组 → 单一类型 + 大小写归一化
        fallback_type = (
            "object" if "properties" in value else "array" if "items" in value else "string"
        )

        type_val = value.get("type")
        if type_val is not None:
            selected: str | None = None
            if isinstance(type_val, str):
                lower = type_val.lower()
                if lower == "null":
                    is_nullable = True
                else:
                    selected = lower
            elif isinstance(type_val, list):
                for item in type_val:
                    if isinstance(item, str):
                        lower = item.lower()
                        if lower == "null":
                            is_nullable = True
                        elif selected is None:
                            selected = lower
            value["type"] = selected if selected else fallback_type

        if is_nullable:
            _append_hint(value, "(nullable)")

        # 9. enum 值强制转字符串
        enum_val = value.get("enum")
        if isinstance(enum_val, list):
            for i, item in enumerate(enum_val):
                if not isinstance(item, str):
                    enum_val[i] = "null" if item is None else str(item)

    return is_nullable


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _merge_all_of(obj: dict[str, Any]) -> None:
    """合并 allOf 数组中的所有子 Schema。对齐 AM merge_all_of。"""
    all_of = obj.pop("allOf", None)
    if not isinstance(all_of, list):
        return

    merged_props: dict[str, Any] = {}
    merged_required: list[str] = []
    merged_required_seen: set[str] = set()
    other_fields: dict[str, Any] = {}

    for sub in all_of:
        if not isinstance(sub, dict):
            continue
        # 合并 properties
        p = sub.get("properties")
        if isinstance(p, dict):
            merged_props.update(p)
        # 合并 required
        r = sub.get("required")
        if isinstance(r, list):
            for item in r:
                if isinstance(item, str) and item not in merged_required_seen:
                    merged_required_seen.add(item)
                    merged_required.append(item)
        # 合并其余字段
        for k, v in sub.items():
            if k not in ("properties", "required", "allOf") and k not in other_fields:
                other_fields[k] = v

    for k, v in other_fields.items():
        if k not in obj:
            obj[k] = v

    if merged_props:
        target = obj.setdefault("properties", {})
        if isinstance(target, dict):
            for k, v in merged_props.items():
                if k not in target:
                    target[k] = v

    if merged_required:
        target_req = obj.setdefault("required", [])
        if isinstance(target_req, list):
            existing = {r for r in target_req if isinstance(r, str)}
            for r in merged_required:
                if r not in existing:
                    target_req.append(r)


def _score_branch(val: Any) -> int:
    """对 Schema 分支打分：Object(3) > Array(2) > Scalar(1) > Null(0)。"""
    if not isinstance(val, dict):
        return 0
    if "properties" in val or val.get("type") == "object":
        return 3
    if "items" in val or val.get("type") == "array":
        return 2
    t = val.get("type")
    if isinstance(t, str) and t != "null":
        return 1
    return 0


def _get_type_name(val: Any) -> str | None:
    """获取 Schema 的类型名称。"""
    if not isinstance(val, dict):
        return None
    t = val.get("type")
    if isinstance(t, str):
        return t
    if "properties" in val:
        return "object"
    if "items" in val:
        return "array"
    return None


def _extract_best_branch(
    union: list[Any],
) -> tuple[dict[str, Any] | None, list[str]]:
    """从 anyOf/oneOf 中选取最佳非 null 分支。返回 (best, all_types)。"""
    best: dict[str, Any] | None = None
    best_score = -1
    all_types: list[str] = []

    for item in union:
        score = _score_branch(item)
        tn = _get_type_name(item)
        if tn and tn not in all_types:
            all_types.append(tn)
        if score > best_score:
            best_score = score
            if isinstance(item, dict):
                best = item
    return best, all_types


def _move_constraints_to_description(obj: dict[str, Any]) -> None:
    """将约束字段迁移到 description。对齐 AM move_constraints_to_description。"""
    hints: list[str] = []
    for field, label in _CONSTRAINT_FIELDS:
        val = obj.get(field)
        if val is not None:
            hints.append(f"{label}: {val}")
    if hints:
        _append_hint(obj, f"[Constraint: {', '.join(hints)}]")


def _append_hint(obj: dict[str, Any], hint: str) -> None:
    """追加提示到 description 字段。"""
    desc = obj.get("description", "")
    if not isinstance(desc, str):
        desc = ""
    if hint not in desc:
        obj["description"] = f"{desc} {hint}".strip() if desc else hint


__all__ = [
    "GEMINI_FORBIDDEN_SCHEMA_FIELDS",
    "clean_gemini_schema",
]
