mod memory;

#[allow(unused_imports)]
pub(crate) use aether_data_contracts::repository::candidates::{
    build_decision_trace, derive_request_candidate_final_status,
    request_candidate_lifecycle_would_regress, DecisionTrace, DecisionTraceCandidate,
    PublicHealthStatusCount, PublicHealthTimelineBucket, RequestCandidateFinalStatus,
    RequestCandidateReadRepository, RequestCandidateRepository, RequestCandidateStatus,
    RequestCandidateTrace, RequestCandidateWriteRepository, StoredRequestCandidate,
    UpsertRequestCandidateRecord,
};
#[cfg(feature = "postgres")]
pub use aether_data_postgres::SqlxRequestCandidateReadRepository;
pub use memory::InMemoryRequestCandidateRepository;
