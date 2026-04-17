use async_trait::async_trait;
use chrono::Utc;
use futures_util::{stream::TryStream, TryStreamExt};
use sqlx::{postgres::PgRow, PgPool, Row};
use uuid::Uuid;

use super::types::{
    redeem_code_credits_recharge_balance, redeem_code_payment_method,
    redeem_code_refundable_amount, AdjustWalletBalanceInput, AdminPaymentOrderListQuery,
    AdminRedeemCodeBatchListQuery, AdminRedeemCodeListQuery, AdminWalletLedgerQuery,
    AdminWalletListQuery, AdminWalletRefundRequestListQuery, CompleteAdminWalletRefundInput,
    CreateAdminRedeemCodeBatchInput, CreateAdminRedeemCodeBatchResult,
    CreateManualWalletRechargeInput, CreateWalletRechargeOrderInput,
    CreateWalletRechargeOrderOutcome, CreateWalletRefundRequestInput,
    CreateWalletRefundRequestOutcome, CreatedAdminRedeemCodePlaintext,
    CreditAdminPaymentOrderInput, DeleteAdminRedeemCodeBatchInput,
    DisableAdminRedeemCodeBatchInput, DisableAdminRedeemCodeInput, FailAdminWalletRefundInput,
    ProcessAdminWalletRefundInput, ProcessPaymentCallbackInput, ProcessPaymentCallbackOutcome,
    RedeemWalletCodeInput, RedeemWalletCodeOutcome, StoredAdminPaymentCallback,
    StoredAdminPaymentCallbackPage, StoredAdminPaymentOrder, StoredAdminPaymentOrderPage,
    StoredAdminRedeemCode, StoredAdminRedeemCodeBatch, StoredAdminRedeemCodeBatchPage,
    StoredAdminRedeemCodePage, StoredAdminWalletLedgerItem, StoredAdminWalletLedgerPage,
    StoredAdminWalletListItem, StoredAdminWalletListPage, StoredAdminWalletRefund,
    StoredAdminWalletRefundPage, StoredAdminWalletRefundRequestItem,
    StoredAdminWalletRefundRequestPage, StoredAdminWalletTransaction,
    StoredAdminWalletTransactionPage, StoredWalletDailyUsageLedger,
    StoredWalletDailyUsageLedgerPage, StoredWalletSnapshot, WalletLookupKey, WalletMutationOutcome,
    WalletReadRepository, WalletWriteRepository,
};
use crate::{
    error::{postgres_error, SqlxResultExt},
    postgres::PostgresTransactionRunner,
    DataLayerError,
};

const FIND_BY_WALLET_ID_SQL: &str = r#"
SELECT
  id,
  user_id,
  api_key_id,
  CAST(balance AS DOUBLE PRECISION) AS balance,
  CAST(gift_balance AS DOUBLE PRECISION) AS gift_balance,
  limit_mode,
  currency,
  status,
  CAST(total_recharged AS DOUBLE PRECISION) AS total_recharged,
  CAST(total_consumed AS DOUBLE PRECISION) AS total_consumed,
  CAST(total_refunded AS DOUBLE PRECISION) AS total_refunded,
  CAST(total_adjusted AS DOUBLE PRECISION) AS total_adjusted,
  CAST(EXTRACT(EPOCH FROM updated_at) AS BIGINT) AS updated_at_unix_secs
FROM wallets
WHERE id = $1
LIMIT 1
"#;

const FIND_BY_USER_ID_SQL: &str = r#"
SELECT
  id,
  user_id,
  api_key_id,
  CAST(balance AS DOUBLE PRECISION) AS balance,
  CAST(gift_balance AS DOUBLE PRECISION) AS gift_balance,
  limit_mode,
  currency,
  status,
  CAST(total_recharged AS DOUBLE PRECISION) AS total_recharged,
  CAST(total_consumed AS DOUBLE PRECISION) AS total_consumed,
  CAST(total_refunded AS DOUBLE PRECISION) AS total_refunded,
  CAST(total_adjusted AS DOUBLE PRECISION) AS total_adjusted,
  CAST(EXTRACT(EPOCH FROM updated_at) AS BIGINT) AS updated_at_unix_secs
FROM wallets
WHERE user_id = $1
LIMIT 1
"#;

const FIND_BY_API_KEY_ID_SQL: &str = r#"
SELECT
  id,
  user_id,
  api_key_id,
  CAST(balance AS DOUBLE PRECISION) AS balance,
  CAST(gift_balance AS DOUBLE PRECISION) AS gift_balance,
  limit_mode,
  currency,
  status,
  CAST(total_recharged AS DOUBLE PRECISION) AS total_recharged,
  CAST(total_consumed AS DOUBLE PRECISION) AS total_consumed,
  CAST(total_refunded AS DOUBLE PRECISION) AS total_refunded,
  CAST(total_adjusted AS DOUBLE PRECISION) AS total_adjusted,
  CAST(EXTRACT(EPOCH FROM updated_at) AS BIGINT) AS updated_at_unix_secs
FROM wallets
WHERE api_key_id = $1
LIMIT 1
"#;

const LIST_BY_USER_IDS_SQL: &str = r#"
SELECT
  id,
  user_id,
  api_key_id,
  CAST(balance AS DOUBLE PRECISION) AS balance,
  CAST(gift_balance AS DOUBLE PRECISION) AS gift_balance,
  limit_mode,
  currency,
  status,
  CAST(total_recharged AS DOUBLE PRECISION) AS total_recharged,
  CAST(total_consumed AS DOUBLE PRECISION) AS total_consumed,
  CAST(total_refunded AS DOUBLE PRECISION) AS total_refunded,
  CAST(total_adjusted AS DOUBLE PRECISION) AS total_adjusted,
  CAST(EXTRACT(EPOCH FROM updated_at) AS BIGINT) AS updated_at_unix_secs
FROM wallets
WHERE user_id = ANY($1)
"#;

const LIST_BY_API_KEY_IDS_SQL: &str = r#"
SELECT
  id,
  user_id,
  api_key_id,
  CAST(balance AS DOUBLE PRECISION) AS balance,
  CAST(gift_balance AS DOUBLE PRECISION) AS gift_balance,
  limit_mode,
  currency,
  status,
  CAST(total_recharged AS DOUBLE PRECISION) AS total_recharged,
  CAST(total_consumed AS DOUBLE PRECISION) AS total_consumed,
  CAST(total_refunded AS DOUBLE PRECISION) AS total_refunded,
  CAST(total_adjusted AS DOUBLE PRECISION) AS total_adjusted,
  CAST(EXTRACT(EPOCH FROM updated_at) AS BIGINT) AS updated_at_unix_secs
FROM wallets
WHERE api_key_id = ANY($1)
"#;

const COUNT_ADMIN_WALLETS_SQL: &str = r#"
SELECT COUNT(*) AS total
FROM wallets
WHERE ($1::TEXT IS NULL OR status = $1)
  AND (
    $2::TEXT IS NULL
    OR ($2 = 'user' AND user_id IS NOT NULL)
    OR ($2 = 'api_key' AND api_key_id IS NOT NULL)
  )
"#;

const LIST_ADMIN_WALLETS_SQL: &str = r#"
SELECT
  w.id,
  w.user_id,
  w.api_key_id,
  CAST(w.balance AS DOUBLE PRECISION) AS balance,
  CAST(w.gift_balance AS DOUBLE PRECISION) AS gift_balance,
  w.limit_mode,
  w.currency,
  w.status,
  CAST(w.total_recharged AS DOUBLE PRECISION) AS total_recharged,
  CAST(w.total_consumed AS DOUBLE PRECISION) AS total_consumed,
  CAST(w.total_refunded AS DOUBLE PRECISION) AS total_refunded,
  CAST(w.total_adjusted AS DOUBLE PRECISION) AS total_adjusted,
  users.username AS user_name,
  api_keys.name AS api_key_name,
  CAST(EXTRACT(EPOCH FROM w.created_at) AS BIGINT) AS created_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM w.updated_at) AS BIGINT) AS updated_at_unix_secs
FROM wallets w
LEFT JOIN users ON users.id = w.user_id
LEFT JOIN api_keys ON api_keys.id = w.api_key_id
WHERE ($1::TEXT IS NULL OR w.status = $1)
  AND (
    $2::TEXT IS NULL
    OR ($2 = 'user' AND w.user_id IS NOT NULL)
    OR ($2 = 'api_key' AND w.api_key_id IS NOT NULL)
  )
ORDER BY w.updated_at DESC
OFFSET $3
LIMIT $4
"#;

const COUNT_ADMIN_WALLET_LEDGER_SQL: &str = r#"
SELECT COUNT(*) AS total
FROM wallet_transactions tx
JOIN wallets w ON w.id = tx.wallet_id
WHERE ($1::TEXT IS NULL OR tx.category = $1)
  AND ($2::TEXT IS NULL OR tx.reason_code = $2)
  AND (
    $3::TEXT IS NULL
    OR ($3 = 'user' AND w.user_id IS NOT NULL)
    OR ($3 = 'api_key' AND w.api_key_id IS NOT NULL)
  )
"#;

const LIST_ADMIN_WALLET_LEDGER_SQL: &str = r#"
SELECT
  tx.id,
  tx.wallet_id,
  tx.category,
  tx.reason_code,
  CAST(tx.amount AS DOUBLE PRECISION) AS amount,
  CAST(tx.balance_before AS DOUBLE PRECISION) AS balance_before,
  CAST(tx.balance_after AS DOUBLE PRECISION) AS balance_after,
  CAST(tx.recharge_balance_before AS DOUBLE PRECISION) AS recharge_balance_before,
  CAST(tx.recharge_balance_after AS DOUBLE PRECISION) AS recharge_balance_after,
  CAST(tx.gift_balance_before AS DOUBLE PRECISION) AS gift_balance_before,
  CAST(tx.gift_balance_after AS DOUBLE PRECISION) AS gift_balance_after,
  tx.link_type,
  tx.link_id,
  tx.operator_id,
  tx.description,
  w.user_id,
  w.api_key_id,
  w.status AS wallet_status,
  wallet_users.username AS wallet_user_name,
  api_keys.name AS api_key_name,
  operator_users.username AS operator_name,
  operator_users.email AS operator_email,
  CAST(EXTRACT(EPOCH FROM tx.created_at) AS BIGINT) AS created_at_unix_ms
FROM wallet_transactions tx
JOIN wallets w ON w.id = tx.wallet_id
LEFT JOIN users wallet_users ON wallet_users.id = w.user_id
LEFT JOIN api_keys ON api_keys.id = w.api_key_id
LEFT JOIN users operator_users ON operator_users.id = tx.operator_id
WHERE ($1::TEXT IS NULL OR tx.category = $1)
  AND ($2::TEXT IS NULL OR tx.reason_code = $2)
  AND (
    $3::TEXT IS NULL
    OR ($3 = 'user' AND w.user_id IS NOT NULL)
    OR ($3 = 'api_key' AND w.api_key_id IS NOT NULL)
  )
ORDER BY tx.created_at DESC
OFFSET $4
LIMIT $5
"#;

const COUNT_ADMIN_WALLET_REFUND_REQUESTS_SQL: &str = r#"
SELECT COUNT(*) AS total
FROM refund_requests rr
JOIN wallets w ON w.id = rr.wallet_id
WHERE ($1::TEXT IS NULL OR rr.status = $1)
  AND w.user_id IS NOT NULL
"#;

const LIST_ADMIN_WALLET_REFUND_REQUESTS_SQL: &str = r#"
SELECT
  rr.id,
  rr.refund_no,
  rr.wallet_id,
  rr.user_id,
  rr.payment_order_id,
  rr.source_type,
  rr.source_id,
  rr.refund_mode,
  CAST(rr.amount_usd AS DOUBLE PRECISION) AS amount_usd,
  rr.status,
  rr.reason,
  rr.failure_reason,
  rr.gateway_refund_id,
  rr.payout_method,
  rr.payout_reference,
  rr.payout_proof,
  rr.requested_by,
  rr.approved_by,
  rr.processed_by,
  w.user_id AS wallet_user_id,
  w.api_key_id AS wallet_api_key_id,
  w.status AS wallet_status,
  wallet_users.username AS wallet_user_name,
  api_keys.name AS api_key_name,
  CAST(EXTRACT(EPOCH FROM rr.created_at) AS BIGINT) AS created_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM rr.updated_at) AS BIGINT) AS updated_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM rr.processed_at) AS BIGINT) AS processed_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM rr.completed_at) AS BIGINT) AS completed_at_unix_secs
FROM refund_requests rr
JOIN wallets w ON w.id = rr.wallet_id
LEFT JOIN users wallet_users ON wallet_users.id = w.user_id
LEFT JOIN api_keys ON api_keys.id = w.api_key_id
WHERE ($1::TEXT IS NULL OR rr.status = $1)
  AND w.user_id IS NOT NULL
ORDER BY rr.created_at DESC
OFFSET $2
LIMIT $3
"#;

const COUNT_ADMIN_WALLET_TRANSACTIONS_SQL: &str = r#"
SELECT COUNT(*) AS total
FROM wallet_transactions
WHERE wallet_id = $1
"#;

const LIST_ADMIN_WALLET_TRANSACTIONS_SQL: &str = r#"
SELECT
  tx.id,
  tx.wallet_id,
  tx.category,
  tx.reason_code,
  CAST(tx.amount AS DOUBLE PRECISION) AS amount,
  CAST(tx.balance_before AS DOUBLE PRECISION) AS balance_before,
  CAST(tx.balance_after AS DOUBLE PRECISION) AS balance_after,
  CAST(tx.recharge_balance_before AS DOUBLE PRECISION) AS recharge_balance_before,
  CAST(tx.recharge_balance_after AS DOUBLE PRECISION) AS recharge_balance_after,
  CAST(tx.gift_balance_before AS DOUBLE PRECISION) AS gift_balance_before,
  CAST(tx.gift_balance_after AS DOUBLE PRECISION) AS gift_balance_after,
  tx.link_type,
  tx.link_id,
  tx.operator_id,
  tx.description,
  operator_users.username AS operator_name,
  operator_users.email AS operator_email,
  CAST(EXTRACT(EPOCH FROM tx.created_at) AS BIGINT) AS created_at_unix_ms
FROM wallet_transactions tx
LEFT JOIN users operator_users
  ON operator_users.id = tx.operator_id
WHERE tx.wallet_id = $1
ORDER BY tx.created_at DESC
OFFSET $2
LIMIT $3
"#;

const FIND_WALLET_TODAY_USAGE_SQL: &str = r#"
SELECT
  id,
  billing_date::text AS billing_date,
  billing_timezone,
  CAST(total_cost_usd AS DOUBLE PRECISION) AS total_cost_usd,
  total_requests,
  input_tokens,
  output_tokens,
  cache_creation_tokens,
  cache_read_tokens,
  CAST(EXTRACT(EPOCH FROM first_finalized_at) AS BIGINT) AS first_finalized_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM last_finalized_at) AS BIGINT) AS last_finalized_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM aggregated_at) AS BIGINT) AS aggregated_at_unix_secs
FROM wallet_daily_usage_ledgers
WHERE wallet_id = $1
  AND billing_timezone = $2
  AND billing_date = (timezone($2, now()))::date
LIMIT 1
"#;

const COUNT_WALLET_DAILY_USAGE_HISTORY_SQL: &str = r#"
SELECT COUNT(*) AS total
FROM wallet_daily_usage_ledgers
WHERE wallet_id = $1
  AND billing_timezone = $2
  AND billing_date < (timezone($2, now()))::date
"#;

const LIST_WALLET_DAILY_USAGE_HISTORY_SQL: &str = r#"
SELECT
  id,
  billing_date::text AS billing_date,
  billing_timezone,
  CAST(total_cost_usd AS DOUBLE PRECISION) AS total_cost_usd,
  total_requests,
  input_tokens,
  output_tokens,
  cache_creation_tokens,
  cache_read_tokens,
  CAST(EXTRACT(EPOCH FROM first_finalized_at) AS BIGINT) AS first_finalized_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM last_finalized_at) AS BIGINT) AS last_finalized_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM aggregated_at) AS BIGINT) AS aggregated_at_unix_secs
FROM wallet_daily_usage_ledgers
WHERE wallet_id = $1
  AND billing_timezone = $2
  AND billing_date < (timezone($2, now()))::date
ORDER BY billing_date DESC
LIMIT $3
"#;

const COUNT_ADMIN_WALLET_REFUNDS_SQL: &str = r#"
SELECT COUNT(*) AS total
FROM refund_requests
WHERE wallet_id = $1
"#;

const LIST_ADMIN_WALLET_REFUNDS_SQL: &str = r#"
SELECT
  id,
  refund_no,
  wallet_id,
  user_id,
  payment_order_id,
  source_type,
  source_id,
  refund_mode,
  CAST(amount_usd AS DOUBLE PRECISION) AS amount_usd,
  status,
  reason,
  failure_reason,
  gateway_refund_id,
  payout_method,
  payout_reference,
  payout_proof,
  requested_by,
  approved_by,
  processed_by,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM updated_at) AS BIGINT) AS updated_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM processed_at) AS BIGINT) AS processed_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM completed_at) AS BIGINT) AS completed_at_unix_secs
FROM refund_requests
WHERE wallet_id = $1
ORDER BY created_at DESC
OFFSET $2
LIMIT $3
"#;

const COUNT_ADMIN_PAYMENT_ORDERS_SQL: &str = r#"
SELECT COUNT(*) AS total
FROM payment_orders
WHERE ($1::TEXT IS NULL OR payment_method = $1)
  AND (
    $2::TEXT IS NULL
    OR (
      CASE
        WHEN status = 'pending' AND expires_at IS NOT NULL AND expires_at < NOW() THEN 'expired'
        ELSE status
      END
    ) = $2
  )
"#;

const LIST_ADMIN_PAYMENT_ORDERS_SQL: &str = r#"
SELECT
  id,
  order_no,
  wallet_id,
  user_id,
  CAST(amount_usd AS DOUBLE PRECISION) AS amount_usd,
  CAST(pay_amount AS DOUBLE PRECISION) AS pay_amount,
  pay_currency,
  CAST(exchange_rate AS DOUBLE PRECISION) AS exchange_rate,
  CAST(refunded_amount_usd AS DOUBLE PRECISION) AS refunded_amount_usd,
  CAST(refundable_amount_usd AS DOUBLE PRECISION) AS refundable_amount_usd,
  payment_method,
  gateway_order_id,
  gateway_response,
  status,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM paid_at) AS BIGINT) AS paid_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM credited_at) AS BIGINT) AS credited_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM expires_at) AS BIGINT) AS expires_at_unix_secs
FROM payment_orders
WHERE ($1::TEXT IS NULL OR payment_method = $1)
  AND (
    $2::TEXT IS NULL
    OR (
      CASE
        WHEN status = 'pending' AND expires_at IS NOT NULL AND expires_at < NOW() THEN 'expired'
        ELSE status
      END
    ) = $2
  )
ORDER BY created_at DESC
OFFSET $3
LIMIT $4
"#;

const FIND_ADMIN_PAYMENT_ORDER_SQL: &str = r#"
SELECT
  id,
  order_no,
  wallet_id,
  user_id,
  CAST(amount_usd AS DOUBLE PRECISION) AS amount_usd,
  CAST(pay_amount AS DOUBLE PRECISION) AS pay_amount,
  pay_currency,
  CAST(exchange_rate AS DOUBLE PRECISION) AS exchange_rate,
  CAST(refunded_amount_usd AS DOUBLE PRECISION) AS refunded_amount_usd,
  CAST(refundable_amount_usd AS DOUBLE PRECISION) AS refundable_amount_usd,
  payment_method,
  gateway_order_id,
  gateway_response,
  status,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM paid_at) AS BIGINT) AS paid_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM credited_at) AS BIGINT) AS credited_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM expires_at) AS BIGINT) AS expires_at_unix_secs
FROM payment_orders
WHERE id = $1
LIMIT 1
"#;

const COUNT_WALLET_PAYMENT_ORDERS_BY_USER_SQL: &str = r#"
SELECT COUNT(*) AS total
FROM payment_orders
WHERE user_id = $1
"#;

