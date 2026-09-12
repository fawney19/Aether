/// Normalize a payment currency at an external payment boundary.
///
/// Payment-order storage uses a three-character ISO-style code.  Keep the
/// boundary deliberately narrow so values cannot be truncated differently by
/// the individual database adapters or payment providers.
pub(crate) fn normalize_payment_currency(value: &str, field: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.len() != 3 || !trimmed.bytes().all(|byte| byte.is_ascii_alphabetic()) {
        return Err(format!("{field} must be a 3-letter currency code"));
    }
    Ok(trimmed.to_ascii_uppercase())
}

/// Return the exchange rate that should be persisted and used for settlement.
/// USD is already the canonical accounting currency, so a configured
/// conversion rate (often the CNY default) must never be applied to USD
/// checkouts.
pub(crate) fn effective_payment_exchange_rate(
    pay_currency: &str,
    configured_rate: f64,
) -> Result<f64, String> {
    let currency = normalize_payment_currency(pay_currency, "pay_currency")?;
    if !configured_rate.is_finite() || configured_rate <= 0.0 {
        return Err("usd_exchange_rate must be finite and positive".to_string());
    }
    Ok(if currency == "USD" {
        1.0
    } else {
        configured_rate
    })
}

pub(crate) fn stripe_minor_unit_multiplier(currency: &str) -> f64 {
    match currency.trim().to_ascii_lowercase().as_str() {
        "bif" | "clp" | "djf" | "gnf" | "jpy" | "kmf" | "krw" | "mga" | "pyg" | "rwf" | "ugx"
        | "vnd" | "vuv" | "xaf" | "xof" | "xpf" => 1.0,
        "bhd" | "jod" | "kwd" | "omr" | "tnd" => 1_000.0,
        _ => 100.0,
    }
}

pub(crate) fn stripe_amount_to_minor(amount_major: f64, currency: &str) -> Option<i64> {
    let amount_minor = (amount_major * stripe_minor_unit_multiplier(currency)).round();
    if !amount_minor.is_finite() || amount_minor <= 0.0 || amount_minor >= i64::MAX as f64 {
        return None;
    }
    Some(amount_minor as i64)
}

pub(crate) fn stripe_amount_to_major(amount_minor: i64, currency: &str) -> f64 {
    amount_minor as f64 / stripe_minor_unit_multiplier(currency)
}

#[cfg(test)]
mod tests {
    use super::{
        effective_payment_exchange_rate, normalize_payment_currency, stripe_amount_to_major,
        stripe_amount_to_minor,
    };

    #[test]
    fn payment_currency_is_trimmed_uppercased_and_bounded_to_ascii_three_letters() {
        assert_eq!(
            normalize_payment_currency(" cny ", "pay_currency"),
            Ok("CNY".to_string())
        );
        for value in ["CN", "CNYY", "C1Y", "人民币", ""] {
            assert!(
                normalize_payment_currency(value, "pay_currency").is_err(),
                "invalid currency should be rejected: {value}"
            );
        }
    }

    #[test]
    fn usd_uses_unit_effective_exchange_rate() {
        assert_eq!(effective_payment_exchange_rate(" usd ", 7.2), Ok(1.0));
        assert_eq!(effective_payment_exchange_rate("CNY", 7.2), Ok(7.2));
        assert!(effective_payment_exchange_rate("USD", f64::NAN).is_err());
        assert!(effective_payment_exchange_rate("CN", 7.2).is_err());
    }

    #[test]
    fn stripe_amounts_handle_zero_two_and_three_decimal_currencies() {
        assert_eq!(stripe_amount_to_minor(1234.0, "JPY"), Some(1234));
        assert_eq!(stripe_amount_to_major(1234, "jpy"), 1234.0);

        assert_eq!(stripe_amount_to_minor(12.34, "USD"), Some(1234));
        assert_eq!(stripe_amount_to_major(1234, "usd"), 12.34);

        assert_eq!(stripe_amount_to_minor(1.234, "KWD"), Some(1234));
        assert_eq!(stripe_amount_to_major(1234, "kwd"), 1.234);
    }

    #[test]
    fn stripe_minor_amount_rejects_invalid_or_overflowing_values() {
        assert_eq!(stripe_amount_to_minor(f64::NAN, "usd"), None);
        assert_eq!(stripe_amount_to_minor(0.0, "usd"), None);
        assert_eq!(stripe_amount_to_minor(f64::MAX, "kwd"), None);
    }
}
