use crate::handlers::admin::request::AdminProviderOAuthTemplate;
use serde_json::json;
use url::form_urlencoded;

pub(crate) fn build_provider_oauth_start_response(
    template: AdminProviderOAuthTemplate,
    nonce: &str,
    code_challenge: Option<&str>,
) -> serde_json::Value {
    let mut serializer = form_urlencoded::Serializer::new(String::new());
    serializer.append_pair("client_id", template.client_id);
    serializer.append_pair("response_type", "code");
    serializer.append_pair("redirect_uri", template.redirect_uri);
    serializer.append_pair("scope", &template.scopes.join(" "));
    serializer.append_pair("state", nonce);
    if template.provider_type == "codex" {
        serializer.append_pair("prompt", "login");
        serializer.append_pair("id_token_add_organizations", "true");
        serializer.append_pair("codex_cli_simplified_flow", "true");
    }
    if template.use_pkce {
        if let Some(code_challenge) = code_challenge {
            serializer.append_pair("code_challenge", code_challenge);
            serializer.append_pair("code_challenge_method", "S256");
        }
    }

    json!({
        "authorization_url": format!("{}?{}", template.authorize_url, serializer.finish()),
        "redirect_uri": template.redirect_uri,
        "provider_type": template.provider_type,
        "instructions": "1) 打开 authorization_url 完成授权\n2) 授权后会跳转到 redirect_uri（localhost）\n3) 复制浏览器地址栏完整 URL，调用 complete 接口粘贴 callback_url",
    })
}
