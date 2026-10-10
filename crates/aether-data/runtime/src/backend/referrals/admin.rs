//! Admin read models use existing facts; they never reconstruct historical rules.
use super::*;

#[derive(Debug, Clone, Serialize)]
pub struct ReferralAdminOverviewStats {
    #[serde(flatten)]
    pub legacy: ReferralAdminStats,
    pub cumulative_reward_usd: f64,
    pub failed_reward_count: u64,
    pub pending_reversal_reward_usd: f64,
    pub pending_reversal_reward_count: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReferralRewardDetail {
    pub reward: ReferralRewardRecord,
    pub relationship: Option<ReferralRelationshipRecord>,
    pub rule_snapshot: Option<ReferralRewardConfig>,
    pub source_order: Option<ReferralSourceOrder>,
    pub ledger_entries: Vec<ReferralLedgerEntry>,
    pub refunds: Vec<ReferralRefundRecord>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReferralSourceOrder {
    pub id: String,
    pub order_no: String,
    pub wallet_id: Option<String>,
    pub order_kind: String,
    pub amount_usd: f64,
    pub refunded_amount_usd: f64,
    pub status: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReferralLedgerEntry {
    pub id: String,
    pub reason_code: Option<String>,
    pub amount_usd: f64,
    pub created_at_unix_secs: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReferralRefundRecord {
    pub id: String,
    pub refund_no: String,
    pub status: String,
    pub refund_mode: String,
    pub wallet_id: String,
    pub refund_amount_usd: f64,
    pub created_at_unix_secs: u64,
}

const REWARD_SELECT: &str = r#"
SELECT rw.id, rw.referral_id, rw.inviter_user_id, rw.invitee_user_id,
       inviter.username AS inviter_username, invitee.username AS invitee_username,
       po.order_no AS source_order_no, w.id AS inviter_wallet_id,
       rw.reward_type, rw.source_order_id, rw.trigger_point,
       CAST(rw.amount_usd AS DOUBLE PRECISION) AS amount_usd,
       rw.status, rw.wallet_transaction_id, rw.idempotency_key,
       CAST(rw.reversed_amount_usd AS DOUBLE PRECISION) AS reversed_amount_usd,
       CAST(rw.pending_reversal_amount_usd AS DOUBLE PRECISION) AS pending_reversal_amount_usd,
       rw.admin_operator_id, rw.admin_note,
       EXTRACT(EPOCH FROM rw.created_at)::BIGINT AS created_at_unix_secs,
       EXTRACT(EPOCH FROM rw.updated_at)::BIGINT AS updated_at_unix_secs
"#;
const REWARD_FROM: &str = r#"
FROM referral_rewards rw
LEFT JOIN users inviter ON inviter.id = rw.inviter_user_id
LEFT JOIN users invitee ON invitee.id = rw.invitee_user_id
LEFT JOIN payment_orders po ON po.id = rw.source_order_id
LEFT JOIN wallets w ON w.user_id = rw.inviter_user_id
"#;
const REWARD_FILTER: &str = r#"
WHERE ($1 = '' OR LOWER(COALESCE(rw.source_order_id, '')) LIKE $1 ESCAPE '!')
AND ($2 = '' OR LOWER(rw.reward_type) LIKE $2 ESCAPE '!')
AND ($3 = '' OR LOWER(rw.status) LIKE $3 ESCAPE '!')
AND ($4 = '' OR LOWER(COALESCE(po.order_no, '')) LIKE $4 ESCAPE '!')
AND ($5 = '' OR LOWER(rw.inviter_user_id) LIKE $5 ESCAPE '!' OR LOWER(COALESCE(inviter.username, '')) LIKE $5 ESCAPE '!')
AND ($6 = '' OR LOWER(rw.invitee_user_id) LIKE $6 ESCAPE '!' OR LOWER(COALESCE(invitee.username, '')) LIKE $6 ESCAPE '!')
AND ($7::TEXT IS NULL OR rw.referral_id = $7)
AND ($8::TEXT IS NULL OR rw.trigger_point = $8)
AND ($9::BOOLEAN IS NULL OR (rw.pending_reversal_amount_usd > 0) = $9)
"#;

pub(super) async fn list_rewards(
    state: &ReferralDataState<'_>,
    query: &ReferralRewardListQuery,
) -> Result<(Vec<ReferralRewardRecord>, u64), DataLayerError> {
    #[cfg(feature = "postgres")]
    if let Some(backend) = state.backends.and_then(DataBackends::postgres) {
        let patterns = [
            query.order_id.as_deref(),
            query.reward_type.as_deref(),
            query.status.as_deref(),
            query.order_no.as_deref(),
            query.inviter.as_deref(),
            query.invitee.as_deref(),
        ]
        .map(referral_like_pattern);
        let pool = backend.pool_clone();
        let count_sql = format!("SELECT COUNT(*) AS total {REWARD_FROM} {REWARD_FILTER}");
        let bind_filters = |sql| {
            sqlx::query(sql)
                .bind(patterns[0].clone())
                .bind(patterns[1].clone())
                .bind(patterns[2].clone())
                .bind(patterns[3].clone())
                .bind(patterns[4].clone())
                .bind(patterns[5].clone())
                .bind(query.referral_id.clone())
                .bind(query.trigger_point.clone())
                .bind(query.pending_reversal)
        };
        let count = bind_filters(&count_sql)
            .fetch_one(&pool)
            .await
            .map_err(DataLayerError::postgres)?;
        let sql = format!("{REWARD_SELECT} {REWARD_FROM} {REWARD_FILTER} ORDER BY rw.created_at DESC, rw.id DESC LIMIT $10 OFFSET $11");
        let (limit, offset) = referral_page_bounds(query.limit, query.offset);
        let rows = bind_filters(&sql)
            .bind(limit)
            .bind(offset)
            .fetch_all(&pool)
            .await
            .map_err(DataLayerError::postgres)?;
        let items = rows
            .iter()
            .map(|row| reward_from_row!(row))
            .collect::<Result<Vec<_>, _>>()?;
        return Ok((items, row_string_count(&count, "total")?));
    }
    Ok((Vec::new(), 0))
}

#[cfg(feature = "postgres")]
fn row_string_count(row: &sqlx::postgres::PgRow, key: &str) -> Result<u64, DataLayerError> {
    Ok(row
        .try_get::<i64, _>(key)
        .map_err(DataLayerError::postgres)?
        .max(0) as u64)
}

fn historical_config(value: Option<&serde_json::Value>) -> Option<ReferralRewardConfig> {
    value?
        .get(REFERRAL_SNAPSHOT_KEY)?
        .get("config")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
}

impl ReferralDataState<'_> {
    pub async fn referral_admin_overview_stats(
        &self,
    ) -> Result<Option<ReferralAdminOverviewStats>, DataLayerError> {
        #[cfg(feature = "postgres")]
        if let Some(backend) = self.backends.and_then(DataBackends::postgres) {
            let legacy = self.referral_admin_stats_global(None).await?;
            let row = sqlx::query(r#"
SELECT CAST(COALESCE(SUM(CASE WHEN rw.status IN ('applied','reversed') AND rw.wallet_transaction_id IS NOT NULL AND EXISTS (
 SELECT 1 FROM wallet_transactions tx JOIN wallets w ON w.id = tx.wallet_id
 WHERE tx.id = rw.wallet_transaction_id AND tx.link_type = 'referral_reward' AND tx.link_id = rw.id
 AND tx.reason_code = 'referral_reward' AND tx.category = 'adjust' AND tx.amount = rw.amount_usd
 AND w.user_id = rw.inviter_user_id AND tx.amount > 0
) THEN rw.amount_usd ELSE 0 END),0) AS DOUBLE PRECISION) AS cumulative_reward_usd,
COUNT(*) FILTER (WHERE rw.status = 'failed') AS failed_reward_count,
CAST(COALESCE(SUM(CASE WHEN rw.pending_reversal_amount_usd > 0 THEN rw.pending_reversal_amount_usd ELSE 0 END),0) AS DOUBLE PRECISION) AS pending_reversal_reward_usd,
COUNT(*) FILTER (WHERE rw.pending_reversal_amount_usd > 0) AS pending_reversal_reward_count
FROM referral_rewards rw
"#).fetch_one(&backend.pool_clone()).await.map_err(DataLayerError::postgres)?;
            return Ok(Some(ReferralAdminOverviewStats {
                legacy,
                cumulative_reward_usd: row_f64!(&row, "cumulative_reward_usd"),
                failed_reward_count: row_string_count(&row, "failed_reward_count")?,
                pending_reversal_reward_usd: row_f64!(&row, "pending_reversal_reward_usd"),
                pending_reversal_reward_count: row_string_count(
                    &row,
                    "pending_reversal_reward_count",
                )?,
            }));
        }
        Ok(None)
    }

    pub async fn referral_reward_detail(
        &self,
        reward_id: &str,
    ) -> Result<Option<ReferralRewardDetail>, DataLayerError> {
        #[cfg(feature = "postgres")]
        if let Some(backend) = self.backends.and_then(DataBackends::postgres) {
            let pool = backend.pool_clone();
            let sql = format!("{REWARD_SELECT} {REWARD_FROM} WHERE rw.id=$1");
            let Some(row) = sqlx::query(&sql)
                .bind(reward_id)
                .fetch_optional(&pool)
                .await
                .map_err(DataLayerError::postgres)?
            else {
                return Ok(None);
            };
            let reward = reward_from_row!(&row)?;
            let mut relationship = self.find_referral_relationship(&reward.referral_id).await?;
            // Capture the registration rule before clearing the source payload from the public DTO.
            let registration_config =
                historical_config(relationship.as_ref().and_then(|r| r.source.as_ref()));
            if let Some(r) = relationship.as_mut() {
                r.source = None;
            }
            let mut rule_snapshot = if matches!(
                reward.trigger_point.as_str(),
                "registration" | "email_verified"
            ) {
                registration_config
            } else {
                None
            };
            let mut source_order = None;
            let mut refunds = Vec::new();
            if let Some(order_id) = reward.source_order_id.as_deref() {
                if let Some(order) = sqlx::query("SELECT id,order_no,wallet_id,order_kind,CAST(amount_usd AS DOUBLE PRECISION) AS amount_usd ,status,gateway_response FROM payment_orders WHERE id=$1").bind(order_id).fetch_optional(&pool).await.map_err(DataLayerError::postgres)? {
                    let response: Option<serde_json::Value> = order.try_get("gateway_response").map_err(DataLayerError::postgres)?;
                    rule_snapshot = historical_config(response.as_ref());
                    source_order = Some(ReferralSourceOrder { id: row_string!(&order,"id"),order_no: row_string!(&order,"order_no"),wallet_id: row_optional_string!(&order,"wallet_id"),order_kind:row_string!(&order,"order_kind"),amount_usd:row_f64!(&order,"amount_usd"),refunded_amount_usd:self.find_referral_payment_order_refund_context(order_id).await?.map(|c| c.refunded_amount_usd).unwrap_or(0.0),status:row_string!(&order,"status") });
                } else { rule_snapshot = None; }
                let rows = sqlx::query("SELECT id,refund_no,status,refund_mode,wallet_id,CAST(amount_usd AS DOUBLE PRECISION) AS refund_amount_usd,EXTRACT(EPOCH FROM created_at)::BIGINT AS created_at_unix_secs FROM refund_requests WHERE payment_order_id=$1 ORDER BY created_at,id").bind(order_id).fetch_all(&pool).await.map_err(DataLayerError::postgres)?;
                for row in rows {
                    refunds.push(ReferralRefundRecord {
                        id: row_string!(&row, "id"),
                        refund_no: row_string!(&row, "refund_no"),
                        status: row_string!(&row, "status"),
                        refund_mode: row_string!(&row, "refund_mode"),
                        wallet_id: row_string!(&row, "wallet_id"),
                        refund_amount_usd: row_f64!(&row, "refund_amount_usd"),
                        created_at_unix_secs: row_unix_secs(&row, "created_at_unix_secs")?,
                    });
                }
            }
            let rows = sqlx::query("SELECT tx.id,tx.reason_code,CAST(tx.amount AS DOUBLE PRECISION) AS amount_usd,EXTRACT(EPOCH FROM tx.created_at)::BIGINT AS created_at_unix_secs FROM wallet_transactions tx JOIN wallets w ON w.id=tx.wallet_id WHERE tx.link_type='referral_reward' AND tx.link_id=$1 AND w.user_id=$2 AND tx.reason_code IN ('referral_reward','referral_reward_reversal') ORDER BY tx.created_at,tx.id").bind(reward_id).bind(&reward.inviter_user_id).fetch_all(&pool).await.map_err(DataLayerError::postgres)?;
            let ledger_entries = rows
                .iter()
                .map(|row| {
                    Ok(ReferralLedgerEntry {
                        id: row_string!(row, "id"),
                        reason_code: row_optional_string!(row, "reason_code"),
                        amount_usd: row_f64!(row, "amount_usd"),
                        created_at_unix_secs: row_unix_secs(row, "created_at_unix_secs")?,
                    })
                })
                .collect::<Result<Vec<_>, DataLayerError>>()?;
            return Ok(Some(ReferralRewardDetail {
                reward,
                relationship,
                rule_snapshot,
                source_order,
                ledger_entries,
                refunds,
            }));
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn admin_historical_rules_only_read_whitelisted_snapshot() {
        let config = ReferralRewardConfig {
            percent_enabled: true,
            percent_rate: 5.0,
            headcount_enabled: false,
            headcount_amount_usd: 0.0,
            headcount_trigger: "registration".into(),
        };
        let value =
            serde_json::json!({(REFERRAL_SNAPSHOT_KEY):{"config":config},"secret":"provider-key"});
        let serialized = serde_json::to_value(historical_config(Some(&value)).unwrap()).unwrap();
        assert_eq!(serialized["percent_rate"], 5.0);
        assert!(serialized.get("secret").is_none());
        assert!(historical_config(Some(&serde_json::json!({"percent_rate":99.0}))).is_none());
        assert!(historical_config(None).is_none());
    }
}
