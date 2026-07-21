#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AdmissionMetricsSnapshot {
    pub admitted_total: u64,
    pub rejected_total: u64,
    pub saturated_total: u64,
    pub queue_deadline_exceeded_total: u64,
}
