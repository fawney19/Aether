use crate::core::{current_unix_secs, OAuthError};
use crate::network::{OAuthHttpExecutor, OAuthHttpRequest};
use crate::provider::ProviderOAuthAdapter;
use crate::provider::{
    ProviderOAuthAccount, ProviderOAuthCapabilities, ProviderOAuthImportInput,
    ProviderOAuthRequestAuth, ProviderOAuthTokenSet, ProviderOAuthTransportContext,
};
use async_trait::async_trait;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub const KIRO_PROVIDER_TYPE: &str = "kiro";
const IDC_AMZ_USER_AGENT: &str =
    "aws-sdk-js/3.738.0 ua/2.1 os/other lang/js md/browser#unknown_unknown api/sso-oidc#3.738.0 m/E KiroIDE";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KiroAuthConfig {
    pub auth_method: Option<String>,
    pub refresh_token: Option<String>,
    pub expires_at: Option<u64>,
    pub profile_arn: Option<String>,
    pub region: Option<String>,
    pub auth_region: Option<String>,
    pub api_region: Option<String>,
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub machine_id: Option<String>,
    pub kiro_version: Option<String>,
    pub system_version: Option<String>,
    pub node_version: Option<String>,
    pub access_token: Option<String>,
}

impl KiroAuthConfig {
    pub fn from_json_value(value: &Value) -> Option<Self> {
        let object = value.as_object()?;
        Some(Self {
            auth_method: string_field(
                object,
                &["auth_method", "authMethod", "auth_type", "authType"],
            ),
            refresh_token: string_field(object, &["refresh_token", "refreshToken"]),
            expires_at: u64_field(object.get("expires_at"))
                .or_else(|| u64_field(object.get("expiresAt"))),
            profile_arn: string_field(object, &["profile_arn", "profileArn"]),
            region: string_field(object, &["region"]),
            auth_region: string_field(object, &["auth_region", "authRegion"]),
            api_region: string_field(object, &["api_region", "apiRegion"]),
            client_id: string_field(object, &["client_id", "clientId"]),
            client_secret: string_field(object, &["client_secret", "clientSecret"]),
            machine_id: string_field(object, &["machine_id", "machineId"]),
            kiro_version: string_field(object, &["kiro_version", "kiroVersion"]),
            system_version: string_field(object, &["system_version", "systemVersion"]),
            node_version: string_field(object, &["node_version", "nodeVersion"]),
            access_token: string_field(object, &["access_token", "accessToken"]),
        })
    }

    pub fn from_raw_json(raw: Option<&str>) -> Option<Self> {
        let parsed: Value = serde_json::from_str(raw?.trim()).ok()?;
        Self::from_json_value(&parsed)
    }

    pub fn to_json_value(&self) -> Value {
        let mut object = serde_json::Map::new();
        insert_string(&mut object, "auth_method", self.auth_method.as_deref());
        insert_string(&mut object, "refresh_token", self.refresh_token.as_deref());
        if let Some(expires_at) = self.expires_at {
            object.insert("expires_at".to_string(), json!(expires_at));
        }
        insert_string(&mut object, "profile_arn", self.profile_arn.as_deref());
        insert_string(&mut object, "region", self.region.as_deref());
        insert_string(&mut object, "auth_region", self.auth_region.as_deref());
        insert_string(&mut object, "api_region", self.api_region.as_deref());
        insert_string(&mut object, "client_id", self.client_id.as_deref());
        insert_string(&mut object, "client_secret", self.client_secret.as_deref());
        insert_string(&mut object, "machine_id", self.machine_id.as_deref());
        insert_string(&mut object, "kiro_version", self.kiro_version.as_deref());
        insert_string(
            &mut object,
            "system_version",
            self.system_version.as_deref(),
        );
        insert_string(&mut object, "node_version", self.node_version.as_deref());
        insert_string(&mut object, "access_token", self.access_token.as_deref());
        Value::Object(object)
    }

    pub fn effective_auth_region(&self) -> &str {
        self.auth_region
            .as_deref()
            .or(self.region.as_deref())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("us-east-1")
    }

    pub fn effective_api_region(&self) -> &str {
        self.api_region
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("us-east-1")
    }

    pub fn effective_kiro_version(&self) -> &str {
        self.kiro_version
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("0.3.210")
    }

    pub fn is_idc_auth(&self) -> bool {
        self.auth_method
            .as_deref()
            .map(str::trim)
            .map(str::to_ascii_lowercase)
            .is_some_and(|value| matches!(value.as_str(), "idc" | "external_idp"))
            || (self
                .client_id
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
                && self
                    .client_secret
                    .as_deref()
                    .is_some_and(|value| !value.trim().is_empty()))
    }