const LIST_WALLET_PAYMENT_ORDERS_BY_USER_SQL: &str = r#"
SELECT
  id,
  order_no,
  wallet_id,
  user_id,
  CAST(amount_usd AS DOUBLE PRECISION) AS amount_usd,
  CAST(pay_amount AS DOUBLE PRECISION) AS pay_amount,
  pay_currency,
  CAST(exchange_rate AS DOUBLE PRECISION) AS exchange_rate,
  CAST(refunded_amount_usd AS DOUBLE PRECISION) AS refunded_amount_usd,
  CAST(refundable_amount_usd AS DOUBLE PRECISION) AS refundable_amount_usd,
  payment_method,
  gateway_order_id,
  gateway_response,
  CASE
    WHEN status = 'pending' AND expires_at IS NOT NULL AND expires_at < now() THEN 'expired'
    ELSE status
  END AS status,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM paid_at) AS BIGINT) AS paid_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM credited_at) AS BIGINT) AS credited_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM expires_at) AS BIGINT) AS expires_at_unix_secs
FROM payment_orders
WHERE user_id = $1
ORDER BY created_at DESC
OFFSET $2
LIMIT $3
"#;

const FIND_WALLET_PAYMENT_ORDER_BY_USER_SQL: &str = r#"
SELECT
  id,
  order_no,
  wallet_id,
  user_id,
  CAST(amount_usd AS DOUBLE PRECISION) AS amount_usd,
  CAST(pay_amount AS DOUBLE PRECISION) AS pay_amount,
  pay_currency,
  CAST(exchange_rate AS DOUBLE PRECISION) AS exchange_rate,
  CAST(refunded_amount_usd AS DOUBLE PRECISION) AS refunded_amount_usd,
  CAST(refundable_amount_usd AS DOUBLE PRECISION) AS refundable_amount_usd,
  payment_method,
  gateway_order_id,
  gateway_response,
  CASE
    WHEN status = 'pending' AND expires_at IS NOT NULL AND expires_at < now() THEN 'expired'
    ELSE status
  END AS status,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM paid_at) AS BIGINT) AS paid_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM credited_at) AS BIGINT) AS credited_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM expires_at) AS BIGINT) AS expires_at_unix_secs
FROM payment_orders
WHERE user_id = $1
  AND id = $2
LIMIT 1
"#;

const FIND_WALLET_REFUND_SQL: &str = r#"
SELECT
  id,
  refund_no,
  wallet_id,
  user_id,
  payment_order_id,
  source_type,
  source_id,
  refund_mode,
  CAST(amount_usd AS DOUBLE PRECISION) AS amount_usd,
  status,
  reason,
  failure_reason,
  gateway_refund_id,
  payout_method,
  payout_reference,
  payout_proof,
  requested_by,
  approved_by,
  processed_by,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM updated_at) AS BIGINT) AS updated_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM processed_at) AS BIGINT) AS processed_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM completed_at) AS BIGINT) AS completed_at_unix_secs
FROM refund_requests
WHERE wallet_id = $1
  AND id = $2
LIMIT 1
"#;

const COUNT_ADMIN_PAYMENT_CALLBACKS_SQL: &str = r#"
SELECT COUNT(*) AS total
FROM payment_callbacks
WHERE ($1::TEXT IS NULL OR payment_method = $1)
"#;

const LIST_ADMIN_PAYMENT_CALLBACKS_SQL: &str = r#"
SELECT
  id,
  payment_order_id,
  payment_method,
  callback_key,
  order_no,
  gateway_order_id,
  payload_hash,
  signature_valid,
  status,
  payload,
  error_message,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM processed_at) AS BIGINT) AS processed_at_unix_secs
FROM payment_callbacks
WHERE ($1::TEXT IS NULL OR payment_method = $1)
ORDER BY created_at DESC
OFFSET $2
LIMIT $3
"#;

#[derive(Debug, Clone)]
pub struct SqlxWalletRepository {
    pool: PgPool,
    tx_runner: PostgresTransactionRunner,
}

impl SqlxWalletRepository {
    pub fn new(pool: PgPool) -> Self {
        let tx_runner = PostgresTransactionRunner::new(pool.clone());
        Self { pool, tx_runner }
    }
}

#[async_trait]
impl WalletReadRepository for SqlxWalletRepository {
    async fn find(
        &self,
        key: WalletLookupKey<'_>,
    ) -> Result<Option<StoredWalletSnapshot>, DataLayerError> {
        let query = match key {
            WalletLookupKey::WalletId(_) => FIND_BY_WALLET_ID_SQL,
            WalletLookupKey::UserId(_) => FIND_BY_USER_ID_SQL,
            WalletLookupKey::ApiKeyId(_) => FIND_BY_API_KEY_ID_SQL,
        };
        let bind = match key {
            WalletLookupKey::WalletId(value)
            | WalletLookupKey::UserId(value)
            | WalletLookupKey::ApiKeyId(value) => value,
        };
        let row = sqlx::query(query)
            .bind(bind)
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?;
        row.as_ref().map(map_wallet_row).transpose()
    }
    async fn list_wallets_by_user_ids(
        &self,
        user_ids: &[String],
    ) -> Result<Vec<StoredWalletSnapshot>, DataLayerError> {
        if user_ids.is_empty() {
            return Ok(Vec::new());
        }
        collect_query_rows(
            sqlx::query(LIST_BY_USER_IDS_SQL)
                .bind(user_ids)
                .fetch(&self.pool),
            map_wallet_row,
        )
        .await
    }
    async fn list_wallets_by_api_key_ids(
        &self,
        api_key_ids: &[String],
    ) -> Result<Vec<StoredWalletSnapshot>, DataLayerError> {
        if api_key_ids.is_empty() {
            return Ok(Vec::new());
        }
        collect_query_rows(
            sqlx::query(LIST_BY_API_KEY_IDS_SQL)
                .bind(api_key_ids)
                .fetch(&self.pool),
            map_wallet_row,
        )
        .await
    }

    async fn list_admin_wallets(
        &self,
        query: &AdminWalletListQuery,
    ) -> Result<StoredAdminWalletListPage, DataLayerError> {
        let total = read_count(
            sqlx::query(COUNT_ADMIN_WALLETS_SQL)
                .bind(query.status.as_deref())
                .bind(query.owner_type.as_deref())
                .fetch_one(&self.pool)
                .await
                .map_postgres_err()?,
        )?;
        let items = collect_query_rows(
            sqlx::query(LIST_ADMIN_WALLETS_SQL)
                .bind(query.status.as_deref())
                .bind(query.owner_type.as_deref())
                .bind(as_i64(query.offset, "wallet offset")?)
                .bind(as_i64(query.limit, "wallet limit")?)
                .fetch(&self.pool),
            map_admin_wallet_list_item_row,
        )
        .await?;
        Ok(StoredAdminWalletListPage { items, total })
    }

    async fn list_admin_wallet_ledger(
        &self,
        query: &AdminWalletLedgerQuery,
    ) -> Result<StoredAdminWalletLedgerPage, DataLayerError> {
        let total = read_count(
            sqlx::query(COUNT_ADMIN_WALLET_LEDGER_SQL)
                .bind(query.category.as_deref())
                .bind(query.reason_code.as_deref())
                .bind(query.owner_type.as_deref())
                .fetch_one(&self.pool)
                .await
                .map_postgres_err()?,
        )?;
        let items = collect_query_rows(
            sqlx::query(LIST_ADMIN_WALLET_LEDGER_SQL)
                .bind(query.category.as_deref())
                .bind(query.reason_code.as_deref())
                .bind(query.owner_type.as_deref())
                .bind(as_i64(query.offset, "wallet ledger offset")?)
                .bind(as_i64(query.limit, "wallet ledger limit")?)
                .fetch(&self.pool),
            map_admin_wallet_ledger_item_row,
        )
        .await?;
        Ok(StoredAdminWalletLedgerPage { items, total })
    }

    async fn list_admin_wallet_refund_requests(
        &self,
        query: &AdminWalletRefundRequestListQuery,
    ) -> Result<StoredAdminWalletRefundRequestPage, DataLayerError> {
        let total = read_count(
            sqlx::query(COUNT_ADMIN_WALLET_REFUND_REQUESTS_SQL)
                .bind(query.status.as_deref())
                .fetch_one(&self.pool)
                .await
                .map_postgres_err()?,
        )?;
        let items = collect_query_rows(
            sqlx::query(LIST_ADMIN_WALLET_REFUND_REQUESTS_SQL)
                .bind(query.status.as_deref())
                .bind(as_i64(query.offset, "wallet refund request offset")?)
                .bind(as_i64(query.limit, "wallet refund request limit")?)
                .fetch(&self.pool),
            map_admin_wallet_refund_request_item_row,
        )
        .await?;
        Ok(StoredAdminWalletRefundRequestPage { items, total })
    }

    async fn list_admin_wallet_transactions(
        &self,
        wallet_id: &str,
        limit: usize,
        offset: usize,
    ) -> Result<StoredAdminWalletTransactionPage, DataLayerError> {
        let total = read_count(
            sqlx::query(COUNT_ADMIN_WALLET_TRANSACTIONS_SQL)
                .bind(wallet_id)
                .fetch_one(&self.pool)
                .await
                .map_postgres_err()?,
        )?;
        let items = collect_query_rows(
            sqlx::query(LIST_ADMIN_WALLET_TRANSACTIONS_SQL)
                .bind(wallet_id)
                .bind(as_i64(offset, "wallet transaction offset")?)
                .bind(as_i64(limit, "wallet transaction limit")?)
                .fetch(&self.pool),
            map_admin_wallet_transaction_row,
        )
        .await?;
        Ok(StoredAdminWalletTransactionPage { items, total })
    }

    async fn find_wallet_today_usage(
        &self,
        wallet_id: &str,
        billing_timezone: &str,
    ) -> Result<Option<StoredWalletDailyUsageLedger>, DataLayerError> {
        let row = sqlx::query(FIND_WALLET_TODAY_USAGE_SQL)
            .bind(wallet_id)
            .bind(billing_timezone)
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?;
        row.as_ref().map(map_wallet_daily_usage_row).transpose()
    }

    async fn list_wallet_daily_usage_history(
        &self,
        wallet_id: &str,
        billing_timezone: &str,
        limit: usize,
    ) -> Result<StoredWalletDailyUsageLedgerPage, DataLayerError> {
        let total = read_count(
            sqlx::query(COUNT_WALLET_DAILY_USAGE_HISTORY_SQL)
                .bind(wallet_id)
                .bind(billing_timezone)
                .fetch_one(&self.pool)
                .await
                .map_postgres_err()?,
        )?;
        let items = collect_query_rows(
            sqlx::query(LIST_WALLET_DAILY_USAGE_HISTORY_SQL)
                .bind(wallet_id)
                .bind(billing_timezone)
                .bind(as_i64(limit, "wallet daily usage history limit")?)
                .fetch(&self.pool),
            map_wallet_daily_usage_row,
        )
        .await?;
        Ok(StoredWalletDailyUsageLedgerPage { items, total })
    }

    async fn list_admin_wallet_refunds(
        &self,
        wallet_id: &str,
        limit: usize,
        offset: usize,
    ) -> Result<StoredAdminWalletRefundPage, DataLayerError> {
        let total = read_count(
            sqlx::query(COUNT_ADMIN_WALLET_REFUNDS_SQL)
                .bind(wallet_id)
                .fetch_one(&self.pool)
                .await
                .map_postgres_err()?,
        )?;
        let items = collect_query_rows(
            sqlx::query(LIST_ADMIN_WALLET_REFUNDS_SQL)
                .bind(wallet_id)
                .bind(as_i64(offset, "wallet refund offset")?)
                .bind(as_i64(limit, "wallet refund limit")?)
                .fetch(&self.pool),
            map_admin_wallet_refund_row,
        )
        .await?;
        Ok(StoredAdminWalletRefundPage { items, total })
    }

    async fn list_admin_payment_orders(
        &self,
        query: &AdminPaymentOrderListQuery,
    ) -> Result<StoredAdminPaymentOrderPage, DataLayerError> {
        let total = read_count(
            sqlx::query(COUNT_ADMIN_PAYMENT_ORDERS_SQL)
                .bind(query.payment_method.as_deref())
                .bind(query.status.as_deref())
                .fetch_one(&self.pool)
                .await
                .map_postgres_err()?,
        )?;
        let items = collect_query_rows(
            sqlx::query(LIST_ADMIN_PAYMENT_ORDERS_SQL)
                .bind(query.payment_method.as_deref())
                .bind(query.status.as_deref())
                .bind(as_i64(query.offset, "payment order offset")?)
                .bind(as_i64(query.limit, "payment order limit")?)
                .fetch(&self.pool),
            map_admin_payment_order_row,
        )
        .await?;
        Ok(StoredAdminPaymentOrderPage { items, total })
    }

    async fn find_admin_payment_order(
        &self,
        order_id: &str,
    ) -> Result<Option<StoredAdminPaymentOrder>, DataLayerError> {
        let row = sqlx::query(FIND_ADMIN_PAYMENT_ORDER_SQL)
            .bind(order_id)
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?;
        row.as_ref().map(map_admin_payment_order_row).transpose()
    }

    async fn list_wallet_payment_orders_by_user_id(
        &self,
        user_id: &str,
        limit: usize,
        offset: usize,
    ) -> Result<StoredAdminPaymentOrderPage, DataLayerError> {
        let total = read_count(
            sqlx::query(COUNT_WALLET_PAYMENT_ORDERS_BY_USER_SQL)
                .bind(user_id)
                .fetch_one(&self.pool)
                .await
                .map_postgres_err()?,
        )?;
        let items = collect_query_rows(
            sqlx::query(LIST_WALLET_PAYMENT_ORDERS_BY_USER_SQL)
                .bind(user_id)
                .bind(as_i64(offset, "wallet payment order offset")?)
                .bind(as_i64(limit, "wallet payment order limit")?)
                .fetch(&self.pool),
            map_admin_payment_order_row,
        )
        .await?;
        Ok(StoredAdminPaymentOrderPage { items, total })
    }

    async fn find_wallet_payment_order_by_user_id(
        &self,
        user_id: &str,
        order_id: &str,
    ) -> Result<Option<StoredAdminPaymentOrder>, DataLayerError> {
        let row = sqlx::query(FIND_WALLET_PAYMENT_ORDER_BY_USER_SQL)
            .bind(user_id)
            .bind(order_id)
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?;
        row.as_ref().map(map_admin_payment_order_row).transpose()
    }

    async fn find_wallet_refund(
        &self,
        wallet_id: &str,
        refund_id: &str,
    ) -> Result<Option<StoredAdminWalletRefund>, DataLayerError> {
        let row = sqlx::query(FIND_WALLET_REFUND_SQL)
            .bind(wallet_id)
            .bind(refund_id)
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?;
        row.as_ref().map(map_admin_wallet_refund_row).transpose()
    }

    async fn list_admin_payment_callbacks(
        &self,
        payment_method: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<StoredAdminPaymentCallbackPage, DataLayerError> {
        let total = read_count(
            sqlx::query(COUNT_ADMIN_PAYMENT_CALLBACKS_SQL)
                .bind(payment_method)
                .fetch_one(&self.pool)
                .await
                .map_postgres_err()?,
        )?;
        let items = collect_query_rows(
            sqlx::query(LIST_ADMIN_PAYMENT_CALLBACKS_SQL)
                .bind(payment_method)
                .bind(as_i64(offset, "payment callback offset")?)
                .bind(as_i64(limit, "payment callback limit")?)
                .fetch(&self.pool),
            map_admin_payment_callback_row,
        )
        .await?;
        Ok(StoredAdminPaymentCallbackPage { items, total })
    }

    async fn list_admin_redeem_code_batches(
        &self,
        query: &AdminRedeemCodeBatchListQuery,
    ) -> Result<StoredAdminRedeemCodeBatchPage, DataLayerError> {
        let total = read_count(
            sqlx::query(
                r#"
SELECT COUNT(*) AS total
FROM redeem_code_batches
WHERE $1::TEXT IS NULL OR status = $1
                "#,
            )
            .bind(query.status.as_deref())
            .fetch_one(&self.pool)
            .await
            .map_postgres_err()?,
        )?;
        let items = collect_query_rows(
            sqlx::query(
                r#"
SELECT
  batches.id,
  batches.name,
  CAST(batches.amount_usd AS DOUBLE PRECISION) AS amount_usd,
  batches.currency,
  batches.balance_bucket,
  CAST(batches.total_count AS BIGINT) AS total_count,
  CAST(COALESCE(stats.redeemed_count, 0) AS BIGINT) AS redeemed_count,
  CAST(COALESCE(stats.active_count, 0) AS BIGINT) AS active_count,
  batches.status,
  batches.description,
  batches.created_by,
  CAST(EXTRACT(EPOCH FROM batches.expires_at) AS BIGINT) AS expires_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM batches.created_at) AS BIGINT) AS created_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM batches.updated_at) AS BIGINT) AS updated_at_unix_secs
FROM redeem_code_batches AS batches
LEFT JOIN (
  SELECT
    batch_id,
    COUNT(*) FILTER (WHERE status = 'redeemed') AS redeemed_count,
    COUNT(*) FILTER (WHERE status = 'active') AS active_count
  FROM redeem_codes
  GROUP BY batch_id
) AS stats
  ON stats.batch_id = batches.id
WHERE $1::TEXT IS NULL OR batches.status = $1
ORDER BY batches.created_at DESC, batches.id DESC
OFFSET $2
LIMIT $3
                "#,
            )
            .bind(query.status.as_deref())
            .bind(as_i64(query.offset, "redeem code batch offset")?)
            .bind(as_i64(query.limit, "redeem code batch limit")?)
            .fetch(&self.pool),
            map_admin_redeem_code_batch_row,
        )
        .await?;
        Ok(StoredAdminRedeemCodeBatchPage { items, total })
    }

    async fn find_admin_redeem_code_batch(
        &self,
        batch_id: &str,
    ) -> Result<Option<StoredAdminRedeemCodeBatch>, DataLayerError> {
        let row = sqlx::query(
            r#"
SELECT
  batches.id,
  batches.name,
  CAST(batches.amount_usd AS DOUBLE PRECISION) AS amount_usd,
  batches.currency,
  batches.balance_bucket,
  CAST(batches.total_count AS BIGINT) AS total_count,
  CAST(COALESCE(stats.redeemed_count, 0) AS BIGINT) AS redeemed_count,
  CAST(COALESCE(stats.active_count, 0) AS BIGINT) AS active_count,
  batches.status,
  batches.description,
  batches.created_by,
  CAST(EXTRACT(EPOCH FROM batches.expires_at) AS BIGINT) AS expires_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM batches.created_at) AS BIGINT) AS created_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM batches.updated_at) AS BIGINT) AS updated_at_unix_secs
FROM redeem_code_batches AS batches
LEFT JOIN (
  SELECT
    batch_id,
    COUNT(*) FILTER (WHERE status = 'redeemed') AS redeemed_count,
    COUNT(*) FILTER (WHERE status = 'active') AS active_count
  FROM redeem_codes
  GROUP BY batch_id
) AS stats
  ON stats.batch_id = batches.id
WHERE batches.id = $1
LIMIT 1
            "#,
        )
        .bind(batch_id)
        .fetch_optional(&self.pool)
        .await
        .map_postgres_err()?;
        row.as_ref()
            .map(map_admin_redeem_code_batch_row)
            .transpose()
    }

    async fn list_admin_redeem_codes(
        &self,
        query: &AdminRedeemCodeListQuery,
    ) -> Result<StoredAdminRedeemCodePage, DataLayerError> {
        let total = read_count(
            sqlx::query(
                r#"
SELECT COUNT(*) AS total
FROM redeem_codes
WHERE batch_id = $1
  AND ($2::TEXT IS NULL OR status = $2)
                "#,
            )
            .bind(&query.batch_id)
            .bind(query.status.as_deref())
            .fetch_one(&self.pool)
            .await
            .map_postgres_err()?,
        )?;
        let items = collect_query_rows(
            sqlx::query(
                r#"
SELECT
  codes.id,
  codes.batch_id,
  batches.name AS batch_name,
  codes.code_prefix,
  codes.code_suffix,
  codes.status,
  codes.redeemed_by_user_id,
  users.username AS redeemed_by_user_name,
  codes.redeemed_wallet_id,
  codes.redeemed_payment_order_id,
  orders.order_no AS redeemed_order_no,
  CAST(EXTRACT(EPOCH FROM codes.redeemed_at) AS BIGINT) AS redeemed_at_unix_secs,
  codes.disabled_by,
  CAST(EXTRACT(EPOCH FROM batches.expires_at) AS BIGINT) AS expires_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM codes.created_at) AS BIGINT) AS created_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM codes.updated_at) AS BIGINT) AS updated_at_unix_secs
FROM redeem_codes AS codes
JOIN redeem_code_batches AS batches
  ON batches.id = codes.batch_id
