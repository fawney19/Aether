use std::sync::LazyLock;

use chrono::{DateTime, LocalResult, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;
use tracing::warn;

pub(crate) const DEFAULT_APP_TIMEZONE: &str = "Asia/Shanghai";

static APP_TIMEZONE: LazyLock<Tz> = LazyLock::new(|| {
    let configured = std::env::var("APP_TIMEZONE")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_APP_TIMEZONE.to_string());
    configured.parse().unwrap_or_else(|_| {
        warn!(
            timezone = %configured,
            fallback = DEFAULT_APP_TIMEZONE,
            "gateway APP_TIMEZONE invalid; falling back"
        );
        DEFAULT_APP_TIMEZONE
            .parse()
            .expect("default application timezone should parse")
    })
});

pub(crate) fn app_timezone() -> Tz {
    *APP_TIMEZONE
}

pub(crate) fn local_day_window(
    now_utc: DateTime<Utc>,
    timezone: Tz,
) -> (NaiveDate, DateTime<Utc>, DateTime<Utc>) {
    let local_date = now_utc.with_timezone(&timezone).date_naive();
    let next_date = local_date
        .succ_opt()
        .expect("application local date should have a successor");
    (
        local_date,
        local_midnight_utc(local_date, timezone),
        local_midnight_utc(next_date, timezone),
    )
}

fn local_midnight_utc(date: NaiveDate, timezone: Tz) -> DateTime<Utc> {
    let midnight = date
        .and_hms_opt(0, 0, 0)
        .expect("local midnight should be valid");
    match timezone.from_local_datetime(&midnight) {
        LocalResult::Single(value) => value.with_timezone(&Utc),
        LocalResult::Ambiguous(first, second) => first.min(second).with_timezone(&Utc),
        LocalResult::None => {
            for minute in 1..=180 {
                let candidate = midnight + chrono::Duration::minutes(minute);
                match timezone.from_local_datetime(&candidate) {
                    LocalResult::Single(value) => return value.with_timezone(&Utc),
                    LocalResult::Ambiguous(first, second) => {
                        return first.min(second).with_timezone(&Utc)
                    }
                    LocalResult::None => {}
                }
            }
            panic!("local day start should resolve within three hours")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_timezone_uses_shanghai_natural_day() {
        let timezone: Tz = DEFAULT_APP_TIMEZONE.parse().unwrap();
        let (_, start, end) = local_day_window(
            DateTime::parse_from_rfc3339("2026-08-03T12:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            timezone,
        );
        assert_eq!(start.to_rfc3339(), "2026-08-02T16:00:00+00:00");
        assert_eq!(end.to_rfc3339(), "2026-08-03T16:00:00+00:00");
    }

    #[test]
    fn dst_days_use_natural_local_midnights() {
        let timezone: Tz = "America/New_York".parse().unwrap();
        let spring = local_day_window(
            DateTime::parse_from_rfc3339("2026-03-08T12:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            timezone,
        );
        assert_eq!((spring.2 - spring.1).num_hours(), 23);
        let fall = local_day_window(
            DateTime::parse_from_rfc3339("2026-11-01T12:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            timezone,
        );
        assert_eq!((fall.2 - fall.1).num_hours(), 25);
    }
}
