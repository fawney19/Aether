use serde_json::{json, Map, Value};

const REDACTED_VALUE: &str = "***";
const REDACTED_UPSTREAM_DIAGNOSTIC: &str = "[REDACTED upstream diagnostic]";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdminProxyCredentialField {
    Username,
    Password,
}

impl AdminProxyCredentialField {
    fn canonical_key(self) -> &'static str {
        match self {
            Self::Username => "username",
            Self::Password => "password",
        }
    }
}

/// Classifies the proxy credential spellings accepted by the catalog normalizer.
/// Keep this shared with the admin redactor so an accepted alias can never bypass
/// masking before it is migrated to the canonical encrypted field.
pub fn admin_proxy_credential_field(key: &str) -> Option<AdminProxyCredentialField> {
    proxy_credential_field_from_compact(&compact_json_key(key))
}

/// Returns whether the generic admin redactor treats this JSON field as secret.
/// Catalog persistence uses the same predicate to reject unsupported secret shapes.
pub fn admin_json_field_is_sensitive(key: &str, value: &Value) -> bool {
    json_secret_key(&compact_json_key(key), value)
}

/// Detects secrets that the admin projection hides because of surrounding JSON
/// semantics rather than the field name alone (for example URL userinfo/query,
/// header maps, mutation rules, or a nested proxy object).
pub fn admin_json_field_has_contextual_secrets(key: &str, value: &Value) -> bool {
    let compact_key = compact_json_key(key);
    if json_url_key(&compact_key) {
        return value
            .as_str()
            .is_some_and(network_url_has_hidden_components);
    }
    if compact_key == "proxy" {
        return admin_secret_safe_proxy(Some(value)) != *value;
    }
    if header_rules_key(&compact_key) {
        return admin_secret_safe_header_rules(Some(value)) != *value;
    }
    if body_rules_key(&compact_key) {
        return admin_secret_safe_body_rules(Some(value)) != *value;
    }
    if header_values_key(&compact_key) {
        return value
            .as_object()
            .is_some_and(|headers| headers.values().any(secret_value_is_set));
    }
    false
}

#[derive(Clone, Copy)]
enum RuleKind {
    Header,
    Body,
}

pub fn admin_secret_safe_json(value: Option<&Value>) -> Value {
    value.map(redact_json_value).unwrap_or(Value::Null)
}

pub fn admin_provider_oauth_invalid_reason_safe_text(reason: Option<&str>) -> Option<String> {
    let reason = reason.map(str::trim).filter(|value| !value.is_empty())?;
    let lowered = reason.to_ascii_lowercase();

    if tagged_diagnostic_reason(reason, "[ACCOUNT_BLOCK]") {
        return Some(format!(
            "[ACCOUNT_BLOCK] {}",
            canonical_account_block_reason(&lowered)
        ));
    }
    if tagged_diagnostic_reason(reason, "[OAUTH_EXPIRED]") {
        let detail = if diagnostic_reason_is_hard_token_invalid(&lowered) {
            "OAuth token is invalid"
        } else {
            "OAuth token has expired"
        };
        return Some(format!("[OAUTH_EXPIRED] {detail}"));
    }
    if tagged_diagnostic_reason(reason, "[REFRESH_FAILED]") {
        return Some("[REFRESH_FAILED] OAuth token refresh failed".to_string());
    }
    if tagged_diagnostic_reason(reason, "[REQUEST_FAILED]") {
        let detail = if lowered.contains("agent runtime has been deleted") {
            "Agent runtime has been deleted"
        } else {
            "OAuth account status request failed"
        };
        return Some(format!("[REQUEST_FAILED] {detail}"));
    }

    Some(
        if diagnostic_reason_is_hard_token_invalid(&lowered) {
            "OAuth token is invalid"
        } else if diagnostic_reason_is_token_expired(&lowered) {
            "OAuth token has expired"
        } else if diagnostic_reason_is_account_blocked(&lowered) {
            canonical_account_block_reason(&lowered)
        } else {
            "OAuth credential is unavailable"
        }
        .to_string(),
    )
}

pub fn admin_provider_upstream_metadata_safe_json(value: Option<&Value>) -> Value {
    let mut projected = admin_secret_safe_json(value);
    let Some(root) = projected.as_object_mut() else {
        return projected;
    };

    if let Some(chatgpt_web) = root.get_mut("chatgpt_web") {
        *chatgpt_web = project_chatgpt_web_metadata(chatgpt_web);
    }
    redact_upstream_diagnostic_fields(&mut projected);
    projected
}

pub fn admin_provider_metadata_bucket_safe_json(
    provider_type: &str,
    value: Option<&Value>,
) -> Value {
    let provider_type = provider_type.trim().to_ascii_lowercase();
    let Some(value) = value else {
        return Value::Null;
    };
    if provider_type.is_empty() {
        let mut projected = admin_secret_safe_json(Some(value));
        redact_upstream_diagnostic_fields(&mut projected);
        return projected;
    }

    let mut root = Map::new();
    root.insert(provider_type.clone(), value.clone());
    let mut projected = admin_provider_upstream_metadata_safe_json(Some(&Value::Object(root)));
    projected
        .as_object_mut()
        .and_then(|object| object.remove(&provider_type))
        .unwrap_or(Value::Null)
}

pub fn admin_provider_status_snapshot_safe_json(value: Option<&Value>) -> Value {
    let Some(snapshot) = value.and_then(Value::as_object) else {
        return Value::Null;
    };

    let mut projected = Map::new();
    projected.insert(
        "oauth".to_string(),
        project_oauth_status_snapshot(snapshot.get("oauth")),
    );
    projected.insert(
        "account".to_string(),
        project_account_status_snapshot(snapshot.get("account")),
    );
    projected.insert(
        "quota".to_string(),
        project_quota_status_snapshot(snapshot.get("quota")),
    );
    Value::Object(projected)
}

pub fn admin_restore_secret_safe_json(existing: Option<&Value>, incoming: &Value) -> Value {
    restore_json_value(existing, incoming, None)
}

pub fn admin_secret_safe_header_rules(rules: Option<&Value>) -> Value {
    redact_rule_array(rules, redact_header_rule)
}

pub fn admin_restore_secret_safe_header_rules(existing: Option<&Value>, incoming: &Value) -> Value {
    restore_rule_array(existing, incoming, RuleKind::Header)
}

pub fn admin_secret_safe_body_rules(rules: Option<&Value>) -> Value {
    redact_rule_array(rules, redact_body_rule)
}

pub fn admin_restore_secret_safe_body_rules(existing: Option<&Value>, incoming: &Value) -> Value {
    restore_rule_array(existing, incoming, RuleKind::Body)
}

pub fn admin_validate_retained_header_rule_secrets(
    existing: Option<&Value>,
    incoming: &Value,
) -> Result<(), String> {
    validate_retained_rule_secrets(existing, incoming, RuleKind::Header)
}

pub fn admin_validate_retained_body_rule_secrets(
    existing: Option<&Value>,
    incoming: &Value,
) -> Result<(), String> {
    validate_retained_rule_secrets(existing, incoming, RuleKind::Body)
}

pub fn admin_secret_safe_url(value: Option<&str>) -> Value {
    value
        .and_then(sanitize_network_url)
        .map(Value::String)
        .unwrap_or(Value::Null)
}

pub fn admin_restore_secret_safe_url(existing: Option<&str>, incoming: &str) -> String {
    if existing.and_then(sanitize_network_url).as_deref() == Some(incoming.trim()) {
        return existing.unwrap_or(incoming).to_string();
    }
    incoming.to_string()
}

pub fn admin_secret_safe_proxy(proxy: Option<&Value>) -> Value {
    let Some(proxy) = proxy else {
        return Value::Null;
    };
    if let Some(proxy_url) = proxy.as_str() {
        return admin_secret_safe_url(Some(proxy_url));
    }
    let Some(proxy) = proxy.as_object() else {
        return Value::Null;
    };

    let mut projected = Map::new();
    let mut has_credentials = false;
    for (key, value) in proxy {
        let compact_key = compact_json_key(key);
        if compact_key == "hascredentials" {
            continue;
        }
        if matches!(compact_key.as_str(), "url" | "proxyurl") {
            let sanitized = value
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .and_then(|value| {
                    has_credentials |= network_url_has_hidden_components(value);
                    sanitize_network_url(value).map(Value::String)
                })
                .unwrap_or(Value::Null);
            projected.insert(key.clone(), sanitized);
            continue;
        }
        if proxy_credential_key(&compact_key) {
            has_credentials |= proxy_secret_value_is_set(value);
            projected.insert(key.clone(), redact_proxy_secret_value(value));
            continue;
        }
        projected.insert(key.clone(), redact_proxy_json_value_for_key(key, value));
    }
    if has_credentials {
        projected.insert("has_credentials".to_string(), Value::Bool(true));
    }

    Value::Object(projected)
}

pub fn admin_restore_secret_safe_proxy(existing: Option<&Value>, incoming: &Value) -> Value {
    if let Some(incoming_url) = incoming.as_str() {
        if admin_proxy_identities_match(existing, incoming) {
            return existing
                .and_then(proxy_url_value)
                .map(|existing_url| {
                    Value::String(admin_restore_secret_safe_url(
                        Some(existing_url),
                        incoming_url,
                    ))
                })
                .unwrap_or_else(|| Value::String(incoming_url.to_string()));
        }
        return Value::String(incoming_url.to_string());
    }

    let Some(incoming_object) = incoming.as_object() else {
        return incoming.clone();
    };
    let existing_object = existing.and_then(Value::as_object);
    let identities_match = admin_proxy_identities_match(existing, incoming);
    let mut restored = Map::new();
    for (key, incoming_value) in incoming_object {
        let compact_key = compact_json_key(key);
        if compact_key == "hascredentials" {
            continue;
        }
        if let Some(field) = admin_proxy_credential_field(key) {
            if incoming_value.as_str() == Some(REDACTED_VALUE) {
                if identities_match {
                    if let Some(value) = unambiguous_proxy_credential(existing, field) {
                        if proxy_secret_value_is_set(&value) {
                            restored.insert(key.clone(), value);
                        }
                    }
                }
                continue;
            }
        }
        let existing_value = existing_object.and_then(|object| object.get(key));
        if let Some(value) =
            restore_proxy_json_value(existing_value, incoming_value, Some(key), identities_match)
        {
            restored.insert(key.clone(), value);
        }
    }

    // Secret-safe admin projections normally contain masks, but partial clients may
    // omit those fields. Preserve omitted credentials only while the normalized proxy
    // authority and node identity are unchanged. Any identity change therefore needs
    // explicit new credentials.
    if identities_match {
        for field in [
            AdminProxyCredentialField::Username,
            AdminProxyCredentialField::Password,
        ] {
            if proxy_contains_credential_field(incoming, field) {
                continue;
            }
            if let Some(existing_value) = unambiguous_proxy_credential(existing, field) {
                if proxy_secret_value_is_set(&existing_value) {
                    restored.insert(field.canonical_key().to_string(), existing_value);
                }
            }
        }
    }
    Value::Object(restored)
}

