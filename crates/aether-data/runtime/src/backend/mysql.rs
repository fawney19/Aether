use std::sync::Arc;

use crate::database::SqlDatabaseConfig;
use crate::driver::mysql::{MysqlPool, MysqlPoolFactory};
use crate::repository::announcements::{
    AnnouncementReadRepository, AnnouncementWriteRepository, MysqlAnnouncementRepository,
};
use crate::repository::audit::{AuditLogReadRepository, MysqlAuditLogReadRepository};
use crate::repository::auth::{
    AuthApiKeyReadRepository, AuthApiKeyWriteRepository, MysqlAuthApiKeyReadRepository,
};
use crate::repository::auth_modules::{
    AuthModuleReadRepository, AuthModuleWriteRepository, MysqlAuthModuleReadRepository,
    MysqlAuthModuleRepository,
};
use crate::repository::background_tasks::{
    BackgroundTaskReadRepository, BackgroundTaskWriteRepository, MysqlBackgroundTaskRepository,
};
use crate::repository::billing::{BillingReadRepository, MysqlBillingReadRepository};
use crate::repository::candidate_selection::{
    MinimalCandidateSelectionReadRepository, MysqlMinimalCandidateSelectionReadRepository,
};
use crate::repository::candidates::{
    MysqlRequestCandidateRepository, RequestCandidateReadRepository,
    RequestCandidateWriteRepository,
};
use crate::repository::gemini_file_mappings::{
    GeminiFileMappingReadRepository, GeminiFileMappingWriteRepository,
    MysqlGeminiFileMappingRepository,
};
use crate::repository::global_models::{
    GlobalModelReadRepository, GlobalModelWriteRepository, MysqlGlobalModelReadRepository,
};
use crate::repository::management_tokens::{
    ManagementTokenReadRepository, ManagementTokenWriteRepository, MysqlManagementTokenRepository,
};
use crate::repository::oauth_providers::{
    MysqlOAuthProviderRepository, OAuthProviderReadRepository, OAuthProviderWriteRepository,
};
use crate::repository::pool_scores::{
    MysqlPoolMemberScoreRepository, PoolMemberScoreWriteRepository, PoolScoreReadRepository,
};
use crate::repository::provider_catalog::{
    MysqlProviderCatalogReadRepository, ProviderCatalogReadRepository,
    ProviderCatalogWriteRepository,
};
use crate::repository::proxy_nodes::{
    MysqlProxyNodeReadRepository, ProxyNodeReadRepository, ProxyNodeWriteRepository,
};
use crate::repository::quota::{
    MysqlProviderQuotaRepository, ProviderQuotaReadRepository, ProviderQuotaWriteRepository,
};
use crate::repository::routing_profiles::{
    MysqlRoutingGroupRepository, RoutingGroupReadRepository, RoutingGroupWriteRepository,
};
use crate::repository::settlement::{MysqlSettlementRepository, SettlementWriteRepository};
use crate::repository::usage::{
    MysqlUsageReadRepository, MysqlUsageWriteRepository, UsageReadRepository, UsageWriteRepository,
};
use crate::repository::users::{MysqlUserReadRepository, UserReadRepository};
use crate::repository::video_tasks::{
    MysqlVideoTaskRepository, VideoTaskReadRepository, VideoTaskWriteRepository,
};
use crate::repository::wallet::{
    MysqlWalletReadRepository, WalletReadRepository, WalletWriteRepository,
};
use crate::DataLayerError;

#[derive(Debug, Clone)]
pub struct MysqlBackend {
    config: SqlDatabaseConfig,
    pool: MysqlPool,
}

impl MysqlBackend {
    pub fn from_config(config: SqlDatabaseConfig) -> Result<Self, DataLayerError> {
        let factory = MysqlPoolFactory::new(config.clone())?;
        let pool = factory.connect_lazy()?;

        Ok(Self { config, pool })
    }

    pub fn config(&self) -> &SqlDatabaseConfig {
        &self.config
    }

