use crate::handlers::shared::{
    decrypt_catalog_secret_with_fallbacks, system_config_bool, system_config_string,
};
use crate::{AppState, GatewayError};
use serde_json::{json, Value};

pub(crate) const BARK_PUSH_ENABLED_KEY: &str = "module.bark_push.enabled";
pub(crate) const BARK_PUSH_DEVICE_KEY_KEY: &str = "module.bark_push.device_key";
pub(crate) const BARK_PUSH_SERVER_URL_KEY: &str = "module.bark_push.server_url";
pub(crate) const BARK_PUSH_TEMPLATE_KEY: &str = "module.bark_push.template";

const DEFAULT_BARK_API_BASE: &str = "https://api.day.app";

#[derive(Debug, Clone)]
pub(crate) struct BarkPushConfig {
    pub(crate) enabled: bool,
    pub(crate) device_key: Option<String>,
    pub(crate) server_url: String,
    pub(crate) template: Option<String>,
}

pub(crate) async fn bark_push_module_enabled(state: &AppState) -> Result<bool, GatewayError> {
    let value = state
        .read_system_config_json_value(BARK_PUSH_ENABLED_KEY)
        .await?;
    Ok(system_config_bool(value.as_ref(), false))
}

pub(crate) async fn bark_push_configured(state: &AppState) -> Result<bool, GatewayError> {
    let config = read_bark_push_config(state).await?;
    Ok(config.device_key.is_some() && !config.server_url.trim().is_empty())
}

pub(crate) async fn read_bark_push_config(
    state: &AppState,
) -> Result<BarkPushConfig, GatewayError> {
    let enabled = bark_push_module_enabled(state).await?;
    let device_key = state
        .read_system_config_json_value(BARK_PUSH_DEVICE_KEY_KEY)
        .await?
        .and_then(|value| system_config_string(Some(&value)))
        .map(|value| {
            decrypt_catalog_secret_with_fallbacks(state.encryption_key(), &value).unwrap_or(value)
        });
    let server_url = state
        .read_system_config_json_value(BARK_PUSH_SERVER_URL_KEY)
        .await?
        .and_then(|value| system_config_string(Some(&value)))
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_BARK_API_BASE.to_string());
    let template = state
        .read_system_config_json_value(BARK_PUSH_TEMPLATE_KEY)
        .await?
        .and_then(|value| system_config_string(Some(&value)));

    Ok(BarkPushConfig {
        enabled,
        device_key,
        server_url,
        template,
    })
}

pub(crate) async fn send_bark_push(
    state: &AppState,
    config: &BarkPushConfig,
    title: &str,
    markdown_body: &str,
) -> Result<(), GatewayError> {
    let Some(device_key) = config.device_key.as_deref() else {
        return Err(GatewayError::Internal("未配置 Bark Device Key".to_string()));
    };
    let device_key = device_key.trim();
    if device_key.is_empty() {
        return Err(GatewayError::Internal(
            "Bark Device Key 不能为空".to_string(),
        ));
    }
    let server_url = normalized_bark_server_url(&config.server_url)?;
    let body = render_bark_body(config.template.as_deref(), title, markdown_body);
    let response = state
        .client
        .post(format!("{server_url}/push"))
        .json(&json!({
            "device_key": device_key,
            "title": title,
            "body": body,
        }))
        .send()
        .await
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    if !status.is_success() {
        return Err(GatewayError::Internal(format!(
            "Bark 返回 HTTP {status}: {text}"
        )));
    }
    if let Ok(payload) = serde_json::from_str::<Value>(&text) {
        let code_is_ok = payload
            .get("code")
            .and_then(|value| {
                value
                    .as_i64()
                    .map(|code| matches!(code, 0 | 200))
                    .or_else(|| {
                        value
                            .as_str()
                            .map(|code| matches!(code.trim(), "0" | "200"))
                    })
            })
            .unwrap_or(true);
        if !code_is_ok {
            return Err(GatewayError::Internal(format!("Bark 返回失败: {payload}")));
        }
    }
    Ok(())
}

fn normalized_bark_server_url(server_url: &str) -> Result<String, GatewayError> {
    let server_url = server_url.trim().trim_end_matches('/');
    if server_url.is_empty() {
        return Err(GatewayError::Internal(
            "Bark 服务器地址不能为空".to_string(),
        ));
    }
    if !server_url.starts_with("https://") && !server_url.starts_with("http://") {
        return Err(GatewayError::Internal(
            "Bark 服务器地址必须以 http:// 或 https:// 开头".to_string(),
        ));
    }
    Ok(server_url.to_string())
}

fn render_bark_body(template: Option<&str>, title: &str, markdown_body: &str) -> String {
    match template {
        Some(template) if !template.trim().is_empty() => template
            .replace("{title}", title)
            .replace("{body}", markdown_body),
        _ => markdown_body.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{normalized_bark_server_url, render_bark_body};

    #[test]
    fn bark_body_uses_template_when_provided() {
        let rendered = render_bark_body(Some("{title}\n\n{body}"), "告警", "原始正文");
        assert_eq!(rendered, "告警\n\n原始正文");
    }

    #[test]
    fn bark_body_falls_back_to_markdown_body_for_empty_template() {
        assert_eq!(render_bark_body(None, "告警", "原始正文"), "原始正文");
        assert_eq!(
            render_bark_body(Some("   "), "告警", "原始正文"),
            "原始正文"
        );
    }

    #[test]
    fn bark_server_url_trims_trailing_slashes() {
        assert_eq!(
            normalized_bark_server_url(" https://api.day.app/ ").expect("url should parse"),
            "https://api.day.app"
        );
    }
}
