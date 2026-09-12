use crate::admin_api::AdminAppState;
use crate::{AppState, GatewayError};
use aether_contracts::{
    ExecutionPlan, ExecutionResult, ExecutionTimeouts, ProxySnapshot, RequestBody,
    EXECUTION_REQUEST_FOLLOW_REDIRECTS_HEADER,
};
use aether_oauth::core::OAuthError;
use aether_oauth::network::{
    OAuthHttpExecutor, OAuthHttpRequest, OAuthHttpResponse, OAuthNetworkPolicy, OAuthTimeouts,
};
use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use flate2::read::{DeflateDecoder, GzDecoder};
use futures_util::StreamExt;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, CONTENT_ENCODING, CONTENT_TYPE};
use std::collections::BTreeMap;
use std::io::Read;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

const OAUTH_RESPONSE_BODY_LIMIT_BYTES: usize = 4 * 1024 * 1024;

// Some local DNS interception deployments return RFC 2544's benchmarking
// range (198.18.0.0/15) for well-known public services.  Identity OAuth is a
// sensitive direct-connection path, so the range is never accepted globally:
// only exact, built-in public origins may use it.  URL validation below still
// requires HTTPS, strips credentials/fragments, pins the resolved addresses,
// and disables redirects.
const TRUSTED_IDENTITY_BENCHMARKING_DNS_HOSTS: &[&str] = &[
    "accounts.google.com",
    "auth.openai.com",
    "claude.ai",
    "connect.linux.do",
    "connect.linuxdo.org",
    "oauth2.googleapis.com",
    "platform.claude.com",
    "register.windsurf.com",
    "server.self-serve.windsurf.com",
    "windsurf.com",
];

#[derive(Clone)]
pub(crate) struct GatewayOAuthHttpExecutor<'a> {
    app: AppState,
    _marker: std::marker::PhantomData<&'a AppState>,
}

impl<'a> GatewayOAuthHttpExecutor<'a> {
    pub(crate) fn new(state: AdminAppState<'a>) -> Self {
        Self {
            app: state.cloned_app(),
            _marker: std::marker::PhantomData,
        }
    }

    pub(crate) fn from_app(app: &'a AppState) -> Self {
        Self {
            app: app.clone(),
            _marker: std::marker::PhantomData,
        }
    }
}