fn restore_proxy_json_value(
    existing: Option<&Value>,
    incoming: &Value,
    key: Option<&str>,
    identities_match: bool,
) -> Option<Value> {
    if let Some(key) = key {
        let compact_key = compact_json_key(key);
        if matches!(compact_key.as_str(), "url" | "proxyurl") {
            return Some(
                incoming
                    .as_str()
                    .map(|incoming_url| {
                        if identities_match {
                            restore_url_value(existing, incoming_url)
                        } else {
                            Value::String(incoming_url.to_string())
                        }
                    })
                    .unwrap_or_else(|| incoming.clone()),
            );
        }
        if proxy_credential_key(&compact_key) {
            if incoming.as_str() == Some(REDACTED_VALUE) {
                return identities_match
                    .then(|| {
                        existing
                            .filter(|value| proxy_secret_value_is_set(value))
                            .cloned()
                    })
                    .flatten();
            }
            return Some(incoming.clone());
        }
    }

    match incoming {
        Value::Array(incoming_values) => {
            let existing_values = existing.and_then(Value::as_array);
            Some(Value::Array(
                incoming_values
                    .iter()
                    .enumerate()
                    .filter_map(|(index, incoming_value)| {
                        restore_proxy_json_value(
                            existing_values.and_then(|values| values.get(index)),
                            incoming_value,
                            None,
                            identities_match,
                        )
                    })
                    .collect(),
            ))
        }
        Value::Object(incoming_object) => {
            let existing_object = existing.and_then(Value::as_object);
            Some(Value::Object(
                incoming_object
                    .iter()
                    .filter(|(key, _)| compact_json_key(key) != "hascredentials")
                    .filter_map(|(key, incoming_value)| {
                        restore_proxy_json_value(
                            existing_object.and_then(|object| object.get(key)),
                            incoming_value,
                            Some(key),
                            identities_match,
                        )
                        .map(|value| (key.clone(), value))
                    })
                    .collect(),
            ))
        }
        _ => Some(incoming.clone()),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AdminProxyIdentity {
    url: Option<(String, String, u16)>,
    node_id: Option<String>,
}

fn admin_proxy_identities_match(existing: Option<&Value>, incoming: &Value) -> bool {
    existing
        .and_then(admin_proxy_identity)
        .zip(admin_proxy_identity(incoming))
        .is_some_and(|(existing, incoming)| {
            existing == incoming && (existing.url.is_some() || existing.node_id.is_some())
        })
}

fn admin_proxy_identity(value: &Value) -> Option<AdminProxyIdentity> {
    if let Some(url) = value.as_str() {
        return normalized_proxy_url_identity(url).map(|url| AdminProxyIdentity {
            url: Some(url),
            node_id: None,
        });
    }
    let object = value.as_object()?;

    let mut url_field_seen = false;
    let mut url = None;
    let mut node_field_seen = false;
    let mut node_id = None;
    for (key, value) in object {
        match compact_json_key(key).as_str() {
            "url" | "proxyurl" => {
                let candidate = if value.is_null() {
                    None
                } else {
                    Some(normalized_proxy_url_identity(value.as_str()?)?)
                };
                if url_field_seen && url != candidate {
                    return None;
                }
                url_field_seen = true;
                url = candidate;
            }
            "nodeid" => {
                let candidate = if value.is_null() {
                    None
                } else {
                    value
                        .as_str()
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(ToOwned::to_owned)
                };
                if !value.is_null() && candidate.is_none() {
                    return None;
                }
                if node_field_seen && node_id != candidate {
                    return None;
                }
                node_field_seen = true;
                node_id = candidate;
            }
            _ => {}
        }
    }
    Some(AdminProxyIdentity { url, node_id })
}

fn normalized_proxy_url_identity(value: &str) -> Option<(String, String, u16)> {
    let parsed = url::Url::parse(value.trim()).ok()?;
    let scheme = parsed.scheme().to_ascii_lowercase();
    if !matches!(scheme.as_str(), "http" | "https" | "socks5" | "socks5h") {
        return None;
    }
    let host = parsed.host_str()?.to_ascii_lowercase();
    let port = parsed.port().or(match scheme.as_str() {
        "http" => Some(80),
        "https" => Some(443),
        "socks5" | "socks5h" => Some(1080),
        _ => None,
    })?;
    Some((scheme, host, port))
}

fn proxy_url_value(value: &Value) -> Option<&str> {
    if let Some(url) = value.as_str() {
        return Some(url);
    }
    value.as_object()?.iter().find_map(|(key, value)| {
        matches!(compact_json_key(key).as_str(), "url" | "proxyurl")
            .then(|| value.as_str())
            .flatten()
    })
}

fn proxy_contains_credential_field(value: &Value, target: AdminProxyCredentialField) -> bool {
    match value {
        Value::Array(values) => values
            .iter()
            .any(|value| proxy_contains_credential_field(value, target)),
        Value::Object(object) => object.iter().any(|(key, value)| {
            admin_proxy_credential_field(key) == Some(target)
                || proxy_contains_credential_field(value, target)
        }),
        _ => false,
    }
}

fn unambiguous_proxy_credential(
    value: Option<&Value>,
    target: AdminProxyCredentialField,
) -> Option<Value> {
    fn collect(value: &Value, target: AdminProxyCredentialField, values: &mut Vec<Value>) {
        match value {
            Value::Array(items) => {
                for item in items {
                    collect(item, target, values);
                }
            }
            Value::Object(object) => {
                for (key, value) in object {
                    if admin_proxy_credential_field(key) == Some(target) {
                        values.push(value.clone());
                    } else {
                        collect(value, target, values);
                    }
                }
            }
            _ => {}
        }
    }

    let mut values = Vec::new();
    collect(value?, target, &mut values);
    let first = values.first()?.clone();
    values.iter().all(|value| value == &first).then_some(first)
}

fn tagged_diagnostic_reason(reason: &str, tag: &str) -> bool {
    reason
        .lines()
        .map(str::trim)
        .any(|line| line.starts_with(tag))
}

fn diagnostic_reason_is_hard_token_invalid(lowered: &str) -> bool {
    [
        "token invalid",
        "token_invalid",
        "token has been invalidated",
        "token invalidated",
        "token revoked",
        "personal access token owner is inactive",
        "auth_credential",
        "invalid token",
        "token 无效",
        "token 失效",
        "令牌无效",
        "令牌失效",
    ]
    .iter()
    .any(|keyword| lowered.contains(keyword))
}

fn diagnostic_reason_is_token_expired(lowered: &str) -> bool {
    [
        "token expired",
        "token has expired",
        "session expired",
        "access token expired",
        "oauth_token_expired",
        "token 过期",
        "令牌过期",
        "已过期",
    ]
    .iter()
    .any(|keyword| lowered.contains(keyword))
}

fn diagnostic_reason_is_account_blocked(lowered: &str) -> bool {
    [
        "banned",
        "blocked",
        "suspended",
        "forbidden",
        "disabled",
        "deactivated",
        "validation_required",
        "verify your account",
        "封禁",
        "停用",
        "受限",
        "验证",
    ]
    .iter()
    .any(|keyword| lowered.contains(keyword))
}

fn canonical_account_block_reason(lowered: &str) -> &'static str {
    if lowered.contains("deactivated_workspace") || lowered.contains("workspace deactivated") {
        "Workspace is deactivated"
    } else if diagnostic_reason_is_hard_token_invalid(lowered) {
        "OAuth token is invalid"
    } else if diagnostic_reason_is_token_expired(lowered) {
        "OAuth token has expired"
    } else if lowered.contains("validation_required") || lowered.contains("verify your account") {
        "Account verification is required"
    } else if lowered.contains("disabled") || lowered.contains("account has been deactivated") {
        "Account is disabled"
    } else if lowered.contains("banned") || lowered.contains("suspended") {
        "Account is suspended"
    } else {
        "Account access is restricted"
    }
}

fn project_chatgpt_web_metadata(value: &Value) -> Value {
    let Some(source) = value.as_object() else {
        return Value::Null;
    };
    let mut projected = Map::new();

    for field in [
        "updated_at",
        "image_quota_remaining",
        "image_quota_total",
        "image_quota_used",
        "image_quota_reset_at",
        "image_quota_last_local_request_at",
        "image_quota_local_request_count",
    ] {
        copy_json_number_or_null(source, &mut projected, field);
    }
    // This is an opaque idempotency key, not a diagnostic or credential. Keep it
    // bounded and token-safe so quota request de-duplication survives persistence.
    copy_safe_token_string_or_null(source, &mut projected, "image_quota_last_local_request_key");
    copy_json_bool_or_null(source, &mut projected, "image_quota_blocked");
    for field in [
        "default_model_slug",
        "plan_type",
        "email",
        "account_id",
        "account_user_id",
        "user_id",
        "image_quota_limit_source",
    ] {
        copy_safe_display_string_or_null(source, &mut projected, field);
    }

    let image_blocked = source
        .get("image_quota_blocked")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || source
            .get("blocked_features")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .any(chatgpt_web_is_image_feature);
    if image_blocked {
        projected.insert("image_quota_blocked".to_string(), Value::Bool(true));
        projected.insert("blocked_features".to_string(), json!(["image_generation"]));
    }
    if source
        .get("image_quota_feature_name")
        .and_then(Value::as_str)
        .is_some_and(chatgpt_web_is_image_feature)
    {
        projected.insert(
            "image_quota_feature_name".to_string(),
            Value::String("image_generation".to_string()),
        );
    }

    Value::Object(projected)
}

fn chatgpt_web_is_image_feature(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "image_gen" | "image_generation" | "image_edit" | "img_gen"
    )
}

fn redact_upstream_diagnostic_fields(value: &mut Value) {
    match value {
        Value::Array(values) => {
            for value in values {
                redact_upstream_diagnostic_fields(value);
            }
        }
        Value::Object(object) => {
            for (key, value) in object {
                if upstream_diagnostic_key(key) && diagnostic_value_has_content(value) {
                    *value = Value::String(REDACTED_UPSTREAM_DIAGNOSTIC.to_string());
                } else {
                    redact_upstream_diagnostic_fields(value);
                }
            }
        }
        _ => {}
    }
}

fn upstream_diagnostic_key(key: &str) -> bool {
    let compact = compact_json_key(key);
    matches!(
        compact.as_str(),
        "bodytext"
            | "detail"
            | "details"
            | "error"
            | "errors"
            | "rawbody"
            | "requestbody"
            | "responsebody"
    ) || compact.ends_with("error")
        || compact.ends_with("message")
        || compact.ends_with("reason")
}

fn diagnostic_value_has_content(value: &Value) -> bool {
    match value {
        Value::String(value) => !value.trim().is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
        _ => false,
    }
}

