use crate::data::state::{
    ReferralRelationshipListQuery, ReferralRelationshipRecord, ReferralRewardConfig,
    ReferralRewardListQuery, ReferralRewardRecord, ReferralUserDashboard,
};
use crate::handlers::shared::{system_config_bool, REFERRAL_ENABLED_CONFIG_KEY};
use crate::{AppState, GatewayError};
use axum::http::StatusCode;
use tracing::warn;

const REFERRAL_INVALID_INPUT_FALLBACK: &str = "返利请求无效";

fn safe_referral_invalid_input_detail(detail: &str) -> &'static str {
    // These messages are deliberate domain-level validation responses. Any
    // future adapter/storage detail must stay server-side instead of becoming
    // an oracle for database state or schema information.
    match detail {
        "邀请码无效" => "邀请码无效",
        "不能使用自己的邀请码注册" => "不能使用自己的邀请码注册",
        "仅失败返利可以补发" => "仅失败返利可以补发",
        "返利金额无效，无法补发" => "返利金额无效，无法补发",
        "仅待发或失败返利可以作废" => "仅待发或失败返利可以作废",
        "返利状态已变化，请刷新后重试" => "返利状态已变化，请刷新后重试",
        "邮箱验证返利必须启用注册邮箱验证" | "邮箱验证返利必须同时启用注册邮箱验证" => {
            "邮箱验证返利必须同时启用注册邮箱验证"
        }
        _ => REFERRAL_INVALID_INPUT_FALLBACK,
    }
}

fn referral_data_error(err: aether_data::DataLayerError) -> GatewayError {
    match err {
        aether_data::DataLayerError::InvalidInput(detail) => {
            let safe_detail = safe_referral_invalid_input_detail(&detail);
            if safe_detail == REFERRAL_INVALID_INPUT_FALLBACK {
                warn!(
                    event_name = "referral_invalid_input_hidden",
                    error_length = detail.len(),
                    "referral data-layer validation detail hidden from client"
                );
            }
            GatewayError::Client {
                status: if safe_detail == "返利状态已变化，请刷新后重试" {
                    StatusCode::CONFLICT
                } else {
                    StatusCode::BAD_REQUEST
                },
                message: safe_detail.to_string(),
            }
        }
        other => GatewayError::Internal(other.to_string()),
    }
}

fn config_string(value: Option<&serde_json::Value>) -> Option<String> {
    match value {
        Some(serde_json::Value::String(value)) => {
            let value = value.trim();
            (!value.is_empty()).then_some(value.to_string())
        }
        Some(value) => Some(value.to_string()),
        None => None,
    }
}

impl AppState {
    pub(crate) fn is_referral_settings_key(key: &str) -> bool {
        matches!(
            key,
            "referral_enabled"
                | "referral_reward_mode"
                | "referral_recharge_percent"
                | "referral_headcount_amount_usd"
                | "referral_headcount_trigger"
                | "require_email_verification"
        )
    }