LEFT JOIN users
  ON users.id = codes.redeemed_by_user_id
LEFT JOIN payment_orders AS orders
  ON orders.id = codes.redeemed_payment_order_id
WHERE codes.batch_id = $1
  AND ($2::TEXT IS NULL OR codes.status = $2)
ORDER BY codes.created_at DESC, codes.id DESC
OFFSET $3
LIMIT $4
                "#,
            )
            .bind(&query.batch_id)
            .bind(query.status.as_deref())
            .bind(as_i64(query.offset, "redeem code offset")?)
            .bind(as_i64(query.limit, "redeem code limit")?)
            .fetch(&self.pool),
            map_admin_redeem_code_row,
        )
        .await?;
        Ok(StoredAdminRedeemCodePage { items, total })
    }
}

#[async_trait]
impl WalletWriteRepository for SqlxWalletRepository {
    async fn create_wallet_recharge_order(
        &self,
        input: CreateWalletRechargeOrderInput,
    ) -> Result<CreateWalletRechargeOrderOutcome, DataLayerError> {
        self.tx_runner
            .run_read_write(|tx| {
                Box::pin(async move {
                    let wallet_row = match sqlx::query(
                        r#"
SELECT id, status
FROM wallets
WHERE user_id = $1
LIMIT 1
FOR UPDATE
                        "#,
                    )
                    .bind(&input.user_id)
                    .fetch_optional(&mut **tx)
                    .await
                    .map_postgres_err()?
                    {
                        Some(row) => row,
                        None => {
                            let wallet_id = input
                                .preferred_wallet_id
                                .clone()
                                .unwrap_or_else(|| Uuid::new_v4().to_string());
                            sqlx::query(
                                r#"
INSERT INTO wallets (
  id,
  user_id,
  balance,
  gift_balance,
  limit_mode,
  currency,
  status,
  total_recharged,
  total_consumed,
  total_refunded,
  total_adjusted,
  created_at,
  updated_at
)
VALUES (
  $1,
  $2,
  0,
  0,
  'finite',
  'USD',
  'active',
  0,
  0,
  0,
  0,
  NOW(),
  NOW()
)
ON CONFLICT (user_id) DO UPDATE
SET updated_at = wallets.updated_at
RETURNING id, status
                                "#,
                            )
                            .bind(&wallet_id)
                            .bind(&input.user_id)
                            .fetch_one(&mut **tx)
                            .await
                            .map_postgres_err()?
                        }
                    };
                    let wallet_id: String = row_get(&wallet_row, "id")?;
                    let wallet_status: String = row_get(&wallet_row, "status")?;
                    if wallet_status != "active" {
                        return Ok(CreateWalletRechargeOrderOutcome::WalletInactive);
                    }

                    let expires_at = i64::try_from(input.expires_at_unix_secs).map_err(|_| {
                        DataLayerError::InvalidInput(
                            "wallet recharge expires_at overflow".to_string(),
                        )
                    })?;
                    let row = sqlx::query(
                        r#"
INSERT INTO payment_orders (
  id,
  order_no,
  wallet_id,
  user_id,
  amount_usd,
  pay_amount,
  pay_currency,
  exchange_rate,
  refunded_amount_usd,
  refundable_amount_usd,
  payment_method,
  gateway_order_id,
  gateway_response,
  status,
  created_at,
  expires_at
)
VALUES (
  $1,
  $2,
  $3,
  $4,
  $5,
  $6,
  $7,
  $8,
  0,
  0,
  $9,
  $10,
  $11,
  'pending',
  NOW(),
  to_timestamp($12)
)
RETURNING
  id,
  order_no,
  wallet_id,
  user_id,
  CAST(amount_usd AS DOUBLE PRECISION) AS amount_usd,
  CAST(pay_amount AS DOUBLE PRECISION) AS pay_amount,
  pay_currency,
  CAST(exchange_rate AS DOUBLE PRECISION) AS exchange_rate,
  CAST(refunded_amount_usd AS DOUBLE PRECISION) AS refunded_amount_usd,
  CAST(refundable_amount_usd AS DOUBLE PRECISION) AS refundable_amount_usd,
  payment_method,
  gateway_order_id,
  gateway_response,
  status,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM paid_at) AS BIGINT) AS paid_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM credited_at) AS BIGINT) AS credited_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM expires_at) AS BIGINT) AS expires_at_unix_secs
                        "#,
                    )
                    .bind(Uuid::new_v4().to_string())
                    .bind(&input.order_no)
                    .bind(&wallet_id)
                    .bind(&input.user_id)
                    .bind(input.amount_usd)
                    .bind(input.pay_amount)
                    .bind(input.pay_currency.as_deref())
                    .bind(input.exchange_rate)
                    .bind(&input.payment_method)
                    .bind(&input.gateway_order_id)
                    .bind(&input.gateway_response)
                    .bind(expires_at)
                    .fetch_one(&mut **tx)
                    .await
                    .map_postgres_err()?;
                    Ok(CreateWalletRechargeOrderOutcome::Created(
                        map_admin_payment_order_row(&row)?,
                    ))
                })
            })
            .await
    }

    async fn create_wallet_refund_request(
        &self,
        input: CreateWalletRefundRequestInput,
    ) -> Result<CreateWalletRefundRequestOutcome, DataLayerError> {
        self.tx_runner
            .run_read_write(|tx| {
                Box::pin(async move {
                    let Some(locked_wallet_row) = sqlx::query(
                        r#"
SELECT
  id,
  CAST(balance AS DOUBLE PRECISION) AS balance
FROM wallets
WHERE id = $1
LIMIT 1
FOR UPDATE
                        "#,
                    )
                    .bind(&input.wallet_id)
                    .fetch_optional(&mut **tx)
                    .await
                    .map_postgres_err()?
                    else {
                        return Ok(CreateWalletRefundRequestOutcome::WalletMissing);
                    };
                    let wallet_recharge_balance: f64 = row_get(&locked_wallet_row, "balance")?;
                    let wallet_reserved_row = sqlx::query(
                        r#"
SELECT COALESCE(CAST(SUM(amount_usd) AS DOUBLE PRECISION), 0) AS total
FROM refund_requests
WHERE wallet_id = $1
  AND status IN ('pending_approval', 'approved')
                        "#,
                    )
                    .bind(&input.wallet_id)
                    .fetch_one(&mut **tx)
                    .await
                    .map_postgres_err()?;
                    let wallet_reserved_amount: f64 = row_get(&wallet_reserved_row, "total")?;
                    if input.amount_usd > (wallet_recharge_balance - wallet_reserved_amount) {
                        return Ok(
                            CreateWalletRefundRequestOutcome::RefundAmountExceedsAvailableBalance,
                        );
                    }

                    let mut payment_order_id = None;
                    let mut source_type = input
                        .source_type
                        .clone()
                        .unwrap_or_else(|| "wallet_balance".to_string());
                    let mut source_id = input.source_id.clone();
                    let mut refund_mode = input
                        .refund_mode
                        .clone()
                        .unwrap_or_else(|| "offline_payout".to_string());
                    if let Some(order_id) = input.payment_order_id.as_deref() {
                        let Some(order_row) = sqlx::query(
                            r#"
SELECT
  id,
  status,
  payment_method,
  CAST(refundable_amount_usd AS DOUBLE PRECISION) AS refundable_amount_usd
FROM payment_orders
WHERE id = $1
  AND wallet_id = $2
LIMIT 1
FOR UPDATE
                            "#,
                        )
                        .bind(order_id)
                        .bind(&input.wallet_id)
                        .fetch_optional(&mut **tx)
                        .await
                        .map_postgres_err()?
                        else {
                            return Ok(CreateWalletRefundRequestOutcome::PaymentOrderNotFound);
                        };
                        let status: String = row_get(&order_row, "status")?;
                        if status != "credited" {
                            return Ok(CreateWalletRefundRequestOutcome::PaymentOrderNotRefundable);
                        }
                        let order_reserved_row = sqlx::query(
                            r#"
SELECT COALESCE(CAST(SUM(amount_usd) AS DOUBLE PRECISION), 0) AS total
FROM refund_requests
WHERE payment_order_id = $1
  AND status IN ('pending_approval', 'approved')
                            "#,
                        )
                        .bind(order_id)
                        .fetch_one(&mut **tx)
                        .await
                        .map_postgres_err()?;
                        let refundable_amount: f64 =
                            row_get(&order_row, "refundable_amount_usd")?;
                        let reserved_amount: f64 = row_get(&order_reserved_row, "total")?;
                        if input.amount_usd > (refundable_amount - reserved_amount) {
                            return Ok(
                                CreateWalletRefundRequestOutcome::RefundAmountExceedsAvailableOrderAmount,
                            );
                        }
                        payment_order_id = Some(order_id.to_string());
                        source_type = "payment_order".to_string();
                        source_id = Some(order_id.to_string());
                        if input.refund_mode.is_none() {
                            let payment_method: String = row_get(&order_row, "payment_method")?;
                            refund_mode =
                                default_refund_mode_for_payment_method(&payment_method).to_string();
                        }
                    }

                    let insert_result = sqlx::query(
                        r#"
INSERT INTO refund_requests (
  id,
  refund_no,
  wallet_id,
  user_id,
  payment_order_id,
  source_type,
  source_id,
  refund_mode,
  amount_usd,
  status,
  reason,
  requested_by,
  idempotency_key,
  created_at,
  updated_at
)
VALUES (
  $1,
  $2,
  $3,
  $4,
  $5,
  $6,
  $7,
  $8,
  $9,
  'pending_approval',
  $10,
  $11,
  $12,
  NOW(),
  NOW()
)
RETURNING
  id,
  refund_no,
  wallet_id,
  user_id,
  payment_order_id,
  source_type,
  source_id,
  refund_mode,
  CAST(amount_usd AS DOUBLE PRECISION) AS amount_usd,
  status,
  reason,
  failure_reason,
  gateway_refund_id,
  payout_method,
  payout_reference,
  payout_proof,
  requested_by,
  approved_by,
  processed_by,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM updated_at) AS BIGINT) AS updated_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM processed_at) AS BIGINT) AS processed_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM completed_at) AS BIGINT) AS completed_at_unix_secs
                        "#,
                    )
                    .bind(Uuid::new_v4().to_string())
                    .bind(&input.refund_no)
                    .bind(&input.wallet_id)
                    .bind(&input.user_id)
                    .bind(payment_order_id.as_deref())
                    .bind(&source_type)
                    .bind(source_id.as_deref())
                    .bind(&refund_mode)
                    .bind(input.amount_usd)
                    .bind(input.reason.as_deref())
                    .bind(&input.user_id)
                    .bind(input.idempotency_key.as_deref())
                    .fetch_one(&mut **tx)
                    .await;
                    match insert_result {
                        Ok(row) => Ok(CreateWalletRefundRequestOutcome::Created(
                            map_admin_wallet_refund_row(&row)?,
                        )),
                        Err(sqlx::Error::Database(err))
                            if err.code().as_deref() == Some("23505") =>
                        {
                            if let Some(idempotency_key) = input.idempotency_key.as_deref() {
                                let existing = sqlx::query(
                                    r#"
SELECT
  id,
  refund_no,
  wallet_id,
  user_id,
  payment_order_id,
  source_type,
  source_id,
  refund_mode,
  CAST(amount_usd AS DOUBLE PRECISION) AS amount_usd,
  status,
  reason,
  failure_reason,
  gateway_refund_id,
  payout_method,
  payout_reference,
  payout_proof,
  requested_by,
  approved_by,
  processed_by,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM updated_at) AS BIGINT) AS updated_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM processed_at) AS BIGINT) AS processed_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM completed_at) AS BIGINT) AS completed_at_unix_secs
FROM refund_requests
WHERE user_id = $1
  AND idempotency_key = $2
LIMIT 1
                                    "#,
                                )
                                .bind(&input.user_id)
                                .bind(idempotency_key)
                                .fetch_optional(&mut **tx)
                                .await
                                .map_postgres_err()?;
                                if let Some(row) = existing {
                                    return Ok(CreateWalletRefundRequestOutcome::Duplicate(
                                        map_admin_wallet_refund_row(&row)?,
                                    ));
                                }
                            }
                            Ok(CreateWalletRefundRequestOutcome::DuplicateRejected)
                        }
                        Err(err) => Err(postgres_error(err)),
                    }
                })
            })
            .await
    }

    async fn process_payment_callback(
        &self,
        input: ProcessPaymentCallbackInput,
    ) -> Result<ProcessPaymentCallbackOutcome, DataLayerError> {
        self.tx_runner
            .run_read_write(|tx| {
                Box::pin(async move {
                    let existing_callback = sqlx::query(
                        r#"
SELECT id, payment_order_id, status, order_no, gateway_order_id
FROM payment_callbacks
WHERE callback_key = $1
LIMIT 1
                        "#,
                    )
                    .bind(&input.callback_key)
                    .fetch_optional(&mut **tx)
                    .await
                    .map_postgres_err()?;

                    let duplicate = existing_callback.is_some();
                    let callback_id = if let Some(row) = existing_callback.as_ref() {
                        let status: String = row_get(row, "status")?;
                        if status == "processed" {
                            return Ok(ProcessPaymentCallbackOutcome::DuplicateProcessed {
                                order_id: row_get(row, "payment_order_id")?,
                            });
                        }
                        row_get(row, "id")?
                    } else {
                        let callback_id = Uuid::new_v4().to_string();
                        sqlx::query(
                            r#"
INSERT INTO payment_callbacks (
  id,
  payment_order_id,
  payment_method,
  callback_key,
  order_no,
  gateway_order_id,
  payload_hash,
  signature_valid,
  status,
  payload,
  error_message,
  created_at,
  processed_at
)
VALUES (
  $1,
  NULL,
  $2,
  $3,
  $4,
  $5,
  $6,
  $7,
  'received',
  $8,
  NULL,
  NOW(),
  NULL
)
                            "#,
                        )
                        .bind(&callback_id)
                        .bind(&input.payment_method)
                        .bind(&input.callback_key)
                        .bind(input.order_no.as_deref())
                        .bind(input.gateway_order_id.as_deref())
                        .bind(&input.payload_hash)
                        .bind(input.signature_valid)
                        .bind(&input.payload)
                        .execute(&mut **tx)
                        .await
                        .map_postgres_err()?;
                        callback_id
                    };

                    if !input.signature_valid {
                        update_payment_callback_failure(
                            tx,
                            &callback_id,
                            &input,
                            "invalid callback signature",
                        )
                        .await?;
                        return Ok(ProcessPaymentCallbackOutcome::Failed {
                            duplicate,
                            error: "invalid callback signature".to_string(),
                        });
                    }

                    let lookup_order_no = input.order_no.clone().or_else(|| {
                        existing_callback
                            .as_ref()
                            .and_then(|row| row.try_get("order_no").ok())
                    });
                    let lookup_gateway_order_id = input.gateway_order_id.clone().or_else(|| {
                        existing_callback
                            .as_ref()
                            .and_then(|row| row.try_get("gateway_order_id").ok())
                    });

                    let order_row = if let Some(order_no) = lookup_order_no.as_deref() {
                        sqlx::query(
                            r#"
SELECT
  id,
  order_no,
  wallet_id,
  user_id,
  CAST(amount_usd AS DOUBLE PRECISION) AS amount_usd,
  CAST(pay_amount AS DOUBLE PRECISION) AS pay_amount,
  pay_currency,
  CAST(exchange_rate AS DOUBLE PRECISION) AS exchange_rate,
  CAST(refunded_amount_usd AS DOUBLE PRECISION) AS refunded_amount_usd,
  CAST(refundable_amount_usd AS DOUBLE PRECISION) AS refundable_amount_usd,
  payment_method,
  gateway_order_id,
  gateway_response,
  status,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM paid_at) AS BIGINT) AS paid_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM credited_at) AS BIGINT) AS credited_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM expires_at) AS BIGINT) AS expires_at_unix_secs
FROM payment_orders
WHERE order_no = $1
LIMIT 1
FOR UPDATE
                            "#,
                        )
                        .bind(order_no)
                        .fetch_optional(&mut **tx)
                        .await
                        .map_postgres_err()?
                    } else if let Some(gateway_order_id) = lookup_gateway_order_id.as_deref() {
                        sqlx::query(
                            r#"
SELECT
  id,
  order_no,
  wallet_id,
  user_id,
  CAST(amount_usd AS DOUBLE PRECISION) AS amount_usd,
  CAST(pay_amount AS DOUBLE PRECISION) AS pay_amount,
  pay_currency,
  CAST(exchange_rate AS DOUBLE PRECISION) AS exchange_rate,
  CAST(refunded_amount_usd AS DOUBLE PRECISION) AS refunded_amount_usd,
  CAST(refundable_amount_usd AS DOUBLE PRECISION) AS refundable_amount_usd,
  payment_method,
  gateway_order_id,
  gateway_response,
  status,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM paid_at) AS BIGINT) AS paid_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM credited_at) AS BIGINT) AS credited_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM expires_at) AS BIGINT) AS expires_at_unix_secs
