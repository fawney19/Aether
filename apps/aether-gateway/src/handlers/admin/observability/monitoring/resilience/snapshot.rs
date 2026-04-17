use super::super::usage_helpers::admin_monitoring_usage_is_error;
use crate::handlers::admin::request::AdminAppState;
use crate::handlers::admin::shared::{provider_key_health_summary, unix_secs_to_rfc3339};
use crate::GatewayError;
use aether_data_contracts::repository::{
    provider_catalog::StoredProviderCatalogKey, usage::UsageMonitoringErrorListQuery,
};
use serde_json::json;
use std::collections::BTreeMap;

pub(super) struct AdminMonitoringResilienceSnapshot {
    pub(super) timestamp: chrono::DateTime<chrono::Utc>,
    pub(super) health_score: i64,
    pub(super) status: &'static str,
    pub(super) error_statistics: serde_json::Value,
    pub(super) recent_errors: Vec<serde_json::Value>,
    pub(super) recommendations: Vec<String>,
    pub(super) previous_stats: serde_json::Value,
}

fn build_admin_monitoring_resilience_recommendations(
    total_errors: usize,
    health_score: i64,
    open_breaker_labels: &[String],
) -> Vec<String> {
    let mut recommendations = Vec::new();
    if health_score < 50 {
        recommendations.push("系统健康状况严重，请立即检查错误日志".to_string());
    }
    if total_errors > 100 {
        recommendations.push("错误频率过高，建议检查系统配置和外部依赖".to_string());
    }
    if !open_breaker_labels.is_empty() {
        recommendations.push(format!(
            "以下服务熔断器已打开：{}",
            open_breaker_labels.join(", ")
        ));
    }
    if health_score > 90 {
        recommendations.push("系统运行良好".to_string());
    }
    recommendations
}

pub(super) async fn build_admin_monitoring_provider_name_by_id_and_keys(
    state: &AdminAppState<'_>,
) -> Result<(BTreeMap<String, String>, Vec<StoredProviderCatalogKey>), GatewayError> {
    let providers = state.list_provider_catalog_providers(false).await?;
    let provider_ids = providers
        .iter()
        .map(|item| item.id.clone())
        .collect::<Vec<_>>();
    let provider_name_by_id = providers
        .iter()
        .map(|item| (item.id.clone(), item.name.clone()))
        .collect::<BTreeMap<_, _>>();
    let keys = if provider_ids.is_empty() {
        Vec::new()
    } else {
        state
            .list_provider_catalog_key_summaries_by_provider_ids(&provider_ids)
            .await?
    };
    Ok((provider_name_by_id, keys))
}

