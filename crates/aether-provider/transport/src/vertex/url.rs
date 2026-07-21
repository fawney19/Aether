use std::collections::BTreeMap;

use url::form_urlencoded;

use super::super::url::build_passthrough_path_url;
use super::auth::VertexServiceAccountAuthConfig;

pub const VERTEX_API_KEY_BASE_URL: &str = "https://aiplatform.googleapis.com";

pub fn build_vertex_api_key_gemini_content_url(
    model: &str,
    stream: bool,
    api_key: &str,
    request_query: Option<&str>,
) -> Option<String> {
    let action = if stream {
        "streamGenerateContent"
    } else {
        "generateContent"
    };
    build_vertex_api_key_google_model_url(model, action, stream, api_key, request_query)
}

pub fn build_vertex_api_key_imagen_content_url(
    model: &str,
    stream: bool,
    api_key: &str,
    request_query: Option<&str>,
) -> Option<String> {
    let action = if stream {
        "streamGenerateContent"
    } else {
        "generateContent"
    };
    build_vertex_api_key_google_model_url(model, action, stream, api_key, request_query)
}

pub fn build_vertex_api_key_gemini_embedding_url(
    model: &str,
    api_key: &str,
    request_query: Option<&str>,
) -> Option<String> {
    build_vertex_api_key_google_model_url(model, "predict", false, api_key, request_query)
}

pub fn build_vertex_service_account_gemini_content_url(
    model: &str,
    stream: bool,
    auth_config: &VertexServiceAccountAuthConfig,
    request_query: Option<&str>,
) -> Option<String> {
    let action = if stream {
        "streamGenerateContent"
    } else {
        "generateContent"
    };
    build_vertex_service_account_google_model_url(model, action, stream, auth_config, request_query)
}

pub fn build_vertex_service_account_gemini_embedding_url(
    model: &str,
    auth_config: &VertexServiceAccountAuthConfig,
    request_query: Option<&str>,
) -> Option<String> {
    build_vertex_service_account_google_model_url(
        model,
        "predict",
        false,
        auth_config,
        request_query,
    )
}

fn build_vertex_api_key_google_model_url(
    model: &str,
    action: &str,
    stream: bool,
    api_key: &str,
    request_query: Option<&str>,
) -> Option<String> {
    let trimmed_model = model.trim();
    let trimmed_action = action.trim();
    let trimmed_api_key = api_key.trim();
    if trimmed_model.is_empty() || trimmed_action.is_empty() || trimmed_api_key.is_empty() {
        return None;
    }

    let path = format!("/v1/publishers/google/models/{trimmed_model}:{trimmed_action}");
    let merged_query = build_vertex_api_key_query(trimmed_api_key, request_query, stream);
    build_passthrough_path_url(VERTEX_API_KEY_BASE_URL, &path, merged_query.as_deref(), &[])
}

fn build_vertex_service_account_google_model_url(
    model: &str,
    action: &str,
    stream: bool,
    auth_config: &VertexServiceAccountAuthConfig,
    request_query: Option<&str>,
) -> Option<String> {
    let trimmed_model = model.trim();
    let trimmed_action = action.trim();
    let project_id = auth_config.project_id.trim();
    if trimmed_model.is_empty() || trimmed_action.is_empty() || project_id.is_empty() {
        return None;
    }

    let region = resolve_vertex_service_account_region(trimmed_model, auth_config);
    let base_url = if region == "global" {
        VERTEX_API_KEY_BASE_URL.to_string()
    } else {
        format!("https://{region}-aiplatform.googleapis.com")
    };
    let path = format!(
        "/v1/projects/{project_id}/locations/{region}/publishers/google/models/{trimmed_model}:{trimmed_action}"
    );
    let merged_query = build_vertex_service_account_query(request_query, stream);
    build_passthrough_path_url(&base_url, &path, merged_query.as_deref(), &[])
}

pub fn resolve_vertex_service_account_region(
    model: &str,
    auth_config: &VertexServiceAccountAuthConfig,
) -> String {
    let trimmed_model = model.trim();
    if let Some(region) = auth_config
        .model_regions
        .get(trimmed_model)
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return region.to_string();
    }
    if let Some(region) = default_vertex_model_region(trimmed_model) {
        return region.to_string();
    }
    if let Some(region) = auth_config
        .region
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return region.to_string();
    }
    "global".to_string()
}

fn default_vertex_model_region(model: &str) -> Option<&'static str> {
    if model.starts_with("gemini-3.") || model == "gemini-3-pro-image-preview" {
        return Some("global");
    }
    if matches!(
        model,
        "gemini-2.0-flash"
            | "gemini-2.0-flash-exp"
            | "gemini-2.0-flash-001"
            | "gemini-2.0-pro-exp"
            | "gemini-2.0-flash-exp-image-generation"
            | "gemini-1.5-pro"
            | "gemini-1.5-pro-001"
            | "gemini-1.5-pro-002"
            | "gemini-1.5-flash"
            | "gemini-1.5-flash-001"
            | "gemini-1.5-flash-002"
            | "imagen-3.0-generate-001"
            | "imagen-3.0-fast-generate-001"
    ) {
        return Some("us-central1");
    }
    None
}

