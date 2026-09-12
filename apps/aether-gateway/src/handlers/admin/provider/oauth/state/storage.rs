use crate::handlers::admin::request::AdminProviderOAuthTemplate;
use aether_oauth::core::OAuthError;
use aether_oauth::provider::{ProviderOAuthService, ProviderOAuthTransportContext};
use serde_json::json;

pub(crate) fn build_provider_oauth_start_response(
    template: AdminProviderOAuthTemplate,
    nonce: &str,
    code_challenge: Option<&str>,
) -> Result<serde_json::Value, OAuthError> {
    let authorization_url =
        build_provider_oauth_authorization_url(template, nonce, code_challenge)?;

    Ok(json!({
        "authorization_url": authorization_url,
        "redirect_uri": template.redirect_uri,
        "provider_type": template.provider_type,
        "instructions": "1) 打开 authorization_url 完成授权\n2) 复制授权页面显示的授权码或浏览器中的完整回调 URL\n3) 调用 complete 接口粘贴 callback_url",
    }))
}

fn build_provider_oauth_authorization_url(
    template: AdminProviderOAuthTemplate,
    nonce: &str,
    code_challenge: Option<&str>,
) -> Result<String, OAuthError> {
    let ctx = ProviderOAuthTransportContext {
        provider_id: String::new(),
        provider_type: template.provider_type.to_string(),
        endpoint_id: None,
        key_id: None,
        auth_type: Some("oauth".to_string()),
        decrypted_api_key: None,
        decrypted_auth_config: None,
        provider_config: None,
        endpoint_config: None,
        key_config: None,
        network: aether_oauth::network::OAuthNetworkContext::provider_operation(None),
    };
    ProviderOAuthService::with_builtin_adapters()
        .build_authorize_url(&ctx, nonce, code_challenge)
        .map(|response| response.authorize_url)
}
