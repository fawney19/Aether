"""
格式转换注册表（Canonical / Hub-and-Spoke）

实现路径：
source -> internal -> target

说明：
- 旧 N×N converters 已移除；这里是唯一的格式转换实现。
- 转换失败将抛出 `FormatConversionError`（不再静默回退）。
"""

import ast
import copy
import importlib
import inspect
import threading
import time
from collections.abc import Generator
from contextlib import contextmanager
from pathlib import Path
from typing import Any

from src.core.api_format.conversion.exceptions import FormatConversionError
from src.core.api_format.conversion.image_resolver import resolve_image_urls
from src.core.api_format.conversion.internal import InternalRequest, ToolResultBlock, ToolUseBlock
from src.core.api_format.conversion.normalizer import FormatNormalizer
from src.core.api_format.conversion.stream_state import StreamState
from src.core.logger import logger
from src.core.metrics import format_conversion_duration_seconds, format_conversion_total


@contextmanager
def _track_conversion_metrics(
    direction: str,
    source: str,
    target: str,
) -> Generator[None]:
    start = time.perf_counter()
    try:
        yield
        format_conversion_total.labels(direction, source, target, "success").inc()
    except Exception:
        format_conversion_total.labels(direction, source, target, "error").inc()
        raise
    finally:
        format_conversion_duration_seconds.labels(direction, source, target).observe(
            time.perf_counter() - start
        )


_MATERIALIZING: tuple[str, str] = ("__materializing__", "")


