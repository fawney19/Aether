use aether_crypto::looks_like_python_fernet_ciphertext;

use crate::AppState;

use super::{
    decrypt_catalog_secret_with_fallbacks, open_runtime_secret_payload, seal_runtime_secret_payload,
};

pub(crate) const STRIPE_CLIENT_SECRET_ENCRYPTED_KEY: &str = "_stripe_client_secret_encrypted";
const MAX_STRIPE_CLIENT_SECRET_BYTES: usize = 1024;
const PAYMENT_ORDER_STRIPE_SECRET_ENVELOPE_FAMILY: &str =
    "aether-payment-order-stripe-client-secret-";
const PAYMENT_ORDER_STRIPE_SECRET_ENVELOPE_V2: &str =
    "aether-payment-order-stripe-client-secret-v2:";
const PAYMENT_ORDER_STRIPE_SECRET_PURPOSE_V2: &str = "payment-order-stripe-client-secret-bound-v2";
const AETHER_ENVELOPE_FAMILY: &str = "aether-";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PaymentOrderStripeSecretBinding {
    pub(crate) order_no: String,
    pub(crate) user_id: Option<String>,
    pub(crate) order_kind: String,
    pub(crate) payment_provider: String,
}

impl PaymentOrderStripeSecretBinding {
    pub(crate) fn new(
        order_no: &str,
        user_id: Option<&str>,
        order_kind: &str,
        payment_provider: &str,
    ) -> Result<Self, &'static str> {
        validate_identity_component(order_no, "payment order number", 128)?;
        if let Some(user_id) = user_id {
            validate_identity_component(user_id, "payment order user ID", 128)?;
        }
        let order_kind = order_kind.trim().to_ascii_lowercase();
        if !matches!(order_kind.as_str(), "wallet_recharge" | "plan_purchase") {
            return Err("payment order kind is not eligible for a Stripe client secret");
        }
        let payment_provider = payment_provider.trim().to_ascii_lowercase();
        if payment_provider != "stripe" {
            return Err("payment order provider is not Stripe");
        }
        Ok(Self {
            order_no: order_no.to_string(),
            user_id: user_id.map(ToOwned::to_owned),
            order_kind,
            payment_provider,
        })
    }

    pub(crate) fn from_order(
        order: &aether_data::repository::wallet::StoredAdminPaymentOrder,
    ) -> Result<Self, &'static str> {
        Self::new(
            &order.order_no,
            order.user_id.as_deref(),
            &order.order_kind,
            order
                .payment_provider
                .as_deref()
                .unwrap_or(order.payment_method.as_str()),
        )
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct PaymentOrderStripeSecretProjection {
    pub(crate) plaintext: String,
    pub(crate) protected: String,
    pub(crate) migration_required: bool,
}

fn validate_identity_component(
    value: &str,
    _label: &'static str,
    max_bytes: usize,
) -> Result<(), &'static str> {
    if value.is_empty() {
        return Err("payment order secret binding contains an empty identity component");
    }
    if value.as_bytes().len() > max_bytes {
        return Err("payment order secret binding identity component is too long");
    }
    if value.chars().any(char::is_control) {
        return Err("payment order secret binding identity component contains control characters");
    }
    Ok(())
}

fn payment_order_stripe_secret_purpose(
    binding: &PaymentOrderStripeSecretBinding,
) -> Result<String, &'static str> {
    let user_binding = match binding.user_id.as_deref() {
        Some(user_id) => format!(
            "user-id-present=1\0user-id-bytes={}\0{user_id}",
            user_id.len()
        ),
        None => "user-id-present=0".to_string(),
    };
    Ok(format!(
        "{PAYMENT_ORDER_STRIPE_SECRET_PURPOSE_V2}\0provider-bytes={}\0{}\0order-no-bytes={}\0{}\0{}\0order-kind-bytes={}\0{}\0field-bytes={}\0{}",
        binding.payment_provider.len(),
        binding.payment_provider,
        binding.order_no.len(),
        binding.order_no,
        user_binding,
        binding.order_kind.len(),
        binding.order_kind,
        STRIPE_CLIENT_SECRET_ENCRYPTED_KEY.len(),
        STRIPE_CLIENT_SECRET_ENCRYPTED_KEY,
    ))
}

pub(crate) fn normalize_stripe_client_secret(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()
        && value.len() <= MAX_STRIPE_CLIENT_SECRET_BYTES
        && value.starts_with("pi_")
        && value.contains("_secret_")
        && !value.chars().any(char::is_control))
    .then_some(value)
}

pub(crate) fn seal_payment_order_stripe_client_secret(
    state: &AppState,
    binding: &PaymentOrderStripeSecretBinding,
    plaintext: &str,
) -> Result<String, &'static str> {
    let plaintext = normalize_stripe_client_secret(plaintext)
        .ok_or("Stripe client secret format is invalid")?;
    let purpose = payment_order_stripe_secret_purpose(binding)?;
    let sealed = seal_runtime_secret_payload(state, &purpose, plaintext)
        .ok_or("payment order Stripe client secret encryption key is not configured")?;
    Ok(format!("{PAYMENT_ORDER_STRIPE_SECRET_ENVELOPE_V2}{sealed}"))
}

