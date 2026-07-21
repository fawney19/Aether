use aether_scheduler_core::ClientSessionAffinity;
use serde_json::{Map, Value};

use crate::headers::header_value_str;

pub(crate) const AETHER_SESSION_ID_HEADER: &str = "x-aether-session-id";
pub(crate) const AETHER_AGENT_ID_HEADER: &str = "x-aether-agent-id";
pub(crate) const CLIENT_SESSION_AFFINITY_REPORT_CONTEXT_FIELD: &str = "client_session_affinity";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClientSessionSignalSource {
    ExplicitAetherHeader,
    Header,
    Body,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClientSessionScope {
    pub(crate) client_family: String,
    pub(crate) session_id: String,
    pub(crate) agent_id: Option<String>,
    pub(crate) account_hint: Option<String>,
    pub(crate) source: ClientSessionSignalSource,
}

impl ClientSessionScope {
    fn new(
        client_family: impl Into<String>,
        session_id: impl Into<String>,
        agent_id: Option<String>,
        account_hint: Option<String>,
        source: ClientSessionSignalSource,
    ) -> Self {
        Self {
            client_family: client_family.into(),
            session_id: session_id.into(),
            agent_id,
            account_hint,
            source,
        }
    }

    fn scheduler_session_key(&self) -> Option<String> {
        let session_id = self.session_id.trim();
        if session_id.is_empty() {
            return None;
        }

        Some(normalize_session_key(
            self.account_hint.as_deref(),
            session_id,
            self.agent_id.as_deref(),
        ))
    }

    pub(crate) fn scheduler_affinity(&self) -> Option<ClientSessionAffinity> {
        let client_family = self.client_family.trim();
        let client_family = if client_family.is_empty() {
            "unknown".to_string()
        } else {
            client_family.to_ascii_lowercase()
        };
        Some(ClientSessionAffinity::new(
            Some(client_family),
            Some(self.scheduler_session_key()?),
        ))
    }
}

struct ClientSessionRequest<'a> {
    headers: &'a http::HeaderMap,
    body_json: Option<&'a Value>,
}

trait ClientSessionScopeAdapter {
    fn family(&self) -> &'static str;

    fn detect(&self, request: &ClientSessionRequest<'_>) -> bool;

    fn extract_scope(&self, request: &ClientSessionRequest<'_>) -> Option<ClientSessionScope>;
}

struct GenericSessionScopeAdapter;
struct CodexSessionScopeAdapter;
struct ClaudeCodeSessionScopeAdapter;
struct OpenCodeSessionScopeAdapter;
struct QwenCodeSessionScopeAdapter;
struct RooCodeSessionScopeAdapter;
struct KiloCodeSessionScopeAdapter;
struct CherryStudioSessionScopeAdapter;
struct OpenUiSessionScopeAdapter;
struct OpenAiJsSdkSessionScopeAdapter;
struct OpenAiPythonSdkSessionScopeAdapter;
struct AnthropicJsSdkSessionScopeAdapter;
struct AnthropicPythonSdkSessionScopeAdapter;

pub(crate) fn client_session_affinity_from_request(
    headers: &http::HeaderMap,
    body_json: Option<&Value>,
) -> Option<ClientSessionAffinity> {
    client_session_scope_from_request(headers, body_json)?.scheduler_affinity()
}

pub(crate) fn client_session_affinity_from_api_request(
    api_format: &str,
    headers: &http::HeaderMap,
    body_json: Option<&Value>,
) -> Option<ClientSessionAffinity> {
    client_session_scope_from_api_request(api_format, headers, body_json)?.scheduler_affinity()
}

fn client_session_scope_from_api_request(
    api_format: &str,
    headers: &http::HeaderMap,
    body_json: Option<&Value>,
) -> Option<ClientSessionScope> {
    if api_format.trim().eq_ignore_ascii_case("openai:search") {
        let request = ClientSessionRequest { headers, body_json };
        return explicit_aether_session_scope(&request, CodexSessionScopeAdapter.family())
            .or_else(|| codex_search_session_scope(&request))
            .or_else(|| client_session_scope_from_request(headers, body_json));
    }

    client_session_scope_from_request(headers, body_json)
}

pub(crate) fn client_session_scope_from_request(
    headers: &http::HeaderMap,
    body_json: Option<&Value>,
) -> Option<ClientSessionScope> {
    let request = ClientSessionRequest { headers, body_json };
    let client_family = detect_client_family(&request);
    explicit_aether_session_scope(&request, client_family.as_str())
        .or_else(|| extract_scope_for_client_family(&request, client_family.as_str()))
        .or_else(|| extract_generic_scope_for_client_family(&request, client_family.as_str()))
        .or_else(|| extract_scope_from_other_specific_adapters(&request, client_family.as_str()))
}

fn codex_search_session_scope(request: &ClientSessionRequest<'_>) -> Option<ClientSessionScope> {
    let session_id = request
        .body_json?
        .get("id")?
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    Some(ClientSessionScope::new(
        CodexSessionScopeAdapter.family(),
        session_id,
        None,
        header_value_str(request.headers, "chatgpt-account-id"),
        ClientSessionSignalSource::Body,
    ))
}

pub(crate) fn client_session_affinity_from_parts(
    parts: &http::request::Parts,
    body_json: Option<&Value>,
) -> Option<ClientSessionAffinity> {
    client_session_scope_from_parts(parts, body_json)?.scheduler_affinity()
}

pub(crate) fn client_session_scope_from_parts(
    parts: &http::request::Parts,
    body_json: Option<&Value>,
) -> Option<ClientSessionScope> {
    client_session_scope_from_request(&parts.headers, body_json)
}

pub(crate) fn client_session_affinity_report_context_value(
    affinity: &ClientSessionAffinity,
) -> Option<Value> {
    let session_key = affinity
        .session_key
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let mut object = Map::new();
    if let Some(client_family) = affinity
        .client_family
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        object.insert(
            "client_family".to_string(),
            Value::String(client_family.to_ascii_lowercase()),
        );
    }
    object.insert(
        "session_key".to_string(),
        Value::String(session_key.to_string()),
    );
    Some(Value::Object(object))
}

pub(crate) fn client_session_affinity_from_report_context_value(
    value: Option<&Value>,
) -> Option<ClientSessionAffinity> {
    let object = value?.as_object()?;
    let session_key = object
        .get("session_key")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?
        .to_string();
    let client_family = object
        .get("client_family")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase);

