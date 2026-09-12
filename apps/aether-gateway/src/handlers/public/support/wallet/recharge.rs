use super::super::mark_sensitive_response_no_store;
use super::super::support_payment::payment_epay::{
    build_epay_checkout_url, configured_epay_channels, epay_callback_base_url, load_epay_config,
    resolve_epay_channel, EpayCheckoutInput,
};
use super::super::support_payment::payment_gateway::{
    CreateCheckoutSessionInput, PaymentGatewayRegistry,
};
use super::{
    build_auth_error_response, build_auth_json_response, build_wallet_payload,
    build_wallet_recharge_storage_unavailable_response, http, parse_wallet_limit,
    parse_wallet_offset, resolve_authenticated_local_user, unix_secs_to_rfc3339,
    wallet_normalize_optional_string_field, AppState, Body, GatewayPublicRequestContext, Response,
    WALLET_SAFE_GATEWAY_RESPONSE_KEYS,
};

const MAX_PAYMENT_GATEWAY_RESPONSE_BYTES: usize = 1024 * 1024;
const WALLET_RECHARGE_ORDER_KIND: &str = "wallet_recharge";
const PAYMENT_ORDER_STRIPE_SECRET_MIGRATION_RETRIES: usize = 8;
const PAYMENT_AMOUNT_EPSILON: f64 = 0.00000001;
const STRIPE_WALLET_IDEMPOTENCY_PREFIX: &str = "aether-payment-intent-";
const STRIPE_CHECKOUT_UNCERTAIN_DETAIL: &str = "Stripe 支付服务暂时不可用";
const STRIPE_CHECKOUT_FAILED_DETAIL: &str = "Stripe 支付请求被拒绝";
const WALLET_SAFE_EPAY_PAYMENT_PARAM_KEYS: &[&str] = &[
    "pid",
    "type",
    "out_trade_no",
    "notify_url",
    "return_url",
    "name",
    "money",
    "sign_type",
    "sign",
];

#[derive(Debug, Clone, PartialEq, Eq)]
enum StripeWalletCheckoutError {
    Canceled,
    /// The provider request may have been accepted, but the local process
    /// could not obtain a trustworthy response. Retrying with another gateway
    /// identity could strand the first payment.
    Uncertain(String),
    Failed(String),
}

impl From<String> for StripeWalletCheckoutError {
    fn from(value: String) -> Self {
        Self::Failed(value)
    }
}

fn stripe_wallet_checkout_response_is_canceled(value: &Value) -> bool {
    value
        .get("status")
        .and_then(Value::as_str)
        .is_some_and(|status| status.trim().eq_ignore_ascii_case("canceled"))
}

/// Keep the local merchant order number stable while rotating the provider
/// idempotency key once when Stripe has retained a canceled intent.  The
/// suffix is deterministic so a later request can recover a response that was
/// lost after the retry was accepted, without creating a third intent.
fn stripe_wallet_idempotency_key(order_no: &str, retry: bool) -> String {
    if retry {
        format!("{STRIPE_WALLET_IDEMPOTENCY_PREFIX}{order_no}-retry-1")
    } else {
        format!("{STRIPE_WALLET_IDEMPOTENCY_PREFIX}{order_no}")
    }
}

#[cfg(test)]
use super::{
    record_wallet_test_recharge, wallet_test_recharge_order_by_id,
    wallet_test_recharge_order_by_order_no, wallet_test_recharge_orders_for_user,
};
use crate::handlers::shared::{
    create_alipay_direct_checkout, create_wxpay_direct_checkout, normalize_payment_currency,
    normalize_stripe_client_secret, open_payment_order_stripe_client_secret,
    public_payment_http_client, seal_payment_order_stripe_client_secret,
    DirectPaymentCheckoutError, DirectPaymentCheckoutInput, PaymentOrderStripeSecretBinding,
    STRIPE_CLIENT_SECRET_ENCRYPTED_KEY,
};
use chrono::Utc;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use aether_data::repository::wallet::{
    wallet_recharge_checkout_claim_response, wallet_recharge_order_created_at_unix_secs,
    wallet_recharge_order_is_checkout_placeholder,
    wallet_recharge_order_is_reclaimable_placeholder, FailWalletRechargeCheckoutInput,
    ReclaimWalletRechargeCheckoutInput,
};

#[derive(Debug, Deserialize)]
struct WalletCreateRechargeRequest {
    amount_usd: f64,
    payment_method: String,
    #[serde(default)]
    payment_provider: Option<String>,
    #[serde(default)]
    payment_channel: Option<String>,
    #[serde(default)]
    pay_amount: Option<f64>,
    #[serde(default)]
    pay_currency: Option<String>,
    #[serde(default)]
    exchange_rate: Option<f64>,
    #[serde(default)]
    idempotency_key: Option<String>,
}

#[derive(Debug, Clone)]
struct NormalizedWalletCreateRechargeRequest {
    amount_usd: f64,
    payment_method: String,
    payment_provider: Option<String>,
    payment_channel: Option<String>,
    pay_amount: Option<f64>,
    pay_currency: Option<String>,
    exchange_rate: Option<f64>,
    idempotency_key: Option<String>,
}

fn normalize_wallet_create_recharge_request(
    payload: WalletCreateRechargeRequest,
) -> Result<NormalizedWalletCreateRechargeRequest, &'static str> {
    if !payload.amount_usd.is_finite() || payload.amount_usd <= 0.0 {
        return Err("输入验证失败");
    }
    let payment_method = payload.payment_method.trim().to_ascii_lowercase();
    if payment_method.is_empty() || payment_method.chars().count() > 30 {
        return Err("输入验证失败");
    }
    let payment_provider = wallet_normalize_optional_string_field(payload.payment_provider, 30)?
        .map(|value| value.to_ascii_lowercase());
    let payment_channel = wallet_normalize_optional_string_field(payload.payment_channel, 30)?
        .map(|value| value.to_ascii_lowercase());
    match payment_provider.as_deref() {
        Some("epay") => {
            // EPay is an aggregator. Older clients used alipay/wxpay as the
            // method to select the channel, while newer clients send epay for
            // both method and provider and carry the channel separately.
            if !matches!(payment_method.as_str(), "epay" | "alipay" | "wxpay") {
                return Err("输入验证失败");
            }
            if payment_method != "epay"
                && payment_channel
                    .as_deref()
                    .is_some_and(|channel| channel != payment_method)
            {
                return Err("输入验证失败");
            }
        }
        Some(provider) if provider != payment_method => {
            // Do not let a request select one gateway while recording another
            // payment namespace.
            return Err("输入验证失败");
        }
        _ => {}
    }
    if matches!(payload.pay_amount, Some(value) if !value.is_finite() || value <= 0.0) {
        return Err("输入验证失败");
    }
    if matches!(payload.exchange_rate, Some(value) if !value.is_finite() || value <= 0.0) {
        return Err("输入验证失败");
    }
    let pay_currency = wallet_normalize_optional_string_field(payload.pay_currency, 3)?;
    if matches!(pay_currency.as_deref(), Some(value) if value.chars().count() != 3) {
        return Err("输入验证失败");
    }

    Ok(NormalizedWalletCreateRechargeRequest {
        amount_usd: payload.amount_usd,
        payment_method,
        payment_provider,
        payment_channel,
        pay_amount: payload.pay_amount,
        pay_currency,
        exchange_rate: payload.exchange_rate,
        idempotency_key: wallet_normalize_optional_string_field(payload.idempotency_key, 128)?,
    })
}

fn wallet_build_order_no(now: chrono::DateTime<chrono::Utc>) -> String {
    format!(
        "po_{}_{}",
        now.format("%Y%m%d%H%M%S%6f"),
        &Uuid::new_v4().simple().to_string()[..12]
    )
}

fn wallet_build_idempotent_order_no(user_id: &str, idempotency_key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"wallet-recharge:");
    hasher.update(user_id.trim().as_bytes());
    hasher.update([0]);
    hasher.update(idempotency_key.trim().as_bytes());
    let digest = hasher.finalize();
    // payment_orders.order_no is limited to 64 bytes. Keep the stable prefix
    // and 56 hex characters (28 digest bytes) within that database contract.
    let digest_hex: String = digest[..28]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("po_idem_{digest_hex}")
}

fn wallet_recharge_order_no(
    user_id: &str,
    idempotency_key: Option<&str>,
    now: chrono::DateTime<chrono::Utc>,
) -> String {
    idempotency_key
        .map(|key| wallet_build_idempotent_order_no(user_id, key))
        .unwrap_or_else(|| wallet_build_order_no(now))
}

fn wallet_payment_return_url(callback_base_url: &str, provider: &str, order_no: &str) -> String {
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    serializer.append_pair("payment_provider", provider);
    serializer.append_pair("payment_status", "pending");
    serializer.append_pair("order_no", order_no);
    format!(
        "{callback_base_url}/dashboard/wallet?{}",
        serializer.finish()
    )
}

fn wallet_order_id_from_path(request_path: &str) -> Option<String> {
    let trimmed = request_path.trim_end_matches('/');
    let order_id = trimmed.strip_prefix("/api/wallet/recharge/")?.trim();
    if order_id.is_empty() || order_id.contains('/') {
        None
    } else {
        Some(order_id.to_string())
    }
}

pub(super) fn wallet_recharge_detail_path_matches(request_path: &str) -> bool {
    wallet_order_id_from_path(request_path).is_some()
}

fn remove_stripe_client_secret_fields(value: &mut Value) {
    match value {
        Value::Object(object) => {
            object.remove("client_secret");
            object.remove(STRIPE_CLIENT_SECRET_ENCRYPTED_KEY);
            for item in object.values_mut() {
                remove_stripe_client_secret_fields(item);
            }
        }
        Value::Array(items) => {
            for item in items {
                remove_stripe_client_secret_fields(item);
            }
        }
        _ => {}
    }
}

fn sanitize_wallet_payment_params(value: &Value) -> Option<Value> {
    let object = value.as_object()?;
    let mut projected = serde_json::Map::new();
    for key in WALLET_SAFE_EPAY_PAYMENT_PARAM_KEYS {
        let Some(item) = object.get(*key) else {
            continue;
        };
        // EPay signs and submits string parameters. Reject objects/arrays here
        // so a provider response cannot smuggle an arbitrary nested payload
        // through an otherwise allow-listed field.
        if item.is_string() {
            projected.insert((*key).to_string(), item.clone());
        }
    }
    (!projected.is_empty()).then_some(Value::Object(projected))
}

fn sanitize_wallet_gateway_value(key: &str, value: &Value) -> Option<Value> {
    match key {
        "payment_params" => sanitize_wallet_payment_params(value),
        "payment_method_types" => {
            let values = value.as_array()?;
            let projected = values
                .iter()
                .filter_map(Value::as_str)
                .filter(|item| !item.trim().is_empty())
                .map(ToOwned::to_owned)
                .map(Value::String)
                .collect::<Vec<_>>();
            (!projected.is_empty()).then_some(Value::Array(projected))
        }
        // All other public checkout fields are scalar values in the adapter
        // contract. Do not retain an object/array supplied in a future
        // response under one of those names.
        _ if value.is_object() || value.is_array() => None,
        _ => Some(value.clone()),
    }
}

pub(crate) fn sanitize_wallet_gateway_response(
    value: Option<serde_json::Value>,
) -> serde_json::Value {
    let Some(value) = value else {
        return json!({});
    };
    let Some(object) = value.as_object() else {
        return json!({});
    };
    let mut sanitized = serde_json::Map::new();
    for key in WALLET_SAFE_GATEWAY_RESPONSE_KEYS {
        if let Some(item) = object.get(*key) {
            if let Some(item) = sanitize_wallet_gateway_value(key, item) {
                sanitized.insert((*key).to_string(), item);
            }
        }
    }
    let mut sanitized = serde_json::Value::Object(sanitized);
    remove_stripe_client_secret_fields(&mut sanitized);
    sanitized
}

fn valid_stripe_client_secret(value: &str) -> Option<&str> {
    normalize_stripe_client_secret(value)
}

fn insert_stripe_client_secret(instructions: &mut Value, client_secret: &str) {
    if let Some(object) = instructions.as_object_mut() {
        object.insert(
            "client_secret".to_string(),
            Value::String(client_secret.to_string()),
        );
    }
}

pub(crate) fn prepare_wallet_gateway_response_for_storage(
    state: &AppState,
    payment_provider: &str,
    order_no: &str,
    user_id: &str,
    checkout: &Value,
) -> Result<Value, String> {
    let binding = payment_provider
        .trim()
        .eq_ignore_ascii_case("stripe")
        .then(|| {
            PaymentOrderStripeSecretBinding::new(
                order_no,
                Some(user_id),
                WALLET_RECHARGE_ORDER_KIND,
                payment_provider,
            )
        })
        .transpose()
        .map_err(|error| error.to_string())?;
    prepare_gateway_response_for_storage_with_encrypt(
        payment_provider,
        checkout,
        true,
        |client_secret| {
            binding.as_ref().and_then(|binding| {
                seal_payment_order_stripe_client_secret(state, binding, client_secret).ok()
            })
        },
    )
}

/// Prepare a provider response for a non-wallet payment order (for example a
/// plan purchase). Keep the same credential stripping/encryption guarantees as
/// wallet checkout storage, but do not stamp the response with the wallet
/// recharge discriminator.
pub(crate) fn prepare_billing_gateway_response_for_storage(
    state: &AppState,
    payment_provider: &str,
    order_no: &str,
    user_id: &str,
    checkout: &Value,
) -> Result<Value, String> {
    let binding = payment_provider
        .trim()
        .eq_ignore_ascii_case("stripe")
        .then(|| {
            PaymentOrderStripeSecretBinding::new(
                order_no,
                Some(user_id),
                "plan_purchase",
                payment_provider,
            )
        })
        .transpose()
        .map_err(|error| error.to_string())?;
    prepare_gateway_response_for_storage_with_encrypt(
        payment_provider,
        checkout,
        false,
        |client_secret| {
            binding.as_ref().and_then(|binding| {
                seal_payment_order_stripe_client_secret(state, binding, client_secret).ok()
            })
        },
    )
}

fn prepare_wallet_gateway_response_for_storage_with_encrypt(
    payment_provider: &str,
    checkout: &Value,
    encrypt_secret: impl FnOnce(&str) -> Option<String>,
) -> Result<Value, String> {
    prepare_gateway_response_for_storage_with_encrypt(
        payment_provider,
        checkout,
        true,
        encrypt_secret,
    )
}

fn prepare_gateway_response_for_storage_with_encrypt(
    payment_provider: &str,
    checkout: &Value,
    include_wallet_order_kind: bool,
    encrypt_secret: impl FnOnce(&str) -> Option<String>,
) -> Result<Value, String> {
    if !checkout.is_object() {
        return Err("支付网关响应格式无效".to_string());
    }
    let client_secret = checkout
        .get("client_secret")
        .and_then(Value::as_str)
        .and_then(valid_stripe_client_secret)
        .map(ToOwned::to_owned);
    // Persist only the same allow-listed checkout fields exposed by the
    // public wallet API.  Gateway responses are an adapter boundary, so a
    // future provider integration must not be able to write arbitrary
    // response fields (or credentials) into payment_orders.
    let mut stored = sanitize_wallet_gateway_response(Some(checkout.clone()));
    remove_stripe_client_secret_fields(&mut stored);

    if include_wallet_order_kind {
        if let Some(object) = stored.as_object_mut() {
            object.insert(
                "order_kind".to_string(),
                Value::String(WALLET_RECHARGE_ORDER_KIND.to_string()),
            );
        }
    }

    // Claim metadata is generated by this handler and is needed by the
    // repository to reject stale updates. Keep it in the persisted projection,
    // while leaving it out of the public response projection above.
    if let Some(token) = checkout
        .get("checkout_claim_token")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= 128)
    {
        if let Some(object) = stored.as_object_mut() {
            object.insert(
                "checkout_claim_token".to_string(),
                Value::String(token.to_string()),
            );
            if let Some(claimed_at) = checkout
                .get("checkout_claimed_at_unix_secs")
                .and_then(Value::as_u64)
            {
                object.insert(
                    "checkout_claimed_at_unix_secs".to_string(),
                    Value::Number(serde_json::Number::from(claimed_at)),
                );
            }
        }
    }

    if !payment_provider.trim().eq_ignore_ascii_case("stripe") {
        return Ok(stored);
    }
    let client_secret = client_secret.ok_or_else(|| "Stripe client_secret 无效".to_string())?;
    let encrypted = encrypt_secret(&client_secret)
        .ok_or_else(|| "Stripe client_secret 加密失败".to_string())?;
    let Some(object) = stored.as_object_mut() else {
        return Err("支付网关响应格式无效".to_string());
    };
    object.insert(
        STRIPE_CLIENT_SECRET_ENCRYPTED_KEY.to_string(),
        Value::String(encrypted),
    );
    Ok(stored)
}