class FormatConversionRegistry:
    """基于 Normalizer 的格式转换注册表"""

    def __init__(self) -> None:
        self._normalizers: dict[str, FormatNormalizer] = {}
        self._lazy_normalizers: dict[str, tuple[str, str]] = {}
        self._lock = threading.RLock()

    def register(self, normalizer: FormatNormalizer) -> None:
        key = str(normalizer.FORMAT_ID).upper()
        with self._lock:
            self._normalizers[key] = normalizer
            self._lazy_normalizers.pop(key, None)
        logger.info(f"[FormatConversionRegistry] 注册 normalizer: {normalizer.FORMAT_ID}")

    def register_lazy(self, format_id: str, module_path: str, class_name: str) -> None:
        key = str(format_id).upper()
        with self._lock:
            if key in self._normalizers:
                logger.debug(
                    "[FormatConversionRegistry] 跳过 lazy 注册（normalizer 已实例化）: {}",
                    key,
                )
                return
            existing = self._lazy_normalizers.get(key)
            if existing and existing != (module_path, class_name):
                logger.warning(
                    "[FormatConversionRegistry] FORMAT_ID '{}' 重复 lazy 注册，{}.{}, 将覆盖 {}.{}",
                    key,
                    module_path,
                    class_name,
                    existing[0],
                    existing[1],
                )
            self._lazy_normalizers[key] = (module_path, class_name)
        logger.info(
            "[FormatConversionRegistry] 注册 lazy normalizer: {} -> {}.{}",
            key,
            module_path,
            class_name,
        )

    def _materialize_lazy_normalizer(self, key: str) -> FormatNormalizer | None:
        with self._lock:
            existing = self._normalizers.get(key)
            if existing is not None:
                return existing
            lazy_spec = self._lazy_normalizers.get(key)
            if lazy_spec is None or lazy_spec is _MATERIALIZING:
                return None
            # 标记为正在加载，防止其他线程重复 materialize
            self._lazy_normalizers[key] = _MATERIALIZING

        module_path, class_name = lazy_spec
        try:
            mod = importlib.import_module(module_path)
            obj = getattr(mod, class_name, None)
            if not inspect.isclass(obj) or not issubclass(obj, FormatNormalizer):
                raise TypeError(f"{module_path}.{class_name} 不是有效的 FormatNormalizer")
            normalizer = obj()
        except Exception as e:
            logger.error(
                "[FormatConversionRegistry] lazy 加载 {}.{} 失败: {}",
                module_path,
                class_name,
                e,
            )
            # 恢复 lazy_spec 以便后续重试
            with self._lock:
                if self._lazy_normalizers.get(key) is _MATERIALIZING:
                    self._lazy_normalizers[key] = lazy_spec
            return None

        self.register(normalizer)
        key_upper = str(normalizer.FORMAT_ID).upper()
        with self._lock:
            return self._normalizers.get(key) or self._normalizers.get(key_upper)

    def _find_registered_by_data_format_id(self, target_dfid: str) -> FormatNormalizer | None:
        from src.core.api_format.metadata import get_data_format_id_for_endpoint

        with self._lock:
            registered_items = list(self._normalizers.items())
        for reg_key, reg_normalizer in registered_items:
            if get_data_format_id_for_endpoint(reg_key) == target_dfid:
                return reg_normalizer
        return None

    def _find_lazy_key_by_data_format_id(self, target_dfid: str) -> str | None:
        from src.core.api_format.metadata import get_data_format_id_for_endpoint

        with self._lock:
            lazy_keys = list(self._lazy_normalizers.keys())
        for lazy_key in lazy_keys:
            if get_data_format_id_for_endpoint(lazy_key) == target_dfid:
                return lazy_key
        return None

    def get_normalizer(self, format_id: str) -> FormatNormalizer | None:
        key = str(format_id).upper()
        # 1. 精确匹配
        with self._lock:
            normalizer = self._normalizers.get(key)
        if normalizer is not None:
            return normalizer

        # 2. lazy 精确匹配
        normalizer = self._materialize_lazy_normalizer(key)
        if normalizer is not None:
            return normalizer

        # 2. data_format_id 回退：如 "claude:cli" (dfid="claude") -> ClaudeNormalizer (dfid="claude")
        from src.core.api_format.metadata import get_data_format_id_for_endpoint

        target_dfid = get_data_format_id_for_endpoint(format_id)
        if not target_dfid:
            return None

        # 3. data_format_id 在已实例化 normalizer 中回退
        normalizer = self._find_registered_by_data_format_id(target_dfid)
        if normalizer is not None:
            return normalizer

        # 4. data_format_id 在 lazy normalizer 中回退
        lazy_key = self._find_lazy_key_by_data_format_id(target_dfid)
        if lazy_key:
            return self._materialize_lazy_normalizer(lazy_key)
        return None

    def _require_normalizer(self, format_id: str) -> FormatNormalizer:
        normalizer = self.get_normalizer(format_id)
        if normalizer is None:
            raise FormatConversionError(format_id, format_id, f"未注册 Normalizer: {format_id}")
        return normalizer

    def _same_normalizer(self, source_format: str, target_format: str) -> bool:
        """判断两个 format_id 是否解析到同一个 normalizer 实例（即底层数据格式相同，可直接透传）。
        例如 claude:chat / claude:cli 共享 ClaudeNormalizer，gemini:chat / gemini:cli 共享 GeminiNormalizer。
        """
        if str(source_format).upper() == str(target_format).upper():
            return True
        src = self.get_normalizer(source_format)
        tgt = self.get_normalizer(target_format)
        return src is not None and src is tgt

    def _repair_internal_tool_call_ids(self, internal: InternalRequest) -> dict[str, int]:
        """修复 InternalRequest 中空的 tool id/tool_use_id，避免上游校验报错。"""

        pending_tool_ids: list[str] = []
        auto_counter = 0
        generated_tool_use_ids = 0
        filled_tool_result_ids = 0

        def next_tool_id() -> str:
            nonlocal auto_counter
            auto_counter += 1
            return f"call_auto_{auto_counter}"

        for message in internal.messages:
            for block in message.content:
                if isinstance(block, ToolUseBlock):
                    tool_id = str(block.tool_id or "").strip()
                    if not tool_id:
                        tool_id = next_tool_id()
                        block.tool_id = tool_id
                        generated_tool_use_ids += 1
                    pending_tool_ids.append(tool_id)
                    continue

                if isinstance(block, ToolResultBlock):
                    tool_use_id = str(block.tool_use_id or "").strip()
                    if tool_use_id:
                        block.tool_use_id = tool_use_id
                        if tool_use_id in pending_tool_ids:
                            pending_tool_ids.remove(tool_use_id)
                        continue

                    filled_tool_result_ids += 1
                    if pending_tool_ids:
                        block.tool_use_id = pending_tool_ids.pop(0)
                    else:
                        block.tool_use_id = next_tool_id()

        return {
            "generated_tool_use_ids": generated_tool_use_ids,
            "filled_tool_result_ids": filled_tool_result_ids,
        }

    # ==================== 请求/响应转换（严格） ====================

    def convert_request(
        self,
        request: dict[str, Any],
        source_format: str,
        target_format: str,
        *,
        target_variant: str | None = None,
        output_limit: int | None = None,
    ) -> dict[str, Any]:
        if self._same_normalizer(source_format, target_format) and not target_variant:
            return copy.deepcopy(request)

        # 同 normalizer + variant: 优先尝试轻量补丁（跳过 internal 转换）
        if self._same_normalizer(source_format, target_format) and target_variant:
            normalizer = self._require_normalizer(source_format)
            with _track_conversion_metrics(
                "request_patch", str(source_format).upper(), str(target_format).upper()
            ):
                patched = normalizer.patch_for_variant(copy.deepcopy(request), target_variant)
            if patched is not None:
                return patched

        src = self._require_normalizer(source_format)
        tgt = self._require_normalizer(target_format)

        with _track_conversion_metrics(
            "request", str(source_format).upper(), str(target_format).upper()
        ):
            try:
                internal = src.request_to_internal(copy.deepcopy(request))
                internal.output_limit = output_limit
                repair_stats = self._repair_internal_tool_call_ids(internal)
                if repair_stats["generated_tool_use_ids"] or repair_stats["filled_tool_result_ids"]:
                    logger.debug(
                        "[FormatConversionRegistry] repaired internal tool call ids: source={}, target={}, generated_tool_use_ids={}, filled_tool_result_ids={}",
                        str(source_format).upper(),
                        str(target_format).upper(),
                        repair_stats["generated_tool_use_ids"],
                        repair_stats["filled_tool_result_ids"],
                    )
                return tgt.request_from_internal(internal, target_variant=target_variant)
            except Exception as e:
                raise FormatConversionError(source_format, target_format, str(e)) from e

    async def convert_request_async(
        self,
        request: dict[str, Any],
        source_format: str,
        target_format: str,
        *,
        target_variant: str | None = None,
        output_limit: int | None = None,
    ) -> dict[str, Any]:
        """异步版本的 convert_request，在 internal 阶段执行图片 URL 下载等异步操作。"""
        if self._same_normalizer(source_format, target_format) and not target_variant:
            return copy.deepcopy(request)

        # 同 normalizer + variant: 优先尝试轻量补丁（跳过 internal 转换）
        if self._same_normalizer(source_format, target_format) and target_variant:
            normalizer = self._require_normalizer(source_format)
            with _track_conversion_metrics(
                "request_patch", str(source_format).upper(), str(target_format).upper()
            ):
                patched = normalizer.patch_for_variant(copy.deepcopy(request), target_variant)
            if patched is not None:
                return patched

        src = self._require_normalizer(source_format)
        tgt = self._require_normalizer(target_format)

        with _track_conversion_metrics(
            "request", str(source_format).upper(), str(target_format).upper()
        ):
            try:
                internal = src.request_to_internal(copy.deepcopy(request))
                internal.output_limit = output_limit
                repair_stats = self._repair_internal_tool_call_ids(internal)
                if repair_stats["generated_tool_use_ids"] or repair_stats["filled_tool_result_ids"]:
                    logger.debug(
                        "[FormatConversionRegistry] repaired internal tool call ids: source={}, target={}, generated_tool_use_ids={}, filled_tool_result_ids={}",
                        str(source_format).upper(),
                        str(target_format).upper(),
                        repair_stats["generated_tool_use_ids"],
                        repair_stats["filled_tool_result_ids"],
                    )

                # 异步阶段：解析图片 URL -> base64（仅在目标格式需要时）
                await resolve_image_urls(internal, str(target_format).upper())

                return tgt.request_from_internal(internal, target_variant=target_variant)
            except Exception as e:
                raise FormatConversionError(source_format, target_format, str(e)) from e

    def convert_response(
        self,
        response: dict[str, Any],
        source_format: str,
        target_format: str,
        *,
        requested_model: str | None = None,
    ) -> dict[str, Any]:
        """转换响应格式

        Args:
            response: 原始响应
            source_format: 源格式
            target_format: 目标格式
            requested_model: 用户请求的原始模型名（可选）。
                            如果提供，响应中的 model 字段将使用此值，
                            而不是上游返回的映射后模型名。
        """
        if self._same_normalizer(source_format, target_format):
            response_copy = copy.deepcopy(response)
            # 即使格式相同，也需要替换 model 字段
            if requested_model and isinstance(response_copy, dict):
                # 支持不同格式的 model 字段名
                if "model" in response_copy:
                    response_copy["model"] = requested_model
                elif "modelVersion" in response_copy:
                    response_copy["modelVersion"] = requested_model
            return response_copy

        src = self._require_normalizer(source_format)
        tgt = self._require_normalizer(target_format)

        with _track_conversion_metrics(
            "response", str(source_format).upper(), str(target_format).upper()
        ):
            try:
                internal = src.response_to_internal(copy.deepcopy(response))
                return tgt.response_from_internal(internal, requested_model=requested_model)
            except Exception as e:
                raise FormatConversionError(source_format, target_format, str(e)) from e

    def convert_error_response(
        self,
        error_response: dict[str, Any],
        source_format: str,
        target_format: str,
    ) -> dict[str, Any]:
        if self._same_normalizer(source_format, target_format):
            return copy.deepcopy(error_response)

        src = self._require_normalizer(source_format)
        tgt = self._require_normalizer(target_format)

        if not (
            src.capabilities.supports_error_conversion
            and tgt.capabilities.supports_error_conversion
        ):
            raise FormatConversionError(
                source_format,
                target_format,
                "source/target normalizer 不支持错误转换",
            )

        with _track_conversion_metrics(
            "error", str(source_format).upper(), str(target_format).upper()
        ):
            try:
                internal = src.error_to_internal(copy.deepcopy(error_response))
                return tgt.error_from_internal(internal)
            except Exception as e:
                raise FormatConversionError(source_format, target_format, str(e)) from e

    # ==================== 视频格式转换 ====================

    def convert_video_request(
        self,
        request: dict[str, Any],
        source_format: str,
        target_format: str,
    ) -> dict[str, Any]:
        """转换视频请求格式（OpenAI <-> Gemini）

        Args:
            request: 原始视频请求
            source_format: 源格式（如 openai:video, gemini:video）
            target_format: 目标格式

        Returns:
            转换后的视频请求
        """
        # 统一使用基础格式 ID（去掉 :video 后缀）
        src_base = self._video_format_to_base(source_format)
        tgt_base = self._video_format_to_base(target_format)

        if src_base == tgt_base:
            return copy.deepcopy(request)

        src = self._require_normalizer(src_base)
        tgt = self._require_normalizer(tgt_base)

        with _track_conversion_metrics(
            "video_request", str(source_format).upper(), str(target_format).upper()
        ):
            try:
                internal = src.video_request_to_internal(copy.deepcopy(request))
                return tgt.video_request_from_internal(internal)
            except Exception as e:
                raise FormatConversionError(source_format, target_format, str(e)) from e

    def convert_video_task(
        self,
        task_response: dict[str, Any],
        source_format: str,
        target_format: str,
    ) -> dict[str, Any]:
        """转换视频任务响应格式（OpenAI <-> Gemini）

        Args:
            task_response: 原始任务响应
            source_format: 源格式
            target_format: 目标格式

        Returns:
            转换后的任务响应
        """
        src_base = self._video_format_to_base(source_format)
        tgt_base = self._video_format_to_base(target_format)

        if src_base == tgt_base:
            return copy.deepcopy(task_response)

        src = self._require_normalizer(src_base)
        tgt = self._require_normalizer(tgt_base)

        with _track_conversion_metrics(
            "video_task", str(source_format).upper(), str(target_format).upper()
        ):
            try:
                internal = src.video_task_to_internal(copy.deepcopy(task_response))
                return tgt.video_task_from_internal(internal)
            except Exception as e:
                raise FormatConversionError(source_format, target_format, str(e)) from e

    def can_convert_video(self, source_format: str, target_format: str) -> bool:
        """检查是否支持视频格式转换"""
        src_base = self._video_format_to_base(source_format)
        tgt_base = self._video_format_to_base(target_format)

        if src_base == tgt_base:
            return True

        src = self.get_normalizer(src_base)
        tgt = self.get_normalizer(tgt_base)

        if src is None or tgt is None:
            return False

        # 检查是否有视频转换方法
        return (
            hasattr(src, "video_request_to_internal")
            and hasattr(src, "video_task_to_internal")
            and hasattr(tgt, "video_request_from_internal")
            and hasattr(tgt, "video_task_from_internal")
        )

    def _video_format_to_base(self, format_id: str) -> str:
        """将视频格式 ID 转换为基础格式 ID

        例如: openai:video -> openai:chat, gemini:video -> gemini:chat
        """
        upper = str(format_id).upper()
        if upper.endswith(":VIDEO"):
            base = upper[:-6]  # 去掉 :VIDEO
            return f"{base}:CHAT"
        return upper

    # ==================== 流式转换（严格） ====================

    def convert_stream_chunk(
        self,
        chunk: dict[str, Any],
        source_format: str,
        target_format: str,
        state: StreamState | None = None,
    ) -> list[dict[str, Any]]:
        if self._same_normalizer(source_format, target_format):
            return [chunk]

        src = self._require_normalizer(source_format)
        tgt = self._require_normalizer(target_format)

        if not (src.capabilities.supports_stream and tgt.capabilities.supports_stream):
            raise FormatConversionError(
                source_format,
                target_format,
                "source/target normalizer 不支持流式转换",
            )

        if state is None:
            # 调用方应提供预初始化的 state（包含 model/message_id），
            # 这里仅作为防御性回退，可能导致响应中 model 字段为空
            logger.debug(
                f"convert_stream_chunk: state is None, creating empty StreamState "
                f"(source={source_format}, target={target_format})"
            )
            state = StreamState()

        with _track_conversion_metrics(
            "stream", str(source_format).upper(), str(target_format).upper()
        ):
            try:
                events = src.stream_chunk_to_internal(chunk, state)
                out: list[dict[str, Any]] = []
                for event in events:
                    out.extend(tgt.stream_event_from_internal(event, state))
                return out
            except Exception as e:
                raise FormatConversionError(source_format, target_format, str(e)) from e

    # ==================== 能力查询 ====================

    def can_convert_request(self, source_format: str, target_format: str) -> bool:
        if self._same_normalizer(source_format, target_format):
            return True
        return (
            self.get_normalizer(source_format) is not None
            and self.get_normalizer(target_format) is not None
        )

    def can_convert_response(self, source_format: str, target_format: str) -> bool:
        return self.can_convert_request(source_format, target_format)

    def can_convert_stream(self, source_format: str, target_format: str) -> bool:
        if self._same_normalizer(source_format, target_format):
            return True
        src = self.get_normalizer(source_format)
        tgt = self.get_normalizer(target_format)
        if src is None or tgt is None:
            return False
        return bool(src.capabilities.supports_stream and tgt.capabilities.supports_stream)

    def can_convert_error(self, source_format: str, target_format: str) -> bool:
        if self._same_normalizer(source_format, target_format):
            return True
        src = self.get_normalizer(source_format)
        tgt = self.get_normalizer(target_format)
        if src is None or tgt is None:
            return False
        return bool(
            src.capabilities.supports_error_conversion
            and tgt.capabilities.supports_error_conversion
        )

    def can_convert_full(
        self, format_a: str, format_b: str, *, require_stream: bool = False
    ) -> bool:
        if not self.can_convert_request(format_a, format_b):
            return False
        if not self.can_convert_request(format_b, format_a):
            return False
        if require_stream:
            return self.can_convert_stream(format_a, format_b) and self.can_convert_stream(
                format_b, format_a
            )
        return True

    def list_normalizers(self) -> list[str]:
        with self._lock:
            all_keys = set(self._normalizers.keys()) | set(self._lazy_normalizers.keys())
        return sorted(all_keys)

    def get_supported_targets(self, source_format: str) -> list[str]:
        src = str(source_format).upper()
        with self._lock:
            all_keys = set(self._normalizers.keys()) | set(self._lazy_normalizers.keys())
        if src not in all_keys:
            return []
        return [k for k in sorted(all_keys) if k != src]


