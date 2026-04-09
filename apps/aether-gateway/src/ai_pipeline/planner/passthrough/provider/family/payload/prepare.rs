use tracing::warn;

use crate::ai_pipeline::transport::auth::{resolve_local_gemini_auth, resolve_local_standard_auth};
use crate::ai_pipeline::transport::claude_code::supports_local_claude_code_transport_with_network;
use crate::ai_pipeline::transport::kiro::{
    supports_local_kiro_request_transport_with_network, KiroRequestAuth,
};
use crate::ai_pipeline::transport::policy::{
    supports_local_gemini_transport_with_network, supports_local_standard_transport_with_network,
};
use crate::ai_pipeline::transport::vertex::{
    resolve_local_vertex_api_key_query_auth,
    supports_local_vertex_api_key_gemini_transport_with_network,
};
use crate::ai_pipeline::{
    GatewayProviderTransportSnapshot, LocalResolvedOAuthRequestAuth, PlannerAppState,
};
use crate::AppState;

use super::super::{
    LocalSameFormatProviderDecisionInput, LocalSameFormatProviderFamily,
    LocalSameFormatProviderSpec,
};

pub(super) struct PreparedSameFormatProviderCandidate {
    pub(super) transport: GatewayProviderTransportSnapshot,
    pub(super) is_antigravity: bool,
    pub(super) is_claude_code: bool,
    pub(super) is_vertex: bool,
    pub(super) is_kiro: bool,
    pub(super) kiro_auth: Option<KiroRequestAuth>,
    pub(super) auth_header: Option<String>,
    pub(super) auth_value: Option<String>,
    pub(super) mapped_model: String,
    pub(super) report_kind: &'static str,
    pub(super) upstream_is_stream: bool,
}