FROM payment_orders
WHERE gateway_order_id = $1
LIMIT 1
FOR UPDATE
                            "#,
                        )
                        .bind(gateway_order_id)
                        .fetch_optional(&mut **tx)
                        .await
                        .map_postgres_err()?
                    } else {
                        None
                    };

                    let Some(order_row) = order_row else {
                        update_payment_callback_failure(
                            tx,
                            &callback_id,
                            &input,
                            "payment order not found",
                        )
                        .await?;
                        return Ok(ProcessPaymentCallbackOutcome::Failed {
                            duplicate,
                            error: "payment order not found".to_string(),
                        });
                    };

                    let order_id: String = row_get(&order_row, "id")?;
                    let order_no: String = row_get(&order_row, "order_no")?;
                    let order_wallet_id: String = row_get(&order_row, "wallet_id")?;
                    let order_payment_method: String = row_get(&order_row, "payment_method")?;
                    let order_amount_usd: f64 = row_get(&order_row, "amount_usd")?;
                    let order_status: String = row_get(&order_row, "status")?;
                    let expires_at_unix_secs: Option<i64> =
                        row_get(&order_row, "expires_at_unix_secs")?;

                    if (input.amount_usd - order_amount_usd).abs() > f64::EPSILON {
                        update_payment_callback_failure(
                            tx,
                            &callback_id,
                            &input,
                            "callback amount mismatch",
                        )
                        .await?;
                        return Ok(ProcessPaymentCallbackOutcome::Failed {
                            duplicate,
                            error: "callback amount mismatch".to_string(),
                        });
                    }
                    if !order_payment_method.eq_ignore_ascii_case(&input.payment_method) {
                        update_payment_callback_failure(
                            tx,
                            &callback_id,
                            &input,
                            "payment method mismatch",
                        )
                        .await?;
                        return Ok(ProcessPaymentCallbackOutcome::Failed {
                            duplicate,
                            error: "payment method mismatch".to_string(),
                        });
                    }
                    if order_status == "credited" {
                        mark_payment_callback_processed(
                            tx,
                            &callback_id,
                            &input,
                            &order_id,
                            &order_no,
                        )
                        .await?;
                        return Ok(ProcessPaymentCallbackOutcome::AlreadyCredited {
                            duplicate,
                            order_id,
                            order_no,
                            wallet_id: order_wallet_id,
                        });
                    }
                    if matches!(order_status.as_str(), "failed" | "expired" | "refunded") {
                        let error = format!("payment order is not creditable: {order_status}");
                        update_payment_callback_failure(tx, &callback_id, &input, &error).await?;
                        return Ok(ProcessPaymentCallbackOutcome::Failed { duplicate, error });
                    }
                    if order_status == "pending" {
                        let now = Utc::now().timestamp();
                        if expires_at_unix_secs.is_some_and(|value| value < now) {
                            sqlx::query(
                                "UPDATE payment_orders SET status = 'expired' WHERE id = $1",
                            )
                            .bind(&order_id)
                            .execute(&mut **tx)
                            .await
                            .map_postgres_err()?;
                            update_payment_callback_failure(
                                tx,
                                &callback_id,
                                &input,
                                "payment order expired",
                            )
                            .await?;
                            return Ok(ProcessPaymentCallbackOutcome::Failed {
                                duplicate,
                                error: "payment order expired".to_string(),
                            });
                        }
                    }

                    let Some(wallet_row) = sqlx::query(
                        r#"
SELECT
  id,
  status,
  CAST(balance AS DOUBLE PRECISION) AS balance,
  CAST(gift_balance AS DOUBLE PRECISION) AS gift_balance
FROM wallets
WHERE id = $1
LIMIT 1
FOR UPDATE
                        "#,
                    )
                    .bind(&order_wallet_id)
                    .fetch_optional(&mut **tx)
                    .await
                    .map_postgres_err()?
                    else {
                        update_payment_callback_failure(
                            tx,
                            &callback_id,
                            &input,
                            "wallet not found",
                        )
                        .await?;
                        return Ok(ProcessPaymentCallbackOutcome::Failed {
                            duplicate,
                            error: "wallet not found".to_string(),
                        });
                    };
                    let wallet_status: String = row_get(&wallet_row, "status")?;
                    if wallet_status != "active" {
                        update_payment_callback_failure(
                            tx,
                            &callback_id,
                            &input,
                            "wallet is not active",
                        )
                        .await?;
                        return Ok(ProcessPaymentCallbackOutcome::Failed {
                            duplicate,
                            error: "wallet is not active".to_string(),
                        });
                    }

                    let before_recharge: f64 = row_get(&wallet_row, "balance")?;
                    let before_gift: f64 = row_get(&wallet_row, "gift_balance")?;
                    let before_total = before_recharge + before_gift;
                    let after_recharge = before_recharge + order_amount_usd;
                    let after_total = after_recharge + before_gift;

                    sqlx::query(
                        r#"
UPDATE wallets
SET balance = $2,
    total_recharged = total_recharged + $3,
    updated_at = NOW()
WHERE id = $1
                        "#,
                    )
                    .bind(&order_wallet_id)
                    .bind(after_recharge)
                    .bind(order_amount_usd)
                    .execute(&mut **tx)
                    .await
                    .map_postgres_err()?;

                    sqlx::query(
                        r#"
INSERT INTO wallet_transactions (
  id,
  wallet_id,
  category,
  reason_code,
  amount,
  balance_before,
  balance_after,
  recharge_balance_before,
  recharge_balance_after,
  gift_balance_before,
  gift_balance_after,
  link_type,
  link_id,
  operator_id,
  description,
  created_at
)
VALUES (
  $1,
  $2,
  'recharge',
  'topup_gateway',
  $3,
  $4,
  $5,
  $6,
  $7,
  $8,
  $9,
  'payment_order',
  $10,
  NULL,
  $11,
  NOW()
)
                        "#,
                    )
                    .bind(Uuid::new_v4().to_string())
                    .bind(&order_wallet_id)
                    .bind(order_amount_usd)
                    .bind(before_total)
                    .bind(after_total)
                    .bind(before_recharge)
                    .bind(after_recharge)
                    .bind(before_gift)
                    .bind(before_gift)
                    .bind(&order_id)
                    .bind(format!("充值到账({})", input.payment_method))
                    .execute(&mut **tx)
                    .await
                    .map_postgres_err()?;

                    let updated_order_row = sqlx::query(
                        r#"
UPDATE payment_orders
SET gateway_order_id = COALESCE($2, gateway_order_id),
    gateway_response = $3,
    pay_amount = COALESCE($4, pay_amount),
    pay_currency = COALESCE($5, pay_currency),
    exchange_rate = COALESCE($6, exchange_rate),
    status = 'credited',
    paid_at = COALESCE(paid_at, NOW()),
    credited_at = NOW(),
    refundable_amount_usd = amount_usd
WHERE id = $1
RETURNING
  id,
  order_no,
  wallet_id,
  user_id,
  CAST(amount_usd AS DOUBLE PRECISION) AS amount_usd,
  CAST(pay_amount AS DOUBLE PRECISION) AS pay_amount,
  pay_currency,
  CAST(exchange_rate AS DOUBLE PRECISION) AS exchange_rate,
  CAST(refunded_amount_usd AS DOUBLE PRECISION) AS refunded_amount_usd,
  CAST(refundable_amount_usd AS DOUBLE PRECISION) AS refundable_amount_usd,
  payment_method,
  gateway_order_id,
  gateway_response,
  status,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM paid_at) AS BIGINT) AS paid_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM credited_at) AS BIGINT) AS credited_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM expires_at) AS BIGINT) AS expires_at_unix_secs
                        "#,
                    )
                    .bind(&order_id)
                    .bind(input.gateway_order_id.as_deref())
                    .bind(&input.payload)
                    .bind(input.pay_amount)
                    .bind(input.pay_currency.as_deref())
                    .bind(input.exchange_rate)
                    .fetch_one(&mut **tx)
                    .await
                    .map_postgres_err()?;
                    mark_payment_callback_processed(tx, &callback_id, &input, &order_id, &order_no)
                        .await?;
                    Ok(ProcessPaymentCallbackOutcome::Applied {
                        duplicate,
                        order_id,
                        order_no,
                        wallet_id: order_wallet_id,
                        order: map_admin_payment_order_row(&updated_order_row)?,
                    })
                })
            })
            .await
    }

    async fn adjust_wallet_balance(
        &self,
        input: AdjustWalletBalanceInput,
    ) -> Result<Option<(StoredWalletSnapshot, StoredAdminWalletTransaction)>, DataLayerError> {
        self.tx_runner
            .run_read_write(|tx| {
                Box::pin(async move {
                    let Some(row) = sqlx::query(
                        r#"
SELECT
  id,
  user_id,
  api_key_id,
  CAST(balance AS DOUBLE PRECISION) AS balance,
  CAST(gift_balance AS DOUBLE PRECISION) AS gift_balance,
  limit_mode,
  currency,
  status,
  CAST(total_recharged AS DOUBLE PRECISION) AS total_recharged,
  CAST(total_consumed AS DOUBLE PRECISION) AS total_consumed,
  CAST(total_refunded AS DOUBLE PRECISION) AS total_refunded,
  CAST(total_adjusted AS DOUBLE PRECISION) AS total_adjusted
FROM wallets
WHERE id = $1
FOR UPDATE
                        "#,
                    )
                    .bind(&input.wallet_id)
                    .fetch_optional(&mut **tx)
                    .await
                    .map_postgres_err()?
                    else {
                        return Ok(None);
                    };

                    let before_recharge: f64 = row_get(&row, "balance")?;
                    let before_gift: f64 = row_get(&row, "gift_balance")?;
                    let before_total = before_recharge + before_gift;
                    let mut after_recharge = before_recharge;
                    let mut after_gift = before_gift;

                    if input.amount_usd > 0.0 {
                        if input.balance_type.eq_ignore_ascii_case("gift") {
                            after_gift += input.amount_usd;
                        } else {
                            after_recharge += input.amount_usd;
                        }
                    } else {
                        let mut remaining = -input.amount_usd;
                        let consume_positive_bucket = |balance: &mut f64, to_consume: &mut f64| {
                            if *to_consume <= 0.0 {
                                return;
                            }
                            let available = (*balance).max(0.0);
                            let consumed = available.min(*to_consume);
                            *balance -= consumed;
                            *to_consume -= consumed;
                        };
                        if input.balance_type.eq_ignore_ascii_case("gift") {
                            consume_positive_bucket(&mut after_gift, &mut remaining);
                            consume_positive_bucket(&mut after_recharge, &mut remaining);
                        } else {
                            consume_positive_bucket(&mut after_recharge, &mut remaining);
                            consume_positive_bucket(&mut after_gift, &mut remaining);
                        }
                        if remaining > 0.0 {
                            after_recharge -= remaining;
                        }
                    }

                    let wallet_row = sqlx::query(
                        r#"
UPDATE wallets
SET
  balance = $2,
  gift_balance = $3,
  total_adjusted = total_adjusted + $4,
  updated_at = NOW()
WHERE id = $1
RETURNING
  id,
  user_id,
  api_key_id,
  CAST(balance AS DOUBLE PRECISION) AS balance,
  CAST(gift_balance AS DOUBLE PRECISION) AS gift_balance,
  limit_mode,
  currency,
  status,
  CAST(total_recharged AS DOUBLE PRECISION) AS total_recharged,
  CAST(total_consumed AS DOUBLE PRECISION) AS total_consumed,
  CAST(total_refunded AS DOUBLE PRECISION) AS total_refunded,
  CAST(total_adjusted AS DOUBLE PRECISION) AS total_adjusted,
  CAST(EXTRACT(EPOCH FROM updated_at) AS BIGINT) AS updated_at_unix_secs
                        "#,
                    )
                    .bind(&input.wallet_id)
                    .bind(after_recharge)
                    .bind(after_gift)
                    .bind(input.amount_usd)
                    .fetch_one(&mut **tx)
                    .await
                    .map_postgres_err()?;
                    let wallet = map_wallet_row(&wallet_row)?;

                    let transaction_id = Uuid::new_v4().to_string();
                    let created_at = Utc::now().timestamp().max(0) as u64;
                    let description = input
                        .description
                        .as_deref()
                        .filter(|value| !value.trim().is_empty())
                        .unwrap_or("管理员调账")
                        .to_string();
                    sqlx::query(
                        r#"
INSERT INTO wallet_transactions (
  id,
  wallet_id,
  category,
  reason_code,
  amount,
  balance_before,
  balance_after,
  recharge_balance_before,
  recharge_balance_after,
  gift_balance_before,
  gift_balance_after,
  link_type,
  link_id,
  operator_id,
  description,
  created_at
)
VALUES (
  $1,
  $2,
  'adjust',
  'adjust_admin',
  $3,
  $4,
  $5,
  $6,
  $7,
  $8,
  $9,
  'admin_action',
  $10,
  $11,
  $12,
  NOW()
)
                        "#,
                    )
                    .bind(&transaction_id)
                    .bind(&input.wallet_id)
                    .bind(input.amount_usd)
                    .bind(before_total)
                    .bind(after_recharge + after_gift)
                    .bind(before_recharge)
                    .bind(after_recharge)
                    .bind(before_gift)
                    .bind(after_gift)
                    .bind(&input.wallet_id)
                    .bind(input.operator_id.as_deref())
                    .bind(&description)
                    .execute(&mut **tx)
                    .await
                    .map_postgres_err()?;

                    Ok(Some((
                        wallet,
                        StoredAdminWalletTransaction {
                            id: transaction_id,
                            wallet_id: input.wallet_id,
                            category: "adjust".to_string(),
                            reason_code: "adjust_admin".to_string(),
                            amount: input.amount_usd,
                            balance_before: before_total,
                            balance_after: after_recharge + after_gift,
                            recharge_balance_before: before_recharge,
                            recharge_balance_after: after_recharge,
                            gift_balance_before: before_gift,
                            gift_balance_after: after_gift,
                            link_type: Some("admin_action".to_string()),
                            link_id: Some(row_get(&wallet_row, "id")?),
                            operator_id: input.operator_id,
                            operator_name: None,
                            operator_email: None,
                            description: Some(description),
                            created_at_unix_ms: Some(created_at),
                        },
                    )))
                })
            })
            .await
    }

    async fn create_manual_wallet_recharge(
        &self,
        input: CreateManualWalletRechargeInput,
    ) -> Result<Option<(StoredWalletSnapshot, StoredAdminPaymentOrder)>, DataLayerError> {
        self.tx_runner
            .run_read_write(|tx| {
                Box::pin(async move {
                    let Some(wallet_row) = sqlx::query(
                        r#"
SELECT
  id,
  user_id,
  api_key_id,
  CAST(balance AS DOUBLE PRECISION) AS balance,
  CAST(gift_balance AS DOUBLE PRECISION) AS gift_balance,
  limit_mode,
  currency,
  status,
  CAST(total_recharged AS DOUBLE PRECISION) AS total_recharged,
  CAST(total_consumed AS DOUBLE PRECISION) AS total_consumed,
  CAST(total_refunded AS DOUBLE PRECISION) AS total_refunded,
  CAST(total_adjusted AS DOUBLE PRECISION) AS total_adjusted
FROM wallets
WHERE id = $1
FOR UPDATE
                        "#,
                    )
                    .bind(&input.wallet_id)
                    .fetch_optional(&mut **tx)
                    .await
                    .map_postgres_err()?
                    else {
                        return Ok(None);
                    };

                    let before_recharge: f64 = row_get(&wallet_row, "balance")?;
                    let before_gift: f64 = row_get(&wallet_row, "gift_balance")?;
                    let user_id: Option<String> = row_get(&wallet_row, "user_id")?;
                    let gateway_response = serde_json::json!({
                        "source": "manual",
                        "operator_id": input.operator_id,
                        "description": input.description,
                    });

                    let order_id = Uuid::new_v4().to_string();
                    sqlx::query(
                        r#"
INSERT INTO payment_orders (
  id,
  order_no,
  wallet_id,
  user_id,
  amount_usd,
  refunded_amount_usd,
  refundable_amount_usd,
  payment_method,
  status,
  gateway_response,
  created_at,
  paid_at,
  credited_at
)
VALUES (
  $1,
  $2,
  $3,
  $4,
  $5,
  0,
  $5,
  $6,
  'credited',
  $7,
  NOW(),
  NOW(),
  NOW()
)
                        "#,
                    )
                    .bind(&order_id)
                    .bind(&input.order_no)
                    .bind(&input.wallet_id)
                    .bind(user_id.as_deref())
                    .bind(input.amount_usd)
                    .bind(&input.payment_method)
                    .bind(&gateway_response)
                    .execute(&mut **tx)
                    .await
                    .map_postgres_err()?;

                    let after_recharge = before_recharge + input.amount_usd;
                    let wallet_row = sqlx::query(
                        r#"
UPDATE wallets
SET
  balance = $2,
  total_recharged = total_recharged + $3,
  updated_at = NOW()
WHERE id = $1
RETURNING
  id,
  user_id,
  api_key_id,
  CAST(balance AS DOUBLE PRECISION) AS balance,
  CAST(gift_balance AS DOUBLE PRECISION) AS gift_balance,
  limit_mode,
  currency,
  status,
  CAST(total_recharged AS DOUBLE PRECISION) AS total_recharged,
  CAST(total_consumed AS DOUBLE PRECISION) AS total_consumed,
  CAST(total_refunded AS DOUBLE PRECISION) AS total_refunded,
  CAST(total_adjusted AS DOUBLE PRECISION) AS total_adjusted,
  CAST(EXTRACT(EPOCH FROM updated_at) AS BIGINT) AS updated_at_unix_secs
                        "#,
                    )
                    .bind(&input.wallet_id)
                    .bind(after_recharge)
                    .bind(input.amount_usd)
                    .fetch_one(&mut **tx)
                    .await
                    .map_postgres_err()?;
                    let wallet = map_wallet_row(&wallet_row)?;

                    let reason_code = if matches!(
                        input.payment_method.as_str(),
                        "card_code" | "gift_code" | "card_recharge"
                    ) {
                        "topup_card_code"
                    } else {
                        "topup_admin_manual"
                    };
                    sqlx::query(
                        r#"
INSERT INTO wallet_transactions (
  id,
  wallet_id,
  category,
  reason_code,
  amount,
  balance_before,
  balance_after,
  recharge_balance_before,
  recharge_balance_after,
  gift_balance_before,
  gift_balance_after,
  link_type,
  link_id,
  operator_id,
  description,
  created_at
)
VALUES (
  $1,
  $2,
  'recharge',
  $3,
  $4,
  $5,
  $6,
  $7,
  $8,
  $9,
  $9,
  'payment_order',
  $10,
  $11,
  $12,
  NOW()
)
                        "#,
                    )
                    .bind(Uuid::new_v4().to_string())
                    .bind(&input.wallet_id)
                    .bind(reason_code)
                    .bind(input.amount_usd)
                    .bind(before_recharge + before_gift)
                    .bind(after_recharge + before_gift)
                    .bind(before_recharge)
                    .bind(after_recharge)
                    .bind(before_gift)
                    .bind(&order_id)
                    .bind(input.operator_id.as_deref())
                    .bind(
                        input
                            .description
                            .as_deref()
                            .filter(|value| !value.trim().is_empty())
                            .unwrap_or("管理员手动充值"),
                    )
                    .execute(&mut **tx)
                    .await
                    .map_postgres_err()?;

                    let order_row = sqlx::query(
                        r#"
SELECT
  id,
  order_no,
  wallet_id,
  user_id,
  CAST(amount_usd AS DOUBLE PRECISION) AS amount_usd,
  CAST(pay_amount AS DOUBLE PRECISION) AS pay_amount,
  pay_currency,
  CAST(exchange_rate AS DOUBLE PRECISION) AS exchange_rate,
  CAST(refunded_amount_usd AS DOUBLE PRECISION) AS refunded_amount_usd,
  CAST(refundable_amount_usd AS DOUBLE PRECISION) AS refundable_amount_usd,
  payment_method,
  gateway_order_id,
  gateway_response,
  status,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM paid_at) AS BIGINT) AS paid_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM credited_at) AS BIGINT) AS credited_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM expires_at) AS BIGINT) AS expires_at_unix_secs
FROM payment_orders
WHERE id = $1
LIMIT 1
                        "#,
                    )
                    .bind(&order_id)
                    .fetch_one(&mut **tx)
                    .await
                    .map_postgres_err()?;
                    Ok(Some((wallet, map_admin_payment_order_row(&order_row)?)))
                })
            })
            .await
    }

    async fn process_admin_wallet_refund(
        &self,
        input: ProcessAdminWalletRefundInput,
    ) -> Result<
        WalletMutationOutcome<(
            StoredWalletSnapshot,
            StoredAdminWalletRefund,
            StoredAdminWalletTransaction,
        )>,
        DataLayerError,
    > {
        self.tx_runner
            .run_read_write(|tx| {
                Box::pin(async move {
                    let Some(refund_row) = sqlx::query(
                        r#"
SELECT
  id,
  refund_no,
  wallet_id,
  user_id,
  payment_order_id,
  source_type,
  source_id,
  refund_mode,
  CAST(amount_usd AS DOUBLE PRECISION) AS amount_usd,
  status,
  reason,
  failure_reason,
  gateway_refund_id,
  payout_method,
  payout_reference,
  payout_proof,
  requested_by,
  approved_by,
  processed_by,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM updated_at) AS BIGINT) AS updated_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM processed_at) AS BIGINT) AS processed_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM completed_at) AS BIGINT) AS completed_at_unix_secs
