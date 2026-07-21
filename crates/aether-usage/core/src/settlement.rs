#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettlementInput {
    pub request_id: String,
    pub event_sequence: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettlementDisposition {
    Apply,
    AlreadyApplied,
    RejectOutOfOrder,
}
