use std::collections::BTreeMap;
use std::sync::Arc;

use crate::ai_serving::planner::spec_metadata::local_gemini_files_spec_metadata;
use crate::ai_serving::transport::{
    build_gemini_files_headers, build_gemini_files_request_body, build_gemini_files_upstream_url,
    gemini_files_transport_unsupported_reason, resolve_gemini_files_auth, GeminiFilesHeadersInput,
    GeminiFilesRequestBodyError,
};
use crate::ai_serving::GEMINI_FILES_UPLOAD_PLAN_KIND;
use crate::ai_serving::{
    CandidateFailureDiagnostic, GatewayProviderTransportSnapshot, GEMINI_FILES_DELETE_PLAN_KIND,
    GEMINI_FILES_DOWNLOAD_PLAN_KIND, GEMINI_FILES_GET_PLAN_KIND,
};
use crate::AppState;

use super::support::{
    mark_skipped_local_gemini_files_candidate,
    mark_skipped_local_gemini_files_candidate_with_failure_diagnostic,
    LocalGeminiFilesCandidateAttempt, LocalGeminiFilesDecisionInput,
    GEMINI_FILES_CANDIDATE_API_FORMAT, GEMINI_FILES_CLIENT_API_FORMAT,
};
use super::LocalGeminiFilesSpec;

