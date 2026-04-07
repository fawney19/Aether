use std::fs;
use std::path::{Path, PathBuf};

fn collect_rust_files(root: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(root).expect("directory should be readable") {
        let entry = entry.expect("directory entry should be readable");
        let path = entry.path();
        if path.is_dir() {
            collect_rust_files(&path, files);
            continue;
        }
        if path.extension().and_then(|value| value.to_str()) == Some("rs") {
            files.push(path);
        }
    }
}

fn assert_no_sqlx_queries(root_relative_path: &str) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join(root_relative_path);
    let mut files = Vec::new();
    collect_rust_files(&root, &mut files);

    let patterns = [
        "sqlx::query(",
        "sqlx::query_scalar",
        "query_scalar::<",
        "QueryBuilder<",
    ];
    let violations = files
        .into_iter()
        .filter_map(|path| {
            let source = fs::read_to_string(&path).expect("source file should be readable");
            let hits = patterns
                .iter()
                .filter(|pattern| source.contains(**pattern))
                .copied()
                .collect::<Vec<_>>();
            if hits.is_empty() {
                None
            } else {
                Some(format!("{} -> {}", path.display(), hits.join(", ")))
            }
        })
        .collect::<Vec<_>>();

    assert!(
        violations.is_empty(),
        "disallowed SQL ownership violations:\n{}",
        violations.join("\n")
    );
}

fn assert_no_sensitive_log_patterns(root_relative_path: &str, patterns: &[&str]) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join(root_relative_path);
    let mut files = Vec::new();
    collect_rust_files(&root, &mut files);

    let violations = files
        .into_iter()
        .filter_map(|path| {
            let source = fs::read_to_string(&path).expect("source file should be readable");
            let hits = patterns
                .iter()
                .filter(|pattern| source.contains(**pattern))
                .copied()
                .collect::<Vec<_>>();
            if hits.is_empty() {
                None
            } else {
                Some(format!("{} -> {}", path.display(), hits.join(", ")))
            }
        })
        .collect::<Vec<_>>();

    assert!(
        violations.is_empty(),
        "disallowed sensitive logging patterns:\n{}",
        violations.join("\n")
    );
}

fn assert_no_module_dependency_patterns(root_relative_path: &str, patterns: &[&str]) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join(root_relative_path);
    let mut files = Vec::new();
    collect_rust_files(&root, &mut files);

    let violations = files
        .into_iter()
        .filter_map(|path| {
            let source = fs::read_to_string(&path).expect("source file should be readable");
            let hits = patterns
                .iter()
                .filter(|pattern| source.contains(**pattern))
                .copied()
                .collect::<Vec<_>>();
            if hits.is_empty() {
                None
            } else {
                Some(format!("{} -> {}", path.display(), hits.join(", ")))
            }
        })
        .collect::<Vec<_>>();

    assert!(
        violations.is_empty(),
        "disallowed module dependency patterns:\n{}",
        violations.join("\n")
    );
}

fn workspace_file_exists(root_relative_path: &str) -> bool {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(root_relative_path)
        .exists()
}

fn read_workspace_file(path: &str) -> String {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root should resolve");
    fs::read_to_string(workspace_root.join(path)).expect("source file should be readable")
}

#[test]
fn handlers_do_not_inline_sql_queries() {
    assert_no_sqlx_queries("src/handlers");
}

#[test]
fn gateway_runtime_does_not_inline_sql_queries() {
    assert_no_sqlx_queries("src/state/runtime");
}

#[test]
fn wallet_repository_does_not_reexport_settlement_types() {
    let wallet_mod = read_workspace_file("crates/aether-data/src/repository/wallet/mod.rs");
    let wallet_types = read_workspace_file("crates/aether-data/src/repository/wallet/types.rs");
    let wallet_sql = read_workspace_file("crates/aether-data/src/repository/wallet/sql.rs");
    let wallet_memory = read_workspace_file("crates/aether-data/src/repository/wallet/memory.rs");

    assert!(
        !wallet_mod.contains("StoredUsageSettlement"),
        "wallet/mod.rs should not export StoredUsageSettlement"
    );
    assert!(
        !wallet_mod.contains("UsageSettlementInput"),
        "wallet/mod.rs should not export UsageSettlementInput"
    );
    assert!(
        !wallet_types.contains("pub use crate::repository::settlement"),
        "wallet/types.rs should not re-export settlement types"
    );
    assert!(
        !wallet_types.contains("async fn settle_usage("),
        "wallet/types.rs should not own settlement entrypoints"
    );
    assert!(
        !wallet_sql.contains("impl SettlementWriteRepository"),
        "wallet/sql.rs should not implement SettlementWriteRepository"
    );
    assert!(
        !wallet_memory.contains("impl SettlementWriteRepository"),
        "wallet/memory.rs should not implement SettlementWriteRepository"
    );
}

#[test]
fn usage_runtime_paths_depend_on_shared_crates_not_app_runtime_shims() {
    for path in [
        "apps/aether-gateway/src/usage/runtime.rs",
        "apps/aether-gateway/src/usage/worker.rs",
        "apps/aether-gateway/src/async_task/runtime.rs",
    ] {
        let source = read_workspace_file(path);
        assert!(
            source.contains("aether_usage_runtime"),
            "{path} should depend on aether_usage_runtime"
        );
        assert!(
            !source.contains("wallet_runtime"),
            "{path} should not depend on wallet_runtime"
        );
    }

    {
        let path = "apps/aether-gateway/src/async_task/runtime.rs";
        let source = read_workspace_file(path);
        assert!(
            source.contains("aether_billing"),
            "{path} should depend on aether_billing"
        );
        assert!(
            !source.contains("billing_runtime::enrich_usage_event_with_billing"),
            "{path} should not depend on billing_runtime compat re-export"
        );
        assert!(
            !source.contains("settlement_runtime::settle_usage_if_needed"),
            "{path} should not depend on settlement_runtime compat re-export"
        );
    }

    let usage_runtime = read_workspace_file("apps/aether-gateway/src/usage/runtime.rs");
    assert!(
        !usage_runtime.contains("GatewayDataState"),
        "usage/runtime.rs should not own GatewayDataState integration impls anymore"
    );
    assert!(
        !usage_runtime.contains("UsageBillingEventEnricher"),
        "usage/runtime.rs should not own UsageBillingEventEnricher impl anymore"
    );
    assert!(
        !usage_runtime.contains("UsageRuntimeAccess"),
        "usage/runtime.rs should not own UsageRuntimeAccess impl anymore"
    );

    let usage_worker = read_workspace_file("apps/aether-gateway/src/usage/worker.rs");
    let runtime_usage_worker = usage_worker
        .split("#[cfg(test)]")
        .next()
        .unwrap_or(usage_worker.as_str());
    assert!(
        !runtime_usage_worker.contains("GatewayDataState"),
        "usage/worker.rs runtime path should not own GatewayDataState integration impls anymore"
    );
    assert!(
        !runtime_usage_worker.contains("UsageRecordWriter"),
        "usage/worker.rs runtime path should not own UsageRecordWriter impl anymore"
    );

    let integrations = read_workspace_file("apps/aether-gateway/src/data/state/integrations.rs");
    for pattern in [
        "UsageBillingEventEnricher for GatewayDataState",
        "UsageRuntimeAccess for GatewayDataState",
        "UsageRecordWriter for GatewayDataState",
        "UsageSettlementWriter for GatewayDataState",
    ] {
        assert!(
            integrations.contains(pattern),
            "data/state/integrations.rs should centralize {pattern}"
        );
    }

    let usage_reporting_context =
        read_workspace_file("apps/aether-gateway/src/usage/reporting/context.rs");
    assert!(
        usage_reporting_context.contains("aether_usage_runtime"),
        "usage/reporting/context.rs should depend on aether_usage_runtime"
    );
    assert!(
        usage_reporting_context.contains("resolve_video_task_report_lookup"),
        "usage/reporting/context.rs should depend on shared video task report lookup helper"
    );
    for pattern in [
        "build_locally_actionable_report_context_from_video_task",
        "report_context_is_locally_actionable",
    ] {
        assert!(
            usage_reporting_context.contains(pattern),
            "usage/reporting/context.rs should depend on shared usage helper {pattern}"
        );
    }
    assert!(
        usage_reporting_context.contains("VideoTaskReportLookup::TaskIdOrExternal"),
        "usage/reporting/context.rs should keep app-local external task fallback orchestration"
    );
    for pattern in [
        "build_locally_actionable_report_context_from_request_candidate",
        "read_request_candidates_by_request_id(",
        "resolve_locally_actionable_report_context_from_request_candidates(",
    ] {
        assert!(
            !usage_reporting_context.contains(pattern),
            "usage/reporting/context.rs should not own request-candidate resolver details {pattern}"
        );
    }
    for pattern in [
        "context\n        .get(\"local_task_id\")",
        "context\n        .get(\"local_short_id\")",
        "context\n        .get(\"task_id\")",
        "VideoTaskLookupKey::ShortId(short_id)",
        "fn insert_missing_string_value(",
        "fn insert_missing_optional_string_value(",
        "fn has_non_empty_str(",
        "fn has_u64(",
    ] {
        assert!(
            !usage_reporting_context.contains(pattern),
            "usage/reporting/context.rs should not own video task report lookup parsing {pattern}"
        );
    }

    let usage_reporting_mod = read_workspace_file("apps/aether-gateway/src/usage/reporting/mod.rs");
    assert!(
        usage_reporting_mod.contains("aether_usage_runtime"),
        "usage/reporting/mod.rs should depend on aether_usage_runtime"
    );
    for pattern in [
        "is_local_ai_sync_report_kind",
        "is_local_ai_stream_report_kind",
        "sync_report_represents_failure",
        "extract_gemini_file_mapping_entries",
        "gemini_file_mapping_cache_key",
        "normalize_gemini_file_name",
        "report_request_id",
        "should_handle_local_sync_report",
        "should_handle_local_stream_report",
        "GEMINI_FILE_MAPPING_TTL_SECONDS",
    ] {
        assert!(
            usage_reporting_mod.contains(pattern),
            "usage/reporting/mod.rs should depend on shared usage helper {pattern}"
        );
    }
    for pattern in [
        "fn is_local_ai_sync_report_kind(",
        "fn is_local_ai_stream_report_kind(",
        "fn sync_report_represents_failure(",
        "fn extract_gemini_file_mapping_entries(",
        "fn maybe_push_local_gemini_file_mapping_entry(",
        "fn extract_sync_report_body_json(",
        "fn content_type_starts_with(",
        "fn normalize_file_name(",
        "const GEMINI_FILE_MAPPING_TTL_SECONDS",
        "const GEMINI_FILE_MAPPING_CACHE_PREFIX",
        "fn gemini_file_mapping_cache_key(",
        "fn report_request_id(",
        "fn should_handle_local_sync_report(",
        "fn should_handle_local_stream_report(",
        "\"openai_video_delete_sync_success\" && payload.status_code == 404",
    ] {
        assert!(
            !usage_reporting_mod.contains(pattern),
            "usage/reporting/mod.rs should not own local report classification logic {pattern}"
        );
    }
}

#[test]
fn gateway_system_config_types_are_owned_by_aether_data() {
    let state_mod = read_workspace_file("apps/aether-gateway/src/data/state/mod.rs");
    assert!(
        state_mod.contains("aether_data::repository::system"),
        "data/state/mod.rs should depend on aether-data system types"
    );
    assert!(
        !state_mod.contains("pub(crate) struct StoredSystemConfigEntry"),
        "data/state/mod.rs should not define StoredSystemConfigEntry locally"
    );

    let state_core = read_workspace_file("apps/aether-gateway/src/data/state/core.rs");
    for pattern in [
        "backend.list_system_config_entries().await",
        "upsert_system_config_entry(key, value, description)",
        "AdminSystemStats::default()",
    ] {
        assert!(
            state_core.contains(pattern),
            "data/state/core.rs should use shared system DTO path {pattern}"
        );
    }
    for pattern in [
        "|(key, value, description, updated_at_unix_secs)|",
        "Ok((0, 0, 0, 0))",
    ] {
        assert!(
            !state_core.contains(pattern),
            "data/state/core.rs should not own local system DTO projection {pattern}"
        );
    }

    let system_types = read_workspace_file("crates/aether-data/src/repository/system.rs");
    for pattern in [
        "pub struct StoredSystemConfigEntry",
        "pub struct AdminSystemStats",
        "pub struct AdminSecurityBlacklistEntry",
    ] {
        assert!(
            system_types.contains(pattern),
            "aether-data system module should own {pattern}"
        );
    }

    let admin_types = read_workspace_file("apps/aether-gateway/src/state/admin_types.rs");
    assert!(
        admin_types.contains("aether_data::repository::system::AdminSecurityBlacklistEntry"),
        "state/admin_types.rs should re-export AdminSecurityBlacklistEntry from aether-data"
    );
    assert!(
        !admin_types.contains("struct AdminSecurityBlacklistEntry"),
        "state/admin_types.rs should not define AdminSecurityBlacklistEntry locally"
    );

    let runtime_mod = read_workspace_file("apps/aether-gateway/src/state/runtime/mod.rs");
    assert!(
        !runtime_mod.contains("AdminSecurityBlacklistEntryPayload"),
        "state/runtime/mod.rs should not keep the unused blacklist payload wrapper"
    );
}

#[test]
fn gateway_auth_snapshot_type_is_owned_by_aether_data() {
    let gateway_auth = read_workspace_file("apps/aether-gateway/src/data/auth.rs");
    let runtime_mod = read_workspace_file("apps/aether-gateway/src/state/runtime/mod.rs");
    let auth_api_keys =
        read_workspace_file("apps/aether-gateway/src/state/runtime/auth/api_keys.rs");
    assert!(
        gateway_auth.contains("aether_data::repository::auth"),
        "data/auth.rs should depend on aether-data auth snapshot types"
    );
    assert!(
        gateway_auth.contains("ResolvedAuthApiKeySnapshot as GatewayAuthApiKeySnapshot"),
        "data/auth.rs should expose the shared resolved auth snapshot type under the gateway-facing name"
    );
    for pattern in [
        "pub(crate) struct GatewayAuthApiKeySnapshot",
        "pub(crate) async fn read_auth_api_key_snapshot(",
        "pub(crate) async fn read_auth_api_key_snapshot_by_key_hash(",
        "fn effective_allowed_providers(",
        "fn effective_allowed_api_formats(",
        "fn effective_allowed_models(",
    ] {
        assert!(
            !gateway_auth.contains(pattern),
            "data/auth.rs should not own local auth snapshot logic {pattern}"
        );
    }
    for pattern in [
        "pub(crate) async fn read_auth_api_key_snapshot(",
        "pub(crate) async fn read_auth_api_key_snapshots_by_ids(",
    ] {
        assert!(
            !auth_api_keys.contains(pattern),
            "state/runtime/auth/api_keys.rs should not keep auth snapshot read wrapper {pattern}"
        );
    }
    assert!(
        !runtime_mod.contains("mod audit;"),
        "state/runtime/mod.rs should not keep the obsolete audit runtime module"
    );
    assert!(
        auth_api_keys.contains("touch_auth_api_key_last_used_best_effort"),
        "state/runtime/auth/api_keys.rs should own auth api key last_used touch helper"
    );
    assert!(
        !auth_api_keys.contains("fn has_auth_api_key_writer("),
        "state/runtime/auth/api_keys.rs should not keep auth api key writer passthrough"
    );

    let auth_types = read_workspace_file("crates/aether-data/src/repository/auth/types.rs");
    for pattern in [
        "pub struct ResolvedAuthApiKeySnapshot",
        "pub trait ResolvedAuthApiKeySnapshotReader",
        "pub async fn read_resolved_auth_api_key_snapshot(",
        "pub async fn read_resolved_auth_api_key_snapshot_by_key_hash(",
        "pub async fn read_resolved_auth_api_key_snapshot_by_user_api_key_ids(",
        "pub fn effective_allowed_providers(&self)",
        "pub fn effective_allowed_api_formats(&self)",
        "pub fn effective_allowed_models(&self)",
    ] {
        assert!(
            auth_types.contains(pattern),
            "aether-data auth types should own {pattern}"
        );
    }
}

#[test]
fn gateway_auth_data_layer_does_not_keep_ldap_row_wrapper() {
    let gateway_auth_state = read_workspace_file("apps/aether-gateway/src/data/state/auth.rs");
    for pattern in [
        "struct StoredLdapAuthUserRow",
        "fn map_ldap_user_auth_row(",
        "Result<Option<StoredLdapAuthUserRow>, DataLayerError>",
        "existing.user.",
    ] {
        assert!(
            !gateway_auth_state.contains(pattern),
            "data/state/auth.rs should not keep ldap row wrapper {pattern}"
        );
    }

    for pattern in [
        "Result<Option<StoredUserAuthRecord>, DataLayerError>",
        "return map_user_auth_row(row).map(Some);",
    ] {
        assert!(
            gateway_auth_state.contains(pattern),
            "data/state/auth.rs should use shared user auth record directly via {pattern}"
        );
    }
}

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

