use super::{
    read_decision_trace, read_provider_transport_snapshot, read_request_candidate_trace,
    AdjustWalletBalanceInput, AdminBillingCollectorRecord, AdminBillingCollectorWriteInput,
    AdminBillingMutationOutcome, AdminBillingPresetApplyResult, AdminBillingRuleRecord,
    AdminBillingRuleWriteInput, AdminPaymentOrderListQuery, AdminRedeemCodeBatchListQuery,
    AdminRedeemCodeListQuery, AdminWalletLedgerQuery, AdminWalletListQuery,
    AdminWalletRefundRequestListQuery, AnnouncementListQuery, AuditLogListQuery,
    BackgroundTaskListQuery, BackgroundTaskSummary, BillingModelContextCacheKey, BillingPlanRecord,
    BillingPlanWriteInput, CompleteAdminWalletRefundInput, CreateAdminRedeemCodeBatchInput,
    CreateAdminRedeemCodeBatchResult, CreateAnnouncementRecord, CreateManualWalletRechargeInput,
    CreatePlanPurchaseOrderInput, CreatePlanPurchaseOrderOutcome, CreateWalletRechargeOrderInput,
    CreateWalletRechargeOrderOutcome, CreateWalletRefundRequestInput,
    CreateWalletRefundRequestOutcome, CreditAdminPaymentOrderInput, DataLayerError,
    DatabaseMaintenanceSummary, DecisionTrace, DeleteAdminRedeemCodeBatchInput,
    DisableAdminRedeemCodeBatchInput, DisableAdminRedeemCodeInput, FailAdminWalletRefundInput,
    GatewayDataState, GatewayProviderTransportSnapshot, LocalVideoTaskReadResponse,
    PaymentGatewayConfigRecord, PaymentGatewayConfigWriteInput, ProcessAdminWalletRefundInput,
    ProcessPaymentCallbackInput, ProcessPaymentCallbackOutcome, RedeemWalletCodeInput,
    RedeemWalletCodeOutcome, RequestAuditBundle, RequestCandidateTrace, StoredAdminAuditLogPage,
    StoredAdminPaymentCallbackPage, StoredAdminPaymentOrder, StoredAdminPaymentOrderPage,
    StoredAdminRedeemCodeBatch, StoredAdminRedeemCodeBatchPage, StoredAdminRedeemCodePage,
    StoredAdminWalletLedgerPage, StoredAdminWalletListPage, StoredAdminWalletRefund,
    StoredAdminWalletRefundPage, StoredAdminWalletRefundRequestPage, StoredAdminWalletTransaction,
    StoredAdminWalletTransactionPage, StoredAnnouncement, StoredAnnouncementPage,
    StoredBackgroundTaskEvent, StoredBackgroundTaskRun, StoredBackgroundTaskRunPage,
    StoredBillingModelContext, StoredProviderQuotaSnapshot, StoredProviderUsageSummary,
    StoredRequestUsageAudit, StoredSuspiciousActivity, StoredUsageSettlement,
    StoredUserAuditLogPage, StoredUserAuthRecord, StoredUserExportRow, StoredUserSummary,
    StoredVideoTask, StoredWalletDailyUsageLedger, StoredWalletDailyUsageLedgerPage,
    StoredWalletSnapshot, UpdateAnnouncementRecord, UpsertBackgroundTaskEvent,
    UpsertBackgroundTaskRun, UpsertUsageRecord, UpsertVideoTask, UsageSettlementInput,
    UserDailyQuotaAvailabilityRecord, UserPlanEntitlementRecord, VideoTaskLookupKey,
    VideoTaskModelCount, VideoTaskQueryFilter, VideoTaskStatusCount,
    WalletDailyUsageAggregationInput, WalletDailyUsageAggregationResult, WalletLookupKey,
    WalletMutationOutcome,
};
use aether_data_contracts::repository::usage::{
    PendingUsageCleanupSummary, ProviderApiKeyWindowUsageRequest,
    StoredProviderApiKeyWindowUsageSummary, StoredUsageDailySummary, UsageAuditListQuery,
    UsageCleanupExecutionMode, UsageCleanupSummary, UsageCleanupTargets, UsageCleanupWindow,
    UsageCounterFlushSummary, UsageCounterHealthSnapshot, UsageCounterPendingHealthSnapshot,
    UsageDailyHeatmapQuery,
};
use aether_runtime_state::RuntimeQueueStore;
use aether_video_tasks_core::read_data_backed_video_task_response;
use std::time::{Duration, Instant};
use tokio::time::timeout;

fn normalize_billing_context_cache_part(value: &str) -> String {
    value.trim().to_string()
}

fn normalize_optional_billing_context_cache_part(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

enum BillingModelContextInflightRegistration {
    Leader(u64),
    Follower,
    Bypass,
}

struct BillingModelContextInflightGuard<'a> {
    state: &'a GatewayDataState,
    key: Option<BillingModelContextCacheKey>,
    token: u64,
}

impl<'a> BillingModelContextInflightGuard<'a> {
    fn new(state: &'a GatewayDataState, key: BillingModelContextCacheKey, token: u64) -> Self {
        Self {
            state,
            key: Some(key),
            token,
        }
    }

    fn finish(&mut self) {
        if let Some(key) = self.key.take() {
            self.state
                .finish_billing_model_context_inflight(&key, self.token);
        }
    }
}

impl Drop for BillingModelContextInflightGuard<'_> {
    fn drop(&mut self) {
        self.finish();
    }
}

impl GatewayDataState {
    const MAINTENANCE_POOL_IDLE_RESERVE_ENV: &'static str =
        "AETHER_GATEWAY_MAINTENANCE_POOL_IDLE_RESERVE";
    const MAINTENANCE_POOL_PRESSURE_MAX_DEFER: Duration = Duration::from_secs(30);
    const BILLING_MODEL_CONTEXT_CACHE_TTL: Duration = Duration::from_secs(30);
    const BILLING_MODEL_CONTEXT_CACHE_MAX_ENTRIES: usize = 4096;
    #[cfg(not(test))]
    const BILLING_MODEL_CONTEXT_CACHE_INFLIGHT_WAIT_TIMEOUT: Duration = Duration::from_secs(10);
    #[cfg(test)]
    const BILLING_MODEL_CONTEXT_CACHE_INFLIGHT_WAIT_TIMEOUT: Duration = Duration::from_millis(100);

    pub(crate) async fn run_database_maintenance(
        &self,
        table_names: &[&str],
    ) -> Result<DatabaseMaintenanceSummary, DataLayerError> {
        match &self.backends {
            Some(backends) => backends.run_database_maintenance(table_names).await,
            None => Ok(DatabaseMaintenanceSummary::default()),
        }
    }

    pub(crate) async fn run_database_migrations(
        &self,
    ) -> Result<bool, sqlx::migrate::MigrateError> {
        match &self.backends {
            Some(backends) => backends.run_database_migrations().await,
            None => Ok(false),
        }
    }

    pub(crate) async fn run_database_backfills(&self) -> Result<bool, sqlx::migrate::MigrateError> {
        match &self.backends {
            Some(backends) => backends.run_database_backfills().await,
            None => Ok(false),
        }
    }

    pub(crate) async fn pending_database_migrations(
        &self,
    ) -> Result<
        Option<Vec<aether_data::lifecycle::migrate::PendingMigrationInfo>>,
        sqlx::migrate::MigrateError,
    > {
        match &self.backends {
            Some(backends) => backends.pending_database_migrations().await,
            None => Ok(None),
        }
    }

    pub(crate) async fn prepare_database_for_startup(
        &self,
    ) -> Result<
        Option<Vec<aether_data::lifecycle::migrate::PendingMigrationInfo>>,
        sqlx::migrate::MigrateError,
    > {
        match &self.backends {
            Some(backends) => backends.prepare_database_for_startup().await,
            None => Ok(None),
        }
    }

    pub(crate) async fn warm_database_pool(&self) -> Result<(), DataLayerError> {
        match &self.backends {
            Some(backends) => backends.warm_database_pool().await,
            None => Ok(()),
        }
    }

    pub(crate) async fn pending_database_backfills(
        &self,
    ) -> Result<
        Option<Vec<aether_data::lifecycle::backfill::PendingBackfillInfo>>,
        sqlx::migrate::MigrateError,
    > {
        match &self.backends {
            Some(backends) => backends.pending_database_backfills().await,
            None => Ok(None),
        }
    }

    pub(crate) fn database_pool_summary(&self) -> Option<aether_data::DatabasePoolSummary> {
        self.backends
            .as_ref()
            .and_then(|backends| backends.database_pool_summary())
    }

    pub(crate) async fn postgres_observability_snapshot(
        &self,
    ) -> Result<Option<aether_data::DatabasePostgresObservabilitySnapshot>, DataLayerError> {
        match &self.backends {
            Some(backends) => backends.postgres_observability_snapshot().await,
            None => Ok(None),
        }
    }

