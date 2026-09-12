use super::system_config_string;
use crate::{AppState, GatewayError};
use aether_admin::system::{
    admin_email_template_html_is_valid, admin_email_template_subject_is_valid,
    ADMIN_EMAIL_TEMPLATE_MAX_HTML_BYTES, ADMIN_EMAIL_TEMPLATE_MAX_PREVIEW_VALUE_BYTES,
};
use regex::Regex;
use serde_json::json;

const MAX_RENDERED_EMAIL_HTML_BYTES: usize = 512 * 1024;

pub(crate) struct AdminEmailTemplateDefinition {
    pub(crate) template_type: &'static str,
    pub(crate) name: &'static str,
    pub(crate) variables: &'static [&'static str],
    pub(crate) default_subject: &'static str,
    pub(crate) default_html: &'static str,
}

const ADMIN_EMAIL_TEMPLATE_DEFINITIONS: &[AdminEmailTemplateDefinition] = &[
    AdminEmailTemplateDefinition {
        template_type: "verification",
        name: "注册验证码",
        variables: &["app_name", "code", "expire_minutes", "email"],
        default_subject: "验证码",
        default_html: r#"<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>验证码</title>
</head>
<body style="margin: 0; padding: 0; background-color: #faf9f5; font-family: Georgia, 'Times New Roman', 'Songti SC', 'STSong', serif;">
    <table width="100%" cellpadding="0" cellspacing="0" style="background-color: #faf9f5; padding: 40px 20px;">
        <tr>
            <td align="center">
                <table width="100%" cellpadding="0" cellspacing="0" style="max-width: 480px;">
                    <tr>
                        <td style="padding: 0 0 32px; text-align: center;">
                            <div style="font-size: 13px; font-family: 'SF Mono', Monaco, 'Courier New', monospace; color: #6c695c; letter-spacing: 0.15em; text-transform: uppercase;">
                                {{app_name}}
                            </div>
                        </td>
                    </tr>
                    <tr>
                        <td>
                            <table width="100%" cellpadding="0" cellspacing="0" style="background-color: #ffffff; border: 1px solid rgba(61, 57, 41, 0.1); border-radius: 6px;">
                                <tr>
                                    <td style="padding: 48px 40px;">
                                        <h1 style="margin: 0 0 24px; font-size: 24px; font-weight: 500; color: #3d3929; text-align: center; letter-spacing: -0.02em;">
                                            验证码
                                        </h1>
                                        <p style="margin: 0 0 32px; font-size: 15px; color: #6c695c; line-height: 1.7; text-align: center;">
                                            您正在注册账户，请使用以下验证码完成验证。
                                        </p>
                                        <div style="background-color: #faf9f5; border: 1px solid rgba(61, 57, 41, 0.08); border-radius: 4px; padding: 32px 20px; text-align: center; margin-bottom: 32px;">
                                            <div style="font-size: 40px; font-weight: 500; color: #c96442; letter-spacing: 12px; font-family: 'SF Mono', Monaco, 'Courier New', monospace;">
                                                {{code}}
                                            </div>
                                        </div>
                                        <p style="margin: 0; font-size: 14px; color: #6c695c; line-height: 1.6; text-align: center;">
                                            验证码将在 <span style="color: #3d3929; font-weight: 500;">{{expire_minutes}} 分钟</span>后失效
                                        </p>
                                    </td>
                                </tr>
                            </table>
                        </td>
                    </tr>
                    <tr>
                        <td style="padding: 32px 0 0; text-align: center;">
                            <p style="margin: 0 0 8px; font-size: 12px; color: #6c695c;">
                                如果这不是您的操作，请忽略此邮件。
                            </p>
                            <p style="margin: 0; font-size: 11px; color: rgba(108, 105, 92, 0.6);">
                                此邮件由系统自动发送，请勿回复
                            </p>
                        </td>
                    </tr>
                </table>
            </td>
        </tr>
    </table>
</body>
</html>"#,
    },
    AdminEmailTemplateDefinition {
        template_type: "password_reset",
        name: "找回密码",
        variables: &["app_name", "reset_link", "expire_minutes", "email"],
        default_subject: "密码重置",
        default_html: r#"<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>密码重置</title>
</head>
<body style="margin: 0; padding: 0; background-color: #faf9f5; font-family: Georgia, 'Times New Roman', 'Songti SC', 'STSong', serif;">
    <table width="100%" cellpadding="0" cellspacing="0" style="background-color: #faf9f5; padding: 40px 20px;">
        <tr>
            <td align="center">
                <table width="100%" cellpadding="0" cellspacing="0" style="max-width: 480px;">
                    <tr>
                        <td style="padding: 0 0 32px; text-align: center;">
                            <div style="font-size: 13px; font-family: 'SF Mono', Monaco, 'Courier New', monospace; color: #6c695c; letter-spacing: 0.15em; text-transform: uppercase;">
                                {{app_name}}
                            </div>
                        </td>
                    </tr>
                    <tr>
                        <td>
                            <table width="100%" cellpadding="0" cellspacing="0" style="background-color: #ffffff; border: 1px solid rgba(61, 57, 41, 0.1); border-radius: 6px;">
                                <tr>
                                    <td style="padding: 48px 40px;">
                                        <h1 style="margin: 0 0 24px; font-size: 24px; font-weight: 500; color: #3d3929; text-align: center; letter-spacing: -0.02em;">
                                            重置密码
                                        </h1>
                                        <p style="margin: 0 0 32px; font-size: 15px; color: #6c695c; line-height: 1.7; text-align: center;">
                                            您正在重置账户密码，请点击下方按钮完成操作。
                                        </p>
                                        <div style="text-align: center; margin-bottom: 32px;">
                                            <a href="{{reset_link}}" style="display: inline-block; padding: 14px 36px; background-color: #c96442; color: #ffffff; text-decoration: none; border-radius: 4px; font-size: 15px; font-weight: 500; font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif;">
                                                重置密码
                                            </a>
                                        </div>
                                        <p style="margin: 0; font-size: 14px; color: #6c695c; line-height: 1.6; text-align: center;">
                                            链接将在 <span style="color: #3d3929; font-weight: 500;">{{expire_minutes}} 分钟</span>后失效
                                        </p>
                                    </td>
                                </tr>
                            </table>
                        </td>
                    </tr>
                    <tr>
                        <td style="padding: 32px 0 0; text-align: center;">
                            <p style="margin: 0 0 8px; font-size: 12px; color: #6c695c;">
                                如果您没有请求重置密码，请忽略此邮件。
                            </p>
                            <p style="margin: 0; font-size: 11px; color: rgba(108, 105, 92, 0.6);">
                                此邮件由系统自动发送，请勿回复
                            </p>
                        </td>
                    </tr>
                </table>
            </td>
        </tr>
    </table>
</body>
</html>"#,
    },
];

