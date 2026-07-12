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
pub const DEFAULT_REGION: &str = "us-east-1";
pub const DEFAULT_KIRO_VERSION: &str = "0.3.210";
pub const DEFAULT_NODE_VERSION: &str = "22.21.1";
pub const DEFAULT_SYSTEM_VERSION: &str = "other#unknown";
const IDC_AMZ_USER_AGENT: &str =
    "aws-sdk-js/3.738.0 ua/2.1 os/other lang/js md/browser#unknown_unknown api/sso-oidc#3.738.0 m/E KiroIDE";
const KIRO_PROFILE_DISCOVERY_SDK_VERSION: &str = "1.0.0";

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
            )
            .map(|value| normalize_kiro_auth_method(&value)),
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
            .unwrap_or(DEFAULT_REGION)
    }

    pub fn effective_api_region(&self) -> &str {
        self.api_region
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(DEFAULT_REGION)
    }

    pub fn effective_kiro_version(&self) -> &str {
        self.kiro_version
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(DEFAULT_KIRO_VERSION)
    }

    pub fn effective_system_version(&self) -> &str {
        self.system_version
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(DEFAULT_SYSTEM_VERSION)
    }

    pub fn effective_node_version(&self) -> &str {
        self.node_version
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(DEFAULT_NODE_VERSION)
    }

    pub fn cached_access_token(&self) -> Option<&str> {
        self.access_token
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
    }

    pub fn cached_access_token_requires_refresh(&self, skew_seconds: u64) -> bool {
        let Some(expires_at) = self.expires_at else {
            return self.can_refresh_access_token();
        };
        let now = current_unix_secs();
        now >= expires_at.saturating_sub(skew_seconds)
    }

    pub fn is_idc_auth(&self) -> bool {
        let explicit_method = self
            .auth_method
            .as_deref()
            .map(normalize_kiro_auth_method)
            .unwrap_or_else(|| "social".to_string());
        if explicit_method != "social" {
            return matches!(explicit_method.as_str(), "idc" | "external_idp");
        }
        self.client_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .is_some()
            && self
                .client_secret
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .is_some()
    }

    pub fn uses_external_idp_token_type(&self) -> bool {
        self.auth_method
            .as_deref()
            .map(normalize_kiro_auth_method)
            .as_deref()
            == Some("external_idp")
    }

    pub fn profile_arn_for_payload(&self) -> Option<&str> {
        self.profile_arn
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
    }

    pub fn profile_arn_for_mcp(&self) -> Option<&str> {
        self.profile_arn
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
    }

    pub fn can_refresh_access_token(&self) -> bool {
        let refresh_token = self
            .refresh_token
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .filter(|value| value.len() >= 100 && !value.contains("..."));
        if refresh_token.is_none() {
            return false;
        }
        if !self.is_idc_auth() {
            return true;
        }
        self.client_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .is_some()
            && self
                .client_secret
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .is_some()
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

    pub async fn refresh_auth_config(
        &self,
        executor: &dyn OAuthHttpExecutor,
        ctx: &ProviderOAuthTransportContext,
        auth_config: &KiroAuthConfig,
    ) -> Result<KiroAuthConfig, OAuthError> {
        let refreshed = if auth_config.is_idc_auth() {
            self.refresh_idc_token(executor, ctx, auth_config).await
        } else {
            self.refresh_social_token(executor, ctx, auth_config).await
        }?;
        self.with_discovered_profile_arn(executor, ctx, refreshed)
            .await
    }

    async fn with_discovered_profile_arn(
        &self,
        executor: &dyn OAuthHttpExecutor,
        ctx: &ProviderOAuthTransportContext,
        mut auth_config: KiroAuthConfig,
    ) -> Result<KiroAuthConfig, OAuthError> {
        if auth_config.profile_arn_for_payload().is_some() {
            return Ok(auth_config);
        }
        if let Some(profile_arn) = discover_kiro_profile_arn(executor, ctx, &auth_config).await? {
            auth_config.profile_arn = Some(profile_arn);
        }
        Ok(auth_config)
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

async fn discover_kiro_profile_arn(
    executor: &dyn OAuthHttpExecutor,
    ctx: &ProviderOAuthTransportContext,
    auth_config: &KiroAuthConfig,
) -> Result<Option<String>, OAuthError> {
    let token = auth_config
        .cached_access_token()
        .ok_or_else(|| OAuthError::invalid_response("kiro profile discovery missing access_token"))?
        .to_string();
    let machine_id = generate_kiro_machine_id(auth_config, Some(token.as_str()))
        .ok_or_else(|| OAuthError::invalid_request("missing machine_id seed"))?;

    for region in profile_discovery_regions(auth_config) {
        if let Some(profile_arn) = discover_kiro_profile_arn_in_region(
            executor,
            ctx,
            auth_config,
            token.as_str(),
            machine_id.as_str(),
            region.as_str(),
        )
        .await?
        {
            return Ok(Some(profile_arn));
        }
    }
    Ok(None)
}

async fn discover_kiro_profile_arn_in_region(
    executor: &dyn OAuthHttpExecutor,
    ctx: &ProviderOAuthTransportContext,
    auth_config: &KiroAuthConfig,
    access_token: &str,
    machine_id: &str,
    region: &str,
) -> Result<Option<String>, OAuthError> {
    let mut next_token = None::<String>;
    for _ in 0..4 {
        let mut body = serde_json::Map::new();
        if let Some(token) = next_token.as_deref() {
            body.insert("nextToken".to_string(), Value::String(token.to_string()));
        }
        let response = executor
            .execute(OAuthHttpRequest {
                request_id: "provider-oauth:kiro-profile-discovery".to_string(),
                method: reqwest::Method::POST,
                url: format!(
                    "https://{}/ListAvailableProfiles",
                    kiro_runtime_host(region)
                ),
                headers: build_kiro_profile_discovery_headers(
                    auth_config,
                    access_token,
                    machine_id,
                    region,
                ),
                content_type: Some("application/json".to_string()),
                json_body: Some(Value::Object(body)),
                body_bytes: None,
                network: ctx.network.clone(),
            })
            .await?;
        if !(200..300).contains(&response.status_code) {
            return Ok(None);
        }
        let payload = response
            .json_body
            .or_else(|| serde_json::from_str::<Value>(&response.body_text).ok())
            .ok_or_else(|| {
                OAuthError::invalid_response("kiro ListAvailableProfiles response is not json")
            })?;
        if let Some(profile_arn) = first_kiro_profile_arn(&payload) {
            return Ok(Some(profile_arn));
        }
        next_token = payload
            .get("nextToken")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);
        if next_token.is_none() {
            return Ok(None);
        }
    }
    Ok(None)
}

fn build_kiro_profile_discovery_headers(
    auth_config: &KiroAuthConfig,
    access_token: &str,
    machine_id: &str,
    region: &str,
) -> BTreeMap<String, String> {
    let kiro_version = auth_config.effective_kiro_version();
    let system_version = auth_config.effective_system_version();
    let node_version = auth_config.effective_node_version();
    let ide_tag = build_kiro_ide_tag(kiro_version, machine_id);
    BTreeMap::from([
        ("accept".to_string(), "application/json".to_string()),
        ("amz-sdk-invocation-id".to_string(), uuid::Uuid::new_v4().to_string()),
        ("amz-sdk-request".to_string(), "attempt=1; max=1".to_string()),
        ("authorization".to_string(), format!("Bearer {}", access_token.trim())),
        ("connection".to_string(), "close".to_string()),
        ("content-type".to_string(), "application/json".to_string()),
        ("host".to_string(), kiro_runtime_host(region)),
        (
            "user-agent".to_string(),
            format!(
                "aws-sdk-js/{KIRO_PROFILE_DISCOVERY_SDK_VERSION} ua/2.1 os/{system_version} lang/js md/nodejs#{node_version} api/codewhispererruntime#1.0.0 m/N,E {ide_tag}"
            ),
        ),
        (
            "x-amz-user-agent".to_string(),
            format!("aws-sdk-js/{KIRO_PROFILE_DISCOVERY_SDK_VERSION} {ide_tag}"),
        ),
    ])
}

fn build_kiro_ide_tag(kiro_version: &str, machine_id: &str) -> String {
    if machine_id.trim().is_empty() {
        format!("KiroIDE-{kiro_version}")
    } else {
        format!("KiroIDE-{kiro_version}-{machine_id}")
    }
}

fn profile_discovery_regions(auth_config: &KiroAuthConfig) -> Vec<String> {
    let mut regions = Vec::new();
    push_unique_region(&mut regions, auth_config.effective_api_region());
    push_unique_region(&mut regions, auth_config.effective_auth_region());
    if auth_config.is_idc_auth() {
        push_unique_region(&mut regions, DEFAULT_REGION);
        push_unique_region(&mut regions, "eu-central-1");
    }
    regions
}

fn push_unique_region(regions: &mut Vec<String>, region: &str) {
    let region = region.trim();
    if region.is_empty() || regions.iter().any(|value| value == region) {
        return;
    }
    regions.push(region.to_string());
}

fn kiro_runtime_host(region: &str) -> String {
    match region {
        "us-gov-east-1" | "us-gov-west-1" => format!("q-fips.{region}.amazonaws.com"),
        "us-iso-east-1" => "q.us-iso-east-1.c2s.ic.gov".to_string(),
        "us-isob-east-1" => "q.us-isob-east-1.sc2s.sgov.gov".to_string(),
        "us-isof-south-1" => "q.us-isof-south-1.csp.hci.ic.gov".to_string(),
        "us-isof-east-1" => "q.us-isof-east-1.csp.hci.ic.gov".to_string(),
        _ => format!("q.{region}.amazonaws.com"),
    }
}

fn first_kiro_profile_arn(payload: &Value) -> Option<String> {
    payload
        .get("profiles")
        .or_else(|| payload.get("data"))?
        .as_array()?
        .iter()
        .find_map(|profile| {
            profile
                .get("arn")
                .or_else(|| profile.get("profileArn"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
        })
}

pub fn generate_kiro_machine_id(
    auth_config: &KiroAuthConfig,
    fallback_secret: Option<&str>,
) -> Option<String> {
    if let Some(machine_id) = auth_config
        .machine_id
        .as_deref()
        .and_then(normalize_kiro_machine_id)
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

pub fn normalize_kiro_machine_id(raw: &str) -> Option<String> {
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

fn normalize_kiro_auth_method(raw: &str) -> String {
    let value = raw.trim().to_ascii_lowercase();
    match value.as_str() {
        "" => "social".to_string(),
        "builder-id"
        | "builder_id"
        | "builderid"
        | "device"
        | "device-auth"
        | "device_authorization"
        | "iam"
        | "identity-center"
        | "identity_center"
        | "identitycenter"
        | "idc" => "idc".to_string(),
        "external-idp" | "external_idp" | "externalidp" => "external_idp".to_string(),
        _ => value,
    }
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
    use super::{
        generate_kiro_machine_id, KiroAuthConfig, KiroProviderOAuthAdapter, IDC_AMZ_USER_AGENT,
    };
    use crate::network::{OAuthHttpExecutor, OAuthHttpRequest, OAuthHttpResponse};
    use crate::provider::ProviderOAuthTransportContext;
    use async_trait::async_trait;
    use serde_json::json;
    use std::sync::{Arc, Mutex};

    #[derive(Debug, Clone)]
    struct StaticExecutor {
        seen_request: Arc<Mutex<Option<OAuthHttpRequest>>>,
        response: serde_json::Value,
    }

    #[async_trait]
    impl OAuthHttpExecutor for StaticExecutor {
        async fn execute(
            &self,
            request: OAuthHttpRequest,
        ) -> Result<OAuthHttpResponse, crate::core::OAuthError> {
            *self.seen_request.lock().expect("mutex should lock") = Some(request);
            Ok(OAuthHttpResponse {
                status_code: 200,
                body_text: self.response.to_string(),
                json_body: Some(self.response.clone()),
            })
        }
    }

    #[derive(Debug, Clone)]
    struct RoutingExecutor {
        seen_requests: Arc<Mutex<Vec<OAuthHttpRequest>>>,
    }

    #[async_trait]
    impl OAuthHttpExecutor for RoutingExecutor {
        async fn execute(
            &self,
            request: OAuthHttpRequest,
        ) -> Result<OAuthHttpResponse, crate::core::OAuthError> {
            let request_id = request.request_id.clone();
            self.seen_requests
                .lock()
                .expect("mutex should lock")
                .push(request);
            let response = match request_id.as_str() {
                "provider-oauth:kiro-idc-refresh" => json!({
                    "accessToken": "new-idc-access-token",
                    "refreshToken": "i".repeat(120),
                    "expiresIn": 1800
                }),
                "provider-oauth:kiro-profile-discovery" => json!({
                    "profiles": [{
                        "arn": "arn:aws:codewhisperer:us-east-1:123456789012:profile/demo",
                        "name": "demo"
                    }]
                }),
                other => panic!("unexpected request id: {other}"),
            };
            Ok(OAuthHttpResponse {
                status_code: 200,
                body_text: response.to_string(),
                json_body: Some(response),
            })
        }
    }

    fn test_ctx() -> ProviderOAuthTransportContext {
        ProviderOAuthTransportContext {
            provider_id: "provider-1".to_string(),
            provider_type: "kiro".to_string(),
            endpoint_id: None,
            key_id: Some("key-1".to_string()),
            auth_type: Some("oauth".to_string()),
            decrypted_api_key: None,
            decrypted_auth_config: None,
            provider_config: None,
            endpoint_config: None,
            key_config: None,
            network: crate::network::OAuthNetworkContext::provider_operation(None),
        }
    }

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

    #[tokio::test]
    async fn refreshes_social_auth_config_with_provider_adapter() {
        let seen_request = Arc::new(Mutex::new(None));
        let executor = StaticExecutor {
            seen_request: Arc::clone(&seen_request),
            response: json!({
                "accessToken": "new-social-access-token",
                "refreshToken": "s".repeat(120),
                "expiresIn": 3600,
                "profileArn": "arn:aws:bedrock:demo"
            }),
        };
        let adapter = KiroProviderOAuthAdapter::default()
            .with_refresh_base_urls(Some("https://auth.example.test".to_string()), None);
        let auth_config = KiroAuthConfig {
            auth_method: Some("social".to_string()),
            refresh_token: Some("r".repeat(120)),
            expires_at: Some(1),
            profile_arn: None,
            region: None,
            auth_region: None,
            api_region: None,
            client_id: None,
            client_secret: None,
            machine_id: Some("123e4567-e89b-12d3-a456-426614174000".to_string()),
            kiro_version: Some("1.2.3".to_string()),
            system_version: None,
            node_version: None,
            access_token: None,
        };

        let refreshed = adapter
            .refresh_auth_config(&executor, &test_ctx(), &auth_config)
            .await
            .expect("social refresh should succeed");

        assert_eq!(
            refreshed.access_token.as_deref(),
            Some("new-social-access-token")
        );
        let expected_rotated_refresh_token = "s".repeat(120);
        assert_eq!(
            refreshed.refresh_token.as_deref(),
            Some(expected_rotated_refresh_token.as_str())
        );
        assert_eq!(
            refreshed.profile_arn.as_deref(),
            Some("arn:aws:bedrock:demo")
        );
        assert!(refreshed.expires_at.is_some());

        let seen = seen_request
            .lock()
            .expect("mutex should lock")
            .clone()
            .expect("request should be captured");
        assert_eq!(seen.request_id, "provider-oauth:kiro-social-refresh");
        assert_eq!(seen.method, reqwest::Method::POST);
        assert_eq!(seen.url, "https://auth.example.test/refreshToken");
        assert_eq!(
            seen.headers.get("user-agent").map(String::as_str),
            Some("KiroIDE-1.2.3-123e4567e89b12d3a456426614174000123e4567e89b12d3a456426614174000")
        );
        let expected_refresh_token = "r".repeat(120);
        assert_eq!(
            seen.json_body
                .as_ref()
                .and_then(|body| body.get("refreshToken"))
                .and_then(serde_json::Value::as_str),
            Some(expected_refresh_token.as_str())
        );
    }

    #[tokio::test]
    async fn refreshes_idc_auth_config_with_provider_adapter() {
        let seen_request = Arc::new(Mutex::new(None));
        let executor = StaticExecutor {
            seen_request: Arc::clone(&seen_request),
            response: json!({
                "accessToken": "new-idc-access-token",
                "refreshToken": "i".repeat(120),
                "expiresIn": 1800
            }),
        };
        let adapter = KiroProviderOAuthAdapter::default()
            .with_refresh_base_urls(None, Some("https://idc.example.test".to_string()));
        let auth_config = KiroAuthConfig {
            auth_method: Some("identity_center".to_string()),
            refresh_token: Some("r".repeat(120)),
            expires_at: Some(1),
            profile_arn: Some("arn:aws:bedrock:demo".to_string()),
            region: None,
            auth_region: None,
            api_region: None,
            client_id: Some("client-id".to_string()),
            client_secret: Some("client-secret".to_string()),
            machine_id: None,
            kiro_version: None,
            system_version: None,
            node_version: None,
            access_token: None,
        };

        let refreshed = adapter
            .refresh_auth_config(&executor, &test_ctx(), &auth_config)
            .await
            .expect("idc refresh should succeed");

        assert_eq!(
            refreshed.access_token.as_deref(),
            Some("new-idc-access-token")
        );
        let expected_rotated_refresh_token = "i".repeat(120);
        assert_eq!(
            refreshed.refresh_token.as_deref(),
            Some(expected_rotated_refresh_token.as_str())
        );
        assert_eq!(
            refreshed.profile_arn.as_deref(),
            Some("arn:aws:bedrock:demo")
        );
        assert!(refreshed.machine_id.is_some());
        assert!(refreshed.expires_at.is_some());

        let seen = seen_request
            .lock()
            .expect("mutex should lock")
            .clone()
            .expect("request should be captured");
        assert_eq!(seen.request_id, "provider-oauth:kiro-idc-refresh");
        assert_eq!(seen.method, reqwest::Method::POST);
        assert_eq!(seen.url, "https://idc.example.test/token");
        assert_eq!(
            seen.headers.get("user-agent").map(String::as_str),
            Some("node")
        );
        assert_eq!(
            seen.headers.get("x-amz-user-agent").map(String::as_str),
            Some(IDC_AMZ_USER_AGENT)
        );
        let body = seen.json_body.expect("json body should exist");
        assert_eq!(body.get("grantType"), Some(&json!("refresh_token")));
        assert_eq!(body.get("clientId"), Some(&json!("client-id")));
        assert_eq!(body.get("clientSecret"), Some(&json!("client-secret")));
        let expected_refresh_token = "r".repeat(120);
        assert_eq!(
            body.get("refreshToken").and_then(serde_json::Value::as_str),
            Some(expected_refresh_token.as_str())
        );
    }

    #[tokio::test]
    async fn refresh_discovers_missing_idc_profile_arn() {
        let seen_requests = Arc::new(Mutex::new(Vec::new()));
        let executor = RoutingExecutor {
            seen_requests: Arc::clone(&seen_requests),
        };
        let adapter = KiroProviderOAuthAdapter::default()
            .with_refresh_base_urls(None, Some("https://idc.example.test".to_string()));
        let auth_config = KiroAuthConfig {
            auth_method: Some("identity_center".to_string()),
            refresh_token: Some("r".repeat(120)),
            expires_at: Some(1),
            profile_arn: None,
            region: None,
            auth_region: None,
            api_region: Some("us-east-1".to_string()),
            client_id: Some("client-id".to_string()),
            client_secret: Some("client-secret".to_string()),
            machine_id: Some("123e4567-e89b-12d3-a456-426614174000".to_string()),
            kiro_version: Some("0.3.210".to_string()),
            system_version: None,
            node_version: None,
            access_token: None,
        };

        let refreshed = adapter
            .refresh_auth_config(&executor, &test_ctx(), &auth_config)
            .await
            .expect("idc refresh should discover profile arn");

        assert_eq!(
            refreshed.profile_arn.as_deref(),
            Some("arn:aws:codewhisperer:us-east-1:123456789012:profile/demo")
        );
        let seen_requests = seen_requests.lock().expect("mutex should lock").clone();
        assert_eq!(seen_requests.len(), 2);
        let discovery = seen_requests
            .iter()
            .find(|request| request.request_id == "provider-oauth:kiro-profile-discovery")
            .expect("profile discovery request should be sent");
        assert_eq!(discovery.method, reqwest::Method::POST);
        assert_eq!(
            discovery.url,
            "https://q.us-east-1.amazonaws.com/ListAvailableProfiles"
        );
        assert_eq!(
            discovery.headers.get("authorization").map(String::as_str),
            Some("Bearer new-idc-access-token")
        );
        assert_eq!(
            discovery.headers.get("host").map(String::as_str),
            Some("q.us-east-1.amazonaws.com")
        );
        assert_eq!(
            discovery
                .json_body
                .as_ref()
                .and_then(serde_json::Value::as_object)
                .map(|body| body.is_empty()),
            Some(true)
        );
    }
}
