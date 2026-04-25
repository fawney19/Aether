use super::*;

#[test]
fn ai_pipeline_crate_api_is_confined_to_root_seams() {
    let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root should resolve");
    let mut violations = Vec::new();

    for file in collect_workspace_rust_files("apps/aether-gateway/src") {
        let relative = file
            .canonicalize()
            .expect("workspace file should canonicalize")
            .strip_prefix(&workspace_root)
            .expect("workspace file should be under workspace root")
            .to_string_lossy()
            .replace('\\', "/");
        if relative == "apps/aether-gateway/src/ai_pipeline/pure/mod.rs"
            || relative == "apps/aether-gateway/src/ai_pipeline_api.rs"
            || relative.starts_with("apps/aether-gateway/src/tests/")
        {
            continue;
        }

        let source = std::fs::read_to_string(&file).expect("source file should be readable");
        if source.contains("aether_ai_pipeline::api") {
            violations.push(relative);
        }
    }

    assert!(
        violations.is_empty(),
        "gateway code should only depend on aether_ai_pipeline::api through ai_pipeline/pure/mod.rs or ai_pipeline_api.rs:\n{}",
        violations.join("\n")
    );

    let mut crate_violations = Vec::new();
    for file in collect_workspace_rust_files("apps/aether-gateway/src") {
        let relative = file
            .canonicalize()
            .expect("workspace file should canonicalize")
            .strip_prefix(&workspace_root)
            .expect("workspace file should be under workspace root")
            .to_string_lossy()
            .replace('\\', "/");
        if relative == "apps/aether-gateway/src/ai_pipeline/pure/mod.rs"
            || relative == "apps/aether-gateway/src/ai_pipeline/transport.rs"
            || relative == "apps/aether-gateway/src/ai_pipeline_api.rs"
            || relative.ends_with("/tests.rs")
            || relative.contains("/tests/")
            || relative.starts_with("apps/aether-gateway/src/tests/")
        {
            continue;
        }

        let source = std::fs::read_to_string(&file).expect("source file should be readable");
        if source.contains("aether_ai_pipeline::") {
            crate_violations.push(relative);
        }
    }

    assert!(
        crate_violations.is_empty(),
        "gateway code should only depend directly on aether_ai_pipeline through ai_pipeline root seams:\n{}",
        crate_violations.join("\n")
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
    assert_no_module_dependency_patterns(
        "src/ai_pipeline",
        &[
            "crate::ai_pipeline::control_facade::",
            "use crate::ai_pipeline::control_facade::",
            "crate::ai_pipeline::execution_facade::",
            "use crate::ai_pipeline::execution_facade::",
            "crate::ai_pipeline::provider_transport_facade::",
            "use crate::ai_pipeline::provider_transport_facade::",
            "crate::ai_pipeline::planner::auth_snapshot_facade::",
            "use crate::ai_pipeline::planner::auth_snapshot_facade::",
            "crate::ai_pipeline::planner::scheduler_facade::",
            "use crate::ai_pipeline::planner::scheduler_facade::",
            "crate::ai_pipeline::planner::candidate_runtime_facade::",
            "use crate::ai_pipeline::planner::candidate_runtime_facade::",
            "crate::ai_pipeline::planner::transport_facade::",
            "use crate::ai_pipeline::planner::transport_facade::",
        ],
    );

    let control_payloads =
        read_workspace_file("apps/aether-gateway/src/ai_pipeline/contracts/control_payloads.rs");
    for pattern in patterns {
        assert!(
            !control_payloads.contains(pattern),
            "contracts/control_payloads.rs should route control/runtime dependencies through ai_pipeline facades, found {pattern}"
        );
    }
    assert!(
        !control_payloads.contains("GatewayControlAuthContext"),
        "contracts/control_payloads.rs should not own GatewayControlAuthContext after execution auth DTO extraction"
    );
    for pattern in [
        "struct GatewayControlPlanRequest",
        "struct GatewayControlPlanResponse",
        "struct GatewayControlSyncDecisionResponse",
    ] {
        assert!(
            !control_payloads.contains(pattern),
            "contracts/control_payloads.rs should not own {pattern} after DTO extraction"
        );
    }
    assert!(
        control_payloads.contains("crate::ai_pipeline::"),
        "contracts/control_payloads.rs should consume pipeline-crate control DTOs through the ai_pipeline root seam after DTO extraction"
    );
    assert!(
        control_payloads.contains(
            "generic_decision_missing_exact_provider_request as generic_decision_missing_exact_provider_request_impl"
        ),
        "contracts/control_payloads.rs should delegate exact-request detection to the pipeline crate"
    );
    assert!(
        !control_payloads.contains("GatewayControlPlanRequest {"),
        "contracts/control_payloads.rs should not locally construct GatewayControlPlanRequest after helper extraction"
    );
    assert!(
        !control_payloads.contains("pub(crate) async fn build_gateway_plan_request("),
        "contracts/control_payloads.rs should not keep dead plan-request bridge after helper extraction"
    );

    let gateway_plan_builders =
        read_workspace_file("apps/aether-gateway/src/ai_pipeline/planner/plan_builders.rs");
    for pattern in [
        "struct LocalSyncPlanAndReport",
        "struct LocalStreamPlanAndReport",
    ] {
        assert!(
            !gateway_plan_builders.contains(pattern),
            "planner/plan_builders.rs should not own {pattern} after plan DTO extraction"
        );
    }
    assert!(
        gateway_plan_builders.contains("crate::ai_pipeline::"),
        "planner/plan_builders.rs should consume pipeline-crate plan DTOs through the ai_pipeline root seam after extraction"
    );
    assert!(
        gateway_plan_builders.contains(
            "use crate::ai_pipeline::augment_sync_report_context as augment_sync_report_context_impl;"
        ),
        "planner/plan_builders.rs should delegate report-context augmentation through the ai_pipeline root seam"
    );

    let gateway_finalize_common =
        read_workspace_file("apps/aether-gateway/src/ai_pipeline/finalize/common.rs");
    assert!(
        gateway_finalize_common
            .contains("prepare_local_success_response_parts as prepare_local_success_response_parts_impl"),
        "finalize/common.rs should delegate success response-part normalization to the pipeline crate"
    );
    assert!(
        gateway_finalize_common
            .contains("build_local_success_background_report as build_local_success_background_report_impl"),
        "finalize/common.rs should delegate success background-report construction to the pipeline crate"
    );
    assert!(
        gateway_finalize_common
            .contains("build_local_success_conversion_background_report as build_local_success_conversion_background_report_impl"),
        "finalize/common.rs should delegate conversion success background-report construction to the pipeline crate"
    );

    let ai_pipeline_mod = read_workspace_file("apps/aether-gateway/src/ai_pipeline/mod.rs");
    for pattern in [
        "crate::control::resolve_execution_runtime_auth_context",
        "crate::headers::collect_control_headers",
        "crate::headers::is_json_request",
    ] {
        assert!(
            ai_pipeline_mod.contains(pattern),
            "ai_pipeline/mod.rs should own {pattern}"
        );
    }
    assert!(
        ai_pipeline_mod.contains("ExecutionRuntimeAuthContext"),
        "ai_pipeline/mod.rs should own ExecutionRuntimeAuthContext projection"
    );

    assert!(
        ai_pipeline_mod
            .contains("crate::execution_runtime::maybe_build_local_sync_finalize_response"),
        "ai_pipeline/mod.rs should own local sync finalize response bridging"
    );
    assert!(
        !workspace_file_exists("apps/aether-gateway/src/ai_pipeline/control.rs"),
        "ai_pipeline/control.rs should stay removed after root seam consolidation"
    );
    assert!(
        !workspace_file_exists("apps/aether-gateway/src/ai_pipeline/execution.rs"),
        "ai_pipeline/execution.rs should stay removed after root seam consolidation"
    );

    assert!(
        !ai_pipeline_mod.contains("pub(crate) use aether_ai_pipeline::api::*;"),
        "ai_pipeline/mod.rs should not keep wildcard pipeline-crate exports after root-seam freeze"
    );
    for export in [
        "PlannerAppState",
        "GatewayAuthApiKeySnapshot",
        "GatewayProviderTransportSnapshot",
        "LocalResolvedOAuthRequestAuth",
    ] {
        assert!(
            ai_pipeline_mod.contains(export),
            "ai_pipeline/mod.rs should re-export {export} from the planner root seam"
        );
    }

    assert!(
        !workspace_file_exists("apps/aether-gateway/src/ai_pipeline/pure.rs"),
        "ai_pipeline/pure.rs should stay removed after pure seam directoryization"
    );
    assert!(
        workspace_file_exists("apps/aether-gateway/src/ai_pipeline/pure/mod.rs"),
        "ai_pipeline/pure/mod.rs should exist after pure seam directoryization"
    );
    for path in [
        "apps/aether-gateway/src/ai_pipeline/pure/adaptation.rs",
        "apps/aether-gateway/src/ai_pipeline/pure/contracts.rs",
        "apps/aether-gateway/src/ai_pipeline/pure/conversion.rs",
        "apps/aether-gateway/src/ai_pipeline/pure/finalize.rs",
        "apps/aether-gateway/src/ai_pipeline/pure/planner.rs",
    ] {
        assert!(
            !workspace_file_exists(path),
            "{path} should stay removed after pure seam collapse"
        );
    }

    let pure_mod = read_workspace_file("apps/aether-gateway/src/ai_pipeline/pure/mod.rs");
    for pattern in [
        "pub(crate) use aether_ai_pipeline::api::{",
        "ExecutionRuntimeAuthContext",
        "ProviderAdaptationDescriptor",
        "RequestConversionKind",
        "PipelineFinalizeError",
        "LocalStandardSpec",
    ] {
        assert!(
            pure_mod.contains(pattern),
            "ai_pipeline/pure/mod.rs should own {pattern}"
        );
    }
}

#[test]
fn ai_pipeline_routes_provider_transport_deps_through_facade() {
    let patterns = [
        "use crate::provider_transport::",
        "crate::provider_transport::",
    ];

    for root in ["src/ai_pipeline/planner", "src/ai_pipeline/conversion"] {
        assert_no_module_dependency_patterns(root, &patterns);
    }
    assert!(
        !workspace_file_exists("apps/aether-gateway/src/ai_pipeline/runtime"),
        "ai_pipeline/runtime should stay removed after facade cleanup"
    );
    assert!(
        workspace_file_exists("crates/aether-ai-pipeline/src/transport.rs"),
        "aether-ai-pipeline should own the transport root after provider-transport bridge extraction"
    );

    let provider_transport_facade =
        read_workspace_file("apps/aether-gateway/src/ai_pipeline/transport.rs");
    for pattern in [
        "aether_ai_pipeline::transport::auth",
        "aether_ai_pipeline::transport::url",
        "aether_ai_pipeline::transport::policy",
        "aether_ai_pipeline::transport::snapshot",
    ] {
        assert!(
            provider_transport_facade.contains(pattern),
            "transport.rs should own {pattern}"
        );
    }
    for forbidden in [
        "crate::provider_transport::auth",
        "crate::provider_transport::url",
        "crate::provider_transport::policy",
        "crate::provider_transport::snapshot",
    ] {
        assert!(
            !provider_transport_facade.contains(forbidden),
            "transport.rs should not keep gateway-local provider_transport owner {forbidden}"
        );
    }

    let ai_pipeline_mod = read_workspace_file("apps/aether-gateway/src/ai_pipeline/mod.rs");
    assert!(
        ai_pipeline_mod.contains("pub(crate) mod transport;"),
        "ai_pipeline/mod.rs should expose provider transport capabilities through the root seam module"
    );
}

#[test]
fn ai_pipeline_planner_gateway_state_seam_is_split_by_role() {
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
            !workspace_file_exists(path),
            "{path} should be removed after PlannerAppState absorbed the seam"
        );
    }

    for path in [
        "apps/aether-gateway/src/ai_pipeline/planner/state/mod.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/state/auth.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/state/transport.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/state/scheduler.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/state/candidate_runtime.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/state/executor.rs",
    ] {
        assert!(
            workspace_file_exists(path),
            "{path} should exist after PlannerAppState directoryization"
        );
    }

    let state_mod = read_workspace_file("apps/aether-gateway/src/ai_pipeline/planner/state/mod.rs");
    for pattern in [
        "mod auth;",
        "mod transport;",
        "mod scheduler;",
        "mod candidate_runtime;",
        "mod executor;",
        "struct PlannerAppState",
    ] {
        assert!(
            state_mod.contains(pattern),
            "planner/state/mod.rs should own {pattern}"
        );
    }

    let state_auth =
        read_workspace_file("apps/aether-gateway/src/ai_pipeline/planner/state/auth.rs");
    assert!(
        state_auth.contains("read_auth_api_key_snapshot("),
        "planner/state/auth.rs should own auth snapshot reads"
    );

    let state_transport =
        read_workspace_file("apps/aether-gateway/src/ai_pipeline/planner/state/transport.rs");
    for pattern in [
        "read_provider_transport_snapshot(",
        "resolve_local_oauth_request_auth(",
    ] {
        assert!(
            state_transport.contains(pattern),
            "planner/state/transport.rs should own {pattern}"
        );
    }

    let state_scheduler =
        read_workspace_file("apps/aether-gateway/src/ai_pipeline/planner/state/scheduler.rs");
    for pattern in [
        "list_selectable_candidates(",
        "list_selectable_candidates_for_required_capability_without_requested_model(",
    ] {
        assert!(
            state_scheduler.contains(pattern),
            "planner/state/scheduler.rs should own {pattern}"
        );
    }

    let state_candidate_runtime = read_workspace_file(
        "apps/aether-gateway/src/ai_pipeline/planner/state/candidate_runtime.rs",
    );
    for pattern in [
        "persist_available_local_candidate(",
        "persist_skipped_local_candidate(",
    ] {
        assert!(
            state_candidate_runtime.contains(pattern),
            "planner/state/candidate_runtime.rs should own {pattern}"
        );
    }

    let state_executor =
        read_workspace_file("apps/aether-gateway/src/ai_pipeline/planner/state/executor.rs");
    assert!(
        state_executor.contains("mark_unused_local_candidate_items("),
        "planner/state/executor.rs should own mark_unused_local_candidate_items"
    );
}