fn handlers_do_not_depend_on_raw_state_records() {
    assert_no_sensitive_log_patterns(
        "src/handlers",
        &[
            "StoredUserSessionRecord",
            "StoredUserPreferenceRecord",
            "AdminPaymentCallbackRecord",
        ],
    );

    let state_types = read_workspace_file("apps/aether-gateway/src/state/types.rs");
    for pattern in [
        "pub(crate) struct GatewayUserSessionView",
        "pub(crate) struct GatewayUserPreferenceView",
        "pub(crate) struct GatewayAdminPaymentCallbackView",
    ] {
        assert!(
            state_types.contains(pattern),
            "state/types.rs should keep handler-facing state view {pattern}"
        );
    }
}

#[test]
fn admin_shared_does_not_own_provider_support() {
    let admin_shared = read_workspace_file("apps/aether-gateway/src/handlers/admin/shared/mod.rs");
    assert!(
        !admin_shared.contains("mod support;"),
        "handlers/admin/shared/mod.rs should not keep provider support module"
    );
    assert!(
        !workspace_file_exists("apps/aether-gateway/src/handlers/admin/shared/support.rs"),
        "handlers/admin/shared/support.rs should be removed after provider support extraction"
    );

    let provider_support =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/provider/shared/support.rs");
    for pattern in [
        "pub(crate) struct AdminProviderPoolConfig",
        "pub(crate) struct AdminProviderPoolRuntimeState",
        "pub(crate) const ADMIN_PROVIDER_POOL_SCAN_BATCH",
        "pub(crate) const ADMIN_PROVIDER_OAUTH_DATA_UNAVAILABLE_DETAIL",
    ] {
        assert!(
            provider_support.contains(pattern),
            "provider/shared/support.rs should own {pattern}"
        );
    }

    let admin_shared_paths =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/shared/paths.rs");
    for pattern in [
        "pub(crate) fn admin_provider_id_for_manage_path",
        "pub(crate) fn admin_provider_oauth_start_key_id",
        "pub(crate) fn admin_provider_ops_architecture_id_from_path",
    ] {
        assert!(
            !admin_shared_paths.contains(pattern),
            "handlers/admin/shared/paths.rs should not own {pattern}"
        );
    }

    let provider_paths =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/provider/shared/paths.rs");
    for pattern in [
        "pub(crate) fn admin_provider_id_for_manage_path",
        "pub(crate) fn admin_provider_oauth_start_key_id",
        "pub(crate) fn admin_provider_ops_architecture_id_from_path",
    ] {
        assert!(
            provider_paths.contains(pattern),
            "provider/shared/paths.rs should own {pattern}"
        );
    }

    let admin_shared_payloads =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/shared/payloads.rs");
    for pattern in [
        "pub(crate) struct AdminProviderCreateRequest",
        "pub(crate) struct AdminProviderEndpointCreateRequest",
        "pub(crate) struct AdminProviderModelCreateRequest",
    ] {
        assert!(
            !admin_shared_payloads.contains(pattern),
            "handlers/admin/shared/payloads.rs should not own {pattern}"
        );
    }

    let provider_payloads =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/provider/shared/payloads.rs");
    for pattern in [
        "pub(crate) struct AdminProviderCreateRequest",
        "pub(crate) struct AdminProviderModelCreateRequest",
    ] {
        assert!(
            provider_payloads.contains(pattern),
            "provider/shared/payloads.rs should own {pattern}"
        );
    }
    for pattern in [
        "AdminProviderEndpointCreateRequest",
        "AdminProviderEndpointUpdateRequest",
    ] {
        assert!(
            !provider_payloads.contains(pattern),
            "provider/shared/payloads.rs should not retain endpoint CRUD payload owner {pattern}"
        );
    }

    let provider_endpoints_payloads = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/endpoints_admin/payloads.rs",
    );
    for pattern in [
        "struct AdminProviderEndpointCreateRequest",
        "struct AdminProviderEndpointUpdateRequest",
    ] {
        assert!(
            provider_endpoints_payloads.contains(pattern),
            "provider/endpoints_admin/payloads.rs should own {pattern}"
        );
    }
}

#[test]
fn admin_shared_does_not_own_model_global_routes_or_payloads() {
    let admin_shared_paths =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/shared/paths.rs");
    for pattern in [
        "pub(crate) fn is_admin_global_models_root",
        "pub(crate) fn admin_global_model_id_from_path",
        "pub(crate) fn admin_global_model_routing_id",
    ] {
        assert!(
            !admin_shared_paths.contains(pattern),
            "handlers/admin/shared/paths.rs should not own {pattern}"
        );
    }

    let model_shared_paths =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/model/shared/paths.rs");
    for pattern in [
        "pub(crate) fn is_admin_global_models_root",
        "pub(crate) fn admin_global_model_id_from_path",
        "pub(crate) fn admin_global_model_routing_id",
    ] {
        assert!(
            model_shared_paths.contains(pattern),
            "model/shared/paths.rs should own {pattern}"
        );
    }

    let admin_shared_payloads =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/shared/payloads.rs");
    for pattern in [
        "pub(crate) struct AdminGlobalModelCreateRequest",
        "pub(crate) struct AdminGlobalModelUpdateRequest",
        "pub(crate) struct AdminBatchAssignToProvidersRequest",
    ] {
        assert!(
            !admin_shared_payloads.contains(pattern),
            "handlers/admin/shared/payloads.rs should not own {pattern}"
        );
    }

    let model_shared_payloads =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/model/shared/payloads.rs");
    for pattern in [
        "pub(crate) struct AdminGlobalModelCreateRequest",
        "pub(crate) struct AdminGlobalModelUpdateRequest",
        "pub(crate) struct AdminBatchAssignToProvidersRequest",
    ] {
        assert!(
            model_shared_payloads.contains(pattern),
            "model/shared/payloads.rs should own {pattern}"
        );
    }
}

#[test]
fn admin_shared_does_not_own_system_core_routes_or_payloads() {
    let admin_shared_paths =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/shared/paths.rs");
    for pattern in [
        "pub(crate) fn is_admin_management_tokens_root",
        "pub(crate) fn is_admin_system_configs_root",
        "pub(crate) fn admin_oauth_provider_type_from_path",
    ] {
        assert!(
            !admin_shared_paths.contains(pattern),
            "handlers/admin/shared/paths.rs should not own {pattern}"
        );
    }

    let system_shared_paths =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/system/shared/paths.rs");
    for pattern in [
        "pub(crate) fn is_admin_management_tokens_root",
        "pub(crate) fn is_admin_system_configs_root",
    ] {
        assert!(
            system_shared_paths.contains(pattern),
            "system/shared/paths.rs should own {pattern}"
        );
    }
    for pattern in [
        "admin_oauth_provider_type_from_path",
        "admin_oauth_test_provider_type_from_path",
    ] {
        assert!(
            !system_shared_paths.contains(pattern),
            "system/shared/paths.rs should not own auth oauth path helper {pattern}"
        );
    }

    let admin_shared_payloads =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/shared/payloads.rs");
    assert!(
        !admin_shared_payloads.contains("pub(crate) struct AdminOAuthProviderUpsertRequest"),
        "handlers/admin/shared/payloads.rs should not own AdminOAuthProviderUpsertRequest"
    );

    let auth_oauth_config =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/auth/oauth_config.rs");
    assert!(
        auth_oauth_config.contains("pub(crate) struct AdminOAuthProviderUpsertRequest"),
        "auth/oauth_config.rs should own AdminOAuthProviderUpsertRequest"
    );
    for pattern in [
        "pub(crate) fn admin_oauth_provider_type_from_path",
        "pub(crate) fn admin_oauth_test_provider_type_from_path",
    ] {
        assert!(
            auth_oauth_config.contains(pattern),
            "auth/oauth_config.rs should own {pattern}"
        );
    }
    assert!(
        !auth_oauth_config.contains("pub(crate) fn build_proxy_error_response"),
        "auth/oauth_config.rs should not own build_proxy_error_response"
    );
    assert!(
        !workspace_file_exists("apps/aether-gateway/src/handlers/admin/system/shared/payloads.rs"),
        "system/shared/payloads.rs should be removed after oauth payload ownership moves to auth"
    );

    let admin_shared_mod =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/shared/mod.rs");
    assert!(
        admin_shared_mod.contains("mod proxy_errors;")
            && admin_shared_mod
                .contains("pub(crate) use self::proxy_errors::build_proxy_error_response;"),
        "handlers/admin/shared/mod.rs should expose shared admin proxy error builder"
    );
    let admin_shared_proxy_errors =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/shared/proxy_errors.rs");
    assert!(
        admin_shared_proxy_errors.contains("pub(crate) fn build_proxy_error_response"),
        "handlers/admin/shared/proxy_errors.rs should own build_proxy_error_response"
    );
}

#[test]
fn admin_handlers_expose_real_subdomains_without_facade() {
    let admin_mod = read_workspace_file("apps/aether-gateway/src/handlers/admin/mod.rs");
    assert!(
        !admin_mod.contains("pub(crate) use self::"),
        "handlers/admin/mod.rs should stay as pure module wiring after facade removal"
    );

    for pattern in [
        "pub(crate) mod auth;",
        "pub(crate) mod endpoint;",
        "pub(crate) mod features;",
        "pub(crate) mod observability;",
        "pub(crate) mod provider;",
        "pub(crate) mod system;",
    ] {
        assert!(
            admin_mod.contains(pattern),
            "handlers/admin/mod.rs should expose admin subdomain module {pattern}"
        );
    }

    assert!(
        !workspace_file_exists("apps/aether-gateway/src/handlers/admin/facade.rs"),
        "handlers/admin/facade.rs should be removed after direct subdomain exposure"
    );
}

#[test]
fn admin_system_and_endpoint_roots_stay_thin() {
    let system_mod = read_workspace_file("apps/aether-gateway/src/handlers/admin/system/mod.rs");
    for pattern in [
        "pub(super) use super::auth::{",
        "pub(super) use super::model::{",
        "pub(super) use super::provider::{",
    ] {
        assert!(
            !system_mod.contains(pattern),
            "handlers/admin/system/mod.rs should not act as a cross-domain re-export layer for {pattern}"
        );
    }
    for pattern in [
        "pub(crate) use self::adaptive::maybe_build_local_admin_adaptive_response;",
        "pub(crate) use self::core::maybe_build_local_admin_core_response;",
        "pub(crate) use self::management_tokens::maybe_build_local_admin_management_tokens_response;",
        "pub(crate) use self::modules::maybe_build_local_admin_modules_response;",
        "pub(crate) use self::pool::maybe_build_local_admin_pool_response;",
        "pub(crate) use self::proxy_nodes::maybe_build_local_admin_proxy_nodes_response;",
    ] {
        assert!(
            system_mod.contains(pattern),
            "handlers/admin/system/mod.rs should stay as a thin system subdomain router for {pattern}"
        );
    }

    let system_pool =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/system/pool/mod.rs");
    assert!(
        system_pool.contains(
            "crate::handlers::admin::provider::pool_admin::maybe_build_local_admin_pool_response"
        ),
        "handlers/admin/system/pool/mod.rs should delegate pool admin ownership to provider::pool_admin"
    );
    for pattern in [
        "const ADMIN_POOL_PROVIDER_CATALOG_READER_UNAVAILABLE_DETAIL",
        "fn build_admin_pool_error_response(",
        "fn parse_admin_pool_page(",
        "mod batch_routes;",
        "mod read_routes;",
    ] {
        assert!(
            !system_pool.contains(pattern),
            "handlers/admin/system/pool/mod.rs should not own pool admin implementation detail {pattern}"
        );
    }

    let endpoint_mod =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/endpoint/mod.rs");
    for pattern in [
        "use self::extractors::{",
        "use self::health_builders::{",
        "use self::payloads::{",
    ] {
        assert!(
            !endpoint_mod.contains(pattern),
            "handlers/admin/endpoint/mod.rs should not re-export local helper seam {pattern}"
        );
    }
    assert!(
        endpoint_mod.contains(
            "pub(crate) use self::health_builders::build_admin_endpoint_health_status_payload;"
        ),
        "handlers/admin/endpoint/mod.rs should keep only the public health status payload seam"
    );

    let system_oauth_routes =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/system/core/oauth_routes.rs");
    assert!(
        system_oauth_routes.contains(
            "crate::handlers::admin::auth::maybe_build_local_admin_oauth_response as maybe_build_local_admin_core_oauth_response"
        ),
        "handlers/admin/system/core/oauth_routes.rs should delegate oauth config routing to auth"
    );
    for pattern in [
        "build_admin_oauth_provider_payload",
        "build_admin_oauth_upsert_record",
        "AdminOAuthProviderUpsertRequest",
        "admin_oauth_provider_type_from_path",
    ] {
        assert!(
            !system_oauth_routes.contains(pattern),
            "handlers/admin/system/core/oauth_routes.rs should not retain oauth config implementation {pattern}"
        );
    }

    let system_management_tokens_routes = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/system/core/management_tokens_routes.rs",
    );
    assert!(
        system_management_tokens_routes.contains(
            "crate::handlers::admin::system::maybe_build_local_admin_management_tokens_response as maybe_build_local_admin_core_management_tokens_response"
        ),
        "handlers/admin/system/core/management_tokens_routes.rs should delegate management token routing to system"
    );
    for pattern in [
        "list_management_tokens(",
        "get_management_token_with_user(",
        "delete_management_token(",
        "set_management_token_active(",
        "ManagementTokenListQuery",
    ] {
        assert!(
            !system_management_tokens_routes.contains(pattern),
            "handlers/admin/system/core/management_tokens_routes.rs should not retain management token implementation {pattern}"
        );
    }

    let system_modules_routes =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/system/core/modules_routes.rs");
    assert!(
        system_modules_routes.contains(
            "crate::handlers::admin::system::maybe_build_local_admin_modules_response as maybe_build_local_admin_core_modules_response"
        ),
        "handlers/admin/system/core/modules_routes.rs should delegate module management routing to system"
    );
    for pattern in [
        "build_admin_modules_status_payload",
        "build_admin_module_status_payload",
        "build_admin_module_runtime_state",
        "AdminSetModuleEnabledRequest",
        "admin_module_name_from_status_path",
    ] {
        assert!(
            !system_modules_routes.contains(pattern),
            "handlers/admin/system/core/modules_routes.rs should not retain module management implementation {pattern}"
        );
    }

    let system_model_routes =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/system/core/model_routes.rs");
    assert!(
        system_model_routes.contains(
            "crate::handlers::admin::model::maybe_build_local_admin_model_catalog_response as maybe_build_local_admin_core_model_response"
        ),
        "handlers/admin/system/core/model_routes.rs should delegate model catalog routing to model"
    );
    for pattern in [
        "build_admin_model_catalog_payload",
        "read_admin_external_models_cache",
        "clear_admin_external_models_cache",
        "ADMIN_MODEL_CATALOG_DATA_UNAVAILABLE_DETAIL",
    ] {
        assert!(
            !system_model_routes.contains(pattern),
            "handlers/admin/system/core/model_routes.rs should not retain model catalog implementation {pattern}"
        );
    }

    let system_routes =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/system/core/system_routes.rs");
    assert!(
        !system_routes.contains("use crate::handlers::public::{"),
        "handlers/admin/system/core/system_routes.rs should not borrow system-owned route helpers from handlers/public"
    );
    assert!(
        !system_routes.contains("crate::handlers::admin::auth::build_proxy_error_response")
            && !system_routes.contains("use crate::handlers::admin::auth::build_proxy_error_response;"),
        "handlers/admin/system/core/system_routes.rs should not borrow proxy error builder from auth"
    );
    for pattern in [
        "build_admin_email_template_payload",
        "build_admin_email_templates_payload",
        "preview_admin_email_template",
        "reset_admin_email_template",
    ] {
        assert!(
            system_routes.contains(pattern),
            "handlers/admin/system/core/system_routes.rs should keep delegating through admin system shared helper {pattern}"
        );
    }

    let endpoint_keys =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/endpoint/keys.rs");
    assert!(
        endpoint_keys.contains(
            "crate::handlers::admin::provider::endpoint_keys::maybe_build_local_admin_endpoints_keys_response"
        ),
        "handlers/admin/endpoint/keys.rs should delegate provider key management to provider::endpoint_keys"
    );
    for pattern in [
        "build_admin_create_provider_key_record",
        "build_admin_export_key_payload",
        "build_admin_update_provider_key_record",
        "refresh_codex_provider_quota_locally",
        "AdminProviderQuotaRefreshRequest",
    ] {
        assert!(
            !endpoint_keys.contains(pattern),
            "handlers/admin/endpoint/keys.rs should not own provider key management helper {pattern}"
        );
    }

    let endpoint_routes =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/endpoint/routes.rs");
    assert!(
        endpoint_routes.contains(
            "crate::handlers::admin::provider::endpoints_admin::maybe_build_local_admin_endpoints_routes_response"
        ),
        "handlers/admin/endpoint/routes.rs should delegate provider endpoint CRUD ownership to provider::endpoints_admin"
    );
    for pattern in [
        "build_admin_create_provider_endpoint_record",
        "build_admin_provider_endpoints_payload",
        "admin_provider_id_for_endpoints",
        "AdminProviderEndpointCreateRequest",
        "ADMIN_ENDPOINTS_DATA_UNAVAILABLE_DETAIL",
    ] {
        assert!(
            !endpoint_routes.contains(pattern),
            "handlers/admin/endpoint/routes.rs should not own provider endpoint CRUD helper {pattern}"
        );
    }
}

