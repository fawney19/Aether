"""
FormulaEngine - 配置驱动的安全计费表达式引擎

目标：
- 支持 billing_rules.expression 的安全求值（AST 白名单）
- 支持 dimension_mappings（dimension/matrix/tiered/constant）
- 支持 required/allow_zero 机制，避免维度缺失导致静默少收

注意：该模块不直接依赖数据库；规则查找、维度采集在上层服务完成。
"""

from __future__ import annotations

import ast
from dataclasses import dataclass, field
from decimal import Decimal
from functools import lru_cache
from typing import Any, Iterable, Literal

from src.core.logger import logger
from src.services.billing.precision import DECIMAL_CONTEXT_PRECISION, to_decimal


class UnsafeExpressionError(ValueError):
    """表达式包含不安全/不支持的 AST 结构。"""


class ExpressionEvaluationError(RuntimeError):
    """表达式在安全求值阶段失败（如 NameError/ZeroDivision）。"""


class BillingIncompleteError(RuntimeError):
    """required 维度缺失且 strict_mode=true 时抛出，用于上层拒绝请求/标记任务失败。"""

    def __init__(self, message: str, *, missing_required: list[str]):
        super().__init__(message)
        self.missing_required = missing_required


@dataclass(frozen=True)
class FormulaEvaluationResult:
    status: Literal["complete", "incomplete"]
    cost: Decimal
    resolved_dimensions: dict[str, Any]
    resolved_variables: dict[str, Any]
    cost_breakdown: dict[str, Decimal] = field(default_factory=dict)
    tier_index: int | None = None
    tier_info: dict[str, Any] | None = None
    missing_required: list[str] = field(default_factory=list)
    error: str | None = None


_ALLOWED_BINOPS = (
    ast.Add,
    ast.Sub,
    ast.Mult,
    ast.Div,
    ast.Pow,
    ast.FloorDiv,
    ast.Mod,
)
_ALLOWED_UNARYOPS = (ast.UAdd, ast.USub)
_ALLOWED_OP_NODES = _ALLOWED_BINOPS + _ALLOWED_UNARYOPS

# Allowed function names used in expressions.
_ALLOWED_FUNC_NAMES = frozenset(("min", "max", "abs", "round", "int", "float"))


def _iter_ast_nodes(node: ast.AST) -> Iterable[ast.AST]:
    yield node
    for child in ast.iter_child_nodes(node):
        yield from _iter_ast_nodes(child)


@lru_cache(maxsize=256)
def _validate_expression_cached(expression: str) -> ast.Expression:
    """
    Parse + validate an expression and cache the resulting AST.

    This is a hot path (called for every billing evaluation and many collector transforms),
    so we cache validated ASTs to avoid repeated ast.parse + whitelist scans.
    """
    try:
        tree = ast.parse(expression, mode="eval")
    except SyntaxError as exc:
        raise UnsafeExpressionError(f"Invalid expression syntax: {exc}") from exc

    for node in _iter_ast_nodes(tree):
        if isinstance(node, ast.Expression):
            continue
        # 运算符节点本身也会出现在 iter_child_nodes 中
        if isinstance(node, _ALLOWED_OP_NODES):
            continue
        if isinstance(node, ast.Constant):
            # 仅允许数字常量（bool 是 int 子类，需要显式排除）
            if isinstance(node.value, bool) or not isinstance(node.value, (int, float)):
                raise UnsafeExpressionError("Only int/float constants are allowed")
            continue
        if isinstance(node, ast.BinOp):
            if not isinstance(node.op, _ALLOWED_BINOPS):
                raise UnsafeExpressionError(f"Operator not allowed: {type(node.op).__name__}")
            continue
        if isinstance(node, ast.UnaryOp):
            if not isinstance(node.op, _ALLOWED_UNARYOPS):
                raise UnsafeExpressionError(f"Unary operator not allowed: {type(node.op).__name__}")
            continue
        if isinstance(node, ast.Name):
            # 防御：拒绝双下划线变量名
            if node.id.startswith("__"):
                raise UnsafeExpressionError("Dunder names are not allowed")
            continue
        if isinstance(node, ast.Load):
            continue
        if isinstance(node, ast.keyword):
            continue
        if isinstance(node, ast.Call):
            if not isinstance(node.func, ast.Name):
                raise UnsafeExpressionError("Only direct function calls are allowed")
            func_name = node.func.id
            if func_name not in _ALLOWED_FUNC_NAMES:
                raise UnsafeExpressionError(f"Function not allowed: {func_name}")
            if any(k.arg is None for k in node.keywords):
                raise UnsafeExpressionError("**kwargs is not allowed")
            continue

        # 明确禁止的/不需要的节点类型（属性访问、下标、推导式、比较等）
        if isinstance(
            node,
            (
                ast.Attribute,
                ast.Subscript,
                ast.Compare,
                ast.BoolOp,
                ast.IfExp,
                ast.Lambda,
                ast.Dict,
                ast.List,
                ast.Tuple,
                ast.Set,
                ast.ListComp,
                ast.SetComp,
                ast.DictComp,
                ast.GeneratorExp,
                ast.Await,
                ast.Yield,
                ast.YieldFrom,
            ),
        ):
            raise UnsafeExpressionError(f"AST node not allowed: {type(node).__name__}")

        raise UnsafeExpressionError(f"AST node not allowed: {type(node).__name__}")

    assert isinstance(tree, ast.Expression)
    return tree