    pub fn can_refresh_access_token(&self) -> bool {
        self.refresh_token
            .as_deref()
            .map(str::trim)
            .filter(|value| value.len() >= 100 && !value.contains("..."))
            .is_some()
            && (!self.is_idc_auth()
                || (self
                    .client_id
                    .as_deref()
                    .is_some_and(|value| !value.trim().is_empty())
                    && self
                        .client_secret
                        .as_deref()
                        .is_some_and(|value| !value.trim().is_empty())))
    }
}

#[derive(Debug, Clone, Default)]
pub struct KiroProviderOAuthAdapter {
    social_refresh_base_url: Option<String>,
    idc_refresh_base_url: Option<String>,
}

impl KiroProviderOAuthAdapter {
    pub fn with_refresh_base_urls(
        mut self,
        social_refresh_base_url: Option<String>,
        idc_refresh_base_url: Option<String>,
    ) -> Self {
        self.social_refresh_base_url = social_refresh_base_url;
        self.idc_refresh_base_url = idc_refresh_base_url;
        self
    }

    async fn refresh_auth_config(
        &self,
        executor: &dyn OAuthHttpExecutor,
        ctx: &ProviderOAuthTransportContext,
        auth_config: &KiroAuthConfig,
    ) -> Result<KiroAuthConfig, OAuthError> {
        if auth_config.is_idc_auth() {
            self.refresh_idc_token(executor, ctx, auth_config).await
        } else {
            self.refresh_social_token(executor, ctx, auth_config).await
        }
    }

    async fn refresh_social_token(
        &self,
        executor: &dyn OAuthHttpExecutor,
        ctx: &ProviderOAuthTransportContext,
        auth_config: &KiroAuthConfig,
    ) -> Result<KiroAuthConfig, OAuthError> {
        let url = self
            .social_refresh_base_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|base| format!("{}/refreshToken", base.trim_end_matches('/')))
            .unwrap_or_else(|| {
                format!(
                    "https://prod.{}.auth.desktop.kiro.dev/refreshToken",
                    auth_config.effective_auth_region()
                )
            });
        let machine_id = generate_kiro_machine_id(auth_config, None)
            .ok_or_else(|| OAuthError::invalid_request("missing machine_id seed"))?;
        let response = executor
            .execute(OAuthHttpRequest {
                request_id: "provider-oauth:kiro-social-refresh".to_string(),
                method: reqwest::Method::POST,
                url,
                headers: BTreeMap::from([
                    (
                        "user-agent".to_string(),
                        format!(
                            "KiroIDE-{}-{machine_id}",
                            auth_config.effective_kiro_version()
                        ),
                    ),
                    (
                        "accept".to_string(),
                        "application/json, text/plain, */*".to_string(),
                    ),
                    ("content-type".to_string(), "application/json".to_string()),
                    ("connection".to_string(), "close".to_string()),
                ]),
                content_type: Some("application/json".to_string()),
                json_body: Some(json!({
                    "refreshToken": auth_config.refresh_token.as_deref().unwrap_or_default()
                })),
                body_bytes: None,
                network: ctx.network.clone(),
            })
            .await?;
        if !(200..300).contains(&response.status_code) {
            return Err(OAuthError::HttpStatus {
                status_code: response.status_code,
                body_excerpt: response.body_text.chars().take(500).collect(),
            });
        }
        let payload = response
            .json_body
            .or_else(|| serde_json::from_str::<Value>(&response.body_text).ok())
            .ok_or_else(|| OAuthError::invalid_response("kiro refresh response is not json"))?;
        let access_token = payload
            .get("accessToken")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| OAuthError::invalid_response("kiro refresh missing accessToken"))?;
        let mut refreshed = auth_config.clone();
        refreshed.access_token = Some(access_token.to_string());
        refreshed.expires_at = Some(resolve_expires_at(&payload));
        if refreshed
            .machine_id
            .as_deref()
            .is_none_or(|value| value.trim().is_empty())
        {
            refreshed.machine_id = Some(machine_id);
        }
        if let Some(refresh_token) = payload.get("refreshToken").and_then(Value::as_str) {
            refreshed.refresh_token = Some(refresh_token.trim().to_string());
        }
        if let Some(profile_arn) = payload.get("profileArn").and_then(Value::as_str) {
            refreshed.profile_arn = Some(profile_arn.trim().to_string());
        }
        Ok(refreshed)
    }

    async fn refresh_idc_token(
        &self,
        executor: &dyn OAuthHttpExecutor,
        ctx: &ProviderOAuthTransportContext,
        auth_config: &KiroAuthConfig,
    ) -> Result<KiroAuthConfig, OAuthError> {
        let url = self
            .idc_refresh_base_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|base| format!("{}/token", base.trim_end_matches('/')))
            .unwrap_or_else(|| {
                format!(
                    "https://oidc.{}.amazonaws.com/token",
                    auth_config.effective_auth_region()
                )
            });
        let response = executor
            .execute(OAuthHttpRequest {
                request_id: "provider-oauth:kiro-idc-refresh".to_string(),
                method: reqwest::Method::POST,
                url,
                headers: BTreeMap::from([
                    ("content-type".to_string(), "application/json".to_string()),
                    (
                        "x-amz-user-agent".to_string(),
                        IDC_AMZ_USER_AGENT.to_string(),
                    ),
                    ("user-agent".to_string(), "node".to_string()),
                    ("accept".to_string(), "*/*".to_string()),
                ]),
                content_type: Some("application/json".to_string()),
                json_body: Some(json!({
                    "clientId": auth_config.client_id.as_deref().unwrap_or_default(),
                    "clientSecret": auth_config.client_secret.as_deref().unwrap_or_default(),
                    "refreshToken": auth_config.refresh_token.as_deref().unwrap_or_default(),
                    "grantType": "refresh_token",
                })),
                body_bytes: None,
                network: ctx.network.clone(),
            })
            .await?;
        if !(200..300).contains(&response.status_code) {
            return Err(OAuthError::HttpStatus {
                status_code: response.status_code,
                body_excerpt: response.body_text.chars().take(500).collect(),
            });
        }
        let payload = response
            .json_body
            .or_else(|| serde_json::from_str::<Value>(&response.body_text).ok())
            .ok_or_else(|| OAuthError::invalid_response("kiro idc response is not json"))?;
        let access_token = payload
            .get("accessToken")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| OAuthError::invalid_response("kiro idc missing accessToken"))?;
        let mut refreshed = auth_config.clone();
        refreshed.access_token = Some(access_token.to_string());
        refreshed.expires_at = Some(resolve_expires_at(&payload));
        if refreshed
            .machine_id
            .as_deref()
            .is_none_or(|value| value.trim().is_empty())
        {
            refreshed.machine_id = generate_kiro_machine_id(auth_config, None);
        }
        if let Some(refresh_token) = payload.get("refreshToken").and_then(Value::as_str) {
            refreshed.refresh_token = Some(refresh_token.trim().to_string());
        }
        Ok(refreshed)
    }
}

