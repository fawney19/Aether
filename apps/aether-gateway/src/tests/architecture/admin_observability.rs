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
        "admin_stats_bad_request_response",
        "aggregate_usage_stats",
        "list_usage_for_optional_range",
        "parse_bounded_u32",
        "round_to",
        "AdminStatsTimeRange",
        "AdminStatsUsageFilter",
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

    let shared_usage_stats =
        read_workspace_file("apps/aether-gateway/src/handlers/shared/usage_stats.rs");
    assert!(
        shared_usage_stats.contains("crate::handlers::admin::observability::stats::{"),
        "handlers/shared/usage_stats.rs should depend on observability::stats directly"
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
        monitoring_mod.contains("mod responses;"),
        "handlers/admin/observability/monitoring/mod.rs should register responses as a dedicated response boundary"
    );
    assert!(
        !workspace_file_exists(
            "apps/aether-gateway/src/handlers/admin/observability/monitoring/common.rs",
        ),
        "handlers/admin/observability/monitoring/common.rs should stay removed after boundary split"
    );
}

#[test]
fn admin_stats_root_stays_thin() {
    let stats_mod =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/observability/stats/mod.rs");
    for pattern in [
        "use self::leaderboard::{",
        "use self::responses::{",
        "use self::timeseries::{",
        "enum AdminStatsComparisonType",
        "enum AdminStatsGranularity",
        "struct AdminStatsForecastPoint",
        "struct AdminStatsLeaderboardItem",
        "struct AdminStatsUserMetadata",
        "struct AdminStatsTimeSeriesBucket",
        "impl AdminStatsTimeRange {",
        "pub(crate) fn round_to(",
    ] {
        assert!(
            !stats_mod.contains(pattern),
            "handlers/admin/observability/stats/mod.rs should not own stats helper implementation {pattern}"
        );
    }
    for pattern in [
        "mod helpers;",
        "pub(crate) use self::helpers::{round_to, AdminStatsTimeRange, AdminStatsUsageFilter};",
        "pub(crate) use self::range::{list_usage_for_optional_range, parse_bounded_u32};",
        "pub(crate) use self::responses::admin_stats_bad_request_response;",
        "pub(crate) use self::timeseries::aggregate_usage_stats;",
    ] {
        assert!(
            stats_mod.contains(pattern),
            "handlers/admin/observability/stats/mod.rs should stay as a thin seam for {pattern}"
        );
    }

    let analytics_routes = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/observability/stats/analytics_routes.rs",
    );
    for pattern in [
        "use super::helpers::{",
        "use super::range::{",
        "use super::responses::{",
        "use super::timeseries::{",
    ] {
        assert!(
            analytics_routes.contains(pattern),
            "stats/analytics_routes.rs should depend on split stats boundaries via {pattern}"
        );
    }

    let cost_routes = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/observability/stats/cost_routes.rs",
    );
    for pattern in [
        "use super::helpers::{",
        "use super::range::{",
        "use super::responses::{",
        "use super::timeseries::{",
    ] {
        assert!(
            cost_routes.contains(pattern),
            "stats/cost_routes.rs should depend on split stats boundaries via {pattern}"
        );
    }

    let leaderboard_routes = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/observability/stats/leaderboard_routes.rs",
    );
    for pattern in [
        "use super::helpers::{",
        "use super::leaderboard::{",
        "use super::range::{",
        "use super::responses::{",
    ] {
        assert!(
            leaderboard_routes.contains(pattern),
            "stats/leaderboard_routes.rs should depend on split stats boundaries via {pattern}"
        );
    }

    let provider_quota_routes = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/observability/stats/provider_quota_routes.rs",
    );
    assert!(
        provider_quota_routes
            .contains("use super::responses::admin_stats_provider_quota_usage_empty_response;"),
        "stats/provider_quota_routes.rs should depend on responses boundary directly"
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
        "apps/aether-gateway/src/handlers/admin/observability/monitoring/cache_mutations.rs",
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
            monitoring_cache_mutations.contains(pattern),
            "monitoring/cache_mutations.rs should own {pattern}"
        );
    }

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
        "mod helpers;",
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

    let analytics_routes = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/observability/usage/analytics_routes.rs",
    );
    assert!(
        analytics_routes.contains("use super::analytics::{"),
        "usage/analytics_routes.rs should depend on analytics boundary directly"
    );
    assert!(
        analytics_routes.contains("use super::helpers::{"),
        "usage/analytics_routes.rs should depend on helpers boundary directly"
    );

    let summary_routes = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/observability/usage/summary_routes.rs",
    );
    assert!(
        summary_routes.contains("use super::analytics::{"),
        "usage/summary_routes.rs should depend on analytics boundary directly"
    );
    assert!(
        summary_routes.contains("use super::helpers::{"),
        "usage/summary_routes.rs should depend on helpers boundary directly"
    );

    let detail_routes = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/observability/usage/detail_routes.rs",
    );
    assert!(
        detail_routes.contains("use super::analytics::{"),
        "usage/detail_routes.rs should depend on analytics boundary directly"
    );
    assert!(
        detail_routes.contains("use super::helpers::{"),
        "usage/detail_routes.rs should depend on helpers boundary directly"
    );

    let replay =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/observability/usage/replay.rs");
    assert!(
        replay.contains("use super::helpers::{"),
        "usage/replay.rs should depend on helpers boundary directly"
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
        "apps/aether-gateway/src/handlers/admin/observability/monitoring/resilience.rs",
    );
    assert!(
        monitoring_resilience.contains("AdminMonitoringResilienceSnapshot"),
        "monitoring/resilience.rs should keep resilience snapshot ownership locally"
    );
    assert!(
        monitoring_resilience.contains("struct AdminMonitoringResilienceSnapshot"),
        "monitoring/resilience.rs should define AdminMonitoringResilienceSnapshot locally"
    );

    let data_system = read_workspace_file("crates/aether-data/src/repository/system.rs");
    assert!(
        !data_system.contains("AdminMonitoringCacheSnapshot")
            && !data_system.contains("AdminMonitoringResilienceSnapshot"),
        "monitoring snapshots are admin view DTOs and should not move into aether-data"
    );
}
