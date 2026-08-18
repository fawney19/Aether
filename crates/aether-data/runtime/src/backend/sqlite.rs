use std::sync::Arc;

use crate::database::SqlDatabaseConfig;
use crate::driver::sqlite::{SqlitePool, SqlitePoolFactory};
use crate::repository::announcements::{
    AnnouncementReadRepository, AnnouncementWriteRepository, SqliteAnnouncementRepository,
};
use crate::repository::audit::{AuditLogReadRepository, SqliteAuditLogReadRepository};
use crate::repository::auth::{
    AuthApiKeyReadRepository, AuthApiKeyWriteRepository, SqliteAuthApiKeyReadRepository,
};
use crate::repository::auth_modules::{
    AuthModuleReadRepository, AuthModuleWriteRepository, SqliteAuthModuleReadRepository,
    SqliteAuthModuleRepository,
};
use crate::repository::background_tasks::{
    BackgroundTaskReadRepository, BackgroundTaskWriteRepository, SqliteBackgroundTaskRepository,
};
use crate::repository::billing::{BillingReadRepository, SqliteBillingReadRepository};
use crate::repository::candidate_selection::{
    MinimalCandidateSelectionReadRepository, SqliteMinimalCandidateSelectionReadRepository,
};
use crate::repository::candidates::{
    RequestCandidateReadRepository, RequestCandidateWriteRepository,
    SqliteRequestCandidateRepository,
};
use crate::repository::gemini_file_mappings::{
    GeminiFileMappingReadRepository, GeminiFileMappingWriteRepository,
    SqliteGeminiFileMappingRepository,
};
use crate::repository::global_models::{
    GlobalModelReadRepository, GlobalModelWriteRepository, SqliteGlobalModelReadRepository,
};
use crate::repository::management_tokens::{
    ManagementTokenReadRepository, ManagementTokenWriteRepository, SqliteManagementTokenRepository,
};
use crate::repository::oauth_providers::{
    OAuthProviderReadRepository, OAuthProviderWriteRepository, SqliteOAuthProviderRepository,
};
use crate::repository::pool_scores::{
    PoolMemberScoreWriteRepository, PoolScoreReadRepository, SqlitePoolMemberScoreRepository,
};
use crate::repository::provider_catalog::{
    ProviderCatalogReadRepository, ProviderCatalogWriteRepository,
    SqliteProviderCatalogReadRepository,
};
use crate::repository::proxy_nodes::{
    ProxyNodeReadRepository, ProxyNodeWriteRepository, SqliteProxyNodeReadRepository,
};
use crate::repository::quota::{
    ProviderQuotaReadRepository, ProviderQuotaWriteRepository, SqliteProviderQuotaRepository,
};
use crate::repository::routing_profiles::{
    RoutingGroupReadRepository, RoutingGroupWriteRepository, SqliteRoutingGroupRepository,
};
use crate::repository::settlement::{SettlementWriteRepository, SqliteSettlementRepository};
use crate::repository::usage::{
    SqliteUsageReadRepository, SqliteUsageWriteRepository, UsageReadRepository,
    UsageWriteRepository,
};
use crate::repository::users::{SqliteUserReadRepository, UserReadRepository};
use crate::repository::video_tasks::{
    SqliteVideoTaskRepository, VideoTaskReadRepository, VideoTaskWriteRepository,
};
use crate::repository::wallet::{
    SqliteWalletReadRepository, WalletReadRepository, WalletWriteRepository,
};
use crate::DataLayerError;

#[derive(Debug, Clone)]
pub struct SqliteBackend {
    config: SqlDatabaseConfig,
    pool: SqlitePool,
}

impl SqliteBackend {
    pub fn from_config(config: SqlDatabaseConfig) -> Result<Self, DataLayerError> {
        let factory = SqlitePoolFactory::new(config.clone())?;
        let pool = factory.connect_lazy()?;

        Ok(Self { config, pool })
    }