#[async_trait]
impl ProviderOAuthAdapter for KiroProviderOAuthAdapter {
    fn provider_type(&self) -> &'static str {
        KIRO_PROVIDER_TYPE
    }

    fn capabilities(&self) -> ProviderOAuthCapabilities {
        ProviderOAuthCapabilities {
            supports_authorization_code: false,
            supports_refresh_token_import: true,
            supports_batch_import: true,
            supports_device_flow: true,
            supports_account_probe: true,
            rotates_refresh_token: true,
        }
    }

    async fn import_credentials(
        &self,
        executor: &dyn OAuthHttpExecutor,
        ctx: &ProviderOAuthTransportContext,
        input: ProviderOAuthImportInput,
    ) -> Result<ProviderOAuthTokenSet, OAuthError> {
        let auth_config = input
            .raw_credentials
            .as_ref()
            .and_then(KiroAuthConfig::from_json_value)
            .or_else(|| {
                input
                    .refresh_token
                    .as_ref()
                    .map(|refresh_token| KiroAuthConfig {
                        auth_method: None,
                        refresh_token: Some(refresh_token.clone()),
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
                    })
            })
            .ok_or_else(|| OAuthError::invalid_request("kiro credentials are required"))?;
        let refreshed = self
            .refresh_auth_config(executor, ctx, &auth_config)
            .await?;
        token_set_from_kiro_auth_config(refreshed)
    }

    async fn refresh(
        &self,
        executor: &dyn OAuthHttpExecutor,
        ctx: &ProviderOAuthTransportContext,
        account: &ProviderOAuthAccount,
    ) -> Result<ProviderOAuthTokenSet, OAuthError> {
        let auth_config = KiroAuthConfig::from_json_value(&account.auth_config)
            .ok_or_else(|| OAuthError::invalid_request("invalid kiro auth_config"))?;
        let refreshed = self
            .refresh_auth_config(executor, ctx, &auth_config)
            .await?;
        token_set_from_kiro_auth_config(refreshed)
    }

    fn resolve_request_auth(
        &self,
        account: &ProviderOAuthAccount,
    ) -> Result<ProviderOAuthRequestAuth, OAuthError> {
        let auth_config = KiroAuthConfig::from_json_value(&account.auth_config)
            .ok_or_else(|| OAuthError::invalid_request("invalid kiro auth_config"))?;
        let machine_id = generate_kiro_machine_id(&auth_config, Some(&account.access_token))
            .ok_or_else(|| OAuthError::invalid_request("missing kiro machine_id"))?;
        Ok(ProviderOAuthRequestAuth::Kiro {
            name: "authorization".to_string(),
            value: format!("Bearer {}", account.access_token.trim()),
            auth_config: account.auth_config.clone(),
            machine_id,
        })
    }

    fn account_fingerprint(&self, account: &ProviderOAuthAccount) -> Option<String> {
        account
            .auth_config
            .get("refresh_token")
            .and_then(Value::as_str)
            .map(secret_fingerprint)
    }
}

