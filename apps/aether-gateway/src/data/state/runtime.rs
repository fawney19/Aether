use super::{
    read_decision_trace, read_provider_transport_snapshot, read_request_candidate_trace,
    AdjustWalletBalanceInput, AdminPaymentOrderListQuery, AdminWalletLedgerQuery,
    AdminWalletListQuery, AdminWalletRefundRequestListQuery, AnnouncementListQuery,
    CompleteAdminWalletRefundInput, CreateAnnouncementRecord, CreateManualWalletRechargeInput,
    CreateWalletRechargeOrderInput, CreateWalletRechargeOrderOutcome,
    CreateWalletRefundRequestInput, CreateWalletRefundRequestOutcome, CreditAdminPaymentOrderInput,
    DataLayerError, DecisionTrace, FailAdminWalletRefundInput, GatewayDataState,
    GatewayProviderTransportSnapshot, LocalVideoTaskReadResponse, ProcessAdminWalletRefundInput,
    ProcessPaymentCallbackInput, ProcessPaymentCallbackOutcome, RedisStreamRunner,
    RequestAuditBundle, RequestCandidateTrace, StoredAdminPaymentCallbackPage,
    StoredAdminPaymentOrder, StoredAdminPaymentOrderPage, StoredAdminWalletLedgerPage,
    StoredAdminWalletListPage, StoredAdminWalletRefund, StoredAdminWalletRefundPage,
    StoredAdminWalletRefundRequestPage, StoredAdminWalletTransaction,
    StoredAdminWalletTransactionPage, StoredAnnouncement, StoredAnnouncementPage,
    StoredBillingModelContext, StoredProviderQuotaSnapshot, StoredProviderUsageSummary,
    StoredRequestUsageAudit, StoredUsageSettlement, StoredUserAuthRecord, StoredUserExportRow,
    StoredUserSummary, StoredVideoTask, StoredWalletDailyUsageLedger,
    StoredWalletDailyUsageLedgerPage, StoredWalletSnapshot, UpdateAnnouncementRecord,
    UpsertUsageRecord, UpsertVideoTask, UsageSettlementInput, VideoTaskLookupKey,
    VideoTaskModelCount, VideoTaskQueryFilter, VideoTaskStatusCount, WalletLookupKey,
    WalletMutationOutcome,
};
use aether_data_contracts::repository::usage::{
    StoredUsageDailySummary, UsageAuditListQuery, UsageDailyHeatmapQuery,
};
use aether_video_tasks_core::read_data_backed_video_task_response;

impl GatewayDataState {
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

    #[allow(dead_code)]

    pub(crate) async fn upsert_usage(
        &self,
        usage: UpsertUsageRecord,
    ) -> Result<Option<StoredRequestUsageAudit>, DataLayerError> {
        match &self.usage_writer {
            Some(repository) => repository.upsert(usage).await.map(Some),
            None => Ok(None),
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

    pub(crate) async fn find_request_usage_by_id(
        &self,
        usage_id: &str,
    ) -> Result<Option<StoredRequestUsageAudit>, DataLayerError> {
        match &self.usage_reader {
            Some(repository) => repository.find_by_id(usage_id).await,
            None => Ok(None),
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

    pub(crate) async fn list_users_by_ids(
        &self,
        user_ids: &[String],
    ) -> Result<Vec<StoredUserSummary>, DataLayerError> {
        match &self.user_reader {
            Some(repository) => repository.list_users_by_ids(user_ids).await,
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

    pub(crate) fn usage_worker_runner(&self) -> Option<RedisStreamRunner> {
        self.usage_worker_runner.clone()
    }

    pub(crate) async fn find_billing_model_context(
        &self,
        provider_id: &str,
        provider_api_key_id: Option<&str>,
        global_model_name: &str,
    ) -> Result<Option<StoredBillingModelContext>, DataLayerError> {
        match &self.billing_reader {
            Some(repository) => {
                repository
                    .find_model_context(provider_id, provider_api_key_id, global_model_name)
                    .await
            }
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
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use aether_data::repository::users::{InMemoryUserReadRepository, StoredUserExportRow};

    use super::GatewayDataState;

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
