use aether_crypto::looks_like_python_fernet_ciphertext;

use crate::AppState;

use super::{
    decrypt_catalog_secret_with_fallbacks, open_runtime_secret_payload, seal_runtime_secret_payload,
};

const PAYMENT_GATEWAY_SECRET_ENVELOPE_FAMILY: &str = "aether-payment-gateway-secret-";
const PAYMENT_GATEWAY_SECRET_ENVELOPE_V2: &str = "aether-payment-gateway-secret-v2:";
const PAYMENT_GATEWAY_SECRET_ENVELOPE_V3: &str = "aether-payment-gateway-secret-v3:";
const PAYMENT_GATEWAY_SECRET_PURPOSE_V2: &str = "payment-gateway-secret-bound-v2";
const PAYMENT_GATEWAY_SECRET_PURPOSE_V3: &str = "payment-gateway-secret-bound-v3";
const RUNTIME_SECRET_ENVELOPE_FAMILY: &str = "aether-runtime-secret-";

const ALIPAY_DEFAULT_GATEWAY_URL: &str = "https://openapi.alipay.com/gateway.do";
const WXPAY_DEFAULT_BASE_URL: &str = "https://api.mch.weixin.qq.com";
const STRIPE_DEFAULT_API_URL: &str = "https://api.stripe.com";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PaymentGatewaySecretBinding {
    pub(crate) provider: String,
    pub(crate) endpoint_url: String,
    pub(crate) merchant_id: String,
}

impl PaymentGatewaySecretBinding {
    pub(crate) fn new(
        provider: &str,
        endpoint_url: &str,
        merchant_id: &str,
    ) -> Result<Self, &'static str> {
        let provider = provider.trim().to_ascii_lowercase();
        if provider.is_empty() || provider.contains('\0') || provider.chars().any(char::is_control)
        {
            return Err("payment gateway secret provider is invalid");
        }
        let endpoint_url = canonical_payment_gateway_endpoint(&provider, endpoint_url)?;
        let merchant_id = merchant_id.trim().to_string();
        if merchant_id.chars().any(char::is_control) {
            return Err("payment gateway secret merchant_id contains reserved framing");
        }
        if merchant_id.len() > 256 {
            return Err("payment gateway secret merchant_id is too long");
        }
        Ok(Self {
            provider,
            endpoint_url,
            merchant_id,
        })
    }

    pub(crate) fn from_record(
        record: &aether_data_contracts::repository::billing::PaymentGatewayConfigRecord,
    ) -> Result<Self, &'static str> {
        Self::new(&record.provider, &record.endpoint_url, &record.merchant_id)
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct PaymentGatewaySecretProjection {
    pub(crate) plaintext: String,
    pub(crate) protected: String,
    pub(crate) migration_required: bool,
}

/// Returns whether a stored gateway secret predates destination binding.
///
/// Legacy Fernet values carry no gateway identity at all, while the v2
/// envelope authenticates only the provider.  Neither format can prove that
/// a value belongs to a newly supplied endpoint/merchant pair, so callers
/// performing a destination-changing mutation must require an explicit
/// replacement secret instead of silently reusing it.
pub(crate) fn payment_gateway_secret_is_legacy_unbound(stored: &str) -> bool {
    let stored = stored.trim();
    if stored.is_empty() {
        return false;
    }
    if stored.starts_with(PAYMENT_GATEWAY_SECRET_ENVELOPE_V2) {
        return true;
    }
    if stored.starts_with(PAYMENT_GATEWAY_SECRET_ENVELOPE_FAMILY)
        || stored.starts_with(RUNTIME_SECRET_ENVELOPE_FAMILY)
        || stored.starts_with("aether-")
    {
        return false;
    }
    looks_like_python_fernet_ciphertext(stored)
}

fn payment_gateway_secret_purpose(provider: &str) -> Result<String, &'static str> {
    let provider = provider.trim().to_ascii_lowercase();
    if provider.is_empty() {
        return Err("payment gateway secret provider is empty");
    }
    Ok(format!(
        "{PAYMENT_GATEWAY_SECRET_PURPOSE_V2}\0provider-bytes={}\0{provider}\0field=merchant-key",
        provider.len()
    ))
}

