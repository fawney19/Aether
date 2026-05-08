use std::collections::HashMap;

use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::json;

use super::{
    build_auth_error_response, decode_auth_token, decrypt_catalog_secret_with_fallbacks,
    query_param_value, AppState, GatewayPublicRequestContext,
};

const CLIENT_PROVISIONING_TOKEN_TYPE: &str = "client_provisioning";
const CLIENT_PROVISIONING_KV_PREFIX: &str = "client_provisioning:";
const DEFAULT_CLIENT_BASE_URL: &str = "http://localhost:8084";

#[derive(Debug, Deserialize)]
struct ClientProvisioningExchangeJsonRequest {
    token: String,
    #[serde(default)]
    base_url: Option<String>,
}

#[derive(Debug)]
struct ClientProvisioningExchangeRequest {
    token: String,
    base_url: Option<String>,
}

pub(crate) async fn maybe_build_local_client_provisioning_response(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    _headers: &http::HeaderMap,
    request_body: Option<&axum::body::Bytes>,
) -> Option<Response<Body>> {
    let decision = request_context.control_decision.as_ref()?;
    match decision.route_kind.as_deref() {
        Some("install_script") if request_context.request_path == "/install/client-config.sh" => {
            Some(build_client_config_install_script_response())
        }
        Some("exchange") if request_context.request_path == "/api/client-provisioning/exchange" => {
            Some(handle_client_provisioning_exchange(state, request_context, request_body).await)
        }
        _ => None,
    }
}

fn build_client_config_install_script_response() -> Response<Body> {
    let script = r#"#!/bin/sh
set -eu

info() { printf '%s\n' "[Aether] $*"; }
fail() { printf '%s\n' "[Aether] ERROR: $*" >&2; exit 1; }

if [ -z "${AETHER_PROVISIONING_TOKEN:-}" ]; then
  fail "AETHER_PROVISIONING_TOKEN is required. Copy the full command from Aether."
fi

AETHER_BASE_URL="${AETHER_BASE_URL:-}"
if [ -z "$AETHER_BASE_URL" ]; then
  fail "AETHER_BASE_URL is required. Copy the full command from Aether."
fi
AETHER_BASE_URL="${AETHER_BASE_URL%/}"

CONFIG_DIR="${AETHER_CLIENT_CONFIG_DIR:-$HOME/.aether}"
CONFIG_FILE="${AETHER_CLIENT_CONFIG_FILE:-$CONFIG_DIR/client.env}"
TMP_FILE="$CONFIG_FILE.tmp.$$"
EXCHANGE_URL="$AETHER_BASE_URL/api/client-provisioning/exchange?format=env"

cleanup() { rm -f "$TMP_FILE"; }
trap cleanup EXIT HUP INT TERM

command -v curl >/dev/null 2>&1 || fail "curl is required"

umask 077
mkdir -p "$CONFIG_DIR"
chmod 700 "$CONFIG_DIR" 2>/dev/null || true

HTTP_STATUS=$(curl -sS -o "$TMP_FILE" -w '%{http_code}' \
  -X POST "$EXCHANGE_URL" \
  -H 'Content-Type: application/x-www-form-urlencoded' \
  --data-urlencode "token=$AETHER_PROVISIONING_TOKEN" \
  --data-urlencode "base_url=$AETHER_BASE_URL") || fail "failed to contact Aether server"

case "$HTTP_STATUS" in
  200) ;;
  *)
    printf '%s\n' "[Aether] provisioning failed (HTTP $HTTP_STATUS):" >&2
    sed 's/.*api_key.*/[redacted]/Ig' "$TMP_FILE" >&2 || true
    exit 1
    ;;
esac

mv "$TMP_FILE" "$CONFIG_FILE"
chmod 600 "$CONFIG_FILE" 2>/dev/null || true
trap - EXIT HUP INT TERM

info "Client configuration written to $CONFIG_FILE"
info "Load it with: . $CONFIG_FILE"
info "Verify with: curl -fsS \"$AETHER_BASE_URL/v1/models\" -H \"Authorization: Bearer \\\${AETHER_API_KEY}\""
"#;
    let mut response = Response::new(Body::from(script));
    response.headers_mut().insert(
        http::header::CONTENT_TYPE,
        http::HeaderValue::from_static("application/x-sh; charset=utf-8"),
    );
    response
}