fn project_oauth_status_snapshot(value: Option<&Value>) -> Value {
    let source = value.and_then(Value::as_object);
    let code = source
        .and_then(|source| source.get("code"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|code| {
            matches!(
                *code,
                "none"
                    | "valid"
                    | "expiring"
                    | "expired"
                    | "invalid"
                    | "reauth_required"
                    | "check_failed"
            )
        })
        .unwrap_or("none");
    let mut projected = Map::new();
    projected.insert("code".to_string(), json!(code));
    projected.insert(
        "label".to_string(),
        optional_static_text(oauth_status_label(code)),
    );
    projected.insert(
        "reason".to_string(),
        optional_static_text(oauth_status_reason(code)),
    );
    if let Some(source) = source {
        for field in ["expires_at", "invalid_at"] {
            copy_json_number_or_null(source, &mut projected, field);
        }
        for field in ["requires_reauth", "usable_until_expiry", "expiring_soon"] {
            copy_json_bool_or_null(source, &mut projected, field);
        }
        copy_safe_token_string_or_null(source, &mut projected, "source");
    }
    Value::Object(projected)
}

fn oauth_status_label(code: &str) -> Option<&'static str> {
    match code {
        "valid" => Some("有效"),
        "expiring" => Some("即将过期"),
        "expired" => Some("已过期"),
        "invalid" => Some("已失效"),
        "reauth_required" => Some("续期失败"),
        "check_failed" => Some("检查失败"),
        _ => None,
    }
}

fn oauth_status_reason(code: &str) -> Option<&'static str> {
    match code {
        "expired" => Some("OAuth token has expired"),
        "invalid" => Some("OAuth token is invalid"),
        "reauth_required" => Some("OAuth token refresh failed"),
        "check_failed" => Some("OAuth account status request failed"),
        _ => None,
    }
}

fn project_account_status_snapshot(value: Option<&Value>) -> Value {
    let source = value.and_then(Value::as_object);
    let blocked = source
        .and_then(|source| source.get("blocked"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut code = source
        .and_then(|source| source.get("code"))
        .and_then(Value::as_str)
        .and_then(safe_status_token)
        .unwrap_or(if blocked { "account_blocked" } else { "ok" });
    if code == "ok" && blocked {
        code = "account_blocked";
    }

    let mut projected = Map::new();
    projected.insert("code".to_string(), json!(code));
    projected.insert(
        "label".to_string(),
        optional_static_text(account_status_label(code)),
    );
    projected.insert(
        "reason".to_string(),
        optional_static_text(account_status_reason(code, blocked)),
    );
    projected.insert("blocked".to_string(), Value::Bool(blocked));
    if let Some(source) = source {
        copy_safe_token_string_or_null(source, &mut projected, "source");
        copy_json_bool_or_null(source, &mut projected, "recoverable");
    }
    Value::Object(projected)
}

fn account_status_label(code: &str) -> Option<&'static str> {
    match code {
        "account_banned" | "account_suspended" => Some("账号封禁"),
        "account_quarantined" => Some("账号隔离"),
        "workspace_deactivated" => Some("工作区停用"),
        "account_disabled" => Some("账号停用"),
        "account_forbidden" => Some("访问受限"),
        "account_blocked" => Some("账号异常"),
        "account_verification" => Some("需要验证"),
        "oauth_token_invalid" => Some("Token 失效"),
        "oauth_token_expired" => Some("Token 过期"),
        "oauth_request_failed" => Some("请求失败"),
        _ => None,
    }
}

fn account_status_reason(code: &str, blocked: bool) -> Option<&'static str> {
    match code {
        "account_banned" | "account_suspended" => Some("Account is suspended"),
        "account_quarantined" => Some("Account is quarantined"),
        "workspace_deactivated" => Some("Workspace is deactivated"),
        "account_disabled" => Some("Account is disabled"),
        "account_forbidden" => Some("Account access is restricted"),
        "account_verification" => Some("Account verification is required"),
        "oauth_token_invalid" => Some("OAuth token is invalid"),
        "oauth_token_expired" => Some("OAuth token has expired"),
        "oauth_request_failed" => Some("OAuth account status request failed"),
        "account_blocked" => Some("Account is unavailable"),
        _ if blocked => Some("Account is unavailable"),
        _ => None,
    }
}

fn project_quota_status_snapshot(value: Option<&Value>) -> Value {
    let source = value.and_then(Value::as_object);
    let exhausted = source
        .and_then(|source| source.get("exhausted"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let code = source
        .and_then(|source| source.get("code"))
        .and_then(Value::as_str)
        .and_then(safe_status_token)
        .unwrap_or(if exhausted { "exhausted" } else { "unknown" });

    let mut projected = Map::new();
    projected.insert("code".to_string(), json!(code));
    projected.insert(
        "label".to_string(),
        optional_static_text(quota_status_label(code)),
    );
    projected.insert(
        "reason".to_string(),
        optional_static_text(quota_status_reason(code, exhausted)),
    );
    projected.insert("exhausted".to_string(), Value::Bool(exhausted));

    let Some(source) = source else {
        return Value::Object(projected);
    };
    for field in [
        "version",
        "observed_at",
        "usage_ratio",
        "updated_at",
        "reset_at",
        "reset_seconds",
        "allowed_models_count",
    ] {
        copy_json_number_or_null(source, &mut projected, field);
    }
    for field in ["allowed", "limit_reached"] {
        copy_json_bool_or_null(source, &mut projected, field);
    }
    for field in [
        "provider_type",
        "freshness",
        "source",
        "plan_type",
        "pool_tier",
    ] {
        copy_safe_token_string_or_null(source, &mut projected, field);
    }
    if let Some(credits) = source.get("credits") {
        projected.insert("credits".to_string(), project_quota_credits(credits));
    }
    if let Some(reset_credits) = source.get("reset_credits") {
        projected.insert(
            "reset_credits".to_string(),
            project_quota_reset_credits(reset_credits),
        );
    }
    if let Some(rate_limit) = source.get("rate_limit") {
        projected.insert(
            "rate_limit".to_string(),
            project_quota_rate_limit(rate_limit),
        );
    }
    if let Some(windows) = source.get("windows").and_then(Value::as_array) {
        projected.insert(
            "windows".to_string(),
            Value::Array(windows.iter().filter_map(project_quota_window).collect()),
        );
    }
    Value::Object(projected)
}

fn quota_status_label(code: &str) -> Option<&'static str> {
    match code {
        "banned" => Some("账号已封禁"),
        "quarantined" => Some("账号隔离中"),
        "forbidden" => Some("访问受限"),
        "exhausted" => Some("额度耗尽"),
        "cooldown" => Some("冷却中"),
        "error" => Some("刷新失败"),
        _ => None,
    }
}

fn quota_status_reason(code: &str, exhausted: bool) -> Option<&'static str> {
    match code {
        "banned" => Some("Account is suspended"),
        "quarantined" => Some("Account is quarantined"),
        "forbidden" => Some("Account access is restricted"),
        "exhausted" => Some("Quota is exhausted"),
        "cooldown" => Some("Quota is temporarily cooling down"),
        "error" => Some("Quota refresh failed"),
        _ if exhausted => Some("Quota is exhausted"),
        _ => None,
    }
}

fn project_quota_credits(value: &Value) -> Value {
    let Some(source) = value.as_object() else {
        return Value::Null;
    };
    let mut projected = Map::new();
    for field in ["has_credits", "unlimited"] {
        copy_json_bool_or_null(source, &mut projected, field);
    }
    for field in ["balance", "remaining", "consumed", "total", "updated_at"] {
        copy_json_number_or_null(source, &mut projected, field);
    }
    Value::Object(projected)
}

fn project_quota_reset_credits(value: &Value) -> Value {
    let Some(source) = value.as_object() else {
        return Value::Null;
    };
    let mut projected = Map::new();
    for field in ["available_count", "updated_at"] {
        copy_json_number_or_null(source, &mut projected, field);
    }
    for field in ["detail_source", "detail_status"] {
        copy_safe_token_string_or_null(source, &mut projected, field);
    }
    if let Some(detail_error) = source
        .get("detail_error")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        projected.insert(
            "detail_error".to_string(),
            Value::String(canonical_quota_detail_error(detail_error).to_string()),
        );
    }
    if let Some(credits) = source.get("credits").and_then(Value::as_array) {
        projected.insert(
            "credits".to_string(),
            Value::Array(
                credits
                    .iter()
                    .filter_map(project_quota_reset_credit)
                    .collect(),
            ),
        );
    }
    Value::Object(projected)
}

fn canonical_quota_detail_error(value: &str) -> &'static str {
    let lowered = value.to_ascii_lowercase();
    if lowered.contains("timeout") || lowered.contains("timed out") {
        "Quota refresh timed out"
    } else if ["connect", "connection", "dns", "tls", "certificate"]
        .iter()
        .any(|keyword| lowered.contains(keyword))
    {
        "Quota refresh connection failed"
    } else {
        "Quota refresh failed"
    }
}

fn project_quota_reset_credit(value: &Value) -> Option<Value> {
    let source = value.as_object()?;
    let mut projected = Map::new();
    for field in ["id", "display_key", "status"] {
        copy_safe_display_string_or_null(source, &mut projected, field);
    }
    for field in ["granted_at", "expires_at", "remaining_seconds"] {
        copy_json_number_or_null(source, &mut projected, field);
    }
    (!projected.is_empty()).then_some(Value::Object(projected))
}

fn project_quota_rate_limit(value: &Value) -> Value {
    let Some(source) = value.as_object() else {
        return Value::Null;
    };
    let mut projected = Map::new();
    for field in ["limited", "has_capacity"] {
        copy_json_bool_or_null(source, &mut projected, field);
    }
    for field in [
        "messages_remaining",
        "max_messages",
        "retry_after_ms",
        "reset_at",
        "reset_seconds",
        "updated_at",
    ] {
        copy_json_number_or_null(source, &mut projected, field);
    }
    Value::Object(projected)
}

fn project_quota_window(value: &Value) -> Option<Value> {
    let source = value.as_object()?;
    let code = source
        .get("code")
        .and_then(Value::as_str)
        .and_then(safe_status_token)?;
    let mut projected = Map::new();
    projected.insert("code".to_string(), json!(code));
    for field in ["label", "model"] {
        copy_safe_display_string_or_null(source, &mut projected, field);
    }
    for field in ["scope", "unit", "window", "quota_group", "bucket_id"] {
        copy_safe_token_string_or_null(source, &mut projected, field);
    }
    for field in ["quota_group_label", "description"] {
        copy_safe_display_string_or_null(source, &mut projected, field);
    }
    for field in [
        "used_ratio",
        "remaining_ratio",
        "used_value",
        "remaining_value",
        "limit_value",
        "reset_at",
        "reset_seconds",
        "window_minutes",
        "usage_reset_at",
    ] {
        copy_json_number_or_null(source, &mut projected, field);
    }
    copy_json_bool_or_null(source, &mut projected, "is_exhausted");
    if let Some(usage) = source.get("usage") {
        projected.insert("usage".to_string(), project_quota_window_usage(usage));
    }
    Some(Value::Object(projected))
}

fn project_quota_window_usage(value: &Value) -> Value {
    let Some(source) = value.as_object() else {
        return Value::Null;
    };
    let mut projected = Map::new();
    for field in ["request_count", "total_tokens"] {
        copy_json_number_or_null(source, &mut projected, field);
    }
    if let Some(value) = source.get("total_cost_usd") {
        let safe = value.is_number()
            || value.is_null()
            || value
                .as_str()
                .is_some_and(|value| value.trim().parse::<f64>().is_ok());
        if safe {
            projected.insert("total_cost_usd".to_string(), value.clone());
        }
    }
    Value::Object(projected)
}

fn optional_static_text(value: Option<&'static str>) -> Value {
    value
        .map(|value| Value::String(value.to_string()))
        .unwrap_or(Value::Null)
}

