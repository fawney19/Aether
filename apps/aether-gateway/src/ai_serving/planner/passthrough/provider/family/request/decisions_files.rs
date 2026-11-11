use serde_json::Value;

use crate::{AppState, GatewayError};

pub(super) async fn candidate_matches_decisions_file_mappings(
    state: &AppState,
    body: &Value,
    user_id: &str,
    key_id: &str,
) -> Result<bool, GatewayError> {
    let Some(input) = body.get("input") else {
        return Ok(true);
    };
    if !state.has_gemini_file_mapping_data_reader() {
        return Ok(true);
    }
    let contents = input
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_else(|| std::slice::from_ref(input));
    for content in contents {
        let Some(file_name) = content
            .get("uri")
            .and_then(Value::as_str)
            .and_then(google_file_name)
        else {
            continue;
        };
        // Uploaded files belong to the account recorded by the Files API. External
        // URLs and files uploaded outside this gateway retain normal passthrough.
        if let Some(mapping) = state
            .find_active_gemini_file_mapping_for_user(
                &file_name,
                user_id,
                crate::clock::current_unix_secs(),
            )
            .await?
        {
            if mapping.key_id != key_id {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

fn google_file_name(uri: &str) -> Option<String> {
    let url = url::Url::parse(uri).ok()?;
    if url.scheme() != "https" || url.host_str() != Some("generativelanguage.googleapis.com") {
        return None;
    }
    let id = url.path().strip_prefix("/v1beta/files/")?;
    (!id.is_empty() && !id.contains('/')).then(|| format!("files/{id}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_data::repository::{
        candidates::InMemoryRequestCandidateRepository,
        gemini_file_mappings::{InMemoryGeminiFileMappingRepository, StoredGeminiFileMapping},
    };
    use serde_json::json;
    use std::sync::Arc;

    fn file_state() -> AppState {
        let mappings = Arc::new(InMemoryGeminiFileMappingRepository::seed([
            StoredGeminiFileMapping {
                id: "mapping-decisions".into(),
                file_name: "files/document-1".into(),
                key_id: "upload-account".into(),
                user_id: Some("file-user".into()),
                display_name: None,
                mime_type: Some("application/pdf".into()),
                source_hash: None,
                created_at_unix_ms: 1,
                expires_at_unix_secs: u64::MAX,
            },
        ]));
        AppState::new().expect("test state should build").with_data_state_for_tests(
            crate::data::GatewayDataState::with_request_candidate_and_gemini_file_mapping_repository_for_tests(
                Arc::new(InMemoryRequestCandidateRepository::default()), mappings,
            ),
        )
    }

    #[tokio::test]
    async fn decisions_files_select_upload_account_for_object_and_array_input() {
        let state = file_state();
        let content = json!({"type":"document","uri":"https://generativelanguage.googleapis.com/v1beta/files/document-1","mime_type":"application/pdf"});
        for input in [
            content.clone(),
            json!([{"type":"text","text":"Review"},content]),
        ] {
            let body = json!({"input":input});
            assert!(candidate_matches_decisions_file_mappings(
                &state,
                &body,
                "file-user",
                "upload-account"
            )
            .await
            .unwrap());
            assert!(!candidate_matches_decisions_file_mappings(
                &state,
                &body,
                "file-user",
                "other-account"
            )
            .await
            .unwrap());
        }
    }

    #[tokio::test]
    async fn decisions_files_preserve_external_and_unmapped_uri_passthrough() {
        let state = file_state();
        for uri in [
            "https://example.com/document.pdf",
            "https://generativelanguage.googleapis.com/v1beta/files/external-upload",
        ] {
            assert!(candidate_matches_decisions_file_mappings(
                &state,
                &json!({"input":{"type":"document","uri":uri}}),
                "file-user",
                "other-account"
            )
            .await
            .unwrap());
        }
    }

    #[test]
    fn decisions_files_only_recognize_google_files_resource_uris() {
        assert_eq!(
            google_file_name("https://generativelanguage.googleapis.com/v1beta/files/document-1"),
            Some("files/document-1".into())
        );
        for uri in [
            "https://example.com/v1beta/files/document-1",
            "https://generativelanguage.googleapis.com/v1beta/files/",
            "https://generativelanguage.googleapis.com/v1beta/files/a/b",
        ] {
            assert_eq!(google_file_name(uri), None);
        }
    }
}