pub(super) async fn build_admin_monitoring_resilience_snapshot(
    state: &AdminAppState<'_>,
) -> Result<AdminMonitoringResilienceSnapshot, GatewayError> {
    let now = chrono::Utc::now();
    let recent_error_from = std::cmp::max(
        now - chrono::Duration::hours(24),
        chrono::DateTime::<chrono::Utc>::from_timestamp(
            state
                .admin_monitoring_error_stats_reset_at()
                .unwrap_or_default() as i64,
            0,
        )
        .unwrap_or_else(|| {
            chrono::DateTime::<chrono::Utc>::from_timestamp(0, 0).expect("unix epoch should exist")
        }),
    );

    let (provider_name_by_id, keys) =
        build_admin_monitoring_provider_name_by_id_and_keys(state).await?;

    let active_keys = keys.iter().filter(|item| item.is_active).count();
    let mut degraded_keys = 0usize;
    let mut unhealthy_keys = 0usize;
    let mut open_circuit_breakers = 0usize;
    let mut open_breaker_labels = Vec::new();
    let mut circuit_breakers = serde_json::Map::new();
    let mut previous_circuit_breakers = serde_json::Map::new();

    for key in &keys {
        let (
            health_score,
            consecutive_failures,
            last_failure_at,
            circuit_breaker_open,
            circuit_by_format,
        ) = provider_key_health_summary(key);
        if health_score < 0.8 {
            degraded_keys += 1;
        }
        if health_score < 0.5 {
            unhealthy_keys += 1;
        }

        let open_formats = circuit_by_format
            .iter()
            .filter_map(|(api_format, value)| {
                value
                    .get("open")
                    .and_then(serde_json::Value::as_bool)
                    .filter(|open| *open)
                    .map(|_| api_format.clone())
            })
            .collect::<Vec<_>>();

        if circuit_breaker_open {
            open_circuit_breakers += 1;
            let provider_label = provider_name_by_id
                .get(&key.provider_id)
                .cloned()
                .unwrap_or_else(|| key.provider_id.clone());
            open_breaker_labels.push(format!("{provider_label}/{}", key.name));
        }

        if circuit_breaker_open || consecutive_failures > 0 || health_score < 1.0 {
            circuit_breakers.insert(
                key.id.clone(),
                json!({
                    "state": if circuit_breaker_open { "open" } else { "closed" },
                    "provider_id": key.provider_id,
                    "provider_name": provider_name_by_id.get(&key.provider_id).cloned(),
                    "key_name": key.name,
                    "health_score": health_score,
                    "consecutive_failures": consecutive_failures,
                    "last_failure_at": last_failure_at,
                    "open_formats": open_formats,
                }),
            );
            previous_circuit_breakers.insert(
                key.id.clone(),
                json!({
                    "state": if circuit_breaker_open { "open" } else { "closed" },
                    "failure_count": consecutive_failures,
                }),
            );
        }
    }

    let mut recent_usage_errors = state
        .list_monitoring_usage_errors(&UsageMonitoringErrorListQuery {
            created_from_unix_secs: recent_error_from.timestamp().max(0) as u64,
            created_until_unix_secs: (now.timestamp().max(0) as u64).saturating_add(1),
            limit: None,
        })
        .await?
        .into_iter()
        .filter(admin_monitoring_usage_is_error)
        .collect::<Vec<_>>();
    recent_usage_errors.sort_by_key(|item| std::cmp::Reverse(item.created_at_unix_ms));

    let total_errors = recent_usage_errors.len();
    let mut error_breakdown = BTreeMap::<String, usize>::new();
    for item in &recent_usage_errors {
        let error_type = item
            .error_category
            .clone()
            .unwrap_or_else(|| item.status.clone());
        let operation = format!(
            "{}:{}",
            item.provider_name,
            item.api_format
                .clone()
                .unwrap_or_else(|| item.model.clone())
        );
        *error_breakdown
            .entry(format!("{error_type}:{operation}"))
            .or_default() += 1;
    }
    let recent_errors = recent_usage_errors
        .iter()
        .take(10)
        .map(|item| {
            let error_type = item
                .error_category
                .clone()
                .unwrap_or_else(|| item.status.clone());
            let operation = format!(
                "{}:{}",
                item.provider_name,
                item.api_format
                    .clone()
                    .unwrap_or_else(|| item.model.clone())
            );
            json!({
                "error_id": item.id,
                "error_type": error_type,
                "operation": operation,
                "timestamp": unix_secs_to_rfc3339(item.created_at_unix_ms),
                "context": {
                    "request_id": item.request_id,
                    "provider_id": item.provider_id,
                    "provider_name": item.provider_name,
                    "model": item.model,
                    "api_format": item.api_format,
                    "status_code": item.status_code,
                    "error_message": item.error_message,
                }
            })
        })
        .collect::<Vec<_>>();

    let health_score = (100_i64
        - i64::try_from(total_errors)
            .unwrap_or(i64::MAX)
            .saturating_mul(2)
        - i64::try_from(open_circuit_breakers)
            .unwrap_or(i64::MAX)
            .saturating_mul(20))
    .clamp(0, 100);
    let status = if health_score > 80 {
        "healthy"
    } else if health_score > 50 {
        "degraded"
    } else {
        "critical"
    };
    let recommendations = build_admin_monitoring_resilience_recommendations(
        total_errors,
        health_score,
        &open_breaker_labels,
    );

    Ok(AdminMonitoringResilienceSnapshot {
        timestamp: now,
        health_score,
        status,
        error_statistics: json!({
            "total_errors": total_errors,
            "active_keys": active_keys,
            "degraded_keys": degraded_keys,
            "unhealthy_keys": unhealthy_keys,
            "open_circuit_breakers": open_circuit_breakers,
            "circuit_breakers": circuit_breakers,
        }),
        recent_errors,
        recommendations,
        previous_stats: json!({
            "total_errors": total_errors,
            "error_breakdown": error_breakdown,
            "recent_errors": total_errors,
            "circuit_breakers": previous_circuit_breakers,
        }),
    })
}
