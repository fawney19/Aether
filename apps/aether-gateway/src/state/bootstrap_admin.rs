use crate::{AppState, GatewayError};
use aether_data::repository::wallet::WalletLookupKey;
use regex::Regex;
use tracing::{info, warn};

const BOOTSTRAP_ADMIN_EMAIL_ENVS: &[&str] = &["ADMIN_EMAIL"];
const BOOTSTRAP_ADMIN_USERNAME_ENVS: &[&str] = &["ADMIN_USERNAME"];
const BOOTSTRAP_ADMIN_PASSWORD_ENVS: &[&str] = &["ADMIN_PASSWORD"];

#[derive(Clone, PartialEq, Eq)]
struct BootstrapAdminConfig {
    email: Option<String>,
    username: String,
    password: String,
}

impl std::fmt::Debug for BootstrapAdminConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BootstrapAdminConfig")
            .field("email", &self.email)
            .field("username", &self.username)
            .field("password", &"[REDACTED]")
            .finish()
    }
}

impl BootstrapAdminConfig {
    fn from_env() -> Result<Option<Self>, GatewayError> {
        Self::from_lookup(|key| {
            std::env::var(key)
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        })
    }

    fn from_lookup<F>(lookup: F) -> Result<Option<Self>, GatewayError>
    where
        F: Fn(&str) -> Option<String>,
    {
        let email = first_present_env(&lookup, BOOTSTRAP_ADMIN_EMAIL_ENVS);
        let username = first_present_env(&lookup, BOOTSTRAP_ADMIN_USERNAME_ENVS);
        let password = first_present_env(&lookup, BOOTSTRAP_ADMIN_PASSWORD_ENVS);

        if email.is_none() && username.is_none() && password.is_none() {
            return Ok(None);
        }

        let username = username.ok_or_else(|| {
            GatewayError::Internal(format!(
                "bootstrap admin env is partially configured; set {}",
                BOOTSTRAP_ADMIN_USERNAME_ENVS.join(" or ")
            ))
        })?;
        let password = password.ok_or_else(|| {
            GatewayError::Internal(format!(
                "bootstrap admin env is partially configured; set {}",
                BOOTSTRAP_ADMIN_PASSWORD_ENVS.join(" or ")
            ))
        })?;

        Ok(Some(Self {
            email,
            username,
            password,
        }))
    }
}

fn first_present_env<F>(lookup: &F, keys: &[&str]) -> Option<String>
where
    F: Fn(&str) -> Option<String>,
{
    keys.iter().find_map(|key| lookup(key))
}

fn normalize_bootstrap_admin_email(value: Option<&str>) -> Result<Option<String>, GatewayError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let normalized = value.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return Ok(None);
    }
    let pattern = Regex::new(r"^[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}$")
        .expect("bootstrap admin email regex should compile");
    if !pattern.is_match(&normalized) {
        return Err(GatewayError::Internal(
            "bootstrap admin email format is invalid".to_string(),
        ));
    }
    Ok(Some(normalized))
}

fn normalize_bootstrap_admin_username(value: &str) -> Result<String, GatewayError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(GatewayError::Internal(
            "bootstrap admin username cannot be empty".to_string(),
        ));
    }
    if value.len() < 3 {
        return Err(GatewayError::Internal(
            "bootstrap admin username must be at least 3 characters".to_string(),
        ));
    }
    if value.len() > 30 {
        return Err(GatewayError::Internal(
            "bootstrap admin username must not exceed 30 characters".to_string(),
        ));
    }
    let pattern =
        Regex::new(r"^[a-zA-Z0-9_.-]+$").expect("bootstrap admin username regex should compile");
    if !pattern.is_match(value) {
        return Err(GatewayError::Internal(
            "bootstrap admin username may only contain letters, numbers, underscores, hyphens, and dots".to_string(),
        ));
    }
    Ok(value.to_string())
}

