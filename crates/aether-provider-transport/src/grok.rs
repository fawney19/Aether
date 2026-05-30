use std::collections::BTreeMap;

use aether_contracts::{
    ResolvedTransportProfile, TRANSPORT_BACKEND_BROWSER_WREQ, TRANSPORT_HTTP_MODE_AUTO,
    TRANSPORT_POOL_SCOPE_KEY,
};
use serde_json::Value;
use serde_json::{json, Map};
use uuid::Uuid;

use crate::rules::apply_local_header_rules_with_request_headers;
use crate::snapshot::GatewayProviderTransportSnapshot;

pub const GROK_INTERNAL_HEADER: &str = "x-aether-grok-runtime";
pub const GROK_DEFAULT_BASE_URL: &str = "https://grok.com";
pub const GROK_CHAT_PATH: &str = "/rest/app-chat/conversations/new";
pub const GROK_RATE_LIMITS_PATH: &str = "/rest/rate-limits";
pub const GROK_IMAGE_EDIT_MODEL_NAME: &str = "imagine-image-edit";
pub const GROK_IMAGE_EDIT_MODEL_KIND: &str = "imagine";

pub const GROK_DEFAULT_BROWSER_PROFILE: &str = "chrome136";
pub const GROK_DEFAULT_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/136.0.0.0 Safari/537.36";
const GROK_IMAGE_GENERATION_MAX_COUNT: u64 = 4;
const GROK_SEC_CH_UA_PLATFORM: &str = r#""macOS""#;
const GROK_STATSIG_ID: &str = "ZTpUeXBlRXJyb3I6IENhbm5vdCByZWFkIHByb3BlcnRpZXMgb2YgdW5kZWZpbmVkIChyZWFkaW5nICdjaGlsZE5vZGVzJyk=";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrokBrowserProfileMetadata {
    pub profile_id: String,
    pub browser_profile: String,
    pub user_agent: String,
    pub sec_ch_ua: String,
    pub sec_ch_ua_platform: String,
}

#[derive(Debug, Clone)]
pub struct GrokHeaderInput<'a> {
    pub transport: &'a GatewayProviderTransportSnapshot,
    pub transport_profile: Option<&'a ResolvedTransportProfile>,
    pub request_headers: Option<&'a http::HeaderMap>,
    pub content_type: &'a str,
    pub accept: &'a str,
    pub header_rules: Option<&'a Value>,
    pub provider_request_body: &'a Value,
    pub original_request_body: &'a Value,
}

pub fn is_grok_provider_transport(transport: &GatewayProviderTransportSnapshot) -> bool {
    transport
        .provider
        .provider_type
        .trim()
        .eq_ignore_ascii_case("grok")
}

pub fn grok_base_url(base_url: &str) -> String {
    let base_url = base_url.trim().trim_end_matches('/');
    if base_url.is_empty() {
        GROK_DEFAULT_BASE_URL.to_string()
    } else {
        base_url.to_string()
    }
}

pub fn build_grok_upstream_url(transport: &GatewayProviderTransportSnapshot, path: &str) -> String {
    let base_url = grok_base_url(&transport.endpoint.base_url);
    let path = path.trim();
    if path.starts_with('/') {
        format!("{base_url}{path}")
    } else {
        format!("{base_url}/{path}")
    }
}

pub fn resolve_grok_session_auth(
    transport: &GatewayProviderTransportSnapshot,
) -> Option<(String, String)> {
    let cookie = grok_cookie_from_transport(transport)?;
    Some(("cookie".to_string(), cookie))
}

pub fn grok_browser_profile_metadata(
    profile_id: Option<&str>,
) -> Option<GrokBrowserProfileMetadata> {
    let raw = profile_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(GROK_DEFAULT_BROWSER_PROFILE);
    let normalized = raw.trim().to_ascii_lowercase().replace(['_', '-', ' '], "");
    let version = supported_grok_chrome_profile_version(&normalized)?;
    let profile_id = format!("chrome{version}");
    Some(GrokBrowserProfileMetadata {
        profile_id: profile_id.clone(),
        browser_profile: profile_id,
        user_agent: format!(
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{version}.0.0.0 Safari/537.36"
        ),
        sec_ch_ua: format!(
            r#""Google Chrome";v="{version}", "Chromium";v="{version}", "Not(A:Brand";v="24""#
        ),
        sec_ch_ua_platform: GROK_SEC_CH_UA_PLATFORM.to_string(),
    })
}

pub fn grok_browser_profile_id_from_user_agent(user_agent: &str) -> Option<String> {
    chrome_major_version_from_user_agent(user_agent, "Chrome/")
        .or_else(|| chrome_major_version_from_user_agent(user_agent, "Chromium/"))
        .and_then(|version| supported_grok_chrome_profile_version(&format!("chrome{version}")))
        .map(|version| format!("chrome{version}"))
}

pub fn grok_browser_profile_metadata_from_resolved_transport_profile(
    profile: &ResolvedTransportProfile,
) -> Option<GrokBrowserProfileMetadata> {
    if !profile
        .backend
        .trim()
        .eq_ignore_ascii_case(TRANSPORT_BACKEND_BROWSER_WREQ)
    {
        return None;
    }
    let profile_id = profile
        .extra
        .as_ref()
        .and_then(|value| value.get("browser_profile"))
        .and_then(Value::as_str)
        .or(Some(profile.profile_id.as_str()));
    grok_browser_profile_metadata(profile_id)
}