#[test]
fn ai_pipeline_planner_separates_local_candidate_eligibility_from_affinity_ranking() {
    let planner_mod = read_workspace_file("apps/aether-gateway/src/ai_pipeline/planner/mod.rs");
    for pattern in [
        "mod candidate_affinity;",
        "mod candidate_eligibility;",
        "mod candidate_preparation;",
    ] {
        assert!(
            planner_mod.contains(pattern),
            "planner/mod.rs should wire {pattern}"
        );
    }

    let candidate_eligibility =
        read_workspace_file("apps/aether-gateway/src/ai_pipeline/planner/candidate_eligibility.rs");
    for pattern in [
        "pub(crate) async fn filter_and_rank_local_execution_candidates(",
        "pub(crate) async fn filter_and_rank_local_execution_candidates_without_transport_pair_gate(",
        "pub(crate) async fn read_candidate_transport_snapshot(",
    ] {
        assert!(
            candidate_eligibility.contains(pattern),
            "planner/candidate_eligibility.rs should own {pattern}"
        );
    }

    let candidate_affinity =
        read_workspace_file("apps/aether-gateway/src/ai_pipeline/planner/candidate_affinity.rs");
    assert!(
        candidate_affinity.contains("#[cfg(test)]\nasync fn rank_local_execution_candidates("),
        "planner/candidate_affinity.rs should keep raw local ranking as a test-only helper"
    );
    for forbidden in [
        "struct SkippedLocalExecutionCandidate",
        "async fn current_local_execution_candidate_skip_reason(",
        "pub(crate) async fn filter_and_rank_local_execution_candidates(",
    ] {
        assert!(
            !candidate_affinity.contains(forbidden),
            "planner/candidate_affinity.rs should not own local candidate eligibility helper {forbidden}"
        );
    }
}

#[test]
fn ai_pipeline_candidate_preparation_owns_shared_auth_and_mapped_model_resolution() {
    let candidate_preparation =
        read_workspace_file("apps/aether-gateway/src/ai_pipeline/planner/candidate_preparation.rs");
    for pattern in [
        "pub(crate) async fn prepare_header_authenticated_candidate(",
        "pub(crate) async fn resolve_candidate_oauth_auth(",
        "pub(crate) fn resolve_candidate_mapped_model(",
    ] {
        assert!(
            candidate_preparation.contains(pattern),
            "planner/candidate_preparation.rs should own {pattern}"
        );
    }

    for path in [
        "apps/aether-gateway/src/ai_pipeline/planner/standard/openai/chat/decision/request.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/standard/openai/cli/decision/request.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/specialized/image/request.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/standard/family/request.rs",
    ] {
        let source = read_workspace_file(path);
        assert!(
            source.contains("prepare_header_authenticated_candidate("),
            "{path} should use shared header-auth candidate preparation"
        );
        assert!(
            !source.contains("resolve_local_oauth_request_auth("),
            "{path} should not inline oauth header-auth fallback after preparation extraction"
        );
    }

    for path in [
        "apps/aether-gateway/src/ai_pipeline/planner/specialized/video/request.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/passthrough/provider/family/request/prepare.rs",
    ] {
        let source = read_workspace_file(path);
        assert!(
            source.contains("resolve_candidate_mapped_model("),
            "{path} should use shared mapped-model preparation"
        );
        assert!(
            !source.contains("selected_provider_model_name.trim().to_string()"),
            "{path} should not inline mapped-model extraction after preparation extraction"
        );
    }

    let same_format_provider_prepare = read_workspace_file(
        "apps/aether-gateway/src/ai_pipeline/planner/passthrough/provider/family/request/prepare.rs",
    );
    assert!(
        same_format_provider_prepare.contains("resolve_candidate_oauth_auth("),
        "same-format provider preparation should use shared oauth candidate preparation"
    );
    assert!(
        !same_format_provider_prepare.contains("resolve_local_oauth_request_auth("),
        "same-format provider preparation should not inline oauth resolution after preparation extraction"
    );
}

