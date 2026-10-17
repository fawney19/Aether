use super::*;
use aether_data_contracts::repository::referrals::{
    parse_referral_reward_config, referral_config_bool, referral_config_string,
};

fn validate_settings(
    values: &serde_json::Map<String, serde_json::Value>,
) -> Result<(), DataLayerError> {
    let mode = referral_config_string(values.get("referral_reward_mode"))
        .unwrap_or_else(|| "percent".into());
    let trigger = referral_config_string(values.get("referral_headcount_trigger"))
        .unwrap_or_else(|| "registration".into());
    if !matches!(mode.as_str(), "percent" | "headcount" | "both")
        || !matches!(
            trigger.as_str(),
            "registration" | "email_verified" | "first_paid_order"
        )
    {
        return Err(DataLayerError::InvalidInput(
            "邀请返利模式或触发条件无效".into(),
        ));
    }
    if referral_config_bool(values.get("referral_enabled"), false)
        && matches!(mode.as_str(), "headcount" | "both")
        && trigger == "email_verified"
        && !referral_config_bool(values.get("require_email_verification"), false)
    {
        return Err(DataLayerError::InvalidInput(
            "邮箱验证返利必须同时启用注册邮箱验证".into(),
        ));
    }
    for (key, max) in [
        ("referral_recharge_percent", 100.0),
        ("referral_headcount_amount_usd", 999999999999.0),
    ] {
        if let Some(v) = values.get(key) {
            let number = v
                .as_f64()
                .or_else(|| v.as_str().and_then(|v| v.trim().parse::<f64>().ok()));
            if !number.is_some_and(|v| v.is_finite() && v >= 0.0 && v <= max) {
                return Err(DataLayerError::InvalidInput(
                    "邀请返利金额或比例无效".into(),
                ));
            }
        }
    }
    Ok(())
}

#[cfg(feature = "postgres")]
pub(super) async fn capture_registration_config(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<(Option<ReferralRewardConfig>, bool), DataLayerError> {
    sqlx::query("SELECT pg_advisory_xact_lock_shared(614329074)")
        .execute(&mut **tx)
        .await
        .map_err(DataLayerError::postgres)?;
    let rows = sqlx::query("SELECT key,value FROM system_configs WHERE key=ANY($1)")
        .bind(vec![
            "referral_enabled",
            "referral_reward_mode",
            "referral_recharge_percent",
            "referral_headcount_amount_usd",
            "referral_headcount_trigger",
            "require_email_verification",
        ])
        .fetch_all(&mut **tx)
        .await
        .map_err(DataLayerError::postgres)?;
    let mut values = serde_json::Map::new();
    for row in rows {
        values.insert(
            row_string!(row, "key"),
            row.try_get("value").map_err(DataLayerError::postgres)?,
        );
    }
    let required = referral_config_bool(values.get("require_email_verification"), false);
    let mut config = parse_referral_reward_config(&values);
    if let Some(config) = config.as_mut() {
        if config.headcount_trigger == "email_verified" && !required {
            config.headcount_enabled = false;
        }
    }
    Ok((config, required))
}

impl ReferralDataState<'_> {
    pub async fn update_referral_settings(
        &self,
        values: serde_json::Map<String, serde_json::Value>,
    ) -> Result<(), DataLayerError> {
        let keys = [
            "referral_enabled",
            "referral_reward_mode",
            "referral_recharge_percent",
            "referral_headcount_amount_usd",
            "referral_headcount_trigger",
            "require_email_verification",
        ];
        if values.keys().any(|key| !keys.contains(&key.as_str())) {
            return Err(DataLayerError::InvalidInput("不支持的邀请返利设置".into()));
        }
        #[cfg(feature = "postgres")]
        if let Some(backend) = self.backends.and_then(DataBackends::postgres) {
            let mut tx = backend
                .pool_clone()
                .begin()
                .await
                .map_err(DataLayerError::postgres)?;
            // The lock also protects absent config rows and cross-instance edits.
            sqlx::query("SELECT pg_advisory_xact_lock(614329074)")
                .execute(&mut *tx)
                .await
                .map_err(DataLayerError::postgres)?;
            let rows = sqlx::query("SELECT key,value FROM system_configs WHERE key=ANY($1)")
                .bind(keys.to_vec())
                .fetch_all(&mut *tx)
                .await
                .map_err(DataLayerError::postgres)?;
            let mut overlay = serde_json::Map::new();
            for row in rows {
                overlay.insert(
                    row_string!(row, "key"),
                    row.try_get("value").map_err(DataLayerError::postgres)?,
                );
            }
            overlay.extend(values.clone());
            validate_settings(&overlay)?;
            for (key, value) in values {
                sqlx::query("INSERT INTO system_configs(id,key,value,created_at,updated_at) VALUES($1,$2,$3,NOW(),NOW()) ON CONFLICT(key) DO UPDATE SET value=EXCLUDED.value,updated_at=NOW()")
                    .bind(uuid::Uuid::new_v4().to_string()).bind(key).bind(value).execute(&mut *tx).await.map_err(DataLayerError::postgres)?;
            }
            tx.commit().await.map_err(DataLayerError::postgres)?;
            return Ok(());
        }
        Err(DataLayerError::InvalidConfiguration(
            "referral settings require a database backend".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn email_rewards_require_real_verification_setting() {
        let mut values=serde_json::json!({"referral_enabled":true,"referral_reward_mode":"headcount","referral_headcount_trigger":"email_verified","require_email_verification":false}).as_object().unwrap().clone();
        assert!(validate_settings(&values).is_err());
        values.insert("require_email_verification".into(), true.into());
        assert!(validate_settings(&values).is_ok());
    }
    #[test]
    fn disabled_email_rewards_allow_disabling_registration_verification() {
        let values = serde_json::json!({"referral_enabled":false,"referral_reward_mode":"headcount","referral_headcount_trigger":"email_verified","require_email_verification":false});
        assert!(validate_settings(values.as_object().unwrap()).is_ok());
    }
}
