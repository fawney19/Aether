use serde_json::{json, Value};

const REFUND_ENABLED_KEY: &str = "refund_enabled";
const ALLOW_USER_REFUND_KEY: &str = "allow_user_refund";

fn json_bool(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Bool(value)) => *value,
        Some(Value::Number(value)) => value.as_u64().is_some_and(|value| value != 0),
        Some(Value::String(value)) => matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        _ => false,
    }
}

pub(crate) fn payment_gateway_channels_json(value: &Value) -> Value {
    value
        .as_object()
        .and_then(|object| object.get("channels"))
        .cloned()
        .unwrap_or_else(|| value.clone())
}

pub(crate) fn payment_gateway_config_json(value: &Value) -> Value {
    value
        .as_object()
        .and_then(|object| object.get("config"))
        .cloned()
        .unwrap_or_else(|| json!({}))
}

pub(crate) fn payment_gateway_secret_keys_json(value: &Value) -> Value {
    value
        .as_object()
        .and_then(|object| object.get("secret_keys"))
        .cloned()
        .unwrap_or_else(|| json!([]))
}

pub(crate) fn payment_gateway_refund_enabled(value: &Value) -> bool {
    json_bool(
        value
            .as_object()
            .and_then(|object| object.get(REFUND_ENABLED_KEY)),
    )
}

pub(crate) fn payment_gateway_allow_user_refund(value: &Value) -> bool {
    payment_gateway_refund_enabled(value)
        && json_bool(
            value
                .as_object()
                .and_then(|object| object.get(ALLOW_USER_REFUND_KEY)),
        )
}

pub(crate) fn payment_gateway_channels_config_json(
    channels: Value,
    config: Value,
    secret_keys: Value,
    refund_enabled: bool,
    allow_user_refund: bool,
) -> Value {
    json!({
        "channels": channels,
        "config": config,
        "secret_keys": secret_keys,
        "refund_enabled": refund_enabled,
        "allow_user_refund": refund_enabled && allow_user_refund,
    })
}

pub(crate) fn payment_gateway_provider_for_payment_method(
    payment_method: &str,
) -> Option<&'static str> {
    match payment_method.trim().to_ascii_lowercase().as_str() {
        "epay" => Some("epay"),
        "alipay" => Some("alipay"),
        "wxpay" => Some("wxpay"),
        "stripe" => Some("stripe"),
        _ => None,
    }
}
