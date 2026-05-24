use crate::handlers::shared::{
    decrypt_catalog_secret_with_fallbacks, system_config_bool, system_config_string,
};
use crate::{AppState, GatewayError};
use serde_json::Value;

pub(crate) const SERVER_CHAN_PUSH_ENABLED_KEY: &str = "module.server_chan_push.enabled";
pub(crate) const SERVER_CHAN_PUSH_SEND_KEY_KEY: &str = "module.server_chan_push.send_key";
pub(crate) const SERVER_CHAN_PUSH_TEMPLATE_KEY: &str = "module.server_chan_push.template";
pub(crate) const LEGACY_SERVER_CHAN_ENABLED_KEY: &str =
    "module.important_notification.server_chan_enabled";
pub(crate) const LEGACY_SERVER_CHAN_SEND_KEY_KEY: &str =
    "module.important_notification.server_chan_send_key";
pub(crate) const LEGACY_SERVER_CHAN_TEMPLATE_KEY: &str =
    "module.important_notification.server_chan_template";

const SERVER_CHAN_API_BASE: &str = "https://sctapi.ftqq.com";

#[derive(Debug, Clone)]
pub(crate) struct ServerChanPushConfig {
    pub(crate) enabled: bool,
    pub(crate) send_key: Option<String>,
    pub(crate) template: Option<String>,
}

pub(crate) async fn server_chan_push_module_enabled(
    state: &AppState,
) -> Result<bool, GatewayError> {
    let canonical = state
        .read_system_config_json_value(SERVER_CHAN_PUSH_ENABLED_KEY)
        .await?;
    if canonical.is_some() {
        return Ok(system_config_bool(canonical.as_ref(), false));
    }
    let legacy = state
        .read_system_config_json_value(LEGACY_SERVER_CHAN_ENABLED_KEY)
        .await?;
    Ok(system_config_bool(legacy.as_ref(), false))
}

pub(crate) async fn server_chan_push_configured(state: &AppState) -> Result<bool, GatewayError> {
    Ok(read_server_chan_push_config(state)
        .await?
        .send_key
        .is_some())
}

pub(crate) async fn read_server_chan_push_config(
    state: &AppState,
) -> Result<ServerChanPushConfig, GatewayError> {
    let enabled = server_chan_push_module_enabled(state).await?;
    let send_key = read_server_chan_value(
        state,
        SERVER_CHAN_PUSH_SEND_KEY_KEY,
        LEGACY_SERVER_CHAN_SEND_KEY_KEY,
    )
    .await?
    .and_then(|value| system_config_string(Some(&value)))
    .map(|value| {
        decrypt_catalog_secret_with_fallbacks(state.encryption_key(), &value).unwrap_or(value)
    });
    let template = read_server_chan_value(
        state,
        SERVER_CHAN_PUSH_TEMPLATE_KEY,
        LEGACY_SERVER_CHAN_TEMPLATE_KEY,
    )
    .await?
    .and_then(|value| system_config_string(Some(&value)));

    Ok(ServerChanPushConfig {
        enabled,
        send_key,
        template,
    })
}

async fn read_server_chan_value(
    state: &AppState,
    canonical_key: &str,
    legacy_key: &str,
) -> Result<Option<Value>, GatewayError> {
    let canonical = state.read_system_config_json_value(canonical_key).await?;
    if canonical.is_some() {
        return Ok(canonical);
    }
    state.read_system_config_json_value(legacy_key).await
}

pub(crate) async fn send_server_chan_push(
    state: &AppState,
    config: &ServerChanPushConfig,
    title: &str,
    markdown_body: &str,
) -> Result<(), GatewayError> {
    let Some(send_key) = config.send_key.as_deref() else {
        return Err(GatewayError::Internal(
            "未配置 Server 酱 SendKey".to_string(),
        ));
    };
    let send_key = send_key.trim();
    if send_key.is_empty() {
        return Err(GatewayError::Internal(
            "Server 酱 SendKey 不能为空".to_string(),
        ));
    }
    let desp = render_server_chan_desp(config.template.as_deref(), title, markdown_body);
    let url = format!("{SERVER_CHAN_API_BASE}/{send_key}.send");
    let response = state
        .client
        .post(url)
        .form(&[("title", title), ("desp", desp.as_str())])
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
            "Server 酱返回 HTTP {status}: {text}"
        )));
    }
    if let Ok(payload) = serde_json::from_str::<Value>(&text) {
        let code_is_ok = payload
            .get("code")
            .and_then(|value| {
                value
                    .as_i64()
                    .map(|code| code == 0)
                    .or_else(|| value.as_str().map(|code| code.trim() == "0"))
            })
            .unwrap_or(true);
        if !code_is_ok {
            return Err(GatewayError::Internal(format!(
                "Server 酱返回失败: {payload}"
            )));
        }
    }
    Ok(())
}

fn render_server_chan_desp(template: Option<&str>, title: &str, markdown_body: &str) -> String {
    match template {
        Some(template) if !template.trim().is_empty() => template
            .replace("{title}", title)
            .replace("{body}", markdown_body),
        _ => markdown_body.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::render_server_chan_desp;

    #[test]
    fn server_chan_desp_uses_template_when_provided() {
        let rendered =
            render_server_chan_desp(Some("**{title}**\n\n{body}\n\n--end--"), "告警", "原始正文");
        assert_eq!(rendered, "**告警**\n\n原始正文\n\n--end--");
    }

    #[test]
    fn server_chan_desp_falls_back_to_markdown_body_for_empty_template() {
        assert_eq!(
            render_server_chan_desp(None, "告警", "原始正文"),
            "原始正文"
        );
        assert_eq!(
            render_server_chan_desp(Some("   "), "告警", "原始正文"),
            "原始正文"
        );
    }
}