    Some(ClientSessionAffinity::new(client_family, Some(session_key)))
}

fn detect_client_family(request: &ClientSessionRequest<'_>) -> String {
    for adapter in specific_client_session_scope_adapters() {
        if adapter.detect(request) {
            return adapter.family().to_string();
        }
    }
    if let Some(client_family) = detect_fingerprint_client_family(request) {
        return client_family.to_string();
    }
    GenericSessionScopeAdapter.family().to_string()
}

fn detect_fingerprint_client_family(request: &ClientSessionRequest<'_>) -> Option<&'static str> {
    if header_contains(
        request.headers,
        http::header::USER_AGENT.as_str(),
        "geminicli",
    ) || header_contains(
        request.headers,
        http::header::USER_AGENT.as_str(),
        "gemini-cli",
    ) {
        return Some("gemini_cli");
    }
    if header_contains(request.headers, http::header::USER_AGENT.as_str(), "cursor") {
        return Some("cursor");
    }
    if header_contains(
        request.headers,
        http::header::USER_AGENT.as_str(),
        "windsurf",
    ) {
        return Some("windsurf");
    }
    if header_contains(
        request.headers,
        http::header::USER_AGENT.as_str(),
        "continue",
    ) {
        return Some("continue");
    }
    if header_contains(request.headers, http::header::USER_AGENT.as_str(), "cline") {
        return Some("cline");
    }
    if header_contains(request.headers, http::header::USER_AGENT.as_str(), "aider") {
        return Some("aider");
    }
    if header_contains(
        request.headers,
        http::header::USER_AGENT.as_str(),
        "langchain",
    ) {
        return Some("langchain");
    }
    if header_contains(
        request.headers,
        http::header::USER_AGENT.as_str(),
        "llamaindex",
    ) || header_contains(
        request.headers,
        http::header::USER_AGENT.as_str(),
        "llama-index",
    ) {
        return Some("llamaindex");
    }
    if has_header_with_prefix(request.headers, "x-stainless-") {
        return Some("sdk");
    }
    None
}

fn specific_client_session_scope_adapters() -> [&'static dyn ClientSessionScopeAdapter; 12] {
    [
        &CodexSessionScopeAdapter,
        &ClaudeCodeSessionScopeAdapter,
        &OpenCodeSessionScopeAdapter,
        &QwenCodeSessionScopeAdapter,
        &RooCodeSessionScopeAdapter,
        &KiloCodeSessionScopeAdapter,
        &CherryStudioSessionScopeAdapter,
        &OpenUiSessionScopeAdapter,
        &AnthropicJsSdkSessionScopeAdapter,
        &AnthropicPythonSdkSessionScopeAdapter,
        &OpenAiJsSdkSessionScopeAdapter,
        &OpenAiPythonSdkSessionScopeAdapter,
    ]
}

fn extract_scope_for_client_family(
    request: &ClientSessionRequest<'_>,
    client_family: &str,
) -> Option<ClientSessionScope> {
    specific_client_session_scope_adapters()
        .into_iter()
        .find(|adapter| adapter.family() == client_family)
        .and_then(|adapter| adapter.extract_scope(request))
}

fn extract_scope_from_other_specific_adapters(
    request: &ClientSessionRequest<'_>,
    client_family: &str,
) -> Option<ClientSessionScope> {
    specific_client_session_scope_adapters()
        .into_iter()
        .filter(|adapter| adapter.family() != client_family)
        .filter(|adapter| adapter.detect(request))
        .find_map(|adapter| adapter.extract_scope(request))
}

fn extract_generic_scope_for_client_family(
    request: &ClientSessionRequest<'_>,
    client_family: &str,
) -> Option<ClientSessionScope> {
    let generic = GenericSessionScopeAdapter.extract_scope(request)?;
    Some(ClientSessionScope::new(
        client_family,
        generic.session_id,
        generic.agent_id,
        generic.account_hint,
        generic.source,
    ))
}

impl ClientSessionScopeAdapter for GenericSessionScopeAdapter {
    fn family(&self) -> &'static str {
        "unknown"
    }

    fn detect(&self, _request: &ClientSessionRequest<'_>) -> bool {
        true
    }

    fn extract_scope(&self, request: &ClientSessionRequest<'_>) -> Option<ClientSessionScope> {
        if let Some(root_session) = header_value_str(request.headers, "session-id")
            .or_else(|| header_value_str(request.headers, "thread-id"))
            .or_else(|| header_value_str(request.headers, "session_id"))
            .or_else(|| header_value_str(request.headers, "conversation_id"))
        {
            return Some(ClientSessionScope::new(
                self.family(),
                root_session,
                None,
                None,
                ClientSessionSignalSource::Header,
            ));
        }

        let body = request.body_json?;
        let root_session = value_at_paths(
            body,
            &[
                &["prompt_cache_key"],
                &["conversation_id"],
                &["conversationId"],
                &["session_id"],
                &["sessionId"],
                &["metadata", "session_id"],
                &["metadata", "conversation_id"],
                &["conversationState", "conversationId"],
                &["conversationState", "sessionId"],
            ],
        )?;
        let agent_id = value_at_paths(
            body,
            &[
                &["agent_id"],
                &["agentId"],
                &["metadata", "agent_id"],
                &["metadata", "agentId"],
                &["conversationState", "agentId"],
            ],
        );

        Some(ClientSessionScope::new(
            self.family(),
            root_session,
            agent_id.map(ToOwned::to_owned),
            None,
            ClientSessionSignalSource::Body,
        ))
    }
}