    pub(crate) async fn postgres_activity_groups(
        &self,
        limit: i64,
    ) -> Result<Vec<aether_data::DatabasePostgresActivityGroup>, DataLayerError> {
        match &self.backends {
            Some(backends) => backends.postgres_activity_groups(limit).await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) fn database_pool_under_maintenance_pressure(&self) -> bool {
        self.database_pool_summary()
            .as_ref()
            .is_some_and(Self::database_pool_summary_under_maintenance_pressure)
    }

    pub(crate) fn database_pool_summary_under_maintenance_pressure(
        summary: &aether_data::DatabasePoolSummary,
    ) -> bool {
        summary.checked_out > 0
            && Self::database_pool_available_capacity(summary)
                <= Self::maintenance_pool_idle_reserve(summary)
    }

    pub(crate) fn database_pool_summary_under_usage_worker_pressure(
        summary: &aether_data::DatabasePoolSummary,
    ) -> bool {
        summary.checked_out > 0
            && Self::database_pool_available_capacity(summary)
                <= Self::usage_worker_pool_idle_reserve(summary)
    }

    fn database_pool_available_capacity(summary: &aether_data::DatabasePoolSummary) -> usize {
        let unopened = (summary.max_connections as usize).saturating_sub(summary.pool_size);
        summary.idle.saturating_add(unopened)
    }

    pub(crate) fn maintenance_pool_idle_reserve(
        summary: &aether_data::DatabasePoolSummary,
    ) -> usize {
        if let Some(override_value) = std::env::var(Self::MAINTENANCE_POOL_IDLE_RESERVE_ENV)
            .ok()
            .and_then(|value| value.trim().parse::<usize>().ok())
        {
            return override_value;
        }

        let max_connections = summary.max_connections as usize;
        if max_connections == 0 {
            return 0;
        }

        let ten_percent_ceil = (max_connections + 9) / 10;
        ten_percent_ceil.clamp(2, 10).min(max_connections)
    }

    fn usage_worker_pool_idle_reserve(summary: &aether_data::DatabasePoolSummary) -> usize {
        if summary.max_connections <= 1 {
            return 0;
        }
        1
    }

    pub(crate) fn should_defer_maintenance_for_database_pool_pressure(
        &self,
        deferred_since: &mut Option<Instant>,
    ) -> bool {
        Self::should_defer_maintenance_for_pool_pressure_state(
            self.database_pool_under_maintenance_pressure(),
            deferred_since,
        )
    }

    pub(crate) fn should_defer_maintenance_for_pool_pressure_state(
        pool_under_pressure: bool,
        deferred_since: &mut Option<Instant>,
    ) -> bool {
        if !pool_under_pressure {
            *deferred_since = None;
            return false;
        }

        let now = Instant::now();
        let since = deferred_since.get_or_insert(now);
        if now.duration_since(*since) >= Self::MAINTENANCE_POOL_PRESSURE_MAX_DEFER {
            *deferred_since = None;
            return false;
        }

        true
    }

    pub(crate) async fn aggregate_wallet_daily_usage(
        &self,
        input: &WalletDailyUsageAggregationInput,
    ) -> Result<WalletDailyUsageAggregationResult, DataLayerError> {
        match &self.backends {
            Some(backends) => backends.aggregate_wallet_daily_usage(input).await,
            None => Ok(WalletDailyUsageAggregationResult::default()),
        }
    }

    pub(crate) async fn aggregate_stats_hourly(
        &self,
        input: &aether_data::StatsHourlyAggregationInput,
    ) -> Result<Option<aether_data::StatsHourlyAggregationSummary>, DataLayerError> {
        match &self.backends {
            Some(backends) => backends.aggregate_stats_hourly(input).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn aggregate_stats_daily(
        &self,
        input: &aether_data::StatsDailyAggregationInput,
    ) -> Result<Option<aether_data::StatsDailyAggregationSummary>, DataLayerError> {
        match &self.backends {
            Some(backends) => backends.aggregate_stats_daily(input).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn list_announcements(
        &self,
        query: &AnnouncementListQuery,
    ) -> Result<StoredAnnouncementPage, DataLayerError> {
        match &self.announcement_reader {
            Some(repository) => repository.list_announcements(query).await,
            None => Ok(StoredAnnouncementPage::default()),
        }
    }

    pub(crate) async fn find_announcement_by_id(
        &self,
        announcement_id: &str,
    ) -> Result<Option<StoredAnnouncement>, DataLayerError> {
        match &self.announcement_reader {
            Some(repository) => repository.find_by_id(announcement_id).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn list_admin_audit_logs(
        &self,
        query: &AuditLogListQuery,
    ) -> Result<StoredAdminAuditLogPage, DataLayerError> {
        let Some(repository) = self
            .backends
            .as_ref()
            .and_then(|backends| backends.read().audit_logs())
        else {
            return Ok(StoredAdminAuditLogPage {
                items: Vec::new(),
                total: 0,
            });
        };
        repository.list_admin_audit_logs(query).await
    }

    pub(crate) async fn list_admin_suspicious_activities(
        &self,
        cutoff_unix_secs: u64,
    ) -> Result<Vec<StoredSuspiciousActivity>, DataLayerError> {
        let Some(repository) = self
            .backends
            .as_ref()
            .and_then(|backends| backends.read().audit_logs())
        else {
            return Ok(Vec::new());
        };
        repository
            .list_admin_suspicious_activities(cutoff_unix_secs)
            .await
    }

    pub(crate) async fn read_admin_user_behavior_event_counts(
        &self,
        user_id: &str,
        cutoff_unix_secs: u64,
    ) -> Result<std::collections::BTreeMap<String, u64>, DataLayerError> {
        let Some(repository) = self
            .backends
            .as_ref()
            .and_then(|backends| backends.read().audit_logs())
        else {
            return Ok(std::collections::BTreeMap::new());
        };
        repository
            .read_admin_user_behavior_event_counts(user_id, cutoff_unix_secs)
            .await
    }

    pub(crate) async fn list_user_audit_logs(
        &self,
        user_id: &str,
        query: &AuditLogListQuery,
    ) -> Result<StoredUserAuditLogPage, DataLayerError> {
        let Some(repository) = self
            .backends
            .as_ref()
            .and_then(|backends| backends.read().audit_logs())
        else {
            return Ok(StoredUserAuditLogPage {
                items: Vec::new(),
                total: 0,
            });
        };
        repository.list_user_audit_logs(user_id, query).await
    }

    pub(crate) async fn delete_audit_logs_before(
        &self,
        cutoff_unix_secs: u64,
        limit: usize,
    ) -> Result<usize, DataLayerError> {
        let Some(repository) = self
            .backends
            .as_ref()
            .and_then(|backends| backends.read().audit_logs())
        else {
            return Ok(0);
        };
        repository
            .delete_audit_logs_before(cutoff_unix_secs, limit)
            .await
    }

    pub(crate) async fn count_unread_active_announcements(
        &self,
        user_id: &str,
        now_unix_secs: u64,
    ) -> Result<u64, DataLayerError> {
        match &self.announcement_reader {
            Some(repository) => {
                repository
                    .count_unread_active_announcements(user_id, now_unix_secs)
                    .await
            }
            None => Ok(0),
        }
    }

    pub(crate) async fn list_required_unread_active_announcements(
        &self,
        user_id: &str,
        now_unix_secs: u64,
        limit: usize,
    ) -> Result<Vec<StoredAnnouncement>, DataLayerError> {
        match &self.announcement_reader {
            Some(repository) => {
                repository
                    .list_required_unread_active_announcements(user_id, now_unix_secs, limit)
                    .await
            }
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn create_announcement(
        &self,
        record: CreateAnnouncementRecord,
    ) -> Result<Option<StoredAnnouncement>, DataLayerError> {
        match &self.announcement_writer {
            Some(repository) => repository.create_announcement(record).await.map(Some),
            None => Ok(None),
        }
    }

    pub(crate) async fn update_announcement(
        &self,
        record: UpdateAnnouncementRecord,
    ) -> Result<Option<StoredAnnouncement>, DataLayerError> {
        match &self.announcement_writer {
            Some(repository) => repository.update_announcement(record).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn delete_announcement(
        &self,
        announcement_id: &str,
    ) -> Result<bool, DataLayerError> {
        match &self.announcement_writer {
            Some(repository) => repository.delete_announcement(announcement_id).await,
            None => Ok(false),
        }
    }

    pub(crate) async fn mark_announcement_as_read(
        &self,
        user_id: &str,
        announcement_id: &str,
        read_at_unix_secs: u64,
    ) -> Result<bool, DataLayerError> {
        match &self.announcement_writer {
            Some(repository) => {
                repository
                    .mark_announcement_as_read(user_id, announcement_id, read_at_unix_secs)
                    .await
            }
            None => Ok(false),
        }
    }

    pub(crate) async fn find_video_task(
        &self,
        key: VideoTaskLookupKey<'_>,
    ) -> Result<Option<StoredVideoTask>, DataLayerError> {
        match &self.video_task_reader {
            Some(repository) => repository.find(key).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn list_video_task_page(
        &self,
        filter: &VideoTaskQueryFilter,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<StoredVideoTask>, DataLayerError> {
        match &self.video_task_reader {
            Some(repository) => repository.list_page(filter, offset, limit).await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn list_video_task_page_summary(
        &self,
        filter: &VideoTaskQueryFilter,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<StoredVideoTask>, DataLayerError> {
        match &self.video_task_reader {
            Some(repository) => repository.list_page_summary(filter, offset, limit).await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn count_video_tasks(
        &self,
        filter: &VideoTaskQueryFilter,
    ) -> Result<u64, DataLayerError> {
        match &self.video_task_reader {
            Some(repository) => repository.count(filter).await,
            None => Ok(0),
        }
    }

    pub(crate) async fn count_video_tasks_by_status(
        &self,
        filter: &VideoTaskQueryFilter,
    ) -> Result<Vec<VideoTaskStatusCount>, DataLayerError> {
        match &self.video_task_reader {
            Some(repository) => repository.count_by_status(filter).await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn count_distinct_video_task_users(
        &self,
        filter: &VideoTaskQueryFilter,
    ) -> Result<u64, DataLayerError> {
        match &self.video_task_reader {
            Some(repository) => repository.count_distinct_users(filter).await,
            None => Ok(0),
        }
    }

    pub(crate) async fn top_video_task_models(
        &self,
        filter: &VideoTaskQueryFilter,
        limit: usize,
    ) -> Result<Vec<VideoTaskModelCount>, DataLayerError> {
        match &self.video_task_reader {
            Some(repository) => repository.top_models(filter, limit).await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn count_video_tasks_created_since(
        &self,
        filter: &VideoTaskQueryFilter,
        created_since_unix_secs: u64,
    ) -> Result<u64, DataLayerError> {
        match &self.video_task_reader {
            Some(repository) => {
                repository
                    .count_created_since(filter, created_since_unix_secs)
                    .await
            }
            None => Ok(0),
        }
    }

    pub(crate) async fn upsert_video_task(
        &self,
        task: UpsertVideoTask,
    ) -> Result<Option<StoredVideoTask>, DataLayerError> {
        match &self.video_task_writer {
            Some(repository) => repository.upsert(task).await.map(Some),
            None => Ok(None),
        }
    }

    pub(crate) async fn update_active_video_task(
        &self,
        task: UpsertVideoTask,
    ) -> Result<Option<StoredVideoTask>, DataLayerError> {
        match &self.video_task_writer {
            Some(repository) => repository.update_if_active(task).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn claim_due_video_tasks(
        &self,
        now_unix_secs: u64,
        claim_until_unix_secs: u64,
        limit: usize,
    ) -> Result<Vec<StoredVideoTask>, DataLayerError> {
        match &self.video_task_writer {
            Some(repository) => {
                repository
                    .claim_due(now_unix_secs, claim_until_unix_secs, limit)
                    .await
            }
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn find_wallet(
        &self,
        key: WalletLookupKey<'_>,
    ) -> Result<Option<StoredWalletSnapshot>, DataLayerError> {
        match &self.wallet_reader {
            Some(repository) => repository.find(key).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn list_wallets_by_api_key_ids(
        &self,
        api_key_ids: &[String],
    ) -> Result<Vec<StoredWalletSnapshot>, DataLayerError> {
        match &self.wallet_reader {
            Some(repository) => repository.list_wallets_by_api_key_ids(api_key_ids).await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn list_wallets_by_user_ids(
        &self,
        user_ids: &[String],
    ) -> Result<Vec<StoredWalletSnapshot>, DataLayerError> {
        match &self.wallet_reader {
            Some(repository) => repository.list_wallets_by_user_ids(user_ids).await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn list_admin_wallets(
        &self,
        query: &AdminWalletListQuery,
    ) -> Result<StoredAdminWalletListPage, DataLayerError> {
        match &self.wallet_reader {
            Some(repository) => repository.list_admin_wallets(query).await,
            None => Ok(StoredAdminWalletListPage::default()),
        }
    }

    pub(crate) async fn list_admin_wallet_ledger(
        &self,
        query: &AdminWalletLedgerQuery,
    ) -> Result<StoredAdminWalletLedgerPage, DataLayerError> {
        match &self.wallet_reader {
            Some(repository) => repository.list_admin_wallet_ledger(query).await,
            None => Ok(StoredAdminWalletLedgerPage::default()),
        }
    }

    pub(crate) async fn list_admin_wallet_refund_requests(
        &self,
        query: &AdminWalletRefundRequestListQuery,
    ) -> Result<StoredAdminWalletRefundRequestPage, DataLayerError> {
        match &self.wallet_reader {
            Some(repository) => repository.list_admin_wallet_refund_requests(query).await,
            None => Ok(StoredAdminWalletRefundRequestPage::default()),
        }
    }

    pub(crate) async fn list_admin_wallet_transactions(
        &self,
        wallet_id: &str,
        limit: usize,
        offset: usize,
    ) -> Result<StoredAdminWalletTransactionPage, DataLayerError> {
        match &self.wallet_reader {
            Some(repository) => {
                repository
                    .list_admin_wallet_transactions(wallet_id, limit, offset)
                    .await
            }
            None => Ok(StoredAdminWalletTransactionPage::default()),
        }
    }

    pub(crate) async fn find_wallet_today_usage(
        &self,
        wallet_id: &str,
        billing_timezone: &str,
    ) -> Result<Option<StoredWalletDailyUsageLedger>, DataLayerError> {
        match &self.wallet_reader {
            Some(repository) => {
                repository
                    .find_wallet_today_usage(wallet_id, billing_timezone)
                    .await
            }
            None => Ok(None),
        }
    }

    pub(crate) async fn list_wallet_daily_usage_history(
        &self,
        wallet_id: &str,
        billing_timezone: &str,
        limit: usize,
    ) -> Result<StoredWalletDailyUsageLedgerPage, DataLayerError> {
        match &self.wallet_reader {
            Some(repository) => {
                repository
                    .list_wallet_daily_usage_history(wallet_id, billing_timezone, limit)
                    .await
            }
            None => Ok(StoredWalletDailyUsageLedgerPage::default()),
        }
    }

    pub(crate) async fn list_admin_wallet_refunds(
        &self,
        wallet_id: &str,
        limit: usize,
        offset: usize,
    ) -> Result<StoredAdminWalletRefundPage, DataLayerError> {
        match &self.wallet_reader {
            Some(repository) => {
                repository
                    .list_admin_wallet_refunds(wallet_id, limit, offset)
                    .await
            }
            None => Ok(StoredAdminWalletRefundPage::default()),
        }
    }

    pub(crate) async fn list_admin_payment_orders(
        &self,
        query: &AdminPaymentOrderListQuery,
    ) -> Result<StoredAdminPaymentOrderPage, DataLayerError> {
        match &self.wallet_reader {
            Some(repository) => repository.list_admin_payment_orders(query).await,
            None => Ok(StoredAdminPaymentOrderPage::default()),
        }
    }

    pub(crate) async fn list_admin_payment_callbacks(
        &self,
        payment_method: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<StoredAdminPaymentCallbackPage, DataLayerError> {
        match &self.wallet_reader {
            Some(repository) => {
                repository
                    .list_admin_payment_callbacks(payment_method, limit, offset)
                    .await
            }
            None => Ok(StoredAdminPaymentCallbackPage::default()),
        }
    }

    pub(crate) async fn list_admin_redeem_code_batches(
        &self,
        query: &AdminRedeemCodeBatchListQuery,
    ) -> Result<StoredAdminRedeemCodeBatchPage, DataLayerError> {
        match &self.wallet_reader {
            Some(repository) => repository.list_admin_redeem_code_batches(query).await,
            None => Ok(StoredAdminRedeemCodeBatchPage::default()),
        }
    }

    pub(crate) async fn find_admin_redeem_code_batch(
        &self,
        batch_id: &str,
    ) -> Result<Option<StoredAdminRedeemCodeBatch>, DataLayerError> {
        match &self.wallet_reader {
            Some(repository) => repository.find_admin_redeem_code_batch(batch_id).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn list_admin_redeem_codes(
        &self,
        query: &AdminRedeemCodeListQuery,
    ) -> Result<StoredAdminRedeemCodePage, DataLayerError> {
        match &self.wallet_reader {
            Some(repository) => repository.list_admin_redeem_codes(query).await,
            None => Ok(StoredAdminRedeemCodePage::default()),
        }
    }

    pub(crate) async fn find_admin_payment_order(
        &self,
        order_id: &str,
    ) -> Result<Option<StoredAdminPaymentOrder>, DataLayerError> {
        match &self.wallet_reader {
            Some(repository) => repository.find_admin_payment_order(order_id).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn list_wallet_payment_orders_by_user_id(
        &self,
        user_id: &str,
        limit: usize,
        offset: usize,
    ) -> Result<StoredAdminPaymentOrderPage, DataLayerError> {
        match &self.wallet_reader {
            Some(repository) => {
                repository
                    .list_wallet_payment_orders_by_user_id(user_id, limit, offset)
                    .await
            }
            None => Ok(StoredAdminPaymentOrderPage::default()),
        }
    }

    pub(crate) async fn find_wallet_payment_order_by_user_id(
        &self,
        user_id: &str,
        order_id: &str,
    ) -> Result<Option<StoredAdminPaymentOrder>, DataLayerError> {
        match &self.wallet_reader {
            Some(repository) => {
                repository
                    .find_wallet_payment_order_by_user_id(user_id, order_id)
                    .await
            }
            None => Ok(None),
        }
    }

    pub(crate) async fn find_pending_plan_purchase_order_by_user_id(
        &self,
        user_id: &str,
        product_id: &str,
    ) -> Result<Option<StoredAdminPaymentOrder>, DataLayerError> {
        match &self.wallet_reader {
            Some(repository) => {
                repository
                    .find_pending_plan_purchase_order_by_user_id(user_id, product_id)
                    .await
            }
            None => Ok(None),
        }
    }

    pub(crate) async fn find_wallet_refund(
        &self,
        wallet_id: &str,
        refund_id: &str,
    ) -> Result<Option<aether_data::repository::wallet::StoredAdminWalletRefund>, DataLayerError>
    {
        match &self.wallet_reader {
            Some(repository) => repository.find_wallet_refund(wallet_id, refund_id).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn create_wallet_recharge_order(
        &self,
        input: CreateWalletRechargeOrderInput,
    ) -> Result<Option<CreateWalletRechargeOrderOutcome>, DataLayerError> {
        match &self.wallet_writer {
            Some(repository) => repository
                .create_wallet_recharge_order(input)
                .await
                .map(Some),
            None => Ok(None),
        }
    }

    pub(crate) async fn create_plan_purchase_order(
        &self,
        input: CreatePlanPurchaseOrderInput,
    ) -> Result<Option<CreatePlanPurchaseOrderOutcome>, DataLayerError> {
        match &self.wallet_writer {
            Some(repository) => repository.create_plan_purchase_order(input).await.map(Some),
            None => Ok(None),
        }
    }

    pub(crate) async fn create_wallet_refund_request(
        &self,
        input: CreateWalletRefundRequestInput,
    ) -> Result<Option<CreateWalletRefundRequestOutcome>, DataLayerError> {
        match &self.wallet_writer {
            Some(repository) => repository
                .create_wallet_refund_request(input)
                .await
                .map(Some),
            None => Ok(None),
        }
    }

    pub(crate) async fn process_payment_callback(
        &self,
        input: ProcessPaymentCallbackInput,
    ) -> Result<Option<ProcessPaymentCallbackOutcome>, DataLayerError> {
        match &self.wallet_writer {
            Some(repository) => repository.process_payment_callback(input).await.map(Some),
            None => Ok(None),
        }
    }

    pub(crate) async fn adjust_wallet_balance(
        &self,
        input: AdjustWalletBalanceInput,
    ) -> Result<Option<(StoredWalletSnapshot, StoredAdminWalletTransaction)>, DataLayerError> {
        match &self.wallet_writer {
            Some(repository) => repository.adjust_wallet_balance(input).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn create_manual_wallet_recharge(
        &self,
        input: CreateManualWalletRechargeInput,
    ) -> Result<Option<(StoredWalletSnapshot, StoredAdminPaymentOrder)>, DataLayerError> {
        match &self.wallet_writer {
            Some(repository) => repository.create_manual_wallet_recharge(input).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn process_admin_wallet_refund(
        &self,
        input: ProcessAdminWalletRefundInput,
    ) -> Result<
        Option<
            WalletMutationOutcome<(
                StoredWalletSnapshot,
                StoredAdminWalletRefund,
                StoredAdminWalletTransaction,
            )>,
        >,
        DataLayerError,
    > {
        match &self.wallet_writer {
            Some(repository) => repository
                .process_admin_wallet_refund(input)
                .await
                .map(Some),
            None => Ok(None),
        }
    }

    pub(crate) async fn complete_admin_wallet_refund(
        &self,
        input: CompleteAdminWalletRefundInput,
    ) -> Result<Option<WalletMutationOutcome<StoredAdminWalletRefund>>, DataLayerError> {
        match &self.wallet_writer {
            Some(repository) => repository
                .complete_admin_wallet_refund(input)
                .await
                .map(Some),
            None => Ok(None),
        }
    }

    pub(crate) async fn fail_admin_wallet_refund(
        &self,
        input: FailAdminWalletRefundInput,
    ) -> Result<
        Option<
            WalletMutationOutcome<(
                StoredWalletSnapshot,
                StoredAdminWalletRefund,
                Option<StoredAdminWalletTransaction>,
            )>,
        >,
        DataLayerError,
    > {
        match &self.wallet_writer {
            Some(repository) => repository.fail_admin_wallet_refund(input).await.map(Some),
            None => Ok(None),
        }
    }

    pub(crate) async fn expire_admin_payment_order(
        &self,
        order_id: &str,
    ) -> Result<Option<WalletMutationOutcome<(StoredAdminPaymentOrder, bool)>>, DataLayerError>
    {
        match &self.wallet_writer {
            Some(repository) => repository
                .expire_admin_payment_order(order_id)
                .await
                .map(Some),
            None => Ok(None),
        }
    }

    pub(crate) async fn fail_admin_payment_order(
        &self,
        order_id: &str,
    ) -> Result<Option<WalletMutationOutcome<StoredAdminPaymentOrder>>, DataLayerError> {
        match &self.wallet_writer {
            Some(repository) => repository
                .fail_admin_payment_order(order_id)
                .await
                .map(Some),
            None => Ok(None),
        }
    }

    pub(crate) async fn credit_admin_payment_order(
        &self,
        input: CreditAdminPaymentOrderInput,
    ) -> Result<Option<WalletMutationOutcome<(StoredAdminPaymentOrder, bool)>>, DataLayerError>
    {
        match &self.wallet_writer {
            Some(repository) => repository.credit_admin_payment_order(input).await.map(Some),
            None => Ok(None),
        }
    }

    pub(crate) async fn create_admin_redeem_code_batch(
        &self,
        input: CreateAdminRedeemCodeBatchInput,
    ) -> Result<Option<CreateAdminRedeemCodeBatchResult>, DataLayerError> {
        match &self.wallet_writer {
            Some(repository) => repository
                .create_admin_redeem_code_batch(input)
                .await
                .map(Some),
            None => Ok(None),
        }
    }

    pub(crate) async fn disable_admin_redeem_code_batch(
        &self,
        input: DisableAdminRedeemCodeBatchInput,
    ) -> Result<Option<WalletMutationOutcome<StoredAdminRedeemCodeBatch>>, DataLayerError> {
        match &self.wallet_writer {
            Some(repository) => repository
                .disable_admin_redeem_code_batch(input)
                .await
                .map(Some),
            None => Ok(None),
        }
    }

    pub(crate) async fn delete_admin_redeem_code_batch(
        &self,
        input: DeleteAdminRedeemCodeBatchInput,
    ) -> Result<Option<WalletMutationOutcome<StoredAdminRedeemCodeBatch>>, DataLayerError> {
        match &self.wallet_writer {
            Some(repository) => repository
                .delete_admin_redeem_code_batch(input)
                .await
                .map(Some),
            None => Ok(None),
        }
    }

    pub(crate) async fn disable_admin_redeem_code(
        &self,
        input: DisableAdminRedeemCodeInput,
    ) -> Result<
        Option<WalletMutationOutcome<aether_data::repository::wallet::StoredAdminRedeemCode>>,
        DataLayerError,
    > {
        match &self.wallet_writer {
            Some(repository) => repository.disable_admin_redeem_code(input).await.map(Some),
            None => Ok(None),
        }
    }

    pub(crate) async fn redeem_wallet_code(
        &self,
        input: RedeemWalletCodeInput,
    ) -> Result<Option<RedeemWalletCodeOutcome>, DataLayerError> {
        match &self.wallet_writer {
            Some(repository) => repository.redeem_wallet_code(input).await.map(Some),
            None => Ok(None),
        }
    }

    pub(crate) async fn settle_usage(
        &self,
        input: UsageSettlementInput,
    ) -> Result<Option<StoredUsageSettlement>, DataLayerError> {
        match &self.settlement_writer {
            Some(repository) => repository.settle_usage(input).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn reset_due_provider_quotas(
        &self,
        now_unix_secs: u64,
    ) -> Result<usize, DataLayerError> {
        match &self.provider_quota_writer {
            Some(repository) => repository.reset_due(now_unix_secs).await,
            None => Ok(0),
        }
    }

    pub(crate) async fn find_provider_quota_by_provider_id(
        &self,
        provider_id: &str,
    ) -> Result<Option<StoredProviderQuotaSnapshot>, DataLayerError> {
        match &self.provider_quota_reader {
            Some(repository) => repository.find_by_provider_id(provider_id).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn find_provider_quotas_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderQuotaSnapshot>, DataLayerError> {
        match &self.provider_quota_reader {
            Some(repository) => repository.find_by_provider_ids(provider_ids).await,
            None => Ok(Vec::new()),
        }
    }

    #[allow(dead_code)]

    pub(crate) async fn upsert_usage(
        &self,
        usage: UpsertUsageRecord,
    ) -> Result<Option<StoredRequestUsageAudit>, DataLayerError> {
        crate::request_diagnostics::observe_db_operation(
            "usage_upsert",
            self.database_pool_summary(),
            async {
                match &self.usage_writer {
                    Some(repository) => repository.upsert(usage).await.map(Some),
                    None => Ok(None),
                }
            },
        )
        .await
    }

    #[allow(dead_code)]
    pub(crate) async fn rebuild_api_key_usage_stats(&self) -> Result<u64, DataLayerError> {
        match &self.usage_writer {
            Some(repository) => repository.rebuild_api_key_usage_stats().await,
            None => Ok(0),
        }
    }

    #[allow(dead_code)]
    pub(crate) async fn rebuild_provider_api_key_usage_stats(&self) -> Result<u64, DataLayerError> {
        match &self.usage_writer {
            Some(repository) => repository.rebuild_provider_api_key_usage_stats().await,
            None => Ok(0),
        }
    }

    pub(crate) async fn flush_usage_counter_deltas(
        &self,
        batch_size: usize,
    ) -> Result<UsageCounterFlushSummary, DataLayerError> {
        match &self.usage_writer {
            Some(repository) => repository.flush_usage_counter_deltas(batch_size).await,
            None => Ok(UsageCounterFlushSummary::default()),
        }
    }

    pub(crate) async fn cleanup_processed_usage_counter_deltas(
        &self,
        cutoff_unix_secs: u64,
        batch_size: usize,
    ) -> Result<usize, DataLayerError> {
        match &self.usage_writer {
            Some(repository) => {
                repository
                    .cleanup_processed_usage_counter_deltas(cutoff_unix_secs, batch_size)
                    .await
            }
            None => Ok(0),
        }
    }

    pub(crate) async fn cleanup_stale_pending_requests(
        &self,
        cutoff_unix_secs: u64,
        now_unix_secs: u64,
        timeout_minutes: u64,
        batch_size: usize,
    ) -> Result<PendingUsageCleanupSummary, DataLayerError> {
        match &self.usage_writer {
            Some(repository) => {
                repository
                    .cleanup_stale_pending_requests(
                        cutoff_unix_secs,
                        now_unix_secs,
                        timeout_minutes,
                        batch_size,
                    )
                    .await
            }
            None => Ok(PendingUsageCleanupSummary::default()),
        }
    }

    pub(crate) async fn cleanup_usage(
        &self,
        window: &UsageCleanupWindow,
        batch_size: usize,
        auto_delete_expired_keys: bool,
        targets: UsageCleanupTargets,
        mode: UsageCleanupExecutionMode,
    ) -> Result<UsageCleanupSummary, DataLayerError> {
        match &self.usage_writer {
            Some(repository) => {
                repository
                    .cleanup_usage(window, batch_size, auto_delete_expired_keys, targets, mode)
                    .await
            }
            None => Ok(UsageCleanupSummary::default()),
        }
    }

    pub(crate) async fn preview_usage_cleanup(
        &self,
        window: &UsageCleanupWindow,
        targets: UsageCleanupTargets,
        mode: UsageCleanupExecutionMode,
    ) -> Result<aether_data_contracts::repository::usage::UsageCleanupPreviewCounts, DataLayerError>
    {
        match &self.usage_writer {
            Some(repository) => {
                repository
                    .preview_usage_cleanup(window, targets, mode)
                    .await
            }
            None => {
                Ok(aether_data_contracts::repository::usage::UsageCleanupPreviewCounts::default())
            }
        }
    }

    pub(crate) async fn find_request_usage_by_request_id(
        &self,
        request_id: &str,
    ) -> Result<Option<StoredRequestUsageAudit>, DataLayerError> {
        match &self.usage_reader {
            Some(repository) => repository.find_by_request_id(request_id).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn find_request_usage_by_request_id_shallow(
        &self,
        request_id: &str,
    ) -> Result<Option<StoredRequestUsageAudit>, DataLayerError> {
        match &self.usage_reader {
            Some(repository) => repository.find_by_request_id_shallow(request_id).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn find_request_usage_by_id(
        &self,
        usage_id: &str,
    ) -> Result<Option<StoredRequestUsageAudit>, DataLayerError> {
        match &self.usage_reader {
            Some(repository) => repository.find_by_id(usage_id).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn list_request_usage_by_ids(
        &self,
        usage_ids: &[String],
    ) -> Result<Vec<StoredRequestUsageAudit>, DataLayerError> {
        match &self.usage_reader {
            Some(repository) => repository.list_by_ids(usage_ids).await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn resolve_request_usage_body_ref(
        &self,
        body_ref: &str,
    ) -> Result<Option<serde_json::Value>, DataLayerError> {
        match &self.usage_reader {
            Some(repository) => repository.resolve_body_ref(body_ref).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn list_usage_audits(
        &self,
        query: &UsageAuditListQuery,
    ) -> Result<Vec<StoredRequestUsageAudit>, DataLayerError> {
        match &self.usage_reader {
            Some(repository) => repository.list_usage_audits(query).await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn count_usage_audits(
        &self,
        query: &UsageAuditListQuery,
    ) -> Result<u64, DataLayerError> {
        match &self.usage_reader {
            Some(repository) => repository.count_usage_audits(query).await,
            None => Ok(0),
        }
    }

    pub(crate) async fn list_usage_audits_by_keyword_search(
        &self,
        query: &aether_data_contracts::repository::usage::UsageAuditKeywordSearchQuery,
    ) -> Result<Vec<StoredRequestUsageAudit>, DataLayerError> {
        match &self.usage_reader {
            Some(repository) => repository.list_usage_audits_by_keyword_search(query).await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn count_usage_audits_by_keyword_search(
        &self,
        query: &aether_data_contracts::repository::usage::UsageAuditKeywordSearchQuery,
    ) -> Result<u64, DataLayerError> {
        match &self.usage_reader {
            Some(repository) => repository.count_usage_audits_by_keyword_search(query).await,
            None => Ok(0),
        }
    }

    pub(crate) async fn aggregate_usage_audits(
        &self,
        query: &aether_data_contracts::repository::usage::UsageAuditAggregationQuery,
    ) -> Result<
        Vec<aether_data_contracts::repository::usage::StoredUsageAuditAggregation>,
        DataLayerError,
    > {
        match &self.usage_reader {
            Some(repository) => repository.aggregate_usage_audits(query).await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn summarize_usage_audits(
        &self,
        query: &aether_data_contracts::repository::usage::UsageAuditSummaryQuery,
    ) -> Result<aether_data_contracts::repository::usage::StoredUsageAuditSummary, DataLayerError>
    {
        match &self.usage_reader {
            Some(repository) => repository.summarize_usage_audits(query).await,
            None => {
                Ok(aether_data_contracts::repository::usage::StoredUsageAuditSummary::default())
            }
        }
    }

    pub(crate) async fn read_usage_counter_health(
        &self,
    ) -> Result<UsageCounterHealthSnapshot, DataLayerError> {
        match &self.usage_reader {
            Some(repository) => repository.read_usage_counter_health().await,
            None => Ok(UsageCounterHealthSnapshot::default()),
        }
    }

    pub(crate) async fn read_usage_counter_pending_health(
        &self,
    ) -> Result<UsageCounterPendingHealthSnapshot, DataLayerError> {
        match &self.usage_reader {
            Some(repository) => repository.read_usage_counter_pending_health().await,
            None => Ok(UsageCounterPendingHealthSnapshot::default()),
        }
    }

    pub(crate) async fn summarize_usage_totals_by_user_ids(
        &self,
        user_ids: &[String],
    ) -> Result<Vec<aether_data_contracts::repository::usage::StoredUsageUserTotals>, DataLayerError>
    {
        match &self.usage_reader {
            Some(repository) => {
                repository
                    .summarize_usage_totals_by_user_ids(user_ids)
                    .await
            }
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn summarize_usage_cache_hit_summary(
        &self,
        query: &aether_data_contracts::repository::usage::UsageCacheHitSummaryQuery,
    ) -> Result<aether_data_contracts::repository::usage::StoredUsageCacheHitSummary, DataLayerError>
    {
        match &self.usage_reader {
            Some(repository) => repository.summarize_usage_cache_hit_summary(query).await,
            None => {
                Ok(aether_data_contracts::repository::usage::StoredUsageCacheHitSummary::default())
            }
        }
    }

    pub(crate) async fn summarize_usage_settled_cost(
        &self,
        query: &aether_data_contracts::repository::usage::UsageSettledCostSummaryQuery,
    ) -> Result<
        aether_data_contracts::repository::usage::StoredUsageSettledCostSummary,
        DataLayerError,
    > {
        match &self.usage_reader {
            Some(repository) => repository.summarize_usage_settled_cost(query).await,
            None => Ok(
                aether_data_contracts::repository::usage::StoredUsageSettledCostSummary::default(),
            ),
        }
    }

    pub(crate) async fn summarize_usage_cache_affinity_hit_summary(
        &self,
        query: &aether_data_contracts::repository::usage::UsageCacheAffinityHitSummaryQuery,
    ) -> Result<
        aether_data_contracts::repository::usage::StoredUsageCacheAffinityHitSummary,
        DataLayerError,
    > {
        match &self.usage_reader {
            Some(repository) => repository
                .summarize_usage_cache_affinity_hit_summary(query)
                .await,
            None => Ok(
                aether_data_contracts::repository::usage::StoredUsageCacheAffinityHitSummary::default(),
            ),
        }
    }

    pub(crate) async fn list_usage_cache_affinity_intervals(
        &self,
        query: &aether_data_contracts::repository::usage::UsageCacheAffinityIntervalQuery,
    ) -> Result<
        Vec<aether_data_contracts::repository::usage::StoredUsageCacheAffinityIntervalRow>,
        DataLayerError,
    > {
        match &self.usage_reader {
            Some(repository) => repository.list_usage_cache_affinity_intervals(query).await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn summarize_dashboard_usage(
        &self,
        query: &aether_data_contracts::repository::usage::UsageDashboardSummaryQuery,
    ) -> Result<aether_data_contracts::repository::usage::StoredUsageDashboardSummary, DataLayerError>
    {
        match &self.usage_reader {
            Some(repository) => repository.summarize_dashboard_usage(query).await,
            None => Ok(
                aether_data_contracts::repository::usage::StoredUsageDashboardSummary::default(),
            ),
        }
    }

    pub(crate) async fn summarize_dashboard_stats(
        &self,
        query: &aether_data_contracts::repository::usage::UsageDashboardSummaryQuery,
    ) -> Result<
        aether_data_contracts::repository::usage::StoredUsageDashboardStatsSummary,
        DataLayerError,
    > {
        match &self.usage_reader {
            Some(repository) => repository.summarize_dashboard_stats(query).await,
            None => Ok(
                aether_data_contracts::repository::usage::StoredUsageDashboardStatsSummary::default(
                ),
            ),
        }
    }

    pub(crate) async fn list_dashboard_daily_breakdown(
        &self,
        query: &aether_data_contracts::repository::usage::UsageDashboardDailyBreakdownQuery,
    ) -> Result<
        Vec<aether_data_contracts::repository::usage::StoredUsageDashboardDailyBreakdownRow>,
        DataLayerError,
    > {
        match &self.usage_reader {
            Some(repository) => repository.list_dashboard_daily_breakdown(query).await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn summarize_dashboard_provider_counts(
        &self,
        query: &aether_data_contracts::repository::usage::UsageDashboardProviderCountsQuery,
    ) -> Result<
        Vec<aether_data_contracts::repository::usage::StoredUsageDashboardProviderCount>,
        DataLayerError,
    > {
        match &self.usage_reader {
            Some(repository) => repository.summarize_dashboard_provider_counts(query).await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn summarize_usage_breakdown(
        &self,
        query: &aether_data_contracts::repository::usage::UsageBreakdownSummaryQuery,
    ) -> Result<
        Vec<aether_data_contracts::repository::usage::StoredUsageBreakdownSummaryRow>,
        DataLayerError,
    > {
        match &self.usage_reader {
            Some(repository) => repository.summarize_usage_breakdown(query).await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn count_monitoring_usage_errors(
        &self,
        query: &aether_data_contracts::repository::usage::UsageMonitoringErrorCountQuery,
    ) -> Result<u64, DataLayerError> {
        match &self.usage_reader {
            Some(repository) => repository.count_monitoring_usage_errors(query).await,
            None => Ok(0),
        }
    }

    pub(crate) async fn list_monitoring_usage_errors(
        &self,
        query: &aether_data_contracts::repository::usage::UsageMonitoringErrorListQuery,
    ) -> Result<Vec<StoredRequestUsageAudit>, DataLayerError> {
        match &self.usage_reader {
            Some(repository) => repository.list_monitoring_usage_errors(query).await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn summarize_usage_error_distribution(
        &self,
        query: &aether_data_contracts::repository::usage::UsageErrorDistributionQuery,
    ) -> Result<
        Vec<aether_data_contracts::repository::usage::StoredUsageErrorDistributionRow>,
        DataLayerError,
    > {
        match &self.usage_reader {
            Some(repository) => repository.summarize_usage_error_distribution(query).await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn summarize_usage_performance_percentiles(
        &self,
        query: &aether_data_contracts::repository::usage::UsagePerformancePercentilesQuery,
    ) -> Result<
        Vec<aether_data_contracts::repository::usage::StoredUsagePerformancePercentilesRow>,
        DataLayerError,
    > {
        match &self.usage_reader {
            Some(repository) => {
                repository
                    .summarize_usage_performance_percentiles(query)
                    .await
            }
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn summarize_usage_provider_performance(
        &self,
        query: &aether_data_contracts::repository::usage::UsageProviderPerformanceQuery,
    ) -> Result<
        aether_data_contracts::repository::usage::StoredUsageProviderPerformance,
        DataLayerError,
    > {
        match &self.usage_reader {
            Some(repository) => repository.summarize_usage_provider_performance(query).await,
            None => Ok(
                aether_data_contracts::repository::usage::StoredUsageProviderPerformance::default(),
            ),
        }
    }

    pub(crate) async fn summarize_usage_cost_savings(
        &self,
        query: &aether_data_contracts::repository::usage::UsageCostSavingsSummaryQuery,
    ) -> Result<
        aether_data_contracts::repository::usage::StoredUsageCostSavingsSummary,
        DataLayerError,
    > {
        match &self.usage_reader {
            Some(repository) => repository.summarize_usage_cost_savings(query).await,
            None => Ok(
                aether_data_contracts::repository::usage::StoredUsageCostSavingsSummary::default(),
            ),
        }
    }

    pub(crate) async fn summarize_usage_time_series(
        &self,
        query: &aether_data_contracts::repository::usage::UsageTimeSeriesQuery,
    ) -> Result<
        Vec<aether_data_contracts::repository::usage::StoredUsageTimeSeriesBucket>,
        DataLayerError,
    > {
        match &self.usage_reader {
            Some(repository) => repository.summarize_usage_time_series(query).await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn summarize_usage_leaderboard(
        &self,
        query: &aether_data_contracts::repository::usage::UsageLeaderboardQuery,
    ) -> Result<
        Vec<aether_data_contracts::repository::usage::StoredUsageLeaderboardSummary>,
        DataLayerError,
    > {
        match &self.usage_reader {
            Some(repository) => repository.summarize_usage_leaderboard(query).await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn summarize_usage_daily_heatmap(
        &self,
        query: &UsageDailyHeatmapQuery,
    ) -> Result<Vec<StoredUsageDailySummary>, DataLayerError> {
        match &self.usage_reader {
            Some(repository) => repository.summarize_usage_daily_heatmap(query).await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn list_recent_usage_audits(
        &self,
        user_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<StoredRequestUsageAudit>, DataLayerError> {
        match &self.usage_reader {
            Some(repository) => repository.list_recent_usage_audits(user_id, limit).await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn summarize_usage_total_tokens_by_api_key_ids(
        &self,
        api_key_ids: &[String],
    ) -> Result<std::collections::BTreeMap<String, u64>, DataLayerError> {
        match &self.usage_reader {
            Some(repository) => {
                repository
                    .summarize_total_tokens_by_api_key_ids(api_key_ids)
                    .await
            }
            None => Ok(std::collections::BTreeMap::new()),
        }
    }

    pub(crate) async fn summarize_usage_by_provider_api_key_ids(
        &self,
        provider_api_key_ids: &[String],
    ) -> Result<
        std::collections::BTreeMap<
            String,
            aether_data_contracts::repository::usage::StoredProviderApiKeyUsageSummary,
        >,
        DataLayerError,
    > {
        match &self.usage_reader {
            Some(repository) => {
                repository
                    .summarize_usage_by_provider_api_key_ids(provider_api_key_ids)
                    .await
            }
            None => Ok(std::collections::BTreeMap::new()),
        }
    }

    pub(crate) async fn summarize_usage_by_provider_api_key_windows(
        &self,
        requests: &[ProviderApiKeyWindowUsageRequest],
    ) -> Result<Vec<StoredProviderApiKeyWindowUsageSummary>, DataLayerError> {
        match &self.usage_reader {
            Some(repository) => {
                repository
                    .summarize_usage_by_provider_api_key_windows(requests)
                    .await
            }
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn list_users_by_ids(
        &self,
        user_ids: &[String],
    ) -> Result<Vec<StoredUserSummary>, DataLayerError> {
        match &self.user_reader {
            Some(repository) => repository.list_users_by_ids(user_ids).await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn list_users_by_username_search(
        &self,
        username_search: &str,
    ) -> Result<Vec<StoredUserSummary>, DataLayerError> {
        match &self.user_reader {
            Some(repository) => {
                repository
                    .list_users_by_username_search(username_search)
                    .await
            }
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn list_export_users(
        &self,
    ) -> Result<Vec<StoredUserExportRow>, DataLayerError> {
        match &self.user_reader {
            Some(repository) => repository.list_export_users().await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn list_export_users_page(
        &self,
        query: &aether_data::repository::users::UserExportListQuery,
    ) -> Result<Vec<StoredUserExportRow>, DataLayerError> {
        match &self.user_reader {
            Some(repository) => repository.list_export_users_page(query).await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn count_export_users(
        &self,
        query: &aether_data::repository::users::UserExportListQuery,
    ) -> Result<u64, DataLayerError> {
        match &self.user_reader {
            Some(repository) => repository.count_export_users(query).await,
            None => Ok(0),
        }
    }

    pub(crate) async fn summarize_export_users(
        &self,
    ) -> Result<aether_data::repository::users::UserExportSummary, DataLayerError> {
        match &self.user_reader {
            Some(repository) => repository.summarize_export_users().await,
            None => Ok(aether_data::repository::users::UserExportSummary::default()),
        }
    }

    pub(crate) async fn find_export_user_by_id(
        &self,
        user_id: &str,
    ) -> Result<Option<StoredUserExportRow>, DataLayerError> {
        match &self.user_reader {
            Some(repository) => repository.find_export_user_by_id(user_id).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn read_user_feature_settings(
        &self,
        user_id: &str,
    ) -> Result<Option<serde_json::Value>, DataLayerError> {
        let user_id = user_id.trim();
        if user_id.is_empty() {
            return Ok(None);
        }
        if let Some(user) = self.find_export_user_by_id(user_id).await? {
            return Ok(user.feature_settings);
        }
        Ok(None)
    }

    pub(crate) async fn list_non_admin_export_users(
        &self,
    ) -> Result<Vec<StoredUserExportRow>, DataLayerError> {
        match &self.user_reader {
            Some(repository) => repository.list_non_admin_export_users().await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn list_user_auth_by_ids(
        &self,
        user_ids: &[String],
    ) -> Result<Vec<StoredUserAuthRecord>, DataLayerError> {
        match &self.user_reader {
            Some(repository) => repository.list_user_auth_by_ids(user_ids).await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn summarize_provider_usage_since(
        &self,
        provider_id: &str,
        since_unix_secs: u64,
    ) -> Result<StoredProviderUsageSummary, DataLayerError> {
        match &self.usage_reader {
            Some(repository) => {
                repository
                    .summarize_provider_usage_since(provider_id, since_unix_secs)
                    .await
            }
            None => Ok(StoredProviderUsageSummary::default()),
        }
    }

    pub(crate) fn usage_worker_queue(&self) -> Option<std::sync::Arc<dyn RuntimeQueueStore>> {
        self.usage_worker_queue.clone()
    }

    pub(crate) async fn find_billing_model_context(
        &self,
        provider_id: &str,
        provider_api_key_id: Option<&str>,
        global_model_name: &str,
    ) -> Result<Option<StoredBillingModelContext>, DataLayerError> {
        let key = BillingModelContextCacheKey::ByGlobalModelName {
            provider_id: normalize_billing_context_cache_part(provider_id),
            provider_api_key_id: normalize_optional_billing_context_cache_part(provider_api_key_id),
            global_model_name: normalize_billing_context_cache_part(global_model_name),
        };
        if let Some(value) = self.cached_billing_model_context(&key) {
            return Ok(value);
        }
        loop {
            let notified = self.billing_model_context_cache.inflight_notify.notified();
            match self.register_billing_model_context_inflight(&key) {
                BillingModelContextInflightRegistration::Bypass => {
                    let load_epoch = self
                        .billing_model_context_cache
                        .epoch
                        .load(std::sync::atomic::Ordering::Acquire);
                    return self
                        .load_billing_model_context_by_name(
                            key,
                            provider_id,
                            provider_api_key_id,
                            global_model_name,
                            load_epoch,
                        )
                        .await;
                }
                BillingModelContextInflightRegistration::Follower => {
                    if timeout(
                        Self::BILLING_MODEL_CONTEXT_CACHE_INFLIGHT_WAIT_TIMEOUT,
                        notified,
                    )
                    .await
                    .is_err()
                    {
                        self.expire_billing_model_context_inflight(&key);
                    }
                    if let Some(value) = self.cached_billing_model_context(&key) {
                        return Ok(value);
                    }
                    continue;
                }
                BillingModelContextInflightRegistration::Leader(token) => {
                    let mut guard = BillingModelContextInflightGuard::new(self, key.clone(), token);
                    let load_epoch = self
                        .billing_model_context_cache
                        .epoch
                        .load(std::sync::atomic::Ordering::Acquire);
                    let result = self
                        .load_billing_model_context_by_name(
                            key,
                            provider_id,
                            provider_api_key_id,
                            global_model_name,
                            load_epoch,
                        )
                        .await;
                    guard.finish();
                    return result;
                }
            }
        }
    }

    pub(crate) async fn find_billing_model_context_by_model_id(
        &self,
        provider_id: &str,
        provider_api_key_id: Option<&str>,
        model_id: &str,
    ) -> Result<Option<StoredBillingModelContext>, DataLayerError> {
        let key = BillingModelContextCacheKey::ByModelId {
            provider_id: normalize_billing_context_cache_part(provider_id),
            provider_api_key_id: normalize_optional_billing_context_cache_part(provider_api_key_id),
            model_id: normalize_billing_context_cache_part(model_id),
        };
        if let Some(value) = self.cached_billing_model_context(&key) {
            return Ok(value);
        }
        loop {
            let notified = self.billing_model_context_cache.inflight_notify.notified();
            match self.register_billing_model_context_inflight(&key) {
                BillingModelContextInflightRegistration::Bypass => {
                    let load_epoch = self
                        .billing_model_context_cache
                        .epoch
                        .load(std::sync::atomic::Ordering::Acquire);
                    return self
                        .load_billing_model_context_by_model_id(
                            key,
                            provider_id,
                            provider_api_key_id,
                            model_id,
                            load_epoch,
                        )
                        .await;
                }
                BillingModelContextInflightRegistration::Follower => {
                    if timeout(
                        Self::BILLING_MODEL_CONTEXT_CACHE_INFLIGHT_WAIT_TIMEOUT,
                        notified,
                    )
                    .await
                    .is_err()
                    {
                        self.expire_billing_model_context_inflight(&key);
                    }
                    if let Some(value) = self.cached_billing_model_context(&key) {
                        return Ok(value);
                    }
                    continue;
                }
                BillingModelContextInflightRegistration::Leader(token) => {
                    let mut guard = BillingModelContextInflightGuard::new(self, key.clone(), token);
                    let load_epoch = self
                        .billing_model_context_cache
                        .epoch
                        .load(std::sync::atomic::Ordering::Acquire);
                    let result = self
                        .load_billing_model_context_by_model_id(
                            key,
                            provider_id,
                            provider_api_key_id,
                            model_id,
                            load_epoch,
                        )
                        .await;
                    guard.finish();
                    return result;
                }
            }
        }
    }

    async fn load_billing_model_context_by_name(
        &self,
        key: BillingModelContextCacheKey,
        provider_id: &str,
        provider_api_key_id: Option<&str>,
        global_model_name: &str,
        load_epoch: u64,
    ) -> Result<Option<StoredBillingModelContext>, DataLayerError> {
        crate::request_diagnostics::observe_db_operation(
            "billing_model_context",
            self.database_pool_summary(),
            async {
                match &self.billing_reader {
                    Some(repository) => {
                        let value = repository
                            .find_model_context(provider_id, provider_api_key_id, global_model_name)
                            .await?;
                        self.remember_billing_model_context(key, value.clone(), load_epoch);
                        Ok(value)
                    }
                    None => {
                        self.remember_billing_model_context(key, None, load_epoch);
                        Ok(None)
                    }
                }
            },
        )
        .await
    }

    async fn load_billing_model_context_by_model_id(
        &self,
        key: BillingModelContextCacheKey,
        provider_id: &str,
        provider_api_key_id: Option<&str>,
        model_id: &str,
        load_epoch: u64,
    ) -> Result<Option<StoredBillingModelContext>, DataLayerError> {
        crate::request_diagnostics::observe_db_operation(
            "billing_model_context",
            self.database_pool_summary(),
            async {
                match &self.billing_reader {
                    Some(repository) => {
                        let value = repository
                            .find_model_context_by_model_id(
                                provider_id,
                                provider_api_key_id,
                                model_id,
                            )
                            .await?;
                        self.remember_billing_model_context(key, value.clone(), load_epoch);
                        Ok(value)
                    }
                    None => {
                        self.remember_billing_model_context(key, None, load_epoch);
                        Ok(None)
                    }
                }
            },
        )
        .await
    }

    fn register_billing_model_context_inflight(
        &self,
        key: &BillingModelContextCacheKey,
    ) -> BillingModelContextInflightRegistration {
        match self.billing_model_context_cache.inflight.lock() {
            Ok(mut inflight) => {
                if inflight.contains_key(key) {
                    return BillingModelContextInflightRegistration::Follower;
                }
                let token = self
                    .billing_model_context_cache
                    .next_inflight_token
                    .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                inflight.insert(key.clone(), token);
                BillingModelContextInflightRegistration::Leader(token)
            }
            Err(_) => BillingModelContextInflightRegistration::Bypass,
        }
    }

    fn finish_billing_model_context_inflight(&self, key: &BillingModelContextCacheKey, token: u64) {
        let mut removed = false;
        if let Ok(mut inflight) = self.billing_model_context_cache.inflight.lock() {
            if inflight.get(key).copied() == Some(token) {
                inflight.remove(key);
                removed = true;
            }
        }
        if removed {
            self.billing_model_context_cache
                .inflight_notify
                .notify_waiters();
        }
    }

    fn expire_billing_model_context_inflight(&self, key: &BillingModelContextCacheKey) {
        let mut removed = false;
        if let Ok(mut inflight) = self.billing_model_context_cache.inflight.lock() {
            removed = inflight.remove(key).is_some();
        }
        if removed {
            tracing::warn!(
                event_name = "billing_model_context_cache_inflight_expired",
                log_type = "ops",
                cache_key = ?key,
                wait_timeout_ms = Self::BILLING_MODEL_CONTEXT_CACHE_INFLIGHT_WAIT_TIMEOUT.as_millis() as u64,
                "gateway billing model context cache expired stale inflight load"
            );
            self.billing_model_context_cache
                .inflight_notify
                .notify_waiters();
        }
    }

    fn cached_billing_model_context(
        &self,
        key: &BillingModelContextCacheKey,
    ) -> Option<Option<StoredBillingModelContext>> {
        self.billing_model_context_cache
            .entries
            .read()
            .expect("billing model context cache lock")
            .get(key)
            .and_then(|(cached_at, value)| {
                (cached_at.elapsed() <= Self::BILLING_MODEL_CONTEXT_CACHE_TTL)
                    .then(|| value.clone())
            })
    }

    fn remember_billing_model_context(
        &self,
        key: BillingModelContextCacheKey,
        value: Option<StoredBillingModelContext>,
        load_epoch: u64,
    ) {
        let mut cache = self
            .billing_model_context_cache
            .entries
            .write()
            .expect("billing model context cache lock");
        if load_epoch
            != self
                .billing_model_context_cache
                .epoch
                .load(std::sync::atomic::Ordering::Acquire)
        {
            return;
        }
        cache.retain(|_, (cached_at, _)| {
            cached_at.elapsed() <= Self::BILLING_MODEL_CONTEXT_CACHE_TTL
        });
        if cache.len() >= Self::BILLING_MODEL_CONTEXT_CACHE_MAX_ENTRIES {
            if let Some(oldest_key) = cache
                .iter()
                .min_by_key(|(_, (cached_at, _))| *cached_at)
                .map(|(key, _)| key.clone())
            {
                cache.remove(&oldest_key);
            }
        }
        cache.insert(key, (Instant::now(), value));
    }

    pub(super) fn clear_billing_model_context_cache(&self) {
        self.billing_model_context_cache
            .epoch
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        self.billing_model_context_cache
            .entries
            .write()
            .expect("billing model context cache lock")
            .clear();
        let mut cleared_inflight = false;
        if let Ok(mut inflight) = self.billing_model_context_cache.inflight.lock() {
            cleared_inflight = !inflight.is_empty();
            inflight.clear();
        }
        if cleared_inflight {
            tracing::warn!(
                event_name = "billing_model_context_cache_inflight_cleared",
                log_type = "ops",
                "gateway billing model context cache cleared in-flight loads"
            );
            self.billing_model_context_cache
                .inflight_notify
                .notify_waiters();
        }
    }

    pub(crate) async fn admin_billing_enabled_default_value_exists(
        &self,
        api_format: &str,
        task_type: &str,
        dimension_name: &str,
        existing_id: Option<&str>,
    ) -> Result<Option<bool>, DataLayerError> {
        match &self.billing_reader {
            Some(repository) => {
                repository
                    .admin_billing_enabled_default_value_exists(
                        api_format,
                        task_type,
                        dimension_name,
                        existing_id,
                    )
                    .await
            }
            None => Ok(None),
        }
    }

    pub(crate) async fn create_admin_billing_rule(
        &self,
        input: &AdminBillingRuleWriteInput,
    ) -> Result<AdminBillingMutationOutcome<AdminBillingRuleRecord>, DataLayerError> {
        let result = match &self.billing_reader {
            Some(repository) => repository.create_admin_billing_rule(input).await,
            None => Ok(AdminBillingMutationOutcome::Unavailable),
        };
        if result.is_ok() {
            self.clear_billing_model_context_cache();
        }
        result
    }

    pub(crate) async fn list_admin_billing_rules(
        &self,
        task_type: Option<&str>,
        is_enabled: Option<bool>,
        page: u32,
        page_size: u32,
    ) -> Result<Option<(Vec<AdminBillingRuleRecord>, u64)>, DataLayerError> {
        match &self.billing_reader {
            Some(repository) => {
                repository
                    .list_admin_billing_rules(task_type, is_enabled, page, page_size)
                    .await
            }
            None => Ok(None),
        }
    }

    pub(crate) async fn find_admin_billing_rule(
        &self,
        rule_id: &str,
    ) -> Result<Option<AdminBillingRuleRecord>, DataLayerError> {
        match &self.billing_reader {
            Some(repository) => repository.find_admin_billing_rule(rule_id).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn update_admin_billing_rule(
        &self,
        rule_id: &str,
        input: &AdminBillingRuleWriteInput,
    ) -> Result<AdminBillingMutationOutcome<AdminBillingRuleRecord>, DataLayerError> {
        let result = match &self.billing_reader {
            Some(repository) => repository.update_admin_billing_rule(rule_id, input).await,
            None => Ok(AdminBillingMutationOutcome::Unavailable),
        };
        if result.is_ok() {
            self.clear_billing_model_context_cache();
        }
        result
    }

    pub(crate) async fn create_admin_billing_collector(
        &self,
        input: &AdminBillingCollectorWriteInput,
    ) -> Result<AdminBillingMutationOutcome<AdminBillingCollectorRecord>, DataLayerError> {
        let result = match &self.billing_reader {
            Some(repository) => repository.create_admin_billing_collector(input).await,
            None => Ok(AdminBillingMutationOutcome::Unavailable),
        };
        if result.is_ok() {
            self.clear_billing_model_context_cache();
        }
        result
    }

    pub(crate) async fn list_admin_billing_collectors(
        &self,
        api_format: Option<&str>,
        task_type: Option<&str>,
        dimension_name: Option<&str>,
        is_enabled: Option<bool>,
        page: u32,
        page_size: u32,
    ) -> Result<Option<(Vec<AdminBillingCollectorRecord>, u64)>, DataLayerError> {
        match &self.billing_reader {
            Some(repository) => {
                repository
                    .list_admin_billing_collectors(
                        api_format,
                        task_type,
                        dimension_name,
                        is_enabled,
                        page,
                        page_size,
                    )
                    .await
            }
            None => Ok(None),
        }
    }

    pub(crate) async fn find_admin_billing_collector(
        &self,
        collector_id: &str,
    ) -> Result<Option<AdminBillingCollectorRecord>, DataLayerError> {
        match &self.billing_reader {
            Some(repository) => repository.find_admin_billing_collector(collector_id).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn update_admin_billing_collector(
        &self,
        collector_id: &str,
        input: &AdminBillingCollectorWriteInput,
    ) -> Result<AdminBillingMutationOutcome<AdminBillingCollectorRecord>, DataLayerError> {
        let result = match &self.billing_reader {
            Some(repository) => {
                repository
                    .update_admin_billing_collector(collector_id, input)
                    .await
            }
            None => Ok(AdminBillingMutationOutcome::Unavailable),
        };
        if result.is_ok() {
            self.clear_billing_model_context_cache();
        }
        result
    }

    pub(crate) async fn apply_admin_billing_preset(
        &self,
        preset: &str,
        mode: &str,
        collectors: &[AdminBillingCollectorWriteInput],
    ) -> Result<AdminBillingMutationOutcome<AdminBillingPresetApplyResult>, DataLayerError> {
        let result = match &self.billing_reader {
            Some(repository) => {
                repository
                    .apply_admin_billing_preset(preset, mode, collectors)
                    .await
            }
            None => Ok(AdminBillingMutationOutcome::Unavailable),
        };
        if result.is_ok() {
            self.clear_billing_model_context_cache();
        }
        result
    }

    pub(crate) async fn find_payment_gateway_config(
        &self,
        provider: &str,
    ) -> Result<Option<PaymentGatewayConfigRecord>, DataLayerError> {
        match &self.billing_reader {
            Some(repository) => repository.find_payment_gateway_config(provider).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn upsert_payment_gateway_config(
        &self,
        input: &PaymentGatewayConfigWriteInput,
    ) -> Result<AdminBillingMutationOutcome<PaymentGatewayConfigRecord>, DataLayerError> {
        match &self.billing_reader {
            Some(repository) => repository.upsert_payment_gateway_config(input).await,
            None => Ok(AdminBillingMutationOutcome::Unavailable),
        }
    }

    pub(crate) async fn list_billing_plans(
        &self,
        include_disabled: bool,
    ) -> Result<Option<Vec<BillingPlanRecord>>, DataLayerError> {
        match &self.billing_reader {
            Some(repository) => repository.list_billing_plans(include_disabled).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn find_billing_plan(
        &self,
        plan_id: &str,
    ) -> Result<Option<BillingPlanRecord>, DataLayerError> {
        match &self.billing_reader {
            Some(repository) => repository.find_billing_plan(plan_id).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn create_billing_plan(
        &self,
        input: &BillingPlanWriteInput,
    ) -> Result<AdminBillingMutationOutcome<BillingPlanRecord>, DataLayerError> {
        match &self.billing_reader {
            Some(repository) => repository.create_billing_plan(input).await,
            None => Ok(AdminBillingMutationOutcome::Unavailable),
        }
    }

    pub(crate) async fn update_billing_plan(
        &self,
        plan_id: &str,
        input: &BillingPlanWriteInput,
    ) -> Result<AdminBillingMutationOutcome<BillingPlanRecord>, DataLayerError> {
        match &self.billing_reader {
            Some(repository) => repository.update_billing_plan(plan_id, input).await,
            None => Ok(AdminBillingMutationOutcome::Unavailable),
        }
    }

    pub(crate) async fn set_billing_plan_enabled(
        &self,
        plan_id: &str,
        enabled: bool,
    ) -> Result<AdminBillingMutationOutcome<BillingPlanRecord>, DataLayerError> {
        match &self.billing_reader {
            Some(repository) => repository.set_billing_plan_enabled(plan_id, enabled).await,
            None => Ok(AdminBillingMutationOutcome::Unavailable),
        }
    }

    pub(crate) async fn delete_billing_plan(
        &self,
        plan_id: &str,
    ) -> Result<AdminBillingMutationOutcome<()>, DataLayerError> {
        match &self.billing_reader {
            Some(repository) => repository.delete_billing_plan(plan_id).await,
            None => Ok(AdminBillingMutationOutcome::Unavailable),
        }
    }

    pub(crate) async fn list_user_plan_entitlements(
        &self,
        user_id: &str,
    ) -> Result<Option<Vec<UserPlanEntitlementRecord>>, DataLayerError> {
        match &self.billing_reader {
            Some(repository) => repository.list_user_plan_entitlements(user_id).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn find_user_daily_quota_availability(
        &self,
        user_id: &str,
    ) -> Result<Option<UserDailyQuotaAvailabilityRecord>, DataLayerError> {
        match &self.billing_reader {
            Some(repository) => repository.find_user_daily_quota_availability(user_id).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn read_request_candidate_trace(
        &self,
        request_id: &str,
        attempted_only: bool,
    ) -> Result<Option<RequestCandidateTrace>, DataLayerError> {
        read_request_candidate_trace(self, request_id, attempted_only).await
    }

    pub(crate) async fn read_decision_trace(
        &self,
        request_id: &str,
        attempted_only: bool,
    ) -> Result<Option<DecisionTrace>, DataLayerError> {
        read_decision_trace(self, request_id, attempted_only).await
    }

    pub(crate) async fn read_request_usage_audit(
        &self,
        request_id: &str,
    ) -> Result<Option<StoredRequestUsageAudit>, DataLayerError> {
        self.find_request_usage_by_request_id(request_id).await
    }

    pub(crate) async fn read_request_usage_audit_shallow(
        &self,
        request_id: &str,
    ) -> Result<Option<StoredRequestUsageAudit>, DataLayerError> {
        self.find_request_usage_by_request_id_shallow(request_id)
            .await
    }

    pub(crate) async fn read_request_audit_bundle(
        &self,
        request_id: &str,
        attempted_only: bool,
        now_unix_secs: u64,
    ) -> Result<Option<RequestAuditBundle>, DataLayerError> {
        aether_data::repository::audit::read_request_audit_bundle(
            self,
            request_id,
            attempted_only,
            now_unix_secs,
        )
        .await
    }

    #[allow(dead_code)]
    pub(crate) async fn read_provider_transport_snapshot(
        &self,
        provider_id: &str,
        endpoint_id: &str,
        key_id: &str,
    ) -> Result<Option<GatewayProviderTransportSnapshot>, DataLayerError> {
        read_provider_transport_snapshot(self, provider_id, endpoint_id, key_id).await
    }

    pub(crate) async fn read_video_task_response(
        &self,
        route_family: Option<&str>,
        request_path: &str,
    ) -> Result<Option<LocalVideoTaskReadResponse>, DataLayerError> {
        read_data_backed_video_task_response(self, route_family, request_path).await
    }

    pub(crate) async fn find_background_task_run(
        &self,
        run_id: &str,
    ) -> Result<Option<StoredBackgroundTaskRun>, DataLayerError> {
        match &self.background_task_reader {
            Some(repository) => repository.find_run(run_id).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn list_background_task_runs(
        &self,
        query: &BackgroundTaskListQuery,
    ) -> Result<StoredBackgroundTaskRunPage, DataLayerError> {
        match &self.background_task_reader {
            Some(repository) => repository.list_runs(query).await,
            None => Ok(StoredBackgroundTaskRunPage::default()),
        }
    }

    pub(crate) async fn list_background_task_events(
        &self,
        run_id: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<StoredBackgroundTaskEvent>, DataLayerError> {
        match &self.background_task_reader {
            Some(repository) => repository.list_events(run_id, offset, limit).await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn summarize_background_task_runs(
        &self,
    ) -> Result<BackgroundTaskSummary, DataLayerError> {
        match &self.background_task_reader {
            Some(repository) => repository.summarize_runs().await,
            None => Ok(BackgroundTaskSummary::default()),
        }
    }

    pub(crate) async fn upsert_background_task_run(
        &self,
        run: UpsertBackgroundTaskRun,
    ) -> Result<Option<StoredBackgroundTaskRun>, DataLayerError> {
        match &self.background_task_writer {
            Some(repository) => repository.upsert_run(run).await.map(Some),
            None => Ok(None),
        }
    }

    pub(crate) async fn request_cancel_background_task_run(
        &self,
        run_id: &str,
        updated_at_unix_secs: u64,
    ) -> Result<bool, DataLayerError> {
        match &self.background_task_writer {
            Some(repository) => {
                repository
                    .request_cancel(run_id, updated_at_unix_secs)
                    .await
            }
            None => Ok(false),
        }
    }

    pub(crate) async fn upsert_background_task_event(
        &self,
        event: UpsertBackgroundTaskEvent,
    ) -> Result<Option<StoredBackgroundTaskEvent>, DataLayerError> {
        match &self.background_task_writer {
            Some(repository) => repository.upsert_event(event).await.map(Some),
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use aether_data::repository::global_models::InMemoryGlobalModelReadRepository;
    use aether_data::repository::users::{InMemoryUserReadRepository, StoredUserExportRow};
    use aether_data_contracts::repository::billing::{
        BillingReadRepository, StoredBillingModelContext,
    };
    use aether_data_contracts::repository::global_models::{
        StoredAdminGlobalModel, StoredPublicGlobalModel, UpdateAdminGlobalModelRecord,
    };
    use async_trait::async_trait;
    use serde_json::json;
    use tokio::sync::Barrier;

    use super::GatewayDataState;

    struct SlowBillingContextRepository {
        calls: AtomicUsize,
        context: StoredBillingModelContext,
    }

    struct BlockedBillingContextRepository {
        calls: AtomicUsize,
        context: Mutex<StoredBillingModelContext>,
        first_read: Barrier,
        release_first_read: Barrier,
    }

    #[async_trait]
    impl BillingReadRepository for SlowBillingContextRepository {
        async fn find_model_context(
            &self,
            _provider_id: &str,
            _provider_api_key_id: Option<&str>,
            _global_model_name: &str,
        ) -> Result<Option<StoredBillingModelContext>, aether_data_contracts::DataLayerError>
        {
            self.calls.fetch_add(1, Ordering::AcqRel);
            tokio::time::sleep(Duration::from_millis(25)).await;
            Ok(Some(self.context.clone()))
        }
    }

    #[async_trait]
    impl BillingReadRepository for BlockedBillingContextRepository {
        async fn find_model_context(
            &self,
            _provider_id: &str,
            _provider_api_key_id: Option<&str>,
            _global_model_name: &str,
        ) -> Result<Option<StoredBillingModelContext>, aether_data_contracts::DataLayerError>
        {
            let call_index = self.calls.fetch_add(1, Ordering::AcqRel);
            let context = self
                .context
                .lock()
                .expect("mutable billing context lock")
                .clone();
            if call_index == 0 {
                self.first_read.wait().await;
                self.release_first_read.wait().await;
            }
            Ok(Some(context))
        }
    }

    fn billing_context() -> StoredBillingModelContext {
        StoredBillingModelContext::new(
            "provider-1".to_string(),
            Some("pay_as_you_go".to_string()),
            Some("key-1".to_string()),
            None,
            None,
            "global-model-1".to_string(),
            "gpt-5".to_string(),
            None,
            Some(0.02),
            Some(json!({"tiers":[{"up_to":null,"input_price_per_1m":3.0,"output_price_per_1m":15.0}]})),
            Some("model-1".to_string()),
            Some("gpt-5-upstream".to_string()),
            None,
            None,
            None,
        )
        .expect("billing context should build")
    }

    #[tokio::test]
    async fn billing_model_context_cache_coalesces_concurrent_loads() {
        let repository = Arc::new(SlowBillingContextRepository {
            calls: AtomicUsize::new(0),
            context: billing_context(),
        });
        let state = Arc::new(GatewayDataState::with_billing_reader_for_tests(
            repository.clone(),
        ));
        let mut tasks = Vec::new();

        for _ in 0..16 {
            let state = Arc::clone(&state);
            tasks.push(tokio::spawn(async move {
                state
                    .find_billing_model_context("provider-1", Some("key-1"), "gpt-5")
                    .await
                    .expect("billing context lookup should succeed")
                    .expect("billing context should exist");
            }));
        }

        for task in tasks {
            task.await.expect("lookup task should complete");
        }

        assert_eq!(repository.calls.load(Ordering::Acquire), 1);
    }

    #[tokio::test]
    async fn global_model_price_update_invalidates_inflight_billing_context_cache() {
        let mut initial_context = billing_context();
        initial_context.default_price_per_request = None;
        initial_context.default_tiered_pricing = None;
        let repository = Arc::new(BlockedBillingContextRepository {
            calls: AtomicUsize::new(0),
            context: Mutex::new(initial_context),
            first_read: Barrier::new(2),
            release_first_read: Barrier::new(2),
        });
        let mut state = GatewayDataState::with_billing_reader_for_tests(repository.clone());
        let stored_global_model = StoredAdminGlobalModel::new(
            "global-model-1".to_string(),
            "gpt-5".to_string(),
            "GPT-5".to_string(),
            true,
            None,
            None,
            None,
            None,
            1,
            1,
            0,
            Some(1_711_000_000),
            Some(1_711_000_000),
        )
        .expect("stored global model should build");
        state.global_model_writer = Some(Arc::new(
            InMemoryGlobalModelReadRepository::seed(Vec::<StoredPublicGlobalModel>::new())
                .with_admin_global_models([stored_global_model]),
        ));
        let state = Arc::new(state);
        let lookup_state = Arc::clone(&state);
        let stale_lookup = tokio::spawn(async move {
            lookup_state
                .find_billing_model_context("provider-1", Some("key-1"), "gpt-5")
                .await
        });
        repository.first_read.wait().await;

        let updated_pricing =
            json!({"tiers":[{"up_to":null,"input_price_per_1m":3.0,"output_price_per_1m":15.0}]});
        let update = UpdateAdminGlobalModelRecord::new(
            "global-model-1".to_string(),
            "GPT-5".to_string(),
            true,
            None,
            Some(updated_pricing.clone()),
            None,
            None,
        )
        .expect("global model update should build");
        repository
            .context
            .lock()
            .expect("mutable billing context lock")
            .default_tiered_pricing = Some(updated_pricing.clone());
        state
            .update_admin_global_model(&update)
            .await
            .expect("global model price update should succeed");
        repository.release_first_read.wait().await;

        let before = stale_lookup
            .await
            .expect("initial billing lookup task should complete")
            .expect("initial billing lookup should succeed")
            .expect("initial billing context should exist");
        assert_eq!(before.default_tiered_pricing, None);
        assert_eq!(repository.calls.load(Ordering::Acquire), 1);

        let after = state
            .find_billing_model_context("provider-1", Some("key-1"), "gpt-5")
            .await
            .expect("updated billing lookup should succeed")
            .expect("updated billing context should exist");
        assert_eq!(after.default_tiered_pricing, Some(updated_pricing));
        assert_eq!(repository.calls.load(Ordering::Acquire), 2);
    }

    #[tokio::test]
    async fn lists_non_admin_export_users_from_user_reader() {
        let repository = Arc::new(InMemoryUserReadRepository::seed_export_users(vec![
            StoredUserExportRow::new(
                "user-1".to_string(),
                Some("alice@example.com".to_string()),
                true,
                "alice".to_string(),
                Some("hash".to_string()),
                "user".to_string(),
                "local".to_string(),
                Some(serde_json::json!(["openai"])),
                Some(serde_json::json!(["openai:chat"])),
                Some(serde_json::json!(["gpt-4.1"])),
                Some(60),
                Some(serde_json::json!({"gpt-4.1": {"cache_1h": true}})),
                true,
            )
            .expect("user export row should build"),
        ]));
        let state = GatewayDataState::with_user_reader_for_tests(repository);

        let rows = state
            .list_non_admin_export_users()
            .await
            .expect("export users should succeed");

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].username, "alice");
        assert!(rows[0].email_verified);
        assert_eq!(rows[0].password_hash.as_deref(), Some("hash"));
        assert_eq!(rows[0].allowed_models, Some(vec!["gpt-4.1".to_string()]));
        assert_eq!(
            rows[0].model_capability_settings,
            Some(serde_json::json!({"gpt-4.1": {"cache_1h": true}}))
        );
    }
}
