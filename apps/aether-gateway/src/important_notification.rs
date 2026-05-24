use crate::admin_api::AdminAppState;
use crate::bark_push::{read_bark_push_config, send_bark_push, BarkPushConfig};
use crate::email_delivery::{
    read_smtp_delivery_config, send_smtp_email, ComposedEmail, SmtpDeliveryConfig,
};
use crate::handlers::shared::{system_config_bool, system_config_string};
use crate::server_chan_push::{
    read_server_chan_push_config, send_server_chan_push, ServerChanPushConfig,
};
use crate::{AppState, GatewayError};
use axum::body::Bytes;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tracing::warn;

pub(crate) const IMPORTANT_NOTIFICATION_ENABLED_KEY: &str = "module.important_notification.enabled";
pub(crate) const LEGACY_NOTIFICATION_EMAIL_ENABLED_KEY: &str = "module.notification_email.enabled";
pub(crate) const IMPORTANT_NOTIFICATION_EMAIL_ENABLED_KEY: &str =
    "module.important_notification.email_enabled";
pub(crate) const IMPORTANT_NOTIFICATION_EMAIL_RECIPIENTS_KEY: &str =
    "module.important_notification.email_recipients";
pub(crate) const IMPORTANT_NOTIFICATION_DEFAULT_CHANNEL_KEY: &str =
    "module.important_notification.default_channel";
pub(crate) const IMPORTANT_NOTIFICATION_ITEMS_KEY: &str = "module.important_notification.items";
pub(crate) const PROVIDER_QUOTA_ALERT_ITEM_KEY: &str = "provider_quota_alert";

