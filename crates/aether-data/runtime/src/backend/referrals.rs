use crate::DataLayerError;
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::collections::HashSet;

use super::DataBackends;

#[derive(Debug, Clone, Copy)]
pub struct ReferralDataState<'a> {
    backends: Option<&'a DataBackends>,
}

impl<'a> ReferralDataState<'a> {
    pub fn new(backends: Option<&'a DataBackends>) -> Self {
        Self { backends }
    }
}

const REFERRAL_FETCH_LIMIT: usize = 5_000;

#[derive(Debug, Clone, Serialize)]
pub struct ReferralUserDashboard {
    pub invite_code: String,
    pub total_invites: u64,
    pub effective_invites: u64,
    pub paid_reward_usd: f64,
    pub pending_reward_usd: f64,
    pub reversed_reward_usd: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReferralRelationshipRecord {
    pub id: String,
    pub inviter_user_id: String,
    pub inviter_username: Option<String>,
    pub invitee_user_id: String,
    pub invitee_username: Option<String>,
    pub invite_code_snapshot: String,
    pub first_paid_order_id: Option<String>,
    pub first_paid_at_unix_secs: Option<u64>,
    pub source: Option<serde_json::Value>,
    pub created_at_unix_secs: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReferralRewardRecord {
    pub id: String,
    pub referral_id: String,
    pub inviter_user_id: String,
    pub invitee_user_id: String,
    pub reward_type: String,
    pub source_order_id: Option<String>,
    pub trigger_point: String,
    pub amount_usd: f64,
    pub status: String,
    pub wallet_transaction_id: Option<String>,
    pub idempotency_key: String,
    pub reversed_amount_usd: f64,
    pub pending_reversal_amount_usd: f64,
    pub admin_operator_id: Option<String>,
    pub admin_note: Option<String>,
    pub created_at_unix_secs: u64,
    pub updated_at_unix_secs: u64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ReferralAdminStats {
    pub total_invites: u64,
    pub effective_invites: u64,
    pub paid_reward_usd: f64,
    pub pending_reward_usd: f64,
    pub reversed_reward_usd: f64,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ReferralRelationshipListQuery {
    pub inviter: Option<String>,
    pub invitee: Option<String>,
    pub invite_code: Option<String>,
    pub first_paid: Option<bool>,
    pub limit: usize,
    pub offset: usize,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ReferralRewardListQuery {
    pub order_id: Option<String>,
    pub reward_type: Option<String>,
    pub status: Option<String>,
    pub limit: usize,
    pub offset: usize,
}

#[derive(Debug, Clone)]
pub struct ReferralRewardConfig {
    pub percent_enabled: bool,
    pub percent_rate: f64,
    pub headcount_enabled: bool,
    pub headcount_amount_usd: f64,
    pub headcount_trigger: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferralMutationStatus {
    Applied,
    NotFound,
    Invalid,
    Unavailable,
}

#[derive(Debug, Clone)]
struct ReferralPaymentOrderContext {
    id: String,
    user_id: String,
    amount_usd: f64,
    payment_method: String,
    status: String,
    order_kind: String,
}

#[derive(Debug, Clone)]
struct ReferralPaymentOrderRefundContext {
    amount_usd: f64,
    refunded_amount_usd: f64,
}

#[derive(Debug, Clone)]
struct ReferralCreditTarget {
    id: String,
    wallet_id: String,
    amount_usd: f64,
    reward_type: String,
}

macro_rules! row_string {
    ($row:expr, $col:expr) => {
        $row.try_get::<String, _>($col)
            .map_err(DataLayerError::sql)?
    };
}

macro_rules! row_optional_string {
    ($row:expr, $col:expr) => {
        $row.try_get::<Option<String>, _>($col)
            .map_err(DataLayerError::sql)?
    };
}

macro_rules! row_f64 {
    ($row:expr, $col:expr) => {
        $row.try_get::<f64, _>($col).map_err(DataLayerError::sql)?
    };
}

macro_rules! relationship_from_row {
    ($row:expr) => {{
        let source_text = row_optional_string!($row, "source_json");
        Ok(ReferralRelationshipRecord {
            id: row_string!($row, "id"),
            inviter_user_id: row_string!($row, "inviter_user_id"),
            inviter_username: row_optional_string!($row, "inviter_username"),
            invitee_user_id: row_string!($row, "invitee_user_id"),
            invitee_username: row_optional_string!($row, "invitee_username"),
            invite_code_snapshot: row_string!($row, "invite_code_snapshot"),
            first_paid_order_id: row_optional_string!($row, "first_paid_order_id"),
            first_paid_at_unix_secs: row_optional_unix_secs($row, "first_paid_at_unix_secs")?,
            source: parse_optional_json(source_text)?,
            created_at_unix_secs: row_unix_secs($row, "created_at_unix_secs")?,
        })
    }};
}

macro_rules! reward_from_row {
    ($row:expr) => {{
        Ok(ReferralRewardRecord {
            id: row_string!($row, "id"),
            referral_id: row_string!($row, "referral_id"),
            inviter_user_id: row_string!($row, "inviter_user_id"),
            invitee_user_id: row_string!($row, "invitee_user_id"),
            reward_type: row_string!($row, "reward_type"),
            source_order_id: row_optional_string!($row, "source_order_id"),
            trigger_point: row_string!($row, "trigger_point"),
            amount_usd: row_f64!($row, "amount_usd"),
            status: row_string!($row, "status"),
            wallet_transaction_id: row_optional_string!($row, "wallet_transaction_id"),
            idempotency_key: row_string!($row, "idempotency_key"),
            reversed_amount_usd: row_f64!($row, "reversed_amount_usd"),
            pending_reversal_amount_usd: row_f64!($row, "pending_reversal_amount_usd"),
            admin_operator_id: row_optional_string!($row, "admin_operator_id"),
            admin_note: row_optional_string!($row, "admin_note"),
            created_at_unix_secs: row_unix_secs($row, "created_at_unix_secs")?,
            updated_at_unix_secs: row_unix_secs($row, "updated_at_unix_secs")?,
        })
    }};
}

#[cfg(any(feature = "mysql", feature = "sqlite"))]
fn now_unix_secs() -> u64 {
    chrono::Utc::now().timestamp().max(0) as u64
}

#[cfg(any(feature = "mysql", feature = "sqlite"))]
fn now_unix_ms() -> u64 {
    chrono::Utc::now().timestamp_millis().max(0) as u64
}

fn row_unix_secs<R>(row: &R, column: &str) -> Result<u64, DataLayerError>
where
    R: Row,
    for<'c> &'c str: sqlx::ColumnIndex<R>,
    for<'r> i64: sqlx::Decode<'r, R::Database> + sqlx::Type<R::Database>,
{
    let value = row.try_get::<i64, _>(column).map_err(DataLayerError::sql)?;
    Ok(value.max(0) as u64)
}

fn row_optional_unix_secs<R>(row: &R, column: &str) -> Result<Option<u64>, DataLayerError>
where
    R: Row,
    for<'c> &'c str: sqlx::ColumnIndex<R>,
    for<'r> Option<i64>: sqlx::Decode<'r, R::Database> + sqlx::Type<R::Database>,
{
    let value = row
        .try_get::<Option<i64>, _>(column)
        .map_err(DataLayerError::sql)?;
    Ok(value.map(|value| value.max(0) as u64))
}

fn parse_optional_json(value: Option<String>) -> Result<Option<serde_json::Value>, DataLayerError> {
    value
        .filter(|value| !value.trim().is_empty())
        .map(|value| serde_json::from_str(&value).map_err(DataLayerError::sql))
        .transpose()
}

fn generate_invite_code() -> String {
    format!(
        "AE{}",
        &uuid::Uuid::new_v4().simple().to_string()[..10].to_ascii_uppercase()
    )
}

fn referral_text_matches(value: Option<&str>, needle: Option<&str>) -> bool {
    let Some(needle) = needle.map(str::trim).filter(|value| !value.is_empty()) else {
        return true;
    };
    let Some(value) = value else {
        return false;
    };
    value
        .to_ascii_lowercase()
        .contains(&needle.to_ascii_lowercase())
}

fn referral_list_window<T: Clone>(items: &[T], limit: usize, offset: usize) -> Vec<T> {
    let limit = limit.clamp(1, 200);
    items.iter().skip(offset).take(limit).cloned().collect()
}

fn referral_admin_stats(
    relationships: &[ReferralRelationshipRecord],
    rewards: &[ReferralRewardRecord],
) -> ReferralAdminStats {
    ReferralAdminStats {
        total_invites: relationships.len() as u64,
        effective_invites: relationships
            .iter()
            .filter(|item| item.first_paid_order_id.is_some())
            .count() as u64,
        paid_reward_usd: rewards
            .iter()
            .filter(|item| item.status == "applied")
            .map(|item| item.amount_usd)
            .sum(),
        pending_reward_usd: rewards
            .iter()
            .filter(|item| matches!(item.status.as_str(), "pending" | "failed"))
            .map(|item| item.amount_usd)
            .sum(),
        reversed_reward_usd: rewards.iter().map(|item| item.reversed_amount_usd).sum(),
    }
}

fn normalize_referral_code(value: &str) -> Option<String> {
    let value = value.trim().to_ascii_uppercase();
    (!value.is_empty() && value.len() <= 64).then_some(value)
}

fn reward_description(target: &ReferralCreditTarget) -> String {
    match target.reward_type.as_str() {
        "percent" => "邀请充值比例返利".to_string(),
        "headcount" => "邀请人头返利".to_string(),
        _ => "邀请返利".to_string(),
    }
}

fn referral_retry_allowed(status: &str) -> bool {
    status == "failed"
}

fn referral_void_allowed(status: &str) -> bool {
    matches!(status, "pending" | "failed")
}

fn referral_reversal_delta(
    reward_amount_usd: f64,
    order_amount_usd: f64,
    refunded_amount_usd: f64,
    reversed_amount_usd: f64,
    pending_reversal_amount_usd: f64,
) -> f64 {
    let target_reversal =
        referral_reversal_target(reward_amount_usd, order_amount_usd, refunded_amount_usd);
    (target_reversal - reversed_amount_usd - pending_reversal_amount_usd).max(0.0)
}

fn referral_reversal_target(
    reward_amount_usd: f64,
    order_amount_usd: f64,
    refunded_amount_usd: f64,
) -> f64 {
    if reward_amount_usd <= 0.0 || order_amount_usd <= 0.0 || refunded_amount_usd <= 0.0 {
        return 0.0;
    }
    reward_amount_usd * (refunded_amount_usd / order_amount_usd).clamp(0.0, 1.0)
}

impl ReferralDataState<'_> {
    pub fn has_referral_data_backend(&self) -> bool {
        self.backends.is_some()
    }

    pub async fn record_user_privacy_policy_acceptance(
        &self,
        user_id: &str,
        version: &str,
    ) -> Result<bool, DataLayerError> {
        let Some(backends) = self.backends.as_ref() else {
            return Ok(false);
        };
        #[cfg(feature = "postgres")]
        if let Some(backend) = backends.postgres() {
            let affected = sqlx::query(
                r#"
UPDATE users
SET privacy_policy_accepted_version = $2,
    privacy_policy_accepted_at = NOW()
WHERE id = $1
"#,
            )
            .bind(user_id)
            .bind(version)
            .execute(&backend.pool_clone())
            .await
            .map_err(DataLayerError::postgres)?
            .rows_affected();
            return Ok(affected > 0);
        }
        #[cfg(feature = "mysql")]
        if let Some(backend) = backends.mysql() {
            let affected = sqlx::query(
                r#"
UPDATE users
SET privacy_policy_accepted_version = ?,
    privacy_policy_accepted_at = ?
WHERE id = ?
"#,
            )
            .bind(version)
            .bind(now_unix_secs() as i64)
            .bind(user_id)
            .execute(&backend.pool_clone())
            .await
            .map_err(DataLayerError::sql)?
            .rows_affected();
            return Ok(affected > 0);
        }
        #[cfg(feature = "sqlite")]
        if let Some(backend) = backends.sqlite() {
            let affected = sqlx::query(
                r#"
UPDATE users
SET privacy_policy_accepted_version = ?,
    privacy_policy_accepted_at = ?
WHERE id = ?
"#,
            )
            .bind(version)
            .bind(now_unix_secs() as i64)
            .bind(user_id)
            .execute(&backend.pool_clone())
            .await
            .map_err(DataLayerError::sql)?
            .rows_affected();
            return Ok(affected > 0);
        }
        Ok(false)
    }

    pub async fn referral_dashboard(
        &self,
        user_id: &str,
    ) -> Result<Option<ReferralUserDashboard>, DataLayerError> {
        let Some(invite_code) = self.ensure_referral_invite_code(user_id).await? else {
            return Ok(None);
        };
        let relationships = self
            .list_referral_relationships_raw(Some(user_id), None)
            .await?;
        let rewards = self.list_referral_rewards_raw(Some(user_id)).await?;
        let stats = referral_admin_stats(&relationships, &rewards);
        Ok(Some(ReferralUserDashboard {
            invite_code,
            total_invites: stats.total_invites,
            effective_invites: stats.effective_invites,
            paid_reward_usd: stats.paid_reward_usd,
            pending_reward_usd: stats.pending_reward_usd,
            reversed_reward_usd: stats.reversed_reward_usd,
        }))
    }

    pub async fn list_admin_referral_relationships(
        &self,
        query: ReferralRelationshipListQuery,
    ) -> Result<Option<(Vec<ReferralRelationshipRecord>, u64, ReferralAdminStats)>, DataLayerError>
    {
        if self.backends.is_none() {
            return Ok(None);
        }
        let all_relationships = self.list_referral_relationships_raw(None, None).await?;
        let all_rewards = self.list_referral_rewards_raw(None).await?;
        let filtered = all_relationships
            .into_iter()
            .filter(|item| {
                referral_text_matches(item.inviter_username.as_deref(), query.inviter.as_deref())
                    || referral_text_matches(
                        Some(item.inviter_user_id.as_str()),
                        query.inviter.as_deref(),
                    )
            })
            .filter(|item| {
                referral_text_matches(item.invitee_username.as_deref(), query.invitee.as_deref())
                    || referral_text_matches(
                        Some(item.invitee_user_id.as_str()),
                        query.invitee.as_deref(),
                    )
            })
            .filter(|item| {
                referral_text_matches(
                    Some(item.invite_code_snapshot.as_str()),
                    query.invite_code.as_deref(),
                )
            })
            .filter(|item| {
                query
                    .first_paid
                    .map(|expected| item.first_paid_order_id.is_some() == expected)
                    .unwrap_or(true)
            })
            .collect::<Vec<_>>();
        let total = filtered.len() as u64;
        let filtered_referral_ids = filtered
            .iter()
            .map(|item| item.id.as_str())
            .collect::<HashSet<_>>();
        let filtered_rewards = all_rewards
            .into_iter()
            .filter(|item| filtered_referral_ids.contains(item.referral_id.as_str()))
            .collect::<Vec<_>>();
        let stats = referral_admin_stats(&filtered, &filtered_rewards);
        Ok(Some((
            referral_list_window(&filtered, query.limit, query.offset),
            total,
            stats,
        )))
    }

    pub async fn list_admin_referral_rewards(
        &self,
        query: ReferralRewardListQuery,
    ) -> Result<Option<(Vec<ReferralRewardRecord>, u64, ReferralAdminStats)>, DataLayerError> {
        if self.backends.is_none() {
            return Ok(None);
        }
        let relationships = self.list_referral_relationships_raw(None, None).await?;
        let rewards = self.list_referral_rewards_raw(None).await?;
        let filtered = rewards
            .iter()
            .filter(|item| {
                referral_text_matches(item.source_order_id.as_deref(), query.order_id.as_deref())
            })
            .filter(|item| {
                referral_text_matches(
                    Some(item.reward_type.as_str()),
                    query.reward_type.as_deref(),
                )
            })
            .filter(|item| {
                referral_text_matches(Some(item.status.as_str()), query.status.as_deref())
            })
            .cloned()
            .collect::<Vec<_>>();
        let total = filtered.len() as u64;
        let filtered_referral_ids = filtered
            .iter()
            .map(|item| item.referral_id.as_str())
            .collect::<HashSet<_>>();
        let filtered_relationships = relationships
            .into_iter()
            .filter(|item| filtered_referral_ids.contains(item.id.as_str()))
            .collect::<Vec<_>>();
        let stats = referral_admin_stats(&filtered_relationships, &filtered);
        Ok(Some((
            referral_list_window(&filtered, query.limit, query.offset),
            total,
            stats,
        )))
    }

    pub async fn bind_referral_invite_code(
        &self,
        invitee_user_id: &str,
        invite_code: Option<&str>,
        source: Option<serde_json::Value>,
    ) -> Result<Option<ReferralRelationshipRecord>, DataLayerError> {
        let Some(code) = invite_code.and_then(normalize_referral_code) else {
            return Ok(None);
        };
        let Some(inviter_user_id) = self.find_referral_inviter_by_code(&code).await? else {
            return Err(DataLayerError::InvalidInput("邀请码无效".to_string()));
        };
        if inviter_user_id == invitee_user_id {
            return Err(DataLayerError::InvalidInput(
                "不能使用自己的邀请码注册".to_string(),
            ));
        }
        let referral_id = uuid::Uuid::new_v4().to_string();
        let source_json = source.map(|value| value.to_string());
        let inserted = self
            .insert_referral_relationship(
                &referral_id,
                &inviter_user_id,
                invitee_user_id,
                &code,
                source_json.as_deref(),
            )
            .await?;
        if !inserted {
            return Ok(None);
        }
        self.find_referral_relationship(&referral_id).await
    }

    pub async fn apply_registration_referral_reward(
        &self,
        invitee_user_id: &str,
        amount_usd: f64,
        trigger_point: &str,
    ) -> Result<Vec<ReferralRewardRecord>, DataLayerError> {
        if amount_usd <= 0.0 {
            return Ok(Vec::new());
        }
        let Some(relationship) = self
            .find_referral_relationship_by_invitee(invitee_user_id)
            .await?
        else {
            return Ok(Vec::new());
        };
        let idempotency_key = format!("referral:{}:headcount:{trigger_point}", relationship.id);
        self.insert_referral_reward(
            &relationship,
            "headcount",
            None,
            trigger_point,
            amount_usd,
            &idempotency_key,
        )
        .await?;
        self.credit_pending_referral_rewards(&[idempotency_key], None, None)
            .await
    }

    pub async fn apply_paid_order_referral_rewards(
        &self,
        order_id: &str,
        config: ReferralRewardConfig,
    ) -> Result<Vec<ReferralRewardRecord>, DataLayerError> {
        if self.backends.is_none() {
            return Ok(Vec::new());
        }
        if !config.percent_enabled && !config.headcount_enabled {
            return Ok(Vec::new());
        }
        let Some(context) = self.find_referral_payment_order_context(order_id).await? else {
            return Ok(Vec::new());
        };
        if context.status != "credited" {
            return Ok(Vec::new());
        }
        if !matches!(
            context.order_kind.as_str(),
            "wallet_recharge" | "plan_purchase"
        ) {
            return Ok(Vec::new());
        }
        if matches!(
            context.payment_method.as_str(),
            "manual" | "admin_manual" | "redeem_code" | "gift"
        ) {
            return Ok(Vec::new());
        }
        let Some(relationship) = self
            .find_referral_relationship_by_invitee(&context.user_id)
            .await?
        else {
            return Ok(Vec::new());
        };
        let marked_first_paid = self
            .mark_referral_first_paid_order(&relationship.id, &context.id)
            .await?;

        let mut idempotency_keys = Vec::new();
        if config.percent_enabled && config.percent_rate > 0.0 {
            let amount_usd = (context.amount_usd * config.percent_rate / 100.0).max(0.0);
            if amount_usd > 0.0 {
                let idempotency_key =
                    format!("referral:{}:percent:{}", relationship.id, context.id);
                self.insert_referral_reward(
                    &relationship,
                    "percent",
                    Some(&context.id),
                    "paid_order",
                    amount_usd,
                    &idempotency_key,
                )
                .await?;
                idempotency_keys.push(idempotency_key);
            }
        }
        if config.headcount_enabled
            && config.headcount_amount_usd > 0.0
            && config.headcount_trigger == "first_paid_order"
            && marked_first_paid
        {
            let idempotency_key =
                format!("referral:{}:headcount:first_paid_order", relationship.id);
            self.insert_referral_reward(
                &relationship,
                "headcount",
                Some(&context.id),
                "first_paid_order",
                config.headcount_amount_usd,
                &idempotency_key,
            )
            .await?;
            idempotency_keys.push(idempotency_key);
        }

        if idempotency_keys.is_empty() {
            return Ok(Vec::new());
        }
        self.credit_pending_referral_rewards(&idempotency_keys, None, None)
            .await
    }

    pub async fn retry_referral_reward(
        &self,
        reward_id: &str,
        operator_id: Option<&str>,
        note: Option<&str>,
    ) -> Result<Option<ReferralRewardRecord>, DataLayerError> {
        let Some(reward) = self.find_referral_reward(reward_id).await? else {
            return Ok(None);
        };
        if !referral_retry_allowed(&reward.status) {
            return Err(DataLayerError::InvalidInput(
                "仅失败返利可以补发".to_string(),
            ));
        }
        let rewards = self
            .credit_pending_referral_rewards(&[reward.idempotency_key], operator_id, note)
            .await?;
        Ok(rewards.into_iter().next())
    }

    pub async fn void_referral_reward(
        &self,
        reward_id: &str,
        operator_id: Option<&str>,
        note: Option<&str>,
    ) -> Result<Option<ReferralRewardRecord>, DataLayerError> {
        let Some(reward) = self.find_referral_reward(reward_id).await? else {
            return Ok(None);
        };
        if !referral_void_allowed(&reward.status) {
            return Err(DataLayerError::InvalidInput(
                "仅待发或失败返利可以作废".to_string(),
            ));
        }
        self.update_referral_reward_status(reward_id, "voided", operator_id, note)
            .await?;
        self.find_referral_reward(reward_id).await
    }

    pub async fn reverse_referral_rewards_for_order(
        &self,
        order_id: &str,
        amount_usd: f64,
    ) -> Result<Vec<ReferralRewardRecord>, DataLayerError> {
        if amount_usd <= 0.0 {
            return Ok(Vec::new());
        }
        let Some(refund_context) = self
            .find_referral_payment_order_refund_context(order_id)
            .await?
        else {
            return Ok(Vec::new());
        };
        let rewards = self
            .find_applied_referral_rewards_by_order(order_id)
            .await?;
        let mut reversed = Vec::new();
        for reward in rewards {
            let reversal_amount = referral_reversal_delta(
                reward.amount_usd,
                refund_context.amount_usd,
                refund_context.refunded_amount_usd,
                reward.reversed_amount_usd,
                reward.pending_reversal_amount_usd,
            );
            if reversal_amount <= 0.0 {
                continue;
            }
            let target_reversal = referral_reversal_target(
                reward.amount_usd,
                refund_context.amount_usd,
                refund_context.refunded_amount_usd,
            );
            self.apply_referral_reward_reversal(&reward, target_reversal)
                .await?;
            if let Some(updated) = self.find_referral_reward(&reward.id).await? {
                reversed.push(updated);
            }
        }
        Ok(reversed)
    }
}

impl ReferralDataState<'_> {
    async fn ensure_referral_invite_code(
        &self,
        user_id: &str,
    ) -> Result<Option<String>, DataLayerError> {
        if let Some(existing) = self.find_referral_invite_code(user_id).await? {
            return Ok(Some(existing));
        }
        let Some(backends) = self.backends.as_ref() else {
            return Ok(None);
        };
        for _ in 0..5 {
            let code = generate_invite_code();
            let mut inserted = 0;
            #[cfg(feature = "postgres")]
            if let Some(backend) = backends.postgres() {
                inserted = sqlx::query(
                    r#"
INSERT INTO user_invite_codes (user_id, invite_code, active, created_at, updated_at)
VALUES ($1, $2, TRUE, NOW(), NOW())
ON CONFLICT DO NOTHING
"#,
                )
                .bind(user_id)
                .bind(&code)
                .execute(&backend.pool_clone())
                .await
                .map_err(DataLayerError::postgres)?
                .rows_affected();
            }
            #[cfg(feature = "mysql")]
            if inserted == 0 {
                if let Some(backend) = backends.mysql() {
                    inserted = sqlx::query(
                        r#"
INSERT IGNORE INTO user_invite_codes (user_id, invite_code, active, created_at, updated_at)
VALUES (?, ?, TRUE, ?, ?)
"#,
                    )
                    .bind(user_id)
                    .bind(&code)
                    .bind(now_unix_secs() as i64)
                    .bind(now_unix_secs() as i64)
                    .execute(&backend.pool_clone())
                    .await
                    .map_err(DataLayerError::sql)?
                    .rows_affected();
                }
            }
            #[cfg(feature = "sqlite")]
            if inserted == 0 {
                if let Some(backend) = backends.sqlite() {
                    inserted = sqlx::query(
                        r#"
INSERT OR IGNORE INTO user_invite_codes (user_id, invite_code, active, created_at, updated_at)
VALUES (?, ?, 1, ?, ?)
"#,
                    )
                    .bind(user_id)
                    .bind(&code)
                    .bind(now_unix_secs() as i64)
                    .bind(now_unix_secs() as i64)
                    .execute(&backend.pool_clone())
                    .await
                    .map_err(DataLayerError::sql)?
                    .rows_affected();
                }
            }
            if inserted > 0 {
                return Ok(Some(code));
            }
            if let Some(existing) = self.find_referral_invite_code(user_id).await? {
                return Ok(Some(existing));
            }
        }
        self.find_referral_invite_code(user_id).await
    }

    async fn find_referral_invite_code(
        &self,
        user_id: &str,
    ) -> Result<Option<String>, DataLayerError> {
        let Some(backends) = self.backends.as_ref() else {
            return Ok(None);
        };
        #[cfg(feature = "postgres")]
        if let Some(backend) = backends.postgres() {
            let row = sqlx::query(
                "SELECT invite_code FROM user_invite_codes WHERE user_id = $1 AND active = TRUE",
            )
            .bind(user_id)
            .fetch_optional(&backend.pool_clone())
            .await
            .map_err(DataLayerError::postgres)?;
            return row
                .map(|row| {
                    row.try_get::<String, _>("invite_code")
                        .map_err(DataLayerError::sql)
                })
                .transpose();
        }
        #[cfg(feature = "mysql")]
        if let Some(backend) = backends.mysql() {
            let row = sqlx::query(
                "SELECT invite_code FROM user_invite_codes WHERE user_id = ? AND active = TRUE",
            )
            .bind(user_id)
            .fetch_optional(&backend.pool_clone())
            .await
            .map_err(DataLayerError::sql)?;
            return row
                .map(|row| {
                    row.try_get::<String, _>("invite_code")
                        .map_err(DataLayerError::sql)
                })
                .transpose();
        }
        #[cfg(feature = "sqlite")]
        if let Some(backend) = backends.sqlite() {
            let row = sqlx::query(
                "SELECT invite_code FROM user_invite_codes WHERE user_id = ? AND active = 1",
            )
            .bind(user_id)
            .fetch_optional(&backend.pool_clone())
            .await
            .map_err(DataLayerError::sql)?;
            return row
                .map(|row| {
                    row.try_get::<String, _>("invite_code")
                        .map_err(DataLayerError::sql)
                })
                .transpose();
        }
        Ok(None)
    }

    async fn find_referral_inviter_by_code(
        &self,
        invite_code: &str,
    ) -> Result<Option<String>, DataLayerError> {
        let Some(backends) = self.backends.as_ref() else {
            return Ok(None);
        };
        #[cfg(feature = "postgres")]
        if let Some(backend) = backends.postgres() {
            let row = sqlx::query(
                "SELECT user_id FROM user_invite_codes WHERE invite_code = $1 AND active = TRUE",
            )
            .bind(invite_code)
            .fetch_optional(&backend.pool_clone())
            .await
            .map_err(DataLayerError::postgres)?;
            return row
                .map(|row| {
                    row.try_get::<String, _>("user_id")
                        .map_err(DataLayerError::sql)
                })
                .transpose();
        }
        #[cfg(feature = "mysql")]
        if let Some(backend) = backends.mysql() {
            let row = sqlx::query(
                "SELECT user_id FROM user_invite_codes WHERE invite_code = ? AND active = TRUE",
            )
            .bind(invite_code)
            .fetch_optional(&backend.pool_clone())
            .await
            .map_err(DataLayerError::sql)?;
            return row
                .map(|row| {
                    row.try_get::<String, _>("user_id")
                        .map_err(DataLayerError::sql)
                })
                .transpose();
        }
        #[cfg(feature = "sqlite")]
        if let Some(backend) = backends.sqlite() {
            let row = sqlx::query(
                "SELECT user_id FROM user_invite_codes WHERE invite_code = ? AND active = 1",
            )
            .bind(invite_code)
            .fetch_optional(&backend.pool_clone())
            .await
            .map_err(DataLayerError::sql)?;
            return row
                .map(|row| {
                    row.try_get::<String, _>("user_id")
                        .map_err(DataLayerError::sql)
                })
                .transpose();
        }
        Ok(None)
    }

    async fn insert_referral_relationship(
        &self,
        referral_id: &str,
        inviter_user_id: &str,
        invitee_user_id: &str,
        invite_code: &str,
        source_json: Option<&str>,
    ) -> Result<bool, DataLayerError> {
        let Some(backends) = self.backends.as_ref() else {
            return Ok(false);
        };
        #[cfg(feature = "postgres")]
        if let Some(backend) = backends.postgres() {
            let affected = sqlx::query(
                r#"
INSERT INTO user_referrals (
  id, inviter_user_id, invitee_user_id, invite_code_snapshot, source_json, created_at, updated_at
)
VALUES ($1, $2, $3, $4, $5::jsonb, NOW(), NOW())
ON CONFLICT (invitee_user_id) DO NOTHING
"#,
            )
            .bind(referral_id)
            .bind(inviter_user_id)
            .bind(invitee_user_id)
            .bind(invite_code)
            .bind(source_json)
            .execute(&backend.pool_clone())
            .await
            .map_err(DataLayerError::postgres)?
            .rows_affected();
            return Ok(affected > 0);
        }
        #[cfg(feature = "mysql")]
        if let Some(backend) = backends.mysql() {
            let affected = sqlx::query(
                r#"
INSERT IGNORE INTO user_referrals (
  id, inviter_user_id, invitee_user_id, invite_code_snapshot, source_json, created_at, updated_at
)
VALUES (?, ?, ?, ?, ?, ?, ?)
"#,
            )
            .bind(referral_id)
            .bind(inviter_user_id)
            .bind(invitee_user_id)
            .bind(invite_code)
            .bind(source_json)
            .bind(now_unix_secs() as i64)
            .bind(now_unix_secs() as i64)
            .execute(&backend.pool_clone())
            .await
            .map_err(DataLayerError::sql)?
            .rows_affected();
            return Ok(affected > 0);
        }
        #[cfg(feature = "sqlite")]
        if let Some(backend) = backends.sqlite() {
            let affected = sqlx::query(
                r#"
INSERT OR IGNORE INTO user_referrals (
  id, inviter_user_id, invitee_user_id, invite_code_snapshot, source_json, created_at, updated_at
)
VALUES (?, ?, ?, ?, ?, ?, ?)
"#,
            )
            .bind(referral_id)
            .bind(inviter_user_id)
            .bind(invitee_user_id)
            .bind(invite_code)
            .bind(source_json)
            .bind(now_unix_secs() as i64)
            .bind(now_unix_secs() as i64)
            .execute(&backend.pool_clone())
            .await
            .map_err(DataLayerError::sql)?
            .rows_affected();
            return Ok(affected > 0);
        }
        Ok(false)
    }

    async fn list_referral_relationships_raw(
        &self,
        inviter_user_id: Option<&str>,
        invitee_user_id: Option<&str>,
    ) -> Result<Vec<ReferralRelationshipRecord>, DataLayerError> {
        let Some(backends) = self.backends.as_ref() else {
            return Ok(Vec::new());
        };
        #[cfg(feature = "postgres")]
        if let Some(backend) = backends.postgres() {
            let rows = sqlx::query(
                r#"
SELECT
  r.id, r.inviter_user_id, inviter.username AS inviter_username,
  r.invitee_user_id, invitee.username AS invitee_username,
  r.invite_code_snapshot, r.first_paid_order_id,
  EXTRACT(EPOCH FROM r.first_paid_at)::BIGINT AS first_paid_at_unix_secs,
  r.source_json::TEXT AS source_json,
  EXTRACT(EPOCH FROM r.created_at)::BIGINT AS created_at_unix_secs
FROM user_referrals r
LEFT JOIN users inviter ON inviter.id = r.inviter_user_id
LEFT JOIN users invitee ON invitee.id = r.invitee_user_id
WHERE ($1::TEXT IS NULL OR r.inviter_user_id = $1)
  AND ($2::TEXT IS NULL OR r.invitee_user_id = $2)
ORDER BY r.created_at DESC
LIMIT $3
"#,
            )
            .bind(inviter_user_id)
            .bind(invitee_user_id)
            .bind(REFERRAL_FETCH_LIMIT as i64)
            .fetch_all(&backend.pool_clone())
            .await
            .map_err(DataLayerError::postgres)?;
            return rows.iter().map(|row| relationship_from_row!(row)).collect();
        }
        #[cfg(feature = "mysql")]
        if let Some(backend) = backends.mysql() {
            let rows = sqlx::query(
                r#"
SELECT
  r.id, r.inviter_user_id, inviter.username AS inviter_username,
  r.invitee_user_id, invitee.username AS invitee_username,
  r.invite_code_snapshot, r.first_paid_order_id,
  r.first_paid_at AS first_paid_at_unix_secs,
  r.source_json AS source_json,
  r.created_at AS created_at_unix_secs
FROM user_referrals r
LEFT JOIN users inviter ON inviter.id = r.inviter_user_id
LEFT JOIN users invitee ON invitee.id = r.invitee_user_id
WHERE (? IS NULL OR r.inviter_user_id = ?)
  AND (? IS NULL OR r.invitee_user_id = ?)
ORDER BY r.created_at DESC
LIMIT ?
"#,
            )
            .bind(inviter_user_id)
            .bind(inviter_user_id)
            .bind(invitee_user_id)
            .bind(invitee_user_id)
            .bind(REFERRAL_FETCH_LIMIT as i64)
            .fetch_all(&backend.pool_clone())
            .await
            .map_err(DataLayerError::sql)?;
            return rows.iter().map(|row| relationship_from_row!(row)).collect();
        }
        #[cfg(feature = "sqlite")]
        if let Some(backend) = backends.sqlite() {
            let rows = sqlx::query(
                r#"
SELECT
  r.id, r.inviter_user_id, inviter.username AS inviter_username,
  r.invitee_user_id, invitee.username AS invitee_username,
  r.invite_code_snapshot, r.first_paid_order_id,
  r.first_paid_at AS first_paid_at_unix_secs,
  r.source_json AS source_json,
  r.created_at AS created_at_unix_secs
FROM user_referrals r
LEFT JOIN users inviter ON inviter.id = r.inviter_user_id
LEFT JOIN users invitee ON invitee.id = r.invitee_user_id
WHERE (? IS NULL OR r.inviter_user_id = ?)
  AND (? IS NULL OR r.invitee_user_id = ?)
ORDER BY r.created_at DESC
LIMIT ?
"#,
            )
            .bind(inviter_user_id)
            .bind(inviter_user_id)
            .bind(invitee_user_id)
            .bind(invitee_user_id)
            .bind(REFERRAL_FETCH_LIMIT as i64)
            .fetch_all(&backend.pool_clone())
            .await
            .map_err(DataLayerError::sql)?;
            return rows.iter().map(|row| relationship_from_row!(row)).collect();
        }
        Ok(Vec::new())
    }

    async fn find_referral_relationship(
        &self,
        referral_id: &str,
    ) -> Result<Option<ReferralRelationshipRecord>, DataLayerError> {
        let Some(backends) = self.backends.as_ref() else {
            return Ok(None);
        };
        #[cfg(feature = "postgres")]
        if let Some(backend) = backends.postgres() {
            let row = sqlx::query(
                r#"
SELECT
  r.id, r.inviter_user_id, inviter.username AS inviter_username,
  r.invitee_user_id, invitee.username AS invitee_username,
  r.invite_code_snapshot, r.first_paid_order_id,
  EXTRACT(EPOCH FROM r.first_paid_at)::BIGINT AS first_paid_at_unix_secs,
  r.source_json::TEXT AS source_json,
  EXTRACT(EPOCH FROM r.created_at)::BIGINT AS created_at_unix_secs
FROM user_referrals r
LEFT JOIN users inviter ON inviter.id = r.inviter_user_id
LEFT JOIN users invitee ON invitee.id = r.invitee_user_id
WHERE r.id = $1
LIMIT 1
"#,
            )
            .bind(referral_id)
            .fetch_optional(&backend.pool_clone())
            .await
            .map_err(DataLayerError::postgres)?;
            return row.map(|row| relationship_from_row!(&row)).transpose();
        }
        #[cfg(feature = "mysql")]
        if let Some(backend) = backends.mysql() {
            let row = sqlx::query(
                r#"
SELECT
  r.id, r.inviter_user_id, inviter.username AS inviter_username,
  r.invitee_user_id, invitee.username AS invitee_username,
  r.invite_code_snapshot, r.first_paid_order_id,
  r.first_paid_at AS first_paid_at_unix_secs,
  r.source_json AS source_json,
  r.created_at AS created_at_unix_secs
FROM user_referrals r
LEFT JOIN users inviter ON inviter.id = r.inviter_user_id
LEFT JOIN users invitee ON invitee.id = r.invitee_user_id
WHERE r.id = ?
LIMIT 1
"#,
            )
            .bind(referral_id)
            .fetch_optional(&backend.pool_clone())
            .await
            .map_err(DataLayerError::sql)?;
            return row.map(|row| relationship_from_row!(&row)).transpose();
        }
        #[cfg(feature = "sqlite")]
        if let Some(backend) = backends.sqlite() {
            let row = sqlx::query(
                r#"
SELECT
  r.id, r.inviter_user_id, inviter.username AS inviter_username,
  r.invitee_user_id, invitee.username AS invitee_username,
  r.invite_code_snapshot, r.first_paid_order_id,
  r.first_paid_at AS first_paid_at_unix_secs,
  r.source_json AS source_json,
  r.created_at AS created_at_unix_secs
FROM user_referrals r
LEFT JOIN users inviter ON inviter.id = r.inviter_user_id
LEFT JOIN users invitee ON invitee.id = r.invitee_user_id
WHERE r.id = ?
LIMIT 1
"#,
            )
            .bind(referral_id)
            .fetch_optional(&backend.pool_clone())
            .await
            .map_err(DataLayerError::sql)?;
            return row.map(|row| relationship_from_row!(&row)).transpose();
        }
        Ok(None)
    }

    async fn find_referral_relationship_by_invitee(
        &self,
        invitee_user_id: &str,
    ) -> Result<Option<ReferralRelationshipRecord>, DataLayerError> {
        let Some(backends) = self.backends.as_ref() else {
            return Ok(None);
        };
        #[cfg(feature = "postgres")]
        if let Some(backend) = backends.postgres() {
            let row = sqlx::query(
                r#"
SELECT
  r.id, r.inviter_user_id, inviter.username AS inviter_username,
  r.invitee_user_id, invitee.username AS invitee_username,
  r.invite_code_snapshot, r.first_paid_order_id,
  EXTRACT(EPOCH FROM r.first_paid_at)::BIGINT AS first_paid_at_unix_secs,
  r.source_json::TEXT AS source_json,
  EXTRACT(EPOCH FROM r.created_at)::BIGINT AS created_at_unix_secs
FROM user_referrals r
LEFT JOIN users inviter ON inviter.id = r.inviter_user_id
LEFT JOIN users invitee ON invitee.id = r.invitee_user_id
WHERE r.invitee_user_id = $1
LIMIT 1
"#,
            )
            .bind(invitee_user_id)
            .fetch_optional(&backend.pool_clone())
            .await
            .map_err(DataLayerError::postgres)?;
            return row.map(|row| relationship_from_row!(&row)).transpose();
        }
        #[cfg(feature = "mysql")]
        if let Some(backend) = backends.mysql() {
            let row = sqlx::query(
                r#"
SELECT
  r.id, r.inviter_user_id, inviter.username AS inviter_username,
  r.invitee_user_id, invitee.username AS invitee_username,
  r.invite_code_snapshot, r.first_paid_order_id,
  r.first_paid_at AS first_paid_at_unix_secs,
  r.source_json AS source_json,
  r.created_at AS created_at_unix_secs
FROM user_referrals r
LEFT JOIN users inviter ON inviter.id = r.inviter_user_id
LEFT JOIN users invitee ON invitee.id = r.invitee_user_id
WHERE r.invitee_user_id = ?
LIMIT 1
"#,
            )
            .bind(invitee_user_id)
            .fetch_optional(&backend.pool_clone())
            .await
            .map_err(DataLayerError::sql)?;
            return row.map(|row| relationship_from_row!(&row)).transpose();
        }
        #[cfg(feature = "sqlite")]
        if let Some(backend) = backends.sqlite() {
            let row = sqlx::query(
                r#"
SELECT
  r.id, r.inviter_user_id, inviter.username AS inviter_username,
  r.invitee_user_id, invitee.username AS invitee_username,
  r.invite_code_snapshot, r.first_paid_order_id,
  r.first_paid_at AS first_paid_at_unix_secs,
  r.source_json AS source_json,
  r.created_at AS created_at_unix_secs
FROM user_referrals r
LEFT JOIN users inviter ON inviter.id = r.inviter_user_id
LEFT JOIN users invitee ON invitee.id = r.invitee_user_id
WHERE r.invitee_user_id = ?
LIMIT 1
"#,
            )
            .bind(invitee_user_id)
            .fetch_optional(&backend.pool_clone())
            .await
            .map_err(DataLayerError::sql)?;
            return row.map(|row| relationship_from_row!(&row)).transpose();
        }
        Ok(None)
    }

    async fn list_referral_rewards_raw(
        &self,
        inviter_user_id: Option<&str>,
    ) -> Result<Vec<ReferralRewardRecord>, DataLayerError> {
        let Some(backends) = self.backends.as_ref() else {
            return Ok(Vec::new());
        };
        #[cfg(feature = "postgres")]
        if let Some(backend) = backends.postgres() {
            let rows = sqlx::query(
                r#"
SELECT
  id, referral_id, inviter_user_id, invitee_user_id, reward_type, source_order_id,
  trigger_point, amount_usd, status, wallet_transaction_id, idempotency_key,
  reversed_amount_usd, pending_reversal_amount_usd, admin_operator_id, admin_note,
  EXTRACT(EPOCH FROM created_at)::BIGINT AS created_at_unix_secs,
  EXTRACT(EPOCH FROM updated_at)::BIGINT AS updated_at_unix_secs
FROM referral_rewards
WHERE ($1::TEXT IS NULL OR inviter_user_id = $1)
ORDER BY created_at DESC
LIMIT $2
"#,
            )
            .bind(inviter_user_id)
            .bind(REFERRAL_FETCH_LIMIT as i64)
            .fetch_all(&backend.pool_clone())
            .await
            .map_err(DataLayerError::postgres)?;
            return rows.iter().map(|row| reward_from_row!(row)).collect();
        }
        #[cfg(feature = "mysql")]
        if let Some(backend) = backends.mysql() {
            let rows = sqlx::query(
                r#"
SELECT
  id, referral_id, inviter_user_id, invitee_user_id, reward_type, source_order_id,
  trigger_point, amount_usd, status, wallet_transaction_id, idempotency_key,
  reversed_amount_usd, pending_reversal_amount_usd, admin_operator_id, admin_note,
  created_at AS created_at_unix_secs, updated_at AS updated_at_unix_secs
FROM referral_rewards
WHERE (? IS NULL OR inviter_user_id = ?)
ORDER BY created_at DESC
LIMIT ?
"#,
            )
            .bind(inviter_user_id)
            .bind(inviter_user_id)
            .bind(REFERRAL_FETCH_LIMIT as i64)
            .fetch_all(&backend.pool_clone())
            .await
            .map_err(DataLayerError::sql)?;
            return rows.iter().map(|row| reward_from_row!(row)).collect();
        }
        #[cfg(feature = "sqlite")]
        if let Some(backend) = backends.sqlite() {
            let rows = sqlx::query(
                r#"
SELECT
  id, referral_id, inviter_user_id, invitee_user_id, reward_type, source_order_id,
  trigger_point, amount_usd, status, wallet_transaction_id, idempotency_key,
  reversed_amount_usd, pending_reversal_amount_usd, admin_operator_id, admin_note,
  created_at AS created_at_unix_secs, updated_at AS updated_at_unix_secs
FROM referral_rewards
WHERE (? IS NULL OR inviter_user_id = ?)
ORDER BY created_at DESC
LIMIT ?
"#,
            )
            .bind(inviter_user_id)
            .bind(inviter_user_id)
            .bind(REFERRAL_FETCH_LIMIT as i64)
            .fetch_all(&backend.pool_clone())
            .await
            .map_err(DataLayerError::sql)?;
            return rows.iter().map(|row| reward_from_row!(row)).collect();
        }
        Ok(Vec::new())
    }

    async fn find_referral_reward(
        &self,
        reward_id: &str,
    ) -> Result<Option<ReferralRewardRecord>, DataLayerError> {
        let Some(backends) = self.backends.as_ref() else {
            return Ok(None);
        };
        #[cfg(feature = "postgres")]
        if let Some(backend) = backends.postgres() {
            let row = sqlx::query(
                r#"
SELECT
  id, referral_id, inviter_user_id, invitee_user_id, reward_type, source_order_id,
  trigger_point, amount_usd, status, wallet_transaction_id, idempotency_key,
  reversed_amount_usd, pending_reversal_amount_usd, admin_operator_id, admin_note,
  EXTRACT(EPOCH FROM created_at)::BIGINT AS created_at_unix_secs,
  EXTRACT(EPOCH FROM updated_at)::BIGINT AS updated_at_unix_secs
FROM referral_rewards
WHERE id = $1
LIMIT 1
"#,
            )
            .bind(reward_id)
            .fetch_optional(&backend.pool_clone())
            .await
            .map_err(DataLayerError::postgres)?;
            return row.map(|row| reward_from_row!(&row)).transpose();
        }
        #[cfg(feature = "mysql")]
        if let Some(backend) = backends.mysql() {
            let row = sqlx::query(
                r#"
SELECT
  id, referral_id, inviter_user_id, invitee_user_id, reward_type, source_order_id,
  trigger_point, amount_usd, status, wallet_transaction_id, idempotency_key,
  reversed_amount_usd, pending_reversal_amount_usd, admin_operator_id, admin_note,
  created_at AS created_at_unix_secs, updated_at AS updated_at_unix_secs
FROM referral_rewards
WHERE id = ?
LIMIT 1
"#,
            )
            .bind(reward_id)
            .fetch_optional(&backend.pool_clone())
            .await
            .map_err(DataLayerError::sql)?;
            return row.map(|row| reward_from_row!(&row)).transpose();
        }
        #[cfg(feature = "sqlite")]
        if let Some(backend) = backends.sqlite() {
            let row = sqlx::query(
                r#"
SELECT
  id, referral_id, inviter_user_id, invitee_user_id, reward_type, source_order_id,
  trigger_point, amount_usd, status, wallet_transaction_id, idempotency_key,
  reversed_amount_usd, pending_reversal_amount_usd, admin_operator_id, admin_note,
  created_at AS created_at_unix_secs, updated_at AS updated_at_unix_secs
FROM referral_rewards
WHERE id = ?
LIMIT 1
"#,
            )
            .bind(reward_id)
            .fetch_optional(&backend.pool_clone())
            .await
            .map_err(DataLayerError::sql)?;
            return row.map(|row| reward_from_row!(&row)).transpose();
        }
        Ok(None)
    }

    async fn find_referral_reward_by_idempotency_key(
        &self,
        idempotency_key: &str,
    ) -> Result<Option<ReferralRewardRecord>, DataLayerError> {
        let Some(backends) = self.backends.as_ref() else {
            return Ok(None);
        };
        #[cfg(feature = "postgres")]
        if let Some(backend) = backends.postgres() {
            let row = sqlx::query(
                r#"
SELECT
  id, referral_id, inviter_user_id, invitee_user_id, reward_type, source_order_id,
  trigger_point, amount_usd, status, wallet_transaction_id, idempotency_key,
  reversed_amount_usd, pending_reversal_amount_usd, admin_operator_id, admin_note,
  EXTRACT(EPOCH FROM created_at)::BIGINT AS created_at_unix_secs,
  EXTRACT(EPOCH FROM updated_at)::BIGINT AS updated_at_unix_secs
FROM referral_rewards
WHERE idempotency_key = $1
LIMIT 1
"#,
            )
            .bind(idempotency_key)
            .fetch_optional(&backend.pool_clone())
            .await
            .map_err(DataLayerError::postgres)?;
            return row.map(|row| reward_from_row!(&row)).transpose();
        }
        #[cfg(feature = "mysql")]
        if let Some(backend) = backends.mysql() {
            let row = sqlx::query(
                r#"
SELECT
  id, referral_id, inviter_user_id, invitee_user_id, reward_type, source_order_id,
  trigger_point, amount_usd, status, wallet_transaction_id, idempotency_key,
  reversed_amount_usd, pending_reversal_amount_usd, admin_operator_id, admin_note,
  created_at AS created_at_unix_secs, updated_at AS updated_at_unix_secs
FROM referral_rewards
WHERE idempotency_key = ?
LIMIT 1
"#,
            )
            .bind(idempotency_key)
            .fetch_optional(&backend.pool_clone())
            .await
            .map_err(DataLayerError::sql)?;
            return row.map(|row| reward_from_row!(&row)).transpose();
        }
        #[cfg(feature = "sqlite")]
        if let Some(backend) = backends.sqlite() {
            let row = sqlx::query(
                r#"
SELECT
  id, referral_id, inviter_user_id, invitee_user_id, reward_type, source_order_id,
  trigger_point, amount_usd, status, wallet_transaction_id, idempotency_key,
  reversed_amount_usd, pending_reversal_amount_usd, admin_operator_id, admin_note,
  created_at AS created_at_unix_secs, updated_at AS updated_at_unix_secs
FROM referral_rewards
WHERE idempotency_key = ?
LIMIT 1
"#,
            )
            .bind(idempotency_key)
            .fetch_optional(&backend.pool_clone())
            .await
            .map_err(DataLayerError::sql)?;
            return row.map(|row| reward_from_row!(&row)).transpose();
        }
        Ok(None)
    }

    async fn find_applied_referral_rewards_by_order(
        &self,
        order_id: &str,
    ) -> Result<Vec<ReferralRewardRecord>, DataLayerError> {
        let Some(backends) = self.backends.as_ref() else {
            return Ok(Vec::new());
        };
        #[cfg(feature = "postgres")]
        if let Some(backend) = backends.postgres() {
            let rows = sqlx::query(
                r#"
SELECT
  id, referral_id, inviter_user_id, invitee_user_id, reward_type, source_order_id,
  trigger_point, amount_usd, status, wallet_transaction_id, idempotency_key,
  reversed_amount_usd, pending_reversal_amount_usd, admin_operator_id, admin_note,
  EXTRACT(EPOCH FROM created_at)::BIGINT AS created_at_unix_secs,
  EXTRACT(EPOCH FROM updated_at)::BIGINT AS updated_at_unix_secs
FROM referral_rewards
WHERE source_order_id = $1 AND status = 'applied'
ORDER BY created_at ASC
"#,
            )
            .bind(order_id)
            .fetch_all(&backend.pool_clone())
            .await
            .map_err(DataLayerError::postgres)?;
            return rows.iter().map(|row| reward_from_row!(row)).collect();
        }
        #[cfg(feature = "mysql")]
        if let Some(backend) = backends.mysql() {
            let rows = sqlx::query(
                r#"
SELECT
  id, referral_id, inviter_user_id, invitee_user_id, reward_type, source_order_id,
  trigger_point, amount_usd, status, wallet_transaction_id, idempotency_key,
  reversed_amount_usd, pending_reversal_amount_usd, admin_operator_id, admin_note,
  created_at AS created_at_unix_secs, updated_at AS updated_at_unix_secs
FROM referral_rewards
WHERE source_order_id = ? AND status = 'applied'
ORDER BY created_at ASC
"#,
            )
            .bind(order_id)
            .fetch_all(&backend.pool_clone())
            .await
            .map_err(DataLayerError::sql)?;
            return rows.iter().map(|row| reward_from_row!(row)).collect();
        }
        #[cfg(feature = "sqlite")]
        if let Some(backend) = backends.sqlite() {
            let rows = sqlx::query(
                r#"
SELECT
  id, referral_id, inviter_user_id, invitee_user_id, reward_type, source_order_id,
  trigger_point, amount_usd, status, wallet_transaction_id, idempotency_key,
  reversed_amount_usd, pending_reversal_amount_usd, admin_operator_id, admin_note,
  created_at AS created_at_unix_secs, updated_at AS updated_at_unix_secs
FROM referral_rewards
WHERE source_order_id = ? AND status = 'applied'
ORDER BY created_at ASC
"#,
            )
            .bind(order_id)
            .fetch_all(&backend.pool_clone())
            .await
            .map_err(DataLayerError::sql)?;
            return rows.iter().map(|row| reward_from_row!(row)).collect();
        }
        Ok(Vec::new())
    }

    async fn insert_referral_reward(
        &self,
        relationship: &ReferralRelationshipRecord,
        reward_type: &str,
        source_order_id: Option<&str>,
        trigger_point: &str,
        amount_usd: f64,
        idempotency_key: &str,
    ) -> Result<bool, DataLayerError> {
        let Some(backends) = self.backends.as_ref() else {
            return Ok(false);
        };
        let reward_id = uuid::Uuid::new_v4().to_string();
        #[cfg(feature = "postgres")]
        if let Some(backend) = backends.postgres() {
            let affected = sqlx::query(
                r#"
INSERT INTO referral_rewards (
  id, referral_id, inviter_user_id, invitee_user_id, reward_type, source_order_id,
  trigger_point, amount_usd, status, idempotency_key, created_at, updated_at
)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'pending', $9, NOW(), NOW())
ON CONFLICT (idempotency_key) DO NOTHING
"#,
            )
            .bind(&reward_id)
            .bind(&relationship.id)
            .bind(&relationship.inviter_user_id)
            .bind(&relationship.invitee_user_id)
            .bind(reward_type)
            .bind(source_order_id)
            .bind(trigger_point)
            .bind(amount_usd)
            .bind(idempotency_key)
            .execute(&backend.pool_clone())
            .await
            .map_err(DataLayerError::postgres)?
            .rows_affected();
            return Ok(affected > 0);
        }
        #[cfg(feature = "mysql")]
        if let Some(backend) = backends.mysql() {
            let affected = sqlx::query(
                r#"
INSERT IGNORE INTO referral_rewards (
  id, referral_id, inviter_user_id, invitee_user_id, reward_type, source_order_id,
  trigger_point, amount_usd, status, idempotency_key, created_at, updated_at
)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'pending', ?, ?, ?)
"#,
            )
            .bind(&reward_id)
            .bind(&relationship.id)
            .bind(&relationship.inviter_user_id)
            .bind(&relationship.invitee_user_id)
            .bind(reward_type)
            .bind(source_order_id)
            .bind(trigger_point)
            .bind(amount_usd)
            .bind(idempotency_key)
            .bind(now_unix_secs() as i64)
            .bind(now_unix_secs() as i64)
            .execute(&backend.pool_clone())
            .await
            .map_err(DataLayerError::sql)?
            .rows_affected();
            return Ok(affected > 0);
        }
        #[cfg(feature = "sqlite")]
        if let Some(backend) = backends.sqlite() {
            let affected = sqlx::query(
                r#"
INSERT OR IGNORE INTO referral_rewards (
  id, referral_id, inviter_user_id, invitee_user_id, reward_type, source_order_id,
  trigger_point, amount_usd, status, idempotency_key, created_at, updated_at
)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'pending', ?, ?, ?)
"#,
            )
            .bind(&reward_id)
            .bind(&relationship.id)
            .bind(&relationship.inviter_user_id)
            .bind(&relationship.invitee_user_id)
            .bind(reward_type)
            .bind(source_order_id)
            .bind(trigger_point)
            .bind(amount_usd)
            .bind(idempotency_key)
            .bind(now_unix_secs() as i64)
            .bind(now_unix_secs() as i64)
            .execute(&backend.pool_clone())
            .await
            .map_err(DataLayerError::sql)?
            .rows_affected();
            return Ok(affected > 0);
        }
        Ok(false)
    }

    async fn find_referral_payment_order_context(
        &self,
        order_id: &str,
    ) -> Result<Option<ReferralPaymentOrderContext>, DataLayerError> {
        let Some(backends) = self.backends.as_ref() else {
            return Ok(None);
        };
        #[cfg(feature = "postgres")]
        if let Some(backend) = backends.postgres() {
            let row = sqlx::query(
                r#"
SELECT id, user_id, amount_usd, payment_method, status, order_kind
FROM payment_orders
WHERE id = $1
"#,
            )
            .bind(order_id)
            .fetch_optional(&backend.pool_clone())
            .await
            .map_err(DataLayerError::postgres)?;
            return row.map(payment_order_context_from_row).transpose();
        }
        #[cfg(feature = "mysql")]
        if let Some(backend) = backends.mysql() {
            let row = sqlx::query(
                r#"
SELECT id, user_id, amount_usd, payment_method, status, order_kind
FROM payment_orders
WHERE id = ?
"#,
            )
            .bind(order_id)
            .fetch_optional(&backend.pool_clone())
            .await
            .map_err(DataLayerError::sql)?;
            return row.map(payment_order_context_from_row).transpose();
        }
        #[cfg(feature = "sqlite")]
        if let Some(backend) = backends.sqlite() {
            let row = sqlx::query(
                r#"
SELECT id, user_id, amount_usd, payment_method, status, order_kind
FROM payment_orders
WHERE id = ?
"#,
            )
            .bind(order_id)
            .fetch_optional(&backend.pool_clone())
            .await
            .map_err(DataLayerError::sql)?;
            return row.map(payment_order_context_from_row).transpose();
        }
        Ok(None)
    }

    async fn find_referral_payment_order_refund_context(
        &self,
        order_id: &str,
    ) -> Result<Option<ReferralPaymentOrderRefundContext>, DataLayerError> {
        let Some(backends) = self.backends.as_ref() else {
            return Ok(None);
        };
        #[cfg(feature = "postgres")]
        if let Some(backend) = backends.postgres() {
            let row = sqlx::query(
                r#"
SELECT amount_usd, refunded_amount_usd
FROM payment_orders
WHERE id = $1
"#,
            )
            .bind(order_id)
            .fetch_optional(&backend.pool_clone())
            .await
            .map_err(DataLayerError::postgres)?;
            return row.map(payment_order_refund_context_from_row).transpose();
        }
        #[cfg(feature = "mysql")]
        if let Some(backend) = backends.mysql() {
            let row = sqlx::query(
                r#"
SELECT amount_usd, refunded_amount_usd
FROM payment_orders
WHERE id = ?
"#,
            )
            .bind(order_id)
            .fetch_optional(&backend.pool_clone())
            .await
            .map_err(DataLayerError::sql)?;
            return row.map(payment_order_refund_context_from_row).transpose();
        }
        #[cfg(feature = "sqlite")]
        if let Some(backend) = backends.sqlite() {
            let row = sqlx::query(
                r#"
SELECT amount_usd, refunded_amount_usd
FROM payment_orders
WHERE id = ?
"#,
            )
            .bind(order_id)
            .fetch_optional(&backend.pool_clone())
            .await
            .map_err(DataLayerError::sql)?;
            return row.map(payment_order_refund_context_from_row).transpose();
        }
        Ok(None)
    }

    async fn mark_referral_first_paid_order(
        &self,
        referral_id: &str,
        order_id: &str,
    ) -> Result<bool, DataLayerError> {
        let Some(backends) = self.backends.as_ref() else {
            return Ok(false);
        };
        #[cfg(feature = "postgres")]
        if let Some(backend) = backends.postgres() {
            let affected = sqlx::query(
                r#"
UPDATE user_referrals
SET first_paid_order_id = $2,
    first_paid_at = NOW(),
    updated_at = NOW()
WHERE id = $1 AND first_paid_order_id IS NULL
"#,
            )
            .bind(referral_id)
            .bind(order_id)
            .execute(&backend.pool_clone())
            .await
            .map_err(DataLayerError::postgres)?
            .rows_affected();
            return Ok(affected > 0);
        }
        #[cfg(feature = "mysql")]
        if let Some(backend) = backends.mysql() {
            let affected = sqlx::query(
                r#"
UPDATE user_referrals
SET first_paid_order_id = ?,
    first_paid_at = ?,
    updated_at = ?
WHERE id = ? AND first_paid_order_id IS NULL
"#,
            )
            .bind(order_id)
            .bind(now_unix_secs() as i64)
            .bind(now_unix_secs() as i64)
            .bind(referral_id)
            .execute(&backend.pool_clone())
            .await
            .map_err(DataLayerError::sql)?
            .rows_affected();
            return Ok(affected > 0);
        }
        #[cfg(feature = "sqlite")]
        if let Some(backend) = backends.sqlite() {
            let affected = sqlx::query(
                r#"
UPDATE user_referrals
SET first_paid_order_id = ?,
    first_paid_at = ?,
    updated_at = ?
WHERE id = ? AND first_paid_order_id IS NULL
"#,
            )
            .bind(order_id)
            .bind(now_unix_secs() as i64)
            .bind(now_unix_secs() as i64)
            .bind(referral_id)
            .execute(&backend.pool_clone())
            .await
            .map_err(DataLayerError::sql)?
            .rows_affected();
            return Ok(affected > 0);
        }
        Ok(false)
    }

    async fn update_referral_reward_status(
        &self,
        reward_id: &str,
        status: &str,
        operator_id: Option<&str>,
        note: Option<&str>,
    ) -> Result<bool, DataLayerError> {
        let Some(backends) = self.backends.as_ref() else {
            return Ok(false);
        };
        #[cfg(feature = "postgres")]
        if let Some(backend) = backends.postgres() {
            let affected = sqlx::query(
                r#"
UPDATE referral_rewards
SET status = $2,
    admin_operator_id = COALESCE($3, admin_operator_id),
    admin_note = COALESCE($4, admin_note),
    updated_at = NOW()
WHERE id = $1 AND status IN ('pending', 'failed')
"#,
            )
            .bind(reward_id)
            .bind(status)
            .bind(operator_id)
            .bind(note)
            .execute(&backend.pool_clone())
            .await
            .map_err(DataLayerError::postgres)?
            .rows_affected();
            return Ok(affected > 0);
        }
        #[cfg(feature = "mysql")]
        if let Some(backend) = backends.mysql() {
            let affected = sqlx::query(
                r#"
UPDATE referral_rewards
SET status = ?,
    admin_operator_id = COALESCE(?, admin_operator_id),
    admin_note = COALESCE(?, admin_note),
    updated_at = ?
WHERE id = ? AND status IN ('pending', 'failed')
"#,
            )
            .bind(status)
            .bind(operator_id)
            .bind(note)
            .bind(now_unix_secs() as i64)
            .bind(reward_id)
            .execute(&backend.pool_clone())
            .await
            .map_err(DataLayerError::sql)?
            .rows_affected();
            return Ok(affected > 0);
        }
        #[cfg(feature = "sqlite")]
        if let Some(backend) = backends.sqlite() {
            let affected = sqlx::query(
                r#"
UPDATE referral_rewards
SET status = ?,
    admin_operator_id = COALESCE(?, admin_operator_id),
    admin_note = COALESCE(?, admin_note),
    updated_at = ?
WHERE id = ? AND status IN ('pending', 'failed')
"#,
            )
            .bind(status)
            .bind(operator_id)
            .bind(note)
            .bind(now_unix_secs() as i64)
            .bind(reward_id)
            .execute(&backend.pool_clone())
            .await
            .map_err(DataLayerError::sql)?
            .rows_affected();
            return Ok(affected > 0);
        }
        Ok(false)
    }

    async fn credit_pending_referral_rewards(
        &self,
        idempotency_keys: &[String],
        operator_id: Option<&str>,
        note: Option<&str>,
    ) -> Result<Vec<ReferralRewardRecord>, DataLayerError> {
        let mut credited = Vec::new();
        for key in idempotency_keys {
            if let Some(target) = self.referral_credit_target_by_key(key).await? {
                self.credit_referral_reward(target, operator_id, note)
                    .await?;
                if let Some(updated) = self.find_referral_reward_by_idempotency_key(key).await? {
                    credited.push(updated);
                }
            }
        }
        Ok(credited)
    }

    async fn referral_credit_target_by_key(
        &self,
        idempotency_key: &str,
    ) -> Result<Option<ReferralCreditTarget>, DataLayerError> {
        let Some(backends) = self.backends.as_ref() else {
            return Ok(None);
        };
        #[cfg(feature = "postgres")]
        if let Some(backend) = backends.postgres() {
            let row = sqlx::query(
                r#"
SELECT
  rw.id, rw.inviter_user_id, rw.invitee_user_id, rw.amount_usd, rw.reward_type,
  rw.trigger_point, wallets.id AS wallet_id
FROM referral_rewards rw
JOIN wallets ON wallets.user_id = rw.inviter_user_id
WHERE rw.idempotency_key = $1
  AND rw.status IN ('pending', 'failed')
"#,
            )
            .bind(idempotency_key)
            .fetch_optional(&backend.pool_clone())
            .await
            .map_err(DataLayerError::postgres)?;
            return row.map(credit_target_from_row).transpose();
        }
        #[cfg(feature = "mysql")]
        if let Some(backend) = backends.mysql() {
            let row = sqlx::query(
                r#"
SELECT
  rw.id, rw.inviter_user_id, rw.invitee_user_id, rw.amount_usd, rw.reward_type,
  rw.trigger_point, wallets.id AS wallet_id
FROM referral_rewards rw
JOIN wallets ON wallets.user_id = rw.inviter_user_id
WHERE rw.idempotency_key = ?
  AND rw.status IN ('pending', 'failed')
"#,
            )
            .bind(idempotency_key)
            .fetch_optional(&backend.pool_clone())
            .await
            .map_err(DataLayerError::sql)?;
            return row.map(credit_target_from_row).transpose();
        }
        #[cfg(feature = "sqlite")]
        if let Some(backend) = backends.sqlite() {
            let row = sqlx::query(
                r#"
SELECT
  rw.id, rw.inviter_user_id, rw.invitee_user_id, rw.amount_usd, rw.reward_type,
  rw.trigger_point, wallets.id AS wallet_id
FROM referral_rewards rw
JOIN wallets ON wallets.user_id = rw.inviter_user_id
WHERE rw.idempotency_key = ?
  AND rw.status IN ('pending', 'failed')
"#,
            )
            .bind(idempotency_key)
            .fetch_optional(&backend.pool_clone())
            .await
            .map_err(DataLayerError::sql)?;
            return row.map(credit_target_from_row).transpose();
        }
        Ok(None)
    }

    async fn credit_referral_reward(
        &self,
        target: ReferralCreditTarget,
        operator_id: Option<&str>,
        note: Option<&str>,
    ) -> Result<(), DataLayerError> {
        #[cfg(feature = "postgres")]
        if let Some(backend) = self.backends.and_then(DataBackends::postgres) {
            let mut tx = backend
                .pool_clone()
                .begin()
                .await
                .map_err(DataLayerError::postgres)?;
            let claimed = sqlx::query(
                r#"
UPDATE referral_rewards
SET status = 'applying',
    admin_operator_id = COALESCE($2, admin_operator_id),
    admin_note = COALESCE($3, admin_note),
    updated_at = NOW()
WHERE id = $1 AND status IN ('pending', 'failed')
"#,
            )
            .bind(&target.id)
            .bind(operator_id)
            .bind(note)
            .execute(&mut *tx)
            .await
            .map_err(DataLayerError::postgres)?
            .rows_affected();
            if claimed == 0 {
                tx.commit().await.map_err(DataLayerError::postgres)?;
                return Ok(());
            }
            let wallet = sqlx::query(
                r#"
SELECT balance, gift_balance
FROM wallets
WHERE id = $1
FOR UPDATE
"#,
            )
            .bind(&target.wallet_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(DataLayerError::postgres)?;
            let Some(wallet) = wallet else {
                sqlx::query(
                    r#"
UPDATE referral_rewards
SET status = 'failed',
    admin_operator_id = COALESCE($2, admin_operator_id),
    admin_note = COALESCE($3, admin_note),
    updated_at = NOW()
WHERE id = $1
"#,
                )
                .bind(&target.id)
                .bind(operator_id)
                .bind(note.or(Some("邀请人钱包不存在")))
                .execute(&mut *tx)
                .await
                .map_err(DataLayerError::postgres)?;
                tx.commit().await.map_err(DataLayerError::postgres)?;
                return Ok(());
            };
            let balance = row_f64!(wallet, "balance");
            let gift_before = row_f64!(wallet, "gift_balance");
            let gift_after = gift_before + target.amount_usd;
            let tx_id = uuid::Uuid::new_v4().to_string();
            let description = note
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| reward_description(&target));
            sqlx::query(
                r#"
UPDATE wallets
SET gift_balance = $2,
    total_adjusted = total_adjusted + $3,
    updated_at = NOW()
WHERE id = $1
"#,
            )
            .bind(&target.wallet_id)
            .bind(gift_after)
            .bind(target.amount_usd)
            .execute(&mut *tx)
            .await
            .map_err(DataLayerError::postgres)?;
            sqlx::query(
                r#"
INSERT INTO wallet_transactions (
  id, wallet_id, category, reason_code, amount,
  balance_before, balance_after,
  recharge_balance_before, recharge_balance_after,
  gift_balance_before, gift_balance_after,
  link_type, link_id, operator_id, description, created_at
)
VALUES ($1, $2, 'adjust', 'referral_reward', $3, $4, $5, $6, $6, $7, $8,
        'referral_reward', $9, $10, $11, NOW())
"#,
            )
            .bind(&tx_id)
            .bind(&target.wallet_id)
            .bind(target.amount_usd)
            .bind(balance + gift_before)
            .bind(balance + gift_after)
            .bind(balance)
            .bind(gift_before)
            .bind(gift_after)
            .bind(&target.id)
            .bind(operator_id)
            .bind(&description)
            .execute(&mut *tx)
            .await
            .map_err(DataLayerError::postgres)?;
            sqlx::query(
                r#"
UPDATE referral_rewards
SET status = 'applied',
    wallet_transaction_id = $2,
    admin_operator_id = COALESCE($3, admin_operator_id),
    admin_note = COALESCE($4, admin_note),
    updated_at = NOW()
WHERE id = $1
"#,
            )
            .bind(&target.id)
            .bind(&tx_id)
            .bind(operator_id)
            .bind(note)
            .execute(&mut *tx)
            .await
            .map_err(DataLayerError::postgres)?;
            tx.commit().await.map_err(DataLayerError::postgres)?;
            return Ok(());
        }
        #[cfg(feature = "mysql")]
        if let Some(backend) = self.backends.and_then(DataBackends::mysql) {
            self.credit_referral_reward_mysql_numeric_time(
                &backend.pool_clone(),
                target,
                operator_id,
                note,
            )
            .await?;
            return Ok(());
        }
        #[cfg(feature = "sqlite")]
        if let Some(backend) = self.backends.and_then(DataBackends::sqlite) {
            self.credit_referral_reward_sqlite_numeric_time(
                &backend.pool_clone(),
                target,
                operator_id,
                note,
            )
            .await?;
            return Ok(());
        }
        Ok(())
    }

    async fn apply_referral_reward_reversal(
        &self,
        reward: &ReferralRewardRecord,
        target_reversal_amount_usd: f64,
    ) -> Result<(), DataLayerError> {
        #[cfg(feature = "postgres")]
        if let Some(backend) = self.backends.and_then(DataBackends::postgres) {
            let mut tx = backend
                .pool_clone()
                .begin()
                .await
                .map_err(DataLayerError::postgres)?;
            let reward_row = sqlx::query(
                r#"
SELECT reversed_amount_usd, pending_reversal_amount_usd
FROM referral_rewards
WHERE id = $1
FOR UPDATE
"#,
            )
            .bind(&reward.id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(DataLayerError::postgres)?;
            let Some(reward_row) = reward_row else {
                tx.commit().await.map_err(DataLayerError::postgres)?;
                return Ok(());
            };
            let current_reversed = row_f64!(reward_row, "reversed_amount_usd");
            let current_pending = row_f64!(reward_row, "pending_reversal_amount_usd");
            let amount_usd =
                (target_reversal_amount_usd - current_reversed - current_pending).max(0.0);
            if amount_usd <= 0.0 {
                tx.commit().await.map_err(DataLayerError::postgres)?;
                return Ok(());
            }
            let wallet = sqlx::query(
                r#"
SELECT id, balance, gift_balance
FROM wallets
WHERE user_id = $1
FOR UPDATE
"#,
            )
            .bind(&reward.inviter_user_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(DataLayerError::postgres)?;
            let Some(wallet) = wallet else {
                tx.commit().await.map_err(DataLayerError::postgres)?;
                return Ok(());
            };
            let wallet_id = row_string!(wallet, "id");
            let balance = row_f64!(wallet, "balance");
            let gift_before = row_f64!(wallet, "gift_balance");
            let actual_reverse = gift_before.min(amount_usd);
            let pending_reverse = (amount_usd - actual_reverse).max(0.0);
            let gift_after = gift_before - actual_reverse;
            let tx_id = uuid::Uuid::new_v4().to_string();
            if actual_reverse > 0.0 {
                sqlx::query(
                    r#"
UPDATE wallets
SET gift_balance = $2,
    total_adjusted = total_adjusted - $3,
    updated_at = NOW()
WHERE id = $1
"#,
                )
                .bind(&wallet_id)
                .bind(gift_after)
                .bind(actual_reverse)
                .execute(&mut *tx)
                .await
                .map_err(DataLayerError::postgres)?;
                sqlx::query(
                    r#"
INSERT INTO wallet_transactions (
  id, wallet_id, category, reason_code, amount,
  balance_before, balance_after,
  recharge_balance_before, recharge_balance_after,
  gift_balance_before, gift_balance_after,
  link_type, link_id, description, created_at
)
VALUES ($1, $2, 'adjust', 'referral_reward_reversal', $3, $4, $5, $6, $6, $7, $8,
        'referral_reward', $9, '邀请返利退款冲回', NOW())
"#,
                )
                .bind(&tx_id)
                .bind(&wallet_id)
                .bind(-actual_reverse)
                .bind(balance + gift_before)
                .bind(balance + gift_after)
                .bind(balance)
                .bind(gift_before)
                .bind(gift_after)
                .bind(&reward.id)
                .execute(&mut *tx)
                .await
                .map_err(DataLayerError::postgres)?;
            }
            sqlx::query(
                r#"
UPDATE referral_rewards
SET reversed_amount_usd = reversed_amount_usd + $2,
    pending_reversal_amount_usd = pending_reversal_amount_usd + $3,
    status = CASE
      WHEN reversed_amount_usd + $2 >= amount_usd THEN 'reversed'
      ELSE status
    END,
    updated_at = NOW()
WHERE id = $1
"#,
            )
            .bind(&reward.id)
            .bind(actual_reverse)
            .bind(pending_reverse)
            .execute(&mut *tx)
            .await
            .map_err(DataLayerError::postgres)?;
            tx.commit().await.map_err(DataLayerError::postgres)?;
            return Ok(());
        }
        #[cfg(any(feature = "mysql", feature = "sqlite"))]
        {
            // MySQL/SQLite refunds use integer timestamps in the wallet tables.
            return self
                .apply_referral_reward_reversal_numeric_time(reward, target_reversal_amount_usd)
                .await;
        }
        #[cfg(not(any(feature = "mysql", feature = "sqlite")))]
        Ok(())
    }
}

#[cfg(any(feature = "mysql", feature = "sqlite"))]
macro_rules! referral_credit_numeric_method {
    ($name:ident, $pool_ty:ty, $wallet_sql:expr) => {
        async fn $name(
            &self,
            pool: &$pool_ty,
            target: ReferralCreditTarget,
            operator_id: Option<&str>,
            note: Option<&str>,
        ) -> Result<(), DataLayerError> {
            let mut tx = pool.begin().await.map_err(DataLayerError::sql)?;
            let claimed = sqlx::query(
                r#"
UPDATE referral_rewards
SET status = 'applying',
    admin_operator_id = COALESCE(?, admin_operator_id),
    admin_note = COALESCE(?, admin_note),
    updated_at = ?
WHERE id = ? AND status IN ('pending', 'failed')
"#,
            )
            .bind(operator_id)
            .bind(note)
            .bind(now_unix_secs() as i64)
            .bind(&target.id)
            .execute(&mut *tx)
            .await
            .map_err(DataLayerError::sql)?
            .rows_affected();
            if claimed == 0 {
                tx.commit().await.map_err(DataLayerError::sql)?;
                return Ok(());
            }
            let wallet = sqlx::query($wallet_sql)
                .bind(&target.wallet_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(DataLayerError::sql)?;
            let Some(wallet) = wallet else {
                sqlx::query(
                    r#"
UPDATE referral_rewards
SET status = 'failed',
    admin_operator_id = COALESCE(?, admin_operator_id),
    admin_note = COALESCE(?, admin_note),
    updated_at = ?
WHERE id = ?
"#,
                )
                .bind(operator_id)
                .bind(note.or(Some("邀请人钱包不存在")))
                .bind(now_unix_secs() as i64)
                .bind(&target.id)
                .execute(&mut *tx)
                .await
                .map_err(DataLayerError::sql)?;
                tx.commit().await.map_err(DataLayerError::sql)?;
                return Ok(());
            };
            let balance = row_f64!(wallet, "balance");
            let gift_before = row_f64!(wallet, "gift_balance");
            let gift_after = gift_before + target.amount_usd;
            let tx_id = uuid::Uuid::new_v4().to_string();
            let description = note
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| reward_description(&target));
            sqlx::query(
                r#"
UPDATE wallets
SET gift_balance = ?,
    total_adjusted = total_adjusted + ?,
    updated_at = ?
WHERE id = ?
"#,
            )
            .bind(gift_after)
            .bind(target.amount_usd)
            .bind(now_unix_secs() as i64)
            .bind(&target.wallet_id)
            .execute(&mut *tx)
            .await
            .map_err(DataLayerError::sql)?;
            sqlx::query(
                r#"
INSERT INTO wallet_transactions (
  id, wallet_id, category, reason_code, amount,
  balance_before, balance_after,
  recharge_balance_before, recharge_balance_after,
  gift_balance_before, gift_balance_after,
  link_type, link_id, operator_id, description, created_at
)
VALUES (?, ?, 'adjust', 'referral_reward', ?, ?, ?, ?, ?, ?, ?,
        'referral_reward', ?, ?, ?, ?)
"#,
            )
            .bind(&tx_id)
            .bind(&target.wallet_id)
            .bind(target.amount_usd)
            .bind(balance + gift_before)
            .bind(balance + gift_after)
            .bind(balance)
            .bind(balance)
            .bind(gift_before)
            .bind(gift_after)
            .bind(&target.id)
            .bind(operator_id)
            .bind(&description)
            .bind(now_unix_ms() as i64)
            .execute(&mut *tx)
            .await
            .map_err(DataLayerError::sql)?;
            sqlx::query(
                r#"
UPDATE referral_rewards
SET status = 'applied',
    wallet_transaction_id = ?,
    admin_operator_id = COALESCE(?, admin_operator_id),
    admin_note = COALESCE(?, admin_note),
    updated_at = ?
WHERE id = ?
"#,
            )
            .bind(&tx_id)
            .bind(operator_id)
            .bind(note)
            .bind(now_unix_secs() as i64)
            .bind(&target.id)
            .execute(&mut *tx)
            .await
            .map_err(DataLayerError::sql)?;
            tx.commit().await.map_err(DataLayerError::sql)?;
            Ok(())
        }
    };
}

impl ReferralDataState<'_> {
    #[cfg(feature = "mysql")]
    referral_credit_numeric_method!(
        credit_referral_reward_mysql_numeric_time,
        sqlx::MySqlPool,
        "SELECT balance, gift_balance FROM wallets WHERE id = ? FOR UPDATE"
    );
    #[cfg(feature = "sqlite")]
    referral_credit_numeric_method!(
        credit_referral_reward_sqlite_numeric_time,
        sqlx::SqlitePool,
        "SELECT balance, gift_balance FROM wallets WHERE id = ?"
    );

    #[cfg(any(feature = "mysql", feature = "sqlite"))]
    async fn apply_referral_reward_reversal_numeric_time(
        &self,
        reward: &ReferralRewardRecord,
        target_reversal_amount_usd: f64,
    ) -> Result<(), DataLayerError> {
        let Some(backends) = self.backends.as_ref() else {
            return Ok(());
        };
        #[cfg(feature = "mysql")]
        if let Some(backend) = backends.mysql() {
            return apply_referral_reward_reversal_for_mysql_pool(
                &backend.pool_clone(),
                reward,
                target_reversal_amount_usd,
            )
            .await;
        }
        #[cfg(feature = "sqlite")]
        if let Some(backend) = backends.sqlite() {
            return apply_referral_reward_reversal_for_sqlite_pool(
                &backend.pool_clone(),
                reward,
                target_reversal_amount_usd,
            )
            .await;
        }
        Ok(())
    }
}

fn payment_order_context_from_row<R>(row: R) -> Result<ReferralPaymentOrderContext, DataLayerError>
where
    R: Row,
    for<'c> &'c str: sqlx::ColumnIndex<R>,
    for<'r> String: sqlx::Decode<'r, R::Database> + sqlx::Type<R::Database>,
    for<'r> Option<String>: sqlx::Decode<'r, R::Database> + sqlx::Type<R::Database>,
    for<'r> f64: sqlx::Decode<'r, R::Database> + sqlx::Type<R::Database>,
{
    let Some(user_id) = row_optional_string!(row, "user_id") else {
        return Err(DataLayerError::InvalidInput(
            "payment order has no user_id".to_string(),
        ));
    };
    Ok(ReferralPaymentOrderContext {
        id: row_string!(row, "id"),
        user_id,
        amount_usd: row_f64!(row, "amount_usd"),
        payment_method: row_string!(row, "payment_method"),
        status: row_string!(row, "status"),
        order_kind: row_string!(row, "order_kind"),
    })
}

fn payment_order_refund_context_from_row<R>(
    row: R,
) -> Result<ReferralPaymentOrderRefundContext, DataLayerError>
where
    R: Row,
    for<'c> &'c str: sqlx::ColumnIndex<R>,
    for<'r> f64: sqlx::Decode<'r, R::Database> + sqlx::Type<R::Database>,
{
    Ok(ReferralPaymentOrderRefundContext {
        amount_usd: row_f64!(row, "amount_usd"),
        refunded_amount_usd: row_f64!(row, "refunded_amount_usd"),
    })
}

fn credit_target_from_row<R>(row: R) -> Result<ReferralCreditTarget, DataLayerError>
where
    R: Row,
    for<'c> &'c str: sqlx::ColumnIndex<R>,
    for<'r> String: sqlx::Decode<'r, R::Database> + sqlx::Type<R::Database>,
    for<'r> f64: sqlx::Decode<'r, R::Database> + sqlx::Type<R::Database>,
{
    Ok(ReferralCreditTarget {
        id: row_string!(row, "id"),
        wallet_id: row_string!(row, "wallet_id"),
        amount_usd: row_f64!(row, "amount_usd"),
        reward_type: row_string!(row, "reward_type"),
    })
}

#[cfg(any(feature = "mysql", feature = "sqlite"))]
macro_rules! referral_reversal_numeric_fn {
    ($name:ident, $pool_ty:ty, $wallet_sql:expr) => {
        async fn $name(
            pool: &$pool_ty,
            reward: &ReferralRewardRecord,
            target_reversal_amount_usd: f64,
        ) -> Result<(), DataLayerError> {
            let mut tx = pool.begin().await.map_err(DataLayerError::sql)?;
            sqlx::query("UPDATE referral_rewards SET updated_at = updated_at WHERE id = ?")
                .bind(&reward.id)
                .execute(&mut *tx)
                .await
                .map_err(DataLayerError::sql)?;
            let reward_row = sqlx::query(
                r#"
SELECT reversed_amount_usd, pending_reversal_amount_usd
FROM referral_rewards
WHERE id = ?
"#,
            )
            .bind(&reward.id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(DataLayerError::sql)?;
            let Some(reward_row) = reward_row else {
                tx.commit().await.map_err(DataLayerError::sql)?;
                return Ok(());
            };
            let current_reversed = row_f64!(reward_row, "reversed_amount_usd");
            let current_pending = row_f64!(reward_row, "pending_reversal_amount_usd");
            let amount_usd =
                (target_reversal_amount_usd - current_reversed - current_pending).max(0.0);
            if amount_usd <= 0.0 {
                tx.commit().await.map_err(DataLayerError::sql)?;
                return Ok(());
            }
            let wallet = sqlx::query($wallet_sql)
                .bind(&reward.inviter_user_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(DataLayerError::sql)?;
            let Some(wallet) = wallet else {
                tx.commit().await.map_err(DataLayerError::sql)?;
                return Ok(());
            };
            let wallet_id = row_string!(wallet, "id");
            let balance = row_f64!(wallet, "balance");
            let gift_before = row_f64!(wallet, "gift_balance");
            let actual_reverse = gift_before.min(amount_usd);
            let pending_reverse = (amount_usd - actual_reverse).max(0.0);
            let gift_after = gift_before - actual_reverse;
            if actual_reverse > 0.0 {
                sqlx::query(
                    r#"
UPDATE wallets
SET gift_balance = ?,
    total_adjusted = total_adjusted - ?,
    updated_at = ?
WHERE id = ?
"#,
                )
                .bind(gift_after)
                .bind(actual_reverse)
                .bind(now_unix_secs() as i64)
                .bind(&wallet_id)
                .execute(&mut *tx)
                .await
                .map_err(DataLayerError::sql)?;
                sqlx::query(
                    r#"
INSERT INTO wallet_transactions (
  id, wallet_id, category, reason_code, amount,
  balance_before, balance_after,
  recharge_balance_before, recharge_balance_after,
  gift_balance_before, gift_balance_after,
  link_type, link_id, description, created_at
)
VALUES (?, ?, 'adjust', 'referral_reward_reversal', ?, ?, ?, ?, ?, ?, ?,
        'referral_reward', ?, '邀请返利退款冲回', ?)
"#,
                )
                .bind(uuid::Uuid::new_v4().to_string())
                .bind(&wallet_id)
                .bind(-actual_reverse)
                .bind(balance + gift_before)
                .bind(balance + gift_after)
                .bind(balance)
                .bind(balance)
                .bind(gift_before)
                .bind(gift_after)
                .bind(&reward.id)
                .bind(now_unix_ms() as i64)
                .execute(&mut *tx)
                .await
                .map_err(DataLayerError::sql)?;
            }
            sqlx::query(
                r#"
UPDATE referral_rewards
SET reversed_amount_usd = reversed_amount_usd + ?,
    pending_reversal_amount_usd = pending_reversal_amount_usd + ?,
    status = CASE
      WHEN reversed_amount_usd + ? >= amount_usd THEN 'reversed'
      ELSE status
    END,
    updated_at = ?
WHERE id = ?
"#,
            )
            .bind(actual_reverse)
            .bind(pending_reverse)
            .bind(actual_reverse)
            .bind(now_unix_secs() as i64)
            .bind(&reward.id)
            .execute(&mut *tx)
            .await
            .map_err(DataLayerError::sql)?;
            tx.commit().await.map_err(DataLayerError::sql)?;
            Ok(())
        }
    };
}

#[cfg(feature = "mysql")]
referral_reversal_numeric_fn!(
    apply_referral_reward_reversal_for_mysql_pool,
    sqlx::MySqlPool,
    "SELECT id, balance, gift_balance FROM wallets WHERE user_id = ? FOR UPDATE"
);
#[cfg(feature = "sqlite")]
referral_reversal_numeric_fn!(
    apply_referral_reward_reversal_for_sqlite_pool,
    sqlx::SqlitePool,
    "SELECT id, balance, gift_balance FROM wallets WHERE user_id = ?"
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn referral_retry_only_allows_failed_rewards() {
        assert!(referral_retry_allowed("failed"));

        for status in ["pending", "applied", "reversed", "voided"] {
            assert!(!referral_retry_allowed(status), "{status} must not retry");
        }
    }

    #[test]
    fn referral_void_only_allows_pending_and_failed_rewards() {
        assert!(referral_void_allowed("pending"));
        assert!(referral_void_allowed("failed"));

        for status in ["applied", "reversed", "voided"] {
            assert!(!referral_void_allowed(status), "{status} must not void");
        }
    }

    #[test]
    fn referral_reversal_delta_uses_cumulative_refund_ratio() {
        let first = referral_reversal_delta(10.0, 100.0, 20.0, 0.0, 0.0);
        assert!((first - 2.0).abs() < f64::EPSILON);

        let second = referral_reversal_delta(10.0, 100.0, 50.0, 2.0, 0.0);
        assert!((second - 3.0).abs() < f64::EPSILON);

        let repeated = referral_reversal_delta(10.0, 100.0, 50.0, 2.0, 3.0);
        assert_eq!(repeated, 0.0);

        let full = referral_reversal_delta(10.0, 100.0, 125.0, 5.0, 0.0);
        assert!((full - 5.0).abs() < f64::EPSILON);
    }
}