#[test]
fn ai_pipeline_candidate_materialization_owns_affinity_and_candidate_runtime_persistence() {
    let planner_mod = read_workspace_file("apps/aether-gateway/src/ai_pipeline/planner/mod.rs");
    assert!(
        planner_mod.contains("mod candidate_materialization;"),
        "planner/mod.rs should wire candidate_materialization helper module"
    );

    let candidate_materialization = read_workspace_file(
        "apps/aether-gateway/src/ai_pipeline/planner/candidate_materialization.rs",
    );
    for pattern in [
        "pub(crate) struct LocalExecutionCandidateAttempt {",
        "pub(crate) struct LocalAvailableCandidatePersistenceContext<'a> {",
        "pub(crate) struct LocalSkippedCandidatePersistenceContext<'a> {",
        "pub(crate) fn remember_first_local_candidate_affinity(",
        "persist_available_local_execution_candidates",
        "persist_available_local_execution_candidates_with_context",
        "pub(crate) async fn persist_skipped_local_execution_candidate(",
        "pub(crate) async fn mark_skipped_local_execution_candidate(",
        "pub(crate) async fn persist_skipped_local_execution_candidates(",
        "pub(crate) async fn persist_skipped_local_execution_candidates_with_context(",
    ] {
        assert!(
            candidate_materialization.contains(pattern),
            "planner/candidate_materialization.rs should own {pattern}"
        );
    }

    for path in [
        "apps/aether-gateway/src/ai_pipeline/planner/standard/family/candidates.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/passthrough/provider/family/candidates.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/standard/openai/chat/decision/support.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/standard/openai/cli/decision/support.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/specialized/video/support.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/specialized/files/support.rs",
    ] {
        let source = read_workspace_file(path);
        for pattern in [
            "remember_first_local_candidate_affinity(",
            "persist_available_local_execution_candidates_with_context(",
            "persist_skipped_local_execution_candidates_with_context(",
        ] {
            assert!(
                source.contains(pattern),
                "{path} should use shared candidate materialization helper {pattern}"
            );
        }
        for forbidden in [
            "remember_scheduler_affinity_for_candidate(",
            "persist_available_local_candidate(",
            "persist_skipped_local_candidate(",
        ] {
            assert!(
                !source.contains(forbidden),
                "{path} should not inline candidate materialization step {forbidden}"
            );
        }
    }

    for path in [
        "apps/aether-gateway/src/ai_pipeline/planner/standard/family/payload.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/passthrough/provider/family/payload.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/standard/openai/chat/decision/support.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/standard/openai/cli/decision/support.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/specialized/video/support.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/specialized/files/support.rs",
    ] {
        let source = read_workspace_file(path);
        assert!(
            source.contains("mark_skipped_local_execution_candidate("),
            "{path} should route skipped candidate persistence through shared materialization helper"
        );
    }

    for (path, pattern) in [
        (
            "apps/aether-gateway/src/ai_pipeline/planner/standard/family/mod.rs",
            "LocalExecutionCandidateAttempt as LocalStandardCandidateAttempt",
        ),
        (
            "apps/aether-gateway/src/ai_pipeline/planner/passthrough/provider/family/mod.rs",
            "LocalExecutionCandidateAttempt as LocalSameFormatProviderCandidateAttempt",
        ),
        (
            "apps/aether-gateway/src/ai_pipeline/planner/standard/openai/chat/decision/support.rs",
            "LocalExecutionCandidateAttempt as LocalOpenAiChatCandidateAttempt",
        ),
        (
            "apps/aether-gateway/src/ai_pipeline/planner/standard/openai/cli/decision/support.rs",
            "LocalExecutionCandidateAttempt as LocalOpenAiCliCandidateAttempt",
        ),
        (
            "apps/aether-gateway/src/ai_pipeline/planner/specialized/video/support.rs",
            "LocalExecutionCandidateAttempt as LocalVideoCreateCandidateAttempt",
        ),
        (
            "apps/aether-gateway/src/ai_pipeline/planner/specialized/files/support.rs",
            "LocalExecutionCandidateAttempt as LocalGeminiFilesCandidateAttempt",
        ),
    ] {
        let source = read_workspace_file(path);
        assert!(
            source.contains(pattern),
            "{path} should rename shared LocalExecutionCandidateAttempt instead of redefining attempt structs"
        );
    }
}

#[test]
fn ai_pipeline_materialization_policy_owns_local_candidate_persistence_modes() {
    let planner_mod = read_workspace_file("apps/aether-gateway/src/ai_pipeline/planner/mod.rs");
    assert!(
        planner_mod.contains("mod materialization_policy;"),
        "planner/mod.rs should wire materialization_policy helper module"
    );

    let materialization_policy = read_workspace_file(
        "apps/aether-gateway/src/ai_pipeline/planner/materialization_policy.rs",
    );
    for pattern in [
        "pub(crate) enum LocalCandidatePersistencePolicyKind {",
        "pub(crate) struct LocalCandidatePersistencePolicy<'a> {",
        "pub(crate) fn build_local_candidate_persistence_policy<'a>(",
        "LocalCandidatePersistencePolicyKind::StandardDecision",
        "LocalCandidatePersistencePolicyKind::SameFormatProviderDecision",
        "LocalCandidatePersistencePolicyKind::OpenAiChatDecision",
        "LocalCandidatePersistencePolicyKind::OpenAiCliDecision",
        "LocalCandidatePersistencePolicyKind::GeminiFilesDecision",
        "LocalCandidatePersistencePolicyKind::VideoDecision",
    ] {
        assert!(
            materialization_policy.contains(pattern),
            "planner/materialization_policy.rs should own {pattern}"
        );
    }

    for path in [
        "apps/aether-gateway/src/ai_pipeline/planner/standard/family/candidates.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/passthrough/provider/family/candidates.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/standard/openai/chat/decision/support.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/standard/openai/cli/decision/support.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/specialized/video/support.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/specialized/files/support.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/standard/family/payload.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/passthrough/provider/family/payload.rs",
    ] {
        let source = read_workspace_file(path);
        assert!(
            source.contains("build_local_candidate_persistence_policy("),
            "{path} should route candidate persistence policy through planner/materialization_policy.rs"
        );
        assert!(
            source.contains("LocalCandidatePersistencePolicyKind::"),
            "{path} should select a shared materialization policy kind"
        );
        for forbidden in [
            "fn available_candidate_persistence_context(",
            "fn skipped_candidate_persistence_context(",
            "LocalAvailableCandidatePersistenceContext {",
            "LocalSkippedCandidatePersistenceContext {",
        ] {
            assert!(
                !source.contains(forbidden),
                "{path} should not inline persistence policy helper {forbidden}"
            );
        }
    }
}

#[test]
fn ai_pipeline_candidate_metadata_owns_local_execution_candidate_extra_data_shape() {
    let planner_mod = read_workspace_file("apps/aether-gateway/src/ai_pipeline/planner/mod.rs");
    assert!(
        planner_mod.contains("mod candidate_metadata;"),
        "planner/mod.rs should wire candidate_metadata helper module"
    );

    let candidate_metadata =
        read_workspace_file("apps/aether-gateway/src/ai_pipeline/planner/candidate_metadata.rs");
    for pattern in [
        "pub(crate) struct LocalExecutionCandidateMetadataParts<'a> {",
        "pub(crate) fn build_local_execution_candidate_metadata(",
        "pub(crate) fn build_local_execution_candidate_contract_metadata(",
    ] {
        assert!(
            candidate_metadata.contains(pattern),
            "planner/candidate_metadata.rs should own {pattern}"
        );
    }

    for path in [
        "apps/aether-gateway/src/ai_pipeline/planner/standard/family/candidates.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/standard/openai/chat/decision/support.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/standard/openai/cli/decision/support.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/passthrough/provider/family/candidates.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/specialized/files/support.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/specialized/video/support.rs",
    ] {
        let source = read_workspace_file(path);
        assert!(
            source.contains("build_local_execution_candidate_"),
            "{path} should route candidate persistence metadata through candidate_metadata.rs"
        );
        for forbidden in [
            "\"global_model_id\": eligible.candidate.global_model_id.clone()",
            "\"provider_name\": eligible.candidate.provider_name.clone()",
        ] {
            assert!(
                !source.contains(forbidden),
                "{path} should not inline shared candidate metadata field {forbidden}"
            );
        }
    }
}

#[test]
fn ai_pipeline_runtime_miss_owns_local_execution_miss_state_machine() {
    let planner_mod = read_workspace_file("apps/aether-gateway/src/ai_pipeline/planner/mod.rs");
    assert!(
        planner_mod.contains("mod runtime_miss;"),
        "planner/mod.rs should wire runtime_miss helper module"
    );

    let runtime_miss =
        read_workspace_file("apps/aether-gateway/src/ai_pipeline/planner/runtime_miss.rs");
    for pattern in [
        "pub(crate) fn set_local_runtime_miss_diagnostic_reason(",
        "pub(crate) fn build_local_runtime_execution_exhausted_diagnostic(",
        "pub(crate) fn set_local_runtime_execution_exhausted_diagnostic(",
        "pub(crate) fn build_local_runtime_candidate_evaluation_diagnostic(",
        "pub(crate) fn set_local_runtime_candidate_evaluation_diagnostic(",
        "pub(crate) fn apply_local_runtime_candidate_evaluation_progress(",
        "pub(crate) fn apply_local_runtime_candidate_evaluation_progress_preserving_candidate_signal(",
        "pub(crate) fn apply_local_runtime_candidate_terminal_reason(",
        "pub(crate) fn record_local_runtime_candidate_skip_reason(",
    ] {
        assert!(
            runtime_miss.contains(pattern),
            "planner/runtime_miss.rs should own {pattern}"
        );
    }

    for path in [
        "apps/aether-gateway/src/ai_pipeline/planner/standard/family/build.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/passthrough/provider/plans.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/passthrough/provider/family/build.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/candidate_materialization.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/standard/openai/chat/mod.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/standard/openai/chat/plans/diagnostic.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/standard/openai/chat/plans/sync.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/standard/openai/chat/plans/stream.rs",
    ] {
        let source = read_workspace_file(path);
        assert!(
            source.contains("runtime_miss")
                || source.contains("set_local_runtime_")
                || source.contains("apply_local_runtime_"),
            "{path} should route runtime miss state handling through planner/runtime_miss.rs"
        );
    }

    for path in [
        "apps/aether-gateway/src/ai_pipeline/planner/standard/family/build.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/passthrough/provider/plans.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/passthrough/provider/family/build.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/candidate_materialization.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/standard/openai/chat/mod.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/standard/openai/chat/plans/diagnostic.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/standard/openai/chat/plans/sync.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/standard/openai/chat/plans/stream.rs",
    ] {
        let source = read_workspace_file(path);
        for forbidden in [
            "state.set_local_execution_runtime_miss_diagnostic(",
            "state.mutate_local_execution_runtime_miss_diagnostic(",
        ] {
            assert!(
                !source.contains(forbidden),
                "{path} should not inline runtime miss state mutation {forbidden}"
            );
        }
    }
}

#[test]
fn ai_pipeline_standard_family_routes_request_preparation_through_request_payload_seams() {
    let standard_family_mod =
        read_workspace_file("apps/aether-gateway/src/ai_pipeline/planner/standard/family/mod.rs");
    assert!(
        standard_family_mod.contains("mod request;"),
        "standard family mod.rs should wire request seam"
    );

    let standard_family_request = read_workspace_file(
        "apps/aether-gateway/src/ai_pipeline/planner/standard/family/request.rs",
    );
    for pattern in [
        "pub(crate) struct LocalStandardCandidatePayloadParts {",
        "pub(crate) async fn resolve_local_standard_candidate_payload_parts(",
    ] {
        assert!(
            standard_family_request.contains(pattern),
            "standard family request.rs should own {pattern}"
        );
    }

    let standard_family_payload = read_workspace_file(
        "apps/aether-gateway/src/ai_pipeline/planner/standard/family/payload.rs",
    );
    assert!(
        standard_family_payload.contains("resolve_local_standard_candidate_payload_parts("),
        "standard family payload.rs should consume request.rs preparation output"
    );
}

