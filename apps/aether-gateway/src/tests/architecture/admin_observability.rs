use super::*;

#[test]
fn non_admin_handlers_do_not_depend_on_admin_stats_module() {
    let handlers_mod = read_workspace_file("apps/aether-gateway/src/handlers/mod.rs");
    assert!(
        !handlers_mod.contains("pub(crate) use admin::{"),
        "handlers/mod.rs should stay as pure module wiring after shared usage stats facade extraction"
    );

    for path in [
        "apps/aether-gateway/src/handlers/public/support/user_me.rs",
        "apps/aether-gateway/src/handlers/public/support/wallet/reads.rs",
        "apps/aether-gateway/src/handlers/admin/observability/monitoring/cache_store.rs",
    ] {
        let file = read_workspace_file(path);
        assert!(
            !file.contains("handlers::admin::stats::"),
            "{path} should not depend directly on admin::stats"
        );
    }

    let admin_mod = read_workspace_file("apps/aether-gateway/src/handlers/admin/mod.rs");
    assert!(
        !admin_mod.contains("pub(crate) mod facade;"),
        "handlers/admin/mod.rs should not keep admin facade after direct subdomain exposure"
    );

    let shared_mod = read_workspace_file("apps/aether-gateway/src/handlers/shared/mod.rs");
    for pattern in [
        "admin_stats_bad_request_response",
        "list_usage_for_optional_range",
        "parse_bounded_u32",
        "round_to",
        "AdminStatsTimeRange",
        "AdminStatsUsageFilter",
    ] {
        assert!(
            shared_mod.contains(pattern),
            "handlers/shared/mod.rs should expose shared usage stats helper {pattern}"
        );
    }

    let admin_observability_mod =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/observability/mod.rs");
    for pattern in [
        "aggregate_usage_stats",
        "match_admin_monitoring_route",
        "AdminMonitoringRoute",
        "ADMIN_MONITORING_REDIS_REQUIRED_DETAIL",
        "test_support",
    ] {
        assert!(
            !admin_observability_mod.contains(pattern),
            "handlers/admin/observability/mod.rs should not re-export {pattern}"
        );
    }
    for pattern in [
        "admin_stats_bad_request_response",
        "list_usage_for_optional_range",
        "parse_bounded_u32",
        "round_to",
        "AdminStatsTimeRange",
        "AdminStatsUsageFilter",
    ] {
        assert!(
            admin_observability_mod.contains(pattern),
            "handlers/admin/observability/mod.rs should expose admin stats facade helper {pattern}"
        );
    }

    let shared_usage_stats =
        read_workspace_file("apps/aether-gateway/src/handlers/shared/usage_stats.rs");
    assert!(
        shared_usage_stats.contains("crate::admin_api::{"),
        "handlers/shared/usage_stats.rs should depend on crate::admin_api facade directly"
    );
}