impl ClientSessionScopeAdapter for CodexSessionScopeAdapter {
    fn family(&self) -> &'static str {
        "codex"
    }

    fn detect(&self, request: &ClientSessionRequest<'_>) -> bool {
        header_contains(request.headers, http::header::USER_AGENT.as_str(), "codex")
            || header_contains(request.headers, "originator", "codex")
            || header_value_str(request.headers, "chatgpt-account-id").is_some()
    }

    fn extract_scope(&self, request: &ClientSessionRequest<'_>) -> Option<ClientSessionScope> {
        header_value_str(request.headers, "session-id")
            .or_else(|| header_value_str(request.headers, "thread-id"))
            .or_else(|| header_value_str(request.headers, "session_id"))
            .or_else(|| header_value_str(request.headers, "conversation_id"))
            .map(|root_session| {
                ClientSessionScope::new(
                    self.family(),
                    root_session,
                    None,
                    header_value_str(request.headers, "chatgpt-account-id"),
                    ClientSessionSignalSource::Header,
                )
            })
            .or_else(|| {
                let body_session = GenericSessionScopeAdapter.extract_scope(request)?;
                Some(ClientSessionScope::new(
                    self.family(),
                    body_session.session_id,
                    body_session.agent_id,
                    header_value_str(request.headers, "chatgpt-account-id"),
                    body_session.source,
                ))
            })
    }
}

impl ClientSessionScopeAdapter for ClaudeCodeSessionScopeAdapter {
    fn family(&self) -> &'static str {
        "claude_code"
    }

    fn detect(&self, request: &ClientSessionRequest<'_>) -> bool {
        header_contains(
            request.headers,
            http::header::USER_AGENT.as_str(),
            "claude-code",
        ) || header_contains(
            request.headers,
            http::header::USER_AGENT.as_str(),
            "claude code",
        ) || header_value_str(request.headers, "x-claude-code-session-id").is_some()
    }

    fn extract_scope(&self, request: &ClientSessionRequest<'_>) -> Option<ClientSessionScope> {
        header_value_str(request.headers, "x-claude-code-session-id")
            .or_else(|| header_value_str(request.headers, "session_id"))
            .or_else(|| header_value_str(request.headers, "conversation_id"))
            .map(|root_session| {
                ClientSessionScope::new(
                    self.family(),
                    root_session,
                    None,
                    None,
                    ClientSessionSignalSource::Header,
                )
            })
            .or_else(|| {
                let root_session = claude_code_session_id_from_body(request.body_json?)?;
                Some(ClientSessionScope::new(
                    self.family(),
                    root_session,
                    None,
                    None,
                    ClientSessionSignalSource::Body,
                ))
            })
            .or_else(|| {
                let body_session = GenericSessionScopeAdapter.extract_scope(request)?;
                Some(ClientSessionScope::new(
                    self.family(),
                    body_session.session_id,
                    body_session.agent_id,
                    body_session.account_hint,
                    body_session.source,
                ))
            })
    }
}

