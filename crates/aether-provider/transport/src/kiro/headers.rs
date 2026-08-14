use std::collections::BTreeMap;

use uuid::Uuid;

use super::credentials::KiroAuthConfig;

pub const AWS_EVENTSTREAM_CONTENT_TYPE: &str = "application/vnd.amazon.eventstream";
pub const KIRO_TOKEN_TYPE_HEADER: &str = "TokenType";
pub const KIRO_EXTERNAL_IDP_TOKEN_TYPE: &str = "EXTERNAL_IDP";
pub const KIRO_RUNTIME_TARGET: &str = "KiroRuntimeService.GenerateAssistantResponse";
pub const KIRO_INVOKE_MCP_TARGET: &str = "KiroRuntimeService.InvokeMCP";
pub const KIRO_LIST_MODELS_TARGET: &str = "KiroControlPlaneBearerService.ListAvailableModels";
const KIRO_RUNTIME_SDK_JS_VERSION: &str = "1.0.0";
const AWS_SDK_JS_LIST_MODELS_VERSION: &str = "1.0.0";

fn build_kiro_ide_tag(kiro_version: &str, machine_id: &str) -> String {
    if machine_id.trim().is_empty() {
        format!("KiroIDE-{kiro_version}")
    } else {
        format!("KiroIDE-{kiro_version}-{machine_id}")
    }
}

fn build_x_amz_user_agent_main(sdk_version: &str, kiro_version: &str, machine_id: &str) -> String {
    format!(
        "aws-sdk-js/{sdk_version} {}",
        build_kiro_ide_tag(kiro_version, machine_id)
    )
}

fn build_runtime_user_agent(
    system_version: &str,
    node_version: &str,
    kiro_version: &str,
    machine_id: &str,
) -> String {
    format!(
        "aws-sdk-js/{KIRO_RUNTIME_SDK_JS_VERSION} ua/2.1 os/{system_version} lang/js md/nodejs#{node_version} api/kiroruntime#{KIRO_RUNTIME_SDK_JS_VERSION} m/N {}",
        build_kiro_ide_tag(kiro_version, machine_id)
    )
}

pub fn build_generate_assistant_headers(
    auth_config: &KiroAuthConfig,
    machine_id: &str,
) -> BTreeMap<String, String> {
    let kiro_version = auth_config.effective_kiro_version();
    let system_version = auth_config.effective_system_version();
    let node_version = auth_config.effective_node_version();
    let region = auth_config.effective_api_region();
    BTreeMap::from([
        (
            "accept".to_string(),
            AWS_EVENTSTREAM_CONTENT_TYPE.to_string(),
        ),
        (
            "amz-sdk-invocation-id".to_string(),
            Uuid::new_v4().to_string(),
        ),
        (
            "amz-sdk-request".to_string(),
            "attempt=1; max=3".to_string(),
        ),
        ("connection".to_string(), "close".to_string()),
        (
            "content-type".to_string(),
            "application/x-amz-json-1.0".to_string(),
        ),
        ("host".to_string(), format!("runtime.{region}.kiro.dev")),
        (
            "user-agent".to_string(),
            build_runtime_user_agent(system_version, node_version, kiro_version, machine_id),
        ),
        (
            "x-amz-user-agent".to_string(),
            build_x_amz_user_agent_main(KIRO_RUNTIME_SDK_JS_VERSION, kiro_version, machine_id),
        ),
        ("x-amz-target".to_string(), KIRO_RUNTIME_TARGET.to_string()),
    ])
}

pub fn build_mcp_headers(
    auth_config: &KiroAuthConfig,
    machine_id: &str,
) -> BTreeMap<String, String> {
    let kiro_version = auth_config.effective_kiro_version();
    let system_version = auth_config.effective_system_version();
    let node_version = auth_config.effective_node_version();
    let region = auth_config.effective_api_region();
    BTreeMap::from([
        ("accept".to_string(), "application/json".to_string()),
        (
            "amz-sdk-invocation-id".to_string(),
            Uuid::new_v4().to_string(),
        ),
        (
            "amz-sdk-request".to_string(),
            "attempt=1; max=3".to_string(),
        ),
        ("connection".to_string(), "close".to_string()),
        (
            "content-type".to_string(),
            "application/x-amz-json-1.0".to_string(),
        ),
        ("host".to_string(), format!("runtime.{region}.kiro.dev")),
        (
            "user-agent".to_string(),
            build_runtime_user_agent(system_version, node_version, kiro_version, machine_id),
        ),
        (
            "x-amz-user-agent".to_string(),
            build_x_amz_user_agent_main(KIRO_RUNTIME_SDK_JS_VERSION, kiro_version, machine_id),
        ),
        (
            "x-amz-target".to_string(),
            KIRO_INVOKE_MCP_TARGET.to_string(),
        ),
    ])
}

