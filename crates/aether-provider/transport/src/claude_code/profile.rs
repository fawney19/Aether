use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, OnceLock, RwLock};

use aether_ai_formats::ApiOperation;

pub const CLAUDE_CODE_CONTEXT_MANAGEMENT_BETA: &str = "context-management-2025-06-27";

/// Built-in Claude Code CLI version used until the gateway publishes a newer
/// verified release (or when release checks are disabled / unavailable).
pub const CLAUDE_CODE_BUILTIN_CLI_VERSION: &str = "2.1.284";

const MESSAGE_BETAS_2026_04: &[&str] = &[
    "claude-code-20250219",
    "oauth-2025-04-20",
    "interleaved-thinking-2025-05-14",
    "prompt-caching-scope-2026-01-05",
    "effort-2025-11-24",
    CLAUDE_CODE_CONTEXT_MANAGEMENT_BETA,
    "extended-cache-ttl-2025-04-11",
];
const COUNT_TOKENS_BETAS_2026_04: &[&str] = &[
    "claude-code-20250219",
    "oauth-2025-04-20",
    "interleaved-thinking-2025-05-14",
    "prompt-caching-scope-2026-01-05",
    "effort-2025-11-24",
    CLAUDE_CODE_CONTEXT_MANAGEMENT_BETA,
    "extended-cache-ttl-2025-04-11",
    "token-counting-2024-11-01",
];
const DROPPED_BETAS_2026_04: &[&str] = &[];
const BODY_CAPABILITY_GATES_2026_04: &[ClaudeCodeBodyCapabilityGate] =
    &[ClaudeCodeBodyCapabilityGate {
        body_field: "context_management",
        beta_token: CLAUDE_CODE_CONTEXT_MANAGEMENT_BETA,
        inject_when_thinking_enabled: true,
        default_edit_type: Some("clear_thinking_20251015"),
    }];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaudeCodeTransportIdentityProfileVersion {
    V2026_04,
}

impl ClaudeCodeTransportIdentityProfileVersion {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::V2026_04 => "2026-04",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClaudeCodeBodyCapabilityGate {
    pub body_field: &'static str,
    pub beta_token: &'static str,
    pub inject_when_thinking_enabled: bool,
    pub default_edit_type: Option<&'static str>,
}

/// Dynamic Claude Code CLI identity (version-derived wire values).
///
/// The gateway refreshes this from verified official releases in a background
/// task; request paths only read an immutable snapshot and never touch the
/// network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeCodeClientProfile {
    pub cli_version: String,
    pub user_agent: String,
}

impl ClaudeCodeClientProfile {
    /// Builds a CLI profile from a release version. Release verification is owned
    /// by the release checker; the constructor only rejects clearly invalid values.
    pub fn cli(version: &str) -> Result<Self, &'static str> {
        let version = version.trim();
        if version.is_empty()
            || version.len() > 64
            || !version.bytes().all(|byte| (33..=126).contains(&byte))
        {
            return Err("invalid Claude Code CLI version");
        }
        Ok(Self {
            cli_version: version.to_owned(),
            user_agent: format!("claude-cli/{version} (external, cli)"),
        })
    }
}

impl Default for ClaudeCodeClientProfile {
    fn default() -> Self {
        Self::cli(CLAUDE_CODE_BUILTIN_CLI_VERSION)
            .expect("built-in Claude Code CLI profile must be valid")
    }
}

static ACTIVE_CLIENT_PROFILE: OnceLock<RwLock<Arc<ClaudeCodeClientProfile>>> = OnceLock::new();

fn active_client_profile() -> &'static RwLock<Arc<ClaudeCodeClientProfile>> {
    ACTIVE_CLIENT_PROFILE.get_or_init(|| RwLock::new(Arc::new(ClaudeCodeClientProfile::default())))
}

