"""
号池额度主动探测调度器。

行为：
- 当 provider.pool_advanced.probing_enabled=true 时启用
- Key 在静默超过 probing_interval_minutes 后，主动触发额度刷新
- Key 一旦被实际请求使用（last_used_at 变新），探测冷却自动重置
"""

from __future__ import annotations

import os
import time
from datetime import datetime, timezone
from typing import Any

from sqlalchemy.orm import load_only

from src.clients.redis_client import get_redis_client
from src.core.logger import logger
from src.core.provider_types import ProviderType, normalize_provider_type
from src.database import create_session
from src.models.database import Provider, ProviderAPIKey
from src.services.provider.pool.config import parse_pool_config
from src.services.provider_keys.key_quota_service import (
    CODEX_WHAM_USAGE_URL,
    QUOTA_REFRESH_PROVIDER_TYPES,
    refresh_provider_quota_for_provider,
)
from src.services.system.scheduler import get_scheduler

_REDIS_PREFIX = "ap:quota_probe:last"
_DEFAULT_INTERVAL_MINUTES = 10
_DEFAULT_SCAN_INTERVAL_SECONDS = 60
_DEFAULT_MAX_KEYS_PER_PROVIDER = 50
_MAX_INTERVAL_MINUTES = 1440


def _probe_stamp_key(provider_id: str, key_id: str) -> str:
    return f"{_REDIS_PREFIX}:{provider_id}:{key_id}"


def _to_unix_seconds(value: datetime | None) -> int | None:
    if not isinstance(value, datetime):
        return None
    dt = value if value.tzinfo else value.replace(tzinfo=timezone.utc)
    try:
        return int(dt.timestamp())
    except Exception:
        return None


def _to_float(value: Any) -> float | None:
    if isinstance(value, bool):
        return None
    if isinstance(value, (int, float)):
        return float(value)
    if isinstance(value, str):
        text = value.strip()
        if not text:
            return None
        try:
            return float(text)
        except ValueError:
            return None
    return None


def _extract_quota_updated_at(provider_type: str, upstream_metadata: Any) -> int | None:
    if not isinstance(upstream_metadata, dict):
        return None

    normalized = normalize_provider_type(provider_type)
    if normalized == ProviderType.CODEX.value:
        bucket = upstream_metadata.get("codex")
    elif normalized == ProviderType.KIRO.value:
        bucket = upstream_metadata.get("kiro")
    elif normalized == ProviderType.ANTIGRAVITY.value:
        bucket = upstream_metadata.get("antigravity")
    else:
        return None

    if not isinstance(bucket, dict):
        return None

    updated_at = _to_float(bucket.get("updated_at"))
    if updated_at is None or updated_at <= 0:
        return None

    # 兼容毫秒时间戳
    if updated_at > 1_000_000_000_000:
        updated_at /= 1000
    return int(updated_at)


def _parse_probe_stamp(raw_value: Any) -> int | None:
    parsed = _to_float(raw_value)
    if parsed is None or parsed <= 0:
        return None
    return int(parsed)


def _normalize_probe_interval_minutes(raw_value: Any) -> int:
    parsed = _to_float(raw_value)
    if parsed is None:
        return _DEFAULT_INTERVAL_MINUTES
    return max(1, min(int(parsed), _MAX_INTERVAL_MINUTES))


def _select_probe_key_ids(
    *,
    keys: list[ProviderAPIKey],
    provider_type: str,
    now_ts: int,
    interval_seconds: int,
    last_probe_timestamps: dict[str, int],
    limit: int,
) -> list[str]:
    stale: list[tuple[int, str]] = []
    for key in keys:
        key_id = str(getattr(key, "id", "") or "")
        if not key_id:
            continue
        last_used_ts = _to_unix_seconds(getattr(key, "last_used_at", None))
        quota_updated_ts = _extract_quota_updated_at(
            provider_type,
            getattr(key, "upstream_metadata", None),
        )
        last_probe_ts = last_probe_timestamps.get(key_id)
        anchor_ts = max(last_used_ts or 0, quota_updated_ts or 0, last_probe_ts or 0)
        if anchor_ts <= 0 or (now_ts - anchor_ts) >= interval_seconds:
            stale.append((anchor_ts, key_id))

    # anchor 越小说明越久未被探测/使用，优先探测
    stale.sort(key=lambda item: item[0])
    if limit > 0:
        stale = stale[:limit]
    return [key_id for _, key_id in stale]


