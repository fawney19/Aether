use aether_data::backend::ReferralDataState;
use aether_data::DataLayerError;

use super::GatewayDataState;

pub(crate) use aether_data::backend::{
    ReferralAdminOverviewStats, ReferralAdminStats, ReferralMutationStatus,
    ReferralReconciliationSummary, ReferralRefundPreview, ReferralRelationshipListQuery,
    ReferralRelationshipRecord, ReferralRewardConfig, ReferralRewardDetail,
    ReferralRewardListQuery, ReferralRewardRecord, ReferralUserDashboard,
};

impl GatewayDataState {
    pub(crate) async fn register_local_auth_user_with_referral(
        &self,
        email: Option<String>,
        email_verified: bool,
        username: String,
        password_hash: String,
        initial_gift_usd: f64,
        unlimited: bool,
        invite_code: Option<&str>,
        source: Option<serde_json::Value>,
        config: Option<ReferralRewardConfig>,
        privacy_version: Option<&str>,
        default_group_id: Option<&str>,
    ) -> Result<
        Option<(
            aether_data::repository::users::StoredUserAuthRecord,
            aether_data::repository::wallet::StoredWalletSnapshot,
            bool,
        )>,
        DataLayerError,
    > {
        self.referrals()
            .register_local_auth_user_with_referral(
                email,
                email_verified,
                username,
                password_hash,
                initial_gift_usd,
                unlimited,
                invite_code,
                source,
                config,
                privacy_version,
                default_group_id,
            )
            .await
    }
    pub(crate) async fn update_referral_settings(
        &self,
        values: serde_json::Map<String, serde_json::Value>,
    ) -> Result<(), DataLayerError> {
        self.referrals().update_referral_settings(values).await
    }

    pub(crate) async fn validate_referral_invite_code(
        &self,
        code: Option<&str>,
    ) -> Result<(), DataLayerError> {
        self.referrals().validate_referral_invite_code(code).await
    }

    pub(crate) async fn settle_paid_order_referral_rewards(
        &self,
        order_id: &str,
    ) -> Result<Vec<ReferralRewardRecord>, DataLayerError> {
        self.referrals()
            .settle_paid_order_referral_rewards(order_id)
            .await
    }

    pub(crate) async fn settle_registration_referral_rewards(
        &self,
        user_id: &str,
    ) -> Result<Vec<ReferralRewardRecord>, DataLayerError> {
        self.referrals()
            .settle_registration_referral_rewards(user_id)
            .await
    }

    pub(crate) async fn referral_refund_preview(
        &self,
        order_id: &str,
        amount: f64,
    ) -> Result<Vec<ReferralRefundPreview>, DataLayerError> {
        self.referrals()
            .referral_refund_preview(order_id, amount)
            .await
    }

    pub(crate) async fn referral_order_reversal_summary(
        &self,
        order_id: &str,
    ) -> Result<aether_data::backend::ReferralReversalSummary, DataLayerError> {
        self.referrals()
            .referral_order_reversal_summary(order_id)
            .await
    }

    fn referrals(&self) -> ReferralDataState<'_> {
        ReferralDataState::new(self.backends.as_ref())
    }

    pub(crate) fn has_referral_data_backend(&self) -> bool {
        self.referrals().has_referral_data_backend()
    }

    pub(crate) async fn record_user_privacy_policy_acceptance(
        &self,
        user_id: &str,
        version: &str,
    ) -> Result<bool, DataLayerError> {
        self.referrals()
            .record_user_privacy_policy_acceptance(user_id, version)
            .await
    }

    pub(crate) async fn referral_dashboard(
        &self,
        user_id: &str,
    ) -> Result<Option<ReferralUserDashboard>, DataLayerError> {
        self.referrals().referral_dashboard(user_id).await
    }

    pub(crate) async fn list_admin_referral_relationships(
        &self,
        query: ReferralRelationshipListQuery,
    ) -> Result<Option<(Vec<ReferralRelationshipRecord>, u64, ReferralAdminStats)>, DataLayerError>
    {
        self.referrals()
            .list_admin_referral_relationships(query)
            .await
    }

    pub(crate) async fn list_admin_referral_rewards(
        &self,
        query: ReferralRewardListQuery,
    ) -> Result<Option<(Vec<ReferralRewardRecord>, u64, ReferralAdminStats)>, DataLayerError> {
        self.referrals().list_admin_referral_rewards(query).await
    }

    pub(crate) async fn referral_admin_overview_stats(
        &self,
    ) -> Result<Option<ReferralAdminOverviewStats>, DataLayerError> {
        self.referrals().referral_admin_overview_stats().await
    }
    pub(crate) async fn referral_reward_detail(
        &self,
        reward_id: &str,
    ) -> Result<Option<ReferralRewardDetail>, DataLayerError> {
        self.referrals().referral_reward_detail(reward_id).await
    }

    pub(crate) async fn retry_referral_reward(
        &self,
        reward_id: &str,
        operator_id: Option<&str>,
        note: Option<&str>,
    ) -> Result<Option<ReferralRewardRecord>, DataLayerError> {
        self.referrals()
            .retry_referral_reward(reward_id, operator_id, note)
            .await
    }

    pub(crate) async fn void_referral_reward(
        &self,
        reward_id: &str,
        operator_id: Option<&str>,
        note: Option<&str>,
    ) -> Result<Option<ReferralRewardRecord>, DataLayerError> {
        self.referrals()
            .void_referral_reward(reward_id, operator_id, note)
            .await
    }

    pub(crate) async fn reverse_referral_rewards_for_order(
        &self,
        order_id: &str,
        amount_usd: f64,
    ) -> Result<Vec<ReferralRewardRecord>, DataLayerError> {
        self.referrals()
            .reverse_referral_rewards_for_order(order_id, amount_usd)
            .await
    }

    pub(crate) async fn reconcile_referral_rewards_once(
        &self,
        reward_config: Option<ReferralRewardConfig>,
    ) -> Result<ReferralReconciliationSummary, DataLayerError> {
        self.referrals()
            .reconcile_referral_rewards_once(reward_config)
            .await
    }
}