pub(crate) fn open_payment_order_stripe_client_secret(
    state: &AppState,
    binding: &PaymentOrderStripeSecretBinding,
    stored: &str,
) -> Result<PaymentOrderStripeSecretProjection, &'static str> {
    let purpose = payment_order_stripe_secret_purpose(binding)?;
    let observed = stored;
    let stored = observed.trim();
    if stored.is_empty() {
        return Err("payment order Stripe client secret ciphertext is empty");
    }

    let (plaintext, protected, migration_required) = if let Some(sealed) =
        stored.strip_prefix(PAYMENT_ORDER_STRIPE_SECRET_ENVELOPE_V2)
    {
        let plaintext = open_runtime_secret_payload(state, &purpose, sealed)
            .ok_or("payment order Stripe client secret authentication failed")?;
        (
            plaintext,
            stored.to_string(),
            observed.as_bytes() != stored.as_bytes(),
        )
    } else {
        if stored.starts_with(PAYMENT_ORDER_STRIPE_SECRET_ENVELOPE_FAMILY) {
            return Err("unsupported payment order Stripe client secret envelope");
        }
        if stored.starts_with(AETHER_ENVELOPE_FAMILY) {
            return Err("secret envelope has the wrong payment order binding");
        }
        if !looks_like_python_fernet_ciphertext(stored) {
            return Err("payment order Stripe client secret is not an authenticated ciphertext");
        }
        let plaintext = decrypt_catalog_secret_with_fallbacks(state.encryption_key(), stored)
            .ok_or("legacy payment order Stripe client secret authentication failed")?;
        if plaintext.contains('\0') {
            return Err("legacy payment order Stripe client secret contains reserved framing");
        }
        let protected = seal_payment_order_stripe_client_secret(state, binding, &plaintext)?;
        (plaintext, protected, true)
    };

    let plaintext = normalize_stripe_client_secret(&plaintext)
        .ok_or("Stripe client secret plaintext format is invalid")?
        .to_string();
    Ok(PaymentOrderStripeSecretProjection {
        plaintext,
        protected,
        migration_required,
    })
}

#[cfg(test)]
mod tests {
    use aether_crypto::DEVELOPMENT_ENCRYPTION_KEY;

    use super::{
        open_payment_order_stripe_client_secret, seal_payment_order_stripe_client_secret,
        PaymentOrderStripeSecretBinding,
    };
    use crate::handlers::shared::{
        encrypt_catalog_secret_with_fallbacks, seal_runtime_secret_payload,
    };
    use crate::{data::GatewayDataState, AppState};

    fn state_with_encryption_key() -> AppState {
        AppState::new()
            .expect("test state should build")
            .with_data_state_for_tests(
                GatewayDataState::disabled()
                    .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            )
    }

    fn binding(order_no: &str, user_id: &str, order_kind: &str) -> PaymentOrderStripeSecretBinding {
        PaymentOrderStripeSecretBinding::new(order_no, Some(user_id), order_kind, "stripe")
            .expect("test binding should be valid")
    }

    #[test]
    fn v2_ciphertext_is_bound_to_order_owner_kind_provider_and_field() {
        let state = state_with_encryption_key();
        let source = binding("po-source", "user-a", "wallet_recharge");
        let sealed =
            seal_payment_order_stripe_client_secret(&state, &source, "pi_source_secret_capability")
                .expect("client secret should seal");

        let opened = open_payment_order_stripe_client_secret(&state, &source, &sealed)
            .expect("matching order should open");
        assert_eq!(opened.plaintext, "pi_source_secret_capability");
        assert!(!opened.migration_required);
        for foreign in [
            binding("po-foreign", "user-a", "wallet_recharge"),
            binding("po-source", "user-b", "wallet_recharge"),
            binding("po-source", "user-a", "plan_purchase"),
        ] {
            assert!(open_payment_order_stripe_client_secret(&state, &foreign, &sealed).is_err());
        }
        assert!(PaymentOrderStripeSecretBinding::new(
            "po-source",
            Some("user-a"),
            "wallet_recharge",
            "alipay",
        )
        .is_err());
    }

    #[test]
    fn reader_migrates_only_real_legacy_fernet_and_rejects_other_envelopes() {
        let state = state_with_encryption_key();
        let binding = binding("po-source", "user-a", "wallet_recharge");
        let legacy = encrypt_catalog_secret_with_fallbacks(&state, "pi_legacy_secret_capability")
            .expect("legacy secret should encrypt");
        let opened = open_payment_order_stripe_client_secret(&state, &binding, &legacy)
            .expect("real legacy Fernet should open");
        assert!(opened.migration_required);
        assert!(opened
            .protected
            .starts_with("aether-payment-order-stripe-client-secret-v2:"));

        for stored in [
            "plaintext-secret",
            "aether-payment-order-stripe-client-secret-v3:unknown",
            "aether-payment-gateway-secret-v2:foreign",
        ] {
            assert!(open_payment_order_stripe_client_secret(&state, &binding, stored).is_err());
        }
        let foreign_runtime =
            seal_runtime_secret_payload(&state, "another-purpose", "pi_x_secret_y")
                .expect("runtime secret should seal");
        assert!(
            open_payment_order_stripe_client_secret(&state, &binding, &foreign_runtime,).is_err()
        );

        let invalid_legacy = encrypt_catalog_secret_with_fallbacks(&state, "not-a-stripe-secret")
            .expect("legacy value should encrypt");
        assert!(
            open_payment_order_stripe_client_secret(&state, &binding, &invalid_legacy,).is_err()
        );
    }
}