# 全局注册表（唯一实现）
format_conversion_registry = FormatConversionRegistry()
_DEFAULT_NORMALIZERS_REGISTERED = False
_REGISTRATION_LOCK = threading.Lock()


def _is_format_normalizer_base(node: ast.expr) -> bool:
    if isinstance(node, ast.Name):
        return node.id == "FormatNormalizer"
    if isinstance(node, ast.Attribute):
        return node.attr == "FormatNormalizer"
    return False


def _extract_format_id_literal(class_node: ast.ClassDef) -> str | None:
    for stmt in class_node.body:
        if isinstance(stmt, ast.Assign):
            for target in stmt.targets:
                if isinstance(target, ast.Name) and target.id == "FORMAT_ID":
                    if isinstance(stmt.value, ast.Constant) and isinstance(stmt.value.value, str):
                        value = stmt.value.value.strip()
                        return value or None
        elif isinstance(stmt, ast.AnnAssign):
            target = stmt.target
            if isinstance(target, ast.Name) and target.id == "FORMAT_ID":
                value = stmt.value
                if isinstance(value, ast.Constant) and isinstance(value.value, str):
                    text = value.value.strip()
                    return text or None
    return None


def _discover_normalizer_specs(normalizers_dir: Path) -> list[tuple[str, str, str]]:
    specs: list[tuple[str, str, str]] = []

    for py_file in sorted(normalizers_dir.glob("*.py")):
        if py_file.name.startswith("_"):
            continue

        module_name = py_file.stem
        module_path = f"src.core.api_format.conversion.normalizers.{module_name}"
        module_specs: list[tuple[str, str, str]] = []

        # 优先 AST 发现，避免导入大模块
        try:
            source = py_file.read_text(encoding="utf-8")
            tree = ast.parse(source, filename=str(py_file))
            for node in tree.body:
                if not isinstance(node, ast.ClassDef):
                    continue
                if not any(_is_format_normalizer_base(base) for base in node.bases):
                    continue
                fmt_id = _extract_format_id_literal(node)
                if fmt_id:
                    module_specs.append((fmt_id, module_path, node.name))
        except Exception as e:
            logger.warning("[FormatConversionRegistry] AST 扫描 {} 失败: {}", module_path, e)

        if module_specs:
            specs.extend(module_specs)
            continue

        # AST 无法识别时，回退到反射发现（保持兼容）
        try:
            mod = importlib.import_module(module_path)
        except Exception as e:
            logger.error("[FormatConversionRegistry] 导入 {} 失败: {}", module_path, e)
            continue

        for _attr_name, obj in inspect.getmembers(mod, inspect.isclass):
            if (
                issubclass(obj, FormatNormalizer)
                and obj is not FormatNormalizer
                and hasattr(obj, "FORMAT_ID")
                and obj.__module__ == mod.__name__
            ):
                fmt_id = str(getattr(obj, "FORMAT_ID", "")).strip()
                if fmt_id:
                    module_specs.append((fmt_id, module_path, obj.__name__))

        if not module_specs:
            logger.warning("[FormatConversionRegistry] 未在 {} 发现可注册 normalizer", module_path)
            continue

        specs.extend(module_specs)

    return specs


def register_default_normalizers() -> None:
    """自动发现并懒注册 normalizers/ 目录下的所有 FormatNormalizer 实现"""
    global _DEFAULT_NORMALIZERS_REGISTERED  # noqa: PLW0603 - module-level 缓存

    # 快速路径：已注册则直接返回（无锁）
    if _DEFAULT_NORMALIZERS_REGISTERED:
        return

    # 慢路径：加锁后双重检查
    with _REGISTRATION_LOCK:
        if _DEFAULT_NORMALIZERS_REGISTERED:
            return

        normalizers_dir = Path(__file__).parent / "normalizers"
        for fmt_id, module_path, class_name in _discover_normalizer_specs(normalizers_dir):
            format_conversion_registry.register_lazy(fmt_id, module_path, class_name)

        _DEFAULT_NORMALIZERS_REGISTERED = True
        logger.info(
            "[FormatConversionRegistry] 已懒注册 {} 个 normalizer",
            len(format_conversion_registry.list_normalizers()),
        )


__all__ = [
    "FormatConversionRegistry",
    "format_conversion_registry",
    "register_default_normalizers",
]