pub(crate) fn wallet_payment_instructions_from_checkout(
    payment_provider: &str,
    checkout: &Value,
) -> Value {
    let mut instructions = sanitize_wallet_gateway_response(Some(checkout.clone()));
    if payment_provider == "stripe" {
        if let Some(client_secret) = checkout
            .get("client_secret")
            .and_then(Value::as_str)
            .and_then(valid_stripe_client_secret)
        {
            insert_stripe_client_secret(&mut instructions, client_secret);
        }
    }
    instructions
}

fn payment_order_stripe_secret_identity_matches(
    expected: &aether_data::repository::wallet::StoredAdminPaymentOrder,
    actual: &aether_data::repository::wallet::StoredAdminPaymentOrder,
) -> bool {
    expected.id == actual.id
        && expected.order_no == actual.order_no
        && expected.wallet_id == actual.wallet_id
        && expected.user_id == actual.user_id
        && expected.payment_method == actual.payment_method
        && expected.payment_provider == actual.payment_provider
        && expected.order_kind == actual.order_kind
        && expected.gateway_order_id == actual.gateway_order_id
}

fn payment_order_is_live_pending(
    order: &aether_data::repository::wallet::StoredAdminPaymentOrder,
) -> bool {
    let now_unix_secs = Utc::now().timestamp().max(0) as u64;
    order.status.trim().eq_ignore_ascii_case("pending")
        && order
            .expires_at_unix_secs
            .is_some_and(|expires_at| expires_at > now_unix_secs)
}

async fn decrypt_or_migrate_payment_order_stripe_client_secret(
    state: &AppState,
    initial: &aether_data::repository::wallet::StoredAdminPaymentOrder,
) -> Result<String, String> {
    let identity = initial.clone();
    let mut current = initial.clone();

    for _ in 0..PAYMENT_ORDER_STRIPE_SECRET_MIGRATION_RETRIES {
        if !payment_order_stripe_secret_identity_matches(&identity, &current) {
            return Err(
                "payment order Stripe secret identity changed during migration".to_string(),
            );
        }
        if !payment_order_is_live_pending(&current) {
            return Err("payment order is no longer live and pending".to_string());
        }
        let gateway_response = current
            .gateway_response
            .clone()
            .ok_or_else(|| "payment order gateway response is unavailable".to_string())?;
        let observed = gateway_response
            .as_object()
            .and_then(|object| object.get(STRIPE_CLIENT_SECRET_ENCRYPTED_KEY))
            .and_then(Value::as_str)
            .ok_or_else(|| "payment order Stripe secret ciphertext is unavailable".to_string())?
            .to_string();
        let binding = PaymentOrderStripeSecretBinding::from_order(&current)
            .map_err(|error| error.to_string())?;
        let projection = open_payment_order_stripe_client_secret(state, &binding, &observed)
            .map_err(|error| error.to_string())?;
        if !projection.migration_required {
            return Ok(projection.plaintext);
        }

        let mutation =
            aether_data::repository::wallet::CompareAndSwapPaymentOrderStripeClientSecretInput {
                order_id: current.id.clone(),
                order_no: current.order_no.clone(),
                wallet_id: current.wallet_id.clone(),
                user_id: current.user_id.clone(),
                payment_method: current.payment_method.clone(),
                payment_provider: current.payment_provider.clone(),
                order_kind: current.order_kind.clone(),
                gateway_order_id: current.gateway_order_id.clone(),
                expected_status: current.status.clone(),
                expected_expires_at_unix_secs: current.expires_at_unix_secs,
                expected_gateway_response: gateway_response,
                expected_client_secret_encrypted: observed,
                replacement_client_secret_encrypted: projection.protected,
            };
        match state
            .compare_and_swap_payment_order_stripe_client_secret(mutation)
            .await
            .map_err(|error| format!("payment order Stripe secret migration failed: {error:?}"))?
        {
            Some(true) => return Ok(projection.plaintext),
            Some(false) => {}
            None => {
                return Err(
                    "payment order Stripe secret migration storage is unavailable".to_string(),
                )
            }
        }

        current = state
            .find_payment_order_by_id(&identity.id)
            .await
            .map_err(|error| format!("payment order Stripe secret reread failed: {error:?}"))?
            .ok_or_else(|| {
                "payment order disappeared during Stripe secret migration".to_string()
            })?;
    }

    Err("payment order Stripe secret migration did not stabilize".to_string())
}

pub(crate) async fn wallet_payment_instructions_from_stored(
    state: &AppState,
    order: &aether_data::repository::wallet::StoredAdminPaymentOrder,
) -> Value {
    if !payment_order_is_live_pending(order) {
        return json!({});
    }

    let mut instructions = sanitize_wallet_gateway_response(order.gateway_response.clone());
    let payment_provider = order
        .payment_provider
        .as_deref()
        .unwrap_or(order.payment_method.as_str());
    if !payment_provider.trim().eq_ignore_ascii_case("stripe") {
        return instructions;
    }
    let client_secret =
        match decrypt_or_migrate_payment_order_stripe_client_secret(state, order).await {
            Ok(value) => value,
            Err(error) => {
                tracing::warn!(
                    order_id = %order.id,
                    order_no = %order.order_no,
                    error_category = "stripe_client_secret_open_failed",
                    reason = %error,
                    "payment order Stripe client secret was withheld"
                );
                return instructions;
            }
        };
    insert_stripe_client_secret(&mut instructions, &client_secret);
    instructions
}

fn build_wallet_payment_order_payload(
    id: String,
    order_no: String,
    wallet_id: String,
    user_id: Option<String>,
    amount_usd: f64,
    pay_amount: Option<f64>,
    pay_currency: Option<String>,
    exchange_rate: Option<f64>,
    refunded_amount_usd: f64,
    refundable_amount_usd: f64,
    payment_method: String,
    gateway_order_id: Option<String>,
    gateway_response: Option<serde_json::Value>,
    status: String,
    created_at: Option<String>,
    paid_at: Option<String>,
    credited_at: Option<String>,
    expires_at: Option<String>,
) -> serde_json::Value {
    json!({
        "id": id,
        "order_no": order_no,
        "wallet_id": wallet_id,
        "user_id": user_id,
        "amount_usd": amount_usd,
        "pay_amount": pay_amount,
        "pay_currency": pay_currency,
        "exchange_rate": exchange_rate,
        "refunded_amount_usd": refunded_amount_usd,
        "refundable_amount_usd": refundable_amount_usd,
        "payment_method": payment_method,
        "gateway_order_id": gateway_order_id,
        "gateway_response": sanitize_wallet_gateway_response(gateway_response),
        "status": status,
        "created_at": created_at,
        "paid_at": paid_at,
        "credited_at": credited_at,
        "expires_at": expires_at,
    })
}

fn wallet_payment_order_payload_from_record(
    record: &aether_data::repository::wallet::StoredAdminPaymentOrder,
) -> serde_json::Value {
    // The order history is durable, but checkout capabilities are not. An
    // expired, paid, or credited order must not keep advertising a URL/form
    // that the callback path will no longer accept.
    let gateway_response = wallet_recharge_order_is_pending_and_live(record)
        .then(|| record.gateway_response.clone())
        .flatten();
    build_wallet_payment_order_payload(
        record.id.clone(),
        record.order_no.clone(),
        record.wallet_id.clone(),
        record.user_id.clone(),
        record.amount_usd,
        record.pay_amount,
        record.pay_currency.clone(),
        record.exchange_rate,
        record.refunded_amount_usd,
        record.refundable_amount_usd,
        record.payment_method.clone(),
        record.gateway_order_id.clone(),
        gateway_response,
        record.status.clone(),
        unix_secs_to_rfc3339(wallet_recharge_order_created_at_unix_secs(record)),
        record.paid_at_unix_secs.and_then(unix_secs_to_rfc3339),
        record.credited_at_unix_secs.and_then(unix_secs_to_rfc3339),
        record.expires_at_unix_secs.and_then(unix_secs_to_rfc3339),
    )
}

fn wallet_recharge_order_has_checkout(
    order: &aether_data::repository::wallet::StoredAdminPaymentOrder,
) -> bool {
    order
        .gateway_response
        .as_ref()
        .and_then(Value::as_object)
        .is_some_and(|object| {
            object.keys().any(|key| {
                matches!(
                    key.as_str(),
                    "payment_url"
                        | "payment_params"
                        | "qr_code"
                        | "code_url"
                        | "h5_url"
                        | "jsapi"
                        | "client_secret"
                        | STRIPE_CLIENT_SECRET_ENCRYPTED_KEY
                        | "intent_id"
                )
            })
        })
}

fn wallet_recharge_order_is_pending_and_live(
    order: &aether_data::repository::wallet::StoredAdminPaymentOrder,
) -> bool {
    order.status.eq_ignore_ascii_case("pending")
        && order
            .expires_at_unix_secs
            .is_some_and(|expires_at| expires_at > Utc::now().timestamp().max(0) as u64)
}

fn wallet_recharge_exchange_rates_match(
    pay_currency: Option<&str>,
    stored_rate: Option<f64>,
    requested_rate: f64,
) -> bool {
    let Some(pay_currency) = pay_currency else {
        return false;
    };
    let Some(stored_rate) = stored_rate else {
        return false;
    };
    let Ok(stored_rate) =
        crate::handlers::shared::effective_payment_exchange_rate(pay_currency, stored_rate)
    else {
        return false;
    };
    let Ok(requested_rate) =
        crate::handlers::shared::effective_payment_exchange_rate(pay_currency, requested_rate)
    else {
        return false;
    };
    (stored_rate - requested_rate).abs() <= PAYMENT_AMOUNT_EPSILON
}

fn wallet_recharge_order_matches_request(
    order: &aether_data::repository::wallet::StoredAdminPaymentOrder,
    payload: &NormalizedWalletCreateRechargeRequest,
) -> bool {
    let amount_matches = order.amount_usd.is_finite()
        && (order.amount_usd - payload.amount_usd).abs() <= PAYMENT_AMOUNT_EPSILON;
    let requested_provider = payload
        .payment_provider
        .as_deref()
        .unwrap_or(payload.payment_method.as_str());
    let stored_provider = order
        .gateway_response
        .as_ref()
        .and_then(Value::as_object)
        .and_then(|object| object.get("gateway"))
        .and_then(Value::as_str);
    // EPay exposes multiple channels behind one provider.  When callers use
    // the legacy shorthand (`payment_method: alipay|wxpay`) without an
    // explicit payment_channel, that method is still part of the idempotent
    // request identity and must not replay another channel's order.
    let requested_channel = payload.payment_channel.as_deref().or_else(|| {
        (requested_provider.eq_ignore_ascii_case("epay")
            && !payload.payment_method.eq_ignore_ascii_case("epay"))
        .then_some(payload.payment_method.as_str())
    });
    let metadata = order.gateway_response.as_ref().and_then(Value::as_object);
    let stored_channel = metadata
        .and_then(|object| object.get("payment_channel"))
        .and_then(Value::as_str);
    let payment_identity_matches = wallet_recharge_payment_identity_matches(
        &order.payment_method,
        stored_provider,
        stored_channel,
        requested_provider,
        requested_channel,
    );
    let pay_amount_matches = payload.pay_amount.is_none_or(|value| {
        order.pay_amount.is_some_and(|stored| {
            stored.is_finite() && (stored - value).abs() <= PAYMENT_AMOUNT_EPSILON
        })
    });
    let currency_matches = payload.pay_currency.as_deref().is_none_or(|currency| {
        order
            .pay_currency
            .as_deref()
            .is_some_and(|stored| stored.eq_ignore_ascii_case(currency))
    });
    let exchange_rate_matches = payload.exchange_rate.is_none_or(|value| {
        wallet_recharge_exchange_rates_match(
            order.pay_currency.as_deref(),
            order.exchange_rate,
            value,
        )
    });
    amount_matches
        && payment_identity_matches
        && pay_amount_matches
        && currency_matches
        && exchange_rate_matches
}

fn wallet_recharge_payment_identity_matches(
    stored_method: &str,
    stored_provider: Option<&str>,
    stored_channel: Option<&str>,
    requested_provider: &str,
    requested_channel: Option<&str>,
) -> bool {
    let legacy_epay_method = stored_provider
        .is_some_and(|provider| provider.eq_ignore_ascii_case("epay"))
        && ["alipay", "wxpay"]
            .iter()
            .any(|method| stored_method.eq_ignore_ascii_case(method));
    let method_matches = stored_method.eq_ignore_ascii_case(requested_provider)
        || (requested_provider.eq_ignore_ascii_case("epay") && legacy_epay_method);
    let provider_matches = stored_provider.map_or_else(
        || stored_method.eq_ignore_ascii_case(requested_provider),
        |provider| provider.eq_ignore_ascii_case(requested_provider),
    );
    let effective_stored_channel =
        stored_channel.or_else(|| legacy_epay_method.then_some(stored_method));
    let channel_matches = requested_channel.is_none_or(|channel| {
        effective_stored_channel.is_some_and(|stored| stored.eq_ignore_ascii_case(channel))
    });
    method_matches && provider_matches && channel_matches
}

fn wallet_recharge_order_matches_effective_request(
    order: &aether_data::repository::wallet::StoredAdminPaymentOrder,
    provider: &str,
    channel: &str,
    amount_usd: f64,
    pay_amount: f64,
    pay_currency: &str,
    exchange_rate: f64,
) -> bool {
    if !order.amount_usd.is_finite()
        || !amount_usd.is_finite()
        || (order.amount_usd - amount_usd).abs() > PAYMENT_AMOUNT_EPSILON
        || !order.pay_amount.is_some_and(|value| {
            value.is_finite() && (value - pay_amount).abs() <= PAYMENT_AMOUNT_EPSILON
        })
        || !order
            .pay_currency
            .as_deref()
            .is_some_and(|value| value.eq_ignore_ascii_case(pay_currency))
        || !wallet_recharge_exchange_rates_match(
            order.pay_currency.as_deref(),
            order.exchange_rate,
            exchange_rate,
        )
    {
        return false;
    }
    let Some(metadata) = order.gateway_response.as_ref().and_then(Value::as_object) else {
        return false;
    };
    let stored_provider = metadata.get("gateway").and_then(Value::as_str);
    let stored_channel = metadata.get("payment_channel").and_then(Value::as_str);
    wallet_recharge_payment_identity_matches(
        &order.payment_method,
        stored_provider,
        stored_channel,
        provider,
        Some(channel),
    )
}

