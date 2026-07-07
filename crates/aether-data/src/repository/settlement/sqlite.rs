use async_trait::async_trait;
use sqlx::{sqlite::SqliteRow, Row};

use super::{
    finite_wallet_available_usd, plan_finite_wallet_debit, settlement_billable_cost_usd,
    settlement_billing_status_for_usage_status, SettlementWriteRepository, StoredUsageSettlement,
    UsageSettlementInput, SETTLEMENT_EPSILON_USD,
};
use crate::driver::sqlite::{sqlite_optional_real, sqlite_real, SqlitePool};
use crate::error::SqlResultExt;
use crate::DataLayerError;

const FIND_USAGE_FOR_SETTLEMENT_SQL: &str = r#"
SELECT
  usage_record.request_id,
  COALESCE(usage_settlement_snapshots.wallet_id, usage_record.wallet_id) AS wallet_id,
  COALESCE(usage_settlement_snapshots.billing_status, usage_record.billing_status) AS billing_status,
  COALESCE(
    usage_settlement_snapshots.wallet_balance_before,
    usage_record.wallet_balance_before
  ) AS wallet_balance_before,
  COALESCE(
    usage_settlement_snapshots.wallet_balance_after,
    usage_record.wallet_balance_after
  ) AS wallet_balance_after,
  COALESCE(
    usage_settlement_snapshots.wallet_recharge_balance_before,
    usage_record.wallet_recharge_balance_before
  ) AS wallet_recharge_balance_before,
  COALESCE(
    usage_settlement_snapshots.wallet_recharge_balance_after,
    usage_record.wallet_recharge_balance_after
  ) AS wallet_recharge_balance_after,
  COALESCE(
    usage_settlement_snapshots.wallet_gift_balance_before,
    usage_record.wallet_gift_balance_before
  ) AS wallet_gift_balance_before,
  COALESCE(
    usage_settlement_snapshots.wallet_gift_balance_after,
    usage_record.wallet_gift_balance_after
  ) AS wallet_gift_balance_after,
  CAST(usage_settlement_snapshots.provider_monthly_used_usd AS REAL) AS provider_monthly_used_usd,
  usage_record.provider_id,
  COALESCE(usage_settlement_snapshots.finalized_at, usage_record.finalized_at) AS finalized_at_unix_secs
FROM "usage" AS usage_record
LEFT JOIN usage_settlement_snapshots
  ON usage_settlement_snapshots.request_id = usage_record.request_id
WHERE usage_record.request_id = ?
"#;

const FINALIZE_USAGE_BILLING_SQL: &str = r#"
UPDATE "usage"
SET
  billing_status = ?,
  finalized_at = COALESCE(finalized_at, ?)
WHERE request_id = ?
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
  finalized_at,
  created_at,
  updated_at
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
ON CONFLICT (request_id)
DO UPDATE SET
  billing_status = excluded.billing_status,
  wallet_id = COALESCE(excluded.wallet_id, usage_settlement_snapshots.wallet_id),
  wallet_balance_before = COALESCE(
    excluded.wallet_balance_before,
    usage_settlement_snapshots.wallet_balance_before
  ),
  wallet_balance_after = COALESCE(
    excluded.wallet_balance_after,
    usage_settlement_snapshots.wallet_balance_after
  ),
  wallet_recharge_balance_before = COALESCE(
    excluded.wallet_recharge_balance_before,
    usage_settlement_snapshots.wallet_recharge_balance_before
  ),
  wallet_recharge_balance_after = COALESCE(
    excluded.wallet_recharge_balance_after,
    usage_settlement_snapshots.wallet_recharge_balance_after
  ),
  wallet_gift_balance_before = COALESCE(
    excluded.wallet_gift_balance_before,
    usage_settlement_snapshots.wallet_gift_balance_before
  ),
  wallet_gift_balance_after = COALESCE(
    excluded.wallet_gift_balance_after,
    usage_settlement_snapshots.wallet_gift_balance_after
  ),
  provider_monthly_used_usd = COALESCE(
    excluded.provider_monthly_used_usd,
    usage_settlement_snapshots.provider_monthly_used_usd
  ),
  finalized_at = COALESCE(excluded.finalized_at, usage_settlement_snapshots.finalized_at),
  updated_at = excluded.updated_at
