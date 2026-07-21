use std::collections::BTreeMap;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::sync::RwLock;
use std::time::Instant;

use super::auth::GatewayAuthApiKeySnapshot;
use super::candidates::{read_request_candidate_trace, RequestCandidateTrace};
use super::config::GatewayDataConfig;
use super::decision_trace::{read_decision_trace, DecisionTrace};
use crate::provider_transport::{
    read_provider_transport_snapshot, GatewayProviderTransportSnapshot,
};
use crate::video_tasks::LocalVideoTaskReadResponse;
use aether_data::repository::announcements::{
    AnnouncementListQuery, AnnouncementReadRepository, AnnouncementWriteRepository,
    CreateAnnouncementRecord, StoredAnnouncement, StoredAnnouncementPage, UpdateAnnouncementRecord,
};
use aether_data::repository::audit::{
    AuditLogListQuery, RequestAuditBundle, StoredAdminAuditLogPage, StoredSuspiciousActivity,
    StoredUserAuditLogPage,
};
use aether_data::repository::auth::{
    AuthApiKeyLookupKey, AuthApiKeyReadRepository, AuthApiKeyWriteRepository,
    StoredAuthApiKeyExportRecord, StoredAuthApiKeySnapshot,
};
use aether_data::repository::auth_modules::{
    AuthModuleReadRepository, AuthModuleWriteRepository, StoredLdapModuleConfig,
    StoredOAuthProviderModuleConfig,
};
use aether_data::repository::gemini_file_mappings::{
    GeminiFileMappingListQuery, GeminiFileMappingReadRepository, GeminiFileMappingStats,
    GeminiFileMappingWriteRepository, StoredGeminiFileMapping, StoredGeminiFileMappingListPage,
    UpsertGeminiFileMappingRecord,
};
use aether_data::repository::management_tokens::{
    CreateManagementTokenRecord, ManagementTokenListQuery, ManagementTokenReadRepository,
    ManagementTokenWriteRepository, RegenerateManagementTokenSecret, StoredManagementToken,
    StoredManagementTokenListPage, StoredManagementTokenWithUser, UpdateManagementTokenRecord,
};
use aether_data::repository::oauth_providers::{
    OAuthProviderReadRepository, OAuthProviderWriteRepository, StoredOAuthProviderConfig,
    UpsertOAuthProviderConfigRecord,
};
use aether_data::repository::proxy_nodes::{
    ProxyNodeEventQuery, ProxyNodeHeartbeatMutation, ProxyNodeManualCreateMutation,
    ProxyNodeManualUpdateMutation, ProxyNodeMetricsCleanupSummary, ProxyNodeMetricsStep,
    ProxyNodeReadRepository, ProxyNodeRegistrationMutation, ProxyNodeRemoteConfigMutation,
    ProxyNodeTrafficMutation, ProxyNodeTunnelStatusMutation, ProxyNodeWriteRepository,
    StoredProxyFleetMetricsBucket, StoredProxyNode, StoredProxyNodeEvent,
    StoredProxyNodeMetricsBucket,
};
pub(crate) use aether_data::repository::system::{AdminSystemStats, StoredSystemConfigEntry};
use aether_data::repository::users::{
    StoredUserAuthRecord, StoredUserExportRow, StoredUserOAuthLinkSummary, StoredUserSummary,
    UserReadRepository,
};
pub(crate) use aether_data::repository::users::{
    StoredUserPreferenceRecord, StoredUserSessionRecord,
};
use aether_data::repository::wallet::{
    AdjustWalletBalanceInput, AdminPaymentOrderListQuery, AdminRedeemCodeBatchListQuery,
    AdminRedeemCodeListQuery, AdminWalletLedgerQuery, AdminWalletListQuery,
    AdminWalletRefundRequestListQuery, CompleteAdminWalletRefundInput,
    CreateAdminRedeemCodeBatchInput, CreateAdminRedeemCodeBatchResult,
    CreateManualWalletRechargeInput, CreatePlanPurchaseOrderInput, CreatePlanPurchaseOrderOutcome,
    CreateWalletRechargeOrderInput, CreateWalletRechargeOrderOutcome,
    CreateWalletRefundRequestInput, CreateWalletRefundRequestOutcome, CreditAdminPaymentOrderInput,
    DeleteAdminRedeemCodeBatchInput, DisableAdminRedeemCodeBatchInput, DisableAdminRedeemCodeInput,
    FailAdminWalletRefundInput, ProcessAdminWalletRefundInput, ProcessPaymentCallbackInput,
    ProcessPaymentCallbackOutcome, RedeemWalletCodeInput, RedeemWalletCodeOutcome,
    StoredAdminPaymentCallback, StoredAdminPaymentCallbackPage, StoredAdminPaymentOrder,
    StoredAdminPaymentOrderPage, StoredAdminRedeemCode, StoredAdminRedeemCodeBatch,
    StoredAdminRedeemCodeBatchPage, StoredAdminRedeemCodePage, StoredAdminWalletLedgerPage,
    StoredAdminWalletListPage, StoredAdminWalletRefund, StoredAdminWalletRefundPage,
    StoredAdminWalletRefundRequestPage, StoredAdminWalletTransaction,
    StoredAdminWalletTransactionPage, StoredWalletDailyUsageLedger,
    StoredWalletDailyUsageLedgerPage, StoredWalletSnapshot, WalletLookupKey, WalletMutationOutcome,
    WalletReadRepository, WalletWriteRepository,
};
use aether_data::{
    DataBackends, DataLayerError, DatabaseMaintenanceSummary, WalletDailyUsageAggregationInput,
    WalletDailyUsageAggregationResult,
};
use aether_data_contracts::repository::background_tasks::{
    BackgroundTaskListQuery, BackgroundTaskReadRepository, BackgroundTaskSummary,
    BackgroundTaskWriteRepository, StoredBackgroundTaskEvent, StoredBackgroundTaskRun,
    StoredBackgroundTaskRunPage, UpsertBackgroundTaskEvent, UpsertBackgroundTaskRun,
};
use aether_data_contracts::repository::billing::{
    AdminBillingCollectorRecord, AdminBillingCollectorWriteInput, AdminBillingMutationOutcome,
    AdminBillingPresetApplyResult, AdminBillingRuleRecord, AdminBillingRuleWriteInput,
    BillingPlanRecord, BillingPlanWriteInput, BillingReadRepository, PaymentGatewayConfigRecord,
    PaymentGatewayConfigWriteInput, StoredBillingModelContext, UserDailyQuotaAvailabilityRecord,
    UserPlanEntitlementRecord,
};
use aether_data_contracts::repository::candidate_selection::{
    MinimalCandidateSelectionReadRepository, StoredMinimalCandidateSelectionRow,
    StoredPoolKeyCandidateRowsByKeyIdsQuery, StoredPoolKeyCandidateRowsQuery,
    StoredRequestedModelCandidateRowsQuery,
};
use aether_data_contracts::repository::candidates::{
    PublicHealthStatusCount, PublicHealthTimelineBucket, RequestCandidateReadRepository,
    RequestCandidateWriteRepository, StoredRequestCandidate, UpsertRequestCandidateRecord,
};
use aether_data_contracts::repository::global_models::{
    AdminGlobalModelListQuery, AdminProviderModelListQuery, CreateAdminGlobalModelRecord,
    GlobalModelReadRepository, GlobalModelWriteRepository, PublicCatalogModelListQuery,
    PublicCatalogModelSearchQuery, PublicGlobalModelQuery, StoredAdminGlobalModel,
    StoredAdminGlobalModelPage, StoredAdminProviderModel, StoredProviderActiveGlobalModel,
    StoredProviderModelStats, StoredPublicCatalogModel, StoredPublicGlobalModel,
    StoredPublicGlobalModelPage, UpdateAdminGlobalModelRecord, UpsertAdminProviderModelRecord,
};
use aether_data_contracts::repository::pool_scores::{
    GetPoolMemberScoresByIdsQuery, ListPoolMemberProbeCandidatesQuery, ListPoolMemberScoresQuery,
    ListRankedPoolMembersQuery, PoolMemberHardState, PoolMemberIdentity, PoolMemberProbeAttempt,
    PoolMemberProbeResult, PoolMemberProbeStatus, PoolMemberScheduleFeedback,
    PoolMemberScoreWriteRepository, PoolScoreReadRepository, PoolScoreScope, StoredPoolMemberScore,
    UpsertPoolMemberScore,
};
use aether_data_contracts::repository::provider_catalog::{
    ProviderCatalogKeyListQuery, ProviderCatalogReadRepository, ProviderCatalogWriteRepository,
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey,
    StoredProviderCatalogKeyMaintenanceSummary, StoredProviderCatalogKeyPage,
    StoredProviderCatalogKeyStats, StoredProviderCatalogProvider,
};
use aether_data_contracts::repository::quota::{
    ProviderQuotaReadRepository, ProviderQuotaWriteRepository, StoredProviderQuotaSnapshot,
};
use aether_data_contracts::repository::routing_profiles::{
    RoutingGroupReadRepository, RoutingGroupWriteRepository,
};
use aether_data_contracts::repository::settlement::{
    SettlementWriteRepository, StoredUsageSettlement, UsageSettlementInput,
};
use aether_data_contracts::repository::usage::{
    ApiKeyLastUsedDelta, ManagementTokenCounterDelta, PendingUsageCleanupSummary,
    ProxyNodeCounterDelta, StoredProviderUsageSummary, StoredRequestUsageAudit, UpsertUsageRecord,
    UsageReadRepository, UsageWriteRepository,
};
use aether_data_contracts::repository::video_tasks::{
    StoredVideoTask, UpsertVideoTask, VideoTaskLookupKey, VideoTaskModelCount,
    VideoTaskQueryFilter, VideoTaskReadRepository, VideoTaskStatusCount, VideoTaskWriteRepository,
};
use aether_runtime_state::RuntimeQueueStore;

