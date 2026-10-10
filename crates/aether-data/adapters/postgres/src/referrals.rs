//! Referral obligations are committed with the payment credit, never inferred
//! later from mutable settings or a replayed provider callback.
use crate::DataLayerError;
use aether_data_contracts::repository::referrals::{
    is_real_referral_payment, parse_referral_reward_config, REFERRAL_SNAPSHOT_KEY,
};
use sqlx::{Postgres, Row, Transaction};

pub(crate) async fn capture_paid_referral_obligations(
    tx: &mut Transaction<'_, Postgres>,
    order_id: &str,
) -> Result<serde_json::Value, DataLayerError> {
    let order = sqlx::query("SELECT user_id, payment_method, order_kind, CAST(amount_usd AS DOUBLE PRECISION) AS amount_usd, gateway_response FROM payment_orders WHERE id=$1 FOR UPDATE")
        .bind(order_id).fetch_one(&mut **tx).await.map_err(DataLayerError::postgres)?;
    let existing: Option<serde_json::Value> = order
        .try_get("gateway_response")
        .map_err(DataLayerError::postgres)?;
    // Called only on the first credited transition: overwrite any provider-supplied internal field.
    let user_id: Option<String> = order.try_get("user_id").map_err(DataLayerError::postgres)?;
    let method: String = order
        .try_get("payment_method")
        .map_err(DataLayerError::postgres)?;
    let kind: String = order
        .try_get("order_kind")
        .map_err(DataLayerError::postgres)?;
    let amount: f64 = order
        .try_get("amount_usd")
        .map_err(DataLayerError::postgres)?;
    let eligible = user_id.is_some() && is_real_referral_payment(&method, &kind, amount);
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
        ])
        .fetch_all(&mut **tx)
        .await
        .map_err(DataLayerError::postgres)?;
    let mut values = serde_json::Map::new();
    for row in rows {
        let key: String = row.try_get("key").map_err(DataLayerError::postgres)?;
        let value: serde_json::Value = row.try_get("value").map_err(DataLayerError::postgres)?;
        values.insert(key, value);
    }
    let config = parse_referral_reward_config(&values);
    let enabled = config.is_some();
    let mut response = match existing {
        Some(serde_json::Value::Object(map)) => map,
        Some(v) => serde_json::Map::from_iter([("provider_response".to_string(), v)]),
        None => serde_json::Map::new(),
    };
    response.insert(
        REFERRAL_SNAPSHOT_KEY.to_string(),
        serde_json::json!({"enabled":enabled,"eligible":eligible,"config":config}),
    );
    let response = serde_json::Value::Object(response);
    if !eligible {
        return Ok(response);
    }
    let referral = sqlx::query("SELECT id,inviter_user_id,invitee_user_id,first_paid_order_id FROM user_referrals WHERE invitee_user_id=$1 FOR UPDATE")
        .bind(user_id).fetch_optional(&mut **tx).await.map_err(DataLayerError::postgres)?;
    let Some(referral) = referral else {
        return Ok(response);
    };
    let id: String = referral.try_get("id").map_err(DataLayerError::postgres)?;
    let inviter: String = referral
        .try_get("inviter_user_id")
        .map_err(DataLayerError::postgres)?;
    let invitee: String = referral
        .try_get("invitee_user_id")
        .map_err(DataLayerError::postgres)?;
    let first: Option<String> = referral
        .try_get("first_paid_order_id")
        .map_err(DataLayerError::postgres)?;
    let prior = if first.is_none() {
        // Legacy rows may not have tracked payments while the feature was off.
        // Use actual historical credit facts only to rule out a new first-paid
        // award; never infer an old rate or create historical money obligations.
        sqlx::query_scalar::<_,String>("SELECT id FROM payment_orders WHERE user_id=$1 AND id<>$2 AND (status IN ('credited','refunded') OR credited_at IS NOT NULL) AND order_kind IN ('wallet_recharge','plan_purchase') AND amount_usd>0 AND LOWER(BTRIM(payment_method)) NOT IN ('manual','admin_manual','redeem_code','gift','card_code','gift_code','card_recharge') ORDER BY COALESCE(credited_at,paid_at,created_at),id LIMIT 1")
            .bind(&invitee).bind(order_id).fetch_optional(&mut **tx).await.map_err(DataLayerError::postgres)?
    } else {
        None
    };
    let is_first = first.is_none() && prior.is_none();
    if first.is_none() {
        let first_order = prior.as_deref().unwrap_or(order_id);
        sqlx::query("UPDATE user_referrals SET first_paid_order_id=$2,first_paid_at=CASE WHEN $2=$3 THEN NOW() ELSE COALESCE((SELECT COALESCE(credited_at,paid_at,created_at) FROM payment_orders WHERE id=$2),NOW()) END,updated_at=NOW() WHERE id=$1 AND first_paid_order_id IS NULL").bind(&id).bind(first_order).bind(order_id).execute(&mut **tx).await.map_err(DataLayerError::postgres)?;
    }
    let Some(config) = config else {
        return Ok(response);
    };
    if config.percent_enabled && config.percent_rate > 0.0 {
        insert_reward(
            tx,
            &id,
            &inviter,
            &invitee,
            "percent",
            Some(order_id),
            "paid_order",
            amount * config.percent_rate / 100.0,
            &format!("referral:{id}:percent:{order_id}"),
        )
        .await?;
    }
    if is_first && config.headcount_enabled && config.headcount_trigger == "first_paid_order" {
        insert_reward(
            tx,
            &id,
            &inviter,
            &invitee,
            "headcount",
            Some(order_id),
            "first_paid_order",
            config.headcount_amount_usd,
            &format!("referral:{id}:headcount:first_paid_order"),
        )
        .await?;
    }
    Ok(response)
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn insert_reward(
    tx: &mut Transaction<'_, Postgres>,
    referral_id: &str,
    inviter: &str,
    invitee: &str,
    reward_type: &str,
    order: Option<&str>,
    trigger: &str,
    amount: f64,
    key: &str,
) -> Result<(), DataLayerError> {
    if !amount.is_finite() || amount < 0.000000005 {
        return Ok(());
    }
    // The relationship row is locked by the caller. A change of trigger, or a
    // voided old award, must not create a second headcount obligation.
    sqlx::query("INSERT INTO referral_rewards(id,referral_id,inviter_user_id,invitee_user_id,reward_type,source_order_id,trigger_point,amount_usd,status,idempotency_key,created_at,updated_at) SELECT $1,$2,$3,$4,$5,$6,$7,$8,'pending',$9,NOW(),NOW() WHERE $5<>'headcount' OR NOT EXISTS(SELECT 1 FROM referral_rewards WHERE referral_id=$2 AND reward_type='headcount') ON CONFLICT(idempotency_key) DO NOTHING")
        .bind(uuid::Uuid::new_v4().to_string()).bind(referral_id).bind(inviter).bind(invitee).bind(reward_type).bind(order).bind(trigger).bind(amount).bind(key).execute(&mut **tx).await.map_err(DataLayerError::postgres)?;
    Ok(())
}