impl ClientSessionScopeAdapter for OpenCodeSessionScopeAdapter {
    fn family(&self) -> &'static str {
        "opencode"
    }

    fn detect(&self, request: &ClientSessionRequest<'_>) -> bool {
        header_contains(
            request.headers,
            http::header::USER_AGENT.as_str(),
            "opencode",
        ) || header_value_str(request.headers, "x-opencode-session-id").is_some()
    }

    fn extract_scope(&self, request: &ClientSessionRequest<'_>) -> Option<ClientSessionScope> {
        let root_session = header_value_str(request.headers, "x-opencode-session-id")
            .or_else(|| header_value_str(request.headers, "session_id"))?;
        let agent_id = header_value_str(request.headers, "x-opencode-agent-id");
        Some(ClientSessionScope::new(
            self.family(),
            root_session,
            agent_id,
            None,
            ClientSessionSignalSource::Header,
        ))
    }
}

impl ClientSessionScopeAdapter for QwenCodeSessionScopeAdapter {
    fn family(&self) -> &'static str {
        "qwen_code"
    }

    fn detect(&self, request: &ClientSessionRequest<'_>) -> bool {
        header_contains(
            request.headers,
            http::header::USER_AGENT.as_str(),
            "qwencode",
        ) || header_contains(
            request.headers,
            http::header::USER_AGENT.as_str(),
            "qwen-code",
        ) || header_contains(request.headers, "x-dashscope-useragent", "qwencode")
            || header_contains(request.headers, "x-dashscope-useragent", "qwen-code")
    }

    fn extract_scope(&self, request: &ClientSessionRequest<'_>) -> Option<ClientSessionScope> {
        scoped_from_standard_session_headers(self.family(), request)
            .or_else(|| scoped_from_generic_body(self.family(), request))
    }
}

impl ClientSessionScopeAdapter for RooCodeSessionScopeAdapter {
    fn family(&self) -> &'static str {
        "roo_code"
    }

    fn detect(&self, request: &ClientSessionRequest<'_>) -> bool {
        header_contains(
            request.headers,
            http::header::USER_AGENT.as_str(),
            "roo-code",
        ) || header_contains(
            request.headers,
            http::header::USER_AGENT.as_str(),
            "roocode",
        ) || header_contains(request.headers, "originator", "roo-code")
            || header_contains(request.headers, "originator", "roocode")
    }

    fn extract_scope(&self, request: &ClientSessionRequest<'_>) -> Option<ClientSessionScope> {
        scoped_from_standard_session_headers(self.family(), request)
            .or_else(|| scoped_from_generic_body(self.family(), request))
    }
}

impl ClientSessionScopeAdapter for KiloCodeSessionScopeAdapter {
    fn family(&self) -> &'static str {
        "kilocode"
    }

    fn detect(&self, request: &ClientSessionRequest<'_>) -> bool {
        header_contains(
            request.headers,
            http::header::USER_AGENT.as_str(),
            "kilo-code",
        ) || header_contains(
            request.headers,
            http::header::USER_AGENT.as_str(),
            "kilocode",
        ) || has_header_with_prefix(request.headers, "x-kilocode-")
            || header_value_str(request.headers, "x-kilo-directory").is_some()
            || header_value_str(request.headers, "x-kilo-workspace").is_some()
    }

    fn extract_scope(&self, request: &ClientSessionRequest<'_>) -> Option<ClientSessionScope> {
        scoped_from_standard_session_headers(self.family(), request)
            .or_else(|| scoped_from_generic_body(self.family(), request))
    }
}

impl ClientSessionScopeAdapter for CherryStudioSessionScopeAdapter {
    fn family(&self) -> &'static str {
        "cherrystudio"
    }

    fn detect(&self, request: &ClientSessionRequest<'_>) -> bool {
        header_contains(
            request.headers,
            http::header::USER_AGENT.as_str(),
            "cherrystudio",
        ) || header_contains(
            request.headers,
            http::header::USER_AGENT.as_str(),
            "cherry-studio",
        ) || header_contains(
            request.headers,
            http::header::USER_AGENT.as_str(),
            "cherry studio",
        )
    }

    fn extract_scope(&self, request: &ClientSessionRequest<'_>) -> Option<ClientSessionScope> {
        scoped_from_standard_session_headers(self.family(), request)
            .or_else(|| scoped_from_generic_body(self.family(), request))
    }
}