fn validate_bootstrap_admin_password(password: &str, policy: &str) -> Result<(), GatewayError> {
    if password.is_empty() {
        return Err(GatewayError::Internal(
            "bootstrap admin password cannot be empty".to_string(),
        ));
    }
    if password.as_bytes().len() > 72 {
        return Err(GatewayError::Internal(
            "bootstrap admin password must not exceed 72 bytes".to_string(),
        ));
    }
    let min_len = if matches!(policy, "medium" | "strong") {
        8
    } else {
        6
    };
    if password.chars().count() < min_len {
        return Err(GatewayError::Internal(format!(
            "bootstrap admin password must be at least {min_len} characters"
        )));
    }
    if policy == "medium" {
        if !password.chars().any(|ch| ch.is_ascii_alphabetic()) {
            return Err(GatewayError::Internal(
                "bootstrap admin password must contain at least one letter".to_string(),
            ));
        }
        if !password.chars().any(|ch| ch.is_ascii_digit()) {
            return Err(GatewayError::Internal(
                "bootstrap admin password must contain at least one digit".to_string(),
            ));
        }
    } else if policy == "strong" {
        if !password.chars().any(|ch| ch.is_ascii_uppercase()) {
            return Err(GatewayError::Internal(
                "bootstrap admin password must contain at least one uppercase letter".to_string(),
            ));
        }
        if !password.chars().any(|ch| ch.is_ascii_lowercase()) {
            return Err(GatewayError::Internal(
                "bootstrap admin password must contain at least one lowercase letter".to_string(),
            ));
        }
        if !password.chars().any(|ch| ch.is_ascii_digit()) {
            return Err(GatewayError::Internal(
                "bootstrap admin password must contain at least one digit".to_string(),
            ));
        }
        if !password.chars().any(|ch| !ch.is_ascii_alphanumeric()) {
            return Err(GatewayError::Internal(
                "bootstrap admin password must contain at least one special character".to_string(),
            ));
        }
    }
    Ok(())
}

async fn resolve_bootstrap_admin_password_policy(state: &AppState) -> Result<String, GatewayError> {
    let configured = state
        .read_system_config_json_value("password_policy_level")
        .await?;
    Ok(match configured.as_ref() {
        Some(serde_json::Value::String(value))
            if matches!(value.trim(), "weak" | "medium" | "strong") =>
        {
            value.trim().to_string()
        }
        _ => "weak".to_string(),
    })
}

async fn find_existing_bootstrap_user(
    state: &AppState,
    username: &str,
    email: Option<&str>,
) -> Result<Option<aether_data::repository::users::StoredUserAuthRecord>, GatewayError> {
    let by_username = state.find_user_auth_by_identifier(username).await?;
    let by_email = match email {
        Some(email) => state.find_user_auth_by_identifier(email).await?,
        None => None,
    };

    match (by_username, by_email) {
        (Some(username_user), Some(email_user)) if username_user.id != email_user.id => {
            Err(GatewayError::Internal(
                "bootstrap admin username and email point to different existing users".to_string(),
            ))
        }
        (Some(user), _) | (_, Some(user)) => Ok(Some(user)),
        (None, None) => Ok(None),
    }
}

async fn ensure_bootstrap_admin_wallet(
    state: &AppState,
    user_id: &str,
) -> Result<(), GatewayError> {
    if state
        .find_wallet(WalletLookupKey::UserId(user_id))
        .await?
        .is_some()
    {
        return Ok(());
    }

    let created = state
        .initialize_auth_user_wallet(user_id, 0.0, true)
        .await?;
    if created.is_none() {
        return Err(GatewayError::Internal(
            "bootstrap admin wallet storage is unavailable".to_string(),
        ));
    }
    Ok(())
}

impl AppState {
    pub async fn bootstrap_admin_from_env(&self) -> Result<(), std::io::Error> {
        let Some(config) = BootstrapAdminConfig::from_env()
            .map_err(|err| std::io::Error::other(format!("{err:?}")))?
        else {
            return Ok(());
        };
        self.bootstrap_admin_from_config(config)
            .await
            .map_err(|err| std::io::Error::other(format!("{err:?}")))
    }

