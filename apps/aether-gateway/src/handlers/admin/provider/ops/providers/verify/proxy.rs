use super::request::admin_provider_ops_execute_get_text_no_redirect;
use crate::handlers::admin::request::AdminAppState;
use aether_admin::provider::ops::admin_provider_ops_anyrouter_compute_acw_sc_v2;
use aether_contracts::ProxySnapshot;
use regex::Regex;
use serde_json::{Map, Value};

pub(in super::super) struct AdminProviderOpsAnyrouterChallenge {
    pub(in super::super) acw_cookie: String,
}

pub(in super::super) async fn admin_provider_ops_anyrouter_acw_cookie(
    state: &AdminAppState<'_>,
    base_url: &str,
    connector_config: Option<&Map<String, Value>>,
) -> Option<AdminProviderOpsAnyrouterChallenge> {
    let proxy_snapshot = admin_provider_ops_resolve_proxy_snapshot(state, connector_config).await;
    let headers = reqwest::header::HeaderMap::from_iter([(
        reqwest::header::USER_AGENT,
        reqwest::header::HeaderValue::from_static(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
        ),
    )]);
    let response = admin_provider_ops_execute_get_text_no_redirect(
        state,
        "provider-ops-acw:anyrouter",
        base_url.trim_end_matches('/'),
        &headers,
        proxy_snapshot.as_ref(),
    )
    .await
    .ok()?;
    let compiled = Regex::new(r"var\s+arg1\s*=\s*'([0-9a-fA-F]{40})'").ok()?;
    let captures = compiled.captures(&response.body)?;
    let arg1 = captures.get(1)?.as_str();
    admin_provider_ops_anyrouter_compute_acw_sc_v2(arg1).map(|value| {
        AdminProviderOpsAnyrouterChallenge {
            acw_cookie: format!("acw_sc__v2={value}"),
        }
    })
}

pub(in super::super) async fn admin_provider_ops_resolve_proxy_snapshot(
    state: &AdminAppState<'_>,
    connector_config: Option<&Map<String, Value>>,
) -> Option<ProxySnapshot> {
    state
        .resolve_admin_connector_proxy_snapshot(connector_config)
        .await
}
