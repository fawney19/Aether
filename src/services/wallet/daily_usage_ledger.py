from __future__ import annotations

from dataclasses import dataclass
from datetime import date, datetime, time, timedelta, timezone
from decimal import Decimal
from zoneinfo import ZoneInfo

from sqlalchemy import func
from sqlalchemy.orm import Session

from src.core.logger import logger
from src.models.database import Usage, WalletDailyUsageLedger
from src.services.billing.precision import to_money_decimal
from src.services.system.scheduler import APP_TIMEZONE


@dataclass(slots=True)
class WalletDailyUsageSnapshot:
    wallet_id: str | None
    billing_date: date
    billing_timezone: str
    total_cost_usd: Decimal
    total_requests: int
    input_tokens: int
    output_tokens: int
    cache_creation_tokens: int
    cache_read_tokens: int
    first_finalized_at: datetime | None
    last_finalized_at: datetime | None
    aggregated_at: datetime
    is_today: bool = False
    id: str | None = None


class WalletDailyUsageLedgerService:
    """钱包每日消费汇总服务。"""

    @staticmethod
    def _get_timezone(timezone_name: str | None = None) -> ZoneInfo:
        tz_name = timezone_name or APP_TIMEZONE
        try:
            return ZoneInfo(tz_name)
        except Exception:
            logger.warning("Invalid billing timezone {}, fallback to UTC", tz_name)
            return ZoneInfo("UTC")

    @classmethod
    def get_today_billing_date(cls, timezone_name: str | None = None) -> date:
        tz = cls._get_timezone(timezone_name)
        return datetime.now(tz).date()

    @classmethod
    def get_day_window_utc(
        cls,
        billing_date: date,
        timezone_name: str | None = None,
    ) -> tuple[datetime, datetime]:
        tz = cls._get_timezone(timezone_name)
        local_start = datetime.combine(billing_date, time.min, tzinfo=tz)
        local_end = local_start + timedelta(days=1)
        return local_start.astimezone(timezone.utc), local_end.astimezone(timezone.utc)

    @classmethod
    def aggregate_day(
        cls,
        db: Session,
        billing_date: date,
        *,
        timezone_name: str | None = None,
        commit: bool = True,
    ) -> int:
        tz_name = timezone_name or APP_TIMEZONE
        start_utc, end_utc = cls.get_day_window_utc(billing_date, tz_name)
        now_utc = datetime.now(timezone.utc)

        rows = (
            db.query(
                Usage.wallet_id.label("wallet_id"),
                func.count(Usage.id).label("total_requests"),
                func.coalesce(func.sum(Usage.total_cost_usd), 0).label("total_cost_usd"),
                func.coalesce(func.sum(Usage.input_tokens), 0).label("input_tokens"),
                func.coalesce(func.sum(Usage.output_tokens), 0).label("output_tokens"),
                func.coalesce(func.sum(Usage.cache_creation_input_tokens), 0).label(
                    "cache_creation_tokens"
                ),
                func.coalesce(func.sum(Usage.cache_read_input_tokens), 0).label(
                    "cache_read_tokens"
                ),
                func.min(Usage.finalized_at).label("first_finalized_at"),
                func.max(Usage.finalized_at).label("last_finalized_at"),
            )
            .filter(
                Usage.wallet_id.isnot(None),
                Usage.billing_status == "settled",
                Usage.total_cost_usd > 0,
                Usage.finalized_at >= start_utc,
                Usage.finalized_at < end_utc,
            )
            .group_by(Usage.wallet_id)
            .all()
        )

        existing_ledgers = (
            db.query(WalletDailyUsageLedger)
            .filter(
                WalletDailyUsageLedger.billing_date == billing_date,
                WalletDailyUsageLedger.billing_timezone == tz_name,
            )
            .all()
        )
        existing_map = {ledger.wallet_id: ledger for ledger in existing_ledgers}
        seen_wallet_ids: set[str] = set()

        for row in rows:
            wallet_id = getattr(row, "wallet_id", None)
            if not wallet_id:
                continue
            seen_wallet_ids.add(str(wallet_id))
            ledger = existing_map.get(str(wallet_id))
            if ledger is None:
                ledger = WalletDailyUsageLedger(
                    wallet_id=str(wallet_id),
                    billing_date=billing_date,
                    billing_timezone=tz_name,
                    aggregated_at=now_utc,
                )
                db.add(ledger)

            ledger.total_cost_usd = to_money_decimal(getattr(row, "total_cost_usd", 0) or 0)
            ledger.total_requests = int(getattr(row, "total_requests", 0) or 0)
            ledger.input_tokens = int(getattr(row, "input_tokens", 0) or 0)
            ledger.output_tokens = int(getattr(row, "output_tokens", 0) or 0)
            ledger.cache_creation_tokens = int(getattr(row, "cache_creation_tokens", 0) or 0)
            ledger.cache_read_tokens = int(getattr(row, "cache_read_tokens", 0) or 0)
            ledger.first_finalized_at = getattr(row, "first_finalized_at", None)
            ledger.last_finalized_at = getattr(row, "last_finalized_at", None)
            ledger.aggregated_at = now_utc

        stale_ledgers = [ledger for ledger in existing_ledgers if ledger.wallet_id not in seen_wallet_ids]
        for ledger in stale_ledgers:
            db.delete(ledger)

        if commit:
            db.commit()
        return len(seen_wallet_ids)

    @classmethod
    def aggregate_recent_complete_days(
        cls,
        db: Session,
        *,
        days: int = 7,
        timezone_name: str | None = None,
    ) -> int:
        tz_name = timezone_name or APP_TIMEZONE
        today = cls.get_today_billing_date(tz_name)
        processed = 0
        for offset in range(1, max(days, 0) + 1):
            billing_date = today - timedelta(days=offset)
            cls.aggregate_day(db, billing_date, timezone_name=tz_name, commit=False)
            processed += 1
        if processed > 0:
            db.commit()
        return processed

    @classmethod
    def get_today_snapshot(
        cls,
        db: Session,
        wallet_id: str | None,
        *,
        timezone_name: str | None = None,
    ) -> WalletDailyUsageSnapshot:
        tz_name = timezone_name or APP_TIMEZONE
        billing_date = cls.get_today_billing_date(tz_name)
        start_utc, end_utc = cls.get_day_window_utc(billing_date, tz_name)
        now_utc = datetime.now(timezone.utc)

        if wallet_id:
            row = (
                db.query(
                    func.count(Usage.id).label("total_requests"),
                    func.coalesce(func.sum(Usage.total_cost_usd), 0).label("total_cost_usd"),
                    func.coalesce(func.sum(Usage.input_tokens), 0).label("input_tokens"),
                    func.coalesce(func.sum(Usage.output_tokens), 0).label("output_tokens"),
                    func.coalesce(func.sum(Usage.cache_creation_input_tokens), 0).label(
                        "cache_creation_tokens"
                    ),
                    func.coalesce(func.sum(Usage.cache_read_input_tokens), 0).label(
                        "cache_read_tokens"
                    ),
                    func.min(Usage.finalized_at).label("first_finalized_at"),
                    func.max(Usage.finalized_at).label("last_finalized_at"),
                )
                .filter(
                    Usage.wallet_id == wallet_id,
                    Usage.billing_status == "settled",
                    Usage.total_cost_usd > 0,
                    Usage.finalized_at >= start_utc,
                    Usage.finalized_at < end_utc,
                )
                .first()
            )
        else:
            row = None

        return WalletDailyUsageSnapshot(
            wallet_id=wallet_id,
            billing_date=billing_date,
            billing_timezone=tz_name,
            total_cost_usd=to_money_decimal(getattr(row, "total_cost_usd", 0) or 0),
            total_requests=int(getattr(row, "total_requests", 0) or 0),
            input_tokens=int(getattr(row, "input_tokens", 0) or 0),
            output_tokens=int(getattr(row, "output_tokens", 0) or 0),
            cache_creation_tokens=int(getattr(row, "cache_creation_tokens", 0) or 0),
            cache_read_tokens=int(getattr(row, "cache_read_tokens", 0) or 0),
            first_finalized_at=getattr(row, "first_finalized_at", None),
            last_finalized_at=getattr(row, "last_finalized_at", None),
            aggregated_at=now_utc,
            is_today=True,
        )