impl ClientSessionScopeAdapter for OpenUiSessionScopeAdapter {
    fn family(&self) -> &'static str {
        "openui"
    }

    fn detect(&self, request: &ClientSessionRequest<'_>) -> bool {
        header_contains(request.headers, http::header::USER_AGENT.as_str(), "openui")
            || header_contains(
                request.headers,
                http::header::USER_AGENT.as_str(),
                "openui-agent-manager",
            )
    }

    fn extract_scope(&self, request: &ClientSessionRequest<'_>) -> Option<ClientSessionScope> {
        scoped_from_standard_session_headers(self.family(), request)
            .or_else(|| scoped_from_generic_body(self.family(), request))
    }
}

impl ClientSessionScopeAdapter for OpenAiJsSdkSessionScopeAdapter {
    fn family(&self) -> &'static str {
        "openai_js_sdk"
    }

    fn detect(&self, request: &ClientSessionRequest<'_>) -> bool {
        header_contains(
            request.headers,
            http::header::USER_AGENT.as_str(),
            "openai/js",
        ) || (header_contains(request.headers, http::header::USER_AGENT.as_str(), "/js ")
            && header_contains(request.headers, "x-stainless-lang", "js"))
    }

    fn extract_scope(&self, request: &ClientSessionRequest<'_>) -> Option<ClientSessionScope> {
        scoped_from_standard_session_headers(self.family(), request)
            .or_else(|| scoped_from_generic_body(self.family(), request))
    }
}

impl ClientSessionScopeAdapter for OpenAiPythonSdkSessionScopeAdapter {
    fn family(&self) -> &'static str {
        "openai_python_sdk"
    }

    fn detect(&self, request: &ClientSessionRequest<'_>) -> bool {
        header_contains(
            request.headers,
            http::header::USER_AGENT.as_str(),
            "openai/python",
        ) || (header_contains(
            request.headers,
            http::header::USER_AGENT.as_str(),
            "/python ",
        ) && header_contains(request.headers, "x-stainless-lang", "python")
            && header_value_str(request.headers, "anthropic-version").is_none())
    }

    fn extract_scope(&self, request: &ClientSessionRequest<'_>) -> Option<ClientSessionScope> {
        scoped_from_standard_session_headers(self.family(), request)
            .or_else(|| scoped_from_generic_body(self.family(), request))
    }
}

impl ClientSessionScopeAdapter for AnthropicJsSdkSessionScopeAdapter {
    fn family(&self) -> &'static str {
        "anthropic_js_sdk"
    }

    fn detect(&self, request: &ClientSessionRequest<'_>) -> bool {
        header_contains(
            request.headers,
            http::header::USER_AGENT.as_str(),
            "anthropic/js",
        ) || (header_contains(request.headers, http::header::USER_AGENT.as_str(), "/js ")
            && header_contains(request.headers, "x-stainless-lang", "js")
            && header_value_str(request.headers, "anthropic-version").is_some())
    }

    fn extract_scope(&self, request: &ClientSessionRequest<'_>) -> Option<ClientSessionScope> {
        scoped_from_standard_session_headers(self.family(), request)
            .or_else(|| scoped_from_generic_body(self.family(), request))
    }
}

impl ClientSessionScopeAdapter for AnthropicPythonSdkSessionScopeAdapter {
    fn family(&self) -> &'static str {
        "anthropic_python_sdk"
    }

    fn detect(&self, request: &ClientSessionRequest<'_>) -> bool {
        header_contains(
            request.headers,
            http::header::USER_AGENT.as_str(),
            "anthropic/python",
        ) || (header_contains(
            request.headers,
            http::header::USER_AGENT.as_str(),
            "/python ",
        ) && header_contains(request.headers, "x-stainless-lang", "python")
            && header_value_str(request.headers, "anthropic-version").is_some())
    }

    fn extract_scope(&self, request: &ClientSessionRequest<'_>) -> Option<ClientSessionScope> {
        scoped_from_standard_session_headers(self.family(), request)
            .or_else(|| scoped_from_generic_body(self.family(), request))
    }
}

fn scoped_from_standard_session_headers(
    client_family: &str,
    request: &ClientSessionRequest<'_>,
) -> Option<ClientSessionScope> {
    header_value_str(request.headers, "session_id")
        .or_else(|| header_value_str(request.headers, "conversation_id"))
        .map(|root_session| {
            ClientSessionScope::new(
                client_family,
                root_session,
                None,
                None,
                ClientSessionSignalSource::Header,
            )
        })
}

fn scoped_from_generic_body(
    client_family: &str,
    request: &ClientSessionRequest<'_>,
) -> Option<ClientSessionScope> {
    let body_session = GenericSessionScopeAdapter.extract_scope(request)?;
    Some(ClientSessionScope::new(
        client_family,
        body_session.session_id,
        body_session.agent_id,
        body_session.account_hint,
        body_session.source,
    ))
}