    pub(crate) async fn register_local_auth_user_with_referral(
        &self,
        email: Option<String>,
        email_verified: bool,
        username: String,
        password_hash: String,
        initial_gift_usd: f64,
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
        GatewayError,
    > {
        self.data
            .register_local_auth_user_with_referral(
                email,
                email_verified,
                username,
                password_hash,
                initial_gift_usd,
                false,
                invite_code,
                source,
                config,
                privacy_version,
                default_group_id,
            )
            .await
            .map_err(referral_data_error)
    }

    pub(crate) async fn settle_registration_referral_reward(
        &self,
        user_id: &str,
    ) -> Result<(), GatewayError> {
        self.data
            .settle_registration_referral_rewards(user_id)
            .await
            .map_err(|error| GatewayError::Internal(error.to_string()))?;
        Ok(())
    }
    pub(crate) async fn validate_referral_invite_code(
        &self,
        code: Option<&str>,
    ) -> Result<(), GatewayError> {
        self.data
            .validate_referral_invite_code(code)
            .await
            .map_err(referral_data_error)
    }

    pub(crate) async fn validate_referral_settings_overlay(
        &self,
        values: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<(), GatewayError> {
        let read = |key: &'static str| async {
            if let Some(value) = values.get(key) {
                Ok::<_, GatewayError>(Some(value.clone()))
            } else {
                self.read_system_config_json_value_strong(key).await
            }
        };
        let mode = read("referral_reward_mode").await?;
        let enabled = read("referral_enabled").await?;
        let trigger = read("referral_headcount_trigger").await?;
        let require = read("require_email_verification").await?;
        if system_config_bool(enabled.as_ref(), false)
            && matches!(
                config_string(mode.as_ref()).as_deref(),
                Some("headcount" | "both")
            )
            && config_string(trigger.as_ref()).as_deref() == Some("email_verified")
            && !system_config_bool(require.as_ref(), false)
        {
            return Err(GatewayError::Client {
                status: StatusCode::BAD_REQUEST,
                message: "邮箱验证返利必须同时启用注册邮箱验证".to_string(),
            });
        }
        Ok(())
    }

    pub(crate) async fn update_referral_settings(
        &self,
        values: serde_json::Map<String, serde_json::Value>,
    ) -> Result<(), GatewayError> {
        #[cfg(test)]
        if !self.has_referral_data_backend() {
            self.validate_referral_settings_overlay(&values).await?;
            for (key, value) in values {
                self.upsert_system_config_json_value(&key, &value, None)
                    .await?;
            }
            return Ok(());
        }
        self.data
            .update_referral_settings(values.clone())
            .await
            .map_err(referral_data_error)?;
        for (key, value) in values {
            self.remember_system_config_write(&key, Some(value));
        }
        Ok(())
    }

    pub(crate) fn has_referral_data_backend(&self) -> bool {
        self.data.has_referral_data_backend()
    }

    pub(crate) async fn record_user_privacy_policy_acceptance(
        &self,
        user_id: &str,
        version: &str,
    ) -> Result<bool, GatewayError> {
        self.data
            .record_user_privacy_policy_acceptance(user_id, version)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn referral_reward_config(
        &self,
    ) -> Result<Option<ReferralRewardConfig>, GatewayError> {
        let mut values = serde_json::Map::new();
        for key in [
            REFERRAL_ENABLED_CONFIG_KEY,
            "referral_reward_mode",
            "referral_recharge_percent",
            "referral_headcount_amount_usd",
            "referral_headcount_trigger",
        ] {
            if let Some(value) = self.read_system_config_json_value(key).await? {
                values.insert(key.to_string(), value);
            }
        }
        Ok(aether_data_contracts::repository::referrals::parse_referral_reward_config(&values))
    }

    pub(crate) async fn referral_dashboard(
        &self,
        user_id: &str,
    ) -> Result<Option<ReferralUserDashboard>, GatewayError> {
        self.data
            .referral_dashboard(user_id)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn list_admin_referral_relationships(
        &self,
        query: ReferralRelationshipListQuery,
    ) -> Result<
        Option<(
            Vec<ReferralRelationshipRecord>,
            u64,
            crate::data::state::ReferralAdminStats,
        )>,
        GatewayError,
    > {
        self.data
            .list_admin_referral_relationships(query)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn list_admin_referral_rewards(
        &self,
        query: ReferralRewardListQuery,
    ) -> Result<
        Option<(
            Vec<ReferralRewardRecord>,
            u64,
            crate::data::state::ReferralAdminStats,
        )>,
        GatewayError,
    > {
        self.data
            .list_admin_referral_rewards(query)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn referral_admin_overview(
        &self,
    ) -> Result<Option<serde_json::Value>, GatewayError> {
        let Some(stats) = self
            .data
            .referral_admin_overview_stats()
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?
        else {
            return Ok(None);
        };
        let mut values = serde_json::Map::new();
        for key in [
            "referral_enabled",
            "referral_reward_mode",
            "referral_recharge_percent",
            "referral_headcount_amount_usd",
            "referral_headcount_trigger",
        ] {
            if let Some(value) = self.read_system_config_json_value(key).await? {
                values.insert(key.to_owned(), value);
            }
        }
        use aether_data_contracts::repository::referrals::{
            referral_config_bool, referral_config_number, referral_config_string,
        };
        Ok(Some(serde_json::json!({"stats": stats,"rules":{
            "available":crate::handlers::shared::module_available_from_env("REFERRAL_AVAILABLE",true),
            "enabled":referral_config_bool(values.get("referral_enabled"),false),
            "reward_mode":referral_config_string(values.get("referral_reward_mode")).unwrap_or_else(||"percent".into()),
            "recharge_percent":referral_config_number(values.get("referral_recharge_percent")),
            "headcount_amount_usd":referral_config_number(values.get("referral_headcount_amount_usd")),
            "headcount_trigger":referral_config_string(values.get("referral_headcount_trigger")).unwrap_or_else(||"registration".into())
        }})))
    }
    pub(crate) async fn referral_reward_detail(
        &self,
        reward_id: &str,
    ) -> Result<Option<crate::data::state::ReferralRewardDetail>, GatewayError> {
        self.data
            .referral_reward_detail(reward_id)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn retry_referral_reward(
        &self,
        reward_id: &str,
        operator_id: Option<&str>,
        note: Option<&str>,
    ) -> Result<Option<ReferralRewardRecord>, GatewayError> {
        self.data
            .retry_referral_reward(reward_id, operator_id, note)
            .await
            .map_err(referral_data_error)
    }

    pub(crate) async fn void_referral_reward(
        &self,
        reward_id: &str,
        operator_id: Option<&str>,
        note: Option<&str>,
    ) -> Result<Option<ReferralRewardRecord>, GatewayError> {
        self.data
            .void_referral_reward(reward_id, operator_id, note)
            .await
            .map_err(referral_data_error)
    }

    pub(crate) async fn apply_referral_rewards_for_paid_order(
        &self,
        order: &aether_data::repository::wallet::StoredAdminPaymentOrder,
    ) -> Result<Vec<ReferralRewardRecord>, GatewayError> {
        self.data
            .settle_paid_order_referral_rewards(&order.id)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn apply_referral_rewards_for_payment_order_id(
        &self,
        order_id: &str,
    ) -> Result<Vec<ReferralRewardRecord>, GatewayError> {
        self.data
            .settle_paid_order_referral_rewards(order_id)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn reverse_referral_rewards_for_order(
        &self,
        order_id: &str,
        amount_usd: f64,
    ) -> Result<Vec<ReferralRewardRecord>, GatewayError> {
        self.data
            .reverse_referral_rewards_for_order(order_id, amount_usd)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn reconcile_referral_rewards_once(
        &self,
    ) -> Result<crate::data::state::ReferralReconciliationSummary, GatewayError> {
        self.data
            .reconcile_referral_rewards_once(None)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::{referral_data_error, REFERRAL_INVALID_INPUT_FALLBACK};

    #[test]
    fn referral_invalid_input_projection_allowlists_domain_messages() {
        let known = super::referral_data_error(aether_data::DataLayerError::InvalidInput(
            "邀请码无效".to_string(),
        ));
        match known {
            crate::GatewayError::Client { message, .. } => assert_eq!(message, "邀请码无效"),
            other => panic!("expected client error, got {other:?}"),
        }

        let secret = "database table referral_rewards row reward-secret has invalid wallet";
        let unknown = referral_data_error(aether_data::DataLayerError::InvalidInput(
            secret.to_string(),
        ));
        match unknown {
            crate::GatewayError::Client { message, .. } => {
                assert_eq!(message, REFERRAL_INVALID_INPUT_FALLBACK);
                assert!(!message.contains("reward-secret"));
                assert!(!message.contains("referral_rewards"));
            }
            other => panic!("expected client error, got {other:?}"),
        }
    }

    #[test]
    fn referral_void_race_is_a_safe_conflict() {
        let error = referral_data_error(aether_data::DataLayerError::InvalidInput(
            "返利状态已变化，请刷新后重试".to_string(),
        ));
        match error {
            crate::GatewayError::Client { status, message } => {
                assert_eq!(status, axum::http::StatusCode::CONFLICT);
                assert_eq!(message, "返利状态已变化，请刷新后重试");
            }
            other => panic!("expected client error, got {other:?}"),
        }
    }
}