pub fn build_list_available_models_headers(
    auth_config: &KiroAuthConfig,
    machine_id: &str,
) -> BTreeMap<String, String> {
    let kiro_version = auth_config.effective_kiro_version();
    let system_version = auth_config.effective_system_version();
    let node_version = auth_config.effective_node_version();
    let region = auth_config.effective_api_region();
    let ide_tag = build_kiro_ide_tag(kiro_version, machine_id);

    BTreeMap::from([
        ("accept".to_string(), "application/json".to_string()),
        (
            "amz-sdk-invocation-id".to_string(),
            Uuid::new_v4().to_string(),
        ),
        (
            "amz-sdk-request".to_string(),
            "attempt=1; max=3".to_string(),
        ),
        ("connection".to_string(), "close".to_string()),
        ("host".to_string(), format!("management.{region}.kiro.dev")),
        (
            "user-agent".to_string(),
            format!(
                "aws-sdk-js/{AWS_SDK_JS_LIST_MODELS_VERSION} ua/2.1 os/{system_version} lang/js md/nodejs#{node_version} api/kirocontrolplanebearer#1.0.0 m/N,E {ide_tag}"
            ),
        ),
        (
            "x-amz-user-agent".to_string(),
            format!("aws-sdk-js/{AWS_SDK_JS_LIST_MODELS_VERSION} {ide_tag}"),
        ),
        (
            "content-type".to_string(),
            "application/x-amz-json-1.0".to_string(),
        ),
        (
            "x-amz-target".to_string(),
            KIRO_LIST_MODELS_TARGET.to_string(),
        ),
    ])
}

#[cfg(test)]
mod tests {
    use super::super::credentials::KiroAuthConfig;
    use super::{
        build_generate_assistant_headers, build_list_available_models_headers, build_mcp_headers,
        AWS_EVENTSTREAM_CONTENT_TYPE, KIRO_INVOKE_MCP_TARGET, KIRO_LIST_MODELS_TARGET,
        KIRO_RUNTIME_TARGET, KIRO_TOKEN_TYPE_HEADER,
    };

    #[test]
    fn builds_generate_assistant_headers_for_region() {
        let auth_config = KiroAuthConfig {
            auth_method: None,
            refresh_token: None,
            expires_at: None,
            profile_arn: None,
            region: None,
            auth_region: None,
            api_region: Some("us-west-2".to_string()),
            client_id: None,
            client_secret: None,
            machine_id: None,
            kiro_version: Some("1.2.3".to_string()),
            system_version: Some("darwin#24.6.0".to_string()),
            node_version: Some("22.21.1".to_string()),
            access_token: None,
        };

        let headers = build_generate_assistant_headers(&auth_config, "machine-123");
        assert_eq!(
            headers.get("accept").map(String::as_str),
            Some(AWS_EVENTSTREAM_CONTENT_TYPE)
        );
        assert_eq!(
            headers.get("host").map(String::as_str),
            Some("runtime.us-west-2.kiro.dev")
        );
        assert!(headers
            .get("user-agent")
            .is_some_and(|value| value.contains("api/kiroruntime#1.0.0")));
        assert!(!headers.contains_key("x-amzn-codewhisperer-optout"));
        assert_eq!(
            headers.get("x-amz-target").map(String::as_str),
            Some(KIRO_RUNTIME_TARGET)
        );
        assert!(!headers.contains_key(KIRO_TOKEN_TYPE_HEADER));
    }

