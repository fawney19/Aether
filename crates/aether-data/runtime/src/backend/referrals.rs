use crate::DataLayerError;
use aether_data_contracts::repository::wallet::payment_order_refund_amounts_are_consistent;
use serde::{Deserialize, Serialize};
use sqlx::Row;

use super::DataBackends;
pub use aether_data_contracts::repository::referrals::ReferralRewardConfig;
use aether_data_contracts::repository::referrals::REFERRAL_SNAPSHOT_KEY;

#[derive(Debug, Clone, Copy)]
pub struct ReferralDataState<'a> {
    backends: Option<&'a DataBackends>,
}

impl<'a> ReferralDataState<'a> {
    pub fn new(backends: Option<&'a DataBackends>) -> Self {
        Self { backends }
    }
}

const REFERRAL_RECONCILIATION_LIMIT: usize = 200;

// Successful refunds exclude processing reservations; the fallback supports
// historical refunds that only updated the order aggregate.
const REFERRAL_REFUND_CONTEXT_SQL: &str = r#"
SELECT CAST(po.amount_usd AS DOUBLE PRECISION) AS amount_usd,
       CAST(
         CASE
           WHEN COALESCE((
             SELECT SUM(rr.amount_usd)
             FROM refund_requests rr
             WHERE rr.payment_order_id = po.id
               AND rr.status = 'succeeded'
           ), 0.0) >=
             COALESCE(po.refunded_amount_usd, 0.0) - COALESCE((
               SELECT SUM(rr.amount_usd)
               FROM refund_requests rr
               WHERE rr.payment_order_id = po.id
                 AND rr.status = 'processing'
             ), 0.0)
           THEN COALESCE((
             SELECT SUM(rr.amount_usd)
             FROM refund_requests rr
             WHERE rr.payment_order_id = po.id
               AND rr.status = 'succeeded'
           ), 0.0)
           ELSE COALESCE(po.refunded_amount_usd, 0.0) - COALESCE((
             SELECT SUM(rr.amount_usd)
             FROM refund_requests rr
             WHERE rr.payment_order_id = po.id
               AND rr.status = 'processing'
           ), 0.0)
         END AS DOUBLE PRECISION
       ) AS refunded_amount_usd
FROM payment_orders po
WHERE po.id = $1
"#;

// The list tests intentionally build one page larger than the historical
// in-memory fetch cap. Keep the fixture cap test-only now that production
// queries paginate directly in SQL.

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
    pub inviter_username: Option<String>,
    pub invitee_username: Option<String>,
    pub source_order_no: Option<String>,
    pub inviter_wallet_id: Option<String>,
    pub reward_type: String,
    pub source_order_id: Option<String>,
    pub trigger_point: String,
    pub amount_usd: f64,
    pub status: String,
    pub wallet_transaction_id: Option<String>,
    #[serde(skip_serializing)]
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