#[test]
fn admin_monitoring_root_stays_thin() {
    let monitoring_mod = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/observability/monitoring/mod.rs",
    );
    for pattern in [
        "mod common;",
        "use self::activity::{",
        "use self::cache::{",
        "use self::resilience::{",
        "use self::trace::{",
        "pub(crate) use self::routes::{",
        "const ADMIN_MONITORING_",
    ] {
        assert!(
            !monitoring_mod.contains(pattern),
            "handlers/admin/observability/monitoring/mod.rs should not act as a glue re-export layer for {pattern}"
        );
    }

    assert!(
        monitoring_mod.contains("routes::maybe_build_local_admin_monitoring_response"),
        "handlers/admin/observability/monitoring/mod.rs should delegate through routes module"
    );
    assert!(
        monitoring_mod.contains("mod cache_config;"),
        "handlers/admin/observability/monitoring/mod.rs should register cache_config as a dedicated cache boundary"
    );
    assert!(
        monitoring_mod.contains("mod cache_mutations;"),
        "handlers/admin/observability/monitoring/mod.rs should register cache_mutations as a dedicated mutation boundary"
    );
    assert!(
        !monitoring_mod.contains("mod responses;"),
        "handlers/admin/observability/monitoring/mod.rs should not keep local monitoring responses after crate split"
    );
    assert!(
        !workspace_file_exists(
            "apps/aether-gateway/src/handlers/admin/observability/monitoring/common.rs",
        ),
        "handlers/admin/observability/monitoring/common.rs should stay removed after boundary split"
    );
    assert!(
        !workspace_file_exists(
            "apps/aether-gateway/src/handlers/admin/observability/monitoring/responses.rs",
        ),
        "handlers/admin/observability/monitoring/responses.rs should stay removed after crate split"
    );

    let monitoring_routes = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/observability/monitoring/routes.rs",
    );
    for pattern in [
        "use aether_admin::observability::monitoring::{",
        "match_admin_monitoring_route",
        "AdminMonitoringRoute",
    ] {
        assert!(
            monitoring_routes.contains(pattern),
            "handlers/admin/observability/monitoring/routes.rs should depend on crate-owned monitoring seam {pattern}"
        );
    }
    for pattern in [
        "pub(crate) enum AdminMonitoringRoute",
        "pub(crate) fn match_admin_monitoring_route(",
    ] {
        assert!(
            !monitoring_routes.contains(pattern),
            "handlers/admin/observability/monitoring/routes.rs should not keep local monitoring route matcher {pattern}"
        );
    }
}

#[test]
fn admin_stats_root_stays_thin() {
    let stats_mod =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/observability/stats/mod.rs");
    for pattern in [
        "use self::leaderboard::{",
        "enum AdminStatsComparisonType",
        "enum AdminStatsGranularity",
        "struct AdminStatsForecastPoint",
        "struct AdminStatsLeaderboardItem",
        "struct AdminStatsUserMetadata",
        "struct AdminStatsTimeSeriesBucket",
        "impl AdminStatsTimeRange {",
        "pub(crate) fn round_to(",
        "mod helpers;",
        "mod responses;",
        "mod timeseries;",
        "pub(crate) use self::helpers::{round_to, AdminStatsTimeRange, AdminStatsUsageFilter};",
        "pub(crate) use self::responses::admin_stats_bad_request_response;",
        "pub(crate) use self::timeseries::aggregate_usage_stats;",
    ] {
        assert!(
            !stats_mod.contains(pattern),
            "handlers/admin/observability/stats/mod.rs should not own stats helper implementation {pattern}"
        );
    }
    for pattern in [
        "pub(crate) use aether_admin::observability::stats::{",
        "admin_stats_bad_request_response",
        "aggregate_usage_stats",
        "round_to",
        "AdminStatsTimeRange",
        "AdminStatsUsageFilter",
    ] {
        assert!(
            stats_mod.contains(pattern),
            "handlers/admin/observability/stats/mod.rs should stay as a thin seam for {pattern}"
        );
    }
    assert!(
        stats_mod.contains("pub(crate) use self::range::{"),
        "handlers/admin/observability/stats/mod.rs should re-export the split range seam"
    );
    for pattern in [
        "list_usage_for_optional_range",
        "list_usage_for_range",
        "parse_bounded_u32",
    ] {
        assert!(
            stats_mod.contains(pattern),
            "handlers/admin/observability/stats/mod.rs should keep range re-export {pattern}"
        );
    }

    let analytics_routes = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/observability/stats/analytics_routes.rs",
    );
    for pattern in [
        "use aether_admin::observability::stats::{",
        "AdminStatsComparisonType",
        "build_admin_stats_comparison_response",
    ] {
        assert!(
            analytics_routes.contains(pattern),
            "stats/analytics_routes.rs should depend on crate-owned stats helper {pattern}"
        );
    }
    assert!(
        !analytics_routes.contains("use super::helpers::{")
            && !analytics_routes.contains("use super::responses::{")
            && !analytics_routes.contains("use super::timeseries::{"),
        "stats/analytics_routes.rs should no longer depend on local stats pure bridges"
    );
    assert!(
        analytics_routes.contains("use super::range::{")
            || analytics_routes.contains("use super::range::build_comparison_range;"),
        "stats/analytics_routes.rs should depend on the split stats range seam"
    );

    let cost_routes = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/observability/stats/cost_routes.rs",
    );
    for pattern in [
        "use super::range::{",
        "use aether_admin::observability::stats::{",
        "build_admin_stats_cost_forecast_response",
    ] {
        assert!(
            cost_routes.contains(pattern),
            "stats/cost_routes.rs should depend on crate-owned stats helper {pattern}"
        );
    }
    assert!(
        !cost_routes.contains("use super::helpers::{")
            && !cost_routes.contains("use super::responses::{")
            && !cost_routes.contains("use super::timeseries::{"),
        "stats/cost_routes.rs should no longer depend on local stats pure bridges"
    );

    let leaderboard_routes = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/observability/stats/leaderboard_routes.rs",
    );
    for pattern in [
        "use super::leaderboard::{",
        "use super::range::{",
        "use aether_admin::observability::stats::{",
        "admin_stats_leaderboard_empty_response",
    ] {
        assert!(
            leaderboard_routes.contains(pattern),
            "stats/leaderboard_routes.rs should depend on explicit local/crate owner {pattern}"
        );
    }
    assert!(
        !leaderboard_routes.contains("use super::helpers::{")
            && !leaderboard_routes.contains("use super::responses::{"),
        "stats/leaderboard_routes.rs should no longer depend on local stats pure bridges"
    );

    let provider_quota_routes = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/observability/stats/provider_quota_routes.rs",
    );
    assert!(
        provider_quota_routes.contains("use aether_admin::observability::stats::{")
            || provider_quota_routes.contains(
                "use aether_admin::observability::stats::admin_stats_provider_quota_usage_empty_response;",
            ),
        "stats/provider_quota_routes.rs should depend on crate-owned responses directly"
    );
}

