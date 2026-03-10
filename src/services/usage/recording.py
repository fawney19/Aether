from __future__ import annotations

import asyncio
import uuid
from datetime import datetime, timezone
from decimal import Decimal
from typing import Any

from sqlalchemy.orm import Session

from src.core.logger import logger
from src.models.database import ApiKey, Provider, ProxyNode, Usage, User, UserModelUsageCount
from src.services.billing.precision import to_money_decimal
from src.services.provider_keys.codex_quota_sync_dispatcher import (
    dispatch_codex_quota_sync_from_response_headers,
)
from src.services.usage._billing_integration import UsageBillingIntegrationMixin
from src.services.usage._recording_helpers import (
    METADATA_KEEP_KEYS,
    METADATA_PRUNE_KEYS,
    build_usage_params,
    sanitize_request_metadata,
    update_existing_usage,
)
from src.services.usage._types import UsageCostInfo, UsageRecordParams
from src.services.wallet import WalletService


def _extract_manual_proxy_node_id(metadata: dict[str, Any] | None) -> str | None:
    """从 request_metadata 中提取手动代理节点 ID（仅 is_manual 节点）。

    Tunnel 节点的统计由 aether-proxy 心跳上报，此处只处理手动节点以避免重复计数。
    """
    if not metadata:
        return None
    proxy = metadata.get("proxy")
    if not isinstance(proxy, dict):
        return None
    if not proxy.get("is_manual"):
        return None
    node_id = proxy.get("node_id")
    return node_id if isinstance(node_id, str) and node_id.strip() else None


def _increment_proxy_node_requests(
    db: Session,
    node_counts: dict[str, int],
    failed_counts: dict[str, int] | None = None,
) -> None:
    """批量递增手动代理节点的 total_requests 和 failed_requests（原子 SQL UPDATE）。"""
    if not node_counts and not failed_counts:
        return
    from sqlalchemy import update

    # 合并所有涉及的 node_id
    all_ids = set(node_counts) | set(failed_counts or {})
    for node_id in all_ids:
        total = node_counts.get(node_id, 0)
        failed = (failed_counts or {}).get(node_id, 0)
        values: dict[str, Any] = {}
        if total > 0:
            values["total_requests"] = ProxyNode.total_requests + total
        if failed > 0:
            values["failed_requests"] = ProxyNode.failed_requests + failed
        if values:
            db.execute(
                update(ProxyNode)
                .where(ProxyNode.id == node_id, ProxyNode.is_manual == True)  # noqa: E712
                .values(**values)
            )