fn claude_code_session_id_from_body(body: &Value) -> Option<&str> {
    value_at_path(body, &["metadata", "user_id"]).and_then(|user_id| {
        user_id
            .rsplit_once("_session_")
            .map(|(_, session_id)| session_id.trim())
            .filter(|value| !value.is_empty())
    })
}

fn explicit_aether_session_scope(
    request: &ClientSessionRequest<'_>,
    client_family: &str,
) -> Option<ClientSessionScope> {
    let root_session = header_value_str(request.headers, AETHER_SESSION_ID_HEADER)?;
    let agent_id = header_value_str(request.headers, AETHER_AGENT_ID_HEADER);
    Some(ClientSessionScope::new(
        client_family,
        root_session,
        agent_id,
        None,
        ClientSessionSignalSource::ExplicitAetherHeader,
    ))
}

fn normalize_session_key(
    account_hint: Option<&str>,
    root_session: &str,
    agent_id: Option<&str>,
) -> String {
    let root_session = root_session.trim();
    let mut parts = Vec::new();
    if let Some(account_hint) = account_hint
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        parts.push(format!("account={account_hint}"));
    }
    parts.push(format!("session={root_session}"));
    if let Some(agent_id) = agent_id.map(str::trim).filter(|value| !value.is_empty()) {
        parts.push(format!("agent={agent_id}"));
    }
    parts.join(";")
}

fn value_at_paths<'a>(body: &'a Value, paths: &[&[&str]]) -> Option<&'a str> {
    paths.iter().find_map(|path| value_at_path(body, path))
}

fn value_at_path<'a>(body: &'a Value, path: &[&str]) -> Option<&'a str> {
    let mut current = body;
    for segment in path {
        current = current.get(*segment)?;
    }
    current
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn header_contains(headers: &http::HeaderMap, key: &str, needle: &str) -> bool {
    header_value_str(headers, key)
        .map(|value| value.to_ascii_lowercase().contains(needle))
        .unwrap_or(false)
}

fn has_header_with_prefix(headers: &http::HeaderMap, prefix: &str) -> bool {
    headers
        .keys()
        .any(|key| key.as_str().to_ascii_lowercase().starts_with(prefix))
}

#[cfg(test)]
mod tests {
    use super::{
        client_session_affinity_from_api_request,
        client_session_affinity_from_report_context_value, client_session_affinity_from_request,
        client_session_affinity_report_context_value, client_session_scope_from_request,
        ClientSessionSignalSource, AETHER_AGENT_ID_HEADER, AETHER_SESSION_ID_HEADER,
    };
    use aether_scheduler_core::ClientSessionAffinity;
    use http::{HeaderMap, HeaderValue};
    use serde_json::json;

    #[test]
    fn unknown_adapter_extracts_body_session_and_agent() {
        let body = json!({
            "metadata": {
                "session_id": "session-1",
                "agent_id": "planner"
            }
        });

        let affinity = client_session_affinity_from_request(&HeaderMap::new(), Some(&body))
            .expect("affinity should build");

        assert_eq!(affinity.client_family.as_deref(), Some("unknown"));
        assert_eq!(
            affinity.session_key.as_deref(),
            Some("session=session-1;agent=planner")
        );
    }

    #[test]
    fn explicit_aether_headers_win_over_body_session() {
        let mut headers = HeaderMap::new();
        headers.insert(
            AETHER_SESSION_ID_HEADER,
            HeaderValue::from_static("root-session"),
        );
        headers.insert(AETHER_AGENT_ID_HEADER, HeaderValue::from_static("coder"));
        headers.insert(
            http::header::USER_AGENT,
            HeaderValue::from_static("OpenCode/1.0"),
        );
        let body = json!({"session_id": "body-session"});

        let affinity = client_session_affinity_from_request(&headers, Some(&body))
            .expect("affinity should build");

        assert_eq!(affinity.client_family.as_deref(), Some("opencode"));
        assert_eq!(
            affinity.session_key.as_deref(),
            Some("session=root-session;agent=coder")
        );
    }

    #[test]
    fn codex_adapter_extracts_header_session() {
        let mut headers = HeaderMap::new();
        headers.insert(
            http::header::USER_AGENT,
            HeaderValue::from_static("codex_cli_rs/0.144.1"),
        );
        headers.insert("session-id", HeaderValue::from_static("codex-session"));
        headers.insert("thread-id", HeaderValue::from_static("codex-thread"));

        let affinity =
            client_session_affinity_from_request(&headers, None).expect("affinity should build");

        assert_eq!(affinity.client_family.as_deref(), Some("codex"));
        assert_eq!(
            affinity.session_key.as_deref(),
            Some("session=codex-session")
        );
    }

