"""Time range utilities for stats queries."""

from __future__ import annotations

from datetime import date, datetime, time, timedelta, timezone
from typing import Literal

from pydantic import BaseModel, model_validator


class TimeRangeParams(BaseModel):
    """
    Time range parameters (local-date semantics).

    Rules:
    1) Inputs are user-local dates, backend converts to UTC datetime range.
    2) Range is half-open: [start, end).
    """

    start_date: date | None = None
    end_date: date | None = None

    preset: (
        Literal[
            "today",
            "last7days",
            "last30days",
            "last180days",
            "last1year",
        ]
        | None
    ) = None

    granularity: Literal["hour", "day", "week", "month"] = "day"

    timezone: str | None = None
    tz_offset_minutes: int = 0

    @model_validator(mode="after")
    def validate_and_resolve(self) -> "TimeRangeParams":
        """Validate and resolve preset to concrete dates."""
        if self.preset:
            user_today = self._get_user_today()
            match self.preset:
                case "today":
                    self.start_date = self.end_date = user_today
                case "last7days":
                    self.start_date = user_today - timedelta(days=6)
                    self.end_date = user_today
                case "last30days":
                    self.start_date = user_today - timedelta(days=29)
                    self.end_date = user_today
                case "last180days":
                    self.start_date = user_today - timedelta(days=179)
                    self.end_date = user_today
                case "last1year":
                    self.start_date = user_today.replace(year=user_today.year - 1)
                    self.end_date = user_today

        if not self.preset and (self.start_date is None or self.end_date is None):
            raise ValueError("Either preset or both start_date and end_date must be provided")

        if self.start_date and self.end_date and self.start_date > self.end_date:
            raise ValueError("start_date must be <= end_date")

        if self.start_date and self.end_date:
            max_days = 366
            days = (self.end_date - self.start_date).days
            if days > max_days:
                raise ValueError(f"Query range cannot exceed {max_days} days")

        if self.granularity == "hour":
            if self.start_date != self.end_date:
                raise ValueError("Hour granularity only supports single day query")

        return self

    def validate_for_time_series(self) -> "TimeRangeParams":
        """Extra validation for time series queries."""
        if self.granularity == "hour" and self.start_date != self.end_date:
            raise ValueError("Hour granularity only supports single day query")
        if self.start_date and self.end_date:
            days_inclusive = (self.end_date - self.start_date).days + 1
            max_days_for_time_series = 90
            if days_inclusive > max_days_for_time_series:
                raise ValueError(
                    f"Time series query range cannot exceed {max_days_for_time_series} days "
                    f"(requested {days_inclusive} days). "
                    "For longer ranges, use aggregated statistics instead."
                )
        return self

    def _get_user_today(self) -> date:
        """Get user-local 'today'."""
        if self.timezone:
            try:
                from zoneinfo import ZoneInfo

                user_tz = ZoneInfo(self.timezone)
                return datetime.now(user_tz).date()
            except Exception:
                pass

        user_now = datetime.now(timezone.utc) + timedelta(minutes=self.tz_offset_minutes)
        return user_now.date()

    def _get_tz_offset_for_date(self, local_date: date) -> timedelta:
        """Get timezone offset for a local date (DST-aware if timezone is provided)."""
        if self.timezone:
            try:
                from zoneinfo import ZoneInfo

                user_tz = ZoneInfo(self.timezone)
                local_midnight = datetime.combine(local_date, time.min)
                local_aware = local_midnight.replace(tzinfo=user_tz)
                return local_aware.utcoffset() or timedelta(0)
            except Exception:
                pass
        return timedelta(minutes=self.tz_offset_minutes)

    def to_utc_datetime_range(self) -> tuple[datetime, datetime]:
        """Convert to UTC datetime range (half-open)."""
        start_offset = self._get_tz_offset_for_date(self.start_date)
        end_offset = self._get_tz_offset_for_date(self.end_date + timedelta(days=1))

        local_start = datetime.combine(self.start_date, time.min)
        local_end = datetime.combine(self.end_date + timedelta(days=1), time.min)

        start_utc = (local_start - start_offset).replace(tzinfo=timezone.utc)
        end_utc = (local_end - end_offset).replace(tzinfo=timezone.utc)
        return start_utc, end_utc

    def get_complete_utc_dates(
        self,
    ) -> tuple[list[date], tuple[datetime, datetime] | None, tuple[datetime, datetime] | None]:
        """Split into complete UTC days + head/tail boundaries."""
        start_utc, end_utc = self.to_utc_datetime_range()

        if (
            start_utc.hour == 0
            and start_utc.minute == 0
            and start_utc.second == 0
            and start_utc.microsecond == 0
        ):
            first_complete_date = start_utc.date()
            head_boundary = None
        else:
            first_complete_date = start_utc.date() + timedelta(days=1)
            head_boundary = (
                start_utc,
                datetime.combine(first_complete_date, time.min, tzinfo=timezone.utc),
            )

        if (
            end_utc.hour == 0
            and end_utc.minute == 0
            and end_utc.second == 0
            and end_utc.microsecond == 0
        ):
            last_complete_date = end_utc.date() - timedelta(days=1)
            tail_boundary = None
        else:
            last_complete_date = end_utc.date() - timedelta(days=1)
            tail_start = datetime.combine(end_utc.date(), time.min, tzinfo=timezone.utc)
            tail_boundary = (tail_start, end_utc)

        complete_dates = []
        if first_complete_date <= last_complete_date:
            current = first_complete_date
            while current <= last_complete_date:
                complete_dates.append(current)
                current += timedelta(days=1)

        return complete_dates, head_boundary, tail_boundary

    def get_local_day_hours(self) -> list[tuple[date, datetime, datetime]]:
        """Return local-day mapped UTC ranges for time-series."""
        result = []
        current_date = self.start_date
        while current_date <= self.end_date:
            offset = self._get_tz_offset_for_date(current_date)
            local_start = datetime.combine(current_date, time.min)
            local_end = datetime.combine(current_date + timedelta(days=1), time.min)
            day_start_utc = (local_start - offset).replace(tzinfo=timezone.utc)
            day_end_utc = (
                local_end - self._get_tz_offset_for_date(current_date + timedelta(days=1))
            ).replace(tzinfo=timezone.utc)
            result.append((current_date, day_start_utc, day_end_utc))
            current_date += timedelta(days=1)
        return result


def split_time_range_for_hourly(start_utc: datetime, end_utc: datetime) -> tuple[
    tuple[datetime, datetime] | None,
    list[datetime],
    tuple[datetime, datetime] | None,
]:
    """Split into head fragment, complete hours, tail fragment."""
    first_hour = start_utc.replace(minute=0, second=0, microsecond=0)
    if start_utc > first_hour:
        first_hour += timedelta(hours=1)
        head_fragment = (start_utc, first_hour) if first_hour <= end_utc else None
    else:
        head_fragment = None
        first_hour = start_utc

    last_hour = end_utc.replace(minute=0, second=0, microsecond=0)
    if end_utc > last_hour:
        tail_fragment = (last_hour, end_utc) if last_hour >= first_hour else None
    else:
        tail_fragment = None
        last_hour = end_utc

    complete_hours = []
    current = first_hour
    while current < last_hour:
        complete_hours.append(current)
        current += timedelta(hours=1)

    return head_fragment, complete_hours, tail_fragment
