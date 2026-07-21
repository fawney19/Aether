//! PostgreSQL repositories, pool/transaction primitives, leases, and migrations.

mod announcements;
mod audit;
mod auth;
mod auth_modules;
mod background_tasks;
mod billing;
mod candidate_selection;
mod candidates;
mod error;
mod gemini_file_mappings;
mod global_models;
mod lease;
mod management_tokens;
mod migrations;
mod oauth_providers;
mod pool;
mod pool_scores;
mod provider_catalog;
mod proxy_nodes;
mod quota;
mod routing_profiles;
mod settlement;
mod tx;
mod types;
mod usage;
mod users;
mod video_tasks;
mod wallet;

pub use aether_data_contracts::{DataLayerError, PostgresPoolConfig};
pub use announcements::SqlxAnnouncementReadRepository;
pub use audit::PostgresAuditLogReadRepository;
pub use auth::SqlxAuthApiKeySnapshotReadRepository;
pub use auth_modules::{SqlxAuthModuleReadRepository, SqlxAuthModuleRepository};
pub use background_tasks::SqlxBackgroundTaskRepository;
pub use billing::SqlxBillingReadRepository;
pub use candidate_selection::SqlxMinimalCandidateSelectionReadRepository;
pub use candidates::SqlxRequestCandidateReadRepository;
pub use gemini_file_mappings::SqlxGeminiFileMappingRepository;
pub use global_models::SqlxGlobalModelReadRepository;
pub use lease::{
    build_postgres_lease_claim_sql, build_postgres_lease_release_sql,
    build_postgres_lease_renew_sql, PostgresLeaseClaimOptions, PostgresLeaseClaimSpec,
    PostgresLeaseRunner, PostgresLeaseRunnerConfig,
};
pub use management_tokens::SqlxManagementTokenRepository;
pub use migrations::{
    all_up_migrations, pending_migrations, pending_migrations_from_applied,
    prepare_database_for_startup, prepare_database_for_startup_with_bootstrap, run_migrations,
    run_migrations_with_bootstrap, BootstrapFuture, PostgresMigrationBootstrap, POSTGRES_MIGRATOR,
};
pub use oauth_providers::SqlxOAuthProviderRepository;
pub use pool::{PostgresPool, PostgresPoolFactory};
pub use pool_scores::PostgresPoolMemberScoreRepository;
pub use provider_catalog::SqlxProviderCatalogReadRepository;
pub use proxy_nodes::SqlxProxyNodeRepository;
pub use quota::SqlxProviderQuotaRepository;
pub use routing_profiles::PostgresRoutingGroupRepository;
pub use settlement::SqlxSettlementRepository;
pub use tx::{
    PostgresTransaction, PostgresTransactionOptions, PostgresTransactionRunner, TransactionMode,
};
pub use types::DatabaseRecordId;
pub use usage::{cleanup, SqlxUsageReadRepository};
pub use users::SqlxUserReadRepository;
pub use video_tasks::{SqlxVideoTaskReadRepository, SqlxVideoTaskRepository};
pub use wallet::SqlxWalletRepository;