def extract_variable_names(expression: str) -> set[str]:
    """提取表达式中出现的变量名（不含函数名）。"""
    tree = _validate_expression_cached(expression)

    names: set[str] = set()
    for node in _iter_ast_nodes(tree):
        if isinstance(node, ast.Name):
            names.add(node.id)
        if isinstance(node, ast.Call):
            # Call 的函数名会以 ast.Name 出现，需要从结果中过滤掉
            if isinstance(node.func, ast.Name):
                names.discard(node.func.id)
    return names


class SafeExpressionEvaluator:
    """AST 白名单 + 无 builtins 的安全求值器。"""

    def __init__(self) -> None:
        # Decimal-friendly allowed functions (return Decimal)
        self.ALLOWED_FUNCS: dict[str, Any] = {
            "min": self._min,
            "max": self._max,
            "abs": self._abs,
            "round": self._round,
            "int": self._int,
            "float": self._float,
        }

    @staticmethod
    def _min(*args: Any) -> Decimal:
        return min(to_decimal(a) for a in args)

    @staticmethod
    def _max(*args: Any) -> Decimal:
        return max(to_decimal(a) for a in args)

    @staticmethod
    def _abs(x: Any) -> Decimal:
        return abs(to_decimal(x))

    @staticmethod
    def _round(x: Any, ndigits: Any = 0) -> Decimal:
        # Round Decimal returns Decimal; coerce ndigits to int safely.
        try:
            n = int(ndigits)
        except Exception:
            n = 0
        return round(to_decimal(x), n)

    @staticmethod
    def _int(x: Any) -> Decimal:
        return to_decimal(int(to_decimal(x)))

    @staticmethod
    def _float(x: Any) -> Decimal:
        # Keep numeric chain in Decimal even if caller used float()
        return to_decimal(float(to_decimal(x)))

    def validate(self, expression: str) -> ast.Expression:
        return _validate_expression_cached(expression)

    def eval_decimal(self, expression: str, variables: dict[str, Any]) -> Decimal:
        """
        Evaluate expression into Decimal.

        We avoid Python eval() here to ensure:
        - float literals don't leak binary float arithmetic
        - all arithmetic stays within Decimal
        """
        tree = self.validate(expression)
        try:
            with _decimal_context(DECIMAL_CONTEXT_PRECISION):
                return _eval_decimal(tree.body, variables or {}, self.ALLOWED_FUNCS)
        except NameError:
            raise
        except ExpressionEvaluationError:
            raise
        except Exception as exc:
            raise ExpressionEvaluationError(str(exc)) from exc

    def eval_number(self, expression: str, variables: dict[str, Any]) -> float:
        """Backward-compatible float evaluation (used by DimensionCollector transforms)."""
        value = self.eval_decimal(expression, variables)
        try:
            return float(value)
        except Exception as exc:
            raise ExpressionEvaluationError(f"Expression result is not numeric: {value!r}") from exc


class _decimal_context:
    def __init__(self, prec: int):
        self.prec = prec

    def __enter__(self) -> None:
        from decimal import getcontext

        self._ctx = getcontext().copy()
        getcontext().prec = self.prec

    def __exit__(self, exc_type: type | None, exc: BaseException | None, tb: Any) -> None:
        from decimal import setcontext

        # Restore full context to avoid leaking settings.
        setcontext(self._ctx)


