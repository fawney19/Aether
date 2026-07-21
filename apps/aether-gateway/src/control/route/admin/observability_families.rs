use axum::http;

use super::{classified, ClassifiedRoute};

pub(super) fn classify_admin_observability_family_route(
    method: &http::Method,
    normalized_path: &str,
    normalized_path_no_trailing: &str,
) -> Option<ClassifiedRoute> {
    if method == http::Method::POST
        && matches!(
            normalized_path,
            "/api/admin/provider-query/models" | "/api/admin/provider-query/models/"
        )
    {
        Some(classified(
            "admin_proxy",
            "provider_query_manage",
            "query_models",
            "admin:provider_query",
            false,
        ))
    } else if method == http::Method::POST
        && matches!(
            normalized_path,
            "/api/admin/provider-query/test-model" | "/api/admin/provider-query/test-model/"
        )
    {
        Some(classified(
            "admin_proxy",
            "provider_query_manage",
            "test_model",
            "admin:provider_query",
            false,
        ))
    } else if method == http::Method::POST
        && matches!(
            normalized_path,
            "/api/admin/provider-query/test-model-failover"
                | "/api/admin/provider-query/test-model-failover/"
        )
    {
        Some(classified(
            "admin_proxy",
            "provider_query_manage",
            "test_model_failover",
            "admin:provider_query",
            false,
        ))
    } else if method == http::Method::POST
        && matches!(
            normalized_path,
            "/api/admin/security/ip/blacklist" | "/api/admin/security/ip/blacklist/"
        )
    {
        Some(classified(
            "admin_proxy",
            "security_manage",
            "blacklist_add",
            "admin:security",
            false,
        ))
    } else if method == http::Method::DELETE
        && normalized_path.starts_with("/api/admin/security/ip/blacklist/")
    {
        Some(classified(
            "admin_proxy",
            "security_manage",
            "blacklist_remove",
            "admin:security",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/admin/security/ip/blacklist/stats" | "/api/admin/security/ip/blacklist/stats/"
        )
    {
        Some(classified(
            "admin_proxy",
            "security_manage",
            "blacklist_stats",
            "admin:security",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/admin/security/ip/blacklist" | "/api/admin/security/ip/blacklist/"
        )
    {
        Some(classified(
            "admin_proxy",
            "security_manage",
            "blacklist_list",
            "admin:security",
            false,
        ))
    } else if method == http::Method::POST
        && matches!(
            normalized_path,
            "/api/admin/security/ip/whitelist" | "/api/admin/security/ip/whitelist/"
        )
    {
        Some(classified(
            "admin_proxy",
            "security_manage",
            "whitelist_add",
            "admin:security",
            false,
        ))
    } else if method == http::Method::DELETE
        && normalized_path.starts_with("/api/admin/security/ip/whitelist/")
    {
        Some(classified(
            "admin_proxy",
            "security_manage",
            "whitelist_remove",
            "admin:security",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/admin/security/ip/whitelist" | "/api/admin/security/ip/whitelist/"
        )
    {
        Some(classified(
            "admin_proxy",
            "security_manage",
            "whitelist_list",
            "admin:security",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/admin/api-keys" | "/api/admin/api-keys/"
        )
    {
        Some(classified(
            "admin_proxy",
            "api_keys_manage",
            "list_api_keys",
            "admin:api_keys",
            false,
        ))
    } else if method == http::Method::POST
        && matches!(
            normalized_path,
            "/api/admin/api-keys" | "/api/admin/api-keys/"
        )
    {
        Some(classified(
            "admin_proxy",
            "api_keys_manage",
            "create_api_key",
            "admin:api_keys",
            false,
        ))
    } else if method == http::Method::POST
        && normalized_path_no_trailing.starts_with("/api/admin/api-keys/")
        && normalized_path_no_trailing.ends_with("/install-sessions")
        && normalized_path_no_trailing.matches('/').count() == 5
    {
        Some(classified(
            "admin_proxy",
            "api_keys_manage",
            "create_api_key_install_session",
            "admin:api_keys",
            false,
        ))
    } else if method == http::Method::GET
        && normalized_path_no_trailing.starts_with("/api/admin/api-keys/")
        && normalized_path_no_trailing.matches('/').count() == 4
    {
        Some(classified(
            "admin_proxy",
            "api_keys_manage",
            "api_key_detail",
            "admin:api_keys",
            false,
        ))
    } else if method == http::Method::PUT
        && normalized_path_no_trailing.starts_with("/api/admin/api-keys/")
        && normalized_path_no_trailing.matches('/').count() == 4
    {
        Some(classified(
            "admin_proxy",
            "api_keys_manage",
            "update_api_key",
            "admin:api_keys",
            false,
        ))
    } else if method == http::Method::PATCH
        && normalized_path.starts_with("/api/admin/api-keys/")
        && normalized_path.matches('/').count() == 4
    {
        Some(classified(
            "admin_proxy",
            "api_keys_manage",
            "toggle_api_key",
            "admin:api_keys",
            false,
        ))
    } else if method == http::Method::DELETE
        && normalized_path.starts_with("/api/admin/api-keys/")
        && normalized_path.matches('/').count() == 4
    {
        Some(classified(
            "admin_proxy",
            "api_keys_manage",
            "delete_api_key",
            "admin:api_keys",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/admin/pool/overview" | "/api/admin/pool/overview/"
        )
    {
        Some(classified(
            "admin_proxy",
            "pool_manage",
            "overview",
            "admin:pool",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/admin/pool/scheduling-presets" | "/api/admin/pool/scheduling-presets/"
        )
    {
        Some(classified(
            "admin_proxy",
            "pool_manage",
            "scheduling_presets",
            "admin:pool",
            false,
        ))
    } else if method == http::Method::GET
        && normalized_path_no_trailing.starts_with("/api/admin/pool/")
        && normalized_path_no_trailing.ends_with("/keys")
        && normalized_path_no_trailing.matches('/').count() == 5
    {
        Some(classified(
            "admin_proxy",
            "pool_manage",
            "list_keys",
            "admin:pool",
            false,
        ))
    } else if method == http::Method::GET
        && normalized_path_no_trailing.starts_with("/api/admin/pool/")
        && normalized_path_no_trailing.ends_with("/scores")
        && normalized_path_no_trailing.matches('/').count() == 5
    {
        Some(classified(
            "admin_proxy",
            "pool_manage",
            "scores",
            "admin:pool",
            false,
        ))
    } else if method == http::Method::POST
        && normalized_path_no_trailing.starts_with("/api/admin/pool/")
        && normalized_path_no_trailing.ends_with("/keys/batch-import")
        && normalized_path_no_trailing.matches('/').count() == 6
    {
        Some(classified(
            "admin_proxy",
            "pool_manage",
            "batch_import_keys",
            "admin:pool",
            false,
        ))
    } else if method == http::Method::POST
        && normalized_path_no_trailing.starts_with("/api/admin/pool/")
        && normalized_path_no_trailing.ends_with("/keys/batch-action")
        && normalized_path_no_trailing.matches('/').count() == 6
    {
        Some(classified(
            "admin_proxy",
            "pool_manage",
            "batch_action_keys",
            "admin:pool",
            false,
        ))
    } else if method == http::Method::PATCH
        && normalized_path_no_trailing.starts_with("/api/admin/pool/")
        && normalized_path_no_trailing.ends_with("/keys/batch-update")
        && normalized_path_no_trailing.matches('/').count() == 6
    {
        Some(classified(
            "admin_proxy",
            "pool_manage",
            "batch_update_keys",
            "admin:pool",
            false,
        ))
    } else if method == http::Method::POST
        && normalized_path_no_trailing.starts_with("/api/admin/pool/")
        && normalized_path_no_trailing.ends_with("/keys/resolve-selection")
        && normalized_path_no_trailing.matches('/').count() == 6
    {
        Some(classified(
            "admin_proxy",
            "pool_manage",
            "resolve_selection",
            "admin:pool",
            false,
        ))
    } else if method == http::Method::GET
        && normalized_path_no_trailing.starts_with("/api/admin/pool/")
        && normalized_path_no_trailing.contains("/keys/batch-delete-task/")
        && normalized_path_no_trailing.matches('/').count() == 7
    {
        Some(classified(
            "admin_proxy",
            "pool_manage",
            "batch_delete_task_status",
            "admin:pool",
            false,
        ))
    } else if method == http::Method::POST
        && normalized_path_no_trailing.starts_with("/api/admin/pool/")
        && normalized_path_no_trailing.ends_with("/keys/cleanup-banned")
        && normalized_path_no_trailing.matches('/').count() == 6
    {
        Some(classified(
            "admin_proxy",
            "pool_manage",
            "cleanup_banned_keys",
            "admin:pool",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/admin/usage/aggregation/stats" | "/api/admin/usage/aggregation/stats/"
        )
    {
        Some(classified(
            "admin_proxy",
            "usage_manage",
            "aggregation_stats",
            "admin:usage",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/admin/usage/stats" | "/api/admin/usage/stats/"
        )
    {
        Some(classified(
            "admin_proxy",
            "usage_manage",
            "stats",
            "admin:usage",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/admin/usage/heatmap" | "/api/admin/usage/heatmap/"
        )
    {
        Some(classified(
            "admin_proxy",
            "usage_manage",
            "heatmap",
            "admin:usage",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/admin/usage/records" | "/api/admin/usage/records/"
        )
    {
        Some(classified(
            "admin_proxy",
            "usage_manage",
            "records",
            "admin:usage",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/admin/usage/active" | "/api/admin/usage/active/"
        )
    {
        Some(classified(
            "admin_proxy",
            "usage_manage",
            "active",
            "admin:usage",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/admin/usage/cache-affinity/hit-analysis"
                | "/api/admin/usage/cache-affinity/hit-analysis/"
        )
    {
        Some(classified(
            "admin_proxy",
            "usage_manage",
            "cache_affinity_hit_analysis",
            "admin:usage",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/admin/usage/cache-affinity/interval-timeline"
                | "/api/admin/usage/cache-affinity/interval-timeline/"
        )
    {
        Some(classified(
            "admin_proxy",
            "usage_manage",
            "cache_affinity_interval_timeline",
            "admin:usage",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/admin/usage/cache-affinity/ttl-analysis"
                | "/api/admin/usage/cache-affinity/ttl-analysis/"
        )
    {
        Some(classified(
            "admin_proxy",
            "usage_manage",
            "cache_affinity_ttl_analysis",
            "admin:usage",
            false,
        ))
    } else if method == http::Method::GET
        && normalized_path.starts_with("/api/admin/usage/")
        && normalized_path.ends_with("/curl")
        && normalized_path.matches('/').count() == 5
    {
        Some(classified(
            "admin_proxy",
            "usage_manage",
            "curl",
            "admin:usage",
            false,
        ))
    } else if method == http::Method::POST
        && normalized_path.starts_with("/api/admin/usage/")
        && normalized_path.ends_with("/replay")
        && normalized_path.matches('/').count() == 5
    {
        Some(classified(
            "admin_proxy",
            "usage_manage",
            "replay",
            "admin:usage",
            false,
        ))
    } else if method == http::Method::GET
        && normalized_path.starts_with("/api/admin/usage/")
        && normalized_path.matches('/').count() == 4
    {
        Some(classified(
            "admin_proxy",
            "usage_manage",
            "detail",
            "admin:usage",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/admin/stats/providers/quota-usage" | "/api/admin/stats/providers/quota-usage/"
        )
    {
        Some(classified(
            "admin_proxy",
            "stats_manage",
            "provider_quota_usage",
            "admin:stats",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/admin/stats/comparison" | "/api/admin/stats/comparison/"
        )
    {
        Some(classified(
            "admin_proxy",
            "stats_manage",
            "comparison",
            "admin:stats",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/admin/stats/errors/distribution" | "/api/admin/stats/errors/distribution/"
        )
    {
        Some(classified(
            "admin_proxy",
            "stats_manage",
            "error_distribution",
            "admin:stats",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/admin/stats/performance/percentiles"
                | "/api/admin/stats/performance/percentiles/"
        )
    {
        Some(classified(
            "admin_proxy",
            "stats_manage",
            "performance_percentiles",
            "admin:stats",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/admin/stats/performance/providers" | "/api/admin/stats/performance/providers/"
        )
    {
        Some(classified(
            "admin_proxy",
            "stats_manage",
            "provider_performance",
            "admin:stats",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/admin/stats/cost/forecast" | "/api/admin/stats/cost/forecast/"
        )
    {
        Some(classified(
            "admin_proxy",
            "stats_manage",
            "cost_forecast",
            "admin:stats",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/admin/stats/cost/savings" | "/api/admin/stats/cost/savings/"
        )
    {
        Some(classified(
            "admin_proxy",
            "stats_manage",
            "cost_savings",
            "admin:stats",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/admin/stats/leaderboard/api-keys" | "/api/admin/stats/leaderboard/api-keys/"
        )
    {
        Some(classified(
            "admin_proxy",
            "stats_manage",
            "leaderboard_api_keys",
            "admin:stats",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/admin/stats/leaderboard/models" | "/api/admin/stats/leaderboard/models/"
        )
    {
        Some(classified(
            "admin_proxy",
            "stats_manage",
            "leaderboard_models",
            "admin:stats",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/admin/stats/leaderboard/users" | "/api/admin/stats/leaderboard/users/"
        )
    {
        Some(classified(
            "admin_proxy",
            "stats_manage",
            "leaderboard_users",
            "admin:stats",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/admin/stats/time-series" | "/api/admin/stats/time-series/"
        )
    {
        Some(classified(
            "admin_proxy",
            "stats_manage",
            "time_series",
            "admin:stats",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/admin/monitoring/audit-logs" | "/api/admin/monitoring/audit-logs/"
        )
    {
        Some(classified(
            "admin_proxy",
            "monitoring",
            "audit_logs",
            "admin:monitoring",
            false,
        ))
    } else if method == http::Method::GET
        && (matches!(
            normalized_path,
            "/api/admin/monitoring/system-status"
                | "/api/admin/monitoring/system-status/"
                | "/api/admin/monitoring/suspicious-activities"
                | "/api/admin/monitoring/suspicious-activities/"
                | "/api/admin/monitoring/user-behavior"
        ) || normalized_path.starts_with("/api/admin/monitoring/user-behavior/"))
    {
        Some(classified(
            "admin_proxy",
            "monitoring",
            "user_behavior",
            "admin:monitoring",
            false,
        ))
    } else if method == http::Method::GET
        && (matches!(
            normalized_path,
            "/api/admin/monitoring/resilience-status"
                | "/api/admin/monitoring/resilience-status/"
                | "/api/admin/monitoring/resilience/circuit-history"
                | "/api/admin/monitoring/resilience/circuit-history/"
        ) || (normalized_path == "/api/admin/monitoring/resilience/error-stats"))
    {
        Some(classified(
            "admin_proxy",
            "monitoring",
            "monitoring_resilience",
            "admin:monitoring",
            false,
        ))
    } else if method == http::Method::DELETE
        && normalized_path == "/api/admin/monitoring/resilience/error-stats"
    {
        Some(classified(
            "admin_proxy",
            "monitoring",
            "monitoring_resilience",
            "admin:monitoring",
            false,
        ))
    } else if (method == http::Method::GET || method == http::Method::DELETE)
        && normalized_path.starts_with("/api/admin/monitoring/cache")
    {
        Some(classified(
            "admin_proxy",
            "monitoring",
            "monitoring_cache",
            "admin:monitoring",
            false,
        ))
    } else if method == http::Method::GET
        && normalized_path.starts_with("/api/admin/monitoring/trace/stats/provider/")
    {
        Some(classified(
            "admin_proxy",
            "monitoring",
            "trace_provider_stats",
            "admin:monitoring",
            false,
        ))
    } else if method == http::Method::GET
        && normalized_path.starts_with("/api/admin/monitoring/trace/")
        && !normalized_path.starts_with("/api/admin/monitoring/trace/stats/")
    {
        Some(classified(
            "admin_proxy",
            "monitoring",
            "trace_request",
            "admin:monitoring",
            false,
        ))
    } else {
        None
    }
}
