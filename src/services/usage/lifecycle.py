from __future__ import annotations

from datetime import datetime, timezone
from typing import Any

from sqlalchemy.orm import Session

from src.core.logger import logger
from src.models.database import ApiKey, Usage, User
from src.services.billing.precision import to_money_decimal
from src.services.provider_keys.codex_quota_sync_dispatcher import (
    dispatch_codex_quota_sync_from_response_headers,
)
from src.services.system.config import SystemConfigService
from src.services.wallet import WalletService


class UsageLifecycleMixin:
    """使用记录生命周期管理方法"""

    @staticmethod
    def _is_billing_terminal(usage: Usage | None) -> bool:
        return bool(
            usage is not None and getattr(usage, "billing_status", None) in {"settled", "void"}
        )

    @classmethod
    def begin_pending_usage(
        cls,
        db: Session,
        request_id: str,
        user: User | None,
        api_key: ApiKey | None,
        model: str,
        *,
        is_stream: bool = False,
        request_type: str = "chat",
        api_format: str | None = None,
        request_headers: dict[str, Any] | None = None,
        request_body: Any | None = None,
    ) -> Usage:
        """
        创建（或返回已有）pending Usage 记录，但**不提交事务**。

        适用场景：
        - ApplicationService 在同一事务内创建 pending usage + task + candidates
        - submit 幂等：重复调用同一 request_id 时返回已有记录
        """
        existing = db.query(Usage).filter(Usage.request_id == request_id).first()
        if existing:
            return existing

        # 根据配置决定是否记录请求详情
        should_log_headers = SystemConfigService.should_log_headers(db)
        should_log_body = SystemConfigService.should_log_body(db)

        # 处理请求头
        processed_request_headers = None
        if should_log_headers and request_headers is not None:
            processed_request_headers = SystemConfigService.mask_sensitive_headers(
                db, request_headers
            )

        # 处理请求体
        processed_request_body = None
        if should_log_body and request_body is not None:
            processed_request_body = SystemConfigService.truncate_body(
                db, request_body, is_request=True
            )

        usage = Usage(
            user_id=user.id if user else None,
            api_key_id=api_key.id if api_key else None,
            username=user.username if user else None,
            api_key_name=api_key.name if api_key else None,
            request_id=request_id,
            provider_name="pending",  # 尚未确定 provider
            model=model,
            input_tokens=0,
            output_tokens=0,
            input_output_total_tokens=0,
            input_context_tokens=0,
            total_tokens=0,
            total_cost_usd=0.0,
            request_type=request_type,
            api_format=api_format,
            is_stream=is_stream,
            status="pending",
            billing_status="pending",
            request_headers=processed_request_headers,
            request_body=processed_request_body,
        )

        db.add(usage)
        db.flush()
        return usage

    @classmethod
    def create_pending_usage(
        cls,
        db: Session,
        request_id: str,
        user: User | None,
        api_key: ApiKey | None,
        model: str,
        is_stream: bool = False,
        request_type: str = "chat",
        api_format: str | None = None,
        request_headers: dict[str, Any] | None = None,
        request_body: Any | None = None,
    ) -> Usage:
        """
        创建 pending 状态的使用记录（在请求开始时调用）

        Args:
            db: 数据库会话
            request_id: 请求ID
            user: 用户对象
            api_key: API Key 对象
            model: 模型名称
            is_stream: 是否流式请求
            api_format: API 格式
            request_headers: 请求头
            request_body: 请求体

        Returns:
            创建的 Usage 记录
        """
        usage = cls.begin_pending_usage(
            db,
            request_id=request_id,
            user=user,
            api_key=api_key,
            model=model,
            is_stream=is_stream,
            request_type=request_type,
            api_format=api_format,
            request_headers=request_headers,
            request_body=request_body,
        )
        db.commit()

        logger.debug("创建 pending 使用记录: request_id={}, model={}", request_id, model)

        return usage

    # ========== billing_status 并发幂等 finalize ==========

    @classmethod
    def finalize_settled(
        cls,
        db: Session,
        request_id: str,
        *,
        total_cost_usd: float,
        request_cost_usd: float | None = None,
        status: str = "completed",
        status_code: int = 200,
        error_message: str | None = None,
        response_time_ms: int | None = None,
        billing_snapshot: dict[str, Any] | None = None,
        extra_metadata: dict[str, Any] | None = None,
        finalized_at: datetime | None = None,
    ) -> bool:
        """
        并发安全的幂等 finalize（settled）。

        约定：
        - 仅当 billing_status='pending' 时才会生效（rowcount==1）
        - 不在本方法内 commit，由调用方决定事务提交时机
        """
        now = finalized_at or datetime.now(timezone.utc)
        cost = to_money_decimal(total_cost_usd)
        request_cost = to_money_decimal(request_cost_usd) if request_cost_usd is not None else cost

        usage = db.query(Usage).filter(Usage.request_id == request_id).with_for_update().first()
        if not usage or usage.billing_status != "pending":
            return False

        usage.billing_status = "settled"
        usage.finalized_at = now
        usage.total_cost_usd = cost
        usage.request_cost_usd = request_cost
        usage.status = status
        usage.status_code = status_code
        usage.error_message = error_message
        usage.response_time_ms = response_time_ms
        if cost > 0:
            WalletService.apply_usage_charge(db, usage=usage, amount_usd=cost)

        # 写入审计快照（只在本次 finalize 生效时执行）
        metadata = usage.request_metadata or {}
        if billing_snapshot is not None:
            metadata["billing_snapshot"] = billing_snapshot
        if extra_metadata:
            metadata.update(extra_metadata)
        usage.request_metadata = cls._sanitize_request_metadata(metadata)

        return True

    @classmethod
    def finalize_void(
        cls,
        db: Session,
        request_id: str,
        *,
        reason: str | None = None,
        status_code: int = 499,
        finalized_at: datetime | None = None,
    ) -> bool:
        """
        并发安全的幂等 finalize（void，不收费）。

        约定：
        - 仅当 billing_status='pending' 时才会生效（rowcount==1）
        - 不在本方法内 commit，由调用方决定事务提交时机
        """
        now = finalized_at or datetime.now(timezone.utc)
        usage = db.query(Usage).filter(Usage.request_id == request_id).with_for_update().first()
        if not usage or usage.billing_status != "pending":
            return False

        usage.billing_status = "void"
        usage.finalized_at = now
        usage.total_cost_usd = to_money_decimal(0)
        usage.request_cost_usd = to_money_decimal(0)
        usage.actual_total_cost_usd = to_money_decimal(0)
        usage.actual_request_cost_usd = to_money_decimal(0)
        usage.status = "cancelled"
        usage.status_code = status_code
        usage.error_message = reason
        usage.response_time_ms = None
        return True

    @classmethod
    def finalize_submitted(
        cls,
        db: Session,
        request_id: str,
        *,
        provider_name: str,
        provider_id: str | None = None,
        provider_endpoint_id: str | None = None,
        provider_api_key_id: str | None = None,
        response_time_ms: int | None = None,
        status_code: int = 200,
        endpoint_api_format: str | None = None,
        provider_request_headers: dict[str, Any] | None = None,
        response_headers: dict[str, Any] | None = None,
        response_body: Any | None = None,
    ) -> bool:
        """
        异步任务提交成功时的幂等结算。

        将 pending 使用记录保留为 pending，仅补齐已知的 provider/响应信息。
        后续轮询完成后通过 update_settled_billing 一次性写入实际费用并扣钱包。

        约定：
        - 仅当 billing_status='pending' 时才会生效（rowcount==1）
        - 不在本方法内 commit，由调用方决定事务提交时机
        """
        # 处理响应头和响应体
        should_log_headers = SystemConfigService.should_log_headers(db)
        should_log_body = SystemConfigService.should_log_body(db)

        processed_provider_headers = None
        if should_log_headers and provider_request_headers is not None:
            processed_provider_headers = SystemConfigService.mask_sensitive_headers(
                db, provider_request_headers
            )

        processed_response_headers = None
        if should_log_headers and response_headers is not None:
            processed_response_headers = dict(response_headers)

        processed_response_body = None
        if should_log_body and response_body is not None:
            processed_response_body = SystemConfigService.truncate_body(
                db, response_body, is_request=False
            )

        values: dict[str, Any] = {
            "status": "pending",
            "status_code": status_code,
            "response_time_ms": response_time_ms,
            "provider_name": provider_name,
            "provider_id": provider_id,
            "provider_endpoint_id": provider_endpoint_id,
            "provider_api_key_id": provider_api_key_id,
            "endpoint_api_format": endpoint_api_format,
        }

        if processed_provider_headers is not None:
            values["provider_request_headers"] = processed_provider_headers
        if processed_response_headers is not None:
            values["response_headers"] = processed_response_headers
        if processed_response_body is not None:
            values["response_body"] = processed_response_body

        usage = db.query(Usage).filter(Usage.request_id == request_id).with_for_update().first()
        if not usage or usage.billing_status != "pending":
            return False
        for key, value in values.items():
            setattr(usage, key, value)
        finalized = True
        if finalized:
            dispatch_codex_quota_sync_from_response_headers(
                provider_api_key_id=provider_api_key_id,
                response_headers=response_headers,
                db=db,
            )
        return finalized

    @classmethod
    def update_settled_billing(
        cls,
        db: Session,
        request_id: str,
        *,
        total_cost_usd: float,
        request_cost_usd: float | None = None,
        status: str = "completed",
        status_code: int = 200,
        error_message: str | None = None,
        response_time_ms: int | None = None,
        billing_snapshot: dict[str, Any] | None = None,
        extra_metadata: dict[str, Any] | None = None,
        finalized_at: datetime | None = None,
    ) -> bool:
        """
        写入异步任务最终账单（轮询完成后调用）。

        语义：
        - 仅允许 pending -> settled / void（首次最终结算）
        - settled / void 一旦进入即不可再修改

        约定：
        - 不在本方法内 commit，由调用方决定事务提交时机
        """
        now = finalized_at or datetime.now(timezone.utc)
        cost = to_money_decimal(total_cost_usd)
        request_cost = to_money_decimal(request_cost_usd) if request_cost_usd is not None else cost

        usage = db.query(Usage).filter(Usage.request_id == request_id).with_for_update().first()
        if not usage or usage.billing_status != "pending":
            return False

        usage.total_cost_usd = cost
        usage.request_cost_usd = request_cost
        usage.status = status
        usage.status_code = status_code
        if error_message is not None:
            usage.error_message = error_message
        if response_time_ms is not None:
            usage.response_time_ms = response_time_ms
        usage.finalized_at = now
        if cost > 0:
            usage.billing_status = "settled"
            WalletService.apply_usage_charge(db, usage=usage, amount_usd=cost)
        else:
            usage.billing_status = "void" if status in {"failed", "cancelled"} else "settled"

        # 写入审计快照
        metadata = usage.request_metadata or {}
        if billing_snapshot is not None:
            metadata["billing_snapshot"] = billing_snapshot
        if extra_metadata:
            metadata.update(extra_metadata)
        metadata["billing_updated_at"] = now.isoformat()
        usage.request_metadata = cls._sanitize_request_metadata(metadata)

        return True

    @classmethod
    def void_settled(
        cls,
        db: Session,
        request_id: str,
        *,
        reason: str | None = None,
        status_code: int = 499,
    ) -> bool:
        """
        已废弃：settled 为账务终态，不允许再回滚为 void。
        """
        logger.warning(
            "void_settled is deprecated and ignored: request_id={}, reason={}, status_code={}",
            request_id,
            reason,
            status_code,
        )
        return False

    @classmethod
    def update_usage_status(
        cls,
        db: Session,
        request_id: str,
        status: str,
        error_message: str | None = None,
        provider: str | None = None,
        target_model: str | None = None,
        first_byte_time_ms: int | None = None,
        provider_id: str | None = None,
        provider_endpoint_id: str | None = None,
        provider_api_key_id: str | None = None,
        api_format: str | None = None,
        endpoint_api_format: str | None = None,
        has_format_conversion: bool | None = None,
        status_code: int | None = None,
        request_headers: dict[str, Any] | None = None,
        request_body: Any | None = None,
        provider_request_headers: dict[str, Any] | None = None,
        provider_request_body: Any | None = None,
    ) -> Usage | None:
        """
        快速更新使用记录状态

        Args:
            db: 数据库会话
            request_id: 请求ID
            status: 新状态 (pending, streaming, completed, failed)
            error_message: 错误消息（仅在 failed 状态时使用）
            provider: 提供商名称（可选，streaming 状态时更新）
            target_model: 映射后的目标模型名（可选）
            first_byte_time_ms: 首字时间/TTFB（可选，streaming 状态时更新）
            provider_id: Provider ID（可选，streaming 状态时更新）
            provider_endpoint_id: Endpoint ID（可选，streaming 状态时更新）
            provider_api_key_id: Provider API Key ID（可选，streaming 状态时更新）
            api_format: API 格式（可选，用于获取按格式配置的倍率）
            endpoint_api_format: 端点原生 API 格式（可选）
            has_format_conversion: 是否发生了格式转换（可选）
            status_code: HTTP 状态码（可选）
            request_headers: 客户端请求头（可选，用于补写 pending/streaming 记录）
            request_body: 客户端请求体（可选，用于补写 pending/streaming 记录）
            provider_request_headers: 提供商请求头（可选，streaming 时可写入）
            provider_request_body: 提供商请求体（可选，streaming 时可写入）

        Returns:
            更新后的 Usage 记录，如果未找到则返回 None
        """
        usage = db.query(Usage).filter(Usage.request_id == request_id).first()
        if not usage:
            logger.warning("未找到 request_id={} 的使用记录，无法更新状态", request_id)
            return None

        if cls._is_billing_terminal(usage):
            logger.debug(
                "跳过已终态 Usage 状态更新: request_id={}, status={}, billing_status={}",
                request_id,
                status,
                getattr(usage, "billing_status", None),
            )
            return usage

        # 避免状态回退：streaming 只能从 pending/streaming 进入
        if status == "streaming" and usage.status not in ("pending", "streaming"):
            logger.debug(
                f"跳过 streaming 状态更新（避免回退）: request_id={request_id}, "
                f"{usage.status} -> {status}"
            )
            return usage

        old_status = usage.status
        usage.status = status
        if error_message:
            usage.error_message = error_message
        if provider:
            usage.provider_name = provider
        elif status == "streaming" and usage.provider_name == "pending":
            # 状态变为 streaming 但 provider_name 仍为 pending，记录警告
            logger.warning(
                f"状态更新为 streaming 但 provider_name 为空: request_id={request_id}, "
                f"当前 provider_name={usage.provider_name}"
            )
        if target_model:
            usage.target_model = target_model
        if first_byte_time_ms is not None:
            usage.first_byte_time_ms = first_byte_time_ms
        if provider_id is not None:
            usage.provider_id = provider_id
        if provider_endpoint_id is not None:
            usage.provider_endpoint_id = provider_endpoint_id
        if provider_api_key_id is not None:
            usage.provider_api_key_id = provider_api_key_id
            # 当设置 provider_api_key_id 时，同步获取并更新 rate_multiplier
            # 这样前端在 streaming 状态就能显示倍率
            rate_multiplier = cls._get_rate_multiplier_sync(
                db, provider_api_key_id, api_format or usage.api_format
            )
            if rate_multiplier is not None:
                usage.rate_multiplier = rate_multiplier
        if endpoint_api_format is not None:
            usage.endpoint_api_format = endpoint_api_format
        if has_format_conversion is not None:
            usage.has_format_conversion = has_format_conversion
        if status_code is not None:
            usage.status_code = status_code

        should_log_headers = SystemConfigService.should_log_headers(db)
        should_log_body = SystemConfigService.should_log_body(db)

        if should_log_headers:
            if isinstance(request_headers, dict):
                usage.request_headers = SystemConfigService.mask_sensitive_headers(
                    db, request_headers
                )
            if isinstance(provider_request_headers, dict):
                usage.provider_request_headers = SystemConfigService.mask_sensitive_headers(
                    db, provider_request_headers
                )
        if should_log_body:
            if request_body is not None:
                usage.request_body = SystemConfigService.truncate_body(
                    db, request_body, is_request=True
                )
            if provider_request_body is not None:
                usage.provider_request_body = SystemConfigService.truncate_body(
                    db, provider_request_body, is_request=True
                )

        # 仅在“明确不会收费”的终态下直接关闭账单。
        # completed 的费用通常要由后续 record_usage / update_settled_billing 写入，
        # 这里不能提前把 billing_status 置为 settled，否则会阻断真正扣费。
        if (
            status in ("failed", "cancelled")
            and getattr(usage, "billing_status", None) == "pending"
        ):
            usage.billing_status = "void"
            if getattr(usage, "finalized_at", None) is None:
                usage.finalized_at = datetime.now(timezone.utc)
            usage.total_cost_usd = to_money_decimal(0)
            usage.request_cost_usd = to_money_decimal(0)
            usage.actual_total_cost_usd = to_money_decimal(0)
            usage.actual_request_cost_usd = to_money_decimal(0)

        db.commit()

        logger.debug("更新使用记录状态: request_id={}, {} -> {}", request_id, old_status, status)

        return usage
