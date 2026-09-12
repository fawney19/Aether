use crate::handlers::admin::shared::attach_admin_audit_response;
use axum::{
    body::Body,
    response::{IntoResponse, Response},
};
use serde_json::{Map, Value};
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) fn admin_provider_oauth_single_import_audit_taxonomy(
    request_body: Option<&axum::body::Bytes>,
) -> (&'static str, &'static str) {
    let creates_agent_identity = request_body
        .and_then(|body| serde_json::from_slice::<Value>(body).ok())
        .and_then(|value| value.as_object().cloned())
        .is_some_and(|payload| {
            payload
                .get("create_agent_identity")
                .and_then(Value::as_bool)
                == Some(true)
        });
    if creates_agent_identity {
        (
            "admin_provider_oauth_agent_identity_created",
            "create_provider_agent_identity",
        )
    } else {
        (
            "admin_provider_oauth_refresh_token_imported",
            "import_provider_oauth_refresh_token",
        )
    }
}

pub(super) fn attach_admin_provider_oauth_audit_response(
    response: Response<Body>,
    event_name: &'static str,
    action: &'static str,
    target_type: &'static str,
    target_id: Option<String>,
) -> Response<Body> {
    if !response.status().is_success() {
        return response;
    }
    let Some(target_id) = target_id else {
        return response;
    };
    attach_admin_audit_response(response, event_name, action, target_type, &target_id)
}

pub(super) fn admin_provider_oauth_key_name_from_auth_config(
    provider_type: &str,
    auth_config: &Map<String, Value>,
    batch_index: Option<usize>,
) -> String {
    if let Some(email) = trimmed_auth_config_string(auth_config, "email") {
        return email;
    }
    if provider_type.trim().eq_ignore_ascii_case("grok") {
        if let Some(user_id) = trimmed_auth_config_string(auth_config, "user_id") {
            return user_id;
        }
    }

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    match batch_index {
        Some(index) => format!("账号_{timestamp}_{index}"),
        None => format!("账号_{timestamp}"),
    }
}