#[test]
fn admin_provider_root_stays_thin() {
    let provider_mod =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/provider/mod.rs");
    for pattern in [
        "pub(crate) mod endpoint_keys;",
        "pub(crate) mod endpoints_admin;",
        "pub(crate) mod oauth;",
        "pub(crate) mod ops;",
        "pub(crate) mod pool;",
        "pub(crate) mod pool_admin;",
        "pub(crate) mod shared;",
        "pub(crate) mod write;",
    ] {
        assert!(
            provider_mod.contains(pattern),
            "handlers/admin/provider/mod.rs should expose provider subdomain module {pattern}"
        );
    }

    for pattern in [
        "pub(crate) use self::oauth::{",
        "pub(crate) use self::ops::{",
        "pub(crate) use self::pool::{",
        "pub(crate) use self::endpoints_admin::{",
        "pub(crate) use self::pool_admin::{",
        "pub(crate) use self::write::{",
        "build_proxy_error_response",
        "build_internal_control_error_response",
        "admin_provider_ops_local_action_response",
        "admin_provider_pool_config",
        "build_admin_create_provider_key_record",
        "build_admin_export_key_payload",
        "normalize_provider_billing_type",
        "parse_optional_rfc3339_unix_secs",
    ] {
        assert!(
            !provider_mod.contains(pattern),
            "handlers/admin/provider/mod.rs should not act as internal helper export hub for {pattern}"
        );
    }

    for pattern in [
        "pub(crate) use self::crud::maybe_build_local_admin_providers_response;",
        "pub(crate) use self::models::maybe_build_local_admin_provider_models_response;",
        "pub(crate) use self::oauth::maybe_build_local_admin_provider_oauth_response;",
        "pub(crate) use self::ops::maybe_build_local_admin_provider_ops_response;",
        "pub(crate) use self::query::maybe_build_local_admin_provider_query_response;",
        "pub(crate) use self::strategy::maybe_build_local_admin_provider_strategy_response;",
    ] {
        assert!(
            provider_mod.contains(pattern),
            "handlers/admin/provider/mod.rs should keep route entry seam {pattern}"
        );
    }
}

#[test]
fn admin_model_root_owns_model_catalog_routes() {
    let model_mod = read_workspace_file("apps/aether-gateway/src/handlers/admin/model/mod.rs");
    assert!(
        model_mod.contains("mod catalog_routes;"),
        "handlers/admin/model/mod.rs should register catalog_routes owner"
    );
    assert!(
        model_mod.contains(
            "pub(crate) use self::catalog_routes::maybe_build_local_admin_model_catalog_response;"
        ),
        "handlers/admin/model/mod.rs should expose model catalog route seam"
    );

    let model_catalog_routes =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/model/catalog_routes.rs");
    for pattern in [
        "build_admin_model_catalog_payload",
        "read_admin_external_models_cache",
        "clear_admin_external_models_cache",
        "ADMIN_MODEL_CATALOG_DATA_UNAVAILABLE_DETAIL",
    ] {
        assert!(
            model_catalog_routes.contains(pattern),
            "handlers/admin/model/catalog_routes.rs should own {pattern}"
        );
    }
}

#[test]
fn admin_system_owns_admin_module_helpers() {
    let public_modules_helpers =
        read_workspace_file("apps/aether-gateway/src/handlers/public/system_modules_helpers.rs");
    for pattern in [
        "admin_module_by_name",
        "admin_module_name_from_enabled_path",
        "admin_module_name_from_status_path",
        "build_admin_module_runtime_state",
        "build_admin_module_status_payload",
        "build_admin_module_validation_result",
        "build_admin_modules_status_payload",
        "AdminSetModuleEnabledRequest",
    ] {
        assert!(
            !public_modules_helpers.contains(pattern),
            "handlers/public/system_modules_helpers.rs should not re-export admin module helper {pattern}"
        );
    }

    let public_modules = read_workspace_file(
        "apps/aether-gateway/src/handlers/public/system_modules_helpers/modules.rs",
    );
    for pattern in [
        "pub(crate) struct AdminModuleDefinition",
        "pub(crate) struct AdminSetModuleEnabledRequest",
        "pub(crate) struct AdminModuleRuntimeState",
        "pub(crate) fn admin_module_by_name",
        "pub(crate) async fn build_admin_module_runtime_state",
        "pub(crate) fn build_admin_module_validation_result",
        "pub(crate) async fn build_admin_module_status_payload",
        "pub(crate) async fn build_admin_modules_status_payload",
        "pub(crate) fn admin_module_name_from_status_path",
        "pub(crate) fn admin_module_name_from_enabled_path",
    ] {
        assert!(
            !public_modules.contains(pattern),
            "handlers/public/system_modules_helpers/modules.rs should not own admin module helper {pattern}"
        );
    }

    let system_shared_modules =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/system/shared/modules.rs");
    for pattern in [
        "pub(crate) struct AdminModuleDefinition",
        "pub(crate) struct AdminSetModuleEnabledRequest",
        "pub(crate) struct AdminModuleRuntimeState",
        "pub(crate) fn admin_module_by_name",
        "pub(crate) async fn build_admin_module_runtime_state",
        "pub(crate) fn build_admin_module_validation_result",
        "pub(crate) async fn build_admin_module_status_payload",
        "pub(crate) async fn build_admin_modules_status_payload",
        "pub(crate) fn admin_module_name_from_status_path",
        "pub(crate) fn admin_module_name_from_enabled_path",
    ] {
        assert!(
            system_shared_modules.contains(pattern),
            "handlers/admin/system/shared/modules.rs should own {pattern}"
        );
    }
}

#[test]
fn admin_system_owns_system_route_helpers() {
    let public_system_helpers =
        read_workspace_file("apps/aether-gateway/src/handlers/public/system_modules_helpers.rs");
    for pattern in [
        "current_aether_version",
        "build_admin_system_check_update_payload",
        "build_admin_system_stats_payload",
        "build_admin_system_settings_payload",
        "apply_admin_system_settings_update",
        "build_admin_api_formats_payload",
        "build_admin_system_config_export_payload",
        "build_admin_system_users_export_payload",
        "build_admin_system_configs_payload",
        "build_admin_system_config_detail_payload",
        "apply_admin_system_config_update",
        "delete_admin_system_config",
        "serialize_admin_system_users_export_wallet",
        "module_available_from_env",
        "system_config_bool",
        "system_config_string",
        "read_admin_email_template_payload",
        "escape_admin_email_template_html",
        "render_admin_email_template_html",
    ] {
        assert!(
            !public_system_helpers.contains(pattern),
            "handlers/public/system_modules_helpers.rs should not re-export admin system helper {pattern}"
        );
    }

    let public_mod = read_workspace_file("apps/aether-gateway/src/handlers/public/mod.rs");
    for pattern in [
        "current_aether_version",
        "build_admin_system_check_update_payload",
        "build_admin_system_stats_payload",
        "build_admin_system_settings_payload",
        "apply_admin_system_settings_update",
        "build_admin_api_formats_payload",
        "build_admin_system_config_export_payload",
        "build_admin_system_users_export_payload",
        "build_admin_system_configs_payload",
        "build_admin_system_config_detail_payload",
        "apply_admin_system_config_update",
        "delete_admin_system_config",
        "serialize_admin_system_users_export_wallet",
        "module_available_from_env",
        "system_config_bool",
        "system_config_string",
        "read_admin_email_template_payload",
        "escape_admin_email_template_html",
        "render_admin_email_template_html",
    ] {
        assert!(
            !public_mod.contains(pattern),
            "handlers/public/mod.rs should not re-export admin system helper {pattern}"
        );
    }

    let public_system_file = read_workspace_file(
        "apps/aether-gateway/src/handlers/public/system_modules_helpers/system.rs",
    );
    for pattern in [
        "pub(crate) fn current_aether_version",
        "pub(crate) fn build_admin_system_check_update_payload",
        "pub(crate) async fn build_admin_system_stats_payload",
        "pub(crate) async fn build_admin_system_settings_payload",
        "pub(crate) async fn build_admin_system_config_export_payload",
        "pub(crate) async fn build_admin_system_users_export_payload",
        "pub(crate) fn build_admin_system_configs_payload",
        "pub(crate) async fn build_admin_system_config_detail_payload",
        "pub(crate) async fn apply_admin_system_config_update",
        "pub(crate) async fn delete_admin_system_config",
        "pub(crate) fn serialize_admin_system_users_export_wallet",
        "pub(crate) fn module_available_from_env",
        "pub(crate) fn system_config_bool",
        "pub(crate) fn system_config_string",
        "pub(crate) async fn read_admin_email_template_payload",
        "pub(crate) fn escape_admin_email_template_html",
        "pub(crate) fn render_admin_email_template_html",
    ] {
        assert!(
            !public_system_file.contains(pattern),
            "handlers/public/system_modules_helpers/system.rs should not own shared/admin system helper {pattern}"
        );
    }

    let shared_mod = read_workspace_file("apps/aether-gateway/src/handlers/shared/mod.rs");
    for pattern in [
        "mod email_templates;",
        "mod system_config_values;",
        "pub(crate) use self::email_templates::{",
        "pub(crate) use self::system_config_values::{",
    ] {
        assert!(
            shared_mod.contains(pattern),
            "handlers/shared/mod.rs should wire shared system helper owner {pattern}"
        );
    }

    let shared_system_config_values =
        read_workspace_file("apps/aether-gateway/src/handlers/shared/system_config_values.rs");
    for pattern in [
        "pub(crate) fn module_available_from_env",
        "pub(crate) fn system_config_bool",
        "pub(crate) fn system_config_string",
    ] {
        assert!(
            shared_system_config_values.contains(pattern),
            "handlers/shared/system_config_values.rs should own {pattern}"
        );
    }

    let shared_email_templates =
        read_workspace_file("apps/aether-gateway/src/handlers/shared/email_templates.rs");
    for pattern in [
        "pub(crate) fn admin_email_template_definition",
        "pub(crate) fn admin_email_template_subject_key",
        "pub(crate) fn admin_email_template_html_key",
        "pub(crate) async fn read_admin_email_template_payload",
        "pub(crate) fn escape_admin_email_template_html",
        "pub(crate) fn render_admin_email_template_html",
    ] {
        assert!(
            shared_email_templates.contains(pattern),
            "handlers/shared/email_templates.rs should own {pattern}"
        );
    }

    let system_shared_mod =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/system/shared/mod.rs");
    for pattern in [
        "mod configs;",
        "mod email_templates;",
        "mod settings;",
        "pub(crate) use self::email_templates::{",
        "pub(crate) use self::configs::*;",
        "pub(crate) use self::settings::*;",
    ] {
        assert!(
            system_shared_mod.contains(pattern),
            "handlers/admin/system/shared/mod.rs should wire system helper owner {pattern}"
        );
    }

    assert!(
        !workspace_file_exists("apps/aether-gateway/src/handlers/admin/system/shared/system.rs"),
        "handlers/admin/system/shared/system.rs should be replaced by email_templates.rs"
    );

    let system_shared_settings =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/system/shared/settings.rs");
    for pattern in [
        "pub(crate) fn current_aether_version",
        "pub(crate) fn build_admin_system_check_update_payload",
        "pub(crate) async fn build_admin_system_stats_payload",
        "pub(crate) async fn build_admin_system_settings_payload",
        "pub(crate) async fn apply_admin_system_settings_update",
        "pub(crate) fn build_admin_api_formats_payload",
    ] {
        assert!(
            system_shared_settings.contains(pattern),
            "handlers/admin/system/shared/settings.rs should own {pattern}"
        );
    }

    let system_shared_configs =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/system/shared/configs.rs");
    for pattern in [
        "pub(crate) async fn build_admin_system_config_export_payload",
        "pub(crate) fn serialize_admin_system_users_export_wallet",
        "pub(crate) async fn build_admin_system_users_export_payload",
        "pub(crate) fn build_admin_system_configs_payload",
        "pub(crate) async fn build_admin_system_config_detail_payload",
        "pub(crate) async fn apply_admin_system_config_update",
        "pub(crate) async fn delete_admin_system_config",
    ] {
        assert!(
            system_shared_configs.contains(pattern),
            "handlers/admin/system/shared/configs.rs should own {pattern}"
        );
    }

    let system_shared_email_templates = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/system/shared/email_templates.rs",
    );
    for pattern in [
        "pub(crate) async fn build_admin_email_templates_payload",
        "pub(crate) async fn build_admin_email_template_payload",
        "pub(crate) async fn apply_admin_email_template_update",
        "pub(crate) async fn preview_admin_email_template",
        "pub(crate) async fn reset_admin_email_template",
    ] {
        assert!(
            system_shared_email_templates.contains(pattern),
            "handlers/admin/system/shared/email_templates.rs should own {pattern}"
        );
    }
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

#[test]
fn gateway_provider_oauth_storage_types_are_owned_by_aether_data() {
    let provider_oauth_state =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/provider/oauth/state.rs");
    assert!(
        provider_oauth_state.contains("aether_data::repository::provider_oauth"),
        "provider_oauth/state.rs should depend on aether-data provider oauth storage types"
    );
    for pattern in [
        "pub(crate) struct StoredAdminProviderOAuthDeviceSession",
        "pub(crate) struct StoredAdminProviderOAuthState",
        "const KIRO_DEVICE_AUTH_SESSION_PREFIX",
        "fn provider_oauth_device_session_key(",
        "fn build_provider_oauth_batch_task_status_payload(",
        "fn provider_oauth_batch_task_key(",
        "const PROVIDER_OAUTH_BATCH_TASK_TTL_SECS",
        "format!(\"provider_oauth_state:{nonce}\")",
    ] {
        assert!(
            !provider_oauth_state.contains(pattern),
            "provider_oauth/state.rs should not own local storage helper {pattern}"
        );
    }

    let dispatch_device = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/oauth/dispatch/device.rs",
    );
    assert!(
        dispatch_device.contains("aether_data::repository::provider_oauth"),
        "provider_oauth/dispatch/device.rs should use shared provider oauth storage DTOs"
    );

    let shared_provider_oauth =
        read_workspace_file("crates/aether-data/src/repository/provider_oauth.rs");
    for pattern in [
        "pub struct StoredAdminProviderOAuthDeviceSession",
        "pub struct StoredAdminProviderOAuthState",
        "pub fn provider_oauth_device_session_storage_key(",
        "pub fn provider_oauth_state_storage_key(",
        "pub fn provider_oauth_batch_task_storage_key(",
        "pub fn build_provider_oauth_batch_task_status_payload(",
        "pub const KIRO_DEVICE_AUTH_SESSION_TTL_BUFFER_SECS: u64 = 60;",
        "pub const PROVIDER_OAUTH_BATCH_TASK_TTL_SECS: u64 = 24 * 60 * 60;",
        "pub const PROVIDER_OAUTH_STATE_TTL_SECS: u64 = 600;",
    ] {
        assert!(
            shared_provider_oauth.contains(pattern),
            "aether-data provider oauth storage module should own {pattern}"
        );
    }
}

#[test]
fn gateway_request_candidate_trace_type_is_owned_by_aether_data_contracts() {
    let gateway_candidates = read_workspace_file("apps/aether-gateway/src/data/candidates.rs");
    assert!(
        gateway_candidates.contains("aether_data_contracts::repository::candidates"),
        "data/candidates.rs should depend on aether-data-contracts request candidate types"
    );
    assert!(
        gateway_candidates.contains("RequestCandidateTrace::from_candidates"),
        "data/candidates.rs should build traces through shared candidate trace helper"
    );
    for pattern in [
        "pub(crate) enum RequestCandidateFinalStatus",
        "pub(crate) struct RequestCandidateTrace",
        "fn derive_final_status(",
    ] {
        assert!(
            !gateway_candidates.contains(pattern),
            "data/candidates.rs should not own local request candidate trace logic {pattern}"
        );
    }

    let candidate_types =
        read_workspace_file("crates/aether-data-contracts/src/repository/candidates/types.rs");
    for pattern in [
        "pub enum RequestCandidateFinalStatus",
        "pub struct RequestCandidateTrace",
        "pub fn derive_request_candidate_final_status(",
        "pub fn from_candidates(",
    ] {
        assert!(
            candidate_types.contains(pattern),
            "aether-data-contracts candidate types should own {pattern}"
        );
    }
}