pub(crate) fn admin_email_template_definition(
    template_type: &str,
) -> Option<&'static AdminEmailTemplateDefinition> {
    let normalized = template_type.trim();
    ADMIN_EMAIL_TEMPLATE_DEFINITIONS
        .iter()
        .find(|definition| definition.template_type == normalized)
}

pub(crate) fn admin_email_template_subject_key(template_type: &str) -> String {
    format!("email_template_{template_type}_subject")
}

pub(crate) fn admin_email_template_html_key(template_type: &str) -> String {
    format!("email_template_{template_type}_html")
}

pub(crate) async fn read_admin_email_template_payload(
    state: &AppState,
    template_type: &str,
) -> Result<Option<serde_json::Value>, GatewayError> {
    let Some(definition) = admin_email_template_definition(template_type) else {
        return Ok(None);
    };
    let subject = state
        .read_system_config_json_value(&admin_email_template_subject_key(definition.template_type))
        .await?;
    let html = state
        .read_system_config_json_value(&admin_email_template_html_key(definition.template_type))
        .await?;
    let subject = match system_config_string(subject.as_ref()) {
        Some(value) if admin_email_template_subject_is_valid(&value) => value,
        Some(_) => {
            return Err(GatewayError::Internal(
                "stored email template subject is invalid or oversized".to_string(),
            ));
        }
        None => definition.default_subject.to_string(),
    };
    let html = match system_config_string(html.as_ref()) {
        Some(value) if admin_email_template_html_is_valid(&value) => value,
        Some(_) => {
            return Err(GatewayError::Internal(
                "stored email template html is invalid or oversized".to_string(),
            ));
        }
        None => definition.default_html.to_string(),
    };
    let is_custom = subject != definition.default_subject || html != definition.default_html;

    Ok(Some(json!({
        "type": definition.template_type,
        "name": definition.name,
        "variables": definition.variables,
        "subject": subject,
        "html": html,
        "is_custom": is_custom,
        "default_subject": definition.default_subject,
        "default_html": definition.default_html,
    })))
}

