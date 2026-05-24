use chrono::{DateTime, Datelike, TimeZone, Timelike, Utc};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BackupScheduleUnit {
    Hours,
    Days,
    Weeks,
    Months,
}

impl BackupScheduleUnit {
    pub(crate) fn from_config_value(value: &str) -> Option<Self> {
        match value.trim() {
            "hours" => Some(Self::Hours),
            "days" => Some(Self::Days),
            "weeks" => Some(Self::Weeks),
            "months" => Some(Self::Months),
            _ => None,
        }
    }

    fn slot_prefix(self) -> &'static str {
        match self {
            Self::Hours => "hours",
            Self::Days => "days",
            Self::Weeks => "weeks",
            Self::Months => "months",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BackupSchedule {
    pub(crate) unit: BackupScheduleUnit,
    pub(crate) interval: u32,
    pub(crate) minute: u32,
    pub(crate) hour: u32,
    pub(crate) weekday: u32,
    pub(crate) month_day: u32,
}

impl Default for BackupSchedule {
    fn default() -> Self {
        Self {
            unit: BackupScheduleUnit::Days,
            interval: 1,
            minute: 0,
            hour: 3,
            weekday: 1,
            month_day: 1,
        }
    }
}

impl BackupSchedule {
    pub(crate) fn due_slot(&self, now_utc: DateTime<Utc>) -> Option<String> {
        let interval = self.interval.max(1);
        if now_utc.minute() != self.minute {
            return None;
        }

        let due = match self.unit {
            BackupScheduleUnit::Hours => (now_utc.hour() + 8) % interval == self.hour % interval,
            BackupScheduleUnit::Days => {
                now_utc.hour() == self.hour && epoch_day(now_utc) % i64::from(interval) == 0
            }
            BackupScheduleUnit::Weeks => {
                now_utc.hour() == self.hour
                    && now_utc.weekday().number_from_monday() == self.weekday
                    && epoch_week(now_utc) % i64::from(interval) == 0
            }
            BackupScheduleUnit::Months => {
                now_utc.hour() == self.hour
                    && now_utc.day() == self.month_day
                    && month_ordinal(now_utc) % i64::from(interval) == 0
            }
        };
        if !due {
            return None;
        }

        let slot = Utc.from_utc_datetime(&now_utc.date_naive().and_hms_opt(
            now_utc.hour(),
            self.minute,
            0,
        )?);
        Some(format!(
            "{}:{}",
            self.unit.slot_prefix(),
            slot.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        ))
    }
}

fn epoch_day(now_utc: DateTime<Utc>) -> i64 {
    now_utc.timestamp().div_euclid(86_400)
}

fn epoch_week(now_utc: DateTime<Utc>) -> i64 {
    epoch_day(now_utc).div_euclid(7)
}

fn month_ordinal(now_utc: DateTime<Utc>) -> i64 {
    i64::from(now_utc.year()) * 12 + i64::from(now_utc.month0())
}

#[cfg(test)]
mod tests {
    use super::{BackupSchedule, BackupScheduleUnit};

    #[test]
    fn hourly_schedule_returns_stable_slot_once_per_due_hour() {
        let schedule = BackupSchedule {
            unit: BackupScheduleUnit::Hours,
            interval: 6,
            minute: 10,
            hour: 0,
            weekday: 1,
            month_day: 1,
        };
        let now = chrono::DateTime::parse_from_rfc3339("2026-05-24T12:10:30+08:00")
            .unwrap()
            .with_timezone(&chrono::Utc);

        assert_eq!(
            schedule.due_slot(now).as_deref(),
            Some("hours:2026-05-24T04:10:00Z")
        );
    }

    #[test]
    fn daily_schedule_respects_epoch_day_interval() {
        let schedule = BackupSchedule {
            unit: BackupScheduleUnit::Days,
            interval: 2,
            minute: 15,
            hour: 3,
            weekday: 1,
            month_day: 1,
        };
        let due = chrono::DateTime::parse_from_rfc3339("2026-05-23T03:15:45Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let not_due = chrono::DateTime::parse_from_rfc3339("2026-05-24T03:15:45Z")
            .unwrap()
            .with_timezone(&chrono::Utc);

        assert_eq!(
            schedule.due_slot(due).as_deref(),
            Some("days:2026-05-23T03:15:00Z")
        );
        assert_eq!(schedule.due_slot(not_due), None);
    }

    #[test]
    fn weekly_schedule_respects_weekday_and_interval() {
        let schedule = BackupSchedule {
            unit: BackupScheduleUnit::Weeks,
            interval: 2,
            minute: 30,
            hour: 5,
            weekday: 1,
            month_day: 1,
        };
        let due = chrono::DateTime::parse_from_rfc3339("2026-05-25T05:30:59Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let wrong_week = chrono::DateTime::parse_from_rfc3339("2026-05-18T05:30:59Z")
            .unwrap()
            .with_timezone(&chrono::Utc);

        assert_eq!(
            schedule.due_slot(due).as_deref(),
            Some("weeks:2026-05-25T05:30:00Z")
        );
        assert_eq!(schedule.due_slot(wrong_week), None);
    }

    #[test]
    fn monthly_schedule_respects_month_day_and_interval() {
        let schedule = BackupSchedule {
            unit: BackupScheduleUnit::Months,
            interval: 3,
            minute: 45,
            hour: 2,
            weekday: 1,
            month_day: 1,
        };
        let due = chrono::DateTime::parse_from_rfc3339("2026-04-01T02:45:01Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let wrong_month = chrono::DateTime::parse_from_rfc3339("2026-05-01T02:45:01Z")
            .unwrap()
            .with_timezone(&chrono::Utc);

        assert_eq!(
            schedule.due_slot(due).as_deref(),
            Some("months:2026-04-01T02:45:00Z")
        );
        assert_eq!(schedule.due_slot(wrong_month), None);
    }
}