fn trimmed_auth_config_string(auth_config: &Map<String, Value>, key: &str) -> Option<String> {
    auth_config
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Map};

    const PROVIDER_TYPES: &[&str] = &[
        "codex",
        " Codex ",
        "claude_code",
        "chatgpt_web",
        "gemini_cli",
        "antigravity",
        "grok",
        " Grok ",
        "kiro",
        "windsurf",
    ];

    #[test]
    fn default_key_name_uses_email_without_provider_prefix() {
        let mut auth_config = Map::new();
        auth_config.insert("email".to_string(), json!("  user@example.com  "));

        for provider_type in PROVIDER_TYPES {
            for batch_index in [None, Some(3)] {
                assert_eq!(
                    admin_provider_oauth_key_name_from_auth_config(
                        provider_type,
                        &auth_config,
                        batch_index,
                    ),
                    "user@example.com"
                );
            }
        }
    }

    #[test]
    fn antigravity_default_key_name_uses_email_without_provider_prefix() {
        for email in ["  user@example.com  ", "antigravity_user@example.com"] {
            let mut auth_config = Map::new();
            auth_config.insert("email".to_string(), json!(email));

            for provider_type in ["antigravity", " Antigravity "] {
                for batch_index in [None, Some(3)] {
                    assert_eq!(
                        admin_provider_oauth_key_name_from_auth_config(
                            provider_type,
                            &auth_config,
                            batch_index,
                        ),
                        email.trim()
                    );
                }
            }
        }
    }

    #[test]
    fn default_key_name_preserves_email_with_provider_prefix() {
        for provider_type in PROVIDER_TYPES {
            let email = format!("{}_user@example.com", provider_type.trim());
            let mut auth_config = Map::new();
            auth_config.insert("email".to_string(), json!(email));

            for batch_index in [None, Some(3)] {
                assert_eq!(
                    admin_provider_oauth_key_name_from_auth_config(
                        provider_type,
                        &auth_config,
                        batch_index,
                    ),
                    email
                );
            }
        }
    }

    #[test]
    fn default_key_name_without_email_uses_generic_account_name() {
        for email in [None, Some(""), Some("  ")] {
            let mut auth_config = Map::new();
            if let Some(email) = email {
                auth_config.insert("email".to_string(), json!(email));
            }

            for provider_type in PROVIDER_TYPES {
                for batch_index in [None, Some(3)] {
                    let name = admin_provider_oauth_key_name_from_auth_config(
                        provider_type,
                        &auth_config,
                        batch_index,
                    );
                    let suffix = name.strip_prefix("账号_").expect("generic account prefix");
                    let timestamp = if batch_index.is_some() {
                        suffix.strip_suffix("_3").expect("batch index suffix")
                    } else {
                        suffix
                    };
                    assert!(timestamp.parse::<u64>().is_ok());
                }
            }
        }
    }

    #[test]
    fn grok_default_key_name_uses_full_user_id() {
        let mut auth_config = Map::new();
        auth_config.insert(
            "user_id".to_string(),
            json!("1619039a-0191-4e0a-a490-8f4ad21262c9"),
        );

        for provider_type in ["grok", " Grok "] {
            for batch_index in [None, Some(3)] {
                assert_eq!(
                    admin_provider_oauth_key_name_from_auth_config(
                        provider_type,
                        &auth_config,
                        batch_index,
                    ),
                    "1619039a-0191-4e0a-a490-8f4ad21262c9"
                );
            }
        }
    }

    #[test]
    fn default_key_name_prefers_email_over_grok_user_id() {
        let mut auth_config = Map::new();
        auth_config.insert("email".to_string(), json!("grok@example.com"));
        auth_config.insert("user_id".to_string(), json!("user-1"));

        assert_eq!(
            admin_provider_oauth_key_name_from_auth_config("grok", &auth_config, None),
            "grok@example.com"
        );
    }

    #[test]
    fn batch_default_key_name_keeps_distinct_indexes_without_provider_prefix() {
        let auth_config = Map::new();
        let name = admin_provider_oauth_key_name_from_auth_config("grok", &auth_config, Some(3));
        let other_name =
            admin_provider_oauth_key_name_from_auth_config("grok", &auth_config, Some(4));

        assert!(name.starts_with("账号_"));
        assert!(name.ends_with("_3"));
        assert!(other_name.starts_with("账号_"));
        assert!(other_name.ends_with("_4"));
        assert_ne!(name, other_name);
    }

    #[test]
    fn single_import_audit_distinguishes_agent_identity_creation_without_exposing_input() {
        let body = axum::body::Bytes::from(
            json!({
                "create_agent_identity": true,
                "access_token": "secret-access-token"
            })
            .to_string(),
        );
        assert_eq!(
            admin_provider_oauth_single_import_audit_taxonomy(Some(&body)),
            (
                "admin_provider_oauth_agent_identity_created",
                "create_provider_agent_identity",
            )
        );
    }

    #[test]
    fn single_import_audit_rejects_removed_session_token_creation_alias() {
        let body = axum::body::Bytes::from(
            json!({
                "create_agent_identity_from_session_token": true,
                "access_token": "secret-access-token"
            })
            .to_string(),
        );
        assert_eq!(
            admin_provider_oauth_single_import_audit_taxonomy(Some(&body)),
            (
                "admin_provider_oauth_refresh_token_imported",
                "import_provider_oauth_refresh_token",
            )
        );
    }

    #[test]
    fn single_import_audit_keeps_standard_import_taxonomy() {
        let body =
            axum::body::Bytes::from(json!({ "refresh_token": "secret-refresh-token" }).to_string());
        assert_eq!(
            admin_provider_oauth_single_import_audit_taxonomy(Some(&body)),
            (
                "admin_provider_oauth_refresh_token_imported",
                "import_provider_oauth_refresh_token",
            )
        );
    }
}