#[test]
fn gateway_decision_trace_type_is_owned_by_aether_data_contracts() {
    let gateway_decision_trace =
        read_workspace_file("apps/aether-gateway/src/data/decision_trace.rs");
    assert!(
        gateway_decision_trace.contains("aether_data_contracts::repository::candidates"),
        "data/decision_trace.rs should depend on aether-data-contracts candidate trace types"
    );
    assert!(
        gateway_decision_trace.contains("build_decision_trace"),
        "data/decision_trace.rs should build enriched traces through shared decision trace helper"
    );
    for pattern in [
        "pub(crate) struct DecisionTraceCandidate",
        "pub(crate) struct DecisionTrace",
        "fn enrich_candidate(",
    ] {
        assert!(
            !gateway_decision_trace.contains(pattern),
            "data/decision_trace.rs should not own local decision trace logic {pattern}"
        );
    }

    let candidate_types =
        read_workspace_file("crates/aether-data-contracts/src/repository/candidates/types.rs");
    for pattern in [
        "pub struct DecisionTraceCandidate",
        "pub struct DecisionTrace",
        "pub fn build_decision_trace(",
    ] {
        assert!(
            candidate_types.contains(pattern),
            "aether-data-contracts candidate types should own {pattern}"
        );
    }
}

#[test]
fn scheduler_candidate_runtime_paths_depend_on_scheduler_core_and_state_trait() {
    let scheduler_mod = read_workspace_file("apps/aether-gateway/src/scheduler/mod.rs");
    let candidate_mod = read_workspace_file("apps/aether-gateway/src/scheduler/candidate/mod.rs");
    assert!(
        !scheduler_mod.contains("mod health;"),
        "scheduler/mod.rs should not keep the legacy health re-export module"
    );
    for pattern in [
        "count_recent_rpm_requests_for_provider_key",
        "count_recent_rpm_requests_for_provider_key_since",
        "is_provider_key_circuit_open",
        "provider_key_health_score",
        "provider_key_rpm_allows_request_since",
        "PROVIDER_KEY_RPM_WINDOW_SECS",
        "SchedulerMinimalCandidateSelectionCandidate",
        "read_minimal_candidate_selection",
        "read_cached_scheduler_affinity_target",
        "list_selectable_candidates",
        "list_selectable_candidates_for_required_capability_without_requested_model",
        "MinimalCandidateSelectionRowSource",
        "SchedulerRuntimeState",
    ] {
        assert!(
            !scheduler_mod.contains(pattern),
            "scheduler/mod.rs should not re-export scheduler helper {pattern}"
        );
    }
    assert!(
        candidate_mod.contains("SchedulerMinimalCandidateSelectionCandidate"),
        "candidate/mod.rs should depend on core minimal candidate DTO"
    );
    assert!(
        !candidate_mod.contains("build_minimal_candidate_selection"),
        "candidate/mod.rs should not own the core minimal candidate builder anymore"
    );
    assert!(
        !candidate_mod.contains("collect_global_model_names_for_required_capability"),
        "candidate/mod.rs should not own the core capability model-name collector anymore"
    );
    assert!(
        !candidate_mod.contains("collect_selectable_candidates_from_keys"),
        "candidate/mod.rs should not own the core selectable-candidate collector anymore"
    );
    assert!(
        !candidate_mod.contains("auth_api_key_concurrency_limit_reached"),
        "candidate/mod.rs should not own the core auth api key concurrency helper anymore"
    );
    assert!(
        !candidate_mod.contains("pub(crate) struct SchedulerMinimalCandidateSelectionCandidate"),
        "candidate/mod.rs should not own the minimal candidate DTO"
    );
    for pattern in [
        "pub(crate) async fn read_minimal_candidate_selection(",
        "pub(crate) async fn select_minimal_candidate(",
        "pub(crate) fn read_cached_scheduler_affinity_target(",
        "async fn collect_selectable_candidates(",
    ] {
        assert!(
            !candidate_mod.contains(pattern),
            "candidate/mod.rs should not expose test-only scheduler helper {pattern}"
        );
    }
    for pattern in [
        "pub(crate) async fn list_selectable_candidates(",
        "pub(crate) async fn list_selectable_candidates_for_required_capability_without_requested_model(",
    ] {
        assert!(
            candidate_mod.contains(pattern),
            "candidate/mod.rs should host scheduler selection entrypoint {pattern}"
        );
    }
    for pattern in [
        "resolve_provider_model_name(&row",
        "extract_global_priority_for_format(",
        "compare_affinity_order(",
        "row_supports_required_capability(&row",
        "selected.push(candidate);",
        "if let Some(target) = cached_affinity_target",
        "count_recent_active_requests_for_api_key(",
    ] {
        assert!(
            !candidate_mod.contains(pattern),
            "candidate/mod.rs should not own {pattern}"
        );
    }

    let selection = read_workspace_file("apps/aether-gateway/src/scheduler/candidate/selection.rs");
    assert!(
        selection.contains("SchedulerRuntimeState"),
        "selection.rs should depend on SchedulerRuntimeState"
    );
    assert!(
        !selection.contains("use crate::{AppState"),
        "selection.rs should not depend on AppState directly"
    );
    assert!(
        !selection.contains("crate::cache::SchedulerAffinityTarget"),
        "selection.rs should not depend on gateway-local SchedulerAffinityTarget"
    );
    assert!(
        selection.contains("reorder_candidates_by_scheduler_health_in_core"),
        "selection.rs should depend on core candidate reorder helper"
    );
    assert!(
        selection.contains("collect_selectable_candidates_from_keys"),
        "selection.rs should depend on core selectable-candidate collector"
    );
    for pattern in [
        "async fn collect_selectable_candidates(",
        "async fn select_minimal_candidate(",
    ] {
        assert!(
            selection.contains(pattern),
            "selection.rs should host internal selection pipeline helper {pattern}"
        );
    }
    for pattern in [
        "fn compare_provider_key_health_order(",
        "fn candidate_provider_key_health_bucket(",
        "fn candidate_provider_key_health_score(",
        "count_recent_active_requests_for_provider(",
        "is_candidate_in_recent_failure_cooldown(",
        "provider_key_health_score(",
        "provider_key_rpm_allows_request_since(",
        "read_recent_request_candidates(128)",
        "read_provider_concurrent_limits(",
        "read_provider_key_rpm_states(",
        "candidate_is_selectable_with_runtime_state",
        "auth_api_key_concurrency_limit_reached",
        "build_provider_concurrent_limit_map(",
    ] {
        assert!(
            !selection.contains(pattern),
            "selection.rs should not own {pattern}"
        );
    }

    let runtime = read_workspace_file("apps/aether-gateway/src/scheduler/candidate/runtime.rs");
    assert!(
        runtime.contains("SchedulerRuntimeState"),
        "candidate/runtime.rs should depend on SchedulerRuntimeState"
    );
    assert!(
        runtime.contains("candidate_is_selectable_with_runtime_state"),
        "candidate/runtime.rs should depend on core selectable predicate helper"
    );
    assert!(
        runtime.contains("SchedulerAffinityTarget"),
        "candidate/runtime.rs should depend on core SchedulerAffinityTarget"
    );
    assert!(
        runtime.contains("auth_api_key_concurrency_limit_reached"),
        "candidate/runtime.rs should depend on core auth api key concurrency helper"
    );
    assert!(
        runtime.contains("build_provider_concurrent_limit_map"),
        "candidate/runtime.rs should depend on core provider concurrent limit helper"
    );
    assert!(
        runtime.contains("CandidateRuntimeSelectionSnapshot"),
        "candidate/runtime.rs should host runtime snapshot type"
    );
    assert!(
        runtime.contains("read_candidate_runtime_selection_snapshot"),
        "candidate/runtime.rs should host runtime snapshot reader"
    );
    assert!(
        runtime.contains("should_skip_provider_quota"),
        "candidate/runtime.rs should host provider quota skip helper"
    );
    assert!(
        !runtime.contains("AppState"),
        "candidate/runtime.rs should not depend on AppState directly"
    );

    assert!(
        !workspace_file_exists("apps/aether-gateway/src/scheduler/candidate/tests.rs"),
        "candidate/tests.rs should be split into themed test modules"
    );
    for path in [
        "apps/aether-gateway/src/scheduler/candidate/tests/mod.rs",
        "apps/aether-gateway/src/scheduler/candidate/tests/support.rs",
        "apps/aether-gateway/src/scheduler/candidate/tests/model.rs",
        "apps/aether-gateway/src/scheduler/candidate/tests/affinity.rs",
        "apps/aether-gateway/src/scheduler/candidate/tests/selection.rs",
    ] {
        assert!(
            workspace_file_exists(path),
            "candidate test module should exist at {path}"
        );
    }
    let candidate_tests_mod =
        read_workspace_file("apps/aether-gateway/src/scheduler/candidate/tests/mod.rs");
    for pattern in [
        "mod support;",
        "mod model;",
        "mod affinity;",
        "mod selection;",
    ] {
        assert!(
            candidate_tests_mod.contains(pattern),
            "candidate tests/mod.rs should declare {pattern}"
        );
    }

    let affinity = read_workspace_file("apps/aether-gateway/src/scheduler/candidate/affinity.rs");
    assert!(
        affinity.contains("SchedulerRuntimeState"),
        "candidate/affinity.rs should depend on SchedulerRuntimeState"
    );
    assert!(
        affinity.contains("aether_scheduler_core::{")
            && affinity.contains("SchedulerAffinityTarget"),
        "candidate/affinity.rs should depend on core SchedulerAffinityTarget"
    );
    for pattern in [
        "candidate_affinity_hash",
        "compare_affinity_order",
        "matches_affinity_target",
        "candidate_key",
    ] {
        assert!(
            affinity.contains(pattern),
            "candidate/affinity.rs should depend on core affinity helper {pattern}"
        );
    }
    assert!(
        !affinity.contains("use crate::AppState"),
        "candidate/affinity.rs should not depend on AppState directly"
    );
    assert!(
        !affinity.contains("crate::cache::SchedulerAffinityTarget"),
        "candidate/affinity.rs should not depend on gateway-local SchedulerAffinityTarget"
    );
    assert!(
        !affinity.contains("use sha2::{Digest, Sha256};"),
        "candidate/affinity.rs should not own affinity hashing implementation anymore"
    );
    for pattern in [
        "fn compare_affinity_order(",
        "fn candidate_affinity_hash(",
        "fn matches_affinity_target(",
        "fn candidate_key(",
    ] {
        assert!(
            !affinity.contains(pattern),
            "candidate/affinity.rs should not own {pattern}"
        );
    }

    let scheduler_affinity = read_workspace_file("apps/aether-gateway/src/scheduler/affinity.rs");
    assert!(
        scheduler_affinity.contains("SchedulerRuntimeState"),
        "scheduler/affinity.rs should depend on SchedulerRuntimeState"
    );
    assert!(
        scheduler_affinity.contains("build_scheduler_affinity_cache_key_for_api_key_id"),
        "scheduler/affinity.rs should depend on core affinity cache-key helper"
    );
    assert!(
        scheduler_affinity.contains("pub(crate) fn read_cached_scheduler_affinity_target("),
        "scheduler/affinity.rs should host the external affinity cache lookup"
    );
    assert!(
        scheduler_affinity.contains("SCHEDULER_AFFINITY_TTL"),
        "scheduler/affinity.rs should host the shared affinity ttl"
    );
    assert!(
        !scheduler_affinity.contains("MinimalCandidateSelectionRowSource"),
        "scheduler/affinity.rs should not depend on minimal candidate selection row source"
    );
    assert!(
        !scheduler_affinity.contains("AppState"),
        "scheduler/affinity.rs should not depend on AppState directly"
    );

    assert!(
        !workspace_file_exists("apps/aether-gateway/src/scheduler/candidate/model.rs"),
        "scheduler/candidate/model.rs facade should be removed"
    );
    assert!(
        candidate_mod.contains("candidate_supports_required_capability"),
        "candidate/mod.rs should depend directly on core candidate capability helper"
    );
    assert!(
        candidate_mod.contains("normalize_api_format"),
        "candidate/mod.rs should depend directly on core model helper namespace"
    );

    let affinity_cache = read_workspace_file("apps/aether-gateway/src/cache/scheduler_affinity.rs");
    assert!(
        affinity_cache.contains("aether_scheduler_core::SchedulerAffinityTarget"),
        "scheduler affinity cache should reuse core SchedulerAffinityTarget"
    );

    let state_core = read_workspace_file("apps/aether-gateway/src/state/core.rs");
    assert!(
        state_core.contains("aether_scheduler_core::PROVIDER_KEY_RPM_WINDOW_SECS"),
        "state/core.rs should depend directly on core rpm window constant"
    );
    assert!(
        !state_core.contains("scheduler::PROVIDER_KEY_RPM_WINDOW_SECS"),
        "state/core.rs should not route rpm window constant through crate::scheduler"
    );

    let candidate_state = read_workspace_file("apps/aether-gateway/src/scheduler/state.rs");
    assert!(
        candidate_state.contains("pub(crate) trait SchedulerRuntimeState"),
        "scheduler/state.rs should host SchedulerRuntimeState"
    );
    assert!(
        !candidate_state.contains("MinimalCandidateSelectionRowSource"),
        "scheduler/state.rs should not host MinimalCandidateSelectionRowSource"
    );
    assert!(
        !candidate_state.contains("pub(crate) trait SchedulerCandidateState"),
        "scheduler/state.rs should not keep a merged SchedulerCandidateState wrapper"
    );
    for pattern in [
        "impl MinimalCandidateSelectionRowSource for GatewayDataState",
        "impl MinimalCandidateSelectionRowSource for AppState",
        "impl SchedulerRuntimeState for AppState",
        "async fn read_minimal_candidate_selection(",
    ] {
        assert!(
            !candidate_state.contains(pattern),
            "scheduler/state.rs should not host {pattern} anymore"
        );
    }

    let candidate_selection =
        read_workspace_file("apps/aether-gateway/src/data/candidate_selection.rs");
    assert!(
        candidate_selection.contains("pub(crate) trait MinimalCandidateSelectionRowSource"),
        "data/candidate_selection.rs should host MinimalCandidateSelectionRowSource"
    );
    assert!(
        candidate_selection.contains("pub(crate) async fn read_requested_model_rows("),
        "data/candidate_selection.rs should host requested-model row lookup"
    );
    assert!(
        candidate_selection.contains("pub(crate) async fn read_minimal_candidate_selection("),
        "data/candidate_selection.rs should host minimal candidate selection builder"
    );
    assert!(
        candidate_selection
            .contains("pub(crate) async fn read_global_model_names_for_required_capability("),
        "data/candidate_selection.rs should host capability model-name lookup"
    );
    assert!(
        candidate_selection.contains("resolve_requested_global_model_name"),
        "data/candidate_selection.rs should depend on core requested-model resolver"
    );
    assert!(
        candidate_selection.contains("build_minimal_candidate_selection"),
        "data/candidate_selection.rs should depend on core minimal candidate builder"
    );
    assert!(
        candidate_selection.contains("collect_global_model_names_for_required_capability"),
        "data/candidate_selection.rs should depend on core capability model-name collector"
    );
    assert!(
        candidate_selection.contains("auth_constraints_allow_api_format"),
        "data/candidate_selection.rs should depend on core auth api-format helper"
    );
    assert!(
        candidate_selection.contains("GatewayAuthApiKeySnapshot"),
        "data/candidate_selection.rs should depend on GatewayAuthApiKeySnapshot for auth gating"
    );
    for pattern in [
        "impl MinimalCandidateSelectionRowSource for GatewayDataState",
        "impl MinimalCandidateSelectionRowSource for AppState",
        "SchedulerRuntimeState",
    ] {
        assert!(
            !candidate_selection.contains(pattern),
            "data/candidate_selection.rs should not host {pattern}"
        );
    }

    let candidate_mod = read_workspace_file("apps/aether-gateway/src/scheduler/candidate/mod.rs");
    for pattern in [
        "use crate::{AppState, GatewayError};",
        "state: &AppState,",
        "state: &impl SchedulerCandidateState",
    ] {
        assert!(
            !candidate_mod.contains(pattern),
            "candidate/mod.rs should not hard-code AppState boundary {pattern}"
        );
    }
    for pattern in [
        "selection_row_source: &(impl MinimalCandidateSelectionRowSource + Sync)",
        "runtime_state: &impl SchedulerRuntimeState",
    ] {
        assert!(
            candidate_mod.contains(pattern),
            "candidate/mod.rs should expose split scheduler boundaries via {pattern}"
        );
    }

    let planner_candidate_affinity =
        read_workspace_file("apps/aether-gateway/src/ai_pipeline/planner/candidate_affinity.rs");
    assert!(
        planner_candidate_affinity
            .contains("aether_scheduler_core::SchedulerMinimalCandidateSelectionCandidate"),
        "planner/candidate_affinity.rs should depend directly on core minimal candidate DTO"
    );
    assert!(
        !planner_candidate_affinity
            .contains("crate::scheduler::SchedulerMinimalCandidateSelectionCandidate"),
        "planner/candidate_affinity.rs should not depend on scheduler candidate DTO re-export"
    );

    let request_candidate_runtime =
        read_workspace_file("apps/aether-gateway/src/request_candidate_runtime.rs");
    assert!(
        request_candidate_runtime.contains("SchedulerMinimalCandidateSelectionCandidate"),
        "request_candidate_runtime.rs should depend directly on core minimal candidate DTO"
    );
    assert!(
        !request_candidate_runtime
            .contains("crate::scheduler::SchedulerMinimalCandidateSelectionCandidate"),
        "request_candidate_runtime.rs should not depend on scheduler candidate DTO re-export"
    );

    let state_integrations = read_workspace_file("apps/aether-gateway/src/state/integrations.rs");
    assert!(
        !state_integrations.contains("impl MinimalCandidateSelectionRowSource for AppState"),
        "state/integrations.rs should not host MinimalCandidateSelectionRowSource for AppState anymore"
    );
    assert!(
        state_integrations.contains("impl SchedulerRuntimeState for AppState"),
        "state/integrations.rs should host SchedulerRuntimeState for AppState"
    );
    assert!(
        !state_integrations.contains("async fn read_minimal_candidate_selection("),
        "state/integrations.rs should not re-host scheduler minimal candidate bridge anymore"
    );

    let data_state_integrations =
        read_workspace_file("apps/aether-gateway/src/data/state/integrations.rs");
    assert!(
        data_state_integrations
            .contains("impl MinimalCandidateSelectionRowSource for GatewayDataState"),
        "data/state/integrations.rs should host MinimalCandidateSelectionRowSource for GatewayDataState"
    );
}

