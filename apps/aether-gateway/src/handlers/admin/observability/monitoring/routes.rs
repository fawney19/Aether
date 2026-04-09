use super::activity::{
    build_admin_monitoring_audit_logs_response,
    build_admin_monitoring_suspicious_activities_response,
    build_admin_monitoring_system_status_response, build_admin_monitoring_user_behavior_response,
};
use super::cache::{
    build_admin_monitoring_cache_config_response, build_admin_monitoring_cache_metrics_response,
    build_admin_monitoring_cache_stats_response,
};
use super::cache_affinity_reads::{
    build_admin_monitoring_cache_affinities_response,
    build_admin_monitoring_cache_affinity_response,
};
use super::cache_model_mapping::{
    build_admin_monitoring_model_mapping_stats_response,
    build_admin_monitoring_redis_cache_categories_response,
};
use super::cache_mutations::{
    build_admin_monitoring_cache_affinity_delete_response,
    build_admin_monitoring_cache_flush_response,
    build_admin_monitoring_cache_provider_delete_response,
    build_admin_monitoring_cache_users_delete_response,
    build_admin_monitoring_model_mapping_delete_model_response,
    build_admin_monitoring_model_mapping_delete_provider_response,
    build_admin_monitoring_model_mapping_delete_response,
    build_admin_monitoring_redis_keys_delete_response,
};
use super::resilience::{
    build_admin_monitoring_reset_error_stats_response,
    build_admin_monitoring_resilience_circuit_history_response,
    build_admin_monitoring_resilience_status_response,
};
use super::trace::{
    build_admin_monitoring_trace_provider_stats_response,
    build_admin_monitoring_trace_request_response,
};
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::attach_admin_audit_response;
use crate::GatewayError;
use aether_admin::observability::monitoring::{match_admin_monitoring_route, AdminMonitoringRoute};
use axum::{body::Body, http, response::Response};

pub(crate) async fn maybe_build_local_admin_monitoring_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let Some(route) = match_admin_monitoring_route(
        &request_context.request_method,
        request_context.request_path.as_str(),
    ) else {
        return Ok(None);
    };

    match route {
        AdminMonitoringRoute::AuditLogs => Ok(Some(attach_admin_audit_response(
            build_admin_monitoring_audit_logs_response(state, request_context).await?,
            "admin_monitoring_audit_logs_viewed",
            "view_admin_audit_logs",
            "audit_log",
            &admin_monitoring_audit_target_id(request_context),
        ))),
        AdminMonitoringRoute::ResilienceStatus => Ok(Some(
            build_admin_monitoring_resilience_status_response(state).await?,
        )),
        AdminMonitoringRoute::ResilienceErrorStats => Ok(Some(
            build_admin_monitoring_reset_error_stats_response(state, request_context).await?,
        )),
        AdminMonitoringRoute::ResilienceCircuitHistory => Ok(Some(
            build_admin_monitoring_resilience_circuit_history_response(state, request_context)
                .await?,
        )),
        AdminMonitoringRoute::CacheStats => Ok(Some(
            build_admin_monitoring_cache_stats_response(state).await?,
        )),
        AdminMonitoringRoute::CacheAffinities => Ok(Some(attach_admin_audit_response(
            build_admin_monitoring_cache_affinities_response(state, request_context).await?,
            "admin_monitoring_cache_affinities_viewed",
            "view_cache_affinities",
            "cache_affinity",
            &admin_monitoring_audit_target_id(request_context),
        ))),
        AdminMonitoringRoute::CacheAffinity => Ok(Some(attach_admin_audit_response(
            build_admin_monitoring_cache_affinity_response(state, request_context).await?,
            "admin_monitoring_cache_affinity_viewed",
            "view_cache_affinity",
            "cache_affinity",
            &admin_monitoring_audit_target_id(request_context),
        ))),
        AdminMonitoringRoute::CacheUsersDelete => Ok(Some(
            build_admin_monitoring_cache_users_delete_response(state, request_context).await?,
        )),
        AdminMonitoringRoute::CacheAffinityDelete => Ok(Some(
            build_admin_monitoring_cache_affinity_delete_response(state, request_context).await?,
        )),
        AdminMonitoringRoute::CacheFlush => Ok(Some(
            build_admin_monitoring_cache_flush_response(state).await?,
        )),
        AdminMonitoringRoute::CacheProviderDelete => Ok(Some(
            build_admin_monitoring_cache_provider_delete_response(state, request_context).await?,
        )),
        AdminMonitoringRoute::CacheModelMappingDelete => Ok(Some(
            build_admin_monitoring_model_mapping_delete_response(state).await?,
        )),
        AdminMonitoringRoute::CacheModelMappingDeleteModel => Ok(Some(
            build_admin_monitoring_model_mapping_delete_model_response(state, request_context)
                .await?,
        )),
        AdminMonitoringRoute::CacheModelMappingDeleteProvider => Ok(Some(
            build_admin_monitoring_model_mapping_delete_provider_response(state, request_context)
                .await?,
        )),
        AdminMonitoringRoute::CacheRedisKeysDelete => Ok(Some(
            build_admin_monitoring_redis_keys_delete_response(state, request_context).await?,
        )),
        AdminMonitoringRoute::CacheMetrics => Ok(Some(
            build_admin_monitoring_cache_metrics_response(state).await?,
        )),
        AdminMonitoringRoute::CacheConfig => {
            Ok(Some(build_admin_monitoring_cache_config_response().await?))
        }
        AdminMonitoringRoute::CacheModelMappingStats => Ok(Some(
            build_admin_monitoring_model_mapping_stats_response(state).await?,
        )),
        AdminMonitoringRoute::CacheRedisKeys => Ok(Some(
            build_admin_monitoring_redis_cache_categories_response(state).await?,
        )),
        AdminMonitoringRoute::SystemStatus => Ok(Some(
            build_admin_monitoring_system_status_response(state).await?,
        )),
        AdminMonitoringRoute::SuspiciousActivities => Ok(Some(attach_admin_audit_response(
            build_admin_monitoring_suspicious_activities_response(state, request_context).await?,
            "admin_monitoring_suspicious_activities_viewed",
            "view_suspicious_activities",
            "suspicious_activity",
            &admin_monitoring_audit_target_id(request_context),
        ))),
        AdminMonitoringRoute::UserBehavior => Ok(Some(attach_admin_audit_response(
            build_admin_monitoring_user_behavior_response(state, request_context).await?,
            "admin_monitoring_user_behavior_viewed",
            "view_user_behavior",
            "user",
            &admin_monitoring_audit_target_id(request_context),
        ))),
        AdminMonitoringRoute::TraceRequest => Ok(Some(attach_admin_audit_response(
            build_admin_monitoring_trace_request_response(state, request_context).await?,
            "admin_monitoring_request_trace_viewed",
            "view_request_trace",
            "request_trace",
            &admin_monitoring_audit_target_id(request_context),
        ))),
        AdminMonitoringRoute::TraceProviderStats => Ok(Some(attach_admin_audit_response(
            build_admin_monitoring_trace_provider_stats_response(state, request_context).await?,
            "admin_monitoring_provider_trace_stats_viewed",
            "view_provider_trace_stats",
            "provider",
            &admin_monitoring_audit_target_id(request_context),
        ))),
    }
}

fn admin_monitoring_audit_target_id(request_context: &AdminRequestContext<'_>) -> String {
    match request_context.request_query_string.as_deref() {
        Some(query) if !query.trim().is_empty() => {
            format!("{}?{query}", request_context.request_path)
        }
        _ => request_context.request_path.clone(),
    }
}
