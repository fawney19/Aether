use std::time::Duration;

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
    let mut builder = base_update_http_client_builder(timeout);
    if let Some(proxy_url) = update_proxy_url_from_env() {
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
        .build()
        .map_err(|err| format!("创建{label}客户端失败: {err}"))
}

pub(crate) fn has_explicit_update_proxy_env() -> bool {
    read_nonempty_env_value(EXPLICIT_UPDATE_PROXY_ENV_KEYS).is_some()
}

fn base_update_http_client_builder(timeout: Duration) -> reqwest::ClientBuilder {
    reqwest::Client::builder().timeout(timeout)
}

fn update_proxy_url_from_env() -> Option<String> {
    read_nonempty_env_value(UPDATE_PROXY_ENV_KEYS)
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