#[test]
fn ai_pipeline_same_format_provider_routes_request_preparation_through_request_payload_seams() {
    let same_format_provider_mod = read_workspace_file(
        "apps/aether-gateway/src/ai_pipeline/planner/passthrough/provider/family/mod.rs",
    );
    assert!(
        same_format_provider_mod.contains("mod request;"),
        "same-format provider mod.rs should wire request seam"
    );
    assert!(
        !workspace_file_exists(
            "apps/aether-gateway/src/ai_pipeline/planner/passthrough/provider/family/payload/prepare.rs"
        ),
        "same-format provider payload/prepare.rs should stay removed after request seam extraction"
    );

    let same_format_provider_request = read_workspace_file(
        "apps/aether-gateway/src/ai_pipeline/planner/passthrough/provider/family/request.rs",
    );
    assert!(
        same_format_provider_request.contains("mod prepare;"),
        "same-format provider request.rs should own its nested prepare module"
    );
    assert!(
        !same_format_provider_request.contains("#[path = \"payload/prepare.rs\"]"),
        "same-format provider request.rs should not path-import payload preparation after seam extraction"
    );
    for pattern in [
        "pub(crate) struct LocalSameFormatProviderCandidatePayloadParts {",
        "pub(crate) async fn resolve_local_same_format_provider_candidate_payload_parts(",
    ] {
        assert!(
            same_format_provider_request.contains(pattern),
            "same-format provider request.rs should own {pattern}"
        );
    }

    let same_format_provider_request_prepare = read_workspace_file(
        "apps/aether-gateway/src/ai_pipeline/planner/passthrough/provider/family/request/prepare.rs",
    );
    for pattern in [
        "pub(super) struct PreparedSameFormatProviderCandidate {",
        "pub(super) async fn prepare_local_same_format_provider_candidate(",
    ] {
        assert!(
            same_format_provider_request_prepare.contains(pattern),
            "same-format provider request/prepare.rs should own {pattern}"
        );
    }

    let same_format_provider_payload = read_workspace_file(
        "apps/aether-gateway/src/ai_pipeline/planner/passthrough/provider/family/payload.rs",
    );
    assert!(
        same_format_provider_payload
            .contains("resolve_local_same_format_provider_candidate_payload_parts("),
        "same-format provider payload.rs should consume request.rs preparation output"
    );
    assert!(
        !same_format_provider_payload.contains("prepare_local_same_format_provider_candidate("),
        "same-format provider payload.rs should not inline request preparation after seam extraction"
    );
}

#[test]
fn ai_pipeline_video_routes_request_preparation_through_request_payload_seams() {
    let specialized_video_mod =
        read_workspace_file("apps/aether-gateway/src/ai_pipeline/planner/specialized/video.rs");
    assert!(
        specialized_video_mod.contains("mod request;"),
        "specialized video mod.rs should wire request seam"
    );

    let specialized_video_request = read_workspace_file(
        "apps/aether-gateway/src/ai_pipeline/planner/specialized/video/request.rs",
    );
    for pattern in [
        "pub(super) struct LocalVideoCreateCandidatePayloadParts {",
        "pub(super) async fn resolve_local_video_create_candidate_payload_parts(",
        "fn build_provider_request_body(",
        "fn build_video_upstream_url(",
    ] {
        assert!(
            specialized_video_request.contains(pattern),
            "specialized video request.rs should own {pattern}"
        );
    }

    let specialized_video_decision = read_workspace_file(
        "apps/aether-gateway/src/ai_pipeline/planner/specialized/video/decision.rs",
    );
    assert!(
        specialized_video_decision.contains("resolve_local_video_create_candidate_payload_parts("),
        "specialized video decision.rs should consume request.rs preparation output"
    );
    for forbidden in [
        "resolve_candidate_mapped_model(",
        "build_provider_request_body(",
        "build_video_upstream_url(",
        "resolve_local_openai_bearer_auth(",
        "resolve_local_gemini_auth(",
    ] {
        assert!(
            !specialized_video_decision.contains(forbidden),
            "specialized video decision.rs should not inline request preparation step {forbidden}"
        );
    }
}

#[test]
fn ai_pipeline_files_routes_request_preparation_through_request_payload_seams() {
    let specialized_files_mod =
        read_workspace_file("apps/aether-gateway/src/ai_pipeline/planner/specialized/files.rs");
    assert!(
        specialized_files_mod.contains("mod request;"),
        "specialized files mod.rs should wire request seam"
    );

    let specialized_files_request = read_workspace_file(
        "apps/aether-gateway/src/ai_pipeline/planner/specialized/files/request.rs",
    );
    for pattern in [
        "pub(super) struct LocalGeminiFilesCandidatePayloadParts {",
        "pub(super) async fn resolve_local_gemini_files_candidate_payload_parts(",
    ] {
        assert!(
            specialized_files_request.contains(pattern),
            "specialized files request.rs should own {pattern}"
        );
    }

    let specialized_files_decision = read_workspace_file(
        "apps/aether-gateway/src/ai_pipeline/planner/specialized/files/decision.rs",
    );
    assert!(
        specialized_files_decision.contains("resolve_local_gemini_files_candidate_payload_parts("),
        "specialized files decision.rs should consume request.rs preparation output"
    );
    for forbidden in [
        "supports_local_gemini_transport_with_network(",
        "resolve_local_gemini_auth(",
        "apply_local_body_rules(",
        "apply_local_header_rules(",
        "build_gemini_files_passthrough_url(",
    ] {
        assert!(
            !specialized_files_decision.contains(forbidden),
            "specialized files decision.rs should not inline request preparation step {forbidden}"
        );
    }
}

#[test]
fn ai_pipeline_same_format_provider_root_request_separates_body_and_url_policy() {
    let provider_request = read_workspace_file(
        "apps/aether-gateway/src/ai_pipeline/planner/passthrough/provider/request.rs",
    );
    for pattern in [
        "mod body;",
        "mod url;",
        "pub(super) use self::body::build_same_format_provider_request_body;",
        "pub(super) use self::url::build_same_format_upstream_url;",
    ] {
        assert!(
            provider_request.contains(pattern),
            "passthrough/provider/request.rs should own request seam pattern {pattern}"
        );
    }
    for forbidden in [
        "fn build_same_format_provider_request_body(",
        "fn build_same_format_upstream_url(",
        "fn maybe_add_gemini_stream_alt_sse(",
        "fn extract_gemini_model_from_path(",
    ] {
        assert!(
            !provider_request.contains(forbidden),
            "passthrough/provider/request.rs should not inline request policy helper {forbidden}"
        );
    }

    let provider_request_body = read_workspace_file(
        "apps/aether-gateway/src/ai_pipeline/planner/passthrough/provider/request/body.rs",
    );
    assert!(
        provider_request_body.contains("fn build_same_format_provider_request_body("),
        "passthrough/provider/request/body.rs should own same-format provider request-body policy"
    );

    let provider_request_url = read_workspace_file(
        "apps/aether-gateway/src/ai_pipeline/planner/passthrough/provider/request/url.rs",
    );
    for pattern in [
        "fn build_same_format_upstream_url(",
        "fn maybe_add_gemini_stream_alt_sse(",
    ] {
        assert!(
            provider_request_url.contains(pattern),
            "passthrough/provider/request/url.rs should own {pattern}"
        );
    }

    let same_format_provider_request = read_workspace_file(
        "apps/aether-gateway/src/ai_pipeline/planner/passthrough/provider/family/request.rs",
    );
    for pattern in [
        "super::super::request::build_same_format_provider_request_body(",
        "super::super::request::build_same_format_upstream_url(",
    ] {
        assert!(
            same_format_provider_request.contains(pattern),
            "same-format provider family request should consume root request seam via {pattern}"
        );
    }
}

#[test]
fn ai_pipeline_openai_chat_routes_request_preparation_through_request_payload_seams() {
    let openai_chat_decision = read_workspace_file(
        "apps/aether-gateway/src/ai_pipeline/planner/standard/openai/chat/decision.rs",
    );
    for pattern in [
        "#[path = \"decision/payload.rs\"]",
        "#[path = \"decision/request.rs\"]",
        "pub(super) use self::payload::maybe_build_local_openai_chat_decision_payload_for_candidate;",
    ] {
        assert!(
            openai_chat_decision.contains(pattern),
            "openai chat decision.rs should wire {pattern}"
        );
    }

    let openai_chat_request = read_workspace_file(
        "apps/aether-gateway/src/ai_pipeline/planner/standard/openai/chat/decision/request.rs",
    );
    for pattern in [
        "pub(crate) struct LocalOpenAiChatCandidatePayloadParts {",
        "pub(crate) async fn resolve_local_openai_chat_candidate_payload_parts(",
    ] {
        assert!(
            openai_chat_request.contains(pattern),
            "openai chat request.rs should own {pattern}"
        );
    }

    let openai_chat_payload = read_workspace_file(
        "apps/aether-gateway/src/ai_pipeline/planner/standard/openai/chat/decision/payload.rs",
    );
    assert!(
        openai_chat_payload.contains("resolve_local_openai_chat_candidate_payload_parts("),
        "openai chat payload.rs should consume request.rs preparation output"
    );
}

#[test]
fn ai_pipeline_payload_metadata_owns_local_execution_decision_response_shape() {
    let planner_mod = read_workspace_file("apps/aether-gateway/src/ai_pipeline/planner/mod.rs");
    assert!(
        planner_mod.contains("mod payload_metadata;"),
        "planner/mod.rs should wire payload_metadata helper module"
    );

    let payload_metadata =
        read_workspace_file("apps/aether-gateway/src/ai_pipeline/planner/payload_metadata.rs");
    for pattern in [
        "pub(crate) struct LocalExecutionDecisionResponseParts {",
        "pub(crate) fn build_local_execution_decision_response(",
        "pub(crate) fn local_execution_decision_action(",
    ] {
        assert!(
            payload_metadata.contains(pattern),
            "planner/payload_metadata.rs should own {pattern}"
        );
    }

    for path in [
        "apps/aether-gateway/src/ai_pipeline/planner/standard/family/payload.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/passthrough/provider/family/payload.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/standard/openai/chat/decision/payload.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/standard/openai/cli/decision/payload.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/specialized/video/decision.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/specialized/files/decision.rs",
    ] {
        let source = read_workspace_file(path);
        assert!(
            source.contains("build_local_execution_decision_response("),
            "{path} should route local decision payload construction through payload_metadata"
        );
        assert!(
            !source.contains("GatewayControlSyncDecisionResponse {"),
            "{path} should not inline GatewayControlSyncDecisionResponse construction after payload metadata extraction"
        );
    }
}

