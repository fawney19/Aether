use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use reqwest::dns::{Addrs, Name, Resolve, Resolving};

const EXPLICIT_UPDATE_PROXY_ENV_KEYS: &[&str] = &["AETHER_UPDATE_PROXY_URL", "UPDATE_PROXY_URL"];

const UPDATE_PROXY_ENV_KEYS: &[&str] = &[
    "AETHER_UPDATE_PROXY_URL",
    "UPDATE_PROXY_URL",
    "HTTPS_PROXY",
    "https_proxy",
    "ALL_PROXY",
    "all_proxy",
    "HTTP_PROXY",
    "http_proxy",
];

const UPDATE_GITHUB_TOKEN_ENV_KEYS: &[&str] =
    &["AETHER_UPDATE_GITHUB_TOKEN", "GITHUB_TOKEN", "GH_TOKEN"];

pub(crate) fn build_update_http_client(
    timeout: Duration,
    label: &str,
) -> Result<reqwest::Client, String> {
    let proxy_url = update_proxy_url_from_env();
    let proxy_host = proxy_url
        .as_deref()
        .and_then(update_proxy_host)
        .map(|host| host.trim_end_matches('.').to_ascii_lowercase());
    let mut builder = base_update_http_client_builder(timeout)
        .no_proxy()
        .dns_resolver(Arc::new(SafeUpdateDnsResolver::new(proxy_host)));
    if let Some(proxy_url) = proxy_url {
        let proxy = reqwest::Proxy::all(proxy_url)
            .map_err(|_| format!("创建{label}代理失败，请检查更新代理环境变量"))?
            .no_proxy(reqwest::NoProxy::from_env());
        builder = builder.proxy(proxy);
    }
    builder
        .build()
        .map_err(|err| format!("创建{label}客户端失败: {err}"))
}

pub(crate) fn build_direct_update_http_client(
    timeout: Duration,
    label: &str,
) -> Result<reqwest::Client, String> {
    base_update_http_client_builder(timeout)
        .no_proxy()
        .dns_resolver(Arc::new(SafeUpdateDnsResolver::new(None)))
        .build()
        .map_err(|err| format!("创建{label}客户端失败: {err}"))
}

pub(crate) fn has_explicit_update_proxy_env() -> bool {
    read_nonempty_env_value(EXPLICIT_UPDATE_PROXY_ENV_KEYS).is_some()
}

fn base_update_http_client_builder(timeout: Duration) -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 10 {
                return attempt.error("too many update download redirects");
            }
            if is_trusted_update_url(attempt.url()) {
                attempt.follow()
            } else {
                attempt.error("update download redirected to an untrusted URL")
            }
        }))
}

#[derive(Debug)]
struct SafeUpdateDnsResolver {
    private_allowed_host: Option<String>,
}

impl SafeUpdateDnsResolver {
    fn new(private_allowed_host: Option<String>) -> Self {
        Self {
            private_allowed_host,
        }
    }
}

impl Resolve for SafeUpdateDnsResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().trim_end_matches('.').to_ascii_lowercase();
        let allow_private = self.private_allowed_host.as_deref() == Some(host.as_str());
        // Transparent DNS proxies may use RFC 2544's 198.18.0.0/15 benchmark
        // range as a synthetic address. Update destinations are compiled-in
        // GitHub hosts, so accepting that range
        // for those exact hosts preserves proxy compatibility without opening
        // the resolver to arbitrary custom destinations.
        let allow_benchmarking_ip = is_trusted_update_host(&host);
        Box::pin(async move {
            let addresses = aether_http::lookup_host_with_limits(
                host.as_str(),
                0,
                aether_http::DEFAULT_DNS_LOOKUP_TIMEOUT,
            )
            .await
            .map_err(|error| -> Box<dyn std::error::Error + Send + Sync> { Box::new(error) })?;
            validate_update_resolved_addrs(&addresses, allow_private, allow_benchmarking_ip)
                .map_err(|message| {
                    Box::new(std::io::Error::other(message))
                        as Box<dyn std::error::Error + Send + Sync>
                })?;
            Ok(Box::new(addresses.into_iter()) as Addrs)
        })
    }
}