#[test]
fn admin_monitoring_cache_mutations_are_split_from_reads() {
    let monitoring_cache = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/observability/monitoring/cache.rs",
    );
    for pattern in [
        "pub(super) async fn build_admin_monitoring_cache_users_delete_response(",
        "pub(super) async fn build_admin_monitoring_cache_affinity_delete_response(",
        "pub(super) async fn build_admin_monitoring_cache_flush_response(",
        "pub(super) async fn build_admin_monitoring_cache_provider_delete_response(",
        "pub(super) async fn build_admin_monitoring_model_mapping_delete_response(",
        "pub(super) async fn build_admin_monitoring_model_mapping_delete_model_response(",
        "pub(super) async fn build_admin_monitoring_model_mapping_delete_provider_response(",
        "pub(super) async fn build_admin_monitoring_redis_keys_delete_response(",
    ] {
        assert!(
            !monitoring_cache.contains(pattern),
            "monitoring/cache.rs should stay focused on read/report handlers, not {pattern}"
        );
    }

    let monitoring_cache_mutations = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/observability/monitoring/cache_mutations/mod.rs",
    );
    for pattern in [
        "mod users;",
        "mod affinity;",
        "mod flush;",
        "mod provider;",
        "mod model_mapping;",
        "mod redis_keys;",
        "pub(super) use users::build_admin_monitoring_cache_users_delete_response;",
        "pub(super) use affinity::build_admin_monitoring_cache_affinity_delete_response;",
        "pub(super) use flush::build_admin_monitoring_cache_flush_response;",
        "pub(super) use provider::build_admin_monitoring_cache_provider_delete_response;",
        "pub(super) use model_mapping::{",
        "pub(super) use redis_keys::build_admin_monitoring_redis_keys_delete_response;",
    ] {
        assert!(
            monitoring_cache_mutations.contains(pattern),
            "monitoring/cache_mutations/mod.rs should own {pattern}"
        );
    }
    assert!(
        !workspace_file_exists(
            "apps/aether-gateway/src/handlers/admin/observability/monitoring/cache_mutations.rs"
        ),
        "monitoring/cache_mutations.rs should stay removed after directory split"
    );

    let monitoring_routes = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/observability/monitoring/routes.rs",
    );
    assert!(
        monitoring_routes.contains("use super::cache_mutations::{"),
        "monitoring/routes.rs should depend on cache_mutations directly for delete handlers"
    );
    assert!(
        monitoring_routes.contains("use super::cache_affinity_reads::{"),
        "monitoring/routes.rs should depend on cache_affinity_reads directly for affinity read handlers"
    );
    assert!(
        monitoring_routes.contains("use super::cache_model_mapping::{"),
        "monitoring/routes.rs should depend on cache_model_mapping directly for model-mapping read handlers"
    );

    let monitoring_mod = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/observability/monitoring/mod.rs",
    );
    for pattern in ["mod cache_affinity_reads;", "mod cache_model_mapping;"] {
        assert!(
            monitoring_mod.contains(pattern),
            "monitoring/mod.rs should register split read module {pattern}"
        );
    }

    let monitoring_affinity_reads = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/observability/monitoring/cache_affinity_reads.rs",
    );
    for pattern in [
        "pub(super) async fn build_admin_monitoring_cache_affinities_response(",
        "pub(super) async fn build_admin_monitoring_cache_affinity_response(",
    ] {
        assert!(
            monitoring_affinity_reads.contains(pattern),
            "monitoring/cache_affinity_reads.rs should own {pattern}"
        );
    }

    let monitoring_model_mapping = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/observability/monitoring/cache_model_mapping.rs",
    );
    for pattern in [
        "pub(super) async fn build_admin_monitoring_model_mapping_stats_response(",
        "pub(super) async fn build_admin_monitoring_redis_cache_categories_response(",
    ] {
        assert!(
            monitoring_model_mapping.contains(pattern),
            "monitoring/cache_model_mapping.rs should own {pattern}"
        );
    }
}