    async fn bootstrap_admin_from_config(
        &self,
        config: BootstrapAdminConfig,
    ) -> Result<(), GatewayError> {
        if !self.has_auth_user_write_capability() || !self.has_auth_wallet_write_capability() {
            return Err(GatewayError::Internal(
                "bootstrap admin requires SQL-backed user and wallet write capability".to_string(),
            ));
        }

        let email = normalize_bootstrap_admin_email(config.email.as_deref())?;
        let username = normalize_bootstrap_admin_username(&config.username)?;
        let password_policy = resolve_bootstrap_admin_password_policy(self).await?;
        validate_bootstrap_admin_password(&config.password, &password_policy)?;

        if let Some(existing_user) =
            find_existing_bootstrap_user(self, &username, email.as_deref()).await?
        {
            if !existing_user.role.eq_ignore_ascii_case("admin")
                || !existing_user.auth_source.eq_ignore_ascii_case("local")
                || !existing_user.is_active
                || existing_user.is_deleted
            {
                return Err(GatewayError::Internal(format!(
                    "bootstrap admin target already exists but is not an active local admin: {}",
                    existing_user.username
                )));
            }
            ensure_bootstrap_admin_wallet(self, &existing_user.id).await?;
            info!(
                event_name = "bootstrap_admin_ready",
                log_type = "ops",
                user_id = %existing_user.id,
                username = %existing_user.username,
                email = existing_user.email.as_deref().unwrap_or("-"),
                status = "existing",
                "bootstrap admin already exists"
            );
            return Ok(());
        }

        if self.count_active_admin_users().await? > 0 {
            info!(
                event_name = "bootstrap_admin_skipped",
                log_type = "ops",
                username = %username,
                email = email.as_deref().unwrap_or("-"),
                status = "active_admin_exists",
                "bootstrap admin skipped because another active admin already exists"
            );
            return Ok(());
        }

        let password_hash =
            bcrypt::hash(&config.password, bcrypt::DEFAULT_COST).map_err(|err| {
                GatewayError::Internal(format!("bootstrap admin password hash failed: {err}"))
            })?;

        match self
            .create_local_auth_user_with_settings(
                email.clone(),
                true,
                username.clone(),
                password_hash,
                "admin".to_string(),
                None,
                None,
                None,
                None,
            )
            .await
        {
            Ok(Some(user)) => {
                ensure_bootstrap_admin_wallet(self, &user.id).await?;
                info!(
                    event_name = "bootstrap_admin_created",
                    log_type = "ops",
                    user_id = %user.id,
                    username = %user.username,
                    email = user.email.as_deref().unwrap_or("-"),
                    status = "created",
                    "bootstrap admin created from environment"
                );
                Ok(())
            }
            Ok(None) => Err(GatewayError::Internal(
                "bootstrap admin user storage is unavailable".to_string(),
            )),
            Err(err) => {
                if let Some(user) =
                    find_existing_bootstrap_user(self, &username, email.as_deref()).await?
                {
                    ensure_bootstrap_admin_wallet(self, &user.id).await?;
                    warn!(
                        event_name = "bootstrap_admin_race_resolved",
                        log_type = "ops",
                        username = %user.username,
                        user_id = %user.id,
                        error = ?err,
                        "bootstrap admin creation raced with another writer; continuing with existing admin"
                    );
                    return Ok(());
                }
                Err(err)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::BootstrapAdminConfig;
    use crate::AppState;
    use aether_data::repository::wallet::WalletLookupKey;

    fn bootstrap_config() -> BootstrapAdminConfig {
        BootstrapAdminConfig {
            email: Some("admin@example.com".to_string()),
            username: "admin".to_string(),
            password: "Secret123!".to_string(),
        }
    }

    fn sample_local_admin(
        user_id: &str,
        username: &str,
        email: Option<&str>,
    ) -> aether_data::repository::users::StoredUserAuthRecord {
        aether_data::repository::users::StoredUserAuthRecord::new(
            user_id.to_string(),
            email.map(|value| value.to_string()),
            true,
            username.to_string(),
            Some(
                bcrypt::hash("Secret123!", bcrypt::DEFAULT_COST)
                    .expect("sample admin password hash should build"),
            ),
            "admin".to_string(),
            "local".to_string(),
            None,
            None,
            None,
            true,
            false,
            Some(chrono::Utc::now()),
            None,
        )
        .expect("sample admin should build")
    }

    #[tokio::test]
    async fn bootstrap_admin_creates_missing_admin_and_wallet() {
        let state = AppState::new().expect("state should build");

        state
            .bootstrap_admin_from_config(bootstrap_config())
            .await
            .expect("bootstrap should succeed");

        let user = state
            .find_user_auth_by_identifier("admin")
            .await
            .expect("lookup should succeed")
            .expect("admin should exist");
        assert_eq!(user.role, "admin");
        assert_eq!(user.auth_source, "local");
        assert_eq!(user.email.as_deref(), Some("admin@example.com"));
        assert!(bcrypt::verify(
            "Secret123!",
            user.password_hash
                .as_deref()
                .expect("password hash should exist")
        )
        .expect("password hash should verify"));

        let wallet = state
            .find_wallet(WalletLookupKey::UserId(&user.id))
            .await
            .expect("wallet lookup should succeed")
            .expect("wallet should exist");
        assert_eq!(wallet.limit_mode, "unlimited");
    }

    #[tokio::test]
    async fn bootstrap_admin_repairs_missing_wallet_for_existing_matching_admin() {
        let existing = sample_local_admin("admin-user-1", "admin", Some("admin@example.com"));
        let state = AppState::new()
            .expect("state should build")
            .with_auth_users_for_tests([existing.clone()]);

        state
            .bootstrap_admin_from_config(bootstrap_config())
            .await
            .expect("bootstrap should succeed");

        let wallet = state
            .find_wallet(WalletLookupKey::UserId(&existing.id))
            .await
            .expect("wallet lookup should succeed")
            .expect("wallet should exist");
        assert_eq!(wallet.limit_mode, "unlimited");
    }

    #[tokio::test]
    async fn bootstrap_admin_rejects_existing_non_admin_collision() {
        let existing = aether_data::repository::users::StoredUserAuthRecord::new(
            "user-1".to_string(),
            Some("admin@example.com".to_string()),
            true,
            "admin".to_string(),
            Some(
                bcrypt::hash("Secret123!", bcrypt::DEFAULT_COST)
                    .expect("sample password hash should build"),
            ),
            "user".to_string(),
            "local".to_string(),
            None,
            None,
            None,
            true,
            false,
            Some(chrono::Utc::now()),
            None,
        )
        .expect("sample user should build");
        let state = AppState::new()
            .expect("state should build")
            .with_auth_users_for_tests([existing]);

        let err = state
            .bootstrap_admin_from_config(bootstrap_config())
            .await
            .expect_err("bootstrap should fail");
        let detail = format!("{err:?}");
        assert!(detail.contains("not an active local admin"));
    }

    #[test]
    fn bootstrap_admin_config_reads_admin_env_names() {
        let vars = std::collections::BTreeMap::from([
            ("ADMIN_EMAIL".to_string(), "admin@example.com".to_string()),
            ("ADMIN_USERNAME".to_string(), "admin".to_string()),
            ("ADMIN_PASSWORD".to_string(), "Secret123!".to_string()),
        ]);

        let config = BootstrapAdminConfig::from_lookup(|key| vars.get(key).cloned())
            .expect("config parsing should succeed")
            .expect("config should exist");
        assert_eq!(
            config,
            BootstrapAdminConfig {
                email: Some("admin@example.com".to_string()),
                username: "admin".to_string(),
                password: "Secret123!".to_string(),
            }
        );
    }

    #[test]
    fn bootstrap_admin_config_debug_output_redacts_password() {
        let config = bootstrap_config();
        let debug = format!("{config:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("Secret123!"));
    }

    #[test]
    fn bootstrap_admin_config_rejects_partial_env() {
        let vars =
            std::collections::BTreeMap::from([("ADMIN_USERNAME".to_string(), "admin".to_string())]);

        let err = BootstrapAdminConfig::from_lookup(|key| vars.get(key).cloned())
            .expect_err("partial config should fail");
        let detail = format!("{err:?}");
        assert!(detail.contains("bootstrap admin env is partially configured"));
    }
}
