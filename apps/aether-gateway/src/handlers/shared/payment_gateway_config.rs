use serde_json::{json, Value};

const REFUND_ENABLED_KEY: &str = "refund_enabled";
const ALLOW_USER_REFUND_KEY: &str = "allow_user_refund";

pub(crate) fn normalize_payment_https_url(value: &str, field: &str) -> Result<String, String> {
    let trimmed = value.trim();
    let parsed =
        url::Url::parse(trimmed).map_err(|_| format!("{field} must be an absolute HTTPS URL"))?;
    if parsed.scheme() != "https"
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.fragment().is_some()
    {
        return Err(format!(
            "{field} must be an absolute HTTPS URL without credentials or a fragment"
        ));
    }
    let literal_ip = match parsed.host() {
        Some(url::Host::Ipv4(address)) => Some(std::net::IpAddr::V4(address)),
        Some(url::Host::Ipv6(address)) => Some(std::net::IpAddr::V6(address)),
        Some(url::Host::Domain(_)) | None => None,
    };
    if literal_ip.is_some_and(aether_http::is_private_or_reserved_ip) {
        return Err(format!(
            "{field} must not target a private or reserved address"
        ));
    }
    Ok(trimmed.to_string())
}

pub(crate) fn normalize_payment_callback_base_url(value: &str) -> Result<String, String> {
    let normalized = normalize_payment_https_url(value, "callback_base_url")?;
    let parsed = url::Url::parse(&normalized)
        .map_err(|_| "callback_base_url must be an absolute HTTPS URL".to_string())?;
    if parsed.query().is_some() {
        return Err("callback_base_url must not contain a query string".to_string());
    }
    Ok(normalized.trim_end_matches('/').to_string())
}

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

#[cfg(test)]
mod tests {
    use super::{normalize_payment_callback_base_url, normalize_payment_https_url};

    #[test]
    fn payment_urls_require_absolute_https_without_embedded_credentials() {
        assert_eq!(
            normalize_payment_https_url(" https://pay.example/submit.php ", "endpoint_url"),
            Ok("https://pay.example/submit.php".to_string())
        );

        for value in [
            "javascript:alert(1)",
            "data:text/html,attack",
            "//pay.example/submit.php",
            "/submit.php",
            "http://pay.example/submit.php",
            "https://user:secret@pay.example/submit.php",
            "https://pay.example/submit.php#fragment",
            "https://127.0.0.1/submit.php",
            "https://169.254.169.254/latest/meta-data",
            "https://[::1]/submit.php",
        ] {
            assert!(
                normalize_payment_https_url(value, "endpoint_url").is_err(),
                "unsafe URL should be rejected: {value}"
            );
        }
    }

    #[test]
    fn callback_base_url_rejects_query_strings_and_trims_trailing_slashes() {
        assert_eq!(
            normalize_payment_callback_base_url("https://app.example/"),
            Ok("https://app.example".to_string())
        );
        assert!(normalize_payment_callback_base_url("https://app.example/?tenant=one").is_err());
    }
}
