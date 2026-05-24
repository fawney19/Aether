use chrono::{DateTime, Datelike, NaiveDate, TimeZone, Timelike, Utc};
use chrono_tz::Tz;

const BACKUP_SCHEDULE_DEFAULT_TIMEZONE: &str = "Asia/Shanghai";

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
        let timezone = backup_schedule_timezone();
        let local_now = now_utc.with_timezone(&timezone);
        let interval = self.interval.max(1);
        if local_now.minute() != self.minute {
            return None;
        }

        let due = match self.unit {
            BackupScheduleUnit::Hours => {
                (local_epoch_hour(local_now.date_naive(), local_now.hour()) - i64::from(self.hour))
                    .rem_euclid(i64::from(interval))
                    == 0
            }
            BackupScheduleUnit::Days => {
                local_now.hour() == self.hour
                    && local_epoch_day(local_now.date_naive()) % i64::from(interval) == 0
            }
            BackupScheduleUnit::Weeks => {
                local_now.hour() == self.hour
                    && local_now.weekday().number_from_monday() == self.weekday
                    && local_epoch_week(local_now.date_naive()) % i64::from(interval) == 0
            }
            BackupScheduleUnit::Months => {
                local_now.hour() == self.hour
                    && local_now.day() == self.month_day
                    && month_ordinal(local_now.year(), local_now.month0()) % i64::from(interval)
                        == 0
            }
        };
        if !due {
            return None;
        }

        let slot = Utc.from_utc_datetime(&now_utc.date_naive().and_hms_opt(
            now_utc.hour(),
            now_utc.minute(),
            0,
        )?);
        Some(format!(
            "{}:{}",
            self.unit.slot_prefix(),
            slot.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        ))
    }
}

pub(crate) fn backup_schedule_timezone() -> Tz {
    std::env::var("APP_TIMEZONE")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .as_deref()
        .unwrap_or(BACKUP_SCHEDULE_DEFAULT_TIMEZONE)
        .parse()
        .unwrap_or(chrono_tz::Asia::Shanghai)
}

fn local_epoch_day(date: NaiveDate) -> i64 {
    let epoch = NaiveDate::from_ymd_opt(1970, 1, 1).expect("unix epoch date should be valid");
    date.signed_duration_since(epoch).num_days()
}

fn local_epoch_week(date: NaiveDate) -> i64 {
    local_epoch_day(date).div_euclid(7)
}

fn local_epoch_hour(date: NaiveDate, hour: u32) -> i64 {
    local_epoch_day(date) * 24 + i64::from(hour)
}

fn month_ordinal(year: i32, month0: u32) -> i64 {
    i64::from(year) * 12 + i64::from(month0)
}

#[cfg(test)]
mod tests {
    use super::{BackupSchedule, BackupScheduleUnit};
    use std::ffi::OsString;
    use std::sync::{Mutex, OnceLock};

    struct TimezoneEnvGuard {
        previous: Option<OsString>,
    }

    impl TimezoneEnvGuard {
        fn set(value: Option<&str>) -> Self {
            let previous = std::env::var_os("APP_TIMEZONE");
            match value {
                Some(value) => std::env::set_var("APP_TIMEZONE", value),
                None => std::env::remove_var("APP_TIMEZONE"),
            }
            Self { previous }
        }
    }

    impl Drop for TimezoneEnvGuard {
        fn drop(&mut self) {
            match &self.previous {
                Some(value) => std::env::set_var("APP_TIMEZONE", value),
                None => std::env::remove_var("APP_TIMEZONE"),
            }
        }
    }

