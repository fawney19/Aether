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
#[cfg(feature = "mysql")]
pub use aether_data_mysql::MysqlRequestCandidateRepository;
#[cfg(feature = "postgres")]
pub use aether_data_postgres::SqlxRequestCandidateReadRepository;
#[cfg(feature = "sqlite")]
pub use aether_data_sqlite::SqliteRequestCandidateRepository;
pub use memory::InMemoryRequestCandidateRepository;