    pub fn pool(&self) -> &MysqlPool {
        &self.pool
    }

    pub fn pool_clone(&self) -> MysqlPool {
        self.pool.clone()
    }

    pub fn auth_api_key_read_repository(&self) -> Arc<dyn AuthApiKeyReadRepository> {
        Arc::new(MysqlAuthApiKeyReadRepository::new(self.pool_clone()))
    }

    pub fn announcement_read_repository(&self) -> Arc<dyn AnnouncementReadRepository> {
        Arc::new(MysqlAnnouncementRepository::new(self.pool_clone()))
    }

    pub fn audit_log_read_repository(&self) -> Arc<dyn AuditLogReadRepository> {
        Arc::new(MysqlAuditLogReadRepository::new(self.pool_clone()))
    }

    pub fn announcement_write_repository(&self) -> Arc<dyn AnnouncementWriteRepository> {
        Arc::new(MysqlAnnouncementRepository::new(self.pool_clone()))
    }

    pub fn auth_api_key_write_repository(&self) -> Arc<dyn AuthApiKeyWriteRepository> {
        Arc::new(MysqlAuthApiKeyReadRepository::new(self.pool_clone()))
    }

    pub fn management_token_read_repository(&self) -> Arc<dyn ManagementTokenReadRepository> {
        Arc::new(MysqlManagementTokenRepository::new(self.pool_clone()))
    }

    pub fn management_token_write_repository(&self) -> Arc<dyn ManagementTokenWriteRepository> {
        Arc::new(MysqlManagementTokenRepository::new(self.pool_clone()))
    }

    pub fn auth_module_read_repository(&self) -> Arc<dyn AuthModuleReadRepository> {
        Arc::new(MysqlAuthModuleReadRepository::new(self.pool_clone()))
    }

    pub fn auth_module_write_repository(&self) -> Arc<dyn AuthModuleWriteRepository> {
        Arc::new(MysqlAuthModuleRepository::new(self.pool_clone()))
    }

    pub fn billing_read_repository(&self) -> Arc<dyn BillingReadRepository> {
        Arc::new(MysqlBillingReadRepository::new(self.pool_clone()))
    }

    pub fn background_task_read_repository(&self) -> Arc<dyn BackgroundTaskReadRepository> {
        Arc::new(MysqlBackgroundTaskRepository::new(self.pool_clone()))
    }

    pub fn background_task_write_repository(&self) -> Arc<dyn BackgroundTaskWriteRepository> {
        Arc::new(MysqlBackgroundTaskRepository::new(self.pool_clone()))
    }

    pub fn request_candidate_read_repository(&self) -> Arc<dyn RequestCandidateReadRepository> {
        Arc::new(MysqlRequestCandidateRepository::new(self.pool_clone()))
    }

    pub fn request_candidate_write_repository(&self) -> Arc<dyn RequestCandidateWriteRepository> {
        Arc::new(MysqlRequestCandidateRepository::new(self.pool_clone()))
    }

    pub fn minimal_candidate_selection_read_repository(
        &self,
    ) -> Arc<dyn MinimalCandidateSelectionReadRepository> {
        Arc::new(MysqlMinimalCandidateSelectionReadRepository::new(
            self.pool_clone(),
        ))
    }

    pub fn gemini_file_mapping_read_repository(&self) -> Arc<dyn GeminiFileMappingReadRepository> {
        Arc::new(MysqlGeminiFileMappingRepository::new(self.pool_clone()))
    }

    pub fn gemini_file_mapping_write_repository(
        &self,
    ) -> Arc<dyn GeminiFileMappingWriteRepository> {
        Arc::new(MysqlGeminiFileMappingRepository::new(self.pool_clone()))
    }

    pub fn global_model_read_repository(&self) -> Arc<dyn GlobalModelReadRepository> {
        Arc::new(MysqlGlobalModelReadRepository::new(self.pool_clone()))
    }

    pub fn global_model_write_repository(&self) -> Arc<dyn GlobalModelWriteRepository> {
        Arc::new(MysqlGlobalModelReadRepository::new(self.pool_clone()))
    }