pub fn generate_kiro_machine_id(
    auth_config: &KiroAuthConfig,
    fallback_secret: Option<&str>,
) -> Option<String> {
    if let Some(machine_id) = auth_config
        .machine_id
        .as_deref()
        .and_then(normalize_machine_id)
    {
        return Some(machine_id);
    }
    let seed = auth_config
        .refresh_token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| {
            fallback_secret
                .map(str::trim)
                .filter(|value| !value.is_empty())
        })?;
    let mut hasher = Sha256::new();
    hasher.update(b"KotlinNativeAPI/");
    hasher.update(seed.as_bytes());
    Some(format!("{:x}", hasher.finalize()))
}

fn token_set_from_kiro_auth_config(
    auth_config: KiroAuthConfig,
) -> Result<ProviderOAuthTokenSet, OAuthError> {
    let access_token = auth_config
        .access_token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| OAuthError::invalid_response("kiro auth_config missing access_token"))?
        .to_string();
    let token_set = crate::core::OAuthTokenSet {
        access_token,
        refresh_token: auth_config.refresh_token.clone(),
        token_type: Some("Bearer".to_string()),
        scope: None,
        expires_at_unix_secs: auth_config.expires_at,
        raw_payload: Some(auth_config.to_json_value()),
    };
    let mut value = auth_config.to_json_value();
    if let Some(object) = value.as_object_mut() {
        object.insert("provider_type".to_string(), json!(KIRO_PROVIDER_TYPE));
    }
    Ok(ProviderOAuthTokenSet {
        token_set,
        auth_config: value,
    })
}

fn resolve_expires_at(payload: &Value) -> u64 {
    let expires_in = payload
        .get("expiresIn")
        .or_else(|| payload.get("expires_in"))
        .and_then(|value| {
            value
                .as_u64()
                .or_else(|| value.as_str()?.parse::<u64>().ok())
        })
        .unwrap_or(3600);
    current_unix_secs().saturating_add(expires_in)
}

fn normalize_machine_id(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.len() == 64 && raw.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Some(raw.to_ascii_lowercase());
    }
    if raw.len() == 36
        && raw.chars().enumerate().all(|(idx, ch)| match idx {
            8 | 13 | 18 | 23 => ch == '-',
            _ => ch.is_ascii_hexdigit(),
        })
    {
        let normalized = raw.replace('-', "").to_ascii_lowercase();
        return Some(format!("{normalized}{normalized}"));
    }
    None
}

fn string_field(object: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| object.get(*key))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn u64_field(value: Option<&Value>) -> Option<u64> {
    match value? {
        Value::Number(number) => number.as_u64(),
        Value::String(value) => value.trim().parse::<u64>().ok(),
        _ => None,
    }
}

fn insert_string(object: &mut serde_json::Map<String, Value>, key: &str, value: Option<&str>) {
    if let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) {
        object.insert(key.to_string(), Value::String(value.to_string()));
    }
}

fn secret_fingerprint(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    digest
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{generate_kiro_machine_id, KiroAuthConfig};

    #[test]
    fn normalizes_kiro_uuid_machine_id() {
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
            machine_id: Some("123e4567-e89b-12d3-a456-426614174000".to_string()),
            kiro_version: None,
            system_version: None,
            node_version: None,
            access_token: None,
        };
        assert_eq!(
            generate_kiro_machine_id(&auth_config, None).as_deref(),
            Some("123e4567e89b12d3a456426614174000123e4567e89b12d3a456426614174000")
        );
    }
}