#[test]
fn admin_monitoring_route_payloads_prefer_crate_owned_builders() {
    for (path, pattern) in [
        (
            "apps/aether-gateway/src/handlers/admin/observability/monitoring/activity.rs",
            "build_admin_monitoring_system_status_payload_response",
        ),
        (
            "apps/aether-gateway/src/handlers/admin/observability/monitoring/trace.rs",
            "build_admin_monitoring_trace_provider_stats_payload_response",
        ),
        (
            "apps/aether-gateway/src/handlers/admin/observability/monitoring/resilience/history.rs",
            "build_admin_monitoring_circuit_history_payload_response",
        ),
        (
            "apps/aether-gateway/src/handlers/admin/observability/monitoring/resilience/status.rs",
            "build_admin_monitoring_resilience_status_payload_response",
        ),
        (
            "apps/aether-gateway/src/handlers/admin/observability/monitoring/resilience/reset.rs",
            "build_admin_monitoring_reset_error_stats_payload_response",
        ),
        (
            "apps/aether-gateway/src/handlers/admin/observability/monitoring/cache_mutations/flush.rs",
            "build_admin_monitoring_cache_flush_success_response",
        ),
        (
            "apps/aether-gateway/src/handlers/admin/observability/monitoring/cache_mutations/provider.rs",
            "admin_monitoring_cache_provider_not_found_response",
        ),
        (
            "apps/aether-gateway/src/handlers/admin/observability/monitoring/cache_mutations/model_mapping.rs",
            "build_admin_monitoring_model_mapping_delete_success_response",
        ),
        (
            "apps/aether-gateway/src/handlers/admin/observability/monitoring/cache_mutations/redis_keys.rs",
            "build_admin_monitoring_redis_keys_delete_success_response",
        ),
    ] {
        let file = read_workspace_file(path);
        assert!(
            file.contains("use aether_admin::observability::monitoring::{")
                || file.contains("use aether_admin::observability::monitoring::"),
            "{path} should depend on aether_admin::observability::monitoring"
        );
        assert!(
            file.contains(pattern),
            "{path} should route payload building through crate-owned helper {pattern}"
        );
    }
}