fn copy_json_number_or_null(
    source: &Map<String, Value>,
    target: &mut Map<String, Value>,
    key: &str,
) {
    if let Some(value) = source
        .get(key)
        .filter(|value| value.is_number() || value.is_null())
    {
        target.insert(key.to_string(), value.clone());
    }
}

fn copy_json_bool_or_null(source: &Map<String, Value>, target: &mut Map<String, Value>, key: &str) {
    if let Some(value) = source
        .get(key)
        .filter(|value| value.is_boolean() || value.is_null())
    {
        target.insert(key.to_string(), value.clone());
    }
}

fn copy_safe_token_string_or_null(
    source: &Map<String, Value>,
    target: &mut Map<String, Value>,
    key: &str,
) {
    let Some(value) = source.get(key) else {
        return;
    };
    if value.is_null() {
        target.insert(key.to_string(), Value::Null);
    } else if let Some(value) = value.as_str().and_then(safe_status_token) {
        target.insert(key.to_string(), Value::String(value.to_string()));
    }
}

fn copy_safe_display_string_or_null(
    source: &Map<String, Value>,
    target: &mut Map<String, Value>,
    key: &str,
) {
    let Some(value) = source.get(key) else {
        return;
    };
    if value.is_null() {
        target.insert(key.to_string(), Value::Null);
    } else if let Some(value) = value.as_str().and_then(safe_display_text) {
        target.insert(key.to_string(), Value::String(value.to_string()));
    }
}

fn safe_status_token(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()
        && value.len() <= 160
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric()
                || matches!(character, '_' | '-' | ':' | '.' | '/' | '+')
        }))
    .then_some(value)
}

fn safe_display_text(value: &str) -> Option<&str> {
    let value = value.trim();
    if value.is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
        return None;
    }
    let lowered = value.to_ascii_lowercase();
    (![
        "authorization",
        "bearer ",
        "password",
        "secret",
        "token=",
        "://",
    ]
    .iter()
    .any(|marker| lowered.contains(marker)))
    .then_some(value)
}

fn redact_rule_array(rules: Option<&Value>, redact: fn(&Value) -> Value) -> Value {
    let Some(rules) = rules.and_then(Value::as_array) else {
        return Value::Null;
    };
    Value::Array(rules.iter().map(redact).collect())
}

fn redact_json_value(value: &Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.iter().map(redact_json_value).collect()),
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(key, value)| (key.clone(), redact_json_value_for_key(key, value)))
                .collect(),
        ),
        _ => value.clone(),
    }
}

fn redact_proxy_json_value(value: &Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.iter().map(redact_proxy_json_value).collect()),
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(key, value)| (key.clone(), redact_proxy_json_value_for_key(key, value)))
                .collect(),
        ),
        _ => value.clone(),
    }
}

fn redact_proxy_json_value_for_key(key: &str, value: &Value) -> Value {
    let compact_key = compact_json_key(key);
    if proxy_credential_key(&compact_key) {
        return redact_proxy_secret_value(value);
    }
    if json_url_key(&compact_key) {
        return redact_json_url_value(value);
    }
    if compact_key == "proxy" {
        return admin_secret_safe_proxy(Some(value));
    }
    if header_rules_key(&compact_key) {
        return admin_secret_safe_header_rules(Some(value));
    }
    if body_rules_key(&compact_key) {
        return admin_secret_safe_body_rules(Some(value));
    }
    if header_values_key(&compact_key) && value.is_object() {
        return redact_header_values(value);
    }
    redact_proxy_json_value(value)
}

fn redact_json_value_for_key(key: &str, value: &Value) -> Value {
    let compact_key = compact_json_key(key);
    if json_secret_key(&compact_key, value) {
        return redact_secret_value(value);
    }
    if json_url_key(&compact_key) {
        return redact_json_url_value(value);
    }
    if compact_key == "proxy" {
        return admin_secret_safe_proxy(Some(value));
    }
    if header_rules_key(&compact_key) {
        return admin_secret_safe_header_rules(Some(value));
    }
    if body_rules_key(&compact_key) {
        return admin_secret_safe_body_rules(Some(value));
    }
    if header_values_key(&compact_key) && value.is_object() {
        return redact_header_values(value);
    }
    redact_json_value(value)
}

fn redact_json_url_value(value: &Value) -> Value {
    match value {
        Value::String(raw) if url::Url::parse(raw).is_ok_and(|url| url.scheme() == "data") => {
            value.clone()
        }
        Value::String(raw) => admin_secret_safe_url(Some(raw)),
        Value::Array(values) => Value::Array(values.iter().map(redact_json_url_value).collect()),
        _ => redact_json_value(value),
    }
}

fn redact_body_rule(rule: &Value) -> Value {
    let Some(rule) = rule.as_object() else {
        return redact_json_value(rule);
    };
    let mut projected = rule
        .iter()
        .filter(|(key, _)| !is_rule_marker(key))
        .map(|(key, value)| (key.clone(), redact_json_value_for_key(key, value)))
        .collect::<Map<_, _>>();
    if let Some(condition) = rule.get("condition") {
        projected.insert("condition".to_string(), redact_condition(condition));
    }

    let target_is_sensitive = rule
        .get("path")
        .and_then(Value::as_str)
        .is_some_and(json_path_targets_secret);
    let action = normalized_string_field(rule, "action");
    if target_is_sensitive && matches!(action.as_deref(), Some("set" | "append" | "insert")) {
        redact_rule_secret_field(rule, &mut projected, "value", "has_value");
    }
    if target_is_sensitive && action.as_deref() == Some("regex_replace") {
        redact_rule_secret_field(rule, &mut projected, "pattern", "has_pattern");
        redact_rule_secret_field(rule, &mut projected, "replacement", "has_replacement");
    }
    Value::Object(projected)
}

fn redact_header_rule(rule: &Value) -> Value {
    let Some(rule) = rule.as_object() else {
        return redact_json_value(rule);
    };
    let mut projected = rule
        .iter()
        .filter(|(key, _)| !is_rule_marker(key))
        .map(|(key, value)| (key.clone(), redact_json_value_for_key(key, value)))
        .collect::<Map<_, _>>();
    let is_set = normalized_string_field(rule, "action").as_deref() == Some("set");
    if is_set
        && rule
            .get("key")
            .and_then(Value::as_str)
            .is_none_or(header_value_is_secret)
    {
        redact_rule_secret_field(rule, &mut projected, "value", "has_value");
    }
    if let Some(condition) = rule.get("condition") {
        projected.insert("condition".to_string(), redact_condition(condition));
    }
    Value::Object(projected)
}

fn redact_rule_secret_field(
    source: &Map<String, Value>,
    projected: &mut Map<String, Value>,
    field: &str,
    marker: &str,
) {
    let Some(value) = source.get(field).filter(|value| secret_value_is_set(value)) else {
        return;
    };
    projected.insert(field.to_string(), redact_secret_value(value));
    projected.insert(marker.to_string(), Value::Bool(true));
}

fn redact_condition(condition: &Value) -> Value {
    let Some(condition) = condition.as_object() else {
        return redact_json_value(condition);
    };
    let mut projected = condition
        .iter()
        .filter(|(key, _)| !is_rule_marker(key))
        .map(|(key, value)| {
            let value = if matches!(key.as_str(), "all" | "any") {
                value
                    .as_array()
                    .map(|items| Value::Array(items.iter().map(redact_condition).collect()))
                    .unwrap_or_else(|| redact_json_value(value))
            } else {
                redact_json_value_for_key(key, value)
            };
            (key.clone(), value)
        })
        .collect::<Map<_, _>>();
    if condition_value_is_secret(condition) {
        redact_rule_secret_field(condition, &mut projected, "value", "has_value");
    }
    Value::Object(projected)
}

fn redact_header_values(value: &Value) -> Value {
    let Some(headers) = value.as_object() else {
        return Value::Null;
    };
    Value::Object(
        headers
            .iter()
            .map(|(key, value)| {
                let value = if header_value_is_secret(key) {
                    redact_secret_value(value)
                } else {
                    redact_json_value(value)
                };
                (key.clone(), value)
            })
            .collect(),
    )
}

fn restore_json_value(existing: Option<&Value>, incoming: &Value, key: Option<&str>) -> Value {
    if let Some(key) = key {
        let compact_key = compact_json_key(key);
        if compact_key == "proxy" {
            return admin_restore_secret_safe_proxy(existing, incoming);
        }
        if header_rules_key(&compact_key) {
            return admin_restore_secret_safe_header_rules(existing, incoming);
        }
        if body_rules_key(&compact_key) {
            return admin_restore_secret_safe_body_rules(existing, incoming);
        }
        if header_values_key(&compact_key) && incoming.is_object() {
            return restore_header_values(existing, incoming);
        }
        if json_url_key(&compact_key) {
            if let Some(incoming_url) = incoming.as_str() {
                return restore_url_value(existing, incoming_url);
            }
            if let Some(values) = incoming.as_array() {
                let existing_values = existing.and_then(Value::as_array);
                return Value::Array(
                    values
                        .iter()
                        .enumerate()
                        .map(|(index, value)| {
                            restore_json_value(
                                existing_values.and_then(|values| values.get(index)),
                                value,
                                Some(key),
                            )
                        })
                        .collect(),
                );
            }
        }
        if json_secret_key(&compact_key, incoming) {
            return restore_masked_secret(existing, incoming, true);
        }
    }

    match incoming {
        Value::Array(incoming_values) => {
            let existing_values = existing.and_then(Value::as_array);
            Value::Array(
                incoming_values
                    .iter()
                    .enumerate()
                    .map(|(index, incoming_value)| {
                        restore_json_value(
                            existing_values.and_then(|values| values.get(index)),
                            incoming_value,
                            None,
                        )
                    })
                    .collect(),
            )
        }
        Value::Object(incoming_object) => {
            let existing_object = existing.and_then(Value::as_object);
            Value::Object(
                incoming_object
                    .iter()
                    .map(|(key, incoming_value)| {
                        (
                            key.clone(),
                            restore_json_value(
                                existing_object.and_then(|object| object.get(key)),
                                incoming_value,
                                Some(key),
                            ),
                        )
                    })
                    .collect(),
            )
        }
        _ => incoming.clone(),
    }
}

fn restore_rule_array(existing: Option<&Value>, incoming: &Value, kind: RuleKind) -> Value {
    let Some(incoming_values) = incoming.as_array() else {
        return incoming.clone();
    };
    if unchanged_projected_rules(existing, incoming, kind) {
        return existing.cloned().unwrap_or_else(|| incoming.clone());
    }
    let existing_values = existing
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();

    Value::Array(
        incoming_values
            .iter()
            .map(|incoming_rule| {
                let existing_rule =
                    match_existing_rule(existing_values, incoming_values, incoming_rule, kind);
                restore_rule(existing_rule, incoming_rule, kind)
            })
            .collect(),
    )
}

