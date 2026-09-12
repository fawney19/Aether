use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use hmac::Mac;
use serde_json::{Map, Value};

const TEST_JWT_SECRET: &str = "aether-rust-test-jwt-secret-32-bytes-minimum";
const INSECURE_JWT_SECRETS: &[&str] = &[
    "change-this-to-a-secure-random-string",
    "aether-rust-dev-jwt-secret",
    TEST_JWT_SECRET,
];
const INVALID_TOKEN: &str = "无效的Token";
const MAX_LOCAL_AUTH_TOKEN_BYTES: usize = 128 * 1024;
const MAX_LOCAL_AUTH_JWT_PART_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LocalAuthTokenType {
    Access,
    Refresh,
}

impl LocalAuthTokenType {
    fn as_str(self) -> &'static str {
        match self {
            Self::Access => "access",
            Self::Refresh => "refresh",
        }
    }
}

fn validate_jwt_secret_value(value: Option<&str>) -> Result<String, String> {
    let Some(value) = value else {
        return Err("JWT_SECRET_KEY 未配置".to_string());
    };
    let value = value.trim();
    if value.as_bytes().len() < 32 || INSECURE_JWT_SECRETS.contains(&value) {
        return Err("JWT_SECRET_KEY 必须是至少32字节的非默认随机密钥".to_string());
    }
    Ok(value.to_string())
}

pub(crate) fn local_auth_jwt_secret() -> Result<String, String> {
    match std::env::var("JWT_SECRET_KEY") {
        Ok(value) => validate_jwt_secret_value(Some(&value)),
        Err(std::env::VarError::NotPresent) => {
            #[cfg(test)]
            {
                Ok(TEST_JWT_SECRET.to_string())
            }

            #[cfg(not(test))]
            Err("JWT_SECRET_KEY 未配置".to_string())
        }
        Err(std::env::VarError::NotUnicode(_)) => {
            Err("JWT_SECRET_KEY 必须是有效的UTF-8字符串".to_string())
        }
    }
}

fn base64url_encode(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

fn base64url_decode(value: &str) -> Result<Vec<u8>, String> {
    let max_encoded_len = MAX_LOCAL_AUTH_JWT_PART_BYTES
        .saturating_add(2)
        .checked_div(3)
        .unwrap_or(usize::MAX)
        .saturating_mul(4);
    if value.len() > max_encoded_len {
        return Err(INVALID_TOKEN.to_string());
    }
    let decoded = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| INVALID_TOKEN.to_string())?;
    (decoded.len() <= MAX_LOCAL_AUTH_JWT_PART_BYTES)
        .then_some(decoded)
        .ok_or_else(|| INVALID_TOKEN.to_string())
}

fn non_empty_string_claim<'a>(payload: &'a Map<String, Value>, name: &str) -> Option<&'a str> {
    payload
        .get(name)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn local_auth_claims_are_valid(
    payload: &Map<String, Value>,
    token_type: LocalAuthTokenType,
) -> bool {
    if non_empty_string_claim(payload, "user_id").is_none()
        || non_empty_string_claim(payload, "session_id").is_none()
    {
        return false;
    }
    if !matches!(
        payload.get("created_at"),
        Some(Value::String(value)) if chrono::DateTime::parse_from_rfc3339(value).is_ok()
    ) {
        return false;
    }
    match token_type {
        LocalAuthTokenType::Access => non_empty_string_claim(payload, "role").is_some(),
        LocalAuthTokenType::Refresh => non_empty_string_claim(payload, "jti").is_some(),
    }
}