pub fn grok_browser_resolved_transport_profile_from_auth_config(
    auth_config: &Map<String, Value>,
    source: &str,
) -> Option<ResolvedTransportProfile> {
    let browser_profile = grok_browser_profile_id_from_auth_config(auth_config)?;
    grok_browser_resolved_transport_profile(browser_profile.as_deref(), source)
}

pub fn grok_browser_resolved_transport_profile(
    profile_id: Option<&str>,
    source: &str,
) -> Option<ResolvedTransportProfile> {
    let metadata = grok_browser_profile_metadata(profile_id)?;
    Some(ResolvedTransportProfile {
        profile_id: metadata.profile_id.clone(),
        backend: TRANSPORT_BACKEND_BROWSER_WREQ.to_string(),
        http_mode: TRANSPORT_HTTP_MODE_AUTO.to_string(),
        pool_scope: TRANSPORT_POOL_SCOPE_KEY.to_string(),
        header_fingerprint: None,
        extra: Some(json!({
            "browser_profile": metadata.browser_profile,
            "source": source,
        })),
    })
}

pub fn grok_browser_transport_fingerprint_from_auth_config(
    auth_config: &Map<String, Value>,
) -> Option<Value> {
    let transport_profile =
        grok_browser_resolved_transport_profile_from_auth_config(auth_config, "grok_import")?;
    Some(json!({ "transport_profile": transport_profile }))
}

fn grok_browser_profile_id_from_auth_config(
    auth_config: &Map<String, Value>,
) -> Option<Option<String>> {
    if let Some(browser_profile) = grok_config_string_from_object(
        Some(auth_config),
        &[
            "browser_profile",
            "browserProfile",
            "browser",
            "impersonate",
        ],
    ) {
        return Some(Some(browser_profile));
    }
    let Some(user_agent) =
        grok_config_string_from_object(Some(auth_config), &["user_agent", "userAgent"])
    else {
        return Some(None);
    };
    grok_browser_profile_id_from_user_agent(&user_agent).map(Some)
}

fn chrome_major_version_from_user_agent(user_agent: &str, marker: &str) -> Option<u16> {
    let user_agent_lower = user_agent.to_ascii_lowercase();
    let marker_lower = marker.to_ascii_lowercase();
    let index = user_agent_lower.find(&marker_lower)?;
    let version_start = index + marker.len();
    let digits = user_agent[version_start..]
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect::<String>();
    (!digits.is_empty())
        .then_some(digits)
        .and_then(|digits| digits.parse::<u16>().ok())
}

fn supported_grok_chrome_profile_version(normalized: &str) -> Option<u16> {
    let version = normalized.strip_prefix("chrome")?.parse::<u16>().ok()?;
    match version {
        100 | 101 | 104 | 105 | 106 | 107 | 108 | 109 | 110 | 114 | 116 | 117 | 118 | 119 | 120
        | 123 | 124 | 126 | 127 | 128 | 129 | 130 | 131 | 132 | 133 | 134 | 135 | 136 | 137
        | 138 | 139 | 140 | 141 | 142 | 143 | 144 | 145 => Some(version),
        _ => None,
    }
}

pub fn build_grok_browser_headers(input: GrokHeaderInput<'_>) -> Option<BTreeMap<String, String>> {
    let cookie = grok_cookie_from_transport(input.transport)?;
    let base_url = grok_base_url(&input.transport.endpoint.base_url);
    let browser_profile =
        grok_browser_profile_metadata_from_resolved_transport_profile(input.transport_profile?)?;

    let mut headers = BTreeMap::from([
        ("accept".to_string(), input.accept.to_string()),
        (
            "accept-language".to_string(),
            "zh-CN,zh;q=0.9,en;q=0.8,en-US;q=0.7".to_string(),
        ),
        (
            "baggage".to_string(),
            "sentry-environment=production,sentry-release=d6add6fb0460641fd482d767a335ef72b9b6abb8,sentry-public_key=b311e0f2690c81f25e2c4cf6d4f7ce1c".to_string(),
        ),
        ("content-type".to_string(), input.content_type.to_string()),
        ("cookie".to_string(), cookie),
        (GROK_INTERNAL_HEADER.to_string(), "1".to_string()),
        ("origin".to_string(), base_url.clone()),
        ("priority".to_string(), "u=1, i".to_string()),
        ("referer".to_string(), format!("{base_url}/")),
        ("sec-ch-ua".to_string(), browser_profile.sec_ch_ua),
        ("sec-ch-ua-mobile".to_string(), "?0".to_string()),
        (
            "sec-ch-ua-platform".to_string(),
            browser_profile.sec_ch_ua_platform,
        ),
        ("sec-fetch-dest".to_string(), "empty".to_string()),
        ("sec-fetch-mode".to_string(), "cors".to_string()),
        ("sec-fetch-site".to_string(), "same-origin".to_string()),
        ("user-agent".to_string(), browser_profile.user_agent),
        ("x-statsig-id".to_string(), GROK_STATSIG_ID.to_string()),
        ("x-xai-request-id".to_string(), Uuid::new_v4().to_string()),
    ]);

    if !apply_local_header_rules_with_request_headers(
        &mut headers,
        input.header_rules,
        &["cookie", "content-type", "accept", GROK_INTERNAL_HEADER],
        input.provider_request_body,
        Some(input.original_request_body),
        input.request_headers,
    ) {
        return None;
    }
    Some(headers)
}