fn payment_gateway_secret_purpose_v3(
    binding: &PaymentGatewaySecretBinding,
) -> Result<String, &'static str> {
    for value in [
        binding.provider.as_str(),
        binding.endpoint_url.as_str(),
        binding.merchant_id.as_str(),
    ] {
        if value.contains('\0') {
            return Err("payment gateway secret binding contains reserved framing");
        }
    }
    Ok(format!(
        "{PAYMENT_GATEWAY_SECRET_PURPOSE_V3}\0provider-bytes={}\0{}\0endpoint-url-bytes={}\0{}\0merchant-id-bytes={}\0{}\0field=merchant-key",
        binding.provider.len(),
        binding.provider,
        binding.endpoint_url.len(),
        binding.endpoint_url,
        binding.merchant_id.len(),
        binding.merchant_id,
    ))
}

fn canonical_payment_gateway_endpoint(
    provider: &str,
    endpoint_url: &str,
) -> Result<String, &'static str> {
    let endpoint_url = endpoint_url.trim();
    let endpoint_url = if endpoint_url.is_empty() {
        match provider {
            "alipay" => ALIPAY_DEFAULT_GATEWAY_URL,
            "wxpay" => WXPAY_DEFAULT_BASE_URL,
            "stripe" => STRIPE_DEFAULT_API_URL,
            // EPay requires an explicit endpoint at checkout time. Keep an
            // explicit marker for legacy records so their secret remains
            // bound to the empty value instead of silently changing scope.
            _ => return Ok("<empty>".to_string()),
        }
    } else {
        endpoint_url
    };
    let endpoint_url = super::normalize_payment_https_url(endpoint_url, "endpoint_url")
        .map_err(|_| "payment gateway secret endpoint_url is invalid")?;
    let mut parsed = url::Url::parse(&endpoint_url)
        .map_err(|_| "payment gateway secret endpoint_url is invalid")?;
    if parsed.scheme() != "https"
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err("payment gateway secret endpoint_url must be an HTTPS URL without credentials or a fragment");
    }
    if let Some(host) = parsed.host_str() {
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        if host.is_empty() {
            return Err("payment gateway secret endpoint_url host is empty");
        }
        parsed
            .set_host(Some(&host))
            .map_err(|_| "payment gateway secret endpoint_url host is invalid")?;
    }
    if parsed.port() == Some(443) {
        parsed
            .set_port(None)
            .map_err(|_| "payment gateway secret endpoint_url port is invalid")?;
    }
    let canonical = parsed.to_string().trim_end_matches('/').to_string();
    // Stripe requests are intentionally sent to the official API origin in
    // the checkout/refund implementations below.  Accepting a configurable
    // destination here would bind the credential to one host while sending
    // it to another, which defeats the purpose of destination binding and
    // could silently route a live secret through an unintended proxy.
    if provider == "stripe" && canonical != STRIPE_DEFAULT_API_URL {
        return Err("Stripe endpoint_url must use the official API endpoint");
    }
    Ok(canonical)
}

pub(crate) fn seal_payment_gateway_secret(
    state: &AppState,
    binding: &PaymentGatewaySecretBinding,
    plaintext: &str,
) -> Result<String, &'static str> {
    if plaintext.contains('\0') {
        return Err("payment gateway secret contains reserved framing");
    }
    let purpose = payment_gateway_secret_purpose_v3(binding)?;
    let sealed = seal_runtime_secret_payload(state, &purpose, plaintext)
        .ok_or("payment gateway secret encryption key is not configured")?;
    Ok(format!("{PAYMENT_GATEWAY_SECRET_ENVELOPE_V3}{sealed}"))
}