pub(crate) fn create_local_auth_token(
    token_type: LocalAuthTokenType,
    mut payload: Map<String, Value>,
    expires_at: chrono::DateTime<chrono::Utc>,
) -> Result<String, String> {
    let secret = local_auth_jwt_secret()?;
    let header = serde_json::json!({ "alg": "HS256", "typ": "JWT" });
    payload.insert("exp".to_string(), serde_json::json!(expires_at.timestamp()));
    payload.insert("type".to_string(), serde_json::json!(token_type.as_str()));
    if !local_auth_claims_are_valid(&payload, token_type) {
        return Err("无法签发缺少必要身份声明的Token".to_string());
    }
    let header_segment = base64url_encode(
        &serde_json::to_vec(&header).map_err(|_| "无法序列化JWT header".to_string())?,
    );
    let payload_segment = base64url_encode(
        &serde_json::to_vec(&payload).map_err(|_| "无法序列化JWT payload".to_string())?,
    );
    let signing_input = format!("{header_segment}.{payload_segment}");
    let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(secret.as_bytes())
        .map_err(|_| "JWT secret 无效".to_string())?;
    mac.update(signing_input.as_bytes());
    let signature = mac.finalize().into_bytes();
    Ok(format!(
        "{signing_input}.{}",
        base64url_encode(signature.as_slice())
    ))
}

pub(crate) fn decode_local_auth_token(
    token: &str,
    expected_type: LocalAuthTokenType,
) -> Result<Map<String, Value>, String> {
    if token.len() > MAX_LOCAL_AUTH_TOKEN_BYTES {
        return Err(INVALID_TOKEN.to_string());
    }
    let mut parts = token.split('.');
    let (Some(header_segment), Some(payload_segment), Some(signature_segment)) =
        (parts.next(), parts.next(), parts.next())
    else {
        return Err(INVALID_TOKEN.to_string());
    };
    if header_segment.is_empty()
        || payload_segment.is_empty()
        || signature_segment.is_empty()
        || parts.next().is_some()
    {
        return Err(INVALID_TOKEN.to_string());
    }

    let signature = base64url_decode(signature_segment)?;
    if signature.len() != 32 {
        return Err(INVALID_TOKEN.to_string());
    }
    let secret = local_auth_jwt_secret()?;
    let signing_input = format!("{header_segment}.{payload_segment}");
    let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(secret.as_bytes())
        .map_err(|_| "JWT secret 无效".to_string())?;
    mac.update(signing_input.as_bytes());
    mac.verify_slice(&signature)
        .map_err(|_| INVALID_TOKEN.to_string())?;

    let header_bytes = base64url_decode(header_segment)?;
    let header =
        serde_json::from_slice::<Value>(&header_bytes).map_err(|_| INVALID_TOKEN.to_string())?;
    if header.get("alg").and_then(Value::as_str) != Some("HS256")
        || header.get("typ").and_then(Value::as_str) != Some("JWT")
    {
        return Err(INVALID_TOKEN.to_string());
    }

    let payload_bytes = base64url_decode(payload_segment)?;
    let payload =
        serde_json::from_slice::<Value>(&payload_bytes).map_err(|_| INVALID_TOKEN.to_string())?;
    let payload = payload
        .as_object()
        .cloned()
        .ok_or_else(|| INVALID_TOKEN.to_string())?;
    let actual_type = payload
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if actual_type != expected_type.as_str() {
        return Err(format!(
            "Token类型错误: 期望 {}, 实际 {actual_type}",
            expected_type.as_str()
        ));
    }
    let exp = payload
        .get("exp")
        .and_then(Value::as_i64)
        .ok_or_else(|| INVALID_TOKEN.to_string())?;
    if exp <= chrono::Utc::now().timestamp() {
        return Err("Token已过期".to_string());
    }
    if !local_auth_claims_are_valid(&payload, expected_type) {
        return Err(INVALID_TOKEN.to_string());
    }
    Ok(payload)
}

