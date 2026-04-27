use std::path::{Path, PathBuf};

use super::*;

#[test]
fn gateway_small_runtime_shims_stay_deleted() {
    for path in [
        "apps/aether-gateway/src/hooks/audit.rs",
        "apps/aether-gateway/src/hooks/shadow.rs",
        "apps/aether-gateway/src/auth/runtime.rs",
        "apps/aether-gateway/src/auth/trusted.rs",
        "apps/aether-gateway/src/usage/runtime.rs",
        "apps/aether-gateway/src/usage/config.rs",
        "apps/aether-gateway/src/usage/queue.rs",
        "apps/aether-gateway/src/usage/event.rs",
        "apps/aether-gateway/src/executor/diagnostics.rs",
        "apps/aether-gateway/src/executor/reports.rs",
        "apps/aether-gateway/src/executor/retries.rs",
        "apps/aether-gateway/src/query/billing/mod.rs",
        "apps/aether-gateway/src/query/monitoring/mod.rs",
        "apps/aether-gateway/src/state/runtime/security/mod.rs",
        "apps/aether-gateway/src/state/runtime/payments/mod.rs",
    ] {
        assert!(
            !workspace_file_exists(path),
            "{path} should stay removed after M6 shim cleanup"
        );
    }

    let hooks_mod = read_workspace_file("apps/aether-gateway/src/hooks/mod.rs");
    assert!(
        hooks_mod.contains("pub(crate) use crate::usage::http::{get_request_audit_bundle, get_request_usage_audit};"),
        "hooks/mod.rs should re-export request audit helpers directly from usage/http"
    );

    let auth_mod = read_workspace_file("apps/aether-gateway/src/auth/mod.rs");
    for pattern in [
        "resolve_execution_runtime_auth_context",
        "should_buffer_request_for_local_auth",
        "GatewayControlAuthContext",
        "request_model_local_rejection",
        "trusted_auth_local_rejection",
        "GatewayLocalAuthRejection",
    ] {
        assert!(
            auth_mod.contains(pattern),
            "auth/mod.rs should expose control-owned auth seam {pattern}"
        );
    }
    for forbidden in ["mod runtime;", "mod trusted;"] {
        assert!(
            !auth_mod.contains(forbidden),
            "auth/mod.rs should not keep local shim {forbidden}"
        );
    }

    let usage_mod = read_workspace_file("apps/aether-gateway/src/usage/mod.rs");
    assert!(
        usage_mod.contains("pub(crate) use aether_usage_runtime::UsageRuntime;"),
        "usage/mod.rs should expose UsageRuntime directly from aether_usage_runtime"
    );
    assert!(
        !usage_mod.contains("mod runtime;"),
        "usage/mod.rs should not keep a local runtime shim"
    );
    for forbidden in ["mod config;", "mod queue;", "mod event;"] {
        assert!(
            !usage_mod.contains(forbidden),
            "usage/mod.rs should not keep deleted shim module {forbidden}"
        );
    }

    let executor_mod = read_workspace_file("apps/aether-gateway/src/executor/mod.rs");
    for forbidden in ["mod diagnostics;", "mod reports;", "mod retries;"] {
        assert!(
            !executor_mod.contains(forbidden),
            "executor/mod.rs should not keep deleted shim module {forbidden}"
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
        "read_ranked_minimal_candidate_selection",
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
        !candidate_mod.contains("build_ranked_minimal_candidate_selection"),
        "candidate/mod.rs should not own the core ranked minimal candidate builder anymore"
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
        "pub(crate) async fn read_ranked_minimal_candidate_selection(",
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
        selection.contains("enumerate_scheduler_candidates("),
        "selection.rs should delegate candidate enumeration"
    );
    assert!(
        selection.contains("read_candidate_runtime_selection_snapshot("),
        "selection.rs should delegate runtime snapshot loading"
    );
    assert!(
        selection.contains("resolve_scheduler_candidate_selectability("),
        "selection.rs should delegate selectability resolution"
    );
    assert!(
        selection.contains("rank_scheduler_candidates("),
        "selection.rs should delegate final ranking"
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
        "collect_selectable_candidates_from_keys",
        "auth_api_key_concurrency_limit_reached",
        "build_provider_concurrent_limit_map(",
        "reorder_candidates_by_scheduler_health",
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

    let core_candidate_enumeration =
        read_workspace_file("crates/aether-scheduler-core/src/candidate/enumeration.rs");
    for forbidden in [
        "apply_scheduler_candidate_ranking",
        "SchedulerRankableCandidate",
        "candidate_affinity_hash",
        "requested_capability_priority_for_candidate_descriptors",
    ] {
        assert!(
            !core_candidate_enumeration.contains(forbidden),
            "core candidate/enumeration.rs should only enumerate theoretical candidates, not rank with {forbidden}"
        );
    }

    let core_candidate_selectability =
        read_workspace_file("crates/aether-scheduler-core/src/candidate/selectability.rs");
    for forbidden in [
        "apply_scheduler_candidate_ranking",
        "with_affinity_hash",
        "collect_selectable_candidates_from_keys",
        "reorder_candidates_by_scheduler_health",
    ] {
        assert!(
            !core_candidate_selectability.contains(forbidden),
            "core candidate/selectability.rs should only decide selectability, not rank with {forbidden}"
        );
    }

    assert!(
        !workspace_file_exists("crates/aether-scheduler-core/src/candidate/selection.rs"),
        "core candidate/selection.rs compatibility helper should be removed"
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
        "async fn read_ranked_minimal_candidate_selection(",
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
        candidate_selection.contains(
            "pub(crate) async fn enumerate_minimal_candidate_selection_with_required_capabilities(",
        ),
        "data/candidate_selection.rs should host minimal candidate enumeration builder"
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
        !candidate_selection.contains("read_ranked_minimal_candidate_selection"),
        "data/candidate_selection.rs should not host ranked candidate selection compatibility readers"
    );
    assert!(
        !candidate_selection.contains("build_ranked_minimal_candidate_selection"),
        "data/candidate_selection.rs should not depend on core ranked minimal candidate builder"
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

    let planner_candidate_ranking =
        read_workspace_file("apps/aether-gateway/src/ai_pipeline/planner/candidate_ranking.rs");
    assert!(
        planner_candidate_ranking.contains("use aether_scheduler_core::{")
            && planner_candidate_ranking.contains("SchedulerMinimalCandidateSelectionCandidate"),
        "planner/candidate_ranking.rs should depend directly on core minimal candidate DTO"
    );
    assert!(
        !planner_candidate_ranking
            .contains("crate::scheduler::SchedulerMinimalCandidateSelectionCandidate"),
        "planner/candidate_ranking.rs should not depend on scheduler candidate DTO re-export"
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
        !state_integrations.contains("async fn read_ranked_minimal_candidate_selection("),
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
        !state_mod.contains("read_ranked_minimal_candidate_selection"),
        "data/state/mod.rs should not import scheduler candidate selection entrypoints"
    );
    assert!(
        !state_runtime.contains("pub(crate) async fn read_ranked_minimal_candidate_selection("),
        "data/state/runtime.rs should not own scheduler minimal candidate derived read"
    );
    assert!(
        !auth_api_keys.contains("read_ranked_minimal_candidate_selection("),
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
        "resolve_local_openai_bearer_auth(",
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

#[test]
fn ai_pipeline_runtime_kiro_wrapper_is_facade_only() {
    for path in [
        "apps/aether-gateway/src/ai_pipeline/runtime/mod.rs",
        "apps/aether-gateway/src/ai_pipeline/runtime/provider_types.rs",
        "apps/aether-gateway/src/ai_pipeline/runtime/adapters/mod.rs",
        "apps/aether-gateway/src/ai_pipeline/runtime/adapters/antigravity/mod.rs",
        "apps/aether-gateway/src/ai_pipeline/runtime/adapters/claude/mod.rs",
        "apps/aether-gateway/src/ai_pipeline/runtime/adapters/claude_code/mod.rs",
        "apps/aether-gateway/src/ai_pipeline/runtime/adapters/gemini/mod.rs",
        "apps/aether-gateway/src/ai_pipeline/runtime/adapters/generic_oauth.rs",
        "apps/aether-gateway/src/ai_pipeline/runtime/adapters/kiro/mod.rs",
        "apps/aether-gateway/src/ai_pipeline/runtime/adapters/openai/mod.rs",
        "apps/aether-gateway/src/ai_pipeline/runtime/adapters/vertex/mod.rs",
        "apps/aether-gateway/src/ai_pipeline/runtime/adapters/kiro/auth.rs",
        "apps/aether-gateway/src/ai_pipeline/runtime/adapters/kiro/converter.rs",
        "apps/aether-gateway/src/ai_pipeline/runtime/adapters/kiro/credentials.rs",
        "apps/aether-gateway/src/ai_pipeline/runtime/adapters/kiro/headers.rs",
        "apps/aether-gateway/src/ai_pipeline/runtime/adapters/kiro/policy.rs",
        "apps/aether-gateway/src/ai_pipeline/runtime/adapters/kiro/refresh.rs",
        "apps/aether-gateway/src/ai_pipeline/runtime/adapters/kiro/request.rs",
        "apps/aether-gateway/src/ai_pipeline/runtime/adapters/kiro/url.rs",
    ] {
        assert!(
            !workspace_file_exists(path),
            "{path} should be removed once gateway ai_pipeline runtime adapter ownership is flattened into adaptation/provider_transport facades"
        );
    }

    let adaptation_mod =
        read_workspace_file("apps/aether-gateway/src/ai_pipeline/adaptation/mod.rs");
    assert!(
        adaptation_mod.contains("pub(crate) use kiro::KiroToClaudeCliStreamState;"),
        "adaptation/mod.rs should own KiroToClaudeCliStreamState export after runtime facade removal"
    );
}
