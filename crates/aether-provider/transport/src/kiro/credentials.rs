pub use aether_oauth::provider::providers::{
    generate_kiro_machine_id as generate_machine_id,
    normalize_kiro_machine_id as normalize_machine_id, KiroAuthConfig, DEFAULT_REGION,
};

#[cfg(test)]
mod tests {
    use super::{generate_machine_id, normalize_machine_id, KiroAuthConfig, DEFAULT_REGION};

    #[test]
    fn normalizes_uuid_machine_id() {
        assert_eq!(
            normalize_machine_id("123e4567-e89b-12d3-a456-426614174000").as_deref(),
            Some("123e4567e89b12d3a456426614174000123e4567e89b12d3a456426614174000")
        );
    }

    #[test]
    fn hashes_refresh_token_into_machine_id() {
        let auth_config = KiroAuthConfig {
            auth_method: None,
            refresh_token: Some("r".repeat(128)),
            expires_at: None,
            profile_arn: None,
            region: None,
            auth_region: None,
            api_region: None,
            client_id: None,
            client_secret: None,
            machine_id: None,
            kiro_version: None,
            system_version: None,
            node_version: None,
            access_token: None,
        };

        let machine_id = generate_machine_id(&auth_config, None).expect("machine id should exist");
        assert_eq!(machine_id.len(), 64);
    }

    #[test]
    fn parses_auth_config_aliases() {
        let auth_config = KiroAuthConfig::from_raw_json(Some(
            r#"{
                "authMethod":"identity_center",
                "refreshToken":"rrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrr",
                "expires_at": 4102444800,
                "profileArn":"arn:aws:bedrock:demo",
                "apiRegion":"us-west-2",
                "clientId":"cid",
                "clientSecret":"secret",
                "machineId":"123e4567-e89b-12d3-a456-426614174000",
                "kiroVersion":"1.2.3",
                "systemVersion":"darwin#24.6.0",
                "nodeVersion":"22.21.1",
                "accessToken":"cached-token"
            }"#,
        ))
        .expect("auth config should parse");

        assert_eq!(auth_config.auth_method.as_deref(), Some("idc"));
        assert_eq!(
            auth_config.refresh_token.as_deref(),
            Some(
                "rrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrr"
            )
        );
        assert_eq!(auth_config.expires_at, Some(4_102_444_800));
        assert_eq!(
            auth_config.profile_arn.as_deref(),
            Some("arn:aws:bedrock:demo")
        );
        assert_eq!(auth_config.client_id.as_deref(), Some("cid"));
        assert_eq!(auth_config.client_secret.as_deref(), Some("secret"));
        assert_eq!(auth_config.effective_api_region(), "us-west-2");
        assert_eq!(auth_config.effective_kiro_version(), "1.2.3");
        assert_eq!(auth_config.effective_system_version(), "darwin#24.6.0");
        assert_eq!(auth_config.effective_node_version(), "22.21.1");
        assert_eq!(auth_config.access_token.as_deref(), Some("cached-token"));
        assert!(auth_config.is_idc_auth());
        assert!(auth_config.profile_arn_for_payload().is_none());
        assert_eq!(auth_config.effective_auth_region(), "us-east-1");
        assert!(auth_config.can_refresh_access_token());
        assert_eq!(DEFAULT_REGION, "us-east-1");
    }

    #[test]
    fn preserves_external_idp_auth_method_for_header_selection() {
        let auth_config = KiroAuthConfig::from_raw_json(Some(
            r#"{
                "authMethod":"external_idp",
                "refreshToken":"rt-1",
                "clientId":"cid",
                "clientSecret":"secret",
                "profileArn":"arn:aws:bedrock:demo"
            }"#,
        ))
        .expect("auth config should parse");

        assert_eq!(auth_config.auth_method.as_deref(), Some("external_idp"));
        assert!(auth_config.is_idc_auth());
        assert!(auth_config.uses_external_idp_token_type());
        assert!(auth_config.profile_arn_for_payload().is_none());
        assert_eq!(
            auth_config.profile_arn_for_mcp(),
            Some("arn:aws:bedrock:demo")
        );
    }

    #[test]
    fn infers_idc_when_client_credentials_exist() {
        let auth_config = KiroAuthConfig::from_raw_json(Some(
            r#"{
                "refreshToken":"rt-1",
                "clientId":"cid",
                "clientSecret":"secret",
                "profileArn":"arn:aws:bedrock:demo"
            }"#,
        ))
        .expect("auth config should parse");

        assert!(auth_config.is_idc_auth());
        assert!(!auth_config.uses_external_idp_token_type());
        assert!(auth_config.profile_arn_for_payload().is_none());
        assert_eq!(
            auth_config.profile_arn_for_mcp(),
            Some("arn:aws:bedrock:demo")
        );
    }

    #[test]
    fn round_trips_json_value() {
        let auth_config = KiroAuthConfig::from_raw_json(Some(
            r#"{
                "auth_method":"social",
                "refreshToken":"rt-1....................................................................................................",
                "expires_at": 4102444800,
                "profileArn":"arn:aws:bedrock:demo",
                "region":"eu-north-1",
                "apiRegion":"us-west-2",
                "machineId":"123e4567-e89b-12d3-a456-426614174000",
                "kiroVersion":"1.2.3",
                "systemVersion":"darwin#24.6.0",
                "nodeVersion":"22.21.1",
                "accessToken":"cached-token"
            }"#,
        ))
        .expect("auth config should parse");

        let value = auth_config.to_json_value();
        let reparsed = KiroAuthConfig::from_json_value(&value).expect("auth config should reparse");
        assert_eq!(reparsed, auth_config);
    }
}