pub(crate) use self::referrals::{
    ReferralAdminStats, ReferralMutationStatus, ReferralRelationshipListQuery,
    ReferralRelationshipRecord, ReferralRewardConfig, ReferralRewardListQuery,
    ReferralRewardRecord, ReferralUserDashboard,
};

#[derive(Clone, Default)]
pub(crate) struct GatewayDataState {
    config: GatewayDataConfig,
    backends: Option<DataBackends>,
    auth_api_key_reader: Option<Arc<dyn AuthApiKeyReadRepository>>,
    auth_api_key_writer: Option<Arc<dyn AuthApiKeyWriteRepository>>,
    auth_module_reader: Option<Arc<dyn AuthModuleReadRepository>>,
    auth_module_writer: Option<Arc<dyn AuthModuleWriteRepository>>,
    announcement_reader: Option<Arc<dyn AnnouncementReadRepository>>,
    announcement_writer: Option<Arc<dyn AnnouncementWriteRepository>>,
    management_token_reader: Option<Arc<dyn ManagementTokenReadRepository>>,
    management_token_writer: Option<Arc<dyn ManagementTokenWriteRepository>>,
    oauth_provider_reader: Option<Arc<dyn OAuthProviderReadRepository>>,
    oauth_provider_writer: Option<Arc<dyn OAuthProviderWriteRepository>>,
    proxy_node_reader: Option<Arc<dyn ProxyNodeReadRepository>>,
    proxy_node_writer: Option<Arc<dyn ProxyNodeWriteRepository>>,
    billing_reader: Option<Arc<dyn BillingReadRepository>>,
    background_task_reader: Option<Arc<dyn BackgroundTaskReadRepository>>,
    background_task_writer: Option<Arc<dyn BackgroundTaskWriteRepository>>,
    gemini_file_mapping_reader: Option<Arc<dyn GeminiFileMappingReadRepository>>,
    gemini_file_mapping_writer: Option<Arc<dyn GeminiFileMappingWriteRepository>>,
    global_model_reader: Option<Arc<dyn GlobalModelReadRepository>>,
    global_model_writer: Option<Arc<dyn GlobalModelWriteRepository>>,
    minimal_candidate_selection_reader: Option<Arc<dyn MinimalCandidateSelectionReadRepository>>,
    request_candidate_reader: Option<Arc<dyn RequestCandidateReadRepository>>,
    request_candidate_writer: Option<Arc<dyn RequestCandidateWriteRepository>>,
    provider_catalog_reader: Option<Arc<dyn ProviderCatalogReadRepository>>,
    provider_catalog_writer: Option<Arc<dyn ProviderCatalogWriteRepository>>,
    pool_score_reader: Option<Arc<dyn PoolScoreReadRepository>>,
    pool_score_writer: Option<Arc<dyn PoolMemberScoreWriteRepository>>,
    provider_quota_reader: Option<Arc<dyn ProviderQuotaReadRepository>>,
    provider_quota_writer: Option<Arc<dyn ProviderQuotaWriteRepository>>,
    routing_group_reader: Option<Arc<dyn RoutingGroupReadRepository>>,
    routing_group_writer: Option<Arc<dyn RoutingGroupWriteRepository>>,
    usage_reader: Option<Arc<dyn UsageReadRepository>>,
    usage_writer: Option<Arc<dyn UsageWriteRepository>>,
    user_reader: Option<Arc<dyn UserReadRepository>>,
    user_preferences: Option<Arc<RwLock<BTreeMap<String, StoredUserPreferenceRecord>>>>,
    usage_worker_queue: Option<Arc<dyn RuntimeQueueStore>>,
    video_task_reader: Option<Arc<dyn VideoTaskReadRepository>>,
    video_task_writer: Option<Arc<dyn VideoTaskWriteRepository>>,
    wallet_reader: Option<Arc<dyn WalletReadRepository>>,
    wallet_writer: Option<Arc<dyn WalletWriteRepository>>,
    settlement_writer: Option<Arc<dyn SettlementWriteRepository>>,
    system_config_values: Option<Arc<RwLock<BTreeMap<String, StoredSystemConfigEntry>>>>,
    system_config_value_cache: Arc<RwLock<BTreeMap<String, (Instant, Option<serde_json::Value>)>>>,
    billing_model_context_cache: Arc<BillingModelContextCacheState>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(super) enum BillingModelContextCacheKey {
    ByModelId {
        provider_id: String,
        provider_api_key_id: Option<String>,
        model_id: String,
    },
    ByGlobalModelName {
        provider_id: String,
        provider_api_key_id: Option<String>,
        global_model_name: String,
    },
}

#[derive(Default)]
pub(super) struct BillingModelContextCacheState {
    pub(super) entries:
        RwLock<HashMap<BillingModelContextCacheKey, (Instant, Option<StoredBillingModelContext>)>>,
    pub(super) inflight: std::sync::Mutex<HashMap<BillingModelContextCacheKey, u64>>,
    pub(super) inflight_notify: tokio::sync::Notify,
    pub(super) next_inflight_token: std::sync::atomic::AtomicU64,
    pub(super) epoch: std::sync::atomic::AtomicU64,
}

impl fmt::Debug for GatewayDataState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GatewayDataState")
            .field("config", &self.config)
            .field("has_backends", &self.backends.is_some())
            .field(
                "has_auth_api_key_reader",
                &self.auth_api_key_reader.is_some(),
            )
            .field(
                "has_auth_api_key_writer",
                &self.auth_api_key_writer.is_some(),
            )
            .field("has_auth_module_reader", &self.auth_module_reader.is_some())
            .field("has_auth_module_writer", &self.auth_module_writer.is_some())
            .field(
                "has_announcement_reader",
                &self.announcement_reader.is_some(),
            )
            .field(
                "has_announcement_writer",
                &self.announcement_writer.is_some(),
            )
            .field(
                "has_management_token_reader",
                &self.management_token_reader.is_some(),
            )
            .field(
                "has_management_token_writer",
                &self.management_token_writer.is_some(),
            )
            .field(
                "has_oauth_provider_reader",
                &self.oauth_provider_reader.is_some(),
            )
            .field(
                "has_oauth_provider_writer",
                &self.oauth_provider_writer.is_some(),
            )
            .field("has_proxy_node_reader", &self.proxy_node_reader.is_some())
            .field("has_proxy_node_writer", &self.proxy_node_writer.is_some())
            .field("has_billing_reader", &self.billing_reader.is_some())
            .field(
                "has_background_task_reader",
                &self.background_task_reader.is_some(),
            )
            .field(
                "has_background_task_writer",
                &self.background_task_writer.is_some(),
            )
            .field(
                "has_gemini_file_mapping_reader",
                &self.gemini_file_mapping_reader.is_some(),
            )
            .field(
                "has_gemini_file_mapping_writer",
                &self.gemini_file_mapping_writer.is_some(),
            )
            .field(
                "has_global_model_reader",
                &self.global_model_reader.is_some(),
            )
            .field(
                "has_global_model_writer",
                &self.global_model_writer.is_some(),
            )
            .field(
                "has_minimal_candidate_selection_reader",
                &self.minimal_candidate_selection_reader.is_some(),
            )
            .field(
                "has_request_candidate_reader",
                &self.request_candidate_reader.is_some(),
            )
            .field(
                "has_request_candidate_writer",
                &self.request_candidate_writer.is_some(),
            )
            .field(
                "has_provider_catalog_reader",
                &self.provider_catalog_reader.is_some(),
            )
            .field(
                "has_provider_catalog_writer",
                &self.provider_catalog_writer.is_some(),
            )
            .field("has_pool_score_reader", &self.pool_score_reader.is_some())
            .field("has_pool_score_writer", &self.pool_score_writer.is_some())
            .field(
                "has_provider_quota_reader",
                &self.provider_quota_reader.is_some(),
            )
            .field(
                "has_provider_quota_writer",
                &self.provider_quota_writer.is_some(),
            )
            .field(
                "has_routing_group_reader",
                &self.routing_group_reader.is_some(),
            )
            .field(
                "has_routing_group_writer",
                &self.routing_group_writer.is_some(),
            )
            .field("has_usage_reader", &self.usage_reader.is_some())
            .field("has_usage_writer", &self.usage_writer.is_some())
            .field("has_user_preferences", &self.user_preferences.is_some())
            .field("has_usage_worker_queue", &self.usage_worker_queue.is_some())
            .field("has_video_task_reader", &self.video_task_reader.is_some())
            .field("has_video_task_writer", &self.video_task_writer.is_some())
            .field("has_wallet_reader", &self.wallet_reader.is_some())
            .field("has_wallet_writer", &self.wallet_writer.is_some())
            .field("has_settlement_writer", &self.settlement_writer.is_some())
            .field(
                "has_system_config_values",
                &self.system_config_values.is_some(),
            )
            .finish()
    }
}

mod auth;
mod auth_api_key_cache;
mod candidate_cache;
mod catalog;
mod core;
mod integrations;
mod models;
mod pool_scores;
mod provider_catalog_cache;
mod referrals;
mod request_candidate_cache;
mod routing_group_cache;
mod routing_profiles;
mod runtime;
#[cfg(test)]
mod testing;
#[cfg(feature = "testkit")]
pub(crate) mod testkit;