fn restore_rule(existing: Option<&Value>, incoming: &Value, kind: RuleKind) -> Value {
    let Some(incoming_object) = incoming.as_object() else {
        return restore_json_value(existing, incoming, None);
    };
    let existing_object = existing.and_then(Value::as_object);
    let mut restored = Map::new();
    for (key, incoming_value) in incoming_object {
        if is_rule_marker(key) {
            continue;
        }
        let existing_value = existing_object.and_then(|object| object.get(key));
        let value = if key == "condition" {
            restore_condition(existing_value, incoming_value)
        } else if rule_field_is_secret(incoming_object, kind, key) {
            let marker = marker_for_secret_field(key);
            let marker_set = marker
                .and_then(|marker| incoming_object.get(marker))
                .and_then(Value::as_bool)
                == Some(true);
            restore_masked_secret(existing_value, incoming_value, marker_set)
        } else {
            restore_json_value(existing_value, incoming_value, Some(key))
        };
        restored.insert(key.clone(), value);
    }
    Value::Object(restored)
}

fn restore_condition(existing: Option<&Value>, incoming: &Value) -> Value {
    let Some(incoming_object) = incoming.as_object() else {
        return restore_json_value(existing, incoming, None);
    };
    let existing_object = existing.and_then(Value::as_object);

    for group_key in ["all", "any"] {
        if let Some(incoming_children) = incoming_object.get(group_key) {
            let matching_existing = existing_object
                .filter(|object| object.contains_key(group_key))
                .and_then(|object| object.get(group_key));
            let mut restored = Map::new();
            for (key, incoming_value) in incoming_object {
                if is_rule_marker(key) {
                    continue;
                }
                let value = if key == group_key {
                    restore_condition_array(matching_existing, incoming_children)
                } else {
                    restore_json_value(
                        existing_object.and_then(|object| object.get(key)),
                        incoming_value,
                        Some(key),
                    )
                };
                restored.insert(key.clone(), value);
            }
            return Value::Object(restored);
        }
    }

    let identities_match = condition_identity(incoming)
        .zip(existing.and_then(condition_identity))
        .is_some_and(|(incoming, existing)| incoming == existing);
    let existing_object = identities_match.then_some(existing_object).flatten();
    let mut restored = Map::new();
    for (key, incoming_value) in incoming_object {
        if is_rule_marker(key) {
            continue;
        }
        let existing_value = existing_object.and_then(|object| object.get(key));
        let value = if key == "value"
            && (condition_value_is_secret(incoming_object)
                || incoming_object.get("has_value").and_then(Value::as_bool) == Some(true))
        {
            let marker_set =
                incoming_object.get("has_value").and_then(Value::as_bool) == Some(true);
            restore_masked_secret(existing_value, incoming_value, marker_set)
        } else {
            restore_json_value(existing_value, incoming_value, Some(key))
        };
        restored.insert(key.clone(), value);
    }
    Value::Object(restored)
}

fn restore_condition_array(existing: Option<&Value>, incoming: &Value) -> Value {
    let Some(incoming_values) = incoming.as_array() else {
        return incoming.clone();
    };
    let existing_values = existing
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    if projected_conditions_unchanged(existing_values, incoming_values) {
        return Value::Array(existing_values.to_vec());
    }

    Value::Array(
        incoming_values
            .iter()
            .map(|incoming_condition| {
                let existing_condition = match_existing_entry(
                    existing_values,
                    incoming_values,
                    incoming_condition,
                    condition_identity,
                    redact_condition,
                );
                restore_condition(existing_condition, incoming_condition)
            })
            .collect(),
    )
}

fn restore_header_values(existing: Option<&Value>, incoming: &Value) -> Value {
    let Some(incoming_headers) = incoming.as_object() else {
        return incoming.clone();
    };
    let existing_headers = existing.and_then(Value::as_object);
    Value::Object(
        incoming_headers
            .iter()
            .map(|(key, value)| {
                (
                    key.clone(),
                    restore_masked_secret(
                        existing_headers.and_then(|headers| headers.get(key)),
                        value,
                        true,
                    ),
                )
            })
            .collect(),
    )
}

fn restore_masked_secret(existing: Option<&Value>, incoming: &Value, allow_restore: bool) -> Value {
    if allow_restore && incoming.as_str() == Some(REDACTED_VALUE) {
        return existing
            .filter(|value| secret_value_is_set(value))
            .cloned()
            .unwrap_or_else(|| incoming.clone());
    }
    incoming.clone()
}

fn restore_url_value(existing: Option<&Value>, incoming_url: &str) -> Value {
    if let Some(existing_url) = existing.and_then(Value::as_str) {
        return Value::String(admin_restore_secret_safe_url(
            Some(existing_url),
            incoming_url,
        ));
    }
    Value::String(incoming_url.to_string())
}

fn project_rule(rule: &Value, kind: RuleKind) -> Value {
    match kind {
        RuleKind::Header => redact_header_rule(rule),
        RuleKind::Body => redact_body_rule(rule),
    }
}

fn unchanged_projected_rules(existing: Option<&Value>, incoming: &Value, kind: RuleKind) -> bool {
    existing.and_then(Value::as_array).is_some_and(|rules| {
        Value::Array(rules.iter().map(|rule| project_rule(rule, kind)).collect()) == *incoming
    })
}

fn rule_match_shape(rule: &Value, kind: RuleKind) -> Value {
    let mut projected = project_rule(rule, kind);
    if let Some(object) = projected.as_object_mut() {
        for field in [
            "enabled",
            "value",
            "has_value",
            "pattern",
            "has_pattern",
            "replacement",
            "has_replacement",
        ] {
            object.remove(field);
        }
    }
    projected
}

fn match_existing_rule<'a>(
    existing: &'a [Value],
    incoming: &[Value],
    rule: &Value,
    kind: RuleKind,
) -> Option<&'a Value> {
    match_existing_entry(
        existing,
        incoming,
        rule,
        |value| rule_identity(value, kind),
        |value| rule_match_shape(value, kind),
    )
}

fn match_existing_entry<'a>(
    existing: &'a [Value],
    incoming: &[Value],
    entry: &Value,
    identity: impl Fn(&Value) -> Option<String>,
    project: impl Fn(&Value) -> Value,
) -> Option<&'a Value> {
    let entry_identity = identity(entry)?;
    let same_identity = |value: &&Value| identity(value).as_ref() == Some(&entry_identity);
    let candidates = existing.iter().filter(same_identity).collect::<Vec<_>>();
    if candidates.len() == 1 && incoming.iter().filter(same_identity).count() == 1 {
        return candidates.first().copied();
    }
    let projected = project(entry);
    let mut matching = candidates
        .into_iter()
        .filter(|candidate| project(candidate) == projected);
    let matched = matching.next()?;
    if matching.next().is_some()
        || incoming
            .iter()
            .filter(same_identity)
            .filter(|value| project(value) == projected)
            .count()
            != 1
    {
        return None;
    }
    Some(matched)
}

fn projected_conditions_unchanged(existing: &[Value], incoming: &[Value]) -> bool {
    existing.len() == incoming.len()
        && existing
            .iter()
            .zip(incoming)
            .all(|(existing, incoming)| redact_condition(existing) == *incoming)
}

fn validate_retained_rule_secrets(
    existing: Option<&Value>,
    incoming: &Value,
    kind: RuleKind,
) -> Result<(), String> {
    let Some(incoming_values) = incoming.as_array() else {
        return Ok(());
    };
    if unchanged_projected_rules(existing, incoming, kind) {
        return Ok(());
    }
    let existing_values = existing
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    for rule in incoming_values {
        let existing_rule = match_existing_rule(existing_values, incoming_values, rule, kind);
        validate_retained_secret_fields(existing_rule, rule)?;
        if let Some(condition) = rule.get("condition") {
            validate_retained_condition_secrets(
                existing_rule.and_then(|rule| rule.get("condition")),
                condition,
            )?;
        }
    }
    Ok(())
}

fn validate_retained_secret_fields(
    existing: Option<&Value>,
    incoming: &Value,
) -> Result<(), String> {
    for (field, marker) in [
        ("value", "has_value"),
        ("pattern", "has_pattern"),
        ("replacement", "has_replacement"),
    ] {
        if incoming.get(marker).and_then(Value::as_bool) == Some(true)
            && incoming.get(field).and_then(Value::as_str) == Some(REDACTED_VALUE)
            && existing
                .and_then(|value| value.get(field))
                .filter(|value| secret_value_is_set(value))
                .is_none()
        {
            return Err(
                "无法匹配脱敏规则的原值，请查看原值后重新填写，避免将占位符保存为实际配置"
                    .to_string(),
            );
        }
    }
    Ok(())
}

fn validate_retained_condition_secrets(
    existing: Option<&Value>,
    incoming: &Value,
) -> Result<(), String> {
    for group_key in ["all", "any"] {
        if let Some(children) = incoming.get(group_key).and_then(Value::as_array) {
            let existing_children = existing
                .and_then(|value| value.get(group_key))
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or_default();
            if projected_conditions_unchanged(existing_children, children) {
                return Ok(());
            }
            for child in children {
                let existing_child = match_existing_entry(
                    existing_children,
                    children,
                    child,
                    condition_identity,
                    redact_condition,
                );
                validate_retained_condition_secrets(existing_child, child)?;
            }
            return Ok(());
        }
    }
    let existing =
        existing.filter(|value| condition_identity(value) == condition_identity(incoming));
    validate_retained_secret_fields(existing, incoming)
}

fn rule_identity(value: &Value, kind: RuleKind) -> Option<String> {
    let value = value.as_object()?;
    let action = normalized_string_field(value, "action")?;
    match (kind, action.as_str()) {
        (RuleKind::Header, "set" | "drop") => normalized_string_field(value, "key")
            .map(|key| format!("header:{action}:{}", key.to_ascii_lowercase())),
        (RuleKind::Header, "rename") => normalized_string_field(value, "from")
            .zip(normalized_string_field(value, "to"))
            .map(|(from, to)| {
                format!(
                    "header:rename:{}:{}",
                    from.to_ascii_lowercase(),
                    to.to_ascii_lowercase()
                )
            }),
        (RuleKind::Body, "set" | "drop" | "append" | "insert" | "regex_replace") => {
            trimmed_string_field(value, "path").map(|path| format!("body:{action}:{path}"))
        }
        (RuleKind::Body, "rename") => trimmed_string_field(value, "from")
            .zip(trimmed_string_field(value, "to"))
            .map(|(from, to)| format!("body:rename:{from}:{to}")),
        _ => None,
    }
}

fn condition_identity(value: &Value) -> Option<String> {
    let value = value.as_object()?;
    for group_key in ["all", "any"] {
        if let Some(children) = value.get(group_key).and_then(Value::as_array) {
            let mut identities = children
                .iter()
                .map(condition_identity)
                .collect::<Option<Vec<_>>>()?;
            identities.sort();
            return Some(format!(
                "condition:{group_key}:{}",
                serde_json::to_string(&identities).ok()?
            ));
        }
    }
    let path = trimmed_string_field(value, "path")?;
    let op = normalized_string_field(value, "op")?;
    let source = normalized_condition_source(value.get("source").and_then(Value::as_str));
    Some(format!("condition:{source}:{path}:{op}"))
}