pub fn build_grok_app_chat_body(
    client_api_format: &str,
    model: Option<&str>,
    original_body: &Value,
) -> Value {
    let model = model.map(str::trim).filter(|value| !value.is_empty());
    let client_api_format = client_api_format.trim().to_ascii_lowercase();
    if client_api_format.eq_ignore_ascii_case("openai:image")
        && grok_is_image_edit_request(original_body)
    {
        return build_grok_image_edit_body(original_body);
    }
    let is_image_generation =
        grok_is_image_generation_request(client_api_format.as_str(), model, original_body);
    let message = if is_image_generation {
        extract_grok_image_prompt(original_body)
            .map(|prompt| format!("Drawing: {prompt}"))
            .unwrap_or_else(|| "Drawing:".to_string())
    } else {
        match client_api_format.as_str() {
            "openai:responses" | "openai:responses:compact" => {
                extract_grok_message_from_openai_responses_body(original_body)
            }
            "claude:messages" => extract_grok_message_from_claude_messages_body(original_body),
            _ => extract_grok_message_from_openai_chat_body(original_body),
        }
    };
    let mut payload = grok_base_app_chat_payload(message, grok_mode_id_for_model(model));
    if is_image_generation {
        payload.insert(
            "imageGenerationCount".to_string(),
            json!(extract_grok_image_count(original_body)
                .unwrap_or(1)
                .clamp(1, GROK_IMAGE_GENERATION_MAX_COUNT)),
        );
        if let Some(size) = extract_grok_image_option(
            original_body,
            &["aspect_ratio", "aspectRatio", "ratio", "size"],
        ) {
            payload.insert("size".to_string(), Value::String(size));
        }
    }
    Value::Object(payload)
}

fn grok_is_image_generation_request(
    client_api_format: &str,
    model: Option<&str>,
    body: &Value,
) -> bool {
    if client_api_format.eq_ignore_ascii_case("openai:image") {
        return !grok_is_image_edit_request(body);
    }
    if !matches!(
        client_api_format,
        "openai:chat" | "openai:responses" | "openai:responses:compact"
    ) {
        return false;
    }
    grok_is_image_generation_model(model) || grok_has_image_generation_tool(body)
}

fn grok_is_image_generation_model(model: Option<&str>) -> bool {
    let model = model.unwrap_or_default().trim().to_ascii_lowercase();
    model.contains("grok-imagine-image") && !model.contains("edit")
}

fn grok_has_image_generation_tool(body: &Value) -> bool {
    body.get("tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .any(|tool| {
            tool.get("type")
                .and_then(Value::as_str)
                .is_some_and(|value| value.eq_ignore_ascii_case("image_generation"))
                && !tool
                    .get("action")
                    .and_then(Value::as_str)
                    .is_some_and(|value| value.eq_ignore_ascii_case("edit"))
        })
}

pub fn grok_is_image_edit_request(body: &Value) -> bool {
    body.get("tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .any(|tool| {
            tool.get("type")
                .and_then(Value::as_str)
                .is_some_and(|value| value.eq_ignore_ascii_case("image_generation"))
                && tool
                    .get("action")
                    .and_then(Value::as_str)
                    .is_some_and(|value| value.eq_ignore_ascii_case("edit"))
        })
}

pub fn build_grok_image_edit_body(body: &Value) -> Value {
    json!({
        "temporary": true,
        "modelName": GROK_IMAGE_EDIT_MODEL_NAME,
        "message": extract_grok_image_prompt(body).unwrap_or_default(),
        "enableImageGeneration": true,
        "returnImageBytes": false,
        "returnRawGrokInXaiRequest": false,
        "enableImageStreaming": true,
        "imageGenerationCount": extract_grok_image_count(body).unwrap_or(2).clamp(1, 2),
        "forceConcise": false,
        "enableSideBySide": true,
        "sendFinalMetadata": true,
        "isReasoning": false,
        "disableTextFollowUps": true,
        "responseMetadata": {
            "modelConfigOverride": {
                "modelMap": {
                    "imageEditModel": GROK_IMAGE_EDIT_MODEL_KIND,
                    "imageEditModelConfig": {
                        "imageReferences": [],
                        "parentPostId": ""
                    }
                }
            }
        },
        "disableMemory": true,
        "forceSideBySide": false,
    })
}

