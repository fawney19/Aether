use std::net::IpAddr;

use aether_data::repository::management_tokens::{
    StoredManagementToken, StoredManagementTokenWithUser,
};
use aether_data::repository::users::StoredUserAuthRecord;
use axum::http::{self, HeaderMap};
use sha2::{Digest, Sha256};

use crate::data::GatewayDataState;

const MANAGEMENT_TOKEN_MAX_LEN: usize = 512;

#[derive(Debug, Clone)]
pub(crate) struct AuthenticatedManagementToken {
    pub(crate) token: StoredManagementToken,
    pub(crate) user: StoredUserAuthRecord,
    pub(crate) permissions: Vec<String>,
    pub(crate) verified_token_hash: VerifiedManagementTokenHash,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct VerifiedManagementTokenHash(String);

impl VerifiedManagementTokenHash {
    pub(crate) fn new(value: String) -> Self {
        Self(value)
    }

    pub(crate) fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl std::fmt::Debug for VerifiedManagementTokenHash {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("VerifiedManagementTokenHash([REDACTED])")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ManagementTokenAuthError {
    Missing,
    Invalid,
    Unavailable,
}

#[async_trait::async_trait]
pub(crate) trait ManagementTokenAuthSource {
    async fn find_management_token(
        &self,
        token_hash: &str,
    ) -> Result<Option<StoredManagementTokenWithUser>, ManagementTokenAuthError>;

    async fn find_current_user(
        &self,
        user_id: &str,
    ) -> Result<Option<StoredUserAuthRecord>, ManagementTokenAuthError>;
}

#[async_trait::async_trait]
impl ManagementTokenAuthSource for GatewayDataState {
    async fn find_management_token(
        &self,
        token_hash: &str,
    ) -> Result<Option<StoredManagementTokenWithUser>, ManagementTokenAuthError> {
        self.get_management_token_with_user_by_hash(token_hash)
            .await
            .map_err(|_| ManagementTokenAuthError::Unavailable)
    }

    async fn find_current_user(
        &self,
        user_id: &str,
    ) -> Result<Option<StoredUserAuthRecord>, ManagementTokenAuthError> {
        self.find_user_auth_by_id(user_id)
            .await
            .map_err(|_| ManagementTokenAuthError::Unavailable)
    }
}

#[async_trait::async_trait]
impl ManagementTokenAuthSource for crate::AppState {
    async fn find_management_token(
        &self,
        token_hash: &str,
    ) -> Result<Option<StoredManagementTokenWithUser>, ManagementTokenAuthError> {
        self.get_management_token_with_user_by_hash(token_hash)
            .await
            .map_err(|_| ManagementTokenAuthError::Unavailable)
    }

    async fn find_current_user(
        &self,
        user_id: &str,
    ) -> Result<Option<StoredUserAuthRecord>, ManagementTokenAuthError> {
        self.find_user_auth_by_id(user_id)
            .await
            .map_err(|_| ManagementTokenAuthError::Unavailable)
    }
}

pub(crate) async fn authenticate_management_token<S>(
    source: &S,
    headers: &HeaderMap,
    remote_ip: IpAddr,
) -> Result<AuthenticatedManagementToken, ManagementTokenAuthError>
where
    S: ManagementTokenAuthSource + Sync + ?Sized,
{
    let token = extract_unique_management_token_bearer(headers)?;
    let token_hash = VerifiedManagementTokenHash::new(hash_management_token(token));
    authenticate_management_token_hash(source, &token_hash, remote_ip).await
}

pub(crate) async fn authenticate_management_token_hash<S>(
    source: &S,
    token_hash: &VerifiedManagementTokenHash,
    remote_ip: IpAddr,
) -> Result<AuthenticatedManagementToken, ManagementTokenAuthError>
where
    S: ManagementTokenAuthSource + Sync + ?Sized,
{
    let token_with_user = source
        .find_management_token(token_hash.as_str())
        .await?
        .ok_or(ManagementTokenAuthError::Invalid)?;
    if token_with_user.token.user_id != token_with_user.user.id {
        return Err(ManagementTokenAuthError::Invalid);
    }

    let now = chrono::Utc::now().timestamp().max(0) as u64;
    if !token_with_user.token.is_active
        || token_with_user
            .token
            .expires_at_unix_secs
            .is_some_and(|expires_at| expires_at <= now)
        || !crate::handlers::shared::json_ip_rules_allow(
            token_with_user.token.allowed_ips.as_ref(),
            remote_ip,
        )
    {
        return Err(ManagementTokenAuthError::Invalid);
    }

    let user = source
        .find_current_user(&token_with_user.token.user_id)
        .await?
        .ok_or(ManagementTokenAuthError::Invalid)?;
    if !user.is_active || user.is_deleted || !crate::roles::can_access_admin_console(&user.role) {
        return Err(ManagementTokenAuthError::Invalid);
    }

    let permissions = crate::control::management_token_permission_keys_from_value(
        token_with_user.token.permissions.as_ref(),
    )
    .map_err(|_| ManagementTokenAuthError::Invalid)?
    .unwrap_or_else(crate::control::legacy_full_management_token_permissions);

    Ok(AuthenticatedManagementToken {
        token: token_with_user.token,
        user,
        permissions,
        verified_token_hash: token_hash.clone(),
    })
}

fn extract_unique_management_token_bearer(
    headers: &HeaderMap,
) -> Result<&str, ManagementTokenAuthError> {
    let mut values = headers.get_all(http::header::AUTHORIZATION).iter();
    let value = values.next().ok_or(ManagementTokenAuthError::Missing)?;
    if values.next().is_some() {
        return Err(ManagementTokenAuthError::Invalid);
    }
    let value = value
        .to_str()
        .map_err(|_| ManagementTokenAuthError::Invalid)?;
    let (scheme, token) = value
        .split_once(' ')
        .ok_or(ManagementTokenAuthError::Invalid)?;
    let token = token.trim();
    if !scheme.eq_ignore_ascii_case("bearer")
        || token.is_empty()
        || token.len() > MANAGEMENT_TOKEN_MAX_LEN
        || (!token.starts_with("ae-") && !token.starts_with("ae_"))
        || token.chars().any(char::is_whitespace)
    {
        return Err(ManagementTokenAuthError::Invalid);
    }
    Ok(token)
}

fn hash_management_token(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::{
        extract_unique_management_token_bearer, ManagementTokenAuthError,
        VerifiedManagementTokenHash,
    };
    use axum::http::{header, HeaderMap, HeaderValue};

    #[test]
    fn extracts_one_case_insensitive_bearer_value() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("bEaReR ae-valid-token"),
        );
        assert_eq!(
            extract_unique_management_token_bearer(&headers),
            Ok("ae-valid-token")
        );
    }

    #[test]
    fn rejects_duplicate_or_ambiguous_authorization() {
        let mut headers = HeaderMap::new();
        headers.append(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer ae-first"),
        );
        headers.append(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer ae-second"),
        );
        assert_eq!(
            extract_unique_management_token_bearer(&headers),
            Err(ManagementTokenAuthError::Invalid)
        );

        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer ae-valid extra"),
        );
        assert_eq!(
            extract_unique_management_token_bearer(&headers),
            Err(ManagementTokenAuthError::Invalid)
        );
    }

    #[test]
    fn verified_management_token_hash_debug_is_redacted() {
        let hash = VerifiedManagementTokenHash::new("credential-equivalent-hash".to_string());
        let rendered = format!("{hash:?}");
        assert!(rendered.contains("[REDACTED]"));
        assert!(!rendered.contains("credential-equivalent-hash"));
    }
}
