use crate::handlers::admin::request::AdminAppState;
use crate::handlers::shared::{
    admin_email_template_definition, admin_email_template_html_key,
    admin_email_template_subject_key, render_admin_email_template_html, system_config_string,
};
use crate::GatewayError;
use aether_admin::system::{
    admin_email_template_not_found_error, build_admin_email_template_preview_payload,
    build_admin_email_template_reset_payload, build_admin_email_template_saved_payload,
    build_admin_email_templates_payload as build_admin_email_templates_payload_value,
    parse_admin_email_template_preview_payload, parse_admin_email_template_update,
};
use axum::{body::Bytes, http};
use serde_json::json;
use std::collections::BTreeMap;

impl<'a> AdminAppState<'a> {
    pub(crate) async fn build_admin_email_templates_payload(
        &self,
    ) -> Result<serde_json::Value, GatewayError> {
        let mut templates = Vec::new();
        for template_type in ["verification", "password_reset"] {
            if let Some(payload) = self
                .read_admin_email_template_payload(template_type)
                .await?
            {
                let mut payload = payload;
                if let Some(object) = payload.as_object_mut() {
                    object.remove("default_subject");
                    object.remove("default_html");
                }
                templates.push(payload);
            }
        }

        Ok(build_admin_email_templates_payload_value(templates))
    }

    pub(crate) async fn build_admin_email_template_payload(
        &self,
        template_type: &str,
    ) -> Result<Result<serde_json::Value, (http::StatusCode, serde_json::Value)>, GatewayError>
    {
        let Some(payload) = self
            .read_admin_email_template_payload(template_type)
            .await?
        else {
            return Ok(Err(admin_email_template_not_found_error(template_type)));
        };
        Ok(Ok(payload))
    }

    pub(crate) async fn apply_admin_email_template_update(
        &self,
        template_type: &str,
        request_body: &Bytes,
    ) -> Result<Result<serde_json::Value, (http::StatusCode, serde_json::Value)>, GatewayError>
    {
        let Some(definition) = admin_email_template_definition(template_type) else {
            return Ok(Err(admin_email_template_not_found_error(template_type)));
        };
        let update = match parse_admin_email_template_update(request_body) {
            Ok(update) => update,
            Err(err) => return Ok(Err(err)),
        };

        let subject_key = admin_email_template_subject_key(definition.template_type);
        let html_key = admin_email_template_html_key(definition.template_type);

        if let Some(subject) = update.subject {
            if subject.is_empty() {
                let _ = self.delete_system_config_value(&subject_key).await?;
            } else {
                let _ = self
                    .upsert_system_config_json_value(
                        &subject_key,
                        &serde_json::json!(subject),
                        None,
                    )
                    .await?;
            }
        }

        if let Some(html) = update.html {
            if html.is_empty() {
                let _ = self.delete_system_config_value(&html_key).await?;
            } else {
                let _ = self
                    .upsert_system_config_json_value(&html_key, &serde_json::json!(html), None)
                    .await?;
            }
        }

        Ok(Ok(build_admin_email_template_saved_payload()))
    }

    pub(crate) async fn preview_admin_email_template(
        &self,
        template_type: &str,
        request_body: Option<&Bytes>,
    ) -> Result<Result<serde_json::Value, (http::StatusCode, serde_json::Value)>, GatewayError>
    {
        let Some(definition) = admin_email_template_definition(template_type) else {
            return Ok(Err(admin_email_template_not_found_error(template_type)));
        };

        let payload = match parse_admin_email_template_preview_payload(
            request_body.map(|bytes| bytes.as_ref()),
        ) {
            Ok(payload) => payload,
            Err(err) => return Ok(Err(err)),
        };

        let resolved = self
            .read_admin_email_template_payload(definition.template_type)
            .await?
            .expect("validated template type should exist");
        let resolved_html = resolved["html"].as_str().unwrap_or(definition.default_html);
        let html = payload
            .get("html")
            .and_then(|value| value.as_str())
            .filter(|value| !value.is_empty())
            .unwrap_or(resolved_html);

        let email_app_name = self.read_system_config_json_value("email_app_name").await?;
        let smtp_from_name = self.read_system_config_json_value("smtp_from_name").await?;
        let app_name = system_config_string(email_app_name.as_ref())
            .or_else(|| system_config_string(smtp_from_name.as_ref()))
            .unwrap_or_else(|| "Aether".to_string());

        let mut defaults = BTreeMap::new();
        defaults.insert("app_name".to_string(), app_name);
        defaults.insert("code".to_string(), "123456".to_string());
        defaults.insert("expire_minutes".to_string(), "30".to_string());
        defaults.insert("email".to_string(), "example@example.com".to_string());
        defaults.insert(
            "reset_link".to_string(),
            "https://example.com/reset?token=abc123".to_string(),
        );

        let preview_variables = definition
            .variables
            .iter()
            .map(|key| {
                let value = payload
                    .get(*key)
                    .map(|value| match value {
                        serde_json::Value::String(value) => value.clone(),
                        serde_json::Value::Null => "None".to_string(),
                        _ => value.to_string(),
                    })
                    .or_else(|| defaults.get(*key).cloned())
                    .unwrap_or_else(|| format!("{{{{{key}}}}}"));
                ((*key).to_string(), value)
            })
            .collect::<BTreeMap<_, _>>();

        let rendered_html = render_admin_email_template_html(html, &preview_variables)?;

        Ok(Ok(build_admin_email_template_preview_payload(
            rendered_html,
            preview_variables,
        )))
    }

    pub(crate) async fn reset_admin_email_template(
        &self,
        template_type: &str,
    ) -> Result<Result<serde_json::Value, (http::StatusCode, serde_json::Value)>, GatewayError>
    {
        let Some(definition) = admin_email_template_definition(template_type) else {
            return Ok(Err(admin_email_template_not_found_error(template_type)));
        };

        let _ = self
            .delete_system_config_value(&admin_email_template_subject_key(definition.template_type))
            .await?;
        let _ = self
            .delete_system_config_value(&admin_email_template_html_key(definition.template_type))
            .await?;

        Ok(Ok(build_admin_email_template_reset_payload(
            definition.template_type,
            definition.name,
            definition.default_subject,
            definition.default_html,
        )))
    }
}
