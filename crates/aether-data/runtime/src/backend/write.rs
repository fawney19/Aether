use std::fmt;
use std::sync::Arc;

#[cfg(feature = "mysql")]
use super::MysqlBackend;
#[cfg(feature = "postgres")]
use super::PostgresBackend;
#[cfg(feature = "sqlite")]
use super::SqliteBackend;
use crate::repository::announcements::AnnouncementWriteRepository;
use crate::repository::auth::AuthApiKeyWriteRepository;
use crate::repository::auth_modules::AuthModuleWriteRepository;
use crate::repository::background_tasks::BackgroundTaskWriteRepository;
use crate::repository::candidates::RequestCandidateWriteRepository;
use crate::repository::gemini_file_mappings::GeminiFileMappingWriteRepository;
use crate::repository::global_models::GlobalModelWriteRepository;
use crate::repository::management_tokens::ManagementTokenWriteRepository;
use crate::repository::oauth_providers::OAuthProviderWriteRepository;
use crate::repository::pool_scores::PoolMemberScoreWriteRepository;
use crate::repository::provider_catalog::ProviderCatalogWriteRepository;
use crate::repository::proxy_nodes::ProxyNodeWriteRepository;
use crate::repository::quota::ProviderQuotaWriteRepository;
use crate::repository::routing_profiles::RoutingGroupWriteRepository;
use crate::repository::settlement::SettlementWriteRepository;
use crate::repository::usage::UsageWriteRepository;
use crate::repository::video_tasks::VideoTaskWriteRepository;
use crate::repository::wallet::WalletWriteRepository;

#[derive(Clone, Default)]
pub struct DataWriteRepositories {
    announcements: Option<Arc<dyn AnnouncementWriteRepository>>,
    auth_api_keys: Option<Arc<dyn AuthApiKeyWriteRepository>>,
    auth_modules: Option<Arc<dyn AuthModuleWriteRepository>>,
    background_tasks: Option<Arc<dyn BackgroundTaskWriteRepository>>,
    request_candidates: Option<Arc<dyn RequestCandidateWriteRepository>>,
    gemini_file_mappings: Option<Arc<dyn GeminiFileMappingWriteRepository>>,
    global_models: Option<Arc<dyn GlobalModelWriteRepository>>,
    management_tokens: Option<Arc<dyn ManagementTokenWriteRepository>>,
    oauth_providers: Option<Arc<dyn OAuthProviderWriteRepository>>,
    pool_scores: Option<Arc<dyn PoolMemberScoreWriteRepository>>,
    proxy_nodes: Option<Arc<dyn ProxyNodeWriteRepository>>,
    provider_catalog: Option<Arc<dyn ProviderCatalogWriteRepository>>,
    provider_quotas: Option<Arc<dyn ProviderQuotaWriteRepository>>,
    routing_groups: Option<Arc<dyn RoutingGroupWriteRepository>>,
    settlement: Option<Arc<dyn SettlementWriteRepository>>,
    usage: Option<Arc<dyn UsageWriteRepository>>,
    video_tasks: Option<Arc<dyn VideoTaskWriteRepository>>,
    wallets: Option<Arc<dyn WalletWriteRepository>>,
}

impl fmt::Debug for DataWriteRepositories {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DataWriteRepositories")
            .field("has_announcements", &self.announcements.is_some())
            .field("has_auth_api_keys", &self.auth_api_keys.is_some())
            .field("has_auth_modules", &self.auth_modules.is_some())
            .field("has_background_tasks", &self.background_tasks.is_some())
            .field("has_request_candidates", &self.request_candidates.is_some())
            .field(
                "has_gemini_file_mappings",
                &self.gemini_file_mappings.is_some(),
            )
            .field("has_global_models", &self.global_models.is_some())
            .field("has_management_tokens", &self.management_tokens.is_some())
            .field("has_oauth_providers", &self.oauth_providers.is_some())
            .field("has_pool_scores", &self.pool_scores.is_some())
            .field("has_proxy_nodes", &self.proxy_nodes.is_some())
            .field("has_provider_catalog", &self.provider_catalog.is_some())
            .field("has_provider_quotas", &self.provider_quotas.is_some())
            .field("has_routing_groups", &self.routing_groups.is_some())
            .field("has_settlement", &self.settlement.is_some())
            .field("has_usage", &self.usage.is_some())
            .field("has_video_tasks", &self.video_tasks.is_some())
            .field("has_wallets", &self.wallets.is_some())
            .finish()
    }
}