#[test]
fn admin_usage_root_stays_thin() {
    let usage_mod =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/observability/usage/mod.rs");
    for pattern in ["pub(crate) use analytics::{", "pub(crate) use helpers::{"] {
        assert!(
            !usage_mod.contains(pattern),
            "handlers/admin/observability/usage/mod.rs should not re-export helper seam {pattern}"
        );
    }
    for pattern in [
        "mod analytics;",
        "mod analytics_routes;",
        "mod detail_routes;",
        "mod replay;",
        "mod summary_routes;",
        "detail_routes::maybe_build_local_admin_usage_detail_response",
        "summary_routes::maybe_build_local_admin_usage_summary_response",
        "analytics_routes::maybe_build_local_admin_usage_analytics_response",
    ] {
        assert!(
            usage_mod.contains(pattern),
            "handlers/admin/observability/usage/mod.rs should stay as a thin router for {pattern}"
        );
    }

    let analytics_mod = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/observability/usage/analytics/mod.rs",
    );
    for pattern in [
        "mod aggregations;",
        "mod cache_affinity;",
        "mod filters;",
        "pub(super) use aggregations::admin_usage_aggregation_by_user_json;",
        "pub(super) use cache_affinity::list_recent_completed_usage_for_cache_affinity;",
        "pub(super) use filters::admin_usage_provider_key_names;",
    ] {
        assert!(
            analytics_mod.contains(pattern),
            "usage/analytics/mod.rs should keep explicit analytics owner seam {pattern}"
        );
    }
    for pattern in [
        "mod parse;",
        "admin_usage_aggregation_by_model_json",
        "admin_usage_parse_limit",
        "admin_usage_matches_search",
    ] {
        assert!(
            !analytics_mod.contains(pattern),
            "usage/analytics/mod.rs should not keep pure crate forwarders {pattern}"
        );
    }
    assert!(
        !workspace_file_exists(
            "apps/aether-gateway/src/handlers/admin/observability/usage/analytics.rs"
        ),
        "usage/analytics.rs should be removed once analytics is directoryized"
    );

    let analytics_routes = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/observability/usage/analytics_routes/mod.rs",
    );
    assert!(
        analytics_routes.contains("mod aggregation;"),
        "usage/analytics_routes/mod.rs should register aggregation owner"
    );
    assert!(
        analytics_routes.contains("mod cache_affinity_hit_analysis;"),
        "usage/analytics_routes/mod.rs should register cache_affinity_hit_analysis owner"
    );
    assert!(
        analytics_routes.contains("mod cache_affinity_interval_timeline;"),
        "usage/analytics_routes/mod.rs should register cache_affinity_interval_timeline owner"
    );
    assert!(
        analytics_routes.contains("mod cache_affinity_ttl_analysis;"),
        "usage/analytics_routes/mod.rs should register cache_affinity_ttl_analysis owner"
    );
    assert!(
        analytics_routes.contains("mod heatmap;"),
        "usage/analytics_routes/mod.rs should register heatmap owner"
    );
    assert!(
        analytics_routes.contains("aggregation::build_admin_usage_aggregation_stats_response"),
        "usage/analytics_routes/mod.rs should delegate aggregation handling to aggregation owner"
    );
    assert!(
        !workspace_file_exists(
            "apps/aether-gateway/src/handlers/admin/observability/usage/analytics_routes.rs"
        ),
        "usage/analytics_routes.rs should stay removed after directory split"
    );

    let summary_routes = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/observability/usage/summary_routes.rs",
    );
    assert!(
        summary_routes.contains("use super::analytics::admin_usage_provider_key_names;"),
        "usage/summary_routes.rs should keep only the local stateful analytics lookup"
    );
    assert!(
        summary_routes.contains("use aether_admin::observability::usage::{"),
        "usage/summary_routes.rs should depend on crate-owned usage helpers directly"
    );

    let detail_routes = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/observability/usage/detail_routes.rs",
    );
    assert!(
        detail_routes.contains("use super::analytics::admin_usage_provider_key_names;"),
        "usage/detail_routes.rs should keep only the local stateful analytics lookup"
    );
    assert!(
        detail_routes.contains("use aether_admin::observability::usage::{"),
        "usage/detail_routes.rs should depend on crate-owned usage helpers directly"
    );

    let replay =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/observability/usage/replay.rs");
    assert!(
        replay.contains("use aether_admin::observability::usage::{"),
        "usage/replay.rs should depend on crate-owned usage helpers directly"
    );

    let analytics_aggregations = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/observability/usage/analytics/aggregations.rs",
    );
    assert!(
        analytics_aggregations
            .contains("pub(in super::super) async fn admin_usage_aggregation_by_user_json("),
        "usage/analytics/aggregations.rs should keep only the local user aggregation owner"
    );
    let analytics_cache_affinity = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/observability/usage/analytics/cache_affinity.rs",
    );
    assert!(
        analytics_cache_affinity.contains(
            "pub(in super::super) async fn list_recent_completed_usage_for_cache_affinity("
        ),
        "usage/analytics/cache_affinity.rs should own cache-affinity analytics helpers"
    );
    let analytics_filters = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/observability/usage/analytics/filters.rs",
    );
    assert!(
        analytics_filters.contains("pub(in super::super) async fn admin_usage_provider_key_names("),
        "usage/analytics/filters.rs should keep only the local provider key lookup"
    );
    assert!(
        !workspace_file_exists(
            "apps/aether-gateway/src/handlers/admin/observability/usage/helpers.rs"
        ),
        "usage/helpers.rs should stay removed after crate split"
    );
    assert!(
        !workspace_file_exists(
            "apps/aether-gateway/src/handlers/admin/observability/usage/analytics/parse.rs"
        ),
        "usage/analytics/parse.rs should stay removed after crate split"
    );
}

