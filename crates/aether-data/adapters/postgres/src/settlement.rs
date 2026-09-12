use async_trait::async_trait;
use sqlx::{PgPool, Postgres, QueryBuilder, Row};

use aether_data_contracts::repository::settlement::{
    finite_wallet_available_usd, plan_finite_wallet_debit, settlement_billable_cost_usd,
    settlement_billing_status_for_usage_status, validate_wallet_settlement_values,
    ReconcileUsagePolicyCostInput, ReleaseUsagePolicyRequestAdmissionInput,
    ReserveUsagePolicyCostInput, ReserveUsagePolicyCostOutcome, ReserveUsagePolicyRequestInput,
    ReserveUsagePolicyRequestOutcome, SettlementWriteRepository, StoredUsagePolicyCostReservation,
    StoredUsagePolicyRequestAdmission, StoredUsageSettlement, UsagePolicyCostReservationState,
    UsagePolicyRequestAdmissionState, UsageSettlementInput, SETTLEMENT_EPSILON_USD,
};
use aether_data_contracts::DataLayerError;

use crate::error::SqlxResultExt;
use crate::PostgresTransactionRunner;

const FIND_USAGE_FOR_SETTLEMENT_SQL: &str = r#"
SELECT
  usage_record.request_id,
  COALESCE(usage_settlement_snapshots.wallet_id, usage_record.wallet_id) AS wallet_id,
  COALESCE(usage_settlement_snapshots.billing_status, usage_record.billing_status) AS billing_status,
  COALESCE(
    CAST(usage_settlement_snapshots.wallet_balance_before AS DOUBLE PRECISION),
    CAST(usage_record.wallet_balance_before AS DOUBLE PRECISION)
  ) AS wallet_balance_before,
  COALESCE(
    CAST(usage_settlement_snapshots.wallet_balance_after AS DOUBLE PRECISION),
    CAST(usage_record.wallet_balance_after AS DOUBLE PRECISION)
  ) AS wallet_balance_after,
  COALESCE(
    CAST(usage_settlement_snapshots.wallet_recharge_balance_before AS DOUBLE PRECISION),
    CAST(usage_record.wallet_recharge_balance_before AS DOUBLE PRECISION)
  ) AS wallet_recharge_balance_before,
  COALESCE(
    CAST(usage_settlement_snapshots.wallet_recharge_balance_after AS DOUBLE PRECISION),
    CAST(usage_record.wallet_recharge_balance_after AS DOUBLE PRECISION)
  ) AS wallet_recharge_balance_after,
  COALESCE(
    CAST(usage_settlement_snapshots.wallet_gift_balance_before AS DOUBLE PRECISION),
    CAST(usage_record.wallet_gift_balance_before AS DOUBLE PRECISION)
  ) AS wallet_gift_balance_before,
  COALESCE(
    CAST(usage_settlement_snapshots.wallet_gift_balance_after AS DOUBLE PRECISION),
    CAST(usage_record.wallet_gift_balance_after AS DOUBLE PRECISION)
  ) AS wallet_gift_balance_after,
  CAST(usage_settlement_snapshots.provider_monthly_used_usd AS DOUBLE PRECISION) AS provider_monthly_used_usd,
  usage_record.provider_id,
  CAST(
    EXTRACT(
      EPOCH FROM COALESCE(usage_settlement_snapshots.finalized_at, usage_record.finalized_at)
    ) AS BIGINT
  ) AS finalized_at_unix_secs
FROM "usage" AS usage_record
LEFT JOIN usage_settlement_snapshots
  ON usage_settlement_snapshots.request_id = usage_record.request_id
WHERE usage_record.request_id = $1
FOR UPDATE OF usage_record
"#;

const FINALIZE_USAGE_BILLING_SQL: &str = r#"
UPDATE "usage"
SET
  billing_status = $2,
  finalized_at = COALESCE(finalized_at, to_timestamp($3))
WHERE request_id = $1
"#;

const UPSERT_USAGE_SETTLEMENT_SNAPSHOT_SQL: &str = r#"
INSERT INTO usage_settlement_snapshots (
  request_id,
  billing_status,
  wallet_id,
  wallet_balance_before,
  wallet_balance_after,
  wallet_recharge_balance_before,
  wallet_recharge_balance_after,
  wallet_gift_balance_before,
  wallet_gift_balance_after,
  provider_monthly_used_usd,
  finalized_at
) VALUES (
  $1,
  $2,
  $3,
  $4,
  $5,
  $6,
  $7,
  $8,
  $9,
  $10,
  CASE
    WHEN $11 IS NULL THEN NULL
    ELSE TO_TIMESTAMP($11::double precision)
  END
)
ON CONFLICT (request_id)
DO UPDATE SET
  billing_status = EXCLUDED.billing_status,
  wallet_id = COALESCE(EXCLUDED.wallet_id, usage_settlement_snapshots.wallet_id),
  wallet_balance_before = COALESCE(
    EXCLUDED.wallet_balance_before,
    usage_settlement_snapshots.wallet_balance_before
  ),
  wallet_balance_after = COALESCE(
    EXCLUDED.wallet_balance_after,
    usage_settlement_snapshots.wallet_balance_after
  ),
  wallet_recharge_balance_before = COALESCE(
    EXCLUDED.wallet_recharge_balance_before,
    usage_settlement_snapshots.wallet_recharge_balance_before
  ),
  wallet_recharge_balance_after = COALESCE(
    EXCLUDED.wallet_recharge_balance_after,
    usage_settlement_snapshots.wallet_recharge_balance_after
  ),
  wallet_gift_balance_before = COALESCE(
    EXCLUDED.wallet_gift_balance_before,
    usage_settlement_snapshots.wallet_gift_balance_before
  ),
  wallet_gift_balance_after = COALESCE(
    EXCLUDED.wallet_gift_balance_after,
    usage_settlement_snapshots.wallet_gift_balance_after
  ),
  provider_monthly_used_usd = COALESCE(
    EXCLUDED.provider_monthly_used_usd,
    usage_settlement_snapshots.provider_monthly_used_usd
  ),
  finalized_at = COALESCE(EXCLUDED.finalized_at, usage_settlement_snapshots.finalized_at),
  updated_at = NOW()
"#;

const ENQUEUE_PROVIDER_MONTHLY_USAGE_DELTA_SQL: &str = r#"
INSERT INTO usage_counter_deltas (
  id,
  request_id,
  kind,
  target_id,
  total_cost_usd_delta
) VALUES (
  $1,
  $2,
  'provider_monthly',
  $3,
  $4
)
"#;

#[derive(Debug, Clone)]
pub struct SqlxSettlementRepository {
    tx_runner: PostgresTransactionRunner,
}

impl SqlxSettlementRepository {
    pub fn new(pool: PgPool) -> Self {
        let tx_runner = PostgresTransactionRunner::new(pool);
        Self { tx_runner }
    }
}

fn usage_policy_cost_i64(value: u64, field: &str) -> Result<i64, DataLayerError> {
    i64::try_from(value)
        .map_err(|_| DataLayerError::InvalidInput(format!("{field} exceeds the integer range")))
}

fn usage_policy_cost_u64(value: i64, field: &str) -> Result<u64, DataLayerError> {
    u64::try_from(value)
        .map_err(|_| DataLayerError::UnexpectedValue(format!("{field} must not be negative")))
}

fn usage_policy_request_admission_from_postgres_row(
    row: &sqlx::postgres::PgRow,
) -> Result<StoredUsagePolicyRequestAdmission, DataLayerError> {
    let state: String = row.try_get("state").map_postgres_err()?;
    Ok(StoredUsagePolicyRequestAdmission {
        request_id: row.try_get("request_id").map_postgres_err()?,
        subject_id: row.try_get("subject_id").map_postgres_err()?,
        event_token: row.try_get("event_token").map_postgres_err()?,
        admitted_at_unix_secs: usage_policy_cost_u64(
            row.try_get("admitted_at_unix_secs").map_postgres_err()?,
            "usage policy request admitted_at",
        )?,
        retain_until_unix_secs: usage_policy_cost_u64(
            row.try_get("retain_until_unix_secs").map_postgres_err()?,
            "usage policy request retain_until",
        )?,
        state: UsagePolicyRequestAdmissionState::parse(&state).ok_or_else(|| {
            DataLayerError::UnexpectedValue(format!(
                "unknown usage policy request admission state {state}"
            ))
        })?,
        released_at_unix_secs: row
            .try_get::<Option<i64>, _>("released_at_unix_secs")
            .map_postgres_err()?
            .map(|value| usage_policy_cost_u64(value, "usage policy request released_at"))
            .transpose()?,
    })
}

