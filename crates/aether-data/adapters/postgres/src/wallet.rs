use async_trait::async_trait;
use chrono::Utc;
use futures_util::{stream::TryStream, TryStreamExt};
use sqlx::{postgres::PgRow, PgPool, Row};
use uuid::Uuid;

use aether_data_contracts::repository::billing::{
    checked_plan_duration_days_from_snapshot, entitlements_have_replacement_selector,
    entitlements_should_replace_existing,
};
use aether_data_contracts::repository::wallet::{
    canonicalize_payment_method, canonicalize_wallet_refund_fields,
    payment_callback_amount_matches_order, payment_callback_method_matches_order,
    payment_callback_provider_matches_order, payment_order_is_failed_wallet_checkout_placeholder,
    payment_order_is_uncertain_wallet_checkout_placeholder,
    payment_order_refund_amounts_are_consistent,
    payment_order_stripe_client_secret_cas_replacement, project_wallet_gateway_response,
    project_wallet_recharge_gateway_response, redeem_code_payment_method,
    redeem_code_refundable_amount, validate_admin_redeem_code_batch_input,
    validate_manual_wallet_recharge, validate_payment_order_credit_amounts,
    validate_plan_purchase_order_input, validate_plan_wallet_credit_entitlements,
    validate_redeem_wallet_credit, validate_wallet_recharge_order_input,
    wallet_recharge_checkout_claim_response, wallet_recharge_checkout_claim_token,
    wallet_recharge_checkout_failed_response, wallet_recharge_checkout_uncertain_response,
    wallet_recharge_order_is_reclaimable_placeholder, wallet_recharge_replay_matches,
    wallet_recharge_response_is_checkout_placeholder, wallet_refund_proof_is_success,
    AdjustWalletBalanceInput, AdminPaymentOrderListQuery, AdminRedeemCodeBatchListQuery,
    AdminRedeemCodeListQuery, AdminWalletLedgerQuery, AdminWalletListQuery,
    AdminWalletRefundRequestListQuery, CompareAndSwapPaymentOrderStripeClientSecretInput,
    CompleteAdminWalletRefundInput, CreateAdminRedeemCodeBatchInput,
    CreateAdminRedeemCodeBatchResult, CreateManualWalletRechargeInput,
    CreatePlanPurchaseOrderInput, CreatePlanPurchaseOrderOutcome, CreateWalletRechargeOrderInput,
    CreateWalletRechargeOrderOutcome, CreateWalletRefundRequestInput,
    CreateWalletRefundRequestOutcome, CreatedAdminRedeemCodePlaintext,
    CreditAdminPaymentOrderInput, DeleteAdminRedeemCodeBatchInput,
    DisableAdminRedeemCodeBatchInput, DisableAdminRedeemCodeInput, FailAdminWalletRefundInput,
    FailWalletRechargeCheckoutInput, InitializeAuthWalletOutcome, ProcessAdminWalletRefundInput,
    ProcessPaymentCallbackInput, ProcessPaymentCallbackOutcome, ReclaimWalletRechargeCheckoutInput,
    RedeemWalletCodeInput, RedeemWalletCodeOutcome, StoredAdminPaymentCallback,
    StoredAdminPaymentCallbackPage, StoredAdminPaymentOrder, StoredAdminPaymentOrderPage,
    StoredAdminRedeemCode, StoredAdminRedeemCodeBatch, StoredAdminRedeemCodeBatchPage,
    StoredAdminRedeemCodePage, StoredAdminWalletLedgerItem, StoredAdminWalletLedgerPage,
    StoredAdminWalletListItem, StoredAdminWalletListPage, StoredAdminWalletRefund,
    StoredAdminWalletRefundPage, StoredAdminWalletRefundRequestItem,
    StoredAdminWalletRefundRequestPage, StoredAdminWalletTransaction,
    StoredAdminWalletTransactionPage, StoredWalletDailyUsageLedger,
    StoredWalletDailyUsageLedgerPage, StoredWalletSnapshot, UpdateAdminWalletRefundGatewayInput,
    UpdateWalletRechargeCheckoutInput, WalletLookupKey, WalletMutationOutcome,
    WalletReadRepository, WalletWriteRepository,
};
use aether_data_contracts::DataLayerError;

use crate::{
    error::{postgres_error, SqlxResultExt},
    PostgresTransactionRunner,
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

const COUNT_PENDING_REFUNDS_BY_USER_SQL: &str = r#"
SELECT COUNT(*) AS total
FROM refund_requests
WHERE user_id = $1
  AND status = ANY($2::TEXT[])
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
        WHEN status = 'pending' AND expires_at IS NOT NULL AND expires_at <= NOW() THEN 'expired'
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
  payment_provider,
  payment_channel,
  order_kind,
  product_id,
  product_snapshot,
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
        WHEN status = 'pending' AND expires_at IS NOT NULL AND expires_at <= NOW() THEN 'expired'
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
  payment_provider,
  payment_channel,
  order_kind,
  product_id,
  product_snapshot,
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
  AND order_kind = 'wallet_recharge'
"#;

const COUNT_PENDING_PAYMENT_ORDERS_BY_USER_SQL: &str = r#"
SELECT COUNT(*) AS total
FROM payment_orders
WHERE user_id = $1
  AND status = ANY($2::TEXT[])
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
  payment_provider,
  payment_channel,
  order_kind,
  product_id,
  product_snapshot,
  gateway_order_id,
  gateway_response,
  CASE
    WHEN status = 'pending' AND expires_at IS NOT NULL AND expires_at <= now() THEN 'expired'
    ELSE status
  END AS status,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM paid_at) AS BIGINT) AS paid_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM credited_at) AS BIGINT) AS credited_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM expires_at) AS BIGINT) AS expires_at_unix_secs
FROM payment_orders
WHERE user_id = $1
  AND order_kind = 'wallet_recharge'
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
  payment_provider,
  payment_channel,
  order_kind,
  product_id,
  product_snapshot,
  gateway_order_id,
  gateway_response,
  CASE
    WHEN status = 'pending' AND expires_at IS NOT NULL AND expires_at <= now() THEN 'expired'
    ELSE status
  END AS status,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM paid_at) AS BIGINT) AS paid_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM credited_at) AS BIGINT) AS credited_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM expires_at) AS BIGINT) AS expires_at_unix_secs
FROM payment_orders
WHERE user_id = $1
  AND id = $2
  AND order_kind = 'wallet_recharge'
LIMIT 1
"#;

const FIND_WALLET_RECHARGE_ORDER_BY_ORDER_NO_SQL: &str = r#"
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
  payment_provider,
  payment_channel,
  order_kind,
  product_id,
  product_snapshot,
  gateway_order_id,
  gateway_response,
  CASE
    WHEN status = 'pending' AND expires_at IS NOT NULL AND expires_at <= now() THEN 'expired'
    ELSE status
  END AS status,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM paid_at) AS BIGINT) AS paid_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM credited_at) AS BIGINT) AS credited_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM expires_at) AS BIGINT) AS expires_at_unix_secs
FROM payment_orders
WHERE user_id = $1
  AND order_no = $2
  AND order_kind = 'wallet_recharge'
LIMIT 1
"#;

// A recharge retry can discover an existing order after it has provisioned a
// wallet for a user that did not have one yet.  Remove that wallet only while
// it is still an untouched, unreferenced provisional row; any unexpected
// reference keeps it durable rather than risking data loss.
const DELETE_PROVISIONAL_RECHARGE_WALLET_SQL: &str = r#"
DELETE FROM wallets
WHERE id = $1
  AND user_id = $2
  AND api_key_id IS NULL
  AND balance = 0
  AND gift_balance = 0
  AND total_recharged = 0
  AND total_consumed = 0
  AND total_refunded = 0
  AND total_adjusted = 0
  AND limit_mode IN ('finite', 'unlimited')
  AND currency = 'USD'
  AND status = 'active'
  AND NOT EXISTS (SELECT 1 FROM payment_orders p WHERE p.wallet_id = wallets.id)
  AND NOT EXISTS (SELECT 1 FROM refund_requests r WHERE r.wallet_id = wallets.id)
  AND NOT EXISTS (
      SELECT 1 FROM wallet_daily_usage_ledgers d WHERE d.wallet_id = wallets.id
  )
  AND NOT EXISTS (SELECT 1 FROM usage u WHERE u.wallet_id = wallets.id)
  AND NOT EXISTS (
      SELECT 1 FROM usage_settlement_snapshots s WHERE s.wallet_id = wallets.id
  )
  AND NOT EXISTS (SELECT 1 FROM wallet_transactions t WHERE t.wallet_id = wallets.id)
  AND NOT EXISTS (
      SELECT 1 FROM redeem_codes c WHERE c.redeemed_wallet_id = wallets.id
  )
"#;

const FIND_PENDING_PLAN_PURCHASE_ORDER_BY_USER_SQL: &str = r#"
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
  payment_provider,
  payment_channel,
  order_kind,
  product_id,
  product_snapshot,
  gateway_order_id,
  gateway_response,
  status,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM paid_at) AS BIGINT) AS paid_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM credited_at) AS BIGINT) AS credited_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM expires_at) AS BIGINT) AS expires_at_unix_secs
FROM payment_orders
WHERE user_id = $1
  AND product_id = $2
  AND order_kind = 'plan_purchase'
  AND status = 'pending'
  AND expires_at > NOW()
ORDER BY created_at DESC
LIMIT 1
"#;