    #[test]
    fn codex_adapter_uses_body_session_instead_of_request_id() {
        let mut headers = HeaderMap::new();
        headers.insert(
            http::header::USER_AGENT,
            HeaderValue::from_static("codex_cli_rs/0.144.1"),
        );
        headers.insert(
            "x-client-request-id",
            HeaderValue::from_static("request-only-id"),
        );
        headers.insert("chatgpt-account-id", HeaderValue::from_static("account-1"));
        let body = json!({
            "prompt_cache_key": "prompt-session-1"
        });

        let scope = client_session_scope_from_request(&headers, Some(&body))
            .expect("session scope should build");
        let affinity = scope
            .scheduler_affinity()
            .expect("scheduler affinity should build");

        assert_eq!(scope.client_family, "codex");
        assert_eq!(scope.source, ClientSessionSignalSource::Body);
        assert_eq!(scope.session_id, "prompt-session-1");
        assert_eq!(scope.account_hint.as_deref(), Some("account-1"));
        assert_eq!(affinity.client_family.as_deref(), Some("codex"));
        assert_eq!(
            affinity.session_key.as_deref(),
            Some("account=account-1;session=prompt-session-1")
        );
    }

    #[test]
    fn report_context_round_trips_normalized_session_affinity() {
        let affinity = ClientSessionAffinity::new(
            Some("codex".to_string()),
            Some("account=account-1;session=session-1".to_string()),
        );

        let value = client_session_affinity_report_context_value(&affinity)
            .expect("report context value should build");
        let parsed = client_session_affinity_from_report_context_value(Some(&value))
            .expect("report context value should parse");

        assert_eq!(parsed, affinity);
    }

    #[test]
    fn claude_code_adapter_extracts_session_header() {
        let mut headers = HeaderMap::new();
        headers.insert(
            http::header::USER_AGENT,
            HeaderValue::from_static("Claude-Code/1.0"),
        );
        headers.insert(
            "x-claude-code-session-id",
            HeaderValue::from_static("claude-session"),
        );

        let affinity =
            client_session_affinity_from_request(&headers, None).expect("affinity should build");

        assert_eq!(affinity.client_family.as_deref(), Some("claude_code"));
        assert_eq!(
            affinity.session_key.as_deref(),
            Some("session=claude-session")
        );
    }

    #[test]
    fn claude_code_adapter_extracts_metadata_user_session() {
        let mut headers = HeaderMap::new();
        headers.insert(
            http::header::USER_AGENT,
            HeaderValue::from_static("Claude-Code/1.0"),
        );
        let body = json!({
            "metadata": {
                "user_id": "user-1_session_claude-real-session"
            }
        });

        let scope = client_session_scope_from_request(&headers, Some(&body))
            .expect("session scope should build");
        let affinity = scope
            .scheduler_affinity()
            .expect("scheduler affinity should build");

        assert_eq!(scope.client_family, "claude_code");
        assert_eq!(scope.source, ClientSessionSignalSource::Body);
        assert_eq!(scope.session_id, "claude-real-session");
        assert_eq!(affinity.client_family.as_deref(), Some("claude_code"));
        assert_eq!(
            affinity.session_key.as_deref(),
            Some("session=claude-real-session")
        );
    }

    #[test]
    fn detected_client_family_is_kept_for_generic_body_signal() {
        let mut headers = HeaderMap::new();
        headers.insert(
            http::header::USER_AGENT,
            HeaderValue::from_static("OpenCode/0.9"),
        );
        let body = json!({
            "metadata": {
                "session_id": "body-session",
                "agent_id": "body-agent"
            }
        });

        let scope = client_session_scope_from_request(&headers, Some(&body))
            .expect("session scope should build");
        let affinity = scope
            .scheduler_affinity()
            .expect("scheduler affinity should build");

        assert_eq!(scope.client_family, "opencode");
        assert_eq!(scope.source, ClientSessionSignalSource::Body);
        assert_eq!(scope.session_id, "body-session");
        assert_eq!(scope.agent_id.as_deref(), Some("body-agent"));
        assert_eq!(affinity.client_family.as_deref(), Some("opencode"));
        assert_eq!(
            affinity.session_key.as_deref(),
            Some("session=body-session;agent=body-agent")
        );
    }

    #[test]
    fn opencode_adapter_keeps_agent_dimension() {
        let mut headers = HeaderMap::new();
        headers.insert(
            http::header::USER_AGENT,
            HeaderValue::from_static("OpenCode/0.9"),
        );
        headers.insert(
            "x-opencode-session-id",
            HeaderValue::from_static("oc-session"),
        );
        headers.insert("x-opencode-agent-id", HeaderValue::from_static("reviewer"));

        let affinity =
            client_session_affinity_from_request(&headers, None).expect("affinity should build");

        assert_eq!(affinity.client_family.as_deref(), Some("opencode"));
        assert_eq!(
            affinity.session_key.as_deref(),
            Some("session=oc-session;agent=reviewer")
        );
    }