"#;

#[derive(Debug, Clone)]
pub struct SqliteSettlementRepository {
    pool: SqlitePool,
}

impl SqliteSettlementRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

fn settlement_from_row(row: &SqliteRow) -> Result<StoredUsageSettlement, DataLayerError> {
    Ok(StoredUsageSettlement {
        request_id: row.try_get("request_id").map_sql_err()?,
        wallet_id: row.try_get("wallet_id").map_sql_err()?,
        billing_status: row.try_get("billing_status").map_sql_err()?,
        wallet_balance_before: sqlite_optional_real(row, "wallet_balance_before")?,
        wallet_balance_after: sqlite_optional_real(row, "wallet_balance_after")?,
        wallet_recharge_balance_before: sqlite_optional_real(
            row,
            "wallet_recharge_balance_before",
        )?,
        wallet_recharge_balance_after: sqlite_optional_real(row, "wallet_recharge_balance_after")?,
        wallet_gift_balance_before: sqlite_optional_real(row, "wallet_gift_balance_before")?,
        wallet_gift_balance_after: sqlite_optional_real(row, "wallet_gift_balance_after")?,
        provider_monthly_used_usd: sqlite_optional_real(row, "provider_monthly_used_usd")?,
        finalized_at_unix_secs: row
            .try_get::<Option<i64>, _>("finalized_at_unix_secs")
            .map_sql_err()?
            .map(|value| value as u64),
    })
}

fn now_unix_secs() -> Result<i64, DataLayerError> {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    )
    .map_err(|_| DataLayerError::InvalidInput("timestamp overflow".to_string()))
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
        grants.push(DailyQuotaGrant {
            entitlement_id: entitlement_id.to_string(),
            daily_quota_usd,
            usage_date: daily_quota_usage_date(
                item.get("reset_timezone")
                    .and_then(serde_json::Value::as_str),
                now,
            )?,
            allow_wallet_overage: item
                .get("allow_wallet_overage")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
        });
    }
    Ok(grants)
}