/// Returns a snapshot of the active CLI profile without holding the global lock.
pub fn claude_code_client_profile() -> Arc<ClaudeCodeClientProfile> {
    active_client_profile()
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

/// Atomically replaces the active CLI profile and returns the previous one.
pub fn set_claude_code_client_profile(
    profile: ClaudeCodeClientProfile,
) -> Arc<ClaudeCodeClientProfile> {
    let mut current = active_client_profile()
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    std::mem::replace(&mut *current, Arc::new(profile))
}

/// Publishes a CLI profile for the given release version.
pub fn set_claude_code_cli_version(
    version: &str,
) -> Result<Arc<ClaudeCodeClientProfile>, &'static str> {
    let profile = ClaudeCodeClientProfile::cli(version)?;
    Ok(set_claude_code_client_profile(profile))
}

/// Returns the active Claude Code CLI version.
pub fn claude_code_client_version() -> String {
    claude_code_client_profile().cli_version.clone()
}

/// Returns the active Claude Code CLI User-Agent.
pub fn claude_code_client_user_agent() -> String {
    claude_code_client_profile().user_agent.clone()
}

/// Versioned, static part of the Claude Code transport identity: Stainless SDK
/// and runtime values, beta policy and body capability gates. These values are
/// calibrated together and change only with a new template version; the CLI
/// version itself is supplied by the dynamic [`ClaudeCodeClientProfile`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClaudeCodeTransportIdentityTemplate {
    version: ClaudeCodeTransportIdentityProfileVersion,
    transport_profile_id: &'static str,
    anthropic_version: &'static str,
    stainless_lang: &'static str,
    stainless_package_version: &'static str,
    stainless_os: &'static str,
    stainless_arch: &'static str,
    stainless_runtime: &'static str,
    stainless_runtime_version: &'static str,
    stainless_retry_count: &'static str,
    stainless_timeout: &'static str,
    message_required_betas: &'static [&'static str],
    count_tokens_required_betas: &'static [&'static str],
    preserve_incoming_betas: bool,
    dropped_betas: &'static [&'static str],
    body_capability_gates: &'static [ClaudeCodeBodyCapabilityGate],
}

pub const CLAUDE_CODE_TRANSPORT_IDENTITY_2026_04: ClaudeCodeTransportIdentityTemplate =
    ClaudeCodeTransportIdentityTemplate {
        version: ClaudeCodeTransportIdentityProfileVersion::V2026_04,
        transport_profile_id: "claude_code_nodejs",
        anthropic_version: "2023-06-01",
        stainless_lang: "js",
        stainless_package_version: "0.112.1",
        stainless_os: "Linux",
        stainless_arch: "arm64",
        stainless_runtime: "node",
        stainless_runtime_version: "v26.3.0",
        stainless_retry_count: "0",
        stainless_timeout: "600",
        message_required_betas: MESSAGE_BETAS_2026_04,
        count_tokens_required_betas: COUNT_TOKENS_BETAS_2026_04,
        preserve_incoming_betas: true,
        dropped_betas: DROPPED_BETAS_2026_04,
        body_capability_gates: BODY_CAPABILITY_GATES_2026_04,
    };

/// Upstream identity used when Aether is intentionally acting as a Claude Code
/// transport. Native Anthropic transports never resolve this profile and
/// therefore keep their original headers and body untouched.
///
/// Each value is one coherent snapshot of the static template plus the active
/// CLI profile, so headers and billing attribution derived from the same
/// snapshot always agree on the CLI version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeCodeTransportIdentityProfile {
    template: &'static ClaudeCodeTransportIdentityTemplate,
    client: Arc<ClaudeCodeClientProfile>,
}

pub fn current_claude_code_transport_identity_profile() -> ClaudeCodeTransportIdentityProfile {
    ClaudeCodeTransportIdentityProfile::new(
        &CLAUDE_CODE_TRANSPORT_IDENTITY_2026_04,
        claude_code_client_profile(),
    )
}

impl ClaudeCodeTransportIdentityProfile {
    pub fn new(
        template: &'static ClaudeCodeTransportIdentityTemplate,
        client: Arc<ClaudeCodeClientProfile>,
    ) -> Self {
        Self { template, client }
    }

    pub fn version(&self) -> ClaudeCodeTransportIdentityProfileVersion {
        self.template.version
    }

