use std::fmt;
use std::sync::Arc;

#[cfg(feature = "mysql")]
use super::MysqlBackend;
#[cfg(feature = "postgres")]
use super::PostgresBackend;
#[cfg(feature = "sqlite")]
use super::SqliteBackend;
use crate::repository::announcements::AnnouncementReadRepository;
use crate::repository::audit::AuditLogReadRepository;
use crate::repository::auth::AuthApiKeyReadRepository;
use crate::repository::auth_modules::AuthModuleReadRepository;
use crate::repository::background_tasks::BackgroundTaskReadRepository;
use crate::repository::billing::BillingReadRepository;
use crate::repository::candidate_selection::MinimalCandidateSelectionReadRepository;
use crate::repository::candidates::RequestCandidateReadRepository;
use crate::repository::gemini_file_mappings::GeminiFileMappingReadRepository;
use crate::repository::global_models::GlobalModelReadRepository;
use crate::repository::management_tokens::ManagementTokenReadRepository;
use crate::repository::oauth_providers::OAuthProviderReadRepository;
use crate::repository::pool_scores::PoolScoreReadRepository;
use crate::repository::provider_catalog::ProviderCatalogReadRepository;
use crate::repository::proxy_nodes::ProxyNodeReadRepository;
use crate::repository::quota::ProviderQuotaReadRepository;
use crate::repository::routing_profiles::RoutingGroupReadRepository;
use crate::repository::usage::UsageReadRepository;
use crate::repository::users::UserReadRepository;
use crate::repository::video_tasks::VideoTaskReadRepository;
use crate::repository::wallet::WalletReadRepository;

#[derive(Clone, Default)]
pub struct DataReadRepositories {
    announcements: Option<Arc<dyn AnnouncementReadRepository>>,
    audit_logs: Option<Arc<dyn AuditLogReadRepository>>,
    auth_api_keys: Option<Arc<dyn AuthApiKeyReadRepository>>,
    auth_modules: Option<Arc<dyn AuthModuleReadRepository>>,
    background_tasks: Option<Arc<dyn BackgroundTaskReadRepository>>,
    billing: Option<Arc<dyn BillingReadRepository>>,
    gemini_file_mappings: Option<Arc<dyn GeminiFileMappingReadRepository>>,
    global_models: Option<Arc<dyn GlobalModelReadRepository>>,
    management_tokens: Option<Arc<dyn ManagementTokenReadRepository>>,
    oauth_providers: Option<Arc<dyn OAuthProviderReadRepository>>,
    pool_scores: Option<Arc<dyn PoolScoreReadRepository>>,
    proxy_nodes: Option<Arc<dyn ProxyNodeReadRepository>>,
    minimal_candidate_selection: Option<Arc<dyn MinimalCandidateSelectionReadRepository>>,
    request_candidates: Option<Arc<dyn RequestCandidateReadRepository>>,
    provider_catalog: Option<Arc<dyn ProviderCatalogReadRepository>>,
    provider_quotas: Option<Arc<dyn ProviderQuotaReadRepository>>,
    routing_groups: Option<Arc<dyn RoutingGroupReadRepository>>,
    usage: Option<Arc<dyn UsageReadRepository>>,
    users: Option<Arc<dyn UserReadRepository>>,
    video_tasks: Option<Arc<dyn VideoTaskReadRepository>>,
    wallets: Option<Arc<dyn WalletReadRepository>>,
}