    pub fn oauth_provider_read_repository(&self) -> Arc<dyn OAuthProviderReadRepository> {
        Arc::new(MysqlOAuthProviderRepository::new(self.pool_clone()))
    }

    pub fn oauth_provider_write_repository(&self) -> Arc<dyn OAuthProviderWriteRepository> {
        Arc::new(MysqlOAuthProviderRepository::new(self.pool_clone()))
    }

    pub fn provider_catalog_read_repository(&self) -> Arc<dyn ProviderCatalogReadRepository> {
        Arc::new(MysqlProviderCatalogReadRepository::new(self.pool_clone()))
    }

    pub fn provider_catalog_write_repository(&self) -> Arc<dyn ProviderCatalogWriteRepository> {
        Arc::new(MysqlProviderCatalogReadRepository::new(self.pool_clone()))
    }

    pub fn pool_score_read_repository(&self) -> Arc<dyn PoolScoreReadRepository> {
        Arc::new(MysqlPoolMemberScoreRepository::new(self.pool_clone()))
    }

    pub fn pool_score_write_repository(&self) -> Arc<dyn PoolMemberScoreWriteRepository> {
        Arc::new(MysqlPoolMemberScoreRepository::new(self.pool_clone()))
    }

    pub fn routing_group_read_repository(&self) -> Arc<dyn RoutingGroupReadRepository> {
        Arc::new(MysqlRoutingGroupRepository::new(self.pool_clone()))
    }

    pub fn routing_group_write_repository(&self) -> Arc<dyn RoutingGroupWriteRepository> {
        Arc::new(MysqlRoutingGroupRepository::new(self.pool_clone()))
    }

    pub fn proxy_node_read_repository(&self) -> Arc<dyn ProxyNodeReadRepository> {
        Arc::new(MysqlProxyNodeReadRepository::new(self.pool_clone()))
    }

    pub fn proxy_node_write_repository(&self) -> Arc<dyn ProxyNodeWriteRepository> {
        Arc::new(MysqlProxyNodeReadRepository::new(self.pool_clone()))
    }

    pub fn provider_quota_read_repository(&self) -> Arc<dyn ProviderQuotaReadRepository> {
        Arc::new(MysqlProviderQuotaRepository::new(self.pool_clone()))
    }

    pub fn provider_quota_write_repository(&self) -> Arc<dyn ProviderQuotaWriteRepository> {
        Arc::new(MysqlProviderQuotaRepository::new(self.pool_clone()))
    }

    pub fn settlement_write_repository(&self) -> Arc<dyn SettlementWriteRepository> {
        Arc::new(MysqlSettlementRepository::new(self.pool_clone()))
    }

    pub fn usage_write_repository(&self) -> Arc<dyn UsageWriteRepository> {
        Arc::new(MysqlUsageWriteRepository::new(self.pool_clone()))
    }

    pub fn usage_read_repository(&self) -> Arc<dyn UsageReadRepository> {
        Arc::new(MysqlUsageReadRepository::new(self.pool_clone()))
    }

    pub fn user_read_repository(&self) -> Arc<dyn UserReadRepository> {
        Arc::new(MysqlUserReadRepository::new(self.pool_clone()))
    }

    pub fn video_task_read_repository(&self) -> Arc<dyn VideoTaskReadRepository> {
        Arc::new(MysqlVideoTaskRepository::new(self.pool_clone()))
    }

    pub fn video_task_write_repository(&self) -> Arc<dyn VideoTaskWriteRepository> {
        Arc::new(MysqlVideoTaskRepository::new(self.pool_clone()))
    }

    pub fn wallet_read_repository(&self) -> Arc<dyn WalletReadRepository> {
        Arc::new(MysqlWalletReadRepository::new(self.pool_clone()))
    }