const FIND_USAGE_POLICY_REQUEST_ADMISSION_POSTGRES_SQL: &str = r#"
SELECT
  request_id,
  subject_id,
  event_token,
  CAST(EXTRACT(EPOCH FROM admitted_at) AS BIGINT) AS admitted_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM retain_until) AS BIGINT) AS retain_until_unix_secs,
  state,
  CAST(EXTRACT(EPOCH FROM released_at) AS BIGINT) AS released_at_unix_secs
FROM usage_request_admissions
WHERE event_token = $1
FOR UPDATE
"#;

fn usage_policy_cost_reservation_from_postgres_row(
    row: &sqlx::postgres::PgRow,
) -> Result<StoredUsagePolicyCostReservation, DataLayerError> {
    let state: String = row.try_get("state").map_postgres_err()?;
    Ok(StoredUsagePolicyCostReservation {
        request_id: row.try_get("request_id").map_postgres_err()?,
        subject_id: row.try_get("subject_id").map_postgres_err()?,
        reservation_token: row.try_get("reservation_token").map_postgres_err()?,
        admitted_at_unix_secs: usage_policy_cost_u64(
            row.try_get("admitted_at_unix_secs").map_postgres_err()?,
            "usage policy admitted_at",
        )?,
        reserved_cost_units: usage_policy_cost_u64(
            row.try_get("reserved_cost_units").map_postgres_err()?,
            "usage policy reserved_cost_units",
        )?,
        actual_cost_units: row
            .try_get::<Option<i64>, _>("actual_cost_units")
            .map_postgres_err()?
            .map(|value| usage_policy_cost_u64(value, "usage policy actual_cost_units"))
            .transpose()?,
        state: UsagePolicyCostReservationState::parse(&state).ok_or_else(|| {
            DataLayerError::UnexpectedValue(format!(
                "unknown usage policy reservation state {state}"
            ))
        })?,
        reservation_expires_at_unix_secs: usage_policy_cost_u64(
            row.try_get("reservation_expires_at_unix_secs")
                .map_postgres_err()?,
            "usage policy reservation_expires_at",
        )?,
        retain_until_unix_secs: usage_policy_cost_u64(
            row.try_get("retain_until_unix_secs").map_postgres_err()?,
            "usage policy retain_until",
        )?,
        finalized_at_unix_secs: row
            .try_get::<Option<i64>, _>("finalized_at_unix_secs")
            .map_postgres_err()?
            .map(|value| usage_policy_cost_u64(value, "usage policy finalized_at"))
            .transpose()?,
    })
}

const FIND_USAGE_POLICY_COST_RESERVATION_POSTGRES_SQL: &str = r#"
SELECT
  request_id,
  subject_id,
  reservation_token,
  CAST(EXTRACT(EPOCH FROM admitted_at) AS BIGINT) AS admitted_at_unix_secs,
  reserved_cost_units,
  actual_cost_units,
  state,
  CAST(EXTRACT(EPOCH FROM reservation_expires_at) AS BIGINT)
    AS reservation_expires_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM retain_until) AS BIGINT) AS retain_until_unix_secs,
  CAST(EXTRACT(EPOCH FROM finalized_at) AS BIGINT) AS finalized_at_unix_secs
FROM usage_cost_reservations
WHERE reservation_token = $1
FOR UPDATE
"#;

async fn lock_usage_policy_subject_postgres(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    subject_id: &str,
) -> Result<bool, DataLayerError> {
    let exists = sqlx::query_scalar::<_, String>(
        r#"
SELECT id
FROM users
WHERE id = $1
FOR UPDATE
        "#,
    )
    .bind(subject_id)
    .fetch_optional(&mut **tx)
    .await
    .map_postgres_err()?
    .is_some();
    Ok(exists)
}

fn usage_policy_subject_missing() -> DataLayerError {
    DataLayerError::InvalidInput("usage policy subject does not exist".to_string())
}

fn usage_policy_window_aggregate_query(
    windows: impl Iterator<Item = (u64, u64)>,
    aggregate: &str,
) -> Result<(QueryBuilder<'static, Postgres>, i64, i64), DataLayerError> {
    let mut builder = QueryBuilder::new("SELECT ");
    let mut earliest = i64::MAX;
    let mut latest = i64::MIN;
    for (index, (start, end)) in windows.enumerate() {
        let start = usage_policy_cost_i64(start, "usage policy window start")?;
        let end = usage_policy_cost_i64(end, "usage policy window end")?;
        earliest = earliest.min(start);
        latest = latest.max(end);
        if index > 0 {
            builder.push(", ");
        }
        builder
            .push("COALESCE(")
            .push(aggregate)
            .push(" FILTER (WHERE admitted_at >= TO_TIMESTAMP(")
            .push_bind(start)
            .push("::double precision) AND admitted_at < TO_TIMESTAMP(")
            .push_bind(end)
            .push("::double precision)), 0)::BIGINT");
    }
    Ok((builder, earliest, latest))
}

async fn usage_policy_request_window_counts(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    input: &ReserveUsagePolicyRequestInput,
) -> Result<sqlx::postgres::PgRow, DataLayerError> {
    let (mut query, earliest, latest) = usage_policy_window_aggregate_query(
        input
            .windows
            .iter()
            .map(|window| (window.starts_at_unix_secs, window.ends_at_unix_secs)),
        "COUNT(*)",
    )?;
    // The subject lock protects all windows. One bounded history scan replaces
    // repeated scans of overlapping windows without approximating their counts.
    query
        .push(" FROM usage_request_admissions WHERE subject_id = ")
        .push_bind(input.subject_id.clone())
        .push(" AND state = 'active' AND admitted_at >= TO_TIMESTAMP(")
        .push_bind(earliest)
        .push("::double precision) AND admitted_at < TO_TIMESTAMP(")
        .push_bind(latest)
        .push("::double precision)");
    query.build().fetch_one(&mut **tx).await.map_postgres_err()
}

async fn usage_policy_cost_window_totals(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    input: &ReserveUsagePolicyCostInput,
) -> Result<sqlx::postgres::PgRow, DataLayerError> {
    let (mut query, earliest, latest) = usage_policy_window_aggregate_query(
        input.windows.iter().map(|window| (window.starts_at_unix_secs, window.ends_at_unix_secs)),
        "SUM(CASE WHEN state = 'finalized' THEN COALESCE(actual_cost_units, 0) ELSE reserved_cost_units END)",
    )?;
    query.push(" FROM usage_cost_reservations WHERE subject_id = ")
        .push_bind(input.subject_id.clone())
        .push(" AND admitted_at >= TO_TIMESTAMP(").push_bind(earliest)
        .push("::double precision) AND admitted_at < TO_TIMESTAMP(").push_bind(latest)
        .push("::double precision) AND reservation_token <> ").push_bind(input.reservation_token.clone())
        .push(" AND (state = 'finalized' OR (state = 'reserved' AND reservation_expires_at > TO_TIMESTAMP(")
        .push_bind(usage_policy_cost_i64(input.admitted_at_unix_secs, "usage policy admitted_at")?)
        .push("::double precision)))");
    query.build().fetch_one(&mut **tx).await.map_postgres_err()
}

fn settlement_from_row(
    row: &sqlx::postgres::PgRow,
) -> Result<StoredUsageSettlement, DataLayerError> {
    Ok(StoredUsageSettlement {
        request_id: row.try_get("request_id").map_postgres_err()?,
        wallet_id: row.try_get("wallet_id").map_postgres_err()?,
        billing_status: row.try_get("billing_status").map_postgres_err()?,
        wallet_balance_before: row.try_get("wallet_balance_before").map_postgres_err()?,
        wallet_balance_after: row.try_get("wallet_balance_after").map_postgres_err()?,
        wallet_recharge_balance_before: row
            .try_get("wallet_recharge_balance_before")
            .map_postgres_err()?,
        wallet_recharge_balance_after: row
            .try_get("wallet_recharge_balance_after")
            .map_postgres_err()?,
        wallet_gift_balance_before: row
            .try_get("wallet_gift_balance_before")
            .map_postgres_err()?,
        wallet_gift_balance_after: row
            .try_get("wallet_gift_balance_after")
            .map_postgres_err()?,
        provider_monthly_used_usd: row
            .try_get("provider_monthly_used_usd")
            .map_postgres_err()?,
        finalized_at_unix_secs: row
            .try_get::<Option<i64>, _>("finalized_at_unix_secs")
            .map_postgres_err()?
            .map(|value| value as u64),
    })
}