impl fmt::Debug for DataReadRepositories {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DataReadRepositories")
            .field("has_auth_api_keys", &self.auth_api_keys.is_some())
            .field("has_announcements", &self.announcements.is_some())
            .field("has_audit_logs", &self.audit_logs.is_some())
            .field("has_auth_modules", &self.auth_modules.is_some())
            .field("has_background_tasks", &self.background_tasks.is_some())
            .field("has_billing", &self.billing.is_some())
            .field(
                "has_gemini_file_mappings",
                &self.gemini_file_mappings.is_some(),
            )
            .field("has_global_models", &self.global_models.is_some())
            .field("has_management_tokens", &self.management_tokens.is_some())
            .field("has_oauth_providers", &self.oauth_providers.is_some())
            .field("has_pool_scores", &self.pool_scores.is_some())
            .field("has_proxy_nodes", &self.proxy_nodes.is_some())
            .field(
                "has_minimal_candidate_selection",
                &self.minimal_candidate_selection.is_some(),
            )
            .field("has_request_candidates", &self.request_candidates.is_some())
            .field("has_provider_catalog", &self.provider_catalog.is_some())
            .field("has_provider_quotas", &self.provider_quotas.is_some())
            .field("has_routing_groups", &self.routing_groups.is_some())
            .field("has_usage", &self.usage.is_some())
            .field("has_users", &self.users.is_some())
            .field("has_video_tasks", &self.video_tasks.is_some())
            .field("has_wallets", &self.wallets.is_some())
            .finish()
    }
}

impl DataReadRepositories {
    pub(crate) fn from_backends(
        #[cfg(feature = "postgres")] postgres: Option<&PostgresBackend>,
        #[cfg(feature = "mysql")] mysql: Option<&MysqlBackend>,
        #[cfg(feature = "sqlite")] sqlite: Option<&SqliteBackend>,
    ) -> Self {
        let mut repositories = Self::default();
        #[cfg(feature = "postgres")]
        if let Some(postgres) = postgres {
            repositories.install_postgres(postgres);
        }
        #[cfg(feature = "mysql")]
        if let Some(mysql) = mysql {
            repositories.install_mysql(mysql);
        }
        #[cfg(feature = "sqlite")]
        if let Some(sqlite) = sqlite {
            repositories.install_sqlite(sqlite);
        }
        repositories
    }

    #[cfg(feature = "postgres")]
    fn install_postgres(&mut self, backend: &PostgresBackend) {
        if self.announcements.is_none() {
            self.announcements = Some(PostgresBackend::announcement_read_repository(backend));
        }
        if self.audit_logs.is_none() {
            self.audit_logs = Some(PostgresBackend::audit_log_read_repository(backend));
        }
        if self.auth_api_keys.is_none() {
            self.auth_api_keys = Some(PostgresBackend::auth_api_key_read_repository(backend));
        }
        if self.auth_modules.is_none() {
            self.auth_modules = Some(PostgresBackend::auth_module_read_repository(backend));
        }
        if self.background_tasks.is_none() {
            self.background_tasks = Some(PostgresBackend::background_task_read_repository(backend));
        }
        if self.billing.is_none() {
            self.billing = Some(PostgresBackend::billing_read_repository(backend));
        }
        if self.gemini_file_mappings.is_none() {
            self.gemini_file_mappings = Some(PostgresBackend::gemini_file_mapping_read_repository(
                backend,
            ));
        }
        if self.global_models.is_none() {
            self.global_models = Some(PostgresBackend::global_model_read_repository(backend));
        }
        if self.management_tokens.is_none() {
            self.management_tokens =
                Some(PostgresBackend::management_token_read_repository(backend));
        }
        if self.oauth_providers.is_none() {
            self.oauth_providers = Some(PostgresBackend::oauth_provider_read_repository(backend));
        }
        if self.pool_scores.is_none() {
            self.pool_scores = Some(PostgresBackend::pool_score_read_repository(backend));
        }
        if self.proxy_nodes.is_none() {
            self.proxy_nodes = Some(PostgresBackend::proxy_node_read_repository(backend));
        }
        if self.minimal_candidate_selection.is_none() {
            self.minimal_candidate_selection =
                Some(PostgresBackend::minimal_candidate_selection_read_repository(backend));
        }
        if self.request_candidates.is_none() {
            self.request_candidates =
                Some(PostgresBackend::request_candidate_read_repository(backend));
        }
        if self.provider_catalog.is_none() {
            self.provider_catalog =
                Some(PostgresBackend::provider_catalog_read_repository(backend));
        }
        if self.provider_quotas.is_none() {
            self.provider_quotas = Some(PostgresBackend::provider_quota_read_repository(backend));
        }
        if self.routing_groups.is_none() {
            self.routing_groups = Some(PostgresBackend::routing_group_read_repository(backend));
        }
        if self.usage.is_none() {
            self.usage = Some(PostgresBackend::usage_read_repository(backend));
        }
        if self.users.is_none() {
            self.users = Some(PostgresBackend::user_read_repository(backend));
        }
        if self.video_tasks.is_none() {
            self.video_tasks = Some(PostgresBackend::video_task_read_repository(backend));
        }
        if self.wallets.is_none() {
            self.wallets = Some(PostgresBackend::wallet_read_repository(backend));
        }
    }