FROM refund_requests
WHERE id = $1 AND wallet_id = $2
FOR UPDATE
                        "#,
                    )
                    .bind(&input.refund_id)
                    .bind(&input.wallet_id)
                    .fetch_optional(&mut **tx)
                    .await
                    .map_postgres_err()?
                    else {
                        return Ok(WalletMutationOutcome::NotFound);
                    };
                    let refund = map_admin_wallet_refund_row(&refund_row)?;
                    if !matches!(refund.status.as_str(), "approved" | "pending_approval") {
                        return Ok(WalletMutationOutcome::Invalid(
                            "refund status is not approvable".to_string(),
                        ));
                    }

                    let Some(wallet_row) = sqlx::query(
                        r#"
SELECT
  id,
  user_id,
  api_key_id,
  CAST(balance AS DOUBLE PRECISION) AS balance,
  CAST(gift_balance AS DOUBLE PRECISION) AS gift_balance,
  limit_mode,
  currency,
  status,
  CAST(total_recharged AS DOUBLE PRECISION) AS total_recharged,
  CAST(total_consumed AS DOUBLE PRECISION) AS total_consumed,
  CAST(total_refunded AS DOUBLE PRECISION) AS total_refunded,
  CAST(total_adjusted AS DOUBLE PRECISION) AS total_adjusted,
  CAST(EXTRACT(EPOCH FROM updated_at) AS BIGINT) AS updated_at_unix_secs
FROM wallets
WHERE id = $1
FOR UPDATE
                        "#,
                    )
                    .bind(&input.wallet_id)
                    .fetch_optional(&mut **tx)
                    .await
                    .map_postgres_err()?
                    else {
                        return Ok(WalletMutationOutcome::Invalid(
                            "wallet not found".to_string(),
                        ));
                    };
                    let before_recharge: f64 = row_get(&wallet_row, "balance")?;
                    let before_gift: f64 = row_get(&wallet_row, "gift_balance")?;
                    let before_total = before_recharge + before_gift;
                    let amount_usd = refund.amount_usd;
                    let after_recharge = before_recharge - amount_usd;
                    if after_recharge < 0.0 {
                        return Ok(WalletMutationOutcome::Invalid(
                            "refund amount exceeds refundable recharge balance".to_string(),
                        ));
                    }

                    if let Some(payment_order_id) = refund.payment_order_id.as_deref() {
                        let Some(order_row) = sqlx::query(
                            r#"
SELECT
  id,
  CAST(refunded_amount_usd AS DOUBLE PRECISION) AS refunded_amount_usd,
  CAST(refundable_amount_usd AS DOUBLE PRECISION) AS refundable_amount_usd
FROM payment_orders
WHERE id = $1
FOR UPDATE
                            "#,
                        )
                        .bind(payment_order_id)
                        .fetch_optional(&mut **tx)
                        .await
                        .map_postgres_err()?
                        else {
                            return Ok(WalletMutationOutcome::Invalid(
                                "payment order not found".to_string(),
                            ));
                        };
                        let refundable_amount: f64 = row_get(&order_row, "refundable_amount_usd")?;
                        if amount_usd > refundable_amount {
                            return Ok(WalletMutationOutcome::Invalid(
                                "refund amount exceeds refundable amount".to_string(),
                            ));
                        }
                        sqlx::query(
                            r#"
UPDATE payment_orders
SET
  refunded_amount_usd = refunded_amount_usd + $2,
  refundable_amount_usd = refundable_amount_usd - $2
WHERE id = $1
                            "#,
                        )
                        .bind(payment_order_id)
                        .bind(amount_usd)
                        .execute(&mut **tx)
                        .await
                        .map_postgres_err()?;
                    }

                    let wallet_row = sqlx::query(
                        r#"
UPDATE wallets
SET
  balance = $2,
  total_refunded = total_refunded + $3,
  updated_at = NOW()
WHERE id = $1
RETURNING
  id,
  user_id,
  api_key_id,
  CAST(balance AS DOUBLE PRECISION) AS balance,
  CAST(gift_balance AS DOUBLE PRECISION) AS gift_balance,
  limit_mode,
  currency,
  status,
  CAST(total_recharged AS DOUBLE PRECISION) AS total_recharged,
  CAST(total_consumed AS DOUBLE PRECISION) AS total_consumed,
  CAST(total_refunded AS DOUBLE PRECISION) AS total_refunded,
  CAST(total_adjusted AS DOUBLE PRECISION) AS total_adjusted,
  CAST(EXTRACT(EPOCH FROM updated_at) AS BIGINT) AS updated_at_unix_secs
                        "#,
                    )
                    .bind(&input.wallet_id)
                    .bind(after_recharge)
                    .bind(amount_usd)
                    .fetch_one(&mut **tx)
                    .await
                    .map_postgres_err()?;
                    let wallet = map_wallet_row(&wallet_row)?;

                    let transaction_id = Uuid::new_v4().to_string();
                    let created_at_unix_ms = Utc::now().timestamp().max(0) as u64;
                    sqlx::query(
                        r#"
INSERT INTO wallet_transactions (
  id,
  wallet_id,
  category,
  reason_code,
  amount,
  balance_before,
  balance_after,
  recharge_balance_before,
  recharge_balance_after,
  gift_balance_before,
  gift_balance_after,
  link_type,
  link_id,
  operator_id,
  description,
  created_at
)
VALUES (
  $1,
  $2,
  'refund',
  'refund_out',
  $3,
  $4,
  $5,
  $6,
  $7,
  $8,
  $9,
  'refund_request',
  $10,
  $11,
  '退款占款',
  NOW()
)
                        "#,
                    )
                    .bind(&transaction_id)
                    .bind(&input.wallet_id)
                    .bind(-amount_usd)
                    .bind(before_total)
                    .bind(after_recharge + before_gift)
                    .bind(before_recharge)
                    .bind(after_recharge)
                    .bind(before_gift)
                    .bind(before_gift)
                    .bind(&input.refund_id)
                    .bind(input.operator_id.as_deref())
                    .execute(&mut **tx)
                    .await
                    .map_postgres_err()?;

                    let refund_row = sqlx::query(
                        r#"
UPDATE refund_requests
SET
  status = 'processing',
  approved_by = $3,
  processed_by = $3,
  processed_at = NOW(),
  updated_at = NOW()
WHERE id = $1 AND wallet_id = $2
RETURNING
  id,
  refund_no,
  wallet_id,
  user_id,
  payment_order_id,
  source_type,
  source_id,
  refund_mode,
  CAST(amount_usd AS DOUBLE PRECISION) AS amount_usd,
  status,
  reason,
  failure_reason,
  gateway_refund_id,
  payout_method,
  payout_reference,
  payout_proof,
  requested_by,
  approved_by,
  processed_by,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM updated_at) AS BIGINT) AS updated_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM processed_at) AS BIGINT) AS processed_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM completed_at) AS BIGINT) AS completed_at_unix_secs
                        "#,
                    )
                    .bind(&input.refund_id)
                    .bind(&input.wallet_id)
                    .bind(input.operator_id.as_deref())
                    .fetch_one(&mut **tx)
                    .await
                    .map_postgres_err()?;
                    Ok(WalletMutationOutcome::Applied((
                        wallet,
                        map_admin_wallet_refund_row(&refund_row)?,
                        StoredAdminWalletTransaction {
                            id: transaction_id,
                            wallet_id: input.wallet_id.clone(),
                            category: "refund".to_string(),
                            reason_code: "refund_out".to_string(),
                            amount: -amount_usd,
                            balance_before: before_total,
                            balance_after: after_recharge + before_gift,
                            recharge_balance_before: before_recharge,
                            recharge_balance_after: after_recharge,
                            gift_balance_before: before_gift,
                            gift_balance_after: before_gift,
                            link_type: Some("refund_request".to_string()),
                            link_id: Some(input.refund_id.clone()),
                            operator_id: input.operator_id.clone(),
                            operator_name: None,
                            operator_email: None,
                            description: Some("退款占款".to_string()),
                            created_at_unix_ms: Some(created_at_unix_ms),
                        },
                    )))
                })
            })
            .await
    }

    async fn complete_admin_wallet_refund(
        &self,
        input: CompleteAdminWalletRefundInput,
    ) -> Result<WalletMutationOutcome<StoredAdminWalletRefund>, DataLayerError> {
        self.tx_runner
            .run_read_write(|tx| {
                Box::pin(async move {
                    let Some(current_refund) = sqlx::query(
                        r#"
SELECT status
FROM refund_requests
WHERE id = $1 AND wallet_id = $2
FOR UPDATE
                        "#,
                    )
                    .bind(&input.refund_id)
                    .bind(&input.wallet_id)
                    .fetch_optional(&mut **tx)
                    .await
                    .map_postgres_err()?
                    else {
                        return Ok(WalletMutationOutcome::NotFound);
                    };
                    let status: String = row_get(&current_refund, "status")?;
                    if status != "processing" {
                        return Ok(WalletMutationOutcome::Invalid(
                            "refund status must be processing before completion".to_string(),
                        ));
                    }

                    let refund_row = sqlx::query(
                        r#"
UPDATE refund_requests
SET
  status = 'succeeded',
  gateway_refund_id = $3,
  payout_reference = $4,
  payout_proof = $5,
  completed_at = NOW(),
  updated_at = NOW()
WHERE id = $1 AND wallet_id = $2
RETURNING
  id,
  refund_no,
  wallet_id,
  user_id,
  payment_order_id,
  source_type,
  source_id,
  refund_mode,
  CAST(amount_usd AS DOUBLE PRECISION) AS amount_usd,
  status,
  reason,
  failure_reason,
  gateway_refund_id,
  payout_method,
  payout_reference,
  payout_proof,
  requested_by,
  approved_by,
  processed_by,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM updated_at) AS BIGINT) AS updated_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM processed_at) AS BIGINT) AS processed_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM completed_at) AS BIGINT) AS completed_at_unix_secs
                        "#,
                    )
                    .bind(&input.refund_id)
                    .bind(&input.wallet_id)
                    .bind(input.gateway_refund_id.as_deref())
                    .bind(input.payout_reference.as_deref())
                    .bind(&input.payout_proof)
                    .fetch_one(&mut **tx)
                    .await
                    .map_postgres_err()?;
                    Ok(WalletMutationOutcome::Applied(map_admin_wallet_refund_row(
                        &refund_row,
                    )?))
                })
            })
            .await
    }

    async fn fail_admin_wallet_refund(
        &self,
        input: FailAdminWalletRefundInput,
    ) -> Result<
        WalletMutationOutcome<(
            StoredWalletSnapshot,
            StoredAdminWalletRefund,
            Option<StoredAdminWalletTransaction>,
        )>,
        DataLayerError,
    > {
        self.tx_runner
            .run_read_write(|tx| {
                Box::pin(async move {
                    let Some(refund_row) = sqlx::query(
                        r#"
SELECT
  id,
  refund_no,
  wallet_id,
  user_id,
  payment_order_id,
  source_type,
  source_id,
  refund_mode,
  CAST(amount_usd AS DOUBLE PRECISION) AS amount_usd,
  status,
  reason,
  failure_reason,
  gateway_refund_id,
  payout_method,
  payout_reference,
  payout_proof,
  requested_by,
  approved_by,
  processed_by,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM updated_at) AS BIGINT) AS updated_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM processed_at) AS BIGINT) AS processed_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM completed_at) AS BIGINT) AS completed_at_unix_secs
FROM refund_requests
WHERE id = $1 AND wallet_id = $2
FOR UPDATE
                        "#,
                    )
                    .bind(&input.refund_id)
                    .bind(&input.wallet_id)
                    .fetch_optional(&mut **tx)
                    .await
                    .map_postgres_err()?
                    else {
                        return Ok(WalletMutationOutcome::NotFound);
                    };
                    let refund = map_admin_wallet_refund_row(&refund_row)?;

                    if matches!(refund.status.as_str(), "pending_approval" | "approved") {
                        let refund_row = sqlx::query(
                            r#"
UPDATE refund_requests
SET
  status = 'failed',
  failure_reason = $3,
  updated_at = NOW()
WHERE id = $1 AND wallet_id = $2
RETURNING
  id,
  refund_no,
  wallet_id,
  user_id,
  payment_order_id,
  source_type,
  source_id,
  refund_mode,
  CAST(amount_usd AS DOUBLE PRECISION) AS amount_usd,
  status,
  reason,
  failure_reason,
  gateway_refund_id,
  payout_method,
  payout_reference,
  payout_proof,
  requested_by,
  approved_by,
  processed_by,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM updated_at) AS BIGINT) AS updated_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM processed_at) AS BIGINT) AS processed_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM completed_at) AS BIGINT) AS completed_at_unix_secs
                            "#,
                        )
                        .bind(&input.refund_id)
                        .bind(&input.wallet_id)
                        .bind(&input.reason)
                        .fetch_one(&mut **tx)
                        .await
                        .map_postgres_err()?;
                        let wallet_row = sqlx::query(
                            r#"
SELECT
  id,
  user_id,
  api_key_id,
  CAST(balance AS DOUBLE PRECISION) AS balance,
  CAST(gift_balance AS DOUBLE PRECISION) AS gift_balance,
  limit_mode,
  currency,
  status,
  CAST(total_recharged AS DOUBLE PRECISION) AS total_recharged,
  CAST(total_consumed AS DOUBLE PRECISION) AS total_consumed,
  CAST(total_refunded AS DOUBLE PRECISION) AS total_refunded,
  CAST(total_adjusted AS DOUBLE PRECISION) AS total_adjusted,
  CAST(EXTRACT(EPOCH FROM updated_at) AS BIGINT) AS updated_at_unix_secs
FROM wallets
WHERE id = $1
                            "#,
                        )
                        .bind(&input.wallet_id)
                        .fetch_one(&mut **tx)
                        .await
                        .map_postgres_err()?;
                        return Ok(WalletMutationOutcome::Applied((
                            map_wallet_row(&wallet_row)?,
                            map_admin_wallet_refund_row(&refund_row)?,
                            None,
                        )));
                    }

                    if refund.status != "processing" {
                        return Ok(WalletMutationOutcome::Invalid(format!(
                            "cannot fail refund in status: {}",
                            refund.status
                        )));
                    }

                    let Some(wallet_row) = sqlx::query(
                        r#"
SELECT
  id,
  user_id,
  api_key_id,
  CAST(balance AS DOUBLE PRECISION) AS balance,
  CAST(gift_balance AS DOUBLE PRECISION) AS gift_balance,
  limit_mode,
  currency,
  status,
  CAST(total_recharged AS DOUBLE PRECISION) AS total_recharged,
  CAST(total_consumed AS DOUBLE PRECISION) AS total_consumed,
  CAST(total_refunded AS DOUBLE PRECISION) AS total_refunded,
  CAST(total_adjusted AS DOUBLE PRECISION) AS total_adjusted,
  CAST(EXTRACT(EPOCH FROM updated_at) AS BIGINT) AS updated_at_unix_secs
FROM wallets
WHERE id = $1
FOR UPDATE
                        "#,
                    )
                    .bind(&input.wallet_id)
                    .fetch_optional(&mut **tx)
                    .await
                    .map_postgres_err()?
                    else {
                        return Ok(WalletMutationOutcome::Invalid(
                            "wallet not found".to_string(),
                        ));
                    };
                    let amount_usd = refund.amount_usd;
                    let before_recharge: f64 = row_get(&wallet_row, "balance")?;
                    let before_gift: f64 = row_get(&wallet_row, "gift_balance")?;
                    let before_total = before_recharge + before_gift;
                    let after_recharge = before_recharge + amount_usd;

                    let wallet_row = sqlx::query(
                        r#"
UPDATE wallets
SET
  balance = $2,
  total_refunded = GREATEST(total_refunded - $3, 0),
  updated_at = NOW()
WHERE id = $1
RETURNING
  id,
  user_id,
  api_key_id,
  CAST(balance AS DOUBLE PRECISION) AS balance,
  CAST(gift_balance AS DOUBLE PRECISION) AS gift_balance,
  limit_mode,
  currency,
  status,
  CAST(total_recharged AS DOUBLE PRECISION) AS total_recharged,
  CAST(total_consumed AS DOUBLE PRECISION) AS total_consumed,
  CAST(total_refunded AS DOUBLE PRECISION) AS total_refunded,
  CAST(total_adjusted AS DOUBLE PRECISION) AS total_adjusted,
  CAST(EXTRACT(EPOCH FROM updated_at) AS BIGINT) AS updated_at_unix_secs
                        "#,
                    )
                    .bind(&input.wallet_id)
                    .bind(after_recharge)
                    .bind(amount_usd)
                    .fetch_one(&mut **tx)
                    .await
                    .map_postgres_err()?;
                    let wallet = map_wallet_row(&wallet_row)?;

                    let transaction_id = Uuid::new_v4().to_string();
                    let created_at_unix_ms = Utc::now().timestamp().max(0) as u64;
                    sqlx::query(
                        r#"
INSERT INTO wallet_transactions (
  id,
  wallet_id,
  category,
  reason_code,
  amount,
  balance_before,
  balance_after,
  recharge_balance_before,
  recharge_balance_after,
  gift_balance_before,
  gift_balance_after,
  link_type,
  link_id,
  operator_id,
  description,
  created_at
)
VALUES (
  $1,
  $2,
  'refund',
  'refund_revert',
  $3,
  $4,
  $5,
  $6,
  $7,
  $8,
  $9,
  'refund_request',
  $10,
  $11,
  '退款失败回补',
  NOW()
)
                        "#,
                    )
                    .bind(&transaction_id)
                    .bind(&input.wallet_id)
                    .bind(amount_usd)
                    .bind(before_total)
                    .bind(after_recharge + before_gift)
                    .bind(before_recharge)
                    .bind(after_recharge)
                    .bind(before_gift)
                    .bind(before_gift)
                    .bind(&input.refund_id)
                    .bind(input.operator_id.as_deref())
                    .execute(&mut **tx)
                    .await
                    .map_postgres_err()?;

                    if let Some(payment_order_id) = refund.payment_order_id.as_deref() {
                        let _ = sqlx::query(
                            r#"
UPDATE payment_orders
SET
  refunded_amount_usd = refunded_amount_usd - $2,
  refundable_amount_usd = refundable_amount_usd + $2
WHERE id = $1
                            "#,
                        )
                        .bind(payment_order_id)
                        .bind(amount_usd)
                        .execute(&mut **tx)
                        .await
                        .map_postgres_err()?;
                    }

                    let refund_row = sqlx::query(
                        r#"
UPDATE refund_requests
SET
  status = 'failed',
  failure_reason = $3,
  updated_at = NOW()
WHERE id = $1 AND wallet_id = $2
RETURNING
  id,
  refund_no,
  wallet_id,
  user_id,
  payment_order_id,
  source_type,
  source_id,
  refund_mode,
  CAST(amount_usd AS DOUBLE PRECISION) AS amount_usd,
  status,
  reason,
  failure_reason,
  gateway_refund_id,
  payout_method,
  payout_reference,
  payout_proof,
  requested_by,
  approved_by,
  processed_by,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM updated_at) AS BIGINT) AS updated_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM processed_at) AS BIGINT) AS processed_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM completed_at) AS BIGINT) AS completed_at_unix_secs
                        "#,
                    )
                    .bind(&input.refund_id)
                    .bind(&input.wallet_id)
                    .bind(&input.reason)
                    .fetch_one(&mut **tx)
                    .await
                    .map_postgres_err()?;
                    Ok(WalletMutationOutcome::Applied((
                        wallet,
                        map_admin_wallet_refund_row(&refund_row)?,
                        Some(StoredAdminWalletTransaction {
                            id: transaction_id,
                            wallet_id: input.wallet_id.clone(),
                            category: "refund".to_string(),
                            reason_code: "refund_revert".to_string(),
                            amount: amount_usd,
                            balance_before: before_total,
                            balance_after: after_recharge + before_gift,
                            recharge_balance_before: before_recharge,
                            recharge_balance_after: after_recharge,
                            gift_balance_before: before_gift,
                            gift_balance_after: before_gift,
                            link_type: Some("refund_request".to_string()),
                            link_id: Some(input.refund_id.clone()),
                            operator_id: input.operator_id.clone(),
                            operator_name: None,
                            operator_email: None,
                            description: Some("退款失败回补".to_string()),
                            created_at_unix_ms: Some(created_at_unix_ms),
                        }),
                    )))
                })
            })
            .await
    }

    async fn expire_admin_payment_order(
        &self,
        order_id: &str,
    ) -> Result<WalletMutationOutcome<(StoredAdminPaymentOrder, bool)>, DataLayerError> {
        let order_id = order_id.to_string();
        self.tx_runner
            .run_read_write(|tx| {
                Box::pin(async move {
                    let Some(row) = sqlx::query(
                        r#"
SELECT
  id,
  order_no,
  wallet_id,
  user_id,
  CAST(amount_usd AS DOUBLE PRECISION) AS amount_usd,
  CAST(pay_amount AS DOUBLE PRECISION) AS pay_amount,
  pay_currency,
  CAST(exchange_rate AS DOUBLE PRECISION) AS exchange_rate,
  CAST(refunded_amount_usd AS DOUBLE PRECISION) AS refunded_amount_usd,
  CAST(refundable_amount_usd AS DOUBLE PRECISION) AS refundable_amount_usd,
  payment_method,
  gateway_order_id,
  gateway_response,
  status,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM paid_at) AS BIGINT) AS paid_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM credited_at) AS BIGINT) AS credited_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM expires_at) AS BIGINT) AS expires_at_unix_secs