#[test]
fn ai_pipeline_routes_control_and_execution_deps_through_facades() {
    let patterns = [
        "use crate::control::",
        "crate::control::",
        "use crate::headers::",
        "crate::headers::",
        "use crate::execution_runtime::",
        "crate::execution_runtime::",
    ];

    for root in ["src/ai_pipeline/planner", "src/ai_pipeline/finalize"] {
        assert_no_module_dependency_patterns(root, &patterns);
    }

    let control_payloads =
        read_workspace_file("apps/aether-gateway/src/ai_pipeline/contracts/control_payloads.rs");
    for pattern in patterns {
        assert!(
            !control_payloads.contains(pattern),
            "contracts/control_payloads.rs should route control/runtime dependencies through ai_pipeline facades, found {pattern}"
        );
    }

    let control_facade =
        read_workspace_file("apps/aether-gateway/src/ai_pipeline/control_facade.rs");
    for pattern in [
        "crate::control::resolve_execution_runtime_auth_context",
        "crate::headers::collect_control_headers",
        "crate::headers::is_json_request",
    ] {
        assert!(
            control_facade.contains(pattern),
            "control_facade.rs should own {pattern}"
        );
    }

    let execution_facade =
        read_workspace_file("apps/aether-gateway/src/ai_pipeline/execution_facade.rs");
    assert!(
        execution_facade
            .contains("crate::execution_runtime::maybe_build_local_sync_finalize_response"),
        "execution_facade.rs should own local sync finalize response bridging"
    );
}

#[test]
fn ai_pipeline_routes_provider_transport_deps_through_facade() {
    let patterns = [
        "use crate::provider_transport::",
        "crate::provider_transport::",
    ];

    for root in [
        "src/ai_pipeline/planner",
        "src/ai_pipeline/runtime",
        "src/ai_pipeline/conversion",
    ] {
        assert_no_module_dependency_patterns(root, &patterns);
    }

    let provider_transport_facade =
        read_workspace_file("apps/aether-gateway/src/ai_pipeline/provider_transport_facade.rs");
    for pattern in [
        "crate::provider_transport::auth",
        "crate::provider_transport::url",
        "crate::provider_transport::policy",
        "crate::provider_transport::snapshot",
    ] {
        assert!(
            provider_transport_facade.contains(pattern),
            "provider_transport_facade.rs should own {pattern}"
        );
    }
}

#[test]
fn ai_pipeline_planner_gateway_facade_is_split_by_role() {
    assert!(
        !workspace_file_exists("apps/aether-gateway/src/ai_pipeline/planner/gateway_facade.rs"),
        "planner/gateway_facade.rs should be removed after seam split"
    );

    for path in [
        "apps/aether-gateway/src/ai_pipeline/planner/auth_snapshot_facade.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/transport_facade.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/scheduler_facade.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/candidate_runtime_facade.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/executor_facade.rs",
    ] {
        assert!(
            workspace_file_exists(path),
            "{path} should exist after planner seam split"
        );
    }

    let auth_snapshot_facade =
        read_workspace_file("apps/aether-gateway/src/ai_pipeline/planner/auth_snapshot_facade.rs");
    assert!(
        auth_snapshot_facade.contains("read_auth_api_key_snapshot("),
        "auth_snapshot_facade.rs should own auth snapshot reads"
    );

    let transport_facade =
        read_workspace_file("apps/aether-gateway/src/ai_pipeline/planner/transport_facade.rs");
    for pattern in [
        "read_provider_transport_snapshot(",
        "resolve_local_oauth_request_auth(",
    ] {
        assert!(
            transport_facade.contains(pattern),
            "transport_facade.rs should own {pattern}"
        );
    }

    let scheduler_facade =
        read_workspace_file("apps/aether-gateway/src/ai_pipeline/planner/scheduler_facade.rs");
    for pattern in [
        "list_selectable_candidates(",
        "list_selectable_candidates_for_required_capability_without_requested_model(",
    ] {
        assert!(
            scheduler_facade.contains(pattern),
            "scheduler_facade.rs should own {pattern}"
        );
    }

    let candidate_runtime_facade = read_workspace_file(
        "apps/aether-gateway/src/ai_pipeline/planner/candidate_runtime_facade.rs",
    );
    for pattern in [
        "persist_available_local_candidate(",
        "persist_skipped_local_candidate(",
    ] {
        assert!(
            candidate_runtime_facade.contains(pattern),
            "candidate_runtime_facade.rs should own {pattern}"
        );
    }

    let executor_facade =
        read_workspace_file("apps/aether-gateway/src/ai_pipeline/planner/executor_facade.rs");
    assert!(
        executor_facade.contains("mark_unused_local_candidate_items("),
        "executor_facade.rs should own mark_unused_local_candidate_items"
    );
}

#[test]
fn ai_pipeline_m5_moves_contracts_and_route_logic_into_pipeline_crate() {
    for path in [
        "crates/aether-ai-pipeline/src/contracts/actions.rs",
        "crates/aether-ai-pipeline/src/contracts/plan_kinds.rs",
        "crates/aether-ai-pipeline/src/contracts/report_kinds.rs",
        "crates/aether-ai-pipeline/src/planner/route.rs",
    ] {
        assert!(
            workspace_file_exists(path),
            "{path} should exist after initial pipeline crate extraction"
        );
    }

    for path in [
        "apps/aether-gateway/src/ai_pipeline/contracts/actions.rs",
        "apps/aether-gateway/src/ai_pipeline/contracts/plan_kinds.rs",
        "apps/aether-gateway/src/ai_pipeline/contracts/report_kinds.rs",
    ] {
        assert!(
            !workspace_file_exists(path),
            "{path} should be removed after moving pipeline contract ownership"
        );
    }

    let gateway_contracts_mod =
        read_workspace_file("apps/aether-gateway/src/ai_pipeline/contracts/mod.rs");
    assert!(
        gateway_contracts_mod.contains("aether_ai_pipeline::contracts"),
        "gateway contracts/mod.rs should thinly re-export pipeline crate contracts"
    );

    let gateway_route = read_workspace_file("apps/aether-gateway/src/ai_pipeline/planner/route.rs");
    let gateway_route_runtime = gateway_route
        .split("#[cfg(test)]")
        .next()
        .unwrap_or(gateway_route.as_str());
    assert!(
        gateway_route_runtime.contains("aether_ai_pipeline::planner::route"),
        "planner/route.rs should delegate route logic to the pipeline crate"
    );
    for legacy_literal in [
        "\"openai_chat_stream\"",
        "\"openai_chat_sync\"",
        "\"gemini_files_upload\"",
        "\"openai_video_content\"",
    ] {
        assert!(
            !gateway_route_runtime.contains(legacy_literal),
            "planner/route.rs should not own hardcoded route resolution literal {legacy_literal}"
        );
    }
}

#[test]
fn ai_pipeline_planner_route_remains_control_only() {
    let gateway_route = read_workspace_file("apps/aether-gateway/src/ai_pipeline/planner/route.rs");
    let gateway_route_runtime = gateway_route
        .split("#[cfg(test)]")
        .next()
        .unwrap_or(gateway_route.as_str());

    for forbidden in [
        "crate::scheduler::",
        "crate::request_candidate_runtime::",
        "crate::provider_transport::",
        "crate::execution_runtime::",
    ] {
        assert!(
            !gateway_route_runtime.contains(forbidden),
            "planner/route.rs should not depend on {forbidden}"
        );
    }

    assert!(
        gateway_route_runtime.contains("GatewayControlDecision"),
        "planner/route.rs should stay as the thin adapter from control decisions"
    );
}

#[test]
fn ai_pipeline_conversion_error_is_owned_by_pipeline_crate() {
    assert!(
        !workspace_file_exists("apps/aether-gateway/src/ai_pipeline/conversion/error.rs"),
        "ai_pipeline/conversion/error.rs should move into aether-ai-pipeline"
    );

    let conversion_mod =
        read_workspace_file("apps/aether-gateway/src/ai_pipeline/conversion/mod.rs");
    assert!(
        conversion_mod.contains("aether_ai_pipeline::conversion"),
        "gateway conversion/mod.rs should thinly re-export pipeline conversion"
    );

    for forbidden in [
        "pub(crate) enum LocalCoreSyncErrorKind",
        "pub enum LocalCoreSyncErrorKind",
        "fn build_core_error_body_for_client_format(",
    ] {
        assert!(
            !conversion_mod.contains(forbidden),
            "gateway conversion/mod.rs should not own {forbidden}"
        );
    }
}

#[test]
fn ai_pipeline_conversion_request_is_owned_by_pipeline_crate() {
    assert!(
        workspace_file_exists("crates/aether-ai-pipeline/src/conversion/request/mod.rs"),
        "crates/aether-ai-pipeline/src/conversion/request/mod.rs should exist"
    );
    assert!(
        !workspace_file_exists(
            "apps/aether-gateway/src/ai_pipeline/conversion/request/from_openai_chat/claude.rs"
        ),
        "ai_pipeline/conversion/request/from_openai_chat should not remain in gateway"
    );
    assert!(
        !workspace_file_exists(
            "apps/aether-gateway/src/ai_pipeline/conversion/request/to_openai_chat/claude.rs"
        ),
        "ai_pipeline/conversion/request/to_openai_chat should not remain in gateway"
    );
    let conversion_request =
        read_workspace_file("apps/aether-gateway/src/ai_pipeline/conversion/request/mod.rs");
    assert!(
        conversion_request.contains("aether_ai_pipeline::conversion::request"),
        "gateway conversion/request/mod.rs should thinly re-export pipeline conversion request helpers"
    );
}

#[test]
fn ai_pipeline_conversion_response_is_owned_by_pipeline_crate() {
    assert!(
        workspace_file_exists("crates/aether-ai-pipeline/src/conversion/response/mod.rs"),
        "crates/aether-ai-pipeline/src/conversion/response/mod.rs should exist"
    );
    assert!(
        !workspace_file_exists(
            "apps/aether-gateway/src/ai_pipeline/conversion/response/from_openai_chat/claude_chat.rs"
        ),
        "ai_pipeline/conversion/response/from_openai_chat should not remain in gateway"
    );
    assert!(
        !workspace_file_exists(
            "apps/aether-gateway/src/ai_pipeline/conversion/response/to_openai_chat/claude_chat.rs"
        ),
        "ai_pipeline/conversion/response/to_openai_chat should not remain in gateway"
    );
    let conversion_response =
        read_workspace_file("apps/aether-gateway/src/ai_pipeline/conversion/response/mod.rs");
    assert!(
        conversion_response.contains("aether_ai_pipeline::conversion::response"),
        "gateway conversion/response/mod.rs should thinly re-export pipeline conversion response helpers"
    );
}

#[test]
fn ai_pipeline_finalize_standard_sync_response_converters_are_owned_by_pipeline_crate() {
    for path in [
        "apps/aether-gateway/src/ai_pipeline/finalize/standard/openai/sync/chat.rs",
        "apps/aether-gateway/src/ai_pipeline/finalize/standard/openai/sync/cli.rs",
        "apps/aether-gateway/src/ai_pipeline/finalize/standard/claude/sync/chat.rs",
        "apps/aether-gateway/src/ai_pipeline/finalize/standard/claude/sync/cli.rs",
        "apps/aether-gateway/src/ai_pipeline/finalize/standard/gemini/sync/chat.rs",
        "apps/aether-gateway/src/ai_pipeline/finalize/standard/gemini/sync/cli.rs",
    ] {
        assert!(
            !workspace_file_exists(path),
            "{path} should be deleted after sync finalize dispatch moved into pipeline-owned helpers"
        );
    }

    for (candidate_paths, symbol) in [
        (
            vec![
                "apps/aether-gateway/src/ai_pipeline/finalize/standard/openai/mod.rs",
                "apps/aether-gateway/src/ai_pipeline/finalize/standard/mod.rs",
            ],
            "convert_openai_cli_response_to_openai_chat",
        ),
        (
            vec![
                "apps/aether-gateway/src/ai_pipeline/finalize/standard/openai/mod.rs",
                "apps/aether-gateway/src/ai_pipeline/finalize/standard/mod.rs",
            ],
            "build_openai_cli_response",
        ),
        (
            vec![
                "apps/aether-gateway/src/ai_pipeline/finalize/standard/openai/mod.rs",
                "apps/aether-gateway/src/ai_pipeline/finalize/standard/mod.rs",
            ],
            "convert_openai_chat_response_to_openai_cli",
        ),
        (
            vec![
                "apps/aether-gateway/src/ai_pipeline/finalize/standard/claude/mod.rs",
                "apps/aether-gateway/src/ai_pipeline/finalize/standard/mod.rs",
            ],
            "convert_claude_chat_response_to_openai_chat",
        ),
        (
            vec![
                "apps/aether-gateway/src/ai_pipeline/finalize/standard/claude/mod.rs",
                "apps/aether-gateway/src/ai_pipeline/finalize/standard/mod.rs",
            ],
            "convert_openai_chat_response_to_claude_chat",
        ),
        (
            vec![
                "apps/aether-gateway/src/ai_pipeline/finalize/standard/claude/mod.rs",
                "apps/aether-gateway/src/ai_pipeline/finalize/standard/mod.rs",
            ],
            "convert_claude_cli_response_to_openai_cli",
        ),
        (
            vec![
                "apps/aether-gateway/src/ai_pipeline/finalize/standard/gemini/mod.rs",
                "apps/aether-gateway/src/ai_pipeline/finalize/standard/mod.rs",
            ],
            "convert_gemini_chat_response_to_openai_chat",
        ),
        (
            vec![
                "apps/aether-gateway/src/ai_pipeline/finalize/standard/gemini/mod.rs",
                "apps/aether-gateway/src/ai_pipeline/finalize/standard/mod.rs",
            ],
            "convert_openai_chat_response_to_gemini_chat",
        ),
        (
            vec![
                "apps/aether-gateway/src/ai_pipeline/finalize/standard/gemini/mod.rs",
                "apps/aether-gateway/src/ai_pipeline/finalize/standard/mod.rs",
            ],
            "convert_gemini_cli_response_to_openai_cli",
        ),
    ] {
        let sources = candidate_paths
            .iter()
            .map(|path| read_workspace_file(path))
            .collect::<Vec<_>>();
        assert!(
            sources.iter().any(|source| {
                source.contains("crate::ai_pipeline::conversion::response")
                    && source.contains(symbol)
            }),
            "{symbol} should stay exposed through conversion::response from a sync/mod.rs or outer mod.rs"
        );
    }
}