fn wallet_test_recharge_payload_matches_request(
    order: &Value,
    payload: &NormalizedWalletCreateRechargeRequest,
) -> bool {
    let amount_matches = order
        .get("amount_usd")
        .and_then(Value::as_f64)
        .is_some_and(|value| {
            value.is_finite() && (value - payload.amount_usd).abs() <= PAYMENT_AMOUNT_EPSILON
        });
    let stored_method = order.get("payment_method").and_then(Value::as_str);
    let stored_provider = order
        .get("payment_provider")
        .and_then(Value::as_str)
        .or_else(|| {
            order
                .get("gateway_response")
                .and_then(Value::as_object)
                .and_then(|object| object.get("gateway"))
                .and_then(Value::as_str)
        });
    let requested_provider = payload
        .payment_provider
        .as_deref()
        .unwrap_or(payload.payment_method.as_str());
    let requested_channel = payload.payment_channel.as_deref().or_else(|| {
        (requested_provider.eq_ignore_ascii_case("epay")
            && !payload.payment_method.eq_ignore_ascii_case("epay"))
        .then_some(payload.payment_method.as_str())
    });
    let metadata = order.get("gateway_response").and_then(Value::as_object);
    let stored_channel = order
        .get("payment_channel")
        .and_then(Value::as_str)
        .or_else(|| {
            metadata
                .and_then(|object| object.get("payment_channel"))
                .and_then(Value::as_str)
        });
    let payment_identity_matches = stored_method.is_some_and(|stored_method| {
        wallet_recharge_payment_identity_matches(
            stored_method,
            stored_provider,
            stored_channel,
            requested_provider,
            requested_channel,
        )
    });
    let pay_amount_matches = payload.pay_amount.is_none_or(|value| {
        order
            .get("pay_amount")
            .and_then(Value::as_f64)
            .is_some_and(|stored| {
                stored.is_finite() && (stored - value).abs() <= PAYMENT_AMOUNT_EPSILON
            })
    });
    let currency_matches = payload.pay_currency.as_deref().is_none_or(|currency| {
        order
            .get("pay_currency")
            .and_then(Value::as_str)
            .is_some_and(|stored| stored.eq_ignore_ascii_case(currency))
    });
    let exchange_rate_matches = payload.exchange_rate.is_none_or(|value| {
        wallet_recharge_exchange_rates_match(
            order.get("pay_currency").and_then(Value::as_str),
            order.get("exchange_rate").and_then(Value::as_f64),
            value,
        )
    });
    amount_matches
        && payment_identity_matches
        && pay_amount_matches
        && currency_matches
        && exchange_rate_matches
}

fn wallet_test_recharge_replay_payment_instructions(order: &Value) -> Value {
    let is_pending_and_live = order
        .get("status")
        .and_then(Value::as_str)
        .is_some_and(|status| status.eq_ignore_ascii_case("pending"))
        && order
            .get("expires_at")
            .and_then(Value::as_str)
            .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
            .is_some_and(|expires_at| expires_at.timestamp() > Utc::now().timestamp());
    if !is_pending_and_live {
        return json!({});
    }
    sanitize_wallet_gateway_response(order.get("gateway_response").cloned())
}

#[cfg(test)]
fn wallet_test_recharge_public_payload(mut order: Value) -> Value {
    let is_pending_and_live = order
        .get("status")
        .and_then(Value::as_str)
        .is_some_and(|status| status.eq_ignore_ascii_case("pending"))
        && order
            .get("expires_at")
            .and_then(Value::as_str)
            .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
            .is_some_and(|expires_at| expires_at.timestamp() > Utc::now().timestamp());
    if !is_pending_and_live {
        if let Some(object) = order.as_object_mut() {
            object.insert("gateway_response".to_string(), json!({}));
        }
    } else if let Some(gateway_response) = order.get("gateway_response").cloned() {
        order["gateway_response"] = sanitize_wallet_gateway_response(Some(gateway_response));
    }
    order
}

async fn wallet_recharge_replay_payment_instructions(
    state: &AppState,
    order: &aether_data::repository::wallet::StoredAdminPaymentOrder,
) -> Value {
    // A replayed idempotency response must not hand out a stale payment URL.
    // Providers can reject an expired order, but returning the URL still
    // invites a user to submit a payment that cannot be credited safely.
    if !wallet_recharge_order_is_pending_and_live(order) {
        return json!({});
    }
    wallet_payment_instructions_from_stored(state, order).await
}

async fn wallet_recharge_replay_response(
    state: &AppState,
    order: &aether_data::repository::wallet::StoredAdminPaymentOrder,
) -> Response<Body> {
    let payment_instructions = wallet_recharge_replay_payment_instructions(state, order).await;
    mark_sensitive_response_no_store(build_auth_json_response(
        http::StatusCode::OK,
        json!({
            "order": wallet_payment_order_payload_from_record(order),
            "payment_instructions": payment_instructions,
            "reused_idempotent_order": true,
        }),
        None,
    ))
}

/// A provider checkout can finish after its callback has already credited the
/// order.  In that case the conditional checkout update reports a conflict,
/// but the client must observe the durable settled order instead of receiving a
/// misleading checkout error (or a second payment capability).
async fn settled_wallet_recharge_after_checkout_conflict(
    state: &AppState,
    user_id: &str,
    order_id: &str,
) -> Option<aether_data::repository::wallet::StoredAdminPaymentOrder> {
    match state
        .find_wallet_payment_order_by_user_id(user_id, order_id)
        .await
    {
        Ok(Some(order)) if wallet_recharge_order_is_settled(&order) => Some(order),
        _ => None,
    }
}

fn wallet_recharge_order_is_settled(
    order: &aether_data::repository::wallet::StoredAdminPaymentOrder,
) -> bool {
    matches!(order.status.as_str(), "paid" | "credited")
}

fn wallet_recharge_claim_token() -> String {
    format!("wrc_{}", Uuid::new_v4().simple())
}

fn wallet_recharge_claimed_placeholder(
    value: &Value,
    claim_token: &str,
    claimed_at_unix_secs: u64,
) -> Result<Value, String> {
    wallet_recharge_checkout_claim_response(value, claim_token, claimed_at_unix_secs)
}

async fn best_effort_fail_wallet_recharge_checkout(
    state: &AppState,
    order: &aether_data::repository::wallet::StoredAdminPaymentOrder,
    claim_token: &str,
    reason: &str,
) {
    let _ = state
        .fail_wallet_recharge_checkout(FailWalletRechargeCheckoutInput {
            order_id: order.id.clone(),
            claim_token: claim_token.to_string(),
            reason: reason.to_string(),
            provider_request_may_have_succeeded: false,
        })
        .await;
}

async fn best_effort_mark_wallet_recharge_checkout_uncertain(
    state: &AppState,
    order: &aether_data::repository::wallet::StoredAdminPaymentOrder,
    claim_token: &str,
    reason: &str,
) {
    let _ = state
        .fail_wallet_recharge_checkout(FailWalletRechargeCheckoutInput {
            order_id: order.id.clone(),
            claim_token: claim_token.to_string(),
            reason: reason.to_string(),
            provider_request_may_have_succeeded: true,
        })
        .await;
}

async fn reclaim_wallet_recharge_checkout(
    state: &AppState,
    order: &aether_data::repository::wallet::StoredAdminPaymentOrder,
    placeholder: Value,
    claim_token: &str,
    expires_at_unix_secs: u64,
) -> Result<aether_data::repository::wallet::StoredAdminPaymentOrder, String> {
    match state
        .reclaim_wallet_recharge_checkout(ReclaimWalletRechargeCheckoutInput {
            order_id: order.id.clone(),
            claim_token: claim_token.to_string(),
            gateway_response: placeholder,
            expires_at_unix_secs,
        })
        .await
    {
        Ok(Some(aether_data::repository::wallet::WalletMutationOutcome::Applied(order))) => {
            Ok(order)
        }
        Ok(Some(aether_data::repository::wallet::WalletMutationOutcome::NotFound)) => {
            Err("充值订单已不存在".to_string())
        }
        Ok(Some(aether_data::repository::wallet::WalletMutationOutcome::Invalid(detail))) => {
            Err(detail)
        }
        Ok(None) => Err("钱包充值后端暂不可用".to_string()),
        Err(err) => Err(format!("wallet recharge checkout reclaim failed: {err:?}")),
    }
}

fn attach_wallet_recharge_claim_token(mut checkout: Value, claim_token: &str) -> Value {
    if let Some(object) = checkout.as_object_mut() {
        object.insert(
            "checkout_claim_token".to_string(),
            Value::String(claim_token.to_string()),
        );
    }
    checkout
}

#[derive(Debug, Clone)]
pub(crate) struct DirectGatewayChannelConfig {
    pub(crate) channel: String,
    pub(crate) display_name: String,
    pub(crate) fee_rate: f64,
}

fn configured_channel_fee_rate(value: Option<&Value>) -> f64 {
    let fee_rate = match value {
        Some(Value::Number(number)) => number.as_f64().unwrap_or(0.0),
        Some(Value::String(value)) => value.trim().parse::<f64>().unwrap_or(0.0),
        _ => 0.0,
    };
    if fee_rate.is_finite() && fee_rate >= 0.0 {
        fee_rate
    } else {
        0.0
    }
}

fn round_payment_amount(value: f64) -> Option<f64> {
    if !value.is_finite() {
        return None;
    }
    let rounded = (value * 100.0).round() / 100.0;
    rounded.is_finite().then_some(rounded)
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct WalletRechargePaymentBreakdown {
    base_pay_amount: f64,
    fee_amount: f64,
    pay_amount: f64,
    exchange_rate: f64,
}

fn wallet_recharge_payment_breakdown(
    amount_usd: f64,
    pay_currency: &str,
    usd_exchange_rate: f64,
    fee_rate: f64,
) -> Result<WalletRechargePaymentBreakdown, &'static str> {
    if !amount_usd.is_finite() || amount_usd <= 0.0 || !fee_rate.is_finite() || fee_rate < 0.0 {
        return Err("充值金额配置无效");
    }
    let exchange_rate =
        crate::handlers::shared::effective_payment_exchange_rate(pay_currency, usd_exchange_rate)
            .map_err(|_| "充值金额配置无效")?;
    let base_pay_amount = round_payment_amount(amount_usd * exchange_rate)
        .filter(|value| *value > 0.0)
        .ok_or("充值金额配置无效")?;
    let fee_amount = round_payment_amount(base_pay_amount * fee_rate / 100.0)
        .filter(|value| *value >= 0.0)
        .ok_or("充值金额配置无效")?;
    let pay_amount = round_payment_amount(base_pay_amount + fee_amount)
        .filter(|value| *value > 0.0)
        .ok_or("充值金额配置无效")?;
    Ok(WalletRechargePaymentBreakdown {
        base_pay_amount,
        fee_amount,
        pay_amount,
        exchange_rate,
    })
}

fn add_wallet_recharge_fee_metadata(
    mut checkout: Value,
    base_pay_amount: f64,
    fee_rate: f64,
    fee_amount: f64,
) -> Value {
    if let Some(object) = checkout.as_object_mut() {
        object.insert("base_pay_amount".to_string(), json!(base_pay_amount));
        object.insert("fee_rate".to_string(), json!(fee_rate));
        object.insert("fee_amount".to_string(), json!(fee_amount));
    }
    checkout
}