#[test]
fn ai_pipeline_report_context_owns_local_execution_context_shape() {
    let planner_mod = read_workspace_file("apps/aether-gateway/src/ai_pipeline/planner/mod.rs");
    assert!(
        planner_mod.contains("mod report_context;"),
        "planner/mod.rs should wire report_context helper module"
    );

    let report_context =
        read_workspace_file("apps/aether-gateway/src/ai_pipeline/planner/report_context.rs");
    for pattern in [
        "pub(crate) struct LocalExecutionReportContextParts<'a> {",
        "pub(crate) fn build_local_execution_report_context(",
    ] {
        assert!(
            report_context.contains(pattern),
            "planner/report_context.rs should own {pattern}"
        );
    }

    for path in [
        "apps/aether-gateway/src/ai_pipeline/planner/standard/family/payload.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/passthrough/provider/family/payload.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/standard/openai/chat/decision/payload.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/standard/openai/cli/decision/payload.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/specialized/video/decision.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/specialized/files/decision.rs",
    ] {
        let source = read_workspace_file(path);
        assert!(
            source.contains("build_local_execution_report_context("),
            "{path} should route report-context base construction through report_context.rs"
        );
        for forbidden in [
            "\"original_headers\": collect_control_headers(&parts.headers)",
            "\"retry_index\": 0,",
        ] {
            assert!(
                !source.contains(forbidden),
                "{path} should not inline shared report-context base field {forbidden}"
            );
        }
    }
}

#[test]
fn ai_pipeline_standard_attempts_consume_eligible_local_candidates_without_transport_rereads() {
    let openai_chat_support = read_workspace_file(
        "apps/aether-gateway/src/ai_pipeline/planner/standard/openai/chat/decision/support.rs",
    );
    assert!(
        openai_chat_support
            .contains("LocalExecutionCandidateAttempt as LocalOpenAiChatCandidateAttempt"),
        "openai chat attempts should reuse shared LocalExecutionCandidateAttempt"
    );

    let openai_cli_support = read_workspace_file(
        "apps/aether-gateway/src/ai_pipeline/planner/standard/openai/cli/decision/support.rs",
    );
    assert!(
        openai_cli_support
            .contains("LocalExecutionCandidateAttempt as LocalOpenAiCliCandidateAttempt"),
        "openai cli attempts should reuse shared LocalExecutionCandidateAttempt"
    );

    let standard_family_mod =
        read_workspace_file("apps/aether-gateway/src/ai_pipeline/planner/standard/family/mod.rs");
    assert!(
        standard_family_mod
            .contains("LocalExecutionCandidateAttempt as LocalStandardCandidateAttempt"),
        "standard family attempts should reuse shared LocalExecutionCandidateAttempt"
    );

    for path in [
        "apps/aether-gateway/src/ai_pipeline/planner/standard/openai/chat/decision.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/standard/openai/chat/decision/request.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/standard/openai/cli/decision/request.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/standard/family/request.rs",
    ] {
        let source = read_workspace_file(path);
        assert!(
            !source.contains("read_provider_transport_snapshot("),
            "{path} should consume eligibility-owned transport snapshots instead of rereading them"
        );
    }
}

#[test]
fn ai_pipeline_specialized_files_attempts_consume_eligible_local_candidates_without_transport_rereads(
) {
    let specialized_files_support = read_workspace_file(
        "apps/aether-gateway/src/ai_pipeline/planner/specialized/files/support.rs",
    );
    assert!(
        specialized_files_support
            .contains("LocalExecutionCandidateAttempt as LocalGeminiFilesCandidateAttempt"),
        "specialized files attempts should reuse shared LocalExecutionCandidateAttempt"
    );
    assert!(
        specialized_files_support
            .contains("filter_and_rank_local_execution_candidates_without_transport_pair_gate("),
        "specialized files support should source runtime gating from candidate_eligibility"
    );
    assert!(
        !specialized_files_support.contains("rank_local_execution_candidates("),
        "specialized files support should not bypass candidate_eligibility with raw affinity ranking"
    );

    let specialized_files_decision = read_workspace_file(
        "apps/aether-gateway/src/ai_pipeline/planner/specialized/files/decision.rs",
    );
    assert!(
        !specialized_files_decision.contains("read_provider_transport_snapshot("),
        "specialized files decision should consume eligibility-owned transport snapshots instead of rereading them"
    );
}

#[test]
fn ai_pipeline_candidate_sources_share_cross_format_auth_filter_helper() {
    let planner_mod = read_workspace_file("apps/aether-gateway/src/ai_pipeline/planner/mod.rs");
    assert!(
        planner_mod.contains("mod candidate_source;"),
        "planner/mod.rs should wire candidate_source helper module"
    );

    let candidate_source =
        read_workspace_file("apps/aether-gateway/src/ai_pipeline/planner/candidate_source.rs");
    assert!(
        candidate_source.contains("pub(crate) fn auth_snapshot_allows_cross_format_candidate("),
        "planner/candidate_source.rs should own shared cross-format auth filtering"
    );

    for path in [
        "apps/aether-gateway/src/ai_pipeline/planner/standard/openai/chat/plans/candidates.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/standard/openai/cli/decision/support.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/standard/family/candidates.rs",
    ] {
        let source = read_workspace_file(path);
        assert!(
            source.contains("auth_snapshot_allows_cross_format_candidate("),
            "{path} should use the shared cross-format auth filter helper"
        );
    }
}

#[test]
fn ai_pipeline_spec_metadata_owns_family_requested_model_and_plan_builder_routing() {
    let planner_mod = read_workspace_file("apps/aether-gateway/src/ai_pipeline/planner/mod.rs");
    assert!(
        planner_mod.contains("mod spec_metadata;"),
        "planner/mod.rs should wire spec_metadata helper module"
    );

    let spec_metadata =
        read_workspace_file("apps/aether-gateway/src/ai_pipeline/planner/spec_metadata.rs");
    for pattern in [
        "pub(crate) struct LocalExecutionSurfaceSpecMetadata {",
        "pub(crate) fn requested_model_family_for_standard_source(",
        "pub(crate) fn requested_model_family_for_same_format_provider(",
        "pub(crate) fn requested_model_family_for_video_create(",
        "pub(crate) fn local_standard_spec_metadata(",
        "pub(crate) fn local_same_format_provider_spec_metadata(",
        "pub(crate) fn local_openai_cli_spec_metadata(",
        "pub(crate) fn local_gemini_files_spec_metadata(",
        "pub(crate) fn local_video_create_spec_metadata(",
        "pub(crate) fn build_sync_plan_from_requested_model_family(",
        "pub(crate) fn build_stream_plan_from_requested_model_family(",
    ] {
        assert!(
            spec_metadata.contains(pattern),
            "planner/spec_metadata.rs should own {pattern}"
        );
    }

    for (path, pattern) in [
        (
            "apps/aether-gateway/src/ai_pipeline/planner/standard/family/candidates.rs",
            "local_standard_spec_metadata(",
        ),
        (
            "apps/aether-gateway/src/ai_pipeline/planner/standard/family/build.rs",
            "local_standard_spec_metadata(",
        ),
        (
            "apps/aether-gateway/src/ai_pipeline/planner/standard/family/request.rs",
            "local_standard_spec_metadata(",
        ),
        (
            "apps/aether-gateway/src/ai_pipeline/planner/standard/family/payload.rs",
            "local_standard_spec_metadata(",
        ),
        (
            "apps/aether-gateway/src/ai_pipeline/planner/passthrough/provider/family/candidates.rs",
            "local_same_format_provider_spec_metadata(",
        ),
        (
            "apps/aether-gateway/src/ai_pipeline/planner/passthrough/provider/plans.rs",
            "local_same_format_provider_spec_metadata(",
        ),
        (
            "apps/aether-gateway/src/ai_pipeline/planner/passthrough/provider/family/build.rs",
            "local_same_format_provider_spec_metadata(",
        ),
        (
            "apps/aether-gateway/src/ai_pipeline/planner/passthrough/provider/family/request/prepare.rs",
            "local_same_format_provider_spec_metadata(",
        ),
        (
            "apps/aether-gateway/src/ai_pipeline/planner/passthrough/provider/family/payload.rs",
            "local_same_format_provider_spec_metadata(",
        ),
        (
            "apps/aether-gateway/src/ai_pipeline/planner/specialized/video/support.rs",
            "local_video_create_spec_metadata(",
        ),
        (
            "apps/aether-gateway/src/ai_pipeline/planner/specialized/video.rs",
            "local_video_create_spec_metadata(",
        ),
        (
            "apps/aether-gateway/src/ai_pipeline/planner/specialized/video/decision.rs",
            "local_video_create_spec_metadata(",
        ),
        (
            "apps/aether-gateway/src/ai_pipeline/planner/specialized/files.rs",
            "local_gemini_files_spec_metadata(",
        ),
        (
            "apps/aether-gateway/src/ai_pipeline/planner/specialized/files/decision.rs",
            "local_gemini_files_spec_metadata(",
        ),
        (
            "apps/aether-gateway/src/ai_pipeline/planner/standard/openai/cli/plans.rs",
            "local_openai_cli_spec_metadata(",
        ),
        (
            "apps/aether-gateway/src/ai_pipeline/planner/standard/openai/cli/decision/support.rs",
            "local_openai_cli_spec_metadata(",
        ),
        (
            "apps/aether-gateway/src/ai_pipeline/planner/standard/openai/cli/decision/request.rs",
            "local_openai_cli_spec_metadata(",
        ),
        (
            "apps/aether-gateway/src/ai_pipeline/planner/standard/openai/cli/decision/payload.rs",
            "local_openai_cli_spec_metadata(",
        ),
        (
            "apps/aether-gateway/src/ai_pipeline/planner/standard/family/build.rs",
            "build_sync_plan_from_requested_model_family(",
        ),
        (
            "apps/aether-gateway/src/ai_pipeline/planner/standard/family/build.rs",
            "build_stream_plan_from_requested_model_family(",
        ),
        (
            "apps/aether-gateway/src/ai_pipeline/planner/passthrough/provider/plans.rs",
            "build_sync_plan_from_requested_model_family(",
        ),
        (
            "apps/aether-gateway/src/ai_pipeline/planner/passthrough/provider/plans.rs",
            "build_stream_plan_from_requested_model_family(",
        ),
    ] {
        let source = read_workspace_file(path);
        assert!(
            source.contains(pattern),
            "{path} should use shared spec metadata helper {pattern}"
        );
    }
}