#[async_trait]
impl<'a> OAuthHttpExecutor for GatewayOAuthHttpExecutor<'a> {
    async fn execute(&self, request: OAuthHttpRequest) -> Result<OAuthHttpResponse, OAuthError> {
        match request.network.policy {
            OAuthNetworkPolicy::DirectOnly | OAuthNetworkPolicy::DirectOrSystemProxy => {
                match identity_oauth_route(request.network.policy, request.network.proxy.as_ref())?
                {
                    IdentityOAuthRoute::Direct => {
                        execute_direct_identity_oauth(&self.app, request).await
                    }
                }
            }
            OAuthNetworkPolicy::ProviderOperationProxy => {
                #[cfg(test)]
                if self.app.execution_runtime_override_base_url().is_none()
                    && identity_oauth_endpoint_policy(&self.app, &request.url)
                        == IdentityOAuthEndpointPolicy::ExplicitTestLoopback
                {
                    return execute_direct_identity_oauth(&self.app, request).await;
                }
                execute_provider_oauth_via_runtime(&self.app, request).await
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IdentityOAuthRoute {
    Direct,
}

fn identity_oauth_route(
    policy: OAuthNetworkPolicy,
    proxy: Option<&ProxySnapshot>,
) -> Result<IdentityOAuthRoute, OAuthError> {
    let Some(proxy) = proxy.filter(|proxy| proxy.enabled != Some(false)) else {
        return Ok(IdentityOAuthRoute::Direct);
    };

    if policy == OAuthNetworkPolicy::DirectOnly {
        return Err(OAuthError::transport(
            "identity OAuth direct-only transport cannot use a proxy",
        ));
    }

    let has_proxy_url = proxy
        .url
        .as_deref()
        .map(str::trim)
        .is_some_and(|value| !value.is_empty());
    if has_proxy_url {
        return Err(OAuthError::transport(
            "identity OAuth HTTP/SOCKS proxies are disabled; use a controlled tunnel or direct transport",
        ));
    }

    Err(OAuthError::transport(
        "identity OAuth cannot use a configured proxy or tunnel; use direct transport",
    ))
}

async fn execute_provider_oauth_via_runtime(
    app: &AppState,
    request: OAuthHttpRequest,
) -> Result<OAuthHttpResponse, OAuthError> {
    let plan = oauth_execution_plan(request, false);
    let result = crate::execution_runtime::execute_execution_runtime_sync_plan(app, None, &plan)
        .await
        .map_err(gateway_error_to_oauth_error)?;
    Ok(execution_result_to_oauth_response(&result))
}

fn oauth_execution_plan(request: OAuthHttpRequest, force_disable_redirects: bool) -> ExecutionPlan {
    let OAuthHttpRequest {
        request_id,
        method,
        url,
        mut headers,
        content_type,
        json_body,
        body_bytes,
        network,
        transport_profile,
    } = request;
    let body = if let Some(json_body) = json_body {
        RequestBody::from_json(json_body)
    } else {
        RequestBody {
            json_body: None,
            body_bytes_b64: body_bytes.map(|bytes| STANDARD.encode(bytes)),
            body_ref: None,
        }
    };
    let timeouts = network.timeouts;
    if force_disable_redirects {
        headers.insert(
            EXECUTION_REQUEST_FOLLOW_REDIRECTS_HEADER.to_string(),
            "false".to_string(),
        );
    } else {
        headers
            .entry(EXECUTION_REQUEST_FOLLOW_REDIRECTS_HEADER.to_string())
            .or_insert_with(|| "true".to_string());
    }
    let plan = ExecutionPlan {
        request_id,
        candidate_id: None,
        provider_name: Some("oauth".to_string()),
        provider_id: String::new(),
        endpoint_id: String::new(),
        key_id: String::new(),
        method: method.as_str().to_string(),
        url,
        headers,
        content_type,
        content_encoding: None,
        body,
        stream: false,
        client_api_format: "oauth:exchange".to_string(),
        provider_api_format: "oauth:exchange".to_string(),
        model_name: Some("oauth-exchange".to_string()),
        proxy: network.proxy,
        transport_profile,
        timeouts: Some(ExecutionTimeouts {
            connect_ms: Some(timeouts.connect_ms),
            read_ms: Some(timeouts.read_ms),
            write_ms: Some(timeouts.write_ms),
            pool_ms: Some(timeouts.connect_ms),
            total_ms: Some(timeouts.total_ms),
            ..ExecutionTimeouts::default()
        }),
    };
    crate::execution_runtime::transport::with_upstream_response_body_limit(
        &plan,
        OAUTH_RESPONSE_BODY_LIMIT_BYTES,
    )
}

#[derive(Debug)]
struct ResolvedIdentityOAuthEndpoint {
    url: reqwest::Url,
    host: String,
    addrs: Vec<SocketAddr>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IdentityOAuthEndpointPolicy {
    PublicHttps,
    #[cfg(test)]
    ExplicitTestLoopback,
}

fn identity_oauth_endpoint_policy(app: &AppState, raw_url: &str) -> IdentityOAuthEndpointPolicy {
    #[cfg(test)]
    {
        let parsed_url = reqwest::Url::parse(raw_url).ok();
        let is_explicit_test_url = app
            .provider_oauth_token_url_overrides
            .lock()
            .expect("provider oauth token URL overrides should lock")
            .values()
            .any(|value| {
                parsed_url
                    .as_ref()
                    .is_some_and(|url| test_loopback_override_allows_url(value, url))
            });
        if is_explicit_test_url {
            return IdentityOAuthEndpointPolicy::ExplicitTestLoopback;
        }
    }

    let _ = (app, raw_url);
    IdentityOAuthEndpointPolicy::PublicHttps
}

#[cfg(test)]
fn test_loopback_override_allows_url(override_url: &str, target: &reqwest::Url) -> bool {
    let Ok(registered) = reqwest::Url::parse(override_url) else {
        return false;
    };
    let Some(target_ip) = target
        .host_str()
        .and_then(|host| host.parse::<IpAddr>().ok())
        .filter(|address| address.is_loopback())
    else {
        return false;
    };
    let same_origin = registered.scheme() == target.scheme()
        && registered
            .host_str()
            .and_then(|host| host.parse::<IpAddr>().ok())
            == Some(target_ip)
        && registered.port_or_known_default() == target.port_or_known_default();
    if !same_origin || registered.query().is_some() || registered.fragment().is_some() {
        return registered == *target;
    }

    let registered_path = registered.path().trim_end_matches('/');
    registered == *target
        || registered_path.is_empty()
        || target
            .path()
            .strip_prefix(registered_path)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

fn parse_identity_oauth_endpoint(
    raw_url: &str,
    policy: IdentityOAuthEndpointPolicy,
) -> Result<(reqwest::Url, String, u16), OAuthError> {
    let url = reqwest::Url::parse(raw_url)
        .map_err(|_| OAuthError::transport("identity OAuth endpoint URL is invalid"))?;
    if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
        return Err(OAuthError::transport(
            "identity OAuth endpoint must not contain credentials or a fragment",
        ));
    }
    let host = url
        .host_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| OAuthError::transport("identity OAuth endpoint is missing a host"))?;
    match policy {
        IdentityOAuthEndpointPolicy::PublicHttps if url.scheme() != "https" => {
            return Err(OAuthError::transport(
                "identity OAuth endpoint must use HTTPS",
            ));
        }
        #[cfg(test)]
        IdentityOAuthEndpointPolicy::ExplicitTestLoopback => {
            let is_loopback_literal = host
                .parse::<IpAddr>()
                .is_ok_and(|address| address.is_loopback());
            if !matches!(url.scheme(), "http" | "https") || !is_loopback_literal {
                return Err(OAuthError::transport(
                    "test identity OAuth endpoint must use a loopback IP literal",
                ));
            }
        }
        _ => {}
    }
    let port = url
        .port_or_known_default()
        .ok_or_else(|| OAuthError::transport("identity OAuth endpoint is missing a port"))?;
    Ok((url, host, port))
}

fn validate_identity_oauth_resolved_addrs(
    url: &reqwest::Url,
    addrs: &[SocketAddr],
    policy: IdentityOAuthEndpointPolicy,
) -> Result<(), OAuthError> {
    if addrs.is_empty() {
        return Err(OAuthError::transport(
            "identity OAuth endpoint DNS resolution returned no addresses",
        ));
    }
    #[cfg(test)]
    if policy == IdentityOAuthEndpointPolicy::ExplicitTestLoopback {
        if addrs.iter().all(|addr| addr.ip().is_loopback()) {
            return Ok(());
        }
        return Err(OAuthError::transport(
            "test identity OAuth endpoint must resolve only to loopback addresses",
        ));
    }
    let allows_benchmarking_dns = policy == IdentityOAuthEndpointPolicy::PublicHttps
        && identity_oauth_origin_allows_benchmarking_dns(url);
    if addrs.iter().any(|addr| {
        aether_http::is_private_or_reserved_ip(addr.ip())
            && !(allows_benchmarking_dns && aether_http::is_ipv4_benchmarking_fake_ip(addr.ip()))
    }) {
        return Err(OAuthError::transport(
            "identity OAuth endpoint resolves to a private or reserved address",
        ));
    }
    Ok(())
}

fn identity_oauth_origin_allows_benchmarking_dns(url: &reqwest::Url) -> bool {
    url.scheme() == "https"
        && url.port_or_known_default() == Some(443)
        && url.username().is_empty()
        && url.password().is_none()
        && url.fragment().is_none()
        && url.host_str().is_some_and(|host| {
            let host = host.trim_end_matches('.');
            TRUSTED_IDENTITY_BENCHMARKING_DNS_HOSTS
                .iter()
                .any(|trusted| trusted.eq_ignore_ascii_case(host))
        })
}

async fn resolve_identity_oauth_endpoint(
    raw_url: &str,
    policy: IdentityOAuthEndpointPolicy,
) -> Result<ResolvedIdentityOAuthEndpoint, OAuthError> {
    let (url, host, port) = parse_identity_oauth_endpoint(raw_url, policy)?;
    let addrs = if let Ok(ip) = host.parse::<IpAddr>() {
        vec![SocketAddr::new(ip, port)]
    } else {
        aether_http::lookup_host_with_limits(
            host.as_str(),
            port,
            aether_http::DEFAULT_DNS_LOOKUP_TIMEOUT,
        )
        .await
        .map_err(|_| OAuthError::transport("identity OAuth endpoint DNS resolution failed"))?
    };
    validate_identity_oauth_resolved_addrs(&url, &addrs, policy)?;
    Ok(ResolvedIdentityOAuthEndpoint { url, host, addrs })
}

fn build_pinned_identity_oauth_client(
    host: &str,
    addrs: &[SocketAddr],
    timeouts: OAuthTimeouts,
) -> Result<reqwest::Client, OAuthError> {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(identity_oauth_redirect_policy())
        .connect_timeout(Duration::from_millis(timeouts.connect_ms))
        .read_timeout(Duration::from_millis(timeouts.read_ms))
        .timeout(Duration::from_millis(timeouts.total_ms))
        .resolve_to_addrs(host, addrs)
        .build()
        .map_err(|_| OAuthError::transport("identity OAuth HTTP client initialization failed"))
}

fn identity_oauth_redirect_policy() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::none()
}

fn identity_oauth_headers(
    headers: &BTreeMap<String, String>,
    content_type: Option<&str>,
) -> Result<HeaderMap, OAuthError> {
    let mut result = HeaderMap::new();
    for (name, value) in headers {
        if name.eq_ignore_ascii_case(EXECUTION_REQUEST_FOLLOW_REDIRECTS_HEADER) {
            continue;
        }
        let name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| OAuthError::transport("identity OAuth request has an invalid header"))?;
        let value = HeaderValue::from_str(value)
            .map_err(|_| OAuthError::transport("identity OAuth request has an invalid header"))?;
        result.insert(name, value);
    }
    if !result.contains_key(CONTENT_TYPE) {
        if let Some(content_type) = content_type {
            let value = HeaderValue::from_str(content_type).map_err(|_| {
                OAuthError::transport("identity OAuth request has an invalid content type")
            })?;
            result.insert(CONTENT_TYPE, value);
        }
    }
    Ok(result)
}

async fn execute_direct_identity_oauth(
    app: &AppState,
    request: OAuthHttpRequest,
) -> Result<OAuthHttpResponse, OAuthError> {
    let endpoint_policy = identity_oauth_endpoint_policy(app, &request.url);
    let target = resolve_identity_oauth_endpoint(&request.url, endpoint_policy).await?;
    let client = build_pinned_identity_oauth_client(
        target.host.as_str(),
        &target.addrs,
        request.network.timeouts,
    )?;
    let headers = identity_oauth_headers(&request.headers, request.content_type.as_deref())?;
    let mut builder = client.request(request.method, target.url).headers(headers);
    if let Some(json_body) = request.json_body {
        builder = builder.json(&json_body);
    } else if let Some(body_bytes) = request.body_bytes {
        builder = builder.body(body_bytes);
    }

    let response = builder
        .send()
        .await
        .map_err(|_| OAuthError::transport("identity OAuth request failed"))?;
    let status_code = response.status().as_u16();
    let content_encoding = response
        .headers()
        .get(CONTENT_ENCODING)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let body_bytes = collect_identity_oauth_response_body(response).await?;
    let decoded = decode_response_bytes_with_limit(
        &body_bytes,
        content_encoding.as_deref(),
        OAUTH_RESPONSE_BODY_LIMIT_BYTES,
    )?
    .unwrap_or(body_bytes);
    Ok(OAuthHttpResponse {
        status_code,
        body_text: String::from_utf8_lossy(&decoded).to_string(),
        json_body: serde_json::from_slice(&decoded).ok(),
    })
}

async fn collect_identity_oauth_response_body(
    response: reqwest::Response,
) -> Result<Vec<u8>, OAuthError> {
    if response
        .content_length()
        .is_some_and(|length| length > OAUTH_RESPONSE_BODY_LIMIT_BYTES as u64)
    {
        return Err(oauth_response_too_large());
    }

    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk =
            chunk.map_err(|_| OAuthError::transport("identity OAuth response body read failed"))?;
        if chunk.len() > OAUTH_RESPONSE_BODY_LIMIT_BYTES.saturating_sub(body.len()) {
            return Err(oauth_response_too_large());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn oauth_response_too_large() -> OAuthError {
    OAuthError::transport(format!(
        "OAuth response body exceeds {OAUTH_RESPONSE_BODY_LIMIT_BYTES} bytes"
    ))
}

fn execution_result_to_oauth_response(result: &ExecutionResult) -> OAuthHttpResponse {
    OAuthHttpResponse {
        status_code: result.status_code,
        body_text: execution_body_text(result),
        json_body: execution_json_body(result),
    }
}

fn execution_json_body(result: &ExecutionResult) -> Option<serde_json::Value> {
    result
        .body
        .as_ref()
        .and_then(|body| body.json_body.clone())
        .or_else(|| {
            result
                .body
                .as_ref()
                .and_then(|body| execution_body_bytes(&result.headers, body))
                .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        })
}

fn execution_body_text(result: &ExecutionResult) -> String {
    result
        .body
        .as_ref()
        .and_then(|body| execution_body_bytes(&result.headers, body))
        .map(|bytes| String::from_utf8_lossy(&bytes).to_string())
        .or_else(|| {
            result
                .body
                .as_ref()
                .and_then(|body| body.json_body.as_ref())
                .and_then(|value| serde_json::to_string(value).ok())
        })
        .unwrap_or_default()
}

fn execution_body_bytes(
    headers: &BTreeMap<String, String>,
    body: &aether_contracts::ResponseBody,
) -> Option<Vec<u8>> {
    let bytes = body.body_bytes_b64.as_deref().and_then(|value| {
        crate::execution_runtime::transport::decode_base64_body_with_limit(
            value,
            OAUTH_RESPONSE_BODY_LIMIT_BYTES,
        )
        .ok()
    })?;
    decode_response_bytes_with_limit(
        &bytes,
        headers.get("content-encoding").map(String::as_str),
        OAUTH_RESPONSE_BODY_LIMIT_BYTES,
    )
    .ok()
    .flatten()
    .or(Some(bytes))
}

fn decode_response_bytes_with_limit(
    bytes: &[u8],
    content_encoding: Option<&str>,
    limit_bytes: usize,
) -> Result<Option<Vec<u8>>, OAuthError> {
    match content_encoding
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("gzip") => {
            let mut decoder = GzDecoder::new(bytes);
            read_oauth_response_decoder(&mut decoder, limit_bytes).map(Some)
        }
        Some("deflate") => {
            let mut decoder = DeflateDecoder::new(bytes);
            read_oauth_response_decoder(&mut decoder, limit_bytes).map(Some)
        }
        _ => Ok(None),
    }
}

fn read_oauth_response_decoder(
    decoder: &mut impl Read,
    limit_bytes: usize,
) -> Result<Vec<u8>, OAuthError> {
    let read_limit = u64::try_from(limit_bytes)
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    let mut limited = decoder.take(read_limit);
    let mut out = Vec::new();
    limited
        .read_to_end(&mut out)
        .map_err(|_| OAuthError::transport("OAuth response body decompression failed"))?;
    if out.len() > limit_bytes {
        return Err(oauth_response_too_large());
    }
    Ok(out)
}

fn gateway_error_to_oauth_error(error: GatewayError) -> OAuthError {
    OAuthError::Transport(error.into_message())
}

#[cfg(test)]
mod tests {
    use super::{
        decode_response_bytes_with_limit, execution_body_bytes, identity_oauth_endpoint_policy,
        identity_oauth_origin_allows_benchmarking_dns, identity_oauth_redirect_policy,
        identity_oauth_route, oauth_execution_plan, parse_identity_oauth_endpoint,
        resolve_identity_oauth_endpoint, validate_identity_oauth_resolved_addrs,
        IdentityOAuthEndpointPolicy, IdentityOAuthRoute,
    };
    use aether_contracts::{
        ProxySnapshot, ResponseBody, EXECUTION_REQUEST_FOLLOW_REDIRECTS_HEADER,
    };
    use aether_oauth::network::{
        OAuthHttpRequest, OAuthNetworkContext, OAuthNetworkPolicy, OAuthTimeouts,
    };
    use std::collections::BTreeMap;
    use std::io::Write;
    use std::net::SocketAddr;

    fn identity_request(proxy: Option<ProxySnapshot>) -> OAuthHttpRequest {
        OAuthHttpRequest {
            request_id: "identity-oauth:test".to_string(),
            method: reqwest::Method::GET,
            url: "https://oauth.example.test/userinfo".to_string(),
            headers: BTreeMap::new(),
            content_type: None,
            json_body: None,
            body_bytes: None,
            network: OAuthNetworkContext {
                policy: OAuthNetworkPolicy::DirectOrSystemProxy,
                requirement: aether_oauth::network::NetworkRequirement::Optional,
                proxy,
                timeouts: OAuthTimeouts::DIRECT_DEFAULT,
            },
            transport_profile: None,
        }
    }

    #[tokio::test]
    async fn private_ip_literal_is_rejected_before_connect() {
        let error = resolve_identity_oauth_endpoint(
            "https://127.0.0.1/token",
            IdentityOAuthEndpointPolicy::PublicHttps,
        )
        .await
        .expect_err("loopback identity endpoint must be rejected");

        assert!(error.to_string().contains("private or reserved"));
    }

    #[test]
    fn public_https_endpoint_and_resolved_addresses_are_accepted_without_dns() {
        let (url, host, port) = parse_identity_oauth_endpoint(
            "https://oauth.example.test/token?flow=login",
            IdentityOAuthEndpointPolicy::PublicHttps,
        )
        .expect("public HTTPS URL should parse");
        let addrs = ["8.8.8.8:443".parse::<SocketAddr>().unwrap()];

        assert_eq!(url.scheme(), "https");
        assert_eq!(host, "oauth.example.test");
        assert_eq!(port, 443);
        validate_identity_oauth_resolved_addrs(
            &url,
            &addrs,
            IdentityOAuthEndpointPolicy::PublicHttps,
        )
        .expect("controlled public address should pass validation");
    }

    #[test]
    fn identity_oauth_rejects_credentials_fragments_and_any_private_dns_answer() {
        assert!(parse_identity_oauth_endpoint(
            "https://user:secret@example.test/token",
            IdentityOAuthEndpointPolicy::PublicHttps,
        )
        .is_err());
        assert!(parse_identity_oauth_endpoint(
            "https://example.test/token#secret",
            IdentityOAuthEndpointPolicy::PublicHttps,
        )
        .is_err());
        assert!(parse_identity_oauth_endpoint(
            "http://example.test/token",
            IdentityOAuthEndpointPolicy::PublicHttps,
        )
        .is_err());

        let mixed = [
            "8.8.8.8:443".parse::<SocketAddr>().unwrap(),
            "10.0.0.4:443".parse::<SocketAddr>().unwrap(),
        ];
        assert!(validate_identity_oauth_resolved_addrs(
            &reqwest::Url::parse("https://example.test/token").unwrap(),
            &mixed,
            IdentityOAuthEndpointPolicy::PublicHttps,
        )
        .is_err());
    }

    #[test]
    fn explicit_test_endpoint_policy_only_allows_loopback_literals() {
        let (_, host, port) = parse_identity_oauth_endpoint(
            "http://127.0.0.1:32123/token",
            IdentityOAuthEndpointPolicy::ExplicitTestLoopback,
        )
        .expect("explicit test loopback URL should parse");
        assert_eq!(host, "127.0.0.1");
        assert_eq!(port, 32123);
        validate_identity_oauth_resolved_addrs(
            &reqwest::Url::parse("http://127.0.0.1:32123/token").unwrap(),
            &["127.0.0.1:32123".parse().unwrap()],
            IdentityOAuthEndpointPolicy::ExplicitTestLoopback,
        )
        .expect("loopback resolution should be accepted for an explicit test endpoint");

        assert!(parse_identity_oauth_endpoint(
            "http://10.0.0.1/token",
            IdentityOAuthEndpointPolicy::ExplicitTestLoopback,
        )
        .is_err());
        assert!(parse_identity_oauth_endpoint(
            "http://localhost/token",
            IdentityOAuthEndpointPolicy::ExplicitTestLoopback,
        )
        .is_err());
        assert!(validate_identity_oauth_resolved_addrs(
            &reqwest::Url::parse("http://10.0.0.1/token").unwrap(),
            &["10.0.0.1:80".parse().unwrap()],
            IdentityOAuthEndpointPolicy::ExplicitTestLoopback,
        )
        .is_err());
    }

    #[test]
    fn identity_oauth_benchmarking_dns_is_exact_origin_only() {
        let fake = "198.18.75.234:443".parse::<SocketAddr>().unwrap();
        let trusted = reqwest::Url::parse("https://CONNECT.LINUX.DO/oauth2/token").unwrap();
        assert!(identity_oauth_origin_allows_benchmarking_dns(&trusted));
        assert!(validate_identity_oauth_resolved_addrs(
            &trusted,
            &[fake],
            IdentityOAuthEndpointPolicy::PublicHttps,
        )
        .is_ok());

        for raw_url in [
            "https://connect.linux.do:8443/oauth2/token",
            "https://connect.linux.do.evil.test/oauth2/token",
            "http://connect.linux.do/oauth2/token",
            "https://oauth.example.test/token",
        ] {
            let url = reqwest::Url::parse(raw_url).unwrap();
            assert!(!identity_oauth_origin_allows_benchmarking_dns(&url));
            assert!(validate_identity_oauth_resolved_addrs(
                &url,
                &[fake],
                IdentityOAuthEndpointPolicy::PublicHttps,
            )
            .is_err());
        }

        let mixed = [fake, "127.0.0.1:443".parse::<SocketAddr>().unwrap()];
        assert!(validate_identity_oauth_resolved_addrs(
            &trusted,
            &mixed,
            IdentityOAuthEndpointPolicy::PublicHttps,
        )
        .is_err());
    }

    #[test]
    fn test_loopback_policy_requires_a_registered_loopback_origin_and_path() {
        let app = crate::AppState::new()
            .expect("gateway state should build")
            .with_provider_oauth_token_url_for_tests("codex", "http://127.0.0.1:32123/oauth")
            .with_provider_oauth_token_url_for_tests("bad", "http://10.0.0.1/token");

        assert_eq!(
            identity_oauth_endpoint_policy(&app, "http://127.0.0.1:32123/oauth/token"),
            IdentityOAuthEndpointPolicy::ExplicitTestLoopback
        );
        assert_eq!(
            identity_oauth_endpoint_policy(&app, "http://127.0.0.1:32123/oauth2/token"),
            IdentityOAuthEndpointPolicy::PublicHttps
        );
        assert_eq!(
            identity_oauth_endpoint_policy(&app, "http://127.0.0.1:32124/oauth/token"),
            IdentityOAuthEndpointPolicy::PublicHttps
        );
        assert_eq!(
            identity_oauth_endpoint_policy(&app, "http://10.0.0.1/token"),
            IdentityOAuthEndpointPolicy::PublicHttps
        );
    }

    #[test]
    fn identity_oauth_disables_redirects_for_direct_and_tunnel_requests() {
        assert_eq!(
            format!("{:?}", identity_oauth_redirect_policy()),
            "Policy(None)"
        );

        let tunnel = ProxySnapshot {
            enabled: Some(true),
            mode: Some("tunnel".to_string()),
            node_id: Some("node-1".to_string()),
            ..ProxySnapshot::default()
        };
        assert!(
            identity_oauth_route(OAuthNetworkPolicy::DirectOrSystemProxy, Some(&tunnel)).is_err()
        );
        let plan = oauth_execution_plan(identity_request(None), true);
        assert_eq!(
            plan.headers
                .get(EXECUTION_REQUEST_FOLLOW_REDIRECTS_HEADER)
                .map(String::as_str),
            Some("false")
        );
        assert_eq!(
            crate::execution_runtime::transport::execution_plan_response_body_limit_bytes(&plan),
            super::OAUTH_RESPONSE_BODY_LIMIT_BYTES
        );
    }

    #[test]
    fn identity_oauth_rejects_forward_proxy_snapshots() {
        let proxy = ProxySnapshot {
            enabled: Some(true),
            mode: Some("http".to_string()),
            url: Some("http://proxy.example.test:8080".to_string()),
            ..ProxySnapshot::default()
        };

        assert!(
            identity_oauth_route(OAuthNetworkPolicy::DirectOrSystemProxy, Some(&proxy)).is_err()
        );
        assert_eq!(
            identity_oauth_route(OAuthNetworkPolicy::DirectOrSystemProxy, None).unwrap(),
            IdentityOAuthRoute::Direct
        );
    }

    #[test]
    fn identity_oauth_rejects_enabled_tunnel_even_when_node_id_is_present() {
        let proxy = ProxySnapshot {
            enabled: Some(true),
            mode: Some("tunnel".to_string()),
            node_id: Some("node-1".to_string()),
            ..ProxySnapshot::default()
        };
        assert!(
            identity_oauth_route(OAuthNetworkPolicy::DirectOrSystemProxy, Some(&proxy)).is_err()
        );
    }

    #[test]
    fn oauth_response_decoder_rejects_decompression_bombs() {
        let payload = b"123456789";
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder
            .write_all(payload)
            .expect("gzip payload should write");
        let encoded = encoder.finish().expect("gzip payload should finish");

        let error = decode_response_bytes_with_limit(&encoded, Some("gzip"), 8)
            .expect_err("decoded OAuth body above the limit must fail closed");

        assert!(error.to_string().contains("exceeds"));
    }

    #[test]
    fn oauth_execution_body_rejects_oversized_base64_before_decode() {
        let encoded_limit =
            crate::execution_runtime::transport::maximum_base64_len_for_decoded_limit(
                super::OAUTH_RESPONSE_BODY_LIMIT_BYTES,
            );
        let body = ResponseBody {
            json_body: None,
            body_bytes_b64: Some("A".repeat(encoded_limit + 1)),
        };

        assert!(execution_body_bytes(&BTreeMap::new(), &body).is_none());
    }
}