pub(crate) fn direct_gateway_channels(
    provider: &str,
    record: &aether_data_contracts::repository::billing::PaymentGatewayConfigRecord,
) -> Vec<DirectGatewayChannelConfig> {
    let channels_value =
        crate::handlers::shared::payment_gateway_channels_json(&record.channels_json);
    let channels = channels_value.as_array().into_iter().flatten();
    channels
        .filter_map(|channel| {
            let channel_id = channel
                .get("channel")
                .or_else(|| channel.get("type"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())?;
            let display_name = channel
                .get("display_name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or(channel_id);
            Some(DirectGatewayChannelConfig {
                channel: channel_id.to_ascii_lowercase(),
                display_name: display_name.to_string(),
                fee_rate: configured_channel_fee_rate(channel.get("fee_rate")),
            })
        })
        .filter(
            |channel| match provider.trim().to_ascii_lowercase().as_str() {
                "alipay" => channel.channel == "alipay",
                "wxpay" => matches!(channel.channel.as_str(), "native" | "h5" | "jsapi"),
                "stripe" => matches!(
                    channel.channel.as_str(),
                    "card" | "alipay" | "wechat_pay" | "link"
                ),
                _ => false,
            },
        )
        .collect()
}

pub(crate) fn resolve_direct_gateway_channel(
    provider: &str,
    record: &aether_data_contracts::repository::billing::PaymentGatewayConfigRecord,
    requested: Option<&str>,
) -> Result<DirectGatewayChannelConfig, String> {
    let channels = direct_gateway_channels(provider, record);
    if channels.is_empty() {
        return Err("支付网关没有可用通道".to_string());
    }
    let Some(requested) = requested.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(channels[0].clone());
    };
    channels
        .into_iter()
        .find(|channel| channel.channel.eq_ignore_ascii_case(requested))
        .ok_or_else(|| "支付通道不可用".to_string())
}

fn direct_gateway_public_config_string(
    record: &aether_data_contracts::repository::billing::PaymentGatewayConfigRecord,
    key: &str,
) -> Option<String> {
    crate::handlers::shared::payment_gateway_config_json(&record.channels_json)
        .as_object()?
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn decrypt_direct_gateway_secrets(
    state: &AppState,
    record: &aether_data_contracts::repository::billing::PaymentGatewayConfigRecord,
) -> Result<serde_json::Map<String, Value>, String> {
    let Some(encrypted) = record.merchant_key_encrypted.as_deref() else {
        return Err("支付网关密钥未配置".to_string());
    };
    let binding = crate::handlers::shared::PaymentGatewaySecretBinding::from_record(record)
        .map_err(|_| "支付网关密钥绑定无效".to_string())?;
    let plaintext =
        crate::handlers::shared::open_payment_gateway_secret(state, &binding, encrypted)
            .map_err(|_| "支付网关密钥解密失败".to_string())?
            .plaintext;
    serde_json::from_str::<Value>(&plaintext)
        .ok()
        .and_then(|value| value.as_object().cloned())
        .ok_or_else(|| "支付网关密钥格式无效".to_string())
}

fn direct_gateway_secret_string(
    secrets: &serde_json::Map<String, Value>,
    key: &str,
) -> Option<String> {
    secrets
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

async fn create_stripe_wallet_recharge_checkout(
    state: &AppState,
    record: &aether_data_contracts::repository::billing::PaymentGatewayConfigRecord,
    payment_channel: &str,
    display_name: &str,
    order_no: &str,
    pay_amount: f64,
    expires_at: chrono::DateTime<chrono::Utc>,
    idempotency_key: &str,
) -> Result<Value, StripeWalletCheckoutError> {
    let secrets = decrypt_direct_gateway_secrets(state, record)?;
    let Some(secret_key) = direct_gateway_secret_string(&secrets, "secret_key") else {
        return Err("Stripe secret_key 未配置".to_string().into());
    };
    let Some(publishable_key) = direct_gateway_public_config_string(record, "publishable_key")
    else {
        return Err("Stripe publishable_key 未配置".to_string().into());
    };
    let amount = crate::handlers::shared::stripe_amount_to_minor(pay_amount, &record.pay_currency)
        .ok_or_else(|| "Stripe 支付金额无效".to_string())?;
    let currency = record.pay_currency.trim().to_ascii_lowercase();
    let mut form = vec![
        ("amount".to_string(), amount.to_string()),
        ("currency".to_string(), currency.clone()),
        ("description".to_string(), "钱包充值".to_string()),
        ("metadata[order_no]".to_string(), order_no.to_string()),
        (
            "metadata[payment_provider]".to_string(),
            "stripe".to_string(),
        ),
        (
            "metadata[payment_channel]".to_string(),
            payment_channel.to_string(),
        ),
        (
            "payment_method_types[]".to_string(),
            payment_channel.to_string(),
        ),
    ];
    if payment_channel == "wechat_pay" {
        form.push((
            "payment_method_options[wechat_pay][client]".to_string(),
            "web".to_string(),
        ));
    }
    let stripe_endpoint =
        url::Url::parse("https://api.stripe.com/v1/payment_intents").map_err(|_| {
            StripeWalletCheckoutError::Uncertain(STRIPE_CHECKOUT_UNCERTAIN_DETAIL.to_string())
        })?;
    let stripe_client = public_payment_http_client(&stripe_endpoint)
        .await
        .map_err(|_| {
            StripeWalletCheckoutError::Uncertain(STRIPE_CHECKOUT_UNCERTAIN_DETAIL.to_string())
        })?;
    let response = match stripe_client
        .post(stripe_endpoint)
        .header("Idempotency-Key", idempotency_key)
        .basic_auth(secret_key, Some(""))
        .form(&form)
        .send()
        .await
    {
        Ok(response) => response,
        Err(_) => {
            tracing::warn!(
                event_name = "stripe_wallet_payment_intent_request_failed",
                "Stripe wallet PaymentIntent request failed"
            );
            return Err(StripeWalletCheckoutError::Uncertain(
                STRIPE_CHECKOUT_UNCERTAIN_DETAIL.to_string(),
            ));
        }
    };
    let status = response.status();
    let body =
        aether_http::read_response_bytes_with_limit(response, MAX_PAYMENT_GATEWAY_RESPONSE_BYTES)
            .await
            .map_err(|_| {
                tracing::warn!(
                    event_name = "stripe_wallet_payment_intent_response_read_failed",
                    upstream_status = %status,
                    "Stripe wallet PaymentIntent response could not be read"
                );
                if status.is_server_error() || status == http::StatusCode::TOO_MANY_REQUESTS {
                    StripeWalletCheckoutError::Uncertain(
                        STRIPE_CHECKOUT_UNCERTAIN_DETAIL.to_string(),
                    )
                } else {
                    StripeWalletCheckoutError::Failed(STRIPE_CHECKOUT_FAILED_DETAIL.to_string())
                }
            })?;
    if !status.is_success() {
        tracing::warn!(
            event_name = "stripe_wallet_payment_intent_upstream_rejected",
            upstream_status = %status,
            "Stripe wallet PaymentIntent request was rejected"
        );
        if status.is_server_error() || status == http::StatusCode::TOO_MANY_REQUESTS {
            return Err(StripeWalletCheckoutError::Uncertain(
                STRIPE_CHECKOUT_UNCERTAIN_DETAIL.to_string(),
            ));
        }
        return Err(StripeWalletCheckoutError::Failed(
            STRIPE_CHECKOUT_FAILED_DETAIL.to_string(),
        ));
    }
    let value = serde_json::from_slice::<Value>(&body)
        .map_err(|_| StripeWalletCheckoutError::Uncertain("Stripe 响应格式无效".to_string()))?;
    if stripe_wallet_checkout_response_is_canceled(&value) {
        return Err(StripeWalletCheckoutError::Canceled);
    }
    let Some(intent_id) = value.get("id").and_then(Value::as_str) else {
        return Err(StripeWalletCheckoutError::Uncertain(
            "Stripe 响应缺少 PaymentIntent ID".to_string(),
        ));
    };
    let Some(client_secret) = value.get("client_secret").and_then(Value::as_str) else {
        return Err(StripeWalletCheckoutError::Uncertain(
            "Stripe 响应缺少 client_secret".to_string(),
        ));
    };
    Ok(json!({
        "gateway": "stripe",
        "display_name": display_name,
        "gateway_order_id": intent_id,
        "intent_id": intent_id,
        "client_secret": client_secret,
        "publishable_key": publishable_key,
        "expires_at": expires_at.to_rfc3339(),
        "pay_amount": pay_amount,
        "pay_currency": record.pay_currency,
        "payment_channel": payment_channel,
        "payment_method_types": [payment_channel],
        "submit_method": "stripe_payment_intent"
    }))
}

pub(super) async fn handle_wallet_create_recharge(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
    client_ip: std::net::IpAddr,
    request_body: Option<&axum::body::Bytes>,
) -> Response<Body> {
    let auth = match resolve_authenticated_local_user(state, request_context, headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Some(request_body) = request_body else {
        return build_auth_error_response(http::StatusCode::BAD_REQUEST, "缺少请求体", false);
    };
    let payload = match serde_json::from_slice::<WalletCreateRechargeRequest>(request_body) {
        Ok(value) => value,
        Err(_) => {
            return build_auth_error_response(http::StatusCode::BAD_REQUEST, "输入验证失败", false)
        }
    };
    let payload = match normalize_wallet_create_recharge_request(payload) {
        Ok(value) => value,
        Err(detail) => {
            return build_auth_error_response(http::StatusCode::BAD_REQUEST, detail, false)
        }
    };
    if payload.payment_method == "admin_manual" {
        return build_auth_error_response(
            http::StatusCode::BAD_REQUEST,
            "admin_manual is reserved for admin recharge",
            false,
        );
    }

    let wallet = match state
        .find_wallet(aether_data::repository::wallet::WalletLookupKey::UserId(
            &auth.user.id,
        ))
        .await
    {
        Ok(value) => value,
        Err(err) => {
            return build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("wallet lookup failed: {err:?}"),
                false,
            )
        }
    };

    let now = Utc::now();
    let now_unix_secs = now.timestamp().max(0) as u64;
    let order_no = wallet_recharge_order_no(&auth.user.id, payload.idempotency_key.as_deref(), now);
    if payload.idempotency_key.is_some() {
        match state
            .find_wallet_recharge_order_by_order_no(&auth.user.id, &order_no)
            .await
        {
            Ok(Some(order)) => {
                if !wallet_recharge_order_matches_request(&order, &payload) {
                    return build_auth_error_response(
                        http::StatusCode::CONFLICT,
                        "idempotency_key 已用于其他充值订单",
                        false,
                    );
                }
                let reclaimable =
                    wallet_recharge_order_is_reclaimable_placeholder(&order, now_unix_secs);
                if wallet_recharge_order_has_checkout(&order)
                    || (!reclaimable && !wallet_recharge_order_is_pending_and_live(&order))
                {
                    return wallet_recharge_replay_response(state, &order).await;
                }
                if !reclaimable {
                    // A pending order without checkout data is an in-flight claim held by
                    // another request. Never call an external gateway from a retry: doing so
                    // would create duplicate provider-side orders while the first request is
                    // still waiting for its response.
                    return build_auth_error_response(
                        http::StatusCode::CONFLICT,
                        "充值订单正在创建，请稍后重试",
                        false,
                    );
                }
                // The provider-specific branch below will atomically replace this
                // placeholder after resolving the effective channel and amount.
            }
            Ok(None) => {}
            Err(err) => {
                return build_auth_error_response(
                    http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("wallet recharge idempotency lookup failed: {err:?}"),
                    false,
                )
            }
        }
    }

    if !state.has_database_wallet_data_writer() {
        #[cfg(test)]
        {
            let Some(wallet) = wallet else {
                return build_auth_error_response(
                    http::StatusCode::BAD_REQUEST,
                    "wallet not available",
                    false,
                );
            };
            if wallet.status != "active" {
                return build_auth_error_response(
                    http::StatusCode::BAD_REQUEST,
                    "wallet is not active",
                    false,
                );
            }
            if let Some(idempotency_key) = payload.idempotency_key.as_deref() {
                if let Some(existing) =
                    wallet_test_recharge_order_by_order_no(&auth.user.id, &order_no)
                {
                    if !wallet_test_recharge_payload_matches_request(&existing, &payload) {
                        return build_auth_error_response(
                            http::StatusCode::CONFLICT,
                            "idempotency_key 已用于其他充值订单",
                            false,
                        );
                    }
                    let instructions = wallet_test_recharge_replay_payment_instructions(&existing);
                    return mark_sensitive_response_no_store(build_auth_json_response(
                        http::StatusCode::OK,
                        json!({
                            "order": existing,
                            "payment_instructions": instructions,
                            "reused_idempotent_order": true,
                        }),
                        None,
                    ));
                }
                let _ = idempotency_key;
            }
            let order_id = Uuid::new_v4().to_string();
            let expires_at = now + chrono::Duration::minutes(30);
            let Some(adapter) = PaymentGatewayRegistry::get(&payload.payment_method) else {
                return build_auth_error_response(
                    http::StatusCode::BAD_REQUEST,
                    format!("unsupported payment_method: {}", payload.payment_method),
                    false,
                );
            };
            let checkout = match adapter.create_checkout_session(&CreateCheckoutSessionInput {
                order_no: order_no.clone(),
                amount_usd: payload.amount_usd,
                expires_at,
            }) {
                Ok(value) => value,
                Err(detail) => {
                    return build_auth_error_response(http::StatusCode::BAD_REQUEST, detail, false);
                }
            };
            let mut order_payload = build_wallet_payment_order_payload(
                order_id,
                order_no.clone(),
                wallet.id.clone(),
                Some(auth.user.id.clone()),
                payload.amount_usd,
                payload.pay_amount,
                payload.pay_currency.clone(),
                payload.exchange_rate,
                0.0,
                0.0,
                payload.payment_method.clone(),
                Some(checkout.gateway_order_id.clone()),
                Some(checkout.gateway_response.clone()),
                "pending".to_string(),
                Some(now.to_rfc3339()),
                None,
                None,
                Some(expires_at.to_rfc3339()),
            );
            if let Some(object) = order_payload.as_object_mut() {
                object.insert(
                    "payment_provider".to_string(),
                    Value::String(payload.payment_method.clone()),
                );
                object.insert(
                    "payment_channel".to_string(),
                    payload
                        .payment_channel
                        .clone()
                        .map(Value::String)
                        .unwrap_or(Value::Null),
                );
                object.insert(
                    "order_kind".to_string(),
                    Value::String(WALLET_RECHARGE_ORDER_KIND.to_string()),
                );
            }
            record_wallet_test_recharge(auth.user.id, order_no.clone(), order_payload.clone());
            return mark_sensitive_response_no_store(build_auth_json_response(
                http::StatusCode::OK,
                json!({
                    "order": order_payload,
                    "payment_instructions": sanitize_wallet_gateway_response(Some(checkout.gateway_response)),
                }),
                None,
            ));
        }
        #[cfg(not(test))]
        return build_wallet_recharge_storage_unavailable_response();
    }

    let expires_at = now + chrono::Duration::minutes(30);
    let uses_epay =
        payload.payment_provider.as_deref() == Some("epay") || payload.payment_method == "epay";
    if uses_epay {
        let config = match load_epay_config(state).await {
            Ok(value) => value,
            Err(detail) => {
                return build_auth_error_response(http::StatusCode::BAD_REQUEST, detail, false);
            }
        };
        if payload.amount_usd < config.min_recharge_usd {
            return build_auth_error_response(
                http::StatusCode::BAD_REQUEST,
                "充值金额低于支付网关最小金额",
                false,
            );
        }
        let requested_channel = payload.payment_channel.as_deref().or_else(|| {
            (payload.payment_method != "epay").then_some(payload.payment_method.as_str())
        });
        let payment_channel = match resolve_epay_channel(&config, requested_channel) {
            Ok(value) => value,
            Err(detail) => {
                return build_auth_error_response(http::StatusCode::BAD_REQUEST, detail, false);
            }
        };
        let WalletRechargePaymentBreakdown {
            base_pay_amount,
            fee_amount,
            pay_amount,
            exchange_rate,
        } = match wallet_recharge_payment_breakdown(
            payload.amount_usd,
            &config.pay_currency,
            config.usd_exchange_rate,
            payment_channel.fee_rate,
        ) {
            Ok(value) => value,
            Err(detail) => {
                return build_auth_error_response(http::StatusCode::BAD_REQUEST, detail, false)
            }
        };
        let Some(callback_base_url) = epay_callback_base_url(config.callback_base_url.as_deref())
        else {
            return build_auth_error_response(
                http::StatusCode::BAD_REQUEST,
                "epay callback_base_url is required",
                false,
            );
        };
        let payment_channel_id = payment_channel.channel.clone();
        let claim_token = wallet_recharge_claim_token();
        let expires_at_unix_secs = expires_at.timestamp().max(0) as u64;
        let placeholder = json!({
            "gateway": "epay",
            "gateway_order_id": order_no.clone(),
            "order_kind": WALLET_RECHARGE_ORDER_KIND,
            "payment_channel": payment_channel_id.clone(),
            "pay_amount": pay_amount,
            "pay_currency": config.pay_currency.clone(),
            "exchange_rate": exchange_rate,
            "integration_status": "checkout_pending",
        });
        let placeholder =
            match wallet_recharge_claimed_placeholder(&placeholder, &claim_token, now_unix_secs) {
                Ok(value) => value,
                Err(detail) => {
                    return build_auth_error_response(
                        http::StatusCode::INTERNAL_SERVER_ERROR,
                        detail,
                        false,
                    )
                }
            };
        let order_record = {
            let outcome = match state
                .create_wallet_recharge_order(
                    aether_data::repository::wallet::CreateWalletRechargeOrderInput {
                        preferred_wallet_id: wallet.as_ref().map(|value| value.id.clone()),
                        user_id: auth.user.id.clone(),
                        amount_usd: payload.amount_usd,
                        pay_amount: Some(pay_amount),
                        pay_currency: Some(config.pay_currency.clone()),
                        exchange_rate: Some(exchange_rate),
                        payment_method: "epay".to_string(),
                        payment_provider: Some("epay".to_string()),
                        payment_channel: Some(payment_channel_id.clone()),
                        gateway_order_id: order_no.clone(),
                        gateway_response: placeholder.clone(),
                        order_no: order_no.clone(),
                        expires_at_unix_secs,
                    },
                )
                .await
            {
                Ok(Some(value)) => value,
                Ok(None) => return build_wallet_recharge_storage_unavailable_response(),
                Err(err) => {
                    return build_auth_error_response(
                        http::StatusCode::INTERNAL_SERVER_ERROR,
                        format!("wallet recharge create failed: {err:?}"),
                        false,
                    )
                }
            };
            match outcome {
                aether_data::repository::wallet::CreateWalletRechargeOrderOutcome::Created(
                    order,
                ) => {
                    if wallet_recharge_order_has_checkout(&order) {
                    return wallet_recharge_replay_response(state, &order).await;
                    }
                    order
                }
                aether_data::repository::wallet::CreateWalletRechargeOrderOutcome::Existing(
                    order,
                ) => {
                    if !wallet_recharge_order_matches_effective_request(
                        &order,
                        "epay",
                        &payment_channel_id,
                        payload.amount_usd,
                        pay_amount,
                        &config.pay_currency,
                        exchange_rate,
                    ) {
                        return build_auth_error_response(
                            http::StatusCode::CONFLICT,
                            "幂等订单的支付参数已发生变化，请重新发起充值",
                            false,
                        );
                    }
                    if wallet_recharge_order_has_checkout(&order) {
                    return wallet_recharge_replay_response(state, &order).await;
                    }
                    if !wallet_recharge_order_is_reclaimable_placeholder(
                        &order,
                        now_unix_secs,
                    ) {
                        if wallet_recharge_order_is_pending_and_live(&order) {
                            return build_auth_error_response(
                                http::StatusCode::CONFLICT,
                                "充值订单正在创建，请稍后重试",
                                false,
                            );
                        }
                    return wallet_recharge_replay_response(state, &order).await;
                    }
                    match reclaim_wallet_recharge_checkout(
                        state,
                        &order,
                        placeholder.clone(),
                        &claim_token,
                        expires_at_unix_secs,
                    )
                    .await
                    {
                        Ok(order) => order,
                        Err(detail) => {
                            return build_auth_error_response(
                                http::StatusCode::CONFLICT,
                                detail,
                                false,
                            )
                        }
                    }
                }
                aether_data::repository::wallet::CreateWalletRechargeOrderOutcome::WalletInactive => {
                    return build_auth_error_response(
                        http::StatusCode::BAD_REQUEST,
                        "wallet is not active",
                        false,
                    )
                }
            }
        };
        if !wallet_recharge_order_matches_effective_request(
            &order_record,
            "epay",
            &payment_channel_id,
            payload.amount_usd,
            pay_amount,
            &config.pay_currency,
            exchange_rate,
        ) {
            best_effort_fail_wallet_recharge_checkout(
                state,
                &order_record,
                &claim_token,
                "充值订单支付参数校验失败",
            )
            .await;
            return build_auth_error_response(
                http::StatusCode::CONFLICT,
                "幂等订单的支付参数已发生变化，请重新发起充值",
                false,
            );
        }
        let checkout = match build_epay_checkout_url(
            &config,
            &EpayCheckoutInput {
                order_no: order_no.clone(),
                channel: payment_channel_id,
                subject: "钱包充值".to_string(),
                pay_amount,
                notify_url: format!("{callback_base_url}/api/payment/epay/notify"),
                return_url: format!("{callback_base_url}/api/payment/epay/return"),
            },
        ) {
            Ok(value) => value,
            Err(detail) => {
                best_effort_fail_wallet_recharge_checkout(
                    state,
                    &order_record,
                    &claim_token,
                    &detail,
                )
                .await;
                return build_auth_error_response(http::StatusCode::BAD_REQUEST, detail, false);
            }
        };
        let checkout = add_wallet_recharge_fee_metadata(
            checkout,
            base_pay_amount,
            payment_channel.fee_rate,
            fee_amount,
        );
        let checkout_for_storage =
            attach_wallet_recharge_claim_token(checkout.clone(), &claim_token);
        let stored_gateway_response = match prepare_wallet_gateway_response_for_storage(
            state,
            "epay",
            &order_no,
            &auth.user.id,
            &checkout_for_storage,
        ) {
            Ok(value) => value,
            Err(detail) => {
                best_effort_fail_wallet_recharge_checkout(
                    state,
                    &order_record,
                    &claim_token,
                    &detail,
                )
                .await;
                return build_auth_error_response(
                    http::StatusCode::INTERNAL_SERVER_ERROR,
                    detail,
                    false,
                );
            }
        };
        let stored_order = match state
            .update_wallet_recharge_checkout(
                aether_data::repository::wallet::UpdateWalletRechargeCheckoutInput {
                    order_id: order_record.id.clone(),
                    gateway_order_id: order_no.clone(),
                    gateway_response: stored_gateway_response,
                },
            )
            .await
        {
            Ok(Some(aether_data::repository::wallet::WalletMutationOutcome::Applied(order))) => {
                order
            }
            Ok(Some(aether_data::repository::wallet::WalletMutationOutcome::NotFound)) => {
                best_effort_fail_wallet_recharge_checkout(
                    state,
                    &order_record,
                    &claim_token,
                    "充值订单已不存在",
                )
                .await;
                return build_auth_error_response(
                    http::StatusCode::CONFLICT,
                    "充值订单已不存在",
                    false,
                );
            }
            Ok(Some(aether_data::repository::wallet::WalletMutationOutcome::Invalid(detail))) => {
                if let Some(order) = settled_wallet_recharge_after_checkout_conflict(
                    state,
                    &auth.user.id,
                    &order_record.id,
                )
                .await
                {
                    let order_payload = wallet_payment_order_payload_from_record(&order);
                    return mark_sensitive_response_no_store(build_auth_json_response(
                        http::StatusCode::OK,
                        json!({
                            "order": order_payload,
                            "payment_instructions": wallet_recharge_replay_payment_instructions(
                                state, &order
                            ).await,
                        }),
                        None,
                    ));
                }
                best_effort_fail_wallet_recharge_checkout(
                    state,
                    &order_record,
                    &claim_token,
                    &detail,
                )
                .await;
                return build_auth_error_response(http::StatusCode::CONFLICT, detail, false);
            }
            Ok(None) => {
                best_effort_fail_wallet_recharge_checkout(
                    state,
                    &order_record,
                    &claim_token,
                    "钱包充值后端暂不可用",
                )
                .await;
                return build_wallet_recharge_storage_unavailable_response();
            }
            Err(err) => {
                best_effort_fail_wallet_recharge_checkout(
                    state,
                    &order_record,
                    &claim_token,
                    &format!("wallet recharge checkout update failed: {err:?}"),
                )
                .await;
                return build_auth_error_response(
                    http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("wallet recharge checkout update failed: {err:?}"),
                    false,
                );
            }
        };
        let order_payload = wallet_payment_order_payload_from_record(&stored_order);
        // The provider call can race a payment callback.  The repository may
        // therefore return an already-credited order even though this request
        // still has a checkout response in hand.  Rebuild instructions from
        // the final persisted state so settled/expired orders never receive a
        // payment URL or client secret.
        let payment_instructions =
            wallet_recharge_replay_payment_instructions(state, &stored_order).await;
        return mark_sensitive_response_no_store(build_auth_json_response(
            http::StatusCode::OK,
            json!({
                "order": order_payload,
                "payment_instructions": payment_instructions,
            }),
            None,
        ));
    }
    let requested_provider = payload
        .payment_provider
        .as_deref()
        .unwrap_or(payload.payment_method.as_str());
    if matches!(requested_provider, "alipay" | "wxpay" | "stripe") {
        let mut record = match state.find_payment_gateway_config(requested_provider).await {
            Ok(Some(value)) if value.enabled && value.merchant_key_encrypted.is_some() => value,
            Ok(Some(_)) | Ok(None) => {
                return build_auth_error_response(
                    http::StatusCode::BAD_REQUEST,
                    "支付网关未启用或密钥未配置",
                    false,
                )
            }
            Err(err) => {
                return build_auth_error_response(
                    http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("payment gateway lookup failed: {err:?}"),
                    false,
                )
            }
        };
        record.pay_currency = match normalize_payment_currency(&record.pay_currency, "pay_currency")
        {
            Ok(value) => value,
            Err(_) => {
                return build_auth_error_response(
                    http::StatusCode::BAD_REQUEST,
                    "支付网关币种配置无效",
                    false,
                )
            }
        };
        if payload.amount_usd < record.min_recharge_usd {
            return build_auth_error_response(
                http::StatusCode::BAD_REQUEST,
                "充值金额低于支付网关最小金额",
                false,
            );
        }
        let payment_channel = match resolve_direct_gateway_channel(
            requested_provider,
            &record,
            payload.payment_channel.as_deref(),
        ) {
            Ok(value) => value,
            Err(detail) => {
                return build_auth_error_response(http::StatusCode::BAD_REQUEST, detail, false)
            }
        };
        let WalletRechargePaymentBreakdown {
            base_pay_amount,
            fee_amount,
            pay_amount,
            exchange_rate,
        } = match wallet_recharge_payment_breakdown(
            payload.amount_usd,
            &record.pay_currency,
            record.usd_exchange_rate,
            payment_channel.fee_rate,
        ) {
            Ok(value) => value,
            Err(detail) => {
                return build_auth_error_response(http::StatusCode::BAD_REQUEST, detail, false)
            }
        };
        let payment_channel_id = payment_channel.channel.clone();
        let claim_token = wallet_recharge_claim_token();
        let expires_at_unix_secs = expires_at.timestamp().max(0) as u64;
        let placeholder = json!({
            "gateway": requested_provider,
            "gateway_order_id": order_no.clone(),
            "order_kind": WALLET_RECHARGE_ORDER_KIND,
            "payment_channel": payment_channel_id.clone(),
            "pay_amount": pay_amount,
            "pay_currency": record.pay_currency.clone(),
            "exchange_rate": exchange_rate,
            "integration_status": "checkout_pending",
        });
        let placeholder =
            match wallet_recharge_claimed_placeholder(&placeholder, &claim_token, now_unix_secs) {
                Ok(value) => value,
                Err(detail) => {
                    return build_auth_error_response(
                        http::StatusCode::INTERNAL_SERVER_ERROR,
                        detail,
                        false,
                    )
                }
            };
        let order_record = {
            let outcome = match state
                .create_wallet_recharge_order(
                    aether_data::repository::wallet::CreateWalletRechargeOrderInput {
                        preferred_wallet_id: wallet.as_ref().map(|value| value.id.clone()),
                        user_id: auth.user.id.clone(),
                        amount_usd: payload.amount_usd,
                        pay_amount: Some(pay_amount),
                        pay_currency: Some(record.pay_currency.clone()),
                        exchange_rate: Some(exchange_rate),
                        payment_method: requested_provider.to_string(),
                        payment_provider: Some(requested_provider.to_string()),
                        payment_channel: Some(payment_channel_id.clone()),
                        gateway_order_id: order_no.clone(),
                        gateway_response: placeholder.clone(),
                        order_no: order_no.clone(),
                        expires_at_unix_secs,
                    },
                )
                .await
            {
                Ok(Some(value)) => value,
                Ok(None) => return build_wallet_recharge_storage_unavailable_response(),
                Err(err) => {
                    return build_auth_error_response(
                        http::StatusCode::INTERNAL_SERVER_ERROR,
                        format!("wallet recharge create failed: {err:?}"),
                        false,
                    )
                }
            };
            match outcome {
                aether_data::repository::wallet::CreateWalletRechargeOrderOutcome::Created(
                    order,
                ) => {
                    if wallet_recharge_order_has_checkout(&order) {
                    return wallet_recharge_replay_response(state, &order).await;
                    }
                    order
                }
                aether_data::repository::wallet::CreateWalletRechargeOrderOutcome::Existing(
                    order,
                ) => {
                    if !wallet_recharge_order_matches_effective_request(
                        &order,
                        requested_provider,
                        &payment_channel_id,
                        payload.amount_usd,
                        pay_amount,
                        &record.pay_currency,
                        exchange_rate,
                    ) {
                        return build_auth_error_response(
                            http::StatusCode::CONFLICT,
                            "幂等订单的支付参数已发生变化，请重新发起充值",
                            false,
                        );
                    }
                    if wallet_recharge_order_has_checkout(&order) {
                    return wallet_recharge_replay_response(state, &order).await;
                    }
                    if !wallet_recharge_order_is_reclaimable_placeholder(
                        &order,
                        now_unix_secs,
                    ) {
                        if wallet_recharge_order_is_pending_and_live(&order) {
                            return build_auth_error_response(
                                http::StatusCode::CONFLICT,
                                "充值订单正在创建，请稍后重试",
                                false,
                            );
                        }
                    return wallet_recharge_replay_response(state, &order).await;
                    }
                    match reclaim_wallet_recharge_checkout(
                        state,
                        &order,
                        placeholder.clone(),
                        &claim_token,
                        expires_at_unix_secs,
                    )
                    .await
                    {
                        Ok(order) => order,
                        Err(detail) => {
                            return build_auth_error_response(
                                http::StatusCode::CONFLICT,
                                detail,
                                false,
                            )
                        }
                    }
                }
                aether_data::repository::wallet::CreateWalletRechargeOrderOutcome::WalletInactive => {
                    return build_auth_error_response(
                        http::StatusCode::BAD_REQUEST,
                        "wallet is not active",
                        false,
                    )
                }
            }
        };
        if !wallet_recharge_order_matches_effective_request(
            &order_record,
            requested_provider,
            &payment_channel_id,
            payload.amount_usd,
            pay_amount,
            &record.pay_currency,
            exchange_rate,
        ) {
            best_effort_fail_wallet_recharge_checkout(
                state,
                &order_record,
                &claim_token,
                "充值订单支付参数校验失败",
            )
            .await;
            return build_auth_error_response(
                http::StatusCode::CONFLICT,
                "幂等订单的支付参数已发生变化，请重新发起充值",
                false,
            );
        }
        let checkout = if requested_provider == "stripe" {
            let mut retry_with_new_provider_key = false;
            loop {
                let idempotency_key =
                    stripe_wallet_idempotency_key(&order_no, retry_with_new_provider_key);
                match create_stripe_wallet_recharge_checkout(
                    state,
                    &record,
                    &payment_channel_id,
                    &payment_channel.display_name,
                    &order_no,
                    pay_amount,
                    expires_at,
                    &idempotency_key,
                )
                .await
                {
                    Ok(value) => break value,
                    Err(StripeWalletCheckoutError::Canceled) if !retry_with_new_provider_key => {
                        // Stripe can retain a canceled PaymentIntent under an
                        // idempotency key. Rotate the provider key once while
                        // keeping the local order and metadata identity stable.
                        retry_with_new_provider_key = true;
                    }
                    Err(StripeWalletCheckoutError::Canceled) => {
                        let detail = "Stripe PaymentIntent 已取消，请稍后重试".to_string();
                        best_effort_fail_wallet_recharge_checkout(
                            state,
                            &order_record,
                            &claim_token,
                            &detail,
                        )
                        .await;
                        return build_auth_error_response(
                            http::StatusCode::BAD_GATEWAY,
                            detail,
                            false,
                        );
                    }
                    Err(StripeWalletCheckoutError::Uncertain(detail)) => {
                        best_effort_mark_wallet_recharge_checkout_uncertain(
                            state,
                            &order_record,
                            &claim_token,
                            &detail,
                        )
                        .await;
                        return build_auth_error_response(
                            http::StatusCode::BAD_GATEWAY,
                            detail,
                            false,
                        );
                    }
                    Err(StripeWalletCheckoutError::Failed(detail)) => {
                        best_effort_fail_wallet_recharge_checkout(
                            state,
                            &order_record,
                            &claim_token,
                            &detail,
                        )
                        .await;
                        return build_auth_error_response(
                            http::StatusCode::BAD_GATEWAY,
                            detail,
                            false,
                        );
                    }
                }
            }
        } else {
            let Some(callback_base_url) =
                epay_callback_base_url(record.callback_base_url.as_deref())
            else {
                best_effort_fail_wallet_recharge_checkout(
                    state,
                    &order_record,
                    &claim_token,
                    "支付网关 callback_base_url is required",
                )
                .await;
                return build_auth_error_response(
                    http::StatusCode::BAD_REQUEST,
                    "支付网关 callback_base_url is required",
                    false,
                );
            };
            let direct_input = DirectPaymentCheckoutInput {
                payment_channel: payment_channel_id.clone(),
                display_name: payment_channel.display_name.clone(),
                order_no: order_no.clone(),
                subject: "钱包充值".to_string(),
                pay_amount,
                pay_currency: record.pay_currency.clone(),
                notify_url: format!("{callback_base_url}/api/payment/{requested_provider}/notify"),
                return_url: Some(wallet_payment_return_url(
                    &callback_base_url,
                    requested_provider,
                    &order_no,
                )),
                client_ip: Some(client_ip.to_string()),
                expires_at,
            };
            let result = match requested_provider {
                "alipay" => create_alipay_direct_checkout(state, &direct_input).await,
                "wxpay" => create_wxpay_direct_checkout(state, &direct_input).await,
                _ => Err(DirectPaymentCheckoutError::Failed(
                    "支付网关不支持".to_string(),
                )),
            };
            match result {
                Ok(value) => value,
                Err(DirectPaymentCheckoutError::Uncertain(detail)) => {
                    // The direct provider may have accepted the request before
                    // its response was lost. Keep the order non-reclaimable so
                    // a retry cannot create a second payment capability.
                    best_effort_mark_wallet_recharge_checkout_uncertain(
                        state,
                        &order_record,
                        &claim_token,
                        &detail,
                    )
                    .await;
                    return build_auth_error_response(http::StatusCode::BAD_GATEWAY, detail, false);
                }
                Err(
                    error @ (DirectPaymentCheckoutError::Canceled
                    | DirectPaymentCheckoutError::Failed(_)),
                ) => {
                    let detail = error.into_detail();
                    best_effort_fail_wallet_recharge_checkout(
                        state,
                        &order_record,
                        &claim_token,
                        &detail,
                    )
                    .await;
                    return build_auth_error_response(http::StatusCode::BAD_GATEWAY, detail, false);
                }
            }
        };
        let checkout = add_wallet_recharge_fee_metadata(
            checkout,
            base_pay_amount,
            payment_channel.fee_rate,
            fee_amount,
        );
        let gateway_order_id = checkout
            .get("gateway_order_id")
            .and_then(Value::as_str)
            .unwrap_or(&order_no)
            .to_string();
        let checkout_for_storage =
            attach_wallet_recharge_claim_token(checkout.clone(), &claim_token);
        let stored_gateway_response = match prepare_wallet_gateway_response_for_storage(
            state,
            requested_provider,
            &order_no,
            &auth.user.id,
            &checkout_for_storage,
        ) {
            Ok(value) => value,
            Err(detail) => {
                // The provider has already returned a checkout response. A
                // local projection/encryption failure therefore has an
                // unknown provider outcome and must not permit a replacement
                // checkout on retry.
                best_effort_mark_wallet_recharge_checkout_uncertain(
                    state,
                    &order_record,
                    &claim_token,
                    &detail,
                )
                .await;
                return build_auth_error_response(
                    http::StatusCode::INTERNAL_SERVER_ERROR,
                    detail,
                    false,
                );
            }
        };
        let stored_order = match state
            .update_wallet_recharge_checkout(
                aether_data::repository::wallet::UpdateWalletRechargeCheckoutInput {
                    order_id: order_record.id.clone(),
                    gateway_order_id,
                    gateway_response: stored_gateway_response,
                },
            )
            .await
        {
            Ok(Some(aether_data::repository::wallet::WalletMutationOutcome::Applied(order))) => {
                order
            }
            Ok(Some(aether_data::repository::wallet::WalletMutationOutcome::NotFound)) => {
                best_effort_mark_wallet_recharge_checkout_uncertain(
                    state,
                    &order_record,
                    &claim_token,
                    "充值订单已不存在",
                )
                .await;
                return build_auth_error_response(
                    http::StatusCode::CONFLICT,
                    "充值订单已不存在",
                    false,
                );
            }
            Ok(Some(aether_data::repository::wallet::WalletMutationOutcome::Invalid(detail))) => {
                if let Some(order) = settled_wallet_recharge_after_checkout_conflict(
                    state,
                    &auth.user.id,
                    &order_record.id,
                )
                .await
                {
                    let order_payload = wallet_payment_order_payload_from_record(&order);
                    return mark_sensitive_response_no_store(build_auth_json_response(
                        http::StatusCode::OK,
                        json!({
                            "order": order_payload,
                            "payment_instructions": wallet_recharge_replay_payment_instructions(
                                state, &order
                            ).await,
                        }),
                        None,
                    ));
                }
                best_effort_mark_wallet_recharge_checkout_uncertain(
                    state,
                    &order_record,
                    &claim_token,
                    &detail,
                )
                .await;
                return build_auth_error_response(http::StatusCode::CONFLICT, detail, false);
            }
            Ok(None) => {
                best_effort_mark_wallet_recharge_checkout_uncertain(
                    state,
                    &order_record,
                    &claim_token,
                    "钱包充值后端暂不可用",
                )
                .await;
                return build_wallet_recharge_storage_unavailable_response();
            }
            Err(err) => {
                best_effort_mark_wallet_recharge_checkout_uncertain(
                    state,
                    &order_record,
                    &claim_token,
                    &format!("wallet recharge checkout update failed: {err:?}"),
                )
                .await;
                return build_auth_error_response(
                    http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("wallet recharge checkout update failed: {err:?}"),
                    false,
                );
            }
        };
        let order_payload = wallet_payment_order_payload_from_record(&stored_order);
        // See the EPay path above: a callback may have credited the order while
        // the external checkout request was in flight.  Only replay evidence
        // from a still-live pending order.
        let payment_instructions =
            wallet_recharge_replay_payment_instructions(state, &stored_order).await;
        return mark_sensitive_response_no_store(build_auth_json_response(
            http::StatusCode::OK,
            json!({
                "order": order_payload,
                "payment_instructions": payment_instructions,
            }),
            None,
        ));
    }
    build_auth_error_response(
        http::StatusCode::BAD_REQUEST,
        format!("unsupported payment_method: {}", payload.payment_method),
        false,
    )
}

pub(super) async fn handle_wallet_recharge_options(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
) -> Response<Body> {
    if let Err(response) = resolve_authenticated_local_user(state, request_context, headers).await {
        return response;
    }
    let mut methods = Vec::new();
    if let Ok(config) = load_epay_config(state).await {
        for channel in configured_epay_channels(&config) {
            let payment_channel = channel.channel.clone();
            let display_name = channel.display_name.clone();
            let fee_rate = channel.fee_rate;
            methods.push(json!({
                "payment_method": "epay",
                "payment_provider": "epay",
                "payment_channel": payment_channel,
                "display_name": display_name,
                "pay_currency": config.pay_currency,
                "usd_exchange_rate": config.usd_exchange_rate,
                "min_recharge_usd": config.min_recharge_usd,
                "fee_rate": fee_rate,
            }));
        }
    }
    for provider in ["alipay", "wxpay", "stripe"] {
        let Ok(Some(mut record)) = state.find_payment_gateway_config(provider).await else {
            continue;
        };
        if !record.enabled || record.merchant_key_encrypted.is_none() {
            continue;
        }
        let Ok(pay_currency) = normalize_payment_currency(&record.pay_currency, "pay_currency")
        else {
            continue;
        };
        record.pay_currency = pay_currency;
        let Ok(exchange_rate) = crate::handlers::shared::effective_payment_exchange_rate(
            &record.pay_currency,
            record.usd_exchange_rate,
        ) else {
            continue;
        };
        record.usd_exchange_rate = exchange_rate;
        for DirectGatewayChannelConfig {
            channel: payment_channel,
            display_name,
            fee_rate,
        } in direct_gateway_channels(provider, &record)
        {
            methods.push(json!({
                "payment_method": provider,
                "payment_provider": provider,
                "payment_channel": payment_channel,
                "display_name": display_name,
                "pay_currency": record.pay_currency,
                "usd_exchange_rate": record.usd_exchange_rate,
                "min_recharge_usd": record.min_recharge_usd,
                "fee_rate": fee_rate,
            }));
        }
    }
    build_auth_json_response(http::StatusCode::OK, json!({ "items": methods }), None)
}

pub(super) async fn handle_wallet_recharge_list(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
) -> Response<Body> {
    let auth = match resolve_authenticated_local_user(state, request_context, headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let query = request_context.request_query_string.as_deref();
    let limit = match parse_wallet_limit(query) {
        Ok(value) => value,
        Err(detail) => {
            return build_auth_error_response(http::StatusCode::BAD_REQUEST, detail, false)
        }
    };
    let offset = match parse_wallet_offset(query) {
        Ok(value) => value,
        Err(detail) => {
            return build_auth_error_response(http::StatusCode::BAD_REQUEST, detail, false)
        }
    };
    let wallet = match state
        .find_wallet(aether_data::repository::wallet::WalletLookupKey::UserId(
            &auth.user.id,
        ))
        .await
    {
        Ok(value) => value,
        Err(err) => {
            return build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("wallet lookup failed: {err:?}"),
                false,
            )
        }
    };

    let (items, total) = match state
        .list_wallet_payment_orders_by_user_id(&auth.user.id, limit, offset)
        .await
    {
        Ok(page) => (
            page.items
                .iter()
                .map(wallet_payment_order_payload_from_record)
                .collect::<Vec<_>>(),
            page.total,
        ),
        Err(err) => {
            return build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("wallet recharge lookup failed: {err:?}"),
                false,
            )
        }
    };
    #[cfg(test)]
    let (items, total) =
        if !state.has_database_wallet_data_writer() && items.is_empty() && total == 0 {
            let (items, total) = wallet_test_recharge_orders_for_user(&auth.user.id, limit, offset);
            (
                items
                    .into_iter()
                    .map(wallet_test_recharge_public_payload)
                    .collect(),
                total,
            )
        } else {
            (items, total)
        };

    let mut payload = json!({
        "items": items,
        "total": total,
        "limit": limit,
        "offset": offset,
    });
    if let Some(object) = payload.as_object_mut() {
        if let Some(wallet_payload) = build_wallet_payload(wallet.as_ref()).as_object() {
            object.extend(wallet_payload.clone());
        }
    }
    build_auth_json_response(http::StatusCode::OK, payload, None)
}

pub(super) async fn handle_wallet_recharge_detail(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
) -> Response<Body> {
    let auth = match resolve_authenticated_local_user(state, request_context, headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Some(order_id) = wallet_order_id_from_path(&request_context.request_path) else {
        return build_auth_error_response(
            http::StatusCode::NOT_FOUND,
            "Payment order not found",
            false,
        );
    };
    match state
        .find_wallet_payment_order_by_user_id(&auth.user.id, &order_id)
        .await
    {
        Ok(Some(order)) => {
            let payment_instructions = wallet_payment_instructions_from_stored(state, &order).await;
            mark_sensitive_response_no_store(build_auth_json_response(
                http::StatusCode::OK,
                json!({
                    "order": wallet_payment_order_payload_from_record(&order),
                    "payment_instructions": payment_instructions,
                }),
                None,
            ))
        }
        Ok(None) => {
            #[cfg(test)]
            if let Some(order) = wallet_test_recharge_order_by_id(&auth.user.id, &order_id) {
                return mark_sensitive_response_no_store(build_auth_json_response(
                    http::StatusCode::OK,
                    json!({
                        "order": wallet_test_recharge_public_payload(order.clone()),
                        "payment_instructions": wallet_test_recharge_replay_payment_instructions(&order),
                    }),
                    None,
                ));
            }
            build_auth_error_response(
                http::StatusCode::NOT_FOUND,
                "Payment order not found",
                false,
            )
        }
        Err(err) => build_auth_error_response(
            http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("wallet recharge detail lookup failed: {err:?}"),
            false,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        attach_wallet_recharge_claim_token, prepare_wallet_gateway_response_for_storage,
        prepare_wallet_gateway_response_for_storage_with_encrypt, sanitize_wallet_gateway_response,
        stripe_wallet_checkout_response_is_canceled, stripe_wallet_idempotency_key,
        wallet_payment_instructions_from_checkout, wallet_payment_instructions_from_stored,
        wallet_payment_order_payload_from_record, wallet_recharge_order_is_settled,
        wallet_recharge_order_matches_effective_request, wallet_recharge_order_matches_request,
        wallet_recharge_payment_breakdown, wallet_recharge_replay_payment_instructions,
        wallet_test_recharge_payload_matches_request,
        wallet_test_recharge_replay_payment_instructions, AppState,
        NormalizedWalletCreateRechargeRequest, WalletRechargePaymentBreakdown,
        STRIPE_CLIENT_SECRET_ENCRYPTED_KEY, WALLET_RECHARGE_ORDER_KIND,
    };
    use aether_crypto::DEVELOPMENT_ENCRYPTION_KEY;
    use aether_data::repository::auth::InMemoryAuthApiKeySnapshotRepository;
    use aether_data::repository::wallet::{
        CreateWalletRechargeOrderInput, CreateWalletRechargeOrderOutcome, InMemoryWalletRepository,
        StoredAdminPaymentOrder, WalletReadRepository, WalletWriteRepository,
    };
    use chrono::Utc;
    use serde_json::json;
    use std::sync::Arc;

    use crate::data::GatewayDataState;
    use crate::handlers::shared::encrypt_catalog_secret_with_fallbacks;

    fn stored_checkout_order(
        provider: &str,
        status: &str,
        expires_at_unix_secs: Option<u64>,
        gateway_response: serde_json::Value,
    ) -> StoredAdminPaymentOrder {
        StoredAdminPaymentOrder {
            id: "order-test".to_string(),
            order_no: "po-test".to_string(),
            wallet_id: "wallet-test".to_string(),
            user_id: Some("user-test".to_string()),
            amount_usd: 10.0,
            pay_amount: Some(10.0),
            pay_currency: Some("USD".to_string()),
            exchange_rate: Some(1.0),
            refunded_amount_usd: 0.0,
            refundable_amount_usd: 0.0,
            payment_method: provider.to_string(),
            payment_provider: Some(provider.to_string()),
            order_kind: super::WALLET_RECHARGE_ORDER_KIND.to_string(),
            gateway_order_id: Some("gateway-test".to_string()),
            gateway_response: Some(gateway_response),
            status: status.to_string(),
            created_at_unix_ms: 1,
            paid_at_unix_secs: None,
            credited_at_unix_secs: None,
            expires_at_unix_secs,
        }
    }

    #[test]
    fn stripe_canceled_response_detection_is_case_and_whitespace_insensitive() {
        assert!(stripe_wallet_checkout_response_is_canceled(&json!({
            "status": " canceled "
        })));
        assert!(stripe_wallet_checkout_response_is_canceled(&json!({
            "status": "CANCELED"
        })));
        assert!(!stripe_wallet_checkout_response_is_canceled(&json!({
            "status": "requires_payment_method"
        })));
        assert!(!stripe_wallet_checkout_response_is_canceled(&json!({
            "status": null
        })));
        assert!(!stripe_wallet_checkout_response_is_canceled(&json!({
            "status": ["canceled"]
        })));
    }

    #[test]
    fn stripe_wallet_retry_key_is_stable_distinct_and_keeps_order_identity() {
        let order_no = "po_202608291234567890123456789012345678";
        let initial = stripe_wallet_idempotency_key(order_no, false);
        let retry = stripe_wallet_idempotency_key(order_no, true);

        assert_ne!(initial, retry);
        assert_eq!(initial, stripe_wallet_idempotency_key(order_no, false));
        assert_eq!(retry, stripe_wallet_idempotency_key(order_no, true));
        assert!(initial.contains(order_no));
        assert!(retry.contains(order_no));
        assert!(initial.len() <= 255);
        assert!(retry.len() <= 255);
        assert!(retry.ends_with("-retry-1"));
    }

    #[tokio::test]
    async fn stripe_client_secret_is_encrypted_at_rest_and_only_rehydrated_explicitly() {
        let state = AppState::new().expect("test app state");
        let client_secret = "pi_test_123_secret_payment_capability";
        let checkout = json!({
            "gateway": "stripe",
            "gateway_order_id": "pi_test_123",
            "intent_id": "pi_test_123",
            "client_secret": client_secret,
            "publishable_key": "pk_test_public",
            "payment_channel": "card",
        });

        let stored = prepare_wallet_gateway_response_for_storage(
            &state,
            "stripe",
            "po-test",
            "user-test",
            &checkout,
        )
        .expect("Stripe checkout should encrypt");
        assert!(stored.get("client_secret").is_none());
        assert!(stored
            .get(STRIPE_CLIENT_SECRET_ENCRYPTED_KEY)
            .and_then(serde_json::Value::as_str)
            .is_some());
        assert!(!stored.to_string().contains(client_secret));

        let projected = sanitize_wallet_gateway_response(Some(stored.clone()));
        assert!(projected.get("client_secret").is_none());
        assert!(projected.get(STRIPE_CLIENT_SECRET_ENCRYPTED_KEY).is_none());

        let fresh = wallet_payment_instructions_from_checkout("stripe", &checkout);
        assert_eq!(fresh["client_secret"], client_secret);
        let order = stored_checkout_order("stripe", "pending", Some(u64::MAX), stored);
        let resumed = wallet_payment_instructions_from_stored(&state, &order).await;
        assert_eq!(resumed["client_secret"], client_secret);
    }

    #[tokio::test]
    async fn wallet_stripe_client_secret_cannot_be_copied_to_another_order_or_kind() {
        let state = AppState::new().expect("test app state");
        let client_secret = "pi_wallet_source_secret_capability";
        let stored = prepare_wallet_gateway_response_for_storage(
            &state,
            "stripe",
            "po-source",
            "user-source",
            &json!({
                "gateway": "stripe",
                "client_secret": client_secret,
                "publishable_key": "pk_test_public",
            }),
        )
        .expect("source checkout should encrypt");
        let mut source = stored_checkout_order("stripe", "pending", Some(u64::MAX), stored.clone());
        source.id = "order-source".to_string();
        source.order_no = "po-source".to_string();
        source.user_id = Some("user-source".to_string());
        assert_eq!(
            wallet_payment_instructions_from_stored(&state, &source).await["client_secret"],
            client_secret
        );

        let mut foreign_order = source.clone();
        foreign_order.id = "order-foreign".to_string();
        foreign_order.order_no = "po-foreign".to_string();
        assert!(
            wallet_payment_instructions_from_stored(&state, &foreign_order)
                .await
                .get("client_secret")
                .is_none()
        );

        let mut foreign_owner = source.clone();
        foreign_owner.id = "order-foreign-owner".to_string();
        foreign_owner.user_id = Some("user-foreign".to_string());
        assert!(
            wallet_payment_instructions_from_stored(&state, &foreign_owner)
                .await
                .get("client_secret")
                .is_none()
        );

        let mut foreign_kind = source;
        foreign_kind.id = "order-plan".to_string();
        foreign_kind.order_kind = "plan_purchase".to_string();
        assert!(
            wallet_payment_instructions_from_stored(&state, &foreign_kind)
                .await
                .get("client_secret")
                .is_none()
        );
    }

    #[tokio::test]
    async fn legacy_wallet_stripe_secret_is_atomically_migrated_but_unknown_envelope_is_not() {
        let wallet_repository = Arc::new(InMemoryWalletRepository::default());
        let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::default());
        let state = AppState::new()
            .expect("test app state")
            .with_data_state_for_tests(
                GatewayDataState::with_auth_and_wallet_for_tests(
                    auth_repository,
                    Arc::clone(&wallet_repository),
                )
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            );
        let plaintext = "pi_legacy_wallet_secret_capability";
        let legacy = encrypt_catalog_secret_with_fallbacks(&state, plaintext)
            .expect("legacy Fernet should encrypt");
        let expires_at = Utc::now().timestamp().max(0) as u64 + 600;
        let order = match wallet_repository
            .create_wallet_recharge_order(CreateWalletRechargeOrderInput {
                preferred_wallet_id: Some("wallet-legacy".to_string()),
                user_id: "user-legacy".to_string(),
                amount_usd: 10.0,
                pay_amount: Some(10.0),
                pay_currency: Some("USD".to_string()),
                exchange_rate: Some(1.0),
                payment_method: "stripe".to_string(),
                payment_provider: Some("stripe".to_string()),
                payment_channel: Some("card".to_string()),
                gateway_order_id: "pi-legacy".to_string(),
                gateway_response: json!({
                    "gateway": "stripe",
                    "publishable_key": "pk_test_public",
                    (STRIPE_CLIENT_SECRET_ENCRYPTED_KEY): legacy,
                }),
                order_no: "po-legacy".to_string(),
                expires_at_unix_secs: expires_at,
            })
            .await
            .expect("legacy order creation should run")
        {
            CreateWalletRechargeOrderOutcome::Created(order) => order,
            other => panic!("unexpected legacy order outcome: {other:?}"),
        };

        let instructions = wallet_payment_instructions_from_stored(&state, &order).await;
        assert_eq!(instructions["client_secret"], plaintext);
        let migrated = wallet_repository
            .find_admin_payment_order(&order.id)
            .await
            .expect("migrated order should be readable")
            .expect("migrated order should remain");
        assert!(migrated
            .gateway_response
            .as_ref()
            .and_then(|response| response[STRIPE_CLIENT_SECRET_ENCRYPTED_KEY].as_str())
            .is_some_and(|value| value.starts_with(
                "aether-payment-order-stripe-client-secret-v2:aether-runtime-secret-v1:"
            )));

        let unknown = "aether-payment-order-stripe-client-secret-v3:unknown";
        let unknown_order = match wallet_repository
            .create_wallet_recharge_order(CreateWalletRechargeOrderInput {
                preferred_wallet_id: Some("wallet-unknown".to_string()),
                user_id: "user-unknown".to_string(),
                amount_usd: 10.0,
                pay_amount: Some(10.0),
                pay_currency: Some("USD".to_string()),
                exchange_rate: Some(1.0),
                payment_method: "stripe".to_string(),
                payment_provider: Some("stripe".to_string()),
                payment_channel: Some("card".to_string()),
                gateway_order_id: "pi-unknown".to_string(),
                gateway_response: json!({
                    "gateway": "stripe",
                    "publishable_key": "pk_test_public",
                    (STRIPE_CLIENT_SECRET_ENCRYPTED_KEY): unknown,
                }),
                order_no: "po-unknown".to_string(),
                expires_at_unix_secs: expires_at,
            })
            .await
            .expect("unknown-envelope order creation should run")
        {
            CreateWalletRechargeOrderOutcome::Created(order) => order,
            other => panic!("unexpected unknown-envelope outcome: {other:?}"),
        };
        assert!(
            wallet_payment_instructions_from_stored(&state, &unknown_order)
                .await
                .get("client_secret")
                .is_none()
        );
        let unchanged = wallet_repository
            .find_admin_payment_order(&unknown_order.id)
            .await
            .expect("unknown-envelope order should be readable")
            .expect("unknown-envelope order should remain");
        assert_eq!(
            unchanged
                .gateway_response
                .as_ref()
                .and_then(|response| response[STRIPE_CLIENT_SECRET_ENCRYPTED_KEY].as_str()),
            Some(unknown)
        );
    }

    #[test]
    fn stripe_checkout_is_not_persistable_when_client_secret_encryption_fails() {
        let checkout = json!({
            "gateway": "stripe",
            "client_secret": "pi_test_123_secret_payment_capability",
            "publishable_key": "pk_test_public",
        });

        let error =
            prepare_wallet_gateway_response_for_storage_with_encrypt("stripe", &checkout, |_| None)
                .expect_err("encryption failure must stop persistence");

        assert_eq!(error, "Stripe client_secret 加密失败");
    }

    #[tokio::test]
    async fn stored_legacy_plaintext_client_secret_is_not_replayed() {
        let state = AppState::new().expect("test app state");
        let legacy = json!({
            "gateway": "stripe",
            "client_secret": "pi_legacy_secret_plaintext",
            "publishable_key": "pk_test_public",
        });

        let order = stored_checkout_order("stripe", "pending", Some(u64::MAX), legacy);
        let instructions = wallet_payment_instructions_from_stored(&state, &order).await;
        assert!(instructions.get("client_secret").is_none());
        assert_eq!(instructions["publishable_key"], "pk_test_public");
    }

    #[tokio::test]
    async fn stored_stripe_client_secret_is_only_rehydrated_for_live_pending_orders() {
        let state = AppState::new().expect("test app state");
        let checkout = json!({
            "gateway": "stripe",
            "client_secret": "pi_test_123_secret_payment_capability",
            "publishable_key": "pk_test_public",
        });
        let stored = prepare_wallet_gateway_response_for_storage(
            &state,
            "stripe",
            "po-test",
            "user-test",
            &checkout,
        )
        .expect("Stripe checkout should encrypt");

        for (status, expires_at) in [
            ("paid", Some(u64::MAX)),
            ("credited", Some(u64::MAX)),
            ("expired", Some(u64::MAX)),
            ("pending", Some(0)),
            ("pending", None),
        ] {
            let order = stored_checkout_order("stripe", status, expires_at, stored.clone());
            let instructions = wallet_payment_instructions_from_stored(&state, &order).await;
            assert!(
                instructions.get("client_secret").is_none(),
                "secret was replayed for status={status}, expires_at={expires_at:?}",
            );
            assert_eq!(
                instructions,
                json!({}),
                "checkout capabilities were replayed for status={status}, expires_at={expires_at:?}"
            );
        }

        let live_order = stored_checkout_order(
            "stripe",
            "pending",
            Some(chrono::Utc::now().timestamp().max(0) as u64 + 60),
            stored,
        );
        let live = wallet_payment_instructions_from_stored(&state, &live_order).await;
        assert_eq!(live["publishable_key"], "pk_test_public");
        assert_eq!(
            live["client_secret"],
            "pi_test_123_secret_payment_capability"
        );
    }

    #[tokio::test]
    async fn stored_non_stripe_payment_instructions_are_only_returned_for_live_pending_orders() {
        let state = AppState::new().expect("test app state");
        let checkout = json!({
            "gateway": "alipay",
            "payment_url": "https://pay.example.test/order",
            "payment_params": {
                "out_trade_no": "order-1",
                "sign": "signed-value",
            },
        });
        let now = chrono::Utc::now().timestamp().max(0) as u64;

        let live_order = stored_checkout_order(
            "alipay",
            "pending",
            Some(now.saturating_add(60)),
            checkout.clone(),
        );
        let live = wallet_payment_instructions_from_stored(&state, &live_order).await;
        assert_eq!(live["payment_url"], checkout["payment_url"]);

        for (status, expires_at) in [
            ("paid", Some(now.saturating_add(60))),
            ("credited", Some(now.saturating_add(60))),
            ("pending", Some(now.saturating_sub(1))),
            ("pending", None),
        ] {
            let order = stored_checkout_order("alipay", status, expires_at, checkout.clone());
            let instructions = wallet_payment_instructions_from_stored(&state, &order).await;
            assert_eq!(
                instructions,
                json!({}),
                "status={status} expires={expires_at:?}"
            );
        }
    }

    #[test]
    fn historical_order_payload_does_not_expose_checkout_capabilities() {
        let order = StoredAdminPaymentOrder {
            id: "historical-order".to_string(),
            order_no: "po-historical".to_string(),
            wallet_id: "wallet-historical".to_string(),
            user_id: Some("user-historical".to_string()),
            amount_usd: 10.0,
            pay_amount: Some(72.5),
            pay_currency: Some("CNY".to_string()),
            exchange_rate: Some(7.25),
            refunded_amount_usd: 0.0,
            refundable_amount_usd: 0.0,
            payment_method: "alipay".to_string(),
            payment_provider: Some("alipay".to_string()),
            order_kind: WALLET_RECHARGE_ORDER_KIND.to_string(),
            gateway_order_id: Some("gateway-historical".to_string()),
            gateway_response: Some(json!({
                "gateway": "alipay",
                "payment_url": "https://pay.example.test/historical",
                "payment_params": {"out_trade_no": "po-historical", "sign": "signed"},
            })),
            status: "credited".to_string(),
            // SQL adapters expose this field in seconds despite its legacy
            // name; the public projection must preserve that meaning.
            created_at_unix_ms: 1_700_000_000,
            paid_at_unix_secs: Some(1_700_000_001),
            credited_at_unix_secs: Some(1_700_000_002),
            expires_at_unix_secs: Some(1_700_000_003),
        };

        let payload = wallet_payment_order_payload_from_record(&order);
        assert_eq!(payload["gateway_response"], json!({}));
        assert_eq!(payload["created_at"], "2023-11-14T22:13:20Z");
    }

    #[test]
    fn unexpected_client_secret_is_removed_for_non_stripe_storage() {
        let state = AppState::new().expect("test app state");
        let checkout = json!({
            "gateway": "alipay",
            "payment_url": "https://pay.example.test/order",
            "client_secret": "pi_injected_secret_should_not_survive",
            "provider_private_token": "should_not_survive",
            "payment_params": {
                "client_secret": "pi_nested_secret_should_not_survive",
                "order_no": "order-1",
                "out_trade_no": "order-1",
                "sign": "signed-value",
                "nested": {"authorization": "Bearer secret"},
                "array": ["secret"],
            },
        });

        let stored = prepare_wallet_gateway_response_for_storage(
            &state,
            "alipay",
            "po-test",
            "user-test",
            &checkout,
        )
        .expect("non-Stripe checkout should project");
        assert!(stored.get("client_secret").is_none());
        assert!(stored.get("provider_private_token").is_none());
        assert!(stored["payment_params"].get("client_secret").is_none());
        assert!(stored["payment_params"].get("nested").is_none());
        assert!(stored["payment_params"].get("array").is_none());
        assert_eq!(stored["payment_params"]["out_trade_no"], "order-1");
        assert_eq!(stored["payment_params"]["sign"], "signed-value");
        assert_eq!(stored["payment_url"], checkout["payment_url"]);
    }

    #[test]
    fn checkout_claim_metadata_is_persisted_but_never_exposed() {
        let state = AppState::new().expect("test app state");
        let checkout = attach_wallet_recharge_claim_token(
            json!({
                "gateway": "alipay",
                "payment_url": "https://pay.example.test/order",
                "payment_channel": "alipay",
            }),
            "wrc_test_claim",
        );

        let stored = prepare_wallet_gateway_response_for_storage(
            &state,
            "alipay",
            "po-test",
            "user-test",
            &checkout,
        )
        .expect("checkout claim should be persistable");
        assert_eq!(
            stored
                .get("checkout_claim_token")
                .and_then(serde_json::Value::as_str),
            Some("wrc_test_claim")
        );
        let public = sanitize_wallet_gateway_response(Some(stored));
        assert!(public.get("checkout_claim_token").is_none());
    }

    #[tokio::test]
    async fn idempotent_replay_hides_expired_non_stripe_checkout() {
        let state = AppState::new().expect("test app state");
        let order = StoredAdminPaymentOrder {
            id: "expired-replay-order".to_string(),
            order_no: "po_idem_expired".to_string(),
            wallet_id: "wallet-expired-replay".to_string(),
            user_id: Some("user-expired-replay".to_string()),
            amount_usd: 10.0,
            pay_amount: Some(72.5),
            pay_currency: Some("CNY".to_string()),
            exchange_rate: Some(7.25),
            refunded_amount_usd: 0.0,
            refundable_amount_usd: 0.0,
            payment_method: "alipay".to_string(),
            payment_provider: Some("alipay".to_string()),
            order_kind: WALLET_RECHARGE_ORDER_KIND.to_string(),
            gateway_order_id: Some("po_idem_expired".to_string()),
            gateway_response: Some(json!({
                "gateway": "alipay",
                "payment_url": "https://pay.example.test/expired",
                "payment_channel": "alipay",
            })),
            status: "pending".to_string(),
            created_at_unix_ms: 0,
            paid_at_unix_secs: None,
            credited_at_unix_secs: None,
            expires_at_unix_secs: Some(0),
        };

        assert_eq!(
            wallet_recharge_replay_payment_instructions(&state, &order).await,
            json!({}),
        );
    }

    #[test]
    fn effective_matcher_rejects_a_competing_request_with_a_different_usd_amount() {
        let order = StoredAdminPaymentOrder {
            id: "placeholder-order".to_string(),
            order_no: "po_idem_placeholder".to_string(),
            wallet_id: "wallet-1".to_string(),
            user_id: Some("user-1".to_string()),
            amount_usd: 10.0,
            pay_amount: Some(72.5),
            pay_currency: Some("CNY".to_string()),
            exchange_rate: Some(7.25),
            refunded_amount_usd: 0.0,
            refundable_amount_usd: 0.0,
            payment_method: "alipay".to_string(),
            payment_provider: Some("alipay".to_string()),
            order_kind: WALLET_RECHARGE_ORDER_KIND.to_string(),
            gateway_order_id: Some("po_idem_placeholder".to_string()),
            gateway_response: Some(json!({
                "gateway": "alipay",
                "payment_channel": "alipay",
                "integration_status": "checkout_pending",
            })),
            status: "pending".to_string(),
            created_at_unix_ms: 0,
            paid_at_unix_secs: None,
            credited_at_unix_secs: None,
            expires_at_unix_secs: Some(u64::MAX),
        };

        assert!(!wallet_recharge_order_matches_effective_request(
            &order, "alipay", "alipay", 11.0, 79.75, "CNY", 7.25,
        ));
        assert!(wallet_recharge_order_matches_effective_request(
            &order, "alipay", "alipay", 10.0, 72.5, "CNY", 7.25,
        ));
        assert!(!wallet_recharge_order_matches_effective_request(
            &order, "alipay", "wxpay", 10.0, 72.5, "CNY", 7.25,
        ));
        assert!(!wallet_recharge_order_matches_effective_request(
            &order, "alipay", "alipay", 10.0, 72.51, "CNY", 7.25,
        ));
        assert!(!wallet_recharge_order_matches_effective_request(
            &order, "alipay", "alipay", 10.0, 72.5, "USD", 7.25,
        ));
        assert!(!wallet_recharge_order_matches_effective_request(
            &order, "alipay", "alipay", 10.0, 72.5, "CNY", 7.26,
        ));
    }

    #[test]
    fn effective_matcher_still_runs_when_existing_order_has_checkout_evidence() {
        let mut order = StoredAdminPaymentOrder {
            id: "checkout-order".to_string(),
            order_no: "po_idem_checkout".to_string(),
            wallet_id: "wallet-1".to_string(),
            user_id: Some("user-1".to_string()),
            amount_usd: 10.0,
            pay_amount: Some(72.5),
            pay_currency: Some("CNY".to_string()),
            exchange_rate: Some(7.25),
            refunded_amount_usd: 0.0,
            refundable_amount_usd: 0.0,
            payment_method: "alipay".to_string(),
            payment_provider: Some("alipay".to_string()),
            order_kind: WALLET_RECHARGE_ORDER_KIND.to_string(),
            gateway_order_id: Some("gw-1".to_string()),
            gateway_response: Some(json!({
                "gateway": "alipay",
                "payment_channel": "alipay",
                "payment_url": "https://pay.example.test/gw-1"
            })),
            status: "pending".to_string(),
            created_at_unix_ms: 1_700_000_000_000,
            paid_at_unix_secs: None,
            credited_at_unix_secs: None,
            expires_at_unix_secs: Some(u64::MAX),
        };

        assert!(wallet_recharge_order_matches_effective_request(
            &order, "alipay", "alipay", 10.0, 72.5, "CNY", 7.25,
        ));
        assert!(!wallet_recharge_order_matches_effective_request(
            &order, "alipay", "alipay", 11.0, 79.75, "CNY", 7.25,
        ));
    }

    #[test]
    fn effective_matcher_accepts_legacy_epay_channel_alias() {
        let order = StoredAdminPaymentOrder {
            id: "legacy-epay-order".to_string(),
            order_no: "po_legacy_epay".to_string(),
            wallet_id: "wallet-legacy-epay".to_string(),
            user_id: Some("user-legacy-epay".to_string()),
            amount_usd: 10.0,
            pay_amount: Some(72.5),
            pay_currency: Some("CNY".to_string()),
            exchange_rate: Some(7.25),
            refunded_amount_usd: 0.0,
            refundable_amount_usd: 0.0,
            // Older EPay rows stored the selected channel in payment_method.
            payment_method: "alipay".to_string(),
            payment_provider: Some("epay".to_string()),
            order_kind: WALLET_RECHARGE_ORDER_KIND.to_string(),
            gateway_order_id: Some("po_legacy_epay".to_string()),
            gateway_response: Some(json!({
                "gateway": "epay",
                "integration_status": "checkout_pending",
            })),
            status: "pending".to_string(),
            created_at_unix_ms: 0,
            paid_at_unix_secs: None,
            credited_at_unix_secs: None,
            expires_at_unix_secs: Some(u64::MAX),
        };

        assert!(wallet_recharge_order_matches_effective_request(
            &order, "epay", "alipay", 10.0, 72.5, "CNY", 7.25,
        ));
        assert!(!wallet_recharge_order_matches_effective_request(
            &order, "epay", "wxpay", 10.0, 72.5, "CNY", 7.25,
        ));
        assert!(!wallet_recharge_order_matches_effective_request(
            &order, "alipay", "alipay", 10.0, 72.5, "CNY", 7.25,
        ));
    }

    #[tokio::test]
    async fn replay_instructions_are_withheld_after_checkout_race_credits_order() {
        let state = AppState::new().expect("test app state");
        let mut order = StoredAdminPaymentOrder {
            id: "credited-checkout-order".to_string(),
            order_no: "po_credited_checkout".to_string(),
            wallet_id: "wallet-1".to_string(),
            user_id: Some("user-1".to_string()),
            amount_usd: 10.0,
            pay_amount: Some(72.5),
            pay_currency: Some("CNY".to_string()),
            exchange_rate: Some(7.25),
            refunded_amount_usd: 0.0,
            refundable_amount_usd: 0.0,
            payment_method: "alipay".to_string(),
            payment_provider: Some("alipay".to_string()),
            order_kind: WALLET_RECHARGE_ORDER_KIND.to_string(),
            gateway_order_id: Some("gw-1".to_string()),
            gateway_response: Some(json!({
                "gateway": "alipay",
                "payment_channel": "alipay",
                "payment_url": "https://pay.example.test/gw-1"
            })),
            status: "credited".to_string(),
            created_at_unix_ms: 1_700_000_000_000,
            paid_at_unix_secs: Some(1_700_000_010),
            credited_at_unix_secs: Some(1_700_000_010),
            expires_at_unix_secs: Some(u64::MAX),
        };

        assert!(wallet_recharge_order_is_settled(&order));
        assert_eq!(
            super::wallet_recharge_replay_payment_instructions(&state, &order).await,
            json!({}),
        );

        order.status = "paid".to_string();
        assert!(wallet_recharge_order_is_settled(&order));
        order.status = "pending".to_string();
        assert!(!wallet_recharge_order_is_settled(&order));
        order.status = "failed".to_string();
        assert!(!wallet_recharge_order_is_settled(&order));
    }

    #[test]
    fn test_recharge_replay_hides_expired_or_non_pending_instructions() {
        let expired = json!({
            "status": "pending",
            "expires_at": "2000-01-01T00:00:00Z",
            "gateway_response": {
                "gateway": "alipay",
                "payment_url": "https://pay.example.test/expired"
            }
        });
        assert_eq!(
            wallet_test_recharge_replay_payment_instructions(&expired),
            json!({})
        );

        let paid = json!({
            "status": "credited",
            "expires_at": "2999-01-01T00:00:00Z",
            "gateway_response": {
                "gateway": "alipay",
                "payment_url": "https://pay.example.test/paid"
            }
        });
        assert_eq!(
            wallet_test_recharge_replay_payment_instructions(&paid),
            json!({})
        );
    }

    #[test]
    fn test_recharge_idempotency_matches_all_client_amount_fields() {
        let order = json!({
            "amount_usd": 10.0,
            "payment_method": "alipay",
            "payment_provider": "alipay",
            "payment_channel": "alipay",
            "pay_amount": 72.5,
            "pay_currency": "CNY",
            "exchange_rate": 7.25,
            "gateway_response": {
                "gateway": "alipay",
                "payment_channel": "alipay"
            }
        });
        let payload = NormalizedWalletCreateRechargeRequest {
            amount_usd: 10.0,
            payment_method: "alipay".to_string(),
            payment_provider: Some("alipay".to_string()),
            payment_channel: Some("alipay".to_string()),
            pay_amount: Some(72.5),
            pay_currency: Some("CNY".to_string()),
            exchange_rate: Some(7.25),
            idempotency_key: Some("recharge-match".to_string()),
        };
        assert!(wallet_test_recharge_payload_matches_request(
            &order, &payload
        ));

        let mut changed = payload.clone();
        changed.pay_amount = Some(72.51);
        assert!(!wallet_test_recharge_payload_matches_request(
            &order, &changed
        ));
        changed = payload.clone();
        changed.pay_currency = Some("USD".to_string());
        assert!(!wallet_test_recharge_payload_matches_request(
            &order, &changed
        ));
        changed = payload;
        changed.exchange_rate = Some(7.26);
        assert!(!wallet_test_recharge_payload_matches_request(
            &order, &changed
        ));
    }

    #[test]
    fn epay_idempotency_includes_legacy_payment_method_channel_shorthand() {
        let order = StoredAdminPaymentOrder {
            id: "epay-idempotent-order".to_string(),
            order_no: "po_idem_epay".to_string(),
            wallet_id: "wallet-epay".to_string(),
            user_id: Some("user-epay".to_string()),
            amount_usd: 10.0,
            pay_amount: Some(72.5),
            pay_currency: Some("CNY".to_string()),
            exchange_rate: Some(7.25),
            refunded_amount_usd: 0.0,
            refundable_amount_usd: 0.0,
            payment_method: "epay".to_string(),
            payment_provider: Some("epay".to_string()),
            order_kind: WALLET_RECHARGE_ORDER_KIND.to_string(),
            gateway_order_id: Some("po_idem_epay".to_string()),
            gateway_response: Some(json!({
                "gateway": "epay",
                "payment_channel": "alipay",
                "integration_status": "checkout_pending"
            })),
            status: "pending".to_string(),
            created_at_unix_ms: 0,
            paid_at_unix_secs: None,
            credited_at_unix_secs: None,
            expires_at_unix_secs: Some(u64::MAX),
        };
        let payload = |payment_method: &str, payment_channel: Option<&str>| {
            NormalizedWalletCreateRechargeRequest {
                amount_usd: 10.0,
                payment_method: payment_method.to_string(),
                payment_provider: Some("epay".to_string()),
                payment_channel: payment_channel.map(str::to_string),
                pay_amount: Some(72.5),
                pay_currency: Some("CNY".to_string()),
                exchange_rate: Some(7.25),
                idempotency_key: Some("epay-channel-regression".to_string()),
            }
        };

        assert!(wallet_recharge_order_matches_request(
            &order,
            &payload("alipay", None),
        ));
        assert!(!wallet_recharge_order_matches_request(
            &order,
            &payload("wxpay", None),
        ));
        assert!(!wallet_recharge_order_matches_request(
            &order,
            &payload("alipay", Some("wxpay")),
        ));

        let legacy_order = StoredAdminPaymentOrder {
            payment_method: "alipay".to_string(),
            gateway_response: Some(json!({
                "gateway": "epay",
                "integration_status": "checkout_pending"
            })),
            ..order
        };
        assert!(wallet_recharge_order_matches_request(
            &legacy_order,
            &payload("alipay", None),
        ));
        assert!(!wallet_recharge_order_matches_request(
            &legacy_order,
            &payload("wxpay", None),
        ));

        let mut direct_provider_payload = payload("alipay", None);
        direct_provider_payload.payment_provider = Some("alipay".to_string());
        assert!(!wallet_recharge_order_matches_request(
            &legacy_order,
            &direct_provider_payload,
        ));
    }

    #[test]
    fn test_epay_idempotency_includes_legacy_payment_method_channel_shorthand() {
        let order = json!({
            "amount_usd": 10.0,
            "payment_method": "epay",
            "payment_provider": "epay",
            "pay_amount": 72.5,
            "pay_currency": "CNY",
            "exchange_rate": 7.25,
            "gateway_response": {
                "gateway": "epay",
                "payment_channel": "alipay"
            }
        });
        let payload = |payment_method: &str| NormalizedWalletCreateRechargeRequest {
            amount_usd: 10.0,
            payment_method: payment_method.to_string(),
            payment_provider: Some("epay".to_string()),
            payment_channel: None,
            pay_amount: Some(72.5),
            pay_currency: Some("CNY".to_string()),
            exchange_rate: Some(7.25),
            idempotency_key: Some("epay-channel-regression-json".to_string()),
        };
        assert!(wallet_test_recharge_payload_matches_request(
            &order,
            &payload("alipay")
        ));
        assert!(!wallet_test_recharge_payload_matches_request(
            &order,
            &payload("wxpay")
        ));

        let legacy_order = json!({
            "amount_usd": 10.0,
            "payment_method": "alipay",
            "payment_provider": "epay",
            "pay_amount": 72.5,
            "pay_currency": "CNY",
            "exchange_rate": 7.25,
            "gateway_response": {
                "gateway": "epay"
            }
        });
        assert!(wallet_test_recharge_payload_matches_request(
            &legacy_order,
            &payload("alipay")
        ));
        assert!(!wallet_test_recharge_payload_matches_request(
            &legacy_order,
            &payload("wxpay")
        ));
    }

    #[test]
    fn direct_gateway_channels_normalize_configured_channel_case() {
        let record = aether_data_contracts::repository::billing::PaymentGatewayConfigRecord {
            provider: "wxpay".to_string(),
            enabled: true,
            endpoint_url: "https://pay.example.test".to_string(),
            callback_base_url: Some("https://app.example.test".to_string()),
            merchant_id: "merchant".to_string(),
            merchant_key_encrypted: Some("encrypted".to_string()),
            pay_currency: "CNY".to_string(),
            usd_exchange_rate: 7.0,
            min_recharge_usd: 1.0,
            channels_json: json!([
                {"channel": "NATIVE", "display_name": "Native", "fee_rate": 0.0},
                {"channel": "H5", "display_name": "H5", "fee_rate": 0.0},
                {"channel": "APP", "display_name": "Unsupported", "fee_rate": 0.0}
            ]),
            created_at_unix_secs: 0,
            updated_at_unix_secs: 0,
        };

        let channels = super::direct_gateway_channels("WXPAY", &record);
        assert_eq!(
            channels
                .iter()
                .map(|channel| channel.channel.as_str())
                .collect::<Vec<_>>(),
            vec!["native", "h5"]
        );
        let selected = super::resolve_direct_gateway_channel("wxpay", &record, Some("NATIVE"))
            .expect("configured channel should resolve case-insensitively");
        assert_eq!(selected.channel, "native");
    }

    #[test]
    fn recharge_payment_breakdown_rejects_non_finite_inputs_and_overflow() {
        assert!(wallet_recharge_payment_breakdown(10.0, "CNY", f64::NAN, 0.0).is_err());
        assert!(wallet_recharge_payment_breakdown(10.0, "CNY", 7.0, f64::INFINITY).is_err());
        assert!(wallet_recharge_payment_breakdown(f64::MAX, "CNY", f64::MAX, 0.0).is_err());
        let breakdown = wallet_recharge_payment_breakdown(10.0, "CNY", 7.0, 2.5)
            .expect("valid recharge amounts");
        assert!(breakdown.base_pay_amount.is_finite());
        assert!(breakdown.fee_amount.is_finite());
        assert!(breakdown.pay_amount.is_finite());
        assert_eq!(
            breakdown,
            WalletRechargePaymentBreakdown {
                base_pay_amount: 70.0,
                fee_amount: 1.75,
                pay_amount: 71.75,
                exchange_rate: 7.0,
            }
        );
    }

    #[test]
    fn usd_recharge_uses_unit_exchange_rate_for_amount_order_and_fee() {
        let breakdown = wallet_recharge_payment_breakdown(10.0, "USD", 7.2, 2.5)
            .expect("USD recharge should use canonical rate");
        assert_eq!(breakdown.exchange_rate, 1.0);
        assert_eq!(
            (
                breakdown.base_pay_amount,
                breakdown.fee_amount,
                breakdown.pay_amount
            ),
            (10.0, 0.25, 10.25)
        );
    }
}