async fn sync_usage_settlement_snapshot<'e, E>(
    executor: E,
    settlement: &StoredUsageSettlement,
) -> Result<(), DataLayerError>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    sqlx::query(UPSERT_USAGE_SETTLEMENT_SNAPSHOT_SQL)
        .bind(&settlement.request_id)
        .bind(&settlement.billing_status)
        .bind(settlement.wallet_id.as_deref())
        .bind(settlement.wallet_balance_before)
        .bind(settlement.wallet_balance_after)
        .bind(settlement.wallet_recharge_balance_before)
        .bind(settlement.wallet_recharge_balance_after)
        .bind(settlement.wallet_gift_balance_before)
        .bind(settlement.wallet_gift_balance_after)
        .bind(settlement.provider_monthly_used_usd)
        .bind(settlement.finalized_at_unix_secs.map(|value| value as f64))
        .execute(executor)
        .await
        .map_postgres_err()?;
    Ok(())
}

async fn enqueue_provider_monthly_usage_delta<'e, E>(
    executor: E,
    request_id: &str,
    provider_id: &str,
    total_cost_usd_delta: f64,
) -> Result<(), DataLayerError>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let request_id = request_id.trim();
    let provider_id = provider_id.trim();
    if request_id.is_empty() || provider_id.is_empty() || total_cost_usd_delta == 0.0 {
        return Ok(());
    }
    if !total_cost_usd_delta.is_finite() {
        return Err(DataLayerError::UnexpectedValue(format!(
            "provider monthly usage delta is not finite for {provider_id}"
        )));
    }

    sqlx::query(ENQUEUE_PROVIDER_MONTHLY_USAGE_DELTA_SQL)
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(request_id)
        .bind(provider_id)
        .bind(total_cost_usd_delta)
        .execute(executor)
        .await
        .map_postgres_err()?;
    Ok(())
}

#[derive(Debug, Default)]
struct DailyQuotaDebitResult {
    debited_usd: f64,
    insufficient: bool,
}

#[derive(Debug)]
struct DailyQuotaGrant {
    entitlement_id: String,
    daily_quota_usd: f64,
    usage_date: String,
    allow_wallet_overage: bool,
}

fn daily_quota_usage_date(
    reset_timezone: Option<&str>,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<String, DataLayerError> {
    let timezone = reset_timezone
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("Asia/Shanghai")
        .parse::<chrono_tz::Tz>()
        .map_err(|err| DataLayerError::InvalidInput(format!("invalid reset_timezone: {err}")))?;
    Ok(now.with_timezone(&timezone).date_naive().to_string())
}

fn daily_quota_grants_from_entitlement(
    entitlement_id: &str,
    entitlements: &serde_json::Value,
    current_allow_wallet_overage: Option<bool>,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<Vec<DailyQuotaGrant>, DataLayerError> {
    let mut grants = Vec::new();
    let Some(items) = entitlements.as_array() else {
        return Ok(grants);
    };
    for item in items {
        if item.get("type").and_then(serde_json::Value::as_str) != Some("daily_quota") {
            continue;
        }
        let daily_quota_usd = item
            .get("daily_quota_usd")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0);
        if !daily_quota_usd.is_finite() || daily_quota_usd <= 0.0 {
            continue;
        }
        let usage_date = daily_quota_usage_date(
            item.get("reset_timezone")
                .and_then(serde_json::Value::as_str),
            now,
        )?;
        grants.push(DailyQuotaGrant {
            entitlement_id: entitlement_id.to_string(),
            daily_quota_usd,
            usage_date,
            allow_wallet_overage: current_allow_wallet_overage.unwrap_or_else(|| {
                item.get("allow_wallet_overage")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false)
            }),
        });
    }
    Ok(grants)
}

fn daily_quota_wallet_overage_policy(entitlements: &serde_json::Value) -> Option<bool> {
    entitlements.as_array()?.iter().find_map(|item| {
        (item.get("type").and_then(serde_json::Value::as_str) == Some("daily_quota"))
            .then(|| {
                item.get("allow_wallet_overage")
                    .and_then(serde_json::Value::as_bool)
            })
            .flatten()
    })
}

async fn consume_daily_quota_postgres(
    tx: &mut crate::PostgresTransaction,
    user_id: &str,
    request_id: &str,
    total_cost_usd: f64,
    wallet_available_usd: Option<f64>,
    wallet_can_overdraft: bool,
) -> Result<DailyQuotaDebitResult, DataLayerError> {
    if !total_cost_usd.is_finite() || total_cost_usd < 0.0 {
        return Err(DataLayerError::InvalidInput(
            "daily quota settlement cost must be finite and non-negative".to_string(),
        ));
    }
    if total_cost_usd == 0.0 {
        return Ok(DailyQuotaDebitResult::default());
    }
    let now = chrono::Utc::now();
    // Serialize each entitlement's debits. Read the shared plan's current overage policy
    // from this statement's snapshot without locking every subscriber's plan row.
    let entitlement_rows = sqlx::query(
        r#"
SELECT
    user_plan_entitlements.id,
    user_plan_entitlements.entitlements_snapshot,
    billing_plans.entitlements_json AS plan_entitlements_json
FROM user_plan_entitlements
JOIN billing_plans ON billing_plans.id = user_plan_entitlements.plan_id
WHERE user_plan_entitlements.user_id = $1
    AND user_plan_entitlements.status = 'active'
    AND user_plan_entitlements.starts_at <= NOW()
    AND user_plan_entitlements.expires_at > NOW()
ORDER BY user_plan_entitlements.expires_at ASC,
                 user_plan_entitlements.created_at ASC,
                 user_plan_entitlements.id ASC
FOR UPDATE OF user_plan_entitlements
        "#,
    )
    .bind(user_id)
    .fetch_all(&mut **tx)
    .await
    .map_postgres_err()?;
    let mut grants = Vec::new();
    for row in entitlement_rows {
        let entitlement_id: String = row.try_get("id").map_postgres_err()?;
        let entitlements: serde_json::Value =
            row.try_get("entitlements_snapshot").map_postgres_err()?;
        let plan_entitlements: serde_json::Value =
            row.try_get("plan_entitlements_json").map_postgres_err()?;
        grants.extend(daily_quota_grants_from_entitlement(
            &entitlement_id,
            &entitlements,
            daily_quota_wallet_overage_policy(&plan_entitlements),
            now,
        )?);
    }
    if grants.is_empty() {
        return Ok(DailyQuotaDebitResult::default());
    }

    let mut grants_with_remaining = Vec::new();
    let mut total_remaining = 0.0;
    let mut allow_wallet_overage = true;
    for grant in grants {
        allow_wallet_overage &= grant.allow_wallet_overage;
        let used = sqlx::query_scalar::<_, Option<f64>>(
            r#"
SELECT CAST(COALESCE(SUM(amount_usd), 0) AS DOUBLE PRECISION)
FROM entitlement_usage_ledgers
WHERE user_entitlement_id = $1
  AND usage_date = $2
            "#,
        )
        .bind(&grant.entitlement_id)
        .bind(&grant.usage_date)
        .fetch_one(&mut **tx)
        .await
        .map_postgres_err()?
        .unwrap_or(0.0);
        if !used.is_finite() || used < 0.0 {
            return Err(DataLayerError::UnexpectedValue(
                "daily quota usage ledger total is invalid".to_string(),
            ));
        }
        let remaining = (grant.daily_quota_usd - used).max(0.0);
        total_remaining += remaining;
        if !total_remaining.is_finite() {
            return Err(DataLayerError::UnexpectedValue(
                "daily quota remaining total overflowed".to_string(),
            ));
        }
        grants_with_remaining.push((grant, remaining));
    }

    let insufficient = (!allow_wallet_overage && total_remaining + 0.000_000_01 < total_cost_usd)
        || (allow_wallet_overage
            && !wallet_can_overdraft
            && wallet_available_usd.is_some_and(|available| {
                total_remaining + available + SETTLEMENT_EPSILON_USD < total_cost_usd
            }));

    let mut remaining_cost = total_cost_usd;
    let mut debited = 0.0;
    for (grant, balance_before) in grants_with_remaining {
        if remaining_cost <= 0.000_000_01 || balance_before <= 0.0 {
            continue;
        }
        let amount = remaining_cost.min(balance_before);
        let balance_after = balance_before - amount;
        sqlx::query(
            r#"
INSERT INTO entitlement_usage_ledgers (
  id, user_entitlement_id, user_id, request_id, amount_usd,
  balance_before, balance_after, usage_date, created_at
)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, NOW())
ON CONFLICT (user_entitlement_id, request_id) DO NOTHING
            "#,
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(&grant.entitlement_id)
        .bind(user_id)
        .bind(request_id)
        .bind(amount)
        .bind(balance_before)
        .bind(balance_after)
        .bind(&grant.usage_date)
        .execute(&mut **tx)
        .await
        .map_postgres_err()?;
        remaining_cost -= amount;
        debited += amount;
    }
    Ok(DailyQuotaDebitResult {
        debited_usd: debited,
        insufficient,
    })
}