fn grok_base_app_chat_payload(message: String, mode_id: &'static str) -> Map<String, Value> {
    let mut payload = Map::new();
    payload.insert("collectionIds".to_string(), json!([]));
    payload.insert("connectors".to_string(), json!([]));
    payload.insert(
        "deviceEnvInfo".to_string(),
        json!({
            "darkModeEnabled": false,
            "devicePixelRatio": 2,
            "screenHeight": 1329,
            "screenWidth": 2056,
            "viewportHeight": 1083,
            "viewportWidth": 2056,
        }),
    );
    payload.insert("disableMemory".to_string(), json!(true));
    payload.insert("disableSearch".to_string(), json!(false));
    payload.insert("disableSelfHarmShortCircuit".to_string(), json!(false));
    payload.insert("disableTextFollowUps".to_string(), json!(false));
    payload.insert("enableImageGeneration".to_string(), json!(true));
    payload.insert("enableImageStreaming".to_string(), json!(true));
    payload.insert("enableSideBySide".to_string(), json!(true));
    payload.insert("fileAttachments".to_string(), json!([]));
    payload.insert("forceConcise".to_string(), json!(false));
    payload.insert("forceSideBySide".to_string(), json!(false));
    payload.insert("imageAttachments".to_string(), json!([]));
    payload.insert("imageGenerationCount".to_string(), json!(2));
    payload.insert("isAsyncChat".to_string(), json!(false));
    payload.insert("message".to_string(), Value::String(message));
    payload.insert("modeId".to_string(), Value::String(mode_id.to_string()));
    payload.insert("responseMetadata".to_string(), json!({}));
    payload.insert("returnImageBytes".to_string(), json!(false));
    payload.insert("returnRawGrokInXaiRequest".to_string(), json!(false));
    payload.insert("searchAllConnectors".to_string(), json!(false));
    payload.insert("sendFinalMetadata".to_string(), json!(true));
    payload.insert("temporary".to_string(), json!(true));
    payload.insert(
        "toolOverrides".to_string(),
        json!({
            "gmailSearch": false,
            "googleCalendarSearch": false,
            "outlookSearch": false,
            "outlookCalendarSearch": false,
            "googleDriveSearch": false,
        }),
    );
    payload
}

fn grok_mode_id_for_model(model: Option<&str>) -> &'static str {
    let model = model.unwrap_or_default().to_ascii_lowercase();
    if model.contains("4.3") || model.contains("computer") {
        "grok-420-computer-use-sa"
    } else if model.contains("multi-agent") {
        "heavy"
    } else if model.contains("non-reasoning") || model.contains("fast") || model.contains("lite") {
        "fast"
    } else if model.contains("expert") || model.contains("reasoning") {
        "expert"
    } else if model.contains("0309-heavy") {
        "auto"
    } else if model.contains("heavy") {
        "heavy"
    } else {
        "auto"
    }
}

fn extract_grok_message_from_openai_chat_body(body: &Value) -> String {
    let parts = body
        .get("messages")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|message| {
                    let role = message
                        .get("role")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .unwrap_or("user");
                    grok_chat_message_text(message).map(|text| format!("[{role}]: {text}"))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    (!parts.is_empty())
        .then(|| parts.join("\n\n"))
        .or_else(|| value_text(body.get("prompt")?))
        .unwrap_or_default()
}

fn extract_grok_message_from_openai_responses_body(body: &Value) -> String {
    let mut parts = Vec::new();
    if let Some(instructions) = body
        .get("instructions")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        parts.push(format!("system: {instructions}"));
    }
    if let Some(input) = body.get("input").and_then(value_text) {
        if !input.trim().is_empty() {
            parts.push(input);
        }
    }
    parts.join("\n\n")
}

fn extract_grok_message_from_claude_messages_body(body: &Value) -> String {
    let mut parts = Vec::new();
    if let Some(system) = body.get("system").and_then(value_text) {
        if !system.trim().is_empty() {
            parts.push(format!("system: {system}"));
        }
    }
    if let Some(messages) = body.get("messages").and_then(Value::as_array) {
        for message in messages {
            let role = message
                .get("role")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("user");
            if let Some(text) = grok_chat_message_text(message) {
                parts.push(format!("[{role}]: {text}"));
            }
        }
    }
    parts.join("\n\n")
}

fn grok_chat_message_text(message: &Value) -> Option<String> {
    let role = message
        .get("role")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if role.eq_ignore_ascii_case("tool") {
        let text = value_text(message.get("content")?)?;
        let text = text.trim();
        if text.is_empty() {
            return None;
        }
        let label = message
            .get("tool_call_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|id| format!("[tool result for {id}]"))
            .unwrap_or_else(|| "[tool result]".to_string());
        return Some(format!("{label}:\n{text}"));
    }

    if role.eq_ignore_ascii_case("assistant") {
        if let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array) {
            if !tool_calls.is_empty() {
                let mut parts = Vec::new();
                if let Some(text) = message.get("content").and_then(value_text) {
                    let text = text.trim();
                    if !text.is_empty() {
                        parts.push(text.to_string());
                    }
                }
                parts.push(grok_tool_calls_text(tool_calls));
                return Some(parts.join("\n"));
            }
        }
    }

    let text = value_text(message.get("content")?)?;
    let text = strip_grok_generated_artifacts(text.trim())
        .trim()
        .to_string();
    (!text.is_empty()).then_some(text)
}

fn grok_tool_calls_text(tool_calls: &[Value]) -> String {
    let mut parts = Vec::new();
    for tool_call in tool_calls {
        let function = tool_call.get("function").unwrap_or(tool_call);
        let name = function
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("tool");
        let arguments = function
            .get("arguments")
            .and_then(value_text)
            .unwrap_or_else(|| "{}".to_string());
        parts.push(format!(
            "<tool_call name=\"{name}\">{arguments}</tool_call>"
        ));
    }
    parts.join("\n")
}

fn strip_grok_generated_artifacts(text: &str) -> String {
    let marker = "[grok2api-sources]: #";
    let Some(marker_index) = text.find(marker) else {
        return text.to_string();
    };
    let prefix = &text[..marker_index];
    if let Some(sources_index) = prefix.rfind("## Sources") {
        prefix[..sources_index].trim_end().to_string()
    } else {
        text.to_string()
    }
}

fn extract_grok_image_prompt(body: &Value) -> Option<String> {
    body.get("prompt")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| body.get("input").and_then(value_text))
        .or_else(|| extract_grok_last_user_text_from_openai_chat_body(body))
}