    #[cfg(feature = "mysql")]
    fn install_mysql(&mut self, backend: &MysqlBackend) {
        if self.announcements.is_none() {
            self.announcements = Some(MysqlBackend::announcement_read_repository(backend));
        }
        if self.audit_logs.is_none() {
            self.audit_logs = Some(MysqlBackend::audit_log_read_repository(backend));
        }
        if self.auth_api_keys.is_none() {
            self.auth_api_keys = Some(MysqlBackend::auth_api_key_read_repository(backend));
        }
        if self.auth_modules.is_none() {
            self.auth_modules = Some(MysqlBackend::auth_module_read_repository(backend));
        }
        if self.background_tasks.is_none() {
            self.background_tasks = Some(MysqlBackend::background_task_read_repository(backend));
        }
        if self.billing.is_none() {
            self.billing = Some(MysqlBackend::billing_read_repository(backend));
        }
        if self.gemini_file_mappings.is_none() {
            self.gemini_file_mappings =
                Some(MysqlBackend::gemini_file_mapping_read_repository(backend));
        }
        if self.global_models.is_none() {
            self.global_models = Some(MysqlBackend::global_model_read_repository(backend));
        }
        if self.management_tokens.is_none() {
            self.management_tokens = Some(MysqlBackend::management_token_read_repository(backend));
        }
        if self.oauth_providers.is_none() {
            self.oauth_providers = Some(MysqlBackend::oauth_provider_read_repository(backend));
        }
        if self.pool_scores.is_none() {
            self.pool_scores = Some(MysqlBackend::pool_score_read_repository(backend));
        }
        if self.proxy_nodes.is_none() {
            self.proxy_nodes = Some(MysqlBackend::proxy_node_read_repository(backend));
        }
        if self.minimal_candidate_selection.is_none() {
            self.minimal_candidate_selection = Some(
                MysqlBackend::minimal_candidate_selection_read_repository(backend),
            );
        }
        if self.request_candidates.is_none() {
            self.request_candidates =
                Some(MysqlBackend::request_candidate_read_repository(backend));
        }
        if self.provider_catalog.is_none() {
            self.provider_catalog = Some(MysqlBackend::provider_catalog_read_repository(backend));
        }
        if self.provider_quotas.is_none() {
            self.provider_quotas = Some(MysqlBackend::provider_quota_read_repository(backend));
        }
        if self.routing_groups.is_none() {
            self.routing_groups = Some(MysqlBackend::routing_group_read_repository(backend));
        }
        if self.usage.is_none() {
            self.usage = Some(MysqlBackend::usage_read_repository(backend));
        }
        if self.users.is_none() {
            self.users = Some(MysqlBackend::user_read_repository(backend));
        }
        if self.video_tasks.is_none() {
            self.video_tasks = Some(MysqlBackend::video_task_read_repository(backend));
        }
        if self.wallets.is_none() {
            self.wallets = Some(MysqlBackend::wallet_read_repository(backend));
        }
    }