impl DataWriteRepositories {
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
            self.announcements = Some(PostgresBackend::announcement_write_repository(backend));
        }
        if self.auth_api_keys.is_none() {
            self.auth_api_keys = Some(PostgresBackend::auth_api_key_write_repository(backend));
        }
        if self.auth_modules.is_none() {
            self.auth_modules = Some(PostgresBackend::auth_module_write_repository(backend));
        }
        if self.background_tasks.is_none() {
            self.background_tasks =
                Some(PostgresBackend::background_task_write_repository(backend));
        }
        if self.request_candidates.is_none() {
            self.request_candidates =
                Some(PostgresBackend::request_candidate_write_repository(backend));
        }
        if self.gemini_file_mappings.is_none() {
            self.gemini_file_mappings = Some(
                PostgresBackend::gemini_file_mapping_write_repository(backend),
            );
        }
        if self.global_models.is_none() {
            self.global_models = Some(PostgresBackend::global_model_write_repository(backend));
        }
        if self.management_tokens.is_none() {
            self.management_tokens =
                Some(PostgresBackend::management_token_write_repository(backend));
        }
        if self.oauth_providers.is_none() {
            self.oauth_providers = Some(PostgresBackend::oauth_provider_write_repository(backend));
        }
        if self.pool_scores.is_none() {
            self.pool_scores = Some(PostgresBackend::pool_score_write_repository(backend));
        }
        if self.proxy_nodes.is_none() {
            self.proxy_nodes = Some(PostgresBackend::proxy_node_write_repository(backend));
        }
        if self.provider_catalog.is_none() {
            self.provider_catalog =
                Some(PostgresBackend::provider_catalog_write_repository(backend));
        }
        if self.provider_quotas.is_none() {
            self.provider_quotas = Some(PostgresBackend::provider_quota_write_repository(backend));
        }
        if self.routing_groups.is_none() {
            self.routing_groups = Some(PostgresBackend::routing_group_write_repository(backend));
        }
        if self.settlement.is_none() {
            self.settlement = Some(PostgresBackend::settlement_write_repository(backend));
        }
        if self.usage.is_none() {
            self.usage = Some(PostgresBackend::usage_write_repository(backend));
        }
        if self.video_tasks.is_none() {
            self.video_tasks = Some(PostgresBackend::video_task_write_repository(backend));
        }
        if self.wallets.is_none() {
            self.wallets = Some(PostgresBackend::wallet_write_repository(backend));
        }
    }

    #[cfg(feature = "mysql")]
    fn install_mysql(&mut self, backend: &MysqlBackend) {
        if self.announcements.is_none() {
            self.announcements = Some(MysqlBackend::announcement_write_repository(backend));
        }
        if self.auth_api_keys.is_none() {
            self.auth_api_keys = Some(MysqlBackend::auth_api_key_write_repository(backend));
        }
        if self.auth_modules.is_none() {
            self.auth_modules = Some(MysqlBackend::auth_module_write_repository(backend));
        }
        if self.background_tasks.is_none() {
            self.background_tasks = Some(MysqlBackend::background_task_write_repository(backend));
        }
        if self.request_candidates.is_none() {
            self.request_candidates =
                Some(MysqlBackend::request_candidate_write_repository(backend));
        }
        if self.gemini_file_mappings.is_none() {
            self.gemini_file_mappings =
                Some(MysqlBackend::gemini_file_mapping_write_repository(backend));
        }
        if self.global_models.is_none() {
            self.global_models = Some(MysqlBackend::global_model_write_repository(backend));
        }
        if self.management_tokens.is_none() {
            self.management_tokens = Some(MysqlBackend::management_token_write_repository(backend));
        }
        if self.oauth_providers.is_none() {
            self.oauth_providers = Some(MysqlBackend::oauth_provider_write_repository(backend));
        }
        if self.pool_scores.is_none() {
            self.pool_scores = Some(MysqlBackend::pool_score_write_repository(backend));
        }
        if self.proxy_nodes.is_none() {
            self.proxy_nodes = Some(MysqlBackend::proxy_node_write_repository(backend));
        }
        if self.provider_catalog.is_none() {
            self.provider_catalog = Some(MysqlBackend::provider_catalog_write_repository(backend));
        }
        if self.provider_quotas.is_none() {
            self.provider_quotas = Some(MysqlBackend::provider_quota_write_repository(backend));
        }
        if self.routing_groups.is_none() {
            self.routing_groups = Some(MysqlBackend::routing_group_write_repository(backend));
        }
        if self.settlement.is_none() {
            self.settlement = Some(MysqlBackend::settlement_write_repository(backend));
        }
        if self.usage.is_none() {
            self.usage = Some(MysqlBackend::usage_write_repository(backend));
        }
        if self.video_tasks.is_none() {
            self.video_tasks = Some(MysqlBackend::video_task_write_repository(backend));
        }
        if self.wallets.is_none() {
            self.wallets = Some(MysqlBackend::wallet_write_repository(backend));
        }
    }

    #[cfg(feature = "sqlite")]
    fn install_sqlite(&mut self, backend: &SqliteBackend) {
        if self.announcements.is_none() {
            self.announcements = Some(SqliteBackend::announcement_write_repository(backend));
        }
        if self.auth_api_keys.is_none() {
            self.auth_api_keys = Some(SqliteBackend::auth_api_key_write_repository(backend));
        }
        if self.auth_modules.is_none() {
            self.auth_modules = Some(SqliteBackend::auth_module_write_repository(backend));
        }
        if self.background_tasks.is_none() {
            self.background_tasks = Some(SqliteBackend::background_task_write_repository(backend));
        }
        if self.request_candidates.is_none() {
            self.request_candidates =
                Some(SqliteBackend::request_candidate_write_repository(backend));
        }
        if self.gemini_file_mappings.is_none() {
            self.gemini_file_mappings =
                Some(SqliteBackend::gemini_file_mapping_write_repository(backend));
        }
        if self.global_models.is_none() {
            self.global_models = Some(SqliteBackend::global_model_write_repository(backend));
        }
        if self.management_tokens.is_none() {
            self.management_tokens =
                Some(SqliteBackend::management_token_write_repository(backend));
        }
        if self.oauth_providers.is_none() {
            self.oauth_providers = Some(SqliteBackend::oauth_provider_write_repository(backend));
        }
        if self.pool_scores.is_none() {
            self.pool_scores = Some(SqliteBackend::pool_score_write_repository(backend));
        }
        if self.proxy_nodes.is_none() {
            self.proxy_nodes = Some(SqliteBackend::proxy_node_write_repository(backend));
        }
        if self.provider_catalog.is_none() {
            self.provider_catalog = Some(SqliteBackend::provider_catalog_write_repository(backend));
        }
        if self.provider_quotas.is_none() {
            self.provider_quotas = Some(SqliteBackend::provider_quota_write_repository(backend));
        }
        if self.routing_groups.is_none() {
            self.routing_groups = Some(SqliteBackend::routing_group_write_repository(backend));
        }
        if self.settlement.is_none() {
            self.settlement = Some(SqliteBackend::settlement_write_repository(backend));
        }
        if self.usage.is_none() {
            self.usage = Some(SqliteBackend::usage_write_repository(backend));
        }
        if self.video_tasks.is_none() {
            self.video_tasks = Some(SqliteBackend::video_task_write_repository(backend));
        }
        if self.wallets.is_none() {
            self.wallets = Some(SqliteBackend::wallet_write_repository(backend));
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

    pub fn announcements(&self) -> Option<Arc<dyn AnnouncementWriteRepository>> {
        self.announcements.clone()
    }

    pub fn auth_api_keys(&self) -> Option<Arc<dyn AuthApiKeyWriteRepository>> {
        self.auth_api_keys.clone()
    }

    pub fn auth_modules(&self) -> Option<Arc<dyn AuthModuleWriteRepository>> {
        self.auth_modules.clone()
    }

    pub fn background_tasks(&self) -> Option<Arc<dyn BackgroundTaskWriteRepository>> {
        self.background_tasks.clone()
    }

    pub fn usage(&self) -> Option<Arc<dyn UsageWriteRepository>> {
        self.usage.clone()
    }

    pub fn request_candidates(&self) -> Option<Arc<dyn RequestCandidateWriteRepository>> {
        self.request_candidates.clone()
    }

    pub fn gemini_file_mappings(&self) -> Option<Arc<dyn GeminiFileMappingWriteRepository>> {
        self.gemini_file_mappings.clone()
    }

    pub fn global_models(&self) -> Option<Arc<dyn GlobalModelWriteRepository>> {
        self.global_models.clone()
    }

    pub fn management_tokens(&self) -> Option<Arc<dyn ManagementTokenWriteRepository>> {
        self.management_tokens.clone()
    }

    pub fn oauth_providers(&self) -> Option<Arc<dyn OAuthProviderWriteRepository>> {
        self.oauth_providers.clone()
    }

    pub fn pool_scores(&self) -> Option<Arc<dyn PoolMemberScoreWriteRepository>> {
        self.pool_scores.clone()
    }

    pub fn proxy_nodes(&self) -> Option<Arc<dyn ProxyNodeWriteRepository>> {
        self.proxy_nodes.clone()
    }

    pub fn provider_quotas(&self) -> Option<Arc<dyn ProviderQuotaWriteRepository>> {
        self.provider_quotas.clone()
    }

    pub fn routing_groups(&self) -> Option<Arc<dyn RoutingGroupWriteRepository>> {
        self.routing_groups.clone()
    }

    pub fn provider_catalog(&self) -> Option<Arc<dyn ProviderCatalogWriteRepository>> {
        self.provider_catalog.clone()
    }

    pub fn settlement(&self) -> Option<Arc<dyn SettlementWriteRepository>> {
        self.settlement.clone()
    }

    pub fn video_tasks(&self) -> Option<Arc<dyn VideoTaskWriteRepository>> {
        self.video_tasks.clone()
    }

    pub fn wallets(&self) -> Option<Arc<dyn WalletWriteRepository>> {
        self.wallets.clone()
    }

    pub fn has_any(&self) -> bool {
        self.announcements.is_some()
            || self.auth_api_keys.is_some()
            || self.auth_modules.is_some()
            || self.background_tasks.is_some()
            || self.request_candidates.is_some()
            || self.gemini_file_mappings.is_some()
            || self.global_models.is_some()
            || self.management_tokens.is_some()
            || self.oauth_providers.is_some()
            || self.pool_scores.is_some()
            || self.proxy_nodes.is_some()
            || self.provider_catalog.is_some()
            || self.provider_quotas.is_some()
            || self.routing_groups.is_some()
            || self.settlement.is_some()
            || self.usage.is_some()
            || self.video_tasks.is_some()
            || self.wallets.is_some()
    }
}

#[cfg(all(test, feature = "postgres"))]
mod tests {
    use super::DataWriteRepositories;
    use crate::backend::PostgresBackend;
    use crate::driver::postgres::PostgresPoolConfig;

    #[tokio::test]
    async fn builds_write_repositories_from_postgres_backend() {
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

        let write = DataWriteRepositories::from_postgres(Some(&backend));

        assert!(write.has_any());
        assert!(write.announcements().is_some());
        assert!(write.auth_api_keys().is_some());
        assert!(write.auth_modules().is_some());
        assert!(write.request_candidates().is_some());
        assert!(write.gemini_file_mappings().is_some());
        assert!(write.global_models().is_some());
        assert!(write.management_tokens().is_some());
        assert!(write.oauth_providers().is_some());
        assert!(write.proxy_nodes().is_some());
        assert!(write.provider_catalog().is_some());
        assert!(write.provider_quotas().is_some());
        assert!(write.settlement().is_some());
        assert!(write.usage().is_some());
        assert!(write.video_tasks().is_some());
        assert!(write.wallets().is_some());
    }
}