    pub fn transport_profile_id(&self) -> &'static str {
        self.template.transport_profile_id
    }

    pub fn cli_version(&self) -> &str {
        &self.client.cli_version
    }

    pub fn billing_cli_version(&self) -> &str {
        &self.client.cli_version
    }

    pub fn user_agent(&self) -> &str {
        &self.client.user_agent
    }

    pub fn stainless_package_version(&self) -> &'static str {
        self.template.stainless_package_version
    }

    pub fn stainless_lang(&self) -> &'static str {
        self.template.stainless_lang
    }

    pub fn stainless_os(&self) -> &'static str {
        self.template.stainless_os
    }

    pub fn stainless_arch(&self) -> &'static str {
        self.template.stainless_arch
    }

    pub fn stainless_runtime(&self) -> &'static str {
        self.template.stainless_runtime
    }

    pub fn stainless_runtime_version(&self) -> &'static str {
        self.template.stainless_runtime_version
    }

    pub fn stainless_retry_count(&self) -> &'static str {
        self.template.stainless_retry_count
    }

    pub fn stainless_timeout(&self) -> &'static str {
        self.template.stainless_timeout
    }

    pub fn required_beta_tokens(&self, operation: Option<ApiOperation>) -> &'static [&'static str] {
        if operation == Some(ApiOperation::ClaudeCountTokens) {
            self.template.count_tokens_required_betas
        } else {
            self.template.message_required_betas
        }
    }

    pub fn preserves_incoming_betas(&self) -> bool {
        self.template.preserve_incoming_betas
    }

    pub fn dropped_beta_tokens(&self) -> &'static [&'static str] {
        self.template.dropped_betas
    }

    pub fn body_capability_gates(&self) -> &'static [ClaudeCodeBodyCapabilityGate] {
        self.template.body_capability_gates
    }

    pub fn body_capability_gate(&self, field: &str) -> Option<ClaudeCodeBodyCapabilityGate> {
        self.template
            .body_capability_gates
            .iter()
            .copied()
            .find(|gate| gate.body_field == field)
    }

    pub fn apply_fixed_headers(&self, headers: &mut BTreeMap<String, String>, stream: bool) {
        let template = self.template;
        for (name, value) in [
            ("accept", "application/json"),
            ("anthropic-version", template.anthropic_version),
            ("anthropic-dangerous-direct-browser-access", "true"),
            ("x-app", "cli"),
            ("x-stainless-lang", template.stainless_lang),
            (
                "x-stainless-package-version",
                template.stainless_package_version,
            ),
            ("x-stainless-os", template.stainless_os),
            ("x-stainless-arch", template.stainless_arch),
            ("x-stainless-runtime", template.stainless_runtime),
            (
                "x-stainless-runtime-version",
                template.stainless_runtime_version,
            ),
            ("x-stainless-retry-count", template.stainless_retry_count),
            ("x-stainless-timeout", template.stainless_timeout),
        ] {
            headers.insert(name.to_string(), value.to_string());
        }
        headers.insert("user-agent".to_string(), self.user_agent().to_string());
        if stream {
            headers.insert(
                "x-stainless-helper-method".to_string(),
                "stream".to_string(),
            );
        } else {
            headers.remove("x-stainless-helper-method");
        }
    }

    pub fn apply_beta_policy(
        &self,
        headers: &mut BTreeMap<String, String>,
        operation: Option<ApiOperation>,
    ) {
        let incoming = headers.get("anthropic-beta").map(String::as_str);
        let merged = self.merge_beta_tokens(incoming, operation);
        if merged.is_empty() {
            headers.remove("anthropic-beta");
        } else {
            headers.insert("anthropic-beta".to_string(), merged);
        }
    }

    pub fn merge_beta_tokens(
        &self,
        incoming: Option<&str>,
        operation: Option<ApiOperation>,
    ) -> String {
        let mut seen = BTreeSet::new();
        let mut merged = Vec::new();

        for token in self.required_beta_tokens(operation) {
            self.append_beta_token(&mut seen, &mut merged, token);
        }
        if self.template.preserve_incoming_betas {
            for token in incoming.unwrap_or_default().split(',') {
                self.append_beta_token(&mut seen, &mut merged, token);
            }
        }
        merged.join(",")
    }

    pub fn beta_header_enables_body_field(&self, beta_header: &str, field: &str) -> bool {
        let Some(gate) = self.body_capability_gate(field) else {
            return true;
        };
        beta_header
            .split(',')
            .map(str::trim)
            .any(|token| token.eq_ignore_ascii_case(gate.beta_token))
    }

    fn append_beta_token(
        &self,
        seen: &mut BTreeSet<String>,
        merged: &mut Vec<String>,
        token: &str,
    ) {
        let token = token.trim();
        if token.is_empty()
            || self
                .template
                .dropped_betas
                .iter()
                .any(|dropped| token.eq_ignore_ascii_case(dropped))
        {
            return;
        }
        if seen.insert(token.to_ascii_lowercase()) {
            merged.push(token.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{
        current_claude_code_transport_identity_profile, ClaudeCodeClientProfile,
        ClaudeCodeTransportIdentityProfile, CLAUDE_CODE_TRANSPORT_IDENTITY_2026_04,
    };
    use aether_ai_formats::ApiOperation;

    #[test]
    fn profile_versions_cli_user_agent_stainless_and_billing_together() {
        let profile = current_claude_code_transport_identity_profile();

        assert_eq!(profile.version().as_str(), "2026-04");
        assert_eq!(profile.cli_version(), "2.1.284");
        assert_eq!(profile.billing_cli_version(), profile.cli_version());
        assert_eq!(
            profile.user_agent(),
            format!("claude-cli/{} (external, cli)", profile.cli_version())
        );
        assert_eq!(profile.stainless_package_version(), "0.112.1");
        assert_eq!(profile.stainless_runtime_version(), "v26.3.0");
    }

    #[test]
    fn cli_profile_derives_wire_identity_from_version() {
        let client = ClaudeCodeClientProfile::cli(" 2.1.300 ").expect("valid version");
        assert_eq!(client.cli_version, "2.1.300");
        assert_eq!(client.user_agent, "claude-cli/2.1.300 (external, cli)");

        // A snapshot keeps headers and billing attribution on the same CLI version
        // without mutating the process-wide active profile.
        let profile = ClaudeCodeTransportIdentityProfile::new(
            &CLAUDE_CODE_TRANSPORT_IDENTITY_2026_04,
            Arc::new(client),
        );
        let mut headers = std::collections::BTreeMap::new();
        profile.apply_fixed_headers(&mut headers, false);
        assert_eq!(
            headers.get("user-agent").map(String::as_str),
            Some("claude-cli/2.1.300 (external, cli)")
        );
        assert_eq!(profile.billing_cli_version(), "2.1.300");
        assert_eq!(
            headers
                .get("x-stainless-package-version")
                .map(String::as_str),
            Some("0.112.1")
        );
    }

    #[test]
    fn cli_profile_rejects_empty_or_control_values() {
        assert!(ClaudeCodeClientProfile::cli("").is_err());
        assert!(ClaudeCodeClientProfile::cli("2.1.0\nspoof").is_err());
        assert!(ClaudeCodeClientProfile::cli("2.1.0 spoof").is_err());
    }

    #[test]
    fn profile_preserves_context_1m_and_adds_operation_specific_betas() {
        let profile = current_claude_code_transport_identity_profile();
        let messages = profile.merge_beta_tokens(Some("context-1m-2025-08-07,custom"), None);

        assert!(messages
            .split(',')
            .any(|token| token == "context-1m-2025-08-07"));
        assert!(messages.split(',').any(|token| token == "custom"));
        assert!(!messages
            .split(',')
            .any(|token| token == "token-counting-2024-11-01"));

        let count_tokens = profile.merge_beta_tokens(
            Some("context-1m-2025-08-07"),
            Some(ApiOperation::ClaudeCountTokens),
        );
        assert!(count_tokens
            .split(',')
            .any(|token| token == "token-counting-2024-11-01"));
        assert!(profile.dropped_beta_tokens().is_empty());
        assert!(profile.preserves_incoming_betas());
    }
}
