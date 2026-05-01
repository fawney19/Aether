"""
DimensionCollector 运行时维度采集

特性（与 .plans/humming-seeking-marble.md 对齐）：
- (api_format, task_type) 作用域
- 同一维度支持多条 collector（priority 回退）
- 支持 transform_expression（与 billing expression 共用 AST 安全规范）
- computed 维度支持依赖拓扑排序，并对环依赖做保护性降级
"""

from __future__ import annotations

import re
from collections import deque
from dataclasses import dataclass
from typing import Any, Literal, Protocol

from sqlalchemy.orm import Session

from src.core.logger import logger
from src.models.database import DimensionCollector
from src.services.billing.cache import BillingCache
from src.services.billing.formula_engine import (
    ExpressionEvaluationError,
    SafeExpressionEvaluator,
    UnsafeExpressionError,
    extract_variable_names,
)
from src.services.billing.presets import CORE_PRESET_PACK

ValueType = Literal["float", "int", "string"]


class CollectorLike(Protocol):
    api_format: str
    task_type: str
    dimension_name: str
    source_type: str
    source_path: str | None
    value_type: str
    transform_expression: str | None
    default_value: str | None
    priority: int
    is_enabled: bool


def _normalize_api_format(api_format: str | None) -> str:
    if not api_format:
        return ""
    from src.core.api_format.signature import normalize_signature_key

    return normalize_signature_key(api_format)


def _normalize_task_type(task_type: str | None) -> str:
    return (task_type or "").lower()


def _get_nested_value(data: Any, path: str) -> Any:
    """
    简单 JSON path：
    - a.b.c
    - 列表索引用数字：items.0.id
    """
    if data is None or path is None or path == "":
        return None

    value: Any = data
    for key in path.split("."):
        if isinstance(value, dict):
            value = value.get(key)
        elif isinstance(value, list):
            if not key.isdigit():
                return None
            idx = int(key)
            if idx < 0 or idx >= len(value):
                return None
            value = value[idx]
        else:
            return None
        if value is None:
            return None
    return value


def _cast_value(value: Any, value_type: ValueType) -> Any:
    if value_type == "string":
        return "" if value is None else str(value)
    if value_type == "int":
        if value is None:
            return 0
        if isinstance(value, bool):
            raise ValueError("bool is not a valid int dimension value")
        return int(float(value))
    # float
    if value is None:
        return 0.0
    if isinstance(value, bool):
        raise ValueError("bool is not a valid float dimension value")
    return float(value)


def _type_default(value_type: ValueType) -> Any:
    return "" if value_type == "string" else (0 if value_type == "int" else 0.0)


_WXH_PATTERN = re.compile(r"^(\d+)x(\d+)$")


def _normalize_resolution_key(raw: str) -> str:
    """
    Normalize resolution key:
    - lowercase, remove spaces, × → x
    - For WxH format, sort dimensions so smaller comes first (1080x720 → 720x1080)
    """
    k = (raw or "").strip().lower().replace(" ", "").replace("×", "x")
    match = _WXH_PATTERN.match(k)
    if match:
        a, b = int(match.group(1)), int(match.group(2))
        k = f"{a}x{b}" if a <= b else f"{b}x{a}"
    return k


@dataclass(frozen=True)
class DimensionCollectInput:
    request: dict[str, Any] | None = None
    response: dict[str, Any] | None = None
    metadata: dict[str, Any] | None = None
    base_dimensions: dict[str, Any] | None = None


