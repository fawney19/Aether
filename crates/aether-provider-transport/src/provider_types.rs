#[derive(Debug, Clone, Copy)]
pub struct ProviderOAuthTemplate {
    pub provider_type: &'static str,
    pub display_name: &'static str,
    pub authorize_url: &'static str,
    pub token_url: &'static str,
    pub client_id: &'static str,
    pub client_secret: &'static str,
    pub scopes: &'static [&'static str],
    pub redirect_uri: &'static str,
    pub use_pkce: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FixedProviderEndpointConfigValue {
    String(&'static str),
    Bool(bool),
    I64(i64),
}

impl FixedProviderEndpointConfigValue {
    pub fn to_json_value(self) -> serde_json::Value {
        match self {
            Self::String(value) => serde_json::Value::String(value.to_string()),
            Self::Bool(value) => serde_json::Value::Bool(value),
            Self::I64(value) => serde_json::Value::Number(value.into()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FixedProviderEndpointConfigDefault {
    pub key: &'static str,
    pub value: FixedProviderEndpointConfigValue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FixedProviderEndpointTemplate {
    pub item_key: &'static str,
    pub api_format: &'static str,
    pub custom_path: Option<&'static str>,
    pub config_defaults: &'static [FixedProviderEndpointConfigDefault],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FixedProviderTemplate {
    pub provider_type: &'static str,
    pub version: u32,
    pub base_url: &'static str,
    pub endpoints: &'static [FixedProviderEndpointTemplate],
}

const EMPTY_ENDPOINT_CONFIG_DEFAULTS: &[FixedProviderEndpointConfigDefault] = &[];
const FORCE_STREAM_ENDPOINT_CONFIG_DEFAULTS: &[FixedProviderEndpointConfigDefault] =
    &[FixedProviderEndpointConfigDefault {
        key: "upstream_stream_policy",
        value: FixedProviderEndpointConfigValue::String("force_stream"),
    }];

const CLAUDE_CODE_FIXED_PROVIDER_TEMPLATE: FixedProviderTemplate = FixedProviderTemplate {
    provider_type: "claude_code",
    version: 1,
    base_url: "https://api.anthropic.com",
    endpoints: &[FixedProviderEndpointTemplate {
        item_key: "claude:messages",
        api_format: "claude:messages",
        custom_path: None,
        config_defaults: EMPTY_ENDPOINT_CONFIG_DEFAULTS,
    }],
};

const CODEX_FIXED_PROVIDER_TEMPLATE: FixedProviderTemplate = FixedProviderTemplate {
    provider_type: "codex",
    version: 1,
    base_url: "https://chatgpt.com/backend-api/codex",
    endpoints: &[
        FixedProviderEndpointTemplate {
            item_key: "openai:responses",
            api_format: "openai:responses",
            custom_path: None,
            config_defaults: FORCE_STREAM_ENDPOINT_CONFIG_DEFAULTS,
        },
        FixedProviderEndpointTemplate {
            item_key: "openai:responses:compact",
            api_format: "openai:responses:compact",
            custom_path: None,
            config_defaults: EMPTY_ENDPOINT_CONFIG_DEFAULTS,
        },
        FixedProviderEndpointTemplate {
            item_key: "openai:image",
            api_format: "openai:image",
            custom_path: None,
            config_defaults: FORCE_STREAM_ENDPOINT_CONFIG_DEFAULTS,
        },
    ],
};

const KIRO_FIXED_PROVIDER_TEMPLATE: FixedProviderTemplate = FixedProviderTemplate {
    provider_type: "kiro",
    version: 1,
    base_url: "https://q.{region}.amazonaws.com",
    endpoints: &[FixedProviderEndpointTemplate {
        item_key: "claude:messages",
        api_format: "claude:messages",
        custom_path: None,
        config_defaults: EMPTY_ENDPOINT_CONFIG_DEFAULTS,
    }],
};

const GEMINI_CLI_FIXED_PROVIDER_TEMPLATE: FixedProviderTemplate = FixedProviderTemplate {
    provider_type: "gemini_cli",
    version: 1,
    base_url: "https://cloudcode-pa.googleapis.com",
    endpoints: &[FixedProviderEndpointTemplate {
        item_key: "gemini:generate_content",
        api_format: "gemini:generate_content",
        custom_path: None,
        config_defaults: EMPTY_ENDPOINT_CONFIG_DEFAULTS,
    }],
};

const VERTEX_AI_FIXED_PROVIDER_TEMPLATE: FixedProviderTemplate = FixedProviderTemplate {
    provider_type: "vertex_ai",
    version: 1,
    base_url: "https://aiplatform.googleapis.com",
    endpoints: &[
        FixedProviderEndpointTemplate {
            item_key: "gemini:generate_content",
            api_format: "gemini:generate_content",
            custom_path: None,
            config_defaults: EMPTY_ENDPOINT_CONFIG_DEFAULTS,
        },
        FixedProviderEndpointTemplate {
            item_key: "claude:messages",
            api_format: "claude:messages",
            custom_path: None,
            config_defaults: EMPTY_ENDPOINT_CONFIG_DEFAULTS,
        },
    ],
};

const ANTIGRAVITY_FIXED_PROVIDER_TEMPLATE: FixedProviderTemplate = FixedProviderTemplate {
    provider_type: "antigravity",
    version: 1,
    base_url: "https://cloudcode-pa.googleapis.com",
    endpoints: &[FixedProviderEndpointTemplate {
        item_key: "gemini:generate_content",
        api_format: "gemini:generate_content",
        custom_path: None,
        config_defaults: EMPTY_ENDPOINT_CONFIG_DEFAULTS,
    }],
};

pub fn provider_type_is_fixed(provider_type: &str) -> bool {
    matches!(
        provider_type.trim().to_ascii_lowercase().as_str(),
        "claude_code" | "kiro" | "codex" | "gemini_cli" | "antigravity" | "vertex_ai"
    )
}

pub fn fixed_provider_key_inherits_api_formats(
    provider_type: &str,
    auth_type: &str,
    decrypted_auth_config: Option<&str>,
) -> bool {
    let provider_type = provider_type.trim().to_ascii_lowercase();
    let auth_type = auth_type.trim().to_ascii_lowercase();
    provider_type_is_fixed(&provider_type)
        && (auth_type == "oauth"
            || provider_type == "kiro"
                && auth_type == "bearer"
                && decrypted_auth_config
                    .map(str::trim)
                    .is_some_and(|value| !value.is_empty()))
}

pub fn provider_type_enables_format_conversion_by_default(provider_type: &str) -> bool {
    matches!(
        provider_type.trim().to_ascii_lowercase().as_str(),
        "claude_code" | "kiro" | "codex" | "antigravity" | "vertex_ai"
    )
}

pub fn fixed_provider_template(provider_type: &str) -> Option<&'static FixedProviderTemplate> {
    match provider_type.trim().to_ascii_lowercase().as_str() {
        "claude_code" => Some(&CLAUDE_CODE_FIXED_PROVIDER_TEMPLATE),
        "codex" => Some(&CODEX_FIXED_PROVIDER_TEMPLATE),
        "kiro" => Some(&KIRO_FIXED_PROVIDER_TEMPLATE),
        "gemini_cli" => Some(&GEMINI_CLI_FIXED_PROVIDER_TEMPLATE),
        "vertex_ai" => Some(&VERTEX_AI_FIXED_PROVIDER_TEMPLATE),
        "antigravity" => Some(&ANTIGRAVITY_FIXED_PROVIDER_TEMPLATE),
        _ => None,
    }
}

pub fn fixed_provider_endpoint_template_by_api_format(
    provider_type: &str,
    api_format: &str,
) -> Option<&'static FixedProviderEndpointTemplate> {
    let normalized = aether_ai_formats::normalize_api_format_alias(api_format);
    fixed_provider_template(provider_type)?
        .endpoints
        .iter()
        .find(|item| item.api_format.eq_ignore_ascii_case(&normalized))
}

pub fn provider_type_supports_model_fetch(provider_type: &str) -> bool {
    !matches!(
        provider_type.trim().to_ascii_lowercase().as_str(),
        "vertex_ai" | "antigravity" | "codex" | "kiro" | "claude_code"
    )
}

pub fn provider_type_supports_local_openai_chat_transport(provider_type: &str) -> bool {
    !matches!(
        provider_type.trim().to_ascii_lowercase().as_str(),
        "antigravity" | "claude_code" | "codex" | "gemini_cli" | "kiro" | "vertex_ai"
    )
}

pub fn provider_type_supports_local_same_format_transport(provider_type: &str) -> bool {
    !matches!(
        provider_type.trim().to_ascii_lowercase().as_str(),
        "antigravity" | "claude_code" | "kiro" | "vertex_ai"
    )
}

pub fn is_codex_cli_backend_url(url: &str) -> bool {
    let url = url.trim().to_ascii_lowercase();
    url.contains("/codex") && (url.contains("/backend-api/") || url.contains("/backendapi/"))
}

pub fn provider_type_is_fixed_for_admin_oauth(provider_type: &str) -> bool {
    provider_type_is_fixed(provider_type)
}

pub fn provider_type_admin_oauth_template(provider_type: &str) -> Option<ProviderOAuthTemplate> {
    match provider_type.trim().to_ascii_lowercase().as_str() {
        "claude_code" => Some(ProviderOAuthTemplate {
            provider_type: "claude_code",
            display_name: "ClaudeCode",
            authorize_url: "https://claude.ai/oauth/authorize",
            token_url: "https://console.anthropic.com/v1/oauth/token",
            client_id: "9d1c250a-e61b-44d9-88ed-5944d1962f5e",
            client_secret: "",
            scopes: &["org:create_api_key", "user:profile", "user:inference"],
            redirect_uri: "http://localhost:54545/callback",
            use_pkce: true,
        }),
        "codex" => Some(ProviderOAuthTemplate {
            provider_type: "codex",
            display_name: "Codex",
            authorize_url: "https://auth.openai.com/oauth/authorize",
            token_url: "https://auth.openai.com/oauth/token",
            client_id: "app_EMoamEEZ73f0CkXaXp7hrann",
            client_secret: "",
            scopes: &["openid", "email", "profile", "offline_access"],
            redirect_uri: "http://localhost:1455/auth/callback",
            use_pkce: true,
        }),
        "gemini_cli" => Some(ProviderOAuthTemplate {
            provider_type: "gemini_cli",
            display_name: "GeminiCli",
            authorize_url: "https://accounts.google.com/o/oauth2/v2/auth",
            token_url: "https://oauth2.googleapis.com/token",
            client_id: "681255809395-oo8ft2oprdrnp9e3aqf6av3hmdib135j.apps.googleusercontent.com",
            client_secret: "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",
            scopes: &[
                "https://www.googleapis.com/auth/cloud-platform",
                "https://www.googleapis.com/auth/userinfo.email",
                "https://www.googleapis.com/auth/userinfo.profile",
            ],
            redirect_uri: "http://localhost:8085/oauth2callback",
            use_pkce: false,
        }),
        "antigravity" => Some(ProviderOAuthTemplate {
            provider_type: "antigravity",
            display_name: "Antigravity",
            authorize_url: "https://accounts.google.com/o/oauth2/v2/auth",
            token_url: "https://oauth2.googleapis.com/token",
            client_id: "1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com",
            client_secret: "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",
            scopes: &[
                "https://www.googleapis.com/auth/cloud-platform",
                "https://www.googleapis.com/auth/userinfo.email",
                "https://www.googleapis.com/auth/userinfo.profile",
                "https://www.googleapis.com/auth/cclog",
                "https://www.googleapis.com/auth/experimentsandconfigs",
            ],
            redirect_uri: "http://localhost:51121/oauth2callback",
            use_pkce: true,
        }),
        _ => None,
    }
}

pub const ADMIN_PROVIDER_OAUTH_TEMPLATE_TYPES: &[&str] =
    &["claude_code", "codex", "gemini_cli", "antigravity"];

#[cfg(test)]
mod tests {
    use super::{
        fixed_provider_endpoint_template_by_api_format, fixed_provider_key_inherits_api_formats,
        fixed_provider_template, FixedProviderEndpointConfigValue,
    };

    #[test]
    fn codex_fixed_provider_template_includes_openai_image() {
        let template = fixed_provider_template("codex").expect("codex template should exist");
        assert_eq!(template.base_url, "https://chatgpt.com/backend-api/codex");
        assert_eq!(template.version, 1);
        assert_eq!(
            template
                .endpoints
                .iter()
                .map(|item| item.api_format)
                .collect::<Vec<_>>(),
            vec![
                "openai:responses",
                "openai:responses:compact",
                "openai:image"
            ]
        );

        let image_template =
            fixed_provider_endpoint_template_by_api_format("codex", "openai:image")
                .expect("codex image endpoint should exist");
        assert_eq!(
            image_template
                .config_defaults
                .iter()
                .map(|item| (item.key, item.value))
                .collect::<Vec<_>>(),
            vec![(
                "upstream_stream_policy",
                FixedProviderEndpointConfigValue::String("force_stream")
            )]
        );
    }

    #[test]
    fn fixed_provider_key_inheritance_keeps_oauth_and_kiro_configured_bearer_keys_open() {
        assert!(fixed_provider_key_inherits_api_formats(
            "codex", "oauth", None
        ));
        assert!(fixed_provider_key_inherits_api_formats(
            "kiro",
            "bearer",
            Some("{}")
        ));
        assert!(!fixed_provider_key_inherits_api_formats(
            "kiro", "bearer", None
        ));
        assert!(!fixed_provider_key_inherits_api_formats(
            "custom", "oauth", None
        ));
    }
}