#[test]
fn ai_pipeline_same_format_provider_request_policy_owns_provider_type_behavior() {
    let request_root = read_workspace_file(
        "apps/aether-gateway/src/ai_pipeline/planner/passthrough/provider/family/request.rs",
    );
    assert!(
        request_root.contains("mod policy;"),
        "same-format provider request seam should wire request/policy.rs"
    );

    let request_policy = read_workspace_file(
        "apps/aether-gateway/src/ai_pipeline/planner/passthrough/provider/family/request/policy.rs",
    );
    for pattern in [
        "pub(super) struct SameFormatProviderRequestBehavior {",
        "pub(super) fn classify_same_format_provider_request_behavior(",
        "pub(super) fn same_format_provider_transport_supported(",
        "pub(super) fn should_try_same_format_provider_oauth_auth(",
        "pub(super) fn resolve_same_format_provider_direct_auth(",
    ] {
        assert!(
            request_policy.contains(pattern),
            "same-format provider request policy should own {pattern}"
        );
    }

    let request_prepare = read_workspace_file(
        "apps/aether-gateway/src/ai_pipeline/planner/passthrough/provider/family/request/prepare.rs",
    );
    for pattern in [
        "classify_same_format_provider_request_behavior(",
        "same_format_provider_transport_supported(",
        "should_try_same_format_provider_oauth_auth(",
        "resolve_same_format_provider_direct_auth(",
    ] {
        assert!(
            request_prepare.contains(pattern),
            "same-format provider request prepare should route provider-type behavior through request/policy.rs via {pattern}"
        );
    }
    for forbidden in [
        ".eq_ignore_ascii_case(\"antigravity\")",
        ".eq_ignore_ascii_case(\"claude_code\")",
        ".eq_ignore_ascii_case(\"vertex_ai\")",
        ".eq_ignore_ascii_case(\"kiro\")",
        "supports_local_claude_code_transport_with_network(",
        "supports_local_kiro_request_transport_with_network(",
        "supports_local_vertex_api_key_gemini_transport_with_network(",
    ] {
        assert!(
            !request_prepare.contains(forbidden),
            "same-format provider request prepare should not inline provider-type policy {forbidden}"
        );
    }
}

#[test]
fn ai_pipeline_decision_inputs_share_authenticated_input_helper() {
    let planner_mod = read_workspace_file("apps/aether-gateway/src/ai_pipeline/planner/mod.rs");
    assert!(
        planner_mod.contains("mod decision_input;"),
        "planner/mod.rs should wire decision_input helper module"
    );

    let decision_input =
        read_workspace_file("apps/aether-gateway/src/ai_pipeline/planner/decision_input.rs");
    for pattern in [
        "pub(crate) struct ResolvedLocalDecisionAuthInput {",
        "pub(crate) struct LocalRequestedModelDecisionInput {",
        "pub(crate) struct LocalAuthenticatedDecisionInput {",
        "pub(crate) fn build_local_requested_model_decision_input(",
        "pub(crate) fn build_local_authenticated_decision_input(",
        "pub(crate) async fn resolve_local_authenticated_decision_input(",
    ] {
        assert!(
            decision_input.contains(pattern),
            "planner/decision_input.rs should own {pattern}"
        );
    }

    for path in [
        "apps/aether-gateway/src/ai_pipeline/planner/standard/family/candidates.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/passthrough/provider/family/candidates.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/standard/openai/cli/decision/support.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/standard/openai/chat/plans/resolve.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/specialized/video/support.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/specialized/files/support.rs",
    ] {
        let source = read_workspace_file(path);
        assert!(
            source.contains("resolve_local_authenticated_decision_input("),
            "{path} should use the shared authenticated decision input helper"
        );
        if path.ends_with("/standard/openai/cli/decision/support.rs")
            || path.ends_with("/standard/openai/chat/plans/resolve.rs")
        {
            assert!(
                source.contains("extract_standard_requested_model("),
                "{path} should use shared standard requested-model extraction"
            );
        } else if !path.ends_with("/specialized/files/support.rs") {
            assert!(
                source.contains("extract_requested_model_from_request("),
                "{path} should use shared family-aware requested-model extraction"
            );
        }
        for forbidden in [
            "read_auth_api_key_snapshot(",
            "resolve_request_candidate_required_capabilities(",
            "fn extract_gemini_model_from_path(",
            "fn extract_gemini_video_model_from_path(",
        ] {
            assert!(
                !source.contains(forbidden),
                "{path} should not inline authenticated decision input step {forbidden}"
            );
        }
    }

    for (path, pattern) in [
        (
            "apps/aether-gateway/src/ai_pipeline/planner/standard/family/mod.rs",
            "LocalRequestedModelDecisionInput as LocalStandardDecisionInput",
        ),
        (
            "apps/aether-gateway/src/ai_pipeline/planner/passthrough/provider/family/mod.rs",
            "LocalRequestedModelDecisionInput as LocalSameFormatProviderDecisionInput",
        ),
        (
            "apps/aether-gateway/src/ai_pipeline/planner/standard/openai/chat/decision/support.rs",
            "LocalRequestedModelDecisionInput as LocalOpenAiChatDecisionInput",
        ),
        (
            "apps/aether-gateway/src/ai_pipeline/planner/standard/openai/cli/decision/support.rs",
            "LocalRequestedModelDecisionInput as LocalOpenAiCliDecisionInput",
        ),
        (
            "apps/aether-gateway/src/ai_pipeline/planner/specialized/video/support.rs",
            "LocalRequestedModelDecisionInput as LocalVideoCreateDecisionInput",
        ),
        (
            "apps/aether-gateway/src/ai_pipeline/planner/specialized/files/support.rs",
            "LocalAuthenticatedDecisionInput as LocalGeminiFilesDecisionInput",
        ),
    ] {
        let source = read_workspace_file(path);
        assert!(
            source.contains(pattern),
            "{path} should rename shared decision input shapes instead of redefining local decision input structs"
        );
    }

    for (path, pattern) in [
        (
            "apps/aether-gateway/src/ai_pipeline/planner/standard/family/candidates.rs",
            "build_local_requested_model_decision_input(",
        ),
        (
            "apps/aether-gateway/src/ai_pipeline/planner/passthrough/provider/family/candidates.rs",
            "build_local_requested_model_decision_input(",
        ),
        (
            "apps/aether-gateway/src/ai_pipeline/planner/standard/openai/chat/plans/resolve.rs",
            "build_local_requested_model_decision_input(",
        ),
        (
            "apps/aether-gateway/src/ai_pipeline/planner/standard/openai/cli/decision/support.rs",
            "build_local_requested_model_decision_input(",
        ),
        (
            "apps/aether-gateway/src/ai_pipeline/planner/specialized/video/support.rs",
            "build_local_requested_model_decision_input(",
        ),
        (
            "apps/aether-gateway/src/ai_pipeline/planner/specialized/files/support.rs",
            "build_local_authenticated_decision_input(",
        ),
    ] {
        let source = read_workspace_file(path);
        assert!(
            source.contains(pattern),
            "{path} should build local decision inputs through shared decision_input builders"
        );
    }
}