pub(crate) fn escape_admin_email_template_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\"', "&quot;")
        .replace('\'', "&#x27;")
}

pub(crate) fn render_admin_email_template_html(
    template_html: &str,
    variables: &std::collections::BTreeMap<String, String>,
) -> Result<String, GatewayError> {
    if template_html.len() > ADMIN_EMAIL_TEMPLATE_MAX_HTML_BYTES {
        return Err(GatewayError::Internal(
            "email template html exceeds the allowed size".to_string(),
        ));
    }
    if !admin_email_template_html_is_valid(template_html) {
        return Err(GatewayError::Internal(
            "email template html contains invalid control bytes".to_string(),
        ));
    }
    let mut rendered = template_html.to_string();
    for (key, value) in variables {
        if value.len() > ADMIN_EMAIL_TEMPLATE_MAX_PREVIEW_VALUE_BYTES {
            return Err(GatewayError::Internal(
                "email template variable is oversized".to_string(),
            ));
        }
        let pattern = Regex::new(&format!(r"\{{\{{\s*{}\s*\}}\}}", regex::escape(key)))
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        let escaped = escape_admin_email_template_html(value);
        let (matched_bytes, occurrences) = pattern
            .find_iter(&rendered)
            .fold((0usize, 0usize), |(matched, count), found| {
                (matched.saturating_add(found.as_str().len()), count + 1)
            });
        let replacement_bytes = occurrences.checked_mul(escaped.len()).and_then(|bytes| {
            rendered
                .len()
                .checked_sub(matched_bytes)?
                .checked_add(bytes)
        });
        if replacement_bytes.is_none_or(|bytes| bytes > MAX_RENDERED_EMAIL_HTML_BYTES) {
            return Err(GatewayError::Internal(
                "rendered email template exceeds the allowed size".to_string(),
            ));
        }
        rendered = pattern
            .replace_all(&rendered, regex::NoExpand(escaped.as_str()))
            .into_owned();
    }
    if rendered.len() > MAX_RENDERED_EMAIL_HTML_BYTES {
        return Err(GatewayError::Internal(
            "rendered email template exceeds the allowed size".to_string(),
        ));
    }
    Ok(rendered)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn renderer_escapes_values_without_regex_replacement_expansion() {
        let variables = BTreeMap::from([(String::from("app_name"), String::from("$1<&"))]);
        let rendered = render_admin_email_template_html("<p>{{app_name}}</p>", &variables)
            .expect("normal template should render");
        assert_eq!(rendered, "<p>$1&lt;&amp;</p>");
    }

    #[test]
    fn renderer_rejects_control_bytes_and_expansion_bombs() {
        let controls = render_admin_email_template_html("<p>bad\u{0001}</p>", &BTreeMap::new());
        assert!(controls.is_err());

        let template = "{{value}}".repeat(100_000);
        let variables = BTreeMap::from([(String::from("value"), "x".repeat(64))]);
        let error = render_admin_email_template_html(&template, &variables)
            .expect_err("rendered output must remain bounded");
        assert!(format!("{error:?}").contains("exceeds"));
    }
}