    #[cfg(feature = "sqlite")]
    fn install_sqlite(&mut self, backend: &SqliteBackend) {
        if self.announcements.is_none() {
            self.announcements = Some(SqliteBackend::announcement_read_repository(backend));
        }
        if self.audit_logs.is_none() {
            self.audit_logs = Some(SqliteBackend::audit_log_read_repository(backend));
        }
        if self.auth_api_keys.is_none() {
            self.auth_api_keys = Some(SqliteBackend::auth_api_key_read_repository(backend));
        }
        if self.auth_modules.is_none() {
            self.auth_modules = Some(SqliteBackend::auth_module_read_repository(backend));
        }
        if self.background_tasks.is_none() {
            self.background_tasks = Some(SqliteBackend::background_task_read_repository(backend));
        }
        if self.billing.is_none() {
            self.billing = Some(SqliteBackend::billing_read_repository(backend));
        }
        if self.gemini_file_mappings.is_none() {
            self.gemini_file_mappings =
                Some(SqliteBackend::gemini_file_mapping_read_repository(backend));
        }
        if self.global_models.is_none() {
            self.global_models = Some(SqliteBackend::global_model_read_repository(backend));
        }
        if self.management_tokens.is_none() {
            self.management_tokens = Some(SqliteBackend::management_token_read_repository(backend));
        }
        if self.oauth_providers.is_none() {
            self.oauth_providers = Some(SqliteBackend::oauth_provider_read_repository(backend));
        }
        if self.pool_scores.is_none() {
            self.pool_scores = Some(SqliteBackend::pool_score_read_repository(backend));
        }
        if self.proxy_nodes.is_none() {
            self.proxy_nodes = Some(SqliteBackend::proxy_node_read_repository(backend));
        }
        if self.minimal_candidate_selection.is_none() {
            self.minimal_candidate_selection = Some(
                SqliteBackend::minimal_candidate_selection_read_repository(backend),
            );
        }
        if self.request_candidates.is_none() {
            self.request_candidates =
                Some(SqliteBackend::request_candidate_read_repository(backend));
        }
        if self.provider_catalog.is_none() {
            self.provider_catalog = Some(SqliteBackend::provider_catalog_read_repository(backend));
        }
        if self.provider_quotas.is_none() {
            self.provider_quotas = Some(SqliteBackend::provider_quota_read_repository(backend));
        }
        if self.routing_groups.is_none() {
            self.routing_groups = Some(SqliteBackend::routing_group_read_repository(backend));
        }
        if self.usage.is_none() {
            self.usage = Some(SqliteBackend::usage_read_repository(backend));
        }
        if self.users.is_none() {
            self.users = Some(SqliteBackend::user_read_repository(backend));
        }
        if self.video_tasks.is_none() {
            self.video_tasks = Some(SqliteBackend::video_task_read_repository(backend));
        }
        if self.wallets.is_none() {
            self.wallets = Some(SqliteBackend::wallet_read_repository(backend));
        }
    }
    #[cfg(test)]
    #[cfg(feature = "postgres")]
    pub(crate) fn from_postgres(postgres: Option<&PostgresBackend>) -> Self {
        Self::from_backends(
            postgres,
            #[cfg(feature = "mysql")]
            None,
            #[cfg(feature = "sqlite")]
            None,
        )
    }

    pub fn auth_api_keys(&self) -> Option<Arc<dyn AuthApiKeyReadRepository>> {
        self.auth_api_keys.clone()
    }

    pub fn announcements(&self) -> Option<Arc<dyn AnnouncementReadRepository>> {
        self.announcements.clone()
    }

    pub fn audit_logs(&self) -> Option<Arc<dyn AuditLogReadRepository>> {
        self.audit_logs.clone()
    }

    pub fn auth_modules(&self) -> Option<Arc<dyn AuthModuleReadRepository>> {
        self.auth_modules.clone()
    }

    pub fn background_tasks(&self) -> Option<Arc<dyn BackgroundTaskReadRepository>> {
        self.background_tasks.clone()
    }

    pub fn billing(&self) -> Option<Arc<dyn BillingReadRepository>> {
        self.billing.clone()
    }

    pub fn gemini_file_mappings(&self) -> Option<Arc<dyn GeminiFileMappingReadRepository>> {
        self.gemini_file_mappings.clone()
    }

    pub fn global_models(&self) -> Option<Arc<dyn GlobalModelReadRepository>> {
        self.global_models.clone()
    }

    pub fn management_tokens(&self) -> Option<Arc<dyn ManagementTokenReadRepository>> {
        self.management_tokens.clone()
    }

    pub fn oauth_providers(&self) -> Option<Arc<dyn OAuthProviderReadRepository>> {
        self.oauth_providers.clone()
    }