#[async_trait]
impl SettlementWriteRepository for SqlxSettlementRepository {
    async fn reserve_usage_policy_request(
        &self,
        input: ReserveUsagePolicyRequestInput,
    ) -> Result<ReserveUsagePolicyRequestOutcome, DataLayerError> {
        input.validate()?;
        self.tx_runner
            .run_read_write(|tx| {
                Box::pin(async move {
                    if !lock_usage_policy_subject_postgres(tx, &input.subject_id).await? {
                        return Err(usage_policy_subject_missing());
                    }
                    let existing_row =
                        sqlx::query(FIND_USAGE_POLICY_REQUEST_ADMISSION_POSTGRES_SQL)
                            .bind(&input.event_token)
                            .fetch_optional(&mut **tx)
                            .await
                            .map_postgres_err()?;
                    if let Some(row) = existing_row.as_ref() {
                        let existing = usage_policy_request_admission_from_postgres_row(row)?;
                        if existing.request_id != input.request_id
                            || existing.subject_id != input.subject_id
                        {
                            return Ok(ReserveUsagePolicyRequestOutcome::Conflict);
                        }
                        if existing.admitted_at_unix_secs != input.admitted_at_unix_secs {
                            return Err(DataLayerError::InvalidInput(
                                "usage policy event_token must keep its original admitted_at"
                                    .to_string(),
                            ));
                        }
                        sqlx::query(
                            r#"
UPDATE usage_request_admissions
SET retain_until = GREATEST(retain_until, TO_TIMESTAMP($2::double precision))
WHERE event_token = $1
                            "#,
                        )
                        .bind(&input.event_token)
                        .bind(usage_policy_cost_i64(
                            input.retain_until_unix_secs,
                            "usage policy request retain_until",
                        )?)
                        .execute(&mut **tx)
                        .await
                        .map_postgres_err()?;
                        return Ok(match existing.state {
                            UsagePolicyRequestAdmissionState::Active => {
                                ReserveUsagePolicyRequestOutcome::Allowed
                            }
                            UsagePolicyRequestAdmissionState::Released => {
                                ReserveUsagePolicyRequestOutcome::AlreadyReleased
                            }
                        });
                    }

                    let window_counts = usage_policy_request_window_counts(tx, &input).await?;
                    for (window_index, window) in input.windows.iter().enumerate() {
                        let used_requests = usage_policy_cost_u64(
                            window_counts.try_get(window_index).map_postgres_err()?,
                            "usage policy request used_requests",
                        )?;
                        if used_requests >= window.limit_requests {
                            return Ok(ReserveUsagePolicyRequestOutcome::Rejected {
                                window_index,
                                limit_requests: window.limit_requests,
                                used_requests,
                            });
                        }
                    }

                    let insert_result = sqlx::query(
                        r#"
INSERT INTO usage_request_admissions (
  request_id, subject_id, event_token, admitted_at, retain_until,
  state, released_at, created_at
) VALUES (
  $1, $2, $3, TO_TIMESTAMP($4::double precision),
  TO_TIMESTAMP($5::double precision), 'active', NULL, NOW()
)
ON CONFLICT (event_token) DO NOTHING
                        "#,
                    )
                    .bind(&input.request_id)
                    .bind(&input.subject_id)
                    .bind(&input.event_token)
                    .bind(usage_policy_cost_i64(
                        input.admitted_at_unix_secs,
                        "usage policy request admitted_at",
                    )?)
                    .bind(usage_policy_cost_i64(
                        input.retain_until_unix_secs,
                        "usage policy request retain_until",
                    )?)
                    .execute(&mut **tx)
                    .await
                    .map_postgres_err()?;
                    if insert_result.rows_affected() == 1 {
                        return Ok(ReserveUsagePolicyRequestOutcome::Allowed);
                    }

                    // A token can race across different subjects, which hold different subject
                    // locks. The unique key resolves that race; classify it explicitly here.
                    let row = sqlx::query(FIND_USAGE_POLICY_REQUEST_ADMISSION_POSTGRES_SQL)
                        .bind(&input.event_token)
                        .fetch_one(&mut **tx)
                        .await
                        .map_postgres_err()?;
                    let existing = usage_policy_request_admission_from_postgres_row(&row)?;
                    if existing.request_id != input.request_id
                        || existing.subject_id != input.subject_id
                    {
                        return Ok(ReserveUsagePolicyRequestOutcome::Conflict);
                    }
                    if existing.admitted_at_unix_secs != input.admitted_at_unix_secs {
                        return Err(DataLayerError::InvalidInput(
                            "usage policy event_token must keep its original admitted_at"
                                .to_string(),
                        ));
                    }
                    Ok(match existing.state {
                        UsagePolicyRequestAdmissionState::Active => {
                            ReserveUsagePolicyRequestOutcome::Allowed
                        }
                        UsagePolicyRequestAdmissionState::Released => {
                            ReserveUsagePolicyRequestOutcome::AlreadyReleased
                        }
                    })
                })
            })
            .await
    }

    async fn release_usage_policy_request_admission(
        &self,
        input: ReleaseUsagePolicyRequestAdmissionInput,
    ) -> Result<Option<StoredUsagePolicyRequestAdmission>, DataLayerError> {
        input.validate()?;
        self.tx_runner
            .run_read_write(|tx| {
                Box::pin(async move {
                    if !lock_usage_policy_subject_postgres(tx, &input.subject_id).await? {
                        return Ok(None);
                    }
                    let row = sqlx::query(FIND_USAGE_POLICY_REQUEST_ADMISSION_POSTGRES_SQL)
                        .bind(&input.event_token)
                        .fetch_optional(&mut **tx)
                        .await
                        .map_postgres_err()?;
                    let Some(row) = row else {
                        return Ok(None);
                    };
                    let mut admission = usage_policy_request_admission_from_postgres_row(&row)?;
                    if admission.request_id != input.request_id
                        || admission.subject_id != input.subject_id
                    {
                        return Ok(None);
                    }
                    if input.released_at_unix_secs < admission.admitted_at_unix_secs {
                        return Err(DataLayerError::InvalidInput(
                            "usage policy released_at must not precede admitted_at".to_string(),
                        ));
                    }
                    if admission.state == UsagePolicyRequestAdmissionState::Active {
                        sqlx::query(
                            r#"
UPDATE usage_request_admissions
SET state = 'released', released_at = TO_TIMESTAMP($2::double precision)
WHERE event_token = $1 AND state = 'active'
                            "#,
                        )
                        .bind(&input.event_token)
                        .bind(usage_policy_cost_i64(
                            input.released_at_unix_secs,
                            "usage policy request released_at",
                        )?)
                        .execute(&mut **tx)
                        .await
                        .map_postgres_err()?;
                        admission.state = UsagePolicyRequestAdmissionState::Released;
                        admission.released_at_unix_secs = Some(input.released_at_unix_secs);
                    }
                    Ok(Some(admission))
                })
            })
            .await
    }

    async fn cleanup_usage_policy_request_admissions(
        &self,
        now_unix_secs: u64,
        batch_size: usize,
    ) -> Result<usize, DataLayerError> {
        if batch_size == 0 {
            return Ok(0);
        }
        let now = usage_policy_cost_i64(now_unix_secs, "usage policy request cleanup timestamp")?;
        let limit = i64::try_from(batch_size).unwrap_or(i64::MAX);
        let result = sqlx::query(
            r#"
DELETE FROM usage_request_admissions
WHERE retain_until <= TO_TIMESTAMP($1::double precision)
  AND event_token IN (
  SELECT event_token
  FROM usage_request_admissions
  WHERE retain_until <= TO_TIMESTAMP($1::double precision)
  ORDER BY retain_until, event_token
  LIMIT $2
)
            "#,
        )
        .bind(now)
        .bind(limit)
        .execute(self.tx_runner.pool())
        .await
        .map_postgres_err()?;
        Ok(result.rows_affected() as usize)
    }