fn rule_field_is_secret(rule: &Map<String, Value>, kind: RuleKind, field: &str) -> bool {
    match kind {
        RuleKind::Header => {
            normalized_string_field(rule, "action").as_deref() == Some("set") && field == "value"
        }
        RuleKind::Body => {
            let target_is_sensitive = rule
                .get("path")
                .and_then(Value::as_str)
                .is_some_and(json_path_targets_secret);
            let action = normalized_string_field(rule, "action");
            target_is_sensitive
                && (matches!(action.as_deref(), Some("set" | "append" | "insert"))
                    && field == "value"
                    || action.as_deref() == Some("regex_replace")
                        && matches!(field, "pattern" | "replacement"))
        }
    }
}

fn marker_for_secret_field(field: &str) -> Option<&'static str> {
    match field {
        "value" => Some("has_value"),
        "pattern" => Some("has_pattern"),
        "replacement" => Some("has_replacement"),
        _ => None,
    }
}

fn is_rule_marker(key: &str) -> bool {
    matches!(key, "has_value" | "has_pattern" | "has_replacement")
}

fn condition_value_is_secret(condition: &Map<String, Value>) -> bool {
    let source = normalized_condition_source(condition.get("source").and_then(Value::as_str));
    let path = condition.get("path").and_then(Value::as_str);
    if source == "request_headers" {
        path.is_none_or(header_value_is_secret)
    } else {
        path.is_some_and(json_path_targets_secret)
    }
}

fn header_value_is_secret(name: &str) -> bool {
    !matches!(
        name.trim().to_ascii_lowercase().as_str(),
        "accept"
            | "accept-encoding"
            | "accept-language"
            | "cache-control"
            | "content-encoding"
            | "content-type"
            | "user-agent"
            | "anthropic-version"
            | "anthropic-beta"
            | "openai-beta"
            | "x-stainless-lang"
            | "x-stainless-package-version"
            | "x-stainless-os"
            | "x-stainless-arch"
            | "x-stainless-runtime"
            | "x-stainless-runtime-version"
            | "x-stainless-retry-count"
            | "x-stainless-timeout"
    )
}

fn normalized_condition_source(source: Option<&str>) -> String {
    match source
        .map(str::trim)
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("headers" | "request_headers") => "request_headers".to_string(),
        Some(source) if !source.is_empty() => source.to_string(),
        _ => "body".to_string(),
    }
}

fn normalized_string_field(object: &Map<String, Value>, key: &str) -> Option<String> {
    trimmed_string_field(object, key).map(|value| value.to_ascii_lowercase())
}

