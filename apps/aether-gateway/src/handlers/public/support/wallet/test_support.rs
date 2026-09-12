#[derive(Debug, Clone)]
pub(super) struct WalletTestRefundRecord {
    pub(crate) wallet_id: String,
    pub(crate) user_id: String,
    pub(crate) idempotency_key: Option<String>,
    pub(crate) payload: serde_json::Value,
}

#[derive(Debug, Clone)]
pub(crate) struct WalletTestRechargeRecord {
    pub(crate) user_id: String,
    pub(crate) order_no: String,
    pub(crate) payload: serde_json::Value,
}

pub(super) fn wallet_test_refund_store() -> &'static std::sync::Mutex<Vec<WalletTestRefundRecord>> {
    static STORE: std::sync::OnceLock<std::sync::Mutex<Vec<WalletTestRefundRecord>>> =
        std::sync::OnceLock::new();
    STORE.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

pub(crate) fn wallet_test_recharge_store(
) -> &'static std::sync::Mutex<Vec<WalletTestRechargeRecord>> {
    static STORE: std::sync::OnceLock<std::sync::Mutex<Vec<WalletTestRechargeRecord>>> =
        std::sync::OnceLock::new();
    STORE.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

pub(super) fn wallet_test_refunds_for_wallet(wallet_id: &str) -> Vec<serde_json::Value> {
    let mut items = wallet_test_refund_store()
        .lock()
        .expect("wallet test refund store should lock")
        .iter()
        .filter(|entry| entry.wallet_id == wallet_id)
        .map(|entry| entry.payload.clone())
        .collect::<Vec<_>>();
    items.sort_by(|left, right| {
        right["created_at"]
            .as_str()
            .cmp(&left["created_at"].as_str())
    });
    items
}

pub(super) fn wallet_test_refund_by_id(
    wallet_id: &str,
    refund_id: &str,
) -> Option<serde_json::Value> {
    wallet_test_refund_store()
        .lock()
        .expect("wallet test refund store should lock")
        .iter()
        .find(|entry| {
            entry.wallet_id == wallet_id && entry.payload["id"].as_str() == Some(refund_id)
        })
        .map(|entry| entry.payload.clone())
}

pub(super) fn wallet_test_refund_by_idempotency(
    user_id: &str,
    idempotency_key: &str,
) -> Option<serde_json::Value> {
    wallet_test_refund_store()
        .lock()
        .expect("wallet test refund store should lock")
        .iter()
        .find(|entry| {
            entry.user_id == user_id && entry.idempotency_key.as_deref() == Some(idempotency_key)
        })
        .map(|entry| entry.payload.clone())
}

pub(super) fn wallet_test_reserved_refund_amount(wallet_id: &str) -> f64 {
    wallet_test_refund_store()
        .lock()
        .expect("wallet test refund store should lock")
        .iter()
        .filter(|entry| {
            entry.wallet_id == wallet_id
                && matches!(
                    entry.payload["status"].as_str(),
                    Some("pending_approval" | "approved")
                )
        })
        .filter_map(|entry| entry.payload["amount_usd"].as_f64())
        .filter(|amount| amount.is_finite() && *amount > 0.0)
        .try_fold(0.0_f64, |total, amount| {
            let next = total + amount;
            next.is_finite().then_some(next)
        })
        .unwrap_or(f64::INFINITY)
}

pub(super) fn record_wallet_test_refund(
    wallet_id: String,
    user_id: String,
    idempotency_key: Option<String>,
    payload: serde_json::Value,
) {
    wallet_test_refund_store()
        .lock()
        .expect("wallet test refund store should lock")
        .push(WalletTestRefundRecord {
            wallet_id,
            user_id,
            idempotency_key,
            payload,
        });
}

pub(super) fn wallet_test_recharge_orders_for_user(
    user_id: &str,
    limit: usize,
    offset: usize,
) -> (Vec<serde_json::Value>, u64) {
    let mut items = wallet_test_recharge_store()
        .lock()
        .expect("wallet test recharge store should lock")
        .iter()
        .filter(|entry| entry.user_id == user_id)
        .map(|entry| entry.payload.clone())
        .collect::<Vec<_>>();
    items.sort_by(|left, right| {
        right["created_at"]
            .as_str()
            .cmp(&left["created_at"].as_str())
    });
    let total = items.len() as u64;
    let items = items
        .into_iter()
        .skip(offset)
        .take(limit)
        .collect::<Vec<_>>();
    (items, total)
}

pub(super) fn wallet_test_recharge_order_by_id(
    user_id: &str,
    order_id: &str,
) -> Option<serde_json::Value> {
    wallet_test_recharge_store()
        .lock()
        .expect("wallet test recharge store should lock")
        .iter()
        .find(|entry| entry.user_id == user_id && entry.payload["id"].as_str() == Some(order_id))
        .map(|entry| entry.payload.clone())
}

pub(super) fn wallet_test_recharge_order_by_order_no(
    user_id: &str,
    order_no: &str,
) -> Option<serde_json::Value> {
    wallet_test_recharge_store()
        .lock()
        .expect("wallet test recharge store should lock")
        .iter()
        .find(|entry| entry.user_id == user_id && entry.order_no == order_no)
        .map(|entry| entry.payload.clone())
}

pub(super) fn record_wallet_test_recharge(
    user_id: String,
    order_no: String,
    payload: serde_json::Value,
) {
    let mut store = wallet_test_recharge_store()
        .lock()
        .expect("wallet test recharge store should lock");
    if let Some(existing) = store
        .iter_mut()
        .find(|entry| entry.user_id == user_id && entry.order_no == order_no)
    {
        existing.payload = payload;
        return;
    }
    store.push(WalletTestRechargeRecord {
        user_id,
        order_no,
        payload,
    });
}