class PoolQuotaProbeScheduler:
    """按号池高级配置执行额度主动探测。"""

    def __init__(self) -> None:
        scan_interval_raw = os.getenv(
            "POOL_QUOTA_PROBE_SCAN_INTERVAL_SECONDS",
            str(_DEFAULT_SCAN_INTERVAL_SECONDS),
        )
        max_keys_raw = os.getenv(
            "POOL_QUOTA_PROBE_MAX_KEYS_PER_PROVIDER",
            str(_DEFAULT_MAX_KEYS_PER_PROVIDER),
        )
        self.scan_interval_seconds = max(
            15, int(_to_float(scan_interval_raw) or _DEFAULT_SCAN_INTERVAL_SECONDS)
        )
        self.max_keys_per_provider = max(
            0, int(_to_float(max_keys_raw) or _DEFAULT_MAX_KEYS_PER_PROVIDER)
        )
        self.running = False

    async def start(self) -> Any:
        if self.running:
            logger.warning("PoolQuotaProbeScheduler already running")
            return
        self.running = True
        logger.info(
            "PoolQuotaProbeScheduler started: scan={}s, max_keys_per_provider={}",
            self.scan_interval_seconds,
            self.max_keys_per_provider,
        )

        scheduler = get_scheduler()
        scheduler.add_interval_job(
            self._scheduled_probe_check,
            seconds=self.scan_interval_seconds,
            job_id="pool_quota_probe_check",
            name="号池额度主动探测检查",
        )
        # 不在启动时立即探测：大号池场景下会阻塞启动并占用大量内存。
        # 首次探测由定时调度器在 scan_interval_seconds 后自动触发。

    async def stop(self) -> Any:
        if not self.running:
            return
        self.running = False
        logger.info("PoolQuotaProbeScheduler stopped")

    async def _scheduled_probe_check(self) -> None:
        if not self.running:
            return
        await self._run_probe_cycle()

    async def _load_probe_timestamps(
        self,
        *,
        redis_client: Any,
        provider_id: str,
        key_ids: list[str],
    ) -> dict[str, int]:
        if redis_client is None or not key_ids:
            return {}
        redis_keys = [_probe_stamp_key(provider_id, key_id) for key_id in key_ids]
        try:
            values = await redis_client.mget(redis_keys)
        except Exception as exc:
            logger.debug("PoolQuotaProbeScheduler mget probe stamps failed: {}", exc)
            return {}

        mapping: dict[str, int] = {}
        for key_id, raw in zip(key_ids, values, strict=False):
            parsed = _parse_probe_stamp(raw)
            if parsed is not None:
                mapping[key_id] = parsed
        return mapping

    async def _mark_probe_timestamps(
        self,
        *,
        redis_client: Any,
        provider_id: str,
        key_ids: list[str],
        now_ts: int,
        interval_seconds: int,
    ) -> None:
        if redis_client is None or not key_ids:
            return
        ttl_seconds = max(interval_seconds * 2, 120)
        try:
            pipe = redis_client.pipeline(transaction=False)
            value = str(now_ts)
            for key_id in key_ids:
                pipe.set(_probe_stamp_key(provider_id, key_id), value, ex=ttl_seconds)
            await pipe.execute()
        except Exception as exc:
            logger.debug("PoolQuotaProbeScheduler set probe stamps failed: {}", exc)

    async def _run_probe_cycle(self) -> None:
        now_ts = int(time.time())
        redis_client = await get_redis_client(require_redis=False)

        # 第一阶段：查出符合条件的 provider 列表（轻量查询）
        eligible_providers: list[tuple[str, str, int]] = []  # (id, type, interval_seconds)
        db = create_session()
        try:
            providers = db.query(Provider).filter(Provider.is_active == True).all()  # noqa: E712
            for provider in providers:
                provider_id = str(getattr(provider, "id", "") or "")
                provider_type = normalize_provider_type(getattr(provider, "provider_type", ""))
                if not provider_id or provider_type not in QUOTA_REFRESH_PROVIDER_TYPES:
                    continue

                pool_cfg = parse_pool_config(getattr(provider, "config", None))
                if pool_cfg is None or not pool_cfg.probing_enabled:
                    continue

                interval_minutes = _normalize_probe_interval_minutes(
                    pool_cfg.probing_interval_minutes
                )
                eligible_providers.append((provider_id, provider_type, interval_minutes * 60))
        finally:
            db.close()

        if not eligible_providers:
            return

        # 第二阶段：逐个 provider 查询 key 并筛选探测目标
        # 避免一次性加载所有 provider 的全部 key 到内存
        for provider_id, provider_type, interval_seconds in eligible_providers:
            if not self.running:
                break

            probe_key_ids = await self._select_keys_for_provider(
                provider_id=provider_id,
                provider_type=provider_type,
                interval_seconds=interval_seconds,
                now_ts=now_ts,
                redis_client=redis_client,
            )
            if not probe_key_ids:
                continue

            # 先写探测节流时间戳，避免异常时高频重入
            await self._mark_probe_timestamps(
                redis_client=redis_client,
                provider_id=provider_id,
                key_ids=probe_key_ids,
                now_ts=now_ts,
                interval_seconds=interval_seconds,
            )

            probe_db = create_session()
            try:
                result = await refresh_provider_quota_for_provider(
                    db=probe_db,
                    provider_id=provider_id,
                    codex_wham_usage_url=CODEX_WHAM_USAGE_URL,
                    key_ids=probe_key_ids,
                )
                logger.info(
                    "[POOL_PROBE] Provider {} ({}) 静默探测完成: selected={}, success={}, failed={}",
                    provider_id[:8],
                    provider_type,
                    len(probe_key_ids),
                    int(result.get("success") or 0),
                    int(result.get("failed") or 0),
                )
            except Exception as exc:
                try:
                    probe_db.rollback()
                except Exception:
                    pass
                logger.warning(
                    "[POOL_PROBE] Provider {} ({}) 静默探测失败: {}",
                    provider_id[:8],
                    provider_type,
                    exc,
                )
            finally:
                probe_db.close()

    async def _select_keys_for_provider(
        self,
        *,
        provider_id: str,
        provider_type: str,
        interval_seconds: int,
        now_ts: int,
        redis_client: Any,
    ) -> list[str]:
        """为单个 provider 筛选需要探测的 key，使用独立短生命周期 session。"""
        db = create_session()
        try:
            keys = (
                db.query(ProviderAPIKey)
                .options(
                    load_only(
                        ProviderAPIKey.id,
                        ProviderAPIKey.provider_id,
                        ProviderAPIKey.last_used_at,
                        ProviderAPIKey.upstream_metadata,
                    )
                )
                .filter(
                    ProviderAPIKey.provider_id == provider_id,
                    ProviderAPIKey.is_active == True,  # noqa: E712
                )
                .all()
            )
            if not keys:
                return []

            key_ids = [str(key.id) for key in keys if getattr(key, "id", None)]
            probe_stamps = await self._load_probe_timestamps(
                redis_client=redis_client,
                provider_id=provider_id,
                key_ids=key_ids,
            )
            return _select_probe_key_ids(
                keys=keys,
                provider_type=provider_type,
                now_ts=now_ts,
                interval_seconds=interval_seconds,
                last_probe_timestamps=probe_stamps,
                limit=self.max_keys_per_provider,
            )
        finally:
            db.close()


_pool_quota_probe_scheduler: PoolQuotaProbeScheduler | None = None


def get_pool_quota_probe_scheduler() -> PoolQuotaProbeScheduler:
    global _pool_quota_probe_scheduler
    if _pool_quota_probe_scheduler is None:
        _pool_quota_probe_scheduler = PoolQuotaProbeScheduler()
    return _pool_quota_probe_scheduler


__all__ = [
    "PoolQuotaProbeScheduler",
    "get_pool_quota_probe_scheduler",
    "_select_probe_key_ids",
]