pub(crate) fn open_payment_gateway_secret(
    state: &AppState,
    binding: &PaymentGatewaySecretBinding,
    stored: &str,
) -> Result<PaymentGatewaySecretProjection, &'static str> {
    let purpose = payment_gateway_secret_purpose_v3(binding)?;
    if let Some(sealed) = stored.strip_prefix(PAYMENT_GATEWAY_SECRET_ENVELOPE_V3) {
        let plaintext = open_runtime_secret_payload(state, &purpose, sealed)
            .ok_or("payment gateway secret authentication or binding failed")?;
        if plaintext.contains('\0') {
            return Err("payment gateway secret contains reserved framing");
        }
        return Ok(PaymentGatewaySecretProjection {
            plaintext,
            protected: stored.to_string(),
            migration_required: false,
        });
    }
    if let Some(sealed) = stored.strip_prefix(PAYMENT_GATEWAY_SECRET_ENVELOPE_V2) {
        // v2 was bound only to provider. Authenticate it with the historical
        // purpose, then immediately re-seal under the complete destination
        // binding before returning the plaintext to a caller.
        let plaintext = open_runtime_secret_payload(
            state,
            &payment_gateway_secret_purpose(&binding.provider)?,
            sealed,
        )
        .ok_or("legacy payment gateway secret authentication failed")?;
        if plaintext.contains('\0') {
            return Err("legacy payment gateway secret contains reserved framing");
        }
        let protected = seal_payment_gateway_secret(state, binding, &plaintext)?;
        return Ok(PaymentGatewaySecretProjection {
            plaintext,
            protected,
            migration_required: true,
        });
    }
    if stored.starts_with(PAYMENT_GATEWAY_SECRET_ENVELOPE_FAMILY) {
        return Err("unsupported payment gateway secret envelope");
    }
    if stored.starts_with(RUNTIME_SECRET_ENVELOPE_FAMILY) {
        return Err("runtime secret envelope has the wrong purpose");
    }
    if !looks_like_python_fernet_ciphertext(stored) {
        return Err("payment gateway secret is not an authenticated ciphertext");
    }

    let plaintext = decrypt_catalog_secret_with_fallbacks(state.encryption_key(), stored)
        .ok_or("legacy payment gateway secret authentication failed")?;
    if plaintext.contains('\0') {
        return Err("legacy payment gateway secret contains reserved framing");
    }
    let protected = seal_payment_gateway_secret(state, binding, &plaintext)?;
    Ok(PaymentGatewaySecretProjection {
        plaintext,
        protected,
        migration_required: true,
    })
}

#[cfg(test)]
mod tests {
    use aether_crypto::DEVELOPMENT_ENCRYPTION_KEY;