const FIND_PAYMENT_ORDER_BY_ORDER_NO_SQL: &str = r#"
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
  payment_provider,
  payment_channel,
  order_kind,
  product_id,
  product_snapshot,
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

    async fn update_auth_user_wallet_limit_mode(
        &self,
        user_id: &str,
        limit_mode: &str,
    ) -> Result<Option<StoredWalletSnapshot>, DataLayerError> {
        let result = sqlx::query(
            "UPDATE wallets SET limit_mode = $2, updated_at = NOW() WHERE user_id = $1",
        )
        .bind(user_id)
        .bind(limit_mode)
        .execute(&self.pool)
        .await
        .map_postgres_err()?;
        if result.rows_affected() == 0 {
            return Ok(None);
        }
        self.find(WalletLookupKey::UserId(user_id)).await
    }

    async fn update_auth_api_key_wallet_limit_mode(
        &self,
        api_key_id: &str,
        limit_mode: &str,
    ) -> Result<Option<StoredWalletSnapshot>, DataLayerError> {
        let result = sqlx::query(
            "UPDATE wallets SET limit_mode = $2, updated_at = NOW() WHERE api_key_id = $1",
        )
        .bind(api_key_id)
        .bind(limit_mode)
        .execute(&self.pool)
        .await
        .map_postgres_err()?;
        if result.rows_affected() == 0 {
            return Ok(None);
        }
        self.find(WalletLookupKey::ApiKeyId(api_key_id)).await
    }

    async fn initialize_auth_user_wallet(
        &self,
        user_id: &str,
        initial_gift_usd: f64,
        unlimited: bool,
    ) -> Result<Option<StoredWalletSnapshot>, DataLayerError> {
        initialize_postgres_auth_wallet(
            &self.pool,
            Some(user_id),
            None,
            initial_gift_usd,
            unlimited,
        )
        .await
        .map(|result| result.map(|(wallet, _created)| wallet))
    }

    async fn initialize_auth_user_wallet_with_outcome(
        &self,
        user_id: &str,
        initial_gift_usd: f64,
        unlimited: bool,
    ) -> Result<Option<InitializeAuthWalletOutcome>, DataLayerError> {
        initialize_postgres_auth_wallet(
            &self.pool,
            Some(user_id),
            None,
            initial_gift_usd,
            unlimited,
        )
        .await
        .map(|result| {
            result.map(|(wallet, created)| InitializeAuthWalletOutcome { wallet, created })
        })
    }

    async fn initialize_auth_api_key_wallet(
        &self,
        api_key_id: &str,
        initial_gift_usd: f64,
        unlimited: bool,
    ) -> Result<Option<StoredWalletSnapshot>, DataLayerError> {
        initialize_postgres_auth_wallet(
            &self.pool,
            None,
            Some(api_key_id),
            initial_gift_usd,
            unlimited,
        )
        .await
        .map(|result| result.map(|(wallet, _created)| wallet))
    }

    async fn initialize_auth_api_key_wallet_with_outcome(
        &self,
        api_key_id: &str,
        initial_gift_usd: f64,
        unlimited: bool,
    ) -> Result<Option<InitializeAuthWalletOutcome>, DataLayerError> {
        initialize_postgres_auth_wallet(
            &self.pool,
            None,
            Some(api_key_id),
            initial_gift_usd,
            unlimited,
        )
        .await
        .map(|result| {
            result.map(|(wallet, created)| InitializeAuthWalletOutcome { wallet, created })
        })
    }

    async fn update_auth_user_wallet_snapshot(
        &self,
        user_id: &str,
        balance: f64,
        gift_balance: f64,
        limit_mode: &str,
        currency: &str,
        status: &str,
        total_recharged: f64,
        total_consumed: f64,
        total_refunded: f64,
        total_adjusted: f64,
        updated_at_unix_secs: Option<u64>,
    ) -> Result<Option<StoredWalletSnapshot>, DataLayerError> {
        update_postgres_wallet_snapshot(
            &self.pool,
            "user_id",
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
            updated_at_unix_secs,
        )
        .await?;
        self.find(WalletLookupKey::UserId(user_id)).await
    }

    async fn update_auth_api_key_wallet_snapshot(
        &self,
        api_key_id: &str,
        balance: f64,
        gift_balance: f64,
        limit_mode: &str,
        currency: &str,
        status: &str,
        total_recharged: f64,
        total_consumed: f64,
        total_refunded: f64,
        total_adjusted: f64,
        updated_at_unix_secs: Option<u64>,
    ) -> Result<Option<StoredWalletSnapshot>, DataLayerError> {
        update_postgres_wallet_snapshot(
            &self.pool,
            "api_key_id",
            api_key_id,
            balance,
            gift_balance,
            limit_mode,
            currency,
            status,
            total_recharged,
            total_consumed,
            total_refunded,
            total_adjusted,
            updated_at_unix_secs,
        )
        .await?;
        self.find(WalletLookupKey::ApiKeyId(api_key_id)).await
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

    async fn count_pending_refunds_by_user_id(&self, user_id: &str) -> Result<u64, DataLayerError> {
        let statuses = vec![
            "pending_approval".to_string(),
            "approved".to_string(),
            "processing".to_string(),
        ];
        read_count(
            sqlx::query(COUNT_PENDING_REFUNDS_BY_USER_SQL)
                .bind(user_id)
                .bind(statuses)
                .fetch_one(&self.pool)
                .await
                .map_postgres_err()?,
        )
    }

    async fn count_pending_payment_orders_by_user_id(
        &self,
        user_id: &str,
    ) -> Result<u64, DataLayerError> {
        let statuses = vec!["pending".to_string(), "paid".to_string()];
        read_count(
            sqlx::query(COUNT_PENDING_PAYMENT_ORDERS_BY_USER_SQL)
                .bind(user_id)
                .bind(statuses)
                .fetch_one(&self.pool)
                .await
                .map_postgres_err()?,
        )
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

    async fn find_wallet_recharge_order_by_order_no(
        &self,
        user_id: &str,
        order_no: &str,
    ) -> Result<Option<StoredAdminPaymentOrder>, DataLayerError> {
        let row = sqlx::query(FIND_WALLET_RECHARGE_ORDER_BY_ORDER_NO_SQL)
            .bind(user_id)
            .bind(order_no)
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?;
        row.as_ref().map(map_admin_payment_order_row).transpose()
    }

    async fn find_pending_plan_purchase_order_by_user_id(
        &self,
        user_id: &str,
        product_id: &str,
    ) -> Result<Option<StoredAdminPaymentOrder>, DataLayerError> {
        let row = sqlx::query(FIND_PENDING_PLAN_PURCHASE_ORDER_BY_USER_SQL)
            .bind(user_id)
            .bind(product_id)
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?;
        row.as_ref().map(map_admin_payment_order_row).transpose()
    }

    async fn find_payment_order_by_order_no(
        &self,
        order_no: &str,
    ) -> Result<Option<StoredAdminPaymentOrder>, DataLayerError> {
        let row = sqlx::query(FIND_PAYMENT_ORDER_BY_ORDER_NO_SQL)
            .bind(order_no)
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
    async fn delete_wallet_if_unreferenced(
        &self,
        wallet_id: &str,
        owner: WalletLookupKey<'_>,
    ) -> Result<bool, DataLayerError> {
        if wallet_id.trim().is_empty() {
            return Ok(false);
        }
        let (owner_clause, owner_id) = match owner {
            WalletLookupKey::UserId(user_id) if !user_id.trim().is_empty() => {
                ("user_id = $2 AND api_key_id IS NULL", user_id.to_string())
            }
            WalletLookupKey::ApiKeyId(api_key_id) if !api_key_id.trim().is_empty() => (
                "api_key_id = $2 AND user_id IS NULL",
                api_key_id.to_string(),
            ),
            WalletLookupKey::WalletId(_) => {
                return Err(DataLayerError::InvalidInput(
                    "wallet compensation requires an explicit user or API-key owner".to_string(),
                ))
            }
            _ => return Ok(false),
        };
        let wallet_id = wallet_id.to_string();
        self.tx_runner
            .run_read_write(|tx| {
                let owner_clause = owner_clause.to_string();
                let owner_id = owner_id.clone();
                let wallet_id = wallet_id.clone();
                Box::pin(async move {
                    let select_sql = format!(
                        r#"
SELECT id
FROM wallets
WHERE id = $1
  AND {owner_clause}
  AND balance = 0
  AND gift_balance = 0
  AND total_recharged = 0
  AND total_consumed = 0
  AND total_refunded = 0
  AND total_adjusted = 0
  AND limit_mode IN ('finite', 'unlimited')
  AND currency = 'USD'
  AND status = 'active'
  AND NOT EXISTS (SELECT 1 FROM payment_orders p WHERE p.wallet_id = wallets.id)
  AND NOT EXISTS (SELECT 1 FROM refund_requests r WHERE r.wallet_id = wallets.id)
  AND NOT EXISTS (
      SELECT 1 FROM wallet_daily_usage_ledgers d WHERE d.wallet_id = wallets.id
  )
  AND NOT EXISTS (SELECT 1 FROM usage u WHERE u.wallet_id = wallets.id)
  AND NOT EXISTS (
      SELECT 1 FROM usage_settlement_snapshots s WHERE s.wallet_id = wallets.id
  )
  AND NOT EXISTS (SELECT 1 FROM wallet_transactions t WHERE t.wallet_id = wallets.id)
  AND NOT EXISTS (
      SELECT 1 FROM redeem_codes c WHERE c.redeemed_wallet_id = wallets.id
  )
LIMIT 1
FOR UPDATE
                        "#
                    );
                    let found = sqlx::query_scalar::<_, String>(&select_sql)
                        .bind(&wallet_id)
                        .bind(&owner_id)
                        .fetch_optional(&mut **tx)
                        .await
                        .map_postgres_err()?;
                    let Some(found_id) = found else {
                        return Ok(false);
                    };
                    let removed = sqlx::query("DELETE FROM wallets WHERE id = $1")
                        .bind(&found_id)
                        .execute(&mut **tx)
                        .await
                        .map_postgres_err()?
                        .rows_affected()
                        > 0;
                    Ok(removed)
                })
            })
            .await
    }

    async fn delete_wallet_if_snapshot_matches_and_unreferenced(
        &self,
        expected: &StoredWalletSnapshot,
        owner: WalletLookupKey<'_>,
    ) -> Result<bool, DataLayerError> {
        if expected.id.trim().is_empty() {
            return Ok(false);
        }
        let (owner_clause, owner_id) = match owner {
            WalletLookupKey::UserId(user_id) if !user_id.trim().is_empty() => {
                ("user_id = $2 AND api_key_id IS NULL", user_id.to_string())
            }
            WalletLookupKey::ApiKeyId(api_key_id) if !api_key_id.trim().is_empty() => (
                "api_key_id = $2 AND user_id IS NULL",
                api_key_id.to_string(),
            ),
            WalletLookupKey::WalletId(_) => {
                return Err(DataLayerError::InvalidInput(
                    "wallet compensation requires an explicit user or API-key owner".to_string(),
                ))
            }
            _ => return Ok(false),
        };
        let expected = expected.clone();
        self.tx_runner
            .run_read_write(|tx| {
                let owner_clause = owner_clause.to_string();
                let owner_id = owner_id.clone();
                Box::pin(async move {
                    let select_sql = format!(
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
  AND {owner_clause}
  AND NOT EXISTS (SELECT 1 FROM payment_orders p WHERE p.wallet_id = wallets.id)
  AND NOT EXISTS (SELECT 1 FROM refund_requests r WHERE r.wallet_id = wallets.id)
  AND NOT EXISTS (
      SELECT 1 FROM wallet_daily_usage_ledgers d WHERE d.wallet_id = wallets.id
  )
  AND NOT EXISTS (SELECT 1 FROM usage u WHERE u.wallet_id = wallets.id)
  AND NOT EXISTS (
      SELECT 1 FROM usage_settlement_snapshots s WHERE s.wallet_id = wallets.id
  )
  AND NOT EXISTS (SELECT 1 FROM wallet_transactions t WHERE t.wallet_id = wallets.id)
  AND NOT EXISTS (
      SELECT 1 FROM redeem_codes c WHERE c.redeemed_wallet_id = wallets.id
  )
LIMIT 1
FOR UPDATE
                        "#
                    );
                    let row = sqlx::query(&select_sql)
                        .bind(&expected.id)
                        .bind(&owner_id)
                        .fetch_optional(&mut **tx)
                        .await
                        .map_postgres_err()?;
                    let Some(row) = row else {
                        return Ok(false);
                    };
                    let current = map_wallet_row(&row)?;
                    if current != expected {
                        return Ok(false);
                    }
                    let removed = sqlx::query("DELETE FROM wallets WHERE id = $1")
                        .bind(&expected.id)
                        .execute(&mut **tx)
                        .await
                        .map_postgres_err()?
                        .rows_affected()
                        > 0;
                    Ok(removed)
                })
            })
            .await
    }

    async fn restore_wallet_if_snapshot_matches(
        &self,
        before: &StoredWalletSnapshot,
        after: &StoredWalletSnapshot,
        owner: WalletLookupKey<'_>,
    ) -> Result<bool, DataLayerError> {
        if before.id.trim().is_empty() || after.id.trim().is_empty() {
            return Ok(false);
        }
        if before.id != after.id {
            return Err(DataLayerError::InvalidInput(
                "wallet restore snapshots must reference the same wallet".to_string(),
            ));
        }
        let (owner_clause, owner_id) = match owner {
            WalletLookupKey::UserId(user_id) if !user_id.trim().is_empty() => {
                ("user_id = $2 AND api_key_id IS NULL", user_id.to_string())
            }
            WalletLookupKey::ApiKeyId(api_key_id) if !api_key_id.trim().is_empty() => (
                "api_key_id = $2 AND user_id IS NULL",
                api_key_id.to_string(),
            ),
            WalletLookupKey::WalletId(_) => {
                return Err(DataLayerError::InvalidInput(
                    "wallet restore requires an explicit user or API-key owner".to_string(),
                ))
            }
            _ => return Ok(false),
        };
        let owner_matches = match owner {
            WalletLookupKey::UserId(user_id) => {
                before.user_id.as_deref() == Some(user_id)
                    && after.user_id.as_deref() == Some(user_id)
                    && before.api_key_id.is_none()
                    && after.api_key_id.is_none()
            }
            WalletLookupKey::ApiKeyId(api_key_id) => {
                before.api_key_id.as_deref() == Some(api_key_id)
                    && after.api_key_id.as_deref() == Some(api_key_id)
                    && before.user_id.is_none()
                    && after.user_id.is_none()
            }
            WalletLookupKey::WalletId(_) => false,
        };
        if !owner_matches {
            return Ok(false);
        }
        let before_updated_at = i64::try_from(before.updated_at_unix_secs).map_err(|_| {
            DataLayerError::InvalidInput(
                "wallet restore timestamp is outside the supported range".to_string(),
            )
        })?;
        let before = before.clone();
        let after = after.clone();
        self.tx_runner
            .run_read_write(|tx| {
                let owner_clause = owner_clause.to_string();
                let owner_id = owner_id.clone();
                let before = before.clone();
                let after = after.clone();
                Box::pin(async move {
                    let select_sql = format!(
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
  AND {owner_clause}
LIMIT 1
FOR UPDATE
                        "#
                    );
                    let row = sqlx::query(&select_sql)
                        .bind(&after.id)
                        .bind(&owner_id)
                        .fetch_optional(&mut **tx)
                        .await
                        .map_postgres_err()?;
                    let Some(row) = row else {
                        return Ok(false);
                    };
                    let current = map_wallet_row(&row)?;
                    if current != after {
                        return Ok(false);
                    }
                    let updated = sqlx::query(
                        r#"
UPDATE wallets
SET balance = $2,
    gift_balance = $3,
    limit_mode = $4,
    currency = $5,
    status = $6,
    total_recharged = $7,
    total_consumed = $8,
    total_refunded = $9,
    total_adjusted = $10,
    updated_at = to_timestamp($11::DOUBLE PRECISION)
WHERE id = $1
"#,
                    )
                    .bind(&before.id)
                    .bind(before.balance)
                    .bind(before.gift_balance)
                    .bind(&before.limit_mode)
                    .bind(&before.currency)
                    .bind(&before.status)
                    .bind(before.total_recharged)
                    .bind(before.total_consumed)
                    .bind(before.total_refunded)
                    .bind(before.total_adjusted)
                    .bind(before_updated_at)
                    .execute(&mut **tx)
                    .await
                    .map_postgres_err()?
                    .rows_affected();
                    Ok(updated > 0)
                })
            })
            .await
    }

    async fn delete_provisional_auth_user_wallet(
        &self,
        wallet_id: &str,
        user_id: &str,
    ) -> Result<bool, DataLayerError> {
        if wallet_id.trim().is_empty() || user_id.trim().is_empty() {
            return Ok(false);
        }
        let wallet_id = wallet_id.to_string();
        let user_id = user_id.to_string();
        self.tx_runner
            .run_read_write(|tx| {
                let wallet_id = wallet_id.clone();
                Box::pin(async move {
                    let found_wallet_id = sqlx::query_scalar::<_, String>(
                        r#"
SELECT w.id
FROM wallets AS w
WHERE w.id = $1
  AND w.user_id = $2
  AND w.api_key_id IS NULL
  AND w.balance = 0
  AND w.gift_balance >= 0
  AND w.total_recharged = 0
  AND w.total_consumed = 0
  AND w.total_refunded = 0
  AND w.total_adjusted = w.gift_balance
  AND w.limit_mode IN ('finite', 'unlimited')
  AND w.currency = 'USD'
  AND w.status = 'active'
  AND NOT EXISTS (SELECT 1 FROM payment_orders p WHERE p.wallet_id = w.id)
  AND NOT EXISTS (SELECT 1 FROM refund_requests r WHERE r.wallet_id = w.id)
  AND NOT EXISTS (
      SELECT 1 FROM wallet_daily_usage_ledgers d WHERE d.wallet_id = w.id
  )
  AND NOT EXISTS (SELECT 1 FROM usage u WHERE u.wallet_id = w.id)
  AND NOT EXISTS (
      SELECT 1 FROM usage_settlement_snapshots s WHERE s.wallet_id = w.id
  )
  AND NOT EXISTS (
      SELECT 1 FROM redeem_codes c WHERE c.redeemed_wallet_id = w.id
  )
  AND (
      (w.gift_balance = 0 AND NOT EXISTS (
          SELECT 1 FROM wallet_transactions t WHERE t.wallet_id = w.id
      ))
      OR
      (w.gift_balance > 0
       AND (SELECT COUNT(*) FROM wallet_transactions t WHERE t.wallet_id = w.id) = 1
       AND EXISTS (
          SELECT 1 FROM wallet_transactions t
          WHERE t.wallet_id = w.id
            AND t.category = 'gift'
            AND t.reason_code = 'gift_initial'
            AND t.amount = w.gift_balance
            AND t.balance_before = 0
            AND t.balance_after = w.gift_balance
            AND t.recharge_balance_before = 0
            AND t.recharge_balance_after = 0
            AND t.gift_balance_before = 0
            AND t.gift_balance_after = w.gift_balance
            AND t.link_type = 'system_task'
            AND t.link_id = w.user_id
            AND t.operator_id IS NULL
       ))
  )
LIMIT 1
FOR UPDATE
                        "#,
                    )
                    .bind(&wallet_id)
                    .bind(&user_id)
                    .fetch_optional(&mut **tx)
                    .await
                    .map_postgres_err()?;
                    let Some(found_wallet_id) = found_wallet_id else {
                        return Ok(false);
                    };
                    sqlx::query("DELETE FROM wallet_transactions WHERE wallet_id = $1")
                        .bind(&found_wallet_id)
                        .execute(&mut **tx)
                        .await
                        .map_postgres_err()?;
                    let removed = sqlx::query("DELETE FROM wallets WHERE id = $1 AND user_id = $2")
                        .bind(&found_wallet_id)
                        .bind(&user_id)
                        .execute(&mut **tx)
                        .await
                        .map_postgres_err()?
                        .rows_affected()
                        > 0;
                    Ok(removed)
                })
            })
            .await
    }

    async fn create_wallet_recharge_order(
        &self,
        mut input: CreateWalletRechargeOrderInput,
    ) -> Result<CreateWalletRechargeOrderOutcome, DataLayerError> {
        input.payment_method = canonicalize_payment_method(&input.payment_method)
            .map_err(DataLayerError::InvalidInput)?;
        if !input.amount_usd.is_finite() || input.amount_usd <= 0.0 {
            return Err(DataLayerError::InvalidInput(
                "manual recharge amount must be finite and positive".to_string(),
            ));
        }
        validate_wallet_recharge_order_input(&input).map_err(DataLayerError::InvalidInput)?;
        if !input.amount_usd.is_finite()
            || input.amount_usd <= 0.0
            || input
                .pay_amount
                .is_some_and(|value| !value.is_finite() || value <= 0.0)
            || input
                .exchange_rate
                .is_some_and(|value| !value.is_finite() || value <= 0.0)
        {
            return Err(DataLayerError::InvalidInput(
                "invalid wallet recharge numeric fields".to_string(),
            ));
        }
        let projected_gateway_response =
            project_wallet_recharge_gateway_response(&input.gateway_response)
                .map_err(DataLayerError::InvalidInput)?;
        self.tx_runner
            .run_read_write(|tx| {
                Box::pin(async move {
                    // `wallets.user_id` is nullable so deleted-user history
                    // can be retained. Check the live owner before any
                    // automatic wallet or payment-order insert.
                    let user_exists: Option<String> =
                        sqlx::query_scalar("SELECT id FROM public.users WHERE id = $1 FOR UPDATE")
                            .bind(&input.user_id)
                            .fetch_optional(&mut **tx)
                            .await
                            .map_postgres_err()?;
                    if user_exists.is_none() {
                        return Err(DataLayerError::InvalidInput("user not found".to_string()));
                    }

                    let (wallet_row, created_wallet) = match sqlx::query(
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
                        Some(row) => (row, false),
                        None => {
                            let wallet_id = input
                                .preferred_wallet_id
                                .clone()
                                .unwrap_or_else(|| Uuid::new_v4().to_string());
                            let inserted_wallet = sqlx::query(
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
ON CONFLICT DO NOTHING
RETURNING id, status
                                "#,
                            )
                            .bind(&wallet_id)
                            .bind(&input.user_id)
                            .fetch_optional(&mut **tx)
                            .await
                            .map_postgres_err()?;
                            match inserted_wallet {
                                Some(row) => (row, true),
                                None => {
                                    let Some(row) = sqlx::query(
                                        "SELECT id, status FROM wallets WHERE user_id = $1 LIMIT 1 FOR UPDATE",
                                    )
                                    .bind(&input.user_id)
                                    .fetch_optional(&mut **tx)
                                    .await
                                    .map_postgres_err()?
                                    else {
                                        return Err(DataLayerError::InvalidInput(
                                            "wallet identifier already belongs to another owner"
                                                .to_string(),
                                        ));
                                    };
                                    (row, false)
                                }
                            }
                        }
                    };
                    let wallet_id: String = row_get(&wallet_row, "id")?;
                    let wallet_status: String = row_get(&wallet_row, "status")?;
                    if wallet_status != "active" {
                        return Ok(CreateWalletRechargeOrderOutcome::WalletInactive);
                    }

                    // `order_no` is globally unique. Inspect the existing row
                    // before INSERT so callers get a deterministic idempotent
                    // replay only for their own wallet-recharge order; a
                    // collision with another user or order kind is invalid.
                    if let Some(existing_row) = sqlx::query(
                        "SELECT id, user_id, order_kind FROM payment_orders WHERE order_no = $1 FOR UPDATE",
                    )
                    .bind(&input.order_no)
                    .fetch_optional(&mut **tx)
                    .await
                    .map_postgres_err()?
                    {
                        let existing_user_id: Option<String> = row_get(&existing_row, "user_id")?;
                        let existing_kind: String = row_get(&existing_row, "order_kind")?;
                        if existing_user_id.as_deref() == Some(input.user_id.as_str())
                            && existing_kind == "wallet_recharge"
                        {
                            let existing = sqlx::query(FIND_ADMIN_PAYMENT_ORDER_SQL)
                                .bind(row_get::<String>(&existing_row, "id")?)
                                .fetch_one(&mut **tx)
                                .await
                                .map_postgres_err()?;
                            if !postgres_wallet_recharge_replay_matches(
                                &existing,
                                &wallet_id,
                                &input,
                            )? {
                                return Err(DataLayerError::InvalidInput(
                                    "wallet recharge replay changes immutable order fields".to_string(),
                                ));
                            }
                            if created_wallet {
                                sqlx::query(DELETE_PROVISIONAL_RECHARGE_WALLET_SQL)
                                    .bind(&wallet_id)
                                    .bind(&input.user_id)
                                    .execute(&mut **tx)
                                    .await
                                    .map_postgres_err()?;
                            }
                            return Ok(CreateWalletRechargeOrderOutcome::Existing(
                                map_admin_payment_order_row(&existing)?,
                            ));
                        }
                        return Err(DataLayerError::InvalidInput(
                            "payment order number already belongs to another order".to_string(),
                        ));
                    }

                    let expires_at = i64::try_from(input.expires_at_unix_secs).map_err(|_| {
                        DataLayerError::InvalidInput(
                            "wallet recharge expires_at overflow".to_string(),
                        )
                    })?;
                    let insert_result = sqlx::query(
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
  payment_provider,
  payment_channel,
  order_kind,
  fulfillment_status,
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
  'wallet_recharge',
  'pending',
  $12,
  $13,
  'pending',
  NOW(),
  to_timestamp($14)
)
ON CONFLICT DO NOTHING
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
  payment_provider,
  payment_channel,
  order_kind,
  product_id,
  product_snapshot,
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
                    .bind(input.payment_provider.as_deref())
                    .bind(input.payment_channel.as_deref())
                    .bind(&input.gateway_order_id)
                    .bind(&projected_gateway_response)
                    .bind(expires_at)
                    .fetch_optional(&mut **tx)
                    .await
                    .map_postgres_err()?;
                    let Some(row) = insert_result else {
                        let existing_row = sqlx::query(
                            "SELECT id, user_id, order_kind FROM payment_orders WHERE order_no = $1 FOR UPDATE",
                        )
                        .bind(&input.order_no)
                        .fetch_optional(&mut **tx)
                        .await
                        .map_postgres_err()?;
                        if let Some(existing_row) = existing_row {
                            let existing_user_id: Option<String> =
                                row_get(&existing_row, "user_id")?;
                            let existing_kind: String = row_get(&existing_row, "order_kind")?;
                            let existing = sqlx::query(FIND_ADMIN_PAYMENT_ORDER_SQL)
                                .bind(row_get::<String>(&existing_row, "id")?)
                                .fetch_one(&mut **tx)
                                .await
                                .map_postgres_err()?;
                            if existing_user_id.as_deref() == Some(input.user_id.as_str())
                                && existing_kind == "wallet_recharge"
                            {
                                if !postgres_wallet_recharge_replay_matches(
                                    &existing,
                                    &wallet_id,
                                    &input,
                                )? {
                                    return Err(DataLayerError::InvalidInput(
                                        "wallet recharge replay changes immutable order fields".to_string(),
                                    ));
                                }
                                if created_wallet {
                                    sqlx::query(DELETE_PROVISIONAL_RECHARGE_WALLET_SQL)
                                        .bind(&wallet_id)
                                        .bind(&input.user_id)
                                        .execute(&mut **tx)
                                        .await
                                        .map_postgres_err()?;
                                }
                                return Ok(CreateWalletRechargeOrderOutcome::Existing(
                                    map_admin_payment_order_row(&existing)?,
                                ));
                            }
                            return Err(DataLayerError::InvalidInput(
                                "payment order number already belongs to another order".to_string(),
                            ));
                        }
                        let gateway_conflict = sqlx::query(
                            "SELECT 1 FROM payment_orders WHERE payment_method = $1 AND gateway_order_id = $2 LIMIT 1",
                        )
                        .bind(&input.payment_method)
                        .bind(&input.gateway_order_id)
                        .fetch_optional(&mut **tx)
                        .await
                        .map_postgres_err()?;
                        if gateway_conflict.is_some() {
                            return Err(DataLayerError::InvalidInput(
                                "payment gateway order already belongs to another order".to_string(),
                            ));
                        }
                        return Err(DataLayerError::InvalidInput(
                            "wallet recharge order could not be created".to_string(),
                        ));
                    };
                    Ok(CreateWalletRechargeOrderOutcome::Created(
                        map_admin_payment_order_row(&row)?,
                    ))
                })
            })
            .await
    }

    async fn update_wallet_recharge_checkout(
        &self,
        input: UpdateWalletRechargeCheckoutInput,
    ) -> Result<WalletMutationOutcome<StoredAdminPaymentOrder>, DataLayerError> {
        if input.order_id.trim().is_empty() || input.gateway_order_id.trim().is_empty() {
            return Ok(WalletMutationOutcome::Invalid(
                "wallet recharge checkout identifiers are required".to_string(),
            ));
        }
        let projected_gateway_response =
            match project_wallet_recharge_gateway_response(&input.gateway_response) {
                Ok(value) => value,
                Err(error) => return Ok(WalletMutationOutcome::Invalid(error)),
            };
        self.tx_runner
            .run_read_write(|tx| {
                Box::pin(async move {
                    let Some(current_row) = sqlx::query(
                        "SELECT id, order_no, payment_method, payment_provider, payment_channel, order_kind, gateway_order_id, gateway_response, status, COALESCE(expires_at > NOW(), FALSE) AS checkout_live FROM payment_orders WHERE id = $1 FOR UPDATE",
                    )
                    .bind(&input.order_id)
                    .fetch_optional(&mut **tx)
                    .await
                    .map_postgres_err()?
                    else {
                        return Ok(WalletMutationOutcome::NotFound);
                    };
                    let order_kind: Option<String> = row_get(&current_row, "order_kind")?;
                    if order_kind.as_deref() != Some("wallet_recharge") {
                        return Ok(WalletMutationOutcome::Invalid(
                            "payment order is not a wallet recharge".to_string(),
                        ));
                    }
                    let current_gateway_response: Option<serde_json::Value> =
                        row_get(&current_row, "gateway_response")?;
                    let current_is_checkout_placeholder = current_gateway_response
                        .as_ref()
                        .is_some_and(wallet_recharge_response_is_checkout_placeholder);
                    let current_token = current_gateway_response
                        .as_ref()
                        .and_then(wallet_recharge_checkout_claim_token);
                    let requested_token =
                        wallet_recharge_checkout_claim_token(&projected_gateway_response);
                    if current_token.is_some() && current_token != requested_token {
                        return Ok(WalletMutationOutcome::Invalid(
                            "wallet recharge checkout claim is no longer current".to_string(),
                        ));
                    }
                    let current_status: String = row_get(&current_row, "status")?;
                    let current_gateway: Option<String> = row_get(&current_row, "gateway_order_id")?;
                    if current_status != "pending" {
                        if current_gateway.as_deref() == Some(input.gateway_order_id.as_str()) {
                            let row = sqlx::query(FIND_ADMIN_PAYMENT_ORDER_SQL)
                                .bind(&input.order_id)
                                .fetch_one(&mut **tx)
                                .await
                                .map_postgres_err()?;
                            return Ok(WalletMutationOutcome::Applied(
                                map_admin_payment_order_row(&row)?,
                            ));
                        }
                        return Ok(WalletMutationOutcome::Invalid(
                            "wallet recharge order is no longer pending".to_string(),
                        ));
                    }
                    let checkout_live: bool = row_get(&current_row, "checkout_live")?;
                    if !checkout_live {
                        return Ok(WalletMutationOutcome::Invalid(
                            "wallet recharge order is expired".to_string(),
                        ));
                    }
                    let order_no: String = row_get(&current_row, "order_no")?;
                    if current_gateway.as_deref().is_some_and(|existing| {
                        existing != input.gateway_order_id.as_str()
                            && existing != order_no.as_str()
                            && !current_is_checkout_placeholder
                    }) {
                        return Ok(WalletMutationOutcome::Invalid(
                            "wallet recharge checkout is already bound".to_string(),
                        ));
                    }
                    let payment_method: String = row_get(&current_row, "payment_method")?;
                    let conflict = sqlx::query(
                        "SELECT id FROM payment_orders WHERE payment_method = $1 AND gateway_order_id = $2 AND id <> $3 LIMIT 1",
                    )
                    .bind(&payment_method)
                    .bind(&input.gateway_order_id)
                    .bind(&input.order_id)
                    .fetch_optional(&mut **tx)
                    .await
                    .map_postgres_err()?;
                    if conflict.is_some() {
                        return Ok(WalletMutationOutcome::Invalid(
                            "payment gateway order already belongs to another order".to_string(),
                        ));
                    }
                    let updated = sqlx::query(
                        "UPDATE payment_orders SET gateway_order_id = $2, gateway_response = $3 WHERE id = $1 AND status = 'pending' AND expires_at > NOW()",
                    )
                    .bind(&input.order_id)
                    .bind(&input.gateway_order_id)
                    .bind(&projected_gateway_response)
                    .execute(&mut **tx)
                    .await
                    .map_postgres_err()?;
                    if updated.rows_affected() == 0 {
                        return Ok(WalletMutationOutcome::Invalid(
                            "wallet recharge order is expired or no longer pending".to_string(),
                        ));
                    }
                    let row = sqlx::query(FIND_ADMIN_PAYMENT_ORDER_SQL)
                        .bind(&input.order_id)
                        .fetch_one(&mut **tx)
                        .await
                        .map_postgres_err()?;
                    Ok(WalletMutationOutcome::Applied(map_admin_payment_order_row(&row)?))
                })
            })
            .await
    }

    async fn compare_and_swap_payment_order_stripe_client_secret(
        &self,
        input: CompareAndSwapPaymentOrderStripeClientSecretInput,
    ) -> Result<bool, DataLayerError> {
        self.tx_runner
            .run_read_write(|tx| {
                Box::pin(async move {
                    let locked: Option<String> = sqlx::query_scalar(
                        "SELECT id FROM payment_orders WHERE id = $1 FOR UPDATE",
                    )
                    .bind(&input.order_id)
                    .fetch_optional(&mut **tx)
                    .await
                    .map_postgres_err()?;
                    if locked.is_none() {
                        return Ok(false);
                    }
                    let row = sqlx::query(FIND_ADMIN_PAYMENT_ORDER_SQL)
                        .bind(&input.order_id)
                        .fetch_one(&mut **tx)
                        .await
                        .map_postgres_err()?;
                    let current = map_admin_payment_order_row(&row)?;
                    let Some(replacement) =
                        payment_order_stripe_client_secret_cas_replacement(&current, &input)
                            .map_err(DataLayerError::InvalidInput)?
                    else {
                        return Ok(false);
                    };
                    let updated = sqlx::query(
                        "UPDATE payment_orders SET gateway_response = $2 WHERE id = $1",
                    )
                    .bind(&input.order_id)
                    .bind(replacement)
                    .execute(&mut **tx)
                    .await
                    .map_postgres_err()?;
                    Ok(updated.rows_affected() == 1)
                })
            })
            .await
    }

    async fn fail_wallet_recharge_checkout(
        &self,
        input: FailWalletRechargeCheckoutInput,
    ) -> Result<WalletMutationOutcome<StoredAdminPaymentOrder>, DataLayerError> {
        if input.order_id.trim().is_empty()
            || input.claim_token.trim().is_empty()
            || input.claim_token.len() > 128
        {
            return Ok(WalletMutationOutcome::Invalid(
                "wallet recharge checkout failure identifiers are required".to_string(),
            ));
        }
        let order_id = input.order_id.clone();
        self.tx_runner
            .run_read_write(|tx| {
                Box::pin(async move {
                    let Some(row) = sqlx::query(
                        "SELECT id, order_kind, gateway_response, status FROM payment_orders WHERE id = $1 FOR UPDATE",
                    )
                    .bind(&order_id)
                    .fetch_optional(&mut **tx)
                    .await
                    .map_postgres_err()?
                    else {
                        return Ok(WalletMutationOutcome::NotFound);
                    };
                    let order_kind: Option<String> = row_get(&row, "order_kind")?;
                    let gateway_response: Option<serde_json::Value> =
                        row_get(&row, "gateway_response")?;
                    let status: String = row_get(&row, "status")?;
                    if order_kind.as_deref() != Some("wallet_recharge")
                        || !gateway_response
                            .as_ref()
                            .is_some_and(wallet_recharge_response_is_checkout_placeholder)
                    {
                        return Ok(WalletMutationOutcome::Invalid(
                            "payment order is not a checkout placeholder".to_string(),
                        ));
                    }
                    let current_token = gateway_response
                        .as_ref()
                        .and_then(wallet_recharge_checkout_claim_token);
                    if current_token != Some(input.claim_token.trim()) {
                        return Ok(WalletMutationOutcome::Invalid(
                            "wallet recharge checkout claim is no longer current".to_string(),
                        ));
                    }
                    let full_row = sqlx::query(FIND_ADMIN_PAYMENT_ORDER_SQL)
                        .bind(&order_id)
                        .fetch_one(&mut **tx)
                        .await
                        .map_postgres_err()?;
                    let order = map_admin_payment_order_row(&full_row)?;
                    if status != "pending" {
                        return Ok(WalletMutationOutcome::Applied(order));
                    }
                    let failed = if input.provider_request_may_have_succeeded {
                        wallet_recharge_checkout_uncertain_response(
                            gateway_response.as_ref(),
                            &input.reason,
                            Utc::now().timestamp().max(0) as u64,
                        )
                    } else {
                        wallet_recharge_checkout_failed_response(
                            gateway_response.as_ref(),
                            &input.reason,
                            Utc::now().timestamp().max(0) as u64,
                        )
                    };
                    let updated = sqlx::query(
                        "UPDATE payment_orders SET status = 'failed', gateway_response = $2 WHERE id = $1 AND status = 'pending'",
                    )
                    .bind(&order_id)
                    .bind(failed)
                    .execute(&mut **tx)
                    .await
                    .map_postgres_err()?;
                    if updated.rows_affected() == 0 {
                        return Ok(WalletMutationOutcome::Invalid(
                            "wallet recharge checkout claim is no longer current".to_string(),
                        ));
                    }
                    let row = sqlx::query(FIND_ADMIN_PAYMENT_ORDER_SQL)
                        .bind(&order_id)
                        .fetch_one(&mut **tx)
                        .await
                        .map_postgres_err()?;
                    Ok(WalletMutationOutcome::Applied(map_admin_payment_order_row(&row)?))
                })
            })
            .await
    }

    async fn reclaim_wallet_recharge_checkout(
        &self,
        input: ReclaimWalletRechargeCheckoutInput,
    ) -> Result<WalletMutationOutcome<StoredAdminPaymentOrder>, DataLayerError> {
        let now = Utc::now().timestamp().max(0) as u64;
        if input.order_id.trim().is_empty()
            || input.claim_token.trim().is_empty()
            || input.claim_token.len() > 128
            || input.expires_at_unix_secs <= now
        {
            return Ok(WalletMutationOutcome::Invalid(
                "wallet recharge checkout reclaim identifiers are invalid".to_string(),
            ));
        }
        if !wallet_recharge_response_is_checkout_placeholder(&input.gateway_response) {
            return Ok(WalletMutationOutcome::Invalid(
                "wallet recharge reclaim response must be a placeholder".to_string(),
            ));
        }
        let response = wallet_recharge_checkout_claim_response(
            &input.gateway_response,
            &input.claim_token,
            now,
        )
        .map_err(DataLayerError::InvalidInput)?;
        let order_id = input.order_id.clone();
        self.tx_runner
            .run_read_write(|tx| {
                Box::pin(async move {
                    let Some(row) = sqlx::query(
                        "SELECT id, order_no, order_kind, gateway_response, status, CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_ms, CAST(EXTRACT(EPOCH FROM expires_at) AS BIGINT) AS expires_at_unix_secs FROM payment_orders WHERE id = $1 FOR UPDATE",
                    )
                    .bind(&order_id)
                    .fetch_optional(&mut **tx)
                    .await
                    .map_postgres_err()?
                    else {
                        return Ok(WalletMutationOutcome::NotFound);
                    };
                    let order_kind: Option<String> = row_get(&row, "order_kind")?;
                    let full_row = sqlx::query(FIND_ADMIN_PAYMENT_ORDER_SQL)
                        .bind(&order_id)
                        .fetch_one(&mut **tx)
                        .await
                        .map_postgres_err()?;
                    let order = map_admin_payment_order_row(&full_row)?;
                    if order_kind.as_deref() != Some("wallet_recharge")
                        || !wallet_recharge_order_is_reclaimable_placeholder(&order, now)
                    {
                        return Ok(WalletMutationOutcome::Invalid(
                            "wallet recharge checkout is still in progress or already completed".to_string(),
                        ));
                    }
                    let updated = sqlx::query(
                        "UPDATE payment_orders SET gateway_order_id = $2, gateway_response = $3, status = 'pending', expires_at = to_timestamp($4) WHERE id = $1",
                    )
                    .bind(&order_id)
                    .bind(&order.order_no)
                    .bind(response)
                    .bind(i64::try_from(input.expires_at_unix_secs).map_err(|_| {
                        DataLayerError::InvalidInput("wallet recharge expires_at overflow".to_string())
                    })?)
                    .execute(&mut **tx)
                    .await
                    .map_postgres_err()?;
                    if updated.rows_affected() == 0 {
                        return Ok(WalletMutationOutcome::Invalid(
                            "wallet recharge checkout reclaim lost the order race".to_string(),
                        ));
                    }
                    let row = sqlx::query(FIND_ADMIN_PAYMENT_ORDER_SQL)
                        .bind(&order_id)
                        .fetch_one(&mut **tx)
                        .await
                        .map_postgres_err()?;
                    Ok(WalletMutationOutcome::Applied(map_admin_payment_order_row(&row)?))
                })
            })
            .await
    }

    async fn create_plan_purchase_order(
        &self,
        mut input: CreatePlanPurchaseOrderInput,
    ) -> Result<CreatePlanPurchaseOrderOutcome, DataLayerError> {
        input.payment_method = canonicalize_payment_method(&input.payment_method)
            .map_err(DataLayerError::InvalidInput)?;
        validate_plan_purchase_order_input(&input).map_err(DataLayerError::InvalidInput)?;
        let projected_gateway_response = project_wallet_gateway_response(&input.gateway_response)
            .map_err(DataLayerError::InvalidInput)?;
        self.tx_runner
            .run_read_write(|tx| {
                Box::pin(async move {
                    // Validate and lock the order owner before the automatic
                    // wallet upsert. This keeps a missing-user checkout from
                    // creating an orphan financial row.
                    let user_exists: Option<String> =
                        sqlx::query_scalar("SELECT id FROM users WHERE id = $1 FOR UPDATE")
                            .bind(&input.user_id)
                            .fetch_optional(&mut **tx)
                            .await
                            .map_postgres_err()?;
                    if user_exists.is_none() {
                        return Err(DataLayerError::InvalidInput("user not found".to_string()));
                    }

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
                            let inserted_wallet = sqlx::query(
                                r#"
INSERT INTO wallets (
  id, user_id, balance, gift_balance, limit_mode, currency, status,
  total_recharged, total_consumed, total_refunded, total_adjusted,
  created_at, updated_at
)
VALUES ($1, $2, 0, 0, 'finite', 'USD', 'active', 0, 0, 0, 0, NOW(), NOW())
ON CONFLICT DO NOTHING
RETURNING id, status
                                "#,
                            )
                            .bind(&wallet_id)
                            .bind(&input.user_id)
                            .fetch_optional(&mut **tx)
                            .await
                            .map_postgres_err()?;
                            match inserted_wallet {
                                Some(row) => row,
                                None => {
                                    let Some(row) = sqlx::query(
                                        "SELECT id, status FROM wallets WHERE user_id = $1 LIMIT 1 FOR UPDATE",
                                    )
                                    .bind(&input.user_id)
                                    .fetch_optional(&mut **tx)
                                    .await
                                    .map_postgres_err()?
                                    else {
                                        return Err(DataLayerError::InvalidInput(
                                            "wallet identifier already belongs to another owner"
                                                .to_string(),
                                        ));
                                    };
                                    row
                                }
                            }
                        }
                    };
                    let wallet_id: String = row_get(&wallet_row, "id")?;
                    let wallet_status: String = row_get(&wallet_row, "status")?;
                    if wallet_status != "active" {
                        return Ok(CreatePlanPurchaseOrderOutcome::WalletInactive);
                    }

                    let purchase_limit_scope = plan_purchase_limit_scope(&input.product_snapshot);
                    if purchase_limit_scope != "unlimited" {
                        let max_active_per_user = plan_max_active_per_user(&input.product_snapshot);
                        let mut active_count = if purchase_limit_scope == "lifetime" {
                            sqlx::query_scalar::<_, i64>(
                                r#"
SELECT COUNT(*)::bigint
FROM user_plan_entitlements
WHERE user_id = $1
  AND plan_id = $2
  AND status = 'active'
                        "#,
                            )
                            .bind(&input.user_id)
                            .bind(&input.product_id)
                            .fetch_one(&mut **tx)
                            .await
                            .map_postgres_err()?
                        } else {
                            sqlx::query_scalar::<_, i64>(
                                r#"
SELECT COUNT(*)::bigint
FROM user_plan_entitlements
WHERE user_id = $1
  AND plan_id = $2
  AND status = 'active'
  AND expires_at > NOW()
                        "#,
                            )
                            .bind(&input.user_id)
                            .bind(&input.product_id)
                            .fetch_one(&mut **tx)
                            .await
                            .map_postgres_err()?
                        };
                        active_count += sqlx::query_scalar::<_, i64>(
                            r#"
	SELECT COUNT(*)::bigint
	FROM payment_orders
	WHERE user_id = $1
	  AND product_id = $2
	  AND order_kind = 'plan_purchase'
	  AND status = 'pending'
	  AND expires_at > NOW()
	                        "#,
                        )
                        .bind(&input.user_id)
                        .bind(&input.product_id)
                        .fetch_one(&mut **tx)
                        .await
                        .map_postgres_err()?;
                        if active_count >= max_active_per_user {
                            return Ok(CreatePlanPurchaseOrderOutcome::ActivePlanLimitReached);
                        }
                    }

                    let expires_at = i64::try_from(input.expires_at_unix_secs).map_err(|_| {
                        DataLayerError::InvalidInput(
                            "plan purchase expires_at overflow".to_string(),
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
  payment_provider,
  payment_channel,
  order_kind,
  product_id,
  product_snapshot,
  fulfillment_status,
  gateway_order_id,
  gateway_response,
  status,
  created_at,
  expires_at
)
VALUES (
  $1, $2, $3, $4, $5, $6, $7, $8, 0, 0, $9, $10, $11,
  'plan_purchase', $12, $13, 'pending', $14, $15, 'pending', NOW(),
  to_timestamp($16)
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
  payment_provider,
  payment_channel,
  order_kind,
  product_id,
  product_snapshot,
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
                    .bind(&input.pay_currency)
                    .bind(input.exchange_rate)
                    .bind(&input.payment_method)
                    .bind(input.payment_provider.as_deref())
                    .bind(input.payment_channel.as_deref())
                    .bind(&input.product_id)
                    .bind(&input.product_snapshot)
                    .bind(&input.gateway_order_id)
                    .bind(&projected_gateway_response)
                    .bind(expires_at)
                    .fetch_one(&mut **tx)
                    .await
                    .map_postgres_err()?;
                    Ok(CreatePlanPurchaseOrderOutcome::Created(
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
        if !input.amount_usd.is_finite() || input.amount_usd <= 0.0 {
            return Ok(CreateWalletRefundRequestOutcome::InvalidInput(
                "refund amount must be finite and greater than zero".to_string(),
            ));
        }
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
  AND user_id = $2
LIMIT 1
FOR UPDATE
                        "#,
                    )
                    .bind(&input.wallet_id)
                    .bind(&input.user_id)
                    .fetch_optional(&mut **tx)
                    .await
                    .map_postgres_err()?
                    else {
                        return Ok(CreateWalletRefundRequestOutcome::WalletMissing);
                    };
                    let wallet_recharge_balance: f64 = row_get(&locked_wallet_row, "balance")?;
                    if !wallet_recharge_balance.is_finite() {
                        return Ok(CreateWalletRefundRequestOutcome::InvalidInput(
                            "wallet recharge balance is invalid".to_string(),
                        ));
                    }
                    let wallet_reserved_amount = sqlx::query_scalar::<_, Option<f64>>(
                        r#"
SELECT CAST(amount_usd AS DOUBLE PRECISION)
FROM refund_requests
WHERE wallet_id = $1
  AND status IN ('pending_approval', 'approved')
                        "#,
                    )
                    .bind(&input.wallet_id)
                    .fetch_all(&mut **tx)
                    .await
                    .map_postgres_err()?
                    .into_iter()
                    .try_fold(0.0_f64, |total, amount| {
                        let amount = amount?;
                        if !amount.is_finite() || amount <= 0.0 {
                            return None;
                        }
                        let next = total + amount;
                        next.is_finite().then_some(next)
                    });
                    let Some(wallet_reserved_amount) = wallet_reserved_amount else {
                        return Ok(CreateWalletRefundRequestOutcome::InvalidInput(
                            "wallet refund reservation is invalid".to_string(),
                        ));
                    };
                    if input.amount_usd > (wallet_recharge_balance - wallet_reserved_amount) {
                        return Ok(
                            CreateWalletRefundRequestOutcome::RefundAmountExceedsAvailableBalance,
                        );
                    }

                    let mut payment_order_id = None;
                    let mut resolved_payment_method = None;
                    if let Some(order_id) = input.payment_order_id.as_deref() {
                        let Some(order_row) = sqlx::query(
                            r#"
SELECT
  id,
  status,
  payment_method,
  CAST(amount_usd AS DOUBLE PRECISION) AS amount_usd,
  CAST(refunded_amount_usd AS DOUBLE PRECISION) AS refunded_amount_usd,
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
                        let reserved_amount = sqlx::query_scalar::<_, Option<f64>>(
                            r#"
SELECT CAST(amount_usd AS DOUBLE PRECISION)
FROM refund_requests
WHERE payment_order_id = $1
  AND status IN ('pending_approval', 'approved')
                            "#,
                        )
                        .bind(order_id)
                        .fetch_all(&mut **tx)
                        .await
                        .map_postgres_err()?
                        .into_iter()
                        .try_fold(0.0_f64, |total, amount| {
                            let amount = amount?;
                            if !amount.is_finite() || amount <= 0.0 {
                                return None;
                            }
                            let next = total + amount;
                            next.is_finite().then_some(next)
                        });
                        let Some(reserved_amount) = reserved_amount else {
                            return Ok(CreateWalletRefundRequestOutcome::InvalidInput(
                                "payment order refund reservation is invalid".to_string(),
                            ));
                        };
                        let order_amount: f64 = row_get(&order_row, "amount_usd")?;
                        let refunded_amount: f64 =
                            row_get(&order_row, "refunded_amount_usd")?;
                        let refundable_amount: f64 =
                            row_get(&order_row, "refundable_amount_usd")?;
                        if !payment_order_refund_amounts_are_consistent(
                            order_amount,
                            refunded_amount,
                            refundable_amount,
                        ) {
                            return Ok(CreateWalletRefundRequestOutcome::InvalidInput(
                                "payment order refund amounts are invalid".to_string(),
                            ));
                        }
                        if input.amount_usd > (refundable_amount - reserved_amount) {
                            return Ok(
                                CreateWalletRefundRequestOutcome::RefundAmountExceedsAvailableOrderAmount,
                            );
                        }
                        payment_order_id = Some(order_id.to_string());
                        resolved_payment_method = Some(row_get::<String>(&order_row, "payment_method")?);
                    }

                    let canonical = canonicalize_wallet_refund_fields(
                        payment_order_id.as_deref(),
                        input.source_type.as_deref(),
                        input.source_id.as_deref(),
                        input.refund_mode.as_deref(),
                        resolved_payment_method.as_deref(),
                    )
                    .map_err(DataLayerError::InvalidInput)?;
                    let source_type = canonical.source_type;
                    let source_id = canonical.source_id;
                    let refund_mode = canonical.refund_mode;

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
ON CONFLICT (idempotency_key) DO NOTHING
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
                    .fetch_optional(&mut **tx)
                    .await
                    .map_postgres_err()?;
                    match insert_result {
                        Some(row) => Ok(CreateWalletRefundRequestOutcome::Created(
                            map_admin_wallet_refund_row(&row)?,
                        )),
                        None => {
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
                    }
                })
            })
            .await
    }

    async fn process_payment_callback(
        &self,
        mut input: ProcessPaymentCallbackInput,
    ) -> Result<ProcessPaymentCallbackOutcome, DataLayerError> {
        input
            .canonicalize_and_validate()
            .map_err(DataLayerError::InvalidInput)?;
        if input.callback_key.trim().is_empty()
            || input.callback_key.chars().count() > 128
            || input.payload_hash.trim().is_empty()
            || !input.amount_usd.is_finite()
            || input.amount_usd <= 0.0
            || input
                .pay_amount
                .is_some_and(|value| !value.is_finite() || value <= 0.0)
            || input
                .exchange_rate
                .is_some_and(|value| !value.is_finite() || value <= 0.0)
        {
            return Err(DataLayerError::InvalidInput(
                "invalid payment callback numeric or identity fields".to_string(),
            ));
        }
        self.tx_runner
            .run_read_write(|tx| {
                Box::pin(async move {
                    let candidate_callback_id = Uuid::new_v4().to_string();
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
  NULL,
  NULL,
  NOW(),
  NULL
)
ON CONFLICT (callback_key) DO NOTHING
                        "#,
                    )
                    .bind(&candidate_callback_id)
                    .bind(&input.payment_method)
                    .bind(&input.callback_key)
                    .bind(input.order_no.as_deref())
                    .bind(input.gateway_order_id.as_deref())
                    .bind(&input.payload_hash)
                    .bind(input.signature_valid)
                    .execute(&mut **tx)
                    .await
                    .map_postgres_err()?;

                    let callback_row = sqlx::query(
                        r#"
SELECT id, payment_order_id, payment_method, payload_hash, status, order_no, gateway_order_id
FROM payment_callbacks
WHERE callback_key = $1
LIMIT 1
FOR UPDATE
                        "#,
                    )
                    .bind(&input.callback_key)
                    .fetch_one(&mut **tx)
                    .await
                    .map_postgres_err()?;
                    let callback_id: String = row_get(&callback_row, "id")?;
                    let duplicate = callback_id != candidate_callback_id;
                    let callback_order_no: Option<String> = row_get(&callback_row, "order_no")?;
                    let callback_gateway_order_id: Option<String> =
                        row_get(&callback_row, "gateway_order_id")?;
                    let stored_method: String = row_get(&callback_row, "payment_method")?;
                    let stored_hash: Option<String> = row_get(&callback_row, "payload_hash")?;
                    if !stored_method.eq_ignore_ascii_case(&input.payment_method)
                        || stored_hash.as_deref() != Some(input.payload_hash.as_str())
                    {
                        return Ok(ProcessPaymentCallbackOutcome::Failed {
                            duplicate: true,
                            error: "callback key reused with different payment payload".to_string(),
                        });
                    }
                    let status: String = row_get(&callback_row, "status")?;
                    if status == "processed" {
                        return Ok(ProcessPaymentCallbackOutcome::DuplicateProcessed {
                            order_id: row_get(&callback_row, "payment_order_id")?,
                        });
                    }

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

                    let lookup_order_no =
                        input.order_no.clone().or_else(|| callback_order_no.clone());
                    let lookup_gateway_order_id = input
                        .gateway_order_id
                        .clone()
                        .or_else(|| callback_gateway_order_id.clone());

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
  payment_provider,
  payment_channel,
  order_kind,
  product_id,
  product_snapshot,
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
  payment_provider,
  payment_channel,
  order_kind,
  product_id,
  product_snapshot,
  gateway_order_id,
  gateway_response,
  status,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM paid_at) AS BIGINT) AS paid_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM credited_at) AS BIGINT) AS credited_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM expires_at) AS BIGINT) AS expires_at_unix_secs
FROM payment_orders
WHERE payment_method = $1
  AND gateway_order_id = $2
LIMIT 1
FOR UPDATE
                            "#,
                        )
                        .bind(&input.payment_method)
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
                    let order_payment_provider: Option<String> =
                        row_get(&order_row, "payment_provider")?;
                    let order_payment_channel: Option<String> =
                        row_get(&order_row, "payment_channel")?;
                    let order_pay_currency: Option<String> = row_get(&order_row, "pay_currency")?;
                    let order_gateway_order_id: Option<String> =
                        row_get(&order_row, "gateway_order_id")?;
                    let order_kind: String = row_get(&order_row, "order_kind")?;
                    let order_amount_usd: f64 = row_get(&order_row, "amount_usd")?;
                    let order_pay_amount: Option<f64> = row_get(&order_row, "pay_amount")?;
                    let order_exchange_rate: Option<f64> =
                        row_get(&order_row, "exchange_rate")?;
                    let order_status: String = row_get(&order_row, "status")?;
                    let expires_at_unix_secs: Option<i64> =
                        row_get(&order_row, "expires_at_unix_secs")?;
                    let order_gateway_response: Option<serde_json::Value> =
                        if order_status.eq_ignore_ascii_case("failed") {
                            row_get(&order_row, "gateway_response")?
                        } else {
                            None
                        };
                    let failed_checkout_recoverable =
                        payment_order_is_failed_wallet_checkout_placeholder(
                            &order_status,
                            &order_kind,
                            order_gateway_response.as_ref(),
                        );
                    let uncertain_checkout =
                        payment_order_is_uncertain_wallet_checkout_placeholder(
                            &order_status,
                            &order_kind,
                            order_gateway_response.as_ref(),
                        );
                    if !order_amount_usd.is_finite()
                        || order_amount_usd <= 0.0
                        || order_pay_amount
                            .is_some_and(|value| !value.is_finite() || value <= 0.0)
                    {
                        update_payment_callback_failure(
                            tx,
                            &callback_id,
                            &input,
                            "payment order amount is invalid",
                        )
                        .await?;
                        return Ok(ProcessPaymentCallbackOutcome::Failed {
                            duplicate,
                            error: "payment order amount is invalid".to_string(),
                        });
                    }

                    // A payment order may only credit a wallet owned by the
                    // same live user. Lock the wallet and associated API-key row
                    // before any gateway binding, entitlement, wallet, or
                    // order mutation. Reject legacy rows with an ambiguous
                    // owner shape instead of guessing an owner.
                    let order_user_id: Option<String> = row_get(&order_row, "user_id")?;
                    let Some(order_user_id) = order_user_id
                        .as_deref()
                        .filter(|value| !value.trim().is_empty())
                    else {
                        update_payment_callback_failure(
                            tx,
                            &callback_id,
                            &input,
                            "payment order user missing",
                        )
                        .await?;
                        return Ok(ProcessPaymentCallbackOutcome::Failed {
                            duplicate,
                            error: "payment order user missing".to_string(),
                        });
                    };
                    let Some(wallet_owner_row) = sqlx::query(
                        r#"
SELECT
  user_id AS wallet_user_id,
  api_key_id AS wallet_api_key_id
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
                    let wallet_user_id: Option<String> =
                        row_get(&wallet_owner_row, "wallet_user_id")?;
                    let wallet_api_key_id: Option<String> =
                        row_get(&wallet_owner_row, "wallet_api_key_id")?;
                    // PostgreSQL cannot lock the nullable side of a LEFT JOIN.
                    // Read API-key ownership under its own lock while retaining
                    // the wallet lock so both identities remain stable.
                    let api_key_user_id: Option<String> =
                        if let Some(api_key_id) = wallet_api_key_id.as_deref() {
                            sqlx::query_scalar(
                                "SELECT user_id FROM api_keys WHERE id = $1 FOR UPDATE",
                            )
                            .bind(api_key_id)
                            .fetch_optional(&mut **tx)
                            .await
                            .map_postgres_err()?
                        } else {
                            None
                        };
                    let wallet_owner_matches = match (
                        wallet_user_id.as_deref(),
                        wallet_api_key_id.as_deref(),
                        api_key_user_id.as_deref(),
                    ) {
                        (Some(wallet_user_id), None, _)
                            if !wallet_user_id.trim().is_empty() =>
                        {
                            wallet_user_id == order_user_id
                        }
                        (None, Some(wallet_api_key_id), Some(api_key_user_id))
                            if !wallet_api_key_id.trim().is_empty()
                                && !api_key_user_id.trim().is_empty() =>
                        {
                            api_key_user_id == order_user_id
                        }
                        _ => false,
                    };
                    if !wallet_owner_matches {
                        update_payment_callback_failure(
                            tx,
                            &callback_id,
                            &input,
                            "payment order wallet owner mismatch",
                        )
                        .await?;
                        return Ok(ProcessPaymentCallbackOutcome::Failed {
                            duplicate,
                            error: "payment order wallet owner mismatch".to_string(),
                        });
                    }

                    // The lookup identifier is not proof that the callback
                    // belongs to this order: order_no takes precedence over
                    // gateway_order_id. Check every identifier supplied by
                    // this delivery (and any persisted fallback from the
                    // callback row) before changing the order or wallet.
                    // Orders created before the gateway returns a provider
                    // transaction id store order_no as a placeholder; that
                    // value may be replaced by a verified callback, but a
                    // real id must never be rebound to another order.
                    if input
                        .order_no
                        .as_deref()
                        .is_some_and(|value| value != order_no)
                        || callback_order_no
                            .as_deref()
                            .is_some_and(|value| value != order_no)
                    {
                        update_payment_callback_failure(
                            tx,
                            &callback_id,
                            &input,
                            "payment order number mismatch",
                        )
                        .await?;
                        return Ok(ProcessPaymentCallbackOutcome::Failed {
                            duplicate,
                            error: "payment order number mismatch".to_string(),
                        });
                    }
                    let input_gateway_order_id = input
                        .gateway_order_id
                        .as_deref()
                        .map(str::trim)
                        .filter(|value| !value.is_empty());
                    let callback_gateway_order_id = callback_gateway_order_id
                        .as_deref()
                        .map(str::trim)
                        .filter(|value| !value.is_empty());
                    let stored_real_gateway_order_id = order_gateway_order_id
                        .as_deref()
                        .map(str::trim)
                        .filter(|value| !value.is_empty() && *value != order_no);
                    let input_real_gateway_order_id =
                        input_gateway_order_id.filter(|value| *value != order_no);
                    let callback_real_gateway_order_id =
                        callback_gateway_order_id.filter(|value| *value != order_no);
                    let effective_gateway_order_id = input_real_gateway_order_id
                        .or(callback_real_gateway_order_id)
                        .or(stored_real_gateway_order_id);
                    if let Some(expected_gateway_order_id) = stored_real_gateway_order_id {
                        if input_gateway_order_id
                            .is_some_and(|value| value != expected_gateway_order_id)
                            || callback_gateway_order_id
                                .is_some_and(|value| value != expected_gateway_order_id)
                        {
                            update_payment_callback_failure(
                                tx,
                                &callback_id,
                                &input,
                                "payment gateway order mismatch",
                            )
                            .await?;
                            return Ok(ProcessPaymentCallbackOutcome::Failed {
                                duplicate,
                                error: "payment gateway order mismatch".to_string(),
                            });
                        }
                    } else if let (Some(input_gateway), Some(callback_gateway)) =
                        (input_real_gateway_order_id, callback_real_gateway_order_id)
                    {
                        if input_gateway != callback_gateway {
                            update_payment_callback_failure(
                                tx,
                                &callback_id,
                                &input,
                                "payment gateway order identifier mismatch",
                            )
                            .await?;
                            return Ok(ProcessPaymentCallbackOutcome::Failed {
                                duplicate,
                                error: "payment gateway order identifier mismatch".to_string(),
                            });
                        }
                    }
                    if stored_real_gateway_order_id.is_none() {
                        if let Some(gateway_order_id) = effective_gateway_order_id {
                            let conflicting_order_id: Option<String> = sqlx::query_scalar(
                                "SELECT id FROM payment_orders WHERE payment_method = $1 AND gateway_order_id = $2 AND id <> $3 LIMIT 1",
                            )
                            .bind(&order_payment_method)
                            .bind(gateway_order_id)
                            .bind(&order_id)
                            .fetch_optional(&mut **tx)
                            .await
                            .map_postgres_err()?;
                            if conflicting_order_id.is_some() {
                                update_payment_callback_failure(
                                    tx,
                                    &callback_id,
                                    &input,
                                    "payment gateway order belongs to another payment order",
                                )
                                .await?;
                                return Ok(ProcessPaymentCallbackOutcome::Failed {
                                    duplicate,
                                    error: "payment gateway order belongs to another payment order"
                                        .to_string(),
                                });
                            }
                        }
                    }

                    let amount_matches = payment_callback_amount_matches_order(
                        order_amount_usd,
                        order_pay_amount,
                        order_pay_currency.as_deref(),
                        order_exchange_rate,
                        input.amount_usd,
                        input.pay_amount,
                    );
                    if !amount_matches {
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
                    if !payment_callback_method_matches_order(
                        &order_payment_method,
                        order_payment_provider.as_deref(),
                        &input.payment_method,
                        input.payment_provider.as_deref(),
                    ) {
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
                    let payment_provider_matches = payment_callback_provider_matches_order(
                        &order_payment_method,
                        order_payment_provider.as_deref(),
                        &input.payment_method,
                        input.payment_provider.as_deref(),
                    );
                    if !payment_provider_matches {
                            update_payment_callback_failure(
                                tx,
                                &callback_id,
                                &input,
                                "payment provider mismatch",
                            )
                            .await?;
                            return Ok(ProcessPaymentCallbackOutcome::Failed {
                                duplicate,
                                error: "payment provider mismatch".to_string(),
                            });
                    }
                    let currency_matches =
                        match (input.pay_currency.as_deref(), order_pay_currency.as_deref()) {
                            (Some(callback), Some(order)) => {
                                order.eq_ignore_ascii_case(callback)
                            }
                            (None, None) => input.pay_amount.is_none() && order_pay_amount.is_none(),
                            _ => false,
                        };
                    if !currency_matches {
                        update_payment_callback_failure(
                            tx,
                            &callback_id,
                            &input,
                            "payment currency mismatch",
                        )
                        .await?;
                        return Ok(ProcessPaymentCallbackOutcome::Failed {
                            duplicate,
                            error: "payment currency mismatch".to_string(),
                        });
                    }
                    if let Some(expected_channel) = input.payment_channel.as_deref() {
                        let stored_channel = order_payment_channel.as_deref().or_else(|| {
                            (order_payment_provider.is_none()
                                && ["alipay", "wxpay"].iter().any(|method| {
                                    method.eq_ignore_ascii_case(&order_payment_method)
                                }))
                            .then_some(order_payment_method.as_str())
                        });
                        if stored_channel
                            .is_none_or(|value| !value.eq_ignore_ascii_case(expected_channel))
                        {
                            update_payment_callback_failure(
                                tx,
                                &callback_id,
                                &input,
                                "payment channel mismatch",
                            )
                            .await?;
                            return Ok(ProcessPaymentCallbackOutcome::Failed {
                                duplicate,
                                error: "payment channel mismatch".to_string(),
                            });
                        }
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
                    if !matches!(order_status.as_str(), "pending" | "paid")
                        && !failed_checkout_recoverable
                    {
                        let error = format!("payment order is not creditable: {order_status}");
                        update_payment_callback_failure(tx, &callback_id, &input, &error).await?;
                        return Ok(ProcessPaymentCallbackOutcome::Failed { duplicate, error });
                    }
                    if order_status == "pending"
                        || (failed_checkout_recoverable && !uncertain_checkout)
                    {
                        let now = Utc::now().timestamp();
                        if expires_at_unix_secs.is_some_and(|value| value <= now) {
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

                    if stored_real_gateway_order_id.is_none() {
                        if let Some(gateway_order_id) = effective_gateway_order_id {
                            if !postgres_bind_payment_gateway_order_id(
                                tx,
                                &order_id,
                                gateway_order_id,
                            )
                            .await?
                            {
                                update_payment_callback_failure(
                                    tx,
                                    &callback_id,
                                    &input,
                                    "payment gateway order belongs to another payment order",
                                )
                                .await?;
                                return Ok(ProcessPaymentCallbackOutcome::Failed {
                                    duplicate,
                                    error: "payment gateway order belongs to another payment order"
                                        .to_string(),
                                });
                            }
                        }
                    }

                    if order_kind == "plan_purchase" {
                        let product_id: Option<String> = row_get(&order_row, "product_id")?;
                        let product_snapshot: Option<serde_json::Value> =
                            row_get(&order_row, "product_snapshot")?;
                        let order_user_id: Option<String> = row_get(&order_row, "user_id")?;
                        let Some(user_id) = order_user_id else {
                            update_payment_callback_failure(
                                tx,
                                &callback_id,
                                &input,
                                "payment order user missing",
                            )
                            .await?;
                            return Ok(ProcessPaymentCallbackOutcome::Failed {
                                duplicate,
                                error: "payment order user missing".to_string(),
                            });
                        };
                        let snapshot = product_snapshot.unwrap_or_else(|| serde_json::json!({}));
                        let plan_id = product_id.unwrap_or_else(|| {
                            snapshot
                                .get("id")
                                .and_then(|value| value.as_str())
                                .unwrap_or("unknown")
                                .to_string()
                        });
                        let entitlements = plan_entitlements_snapshot(&snapshot);
                        let now = Utc::now();
                        let expires_at = plan_expires_at(&snapshot, now)?;
                        let existing_entitlement_id = sqlx::query_scalar::<_, String>(
                            r#"
SELECT id
FROM user_plan_entitlements
WHERE payment_order_id = $1
LIMIT 1
                            "#,
                        )
                        .bind(&order_id)
                        .fetch_optional(&mut **tx)
                        .await
                        .map_postgres_err()?;
                        if existing_entitlement_id.is_none() {
                            sqlx::query("SELECT id FROM wallets WHERE id = $1 LIMIT 1 FOR UPDATE")
                                .bind(&order_wallet_id)
                                .fetch_optional(&mut **tx)
                                .await
                                .map_postgres_err()?;
                            let purchase_limit_scope = plan_purchase_limit_scope(&snapshot);
                            if purchase_limit_scope != "unlimited" {
                                let max_active_per_user = plan_max_active_per_user(&snapshot);
                                let active_count = if purchase_limit_scope == "lifetime" {
                                    sqlx::query_scalar::<_, i64>(
                                        r#"
SELECT COUNT(*)::bigint
FROM user_plan_entitlements
WHERE user_id = $1
  AND plan_id = $2
  AND status = 'active'
                                "#,
                                    )
                                    .bind(&user_id)
                                    .bind(&plan_id)
                                    .fetch_one(&mut **tx)
                                    .await
                                    .map_postgres_err()?
                                } else {
                                    sqlx::query_scalar::<_, i64>(
                                        r#"
SELECT COUNT(*)::bigint
FROM user_plan_entitlements
WHERE user_id = $1
  AND plan_id = $2
  AND status = 'active'
  AND expires_at > NOW()
                                "#,
                                    )
                                    .bind(&user_id)
                                    .bind(&plan_id)
                                    .fetch_one(&mut **tx)
                                    .await
                                    .map_postgres_err()?
                                };
                                if active_count >= max_active_per_user {
                                    update_payment_callback_failure(
                                        tx,
                                        &callback_id,
                                        &input,
                                        "plan purchase limit reached",
                                    )
                                    .await?;
                                    return Ok(ProcessPaymentCallbackOutcome::Failed {
                                        duplicate,
                                        error: "plan purchase limit reached".to_string(),
                                    });
                                }
                            }
                            replace_matching_plan_entitlements_postgres(
                                tx, &user_id, &snapshot, now,
                            )
                            .await?;
                            sqlx::query(
                                r#"
INSERT INTO user_plan_entitlements (
  id, user_id, plan_id, payment_order_id, status, starts_at, expires_at,
  entitlements_snapshot, created_at, updated_at
)
VALUES ($1, $2, $3, $4, 'active', $5, $6, $7, NOW(), NOW())
                                "#,
                            )
                            .bind(Uuid::new_v4().to_string())
                            .bind(&user_id)
                            .bind(&plan_id)
                            .bind(&order_id)
                            .bind(now)
                            .bind(expires_at)
                            .bind(&entitlements)
                            .execute(&mut **tx)
                            .await
                            .map_postgres_err()?;
                            apply_plan_wallet_credit_postgres(
                                tx,
                                &order_wallet_id,
                                &order_id,
                                &input.payment_method,
                                &entitlements,
                            )
                            .await?;
                        }
                        let updated_order_row = sqlx::query(
                            r#"
UPDATE payment_orders
SET gateway_order_id = COALESCE($2, gateway_order_id),
    gateway_response = $3,
    pay_amount = COALESCE(pay_amount, $4),
    pay_currency = COALESCE(pay_currency, $5),
    exchange_rate = COALESCE(exchange_rate, $6),
    status = 'credited',
    fulfillment_status = 'fulfilled',
    fulfillment_error = NULL,
    paid_at = COALESCE(paid_at, NOW()),
    credited_at = NOW(),
    refundable_amount_usd = 0
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
	  payment_provider,
	  payment_channel,
	  order_kind,
	  product_id,
	  product_snapshot,
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
                        .bind(effective_gateway_order_id)
                        .bind(input.gateway_response_projection(
                            &order_no,
                            effective_gateway_order_id,
                        ))
                        .bind(input.pay_amount)
                        .bind(input.pay_currency.as_deref())
                        .bind(input.exchange_rate)
                        .fetch_one(&mut **tx)
                        .await
                        .map_postgres_err()?;
                        mark_payment_callback_processed(
                            tx,
                            &callback_id,
                            &input,
                            &order_id,
                            &order_no,
                        )
                        .await?;
                        return Ok(ProcessPaymentCallbackOutcome::Applied {
                            duplicate,
                            order_id,
                            order_no,
                            wallet_id: order_wallet_id,
                            order: map_admin_payment_order_row(&updated_order_row)?,
                        });
                    }

                    let Some(wallet_row) = sqlx::query(
                        r#"
SELECT
  id,
  status,
  CAST(balance AS DOUBLE PRECISION) AS balance,
  CAST(gift_balance AS DOUBLE PRECISION) AS gift_balance,
  CAST(total_recharged AS DOUBLE PRECISION) AS total_recharged
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
                    let total_recharged: f64 = row_get(&wallet_row, "total_recharged")?;
                    // Finite recharge balances may be negative: usage settlement permits a
                    // finite wallet to overdraft, and a later recharge must be able to restore
                    // that balance. Reject only malformed values and arithmetic overflow here.
                    if !before_recharge.is_finite()
                        || !before_gift.is_finite()
                        || before_gift < 0.0
                        || !total_recharged.is_finite()
                        || total_recharged < 0.0
                        || !(total_recharged + order_amount_usd).is_finite()
                        || !(before_recharge + before_gift + order_amount_usd).is_finite()
                    {
                        update_payment_callback_failure(
                            tx,
                            &callback_id,
                            &input,
                            "wallet balance is invalid",
                        )
                        .await?;
                        return Ok(ProcessPaymentCallbackOutcome::Failed {
                            duplicate,
                            error: "wallet balance is invalid".to_string(),
                        });
                    }
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
    pay_amount = COALESCE(pay_amount, $4),
    pay_currency = COALESCE(pay_currency, $5),
    exchange_rate = COALESCE(exchange_rate, $6),
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
	  payment_provider,
	  payment_channel,
	  order_kind,
	  product_id,
	  product_snapshot,
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
                        .bind(effective_gateway_order_id)
                    .bind(input.gateway_response_projection(
                        &order_no,
                        effective_gateway_order_id,
                    ))
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
        if !input.amount_usd.is_finite() || input.amount_usd == 0.0 {
            return Err(DataLayerError::InvalidInput(
                "adjustment amount must be finite and non-zero".to_string(),
            ));
        }
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
                    let before_total_adjusted: f64 = row_get(&row, "total_adjusted")?;
                    if !before_recharge.is_finite()
                        || !before_gift.is_finite()
                        || !before_total.is_finite()
                        || !before_total_adjusted.is_finite()
                    {
                        return Err(DataLayerError::UnexpectedValue(
                            "wallet balance is invalid".to_string(),
                        ));
                    }
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
                    let after_total = after_recharge + after_gift;
                    let after_total_adjusted = before_total_adjusted + input.amount_usd;
                    if !after_recharge.is_finite()
                        || !after_gift.is_finite()
                        || !after_total.is_finite()
                        || !after_total_adjusted.is_finite()
                    {
                        return Err(DataLayerError::UnexpectedValue(
                            "wallet balance overflow during admin adjustment".to_string(),
                        ));
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
                    .bind(after_total)
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
                            balance_after: after_total,
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
        mut input: CreateManualWalletRechargeInput,
    ) -> Result<Option<(StoredWalletSnapshot, StoredAdminPaymentOrder)>, DataLayerError> {
        input.payment_method = canonicalize_payment_method(&input.payment_method)
            .map_err(DataLayerError::InvalidInput)?;
        if !input.amount_usd.is_finite() || input.amount_usd <= 0.0 {
            return Err(DataLayerError::InvalidInput(
                "manual recharge amount must be finite and positive".to_string(),
            ));
        }
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
                    let before_total_recharged: f64 = row_get(&wallet_row, "total_recharged")?;
                    let (after_recharge, after_total_recharged) = validate_manual_wallet_recharge(
                        input.amount_usd,
                        before_recharge,
                        before_gift,
                        before_total_recharged,
                    )
                    .map_err(DataLayerError::InvalidInput)?;
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

                    let wallet_row = sqlx::query(
                        r#"
UPDATE wallets
SET
  balance = $2,
  total_recharged = $3,
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
                    .bind(after_total_recharged)
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

                    let order_row = sqlx::query(FIND_ADMIN_PAYMENT_ORDER_SQL)
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
                    if !refund.amount_usd.is_finite() || refund.amount_usd <= 0.0 {
                        return Ok(WalletMutationOutcome::Invalid(
                            "refund amount must be finite and greater than zero".to_string(),
                        ));
                    }
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
                    let before_total_refunded: f64 = row_get(&wallet_row, "total_refunded")?;
                    let amount_usd = refund.amount_usd;
                    let after_recharge = before_recharge - amount_usd;
                    let before_total = before_recharge + before_gift;
                    let after_total = after_recharge + before_gift;
                    let after_total_refunded = before_total_refunded + amount_usd;
                    if !before_recharge.is_finite()
                        || before_recharge < 0.0
                        || !before_gift.is_finite()
                        || before_gift < 0.0
                        || !before_total_refunded.is_finite()
                        || before_total_refunded < 0.0
                        || !before_total.is_finite()
                        || !after_recharge.is_finite()
                        || !after_total.is_finite()
                        || !after_total_refunded.is_finite()
                    {
                        return Ok(WalletMutationOutcome::Invalid(
                            "wallet balance is invalid".to_string(),
                        ));
                    }
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
  wallet_id,
  status,
  CAST(amount_usd AS DOUBLE PRECISION) AS amount_usd,
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
                        let order_wallet_id: String = row_get(&order_row, "wallet_id")?;
                        let order_status: String = row_get(&order_row, "status")?;
                        if order_wallet_id != input.wallet_id || order_status != "credited" {
                            return Ok(WalletMutationOutcome::Invalid(
                                "payment order is not refundable for this wallet".to_string(),
                            ));
                        }
                        let order_amount: f64 = row_get(&order_row, "amount_usd")?;
                        let refunded_before: f64 = row_get(&order_row, "refunded_amount_usd")?;
                        let refundable_before: f64 = row_get(&order_row, "refundable_amount_usd")?;
                        let refunded_after = refunded_before + amount_usd;
                        let refundable_after = refundable_before - amount_usd;
                        if !payment_order_refund_amounts_are_consistent(
                            order_amount,
                            refunded_before,
                            refundable_before,
                        ) || amount_usd > refundable_before
                            || !refunded_after.is_finite()
                            || refunded_after < 0.0
                            || refunded_after > order_amount
                            || !refundable_after.is_finite()
                            || refundable_after < 0.0
                            || refundable_after > order_amount
                        {
                            return Ok(WalletMutationOutcome::Invalid(
                                "payment order refund amounts are invalid".to_string(),
                            ));
                        }
                        let result = sqlx::query(
                            r#"
UPDATE payment_orders
SET
  refunded_amount_usd = $2,
  refundable_amount_usd = $3
WHERE id = $1
                            "#,
                        )
                        .bind(payment_order_id)
                        .bind(refunded_after)
                        .bind(refundable_after)
                        .execute(&mut **tx)
                        .await
                        .map_postgres_err()?;
                        if result.rows_affected() != 1 {
                            return Err(DataLayerError::UnexpectedValue(
                                "payment order disappeared during refund processing".to_string(),
                            ));
                        }
                    }

                    let wallet_row = sqlx::query(
                        r#"
UPDATE wallets
SET
  balance = $2,
  total_refunded = $3,
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
                    .bind(after_total_refunded)
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
                    let refund = map_admin_wallet_refund_row(&current_refund)?;
                    if !refund.amount_usd.is_finite() || refund.amount_usd <= 0.0 {
                        return Ok(WalletMutationOutcome::Invalid(
                            "refund amount must be finite and greater than zero".to_string(),
                        ));
                    }
                    if let (Some(existing_id), Some(incoming_id)) = (
                        refund.gateway_refund_id.as_deref(),
                        input.gateway_refund_id.as_deref(),
                    ) {
                        if existing_id != incoming_id {
                            return Ok(WalletMutationOutcome::Invalid(
                                "gateway refund identifier conflicts with existing evidence"
                                    .to_string(),
                            ));
                        }
                    }
                    if refund.status == "succeeded" {
                        return Ok(WalletMutationOutcome::Applied(refund));
                    }
                    if refund.status != "processing" {
                        return Ok(WalletMutationOutcome::Invalid(
                            "refund status must be processing before completion".to_string(),
                        ));
                    }
                    // A processing proof is durable evidence of the provider's last
                    // response. Preserve it for ordinary replays, but replace it when the
                    // same gateway refund reaches an explicit successful terminal state.
                    let payout_proof = input
                        .payout_proof
                        .as_ref()
                        .filter(|proof| {
                            refund.payout_proof.is_none() || wallet_refund_proof_is_success(proof)
                        })
                        .cloned()
                        .or_else(|| refund.payout_proof.clone());

                    let refund_row = sqlx::query(
                        r#"
UPDATE refund_requests
SET
  status = 'succeeded',
  gateway_refund_id = COALESCE($3, gateway_refund_id),
  payout_reference = COALESCE($4, payout_reference),
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
                    .bind(&payout_proof)
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

    async fn update_admin_wallet_refund_gateway(
        &self,
        input: UpdateAdminWalletRefundGatewayInput,
    ) -> Result<WalletMutationOutcome<StoredAdminWalletRefund>, DataLayerError> {
        if input.gateway_refund_id.trim().is_empty() || input.gateway_refund_id.len() > 128 {
            return Ok(WalletMutationOutcome::Invalid(
                "gateway refund identifier is invalid".to_string(),
            ));
        }
        if input
            .payout_proof
            .as_ref()
            .is_some_and(|proof| !proof.is_object())
        {
            return Ok(WalletMutationOutcome::Invalid(
                "gateway refund proof must be an object".to_string(),
            ));
        }
        self.tx_runner
            .run_read_write(|tx| {
                Box::pin(async move {
                    let Some(current_row) = sqlx::query(
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
                    let current = map_admin_wallet_refund_row(&current_row)?;
                    if !current.amount_usd.is_finite() || current.amount_usd <= 0.0 {
                        return Ok(WalletMutationOutcome::Invalid(
                            "refund amount must be finite and greater than zero".to_string(),
                        ));
                    }
                    if let Some(existing_id) = current.gateway_refund_id.as_deref() {
                        if existing_id != input.gateway_refund_id {
                            return Ok(WalletMutationOutcome::Invalid(
                                "gateway refund identifier conflicts with existing evidence"
                                    .to_string(),
                            ));
                        }
                    }
                    if current.status == "succeeded" {
                        return Ok(WalletMutationOutcome::Applied(current));
                    }
                    if current.status != "processing" {
                        return Ok(WalletMutationOutcome::Invalid(
                            "refund status must be processing before gateway update".to_string(),
                        ));
                    }
                    // Do not let a retry overwrite a pending proof with arbitrary data. A
                    // provider's explicit success response is the one permitted upgrade.
                    let payout_proof = input
                        .payout_proof
                        .as_ref()
                        .filter(|proof| {
                            current.payout_proof.is_none() || wallet_refund_proof_is_success(proof)
                        })
                        .cloned()
                        .or_else(|| current.payout_proof.clone());
                    let row = sqlx::query(
                        r#"
UPDATE refund_requests
SET gateway_refund_id = COALESCE(gateway_refund_id, $3),
    payout_proof = $4,
    updated_at = NOW()
WHERE id = $1 AND wallet_id = $2 AND status = 'processing'
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
                    .bind(&input.gateway_refund_id)
                    .bind(&payout_proof)
                    .fetch_one(&mut **tx)
                    .await
                    .map_postgres_err()?;
                    Ok(WalletMutationOutcome::Applied(map_admin_wallet_refund_row(
                        &row,
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
                    if !refund.amount_usd.is_finite() || refund.amount_usd <= 0.0 {
                        return Ok(WalletMutationOutcome::Invalid(
                            "refund amount must be finite and greater than zero".to_string(),
                        ));
                    }

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

                    // Only an explicitly offline payout can be released without
                    // external settlement evidence.  An original-channel refund
                    // may still be in flight between the provider request and
                    // the evidence update.
                    if refund.gateway_refund_id.is_some()
                        || refund.payout_proof.is_some()
                        || !refund
                            .refund_mode
                            .trim()
                            .eq_ignore_ascii_case("offline_payout")
                    {
                        return Ok(WalletMutationOutcome::Invalid(
                            "cannot fail refund while gateway settlement is processing".to_string(),
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
                    let amount_usd = refund.amount_usd;
                    let before_recharge: f64 = row_get(&wallet_row, "balance")?;
                    let before_gift: f64 = row_get(&wallet_row, "gift_balance")?;
                    let before_total_refunded: f64 = row_get(&wallet_row, "total_refunded")?;
                    let before_total = before_recharge + before_gift;
                    let after_recharge = before_recharge + amount_usd;
                    let after_total = after_recharge + before_gift;
                    let after_total_refunded = before_total_refunded - amount_usd;
                    if !before_recharge.is_finite()
                        || before_recharge < 0.0
                        || !before_gift.is_finite()
                        || before_gift < 0.0
                        || !before_total_refunded.is_finite()
                        || before_total_refunded < 0.0
                        || before_total_refunded < amount_usd
                        || !before_total.is_finite()
                        || !after_recharge.is_finite()
                        || !after_total.is_finite()
                        || !after_total_refunded.is_finite()
                        || after_total_refunded < 0.0
                    {
                        return Ok(WalletMutationOutcome::Invalid(
                            "wallet balance is invalid for refund recovery".to_string(),
                        ));
                    }

                    let mut order_amounts = None;
                    if let Some(payment_order_id) = refund.payment_order_id.as_deref() {
                        let Some(order_row) = sqlx::query(
                            r#"
SELECT
  wallet_id,
  status,
  CAST(amount_usd AS DOUBLE PRECISION) AS amount_usd,
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
                        let order_wallet_id: String = row_get(&order_row, "wallet_id")?;
                        let order_status: String = row_get(&order_row, "status")?;
                        if order_wallet_id != input.wallet_id || order_status != "credited" {
                            return Ok(WalletMutationOutcome::Invalid(
                                "payment order is not refundable for this wallet".to_string(),
                            ));
                        }
                        let order_amount: f64 = row_get(&order_row, "amount_usd")?;
                        let refunded_before: f64 = row_get(&order_row, "refunded_amount_usd")?;
                        let refundable_before: f64 = row_get(&order_row, "refundable_amount_usd")?;
                        let refunded_after = refunded_before - amount_usd;
                        let refundable_after = refundable_before + amount_usd;
                        if !payment_order_refund_amounts_are_consistent(
                            order_amount,
                            refunded_before,
                            refundable_before,
                        ) || refunded_before < amount_usd
                            || !refunded_after.is_finite()
                            || refunded_after < 0.0
                            || !refundable_after.is_finite()
                            || refundable_after < 0.0
                            || refundable_after > order_amount
                        {
                            return Ok(WalletMutationOutcome::Invalid(
                                "payment order refund amounts are invalid".to_string(),
                            ));
                        }
                        order_amounts = Some((
                            payment_order_id.to_string(),
                            refunded_after,
                            refundable_after,
                        ));
                    }

                    let wallet_row = sqlx::query(
                        r#"
UPDATE wallets
SET
  balance = $2,
  total_refunded = $3,
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
                    .bind(after_total_refunded)
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
                    .bind(after_total)
                    .bind(before_recharge)
                    .bind(after_recharge)
                    .bind(before_gift)
                    .bind(before_gift)
                    .bind(&input.refund_id)
                    .bind(input.operator_id.as_deref())
                    .execute(&mut **tx)
                    .await
                    .map_postgres_err()?;

                    if let Some((payment_order_id, refunded_after, refundable_after)) =
                        order_amounts
                    {
                        let result = sqlx::query(
                            r#"
UPDATE payment_orders
SET
  refunded_amount_usd = $2,
  refundable_amount_usd = $3
WHERE id = $1
                            "#,
                        )
                        .bind(payment_order_id)
                        .bind(refunded_after)
                        .bind(refundable_after)
                        .execute(&mut **tx)
                        .await
                        .map_postgres_err()?;
                        if result.rows_affected() != 1 {
                            return Err(DataLayerError::UnexpectedValue(
                                "payment order disappeared during refund recovery".to_string(),
                            ));
                        }
                    }

                    let refund_row = sqlx::query(
                        r#"
UPDATE refund_requests
SET
  status = 'failed',
  failure_reason = $3,
  updated_at = NOW()
WHERE id = $1 AND wallet_id = $2 AND status = 'processing'
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
                    .fetch_optional(&mut **tx)
                    .await
                    .map_postgres_err()?
                    .ok_or_else(|| {
                        DataLayerError::UnexpectedValue(
                            "refund status changed during recovery".to_string(),
                        )
                    })?;
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
                            balance_after: after_total,
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
	  payment_provider,
	  payment_channel,
	  order_kind,
	  product_id,
	  product_snapshot,
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
  payment_provider,
  order_kind,
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
  payment_provider,
  order_kind,
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
  payment_provider,
  order_kind,
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
  payment_provider,
  payment_channel,
  order_kind,
  product_id,
  product_snapshot,
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
                        .is_some_and(|value| value <= Utc::now().timestamp().max(0) as u64)
                    {
                        return Ok(WalletMutationOutcome::Invalid(
                            "payment order expired".to_string(),
                        ));
                    }
                    let order_kind: String = row_get(&order_row, "order_kind")?;
                    let order_payment_provider: Option<String> =
                        row_get(&order_row, "payment_provider")?;
                    let order_payment_channel: Option<String> =
                        row_get(&order_row, "payment_channel")?;
                    if validate_payment_order_credit_amounts(
                        &order_kind,
                        &order.payment_method,
                        order_payment_provider.as_deref(),
                        order_payment_channel.as_deref(),
                        order.amount_usd,
                        order.pay_amount,
                    )
                    .is_err()
                    {
                        return Ok(WalletMutationOutcome::Invalid(
                            "payment order amount is invalid".to_string(),
                        ));
                    }
                    if order_kind == "plan_purchase" {
                        let order_user_id: Option<String> = row_get(&order_row, "user_id")?;
                        let Some(user_id) = order_user_id else {
                            return Ok(WalletMutationOutcome::Invalid(
                                "payment order user missing".to_string(),
                            ));
                        };
                        let product_id: Option<String> = row_get(&order_row, "product_id")?;
                        let product_snapshot: Option<serde_json::Value> =
                            row_get(&order_row, "product_snapshot")?;
                        let snapshot = product_snapshot.unwrap_or_else(|| serde_json::json!({}));
                        let plan_id = product_id.unwrap_or_else(|| {
                            snapshot
                                .get("id")
                                .and_then(|value| value.as_str())
                                .unwrap_or("unknown")
                                .to_string()
                        });
                        let entitlements = plan_entitlements_snapshot(&snapshot);
                        let now = Utc::now();
                        let expires_at = plan_expires_at(&snapshot, now)?;
                        let existing_entitlement_id = sqlx::query_scalar::<_, String>(
                            r#"
SELECT id
FROM user_plan_entitlements
WHERE payment_order_id = $1
LIMIT 1
                            "#,
                        )
                        .bind(&input.order_id)
                        .fetch_optional(&mut **tx)
                        .await
                        .map_postgres_err()?;
                        if existing_entitlement_id.is_none() {
                            let purchase_limit_scope = plan_purchase_limit_scope(&snapshot);
                            if purchase_limit_scope != "unlimited" {
                                let max_active_per_user = plan_max_active_per_user(&snapshot);
                                let active_count = if purchase_limit_scope == "lifetime" {
                                    sqlx::query_scalar::<_, i64>(
                                        r#"
SELECT COUNT(*)::bigint
FROM user_plan_entitlements
WHERE user_id = $1
  AND plan_id = $2
  AND status = 'active'
                                "#,
                                    )
                                    .bind(&user_id)
                                    .bind(&plan_id)
                                    .fetch_one(&mut **tx)
                                    .await
                                    .map_postgres_err()?
                                } else {
                                    sqlx::query_scalar::<_, i64>(
                                        r#"
SELECT COUNT(*)::bigint
FROM user_plan_entitlements
WHERE user_id = $1
  AND plan_id = $2
  AND status = 'active'
  AND expires_at > NOW()
                                "#,
                                    )
                                    .bind(&user_id)
                                    .bind(&plan_id)
                                    .fetch_one(&mut **tx)
                                    .await
                                    .map_postgres_err()?
                                };
                                if active_count >= max_active_per_user {
                                    return Ok(WalletMutationOutcome::Invalid(
                                        "plan purchase limit reached".to_string(),
                                    ));
                                }
                            }
                            replace_matching_plan_entitlements_postgres(
                                tx, &user_id, &snapshot, now,
                            )
                            .await?;
                            sqlx::query(
                                r#"
INSERT INTO user_plan_entitlements (
  id, user_id, plan_id, payment_order_id, status, starts_at, expires_at,
  entitlements_snapshot, created_at, updated_at
)
VALUES ($1, $2, $3, $4, 'active', $5, $6, $7, NOW(), NOW())
                                "#,
                            )
                            .bind(Uuid::new_v4().to_string())
                            .bind(&user_id)
                            .bind(&plan_id)
                            .bind(&input.order_id)
                            .bind(now)
                            .bind(expires_at)
                            .bind(&entitlements)
                            .execute(&mut **tx)
                            .await
                            .map_postgres_err()?;
                            apply_plan_wallet_credit_postgres(
                                tx,
                                &order.wallet_id,
                                &input.order_id,
                                &order.payment_method,
                                &entitlements,
                            )
                            .await?;
                        }

                        let mut gateway_response =
                            payment_gateway_response_map(order.gateway_response.clone());
                        if let Some(serde_json::Value::Object(map)) =
                            input.gateway_response_patch.clone()
                        {
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
                        let next_gateway_order_id =
                            input.gateway_order_id.clone().or(order.gateway_order_id);
                        let next_pay_amount = input.pay_amount.or(order.pay_amount);
                        let next_pay_currency = input.pay_currency.clone().or(order.pay_currency);
                        let next_exchange_rate = input.exchange_rate.or(order.exchange_rate);
                        let next_paid_at_unix_secs = order
                            .paid_at_unix_secs
                            .or(Some(now.timestamp().max(0) as u64));

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
  fulfillment_status = 'fulfilled',
  fulfillment_error = NULL,
  paid_at = COALESCE(to_timestamp($7), NOW()),
  credited_at = NOW(),
  refundable_amount_usd = 0
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
  payment_provider,
  order_kind,
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
                            i64::try_from(
                                next_paid_at_unix_secs.unwrap_or(now.timestamp().max(0) as u64),
                            )
                            .unwrap_or_default(),
                        )
                        .fetch_one(&mut **tx)
                        .await
                        .map_postgres_err()?;
                        return Ok(WalletMutationOutcome::Applied((
                            map_admin_payment_order_row(&row)?,
                            true,
                        )));
                    }

                    let Some(wallet_row) = sqlx::query(
                        r#"
SELECT
  id,
  status,
  CAST(balance AS DOUBLE PRECISION) AS balance,
  CAST(gift_balance AS DOUBLE PRECISION) AS gift_balance,
  CAST(total_recharged AS DOUBLE PRECISION) AS total_recharged
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
                    let total_recharged: f64 = row_get(&wallet_row, "total_recharged")?;
                    if !before_recharge.is_finite()
                        || !before_gift.is_finite()
                        || before_gift < 0.0
                        || !total_recharged.is_finite()
                        || total_recharged < 0.0
                        || !(total_recharged + order.amount_usd).is_finite()
                        || !(before_recharge + before_gift + order.amount_usd).is_finite()
                    {
                        return Ok(WalletMutationOutcome::Invalid(
                            "wallet balance is invalid".to_string(),
                        ));
                    }
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
  payment_provider,
  order_kind,
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
        validate_admin_redeem_code_batch_input(&input).map_err(DataLayerError::InvalidInput)?;
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
                    let (after_recharge, after_gift, after_total_recharged) =
                        validate_redeem_wallet_credit(
                            &balance_bucket,
                            amount_usd,
                            before_recharge,
                            before_gift,
                            wallet_snapshot.total_recharged,
                        )
                        .map_err(DataLayerError::UnexpectedValue)?;

                    let wallet_row = sqlx::query(
                        r#"
UPDATE wallets
SET
  balance = $2,
  gift_balance = $3,
  total_recharged = $4,
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
                    .bind(after_total_recharged)
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
  payment_provider,
  order_kind,
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

fn plan_entitlements_snapshot(snapshot: &serde_json::Value) -> serde_json::Value {
    snapshot
        .get("entitlements")
        .or_else(|| snapshot.get("entitlements_json"))
        .cloned()
        .unwrap_or_else(|| serde_json::json!([]))
}

fn plan_max_active_per_user(snapshot: &serde_json::Value) -> i64 {
    snapshot
        .get("max_active_per_user")
        .and_then(|value| value.as_i64())
        .unwrap_or(1)
        .max(1)
}

fn plan_purchase_limit_scope(snapshot: &serde_json::Value) -> &str {
    match snapshot
        .get("purchase_limit_scope")
        .and_then(|value| value.as_str())
    {
        Some("lifetime") => "lifetime",
        Some("unlimited") => "unlimited",
        _ => "active_period",
    }
}

async fn replace_matching_plan_entitlements_postgres(
    tx: &mut crate::PostgresTransaction,
    user_id: &str,
    snapshot: &serde_json::Value,
    now: chrono::DateTime<Utc>,
) -> Result<(), DataLayerError> {
    let incoming_entitlements = plan_entitlements_snapshot(snapshot);
    if !entitlements_have_replacement_selector(&incoming_entitlements) {
        return Ok(());
    }

    let rows = sqlx::query(
        r#"
SELECT id, entitlements_snapshot
FROM user_plan_entitlements
WHERE user_id = $1
  AND status = 'active'
  AND expires_at > $2
        "#,
    )
    .bind(user_id)
    .bind(now)
    .fetch_all(&mut **tx)
    .await
    .map_postgres_err()?;

    for row in rows {
        let entitlements: serde_json::Value = row_get(&row, "entitlements_snapshot")?;
        let should_replace =
            entitlements_should_replace_existing(&incoming_entitlements, &entitlements);
        if !should_replace {
            continue;
        }
        let entitlement_id: String = row_get(&row, "id")?;
        sqlx::query(
            r#"
UPDATE user_plan_entitlements
SET status = 'replaced',
    expires_at = LEAST(expires_at, $1),
    updated_at = NOW()
WHERE id = $2
  AND status = 'active'
  AND expires_at > $1
        "#,
        )
        .bind(now)
        .bind(entitlement_id)
        .execute(&mut **tx)
        .await
        .map_postgres_err()?;
    }
    Ok(())
}

fn plan_expires_at(
    snapshot: &serde_json::Value,
    starts_at: chrono::DateTime<Utc>,
) -> Result<chrono::DateTime<Utc>, DataLayerError> {
    let days =
        checked_plan_duration_days_from_snapshot(snapshot).map_err(DataLayerError::InvalidInput)?;
    let duration = chrono::TimeDelta::try_days(days).ok_or_else(|| {
        DataLayerError::InvalidInput("plan duration exceeds the supported range".to_string())
    })?;
    starts_at.checked_add_signed(duration).ok_or_else(|| {
        DataLayerError::InvalidInput("plan expiration exceeds the supported range".to_string())
    })
}

async fn apply_plan_wallet_credit_postgres(
    tx: &mut crate::PostgresTransaction,
    wallet_id: &str,
    order_id: &str,
    payment_method: &str,
    entitlements: &serde_json::Value,
) -> Result<(), DataLayerError> {
    validate_plan_wallet_credit_entitlements(entitlements).map_err(DataLayerError::InvalidInput)?;
    let credits = entitlements
        .as_array()
        .into_iter()
        .flatten()
        .filter(|item| {
            item.get("type")
                .and_then(|value| value.as_str())
                .is_some_and(|value| value.eq_ignore_ascii_case("wallet_credit"))
        })
        .filter_map(|item| {
            let amount = item.get("amount_usd").and_then(|value| value.as_f64())?;
            if amount <= 0.0 || !amount.is_finite() {
                return None;
            }
            let bucket = item
                .get("balance_bucket")
                .and_then(|value| value.as_str())
                .unwrap_or("gift")
                .trim()
                .to_ascii_lowercase();
            Some((amount, bucket))
        })
        .collect::<Vec<_>>();
    if credits.is_empty() {
        return Ok(());
    }

    let Some(wallet_row) = sqlx::query(
        r#"
SELECT
  id,
  status,
  CAST(balance AS DOUBLE PRECISION) AS balance,
  CAST(gift_balance AS DOUBLE PRECISION) AS gift_balance,
  CAST(total_recharged AS DOUBLE PRECISION) AS total_recharged
FROM wallets
WHERE id = $1
LIMIT 1
FOR UPDATE
        "#,
    )
    .bind(wallet_id)
    .fetch_optional(&mut **tx)
    .await
    .map_postgres_err()?
    else {
        return Err(DataLayerError::UnexpectedValue(
            "wallet not found for plan wallet_credit".to_string(),
        ));
    };
    let wallet_status: String = row_get(&wallet_row, "status")?;
    if wallet_status != "active" {
        return Err(DataLayerError::UnexpectedValue(
            "wallet is not active for plan wallet_credit".to_string(),
        ));
    }
    let mut recharge_balance: f64 = row_get(&wallet_row, "balance")?;
    let mut gift_balance: f64 = row_get(&wallet_row, "gift_balance")?;
    let mut total_recharged: f64 = row_get(&wallet_row, "total_recharged")?;
    if !recharge_balance.is_finite()
        || !gift_balance.is_finite()
        || gift_balance < 0.0
        || !total_recharged.is_finite()
        || total_recharged < 0.0
    {
        return Err(DataLayerError::UnexpectedValue(
            "wallet balance is invalid for plan wallet_credit".to_string(),
        ));
    }
    for (amount, bucket) in credits {
        let before_recharge = recharge_balance;
        let before_gift = gift_balance;
        let before_total = before_recharge + before_gift;
        let credits_recharge = bucket == "recharge";
        if credits_recharge {
            recharge_balance += amount;
            total_recharged += amount;
        } else {
            gift_balance += amount;
        }
        let after_total = recharge_balance + gift_balance;
        if !before_total.is_finite()
            || !recharge_balance.is_finite()
            || !gift_balance.is_finite()
            || !total_recharged.is_finite()
            || !after_total.is_finite()
        {
            return Err(DataLayerError::UnexpectedValue(
                "wallet balance overflow for plan wallet_credit".to_string(),
            ));
        }
        sqlx::query(
            r#"
UPDATE wallets
SET balance = $2,
    gift_balance = $3,
    total_recharged = $4,
    updated_at = NOW()
WHERE id = $1
            "#,
        )
        .bind(wallet_id)
        .bind(recharge_balance)
        .bind(gift_balance)
        .bind(total_recharged)
        .execute(&mut **tx)
        .await
        .map_postgres_err()?;
        sqlx::query(
            r#"
INSERT INTO wallet_transactions (
  id, wallet_id, category, reason_code, amount, balance_before, balance_after,
  recharge_balance_before, recharge_balance_after, gift_balance_before,
  gift_balance_after, link_type, link_id, operator_id, description, created_at
)
VALUES (
  $1, $2, 'recharge', 'plan_wallet_credit', $3, $4, $5, $6, $7, $8, $9,
  'payment_order', $10, NULL, $11, NOW()
)
            "#,
        )
        .bind(Uuid::new_v4().to_string())
        .bind(wallet_id)
        .bind(amount)
        .bind(before_total)
        .bind(after_total)
        .bind(before_recharge)
        .bind(recharge_balance)
        .bind(before_gift)
        .bind(gift_balance)
        .bind(order_id)
        .bind(format!("套餐附赠余额({payment_method})"))
        .execute(&mut **tx)
        .await
        .map_postgres_err()?;
    }
    Ok(())
}

async fn update_payment_callback_failure(
    tx: &mut crate::PostgresTransaction,
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
    payload = NULL,
    processed_at = NOW()
WHERE id = $1
        "#,
    )
    .bind(callback_id)
    .bind(input.signature_valid)
    .bind(error)
    .bind(&input.payload_hash)
    .execute(&mut **tx)
    .await
    .map_postgres_err()?;
    Ok(())
}

async fn postgres_bind_payment_gateway_order_id(
    tx: &mut crate::PostgresTransaction,
    order_id: &str,
    gateway_order_id: &str,
) -> Result<bool, DataLayerError> {
    sqlx::query("SAVEPOINT payment_gateway_order_binding")
        .execute(&mut **tx)
        .await
        .map_postgres_err()?;
    let bind_result = sqlx::query("UPDATE payment_orders SET gateway_order_id = $2 WHERE id = $1")
        .bind(order_id)
        .bind(gateway_order_id)
        .execute(&mut **tx)
        .await;
    match bind_result {
        Ok(_) => {
            sqlx::query("RELEASE SAVEPOINT payment_gateway_order_binding")
                .execute(&mut **tx)
                .await
                .map_postgres_err()?;
            Ok(true)
        }
        Err(error)
            if error
                .as_database_error()
                .is_some_and(|database_error| database_error.is_unique_violation()) =>
        {
            sqlx::query("ROLLBACK TO SAVEPOINT payment_gateway_order_binding")
                .execute(&mut **tx)
                .await
                .map_postgres_err()?;
            sqlx::query("RELEASE SAVEPOINT payment_gateway_order_binding")
                .execute(&mut **tx)
                .await
                .map_postgres_err()?;
            Ok(false)
        }
        Err(error) => {
            let _ = sqlx::query("ROLLBACK TO SAVEPOINT payment_gateway_order_binding")
                .execute(&mut **tx)
                .await;
            let _ = sqlx::query("RELEASE SAVEPOINT payment_gateway_order_binding")
                .execute(&mut **tx)
                .await;
            Err(postgres_error(error))
        }
    }
}

async fn mark_payment_callback_processed(
    tx: &mut crate::PostgresTransaction,
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
    payload = NULL,
    processed_at = NOW(),
    order_no = $4,
    gateway_order_id = COALESCE($5, gateway_order_id)
WHERE id = $1
        "#,
    )
    .bind(callback_id)
    .bind(order_id)
    .bind(&input.payload_hash)
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

fn postgres_wallet_recharge_replay_matches(
    row: &PgRow,
    wallet_id: &str,
    input: &CreateWalletRechargeOrderInput,
) -> Result<bool, DataLayerError> {
    let existing_wallet_id: String = row_get(row, "wallet_id")?;
    let pay_currency: Option<String> = row_get(row, "pay_currency")?;
    let payment_method: String = row_get(row, "payment_method")?;
    let payment_provider: Option<String> = row_get(row, "payment_provider")?;
    let payment_channel: Option<String> = row_get(row, "payment_channel")?;
    Ok(wallet_recharge_replay_matches(
        &existing_wallet_id,
        row_get(row, "amount_usd")?,
        row_get(row, "pay_amount")?,
        pay_currency.as_deref(),
        row_get(row, "exchange_rate")?,
        &payment_method,
        payment_provider.as_deref(),
        payment_channel.as_deref(),
        wallet_id,
        input,
    ))
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
        payment_provider: row_get(row, "payment_provider")?,
        order_kind: row_get(row, "order_kind")?,
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

#[allow(clippy::too_many_arguments)]
async fn update_postgres_wallet_snapshot(
    pool: &PgPool,
    owner_column: &str,
    owner_id: &str,
    balance: f64,
    gift_balance: f64,
    limit_mode: &str,
    currency: &str,
    status: &str,
    total_recharged: f64,
    total_consumed: f64,
    total_refunded: f64,
    total_adjusted: f64,
    updated_at_unix_secs: Option<u64>,
) -> Result<(), DataLayerError> {
    let owner_predicate = match owner_column {
        "user_id" => "user_id = $1",
        "api_key_id" => "api_key_id = $1",
        _ => {
            return Err(DataLayerError::UnexpectedValue(format!(
                "unsupported wallet owner column: {owner_column}"
            )));
        }
    };
    let sql = format!(
        r#"
UPDATE wallets
SET balance = $2,
    gift_balance = $3,
    limit_mode = $4,
    currency = $5,
    status = $6,
    total_recharged = $7,
    total_consumed = $8,
    total_refunded = $9,
    total_adjusted = $10,
    updated_at = COALESCE(to_timestamp($11::DOUBLE PRECISION), NOW())
WHERE {owner_predicate}
"#
    );
    sqlx::query(&sql)
        .bind(owner_id)
        .bind(balance)
        .bind(gift_balance)
        .bind(limit_mode)
        .bind(currency)
        .bind(status)
        .bind(total_recharged)
        .bind(total_consumed)
        .bind(total_refunded)
        .bind(total_adjusted)
        .bind(updated_at_unix_secs.map(|value| value as i64))
        .execute(pool)
        .await
        .map_postgres_err()?;
    Ok(())
}

async fn initialize_postgres_auth_wallet(
    pool: &PgPool,
    user_id: Option<&str>,
    api_key_id: Option<&str>,
    initial_gift_usd: f64,
    unlimited: bool,
) -> Result<Option<(StoredWalletSnapshot, bool)>, DataLayerError> {
    let owner = user_id
        .or(api_key_id)
        .filter(|value| !value.trim().is_empty());
    if owner.is_none() || (user_id.is_some() && api_key_id.is_some()) {
        return Err(DataLayerError::InvalidInput(
            "wallet owner must be exactly one non-empty user or API-key id".to_string(),
        ));
    }
    if !initial_gift_usd.is_finite() {
        return Err(DataLayerError::InvalidInput(
            "initial gift amount must be finite".to_string(),
        ));
    }
    let gift_amount = if unlimited {
        0.0
    } else {
        initial_gift_usd.max(0.0)
    };
    let owner_column = if user_id.is_some() {
        "user_id"
    } else {
        "api_key_id"
    };
    let owner_value = owner.expect("validated wallet owner");
    let mut tx = pool.begin().await.map_postgres_err()?;

    // Keep the ownership lock order identical to the guarded user-deletion
    // path: users first, then api_keys/wallets. This closes the window where
    // a deletion can observe no wallet while a concurrent initializer inserts
    // one after the user row has been removed.
    if let Some(user_id) = user_id {
        let user_exists: Option<String> =
            sqlx::query_scalar("SELECT id FROM users WHERE id = $1 FOR UPDATE")
                .bind(user_id)
                .fetch_optional(&mut *tx)
                .await
                .map_postgres_err()?;
        if user_exists.is_none() {
            tx.rollback().await.map_postgres_err()?;
            return Ok(None);
        }
    } else {
        let api_key_user_id: Option<String> =
            sqlx::query_scalar("SELECT user_id FROM api_keys WHERE id = $1")
                .bind(owner_value)
                .fetch_optional(&mut *tx)
                .await
                .map_postgres_err()?;
        let Some(api_key_user_id) = api_key_user_id else {
            tx.rollback().await.map_postgres_err()?;
            return Ok(None);
        };
        let user_exists: Option<String> =
            sqlx::query_scalar("SELECT id FROM users WHERE id = $1 FOR UPDATE")
                .bind(&api_key_user_id)
                .fetch_optional(&mut *tx)
                .await
                .map_postgres_err()?;
        if user_exists.is_none() {
            tx.rollback().await.map_postgres_err()?;
            return Ok(None);
        }
        let api_key_exists: Option<String> =
            sqlx::query_scalar("SELECT id FROM api_keys WHERE id = $1 AND user_id = $2 FOR UPDATE")
                .bind(owner_value)
                .bind(&api_key_user_id)
                .fetch_optional(&mut *tx)
                .await
                .map_postgres_err()?;
        if api_key_exists.is_none() {
            tx.rollback().await.map_postgres_err()?;
            return Ok(None);
        }
    }
    let existing_sql = format!(
        "SELECT id, user_id, api_key_id, CAST(balance AS DOUBLE PRECISION) AS balance, CAST(gift_balance AS DOUBLE PRECISION) AS gift_balance, limit_mode, currency, status, CAST(total_recharged AS DOUBLE PRECISION) AS total_recharged, CAST(total_consumed AS DOUBLE PRECISION) AS total_consumed, CAST(total_refunded AS DOUBLE PRECISION) AS total_refunded, CAST(total_adjusted AS DOUBLE PRECISION) AS total_adjusted, CAST(EXTRACT(EPOCH FROM updated_at) AS BIGINT) AS updated_at_unix_secs FROM wallets WHERE {owner_column} = $1 LIMIT 1 FOR UPDATE"
    );
    if let Some(row) = sqlx::query(&existing_sql)
        .bind(owner_value)
        .fetch_optional(&mut *tx)
        .await
        .map_postgres_err()?
    {
        let existing = map_wallet_row(&row)?;
        tx.commit().await.map_err(postgres_error)?;
        return Ok(Some((existing, false)));
    }

    let wallet_row = sqlx::query(
        r#"
INSERT INTO wallets (
    id, user_id, api_key_id, balance, gift_balance, limit_mode, currency,
    status, total_recharged, total_consumed, total_refunded, total_adjusted,
    created_at, updated_at
)
VALUES ($1, $2, $3, 0, $4, $5, 'USD', 'active', 0, 0, 0, $6, NOW(), NOW())
ON CONFLICT DO NOTHING
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
    .bind(Uuid::new_v4().to_string())
    .bind(user_id)
    .bind(api_key_id)
    .bind(gift_amount)
    .bind(if unlimited { "unlimited" } else { "finite" })
    .bind(gift_amount)
    .fetch_optional(&mut *tx)
    .await
    .map_postgres_err()?;
    let Some(wallet_row) = wallet_row else {
        // Another initializer may have committed the owner row while this
        // transaction waited on the unique index. Read it under lock and
        // return it without creating a duplicate gift transaction.
        let Some(row) = sqlx::query(&existing_sql)
            .bind(owner_value)
            .fetch_optional(&mut *tx)
            .await
            .map_postgres_err()?
        else {
            tx.rollback().await.map_err(postgres_error)?;
            return Err(DataLayerError::InvalidInput(
                "wallet identifier already belongs to another owner".to_string(),
            ));
        };
        let existing = map_wallet_row(&row)?;
        tx.commit().await.map_err(postgres_error)?;
        return Ok(Some((existing, false)));
    };
    let wallet = map_wallet_row(&wallet_row)?;
    if gift_amount > 0.0 {
        let link_id = user_id.or(api_key_id).unwrap_or_default();
        let description = if api_key_id.is_some() {
            "独立余额 Key 初始赠款"
        } else {
            "用户初始赠款"
        };
        sqlx::query(
            r#"
INSERT INTO wallet_transactions (
    id, wallet_id, category, reason_code, amount, balance_before,
    balance_after, recharge_balance_before, recharge_balance_after,
    gift_balance_before, gift_balance_after, link_type, link_id, operator_id,
    description, created_at
)
VALUES ($1, $2, 'gift', 'gift_initial', $3, 0, $3, 0, 0, 0, $3, 'system_task', $4, NULL, $5, NOW())
"#,
        )
        .bind(Uuid::new_v4().to_string())
        .bind(&wallet.id)
        .bind(gift_amount)
        .bind(link_id)
        .bind(description)
        .execute(&mut *tx)
        .await
        .map_postgres_err()?;
    }
    tx.commit().await.map_err(postgres_error)?;
    Ok(Some((wallet, true)))
}

#[cfg(test)]
mod tests {
    use aether_data_contracts::repository::wallet::{
        CreateManualWalletRechargeInput, CreditAdminPaymentOrderInput, ProcessPaymentCallbackInput,
        ProcessPaymentCallbackOutcome, RedeemWalletCodeInput, RedeemWalletCodeOutcome,
        WalletLookupKey, WalletMutationOutcome, WalletReadRepository, WalletWriteRepository,
    };
    use sqlx::Row;

    use super::SqlxWalletRepository;
    use crate::{PostgresPoolConfig, PostgresPoolFactory};

    #[test]
    fn payment_order_sql_projections_cover_mapper_columns() {
        let source = include_str!("wallet.rs")
            .split("#[cfg(test)]")
            .next()
            .expect("wallet implementation should exist");
        let mapper = source
            .split("fn map_admin_payment_order_row(")
            .nth(1)
            .expect("payment order mapper should exist")
            .split("\nfn ")
            .next()
            .expect("payment order mapper body should exist");
        let required_columns = mapper
            .split("row_get(row, \"")
            .skip(1)
            .map(|read| read.split('"').next().expect("column name should exist"))
            .collect::<Vec<_>>();
        assert!(required_columns.contains(&"payment_provider"));
        assert!(required_columns.contains(&"order_kind"));

        let mut projections_checked = 0;
        for fragment in source.split("r#\"").skip(1) {
            let Some((sql, _)) = fragment.split_once("\"#") else {
                continue;
            };
            if !sql.contains("payment_orders")
                || !sql.contains("AS created_at_unix_ms")
                || !sql.contains("pay_currency")
            {
                continue;
            }
            let projection = match sql.rsplit_once("RETURNING") {
                Some((_, projection)) => projection.to_string(),
                None => sql
                    .lines()
                    .take_while(|line| !line.trim_start().starts_with("FROM payment_orders"))
                    .collect::<Vec<_>>()
                    .join("\n"),
            };
            let tokens = projection
                .split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
                .collect::<Vec<_>>();
            for column in &required_columns {
                assert!(
                    tokens.contains(column),
                    "payment order projection omits {column}: {sql}"
                );
            }
            projections_checked += 1;
        }
        assert!(
            projections_checked > 0,
            "payment order projections should exist"
        );
    }

    async fn isolated_wallet_test_pool() -> sqlx::PgPool {
        let database_url = std::env::var("AETHER_TEST_DATABASE_URL")
            .expect("AETHER_TEST_DATABASE_URL must point at the test database");
        let options = database_url
            .parse::<sqlx::postgres::PgConnectOptions>()
            .expect("test database URL should parse")
            .options([("search_path", "pg_temp")]);
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .idle_timeout(None)
            .max_lifetime(None)
            .connect_with(options)
            .await
            .expect("test database should connect");
        for table in [
            "api_keys",
            "wallets",
            "payment_orders",
            "payment_callbacks",
            "wallet_transactions",
            "user_plan_entitlements",
            "redeem_code_batches",
            "redeem_codes",
        ] {
            sqlx::query(&format!(
                "CREATE TEMP TABLE {table} (LIKE public.{table} INCLUDING ALL)"
            ))
            .execute(&pool)
            .await
            .expect("isolated wallet table should be created");
        }
        pool
    }

    async fn seed_wallet(pool: &sqlx::PgPool) -> (String, String) {
        let wallet_id = uuid::Uuid::new_v4().to_string();
        let user_id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO wallets (id, user_id, balance, gift_balance, total_recharged, created_at, updated_at) VALUES ($1, $2, 10, 3, 20, NOW(), NOW())",
        )
        .bind(&wallet_id)
        .bind(&user_id)
        .execute(pool)
        .await
        .expect("test wallet should be created");
        (wallet_id, user_id)
    }

    async fn seed_pending_order(
        pool: &sqlx::PgPool,
        wallet_id: &str,
        user_id: &str,
        order_kind: &str,
    ) -> String {
        let order_id = uuid::Uuid::new_v4().to_string();
        let plan_snapshot = (order_kind == "plan_purchase").then(|| {
            serde_json::json!({
                "id": "test-plan",
                "duration_days": 30,
                "purchase_limit_scope": "unlimited",
                "entitlements": [{
                    "type": "wallet_credit",
                    "amount_usd": 4.0,
                    "balance_bucket": "gift",
                }],
            })
        });
        sqlx::query(
            "INSERT INTO payment_orders (id, order_no, wallet_id, user_id, amount_usd, pay_amount, pay_currency, payment_method, payment_provider, payment_channel, order_kind, product_id, product_snapshot, status, created_at, expires_at) VALUES ($1, $2, $3, $4, 5, 5, 'USD', 'stripe', 'stripe', 'card', $5, $6, $7, 'pending', NOW(), NOW() + INTERVAL '1 hour')",
        )
        .bind(&order_id)
        .bind(format!("order-{order_id}"))
        .bind(wallet_id)
        .bind(user_id)
        .bind(order_kind)
        .bind(plan_snapshot.as_ref().map(|_| "test-plan"))
        .bind(plan_snapshot)
        .execute(pool)
        .await
        .expect("pending payment order should be created");
        order_id
    }

    fn payment_callback_input(order_id: &str) -> ProcessPaymentCallbackInput {
        ProcessPaymentCallbackInput {
            payment_method: "stripe".to_string(),
            payment_provider: Some("stripe".to_string()),
            payment_channel: Some("card".to_string()),
            callback_key: format!("stripe:event-{order_id}"),
            order_no: Some(format!("order-{order_id}")),
            gateway_order_id: Some(format!("gateway-{order_id}")),
            amount_usd: 5.0,
            pay_amount: Some(5.0),
            pay_currency: Some("USD".to_string()),
            exchange_rate: Some(1.0),
            payload_hash: "payment-callback-regression".to_string(),
            payload: serde_json::json!({"status": "success"}),
            signature_valid: true,
        }
    }

    async fn make_api_key_wallet(pool: &sqlx::PgPool, wallet_id: &str, user_id: &str) -> String {
        let api_key_id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO api_keys (id, user_id, key_hash, created_at, updated_at) VALUES ($1, $2, $1, NOW(), NOW())",
        )
        .bind(&api_key_id)
        .bind(user_id)
        .execute(pool)
        .await
        .expect("test API key should be created");
        sqlx::query("UPDATE wallets SET user_id = NULL, api_key_id = $2 WHERE id = $1")
            .bind(wallet_id)
            .bind(&api_key_id)
            .execute(pool)
            .await
            .expect("test wallet should belong to the API key");
        api_key_id
    }

    async fn assert_payment_callback_credits_once(api_key_wallet: bool) {
        let pool = isolated_wallet_test_pool().await;
        let (wallet_id, user_id) = seed_wallet(&pool).await;
        if api_key_wallet {
            make_api_key_wallet(&pool, &wallet_id, &user_id).await;
        }
        let order_id = seed_pending_order(&pool, &wallet_id, &user_id, "wallet_recharge").await;
        let repository = SqlxWalletRepository::new(pool.clone());
        let input = payment_callback_input(&order_id);
        let outcome = repository
            .process_payment_callback(input.clone())
            .await
            .expect("verified payment callback should commit");
        let ProcessPaymentCallbackOutcome::Applied {
            order, duplicate, ..
        } = outcome
        else {
            panic!("pending payment order should be credited: {outcome:?}");
        };
        assert!(!duplicate);
        assert_eq!(order.status, "credited");
        assert_eq!(order.wallet_id, wallet_id);
        assert_eq!(order.amount_usd, 5.0);
        assert!(order.credited_at_unix_secs.is_some());
        assert_eq!(
            repository
                .process_payment_callback(input.clone())
                .await
                .expect("duplicate callback should succeed"),
            ProcessPaymentCallbackOutcome::DuplicateProcessed {
                order_id: Some(order_id.clone()),
            }
        );
        let another_event = ProcessPaymentCallbackInput {
            callback_key: format!("stripe:another-event-{order_id}"),
            ..input
        };
        assert!(matches!(
            repository
                .process_payment_callback(another_event)
                .await
                .expect("another event for a credited order should succeed"),
            ProcessPaymentCallbackOutcome::AlreadyCredited { .. }
        ));
        let wallet = repository
            .find(WalletLookupKey::WalletId(&wallet_id))
            .await
            .expect("wallet should be readable")
            .expect("wallet should persist");
        assert_eq!(wallet.balance, 15.0);
        assert_eq!(wallet.gift_balance, 3.0);
        assert_eq!(wallet.total_recharged, 25.0);
        let transactions = sqlx::query(
            "SELECT reason_code, CAST(amount AS DOUBLE PRECISION) AS amount, CAST(balance_before AS DOUBLE PRECISION) AS balance_before, CAST(balance_after AS DOUBLE PRECISION) AS balance_after, link_id FROM wallet_transactions WHERE wallet_id = $1",
        )
        .bind(&wallet_id)
        .fetch_all(&pool)
        .await
        .expect("payment transactions should be readable");
        assert_eq!(transactions.len(), 1);
        assert_eq!(
            transactions[0].get::<String, _>("reason_code"),
            "topup_gateway"
        );
        assert_eq!(transactions[0].get::<f64, _>("amount"), 5.0);
        assert_eq!(transactions[0].get::<f64, _>("balance_before"), 13.0);
        assert_eq!(transactions[0].get::<f64, _>("balance_after"), 18.0);
        assert_eq!(transactions[0].get::<String, _>("link_id"), order_id);
        let processed_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM payment_callbacks WHERE payment_order_id = $1 AND status = 'processed'",
        )
        .bind(&order_id)
        .fetch_one(&pool)
        .await
        .expect("processed callbacks should persist");
        assert_eq!(processed_count, 2);
        pool.close().await;
    }

    #[tokio::test]
    #[ignore = "requires AETHER_TEST_DATABASE_URL and PostgreSQL bootstrap schema"]
    async fn live_payment_callback_user_wallet_credits_once() {
        assert_payment_callback_credits_once(false).await;
    }

    #[tokio::test]
    #[ignore = "requires AETHER_TEST_DATABASE_URL and PostgreSQL bootstrap schema"]
    async fn live_payment_callback_api_key_wallet_credits_once() {
        assert_payment_callback_credits_once(true).await;
    }

    #[tokio::test]
    #[ignore = "requires AETHER_TEST_DATABASE_URL and PostgreSQL bootstrap schema"]
    async fn live_payment_callback_rejects_wrong_or_missing_wallet_owner() {
        for owner_case in ["wrong_user", "wrong_api_key_user", "missing_api_key"] {
            let pool = isolated_wallet_test_pool().await;
            let (wallet_id, user_id) = seed_wallet(&pool).await;
            let order_id = seed_pending_order(&pool, &wallet_id, &user_id, "wallet_recharge").await;
            let other_user_id = uuid::Uuid::new_v4().to_string();
            if owner_case == "wrong_user" {
                sqlx::query("UPDATE wallets SET user_id = $2 WHERE id = $1")
                    .bind(&wallet_id)
                    .bind(&other_user_id)
                    .execute(&pool)
                    .await
                    .expect("wallet owner should change for the rejection test");
            } else {
                let api_key_id = make_api_key_wallet(&pool, &wallet_id, &other_user_id).await;
                if owner_case == "missing_api_key" {
                    sqlx::query("DELETE FROM api_keys WHERE id = $1")
                        .bind(api_key_id)
                        .execute(&pool)
                        .await
                        .expect("test API key should be removed");
                }
            }
            let repository = SqlxWalletRepository::new(pool.clone());
            assert_eq!(
                repository
                    .process_payment_callback(payment_callback_input(&order_id))
                    .await
                    .expect("owner mismatch should return a durable rejection"),
                ProcessPaymentCallbackOutcome::Failed {
                    duplicate: false,
                    error: "payment order wallet owner mismatch".to_string(),
                },
                "owner case: {owner_case}"
            );
            let wallet = repository
                .find(WalletLookupKey::WalletId(&wallet_id))
                .await
                .expect("wallet should be readable")
                .expect("wallet should persist");
            assert_eq!(wallet.balance, 10.0);
            assert_eq!(wallet.total_recharged, 20.0);
            let order = repository
                .find_admin_payment_order(&order_id)
                .await
                .expect("order should be readable")
                .expect("order should persist");
            assert_eq!(order.status, "pending");
            assert!(order.gateway_order_id.is_none());
            let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM wallet_transactions")
                .fetch_one(&pool)
                .await
                .expect("transactions should be readable");
            assert_eq!(count, 0);
            let callback_status: String =
                sqlx::query_scalar("SELECT status FROM payment_callbacks")
                    .fetch_one(&pool)
                    .await
                    .expect("rejected callback should persist");
            assert_eq!(callback_status, "failed");
            pool.close().await;
        }
    }

    #[tokio::test]
    #[ignore = "requires AETHER_TEST_DATABASE_URL and PostgreSQL bootstrap schema"]
    async fn live_manual_recharge_commits_wallet_order_and_transaction() {
        let pool = isolated_wallet_test_pool().await;
        let (wallet_id, user_id) = seed_wallet(&pool).await;
        let repository = SqlxWalletRepository::new(pool.clone());
        let input = CreateManualWalletRechargeInput {
            wallet_id: wallet_id.clone(),
            amount_usd: 5.0,
            payment_method: "admin_manual".to_string(),
            operator_id: Some(uuid::Uuid::new_v4().to_string()),
            description: Some("manual recharge regression".to_string()),
            order_no: format!("manual-{}", uuid::Uuid::new_v4()),
        };
        let (wallet, order) = repository
            .create_manual_wallet_recharge(input.clone())
            .await
            .expect("manual recharge should commit")
            .expect("test wallet should exist");

        assert_eq!(wallet.id, wallet_id);
        assert_eq!(wallet.user_id.as_deref(), Some(user_id.as_str()));
        assert_eq!(wallet.balance, 15.0);
        assert_eq!(wallet.gift_balance, 3.0);
        assert_eq!(wallet.total_recharged, 25.0);
        assert_eq!(order.order_no, input.order_no);
        assert_eq!(order.wallet_id, wallet_id);
        assert_eq!(order.user_id.as_deref(), Some(user_id.as_str()));
        assert_eq!(order.amount_usd, 5.0);
        assert_eq!(order.refunded_amount_usd, 0.0);
        assert_eq!(order.refundable_amount_usd, 5.0);
        assert_eq!(order.payment_method, "admin_manual");
        assert_eq!(order.payment_provider, None);
        assert_eq!(order.order_kind, "wallet_recharge");
        assert_eq!(order.status, "credited");
        assert!(order.paid_at_unix_secs.is_some());
        assert!(order.credited_at_unix_secs.is_some());
        assert_eq!(
            order.gateway_response,
            Some(serde_json::json!({
                "source": "manual",
                "operator_id": input.operator_id,
                "description": input.description,
            }))
        );

        assert!(repository
            .create_manual_wallet_recharge(input.clone())
            .await
            .is_err());
        for amount_usd in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert!(repository
                .create_manual_wallet_recharge(CreateManualWalletRechargeInput {
                    amount_usd,
                    ..input.clone()
                })
                .await
                .is_err());
        }
        assert!(repository
            .create_manual_wallet_recharge(CreateManualWalletRechargeInput {
                wallet_id: uuid::Uuid::new_v4().to_string(),
                ..input.clone()
            })
            .await
            .expect("missing wallet should not fail")
            .is_none());

        let persisted_wallet = repository
            .find(WalletLookupKey::WalletId(&wallet_id))
            .await
            .expect("wallet should be readable after commit")
            .expect("wallet should persist");
        assert_eq!(persisted_wallet, wallet);
        let persisted_order = repository
            .find_admin_payment_order(&order.id)
            .await
            .expect("payment order should be readable after commit")
            .expect("payment order should persist");
        assert_eq!(persisted_order, order);

        let transaction = sqlx::query(
            "SELECT category, reason_code, CAST(amount AS DOUBLE PRECISION) AS amount, CAST(balance_before AS DOUBLE PRECISION) AS balance_before, CAST(balance_after AS DOUBLE PRECISION) AS balance_after, CAST(recharge_balance_before AS DOUBLE PRECISION) AS recharge_balance_before, CAST(recharge_balance_after AS DOUBLE PRECISION) AS recharge_balance_after, CAST(gift_balance_before AS DOUBLE PRECISION) AS gift_balance_before, CAST(gift_balance_after AS DOUBLE PRECISION) AS gift_balance_after, link_type, link_id, operator_id, description FROM wallet_transactions WHERE wallet_id = $1",
        )
        .bind(&wallet_id)
        .fetch_one(&pool)
        .await
        .expect("recharge transaction should persist");
        assert_eq!(transaction.get::<String, _>("category"), "recharge");
        assert_eq!(
            transaction.get::<String, _>("reason_code"),
            "topup_admin_manual"
        );
        assert_eq!(transaction.get::<f64, _>("amount"), 5.0);
        assert_eq!(transaction.get::<f64, _>("balance_before"), 13.0);
        assert_eq!(transaction.get::<f64, _>("balance_after"), 18.0);
        assert_eq!(transaction.get::<f64, _>("recharge_balance_before"), 10.0);
        assert_eq!(transaction.get::<f64, _>("recharge_balance_after"), 15.0);
        assert_eq!(transaction.get::<f64, _>("gift_balance_before"), 3.0);
        assert_eq!(transaction.get::<f64, _>("gift_balance_after"), 3.0);
        assert_eq!(transaction.get::<String, _>("link_type"), "payment_order");
        assert_eq!(transaction.get::<String, _>("link_id"), order.id);
        assert_eq!(
            transaction.get::<Option<String>, _>("operator_id"),
            input.operator_id
        );
        assert_eq!(
            transaction.get::<Option<String>, _>("description"),
            input.description
        );
        for table in ["wallets", "payment_orders", "wallet_transactions"] {
            let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
                .fetch_one(&pool)
                .await
                .expect("wallet record count should be readable");
            assert_eq!(count, 1, "rejected recharges must not add {table} rows");
        }
        pool.close().await;
    }

    #[tokio::test]
    #[ignore = "requires AETHER_TEST_DATABASE_URL and PostgreSQL bootstrap schema"]
    async fn live_admin_order_state_changes_preserve_metadata() {
        let pool = isolated_wallet_test_pool().await;
        let (wallet_id, user_id) = seed_wallet(&pool).await;
        let repository = SqlxWalletRepository::new(pool.clone());
        for target_status in ["expired", "failed"] {
            let order_id = seed_pending_order(&pool, &wallet_id, &user_id, "wallet_recharge").await;
            let order = if target_status == "expired" {
                let outcome = repository
                    .expire_admin_payment_order(&order_id)
                    .await
                    .expect("order expiry should commit");
                let WalletMutationOutcome::Applied((order, changed)) = outcome else {
                    panic!("pending order should expire");
                };
                assert!(changed);
                assert!(matches!(
                    repository.expire_admin_payment_order(&order_id).await,
                    Ok(WalletMutationOutcome::Applied((_, false)))
                ));
                order
            } else {
                let outcome = repository
                    .fail_admin_payment_order(&order_id)
                    .await
                    .expect("order failure should commit");
                let WalletMutationOutcome::Applied(order) = outcome else {
                    panic!("pending order should be marked failed");
                };
                order
            };
            assert_eq!(order.status, target_status);
            assert_eq!(order.payment_provider.as_deref(), Some("stripe"));
            assert_eq!(order.order_kind, "wallet_recharge");
            assert_eq!(
                repository
                    .find_admin_payment_order(&order_id)
                    .await
                    .unwrap(),
                Some(order)
            );
        }
        let wallet = repository
            .find(WalletLookupKey::WalletId(&wallet_id))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            (wallet.balance, wallet.gift_balance, wallet.total_recharged),
            (10.0, 3.0, 20.0)
        );
        let transaction_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM wallet_transactions")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(transaction_count, 0);
        pool.close().await;
    }

    async fn assert_admin_payment_order_credit(order_kind: &str) {
        let pool = isolated_wallet_test_pool().await;
        let (wallet_id, user_id) = seed_wallet(&pool).await;
        let order_id = seed_pending_order(&pool, &wallet_id, &user_id, order_kind).await;
        let repository = SqlxWalletRepository::new(pool.clone());
        let input = CreditAdminPaymentOrderInput {
            order_id: order_id.clone(),
            gateway_order_id: None,
            pay_amount: None,
            pay_currency: None,
            exchange_rate: None,
            gateway_response_patch: None,
            operator_id: Some(uuid::Uuid::new_v4().to_string()),
        };
        let outcome = repository
            .credit_admin_payment_order(input.clone())
            .await
            .expect("admin credit should commit");
        let WalletMutationOutcome::Applied((order, changed)) = outcome else {
            panic!("pending payment order should be credited");
        };
        assert!(changed);
        assert_eq!(order.status, "credited");
        assert_eq!(order.payment_provider.as_deref(), Some("stripe"));
        assert_eq!(order.order_kind, order_kind);
        assert!(order.paid_at_unix_secs.is_some());
        assert!(order.credited_at_unix_secs.is_some());
        assert_eq!(
            repository
                .find_admin_payment_order(&order_id)
                .await
                .unwrap(),
            Some(order.clone())
        );
        assert!(matches!(
            repository.credit_admin_payment_order(input).await,
            Ok(WalletMutationOutcome::Applied((_, false)))
        ));
        assert!(matches!(
            repository.expire_admin_payment_order(&order_id).await,
            Ok(WalletMutationOutcome::Invalid(_))
        ));
        assert!(matches!(
            repository.fail_admin_payment_order(&order_id).await,
            Ok(WalletMutationOutcome::Invalid(_))
        ));

        let wallet = repository
            .find(WalletLookupKey::WalletId(&wallet_id))
            .await
            .unwrap()
            .unwrap();
        let expected_balances = if order_kind == "plan_purchase" {
            assert_eq!(order.refundable_amount_usd, 0.0);
            let fulfillment: String =
                sqlx::query_scalar("SELECT fulfillment_status FROM payment_orders WHERE id = $1")
                    .bind(&order_id)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(fulfillment, "fulfilled");
            (10.0, 7.0, 20.0)
        } else {
            assert_eq!(order.refundable_amount_usd, 5.0);
            (15.0, 3.0, 25.0)
        };
        assert_eq!(
            (wallet.balance, wallet.gift_balance, wallet.total_recharged),
            expected_balances
        );
        let transaction_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM wallet_transactions WHERE wallet_id = $1")
                .bind(&wallet_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(transaction_count, 1);
        let entitlement_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM user_plan_entitlements WHERE payment_order_id = $1",
        )
        .bind(&order_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(entitlement_count, i64::from(order_kind == "plan_purchase"));
        pool.close().await;
    }

    #[tokio::test]
    #[ignore = "requires AETHER_TEST_DATABASE_URL and PostgreSQL bootstrap schema"]
    async fn live_admin_wallet_order_credit_commits_once() {
        assert_admin_payment_order_credit("wallet_recharge").await;
    }

    #[tokio::test]
    #[ignore = "requires AETHER_TEST_DATABASE_URL and PostgreSQL bootstrap schema"]
    async fn live_admin_plan_order_credit_commits_once() {
        assert_admin_payment_order_credit("plan_purchase").await;
    }

    #[tokio::test]
    #[ignore = "requires AETHER_TEST_DATABASE_URL and PostgreSQL bootstrap schema"]
    async fn live_redeem_code_commits_order_and_wallet_once_for_each_bucket() {
        let pool = isolated_wallet_test_pool().await;
        let repository = SqlxWalletRepository::new(pool.clone());
        for balance_bucket in ["recharge", "gift"] {
            let (wallet_id, user_id) = seed_wallet(&pool).await;
            let batch_id = uuid::Uuid::new_v4().to_string();
            let code_id = uuid::Uuid::new_v4().to_string();
            let code = super::generate_redeem_code_normalized();
            sqlx::query(
                "INSERT INTO redeem_code_batches (id, name, amount_usd, balance_bucket, total_count, created_at, updated_at) VALUES ($1, 'regression batch', 5, $2, 1, NOW(), NOW())",
            )
            .bind(&batch_id)
            .bind(balance_bucket)
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO redeem_codes (id, batch_id, code_hash, code_prefix, code_suffix, created_at, updated_at) VALUES ($1, $2, $3, $4, $5, NOW(), NOW())",
            )
            .bind(&code_id)
            .bind(&batch_id)
            .bind(super::hash_redeem_code(&code))
            .bind(super::redeem_code_prefix(&code))
            .bind(super::redeem_code_suffix(&code))
            .execute(&pool)
            .await
            .unwrap();
            let input = RedeemWalletCodeInput {
                code: super::format_redeem_code(&code),
                user_id,
                order_no: format!("redeem-{}", uuid::Uuid::new_v4()),
            };
            let outcome = repository
                .redeem_wallet_code(input.clone())
                .await
                .expect("redeem code recharge should commit");
            let RedeemWalletCodeOutcome::Redeemed {
                wallet,
                order,
                amount_usd,
                ..
            } = outcome
            else {
                panic!("active code should be redeemed");
            };
            assert_eq!(amount_usd, 5.0);
            assert_eq!(order.status, "credited");
            assert_eq!(order.order_kind, "wallet_recharge");
            assert_eq!(order.payment_provider, None);
            let expected_balances = if balance_bucket == "recharge" {
                assert_eq!(order.payment_method, "card_code");
                assert_eq!(order.refundable_amount_usd, 5.0);
                (15.0, 3.0, 25.0)
            } else {
                assert_eq!(order.payment_method, "gift_code");
                assert_eq!(order.refundable_amount_usd, 0.0);
                (10.0, 8.0, 25.0)
            };
            assert_eq!(
                (wallet.balance, wallet.gift_balance, wallet.total_recharged),
                expected_balances
            );
            assert!(matches!(
                repository.redeem_wallet_code(input).await,
                Ok(RedeemWalletCodeOutcome::CodeRedeemed)
            ));
            assert_eq!(
                repository
                    .find(WalletLookupKey::WalletId(&wallet_id))
                    .await
                    .unwrap(),
                Some(wallet)
            );
            assert_eq!(
                repository
                    .find_admin_payment_order(&order.id)
                    .await
                    .unwrap(),
                Some(order.clone())
            );
            let redeemed = sqlx::query(
                "SELECT status, redeemed_wallet_id, redeemed_payment_order_id FROM redeem_codes WHERE id = $1",
            )
            .bind(&code_id)
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(redeemed.get::<String, _>("status"), "redeemed");
            assert_eq!(redeemed.get::<String, _>("redeemed_wallet_id"), wallet_id);
            assert_eq!(
                redeemed.get::<String, _>("redeemed_payment_order_id"),
                order.id
            );
            for table in ["payment_orders", "wallet_transactions"] {
                let count: i64 = sqlx::query_scalar(&format!(
                    "SELECT COUNT(*) FROM {table} WHERE wallet_id = $1"
                ))
                .bind(&wallet_id)
                .fetch_one(&pool)
                .await
                .unwrap();
                assert_eq!(count, 1, "redeeming twice must not duplicate {table}");
            }
        }
        pool.close().await;
    }

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