pub(crate) fn local_auth_token_identity_matches_user(
    payload: &Map<String, Value>,
    user: &aether_data::repository::users::StoredUserAuthRecord,
) -> bool {
    if non_empty_string_claim(payload, "user_id") != Some(user.id.as_str()) {
        return false;
    }

    // Access tokens carry the role used by downstream authorization checks.
    // Refresh tokens intentionally omit it, so only validate the claim when
    // present.  A present-but-malformed role must never be treated as absent.
    if let Some(token_role) = payload.get("role") {
        if token_role.as_str() != Some(user.role.as_str()) {
            return false;
        }
    }

    if let Some(token_email) = payload.get("email") {
        match (token_email.as_str(), user.email.as_deref()) {
            (Some(token_email), Some(email)) if token_email == email => {}
            (None, None) if token_email.is_null() => {}
            _ => return false,
        }
    }

    match (payload.get("created_at"), user.created_at) {
        (Some(Value::String(token_created_at)), Some(user_created_at)) => {
            let Ok(token_created_at) = chrono::DateTime::parse_from_rfc3339(token_created_at)
            else {
                return false;
            };
            let token_created_at = token_created_at.with_timezone(&chrono::Utc);
            user_created_at == token_created_at
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        create_local_auth_token, decode_local_auth_token, local_auth_jwt_secret,
        local_auth_token_identity_matches_user, validate_jwt_secret_value, LocalAuthTokenType,
        MAX_LOCAL_AUTH_TOKEN_BYTES, TEST_JWT_SECRET,
    };
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    use hmac::Mac;
    use serde_json::{json, Map, Value};

    fn signed_token(header: Value, payload: Value) -> String {
        let header =
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&header).expect("header should encode"));
        let payload =
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).expect("payload should encode"));
        let signing_input = format!("{header}.{payload}");
        let secret = local_auth_jwt_secret().expect("test JWT secret should resolve");
        let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(secret.as_bytes())
            .expect("test JWT secret should sign");
        mac.update(signing_input.as_bytes());
        format!(
            "{signing_input}.{}",
            URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
        )
    }

    fn access_claims() -> Map<String, Value> {
        Map::from_iter([
            ("user_id".to_string(), json!("user-1")),
            ("session_id".to_string(), json!("session-1")),
            ("role".to_string(), json!("user")),
            (
                "created_at".to_string(),
                json!(chrono::Utc::now().to_rfc3339()),
            ),
        ])
    }

    #[test]
    fn jwt_secret_validation_fails_closed() {
        assert!(validate_jwt_secret_value(None).is_err());
        assert!(validate_jwt_secret_value(Some("short-secret")).is_err());
        assert!(validate_jwt_secret_value(Some("change-this-to-a-secure-random-string")).is_err());
        assert!(validate_jwt_secret_value(Some("aether-rust-dev-jwt-secret")).is_err());
        assert!(validate_jwt_secret_value(Some(TEST_JWT_SECRET)).is_err());
        assert!(validate_jwt_secret_value(Some(
            "a-valid-local-auth-secret-with-at-least-32-bytes"
        ))
        .is_ok());
    }

    #[test]
    fn access_and_refresh_tokens_cannot_be_interchanged() {
        let expires_at = chrono::Utc::now() + chrono::Duration::minutes(5);
        let access_claims = access_claims();
        let mut refresh_claims = access_claims.clone();
        refresh_claims.remove("role");
        refresh_claims.insert("jti".to_string(), json!("refresh-1"));
        let access = create_local_auth_token(LocalAuthTokenType::Access, access_claims, expires_at)
            .expect("access token should encode");
        let refresh =
            create_local_auth_token(LocalAuthTokenType::Refresh, refresh_claims, expires_at)
                .expect("refresh token should encode");

        assert!(decode_local_auth_token(&access, LocalAuthTokenType::Access).is_ok());
        assert!(decode_local_auth_token(&refresh, LocalAuthTokenType::Refresh).is_ok());
        assert!(decode_local_auth_token(&access, LocalAuthTokenType::Refresh).is_err());
        assert!(decode_local_auth_token(&refresh, LocalAuthTokenType::Access).is_err());
    }

    #[test]
    fn decoder_rejects_expired_and_malformed_tokens() {
        let expired = create_local_auth_token(
            LocalAuthTokenType::Access,
            access_claims(),
            chrono::Utc::now() - chrono::Duration::seconds(1),
        )
        .expect("expired token should still encode");

        assert_eq!(
            decode_local_auth_token(&expired, LocalAuthTokenType::Access),
            Err("Token已过期".to_string())
        );
        for malformed in ["", "one", "one.two", "one.two.three.four", ".."] {
            assert!(decode_local_auth_token(malformed, LocalAuthTokenType::Access).is_err());
        }

        let oversized = "a".repeat(MAX_LOCAL_AUTH_TOKEN_BYTES + 1);
        assert!(decode_local_auth_token(&oversized, LocalAuthTokenType::Access).is_err());
    }

    #[test]
    fn decoder_rejects_wrong_header_and_required_claim_shapes() {
        let exp = (chrono::Utc::now() + chrono::Duration::minutes(5)).timestamp();
        let valid_payload = json!({
            "user_id": "user-1",
            "session_id": "session-1",
            "role": "user",
            "created_at": chrono::Utc::now().to_rfc3339(),
            "type": "access",
            "exp": exp,
        });
        for header in [
            json!({"alg": "none", "typ": "JWT"}),
            json!({"alg": "HS256", "typ": "JWS"}),
        ] {
            let token = signed_token(header, valid_payload.clone());
            assert!(decode_local_auth_token(&token, LocalAuthTokenType::Access).is_err());
        }

        for payload in [
            json!({"session_id": "session-1", "role": "user", "created_at": null, "type": "access", "exp": exp}),
            json!({"user_id": "user-1", "session_id": "session-1", "created_at": null, "type": "access", "exp": exp}),
            json!({"user_id": "user-1", "session_id": "session-1", "role": "user", "type": "access", "exp": exp}),
            json!({"user_id": "user-1", "session_id": "session-1", "role": "user", "created_at": null, "type": "access", "exp": exp}),
            json!({"user_id": "user-1", "session_id": "session-1", "role": "user", "created_at": null, "type": "access", "exp": "later"}),
        ] {
            let token = signed_token(json!({"alg": "HS256", "typ": "JWT"}), payload);
            assert!(decode_local_auth_token(&token, LocalAuthTokenType::Access).is_err());
        }
    }

    #[test]
    fn issuer_rejects_null_created_at_identity_binding() {
        let mut claims = access_claims();
        claims.insert("created_at".to_string(), Value::Null);

        assert!(create_local_auth_token(
            LocalAuthTokenType::Access,
            claims,
            chrono::Utc::now() + chrono::Duration::minutes(5),
        )
        .is_err());
    }

    #[test]
    fn identity_binding_rejects_even_subsecond_user_replacement() {
        let created_at = chrono::Utc::now();
        let user = aether_data::repository::users::StoredUserAuthRecord::new(
            "user-1".to_string(),
            Some("user-1@example.com".to_string()),
            true,
            "user-1".to_string(),
            None,
            "user".to_string(),
            "local".to_string(),
            None,
            None,
            None,
            true,
            false,
            Some(created_at + chrono::Duration::milliseconds(1)),
            None,
        )
        .expect("test user should be valid");
        let claims = Map::from_iter([
            ("created_at".to_string(), json!(created_at.to_rfc3339())),
            ("user_id".to_string(), json!("user-1")),
        ]);

        assert!(!local_auth_token_identity_matches_user(&claims, &user));

        let mut replaced_id_claims = claims;
        replaced_id_claims.insert("created_at".to_string(), json!(user.created_at));
        replaced_id_claims.insert("user_id".to_string(), json!("user-2"));
        assert!(!local_auth_token_identity_matches_user(
            &replaced_id_claims,
            &user
        ));
    }

    #[test]
    fn identity_binding_validates_present_role_claim() {
        let created_at = chrono::Utc::now();
        let user = aether_data::repository::users::StoredUserAuthRecord::new(
            "user-1".to_string(),
            Some("user-1@example.com".to_string()),
            true,
            "user-1".to_string(),
            None,
            "admin".to_string(),
            "local".to_string(),
            None,
            None,
            None,
            true,
            false,
            Some(created_at),
            None,
        )
        .expect("test user should be valid");
        let mut claims = Map::from_iter([
            ("created_at".to_string(), json!(created_at.to_rfc3339())),
            ("user_id".to_string(), json!("user-1")),
        ]);

        // Refresh tokens intentionally omit role, so this remains valid.
        assert!(local_auth_token_identity_matches_user(&claims, &user));

        claims.insert("role".to_string(), json!("admin"));
        assert!(local_auth_token_identity_matches_user(&claims, &user));

        claims.insert("role".to_string(), json!("user"));
        assert!(!local_auth_token_identity_matches_user(&claims, &user));

        claims.insert("role".to_string(), Value::Null);
        assert!(!local_auth_token_identity_matches_user(&claims, &user));
    }
}