    async fn reserve_usage_policy_cost(
        &self,
        input: ReserveUsagePolicyCostInput,
    ) -> Result<ReserveUsagePolicyCostOutcome, DataLayerError> {
        input.validate()?;
        self.tx_runner
            .run_read_write(|tx| {
                Box::pin(async move {
                    if !lock_usage_policy_subject_postgres(tx, &input.subject_id).await? {
                        return Err(usage_policy_subject_missing());
                    }
                    let existing_row = sqlx::query(FIND_USAGE_POLICY_COST_RESERVATION_POSTGRES_SQL)
                        .bind(&input.reservation_token)
                        .fetch_optional(&mut **tx)
                        .await
                        .map_postgres_err()?;
                    let existing = existing_row
                        .as_ref()
                        .map(usage_policy_cost_reservation_from_postgres_row)
                        .transpose()?;
                    if let Some(existing) = existing.as_ref() {
                        if existing.request_id != input.request_id
                            || existing.subject_id != input.subject_id
                        {
                            return Ok(ReserveUsagePolicyCostOutcome::Conflict);
                        }
                        if existing.state != UsagePolicyCostReservationState::Reserved {
                            return Ok(ReserveUsagePolicyCostOutcome::AlreadyTerminal {
                                state: existing.state,
                            });
                        }
                        if existing.admitted_at_unix_secs != input.admitted_at_unix_secs {
                            return Err(DataLayerError::InvalidInput(
                                "usage policy reservation_token must keep its original admitted_at"
                                    .to_string(),
                            ));
                        }
                    }

                    let previous_reserved_cost_units = existing
                        .as_ref()
                        .map(|reservation| reservation.reserved_cost_units)
                        .unwrap_or(0);
                    let target_reserved_cost_units =
                        previous_reserved_cost_units.max(input.reserved_cost_units);
                    let window_totals = usage_policy_cost_window_totals(tx, &input).await?;
                    for (window_index, window) in input.windows.iter().enumerate() {
                        let used_cost_units = usage_policy_cost_u64(
                            window_totals.try_get(window_index).map_postgres_err()?,
                            "usage policy used_cost_units",
                        )?;
                        if used_cost_units
                            .checked_add(target_reserved_cost_units)
                            .is_none_or(|total| total > window.limit_cost_units)
                        {
                            return Ok(ReserveUsagePolicyCostOutcome::Rejected {
                                window_index,
                                limit_cost_units: window.limit_cost_units,
                                used_cost_units,
                            });
                        }
                    }

                    sqlx::query(
                        r#"
INSERT INTO usage_cost_reservations (
  request_id, subject_id, reservation_token, admitted_at,
  reserved_cost_units, actual_cost_units,
  state, reservation_expires_at, retain_until, finalized_at, created_at, updated_at
) VALUES (
  $1, $2, $3, TO_TIMESTAMP($4::double precision), $5, NULL,
  'reserved', TO_TIMESTAMP($6::double precision), TO_TIMESTAMP($7::double precision),
  NULL, NOW(), NOW()
)
ON CONFLICT (reservation_token) DO UPDATE SET
  reserved_cost_units = GREATEST(
    usage_cost_reservations.reserved_cost_units,
    EXCLUDED.reserved_cost_units
  ),
  reservation_expires_at = GREATEST(
    usage_cost_reservations.reservation_expires_at,
    EXCLUDED.reservation_expires_at
  ),
  retain_until = GREATEST(
    usage_cost_reservations.retain_until,
    EXCLUDED.retain_until
  ),
  updated_at = NOW()
                        "#,
                    )
                    .bind(&input.request_id)
                    .bind(&input.subject_id)
                    .bind(&input.reservation_token)
                    .bind(usage_policy_cost_i64(
                        input.admitted_at_unix_secs,
                        "usage policy admitted_at",
                    )?)
                    .bind(usage_policy_cost_i64(
                        target_reserved_cost_units,
                        "usage policy reserved_cost_units",
                    )?)
                    .bind(usage_policy_cost_i64(
                        input.reservation_expires_at_unix_secs,
                        "usage policy reservation_expires_at",
                    )?)
                    .bind(usage_policy_cost_i64(
                        input.retain_until_unix_secs,
                        "usage policy retain_until",
                    )?)
                    .execute(&mut **tx)
                    .await
                    .map_postgres_err()?;

                    Ok(ReserveUsagePolicyCostOutcome::Allowed {
                        reserved_cost_units: target_reserved_cost_units,
                        additional_reserved_cost_units: target_reserved_cost_units
                            .saturating_sub(previous_reserved_cost_units),
                    })
                })
            })
            .await
    }

    async fn reconcile_usage_policy_cost(
        &self,
        input: ReconcileUsagePolicyCostInput,
    ) -> Result<Option<StoredUsagePolicyCostReservation>, DataLayerError> {
        input.validate()?;
        self.tx_runner
            .run_read_write(|tx| {
                Box::pin(async move {
                    if !lock_usage_policy_subject_postgres(tx, &input.subject_id).await? {
                        return Ok(None);
                    }
                    let row = sqlx::query(FIND_USAGE_POLICY_COST_RESERVATION_POSTGRES_SQL)
                        .bind(&input.reservation_token)
                        .fetch_optional(&mut **tx)
                        .await
                        .map_postgres_err()?;
                    let Some(row) = row else {
                        return Ok(None);
                    };
                    let mut reservation = usage_policy_cost_reservation_from_postgres_row(&row)?;
                    if reservation.request_id != input.request_id
                        || reservation.subject_id != input.subject_id
                    {
                        // The token selects the row; audit identity must still match before the
                        // reservation can be finalized.
                        return Ok(None);
                    }
                    if reservation.state == UsagePolicyCostReservationState::Reserved {
                        sqlx::query(
                            r#"
UPDATE usage_cost_reservations
SET state = $4,
    actual_cost_units = $5,
    finalized_at = TO_TIMESTAMP($6::double precision),
    updated_at = NOW()
WHERE reservation_token = $1
  AND request_id = $2
  AND subject_id = $3
  AND state = 'reserved'
                            "#,
                        )
                        .bind(&input.reservation_token)
                        .bind(&input.request_id)
                        .bind(&input.subject_id)
                        .bind(input.terminal_state.as_str())
                        .bind(usage_policy_cost_i64(
                            input.actual_cost_units,
                            "usage policy actual_cost_units",
                        )?)
                        .bind(usage_policy_cost_i64(
                            input.finalized_at_unix_secs,
                            "usage policy finalized_at",
                        )?)
                        .execute(&mut **tx)
                        .await
                        .map_postgres_err()?;
                        reservation.state = input.terminal_state;
                        reservation.actual_cost_units = Some(input.actual_cost_units);
                        reservation.finalized_at_unix_secs = Some(input.finalized_at_unix_secs);
                    }
                    Ok(Some(reservation))
                })
            })
            .await
    }

    async fn cleanup_usage_policy_cost_reservations(
        &self,
        now_unix_secs: u64,
        batch_size: usize,
    ) -> Result<usize, DataLayerError> {
        if batch_size == 0 {
            return Ok(0);
        }
        let now = usage_policy_cost_i64(now_unix_secs, "usage policy cleanup timestamp")?;
        let limit = i64::try_from(batch_size).unwrap_or(i64::MAX);
        let result = sqlx::query(
            r#"
DELETE FROM usage_cost_reservations
WHERE retain_until <= TO_TIMESTAMP($1::double precision)
  AND reservation_token IN (
  SELECT reservation_token
  FROM usage_cost_reservations
  WHERE retain_until <= TO_TIMESTAMP($1::double precision)
  ORDER BY retain_until, reservation_token
  LIMIT $2
)
            "#,
        )
        .bind(now)
        .bind(limit)
        .execute(self.tx_runner.pool())
        .await
        .map_postgres_err()?;
        Ok(result.rows_affected() as usize)
    }

