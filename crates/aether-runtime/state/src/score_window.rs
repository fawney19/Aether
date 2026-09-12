/// Maximum number of members examined by one server-side window aggregation.
pub const SCORE_WINDOW_AGGREGATION_MEMBER_LIMIT: usize = 512;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ScoreWindowU64Stats {
    pub sum: u64,
    pub positive_count: u64,
}

impl ScoreWindowU64Stats {
    /// Members encode the value after their final colon. Invalid or zero values
    /// do not contribute to the positive sample count.
    pub fn from_members<'a>(members: impl IntoIterator<Item = &'a str>) -> Self {
        let mut stats = Self::default();
        for member in members {
            let value = member
                .rsplit_once(':')
                .and_then(|(_, suffix)| suffix.parse::<u64>().ok())
                .unwrap_or(0);
            stats.sum = stats.sum.saturating_add(value);
            stats.positive_count += u64::from(value > 0);
        }
        stats
    }
}
