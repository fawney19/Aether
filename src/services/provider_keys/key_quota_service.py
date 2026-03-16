"""Provider Key 配额刷新编排服务。"""

from __future__ import annotations

import asyncio
from collections.abc import Awaitable, Callable
from typing import Any

from sqlalchemy import or_ as db_or
from sqlalchemy.orm import Session, defer

from src.core.exceptions import InvalidRequestException, NotFoundException
from src.core.logger import logger
from src.core.provider_types import ProviderType, normalize_provider_type
from src.models.database import Provider, ProviderAPIKey, ProviderEndpoint
from src.services.model.upstream_fetcher import merge_upstream_metadata
from src.services.provider.pool import redis_ops as pool_redis
from src.services.provider.pool.account_state import resolve_pool_account_state
from src.services.provider.pool.config import parse_pool_config
from src.services.provider_keys.key_side_effects import run_delete_key_side_effects
from src.services.provider_keys.quota_refresh import (
    refresh_antigravity_key_quota,
    refresh_codex_key_quota,
    refresh_kiro_key_quota,
)

QuotaRefreshHandler = Callable[..., Awaitable[dict]]


CODEX_WHAM_USAGE_URL = "https://chatgpt.com/backend-api/wham/usage"

_QUOTA_REFRESH_HANDLERS: dict[str, QuotaRefreshHandler] = {
    ProviderType.CODEX: refresh_codex_key_quota,
    ProviderType.ANTIGRAVITY: refresh_antigravity_key_quota,
    ProviderType.KIRO: refresh_kiro_key_quota,
}

QUOTA_REFRESH_PROVIDER_TYPES: frozenset[str] = frozenset(_QUOTA_REFRESH_HANDLERS.keys())


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
    handler = _QUOTA_REFRESH_HANDLERS.get(provider_type)
    if handler is not None:
        return handler
    raise InvalidRequestException("仅支持 Codex / Antigravity / Kiro 类型的 Provider 刷新限额")


async def refresh_provider_quota_for_provider(
    db: Session,
    provider_id: str,
    codex_wham_usage_url: str,
    key_ids: list[str] | None = None,
) -> dict:
    """刷新指定 Provider 的限额信息（默认所有活跃 Key，可按 key_ids 限定）。"""
    provider = db.query(Provider).filter(Provider.id == provider_id).first()
    if not provider:
        raise NotFoundException(f"Provider {provider_id} 不存在")

    provider_type = normalize_provider_type(getattr(provider, "provider_type", ""))
    if provider_type not in QUOTA_REFRESH_PROVIDER_TYPES:
        raise InvalidRequestException("仅支持 Codex / Antigravity / Kiro 类型的 Provider 刷新限额")
    pool_cfg = parse_pool_config(getattr(provider, "config", None))
    auto_remove_banned_keys = bool(pool_cfg and pool_cfg.auto_remove_banned_keys)

    selected_key_ids: list[str] | None = None
    if key_ids is not None:
        deduped: list[str] = []
        seen: set[str] = set()
        for raw in key_ids:
            value = str(raw).strip()
            if not value or value in seen:
                continue
            seen.add(value)
            deduped.append(value)
        selected_key_ids = deduped

    keys_query = (
        db.query(ProviderAPIKey)
        .options(
            defer(ProviderAPIKey.health_by_format),
            defer(ProviderAPIKey.circuit_breaker_by_format),
            defer(ProviderAPIKey.adjustment_history),
            defer(ProviderAPIKey.utilization_samples),
        )
        .filter(
            ProviderAPIKey.provider_id == provider_id,
        )
    )
    if selected_key_ids is None:
        # 全量刷新：活跃 key + 被系统自动标记 ACCOUNT_BLOCK 的 key。
        # 后者使 ACCOUNT_BLOCK 标记的 key 也参与刷新，账号恢复后可自动解除。
        # 注意：不能用宽泛的 oauth_invalid_reason IS NOT NULL，否则会纳入
        # 用户手动停用 (is_active=False) 但恰好也有 reason 的历史 key。
        keys_query = keys_query.filter(
            db_or(
                ProviderAPIKey.is_active.is_(True),
                ProviderAPIKey.oauth_invalid_reason.startswith("[ACCOUNT_BLOCK]"),
            )
        )
    else:
        if not selected_key_ids:
            return {
                "success": 0,
                "failed": 0,
                "total": 0,
                "results": [],
                "message": "未提供可刷新的 Key",
                "auto_removed": 0,
            }
        keys_query = keys_query.filter(ProviderAPIKey.id.in_(selected_key_ids))

    keys = keys_query.all()
    if not keys:
        return {
            "success": 0,
            "failed": 0,
            "total": 0,
            "results": [],
            "message": "没有可刷新的 Key",
            "auto_removed": 0,
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
    auto_removed_contexts: list[tuple[str, str | None, list[str] | None]] = []
    result_index_by_key_id: dict[str, dict[str, Any]] = {}
    for result in results:
        rid = str(result.get("key_id", "")).strip()
        if rid:
            result_index_by_key_id[rid] = result

    if metadata_updates or state_updates or auto_remove_banned_keys:
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

            if auto_remove_banned_keys:
                account_state = resolve_pool_account_state(
                    provider_type=provider_type,
                    upstream_metadata=getattr(key, "upstream_metadata", None),
                    oauth_invalid_reason=getattr(key, "oauth_invalid_reason", None),
                )
                if account_state.blocked:
                    key_id = str(getattr(key, "id", "") or "")
                    auto_removed_contexts.append(
                        (
                            key_id,
                            (
                                str(getattr(key, "provider_id", "") or "")
                                if getattr(key, "provider_id", None)
                                else None
                            ),
                            getattr(key, "allowed_models", None),
                        )
                    )
                    if key_id and key_id in result_index_by_key_id:
                        result_index_by_key_id[key_id]["auto_removed"] = True
                    db.delete(key)
                    continue

            if key_dirty:
                db.add(key)

    db.commit()

    if auto_removed_contexts:
        cleanup_coros = []
        for key_id, pid, _allowed_models in auto_removed_contexts:
            if not key_id or not pid:
                continue
            cleanup_coros.append(pool_redis.clear_cooldown(pid, key_id))
            cleanup_coros.append(pool_redis.clear_cost(pid, key_id))
        if cleanup_coros:
            await asyncio.gather(*cleanup_coros, return_exceptions=True)
        for _key_id, pid, allowed_models in auto_removed_contexts:
            await run_delete_key_side_effects(
                db=db,
                provider_id=pid,
                deleted_key_allowed_models=allowed_models,
            )
        logger.warning(
            "[QUOTA_REFRESH] Provider {}: auto removed {} banned key(s): {}",
            provider_id,
            len(auto_removed_contexts),
            [ctx[0][:8] for ctx in auto_removed_contexts if ctx[0]],
        )

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
        "auto_removed": len(auto_removed_contexts),
    }
