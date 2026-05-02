use super::*;

#[test]
fn ai_serving_target_structure_removes_legacy_pipeline_boundary() {
    assert!(
        !workspace_file_exists("crates/aether-ai-pipeline"),
        "legacy aether-ai-pipeline crate should be fully removed"
    );
    assert!(
        !workspace_file_exists("apps/aether-gateway/src/ai_pipeline"),
        "gateway ai_pipeline module should be fully removed"
    );
    assert!(
        !workspace_file_exists("apps/aether-gateway/src/ai_pipeline_api.rs"),
        "gateway ai_pipeline_api facade should be fully removed"
    );

    let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root should resolve");
    let forbidden = [
        "aether-ai-pipeline",
        "aether_ai_pipeline",
        "ai_pipeline::",
        "ai_pipeline_api",
        "PipelineFinalizeError",
    ];
    let mut violations = Vec::new();
    for root in [
        "apps/aether-gateway/src",
        "crates/aether-ai-serving/src",
        "crates/aether-ai-formats/src",
    ] {
        for file in collect_workspace_rust_files(root) {
            let relative = file
                .canonicalize()
                .expect("workspace file should canonicalize")
                .strip_prefix(&workspace_root)
                .expect("workspace file should be under workspace root")
                .to_string_lossy()
                .replace('\\', "/");
            if relative.starts_with("apps/aether-gateway/src/tests/") {
                continue;
            }
            let source = std::fs::read_to_string(&file).expect("source file should be readable");
            let hits = forbidden
                .iter()
                .filter(|pattern| source.contains(**pattern))
                .copied()
                .collect::<Vec<_>>();
            if !hits.is_empty() {
                violations.push(format!("{relative} -> {}", hits.join(", ")));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "legacy ai-pipeline identifiers should not remain in active AI serving code:\n{}",
        violations.join("\n")
    );

    let workspace_manifest = read_workspace_file("Cargo.toml");
    assert!(
        !workspace_manifest.contains("aether-ai-pipeline"),
        "workspace manifest should not register the retired AI pipeline crate"
    );
    let gateway_manifest = read_workspace_file("apps/aether-gateway/Cargo.toml");
    assert!(
        !gateway_manifest.contains("aether-ai-pipeline"),
        "gateway manifest should not depend on the retired AI pipeline crate"
    );
}

#[test]
fn ai_serving_crate_owns_attempt_loop_without_gateway_runtime_deps() {
    let serving_manifest = read_workspace_file("crates/aether-ai-serving/Cargo.toml");
    for forbidden in ["axum", "sqlx", "redis", "aether-gateway"] {
        assert!(
            !serving_manifest.contains(forbidden),
            "aether-ai-serving should not depend on gateway/runtime adapter dependency {forbidden}"
        );
    }

    let mut violations = Vec::new();
    for file in collect_workspace_rust_files("crates/aether-ai-serving/src") {
        let source = std::fs::read_to_string(&file).expect("source file should be readable");
        let hits = ["AppState", "axum::", "sqlx::", "redis::"]
            .iter()
            .filter(|pattern| source.contains(**pattern))
            .copied()
            .collect::<Vec<_>>();
        if !hits.is_empty() {
            violations.push(format!("{} -> {}", file.display(), hits.join(", ")));
        }
    }
    assert!(
        violations.is_empty(),
        "aether-ai-serving should expose ports/use cases without concrete gateway runtime deps:\n{}",
        violations.join("\n")
    );

    let serving_attempt_loop = read_workspace_file("crates/aether-ai-serving/src/attempt_loop.rs");
    for pattern in [
        "pub trait AiExecutionAttempt",
        "pub trait AiAttemptLoopPort",
        "pub async fn run_ai_attempt_loop",
        "AiAttemptLoopOutcome::Responded",
        "AiAttemptLoopOutcome::Exhausted",
    ] {
        assert!(
            serving_attempt_loop.contains(pattern),
            "aether-ai-serving attempt loop should own serving state-machine primitive {pattern}"
        );
    }

    let gateway_candidate_loop =
        read_workspace_file("apps/aether-gateway/src/executor/candidate_loop.rs");
    for pattern in [
        "run_ai_attempt_loop(&port",
        "impl<T> AiAttemptLoopPort<T> for SyncAttemptLoopPort",
        "impl<T> AiAttemptLoopPort<T> for StreamAttemptLoopPort",
    ] {
        assert!(
            gateway_candidate_loop.contains(pattern),
            "gateway candidate loop should implement serving ports and delegate attempt-loop policy through {pattern}"
        );
    }

    let serving_decision_path =
        read_workspace_file("crates/aether-ai-serving/src/decision_path.rs");
    for pattern in [
        "pub enum AiSyncDecisionStep",
        "pub enum AiStreamDecisionStep",
        "pub trait AiSyncDecisionPathPort",
        "pub trait AiStreamDecisionPathPort",
        "pub async fn run_ai_sync_decision_path",
        "pub async fn run_ai_stream_decision_path",
    ] {
        assert!(
            serving_decision_path.contains(pattern),
            "aether-ai-serving decision path should own serving use-case primitive {pattern}"
        );
    }

    let gateway_sync_decision_path =
        read_workspace_file("apps/aether-gateway/src/ai_serving/planner/decision/sync.rs");
    for pattern in [
        "run_ai_sync_decision_path(&port",
        "impl AiSyncDecisionPathPort for GatewaySyncDecisionPathPort",
    ] {
        assert!(
            gateway_sync_decision_path.contains(pattern),
            "gateway sync decision path should implement serving ports and delegate decision-path policy through {pattern}"
        );
    }

    let gateway_stream_decision_path =
        read_workspace_file("apps/aether-gateway/src/ai_serving/planner/decision/stream.rs");
    for pattern in [
        "run_ai_stream_decision_path(&port",
        "impl AiStreamDecisionPathPort for GatewayStreamDecisionPathPort",
    ] {
        assert!(
            gateway_stream_decision_path.contains(pattern),
            "gateway stream decision path should implement serving ports and delegate decision-path policy through {pattern}"
        );
    }

    let serving_plan_payload = read_workspace_file("crates/aether-ai-serving/src/plan_payload.rs");
    for pattern in [
        "pub fn build_ai_sync_execution_plan_payload",
        "pub fn build_ai_stream_execution_plan_payload",
        "EXECUTION_RUNTIME_SYNC_ACTION",
        "EXECUTION_RUNTIME_STREAM_ACTION",
    ] {
        assert!(
            serving_plan_payload.contains(pattern),
            "aether-ai-serving should own execution plan payload output shape {pattern}"
        );
    }

    let gateway_control_plan =
        read_workspace_file("apps/aether-gateway/src/ai_serving/planner/decision/control_plan.rs");
    for pattern in [
        "build_ai_sync_execution_plan_payload(plan_kind",
        "build_ai_stream_execution_plan_payload(plan_kind",
    ] {
        assert!(
            gateway_control_plan.contains(pattern),
            "gateway control_plan.rs should delegate plan payload wrapping through aether-ai-serving via {pattern}"
        );
    }
    for forbidden in [
        "fn build_sync_plan_response(",
        "fn build_stream_plan_response(",
        "EXECUTION_RUNTIME_SYNC_ACTION.to_string()",
        "EXECUTION_RUNTIME_STREAM_ACTION.to_string()",
    ] {
        assert!(
            !gateway_control_plan.contains(forbidden),
            "gateway control_plan.rs should not own execution plan payload output shape {forbidden}"
        );
    }

    let serving_attempt_plan = read_workspace_file("crates/aether-ai-serving/src/attempt_plan.rs");
    for pattern in [
        "pub fn build_ai_execution_decision_from_plan",
        "pub fn infer_ai_upstream_base_url",
        "pub fn extract_ai_auth_header_pair",
    ] {
        assert!(
            serving_attempt_plan.contains(pattern),
            "aether-ai-serving should own execution-plan to decision mapping helper {pattern}"
        );
    }
    for path in [
        "apps/aether-gateway/src/ai_serving/planner/decision/sync.rs",
        "apps/aether-gateway/src/ai_serving/planner/decision/stream.rs",
    ] {
        let source = read_workspace_file(path);
        assert!(
            source.contains("build_ai_execution_decision_from_plan("),
            "{path} should delegate ExecutionPlan -> AiExecutionDecision mapping to aether-ai-serving"
        );
        for forbidden in [
            "fn infer_upstream_base_url(",
            "fn extract_auth_header_pair(",
            "ExecutionStrategy::LocalSameFormat",
            "ConversionMode::Bidirectional",
        ] {
            assert!(
                !source.contains(forbidden),
                "{path} should not keep serving decision mapping helper {forbidden}"
            );
        }
    }

    let serving_candidate_materialization =
        read_workspace_file("crates/aether-ai-serving/src/candidate_materialization.rs");
    for pattern in [
        "pub trait AiCandidateMaterializationPort",
        "pub async fn run_ai_candidate_materialization",
        "resolve_and_rank_candidates",
        "decorate_skipped_candidate",
        "remember_first_candidate_affinity",
        "persist_available_candidates",
        "persist_skipped_candidates",
        "AiCandidateMaterializationOutcome",
    ] {
        assert!(
            serving_candidate_materialization.contains(pattern),
            "aether-ai-serving should own candidate materialization use-case primitive {pattern}"
        );
    }

    let gateway_candidate_materialization = read_workspace_file(
        "apps/aether-gateway/src/ai_serving/planner/candidate_materialization.rs",
    );
    for pattern in [
        "run_ai_candidate_materialization(&port",
        "impl<F, G> AiCandidateMaterializationPort for GatewayLocalCandidateMaterializationPort",
        "materialize_local_execution_candidates_with_serving",
    ] {
        assert!(
            gateway_candidate_materialization.contains(pattern),
            "gateway candidate materialization should implement serving ports and delegate materialization policy through {pattern}"
        );
    }

    let serving_candidate_preselection =
        read_workspace_file("crates/aether-ai-serving/src/candidate_preselection.rs");
    for pattern in [
        "pub trait AiCandidatePreselectionPort",
        "pub async fn run_ai_candidate_preselection",
        "candidate_api_formats",
        "candidate_api_format_matches_client",
        "list_candidates_for_api_format",
        "candidate_allowed",
        "skipped_candidate_allowed",
        "candidate_key",
        "skipped_candidate_key",
        "AiCandidatePreselectionOutcome",
    ] {
        assert!(
            serving_candidate_preselection.contains(pattern),
            "aether-ai-serving should own candidate preselection use-case primitive {pattern}"
        );
    }

    let serving_candidate_ranking =
        read_workspace_file("crates/aether-ai-serving/src/candidate_ranking.rs");
    for pattern in [
        "pub trait AiCandidateRankingPort",
        "pub async fn run_ai_candidate_ranking",
        "pub fn build_ai_rankable_candidate",
        "pub fn ai_ranking_context",
        "pub struct AiRankableCandidateParts",
        "pub struct AiRankingContextConfig",
        "affinity_requested_model",
        "read_cached_affinity_target",
        "cached_affinity_matches",
        "build_rankable_candidate",
        "apply_scheduler_candidate_ranking",
        "apply_ranking_outcome",
    ] {
        assert!(
            serving_candidate_ranking.contains(pattern),
            "aether-ai-serving should own candidate ranking use-case primitive {pattern}"
        );
    }

    let serving_candidate_resolution =
        read_workspace_file("crates/aether-ai-serving/src/candidate_resolution.rs");
    for pattern in [
        "pub trait AiCandidateResolutionPort",
        "pub async fn run_ai_candidate_resolution",
        "pub enum AiCandidateResolutionMode",
        "pub struct AiCandidateResolutionRequest",
        "fn candidate_skip_reason_for_mode",
        "extract_ai_pool_sticky_session_token",
        "read_candidate_transport",
        "build_missing_transport_skipped_candidate",
        "candidate_common_skip_reason",
        "candidate_transport_pair_skip_reason",
        "build_skipped_candidate",
        "build_eligible_candidate",
        "rank_eligible_candidates",
        "apply_pool_scheduler",
        "AiCandidateResolutionOutcome",
    ] {
        assert!(
            serving_candidate_resolution.contains(pattern),
            "aether-ai-serving should own candidate resolution use-case primitive {pattern}"
        );
    }

    let gateway_candidate_resolution =
        read_workspace_file("apps/aether-gateway/src/ai_serving/planner/candidate_resolution.rs");
    for pattern in [
        "run_ai_candidate_resolution(&port",
        "impl AiCandidateResolutionPort for GatewayLocalCandidateResolutionPort",
        "candidate_common_transport_skip_reason(",
        "candidate_transport_pair_skip_reason(",
        "candidate_transport_policy_facts(",
    ] {
        assert!(
            gateway_candidate_resolution.contains(pattern),
            "gateway candidate_resolution.rs should implement serving ports and delegate resolution policy through {pattern}"
        );
    }
    for forbidden in [
        "pub(crate) enum LocalCandidateResolutionMode",
        "fn extract_pool_sticky_session_token(",
        "fixed_provider_key_inherits_api_formats(",
        "fn transport_key_supports_api_format(",
        "fn transport_key_allows_candidate_model(",
        "fn disabled_format_conversion_skip_reason(",
        "request_pair_allowed_for_transport(",
        "request_conversion_enabled_for_transport(",
        "request_conversion_requires_enable_flag(",
        "provider_type_is_fixed(",
        ".eq_ignore_ascii_case(\"kiro\")",
    ] {
        assert!(
            !gateway_candidate_resolution.contains(forbidden),
            "gateway candidate_resolution.rs should not own serving resolution policy helper {forbidden}"
        );
    }

    let provider_transport_conversion =
        read_workspace_file("crates/aether-provider-transport/src/conversion.rs");
    for pattern in [
        "pub struct CandidateTransportPolicyFacts",
        "pub fn candidate_common_transport_skip_reason(",
        "pub fn candidate_transport_pair_skip_reason(",
        "fn transport_key_supports_api_format(",
        "fn transport_key_allows_candidate_model(",
        "fn disabled_format_conversion_skip_reason(",
        "fixed_provider_key_inherits_api_formats(",
    ] {
        assert!(
            provider_transport_conversion.contains(pattern),
            "aether-provider-transport conversion.rs should own candidate transport policy {pattern}"
        );
    }

    let gateway_candidate_ranking =
        read_workspace_file("apps/aether-gateway/src/ai_serving/planner/candidate_ranking.rs");
    for pattern in [
        "run_ai_candidate_ranking(&port",
        "impl AiCandidateRankingPort for GatewayLocalCandidateRankingPort",
        "build_ai_rankable_candidate(",
        "ai_ranking_context(",
        "build_rankable_candidate",
        "apply_ranking_outcome",
    ] {
        assert!(
            gateway_candidate_ranking.contains(pattern),
            "gateway candidate_ranking.rs should implement serving ports and delegate ranking policy through {pattern}"
        );
    }
    for forbidden in [
        "fn rankable_candidate_from_candidate(",
        "fn planner_ranking_context(",
        "fn planner_ranking_mode(",
        "fn normalize_api_format_alias(",
        "fn api_format_matches(",
        "fn candidate_api_format_preference(",
        "requested_capability_priority_for_candidate",
        "request_candidate_api_format_preference",
    ] {
        assert!(
            !gateway_candidate_ranking.contains(forbidden),
            "gateway candidate_ranking.rs should not own serving ranking helper {forbidden}"
        );
    }

    for path in [
        "apps/aether-gateway/src/ai_serving/planner/standard/openai/chat/decision/support.rs",
        "apps/aether-gateway/src/ai_serving/planner/standard/openai/responses/decision/support.rs",
        "apps/aether-gateway/src/ai_serving/planner/standard/family/candidates.rs",
        "apps/aether-gateway/src/ai_serving/planner/passthrough/provider/family/candidates.rs",
        "apps/aether-gateway/src/ai_serving/planner/specialized/image/support.rs",
        "apps/aether-gateway/src/ai_serving/planner/specialized/video/support.rs",
        "apps/aether-gateway/src/ai_serving/planner/specialized/files/support.rs",
    ] {
        let source = read_workspace_file(path);
        assert!(
            source.contains("materialize_local_execution_candidates_with_serving("),
            "{path} should delegate local candidate materialization through the serving port adapter"
        );
        for forbidden in [
            "resolve_and_rank_local_execution_candidates(",
            "persist_available_local_execution_candidates_with_context(",
            "persist_skipped_local_execution_candidates_with_context(",
            "remember_first_local_candidate_affinity(",
        ] {
            assert!(
                !source.contains(forbidden),
                "{path} should not hand-roll candidate materialization sequence {forbidden}"
            );
        }
    }

    let serving_execution_path =
        read_workspace_file("crates/aether-ai-serving/src/execution_path.rs");
    for pattern in [
        "pub enum AiSyncExecutionStep",
        "pub enum AiStreamExecutionStep",
        "pub trait AiSyncExecutionPathPort",
        "pub trait AiStreamExecutionPathPort",
        "pub async fn run_ai_sync_execution_path",
        "pub async fn run_ai_stream_execution_path",
        "AiPlanFallbackReason::RemoteDecisionMiss",
        "AiPlanFallbackReason::SchedulerDecisionUnsupported",
    ] {
        assert!(
            serving_execution_path.contains(pattern),
            "aether-ai-serving execution path should own serving use-case primitive {pattern}"
        );
    }

    let gateway_sync_path = read_workspace_file("apps/aether-gateway/src/executor/sync_path.rs");
    for pattern in [
        "run_ai_sync_execution_path(&port",
        "impl AiSyncExecutionPathPort for GatewaySyncExecutionPathPort",
    ] {
        assert!(
            gateway_sync_path.contains(pattern),
            "gateway sync path should implement serving ports and delegate execution-path policy through {pattern}"
        );
    }

    let gateway_stream_path =
        read_workspace_file("apps/aether-gateway/src/executor/stream_path.rs");
    for pattern in [
        "run_ai_stream_execution_path(&port",
        "impl AiStreamExecutionPathPort for GatewayStreamExecutionPathPort",
    ] {
        assert!(
            gateway_stream_path.contains(pattern),
            "gateway stream path should implement serving ports and delegate execution-path policy through {pattern}"
        );
    }
}

#[test]
fn ai_serving_internal_dtos_use_ai_execution_names() {
    let serving_dto = read_workspace_file("crates/aether-ai-serving/src/dto.rs");
    for expected in [
        "pub struct AiExecutionDecision",
        "pub struct AiExecutionPlanPayload",
        "pub struct AiSyncAttempt",
        "pub struct AiStreamAttempt",
    ] {
        assert!(
            serving_dto.contains(expected),
            "aether-ai-serving dto.rs should own {expected}"
        );
    }

    let gateway_root = read_workspace_file("apps/aether-gateway/src/ai_serving/mod.rs");
    assert!(
        gateway_root.contains("AiExecutionDecision")
            && gateway_root.contains("AiExecutionPlanPayload")
            && gateway_root.contains("AiSyncAttempt")
            && gateway_root.contains("AiStreamAttempt"),
        "gateway ai_serving root should expose serving-owned DTO names"
    );

    let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root should resolve");
    let legacy_names = [
        ["GatewayControl", "SyncDecisionResponse"].concat(),
        ["GatewayControl", "PlanResponse"].concat(),
        ["LocalSync", "PlanAndReport"].concat(),
        ["LocalStream", "PlanAndReport"].concat(),
    ];
    let mut violations = Vec::new();
    for root in [
        "apps/aether-gateway/src/ai_serving",
        "apps/aether-gateway/src/executor",
        "apps/aether-gateway/src/execution_runtime",
        "crates/aether-ai-serving/src",
        "crates/aether-ai-formats/src",
    ] {
        for file in collect_workspace_rust_files(root) {
            let relative = file
                .canonicalize()
                .expect("workspace file should canonicalize")
                .strip_prefix(&workspace_root)
                .expect("workspace file should be under workspace root")
                .to_string_lossy()
                .replace('\\', "/");
            let source = std::fs::read_to_string(&file).expect("source file should be readable");
            let hits = legacy_names
                .iter()
                .filter_map(|pattern| {
                    source
                        .contains(pattern.as_str())
                        .then_some(pattern.as_str())
                })
                .collect::<Vec<_>>();
            if !hits.is_empty() {
                violations.push(format!("{relative} -> {}", hits.join(", ")));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "AI serving code should not retain legacy GatewayControl*/Local* DTO names:\n{}",
        violations.join("\n")
    );
}

#[test]
fn ai_format_crate_stays_free_of_gateway_runtime_deps() {
    for manifest_path in ["crates/aether-ai-formats/Cargo.toml"] {
        let manifest = read_workspace_file(manifest_path);
        for forbidden in [
            "axum",
            "sqlx",
            "redis",
            "aether-gateway",
            "aether-usage-runtime",
            "aether-provider-transport",
        ] {
            assert!(
                !manifest.contains(forbidden),
                "{manifest_path} should not depend on gateway/runtime adapter dependency {forbidden}"
            );
        }
    }

    let mut violations = Vec::new();
    for root in ["crates/aether-ai-formats/src"] {
        for file in collect_workspace_rust_files(root) {
            let source = std::fs::read_to_string(&file).expect("source file should be readable");
            let hits = [
                "AppState",
                "axum::",
                "sqlx::",
                "redis::",
                "GatewaySyncReportRequest",
                "aether_usage_runtime",
                "aether_provider_transport",
            ]
            .iter()
            .filter(|pattern| source.contains(**pattern))
            .copied()
            .collect::<Vec<_>>();
            if !hits.is_empty() {
                violations.push(format!("{} -> {}", file.display(), hits.join(", ")));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "format crate should stay pure of gateway runtime dependencies:\n{}",
        violations.join("\n")
    );
}

#[test]
fn aether_runtime_stays_free_of_ai_serving_policy() {
    let runtime_manifest = read_workspace_file("crates/aether-runtime/Cargo.toml");
    for forbidden in [
        "aether-ai-serving",
        "aether-ai-formats",
        "aether-provider-transport",
        "aether-gateway",
    ] {
        assert!(
            !runtime_manifest.contains(forbidden),
            "aether-runtime should not depend on AI serving/pure/gateway crate {forbidden}"
        );
    }

    let mut violations = Vec::new();
    for file in collect_workspace_rust_files("crates/aether-runtime/src") {
        let source = std::fs::read_to_string(&file).expect("source file should be readable");
        let hits = [
            "aether_ai_serving",
            "aether_ai_formats",
            "aether_provider_transport",
            "AiExecution",
            "ExecutionPlan",
            "provider_api_format",
            "client_api_format",
            "OpenAI",
            "OpenAi",
            "Claude",
            "Gemini",
            "finalize",
            "request_candidate",
        ]
        .iter()
        .filter(|pattern| source.contains(**pattern))
        .copied()
        .collect::<Vec<_>>();
        if !hits.is_empty() {
            violations.push(format!("{} -> {}", file.display(), hits.join(", ")));
        }
    }

    assert!(
        violations.is_empty(),
        "aether-runtime should stay execution/runtime infrastructure only, without AI routing, candidate, provider, or finalize policy:\n{}",
        violations.join("\n")
    );
}

#[test]
fn ai_serving_crate_api_is_confined_to_root_seams() {
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
        if relative == "apps/aether-gateway/src/ai_serving/pure/mod.rs"
            || relative == "apps/aether-gateway/src/ai_serving/api.rs"
            || relative.starts_with("apps/aether-gateway/src/tests/")
        {
            continue;
        }

        let source = std::fs::read_to_string(&file).expect("source file should be readable");
        if source.contains("aether_ai_formats::api") {
            violations.push(relative);
        }
    }

    assert!(
        violations.is_empty(),
        "gateway code should only depend on aether_ai_formats::api through ai_serving/pure/mod.rs or ai_serving/api.rs:\n{}",
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
        if relative == "apps/aether-gateway/src/ai_serving/pure/mod.rs"
            || relative == "apps/aether-gateway/src/ai_serving/transport.rs"
            || relative == "apps/aether-gateway/src/ai_serving/api.rs"
            || relative.ends_with("/tests.rs")
            || relative.contains("/tests/")
            || relative.starts_with("apps/aether-gateway/src/tests/")
        {
            continue;
        }

        let source = std::fs::read_to_string(&file).expect("source file should be readable");
        if source.contains("aether_ai_formats::") {
            crate_violations.push(relative);
        }
    }

    assert!(
        crate_violations.is_empty(),
        "gateway code should only depend directly on aether_ai_formats through ai_serving root seams:\n{}",
        crate_violations.join("\n")
    );
}

#[test]
fn ai_serving_routes_control_and_execution_deps_through_facades() {
    let patterns = [
        "use crate::control::",
        "crate::control::",
        "use crate::headers::",
        "crate::headers::",
        "use crate::execution_runtime::",
        "crate::execution_runtime::",
    ];

    for root in ["src/ai_serving/planner", "src/ai_serving/finalize"] {
        assert_no_module_dependency_patterns(root, &patterns);
    }
    assert_no_module_dependency_patterns(
        "src/ai_serving",
        &[
            "crate::ai_serving::control_facade::",
            "use crate::ai_serving::control_facade::",
            "crate::ai_serving::execution_facade::",
            "use crate::ai_serving::execution_facade::",
            "crate::ai_serving::provider_transport_facade::",
            "use crate::ai_serving::provider_transport_facade::",
            "crate::ai_serving::planner::auth_snapshot_facade::",
            "use crate::ai_serving::planner::auth_snapshot_facade::",
            "crate::ai_serving::planner::scheduler_facade::",
            "use crate::ai_serving::planner::scheduler_facade::",
            "crate::ai_serving::planner::candidate_runtime_facade::",
            "use crate::ai_serving::planner::candidate_runtime_facade::",
            "crate::ai_serving::planner::transport_facade::",
            "use crate::ai_serving::planner::transport_facade::",
        ],
    );

    assert!(
        !workspace_file_exists("apps/aether-gateway/src/ai_serving/contracts"),
        "gateway ai_serving should not keep a contracts directory; contracts belong to aether-ai-formats and serving DTOs belong to aether-ai-serving"
    );

    let ai_serving_mod = read_workspace_file("apps/aether-gateway/src/ai_serving/mod.rs");
    assert!(
        !ai_serving_mod.contains("GatewayControlAuthContext {"),
        "ai_serving/mod.rs should not own GatewayControlAuthContext after execution auth DTO extraction"
    );
    for pattern in [
        "struct AiControlPlanRequest",
        "struct AiExecutionPlanPayload",
        "struct AiExecutionDecision",
    ] {
        assert!(
            !ai_serving_mod.contains(pattern),
            "ai_serving/mod.rs should not own {pattern} after DTO extraction"
        );
    }
    assert!(
        ai_serving_mod.contains("pub(crate) use aether_ai_serving::{"),
        "ai_serving/mod.rs should consume serving DTOs through aether-ai-serving"
    );
    assert!(
        ai_serving_mod.contains(
            "generic_decision_missing_exact_provider_request as generic_decision_missing_exact_provider_request_impl"
        ),
        "ai_serving/mod.rs should delegate exact-request detection through aether-ai-serving"
    );
    assert!(
        !ai_serving_mod.contains("AiControlPlanRequest {"),
        "ai_serving/mod.rs should not locally construct AiControlPlanRequest after helper extraction"
    );
    assert!(
        !ai_serving_mod.contains("pub(crate) async fn build_gateway_plan_request("),
        "ai_serving/mod.rs should not keep dead plan-request bridge after helper extraction"
    );

    let gateway_plan_builders =
        read_workspace_file("apps/aether-gateway/src/ai_serving/planner/plan_builders.rs");
    for pattern in ["struct AiSyncAttempt", "struct AiStreamAttempt"] {
        assert!(
            !gateway_plan_builders.contains(pattern),
            "planner/plan_builders.rs should not own {pattern} after plan DTO extraction"
        );
    }
    assert!(
        gateway_plan_builders.contains("crate::ai_serving::"),
        "planner/plan_builders.rs should consume serving plan DTOs through the ai_serving root seam after extraction"
    );
    assert!(
        gateway_plan_builders.contains(
            "use crate::ai_serving::augment_sync_report_context as augment_sync_report_context_impl;"
        ),
        "planner/plan_builders.rs should delegate report-context augmentation through the ai_serving root seam"
    );
    for pattern in [
        "pub(super) fn take_non_empty_string(",
        "fn resolve_passthrough_sync_request_body(",
        "fn trim_owned_non_empty_string(",
    ] {
        assert!(
            !gateway_plan_builders.contains(pattern),
            "planner/plan_builders.rs should not own pure decision payload helpers after serving extraction, found {pattern}"
        );
    }

    let serving_attempt_plan = read_workspace_file("crates/aether-ai-serving/src/attempt_plan.rs");
    for pattern in [
        "pub fn take_ai_decision_plan_core(",
        "pub fn take_ai_upstream_auth_pair(",
        "pub fn resolve_ai_passthrough_sync_request_body(",
        "pub fn build_ai_execution_plan_from_decision(",
    ] {
        assert!(
            serving_attempt_plan.contains(pattern),
            "aether-ai-serving should own pure attempt-plan helper {pattern}"
        );
    }

    let gateway_passthrough_plan_builders = read_workspace_file(
        "apps/aether-gateway/src/ai_serving/planner/passthrough/plan_builders.rs",
    );
    for pattern in [
        "fn resolve_passthrough_sync_request_body(",
        "fn trim_owned_non_empty_string(",
    ] {
        assert!(
            !gateway_passthrough_plan_builders.contains(pattern),
            "passthrough plan builders should delegate pure body helpers to aether-ai-serving, found {pattern}"
        );
    }

    for path in [
        "apps/aether-gateway/src/ai_serving/planner/passthrough/plan_builders.rs",
        "apps/aether-gateway/src/ai_serving/planner/standard/plan_builders.rs",
        "apps/aether-gateway/src/ai_serving/planner/standard/gemini/plan_builders.rs",
        "apps/aether-gateway/src/ai_serving/planner/standard/openai/plan_builders/sync.rs",
        "apps/aether-gateway/src/ai_serving/planner/standard/openai/plan_builders/stream.rs",
    ] {
        let contents = read_workspace_file(path);
        assert!(
            !contents.contains("let plan = ExecutionPlan {"),
            "{path} should delegate common ExecutionPlan assembly to aether-ai-serving"
        );
        assert!(
            contents.contains("build_ai_execution_plan_from_decision"),
            "{path} should use the serving-owned execution plan builder"
        );
    }

    let gateway_finalize_common =
        read_workspace_file("apps/aether-gateway/src/ai_serving/finalize/common.rs");
    assert!(
        gateway_finalize_common
            .contains("prepare_local_success_response_parts as prepare_local_success_response_parts_impl"),
        "finalize/common.rs should delegate success response-part normalization to the format crate"
    );
    assert!(
        gateway_finalize_common
            .contains("build_local_success_background_report as build_local_success_background_report_impl"),
        "finalize/common.rs should delegate pure success background-report construction to the format crate"
    );
    assert!(
        gateway_finalize_common
            .contains("build_local_success_conversion_background_report as build_local_success_conversion_background_report_impl"),
        "finalize/common.rs should delegate pure conversion success background-report construction to the format crate"
    );
    assert!(
        gateway_finalize_common.contains("surface_report_parts_from_gateway(")
            && gateway_finalize_common.contains("gateway_report_from_surface("),
        "finalize/common.rs should own usage-runtime DTO mapping around pure surface report parts"
    );

    let ai_serving_mod = read_workspace_file("apps/aether-gateway/src/ai_serving/mod.rs");
    for pattern in [
        "crate::control::resolve_execution_runtime_auth_context",
        "crate::headers::collect_control_headers",
        "crate::headers::is_json_request",
    ] {
        assert!(
            ai_serving_mod.contains(pattern),
            "ai_serving/mod.rs should own {pattern}"
        );
    }
    assert!(
        ai_serving_mod.contains("ExecutionRuntimeAuthContext"),
        "ai_serving/mod.rs should own ExecutionRuntimeAuthContext projection"
    );

    assert!(
        ai_serving_mod
            .contains("crate::execution_runtime::maybe_build_local_sync_finalize_response"),
        "ai_serving/mod.rs should own local sync finalize response bridging"
    );
    assert!(
        !workspace_file_exists("apps/aether-gateway/src/ai_serving/control.rs"),
        "ai_serving/control.rs should stay removed after root seam consolidation"
    );
    assert!(
        !workspace_file_exists("apps/aether-gateway/src/ai_serving/execution.rs"),
        "ai_serving/execution.rs should stay removed after root seam consolidation"
    );

    assert!(
        !ai_serving_mod.contains("pub(crate) use aether_ai_formats::api::*;"),
        "ai_serving/mod.rs should not keep wildcard surface-crate exports after root-seam freeze"
    );
    for export in [
        "PlannerAppState",
        "GatewayAuthApiKeySnapshot",
        "GatewayProviderTransportSnapshot",
        "LocalResolvedOAuthRequestAuth",
    ] {
        assert!(
            ai_serving_mod.contains(export),
            "ai_serving/mod.rs should re-export {export} from the planner root seam"
        );
    }

    assert!(
        !workspace_file_exists("apps/aether-gateway/src/ai_serving/pure.rs"),
        "ai_serving/pure.rs should stay removed after pure seam directoryization"
    );
    assert!(
        workspace_file_exists("apps/aether-gateway/src/ai_serving/pure/mod.rs"),
        "ai_serving/pure/mod.rs should exist after pure seam directoryization"
    );
    for path in [
        "apps/aether-gateway/src/ai_serving/pure/adaptation.rs",
        "apps/aether-gateway/src/ai_serving/pure/contracts.rs",
        "apps/aether-gateway/src/ai_serving/pure/conversion.rs",
        "apps/aether-gateway/src/ai_serving/pure/finalize.rs",
        "apps/aether-gateway/src/ai_serving/pure/planner.rs",
    ] {
        assert!(
            !workspace_file_exists(path),
            "{path} should stay removed after pure seam collapse"
        );
    }

    let pure_mod = read_workspace_file("apps/aether-gateway/src/ai_serving/pure/mod.rs");
    for pattern in [
        "pub(crate) use aether_ai_formats::api::{",
        "ExecutionRuntimeAuthContext",
        "ProviderAdaptationDescriptor",
        "RequestConversionKind",
        "AiSurfaceFinalizeError",
        "LocalStandardSpec",
    ] {
        assert!(
            pure_mod.contains(pattern),
            "ai_serving/pure/mod.rs should own {pattern}"
        );
    }
}

#[test]
fn ai_serving_routes_provider_transport_deps_through_facade() {
    let patterns = [
        "use crate::provider_transport::",
        "crate::provider_transport::",
    ];

    assert_no_module_dependency_patterns("src/ai_serving/planner", &patterns);
    let mut direct_transport_violations = Vec::new();
    for root in ["apps/aether-gateway/src/ai_serving/planner"] {
        for file in collect_workspace_rust_files(root) {
            let path = file.to_string_lossy().replace('\\', "/");
            if path.ends_with("/tests.rs") || path.contains("/tests/") {
                continue;
            }
            let source = std::fs::read_to_string(&file).expect("source file should be readable");
            let runtime_source = source
                .split("#[cfg(test)]")
                .next()
                .unwrap_or(source.as_str());
            if runtime_source.contains("aether_provider_transport::") {
                direct_transport_violations.push(path);
            }
        }
    }
    assert!(
        direct_transport_violations.is_empty(),
        "gateway ai_serving runtime code should route provider transport through ai_serving/transport.rs:\n{}",
        direct_transport_violations.join("\n")
    );
    assert!(
        !workspace_file_exists("apps/aether-gateway/src/ai_serving/runtime"),
        "ai_serving/runtime should stay removed after facade cleanup"
    );
    assert!(
        !workspace_file_exists("crates/aether-ai-formats/src/transport.rs"),
        "aether-ai-formats should not expose a provider transport bridge"
    );

    let provider_transport_facade =
        read_workspace_file("apps/aether-gateway/src/ai_serving/transport.rs");
    for pattern in [
        "aether_provider_transport::auth",
        "aether_provider_transport::url",
        "aether_provider_transport::policy",
        "aether_provider_transport::snapshot",
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
        "aether_ai_formats::transport",
    ] {
        assert!(
            !provider_transport_facade.contains(forbidden),
            "transport.rs should not keep gateway-local provider_transport owner {forbidden}"
        );
    }

    let ai_serving_mod = read_workspace_file("apps/aether-gateway/src/ai_serving/mod.rs");
    assert!(
        ai_serving_mod.contains("pub(crate) mod transport;"),
        "ai_serving/mod.rs should expose provider transport capabilities through the root seam module"
    );
    assert!(
        ai_serving_mod.contains("self::transport::build_transport_request_url("),
        "ai_serving/mod.rs should route transport URL construction through ai_serving/transport.rs"
    );
    assert!(
        !ai_serving_mod.contains("crate::provider_transport::"),
        "ai_serving/mod.rs should not bypass the provider transport root seam"
    );
}

#[test]
fn ai_serving_planner_gateway_state_seam_is_split_by_role() {
    assert!(
        !workspace_file_exists("apps/aether-gateway/src/ai_serving/planner/gateway_facade.rs"),
        "planner/gateway_facade.rs should be removed after seam split"
    );

    for path in [
        "apps/aether-gateway/src/ai_serving/planner/auth_snapshot_facade.rs",
        "apps/aether-gateway/src/ai_serving/planner/transport_facade.rs",
        "apps/aether-gateway/src/ai_serving/planner/scheduler_facade.rs",
        "apps/aether-gateway/src/ai_serving/planner/candidate_runtime_facade.rs",
        "apps/aether-gateway/src/ai_serving/planner/executor_facade.rs",
    ] {
        assert!(
            !workspace_file_exists(path),
            "{path} should be removed after PlannerAppState absorbed the seam"
        );
    }

    for path in [
        "apps/aether-gateway/src/ai_serving/planner/state/mod.rs",
        "apps/aether-gateway/src/ai_serving/planner/state/auth.rs",
        "apps/aether-gateway/src/ai_serving/planner/state/transport.rs",
        "apps/aether-gateway/src/ai_serving/planner/state/scheduler.rs",
        "apps/aether-gateway/src/ai_serving/planner/state/candidate_runtime.rs",
        "apps/aether-gateway/src/ai_serving/planner/state/executor.rs",
    ] {
        assert!(
            workspace_file_exists(path),
            "{path} should exist after PlannerAppState directoryization"
        );
    }

    let state_mod = read_workspace_file("apps/aether-gateway/src/ai_serving/planner/state/mod.rs");
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
        read_workspace_file("apps/aether-gateway/src/ai_serving/planner/state/auth.rs");
    assert!(
        state_auth.contains("read_auth_api_key_snapshot("),
        "planner/state/auth.rs should own auth snapshot reads"
    );

    let state_transport =
        read_workspace_file("apps/aether-gateway/src/ai_serving/planner/state/transport.rs");
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
        read_workspace_file("apps/aether-gateway/src/ai_serving/planner/state/scheduler.rs");
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
        "apps/aether-gateway/src/ai_serving/planner/state/candidate_runtime.rs",
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
        read_workspace_file("apps/aether-gateway/src/ai_serving/planner/state/executor.rs");
    assert!(
        state_executor.contains("mark_unused_local_candidate_items("),
        "planner/state/executor.rs should own mark_unused_local_candidate_items"
    );
}

#[test]
fn ai_serving_planner_separates_local_candidate_resolution_from_ranking() {
    let planner_mod = read_workspace_file("apps/aether-gateway/src/ai_serving/planner/mod.rs");
    for pattern in [
        "mod candidate_affinity_cache;",
        "mod candidate_ranking;",
        "mod candidate_resolution;",
        "mod candidate_preparation;",
        "mod candidate_transport_ranking_facts;",
        "mod pool_scheduler;",
    ] {
        assert!(
            planner_mod.contains(pattern),
            "planner/mod.rs should wire {pattern}"
        );
    }

    let candidate_resolution =
        read_workspace_file("apps/aether-gateway/src/ai_serving/planner/candidate_resolution.rs");
    let ranking_call = candidate_resolution
        .find("rank_eligible_local_execution_candidates(")
        .expect("candidate_resolution.rs should call core-backed local candidate ranking");
    let pool_scheduler_call = candidate_resolution
        .find("apply_local_execution_pool_scheduler(")
        .expect("candidate_resolution.rs should call pool scheduler after ranking");
    assert!(
        ranking_call < pool_scheduler_call,
        "candidate_resolution.rs should rank eligible candidates before applying pool-internal account scheduling"
    );
    for pattern in [
        "pub(crate) async fn resolve_and_rank_local_execution_candidates(",
        "pub(crate) async fn resolve_and_rank_local_execution_candidates_without_transport_pair_gate(",
        "pub(crate) async fn read_candidate_transport_snapshot(",
    ] {
        assert!(
            candidate_resolution.contains(pattern),
            "planner/candidate_resolution.rs should own {pattern}"
        );
    }

    assert!(
        !planner_mod.contains("mod candidate_eligibility;"),
        "planner should not wire the removed candidate eligibility compatibility shim"
    );

    let candidate_ranking =
        read_workspace_file("apps/aether-gateway/src/ai_serving/planner/candidate_ranking.rs");
    assert!(
        candidate_ranking.contains("async fn rank_local_execution_candidates("),
        "planner/candidate_ranking.rs should keep raw local ranking helper inside tests"
    );
    assert!(
        !candidate_ranking.contains("#[cfg(test)]\nasync fn rank_local_execution_candidates("),
        "planner/candidate_ranking.rs should not keep the test-only ranking helper at module root"
    );
    for forbidden in [
        "struct SkippedLocalExecutionCandidate",
        "async fn current_local_execution_candidate_skip_reason(",
        "pub(crate) async fn resolve_and_rank_local_execution_candidates(",
        "resolve_transport_proxy_snapshot_with_tunnel_affinity",
    ] {
        assert!(
            !candidate_ranking.contains(forbidden),
            "planner/candidate_ranking.rs should not own local candidate resolution or transport ranking facts helper {forbidden}"
        );
    }

    let candidate_affinity_cache = read_workspace_file(
        "apps/aether-gateway/src/ai_serving/planner/candidate_affinity_cache.rs",
    );
    for pattern in [
        "pub(crate) fn read_cached_scheduler_affinity_target(",
        "pub(crate) fn remember_scheduler_affinity_for_candidate(",
    ] {
        assert!(
            candidate_affinity_cache.contains(pattern),
            "planner/candidate_affinity_cache.rs should own {pattern}"
        );
    }
    for forbidden in [
        "apply_scheduler_candidate_ranking",
        "rank_eligible_local_execution_candidates",
        "resolve_and_rank_local_execution_candidates",
    ] {
        assert!(
            !candidate_affinity_cache.contains(forbidden),
            "planner/candidate_affinity_cache.rs should not own ranking or resolution helper {forbidden}"
        );
    }

    let candidate_transport_ranking_facts = read_workspace_file(
        "apps/aether-gateway/src/ai_serving/planner/candidate_transport_ranking_facts.rs",
    );
    for pattern in [
        "pub(super) struct CandidateTransportRankingFacts {",
        "resolve_cached_candidate_transport_ranking_facts",
        "resolve_cached_transport_ranking_facts",
        "resolve_transport_proxy_snapshot_with_tunnel_affinity",
    ] {
        assert!(
            candidate_transport_ranking_facts.contains(pattern),
            "planner/candidate_transport_ranking_facts.rs should own {pattern}"
        );
    }
    for forbidden in [
        "apply_scheduler_candidate_ranking",
        "rank_eligible_local_execution_candidates",
        "resolve_and_rank_local_execution_candidates",
    ] {
        assert!(
            !candidate_transport_ranking_facts.contains(forbidden),
            "planner/candidate_transport_ranking_facts.rs should not own ranking or resolution helper {forbidden}"
        );
    }

    let pool_scheduler =
        read_workspace_file("apps/aether-gateway/src/ai_serving/planner/pool_scheduler.rs");
    for pattern in [
        "pub(crate) async fn apply_local_execution_pool_scheduler(",
        "run_ai_pool_scheduler(",
        "fn ai_pool_candidate_facts(",
        "fn ai_pool_scheduling_config(",
        "fn ai_pool_runtime_state(",
    ] {
        assert!(
            pool_scheduler.contains(pattern),
            "planner/pool_scheduler.rs should adapt gateway pool runtime data through serving pool scheduler helper {pattern}"
        );
    }
    for forbidden in [
        "apply_scheduler_candidate_ranking",
        "SchedulerRankableCandidate",
        "rank_eligible_local_execution_candidates",
        "fn schedule_pool_group(",
        "fn pool_group_key(",
        "fn build_pool_sort_vectors",
        "fn plan_priority_score(",
        "fn stable_hash_score(",
    ] {
        assert!(
            !pool_scheduler.contains(forbidden),
            "planner/pool_scheduler.rs should not own global candidate ranking or pool scheduling policy helper {forbidden}"
        );
    }

    let serving_pool_scheduler =
        read_workspace_file("crates/aether-ai-serving/src/pool_scheduler.rs");
    for pattern in [
        "pub fn run_ai_pool_scheduler",
        "pub struct AiPoolCandidateInput",
        "pub struct AiPoolRuntimeState",
        "pub struct AiPoolSchedulingConfig",
        "fn schedule_pool_group",
        "fn build_pool_sort_vectors",
        "fn plan_priority_score(",
        "fn stable_hash_score(",
    ] {
        assert!(
            serving_pool_scheduler.contains(pattern),
            "aether-ai-serving pool_scheduler.rs should own pool scheduling use-case primitive {pattern}"
        );
    }
}

#[test]
fn ai_serving_candidate_preparation_owns_shared_auth_and_mapped_model_resolution() {
    let serving_candidate_preparation =
        read_workspace_file("crates/aether-ai-serving/src/candidate_preparation.rs");
    for pattern in [
        "pub struct AiPreparedHeaderAuthenticatedCandidate",
        "pub fn prepare_ai_header_authenticated_candidate(",
        "pub fn resolve_ai_candidate_mapped_model(",
        "transport_auth_unavailable",
        "mapped_model_missing",
    ] {
        assert!(
            serving_candidate_preparation.contains(pattern),
            "aether-ai-serving candidate_preparation.rs should own pure candidate preparation policy {pattern}"
        );
    }

    let serving_lib = read_workspace_file("crates/aether-ai-serving/src/lib.rs");
    for pattern in [
        "pub mod candidate_preparation;",
        "prepare_ai_header_authenticated_candidate",
        "resolve_ai_candidate_mapped_model",
        "AiPreparedHeaderAuthenticatedCandidate",
    ] {
        assert!(
            serving_lib.contains(pattern),
            "aether-ai-serving lib.rs should export candidate preparation contract {pattern}"
        );
    }

    let candidate_preparation =
        read_workspace_file("apps/aether-gateway/src/ai_serving/planner/candidate_preparation.rs");
    for pattern in [
        "pub(crate) type PreparedHeaderAuthenticatedCandidate = AiPreparedHeaderAuthenticatedCandidate;",
        "pub(crate) async fn prepare_header_authenticated_candidate(",
        "pub(crate) fn prepare_header_authenticated_candidate_from_auth(",
        "pub(crate) async fn resolve_candidate_oauth_auth(",
        "pub(crate) fn resolve_candidate_mapped_model(",
        "prepare_ai_header_authenticated_candidate(",
        "resolve_ai_candidate_mapped_model(",
    ] {
        assert!(
            candidate_preparation.contains(pattern),
            "planner/candidate_preparation.rs should expose gateway adapter/delegation point {pattern}"
        );
    }
    for forbidden in [
        "let mapped_model = candidate.selected_provider_model_name.trim().to_string()",
        "return Err(\"mapped_model_missing\")",
        "return Err(\"transport_auth_unavailable\")",
    ] {
        assert!(
            !candidate_preparation.contains(forbidden),
            "planner/candidate_preparation.rs should delegate pure candidate preparation policy instead of keeping {forbidden}"
        );
    }

    for path in [
        "apps/aether-gateway/src/ai_serving/planner/standard/openai/chat/decision/request.rs",
        "apps/aether-gateway/src/ai_serving/planner/standard/openai/responses/decision/request.rs",
        "apps/aether-gateway/src/ai_serving/planner/specialized/image/request.rs",
        "apps/aether-gateway/src/ai_serving/planner/standard/family/request.rs",
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
        assert!(
            !source.contains("PreparedHeaderAuthenticatedCandidate {"),
            "{path} should not manually assemble prepared header-auth candidates"
        );
    }

    for path in [
        "apps/aether-gateway/src/ai_serving/planner/specialized/video/request.rs",
        "apps/aether-gateway/src/ai_serving/planner/passthrough/provider/family/request/prepare.rs",
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
        "apps/aether-gateway/src/ai_serving/planner/passthrough/provider/family/request/prepare.rs",
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
fn ai_serving_candidate_materialization_owns_affinity_and_candidate_runtime_persistence() {
    let planner_mod = read_workspace_file("apps/aether-gateway/src/ai_serving/planner/mod.rs");
    assert!(
        planner_mod.contains("mod candidate_materialization;"),
        "planner/mod.rs should wire candidate_materialization helper module"
    );

    let serving_candidate_persistence =
        read_workspace_file("crates/aether-ai-serving/src/candidate_persistence.rs");
    for pattern in [
        "pub trait AiAvailableCandidatePersistencePort",
        "pub async fn run_ai_available_candidate_persistence",
        "pub fn ai_should_persist_available_candidate_for_pool_key",
        "pub fn ai_should_persist_skipped_candidate_for_pool_membership",
        "pub fn ai_candidate_extra_data_with_ranking",
        "attempt_slot_count",
        "should_persist_available_candidate",
        "persist_available_candidate",
        "build_attempt",
        "pub trait AiSkippedCandidatePersistencePort",
        "pub async fn run_ai_skipped_candidate_persistence",
        "should_persist_skipped_candidate",
        "persist_skipped_candidate",
    ] {
        assert!(
            serving_candidate_persistence.contains(pattern),
            "aether-ai-serving should own candidate persistence loop primitive {pattern}"
        );
    }

    let candidate_materialization = read_workspace_file(
        "apps/aether-gateway/src/ai_serving/planner/candidate_materialization.rs",
    );
    for pattern in [
        "pub(crate) struct LocalExecutionCandidateAttempt {",
        "pub(crate) struct LocalAvailableCandidatePersistenceContext<'a> {",
        "pub(crate) struct LocalSkippedCandidatePersistenceContext<'a> {",
        "pub(crate) fn remember_first_local_candidate_affinity(",
        "impl<F> AiAvailableCandidatePersistencePort for GatewayAvailableCandidatePersistencePort",
        "impl AiSkippedCandidatePersistencePort for GatewaySkippedCandidatePersistencePort",
        "run_ai_available_candidate_persistence(&port",
        "run_ai_skipped_candidate_persistence(&port",
        "ai_should_persist_available_candidate_for_pool_key(",
        "ai_should_persist_skipped_candidate_for_pool_membership(",
        "ai_candidate_extra_data_with_ranking(",
        "persist_available_local_execution_candidates",
        "persist_available_local_execution_candidates_with_context",
        "pub(crate) async fn persist_skipped_local_execution_candidate(",
        "pub(crate) async fn mark_skipped_local_execution_candidate(",
        "pub(crate) async fn persist_skipped_local_execution_candidates(",
        "pub(crate) async fn persist_skipped_local_execution_candidates_with_context(",
    ] {
        assert!(
            candidate_materialization.contains(pattern),
            "planner/candidate_materialization.rs should adapt candidate materialization and persistence through {pattern}"
        );
    }
    for forbidden in [
        "for (candidate_index, eligible) in candidates.into_iter().enumerate()",
        "for skipped_candidate in skipped_candidates",
        "fn local_candidate_extra_data_with_ranking(",
        "append_ranking_metadata_to_object(",
    ] {
        assert!(
            !candidate_materialization.contains(forbidden),
            "planner/candidate_materialization.rs should not hand-roll serving candidate persistence loop {forbidden}"
        );
    }

    for path in [
        "apps/aether-gateway/src/ai_serving/planner/standard/family/candidates.rs",
        "apps/aether-gateway/src/ai_serving/planner/passthrough/provider/family/candidates.rs",
        "apps/aether-gateway/src/ai_serving/planner/standard/openai/chat/decision/support.rs",
        "apps/aether-gateway/src/ai_serving/planner/standard/openai/responses/decision/support.rs",
        "apps/aether-gateway/src/ai_serving/planner/specialized/video/support.rs",
        "apps/aether-gateway/src/ai_serving/planner/specialized/files/support.rs",
    ] {
        let source = read_workspace_file(path);
        assert!(
            source.contains("materialize_local_execution_candidates_with_serving("),
            "{path} should route candidate materialization through the serving port adapter"
        );
        for forbidden in [
            "remember_scheduler_affinity_for_candidate(",
            "persist_available_local_candidate(",
            "persist_skipped_local_candidate(",
            "persist_available_local_execution_candidates_with_context(",
            "persist_skipped_local_execution_candidates_with_context(",
            "remember_first_local_candidate_affinity(",
        ] {
            assert!(
                !source.contains(forbidden),
                "{path} should not inline candidate materialization step {forbidden}"
            );
        }
    }

    for path in [
        "apps/aether-gateway/src/ai_serving/planner/standard/family/payload.rs",
        "apps/aether-gateway/src/ai_serving/planner/passthrough/provider/family/payload.rs",
        "apps/aether-gateway/src/ai_serving/planner/standard/openai/chat/decision/support.rs",
        "apps/aether-gateway/src/ai_serving/planner/standard/openai/responses/decision/support.rs",
        "apps/aether-gateway/src/ai_serving/planner/specialized/video/support.rs",
        "apps/aether-gateway/src/ai_serving/planner/specialized/files/support.rs",
    ] {
        let source = read_workspace_file(path);
        assert!(
            source.contains("mark_skipped_local_execution_candidate("),
            "{path} should route skipped candidate persistence through shared materialization helper"
        );
    }

    for (path, pattern) in [
        (
            "apps/aether-gateway/src/ai_serving/planner/standard/family/mod.rs",
            "LocalExecutionCandidateAttempt as LocalStandardCandidateAttempt",
        ),
        (
            "apps/aether-gateway/src/ai_serving/planner/passthrough/provider/family/mod.rs",
            "LocalExecutionCandidateAttempt as LocalSameFormatProviderCandidateAttempt",
        ),
        (
            "apps/aether-gateway/src/ai_serving/planner/standard/openai/chat/decision/support.rs",
            "LocalExecutionCandidateAttempt as LocalOpenAiChatCandidateAttempt",
        ),
        (
            "apps/aether-gateway/src/ai_serving/planner/standard/openai/responses/decision/support.rs",
            "LocalExecutionCandidateAttempt as LocalOpenAiResponsesCandidateAttempt",
        ),
        (
            "apps/aether-gateway/src/ai_serving/planner/specialized/video/support.rs",
            "LocalExecutionCandidateAttempt as LocalVideoCreateCandidateAttempt",
        ),
        (
            "apps/aether-gateway/src/ai_serving/planner/specialized/files/support.rs",
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
fn ai_serving_materialization_policy_owns_local_candidate_persistence_modes() {
    let planner_mod = read_workspace_file("apps/aether-gateway/src/ai_serving/planner/mod.rs");
    assert!(
        planner_mod.contains("mod materialization_policy;"),
        "planner/mod.rs should wire materialization_policy helper module"
    );

    let serving_candidate_persistence_policy =
        read_workspace_file("crates/aether-ai-serving/src/candidate_persistence_policy.rs");
    for pattern in [
        "pub enum AiCandidatePersistencePolicyKind {",
        "pub struct AiCandidatePersistencePolicySpec {",
        "pub fn ai_candidate_persistence_policy_spec(",
        "AiCandidatePersistencePolicyKind::StandardDecision",
        "AiCandidatePersistencePolicyKind::SameFormatProviderDecision",
        "AiCandidatePersistencePolicyKind::OpenAiChatDecision",
        "AiCandidatePersistencePolicyKind::OpenAiResponsesDecision",
        "AiCandidatePersistencePolicyKind::GeminiFilesDecision",
        "AiCandidatePersistencePolicyKind::VideoDecision",
    ] {
        assert!(
            serving_candidate_persistence_policy.contains(pattern),
            "aether-ai-serving should own candidate persistence policy primitive {pattern}"
        );
    }

    let materialization_policy =
        read_workspace_file("apps/aether-gateway/src/ai_serving/planner/materialization_policy.rs");
    for pattern in [
        "AiCandidatePersistencePolicyKind as LocalCandidatePersistencePolicyKind",
        "pub(crate) struct LocalCandidatePersistencePolicy<'a> {",
        "pub(crate) fn build_local_candidate_persistence_policy<'a>(",
        "ai_candidate_persistence_policy_spec(kind)",
    ] {
        assert!(
            materialization_policy.contains(pattern),
            "planner/materialization_policy.rs should map serving persistence policy into gateway contexts through {pattern}"
        );
    }

    for path in [
        "apps/aether-gateway/src/ai_serving/planner/standard/family/candidates.rs",
        "apps/aether-gateway/src/ai_serving/planner/passthrough/provider/family/candidates.rs",
        "apps/aether-gateway/src/ai_serving/planner/standard/openai/chat/decision/support.rs",
        "apps/aether-gateway/src/ai_serving/planner/standard/openai/responses/decision/support.rs",
        "apps/aether-gateway/src/ai_serving/planner/specialized/video/support.rs",
        "apps/aether-gateway/src/ai_serving/planner/specialized/files/support.rs",
        "apps/aether-gateway/src/ai_serving/planner/standard/family/payload.rs",
        "apps/aether-gateway/src/ai_serving/planner/passthrough/provider/family/payload.rs",
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
fn ai_serving_candidate_metadata_owns_local_execution_candidate_extra_data_shape() {
    let planner_mod = read_workspace_file("apps/aether-gateway/src/ai_serving/planner/mod.rs");
    assert!(
        planner_mod.contains("mod candidate_metadata;"),
        "planner/mod.rs should wire candidate_metadata helper module"
    );

    let candidate_metadata =
        read_workspace_file("apps/aether-gateway/src/ai_serving/planner/candidate_metadata.rs");
    let serving_ranking_metadata =
        read_workspace_file("crates/aether-ai-serving/src/ranking_metadata.rs");
    let serving_candidate_metadata =
        read_workspace_file("crates/aether-ai-serving/src/candidate_metadata.rs");
    for pattern in [
        "pub struct AiCandidateMetadataParts<'a> {",
        "pub fn build_ai_candidate_metadata(",
        "pub fn build_ai_candidate_metadata_from_candidate(",
        "pub fn append_ai_execution_contract_fields_to_value(",
        "pub fn ai_local_execution_contract_for_formats(",
        "\"provider_api_format\"",
        "\"global_model_id\"",
        "\"selected_provider_model_name\"",
        "\"provider_contract\"",
    ] {
        assert!(
            serving_candidate_metadata.contains(pattern),
            "aether-ai-serving should own base candidate metadata shape {pattern}"
        );
    }
    for pattern in [
        "pub fn append_ai_ranking_metadata_to_object(",
        "\"ranking_mode\"",
        "\"priority_mode\"",
        "\"ranking_index\"",
        "\"priority_slot\"",
        "\"promoted_by\"",
        "\"demoted_by\"",
    ] {
        assert!(
            serving_ranking_metadata.contains(pattern),
            "aether-ai-serving should own scheduler ranking metadata field helper {pattern}"
        );
    }
    for pattern in [
        "pub(crate) struct LocalExecutionCandidateMetadataParts<'a> {",
        "pub(crate) fn build_local_execution_candidate_metadata(",
        "pub(crate) fn build_local_execution_candidate_contract_metadata(",
        "append_ai_ranking_metadata_to_object(object, ranking)",
        "build_ai_candidate_metadata_from_candidate(",
        "append_ai_execution_contract_fields_to_value(",
    ] {
        assert!(
            candidate_metadata.contains(pattern),
            "planner/candidate_metadata.rs should adapt candidate metadata through {pattern}"
        );
    }

    for path in [
        "apps/aether-gateway/src/ai_serving/planner/standard/family/candidates.rs",
        "apps/aether-gateway/src/ai_serving/planner/standard/openai/chat/decision/support.rs",
        "apps/aether-gateway/src/ai_serving/planner/standard/openai/responses/decision/support.rs",
    ] {
        let source = read_workspace_file(path);
        assert!(
            source.contains("ai_local_execution_contract_for_formats("),
            "{path} should delegate local execution strategy/conversion mode policy to aether-ai-serving"
        );
    }
    for forbidden in [
        "\"global_model_id\".to_string()",
        "\"selected_provider_model_name\".to_string()",
        "\"provider_contract\".to_string()",
    ] {
        assert!(
            !candidate_metadata.contains(forbidden),
            "planner/candidate_metadata.rs should not own base candidate metadata field shape {forbidden}"
        );
    }

    for path in [
        "apps/aether-gateway/src/ai_serving/planner/standard/family/candidates.rs",
        "apps/aether-gateway/src/ai_serving/planner/standard/openai/chat/decision/support.rs",
        "apps/aether-gateway/src/ai_serving/planner/standard/openai/responses/decision/support.rs",
        "apps/aether-gateway/src/ai_serving/planner/passthrough/provider/family/candidates.rs",
        "apps/aether-gateway/src/ai_serving/planner/specialized/files/support.rs",
        "apps/aether-gateway/src/ai_serving/planner/specialized/video/support.rs",
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
fn ai_serving_runtime_miss_owns_local_execution_miss_state_machine() {
    let planner_mod = read_workspace_file("apps/aether-gateway/src/ai_serving/planner/mod.rs");
    assert!(
        planner_mod.contains("mod runtime_miss;"),
        "planner/mod.rs should wire runtime_miss helper module"
    );

    let serving_runtime_miss = read_workspace_file("crates/aether-ai-serving/src/runtime_miss.rs");
    for pattern in [
        "pub trait AiRuntimeMissDiagnosticPort",
        "pub trait AiRuntimeMissDiagnosticFields",
        "pub fn set_ai_runtime_miss_diagnostic_reason",
        "pub fn build_ai_runtime_execution_exhausted_diagnostic",
        "pub fn set_ai_runtime_execution_exhausted_diagnostic",
        "pub fn build_ai_runtime_candidate_evaluation_diagnostic",
        "pub fn set_ai_runtime_candidate_evaluation_diagnostic",
        "pub fn apply_ai_runtime_candidate_evaluation_progress",
        "pub fn apply_ai_runtime_candidate_evaluation_progress_preserving_candidate_signal",
        "pub fn apply_ai_runtime_candidate_terminal_reason",
        "pub fn record_ai_runtime_candidate_skip_reason",
        "pub fn apply_ai_runtime_candidate_evaluation_progress_to_diagnostic",
        "pub fn apply_ai_runtime_candidate_terminal_plan_reason_to_diagnostic",
        "pub fn record_ai_runtime_candidate_skip_reason_on_diagnostic",
    ] {
        assert!(
            serving_runtime_miss.contains(pattern),
            "aether-ai-serving should own runtime miss diagnostic state-machine primitive {pattern}"
        );
    }

    let runtime_miss =
        read_workspace_file("apps/aether-gateway/src/ai_serving/planner/runtime_miss.rs");
    for pattern in [
        "impl AiRuntimeMissDiagnosticFields for LocalExecutionRuntimeMissDiagnostic",
        "impl AiRuntimeMissDiagnosticPort for GatewayRuntimeMissDiagnosticPort",
        "pub(crate) fn set_local_runtime_miss_diagnostic_reason(",
        "pub(crate) fn build_local_runtime_execution_exhausted_diagnostic(",
        "pub(crate) fn set_local_runtime_execution_exhausted_diagnostic(",
        "pub(crate) fn build_local_runtime_candidate_evaluation_diagnostic(",
        "pub(crate) fn set_local_runtime_candidate_evaluation_diagnostic(",
        "pub(crate) fn apply_local_runtime_candidate_evaluation_progress(",
        "pub(crate) fn apply_local_runtime_candidate_evaluation_progress_preserving_candidate_signal(",
        "pub(crate) fn apply_local_runtime_candidate_terminal_reason(",
        "pub(crate) fn record_local_runtime_candidate_skip_reason(",
        "set_ai_runtime_miss_diagnostic_reason(",
        "build_ai_runtime_execution_exhausted_diagnostic(",
        "apply_ai_runtime_candidate_evaluation_progress_to_diagnostic(",
        "apply_ai_runtime_candidate_terminal_plan_reason_to_diagnostic(",
        "record_ai_runtime_candidate_skip_reason_on_diagnostic(",
        "apply_ai_runtime_candidate_evaluation_progress_preserving_candidate_signal(",
        "record_ai_runtime_candidate_skip_reason(",
    ] {
        assert!(
            runtime_miss.contains(pattern),
            "planner/runtime_miss.rs should adapt gateway runtime miss state through {pattern}"
        );
    }

    for path in [
        "apps/aether-gateway/src/ai_serving/planner/standard/family/build.rs",
        "apps/aether-gateway/src/ai_serving/planner/passthrough/provider/plans.rs",
        "apps/aether-gateway/src/ai_serving/planner/passthrough/provider/family/build.rs",
        "apps/aether-gateway/src/ai_serving/planner/candidate_materialization.rs",
        "apps/aether-gateway/src/ai_serving/planner/standard/openai/chat/mod.rs",
        "apps/aether-gateway/src/ai_serving/planner/standard/openai/chat/plans/diagnostic.rs",
        "apps/aether-gateway/src/ai_serving/planner/standard/openai/chat/plans/sync.rs",
        "apps/aether-gateway/src/ai_serving/planner/standard/openai/chat/plans/stream.rs",
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
        "apps/aether-gateway/src/ai_serving/planner/standard/family/build.rs",
        "apps/aether-gateway/src/ai_serving/planner/passthrough/provider/plans.rs",
        "apps/aether-gateway/src/ai_serving/planner/passthrough/provider/family/build.rs",
        "apps/aether-gateway/src/ai_serving/planner/candidate_materialization.rs",
        "apps/aether-gateway/src/ai_serving/planner/standard/openai/chat/mod.rs",
        "apps/aether-gateway/src/ai_serving/planner/standard/openai/chat/plans/diagnostic.rs",
        "apps/aether-gateway/src/ai_serving/planner/standard/openai/chat/plans/sync.rs",
        "apps/aether-gateway/src/ai_serving/planner/standard/openai/chat/plans/stream.rs",
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
fn ai_serving_standard_family_routes_request_preparation_through_request_payload_seams() {
    let standard_family_mod =
        read_workspace_file("apps/aether-gateway/src/ai_serving/planner/standard/family/mod.rs");
    assert!(
        standard_family_mod.contains("mod request;"),
        "standard family mod.rs should wire request seam"
    );

    let standard_family_request = read_workspace_file(
        "apps/aether-gateway/src/ai_serving/planner/standard/family/request.rs",
    );
    for pattern in [
        "pub(crate) struct LocalStandardCandidatePayloadParts {",
        "pub(crate) async fn resolve_local_standard_candidate_payload_parts(",
        "is_kiro_claude_messages_transport(",
        "build_kiro_cross_format_upstream_url(",
        "build_standard_provider_request_headers(",
    ] {
        assert!(
            standard_family_request.contains(pattern),
            "standard family request.rs should own {pattern}"
        );
    }
    assert!(
        !standard_family_request.contains(".eq_ignore_ascii_case(\"kiro\")"),
        "standard family request.rs should route Kiro provider checks through provider-transport"
    );
    for forbidden in [
        "build_claude_passthrough_headers(",
        "build_openai_passthrough_headers(",
        "build_complete_passthrough_headers_with_auth(",
        "apply_local_header_rules(",
        "ensure_upstream_auth_header(",
        "uses_vertex_api_key_query_auth(",
        "build_provider_transport_request_url(",
    ] {
        assert!(
            !standard_family_request.contains(forbidden),
            "standard family request.rs should not own standard provider header policy {forbidden}"
        );
    }

    let standard_family_payload = read_workspace_file(
        "apps/aether-gateway/src/ai_serving/planner/standard/family/payload.rs",
    );
    assert!(
        standard_family_payload.contains("resolve_local_standard_candidate_payload_parts("),
        "standard family payload.rs should consume request.rs preparation output"
    );
}

#[test]
fn ai_serving_same_format_provider_routes_request_preparation_through_request_payload_seams() {
    let same_format_provider_mod = read_workspace_file(
        "apps/aether-gateway/src/ai_serving/planner/passthrough/provider/family/mod.rs",
    );
    assert!(
        same_format_provider_mod.contains("mod request;"),
        "same-format provider mod.rs should wire request seam"
    );
    assert!(
        !workspace_file_exists(
            "apps/aether-gateway/src/ai_serving/planner/passthrough/provider/family/payload/prepare.rs"
        ),
        "same-format provider payload/prepare.rs should stay removed after request seam extraction"
    );

    let same_format_provider_request = read_workspace_file(
        "apps/aether-gateway/src/ai_serving/planner/passthrough/provider/family/request.rs",
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
        "apps/aether-gateway/src/ai_serving/planner/passthrough/provider/family/request/prepare.rs",
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
        "apps/aether-gateway/src/ai_serving/planner/passthrough/provider/family/payload.rs",
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
fn ai_serving_video_routes_request_preparation_through_request_payload_seams() {
    let specialized_video_mod =
        read_workspace_file("apps/aether-gateway/src/ai_serving/planner/specialized/video.rs");
    assert!(
        specialized_video_mod.contains("mod request;"),
        "specialized video mod.rs should wire request seam"
    );

    let specialized_video_request = read_workspace_file(
        "apps/aether-gateway/src/ai_serving/planner/specialized/video/request.rs",
    );
    for pattern in [
        "pub(super) struct LocalVideoCreateCandidatePayloadParts {",
        "pub(super) async fn resolve_local_video_create_candidate_payload_parts(",
        "build_video_create_request_body(",
        "build_video_create_upstream_url(",
        "build_video_create_headers(",
        "provider_video_create_family(",
    ] {
        assert!(
            specialized_video_request.contains(pattern),
            "specialized video request.rs should adapt through provider-transport via {pattern}"
        );
    }
    for forbidden in [
        "fn build_provider_request_body(",
        "fn build_video_upstream_url(",
        "apply_local_body_rules(",
        "apply_local_header_rules(",
        "build_passthrough_headers_with_auth(",
        "build_gemini_video_predict_long_running_url(",
    ] {
        assert!(
            !specialized_video_request.contains(forbidden),
            "specialized video request.rs should not own provider transport policy {forbidden}"
        );
    }

    let provider_transport_video =
        read_workspace_file("crates/aether-provider-transport/src/video.rs");
    for pattern in [
        "pub enum ProviderVideoCreateFamily",
        "pub fn video_create_transport_unsupported_reason(",
        "pub fn resolve_video_create_auth(",
        "pub fn build_video_create_request_body(",
        "pub fn build_video_create_upstream_url(",
        "pub fn build_video_create_headers(",
    ] {
        assert!(
            provider_transport_video.contains(pattern),
            "aether-provider-transport video.rs should own video create transport policy {pattern}"
        );
    }

    let specialized_video_decision = read_workspace_file(
        "apps/aether-gateway/src/ai_serving/planner/specialized/video/decision.rs",
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
fn ai_serving_files_routes_request_preparation_through_request_payload_seams() {
    let specialized_files_mod =
        read_workspace_file("apps/aether-gateway/src/ai_serving/planner/specialized/files.rs");
    assert!(
        specialized_files_mod.contains("mod request;"),
        "specialized files mod.rs should wire request seam"
    );

    let specialized_files_request = read_workspace_file(
        "apps/aether-gateway/src/ai_serving/planner/specialized/files/request.rs",
    );
    for pattern in [
        "pub(super) struct LocalGeminiFilesCandidatePayloadParts {",
        "pub(super) async fn resolve_local_gemini_files_candidate_payload_parts(",
        "build_gemini_files_upstream_url(",
        "build_gemini_files_request_body(",
        "build_gemini_files_headers(",
    ] {
        assert!(
            specialized_files_request.contains(pattern),
            "specialized files request.rs should adapt through provider-transport via {pattern}"
        );
    }
    for forbidden in [
        "build_gemini_files_passthrough_url(",
        "build_passthrough_headers_with_auth(",
        "apply_local_body_rules(",
        "apply_local_header_rules(",
        "resolve_local_gemini_auth(",
        "local_gemini_transport_unsupported_reason_with_network(",
    ] {
        assert!(
            !specialized_files_request.contains(forbidden),
            "specialized files request.rs should not own provider transport policy {forbidden}"
        );
    }

    let provider_transport_files =
        read_workspace_file("crates/aether-provider-transport/src/gemini_files.rs");
    for pattern in [
        "pub fn gemini_files_transport_unsupported_reason(",
        "pub fn resolve_gemini_files_auth(",
        "pub fn build_gemini_files_upstream_url(",
        "pub fn build_gemini_files_request_body(",
        "pub fn build_gemini_files_headers(",
    ] {
        assert!(
            provider_transport_files.contains(pattern),
            "aether-provider-transport gemini_files.rs should own files transport policy {pattern}"
        );
    }

    let specialized_files_decision = read_workspace_file(
        "apps/aether-gateway/src/ai_serving/planner/specialized/files/decision.rs",
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
fn ai_serving_image_routes_split_surface_normalization_and_transport_policy() {
    let specialized_image_request = read_workspace_file(
        "apps/aether-gateway/src/ai_serving/planner/specialized/image/request.rs",
    );
    for pattern in [
        "pub(super) struct LocalOpenAiImageCandidatePayloadParts {",
        "pub(super) async fn resolve_local_openai_image_candidate_payload_parts(",
        "normalize_openai_image_request(",
        "build_openai_image_provider_request_body(",
        "build_openai_image_upstream_url(",
        "build_openai_image_headers(",
    ] {
        assert!(
            specialized_image_request.contains(pattern),
            "specialized image request.rs should adapt through surface/transport helpers via {pattern}"
        );
    }
    for forbidden in [
        "fn build_provider_request_body(",
        "fn normalize_openai_image_request(",
        "fn parse_multipart_fields",
        "enum OpenAiImageOperation",
        "build_openai_responses_url(",
        "build_passthrough_headers_with_auth(",
        "apply_local_header_rules(",
        "resolve_local_openai_bearer_auth(",
    ] {
        assert!(
            !specialized_image_request.contains(forbidden),
            "specialized image request.rs should not own image surface/transport policy {forbidden}"
        );
    }

    let surface_image =
        read_workspace_file("crates/aether-ai-formats/src/request/specialized/image.rs");
    for pattern in [
        "pub enum OpenAiImageOperation",
        "pub fn is_openai_image_stream_request(",
        "pub fn resolve_requested_openai_image_model_for_request(",
        "pub fn normalize_openai_image_request(",
        "pub fn build_openai_image_provider_request_body(",
        "fn parse_multipart_fields_from_base64(",
    ] {
        assert!(
            surface_image.contains(pattern),
            "aether-ai-formats specialized/image.rs should own OpenAI image format surface logic {pattern}"
        );
    }

    let provider_transport_image =
        read_workspace_file("crates/aether-provider-transport/src/openai_image.rs");
    for pattern in [
        "pub fn openai_image_transport_unsupported_reason(",
        "pub fn resolve_openai_image_auth(",
        "pub fn build_openai_image_upstream_url(",
        "pub fn build_openai_image_headers(",
    ] {
        assert!(
            provider_transport_image.contains(pattern),
            "aether-provider-transport openai_image.rs should own image transport policy {pattern}"
        );
    }
}

#[test]
fn ai_serving_same_format_provider_root_request_separates_body_and_url_policy() {
    let provider_request = read_workspace_file(
        "apps/aether-gateway/src/ai_serving/planner/passthrough/provider/request.rs",
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
        "apps/aether-gateway/src/ai_serving/planner/passthrough/provider/request/body.rs",
    );
    assert!(
        provider_request_body.contains("build_same_format_provider_request_body_impl("),
        "passthrough/provider/request/body.rs should adapt same-format request-body construction through provider-transport"
    );
    for forbidden in [
        "serde_json::Map::from_iter(",
        "apply_local_body_rules(",
        "sanitize_claude_code_request_body(",
    ] {
        assert!(
            !provider_request_body.contains(forbidden),
            "passthrough/provider/request/body.rs should not own provider transport body policy {forbidden}"
        );
    }

    let provider_request_url = read_workspace_file(
        "apps/aether-gateway/src/ai_serving/planner/passthrough/provider/request/url.rs",
    );
    assert!(
        provider_request_url.contains("build_same_format_provider_upstream_url_impl("),
        "passthrough/provider/request/url.rs should adapt upstream URL construction through provider-transport"
    );
    for forbidden in [
        "crate::ai_serving::build_provider_transport_request_url(",
        "fn maybe_add_gemini_stream_alt_sse(",
    ] {
        assert!(
            !provider_request_url.contains(forbidden),
            "passthrough/provider/request/url.rs should not own provider transport URL policy {forbidden}"
        );
    }

    let same_format_provider_request = read_workspace_file(
        "apps/aether-gateway/src/ai_serving/planner/passthrough/provider/family/request.rs",
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
    assert!(
        same_format_provider_request.contains("build_same_format_provider_headers("),
        "same-format provider family request should route header construction through provider-transport"
    );
    for forbidden in [
        "build_complete_passthrough_headers(",
        "build_complete_passthrough_headers_with_auth(",
        "build_claude_code_passthrough_headers(",
        "build_kiro_provider_headers(",
        "apply_local_header_rules(",
        "ensure_upstream_auth_header(",
    ] {
        assert!(
            !same_format_provider_request.contains(forbidden),
            "same-format provider family request should not own provider transport header policy {forbidden}"
        );
    }
}

#[test]
fn ai_serving_openai_chat_routes_request_preparation_through_request_payload_seams() {
    let openai_chat_decision = read_workspace_file(
        "apps/aether-gateway/src/ai_serving/planner/standard/openai/chat/decision.rs",
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
        "apps/aether-gateway/src/ai_serving/planner/standard/openai/chat/decision/request.rs",
    );
    for pattern in [
        "pub(crate) struct LocalOpenAiChatCandidatePayloadParts {",
        "pub(crate) async fn resolve_local_openai_chat_candidate_payload_parts(",
        "is_kiro_claude_messages_transport(",
        "build_kiro_cross_format_upstream_url(",
        "build_standard_provider_request_headers(",
    ] {
        assert!(
            openai_chat_request.contains(pattern),
            "openai chat request.rs should own {pattern}"
        );
    }
    assert!(
        !openai_chat_request.contains(".eq_ignore_ascii_case(\"kiro\")"),
        "openai chat request.rs should route Kiro provider checks through provider-transport"
    );
    for forbidden in [
        "build_claude_passthrough_headers(",
        "build_openai_passthrough_headers(",
        "build_complete_passthrough_headers_with_auth(",
        "apply_local_header_rules(",
        "ensure_upstream_auth_header(",
        "uses_vertex_api_key_query_auth(",
        "build_provider_transport_request_url(",
    ] {
        assert!(
            !openai_chat_request.contains(forbidden),
            "openai chat request.rs should not own standard provider header policy {forbidden}"
        );
    }

    let openai_chat_payload = read_workspace_file(
        "apps/aether-gateway/src/ai_serving/planner/standard/openai/chat/decision/payload.rs",
    );
    assert!(
        openai_chat_payload.contains("resolve_local_openai_chat_candidate_payload_parts("),
        "openai chat payload.rs should consume request.rs preparation output"
    );
}

#[test]
fn ai_serving_payload_metadata_owns_local_execution_decision_response_shape() {
    let planner_mod = read_workspace_file("apps/aether-gateway/src/ai_serving/planner/mod.rs");
    assert!(
        !planner_mod.contains("mod payload_metadata;"),
        "planner/mod.rs should not keep gateway-owned payload_metadata after serving extraction"
    );

    assert!(
        !workspace_file_exists("apps/aether-gateway/src/ai_serving/planner/payload_metadata.rs"),
        "gateway payload_metadata.rs should be removed after serving extraction"
    );

    let decision_payload = read_workspace_file("crates/aether-ai-serving/src/decision_payload.rs");
    for pattern in [
        "pub struct AiExecutionDecisionResponseParts {",
        "pub fn build_ai_execution_decision_response(",
        "pub const fn ai_execution_decision_action(",
    ] {
        assert!(
            decision_payload.contains(pattern),
            "aether-ai-serving decision_payload.rs should own {pattern}"
        );
    }

    for path in [
        "apps/aether-gateway/src/ai_serving/planner/standard/family/payload.rs",
        "apps/aether-gateway/src/ai_serving/planner/passthrough/provider/family/payload.rs",
        "apps/aether-gateway/src/ai_serving/planner/standard/openai/chat/decision/payload.rs",
        "apps/aether-gateway/src/ai_serving/planner/standard/openai/responses/decision/payload.rs",
        "apps/aether-gateway/src/ai_serving/planner/specialized/image/decision.rs",
        "apps/aether-gateway/src/ai_serving/planner/specialized/video/decision.rs",
        "apps/aether-gateway/src/ai_serving/planner/specialized/files/decision.rs",
    ] {
        let source = read_workspace_file(path);
        assert!(
            source.contains("build_ai_execution_decision_response("),
            "{path} should route local decision payload construction through aether-ai-serving"
        );
        assert!(
            !source.contains("AiExecutionDecision {"),
            "{path} should not inline AiExecutionDecision construction after payload metadata extraction"
        );
    }
}

#[test]
fn ai_serving_owns_pure_planner_diagnostics_and_execution_labels() {
    for path in [
        "apps/aether-gateway/src/ai_serving/planner/failure_diagnostic.rs",
        "apps/aether-gateway/src/ai_serving/planner/standard/request_body_diagnostics.rs",
    ] {
        assert!(
            !workspace_file_exists(path),
            "{path} should be removed after pure planner diagnostics moved to aether-ai-serving"
        );
    }

    let serving_failure_diagnostic =
        read_workspace_file("crates/aether-ai-serving/src/failure_diagnostic.rs");
    for pattern in [
        "pub enum CandidateFailureDiagnosticKind {",
        "pub struct CandidateFailureDiagnostic {",
        "pub fn upstream_url_missing(",
        "pub fn header_rules_apply_failed(",
        "pub fn body_rules_apply_failed(",
        "pub fn to_extra_data(",
    ] {
        assert!(
            serving_failure_diagnostic.contains(pattern),
            "aether-ai-serving should own candidate failure diagnostic helper {pattern}"
        );
    }

    let serving_request_body_diagnostics =
        read_workspace_file("crates/aether-ai-serving/src/request_body_diagnostics.rs");
    for pattern in [
        "pub fn request_body_build_failure_extra_data(",
        "pub fn same_format_provider_request_body_failure_extra_data(",
        "is_openai_responses_family_format(client_api_format)",
    ] {
        assert!(
            serving_request_body_diagnostics.contains(pattern),
            "aether-ai-serving should own request-body diagnostic helper {pattern}"
        );
    }
    assert!(
        !serving_request_body_diagnostics.contains("crate::ai_serving"),
        "serving request-body diagnostics should not depend on gateway ai_serving seams"
    );

    let standard_mod =
        read_workspace_file("apps/aether-gateway/src/ai_serving/planner/standard/mod.rs");
    assert!(
        standard_mod.contains("pub(crate) use aether_ai_serving::{")
            && standard_mod.contains("request_body_build_failure_extra_data")
            && standard_mod.contains("same_format_provider_request_body_failure_extra_data"),
        "gateway standard planner should consume request-body diagnostics from aether-ai-serving"
    );

    let serving_dto = read_workspace_file("crates/aether-ai-serving/src/dto.rs");
    for pattern in ["pub enum ExecutionStrategy", "pub enum ConversionMode"] {
        assert!(
            serving_dto.contains(pattern),
            "aether-ai-serving DTO layer should own execution label {pattern}"
        );
    }

    let execution_runtime = read_workspace_file("apps/aether-gateway/src/execution_runtime/mod.rs");
    assert!(
        execution_runtime
            .contains("pub(crate) use aether_ai_serving::{ConversionMode, ExecutionStrategy};"),
        "gateway execution_runtime should reuse serving-owned execution labels"
    );
    for forbidden in [
        "pub(crate) enum ExecutionStrategy",
        "pub(crate) enum ConversionMode",
    ] {
        assert!(
            !execution_runtime.contains(forbidden),
            "gateway execution_runtime should not own execution labels after serving extraction: {forbidden}"
        );
    }
}

#[test]
fn ai_serving_report_context_owns_local_execution_context_shape() {
    let planner_mod = read_workspace_file("apps/aether-gateway/src/ai_serving/planner/mod.rs");
    assert!(
        planner_mod.contains("mod report_context;"),
        "planner/mod.rs should wire report_context helper module"
    );

    let serving_report_context =
        read_workspace_file("crates/aether-ai-serving/src/report_context.rs");
    for pattern in [
        "pub struct AiExecutionReportContextParts<'a> {",
        "pub fn build_ai_execution_report_context(",
        "pub fn provider_stream_event_api_format_for_provider_type(",
        "pub fn insert_provider_stream_event_api_format(",
        "pub fn build_ai_report_context_original_request_echo(",
        "\"original_headers\"",
        "\"retry_index\"",
        "\"provider_request_headers\"",
    ] {
        assert!(
            serving_report_context.contains(pattern),
            "aether-ai-serving should own report-context base shape {pattern}"
        );
    }

    let report_context =
        read_workspace_file("apps/aether-gateway/src/ai_serving/planner/report_context.rs");
    for pattern in [
        "pub(crate) struct LocalExecutionReportContextParts<'a> {",
        "pub(crate) fn build_local_execution_report_context(",
        "build_ai_execution_report_context(AiExecutionReportContextParts",
        "ai_provider_stream_event_api_format_for_provider_type(provider_type)",
        "insert_ai_provider_stream_event_api_format(extra_fields, provider_type)",
    ] {
        assert!(
            report_context.contains(pattern),
            "planner/report_context.rs should adapt report context through {pattern}"
        );
    }
    for forbidden in [
        "\"original_headers\".to_string()",
        "\"retry_index\".to_string()",
        "\"provider_request_headers\".to_string()",
    ] {
        assert!(
            !report_context.contains(forbidden),
            "planner/report_context.rs should not own report-context base field shape {forbidden}"
        );
    }

    for path in [
        "apps/aether-gateway/src/ai_serving/planner/standard/family/payload.rs",
        "apps/aether-gateway/src/ai_serving/planner/passthrough/provider/family/payload.rs",
        "apps/aether-gateway/src/ai_serving/planner/standard/openai/chat/decision/payload.rs",
        "apps/aether-gateway/src/ai_serving/planner/standard/openai/responses/decision/payload.rs",
        "apps/aether-gateway/src/ai_serving/planner/specialized/video/decision.rs",
        "apps/aether-gateway/src/ai_serving/planner/specialized/files/decision.rs",
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

    let ai_serving_mod = read_workspace_file("apps/aether-gateway/src/ai_serving/mod.rs");
    assert!(
        ai_serving_mod.contains(
            "build_ai_report_context_original_request_echo as build_report_context_original_request_echo"
        ),
        "ai_serving/mod.rs should expose original request echo through the serving seam"
    );
    assert!(
        !ai_serving_mod.contains("fn build_report_context_original_request_echo("),
        "ai_serving/mod.rs should not own original request echo construction after serving extraction"
    );
}

#[test]
fn ai_serving_standard_attempts_consume_eligible_local_candidates_without_transport_rereads() {
    let openai_chat_support = read_workspace_file(
        "apps/aether-gateway/src/ai_serving/planner/standard/openai/chat/decision/support.rs",
    );
    assert!(
        openai_chat_support
            .contains("LocalExecutionCandidateAttempt as LocalOpenAiChatCandidateAttempt"),
        "openai chat attempts should reuse shared LocalExecutionCandidateAttempt"
    );

    let openai_responses_support = read_workspace_file(
        "apps/aether-gateway/src/ai_serving/planner/standard/openai/responses/decision/support.rs",
    );
    assert!(
        openai_responses_support
            .contains("LocalExecutionCandidateAttempt as LocalOpenAiResponsesCandidateAttempt"),
        "openai responses attempts should reuse shared LocalExecutionCandidateAttempt"
    );

    let standard_family_mod =
        read_workspace_file("apps/aether-gateway/src/ai_serving/planner/standard/family/mod.rs");
    assert!(
        standard_family_mod
            .contains("LocalExecutionCandidateAttempt as LocalStandardCandidateAttempt"),
        "standard family attempts should reuse shared LocalExecutionCandidateAttempt"
    );

    for path in [
        "apps/aether-gateway/src/ai_serving/planner/standard/openai/chat/decision.rs",
        "apps/aether-gateway/src/ai_serving/planner/standard/openai/chat/decision/request.rs",
        "apps/aether-gateway/src/ai_serving/planner/standard/openai/responses/decision/request.rs",
        "apps/aether-gateway/src/ai_serving/planner/standard/family/request.rs",
    ] {
        let source = read_workspace_file(path);
        assert!(
            !source.contains("read_provider_transport_snapshot("),
            "{path} should consume eligibility-owned transport snapshots instead of rereading them"
        );
    }

    let openai_responses_request = read_workspace_file(
        "apps/aether-gateway/src/ai_serving/planner/standard/openai/responses/decision/request.rs",
    );
    for pattern in [
        "is_antigravity_provider_transport(",
        "is_kiro_claude_messages_transport(",
        "build_kiro_cross_format_upstream_url(",
        "build_standard_provider_request_headers(",
    ] {
        assert!(
            openai_responses_request.contains(pattern),
            "openai responses request preparation should route provider-private type checks through provider-transport via {pattern}"
        );
    }
    for forbidden in [
        ".eq_ignore_ascii_case(\"antigravity\")",
        ".eq_ignore_ascii_case(\"kiro\")",
        "build_claude_passthrough_headers(",
        "build_openai_passthrough_headers(",
        "build_complete_passthrough_headers_with_auth(",
        "apply_local_header_rules(",
        "ensure_upstream_auth_header(",
        "uses_vertex_api_key_query_auth(",
        "build_provider_transport_request_url(",
    ] {
        assert!(
            !openai_responses_request.contains(forbidden),
            "openai responses request preparation should not inline provider-private transport policy {forbidden}"
        );
    }

    let provider_transport_standard =
        read_workspace_file("crates/aether-provider-transport/src/standard.rs");
    for pattern in [
        "pub struct StandardProviderRequestHeadersInput",
        "pub struct StandardProviderRequestHeaders",
        "pub fn build_standard_provider_request_headers(",
        "pub fn apply_standard_provider_request_body_rules(",
        "apply_local_body_rules(",
        "build_complete_passthrough_headers_with_auth(",
        "build_claude_passthrough_headers(",
        "build_openai_passthrough_headers(",
        "apply_local_header_rules(",
        "uses_vertex_api_key_query_auth(",
    ] {
        assert!(
            provider_transport_standard.contains(pattern),
            "aether-provider-transport standard.rs should own standard provider header policy {pattern}"
        );
    }

    let provider_transport_request_url =
        read_workspace_file("crates/aether-provider-transport/src/request_url.rs");
    assert!(
        provider_transport_request_url.contains("pub fn build_kiro_cross_format_upstream_url("),
        "provider-transport request_url.rs should own Kiro cross-format URL hook"
    );
}

#[test]
fn ai_serving_standard_plan_builders_delegate_fallback_transport_policy() {
    for path in [
        "apps/aether-gateway/src/ai_serving/planner/standard/plan_builders.rs",
        "apps/aether-gateway/src/ai_serving/planner/standard/gemini/plan_builders.rs",
        "apps/aether-gateway/src/ai_serving/planner/standard/openai/plan_builders/sync.rs",
        "apps/aether-gateway/src/ai_serving/planner/standard/openai/plan_builders/stream.rs",
    ] {
        let source = read_workspace_file(path);
        for pattern in [
            "build_standard_plan_fallback_headers(",
            "StandardPlanFallbackAcceptPolicy",
            "StandardPlanFallbackHeadersInput",
        ] {
            assert!(
                source.contains(pattern),
                "{path} should route fallback transport policy through provider-transport via {pattern}"
            );
        }
        for forbidden in [
            "build_complete_passthrough_headers_with_auth(",
            "build_claude_passthrough_headers(",
            "build_openai_passthrough_headers(",
            "ensure_upstream_auth_header(",
        ] {
            assert!(
                !source.contains(forbidden),
                "{path} should not own fallback provider transport detail {forbidden}"
            );
        }
    }

    for path in [
        "apps/aether-gateway/src/ai_serving/planner/standard/openai/plan_builders/sync.rs",
        "apps/aether-gateway/src/ai_serving/planner/standard/openai/plan_builders/stream.rs",
    ] {
        let source = read_workspace_file(path);
        for pattern in [
            "build_standard_plan_fallback_openai_chat_url(",
            "build_standard_plan_fallback_openai_responses_url(",
        ] {
            assert!(
                source.contains(pattern),
                "{path} should route OpenAI fallback URL policy through provider-transport via {pattern}"
            );
        }
        for forbidden in ["build_openai_chat_url(", "build_openai_responses_url("] {
            assert!(
                !source.contains(forbidden),
                "{path} should not own OpenAI fallback URL detail {forbidden}"
            );
        }
    }

    let provider_transport_standard =
        read_workspace_file("crates/aether-provider-transport/src/standard.rs");
    for pattern in [
        "pub enum StandardPlanFallbackAcceptPolicy",
        "pub struct StandardPlanFallbackHeadersInput",
        "pub fn build_standard_plan_fallback_headers(",
        "pub fn build_standard_plan_fallback_openai_chat_url(",
        "pub fn build_standard_plan_fallback_openai_responses_url(",
        "build_complete_passthrough_headers_with_auth(",
        "build_claude_passthrough_headers(",
        "build_openai_passthrough_headers(",
        "build_openai_chat_url(",
        "build_openai_responses_url(",
        "ensure_upstream_auth_header(",
    ] {
        assert!(
            provider_transport_standard.contains(pattern),
            "aether-provider-transport standard.rs should own fallback transport policy {pattern}"
        );
    }
}

#[test]
fn ai_serving_specialized_files_attempts_consume_eligible_local_candidates_without_transport_rereads(
) {
    let specialized_files_support = read_workspace_file(
        "apps/aether-gateway/src/ai_serving/planner/specialized/files/support.rs",
    );
    assert!(
        specialized_files_support
            .contains("LocalExecutionCandidateAttempt as LocalGeminiFilesCandidateAttempt"),
        "specialized files attempts should reuse shared LocalExecutionCandidateAttempt"
    );
    assert!(
        specialized_files_support
            .contains("LocalCandidateResolutionMode::WithoutTransportPairGate"),
        "specialized files support should request no-transport-pair-gate runtime gating through candidate materialization"
    );
    assert!(
        !specialized_files_support.contains("rank_local_execution_candidates("),
        "specialized files support should not bypass candidate_resolution with raw affinity ranking"
    );

    let specialized_files_decision = read_workspace_file(
        "apps/aether-gateway/src/ai_serving/planner/specialized/files/decision.rs",
    );
    assert!(
        !specialized_files_decision.contains("read_provider_transport_snapshot("),
        "specialized files decision should consume eligibility-owned transport snapshots instead of rereading them"
    );
}

#[test]
fn ai_serving_candidate_sources_share_cross_format_auth_filter_helper() {
    let planner_mod = read_workspace_file("apps/aether-gateway/src/ai_serving/planner/mod.rs");
    assert!(
        planner_mod.contains("mod candidate_source;"),
        "planner/mod.rs should wire candidate_source helper module"
    );

    let candidate_source =
        read_workspace_file("apps/aether-gateway/src/ai_serving/planner/candidate_source.rs");
    assert!(
        candidate_source.contains("pub(crate) fn auth_snapshot_allows_cross_format_candidate("),
        "planner/candidate_source.rs should own shared cross-format auth filtering"
    );
    for pattern in [
        "run_ai_candidate_preselection(&port",
        "impl AiCandidatePreselectionPort for GatewayLocalCandidatePreselectionPort",
        "preselect_local_execution_candidates_with_serving",
    ] {
        assert!(
            candidate_source.contains(pattern),
            "planner/candidate_source.rs should implement serving preselection ports through {pattern}"
        );
    }

    for path in [
        "apps/aether-gateway/src/ai_serving/planner/standard/openai/chat/plans/candidates.rs",
        "apps/aether-gateway/src/ai_serving/planner/standard/openai/responses/decision/support.rs",
        "apps/aether-gateway/src/ai_serving/planner/standard/family/candidates.rs",
    ] {
        let source = read_workspace_file(path);
        assert!(
            source.contains("preselect_local_execution_candidates_with_serving("),
            "{path} should use the serving candidate preselection adapter"
        );
        assert!(
            !source.contains("auth_snapshot_allows_cross_format_candidate("),
            "{path} should not hand-roll cross-format auth filtering after preselection extraction"
        );
    }
}

#[test]
fn ai_serving_spec_metadata_owns_family_requested_model_and_plan_builder_routing() {
    let planner_mod = read_workspace_file("apps/aether-gateway/src/ai_serving/planner/mod.rs");
    assert!(
        planner_mod.contains("mod spec_metadata;"),
        "planner/mod.rs should wire spec_metadata plan-routing adapter"
    );

    let spec_metadata =
        read_workspace_file("apps/aether-gateway/src/ai_serving/planner/spec_metadata.rs");
    let serving_surface_spec = read_workspace_file("crates/aether-ai-serving/src/surface_spec.rs");
    for pattern in [
        "pub enum AiRequestedModelFamily {",
        "pub struct AiExecutionSurfaceSpecMetadata {",
        "pub const fn ai_requested_model_family_for_standard_source(",
        "pub const fn ai_requested_model_family_for_same_format_provider(",
        "pub const fn ai_requested_model_family_for_video_create(",
        "pub const fn ai_standard_spec_metadata(",
        "pub const fn ai_same_format_provider_spec_metadata(",
        "pub const fn ai_openai_responses_spec_metadata(",
        "pub const fn ai_gemini_files_spec_metadata(",
        "pub const fn ai_video_create_spec_metadata(",
        "pub const fn ai_openai_image_spec_metadata(",
    ] {
        assert!(
            serving_surface_spec.contains(pattern),
            "aether-ai-serving should own pure surface spec metadata {pattern}"
        );
    }
    for pattern in [
        "ai_standard_spec_metadata as local_standard_spec_metadata",
        "ai_same_format_provider_spec_metadata as local_same_format_provider_spec_metadata",
        "ai_openai_responses_spec_metadata as local_openai_responses_spec_metadata",
        "ai_gemini_files_spec_metadata as local_gemini_files_spec_metadata",
        "ai_video_create_spec_metadata as local_video_create_spec_metadata",
        "AiExecutionSurfaceSpecMetadata as LocalExecutionSurfaceSpecMetadata",
        "AiRequestedModelFamily as RequestedModelFamily",
        "pub(crate) fn build_sync_plan_from_requested_model_family(",
        "pub(crate) fn build_stream_plan_from_requested_model_family(",
    ] {
        assert!(
            spec_metadata.contains(pattern),
            "planner/spec_metadata.rs should adapt serving spec metadata or own gateway plan routing through {pattern}"
        );
    }
    for forbidden in [
        "pub(crate) struct LocalExecutionSurfaceSpecMetadata {",
        "pub(crate) fn requested_model_family_for_standard_source(",
        "pub(crate) fn requested_model_family_for_same_format_provider(",
        "pub(crate) fn requested_model_family_for_video_create(",
        "pub(crate) fn local_standard_spec_metadata(",
        "pub(crate) fn local_same_format_provider_spec_metadata(",
        "pub(crate) fn local_openai_responses_spec_metadata(",
        "pub(crate) fn local_gemini_files_spec_metadata(",
        "pub(crate) fn local_video_create_spec_metadata(",
        "pub(crate) fn local_openai_image_spec_metadata(",
    ] {
        assert!(
            !spec_metadata.contains(forbidden),
            "planner/spec_metadata.rs should not own pure surface spec metadata after serving extraction: {forbidden}"
        );
    }

    for (path, pattern) in [
        (
            "apps/aether-gateway/src/ai_serving/planner/standard/family/candidates.rs",
            "local_standard_spec_metadata(",
        ),
        (
            "apps/aether-gateway/src/ai_serving/planner/standard/family/build.rs",
            "local_standard_spec_metadata(",
        ),
        (
            "apps/aether-gateway/src/ai_serving/planner/standard/family/request.rs",
            "local_standard_spec_metadata(",
        ),
        (
            "apps/aether-gateway/src/ai_serving/planner/standard/family/payload.rs",
            "local_standard_spec_metadata(",
        ),
        (
            "apps/aether-gateway/src/ai_serving/planner/passthrough/provider/family/candidates.rs",
            "local_same_format_provider_spec_metadata(",
        ),
        (
            "apps/aether-gateway/src/ai_serving/planner/passthrough/provider/plans.rs",
            "local_same_format_provider_spec_metadata(",
        ),
        (
            "apps/aether-gateway/src/ai_serving/planner/passthrough/provider/family/build.rs",
            "local_same_format_provider_spec_metadata(",
        ),
        (
            "apps/aether-gateway/src/ai_serving/planner/passthrough/provider/family/request/prepare.rs",
            "local_same_format_provider_spec_metadata(",
        ),
        (
            "apps/aether-gateway/src/ai_serving/planner/passthrough/provider/family/payload.rs",
            "local_same_format_provider_spec_metadata(",
        ),
        (
            "apps/aether-gateway/src/ai_serving/planner/specialized/video/support.rs",
            "local_video_create_spec_metadata(",
        ),
        (
            "apps/aether-gateway/src/ai_serving/planner/specialized/video.rs",
            "local_video_create_spec_metadata(",
        ),
        (
            "apps/aether-gateway/src/ai_serving/planner/specialized/video/decision.rs",
            "local_video_create_spec_metadata(",
        ),
        (
            "apps/aether-gateway/src/ai_serving/planner/specialized/files.rs",
            "local_gemini_files_spec_metadata(",
        ),
        (
            "apps/aether-gateway/src/ai_serving/planner/specialized/files/decision.rs",
            "local_gemini_files_spec_metadata(",
        ),
        (
            "apps/aether-gateway/src/ai_serving/planner/standard/openai/responses/plans.rs",
            "local_openai_responses_spec_metadata(",
        ),
        (
            "apps/aether-gateway/src/ai_serving/planner/standard/openai/responses/decision/support.rs",
            "local_openai_responses_spec_metadata(",
        ),
        (
            "apps/aether-gateway/src/ai_serving/planner/standard/openai/responses/decision/request.rs",
            "local_openai_responses_spec_metadata(",
        ),
        (
            "apps/aether-gateway/src/ai_serving/planner/standard/openai/responses/decision/payload.rs",
            "local_openai_responses_spec_metadata(",
        ),
        (
            "apps/aether-gateway/src/ai_serving/planner/standard/family/build.rs",
            "build_sync_plan_from_requested_model_family(",
        ),
        (
            "apps/aether-gateway/src/ai_serving/planner/standard/family/build.rs",
            "build_stream_plan_from_requested_model_family(",
        ),
        (
            "apps/aether-gateway/src/ai_serving/planner/passthrough/provider/plans.rs",
            "build_sync_plan_from_requested_model_family(",
        ),
        (
            "apps/aether-gateway/src/ai_serving/planner/passthrough/provider/plans.rs",
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
fn ai_serving_same_format_provider_request_policy_owns_provider_type_behavior() {
    let request_root = read_workspace_file(
        "apps/aether-gateway/src/ai_serving/planner/passthrough/provider/family/request.rs",
    );
    assert!(
        request_root.contains("mod policy;"),
        "same-format provider request seam should wire request/policy.rs"
    );

    let request_policy = read_workspace_file(
        "apps/aether-gateway/src/ai_serving/planner/passthrough/provider/family/request/policy.rs",
    );
    let provider_transport_policy =
        read_workspace_file("crates/aether-provider-transport/src/same_format_provider.rs");
    for pattern in [
        "pub struct SameFormatProviderRequestBehavior {",
        "pub struct SameFormatProviderRequestBodyInput",
        "pub struct SameFormatProviderHeadersInput",
        "pub fn classify_same_format_provider_request_behavior(",
        "pub fn build_same_format_provider_request_body(",
        "pub fn build_same_format_provider_upstream_url(",
        "pub fn build_same_format_provider_headers(",
        "pub fn same_format_provider_transport_supported(",
        "pub fn should_try_same_format_provider_oauth_auth(",
        "pub fn resolve_same_format_provider_direct_auth(",
    ] {
        assert!(
            provider_transport_policy.contains(pattern),
            "aether-provider-transport should own same-format provider transport policy {pattern}"
        );
    }
    for pattern in [
        "classify_same_format_provider_request_behavior_impl(",
        "same_format_provider_transport_supported_impl(",
        "should_try_same_format_provider_oauth_auth_impl(",
        "resolve_same_format_provider_direct_auth_impl(",
        "SameFormatProviderRequestBehaviorParams",
        "fn same_format_provider_family(",
    ] {
        assert!(
            request_policy.contains(pattern),
            "gateway same-format provider request policy should adapt provider-transport helper {pattern}"
        );
    }
    for forbidden in [
        ".eq_ignore_ascii_case(\"antigravity\")",
        ".eq_ignore_ascii_case(\"claude_code\")",
        ".eq_ignore_ascii_case(\"kiro\")",
        "local_kiro_request_transport_unsupported_reason_with_network(",
        "local_claude_code_transport_unsupported_reason_with_network(",
        "local_vertex_api_key_gemini_transport_unsupported_reason_with_network(",
        "resolve_local_standard_auth(",
        "resolve_local_gemini_auth(",
    ] {
        assert!(
            !request_policy.contains(forbidden),
            "gateway same-format provider request policy should not own provider transport detail {forbidden}"
        );
    }

    let request_prepare = read_workspace_file(
        "apps/aether-gateway/src/ai_serving/planner/passthrough/provider/family/request/prepare.rs",
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
fn ai_serving_decision_inputs_share_authenticated_input_helper() {
    let planner_mod = read_workspace_file("apps/aether-gateway/src/ai_serving/planner/mod.rs");
    assert!(
        planner_mod.contains("mod decision_input;"),
        "planner/mod.rs should wire decision_input helper module"
    );

    let serving_decision_input =
        read_workspace_file("crates/aether-ai-serving/src/decision_input.rs");
    for pattern in [
        "pub trait AiAuthenticatedDecisionInputPort",
        "pub async fn run_ai_authenticated_decision_input",
        "read_auth_snapshot",
        "resolve_required_capabilities",
        "build_resolved_input",
    ] {
        assert!(
            serving_decision_input.contains(pattern),
            "aether-ai-serving should own authenticated decision-input use-case primitive {pattern}"
        );
    }

    let decision_input =
        read_workspace_file("apps/aether-gateway/src/ai_serving/planner/decision_input.rs");
    for pattern in [
        "pub(crate) struct ResolvedLocalDecisionAuthInput {",
        "pub(crate) struct LocalRequestedModelDecisionInput {",
        "pub(crate) struct LocalAuthenticatedDecisionInput {",
        "pub(crate) fn build_local_requested_model_decision_input(",
        "pub(crate) fn build_local_authenticated_decision_input(",
        "pub(crate) async fn resolve_local_authenticated_decision_input(",
        "impl AiAuthenticatedDecisionInputPort for GatewayAuthenticatedDecisionInputPort",
        "run_ai_authenticated_decision_input(",
    ] {
        assert!(
            decision_input.contains(pattern),
            "planner/decision_input.rs should keep gateway DTOs and delegate authenticated decision input through {pattern}"
        );
    }

    for path in [
        "apps/aether-gateway/src/ai_serving/planner/standard/family/candidates.rs",
        "apps/aether-gateway/src/ai_serving/planner/passthrough/provider/family/candidates.rs",
        "apps/aether-gateway/src/ai_serving/planner/standard/openai/responses/decision/support.rs",
        "apps/aether-gateway/src/ai_serving/planner/standard/openai/chat/plans/resolve.rs",
        "apps/aether-gateway/src/ai_serving/planner/specialized/video/support.rs",
        "apps/aether-gateway/src/ai_serving/planner/specialized/files/support.rs",
    ] {
        let source = read_workspace_file(path);
        assert!(
            source.contains("resolve_local_authenticated_decision_input("),
            "{path} should use the shared authenticated decision input helper"
        );
        if path.ends_with("/standard/openai/responses/decision/support.rs")
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
            "apps/aether-gateway/src/ai_serving/planner/standard/family/mod.rs",
            "LocalRequestedModelDecisionInput as LocalStandardDecisionInput",
        ),
        (
            "apps/aether-gateway/src/ai_serving/planner/passthrough/provider/family/mod.rs",
            "LocalRequestedModelDecisionInput as LocalSameFormatProviderDecisionInput",
        ),
        (
            "apps/aether-gateway/src/ai_serving/planner/standard/openai/chat/decision/support.rs",
            "LocalRequestedModelDecisionInput as LocalOpenAiChatDecisionInput",
        ),
        (
            "apps/aether-gateway/src/ai_serving/planner/standard/openai/responses/decision/support.rs",
            "LocalRequestedModelDecisionInput as LocalOpenAiResponsesDecisionInput",
        ),
        (
            "apps/aether-gateway/src/ai_serving/planner/specialized/video/support.rs",
            "LocalRequestedModelDecisionInput as LocalVideoCreateDecisionInput",
        ),
        (
            "apps/aether-gateway/src/ai_serving/planner/specialized/files/support.rs",
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
            "apps/aether-gateway/src/ai_serving/planner/standard/family/candidates.rs",
            "build_local_requested_model_decision_input(",
        ),
        (
            "apps/aether-gateway/src/ai_serving/planner/passthrough/provider/family/candidates.rs",
            "build_local_requested_model_decision_input(",
        ),
        (
            "apps/aether-gateway/src/ai_serving/planner/standard/openai/chat/plans/resolve.rs",
            "build_local_requested_model_decision_input(",
        ),
        (
            "apps/aether-gateway/src/ai_serving/planner/standard/openai/responses/decision/support.rs",
            "build_local_requested_model_decision_input(",
        ),
        (
            "apps/aether-gateway/src/ai_serving/planner/specialized/video/support.rs",
            "build_local_requested_model_decision_input(",
        ),
        (
            "apps/aether-gateway/src/ai_serving/planner/specialized/files/support.rs",
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
fn ai_serving_leaf_planner_owners_route_contract_specs_through_gateway_seams() {
    for path in [
        "apps/aether-gateway/src/ai_serving/planner/specialized/files/support.rs",
        "apps/aether-gateway/src/ai_serving/planner/specialized/video/support.rs",
        "apps/aether-gateway/src/ai_serving/planner/standard/openai/chat/decision/support.rs",
        "apps/aether-gateway/src/ai_serving/planner/standard/openai/responses/decision/support.rs",
    ] {
        let source = read_workspace_file(path);
        assert!(
            !source.contains("aether_ai_formats::contracts::ExecutionRuntimeAuthContext"),
            "{path} should consume ExecutionRuntimeAuthContext through gateway ai_serving seams"
        );
        assert!(
            source.contains("ExecutionRuntimeAuthContext"),
            "{path} should use the gateway ai_serving root seam for ExecutionRuntimeAuthContext"
        );
    }

    let specialized_files_decision = read_workspace_file(
        "apps/aether-gateway/src/ai_serving/planner/specialized/files/decision.rs",
    );
    assert!(
        !specialized_files_decision
            .contains("aether_ai_formats::request::specialized::files::LocalGeminiFilesSpec"),
        "planner/specialized/files/decision.rs should consume LocalGeminiFilesSpec through the local specialized seam"
    );
    assert!(
        specialized_files_decision.contains("use super::LocalGeminiFilesSpec;"),
        "planner/specialized/files/decision.rs should use the local specialized seam for LocalGeminiFilesSpec"
    );

    let specialized_video_support = read_workspace_file(
        "apps/aether-gateway/src/ai_serving/planner/specialized/video/support.rs",
    );
    assert!(
        specialized_video_support.contains("use super::{LocalVideoCreateFamily, LocalVideoCreateSpec};"),
        "planner/specialized/video/support.rs should use local video seams for LocalVideoCreate* types"
    );
}

#[test]
fn ai_serving_m5_moves_contracts_and_route_logic_into_format_crate() {
    for path in [
        "crates/aether-ai-formats/src/contracts/actions.rs",
        "crates/aether-ai-formats/src/contracts/plan_kinds.rs",
        "crates/aether-ai-formats/src/contracts/report_kinds.rs",
        "crates/aether-ai-formats/src/request/route.rs",
    ] {
        assert!(
            workspace_file_exists(path),
            "{path} should exist after initial format crate extraction"
        );
    }

    for path in [
        "apps/aether-gateway/src/ai_serving/contracts",
        "apps/aether-gateway/src/ai_serving/contracts/actions.rs",
        "apps/aether-gateway/src/ai_serving/contracts/plan_kinds.rs",
        "apps/aether-gateway/src/ai_serving/contracts/report_kinds.rs",
    ] {
        assert!(
            !workspace_file_exists(path),
            "{path} should be removed after moving surface contract ownership"
        );
    }

    let gateway_ai_serving_mod = read_workspace_file("apps/aether-gateway/src/ai_serving/mod.rs");
    assert!(
        !gateway_ai_serving_mod.contains("mod contracts;"),
        "gateway ai_serving/mod.rs should not register a contracts module"
    );

    let gateway_route = read_workspace_file("apps/aether-gateway/src/ai_serving/planner/route.rs");
    let gateway_route_runtime = gateway_route
        .split("#[cfg(test)]")
        .next()
        .unwrap_or(gateway_route.as_str());
    assert!(
        gateway_route_runtime.contains("crate::ai_serving::"),
        "planner/route.rs should delegate route logic through the ai_serving root seam"
    );
    assert!(
        gateway_route_runtime.contains("is_matching_stream_http_request"),
        "planner/route.rs should delegate full HTTP stream matching to aether-ai-formats"
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
    for forbidden in [
        "OPENAI_IMAGE_STREAM_PLAN_KIND",
        "is_openai_image_stream_request(",
        "parts.uri.path(), body_json",
    ] {
        assert!(
            !gateway_route_runtime.contains(forbidden),
            "planner/route.rs should not keep surface-specific stream matching branch {forbidden}"
        );
    }

    let surface_route = read_workspace_file("crates/aether-ai-formats/src/request/route.rs");
    for pattern in [
        "pub fn is_matching_stream_http_request(",
        "is_openai_image_stream_request(parts, body_json, body_base64)",
    ] {
        assert!(
            surface_route.contains(pattern),
            "aether-ai-formats planner/route.rs should own HTTP stream matching format surface logic {pattern}"
        );
    }

    let gateway_api = read_workspace_file("apps/aether-gateway/src/ai_serving/api.rs");
    for pattern in [
        "pub(crate) fn parse_direct_request_body(",
        "pub(crate) fn resolve_execution_runtime_stream_plan_kind(",
        "pub(crate) fn resolve_execution_runtime_sync_plan_kind(",
        "pub(crate) fn is_matching_stream_request(",
        "pub(crate) fn supports_sync_execution_decision_kind(",
        "pub(crate) fn supports_stream_execution_decision_kind(",
    ] {
        assert!(
            gateway_api.contains(pattern),
            "ai_serving/api.rs should own facade wrapper {pattern}"
        );
    }

    let planner_mod = read_workspace_file("apps/aether-gateway/src/ai_serving/planner/mod.rs");
    for pattern in [
        "pub(crate) use self::common::parse_direct_request_body;",
        "pub(crate) use self::route::{",
    ] {
        assert!(
            !planner_mod.contains(pattern),
            "planner/mod.rs should not act as facade hub for {pattern}"
        );
    }

    let finalize_mod = read_workspace_file("apps/aether-gateway/src/ai_serving/finalize/mod.rs");
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
fn ai_serving_m5_moves_kiro_stream_helpers_into_format_crate() {
    assert!(
        workspace_file_exists("crates/aether-ai-formats/src/provider_compat/kiro_stream.rs"),
        "crates/aether-ai-formats/src/provider_compat/kiro_stream.rs should exist after kiro helper extraction"
    );
    assert!(
        workspace_file_exists("crates/aether-ai-formats/src/provider_compat/kiro_stream/state.rs"),
        "crates/aether-ai-formats/src/provider_compat/kiro_stream/state.rs should own the Kiro stream state machine"
    );

    for path in [
        "apps/aether-gateway/src/ai_serving/adaptation/kiro",
        "apps/aether-gateway/src/ai_serving/adaptation/kiro/stream",
        "apps/aether-gateway/src/ai_serving/adaptation/kiro/stream/util.rs",
    ] {
        assert!(
            !workspace_file_exists(path),
            "{path} should be removed after moving kiro helper ownership"
        );
    }

    let surface_api = read_workspace_file("crates/aether-ai-formats/src/api.rs");
    assert!(
        surface_api.contains("KiroToClaudeCliStreamState"),
        "aether-ai-formats api should export KiroToClaudeCliStreamState"
    );

    for file in collect_workspace_rust_files("apps/aether-gateway/src/ai_serving") {
        let source = std::fs::read_to_string(&file).expect("source file should be readable");
        assert!(
            !source.contains("crate::ai_serving::adaptation::KiroToClaudeCliStreamState"),
            "{} should import KiroToClaudeCliStreamState through the serving facade, not gateway adaptation",
            file.display()
        );
        assert!(
            !source.contains("adaptation::kiro::"),
            "{} should not reference the removed gateway Kiro adaptation module",
            file.display()
        );
    }

    let gateway_adaptation_mod =
        read_workspace_file("apps/aether-gateway/src/ai_serving/adaptation/mod.rs");
    assert!(
        !gateway_adaptation_mod.contains("mod kiro;"),
        "gateway adaptation module should not declare a Kiro stream adaptation submodule"
    );

    let gateway_pure = read_workspace_file("apps/aether-gateway/src/ai_serving/pure/mod.rs");
    assert!(
        gateway_pure.contains("KiroToClaudeCliStreamState"),
        "gateway pure facade should re-export KiroToClaudeCliStreamState from surfaces while call sites are migrated"
    );
}

#[test]
fn ai_serving_private_envelope_stream_normalizer_is_owned_by_format_crate() {
    assert!(
        workspace_file_exists("crates/aether-ai-formats/src/provider_compat/private_envelope.rs"),
        "surface private envelope adapter should exist"
    );
    assert!(
        !workspace_file_exists(
            "apps/aether-gateway/src/ai_serving/adaptation/private_envelope/stream.rs"
        ),
        "gateway private_envelope/stream.rs should be removed after stream normalizer ownership moves to surfaces"
    );
    assert!(
        !workspace_file_exists(
            "apps/aether-gateway/src/ai_serving/adaptation/private_envelope/tests.rs"
        ),
        "gateway private_envelope/tests.rs should be removed after pure normalizer tests move to surfaces"
    );

    let surface_private_envelope =
        read_workspace_file("crates/aether-ai-formats/src/provider_compat/private_envelope.rs");
    for expected in [
        "pub struct ProviderPrivateStreamNormalizer",
        "pub fn maybe_build_provider_private_stream_normalizer",
        "enum ProviderPrivateStreamNormalizeMode",
        "KiroToClaudeCliStreamState",
        "transform_provider_private_stream_line",
    ] {
        assert!(
            surface_private_envelope.contains(expected),
            "surface private_envelope.rs should own stream normalizer detail {expected}"
        );
    }

    let gateway_private_envelope =
        read_workspace_file("apps/aether-gateway/src/ai_serving/adaptation/private_envelope.rs");
    assert!(
        gateway_private_envelope.contains("maybe_normalize_provider_private_sync_report_payload"),
        "gateway private_envelope.rs should keep the usage-report DTO adapter"
    );
    assert!(
        gateway_private_envelope.contains("maybe_build_provider_private_stream_normalizer"),
        "gateway private_envelope.rs should re-export surface stream normalizer through ai_serving"
    );
    for forbidden in [
        "#[path = \"private_envelope/stream.rs\"]",
        "#[path = \"private_envelope/tests.rs\"]",
        "mod stream;",
        "mod tests;",
        "pub(crate) use self::stream",
        "ProviderPrivateStreamNormalizeMode",
        "KiroToClaudeCliStreamState",
    ] {
        assert!(
            !gateway_private_envelope.contains(forbidden),
            "gateway private_envelope.rs should not own stream normalizer detail {forbidden}"
        );
    }

    let gateway_sync = read_workspace_file(
        "apps/aether-gateway/src/ai_serving/adaptation/private_envelope/sync.rs",
    );
    assert!(
        gateway_sync.contains("GatewaySyncReportRequest"),
        "gateway private_envelope/sync.rs should be the concrete usage report adapter"
    );
    assert!(
        gateway_sync.contains("maybe_build_provider_private_stream_normalizer"),
        "gateway private_envelope/sync.rs should call the surface-owned stream normalizer"
    );
}

#[test]
fn ai_serving_runtime_adapter_dead_duplicates_are_removed() {
    for path in [
        "apps/aether-gateway/src/ai_serving/runtime/adapters/antigravity/auth.rs",
        "apps/aether-gateway/src/ai_serving/runtime/adapters/antigravity/policy.rs",
        "apps/aether-gateway/src/ai_serving/runtime/adapters/antigravity/request.rs",
        "apps/aether-gateway/src/ai_serving/runtime/adapters/antigravity/url.rs",
        "apps/aether-gateway/src/ai_serving/runtime/adapters/vertex/auth.rs",
        "apps/aether-gateway/src/ai_serving/runtime/adapters/vertex/policy.rs",
        "apps/aether-gateway/src/ai_serving/runtime/adapters/vertex/url.rs",
        "apps/aether-gateway/src/ai_serving/runtime/adapters/claude_code/auth.rs",
        "apps/aether-gateway/src/ai_serving/runtime/adapters/claude_code/policy.rs",
        "apps/aether-gateway/src/ai_serving/runtime/adapters/claude_code/request.rs",
        "apps/aether-gateway/src/ai_serving/runtime/adapters/claude_code/url.rs",
        "apps/aether-gateway/src/ai_serving/runtime/adapters/openai/auth.rs",
        "apps/aether-gateway/src/ai_serving/runtime/adapters/openai/policy.rs",
        "apps/aether-gateway/src/ai_serving/runtime/adapters/openai/request.rs",
        "apps/aether-gateway/src/ai_serving/runtime/adapters/openai/url.rs",
        "apps/aether-gateway/src/ai_serving/runtime/adapters/gemini/auth.rs",
        "apps/aether-gateway/src/ai_serving/runtime/adapters/gemini/policy.rs",
        "apps/aether-gateway/src/ai_serving/runtime/adapters/gemini/request.rs",
        "apps/aether-gateway/src/ai_serving/runtime/adapters/gemini/url.rs",
        "apps/aether-gateway/src/ai_serving/runtime/adapters/claude/auth.rs",
        "apps/aether-gateway/src/ai_serving/runtime/adapters/claude/policy.rs",
        "apps/aether-gateway/src/ai_serving/runtime/adapters/claude/request.rs",
        "apps/aether-gateway/src/ai_serving/runtime/adapters/claude/url.rs",
    ] {
        assert!(
            !workspace_file_exists(path),
            "{path} should be removed after provider-transport ownership consolidation"
        );
    }
}

#[test]
fn ai_serving_planner_route_remains_control_only() {
    let gateway_route = read_workspace_file("apps/aether-gateway/src/ai_serving/planner/route.rs");
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
fn ai_serving_error_body_is_owned_by_format_finalize_module() {
    assert!(
        !workspace_file_exists("apps/aether-gateway/src/ai_serving/conversion"),
        "gateway ai_serving should not keep a conversion directory; format conversion belongs to aether-ai-formats and transport checks belong to provider transport"
    );
    assert!(
        !workspace_file_exists("apps/aether-gateway/src/ai_serving/conversion/error.rs"),
        "ai_serving/conversion/error.rs should stay removed"
    );
    assert!(
        workspace_file_exists("crates/aether-ai-formats/src/response/error_body.rs"),
        "format error response-body helpers should live under finalize/error_body.rs"
    );
    assert!(
        !workspace_file_exists("crates/aether-ai-formats/src/protocol/conversion/error.rs"),
        "aether-ai-formats should not keep error response-body helpers under conversion"
    );

    let gateway_ai_serving_mod = read_workspace_file("apps/aether-gateway/src/ai_serving/mod.rs");
    assert!(
        !gateway_ai_serving_mod.contains("mod conversion;"),
        "gateway ai_serving/mod.rs should not register a conversion module"
    );

    for forbidden in [
        "pub(crate) enum LocalCoreSyncErrorKind",
        "pub enum LocalCoreSyncErrorKind",
        "fn build_core_error_body_for_client_format(",
    ] {
        assert!(
            !gateway_ai_serving_mod.contains(forbidden),
            "gateway ai_serving/mod.rs should not own {forbidden}"
        );
    }
}

#[test]
fn ai_serving_conversion_request_is_owned_by_format_crate() {
    assert!(
        workspace_file_exists("crates/aether-ai-formats/src/protocol/conversion/request/mod.rs"),
        "request conversion should live in aether-ai-formats"
    );
    assert!(
        !workspace_file_exists(
            "apps/aether-gateway/src/ai_serving/conversion/request/from_openai_chat/claude.rs"
        ),
        "ai_serving/conversion/request/from_openai_chat should not remain in gateway"
    );
    assert!(
        !workspace_file_exists(
            "apps/aether-gateway/src/ai_serving/conversion/request/to_openai_chat/claude.rs"
        ),
        "ai_serving/conversion/request/to_openai_chat should not remain in gateway"
    );
    assert!(
        !workspace_file_exists("apps/aether-gateway/src/ai_serving/conversion/request/mod.rs"),
        "gateway conversion/request/mod.rs should be removed after root-seam consolidation"
    );
    let gateway_ai_serving_mod = read_workspace_file("apps/aether-gateway/src/ai_serving/mod.rs");
    assert!(
        !gateway_ai_serving_mod.contains("pub(crate) mod request;"),
        "gateway ai_serving/mod.rs should not keep request re-export shell after root-seam consolidation"
    );

    let surface_api = read_workspace_file("crates/aether-ai-formats/src/api.rs");
    assert!(
        surface_api.contains("pub use aether_ai_formats::protocol::conversion::request::{"),
        "format API facade should re-export request conversion directly from aether-ai-formats"
    );
}

#[test]
fn ai_serving_conversion_response_is_owned_by_format_crate() {
    assert!(
        workspace_file_exists("crates/aether-ai-formats/src/protocol/conversion/response/mod.rs"),
        "response conversion should live in aether-ai-formats"
    );
    assert!(
        !workspace_file_exists(
            "apps/aether-gateway/src/ai_serving/conversion/response/from_openai_chat/claude_chat.rs"
        ),
        "ai_serving/conversion/response/from_openai_chat should not remain in gateway"
    );
    assert!(
        !workspace_file_exists(
            "apps/aether-gateway/src/ai_serving/conversion/response/to_openai_chat/claude_chat.rs"
        ),
        "ai_serving/conversion/response/to_openai_chat should not remain in gateway"
    );
    assert!(
        !workspace_file_exists("apps/aether-gateway/src/ai_serving/conversion/response/mod.rs"),
        "gateway conversion/response/mod.rs should be removed after root-seam consolidation"
    );
    let gateway_ai_serving_mod = read_workspace_file("apps/aether-gateway/src/ai_serving/mod.rs");
    assert!(
        !gateway_ai_serving_mod.contains("pub(crate) mod response;"),
        "gateway ai_serving/mod.rs should not keep response re-export shell after root-seam consolidation"
    );

    let surface_api = read_workspace_file("crates/aether-ai-formats/src/api.rs");
    assert!(
        surface_api.contains("pub use aether_ai_formats::protocol::conversion::response::{"),
        "format API facade should re-export response conversion directly from aether-ai-formats"
    );
}

#[test]
fn ai_format_crate_owns_conversion_and_surface_facade() {
    assert!(
        workspace_file_exists("crates/aether-ai-formats/src/protocol/conversion"),
        "aether-ai-formats should own the conversion directory"
    );

    let surface_lib = read_workspace_file("crates/aether-ai-formats/src/lib.rs");
    assert!(
        surface_lib.contains("pub mod protocol;"),
        "aether-ai-formats lib.rs should expose the protocol module"
    );

    let surface_api = read_workspace_file("crates/aether-ai-formats/src/api.rs");
    for pattern in [
        "pub use aether_ai_formats::{",
        "pub use aether_ai_formats::protocol::conversion::request::{",
        "pub use aether_ai_formats::protocol::conversion::response::{",
        "pub use crate::response::error_body::{",
    ] {
        assert!(
            surface_api.contains(pattern),
            "format API facade should expose pure dependencies through {pattern}"
        );
    }
}

#[test]
fn ai_serving_finalize_standard_sync_response_converters_are_owned_by_format_crate() {
    for path in [
        "apps/aether-gateway/src/ai_serving/finalize/standard/openai/sync/chat.rs",
        "apps/aether-gateway/src/ai_serving/finalize/standard/openai/sync/cli.rs",
        "apps/aether-gateway/src/ai_serving/finalize/standard/claude/sync/chat.rs",
        "apps/aether-gateway/src/ai_serving/finalize/standard/claude/sync/cli.rs",
        "apps/aether-gateway/src/ai_serving/finalize/standard/gemini/sync/chat.rs",
        "apps/aether-gateway/src/ai_serving/finalize/standard/gemini/sync/cli.rs",
    ] {
        assert!(
            !workspace_file_exists(path),
            "{path} should be deleted after sync finalize dispatch moved into surface-owned helpers"
        );
    }

    for (candidate_paths, symbol) in [
        (
            vec!["apps/aether-gateway/src/ai_serving/finalize/standard/mod.rs"],
            "convert_openai_responses_response_to_openai_chat",
        ),
        (
            vec!["apps/aether-gateway/src/ai_serving/finalize/standard/mod.rs"],
            "build_openai_responses_response",
        ),
        (
            vec!["apps/aether-gateway/src/ai_serving/finalize/standard/mod.rs"],
            "convert_openai_chat_response_to_openai_responses",
        ),
        (
            vec!["apps/aether-gateway/src/ai_serving/finalize/standard/mod.rs"],
            "convert_claude_chat_response_to_openai_chat",
        ),
        (
            vec!["apps/aether-gateway/src/ai_serving/finalize/standard/mod.rs"],
            "convert_openai_chat_response_to_claude_chat",
        ),
        (
            vec!["apps/aether-gateway/src/ai_serving/finalize/standard/mod.rs"],
            "convert_claude_response_to_openai_responses",
        ),
        (
            vec!["apps/aether-gateway/src/ai_serving/finalize/standard/mod.rs"],
            "convert_gemini_chat_response_to_openai_chat",
        ),
        (
            vec!["apps/aether-gateway/src/ai_serving/finalize/standard/mod.rs"],
            "convert_openai_chat_response_to_gemini_chat",
        ),
        (
            vec!["apps/aether-gateway/src/ai_serving/finalize/standard/mod.rs"],
            "convert_gemini_response_to_openai_responses",
        ),
    ] {
        let sources = candidate_paths
            .iter()
            .map(|path| read_workspace_file(path))
            .collect::<Vec<_>>();
        assert!(
            sources
                .iter()
                .any(|source| source.contains("crate::ai_serving::{") && source.contains(symbol)),
            "{symbol} should stay exposed through the ai_serving root seam from finalize/standard/mod.rs"
        );
    }
}

#[test]
fn ai_serving_finalize_stream_engine_is_owned_by_format_crate() {
    for path in [
        "crates/aether-ai-formats/src/response/sse.rs",
        "crates/aether-ai-formats/src/response/standard/stream_core/common.rs",
        "crates/aether-ai-formats/src/response/standard/stream_core/format_matrix.rs",
        "crates/aether-ai-formats/src/response/standard/openai/stream.rs",
        "crates/aether-ai-formats/src/response/standard/claude/stream.rs",
        "crates/aether-ai-formats/src/response/standard/gemini/stream.rs",
    ] {
        assert!(
            workspace_file_exists(path),
            "{path} should exist in aether-ai-formats finalize engine"
        );
    }

    for path in [
        "apps/aether-gateway/src/ai_serving/finalize/standard/openai/stream.rs",
        "apps/aether-gateway/src/ai_serving/finalize/standard/claude/stream.rs",
        "apps/aether-gateway/src/ai_serving/finalize/standard/gemini/stream.rs",
    ] {
        assert!(
            !workspace_file_exists(path),
            "{path} should be removed after finalize stream wrapper collapse"
        );
    }

    assert!(
        !workspace_file_exists(
            "apps/aether-gateway/src/ai_serving/finalize/standard/stream_core/common.rs"
        ),
        "stream_core/common.rs should be removed after canonical stream helper collapse"
    );
    for path in [
        "apps/aether-gateway/src/ai_serving/finalize/standard/stream_core/mod.rs",
        "apps/aether-gateway/src/ai_serving/finalize/standard/stream_core/orchestrator.rs",
    ] {
        assert!(
            !workspace_file_exists(path),
            "{path} should be removed after stream rewriter collapse"
        );
    }

    let surface_format_matrix = read_workspace_file(
        "crates/aether-ai-formats/src/response/standard/stream_core/format_matrix.rs",
    );
    for pattern in [
        "pub struct StreamingStandardFormatMatrix",
        "enum ProviderStreamParser",
        "enum ClientStreamEmitter",
    ] {
        assert!(
            surface_format_matrix.contains(pattern),
            "surface stream_core/format_matrix.rs should own {pattern}"
        );
    }

    let gateway_standard_mod =
        read_workspace_file("apps/aether-gateway/src/ai_serving/finalize/standard/mod.rs");
    assert!(
        !gateway_standard_mod.contains("stream_core"),
        "gateway standard finalize module should not retain a stream_core wrapper after stream rewrite collapse"
    );
}

#[test]
fn ai_serving_finalize_standard_sync_products_are_owned_by_format_crate() {
    assert!(
        workspace_file_exists("crates/aether-ai-formats/src/response/sync_products.rs"),
        "finalize sync_products should live in aether-ai-formats"
    );
    assert!(
        workspace_file_exists("crates/aether-ai-formats/src/response/sync_to_stream.rs"),
        "finalize sync-to-stream bridge should live in aether-ai-formats"
    );

    let surface_sync_products =
        read_workspace_file("crates/aether-ai-formats/src/response/sync_products.rs");
    for expected in [
        "pub fn maybe_build_standard_cross_format_sync_product_from_normalized_payload(",
        "pub fn maybe_build_standard_same_format_sync_body_from_normalized_payload(",
        "pub fn maybe_build_openai_responses_same_family_sync_body_from_normalized_payload(",
        "pub fn maybe_build_openai_chat_cross_format_sync_product_from_normalized_payload(",
        "pub fn maybe_build_openai_responses_cross_format_sync_product_from_normalized_payload(",
        "pub fn maybe_build_standard_sync_finalize_product_from_normalized_payload(",
        "pub fn aggregate_standard_chat_stream_sync_response(",
        "pub fn aggregate_standard_cli_stream_sync_response(",
        "pub fn aggregate_openai_chat_stream_sync_response(",
        "pub fn aggregate_openai_responses_stream_sync_response(",
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
            surface_sync_products.contains(expected),
            "surface finalize sync_products should own {expected}"
        );
    }

    let gateway_standard =
        read_workspace_file("apps/aether-gateway/src/ai_serving/finalize/standard/mod.rs");
    assert!(
        gateway_standard.contains("crate::ai_serving::"),
        "gateway finalize/standard/mod.rs should thinly re-export sync_products through the gateway ai_serving root seam"
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
        read_workspace_file("apps/aether-gateway/src/ai_serving/finalize/common.rs");
    assert!(
        !gateway_finalize_common.contains("pub(crate) fn parse_stream_json_events("),
        "gateway finalize/common.rs should not keep parse_stream_json_events after sync_products takeover"
    );

    for path in [
        "apps/aether-gateway/src/ai_serving/finalize/standard/openai/sync/mod.rs",
        "apps/aether-gateway/src/ai_serving/finalize/standard/claude/sync/mod.rs",
        "apps/aether-gateway/src/ai_serving/finalize/standard/gemini/sync/mod.rs",
    ] {
        assert!(
            !workspace_file_exists(path),
            "{path} should be deleted after sync wrapper flattening"
        );
    }

    for (path, forbidden) in [
        (
            "apps/aether-gateway/src/ai_serving/finalize/standard/mod.rs",
            "pub(crate) use openai::*;",
        ),
        (
            "apps/aether-gateway/src/ai_serving/finalize/standard/mod.rs",
            "pub(crate) use claude::*;",
        ),
        (
            "apps/aether-gateway/src/ai_serving/finalize/standard/mod.rs",
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
        "apps/aether-gateway/src/ai_serving/finalize/internal/sync_finalize.rs",
    );
    assert!(
        gateway_internal_sync.contains(
            "maybe_build_standard_sync_finalize_product_from_normalized_payload"
        ),
        "gateway internal/sync_finalize.rs should delegate normalized standard sync finalize dispatch to aether-ai-formats"
    );
    assert!(
        gateway_internal_sync.contains("maybe_build_openai_image_sync_finalize_product"),
        "gateway internal/sync_finalize.rs should delegate OpenAI image sync finalize parsing to aether-ai-formats"
    );
    for forbidden in [
        "CODEX_OPENAI_IMAGE_DEFAULT_OUTPUT_FORMAT",
        "base64::engine::general_purpose::STANDARD",
        "std::str::from_utf8(&body_bytes)",
        ".split(\"\\n\\n\")",
        "\"response.output_item.done\"",
        "\"image_generation_call\"",
        "maybe_build_local_openai_chat_stream_sync_response(",
        "maybe_build_local_openai_chat_sync_response(",
        "maybe_build_local_openai_chat_cross_format_stream_sync_response(",
        "maybe_build_local_openai_responses_stream_sync_response(",
        "maybe_build_local_openai_responses_cross_format_stream_sync_response(",
        "maybe_build_local_claude_cli_stream_sync_response(",
        "maybe_build_local_gemini_cli_stream_sync_response(",
        "maybe_build_local_claude_stream_sync_response(",
        "maybe_build_local_claude_sync_response(",
        "maybe_build_local_gemini_stream_sync_response(",
        "maybe_build_local_gemini_sync_response(",
        "maybe_build_local_openai_chat_cross_format_sync_response(",
        "maybe_build_local_openai_responses_cross_format_sync_response(",
    ] {
        assert!(
            !gateway_internal_sync.contains(forbidden),
            "gateway internal/sync_finalize.rs should not keep ordered wrapper dispatch detail {forbidden}"
        );
    }

    let surface_openai_image_stream =
        read_workspace_file("crates/aether-ai-formats/src/response/openai_image_stream.rs");
    for expected in [
        "pub fn maybe_build_openai_image_sync_finalize_product(",
        "pub struct OpenAiImageSyncFinalizeProduct",
        "OPENAI_IMAGE_SYNC_FINALIZE_REPORT_KIND",
        "CODEX_OPENAI_IMAGE_DEFAULT_OUTPUT_FORMAT",
        "base64::engine::general_purpose::STANDARD.decode",
    ] {
        assert!(
            surface_openai_image_stream.contains(expected),
            "surface openai_image_stream.rs should own OpenAI image sync finalize detail {expected}"
        );
    }

    let surface_sync_to_stream =
        read_workspace_file("crates/aether-ai-formats/src/response/sync_to_stream.rs");
    for expected in [
        "pub fn maybe_bridge_standard_sync_json_to_stream(",
        "pub struct SyncToStreamBridgeOutcome",
        "fn maybe_bridge_openai_image_sync_json_to_stream(",
        "fn build_terminal_summary_from_openai_responses_response(",
    ] {
        assert!(
            surface_sync_to_stream.contains(expected),
            "surface sync_to_stream.rs should own {expected}"
        );
    }

    let gateway_sync_to_stream = read_workspace_file(
        "apps/aether-gateway/src/ai_serving/finalize/internal/sync_to_stream.rs",
    );
    assert!(
        gateway_sync_to_stream
            .contains("crate::ai_serving::pure::maybe_bridge_standard_sync_json_to_stream"),
        "gateway sync_to_stream.rs should delegate bridge logic through the ai_serving pure seam"
    );
    for forbidden in [
        "fn maybe_bridge_openai_image_sync_json_to_stream(",
        "fn build_terminal_summary_from_openai_responses_response(",
        "fn standardized_usage_from_openai_usage(",
        "OpenAIResponsesProviderState",
    ] {
        assert!(
            !gateway_sync_to_stream.contains(forbidden),
            "gateway sync_to_stream.rs should not own pure sync-to-stream bridge detail {forbidden}"
        );
    }
}

#[test]
fn ai_serving_finalize_stream_rewrite_matrix_is_owned_by_format_crate() {
    assert!(
        workspace_file_exists("crates/aether-ai-formats/src/response/stream_rewrite.rs"),
        "finalize stream rewrite matrix should live in aether-ai-formats"
    );
    assert!(
        workspace_file_exists("crates/aether-ai-formats/src/response/openai_image_stream.rs"),
        "OpenAI image stream rewrite state should live in aether-ai-formats"
    );

    let gateway_stream_rewrite = read_workspace_file(
        "apps/aether-gateway/src/ai_serving/finalize/internal/stream_rewrite.rs",
    );
    assert!(
        gateway_stream_rewrite.contains("maybe_build_ai_surface_stream_rewriter"),
        "gateway internal stream_rewrite should delegate stream rewrite state machine to aether-ai-formats"
    );

    for forbidden in [
        "enum RewriteMode",
        "OpenAiImageStreamState",
        "KiroToClaudeCliStreamState",
        "StreamingStandardConversionState",
        "StreamingStandardFormatMatrix",
        "transform_provider_private_stream_line",
        "resolve_finalize_stream_rewrite_mode",
        "fn transform_standard_bytes(",
        "buffered: Vec<u8>",
        "struct OpenAiImageStreamState",
        "struct OpenAiImageFrame",
        "fn image_failure_error(",
        "fn completed_response_image_result(",
        "fn requested_partial_images(",
        "fn image_partial_event_name(",
        "fn image_completed_event_name(",
        "fn image_failed_event_name(",
        "fn find_sse_block_end(",
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
fn ai_serving_planner_common_parser_is_owned_by_format_crate() {
    assert!(
        workspace_file_exists("crates/aether-ai-formats/src/request/common.rs"),
        "planner/common pure parser should exist in aether-ai-formats"
    );

    let gateway_common =
        read_workspace_file("apps/aether-gateway/src/ai_serving/planner/common.rs");
    let gateway_common_runtime = gateway_common
        .split("#[cfg(test)]")
        .next()
        .unwrap_or(gateway_common.as_str());

    assert!(
        gateway_common_runtime.contains("crate::ai_serving::"),
        "gateway planner/common.rs should delegate body parsing through the ai_serving root seam"
    );
    assert!(
        gateway_common_runtime
            .contains("force_upstream_streaming_for_provider as force_upstream_streaming_for_provider_impl"),
        "gateway planner/common.rs should delegate upstream streaming policy through the ai_serving root seam"
    );
    for forbidden in [
        "serde_json::from_slice::<serde_json::Value>",
        "base64::engine::general_purpose::STANDARD.encode",
        ".eq_ignore_ascii_case(\"codex\")",
        "extract_gemini_model_from_path as extract_gemini_model_from_path_impl",
    ] {
        assert!(
            !gateway_common_runtime.contains(forbidden),
            "gateway planner/common.rs should not own parser implementation detail {forbidden}"
        );
    }
    for pattern in [
        "use aether_ai_serving::AiRequestedModelFamily as RequestedModelFamily;",
        "pub(crate) fn extract_standard_requested_model(",
        "pub(crate) fn extract_requested_model_from_request(",
    ] {
        assert!(
            gateway_common_runtime.contains(pattern),
            "gateway planner/common.rs should own shared planner helper {pattern}"
        );
    }
    for pattern in [
        "extract_ai_standard_requested_model(body_json)",
        "extract_ai_requested_model_from_request_path(",
    ] {
        assert!(
            gateway_common_runtime.contains(pattern),
            "gateway planner/common.rs should delegate requested-model extraction through serving helper {pattern}"
        );
    }
    for forbidden in [
        "pub(crate) fn build_local_runtime_miss_diagnostic(",
        "pub(crate) fn apply_local_candidate_evaluation_progress(",
        "pub(crate) fn apply_local_candidate_terminal_plan_reason(",
        ".get(\"model\")",
        ".and_then(serde_json::Value::as_str)",
    ] {
        assert!(
            !gateway_common_runtime.contains(forbidden),
            "gateway planner/common.rs should not own pure serving helper after extraction {forbidden}"
        );
    }

    for path in [
        "apps/aether-gateway/src/ai_serving/planner/standard/family/build.rs",
        "apps/aether-gateway/src/ai_serving/planner/passthrough/provider/plans.rs",
        "apps/aether-gateway/src/ai_serving/planner/passthrough/provider/family/build.rs",
        "apps/aether-gateway/src/ai_serving/planner/standard/openai/chat/plans/sync.rs",
        "apps/aether-gateway/src/ai_serving/planner/standard/openai/chat/plans/stream.rs",
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
        "apps/aether-gateway/src/ai_serving/planner/standard/openai/chat/plans/diagnostic.rs",
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
fn ai_serving_root_owns_shared_gemini_request_path_parser() {
    let serving_surface_spec = read_workspace_file("crates/aether-ai-serving/src/surface_spec.rs");
    assert!(
        serving_surface_spec.contains("pub fn extract_ai_gemini_model_from_path("),
        "aether-ai-serving should own shared gemini request-path parsing"
    );

    let ai_serving_mod = read_workspace_file("apps/aether-gateway/src/ai_serving/mod.rs");
    assert!(
        ai_serving_mod.contains(
            "extract_ai_gemini_model_from_path as extract_gemini_model_from_path"
        ),
        "ai_serving/mod.rs should expose shared gemini request-path parsing through the serving seam"
    );

    let passthrough_provider_request = read_workspace_file(
        "apps/aether-gateway/src/ai_serving/planner/passthrough/provider/request.rs",
    );
    assert!(
        !passthrough_provider_request.contains("fn extract_gemini_model_from_path("),
        "passthrough/provider/request.rs should not locally own gemini request-path parsing"
    );

    let auth_credentials =
        read_workspace_file("apps/aether-gateway/src/control/auth/credentials.rs");
    assert!(
        auth_credentials.contains("ai_serving::extract_gemini_model_from_path"),
        "control/auth/credentials.rs should use ai_serving root seam for gemini request-path parsing"
    );
    assert!(
        !auth_credentials.contains("fn extract_gemini_model_from_path("),
        "control/auth/credentials.rs should not inline gemini request-path parsing"
    );
}

#[test]
fn ai_serving_planner_standard_normalize_is_owned_by_format_crate() {
    assert!(
        workspace_file_exists("crates/aether-ai-formats/src/request/standard/normalize.rs"),
        "planner/standard/normalize should live in aether-ai-formats"
    );

    let gateway_normalize =
        read_workspace_file("apps/aether-gateway/src/ai_serving/planner/standard/normalize.rs");
    let gateway_normalize_chat = read_workspace_file(
        "apps/aether-gateway/src/ai_serving/planner/standard/normalize/chat.rs",
    );
    let gateway_normalize_cli = read_workspace_file(
        "apps/aether-gateway/src/ai_serving/planner/standard/normalize/responses.rs",
    );
    assert!(
        gateway_normalize_chat.contains("crate::ai_serving::")
            && gateway_normalize_cli.contains("crate::ai_serving::"),
        "gateway normalize chat/cli owners should delegate to format standard normalize helpers through the ai_serving root seam"
    );

    for forbidden in [
        "serde_json::Map::from_iter",
        "normalize_openai_responses_request_to_openai_chat_request",
        "parse_openai_tool_result_content",
    ] {
        assert!(
            !gateway_normalize.contains(forbidden),
            "gateway normalize.rs should not keep helper implementation detail {forbidden}"
        );
    }
    for forbidden in [
        ".eq_ignore_ascii_case(\"antigravity\")",
        "build_antigravity_v1internal_url(",
        "apply_local_body_rules(",
        "request_conversion_kind(",
        "build_provider_transport_request_url(",
        "build_openai_responses_url(",
        "build_openai_chat_url(",
        "build_claude_messages_url(",
        "build_passthrough_path_url(",
    ] {
        assert!(
            !gateway_normalize_cli.contains(forbidden),
            "gateway standard/normalize/responses.rs should route provider-private URL policy through provider-transport instead of {forbidden}"
        );
    }
    for forbidden in [
        "apply_local_body_rules(",
        "request_conversion_kind(",
        "build_provider_transport_request_url(",
        "build_openai_responses_url(",
        "build_openai_chat_url(",
        "build_claude_messages_url(",
        "build_passthrough_path_url(",
    ] {
        assert!(
            !gateway_normalize_chat.contains(forbidden),
            "gateway standard/normalize/chat.rs should route provider URL policy through provider-transport instead of {forbidden}"
        );
    }
}

#[test]
fn ai_serving_openai_helpers_are_owned_by_format_crate() {
    assert!(
        workspace_file_exists("crates/aether-ai-formats/src/request/openai.rs"),
        "planner/openai helper owner should exist in aether-ai-formats"
    );

    let gateway_openai_mod =
        read_workspace_file("apps/aether-gateway/src/ai_serving/planner/standard/openai/mod.rs");
    assert!(
        gateway_openai_mod.contains("pub(crate) use crate::ai_serving::{"),
        "gateway planner/standard/openai/mod.rs should thinly re-export surface openai helpers through the ai_serving root seam"
    );

    let gateway_openai_chat = read_workspace_file(
        "apps/aether-gateway/src/ai_serving/planner/standard/openai/chat/mod.rs",
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
fn ai_serving_standard_matrix_delegates_format_conversion_to_format_crate() {
    assert!(
        workspace_file_exists("crates/aether-ai-formats/src/request/matrix.rs"),
        "planner/matrix facade should live in aether-ai-formats"
    );
    assert!(
        workspace_file_exists("crates/aether-ai-formats/src/request/standard/matrix.rs"),
        "format standard request-body planner should live in aether-ai-formats"
    );
    for path in [
        "crates/aether-ai-formats/src/protocol/canonical.rs",
        "crates/aether-ai-formats/src/protocol/matrix.rs",
        "crates/aether-ai-formats/src/protocol/registry.rs",
    ] {
        assert!(
            workspace_file_exists(path),
            "{path} should own canonical format conversion primitives"
        );
    }
    let surface_matrix =
        read_workspace_file("crates/aether-ai-formats/src/request/standard/matrix.rs");
    assert!(
        surface_matrix.contains("use aether_ai_formats::protocol::registry::{")
            && surface_matrix.contains("convert_request")
            && surface_matrix.contains("FormatContext")
            && surface_matrix.contains("aether_ai_formats::protocol::conversion::request::{"),
        "format standard matrix should delegate format conversion to aether-ai-formats"
    );
    for forbidden in [
        "pub fn convert_request(",
        "pub enum RequestConversionKind",
        "pub struct CanonicalRequest",
    ] {
        assert!(
            !surface_matrix.contains(forbidden),
            "format standard matrix should not own format conversion primitive {forbidden}"
        );
    }
    assert!(
        !workspace_file_exists("apps/aether-gateway/src/ai_serving/planner/standard/matrix.rs"),
        "planner/standard/matrix.rs should stay removed after wrapper cleanup"
    );

    let matrix = read_workspace_file("apps/aether-gateway/src/ai_serving/planner/standard/mod.rs");
    assert!(
        matrix.contains("crate::ai_serving::"),
        "planner/standard/mod.rs should delegate canonical conversion through the ai_serving root seam after matrix wrapper cleanup"
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
fn ai_serving_standard_family_specs_are_owned_by_format_crate() {
    assert!(
        workspace_file_exists("crates/aether-ai-formats/src/request/standard/family.rs"),
        "planner/standard/family pure spec owner should live in aether-ai-formats"
    );
    assert!(
        workspace_file_exists("crates/aether-ai-formats/src/request/standard/claude/chat.rs"),
        "planner/standard/claude/chat pure spec resolver should live in aether-ai-formats"
    );
    assert!(
        workspace_file_exists("crates/aether-ai-formats/src/request/standard/claude/cli.rs"),
        "planner/standard/claude/cli pure spec resolver should live in aether-ai-formats"
    );
    assert!(
        workspace_file_exists("crates/aether-ai-formats/src/request/standard/gemini/chat.rs"),
        "planner/standard/gemini/chat pure spec resolver should live in aether-ai-formats"
    );
    assert!(
        workspace_file_exists("crates/aether-ai-formats/src/request/standard/gemini/cli.rs"),
        "planner/standard/gemini/cli pure spec resolver should live in aether-ai-formats"
    );

    assert!(
        !workspace_file_exists(
            "apps/aether-gateway/src/ai_serving/planner/standard/family/types.rs"
        ),
        "planner/standard/family/types.rs should stay removed after wrapper cleanup"
    );

    let family_types =
        read_workspace_file("apps/aether-gateway/src/ai_serving/planner/standard/family/mod.rs");
    assert!(
        family_types.contains("pub(crate) use crate::ai_serving::{"),
        "gateway planner/standard/family/mod.rs should re-export pure family spec types through the ai_serving root seam"
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
        "apps/aether-gateway/src/ai_serving/planner/standard/claude/chat.rs",
        "apps/aether-gateway/src/ai_serving/planner/standard/claude/cli.rs",
        "apps/aether-gateway/src/ai_serving/planner/standard/gemini/chat.rs",
        "apps/aether-gateway/src/ai_serving/planner/standard/gemini/cli.rs",
    ] {
        assert!(
            !workspace_file_exists(path),
            "{path} should be removed after moving pure spec resolvers into the format crate"
        );
    }

    for path in [
        "apps/aether-gateway/src/ai_serving/planner/standard/claude/mod.rs",
        "apps/aether-gateway/src/ai_serving/planner/standard/gemini/mod.rs",
    ] {
        let source = read_workspace_file(path);
        assert!(
            source.contains("crate::ai_serving::"),
            "{path} should delegate pure standard-family spec resolution through the ai_serving root seam"
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
fn ai_serving_same_format_provider_specs_are_owned_by_format_crate() {
    assert!(
        workspace_file_exists("crates/aether-ai-formats/src/request/passthrough/provider.rs"),
        "planner/passthrough/provider pure spec owner should live in aether-ai-formats"
    );

    assert!(
        !workspace_file_exists(
            "apps/aether-gateway/src/ai_serving/planner/passthrough/provider/family/types.rs"
        ),
        "planner/passthrough/provider/family/types.rs should stay removed after wrapper cleanup"
    );

    let family_types = read_workspace_file(
        "apps/aether-gateway/src/ai_serving/planner/passthrough/provider/family/mod.rs",
    );
    assert!(
        family_types.contains("pub(crate) use crate::ai_serving::"),
        "gateway passthrough/provider/family/mod.rs should re-export pure same-format provider spec types through the ai_serving root seam"
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
        "apps/aether-gateway/src/ai_serving/planner/passthrough/provider/plans.rs",
    );
    assert!(
        plans.contains("crate::ai_serving::"),
        "gateway passthrough/provider/plans.rs should delegate same-format spec resolution through the ai_serving root seam"
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
fn ai_serving_passthrough_provider_specs_are_owned_by_format_crate() {
    assert!(
        workspace_file_exists("crates/aether-ai-formats/src/request/passthrough/provider.rs"),
        "planner/passthrough/provider pure spec owner should live in aether-ai-formats"
    );

    let family_types = read_workspace_file(
        "apps/aether-gateway/src/ai_serving/planner/passthrough/provider/family/mod.rs",
    );
    assert!(
        family_types.contains("pub(crate) use crate::ai_serving::"),
        "gateway passthrough/provider/family/mod.rs should re-export pure spec types through the ai_serving root seam"
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
        "apps/aether-gateway/src/ai_serving/planner/passthrough/provider/plans.rs",
    );
    assert!(
        plans.contains("crate::ai_serving::"),
        "gateway passthrough/provider/plans.rs should delegate same-format spec resolution through the ai_serving root seam"
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
fn ai_serving_specialized_files_specs_are_owned_by_format_crate() {
    assert!(
        workspace_file_exists("crates/aether-ai-formats/src/request/specialized/files.rs"),
        "planner/specialized/files pure spec owner should live in aether-ai-formats"
    );

    let files =
        read_workspace_file("apps/aether-gateway/src/ai_serving/planner/specialized/files.rs");
    assert!(
        files.contains("crate::ai_serving::"),
        "gateway planner/specialized/files.rs should delegate pure specialized-files spec resolution through the ai_serving root seam"
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
fn ai_serving_specialized_video_specs_are_owned_by_format_crate() {
    assert!(
        workspace_file_exists("crates/aether-ai-formats/src/request/specialized/video.rs"),
        "planner/specialized/video pure spec owner should live in aether-ai-formats"
    );

    let video =
        read_workspace_file("apps/aether-gateway/src/ai_serving/planner/specialized/video.rs");
    assert!(
        video.contains("crate::ai_serving::"),
        "gateway planner/specialized/video.rs should delegate pure specialized-video spec resolution through the ai_serving root seam"
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
fn ai_serving_openai_responses_specs_are_owned_by_format_crate() {
    assert!(
        workspace_file_exists("crates/aether-ai-formats/src/request/standard/openai_responses.rs"),
        "planner/standard/openai_responses pure spec owner should live in aether-ai-formats"
    );

    let decision = read_workspace_file(
        "apps/aether-gateway/src/ai_serving/planner/standard/openai/responses/decision.rs",
    );
    assert!(
        decision.contains("pub(super) use crate::ai_serving::LocalOpenAiResponsesSpec;"),
        "gateway planner/standard/openai/responses/decision.rs should re-export pure openai-responses spec type through the ai_serving root seam"
    );
    assert!(
        !decision.contains("pub(super) struct LocalOpenAiResponsesSpec"),
        "gateway planner/standard/openai/responses/decision.rs should not own LocalOpenAiResponsesSpec"
    );

    let plans = read_workspace_file(
        "apps/aether-gateway/src/ai_serving/planner/standard/openai/responses/plans.rs",
    );
    assert!(
        plans.contains("crate::ai_serving::"),
        "gateway planner/standard/openai/responses/plans.rs should delegate openai-responses spec resolution through the ai_serving root seam"
    );
    for forbidden in [
        "fn resolve_sync_spec(",
        "fn resolve_stream_spec(",
        "OPENAI_CLI_SYNC_PLAN_KIND",
        "OPENAI_COMPACT_STREAM_PLAN_KIND",
        "LocalOpenAiResponsesSpec {",
    ] {
        assert!(
            !plans.contains(forbidden),
            "gateway planner/standard/openai/responses/plans.rs should not keep pure openai-responses resolver detail {forbidden}"
        );
    }
}

#[test]
fn ai_serving_legacy_api_format_names_stay_out_of_primary_paths() {
    for path in [
        "crates/aether-ai-formats/src/contracts/plan_kinds.rs",
        "crates/aether-ai-formats/src/request/route.rs",
        "crates/aether-ai-formats/src/request/standard/openai_responses.rs",
        "apps/aether-gateway/src/ai_serving/planner/decision/control_plan.rs",
        "apps/aether-gateway/src/execution_runtime/fallback.rs",
    ] {
        let source = read_workspace_file(path);
        for forbidden in [
            "openai:cli",
            "openai:compact",
            "claude:chat",
            "claude:cli",
            "gemini:chat",
            "gemini:cli",
            "openai_cli_",
            "openai_compact_",
            "OPENAI_CLI",
            "OPENAI_COMPACT",
        ] {
            assert!(
                !source.contains(forbidden),
                "{path} should not emit or branch on legacy OpenAI Responses aliases: {forbidden}"
            );
        }
    }

    let registry = read_workspace_file("crates/aether-ai-formats/src/protocol/registry.rs");
    let implementation = registry
        .split("#[cfg(test)]")
        .next()
        .expect("registry source should have an implementation section");
    for forbidden in [
        "\"openai:cli\"",
        "\"openai:compact\"",
        "\"claude:chat\"",
        "\"claude:cli\"",
        "\"gemini:chat\"",
        "\"gemini:cli\"",
    ] {
        assert!(
            !implementation.contains(forbidden),
            "format conversion registry implementation should not branch on retired API format aliases: {forbidden}"
        );
    }
}

#[test]
fn retired_api_format_occurrences_are_whitelisted() {
    let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root should resolve");
    let mut files = Vec::new();
    for root in ["apps", "crates", "frontend/src"] {
        collect_alias_scan_files(&workspace_root.join(root), &mut files);
    }

    let allowed_paths = [
        "apps/aether-gateway/src/handlers/admin/provider/write/normalize.rs",
        "crates/aether-ai-formats/src/protocol/formats.rs",
        "crates/aether-ai-formats/src/protocol/matrix.rs",
        "crates/aether-ai-formats/src/protocol/registry.rs",
        "crates/aether-data/src/migrate.rs",
        "crates/aether-usage-runtime/src/report.rs",
        "frontend/src/api/endpoints/types/__tests__/api-format.spec.ts",
    ];
    let allowed = allowed_paths
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();
    let patterns = [
        "openai:cli",
        "openai:compact",
        "claude:chat",
        "claude:cli",
        "gemini:chat",
        "gemini:cli",
    ];

    let mut violations = Vec::new();
    for file in files {
        let relative = file
            .strip_prefix(&workspace_root)
            .expect("file should be under workspace root")
            .to_string_lossy()
            .replace('\\', "/");
        if relative == "apps/aether-gateway/src/tests/architecture/ai_serving.rs" {
            continue;
        }

        let source = std::fs::read_to_string(&file).expect("source file should be readable");
        let hits = patterns
            .iter()
            .filter(|pattern| source.contains(**pattern))
            .copied()
            .collect::<Vec<_>>();
        if !hits.is_empty() && !allowed.contains(relative.as_str()) {
            violations.push(format!("{relative} -> {}", hits.join(", ")));
        }
    }

    assert!(
        violations.is_empty(),
        "retired API format aliases should stay confined to migration or negative-test files:\n{}",
        violations.join("\n")
    );
}

fn collect_alias_scan_files(root: &std::path::Path, files: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(root).expect("directory should be readable") {
        let entry = entry.expect("directory entry should be readable");
        let path = entry.path();
        if path.is_dir() {
            let name = path.file_name().and_then(|value| value.to_str());
            if matches!(name, Some("target" | "node_modules" | ".git")) {
                continue;
            }
            collect_alias_scan_files(&path, files);
            continue;
        }

        if matches!(
            path.extension().and_then(|value| value.to_str()),
            Some("rs" | "ts" | "vue")
        ) {
            files.push(path);
        }
    }
}