fn extract_grok_image_count(body: &Value) -> Option<u64> {
    body.get("n")
        .and_then(Value::as_u64)
        .or_else(|| body.get("imageGenerationCount").and_then(Value::as_u64))
        .or_else(|| {
            body.get("image_config").and_then(|config| {
                config
                    .get("n")
                    .or_else(|| config.get("imageGenerationCount"))
                    .and_then(Value::as_u64)
            })
        })
        .or_else(|| {
            body.get("tools")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .find_map(|tool| {
                    tool.get("n")
                        .or_else(|| tool.get("imageGenerationCount"))
                        .and_then(Value::as_u64)
                })
        })
}

fn extract_grok_image_option(body: &Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(value) = body
            .get(*key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return Some(value.to_string());
        }
    }
    if let Some(config) = body.get("image_config").and_then(Value::as_object) {
        for key in keys {
            if let Some(value) = config
                .get(*key)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                return Some(value.to_string());
            }
        }
    }
    body.get("tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find_map(|tool| {
            keys.iter().find_map(|key| {
                tool.get(*key)
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned)
            })
        })
}

fn extract_grok_last_user_text_from_openai_chat_body(body: &Value) -> Option<String> {
    body.get("messages")
        .and_then(Value::as_array)?
        .iter()
        .rev()
        .find_map(|message| {
            let role = message.get("role").and_then(Value::as_str)?;
            if !role.eq_ignore_ascii_case("user") {
                return None;
            }
            grok_chat_message_text(message)
        })
}

fn value_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Array(items) => {
            let mut parts = Vec::new();
            for item in items {
                if let Some(text) = value_text(item) {
                    if !text.trim().is_empty() {
                        parts.push(text);
                    }
                }
            }
            Some(parts.join("\n"))
        }
        Value::Object(object) => {
            if is_attachment_content_block(object) {
                return None;
            }
            if let Some(text) = object
                .get("text")
                .or_else(|| object.get("input_text"))
                .or_else(|| object.get("content"))
                .and_then(value_text)
            {
                return Some(text);
            }
            None
        }
        _ => None,
    }
}

fn is_attachment_content_block(object: &Map<String, Value>) -> bool {
    let Some(block_type) = object.get("type").and_then(Value::as_str) else {
        return object.contains_key("image_url")
            || object.contains_key("file")
            || object.contains_key("file_data")
            || object.contains_key("file_url");
    };
    matches!(
        block_type,
        "image_url" | "input_image" | "input_file" | "file" | "image" | "document"
    )
}

fn parse_grok_auth_config(transport: &GatewayProviderTransportSnapshot) -> Option<Value> {
    transport
        .key
        .decrypted_auth_config
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .and_then(|value| serde_json::from_str::<Value>(value).ok())
}

fn grok_cookie_from_transport(transport: &GatewayProviderTransportSnapshot) -> Option<String> {
    let auth_config = parse_grok_auth_config(transport);
    let token = auth_config
        .as_ref()
        .and_then(|config| {
            grok_config_string(
                Some(config),
                &[
                    "sso_token",
                    "ssoToken",
                    "access_token",
                    "token",
                    "session_token",
                    "sessionToken",
                ],
            )
        })
        .or_else(|| {
            let raw = transport.key.decrypted_api_key.trim();
            (!raw.is_empty()).then(|| raw.to_string())
        })?;
    let token = strip_cookie_prefix(token.trim(), "sso=");
    if token.is_empty() {
        return None;
    }

    let sso_rw = auth_config
        .as_ref()
        .and_then(|config| grok_config_string(Some(config), &["sso_rw_token", "ssoRwToken"]))
        .map(|value| strip_cookie_prefix(value.trim(), "sso-rw="))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| token.clone());

    let mut parts = vec![format!("sso={token}"), format!("sso-rw={sso_rw}")];
    if let Some(extra_cookies) = auth_config
        .as_ref()
        .and_then(|config| grok_config_string(Some(config), &["cf_cookies", "cfCookies", "cookie"]))
        .and_then(|value| normalize_grok_extra_cookies(value.as_str()))
    {
        parts.push(extra_cookies);
    }
    if let Some(cf_clearance) = auth_config
        .as_ref()
        .and_then(|config| grok_config_string(Some(config), &["cf_clearance", "cfClearance"]))
        .map(|value| strip_cookie_prefix(value.trim(), "cf_clearance="))
        .filter(|value| !value.is_empty())
    {
        if !parts.iter().any(|part| part.contains("cf_clearance=")) {
            parts.push(format!("cf_clearance={cf_clearance}"));
        }
    }
    Some(parts.join("; "))
}

fn grok_config_string(config: Option<&Value>, fields: &[&str]) -> Option<String> {
    grok_config_string_from_object(config.and_then(Value::as_object), fields)
}

fn grok_config_string_from_object(
    object: Option<&Map<String, Value>>,
    fields: &[&str],
) -> Option<String> {
    let object = object?;
    fields.iter().find_map(|field| {
        object
            .get(*field)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    })
}

fn strip_cookie_prefix(value: &str, prefix: &str) -> String {
    value
        .strip_prefix(prefix)
        .unwrap_or(value)
        .trim()
        .to_string()
}

