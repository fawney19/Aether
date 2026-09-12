use crate::data::state::{
    ReferralRelationshipListQuery, ReferralRelationshipRecord, ReferralRewardConfig,
    ReferralRewardListQuery, ReferralRewardRecord, ReferralUserDashboard,
};
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
                status: StatusCode::BAD_REQUEST,
                message: safe_detail.to_string(),
            }
        }
        other => GatewayError::Internal(other.to_string()),
    }
}

fn config_bool(value: Option<&serde_json::Value>, default: bool) -> bool {
    match value {
        Some(serde_json::Value::Bool(value)) => *value,
        Some(serde_json::Value::String(value)) => {
            match value.trim().to_ascii_lowercase().as_str() {
                "true" | "1" | "yes" | "on" => true,
                "false" | "0" | "no" | "off" => false,
                _ => default,
            }
        }
        Some(serde_json::Value::Number(value)) => {
            value.as_i64().map(|value| value != 0).unwrap_or(default)
        }
        _ => default,
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

fn config_f64(value: Option<&serde_json::Value>, default: f64) -> f64 {
    match value {
        Some(serde_json::Value::Number(value)) => value.as_f64().unwrap_or(default),
        Some(serde_json::Value::String(value)) => value.trim().parse::<f64>().unwrap_or(default),
        _ => default,
    }
}

fn config_percent(value: Option<&serde_json::Value>) -> f64 {
    let value = config_f64(value, 0.0);
    if value.is_finite() && value > 0.0 && value <= 100.0 {
        value
    } else {
        0.0
    }
}

impl AppState {
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
        let enabled = self
            .read_system_config_json_value("referral_enabled")
            .await?;
        if !config_bool(enabled.as_ref(), false) {
            return Ok(None);
        }
        let mode = self
            .read_system_config_json_value("referral_reward_mode")
            .await?;
        let mode = config_string(mode.as_ref()).unwrap_or_else(|| "percent".to_string());
        let percent = self
            .read_system_config_json_value("referral_recharge_percent")
            .await?;
        let headcount_amount = self
            .read_system_config_json_value("referral_headcount_amount_usd")
            .await?;
        let headcount_trigger = self
            .read_system_config_json_value("referral_headcount_trigger")
            .await?;
        let headcount_trigger =
            config_string(headcount_trigger.as_ref()).unwrap_or_else(|| "registration".to_string());
        Ok(Some(ReferralRewardConfig {
            percent_enabled: matches!(mode.as_str(), "percent" | "both"),
            percent_rate: config_percent(percent.as_ref()),
            headcount_enabled: matches!(mode.as_str(), "headcount" | "both"),
            headcount_amount_usd: config_f64(headcount_amount.as_ref(), 0.0),
            headcount_trigger,
        }))
    }

    pub(crate) async fn bind_referral_invite_after_registration(
        &self,
        user_id: &str,
        email_verified: bool,
        invite_code: Option<&str>,
        source: Option<serde_json::Value>,
    ) -> Result<(), GatewayError> {
        let Some(config) = self.referral_reward_config().await? else {
            return Ok(());
        };
        let relationship = self
            .data
            .bind_referral_invite_code(user_id, invite_code, source)
            .await
            .map_err(referral_data_error)?;
        let trigger_matches = config.headcount_trigger == "registration"
            || (config.headcount_trigger == "email_verified" && email_verified);
        if relationship.is_some()
            && config.headcount_enabled
            && trigger_matches
            && config.headcount_amount_usd > 0.0
        {
            self.data
                .apply_registration_referral_reward(
                    user_id,
                    config.headcount_amount_usd,
                    &config.headcount_trigger,
                )
                .await
                .map_err(|err| GatewayError::Internal(err.to_string()))?;
        }
        Ok(())
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
        let Some(config) = self.referral_reward_config().await? else {
            return Ok(Vec::new());
        };
        self.data
            .apply_paid_order_referral_rewards(&order.id, config)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn apply_referral_rewards_for_payment_order_id(
        &self,
        order_id: &str,
    ) -> Result<Vec<ReferralRewardRecord>, GatewayError> {
        let Some(config) = self.referral_reward_config().await? else {
            return Ok(Vec::new());
        };
        self.data
            .apply_paid_order_referral_rewards(order_id, config)
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
        let config = self.referral_reward_config().await?;
        self.data
            .reconcile_referral_rewards_once(config)
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
}