class DimensionCollectorRuntime:
    """纯运行时逻辑（不依赖 DB），便于测试与复用。"""

    def __init__(self) -> None:
        self._evaluator = SafeExpressionEvaluator()

    def collect(
        self,
        *,
        collectors: list[CollectorLike],
        inp: DimensionCollectInput,
    ) -> dict[str, Any]:
        dims: dict[str, Any] = dict(inp.base_dimensions or {})

        # dimension_name -> collectors (priority desc)
        grouped: dict[str, list[CollectorLike]] = {}
        for c in collectors:
            grouped.setdefault(c.dimension_name, []).append(c)
        for name in grouped:
            grouped[name].sort(key=lambda x: (x.priority or 0), reverse=True)

        # 1) 先收集非 computed
        computed_only: set[str] = set()
        for dim_name, cs in grouped.items():
            non_computed = [c for c in cs if (c.source_type or "").lower() != "computed"]
            if not non_computed:
                computed_only.add(dim_name)
                continue
            value = self._resolve_dimension(dim_name, non_computed, dims, inp)
            dims[dim_name] = value

        # 2) computed 维度拓扑排序
        ordered = self._toposort_computed(grouped, computed_only)
        for dim_name in ordered:
            cs = [
                c for c in grouped.get(dim_name, []) if (c.source_type or "").lower() == "computed"
            ]
            if not cs:
                continue
            cs.sort(key=lambda x: (x.priority or 0), reverse=True)
            value = self._resolve_computed_dimension(dim_name, cs, dims)
            dims[dim_name] = value

        return dims

    def _resolve_dimension(
        self,
        dim_name: str,
        collectors: list[CollectorLike],
        dims: dict[str, Any],
        inp: DimensionCollectInput,
    ) -> Any:
        fallback_default: str | None = None
        fallback_value_type: ValueType | None = None
        value_type: ValueType = (
            (collectors[0].value_type or "float").lower()  # type: ignore[assignment]
            if collectors
            else "float"
        )

        for c in collectors:
            value_type = (c.value_type or "float").lower()  # type: ignore[assignment]
            if c.default_value is not None and fallback_default is None:
                fallback_default = c.default_value
                fallback_value_type = value_type

            src = (c.source_type or "").lower()
            path = c.source_path or ""

            if src == "request":
                raw = _get_nested_value(inp.request or {}, path)
            elif src == "response":
                raw = _get_nested_value(inp.response or {}, path)
            elif src == "metadata":
                raw = _get_nested_value(inp.metadata or {}, path)
            else:
                # 未知 source：跳过尝试
                continue

            if raw is None:
                continue

            try:
                value: Any = raw
                if c.transform_expression:
                    # transform_expression 仅允许使用 value
                    value = self._evaluator.eval_number(c.transform_expression, {"value": value})
                casted = _cast_value(value, value_type)
                return casted
            except (ValueError, UnsafeExpressionError, ExpressionEvaluationError, Exception) as exc:
                # 注意：这里选择“不中断，尝试下一优先级”
                logger.debug(
                    "Dimension collector failed (dim={}, id={}): {}",
                    dim_name,
                    getattr(c, "id", None),
                    str(exc),
                )
                continue

        # 兜底：default_value（仅允许配置一条，但这里不依赖 DB 校验）
        if fallback_default is not None:
            try:
                return _cast_value(fallback_default, fallback_value_type or value_type)
            except Exception:
                return _type_default(fallback_value_type or value_type)

        return _type_default(value_type)

    def _resolve_computed_dimension(
        self,
        dim_name: str,
        collectors: list[CollectorLike],
        dims: dict[str, Any],
    ) -> Any:
        fallback_default: str | None = None
        fallback_value_type: ValueType | None = None
        value_type: ValueType = (
            (collectors[0].value_type or "float").lower()  # type: ignore[assignment]
            if collectors
            else "float"
        )

        for c in collectors:
            value_type = (c.value_type or "float").lower()  # type: ignore[assignment]
            if c.default_value is not None and fallback_default is None:
                fallback_default = c.default_value
                fallback_value_type = value_type

            expr = c.transform_expression
            if not expr:
                continue

            try:
                value = self._evaluator.eval_number(expr, dims)
                casted = _cast_value(value, value_type)
                return casted
            except (ValueError, ExpressionEvaluationError, UnsafeExpressionError, Exception):
                continue

        if fallback_default is not None:
            try:
                return _cast_value(fallback_default, fallback_value_type or value_type)
            except Exception:
                return _type_default(fallback_value_type or value_type)

        return _type_default(value_type)

    def _toposort_computed(
        self,
        grouped: dict[str, list[CollectorLike]],
        computed_only: set[str],
    ) -> list[str]:
        # 建图：dependency -> dim
        allowed_func_names = set(self._evaluator.ALLOWED_FUNCS.keys())

        deps: dict[str, set[str]] = {d: set() for d in computed_only}
        for dim_name in computed_only:
            for c in grouped.get(dim_name, []):
                if (c.source_type or "").lower() != "computed" or not c.transform_expression:
                    continue
                try:
                    names = extract_variable_names(c.transform_expression)
                except UnsafeExpressionError:
                    # 配置错误：按无依赖处理，避免阻塞
                    logger.error(
                        "Invalid computed transform_expression (dim={}, id={})",
                        dim_name,
                        getattr(c, "id", None),
                    )
                    names = set()
                names.discard("value")
                names -= allowed_func_names
                # 仅关心依赖的 computed 维度（非 computed 会在前一步收集）
                deps[dim_name] |= {n for n in names if n in computed_only and n != dim_name}

        # Kahn
        in_degree: dict[str, int] = {d: 0 for d in computed_only}
        forward: dict[str, set[str]] = {d: set() for d in computed_only}
        for dim_name, dim_deps in deps.items():
            for dep in dim_deps:
                forward[dep].add(dim_name)
                in_degree[dim_name] += 1

        queue = deque(sorted(d for d, deg in in_degree.items() if deg == 0))
        ordered: list[str] = []

        while queue:
            node = queue.popleft()
            ordered.append(node)
            for nxt in sorted(forward.get(node, set())):
                in_degree[nxt] -= 1
                if in_degree[nxt] == 0:
                    queue.append(nxt)

        if len(ordered) != len(computed_only):
            # 有环依赖：保护性降级（按名称补齐），避免阻塞整条计费链路
            remaining = sorted(list(computed_only - set(ordered)))
            logger.error("Computed dimension cycle detected: {}", remaining)
            ordered.extend(remaining)

        return ordered