FROM payment_orders
WHERE id = $1
FOR UPDATE
                        "#,
                    )
                    .bind(&order_id)
                    .fetch_optional(&mut **tx)
                    .await
                    .map_postgres_err()?
                    else {
                        return Ok(WalletMutationOutcome::NotFound);
                    };
                    let order = map_admin_payment_order_row(&row)?;
                    if order.status == "credited" {
                        return Ok(WalletMutationOutcome::Invalid(
                            "credited order cannot be expired".to_string(),
                        ));
                    }
                    if order.status == "expired" {
                        return Ok(WalletMutationOutcome::Applied((order, false)));
                    }
                    if order.status != "pending" {
                        return Ok(WalletMutationOutcome::Invalid(format!(
                            "only pending order can be expired: {}",
                            order.status
                        )));
                    }
                    let mut gateway_response =
                        payment_gateway_response_map(order.gateway_response.clone());
                    gateway_response.insert(
                        "expire_reason".to_string(),
                        serde_json::Value::String("admin_mark_expired".to_string()),
                    );
                    gateway_response.insert(
                        "expired_at".to_string(),
                        serde_json::Value::String(Utc::now().to_rfc3339()),
                    );
                    let row = sqlx::query(
                        r#"
UPDATE payment_orders
SET
  status = 'expired',
  gateway_response = $2
WHERE id = $1
RETURNING
  id,
  order_no,
  wallet_id,
  user_id,
  CAST(amount_usd AS DOUBLE PRECISION) AS amount_usd,
  CAST(pay_amount AS DOUBLE PRECISION) AS pay_amount,
  pay_currency,
  CAST(exchange_rate AS DOUBLE PRECISION) AS exchange_rate,
  CAST(refunded_amount_usd AS DOUBLE PRECISION) AS refunded_amount_usd,
  CAST(refundable_amount_usd AS DOUBLE PRECISION) AS refundable_amount_usd,
  payment_method,
  gateway_order_id,
  gateway_response,
  status,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM paid_at) AS BIGINT) AS paid_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM credited_at) AS BIGINT) AS credited_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM expires_at) AS BIGINT) AS expires_at_unix_secs
                        "#,
                    )
                    .bind(&order_id)
                    .bind(serde_json::Value::Object(gateway_response))
                    .fetch_one(&mut **tx)
                    .await
                    .map_postgres_err()?;
                    Ok(WalletMutationOutcome::Applied((
                        map_admin_payment_order_row(&row)?,
                        true,
                    )))
                })
            })
            .await
    }

    async fn fail_admin_payment_order(
        &self,
        order_id: &str,
    ) -> Result<WalletMutationOutcome<StoredAdminPaymentOrder>, DataLayerError> {
        let order_id = order_id.to_string();
        self.tx_runner
            .run_read_write(|tx| {
                Box::pin(async move {
                    let Some(row) = sqlx::query(
                        r#"
SELECT
  id,
  order_no,
  wallet_id,
  user_id,
  CAST(amount_usd AS DOUBLE PRECISION) AS amount_usd,
  CAST(pay_amount AS DOUBLE PRECISION) AS pay_amount,
  pay_currency,
  CAST(exchange_rate AS DOUBLE PRECISION) AS exchange_rate,
  CAST(refunded_amount_usd AS DOUBLE PRECISION) AS refunded_amount_usd,
  CAST(refundable_amount_usd AS DOUBLE PRECISION) AS refundable_amount_usd,
  payment_method,
  gateway_order_id,
  gateway_response,
  status,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM paid_at) AS BIGINT) AS paid_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM credited_at) AS BIGINT) AS credited_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM expires_at) AS BIGINT) AS expires_at_unix_secs
FROM payment_orders
WHERE id = $1
FOR UPDATE
                        "#,
                    )
                    .bind(&order_id)
                    .fetch_optional(&mut **tx)
                    .await
                    .map_postgres_err()?
                    else {
                        return Ok(WalletMutationOutcome::NotFound);
                    };
                    let order = map_admin_payment_order_row(&row)?;
                    if order.status == "credited" {
                        return Ok(WalletMutationOutcome::Invalid(
                            "credited order cannot be failed".to_string(),
                        ));
                    }
                    let mut gateway_response =
                        payment_gateway_response_map(order.gateway_response.clone());
                    gateway_response.insert(
                        "failure_reason".to_string(),
                        serde_json::Value::String("admin_mark_failed".to_string()),
                    );
                    gateway_response.insert(
                        "failed_at".to_string(),
                        serde_json::Value::String(Utc::now().to_rfc3339()),
                    );
                    let row = sqlx::query(
                        r#"
UPDATE payment_orders
SET
  status = 'failed',
  gateway_response = $2
WHERE id = $1
RETURNING
  id,
  order_no,
  wallet_id,
  user_id,
  CAST(amount_usd AS DOUBLE PRECISION) AS amount_usd,
  CAST(pay_amount AS DOUBLE PRECISION) AS pay_amount,
  pay_currency,
  CAST(exchange_rate AS DOUBLE PRECISION) AS exchange_rate,
  CAST(refunded_amount_usd AS DOUBLE PRECISION) AS refunded_amount_usd,
  CAST(refundable_amount_usd AS DOUBLE PRECISION) AS refundable_amount_usd,
  payment_method,
  gateway_order_id,
  gateway_response,
  status,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM paid_at) AS BIGINT) AS paid_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM credited_at) AS BIGINT) AS credited_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM expires_at) AS BIGINT) AS expires_at_unix_secs
                        "#,
                    )
                    .bind(&order_id)
                    .bind(serde_json::Value::Object(gateway_response))
                    .fetch_one(&mut **tx)
                    .await
                    .map_postgres_err()?;
                    Ok(WalletMutationOutcome::Applied(map_admin_payment_order_row(
                        &row,
                    )?))
                })
            })
            .await
    }

    async fn credit_admin_payment_order(
        &self,
        input: CreditAdminPaymentOrderInput,
    ) -> Result<WalletMutationOutcome<(StoredAdminPaymentOrder, bool)>, DataLayerError> {
        self.tx_runner
            .run_read_write(|tx| {
                Box::pin(async move {
                    let Some(order_row) = sqlx::query(
                        r#"
SELECT
  id,
  order_no,
  wallet_id,
  user_id,
  CAST(amount_usd AS DOUBLE PRECISION) AS amount_usd,
  CAST(pay_amount AS DOUBLE PRECISION) AS pay_amount,
  pay_currency,
  CAST(exchange_rate AS DOUBLE PRECISION) AS exchange_rate,
  CAST(refunded_amount_usd AS DOUBLE PRECISION) AS refunded_amount_usd,
  CAST(refundable_amount_usd AS DOUBLE PRECISION) AS refundable_amount_usd,
  payment_method,
  gateway_order_id,
  gateway_response,
  status,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM paid_at) AS BIGINT) AS paid_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM credited_at) AS BIGINT) AS credited_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM expires_at) AS BIGINT) AS expires_at_unix_secs
FROM payment_orders
WHERE id = $1
FOR UPDATE
                        "#,
                    )
                    .bind(&input.order_id)
                    .fetch_optional(&mut **tx)
                    .await
                    .map_postgres_err()?
                    else {
                        return Ok(WalletMutationOutcome::NotFound);
                    };
                    let order = map_admin_payment_order_row(&order_row)?;
                    if order.status == "credited" {
                        return Ok(WalletMutationOutcome::Applied((order, false)));
                    }
                    if matches!(order.status.as_str(), "failed" | "expired" | "refunded") {
                        return Ok(WalletMutationOutcome::Invalid(format!(
                            "payment order is not creditable: {}",
                            order.status
                        )));
                    }
                    if order
                        .expires_at_unix_secs
                        .is_some_and(|value| value < Utc::now().timestamp().max(0) as u64)
                    {
                        return Ok(WalletMutationOutcome::Invalid(
                            "payment order expired".to_string(),
                        ));
                    }

                    let Some(wallet_row) = sqlx::query(
                        r#"
SELECT
  id,
  status,
  CAST(balance AS DOUBLE PRECISION) AS balance,
  CAST(gift_balance AS DOUBLE PRECISION) AS gift_balance
FROM wallets
WHERE id = $1
FOR UPDATE
                        "#,
                    )
                    .bind(&order.wallet_id)
                    .fetch_optional(&mut **tx)
                    .await
                    .map_postgres_err()?
                    else {
                        return Ok(WalletMutationOutcome::Invalid(
                            "wallet not found".to_string(),
                        ));
                    };
                    let wallet_status: String = row_get(&wallet_row, "status")?;
                    if wallet_status != "active" {
                        return Ok(WalletMutationOutcome::Invalid(
                            "wallet is not active".to_string(),
                        ));
                    }

                    let before_recharge: f64 = row_get(&wallet_row, "balance")?;
                    let before_gift: f64 = row_get(&wallet_row, "gift_balance")?;
                    let before_total = before_recharge + before_gift;
                    let after_recharge = before_recharge + order.amount_usd;
                    let now_unix_secs = Utc::now().timestamp().max(0) as u64;
                    sqlx::query(
                        r#"
UPDATE wallets
SET
  balance = $2,
  total_recharged = total_recharged + $3,
  updated_at = NOW()
WHERE id = $1
                        "#,
                    )
                    .bind(&order.wallet_id)
                    .bind(after_recharge)
                    .bind(order.amount_usd)
                    .execute(&mut **tx)
                    .await
                    .map_postgres_err()?;

                    sqlx::query(
                        r#"
INSERT INTO wallet_transactions (
  id,
  wallet_id,
  category,
  reason_code,
  amount,
  balance_before,
  balance_after,
  recharge_balance_before,
  recharge_balance_after,
  gift_balance_before,
  gift_balance_after,
  link_type,
  link_id,
  operator_id,
  description,
  created_at
)
VALUES (
  $1,
  $2,
  'recharge',
  'topup_gateway',
  $3,
  $4,
  $5,
  $6,
  $7,
  $8,
  $8,
  'payment_order',
  $9,
  NULL,
  $10,
  NOW()
)
                        "#,
                    )
                    .bind(Uuid::new_v4().to_string())
                    .bind(&order.wallet_id)
                    .bind(order.amount_usd)
                    .bind(before_total)
                    .bind(after_recharge + before_gift)
                    .bind(before_recharge)
                    .bind(after_recharge)
                    .bind(before_gift)
                    .bind(&input.order_id)
                    .bind(format!("充值到账({})", order.payment_method))
                    .execute(&mut **tx)
                    .await
                    .map_postgres_err()?;

                    let mut gateway_response =
                        payment_gateway_response_map(order.gateway_response.clone());
                    if let Some(serde_json::Value::Object(map)) = input.gateway_response_patch {
                        gateway_response.extend(map);
                    }
                    gateway_response
                        .insert("manual_credit".to_string(), serde_json::Value::Bool(true));
                    gateway_response.insert(
                        "credited_by".to_string(),
                        input
                            .operator_id
                            .clone()
                            .map(serde_json::Value::String)
                            .unwrap_or(serde_json::Value::Null),
                    );
                    let next_gateway_order_id = input
                        .gateway_order_id
                        .clone()
                        .or(order.gateway_order_id.clone());
                    let next_pay_amount = input.pay_amount.or(order.pay_amount);
                    let next_pay_currency =
                        input.pay_currency.clone().or(order.pay_currency.clone());
                    let next_exchange_rate = input.exchange_rate.or(order.exchange_rate);
                    let next_paid_at_unix_secs = order.paid_at_unix_secs.or(Some(now_unix_secs));

                    let row = sqlx::query(
                        r#"
UPDATE payment_orders
SET
  gateway_order_id = $2,
  gateway_response = $3,
  pay_amount = $4,
  pay_currency = $5,
  exchange_rate = $6,
  status = 'credited',
  paid_at = COALESCE(to_timestamp($7), NOW()),
  credited_at = NOW(),
  refundable_amount_usd = amount_usd
WHERE id = $1
RETURNING
  id,
  order_no,
  wallet_id,
  user_id,
  CAST(amount_usd AS DOUBLE PRECISION) AS amount_usd,
  CAST(pay_amount AS DOUBLE PRECISION) AS pay_amount,
  pay_currency,
  CAST(exchange_rate AS DOUBLE PRECISION) AS exchange_rate,
  CAST(refunded_amount_usd AS DOUBLE PRECISION) AS refunded_amount_usd,
  CAST(refundable_amount_usd AS DOUBLE PRECISION) AS refundable_amount_usd,
  payment_method,
  gateway_order_id,
  gateway_response,
  status,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM paid_at) AS BIGINT) AS paid_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM credited_at) AS BIGINT) AS credited_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM expires_at) AS BIGINT) AS expires_at_unix_secs
                        "#,
                    )
                    .bind(&input.order_id)
                    .bind(next_gateway_order_id)
                    .bind(serde_json::Value::Object(gateway_response))
                    .bind(next_pay_amount)
                    .bind(next_pay_currency)
                    .bind(next_exchange_rate)
                    .bind(
                        i64::try_from(next_paid_at_unix_secs.unwrap_or(now_unix_secs))
                            .unwrap_or_default(),
                    )
                    .fetch_one(&mut **tx)
                    .await
                    .map_postgres_err()?;
                    Ok(WalletMutationOutcome::Applied((
                        map_admin_payment_order_row(&row)?,
                        true,
                    )))
                })
            })
            .await
    }

    async fn create_admin_redeem_code_batch(
        &self,
        input: CreateAdminRedeemCodeBatchInput,
    ) -> Result<CreateAdminRedeemCodeBatchResult, DataLayerError> {
        self.tx_runner
            .run_read_write(|tx| {
                Box::pin(async move {
                    let batch_id = Uuid::new_v4().to_string();
                    let expires_at = input
                        .expires_at_unix_secs
                        .map(|value| {
                            i64::try_from(value).map_err(|_| {
                                DataLayerError::InvalidInput(
                                    "redeem code batch expires_at overflow".to_string(),
                                )
                            })
                        })
                        .transpose()?;

                    sqlx::query(
                        r#"
INSERT INTO redeem_code_batches (
  id,
  name,
  amount_usd,
  currency,
  balance_bucket,
  total_count,
  status,
  description,
  created_by,
  expires_at,
  created_at,
  updated_at
)
VALUES (
  $1,
  $2,
  $3,
  $4,
  $5,
  $6,
  'active',
  $7,
  $8,
  CASE
    WHEN $9 IS NULL THEN NULL
    ELSE to_timestamp($9)
  END,
  NOW(),
  NOW()
)
                        "#,
                    )
                    .bind(&batch_id)
                    .bind(&input.name)
                    .bind(input.amount_usd)
                    .bind(&input.currency)
                    .bind(&input.balance_bucket)
                    .bind(i32::try_from(input.total_count).map_err(|_| {
                        DataLayerError::InvalidInput(
                            "redeem code batch total_count overflow".to_string(),
                        )
                    })?)
                    .bind(input.description.as_deref())
                    .bind(input.created_by.as_deref())
                    .bind(expires_at)
                    .execute(&mut **tx)
                    .await
                    .map_postgres_err()?;

                    let mut plaintext_codes = Vec::with_capacity(input.total_count);
                    for _ in 0..input.total_count {
                        let (normalized_code, display_code) = loop {
                            let normalized = generate_redeem_code_normalized();
                            let code_hash = hash_redeem_code(&normalized);
                            let insert_result = sqlx::query(
                                r#"
INSERT INTO redeem_codes (
  id,
  batch_id,
  code_hash,
  code_prefix,
  code_suffix,
  status,
  created_at,
  updated_at
)
VALUES (
  $1,
  $2,
  $3,
  $4,
  $5,
  'active',
  NOW(),
  NOW()
)
                                "#,
                            )
                            .bind(Uuid::new_v4().to_string())
                            .bind(&batch_id)
                            .bind(&code_hash)
                            .bind(redeem_code_prefix(&normalized))
                            .bind(redeem_code_suffix(&normalized))
                            .execute(&mut **tx)
                            .await;
                            match insert_result {
                                Ok(_) => {
                                    break (normalized.clone(), format_redeem_code(&normalized))
                                }
                                Err(sqlx::Error::Database(err))
                                    if err.code().as_deref() == Some("23505") =>
                                {
                                    continue;
                                }
                                Err(err) => return Err(DataLayerError::postgres(err)),
                            }
                        };
                        let code_hash = hash_redeem_code(&normalized_code);
                        let row = sqlx::query(
                            r#"
SELECT
  codes.id,
  codes.batch_id,
  batches.name AS batch_name,
  codes.code_prefix,
  codes.code_suffix,
  codes.status,
  codes.redeemed_by_user_id,
  NULL::TEXT AS redeemed_by_user_name,
  codes.redeemed_wallet_id,
  codes.redeemed_payment_order_id,
  NULL::TEXT AS redeemed_order_no,
  CAST(EXTRACT(EPOCH FROM codes.redeemed_at) AS BIGINT) AS redeemed_at_unix_secs,
  codes.disabled_by,
  CAST(EXTRACT(EPOCH FROM batches.expires_at) AS BIGINT) AS expires_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM codes.created_at) AS BIGINT) AS created_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM codes.updated_at) AS BIGINT) AS updated_at_unix_secs
FROM redeem_codes AS codes
JOIN redeem_code_batches AS batches
  ON batches.id = codes.batch_id
WHERE codes.code_hash = $1
LIMIT 1
                            "#,
                        )
                        .bind(&code_hash)
                        .fetch_one(&mut **tx)
                        .await
                        .map_postgres_err()?;
                        let code = map_admin_redeem_code_row(&row)?;
                        plaintext_codes.push(CreatedAdminRedeemCodePlaintext {
                            code_id: code.id,
                            code: display_code,
                            masked_code: code.masked_code,
                        });
                    }

                    let batch_row = sqlx::query(
                        r#"
SELECT
  batches.id,
  batches.name,
  CAST(batches.amount_usd AS DOUBLE PRECISION) AS amount_usd,
  batches.currency,
  batches.balance_bucket,
  CAST(batches.total_count AS BIGINT) AS total_count,
  CAST(COALESCE(stats.redeemed_count, 0) AS BIGINT) AS redeemed_count,
  CAST(COALESCE(stats.active_count, 0) AS BIGINT) AS active_count,
  batches.status,
  batches.description,
  batches.created_by,
  CAST(EXTRACT(EPOCH FROM batches.expires_at) AS BIGINT) AS expires_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM batches.created_at) AS BIGINT) AS created_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM batches.updated_at) AS BIGINT) AS updated_at_unix_secs
FROM redeem_code_batches AS batches
LEFT JOIN (
  SELECT
    batch_id,
    COUNT(*) FILTER (WHERE status = 'redeemed') AS redeemed_count,
    COUNT(*) FILTER (WHERE status = 'active') AS active_count
  FROM redeem_codes
  GROUP BY batch_id
) AS stats
  ON stats.batch_id = batches.id
WHERE batches.id = $1
LIMIT 1
                        "#,
                    )
                    .bind(&batch_id)
                    .fetch_one(&mut **tx)
                    .await
                    .map_postgres_err()?;
                    Ok(CreateAdminRedeemCodeBatchResult {
                        batch: map_admin_redeem_code_batch_row(&batch_row)?,
                        codes: plaintext_codes,
                    })
                })
            })
            .await
    }

    async fn disable_admin_redeem_code_batch(
        &self,
        input: DisableAdminRedeemCodeBatchInput,
    ) -> Result<WalletMutationOutcome<StoredAdminRedeemCodeBatch>, DataLayerError> {
        self.tx_runner
            .run_read_write(|tx| {
                Box::pin(async move {
                    let Some(current_batch) = sqlx::query(
                        r#"
SELECT status
FROM redeem_code_batches
WHERE id = $1
FOR UPDATE
                        "#,
                    )
                    .bind(&input.batch_id)
                    .fetch_optional(&mut **tx)
                    .await
                    .map_postgres_err()?
                    else {
                        return Ok(WalletMutationOutcome::NotFound);
                    };
                    let status: String = row_get(&current_batch, "status")?;
                    if status == "disabled" {
                        let row = sqlx::query(
                            r#"
SELECT
  batches.id,
  batches.name,
  CAST(batches.amount_usd AS DOUBLE PRECISION) AS amount_usd,
  batches.currency,
  batches.balance_bucket,
  CAST(batches.total_count AS BIGINT) AS total_count,
  CAST(COALESCE(stats.redeemed_count, 0) AS BIGINT) AS redeemed_count,
  CAST(COALESCE(stats.active_count, 0) AS BIGINT) AS active_count,
  batches.status,
  batches.description,
  batches.created_by,
  CAST(EXTRACT(EPOCH FROM batches.expires_at) AS BIGINT) AS expires_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM batches.created_at) AS BIGINT) AS created_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM batches.updated_at) AS BIGINT) AS updated_at_unix_secs
FROM redeem_code_batches AS batches
LEFT JOIN (
  SELECT
    batch_id,
    COUNT(*) FILTER (WHERE status = 'redeemed') AS redeemed_count,
    COUNT(*) FILTER (WHERE status = 'active') AS active_count
  FROM redeem_codes
  GROUP BY batch_id
) AS stats
  ON stats.batch_id = batches.id
WHERE batches.id = $1
LIMIT 1
                            "#,
                        )
                        .bind(&input.batch_id)
                        .fetch_one(&mut **tx)
                        .await
                        .map_postgres_err()?;
                        return Ok(WalletMutationOutcome::Applied(
                            map_admin_redeem_code_batch_row(&row)?,
                        ));
                    }

                    sqlx::query(
                        r#"
UPDATE redeem_code_batches
SET
  status = 'disabled',
  updated_at = NOW()
WHERE id = $1
                        "#,
                    )
                    .bind(&input.batch_id)
                    .execute(&mut **tx)
                    .await
                    .map_postgres_err()?;

                    sqlx::query(
                        r#"
UPDATE redeem_codes
SET
  status = 'disabled',
  disabled_by = COALESCE($2, disabled_by),
  updated_at = NOW()
WHERE batch_id = $1
  AND status = 'active'
                        "#,
                    )
                    .bind(&input.batch_id)
                    .bind(input.operator_id.as_deref())
                    .execute(&mut **tx)
                    .await
                    .map_postgres_err()?;

                    let row = sqlx::query(
                        r#"
SELECT
  batches.id,
  batches.name,
  CAST(batches.amount_usd AS DOUBLE PRECISION) AS amount_usd,
  batches.currency,
  batches.balance_bucket,
  CAST(batches.total_count AS BIGINT) AS total_count,
  CAST(COALESCE(stats.redeemed_count, 0) AS BIGINT) AS redeemed_count,
  CAST(COALESCE(stats.active_count, 0) AS BIGINT) AS active_count,
  batches.status,
  batches.description,
  batches.created_by,
  CAST(EXTRACT(EPOCH FROM batches.expires_at) AS BIGINT) AS expires_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM batches.created_at) AS BIGINT) AS created_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM batches.updated_at) AS BIGINT) AS updated_at_unix_secs