pub(super) async fn prepare_local_same_format_provider_candidate(
    state: &AppState,
    trace_id: &str,
    input: &LocalSameFormatProviderDecisionInput,
    candidate: &aether_scheduler_core::SchedulerMinimalCandidateSelectionCandidate,
    candidate_index: u32,
    candidate_id: &str,
    spec: LocalSameFormatProviderSpec,
) -> Option<PreparedSameFormatProviderCandidate> {
    let planner_state = PlannerAppState::new(state);
    let transport = match planner_state
        .read_provider_transport_snapshot(
            &candidate.provider_id,
            &candidate.endpoint_id,
            &candidate.key_id,
        )
        .await
    {
        Ok(Some(snapshot)) => snapshot,
        Ok(None) => {
            super::mark_skipped_local_same_format_provider_candidate(
                state,
                input,
                trace_id,
                candidate,
                candidate_index,
                candidate_id,
                "transport_snapshot_missing",
            )
            .await;
            return None;
        }
        Err(err) => {
            warn!(
                trace_id = %trace_id,
                api_format = spec.api_format,
                error = ?err,
                "gateway local same-format decision provider transport read failed"
            );
            super::mark_skipped_local_same_format_provider_candidate(
                state,
                input,
                trace_id,
                candidate,
                candidate_index,
                candidate_id,
                "transport_snapshot_read_failed",
            )
            .await;
            return None;
        }
    };

    let is_antigravity = transport
        .provider
        .provider_type
        .trim()
        .eq_ignore_ascii_case("antigravity");
    let is_claude_code = transport
        .provider
        .provider_type
        .trim()
        .eq_ignore_ascii_case("claude_code");
    let is_vertex = transport
        .provider
        .provider_type
        .trim()
        .eq_ignore_ascii_case("vertex_ai");
    let is_kiro = transport
        .provider
        .provider_type
        .trim()
        .eq_ignore_ascii_case("kiro");
    let transport_supported = if is_kiro {
        supports_local_kiro_request_transport_with_network(&transport)
    } else if is_antigravity {
        true
    } else if is_claude_code {
        supports_local_claude_code_transport_with_network(&transport, spec.api_format)
    } else if is_vertex {
        supports_local_vertex_api_key_gemini_transport_with_network(&transport)
    } else {
        match spec.family {
            LocalSameFormatProviderFamily::Standard => {
                supports_local_standard_transport_with_network(&transport, spec.api_format)
            }
            LocalSameFormatProviderFamily::Gemini => {
                supports_local_gemini_transport_with_network(&transport, spec.api_format)
            }
        }
    };
    if !transport_supported {
        super::mark_skipped_local_same_format_provider_candidate(
            state,
            input,
            trace_id,
            candidate,
            candidate_index,
            candidate_id,
            "transport_unsupported",
        )
        .await;
        return None;
    }

    let vertex_query_auth = if is_vertex {
        resolve_local_vertex_api_key_query_auth(&transport)
    } else {
        None
    };
    let should_try_oauth_auth = is_kiro
        || matches!(spec.family, LocalSameFormatProviderFamily::Standard)
            && resolve_local_standard_auth(&transport).is_none()
        || matches!(spec.family, LocalSameFormatProviderFamily::Gemini)
            && !is_vertex
            && resolve_local_gemini_auth(&transport).is_none();
    let oauth_auth = if should_try_oauth_auth {
        match planner_state
            .resolve_local_oauth_request_auth(&transport)
            .await
        {
            Ok(Some(LocalResolvedOAuthRequestAuth::Kiro(auth))) => {
                Some(LocalResolvedOAuthRequestAuth::Kiro(auth))
            }
            Ok(Some(LocalResolvedOAuthRequestAuth::Header { name, value })) => {
                Some(LocalResolvedOAuthRequestAuth::Header { name, value })
            }
            Ok(None) => None,
            Err(err) => {
                warn!(
                    trace_id = %trace_id,
                    api_format = spec.api_format,
                    provider_type = %transport.provider.provider_type,
                    error = ?err,
                    "gateway local same-format oauth auth resolution failed"
                );
                None
            }
        }
    } else {
        None
    };
    let kiro_auth = match oauth_auth.as_ref() {
        Some(LocalResolvedOAuthRequestAuth::Kiro(auth)) => Some(auth.clone()),
        _ => None,
    };
    let auth = if let Some(kiro_auth) = kiro_auth.as_ref() {
        Some((kiro_auth.name.to_string(), kiro_auth.value.clone()))
    } else if let Some(LocalResolvedOAuthRequestAuth::Header { name, value }) = oauth_auth.as_ref()
    {
        Some((name.clone(), value.clone()))
    } else if is_vertex {
        None
    } else {
        match spec.family {
            LocalSameFormatProviderFamily::Standard => resolve_local_standard_auth(&transport),
            LocalSameFormatProviderFamily::Gemini => resolve_local_gemini_auth(&transport),
        }
    };
    let (auth_header, auth_value) = match auth {
        Some((name, value)) => (Some(name), Some(value)),
        None if is_vertex && vertex_query_auth.is_some() => (None, None),
        None => {
            super::mark_skipped_local_same_format_provider_candidate(
                state,
                input,
                trace_id,
                candidate,
                candidate_index,
                candidate_id,
                "transport_auth_unavailable",
            )
            .await;
            return None;
        }
    };
    if is_vertex && vertex_query_auth.is_none() {
        super::mark_skipped_local_same_format_provider_candidate(
            state,
            input,
            trace_id,
            candidate,
            candidate_index,
            candidate_id,
            "transport_auth_unavailable",
        )
        .await;
        return None;
    }

    let mapped_model = candidate.selected_provider_model_name.trim().to_string();
    if mapped_model.is_empty() {
        super::mark_skipped_local_same_format_provider_candidate(
            state,
            input,
            trace_id,
            candidate,
            candidate_index,
            candidate_id,
            "mapped_model_missing",
        )
        .await;
        return None;
    }

    let upstream_is_stream = is_kiro || is_antigravity || spec.require_streaming;
    let report_kind = if is_kiro && !spec.require_streaming {
        "claude_cli_sync_finalize"
    } else if is_antigravity && !spec.require_streaming {
        match spec.api_format {
            "gemini:chat" => "gemini_chat_sync_finalize",
            "gemini:cli" => "gemini_cli_sync_finalize",
            _ => spec.report_kind,
        }
    } else {
        spec.report_kind
    };

    Some(PreparedSameFormatProviderCandidate {
        transport,
        is_antigravity,
        is_claude_code,
        is_vertex,
        is_kiro,
        kiro_auth,
        auth_header,
        auth_value,
        mapped_model,
        report_kind,
        upstream_is_stream,
    })
}
