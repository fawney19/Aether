use crate::handlers::admin::request::AdminAppState;
use crate::handlers::shared::mark_external_models_official_providers;
use crate::GatewayError;
use serde_json::json;
use tracing::warn;

const ADMIN_EXTERNAL_MODELS_CACHE_KEY: &str = "aether:external:models_dev";
const ADMIN_EXTERNAL_MODELS_CACHE_TTL_SECS: u64 = 15 * 60;
const ADMIN_EXTERNAL_MODELS_SOURCE_URL_ENV: &str = "AETHER_GATEWAY_EXTERNAL_MODELS_URL";
const ADMIN_EXTERNAL_MODELS_SOURCE_URL_DEFAULT: &str = "https://models.dev/api.json";
const ADMIN_EXTERNAL_MODELS_PROXY_ENV: &str = "AETHER_GATEWAY_EXTERNAL_MODELS_PROXY";

fn admin_external_models_source_url() -> String {
    std::env::var(ADMIN_EXTERNAL_MODELS_SOURCE_URL_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| ADMIN_EXTERNAL_MODELS_SOURCE_URL_DEFAULT.to_string())
}

fn admin_external_models_proxy_url() -> Option<String> {
    std::env::var(ADMIN_EXTERNAL_MODELS_PROXY_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn build_admin_external_models_proxy_client(
    proxy_url: Option<&str>,
) -> Result<Option<reqwest::Client>, GatewayError> {
    let Some(proxy_url) = proxy_url else {
        return Ok(None);
    };
    let proxy = reqwest::Proxy::all(proxy_url).map_err(|_| {
        GatewayError::Internal(format!(
            "invalid {ADMIN_EXTERNAL_MODELS_PROXY_ENV} proxy URL"
        ))
    })?;
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(300))
        .http2_adaptive_window(true)
        .use_rustls_tls()
        .no_proxy()
        .proxy(proxy)
        .build()
        .map_err(|_| {
            GatewayError::Internal(format!(
                "failed to build {ADMIN_EXTERNAL_MODELS_PROXY_ENV} proxy client"
            ))
        })?;
    Ok(Some(client))
}

fn normalize_admin_external_models_payload(payload: serde_json::Value) -> serde_json::Value {
    mark_external_models_official_providers(&payload).unwrap_or(payload)
}

async fn store_admin_external_models_cache(
    state: &AdminAppState<'_>,
    payload: &serde_json::Value,
) -> Result<(), GatewayError> {
    let serialized =
        serde_json::to_string(payload).map_err(|err| GatewayError::Internal(err.to_string()))?;
    state
        .as_ref()
        .runtime_kv_setex(
            ADMIN_EXTERNAL_MODELS_CACHE_KEY,
            &serialized,
            ADMIN_EXTERNAL_MODELS_CACHE_TTL_SECS,
        )
        .await?;
    Ok(())
}

async fn fetch_admin_external_models_from_source(
    state: &AdminAppState<'_>,
) -> Result<serde_json::Value, GatewayError> {
    let url = admin_external_models_source_url();
    let proxy_url = admin_external_models_proxy_url();
    let proxy_client = build_admin_external_models_proxy_client(proxy_url.as_deref())?;
    let using_proxy = proxy_client.is_some();
    let client = proxy_client.as_ref().unwrap_or_else(|| state.http_client());
    let response = client.get(&url).send().await.map_err(|err| {
        if using_proxy {
            GatewayError::Internal("external models proxy request failed".to_string())
        } else {
            GatewayError::Internal(err.to_string())
        }
    })?;
    let response = response
        .error_for_status()
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    let payload = response
        .json::<serde_json::Value>()
        .await
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    Ok(normalize_admin_external_models_payload(payload))
}

pub(crate) async fn read_admin_external_models_cache(
    state: &AdminAppState<'_>,
) -> Result<Option<serde_json::Value>, GatewayError> {
    if let Some(raw) = state
        .as_ref()
        .runtime_kv_get(ADMIN_EXTERNAL_MODELS_CACHE_KEY)
        .await?
    {
        match serde_json::from_str::<serde_json::Value>(&raw) {
            Ok(payload) => {
                let payload = normalize_admin_external_models_payload(payload);
                if let Err(err) = store_admin_external_models_cache(state, &payload).await {
                    warn!(error = ?err, "failed to refresh external models cache ttl");
                }
                return Ok(Some(payload));
            }
            Err(err) => {
                warn!(error = %err, "failed to parse cached external models payload");
            }
        }
    }

    match fetch_admin_external_models_from_source(state).await {
        Ok(payload) => {
            if let Err(err) = store_admin_external_models_cache(state, &payload).await {
                warn!(error = ?err, "failed to store fetched external models cache");
            }
            Ok(Some(payload))
        }
        Err(err) => {
            warn!(error = ?err, "failed to fetch external models catalog");
            Ok(None)
        }
    }
}

pub(crate) async fn clear_admin_external_models_cache(
    state: &AdminAppState<'_>,
) -> Result<serde_json::Value, GatewayError> {
    let deleted = state
        .as_ref()
        .runtime_kv_del(ADMIN_EXTERNAL_MODELS_CACHE_KEY)
        .await?;
    Ok(json!({
        "cleared": deleted,
        "message": if deleted { "缓存已清除" } else { "缓存不存在" },
    }))
}

#[cfg(test)]
mod tests {
    use super::{
        admin_external_models_source_url, build_admin_external_models_proxy_client,
        normalize_admin_external_models_payload, read_admin_external_models_cache,
        ADMIN_EXTERNAL_MODELS_SOURCE_URL_ENV,
    };
    use crate::handlers::admin::request::AdminAppState;
    use crate::tests::{start_server, AppState};
    use axum::routing::get;
    use axum::{Json, Router};
    use serde_json::json;
    use std::sync::{Mutex, MutexGuard, OnceLock};

    fn admin_external_models_env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    struct TestEnvVarGuard {
        key: &'static str,
        previous: Option<String>,
        _lock: Option<MutexGuard<'static, ()>>,
    }

    impl Drop for TestEnvVarGuard {
        fn drop(&mut self) {
            if let Some(previous) = self.previous.as_deref() {
                std::env::set_var(self.key, previous);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }

    fn set_test_env_var(key: &'static str, value: &str) -> TestEnvVarGuard {
        let lock = admin_external_models_env_lock().lock().ok();
        let previous = std::env::var(key).ok();
        std::env::set_var(key, value);
        TestEnvVarGuard {
            key,
            previous,
            _lock: lock,
        }
    }

    #[test]
    fn normalizes_external_models_payload_with_official_flags() {
        let payload = json!({
            "openai": {
                "name": "OpenAI",
                "models": {}
            },
            "openrouter": {
                "name": "OpenRouter",
                "models": {}
            }
        });

        let normalized = normalize_admin_external_models_payload(payload);

        assert_eq!(normalized["openai"]["official"], json!(true));
        assert_eq!(normalized["openrouter"]["official"], json!(false));
    }

    #[test]
    fn external_models_source_url_uses_env_override_when_present() {
        let _guard = set_test_env_var(
            ADMIN_EXTERNAL_MODELS_SOURCE_URL_ENV,
            "http://127.0.0.1:12345/api",
        );
        assert_eq!(
            admin_external_models_source_url(),
            "http://127.0.0.1:12345/api"
        );
    }

    #[tokio::test]
    async fn read_external_models_fetches_remote_payload_when_cache_missing() {
        let upstream = Router::new().route(
            "/api.json",
            get(|| async {
                Json(json!({
                    "openai": {
                        "name": "OpenAI",
                        "models": {
                            "gpt-5": {
                                "name": "GPT-5"
                            }
                        }
                    }
                }))
            }),
        );
        let (upstream_url, upstream_handle) = start_server(upstream).await;
        let _guard = set_test_env_var(
            ADMIN_EXTERNAL_MODELS_SOURCE_URL_ENV,
            &format!("{upstream_url}/api.json"),
        );

        let state = AppState::new().expect("gateway should build");
        let payload = read_admin_external_models_cache(&AdminAppState::new(&state))
            .await
            .expect("external models read should succeed")
            .expect("payload should be fetched");

        assert_eq!(payload["openai"]["official"], json!(true));
        assert_eq!(payload["openai"]["models"]["gpt-5"]["name"], json!("GPT-5"));

        upstream_handle.abort();
    }

    #[tokio::test]
    async fn external_models_proxy_client_routes_requests_through_configured_proxy() {
        let proxy = Router::new().fallback(get(|| async {
            Json(json!({
                "openai": {
                    "name": "Proxied OpenAI",
                    "models": {}
                }
            }))
        }));
        let (proxy_url, proxy_handle) = start_server(proxy).await;
        let client = build_admin_external_models_proxy_client(Some(&proxy_url))
            .expect("proxy client should build")
            .expect("proxy client should be configured");

        let payload = client
            .get("http://models.invalid/api.json")
            .send()
            .await
            .expect("request should use the configured proxy")
            .json::<serde_json::Value>()
            .await
            .expect("proxy response should be valid JSON");

        assert_eq!(payload["openai"]["name"], json!("Proxied OpenAI"));

        proxy_handle.abort();
    }
}
