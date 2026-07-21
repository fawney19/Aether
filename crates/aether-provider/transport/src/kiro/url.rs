use super::super::url::build_passthrough_path_url;
use super::credentials::DEFAULT_REGION;

pub const GENERATE_ASSISTANT_RESPONSE_PATH: &str = "/generateAssistantResponse";
pub const LIST_AVAILABLE_MODELS_PATH: &str = "/ListAvailableModels";
pub const MCP_PATH: &str = "/mcp";
pub const MCP_STREAM_PATH: &str = "/mcp/stream";
pub const KIRO_ENVELOPE_NAME: &str = "kiro:generateAssistantResponse";

pub fn resolve_kiro_base_url(upstream_base_url: &str, api_region: Option<&str>) -> String {
    let region = api_region
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_REGION);
    upstream_base_url
        .trim()
        .replace("{region}", region)
        .trim_end_matches('/')
        .to_string()
}

pub fn build_kiro_generate_assistant_response_url(
    upstream_base_url: &str,
    query: Option<&str>,
    api_region: Option<&str>,
) -> Option<String> {
    let upstream_base_url = resolve_kiro_base_url(upstream_base_url, api_region);
    build_passthrough_path_url(
        upstream_base_url.as_str(),
        GENERATE_ASSISTANT_RESPONSE_PATH,
        query,
        &[],
    )
}

pub fn build_kiro_mcp_url(upstream_base_url: &str, api_region: Option<&str>) -> Option<String> {
    let upstream_base_url = resolve_kiro_base_url(upstream_base_url, api_region);
    build_passthrough_path_url(upstream_base_url.as_str(), MCP_PATH, None, &[])
}

pub fn build_kiro_list_available_models_url(
    upstream_base_url: &str,
    api_region: Option<&str>,
) -> Option<String> {
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    serializer.append_pair("origin", "AI_EDITOR");
    let query = serializer.finish();
    let upstream_base_url = resolve_kiro_base_url(upstream_base_url, api_region);
    build_passthrough_path_url(
        upstream_base_url.as_str(),
        LIST_AVAILABLE_MODELS_PATH,
        Some(&query),
        &[],
    )
}

pub fn build_kiro_mcp_url_from_resolved_url(resolved_url: &str) -> Option<String> {
    let mut parsed = url::Url::parse(resolved_url).ok()?;
    parsed.set_path(MCP_PATH);
    parsed.set_query(None);
    Some(parsed.to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        build_kiro_generate_assistant_response_url, build_kiro_list_available_models_url,
        build_kiro_mcp_url, build_kiro_mcp_url_from_resolved_url, resolve_kiro_base_url,
        GENERATE_ASSISTANT_RESPONSE_PATH, KIRO_ENVELOPE_NAME, LIST_AVAILABLE_MODELS_PATH, MCP_PATH,
        MCP_STREAM_PATH,
    };

    #[test]
    fn exposes_kiro_request_constants() {
        assert_eq!(
            GENERATE_ASSISTANT_RESPONSE_PATH,
            "/generateAssistantResponse"
        );
        assert_eq!(LIST_AVAILABLE_MODELS_PATH, "/ListAvailableModels");
        assert_eq!(MCP_PATH, "/mcp");
        assert_eq!(MCP_STREAM_PATH, "/mcp/stream");
        assert_eq!(KIRO_ENVELOPE_NAME, "kiro:generateAssistantResponse");
    }

    #[test]
    fn builds_generate_assistant_response_url() {
        assert_eq!(
            build_kiro_generate_assistant_response_url(
                "https://kiro.{region}.example?tenant=demo",
                Some("stream=true"),
                Some("us-west-2")
            )
            .as_deref(),
            Some(
                "https://kiro.us-west-2.example/generateAssistantResponse?stream=true&tenant=demo"
            )
        );
    }

    #[test]
    fn resolves_region_placeholder_in_base_url() {
        assert_eq!(
            resolve_kiro_base_url("https://kiro.{region}.example/", Some("us-west-2")),
            "https://kiro.us-west-2.example"
        );
    }

    #[test]
    fn builds_mcp_url_for_latest_kiro_endpoint() {
        assert_eq!(
            build_kiro_mcp_url("https://q.{region}.amazonaws.com", Some("eu-west-1")).as_deref(),
            Some("https://q.eu-west-1.amazonaws.com/mcp")
        );
    }

    #[test]
    fn builds_list_available_models_url_for_profile() {
        assert_eq!(
            build_kiro_list_available_models_url(
                "https://q.{region}.amazonaws.com",
                Some("us-west-2")
            )
            .as_deref(),
            Some("https://q.us-west-2.amazonaws.com/ListAvailableModels?origin=AI_EDITOR")
        );
    }

    #[test]
    fn rewrites_generate_assistant_url_to_mcp_url() {
        assert_eq!(
            build_kiro_mcp_url_from_resolved_url(
                "https://q.us-east-1.amazonaws.com/generateAssistantResponse?beta=true"
            )
            .as_deref(),
            Some("https://q.us-east-1.amazonaws.com/mcp")
        );
    }
}
