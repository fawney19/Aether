"""
格式标准化器接口（FormatNormalizer）

每个格式（OpenAI/Claude/Gemini）实现一个 Normalizer，将 provider 结构转换到 internal，
再从 internal 输出到目标格式。
"""

from abc import ABC, abstractmethod
from typing import Any

from .internal import (
    FormatCapabilities,
    ImageBlock,
    InternalError,
    InternalRequest,
    InternalResponse,
)
from .internal_video import InternalVideoPollResult, InternalVideoRequest, InternalVideoTask
from .stream_events import InternalStreamEvent
from .stream_state import StreamState


class FormatNormalizer(ABC):
    """格式标准化器基类"""

    FORMAT_ID: str  # 如 "CLAUDE", "OPENAI", "GEMINI"
    capabilities: FormatCapabilities

    # ============ 请求转换 ============

    @abstractmethod
    def request_to_internal(self, request: dict[str, Any]) -> InternalRequest:
        """将格式特定请求转换为内部表示"""
        raise NotImplementedError

    @abstractmethod
    def request_from_internal(
        self,
        internal: InternalRequest,
        *,
        target_variant: str | None = None,
    ) -> dict[str, Any]:
        """将内部表示转换为格式特定请求

        Args:
            internal: 内部请求表示
            target_variant: 目标变体（如 "codex"），用于同格式但有细微差异的上游
        """
        raise NotImplementedError

    # ============ 同格式变体补丁（可选） ============

    def patch_for_variant(
        self,
        request: dict[str, Any],
        variant: str,
    ) -> dict[str, Any] | None:
        """同格式 + variant 场景下的轻量补丁（跳过 internal 转换）。

        子类可覆盖此方法，对已知 variant 直接在原始请求体上做最小修改。
        返回 None 表示不支持该 variant 的快速路径，registry 将回退到完整的
        request_to_internal -> request_from_internal 流程。
        """
        return None

    # ============ 响应转换 ============

    @abstractmethod
    def response_to_internal(self, response: dict[str, Any]) -> InternalResponse:
        """将格式特定响应转换为内部表示"""
        raise NotImplementedError

    @abstractmethod
    def response_from_internal(
        self,
        internal: InternalResponse,
        *,
        requested_model: str | None = None,
    ) -> dict[str, Any]:
        """将内部表示转换为格式特定响应

        Args:
            internal: 内部响应表示
            requested_model: 用户请求的原始模型名（可选）。
                            如果提供，响应中的 model 字段将使用此值，
                            而不是上游返回的映射后模型名。
        """
        raise NotImplementedError

    # ============ 流式转换（可选） ============

    def stream_chunk_to_internal(
        self,
        chunk: dict[str, Any],
        state: StreamState,
    ) -> list[InternalStreamEvent]:
        """将格式特定流式块转换为内部事件"""
        raise NotImplementedError

    def stream_event_from_internal(
        self,
        event: InternalStreamEvent,
        state: StreamState,
    ) -> list[dict[str, Any]]:
        """将内部事件转换为格式特定流式块"""
        raise NotImplementedError

    # ============ 错误转换（可选） ============

    def is_error_response(self, response: dict[str, Any]) -> bool:
        """基于 body 的兜底判断（不可靠），子类可覆盖"""
        return False

    def error_to_internal(self, error_response: dict[str, Any]) -> InternalError:
        """将格式特定错误转换为内部表示"""
        raise NotImplementedError

    def error_from_internal(self, internal: InternalError) -> dict[str, Any]:
        """将内部错误表示转换为格式特定错误"""
        raise NotImplementedError

    # ============ 图片工具方法 ============

    @staticmethod
    def _image_url_to_block(url: str) -> ImageBlock:
        """将 image_url 字符串转换为 ImageBlock（支持 data URL 和外部 URL）"""
        if url.startswith("data:") and ";base64," in url:
            header, _, data = url.partition(",")
            media_type = header.split(";")[0].split(":", 1)[-1]
            return ImageBlock(data=data, media_type=media_type)
        return ImageBlock(url=url)

    # ============ 通用解析工具 ============

    def _optional_int(self, value: Any) -> int | None:
        if value is None:
            return None
        try:
            return int(value)
        except (TypeError, ValueError):
            return None

    def _optional_float(self, value: Any) -> float | None:
        if value is None:
            return None
        try:
            return float(value)
        except (TypeError, ValueError):
            return None

    def _coerce_str_list(self, value: Any) -> list[str] | None:
        if value is None:
            return None
        if isinstance(value, str):
            return [value]
        if isinstance(value, list):
            return [str(x) for x in value if x is not None]
        return None

    def _extract_extra(self, payload: dict[str, Any], known_keys: set[str]) -> dict[str, Any]:
        return {k: v for k, v in payload.items() if k not in known_keys}

    def _merge_dropped(self, target: dict[str, int], source: dict[str, int]) -> None:
        for k, v in source.items():
            target[k] = target.get(k, 0) + int(v)

    # ============ 视频转换（可选） ============

    def video_request_to_internal(self, request: dict[str, Any]) -> InternalVideoRequest:
        """将视频请求转换为内部表示"""
        raise NotImplementedError(f"{self.__class__.__name__} does not support video conversion")

    def video_request_from_internal(self, internal: InternalVideoRequest) -> dict[str, Any]:
        """将内部视频请求转换为格式特定请求"""
        raise NotImplementedError(f"{self.__class__.__name__} does not support video conversion")

    def video_task_to_internal(self, response: dict[str, Any]) -> InternalVideoTask:
        """将视频任务响应转换为内部表示"""
        raise NotImplementedError(f"{self.__class__.__name__} does not support video conversion")

    def video_task_from_internal(
        self, internal: InternalVideoTask, *, base_url: str | None = None
    ) -> dict[str, Any]:
        """将内部视频任务转换为格式特定响应

        Args:
            internal: 内部视频任务表示
            base_url: 可选的基础 URL，用于构建完整的下载链接
        """
        raise NotImplementedError(f"{self.__class__.__name__} does not support video conversion")

    def video_poll_to_internal(self, response: dict[str, Any]) -> InternalVideoPollResult:
        """将视频轮询响应转换为内部表示"""
        raise NotImplementedError(f"{self.__class__.__name__} does not support video conversion")


__all__ = [
    "FormatNormalizer",
]
