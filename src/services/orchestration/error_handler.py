"""
错误处理服务

负责错误发生后的副作用操作（缓存失效、健康记录、RPM 调整、OAuth Key 标记等）。
与 ErrorClassifier（纯分类，无副作用）分离，遵循单一职责原则。
"""

from __future__ import annotations

import asyncio
import json
import re
from typing import Any

import httpx
from sqlalchemy.orm import Session

from src.core.api_format.signature import make_signature_key
from src.core.crypto import CryptoService
from src.core.exceptions import (
    ProviderAuthException,
    ProviderRateLimitException,
    UpstreamClientException,
)
from src.core.logger import logger
from src.models.database import Provider, ProviderAPIKey, ProviderEndpoint
from src.services.health.monitor import get_health_monitor
from src.services.provider.format import normalize_endpoint_signature
from src.services.provider.pool.config import parse_pool_config
from src.services.rate_limit.adaptive_rpm import get_adaptive_rpm_manager
from src.services.rate_limit.detector import RateLimitType, detect_rate_limit_type
from src.services.scheduling.aware_scheduler import CacheAwareScheduler


class ErrorHandlerService:
    """
    错误处理服务 - 负责错误发生后的副作用操作

    职责：
    1. 缓存亲和性失效
    2. 健康监控记录
    3. 429 自适应 RPM 调整
    4. OAuth Key 状态标记
    """

    def __init__(
        self,
        db: Session,
        adaptive_manager: Any | None = None,
        cache_scheduler: CacheAwareScheduler | None = None,
    ) -> None:
        self.db = db
        self.adaptive_manager = adaptive_manager or get_adaptive_rpm_manager()
        self.cache_scheduler = cache_scheduler

    async def handle_rate_limit(
        self,
        key: ProviderAPIKey,
        provider_name: str,
        current_rpm: int | None,
        exception: ProviderRateLimitException,
        request_id: str | None = None,
    ) -> str:
        """
        处理 429 速率限制错误的自适应调整

        Returns:
            限制类型: "concurrent" 或 "rpm" 或 "unknown"
        """
        try:
            response_headers = {}
            if hasattr(exception, "response_headers"):
                response_headers = exception.response_headers or {}

            rate_limit_info = detect_rate_limit_type(
                headers=response_headers,
                provider_name=provider_name,
                current_usage=current_rpm,
            )

            logger.info(
                "  [{}] 429错误分析: 类型={}, retry_after={}s, 当前RPM={}",
                request_id,
                rate_limit_info.limit_type,
                rate_limit_info.retry_after,
                current_rpm,
            )

            new_limit = self.adaptive_manager.handle_429_error(
                db=self.db,
                key=key,
                rate_limit_info=rate_limit_info,
                current_rpm=current_rpm,
            )

            if rate_limit_info.limit_type == RateLimitType.CONCURRENT:
                logger.warning("  [{}] 并发限制触发（不调整RPM）", request_id)
                return "concurrent"
            elif rate_limit_info.limit_type == RateLimitType.RPM:
                if new_limit is not None:
                    logger.warning(
                        "  [{}] 自适应调整: Key {}... RPM限制 -> {}",
                        request_id,
                        str(key.id)[:8],
                        new_limit,
                    )
                else:
                    logger.info(
                        "  [{}] 学习中: Key {}... 观察已记录，暂不设限",
                        request_id,
                        str(key.id)[:8],
                    )
                return "rpm"
            else:
                return "unknown"

        except Exception as e:
            logger.exception("  [{}] 处理429错误时异常: {}", request_id, e)
            return "unknown"

    async def handle_http_error(
        self,
        http_error: httpx.HTTPStatusError,
        converted_error: Exception,
        error_response_text: str | None,
        *,
        provider: Provider,
        endpoint: ProviderEndpoint,
        key: ProviderAPIKey,
        affinity_key: str,
        api_format: str,
        global_model_id: str,
        request_id: str | None,
        captured_key_concurrent: int | None,
    ) -> None:
        """
        处理 HTTP 错误的副作用（缓存失效、健康记录、OAuth 标记）。

        纯副作用方法：不返回分类结果，不做错误转换。
        """
        client_format_str = normalize_endpoint_signature(api_format)
        fam = str(getattr(endpoint, "api_family", "")).strip().lower()
        kind = str(getattr(endpoint, "endpoint_kind", "")).strip().lower()
        provider_format_str = make_signature_key(fam, kind) if fam and kind else client_format_str

        # 客户端请求错误：不失效缓存，不记录健康失败
        if isinstance(converted_error, UpstreamClientException):
            return

        can_invalidate = bool(endpoint and key and self.cache_scheduler is not None)

        # 认证错误
        if isinstance(converted_error, ProviderAuthException):
            if can_invalidate:
                await self._invalidate_cache(
                    affinity_key, client_format_str, global_model_id, endpoint, key
                )
            if key:
                await asyncio.to_thread(
                    get_health_monitor().record_failure,
                    db=self.db,
                    key_id=str(key.id),
                    api_format=provider_format_str,
                    error_type="ProviderAuthException",
                )
            # 403 VALIDATION_REQUIRED -> 标记 OAuth key 为账号级别封禁
            status_code = http_error.response.status_code if http_error.response else None
            if (
                status_code == 403
                and key
                and str(getattr(key, "auth_type", "") or "").lower() == "oauth"
                and self._is_account_validation_required(error_response_text)
            ):
                self._mark_oauth_key_blocked(key, request_id, provider=provider)
            # 403 suspended -> 标记 OAuth key 为账号被暂停
            elif (
                status_code == 403
                and key
                and str(getattr(key, "auth_type", "") or "").lower() == "oauth"
                and self._is_account_suspended(error_response_text)
            ):
                self._mark_oauth_key_blocked(
                    key,
                    request_id,
                    reason="AWS 账号被暂停",
                    provider=provider,
                )
            # 401 account_deactivated -> 标记 OAuth key 为账号被永久停用
            elif (
                status_code == 401
                and key
                and str(getattr(key, "auth_type", "") or "").lower() == "oauth"
                and self._is_account_deactivated(error_response_text)
            ):
                self._mark_oauth_key_blocked(
                    key,
                    request_id,
                    reason="账号已被停用 (account_deactivated)",
                    provider=provider,
                )
            return

        # 限流错误
        if isinstance(converted_error, ProviderRateLimitException) and key:
            await self.handle_rate_limit(
                key=key,
                provider_name=str(provider.name),
                current_rpm=captured_key_concurrent,
                exception=converted_error,
                request_id=request_id,
            )
            self._sync_gemini_cli_quota_state(
                key=key,
                provider=provider,
                model_name=global_model_id,
                error_text=error_response_text,
                request_id=request_id,
            )

        # 所有非客户端错误均失效缓存
        if can_invalidate:
            await self._invalidate_cache(
                affinity_key, client_format_str, global_model_id, endpoint, key
            )

        # 记录健康失败
        if key:
            await asyncio.to_thread(
                get_health_monitor().record_failure,
                db=self.db,
                key_id=str(key.id),
                api_format=provider_format_str,
                error_type=type(converted_error).__name__,
            )

    async def handle_retriable_error(
        self,
        error: Exception,
        *,
        provider: Provider,
        endpoint: ProviderEndpoint,
        key: ProviderAPIKey,
        affinity_key: str,
        api_format: str,
        global_model_id: str,
        captured_key_concurrent: int | None,
        request_id: str | None,
    ) -> None:
        """处理可重试错误的副作用（缓存失效、健康记录）"""
        client_format_str = normalize_endpoint_signature(api_format)
        fam = str(getattr(endpoint, "api_family", "")).strip().lower()
        kind = str(getattr(endpoint, "endpoint_kind", "")).strip().lower()
        provider_format_str = make_signature_key(fam, kind) if fam and kind else client_format_str

        # 限流错误
        if isinstance(error, ProviderRateLimitException) and key:
            await self.handle_rate_limit(
                key=key,
                provider_name=str(provider.name),
                current_rpm=captured_key_concurrent,
                exception=error,
                request_id=request_id,
            )

        # 失效缓存
        if endpoint and key and self.cache_scheduler is not None:
            await self._invalidate_cache(
                affinity_key, client_format_str, global_model_id, endpoint, key
            )

        # 记录健康失败
        if key:
            await asyncio.to_thread(
                get_health_monitor().record_failure,
                db=self.db,
                key_id=str(key.id),
                api_format=provider_format_str,
                error_type=type(error).__name__,
            )

    async def _invalidate_cache(
        self,
        affinity_key: str,
        api_format: str,
        global_model_id: str,
        endpoint: ProviderEndpoint,
        key: ProviderAPIKey,
    ) -> None:
        """失效缓存亲和性（调用方需确保 cache_scheduler 可用）"""
        assert self.cache_scheduler is not None  # noqa: S101
        await self.cache_scheduler.invalidate_cache(
            affinity_key=affinity_key,
            api_format=api_format,
            global_model_id=global_model_id,
            endpoint_id=str(endpoint.id),
            key_id=str(key.id),
        )

    @staticmethod
    def _extract_oauth_email(key: ProviderAPIKey | None) -> str | None:
        """从 OAuth Key 的加密 auth_config 中提取邮箱"""
        if not key or str(getattr(key, "auth_type", "") or "").lower() != "oauth":
            return None
        encrypted_auth_config = getattr(key, "auth_config", None)
        if not encrypted_auth_config:
            return None
        try:
            decrypted = CryptoService().decrypt(encrypted_auth_config, silent=True)
            auth_config = json.loads(decrypted) if decrypted else {}
        except Exception:
            return None
        email = auth_config.get("email")
        if isinstance(email, str):
            email = email.strip()
            if email:
                return email
        return None

    @classmethod
    def _format_key_display(cls, key: ProviderAPIKey | None) -> str:
        """格式化 Key 显示信息（用于日志）"""
        if not key:
            return "key=unknown"
        key_id = str(getattr(key, "id", "") or "")[:8] or "unknown"
        name = str(getattr(key, "name", "") or "").strip()
        email = cls._extract_oauth_email(key)
        parts = [f"key={key_id}"]
        if email:
            parts.append(f"email={email}")
        if name and name != email:
            parts.append(f"name={name}")
        return " ".join(parts)

    @staticmethod
    def _is_account_validation_required(error_text: str | None) -> bool:
        """
        检测 403 错误是否为 Google 账号验证要求 (VALIDATION_REQUIRED)

        匹配条件（满足任一即可）：
        - error.details 中包含 reason=VALIDATION_REQUIRED
        - error.status 为 PERMISSION_DENIED 且 message 包含 "verify your account"
        """
        if not error_text:
            return False
        search_text = error_text.lower()
        if "validation_required" in search_text:
            return True
        if "verify your account" in search_text and "permission_denied" in search_text:
            return True
        return False

    @staticmethod
    def _is_account_suspended(error_text: str | None) -> bool:
        """
        检测 403 错误是否为 AWS 账号被暂停 (suspended)

        匹配条件（满足任一即可）：
        - 错误文本包含 "temporarily is suspended" 或 "temporarily suspended"
        - 错误文本包含 "AccountSuspendedException"
        - 错误文本匹配 User ID ... suspended 模式
        """
        if not error_text:
            return False
        search_text = error_text.lower()
        if "temporarily is suspended" in search_text or "temporarily suspended" in search_text:
            return True
        if "accountsuspendedexception" in search_text:
            return True
        if re.search(r"user\s*id.*suspend", search_text):
            return True
        return False

    @staticmethod
    def _is_account_deactivated(error_text: str | None) -> bool:
        """
        检测 401 错误是否为账号被永久停用 (deactivated)

        匹配条件：
        - 错误文本包含 "account_deactivated" (OpenAI error code)
        - 错误文本包含 "account has been deactivated"
        - 错误文本包含 "account deactivated"
        """
        if not error_text:
            return False
        search_text = error_text.lower()
        if "account_deactivated" in search_text:
            return True
        if "account has been deactivated" in search_text:
            return True
        if "account deactivated" in search_text:
            return True
        return False

    def _mark_oauth_key_blocked(
        self,
        key: ProviderAPIKey,
        request_id: str | None,
        reason: str = "Google 要求验证账号",
        *,
        provider: Provider,
    ) -> None:
        """标记 OAuth key 为账号级别封禁"""
        try:
            from datetime import datetime, timezone

            from src.services.provider.oauth_token import OAUTH_ACCOUNT_BLOCK_PREFIX

            key.oauth_invalid_at = datetime.now(timezone.utc)
            key.oauth_invalid_reason = f"{OAUTH_ACCOUNT_BLOCK_PREFIX}{reason}"
            # 不设 is_active=False：oauth_invalid 标记已足够阻止调度，
            # 保持 is_active=True 使配额刷新仍能覆盖该 key，账号恢复后可自动解除。

            pool_cfg = parse_pool_config(getattr(provider, "config", None))
            auto_remove_enabled = bool(pool_cfg and pool_cfg.auto_remove_banned_keys)

            if auto_remove_enabled:
                key_id = str(getattr(key, "id", "") or "")
                provider_id = str(getattr(key, "provider_id", "") or "")
                display = self._format_key_display(key)

                self.db.delete(key)
                self.db.commit()
                self._schedule_auto_cleanup_after_delete(provider_id=provider_id, key_id=key_id)
                logger.warning(
                    "  [{}] {} 因 {} 已标记为账号异常并自动清除",
                    request_id,
                    display,
                    reason,
                )
                return

            self.db.commit()
            logger.warning(
                "  [{}] {} 因 {} 已标记为账号异常并自动停用",
                request_id,
                self._format_key_display(key),
                reason,
            )
        except Exception as mark_exc:
            logger.debug("  [{}] 标记 oauth_invalid 失败: {}", request_id, mark_exc)

    @staticmethod
    def _schedule_auto_cleanup_after_delete(*, provider_id: str, key_id: str) -> None:
        if not provider_id or not key_id:
            return

        async def _cleanup() -> None:
            from src.services.cache.model_list_cache import invalidate_models_list_cache
            from src.services.cache.provider_cache import ProviderCacheService
            from src.services.provider.pool import redis_ops as pool_redis

            await ProviderCacheService.invalidate_provider_api_key_cache(key_id)
            await invalidate_models_list_cache()
            await asyncio.gather(
                pool_redis.clear_cooldown(provider_id, key_id),
                pool_redis.clear_cost(provider_id, key_id),
                return_exceptions=True,
            )

        task = asyncio.get_running_loop().create_task(_cleanup())

        def _log_async_error(done_task: asyncio.Task[Any]) -> None:
            try:
                done_task.result()
            except Exception as exc:
                logger.debug("auto cleanup side effect failed for key {}: {}", key_id[:8], exc)

        task.add_done_callback(_log_async_error)

    def _sync_gemini_cli_quota_state(
        self,
        *,
        key: ProviderAPIKey | None,
        provider: Provider | None,
        model_name: str | None,
        error_text: str | None,
        request_id: str | None,
    ) -> None:
        if key is None or provider is None:
            return
        from src.core.provider_types import ProviderType, normalize_provider_type

        provider_type = normalize_provider_type(getattr(provider, "provider_type", None))
        if provider_type != ProviderType.GEMINI_CLI:
            return

        normalized_model = str(model_name or "").strip()
        if not normalized_model:
            return

        try:
            from src.services.model.upstream_fetcher import merge_upstream_metadata
            from src.services.provider.adapters.gemini_cli.quota import (
                build_quota_exhausted_metadata,
                extract_error_model_name,
            )

            resolved_model = extract_error_model_name(error_text, fallback=normalized_model)
            if not resolved_model:
                return

            current_metadata = (
                key.upstream_metadata if isinstance(key.upstream_metadata, dict) else {}
            )
            current_namespace = current_metadata.get("gemini_cli")
            namespace_dict = current_namespace if isinstance(current_namespace, dict) else None

            updates = build_quota_exhausted_metadata(
                model_name=resolved_model,
                error_text=error_text,
                current_namespace=namespace_dict,
            )
            if not updates:
                return

            key.upstream_metadata = merge_upstream_metadata(current_metadata, updates)
            self.db.add(key)
            self.db.commit()
            logger.info(
                "  [{}] Gemini CLI key {} 记录模型冷却: {}",
                request_id,
                str(getattr(key, "id", "") or "")[:8],
                resolved_model,
            )
        except Exception as exc:
            logger.debug("  [{}] Gemini CLI 冷却元数据写入失败: {}", request_id, exc)
