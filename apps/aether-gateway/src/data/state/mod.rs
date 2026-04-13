use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;
use std::sync::RwLock;

use super::auth::GatewayAuthApiKeySnapshot;
use super::candidates::{read_request_candidate_trace, RequestCandidateTrace};
use super::config::GatewayDataConfig;
use super::decision_trace::{read_decision_trace, DecisionTrace};
use crate::provider_transport::{
    read_provider_transport_snapshot, GatewayProviderTransportSnapshot,
};
use crate::video_tasks::LocalVideoTaskReadResponse;
use aether_data::redis::{RedisKvRunner, RedisKvRunnerConfig, RedisLockRunner, RedisStreamRunner};
use aether_data::repository::announcements::{
    AnnouncementListQuery, AnnouncementReadRepository, AnnouncementWriteRepository,
    CreateAnnouncementRecord, StoredAnnouncement, StoredAnnouncementPage, UpdateAnnouncementRecord,
};
use aether_data::repository::audit::RequestAuditBundle;
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
    ProxyNodeHeartbeatMutation, ProxyNodeReadRepository, ProxyNodeRegistrationMutation,
    ProxyNodeRemoteConfigMutation, ProxyNodeTunnelStatusMutation, ProxyNodeWriteRepository,
    StoredProxyNode, StoredProxyNodeEvent,
};
use aether_data::repository::shadow_results::{
    merge_shadow_result_sample, RecordShadowResultSample, ShadowResultLookupKey,
    ShadowResultReadRepository, ShadowResultWriteRepository, StoredShadowResult,
};
pub(crate) use aether_data::repository::system::{AdminSystemStats, StoredSystemConfigEntry};
use aether_data::repository::users::{
    StoredUserAuthRecord, StoredUserExportRow, StoredUserSummary, UserReadRepository,
};
pub(crate) use aether_data::repository::users::{
    StoredUserPreferenceRecord, StoredUserSessionRecord,
};
use aether_data::repository::wallet::{
    AdjustWalletBalanceInput, AdminPaymentOrderListQuery, AdminWalletLedgerQuery,
    AdminWalletListQuery, AdminWalletRefundRequestListQuery, CompleteAdminWalletRefundInput,
    CreateManualWalletRechargeInput, CreateWalletRechargeOrderInput,
    CreateWalletRechargeOrderOutcome, CreateWalletRefundRequestInput,
    CreateWalletRefundRequestOutcome, CreditAdminPaymentOrderInput, FailAdminWalletRefundInput,
    ProcessAdminWalletRefundInput, ProcessPaymentCallbackInput, ProcessPaymentCallbackOutcome,
    StoredAdminPaymentCallback, StoredAdminPaymentCallbackPage, StoredAdminPaymentOrder,
    StoredAdminPaymentOrderPage, StoredAdminWalletLedgerPage, StoredAdminWalletListPage,
    StoredAdminWalletRefund, StoredAdminWalletRefundPage, StoredAdminWalletRefundRequestPage,
    StoredAdminWalletTransaction, StoredAdminWalletTransactionPage, StoredWalletDailyUsageLedger,
    StoredWalletDailyUsageLedgerPage, StoredWalletSnapshot, WalletLookupKey, WalletMutationOutcome,
    WalletReadRepository, WalletWriteRepository,
};
use aether_data::{DataBackends, DataLayerError};
use aether_data_contracts::repository::billing::{
    BillingReadRepository, StoredBillingModelContext,
};
use aether_data_contracts::repository::candidate_selection::{
    MinimalCandidateSelectionReadRepository, StoredMinimalCandidateSelectionRow,
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
use aether_data_contracts::repository::provider_catalog::{
    ProviderCatalogKeyListQuery, ProviderCatalogReadRepository, ProviderCatalogWriteRepository,
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogKeyPage,
    StoredProviderCatalogKeyStats, StoredProviderCatalogProvider,
};
use aether_data_contracts::repository::quota::{
    ProviderQuotaReadRepository, ProviderQuotaWriteRepository, StoredProviderQuotaSnapshot,
};
use aether_data_contracts::repository::settlement::{
    SettlementWriteRepository, StoredUsageSettlement, UsageSettlementInput,
};
use aether_data_contracts::repository::usage::{
    StoredProviderUsageSummary, StoredRequestUsageAudit, UpsertUsageRecord, UsageReadRepository,
    UsageWriteRepository,
};
use aether_data_contracts::repository::video_tasks::{
    StoredVideoTask, UpsertVideoTask, VideoTaskLookupKey, VideoTaskModelCount,
    VideoTaskQueryFilter, VideoTaskReadRepository, VideoTaskStatusCount, VideoTaskWriteRepository,
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
    gemini_file_mapping_reader: Option<Arc<dyn GeminiFileMappingReadRepository>>,
    gemini_file_mapping_writer: Option<Arc<dyn GeminiFileMappingWriteRepository>>,
    global_model_reader: Option<Arc<dyn GlobalModelReadRepository>>,
    global_model_writer: Option<Arc<dyn GlobalModelWriteRepository>>,
    minimal_candidate_selection_reader: Option<Arc<dyn MinimalCandidateSelectionReadRepository>>,
    request_candidate_reader: Option<Arc<dyn RequestCandidateReadRepository>>,
    request_candidate_writer: Option<Arc<dyn RequestCandidateWriteRepository>>,
    provider_catalog_reader: Option<Arc<dyn ProviderCatalogReadRepository>>,
    provider_catalog_writer: Option<Arc<dyn ProviderCatalogWriteRepository>>,
    provider_quota_reader: Option<Arc<dyn ProviderQuotaReadRepository>>,
    provider_quota_writer: Option<Arc<dyn ProviderQuotaWriteRepository>>,
    usage_reader: Option<Arc<dyn UsageReadRepository>>,
    usage_writer: Option<Arc<dyn UsageWriteRepository>>,
    user_reader: Option<Arc<dyn UserReadRepository>>,
    user_preferences: Option<Arc<RwLock<BTreeMap<String, StoredUserPreferenceRecord>>>>,
    usage_worker_runner: Option<RedisStreamRunner>,
    video_task_reader: Option<Arc<dyn VideoTaskReadRepository>>,
    video_task_writer: Option<Arc<dyn VideoTaskWriteRepository>>,
    wallet_reader: Option<Arc<dyn WalletReadRepository>>,
    wallet_writer: Option<Arc<dyn WalletWriteRepository>>,
    settlement_writer: Option<Arc<dyn SettlementWriteRepository>>,
    shadow_result_reader: Option<Arc<dyn ShadowResultReadRepository>>,
    shadow_result_writer: Option<Arc<dyn ShadowResultWriteRepository>>,
    system_config_values: Option<Arc<RwLock<BTreeMap<String, StoredSystemConfigEntry>>>>,
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
            .field(
                "has_provider_quota_reader",
                &self.provider_quota_reader.is_some(),
            )
            .field(
                "has_provider_quota_writer",
                &self.provider_quota_writer.is_some(),
            )
            .field("has_usage_reader", &self.usage_reader.is_some())
            .field("has_usage_writer", &self.usage_writer.is_some())
            .field("has_user_preferences", &self.user_preferences.is_some())
            .field(
                "has_usage_worker_runner",
                &self.usage_worker_runner.is_some(),
            )
            .field("has_video_task_reader", &self.video_task_reader.is_some())
            .field("has_video_task_writer", &self.video_task_writer.is_some())
            .field("has_wallet_reader", &self.wallet_reader.is_some())
            .field("has_wallet_writer", &self.wallet_writer.is_some())
            .field("has_settlement_writer", &self.settlement_writer.is_some())
            .field(
                "has_shadow_result_reader",
                &self.shadow_result_reader.is_some(),
            )
            .field(
                "has_shadow_result_writer",
                &self.shadow_result_writer.is_some(),
            )
            .field(
                "has_system_config_values",
                &self.system_config_values.is_some(),
            )
            .finish()
    }
}

mod auth;
mod catalog;
mod core;
mod integrations;
mod models;
mod runtime;
#[cfg(test)]
mod testing;