#[test]
fn admin_monitoring_snapshots_stay_app_local() {
    let monitoring_cache_types = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/observability/monitoring/cache_types.rs",
    );
    for pattern in [
        "pub(super) struct AdminMonitoringCacheSnapshot",
        "pub(super) struct AdminMonitoringCacheAffinityRecord",
    ] {
        assert!(
            monitoring_cache_types.contains(pattern),
            "monitoring/cache_types.rs should own {pattern}"
        );
    }

    let monitoring_resilience = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/observability/monitoring/resilience/mod.rs",
    );
    assert!(
        monitoring_resilience.contains("mod history;"),
        "monitoring/resilience/mod.rs should register history owner"
    );
    assert!(
        monitoring_resilience.contains("mod snapshot;"),
        "monitoring/resilience/mod.rs should register snapshot owner"
    );
    assert!(
        monitoring_resilience.contains("mod status;"),
        "monitoring/resilience/mod.rs should register status owner"
    );
    assert!(
        monitoring_resilience.contains("mod reset;"),
        "monitoring/resilience/mod.rs should register reset owner"
    );
    assert!(
        !workspace_file_exists(
            "apps/aether-gateway/src/handlers/admin/observability/monitoring/resilience.rs"
        ),
        "monitoring/resilience.rs should stay removed after directory split"
    );

    let resilience_snapshot = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/observability/monitoring/resilience/snapshot.rs",
    );
    assert!(
        resilience_snapshot.contains("AdminMonitoringResilienceSnapshot"),
        "monitoring/resilience/snapshot.rs should keep resilience snapshot ownership locally"
    );
    assert!(
        resilience_snapshot.contains("pub(super) struct AdminMonitoringResilienceSnapshot"),
        "monitoring/resilience/snapshot.rs should define AdminMonitoringResilienceSnapshot locally"
    );

    let data_system = read_workspace_file("crates/aether-data/src/repository/system.rs");
    assert!(
        !data_system.contains("AdminMonitoringCacheSnapshot")
            && !data_system.contains("AdminMonitoringResilienceSnapshot"),
        "monitoring snapshots are admin view DTOs and should not move into aether-data"
    );
}