#[test]
fn ai_pipeline_finalize_stream_engine_is_owned_by_pipeline_crate() {
    for path in [
        "crates/aether-ai-pipeline/src/finalize/sse.rs",
        "crates/aether-ai-pipeline/src/finalize/standard/stream_core/common.rs",
        "crates/aether-ai-pipeline/src/finalize/standard/stream_core/format_matrix.rs",
        "crates/aether-ai-pipeline/src/finalize/standard/openai/stream.rs",
        "crates/aether-ai-pipeline/src/finalize/standard/claude/stream.rs",
        "crates/aether-ai-pipeline/src/finalize/standard/gemini/stream.rs",
    ] {
        assert!(
            workspace_file_exists(path),
            "{path} should exist in aether-ai-pipeline finalize engine"
        );
    }

    for (path, patterns) in [
        (
            "apps/aether-gateway/src/ai_pipeline/finalize/standard/openai/stream.rs",
            vec![
                "pub(crate) struct OpenAIChatProviderState",
                "pub(crate) struct OpenAICliProviderState",
                "pub(crate) struct OpenAIChatClientEmitter",
                "pub(crate) struct OpenAICliClientEmitter",
            ],
        ),
        (
            "apps/aether-gateway/src/ai_pipeline/finalize/standard/claude/stream.rs",
            vec![
                "pub(crate) struct ClaudeProviderState",
                "pub(crate) struct ClaudeClientEmitter",
            ],
        ),
        (
            "apps/aether-gateway/src/ai_pipeline/finalize/standard/gemini/stream.rs",
            vec![
                "pub(crate) struct GeminiProviderState",
                "pub(crate) struct GeminiClientEmitter",
            ],
        ),
    ] {
        let source = read_workspace_file(path);
        for pattern in patterns {
            assert!(
                !source.contains(pattern),
                "{path} should not keep {pattern} after pipeline finalize stream takeover"
            );
        }
    }

    let stream_common = read_workspace_file(
        "apps/aether-gateway/src/ai_pipeline/finalize/standard/stream_core/common.rs",
    );
    assert!(
        stream_common.contains(
            "pub(crate) use aether_ai_pipeline::finalize::standard::stream_core::common::*"
        ),
        "stream_core/common.rs should thinly re-export pipeline canonical stream helpers"
    );
    for pattern in [
        "pub(crate) struct CanonicalUsage",
        "pub(crate) enum CanonicalStreamEvent",
        "pub(crate) struct CanonicalStreamFrame",
        "pub(crate) fn decode_json_data_line",
    ] {
        assert!(
            !stream_common.contains(pattern),
            "stream_core/common.rs should not own {pattern} locally"
        );
    }

    let pipeline_format_matrix = read_workspace_file(
        "crates/aether-ai-pipeline/src/finalize/standard/stream_core/format_matrix.rs",
    );
    for pattern in [
        "pub struct StreamingStandardFormatMatrix",
        "enum ProviderStreamParser",
        "enum ClientStreamEmitter",
    ] {
        assert!(
            pipeline_format_matrix.contains(pattern),
            "pipeline stream_core/format_matrix.rs should own {pattern}"
        );
    }

    let gateway_stream_mod = read_workspace_file(
        "apps/aether-gateway/src/ai_pipeline/finalize/standard/stream_core/mod.rs",
    );
    for pattern in [
        "pub(crate) enum ProviderStreamParser",
        "pub(crate) enum ClientStreamEmitter",
        "impl ProviderStreamParser",
        "impl ClientStreamEmitter",
    ] {
        assert!(
            !gateway_stream_mod.contains(pattern),
            "gateway stream_core/mod.rs should not keep local format-matrix owner {pattern}"
        );
    }

    let gateway_orchestrator = read_workspace_file(
        "apps/aether-gateway/src/ai_pipeline/finalize/standard/stream_core/orchestrator.rs",
    );
    assert!(
        gateway_orchestrator.contains("StreamingStandardFormatMatrix"),
        "gateway stream_core/orchestrator.rs should delegate format matrix selection to aether-ai-pipeline"
    );
}

#[test]
fn ai_pipeline_finalize_standard_sync_products_are_owned_by_pipeline_crate() {
    assert!(
        workspace_file_exists("crates/aether-ai-pipeline/src/finalize/sync_products.rs"),
        "finalize sync_products should live in aether-ai-pipeline"
    );

    let pipeline_sync_products =
        read_workspace_file("crates/aether-ai-pipeline/src/finalize/sync_products.rs");
    for expected in [
        "pub fn maybe_build_standard_cross_format_sync_product_from_normalized_payload(",
        "pub fn maybe_build_standard_same_format_sync_body_from_normalized_payload(",
        "pub fn maybe_build_openai_cli_same_family_sync_body_from_normalized_payload(",
        "pub fn maybe_build_openai_chat_cross_format_sync_product_from_normalized_payload(",
        "pub fn maybe_build_openai_cli_cross_format_sync_product_from_normalized_payload(",
        "pub fn maybe_build_standard_sync_finalize_product_from_normalized_payload(",
        "pub fn aggregate_standard_chat_stream_sync_response(",
        "pub fn aggregate_standard_cli_stream_sync_response(",
        "pub fn aggregate_openai_chat_stream_sync_response(",
        "pub fn aggregate_openai_cli_stream_sync_response(",
        "pub fn aggregate_claude_stream_sync_response(",
        "pub fn aggregate_gemini_stream_sync_response(",
        "pub fn convert_standard_chat_response(",
        "pub fn convert_standard_cli_response(",
        "pub fn maybe_build_standard_cross_format_sync_product(",
        "pub struct StandardCrossFormatSyncProduct",
        "pub enum StandardSyncFinalizeNormalizedProduct",
        "fn parse_stream_json_events(",
    ] {
        assert!(
            pipeline_sync_products.contains(expected),
            "pipeline finalize sync_products should own {expected}"
        );
    }

    let gateway_standard =
        read_workspace_file("apps/aether-gateway/src/ai_pipeline/finalize/standard/mod.rs");
    assert!(
        gateway_standard.contains("aether_ai_pipeline::finalize::sync_products"),
        "gateway finalize/standard/mod.rs should thinly re-export sync_products from aether-ai-pipeline"
    );
    for forbidden in [
        "pub(crate) fn aggregate_standard_chat_stream_sync_response(",
        "pub(crate) fn aggregate_standard_cli_stream_sync_response(",
        "pub(crate) fn convert_standard_chat_response(",
        "pub(crate) fn convert_standard_cli_response(",
    ] {
        assert!(
            !gateway_standard.contains(forbidden),
            "gateway finalize/standard/mod.rs should not own {forbidden}"
        );
    }

    let gateway_finalize_common =
        read_workspace_file("apps/aether-gateway/src/ai_pipeline/finalize/common.rs");
    assert!(
        !gateway_finalize_common.contains("pub(crate) fn parse_stream_json_events("),
        "gateway finalize/common.rs should not keep parse_stream_json_events after sync_products takeover"
    );

    for path in [
        "apps/aether-gateway/src/ai_pipeline/finalize/standard/openai/sync/mod.rs",
        "apps/aether-gateway/src/ai_pipeline/finalize/standard/claude/sync/mod.rs",
        "apps/aether-gateway/src/ai_pipeline/finalize/standard/gemini/sync/mod.rs",
    ] {
        assert!(
            !workspace_file_exists(path),
            "{path} should be deleted after sync wrapper flattening"
        );
    }

    for (path, forbidden) in [
        (
            "apps/aether-gateway/src/ai_pipeline/finalize/standard/openai/mod.rs",
            "aggregate_openai_chat_stream_sync_response",
        ),
        (
            "apps/aether-gateway/src/ai_pipeline/finalize/standard/openai/mod.rs",
            "build_openai_cli_response",
        ),
        (
            "apps/aether-gateway/src/ai_pipeline/finalize/standard/claude/mod.rs",
            "aggregate_claude_stream_sync_response",
        ),
        (
            "apps/aether-gateway/src/ai_pipeline/finalize/standard/gemini/mod.rs",
            "aggregate_gemini_stream_sync_response",
        ),
        (
            "apps/aether-gateway/src/ai_pipeline/finalize/standard/mod.rs",
            "pub(crate) use openai::*;",
        ),
        (
            "apps/aether-gateway/src/ai_pipeline/finalize/standard/mod.rs",
            "pub(crate) use claude::*;",
        ),
        (
            "apps/aether-gateway/src/ai_pipeline/finalize/standard/mod.rs",
            "pub(crate) use gemini::*;",
        ),
    ] {
        let source = read_workspace_file(path);
        assert!(
            !source.contains(forbidden),
            "{path} should not keep dead standard re-export {forbidden}"
        );
    }

    let gateway_internal_sync = read_workspace_file(
        "apps/aether-gateway/src/ai_pipeline/finalize/internal/sync_finalize.rs",
    );
    assert!(
        gateway_internal_sync.contains(
            "maybe_build_standard_sync_finalize_product_from_normalized_payload"
        ),
        "gateway internal/sync_finalize.rs should delegate normalized standard sync finalize dispatch to aether-ai-pipeline"
    );
    for forbidden in [
        "maybe_build_local_openai_chat_stream_sync_response(",
        "maybe_build_local_openai_chat_sync_response(",
        "maybe_build_local_openai_chat_cross_format_stream_sync_response(",
        "maybe_build_local_openai_cli_stream_sync_response(",
        "maybe_build_local_openai_cli_cross_format_stream_sync_response(",
        "maybe_build_local_claude_cli_stream_sync_response(",
        "maybe_build_local_gemini_cli_stream_sync_response(",
        "maybe_build_local_claude_stream_sync_response(",
        "maybe_build_local_claude_sync_response(",
        "maybe_build_local_gemini_stream_sync_response(",
        "maybe_build_local_gemini_sync_response(",
        "maybe_build_local_openai_chat_cross_format_sync_response(",
        "maybe_build_local_openai_cli_cross_format_sync_response(",
    ] {
        assert!(
            !gateway_internal_sync.contains(forbidden),
            "gateway internal/sync_finalize.rs should not keep ordered wrapper dispatch detail {forbidden}"
        );
    }
}

#[test]
fn ai_pipeline_finalize_stream_rewrite_matrix_is_owned_by_pipeline_crate() {
    assert!(
        workspace_file_exists("crates/aether-ai-pipeline/src/finalize/stream_rewrite.rs"),
        "finalize stream rewrite matrix should live in aether-ai-pipeline"
    );

    let gateway_stream_rewrite = read_workspace_file(
        "apps/aether-gateway/src/ai_pipeline/finalize/internal/stream_rewrite.rs",
    );
    assert!(
        gateway_stream_rewrite.contains("aether_ai_pipeline::finalize"),
        "gateway internal stream_rewrite should delegate rewrite-mode resolution to aether-ai-pipeline"
    );
    assert!(
        gateway_stream_rewrite.contains("resolve_finalize_stream_rewrite_mode"),
        "gateway internal stream_rewrite should resolve rewrite mode through pipeline crate"
    );

    for forbidden in [
        "fn is_standard_provider_api_format(",
        "fn is_standard_chat_client_api_format(",
        "fn is_standard_cli_client_api_format(",
        ".get(\"provider_api_format\")",
        ".get(\"client_api_format\")",
        ".get(\"needs_conversion\")",
        ".get(\"envelope_name\")",
    ] {
        assert!(
            !gateway_stream_rewrite.contains(forbidden),
            "gateway internal stream_rewrite should not own rewrite-matrix detail {forbidden}"
        );
    }
}

#[test]
fn ai_pipeline_planner_common_parser_is_owned_by_pipeline_crate() {
    assert!(
        workspace_file_exists("crates/aether-ai-pipeline/src/planner/common.rs"),
        "planner/common pure parser should exist in aether-ai-pipeline"
    );

    let gateway_common =
        read_workspace_file("apps/aether-gateway/src/ai_pipeline/planner/common.rs");
    let gateway_common_runtime = gateway_common
        .split("#[cfg(test)]")
        .next()
        .unwrap_or(gateway_common.as_str());

    assert!(
        gateway_common_runtime.contains("aether_ai_pipeline::planner::common"),
        "gateway planner/common.rs should delegate body parsing to the pipeline crate"
    );

    for forbidden in [
        "serde_json::from_slice::<serde_json::Value>",
        "base64::engine::general_purpose::STANDARD.encode",
    ] {
        assert!(
            !gateway_common_runtime.contains(forbidden),
            "gateway planner/common.rs should not own parser implementation detail {forbidden}"
        );
    }
}

#[test]
fn ai_pipeline_planner_standard_normalize_is_owned_by_pipeline_crate() {
    assert!(
        workspace_file_exists("crates/aether-ai-pipeline/src/planner/standard/normalize.rs"),
        "planner/standard/normalize should live in aether-ai-pipeline"
    );

    let gateway_normalize =
        read_workspace_file("apps/aether-gateway/src/ai_pipeline/planner/standard/normalize.rs");
    assert!(
        gateway_normalize.contains("aether_ai_pipeline::planner::standard::normalize"),
        "gateway normalize.rs should delegate to pipeline standard normalize helpers"
    );

    for forbidden in [
        "serde_json::Map::from_iter",
        "normalize_openai_cli_request_to_openai_chat_request",
        "parse_openai_tool_result_content",
    ] {
        assert!(
            !gateway_normalize.contains(forbidden),
            "gateway normalize.rs should not keep helper implementation detail {forbidden}"
        );
    }
}

#[test]
fn ai_pipeline_openai_helpers_are_owned_by_pipeline_crate() {
    assert!(
        workspace_file_exists("crates/aether-ai-pipeline/src/planner/openai.rs"),
        "planner/openai helper owner should exist in aether-ai-pipeline"
    );

    let gateway_openai_mod =
        read_workspace_file("apps/aether-gateway/src/ai_pipeline/planner/standard/openai/mod.rs");
    assert!(
        gateway_openai_mod.contains("aether_ai_pipeline::planner::openai"),
        "gateway planner/standard/openai/mod.rs should thinly re-export pipeline openai helpers"
    );

    let gateway_openai_chat = read_workspace_file(
        "apps/aether-gateway/src/ai_pipeline/planner/standard/openai/chat/mod.rs",
    );
    for forbidden in [
        "pub(crate) fn parse_openai_stop_sequences(",
        "pub(crate) fn resolve_openai_chat_max_tokens(",
        "pub(crate) fn value_as_u64(",
        "pub(crate) fn copy_request_number_field(",
        "pub(crate) fn copy_request_number_field_as(",
        "pub(crate) fn map_openai_reasoning_effort_to_claude_output(",
        "pub(crate) fn map_openai_reasoning_effort_to_gemini_budget(",
    ] {
        assert!(
            !gateway_openai_chat.contains(forbidden),
            "gateway planner/standard/openai/chat/mod.rs should not own helper {forbidden}"
        );
    }
}

#[test]
fn ai_pipeline_matrix_conversion_is_owned_by_pipeline_crate() {
    assert!(
        workspace_file_exists("crates/aether-ai-pipeline/src/planner/matrix.rs"),
        "planner/matrix facade should live in aether-ai-pipeline"
    );
    assert!(
        workspace_file_exists("crates/aether-ai-pipeline/src/planner/standard/matrix.rs"),
        "planner/standard/matrix owner should live in aether-ai-pipeline"
    );

    let matrix =
        read_workspace_file("apps/aether-gateway/src/ai_pipeline/planner/standard/matrix.rs");
    assert!(
        matrix.contains("aether_ai_pipeline::planner::matrix"),
        "planner/standard/matrix.rs should delegate canonical conversion to the pipeline crate"
    );
    for forbidden in [
        "normalize_openai_cli_request_to_openai_chat_request",
        "normalize_claude_request_to_openai_chat_request",
        "normalize_gemini_request_to_openai_chat_request",
        "serde_json::Map::from_iter",
    ] {
        assert!(
            !matrix.contains(forbidden),
            "planner/standard/matrix.rs should not keep conversion helper {forbidden}"
        );
    }
}

#[test]
fn ai_pipeline_standard_family_specs_are_owned_by_pipeline_crate() {
    assert!(
        workspace_file_exists("crates/aether-ai-pipeline/src/planner/standard/family.rs"),
        "planner/standard/family pure spec owner should live in aether-ai-pipeline"
    );
    assert!(
        workspace_file_exists("crates/aether-ai-pipeline/src/planner/standard/claude/chat.rs"),
        "planner/standard/claude/chat pure spec resolver should live in aether-ai-pipeline"
    );
    assert!(
        workspace_file_exists("crates/aether-ai-pipeline/src/planner/standard/claude/cli.rs"),
        "planner/standard/claude/cli pure spec resolver should live in aether-ai-pipeline"
    );
    assert!(
        workspace_file_exists("crates/aether-ai-pipeline/src/planner/standard/gemini/chat.rs"),
        "planner/standard/gemini/chat pure spec resolver should live in aether-ai-pipeline"
    );
    assert!(
        workspace_file_exists("crates/aether-ai-pipeline/src/planner/standard/gemini/cli.rs"),
        "planner/standard/gemini/cli pure spec resolver should live in aether-ai-pipeline"
    );

    let family_types =
        read_workspace_file("apps/aether-gateway/src/ai_pipeline/planner/standard/family/types.rs");
    assert!(
        family_types.contains("aether_ai_pipeline::planner::standard::family"),
        "gateway planner/standard/family/types.rs should re-export pure family spec types from the pipeline crate"
    );
    for forbidden in [
        "pub(crate) enum LocalStandardSourceFamily",
        "pub(crate) enum LocalStandardSourceMode",
        "pub(crate) struct LocalStandardSpec",
    ] {
        assert!(
            !family_types.contains(forbidden),
            "gateway planner/standard/family/types.rs should not own pure spec type {forbidden}"
        );
    }

    for path in [
        "apps/aether-gateway/src/ai_pipeline/planner/standard/claude/chat.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/standard/claude/cli.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/standard/gemini/chat.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/standard/gemini/cli.rs",
    ] {
        assert!(
            !workspace_file_exists(path),
            "{path} should be removed after moving pure spec resolvers into the pipeline crate"
        );
    }

    for (path, expected) in [
        (
            "apps/aether-gateway/src/ai_pipeline/planner/standard/claude/mod.rs",
            "aether_ai_pipeline::planner::standard::claude",
        ),
        (
            "apps/aether-gateway/src/ai_pipeline/planner/standard/gemini/mod.rs",
            "aether_ai_pipeline::planner::standard::gemini",
        ),
    ] {
        let source = read_workspace_file(path);
        assert!(
            source.contains(expected),
            "{path} should delegate pure standard-family spec resolution to the pipeline crate"
        );
        for forbidden in [
            "LocalStandardSpec {",
            "report_kind:",
            "require_streaming:",
            "pub(crate) mod chat;",
            "pub(crate) mod cli;",
        ] {
            assert!(
                !source.contains(forbidden),
                "{path} should not own spec construction detail {forbidden}"
            );
        }
    }
}