pub(super) struct LocalGeminiFilesCandidatePayloadParts {
    pub(super) transport: Arc<GatewayProviderTransportSnapshot>,
    pub(super) auth_header: String,
    pub(super) auth_value: String,
    pub(super) provider_request_headers: BTreeMap<String, String>,
    pub(super) provider_request_body: Option<serde_json::Value>,
    pub(super) provider_request_body_base64: Option<String>,
    pub(super) upstream_url: String,
    pub(super) file_name: String,
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn resolve_local_gemini_files_candidate_payload_parts(
    state: &AppState,
    parts: &http::request::Parts,
    body_json: &serde_json::Value,
    body_base64: Option<&str>,
    body_is_empty: bool,
    trace_id: &str,
    input: &LocalGeminiFilesDecisionInput,
    attempt: &LocalGeminiFilesCandidateAttempt,
    spec: LocalGeminiFilesSpec,
) -> Option<LocalGeminiFilesCandidatePayloadParts> {
    let spec_metadata = local_gemini_files_spec_metadata(spec);
    let candidate = &attempt.eligible.candidate;
    let transport = &attempt.eligible.transport;
    let effective_headers = input.effective_headers(&parts.headers);

    if matches!(
        spec_metadata.decision_kind,
        GEMINI_FILES_GET_PLAN_KIND
            | GEMINI_FILES_DELETE_PLAN_KIND
            | GEMINI_FILES_DOWNLOAD_PLAN_KIND
    ) && !candidate_matches_owned_gemini_file_mapping(state, parts, input, attempt).await
    {
        mark_skipped_local_gemini_files_candidate(
            state,
            input,
            trace_id,
            candidate,
            attempt.candidate_index,
            &attempt.candidate_id,
            "gemini_file_mapping_mismatch",
        )
        .await;
        return None;
    }

    if let Some(skip_reason) =
        gemini_files_transport_unsupported_reason(transport, GEMINI_FILES_CANDIDATE_API_FORMAT)
    {
        mark_skipped_local_gemini_files_candidate(
            state,
            input,
            trace_id,
            candidate,
            attempt.candidate_index,
            &attempt.candidate_id,
            skip_reason,
        )
        .await;
        return None;
    }

    let Some((auth_header, auth_value)) = resolve_gemini_files_auth(transport) else {
        mark_skipped_local_gemini_files_candidate(
            state,
            input,
            trace_id,
            candidate,
            attempt.candidate_index,
            &attempt.candidate_id,
            "transport_auth_unavailable",
        )
        .await;
        return None;
    };

    let Some(upstream_url) =
        build_gemini_files_upstream_url(transport, parts.uri.path(), parts.uri.query())
    else {
        mark_skipped_local_gemini_files_candidate_with_failure_diagnostic(
            state,
            input,
            trace_id,
            candidate,
            attempt.candidate_index,
            &attempt.candidate_id,
            "upstream_url_missing",
            CandidateFailureDiagnostic::upstream_url_missing(
                GEMINI_FILES_CLIENT_API_FORMAT,
                GEMINI_FILES_CANDIDATE_API_FORMAT,
                "gemini_files_passthrough_url",
            ),
        )
        .await;
        return None;
    };

    let body_parts = match build_gemini_files_request_body(
        body_json,
        body_base64,
        body_is_empty,
        spec_metadata.decision_kind == GEMINI_FILES_UPLOAD_PLAN_KIND,
        transport.endpoint.body_rules.as_ref(),
        Some(effective_headers),
    ) {
        Ok(parts) => parts,
        Err(GeminiFilesRequestBodyError::BodyRulesUnsupportedForBinaryUpload) => {
            mark_skipped_local_gemini_files_candidate_with_failure_diagnostic(
                state,
                input,
                trace_id,
                candidate,
                attempt.candidate_index,
                &attempt.candidate_id,
                "transport_body_rules_unsupported_for_binary_upload",
                CandidateFailureDiagnostic::body_rules_unsupported_for_binary_upload(
                    GEMINI_FILES_CLIENT_API_FORMAT,
                    GEMINI_FILES_CANDIDATE_API_FORMAT,
                    "gemini_files_binary_upload",
                ),
            )
            .await;
            return None;
        }
        Err(GeminiFilesRequestBodyError::BodyRulesApplyFailed) => {
            mark_skipped_local_gemini_files_candidate_with_failure_diagnostic(
                state,
                input,
                trace_id,
                candidate,
                attempt.candidate_index,
                &attempt.candidate_id,
                "transport_body_rules_apply_failed",
                CandidateFailureDiagnostic::body_rules_apply_failed(
                    GEMINI_FILES_CLIENT_API_FORMAT,
                    GEMINI_FILES_CANDIDATE_API_FORMAT,
                    "gemini_files_body_rules",
                ),
            )
            .await;
            return None;
        }
    };

    let Some(provider_request_headers) = build_gemini_files_headers(GeminiFilesHeadersInput {
        headers: effective_headers,
        auth_header: &auth_header,
        auth_value: &auth_value,
        header_rules: transport.endpoint.header_rules.as_ref(),
        provider_request_body: body_parts.provider_request_body.as_ref(),
        provider_request_body_base64: body_parts.provider_request_body_base64.as_deref(),
        original_request_body_json: body_json,
        original_body_is_empty: body_is_empty,
    }) else {
        mark_skipped_local_gemini_files_candidate_with_failure_diagnostic(
            state,
            input,
            trace_id,
            candidate,
            attempt.candidate_index,
            &attempt.candidate_id,
            "transport_header_rules_apply_failed",
            CandidateFailureDiagnostic::header_rules_apply_failed(
                GEMINI_FILES_CLIENT_API_FORMAT,
                GEMINI_FILES_CANDIDATE_API_FORMAT,
                "gemini_files_header_rules",
            ),
        )
        .await;
        return None;
    };

    let file_name = parts
        .uri
        .path()
        .trim_start_matches("/v1beta/")
        .trim()
        .to_string();

    Some(LocalGeminiFilesCandidatePayloadParts {
        transport: Arc::clone(transport),
        auth_header,
        auth_value,
        provider_request_headers,
        provider_request_body: body_parts.provider_request_body,
        provider_request_body_base64: body_parts.provider_request_body_base64,
        upstream_url,
        file_name,
    })
}

async fn candidate_matches_owned_gemini_file_mapping(
    state: &AppState,
    parts: &http::request::Parts,
    input: &LocalGeminiFilesDecisionInput,
    attempt: &LocalGeminiFilesCandidateAttempt,
) -> bool {
    let Some(file_name) = normalize_gemini_file_name_from_path(parts.uri.path()) else {
        return false;
    };
    let user_id = input.auth_context.user_id.trim();
    if user_id.is_empty() || !state.has_gemini_file_mapping_data_reader() {
        return false;
    }
    let Ok(Some(mapping)) = state
        .find_active_gemini_file_mapping_for_owner(
            file_name.as_str(),
            &attempt.eligible.transport.key.id,
            user_id,
            crate::clock::current_unix_secs(),
        )
        .await
    else {
        return false;
    };

    mapping.user_id.as_deref().map(str::trim) == Some(user_id)
        && mapping.key_id == attempt.eligible.transport.key.id
}

pub(crate) fn normalize_gemini_file_name_from_path(path: &str) -> Option<String> {
    let suffix = path.strip_prefix("/v1beta/files/")?.trim_matches('/');
    let suffix = suffix.strip_suffix(":download").unwrap_or(suffix).trim();
    let suffix = suffix.strip_prefix("files/").unwrap_or(suffix).trim();
    if suffix.is_empty() || suffix.contains('/') {
        return None;
    }
    Some(format!("files/{suffix}"))
}

#[cfg(test)]
mod tests {
    use super::normalize_gemini_file_name_from_path;

    #[test]
    fn normalizes_supported_gemini_file_object_paths() {
        assert_eq!(
            normalize_gemini_file_name_from_path("/v1beta/files/file-123"),
            Some("files/file-123".to_string())
        );
        assert_eq!(
            normalize_gemini_file_name_from_path("/v1beta/files/file-123:download"),
            Some("files/file-123".to_string())
        );
        assert_eq!(
            normalize_gemini_file_name_from_path("/v1beta/files/files/abc-123"),
            Some("files/abc-123".to_string())
        );
        assert_eq!(normalize_gemini_file_name_from_path("/v1beta/files"), None);
    }
}