    pub fn wallet_write_repository(&self) -> Arc<dyn WalletWriteRepository> {
        Arc::new(MysqlWalletReadRepository::new(self.pool_clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::MysqlBackend;
    use crate::lifecycle::migrate::run_mysql_migrations;
    use crate::{
        DatabaseDriver, SqlDatabaseConfig, SqlPoolConfig, StatsDailyAggregationInput,
        StatsHourlyAggregationInput, WalletDailyUsageAggregationInput,
    };

    #[tokio::test]
    async fn backend_retains_config_and_pool() {
        let config = SqlDatabaseConfig {
            driver: DatabaseDriver::Mysql,
            url: "mysql://user:pass@localhost:3306/aether".to_string(),
            pool: SqlPoolConfig::default(),
        };

        let backend = MysqlBackend::from_config(config.clone()).expect("backend should build");

        assert_eq!(backend.config(), &config);
        let _pool = backend.pool();
        let _pool_clone = backend.pool_clone();
    }

    #[tokio::test]
    async fn mysql_wallet_daily_usage_aggregation_uses_settlement_wallets_when_url_is_set() {
        let Some(database_url) = std::env::var("AETHER_TEST_MYSQL_URL")
            .ok()
            .filter(|value| !value.trim().is_empty())
        else {
            eprintln!(
                "skipping mysql wallet daily usage aggregation smoke test because AETHER_TEST_MYSQL_URL is unset"
            );
            return;
        };

        let config = SqlDatabaseConfig {
            driver: DatabaseDriver::Mysql,
            url: database_url,
            pool: SqlPoolConfig {
                max_connections: 1,
                ..SqlPoolConfig::default()
            },
        };
        let backend = MysqlBackend::from_config(config).expect("backend should build");
        run_mysql_migrations(backend.pool())
            .await
            .expect("mysql migrations should run");

        let suffix = format!(
            "{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        );
        let wallet_id = format!("wallet-daily-{suffix}");
        let stale_wallet_id = format!("wallet-daily-stale-{suffix}");
        let timezone = format!("Test/WalletDaily/{suffix}");
        let request_one = format!("request-daily-1-{suffix}");
        let request_two = format!("request-daily-2-{suffix}");
        let request_zero = format!("request-daily-zero-{suffix}");
        let request_outside = format!("request-daily-outside-{suffix}");
        let stale_ledger_id = format!("stale-ledger-{suffix}");
        let unique_offset = chrono::Utc::now()
            .timestamp_nanos_opt()
            .unwrap_or_default()
            .rem_euclid(10_000_000);
        let window_start = 4_100_000_000_i64 + unique_offset * 1_000;
        let window_end = window_start + 200;
        let first_finalized_at = window_start;
        let last_finalized_at = window_start + 100;
        let zero_finalized_at = window_start + 150;
        let outside_finalized_at = window_end;
        let seed_created_at = window_start - 100;
        let aggregated_at = window_end + 100;

        sqlx::query(
            r#"
INSERT INTO wallets (id, user_id, balance, gift_balance, limit_mode, created_at, updated_at)
VALUES
  (?, ?, 10.0, 2.0, 'finite', 1, 1),
  (?, ?, 0.0, 0.0, 'finite', 1, 1)
"#,
        )
        .bind(&wallet_id)
        .bind(format!("user-{wallet_id}"))
        .bind(&stale_wallet_id)
        .bind(format!("user-{stale_wallet_id}"))
        .execute(backend.pool())
        .await
        .expect("wallets should seed");

        sqlx::query(
            r#"
INSERT INTO `usage` (
  request_id, wallet_id, provider_name, model, status, billing_status,
  total_cost_usd, input_tokens, output_tokens, cache_creation_input_tokens,
  cache_read_input_tokens, finalized_at, created_at_unix_ms, updated_at_unix_secs
) VALUES
  (?, 'wrong-wallet', 'provider', 'model', 'completed', 'pending',
   1.25, 10, 20, 3, 4, ?, ?, ?),
  (?, NULL, 'provider', 'model', 'completed', 'pending',
   2.00, 5, 7, 1, 2, ?, ?, ?),
  (?, NULL, 'provider', 'model', 'completed', 'pending',
   0.00, 100, 100, 0, 0, ?, ?, ?),
  (?, NULL, 'provider', 'model', 'completed', 'pending',
   9.00, 50, 50, 0, 0, ?, ?, ?)
"#,
        )
        .bind(&request_one)
        .bind(seed_created_at)
        .bind(seed_created_at * 1000)
        .bind(seed_created_at)
        .bind(&request_two)
        .bind(seed_created_at + 1)
        .bind((seed_created_at + 1) * 1000)
        .bind(seed_created_at + 1)
        .bind(&request_zero)
        .bind(seed_created_at + 2)
        .bind((seed_created_at + 2) * 1000)
        .bind(seed_created_at + 2)
        .bind(&request_outside)
        .bind(seed_created_at + 3)
        .bind((seed_created_at + 3) * 1000)
        .bind(seed_created_at + 3)
        .execute(backend.pool())
        .await
        .expect("usage should seed");

        sqlx::query(
            r#"
INSERT INTO usage_settlement_snapshots (
  request_id, billing_status, wallet_id, finalized_at, created_at, updated_at
) VALUES
  (?, 'settled', ?, ?, ?, ?),
  (?, 'settled', ?, ?, ?, ?),
  (?, 'settled', ?, ?, ?, ?),
  (?, 'settled', ?, ?, ?, ?)
"#,
        )
        .bind(&request_one)
        .bind(&wallet_id)
        .bind(first_finalized_at)
        .bind(first_finalized_at)
        .bind(first_finalized_at)
        .bind(&request_two)
        .bind(&wallet_id)
        .bind(last_finalized_at)
        .bind(last_finalized_at)
        .bind(last_finalized_at)
        .bind(&request_zero)
        .bind(&wallet_id)
        .bind(zero_finalized_at)
        .bind(zero_finalized_at)
        .bind(zero_finalized_at)
        .bind(&request_outside)
        .bind(&wallet_id)
        .bind(outside_finalized_at)
        .bind(outside_finalized_at)
        .bind(outside_finalized_at)
        .execute(backend.pool())
        .await
        .expect("settlement snapshots should seed");

        sqlx::query(
            r#"
INSERT INTO wallet_daily_usage_ledgers (
  id, wallet_id, billing_date, billing_timezone, total_cost_usd,
  total_requests, input_tokens, output_tokens, cache_creation_tokens,
  cache_read_tokens, aggregated_at, created_at, updated_at
) VALUES (?, ?, '2026-05-03', ?, 7.0, 3, 1, 1, 0, 0, ?, ?, ?)
"#,
        )
        .bind(&stale_ledger_id)
        .bind(&stale_wallet_id)
        .bind(&timezone)
        .bind(seed_created_at)
        .bind(seed_created_at)
        .bind(seed_created_at)
        .execute(backend.pool())
        .await
        .expect("stale ledger should seed");

        let summary = backend
            .aggregate_wallet_daily_usage(&WalletDailyUsageAggregationInput {
                billing_date: "2026-05-03".to_string(),
                billing_timezone: timezone.clone(),
                window_start_unix_secs: window_start as u64,
                window_end_unix_secs: window_end as u64,
                aggregated_at_unix_secs: aggregated_at as u64,
            })
            .await
            .expect("wallet daily usage aggregation should run");

        assert_eq!(summary.aggregated_wallets, 1);
        assert_eq!(summary.deleted_stale_ledgers, 1);

        let ledger = sqlx::query_as::<
            _,
            (
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
WHERE wallet_id = ?
  AND billing_date = '2026-05-03'
  AND billing_timezone = ?
"#,
        )
        .bind(&wallet_id)
        .bind(&timezone)
        .fetch_one(backend.pool())
        .await
        .expect("aggregated ledger should load");

        assert_eq!(ledger.0, wallet_id);
        assert!((ledger.1 - 3.25).abs() < f64::EPSILON);
        assert_eq!(ledger.2, 2);
        assert_eq!(ledger.3, 15);
        assert_eq!(ledger.4, 27);
        assert_eq!(ledger.5, 4);
        assert_eq!(ledger.6, 6);
        assert_eq!(ledger.7, Some(first_finalized_at));
        assert_eq!(ledger.8, Some(last_finalized_at));
        assert_eq!(ledger.9, aggregated_at);

        let stale_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM wallet_daily_usage_ledgers WHERE id = ?")
                .bind(&stale_ledger_id)
                .fetch_one(backend.pool())
                .await
                .expect("stale ledger count should load");
        assert_eq!(stale_count, 0);
    }

    #[tokio::test]
    async fn mysql_stats_aggregation_runs_after_mysql_migrations_when_url_is_set() {
        let Some(database_url) = std::env::var("AETHER_TEST_MYSQL_URL")
            .ok()
            .filter(|value| !value.trim().is_empty())
        else {
            eprintln!(
                "skipping mysql stats aggregation smoke test because AETHER_TEST_MYSQL_URL is unset"
            );
            return;
        };

        let config = SqlDatabaseConfig {
            driver: DatabaseDriver::Mysql,
            url: database_url,
            pool: SqlPoolConfig {
                max_connections: 1,
                ..SqlPoolConfig::default()
            },
        };
        let backend = MysqlBackend::from_config(config).expect("backend should build");
        run_mysql_migrations(backend.pool())
            .await
            .expect("mysql migrations should run");

        for sql in [
            "DELETE FROM stats_daily WHERE `date` = 0",
            "DELETE FROM stats_hourly WHERE hour_utc = 3600",
            "DELETE FROM usage_settlement_snapshots WHERE request_id LIKE 'request-daily-%' OR request_id LIKE 'stats-%'",
            "DELETE FROM `usage` WHERE request_id LIKE 'request-%' OR request_id LIKE 'export-request-%' OR request_id LIKE 'stats-%'",
        ] {
            sqlx::query(sql)
                .execute(backend.pool())
                .await
                .expect("stats smoke cleanup should run");
        }

        sqlx::query(
            r#"
INSERT INTO `usage` (
  request_id, user_id, api_key_id, provider_name, model, status, billing_status,
  status_code, error_category, input_tokens, output_tokens,
  cache_creation_input_tokens, cache_read_input_tokens, total_cost_usd,
  actual_total_cost_usd, response_time_ms, created_at_unix_ms, updated_at_unix_secs
) VALUES
  ('stats-1', 'user-1', 'key-1', 'provider-a', 'model-a', 'completed', 'settled',
   200, NULL, 10, 20, 1, 2, 0.30, 0.25, 100, 3600000, 3600),
  ('stats-2', 'user-2', 'key-2', 'provider-b', 'model-b', 'failed', 'void',
   500, 'upstream_error', 5, 7, 0, 1, 0.20, 0.20, 300, 3610000, 3610),
  ('stats-pending', 'user-3', 'key-3', 'provider-a', 'model-a', 'pending', 'pending',
   NULL, NULL, 100, 100, 0, 0, 9.99, 9.99, 50, 3620000, 3620),
  ('stats-unknown-provider', 'user-4', 'key-4', 'unknown', 'model-a', 'completed', 'settled',
   200, NULL, 100, 100, 0, 0, 9.99, 9.99, 50, 3630000, 3630)
"#,
        )
        .execute(backend.pool())
        .await
        .expect("usage stats rows should seed");

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
        assert_eq!(daily.api_key_rows, 2);
        assert_eq!(daily.error_rows, 1);
        assert_eq!(daily.user_rows, 2);

        let daily_row = sqlx::query_as::<_, (i64, i64, i64, i64)>(
            r#"
SELECT total_requests, success_requests, error_requests, unique_models
FROM stats_daily
WHERE `date` = 0
"#,
        )
        .fetch_one(backend.pool())
        .await
        .expect("daily stats row should load");
        assert_eq!(daily_row, (2, 1, 1, 2));
    }
}