fn validate_update_resolved_addrs(
    addresses: &[SocketAddr],
    allow_private: bool,
    allow_benchmarking_ip: bool,
) -> Result<(), &'static str> {
    if addresses.is_empty() {
        return Err("update DNS resolution returned no addresses");
    }
    if !allow_private
        && addresses.iter().any(|address| {
            aether_http::is_private_or_reserved_ip(address.ip())
                && !(allow_benchmarking_ip
                    && aether_http::is_ipv4_benchmarking_fake_ip(address.ip()))
        })
    {
        return Err("update DNS resolution returned a private or reserved address");
    }
    Ok(())
}

fn is_trusted_update_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("github.com")
        || host.eq_ignore_ascii_case("api.github.com")
        || host.eq_ignore_ascii_case("objects.githubusercontent.com")
        || host.ends_with(".objects.githubusercontent.com")
        || host.eq_ignore_ascii_case("release-assets.githubusercontent.com")
        || host.ends_with(".release-assets.githubusercontent.com")
}

pub(crate) fn is_trusted_update_url(url: &url::Url) -> bool {
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    is_trusted_update_host(host)
}

fn update_proxy_url_from_env() -> Option<String> {
    read_nonempty_env_value(UPDATE_PROXY_ENV_KEYS)
}

fn update_proxy_host(proxy_url: &str) -> Option<String> {
    let parsed = url::Url::parse(proxy_url)
        .ok()
        .filter(url::Url::has_host)
        .or_else(|| url::Url::parse(&format!("http://{proxy_url}")).ok())?;
    parsed.host_str().map(ToOwned::to_owned)
}

pub(crate) fn update_github_token_from_env() -> Option<String> {
    read_nonempty_env_value(UPDATE_GITHUB_TOKEN_ENV_KEYS)
}

fn read_nonempty_env_value(keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        std::env::var(key)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    })
}

#[cfg(test)]
mod tests {
    use super::{
        is_trusted_update_host, is_trusted_update_url, update_proxy_host,
        validate_update_resolved_addrs,
    };
    use std::net::SocketAddr;

    #[test]
    fn update_url_trust_rejects_credentials_and_untrusted_hosts() {
        for trusted in [
            "https://github.com/fawney19/Aether/releases/download/v1/aether.tar.gz",
            "https://api.github.com/repos/fawney19/Aether/releases",
            "https://objects.githubusercontent.com/github-production-release-asset/test",
            "https://release-assets.githubusercontent.com/github-production-release-asset/test",
        ] {
            assert!(is_trusted_update_url(&url::Url::parse(trusted).unwrap()));
        }
        for untrusted in [
            "http://github.com/fawney19/Aether/releases/download/v1/aether.tar.gz",
            "https://github.com.evil.example/aether.tar.gz",
            "https://user@github.com/aether.tar.gz",
            "https://example.com/aether.tar.gz",
        ] {
            assert!(!is_trusted_update_url(&url::Url::parse(untrusted).unwrap()));
        }
    }

    #[test]
    fn update_dns_rejects_private_or_mixed_target_answers() {
        let public = "8.8.8.8:443".parse::<SocketAddr>().unwrap();
        let private = "127.0.0.1:443".parse::<SocketAddr>().unwrap();

        assert!(validate_update_resolved_addrs(&[public], false, false).is_ok());
        assert!(validate_update_resolved_addrs(&[private], false, false).is_err());
        assert!(validate_update_resolved_addrs(&[public, private], false, false).is_err());
        assert!(validate_update_resolved_addrs(&[private], true, false).is_ok());
        assert!(validate_update_resolved_addrs(&[], false, false).is_err());
    }

    #[test]
    fn update_dns_allows_benchmarking_ip_only_for_trusted_github_hosts() {
        let fake = "198.18.75.234:443".parse::<SocketAddr>().unwrap();
        assert!(validate_update_resolved_addrs(&[fake], false, true).is_ok());
        assert!(validate_update_resolved_addrs(
            &[fake, "127.0.0.1:443".parse().unwrap()],
            false,
            true,
        )
        .is_err());
        assert!(validate_update_resolved_addrs(&[fake], false, false).is_err());
        assert!(is_trusted_update_host("api.github.com"));
        assert!(is_trusted_update_host("foo.objects.githubusercontent.com"));
        assert!(!is_trusted_update_host("github.com.evil.example"));
    }

    #[test]
    fn update_proxy_host_supports_explicit_and_legacy_proxy_urls() {
        assert_eq!(
            update_proxy_host("http://user:secret@proxy.example.test:8080").as_deref(),
            Some("proxy.example.test")
        );
        assert_eq!(
            update_proxy_host("127.0.0.1:7890").as_deref(),
            Some("127.0.0.1")
        );
    }
}
