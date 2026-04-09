use crate::handlers::admin::request::AdminAppState;
use crate::GatewayError;
use aether_admin::observability::stats::round_to;
use aether_data_contracts::repository::usage::{StoredRequestUsageAudit, UsageAuditListQuery};
use serde_json::json;
use std::collections::BTreeMap;

pub(in super::super) async fn admin_usage_aggregation_by_user_json(
    state: &AdminAppState<'_>,
    usage: &[StoredRequestUsageAudit],
    limit: usize,
) -> Result<serde_json::Value, GatewayError> {
    let mut grouped: BTreeMap<String, (u64, u64, f64)> = BTreeMap::new();
    for item in usage {
        let Some(user_id) = item.user_id.as_ref() else {
            continue;
        };
        let entry = grouped.entry(user_id.clone()).or_insert((0, 0, 0.0));
        entry.0 = entry.0.saturating_add(1);
        entry.1 = entry.1.saturating_add(item.total_tokens);
        entry.2 += item.total_cost_usd;
    }

    let usernames = if state.has_user_data_reader() && !grouped.is_empty() {
        state
            .list_users_by_ids(&grouped.keys().cloned().collect::<Vec<_>>())
            .await?
            .into_iter()
            .map(|user| (user.id, (user.email, user.username)))
            .collect::<BTreeMap<_, _>>()
    } else {
        BTreeMap::new()
    };

    let mut items: Vec<serde_json::Value> = grouped
        .into_iter()
        .map(|(user_id, (request_count, total_tokens, total_cost))| {
            let (email, username) = usernames
                .get(&user_id)
                .cloned()
                .unwrap_or((None, String::new()));
            json!({
                "user_id": user_id,
                "email": email,
                "username": if username.is_empty() { serde_json::Value::Null } else { json!(username) },
                "request_count": request_count,
                "total_tokens": total_tokens,
                "total_cost": round_to(total_cost, 6),
            })
        })
        .collect();
    items.sort_by(|left, right| {
        right["request_count"]
            .as_u64()
            .unwrap_or_default()
            .cmp(&left["request_count"].as_u64().unwrap_or_default())
            .then_with(|| {
                left["user_id"]
                    .as_str()
                    .unwrap_or_default()
                    .cmp(right["user_id"].as_str().unwrap_or_default())
            })
    });
    items.truncate(limit);
    Ok(json!(items))
}
