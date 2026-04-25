use serde_json::Value;
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};

pub const DEFAULT_REGION: &str = "us-east-1";
pub const DEFAULT_KIRO_VERSION: &str = "0.8.0";
pub const DEFAULT_NODE_VERSION: &str = "22.21.1";
pub const DEFAULT_SYSTEM_VERSION: &str = "other#unknown";

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
    pub fn from_raw_json(raw: Option<&str>) -> Option<Self> {
        let raw = raw?.trim();
        if raw.is_empty() {
            return None;
        }
        let parsed: Value = serde_json::from_str(raw).ok()?;
        Self::from_json_value(&parsed)
    }

    pub fn from_json_value(raw: &Value) -> Option<Self> {
        let object = raw.as_object()?;

        Some(Self {
            auth_method: get_nonempty_string(
                object,
                &["auth_method", "authMethod", "auth_type", "authType"],
            )
            .map(|value| normalize_auth_method(&value)),
            refresh_token: get_nonempty_string(object, &["refresh_token", "refreshToken"]),
            expires_at: get_epoch_seconds(object.get("expires_at"))
                .or_else(|| get_epoch_seconds(object.get("expiresAt"))),
            profile_arn: get_nonempty_string(object, &["profile_arn", "profileArn"]),
            region: get_nonempty_string(object, &["region"]),
            auth_region: get_nonempty_string(object, &["auth_region", "authRegion"]),
            api_region: get_nonempty_string(object, &["api_region", "apiRegion"]),
            client_id: get_nonempty_string(object, &["client_id", "clientId"]),
            client_secret: get_nonempty_string(object, &["client_secret", "clientSecret"]),
            machine_id: get_nonempty_string(object, &["machine_id", "machineId"]),
            kiro_version: get_nonempty_string(object, &["kiro_version", "kiroVersion"]),
            system_version: get_nonempty_string(object, &["system_version", "systemVersion"]),
            node_version: get_nonempty_string(object, &["node_version", "nodeVersion"]),
            access_token: get_nonempty_string(object, &["access_token", "accessToken"]),
        })
    }

    pub fn to_json_value(&self) -> Value {
        let mut object = serde_json::Map::new();
        insert_optional_string(&mut object, "auth_method", self.auth_method.as_deref());
        insert_optional_string(&mut object, "refresh_token", self.refresh_token.as_deref());
        if let Some(expires_at) = self.expires_at {
            object.insert("expires_at".to_string(), Value::from(expires_at));
        }
        insert_optional_string(&mut object, "profile_arn", self.profile_arn.as_deref());
        insert_optional_string(&mut object, "region", self.region.as_deref());
        insert_optional_string(&mut object, "auth_region", self.auth_region.as_deref());
        insert_optional_string(&mut object, "api_region", self.api_region.as_deref());
        insert_optional_string(&mut object, "client_id", self.client_id.as_deref());
        insert_optional_string(&mut object, "client_secret", self.client_secret.as_deref());
        insert_optional_string(&mut object, "machine_id", self.machine_id.as_deref());
        insert_optional_string(&mut object, "kiro_version", self.kiro_version.as_deref());
        insert_optional_string(
            &mut object,
            "system_version",
            self.system_version.as_deref(),
        );
        insert_optional_string(&mut object, "node_version", self.node_version.as_deref());
        insert_optional_string(&mut object, "access_token", self.access_token.as_deref());
        Value::Object(object)
    }

    pub fn effective_api_region(&self) -> &str {
        self.api_region
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(DEFAULT_REGION)
    }

    pub fn effective_auth_region(&self) -> &str {
        self.auth_region
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .or_else(|| {
                self.region
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
            })
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
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|value| value.as_secs())
            .unwrap_or_default();
        now >= expires_at.saturating_sub(skew_seconds)
    }

    pub fn is_idc_auth(&self) -> bool {
        let explicit_method = self
            .auth_method
            .as_deref()
            .map(normalize_auth_method)
            .unwrap_or_else(|| "social".to_string());
        if explicit_method != "social" {
            return explicit_method == "idc";
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

    pub fn profile_arn_for_payload(&self) -> Option<&str> {
        if self.is_idc_auth() {
            return None;
        }
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

pub fn normalize_machine_id(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }

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

pub fn generate_machine_id(
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

fn get_nonempty_string(object: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| object.get(*key))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn insert_optional_string(
    object: &mut serde_json::Map<String, Value>,
    key: &str,
    value: Option<&str>,
) {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return;
    };
    object.insert(key.to_string(), Value::String(value.to_string()));
}

fn get_epoch_seconds(value: Option<&Value>) -> Option<u64> {
    match value? {
        Value::Number(number) => number.as_u64().or_else(|| {
            number
                .as_i64()
                .and_then(|value| (value >= 0).then_some(value as u64))
        }),
        Value::String(text) => text.trim().parse::<u64>().ok(),
        _ => None,
    }
}

fn normalize_auth_method(raw: &str) -> String {
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
        _ => value,
    }
}

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
        assert!(auth_config.profile_arn_for_payload().is_none());
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