    use super::{
        open_payment_gateway_secret, seal_payment_gateway_secret, PaymentGatewaySecretBinding,
        PAYMENT_GATEWAY_SECRET_ENVELOPE_V2, PAYMENT_GATEWAY_SECRET_ENVELOPE_V3,
        STRIPE_DEFAULT_API_URL,
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

    #[test]
    fn v3_round_trip_binds_destination_and_rejects_tampering() {
        let state = state_with_encryption_key();
        let binding = PaymentGatewaySecretBinding::new(
            " EPay ",
            "https://payments.example.test:443/checkout/",
            " merchant-1 ",
        )
        .expect("payment gateway binding should build");
        let sealed = seal_payment_gateway_secret(&state, &binding, "secret-value")
            .expect("payment gateway secret should seal");

        let opened = open_payment_gateway_secret(&state, &binding, &sealed)
            .expect("payment gateway secret should open");
        assert_eq!(opened.plaintext, "secret-value");
        assert!(!opened.migration_required);
        assert!(open_payment_gateway_secret(
            &state,
            &PaymentGatewaySecretBinding::new(
                "epay",
                "https://payments.example.test/other",
                "merchant-1",
            )
            .unwrap(),
            &sealed,
        )
        .is_err());
        assert!(open_payment_gateway_secret(
            &state,
            &PaymentGatewaySecretBinding::new(
                "epay",
                "https://payments.example.test/checkout/",
                "merchant-2",
            )
            .unwrap(),
            &sealed,
        )
        .is_err());
        assert!(open_payment_gateway_secret(
            &state,
            &PaymentGatewaySecretBinding::new(
                "alipay",
                "https://payments.example.test/checkout/",
                "merchant-1",
            )
            .unwrap(),
            &sealed,
        )
        .is_err());

        let stripped = sealed
            .strip_prefix(PAYMENT_GATEWAY_SECRET_ENVELOPE_V3)
            .and_then(|value| value.strip_prefix("aether-runtime-secret-v1:"))
            .expect("test value should contain both envelope layers");
        assert!(open_payment_gateway_secret(&state, &binding, stripped).is_err());

        let mut tampered = sealed.into_bytes();
        let last = tampered
            .last_mut()
            .expect("sealed value should not be empty");
        *last = if *last == b'A' { b'B' } else { b'A' };
        let tampered = String::from_utf8(tampered).expect("ciphertext should remain utf-8");
        assert!(open_payment_gateway_secret(&state, &binding, &tampered).is_err());
    }

    #[test]
    fn authenticated_legacy_values_migrate_to_v3_destination_binding() {
        let state = state_with_encryption_key();
        let binding = PaymentGatewaySecretBinding::new(
            "epay",
            "https://pay.example.test/submit.php",
            "merchant-1",
        )
        .unwrap();
        let legacy = encrypt_catalog_secret_with_fallbacks(&state, "legacy-secret")
            .expect("legacy secret should encrypt");
        let opened = open_payment_gateway_secret(&state, &binding, &legacy)
            .expect("legacy secret should migrate");
        assert_eq!(opened.plaintext, "legacy-secret");
        assert!(opened.migration_required);
        assert!(opened
            .protected
            .starts_with("aether-payment-gateway-secret-v3:"));

        let old_v2 = seal_runtime_secret_payload(
            &state,
            "payment-gateway-secret-bound-v2\0provider-bytes=4\0epay\0field=merchant-key",
            "v2-secret",
        )
        .expect("legacy v2 secret should encrypt");
        let old_v2 = format!("{PAYMENT_GATEWAY_SECRET_ENVELOPE_V2}{old_v2}");
        let migrated = open_payment_gateway_secret(&state, &binding, &old_v2)
            .expect("legacy v2 secret should migrate");
        assert_eq!(migrated.plaintext, "v2-secret");
        assert!(migrated.migration_required);
        assert!(migrated
            .protected
            .starts_with(PAYMENT_GATEWAY_SECRET_ENVELOPE_V3));

        assert!(open_payment_gateway_secret(
            &state,
            &binding,
            "aether-payment-gateway-secret-v4:unknown",
        )
        .is_err());
        assert!(open_payment_gateway_secret(&state, &binding, "plaintext-secret").is_err());

        let other_runtime = seal_runtime_secret_payload(&state, "another-purpose", "secret")
            .expect("runtime secret should seal");
        assert!(open_payment_gateway_secret(&state, &binding, &other_runtime).is_err());
    }

    #[test]
    fn canonical_binding_uses_provider_defaults_and_rejects_unsafe_urls() {
        let default_alipay = PaymentGatewaySecretBinding::new("ALIPAY", "", "merchant")
            .expect("default Alipay endpoint should be accepted");
        let explicit_alipay = PaymentGatewaySecretBinding::new(
            "alipay",
            "https://OPENAPI.ALIPAY.COM:443/gateway.do",
            "merchant",
        )
        .expect("explicit Alipay endpoint should be accepted");
        assert_eq!(default_alipay.endpoint_url, explicit_alipay.endpoint_url);
        let default_stripe = PaymentGatewaySecretBinding::new("stripe", "", "merchant")
            .expect("default Stripe endpoint should be accepted");
        let explicit_stripe =
            PaymentGatewaySecretBinding::new("stripe", "https://API.STRIPE.COM:443/", "merchant")
                .expect("official Stripe endpoint should be accepted");
        assert_eq!(default_stripe.endpoint_url, STRIPE_DEFAULT_API_URL);
        assert_eq!(default_stripe, explicit_stripe);
        assert!(PaymentGatewaySecretBinding::new(
            "stripe",
            "https://stripe-proxy.example.test",
            "merchant",
        )
        .is_err());
        for endpoint in [
            "http://payments.example.test",
            "https://user:password@payments.example.test",
            "https://127.0.0.1/pay",
            "https://payments.example.test/#fragment",
        ] {
            assert!(
                PaymentGatewaySecretBinding::new("stripe", endpoint, "merchant").is_err(),
                "unsafe endpoint should be rejected: {endpoint}"
            );
        }
    }
}