    async fn settle_usage(
        &self,
        input: UsageSettlementInput,
    ) -> Result<Option<StoredUsageSettlement>, DataLayerError> {
        input.validate()?;
        self.tx_runner
            .run_read_write(|tx| {
                Box::pin(async move {
                    let row = sqlx::query(FIND_USAGE_FOR_SETTLEMENT_SQL)
                        .bind(&input.request_id)
                        .fetch_optional(&mut **tx)
                        .await
                        .map_postgres_err()?;

                    let Some(usage_row) = row else {
                        return Ok(None);
                    };

                    let current_billing_status: String =
                        usage_row.try_get("billing_status").map_postgres_err()?;
                    if matches!(
                        current_billing_status.as_str(),
                        "settled" | "void" | "insufficient_quota"
                    ) {
                        return settlement_from_row(&usage_row).map(Some);
                    }

                    let mut final_billing_status =
                        settlement_billing_status_for_usage_status(&input.status).to_string();
                    let finalized_at =
                        i64::try_from(input.finalized_at_unix_secs.unwrap_or_else(|| {
                            std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_secs()
                        }))
                        .map_err(|_| {
                            DataLayerError::InvalidInput("finalized_at overflow".to_string())
                        })?;

                    let mut settlement = StoredUsageSettlement {
                        request_id: input.request_id.clone(),
                        wallet_id: None,
                        billing_status: final_billing_status.to_string(),
                        wallet_balance_before: None,
                        wallet_balance_after: None,
                        wallet_recharge_balance_before: None,
                        wallet_recharge_balance_after: None,
                        wallet_gift_balance_before: None,
                        wallet_gift_balance_after: None,
                        provider_monthly_used_usd: None,
                        finalized_at_unix_secs: Some(finalized_at as u64),
                    };

                    if final_billing_status == "settled" {
                        let api_key_id = input
                            .api_key_id
                            .as_deref()
                            .filter(|value| !value.is_empty());
                        let api_key_is_standalone = if input.api_key_is_standalone {
                            true
                        } else if let Some(api_key_id) = api_key_id {
                            sqlx::query_scalar::<_, bool>(
                                r#"
SELECT is_standalone
FROM api_keys
WHERE id = $1
LIMIT 1
                                "#,
                            )
                            .bind(api_key_id)
                            .fetch_optional(&mut **tx)
                            .await
                            .map_postgres_err()?
                            .unwrap_or(false)
                        } else {
                            false
                        };

                        let wallet_row = if let Some(api_key_id) = api_key_id {
                            sqlx::query(
                                r#"
SELECT
  id,
  CAST(balance AS DOUBLE PRECISION) AS balance,
  CAST(gift_balance AS DOUBLE PRECISION) AS gift_balance,
  CAST(total_consumed AS DOUBLE PRECISION) AS total_consumed,
  limit_mode
FROM wallets
WHERE api_key_id = $1
FOR UPDATE
LIMIT 1
                                "#,
                            )
                            .bind(api_key_id)
                            .fetch_optional(&mut **tx)
                            .await
                            .map_postgres_err()?
                        } else {
                            None
                        };

                        let wallet_row = if wallet_row.is_some() {
                            wallet_row
                        } else if !api_key_is_standalone {
                            if let Some(user_id) =
                                input.user_id.as_deref().filter(|value| !value.is_empty())
                            {
                                sqlx::query(
                                    r#"
SELECT
  id,
  CAST(balance AS DOUBLE PRECISION) AS balance,
  CAST(gift_balance AS DOUBLE PRECISION) AS gift_balance,
  CAST(total_consumed AS DOUBLE PRECISION) AS total_consumed,
  limit_mode
FROM wallets
WHERE user_id = $1
FOR UPDATE
LIMIT 1
                                    "#,
                                )
                                .bind(user_id)
                                .fetch_optional(&mut **tx)
                                .await
                                .map_postgres_err()?
                            } else {
                                None
                            }
                        } else {
                            None
                        };

                        let wallet_can_overdraft = wallet_row.is_some();
                        let wallet_available_usd = match wallet_row.as_ref() {
                            Some(row) => {
                                let recharge_balance: f64 =
                                    row.try_get("balance").map_postgres_err()?;
                                let gift_balance: f64 =
                                    row.try_get("gift_balance").map_postgres_err()?;
                                let total_consumed: f64 =
                                    row.try_get("total_consumed").map_postgres_err()?;
                                validate_wallet_settlement_values(
                                    recharge_balance,
                                    gift_balance,
                                    total_consumed,
                                    0.0,
                                )?;
                                let limit_mode: String =
                                    row.try_get("limit_mode").map_postgres_err()?;
                                if limit_mode.eq_ignore_ascii_case("unlimited") {
                                    None
                                } else {
                                    Some(finite_wallet_available_usd(
                                        recharge_balance,
                                        gift_balance,
                                    ))
                                }
                            }
                            None => Some(0.0),
                        };
                        if let Some(row) = wallet_row.as_ref() {
                            let wallet_id: String = row.try_get("id").map_postgres_err()?;
                            let before_recharge: f64 = row.try_get("balance").map_postgres_err()?;
                            let before_gift: f64 =
                                row.try_get("gift_balance").map_postgres_err()?;
                            let before_total = before_recharge + before_gift;
                            settlement.wallet_id = Some(wallet_id);
                            settlement.wallet_balance_before = Some(before_total);
                            settlement.wallet_balance_after = Some(before_total);
                            settlement.wallet_recharge_balance_before = Some(before_recharge);
                            settlement.wallet_recharge_balance_after = Some(before_recharge);
                            settlement.wallet_gift_balance_before = Some(before_gift);
                            settlement.wallet_gift_balance_after = Some(before_gift);
                        }

                        let billable_cost_usd = settlement_billable_cost_usd(&input);
                        let wallet_debit_cost_usd = if !api_key_is_standalone {
                            if let Some(user_id) =
                                input.user_id.as_deref().filter(|value| !value.is_empty())
                            {
                                let quota = consume_daily_quota_postgres(
                                    tx,
                                    user_id,
                                    &input.request_id,
                                    billable_cost_usd,
                                    wallet_available_usd,
                                    wallet_can_overdraft,
                                )
                                .await?;
                                if quota.insufficient {
                                    final_billing_status = "insufficient_quota".to_string();
                                    settlement.billing_status = final_billing_status.clone();
                                    0.0
                                } else {
                                    (billable_cost_usd - quota.debited_usd).max(0.0)
                                }
                            } else {
                                billable_cost_usd
                            }
                        } else {
                            billable_cost_usd
                        };
                        if final_billing_status != "settled" {
                            sync_usage_settlement_snapshot(&mut **tx, &settlement).await?;
                            sqlx::query(FINALIZE_USAGE_BILLING_SQL)
                                .bind(&input.request_id)
                                .bind(&final_billing_status)
                                .bind(finalized_at)
                                .execute(&mut **tx)
                                .await
                                .map_postgres_err()?;
                            return Ok(Some(settlement));
                        }

                        if wallet_debit_cost_usd > SETTLEMENT_EPSILON_USD {
                            if let Some(wallet_row) = wallet_row {
                                let wallet_id: String =
                                    wallet_row.try_get("id").map_postgres_err()?;
                                let before_recharge: f64 =
                                    wallet_row.try_get("balance").map_postgres_err()?;
                                let before_gift: f64 =
                                    wallet_row.try_get("gift_balance").map_postgres_err()?;
                                let total_consumed: f64 =
                                    wallet_row.try_get("total_consumed").map_postgres_err()?;
                                let limit_mode: String =
                                    wallet_row.try_get("limit_mode").map_postgres_err()?;
                                let before_total = before_recharge + before_gift;
                                let mut after_recharge = before_recharge;
                                let mut after_gift = before_gift;
                                if !limit_mode.eq_ignore_ascii_case("unlimited") {
                                    let debit_plan = plan_finite_wallet_debit(
                                        before_recharge,
                                        before_gift,
                                        wallet_debit_cost_usd,
                                    );
                                    (after_recharge, after_gift) =
                                        debit_plan.after_balances(before_recharge, before_gift);
                                }
                                let total_consumed_after = total_consumed + wallet_debit_cost_usd;
                                validate_wallet_settlement_values(
                                    after_recharge,
                                    after_gift,
                                    total_consumed_after,
                                    0.0,
                                )?;
                                if final_billing_status == "settled" {
                                    sqlx::query(
                                        r#"
UPDATE wallets
SET
  balance = $2,
  gift_balance = $3,
  total_consumed = $4,
  updated_at = NOW()
WHERE id = $1
                                "#,
                                    )
                                    .bind(&wallet_id)
                                    .bind(after_recharge)
                                    .bind(after_gift)
                                    .bind(total_consumed_after)
                                    .execute(&mut **tx)
                                    .await
                                    .map_postgres_err()?;
                                }

                                settlement.wallet_id = Some(wallet_id.clone());
                                settlement.wallet_balance_before = Some(before_total);
                                settlement.wallet_balance_after = Some(after_recharge + after_gift);
                                settlement.wallet_recharge_balance_before = Some(before_recharge);
                                settlement.wallet_recharge_balance_after = Some(after_recharge);
                                settlement.wallet_gift_balance_before = Some(before_gift);
                                settlement.wallet_gift_balance_after = Some(after_gift);
                            } else {
                                final_billing_status = "insufficient_quota".to_string();
                                settlement.billing_status = final_billing_status.clone();
                            }
                        }

                        if final_billing_status != "settled" {
                            sync_usage_settlement_snapshot(&mut **tx, &settlement).await?;
                            sqlx::query(FINALIZE_USAGE_BILLING_SQL)
                                .bind(&input.request_id)
                                .bind(&final_billing_status)
                                .bind(finalized_at)
                                .execute(&mut **tx)
                                .await
                                .map_postgres_err()?;
                            return Ok(Some(settlement));
                        }

                        if let Some(provider_id) = input
                            .provider_id
                            .as_deref()
                            .filter(|value| !value.is_empty())
                        {
                            enqueue_provider_monthly_usage_delta(
                                &mut **tx,
                                &input.request_id,
                                provider_id,
                                input.actual_total_cost_usd,
                            )
                            .await?;
                        }
                    }

                    sync_usage_settlement_snapshot(&mut **tx, &settlement).await?;
                    sqlx::query(FINALIZE_USAGE_BILLING_SQL)
                        .bind(&input.request_id)
                        .bind(&final_billing_status)
                        .bind(finalized_at)
                        .execute(&mut **tx)
                        .await
                        .map_postgres_err()?;

                    Ok(Some(settlement))
                })
            })
            .await
    }
}