FROM redeem_code_batches AS batches
LEFT JOIN (
  SELECT
    batch_id,
    COUNT(*) FILTER (WHERE status = 'redeemed') AS redeemed_count,
    COUNT(*) FILTER (WHERE status = 'active') AS active_count
  FROM redeem_codes
  GROUP BY batch_id
) AS stats
  ON stats.batch_id = batches.id
WHERE batches.id = $1
LIMIT 1
                        "#,
                    )
                    .bind(&input.batch_id)
                    .fetch_one(&mut **tx)
                    .await
                    .map_postgres_err()?;
                    Ok(WalletMutationOutcome::Applied(
                        map_admin_redeem_code_batch_row(&row)?,
                    ))
                })
            })
            .await
    }

    async fn delete_admin_redeem_code_batch(
        &self,
        input: DeleteAdminRedeemCodeBatchInput,
    ) -> Result<WalletMutationOutcome<StoredAdminRedeemCodeBatch>, DataLayerError> {
        self.tx_runner
            .run_read_write(|tx| {
                Box::pin(async move {
                    let Some(current_batch) = sqlx::query(
                        r#"
SELECT status
FROM redeem_code_batches
WHERE id = $1
FOR UPDATE
                        "#,
                    )
                    .bind(&input.batch_id)
                    .fetch_optional(&mut **tx)
                    .await
                    .map_postgres_err()?
                    else {
                        return Ok(WalletMutationOutcome::NotFound);
                    };
                    let status: String = row_get(&current_batch, "status")?;
                    if status != "disabled" {
                        return Ok(WalletMutationOutcome::Invalid(
                            "only disabled redeem code batch can be deleted".to_string(),
                        ));
                    }

                    let redeemed_count_row = sqlx::query(
                        r#"
SELECT COUNT(*) AS total
FROM redeem_codes
WHERE batch_id = $1
  AND status = 'redeemed'
                        "#,
                    )
                    .bind(&input.batch_id)
                    .fetch_one(&mut **tx)
                    .await
                    .map_postgres_err()?;
                    let redeemed_count = read_count(redeemed_count_row)?;
                    if redeemed_count > 0 {
                        return Ok(WalletMutationOutcome::Invalid(
                            "redeemed batch cannot be deleted".to_string(),
                        ));
                    }

                    let row = sqlx::query(
                        r#"
SELECT
  batches.id,
  batches.name,
  CAST(batches.amount_usd AS DOUBLE PRECISION) AS amount_usd,
  batches.currency,
  batches.balance_bucket,
  CAST(batches.total_count AS BIGINT) AS total_count,
  CAST(COALESCE(stats.redeemed_count, 0) AS BIGINT) AS redeemed_count,
  CAST(COALESCE(stats.active_count, 0) AS BIGINT) AS active_count,
  batches.status,
  batches.description,
  batches.created_by,
  CAST(EXTRACT(EPOCH FROM batches.expires_at) AS BIGINT) AS expires_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM batches.created_at) AS BIGINT) AS created_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM batches.updated_at) AS BIGINT) AS updated_at_unix_secs
FROM redeem_code_batches AS batches
LEFT JOIN (
  SELECT
    batch_id,
    COUNT(*) FILTER (WHERE status = 'redeemed') AS redeemed_count,
    COUNT(*) FILTER (WHERE status = 'active') AS active_count
  FROM redeem_codes
  GROUP BY batch_id
) AS stats
  ON stats.batch_id = batches.id
WHERE batches.id = $1
LIMIT 1
                        "#,
                    )
                    .bind(&input.batch_id)
                    .fetch_one(&mut **tx)
                    .await
                    .map_postgres_err()?;
                    let batch = map_admin_redeem_code_batch_row(&row)?;
                    let _ = input.operator_id;

                    sqlx::query(
                        r#"
DELETE FROM redeem_code_batches
WHERE id = $1
                        "#,
                    )
                    .bind(&input.batch_id)
                    .execute(&mut **tx)
                    .await
                    .map_postgres_err()?;

                    Ok(WalletMutationOutcome::Applied(batch))
                })
            })
            .await
    }

    async fn disable_admin_redeem_code(
        &self,
        input: DisableAdminRedeemCodeInput,
    ) -> Result<WalletMutationOutcome<StoredAdminRedeemCode>, DataLayerError> {
        self.tx_runner
            .run_read_write(|tx| {
                Box::pin(async move {
                    let Some(current_code) = sqlx::query(
                        r#"
SELECT batch_id, status
FROM redeem_codes
WHERE id = $1
FOR UPDATE
                        "#,
                    )
                    .bind(&input.code_id)
                    .fetch_optional(&mut **tx)
                    .await
                    .map_postgres_err()?
                    else {
                        return Ok(WalletMutationOutcome::NotFound);
                    };
                    let batch_id: String = row_get(&current_code, "batch_id")?;
                    let status: String = row_get(&current_code, "status")?;
                    if status == "redeemed" {
                        return Ok(WalletMutationOutcome::Invalid(
                            "redeemed code cannot be disabled".to_string(),
                        ));
                    }
                    if status != "disabled" {
                        sqlx::query(
                            r#"
UPDATE redeem_codes
SET
  status = 'disabled',
  disabled_by = COALESCE($2, disabled_by),
  updated_at = NOW()
WHERE id = $1
                            "#,
                        )
                        .bind(&input.code_id)
                        .bind(input.operator_id.as_deref())
                        .execute(&mut **tx)
                        .await
                        .map_postgres_err()?;
                    }

                    let row = sqlx::query(
                        r#"
SELECT
  codes.id,
  codes.batch_id,
  batches.name AS batch_name,
  codes.code_prefix,
  codes.code_suffix,
  codes.status,
  codes.redeemed_by_user_id,
  users.username AS redeemed_by_user_name,
  codes.redeemed_wallet_id,
  codes.redeemed_payment_order_id,
  orders.order_no AS redeemed_order_no,
  CAST(EXTRACT(EPOCH FROM codes.redeemed_at) AS BIGINT) AS redeemed_at_unix_secs,
  codes.disabled_by,
  CAST(EXTRACT(EPOCH FROM batches.expires_at) AS BIGINT) AS expires_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM codes.created_at) AS BIGINT) AS created_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM codes.updated_at) AS BIGINT) AS updated_at_unix_secs
FROM redeem_codes AS codes
JOIN redeem_code_batches AS batches
  ON batches.id = codes.batch_id
LEFT JOIN users
  ON users.id = codes.redeemed_by_user_id
LEFT JOIN payment_orders AS orders
  ON orders.id = codes.redeemed_payment_order_id
WHERE codes.id = $1
LIMIT 1
                        "#,
                    )
                    .bind(&input.code_id)
                    .fetch_one(&mut **tx)
                    .await
                    .map_postgres_err()?;

                    sqlx::query(
                        r#"
UPDATE redeem_code_batches
SET
  updated_at = NOW()
WHERE id = $1
                        "#,
                    )
                    .bind(&batch_id)
                    .execute(&mut **tx)
                    .await
                    .map_postgres_err()?;

                    Ok(WalletMutationOutcome::Applied(map_admin_redeem_code_row(
                        &row,
                    )?))
                })
            })
            .await
    }

    async fn redeem_wallet_code(
        &self,
        input: RedeemWalletCodeInput,
    ) -> Result<RedeemWalletCodeOutcome, DataLayerError> {
        self.tx_runner
            .run_read_write(|tx| {
                Box::pin(async move {
                    let normalized_code = match normalize_redeem_code(&input.code) {
                        Some(value) => value,
                        None => return Ok(RedeemWalletCodeOutcome::InvalidCode),
                    };
                    let code_hash = hash_redeem_code(&normalized_code);
                    let now = Utc::now();
                    let now_unix_secs = now.timestamp().max(0) as u64;

                    let Some(code_row) = sqlx::query(
                        r#"
SELECT
  codes.id,
  codes.batch_id,
  codes.status,
  batches.name AS batch_name,
  batches.status AS batch_status,
  batches.balance_bucket,
  CAST(batches.amount_usd AS DOUBLE PRECISION) AS amount_usd,
  CAST(EXTRACT(EPOCH FROM batches.expires_at) AS BIGINT) AS batch_expires_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM codes.redeemed_at) AS BIGINT) AS redeemed_at_unix_secs
FROM redeem_codes AS codes
JOIN redeem_code_batches AS batches
  ON batches.id = codes.batch_id
WHERE codes.code_hash = $1
LIMIT 1
FOR UPDATE OF codes, batches
                        "#,
                    )
                    .bind(&code_hash)
                    .fetch_optional(&mut **tx)
                    .await
                    .map_postgres_err()?
                    else {
                        return Ok(RedeemWalletCodeOutcome::CodeNotFound);
                    };
                    let code_id: String = row_get(&code_row, "id")?;
                    let batch_id: String = row_get(&code_row, "batch_id")?;
                    let batch_name: String = row_get(&code_row, "batch_name")?;
                    let code_status: String = row_get(&code_row, "status")?;
                    let batch_status: String = row_get(&code_row, "batch_status")?;
                    let balance_bucket: String = row_get(&code_row, "balance_bucket")?;
                    let amount_usd: f64 = row_get(&code_row, "amount_usd")?;
                    let batch_expires_at_unix_secs = parse_optional_timestamp(
                        row_get(&code_row, "batch_expires_at_unix_secs")?,
                        "redeem_code_batches.expires_at",
                    )?;
                    let credits_recharge_balance =
                        redeem_code_credits_recharge_balance(&balance_bucket);

                    if code_status == "disabled" {
                        return Ok(RedeemWalletCodeOutcome::CodeDisabled);
                    }
                    if code_status == "redeemed" {
                        return Ok(RedeemWalletCodeOutcome::CodeRedeemed);
                    }
                    if batch_status != "active" {
                        return Ok(RedeemWalletCodeOutcome::BatchDisabled);
                    }
                    if batch_expires_at_unix_secs.is_some_and(|value| value <= now_unix_secs) {
                        return Ok(RedeemWalletCodeOutcome::CodeExpired);
                    }

                    let wallet_row = match sqlx::query(
                        r#"
SELECT
  id,
  user_id,
  api_key_id,
  CAST(balance AS DOUBLE PRECISION) AS balance,
  CAST(gift_balance AS DOUBLE PRECISION) AS gift_balance,
  limit_mode,
  currency,
  status,
  CAST(total_recharged AS DOUBLE PRECISION) AS total_recharged,
  CAST(total_consumed AS DOUBLE PRECISION) AS total_consumed,
  CAST(total_refunded AS DOUBLE PRECISION) AS total_refunded,
  CAST(total_adjusted AS DOUBLE PRECISION) AS total_adjusted,
  CAST(EXTRACT(EPOCH FROM updated_at) AS BIGINT) AS updated_at_unix_secs
FROM wallets
WHERE user_id = $1
FOR UPDATE
LIMIT 1
                        "#,
                    )
                    .bind(&input.user_id)
                    .fetch_optional(&mut **tx)
                    .await
                    .map_postgres_err()?
                    {
                        Some(row) => row,
                        None => {
                            let wallet_id = Uuid::new_v4().to_string();
                            sqlx::query(
                                r#"
INSERT INTO wallets (
  id,
  user_id,
  balance,
  gift_balance,
  limit_mode,
  currency,
  status,
  total_recharged,
  total_consumed,
  total_refunded,
  total_adjusted,
  created_at,
  updated_at
)
VALUES (
  $1,
  $2,
  0,
  0,
  'finite',
  'USD',
  'active',
  0,
  0,
  0,
  0,
  NOW(),
  NOW()
)
ON CONFLICT (user_id) DO UPDATE
SET updated_at = wallets.updated_at
RETURNING
  id,
  user_id,
  api_key_id,
  CAST(balance AS DOUBLE PRECISION) AS balance,
  CAST(gift_balance AS DOUBLE PRECISION) AS gift_balance,
  limit_mode,
  currency,
  status,
  CAST(total_recharged AS DOUBLE PRECISION) AS total_recharged,
  CAST(total_consumed AS DOUBLE PRECISION) AS total_consumed,
  CAST(total_refunded AS DOUBLE PRECISION) AS total_refunded,
  CAST(total_adjusted AS DOUBLE PRECISION) AS total_adjusted,
  CAST(EXTRACT(EPOCH FROM updated_at) AS BIGINT) AS updated_at_unix_secs
                                "#,
                            )
                            .bind(&wallet_id)
                            .bind(&input.user_id)
                            .fetch_one(&mut **tx)
                            .await
                            .map_postgres_err()?
                        }
                    };
                    let wallet_snapshot = map_wallet_row(&wallet_row)?;
                    if wallet_snapshot.status != "active" {
                        return Ok(RedeemWalletCodeOutcome::WalletInactive);
                    }

                    let before_recharge = wallet_snapshot.balance;
                    let before_gift = wallet_snapshot.gift_balance;
                    let before_total = before_recharge + before_gift;
                    let after_recharge = if credits_recharge_balance {
                        before_recharge + amount_usd
                    } else {
                        before_recharge
                    };
                    let after_gift = if credits_recharge_balance {
                        before_gift
                    } else {
                        before_gift + amount_usd
                    };

                    let wallet_row = sqlx::query(
                        r#"
UPDATE wallets
SET
  balance = $2,
  gift_balance = $3,
  total_recharged = total_recharged + $4,
  updated_at = NOW()
WHERE id = $1
RETURNING
  id,
  user_id,
  api_key_id,
  CAST(balance AS DOUBLE PRECISION) AS balance,
  CAST(gift_balance AS DOUBLE PRECISION) AS gift_balance,
  limit_mode,
  currency,
  status,
  CAST(total_recharged AS DOUBLE PRECISION) AS total_recharged,
  CAST(total_consumed AS DOUBLE PRECISION) AS total_consumed,
  CAST(total_refunded AS DOUBLE PRECISION) AS total_refunded,
  CAST(total_adjusted AS DOUBLE PRECISION) AS total_adjusted,
  CAST(EXTRACT(EPOCH FROM updated_at) AS BIGINT) AS updated_at_unix_secs
                        "#,
                    )
                    .bind(&wallet_snapshot.id)
                    .bind(after_recharge)
                    .bind(after_gift)
                    .bind(amount_usd)
                    .fetch_one(&mut **tx)
                    .await
                    .map_postgres_err()?;
                    let wallet = map_wallet_row(&wallet_row)?;

                    let order_id = Uuid::new_v4().to_string();
                    let gateway_order_id = format!("card_{}", Uuid::new_v4().simple());
                    let payment_method = redeem_code_payment_method(&balance_bucket);
                    let refundable_amount_usd =
                        redeem_code_refundable_amount(&balance_bucket, amount_usd);
                    let gateway_response = serde_json::json!({
                        "source": "redeem_code",
                        "batch_id": batch_id,
                        "batch_name": batch_name,
                        "balance_bucket": balance_bucket,
                    });
                    let order_row = sqlx::query(
                        r#"
INSERT INTO payment_orders (
  id,
  order_no,
  wallet_id,
  user_id,
  amount_usd,
  refunded_amount_usd,
  refundable_amount_usd,
  payment_method,
  gateway_order_id,
  gateway_response,
  status,
  created_at,
  paid_at,
  credited_at
)
VALUES (
  $1,
  $2,
  $3,
  $4,
  $5,
  0,
  $6,
  $7,
  $8,
  $9,
  'credited',
  NOW(),
  NOW(),
  NOW()
)
RETURNING
  id,
  order_no,
  wallet_id,
  user_id,
  CAST(amount_usd AS DOUBLE PRECISION) AS amount_usd,
  CAST(pay_amount AS DOUBLE PRECISION) AS pay_amount,
  pay_currency,
  CAST(exchange_rate AS DOUBLE PRECISION) AS exchange_rate,
  CAST(refunded_amount_usd AS DOUBLE PRECISION) AS refunded_amount_usd,
  CAST(refundable_amount_usd AS DOUBLE PRECISION) AS refundable_amount_usd,
  payment_method,
  gateway_order_id,
  gateway_response,
  status,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM paid_at) AS BIGINT) AS paid_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM credited_at) AS BIGINT) AS credited_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM expires_at) AS BIGINT) AS expires_at_unix_secs
                        "#,
                    )
                    .bind(&order_id)
                    .bind(&input.order_no)
                    .bind(&wallet.id)
                    .bind(&input.user_id)
                    .bind(amount_usd)
                    .bind(refundable_amount_usd)
                    .bind(payment_method)
                    .bind(&gateway_order_id)
                    .bind(&gateway_response)
                    .fetch_one(&mut **tx)
                    .await
                    .map_postgres_err()?;

                    sqlx::query(
                        r#"
INSERT INTO wallet_transactions (
  id,
  wallet_id,
  category,
  reason_code,
  amount,
  balance_before,
  balance_after,
  recharge_balance_before,
  recharge_balance_after,
  gift_balance_before,
  gift_balance_after,
  link_type,
  link_id,
  operator_id,
  description,
  created_at
)
VALUES (
  $1,
  $2,
  'recharge',
  'topup_card_code',
  $3,
  $4,
  $5,
  $6,
  $7,
  $8,
  $9,
  'payment_order',
  $10,
  NULL,
  '兑换码充值',
  NOW()
)
                        "#,
                    )
                    .bind(Uuid::new_v4().to_string())
                    .bind(&wallet.id)
                    .bind(amount_usd)
                    .bind(before_total)
                    .bind(after_recharge + after_gift)
                    .bind(before_recharge)
                    .bind(after_recharge)
                    .bind(before_gift)
                    .bind(after_gift)
                    .bind(&order_id)
                    .execute(&mut **tx)
                    .await
                    .map_postgres_err()?;

                    sqlx::query(
                        r#"
UPDATE redeem_codes
SET
  status = 'redeemed',
  redeemed_by_user_id = $2,
  redeemed_wallet_id = $3,
  redeemed_payment_order_id = $4,
  redeemed_at = NOW(),
  updated_at = NOW()
WHERE id = $1
                        "#,
                    )
                    .bind(&code_id)
                    .bind(&input.user_id)
                    .bind(&wallet.id)
                    .bind(&order_id)
                    .execute(&mut **tx)
                    .await
                    .map_postgres_err()?;

                    sqlx::query(
                        r#"
UPDATE redeem_code_batches
SET
  updated_at = NOW()
WHERE id = $1
                        "#,
                    )
                    .bind(&batch_id)
                    .execute(&mut **tx)
                    .await
                    .map_postgres_err()?;

                    Ok(RedeemWalletCodeOutcome::Redeemed {
                        wallet,
                        order: map_admin_payment_order_row(&order_row)?,
                        amount_usd,
                        batch_name,
                    })
                })
            })
            .await
    }
}

fn as_i64(value: usize, field: &str) -> Result<i64, DataLayerError> {
    i64::try_from(value)
        .map_err(|_| DataLayerError::UnexpectedValue(format!("invalid {field}: {value}")))
}

fn default_refund_mode_for_payment_method(payment_method: &str) -> &'static str {
    if matches!(
        payment_method,
        "admin_manual" | "card_recharge" | "card_code" | "gift_code"
    ) {
        return "offline_payout";
    }
    "original_channel"
}

fn payment_gateway_response_map(
    value: Option<serde_json::Value>,
) -> serde_json::Map<String, serde_json::Value> {
    match value {
        Some(serde_json::Value::Object(map)) => map,
        _ => serde_json::Map::new(),
    }
}

fn normalize_redeem_code(value: &str) -> Option<String> {
    let normalized = value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .map(|ch| ch.to_ascii_uppercase())
        .collect::<String>();
    if normalized.len() < 16 {
        None
    } else {
        Some(normalized)
    }
}

