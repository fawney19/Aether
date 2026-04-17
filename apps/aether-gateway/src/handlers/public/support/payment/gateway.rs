use chrono::{DateTime, Utc};
use serde_json::json;

use super::payment_shared::{
    normalize_payment_callback_request, payment_callback_signature_matches,
    NormalizedPaymentCallbackRequest, PaymentCallbackRequest,
};

pub(crate) struct CreateCheckoutSessionInput {
    pub(crate) order_no: String,
    pub(crate) amount_usd: f64,
    pub(crate) expires_at: DateTime<Utc>,
}

pub(crate) struct CreateCheckoutSessionOutput {
    pub(crate) gateway_order_id: String,
    pub(crate) gateway_response: serde_json::Value,
}

pub(crate) struct VerifyCallbackInput<'a> {
    pub(crate) secret: &'a str,
    pub(crate) signature: &'a str,
    pub(crate) payload: PaymentCallbackRequest,
}

pub(crate) struct VerifyCallbackOutcome {
    pub(crate) normalized_payload: NormalizedPaymentCallbackRequest,
    pub(crate) signature_valid: bool,
}

pub(crate) trait PaymentGatewayAdapter: Sync {
    fn payment_method(&self) -> &'static str;

    fn create_checkout_session(
        &self,
        input: &CreateCheckoutSessionInput,
    ) -> Result<CreateCheckoutSessionOutput, String>;

    fn verify_callback(
        &self,
        input: VerifyCallbackInput<'_>,
    ) -> Result<VerifyCallbackOutcome, String> {
        let normalized_payload = normalize_payment_callback_request(input.payload)
            .map_err(|detail: &'static str| detail.to_string())?;
        let signature_valid = payment_callback_signature_matches(
            &normalized_payload.payload,
            input.signature,
            input.secret,
        )?;
        Ok(VerifyCallbackOutcome {
            normalized_payload,
            signature_valid,
        })
    }
}

pub(crate) struct PaymentGatewayRegistry;

impl PaymentGatewayRegistry {
    pub(crate) fn get(payment_method: &str) -> Option<&'static dyn PaymentGatewayAdapter> {
        match payment_method.trim().to_ascii_lowercase().as_str() {
            "alipay" => Some(&ALIPAY_ADAPTER),
            "wechat" => Some(&WECHAT_ADAPTER),
            "manual" => Some(&MANUAL_ADAPTER),
            _ => None,
        }
    }
}

struct AlipayAdapter;
struct WechatAdapter;
struct ManualAdapter;

static ALIPAY_ADAPTER: AlipayAdapter = AlipayAdapter;
static WECHAT_ADAPTER: WechatAdapter = WechatAdapter;
static MANUAL_ADAPTER: ManualAdapter = ManualAdapter;

impl PaymentGatewayAdapter for AlipayAdapter {
    fn payment_method(&self) -> &'static str {
        "alipay"
    }

    fn create_checkout_session(
        &self,
        input: &CreateCheckoutSessionInput,
    ) -> Result<CreateCheckoutSessionOutput, String> {
        let expires_at = input.expires_at.to_rfc3339();
        let gateway_order_id = format!("ali_{}", input.order_no);
        Ok(CreateCheckoutSessionOutput {
            gateway_order_id: gateway_order_id.clone(),
            gateway_response: json!({
                "gateway": self.payment_method(),
                "display_name": "支付宝",
                "gateway_order_id": gateway_order_id,
                "payment_url": format!("/pay/mock/alipay/{}", input.order_no),
                "qr_code": format!("mock://alipay/{}", input.order_no),
                "expires_at": expires_at,
                "amount_usd": input.amount_usd,
            }),
        })
    }
}

impl PaymentGatewayAdapter for WechatAdapter {
    fn payment_method(&self) -> &'static str {
        "wechat"
    }

    fn create_checkout_session(
        &self,
        input: &CreateCheckoutSessionInput,
    ) -> Result<CreateCheckoutSessionOutput, String> {
        let expires_at = input.expires_at.to_rfc3339();
        let gateway_order_id = format!("wx_{}", input.order_no);
        Ok(CreateCheckoutSessionOutput {
            gateway_order_id: gateway_order_id.clone(),
            gateway_response: json!({
                "gateway": self.payment_method(),
                "display_name": "微信支付",
                "gateway_order_id": gateway_order_id,
                "payment_url": format!("/pay/mock/wechat/{}", input.order_no),
                "qr_code": format!("mock://wechat/{}", input.order_no),
                "expires_at": expires_at,
                "amount_usd": input.amount_usd,
            }),
        })
    }
}

impl PaymentGatewayAdapter for ManualAdapter {
    fn payment_method(&self) -> &'static str {
        "manual"
    }

    fn create_checkout_session(
        &self,
        input: &CreateCheckoutSessionInput,
    ) -> Result<CreateCheckoutSessionOutput, String> {
        let expires_at = input.expires_at.to_rfc3339();
        let gateway_order_id = format!("manual_{}", input.order_no);
        Ok(CreateCheckoutSessionOutput {
            gateway_order_id: gateway_order_id.clone(),
            gateway_response: json!({
                "gateway": self.payment_method(),
                "display_name": "人工打款",
                "gateway_order_id": gateway_order_id,
                "payment_url": serde_json::Value::Null,
                "qr_code": serde_json::Value::Null,
                "instructions": "请线下确认到账后由管理员处理",
                "expires_at": expires_at,
                "amount_usd": input.amount_usd,
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{CreateCheckoutSessionInput, PaymentGatewayRegistry};
    use chrono::Utc;

    #[test]
    fn registry_resolves_builtin_mock_adapters() {
        assert!(PaymentGatewayRegistry::get("alipay").is_some());
        assert!(PaymentGatewayRegistry::get("wechat").is_some());
        assert!(PaymentGatewayRegistry::get("manual").is_some());
        assert!(PaymentGatewayRegistry::get("unknown").is_none());
    }

    #[test]
    fn builtin_mock_checkout_payloads_keep_existing_frontend_keys() {
        let adapter = PaymentGatewayRegistry::get("wechat").expect("adapter should exist");
        let checkout = adapter
            .create_checkout_session(&CreateCheckoutSessionInput {
                order_no: "po_test".to_string(),
                amount_usd: 12.5,
                expires_at: Utc::now(),
            })
            .expect("checkout should build");
        let payload = checkout
            .gateway_response
            .as_object()
            .expect("gateway response should be object");

        for key in ["gateway_order_id", "payment_url", "qr_code", "expires_at"] {
            assert!(
                payload.contains_key(key),
                "mock checkout payload should contain {key}"
            );
        }
    }
}