fn build_vertex_api_key_query(
    api_key: &str,
    request_query: Option<&str>,
    stream: bool,
) -> Option<String> {
    let mut merged = BTreeMap::new();
    merge_query_string(&mut merged, request_query);
    merged.remove("beta");
    merged.insert("key".to_string(), api_key.to_string());
    if stream {
        merged
            .entry("alt".to_string())
            .or_insert_with(|| "sse".to_string());
    }

    let mut serializer = form_urlencoded::Serializer::new(String::new());
    for (key, value) in merged {
        serializer.append_pair(&key, &value);
    }
    let query = serializer.finish();
    if query.is_empty() {
        None
    } else {
        Some(query)
    }
}

fn build_vertex_service_account_query(request_query: Option<&str>, stream: bool) -> Option<String> {
    let mut merged = BTreeMap::new();
    merge_query_string(&mut merged, request_query);
    merged.remove("beta");
    merged.remove("key");
    if stream {
        merged
            .entry("alt".to_string())
            .or_insert_with(|| "sse".to_string());
    }

    let mut serializer = form_urlencoded::Serializer::new(String::new());
    for (key, value) in merged {
        serializer.append_pair(&key, &value);
    }
    let query = serializer.finish();
    if query.is_empty() {
        None
    } else {
        Some(query)
    }
}

fn merge_query_string(out: &mut BTreeMap<String, String>, query: Option<&str>) {
    let Some(query) = query.map(str::trim).filter(|value| !value.is_empty()) else {
        return;
    };

    for (key, value) in form_urlencoded::parse(query.as_bytes()) {
        out.insert(key.into_owned(), value.into_owned());
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{
        build_vertex_api_key_gemini_content_url, build_vertex_api_key_imagen_content_url,
        build_vertex_service_account_gemini_content_url,
    };
    use crate::vertex::VertexServiceAccountAuthConfig;

    #[test]
    fn builds_vertex_gemini_api_key_stream_url() {
        assert_eq!(
            build_vertex_api_key_gemini_content_url(
                "gemini-2.5-pro",
                true,
                "vertex-secret",
                Some("foo=bar&beta=v1")
            )
            .as_deref(),
            Some(
                "https://aiplatform.googleapis.com/v1/publishers/google/models/gemini-2.5-pro:streamGenerateContent?alt=sse&foo=bar&key=vertex-secret"
            )
        );
    }

    #[test]
    fn builds_vertex_imagen_api_key_sync_url() {
        assert_eq!(
            build_vertex_api_key_imagen_content_url(
                "imagen-3.0-generate-001",
                false,
                "vertex-secret",
                Some("view=full")
            )
            .as_deref(),
            Some(
                "https://aiplatform.googleapis.com/v1/publishers/google/models/imagen-3.0-generate-001:generateContent?key=vertex-secret&view=full"
            )
        );
    }

    #[test]
    fn builds_vertex_service_account_gemini_sync_url() {
        let auth_config = VertexServiceAccountAuthConfig {
            client_email: "svc@example.iam.gserviceaccount.com".to_string(),
            private_key: "not-used".to_string(),
            project_id: "demo-project".to_string(),
            token_uri: "https://oauth2.googleapis.com/token".to_string(),
            region: None,
            model_regions: BTreeMap::new(),
        };

        assert_eq!(
            build_vertex_service_account_gemini_content_url(
                "gemini-3.1-pro-preview",
                false,
                &auth_config,
                Some("foo=bar&beta=1&key=client-key")
            )
            .as_deref(),
            Some(
                "https://aiplatform.googleapis.com/v1/projects/demo-project/locations/global/publishers/google/models/gemini-3.1-pro-preview:generateContent?foo=bar"
            )
        );
    }

    #[test]
    fn builds_vertex_service_account_gemini_stream_url_with_model_region_override() {
        let auth_config = VertexServiceAccountAuthConfig {
            client_email: "svc@example.iam.gserviceaccount.com".to_string(),
            private_key: "not-used".to_string(),
            project_id: "demo-project".to_string(),
            token_uri: "https://oauth2.googleapis.com/token".to_string(),
            region: Some("global".to_string()),
            model_regions: BTreeMap::from([(
                "gemini-2.0-flash".to_string(),
                "us-central1".to_string(),
            )]),
        };

        assert_eq!(
            build_vertex_service_account_gemini_content_url(
                "gemini-2.0-flash",
                true,
                &auth_config,
                Some("foo=bar")
            )
            .as_deref(),
            Some(
                "https://us-central1-aiplatform.googleapis.com/v1/projects/demo-project/locations/us-central1/publishers/google/models/gemini-2.0-flash:streamGenerateContent?alt=sse&foo=bar"
            )
        );
    }
}