#[derive(Debug, Clone)]
pub(crate) struct ImportantNotification {
    pub(crate) title: String,
    pub(crate) markdown_body: String,
    pub(crate) text_body: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImportantNotificationChannelFilter {
    All,
    Email,
    ServerChan,
    Bark,
}

#[derive(Debug, Clone)]
struct ImportantNotificationConfig {
    module_enabled: bool,
    email_enabled: bool,
    email_recipients: Vec<String>,
    default_channel: ImportantNotificationChannelFilter,
    items: Vec<ImportantNotificationItemConfig>,
    server_chan: ServerChanPushConfig,
    bark: BarkPushConfig,
}

#[derive(Debug, Clone)]
struct ImportantNotificationItemConfig {
    key: String,
    name: String,
    enabled: bool,
    channel: Option<ImportantNotificationChannelFilter>,
    title_template: Option<String>,
    markdown_template: Option<String>,
    text_template: Option<String>,
    user_email_enabled: bool,
}

#[derive(Debug, Clone, Copy)]
struct NotificationChannelReadiness {
    email: bool,
    server_chan: bool,
    bark: bool,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ImportantNotificationChannelReport {
    pub(crate) channel: &'static str,
    pub(crate) success: bool,
    pub(crate) message: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ImportantNotificationDeliveryReport {
    pub(crate) success: bool,
    pub(crate) channels: Vec<ImportantNotificationChannelReport>,
}

#[derive(Debug, Deserialize)]
struct ImportantNotificationTestRequest {
    #[serde(default)]
    channel: Option<String>,
    #[serde(default)]
    item_key: Option<String>,
}

pub(crate) async fn important_notification_module_enabled(
    state: &AppState,
) -> Result<bool, GatewayError> {
    let canonical = state
        .read_system_config_json_value(IMPORTANT_NOTIFICATION_ENABLED_KEY)
        .await?;
    if canonical.is_some() {
        return Ok(system_config_bool(canonical.as_ref(), false));
    }
    let legacy = state
        .read_system_config_json_value(LEGACY_NOTIFICATION_EMAIL_ENABLED_KEY)
        .await?;
    Ok(system_config_bool(legacy.as_ref(), false))
}

pub(crate) async fn important_notification_configured(
    state: &AppState,
) -> Result<bool, GatewayError> {
    let config = read_important_notification_config(state).await?;
    important_notification_has_configured_channel(state, &config).await
}

pub(crate) async fn important_notification_dispatch_ready_for_item(
    state: &AppState,
    item_key: &str,
) -> Result<bool, GatewayError> {
    let config = read_important_notification_config(state).await?;
    if !config.module_enabled {
        return Ok(false);
    }
    if let Some(item) = find_notification_item(&config, item_key) {
        if !item.enabled {
            return Ok(false);
        }
    }
    let readiness = read_notification_channel_readiness(state, &config).await?;
    Ok(channel_filter_has_ready_channel(
        notification_item_channel_filter(&config, item_key),
        readiness,
    ))
}

pub(crate) async fn send_important_notification(
    state: &AppState,
    notification: ImportantNotification,
) -> Result<ImportantNotificationDeliveryReport, GatewayError> {
    send_important_notification_with_filter(
        state,
        notification,
        ImportantNotificationChannelFilter::All,
    )
    .await
}

pub(crate) async fn send_important_notification_for_item(
    state: &AppState,
    item_key: &str,
    notification: ImportantNotification,
    variables: &[(&str, String)],
) -> Result<ImportantNotificationDeliveryReport, GatewayError> {
    dispatch_important_notification(state, Some(item_key), notification, variables, None, false)
        .await
}

pub(crate) async fn send_important_notification_with_filter(
    state: &AppState,
    notification: ImportantNotification,
    channel_filter: ImportantNotificationChannelFilter,
) -> Result<ImportantNotificationDeliveryReport, GatewayError> {
    dispatch_important_notification(state, None, notification, &[], Some(channel_filter), false)
        .await
}

pub(crate) async fn send_user_important_notification_email(
    state: &AppState,
    item_key: &str,
    user_email: &str,
    notification: ImportantNotification,
    variables: &[(&str, String)],
) -> Result<ImportantNotificationDeliveryReport, GatewayError> {
    let config = read_important_notification_config(state).await?;
    if !config.module_enabled {
        return Ok(single_report("module", false, "通知服务未启用"));
    }
    let Some(item) = find_notification_item(&config, item_key) else {
        return Ok(single_report("item", false, "通知项未定义"));
    };
    if !item.enabled {
        return Ok(single_report("item", false, "通知项未启用"));
    }
    if !item.user_email_enabled {
        return Ok(single_report("user_email", false, "通知项未启用用户邮件"));
    }
    let notification = apply_notification_item_template(Some(item), notification, variables);
    let smtp_config = match read_smtp_delivery_config(state).await? {
        Some(config) => config,
        None => return Ok(single_report("user_email", false, "SMTP 配置不完整")),
    };
    let user_email = user_email.trim();
    if user_email.is_empty() {
        return Ok(single_report("user_email", false, "用户邮箱为空"));
    }
    match send_single_email_notification(smtp_config, user_email, &notification).await {
        Ok(()) => Ok(single_report("user_email", true, "用户邮件通知已发送")),
        Err(err) => {
            warn!(error = ?err, user_email = %user_email, "failed to send user notification email");
            Ok(single_report(
                "user_email",
                false,
                format!("用户邮件通知发送失败: {err:?}"),
            ))
        }
    }
}

async fn important_notification_has_configured_channel(
    state: &AppState,
    config: &ImportantNotificationConfig,
) -> Result<bool, GatewayError> {
    let readiness = read_notification_channel_readiness(state, config).await?;
    if channel_filter_has_ready_channel(config.default_channel, readiness) {
        return Ok(true);
    }
    Ok(config.items.iter().any(|item| {
        item.enabled
            && channel_filter_has_ready_channel(
                item.channel.unwrap_or(config.default_channel),
                readiness,
            )
    }))
}

async fn read_notification_channel_readiness(
    state: &AppState,
    config: &ImportantNotificationConfig,
) -> Result<NotificationChannelReadiness, GatewayError> {
    let smtp_config = read_smtp_delivery_config(state).await?;
    Ok(NotificationChannelReadiness {
        email: config.email_enabled && !config.email_recipients.is_empty() && smtp_config.is_some(),
        server_chan: config.server_chan.enabled && config.server_chan.send_key.is_some(),
        bark: config.bark.enabled && config.bark.device_key.is_some(),
    })
}

fn channel_filter_has_ready_channel(
    filter: ImportantNotificationChannelFilter,
    readiness: NotificationChannelReadiness,
) -> bool {
    match filter {
        ImportantNotificationChannelFilter::All => {
            readiness.email || readiness.server_chan || readiness.bark
        }
        ImportantNotificationChannelFilter::Email => readiness.email,
        ImportantNotificationChannelFilter::ServerChan => readiness.server_chan,
        ImportantNotificationChannelFilter::Bark => readiness.bark,
    }
}

async fn dispatch_important_notification(
    state: &AppState,
    item_key: Option<&str>,
    notification: ImportantNotification,
    variables: &[(&str, String)],
    channel_override: Option<ImportantNotificationChannelFilter>,
    bypass_enable_checks: bool,
) -> Result<ImportantNotificationDeliveryReport, GatewayError> {
    let config = read_important_notification_config(state).await?;
    if !bypass_enable_checks && !config.module_enabled {
        return Ok(single_report("module", false, "通知服务未启用"));
    }

    let item = item_key.and_then(|key| find_notification_item(&config, key));
    if !bypass_enable_checks && item.is_some_and(|item| !item.enabled) {
        return Ok(single_report("item", false, "通知项未启用"));
    }
    let notification = apply_notification_item_template(item, notification, variables);
    let channel_filter = channel_override.unwrap_or_else(|| {
        item.and_then(|item| item.channel)
            .unwrap_or(config.default_channel)
    });

    let mut reports = Vec::new();
    if matches!(
        channel_filter,
        ImportantNotificationChannelFilter::All | ImportantNotificationChannelFilter::Email
    ) {
        maybe_send_email_notification(
            state,
            &config,
            &notification,
            bypass_enable_checks,
            &mut reports,
        )
        .await;
    }

    if matches!(
        channel_filter,
        ImportantNotificationChannelFilter::All | ImportantNotificationChannelFilter::ServerChan
    ) {
        maybe_send_server_chan_notification(
            state,
            &config,
            &notification,
            bypass_enable_checks,
            &mut reports,
        )
        .await;
    }

    if matches!(
        channel_filter,
        ImportantNotificationChannelFilter::All | ImportantNotificationChannelFilter::Bark
    ) {
        maybe_send_bark_notification(
            state,
            &config,
            &notification,
            bypass_enable_checks,
            &mut reports,
        )
        .await;
    }

    if reports.is_empty() {
        reports.push(ImportantNotificationChannelReport {
            channel: "none",
            success: false,
            message: "未启用可用的通知通道".to_string(),
        });
    }
    let success = reports.iter().any(|report| report.success);
    Ok(ImportantNotificationDeliveryReport {
        success,
        channels: reports,
    })
}

pub(crate) async fn build_important_notification_test_payload(
    state: &AdminAppState<'_>,
    request_body: Option<&Bytes>,
) -> Result<Value, GatewayError> {
    let request = match request_body.filter(|body| !body.is_empty()) {
        Some(body) => serde_json::from_slice::<ImportantNotificationTestRequest>(body).unwrap_or(
            ImportantNotificationTestRequest {
                channel: None,
                item_key: None,
            },
        ),
        None => ImportantNotificationTestRequest {
            channel: None,
            item_key: None,
        },
    };
    let filter = request.channel.as_deref().and_then(parse_channel_filter);
    let item_key = request
        .item_key
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let variables = vec![
        ("provider_name", "示例 Provider".to_string()),
        ("provider_id", "provider-demo".to_string()),
        ("total_available", "8.0000".to_string()),
        ("threshold_amount", "10.0000".to_string()),
        ("user_email", "user@example.com".to_string()),
        ("balance", "1.0000".to_string()),
    ];
    let report = dispatch_important_notification(
        state.app(),
        item_key,
        ImportantNotification {
            title: "Aether 通知服务测试".to_string(),
            markdown_body: "这是一条来自 Aether 的通知服务测试。".to_string(),
            text_body: "这是一条来自 Aether 的通知服务测试。".to_string(),
        },
        &variables,
        filter,
        true,
    )
    .await?;

    Ok(json!({
        "success": report.success,
        "message": if report.success { "测试通知已发送" } else { "测试通知发送失败" },
        "channels": report.channels,
    }))
}

async fn read_important_notification_config(
    state: &AppState,
) -> Result<ImportantNotificationConfig, GatewayError> {
    let module_enabled = important_notification_module_enabled(state).await?;
    let email_enabled = state
        .read_system_config_json_value(IMPORTANT_NOTIFICATION_EMAIL_ENABLED_KEY)
        .await?;
    let email_recipients = state
        .read_system_config_json_value(IMPORTANT_NOTIFICATION_EMAIL_RECIPIENTS_KEY)
        .await?;
    let default_channel = state
        .read_system_config_json_value(IMPORTANT_NOTIFICATION_DEFAULT_CHANNEL_KEY)
        .await?;
    let items = state
        .read_system_config_json_value(IMPORTANT_NOTIFICATION_ITEMS_KEY)
        .await?;

    Ok(ImportantNotificationConfig {
        module_enabled,
        email_enabled: system_config_bool(email_enabled.as_ref(), false),
        email_recipients: parse_recipient_list(email_recipients.as_ref()),
        default_channel: default_channel
            .as_ref()
            .and_then(|value| value.as_str())
            .and_then(parse_channel_filter)
            .unwrap_or(ImportantNotificationChannelFilter::All),
        items: parse_notification_items(items.as_ref()),
        server_chan: read_server_chan_push_config(state).await?,
        bark: read_bark_push_config(state).await?,
    })
}

fn parse_channel_filter(raw: &str) -> Option<ImportantNotificationChannelFilter> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "all" => Some(ImportantNotificationChannelFilter::All),
        "email" => Some(ImportantNotificationChannelFilter::Email),
        "server_chan" | "serverchan" | "serve_chan" => {
            Some(ImportantNotificationChannelFilter::ServerChan)
        }
        "bark" => Some(ImportantNotificationChannelFilter::Bark),
        "global" | "" => None,
        _ => None,
    }
}

fn parse_notification_items(value: Option<&Value>) -> Vec<ImportantNotificationItemConfig> {
    let Some(Value::Array(items)) = value else {
        return default_notification_items();
    };
    items
        .iter()
        .filter_map(parse_notification_item)
        .collect::<Vec<_>>()
}

fn parse_notification_item(value: &Value) -> Option<ImportantNotificationItemConfig> {
    let item = value.as_object()?;
    let key = item.get("key")?.as_str()?.trim();
    if key.is_empty() {
        return None;
    }
    let name = item
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(key);
    Some(ImportantNotificationItemConfig {
        key: key.to_string(),
        name: name.to_string(),
        enabled: item.get("enabled").and_then(Value::as_bool).unwrap_or(true),
        channel: item
            .get("channel")
            .and_then(Value::as_str)
            .and_then(parse_channel_filter),
        title_template: optional_non_empty_string(item.get("title_template")),
        markdown_template: optional_non_empty_string(item.get("markdown_template")),
        text_template: optional_non_empty_string(item.get("text_template")),
        user_email_enabled: item
            .get("user_email_enabled")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

fn default_notification_items() -> Vec<ImportantNotificationItemConfig> {
    vec![
        ImportantNotificationItemConfig {
            key: PROVIDER_QUOTA_ALERT_ITEM_KEY.to_string(),
            name: "号池额度不足".to_string(),
            enabled: true,
            channel: None,
            title_template: None,
            markdown_template: None,
            text_template: None,
            user_email_enabled: false,
        },
        ImportantNotificationItemConfig {
            key: "provider_pool_abnormal".to_string(),
            name: "号池异常".to_string(),
            enabled: true,
            channel: None,
            title_template: Some("号池异常：{provider_name}".to_string()),
            markdown_template: Some(
                "号池 `{provider_name}` 出现异常，请检查服务状态。".to_string(),
            ),
            text_template: Some("号池 {provider_name} 出现异常，请检查服务状态。".to_string()),
            user_email_enabled: false,
        },
        ImportantNotificationItemConfig {
            key: "user_balance_low".to_string(),
            name: "用户余额不足".to_string(),
            enabled: true,
            channel: Some(ImportantNotificationChannelFilter::Email),
            title_template: Some("余额不足提醒".to_string()),
            markdown_template: Some("你的账户余额已低于提醒阈值，请及时处理。".to_string()),
            text_template: Some("你的账户余额已低于提醒阈值，请及时处理。".to_string()),
            user_email_enabled: true,
        },
    ]
}

fn optional_non_empty_string(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn find_notification_item<'a>(
    config: &'a ImportantNotificationConfig,
    item_key: &str,
) -> Option<&'a ImportantNotificationItemConfig> {
    let item_key = item_key.trim();
    config.items.iter().find(|item| item.key == item_key)
}

fn notification_item_channel_filter(
    config: &ImportantNotificationConfig,
    item_key: &str,
) -> ImportantNotificationChannelFilter {
    find_notification_item(config, item_key)
        .and_then(|item| item.channel)
        .unwrap_or(config.default_channel)
}

fn apply_notification_item_template(
    item: Option<&ImportantNotificationItemConfig>,
    notification: ImportantNotification,
    variables: &[(&str, String)],
) -> ImportantNotification {
    let Some(item) = item else {
        return notification;
    };
    let title = render_template(
        item.title_template.as_deref(),
        &notification.title,
        &notification,
        variables,
    );
    let markdown_body = render_template(
        item.markdown_template.as_deref(),
        &notification.markdown_body,
        &notification,
        variables,
    );
    let text_body = render_template(
        item.text_template.as_deref(),
        &notification.text_body,
        &notification,
        variables,
    );
    ImportantNotification {
        title,
        markdown_body,
        text_body,
    }
}

fn render_template(
    template: Option<&str>,
    fallback: &str,
    notification: &ImportantNotification,
    variables: &[(&str, String)],
) -> String {
    let mut rendered = template
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(fallback)
        .to_string();
    rendered = rendered
        .replace("{title}", &notification.title)
        .replace("{body}", &notification.markdown_body)
        .replace("{text_body}", &notification.text_body);
    for (key, value) in variables {
        rendered = rendered.replace(&format!("{{{}}}", key.trim()), value);
    }
    rendered
}

async fn maybe_send_email_notification(
    state: &AppState,
    config: &ImportantNotificationConfig,
    notification: &ImportantNotification,
    bypass_channel_toggle: bool,
    reports: &mut Vec<ImportantNotificationChannelReport>,
) {
    if !bypass_channel_toggle && !config.email_enabled {
        return;
    }
    if config.email_recipients.is_empty() {
        reports.push(ImportantNotificationChannelReport {
            channel: "email",
            success: false,
            message: "未配置邮件收件人".to_string(),
        });
        return;
    }
    let smtp_config = match read_smtp_delivery_config(state).await {
        Ok(Some(config)) => config,
        Ok(None) => {
            reports.push(ImportantNotificationChannelReport {
                channel: "email",
                success: false,
                message: "SMTP 配置不完整".to_string(),
            });
            return;
        }
        Err(err) => {
            reports.push(ImportantNotificationChannelReport {
                channel: "email",
                success: false,
                message: format!("读取 SMTP 配置失败: {err:?}"),
            });
            return;
        }
    };

    let mut sent = 0usize;
    let mut failed = 0usize;
    for recipient in &config.email_recipients {
        match send_single_email_notification(smtp_config.clone(), recipient, notification).await {
            Ok(()) => sent += 1,
            Err(err) => {
                failed += 1;
                warn!(
                    error = ?err,
                    recipient = %recipient,
                    "failed to send important notification email"
                );
            }
        }
    }

    reports.push(ImportantNotificationChannelReport {
        channel: "email",
        success: sent > 0,
        message: if failed == 0 {
            format!("邮件通知已发送给 {sent} 个收件人")
        } else {
            format!("邮件通知成功 {sent} 个，失败 {failed} 个")
        },
    });
}

async fn send_single_email_notification(
    smtp_config: SmtpDeliveryConfig,
    recipient: &str,
    notification: &ImportantNotification,
) -> Result<(), GatewayError> {
    send_smtp_email(
        smtp_config,
        ComposedEmail {
            to_email: recipient.to_string(),
            subject: notification.title.clone(),
            html_body: build_notification_html(notification),
            text_body: notification.text_body.clone(),
        },
    )
    .await
}

async fn maybe_send_server_chan_notification(
    state: &AppState,
    config: &ImportantNotificationConfig,
    notification: &ImportantNotification,
    bypass_channel_toggle: bool,
    reports: &mut Vec<ImportantNotificationChannelReport>,
) {
    if !bypass_channel_toggle && !config.server_chan.enabled {
        return;
    }
    if config.server_chan.send_key.is_none() {
        reports.push(ImportantNotificationChannelReport {
            channel: "server_chan",
            success: false,
            message: "未配置 Server 酱 SendKey".to_string(),
        });
        return;
    };
    match send_server_chan_push(
        state,
        &config.server_chan,
        &notification.title,
        &notification.markdown_body,
    )
    .await
    {
        Ok(()) => reports.push(ImportantNotificationChannelReport {
            channel: "server_chan",
            success: true,
            message: "Server 酱通知已发送".to_string(),
        }),
        Err(err) => {
            warn!(error = ?err, "failed to send server chan important notification");
            reports.push(ImportantNotificationChannelReport {
                channel: "server_chan",
                success: false,
                message: format!("Server 酱通知发送失败: {err:?}"),
            });
        }
    }
}

async fn maybe_send_bark_notification(
    state: &AppState,
    config: &ImportantNotificationConfig,
    notification: &ImportantNotification,
    bypass_channel_toggle: bool,
    reports: &mut Vec<ImportantNotificationChannelReport>,
) {
    if !bypass_channel_toggle && !config.bark.enabled {
        return;
    }
    if config.bark.device_key.is_none() {
        reports.push(ImportantNotificationChannelReport {
            channel: "bark",
            success: false,
            message: "未配置 Bark Device Key".to_string(),
        });
        return;
    };
    match send_bark_push(
        state,
        &config.bark,
        &notification.title,
        &notification.markdown_body,
    )
    .await
    {
        Ok(()) => reports.push(ImportantNotificationChannelReport {
            channel: "bark",
            success: true,
            message: "Bark 通知已发送".to_string(),
        }),
        Err(err) => {
            warn!(error = ?err, "failed to send bark important notification");
            reports.push(ImportantNotificationChannelReport {
                channel: "bark",
                success: false,
                message: format!("Bark 通知发送失败: {err:?}"),
            });
        }
    }
}

fn single_report(
    channel: &'static str,
    success: bool,
    message: impl Into<String>,
) -> ImportantNotificationDeliveryReport {
    ImportantNotificationDeliveryReport {
        success,
        channels: vec![ImportantNotificationChannelReport {
            channel,
            success,
            message: message.into(),
        }],
    }
}

fn parse_recipient_list(value: Option<&Value>) -> Vec<String> {
    let mut recipients = Vec::new();
    match value {
        Some(Value::Array(items)) => {
            for item in items {
                if let Some(raw) = item.as_str() {
                    push_recipient_parts(&mut recipients, raw);
                }
            }
        }
        Some(Value::String(raw)) => push_recipient_parts(&mut recipients, raw),
        _ => {}
    }
    recipients.sort();
    recipients.dedup();
    recipients
}

fn push_recipient_parts(recipients: &mut Vec<String>, raw: &str) {
    for item in raw
        .split([',', ';', '\n', '\r'])
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        recipients.push(item.to_string());
    }
}

fn build_notification_html(notification: &ImportantNotification) -> String {
    format!(
        "<!doctype html><html><body><h2>{}</h2><pre style=\"font-family:ui-monospace,SFMono-Regular,Menlo,Consolas,monospace;white-space:pre-wrap;line-height:1.6\">{}</pre></body></html>",
        escape_html(&notification.title),
        escape_html(&notification.text_body),
    )
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[cfg(test)]
mod tests {
    use super::{
        apply_notification_item_template, parse_channel_filter, parse_notification_items,
        parse_recipient_list, ImportantNotification, ImportantNotificationChannelFilter,
    };
    use serde_json::json;

    #[test]
    fn parse_recipient_list_accepts_arrays_and_delimiters() {
        assert_eq!(
            parse_recipient_list(Some(&json!([
                "ops@example.com, admin@example.com",
                "ops@example.com"
            ]))),
            vec![
                "admin@example.com".to_string(),
                "ops@example.com".to_string()
            ]
        );
    }

    #[test]
    fn parse_notification_items_reads_channel_and_user_email_flag() {
        let items = parse_notification_items(Some(&json!([
            {
                "key": "user_balance_low",
                "name": "用户余额不足",
                "enabled": true,
                "channel": "email",
                "title_template": "余额提醒",
                "markdown_template": "{user_email}: {balance}",
                "user_email_enabled": true
            }
        ])));

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].key, "user_balance_low");
        assert_eq!(
            items[0].channel,
            Some(ImportantNotificationChannelFilter::Email)
        );
        assert!(items[0].user_email_enabled);
    }

    #[test]
    fn parse_channel_filter_accepts_bark() {
        assert_eq!(
            parse_channel_filter("bark"),
            Some(ImportantNotificationChannelFilter::Bark)
        );
    }

    #[test]
    fn item_template_renders_fallback_and_variables() {
        let items = parse_notification_items(Some(&json!([
            {
                "key": "provider_quota_alert",
                "name": "号池额度不足",
                "title_template": "额度提醒：{provider_name}",
                "markdown_template": "{body}\n剩余：{total_available}",
                "text_template": "{text_body}\n剩余：{total_available}"
            }
        ])));
        let rendered = apply_notification_item_template(
            Some(&items[0]),
            ImportantNotification {
                title: "默认标题".to_string(),
                markdown_body: "默认正文".to_string(),
                text_body: "默认文本".to_string(),
            },
            &[
                ("provider_name", "示例 Provider".to_string()),
                ("total_available", "8.0000".to_string()),
            ],
        );

        assert_eq!(rendered.title, "额度提醒：示例 Provider");
        assert_eq!(rendered.markdown_body, "默认正文\n剩余：8.0000");
        assert_eq!(rendered.text_body, "默认文本\n剩余：8.0000");
    }
}