    #[test]
    fn qwen_code_detection_keeps_body_session() {
        let mut headers = HeaderMap::new();
        headers.insert(
            http::header::USER_AGENT,
            HeaderValue::from_static("QwenCode/0.1.0 (linux; x64)"),
        );
        let body = json!({"conversation_id": "qwen-session"});

        let scope = client_session_scope_from_request(&headers, Some(&body))
            .expect("session scope should build");

        assert_eq!(scope.client_family, "qwen_code");
        assert_eq!(scope.session_id, "qwen-session");
    }

    #[test]
    fn roo_code_detection_uses_originator_and_session_header() {
        let mut headers = HeaderMap::new();
        headers.insert("originator", HeaderValue::from_static("roo-code"));
        headers.insert("session_id", HeaderValue::from_static("roo-session"));

        let scope =
            client_session_scope_from_request(&headers, None).expect("session scope should build");

        assert_eq!(scope.client_family, "roo_code");
        assert_eq!(scope.session_id, "roo-session");
    }

    #[test]
    fn unknown_user_agent_with_session_header_stays_unknown() {
        let mut headers = HeaderMap::new();
        headers.insert(
            http::header::USER_AGENT,
            HeaderValue::from_static("CustomClient/1.0"),
        );
        headers.insert("session_id", HeaderValue::from_static("custom-session"));

        let scope =
            client_session_scope_from_request(&headers, None).expect("session scope should build");

        assert_eq!(scope.client_family, "unknown");
        assert_eq!(scope.session_id, "custom-session");
    }

    #[test]
    fn vscode_copilot_user_agent_is_not_cherrystudio_by_itself() {
        let mut headers = HeaderMap::new();
        headers.insert(
            http::header::USER_AGENT,
            HeaderValue::from_static("Visual Studio Code (desktop) GithubCopilot/1.155.0"),
        );
        headers.insert("session_id", HeaderValue::from_static("vscode-session"));

        let scope =
            client_session_scope_from_request(&headers, None).expect("session scope should build");

        assert_eq!(scope.client_family, "unknown");
        assert_eq!(scope.session_id, "vscode-session");
    }

    #[test]
    fn sdk_detection_uses_user_agent_before_stainless_fallback() {
        let mut headers = HeaderMap::new();
        headers.insert(
            http::header::USER_AGENT,
            HeaderValue::from_static("OpenAI/JS 6.0.0"),
        );
        headers.insert("x-stainless-lang", HeaderValue::from_static("js"));
        let body = json!({"metadata": {"session_id": "sdk-session"}});

        let scope = client_session_scope_from_request(&headers, Some(&body))
            .expect("session scope should build");

        assert_eq!(scope.client_family, "openai_js_sdk");
        assert_eq!(scope.session_id, "sdk-session");
    }

    #[test]
    fn specific_adapter_wins_over_generic_body_session() {
        let mut headers = HeaderMap::new();
        headers.insert(
            http::header::USER_AGENT,
            HeaderValue::from_static("OpenCode/0.9"),
        );
        headers.insert(
            "x-opencode-session-id",
            HeaderValue::from_static("oc-session"),
        );
        headers.insert("x-opencode-agent-id", HeaderValue::from_static("reviewer"));
        let body = json!({
            "session_id": "body-session",
            "agent_id": "body-agent"
        });

        let affinity = client_session_affinity_from_request(&headers, Some(&body))
            .expect("affinity should build");

        assert_eq!(affinity.client_family.as_deref(), Some("opencode"));
        assert_eq!(
            affinity.session_key.as_deref(),
            Some("session=oc-session;agent=reviewer")
        );
    }

    #[test]
    fn missing_session_signal_returns_none() {
        let headers = HeaderMap::new();
        let body = json!({"model": "gpt-5"});

        assert!(client_session_affinity_from_request(&headers, Some(&body)).is_none());
    }

    #[test]
    fn codex_search_uses_request_id_as_session_affinity() {
        let mut headers = HeaderMap::new();
        headers.insert("chatgpt-account-id", HeaderValue::from_static("account-1"));
        let body = json!({"id": "codex-session-1", "model": "gpt-5.6"});

        let affinity =
            client_session_affinity_from_api_request("openai:search", &headers, Some(&body))
                .expect("search affinity should build");

        assert_eq!(affinity.client_family.as_deref(), Some("codex"));
        assert_eq!(
            affinity.session_key.as_deref(),
            Some("account=account-1;session=codex-session-1")
        );
    }

    #[test]
    fn top_level_request_id_is_not_a_generic_session_signal() {
        let body = json!({"id": "request-id", "model": "gpt-5.6"});

        assert!(client_session_affinity_from_request(&HeaderMap::new(), Some(&body)).is_none());
    }
}
