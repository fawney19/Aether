use super::requests::ADMIN_WALLETS_API_KEY_GIFT_ADJUST_DETAIL;
use crate::handlers::admin::request::AdminRequestContext;
use crate::handlers::admin::shared::query_param_value;
use crate::GatewayError;
use sqlx::Row;

pub(in super::super) fn admin_wallet_operator_id(
    request_context: &AdminRequestContext<'_>,
) -> Option<String> {
    request_context
        .decision()
        .and_then(|decision| decision.admin_principal.as_ref())
        .map(|principal| principal.user_id.clone())
}

pub(in super::super) fn admin_wallet_recharge_reason_code(payment_method: &str) -> &'static str {
    match payment_method {
        "card_code" | "gift_code" | "card_recharge" => "topup_card_code",
        _ => "topup_admin_manual",
    }
}

pub(in super::super) fn admin_wallet_apply_manual_recharge_to_snapshot(
    wallet: &mut aether_data::repository::wallet::StoredWalletSnapshot,
    amount_usd: f64,
) -> (f64, f64, f64, f64, f64, f64) {
    let recharge_before = wallet.balance;
    let gift_before = wallet.gift_balance;
    let balance_before = recharge_before + gift_before;

    wallet.balance += amount_usd;
    wallet.total_recharged += amount_usd;
    wallet.updated_at_unix_secs = chrono::Utc::now().timestamp().max(0) as u64;

    let recharge_after = wallet.balance;
    let gift_after = wallet.gift_balance;
    let balance_after = recharge_after + gift_after;

    (
        balance_before,
        balance_after,
        recharge_before,
        recharge_after,
        gift_before,
        gift_after,
    )
}

pub(in super::super) fn admin_wallet_apply_adjust_to_snapshot(
    wallet: &mut aether_data::repository::wallet::StoredWalletSnapshot,
    amount_usd: f64,
    balance_type: &str,
) -> Result<(f64, f64, f64, f64, f64, f64), String> {
    if amount_usd == 0.0 {
        return Err("adjust amount must not be zero".to_string());
    }
    if balance_type == "gift" && wallet.api_key_id.is_some() {
        return Err(ADMIN_WALLETS_API_KEY_GIFT_ADJUST_DETAIL.to_string());
    }

    let recharge_before = wallet.balance;
    let gift_before = wallet.gift_balance;
    let balance_before = recharge_before + gift_before;

    let mut recharge_after = recharge_before;
    let mut gift_after = gift_before;

    if amount_usd > 0.0 {
        if balance_type == "gift" {
            gift_after += amount_usd;
        } else {
            recharge_after += amount_usd;
        }
    } else {
        let mut remaining = -amount_usd;
        let consume_positive_bucket = |balance: &mut f64, remaining: &mut f64| {
            if *remaining <= 0.0 {
                return;
            }
            let available = balance.max(0.0);
            let consumed = available.min(*remaining);
            *balance -= consumed;
            *remaining -= consumed;
        };

        if balance_type == "gift" {
            consume_positive_bucket(&mut gift_after, &mut remaining);
            consume_positive_bucket(&mut recharge_after, &mut remaining);
        } else {
            consume_positive_bucket(&mut recharge_after, &mut remaining);
            consume_positive_bucket(&mut gift_after, &mut remaining);
        }

        if remaining > 0.0 {
            recharge_after -= remaining;
        }
        if gift_after < 0.0 {
            return Err("gift balance cannot be negative".to_string());
        }
    }

    wallet.balance = recharge_after;
    wallet.gift_balance = gift_after;
    wallet.total_adjusted += amount_usd;
    wallet.updated_at_unix_secs = chrono::Utc::now().timestamp().max(0) as u64;

    Ok((
        balance_before,
        recharge_after + gift_after,
        recharge_before,
        recharge_after,
        gift_before,
        gift_after,
    ))
}

pub(in super::super) fn admin_wallet_id_from_detail_path(request_path: &str) -> Option<String> {
    request_path
        .strip_prefix("/api/admin/wallets/")?
        .trim()
        .trim_matches('/')
        .split('/')
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .filter(|value| !value.contains('/'))
        .map(ToOwned::to_owned)
}

pub(in super::super) fn admin_wallet_id_from_suffix_path(
    request_path: &str,
    suffix: &str,
) -> Option<String> {
    request_path
        .strip_prefix("/api/admin/wallets/")?
        .strip_suffix(suffix)
        .map(|value| value.trim().trim_matches('/').to_string())
        .filter(|value| !value.is_empty() && !value.contains('/'))
}

pub(in super::super) fn admin_wallet_refund_ids_from_suffix_path(
    request_path: &str,
    suffix: &str,
) -> Option<(String, String)> {
    let trimmed = request_path
        .strip_prefix("/api/admin/wallets/")?
        .strip_suffix(suffix)?
        .trim()
        .trim_matches('/');
    let mut segments = trimmed.split('/');
    let wallet_id = segments.next()?.trim();
    let literal = segments.next()?.trim();
    let refund_id = segments.next()?.trim();
    if literal != "refunds"
        || wallet_id.is_empty()
        || refund_id.is_empty()
        || wallet_id.contains('/')
        || refund_id.contains('/')
        || segments.next().is_some()
    {
        return None;
    }
    Some((wallet_id.to_string(), refund_id.to_string()))
}

pub(in super::super) fn parse_admin_wallets_limit(query: Option<&str>) -> Result<usize, String> {
    match query_param_value(query, "limit") {
        Some(value) => {
            let parsed = value
                .parse::<usize>()
                .map_err(|_| "limit must be an integer between 1 and 200".to_string())?;
            if (1..=200).contains(&parsed) {
                Ok(parsed)
            } else {
                Err("limit must be an integer between 1 and 200".to_string())
            }
        }
        None => Ok(50),
    }
}

pub(in super::super) fn parse_admin_wallets_offset(query: Option<&str>) -> Result<usize, String> {
    match query_param_value(query, "offset") {
        Some(value) => value
            .parse::<usize>()
            .map_err(|_| "offset must be a non-negative integer".to_string()),
        None => Ok(0),
    }
}

pub(in super::super) fn parse_admin_wallets_owner_type_filter(
    query: Option<&str>,
) -> Option<String> {
    match query_param_value(query, "owner_type") {
        Some(value) if value.eq_ignore_ascii_case("user") => Some("user".to_string()),
        Some(value) if value.eq_ignore_ascii_case("api_key") => Some("api_key".to_string()),
        _ => None,
    }
}

pub(in super::super) fn optional_epoch_value(
    row: &sqlx::postgres::PgRow,
    key: &str,
) -> Result<Option<String>, GatewayError> {
    Ok(row
        .try_get::<Option<i64>, _>(key)
        .map_err(|err| GatewayError::Internal(err.to_string()))?
        .and_then(|value| u64::try_from(value).ok())
        .and_then(crate::handlers::admin::shared::unix_secs_to_rfc3339))
}

pub(in super::super) fn admin_wallet_build_order_no(now: chrono::DateTime<chrono::Utc>) -> String {
    format!(
        "po_{}_{}",
        now.format("%Y%m%d%H%M%S%6f"),
        &uuid::Uuid::new_v4().simple().to_string()[..12]
    )
}