#[cfg(test)]
mod tests {
    use futures_util::FutureExt;
    use std::panic::AssertUnwindSafe;

    async fn isolated_settlement_test_pool() -> (sqlx::PgPool, String) {
        let database_url = std::env::var("AETHER_TEST_DATABASE_URL")
            .expect("AETHER_TEST_DATABASE_URL must point at the test database");
        let schema = format!("settlement_test_{}", uuid::Uuid::new_v4().simple());
        let options = database_url
            .parse::<sqlx::postgres::PgConnectOptions>()
            .expect("test database URL should parse")
            .options([("search_path", schema.as_str())]);
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect_with(options)
            .await
            .expect("test database should connect");
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&pool)
            .await
            .expect("isolated settlement schema should be created");
        // Separate connections must see the same fixture, so pg_temp cannot be used here.
        for table in [
            "billing_plans",
            "user_plan_entitlements",
            "entitlement_usage_ledgers",
            "users",
            "usage_request_admissions",
            "usage_cost_reservations",
        ] {
            sqlx::query(&format!(
                "CREATE TABLE {table} (LIKE public.{table} INCLUDING ALL)"
            ))
            .execute(&pool)
            .await
            .expect("isolated settlement table should be created");
        }
        (pool, schema)
    }

    #[tokio::test]
    #[ignore = "requires AETHER_TEST_DATABASE_URL and PostgreSQL migrations"]
    async fn live_usage_policy_window_aggregates_preserve_exact_admission_and_idempotency() {
        use super::*;
        use aether_data_contracts::repository::settlement::{
            UsagePolicyCostWindow, UsagePolicyRequestWindow,
        };

        let (pool, schema) = isolated_settlement_test_pool().await;
        let result = AssertUnwindSafe(async {
            sqlx::query("INSERT INTO users (id, username, email_verified) VALUES ('subject', 'subject', false)")
                .execute(&pool).await.unwrap();
            sqlx::raw_sql("INSERT INTO usage_request_admissions (request_id, subject_id, event_token, admitted_at, retain_until, state, released_at) VALUES
                ('older', 'subject', 'older', TO_TIMESTAMP(50), TO_TIMESTAMP(500), 'active', NULL),
                ('start', 'subject', 'start', TO_TIMESTAMP(100), TO_TIMESTAMP(500), 'active', NULL),
                ('inside', 'subject', 'inside', TO_TIMESTAMP(150), TO_TIMESTAMP(500), 'active', NULL),
                ('end', 'subject', 'end', TO_TIMESTAMP(200), TO_TIMESTAMP(500), 'active', NULL),
                ('released', 'subject', 'released', TO_TIMESTAMP(150), TO_TIMESTAMP(500), 'released', TO_TIMESTAMP(170))")
                .execute(&pool).await.unwrap();
            let repo = SqlxSettlementRepository::new(pool.clone());
            let mut request = ReserveUsagePolicyRequestInput {
                request_id: "new".to_string(), subject_id: "subject".to_string(), event_token: "new".to_string(),
                admitted_at_unix_secs: 175, retain_until_unix_secs: 500,
                windows: vec![
                    UsagePolicyRequestWindow { starts_at_unix_secs: 100, ends_at_unix_secs: 200, limit_requests: 2 },
                    UsagePolicyRequestWindow { starts_at_unix_secs: 0, ends_at_unix_secs: 300, limit_requests: 4 },
                ],
            };
            assert_eq!(repo.reserve_usage_policy_request(request.clone()).await.unwrap(),
                ReserveUsagePolicyRequestOutcome::Rejected { window_index: 0, limit_requests: 2, used_requests: 2 });
            request.windows[0].limit_requests = 3;
            assert_eq!(repo.reserve_usage_policy_request(request.clone()).await.unwrap(),
                ReserveUsagePolicyRequestOutcome::Rejected { window_index: 1, limit_requests: 4, used_requests: 4 });
            request.windows[1].limit_requests = 5;
            assert_eq!(repo.reserve_usage_policy_request(request.clone()).await.unwrap(), ReserveUsagePolicyRequestOutcome::Allowed);
            assert_eq!(repo.reserve_usage_policy_request(request.clone()).await.unwrap(), ReserveUsagePolicyRequestOutcome::Allowed);
            assert_eq!(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM usage_request_admissions WHERE event_token = 'new'")
                .fetch_one(&pool).await.unwrap(), 1);
            repo.release_usage_policy_request_admission(ReleaseUsagePolicyRequestAdmissionInput {
                request_id: request.request_id.clone(), subject_id: request.subject_id.clone(), event_token: request.event_token.clone(), released_at_unix_secs: 180,
            }).await.unwrap();
            assert_eq!(repo.reserve_usage_policy_request(request).await.unwrap(), ReserveUsagePolicyRequestOutcome::AlreadyReleased);

            sqlx::raw_sql("INSERT INTO usage_cost_reservations (request_id, subject_id, reservation_token, admitted_at, reserved_cost_units, actual_cost_units, state, reservation_expires_at, retain_until, finalized_at) VALUES
                ('older', 'subject', 'older', TO_TIMESTAMP(50), 99, 11, 'finalized', TO_TIMESTAMP(160), TO_TIMESTAMP(500), TO_TIMESTAMP(160)),
                ('start', 'subject', 'start', TO_TIMESTAMP(100), 99, 7, 'finalized', TO_TIMESTAMP(160), TO_TIMESTAMP(500), TO_TIMESTAMP(160)),
                ('inside', 'subject', 'inside', TO_TIMESTAMP(150), 5, NULL, 'reserved', TO_TIMESTAMP(300), TO_TIMESTAMP(500), NULL),
                ('expired', 'subject', 'expired', TO_TIMESTAMP(150), 99, NULL, 'reserved', TO_TIMESTAMP(175), TO_TIMESTAMP(500), NULL),
                ('end', 'subject', 'end', TO_TIMESTAMP(200), 99, 13, 'finalized', TO_TIMESTAMP(300), TO_TIMESTAMP(500), TO_TIMESTAMP(250)),
                ('released', 'subject', 'released', TO_TIMESTAMP(150), 99, 0, 'released', TO_TIMESTAMP(300), TO_TIMESTAMP(500), TO_TIMESTAMP(170))")
                .execute(&pool).await.unwrap();
            let mut cost = ReserveUsagePolicyCostInput {
                request_id: "cost".to_string(), subject_id: "subject".to_string(), reservation_token: "cost".to_string(),
                admitted_at_unix_secs: 175, reserved_cost_units: 3, reservation_expires_at_unix_secs: 400, retain_until_unix_secs: 500,
                windows: vec![
                    UsagePolicyCostWindow { window_id: "short".to_string(), starts_at_unix_secs: 100, ends_at_unix_secs: 200, limit_cost_units: 14 },
                    UsagePolicyCostWindow { window_id: "long".to_string(), starts_at_unix_secs: 0, ends_at_unix_secs: 300, limit_cost_units: 38 },
                ],
            };
            assert_eq!(repo.reserve_usage_policy_cost(cost.clone()).await.unwrap(),
                ReserveUsagePolicyCostOutcome::Rejected { window_index: 0, limit_cost_units: 14, used_cost_units: 12 });
            cost.windows[0].limit_cost_units = 15;
            assert_eq!(repo.reserve_usage_policy_cost(cost.clone()).await.unwrap(),
                ReserveUsagePolicyCostOutcome::Rejected { window_index: 1, limit_cost_units: 38, used_cost_units: 36 });
            cost.windows[1].limit_cost_units = 39;
            let allowed = repo.reserve_usage_policy_cost(cost.clone()).await.unwrap();
            assert!(matches!(allowed, ReserveUsagePolicyCostOutcome::Allowed { .. }), "{allowed:?}");
            let repeated = repo.reserve_usage_policy_cost(cost.clone()).await.unwrap();
            assert!(matches!(repeated, ReserveUsagePolicyCostOutcome::Allowed { .. }), "{repeated:?}");
            cost.reserved_cost_units = 4;
            assert_eq!(repo.reserve_usage_policy_cost(cost).await.unwrap(),
                ReserveUsagePolicyCostOutcome::Rejected { window_index: 0, limit_cost_units: 15, used_cost_units: 12 });

            sqlx::query("DELETE FROM usage_request_admissions").execute(&pool).await.unwrap();
            let make_request = |id: &str| ReserveUsagePolicyRequestInput {
                request_id: id.to_string(), subject_id: "subject".to_string(), event_token: id.to_string(),
                admitted_at_unix_secs: 175, retain_until_unix_secs: 500,
                windows: vec![UsagePolicyRequestWindow { starts_at_unix_secs: 0, ends_at_unix_secs: 300, limit_requests: 1 }],
            };
            let (first, second) = tokio::join!(repo.reserve_usage_policy_request(make_request("race-a")), repo.reserve_usage_policy_request(make_request("race-b")));
            let outcomes = [first.unwrap(), second.unwrap()];
            assert_eq!(outcomes.iter().filter(|outcome| matches!(outcome, ReserveUsagePolicyRequestOutcome::Allowed)).count(), 1);
            assert_eq!(outcomes.iter().filter(|outcome| matches!(outcome, ReserveUsagePolicyRequestOutcome::Rejected { used_requests: 1, .. })).count(), 1);
        }).catch_unwind().await;
        sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
            .execute(&pool)
            .await
            .unwrap();
        pool.close().await;
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    }

    #[tokio::test]
    #[ignore = "requires AETHER_TEST_DATABASE_URL and PostgreSQL migrations"]
    async fn live_daily_quota_serializes_each_entitlement_without_locking_shared_plan() {
        let (pool, schema) = isolated_settlement_test_pool().await;
        let result = AssertUnwindSafe(async {
            let grant = serde_json::json!([{
                "type": "daily_quota",
                "daily_quota_usd": 10.0,
                "reset_timezone": "UTC",
                "allow_wallet_overage": false,
            }]);
            sqlx::query(
                "INSERT INTO billing_plans (id, title, price_amount, duration_unit, duration_value, entitlements_json, created_at, updated_at) VALUES ('shared-plan', 'Shared plan', 10, 'month', 1, $1, NOW(), NOW())",
            )
            .bind(&grant)
            .execute(&pool)
            .await
            .expect("shared plan should insert");
            for user_id in ["user-a", "user-b"] {
                sqlx::query(
                    "INSERT INTO user_plan_entitlements (id, user_id, plan_id, payment_order_id, starts_at, expires_at, entitlements_snapshot, created_at, updated_at) VALUES ($1, $1, 'shared-plan', $1, NOW() - INTERVAL '1 hour', NOW() + INTERVAL '1 day', $2, NOW(), NOW())",
                )
                .bind(user_id)
                .bind(&grant)
                .execute(&pool)
                .await
                .expect("user entitlement should insert");
            }

            let mut first = pool.begin().await.expect("first transaction should start");
            let first_debit = super::consume_daily_quota_postgres(
                &mut first, "user-a", "request-a", 7.0, Some(0.0), false,
            )
            .await
            .expect("first user should consume quota");
            assert_eq!(first_debit.debited_usd, 7.0);
            assert!(!first_debit.insufficient);

            let mut second = pool.begin().await.expect("second transaction should start");
            sqlx::query("SET LOCAL lock_timeout = '500ms'")
                .execute(&mut *second)
                .await
                .expect("lock timeout should be configured");
            let second_debit = super::consume_daily_quota_postgres(
                &mut second, "user-b", "request-b", 2.0, Some(0.0), false,
            )
            .await
            .expect("another user's quota must not wait for the shared plan");
            assert_eq!(second_debit.debited_usd, 2.0);
            assert!(!second_debit.insufficient);
            second.commit().await.expect("second debit should commit");

            let mut same_user = pool.begin().await.expect("contending transaction should start");
            sqlx::query("SET LOCAL lock_timeout = '500ms'")
                .execute(&mut *same_user)
                .await
                .expect("lock timeout should be configured");
            let blocked = super::consume_daily_quota_postgres(
                &mut same_user, "user-a", "request-a-next", 2.0, Some(0.0), false,
            )
            .await
            .expect_err("the same entitlement must remain locked until commit");
            assert!(blocked.to_string().contains("SQLSTATE 55P03"), "{blocked}");
            same_user.rollback().await.expect("blocked transaction should roll back");
            first.commit().await.expect("first debit should commit");

            let mut next = pool.begin().await.expect("next transaction should start");
            let next_debit = super::consume_daily_quota_postgres(
                &mut next, "user-a", "request-a-next", 2.0, Some(0.0), false,
            )
            .await
            .expect("same user should consume the remaining quota after commit");
            assert_eq!(next_debit.debited_usd, 2.0);
            assert!(!next_debit.insufficient);
            next.commit().await.expect("next debit should commit");
            let balance: (f64, f64) = sqlx::query_as(
                "SELECT balance_before::double precision, balance_after::double precision FROM entitlement_usage_ledgers WHERE request_id = 'request-a-next'",
            )
            .fetch_one(&pool)
            .await
            .expect("next debit ledger should exist");
            assert_eq!(balance, (3.0, 1.0));

            let mut held = pool.begin().await.expect("quota transaction should start");
            super::consume_daily_quota_postgres(
                &mut held, "user-a", "request-policy-before", 0.5, Some(0.0), false,
            )
            .await
            .expect("quota transaction should retain its entitlement lock");
            let mut edit = pool.begin().await.expect("plan edit transaction should start");
            sqlx::query("SET LOCAL lock_timeout = '500ms'")
                .execute(&mut *edit)
                .await
                .expect("plan edit timeout should be configured");
            sqlx::query(
                "UPDATE billing_plans SET entitlements_json = jsonb_set(entitlements_json, '{0,allow_wallet_overage}', 'true'::jsonb) WHERE id = 'shared-plan'",
            )
            .execute(&mut *edit)
            .await
            .expect("plan configuration edits must not wait for usage settlement");
            edit.commit().await.expect("plan edit should commit");
            held.rollback().await.expect("held quota debit should roll back");

            let mut after_edit = pool.begin().await.expect("fresh transaction should start");
            let updated_policy = super::consume_daily_quota_postgres(
                &mut after_edit, "user-a", "request-policy-after", 2.0, Some(5.0), true,
            )
            .await
            .expect("fresh quota read should use current plan configuration");
            assert!(!updated_policy.insufficient);
            assert_eq!(updated_policy.debited_usd, 1.0);
            after_edit.rollback().await.expect("policy verification should roll back");
        })
        .catch_unwind()
        .await;
        sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
            .execute(&pool)
            .await
            .expect("isolated settlement schema should be removed");
        pool.close().await;
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    }

    #[test]
    fn finalize_usage_billing_sql_does_not_require_usage_updated_at_column() {
        assert!(!super::FINALIZE_USAGE_BILLING_SQL.contains("updated_at"));
    }

    #[test]
    fn settlement_sql_reads_settlement_snapshots_before_legacy_usage_columns() {
        assert!(
            super::FIND_USAGE_FOR_SETTLEMENT_SQL.contains("LEFT JOIN usage_settlement_snapshots")
        );
        assert!(super::FIND_USAGE_FOR_SETTLEMENT_SQL.contains(
            "COALESCE(usage_settlement_snapshots.billing_status, usage_record.billing_status)"
        ));
        assert!(super::FIND_USAGE_FOR_SETTLEMENT_SQL.contains("FOR UPDATE OF usage_record"));
    }

    #[test]
    fn settlement_sql_dual_writes_usage_settlement_snapshots() {
        assert!(super::UPSERT_USAGE_SETTLEMENT_SNAPSHOT_SQL
            .contains("INSERT INTO usage_settlement_snapshots"));
        assert!(super::UPSERT_USAGE_SETTLEMENT_SNAPSHOT_SQL.contains("provider_monthly_used_usd"));
        assert!(super::UPSERT_USAGE_SETTLEMENT_SNAPSHOT_SQL
            .contains("TO_TIMESTAMP($11::double precision)"));
    }

    #[test]
    fn settlement_sql_no_longer_dual_writes_wallet_snapshots_to_usage_rows() {
        let source = include_str!("settlement.rs");
        assert!(!source.contains("UPDATE \"usage\"\nSET\n  wallet_id = $2"));
    }

    #[test]
    fn settlement_sql_enqueues_provider_monthly_usage_delta() {
        let source = include_str!("settlement.rs");
        assert!(super::ENQUEUE_PROVIDER_MONTHLY_USAGE_DELTA_SQL.contains("usage_counter_deltas"));
        assert!(super::ENQUEUE_PROVIDER_MONTHLY_USAGE_DELTA_SQL.contains("'provider_monthly'"));
        assert!(!source.contains("UPDATE providers\nSET\n  monthly_used_usd"));
    }

    #[test]
    fn settlement_sql_blocks_standalone_key_owner_wallet_fallback() {
        let source = include_str!("settlement.rs");
        assert!(source.contains("SELECT is_standalone"));
        assert!(source.contains("} else if !api_key_is_standalone {"));
    }
}