    pub fn config(&self) -> &SqlDatabaseConfig {
        &self.config
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    pub fn pool_clone(&self) -> SqlitePool {
        self.pool.clone()
    }

    pub fn auth_api_key_read_repository(&self) -> Arc<dyn AuthApiKeyReadRepository> {
        Arc::new(SqliteAuthApiKeyReadRepository::new(self.pool_clone()))
    }

    pub fn announcement_read_repository(&self) -> Arc<dyn AnnouncementReadRepository> {
        Arc::new(SqliteAnnouncementRepository::new(self.pool_clone()))
    }

    pub fn audit_log_read_repository(&self) -> Arc<dyn AuditLogReadRepository> {
        Arc::new(SqliteAuditLogReadRepository::new(self.pool_clone()))
    }

    pub fn announcement_write_repository(&self) -> Arc<dyn AnnouncementWriteRepository> {
        Arc::new(SqliteAnnouncementRepository::new(self.pool_clone()))
    }

    pub fn auth_api_key_write_repository(&self) -> Arc<dyn AuthApiKeyWriteRepository> {
        Arc::new(SqliteAuthApiKeyReadRepository::new(self.pool_clone()))
    }

    pub fn management_token_read_repository(&self) -> Arc<dyn ManagementTokenReadRepository> {
        Arc::new(SqliteManagementTokenRepository::new(self.pool_clone()))
    }

    pub fn management_token_write_repository(&self) -> Arc<dyn ManagementTokenWriteRepository> {
        Arc::new(SqliteManagementTokenRepository::new(self.pool_clone()))
    }

    pub fn auth_module_read_repository(&self) -> Arc<dyn AuthModuleReadRepository> {
        Arc::new(SqliteAuthModuleReadRepository::new(self.pool_clone()))
    }

    pub fn auth_module_write_repository(&self) -> Arc<dyn AuthModuleWriteRepository> {
        Arc::new(SqliteAuthModuleRepository::new(self.pool_clone()))
    }

    pub fn billing_read_repository(&self) -> Arc<dyn BillingReadRepository> {
        Arc::new(SqliteBillingReadRepository::new(self.pool_clone()))
    }

    pub fn background_task_read_repository(&self) -> Arc<dyn BackgroundTaskReadRepository> {
        Arc::new(SqliteBackgroundTaskRepository::new(self.pool_clone()))
    }

    pub fn background_task_write_repository(&self) -> Arc<dyn BackgroundTaskWriteRepository> {
        Arc::new(SqliteBackgroundTaskRepository::new(self.pool_clone()))
    }

    pub fn request_candidate_read_repository(&self) -> Arc<dyn RequestCandidateReadRepository> {
        Arc::new(SqliteRequestCandidateRepository::new(self.pool_clone()))
    }

    pub fn request_candidate_write_repository(&self) -> Arc<dyn RequestCandidateWriteRepository> {
        Arc::new(SqliteRequestCandidateRepository::new(self.pool_clone()))
    }

    pub fn minimal_candidate_selection_read_repository(
        &self,
    ) -> Arc<dyn MinimalCandidateSelectionReadRepository> {
        Arc::new(SqliteMinimalCandidateSelectionReadRepository::new(
            self.pool_clone(),
        ))
    }

    pub fn gemini_file_mapping_read_repository(&self) -> Arc<dyn GeminiFileMappingReadRepository> {
        Arc::new(SqliteGeminiFileMappingRepository::new(self.pool_clone()))
    }

    pub fn gemini_file_mapping_write_repository(
        &self,
    ) -> Arc<dyn GeminiFileMappingWriteRepository> {
        Arc::new(SqliteGeminiFileMappingRepository::new(self.pool_clone()))
    }

    pub fn global_model_read_repository(&self) -> Arc<dyn GlobalModelReadRepository> {
        Arc::new(SqliteGlobalModelReadRepository::new(self.pool_clone()))
    }

    pub fn global_model_write_repository(&self) -> Arc<dyn GlobalModelWriteRepository> {
        Arc::new(SqliteGlobalModelReadRepository::new(self.pool_clone()))
    }

    pub fn user_read_repository(&self) -> Arc<dyn UserReadRepository> {
        Arc::new(SqliteUserReadRepository::new(self.pool_clone()))
    }

    pub fn video_task_read_repository(&self) -> Arc<dyn VideoTaskReadRepository> {
        Arc::new(SqliteVideoTaskRepository::new(self.pool_clone()))
    }

    pub fn video_task_write_repository(&self) -> Arc<dyn VideoTaskWriteRepository> {
        Arc::new(SqliteVideoTaskRepository::new(self.pool_clone()))
    }

    pub fn oauth_provider_read_repository(&self) -> Arc<dyn OAuthProviderReadRepository> {
        Arc::new(SqliteOAuthProviderRepository::new(self.pool_clone()))
    }

    pub fn oauth_provider_write_repository(&self) -> Arc<dyn OAuthProviderWriteRepository> {
        Arc::new(SqliteOAuthProviderRepository::new(self.pool_clone()))
    }

    pub fn provider_catalog_read_repository(&self) -> Arc<dyn ProviderCatalogReadRepository> {
        Arc::new(SqliteProviderCatalogReadRepository::new(self.pool_clone()))
    }

    pub fn provider_catalog_write_repository(&self) -> Arc<dyn ProviderCatalogWriteRepository> {
        Arc::new(SqliteProviderCatalogReadRepository::new(self.pool_clone()))
    }

    pub fn pool_score_read_repository(&self) -> Arc<dyn PoolScoreReadRepository> {
        Arc::new(SqlitePoolMemberScoreRepository::new(self.pool_clone()))
    }

    pub fn pool_score_write_repository(&self) -> Arc<dyn PoolMemberScoreWriteRepository> {
        Arc::new(SqlitePoolMemberScoreRepository::new(self.pool_clone()))
    }

    pub fn routing_group_read_repository(&self) -> Arc<dyn RoutingGroupReadRepository> {
        Arc::new(SqliteRoutingGroupRepository::new(self.pool_clone()))
    }

    pub fn routing_group_write_repository(&self) -> Arc<dyn RoutingGroupWriteRepository> {
        Arc::new(SqliteRoutingGroupRepository::new(self.pool_clone()))
    }

    pub fn proxy_node_read_repository(&self) -> Arc<dyn ProxyNodeReadRepository> {
        Arc::new(SqliteProxyNodeReadRepository::new(self.pool_clone()))
    }

    pub fn proxy_node_write_repository(&self) -> Arc<dyn ProxyNodeWriteRepository> {
        Arc::new(SqliteProxyNodeReadRepository::new(self.pool_clone()))
    }

    pub fn provider_quota_read_repository(&self) -> Arc<dyn ProviderQuotaReadRepository> {
        Arc::new(SqliteProviderQuotaRepository::new(self.pool_clone()))
    }

    pub fn provider_quota_write_repository(&self) -> Arc<dyn ProviderQuotaWriteRepository> {
        Arc::new(SqliteProviderQuotaRepository::new(self.pool_clone()))
    }

    pub fn settlement_write_repository(&self) -> Arc<dyn SettlementWriteRepository> {
        Arc::new(SqliteSettlementRepository::new(self.pool_clone()))
    }

    pub fn usage_write_repository(&self) -> Arc<dyn UsageWriteRepository> {
        Arc::new(SqliteUsageWriteRepository::new(self.pool_clone()))
    }

    pub fn usage_read_repository(&self) -> Arc<dyn UsageReadRepository> {
        Arc::new(SqliteUsageReadRepository::new(self.pool_clone()))
    }

    pub fn wallet_read_repository(&self) -> Arc<dyn WalletReadRepository> {
        Arc::new(SqliteWalletReadRepository::new(self.pool_clone()))
    }

    pub fn wallet_write_repository(&self) -> Arc<dyn WalletWriteRepository> {
        Arc::new(SqliteWalletReadRepository::new(self.pool_clone()))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::SqliteBackend;
    use crate::lifecycle::migrate::run_sqlite_migrations;
    use crate::repository::system::{
        AdminSystemPurgeTarget, AdminSystemStatsDailyAggregate,
        AdminSystemStatsDailyApiKeyAggregate, AdminSystemStatsUserDailyAggregate,
        AdminSystemUsageAggregateImportMode, AdminSystemUsageAggregateSnapshot,
    };
    use crate::{
        DatabaseDriver, SqlDatabaseConfig, SqlPoolConfig, StatsDailyAggregationInput,
        StatsHourlyAggregationInput, WalletDailyUsageAggregationInput,
    };

    #[tokio::test]
    async fn backend_retains_config_and_pool() {
        let config = SqlDatabaseConfig {
            driver: DatabaseDriver::Sqlite,
            url: "sqlite://./data/aether.db".to_string(),
            pool: SqlPoolConfig::default(),
        };

        let backend = SqliteBackend::from_config(config.clone()).expect("backend should build");

        assert_eq!(backend.config(), &config);
        let _pool = backend.pool();
        let _pool_clone = backend.pool_clone();
    }

    #[tokio::test]
    async fn system_config_round_trips_after_sqlite_migrations() {
        let config = SqlDatabaseConfig {
            driver: DatabaseDriver::Sqlite,
            url: "sqlite::memory:".to_string(),
            pool: SqlPoolConfig {
                max_connections: 1,
                ..SqlPoolConfig::default()
            },
        };
        let backend = SqliteBackend::from_config(config).expect("backend should build");
        run_sqlite_migrations(backend.pool())
            .await
            .expect("sqlite migrations should run");

        let value = serde_json::json!({"enabled": true});
        let stored = backend
            .upsert_system_config_entry("feature.local", &value, Some("local flag"))
            .await
            .expect("system config should upsert");
        assert_eq!(stored.value, value);
        assert_eq!(
            backend
                .find_system_config_value("feature.local")
                .await
                .expect("system config should read"),
            Some(value)
        );
        assert_eq!(
            backend
                .list_system_config_entries()
                .await
                .expect("system config should list")
                .len(),
            2
        );
        assert!(backend
            .delete_system_config_value("feature.local")
            .await
            .expect("system config should delete"));
    }

    #[tokio::test]
    async fn table_maintenance_runs_after_sqlite_migrations() {
        let config = SqlDatabaseConfig {
            driver: DatabaseDriver::Sqlite,
            url: "sqlite::memory:".to_string(),
            pool: SqlPoolConfig {
                max_connections: 1,
                ..SqlPoolConfig::default()
            },
        };
        let backend = SqliteBackend::from_config(config).expect("backend should build");
        run_sqlite_migrations(backend.pool())
            .await
            .expect("sqlite migrations should run");

        let summary = backend
            .run_table_maintenance(&["usage", "request_candidates", "audit_logs"])
            .await
            .expect("sqlite table maintenance should run");

        assert_eq!(summary.attempted, 3);
        assert_eq!(summary.succeeded, 3);
    }

    #[tokio::test]
    async fn admin_system_config_purge_deletes_config_scope_and_preserves_users() {
        let config = SqlDatabaseConfig {
            driver: DatabaseDriver::Sqlite,
            url: "sqlite::memory:".to_string(),
            pool: SqlPoolConfig {
                max_connections: 1,
                ..SqlPoolConfig::default()
            },
        };
        let backend = SqliteBackend::from_config(config).expect("backend should build");
        run_sqlite_migrations(backend.pool())
            .await
            .expect("sqlite migrations should run");

        sqlx::query(
            "INSERT INTO users (id, email, username, role, created_at, updated_at) VALUES ('admin-1', 'admin@example.com', 'admin', 'admin', 1, 1)",
        )
        .execute(backend.pool())
        .await
        .expect("user should insert");
        sqlx::query(
            "INSERT INTO providers (id, name, provider_type, created_at, updated_at) VALUES ('provider-1', 'OpenAI', 'openai', 1, 1)",
        )
        .execute(backend.pool())
        .await
        .expect("provider should insert");
        sqlx::query(
            "INSERT INTO system_configs (id, key, value, created_at, updated_at) VALUES ('config-1', 'site_name', '\"Aether\"', 1, 1)",
        )
        .execute(backend.pool())
        .await
        .expect("system config should insert");

        let summary = backend
            .purge_admin_system_data(AdminSystemPurgeTarget::Config)
            .await
            .expect("config purge should run");
        assert!(summary.total() >= 2);
        assert_eq!(sqlite_count(backend.pool(), "system_configs").await, 0);
        assert_eq!(sqlite_count(backend.pool(), "providers").await, 0);
        assert_eq!(sqlite_count(backend.pool(), "users").await, 1);
    }

    #[tokio::test]
    async fn admin_system_users_purge_deletes_only_non_admin_users_and_keys() {
        let config = SqlDatabaseConfig {
            driver: DatabaseDriver::Sqlite,
            url: "sqlite::memory:".to_string(),
            pool: SqlPoolConfig {
                max_connections: 1,
                ..SqlPoolConfig::default()
            },
        };
        let backend = SqliteBackend::from_config(config).expect("backend should build");
        run_sqlite_migrations(backend.pool())
            .await
            .expect("sqlite migrations should run");

        sqlx::query(
            r#"
INSERT INTO users (id, email, username, role, created_at, updated_at)
VALUES
  ('admin-1', 'admin@example.com', 'admin', 'admin', 1, 1),
  ('user-1', 'user@example.com', 'alice', 'user', 1, 1)
"#,
        )
        .execute(backend.pool())
        .await
        .expect("users should insert");
        sqlx::query(
            r#"
INSERT INTO api_keys (id, user_id, key_hash, name, created_at, updated_at, total_requests, total_tokens, total_cost_usd)
VALUES
  ('admin-key-1', 'admin-1', 'hash-admin', 'admin-key', 1, 1, 5, 50, 0.5),
  ('user-key-1', 'user-1', 'hash-user', 'user-key', 1, 1, 7, 70, 0.7)
"#,
        )
        .execute(backend.pool())
        .await
        .expect("api keys should insert");
        sqlx::query(
            r#"
INSERT INTO stats_daily_api_key (id, api_key_id, "date", total_requests, created_at, updated_at)
VALUES
  ('admin-key-stats-1', 'admin-key-1', 1, 5, 1, 1),
  ('user-key-stats-1', 'user-key-1', 1, 7, 1, 1)
"#,
        )
        .execute(backend.pool())
        .await
        .expect("api key stats should insert");

        let summary = backend
            .purge_admin_system_data(AdminSystemPurgeTarget::Users)
            .await
            .expect("users purge should run");
        assert!(summary.total() >= 2);
        assert_eq!(sqlite_count(backend.pool(), "users").await, 1);
        assert_eq!(sqlite_count(backend.pool(), "api_keys").await, 1);
        assert_eq!(sqlite_count(backend.pool(), "stats_daily_api_key").await, 1);
        let admin_exists: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE id = 'admin-1'")
                .fetch_one(backend.pool())
                .await
                .expect("admin count should load");
        assert_eq!(admin_exists, 1);
    }

    #[tokio::test]
    async fn admin_system_usage_aggregates_round_trip_after_sqlite_migrations() {
        let config = SqlDatabaseConfig {
            driver: DatabaseDriver::Sqlite,
            url: "sqlite::memory:".to_string(),
            pool: SqlPoolConfig {
                max_connections: 1,
                ..SqlPoolConfig::default()
            },
        };
        let backend = SqliteBackend::from_config(config).expect("backend should build");
        run_sqlite_migrations(backend.pool())
            .await
            .expect("sqlite migrations should run");

        sqlx::query(
            r#"
INSERT INTO users (id, email, username, role, created_at, updated_at)
VALUES ('target-user-1', 'target@example.com', 'target', 'user', 1, 1)
"#,
        )
        .execute(backend.pool())
        .await
        .expect("target user should insert");
        sqlx::query(
            r#"
INSERT INTO api_keys (id, user_id, key_hash, name, created_at, updated_at)
VALUES ('target-key-1', 'target-user-1', 'hash-target-key', 'target key', 1, 1)
"#,
        )
        .execute(backend.pool())
        .await
        .expect("target key should insert");

        let snapshot = AdminSystemUsageAggregateSnapshot {
            stats_daily: vec![AdminSystemStatsDailyAggregate {
                date_unix_secs: 86_400,
                total_requests: 10,
                success_requests: 8,
                error_requests: 1,
                sla_eligible_requests: 9,
                user_error_requests: 1,
                input_tokens: 100,
                output_tokens: 200,
                cache_creation_tokens: 3,
                cache_read_tokens: 4,
                total_cost: 1.25,
                actual_total_cost: 1.0,
                is_complete: true,
                aggregated_at_unix_secs: Some(90_000),
            }],
            stats_user_daily: vec![AdminSystemStatsUserDailyAggregate {
                user_id: "source-user-1".to_string(),
                username: Some("source".to_string()),
                date_unix_secs: 86_400,
                total_requests: 5,
                success_requests: 5,
                error_requests: 0,
                sla_eligible_requests: 5,
                user_error_requests: 0,
                input_tokens: 50,
                output_tokens: 60,
                cache_creation_tokens: 1,
                cache_read_tokens: 2,
                total_cost: 0.5,
            }],
            stats_daily_api_key: vec![AdminSystemStatsDailyApiKeyAggregate {
                api_key_id: "source-key-1".to_string(),
                api_key_name: Some("source key".to_string()),
                date_unix_secs: 86_400,
                total_requests: 4,
                success_requests: 3,
                error_requests: 1,
                sla_eligible_requests: 4,
                user_error_requests: 0,
                input_tokens: 40,
                output_tokens: 30,
                cache_creation_tokens: 2,
                cache_read_tokens: 1,
                total_cost: 0.75,
            }],
        };
        let user_id_map =
            BTreeMap::from([("source-user-1".to_string(), "target-user-1".to_string())]);
        let api_key_id_map =
            BTreeMap::from([("source-key-1".to_string(), "target-key-1".to_string())]);

        let summary = backend
            .import_admin_system_usage_aggregates(
                &snapshot,
                &user_id_map,
                &api_key_id_map,
                AdminSystemUsageAggregateImportMode::Overwrite,
            )
            .await
            .expect("usage aggregates should import");
        assert_eq!(summary.stats_daily.created, 1);
        assert_eq!(summary.stats_user_daily.created, 1);
        assert_eq!(summary.stats_daily_api_key.created, 1);

        let exported = backend
            .export_admin_system_usage_aggregates()
            .await
            .expect("usage aggregates should export");
        assert_eq!(exported.stats_daily.len(), 1);
        assert_eq!(exported.stats_daily[0].total_requests, 10);
        assert_eq!(exported.stats_daily[0].sla_eligible_requests, 9);
        assert_eq!(exported.stats_daily[0].user_error_requests, 1);
        assert_eq!(exported.stats_daily[0].actual_total_cost, 1.0);
        assert_eq!(exported.stats_user_daily.len(), 1);
        assert_eq!(exported.stats_user_daily[0].user_id, "target-user-1");
        assert_eq!(exported.stats_user_daily[0].total_requests, 5);
        assert_eq!(exported.stats_daily_api_key.len(), 1);
        assert_eq!(exported.stats_daily_api_key[0].api_key_id, "target-key-1");
        assert_eq!(exported.stats_daily_api_key[0].total_requests, 4);
    }

    #[tokio::test]
    async fn admin_system_request_bodies_purge_clears_inline_usage_body_fields() {
        let config = SqlDatabaseConfig {
            driver: DatabaseDriver::Sqlite,
            url: "sqlite::memory:".to_string(),
            pool: SqlPoolConfig {
                max_connections: 1,
                ..SqlPoolConfig::default()
            },
        };
        let backend = SqliteBackend::from_config(config).expect("backend should build");
        run_sqlite_migrations(backend.pool())
            .await
            .expect("sqlite migrations should run");

        sqlx::query(
            r#"
INSERT INTO "usage" (
    request_id,
    provider_name,
    model,
    request_body,
    response_body,
    provider_request_body,
    client_response_body,
    request_body_compressed,
    response_body_compressed,
    provider_request_body_compressed,
    client_response_body_compressed,
    created_at_unix_ms
)
VALUES (
    'request-1',
    'openai',
    'gpt-4.1',
    'client request',
    'provider response',
    'provider request',
    'client response',
    X'01',
    X'02',
    X'03',
    X'04',
    1
)
"#,
        )
        .execute(backend.pool())
        .await
        .expect("usage row should insert");

        let summary = backend
            .purge_admin_system_data(AdminSystemPurgeTarget::RequestBodies)
            .await
            .expect("request body purge should run");

        assert_eq!(summary.affected.get("usage_body_fields_cleaned"), Some(&1));
        let remaining: i64 = sqlx::query_scalar(
            r#"
SELECT COUNT(*)
FROM "usage"
WHERE request_body IS NOT NULL
   OR response_body IS NOT NULL
   OR provider_request_body IS NOT NULL
   OR client_response_body IS NOT NULL
   OR request_body_compressed IS NOT NULL
   OR response_body_compressed IS NOT NULL
   OR provider_request_body_compressed IS NOT NULL
   OR client_response_body_compressed IS NOT NULL
"#,
        )
        .fetch_one(backend.pool())
        .await
        .expect("remaining body count should load");
        assert_eq!(remaining, 0);
    }

    async fn sqlite_count(pool: &sqlx::SqlitePool, table: &str) -> i64 {
        let sql = format!("SELECT COUNT(*) FROM \"{table}\"");
        sqlx::query_scalar::<_, i64>(&sql)
            .fetch_one(pool)
            .await
            .expect("count should load")
    }

    #[tokio::test]
    async fn wallet_daily_usage_aggregation_uses_settlement_wallets_after_sqlite_migrations() {
        let config = SqlDatabaseConfig {
            driver: DatabaseDriver::Sqlite,
            url: "sqlite::memory:".to_string(),
            pool: SqlPoolConfig {
                max_connections: 1,
                ..SqlPoolConfig::default()
            },
        };
        let backend = SqliteBackend::from_config(config).expect("backend should build");
        run_sqlite_migrations(backend.pool())
            .await
            .expect("sqlite migrations should run");

        sqlx::query(
            r#"
INSERT INTO wallets (id, user_id, balance, gift_balance, limit_mode, created_at, updated_at)
VALUES
  ('wallet-1', 'user-1', 10.0, 2.0, 'finite', 1, 1),
  ('wallet-stale', 'user-stale', 0.0, 0.0, 'finite', 1, 1)
"#,
        )
        .execute(backend.pool())
        .await
        .expect("wallets should seed");

        sqlx::query(
            r#"
INSERT INTO "usage" (
  request_id, wallet_id, provider_name, model, status, billing_status,
  total_cost_usd, input_tokens, output_tokens, cache_creation_input_tokens,
  cache_read_input_tokens, finalized_at, created_at_unix_ms, updated_at_unix_secs
) VALUES
  ('request-1', 'wrong-wallet', 'provider', 'model', 'completed', 'pending',
   1.25, 10, 20, 3, 4, 900, 900000, 900),
  ('request-2', NULL, 'provider', 'model', 'completed', 'pending',
   2.00, 5, 7, 1, 2, 901, 901000, 901),
  ('request-zero', NULL, 'provider', 'model', 'completed', 'pending',
   0.00, 100, 100, 0, 0, 902, 902000, 902),
  ('request-outside', NULL, 'provider', 'model', 'completed', 'pending',
   9.00, 50, 50, 0, 0, 903, 903000, 903)
"#,
        )
        .execute(backend.pool())
        .await
        .expect("usage should seed");

        sqlx::query(
            r#"
INSERT INTO usage_settlement_snapshots (
  request_id, billing_status, wallet_id, finalized_at, created_at, updated_at
) VALUES
  ('request-1', 'settled', 'wallet-1', 1000, 1000, 1000),
  ('request-2', 'settled', 'wallet-1', 1100, 1100, 1100),
  ('request-zero', 'settled', 'wallet-1', 1150, 1150, 1150),
  ('request-outside', 'settled', 'wallet-1', 1200, 1200, 1200)
"#,
        )
        .execute(backend.pool())
        .await
        .expect("settlement snapshots should seed");

        sqlx::query(
            r#"
INSERT INTO wallet_daily_usage_ledgers (
  id, wallet_id, billing_date, billing_timezone, total_cost_usd,
  total_requests, input_tokens, output_tokens, cache_creation_tokens,
  cache_read_tokens, aggregated_at, created_at, updated_at
) VALUES (
  'stale-ledger', 'wallet-stale', '2026-05-03', 'Asia/Shanghai',
  7.0, 3, 1, 1, 0, 0, 999, 999, 999
)
"#,
        )
        .execute(backend.pool())
        .await
        .expect("stale ledger should seed");

        let summary = backend
            .aggregate_wallet_daily_usage(&WalletDailyUsageAggregationInput {
                billing_date: "2026-05-03".to_string(),
                billing_timezone: "Asia/Shanghai".to_string(),
                window_start_unix_secs: 1000,
                window_end_unix_secs: 1200,
                aggregated_at_unix_secs: 1300,
            })
            .await
            .expect("wallet daily usage aggregation should run");

        assert_eq!(summary.aggregated_wallets, 1);
        assert_eq!(summary.deleted_stale_ledgers, 1);

        let ledger = sqlx::query_as::<
            _,
            (
                String,
                String,
                f64,
                i64,
                i64,
                i64,
                i64,
                i64,
                Option<i64>,
                Option<i64>,
                i64,
            ),
        >(
            r#"
SELECT
  id,
  wallet_id,
  total_cost_usd,
  total_requests,
  input_tokens,
  output_tokens,
  cache_creation_tokens,
  cache_read_tokens,
  first_finalized_at,
  last_finalized_at,
  aggregated_at
FROM wallet_daily_usage_ledgers
WHERE billing_date = '2026-05-03'
  AND billing_timezone = 'Asia/Shanghai'
"#,
        )
        .fetch_one(backend.pool())
        .await
        .expect("aggregated ledger should load");

        assert_eq!(ledger.0.len(), 64);
        assert_eq!(ledger.1, "wallet-1");
        assert!((ledger.2 - 3.25).abs() < f64::EPSILON);
        assert_eq!(ledger.3, 2);
        assert_eq!(ledger.4, 15);
        assert_eq!(ledger.5, 27);
        assert_eq!(ledger.6, 4);
        assert_eq!(ledger.7, 6);
        assert_eq!(ledger.8, Some(1000));
        assert_eq!(ledger.9, Some(1100));
        assert_eq!(ledger.10, 1300);

        let stale_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM wallet_daily_usage_ledgers WHERE id = 'stale-ledger'",
        )
        .fetch_one(backend.pool())
        .await
        .expect("stale ledger count should load");
        assert_eq!(stale_count, 0);
    }

    #[tokio::test]
    async fn stats_aggregation_runs_after_sqlite_migrations() {
        let config = SqlDatabaseConfig {
            driver: DatabaseDriver::Sqlite,
            url: "sqlite::memory:".to_string(),
            pool: SqlPoolConfig {
                max_connections: 1,
                ..SqlPoolConfig::default()
            },
        };
        let backend = SqliteBackend::from_config(config).expect("backend should build");
        run_sqlite_migrations(backend.pool())
            .await
            .expect("sqlite migrations should run");

        sqlx::query(
            r#"
INSERT INTO "usage" (
  request_id, user_id, api_key_id, provider_name, model, api_format, status, billing_status,
  outcome_class, sla_eligible,
  status_code, error_category, input_tokens, output_tokens,
  cache_creation_input_tokens, cache_read_input_tokens, total_cost_usd,
  actual_total_cost_usd, cache_creation_cost_usd, cache_read_cost_usd,
  input_price_per_1m, response_time_ms, first_byte_time_ms,
  created_at_unix_ms, updated_at_unix_secs
) VALUES
  ('stats-1', 'user-1', 'key-1', 'provider-a', 'model-a', 'openai', 'completed', 'settled',
   'success', 1, 200, NULL, 10, 20, 1, 2, 0.30, 0.25, 0.01, 0.02, 10.0, 100, 50, 3600, 3600),
  ('stats-2', 'user-2', 'key-2', 'provider-b', 'model-b', 'claude', 'failed', 'void',
   'service_error', 1, 500, 'upstream_error', 5, 7, 0, 1, 0.20, 0.20, 0.00, 0.01, 20.0, 300, 200, 3610, 3610),
  ('stats-pending', 'user-3', 'key-3', 'provider-a', 'model-a', 'openai', 'pending', 'pending',
   'in_flight', 0, NULL, NULL, 100, 100, 0, 0, 9.99, 9.99, 0.00, 0.00, 0.0, 50, 25, 3620, 3620),
  ('stats-unknown-provider', 'user-4', 'key-4', 'unknown', 'model-a', 'openai', 'completed', 'settled',
   'success', 1, 200, NULL, 100, 100, 0, 0, 9.99, 9.99, 0.00, 0.00, 0.0, 50, 25, 3630, 3630)
"#,
        )
        .execute(backend.pool())
        .await
        .expect("usage stats rows should seed");

        sqlx::query(
            r#"
INSERT INTO request_candidates (
  id, request_id, candidate_index, retry_index, status, created_at
) VALUES
  ('stats-candidate-1', 'stats-fallback', 0, 0, 'failed', 3600000),
  ('stats-candidate-2', 'stats-fallback', 1, 0, 'success', 3610000)
"#,
        )
        .execute(backend.pool())
        .await
        .expect("fallback candidates should seed");

        let target_hour = chrono::DateTime::<chrono::Utc>::from_timestamp(3600, 0)
            .expect("target hour should be valid");
        let aggregated_at = chrono::DateTime::<chrono::Utc>::from_timestamp(7200, 0)
            .expect("aggregation time should be valid");
        let hourly = backend
            .aggregate_stats_hourly(&StatsHourlyAggregationInput {
                target_hour_utc: target_hour,
                aggregated_at,
            })
            .await
            .expect("hourly stats aggregation should run")
            .expect("hourly bucket should aggregate");
        assert_eq!(hourly.hour_utc, target_hour);
        assert_eq!(hourly.total_requests, 2);
        assert_eq!(hourly.user_rows, 2);
        assert_eq!(hourly.user_model_rows, 2);
        assert_eq!(hourly.model_rows, 2);
        assert_eq!(hourly.provider_rows, 2);

        let hourly_row = sqlx::query_as::<_, (i64, i64, i64, i64, f64)>(
            r#"
SELECT total_requests, success_requests, error_requests, input_tokens, total_cost
FROM stats_hourly
WHERE hour_utc = 3600
"#,
        )
        .fetch_one(backend.pool())
        .await
        .expect("hourly stats row should load");
        assert_eq!(hourly_row.0, 2);
        assert_eq!(hourly_row.1, 1);
        assert_eq!(hourly_row.2, 1);
        assert_eq!(hourly_row.3, 15);
        assert!((hourly_row.4 - 0.50).abs() < f64::EPSILON);

        let enriched_hourly: (f64, i64, i64, i64, i64, i64) = sqlx::query_as(
            r#"
SELECT response_time_sum_ms, response_time_samples, cache_hit_total_requests,
       cache_hit_requests, completed_total_requests, settled_total_requests
FROM stats_hourly
WHERE hour_utc = 3600
"#,
        )
        .fetch_one(backend.pool())
        .await
        .expect("enriched hourly stats row should load");
        assert!((enriched_hourly.0 - 400.0).abs() < f64::EPSILON);
        assert_eq!(enriched_hourly.1, 2);
        assert_eq!(enriched_hourly.2, 4);
        assert_eq!(enriched_hourly.3, 2);
        assert_eq!(enriched_hourly.4, 2);
        assert_eq!(enriched_hourly.5, 2);

        assert_eq!(sqlite_count(backend.pool(), "stats_hourly_user").await, 2);
        assert_eq!(
            sqlite_count(backend.pool(), "stats_hourly_user_model").await,
            2
        );
        assert_eq!(sqlite_count(backend.pool(), "stats_hourly_model").await, 2);
        assert_eq!(
            sqlite_count(backend.pool(), "stats_hourly_provider").await,
            2
        );
        let hourly_user = sqlx::query_as::<_, (i64, i64, i64, i64, i64, f64)>(
            r#"
SELECT total_requests, success_requests, error_requests, input_tokens, output_tokens, total_cost
FROM stats_hourly_user
WHERE hour_utc = 3600 AND user_id = 'user-2'
"#,
        )
        .fetch_one(backend.pool())
        .await
        .expect("hourly user stats row should load");
        assert_eq!(hourly_user.0, 1);
        assert_eq!(hourly_user.1, 0);
        assert_eq!(hourly_user.2, 1);
        assert_eq!(hourly_user.3, 5);
        assert_eq!(hourly_user.4, 7);
        assert!((hourly_user.5 - 0.20).abs() < f64::EPSILON);
        let hourly_model = sqlx::query_as::<_, (i64, i64, i64, f64, f64)>(
            r#"
SELECT total_requests, input_tokens, output_tokens, total_cost, avg_response_time_ms
FROM stats_hourly_model
WHERE hour_utc = 3600 AND model = 'model-a'
"#,
        )
        .fetch_one(backend.pool())
        .await
        .expect("hourly model stats row should load");
        assert_eq!(hourly_model.0, 1);
        assert_eq!(hourly_model.1, 10);
        assert_eq!(hourly_model.2, 20);
        assert!((hourly_model.3 - 0.30).abs() < f64::EPSILON);
        assert!((hourly_model.4 - 100.0).abs() < f64::EPSILON);

        let second_hourly = backend
            .aggregate_stats_hourly(&StatsHourlyAggregationInput {
                target_hour_utc: target_hour,
                aggregated_at,
            })
            .await
            .expect("second hourly aggregation should run");
        assert!(second_hourly.is_none());

        let target_day = chrono::DateTime::<chrono::Utc>::from_timestamp(0, 0)
            .expect("target day should be valid");
        let daily = backend
            .aggregate_stats_daily(&StatsDailyAggregationInput {
                target_day_utc: target_day,
                aggregated_at,
            })
            .await
            .expect("daily stats aggregation should run")
            .expect("daily bucket should aggregate");
        assert_eq!(daily.day_start_utc, target_day);
        assert_eq!(daily.total_requests, 2);
        assert_eq!(daily.model_rows, 2);
        assert_eq!(daily.provider_rows, 2);
        assert_eq!(daily.api_key_rows, 4);
        assert_eq!(daily.error_rows, 1);
        assert_eq!(daily.user_rows, 2);

        let daily_row = sqlx::query_as::<_, (i64, i64, i64, i64, i64)>(
            r#"
SELECT total_requests, success_requests, error_requests, unique_models, fallback_count
FROM stats_daily
WHERE "date" = 0
"#,
        )
        .fetch_one(backend.pool())
        .await
        .expect("daily stats row should load");
        assert_eq!(daily_row, (2, 1, 1, 2, 1));

        assert_eq!(sqlite_count(backend.pool(), "stats_daily_model").await, 2);
        assert_eq!(
            sqlite_count(backend.pool(), "stats_daily_provider").await,
            2
        );
        assert_eq!(sqlite_count(backend.pool(), "stats_daily_api_key").await, 4);
        assert_eq!(sqlite_count(backend.pool(), "stats_daily_error").await, 1);
        assert_eq!(sqlite_count(backend.pool(), "stats_user_daily").await, 2);
        let daily_model = sqlx::query_as::<_, (i64, i64, i64, i64, i64, f64, f64)>(
            r#"
SELECT total_requests, input_tokens, output_tokens, cache_creation_tokens,
       cache_read_tokens, total_cost, avg_response_time_ms
FROM stats_daily_model
WHERE "date" = 0 AND model = 'model-a'
"#,
        )
        .fetch_one(backend.pool())
        .await
        .expect("daily model stats row should load");
        assert_eq!(daily_model.0, 1);
        assert_eq!(daily_model.1, 10);
        assert_eq!(daily_model.2, 20);
        assert_eq!(daily_model.3, 1);
        assert_eq!(daily_model.4, 2);
        assert!((daily_model.5 - 0.30).abs() < f64::EPSILON);
        assert!((daily_model.6 - 100.0).abs() < f64::EPSILON);
        let daily_error = sqlx::query_as::<_, (String, Option<String>, Option<String>, i64)>(
            r#"
SELECT error_category, provider_name, model, count
FROM stats_daily_error
WHERE "date" = 0
"#,
        )
        .fetch_one(backend.pool())
        .await
        .expect("daily error stats row should load");
        assert_eq!(
            daily_error,
            (
                "upstream_error".to_string(),
                Some("provider-b".to_string()),
                Some("model-b".to_string()),
                1,
            )
        );
        let daily_user = sqlx::query_as::<_, (i64, i64, i64, i64, i64, f64)>(
            r#"
SELECT total_requests, success_requests, error_requests, input_tokens, output_tokens, total_cost
FROM stats_user_daily
WHERE "date" = 0 AND user_id = 'user-2'
"#,
        )
        .fetch_one(backend.pool())
        .await
        .expect("daily user stats row should load");
        assert_eq!(daily_user.0, 1);
        assert_eq!(daily_user.1, 0);
        assert_eq!(daily_user.2, 1);
        assert_eq!(daily_user.3, 5);
        assert_eq!(daily_user.4, 7);
        assert!((daily_user.5 - 0.20).abs() < f64::EPSILON);

        let enriched_daily =
            sqlx::query_as::<_, (i64, i64, f64, i64, i64, i64, i64, i64, i64, Option<i64>)>(
                r#"
SELECT effective_input_tokens, total_input_context, response_time_sum_ms,
       response_time_samples, cache_hit_total_requests, cache_hit_requests,
       completed_total_requests, completed_cache_hit_requests,
       settled_total_requests, p50_response_time_ms
FROM stats_daily
WHERE "date" = 0
"#,
            )
            .fetch_one(backend.pool())
            .await
            .expect("enriched daily stats row should load");
        assert_eq!(enriched_daily.0, 13);
        assert_eq!(enriched_daily.1, 17);
        assert!((enriched_daily.2 - 400.0).abs() < f64::EPSILON);
        assert_eq!(enriched_daily.3, 2);
        assert_eq!(enriched_daily.4, 4);
        assert_eq!(enriched_daily.5, 2);
        assert_eq!(enriched_daily.6, 2);
        assert_eq!(enriched_daily.7, 1);
        assert_eq!(enriched_daily.8, 2);
        assert_eq!(enriched_daily.9, None);

        for (table, expected) in [
            ("stats_user_summary", 2),
            ("stats_user_daily_model", 2),
            ("stats_user_daily_provider", 2),
            ("stats_user_daily_api_format", 2),
            ("stats_daily_model_provider", 2),
            ("stats_user_daily_model_provider", 2),
            ("stats_daily_cost_savings", 1),
            ("stats_daily_cost_savings_provider", 3),
            ("stats_daily_cost_savings_model", 2),
            ("stats_daily_cost_savings_model_provider", 3),
            ("stats_user_daily_cost_savings", 4),
            ("stats_user_daily_cost_savings_provider", 4),
            ("stats_user_daily_cost_savings_model", 4),
            ("stats_user_daily_cost_savings_model_provider", 4),
        ] {
            assert_eq!(
                sqlite_count(backend.pool(), table).await,
                expected,
                "{table}"
            );
        }

        let model_rollup: (i64, i64, i64, f64, i64) = sqlx::query_as(
            r#"
SELECT total_requests, effective_input_tokens, total_tokens,
       response_time_sum_ms, successful_response_time_samples
FROM stats_user_daily_model
WHERE user_id = 'user-1' AND "date" = 0 AND model = 'model-a'
"#,
        )
        .fetch_one(backend.pool())
        .await
        .expect("advanced user model row should load");
        assert_eq!(model_rollup.0, 1);
        assert_eq!(model_rollup.1, 8);
        assert_eq!(model_rollup.2, 31);
        assert!((model_rollup.3 - 100.0).abs() < f64::EPSILON);
        assert_eq!(model_rollup.4, 1);

        let savings: (i64, f64, f64, f64) = sqlx::query_as(
            r#"
SELECT cache_read_tokens, cache_read_cost, cache_creation_cost, estimated_full_cost
FROM stats_daily_cost_savings
WHERE "date" = 0
"#,
        )
        .fetch_one(backend.pool())
        .await
        .expect("daily cost savings row should load");
        assert_eq!(savings.0, 3);
        assert!((savings.1 - 0.03).abs() < 1e-12);
        assert!((savings.2 - 0.01).abs() < 1e-12);
        assert!((savings.3 - 0.00004).abs() < 1e-12);

        let summary: (i64, i64, i64) = sqlx::query_as(
            r#"
SELECT all_time_requests, all_time_input_tokens, active_days
FROM stats_user_summary
WHERE user_id = 'user-1'
"#,
        )
        .fetch_one(backend.pool())
        .await
        .expect("user summary row should load");
        assert_eq!(summary, (1, 10, 1));

        let global_summary: (i64, i64) =
            sqlx::query_as("SELECT all_time_requests, all_time_input_tokens FROM stats_summary")
                .fetch_one(backend.pool())
                .await
                .expect("global stats summary should load");
        assert_eq!(global_summary, (2, 15));
    }
}