class UsageRecordingMixin(UsageBillingIntegrationMixin):
    """记录用量相关方法"""

    # Metadata pruning configuration
    _METADATA_PRUNE_KEYS: tuple[str, ...] = METADATA_PRUNE_KEYS
    _METADATA_KEEP_KEYS: frozenset[str] = METADATA_KEEP_KEYS

    # ------------------------------------------------------------------
    # Helper wrappers
    # ------------------------------------------------------------------

    @staticmethod
    def _build_usage_params(**kwargs: Any) -> dict[str, Any]:
        """构建 Usage 记录的参数字典（委托到模块级函数）"""
        return build_usage_params(**kwargs)

    @staticmethod
    def _update_existing_usage(
        existing_usage: Usage,
        usage_params: dict[str, Any],
        target_model: str | None,
    ) -> None:
        """更新已存在的 Usage 记录（委托到模块级函数）"""
        update_existing_usage(existing_usage, usage_params, target_model)

    @staticmethod
    def _increment_user_model_usage(
        db: Session, user: User | None, model: str, count: int = 1
    ) -> None:
        """原子递增用户-模型调用次数计数器"""
        if user is None:
            return
        from sqlalchemy import func as sa_func
        from sqlalchemy.dialects.postgresql import insert as pg_insert

        stmt = pg_insert(UserModelUsageCount).values(
            id=str(uuid.uuid4()),
            user_id=user.id,
            model=model,
            usage_count=count,
        )
        stmt = stmt.on_conflict_do_update(
            constraint="uq_user_model_usage_count",
            set_={
                "usage_count": UserModelUsageCount.usage_count + count,
                "updated_at": sa_func.now(),
            },
        )
        db.execute(stmt)

    @classmethod
    def _sanitize_request_metadata(cls, metadata: dict[str, Any]) -> dict[str, Any]:
        """元数据清理（委托到模块级函数）"""
        return sanitize_request_metadata(metadata)

    @staticmethod
    def _is_terminal_status(status: str | None) -> bool:
        return status in {"completed", "failed", "cancelled"}

    @staticmethod
    def _is_usage_finalized(usage: Usage) -> bool:
        return (
            getattr(usage, "billing_status", None) in {"settled", "void"}
            and getattr(usage, "finalized_at", None) is not None
        )

    @classmethod
    def _finalize_usage_billing(
        cls,
        db: Session,
        *,
        usage: Usage,
        total_cost: float,
        status: str | None,
        finalized_at: datetime | None = None,
    ) -> tuple[bool, bool]:
        """完成 usage 的结算状态，并在需要时扣减钱包。

        Returns:
            (是否首次进入终态, 是否发生扣费)
        """

        if not cls._is_terminal_status(status):
            if getattr(usage, "billing_status", None) is None:
                usage.billing_status = "pending"
            return False, False

        if (
            getattr(usage, "billing_status", None) in {"settled", "void"}
            and getattr(usage, "finalized_at", None) is not None
        ):
            return False, False

        now = finalized_at or datetime.now(timezone.utc)
        charge_amount = to_money_decimal(total_cost)
        usage.finalized_at = usage.finalized_at or now

        if charge_amount > 0:
            WalletService.apply_usage_charge(db, usage=usage, amount_usd=charge_amount)
            usage.billing_status = "settled"
            return True, True

        usage.billing_status = "void" if status in {"failed", "cancelled"} else "settled"
        return True, False

    # ------------------------------------------------------------------
    # Recording methods
    # ------------------------------------------------------------------

    @classmethod
    async def record_usage_async(
        cls,
        db: Session,
        user: User | None,
        api_key: ApiKey | None,
        provider: str,
        model: str,
        input_tokens: int,
        output_tokens: int,
        cache_creation_input_tokens: int = 0,
        cache_read_input_tokens: int = 0,
        cache_creation_input_tokens_5m: int = 0,
        cache_creation_input_tokens_1h: int = 0,
        request_type: str = "chat",
        api_format: str | None = None,
        api_family: str | None = None,
        endpoint_kind: str | None = None,
        endpoint_api_format: str | None = None,
        has_format_conversion: bool = False,
        is_stream: bool = False,
        response_time_ms: int | None = None,
        first_byte_time_ms: int | None = None,
        status_code: int = 200,
        error_message: str | None = None,
        metadata: dict[str, Any] | None = None,
        request_headers: dict[str, Any] | None = None,
        request_body: Any | None = None,
        provider_request_headers: dict[str, Any] | None = None,
        provider_request_body: Any | None = None,
        response_headers: dict[str, Any] | None = None,
        client_response_headers: dict[str, Any] | None = None,
        response_body: Any | None = None,
        client_response_body: Any | None = None,
        request_id: str | None = None,
        provider_id: str | None = None,
        provider_endpoint_id: str | None = None,
        provider_api_key_id: str | None = None,
        status: str = "completed",
        cache_ttl_minutes: int | None = None,
        use_tiered_pricing: bool = True,
        target_model: str | None = None,
    ) -> Usage:
        """异步记录使用量（简化版，仅插入新记录）

        此方法用于快速记录使用量，不更新用户/API Key 统计，不支持更新已存在的记录。
        适用于不需要更新统计信息的场景。

        如需完整功能（更新用户统计、支持更新已存在记录），请使用 record_usage()。
        """
        # 生成 request_id
        if request_id is None:
            request_id = str(uuid.uuid4())[:8]

        # 使用共享逻辑准备记录参数
        params = UsageRecordParams(
            db=db,
            user=user,
            api_key=api_key,
            provider=provider,
            model=model,
            input_tokens=input_tokens,
            output_tokens=output_tokens,
            cache_creation_input_tokens=cache_creation_input_tokens,
            cache_read_input_tokens=cache_read_input_tokens,
            cache_creation_input_tokens_5m=cache_creation_input_tokens_5m,
            cache_creation_input_tokens_1h=cache_creation_input_tokens_1h,
            request_type=request_type,
            api_format=api_format,
            api_family=api_family,
            endpoint_kind=endpoint_kind,
            endpoint_api_format=endpoint_api_format,
            has_format_conversion=has_format_conversion,
            is_stream=is_stream,
            response_time_ms=response_time_ms,
            first_byte_time_ms=first_byte_time_ms,
            status_code=status_code,
            error_message=error_message,
            metadata=metadata,
            request_headers=request_headers,
            request_body=request_body,
            provider_request_headers=provider_request_headers,
            provider_request_body=provider_request_body,
            response_headers=response_headers,
            client_response_headers=client_response_headers,
            response_body=response_body,
            client_response_body=client_response_body,
            request_id=request_id,
            provider_id=provider_id,
            provider_endpoint_id=provider_endpoint_id,
            provider_api_key_id=provider_api_key_id,
            status=status,
            cache_ttl_minutes=cache_ttl_minutes,
            use_tiered_pricing=use_tiered_pricing,
            target_model=target_model,
        )
        usage_params, total_cost = await cls._prepare_usage_record(params)
        total_cost = to_money_decimal(total_cost)

        # 创建 Usage 记录
        usage = Usage(**usage_params)
        db.add(usage)

        # 更新 GlobalModel 使用计数（原子操作）
        from sqlalchemy import update

        from src.models.database import GlobalModel

        db.execute(
            update(GlobalModel)
            .where(GlobalModel.name == model)
            .values(usage_count=GlobalModel.usage_count + 1)
        )

        # 更新用户-模型调用次数计数器
        cls._increment_user_model_usage(db, user, model)

        # 更新 Provider 月度使用量（原子操作）
        if provider_id:
            actual_total_cost = Decimal(str(usage_params["actual_total_cost_usd"]))
            db.execute(
                update(Provider)
                .where(Provider.id == provider_id)
                .values(monthly_used_usd=Provider.monthly_used_usd + actual_total_cost)
            )

        cls._finalize_usage_billing(
            db,
            usage=usage,
            total_cost=total_cost,
            status=status,
        )

        dispatch_codex_quota_sync_from_response_headers(
            provider_api_key_id=provider_api_key_id,
            response_headers=response_headers,
            db=db,
        )

        db.commit()  # 立即提交事务，释放数据库锁
        return usage

    @classmethod
    async def record_usage(
        cls,
        db: Session,
        user: User | None,
        api_key: ApiKey | None,
        provider: str,
        model: str,
        input_tokens: int,
        output_tokens: int,
        cache_creation_input_tokens: int = 0,
        cache_read_input_tokens: int = 0,
        cache_creation_input_tokens_5m: int = 0,
        cache_creation_input_tokens_1h: int = 0,
        request_type: str = "chat",
        api_format: str | None = None,
        api_family: str | None = None,
        endpoint_kind: str | None = None,
        endpoint_api_format: str | None = None,
        has_format_conversion: bool = False,
        is_stream: bool = False,
        response_time_ms: int | None = None,
        first_byte_time_ms: int | None = None,
        status_code: int = 200,
        error_message: str | None = None,
        metadata: dict[str, Any] | None = None,
        request_headers: dict[str, Any] | None = None,
        request_body: Any | None = None,
        provider_request_headers: dict[str, Any] | None = None,
        provider_request_body: Any | None = None,
        response_headers: dict[str, Any] | None = None,
        client_response_headers: dict[str, Any] | None = None,
        response_body: Any | None = None,
        client_response_body: Any | None = None,
        request_id: str | None = None,
        provider_id: str | None = None,
        provider_endpoint_id: str | None = None,
        provider_api_key_id: str | None = None,
        status: str = "completed",
        cache_ttl_minutes: int | None = None,
        use_tiered_pricing: bool = True,
        target_model: str | None = None,
    ) -> Usage:
        """记录使用量（完整版，支持更新已存在记录和用户统计）

        此方法支持：
        - 检查是否已存在相同 request_id 的记录（更新 vs 插入）
        - 更新用户/API Key 使用统计
        - 阶梯计费

        如只需简单插入新记录，可使用 record_usage_async()。
        """
        # 生成 request_id
        if request_id is None:
            request_id = str(uuid.uuid4())[:8]

        # 使用共享逻辑准备记录参数
        params = UsageRecordParams(
            db=db,
            user=user,
            api_key=api_key,
            provider=provider,
            model=model,
            input_tokens=input_tokens,
            output_tokens=output_tokens,
            cache_creation_input_tokens=cache_creation_input_tokens,
            cache_read_input_tokens=cache_read_input_tokens,
            cache_creation_input_tokens_5m=cache_creation_input_tokens_5m,
            cache_creation_input_tokens_1h=cache_creation_input_tokens_1h,
            request_type=request_type,
            api_format=api_format,
            api_family=api_family,
            endpoint_kind=endpoint_kind,
            endpoint_api_format=endpoint_api_format,
            has_format_conversion=has_format_conversion,
            is_stream=is_stream,
            response_time_ms=response_time_ms,
            first_byte_time_ms=first_byte_time_ms,
            status_code=status_code,
            error_message=error_message,
            metadata=metadata,
            request_headers=request_headers,
            request_body=request_body,
            provider_request_headers=provider_request_headers,
            provider_request_body=provider_request_body,
            response_headers=response_headers,
            client_response_headers=client_response_headers,
            response_body=response_body,
            client_response_body=client_response_body,
            request_id=request_id,
            provider_id=provider_id,
            provider_endpoint_id=provider_endpoint_id,
            provider_api_key_id=provider_api_key_id,
            status=status,
            cache_ttl_minutes=cache_ttl_minutes,
            use_tiered_pricing=use_tiered_pricing,
            target_model=target_model,
        )
        usage_params, total_cost = await cls._prepare_usage_record(params)
        total_cost = to_money_decimal(total_cost)

        def _sync_record() -> Usage:
            """同步 DB 操作: with_for_update + 批量 update + commit"""
            from sqlalchemy import func as sql_func
            from sqlalchemy import update as sa_update

            from src.models.database import ApiKey as ApiKeyModel
            from src.models.database import GlobalModel

            # 检查是否已存在相同 request_id 的记录
            existing_usage = (
                db.query(Usage).filter(Usage.request_id == request_id).with_for_update().first()
            )
            if existing_usage:
                if cls._is_usage_finalized(existing_usage):
                    logger.debug(
                        "request_id {} 已完成结算，跳过重复记账 (billing_status={})",
                        request_id,
                        getattr(existing_usage, "billing_status", None),
                    )
                    return existing_usage
                logger.debug(
                    f"request_id {request_id} 已存在，更新现有记录 "
                    f"(status: {existing_usage.status} -> {status})"
                )
                cls._update_existing_usage(existing_usage, usage_params, target_model)
                usage = existing_usage
            else:
                usage = Usage(**usage_params)
                db.add(usage)

            # 确保 user 和 api_key 在会话中
            nonlocal user, api_key
            if user and not db.object_session(user):
                user = db.merge(user)
            if api_key and not db.object_session(api_key):
                api_key = db.merge(api_key)

            accounted, charge_applied = cls._finalize_usage_billing(
                db,
                usage=usage,
                total_cost=total_cost,
                status=status,
            )

            if accounted:
                # 更新 API 密钥使用量
                if api_key:
                    values: dict[str, Any] = {
                        "total_requests": ApiKeyModel.total_requests + 1,
                        "last_used_at": sql_func.now(),
                        "updated_at": sql_func.now(),
                    }
                    if charge_applied:
                        values["total_cost_usd"] = ApiKeyModel.total_cost_usd + Decimal(
                            str(to_money_decimal(total_cost))
                        )
                    db.execute(
                        sa_update(ApiKeyModel).where(ApiKeyModel.id == api_key.id).values(**values)
                    )

                # 更新 GlobalModel 使用计数
                db.execute(
                    sa_update(GlobalModel)
                    .where(GlobalModel.name == model)
                    .values(usage_count=GlobalModel.usage_count + 1)
                )

                # 更新用户-模型调用次数计数器
                cls._increment_user_model_usage(db, user, model)

                # 更新 Provider 月度使用量（Provider 端真实成本，无论钱包是否扣费）
                if provider_id:
                    actual_total_cost = Decimal(str(usage_params["actual_total_cost_usd"]))
                    db.execute(
                        sa_update(Provider)
                        .where(Provider.id == provider_id)
                        .values(monthly_used_usd=Provider.monthly_used_usd + actual_total_cost)
                    )

                # 更新手动代理节点请求计数（tunnel 节点由心跳上报，不在此处统计）
                manual_node_id = _extract_manual_proxy_node_id(metadata)
                if manual_node_id:
                    failed = {manual_node_id: 1} if status == "failed" else None
                    _increment_proxy_node_requests(db, {manual_node_id: 1}, failed)

            dispatch_codex_quota_sync_from_response_headers(
                provider_api_key_id=provider_api_key_id,
                response_headers=response_headers,
                db=db,
            )

            # 提交事务
            try:
                db.commit()
            except Exception as e:
                logger.error("提交使用记录时出错: {}", e)
                db.rollback()
                raise

            return usage

        return await asyncio.to_thread(_sync_record)

    @classmethod
    async def record_usage_with_custom_cost(
        cls,
        *,
        db: Session,
        user: User | None,
        api_key: ApiKey | None,
        provider: str,
        model: str,
        request_type: str,
        total_cost_usd: float,
        request_cost_usd: float | None = None,
        input_tokens: int = 0,
        output_tokens: int = 0,
        cache_creation_input_tokens: int = 0,
        cache_read_input_tokens: int = 0,
        cache_creation_input_tokens_5m: int = 0,
        cache_creation_input_tokens_1h: int = 0,
        api_format: str | None = None,
        api_family: str | None = None,
        endpoint_kind: str | None = None,
        endpoint_api_format: str | None = None,
        has_format_conversion: bool = False,
        is_stream: bool = False,
        response_time_ms: int | None = None,
        first_byte_time_ms: int | None = None,
        status_code: int = 200,
        error_message: str | None = None,
        metadata: dict[str, Any] | None = None,
        request_headers: dict[str, Any] | None = None,
        request_body: Any | None = None,
        provider_request_headers: dict[str, Any] | None = None,
        provider_request_body: Any | None = None,
        response_headers: dict[str, Any] | None = None,
        client_response_headers: dict[str, Any] | None = None,
        response_body: Any | None = None,
        client_response_body: Any | None = None,
        request_id: str | None = None,
        provider_id: str | None = None,
        provider_endpoint_id: str | None = None,
        provider_api_key_id: str | None = None,
        status: str = "completed",
        target_model: str | None = None,
    ) -> Usage:
        """
        记录"已计算好的"成本（用于 Video/Image/Audio 等异步任务的 FormulaEngine 计费结果）。

        说明：
        - 仍然会应用 ProviderAPIKey.rate_multipliers 计算 actual_* 成本
        - 会更新 User/APIKey/GlobalModel/Provider 的统计（与 record_usage 行为一致）
        - 若 request_id 已存在则更新记录（避免重复写入）
        """
        # 生成 request_id
        if request_id is None:
            request_id = str(uuid.uuid4())[:8]

        # 获取费率倍数与免费套餐
        actual_rate_multiplier, is_free_tier = await cls._get_rate_multiplier_and_free_tier(
            db, provider_api_key_id, provider_id, api_format
        )

        # 成本拆分：非 token 计费默认计入 request_cost
        input_cost = 0.0
        output_cost = 0.0
        cache_creation_cost = 0.0
        cache_read_cost = 0.0
        cache_cost = 0.0
        request_cost_decimal = (
            to_money_decimal(request_cost_usd)
            if request_cost_usd is not None
            else to_money_decimal(total_cost_usd)
        )
        total_cost_decimal = to_money_decimal(total_cost_usd)
        request_cost = float(request_cost_decimal)
        total_cost = float(total_cost_decimal)

        usage_params = build_usage_params(
            db=db,
            user=user,
            api_key=api_key,
            provider=provider,
            model=model,
            input_tokens=input_tokens,
            output_tokens=output_tokens,
            cache_creation_input_tokens=cache_creation_input_tokens,
            cache_read_input_tokens=cache_read_input_tokens,
            cache_creation_input_tokens_5m=cache_creation_input_tokens_5m,
            cache_creation_input_tokens_1h=cache_creation_input_tokens_1h,
            request_type=request_type,
            api_format=api_format,
            api_family=api_family,
            endpoint_kind=endpoint_kind,
            endpoint_api_format=endpoint_api_format,
            has_format_conversion=has_format_conversion,
            is_stream=is_stream,
            response_time_ms=response_time_ms,
            first_byte_time_ms=first_byte_time_ms,
            status_code=status_code,
            error_message=error_message,
            metadata=metadata,
            request_headers=request_headers,
            request_body=request_body,
            provider_request_headers=provider_request_headers,
            provider_request_body=provider_request_body,
            response_headers=response_headers,
            client_response_headers=client_response_headers,
            response_body=response_body,
            client_response_body=client_response_body,
            request_id=request_id,
            provider_id=provider_id,
            provider_endpoint_id=provider_endpoint_id,
            provider_api_key_id=provider_api_key_id,
            status=status,
            target_model=target_model,
            cost=UsageCostInfo(
                input_cost=input_cost,
                output_cost=output_cost,
                cache_creation_cost=cache_creation_cost,
                cache_read_cost=cache_read_cost,
                cache_cost=cache_cost,
                request_cost=request_cost,
                total_cost=total_cost,
                actual_rate_multiplier=actual_rate_multiplier,
                is_free_tier=is_free_tier,
            ),
        )

        # Upsert（并发幂等：锁定 request_id 对应行，避免重复结算）
        existing_usage = (
            db.query(Usage).filter(Usage.request_id == request_id).with_for_update().first()
        )
        if existing_usage:
            # 避免重复记账：若已结算/作废，直接返回（防止并发重复加计数）
            if getattr(existing_usage, "billing_status", None) in ("settled", "void"):
                logger.debug(
                    "record_usage_with_custom_cost: request_id={} already finalized (billing_status={}), skip",
                    request_id,
                    getattr(existing_usage, "billing_status", None),
                )
                return existing_usage

            cls._update_existing_usage(existing_usage, usage_params, target_model)
            usage = existing_usage
        else:
            usage = Usage(**usage_params)
            db.add(usage)

        # 确保 user 和 api_key 在会话中（与 record_usage 保持一致）
        if user and not db.object_session(user):
            user = db.merge(user)
        if api_key and not db.object_session(api_key):
            api_key = db.merge(api_key)

        # 原子更新统计
        from sqlalchemy import func as sql_func
        from sqlalchemy import update

        from src.models.database import ApiKey as ApiKeyModel
        from src.models.database import GlobalModel

        accounted, charge_applied = cls._finalize_usage_billing(
            db,
            usage=usage,
            total_cost=total_cost,
            status=status,
        )

        if accounted:
            # 更新 API 密钥使用量
            if api_key:
                values: dict[str, Any] = {
                    "total_requests": ApiKeyModel.total_requests + 1,
                    "last_used_at": sql_func.now(),
                    "updated_at": sql_func.now(),
                }
                if charge_applied:
                    values["total_cost_usd"] = ApiKeyModel.total_cost_usd + Decimal(
                        str(total_cost_decimal)
                    )
                db.execute(update(ApiKeyModel).where(ApiKeyModel.id == api_key.id).values(**values))

            # 更新 GlobalModel 使用计数
            db.execute(
                update(GlobalModel)
                .where(GlobalModel.name == model)
                .values(usage_count=GlobalModel.usage_count + 1)
            )

            # 更新用户-模型调用次数计数器
            cls._increment_user_model_usage(db, user, model)

            # 更新 Provider 月度使用量（Provider 端真实成本，无论钱包是否扣费）
            if provider_id:
                actual_total_cost = Decimal(str(usage_params["actual_total_cost_usd"]))
                db.execute(
                    update(Provider)
                    .where(Provider.id == provider_id)
                    .values(monthly_used_usd=Provider.monthly_used_usd + actual_total_cost)
                )

        dispatch_codex_quota_sync_from_response_headers(
            provider_api_key_id=provider_api_key_id,
            response_headers=response_headers,
            db=db,
        )

        try:
            db.commit()
        except Exception as e:
            # 并发场景可能触发唯一约束冲突：降级为读取已存在记录
            try:
                from sqlalchemy.exc import IntegrityError

                if isinstance(e, IntegrityError):
                    db.rollback()
                    existing = db.query(Usage).filter(Usage.request_id == request_id).first()
                    if existing:
                        return existing
            except Exception:
                pass

            logger.error("提交使用记录时出错: {}", e)
            db.rollback()
            raise

        return usage

    @classmethod
    async def record_usage_batch(
        cls,
        db: Session,
        records: list[dict[str, Any]],
    ) -> list[Usage]:
        """批量记录使用量（高性能版，单次提交多条记录）

        此方法针对高并发场景优化，特点：
        - 批量插入 Usage 记录，减少 commit 次数
        - 聚合更新用户/API Key 统计（按 user_id/api_key_id 分组）
        - 聚合更新 GlobalModel 和 Provider 统计
        - 支持更新已存在的 pending/streaming 状态记录

        Args:
            db: 数据库会话
            records: 记录列表，每条记录包含 record_usage 所需的参数

        Returns:
            创建的 Usage 记录列表
        """
        if not records:
            return []

        from collections import defaultdict

        from sqlalchemy import update

        from src.models.database import ApiKey as ApiKeyModel
        from src.models.database import GlobalModel

        # 分离需要更新和需要新建的记录
        request_ids = [r.get("request_id") for r in records if r.get("request_id")]
        existing_usages: dict[str, Usage] = {}
        records_to_update: list[dict[str, Any]] = []
        records_to_insert: list[dict[str, Any]] = []

        if request_ids:
            # 查询已存在的 Usage 记录（包括 pending/streaming 状态）
            from sqlalchemy.orm import selectinload

            existing_query = (
                db.query(Usage)
                .options(
                    selectinload(Usage.user),
                    selectinload(Usage.api_key),
                )
                .filter(Usage.request_id.in_(request_ids))
            )
            if hasattr(existing_query, "with_for_update"):
                existing_query = existing_query.with_for_update()
            existing_records = existing_query.all()
            existing_usages = {u.request_id: u for u in existing_records}

            for record in records:
                req_id = record.get("request_id")
                if req_id and req_id in existing_usages:
                    existing_usage = existing_usages[req_id]
                    # 以 billing_status 为幂等闸门：
                    # - pending: 允许更新（补全 tokens/cost/headers/body 等）
                    # - settled/void: 跳过（避免重复记账/重复覆盖）
                    #
                    # 注意：stream_telemetry 在 usage_queue_enabled 时会"直接更新 status"
                    # 来减少 UI 延迟，但不会同步更新 billing_status。
                    # 这会导致出现 status=completed 但 billing_status=pending 的中间态，
                    # 此时仍应允许 completed/failed/cancelled 事件落库补全详情。
                    billing_status = getattr(existing_usage, "billing_status", None)
                    if billing_status == "pending":
                        records_to_update.append(record)
                    else:
                        logger.debug(
                            "批量记录预过滤: 跳过已结算的 request_id={} (status={}, billing_status={})",
                            req_id,
                            getattr(existing_usage, "status", None),
                            billing_status,
                        )
                else:
                    records_to_insert.append(record)
        else:
            records_to_insert = list(records)

        if records_to_update:
            logger.debug(
                f"批量记录: 需要更新 {len(records_to_update)} 条已存在的 billing_status=pending 记录"
            )

        usages: list[Usage] = []
        apikey_stats: dict[str, dict[str, Any]] = defaultdict(
            lambda: {"requests": 0, "cost": 0.0, "is_standalone": False}
        )
        model_counts: dict[str, int] = defaultdict(int)  # model -> count
        user_model_counts: dict[tuple[str, str], int] = defaultdict(
            int
        )  # (user_id, model) -> count
        provider_costs: dict[str, float] = defaultdict(float)  # provider_id -> cost
        proxy_node_counts: dict[str, int] = defaultdict(int)  # node_id -> request count
        proxy_node_failed: dict[str, int] = defaultdict(int)  # node_id -> failed count
        quota_update_candidates: dict[str, dict[str, Any]] = {}

        # 合并所有需要处理的记录（用于预取 user/api_key）
        all_records = records_to_insert + records_to_update

        # 批量预取 User 和 ApiKey，避免 N+1 查询
        user_ids = {r.get("user_id") for r in all_records if r.get("user_id")}
        api_key_ids = {r.get("api_key_id") for r in all_records if r.get("api_key_id")}

        users_map: dict[str, User] = {}
        if user_ids:
            users = db.query(User).filter(User.id.in_(user_ids)).all()
            users_map = {str(u.id): u for u in users}

        api_keys_map: dict[str, ApiKey] = {}
        if api_key_ids:
            api_keys = db.query(ApiKey).filter(ApiKey.id.in_(api_key_ids)).all()
            api_keys_map = {str(k.id): k for k in api_keys}

        skipped_count = 0
        updated_count = 0
        inserted_count = 0
        total_count = len(all_records)

        # 辅助函数：构建 UsageRecordParams
        def build_params(record: dict[str, Any], request_id: str) -> UsageRecordParams:
            user_id = record.get("user_id")
            api_key_id = record.get("api_key_id")
            user = users_map.get(str(user_id)) if user_id else None
            api_key = api_keys_map.get(str(api_key_id)) if api_key_id else None

            return UsageRecordParams(
                db=db,
                user=user,
                api_key=api_key,
                provider=record.get("provider") or "unknown",
                model=record.get("model") or "unknown",
                input_tokens=int(record.get("input_tokens") or 0),
                output_tokens=int(record.get("output_tokens") or 0),
                cache_creation_input_tokens=int(record.get("cache_creation_input_tokens") or 0),
                cache_read_input_tokens=int(record.get("cache_read_input_tokens") or 0),
                cache_creation_input_tokens_5m=int(
                    record.get("cache_creation_input_tokens_5m") or 0
                ),
                cache_creation_input_tokens_1h=int(
                    record.get("cache_creation_input_tokens_1h") or 0
                ),
                request_type=record.get("request_type") or "chat",
                api_format=record.get("api_format"),
                api_family=record.get("api_family"),
                endpoint_kind=record.get("endpoint_kind"),
                endpoint_api_format=record.get("endpoint_api_format"),
                has_format_conversion=bool(record.get("has_format_conversion")),
                is_stream=bool(record.get("is_stream", True)),
                response_time_ms=record.get("response_time_ms"),
                first_byte_time_ms=record.get("first_byte_time_ms"),
                status_code=int(record.get("status_code") or 200),
                error_message=record.get("error_message"),
                metadata=record.get("metadata"),
                request_headers=record.get("request_headers"),
                request_body=record.get("request_body"),
                provider_request_headers=record.get("provider_request_headers"),
                provider_request_body=record.get("provider_request_body"),
                response_headers=record.get("response_headers"),
                client_response_headers=record.get("client_response_headers"),
                response_body=record.get("response_body"),
                client_response_body=record.get("client_response_body"),
                request_id=request_id,
                provider_id=record.get("provider_id"),
                provider_endpoint_id=record.get("provider_endpoint_id"),
                provider_api_key_id=record.get("provider_api_key_id"),
                status=record.get("status") or "completed",
                cache_ttl_minutes=record.get("cache_ttl_minutes"),
                use_tiered_pricing=record.get("use_tiered_pricing", True),
                target_model=record.get("target_model"),
            )

        # 构建所有参数并并行准备
        update_params_list: list[tuple[dict[str, Any], str, UsageRecordParams]] = []
        for record in records_to_update:
            request_id = record.get("request_id")
            if request_id and request_id in existing_usages:
                existing_usage = existing_usages[request_id]
                if existing_usage:
                    try:
                        params = build_params(record, request_id)
                        update_params_list.append((record, request_id, params))
                    except Exception as e:
                        skipped_count += 1
                        logger.warning("批量记录中参数构建失败: {}, request_id={}", e, request_id)

        insert_params_list: list[tuple[dict[str, Any], str, UsageRecordParams]] = []
        for record in records_to_insert:
            request_id = record.get("request_id") or str(uuid.uuid4())[:8]
            try:
                params = build_params(record, request_id)
                insert_params_list.append((record, request_id, params))
            except Exception as e:
                skipped_count += 1
                logger.warning("批量记录中参数构建失败: {}, request_id={}", e, request_id)

        # 并行准备所有记录（性能优化）
        all_params = [p for _, _, p in update_params_list] + [p for _, _, p in insert_params_list]
        if all_params:
            prepared_results = await cls._prepare_usage_records_batch(all_params)
        else:
            prepared_results = []

        # 分配准备结果
        update_results = prepared_results[: len(update_params_list)]
        insert_results = prepared_results[len(update_params_list) :]

        finalized_at = datetime.now(timezone.utc)

        # 1. 处理需要更新的记录
        for i, (record, request_id, params) in enumerate(update_params_list):
            try:
                usage_params, total_cost, exc = update_results[i]
                if exc:
                    raise exc

                # existing_usage 已在构建阶段验证存在
                existing_usage = existing_usages[request_id]
                user = params.user
                api_key = params.api_key

                # 更新已存在的 Usage 记录
                cls._update_existing_usage(existing_usage, usage_params, record.get("target_model"))
                accounted, charge_applied = cls._finalize_usage_billing(
                    db,
                    usage=existing_usage,
                    total_cost=total_cost,
                    status=usage_params.get("status"),
                    finalized_at=finalized_at,
                )
                usages.append(existing_usage)
                updated_count += 1

                # 聚合统计
                if accounted:
                    model_name = record.get("model") or "unknown"
                    model_counts[model_name] += 1
                    if user:
                        user_model_counts[(str(user.id), model_name)] += 1

                    provider_id = record.get("provider_id")
                    if charge_applied and provider_id:
                        actual_cost = float(usage_params.get("actual_total_cost_usd", 0))
                        provider_costs[provider_id] += actual_cost

                    if api_key:
                        key_id = str(api_key.id)
                        apikey_stats[key_id]["requests"] += 1
                        if charge_applied:
                            apikey_stats[key_id]["cost"] += total_cost
                        apikey_stats[key_id]["is_standalone"] = api_key.is_standalone

                    manual_nid = _extract_manual_proxy_node_id(record.get("metadata"))
                    if manual_nid:
                        proxy_node_counts[manual_nid] += 1
                        if record.get("status") == "failed":
                            proxy_node_failed[manual_nid] += 1

                provider_api_key_id = record.get("provider_api_key_id")
                response_headers = record.get("response_headers")
                if (
                    isinstance(provider_api_key_id, str)
                    and provider_api_key_id
                    and isinstance(response_headers, dict)
                ):
                    quota_update_candidates[provider_api_key_id] = response_headers

            except Exception as e:
                skipped_count += 1
                logger.warning("批量记录中更新失败: {}, request_id={}", e, request_id)
                continue

        # 2. 处理需要新建的记录
        for i, (record, request_id, params) in enumerate(insert_params_list):
            try:
                usage_params, total_cost, exc = insert_results[i]
                if exc:
                    raise exc

                user = params.user
                api_key = params.api_key

                usage = Usage(**usage_params)
                db.add(usage)
                accounted, charge_applied = cls._finalize_usage_billing(
                    db,
                    usage=usage,
                    total_cost=total_cost,
                    status=usage_params.get("status"),
                    finalized_at=finalized_at,
                )
                usages.append(usage)
                inserted_count += 1

                # 聚合统计
                if accounted:
                    model_name = record.get("model") or "unknown"
                    model_counts[model_name] += 1
                    if user:
                        user_model_counts[(str(user.id), model_name)] += 1

                    provider_id = record.get("provider_id")
                    if charge_applied and provider_id:
                        actual_cost = float(usage_params.get("actual_total_cost_usd", 0))
                        provider_costs[provider_id] += actual_cost

                    # API Key 统计
                    if api_key:
                        key_id = str(api_key.id)
                        apikey_stats[key_id]["requests"] += 1
                        if charge_applied:
                            apikey_stats[key_id]["cost"] += total_cost
                        apikey_stats[key_id]["is_standalone"] = api_key.is_standalone

                    manual_nid = _extract_manual_proxy_node_id(record.get("metadata"))
                    if manual_nid:
                        proxy_node_counts[manual_nid] += 1
                        if record.get("status") == "failed":
                            proxy_node_failed[manual_nid] += 1

                provider_api_key_id = record.get("provider_api_key_id")
                response_headers = record.get("response_headers")
                if (
                    isinstance(provider_api_key_id, str)
                    and provider_api_key_id
                    and isinstance(response_headers, dict)
                ):
                    quota_update_candidates[provider_api_key_id] = response_headers

            except Exception as e:
                skipped_count += 1
                logger.warning("批量记录中跳过无效记录: {}, request_id={}", e, request_id)
                continue

        # 统计跳过的记录，失败率超过 10% 时提升日志级别
        if skipped_count > 0:
            skip_ratio = skipped_count / total_count if total_count > 0 else 0
            if skip_ratio > 0.1:
                logger.error(
                    "批量记录失败率过高: {}/{} ({:.1f}%) 条记录被跳过",
                    skipped_count,
                    total_count,
                    skip_ratio * 100,
                )
            else:
                logger.warning("批量记录部分失败: {}/{} 条记录被跳过", skipped_count, total_count)

        # 批量更新 GlobalModel 使用计数
        for model_name, count in model_counts.items():
            db.execute(
                update(GlobalModel)
                .where(GlobalModel.name == model_name)
                .values(usage_count=GlobalModel.usage_count + count)
            )

        # 批量更新用户-模型调用次数计数器
        from sqlalchemy import func as sql_func
        from sqlalchemy.dialects.postgresql import insert as pg_insert

        if user_model_counts:
            rows = [
                {
                    "id": str(uuid.uuid4()),
                    "user_id": uid,
                    "model": model_name,
                    "usage_count": count,
                }
                for (uid, model_name), count in user_model_counts.items()
            ]
            stmt = pg_insert(UserModelUsageCount).values(rows)
            stmt = stmt.on_conflict_do_update(
                constraint="uq_user_model_usage_count",
                set_={
                    "usage_count": UserModelUsageCount.usage_count + stmt.excluded.usage_count,
                    "updated_at": sql_func.now(),
                },
            )
            db.execute(stmt)

        # 批量更新 Provider 月度使用量
        for provider_id, cost in provider_costs.items():
            if cost > 0:
                db.execute(
                    update(Provider)
                    .where(Provider.id == provider_id)
                    .values(monthly_used_usd=Provider.monthly_used_usd + Decimal(str(cost)))
                )

        # 批量更新 API Key 统计
        for key_id, stats in apikey_stats.items():
            db.execute(
                update(ApiKeyModel)
                .where(ApiKeyModel.id == key_id)
                .values(
                    total_requests=ApiKeyModel.total_requests + stats["requests"],
                    total_cost_usd=ApiKeyModel.total_cost_usd
                    + Decimal(str(to_money_decimal(stats["cost"]))),
                    last_used_at=sql_func.now(),
                    updated_at=sql_func.now(),
                )
            )

        # 批量更新手动代理节点请求计数
        _increment_proxy_node_requests(db, proxy_node_counts, proxy_node_failed)

        # 配额头实时同步：同一 key 仅取本批次最后一组响应头并执行一次对比更新。
        for provider_api_key_id, response_headers in quota_update_candidates.items():
            dispatch_codex_quota_sync_from_response_headers(
                provider_api_key_id=provider_api_key_id,
                response_headers=response_headers,
                db=db,
            )

        # 单次提交所有更改
        try:
            db.commit()
            total_written = updated_count + inserted_count
            if updated_count > 0:
                logger.debug("批量记录成功: 更新 {} 条, 新建 {} 条", updated_count, inserted_count)
            else:
                logger.debug("批量记录 {} 条使用记录成功", total_written)
        except Exception as e:
            logger.error("批量提交使用记录时出错: {}", e)
            db.rollback()
            raise

        return usages
