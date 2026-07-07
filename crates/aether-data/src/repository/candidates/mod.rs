mod memory;
mod mysql;
mod postgres;
mod sqlite;

#[allow(unused_imports)]
pub(crate) use aether_data_contracts::repository::candidates::{
    build_decision_trace, derive_request_candidate_final_status, DecisionTrace,
    DecisionTraceCandidate, PublicHealthStatusCount, PublicHealthTimelineBucket,
    RequestCandidateFinalStatus, RequestCandidateReadRepository, RequestCandidateRepository,
    RequestCandidateStatus, RequestCandidateTrace, RequestCandidateWriteRepository,
    StoredRequestCandidate, UpsertRequestCandidateRecord,
};
pub use memory::InMemoryRequestCandidateRepository;
pub use mysql::MysqlRequestCandidateRepository;
pub use postgres::SqlxRequestCandidateReadRepository;
pub use sqlite::SqliteRequestCandidateRepository;

fn request_candidate_lifecycle_would_regress(
    existing: RequestCandidateStatus,
    incoming: RequestCandidateStatus,
) -> bool {
    matches!(
        existing,
        RequestCandidateStatus::Success
            | RequestCandidateStatus::Failed
            | RequestCandidateStatus::Cancelled
            | RequestCandidateStatus::Skipped
    ) && matches!(
        incoming,
        RequestCandidateStatus::Available
            | RequestCandidateStatus::Unused
            | RequestCandidateStatus::Pending
            | RequestCandidateStatus::Streaming
    ) || existing == RequestCandidateStatus::Streaming
        && incoming == RequestCandidateStatus::Pending
}