#[test]
fn ai_pipeline_same_format_provider_specs_are_owned_by_pipeline_crate() {
    assert!(
        workspace_file_exists("crates/aether-ai-pipeline/src/planner/passthrough/provider.rs"),
        "planner/passthrough/provider pure spec owner should live in aether-ai-pipeline"
    );

    let family_types = read_workspace_file(
        "apps/aether-gateway/src/ai_pipeline/planner/passthrough/provider/family/types.rs",
    );
    assert!(
        family_types.contains("aether_ai_pipeline::planner::passthrough::provider"),
        "gateway passthrough/provider/family/types.rs should re-export pure same-format provider spec types from the pipeline crate"
    );
    for forbidden in [
        "pub(crate) enum LocalSameFormatProviderFamily",
        "pub(crate) struct LocalSameFormatProviderSpec",
    ] {
        assert!(
            !family_types.contains(forbidden),
            "gateway passthrough/provider/family/types.rs should not own pure same-format type {forbidden}"
        );
    }

    let plans = read_workspace_file(
        "apps/aether-gateway/src/ai_pipeline/planner/passthrough/provider/plans.rs",
    );
    assert!(
        plans.contains("aether_ai_pipeline::planner::passthrough::provider"),
        "gateway passthrough/provider/plans.rs should delegate same-format spec resolution to the pipeline crate"
    );
    for forbidden in [
        "claude_chat_sync_success",
        "gemini_cli_stream_success",
        "pub(crate) fn resolve_sync_spec(",
        "pub(crate) fn resolve_stream_spec(",
    ] {
        assert!(
            !plans.contains(forbidden),
            "gateway passthrough/provider/plans.rs should not own same-format resolver detail {forbidden}"
        );
    }
}

#[test]
fn ai_pipeline_passthrough_provider_specs_are_owned_by_pipeline_crate() {
    assert!(
        workspace_file_exists("crates/aether-ai-pipeline/src/planner/passthrough/provider.rs"),
        "planner/passthrough/provider pure spec owner should live in aether-ai-pipeline"
    );

    let family_types = read_workspace_file(
        "apps/aether-gateway/src/ai_pipeline/planner/passthrough/provider/family/types.rs",
    );
    assert!(
        family_types.contains("aether_ai_pipeline::planner::passthrough::provider"),
        "gateway passthrough/provider/family/types.rs should re-export pure spec types from the pipeline crate"
    );
    for forbidden in [
        "pub(crate) enum LocalSameFormatProviderFamily",
        "pub(crate) struct LocalSameFormatProviderSpec",
    ] {
        assert!(
            !family_types.contains(forbidden),
            "gateway passthrough/provider/family/types.rs should not own pure spec type {forbidden}"
        );
    }

    let plans = read_workspace_file(
        "apps/aether-gateway/src/ai_pipeline/planner/passthrough/provider/plans.rs",
    );
    assert!(
        plans.contains("aether_ai_pipeline::planner::passthrough::provider"),
        "gateway passthrough/provider/plans.rs should delegate same-format spec resolution to the pipeline crate"
    );
    for forbidden in [
        "pub(crate) fn resolve_sync_spec(",
        "pub(crate) fn resolve_stream_spec(",
        "CLAUDE_CHAT_SYNC_PLAN_KIND",
        "GEMINI_CLI_STREAM_PLAN_KIND",
        "LocalSameFormatProviderSpec {",
    ] {
        assert!(
            !plans.contains(forbidden),
            "gateway passthrough/provider/plans.rs should not keep pure spec resolver detail {forbidden}"
        );
    }
}

#[test]
fn ai_pipeline_specialized_files_specs_are_owned_by_pipeline_crate() {
    assert!(
        workspace_file_exists("crates/aether-ai-pipeline/src/planner/specialized/files.rs"),
        "planner/specialized/files pure spec owner should live in aether-ai-pipeline"
    );

    let files =
        read_workspace_file("apps/aether-gateway/src/ai_pipeline/planner/specialized/files.rs");
    assert!(
        files.contains("aether_ai_pipeline::planner::specialized::files"),
        "gateway planner/specialized/files.rs should delegate pure specialized-files spec resolution to the pipeline crate"
    );
    for forbidden in [
        "struct LocalGeminiFilesSpec",
        "fn resolve_sync_spec(",
        "fn resolve_stream_spec(",
        "Some(LocalGeminiFilesSpec {",
        "GEMINI_FILES_LIST_PLAN_KIND",
        "GEMINI_FILES_GET_PLAN_KIND",
        "GEMINI_FILES_DELETE_PLAN_KIND",
        "GEMINI_FILES_DOWNLOAD_PLAN_KIND",
    ] {
        assert!(
            !files.contains(forbidden),
            "gateway planner/specialized/files.rs should not keep pure specialized-files resolver detail {forbidden}"
        );
    }
}

#[test]
fn ai_pipeline_specialized_video_specs_are_owned_by_pipeline_crate() {
    assert!(
        workspace_file_exists("crates/aether-ai-pipeline/src/planner/specialized/video.rs"),
        "planner/specialized/video pure spec owner should live in aether-ai-pipeline"
    );

    let video =
        read_workspace_file("apps/aether-gateway/src/ai_pipeline/planner/specialized/video.rs");
    assert!(
        video.contains("aether_ai_pipeline::planner::specialized::video"),
        "gateway planner/specialized/video.rs should delegate pure specialized-video spec resolution to the pipeline crate"
    );
    for forbidden in [
        "enum LocalVideoCreateFamily",
        "struct LocalVideoCreateSpec",
        "fn resolve_sync_spec(",
        "Some(LocalVideoCreateSpec {",
        "OPENAI_VIDEO_CREATE_SYNC_PLAN_KIND",
        "GEMINI_VIDEO_CREATE_SYNC_PLAN_KIND",
    ] {
        assert!(
            !video.contains(forbidden),
            "gateway planner/specialized/video.rs should not keep pure specialized-video resolver detail {forbidden}"
        );
    }
}

#[test]
fn ai_pipeline_openai_cli_specs_are_owned_by_pipeline_crate() {
    assert!(
        workspace_file_exists("crates/aether-ai-pipeline/src/planner/standard/openai_cli.rs"),
        "planner/standard/openai_cli pure spec owner should live in aether-ai-pipeline"
    );

    let decision = read_workspace_file(
        "apps/aether-gateway/src/ai_pipeline/planner/standard/openai/cli/decision.rs",
    );
    assert!(
        decision.contains("aether_ai_pipeline::planner::standard::openai_cli"),
        "gateway planner/standard/openai/cli/decision.rs should re-export pure openai-cli spec type from the pipeline crate"
    );
    assert!(
        !decision.contains("pub(super) struct LocalOpenAiCliSpec"),
        "gateway planner/standard/openai/cli/decision.rs should not own LocalOpenAiCliSpec"
    );

    let plans = read_workspace_file(
        "apps/aether-gateway/src/ai_pipeline/planner/standard/openai/cli/plans.rs",
    );
    assert!(
        plans.contains("aether_ai_pipeline::planner::standard::openai_cli"),
        "gateway planner/standard/openai/cli/plans.rs should delegate openai-cli spec resolution to the pipeline crate"
    );
    for forbidden in [
        "fn resolve_sync_spec(",
        "fn resolve_stream_spec(",
        "OPENAI_CLI_SYNC_PLAN_KIND",
        "OPENAI_COMPACT_STREAM_PLAN_KIND",
        "LocalOpenAiCliSpec {",
    ] {
        assert!(
            !plans.contains(forbidden),
            "gateway planner/standard/openai/cli/plans.rs should not keep pure openai-cli resolver detail {forbidden}"
        );
    }
}

#[test]
fn gateway_request_audit_bundle_type_is_owned_by_aether_data() {
    let usage_http = read_workspace_file("apps/aether-gateway/src/usage/http.rs");
    assert!(
        usage_http.contains("aether_data::repository::audit::RequestAuditBundle"),
        "usage/http.rs should depend on aether-data request audit bundle type"
    );
    assert!(
        usage_http.contains("aether_data_contracts::repository::usage::StoredRequestUsageAudit"),
        "usage/http.rs should depend on aether-data-contracts usage audit type"
    );

    let auth_api_keys =
        read_workspace_file("apps/aether-gateway/src/state/runtime/auth/api_keys.rs");
    assert!(
        !auth_api_keys.contains("aether_data::repository::audit::RequestAuditBundle"),
        "state/runtime/auth/api_keys.rs should not keep request audit bundle read wrapper anymore"
    );
    assert!(
        !auth_api_keys.contains("aether_data::repository::usage::StoredRequestUsageAudit"),
        "state/runtime/auth/api_keys.rs should not keep usage audit read wrapper anymore"
    );

    let usage_mod = read_workspace_file("apps/aether-gateway/src/usage/mod.rs");
    for pattern in ["mod bundle;", "mod read;"] {
        assert!(
            !usage_mod.contains(pattern),
            "usage/mod.rs should not keep local audit compatibility modules {pattern}"
        );
    }

    let audit_types = read_workspace_file("crates/aether-data/src/repository/audit.rs");
    for pattern in [
        "pub struct RequestAuditBundle",
        "pub trait RequestAuditReader",
        "pub async fn read_request_audit_bundle(",
    ] {
        assert!(
            audit_types.contains(pattern),
            "aether-data request audit module should own {pattern}"
        );
    }
}

#[test]
fn request_candidate_runtime_paths_depend_on_scheduler_core() {
    let clock = read_workspace_file("apps/aether-gateway/src/clock.rs");
    let request_candidate_runtime =
        read_workspace_file("apps/aether-gateway/src/request_candidate_runtime.rs");
    let runtime_request_candidate = request_candidate_runtime
        .split("#[cfg(test)]")
        .next()
        .unwrap_or(request_candidate_runtime.as_str());
    assert!(
        request_candidate_runtime.contains("aether_scheduler_core"),
        "request_candidate_runtime.rs should depend on aether-scheduler-core"
    );
    assert!(
        request_candidate_runtime.contains("RequestCandidateRuntimeReader"),
        "request_candidate_runtime.rs should depend on RequestCandidateRuntimeReader"
    );
    assert!(
        request_candidate_runtime.contains("RequestCandidateRuntimeWriter"),
        "request_candidate_runtime.rs should depend on RequestCandidateRuntimeWriter"
    );
    for pattern in [
        "parse_request_candidate_report_context",
        "resolve_report_request_candidate_slot_from_candidates",
        "build_execution_request_candidate_seed",
        "finalize_execution_request_candidate_report_context",
        "build_local_request_candidate_status_record",
        "build_report_request_candidate_status_record",
        "persist_available_local_candidate",
        "persist_skipped_local_candidate",
        "build_locally_actionable_report_context_from_request_candidate",
        "resolve_locally_actionable_request_candidate_report_context",
    ] {
        assert!(
            runtime_request_candidate.contains(pattern),
            "request_candidate_runtime.rs should depend on shared helper {pattern}"
        );
    }
    for pattern in [
        "fn match_existing_report_candidate(",
        "fn next_candidate_index(",
        "fn build_report_candidate_extra_data(",
        "fn is_terminal_candidate_status(",
        "parse_report_context(report_context)?",
        "let mut context = report_context",
        "context.insert(\"request_id\"",
        "use crate::AppState",
        "pub(crate) use aether_scheduler_core::execution_error_details",
        "pub(crate) fn current_unix_secs()",
    ] {
        assert!(
            !runtime_request_candidate.contains(pattern),
            "request_candidate_runtime.rs should not own {pattern}"
        );
    }
    assert!(
        request_candidate_runtime.contains("pub(crate) trait RequestCandidateRuntimeReader"),
        "request_candidate_runtime.rs should host RequestCandidateRuntimeReader"
    );
    assert!(
        request_candidate_runtime.contains("pub(crate) trait RequestCandidateRuntimeWriter"),
        "request_candidate_runtime.rs should host RequestCandidateRuntimeWriter"
    );
    assert!(
        !request_candidate_runtime.contains("impl RequestCandidateRuntimeReader for AppState"),
        "request_candidate_runtime.rs should not host AppState reader impl anymore"
    );
    assert!(
        !request_candidate_runtime.contains("impl RequestCandidateRuntimeWriter for AppState"),
        "request_candidate_runtime.rs should not host AppState writer impl anymore"
    );
    assert!(
        clock.contains("pub(crate) fn current_unix_secs()"),
        "clock.rs should host current_unix_secs"
    );

    let state_integrations = read_workspace_file("apps/aether-gateway/src/state/integrations.rs");
    assert!(
        state_integrations.contains("impl RequestCandidateRuntimeReader for AppState"),
        "state/integrations.rs should host RequestCandidateRuntimeReader for AppState"
    );
    assert!(
        state_integrations.contains("impl RequestCandidateRuntimeWriter for AppState"),
        "state/integrations.rs should host RequestCandidateRuntimeWriter for AppState"
    );
}

#[test]
fn gateway_data_state_does_not_depend_on_scheduler_candidate_selection() {
    let state_mod = read_workspace_file("apps/aether-gateway/src/data/state/mod.rs");
    let state_runtime = read_workspace_file("apps/aether-gateway/src/data/state/runtime.rs");
    let runtime_mod = read_workspace_file("apps/aether-gateway/src/state/runtime/mod.rs");
    let auth_api_keys =
        read_workspace_file("apps/aether-gateway/src/state/runtime/auth/api_keys.rs");

    assert!(
        !state_mod.contains("read_minimal_candidate_selection"),
        "data/state/mod.rs should not import scheduler candidate selection entrypoints"
    );
    assert!(
        !state_runtime.contains("pub(crate) async fn read_minimal_candidate_selection("),
        "data/state/runtime.rs should not own scheduler minimal candidate derived read"
    );
    assert!(
        !auth_api_keys.contains("read_minimal_candidate_selection("),
        "state/runtime/auth/api_keys.rs should not keep scheduler minimal candidate wrapper anymore"
    );
    for pattern in [
        "pub(crate) async fn read_request_candidate_trace(",
        "pub(crate) async fn read_decision_trace(",
        "pub(crate) async fn read_request_usage_audit(",
        "pub(crate) async fn find_request_usage_by_id(",
        "pub(crate) async fn read_request_audit_bundle(",
        "pub(crate) async fn read_auth_api_key_snapshot(",
        "pub(crate) async fn read_auth_api_key_snapshots_by_ids(",
        "pub(crate) async fn read_auth_api_key_snapshot_by_key_hash(",
    ] {
        assert!(
            !auth_api_keys.contains(pattern),
            "state/runtime/auth/api_keys.rs should not keep low-value data read wrapper {pattern}"
        );
    }
    assert!(
        !runtime_mod.contains("mod audit;"),
        "state/runtime/mod.rs should not keep legacy audit runtime wiring"
    );
}

