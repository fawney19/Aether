"""
上游元数据采集器（MetadataCollector）

可扩展注册表模式：
- 每个 Provider 类型可注册一个 MetadataCollector
- 从响应头解析有价值的元数据（额度、限流等）
- 解析结果存入 ProviderAPIKey.upstream_metadata

扩展方式：
1. 创建新文件实现 MetadataCollector
2. 在本文件底部注册
"""

import time
from abc import ABC, abstractmethod
from typing import Any, ClassVar

from sqlalchemy.orm import Session

from src.core.logger import logger

# 节流：每个 key_id 至少间隔 _THROTTLE_SECONDS 秒才写入一次
_THROTTLE_SECONDS = 30
_last_write_ts: dict[str, float] = {}


class MetadataCollector(ABC):
    """元数据采集器基类"""

    # 支持的 provider_type 列表（小写）
    PROVIDER_TYPES: ClassVar[list[str]] = []

    @abstractmethod
    def parse_headers(self, headers: dict[str, str]) -> dict[str, Any] | None:
        """解析响应头，返回结构化元数据。返回 None 表示无可用数据。"""
        raise NotImplementedError


class MetadataCollectorRegistry:
    """元数据采集器注册表"""

    _collectors: ClassVar[list[MetadataCollector]] = []
    _type_index: ClassVar[dict[str, MetadataCollector]] = {}

    @classmethod
    def register(cls, collector: MetadataCollector) -> None:
        cls._collectors.append(collector)
        for pt in collector.PROVIDER_TYPES:
            cls._type_index[pt.lower()] = collector
        logger.info(
            "[MetadataCollectorRegistry] 注册: {} -> {}",
            collector.__class__.__name__,
            collector.PROVIDER_TYPES,
        )

    @classmethod
    def collect(cls, provider_type: str, headers: dict[str, str]) -> dict[str, Any] | None:
        """根据 provider_type 查找采集器并解析响应头"""
        collector = cls._type_index.get(provider_type.lower())
        if collector is None:
            return None
        try:
            return collector.parse_headers(headers)
        except Exception:
            logger.exception(
                "[MetadataCollectorRegistry] {} 解析失败", collector.__class__.__name__
            )
            return None


_initialized = False


def _ensure_collectors_registered() -> None:
    """惰性注册所有采集器（首次调用时执行，避免循环导入）"""
    global _initialized
    if _initialized:
        return
    _initialized = True

    # 延迟导入，避免模块加载时的循环依赖
    from src.services.provider.metadata_collectors.codex import CodexMetadataCollector

    MetadataCollectorRegistry.register(CodexMetadataCollector())


def ensure_collectors_registered() -> None:
    """Ensure metadata collectors are registered (idempotent)."""
    _ensure_collectors_registered()


def collect_and_save_upstream_metadata(
    db: Session,
    *,
    provider_type: str,
    key_id: str,
    response_headers: dict[str, str],
    request_id: str,
) -> None:
    """采集上游元数据并更新 ProviderAPIKey.upstream_metadata（带节流）

    每个 key_id 至少间隔 _THROTTLE_SECONDS 秒才执行一次数据库写入，
    避免高并发时频繁更新同一行。

    Args:
        db: 数据库 Session
        provider_type: Provider 类型（如 "codex"）
        key_id: ProviderAPIKey.id
        response_headers: 上游响应头
        request_id: 请求 ID（用于日志）
    """
    if not provider_type or not key_id or not response_headers:
        return

    # 确保采集器已注册
    _ensure_collectors_registered()

    # 节流检查
    now = time.monotonic()
    last_ts = _last_write_ts.get(key_id, 0.0)
    if now - last_ts < _THROTTLE_SECONDS:
        return

    try:
        metadata = MetadataCollectorRegistry.collect(provider_type, response_headers)
        if metadata is None:
            return

        from src.models.database import ProviderAPIKey

        key = db.query(ProviderAPIKey).filter(ProviderAPIKey.id == key_id).first()
        if key is None:
            return

        key.upstream_metadata = metadata
        db.commit()
        _last_write_ts[key_id] = now
        logger.debug(
            "[{}] 已更新 ProviderAPIKey({}) upstream_metadata",
            request_id,
            key_id,
        )
    except Exception:
        logger.exception("[{}] 采集上游元数据失败", request_id)
        try:
            db.rollback()
        except Exception:
            pass


__all__ = [
    "MetadataCollector",
    "MetadataCollectorRegistry",
    "collect_and_save_upstream_metadata",
    "ensure_collectors_registered",
]