    pub fn pool_scores(&self) -> Option<Arc<dyn PoolScoreReadRepository>> {
        self.pool_scores.clone()
    }

    pub fn proxy_nodes(&self) -> Option<Arc<dyn ProxyNodeReadRepository>> {
        self.proxy_nodes.clone()
    }

    pub fn minimal_candidate_selection(
        &self,
    ) -> Option<Arc<dyn MinimalCandidateSelectionReadRepository>> {
        self.minimal_candidate_selection.clone()
    }

    pub fn request_candidates(&self) -> Option<Arc<dyn RequestCandidateReadRepository>> {
        self.request_candidates.clone()
    }

    pub fn provider_catalog(&self) -> Option<Arc<dyn ProviderCatalogReadRepository>> {
        self.provider_catalog.clone()
    }

    pub fn provider_quotas(&self) -> Option<Arc<dyn ProviderQuotaReadRepository>> {
        self.provider_quotas.clone()
    }

    pub fn routing_groups(&self) -> Option<Arc<dyn RoutingGroupReadRepository>> {
        self.routing_groups.clone()
    }

    pub fn usage(&self) -> Option<Arc<dyn UsageReadRepository>> {
        self.usage.clone()
    }

    pub fn users(&self) -> Option<Arc<dyn UserReadRepository>> {
        self.users.clone()
    }

    pub fn video_tasks(&self) -> Option<Arc<dyn VideoTaskReadRepository>> {
        self.video_tasks.clone()
    }

    pub fn wallets(&self) -> Option<Arc<dyn WalletReadRepository>> {
        self.wallets.clone()
    }

    pub fn has_any(&self) -> bool {
        self.auth_api_keys.is_some()
            || self.announcements.is_some()
            || self.audit_logs.is_some()
            || self.auth_modules.is_some()
            || self.background_tasks.is_some()
            || self.billing.is_some()
            || self.gemini_file_mappings.is_some()
            || self.global_models.is_some()
            || self.management_tokens.is_some()
            || self.oauth_providers.is_some()
            || self.pool_scores.is_some()
            || self.proxy_nodes.is_some()
            || self.minimal_candidate_selection.is_some()
            || self.request_candidates.is_some()
            || self.provider_catalog.is_some()
            || self.provider_quotas.is_some()
            || self.routing_groups.is_some()
            || self.usage.is_some()
            || self.users.is_some()
            || self.video_tasks.is_some()
            || self.wallets.is_some()
    }
}

#[cfg(all(test, feature = "postgres"))]
mod tests {
    use super::DataReadRepositories;
    use crate::backend::PostgresBackend;
    use crate::driver::postgres::PostgresPoolConfig;

    #[tokio::test]
    async fn builds_read_repositories_from_postgres_backend() {
        let backend = PostgresBackend::from_config(PostgresPoolConfig {
            database_url: "postgres://localhost/aether".to_string(),
            min_connections: 1,
            max_connections: 4,
            acquire_timeout_ms: 1_000,
            idle_timeout_ms: 5_000,
            max_lifetime_ms: 30_000,
            statement_cache_capacity: 64,
            require_ssl: false,
        })
        .expect("postgres backend should build");

        let read = DataReadRepositories::from_postgres(Some(&backend));

        assert!(read.has_any());
        assert!(read.announcements().is_some());
        assert!(read.audit_logs().is_some());
        assert!(read.auth_api_keys().is_some());
        assert!(read.auth_modules().is_some());
        assert!(read.billing().is_some());
        assert!(read.gemini_file_mappings().is_some());
        assert!(read.global_models().is_some());
        assert!(read.management_tokens().is_some());
        assert!(read.oauth_providers().is_some());
        assert!(read.proxy_nodes().is_some());
        assert!(read.minimal_candidate_selection().is_some());
        assert!(read.request_candidates().is_some());
        assert!(read.provider_catalog().is_some());
        assert!(read.provider_quotas().is_some());
        assert!(read.usage().is_some());
        assert!(read.video_tasks().is_some());
        assert!(read.wallets().is_some());
    }
}