fn normalize_grok_extra_cookies(value: &str) -> Option<String> {
    let parts = value
        .trim()
        .trim_matches(';')
        .split(';')
        .filter_map(|segment| {
            let (name, value) = segment.trim().split_once('=')?;
            let name = name.trim();
            let value = value.trim();
            if name.is_empty()
                || value.is_empty()
                || name.eq_ignore_ascii_case("sso")
                || name.eq_ignore_ascii_case("sso-rw")
            {
                return None;
            }
            Some(format!("{name}={value}"))
        })
        .collect::<Vec<_>>();
    (!parts.is_empty()).then(|| parts.join("; "))
}

#[cfg(test)]
mod tests {
    use aether_contracts::{
        ResolvedTransportProfile, TRANSPORT_BACKEND_BROWSER_WREQ, TRANSPORT_HTTP_MODE_AUTO,
        TRANSPORT_POOL_SCOPE_KEY,
    };
    use serde_json::json;

    use super::{
        build_grok_app_chat_body, build_grok_browser_headers, build_grok_upstream_url,
        grok_browser_resolved_transport_profile,
        grok_browser_resolved_transport_profile_from_auth_config, resolve_grok_session_auth,
        GrokHeaderInput, GROK_CHAT_PATH, GROK_INTERNAL_HEADER,
    };
    use crate::snapshot::{
        GatewayProviderTransportEndpoint, GatewayProviderTransportKey,
        GatewayProviderTransportProvider, GatewayProviderTransportSnapshot,
    };

    fn sample_transport(auth_config: serde_json::Value) -> GatewayProviderTransportSnapshot {
        GatewayProviderTransportSnapshot {
            provider: GatewayProviderTransportProvider {
                id: "provider-1".to_string(),
                name: "Grok".to_string(),
                provider_type: "grok".to_string(),
                website: None,
                is_active: true,
                keep_priority_on_conversion: false,
                enable_format_conversion: true,
                concurrent_limit: None,
                max_retries: None,
                proxy: None,
                request_timeout_secs: None,
                stream_first_byte_timeout_secs: None,
                config: None,
            },
            endpoint: GatewayProviderTransportEndpoint {
                id: "endpoint-1".to_string(),
                provider_id: "provider-1".to_string(),
                api_format: "openai:chat".to_string(),
                api_family: None,
                endpoint_kind: None,
                is_active: true,
                base_url: "https://grok.com/".to_string(),
                header_rules: None,
                body_rules: None,
                max_retries: None,
                custom_path: None,
                config: None,
                format_acceptance_config: None,
                proxy: None,
            },
            key: GatewayProviderTransportKey {
                id: "key-1".to_string(),
                provider_id: "provider-1".to_string(),
                name: "key".to_string(),
                auth_type: "oauth".to_string(),
                is_active: true,
                api_formats: None,
                auth_type_by_format: None,
                allow_auth_channel_mismatch_formats: None,
                allowed_models: None,
                capabilities: None,
                rate_multipliers: None,
                global_priority_by_format: None,
                expires_at_unix_secs: None,
                proxy: None,
                fingerprint: None,
                upstream_metadata: None,
                decrypted_api_key: String::new(),
                decrypted_auth_config: Some(auth_config.to_string()),
            },
        }
    }

    #[test]
    fn resolves_sso_cookie_from_auth_config() {
        let transport = sample_transport(json!({
            "sso_token": "sso=abc",
            "sso_rw_token": "rw",
            "cf_clearance": "cf"
        }));

        let (_, value) = resolve_grok_session_auth(&transport).expect("cookie should resolve");

        assert_eq!(value, "sso=abc; sso-rw=rw; cf_clearance=cf");
    }

    #[test]
    fn resolves_sso_cookie_without_duplicate_session_cookies() {
        let transport = sample_transport(json!({
            "sso_token": "abc",
            "sso_rw_token": "rw",
            "cf_cookies": "i18nextLng=zh; sso=ignored; sso-rw=ignored-rw; cf_clearance=cf"
        }));

        let (_, value) = resolve_grok_session_auth(&transport).expect("cookie should resolve");

        assert_eq!(value, "sso=abc; sso-rw=rw; i18nextLng=zh; cf_clearance=cf");
    }

