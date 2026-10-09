//! Durable referral rules captured at the event that creates an obligation.
use serde::{Deserialize, Serialize};

pub const REFERRAL_SNAPSHOT_KEY: &str = "_aether_referral_v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReferralRewardConfig {
    pub percent_enabled: bool,
    pub percent_rate: f64,
    pub headcount_enabled: bool,
    pub headcount_amount_usd: f64,
    pub headcount_trigger: String,
}

/// Keep legacy JSON config coercions identical to the gateway's public switch.
pub fn referral_config_bool(value: Option<&serde_json::Value>, default: bool) -> bool {
    match value {
        Some(serde_json::Value::Bool(v)) => *v,
        Some(serde_json::Value::Number(v)) => v.as_i64().map(|v| v != 0).unwrap_or(default),
        Some(serde_json::Value::String(v)) => match v.trim().to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" | "on" => true,
            "false" | "0" | "no" | "off" => false,
            _ => default,
        },
        _ => default,
    }
}
pub fn referral_config_string(value: Option<&serde_json::Value>) -> Option<String> {
    match value {
        Some(serde_json::Value::String(v)) => {
            let v = v.trim();
            (!v.is_empty()).then(|| v.to_string())
        }
        Some(v) => Some(v.to_string()),
        None => None,
    }
}
pub fn referral_config_number(value: Option<&serde_json::Value>) -> f64 {
    value
        .and_then(|v| {
            v.as_f64()
                .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
        })
        .filter(|v| v.is_finite())
        .unwrap_or(0.0)
}
pub fn parse_referral_reward_config(
    values: &serde_json::Map<String, serde_json::Value>,
) -> Option<ReferralRewardConfig> {
    if !referral_config_bool(values.get("referral_enabled"), false) {
        return None;
    }
    let mode = referral_config_string(values.get("referral_reward_mode"))
        .unwrap_or_else(|| "percent".into());
    let rate = referral_config_number(values.get("referral_recharge_percent"));
    let rate = if rate > 0.0 && rate <= 100.0 {
        rate
    } else {
        0.0
    };
    Some(ReferralRewardConfig {
        percent_enabled: matches!(mode.as_str(), "percent" | "both"),
        percent_rate: rate,
        headcount_enabled: matches!(mode.as_str(), "headcount" | "both"),
        headcount_amount_usd: referral_config_number(values.get("referral_headcount_amount_usd")),
        headcount_trigger: referral_config_string(values.get("referral_headcount_trigger"))
            .unwrap_or_else(|| "registration".into()),
    })
}

pub fn is_real_referral_payment(method: &str, kind: &str, amount: f64) -> bool {
    amount.is_finite()
        && amount > 0.0
        && matches!(kind, "wallet_recharge" | "plan_purchase")
        && !matches!(
            method.trim().to_ascii_lowercase().as_str(),
            "manual"
                | "admin_manual"
                | "redeem_code"
                | "gift"
                | "card_code"
                | "gift_code"
                | "card_recharge"
        )
}

/// Provider fields cannot choose or replace the internal accounting snapshot.
pub fn preserve_referral_snapshot(
    existing: Option<&serde_json::Value>,
    mut incoming: serde_json::Value,
) -> serde_json::Value {
    if !incoming.is_object() {
        incoming = serde_json::json!({"provider_response": incoming});
    }
    let map = incoming.as_object_mut().expect("object constructed above");
    map.remove(REFERRAL_SNAPSHOT_KEY);
    if let Some(snapshot) = existing.and_then(|v| v.get(REFERRAL_SNAPSHOT_KEY)) {
        map.insert(REFERRAL_SNAPSHOT_KEY.to_string(), snapshot.clone());
    }
    incoming
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn provider_cannot_inject_or_replace_accounting_snapshot() {
        let incoming =
            serde_json::json!({"provider_id":"p", REFERRAL_SNAPSHOT_KEY:{"enabled":true}});
        assert!(preserve_referral_snapshot(None, incoming.clone())
            .get(REFERRAL_SNAPSHOT_KEY)
            .is_none());
        let old = serde_json::json!({REFERRAL_SNAPSHOT_KEY:{"enabled":false}});
        let merged = preserve_referral_snapshot(Some(&old), incoming);
        assert_eq!(merged[REFERRAL_SNAPSHOT_KEY], old[REFERRAL_SNAPSHOT_KEY]);
        assert_eq!(merged["provider_id"], "p");
    }
    #[test]
    fn real_payments_include_plans_but_exclude_grants() {
        assert!(is_real_referral_payment("stripe", "plan_purchase", 1.0));
        assert!(is_real_referral_payment(
            "manual_review",
            "wallet_recharge",
            1.0
        ));
        for method in [
            "manual",
            " ADMIN_MANUAL ",
            "gift",
            "redeem_code",
            "card_code",
            "gift_code",
            "card_recharge",
        ] {
            assert!(!is_real_referral_payment(method, "wallet_recharge", 1.0));
        }
        assert!(!is_real_referral_payment(
            "stripe",
            "wallet_recharge",
            f64::NAN
        ));
    }
    #[test]
    fn legacy_settings_match_public_switch_and_trim_rule_values() {
        let mut values=serde_json::json!({"referral_enabled":" On ","referral_reward_mode":" both ","referral_recharge_percent":" 2.5 ","referral_headcount_trigger":" first_paid_order ","referral_headcount_amount_usd":" 3.0 "}).as_object().unwrap().clone();
        let config = parse_referral_reward_config(&values).unwrap();
        assert!(config.percent_enabled && config.headcount_enabled);
        assert_eq!(config.percent_rate, 2.5);
        assert_eq!(config.headcount_amount_usd, 3.0);
        assert_eq!(config.headcount_trigger, "first_paid_order");
        values.insert("referral_enabled".into(), serde_json::json!(0.5));
        assert!(parse_referral_reward_config(&values).is_none());
        values.insert("referral_enabled".into(), serde_json::json!(false));
        assert!(parse_referral_reward_config(&values).is_none());
    }
}