class DimensionCollectorService:
    """运行时读取 collectors 并执行采集（code-only）。"""

    def __init__(self, db: Session):
        self.db = db
        self._runtime = DimensionCollectorRuntime()

    def list_enabled_collectors(
        self,
        *,
        api_format: str | None,
        task_type: str | None,
    ) -> list[CollectorLike]:
        api = _normalize_api_format(api_format)
        task = _normalize_task_type(task_type)
        if not api or not task:
            return []

        # Code-defined collectors cache.
        cache_key = f"code:{api}:{task}"
        cached = BillingCache.get_collectors(cache_key)
        if cached is not None:
            return cached

        built = self._list_builtin_collectors(api_format=api_format, task_type=task_type)
        BillingCache.set_collectors(cache_key, built)
        return built

    def _list_builtin_collectors(
        self,
        *,
        api_format: str | None,
        task_type: str | None,
    ) -> list[DimensionCollector]:
        """
        Built-in (code) collectors.

        Developers ship a curated set of collectors in code (config-file mode).
        """
        api = _normalize_api_format(api_format)
        task = _normalize_task_type(task_type)
        if not api or not task:
            return []

        def _preset_query(api_keys: list[str], task_t: str) -> list[DimensionCollector]:
            out: list[DimensionCollector] = []
            allowed = {k for k in api_keys if k}
            for p in CORE_PRESET_PACK.collectors:
                if not p.is_enabled:
                    continue
                if _normalize_api_format(p.api_format) not in allowed:
                    continue
                if _normalize_task_type(p.task_type) != task_t:
                    continue
                out.append(
                    DimensionCollector(
                        api_format=_normalize_api_format(p.api_format),
                        task_type=_normalize_task_type(p.task_type),
                        dimension_name=p.dimension_name,
                        source_type=p.source_type,
                        source_path=p.source_path,
                        value_type=p.value_type,
                        transform_expression=p.transform_expression,
                        default_value=p.default_value,
                        priority=int(p.priority or 0),
                        is_enabled=True,
                    )
                )
            return out

        api_variants = list({api, api.lower()})

        if task == "video":
            from src.core.api_format.signature import parse_signature_key

            base_api = api
            try:
                sig = parse_signature_key(api)
                if sig.endpoint_kind.value == "video":
                    base_api = f"{sig.api_family.value}:chat"
            except Exception:
                base_api = api
            base_variants = list({base_api, base_api.lower()})

            video_collectors = _preset_query(api_variants, "video")
            base_collectors = _preset_query(base_variants, "video")
            video_dims: set[str] = {c.dimension_name for c in video_collectors}
            result: list[DimensionCollector] = list(video_collectors)
            for c in base_collectors:
                if c.dimension_name not in video_dims:
                    result.append(c)
            return result

        if task == "cli":
            cli_collectors = _preset_query(api_variants, "cli")
            chat_collectors = _preset_query(api_variants, "chat")
            cli_dims: set[str] = {c.dimension_name for c in cli_collectors}
            result: list[DimensionCollector] = list(cli_collectors)
            for c in chat_collectors:
                if c.dimension_name not in cli_dims:
                    result.append(c)
            return result

        if task == "image":
            # image 走 token 计费时可直接复用 chat 的 input/output_tokens 采集器。
            # 用户只配 chat 的 model 定价，image 也能算出成本。
            from src.core.api_format.signature import parse_signature_key

            base_api = api
            try:
                sig = parse_signature_key(api)
                if sig.endpoint_kind.value == "image":
                    base_api = f"{sig.api_family.value}:chat"
            except Exception:
                base_api = api
            base_variants = list({base_api, base_api.lower()})

            image_collectors = _preset_query(api_variants, "image")
            base_collectors = _preset_query(base_variants, "chat")
            image_dims: set[str] = {c.dimension_name for c in image_collectors}
            result = list(image_collectors)
            for c in base_collectors:
                if c.dimension_name not in image_dims:
                    result.append(c)
            return result

        return _preset_query(api_variants, task)

    def collect_dimensions(
        self,
        *,
        api_format: str | None,
        task_type: str | None,
        request: dict[str, Any] | None = None,
        response: dict[str, Any] | None = None,
        metadata: dict[str, Any] | None = None,
        base_dimensions: dict[str, Any] | None = None,
    ) -> dict[str, Any]:
        collectors = self.list_enabled_collectors(api_format=api_format, task_type=task_type)
        dims = self._runtime.collect(
            collectors=collectors,
            inp=DimensionCollectInput(
                request=request,
                response=response,
                metadata=metadata,
                base_dimensions=base_dimensions,
            ),
        )
        # Post-process: normalize video_resolution_key (e.g., 1080x720 → 720x1080)
        if "video_resolution_key" in dims:
            raw = dims["video_resolution_key"]
            if isinstance(raw, str) and raw:
                dims["video_resolution_key"] = _normalize_resolution_key(raw)
        return dims
