use super::*;

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
