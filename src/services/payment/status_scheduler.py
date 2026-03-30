from __future__ import annotations

import asyncio
from datetime import datetime, timezone
from typing import Any

from src.config import config
from src.core.logger import logger
from src.database import create_session
from src.models.database import PaymentOrder
from src.services.payment.gateway import get_payment_gateway, get_payment_gateway_registry
from src.services.payment.service import PaymentService
from src.services.system.scheduler import get_scheduler


class PaymentStatusScheduler:
    """支付宝支付状态主动查询补偿任务。"""

    JOB_ID = "payment_status_check"

    def __init__(self) -> None:
        self.running = False
        self._lock = asyncio.Lock()

    async def start(self) -> None:
        if self.running:
            logger.warning("Payment status scheduler already running")
            return

        if not config.payment_status_scheduler_enabled:
            logger.info("支付状态主动查询调度器已禁用（PAYMENT_STATUS_SCHEDULER_ENABLED=false）")
            return

        self.running = True
        scheduler = get_scheduler()
        scheduler.add_interval_job(
            self._scheduled_status_check,
            seconds=max(15, config.payment_status_check_interval_seconds),
            job_id=self.JOB_ID,
            name="支付状态主动查询",
        )
        logger.info("支付状态主动查询调度器已启动")

        await self._run_status_check_once()

    async def stop(self) -> None:
        if not self.running:
            return

        self.running = False
        scheduler = get_scheduler()
        scheduler.remove_job(self.JOB_ID)
        logger.info("支付状态主动查询调度器已停止")

    async def _scheduled_status_check(self) -> None:
        if not self.running:
            return
        await self._run_status_check_once()

    async def _run_status_check_once(self) -> None:
        async with self._lock:
            now = datetime.now(timezone.utc)
            order_ids = self._list_due_order_ids(now=now)
            if not order_ids:
                return

            logger.info("支付状态主动查询开始，本轮待处理订单数: {}", len(order_ids))
            for order_id in order_ids:
                await self._process_order(order_id=order_id)

    def _list_due_order_ids(self, *, now: datetime) -> list[str]:
        active_query_methods = get_payment_gateway_registry().methods_supporting_active_query()
        if not active_query_methods:
            return []
        db = create_session()
        try:
            rows = (
                db.query(PaymentOrder.id)
                .filter(
                    PaymentOrder.payment_method.in_(active_query_methods),
                    PaymentOrder.status == "pending",
                    PaymentOrder.next_status_check_at.isnot(None),
                    PaymentOrder.next_status_check_at <= now,
                )
                .order_by(PaymentOrder.next_status_check_at.asc(), PaymentOrder.created_at.asc())
                .limit(max(1, config.payment_status_check_batch_size))
                .all()
            )
            return [row[0] for row in rows]
        finally:
            db.close()

    async def _process_order(self, *, order_id: str) -> None:
        snapshot = self._load_order_snapshot(order_id=order_id)
        if snapshot is None:
            return

        now = datetime.now(timezone.utc)
        if snapshot["expires_at"] is not None and snapshot["expires_at"] < now:
            db = create_session()
            try:
                order = PaymentService.get_order(db, order_id=order_id)
                if order is not None and PaymentService.refresh_order_status(order):
                    db.commit()
                else:
                    db.rollback()
            except Exception:
                db.rollback()
                logger.exception("支付状态查询在本地过期清理时失败: {}", order_id)
            finally:
                db.close()
            return

        gateway = get_payment_gateway(snapshot["payment_method"])
        try:
            query_response = await asyncio.to_thread(
                gateway.query_order_status,
                order_no=snapshot["order_no"],
                gateway_order_id=snapshot["gateway_order_id"],
            )
        except Exception as exc:
            await self._record_query_error(order_id=order_id, error=str(exc))
            logger.warning("支付主动查询失败 {}: {}", snapshot["order_no"], exc)
            return

        db = create_session()
        try:
            order = PaymentService.get_order(db, order_id=order_id)
            if order is None:
                db.rollback()
                return

            outcome = PaymentService.sync_order_status(
                db,
                order=order,
                query_response=query_response,
            )
            db.commit()
            self._log_outcome(order_no=order.order_no, outcome=outcome)
        except Exception:
            db.rollback()
            logger.exception("支付状态主动查询处理失败: {}", snapshot["order_no"])
        finally:
            db.close()

    def _load_order_snapshot(self, *, order_id: str) -> dict[str, Any] | None:
        db = create_session()
        try:
            order = PaymentService.get_order(db, order_id=order_id)
            if order is None:
                return None
            return {
                "order_no": order.order_no,
                "payment_method": order.payment_method,
                "gateway_order_id": order.gateway_order_id,
                "expires_at": order.expires_at,
            }
        finally:
            db.close()

    async def _record_query_error(self, *, order_id: str, error: str) -> None:
        db = create_session()
        try:
            order = PaymentService.get_order(db, order_id=order_id)
            if order is None or order.status != "pending":
                db.rollback()
                return

            checked_at = datetime.now(timezone.utc)
            next_check_at = PaymentService.calculate_next_status_check_at(
                order=order,
                now=checked_at,
                attempts=int(order.status_check_attempts or 0) + 1,
            )
            PaymentService.record_status_check_result(
                order=order,
                checked_at=checked_at,
                result="query_exception",
                error=error,
                next_check_at=next_check_at,
            )
            db.commit()
        except Exception:
            db.rollback()
            logger.exception("记录支付主动查询异常失败: {}", order_id)
        finally:
            db.close()

    @staticmethod
    def _log_outcome(*, order_no: str, outcome: dict[str, Any]) -> None:
        trade_status = outcome.get("trade_status")
        if outcome.get("credited"):
            logger.info("支付主动查询补偿入账成功: {} ({})", order_no, trade_status)
            return
        if outcome.get("error"):
            if outcome.get("error") == "交易不存在" and outcome.get("status") == "pending":
                logger.info("支付主动查询暂未查到交易: {}，将按策略继续重试", order_no)
                return
            logger.warning("支付主动查询结果异常: {} - {}", order_no, outcome["error"])
            return
        logger.info(
            "支付主动查询完成: {} status={} trade_status={}",
            order_no,
            outcome.get("status"),
            trade_status,
        )


_payment_status_scheduler: PaymentStatusScheduler | None = None


def get_payment_status_scheduler() -> PaymentStatusScheduler:
    global _payment_status_scheduler
    if _payment_status_scheduler is None:
        _payment_status_scheduler = PaymentStatusScheduler()
    return _payment_status_scheduler