    #[test]
    fn builds_grok_browser_headers_with_internal_marker() {
        let transport = sample_transport(json!({"sso_token": "abc"}));
        let transport_profile =
            grok_browser_resolved_transport_profile(None, "test").expect("profile should resolve");
        let headers = build_grok_browser_headers(GrokHeaderInput {
            transport: &transport,
            transport_profile: Some(&transport_profile),
            request_headers: None,
            content_type: "application/json",
            accept: "*/*",
            header_rules: None,
            provider_request_body: &json!({"message":"hi"}),
            original_request_body: &json!({"messages":[]}),
        })
        .expect("headers should build");

        assert_eq!(headers.get(GROK_INTERNAL_HEADER), Some(&"1".to_string()));
        assert_eq!(
            headers.get("cookie"),
            Some(&"sso=abc; sso-rw=abc".to_string())
        );
        assert_eq!(headers.get("origin"), Some(&"https://grok.com".to_string()));
        assert!(headers
            .get("user-agent")
            .is_some_and(|value| value.contains("Chrome/136.0.0.0")));
        assert_eq!(
            headers.get("sec-ch-ua"),
            Some(
                &r#""Google Chrome";v="136", "Chromium";v="136", "Not(A:Brand";v="24""#.to_string()
            )
        );
        assert_eq!(
            headers.get("sec-ch-ua-platform"),
            Some(&r#""macOS""#.to_string())
        );
    }

    #[test]
    fn builds_grok_browser_headers_from_transport_profile() {
        let mut transport = sample_transport(json!({
            "sso_token": "abc",
            "user_agent": "Mozilla/5.0 custom",
            "browser_profile": "chrome136"
        }));
        transport.key.fingerprint = Some(json!({
            "transport_profile": {
                "profile_id": "chrome137",
                "backend": "browser_wreq",
                "extra": {"browser_profile": "chrome137"}
            }
        }));
        let transport_profile = grok_browser_resolved_transport_profile(Some("chrome137"), "test")
            .expect("profile should resolve");

        let headers = build_grok_browser_headers(GrokHeaderInput {
            transport: &transport,
            transport_profile: Some(&transport_profile),
            request_headers: None,
            content_type: "application/json",
            accept: "*/*",
            header_rules: None,
            provider_request_body: &json!({"message":"hi"}),
            original_request_body: &json!({"messages":[]}),
        })
        .expect("headers should build");

        assert!(headers
            .get("user-agent")
            .is_some_and(|value| value.contains("Chrome/137.0.0.0")));
        assert_eq!(
            headers.get("sec-ch-ua"),
            Some(
                &r#""Google Chrome";v="137", "Chromium";v="137", "Not(A:Brand";v="24""#.to_string()
            )
        );
    }

    #[test]
    fn infers_grok_browser_profile_from_legacy_user_agent() {
        let auth_config = json!({
            "sso_token": "abc",
            "user_agent": "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/137.0.0.0 Safari/537.36"
        });
        let auth_config = auth_config.as_object().expect("object");

        let profile = grok_browser_resolved_transport_profile_from_auth_config(auth_config, "test")
            .expect("profile should resolve from user agent");

        assert_eq!(profile.profile_id, "chrome137");
        assert_eq!(
            profile
                .extra
                .as_ref()
                .and_then(|value| value.get("browser_profile"))
                .and_then(serde_json::Value::as_str),
            Some("chrome137")
        );
    }

    #[test]
    fn rejects_unsupported_legacy_grok_user_agent_profile() {
        let auth_config = json!({
            "sso_token": "abc",
            "user_agent": "Mozilla/5.0 Version/18.0 Safari/605.1.15"
        });
        let auth_config = auth_config.as_object().expect("object");

        assert!(
            grok_browser_resolved_transport_profile_from_auth_config(auth_config, "test").is_none()
        );
    }

    #[test]
    fn rejects_unsupported_grok_browser_header_profile() {
        let mut transport = sample_transport(json!({"sso_token": "abc"}));
        transport.key.fingerprint = Some(json!({
            "transport_profile": {
                "profile_id": "safari999",
                "backend": "browser_wreq",
                "extra": {"browser_profile": "safari999"}
            }
        }));
        let transport_profile = ResolvedTransportProfile {
            profile_id: "safari999".to_string(),
            backend: TRANSPORT_BACKEND_BROWSER_WREQ.to_string(),
            http_mode: TRANSPORT_HTTP_MODE_AUTO.to_string(),
            pool_scope: TRANSPORT_POOL_SCOPE_KEY.to_string(),
            header_fingerprint: None,
            extra: Some(json!({"browser_profile": "safari999"})),
        };

        let headers = build_grok_browser_headers(GrokHeaderInput {
            transport: &transport,
            transport_profile: Some(&transport_profile),
            request_headers: None,
            content_type: "application/json",
            accept: "*/*",
            header_rules: None,
            provider_request_body: &json!({"message":"hi"}),
            original_request_body: &json!({"messages":[]}),
        });

        assert!(headers.is_none());
    }

    #[test]
    fn builds_rest_chat_url_from_endpoint_base() {
        let transport = sample_transport(json!({"sso_token": "abc"}));

        assert_eq!(
            build_grok_upstream_url(&transport, GROK_CHAT_PATH),
            "https://grok.com/rest/app-chat/conversations/new"
        );
    }

    #[test]
    fn builds_app_chat_body_from_openai_chat_messages() {
        let body = build_grok_app_chat_body(
            "openai:chat",
            Some("grok-4.20-fast"),
            &json!({
                "messages": [
                    {"role": "system", "content": "be short"},
                    {"role": "user", "content": [{"type":"text", "text":"hello"}]}
                ]
            }),
        );

        assert_eq!(body["modeId"], "fast");
        assert_eq!(body["message"], "[system]: be short\n\n[user]: hello");
        assert_eq!(body["enableImageGeneration"], true);
    }

    #[test]
    fn builds_app_chat_body_omits_uploaded_attachment_blocks_from_prompt() {
        let body = build_grok_app_chat_body(
            "openai:chat",
            Some("grok-4.20-fast"),
            &json!({
                "messages": [{
                    "role": "user",
                    "content": [
                        {"type": "text", "text": "describe this"},
                        {"type": "image_url", "image_url": {"url": "https://example.com/a.png"}},
                        {"type": "file", "file": {"filename": "notes.txt", "file_data": "data:text/plain;base64,bm90ZXM="}}
                    ]
                }]
            }),
        );

        assert_eq!(body["message"], "[user]: describe this");
    }

    #[test]
    fn builds_app_chat_body_keeps_openai_chat_history() {
        let body = build_grok_app_chat_body(
            "openai:chat",
            Some("grok-4.20-fast"),
            &json!({
                "messages": [
                    {"role": "system", "content": "remember the code word"},
                    {"role": "user", "content": "the code word is lantern"},
                    {"role": "assistant", "content": "Got it.\n\n## Sources\n[grok2api-sources]: #\n- [old](https://example.com)"},
                    {"role": "user", "content": "what is the code word?"}
                ]
            }),
        );

        assert_eq!(
            body["message"],
            "[system]: remember the code word\n\n[user]: the code word is lantern\n\n[assistant]: Got it.\n\n[user]: what is the code word?"
        );
    }

    #[test]
    fn builds_app_chat_body_keeps_tool_context() {
        let body = build_grok_app_chat_body(
            "openai:chat",
            Some("grok-4.20-fast"),
            &json!({
                "messages": [
                    {
                        "role": "assistant",
                        "content": null,
                        "tool_calls": [{
                            "id": "call_1",
                            "type": "function",
                            "function": {"name": "lookup", "arguments": "{\"q\":\"weather\"}"}
                        }]
                    },
                    {"role": "tool", "tool_call_id": "call_1", "content": "sunny"}
                ]
            }),
        );

        assert!(body["message"]
            .as_str()
            .expect("message should be string")
            .contains("<tool_call name=\"lookup\">{\"q\":\"weather\"}</tool_call>"));
        assert!(body["message"]
            .as_str()
            .expect("message should be string")
            .contains("[tool]: [tool result for call_1]:\nsunny"));
    }

    #[test]
    fn maps_grok_0309_model_variants_to_web_modes() {
        let request = json!({"messages": [{"role": "user", "content": "hello"}]});

        let fast = build_grok_app_chat_body(
            "openai:chat",
            Some("grok-4.20-0309-non-reasoning-heavy"),
            &request,
        );
        let auto = build_grok_app_chat_body("openai:chat", Some("grok-4.20-0309-heavy"), &request);
        let expert = build_grok_app_chat_body(
            "openai:chat",
            Some("grok-4.20-0309-reasoning-super"),
            &request,
        );
        let heavy =
            build_grok_app_chat_body("openai:chat", Some("grok-4.20-multi-agent-0309"), &request);

        assert_eq!(fast["modeId"], "fast");
        assert_eq!(auto["modeId"], "auto");
        assert_eq!(expert["modeId"], "expert");
        assert_eq!(heavy["modeId"], "heavy");
    }

    #[test]
    fn builds_app_chat_body_from_openai_image_prompt() {
        let body = build_grok_app_chat_body(
            "openai:image",
            Some("grok-imagine-image"),
            &json!({"prompt":"a red chair", "n": 3}),
        );

        assert_eq!(body["modeId"], "auto");
        assert_eq!(body["message"], "Drawing: a red chair");
        assert_eq!(body["imageGenerationCount"], 3);
    }

    #[test]
    fn builds_app_chat_body_from_openai_responses_image_generation_tool() {
        let body = build_grok_app_chat_body(
            "openai:responses",
            Some("grok-imagine-image-lite"),
            &json!({
                "input": "a red chair",
                "tools": [{
                    "type": "image_generation",
                    "n": 4,
                    "size": "1280x720"
                }]
            }),
        );

        assert_eq!(body["modeId"], "fast");
        assert_eq!(body["message"], "Drawing: a red chair");
        assert_eq!(body["imageGenerationCount"], 4);
        assert_eq!(body["size"], "1280x720");
    }

    #[test]
    fn app_chat_image_generation_count_uses_gateway_ceiling() {
        let body = build_grok_app_chat_body(
            "openai:responses",
            Some("grok-imagine-image-lite"),
            &json!({
                "input": "a red chair",
                "tools": [{
                    "type": "image_generation",
                    "n": 10
                }]
            }),
        );

        assert_eq!(body["imageGenerationCount"], 4);
    }

    #[test]
    fn builds_app_chat_body_from_openai_chat_image_model_request() {
        let body = build_grok_app_chat_body(
            "openai:chat",
            Some("grok-imagine-image-pro"),
            &json!({
                "messages": [
                    {"role": "system", "content": "ignore prior style"},
                    {"role": "user", "content": [{"type":"text", "text":"a blue sofa"}]}
                ],
                "image_config": {
                    "n": 2,
                    "size": "720x1280"
                }
            }),
        );

        assert_eq!(body["message"], "Drawing: a blue sofa");
        assert_eq!(body["imageGenerationCount"], 2);
        assert_eq!(body["size"], "720x1280");
    }

    #[test]
    fn builds_app_chat_body_from_openai_image_edit_request() {
        let body = build_grok_app_chat_body(
            "openai:image",
            Some("grok-imagine-image-edit"),
            &json!({
                "input": [
                    {"type": "input_text", "text": "make the chair blue"},
                    {"type": "input_image", "image_url": "data:image/png;base64,aW1hZ2U="}
                ],
                "tools": [{
                    "type": "image_generation",
                    "action": "edit"
                }]
            }),
        );

        assert_eq!(body["modelName"], "imagine-image-edit");
        assert_eq!(body["message"], "make the chair blue");
        assert_eq!(body["enableImageGeneration"], true);
        assert_eq!(body["enableImageStreaming"], true);
        assert_eq!(body["disableTextFollowUps"], true);
        assert_eq!(body["imageGenerationCount"], 2);
        assert_eq!(
            body["responseMetadata"]["modelConfigOverride"]["modelMap"]["imageEditModel"],
            "imagine"
        );
    }
}