async fn handle_client_provisioning_exchange(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    request_body: Option<&axum::body::Bytes>,
) -> Response<Body> {
    let Some(request_body) = request_body else {
        return build_auth_error_response(
            http::StatusCode::BAD_REQUEST,
            "缺少 provisioning token",
            false,
        );
    };
    let request = match parse_exchange_request(request_body) {
        Ok(value) => value,
        Err(detail) => {
            return build_auth_error_response(http::StatusCode::BAD_REQUEST, detail, false);
        }
    };
    let token_payload = match decode_auth_token(&request.token, CLIENT_PROVISIONING_TOKEN_TYPE) {
        Ok(value) => value,
        Err(detail) => {
            return build_auth_error_response(
                http::StatusCode::UNAUTHORIZED,
                format!("provisioning token 无效或已过期: {detail}"),
                false,
            );
        }
    };
    let Some(jti) = token_payload.get("jti").and_then(serde_json::Value::as_str) else {
        return build_auth_error_response(
            http::StatusCode::UNAUTHORIZED,
            "provisioning token 无效",
            false,
        );
    };
    match state
        .runtime_state
        .kv_take(&format!("{CLIENT_PROVISIONING_KV_PREFIX}{jti}"))
        .await
    {
        Ok(Some(_)) => {}
        Ok(None) => {
            return build_auth_error_response(
                http::StatusCode::CONFLICT,
                "provisioning token 已被使用或已失效，请重新生成客户端配置命令",
                false,
            );
        }
        Err(err) => {
            return build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("client provisioning token consume failed: {err:?}"),
                false,
            );
        }
    }
    let Some(user_id) = token_payload
        .get("user_id")
        .and_then(serde_json::Value::as_str)
    else {
        return build_auth_error_response(
            http::StatusCode::UNAUTHORIZED,
            "provisioning token 无效",
            false,
        );
    };
    let Some(api_key_id) = token_payload
        .get("api_key_id")
        .and_then(serde_json::Value::as_str)
    else {
        return build_auth_error_response(
            http::StatusCode::UNAUTHORIZED,
            "provisioning token 无效",
            false,
        );
    };

    let records = match state
        .list_auth_api_key_export_records_by_user_ids(&[user_id.to_string()])
        .await
    {
        Ok(value) => value,
        Err(err) => {
            return build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("client provisioning api key lookup failed: {err:?}"),
                false,
            );
        }
    };
    let Some(record) = records
        .into_iter()
        .find(|record| !record.is_standalone && record.api_key_id == api_key_id)
    else {
        return build_auth_error_response(
            http::StatusCode::NOT_FOUND,
            "API密钥不存在或已失效",
            false,
        );
    };
    if !record.is_active {
        return build_auth_error_response(http::StatusCode::FORBIDDEN, "API密钥已停用", false);
    }
    let Some(ciphertext) = record
        .key_encrypted
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return build_auth_error_response(
            http::StatusCode::BAD_REQUEST,
            "该密钥没有存储完整密钥信息",
            false,
        );
    };
    let Some(api_key) = decrypt_catalog_secret_with_fallbacks(state.encryption_key(), ciphertext)
    else {
        return build_auth_error_response(
            http::StatusCode::INTERNAL_SERVER_ERROR,
            "解密密钥失败",
            false,
        );
    };
    let base_url = normalize_client_base_url(request.base_url.as_deref())
        .unwrap_or_else(|| DEFAULT_CLIENT_BASE_URL.to_string());
    let api_url = format!("{base_url}/v1");
    let format = query_param_value(request_context.request_query_string.as_deref(), "format")
        .unwrap_or_default();

    if format.eq_ignore_ascii_case("env") {
        return build_client_env_response(&base_url, &api_url, &api_key);
    }

    no_store_response(Json(json!({
        "base_url": base_url,
        "api_url": api_url,
        "api_key": api_key,
        "config_path": "~/.aether/client.env",
        "message": "客户端配置凭证已签发，请妥善保存本地配置文件",
        "verification": format!("curl -fsS {base_url}/v1/models -H 'Authorization: Bearer $AETHER_API_KEY'"),
    }))
    .into_response())
}

fn parse_exchange_request(
    request_body: &[u8],
) -> Result<ClientProvisioningExchangeRequest, String> {
    if let Ok(payload) =
        serde_json::from_slice::<ClientProvisioningExchangeJsonRequest>(request_body)
    {
        return normalize_exchange_request(payload.token, payload.base_url);
    }
    let decoded = url::form_urlencoded::parse(request_body)
        .into_owned()
        .collect::<HashMap<_, _>>();
    normalize_exchange_request(
        decoded.get("token").cloned().unwrap_or_default(),
        decoded.get("base_url").cloned(),
    )
}

fn normalize_exchange_request(
    token: String,
    base_url: Option<String>,
) -> Result<ClientProvisioningExchangeRequest, String> {
    let token = token.trim().to_string();
    if token.is_empty() {
        return Err("缺少 provisioning token".to_string());
    }
    Ok(ClientProvisioningExchangeRequest { token, base_url })
}

fn normalize_client_base_url(value: Option<&str>) -> Option<String> {
    let value = value?.trim().trim_end_matches('/');
    if value.is_empty() {
        return None;
    }
    let parsed = url::Url::parse(value).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return None;
    }
    Some(value.to_string())
}

fn build_client_env_response(base_url: &str, api_url: &str, api_key: &str) -> Response<Body> {
    let body = format!(
        "# Aether client configuration\nAETHER_BASE_URL={}\nAETHER_API_URL={}\nAETHER_API_KEY={}\nOPENAI_BASE_URL={}\nOPENAI_API_KEY={}\n",
        shell_quote(base_url),
        shell_quote(api_url),
        shell_quote(api_key),
        shell_quote(api_url),
        shell_quote(api_key),
    );
    let mut response = Response::new(Body::from(body));
    response.headers_mut().insert(
        http::header::CONTENT_TYPE,
        http::HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    no_store_response(response)
}

fn no_store_response(mut response: Response<Body>) -> Response<Body> {
    response.headers_mut().insert(
        http::header::CACHE_CONTROL,
        http::HeaderValue::from_static("no-store"),
    );
    response.headers_mut().insert(
        http::header::PRAGMA,
        http::HeaderValue::from_static("no-cache"),
    );
    response
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_response_marks_credentials_no_store_and_quotes_values() {
        let response = build_client_env_response(
            "https://aether.example",
            "https://aether.example/v1",
            "sk-test'value",
        );

        assert_eq!(
            response
                .headers()
                .get(http::header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok()),
            Some("no-store")
        );
        assert_eq!(
            response
                .headers()
                .get(http::header::PRAGMA)
                .and_then(|value| value.to_str().ok()),
            Some("no-cache")
        );
    }

    #[test]
    fn normalizes_only_http_client_base_urls() {
        assert_eq!(
            normalize_client_base_url(Some("https://aether.example/")),
            Some("https://aether.example".to_string())
        );
        assert!(normalize_client_base_url(Some("file:///tmp/aether")).is_none());
    }
}