def _eval_decimal(node: ast.AST, variables: dict[str, Any], funcs: dict[str, Any]) -> Decimal:
    if isinstance(node, ast.Constant):
        return to_decimal(node.value)
    if isinstance(node, ast.Name):
        if node.id not in variables:
            raise NameError(node.id)
        return to_decimal(variables[node.id])
    if isinstance(node, ast.UnaryOp):
        v = _eval_decimal(node.operand, variables, funcs)
        if isinstance(node.op, ast.UAdd):
            return v
        if isinstance(node.op, ast.USub):
            return -v
        raise ExpressionEvaluationError(f"Unary operator not allowed: {type(node.op).__name__}")
    if isinstance(node, ast.BinOp):
        left = _eval_decimal(node.left, variables, funcs)
        right = _eval_decimal(node.right, variables, funcs)
        if isinstance(node.op, ast.Add):
            return left + right
        if isinstance(node.op, ast.Sub):
            return left - right
        if isinstance(node.op, ast.Mult):
            return left * right
        if isinstance(node.op, ast.Div):
            return left / right
        if isinstance(node.op, ast.FloorDiv):
            return left // right
        if isinstance(node.op, ast.Mod):
            return left % right
        if isinstance(node.op, ast.Pow):
            # Decimal power is only well-defined for integer exponents here.
            try:
                exp_int = int(right)
                if to_decimal(exp_int) != right:
                    raise ValueError("non-integer exponent")
            except Exception as exc:
                raise ExpressionEvaluationError("Pow only supports integer exponents") from exc
            return left**exp_int
        raise ExpressionEvaluationError(f"Operator not allowed: {type(node.op).__name__}")
    if isinstance(node, ast.Call):
        if not isinstance(node.func, ast.Name):
            raise ExpressionEvaluationError("Only direct function calls are allowed")
        func_name = node.func.id
        func = funcs.get(func_name)
        if func is None:
            raise ExpressionEvaluationError(f"Function not allowed: {func_name}")
        args = [_eval_decimal(a, variables, funcs) for a in node.args]
        kwargs = {
            kw.arg: _eval_decimal(kw.value, variables, funcs) for kw in node.keywords if kw.arg
        }
        try:
            result = func(*args, **kwargs)
        except Exception as exc:
            raise ExpressionEvaluationError(str(exc)) from exc
        return to_decimal(result)

    raise ExpressionEvaluationError(f"AST node not allowed: {type(node).__name__}")


