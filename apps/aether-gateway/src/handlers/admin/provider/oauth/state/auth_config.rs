pub(crate) use aether_admin::provider::state::{
    decode_jwt_claims, enrich_admin_provider_oauth_auth_config, json_non_empty_string,
    json_u64_value,
};

#[cfg(test)]
mod tests {
    use super::enrich_admin_provider_oauth_auth_config;
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    use serde_json::json;

    fn sample_unsigned_jwt(payload: serde_json::Value) -> String {
        let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"none","typ":"JWT"}"#);
        let payload = URL_SAFE_NO_PAD.encode(payload.to_string());
        format!("{header}.{payload}.sig")
    }

    #[test]
    fn codex_enrichment_extracts_identity_from_nested_auth_claims() {
        let access_token = sample_unsigned_jwt(json!({
            "email": "u@example.com",
            "https://api.openai.com/auth": {
                "chatgpt_account_id": "acc-1",
                "chatgpt_account_user_id": "user-1__acc-1",
                "chatgpt_plan_type": "team",
                "chatgpt_user_id": "user-1",
                "organizations": [
                    {"id": "org-1", "title": "Personal", "is_default": true}
                ],
            }
        }));
        let token_payload = json!({
            "access_token": access_token,
        });
        let mut auth_config = serde_json::Map::new();

        enrich_admin_provider_oauth_auth_config("codex", &mut auth_config, &token_payload);

        assert_eq!(auth_config.get("email"), Some(&json!("u@example.com")));
        assert_eq!(auth_config.get("account_id"), Some(&json!("acc-1")));
        assert_eq!(
            auth_config.get("account_user_id"),
            Some(&json!("user-1__acc-1"))
        );
        assert_eq!(auth_config.get("plan_type"), Some(&json!("team")));
        assert_eq!(auth_config.get("user_id"), Some(&json!("user-1")));
        assert_eq!(
            auth_config.get("organizations"),
            Some(&json!([
                {"id": "org-1", "title": "Personal", "is_default": true}
            ]))
        );
    }

    #[test]
    fn codex_enrichment_normalizes_direct_chatgpt_alias_fields() {
        let token_payload = json!({
            "email": "alias@example.com",
            "chatgpt_account_id": "acc-2",
            "chatgpt_account_user_id": "user-2__acc-2",
            "chatgpt_plan_type": "plus",
            "chatgpt_user_id": "user-2",
        });
        let mut auth_config = serde_json::Map::new();

        enrich_admin_provider_oauth_auth_config("codex", &mut auth_config, &token_payload);

        assert_eq!(auth_config.get("email"), Some(&json!("alias@example.com")));
        assert_eq!(auth_config.get("account_id"), Some(&json!("acc-2")));
        assert_eq!(
            auth_config.get("account_user_id"),
            Some(&json!("user-2__acc-2"))
        );
        assert_eq!(auth_config.get("plan_type"), Some(&json!("plus")));
        assert_eq!(auth_config.get("user_id"), Some(&json!("user-2")));
    }

    #[test]
    fn codex_enrichment_extracts_profile_email_from_claims() {
        let access_token = sample_unsigned_jwt(json!({
            "https://api.openai.com/profile": {
                "email": "profile@example.com",
                "email_verified": true,
            },
            "https://api.openai.com/auth": {
                "chatgpt_account_id": "acc-profile",
            },
        }));
        let token_payload = json!({
            "access_token": access_token,
        });
        let mut auth_config = serde_json::Map::new();

        enrich_admin_provider_oauth_auth_config("codex", &mut auth_config, &token_payload);

        assert_eq!(
            auth_config.get("email"),
            Some(&json!("profile@example.com"))
        );
        assert_eq!(auth_config.get("account_id"), Some(&json!("acc-profile")));
    }
}
