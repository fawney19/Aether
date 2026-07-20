use std::collections::BTreeMap;

use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogEndpoint;
use serde_json::{Map, Value};
use url::Url;

use crate::capability::ProviderPoolCapabilities;
use crate::provider::{
    provider_pool_endpoint_format_matches, provider_pool_matching_endpoint, ProviderPoolAdapter,
    ProviderPoolMemberInput,
};
use crate::quota::{
    provider_pool_current_unix_secs, provider_pool_json_f64, provider_pool_metadata_bucket,
    provider_pool_quota_snapshot_exhausted_decision, provider_pool_timestamp_unix_secs,
};
use crate::quota_refresh::ProviderPoolQuotaRequestSpec;

pub const GROK_OAUTH_DEFAULT_BASE_URL: &str = "https://cli-chat-proxy.grok.com/v1";
pub const GROK_OAUTH_BILLING_WEEKLY_PATH: &str = "/billing?format=credits";
pub const GROK_OAUTH_BILLING_MONTHLY_PATH: &str = "/billing";

const GROK_OAUTH_CLI_VERSION: &str = "0.2.93";
const GROK_OAUTH_TOKEN_AUTH_VALUE: &str = "xai-grok-cli";

fn grok_oauth_billing_user_agent(cli_version: &str) -> String {
    format!("grok-pager/{cli_version} grok-shell/{cli_version} (macos; aarch64)")
}

#[derive(Debug, Clone, Default)]
pub struct GrokOAuthProviderPoolAdapter;

impl ProviderPoolAdapter for GrokOAuthProviderPoolAdapter {
    fn provider_type(&self) -> &'static str {
        "grok_oauth"
    }

    fn capabilities(&self) -> ProviderPoolCapabilities {
        ProviderPoolCapabilities {
            plan_tier: true,
            quota_reset: false,
            quota_refresh: true,
        }
    }

    fn quota_exhausted(&self, input: &ProviderPoolMemberInput<'_>) -> bool {
        if let Some(exhausted) =
            provider_pool_quota_snapshot_exhausted_decision(input.key, input.provider_type)
        {
            return exhausted;
        }
        provider_pool_metadata_bucket(input.key.upstream_metadata.as_ref(), input.provider_type)
            .is_some_and(quota_exhausted_from_bucket)
    }

    fn quota_refresh_endpoint(
        &self,
        endpoints: &[StoredProviderCatalogEndpoint],
        include_inactive: bool,
    ) -> Option<StoredProviderCatalogEndpoint> {
        provider_pool_matching_endpoint(endpoints, include_inactive, |endpoint| {
            provider_pool_endpoint_format_matches(endpoint, "openai:responses")
        })
    }

    fn quota_refresh_missing_endpoint_message(&self) -> String {
        "找不到有效的 openai:responses 端点".to_string()
    }
}

fn auth_config_header(auth_config: Option<&Value>, name: &str) -> Option<String> {
    auth_config
        .and_then(|value| value.get("headers"))
        .and_then(Value::as_object)?
        .iter()
        .find(|(header_name, _)| header_name.eq_ignore_ascii_case(name))
        .and_then(|(_, value)| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn grok_oauth_billing_url(base_url: &str, weekly: bool) -> Result<String, String> {
    let base_url = if base_url.trim().is_empty() {
        GROK_OAUTH_DEFAULT_BASE_URL
    } else {
        base_url.trim()
    };
    let mut url = Url::parse(base_url).map_err(|_| "Grok OAuth base_url 无效".to_string())?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("Grok OAuth base_url 仅支持 http/https".to_string());
    }
    let suffix = if weekly {
        GROK_OAUTH_BILLING_WEEKLY_PATH
    } else {
        GROK_OAUTH_BILLING_MONTHLY_PATH
    };
    let (suffix_path, query) = suffix
        .split_once('?')
        .map_or((suffix, None), |(path, query)| (path, Some(query)));
    let path = format!("{}{}", url.path().trim_end_matches('/'), suffix_path);
    url.set_path(&path);
    url.set_query(query);
    url.set_fragment(None);
    Ok(url.to_string())
}

pub fn build_grok_oauth_pool_billing_request(
    key_id: &str,
    base_url: &str,
    authorization: (String, String),
    auth_config: Option<&Value>,
    weekly: bool,
) -> Result<ProviderPoolQuotaRequestSpec, String> {
    let (authorization_name, authorization_value) = authorization;
    let authorization_name = authorization_name.trim();
    let authorization_value = authorization_value.trim();
    if authorization_name.is_empty() || authorization_value.is_empty() {
        return Err("缺少 Grok OAuth 认证信息，请先授权/刷新 Token".to_string());
    }

    let cli_version = auth_config_header(auth_config, "x-grok-client-version")
        .unwrap_or_else(|| GROK_OAUTH_CLI_VERSION.to_string());
    let billing_user_agent = grok_oauth_billing_user_agent(&cli_version);
    let mut headers = BTreeMap::from([
        ("accept".to_string(), "application/json".to_string()),
        ("content-type".to_string(), "application/json".to_string()),
        (
            "x-xai-token-auth".to_string(),
            auth_config_header(auth_config, "x-xai-token-auth")
                .unwrap_or_else(|| GROK_OAUTH_TOKEN_AUTH_VALUE.to_string()),
        ),
        ("x-grok-client-version".to_string(), cli_version),
        ("user-agent".to_string(), billing_user_agent),
    ]);
    headers.insert(
        authorization_name.to_ascii_lowercase(),
        authorization_value.to_string(),
    );

    let quota_window = if weekly { "weekly" } else { "monthly" };
    Ok(ProviderPoolQuotaRequestSpec {
        request_id: format!("grok-oauth-billing-{quota_window}:{key_id}"),
        provider_name: "grok_oauth".to_string(),
        quota_kind: format!("grok_oauth_billing_{quota_window}"),
        method: "GET".to_string(),
        url: grok_oauth_billing_url(base_url, weekly)?,
        headers,
        content_type: None,
        json_body: None,
        client_api_format: "openai:responses".to_string(),
        provider_api_format: "grok_oauth:billing".to_string(),
        model_name: Some(format!("grok-oauth-billing-{quota_window}")),
        accept_invalid_certs: false,
    })
}

fn quota_window_exhausted(bucket: &Map<String, Value>, prefix: &str) -> bool {
    let used_percent = provider_pool_json_f64(bucket.get(&format!("{prefix}_used_percent")));
    if !used_percent.is_some_and(|value| value >= 100.0) {
        return false;
    }

    let reset_at = provider_pool_timestamp_unix_secs(bucket.get(&format!("{prefix}_reset_at")));
    provider_pool_current_unix_secs()
        .zip(reset_at)
        .is_none_or(|(now, reset_at)| now < reset_at)
}

fn quota_exhausted_from_bucket(bucket: &Map<String, Value>) -> bool {
    quota_window_exhausted(bucket, "weekly") || quota_window_exhausted(bucket, "monthly")
}
