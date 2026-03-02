"""Provider Key 配额刷新编排服务。"""

from __future__ import annotations

import asyncio
from collections.abc import Awaitable, Callable
from typing import Any

from sqlalchemy.orm import Session

from src.core.exceptions import InvalidRequestException, NotFoundException
from src.core.logger import logger
from src.core.provider_types import ProviderType, normalize_provider_type
from src.models.database import Provider, ProviderAPIKey, ProviderEndpoint
from src.services.model.upstream_fetcher import merge_upstream_metadata
from src.services.provider_keys.quota_refresh import (
    refresh_antigravity_key_quota,
    refresh_codex_key_quota,
    refresh_kiro_key_quota,
)
from src.services.scheduling.quota_skipper import is_key_quota_exhausted

QuotaRefreshHandler = Callable[..., Awaitable[dict]]


def _normalize_api_format(api_format: Any) -> str:
    """规范化 api_format，兼容大小写和首尾空白。"""
    if not isinstance(api_format, str):
        return ""
    return api_format.strip().lower()


def _select_refresh_endpoint(provider: Provider, provider_type: str) -> ProviderEndpoint | None:
    """为配额刷新选择端点。"""
    if provider_type == ProviderType.CODEX:
        for ep in provider.endpoints:
            if _normalize_api_format(ep.api_format) == "openai:cli" and ep.is_active:
                return ep
        raise InvalidRequestException("找不到有效的 openai:cli 端点")

    if provider_type == ProviderType.ANTIGRAVITY:
        # Prefer the new signature, but keep backward-compat with existing DB rows.
        for sig in ("gemini:chat", "gemini:cli"):
            for ep in provider.endpoints:
                if _normalize_api_format(ep.api_format) == sig and ep.is_active:
                    return ep
        raise InvalidRequestException("找不到有效的 gemini:chat/gemini:cli 端点")

    # Kiro 不需要端点检查，直接使用 auth_config
    return None


def _resolve_quota_refresh_handler(provider_type: str) -> QuotaRefreshHandler:
    """按 provider 类型返回刷新策略。"""
    if provider_type == ProviderType.CODEX:
        return refresh_codex_key_quota
    if provider_type == ProviderType.ANTIGRAVITY:
        return refresh_antigravity_key_quota
    if provider_type == ProviderType.KIRO:
        return refresh_kiro_key_quota
    raise InvalidRequestException("仅支持 Codex / Antigravity / Kiro 类型的 Provider 刷新限额")