fn trimmed_string_field(object: &Map<String, Value>, key: &str) -> Option<String> {
    object
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn sanitize_network_url(value: &str) -> Option<String> {
    let value = value.trim();
    let mut parsed = url::Url::parse(value).ok()?;
    parsed.host_str()?;
    parsed.set_username("").ok()?;
    parsed.set_password(None).ok()?;
    parsed.set_query(None);
    parsed.set_fragment(None);

    let root_path_only = parsed.path() == "/";
    let mut sanitized = parsed.to_string();
    if root_path_only
        && !value
            .split(['?', '#'])
            .next()
            .unwrap_or(value)
            .ends_with('/')
    {
        sanitized.pop();
    }
    Some(sanitized)
}

fn network_url_has_hidden_components(value: &str) -> bool {
    let Ok(parsed) = url::Url::parse(value.trim()) else {
        return false;
    };
    !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
}

fn compact_json_key(key: &str) -> String {
    key.chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .map(|character| character.to_ascii_lowercase())
        .collect()
}

fn proxy_credential_key(compact_key: &str) -> bool {
    proxy_credential_field_from_compact(compact_key).is_some()
        || json_secret_key(compact_key, &Value::String(String::new()))
}

fn proxy_credential_field_from_compact(compact_key: &str) -> Option<AdminProxyCredentialField> {
    match compact_key {
        "user" | "username" | "proxyuser" | "proxyusername" => {
            Some(AdminProxyCredentialField::Username)
        }
        "password" | "passwd" | "passphrase" | "proxypassword" | "proxypasswd"
        | "proxypassphrase" => Some(AdminProxyCredentialField::Password),
        _ => None,
    }
}

fn json_secret_key(compact_key: &str, value: &Value) -> bool {
    if value.is_boolean()
        && (compact_key.starts_with("has")
            || compact_key.starts_with("is")
            || compact_key.starts_with("can")
            || compact_key.starts_with("supports"))
    {
        return false;
    }

    matches!(
        compact_key,
        "authorization"
            | "proxyauthorization"
            | "bearer"
            | "cookie"
            | "cookies"
            | "credential"
            | "credentials"
            | "hmac"
            | "passphrase"
            | "passwd"
            | "password"
            | "psk"
            | "secret"
            | "sessionkey"
            | "token"
            | "username"
    ) || [
        "apikey",
        "accesstoken",
        "refreshtoken",
        "idtoken",
        "sessiontoken",
        "bearertoken",
        "secretkey",
        "accesskey",
        "privatekey",
        "signingkey",
        "clientsecret",
        "secret",
        "credential",
        "credentials",
        "authorization",
        "password",
        "passphrase",
        "cookie",
    ]
    .iter()
    .any(|suffix| compact_key.ends_with(suffix))
}

fn header_rules_key(compact_key: &str) -> bool {
    matches!(
        compact_key,
        "headerrules" | "requestheaderrules" | "responseheaderrules"
    )
}

fn body_rules_key(compact_key: &str) -> bool {
    matches!(compact_key, "bodyrules" | "requestbodyrules")
}

fn json_url_key(compact_key: &str) -> bool {
    compact_key == "url" || compact_key.ends_with("url")
}

fn json_path_targets_secret(path: &str) -> bool {
    body_path_key_segments(path)
        .into_iter()
        .map(|segment| compact_json_key(&segment))
        .any(|segment| json_secret_key(&segment, &Value::String(String::new())))
}

fn body_path_key_segments(path: &str) -> Vec<String> {
    let chars = path.trim().chars().collect::<Vec<_>>();
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut index = 0;
    while index < chars.len() {
        match chars[index] {
            '\\' if index + 1 < chars.len() => {
                current.push(chars[index + 1]);
                index += 2;
            }
            '.' => {
                push_path_segment(&mut segments, &mut current);
                index += 1;
            }
            '[' => {
                push_path_segment(&mut segments, &mut current);
                let mut close = index + 1;
                while close < chars.len() && chars[close] != ']' {
                    close += 1;
                }
                if close >= chars.len() {
                    break;
                }
                let inner = chars[index + 1..close]
                    .iter()
                    .collect::<String>()
                    .trim()
                    .trim_matches(['\'', '"'])
                    .to_string();
                if (!inner.is_empty()
                    && inner != "*"
                    && inner.parse::<isize>().is_err()
                    && !inner.contains('-'))
                    || inner.contains(|character: char| character.is_ascii_alphabetic())
                {
                    segments.push(inner);
                }
                index = close + 1;
            }
            character => {
                current.push(character);
                index += 1;
            }
        }
    }
    push_path_segment(&mut segments, &mut current);
    segments
}

fn push_path_segment(segments: &mut Vec<String>, current: &mut String) {
    let segment = std::mem::take(current).trim().to_string();
    if !segment.is_empty() {
        segments.push(segment);
    }
}

fn header_values_key(compact_key: &str) -> bool {
    matches!(
        compact_key,
        "headers"
            | "extraheaders"
            | "requestheaders"
            | "responseheaders"
            | "staticheaders"
            | "defaultheaders"
    )
}

fn redact_secret_value(value: &Value) -> Value {
    if secret_value_is_set(value) {
        Value::String(REDACTED_VALUE.to_string())
    } else {
        value.clone()
    }
}

fn redact_proxy_secret_value(value: &Value) -> Value {
    if proxy_secret_value_is_set(value) {
        Value::String(REDACTED_VALUE.to_string())
    } else {
        value.clone()
    }
}

fn proxy_secret_value_is_set(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::String(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
        _ => true,
    }
}

fn secret_value_is_set(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::String(value) => !value.trim().is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        admin_provider_metadata_bucket_safe_json, admin_provider_oauth_invalid_reason_safe_text,
        admin_provider_status_snapshot_safe_json, admin_provider_upstream_metadata_safe_json,
        admin_restore_secret_safe_body_rules, admin_restore_secret_safe_header_rules,
        admin_restore_secret_safe_json, admin_restore_secret_safe_proxy,
        admin_secret_safe_body_rules, admin_secret_safe_header_rules, admin_secret_safe_json,
        admin_secret_safe_proxy, admin_secret_safe_url,
    };
    use serde_json::json;

    #[test]
    fn proxy_projection_removes_all_url_credentials_and_nested_secrets() {
        let existing = json!({
            "enabled": true,
            "mode": "direct",
            "node_id": "proxy-node-1",
            "url": "http://alice:proxy-password@proxy.example:8080/path?token=query-secret#fragment",
            "username": "alice",
            "password": "proxy-password",
            "options": {
                "clientSecret": "nested-secret",
                "region": "us-east-1"
            }
        });
        let projected = admin_secret_safe_proxy(Some(&existing));

        assert_eq!(projected["url"], "http://proxy.example:8080/path");
        assert_eq!(projected["username"], "***");
        assert_eq!(projected["password"], "***");
        assert_eq!(projected["options"]["clientSecret"], "***");
        assert_eq!(projected["options"]["region"], "us-east-1");
        assert_eq!(projected["has_credentials"], true);
        let serialized = projected.to_string();
        for secret in [
            "alice",
            "proxy-password",
            "query-secret",
            "nested-secret",
            "fragment",
        ] {
            assert!(!serialized.contains(secret));
        }

        let restored = admin_restore_secret_safe_proxy(Some(&existing), &projected);
        assert_eq!(restored, existing);
        assert!(restored.get("has_credentials").is_none());
    }

    #[test]
    fn proxy_restore_drops_old_credentials_when_authority_changes() {
        let existing = json!({
            "url": "http://proxy-old.example:8080",
            "username": "alice",
            "password": "old-password",
            "options": {
                "clientSecret": "old-nested-secret",
                "region": "us-east-1"
            }
        });
        let mut incoming = admin_secret_safe_proxy(Some(&existing));
        incoming["url"] = json!("http://proxy-new.example:8080");

        let restored = admin_restore_secret_safe_proxy(Some(&existing), &incoming);

        assert_eq!(restored["url"], "http://proxy-new.example:8080");
        assert!(restored.get("username").is_none());
        assert!(restored.get("password").is_none());
        assert!(restored["options"].get("clientSecret").is_none());
        assert_eq!(restored["options"]["region"], "us-east-1");
        assert!(restored.get("has_credentials").is_none());
    }

    #[test]
    fn proxy_restore_preserves_omitted_credentials_only_for_same_identity() {
        let existing = json!({
            "url": "HTTP://Proxy.Example:80",
            "node_id": " node-a ",
            "username": "alice",
            "password": "old-password"
        });
        let same_identity = json!({
            "url": "http://proxy.example",
            "node_id": "node-a"
        });
        let restored = admin_restore_secret_safe_proxy(Some(&existing), &same_identity);
        assert_eq!(restored["username"], "alice");
        assert_eq!(restored["password"], "old-password");

        let changed_node = json!({
            "url": "http://proxy.example",
            "node_id": "node-b",
            "username": "***",
            "password": "***"
        });
        let restored = admin_restore_secret_safe_proxy(Some(&existing), &changed_node);
        assert!(restored.get("username").is_none());
        assert!(restored.get("password").is_none());
    }

    #[test]
    fn proxy_projection_redacts_supported_nested_aliases() {
        let projected = admin_secret_safe_proxy(Some(&json!({
            "url": "socks5h://proxy.example:1080",
            "proxy_auth": {
                "proxy_user": "alice",
                "proxy_passphrase": "proxy-password"
            }
        })));

        assert_eq!(projected["proxy_auth"]["proxy_user"], "***");
        assert_eq!(projected["proxy_auth"]["proxy_passphrase"], "***");
        assert!(!projected.to_string().contains("proxy-password"));
    }

    #[test]
    fn proxy_projection_treats_whitespace_only_credentials_as_significant() {
        let existing = json!({
            "url": "http://proxy.example:8080",
            "username": " ",
            "password": "  "
        });
        let projected = admin_secret_safe_proxy(Some(&existing));
        assert_eq!(projected["username"], "***");
        assert_eq!(projected["password"], "***");
        assert_eq!(projected["has_credentials"], true);
        assert!(!projected.to_string().contains("\"  \""));

        let restored = admin_restore_secret_safe_proxy(Some(&existing), &projected);
        assert_eq!(restored["username"], " ");
        assert_eq!(restored["password"], "  ");
    }

    #[test]
    fn header_rule_projection_hides_set_and_header_condition_values() {
        let projected = admin_secret_safe_header_rules(Some(&json!([
            {
                "action": "set",
                "key": "x-custom-auth",
                "value": "static-secret",
                "condition": {
                    "source": "request_headers",
                    "path": "x-tenant-marker",
                    "op": "eq",
                    "value": "tenant-secret"
                }
            },
            {"action": "drop", "key": "x-debug"}
        ])));

        assert_eq!(projected[0]["value"], "***");
        assert_eq!(projected[0]["has_value"], true);
        assert_eq!(projected[0]["condition"]["value"], "***");
        assert_eq!(projected[0]["condition"]["has_value"], true);
        assert!(projected[1].get("value").is_none());
        let serialized = projected.to_string();
        assert!(!serialized.contains("static-secret"));
        assert!(!serialized.contains("tenant-secret"));
    }

    #[test]
    fn generic_projection_recurses_through_config_headers_and_proxy() {
        let projected = admin_secret_safe_json(Some(&json!({
            "pool_size": 3,
            "auth": {
                "access_token": "access-secret",
                "token": "exact-token-secret",
                "username": "nested-user",
                "webhook_secret": "webhook-secret",
                "has_refresh_token": true
            },
            "headers": {
                "x-region": "us-east-1",
                "authorization": "Bearer nested-secret"
            },
            "proxy": "socks5://user:password@proxy.example:1080?key=secret",
            "response_header_rules": [
                {"action": "set", "key": "x-response-marker", "value": "response-secret"}
            ]
        })));

        assert_eq!(projected["pool_size"], 3);
        assert_eq!(projected["auth"]["access_token"], "***");
        assert_eq!(projected["auth"]["token"], "***");
        assert_eq!(projected["auth"]["username"], "***");
        assert_eq!(projected["auth"]["webhook_secret"], "***");
        assert_eq!(projected["auth"]["has_refresh_token"], true);
        assert_eq!(projected["headers"]["x-region"], "***");
        assert_eq!(projected["proxy"], "socks5://proxy.example:1080");
        assert_eq!(projected["response_header_rules"][0]["value"], "***");
        let serialized = projected.to_string();
        for secret in [
            "access-secret",
            "exact-token-secret",
            "nested-user",
            "webhook-secret",
            "nested-secret",
            "password",
            "response-secret",
        ] {
            assert!(!serialized.contains(secret));
        }
    }

    #[test]
    fn provider_metadata_projection_removes_historical_upstream_diagnostics() {
        let projected = admin_provider_upstream_metadata_safe_json(Some(&json!({
            "codex": {
                "primary_used_percent": 25.0,
                "message": "Authorization: Bearer upstream-secret",
                "reset_credits": {
                    "available_count": 2,
                    "detail_error": "https://user:password@internal.test?q=secret"
                }
            },
            "kiro": {
                "is_banned": true,
                "ban_reason": "Authorization: Bearer upstream-secret"
            },
            "windsurf": {
                "daily_remaining_percent": 70.0,
                "last_error": "https://user:password@internal.test?q=secret",
                "rate_limit": {
                    "limited": true,
                    "message": "Authorization: Bearer upstream-secret"
                },
                "probe_warnings": [{
                    "probe": "models",
                    "message": "Authorization: Bearer upstream-secret"
                }]
            },
            "chatgpt_web": {
                "updated_at": 1_777_000_000,
                "image_quota_remaining": 8,
                "blocked_features": [
                    "image_generation",
                    "Authorization: Bearer upstream-secret"
                ],
                "limits_progress": [{
                    "message": "Authorization: Bearer upstream-secret",
                    "url": "https://user:password@internal.test?q=secret"
                }]
            }
        })));

        assert_eq!(
            projected.pointer("/codex/primary_used_percent"),
            Some(&json!(25.0))
        );
        assert_eq!(
            projected.pointer("/windsurf/daily_remaining_percent"),
            Some(&json!(70.0))
        );
        assert_eq!(
            projected.pointer("/chatgpt_web/image_quota_remaining"),
            Some(&json!(8))
        );
        assert_eq!(
            projected.pointer("/chatgpt_web/blocked_features"),
            Some(&json!(["image_generation"]))
        );
        assert!(projected.pointer("/chatgpt_web/limits_progress").is_none());
        let serialized = projected.to_string();
        assert!(!serialized.contains("upstream-secret"));
        assert!(!serialized.contains("user:password"));
        assert!(!serialized.contains("q=secret"));
    }

    #[test]
    fn chatgpt_metadata_projection_keeps_bounded_quota_dedup_key() {
        let projected = admin_provider_metadata_bucket_safe_json(
            "chatgpt_web",
            Some(&json!({
                "image_quota_last_local_request_key": "request-1:candidate-1"
            })),
        );
        assert_eq!(
            projected["image_quota_last_local_request_key"],
            "request-1:candidate-1"
        );

        let oversized = "x".repeat(161);
        for rejected in [
            "Authorization: Bearer secret",
            "request\nforged",
            oversized.as_str(),
        ] {
            let projected = admin_provider_metadata_bucket_safe_json(
                "chatgpt_web",
                Some(&json!({"image_quota_last_local_request_key": rejected})),
            );
            assert!(
                projected
                    .get("image_quota_last_local_request_key")
                    .is_none(),
                "unsafe quota dedup key must be dropped: {rejected:?}"
            );
        }
    }

    #[test]
    fn provider_status_projection_rebuilds_diagnostic_text_from_codes() {
        let projected = admin_provider_status_snapshot_safe_json(Some(&json!({
            "oauth": {
                "code": "invalid",
                "label": "Authorization: Bearer upstream-secret",
                "reason": "https://user:password@internal.test?q=secret",
                "invalid_at": 1_777_000_000,
                "requires_reauth": true,
                "unknown": {"body": "upstream-secret"}
            },
            "account": {
                "code": "account_disabled",
                "label": "Authorization: Bearer upstream-secret",
                "reason": "https://user:password@internal.test?q=secret",
                "blocked": true,
                "source": "metadata"
            },
            "quota": {
                "provider_type": "codex",
                "code": "cooldown",
                "reason": "Authorization: Bearer upstream-secret",
                "exhausted": false,
                "rate_limit": {
                    "limited": true,
                    "retry_after_ms": 5000,
                    "message": "https://user:password@internal.test?q=secret"
                },
                "reset_credits": {
                    "available_count": 1,
                    "detail_error": "connect https://user:password@internal.test?q=secret"
                },
                "unknown": {"body": "upstream-secret"}
            }
        })));

        assert_eq!(
            projected.pointer("/oauth/reason"),
            Some(&json!("OAuth token is invalid"))
        );
        assert_eq!(
            projected.pointer("/account/reason"),
            Some(&json!("Account is disabled"))
        );
        assert_eq!(
            projected.pointer("/quota/reason"),
            Some(&json!("Quota is temporarily cooling down"))
        );
        assert_eq!(
            projected.pointer("/quota/reset_credits/detail_error"),
            Some(&json!("Quota refresh connection failed"))
        );
        assert_eq!(
            projected.pointer("/quota/rate_limit/retry_after_ms"),
            Some(&json!(5000))
        );
        assert!(projected.pointer("/quota/rate_limit/message").is_none());
        assert!(projected.pointer("/quota/unknown").is_none());
        let serialized = projected.to_string();
        assert!(!serialized.contains("upstream-secret"));
        assert!(!serialized.contains("user:password"));
    }

    #[test]
    fn oauth_invalid_reason_projection_preserves_only_fixed_semantics() {
        let projected = admin_provider_oauth_invalid_reason_safe_text(Some(
            "[ACCOUNT_BLOCK] account has been deactivated: Authorization: Bearer upstream-secret https://user:password@internal.test?q=secret",
        ))
        .expect("reason should be projected");

        assert_eq!(projected, "[ACCOUNT_BLOCK] Account is disabled");
        assert!(!projected.contains("upstream-secret"));
        assert!(!projected.contains("user:password"));

        let bucket = admin_provider_metadata_bucket_safe_json(
            "windsurf",
            Some(&json!({
                "daily_remaining_percent": 50,
                "last_error": "Authorization: Bearer upstream-secret"
            })),
        );
        assert_eq!(bucket["daily_remaining_percent"], json!(50));
        assert!(!bucket.to_string().contains("upstream-secret"));
    }

    #[test]
    fn body_rule_projection_detects_compound_and_bracketed_secret_paths() {
        let projected = admin_secret_safe_body_rules(Some(&json!([
            {
                "action": "set",
                "path": "auth.api_key",
                "value": "body-token-secret",
                "condition": {
                    "source": "original",
                    "path": "auth['private-key']",
                    "op": "eq",
                    "value": "condition-secret"
                }
            },
            {"action": "set", "path": "metadata.region", "value": "us-east-1"}
        ])));

        assert_eq!(projected[0]["value"], "***");
        assert_eq!(projected[0]["has_value"], true);
        assert_eq!(projected[0]["condition"]["value"], "***");
        assert_eq!(projected[0]["condition"]["has_value"], true);
        assert_eq!(projected[1]["value"], "us-east-1");
        let serialized = projected.to_string();
        assert!(!serialized.contains("body-token-secret"));
        assert!(!serialized.contains("condition-secret"));
    }

    #[test]
    fn restore_projection_matches_unique_reordered_rules_and_removes_markers() {
        let existing = json!([
            {"action": "set", "key": "x-first", "value": "first-secret"},
            {"action": "set", "key": "x-second", "value": "second-secret"}
        ]);
        let incoming = json!([
            {"action": "set", "key": "x-second", "value": "***", "has_value": true},
            {"action": "set", "key": "x-first", "value": "replacement", "has_value": false}
        ]);

        let restored = admin_restore_secret_safe_header_rules(Some(&existing), &incoming);

        assert_eq!(restored[0]["value"], "second-secret");
        assert!(restored[0].get("has_value").is_none());
        assert_eq!(restored[1]["value"], "replacement");
        assert!(restored[1].get("has_value").is_none());
    }

    #[test]
    fn restore_does_not_move_a_secret_when_rule_identity_changes() {
        let existing = json!([
            {"action": "set", "path": "auth.token", "value": "secret"}
        ]);
        let incoming = json!([
            {"action": "set", "path": "metadata.note", "value": "***", "has_value": true}
        ]);

        let restored = admin_restore_secret_safe_body_rules(Some(&existing), &incoming);

        assert_eq!(restored[0]["value"], "***");
        assert!(restored[0].get("has_value").is_none());
        assert!(!restored.to_string().contains("secret"));
    }

    #[test]
    fn restore_does_not_move_a_secret_when_condition_semantics_change() {
        let existing = json!([{
            "action": "set",
            "key": "x-output",
            "value": "header-secret",
            "condition": {
                "source": "request_headers",
                "path": "x-tenant",
                "op": "eq",
                "value": "condition-secret"
            }
        }]);
        let incoming = json!([{
            "action": "set",
            "key": "x-output",
            "value": "***",
            "has_value": true,
            "condition": {
                "source": "body",
                "path": "metadata.note",
                "op": "eq",
                "value": "***",
                "has_value": true
            }
        }]);

        let restored = admin_restore_secret_safe_header_rules(Some(&existing), &incoming);

        assert_eq!(restored[0]["value"], "header-secret");
        assert_eq!(restored[0]["condition"]["value"], "***");
        assert!(!restored.to_string().contains("condition-secret"));
    }

    #[test]
    fn restore_requires_rule_marker_but_preserves_generic_sensitive_fields() {
        let header_existing = json!([
            {"action": "set", "key": "x-secret", "value": "old"}
        ]);
        let header_incoming = json!([
            {"action": "set", "key": "x-secret", "value": "***"}
        ]);
        assert_eq!(
            admin_restore_secret_safe_header_rules(Some(&header_existing), &header_incoming)[0]
                ["value"],
            "***"
        );

        let existing = json!({"note": "old note", "access_token": "old token"});
        let incoming = json!({"note": "***", "access_token": "***"});
        let restored = admin_restore_secret_safe_json(Some(&existing), &incoming);
        assert_eq!(restored["note"], "***");
        assert_eq!(restored["access_token"], "old token");
    }

    #[test]
    fn regex_markers_restore_only_the_same_sensitive_rule_and_are_not_persisted() {
        let existing = json!([{
            "action": "regex_replace",
            "path": "auth.token",
            "pattern": "old-pattern",
            "replacement": "old-replacement"
        }]);
        let projected = admin_secret_safe_body_rules(Some(&existing));
        let restored = admin_restore_secret_safe_body_rules(Some(&existing), &projected);

        assert_eq!(restored, existing);
        assert!(restored[0].get("has_pattern").is_none());
        assert!(restored[0].get("has_replacement").is_none());
    }

    #[test]
    fn empty_or_null_rule_values_do_not_gain_secret_markers() {
        let projected = admin_secret_safe_header_rules(Some(&json!([
            {"action": "set", "key": "x-empty", "value": ""},
            {"action": "set", "key": "x-null", "value": null}
        ])));

        assert_eq!(projected[0]["value"], "");
        assert_eq!(projected[1]["value"], json!(null));
        assert!(projected[0].get("has_value").is_none());
        assert!(projected[1].get("has_value").is_none());
    }

    #[test]
    fn generic_restore_preserves_hidden_url_parts_and_response_rule_secrets() {
        let existing = json!({
            "callback_url": "https://alice:password@example.test/callback?token=secret#fragment",
            "response_header_rules": [
                {"action": "set", "key": "authorization", "value": "Bearer secret"}
            ]
        });
        let projected = admin_secret_safe_json(Some(&existing));
        let restored = admin_restore_secret_safe_json(Some(&existing), &projected);

        assert_eq!(restored, existing);
    }

    #[test]
    fn url_projection_removes_userinfo_query_and_fragment() {
        let projected = admin_secret_safe_url(Some(
            "https://alice:password@api.example/v1?token=query-secret#fragment",
        ));

        assert_eq!(projected, "https://api.example/v1");
        assert_eq!(admin_secret_safe_url(Some("not a url")), json!(null));
    }

    #[test]
    fn public_protocol_headers_and_conditions_remain_editable() {
        let rules = json!([
            {"action": "set", "key": "Content-Type", "value": "application/json"},
            {"action": "set", "key": "User-Agent", "value": "client/1.0"},
            {"action": "set", "key": "anthropic-version", "value": "2023-06-01"},
            {"action": "set", "key": "OpenAI-Beta", "value": "responses=experimental", "condition": {
                "source": "request_headers", "path": "Accept", "op": "eq", "value": "text/event-stream"
            }}
        ]);
        assert_eq!(admin_secret_safe_header_rules(Some(&rules)), rules);
        let mut legacy_projection = rules.clone();
        legacy_projection[0]["value"] = json!("***");
        legacy_projection[0]["has_value"] = json!(true);
        legacy_projection[3]["condition"]["value"] = json!("***");
        legacy_projection[3]["condition"]["has_value"] = json!(true);
        assert_eq!(
            admin_restore_secret_safe_header_rules(Some(&rules), &legacy_projection),
            rules
        );
    }

    #[test]
    fn header_maps_keep_protocol_values_but_hide_credentials_and_unknown_headers() {
        let projected = admin_secret_safe_json(Some(&json!({"headers": {
            "Content-Type": "application/json",
            "User-Agent": "client/1.0",
            "Authorization": "Bearer secret",
            "Cookie": "session=secret",
            "x-custom-auth": "custom-secret"
        }})));
        assert_eq!(projected["headers"]["Content-Type"], "application/json");
        assert_eq!(projected["headers"]["User-Agent"], "client/1.0");
        for header in ["Authorization", "Cookie", "x-custom-auth"] {
            assert_eq!(projected["headers"][header], "***");
        }
    }

    #[test]
    fn duplicate_conditional_rules_retain_secrets_when_reordered_or_disabled() {
        let existing = json!([
            {"action": "set", "key": "x-auth", "value": "first-secret", "condition": {
                "path": "model", "op": "eq", "value": "first-model"
            }},
            {"action": "set", "key": "x-auth", "value": "second-secret", "condition": {
                "path": "model", "op": "eq", "value": "second-model"
            }}
        ]);
        let mut incoming = admin_secret_safe_header_rules(Some(&existing));
        incoming.as_array_mut().unwrap().reverse();
        incoming[0]["enabled"] = json!(false);
        super::admin_validate_retained_header_rule_secrets(Some(&existing), &incoming).unwrap();
        let restored = admin_restore_secret_safe_header_rules(Some(&existing), &incoming);
        assert_eq!(restored[0]["value"], "second-secret");
        assert_eq!(restored[0]["enabled"], false);
        assert_eq!(restored[1]["value"], "first-secret");
        assert!(restored[0].get("has_value").is_none());
    }

    #[test]
    fn unchanged_duplicate_body_rules_preserve_their_original_values() {
        let existing = json!([
            {"action": "append", "path": "auth.cookies", "value": "first-secret"},
            {"action": "append", "path": "auth.cookies", "value": "second-secret"}
        ]);
        let incoming = admin_secret_safe_body_rules(Some(&existing));
        super::admin_validate_retained_body_rule_secrets(Some(&existing), &incoming).unwrap();
        assert_eq!(
            admin_restore_secret_safe_body_rules(Some(&existing), &incoming),
            existing
        );
    }

    #[test]
    fn nested_condition_groups_preserve_secrets_after_sibling_edits_and_reordering() {
        let existing = json!([{
            "action": "set", "key": "x-output", "value": "header-secret",
            "condition": {"all": [
                {"any": [
                    {"path": "auth.token", "op": "eq", "value": "condition-secret"},
                    {"path": "model", "op": "eq", "value": "old-model"}
                ]},
                {"path": "metadata.enabled", "op": "eq", "value": true}
            ]}
        }]);
        let mut incoming = admin_secret_safe_header_rules(Some(&existing));
        incoming[0]["condition"]["all"][0]["any"][1]["value"] = json!("new-model");
        incoming[0]["condition"]["all"]
            .as_array_mut()
            .unwrap()
            .reverse();
        super::admin_validate_retained_header_rule_secrets(Some(&existing), &incoming).unwrap();
        let restored = admin_restore_secret_safe_header_rules(Some(&existing), &incoming);
        assert_eq!(restored[0]["value"], "header-secret");
        assert_eq!(
            restored[0]["condition"]["all"][1]["any"][0]["value"],
            "condition-secret"
        );
        assert_eq!(
            restored[0]["condition"]["all"][1]["any"][1]["value"],
            "new-model"
        );
        assert!(!restored.to_string().contains("has_value"));
    }

    #[test]
    fn retained_masks_cannot_silently_overwrite_changed_or_ambiguous_rules() {
        let existing = json!([
            {"action": "set", "key": "x-auth", "value": "first-secret"},
            {"action": "set", "key": "x-auth", "value": "second-secret"}
        ]);
        let mut incoming = admin_secret_safe_header_rules(Some(&existing));
        incoming[0]["enabled"] = json!(false);
        assert!(
            super::admin_validate_retained_header_rule_secrets(Some(&existing), &incoming).is_err()
        );

        let existing = json!([{"action": "set", "path": "auth.token", "value": "secret"}]);
        let mut incoming = admin_secret_safe_body_rules(Some(&existing));
        incoming[0]["path"] = json!("auth.api_key");
        assert!(
            super::admin_validate_retained_body_rule_secrets(Some(&existing), &incoming).is_err()
        );
        incoming[0]["value"] = json!("replacement-secret");
        incoming[0].as_object_mut().unwrap().remove("has_value");
        assert!(
            super::admin_validate_retained_body_rule_secrets(Some(&existing), &incoming).is_ok()
        );
    }

    #[test]
    fn body_rule_projection_preserves_structured_image_urls_and_inline_data() {
        let existing = json!([{
            "action": "append", "path": "messages[0].content",
            "value": {"type": "image_url", "image_url": {"url": "data:image/png;base64,aW1hZ2U=", "detail": "high"}}
        }]);
        let projected = admin_secret_safe_body_rules(Some(&existing));
        assert_eq!(projected, existing);
        assert_eq!(
            admin_restore_secret_safe_body_rules(Some(&existing), &projected),
            existing
        );
    }

    #[test]
    fn structured_url_values_still_hide_and_restore_network_credentials() {
        let existing = json!({
            "image_url": {"url": "https://user:password@example.test/image?token=secret", "detail": "auto"},
            "url": ["https://example.test/file?token=secret", "data:image/png;base64,aW1hZ2U="]
        });
        let projected = admin_secret_safe_json(Some(&existing));
        assert_eq!(projected["image_url"]["url"], "https://example.test/image");
        assert_eq!(projected["image_url"]["detail"], "auto");
        assert_eq!(projected["url"][0], "https://example.test/file");
        assert_eq!(projected["url"][1], existing["url"][1]);
        assert!(!projected.to_string().contains("secret"));
        assert!(!projected.to_string().contains("password"));
        assert_eq!(
            admin_restore_secret_safe_json(Some(&existing), &projected),
            existing
        );
    }
}