class FormulaEngine:
    """计费表达式引擎：解析 dimension_mappings 并进行安全求值。"""

    def __init__(self) -> None:
        self._evaluator = SafeExpressionEvaluator()

    def evaluate(
        self,
        *,
        expression: str,
        variables: dict[str, Any] | None,
        dimensions: dict[str, Any] | None,
        dimension_mappings: dict[str, dict[str, Any]] | None,
        strict_mode: bool = False,
    ) -> FormulaEvaluationResult:
        dims = dimensions or {}
        mappings = dimension_mappings or {}
        resolved: dict[str, Any] = dict(variables or {})

        missing_required: list[str] = []
        tier_index: int | None = None
        tier_info: dict[str, Any] | None = None

        computed: dict[str, dict[str, Any]] = {}

        # 1) Resolve non-computed mappings first
        for var_name, mapping in mappings.items():
            source = (mapping.get("source") or "constant").lower()
            if source == "computed":
                computed[var_name] = mapping
                continue
            # Explicit constant mapping is fallback-only when variable already exists.
            if source == "constant" and var_name in resolved:
                continue
            value, is_missing, tier_meta = self._resolve_mapping(var_name, mapping, dims)
            if tier_meta and tier_index is None:
                tier_index = tier_meta.get("tier_index")
                tier_info = tier_meta.get("tier_info")
            if is_missing:
                missing_required.append(var_name)
                continue
            resolved[var_name] = value

        # 2) Resolve computed mappings (iterative dependency resolution)
        if computed:
            unresolved = dict(computed)
            for _ in range(max(4, len(unresolved) + 1)):
                progressed = False
                for var_name, mapping in list(unresolved.items()):
                    if var_name in resolved:
                        unresolved.pop(var_name, None)
                        continue
                    value, status = self._try_resolve_computed(var_name, mapping, dims, resolved)
                    if status == "pending":
                        continue
                    unresolved.pop(var_name, None)
                    if status == "missing_required":
                        missing_required.append(var_name)
                        continue
                    if status == "error":
                        # 求值异常但非 required，使用 default 值继续
                        resolved[var_name] = value
                        progressed = True
                        continue
                    resolved[var_name] = value
                    progressed = True
                if not progressed:
                    break

            # any remaining unresolved computed vars
            for var_name, mapping in unresolved.items():
                required = bool(mapping.get("required", False))
                default = mapping.get("default", 0)
                logger.warning(
                    "[FormulaEngine] computed 维度 '{}' 在迭代后仍未解析, required={}",
                    var_name,
                    required,
                )
                if required:
                    missing_required.append(var_name)
                else:
                    resolved[var_name] = default

        # required 维度缺失：直接标记 incomplete（并由 strict_mode 决定是否抛错）
        if missing_required:
            if strict_mode:
                raise BillingIncompleteError(
                    f"Missing required dimensions: {missing_required}",
                    missing_required=missing_required,
                )
            return FormulaEvaluationResult(
                status="incomplete",
                cost=Decimal("0"),
                resolved_dimensions=dims,
                resolved_variables=resolved,
                missing_required=missing_required,
                tier_index=tier_index,
                tier_info=tier_info,
            )

        # 3) Evaluate total cost
        try:
            cost = self._evaluator.eval_decimal(expression, resolved)
            if cost < 0:
                return FormulaEvaluationResult(
                    status="incomplete",
                    cost=Decimal("0"),
                    resolved_dimensions=dims,
                    resolved_variables=resolved,
                    missing_required=[],
                    tier_index=tier_index,
                    tier_info=tier_info,
                    error="negative_cost",
                )

            breakdown = self._extract_cost_breakdown(resolved)
            return FormulaEvaluationResult(
                status="complete",
                cost=cost,
                resolved_dimensions=dims,
                resolved_variables=resolved,
                cost_breakdown=breakdown,
                tier_index=tier_index,
                tier_info=tier_info,
                missing_required=[],
            )
        except NameError as exc:
            # expression references missing vars
            if strict_mode:
                raise ExpressionEvaluationError(f"Missing variable: {exc}") from exc
            return FormulaEvaluationResult(
                status="incomplete",
                cost=Decimal("0"),
                resolved_dimensions=dims,
                resolved_variables=resolved,
                missing_required=[],
                tier_index=tier_index,
                tier_info=tier_info,
                error=f"missing_variable:{exc}",
            )
        except (UnsafeExpressionError, ExpressionEvaluationError) as exc:
            if strict_mode:
                raise
            return FormulaEvaluationResult(
                status="incomplete",
                cost=Decimal("0"),
                resolved_dimensions=dims,
                resolved_variables=resolved,
                missing_required=[],
                tier_index=tier_index,
                tier_info=tier_info,
                error=str(exc),
            )

    def _extract_cost_breakdown(self, resolved: dict[str, Any]) -> dict[str, Decimal]:
        breakdown: dict[str, Decimal] = {}
        for k, v in resolved.items():
            if not k.endswith("_cost"):
                continue
            try:
                breakdown[k] = to_decimal(v)
            except Exception:
                continue
        return breakdown

    def _try_resolve_computed(
        self,
        var_name: str,
        mapping: dict[str, Any],
        dims: dict[str, Any],
        resolved: dict[str, Any],
    ) -> tuple[Any, Literal["ok", "pending", "missing_required"]]:
        """
        Try resolve a computed mapping.

        Returns:
            (value, status)
            - ok: value computed
            - pending: missing dependencies, retry later
            - missing_required: required=true and cannot resolve
        """
        required = bool(mapping.get("required", False))
        default = mapping.get("default", 0)
        expr = mapping.get("expression") or mapping.get("transform_expression")
        if not expr:
            return (None, "missing_required") if required else (default, "ok")
        # Computed vars can reference both resolved variables and raw dims.
        env: dict[str, Any] = {}
        env.update(dims)
        env.update(resolved)
        try:
            value = self._evaluator.eval_decimal(str(expr), env)
            return value, "ok"
        except NameError:
            # dependency not ready yet
            return (None, "pending") if required else (default, "pending")
        except Exception as exc:
            logger.warning(
                "[FormulaEngine] computed 维度 '{}' 求值异常: {}, expression={!r}",
                var_name,
                exc,
                expr,
            )
            return (None, "missing_required") if required else (default, "error")

    def _resolve_mapping(
        self,
        var_name: str,
        mapping: dict[str, Any],
        dims: dict[str, Any],
    ) -> tuple[Any, bool, dict[str, Any] | None]:
        """
        Returns:
            (value, is_missing_required, tier_meta)

        说明：
        - is_missing_required 仅在 required=true 且缺失时为 True
        - required=false 的缺失会使用 default 或 0 兜底，并返回 is_missing_required=False
        """
        source = (mapping.get("source") or "constant").lower()

        if source == "constant":
            return self._resolve_constant(mapping)
        if source == "dimension":
            return self._resolve_dimension(var_name, mapping, dims)
        if source == "matrix":
            return self._resolve_matrix(var_name, mapping, dims)
        if source == "tiered":
            return self._resolve_tiered(var_name, mapping, dims)
        # 未知 source：视为配置错误，但不直接中断计费（返回 default）
        return mapping.get("default", 0), False, None

    @staticmethod
    def _resolve_constant(
        mapping: dict[str, Any],
    ) -> tuple[Any, bool, dict[str, Any] | None]:
        """constant 默认行为：由 variables 提供；dimension_mappings 显式 constant 时仅做兜底"""
        return mapping.get("default", 0), False, None

    @staticmethod
    def _resolve_dimension(
        var_name: str,
        mapping: dict[str, Any],
        dims: dict[str, Any],
    ) -> tuple[Any, bool, dict[str, Any] | None]:
        """解析 dimension source：从 dims 中取值并尝试转换为 Decimal"""
        required = bool(mapping.get("required", False))
        allow_zero = bool(mapping.get("allow_zero", False))
        default = mapping.get("default", 0)

        def _missing() -> tuple[Any, bool]:
            if required:
                return None, True
            return default, False

        key = mapping.get("key") or var_name
        raw = dims.get(key)
        if raw is None:
            v, m = _missing()
            return v, m, None
        if isinstance(raw, str):
            if raw == "":
                v, m = _missing()
                return v, m, None
            try:
                num = to_decimal(raw)
                if num == 0 and not allow_zero:
                    v, m = _missing()
                    return v, m, None
                return num, False, None
            except Exception:
                return raw, False, None
        if isinstance(raw, (int, float, Decimal)):
            num = to_decimal(raw)
            if num == 0 and not allow_zero:
                v, m = _missing()
                return v, m, None
            return num, False, None
        try:
            num = to_decimal(raw)
            if num == 0 and not allow_zero:
                v, m = _missing()
                return v, m, None
            return num, False, None
        except Exception:
            v, m = _missing()
            return v, m, None

    @staticmethod
    def _resolve_matrix(
        var_name: str,
        mapping: dict[str, Any],
        dims: dict[str, Any],
    ) -> tuple[Any, bool, dict[str, Any] | None]:
        """解析 matrix source：从 map 中按 key 查找值"""
        required = bool(mapping.get("required", False))
        default = mapping.get("default", 0)

        def _missing() -> tuple[Any, bool]:
            if required:
                return None, True
            return default, False

        key = mapping.get("key") or var_name
        raw = dims.get(key)
        if raw is None or raw == "":
            v, m = _missing()
            return v, m, None
        raw_key = str(raw)
        matrix = mapping.get("map") or {}
        if raw_key in matrix:
            try:
                return to_decimal(matrix[raw_key]), False, None
            except Exception:
                return matrix[raw_key], False, None
        if required:
            return None, True, None
        return default, False, None

    def _resolve_tiered(
        self,
        var_name: str,
        mapping: dict[str, Any],
        dims: dict[str, Any],
    ) -> tuple[Any, bool, dict[str, Any] | None]:
        """解析 tiered source：按阶梯匹配值"""
        required = bool(mapping.get("required", False))
        allow_zero = bool(mapping.get("allow_zero", False))
        default = mapping.get("default", 0)

        def _missing() -> tuple[Any, bool]:
            if required:
                return None, True
            return default, False

        tier_key = mapping.get("tier_key")
        if not tier_key:
            v, m = _missing()
            return v, m, None
        raw_tier_value = dims.get(tier_key)
        if raw_tier_value is None:
            v, m = _missing()
            return v, m, None
        try:
            tier_value = to_decimal(raw_tier_value)
        except Exception:
            v, m = _missing()
            return v, m, None

        if tier_value == 0 and not allow_zero:
            v, m = _missing()
            return v, m, None

        # Optional TTL override (legacy: Claude cache pricing)
        ttl_key = mapping.get("ttl_key")
        ttl_value_key = mapping.get("ttl_value_key")
        ttl_minutes: Decimal | None = None
        if ttl_key and ttl_value_key and dims.get(ttl_key) is not None:
            try:
                ttl_minutes = to_decimal(dims.get(ttl_key))
            except Exception:
                ttl_minutes = None

        tiers = mapping.get("tiers") or []
        # tiers: [{up_to: 128000, value: 2.5}, {up_to: null, value: 1.25}]
        for idx, tier in enumerate(tiers):
            up_to = tier.get("up_to")
            if up_to is None:
                value = to_decimal(tier.get("value", default))
                if (
                    ttl_minutes is not None
                    and ttl_value_key
                    and isinstance(tier.get("cache_ttl_pricing"), list)
                ):
                    value = self._resolve_ttl_pricing(
                        tier.get("cache_ttl_pricing") or [],
                        ttl_minutes,
                        str(ttl_value_key),
                        fallback=value,
                    )
                return value, False, {"tier_index": idx, "tier_info": dict(tier)}
            try:
                if tier_value <= to_decimal(up_to):
                    value = to_decimal(tier.get("value", default))
                    if (
                        ttl_minutes is not None
                        and ttl_value_key
                        and isinstance(tier.get("cache_ttl_pricing"), list)
                    ):
                        value = self._resolve_ttl_pricing(
                            tier.get("cache_ttl_pricing") or [],
                            ttl_minutes,
                            str(ttl_value_key),
                            fallback=value,
                        )
                    return value, False, {"tier_index": idx, "tier_info": dict(tier)}
            except Exception:
                # up_to 配置异常：忽略并继续
                continue
        # 无匹配：使用最后一个或 default
        if tiers:
            last = tiers[-1]
            value = to_decimal(last.get("value", default))
            if (
                ttl_minutes is not None
                and ttl_value_key
                and isinstance(last.get("cache_ttl_pricing"), list)
            ):
                value = self._resolve_ttl_pricing(
                    last.get("cache_ttl_pricing") or [],
                    ttl_minutes,
                    str(ttl_value_key),
                    fallback=value,
                )
            return value, False, {"tier_index": len(tiers) - 1, "tier_info": dict(last)}
        return default, False, None

    def _resolve_ttl_pricing(
        self,
        ttl_pricing: list[Any],
        ttl_minutes: Decimal,
        ttl_value_key: str,
        *,
        fallback: Decimal,
    ) -> Decimal:
        """
        Resolve TTL-dependent pricing (legacy: cache_ttl_pricing).

        Rules:
        - exact match wins
        - otherwise use the nearest configured entry whose ttl_minutes <= requested ttl
        - if the requested ttl is below every configured override, fallback to the base tier price
        - if missing/invalid, fallback to base value
        """
        try:
            entries = [e for e in ttl_pricing if isinstance(e, dict)]
            if not entries:
                return fallback

            def _ttl_key(e: dict[str, Any]) -> Decimal:
                return to_decimal(e.get("ttl_minutes") or 0)

            entries_sorted = sorted(entries, key=_ttl_key)

            for e in entries_sorted:
                try:
                    if ttl_minutes == to_decimal(e.get("ttl_minutes") or 0):
                        v = e.get(ttl_value_key)
                        if v is None:
                            return fallback
                        return to_decimal(v)
                except Exception:
                    continue

            chosen: dict[str, Any] | None = None
            for e in entries_sorted:
                try:
                    if to_decimal(e.get("ttl_minutes") or 0) <= ttl_minutes:
                        chosen = e
                except Exception:
                    continue

            if chosen is None:
                return fallback

            v = chosen.get(ttl_value_key)
            if v is None:
                return fallback
            return to_decimal(v)
        except Exception:
            return fallback
