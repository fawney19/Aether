"""固定 Provider 的模板定义。

该文件用于集中管理：
- 固定 Provider 的上游 API base_url / 固定路径策略（通常通过 EndpointDefinition.default_path + 锁定 custom_path=None）
- 固定 Provider 的 OAuth2 客户端常量（authorize/token/client_id/client_secret/scopes/redirect_uri）

注意：该文件会直接包含从参考项目 CLIProxyAPI 复制的 OAuth client_id/client_secret（敏感）。
务必确保：
- 不把 client_secret/refresh_token/access_token 输出到日志
- 不通过 API 响应把敏感信息返回给前端
"""

from __future__ import annotations

from dataclasses import dataclass

from src.core.provider_templates.types import ProviderType

# Antigravity 生产环境 URL（唯一定义点，services 层通过 re-export 引用）
ANTIGRAVITY_PROD_URL = "https://cloudcode-pa.googleapis.com"


@dataclass(frozen=True, slots=True)
class FixedProviderOAuth:
    authorize_url: str
    token_url: str
    client_id: str
    client_secret: str
    scopes: list[str]
    redirect_uri: str
    use_pkce: bool


@dataclass(frozen=True, slots=True)
class FixedProviderTemplate:
    provider_type: ProviderType
    display_name: str

    # 上游 API（ProviderEndpoint.base_url 应锁定为该值；custom_path 通常保持 None 使用 default_path）
    api_base_url: str

    # 该 Provider 默认创建哪些 endpoint signature
    endpoint_signatures: list[str]

    # OAuth2 配置（用于生成授权 URL / 换 token / refresh）
    oauth: FixedProviderOAuth


# ------------------------------
# Fixed templates
# ------------------------------
# 说明：client_id/client_secret 从 CLIProxyAPI/internal/auth 复制。
# 该文件包含敏感信息，务必避免输出到日志或 API 响应。

FIXED_PROVIDERS: dict[ProviderType, FixedProviderTemplate] = {
    ProviderType.CLAUDE_CODE: FixedProviderTemplate(
        provider_type=ProviderType.CLAUDE_CODE,
        display_name="ClaudeCode",
        api_base_url="https://api.anthropic.com",
        endpoint_signatures=["claude:cli"],
        oauth=FixedProviderOAuth(
            authorize_url="https://claude.ai/oauth/authorize",
            token_url="https://console.anthropic.com/v1/oauth/token",
            client_id="9d1c250a-e61b-44d9-88ed-5944d1962f5e",
            client_secret="",
            scopes=["org:create_api_key", "user:profile", "user:inference"],
            redirect_uri="http://localhost:54545/callback",
            use_pkce=True,
        ),
    ),
    ProviderType.CODEX: FixedProviderTemplate(
        provider_type=ProviderType.CODEX,
        display_name="Codex",
        api_base_url="https://chatgpt.com/backend-api/codex",
        endpoint_signatures=["openai:cli", "openai:compact", "openai:image"],
        oauth=FixedProviderOAuth(
            authorize_url="https://auth.openai.com/oauth/authorize",
            token_url="https://auth.openai.com/oauth/token",
            client_id="app_EMoamEEZ73f0CkXaXp7hrann",
            client_secret="",
            scopes=["openid", "email", "profile", "offline_access"],
            redirect_uri="http://localhost:1455/auth/callback",
            use_pkce=True,
        ),
    ),
    ProviderType.KIRO: FixedProviderTemplate(
        provider_type=ProviderType.KIRO,
        display_name="Kiro",
        # Region is resolved from per-key auth_config (imported credentials).
        # Keep a templated base_url so endpoints remain fixed/locked.
        api_base_url="https://q.{region}.amazonaws.com",
        endpoint_signatures=["claude:cli"],
        # Kiro does not support Aether's OAuth flow; credentials are imported.
        # Keep a placeholder OAuth config so the fixed-provider template shape stays stable.
        oauth=FixedProviderOAuth(
            authorize_url="",
            token_url="",
            client_id="",
            client_secret="",
            scopes=[],
            redirect_uri="",
            use_pkce=False,
        ),
    ),
    ProviderType.GEMINI_CLI: FixedProviderTemplate(
        provider_type=ProviderType.GEMINI_CLI,
        display_name="GeminiCli",
        api_base_url="https://cloudcode-pa.googleapis.com",
        endpoint_signatures=["gemini:cli"],
        oauth=FixedProviderOAuth(
            authorize_url="https://accounts.google.com/o/oauth2/v2/auth",
            token_url="https://oauth2.googleapis.com/token",
            client_id="681255809395-oo8ft2oprdrnp9e3aqf6av3hmdib135j.apps.googleusercontent.com",
            client_secret="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",
            scopes=[
                "https://www.googleapis.com/auth/cloud-platform",
                "https://www.googleapis.com/auth/userinfo.email",
                "https://www.googleapis.com/auth/userinfo.profile",
            ],
            redirect_uri="http://localhost:8085/oauth2callback",
            use_pkce=False,
        ),
    ),
    ProviderType.VERTEX_AI: FixedProviderTemplate(
        provider_type=ProviderType.VERTEX_AI,
        display_name="Vertex AI",
        # Vertex uses fixed global base URL; concrete upstream path is selected by transport hook.
        api_base_url="https://aiplatform.googleapis.com",
        endpoint_signatures=["gemini:chat", "claude:chat"],
        # Vertex does not use this OAuth flow (it uses API Key / Service Account).
        oauth=FixedProviderOAuth(
            authorize_url="",
            token_url="",
            client_id="",
            client_secret="",
            scopes=[],
            redirect_uri="",
            use_pkce=False,
        ),
    ),
    ProviderType.ANTIGRAVITY: FixedProviderTemplate(
        provider_type=ProviderType.ANTIGRAVITY,
        display_name="Antigravity",
        api_base_url=ANTIGRAVITY_PROD_URL,
        endpoint_signatures=["gemini:chat"],
        oauth=FixedProviderOAuth(
            authorize_url="https://accounts.google.com/o/oauth2/v2/auth",
            token_url="https://oauth2.googleapis.com/token",
            client_id="1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com",
            client_secret="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",
            scopes=[
                "https://www.googleapis.com/auth/cloud-platform",
                "https://www.googleapis.com/auth/userinfo.email",
                "https://www.googleapis.com/auth/userinfo.profile",
                "https://www.googleapis.com/auth/cclog",
                "https://www.googleapis.com/auth/experimentsandconfigs",
            ],
            redirect_uri="http://localhost:51121/oauth2callback",
            use_pkce=True,
        ),
    ),
}


__all__ = [
    "FixedProviderOAuth",
    "FixedProviderTemplate",
    "FIXED_PROVIDERS",
]
