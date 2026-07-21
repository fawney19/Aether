#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) struct PercentileSummary {
    pub(super) p50: Option<i64>,
    pub(super) p90: Option<i64>,
    pub(super) p99: Option<i64>,
}

pub(super) fn percentile_ms_to_i64(value: Option<f64>) -> Option<i64> {
    value.and_then(|raw| raw.is_finite().then_some(raw.floor() as i64))
}