async def refresh_provider_quota_for_provider(
    db: Session,
    provider_id: str,
    codex_wham_usage_url: str,
) -> dict:
    """刷新指定 Provider 下所有活跃 Key 的限额信息。"""
    provider = db.query(Provider).filter(Provider.id == provider_id).first()
    if not provider:
        raise NotFoundException(f"Provider {provider_id} 不存在")

    provider_type = normalize_provider_type(getattr(provider, "provider_type", ""))
    if provider_type not in {ProviderType.CODEX, ProviderType.ANTIGRAVITY, ProviderType.KIRO}:
        raise InvalidRequestException("仅支持 Codex / Antigravity / Kiro 类型的 Provider 刷新限额")

    keys = (
        db.query(ProviderAPIKey)
        .filter(
            ProviderAPIKey.provider_id == provider_id,
            ProviderAPIKey.is_active.is_(True),
        )
        .all()
    )
    if not keys:
        return {
            "success": 0,
            "failed": 0,
            "total": 0,
            "results": [],
            "message": "没有活跃的 Key",
        }

    endpoint = _select_refresh_endpoint(provider, provider_type)
    handler = _resolve_quota_refresh_handler(provider_type)

    results: list[dict[str, Any]] = []
    success_count = 0
    failed_count = 0
    metadata_updates: dict[str, dict] = {}  # key_id -> metadata
    state_updates: dict[str, dict[str, Any]] = {}  # key_id -> model field updates

    async def refresh_single_key(key: ProviderAPIKey) -> dict:
        try:
            return await handler(
                db=db,
                provider=provider,
                key=key,
                endpoint=endpoint,
                codex_wham_usage_url=codex_wham_usage_url,
                metadata_updates=metadata_updates,
                state_updates=state_updates,
            )
        except Exception as e:
            error_msg = str(e) or type(e).__name__
            logger.error("刷新 Key {} 限额失败: {}", key.id, error_msg)
            return {
                "key_id": key.id,
                "key_name": key.name,
                "status": "error",
                "message": error_msg,
            }

    # 分批执行，每批最多 5 个并发
    batch_size = 5
    for i in range(0, len(keys), batch_size):
        batch = keys[i : i + batch_size]
        batch_tasks = [refresh_single_key(key) for key in batch]
        batch_results = await asyncio.gather(*batch_tasks)
        results.extend(batch_results)

        # 统计本批次结果
        for result in batch_results:
            if result["status"] == "success":
                success_count += 1
            else:
                failed_count += 1

    # 统一更新数据库（避免在并发任务中操作 session）
    if metadata_updates or state_updates:
        for key in keys:
            key_dirty = False
            if key.id in metadata_updates:
                updates = metadata_updates[key.id]
                if isinstance(updates, dict):
                    key.upstream_metadata = merge_upstream_metadata(key.upstream_metadata, updates)
                    key_dirty = True
            if key.id in state_updates:
                updates = state_updates[key.id]
                if isinstance(updates, dict):
                    for field_name, field_value in updates.items():
                        setattr(key, field_name, field_value)
                    key_dirty = True
            if key_dirty:
                db.add(key)

    db.commit()

    # 根据配额状态设置/清除 Redis 冷却（仅 Codex / Kiro）
    if provider_type in {ProviderType.CODEX, ProviderType.KIRO}:
        from src.services.provider.pool import redis_ops

        pid = str(provider.id)
        _QUOTA_COOLDOWN_REASON = "quota_exhausted"
        # 配额耗尽冷却 TTL：设为 7 天，下次刷新恢复时会主动清除
        _QUOTA_COOLDOWN_TTL = 7 * 24 * 3600
        key_map = {str(k.id): k for k in keys}

        for r in results:
            kid = r.get("key_id")
            if not kid:
                continue
            key_obj = key_map.get(str(kid))
            status = r.get("status")

            # 优先使用刷新策略返回值；缺失时仅做“耗尽”兜底判断。
            # 注意：未明确“未耗尽”时，不应清除已有 quota_exhausted 冷却，避免误判。
            exhausted_raw = r.get("quota_exhausted")
            exhausted: bool | None = exhausted_raw if isinstance(exhausted_raw, bool) else None
            if exhausted is None and key_obj is not None and status in {"success", "no_metadata"}:
                inferred_exhausted, _ = is_key_quota_exhausted(
                    provider_type, key_obj, model_name=""
                )
                if inferred_exhausted:
                    exhausted = True

            if exhausted is True:
                await redis_ops.set_cooldown(
                    pid,
                    kid,
                    _QUOTA_COOLDOWN_REASON,
                    ttl=_QUOTA_COOLDOWN_TTL,
                )
                continue

            # 仅在成功刷新且“明确未耗尽”时清除配额冷却，避免错误响应导致误清除。
            if status == "success" and exhausted is False:
                # 配额恢复，仅清除配额类冷却（不影响其他错误码冷却）
                existing = await redis_ops.get_cooldown(pid, kid)
                if existing == _QUOTA_COOLDOWN_REASON:
                    await redis_ops.clear_cooldown(pid, kid)

    failed_details = [
        f"{r.get('key_name', r.get('key_id', '?'))}: {r.get('message', 'unknown')}"
        for r in results
        if r["status"] != "success"
    ]
    if failed_details:
        logger.info(
            "[QUOTA_REFRESH] Provider {}: 成功 {}/{}, 失败 {} [{}]",
            provider_id,
            success_count,
            len(keys),
            failed_count,
            "; ".join(failed_details),
        )
    else:
        logger.info(
            "[QUOTA_REFRESH] Provider {}: 成功 {}/{}",
            provider_id,
            success_count,
            len(keys),
        )

    return {
        "success": success_count,
        "failed": failed_count,
        "total": len(keys),
        "results": results,
    }
