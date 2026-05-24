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
pub enum ProviderApiFormatInheritance {
    None,
    OAuth,
    OAuthOrBearer,
    OAuthOrServiceAccount,
    OAuthOrConfiguredBearer,
}

impl ProviderApiFormatInheritance {
    pub fn key_inherits_api_formats(
        self,
        auth_type: &str,
        decrypted_auth_config: Option<&str>,
    ) -> bool {
        let auth_type = auth_type.trim().to_ascii_lowercase();
        match self {
            Self::None => false,
            Self::OAuth => auth_type == "oauth",
            Self::OAuthOrBearer => auth_type == "oauth" || auth_type == "bearer",
            Self::OAuthOrServiceAccount => {
                auth_type == "oauth" || auth_type == "service_account" || auth_type == "vertex_ai"
            }
            Self::OAuthOrConfiguredBearer => {
                auth_type == "oauth"
                    || auth_type == "bearer"
                        && decrypted_auth_config
                            .map(str::trim)
                            .is_some_and(|value| !value.is_empty())
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderLocalEmbeddingSupport {
    None,
    AnyKnown,
    OpenAi,
    Gemini,
    Jina,
    Doubao,
}

impl ProviderLocalEmbeddingSupport {
    pub fn supports_api_format(self, api_format: &str) -> bool {
        let api_format = aether_ai_formats::normalize_api_format_alias(api_format);
        match self {
            Self::None => false,
            Self::AnyKnown => matches!(
                api_format.as_str(),
                "openai:embedding"
                    | "openai:rerank"
                    | "gemini:embedding"
                    | "jina:embedding"
                    | "jina:rerank"
                    | "doubao:embedding"
            ),
            Self::OpenAi => matches!(api_format.as_str(), "openai:embedding" | "openai:rerank"),
            Self::Gemini => api_format == "gemini:embedding",
            Self::Jina => matches!(api_format.as_str(), "jina:embedding" | "jina:rerank"),
            Self::Doubao => api_format == "doubao:embedding",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderRuntimePolicy {
    pub fixed_provider: bool,
    pub api_format_inheritance: ProviderApiFormatInheritance,
    pub enable_format_conversion_by_default: bool,
    pub allow_auth_channel_mismatch_by_default: bool,
    pub oauth_is_bearer_like: bool,
    pub supports_model_fetch: bool,
    pub supports_local_openai_chat_transport: bool,
    pub supports_local_same_format_transport: bool,
    pub local_embedding_support: ProviderLocalEmbeddingSupport,
}

impl ProviderRuntimePolicy {
    pub const fn standard() -> Self {
        Self {
            fixed_provider: false,
            api_format_inheritance: ProviderApiFormatInheritance::None,
            enable_format_conversion_by_default: false,
            allow_auth_channel_mismatch_by_default: false,
            oauth_is_bearer_like: false,
            supports_model_fetch: true,
            supports_local_openai_chat_transport: true,
            supports_local_same_format_transport: true,
            local_embedding_support: ProviderLocalEmbeddingSupport::None,
        }
    }

    pub fn key_inherits_api_formats(
        self,
        auth_type: &str,
        decrypted_auth_config: Option<&str>,
    ) -> bool {
        self.api_format_inheritance
            .key_inherits_api_formats(auth_type, decrypted_auth_config)
    }

    pub fn supports_local_embedding_transport(self, api_format: &str) -> bool {
        self.local_embedding_support.supports_api_format(api_format)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FixedProviderTemplate {
    pub provider_type: &'static str,
    pub version: u32,
    pub base_url: &'static str,
    pub endpoints: &'static [FixedProviderEndpointTemplate],
    pub runtime_policy: ProviderRuntimePolicy,
}

const EMPTY_ENDPOINT_CONFIG_DEFAULTS: &[FixedProviderEndpointConfigDefault] = &[];
const FORCE_STREAM_ENDPOINT_CONFIG_DEFAULTS: &[FixedProviderEndpointConfigDefault] =
    &[FixedProviderEndpointConfigDefault {
        key: "upstream_stream_policy",
        value: FixedProviderEndpointConfigValue::String("force_stream"),
    }];

const STANDARD_RUNTIME_POLICY: ProviderRuntimePolicy = ProviderRuntimePolicy::standard();
const CUSTOM_RUNTIME_POLICY: ProviderRuntimePolicy = ProviderRuntimePolicy {
    local_embedding_support: ProviderLocalEmbeddingSupport::AnyKnown,
    ..STANDARD_RUNTIME_POLICY
};
const OPENAI_RUNTIME_POLICY: ProviderRuntimePolicy = ProviderRuntimePolicy {
    local_embedding_support: ProviderLocalEmbeddingSupport::OpenAi,
    ..STANDARD_RUNTIME_POLICY
};
const GEMINI_RUNTIME_POLICY: ProviderRuntimePolicy = ProviderRuntimePolicy {
    local_embedding_support: ProviderLocalEmbeddingSupport::Gemini,
    ..STANDARD_RUNTIME_POLICY
};
const JINA_RUNTIME_POLICY: ProviderRuntimePolicy = ProviderRuntimePolicy {
    local_embedding_support: ProviderLocalEmbeddingSupport::Jina,
    ..STANDARD_RUNTIME_POLICY
};
const DOUBAO_RUNTIME_POLICY: ProviderRuntimePolicy = ProviderRuntimePolicy {
    local_embedding_support: ProviderLocalEmbeddingSupport::Doubao,
    ..STANDARD_RUNTIME_POLICY
};

const CLAUDE_CODE_RUNTIME_POLICY: ProviderRuntimePolicy = ProviderRuntimePolicy {
    fixed_provider: true,
    api_format_inheritance: ProviderApiFormatInheritance::OAuth,
    enable_format_conversion_by_default: true,
    oauth_is_bearer_like: true,
    supports_model_fetch: false,
    supports_local_openai_chat_transport: false,
    supports_local_same_format_transport: false,
    ..STANDARD_RUNTIME_POLICY
};
const CODEX_RUNTIME_POLICY: ProviderRuntimePolicy = ProviderRuntimePolicy {
    fixed_provider: true,
    api_format_inheritance: ProviderApiFormatInheritance::OAuth,
    enable_format_conversion_by_default: true,
    supports_model_fetch: false,
    supports_local_openai_chat_transport: false,
    ..STANDARD_RUNTIME_POLICY
};
const CHATGPT_WEB_RUNTIME_POLICY: ProviderRuntimePolicy = ProviderRuntimePolicy {
    fixed_provider: true,
    api_format_inheritance: ProviderApiFormatInheritance::OAuthOrBearer,
    enable_format_conversion_by_default: true,
    oauth_is_bearer_like: true,
    supports_model_fetch: false,
    supports_local_openai_chat_transport: false,
    supports_local_same_format_transport: false,
    ..STANDARD_RUNTIME_POLICY
};
const GEMINI_CLI_RUNTIME_POLICY: ProviderRuntimePolicy = ProviderRuntimePolicy {
    fixed_provider: true,
    api_format_inheritance: ProviderApiFormatInheritance::OAuth,
    oauth_is_bearer_like: true,
    supports_local_openai_chat_transport: false,
    ..STANDARD_RUNTIME_POLICY
};
const VERTEX_AI_RUNTIME_POLICY: ProviderRuntimePolicy = ProviderRuntimePolicy {
    fixed_provider: true,
    api_format_inheritance: ProviderApiFormatInheritance::OAuthOrServiceAccount,
    enable_format_conversion_by_default: true,
    supports_model_fetch: false,
    supports_local_openai_chat_transport: false,
    supports_local_same_format_transport: false,
    local_embedding_support: ProviderLocalEmbeddingSupport::Gemini,
    ..STANDARD_RUNTIME_POLICY
};
const ANTIGRAVITY_RUNTIME_POLICY: ProviderRuntimePolicy = ProviderRuntimePolicy {
    fixed_provider: true,
    api_format_inheritance: ProviderApiFormatInheritance::OAuth,
    enable_format_conversion_by_default: true,
    oauth_is_bearer_like: true,
    supports_model_fetch: false,
    supports_local_openai_chat_transport: false,
    supports_local_same_format_transport: false,
    ..STANDARD_RUNTIME_POLICY
};
const GROK_RUNTIME_POLICY: ProviderRuntimePolicy = ProviderRuntimePolicy {
    fixed_provider: true,
    api_format_inheritance: ProviderApiFormatInheritance::OAuth,
    enable_format_conversion_by_default: true,
    supports_model_fetch: false,
    supports_local_openai_chat_transport: false,
    supports_local_same_format_transport: false,
    ..STANDARD_RUNTIME_POLICY
};

const WINDSURF_RUNTIME_POLICY: ProviderRuntimePolicy = ProviderRuntimePolicy {
    fixed_provider: true,
    api_format_inheritance: ProviderApiFormatInheritance::OAuthOrBearer,
    enable_format_conversion_by_default: true,
    oauth_is_bearer_like: true,
    supports_model_fetch: false,
    supports_local_openai_chat_transport: false,
    supports_local_same_format_transport: false,
    ..STANDARD_RUNTIME_POLICY
};

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
    runtime_policy: CLAUDE_CODE_RUNTIME_POLICY,
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
    runtime_policy: CODEX_RUNTIME_POLICY,
};

const CHATGPT_WEB_FIXED_PROVIDER_TEMPLATE: FixedProviderTemplate = FixedProviderTemplate {
    provider_type: "chatgpt_web",
    version: 1,
    base_url: "https://chatgpt.com",
    endpoints: &[FixedProviderEndpointTemplate {
        item_key: "openai:image",
        api_format: "openai:image",
        custom_path: None,
        config_defaults: FORCE_STREAM_ENDPOINT_CONFIG_DEFAULTS,
    }],
    runtime_policy: CHATGPT_WEB_RUNTIME_POLICY,
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
    runtime_policy: crate::kiro::RUNTIME_POLICY,
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
    runtime_policy: GEMINI_CLI_RUNTIME_POLICY,
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
            item_key: "gemini:embedding",
            api_format: "gemini:embedding",
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
    runtime_policy: VERTEX_AI_RUNTIME_POLICY,
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
    runtime_policy: ANTIGRAVITY_RUNTIME_POLICY,
};

const GROK_FIXED_PROVIDER_TEMPLATE: FixedProviderTemplate = FixedProviderTemplate {
    provider_type: "grok",
    version: 1,
    base_url: "https://grok.com",
    endpoints: &[
        FixedProviderEndpointTemplate {
            item_key: "openai:chat",
            api_format: "openai:chat",
            custom_path: None,
            config_defaults: EMPTY_ENDPOINT_CONFIG_DEFAULTS,
        },
        FixedProviderEndpointTemplate {
            item_key: "openai:responses",
            api_format: "openai:responses",
            custom_path: None,
            config_defaults: EMPTY_ENDPOINT_CONFIG_DEFAULTS,
        },
        FixedProviderEndpointTemplate {
            item_key: "claude:messages",
            api_format: "claude:messages",
            custom_path: None,
            config_defaults: EMPTY_ENDPOINT_CONFIG_DEFAULTS,
        },
        FixedProviderEndpointTemplate {
            item_key: "openai:image",
            api_format: "openai:image",
            custom_path: None,
            config_defaults: EMPTY_ENDPOINT_CONFIG_DEFAULTS,
        },
    ],
    runtime_policy: GROK_RUNTIME_POLICY,
};

const WINDSURF_FIXED_PROVIDER_TEMPLATE: FixedProviderTemplate = FixedProviderTemplate {
    provider_type: "windsurf",
    version: 1,
    base_url: "https://server.codeium.com",
    endpoints: &[FixedProviderEndpointTemplate {
        item_key: "openai:chat",
        api_format: "openai:chat",
        custom_path: None,
        config_defaults: EMPTY_ENDPOINT_CONFIG_DEFAULTS,
    }],
    runtime_policy: WINDSURF_RUNTIME_POLICY,
};

pub fn provider_type_is_fixed(provider_type: &str) -> bool {
    provider_runtime_policy(provider_type).fixed_provider
}

pub fn fixed_provider_key_inherits_api_formats(
    provider_type: &str,
    auth_type: &str,
    decrypted_auth_config: Option<&str>,
) -> bool {
    provider_runtime_policy(provider_type)
        .key_inherits_api_formats(auth_type, decrypted_auth_config)
}

pub fn provider_type_enables_format_conversion_by_default(provider_type: &str) -> bool {
    provider_runtime_policy(provider_type).enable_format_conversion_by_default
}

pub fn provider_type_allows_auth_channel_mismatch_by_default(provider_type: &str) -> bool {
    provider_runtime_policy(provider_type).allow_auth_channel_mismatch_by_default
}

pub fn provider_type_oauth_is_bearer_like(provider_type: &str) -> bool {
    provider_runtime_policy(provider_type).oauth_is_bearer_like
}

pub fn provider_runtime_policy(provider_type: &str) -> ProviderRuntimePolicy {
    if let Some(template) = fixed_provider_template(provider_type) {
        return template.runtime_policy;
    }

    match provider_type.trim().to_ascii_lowercase().as_str() {
        "custom" => CUSTOM_RUNTIME_POLICY,
        "openai" => OPENAI_RUNTIME_POLICY,
        "gemini" | "google" => GEMINI_RUNTIME_POLICY,
        "jina" => JINA_RUNTIME_POLICY,
        "doubao" | "volcengine" => DOUBAO_RUNTIME_POLICY,
        _ => STANDARD_RUNTIME_POLICY,
    }
}

pub fn fixed_provider_template(provider_type: &str) -> Option<&'static FixedProviderTemplate> {
    match provider_type.trim().to_ascii_lowercase().as_str() {
        "claude_code" => Some(&CLAUDE_CODE_FIXED_PROVIDER_TEMPLATE),
        "codex" => Some(&CODEX_FIXED_PROVIDER_TEMPLATE),
        "chatgpt_web" => Some(&CHATGPT_WEB_FIXED_PROVIDER_TEMPLATE),
        "kiro" => Some(&KIRO_FIXED_PROVIDER_TEMPLATE),
        "grok" => Some(&GROK_FIXED_PROVIDER_TEMPLATE),
        "gemini_cli" => Some(&GEMINI_CLI_FIXED_PROVIDER_TEMPLATE),
        "vertex_ai" => Some(&VERTEX_AI_FIXED_PROVIDER_TEMPLATE),
        "antigravity" => Some(&ANTIGRAVITY_FIXED_PROVIDER_TEMPLATE),
        "windsurf" => Some(&WINDSURF_FIXED_PROVIDER_TEMPLATE),
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
    provider_runtime_policy(provider_type).supports_model_fetch
}

pub fn provider_type_supports_local_openai_chat_transport(provider_type: &str) -> bool {
    provider_runtime_policy(provider_type).supports_local_openai_chat_transport
}

pub fn provider_type_supports_local_same_format_transport(provider_type: &str) -> bool {
    provider_runtime_policy(provider_type).supports_local_same_format_transport
}

pub fn provider_type_supports_local_embedding_transport(
    provider_type: &str,
    api_format: &str,
) -> bool {
    provider_runtime_policy(provider_type).supports_local_embedding_transport(api_format)
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
        "chatgpt_web" => Some(ProviderOAuthTemplate {
            provider_type: "chatgpt_web",
            display_name: "ChatGPT Web",
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
        "windsurf" => Some(ProviderOAuthTemplate {
            provider_type: "windsurf",
            display_name: "Windsurf",
            authorize_url: "https://windsurf.com/windsurf/signin",
            token_url: "https://register.windsurf.com/exa.seat_management_pb.SeatManagementService/RegisterUser",
            client_id: "3GUryQ7ldAeKEuD2obYnppsnmj58eP5u",
            client_secret: "",
            scopes: &[],
            redirect_uri: "show-auth-token",
            use_pkce: false,
        }),
        _ => None,
    }
}

pub const ADMIN_PROVIDER_OAUTH_TEMPLATE_TYPES: &[&str] = &[
    "claude_code",
    "codex",
    "chatgpt_web",
    "gemini_cli",
    "antigravity",
    "windsurf",
];

#[cfg(test)]
mod tests {
    use super::{
        fixed_provider_endpoint_template_by_api_format, fixed_provider_key_inherits_api_formats,
        fixed_provider_template, provider_runtime_policy, provider_type_admin_oauth_template,
        provider_type_allows_auth_channel_mismatch_by_default, provider_type_oauth_is_bearer_like,
        provider_type_supports_local_embedding_transport,
        provider_type_supports_local_same_format_transport, FixedProviderEndpointConfigValue,
        ADMIN_PROVIDER_OAUTH_TEMPLATE_TYPES,
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
    fn chatgpt_web_fixed_provider_template_only_exposes_openai_image() {
        let template =
            fixed_provider_template("chatgpt_web").expect("chatgpt_web template should exist");
        assert_eq!(template.base_url, "https://chatgpt.com");
        assert_eq!(template.version, 1);
        assert_eq!(
            template
                .endpoints
                .iter()
                .map(|item| item.api_format)
                .collect::<Vec<_>>(),
            vec!["openai:image"]
        );

        let image_template =
            fixed_provider_endpoint_template_by_api_format("chatgpt_web", "openai:image")
                .expect("chatgpt_web image endpoint should exist");
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
    fn grok_fixed_provider_template_exposes_chat_responses_messages_and_image() {
        let template = fixed_provider_template("grok").expect("grok template should exist");
        assert_eq!(template.base_url, "https://grok.com");
        assert_eq!(template.version, 1);
        assert_eq!(
            template
                .endpoints
                .iter()
                .map(|item| item.api_format)
                .collect::<Vec<_>>(),
            vec![
                "openai:chat",
                "openai:responses",
                "claude:messages",
                "openai:image"
            ]
        );
        assert!(!template.runtime_policy.supports_model_fetch);
        assert!(!template.runtime_policy.supports_local_openai_chat_transport);
        assert!(!template.runtime_policy.supports_local_same_format_transport);
    }

    #[test]
    fn windsurf_fixed_provider_template_exposes_openai_chat() {
        let template = fixed_provider_template("windsurf").expect("windsurf template should exist");
        assert_eq!(template.provider_type, "windsurf");
        assert_eq!(template.base_url, "https://server.codeium.com");
        assert_eq!(template.version, 1);
        assert_eq!(
            template
                .endpoints
                .iter()
                .map(|item| item.api_format)
                .collect::<Vec<_>>(),
            vec!["openai:chat"]
        );
        assert!(
            fixed_provider_endpoint_template_by_api_format("windsurf", "openai:chat").is_some()
        );

        let policy = provider_runtime_policy("windsurf");
        assert!(policy.fixed_provider);
        assert!(policy.enable_format_conversion_by_default);
        assert!(policy.oauth_is_bearer_like);
        assert!(!policy.supports_model_fetch);
        assert!(!policy.supports_local_same_format_transport);
    }

    #[test]
    fn windsurf_admin_oauth_template_is_advertised() {
        let template =
            provider_type_admin_oauth_template("windsurf").expect("windsurf oauth template");

        assert_eq!(template.provider_type, "windsurf");
        assert_eq!(template.display_name, "Windsurf");
        assert_eq!(
            template.authorize_url,
            "https://windsurf.com/windsurf/signin"
        );
        assert_eq!(template.redirect_uri, "show-auth-token");
        assert!(ADMIN_PROVIDER_OAUTH_TEMPLATE_TYPES.contains(&"windsurf"));
    }

    #[test]
    fn fixed_provider_key_inheritance_keeps_oauth_and_kiro_configured_bearer_keys_open() {
        assert!(fixed_provider_key_inherits_api_formats(
            "codex", "oauth", None
        ));
        assert!(fixed_provider_key_inherits_api_formats(
            "chatgpt_web",
            "oauth",
            None
        ));
        assert!(fixed_provider_key_inherits_api_formats(
            "kiro",
            "bearer",
            Some("{}")
        ));
        assert!(fixed_provider_key_inherits_api_formats(
            "vertex_ai",
            "service_account",
            None
        ));
        assert!(!fixed_provider_key_inherits_api_formats(
            "kiro", "bearer", None
        ));
        assert!(!fixed_provider_key_inherits_api_formats(
            "custom", "oauth", None
        ));
    }

    #[test]
    fn kiro_allows_auth_channel_mismatch_by_default() {
        let policy = provider_runtime_policy("kiro");
        assert!(policy.fixed_provider);
        assert!(policy.enable_format_conversion_by_default);
        assert!(policy.oauth_is_bearer_like);
        assert!(policy.supports_model_fetch);
        assert!(!policy.supports_local_openai_chat_transport);
        assert!(!policy.supports_local_same_format_transport);
        assert!(policy.key_inherits_api_formats("oauth", None));
        assert!(policy.key_inherits_api_formats("bearer", Some("{}")));
        assert!(!policy.key_inherits_api_formats("bearer", None));

        assert!(provider_type_allows_auth_channel_mismatch_by_default(
            "kiro"
        ));
        assert!(provider_type_allows_auth_channel_mismatch_by_default(
            " KIRO "
        ));
        assert!(!provider_type_allows_auth_channel_mismatch_by_default(
            "claude_code"
        ));
        assert!(!provider_type_allows_auth_channel_mismatch_by_default(
            "custom"
        ));
    }

    #[test]
    fn runtime_policy_preserves_other_fixed_provider_behavior() {
        let codex = provider_runtime_policy("codex");
        assert!(codex.fixed_provider);
        assert!(codex.enable_format_conversion_by_default);
        assert!(!codex.oauth_is_bearer_like);
        assert!(!codex.supports_model_fetch);
        assert!(!codex.supports_local_openai_chat_transport);
        assert!(codex.supports_local_same_format_transport);

        let gemini_cli = provider_runtime_policy("gemini_cli");
        assert!(gemini_cli.fixed_provider);
        assert!(!gemini_cli.enable_format_conversion_by_default);
        assert!(provider_type_oauth_is_bearer_like("gemini_cli"));
        assert!(gemini_cli.supports_model_fetch);
        assert!(!gemini_cli.supports_local_openai_chat_transport);
        assert!(gemini_cli.supports_local_same_format_transport);
    }

    #[test]
    fn chatgpt_web_does_not_use_generic_same_format_transport() {
        assert!(!provider_type_supports_local_same_format_transport(
            "chatgpt_web"
        ));
    }

    #[test]
    fn provider_type_supports_only_matching_embedding_formats() {
        for (provider_type, api_format) in [
            ("openai", "openai:embedding"),
            ("custom", "openai:embedding"),
            ("gemini", "gemini:embedding"),
            ("google", "gemini:embedding"),
            ("vertex_ai", "gemini:embedding"),
            ("jina", "jina:embedding"),
            ("doubao", "doubao:embedding"),
            ("volcengine", "doubao:embedding"),
        ] {
            assert!(
                provider_type_supports_local_embedding_transport(provider_type, api_format),
                "{provider_type} should support {api_format}"
            );
        }

        for (provider_type, api_format) in [
            ("openai", "gemini:embedding"),
            ("gemini", "openai:embedding"),
            ("vertex_ai", "openai:embedding"),
            ("jina", "doubao:embedding"),
            ("doubao", "jina:embedding"),
            ("claude_code", "openai:embedding"),
            ("openai", "openai:chat"),
        ] {
            assert!(
                !provider_type_supports_local_embedding_transport(provider_type, api_format),
                "{provider_type} should not support {api_format}"
            );
        }

        assert!(provider_type_supports_local_embedding_transport(
            " Google ",
            "GEMINI:EMBEDDING"
        ));
    }

    #[test]
    fn vertex_fixed_provider_template_includes_gemini_embedding_endpoint() {
        let template =
            fixed_provider_template("vertex_ai").expect("vertex_ai template should exist");

        assert_eq!(
            template
                .endpoints
                .iter()
                .map(|item| item.api_format)
                .collect::<Vec<_>>(),
            vec![
                "gemini:generate_content",
                "gemini:embedding",
                "claude:messages",
            ]
        );

        assert!(
            fixed_provider_endpoint_template_by_api_format("vertex_ai", "gemini:embedding")
                .is_some()
        );
    }
}