fn generate_redeem_code_normalized() -> String {
    Uuid::new_v4().simple().to_string().to_ascii_uppercase()
}

fn format_redeem_code(normalized: &str) -> String {
    normalized
        .as_bytes()
        .chunks(8)
        .map(|chunk| std::str::from_utf8(chunk).unwrap_or_default())
        .collect::<Vec<_>>()
        .join("-")
}

fn hash_redeem_code(normalized: &str) -> String {
    use sha2::Digest;

    format!("{:x}", sha2::Sha256::digest(normalized.as_bytes()))
}

fn redeem_code_prefix(normalized: &str) -> String {
    normalized.chars().take(4).collect()
}

fn redeem_code_suffix(normalized: &str) -> String {
    normalized
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

fn mask_redeem_code(prefix: &str, suffix: &str) -> String {
    format!("{prefix}****{suffix}")
}

async fn update_payment_callback_failure(
    tx: &mut crate::postgres::PostgresTransaction,
    callback_id: &str,
    input: &ProcessPaymentCallbackInput,
    error: &str,
) -> Result<(), DataLayerError> {
    sqlx::query(
        r#"
UPDATE payment_callbacks
SET signature_valid = $2,
    status = 'failed',
    error_message = $3,
    payload_hash = $4,
    payload = $5,
    processed_at = NOW(),
    order_no = COALESCE($6, order_no),
    gateway_order_id = COALESCE($7, gateway_order_id)
WHERE id = $1
        "#,
    )
    .bind(callback_id)
    .bind(input.signature_valid)
    .bind(error)
    .bind(&input.payload_hash)
    .bind(&input.payload)
    .bind(input.order_no.as_deref())
    .bind(input.gateway_order_id.as_deref())
    .execute(&mut **tx)
    .await
    .map_postgres_err()?;
    Ok(())
}

async fn mark_payment_callback_processed(
    tx: &mut crate::postgres::PostgresTransaction,
    callback_id: &str,
    input: &ProcessPaymentCallbackInput,
    order_id: &str,
    order_no: &str,
) -> Result<(), DataLayerError> {
    sqlx::query(
        r#"
UPDATE payment_callbacks
SET payment_order_id = $2,
    signature_valid = true,
    status = 'processed',
    error_message = NULL,
    payload_hash = $3,
    payload = $4,
    processed_at = NOW(),
    order_no = $5,
    gateway_order_id = COALESCE($6, gateway_order_id)
WHERE id = $1
        "#,
    )
    .bind(callback_id)
    .bind(order_id)
    .bind(&input.payload_hash)
    .bind(&input.payload)
    .bind(order_no)
    .bind(input.gateway_order_id.as_deref())
    .execute(&mut **tx)
    .await
    .map_postgres_err()?;
    Ok(())
}

fn parse_timestamp(value: i64, field: &str) -> Result<u64, DataLayerError> {
    u64::try_from(value)
        .map_err(|_| DataLayerError::UnexpectedValue(format!("{field} is negative: {value}")))
}

fn parse_optional_timestamp(
    value: Option<i64>,
    field: &str,
) -> Result<Option<u64>, DataLayerError> {
    value.map(|inner| parse_timestamp(inner, field)).transpose()
}

fn row_get<T>(row: &PgRow, column: &str) -> Result<T, DataLayerError>
where
    for<'r> T: sqlx::Decode<'r, sqlx::Postgres> + sqlx::Type<sqlx::Postgres>,
{
    row.try_get(column).map_postgres_err()
}

fn read_count(row: PgRow) -> Result<u64, DataLayerError> {
    let total = row.try_get::<i64, _>("total").map_postgres_err()?;
    Ok(total.max(0) as u64)
}

async fn collect_query_rows<T, S>(
    mut rows: S,
    map_row: fn(&PgRow) -> Result<T, DataLayerError>,
) -> Result<Vec<T>, DataLayerError>
where
    S: TryStream<Ok = PgRow, Error = sqlx::Error> + Unpin,
{
    let mut items = Vec::new();
    while let Some(row) = rows.try_next().await.map_postgres_err()? {
        items.push(map_row(&row)?);
    }
    Ok(items)
}

fn map_admin_wallet_list_item_row(
    row: &PgRow,
) -> Result<StoredAdminWalletListItem, DataLayerError> {
    Ok(StoredAdminWalletListItem {
        id: row_get(row, "id")?,
        user_id: row_get(row, "user_id")?,
        api_key_id: row_get(row, "api_key_id")?,
        balance: row_get(row, "balance")?,
        gift_balance: row_get(row, "gift_balance")?,
        limit_mode: row_get(row, "limit_mode")?,
        currency: row_get(row, "currency")?,
        status: row_get(row, "status")?,
        total_recharged: row_get(row, "total_recharged")?,
        total_consumed: row_get(row, "total_consumed")?,
        total_refunded: row_get(row, "total_refunded")?,
        total_adjusted: row_get(row, "total_adjusted")?,
        user_name: row_get(row, "user_name")?,
        api_key_name: row_get(row, "api_key_name")?,
        created_at_unix_ms: parse_optional_timestamp(
            row_get(row, "created_at_unix_ms")?,
            "wallets.created_at",
        )?,
        updated_at_unix_secs: parse_optional_timestamp(
            row_get(row, "updated_at_unix_secs")?,
            "wallets.updated_at",
        )?,
    })
}

fn map_admin_wallet_ledger_item_row(
    row: &PgRow,
) -> Result<StoredAdminWalletLedgerItem, DataLayerError> {
    Ok(StoredAdminWalletLedgerItem {
        id: row_get(row, "id")?,
        wallet_id: row_get(row, "wallet_id")?,
        category: row_get(row, "category")?,
        reason_code: row_get(row, "reason_code")?,
        amount: row_get(row, "amount")?,
        balance_before: row_get(row, "balance_before")?,
        balance_after: row_get(row, "balance_after")?,
        recharge_balance_before: row_get(row, "recharge_balance_before")?,
        recharge_balance_after: row_get(row, "recharge_balance_after")?,
        gift_balance_before: row_get(row, "gift_balance_before")?,
        gift_balance_after: row_get(row, "gift_balance_after")?,
        link_type: row_get(row, "link_type")?,
        link_id: row_get(row, "link_id")?,
        operator_id: row_get(row, "operator_id")?,
        operator_name: row_get(row, "operator_name")?,
        operator_email: row_get(row, "operator_email")?,
        description: row_get(row, "description")?,
        wallet_user_id: row_get(row, "user_id")?,
        wallet_user_name: row_get(row, "wallet_user_name")?,
        wallet_api_key_id: row_get(row, "api_key_id")?,
        api_key_name: row_get(row, "api_key_name")?,
        wallet_status: row_get(row, "wallet_status")?,
        created_at_unix_ms: parse_optional_timestamp(
            row_get(row, "created_at_unix_ms")?,
            "wallet_transactions.created_at",
        )?,
    })
}

fn map_admin_wallet_refund_request_item_row(
    row: &PgRow,
) -> Result<StoredAdminWalletRefundRequestItem, DataLayerError> {
    Ok(StoredAdminWalletRefundRequestItem {
        id: row_get(row, "id")?,
        refund_no: row_get(row, "refund_no")?,
        wallet_id: row_get(row, "wallet_id")?,
        user_id: row_get(row, "user_id")?,
        payment_order_id: row_get(row, "payment_order_id")?,
        source_type: row_get(row, "source_type")?,
        source_id: row_get(row, "source_id")?,
        refund_mode: row_get(row, "refund_mode")?,
        amount_usd: row_get(row, "amount_usd")?,
        status: row_get(row, "status")?,
        reason: row_get(row, "reason")?,
        failure_reason: row_get(row, "failure_reason")?,
        gateway_refund_id: row_get(row, "gateway_refund_id")?,
        payout_method: row_get(row, "payout_method")?,
        payout_reference: row_get(row, "payout_reference")?,
        payout_proof: row_get(row, "payout_proof")?,
        requested_by: row_get(row, "requested_by")?,
        approved_by: row_get(row, "approved_by")?,
        processed_by: row_get(row, "processed_by")?,
        wallet_user_id: row_get(row, "wallet_user_id")?,
        wallet_user_name: row_get(row, "wallet_user_name")?,
        wallet_api_key_id: row_get(row, "wallet_api_key_id")?,
        api_key_name: row_get(row, "api_key_name")?,
        wallet_status: row_get(row, "wallet_status")?,
        created_at_unix_ms: parse_optional_timestamp(
            row_get(row, "created_at_unix_ms")?,
            "refund_requests.created_at",
        )?,
        updated_at_unix_secs: parse_optional_timestamp(
            row_get(row, "updated_at_unix_secs")?,
            "refund_requests.updated_at",
        )?,
        processed_at_unix_secs: parse_optional_timestamp(
            row_get(row, "processed_at_unix_secs")?,
            "refund_requests.processed_at",
        )?,
        completed_at_unix_secs: parse_optional_timestamp(
            row_get(row, "completed_at_unix_secs")?,
            "refund_requests.completed_at",
        )?,
    })
}

fn map_admin_wallet_transaction_row(
    row: &PgRow,
) -> Result<StoredAdminWalletTransaction, DataLayerError> {
    Ok(StoredAdminWalletTransaction {
        id: row_get(row, "id")?,
        wallet_id: row_get(row, "wallet_id")?,
        category: row_get(row, "category")?,
        reason_code: row_get(row, "reason_code")?,
        amount: row_get(row, "amount")?,
        balance_before: row_get(row, "balance_before")?,
        balance_after: row_get(row, "balance_after")?,
        recharge_balance_before: row_get(row, "recharge_balance_before")?,
        recharge_balance_after: row_get(row, "recharge_balance_after")?,
        gift_balance_before: row_get(row, "gift_balance_before")?,
        gift_balance_after: row_get(row, "gift_balance_after")?,
        link_type: row_get(row, "link_type")?,
        link_id: row_get(row, "link_id")?,
        operator_id: row_get(row, "operator_id")?,
        operator_name: row_get(row, "operator_name")?,
        operator_email: row_get(row, "operator_email")?,
        description: row_get(row, "description")?,
        created_at_unix_ms: parse_optional_timestamp(
            row_get(row, "created_at_unix_ms")?,
            "wallet_transactions.created_at",
        )?,
    })
}

fn map_wallet_daily_usage_row(row: &PgRow) -> Result<StoredWalletDailyUsageLedger, DataLayerError> {
    Ok(StoredWalletDailyUsageLedger {
        id: row_get(row, "id")?,
        billing_date: row_get(row, "billing_date")?,
        billing_timezone: row_get(row, "billing_timezone")?,
        total_cost_usd: row_get(row, "total_cost_usd")?,
        total_requests: row_get::<i64>(row, "total_requests")?.max(0) as u64,
        input_tokens: row_get::<i64>(row, "input_tokens")?.max(0) as u64,
        output_tokens: row_get::<i64>(row, "output_tokens")?.max(0) as u64,
        cache_creation_tokens: row_get::<i64>(row, "cache_creation_tokens")?.max(0) as u64,
        cache_read_tokens: row_get::<i64>(row, "cache_read_tokens")?.max(0) as u64,
        first_finalized_at_unix_secs: parse_optional_timestamp(
            row_get(row, "first_finalized_at_unix_secs")?,
            "wallet_daily_usage_ledgers.first_finalized_at",
        )?,
        last_finalized_at_unix_secs: parse_optional_timestamp(
            row_get(row, "last_finalized_at_unix_secs")?,
            "wallet_daily_usage_ledgers.last_finalized_at",
        )?,
        aggregated_at_unix_secs: parse_optional_timestamp(
            row_get(row, "aggregated_at_unix_secs")?,
            "wallet_daily_usage_ledgers.aggregated_at",
        )?,
    })
}

fn map_admin_wallet_refund_row(row: &PgRow) -> Result<StoredAdminWalletRefund, DataLayerError> {
    Ok(StoredAdminWalletRefund {
        id: row_get(row, "id")?,
        refund_no: row_get(row, "refund_no")?,
        wallet_id: row_get(row, "wallet_id")?,
        user_id: row_get(row, "user_id")?,
        payment_order_id: row_get(row, "payment_order_id")?,
        source_type: row_get(row, "source_type")?,
        source_id: row_get(row, "source_id")?,
        refund_mode: row_get(row, "refund_mode")?,
        amount_usd: row_get(row, "amount_usd")?,
        status: row_get(row, "status")?,
        reason: row_get(row, "reason")?,
        failure_reason: row_get(row, "failure_reason")?,
        gateway_refund_id: row_get(row, "gateway_refund_id")?,
        payout_method: row_get(row, "payout_method")?,
        payout_reference: row_get(row, "payout_reference")?,
        payout_proof: row_get(row, "payout_proof")?,
        requested_by: row_get(row, "requested_by")?,
        approved_by: row_get(row, "approved_by")?,
        processed_by: row_get(row, "processed_by")?,
        created_at_unix_ms: parse_timestamp(
            row_get(row, "created_at_unix_ms")?,
            "refund_requests.created_at",
        )?,
        updated_at_unix_secs: parse_timestamp(
            row_get(row, "updated_at_unix_secs")?,
            "refund_requests.updated_at",
        )?,
        processed_at_unix_secs: parse_optional_timestamp(
            row_get(row, "processed_at_unix_secs")?,
            "refund_requests.processed_at",
        )?,
        completed_at_unix_secs: parse_optional_timestamp(
            row_get(row, "completed_at_unix_secs")?,
            "refund_requests.completed_at",
        )?,
    })
}

fn map_admin_payment_callback_row(
    row: &PgRow,
) -> Result<StoredAdminPaymentCallback, DataLayerError> {
    Ok(StoredAdminPaymentCallback {
        id: row_get(row, "id")?,
        payment_order_id: row_get(row, "payment_order_id")?,
        payment_method: row_get(row, "payment_method")?,
        callback_key: row_get(row, "callback_key")?,
        order_no: row_get(row, "order_no")?,
        gateway_order_id: row_get(row, "gateway_order_id")?,
        payload_hash: row_get(row, "payload_hash")?,
        signature_valid: row_get(row, "signature_valid")?,
        status: row_get(row, "status")?,
        payload: row_get(row, "payload")?,
        error_message: row_get(row, "error_message")?,
        created_at_unix_ms: parse_timestamp(
            row_get(row, "created_at_unix_ms")?,
            "payment_callbacks.created_at",
        )?,
        processed_at_unix_secs: parse_optional_timestamp(
            row_get(row, "processed_at_unix_secs")?,
            "payment_callbacks.processed_at",
        )?,
    })
}

fn map_admin_redeem_code_batch_row(
    row: &PgRow,
) -> Result<StoredAdminRedeemCodeBatch, DataLayerError> {
    Ok(StoredAdminRedeemCodeBatch {
        id: row_get(row, "id")?,
        name: row_get(row, "name")?,
        amount_usd: row_get(row, "amount_usd")?,
        currency: row_get(row, "currency")?,
        balance_bucket: row_get(row, "balance_bucket")?,
        total_count: row_get::<i64>(row, "total_count")?.max(0) as u64,
        redeemed_count: row_get::<i64>(row, "redeemed_count")?.max(0) as u64,
        active_count: row_get::<i64>(row, "active_count")?.max(0) as u64,
        status: row_get(row, "status")?,
        description: row_get(row, "description")?,
        created_by: row_get(row, "created_by")?,
        expires_at_unix_secs: parse_optional_timestamp(
            row_get(row, "expires_at_unix_secs")?,
            "redeem_code_batches.expires_at",
        )?,
        created_at_unix_ms: parse_timestamp(
            row_get(row, "created_at_unix_ms")?,
            "redeem_code_batches.created_at",
        )?,
        updated_at_unix_secs: parse_timestamp(
            row_get(row, "updated_at_unix_secs")?,
            "redeem_code_batches.updated_at",
        )?,
    })
}

fn map_admin_redeem_code_row(row: &PgRow) -> Result<StoredAdminRedeemCode, DataLayerError> {
    let code_prefix: String = row_get(row, "code_prefix")?;
    let code_suffix: String = row_get(row, "code_suffix")?;
    Ok(StoredAdminRedeemCode {
        id: row_get(row, "id")?,
        batch_id: row_get(row, "batch_id")?,
        batch_name: row_get(row, "batch_name")?,
        code_prefix: code_prefix.clone(),
        code_suffix: code_suffix.clone(),
        masked_code: mask_redeem_code(&code_prefix, &code_suffix),
        status: row_get(row, "status")?,
        redeemed_by_user_id: row_get(row, "redeemed_by_user_id")?,
        redeemed_by_user_name: row_get(row, "redeemed_by_user_name")?,
        redeemed_wallet_id: row_get(row, "redeemed_wallet_id")?,
        redeemed_payment_order_id: row_get(row, "redeemed_payment_order_id")?,
        redeemed_order_no: row_get(row, "redeemed_order_no")?,
        redeemed_at_unix_secs: parse_optional_timestamp(
            row_get(row, "redeemed_at_unix_secs")?,
            "redeem_codes.redeemed_at",
        )?,
        disabled_by: row_get(row, "disabled_by")?,
        expires_at_unix_secs: parse_optional_timestamp(
            row_get(row, "expires_at_unix_secs")?,
            "redeem_code_batches.expires_at",
        )?,
        created_at_unix_ms: parse_timestamp(
            row_get(row, "created_at_unix_ms")?,
            "redeem_codes.created_at",
        )?,
        updated_at_unix_secs: parse_timestamp(
            row_get(row, "updated_at_unix_secs")?,
            "redeem_codes.updated_at",
        )?,
    })
}

fn map_admin_payment_order_row(row: &PgRow) -> Result<StoredAdminPaymentOrder, DataLayerError> {
    Ok(StoredAdminPaymentOrder {
        id: row_get(row, "id")?,
        order_no: row_get(row, "order_no")?,
        wallet_id: row_get(row, "wallet_id")?,
        user_id: row_get(row, "user_id")?,
        amount_usd: row_get(row, "amount_usd")?,
        pay_amount: row_get(row, "pay_amount")?,
        pay_currency: row_get(row, "pay_currency")?,
        exchange_rate: row_get(row, "exchange_rate")?,
        refunded_amount_usd: row_get(row, "refunded_amount_usd")?,
        refundable_amount_usd: row_get(row, "refundable_amount_usd")?,
        payment_method: row_get(row, "payment_method")?,
        gateway_order_id: row_get(row, "gateway_order_id")?,
        gateway_response: row_get(row, "gateway_response")?,
        status: row_get(row, "status")?,
        created_at_unix_ms: parse_timestamp(
            row_get(row, "created_at_unix_ms")?,
            "payment_orders.created_at",
        )?,
        paid_at_unix_secs: parse_optional_timestamp(
            row_get(row, "paid_at_unix_secs")?,
            "payment_orders.paid_at",
        )?,
        credited_at_unix_secs: parse_optional_timestamp(
            row_get(row, "credited_at_unix_secs")?,
            "payment_orders.credited_at",
        )?,
        expires_at_unix_secs: parse_optional_timestamp(
            row_get(row, "expires_at_unix_secs")?,
            "payment_orders.expires_at",
        )?,
    })
}

fn map_wallet_row(row: &sqlx::postgres::PgRow) -> Result<StoredWalletSnapshot, DataLayerError> {
    StoredWalletSnapshot::new(
        row_get(row, "id")?,
        row_get(row, "user_id")?,
        row_get(row, "api_key_id")?,
        row_get(row, "balance")?,
        row_get(row, "gift_balance")?,
        row_get(row, "limit_mode")?,
        row_get(row, "currency")?,
        row_get(row, "status")?,
        row_get(row, "total_recharged")?,
        row_get(row, "total_consumed")?,
        row_get(row, "total_refunded")?,
        row_get(row, "total_adjusted")?,
        row_get(row, "updated_at_unix_secs")?,
    )
}

#[cfg(test)]
mod tests {
    use super::SqlxWalletRepository;
    use crate::postgres::{PostgresPoolConfig, PostgresPoolFactory};

    #[tokio::test]
    async fn repository_constructs_from_lazy_pool() {
        let factory = PostgresPoolFactory::new(PostgresPoolConfig {
            database_url: "postgres://localhost/aether".to_string(),
            min_connections: 1,
            max_connections: 4,
            acquire_timeout_ms: 1_000,
            idle_timeout_ms: 5_000,
            max_lifetime_ms: 30_000,
            statement_cache_capacity: 64,
            require_ssl: false,
        })
        .expect("factory should build");

        let pool = factory.connect_lazy().expect("pool should build");
        let _repository = SqlxWalletRepository::new(pool);
    }
}
