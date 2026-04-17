use crate::handlers::admin::request::AdminAppState;
use crate::GatewayError;
use aether_admin::observability::stats::round_to;
use aether_data_contracts::repository::usage::StoredUsageAuditAggregation;
use serde_json::json;
use std::collections::BTreeMap;

pub(in super::super) async fn admin_usage_aggregation_by_user_json(
    state: &AdminAppState<'_>,
    rows: &[StoredUsageAuditAggregation],
) -> Result<serde_json::Value, GatewayError> {
    let user_ids = rows
        .iter()
        .map(|row| row.group_key.clone())
        .collect::<Vec<_>>();
    let usernames = if state.has_user_data_reader() && !user_ids.is_empty() {
        state
            .list_users_by_ids(&user_ids)
            .await?
            .into_iter()
            .map(|user| (user.id, (user.email, user.username)))
            .collect::<BTreeMap<_, _>>()
    } else {
        BTreeMap::new()
    };

    Ok(json!(rows
        .iter()
        .map(|row| {
            let (email, username) = usernames
                .get(&row.group_key)
                .cloned()
                .unwrap_or((None, String::new()));
            json!({
                "user_id": row.group_key,
                "email": email,
                "username": if username.is_empty() {
                    serde_json::Value::Null
                } else {
                    json!(username)
                },
                "request_count": row.request_count,
                "total_tokens": row.total_tokens,
                "total_cost": round_to(row.total_cost_usd, 6),
            })
        })
        .collect::<Vec<_>>()))
}
