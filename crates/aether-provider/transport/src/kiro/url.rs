use super::super::url::build_passthrough_path_url;
use super::credentials::DEFAULT_REGION;

pub const KIRO_ENVELOPE_NAME: &str = "kiro:generateAssistantResponse";

pub fn resolve_kiro_base_url(api_region: Option<&str>) -> String {
    let region = api_region
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_REGION);
    // Kiro 固定提供商仅支持新版 Runtime；端点由认证区域派生，无需迁移历史数据库记录。
    format!("https://runtime.{region}.kiro.dev")
}

pub fn build_kiro_generate_assistant_response_url(
    query: Option<&str>,
    api_region: Option<&str>,
) -> Option<String> {
    let runtime_base_url = resolve_kiro_base_url(api_region);
    build_passthrough_path_url(runtime_base_url.as_str(), "/", query, &[])
}

pub fn build_kiro_mcp_url(api_region: Option<&str>) -> Option<String> {
    let runtime_base_url = resolve_kiro_base_url(api_region);
    build_passthrough_path_url(runtime_base_url.as_str(), "/", None, &[])
}

pub fn build_kiro_list_available_models_url(api_region: Option<&str>) -> Option<String> {
    let region = api_region
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_REGION);
    url::Url::parse(&format!("https://management.{region}.kiro.dev"))
        .ok()
        .map(|url| url.to_string())
}

pub fn build_kiro_mcp_url_from_resolved_url(resolved_url: &str) -> Option<String> {
    let parsed = url::Url::parse(resolved_url).ok()?;
    let host = parsed.host_str()?;
    if !(host.starts_with("runtime.") && host.ends_with(".kiro.dev")) {
        return None;
    }
    Some(parsed.origin().ascii_serialization())
}

#[cfg(test)]
mod tests {
    use super::{
        build_kiro_generate_assistant_response_url, build_kiro_list_available_models_url,
        build_kiro_mcp_url, build_kiro_mcp_url_from_resolved_url, resolve_kiro_base_url,
        KIRO_ENVELOPE_NAME,
    };

    #[test]
    fn exposes_kiro_envelope_name() {
        assert_eq!(KIRO_ENVELOPE_NAME, "kiro:generateAssistantResponse");
    }

    #[test]
    fn builds_runtime_root_url_from_region() {
        assert_eq!(
            build_kiro_generate_assistant_response_url(Some("stream=true"), Some("us-west-2"))
                .as_deref(),
            Some("https://runtime.us-west-2.kiro.dev/?stream=true")
        );
    }

    #[test]
    fn builds_runtime_root_url_without_query() {
        assert_eq!(
            build_kiro_generate_assistant_response_url(None, Some("us-east-1")).as_deref(),
            Some("https://runtime.us-east-1.kiro.dev/")
        );
    }

    #[test]
    fn resolves_runtime_base_url_from_region() {
        assert_eq!(
            resolve_kiro_base_url(Some("us-west-2")),
            "https://runtime.us-west-2.kiro.dev"
        );
    }

    #[test]
    fn builds_runtime_root_url_for_mcp() {
        assert_eq!(
            build_kiro_mcp_url(Some("eu-west-1")).as_deref(),
            Some("https://runtime.eu-west-1.kiro.dev/")
        );
    }

    #[test]
    fn builds_management_url_from_region() {
        assert_eq!(
            build_kiro_list_available_models_url(Some("us-west-2")).as_deref(),
            Some("https://management.us-west-2.kiro.dev/")
        );
    }

    #[test]
    fn accepts_runtime_url_for_mcp() {
        assert_eq!(
            build_kiro_mcp_url_from_resolved_url("https://runtime.us-east-1.kiro.dev/?beta=true")
                .as_deref(),
            Some("https://runtime.us-east-1.kiro.dev")
        );
    }

    #[test]
    fn rejects_legacy_url_for_mcp() {
        assert!(build_kiro_mcp_url_from_resolved_url(
            "https://q.us-east-1.amazonaws.com/generateAssistantResponse"
        )
        .is_none());
    }
}