    fn timezone_env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|err| err.into_inner())
    }

    #[test]
    fn hourly_schedule_returns_stable_slot_once_per_due_hour() {
        let _guard = timezone_env_lock();
        let _env = TimezoneEnvGuard::set(None);
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
    fn hourly_schedule_interval_does_not_reset_at_midnight() {
        let _guard = timezone_env_lock();
        let _env = TimezoneEnvGuard::set(None);
        let schedule = BackupSchedule {
            unit: BackupScheduleUnit::Hours,
            interval: 5,
            minute: 10,
            hour: 0,
            weekday: 1,
            month_day: 1,
        };
        let midnight = chrono::DateTime::parse_from_rfc3339("2026-05-25T00:10:30+08:00")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let due_after_midnight = chrono::DateTime::parse_from_rfc3339("2026-05-25T03:10:30+08:00")
            .unwrap()
            .with_timezone(&chrono::Utc);

        assert_eq!(schedule.due_slot(midnight), None);
        assert_eq!(
            schedule.due_slot(due_after_midnight).as_deref(),
            Some("hours:2026-05-24T19:10:00Z")
        );
    }

    #[test]
    fn hourly_schedule_supports_intervals_longer_than_one_day() {
        let _guard = timezone_env_lock();
        let _env = TimezoneEnvGuard::set(None);
        let schedule = BackupSchedule {
            unit: BackupScheduleUnit::Hours,
            interval: 25,
            minute: 10,
            hour: 0,
            weekday: 1,
            month_day: 1,
        };
        let daily_midnight = chrono::DateTime::parse_from_rfc3339("2026-05-25T00:10:30+08:00")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let due_after_25_hours = chrono::DateTime::parse_from_rfc3339("2026-05-25T23:10:30+08:00")
            .unwrap()
            .with_timezone(&chrono::Utc);

        assert_eq!(schedule.due_slot(daily_midnight), None);
        assert_eq!(
            schedule.due_slot(due_after_25_hours).as_deref(),
            Some("hours:2026-05-25T15:10:00Z")
        );
    }

    #[test]
    fn daily_schedule_uses_maintenance_timezone_for_hour_and_interval() {
        let _guard = timezone_env_lock();
        let _env = TimezoneEnvGuard::set(None);
        let schedule = BackupSchedule {
            unit: BackupScheduleUnit::Days,
            interval: 2,
            minute: 15,
            hour: 3,
            weekday: 1,
            month_day: 1,
        };
        let due = chrono::DateTime::parse_from_rfc3339("2026-05-23T03:15:45+08:00")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let not_due = chrono::DateTime::parse_from_rfc3339("2026-05-24T03:15:45+08:00")
            .unwrap()
            .with_timezone(&chrono::Utc);

        assert_eq!(
            schedule.due_slot(due).as_deref(),
            Some("days:2026-05-22T19:15:00Z")
        );
        assert_eq!(schedule.due_slot(not_due), None);
    }

    #[test]
    fn weekly_schedule_uses_maintenance_timezone_for_weekday_and_interval() {
        let _guard = timezone_env_lock();
        let _env = TimezoneEnvGuard::set(None);
        let schedule = BackupSchedule {
            unit: BackupScheduleUnit::Weeks,
            interval: 2,
            minute: 30,
            hour: 5,
            weekday: 1,
            month_day: 1,
        };
        let due = chrono::DateTime::parse_from_rfc3339("2026-05-25T05:30:59+08:00")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let wrong_week = chrono::DateTime::parse_from_rfc3339("2026-05-18T05:30:59+08:00")
            .unwrap()
            .with_timezone(&chrono::Utc);

        assert_eq!(
            schedule.due_slot(due).as_deref(),
            Some("weeks:2026-05-24T21:30:00Z")
        );
        assert_eq!(schedule.due_slot(wrong_week), None);
    }

    #[test]
    fn monthly_schedule_uses_maintenance_timezone_for_month_day_and_interval() {
        let _guard = timezone_env_lock();
        let _env = TimezoneEnvGuard::set(None);
        let schedule = BackupSchedule {
            unit: BackupScheduleUnit::Months,
            interval: 3,
            minute: 45,
            hour: 2,
            weekday: 1,
            month_day: 1,
        };
        let due = chrono::DateTime::parse_from_rfc3339("2026-04-01T02:45:01+08:00")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let wrong_month = chrono::DateTime::parse_from_rfc3339("2026-05-01T02:45:01+08:00")
            .unwrap()
            .with_timezone(&chrono::Utc);

        assert_eq!(
            schedule.due_slot(due).as_deref(),
            Some("months:2026-03-31T18:45:00Z")
        );
        assert_eq!(schedule.due_slot(wrong_month), None);
    }

    #[test]
    fn hourly_schedule_uses_utc_when_app_timezone_is_utc() {
        let _guard = timezone_env_lock();
        let _env = TimezoneEnvGuard::set(Some("UTC"));
        let schedule = BackupSchedule {
            unit: BackupScheduleUnit::Hours,
            interval: 6,
            minute: 10,
            hour: 4,
            weekday: 1,
            month_day: 1,
        };
        let now = chrono::DateTime::parse_from_rfc3339("2026-05-24T04:10:30Z")
            .unwrap()
            .with_timezone(&chrono::Utc);

        assert_eq!(
            schedule.due_slot(now).as_deref(),
            Some("hours:2026-05-24T04:10:00Z")
        );
    }

    #[test]
    fn daily_schedule_slot_uses_actual_instant_for_non_whole_hour_timezone() {
        let _guard = timezone_env_lock();
        let _env = TimezoneEnvGuard::set(Some("Asia/Kolkata"));
        let schedule = BackupSchedule {
            unit: BackupScheduleUnit::Days,
            interval: 1,
            minute: 30,
            hour: 3,
            weekday: 1,
            month_day: 1,
        };
        let now = chrono::DateTime::parse_from_rfc3339("2026-05-24T03:30:45+05:30")
            .unwrap()
            .with_timezone(&chrono::Utc);

        assert_eq!(
            schedule.due_slot(now).as_deref(),
            Some("days:2026-05-23T22:00:00Z")
        );
    }
}