/// Result of one bounded referral reconciliation pass.
///
/// The pass is intentionally idempotent: rows that cannot be applied (for
/// example, because the inviter wallet is temporarily unavailable) remain in
/// their durable pending/failed state and are picked up by the next pass.
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct ReferralReconciliationSummary {
    pub order_attempted: u64,
    pub order_repaired: u64,
    pub reward_attempted: u64,
    pub reward_applied: u64,
    pub reversal_attempted: u64,
    pub reversal_applied: u64,
    pub deferred: u64,
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
    pub order_no: Option<String>,
    pub inviter: Option<String>,
    pub invitee: Option<String>,
    pub referral_id: Option<String>,
    pub trigger_point: Option<String>,
    pub pending_reversal: Option<bool>,
    pub reward_type: Option<String>,
    pub status: Option<String>,
    pub limit: usize,
    pub offset: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferralMutationStatus {
    Applied,
    NotFound,
    Invalid,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReferralApplyingRecovery {
    Applied,
    Failed,
    Unchanged,
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
            inviter_username: $row
                .try_get::<Option<String>, _>("inviter_username")
                .ok()
                .flatten(),
            invitee_username: $row
                .try_get::<Option<String>, _>("invitee_username")
                .ok()
                .flatten(),
            source_order_no: $row
                .try_get::<Option<String>, _>("source_order_no")
                .ok()
                .flatten(),
            inviter_wallet_id: $row
                .try_get::<Option<String>, _>("inviter_wallet_id")
                .ok()
                .flatten(),
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

mod admin;
pub use admin::{ReferralAdminOverviewStats, ReferralRewardDetail};
mod preview;
mod registration;
mod settings;
pub use preview::{ReferralRefundPreview, ReferralReversalSummary};

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

/// Build a case-insensitive SQL `LIKE` pattern while treating user input as a
/// literal substring.  `!` is used as the escape character because it is
/// accepted by PostgreSQL.
fn referral_like_pattern(value: Option<&str>) -> String {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return String::new();
    };
    let escaped = value
        .replace('!', "!!")
        .replace('%', "!%")
        .replace('_', "!_")
        .to_ascii_lowercase();
    format!("%{escaped}%")
}

fn referral_page_bounds(limit: usize, offset: usize) -> (i64, i64) {
    let limit = limit.clamp(1, 200) as i64;
    let offset = i64::try_from(offset).unwrap_or(i64::MAX);
    (limit, offset)
}

fn referral_stats_amount(value: f64) -> f64 {
    if value.is_nan() || value < 0.0 {
        0.0
    } else if value.is_infinite() {
        // Database SUM over legacy rows can overflow a binary float. Keep a
        // finite, monotonic public value instead of silently reporting zero.
        f64::MAX
    } else {
        value
    }
}

fn referral_stats_count(value: i64) -> u64 {
    value.max(0) as u64
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

#[cfg(test)]
fn referral_percent_rate_valid(percent_rate: f64) -> bool {
    percent_rate.is_finite() && percent_rate > 0.0 && percent_rate <= 100.0
}

#[cfg(test)]
fn referral_payment_method_excluded(payment_method: &str) -> bool {
    matches!(
        payment_method.trim().to_ascii_lowercase().as_str(),
        "manual" | "admin_manual" | "redeem_code" | "gift"
    )
}

fn referral_refund_context_valid(context: &ReferralPaymentOrderRefundContext) -> bool {
    context.refunded_amount_usd > 0.0
        && payment_order_refund_amounts_are_consistent(
            context.amount_usd,
            context.refunded_amount_usd,
            (context.amount_usd - context.refunded_amount_usd).max(0.0),
        )
}

fn referral_wallet_values_valid(balance: f64, gift_balance: f64) -> bool {
    // Recharge balances may legitimately be negative when overdraft is
    // enabled. Gift balances, however, are never allowed to go below zero.
    balance.is_finite() && gift_balance.is_finite() && gift_balance >= 0.0
}

fn referral_amounts_match(left: f64, right: f64) -> bool {
    if !left.is_finite() || !right.is_finite() {
        return false;
    }
    // PostgreSQL NUMERIC values are decoded through f64 in this runtime.
    // Preserve the eight-decimal storage tolerance while allowing a handful
    // of ULPs when the running balance is large.
    let scale = left.abs().max(right.abs()).max(1.0);
    let tolerance = 0.00000001_f64.max(scale * f64::EPSILON * 8.0);
    (left - right).abs() <= tolerance
}

/// Validate the durable wallet snapshot written alongside a referral credit.
///
/// Matching only `link_id` and `amount` is insufficient: a malformed or
/// manually-inserted transaction could otherwise turn an interrupted
/// `applying` reward into `applied` without ever increasing the inviter's gift
/// balance.  The normal credit path writes a complete before/after snapshot,
/// so recovery can require those same invariants before trusting the fact.
// The fact validator compares the complete before/after ledger snapshot. Keep
// each value explicit so a caller cannot accidentally substitute a bucket or
// omit one of the persisted invariants.
#[allow(clippy::too_many_arguments)]
fn referral_credit_transaction_fact_valid(
    reward_amount_usd: f64,
    amount: f64,
    balance_before: f64,
    balance_after: f64,
    recharge_balance_before: f64,
    recharge_balance_after: f64,
    gift_balance_before: f64,
    gift_balance_after: f64,
) -> bool {
    if !reward_amount_usd.is_finite()
        || reward_amount_usd <= 0.0
        || !amount.is_finite()
        || amount <= 0.0
        || !referral_amounts_match(amount, reward_amount_usd)
        || !balance_before.is_finite()
        || !balance_after.is_finite()
        || !recharge_balance_before.is_finite()
        || !recharge_balance_after.is_finite()
        || !gift_balance_before.is_finite()
        || !gift_balance_after.is_finite()
        || gift_balance_before < 0.0
        || gift_balance_after < 0.0
    {
        return false;
    }

    // Referral credits affect only the gift bucket.  The total balance and
    // both bucket decompositions must agree with the signed transaction.
    referral_amounts_match(recharge_balance_before, recharge_balance_after)
        && referral_amounts_match(balance_after, balance_before + amount)
        && referral_amounts_match(gift_balance_after, gift_balance_before + amount)
        && referral_amounts_match(
            balance_before,
            recharge_balance_before + gift_balance_before,
        )
        && referral_amounts_match(balance_after, recharge_balance_after + gift_balance_after)
}

fn referral_reversal_state_valid(
    reward_amount_usd: f64,
    current_reversed_amount_usd: f64,
    current_pending_amount_usd: f64,
    actual_reverse_amount_usd: f64,
    pending_after_usd: f64,
) -> bool {
    if !reward_amount_usd.is_finite()
        || !current_reversed_amount_usd.is_finite()
        || !current_pending_amount_usd.is_finite()
        || !actual_reverse_amount_usd.is_finite()
        || !pending_after_usd.is_finite()
        || reward_amount_usd <= 0.0
        || current_reversed_amount_usd < 0.0
        || current_pending_amount_usd < 0.0
        || actual_reverse_amount_usd < 0.0
        || pending_after_usd < 0.0
    {
        return false;
    }
    let reversed_after_usd = current_reversed_amount_usd + actual_reverse_amount_usd;
    let total_reversal_after_usd = reversed_after_usd + pending_after_usd;
    reversed_after_usd.is_finite()
        && total_reversal_after_usd.is_finite()
        && reversed_after_usd <= reward_amount_usd + 0.00000001
        && total_reversal_after_usd <= reward_amount_usd + 0.00000001
}

/// Validate the durable reversal counters before calculating or persisting a
/// new debt.  In particular, this must run before the wallet lookup: a missing
/// wallet is a normal retry condition, but it must not become a way to carry
/// malformed negative/overflowed counters forward indefinitely.
fn referral_reversal_inputs_valid(
    reward_amount_usd: f64,
    target_reversal_amount_usd: f64,
    current_reversed_amount_usd: f64,
    current_pending_amount_usd: f64,
) -> bool {
    if !reward_amount_usd.is_finite()
        || !target_reversal_amount_usd.is_finite()
        || !current_reversed_amount_usd.is_finite()
        || !current_pending_amount_usd.is_finite()
        || reward_amount_usd <= 0.0
        || target_reversal_amount_usd < 0.0
        || current_reversed_amount_usd < 0.0
        || current_pending_amount_usd < 0.0
    {
        return false;
    }

    let total_reversal = current_reversed_amount_usd + current_pending_amount_usd;
    let tolerance = 0.00000001_f64;
    total_reversal.is_finite()
        && current_reversed_amount_usd <= reward_amount_usd + tolerance
        && total_reversal <= reward_amount_usd + tolerance
        && target_reversal_amount_usd <= reward_amount_usd + tolerance
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
    referral_reversal_due_bounded(
        target_reversal,
        reward_amount_usd,
        reversed_amount_usd,
        pending_reversal_amount_usd,
    )
}

fn referral_reversal_due(
    target_reversal_amount_usd: f64,
    reversed_amount_usd: f64,
    pending_reversal_amount_usd: f64,
) -> f64 {
    // A pending amount is a debt, not an amount that has already been
    // reversed. Keep it eligible on later passes while also accounting for a
    // refund that increased the cumulative target.
    (target_reversal_amount_usd - reversed_amount_usd)
        .max(0.0)
        .max(pending_reversal_amount_usd.max(0.0))
}

fn referral_reversal_due_bounded(
    target_reversal_amount_usd: f64,
    reward_amount_usd: f64,
    reversed_amount_usd: f64,
    pending_reversal_amount_usd: f64,
) -> f64 {
    if !target_reversal_amount_usd.is_finite()
        || !reward_amount_usd.is_finite()
        || !reversed_amount_usd.is_finite()
        || !pending_reversal_amount_usd.is_finite()
    {
        return 0.0;
    }
    referral_reversal_due(
        target_reversal_amount_usd,
        reversed_amount_usd,
        pending_reversal_amount_usd,
    )
    // Cap the debt at the reward's remaining principal even when a legacy row
    // contains an oversized pending value.
    .min((reward_amount_usd - reversed_amount_usd.max(0.0)).max(0.0))
}

fn referral_pending_reversal_capped(
    reward_amount_usd: f64,
    reversed_amount_usd: f64,
    current_pending_amount_usd: f64,
    due_amount_usd: f64,
) -> f64 {
    let remaining_principal = (reward_amount_usd - reversed_amount_usd.max(0.0)).max(0.0);
    current_pending_amount_usd
        .max(due_amount_usd)
        .min(remaining_principal)
}

fn referral_reversal_target(
    reward_amount_usd: f64,
    order_amount_usd: f64,
    refunded_amount_usd: f64,
) -> f64 {
    if !reward_amount_usd.is_finite()
        || !order_amount_usd.is_finite()
        || !refunded_amount_usd.is_finite()
        || reward_amount_usd <= 0.0
        || order_amount_usd <= 0.0
        || refunded_amount_usd <= 0.0
    {
        return 0.0;
    }
    // Full refunds preserve the exact stored principal; multiplying a large
    // value by the decimal scale and back can otherwise lose several units.
    if refunded_amount_usd >= order_amount_usd {
        return reward_amount_usd;
    }
    // Decimal strings preserve the persisted monetary scale. Rounding a binary
    // float such as 3e-8 * 50% would miss PostgreSQL NUMERIC's half-up boundary.
    // All values are finite and positive above, so parsing cannot fail.
    let reward = reward_amount_usd
        .to_string()
        .parse::<sqlx::types::BigDecimal>()
        .expect("finite reward decimal");
    let refunded = refunded_amount_usd
        .to_string()
        .parse::<sqlx::types::BigDecimal>()
        .expect("finite refunded decimal");
    let order = order_amount_usd
        .to_string()
        .parse::<sqlx::types::BigDecimal>()
        .expect("finite order decimal");
    let half_unit = "0.000000005"
        .parse::<sqlx::types::BigDecimal>()
        .expect("constant decimal");
    // Nonnegative NUMERIC ROUND(...,8) is half-up: add half a unit then
    // truncate, independent of BigDecimal's configurable default rounding.
    ((reward * refunded / order + half_unit).with_scale(8))
        .to_string()
        .parse::<f64>()
        .expect("bounded referral target")
        .min(reward_amount_usd)
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
        Ok(false)
    }

    pub async fn referral_dashboard(
        &self,
        user_id: &str,
    ) -> Result<Option<ReferralUserDashboard>, DataLayerError> {
        let Some(invite_code) = self.ensure_referral_invite_code(user_id).await? else {
            return Ok(None);
        };
        // Dashboard metrics must cover the complete history; do not derive
        // them from a bounded list page.
        let stats = self.referral_admin_stats_global(Some(user_id)).await?;
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
        // The cards in the admin view are global totals (their labels use
        // "total"/"paid" rather than "filtered").  Compute them with an
        // aggregate query instead of deriving them from the bounded list
        // page, so pagination and filters cannot change the headline stats.
        let (mut items, total) = self.list_referral_relationships_raw(&query).await?;
        for relationship in &mut items {
            relationship.source = None;
        }
        let stats = self.referral_admin_stats_global(None).await?;
        Ok(Some((items, total, stats)))
    }

    pub async fn list_admin_referral_rewards(
        &self,
        query: ReferralRewardListQuery,
    ) -> Result<Option<(Vec<ReferralRewardRecord>, u64, ReferralAdminStats)>, DataLayerError> {
        if self.backends.is_none() {
            return Ok(None);
        }
        let (items, total) = self.list_referral_rewards_raw(&query).await?;
        let stats = self.referral_admin_stats_global(None).await?;
        Ok(Some((items, total, stats)))
    }

    /// Read the headline referral metrics without applying the list window.
    ///
    /// Admin list endpoints intentionally cap their row payloads, so deriving
    /// metrics from those rows would silently under-count once the history is
    /// larger than the fetch limit.  Keep the aggregate in the data layer and
    /// use the native numeric type of each backend before normalising it to the
    /// public `f64` contract.
    async fn referral_admin_stats_global(
        &self,
        inviter_user_id: Option<&str>,
    ) -> Result<ReferralAdminStats, DataLayerError> {
        let Some(backends) = self.backends.as_ref() else {
            return Ok(ReferralAdminStats::default());
        };

        #[cfg(feature = "postgres")]
        if let Some(backend) = backends.postgres() {
            let row = sqlx::query(
                r#"
SELECT
  (SELECT COUNT(*) FROM user_referrals
    WHERE ($1::TEXT IS NULL OR inviter_user_id = $1)) AS total_invites,
  (SELECT COUNT(*) FROM user_referrals
    WHERE ($1::TEXT IS NULL OR inviter_user_id = $1)
      AND first_paid_order_id IS NOT NULL)
    AS effective_invites,
  CAST(COALESCE(SUM(CASE
    WHEN status = 'applied' AND amount_usd > 0 THEN amount_usd ELSE 0 END), 0)
    AS DOUBLE PRECISION) AS paid_reward_usd,
  CAST(COALESCE(SUM(CASE
    WHEN status IN ('pending', 'failed', 'applying') AND amount_usd > 0 THEN amount_usd ELSE 0 END), 0)
    AS DOUBLE PRECISION) AS pending_reward_usd,
  CAST(COALESCE(SUM(CASE
    WHEN reversed_amount_usd > 0 THEN reversed_amount_usd ELSE 0 END), 0)
    AS DOUBLE PRECISION) AS reversed_reward_usd
FROM referral_rewards
WHERE ($1::TEXT IS NULL OR inviter_user_id = $1)
"#,
            )
            .bind(inviter_user_id)
            .fetch_one(&backend.pool_clone())
            .await
            .map_err(DataLayerError::postgres)?;
            return Ok(ReferralAdminStats {
                total_invites: referral_stats_count(
                    row.try_get::<i64, _>("total_invites")
                        .map_err(DataLayerError::postgres)?,
                ),
                effective_invites: referral_stats_count(
                    row.try_get::<i64, _>("effective_invites")
                        .map_err(DataLayerError::postgres)?,
                ),
                paid_reward_usd: referral_stats_amount(
                    row.try_get::<f64, _>("paid_reward_usd")
                        .map_err(DataLayerError::postgres)?,
                ),
                pending_reward_usd: referral_stats_amount(
                    row.try_get::<f64, _>("pending_reward_usd")
                        .map_err(DataLayerError::postgres)?,
                ),
                reversed_reward_usd: referral_stats_amount(
                    row.try_get::<f64, _>("reversed_reward_usd")
                        .map_err(DataLayerError::postgres)?,
                ),
            });
        }

        Ok(ReferralAdminStats::default())
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
        _amount_usd: f64,
        _trigger_point: &str,
    ) -> Result<Vec<ReferralRewardRecord>, DataLayerError> {
        self.settle_registration_referral_rewards(invitee_user_id)
            .await
    }

    pub async fn apply_paid_order_referral_rewards(
        &self,
        order_id: &str,
        _config: ReferralRewardConfig,
    ) -> Result<Vec<ReferralRewardRecord>, DataLayerError> {
        self.settle_paid_order_referral_rewards(order_id).await
    }

    /// Consume only obligations committed by the payment credit transaction.
    /// Legacy orders without a durable obligation are never rewarded on replay.
    pub async fn settle_paid_order_referral_rewards(
        &self,
        order_id: &str,
    ) -> Result<Vec<ReferralRewardRecord>, DataLayerError> {
        #[cfg(feature = "postgres")]
        if let Some(backend) = self.backends.and_then(DataBackends::postgres) {
            let keys = sqlx::query_scalar::<_, String>("SELECT idempotency_key FROM referral_rewards WHERE source_order_id=$1 AND status IN ('pending','failed') AND amount_usd>0 ORDER BY created_at,id")
                .bind(order_id).fetch_all(&backend.pool_clone()).await.map_err(DataLayerError::postgres)?;
            let mut rewards = self
                .credit_pending_referral_rewards(&keys, None, None)
                .await?;
            if let Some(refund) = self
                .find_referral_payment_order_refund_context(order_id)
                .await?
            {
                if referral_refund_context_valid(&refund) {
                    self.reverse_referral_rewards_for_order(order_id, refund.refunded_amount_usd)
                        .await?;
                    for reward in &mut rewards {
                        if let Some(updated) = self.find_referral_reward(&reward.id).await? {
                            *reward = updated;
                        }
                    }
                }
            }
            return Ok(rewards);
        }
        Ok(Vec::new())
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
        if !reward.amount_usd.is_finite() || reward.amount_usd <= 0.0 {
            return Err(DataLayerError::InvalidInput(
                "返利金额无效，无法补发".to_string(),
            ));
        }
        let rewards = self
            .credit_pending_referral_rewards(&[reward.idempotency_key], operator_id, note)
            .await?;
        let Some(mut updated) = rewards.into_iter().next() else {
            return Ok(None);
        };

        // A manual retry can race with a payment refund.  Do the same
        // refund-aware reconciliation as the normal paid-order path so a
        // successful retry can never leave a newly credited, already-refunded
        // order permanently over-rewarded.
        if updated.status == "applied" {
            if let Some(order_id) = updated.source_order_id.as_deref() {
                let refund_amount = self
                    .find_referral_payment_order_refund_context(order_id)
                    .await?
                    .and_then(|refund| {
                        let valid = refund.amount_usd.is_finite()
                            && refund.amount_usd > 0.0
                            && refund.refunded_amount_usd.is_finite()
                            && refund.refunded_amount_usd > 0.0;
                        valid.then_some(refund.refunded_amount_usd)
                    });
                if let Some(refund_amount) = refund_amount {
                    self.reverse_referral_rewards_for_order(order_id, refund_amount)
                        .await?;
                    if let Some(refreshed) = self.find_referral_reward(&updated.id).await? {
                        updated = refreshed;
                    }
                }
            }
        }
        Ok(Some(updated))
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
        if !self
            .update_referral_reward_status(reward_id, "voided", operator_id, note)
            .await?
        {
            return Err(DataLayerError::InvalidInput(
                "返利状态已变化，请刷新后重试".to_string(),
            ));
        }
        self.find_referral_reward(reward_id).await
    }

    pub async fn reverse_referral_rewards_for_order(
        &self,
        order_id: &str,
        amount_usd: f64,
    ) -> Result<Vec<ReferralRewardRecord>, DataLayerError> {
        if !amount_usd.is_finite() || amount_usd <= 0.0 {
            return Ok(Vec::new());
        }
        let Some(refund_context) = self
            .find_referral_payment_order_refund_context(order_id)
            .await?
        else {
            return Ok(Vec::new());
        };
        if !referral_refund_context_valid(&refund_context) {
            return Ok(Vec::new());
        }
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
            // The reversal transaction re-reads and locks the source payment
            // order before calculating its target.  The context above is only
            // the caller-side eligibility check and may be stale by now.
            self.apply_referral_reward_reversal(&reward).await?;
            if let Some(updated) = self.find_referral_reward(&reward.id).await? {
                reversed.push(updated);
            }
        }
        Ok(reversed)
    }

    /// Reconcile durable referral obligations left behind by an interrupted
    /// payment callback or by a temporarily unavailable inviter wallet.
    ///
    /// The current reward configuration is accepted for API compatibility, but
    /// it is deliberately not used to infer missing rows from payment history:
    /// configuration has no historical snapshot, so doing that would
    /// retroactively apply today's rate/mode to orders made before the feature
    /// was enabled (or while a different mode was active).  Only durable
    /// pending/failed/applying reward rows and reversal debts are retried.
    pub async fn reconcile_referral_rewards_once(
        &self,
        _reward_config: Option<ReferralRewardConfig>,
    ) -> Result<ReferralReconciliationSummary, DataLayerError> {
        if self.backends.is_none() {
            return Ok(ReferralReconciliationSummary::default());
        }

        let mut summary = ReferralReconciliationSummary::default();
        let mut first_error = None;

        let reward_keys = match self.list_referral_reward_retry_keys().await {
            Ok(keys) => keys,
            Err(error) => {
                if let Some(first_error) = first_error {
                    return Err(first_error);
                }
                return Err(error);
            }
        };

        // Retry rows whose reward credit transaction did not reach `applied`.
        // Process each key independently so one broken wallet does not starve
        // unrelated referral rewards in the same pass.
        for idempotency_key in reward_keys {
            summary.reward_attempted += 1;
            let result = self
                .credit_pending_referral_rewards(std::slice::from_ref(&idempotency_key), None, None)
                .await;
            match result {
                Ok(updated)
                    if updated
                        .iter()
                        .any(|item| matches!(item.status.as_str(), "applied" | "reversed")) =>
                {
                    summary.reward_applied += 1;
                }
                Ok(_) => summary.deferred += 1,
                Err(error) => {
                    summary.deferred += 1;
                    if first_error.is_none() {
                        first_error = Some(error);
                    }
                }
            }
        }

        // An older implementation could commit the intermediate `applying`
        // state independently from the wallet credit. Resolve those rows from
        // the durable wallet transaction fact, never by crediting them again.
        // Rows without a matching transaction become `failed` and are only
        // eligible for the normal credit path on a later pass.
        let applying_reward_ids = match self.list_applying_referral_reward_ids().await {
            Ok(ids) => ids,
            Err(error) => {
                if first_error.is_none() {
                    first_error = Some(error);
                }
                Vec::new()
            }
        };
        for reward_id in applying_reward_ids {
            summary.reward_attempted += 1;
            match self.recover_applying_referral_reward(&reward_id).await {
                Ok(ReferralApplyingRecovery::Applied) => summary.reward_applied += 1,
                Ok(ReferralApplyingRecovery::Failed | ReferralApplyingRecovery::Unchanged) => {
                    summary.deferred += 1;
                }
                Err(error) => {
                    summary.deferred += 1;
                    if first_error.is_none() {
                        first_error = Some(error);
                    }
                }
            }
        }

        // Refresh the rows after reward retries.  A reward that was applied in
        // the first phase may itself carry an outstanding refund reversal.
        let rewards = match self.list_referral_reversal_candidates().await {
            Ok(rewards) => rewards,
            Err(error) => {
                if first_error.is_none() {
                    first_error = Some(error);
                }
                Vec::new()
            }
        };
        for reward in rewards.iter().take(REFERRAL_RECONCILIATION_LIMIT) {
            let Some(order_id) = reward.source_order_id.as_deref() else {
                summary.deferred += 1;
                continue;
            };
            let refund_context = match self
                .find_referral_payment_order_refund_context(order_id)
                .await
            {
                Ok(Some(context)) if referral_refund_context_valid(&context) => context,
                Ok(Some(_)) => {
                    // Do not let a malformed historical order authorize a
                    // pending reversal.  Pending debt is retried only after
                    // its source refund can be validated again.
                    summary.deferred += 1;
                    continue;
                }
                Ok(None) => {
                    summary.deferred += 1;
                    continue;
                }
                Err(error) => {
                    summary.deferred += 1;
                    if first_error.is_none() {
                        first_error = Some(error);
                    }
                    continue;
                }
            };
            let target_reversal = referral_reversal_target(
                reward.amount_usd,
                refund_context.amount_usd,
                refund_context.refunded_amount_usd,
            );
            let due = referral_reversal_due_bounded(
                target_reversal,
                reward.amount_usd,
                reward.reversed_amount_usd,
                reward.pending_reversal_amount_usd,
            );
            if due <= f64::EPSILON {
                continue;
            }
            summary.reversal_attempted += 1;
            // `target_reversal` was calculated from the candidate-list
            // snapshot. The transaction below obtains a fresh, locked order
            // row and recalculates it before mutating either balance or debt.
            match self.apply_referral_reward_reversal(reward).await {
                Ok(()) => summary.reversal_applied += 1,
                Err(error) => {
                    summary.deferred += 1;
                    if first_error.is_none() {
                        first_error = Some(error);
                    }
                }
            }
        }

        if let Some(error) = first_error {
            return Err(error);
        }
        Ok(summary)
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
        Ok(false)
    }

    async fn list_referral_relationships_raw(
        &self,
        query: &ReferralRelationshipListQuery,
    ) -> Result<(Vec<ReferralRelationshipRecord>, u64), DataLayerError> {
        let Some(backends) = self.backends.as_ref() else {
            return Ok((Vec::new(), 0));
        };
        let inviter_pattern = referral_like_pattern(query.inviter.as_deref());
        let invitee_pattern = referral_like_pattern(query.invitee.as_deref());
        let invite_code_pattern = referral_like_pattern(query.invite_code.as_deref());
        let first_paid = query
            .first_paid
            .map(|value| i64::from(value as u8))
            .unwrap_or(-1);
        let (limit, offset) = referral_page_bounds(query.limit, query.offset);
        #[cfg(feature = "postgres")]
        if let Some(backend) = backends.postgres() {
            let count = sqlx::query(
                r#"
SELECT COUNT(*) AS total
FROM user_referrals r
LEFT JOIN users inviter ON inviter.id = r.inviter_user_id
LEFT JOIN users invitee ON invitee.id = r.invitee_user_id
WHERE ($1 = '' OR LOWER(COALESCE(inviter.username, '')) LIKE $1 ESCAPE '!' OR LOWER(r.inviter_user_id) LIKE $1 ESCAPE '!')
  AND ($2 = '' OR LOWER(COALESCE(invitee.username, '')) LIKE $2 ESCAPE '!' OR LOWER(r.invitee_user_id) LIKE $2 ESCAPE '!')
  AND ($3 = '' OR LOWER(r.invite_code_snapshot) LIKE $3 ESCAPE '!')
  AND ($4 < 0 OR ($4 = 1 AND r.first_paid_order_id IS NOT NULL) OR ($4 = 0 AND r.first_paid_order_id IS NULL))
"#,
            )
            .bind(&inviter_pattern)
            .bind(&invitee_pattern)
            .bind(&invite_code_pattern)
            .bind(first_paid)
            .fetch_one(&backend.pool_clone())
            .await
            .map_err(DataLayerError::postgres)?;
            let total = count
                .try_get::<i64, _>("total")
                .map_err(DataLayerError::postgres)?;
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
WHERE ($1 = '' OR LOWER(COALESCE(inviter.username, '')) LIKE $1 ESCAPE '!' OR LOWER(r.inviter_user_id) LIKE $1 ESCAPE '!')
  AND ($2 = '' OR LOWER(COALESCE(invitee.username, '')) LIKE $2 ESCAPE '!' OR LOWER(r.invitee_user_id) LIKE $2 ESCAPE '!')
  AND ($3 = '' OR LOWER(r.invite_code_snapshot) LIKE $3 ESCAPE '!')
  AND ($4 < 0 OR ($4 = 1 AND r.first_paid_order_id IS NOT NULL) OR ($4 = 0 AND r.first_paid_order_id IS NULL))
ORDER BY r.created_at DESC, r.id DESC
LIMIT $5 OFFSET $6
"#,
            )
            .bind(&inviter_pattern)
            .bind(&invitee_pattern)
            .bind(&invite_code_pattern)
            .bind(first_paid)
            .bind(limit)
            .bind(offset)
            .fetch_all(&backend.pool_clone())
            .await
            .map_err(DataLayerError::postgres)?;
            let items = rows
                .iter()
                .map(|row| relationship_from_row!(row))
                .collect::<Result<Vec<_>, _>>()?;
            return Ok((items, total.max(0) as u64));
        }
        Ok((Vec::new(), 0))
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
        Ok(None)
    }

    async fn list_referral_rewards_raw(
        &self,
        query: &ReferralRewardListQuery,
    ) -> Result<(Vec<ReferralRewardRecord>, u64), DataLayerError> {
        admin::list_rewards(self, query).await
    }

    async fn list_applying_referral_reward_ids(&self) -> Result<Vec<String>, DataLayerError> {
        let Some(backends) = self.backends.as_ref() else {
            return Ok(Vec::new());
        };
        #[cfg(feature = "postgres")]
        if let Some(backend) = backends.postgres() {
            let rows = sqlx::query(
                r#"
SELECT id
FROM referral_rewards
WHERE status = 'applying'
ORDER BY updated_at ASC, created_at ASC, id ASC
LIMIT $1
"#,
            )
            .bind(REFERRAL_RECONCILIATION_LIMIT as i64)
            .fetch_all(&backend.pool_clone())
            .await
            .map_err(DataLayerError::postgres)?;
            return rows.iter().map(|row| Ok(row_string!(row, "id"))).collect();
        }
        Ok(Vec::new())
    }

    /// Select only rewards that can be credited now. Ineligible historical
    /// rows must not occupy the bounded retry page and starve valid rewards.
    async fn list_referral_reward_retry_keys(&self) -> Result<Vec<String>, DataLayerError> {
        let Some(backends) = self.backends.as_ref() else {
            return Ok(Vec::new());
        };
        #[cfg(feature = "postgres")]
        if let Some(backend) = backends.postgres() {
            let rows = sqlx::query(
                r#"
SELECT rw.idempotency_key
FROM referral_rewards rw
JOIN wallets ON wallets.user_id = rw.inviter_user_id
  AND wallets.status = 'active'
JOIN users inviter ON inviter.id = rw.inviter_user_id
  AND inviter.is_active IS TRUE AND inviter.is_deleted IS FALSE
WHERE rw.status IN ('pending', 'failed')
  AND rw.amount_usd > 0
ORDER BY rw.created_at ASC, rw.id ASC
LIMIT $1
"#,
            )
            .bind(REFERRAL_RECONCILIATION_LIMIT as i64)
            .fetch_all(&backend.pool_clone())
            .await
            .map_err(DataLayerError::postgres)?;
            return rows
                .iter()
                .map(|row| Ok(row_string!(row, "idempotency_key")))
                .collect();
        }
        Ok(Vec::new())
    }

    /// Return rewards that can have a refund reversal.  Filtering against the
    /// payment order here is important: a reward may be newly applied after a
    /// refund has already completed, in which case its pending column is
    /// still zero and a pending-only scan would miss it forever.
    async fn list_referral_reversal_candidates(
        &self,
    ) -> Result<Vec<ReferralRewardRecord>, DataLayerError> {
        let Some(backends) = self.backends.as_ref() else {
            return Ok(Vec::new());
        };
        #[cfg(feature = "postgres")]
        if let Some(backend) = backends.postgres() {
            let rows = sqlx::query(
                r#"
SELECT
  rw.id, rw.referral_id, rw.inviter_user_id, rw.invitee_user_id,
  rw.reward_type, rw.source_order_id, rw.trigger_point,
  CAST(rw.amount_usd AS DOUBLE PRECISION) AS amount_usd,
  rw.status, rw.wallet_transaction_id, rw.idempotency_key,
  CAST(rw.reversed_amount_usd AS DOUBLE PRECISION) AS reversed_amount_usd,
  CAST(rw.pending_reversal_amount_usd AS DOUBLE PRECISION) AS pending_reversal_amount_usd,
  rw.admin_operator_id, rw.admin_note,
  EXTRACT(EPOCH FROM rw.created_at)::BIGINT AS created_at_unix_secs,
  EXTRACT(EPOCH FROM rw.updated_at)::BIGINT AS updated_at_unix_secs
FROM referral_rewards rw
JOIN (
  SELECT
    po0.id,
    CAST(po0.amount_usd AS NUMERIC) AS amount_usd,
    po0.credited_at,
    po0.paid_at,
    po0.created_at,
    CAST(
      CASE
        WHEN COALESCE((
          SELECT SUM(rr.amount_usd)
          FROM refund_requests rr
          WHERE rr.payment_order_id = po0.id
            AND rr.status = 'succeeded'
        ), 0.0) >=
          COALESCE(po0.refunded_amount_usd, 0.0) - COALESCE((
            SELECT SUM(rr.amount_usd)
            FROM refund_requests rr
            WHERE rr.payment_order_id = po0.id
              AND rr.status = 'processing'
          ), 0.0)
        THEN COALESCE((
          SELECT SUM(rr.amount_usd)
          FROM refund_requests rr
          WHERE rr.payment_order_id = po0.id
            AND rr.status = 'succeeded'
        ), 0.0)
        ELSE COALESCE(po0.refunded_amount_usd, 0.0) - COALESCE((
          SELECT SUM(rr.amount_usd)
          FROM refund_requests rr
          WHERE rr.payment_order_id = po0.id
            AND rr.status = 'processing'
        ), 0.0)
      END AS NUMERIC
    ) AS refunded_amount_usd
  FROM payment_orders po0
) po ON po.id = rw.source_order_id
JOIN wallets wallet ON wallet.user_id = rw.inviter_user_id
  AND wallet.status = 'active'
JOIN users inviter ON inviter.id = rw.inviter_user_id
  AND inviter.is_active IS TRUE AND inviter.is_deleted IS FALSE
WHERE rw.status IN ('applied', 'reversed')
  AND po.refunded_amount_usd > 0
  AND (
    rw.pending_reversal_amount_usd > 0
    OR (
      po.amount_usd > 0
      AND rw.amount_usd > 0
      AND rw.reversed_amount_usd < ROUND((
        rw.amount_usd * CASE
          WHEN po.refunded_amount_usd >= po.amount_usd THEN 1.0
          ELSE po.refunded_amount_usd / po.amount_usd
        END)::numeric, 8)
    )
  )
ORDER BY rw.updated_at ASC, rw.created_at ASC, rw.id ASC
LIMIT $1
"#,
            )
            .bind(REFERRAL_RECONCILIATION_LIMIT as i64)
            .fetch_all(&backend.pool_clone())
            .await
            .map_err(DataLayerError::postgres)?;
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
  trigger_point, CAST(amount_usd AS DOUBLE PRECISION) AS amount_usd,
  status, wallet_transaction_id, idempotency_key,
  CAST(reversed_amount_usd AS DOUBLE PRECISION) AS reversed_amount_usd,
  CAST(pending_reversal_amount_usd AS DOUBLE PRECISION) AS pending_reversal_amount_usd,
  admin_operator_id, admin_note,
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
  trigger_point, CAST(amount_usd AS DOUBLE PRECISION) AS amount_usd,
  status, wallet_transaction_id, idempotency_key,
  CAST(reversed_amount_usd AS DOUBLE PRECISION) AS reversed_amount_usd,
  CAST(pending_reversal_amount_usd AS DOUBLE PRECISION) AS pending_reversal_amount_usd,
  admin_operator_id, admin_note,
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
  trigger_point, CAST(amount_usd AS DOUBLE PRECISION) AS amount_usd,
  status, wallet_transaction_id, idempotency_key,
  CAST(reversed_amount_usd AS DOUBLE PRECISION) AS reversed_amount_usd,
  CAST(pending_reversal_amount_usd AS DOUBLE PRECISION) AS pending_reversal_amount_usd,
  admin_operator_id, admin_note,
  EXTRACT(EPOCH FROM created_at)::BIGINT AS created_at_unix_secs,
  EXTRACT(EPOCH FROM updated_at)::BIGINT AS updated_at_unix_secs
FROM referral_rewards
WHERE source_order_id = $1
  AND status IN ('applied', 'reversed')
ORDER BY id ASC
"#,
            )
            .bind(order_id)
            .fetch_all(&backend.pool_clone())
            .await
            .map_err(DataLayerError::postgres)?;
            return rows.iter().map(|row| reward_from_row!(row)).collect();
        }
        Ok(Vec::new())
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
            let row = sqlx::query(REFERRAL_REFUND_CONTEXT_SQL)
                .bind(order_id)
                .fetch_optional(&backend.pool_clone())
                .await
                .map_err(DataLayerError::postgres)?;
            return row.map(payment_order_refund_context_from_row).transpose();
        }
        Ok(None)
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
        Ok(false)
    }

    async fn recover_applying_referral_reward(
        &self,
        reward_id: &str,
    ) -> Result<ReferralApplyingRecovery, DataLayerError> {
        #[cfg(feature = "postgres")]
        if let Some(backend) = self.backends.and_then(DataBackends::postgres) {
            let mut tx = backend
                .pool_clone()
                .begin()
                .await
                .map_err(DataLayerError::postgres)?;
            let reward = sqlx::query(
                r#"
SELECT id, inviter_user_id, CAST(amount_usd AS DOUBLE PRECISION) AS amount_usd
FROM referral_rewards
WHERE id = $1 AND status = 'applying'
FOR UPDATE
"#,
            )
            .bind(reward_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(DataLayerError::postgres)?;
            if reward.is_none() {
                tx.commit().await.map_err(DataLayerError::postgres)?;
                return Ok(ReferralApplyingRecovery::Unchanged);
            }

            let wallet_transactions = sqlx::query(
                r#"
SELECT tx.id,
       CAST(tx.amount AS DOUBLE PRECISION) AS amount,
       CAST(tx.balance_before AS DOUBLE PRECISION) AS balance_before,
       CAST(tx.balance_after AS DOUBLE PRECISION) AS balance_after,
       CAST(tx.recharge_balance_before AS DOUBLE PRECISION) AS recharge_balance_before,
       CAST(tx.recharge_balance_after AS DOUBLE PRECISION) AS recharge_balance_after,
       CAST(tx.gift_balance_before AS DOUBLE PRECISION) AS gift_balance_before,
       CAST(tx.gift_balance_after AS DOUBLE PRECISION) AS gift_balance_after
FROM wallet_transactions tx
JOIN wallets wallet ON wallet.id = tx.wallet_id
WHERE tx.category = 'adjust'
  AND tx.reason_code = 'referral_reward'
  AND tx.link_type = 'referral_reward'
  AND tx.link_id = $1
  AND wallet.user_id = (SELECT inviter_user_id FROM referral_rewards WHERE id = $1)
  AND tx.amount > 0
ORDER BY tx.created_at ASC, tx.id ASC
LIMIT 32
"#,
            )
            .bind(reward_id)
            .fetch_all(&mut *tx)
            .await
            .map_err(DataLayerError::postgres)?;
            let reward_amount = reward
                .as_ref()
                .and_then(|row| row.try_get::<f64, _>("amount_usd").ok())
                .unwrap_or(0.0);
            let has_wallet_transaction = !wallet_transactions.is_empty();
            let valid_wallet_transaction_ids = wallet_transactions
                .into_iter()
                .filter_map(|row| {
                    let amount = row.try_get::<f64, _>("amount").ok()?;
                    let balance_before = row.try_get::<f64, _>("balance_before").ok()?;
                    let balance_after = row.try_get::<f64, _>("balance_after").ok()?;
                    let recharge_balance_before =
                        row.try_get::<f64, _>("recharge_balance_before").ok()?;
                    let recharge_balance_after =
                        row.try_get::<f64, _>("recharge_balance_after").ok()?;
                    let gift_balance_before = row.try_get::<f64, _>("gift_balance_before").ok()?;
                    let gift_balance_after = row.try_get::<f64, _>("gift_balance_after").ok()?;
                    if !referral_credit_transaction_fact_valid(
                        reward_amount,
                        amount,
                        balance_before,
                        balance_after,
                        recharge_balance_before,
                        recharge_balance_after,
                        gift_balance_before,
                        gift_balance_after,
                    ) {
                        return None;
                    }
                    row.try_get::<String, _>("id").ok()
                })
                .collect::<Vec<_>>();
            // Exactly one valid transaction fact is required.  If multiple
            // facts match the same reward, the historical write may already
            // have credited the wallet twice; silently choosing the first
            // would hide that ambiguity and make the ledger unreconcilable.
            let wallet_transaction_id = (valid_wallet_transaction_ids.len() == 1)
                .then(|| valid_wallet_transaction_ids[0].clone());
            let recovery = if !reward_amount.is_finite() || reward_amount <= 0.0 {
                // A malformed durable amount must never enter the normal
                // failed-reward retry path.  Leave it for operator repair,
                // just like an ambiguous wallet snapshot.
                ReferralApplyingRecovery::Unchanged
            } else if wallet_transaction_id.is_some() {
                ReferralApplyingRecovery::Applied
            } else if has_wallet_transaction {
                // A matching transaction whose durable snapshot is malformed
                // is evidence of an ambiguous historical write.  Retrying it
                // as a normal failed reward could credit the inviter twice.
                // Keep the row applying until an operator repairs the fact.
                ReferralApplyingRecovery::Unchanged
            } else {
                ReferralApplyingRecovery::Failed
            };
            if recovery == ReferralApplyingRecovery::Unchanged {
                // `applying` rows are processed in a bounded queue.  Bump the
                // retry timestamp for ambiguous facts so one permanently
                // malformed row cannot occupy the oldest page forever.
                sqlx::query(
                    "UPDATE referral_rewards SET updated_at = GREATEST(updated_at + INTERVAL '1 microsecond', NOW()) WHERE id = $1 AND status = 'applying'",
                )
                .bind(reward_id)
                .execute(&mut *tx)
                .await
                .map_err(DataLayerError::postgres)?;
                tx.commit().await.map_err(DataLayerError::postgres)?;
                return Ok(recovery);
            }
            let status = match recovery {
                ReferralApplyingRecovery::Applied => "applied",
                ReferralApplyingRecovery::Failed => "failed",
                ReferralApplyingRecovery::Unchanged => unreachable!(),
            };
            sqlx::query(
                r#"
UPDATE referral_rewards
SET status = $2,
    wallet_transaction_id = $3,
    updated_at = NOW()
WHERE id = $1 AND status = 'applying'
"#,
            )
            .bind(reward_id)
            .bind(status)
            .bind(wallet_transaction_id.as_deref())
            .execute(&mut *tx)
            .await
            .map_err(DataLayerError::postgres)?;
            tx.commit().await.map_err(DataLayerError::postgres)?;
            return Ok(recovery);
        }
        Ok(ReferralApplyingRecovery::Unchanged)
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
  rw.id, rw.inviter_user_id, rw.invitee_user_id,
  CAST(rw.amount_usd AS DOUBLE PRECISION) AS amount_usd,
  rw.reward_type,
  rw.trigger_point, wallets.id AS wallet_id
FROM referral_rewards rw
JOIN wallets ON wallets.user_id = rw.inviter_user_id
JOIN users inviter ON inviter.id = rw.inviter_user_id
  AND inviter.is_active IS TRUE AND inviter.is_deleted IS FALSE
WHERE rw.idempotency_key = $1
  AND rw.status IN ('pending', 'failed')
  AND wallets.status = 'active'
"#,
            )
            .bind(idempotency_key)
            .fetch_optional(&backend.pool_clone())
            .await
            .map_err(DataLayerError::postgres)?;
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
        if !target.amount_usd.is_finite() || target.amount_usd <= 0.0 {
            return Err(DataLayerError::InvalidInput(
                "referral reward amount must be finite and greater than zero".to_string(),
            ));
        }
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
RETURNING source_order_id, CAST(amount_usd AS DOUBLE PRECISION) AS amount_usd,
          CAST(reversed_amount_usd AS DOUBLE PRECISION) AS reversed_amount_usd,
          CAST(pending_reversal_amount_usd AS DOUBLE PRECISION) AS pending_reversal_amount_usd
"#,
            )
            .bind(&target.id)
            .bind(operator_id)
            .bind(note)
            .fetch_optional(&mut *tx)
            .await
            .map_err(DataLayerError::postgres)?;
            let Some(claimed) = claimed else {
                tx.commit().await.map_err(DataLayerError::postgres)?;
                return Ok(());
            };
            let reward_amount = row_f64!(claimed, "amount_usd");
            if !referral_amounts_match(reward_amount, target.amount_usd)
                || row_f64!(claimed, "reversed_amount_usd") != 0.0
                || row_f64!(claimed, "pending_reversal_amount_usd") != 0.0
            {
                return Err(DataLayerError::InvalidInput(
                    "pending referral reward state is invalid".into(),
                ));
            }
            let wallet = sqlx::query(
                r#"
SELECT CAST(wallets.balance AS DOUBLE PRECISION) AS balance,
       CAST(wallets.gift_balance AS DOUBLE PRECISION) AS gift_balance,
       CAST(wallets.total_adjusted AS DOUBLE PRECISION) AS total_adjusted
FROM wallets
JOIN users inviter ON inviter.id = wallets.user_id
  AND inviter.is_active IS TRUE AND inviter.is_deleted IS FALSE
WHERE wallets.id = $1
  AND wallets.status = 'active'
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
            let total_adjusted_before = row_f64!(wallet, "total_adjusted");
            if !referral_wallet_values_valid(balance, gift_before)
                || !total_adjusted_before.is_finite()
            {
                return Err(DataLayerError::InvalidInput(
                    "inviter wallet balance is invalid".to_string(),
                ));
            }
            // Keep reward -> inviter wallet -> source order aligned with reversal.
            // Read already successful refunds before exposing any new gift balance.
            // A processing refund is reconciled normally once it succeeds later.
            let refund_context =
                if let Some(source_order_id) = row_optional_string!(claimed, "source_order_id") {
                    let row = sqlx::query(&format!("{REFERRAL_REFUND_CONTEXT_SQL} FOR UPDATE"))
                        .bind(source_order_id)
                        .fetch_optional(&mut *tx)
                        .await
                        .map_err(DataLayerError::postgres)?;
                    Some(
                        row.map(payment_order_refund_context_from_row)
                            .transpose()?
                            .ok_or_else(|| {
                                DataLayerError::InvalidInput(
                                    "referral payment source is missing".into(),
                                )
                            })?,
                    )
                } else {
                    None
                };
            let reverse_at_credit =
                referral_pending_credit_reversal(reward_amount, refund_context.as_ref())?;
            let total_before = balance + gift_before;
            let gift_after = gift_before + reward_amount;
            let total_after = balance + gift_after;
            let total_adjusted_after = total_adjusted_before + reward_amount;
            if !gift_after.is_finite()
                || !total_before.is_finite()
                || !total_after.is_finite()
                || !total_adjusted_after.is_finite()
            {
                return Err(DataLayerError::InvalidInput(
                    "inviter wallet balance overflowed".to_string(),
                ));
            }
            // Subtract the reward components first: a fully refunded large
            // reward must leave the inviter's existing small balance unchanged.
            let net_gift_after =
                referral_net_credit_balance(gift_before, reward_amount, reverse_at_credit);
            let net_adjusted_after = referral_net_credit_balance(
                total_adjusted_before,
                reward_amount,
                reverse_at_credit,
            );
            let description = note
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| reward_description(&target));
            sqlx::query(
                r#"
UPDATE wallets
SET gift_balance = $2,
    total_adjusted = $3,
    updated_at = NOW()
WHERE id = $1
"#,
            )
            .bind(&target.wallet_id)
            .bind(net_gift_after)
            .bind(net_adjusted_after)
            .execute(&mut *tx)
            .await
            .map_err(DataLayerError::postgres)?;
            let tx_id = insert_referral_gift_ledger(
                &mut tx,
                &target.wallet_id,
                &target.id,
                "referral_reward",
                reward_amount,
                balance,
                gift_before,
                gift_after,
                operator_id,
                &description,
            )
            .await?;
            if reverse_at_credit > 0.0 {
                insert_referral_gift_ledger(
                    &mut tx,
                    &target.wallet_id,
                    &target.id,
                    "referral_reward_reversal",
                    -reverse_at_credit,
                    balance,
                    gift_after,
                    net_gift_after,
                    operator_id,
                    "邀请返利退款冲回",
                )
                .await?;
            }
            sqlx::query(
                r#"
UPDATE referral_rewards
SET status = CASE WHEN $5 >= amount_usd THEN 'reversed' ELSE 'applied' END,
    reversed_amount_usd = $5, pending_reversal_amount_usd = 0,
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
            .bind(reverse_at_credit)
            .execute(&mut *tx)
            .await
            .map_err(DataLayerError::postgres)?;
            tx.commit().await.map_err(DataLayerError::postgres)?;
            return Ok(());
        }
        Ok(())
    }

    async fn apply_referral_reward_reversal(
        &self,
        reward: &ReferralRewardRecord,
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
SELECT status,
       inviter_user_id,
       source_order_id,
       CAST(amount_usd AS DOUBLE PRECISION) AS amount_usd,
       CAST(reversed_amount_usd AS DOUBLE PRECISION) AS reversed_amount_usd,
       CAST(pending_reversal_amount_usd AS DOUBLE PRECISION) AS pending_reversal_amount_usd
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
            let reward_status = row_string!(reward_row, "status");
            if !matches!(reward_status.as_str(), "applied" | "reversed") {
                tx.commit().await.map_err(DataLayerError::postgres)?;
                return Ok(());
            }
            let Some(source_order_id) = row_optional_string!(reward_row, "source_order_id") else {
                // Registration/headcount rewards have no payment source and
                // therefore can never be authorized for a refund reversal.
                tx.commit().await.map_err(DataLayerError::postgres)?;
                return Ok(());
            };
            let inviter_user_id = row_string!(reward_row, "inviter_user_id");
            let reward_amount = row_f64!(reward_row, "amount_usd");
            let current_reversed = row_f64!(reward_row, "reversed_amount_usd");
            let current_pending = row_f64!(reward_row, "pending_reversal_amount_usd");

            // Keep the lock order aligned with the wallet refund path
            // (wallet -> payment order).  The order is re-read after its row
            // lock, so a refund committed after the caller's candidate query
            // cannot leave this reversal using an obsolete target amount.
            let wallet = sqlx::query(
                r#"
SELECT wallets.id,
       CAST(wallets.balance AS DOUBLE PRECISION) AS balance,
       CAST(wallets.gift_balance AS DOUBLE PRECISION) AS gift_balance,
       CAST(wallets.total_adjusted AS DOUBLE PRECISION) AS total_adjusted
FROM wallets
JOIN users inviter ON inviter.id = wallets.user_id
  AND inviter.is_active IS TRUE AND inviter.is_deleted IS FALSE
WHERE wallets.user_id = $1
  AND wallets.status = 'active'
FOR UPDATE
"#,
            )
            .bind(&inviter_user_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(DataLayerError::postgres)?;
            let order_row = sqlx::query(&format!("{REFERRAL_REFUND_CONTEXT_SQL} FOR UPDATE"))
                .bind(&source_order_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(DataLayerError::postgres)?;
            let Some(order_row) = order_row else {
                tx.commit().await.map_err(DataLayerError::postgres)?;
                return Ok(());
            };
            let refund_context = payment_order_refund_context_from_row(order_row)?;
            if !referral_refund_context_valid(&refund_context) {
                tx.commit().await.map_err(DataLayerError::postgres)?;
                return Ok(());
            }
            let target_reversal_amount_usd = referral_reversal_target(
                reward_amount,
                refund_context.amount_usd,
                refund_context.refunded_amount_usd,
            );
            if !referral_reversal_inputs_valid(
                reward_amount,
                target_reversal_amount_usd,
                current_reversed,
                current_pending,
            ) {
                return Err(DataLayerError::InvalidInput(
                    "referral reversal state is invalid".to_string(),
                ));
            }
            let amount_usd = referral_reversal_due_bounded(
                target_reversal_amount_usd,
                reward_amount,
                current_reversed,
                current_pending,
            );
            if amount_usd <= 0.0 {
                tx.commit().await.map_err(DataLayerError::postgres)?;
                return Ok(());
            }
            let Some(wallet) = wallet else {
                // Keep the unrecovered amount durable even when the inviter
                // wallet is temporarily absent/inactive. A later
                // reconciliation pass can consume it after the wallet is
                // restored.
                sqlx::query(
                    r#"
UPDATE referral_rewards
SET pending_reversal_amount_usd = $2,
    status = CASE
      WHEN status = 'reversed' THEN 'applied'
      ELSE status
    END,
    updated_at = NOW()
WHERE id = $1
"#,
                )
                .bind(&reward.id)
                .bind(referral_pending_reversal_capped(
                    reward_amount,
                    current_reversed,
                    current_pending,
                    amount_usd,
                ))
                .execute(&mut *tx)
                .await
                .map_err(DataLayerError::postgres)?;
                tx.commit().await.map_err(DataLayerError::postgres)?;
                return Ok(());
            };
            let wallet_id = row_string!(wallet, "id");
            let balance = row_f64!(wallet, "balance");
            let gift_before = row_f64!(wallet, "gift_balance");
            let total_adjusted_before = row_f64!(wallet, "total_adjusted");
            if !referral_wallet_values_valid(balance, gift_before)
                || !total_adjusted_before.is_finite()
            {
                return Err(DataLayerError::InvalidInput(
                    "inviter wallet balance is invalid".to_string(),
                ));
            }
            let actual_reverse = gift_before.max(0.0).min(amount_usd);
            let pending_reverse = (amount_usd - actual_reverse).max(0.0);
            let gift_after = gift_before - actual_reverse;
            let total_before = balance + gift_before;
            let total_after = balance + gift_after;
            let total_adjusted_after = total_adjusted_before - actual_reverse;
            if !actual_reverse.is_finite()
                || !pending_reverse.is_finite()
                || !gift_after.is_finite()
                || !total_before.is_finite()
                || !total_after.is_finite()
                || !total_adjusted_after.is_finite()
                || !referral_reversal_state_valid(
                    reward_amount,
                    current_reversed,
                    current_pending,
                    actual_reverse,
                    pending_reverse,
                )
            {
                return Err(DataLayerError::InvalidInput(
                    "inviter wallet balance overflowed".to_string(),
                ));
            }
            if actual_reverse > 0.0 {
                sqlx::query(
                    r#"
UPDATE wallets
SET gift_balance = $2,
    total_adjusted = $3,
    updated_at = NOW()
WHERE id = $1
"#,
                )
                .bind(&wallet_id)
                .bind(gift_after)
                .bind(total_adjusted_after)
                .execute(&mut *tx)
                .await
                .map_err(DataLayerError::postgres)?;
                insert_referral_gift_ledger(
                    &mut tx,
                    &wallet_id,
                    &reward.id,
                    "referral_reward_reversal",
                    -actual_reverse,
                    balance,
                    gift_before,
                    gift_after,
                    None,
                    "邀请返利退款冲回",
                )
                .await?;
            }
            sqlx::query(
                r#"
UPDATE referral_rewards
SET reversed_amount_usd = reversed_amount_usd + $2,
    pending_reversal_amount_usd = $3,
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
            sqlx::query(
                r#"
UPDATE referral_rewards
SET status = CASE
      WHEN pending_reversal_amount_usd > 0 AND status = 'reversed' THEN 'applied'
      WHEN pending_reversal_amount_usd = 0
        AND reversed_amount_usd >= amount_usd
        AND status IN ('applied', 'reversed') THEN 'reversed'
      ELSE status
    END,
    updated_at = NOW()
WHERE id = $1
"#,
            )
            .bind(&reward.id)
            .execute(&mut *tx)
            .await
            .map_err(DataLayerError::postgres)?;
            tx.commit().await.map_err(DataLayerError::postgres)?;
            return Ok(());
        }
        Ok(())
    }
}

/// An unissued reward funds its own already-due reversal. Both ledgers are
/// committed together, so only the net gift becomes available for consumption.
fn referral_net_credit_balance(before: f64, reward: f64, reverse: f64) -> f64 {
    before + (reward - reverse)
}

fn referral_pending_credit_reversal(
    amount: f64,
    context: Option<&ReferralPaymentOrderRefundContext>,
) -> Result<f64, DataLayerError> {
    if !amount.is_finite() || amount <= 0.0 {
        return Err(DataLayerError::InvalidInput(
            "referral reward amount is invalid".into(),
        ));
    }
    let Some(context) = context else {
        return Ok(0.0);
    };
    if !payment_order_refund_amounts_are_consistent(
        context.amount_usd,
        context.refunded_amount_usd,
        (context.amount_usd - context.refunded_amount_usd).max(0.0),
    ) {
        return Err(DataLayerError::InvalidInput(
            "referral refund context is invalid".into(),
        ));
    }
    Ok(referral_reversal_target(
        amount,
        context.amount_usd,
        context.refunded_amount_usd,
    ))
}

fn referral_signed_gift_fact_valid(
    amount: f64,
    recharge: f64,
    gift_before: f64,
    gift_after: f64,
) -> bool {
    let before = recharge + gift_before;
    let after = recharge + gift_after;
    // A reversal must satisfy the same complete gift-only snapshot invariant
    // as a credit with its before/after values exchanged.
    if amount > 0.0 {
        referral_credit_transaction_fact_valid(
            amount,
            amount,
            before,
            after,
            recharge,
            recharge,
            gift_before,
            gift_after,
        )
    } else {
        referral_credit_transaction_fact_valid(
            -amount,
            -amount,
            after,
            before,
            recharge,
            recharge,
            gift_after,
            gift_before,
        )
    }
}

#[cfg(feature = "postgres")]
#[allow(clippy::too_many_arguments)]
async fn insert_referral_gift_ledger(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    wallet_id: &str,
    reward_id: &str,
    reason: &str,
    amount: f64,
    recharge: f64,
    gift_before: f64,
    gift_after: f64,
    operator_id: Option<&str>,
    description: &str,
) -> Result<String, DataLayerError> {
    let before = recharge + gift_before;
    let after = recharge + gift_after;
    let valid = referral_signed_gift_fact_valid(amount, recharge, gift_before, gift_after);
    if !valid {
        return Err(DataLayerError::InvalidInput(
            "referral wallet ledger snapshot is invalid".into(),
        ));
    }
    let id = uuid::Uuid::new_v4().to_string();
    sqlx::query(
        r#"
INSERT INTO wallet_transactions (id, wallet_id, category, reason_code, amount,
 balance_before, balance_after, recharge_balance_before, recharge_balance_after,
 gift_balance_before, gift_balance_after, link_type, link_id, operator_id, description, created_at)
VALUES ($1,$2,'adjust',$3,$4,$5,$6,$7,$7,$8,$9,'referral_reward',$10,$11,$12,NOW())
"#,
    )
    .bind(&id)
    .bind(wallet_id)
    .bind(reason)
    .bind(amount)
    .bind(before)
    .bind(after)
    .bind(recharge)
    .bind(gift_before)
    .bind(gift_after)
    .bind(reward_id)
    .bind(operator_id)
    .bind(description)
    .execute(&mut **tx)
    .await
    .map_err(DataLayerError::postgres)?;
    Ok(id)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_referral_credit_nets_known_refunds_with_valid_signed_ledgers() {
        for (refund, expected) in [(0.0, 0.0), (40.0, 4.0), (100.0, 10.0)] {
            let context = ReferralPaymentOrderRefundContext {
                amount_usd: 100.0,
                refunded_amount_usd: refund,
            };
            let reverse = referral_pending_credit_reversal(10.0, Some(&context)).unwrap();
            assert_eq!(reverse, expected);
            let gross_gift = 1.0 + 10.0;
            let net_gift = referral_net_credit_balance(1.0, 10.0, reverse);
            assert!(referral_signed_gift_fact_valid(10.0, 2.0, 1.0, gross_gift));
            if reverse > 0.0 {
                assert!(referral_signed_gift_fact_valid(
                    -reverse, 2.0, gross_gift, net_gift
                ));
            }
            assert_eq!(
                referral_reversal_due_bounded(expected, 10.0, reverse, 0.0),
                0.0
            );
        }
        assert!(!referral_signed_gift_fact_valid(-4.0, 2.0, 11.0, 8.0));
        assert!(referral_pending_credit_reversal(
            10.0,
            Some(&ReferralPaymentOrderRefundContext {
                amount_usd: 100.0,
                refunded_amount_usd: 101.0
            })
        )
        .is_err());
        assert_eq!(referral_pending_credit_reversal(3.0, None).unwrap(), 0.0);
    }

    #[test]
    fn pending_referral_credit_preserves_small_balance_for_large_full_refund() {
        let amount = 999_999_999_999.0;
        let reverse = referral_pending_credit_reversal(
            amount,
            Some(&ReferralPaymentOrderRefundContext {
                amount_usd: amount,
                refunded_amount_usd: amount,
            }),
        )
        .unwrap();
        assert_eq!(referral_net_credit_balance(0.1, amount, reverse), 0.1);
        assert_eq!(referral_reversal_target(0.00000002, 3.0, 1.0), 0.00000001);
        assert_eq!(referral_reversal_target(0.00000001, 3.0, 1.0), 0.0);
        assert_eq!(referral_reversal_target(0.00000003, 1.0, 0.5), 0.00000002);
        assert_eq!(referral_reversal_target(0.00000001, 1.0, 0.5), 0.00000001);
        assert_eq!(
            referral_reversal_target(0.00000003, 1.0, 0.49999999),
            0.00000001
        );
    }

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

        // A previously deferred reversal remains due until a later pass can
        // consume the inviter's replenished gift balance.
        let repeated = referral_reversal_delta(10.0, 100.0, 50.0, 2.0, 3.0);
        assert!((repeated - 3.0).abs() < f64::EPSILON);

        let increased_target = referral_reversal_delta(10.0, 100.0, 80.0, 2.0, 3.0);
        assert!((increased_target - 6.0).abs() < f64::EPSILON);

        let full = referral_reversal_delta(10.0, 100.0, 125.0, 5.0, 0.0);
        assert!((full - 5.0).abs() < f64::EPSILON);
    }

    #[test]
    fn referral_reversal_target_rejects_non_finite_amounts() {
        assert_eq!(referral_reversal_target(f64::NAN, 100.0, 10.0), 0.0);
        assert_eq!(referral_reversal_target(10.0, f64::INFINITY, 10.0), 0.0);
        assert_eq!(
            referral_reversal_target(10.0, 100.0, f64::NEG_INFINITY),
            0.0
        );
    }

    #[test]
    fn referral_reversal_due_never_exceeds_current_refund_or_reward() {
        // A legacy additive pending value may be larger than the current
        // target. It must not authorize an over-reversal beyond the reward.
        assert_eq!(referral_reversal_due_bounded(5.0, 10.0, 0.0, 15.0), 10.0);
        assert_eq!(referral_reversal_due_bounded(5.0, 10.0, 10.0, 3.0), 0.0);
        // A malformed target above the reward is still capped by the reward
        // remainder.
        assert_eq!(referral_reversal_due_bounded(20.0, 10.0, 2.0, 3.0), 8.0);
    }

    #[test]
    fn referral_percent_rate_must_be_finite_and_at_most_one_hundred() {
        assert!(referral_percent_rate_valid(0.01));
        assert!(referral_percent_rate_valid(100.0));
        for value in [0.0, -1.0, 100.000001, f64::NAN, f64::INFINITY] {
            assert!(
                !referral_percent_rate_valid(value),
                "{value:?} must be rejected"
            );
        }
    }

    #[test]
    fn referral_payment_method_exclusion_is_case_and_whitespace_insensitive() {
        for method in [
            "manual",
            "MANUAL",
            " Admin_Manual ",
            "REDEEM_CODE",
            " Gift\t",
        ] {
            assert!(
                referral_payment_method_excluded(method),
                "{method:?} must be excluded"
            );
        }
        for method in ["stripe", "paypal", "manual_review"] {
            assert!(
                !referral_payment_method_excluded(method),
                "{method:?} must remain eligible"
            );
        }
    }

    #[test]
    fn referral_wallet_values_allow_overdraft_but_reject_invalid_gifts() {
        assert!(referral_wallet_values_valid(-25.0, 3.0));
        assert!(!referral_wallet_values_valid(f64::NAN, 3.0));
        assert!(!referral_wallet_values_valid(1.0, f64::INFINITY));
        assert!(!referral_wallet_values_valid(1.0, -0.01));
    }

    #[test]
    fn referral_refund_context_rejects_amounts_outside_order_total() {
        assert!(referral_refund_context_valid(
            &ReferralPaymentOrderRefundContext {
                amount_usd: 100.0,
                refunded_amount_usd: 25.0,
            }
        ));
        assert!(!referral_refund_context_valid(
            &ReferralPaymentOrderRefundContext {
                amount_usd: 100.0,
                refunded_amount_usd: 100.00001,
            }
        ));
        assert!(!referral_refund_context_valid(
            &ReferralPaymentOrderRefundContext {
                amount_usd: 100.0,
                refunded_amount_usd: -1.0,
            }
        ));
        assert!(!referral_refund_context_valid(
            &ReferralPaymentOrderRefundContext {
                amount_usd: f64::NAN,
                refunded_amount_usd: 1.0,
            }
        ));
    }

    #[test]
    fn referral_credit_fact_requires_a_consistent_gift_only_delta() {
        assert!(referral_credit_transaction_fact_valid(
            3.0, 3.0, 2.0, 5.0, -1.0, -1.0, 3.0, 6.0,
        ));
        // Matching amount/link metadata alone must not be trusted.
        assert!(!referral_credit_transaction_fact_valid(
            3.0, 3.0, 2.0, 5.0, -1.0, -1.0, 3.0, 3.0,
        ));
        assert!(!referral_credit_transaction_fact_valid(
            3.0, 3.0, 2.0, 5.0, -1.0, 0.0, 3.0, 6.0,
        ));
        assert!(!referral_credit_transaction_fact_valid(
            3.0, 3.0, 2.0, 5.0, -1.0, -1.0, -1.0, 2.0,
        ));
        assert!(!referral_credit_transaction_fact_valid(
            3.0,
            f64::NAN,
            2.0,
            5.0,
            -1.0,
            -1.0,
            3.0,
            6.0,
        ));
    }

    #[test]
    fn referral_reversal_state_rejects_invalid_or_overflowing_totals() {
        assert!(referral_reversal_state_valid(10.0, 2.0, 3.0, 1.0, 4.0));
        assert!(!referral_reversal_state_valid(10.0, -1.0, 0.0, 1.0, 0.0));
        assert!(!referral_reversal_state_valid(10.0, 2.0, 3.0, 9.0, 0.0));
        assert!(!referral_reversal_state_valid(
            f64::MAX,
            f64::MAX,
            0.0,
            f64::MAX,
            0.0,
        ));
    }

    #[test]
    fn referral_reversal_inputs_reject_malformed_durable_counters() {
        assert!(referral_reversal_inputs_valid(10.0, 5.0, 2.0, 3.0));
        assert!(!referral_reversal_inputs_valid(10.0, 5.0, -1.0, 0.0));
        assert!(!referral_reversal_inputs_valid(10.0, 5.0, 2.0, -1.0));
        assert!(!referral_reversal_inputs_valid(10.0, -0.1, 2.0, 3.0));
        assert!(!referral_reversal_inputs_valid(10.0, 5.0, 8.0, 3.0));
        assert!(!referral_reversal_inputs_valid(10.0, 11.0, 0.0, 0.0));
        assert!(!referral_reversal_inputs_valid(f64::NAN, 1.0, 0.0, 0.0,));
    }

    #[test]
    fn referral_pending_reversal_is_capped_at_remaining_reward() {
        assert_eq!(referral_pending_reversal_capped(10.0, 2.0, 15.0, 3.0), 8.0);
        assert_eq!(referral_pending_reversal_capped(10.0, 2.0, 1.0, 3.0), 3.0);
    }

    #[test]
    fn referral_stats_amount_saturates_overflow_without_hiding_it() {
        assert_eq!(referral_stats_amount(f64::INFINITY), f64::MAX);
        assert_eq!(referral_stats_amount(f64::NEG_INFINITY), 0.0);
        assert_eq!(referral_stats_amount(f64::NAN), 0.0);
        assert_eq!(referral_stats_amount(-1.0), 0.0);
        assert_eq!(referral_stats_amount(12.5), 12.5);
    }

    #[test]
    fn referral_like_pattern_escapes_wildcards_and_uses_empty_filter_sentinel() {
        assert_eq!(referral_like_pattern(None), "");
        assert_eq!(referral_like_pattern(Some("  ")), "");
        assert_eq!(referral_like_pattern(Some(" A_%! ")), "%a!_!%!!%");
    }

    #[test]
    fn referral_page_bounds_clamp_limit_and_saturate_offset() {
        assert_eq!(referral_page_bounds(0, 0), (1, 0));
        assert_eq!(referral_page_bounds(999, 4), (200, 4));
        assert_eq!(referral_page_bounds(20, usize::MAX), (20, i64::MAX));
    }
}
