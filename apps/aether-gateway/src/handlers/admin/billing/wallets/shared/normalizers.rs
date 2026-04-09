pub(in super::super) fn normalize_admin_wallet_description(
    value: Option<String>,
) -> Result<Option<String>, String> {
    match value {
        None => Ok(None),
        Some(value) => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                return Ok(None);
            }
            Ok(Some(trimmed.chars().take(500).collect()))
        }
    }
}

pub(in super::super) fn normalize_admin_wallet_required_text(
    value: String,
    field_name: &str,
    max_len: usize,
) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("{field_name} 不能为空"));
    }
    if trimmed.chars().count() > max_len {
        return Err(format!("{field_name} 长度不能超过 {max_len}"));
    }
    Ok(trimmed.to_string())
}

pub(in super::super) fn normalize_admin_wallet_optional_text(
    value: Option<String>,
    field_name: &str,
    max_len: usize,
) -> Result<Option<String>, String> {
    match value {
        None => Ok(None),
        Some(value) => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                return Ok(None);
            }
            if trimmed.chars().count() > max_len {
                return Err(format!("{field_name} 长度不能超过 {max_len}"));
            }
            Ok(Some(trimmed.to_string()))
        }
    }
}

pub(in super::super) fn normalize_admin_wallet_payment_method(
    value: String,
) -> Result<String, String> {
    let normalized = value.trim();
    if normalized.is_empty() {
        return Err("payment_method 不能为空".to_string());
    }
    Ok(normalized.chars().take(30).collect())
}

pub(in super::super) fn normalize_admin_wallet_balance_type(
    value: String,
) -> Result<String, String> {
    let normalized = value.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "recharge" | "gift" => Ok(normalized),
        _ => Err("balance_type 必须为 recharge 或 gift".to_string()),
    }
}

pub(in super::super) fn normalize_admin_wallet_positive_amount(
    value: f64,
    field_name: &str,
) -> Result<f64, String> {
    if !value.is_finite() || value <= 0.0 {
        return Err(format!("{field_name} 必须为大于 0 的有限数字"));
    }
    Ok(value)
}

pub(in super::super) fn normalize_admin_wallet_non_zero_amount(
    value: f64,
    field_name: &str,
) -> Result<f64, String> {
    if !value.is_finite() || value == 0.0 {
        return Err(format!("{field_name} 不能为 0，且必须为有限数字"));
    }
    Ok(value)
}
