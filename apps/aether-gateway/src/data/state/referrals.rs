use aether_data::backend::ReferralDataState;
use aether_data::DataLayerError;

use super::GatewayDataState;

pub(crate) use aether_data::backend::{
    ReferralAdminStats, ReferralMutationStatus, ReferralRelationshipListQuery,
    ReferralRelationshipRecord, ReferralRewardConfig, ReferralRewardListQuery,
    ReferralRewardRecord, ReferralUserDashboard,
};

impl GatewayDataState {
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

    pub(crate) async fn bind_referral_invite_code(
        &self,
        invitee_user_id: &str,
        invite_code: Option<&str>,
        source: Option<serde_json::Value>,
    ) -> Result<Option<ReferralRelationshipRecord>, DataLayerError> {
        self.referrals()
            .bind_referral_invite_code(invitee_user_id, invite_code, source)
            .await
    }

    pub(crate) async fn apply_registration_referral_reward(
        &self,
        invitee_user_id: &str,
        amount_usd: f64,
        trigger_point: &str,
    ) -> Result<Vec<ReferralRewardRecord>, DataLayerError> {
        self.referrals()
            .apply_registration_referral_reward(invitee_user_id, amount_usd, trigger_point)
            .await
    }

    pub(crate) async fn apply_paid_order_referral_rewards(
        &self,
        order_id: &str,
        config: ReferralRewardConfig,
    ) -> Result<Vec<ReferralRewardRecord>, DataLayerError> {
        self.referrals()
            .apply_paid_order_referral_rewards(order_id, config)
            .await
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
}