#[test]
fn model_fetch_runtime_paths_depend_on_shared_crates_not_local_pure_helpers() {
    let runtime = read_workspace_file("apps/aether-gateway/src/model_fetch/runtime.rs");
    assert!(
        runtime.contains("aether_model_fetch"),
        "model_fetch/runtime.rs should depend on aether_model_fetch"
    );
    assert!(
        runtime.contains("ModelFetchRuntimeState"),
        "model_fetch/runtime.rs should depend on ModelFetchRuntimeState"
    );
    assert!(
        runtime.contains("build_models_fetch_execution_plan"),
        "model_fetch/runtime.rs should depend on shared models fetch plan builder"
    );
    for pattern in [
        "fn apply_model_filters(",
        "fn aggregate_models_for_cache(",
        "fn build_models_fetch_url(",
        "fn build_models_fetch_execution_plan(",
        "fn parse_models_response(",
        "fn select_models_fetch_endpoint(",
        "fn model_fetch_interval_minutes(",
        "fn resolve_models_fetch_auth(",
        "state.data.has_provider_catalog_reader()",
        "execute_execution_runtime_sync_plan(state, None, &plan)",
        "resolve_local_standard_auth(",
        "resolve_local_gemini_auth(",
        "resolve_local_openai_chat_auth(",
        "resolve_local_vertex_api_key_query_auth(",
        "apply_local_header_rules(",
        "ensure_upstream_auth_header(",
    ] {
        assert!(
            !runtime.contains(pattern),
            "model_fetch/runtime.rs should not own {pattern}"
        );
    }

    assert!(
        runtime.contains("sync_provider_model_whitelist_associations"),
        "model_fetch/runtime.rs should call shared whitelist sync helper"
    );
    assert!(
        !runtime.contains("mod association_sync;"),
        "model_fetch/runtime.rs should not keep a local association_sync module"
    );
    assert!(
        !runtime.contains("fn sync_provider_model_whitelist_associations("),
        "model_fetch/runtime.rs should not own whitelist sync logic"
    );

    let runtime_state = read_workspace_file("apps/aether-gateway/src/model_fetch/runtime/state.rs");
    assert!(
        runtime_state.contains("pub(crate) trait ModelFetchRuntimeState"),
        "model_fetch/runtime/state.rs should host the runtime state trait definition"
    );
    for pattern in [
        "impl ModelFetchTransportRuntime for AppState",
        "impl ModelFetchRuntimeState for AppState",
        "impl ModelFetchAssociationStore for AppState",
    ] {
        assert!(
            !runtime_state.contains(pattern),
            "model_fetch/runtime/state.rs should not host {pattern} anymore"
        );
    }

    let state_integrations = read_workspace_file("apps/aether-gateway/src/state/integrations.rs");
    for pattern in [
        "impl provider_transport::TransportTunnelAffinityLookup for AppState",
        "impl ModelFetchTransportRuntime for AppState",
        "impl ModelFetchRuntimeState for AppState",
        "impl ModelFetchAssociationStore for AppState",
    ] {
        assert!(
            state_integrations.contains(pattern),
            "state/integrations.rs should host {pattern}"
        );
    }

    let app_state = read_workspace_file("apps/aether-gateway/src/state/app.rs");
    assert!(
        !app_state.contains("impl provider_transport::TransportTunnelAffinityLookup for AppState"),
        "state/app.rs should not host provider transport integration impls anymore"
    );

    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root should resolve");
    let association_sync_path =
        workspace_root.join("apps/aether-gateway/src/model_fetch/runtime/association_sync.rs");
    assert!(
        !association_sync_path.exists(),
        "model_fetch/runtime/association_sync.rs should be removed after extraction"
    );
}

#[test]
fn video_task_helpers_depend_on_shared_core_crate() {
    let types = read_workspace_file("apps/aether-gateway/src/video_tasks/types.rs");
    assert!(
        types.contains("aether_video_tasks_core"),
        "video_tasks/types.rs should depend on aether-video-tasks-core"
    );
    for pattern in [
        "pub(crate) struct LocalVideoTaskTransport",
        "pub(crate) struct LocalVideoTaskPersistence",
        "pub(crate) struct OpenAiVideoTaskSeed",
        "pub(crate) struct GeminiVideoTaskSeed",
        "pub(crate) enum LocalVideoTaskSnapshot",
    ] {
        assert!(
            !types.contains(pattern),
            "video_tasks/types.rs should not own {pattern}"
        );
    }

    let body = read_workspace_file("apps/aether-gateway/src/video_tasks/helpers/body.rs");
    assert!(
        body.contains("aether_video_tasks_core"),
        "video_tasks/helpers/body.rs should depend on aether-video-tasks-core"
    );
    for pattern in [
        "fn context_text(",
        "fn context_u64(",
        "fn request_body_text(",
        "fn request_body_string(",
        "fn request_body_u32(",
    ] {
        assert!(
            !body.contains(pattern),
            "video_tasks/helpers/body.rs should not own {pattern}"
        );
    }

    let path = read_workspace_file("apps/aether-gateway/src/video_tasks/helpers/path.rs");
    assert!(
        path.contains("aether_video_tasks_core"),
        "video_tasks/helpers/path.rs should depend on aether-video-tasks-core"
    );
    for pattern in [
        "fn extract_openai_task_id_from_path(",
        "fn extract_gemini_short_id_from_path(",
        "fn extract_openai_task_id_from_cancel_path(",
        "fn extract_openai_task_id_from_remix_path(",
        "fn extract_openai_task_id_from_content_path(",
        "fn extract_gemini_short_id_from_cancel_path(",
        "fn resolve_video_task_read_lookup_key(",
        "fn resolve_video_task_hydration_lookup_key(",
        "fn current_unix_timestamp_secs(",
        "fn generate_local_short_id(",
    ] {
        assert!(
            !path.contains(pattern),
            "video_tasks/helpers/path.rs should not own {pattern}"
        );
    }

    let util = read_workspace_file("apps/aether-gateway/src/video_tasks/helpers/util.rs");
    assert!(
        util.contains("aether_video_tasks_core"),
        "video_tasks/helpers/util.rs should depend on aether-video-tasks-core"
    );
    assert!(
        !util.contains("fn non_empty_owned("),
        "video_tasks/helpers/util.rs should not own non_empty_owned"
    );

    let helpers = read_workspace_file("apps/aether-gateway/src/video_tasks/helpers.rs");
    assert!(
        !helpers.contains("mod transport;"),
        "video_tasks/helpers.rs should not keep a local transport bridge module"
    );
    assert!(
        !helpers.contains("transport_from_provider_transport"),
        "video_tasks/helpers.rs should not re-export a local transport bridge"
    );
}

#[test]
fn video_task_store_depends_on_shared_core_crate() {
    let store = read_workspace_file("apps/aether-gateway/src/video_tasks/store.rs");
    assert!(
        store.contains("aether_video_tasks_core"),
        "video_tasks/store.rs should depend on aether-video-tasks-core"
    );
    for pattern in [
        "trait VideoTaskStore",
        "struct InMemoryVideoTaskStore",
        "struct FileVideoTaskStore",
        "mod backend;",
        "mod registry;",
    ] {
        assert!(
            !store.contains(pattern),
            "video_tasks/store.rs should not own {pattern}"
        );
    }
}

#[test]
fn video_task_service_depends_on_shared_core_crate() {
    let service = read_workspace_file("apps/aether-gateway/src/video_tasks/service.rs");
    assert!(
        service.contains("aether_video_tasks_core::VideoTaskService"),
        "video_tasks/service.rs should wrap shared VideoTaskService"
    );
    for pattern in [
        "truth_source_mode:",
        "store:",
        "mod follow_up;",
        "mod lifecycle;",
        "mod read;",
        "mod refresh;",
    ] {
        assert!(
            !service.contains(pattern),
            "video_tasks/service.rs should not own {pattern}"
        );
    }
}

#[test]
fn video_task_state_is_split_between_data_and_runtime_crates() {
    let store = read_workspace_file("apps/aether-gateway/src/video_tasks/store.rs");
    assert!(
        store.contains("aether_video_tasks_core"),
        "video_tasks/store.rs should keep runtime store ownership in aether-video-tasks-core"
    );
    assert!(
        !store.contains("aether_data::repository::video_tasks"),
        "video_tasks/store.rs should not own persistent video task repository types"
    );

    let state_video = read_workspace_file("apps/aether-gateway/src/state/video.rs");
    assert!(
        state_video.contains("aether_data_contracts::repository::video_tasks::"),
        "state/video.rs should use aether-data-contracts video task repository types for persistence"
    );
    assert!(
        state_video.contains("reconstruct_local_video_task_snapshot"),
        "state/video.rs should reuse shared runtime snapshot reconstruction"
    );
    for pattern in [
        "InMemoryVideoTaskStore",
        "FileVideoTaskStore",
        "trait VideoTaskStore",
    ] {
        assert!(
            !state_video.contains(pattern),
            "state/video.rs should not own runtime store implementation {pattern}"
        );
    }
}

#[test]
fn data_backed_video_task_rebuild_uses_shared_provider_transport() {
    let state_video = read_workspace_file("apps/aether-gateway/src/state/video.rs");
    assert!(
        state_video.contains("reconstruct_local_video_task_snapshot"),
        "state/video.rs should rebuild snapshots through shared provider transport helper"
    );
    assert!(
        state_video.contains("resolve_video_task_hydration_lookup_key"),
        "state/video.rs should resolve hydrate lookup through shared video task helper"
    );
    assert!(
        !state_video.contains(
            "impl crate::provider_transport::VideoTaskTransportSnapshotLookup for AppState"
        ),
        "state/video.rs should not host video task transport lookup integration impl anymore"
    );
    assert!(
        !state_video.contains("resolve_local_video_task_transport"),
        "state/video.rs should not manually rebuild local video transport"
    );
    for pattern in [
        "extract_openai_task_id_from_path(",
        "extract_openai_task_id_from_cancel_path(",
        "extract_openai_task_id_from_remix_path(",
        "extract_openai_task_id_from_content_path(",
        "extract_gemini_short_id_from_path(",
        "extract_gemini_short_id_from_cancel_path(",
    ] {
        assert!(
            !state_video.contains(pattern),
            "state/video.rs should not inline path extractor {pattern}"
        );
    }
    assert!(
        !state_video.contains("self.data\n            .read_provider_transport_snapshot"),
        "state/video.rs should not inline provider transport snapshot reads in the rebuild path"
    );

    let video_mod = read_workspace_file("apps/aether-gateway/src/video_tasks/mod.rs");
    assert!(
        !video_mod.contains("transport_from_provider_transport"),
        "video_tasks/mod.rs should not export a local provider transport bridge"
    );

    let state_integrations = read_workspace_file("apps/aether-gateway/src/state/integrations.rs");
    assert!(
        state_integrations
            .contains("impl provider_transport::VideoTaskTransportSnapshotLookup for AppState"),
        "state/integrations.rs should host VideoTaskTransportSnapshotLookup for AppState"
    );

    let data_video = read_workspace_file("apps/aether-gateway/src/data/state/runtime.rs");
    assert!(
        data_video.contains("aether_video_tasks_core"),
        "data/state/runtime.rs should depend on aether-video-tasks-core"
    );
    assert!(
        data_video.contains("read_data_backed_video_task_response"),
        "data/state/runtime.rs should delegate data-backed read orchestration to shared video task helper"
    );
    for pattern in [
        "read_openai_video_task_response(",
        "read_gemini_video_task_response(",
        "resolve_video_task_read_lookup_key",
        "map_openai_stored_task_to_read_response",
        "map_gemini_stored_task_to_read_response",
    ] {
        assert!(
            !data_video.contains(pattern),
            "data/state/runtime.rs should not own data-backed video read orchestration {pattern}"
        );
    }

    let core_read_side = read_workspace_file("crates/aether-video-tasks-core/src/read_side.rs");
    for pattern in [
        "pub trait StoredVideoTaskReadSide",
        "pub async fn read_data_backed_video_task_response(",
        "resolve_video_task_read_lookup_key",
        "map_openai_stored_task_to_read_response",
        "map_gemini_stored_task_to_read_response",
    ] {
        assert!(
            core_read_side.contains(pattern),
            "aether-video-tasks-core read_side.rs should own {pattern}"
        );
    }

    let gateway_data_mod = read_workspace_file("apps/aether-gateway/src/data/mod.rs");
    for pattern in ["mod openai;", "mod gemini;", "mod video_tasks;"] {
        assert!(
            !gateway_data_mod.contains(pattern),
            "data/mod.rs should not keep local video task projection wrapper {pattern}"
        );
    }
}

#[test]
fn provider_transport_cache_helpers_live_in_shared_crate() {
    let state_cache = read_workspace_file("apps/aether-gateway/src/state/cache.rs");
    assert!(
        state_cache.contains("pub(crate) struct CachedProviderTransportSnapshot"),
        "state/cache.rs should keep the app-local cached snapshot wrapper"
    );
    for pattern in [
        "struct ProviderTransportSnapshotCacheKey",
        "fn provider_transport_snapshot_looks_refreshed(",
    ] {
        assert!(
            !state_cache.contains(pattern),
            "state/cache.rs should not own provider transport cache helper {pattern}"
        );
    }

    let state_mod = read_workspace_file("apps/aether-gateway/src/state/mod.rs");
    assert!(
        state_mod.contains("super::provider_transport::ProviderTransportSnapshotCacheKey"),
        "state/mod.rs should re-export ProviderTransportSnapshotCacheKey from shared provider transport"
    );
    assert!(
        state_mod
            .contains("super::provider_transport::provider_transport_snapshot_looks_refreshed"),
        "state/mod.rs should import refresh detection from shared provider transport"
    );

    let transport_cache = read_workspace_file("crates/aether-provider-transport/src/cache.rs");
    for pattern in [
        "pub struct ProviderTransportSnapshotCacheKey",
        "pub fn provider_transport_snapshot_looks_refreshed(",
    ] {
        assert!(
            transport_cache.contains(pattern),
            "aether-provider-transport cache helper should own {pattern}"
        );
    }
}

#[test]
fn gateway_provider_transport_transition_copies_are_removed() {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root should resolve");
    let transition_dir = workspace_root.join("apps/aether-gateway/src/provider_transport");
    if !transition_dir.exists() {
        return;
    }
    let mut rust_files = Vec::new();
    collect_rust_files(&transition_dir, &mut rust_files);
    assert!(
        rust_files.is_empty(),
        "apps/aether-gateway/src/provider_transport should not retain Rust transition copies after provider transport extraction"
    );
}

#[test]
fn gateway_billing_and_settlement_runtime_transition_copies_are_removed() {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root should resolve");

    for relative in [
        "apps/aether-gateway/src/billing_runtime",
        "apps/aether-gateway/src/settlement_runtime",
    ] {
        let transition_dir = workspace_root.join(relative);
        if !transition_dir.exists() {
            continue;
        }
        let mut rust_files = Vec::new();
        collect_rust_files(&transition_dir, &mut rust_files);
        assert!(
            rust_files.is_empty(),
            "{relative} should not retain Rust transition copies after usage extraction"
        );
    }
}

#[test]
fn usage_reporting_does_not_log_raw_report_context() {
    let source = read_workspace_file("apps/aether-gateway/src/usage/reporting/mod.rs");
    assert!(
        !source.contains("report_context = ?payload.report_context"),
        "usage/reporting/mod.rs should not log raw report_context"
    );
}

#[test]
fn proxy_registration_client_does_not_log_raw_management_response_body() {
    let source = read_workspace_file("apps/aether-proxy/src/registration/client.rs");
    assert!(
        !source.contains("error!(body = %text"),
        "registration/client.rs should not log raw management response bodies"
    );
    assert!(
        !source.contains("register failed (HTTP {}): {}"),
        "registration/client.rs should not bubble raw register response bodies into logs"
    );
    assert!(
        !source.contains("unregister failed: {}"),
        "registration/client.rs should not bubble raw unregister response bodies into logs"
    );
}

#[test]
fn hotspot_modules_do_not_log_sensitive_payload_like_fields() {
    let patterns = [
        "report_context = ?",
        "payload = ?",
        "headers = ?",
        "original_request_body = ?",
        "provider_request_body = ?",
        "request_body = ?",
        "response_body = ?",
    ];

    for root in [
        "src/ai_pipeline",
        "src/execution_runtime",
        "src/usage",
        "src/async_task",
    ] {
        assert_no_sensitive_log_patterns(root, &patterns);
    }
}

#[test]
fn execution_runtime_video_finalize_paths_depend_on_shared_video_task_core() {
    let response =
        read_workspace_file("apps/aether-gateway/src/execution_runtime/sync/execution/response.rs");
    for pattern in [
        "build_local_sync_finalize_read_response",
        "resolve_local_sync_error_background_report_kind",
        "resolve_local_sync_success_background_report_kind",
    ] {
        assert!(
            response.contains(pattern),
            "execution/runtime response path should depend on shared video helper {pattern}"
        );
    }
    for pattern in [
        "fn resolve_local_sync_success_background_report_kind(",
        "fn resolve_local_sync_error_background_report_kind(",
        "\"openai_video_delete_sync_success\"",
        "\"openai_video_cancel_sync_success\"",
        "\"gemini_video_cancel_sync_success\"",
        "\"openai_video_create_sync_error\"",
        "\"openai_video_remix_sync_error\"",
        "\"gemini_video_create_sync_error\"",
    ] {
        assert!(
            !response.contains(pattern),
            "execution/runtime response path should not own video finalize mapping {pattern}"
        );
    }

    let internal_gateway =
        read_workspace_file("apps/aether-gateway/src/handlers/internal/gateway_helpers.rs");
    assert!(
        internal_gateway.contains("build_local_sync_finalize_request_path"),
        "internal gateway finalize path should depend on shared video finalize request-path helper"
    );
    for pattern in [
        "build_internal_finalize_video_plan",
        "infer_internal_finalize_signature",
        "resolve_internal_finalize_route",
    ] {
        assert!(
            internal_gateway.contains(pattern),
            "internal gateway finalize path should depend on shared helper {pattern}"
        );
    }
    assert!(
        !internal_gateway.contains("fn build_internal_finalize_video_request_path("),
        "internal gateway finalize path should not own local finalize request-path builder"
    );
    assert!(
        !internal_gateway.contains("fn build_internal_finalize_video_plan("),
        "internal gateway finalize path should not own local finalize video plan builder"
    );
    assert!(
        !internal_gateway.contains("fn infer_internal_finalize_signature("),
        "internal gateway finalize path should not own local finalize signature inference"
    );
}
