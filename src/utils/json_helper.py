"""JSON 序列化/反序列化优化包装。

优先使用 orjson（更快），未安装时自动回退到标准库 json。

推荐用法：
    from src.utils import json_helper as json

说明：
- loads/dumps 返回值类型与标准库一致（loads->Any, dumps->str）。
- 调用方若依赖标准库 json 的高级参数（如 parse_float 等），会自动回退到标准库实现。
"""

from __future__ import annotations

import json as _std_json
from typing import Any

try:
    import orjson  # type: ignore

    HAS_ORJSON = True
except ImportError:  # pragma: no cover
    orjson = None  # type: ignore[assignment]
    HAS_ORJSON = False


# 统一暴露 JSONDecodeError，便于调用方 `except json.JSONDecodeError`
if HAS_ORJSON:
    JSONDecodeError = orjson.JSONDecodeError  # type: ignore[attr-defined]
else:
    JSONDecodeError = _std_json.JSONDecodeError


def loads(s: str | bytes | bytearray | memoryview, **kwargs: Any) -> Any:
    """JSON 反序列化。"""
    if HAS_ORJSON and not kwargs:
        if isinstance(s, str):
            s = s.encode("utf-8")
        return orjson.loads(s)  # type: ignore[no-any-return]

    # 标准库 json.loads 支持 bytes/bytearray，会按 UTF-8 解码。
    return _std_json.loads(s, **kwargs)


def dumps_bytes(obj: Any, **kwargs: Any) -> bytes:
    """JSON 序列化为 UTF-8 bytes（便于网络传输/缓存写入）。"""
    if not HAS_ORJSON:
        return _std_json.dumps(obj, **kwargs).encode("utf-8")

    ensure_ascii = kwargs.pop("ensure_ascii", True)
    sort_keys = kwargs.pop("sort_keys", False)
    default = kwargs.pop("default", None)
    separators = kwargs.pop("separators", None)
    indent = kwargs.pop("indent", None)

    # 还有未处理的参数，说明调用方可能依赖 stdlib 语义，安全起见直接回退。
    if kwargs:
        return _std_json.dumps(
            obj,
            ensure_ascii=ensure_ascii,
            sort_keys=sort_keys,
            default=default,
            separators=separators,
            indent=indent,
            **kwargs,
        ).encode("utf-8")

    # orjson 不支持 ensure_ascii=True（转义非 ASCII），需要兼容时回退标准库。
    if ensure_ascii:
        return _std_json.dumps(
            obj,
            ensure_ascii=True,
            sort_keys=sort_keys,
            default=default,
            separators=separators,
            indent=indent,
        ).encode("utf-8")

    # orjson 不支持 separators；仅当调用方显式要求 compact 分隔符时继续使用 orjson。
    if separators is not None and separators != (",", ":"):
        return _std_json.dumps(
            obj,
            ensure_ascii=False,
            sort_keys=sort_keys,
            default=default,
            separators=separators,
            indent=indent,
        ).encode("utf-8")

    option = 0

    if sort_keys:
        option |= orjson.OPT_SORT_KEYS  # type: ignore[attr-defined]

    if indent is not None:
        # orjson 仅支持 indent=2（OPT_INDENT_2）。其他缩进需求回退标准库。
        if indent == 2:
            option |= orjson.OPT_INDENT_2  # type: ignore[attr-defined]
        else:
            return _std_json.dumps(
                obj,
                ensure_ascii=False,
                sort_keys=sort_keys,
                default=default,
                separators=separators,
                indent=indent,
            ).encode("utf-8")

    return orjson.dumps(obj, default=default, option=option)  # type: ignore[arg-type]


def dumps(obj: Any, **kwargs: Any) -> str:
    """JSON 序列化为 str。"""
    return dumps_bytes(obj, **kwargs).decode("utf-8")


__all__ = ["HAS_ORJSON", "JSONDecodeError", "loads", "dumps", "dumps_bytes"]