async fn consume_daily_quota_sqlite(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    user_id: &str,
    request_id: &str,
    total_cost_usd: f64,
    wallet_available_usd: Option<f64>,
    wallet_can_overdraft: bool,
    now_unix_secs: i64,
) -> Result<DailyQuotaDebitResult, DataLayerError> {
    if total_cost_usd <= 0.0 {
        return Ok(DailyQuotaDebitResult::default());
    }
    let rows = sqlx::query(
        r#"
SELECT id, entitlements_snapshot
FROM user_plan_entitlements
WHERE user_id = ?
  AND status = 'active'
  AND starts_at <= ?
  AND expires_at > ?
ORDER BY expires_at ASC, created_at ASC, id ASC
"#,
    )
    .bind(user_id)
    .bind(now_unix_secs)
    .bind(now_unix_secs)
    .fetch_all(&mut **tx)
    .await
    .map_sql_err()?;
    let now = chrono::Utc::now();
    let mut grants = Vec::new();
    for row in rows {
        let entitlement_id: String = row.try_get("id").map_sql_err()?;
        let entitlements_raw: String = row.try_get("entitlements_snapshot").map_sql_err()?;
        let entitlements =
            serde_json::from_str::<serde_json::Value>(&entitlements_raw).map_err(|err| {
                DataLayerError::UnexpectedValue(format!(
                    "user_plan_entitlements.entitlements_snapshot invalid json: {err}"
                ))
            })?;
        grants.extend(daily_quota_grants_from_entitlement(
            &entitlement_id,
            &entitlements,
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
        let used = sqlx::query_scalar::<_, f64>(
            r#"
SELECT CAST(COALESCE(SUM(amount_usd), 0) AS REAL)
FROM entitlement_usage_ledgers
WHERE user_entitlement_id = ?
  AND usage_date = ?
"#,
        )
        .bind(&grant.entitlement_id)
        .bind(&grant.usage_date)
        .fetch_one(&mut **tx)
        .await
        .map_sql_err()?;
        let remaining = (grant.daily_quota_usd - used).max(0.0);
        total_remaining += remaining;
        grants_with_remaining.push((grant, remaining));
    }
    if !allow_wallet_overage && total_remaining + 0.000_000_01 < total_cost_usd {
        return Ok(DailyQuotaDebitResult {
            debited_usd: 0.0,
            insufficient: true,
        });
    }
    if allow_wallet_overage
        && !wallet_can_overdraft
        && wallet_available_usd.is_some_and(|available| {
            total_remaining + available + SETTLEMENT_EPSILON_USD < total_cost_usd
        })
    {
        return Ok(DailyQuotaDebitResult {
            debited_usd: 0.0,
            insufficient: true,
        });
    }

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
INSERT OR IGNORE INTO entitlement_usage_ledgers (
  id, user_entitlement_id, user_id, request_id, amount_usd,
  balance_before, balance_after, usage_date, created_at
)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
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
        .bind(now_unix_secs)
        .execute(&mut **tx)
        .await
        .map_sql_err()?;
        remaining_cost -= amount;
        debited += amount;
    }
    Ok(DailyQuotaDebitResult {
        debited_usd: debited,
        insufficient: false,
    })
}

#[async_trait]
impl SettlementWriteRepository for SqliteSettlementRepository {
    async fn settle_usage(
        &self,
        input: UsageSettlementInput,
    ) -> Result<Option<StoredUsageSettlement>, DataLayerError> {
        input.validate()?;
        let finalized_at = i64::try_from(
            input
                .finalized_at_unix_secs
                .unwrap_or(now_unix_secs()? as u64),
        )
        .map_err(|_| DataLayerError::InvalidInput("finalized_at overflow".to_string()))?;
        let updated_at = now_unix_secs()?;

        let mut tx = self.pool.begin().await.map_sql_err()?;
        let row = sqlx::query(FIND_USAGE_FOR_SETTLEMENT_SQL)
            .bind(&input.request_id)
            .fetch_optional(&mut *tx)
            .await
            .map_sql_err()?;

        let Some(usage_row) = row else {
            tx.commit().await.map_sql_err()?;
            return Ok(None);
        };

        let current_billing_status: String = usage_row.try_get("billing_status").map_sql_err()?;
        if matches!(
            current_billing_status.as_str(),
            "settled" | "void" | "insufficient_quota"
        ) {
            let settlement = settlement_from_row(&usage_row)?;
            tx.commit().await.map_sql_err()?;
            return Ok(Some(settlement));
        }

        let mut final_billing_status =
            settlement_billing_status_for_usage_status(&input.status).to_string();
        let mut settlement = StoredUsageSettlement {
            request_id: input.request_id.clone(),
            wallet_id: None,
            billing_status: final_billing_status.clone(),
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
WHERE id = ?
LIMIT 1
"#,
                )
                .bind(api_key_id)
                .fetch_optional(&mut *tx)
                .await
                .map_sql_err()?
                .unwrap_or(false)
            } else {
                false
            };

            let wallet_row = if let Some(api_key_id) = api_key_id {
                sqlx::query(
                    r#"
SELECT id, balance, gift_balance, limit_mode
FROM wallets
WHERE api_key_id = ?
LIMIT 1
"#,
                )
                .bind(api_key_id)
                .fetch_optional(&mut *tx)
                .await
                .map_sql_err()?
            } else {
                None
            };

            let wallet_row = if wallet_row.is_some() {
                wallet_row
            } else if !api_key_is_standalone {
                if let Some(user_id) = input.user_id.as_deref().filter(|value| !value.is_empty()) {
                    sqlx::query(
                        r#"
SELECT id, balance, gift_balance, limit_mode
FROM wallets
WHERE user_id = ?
LIMIT 1
"#,
                    )
                    .bind(user_id)
                    .fetch_optional(&mut *tx)
                    .await
                    .map_sql_err()?
                } else {
                    None
                }
            } else {
                None
            };

            let wallet_can_overdraft = wallet_row.is_some();
            let wallet_available_usd = match wallet_row.as_ref() {
                Some(row) => {
                    let limit_mode: String = row.try_get("limit_mode").map_sql_err()?;
                    if limit_mode.eq_ignore_ascii_case("unlimited") {
                        None
                    } else {
                        Some(finite_wallet_available_usd(
                            sqlite_real(row, "balance")?,
                            sqlite_real(row, "gift_balance")?,
                        ))
                    }
                }
                None => Some(0.0),
            };
            if let Some(row) = wallet_row.as_ref() {
                let wallet_id: String = row.try_get("id").map_sql_err()?;
                let before_recharge = sqlite_real(row, "balance")?;
                let before_gift = sqlite_real(row, "gift_balance")?;
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
                if let Some(user_id) = input.user_id.as_deref().filter(|value| !value.is_empty()) {
                    let quota = consume_daily_quota_sqlite(
                        &mut tx,
                        user_id,
                        &input.request_id,
                        billable_cost_usd,
                        wallet_available_usd,
                        wallet_can_overdraft,
                        updated_at,
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
                    .bind(settlement.finalized_at_unix_secs.map(|value| value as i64))
                    .bind(updated_at)
                    .bind(updated_at)
                    .execute(&mut *tx)
                    .await
                    .map_sql_err()?;
                sqlx::query(FINALIZE_USAGE_BILLING_SQL)
                    .bind(&final_billing_status)
                    .bind(finalized_at)
                    .bind(&input.request_id)
                    .execute(&mut *tx)
                    .await
                    .map_sql_err()?;
                tx.commit().await.map_sql_err()?;
                return Ok(Some(settlement));
            }

            if wallet_debit_cost_usd > SETTLEMENT_EPSILON_USD {
                if let Some(wallet_row) = wallet_row {
                    let wallet_id: String = wallet_row.try_get("id").map_sql_err()?;
                    let before_recharge = sqlite_real(&wallet_row, "balance")?;
                    let before_gift = sqlite_real(&wallet_row, "gift_balance")?;
                    let limit_mode: String = wallet_row.try_get("limit_mode").map_sql_err()?;
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
                    if final_billing_status == "settled" {
                        sqlx::query(
                            r#"
UPDATE wallets
SET
  balance = ?,
  gift_balance = ?,
  total_consumed = COALESCE(total_consumed, 0) + ?,
  updated_at = ?
WHERE id = ?
"#,
                        )
                        .bind(after_recharge)
                        .bind(after_gift)
                        .bind(wallet_debit_cost_usd)
                        .bind(updated_at)
                        .bind(&wallet_id)
                        .execute(&mut *tx)
                        .await
                        .map_sql_err()?;
                    }

                    settlement.wallet_id = Some(wallet_id);
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
                    .bind(settlement.finalized_at_unix_secs.map(|value| value as i64))
                    .bind(updated_at)
                    .bind(updated_at)
                    .execute(&mut *tx)
                    .await
                    .map_sql_err()?;
                sqlx::query(FINALIZE_USAGE_BILLING_SQL)
                    .bind(&final_billing_status)
                    .bind(finalized_at)
                    .bind(&input.request_id)
                    .execute(&mut *tx)
                    .await
                    .map_sql_err()?;
                tx.commit().await.map_sql_err()?;
                return Ok(Some(settlement));
            }

            if let Some(provider_id) = input
                .provider_id
                .as_deref()
                .filter(|value| !value.is_empty())
            {
                sqlx::query(
                    r#"
UPDATE providers
SET
  monthly_used_usd = CAST(COALESCE(monthly_used_usd, 0) AS REAL) + ?,
  updated_at = ?
WHERE id = ?
"#,
                )
                .bind(input.actual_total_cost_usd)
                .bind(updated_at)
                .bind(provider_id)
                .execute(&mut *tx)
                .await
                .map_sql_err()?;

                settlement.provider_monthly_used_usd = sqlx::query(
                    "SELECT CAST(monthly_used_usd AS REAL) AS monthly_used_usd FROM providers WHERE id = ? LIMIT 1",
                )
                .bind(provider_id)
                .fetch_optional(&mut *tx)
                .await
                .map_sql_err()?
                .map(|row| sqlite_real(&row, "monthly_used_usd"))
                .transpose()?;
            }
        }

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
            .bind(settlement.finalized_at_unix_secs.map(|value| value as i64))
            .bind(updated_at)
            .bind(updated_at)
            .execute(&mut *tx)
            .await
            .map_sql_err()?;

        sqlx::query(FINALIZE_USAGE_BILLING_SQL)
            .bind(&final_billing_status)
            .bind(finalized_at)
            .bind(&input.request_id)
            .execute(&mut *tx)
            .await
            .map_sql_err()?;

        tx.commit().await.map_sql_err()?;
        Ok(Some(settlement))
    }
}

#[cfg(test)]
mod tests {
    use super::SqliteSettlementRepository;
    use crate::lifecycle::migrate::run_sqlite_migrations;
    use crate::repository::settlement::{SettlementWriteRepository, UsageSettlementInput};
    use sqlx::Row;

    #[tokio::test]
    async fn sqlite_repository_settles_usage_once() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("sqlite pool should connect");
        run_sqlite_migrations(&pool)
            .await
            .expect("sqlite migrations should run");
        seed_settlement_rows(&pool).await;

        let repository = SqliteSettlementRepository::new(pool.clone());
        let settlement = repository
            .settle_usage(UsageSettlementInput {
                request_id: "request-1".to_string(),
                user_id: Some("user-1".to_string()),
                api_key_id: None,
                api_key_is_standalone: false,
                provider_id: Some("provider-1".to_string()),
                status: "completed".to_string(),
                billing_status: "pending".to_string(),
                total_cost_usd: 3.0,
                actual_total_cost_usd: 6.0,
                finalized_at_unix_secs: Some(1_234),
            })
            .await
            .expect("settlement should run")
            .expect("usage should exist");

        assert_eq!(settlement.billing_status, "settled");
        assert_eq!(settlement.wallet_id.as_deref(), Some("wallet-1"));
        assert_eq!(settlement.wallet_balance_before, Some(12.0));
        assert_eq!(settlement.wallet_balance_after, Some(6.0));
        assert_eq!(settlement.wallet_recharge_balance_after, Some(4.0));
        assert_eq!(settlement.wallet_gift_balance_after, Some(2.0));
        assert_eq!(settlement.provider_monthly_used_usd, Some(11.0));

        let wallet = sqlx::query(
            "SELECT balance, gift_balance, total_consumed FROM wallets WHERE id = 'wallet-1'",
        )
        .fetch_one(&pool)
        .await
        .expect("wallet should load");
        assert_eq!(wallet.try_get::<f64, _>("balance").unwrap(), 4.0);
        assert_eq!(wallet.try_get::<f64, _>("gift_balance").unwrap(), 2.0);
        assert_eq!(wallet.try_get::<f64, _>("total_consumed").unwrap(), 6.0);

        let second = repository
            .settle_usage(UsageSettlementInput {
                request_id: "request-1".to_string(),
                user_id: Some("user-1".to_string()),
                api_key_id: None,
                api_key_is_standalone: false,
                provider_id: Some("provider-1".to_string()),
                status: "completed".to_string(),
                billing_status: "pending".to_string(),
                total_cost_usd: 3.0,
                actual_total_cost_usd: 6.0,
                finalized_at_unix_secs: Some(9_999),
            })
            .await
            .expect("second settlement should run")
            .expect("usage should exist");
        assert_eq!(second.finalized_at_unix_secs, Some(1_234));

        let provider_used: f64 =
            sqlx::query_scalar("SELECT monthly_used_usd FROM providers WHERE id = 'provider-1'")
                .fetch_one(&pool)
                .await
                .expect("provider should load");
        assert_eq!(provider_used, 11.0);
    }

    #[tokio::test]
    async fn sqlite_repository_voids_failed_usage_without_wallet_mutation() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("sqlite pool should connect");
        run_sqlite_migrations(&pool)
            .await
            .expect("sqlite migrations should run");
        seed_settlement_rows(&pool).await;

        let repository = SqliteSettlementRepository::new(pool.clone());
        let settlement = repository
            .settle_usage(UsageSettlementInput {
                request_id: "request-2".to_string(),
                user_id: Some("user-1".to_string()),
                api_key_id: None,
                api_key_is_standalone: false,
                provider_id: Some("provider-1".to_string()),
                status: "failed".to_string(),
                billing_status: "pending".to_string(),
                total_cost_usd: 3.0,
                actual_total_cost_usd: 2.0,
                finalized_at_unix_secs: Some(1_235),
            })
            .await
            .expect("settlement should run")
            .expect("usage should exist");

        assert_eq!(settlement.billing_status, "void");
        assert_eq!(settlement.wallet_id, None);
        let wallet_total: f64 =
            sqlx::query_scalar("SELECT balance + gift_balance FROM wallets WHERE id = 'wallet-1'")
                .fetch_one(&pool)
                .await
                .expect("wallet should load");
        assert_eq!(wallet_total, 12.0);
    }

    #[tokio::test]
    async fn sqlite_repository_overdraws_finite_wallet_and_settles_usage() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("sqlite pool should connect");
        run_sqlite_migrations(&pool)
            .await
            .expect("sqlite migrations should run");
        seed_settlement_rows(&pool).await;

        let repository = SqliteSettlementRepository::new(pool.clone());
        let settlement = repository
            .settle_usage(UsageSettlementInput {
                request_id: "request-overdraw".to_string(),
                user_id: Some("user-1".to_string()),
                api_key_id: None,
                api_key_is_standalone: false,
                provider_id: Some("provider-1".to_string()),
                status: "completed".to_string(),
                billing_status: "pending".to_string(),
                total_cost_usd: 15.0,
                actual_total_cost_usd: 15.0,
                finalized_at_unix_secs: Some(1_236),
            })
            .await
            .expect("settlement should run")
            .expect("usage should exist");

        assert_eq!(settlement.billing_status, "settled");
        assert_eq!(settlement.wallet_id.as_deref(), Some("wallet-1"));
        assert_eq!(settlement.wallet_balance_before, Some(12.0));
        assert_eq!(settlement.wallet_balance_after, Some(-3.0));
        assert_eq!(settlement.wallet_recharge_balance_after, Some(-3.0));
        assert_eq!(settlement.wallet_gift_balance_after, Some(0.0));
        assert_eq!(settlement.provider_monthly_used_usd, Some(20.0));

        let wallet = sqlx::query(
            "SELECT balance, gift_balance, total_consumed FROM wallets WHERE id = 'wallet-1'",
        )
        .fetch_one(&pool)
        .await
        .expect("wallet should load");
        assert_eq!(wallet.try_get::<f64, _>("balance").unwrap(), -3.0);
        assert_eq!(wallet.try_get::<f64, _>("gift_balance").unwrap(), 0.0);
        assert_eq!(wallet.try_get::<f64, _>("total_consumed").unwrap(), 15.0);
    }

    #[tokio::test]
    async fn sqlite_repository_records_wallet_for_quota_covered_user_usage() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("sqlite pool should connect");
        run_sqlite_migrations(&pool)
            .await
            .expect("sqlite migrations should run");
        seed_quota_covered_settlement_rows(&pool).await;

        let repository = SqliteSettlementRepository::new(pool.clone());
        let settlement = repository
            .settle_usage(UsageSettlementInput {
                request_id: "request-quota-covered".to_string(),
                user_id: Some("user-quota".to_string()),
                api_key_id: Some("key-quota".to_string()),
                api_key_is_standalone: false,
                provider_id: None,
                status: "completed".to_string(),
                billing_status: "pending".to_string(),
                total_cost_usd: 3.0,
                actual_total_cost_usd: 6.0,
                finalized_at_unix_secs: Some(1_260),
            })
            .await
            .expect("settlement should run")
            .expect("usage should exist");

        assert_eq!(settlement.billing_status, "settled");
        assert_eq!(settlement.wallet_id.as_deref(), Some("wallet-quota"));
        assert_eq!(settlement.wallet_balance_before, Some(0.0));
        assert_eq!(settlement.wallet_balance_after, Some(0.0));

        let wallet_total: f64 = sqlx::query_scalar(
            "SELECT balance + gift_balance FROM wallets WHERE id = 'wallet-quota'",
        )
        .fetch_one(&pool)
        .await
        .expect("wallet should load");
        assert_eq!(wallet_total, 0.0);

        let quota_used: f64 = sqlx::query_scalar(
            "SELECT CAST(COALESCE(SUM(amount_usd), 0) AS REAL) FROM entitlement_usage_ledgers WHERE request_id = 'request-quota-covered'",
        )
        .fetch_one(&pool)
        .await
        .expect("quota ledger should load");
        assert_eq!(quota_used, 6.0);
    }

    async fn seed_settlement_rows(pool: &sqlx::SqlitePool) {
        sqlx::query(
            r#"
INSERT INTO providers (
  id, name, provider_type, monthly_used_usd, created_at, updated_at
)
VALUES ('provider-1', 'Provider One', 'openai', 5.0, 1, 1);

INSERT INTO wallets (
  id, user_id, balance, gift_balance, limit_mode, created_at, updated_at
)
VALUES ('wallet-1', 'user-1', 10.0, 2.0, 'finite', 1, 1);

INSERT INTO "usage" (
  request_id, user_id, provider_id, status, billing_status, total_cost_usd, actual_total_cost_usd
)
VALUES
  ('request-1', 'user-1', 'provider-1', 'completed', 'pending', 3.0, 6.0),
  ('request-2', 'user-1', 'provider-1', 'failed', 'pending', 3.0, 2.0),
  ('request-overdraw', 'user-1', 'provider-1', 'completed', 'pending', 15.0, 15.0);
"#,
        )
        .execute(pool)
        .await
        .expect("settlement rows should seed");
    }

    async fn seed_quota_covered_settlement_rows(pool: &sqlx::SqlitePool) {
        sqlx::query(
            r#"
INSERT INTO users (
  id, username, email, role, auth_source, password_hash, is_active,
  is_deleted, created_at, updated_at
) VALUES (
  'user-quota', 'quota-user', 'quota@example.com', 'user', 'local',
  'hash', 1, 0, 1, 1
);

INSERT INTO wallets (
  id, user_id, balance, gift_balance, limit_mode, created_at, updated_at
) VALUES (
  'wallet-quota', 'user-quota', 0.0, 0.0, 'finite', 1, 1
);

INSERT INTO "usage" (
  request_id, user_id, api_key_id, status, billing_status,
  total_cost_usd, actual_total_cost_usd
) VALUES (
  'request-quota-covered', 'user-quota', 'key-quota', 'completed',
  'pending', 3.0, 6.0
);

INSERT INTO billing_plans (
  id, title, price_amount, price_currency, duration_unit,
  duration_value, entitlements_json, created_at, updated_at
) VALUES (
  'plan-quota', 'Quota Plan', 0.0, 'USD', 'month', 1,
  '[{"type":"daily_quota","daily_quota_usd":10.0,"reset_timezone":"Asia/Shanghai","allow_wallet_overage":false}]',
  1, 1
);

INSERT INTO payment_orders (
  id, order_no, wallet_id, user_id, amount_usd, refunded_amount_usd,
  refundable_amount_usd, payment_method, gateway_response, status, created_at
) VALUES (
  'order-quota', 'order-quota', 'wallet-quota', 'user-quota', 0.0, 0.0,
  0.0, 'admin_manual', '{}', 'credited', 1
);

INSERT INTO user_plan_entitlements (
  id, user_id, plan_id, payment_order_id, status, starts_at, expires_at,
  entitlements_snapshot, created_at, updated_at
) VALUES (
  'entitlement-quota', 'user-quota', 'plan-quota', 'order-quota',
  'active', 1, 9999999999,
  '[{"type":"daily_quota","daily_quota_usd":10.0,"reset_timezone":"Asia/Shanghai","allow_wallet_overage":false}]',
  1, 1
);
"#,
        )
        .execute(pool)
        .await
        .expect("quota settlement rows should seed");
    }
}