#[test]
fn ai_pipeline_leaf_planner_owners_route_contract_specs_through_gateway_seams() {
    for path in [
        "apps/aether-gateway/src/ai_pipeline/planner/specialized/files/support.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/specialized/video/support.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/standard/openai/chat/decision/support.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/standard/openai/cli/decision/support.rs",
    ] {
        let source = read_workspace_file(path);
        assert!(
            !source.contains("aether_ai_pipeline::contracts::ExecutionRuntimeAuthContext"),
            "{path} should consume ExecutionRuntimeAuthContext through gateway ai_pipeline seams"
        );
        assert!(
            source.contains("crate::ai_pipeline::contracts::ExecutionRuntimeAuthContext"),
            "{path} should use gateway contracts seam for ExecutionRuntimeAuthContext"
        );
    }

    let specialized_files_decision = read_workspace_file(
        "apps/aether-gateway/src/ai_pipeline/planner/specialized/files/decision.rs",
    );
    assert!(
        !specialized_files_decision
            .contains("aether_ai_pipeline::planner::specialized::files::LocalGeminiFilesSpec"),
        "planner/specialized/files/decision.rs should consume LocalGeminiFilesSpec through the local specialized seam"
    );
    assert!(
        specialized_files_decision.contains("use super::LocalGeminiFilesSpec;"),
        "planner/specialized/files/decision.rs should use the local specialized seam for LocalGeminiFilesSpec"
    );

    let specialized_video_support = read_workspace_file(
        "apps/aether-gateway/src/ai_pipeline/planner/specialized/video/support.rs",
    );
    assert!(
        specialized_video_support.contains("use super::{LocalVideoCreateFamily, LocalVideoCreateSpec};"),
        "planner/specialized/video/support.rs should use local video seams for LocalVideoCreate* types"
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
        gateway_contracts_mod.contains("pub(crate) use crate::ai_pipeline::{"),
        "gateway contracts/mod.rs should thinly re-export pipeline crate contracts through the ai_pipeline root seam"
    );

    let gateway_route = read_workspace_file("apps/aether-gateway/src/ai_pipeline/planner/route.rs");
    let gateway_route_runtime = gateway_route
        .split("#[cfg(test)]")
        .next()
        .unwrap_or(gateway_route.as_str());
    assert!(
        gateway_route_runtime.contains("crate::ai_pipeline::"),
        "planner/route.rs should delegate route logic through the ai_pipeline root seam"
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

    let gateway_api = read_workspace_file("apps/aether-gateway/src/ai_pipeline_api.rs");
    for pattern in [
        "pub(crate) fn parse_direct_request_body(",
        "pub(crate) fn resolve_execution_runtime_stream_plan_kind(",
        "pub(crate) fn resolve_execution_runtime_sync_plan_kind(",
        "pub(crate) fn is_matching_stream_request(",
        "pub(crate) fn supports_sync_scheduler_decision_kind(",
        "pub(crate) fn supports_stream_scheduler_decision_kind(",
    ] {
        assert!(
            gateway_api.contains(pattern),
            "ai_pipeline_api.rs should own facade wrapper {pattern}"
        );
    }

    let planner_mod = read_workspace_file("apps/aether-gateway/src/ai_pipeline/planner/mod.rs");
    for pattern in [
        "pub(crate) use self::common::parse_direct_request_body;",
        "pub(crate) use self::route::{",
    ] {
        assert!(
            !planner_mod.contains(pattern),
            "planner/mod.rs should not act as facade hub for {pattern}"
        );
    }

    let finalize_mod = read_workspace_file("apps/aether-gateway/src/ai_pipeline/finalize/mod.rs");
    for pattern in [
        "pub(crate) use crate::api::response::{build_client_response, build_client_response_from_parts};",
        "pub(crate) use common::build_local_success_outcome;",
        "pub(crate) use internal::{",
    ] {
        assert!(
            !finalize_mod.contains(pattern),
            "finalize/mod.rs should not act as re-export hub for {pattern}"
        );
    }
}

#[test]
fn ai_pipeline_m5_moves_kiro_stream_helpers_into_pipeline_crate() {
    assert!(
        workspace_file_exists("crates/aether-ai-pipeline/src/adaptation/kiro_stream.rs"),
        "crates/aether-ai-pipeline/src/adaptation/kiro_stream.rs should exist after kiro helper extraction"
    );
    assert!(
        !workspace_file_exists("apps/aether-gateway/src/ai_pipeline/adaptation/kiro/stream/util.rs"),
        "apps/aether-gateway/src/ai_pipeline/adaptation/kiro/stream/util.rs should be removed after moving kiro helper ownership"
    );
}

#[test]
fn ai_pipeline_runtime_adapter_dead_duplicates_are_removed() {
    for path in [
        "apps/aether-gateway/src/ai_pipeline/runtime/adapters/antigravity/auth.rs",
        "apps/aether-gateway/src/ai_pipeline/runtime/adapters/antigravity/policy.rs",
        "apps/aether-gateway/src/ai_pipeline/runtime/adapters/antigravity/request.rs",
        "apps/aether-gateway/src/ai_pipeline/runtime/adapters/antigravity/url.rs",
        "apps/aether-gateway/src/ai_pipeline/runtime/adapters/vertex/auth.rs",
        "apps/aether-gateway/src/ai_pipeline/runtime/adapters/vertex/policy.rs",
        "apps/aether-gateway/src/ai_pipeline/runtime/adapters/vertex/url.rs",
        "apps/aether-gateway/src/ai_pipeline/runtime/adapters/claude_code/auth.rs",
        "apps/aether-gateway/src/ai_pipeline/runtime/adapters/claude_code/policy.rs",
        "apps/aether-gateway/src/ai_pipeline/runtime/adapters/claude_code/request.rs",
        "apps/aether-gateway/src/ai_pipeline/runtime/adapters/claude_code/url.rs",
        "apps/aether-gateway/src/ai_pipeline/runtime/adapters/openai/auth.rs",
        "apps/aether-gateway/src/ai_pipeline/runtime/adapters/openai/policy.rs",
        "apps/aether-gateway/src/ai_pipeline/runtime/adapters/openai/request.rs",
        "apps/aether-gateway/src/ai_pipeline/runtime/adapters/openai/url.rs",
        "apps/aether-gateway/src/ai_pipeline/runtime/adapters/gemini/auth.rs",
        "apps/aether-gateway/src/ai_pipeline/runtime/adapters/gemini/policy.rs",
        "apps/aether-gateway/src/ai_pipeline/runtime/adapters/gemini/request.rs",
        "apps/aether-gateway/src/ai_pipeline/runtime/adapters/gemini/url.rs",
        "apps/aether-gateway/src/ai_pipeline/runtime/adapters/claude/auth.rs",
        "apps/aether-gateway/src/ai_pipeline/runtime/adapters/claude/policy.rs",
        "apps/aether-gateway/src/ai_pipeline/runtime/adapters/claude/request.rs",
        "apps/aether-gateway/src/ai_pipeline/runtime/adapters/claude/url.rs",
    ] {
        assert!(
            !workspace_file_exists(path),
            "{path} should be removed after provider-transport ownership consolidation"
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
        conversion_mod.contains("crate::ai_pipeline::"),
        "gateway conversion/mod.rs should thinly re-export pipeline conversion through the ai_pipeline root seam"
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
    assert!(
        !workspace_file_exists("apps/aether-gateway/src/ai_pipeline/conversion/request/mod.rs"),
        "gateway conversion/request/mod.rs should be removed after root-seam consolidation"
    );
    let conversion_mod =
        read_workspace_file("apps/aether-gateway/src/ai_pipeline/conversion/mod.rs");
    assert!(
        !conversion_mod.contains("pub(crate) mod request;"),
        "gateway conversion/mod.rs should not keep request re-export shell after root-seam consolidation"
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
    assert!(
        !workspace_file_exists("apps/aether-gateway/src/ai_pipeline/conversion/response/mod.rs"),
        "gateway conversion/response/mod.rs should be removed after root-seam consolidation"
    );
    let conversion_mod =
        read_workspace_file("apps/aether-gateway/src/ai_pipeline/conversion/mod.rs");
    assert!(
        !conversion_mod.contains("pub(crate) mod response;"),
        "gateway conversion/mod.rs should not keep response re-export shell after root-seam consolidation"
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
            vec!["apps/aether-gateway/src/ai_pipeline/finalize/standard/mod.rs"],
            "convert_openai_cli_response_to_openai_chat",
        ),
        (
            vec!["apps/aether-gateway/src/ai_pipeline/finalize/standard/mod.rs"],
            "build_openai_cli_response",
        ),
        (
            vec!["apps/aether-gateway/src/ai_pipeline/finalize/standard/mod.rs"],
            "convert_openai_chat_response_to_openai_cli",
        ),
        (
            vec!["apps/aether-gateway/src/ai_pipeline/finalize/standard/mod.rs"],
            "convert_claude_chat_response_to_openai_chat",
        ),
        (
            vec!["apps/aether-gateway/src/ai_pipeline/finalize/standard/mod.rs"],
            "convert_openai_chat_response_to_claude_chat",
        ),
        (
            vec!["apps/aether-gateway/src/ai_pipeline/finalize/standard/mod.rs"],
            "convert_claude_cli_response_to_openai_cli",
        ),
        (
            vec!["apps/aether-gateway/src/ai_pipeline/finalize/standard/mod.rs"],
            "convert_gemini_chat_response_to_openai_chat",
        ),
        (
            vec!["apps/aether-gateway/src/ai_pipeline/finalize/standard/mod.rs"],
            "convert_openai_chat_response_to_gemini_chat",
        ),
        (
            vec!["apps/aether-gateway/src/ai_pipeline/finalize/standard/mod.rs"],
            "convert_gemini_cli_response_to_openai_cli",
        ),
    ] {
        let sources = candidate_paths
            .iter()
            .map(|path| read_workspace_file(path))
            .collect::<Vec<_>>();
        assert!(
            sources
                .iter()
                .any(|source| source.contains("crate::ai_pipeline::{") && source.contains(symbol)),
            "{symbol} should stay exposed through the ai_pipeline root seam from finalize/standard/mod.rs"
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

    for path in [
        "apps/aether-gateway/src/ai_pipeline/finalize/standard/openai/stream.rs",
        "apps/aether-gateway/src/ai_pipeline/finalize/standard/claude/stream.rs",
        "apps/aether-gateway/src/ai_pipeline/finalize/standard/gemini/stream.rs",
    ] {
        assert!(
            !workspace_file_exists(path),
            "{path} should be removed after finalize stream wrapper collapse"
        );
    }

    assert!(
        !workspace_file_exists(
            "apps/aether-gateway/src/ai_pipeline/finalize/standard/stream_core/common.rs"
        ),
        "stream_core/common.rs should be removed after canonical stream helper collapse"
    );

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
        gateway_standard.contains("crate::ai_pipeline::"),
        "gateway finalize/standard/mod.rs should thinly re-export sync_products through the gateway ai_pipeline root seam"
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
        if workspace_file_exists(path) {
            let source = read_workspace_file(path);
            assert!(
                !source.contains(forbidden),
                "{path} should not keep dead standard re-export {forbidden}"
            );
        }
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
        gateway_stream_rewrite.contains("crate::ai_pipeline::"),
        "gateway internal stream_rewrite should delegate rewrite-mode resolution through the gateway ai_pipeline root seam"
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
        gateway_common_runtime.contains("crate::ai_pipeline::"),
        "gateway planner/common.rs should delegate body parsing through the ai_pipeline root seam"
    );
    assert!(
        gateway_common_runtime
            .contains("force_upstream_streaming_for_provider as force_upstream_streaming_for_provider_impl"),
        "gateway planner/common.rs should delegate upstream streaming policy through the ai_pipeline root seam"
    );
    assert!(
        gateway_common_runtime
            .contains("extract_gemini_model_from_path as extract_gemini_model_from_path_impl"),
        "gateway planner/common.rs should delegate gemini request-path parsing through the ai_pipeline root seam"
    );

    for forbidden in [
        "serde_json::from_slice::<serde_json::Value>",
        "base64::engine::general_purpose::STANDARD.encode",
        ".eq_ignore_ascii_case(\"codex\")",
    ] {
        assert!(
            !gateway_common_runtime.contains(forbidden),
            "gateway planner/common.rs should not own parser implementation detail {forbidden}"
        );
    }
    for pattern in [
        "pub(crate) enum RequestedModelFamily {",
        "pub(crate) fn extract_standard_requested_model(",
        "pub(crate) fn extract_requested_model_from_request(",
        "pub(crate) fn build_local_runtime_miss_diagnostic(",
        "pub(crate) fn apply_local_candidate_evaluation_progress(",
        "pub(crate) fn apply_local_candidate_terminal_plan_reason(",
    ] {
        assert!(
            gateway_common_runtime.contains(pattern),
            "gateway planner/common.rs should own shared planner helper {pattern}"
        );
    }

    for path in [
        "apps/aether-gateway/src/ai_pipeline/planner/standard/family/build.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/passthrough/provider/plans.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/passthrough/provider/family/build.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/standard/openai/chat/plans/sync.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/standard/openai/chat/plans/stream.rs",
    ] {
        let source = read_workspace_file(path);
        if !path.contains("openai/chat/plans/") {
            assert!(
                source.contains("extract_requested_model_from_request("),
                "{path} should use shared requested-model extraction"
            );
        }
        for forbidden in [
            "fn extract_requested_model(",
            "fn build_local_standard_miss_diagnostic(",
            "fn build_local_same_format_miss_diagnostic(",
            "let skipped_candidate_count = diagnostic.skipped_candidate_count.unwrap_or(0);",
        ] {
            assert!(
                !source.contains(forbidden),
                "{path} should not inline shared planner helper {forbidden}"
            );
        }
    }

    let openai_chat_diagnostic = read_workspace_file(
        "apps/aether-gateway/src/ai_pipeline/planner/standard/openai/chat/plans/diagnostic.rs",
    );
    assert!(
        openai_chat_diagnostic.contains("set_local_runtime_miss_diagnostic_reason(")
            || openai_chat_diagnostic.contains("set_local_runtime_candidate_evaluation_diagnostic("),
        "openai chat diagnostic.rs should delegate miss diagnostic handling through planner/runtime_miss.rs"
    );
    assert!(
        !openai_chat_diagnostic.contains("skip_reasons:"),
        "openai chat diagnostic.rs should not inline miss diagnostic struct fields after helper extraction"
    );
}

#[test]
fn ai_pipeline_root_owns_shared_gemini_request_path_parser() {
    let ai_pipeline_mod = read_workspace_file("apps/aether-gateway/src/ai_pipeline/mod.rs");
    assert!(
        ai_pipeline_mod.contains("pub(crate) fn extract_gemini_model_from_path("),
        "ai_pipeline/mod.rs should own shared gemini request-path parsing"
    );

    let passthrough_provider_request = read_workspace_file(
        "apps/aether-gateway/src/ai_pipeline/planner/passthrough/provider/request.rs",
    );
    assert!(
        !passthrough_provider_request.contains("fn extract_gemini_model_from_path("),
        "passthrough/provider/request.rs should not locally own gemini request-path parsing"
    );

    let auth_credentials =
        read_workspace_file("apps/aether-gateway/src/control/auth/credentials.rs");
    assert!(
        auth_credentials.contains("ai_pipeline::extract_gemini_model_from_path"),
        "control/auth/credentials.rs should use ai_pipeline root seam for gemini request-path parsing"
    );
    assert!(
        !auth_credentials.contains("fn extract_gemini_model_from_path("),
        "control/auth/credentials.rs should not inline gemini request-path parsing"
    );
}

#[test]
fn ai_pipeline_planner_standard_normalize_is_owned_by_pipeline_crate() {
    assert!(
        workspace_file_exists("crates/aether-ai-pipeline/src/planner/standard/normalize.rs"),
        "planner/standard/normalize should live in aether-ai-pipeline"
    );

    let gateway_normalize =
        read_workspace_file("apps/aether-gateway/src/ai_pipeline/planner/standard/normalize.rs");
    let gateway_normalize_chat = read_workspace_file(
        "apps/aether-gateway/src/ai_pipeline/planner/standard/normalize/chat.rs",
    );
    let gateway_normalize_cli = read_workspace_file(
        "apps/aether-gateway/src/ai_pipeline/planner/standard/normalize/cli.rs",
    );
    assert!(
        gateway_normalize_chat.contains("crate::ai_pipeline::")
            && gateway_normalize_cli.contains("crate::ai_pipeline::"),
        "gateway normalize chat/cli owners should delegate to pipeline standard normalize helpers through the ai_pipeline root seam"
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
        gateway_openai_mod.contains("pub(crate) use crate::ai_pipeline::{"),
        "gateway planner/standard/openai/mod.rs should thinly re-export pipeline openai helpers through the ai_pipeline root seam"
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

    assert!(
        !workspace_file_exists("apps/aether-gateway/src/ai_pipeline/planner/standard/matrix.rs"),
        "planner/standard/matrix.rs should stay removed after wrapper cleanup"
    );

    let matrix = read_workspace_file("apps/aether-gateway/src/ai_pipeline/planner/standard/mod.rs");
    assert!(
        matrix.contains("crate::ai_pipeline::"),
        "planner/standard/mod.rs should delegate canonical conversion through the ai_pipeline root seam after matrix wrapper cleanup"
    );
    assert!(
        matrix.contains("build_standard_request_body"),
        "planner/standard/mod.rs should still expose build_standard_request_body after matrix wrapper cleanup"
    );
    assert!(
        matrix.contains("build_standard_upstream_url"),
        "planner/standard/mod.rs should still expose build_standard_upstream_url after matrix wrapper cleanup"
    );
    assert!(
        !matrix.contains("mod matrix;"),
        "planner/standard/mod.rs should not keep a local matrix wrapper module"
    );
    {
        let forbidden = "serde_json::Map::from_iter";
        assert!(
            !matrix.contains(forbidden),
            "planner/standard/mod.rs should not keep matrix conversion helper {forbidden}"
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

    assert!(
        !workspace_file_exists(
            "apps/aether-gateway/src/ai_pipeline/planner/standard/family/types.rs"
        ),
        "planner/standard/family/types.rs should stay removed after wrapper cleanup"
    );

    let family_types =
        read_workspace_file("apps/aether-gateway/src/ai_pipeline/planner/standard/family/mod.rs");
    assert!(
        family_types.contains("pub(crate) use crate::ai_pipeline::{"),
        "gateway planner/standard/family/mod.rs should re-export pure family spec types through the ai_pipeline root seam"
    );
    for forbidden in [
        "pub(crate) enum LocalStandardSourceFamily",
        "pub(crate) enum LocalStandardSourceMode",
        "pub(crate) struct LocalStandardSpec",
    ] {
        assert!(
            !family_types.contains(forbidden),
            "gateway planner/standard/family/mod.rs should not own pure spec type {forbidden}"
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

    for path in [
        "apps/aether-gateway/src/ai_pipeline/planner/standard/claude/mod.rs",
        "apps/aether-gateway/src/ai_pipeline/planner/standard/gemini/mod.rs",
    ] {
        let source = read_workspace_file(path);
        assert!(
            source.contains("crate::ai_pipeline::"),
            "{path} should delegate pure standard-family spec resolution through the ai_pipeline root seam"
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

    assert!(
        !workspace_file_exists(
            "apps/aether-gateway/src/ai_pipeline/planner/passthrough/provider/family/types.rs"
        ),
        "planner/passthrough/provider/family/types.rs should stay removed after wrapper cleanup"
    );

    let family_types = read_workspace_file(
        "apps/aether-gateway/src/ai_pipeline/planner/passthrough/provider/family/mod.rs",
    );
    assert!(
        family_types.contains("pub(crate) use crate::ai_pipeline::"),
        "gateway passthrough/provider/family/mod.rs should re-export pure same-format provider spec types through the ai_pipeline root seam"
    );
    for forbidden in [
        "pub(crate) enum LocalSameFormatProviderFamily",
        "pub(crate) struct LocalSameFormatProviderSpec",
    ] {
        assert!(
            !family_types.contains(forbidden),
            "gateway passthrough/provider/family/mod.rs should not own pure same-format type {forbidden}"
        );
    }

    let plans = read_workspace_file(
        "apps/aether-gateway/src/ai_pipeline/planner/passthrough/provider/plans.rs",
    );
    assert!(
        plans.contains("crate::ai_pipeline::"),
        "gateway passthrough/provider/plans.rs should delegate same-format spec resolution through the ai_pipeline root seam"
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
        "apps/aether-gateway/src/ai_pipeline/planner/passthrough/provider/family/mod.rs",
    );
    assert!(
        family_types.contains("pub(crate) use crate::ai_pipeline::"),
        "gateway passthrough/provider/family/mod.rs should re-export pure spec types through the ai_pipeline root seam"
    );
    for forbidden in [
        "pub(crate) enum LocalSameFormatProviderFamily",
        "pub(crate) struct LocalSameFormatProviderSpec",
    ] {
        assert!(
            !family_types.contains(forbidden),
            "gateway passthrough/provider/family/mod.rs should not own pure spec type {forbidden}"
        );
    }

    let plans = read_workspace_file(
        "apps/aether-gateway/src/ai_pipeline/planner/passthrough/provider/plans.rs",
    );
    assert!(
        plans.contains("crate::ai_pipeline::"),
        "gateway passthrough/provider/plans.rs should delegate same-format spec resolution through the ai_pipeline root seam"
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
        files.contains("crate::ai_pipeline::"),
        "gateway planner/specialized/files.rs should delegate pure specialized-files spec resolution through the ai_pipeline root seam"
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
        video.contains("crate::ai_pipeline::"),
        "gateway planner/specialized/video.rs should delegate pure specialized-video spec resolution through the ai_pipeline root seam"
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
        decision.contains("pub(super) use crate::ai_pipeline::LocalOpenAiCliSpec;"),
        "gateway planner/standard/openai/cli/decision.rs should re-export pure openai-cli spec type through the ai_pipeline root seam"
    );
    assert!(
        !decision.contains("pub(super) struct LocalOpenAiCliSpec"),
        "gateway planner/standard/openai/cli/decision.rs should not own LocalOpenAiCliSpec"
    );

    let plans = read_workspace_file(
        "apps/aether-gateway/src/ai_pipeline/planner/standard/openai/cli/plans.rs",
    );
    assert!(
        plans.contains("crate::ai_pipeline::"),
        "gateway planner/standard/openai/cli/plans.rs should delegate openai-cli spec resolution through the ai_pipeline root seam"
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