    #[test]
    fn builds_list_available_models_headers() {
        let auth_config = KiroAuthConfig {
            auth_method: Some("social".to_string()),
            refresh_token: None,
            expires_at: None,
            profile_arn: Some("arn:aws:codewhisperer:us-east-1:123456789012:profile/demo".into()),
            region: None,
            auth_region: None,
            api_region: Some("us-east-1".to_string()),
            client_id: None,
            client_secret: None,
            machine_id: None,
            kiro_version: Some("0.12.155".to_string()),
            system_version: Some("darwin#24.6.0".to_string()),
            node_version: Some("22.21.1".to_string()),
            access_token: None,
        };

        let headers = build_list_available_models_headers(&auth_config, "machine-123");

        assert_eq!(
            headers.get("host").map(String::as_str),
            Some("management.us-east-1.kiro.dev")
        );
        assert_eq!(
            headers.get("amz-sdk-request").map(String::as_str),
            Some("attempt=1; max=3")
        );
        assert_eq!(
            headers.get("x-amz-user-agent").map(String::as_str),
            Some("aws-sdk-js/1.0.0 KiroIDE-0.12.155-machine-123")
        );
        assert!(!headers.contains_key(KIRO_TOKEN_TYPE_HEADER));
        assert_eq!(
            headers.get("x-amz-target").map(String::as_str),
            Some(KIRO_LIST_MODELS_TARGET)
        );
        assert_eq!(
            headers.get("content-type").map(String::as_str),
            Some("application/x-amz-json-1.0")
        );
    }

    #[test]
    fn keeps_generate_assistant_headers_without_external_idp_token_type() {
        let auth_config = KiroAuthConfig {
            auth_method: Some("idc".to_string()),
            refresh_token: None,
            expires_at: None,
            profile_arn: Some("arn:aws:codewhisperer:us-east-1:123456789012:profile/demo".into()),
            region: None,
            auth_region: None,
            api_region: Some("us-east-1".to_string()),
            client_id: Some("client-id".to_string()),
            client_secret: Some("client-secret".to_string()),
            machine_id: None,
            kiro_version: None,
            system_version: None,
            node_version: None,
            access_token: None,
        };

        let headers = build_generate_assistant_headers(&auth_config, "machine-123");

        assert_eq!(
            headers.get("accept").map(String::as_str),
            Some(AWS_EVENTSTREAM_CONTENT_TYPE)
        );
        assert!(!headers.contains_key(KIRO_TOKEN_TYPE_HEADER));
    }

    #[test]
    fn builds_mcp_headers_without_profile_arn_header() {
        let auth_config = KiroAuthConfig {
            auth_method: Some("social".to_string()),
            refresh_token: None,
            expires_at: None,
            profile_arn: Some("arn:aws:codewhisperer:us-east-1:123456789012:profile/demo".into()),
            region: None,
            auth_region: None,
            api_region: Some("us-east-1".to_string()),
            client_id: None,
            client_secret: None,
            machine_id: None,
            kiro_version: Some("0.3.210".to_string()),
            system_version: Some("darwin#24.6.0".to_string()),
            node_version: Some("22.21.1".to_string()),
            access_token: None,
        };

        let headers = build_mcp_headers(&auth_config, "machine-123");
        assert_eq!(
            headers.get("accept").map(String::as_str),
            Some("application/json")
        );
        assert!(!headers.contains_key("x-amzn-kiro-profile-arn"));
        assert!(!headers.contains_key(KIRO_TOKEN_TYPE_HEADER));
        assert_eq!(
            headers.get("x-amz-target").map(String::as_str),
            Some(KIRO_INVOKE_MCP_TARGET)
        );
    }

    #[test]
    fn builds_mcp_headers_for_idc_auth() {
        let auth_config = KiroAuthConfig {
            auth_method: Some("idc".to_string()),
            refresh_token: None,
            expires_at: None,
            profile_arn: Some("arn:aws:codewhisperer:us-east-1:123456789012:profile/demo".into()),
            region: None,
            auth_region: None,
            api_region: Some("us-west-2".to_string()),
            client_id: Some("client-id".to_string()),
            client_secret: Some("client-secret".to_string()),
            machine_id: None,
            kiro_version: None,
            system_version: None,
            node_version: None,
            access_token: None,
        };

        let headers = build_mcp_headers(&auth_config, "machine-123");
        assert_eq!(
            headers.get("host").map(String::as_str),
            Some("runtime.us-west-2.kiro.dev")
        );
        assert!(!headers.contains_key("x-amzn-kiro-profile-arn"));
        assert!(!headers.contains_key(KIRO_TOKEN_TYPE_HEADER));
    }
}
