use super::{
    is_interactive_export_private_system_config_key, AdminAppState, SystemExportMode,
    ADMIN_SYSTEM_DATA_EXPORT_VERSION, ADMIN_SYSTEM_EXPORT_CREDENTIALS_NOT_EXPORTED,
};
use crate::ai_serving::build_provider_key_pool_score_upsert;
use crate::api::ai::admin_endpoint_signature_parts;
use crate::handlers::admin::admin_provider_pool_config;
use crate::handlers::admin::model::ADMIN_EXTERNAL_MODELS_PROXY_NODE_CONFIG_KEY;
use crate::handlers::admin::provider::endpoints_admin::payloads::AdminProviderEndpointUpdatePatch;
use crate::handlers::admin::provider::oauth::provisioning::ensure_codex_credential_generation_rotated;
use crate::handlers::admin::provider::shared::payloads::{
    AdminProviderCreateRequest, AdminProviderKeyCreateRequest, AdminProviderKeyUpdatePatch,
    AdminProviderUpdatePatch,
};
use crate::handlers::admin::provider::write::keys::{
    build_admin_update_provider_key_record_with_existing_keys,
    build_provider_catalog_key_admin_cas_update,
};
use crate::handlers::admin::shared::{
    normalize_json_array, normalize_json_object, normalize_string_list,
};
use crate::handlers::admin::system::shared::configs::{
    apply_admin_system_config_update, is_sensitive_admin_system_config_key,
};
use crate::handlers::admin::users::{
    hash_admin_user_api_key, normalize_admin_feature_settings, normalize_admin_list_policy_mode,
    normalize_admin_rate_limit_policy_mode, normalize_admin_user_api_formats,
    normalize_admin_user_ip_rules, normalize_admin_user_string_list,
};
use crate::handlers::public::normalize_admin_base_url;
use crate::handlers::shared::{
    canonicalize_provider_ops_base_url, ldap_attribute_description_is_valid,
    ldap_distinguished_name_is_valid, ldap_search_filter_is_valid,
    normalize_ldap_transport_server_url, provider_ops_credential_binding_from_config,
    seal_auth_api_key_secret, seal_provider_ops_credential, PROVIDER_OPS_PERSISTENT_SECRET_FIELDS,
    PROVIDER_OPS_TRANSIENT_METADATA_FIELDS, PROVIDER_OPS_TRANSIENT_SECRET_FIELDS,
};
use crate::GatewayError;
use aether_admin::provider::endpoints as admin_provider_endpoints_pure;
use aether_admin::provider::models_write as admin_provider_models_write_pure;
use aether_admin::provider::redaction::{
    admin_restore_secret_safe_body_rules, admin_restore_secret_safe_header_rules,
    admin_restore_secret_safe_json, admin_restore_secret_safe_proxy,
};
use aether_admin::system::{
    normalize_admin_system_config_key, parse_admin_system_config_array,
    parse_admin_system_config_import_request, parse_admin_system_config_nested_array,
    parse_admin_system_config_optional_object, parse_admin_system_config_update,
    AdminImportMergeMode, AdminSystemConfigEndpoint as ImportedEndpoint,
    AdminSystemConfigEntry as ImportedSystemConfig,
    AdminSystemConfigGlobalModel as ImportedGlobalModel, AdminSystemConfigImportCounter,
    AdminSystemConfigImportStats, AdminSystemConfigLdap as ImportedLdapConfig,
    AdminSystemConfigOAuthProvider as ImportedOAuthProvider,
    AdminSystemConfigProvider as ImportedProvider,
    AdminSystemConfigProviderKey as ImportedProviderKey,
    AdminSystemConfigProviderModel as ImportedProviderModel,
    AdminSystemConfigProxyNode as ImportedProxyNode, ADMIN_SYSTEM_USERS_SUPPORTED_VERSIONS,
};
use aether_data::repository::auth_modules::{
    CompareAndSwapLdapConfigResult, LdapBindPasswordUpdate, StoredLdapModuleConfig,
};
use aether_data::repository::oauth_providers::{
    EncryptedSecretUpdate, UpsertOAuthProviderConfigRecord,
};
use aether_data::repository::system::{
    AdminSystemStatsUserDailyAggregate, AdminSystemUsageAggregateImportMode,
    AdminSystemUsageAggregateImportSummary, AdminSystemUsageAggregateSnapshot,
};
use aether_data::repository::wallet::{StoredWalletSnapshot, WalletLookupKey};
use aether_data_contracts::repository::global_models::{
    CreateAdminGlobalModelRecord, UpdateAdminGlobalModelRecord, UpsertAdminProviderModelRecord,
};
use aether_data_contracts::repository::pool_scores::PoolMemberScoreUpsertMode;
use axum::{body::Bytes, http};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

const ADMIN_SYSTEM_USERS_RECOVERY_IMPORT_VERSION: (u32, u32) = (1, 5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SystemImportMode {
    InteractiveUpload,
    RecoveryBackup,
    /// Internal aggregate rollback. Credentials remain redacted, but operational active flags
    /// from the checkpoint are restored exactly like a recovery backup.
    RollbackCheckpoint,
    /// Internal aggregate rollback for a recovery backup. Unlike the interactive checkpoint,
    /// this mode carries the encrypted/decrypted credential fields needed to restore values that
    /// the failed recovery import may already have overwritten.
    RecoveryRollbackCheckpoint,
}

impl SystemImportMode {
    fn restores_credentials(self) -> bool {
        matches!(
            self,
            Self::RecoveryBackup | Self::RecoveryRollbackCheckpoint
        )
    }

    fn preserves_active_state(self) -> bool {
        matches!(
            self,
            Self::RecoveryBackup | Self::RollbackCheckpoint | Self::RecoveryRollbackCheckpoint
        )
    }

    fn is_rollback_checkpoint(self) -> bool {
        matches!(
            self,
            Self::RollbackCheckpoint | Self::RecoveryRollbackCheckpoint
        )
    }

    fn allows_audit_admin_restore(self) -> bool {
        matches!(
            self,
            Self::RecoveryBackup | Self::RecoveryRollbackCheckpoint
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ImportedApiKeyMaterial {
    key_hash: String,
    key_plaintext: Option<String>,
}

#[derive(Debug, Clone)]
struct ExistingWalletMutation {
    before: StoredWalletSnapshot,
    // `None` means the import observed an existing wallet but did not receive a
    // verifiable post-write snapshot. Rollback must report that as a failure
    // rather than guessing or applying an owner-blind overwrite.
    after: Option<StoredWalletSnapshot>,
}

#[derive(Debug, Clone)]
struct ExistingUserMutation {
    before_auth: aether_data::repository::users::StoredUserAuthRecord,
    after_auth: aether_data::repository::users::StoredUserAuthRecord,
    before_export: Option<aether_data::repository::users::StoredUserExportRow>,
    after_export: Option<aether_data::repository::users::StoredUserExportRow>,
    before_model_capability_settings: Option<Value>,
    after_model_capability_settings: Option<Value>,
    before_feature_settings: Option<Value>,
    after_feature_settings: Option<Value>,
    before_group_ids: Vec<String>,
    after_group_ids: Vec<String>,
}

#[derive(Debug, Clone)]
struct ExistingUserGroupMutation {
    before: aether_data::repository::users::StoredUserGroup,
    after: aether_data::repository::users::StoredUserGroup,
}

#[derive(Debug, Clone)]
struct ExistingApiKeyMutation {
    before: aether_data::repository::auth::StoredAuthApiKeyExportRecord,
    after: aether_data::repository::auth::StoredAuthApiKeyExportRecord,
}

#[cfg(test)]
fn synthetic_rollback_export_row(
    auth: &aether_data::repository::users::StoredUserAuthRecord,
    model_capability_settings: Option<Value>,
    feature_settings: Option<Value>,
) -> Result<aether_data::repository::users::StoredUserExportRow, GatewayError> {
    aether_data::repository::users::StoredUserExportRow::new(
        auth.id.clone(),
        auth.email.clone(),
        auth.email_verified,
        auth.username.clone(),
        auth.password_hash.clone(),
        auth.role.clone(),
        auth.auth_source.clone(),
        auth.allowed_providers.clone().map(Value::from),
        auth.allowed_api_formats.clone().map(Value::from),
        auth.allowed_models.clone().map(Value::from),
        None,
        model_capability_settings,
        auth.is_active,
    )
    .map(|row| row.with_feature_settings(feature_settings))
    .and_then(|row| {
        row.with_policy_modes(
            auth.allowed_providers_mode.clone(),
            auth.allowed_api_formats_mode.clone(),
            auth.allowed_models_mode.clone(),
            "system".to_string(),
        )
    })
    .map_err(|err| GatewayError::Internal(err.to_string()))
}

/// Records rows created by one aggregate import invocation.  A post-failure full-table diff is
/// unsafe because ordinary admin mutations may run concurrently with the aggregate operation;
/// only these IDs are eligible for compensation.
#[derive(Debug, Default)]
struct AggregateMutationJournal {
    global_model_ids: BTreeSet<String>,
    provider_ids: BTreeSet<String>,
    provider_endpoint_ids: BTreeSet<(String, String)>,
    provider_key_ids: BTreeSet<(String, String)>,
    provider_model_ids: BTreeSet<(String, String)>,
    oauth_provider_types: BTreeSet<String>,
    system_config_keys: BTreeSet<String>,
    created_ldap_config: Option<StoredLdapModuleConfig>,
    user_group_ids: BTreeSet<String>,
    user_ids: BTreeSet<String>,
    user_wallet_snapshots: BTreeMap<(String, String), StoredWalletSnapshot>,
    api_key_wallet_snapshots: BTreeMap<(String, String), StoredWalletSnapshot>,
    existing_user_wallets: BTreeMap<(String, String), ExistingWalletMutation>,
    existing_api_key_wallets: BTreeMap<(String, String), ExistingWalletMutation>,
    existing_users: BTreeMap<String, ExistingUserMutation>,
    existing_user_groups: BTreeMap<String, ExistingUserGroupMutation>,
    existing_user_api_keys: BTreeMap<(String, String), ExistingApiKeyMutation>,
    existing_standalone_api_keys: BTreeMap<String, ExistingApiKeyMutation>,
    user_api_key_ids: BTreeSet<(String, String)>,
    standalone_api_key_ids: BTreeSet<String>,
}

/// Result of compensating config rows created by one aggregate import.  LDAP is tracked
/// separately because its checkpoint restore can overwrite a configuration written by another
/// admin while the import was running.
struct ConfigCleanupOutcome {
    result: Result<(), GatewayError>,
    skip_ldap_restore: bool,
}

fn invalid_request(detail: impl Into<String>) -> (http::StatusCode, Value) {
    (
        http::StatusCode::BAD_REQUEST,
        json!({ "detail": detail.into() }),
    )
}

fn normalize_imported_system_config_key(key: &str) -> String {
    let normalized = normalize_admin_system_config_key(key);
    if normalized.eq_ignore_ascii_case(ADMIN_EXTERNAL_MODELS_PROXY_NODE_CONFIG_KEY) {
        ADMIN_EXTERNAL_MODELS_PROXY_NODE_CONFIG_KEY.to_string()
    } else {
        normalized
    }
}

fn build_admin_system_data_import_part_body(
    root: &Map<String, Value>,
    field_name: &str,
    merge_mode: AdminImportMergeMode,
) -> Result<Bytes, (http::StatusCode, Value)> {
    let mut part = match root.get(field_name) {
        Some(Value::Object(map)) => map.clone(),
        Some(_) => return Err(invalid_request(format!("{field_name} 必须是对象"))),
        None => return Err(invalid_request(format!("{field_name} 为必填字段"))),
    };

    let merge_mode_value = serde_json::to_value(merge_mode)
        .map_err(|err| invalid_request(format!("merge_mode 序列化失败: {err}")))?;
    part.insert("merge_mode".to_string(), merge_mode_value);

    serde_json::to_vec(&Value::Object(part))
        .map(Bytes::from)
        .map_err(|err| invalid_request(format!("{field_name} 序列化失败: {err}")))
}

fn build_aggregate_rollback_body(
    checkpoint: &Value,
    is_config: bool,
) -> Result<Bytes, GatewayError> {
    build_aggregate_rollback_body_with_options(checkpoint, is_config, false)
}

/// Build the user half of an aggregate rollback without carrying wallet data.
/// Wallet rows are compensated through the journal's owner-checked CAS path;
/// feeding the checkpoint wallet fields back through the regular importer
/// would otherwise perform an unconditional overwrite and could erase a
/// concurrent recharge or adjustment.
fn build_aggregate_users_rollback_body(checkpoint: &Value) -> Result<Bytes, GatewayError> {
    let mut object = checkpoint.as_object().cloned().ok_or_else(|| {
        GatewayError::Internal("aggregate rollback checkpoint must be a JSON object".to_string())
    })?;
    object.insert("merge_mode".to_string(), json!("overwrite"));

    // Usage aggregates and denormalized counters are runtime state. Replaying them during
    // compensation could erase requests completed while the failed import was running.
    object.remove("usage_aggregates");

    if let Some(users) = object.get_mut("users") {
        let Value::Array(users) = users else {
            return Err(GatewayError::Internal(
                "aggregate users rollback checkpoint users must be an array".to_string(),
            ));
        };
        for (index, user) in users.iter_mut().enumerate() {
            let Some(user) = user.as_object_mut() else {
                return Err(GatewayError::Internal(format!(
                    "aggregate users rollback checkpoint users[{index}] must be an object"
                )));
            };
            user.remove("request_count");
            user.remove("total_tokens");
            user.remove("wallet");
            if let Some(api_keys) = user.get_mut("api_keys") {
                let Value::Array(api_keys) = api_keys else {
                    return Err(GatewayError::Internal(format!(
                        "aggregate users rollback checkpoint users[{index}].api_keys must be an array"
                    )));
                };
                for (key_index, api_key) in api_keys.iter_mut().enumerate() {
                    let Some(api_key) = api_key.as_object_mut() else {
                        return Err(GatewayError::Internal(format!(
                            "aggregate users rollback checkpoint users[{index}].api_keys[{key_index}] must be an object"
                        )));
                    };
                    api_key.remove("total_requests");
                    api_key.remove("total_tokens");
                    api_key.remove("total_cost_usd");
                    api_key.remove("wallet");
                }
            }
        }
    }

    if let Some(standalone_keys) = object.get_mut("standalone_keys") {
        let Value::Array(standalone_keys) = standalone_keys else {
            return Err(GatewayError::Internal(
                "aggregate users rollback checkpoint standalone_keys must be an array".to_string(),
            ));
        };
        for (index, key) in standalone_keys.iter_mut().enumerate() {
            let Some(key) = key.as_object_mut() else {
                return Err(GatewayError::Internal(format!(
                    "aggregate users rollback checkpoint standalone_keys[{index}] must be an object"
                )));
            };
            key.remove("total_requests");
            key.remove("total_tokens");
            key.remove("total_cost_usd");
            key.remove("wallet");
        }
    }

    serde_json::to_vec(&Value::Object(object))
        .map(Bytes::from)
        .map_err(|err| {
            GatewayError::Internal(format!(
                "serialize aggregate users rollback checkpoint: {err}"
            ))
        })
}

fn build_aggregate_rollback_body_with_options(
    checkpoint: &Value,
    is_config: bool,
    skip_ldap_config: bool,
) -> Result<Bytes, GatewayError> {
    let mut object = checkpoint.as_object().cloned().ok_or_else(|| {
        GatewayError::Internal("aggregate rollback checkpoint must be a JSON object".to_string())
    })?;
    object.insert("merge_mode".to_string(), json!("overwrite"));
    if is_config {
        // Proxy nodes are deployment-local and are deliberately not restored by the admin
        // config importer. Excluding them also prevents a rollback from changing local routing
        // resources while it is restoring the portable catalog.
        object.insert("proxy_nodes".to_string(), Value::Array(Vec::new()));
        if skip_ldap_config {
            // A failed owner-checked LDAP delete means another writer may have changed or
            // recreated the row. Missing the field makes the config importer leave that row
            // untouched while still restoring every unrelated config section.
            object.remove("ldap_config");
        }
    }
    serde_json::to_vec(&Value::Object(object))
        .map(Bytes::from)
        .map_err(|err| {
            GatewayError::Internal(format!("serialize aggregate rollback checkpoint: {err}"))
        })
}

fn aggregate_rollback_failure(
    phase: &str,
    original_kind: &'static str,
    rollback: GatewayError,
) -> GatewayError {
    let rollback_kind = gateway_error_kind(&rollback);
    tracing::error!(
        phase,
        original_kind,
        rollback_kind,
        "aggregate system import failed and compensation failed"
    );
    GatewayError::Internal(format!(
        "aggregate system import compensation failed in {phase}"
    ))
}

fn gateway_error_kind(error: &GatewayError) -> &'static str {
    match error {
        GatewayError::UpstreamUnavailable { .. } => "upstream_unavailable",
        GatewayError::ControlUnavailable { .. } => "control_unavailable",
        GatewayError::LocalExecutionPlanningTimeout { .. } => "planning_timeout",
        GatewayError::AdmissionTimeout { .. } => "admission_timeout",
        GatewayError::Client { .. } => "client",
        GatewayError::PlanUsageLimited(_) => "plan_usage_limited",
        GatewayError::LastActiveAdminUpdateDenied => "last_admin_update_denied",
        GatewayError::LastActiveAdminDeleteDenied => "last_admin_delete_denied",
        GatewayError::Internal(_) => "internal",
    }
}

fn aggregate_rollback_error(
    phase: &str,
    original: GatewayError,
    rollback: GatewayError,
) -> GatewayError {
    aggregate_rollback_failure(phase, gateway_error_kind(&original), rollback)
}

fn aggregate_rollback_http_error(
    phase: &str,
    original: &(http::StatusCode, Value),
    rollback: GatewayError,
) -> GatewayError {
    let original_kind = if original.0.is_client_error() {
        "http_client_error"
    } else {
        "http_server_error"
    };
    aggregate_rollback_failure(phase, original_kind, rollback)
}

fn combine_rollback_results(
    first: Result<(), GatewayError>,
    second: Result<(), GatewayError>,
    phase: &str,
) -> Result<(), GatewayError> {
    match (first, second) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(_), Err(_)) => Err(GatewayError::Internal(format!(
            "multiple aggregate rollback operations failed in {phase}"
        ))),
    }
}

fn trim_required(value: &str, field_name: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("{field_name} 不能为空"));
    }
    Ok(trimmed.to_string())
}

fn normalize_optional_price(value: Option<f64>, field_name: &str) -> Result<Option<f64>, String> {
    let value = admin_provider_models_write_pure::normalize_optional_price(value, field_name)?;
    if let Some(value) = value {
        validate_imported_decimal_storage(value, field_name)?;
    }
    Ok(value)
}

fn normalize_supported_capabilities(value: Option<Vec<String>>) -> Option<Value> {
    normalize_string_list(value).map(|items| json!(items))
}

fn normalize_import_auth_config(value: Option<Value>) -> Result<Option<Value>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    match value {
        Value::Null => Ok(None),
        Value::String(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return Ok(None);
            }
            let parsed = serde_json::from_str::<Value>(trimmed)
                .map_err(|_| "auth_config 必须是 JSON 对象或 JSON 字符串".to_string())?;
            normalize_json_object(Some(parsed), "auth_config")
        }
        other => normalize_json_object(Some(other), "auth_config"),
    }
}

fn is_imported_redacted_secret(value: &str) -> bool {
    matches!(value.trim(), "***" | "********")
}

fn imported_value_contains_redacted_secret(value: &Value) -> bool {
    match value {
        Value::String(value) => is_imported_redacted_secret(value),
        Value::Array(items) => items.iter().any(imported_value_contains_redacted_secret),
        Value::Object(object) => object.values().any(imported_value_contains_redacted_secret),
        _ => false,
    }
}

fn imported_config_credentials_not_exported(root: &Map<String, Value>) -> Result<bool, String> {
    let Some(value) = root.get("credential_state") else {
        return Ok(false);
    };
    match value {
        Value::String(value) if value.trim() == ADMIN_SYSTEM_EXPORT_CREDENTIALS_NOT_EXPORTED => {
            Ok(true)
        }
        _ => Err("配置导出 credential_state 无效".to_string()),
    }
}

fn contains_rule_redaction_marker(value: &Value) -> bool {
    match value {
        Value::Array(items) => items.iter().any(contains_rule_redaction_marker),
        Value::Object(object) => {
            object.iter().any(|(key, value)| {
                matches!(
                    key.as_str(),
                    "has_value" | "has_pattern" | "has_replacement"
                ) && value.as_bool() == Some(true)
            }) || object.values().any(contains_rule_redaction_marker)
        }
        _ => false,
    }
}

fn strip_imported_redaction_placeholders(value: Value) -> Option<Value> {
    match value {
        Value::String(value) if is_imported_redacted_secret(&value) => None,
        Value::Array(items) => Some(Value::Array(
            items
                .into_iter()
                .filter(|item| !contains_rule_redaction_marker(item))
                .filter_map(strip_imported_redaction_placeholders)
                .collect(),
        )),
        Value::Object(object) => Some(Value::Object(
            object
                .into_iter()
                .filter(|(key, _)| {
                    !matches!(
                        key.as_str(),
                        "has_credentials" | "has_value" | "has_pattern" | "has_replacement"
                    )
                })
                .filter_map(|(key, value)| {
                    strip_imported_redaction_placeholders(value).map(|value| (key, value))
                })
                .collect(),
        )),
        value => Some(value),
    }
}

fn prepare_imported_secret_safe_json(
    existing: Option<&Value>,
    incoming: Option<Value>,
    credentials_not_exported: bool,
) -> Option<Value> {
    let incoming = incoming?;
    if !credentials_not_exported {
        return Some(incoming);
    }
    if let Some(existing) = existing {
        return strip_imported_redaction_placeholders(admin_restore_secret_safe_json(
            Some(existing),
            &incoming,
        ));
    }
    strip_imported_redaction_placeholders(incoming)
}

fn prepare_imported_secret_safe_rules(
    existing: Option<&Value>,
    incoming: Option<Value>,
    credentials_not_exported: bool,
    restore: fn(Option<&Value>, &Value) -> Value,
) -> Option<Value> {
    let incoming = incoming?;
    if !credentials_not_exported {
        return Some(incoming);
    }

    let restored = existing
        .map(|existing| restore(Some(existing), &incoming))
        .unwrap_or_else(|| incoming.clone());
    let (Some(incoming_rules), Some(restored_rules)) = (incoming.as_array(), restored.as_array())
    else {
        return strip_imported_redaction_placeholders(restored);
    };

    Some(Value::Array(
        incoming_rules
            .iter()
            .zip(restored_rules)
            .filter_map(|(incoming_rule, restored_rule)| {
                let contains_placeholder = contains_rule_redaction_marker(incoming_rule)
                    || imported_value_contains_redacted_secret(incoming_rule);
                if contains_placeholder
                    && (existing.is_none()
                        || imported_value_contains_redacted_secret(restored_rule))
                {
                    return None;
                }
                strip_imported_redaction_placeholders(restored_rule.clone())
            })
            .collect(),
    ))
}

fn prepare_imported_secret_safe_header_rules(
    existing: Option<&Value>,
    incoming: Option<Value>,
    credentials_not_exported: bool,
) -> Option<Value> {
    prepare_imported_secret_safe_rules(
        existing,
        incoming,
        credentials_not_exported,
        admin_restore_secret_safe_header_rules,
    )
}

fn prepare_imported_secret_safe_body_rules(
    existing: Option<&Value>,
    incoming: Option<Value>,
    credentials_not_exported: bool,
) -> Option<Value> {
    prepare_imported_secret_safe_rules(
        existing,
        incoming,
        credentials_not_exported,
        admin_restore_secret_safe_body_rules,
    )
}

fn prepare_imported_secret_safe_proxy(
    existing: Option<&Value>,
    incoming: Option<Value>,
    credentials_not_exported: bool,
    node_id_map: &BTreeMap<String, String>,
) -> Option<Value> {
    let incoming = remap_import_proxy(incoming, node_id_map)?;
    let incoming = if credentials_not_exported {
        if existing.is_some() {
            admin_restore_secret_safe_proxy(existing, &incoming)
        } else {
            strip_imported_redaction_placeholders(incoming)?
        }
    } else {
        incoming
    };
    Some(incoming)
}

fn prepare_imported_provider_config(
    state: &AdminAppState<'_>,
    provider_id: &str,
    fallback_base_url: Option<&str>,
    existing: Option<&Value>,
    incoming: Option<Value>,
    credentials_not_exported: bool,
) -> Result<Option<Value>, String> {
    if credentials_not_exported {
        return normalize_json_object(
            prepare_imported_secret_safe_json(existing, incoming, true),
            "config",
        );
    }
    encrypt_imported_provider_config(state, provider_id, fallback_base_url, incoming)
}

fn imported_provider_ops_fallback_base_url(raw_provider: &Map<String, Value>) -> Option<String> {
    raw_provider
        .get("endpoints")
        .and_then(Value::as_array)
        .and_then(|endpoints| endpoints.first())
        .and_then(Value::as_object)
        .and_then(|endpoint| endpoint.get("base_url"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| {
            raw_provider
                .get("website")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
        })
}

fn imported_provider_key_credentials_not_exported(item: &ImportedProviderKey) -> bool {
    item.credential_state.as_deref().map(str::trim)
        == Some(ADMIN_SYSTEM_EXPORT_CREDENTIALS_NOT_EXPORTED)
}

fn validate_imported_provider_key_credential_state(
    item: &ImportedProviderKey,
) -> Result<bool, String> {
    let credentials_not_exported = imported_provider_key_credentials_not_exported(item);
    if item.credential_state.is_some() && !credentials_not_exported {
        return Err("Provider Key credential_state 无效".to_string());
    }
    if credentials_not_exported
        && (item
            .api_key
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
            || item.auth_config.is_some())
    {
        return Err("credential_state=not_exported 的 Provider Key 不允许包含凭据字段".to_string());
    }
    if item
        .api_key
        .as_deref()
        .is_some_and(is_imported_redacted_secret)
        || item
            .auth_config
            .as_ref()
            .is_some_and(imported_value_contains_redacted_secret)
    {
        return Err("Provider Key 脱敏占位符不能作为凭据导入".to_string());
    }
    Ok(credentials_not_exported)
}

fn encrypt_imported_provider_config(
    state: &AdminAppState<'_>,
    provider_id: &str,
    fallback_base_url: Option<&str>,
    config: Option<Value>,
) -> Result<Option<Value>, String> {
    let Some(mut config) = normalize_json_object(config, "config")? else {
        return Ok(None);
    };
    let Some(provider_ops) = config
        .get_mut("provider_ops")
        .and_then(Value::as_object_mut)
    else {
        return Ok(Some(config));
    };
    let raw_base_url = provider_ops
        .get("base_url")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or(fallback_base_url)
        .ok_or_else(|| "RecoveryBackup Provider Ops 缺少 base_url".to_string())?;
    let destination =
        canonicalize_provider_ops_base_url(raw_base_url).map_err(ToString::to_string)?;
    provider_ops.insert(
        "base_url".to_string(),
        Value::String(destination.base_url().to_string()),
    );
    let binding = provider_ops_credential_binding_from_config(
        provider_id,
        provider_ops,
        destination.base_url(),
    )
    .map_err(ToString::to_string)?;
    let Some(credentials) = provider_ops
        .get_mut("connector")
        .and_then(Value::as_object_mut)
        .and_then(|connector| connector.get_mut("credentials"))
        .and_then(Value::as_object_mut)
    else {
        return Ok(Some(config));
    };

    for field in PROVIDER_OPS_TRANSIENT_SECRET_FIELDS
        .iter()
        .chain(PROVIDER_OPS_TRANSIENT_METADATA_FIELDS)
    {
        credentials.remove(*field);
    }
    for field in PROVIDER_OPS_PERSISTENT_SECRET_FIELDS {
        let Some(Value::String(raw)) = credentials.get_mut(*field) else {
            continue;
        };
        if raw.is_empty() {
            continue;
        }
        if is_imported_redacted_secret(raw) {
            return Err("Provider Ops 脱敏占位符不能作为凭据导入".to_string());
        }
        if raw.starts_with("aether-") {
            return Err(
                "RecoveryBackup Provider Ops 凭据必须是明文，不能包含密文 envelope".to_string(),
            );
        }
        let encrypted = seal_provider_ops_credential(state.app(), &binding, field, raw)
            .map_err(ToString::to_string)?;
        *raw = encrypted;
    }

    Ok(Some(config))
}

fn remap_import_proxy(
    proxy: Option<Value>,
    node_id_map: &BTreeMap<String, String>,
) -> Option<Value> {
    let proxy = match proxy {
        Some(Value::Object(map)) if map.is_empty() => return None,
        Some(Value::Object(map)) => map,
        _ => return None,
    };
    let Some(Value::String(old_node_id)) = proxy.get("node_id") else {
        return Some(Value::Object(proxy));
    };
    let old_node_id = old_node_id.trim();
    if old_node_id.is_empty() {
        return Some(Value::Object(proxy));
    }
    let new_node_id = node_id_map.get(old_node_id)?;
    let mut remapped = proxy;
    remapped.insert("node_id".to_string(), json!(new_node_id));
    Some(Value::Object(remapped))
}

fn normalize_import_endpoint_format(value: &str) -> Result<String, String> {
    let normalized = match value.trim().to_ascii_lowercase().as_str() {
        "openai:cli" => "openai:responses",
        "openai:compact" => "openai:responses:compact",
        "openai_image" | "images" | "image" | "/v1/images/generations" | "/v1/images/edits" => {
            "openai:image"
        }
        "claude:chat" | "claude:cli" => "claude:messages",
        "gemini:chat" | "gemini:cli" => "gemini:generate_content",
        _ => value.trim(),
    };
    admin_endpoint_signature_parts(normalized)
        .map(|(signature, _, _)| signature.to_string())
        .ok_or_else(|| format!("无效的 api_format: {value}"))
}

fn fixed_provider_import_endpoint_supported(provider_type: &str, api_format: &str) -> bool {
    crate::provider_transport::provider_types::fixed_provider_template(provider_type).is_none()
        || crate::provider_transport::provider_types::fixed_provider_endpoint_template_by_api_format(
            provider_type,
            api_format,
        )
        .is_some()
}

fn normalize_import_key_formats(
    item: &ImportedProviderKey,
    provider_endpoint_formats: &BTreeSet<String>,
) -> (Vec<String>, Vec<String>) {
    let source = item
        .api_formats
        .clone()
        .filter(|items| !items.is_empty())
        .or_else(|| {
            item.supported_endpoints
                .clone()
                .filter(|items| !items.is_empty())
        })
        .unwrap_or_else(|| provider_endpoint_formats.iter().cloned().collect());

    let mut normalized = Vec::new();
    let mut missing = Vec::new();
    let mut seen = BTreeSet::new();
    for raw in source {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(api_format) = normalize_import_endpoint_format(trimmed) else {
            missing.push(trimmed.to_string());
            continue;
        };
        if !seen.insert(api_format.clone()) {
            continue;
        }
        if !provider_endpoint_formats.is_empty() && !provider_endpoint_formats.contains(&api_format)
        {
            missing.push(api_format);
            continue;
        }
        normalized.push(api_format);
    }

    (normalized, missing)
}

fn imported_key_auth_type(item: &ImportedProviderKey) -> String {
    item.auth_type
        .as_deref()
        .unwrap_or("api_key")
        .trim()
        .to_ascii_lowercase()
}

fn imported_service_account_email(config: Option<&Value>) -> Option<String> {
    match config {
        Some(Value::Object(map)) => map
            .get("client_email")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned),
        Some(Value::String(raw)) => serde_json::from_str::<Value>(raw)
            .ok()
            .and_then(|value| imported_service_account_email(Some(&value))),
        _ => None,
    }
}

fn imported_provider_credential_identity(
    imported_key: &ImportedProviderKey,
    auth_type: &str,
    normalized_auth_config: Option<&Value>,
) -> Option<String> {
    if matches!(auth_type, "api_key" | "bearer") {
        return imported_key
            .api_key
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| format!("secret:{value}"));
    }
    if matches!(auth_type, "service_account" | "vertex_ai") {
        return imported_service_account_email(normalized_auth_config)
            .map(|email| format!("service_account:{email}"));
    }
    None
}

fn build_import_key_match_name(item: &ImportedProviderKey) -> Option<String> {
    item.name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn normalize_selected_import_key_format(
    value: &str,
    allowed_formats: &BTreeSet<String>,
) -> Option<String> {
    let normalized = normalize_import_endpoint_format(value).ok()?;
    allowed_formats.contains(&normalized).then_some(normalized)
}

fn normalize_import_key_format_scoped_list(
    value: Option<&Value>,
    normalized_api_formats: &[String],
) -> Option<Value> {
    let value = value?;
    let Value::Array(items) = value else {
        return Some(value.clone());
    };
    let allowed_formats = normalized_api_formats
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut seen = BTreeSet::new();
    let mut normalized = Vec::new();
    for item in items {
        let Some(raw) = item
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        let Some(api_format) = normalize_selected_import_key_format(raw, &allowed_formats) else {
            continue;
        };
        if seen.insert(api_format.clone()) {
            normalized.push(json!(api_format));
        }
    }
    Some(Value::Array(normalized))
}

fn normalize_import_key_format_scoped_object(
    value: Option<&Value>,
    normalized_api_formats: &[String],
) -> Option<Value> {
    let value = value?;
    let Value::Object(map) = value else {
        return Some(value.clone());
    };
    let allowed_formats = normalized_api_formats
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut normalized = Map::new();
    for (key, value) in map {
        let Some(api_format) = normalize_selected_import_key_format(key, &allowed_formats) else {
            continue;
        };
        normalized.insert(api_format, value.clone());
    }
    Some(Value::Object(normalized))
}

fn normalize_import_key_raw_payload(
    raw_key: &Map<String, Value>,
    auth_type: &str,
    normalized_api_formats: &[String],
    normalized_auth_config: Option<Value>,
    credentials_not_exported: bool,
) -> Map<String, Value> {
    let mut payload = raw_key.clone();
    payload.remove("credential_state");
    if credentials_not_exported {
        payload.remove("api_key");
        payload.remove("auth_config");
    }
    if auth_type == "oauth" {
        payload.remove("api_key");
    }
    payload.insert("api_formats".to_string(), json!(normalized_api_formats));
    if let Some(auth_type_by_format) = normalize_import_key_format_scoped_object(
        raw_key.get("auth_type_by_format"),
        normalized_api_formats,
    ) {
        payload.insert("auth_type_by_format".to_string(), auth_type_by_format);
    }
    if let Some(allow_auth_channel_mismatch_formats) = normalize_import_key_format_scoped_list(
        raw_key.get("allow_auth_channel_mismatch_formats"),
        normalized_api_formats,
    ) {
        payload.insert(
            "allow_auth_channel_mismatch_formats".to_string(),
            allow_auth_channel_mismatch_formats,
        );
    }
    if !credentials_not_exported {
        if let Some(auth_config) = normalized_auth_config {
            payload.insert("auth_config".to_string(), auth_config);
        } else if raw_key.contains_key("auth_config") {
            payload.insert("auth_config".to_string(), Value::Null);
        }
    }
    payload
}

fn apply_imported_oauth_key_credentials(
    state: &AdminAppState<'_>,
    provider_type: &str,
    previous_codex_credential_generation: Option<&str>,
    raw_key: &Map<String, Value>,
    normalized_auth_config: Option<&Value>,
    record: &mut aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey,
) -> Result<bool, String> {
    let previous_encrypted_api_key = record.encrypted_api_key.clone();
    let previous_encrypted_auth_config = record.encrypted_auth_config.clone();
    let mut credentials_supplied = false;
    let mut api_key_supplied = false;
    if let Some(api_key_value) = raw_key.get("api_key") {
        let plaintext = api_key_value
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        if plaintext.is_some_and(is_imported_redacted_secret) {
            return Err("Provider Key 脱敏占位符不能作为凭据导入".to_string());
        }
        record.encrypted_api_key = match plaintext {
            Some(plaintext) => {
                credentials_supplied = true;
                api_key_supplied = true;
                Some(
                    state
                        .app()
                        .seal_provider_catalog_key_api_key(
                            &record.provider_id,
                            &record.id,
                            plaintext,
                        )
                        .map_err(GatewayError::into_message)?,
                )
            }
            None => None,
        };
    }

    if raw_key.contains_key("auth_config") {
        record.encrypted_auth_config = match normalized_auth_config {
            Some(auth_config) => {
                credentials_supplied |= imported_oauth_auth_config_has_credentials(auth_config);
                let plaintext =
                    serde_json::to_string(auth_config).map_err(|err| err.to_string())?;
                Some(
                    state
                        .app()
                        .seal_provider_catalog_key_auth_config(
                            &record.provider_id,
                            &record.id,
                            &plaintext,
                        )
                        .map_err(GatewayError::into_message)?,
                )
            }
            None => None,
        };
    }
    record.expires_at_unix_secs = imported_oauth_expiry_after_import(
        record.expires_at_unix_secs,
        raw_key.contains_key("auth_config"),
        normalized_auth_config,
        api_key_supplied,
    );

    let credential_material_changed = record.encrypted_api_key != previous_encrypted_api_key
        || record.encrypted_auth_config != previous_encrypted_auth_config;
    if credentials_supplied {
        record.oauth_invalid_at_unix_secs = None;
        record.oauth_invalid_reason = None;
    }
    if credential_material_changed {
        ensure_codex_credential_generation_rotated(
            record,
            provider_type,
            previous_codex_credential_generation,
        );
    }

    Ok(credentials_supplied)
}

fn imported_oauth_auth_config_has_credentials(value: &Value) -> bool {
    const CREDENTIAL_FIELDS: &[&str] = &[
        "access_token",
        "accessToken",
        "api_key",
        "apiKey",
        "auth_token",
        "authToken",
        "cf_clearance",
        "cfClearance",
        "cf_cookies",
        "cfCookies",
        "cookie",
        "cookieHeader",
        "cookies",
        "id_token",
        "idToken",
        "refresh_token",
        "refreshToken",
        "session_token",
        "sessionToken",
        "sso_rw_token",
        "ssoRwToken",
        "sso_token",
        "ssoToken",
        "token",
    ];

    match value {
        Value::Object(object) => object.iter().any(|(key, value)| {
            (CREDENTIAL_FIELDS.contains(&key.as_str()) && imported_credential_value_present(value))
                || imported_oauth_auth_config_has_credentials(value)
        }),
        Value::Array(items) => items.iter().any(imported_oauth_auth_config_has_credentials),
        _ => false,
    }
}

fn imported_credential_value_present(value: &Value) -> bool {
    match value {
        Value::String(value) => !value.trim().is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(object) => !object.is_empty(),
        _ => false,
    }
}

fn imported_oauth_expires_at_unix_secs(normalized_auth_config: Option<&Value>) -> Option<u64> {
    let object = normalized_auth_config?.as_object()?;
    for field in ["expires_at", "expiresAt", "expiry", "exp"] {
        let Some(value) = object.get(field) else {
            continue;
        };
        match value {
            Value::Number(number) => {
                if let Some(expires_at) = number.as_u64() {
                    return Some(expires_at);
                }
            }
            Value::String(raw) => {
                if let Ok(expires_at) = raw.trim().parse::<u64>() {
                    return Some(expires_at);
                }
            }
            _ => {}
        }
    }
    None
}

fn imported_oauth_expiry_after_import(
    current: Option<u64>,
    auth_config_present: bool,
    normalized_auth_config: Option<&Value>,
    api_key_supplied: bool,
) -> Option<u64> {
    if auth_config_present {
        imported_oauth_expires_at_unix_secs(normalized_auth_config)
    } else if api_key_supplied {
        None
    } else {
        current
    }
}

async fn seed_imported_oauth_pool_score(
    state: &AdminAppState<'_>,
    provider_id: &str,
    key: &aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey,
    now_unix_secs: u64,
) -> Result<(), GatewayError> {
    let provider_id = provider_id.to_string();
    let provider = state
        .read_provider_catalog_providers_by_ids(std::slice::from_ref(&provider_id))
        .await?
        .pop();
    let Some(provider) = provider else {
        return Ok(());
    };
    let Some(pool_config) = admin_provider_pool_config(&provider) else {
        return Ok(());
    };
    if !key.is_active || key.provider_id != provider.id {
        return Ok(());
    }

    let upsert = build_provider_key_pool_score_upsert(
        key,
        provider.provider_type.as_str(),
        None,
        now_unix_secs,
        pool_config.score_rules,
    );
    state
        .app()
        .data
        .upsert_pool_member_score_with_mode(upsert, PoolMemberScoreUpsertMode::OAuthRecovery)
        .await
        .map_err(|error| {
            GatewayError::Internal(format!(
                "failed to recover OAuth pool score for key '{}': {error}",
                key.id
            ))
        })?;
    Ok(())
}

fn build_import_provider_model_record(
    provider_id: &str,
    existing_id: Option<&str>,
    existing: Option<&aether_data_contracts::repository::global_models::StoredAdminProviderModel>,
    global_model_id: &str,
    item: &ImportedProviderModel,
    credentials_not_exported: bool,
) -> Result<UpsertAdminProviderModelRecord, String> {
    let provider_model_name = trim_required(&item.provider_model_name, "provider_model_name")?;
    let provider_model_mappings = normalize_json_array(
        prepare_imported_secret_safe_json(
            existing.and_then(|model| model.provider_model_mappings.as_ref()),
            item.provider_model_mappings.clone(),
            credentials_not_exported,
        ),
        "provider_model_mappings",
    )?;
    let price_per_request = normalize_optional_price(item.price_per_request, "price_per_request")?;
    let tiered_pricing = normalize_json_object(
        prepare_imported_secret_safe_json(
            existing.and_then(|model| model.tiered_pricing.as_ref()),
            item.tiered_pricing.clone(),
            credentials_not_exported,
        ),
        "tiered_pricing",
    )?;
    let config = normalize_json_object(
        prepare_imported_secret_safe_json(
            existing.and_then(|model| model.config.as_ref()),
            item.config.clone(),
            credentials_not_exported,
        ),
        "config",
    )?;

    UpsertAdminProviderModelRecord::new(
        existing_id
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| Uuid::new_v4().to_string()),
        provider_id.to_string(),
        global_model_id.to_string(),
        provider_model_name,
        provider_model_mappings,
        price_per_request,
        tiered_pricing,
        item.supports_vision,
        item.supports_function_calling,
        item.supports_streaming,
        item.supports_extended_thinking,
        item.supports_image_generation,
        item.is_active,
        true,
        config,
    )
    .map_err(|err| err.to_string())
}

fn build_imported_oauth_provider_record(
    oauth_provider: &ImportedOAuthProvider,
    client_secret_encrypted: EncryptedSecretUpdate,
) -> Result<UpsertOAuthProviderConfigRecord, String> {
    let record = UpsertOAuthProviderConfigRecord {
        provider_type: trim_required(&oauth_provider.provider_type, "provider_type")?,
        display_name: trim_required(&oauth_provider.display_name, "display_name")?,
        client_id: trim_required(&oauth_provider.client_id, "client_id")?,
        client_secret_encrypted,
        authorization_url_override: oauth_provider
            .authorization_url_override
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned),
        token_url_override: oauth_provider
            .token_url_override
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned),
        userinfo_url_override: oauth_provider
            .userinfo_url_override
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned),
        scopes: normalize_string_list(oauth_provider.scopes.clone()),
        redirect_uri: trim_required(&oauth_provider.redirect_uri, "redirect_uri")?,
        frontend_callback_url: trim_required(
            &oauth_provider.frontend_callback_url,
            "frontend_callback_url",
        )?,
        attribute_mapping: normalize_json_object(
            oauth_provider.attribute_mapping.clone(),
            "attribute_mapping",
        )?,
        extra_config: normalize_json_object(oauth_provider.extra_config.clone(), "extra_config")?,
        icon_url: None,
        is_enabled: oauth_provider.is_enabled,
    };
    record.validate().map_err(|err| err.to_string())?;
    Ok(record)
}

fn is_custom_identity_oauth_provider_type(provider_type: &str) -> bool {
    let provider_type = provider_type.trim().to_ascii_lowercase();
    provider_type == "custom_oidc"
        || provider_type.starts_with("custom_oidc_")
        || provider_type.starts_with("custom_")
        || provider_type.starts_with("oidc_")
}

fn legacy_custom_oauth_provider_type(provider_type: &str) -> String {
    let normalized = provider_type.trim().to_ascii_lowercase();
    let mut suffix = String::with_capacity(normalized.len());
    let mut previous_was_separator = false;
    for character in normalized.chars() {
        if character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-' {
            suffix.push(character);
            previous_was_separator = false;
        } else if !previous_was_separator {
            suffix.push('_');
            previous_was_separator = true;
        }
    }
    let suffix = suffix.trim_matches(['_', '-']);
    let candidate = format!("custom_{suffix}");
    if !suffix.is_empty() && candidate.len() <= 64 {
        candidate
    } else {
        let digest = format!("{:x}", Sha256::digest(normalized.as_bytes()));
        format!("custom_legacy_{}", &digest[..16])
    }
}

fn legacy_oauth_endpoint_domains(
    oauth_provider: &ImportedOAuthProvider,
) -> Result<Vec<String>, String> {
    let mut domains = BTreeSet::new();
    for (field, value) in [
        (
            "authorization_url_override",
            oauth_provider.authorization_url_override.as_deref(),
        ),
        (
            "token_url_override",
            oauth_provider.token_url_override.as_deref(),
        ),
        (
            "userinfo_url_override",
            oauth_provider.userinfo_url_override.as_deref(),
        ),
    ] {
        let value = value
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| format!("legacy custom OAuth provider is missing {field}"))?;
        let parsed = url::Url::parse(value)
            .map_err(|_| format!("legacy custom OAuth provider has invalid {field}"))?;
        let host = parsed
            .host_str()
            .map(|host| host.trim_end_matches('.').to_ascii_lowercase())
            .filter(|host| !host.is_empty())
            .ok_or_else(|| format!("legacy custom OAuth provider has invalid {field}"))?;
        domains.insert(host);
    }
    Ok(domains.into_iter().collect())
}

fn normalize_legacy_imported_oauth_provider(
    mut oauth_provider: ImportedOAuthProvider,
    source_version: &str,
) -> Result<ImportedOAuthProvider, String> {
    if !matches!(source_version.trim(), "2.0" | "2.1" | "2.2") {
        return Ok(oauth_provider);
    }

    let original_provider_type =
        trim_required(&oauth_provider.provider_type, "provider_type")?.to_ascii_lowercase();
    if original_provider_type == "linuxdo" {
        oauth_provider.provider_type = original_provider_type;
        return Ok(oauth_provider);
    }

    let mut requires_review = false;
    if is_custom_identity_oauth_provider_type(&original_provider_type) {
        oauth_provider.provider_type = original_provider_type;
    } else {
        oauth_provider.provider_type = legacy_custom_oauth_provider_type(&original_provider_type);
        requires_review = true;
    }

    let mut extra_config = match oauth_provider.extra_config.take() {
        Some(Value::Object(config)) => config,
        Some(_) => return Err("extra_config must be an object".to_string()),
        None => Map::new(),
    };
    let has_allowed_domains = extra_config
        .get("allowed_domains")
        .or_else(|| extra_config.get("oauth_allowed_domains"))
        .and_then(Value::as_array)
        .is_some_and(|domains| !domains.is_empty());
    if !has_allowed_domains {
        extra_config.insert(
            "allowed_domains".to_string(),
            serde_json::to_value(legacy_oauth_endpoint_domains(&oauth_provider)?)
                .map_err(|err| err.to_string())?,
        );
        requires_review = true;
    }
    oauth_provider.extra_config = Some(Value::Object(extra_config));
    if requires_review {
        oauth_provider.is_enabled = false;
    }
    Ok(oauth_provider)
}

fn find_imported_provider_key_index(
    state: &AdminAppState<'_>,
    imported_key: &ImportedProviderKey,
    auth_type: &str,
    normalized_auth_config: Option<&Value>,
    existing_keys: &[aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey],
) -> Result<Option<usize>, String> {
    if auth_type == "api_key" {
        let target_key = imported_key
            .api_key
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        for (index, existing_key) in existing_keys.iter().enumerate() {
            let decrypted_existing = state
                .app()
                .decrypt_provider_catalog_key_api_key(existing_key)
                .map_err(GatewayError::into_message)?;
            if target_key
                .zip(decrypted_existing.as_deref())
                .is_some_and(|(target, decrypted)| decrypted == target)
            {
                return Ok(Some(index));
            }
        }
        Ok(None)
    } else if matches!(auth_type, "service_account" | "vertex_ai") {
        let target_email = imported_service_account_email(normalized_auth_config);
        for (index, existing_key) in existing_keys.iter().enumerate() {
            let existing_email = imported_existing_provider_auth_config(state, existing_key)?
                .and_then(|config| {
                    config
                        .get("client_email")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(ToOwned::to_owned)
                });
            if target_email
                .as_deref()
                .zip(existing_email.as_deref())
                .is_some_and(|(target, existing)| target == existing)
            {
                return Ok(Some(index));
            }
        }
        Ok(None)
    } else {
        Ok(
            build_import_key_match_name(imported_key).and_then(|target_name| {
                existing_keys.iter().position(|existing_key| {
                    existing_key
                        .auth_type
                        .trim()
                        .eq_ignore_ascii_case(auth_type)
                        && existing_key.name == target_name
                })
            }),
        )
    }
}

fn imported_existing_provider_auth_config(
    state: &AdminAppState<'_>,
    key: &aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey,
) -> Result<Option<Map<String, Value>>, String> {
    let Some(plaintext) = state
        .app()
        .decrypt_provider_catalog_key_auth_config(key)
        .map_err(GatewayError::into_message)?
    else {
        return Ok(None);
    };
    let value = serde_json::from_str::<Value>(&plaintext).map_err(|_| {
        format!(
            "Provider Key '{}' 已保存的 auth_config 不是有效 JSON",
            key.name
        )
    })?;
    value.as_object().cloned().map(Some).ok_or_else(|| {
        format!(
            "Provider Key '{}' 已保存的 auth_config 不是 JSON 对象",
            key.name
        )
    })
}

fn prevalidate_imported_provider_key_uniqueness(
    state: &AdminAppState<'_>,
    imported_key: &ImportedProviderKey,
    auth_type: &str,
    normalized_auth_config: Option<&Value>,
    existing_keys: &[aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey],
) -> Result<(), String> {
    if matches!(auth_type, "api_key" | "bearer") {
        let Some(target_key) = imported_key
            .api_key
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return Ok(());
        };
        let target_key = target_key.to_string();
        for existing in existing_keys.iter().filter(|existing| {
            matches!(
                existing.auth_type.trim().to_ascii_lowercase().as_str(),
                "api_key" | "bearer"
            )
        }) {
            let Some(decrypted) = state
                .app()
                .decrypt_provider_catalog_key_api_key(existing)
                .map_err(GatewayError::into_message)?
            else {
                continue;
            };
            if decrypted != "__placeholder__" && decrypted == target_key {
                return Err(format!(
                    "该 API Key 已存在于当前 Provider 中（名称: {}）",
                    existing.name
                ));
            }
        }
    }

    if auth_type == "service_account" {
        let Some(target_email) = imported_service_account_email(normalized_auth_config) else {
            return Ok(());
        };
        let target_email = target_email.to_string();
        for existing in existing_keys.iter().filter(|existing| {
            matches!(
                existing.auth_type.trim().to_ascii_lowercase().as_str(),
                "service_account" | "vertex_ai"
            )
        }) {
            let Some(existing_email) = imported_existing_provider_auth_config(state, existing)?
                .and_then(|config| {
                    config
                        .get("client_email")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(ToOwned::to_owned)
                })
            else {
                continue;
            };
            if existing_email == target_email {
                return Err(format!(
                    "该 Service Account ({target_email}) 已存在于当前 Provider 中（名称: {}）",
                    existing.name
                ));
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Default, serde::Serialize)]
struct AdminSystemUsersImportStats {
    user_groups: AdminSystemConfigImportCounter,
    users: AdminSystemConfigImportCounter,
    api_keys: AdminSystemConfigImportCounter,
    standalone_keys: AdminSystemConfigImportCounter,
    #[serde(skip_serializing_if = "Option::is_none")]
    usage_aggregates: Option<AdminSystemUsageAggregateImportSummary>,
    errors: Vec<String>,
}

#[derive(Debug, Clone)]
struct ImportedWalletTarget {
    recharge_balance: f64,
    gift_balance: f64,
    limit_mode: String,
    currency: String,
    status: String,
    total_recharged: f64,
    total_consumed: f64,
    total_refunded: f64,
    total_adjusted: f64,
    updated_at_unix_secs: Option<u64>,
}

#[derive(Debug, Clone)]
struct SimulatedImportedUser {
    id: String,
    email: Option<String>,
    username: String,
    role: String,
    existed_before_import: bool,
}

#[derive(Debug, Clone)]
struct SimulatedImportedApiKey {
    owner_id: String,
    is_standalone: bool,
    target_id: String,
    existed_before_import: bool,
}

fn replace_simulated_imported_user(
    users_by_id: &mut BTreeMap<String, SimulatedImportedUser>,
    email_owners: &mut BTreeMap<String, String>,
    username_owners: &mut BTreeMap<String, String>,
    released_emails: &mut BTreeSet<String>,
    released_usernames: &mut BTreeSet<String>,
    user: SimulatedImportedUser,
) {
    if let Some(previous) = users_by_id.remove(&user.id) {
        if previous.email != user.email {
            if previous
                .email
                .as_ref()
                .is_some_and(|email| email_owners.get(email) == Some(&previous.id))
            {
                let previous_email = previous.email.as_deref().unwrap();
                email_owners.remove(previous_email);
                released_emails.insert(previous_email.to_string());
            }
        }
        if previous.username != user.username
            && username_owners.get(&previous.username) == Some(&previous.id)
        {
            username_owners.remove(&previous.username);
            released_usernames.insert(previous.username);
        }
    }
    if let Some(email) = user.email.as_ref() {
        released_emails.remove(email);
        email_owners.insert(email.clone(), user.id.clone());
    }
    released_usernames.remove(&user.username);
    username_owners.insert(user.username.clone(), user.id.clone());
    users_by_id.insert(user.id.clone(), user);
}

fn simulated_imported_user_id_by_identifier(
    email_owners: &BTreeMap<String, String>,
    username_owners: &BTreeMap<String, String>,
    identifier: &str,
) -> Option<String> {
    email_owners
        .get(identifier)
        .or_else(|| username_owners.get(identifier))
        .cloned()
}

fn simulated_imported_user_from_auth_record(
    user: &aether_data::repository::users::StoredUserAuthRecord,
) -> SimulatedImportedUser {
    SimulatedImportedUser {
        id: user.id.clone(),
        email: user.email.clone(),
        username: user.username.clone(),
        role: user.role.clone(),
        existed_before_import: true,
    }
}

fn imported_system_export_version(version: Option<&Value>) -> Result<(u32, u32), String> {
    let Some(Value::String(version)) = version else {
        return Err("version 必须是 x.y 字符串".to_string());
    };
    let version = version.trim();
    if version.is_empty() {
        return Err("version 必须是 x.y 字符串".to_string());
    }
    let mut parts = version.split('.');
    let Some(major) = parts.next().and_then(|value| value.parse::<u32>().ok()) else {
        return Err("version 必须是 x.y 字符串".to_string());
    };
    let Some(minor) = parts.next().and_then(|value| value.parse::<u32>().ok()) else {
        return Err("version 必须是 x.y 字符串".to_string());
    };
    Ok((major, minor))
}

fn validate_imported_system_users_export_version(
    version: Option<&Value>,
) -> Result<(u32, u32), String> {
    let Some(Value::String(raw_version)) = version else {
        return Err("version 必须是 x.y 字符串".to_string());
    };
    let normalized = raw_version.trim();
    if normalized.is_empty() {
        return Err("version 必须是 x.y 字符串".to_string());
    }
    let parsed = imported_system_export_version(version)?;
    if !ADMIN_SYSTEM_USERS_SUPPORTED_VERSIONS.contains(&normalized) {
        return Err(format!(
            "不支持的用户数据版本: {normalized}，支持的版本: {}",
            ADMIN_SYSTEM_USERS_SUPPORTED_VERSIONS.join(", ")
        ));
    }
    Ok(parsed)
}

fn validate_imported_system_users_export_version_for_mode(
    version: Option<&Value>,
    mode: SystemImportMode,
) -> Result<(u32, u32), String> {
    let parsed = validate_imported_system_users_export_version(version)?;
    if mode.restores_credentials() && parsed != ADMIN_SYSTEM_USERS_RECOVERY_IMPORT_VERSION {
        return Err(format!(
            "恢复备份仅支持用户数据版本 {}.{}",
            ADMIN_SYSTEM_USERS_RECOVERY_IMPORT_VERSION.0,
            ADMIN_SYSTEM_USERS_RECOVERY_IMPORT_VERSION.1,
        ));
    }
    Ok(parsed)
}

fn usage_aggregate_import_mode(
    merge_mode: AdminImportMergeMode,
) -> AdminSystemUsageAggregateImportMode {
    match merge_mode {
        AdminImportMergeMode::Skip => AdminSystemUsageAggregateImportMode::Skip,
        AdminImportMergeMode::Overwrite => AdminSystemUsageAggregateImportMode::Overwrite,
        AdminImportMergeMode::Error => AdminSystemUsageAggregateImportMode::Error,
    }
}

fn imported_object_field<'a>(
    value: &'a Value,
    field_name: &str,
) -> Result<&'a Map<String, Value>, String> {
    value
        .as_object()
        .ok_or_else(|| format!("{field_name} 必须是对象"))
}

fn imported_optional_string(value: Option<&Value>) -> Result<Option<String>, String> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(raw)) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                Ok(None)
            } else {
                Ok(Some(trimmed.to_string()))
            }
        }
        _ => Err("字段必须是字符串".to_string()),
    }
}

fn normalize_imported_system_user_role(
    value: Option<&Value>,
    mode: SystemImportMode,
) -> Result<Option<String>, String> {
    let raw_role = imported_optional_string(value)?.unwrap_or_else(|| "user".to_string());
    let role = crate::roles::normalize_assignable_user_role(&raw_role)
        .ok_or_else(|| format!("不支持的用户角色: {raw_role}"))?;

    if crate::roles::is_full_admin_role(role)
        || (crate::roles::is_audit_admin_role(role) && !mode.allows_audit_admin_restore())
    {
        return Ok(None);
    }

    Ok(Some(role.to_string()))
}

fn imported_existing_user_is_protected(role: &str, mode: SystemImportMode) -> bool {
    crate::roles::is_full_admin_role(role)
        || (crate::roles::is_audit_admin_role(role) && !mode.allows_audit_admin_restore())
}

fn validate_rollback_user_source_id(
    mode: SystemImportMode,
    source_user_id: Option<&str>,
) -> Result<(), String> {
    if mode.is_rollback_checkpoint() && source_user_id.is_none() {
        return Err(
            "回滚检查点中的用户必须包含稳定的 users[].id；拒绝按 email/username 猜测用户"
                .to_string(),
        );
    }
    Ok(())
}

const IMPORTED_CREDENTIAL_TOMBSTONE_PREFIX: &str = "$aether-import-revoked$";

fn imported_credential_tombstone(identity: &str) -> String {
    let digest = format!("{:x}", Sha256::digest(identity.as_bytes()));
    let digest_length = 64usize.saturating_sub(IMPORTED_CREDENTIAL_TOMBSTONE_PREFIX.len());
    format!(
        "{IMPORTED_CREDENTIAL_TOMBSTONE_PREFIX}{}",
        &digest[..digest_length]
    )
}

fn imported_api_key_tombstone(api_key_id: &str) -> String {
    imported_credential_tombstone(&format!("api-key-id:{api_key_id}"))
}

fn imported_api_key_id_for_mode(source_api_key_id: Option<&str>, mode: SystemImportMode) -> String {
    if mode.is_rollback_checkpoint() {
        if let Some(source_api_key_id) = source_api_key_id {
            return source_api_key_id.to_string();
        }
    }
    Uuid::new_v4().to_string()
}

fn imported_password_tombstone() -> String {
    imported_credential_tombstone(&format!("password:{}", Uuid::new_v4()))
}

fn resolve_imported_password_hash(
    user: &Map<String, Value>,
    users_export_version: (u32, u32),
    mode: SystemImportMode,
) -> Result<Option<String>, String> {
    if users_export_version >= (1, 6) && user.contains_key("password_hash") {
        return Err("用户数据 1.6+ 不允许包含 password_hash 凭据字段".to_string());
    }
    let password_hash = imported_optional_string(user.get("password_hash"))?;
    if !mode.restores_credentials() {
        return Ok(password_hash.map(|_| imported_password_tombstone()));
    }
    if password_hash
        .as_deref()
        .is_some_and(|value| !aether_data::repository::users::is_valid_bcrypt_hash(value))
    {
        return Err("恢复备份中的 password_hash 不是有效的 bcrypt 哈希".to_string());
    }
    Ok(password_hash)
}

fn imported_optional_bool(value: Option<&Value>) -> Result<Option<bool>, String> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(value)) => Ok(Some(*value)),
        _ => Err("字段必须是布尔值".to_string()),
    }
}

fn imported_optional_i32(value: Option<&Value>, field_name: &str) -> Result<Option<i32>, String> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(number)) => number
            .as_i64()
            .ok_or_else(|| format!("{field_name} 必须是整数"))
            .and_then(|value| i32::try_from(value).map_err(|_| format!("{field_name} 超出范围")))
            .map(Some),
        _ => Err(format!("{field_name} 必须是整数")),
    }
}

fn imported_optional_u64(value: Option<&Value>, field_name: &str) -> Result<Option<u64>, String> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(number)) => number
            .as_u64()
            .ok_or_else(|| format!("{field_name} 必须是非负整数"))
            .map(Some),
        _ => Err(format!("{field_name} 必须是非负整数")),
    }
}

fn validate_imported_u64_storage(value: u64, field_name: &str) -> Result<(), String> {
    i64::try_from(value)
        .map(|_| ())
        .map_err(|_| format!("{field_name} 超出数据库整数范围"))
}

fn validate_imported_request_count(value: u64, field_name: &str) -> Result<(), String> {
    i32::try_from(value)
        .map(|_| ())
        .map_err(|_| format!("{field_name} 超出数据库请求计数范围"))
}

fn validate_imported_timestamp(value: u64, field_name: &str) -> Result<(), String> {
    validate_imported_u64_storage(value, field_name)?;
    chrono::DateTime::<chrono::Utc>::from_timestamp(value as i64, 0)
        .map(|_| ())
        .ok_or_else(|| format!("{field_name} 超出数据库时间范围"))
}

fn validate_imported_decimal_storage(value: f64, field_name: &str) -> Result<(), String> {
    if !value.is_finite() {
        return Err(format!("{field_name} 必须是有限数值"));
    }
    // PostgreSQL persists imported monetary values as NUMERIC(20,8).
    if value.abs() >= 1_000_000_000_000.0 {
        return Err(format!("{field_name} 超出数据库金额范围"));
    }
    Ok(())
}

fn imported_optional_f64(value: Option<&Value>, field_name: &str) -> Result<Option<f64>, String> {
    let parsed = match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(number)) => number
            .as_f64()
            .filter(|value| value.is_finite())
            .ok_or_else(|| format!("{field_name} 必须是有限数值"))
            .map(Some),
        Some(Value::String(value)) => value
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|value| value.is_finite())
            .ok_or_else(|| format!("{field_name} 必须是有限数值"))
            .map(Some),
        _ => Err(format!("{field_name} 必须是有限数值")),
    }?;
    if let Some(value) = parsed {
        validate_imported_decimal_storage(value, field_name)?;
    }
    Ok(parsed)
}

fn imported_optional_json_object(
    value: Option<&Value>,
    field_name: &str,
) -> Result<Option<Value>, String> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Object(map)) => Ok(Some(Value::Object(map.clone()))),
        _ => Err(format!("{field_name} 必须是对象")),
    }
}

fn imported_optional_value(value: Option<&Value>) -> Option<Value> {
    value.cloned().filter(|value| !value.is_null())
}

fn imported_optional_list_policy_mode(
    value: Option<&Value>,
    field_name: &str,
) -> Result<Option<String>, String> {
    let Some(value) = imported_optional_string(value)? else {
        return Ok(None);
    };
    let value = value.to_ascii_lowercase();
    normalize_admin_list_policy_mode(&value)
        .map(Some)
        .map_err(|_| format!("{field_name} 不合法"))
}

fn imported_optional_rate_limit_policy_mode(
    value: Option<&Value>,
    field_name: &str,
) -> Result<Option<String>, String> {
    let Some(value) = imported_optional_string(value)? else {
        return Ok(None);
    };
    let value = value.to_ascii_lowercase();
    normalize_admin_rate_limit_policy_mode(&value)
        .map(Some)
        .map_err(|_| format!("{field_name} 不合法"))
}

fn legacy_imported_list_policy_mode(values: &Option<Vec<String>>) -> String {
    if values.as_ref().is_some_and(|items| !items.is_empty()) {
        "specific".to_string()
    } else {
        "unrestricted".to_string()
    }
}

fn legacy_imported_rate_limit_policy_mode(value: Option<i32>) -> String {
    if value.is_some() {
        "custom".to_string()
    } else {
        "system".to_string()
    }
}

fn imported_user_list_policy_mode(
    object: &Map<String, Value>,
    mode_field: &str,
    value_field: &str,
    values: &Option<Vec<String>>,
) -> Result<Option<String>, String> {
    imported_optional_list_policy_mode(object.get(mode_field), mode_field).map(|mode| {
        mode.or_else(|| {
            object
                .contains_key(value_field)
                .then(|| legacy_imported_list_policy_mode(values))
        })
    })
}

fn imported_user_rate_limit_policy_mode(
    object: &Map<String, Value>,
    mode_field: &str,
    value_field: &str,
    value: Option<i32>,
) -> Result<Option<String>, String> {
    imported_optional_rate_limit_policy_mode(object.get(mode_field), mode_field).map(|mode| {
        mode.or_else(|| {
            object
                .contains_key(value_field)
                .then(|| legacy_imported_rate_limit_policy_mode(value))
        })
    })
}

fn build_imported_user_usage_total_aggregates(
    users: &[Value],
    exported_at: Option<&Value>,
) -> Result<Vec<AdminSystemStatsUserDailyAggregate>, String> {
    let date_unix_secs = imported_export_day_unix_secs(exported_at);
    let mut rows = Vec::new();
    for (index, raw_user) in users.iter().enumerate() {
        let user = imported_object_field(raw_user, &format!("users[{index}]"))?;
        let Some(user_id) = imported_optional_string(user.get("id"))? else {
            continue;
        };
        let request_count = imported_optional_u64(user.get("request_count"), "request_count")?;
        let total_tokens = imported_optional_u64(user.get("total_tokens"), "total_tokens")?;
        if request_count.is_none() && total_tokens.is_none() {
            continue;
        }
        let total_requests = request_count.unwrap_or(0);
        let input_tokens = total_tokens.unwrap_or(0);
        if total_requests == 0 && input_tokens == 0 {
            continue;
        }
        rows.push(AdminSystemStatsUserDailyAggregate {
            user_id,
            username: imported_optional_string(user.get("username"))?,
            date_unix_secs,
            total_requests,
            success_requests: total_requests,
            error_requests: 0,
            input_tokens,
            output_tokens: 0,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
            total_cost: 0.0,
        });
    }
    Ok(rows)
}

fn build_imported_usage_aggregate_snapshot(
    value: Option<&Value>,
    supplemental_user_daily: &[AdminSystemStatsUserDailyAggregate],
) -> Result<AdminSystemUsageAggregateSnapshot, String> {
    let mut snapshot = match value {
        Some(value) if !value.is_null() => {
            serde_json::from_value::<AdminSystemUsageAggregateSnapshot>(value.clone())
                .map_err(|err| format!("usage_aggregates 格式无效: {err}"))?
        }
        _ => AdminSystemUsageAggregateSnapshot::default(),
    };
    let mut existing_user_totals = BTreeMap::<String, (u64, u64)>::new();
    for row in &snapshot.stats_user_daily {
        let total_tokens = row
            .input_tokens
            .saturating_add(row.output_tokens)
            .saturating_add(row.cache_creation_tokens)
            .saturating_add(row.cache_read_tokens);
        let entry = existing_user_totals
            .entry(row.user_id.clone())
            .or_insert((0, 0));
        entry.0 = entry.0.saturating_add(row.total_requests);
        entry.1 = entry.1.saturating_add(total_tokens);
    }
    for row in supplemental_user_daily {
        let existing = existing_user_totals
            .get(&row.user_id)
            .copied()
            .unwrap_or_default();
        let request_delta = row.total_requests.saturating_sub(existing.0);
        let token_delta = row.input_tokens.saturating_sub(existing.1);
        if request_delta == 0 && token_delta == 0 {
            continue;
        }
        if let Some(existing_row) = snapshot
            .stats_user_daily
            .iter_mut()
            .rev()
            .find(|existing_row| existing_row.user_id == row.user_id)
        {
            existing_row.total_requests = existing_row.total_requests.saturating_add(request_delta);
            existing_row.success_requests =
                existing_row.success_requests.saturating_add(request_delta);
            existing_row.input_tokens = existing_row.input_tokens.saturating_add(token_delta);
        } else {
            let mut row = row.clone();
            row.total_requests = request_delta;
            row.success_requests = request_delta;
            row.input_tokens = token_delta;
            snapshot.stats_user_daily.push(row);
        }
    }
    Ok(snapshot)
}

fn validate_imported_usage_aggregate_storage(
    snapshot: &AdminSystemUsageAggregateSnapshot,
) -> Result<(), String> {
    macro_rules! validate_fields {
        ($row:expr, $prefix:expr, [$($field:ident),+ $(,)?]) => {
            $(validate_imported_u64_storage(
                $row.$field,
                &format!("{}.{}", $prefix, stringify!($field)),
            )?;)+
        };
    }

    for (index, row) in snapshot.stats_daily.iter().enumerate() {
        let prefix = format!("usage_aggregates.stats_daily[{index}]");
        validate_imported_timestamp(row.date_unix_secs, &format!("{prefix}.date_unix_secs"))?;
        validate_imported_request_count(row.total_requests, &format!("{prefix}.total_requests"))?;
        validate_imported_request_count(
            row.success_requests,
            &format!("{prefix}.success_requests"),
        )?;
        validate_imported_request_count(row.error_requests, &format!("{prefix}.error_requests"))?;
        validate_fields!(
            row,
            prefix,
            [
                input_tokens,
                output_tokens,
                cache_creation_tokens,
                cache_read_tokens,
            ]
        );
        if let Some(value) = row.aggregated_at_unix_secs {
            validate_imported_timestamp(value, &format!("{prefix}.aggregated_at_unix_secs"))?;
        }
        validate_imported_decimal_storage(row.total_cost, &format!("{prefix}.total_cost"))?;
        validate_imported_decimal_storage(
            row.actual_total_cost,
            &format!("{prefix}.actual_total_cost"),
        )?;
    }
    for (index, row) in snapshot.stats_user_daily.iter().enumerate() {
        let prefix = format!("usage_aggregates.stats_user_daily[{index}]");
        validate_imported_timestamp(row.date_unix_secs, &format!("{prefix}.date_unix_secs"))?;
        validate_imported_request_count(row.total_requests, &format!("{prefix}.total_requests"))?;
        validate_imported_request_count(
            row.success_requests,
            &format!("{prefix}.success_requests"),
        )?;
        validate_imported_request_count(row.error_requests, &format!("{prefix}.error_requests"))?;
        validate_fields!(
            row,
            prefix,
            [
                input_tokens,
                output_tokens,
                cache_creation_tokens,
                cache_read_tokens,
            ]
        );
        validate_imported_decimal_storage(row.total_cost, &format!("{prefix}.total_cost"))?;
    }
    for (index, row) in snapshot.stats_daily_api_key.iter().enumerate() {
        let prefix = format!("usage_aggregates.stats_daily_api_key[{index}]");
        validate_imported_timestamp(row.date_unix_secs, &format!("{prefix}.date_unix_secs"))?;
        validate_imported_request_count(row.total_requests, &format!("{prefix}.total_requests"))?;
        validate_imported_request_count(
            row.success_requests,
            &format!("{prefix}.success_requests"),
        )?;
        validate_imported_request_count(row.error_requests, &format!("{prefix}.error_requests"))?;
        validate_fields!(
            row,
            prefix,
            [
                input_tokens,
                output_tokens,
                cache_creation_tokens,
                cache_read_tokens,
            ]
        );
        validate_imported_decimal_storage(row.total_cost, &format!("{prefix}.total_cost"))?;
    }
    Ok(())
}

fn validate_imported_usage_aggregate_dimensions(
    snapshot: &AdminSystemUsageAggregateSnapshot,
    user_id_map: &BTreeMap<String, String>,
    api_key_id_map: &BTreeMap<String, String>,
) -> Result<(), String> {
    let mut seen_daily = BTreeSet::new();
    for row in &snapshot.stats_daily {
        if !seen_daily.insert(row.date_unix_secs) {
            return Err(format!(
                "stats_daily aggregate already exists for date_unix_secs={}",
                row.date_unix_secs
            ));
        }
    }
    let mut seen_user_daily = BTreeSet::new();
    for row in &snapshot.stats_user_daily {
        let Some(target_user_id) = user_id_map.get(&row.user_id) else {
            continue;
        };
        if !seen_user_daily.insert((target_user_id.clone(), row.date_unix_secs)) {
            return Err(format!(
                "stats_user_daily aggregate already exists for date_unix_secs={}",
                row.date_unix_secs
            ));
        }
    }
    let mut seen_api_key_daily = BTreeSet::new();
    for row in &snapshot.stats_daily_api_key {
        let Some(target_api_key_id) = api_key_id_map.get(&row.api_key_id) else {
            continue;
        };
        if !seen_api_key_daily.insert((target_api_key_id.clone(), row.date_unix_secs)) {
            return Err(format!(
                "stats_daily_api_key aggregate already exists for date_unix_secs={}",
                row.date_unix_secs
            ));
        }
    }
    Ok(())
}

fn insert_imported_id_mapping(
    mappings: &mut BTreeMap<String, String>,
    source_id: String,
    target_id: String,
    field_name: &str,
) -> Result<(), String> {
    if mappings.contains_key(&source_id) {
        return Err(format!("{field_name} 在导入文档中重复: {source_id}"));
    }
    mappings.insert(source_id, target_id);
    Ok(())
}

fn imported_export_day_unix_secs(exported_at: Option<&Value>) -> u64 {
    imported_optional_string(exported_at)
        .ok()
        .flatten()
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(&value).ok())
        .map(|value| unix_day_start_secs(value.timestamp()))
        .unwrap_or_else(|| unix_day_start_secs(chrono::Utc::now().timestamp()))
}

fn unix_day_start_secs(timestamp: i64) -> u64 {
    let timestamp = timestamp.max(0) as u64;
    timestamp - (timestamp % 86_400)
}

fn imported_rfc3339_to_unix_secs(
    value: Option<&Value>,
    field_name: &str,
) -> Result<Option<u64>, String> {
    let Some(value) = imported_optional_string(value)? else {
        return Ok(None);
    };
    let parsed_timestamp = chrono::DateTime::parse_from_rfc3339(&value)
        .map(|parsed| parsed.timestamp())
        .or_else(|_| {
            chrono::NaiveDateTime::parse_from_str(&value, "%Y-%m-%dT%H:%M:%S%.f")
                .map(|parsed| parsed.and_utc().timestamp())
        })
        .map_err(|_| format!("{field_name} 必须是 RFC3339 时间"))?;
    Ok(Some(parsed_timestamp.max(0) as u64))
}

fn imported_string_list_from_value(
    value: Option<&Value>,
    field_name: &str,
) -> Result<Option<Vec<String>>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    match value {
        Value::Null => Ok(None),
        Value::Array(items) => Ok(Some(
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .collect(),
        )),
        _ => Err(format!("{field_name} 必须是字符串列表")),
    }
}

fn normalize_imported_user_string_list(
    object: &Map<String, Value>,
    field_name: &str,
) -> Result<Option<Vec<String>>, String> {
    normalize_admin_user_string_list(
        imported_string_list_from_value(object.get(field_name), field_name)?,
        field_name,
    )
}

fn normalize_imported_user_api_formats(
    object: &Map<String, Value>,
    field_name: &str,
) -> Result<Option<Vec<String>>, String> {
    normalize_admin_user_api_formats(imported_string_list_from_value(
        object.get(field_name),
        field_name,
    )?)
}

fn normalize_imported_provider_key_policies(
    object: &Map<String, Value>,
) -> Result<BTreeMap<String, Vec<String>>, String> {
    let Some(value) = object.get("provider_key_policies") else {
        return Ok(BTreeMap::new());
    };
    let Value::Object(policies) = value else {
        return Err("provider_key_policies 必须是对象".to_string());
    };
    let mut normalized = BTreeMap::new();
    for (provider_id, key_ids) in policies {
        let provider_id = provider_id.trim();
        if provider_id.is_empty() {
            continue;
        }
        let Some(key_ids) = imported_string_list_from_value(
            Some(key_ids),
            &format!("provider_key_policies.{provider_id}"),
        )?
        else {
            continue;
        };
        normalized.insert(
            provider_id.to_string(),
            key_ids
                .into_iter()
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
        );
    }
    Ok(normalized)
}

fn imported_portable_reference<'a>(
    value: &'a Value,
    field_name: &str,
) -> Result<(Option<&'a str>, Option<&'a str>), String> {
    let object = value
        .as_object()
        .ok_or_else(|| format!("{field_name} 必须是对象"))?;
    let id = object
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if id.is_none() && name.is_none() {
        return Err(format!("{field_name} 至少需要 id 或 name"));
    }
    Ok((id, name))
}

fn resolve_imported_provider_reference(
    value: &Value,
    field_name: &str,
    providers: &[aether_data_contracts::repository::provider_catalog::StoredProviderCatalogProvider],
) -> Result<String, String> {
    let (id, name) = imported_portable_reference(value, field_name)?;
    if let Some(id) = id {
        let id_matches = providers
            .iter()
            .filter(|provider| provider.id.eq_ignore_ascii_case(id))
            .collect::<Vec<_>>();
        match id_matches.as_slice() {
            [provider] => return Ok(provider.id.clone()),
            [] => {}
            _ => return Err(format!("{field_name}.id 不唯一: {id}")),
        }
    }

    let Some(name) = name else {
        return Err(format!(
            "{field_name} 无法按 UUID 解析，且没有可回退的 name"
        ));
    };
    let name_matches = providers
        .iter()
        .filter(|provider| provider.name.eq_ignore_ascii_case(name))
        .collect::<Vec<_>>();
    match name_matches.as_slice() {
        [provider] => Ok(provider.id.clone()),
        [] => Err(format!("{field_name} 引用的 Provider 不存在: {name}")),
        _ => Err(format!(
            "{field_name} 的 Provider 名称不唯一，请修正目标实例中的重名项: {name}"
        )),
    }
}

fn resolve_imported_key_reference(
    value: &Value,
    field_name: &str,
    provider_id: &str,
    keys: &[aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey],
) -> Result<String, String> {
    let (id, name) = imported_portable_reference(value, field_name)?;
    if let Some(id) = id {
        let id_matches = keys
            .iter()
            .filter(|key| key.id.eq_ignore_ascii_case(id))
            .collect::<Vec<_>>();
        match id_matches.as_slice() {
            [key] if key.provider_id == provider_id => return Ok(key.id.clone()),
            [key] => {
                return Err(format!("{field_name}.id 属于其他 Provider: {}", key.id));
            }
            [] => {}
            _ => return Err(format!("{field_name}.id 不唯一: {id}")),
        }
    }

    let Some(name) = name else {
        return Err(format!(
            "{field_name} 无法按 UUID 解析，且没有可回退的 name"
        ));
    };
    let name_matches = keys
        .iter()
        .filter(|key| key.provider_id == provider_id && key.name.eq_ignore_ascii_case(name))
        .collect::<Vec<_>>();
    match name_matches.as_slice() {
        [key] => Ok(key.id.clone()),
        [] => Err(format!("{field_name} 引用的 Provider Key 不存在: {name}")),
        _ => Err(format!(
            "{field_name} 的 Provider Key 名称不唯一，请修正目标实例中的重名项: {name}"
        )),
    }
}

fn apply_imported_portable_provider_access(
    group: &Map<String, Value>,
    field_name: &str,
    mut record: aether_data::repository::users::UpsertUserGroupRecord,
    providers: &[aether_data_contracts::repository::provider_catalog::StoredProviderCatalogProvider],
    keys: &[aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey],
) -> Result<aether_data::repository::users::UpsertUserGroupRecord, String> {
    if let Some(value) = group.get("allowed_provider_refs") {
        let references = value
            .as_array()
            .ok_or_else(|| format!("{field_name}.allowed_provider_refs 必须是数组"))?;
        let mut provider_ids = BTreeSet::new();
        for (index, reference) in references.iter().enumerate() {
            provider_ids.insert(resolve_imported_provider_reference(
                reference,
                &format!("{field_name}.allowed_provider_refs[{index}]"),
                providers,
            )?);
        }
        record.allowed_providers = Some(provider_ids.into_iter().collect());
    }

    if let Some(value) = group.get("provider_key_policy_refs") {
        let policies = value
            .as_array()
            .ok_or_else(|| format!("{field_name}.provider_key_policy_refs 必须是数组"))?;
        let mut normalized = BTreeMap::new();
        for (policy_index, policy) in policies.iter().enumerate() {
            let policy_field = format!("{field_name}.provider_key_policy_refs[{policy_index}]");
            let policy = policy
                .as_object()
                .ok_or_else(|| format!("{policy_field} 必须是对象"))?;
            let provider = policy
                .get("provider")
                .ok_or_else(|| format!("{policy_field}.provider 为必填字段"))?;
            let provider_id = resolve_imported_provider_reference(
                provider,
                &format!("{policy_field}.provider"),
                providers,
            )?;
            if normalized.contains_key(&provider_id) {
                return Err(format!("{policy_field} 重复引用 Provider {provider_id}"));
            }
            let key_references = policy
                .get("keys")
                .and_then(Value::as_array)
                .ok_or_else(|| format!("{policy_field}.keys 必须是数组"))?;
            let mut key_ids = BTreeSet::new();
            for (key_index, key_reference) in key_references.iter().enumerate() {
                key_ids.insert(resolve_imported_key_reference(
                    key_reference,
                    &format!("{policy_field}.keys[{key_index}]"),
                    &provider_id,
                    keys,
                )?);
            }
            normalized.insert(provider_id, key_ids.into_iter().collect());
        }
        record.provider_key_policies = normalized;
    }

    Ok(record)
}

fn imported_ip_rules_field<'a>(
    object: &'a Map<String, Value>,
) -> (&'static str, Option<&'a Value>) {
    if let Some(value) = object.get("ip_rules") {
        ("ip_rules", Some(value))
    } else {
        ("allowed_ips", object.get("allowed_ips"))
    }
}

fn imported_ip_rules_present(object: &Map<String, Value>) -> bool {
    object.contains_key("ip_rules") || object.contains_key("allowed_ips")
}

fn normalize_imported_user_ip_rules(
    object: &Map<String, Value>,
) -> Result<Option<Vec<String>>, String> {
    let (field_name, value) = imported_ip_rules_field(object);
    normalize_admin_user_ip_rules(imported_string_list_from_value(value, field_name)?)
}

fn build_imported_user_group_record(
    group: &Map<String, Value>,
    field_name: &str,
) -> Result<
    (
        Option<String>,
        String,
        aether_data::repository::users::UpsertUserGroupRecord,
    ),
    String,
> {
    let export_id = imported_optional_string(group.get("id"))?;
    let name = imported_optional_string(group.get("name"))?
        .ok_or_else(|| format!("{field_name}.name 不能为空"))?;
    let name = aether_data::repository::users::normalize_user_group_name(&name);
    if name.is_empty() {
        return Err(format!("{field_name}.name 不能为空"));
    }
    let description = imported_optional_string(group.get("description"))?;
    let allowed_providers = normalize_imported_user_string_list(group, "allowed_providers")?;
    let provider_key_policies = normalize_imported_provider_key_policies(group)?;
    let allowed_api_formats = normalize_imported_user_api_formats(group, "allowed_api_formats")?;
    let allowed_models = normalize_imported_user_string_list(group, "allowed_models")?;
    let rate_limit = imported_optional_i32(group.get("rate_limit"), "rate_limit")?;

    let allowed_providers_mode = imported_optional_list_policy_mode(
        group.get("allowed_providers_mode"),
        "allowed_providers_mode",
    )?
    .unwrap_or_else(|| {
        if group.contains_key("allowed_providers") {
            legacy_imported_list_policy_mode(&allowed_providers)
        } else {
            "inherit".to_string()
        }
    });
    let allowed_api_formats_mode = imported_optional_list_policy_mode(
        group.get("allowed_api_formats_mode"),
        "allowed_api_formats_mode",
    )?
    .unwrap_or_else(|| {
        if group.contains_key("allowed_api_formats") {
            legacy_imported_list_policy_mode(&allowed_api_formats)
        } else {
            "inherit".to_string()
        }
    });
    let allowed_models_mode = imported_optional_list_policy_mode(
        group.get("allowed_models_mode"),
        "allowed_models_mode",
    )?
    .unwrap_or_else(|| {
        if group.contains_key("allowed_models") {
            legacy_imported_list_policy_mode(&allowed_models)
        } else {
            "inherit".to_string()
        }
    });
    let rate_limit_mode =
        imported_optional_rate_limit_policy_mode(group.get("rate_limit_mode"), "rate_limit_mode")?
            .unwrap_or_else(|| {
                if group.contains_key("rate_limit") {
                    legacy_imported_rate_limit_policy_mode(rate_limit)
                } else {
                    "inherit".to_string()
                }
            });

    let normalized_name = name.to_ascii_lowercase();

    Ok((
        export_id,
        normalized_name,
        aether_data::repository::users::UpsertUserGroupRecord {
            name,
            description,
            priority: 0,
            allowed_providers,
            allowed_providers_mode,
            provider_key_policies,
            allowed_api_formats,
            allowed_api_formats_mode,
            allowed_models,
            allowed_models_mode,
            rate_limit,
            rate_limit_mode,
        },
    ))
}

fn resolve_imported_user_group_ids(
    user: &Map<String, Value>,
    imported_group_id_map: &BTreeMap<String, String>,
    imported_group_name_map: &BTreeMap<String, String>,
    groups_by_name: &BTreeMap<String, aether_data::repository::users::StoredUserGroup>,
) -> Result<Vec<String>, String> {
    let raw_group_ids =
        imported_string_list_from_value(user.get("group_ids"), "group_ids")?.unwrap_or_default();
    let raw_group_names = imported_string_list_from_value(user.get("group_names"), "group_names")?
        .unwrap_or_default();
    let mut group_ids = BTreeSet::new();
    for raw_group_id in raw_group_ids {
        if let Some(group_id) = imported_group_id_map.get(&raw_group_id) {
            group_ids.insert(group_id.clone());
            continue;
        }
        group_ids.insert(raw_group_id);
    }
    for raw_group_name in raw_group_names {
        let normalized_name =
            aether_data::repository::users::normalize_user_group_name(&raw_group_name)
                .to_ascii_lowercase();
        if normalized_name.is_empty() {
            continue;
        }
        if let Some(group_id) = imported_group_name_map.get(&normalized_name) {
            group_ids.insert(group_id.clone());
            continue;
        }
        if let Some(group) = groups_by_name.get(&normalized_name) {
            group_ids.insert(group.id.clone());
        }
    }
    Ok(group_ids.into_iter().collect())
}

fn normalize_imported_wallet_target(
    wallet: Option<&Map<String, Value>>,
    unlimited: bool,
) -> Result<ImportedWalletTarget, String> {
    let gift_balance = imported_optional_f64(
        wallet.and_then(|map| map.get("gift_balance")),
        "wallet.gift_balance",
    )?
    .unwrap_or(0.0)
    .max(0.0);
    let recharge_balance = if let Some(map) = wallet {
        if map.contains_key("recharge_balance") {
            imported_optional_f64(map.get("recharge_balance"), "wallet.recharge_balance")?
                .unwrap_or(0.0)
        } else if map.contains_key("refundable_balance") {
            imported_optional_f64(map.get("refundable_balance"), "wallet.refundable_balance")?
                .unwrap_or(0.0)
        } else {
            let total_balance =
                imported_optional_f64(map.get("balance"), "wallet.balance")?.unwrap_or(0.0);
            total_balance - gift_balance
        }
    } else {
        0.0
    };
    let limit_mode = if let Some(map) = wallet {
        if let Some(mode) = imported_optional_string(map.get("limit_mode"))? {
            match mode.to_ascii_lowercase().as_str() {
                "finite" => "finite".to_string(),
                "unlimited" => "unlimited".to_string(),
                _ => return Err("wallet.limit_mode 仅支持 finite / unlimited".to_string()),
            }
        } else if imported_optional_bool(map.get("unlimited"))?.unwrap_or(unlimited) {
            "unlimited".to_string()
        } else {
            "finite".to_string()
        }
    } else if unlimited {
        "unlimited".to_string()
    } else {
        "finite".to_string()
    };
    let currency = imported_optional_string(wallet.and_then(|map| map.get("currency")))?
        .unwrap_or_else(|| "USD".to_string());
    let status = imported_optional_string(wallet.and_then(|map| map.get("status")))?
        .unwrap_or_else(|| "active".to_string());
    if currency.chars().count() > 3 {
        return Err("wallet.currency 最多允许 3 个字符".to_string());
    }
    if status.chars().count() > 20 {
        return Err("wallet.status 最多允许 20 个字符".to_string());
    }
    let imported_total_recharged = imported_optional_f64(
        wallet.and_then(|map| map.get("total_recharged")),
        "wallet.total_recharged",
    )?;
    let total_recharged = imported_total_recharged.unwrap_or_else(|| recharge_balance.max(0.0));
    if total_recharged < 0.0 {
        return Err("wallet.total_recharged 必须是非负有限数值".to_string());
    }
    let imported_total_consumed = imported_optional_f64(
        wallet.and_then(|map| map.get("total_consumed")),
        "wallet.total_consumed",
    )?;
    let total_consumed = imported_total_consumed.unwrap_or(0.0);
    if total_consumed < 0.0 {
        return Err("wallet.total_consumed 必须是非负有限数值".to_string());
    }
    let imported_total_refunded = imported_optional_f64(
        wallet.and_then(|map| map.get("total_refunded")),
        "wallet.total_refunded",
    )?;
    let total_refunded = imported_total_refunded.unwrap_or(0.0);
    if total_refunded < 0.0 {
        return Err("wallet.total_refunded 必须是非负有限数值".to_string());
    }
    let total_adjusted = imported_optional_f64(
        wallet.and_then(|map| map.get("total_adjusted")),
        "wallet.total_adjusted",
    )?
    .unwrap_or(gift_balance);
    let updated_at_unix_secs = imported_rfc3339_to_unix_secs(
        wallet.and_then(|map| map.get("updated_at")),
        "wallet.updated_at",
    )?;

    for (field_name, value) in [
        ("wallet.recharge_balance", recharge_balance),
        ("wallet.gift_balance", gift_balance),
        ("wallet.total_recharged", total_recharged),
        ("wallet.total_consumed", total_consumed),
        ("wallet.total_refunded", total_refunded),
        ("wallet.total_adjusted", total_adjusted),
    ] {
        validate_imported_decimal_storage(value, field_name)?;
    }

    Ok(ImportedWalletTarget {
        recharge_balance,
        gift_balance,
        limit_mode,
        currency,
        status,
        total_recharged,
        total_consumed,
        total_refunded,
        total_adjusted,
        updated_at_unix_secs,
    })
}

impl<'a> AdminAppState<'a> {
    async fn prevalidate_admin_system_config_import(
        &self,
        request_body: &[u8],
        mode: SystemImportMode,
    ) -> Result<Result<(), (http::StatusCode, Value)>, GatewayError> {
        macro_rules! invalid {
            ($expr:expr) => {
                match $expr {
                    Ok(value) => value,
                    Err(detail) => return Ok(Err(invalid_request(detail))),
                }
            };
        }
        macro_rules! routed {
            ($expr:expr) => {
                match $expr {
                    Ok(value) => value,
                    Err(err) => return Ok(Err(err)),
                }
            };
        }

        let parsed = routed!(parse_admin_system_config_import_request(request_body));
        let source_version = parsed.request.document.version.clone();
        let root = parsed.root;
        let credentials_not_exported = invalid!(imported_config_credentials_not_exported(&root));
        let merge_mode = parsed.request.merge_mode;
        let imported_global_models = routed!(
            parse_admin_system_config_array::<ImportedGlobalModel>(&root, "global_models")
        );
        let imported_providers = routed!(parse_admin_system_config_array::<ImportedProvider>(
            &root,
            "providers",
        ));
        let imported_proxy_nodes = routed!(parse_admin_system_config_array::<ImportedProxyNode>(
            &root,
            "proxy_nodes",
        ));
        if mode.restores_credentials() && !imported_proxy_nodes.is_empty() {
            return Ok(Err(invalid_request(
                "恢复备份包含 proxy_nodes，但当前恢复入口不支持安全恢复代理节点",
            )));
        }
        let imported_ldap = routed!(parse_admin_system_config_optional_object::<
            ImportedLdapConfig,
        >(&root, "ldap_config"));
        let imported_oauth_providers = routed!(parse_admin_system_config_array::<
            ImportedOAuthProvider,
        >(&root, "oauth_providers"));
        let imported_system_configs = routed!(parse_admin_system_config_array::<
            ImportedSystemConfig,
        >(&root, "system_configs"));
        let (imported_external_models_configs, mut imported_system_configs): (Vec<_>, Vec<_>) =
            imported_system_configs.into_iter().partition(|item| {
                normalize_imported_system_config_key(&item.value.key)
                    == ADMIN_EXTERNAL_MODELS_PROXY_NODE_CONFIG_KEY
            });
        // Destination-bound secrets must be applied after their destination fields.  Recovery
        // documents are user-controlled JSON and do not guarantee any ordering.
        imported_system_configs.sort_by_key(|item| {
            matches!(
                normalize_imported_system_config_key(&item.value.key).as_str(),
                "smtp_password" | "module.bark_push.device_key"
            )
        });
        let mut existing_system_config_keys = self
            .list_system_config_entries()
            .await?
            .into_iter()
            .map(|entry| normalize_imported_system_config_key(&entry.key))
            .collect::<BTreeSet<_>>();
        for imported_config_item in imported_external_models_configs {
            let config = imported_config_item.value;
            let exists =
                existing_system_config_keys.contains(ADMIN_EXTERNAL_MODELS_PROXY_NODE_CONFIG_KEY);
            match (exists, merge_mode) {
                (true, AdminImportMergeMode::Skip) => continue,
                (true, AdminImportMergeMode::Error) => {
                    return Ok(Err(invalid_request(format!(
                        "SystemConfig '{ADMIN_EXTERNAL_MODELS_PROXY_NODE_CONFIG_KEY}' 已存在"
                    ))));
                }
                _ => {}
            }
            match config.value {
                Value::Null => {}
                Value::String(value) if !value.trim().is_empty() => {}
                Value::String(_) => {
                    return Ok(Err(invalid_request(
                        "external_models_proxy_node_id 不能为空",
                    )));
                }
                _ => {
                    return Ok(Err(invalid_request(
                        "external_models_proxy_node_id 必须是字符串或 null",
                    )));
                }
            }
            existing_system_config_keys
                .insert(ADMIN_EXTERNAL_MODELS_PROXY_NODE_CONFIG_KEY.to_string());
        }

        let mut global_models_by_name = self
            .list_all_admin_global_models_for_system_transfer()
            .await?
            .into_iter()
            .map(|model| (model.name.clone(), (model.id.clone(), Some(model))))
            .collect::<BTreeMap<_, _>>();
        for imported_model_item in &imported_global_models {
            let model = &imported_model_item.value;
            let name = invalid!(trim_required(&model.name, "name"));
            let display_name = invalid!(trim_required(&model.display_name, "display_name"));
            let default_price_per_request = invalid!(normalize_optional_price(
                model.default_price_per_request,
                "default_price_per_request",
            ));
            let existing_model = global_models_by_name
                .get(&name)
                .and_then(|(_, model)| model.as_ref());
            let default_tiered_pricing = invalid!(normalize_json_object(
                prepare_imported_secret_safe_json(
                    existing_model.and_then(|model| model.default_tiered_pricing.as_ref()),
                    model.default_tiered_pricing.clone(),
                    credentials_not_exported,
                ),
                "default_tiered_pricing",
            ));
            let supported_capabilities =
                normalize_supported_capabilities(model.supported_capabilities.clone());
            let config = invalid!(normalize_json_object(
                prepare_imported_secret_safe_json(
                    existing_model.and_then(|model| model.config.as_ref()),
                    model.config.clone(),
                    credentials_not_exported,
                ),
                "config",
            ));
            if let Some((existing_id, _)) = global_models_by_name.get(&name).cloned() {
                match merge_mode {
                    AdminImportMergeMode::Skip => continue,
                    AdminImportMergeMode::Error => {
                        return Ok(Err(invalid_request(format!("GlobalModel '{name}' 已存在"))));
                    }
                    AdminImportMergeMode::Overwrite => {
                        invalid!(UpdateAdminGlobalModelRecord::new(
                            existing_id,
                            display_name,
                            model.is_active,
                            default_price_per_request,
                            default_tiered_pricing,
                            supported_capabilities,
                            config,
                        )
                        .map_err(|err| err.to_string()));
                    }
                }
            } else {
                let id = Uuid::new_v4().to_string();
                invalid!(CreateAdminGlobalModelRecord::new(
                    id.clone(),
                    name.clone(),
                    display_name,
                    model.is_active,
                    default_price_per_request,
                    default_tiered_pricing,
                    supported_capabilities,
                    config,
                )
                .map_err(|err| err.to_string()));
                global_models_by_name.insert(name, (id, None));
            }
        }

        let mut providers_by_name = self
            .list_provider_catalog_providers(false)
            .await?
            .into_iter()
            .map(|provider| (provider.name.clone(), provider))
            .collect::<BTreeMap<_, _>>();
        let mut endpoints_by_provider = BTreeMap::<
            String,
            BTreeMap<
                String,
                aether_data_contracts::repository::provider_catalog::StoredProviderCatalogEndpoint,
            >,
        >::new();
        let mut keys_by_provider = BTreeMap::<
            String,
            Vec<aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey>,
        >::new();
        let mut models_by_provider = BTreeMap::<
            String,
            BTreeMap<
                String,
                (
                    String,
                    Option<
                        aether_data_contracts::repository::global_models::StoredAdminProviderModel,
                    >,
                ),
            >,
        >::new();
        let node_id_map = BTreeMap::<String, String>::new();

        for imported_provider_item in &imported_providers {
            let raw_provider = &imported_provider_item.raw;
            let imported_provider = &imported_provider_item.value;
            let provider_name = invalid!(trim_required(&imported_provider.name, "name"));
            invalid!(
                crate::provider_transport::validate_anthropic_compatibility_profile_config(
                    imported_provider.config.as_ref(),
                )
                .map_err(|_| "无效的 Anthropic compatibility profile".to_string())
            );

            let existing_provider = providers_by_name.get(&provider_name).cloned();
            if existing_provider.is_some() && merge_mode == AdminImportMergeMode::Error {
                return Ok(Err(invalid_request(format!(
                    "Provider '{provider_name}' 已存在"
                ))));
            }
            let provider = if let Some(existing) = existing_provider {
                if merge_mode == AdminImportMergeMode::Skip {
                    existing
                } else {
                    let patch = AdminProviderUpdatePatch::from_object(raw_provider.clone())
                        .map_err(|_| "Provider 配置格式无效".to_string());
                    let patch = invalid!(patch);
                    let mut updated = invalid!(
                        self.build_admin_update_provider_record(&existing, patch)
                            .await
                    );
                    updated.proxy = prepare_imported_secret_safe_proxy(
                        existing.proxy.as_ref(),
                        imported_provider.proxy.clone(),
                        credentials_not_exported,
                        &node_id_map,
                    );
                    let provider_ops_fallback_base_url =
                        imported_provider_ops_fallback_base_url(raw_provider);
                    updated.config = invalid!(prepare_imported_provider_config(
                        self,
                        &updated.id,
                        provider_ops_fallback_base_url.as_deref(),
                        existing.config.as_ref(),
                        imported_provider.config.clone(),
                        credentials_not_exported,
                    ));
                    updated
                }
            } else {
                let payload = serde_json::from_value::<AdminProviderCreateRequest>(Value::Object(
                    raw_provider.clone(),
                ))
                .map_err(|_| format!("Provider '{provider_name}' 配置格式无效"));
                let payload = invalid!(payload);
                let (mut record, _) =
                    invalid!(self.build_admin_create_provider_record(payload).await);
                record.name = provider_name.clone();
                if let Some(enable_format_conversion) = imported_provider.enable_format_conversion {
                    record.enable_format_conversion = enable_format_conversion;
                }
                record.proxy = prepare_imported_secret_safe_proxy(
                    None,
                    imported_provider.proxy.clone(),
                    credentials_not_exported,
                    &node_id_map,
                );
                let provider_ops_fallback_base_url =
                    imported_provider_ops_fallback_base_url(raw_provider);
                record.config = invalid!(prepare_imported_provider_config(
                    self,
                    &record.id,
                    provider_ops_fallback_base_url.as_deref(),
                    None,
                    imported_provider.config.clone(),
                    credentials_not_exported,
                ));
                record
            };

            let mut existing_endpoints_by_format =
                match endpoints_by_provider.remove(&provider_name) {
                    Some(endpoints) => endpoints,
                    None => self
                        .list_provider_catalog_endpoints_by_provider_ids(std::slice::from_ref(
                            &provider.id,
                        ))
                        .await?
                        .into_iter()
                        .map(|endpoint| (endpoint.api_format.clone(), endpoint))
                        .collect(),
                };
            let imported_endpoints = routed!(parse_admin_system_config_nested_array::<
                ImportedEndpoint,
            >(raw_provider, "endpoints"));
            for imported_endpoint_item in imported_endpoints {
                let (raw_endpoint, imported_endpoint) = imported_endpoint_item.into_parts();
                let normalized_api_format = invalid!(normalize_import_endpoint_format(
                    &imported_endpoint.api_format,
                ));
                invalid!(
                    crate::provider_transport::validate_anthropic_compatibility_profile_config(
                        imported_endpoint.config.as_ref(),
                    )
                    .map_err(|_| "无效的 Anthropic compatibility profile".to_string())
                );
                if !fixed_provider_import_endpoint_supported(
                    &provider.provider_type,
                    &normalized_api_format,
                ) {
                    existing_endpoints_by_format.remove(&normalized_api_format);
                    continue;
                }
                let existing_endpoint = existing_endpoints_by_format
                    .get(&normalized_api_format)
                    .cloned();
                if existing_endpoint.is_some() {
                    if merge_mode == AdminImportMergeMode::Error {
                        return Ok(Err(invalid_request(format!(
                            "Endpoint '{normalized_api_format}' 已存在于 Provider '{provider_name}'"
                        ))));
                    }
                    if merge_mode == AdminImportMergeMode::Skip {
                        continue;
                    }
                }
                let Some((signature, api_family, endpoint_kind)) =
                    admin_endpoint_signature_parts(&normalized_api_format)
                else {
                    return Ok(Err(invalid_request(format!(
                        "无效的 api_format: {}",
                        imported_endpoint.api_format
                    ))));
                };
                let endpoint = if let Some(existing) = existing_endpoint.as_ref() {
                    let patch = AdminProviderEndpointUpdatePatch::from_object(raw_endpoint)
                        .map_err(|_| "Provider Endpoint 配置格式无效".to_string());
                    let patch = invalid!(patch);
                    let (fields, payload) = patch.into_parts();
                    let normalized_base_url = payload
                        .base_url
                        .as_deref()
                        .map(normalize_admin_base_url)
                        .transpose();
                    let normalized_base_url = invalid!(normalized_base_url);
                    let update_fields =
                        admin_provider_endpoints_pure::AdminProviderEndpointUpdateFields {
                            base_url: normalized_base_url,
                            custom_path: payload.custom_path,
                            header_rules: prepare_imported_secret_safe_header_rules(
                                existing.header_rules.as_ref(),
                                payload.header_rules,
                                credentials_not_exported,
                            ),
                            body_rules: prepare_imported_secret_safe_body_rules(
                                existing.body_rules.as_ref(),
                                payload.body_rules,
                                credentials_not_exported,
                            ),
                            max_retries: payload.max_retries,
                            is_active: payload.is_active,
                            config: prepare_imported_secret_safe_json(
                                existing.config.as_ref(),
                                payload.config,
                                credentials_not_exported,
                            ),
                            proxy: payload.proxy,
                            format_acceptance_config: prepare_imported_secret_safe_json(
                                existing.format_acceptance_config.as_ref(),
                                payload.format_acceptance_config,
                                credentials_not_exported,
                            ),
                        };
                    let mut updated = invalid!(
                        admin_provider_endpoints_pure::apply_admin_provider_endpoint_update_fields(
                            existing,
                            |field| fields.contains(field),
                            |field| fields.is_null(field),
                            &update_fields,
                        )
                    );
                    updated.api_format = signature.to_string();
                    updated.api_family = Some(api_family.to_string());
                    updated.endpoint_kind = Some(endpoint_kind.to_string());
                    updated
                } else {
                    invalid!(
                        admin_provider_endpoints_pure::build_admin_provider_endpoint_record(
                            Uuid::new_v4().to_string(),
                            provider.id.clone(),
                            signature.to_string(),
                            api_family.to_string(),
                            endpoint_kind.to_string(),
                            invalid!(normalize_admin_base_url(&imported_endpoint.base_url)),
                            imported_endpoint.custom_path,
                            prepare_imported_secret_safe_header_rules(
                                None,
                                imported_endpoint.header_rules,
                                credentials_not_exported,
                            ),
                            prepare_imported_secret_safe_body_rules(
                                None,
                                imported_endpoint.body_rules,
                                credentials_not_exported,
                            ),
                            imported_endpoint.max_retries.unwrap_or(2),
                            prepare_imported_secret_safe_json(
                                None,
                                imported_endpoint.config,
                                credentials_not_exported,
                            ),
                            prepare_imported_secret_safe_proxy(
                                None,
                                imported_endpoint.proxy,
                                credentials_not_exported,
                                &node_id_map,
                            ),
                            prepare_imported_secret_safe_json(
                                None,
                                imported_endpoint.format_acceptance_config,
                                credentials_not_exported,
                            ),
                            0,
                        )
                    )
                };
                existing_endpoints_by_format.insert(normalized_api_format, endpoint);
            }

            let endpoint_formats = existing_endpoints_by_format
                .keys()
                .cloned()
                .collect::<BTreeSet<_>>();
            let mut existing_keys = match keys_by_provider.remove(&provider_name) {
                Some(keys) => keys,
                None => {
                    self.list_provider_catalog_keys_by_provider_ids(std::slice::from_ref(
                        &provider.id,
                    ))
                    .await?
                }
            };
            let imported_keys = routed!(parse_admin_system_config_nested_array::<
                ImportedProviderKey,
            >(raw_provider, "api_keys"));
            let mut imported_credential_identities = BTreeSet::new();
            for imported_key_item in imported_keys {
                let (raw_key, imported_key) = imported_key_item.into_parts();
                let (normalized_api_formats, _) =
                    normalize_import_key_formats(&imported_key, &endpoint_formats);
                if normalized_api_formats.is_empty() {
                    continue;
                }
                let normalized_auth_config = invalid!(normalize_import_auth_config(
                    imported_key.auth_config.clone()
                ));
                let auth_type = imported_key_auth_type(&imported_key);
                let credentials_not_exported = invalid!(
                    validate_imported_provider_key_credential_state(&imported_key)
                );
                if imported_provider_credential_identity(
                    &imported_key,
                    &auth_type,
                    normalized_auth_config.as_ref(),
                )
                .is_some_and(|identity| !imported_credential_identities.insert(identity))
                {
                    return Ok(Err(invalid_request(format!(
                        "Provider '{provider_name}' 的凭据在导入文档中重复"
                    ))));
                }
                let normalized_raw_key = normalize_import_key_raw_payload(
                    &raw_key,
                    &auth_type,
                    &normalized_api_formats,
                    normalized_auth_config.clone(),
                    credentials_not_exported,
                );
                let existing_key_index = if credentials_not_exported {
                    build_import_key_match_name(&imported_key).and_then(|target_name| {
                        existing_keys.iter().position(|existing_key| {
                            existing_key
                                .auth_type
                                .trim()
                                .eq_ignore_ascii_case(&auth_type)
                                && existing_key.name == target_name
                        })
                    })
                } else {
                    invalid!(find_imported_provider_key_index(
                        self,
                        &imported_key,
                        &auth_type,
                        normalized_auth_config.as_ref(),
                        &existing_keys,
                    ))
                };
                if credentials_not_exported && existing_key_index.is_none() {
                    continue;
                }
                if existing_key_index.is_some() && merge_mode == AdminImportMergeMode::Skip {
                    continue;
                }
                let mut record = if let Some(existing_index) = existing_key_index {
                    if merge_mode == AdminImportMergeMode::Error {
                        return Ok(Err(invalid_request(format!(
                            "Provider '{provider_name}' 中存在重复 Key"
                        ))));
                    }
                    let patch = AdminProviderKeyUpdatePatch::from_object(normalized_raw_key)
                        .map_err(|_| "Provider Key 配置格式无效".to_string());
                    let patch = invalid!(patch);
                    let mut record =
                        invalid!(build_admin_update_provider_key_record_with_existing_keys(
                            self,
                            &provider,
                            &existing_keys[existing_index],
                            &existing_keys,
                            patch,
                        ));
                    if credentials_not_exported {
                        record.encrypted_api_key =
                            existing_keys[existing_index].encrypted_api_key.clone();
                        record.encrypted_auth_config =
                            existing_keys[existing_index].encrypted_auth_config.clone();
                    }
                    record
                } else {
                    invalid!(prevalidate_imported_provider_key_uniqueness(
                        self,
                        &imported_key,
                        &auth_type,
                        normalized_auth_config.as_ref(),
                        &existing_keys,
                    ));
                    let payload = serde_json::from_value::<AdminProviderKeyCreateRequest>(
                        Value::Object(normalized_raw_key),
                    )
                    .map_err(|_| "Provider Key 配置格式无效".to_string());
                    invalid!(
                        self.build_admin_create_provider_key_record(&provider, invalid!(payload))
                            .await
                    )
                };
                if auth_type == "oauth" {
                    invalid!(apply_imported_oauth_key_credentials(
                        self,
                        &provider.provider_type,
                        None,
                        &raw_key,
                        normalized_auth_config.as_ref(),
                        &mut record,
                    ));
                }
                if existing_key_index.is_none() || merge_mode != AdminImportMergeMode::Skip {
                    invalid!(normalize_json_object(
                        imported_key.global_priority_by_format.clone(),
                        "global_priority_by_format",
                    ));
                    invalid!(normalize_json_object(
                        imported_key.fingerprint.clone(),
                        "fingerprint",
                    ));
                }
                if let Some(index) = existing_key_index {
                    existing_keys[index] = record;
                } else {
                    existing_keys.push(record);
                }
            }

            let imported_models = routed!(parse_admin_system_config_nested_array::<
                ImportedProviderModel,
            >(raw_provider, "models"));
            let mut existing_models_by_name = match models_by_provider.remove(&provider_name) {
                Some(models) => models,
                None => self
                    .list_all_admin_provider_models_for_system_transfer(&provider.id)
                    .await?
                    .into_iter()
                    .map(|model| {
                        (
                            model.provider_model_name.clone(),
                            (model.id.clone(), Some(model)),
                        )
                    })
                    .collect::<BTreeMap<_, _>>(),
            };
            for imported_model_item in imported_models {
                let imported_model = imported_model_item.value;
                let Some(global_model_name) = imported_model
                    .global_model_name
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                else {
                    continue;
                };
                let Some((global_model_id, _)) = global_models_by_name.get(global_model_name)
                else {
                    continue;
                };
                let provider_model_name = invalid!(trim_required(
                    &imported_model.provider_model_name,
                    "provider_model_name",
                ));
                if existing_models_by_name.contains_key(&provider_model_name) {
                    if merge_mode == AdminImportMergeMode::Error {
                        return Ok(Err(invalid_request(format!(
                            "Model '{provider_model_name}' 已存在于 Provider '{provider_name}'"
                        ))));
                    }
                    if merge_mode == AdminImportMergeMode::Skip {
                        continue;
                    }
                }
                let existing_model = existing_models_by_name
                    .get(&provider_model_name)
                    .and_then(|(_, model)| model.as_ref());
                invalid!(build_import_provider_model_record(
                    &provider.id,
                    existing_models_by_name
                        .get(&provider_model_name)
                        .map(|(id, _)| id.as_str()),
                    existing_model,
                    global_model_id,
                    &imported_model,
                    credentials_not_exported,
                ));
                existing_models_by_name
                    .entry(provider_model_name)
                    .or_insert_with(|| (Uuid::new_v4().to_string(), None));
            }

            providers_by_name.insert(provider_name.clone(), provider);
            endpoints_by_provider.insert(provider_name.clone(), existing_endpoints_by_format);
            keys_by_provider.insert(provider_name.clone(), existing_keys);
            models_by_provider.insert(provider_name, existing_models_by_name);
        }

        if let Some(imported_ldap) = imported_ldap.filter(|_| self.has_auth_module_writer()) {
            let ldap_config = imported_ldap.value;
            let existing = self.get_ldap_module_config().await?;
            let server_url = invalid!(trim_required(&ldap_config.server_url, "LDAP 服务器地址"));
            let server_url = invalid!(normalize_ldap_transport_server_url(
                &server_url,
                ldap_config.use_starttls,
            )
            .ok_or_else(|| {
                "LDAP 服务器地址必须使用 ldaps://，或在启用 StartTLS 时使用 ldap://；不得包含凭据、查询参数或片段"
                    .to_string()
            }));
            let bind_dn = invalid!(trim_required(&ldap_config.bind_dn, "绑定 DN"));
            let base_dn = invalid!(trim_required(&ldap_config.base_dn, "Base DN"));
            if !ldap_distinguished_name_is_valid(&bind_dn)
                || !ldap_distinguished_name_is_valid(&base_dn)
            {
                return Ok(Err(invalid_request(
                    "LDAP 绑定 DN 或 Base DN 格式无效或过长",
                )));
            }
            let user_search_filter = invalid!(trim_required(
                ldap_config
                    .user_search_filter
                    .as_deref()
                    .unwrap_or("(uid={username})"),
                "搜索过滤器",
            ));
            if !ldap_search_filter_is_valid(&user_search_filter) {
                return Ok(Err(invalid_request(
                    "LDAP 搜索过滤器格式无效，必须包含 {username} 且使用有限的括号结构",
                )));
            }
            let username_attr = invalid!(trim_required(
                ldap_config.username_attr.as_deref().unwrap_or("uid"),
                "用户名属性",
            ));
            let email_attr = invalid!(trim_required(
                ldap_config.email_attr.as_deref().unwrap_or("mail"),
                "邮箱属性",
            ));
            let display_name_attr = invalid!(trim_required(
                ldap_config.display_name_attr.as_deref().unwrap_or("cn"),
                "显示名称属性",
            ));
            if [
                username_attr.as_str(),
                email_attr.as_str(),
                display_name_attr.as_str(),
            ]
            .into_iter()
            .any(|attribute| !ldap_attribute_description_is_valid(attribute))
            {
                return Ok(Err(invalid_request(
                    "LDAP 用户名、邮箱或显示名称属性格式无效",
                )));
            }
            let connect_timeout = ldap_config.connect_timeout.unwrap_or(10);
            if !(1..=60).contains(&connect_timeout) {
                return Ok(Err(invalid_request(
                    "LDAP connect_timeout 必须在 1 到 60 秒之间",
                )));
            }
            let config = StoredLdapModuleConfig {
                server_url,
                bind_dn,
                // Password mutation is explicit and separate from the replacement snapshot.
                // In particular, Preserve never copies a previously read ciphertext here.
                bind_password_encrypted: None,
                base_dn,
                user_search_filter: Some(user_search_filter),
                username_attr: Some(username_attr),
                email_attr: Some(email_attr),
                display_name_attr: Some(display_name_attr),
                is_enabled: ldap_config.is_enabled,
                is_exclusive: ldap_config.is_exclusive,
                use_starttls: ldap_config.use_starttls,
                connect_timeout: Some(connect_timeout),
            };
            let bind_password = ldap_config
                .bind_password
                .as_deref()
                .map(str::trim)
                .map(ToOwned::to_owned);
            if bind_password
                .as_deref()
                .is_some_and(is_imported_redacted_secret)
            {
                return Ok(Err(invalid_request("LDAP 脱敏占位符不能作为绑定密码导入")));
            }
            let bind_password_update = match bind_password {
                Some(password) if password.is_empty() => LdapBindPasswordUpdate::Clear,
                Some(password) => LdapBindPasswordUpdate::Set(invalid!(self
                    .encrypt_ldap_bind_password(&config, &password)
                    .ok_or_else(|| {
                        "LDAP 绑定密码加密失败，请检查 Rust 数据加密配置".to_string()
                    }))),
                None => LdapBindPasswordUpdate::Preserve,
            };
            if matches!(&bind_password_update, LdapBindPasswordUpdate::Preserve) {
                if let Some(existing) = existing.as_ref() {
                    if existing
                        .bind_password_encrypted
                        .as_deref()
                        .is_some_and(|value| !value.trim().is_empty())
                    {
                        let binding_matches =
                            invalid!(crate::handlers::shared::ldap_bind_password_binding_matches(
                                existing, &config,
                            ));
                        if !binding_matches {
                            return Ok(Err(invalid_request(
                                "导入 LDAP 时修改了服务器、StartTLS、bind DN 或 Base DN，必须提供绑定密码",
                            )));
                        }
                    }
                }
            }
            let will_have_password = match &bind_password_update {
                LdapBindPasswordUpdate::Set(ciphertext) => !ciphertext.trim().is_empty(),
                LdapBindPasswordUpdate::Clear => false,
                LdapBindPasswordUpdate::Preserve => existing
                    .as_ref()
                    .and_then(|config| config.bind_password_encrypted.as_deref())
                    .map(str::trim)
                    .is_some_and(|value| !value.is_empty()),
            };
            if existing.is_none()
                && !matches!(&bind_password_update, LdapBindPasswordUpdate::Set(_))
            {
                return Ok(Err(invalid_request("首次配置 LDAP 时必须设置绑定密码")));
            }
            if ldap_config.is_exclusive && !ldap_config.is_enabled {
                return Ok(Err(invalid_request(
                    "仅允许 LDAP 登录 需要先启用 LDAP 认证",
                )));
            }
            if ldap_config.is_enabled && !will_have_password {
                return Ok(Err(invalid_request("启用 LDAP 认证 需要先设置绑定密码")));
            }
            if ldap_config.is_enabled && ldap_config.is_exclusive {
                let admin_count = self
                    .count_active_local_admin_users_with_valid_password()
                    .await?;
                if admin_count < 1 {
                    return Ok(Err(invalid_request(
                        "启用 LDAP 独占模式前，必须至少保留 1 个有效的本地管理员账户（含有效密码）作为紧急恢复通道",
                    )));
                }
            }
            if existing.is_some() && merge_mode == AdminImportMergeMode::Error {
                return Ok(Err(invalid_request("LDAP 配置已存在")));
            }
        }

        let existing_oauth_providers = self.list_oauth_provider_configs().await?;
        let existing_oauth_by_type = existing_oauth_providers
            .iter()
            .map(|provider| (provider.provider_type.clone(), provider))
            .collect::<BTreeMap<_, _>>();
        let mut oauth_provider_types = existing_oauth_by_type
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        for imported_oauth_item in imported_oauth_providers {
            let oauth_provider = invalid!(normalize_legacy_imported_oauth_provider(
                imported_oauth_item.value,
                &source_version,
            ));
            let provider_type = invalid!(trim_required(
                &oauth_provider.provider_type,
                "provider_type",
            ));
            if oauth_provider_types.contains(&provider_type) {
                match merge_mode {
                    AdminImportMergeMode::Skip => continue,
                    AdminImportMergeMode::Error => {
                        return Ok(Err(invalid_request(format!(
                            "OAuth Provider '{provider_type}' 已存在"
                        ))));
                    }
                    AdminImportMergeMode::Overwrite => {}
                }
            }
            // Construct and validate the complete record before sealing the secret.  The
            // envelope binding includes client_id, redirect URI, and endpoint overrides;
            // sealing against provider_type alone would allow a secret to be replayed after
            // those fields change.
            let mut record = invalid!(build_imported_oauth_provider_record(
                &oauth_provider,
                EncryptedSecretUpdate::Preserve,
            ));
            let client_secret_update = match oauth_provider.client_secret.as_deref().map(str::trim)
            {
                Some(secret) if is_imported_redacted_secret(secret) => {
                    EncryptedSecretUpdate::Preserve
                }
                Some(secret) if !secret.is_empty() => EncryptedSecretUpdate::Set(invalid!(
                    crate::handlers::shared::seal_identity_oauth_provider_client_secret(
                        self.as_ref(),
                        &record,
                        secret,
                    )
                    .map_err(str::to_string)
                )),
                _ => EncryptedSecretUpdate::Preserve,
            };
            record.client_secret_encrypted = client_secret_update;
            if matches!(
                &record.client_secret_encrypted,
                EncryptedSecretUpdate::Preserve
            ) {
                if let Some(existing) = existing_oauth_by_type.get(&provider_type) {
                    if existing
                        .client_secret_encrypted
                        .as_deref()
                        .is_some_and(|value| !value.trim().is_empty())
                    {
                        let binding_matches = invalid!(
                            crate::handlers::shared::identity_oauth_provider_secret_binding_matches(
                                existing, &record,
                            )
                        );
                        if !binding_matches {
                            return Ok(Err(invalid_request(
                                "导入 OAuth Provider 时修改了 Client ID、端点或 redirect_uri，必须提供 client_secret",
                            )));
                        }
                    }
                }
            }
            oauth_provider_types.insert(provider_type);
        }

        for imported_config_item in imported_system_configs {
            let config = imported_config_item.value;
            let normalized_key = normalize_imported_system_config_key(&config.key);
            if credentials_not_exported
                && (is_sensitive_admin_system_config_key(&normalized_key)
                    || is_interactive_export_private_system_config_key(&normalized_key))
            {
                continue;
            }
            let exists = existing_system_config_keys.contains(&normalized_key);
            match (exists, merge_mode) {
                (true, AdminImportMergeMode::Skip) => continue,
                (true, AdminImportMergeMode::Error) => {
                    return Ok(Err(invalid_request(format!(
                        "SystemConfig '{normalized_key}' 已存在"
                    ))));
                }
                _ => {}
            }
            let request_body = serde_json::to_vec(&json!({
                "value": config.value,
                "description": config.description,
            }))
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
            let update = routed!(parse_admin_system_config_update(&config.key, &request_body));
            if is_sensitive_admin_system_config_key(&update.normalized_key)
                && update.value.as_str().is_some_and(|raw| !raw.is_empty())
            {
                let Some(_) = self.encrypt_system_config_secret(
                    &update.normalized_key,
                    update
                        .value
                        .as_str()
                        .expect("sensitive imported config value was a string"),
                ) else {
                    return Ok(Err((
                        http::StatusCode::SERVICE_UNAVAILABLE,
                        json!({ "detail": "系统配置写入需要可用的加密密钥" }),
                    )));
                };
            }
            existing_system_config_keys.insert(normalized_key);
        }

        Ok(Ok(()))
    }

    async fn prevalidate_admin_system_users_import(
        &self,
        request_body: &[u8],
        operator_id: Option<&str>,
        mode: SystemImportMode,
    ) -> Result<Result<(), (http::StatusCode, Value)>, GatewayError> {
        macro_rules! invalid {
            ($expr:expr) => {
                match $expr {
                    Ok(value) => value,
                    Err(detail) => return Ok(Err(invalid_request(detail))),
                }
            };
        }

        let root = match serde_json::from_slice::<Value>(request_body) {
            Ok(Value::Object(map)) => map,
            _ => return Ok(Err(invalid_request("请求数据验证失败"))),
        };
        let merge_mode = match serde_json::from_value::<AdminImportMergeMode>(
            root.get("merge_mode").cloned().unwrap_or(Value::Null),
        ) {
            Ok(value) => value,
            Err(_) => {
                return Ok(Err(invalid_request(
                    "merge_mode 仅支持 skip / overwrite / error",
                )))
            }
        };
        let users_export_version = invalid!(
            validate_imported_system_users_export_version_for_mode(root.get("version"), mode)
        );
        let empty = Vec::new();
        let users = match root.get("users") {
            Some(Value::Array(items)) => items,
            Some(_) => return Ok(Err(invalid_request("users 必须是数组"))),
            None => &empty,
        };
        let standalone_keys = match root.get("standalone_keys") {
            Some(Value::Array(items)) => items,
            Some(_) => return Ok(Err(invalid_request("standalone_keys 必须是数组"))),
            None => &empty,
        };
        let imported_user_groups = match root.get("user_groups") {
            Some(Value::Array(items)) => items,
            Some(_) => return Ok(Err(invalid_request("user_groups 必须是数组"))),
            None => &empty,
        };
        // Aggregate rollback restores identity/configuration only. Runtime usage rows are left
        // untouched so a request that completes concurrently cannot be overwritten by the
        // checkpoint. Skip both parsing and conflict validation in this mode; the rollback body
        // also strips these fields before it reaches the regular importer.
        let usage_aggregate_snapshot = if mode.is_rollback_checkpoint() {
            AdminSystemUsageAggregateSnapshot::default()
        } else {
            let supplemental = invalid!(build_imported_user_usage_total_aggregates(
                users,
                root.get("exported_at")
            ));
            let snapshot = invalid!(build_imported_usage_aggregate_snapshot(
                root.get("usage_aggregates"),
                &supplemental,
            ));
            invalid!(validate_imported_usage_aggregate_storage(&snapshot));
            snapshot
        };

        let default_group_id = self.effective_default_user_group_id().await?;
        let mut groups_by_name = self
            .list_user_groups()
            .await?
            .into_iter()
            .map(|group| {
                (
                    aether_data::repository::users::normalize_user_group_name(&group.name)
                        .to_ascii_lowercase(),
                    group,
                )
            })
            .collect::<BTreeMap<_, _>>();
        let mut imported_group_id_map = BTreeMap::<String, String>::new();
        let mut imported_group_name_map = BTreeMap::<String, String>::new();
        for (index, raw_group) in imported_user_groups.iter().enumerate() {
            let group = invalid!(imported_object_field(
                raw_group,
                &format!("user_groups[{index}]"),
            ));
            let (export_id, normalized_name, record) = invalid!(build_imported_user_group_record(
                group,
                &format!("user_groups[{index}]")
            ));
            if default_group_id
                .as_deref()
                .is_some_and(|group_id| export_id.as_deref() == Some(group_id))
                || normalized_name == "default"
            {
                if let Some(default_group_id) = default_group_id.as_ref() {
                    if let Some(export_id) = export_id {
                        imported_group_id_map.insert(export_id, default_group_id.clone());
                    }
                    imported_group_name_map.insert(normalized_name, default_group_id.clone());
                }
                continue;
            }
            let existing_by_id = mode
                .is_rollback_checkpoint()
                .then(|| {
                    export_id.as_deref().and_then(|export_id| {
                        groups_by_name.values().find(|group| group.id == export_id)
                    })
                })
                .flatten();
            if mode.is_rollback_checkpoint() && export_id.is_some() && existing_by_id.is_none() {
                return Ok(Err(invalid_request(format!(
                    "回滚检查点用户组 '{}' 不存在；拒绝按名称匹配",
                    export_id.as_deref().unwrap_or_default()
                ))));
            }
            if let Some(existing) = existing_by_id.or_else(|| groups_by_name.get(&normalized_name))
            {
                if merge_mode == AdminImportMergeMode::Error {
                    return Ok(Err(invalid_request(format!(
                        "用户组 '{}' 已存在",
                        existing.name
                    ))));
                }
                if let Some(export_id) = export_id {
                    imported_group_id_map.insert(export_id, existing.id.clone());
                }
                imported_group_name_map.insert(normalized_name, existing.id.clone());
            } else {
                let synthetic_id = format!("prevalidated-group-{index}");
                let stored = aether_data::repository::users::StoredUserGroup::new(
                    synthetic_id.clone(),
                    record.name.clone(),
                    normalized_name.clone(),
                    record.description.clone(),
                    record.priority,
                    record.allowed_providers.clone().map(Value::from),
                    record.allowed_providers_mode.clone(),
                    if record.provider_key_policies.is_empty() {
                        None
                    } else {
                        Some(
                            serde_json::to_value(&record.provider_key_policies).unwrap_or_default(),
                        )
                    },
                    record.allowed_api_formats.clone().map(Value::from),
                    record.allowed_api_formats_mode.clone(),
                    record.allowed_models.clone().map(Value::from),
                    record.allowed_models_mode.clone(),
                    record.rate_limit,
                    record.rate_limit_mode.clone(),
                    None,
                    None,
                )
                .map_err(|err| GatewayError::Internal(err.to_string()))?;
                if let Some(export_id) = export_id {
                    imported_group_id_map.insert(export_id, synthetic_id.clone());
                }
                imported_group_name_map.insert(normalized_name.clone(), synthetic_id);
                groups_by_name.insert(normalized_name, stored);
            }
        }

        let standalone_owner_id = match operator_id {
            Some(candidate) => match self.find_user_auth_by_id(candidate).await? {
                Some(user) if crate::roles::is_full_admin_role(&user.role) => Some(user.id),
                _ => None,
            },
            None => None,
        };
        let mut simulated_users_by_id = BTreeMap::<String, SimulatedImportedUser>::new();
        let mut simulated_email_owners = BTreeMap::<String, String>::new();
        let mut simulated_username_owners = BTreeMap::<String, String>::new();
        let mut released_emails = BTreeSet::<String>::new();
        let mut released_usernames = BTreeSet::<String>::new();
        let mut api_keys_by_hash = BTreeMap::<String, SimulatedImportedApiKey>::new();
        let mut imported_api_key_hashes = BTreeSet::<String>::new();
        let mut imported_user_id_map = BTreeMap::<String, String>::new();
        let mut imported_api_key_id_map = BTreeMap::<String, String>::new();
        for user in self.list_export_users().await? {
            replace_simulated_imported_user(
                &mut simulated_users_by_id,
                &mut simulated_email_owners,
                &mut simulated_username_owners,
                &mut released_emails,
                &mut released_usernames,
                SimulatedImportedUser {
                    id: user.id,
                    email: user.email,
                    username: user.username,
                    role: user.role,
                    existed_before_import: true,
                },
            );
        }
        #[cfg(test)]
        if let Some(store) = self.app().auth_user_store.as_ref() {
            for user in store.lock().expect("auth user store should lock").values() {
                replace_simulated_imported_user(
                    &mut simulated_users_by_id,
                    &mut simulated_email_owners,
                    &mut simulated_username_owners,
                    &mut released_emails,
                    &mut released_usernames,
                    simulated_imported_user_from_auth_record(user),
                );
            }
        }
        for record in self.list_auth_api_key_export_standalone_records().await? {
            let simulated = SimulatedImportedApiKey {
                owner_id: record.user_id,
                is_standalone: true,
                target_id: record.api_key_id.clone(),
                existed_before_import: true,
            };
            api_keys_by_hash.insert(record.key_hash, simulated.clone());
            if mode.is_rollback_checkpoint() {
                api_keys_by_hash
                    .entry(imported_api_key_tombstone(&record.api_key_id))
                    .or_insert(simulated);
            }
        }
        let now_unix_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|duration| duration.as_secs())
            .unwrap_or(0);

        for (index, raw_user) in users.iter().enumerate() {
            let user = invalid!(imported_object_field(raw_user, &format!("users[{index}]")));
            let source_user_id = invalid!(imported_optional_string(user.get("id")));
            invalid!(validate_rollback_user_source_id(
                mode,
                source_user_id.as_deref(),
            ));
            let Some(role) = invalid!(normalize_imported_system_user_role(user.get("role"), mode))
            else {
                invalid!(imported_optional_string(user.get("email")));
                invalid!(imported_optional_string(user.get("username")));
                continue;
            };
            let email = invalid!(imported_optional_string(user.get("email")))
                .map(|value| value.to_ascii_lowercase());
            let username = invalid!(imported_optional_string(user.get("username")))
                .or_else(|| {
                    email.as_ref().map(|value| {
                        value
                            .split('@')
                            .next()
                            .unwrap_or(value.as_str())
                            .to_string()
                    })
                })
                .unwrap_or_else(|| format!("imported-user-{index}"));
            invalid!(imported_optional_bool(user.get("email_verified")));
            invalid!(resolve_imported_password_hash(
                user,
                users_export_version,
                mode,
            ));
            let allowed_providers = invalid!(normalize_imported_user_string_list(
                user,
                "allowed_providers"
            ));
            let allowed_api_formats = invalid!(normalize_imported_user_api_formats(
                user,
                "allowed_api_formats"
            ));
            let allowed_models =
                invalid!(normalize_imported_user_string_list(user, "allowed_models"));
            let rate_limit = invalid!(imported_optional_i32(user.get("rate_limit"), "rate_limit"));
            invalid!(imported_user_list_policy_mode(
                user,
                "allowed_providers_mode",
                "allowed_providers",
                &allowed_providers,
            ));
            invalid!(imported_user_list_policy_mode(
                user,
                "allowed_api_formats_mode",
                "allowed_api_formats",
                &allowed_api_formats,
            ));
            invalid!(imported_user_list_policy_mode(
                user,
                "allowed_models_mode",
                "allowed_models",
                &allowed_models,
            ));
            invalid!(imported_user_rate_limit_policy_mode(
                user,
                "rate_limit_mode",
                "rate_limit",
                rate_limit,
            ));
            let group_ids = invalid!(resolve_imported_user_group_ids(
                user,
                &imported_group_id_map,
                &imported_group_name_map,
                &groups_by_name,
            ));
            if user.contains_key("group_ids") || user.contains_key("group_names") {
                let group_ids = self.include_default_user_group_ids(&group_ids).await?;
                let known_group_ids = groups_by_name
                    .values()
                    .map(|group| group.id.as_str())
                    .collect::<BTreeSet<_>>();
                if group_ids
                    .iter()
                    .any(|group_id| !known_group_ids.contains(group_id.as_str()))
                {
                    return Ok(Err(invalid_request(format!(
                        "用户 '{}' 的用户组不存在",
                        email.clone().unwrap_or(username.clone())
                    ))));
                }
            }
            invalid!(imported_optional_bool(user.get("is_active")));
            invalid!(imported_optional_json_object(
                user.get("model_capability_settings"),
                "model_capability_settings",
            ));
            invalid!(imported_optional_json_object(
                user.get("feature_settings"),
                "feature_settings",
            )
            .and_then(normalize_admin_feature_settings));
            let wallet = match user.get("wallet") {
                Some(Value::Object(map)) => Some(map),
                Some(Value::Null) | None => None,
                Some(_) => return Ok(Err(invalid_request("wallet 必须是对象"))),
            };
            if let Some(wallet) = wallet {
                invalid!(normalize_imported_wallet_target(Some(wallet), false));
            }

            // Rollback checkpoints originate from this deployment and carry stable user IDs.
            // Never fall back to mutable email/username fields: if the checkpoint ID disappeared
            // concurrently, guessing could overwrite an unrelated user.
            let existing_user = if mode.is_rollback_checkpoint() {
                let mut existing = source_user_id
                    .as_deref()
                    .and_then(|user_id| simulated_users_by_id.get(user_id).cloned());
                if existing.is_none() {
                    if let Some(source_user_id) = source_user_id.as_deref() {
                        if let Some(record) = self.find_user_auth_by_id(source_user_id).await? {
                            let simulated = simulated_imported_user_from_auth_record(&record);
                            replace_simulated_imported_user(
                                &mut simulated_users_by_id,
                                &mut simulated_email_owners,
                                &mut simulated_username_owners,
                                &mut released_emails,
                                &mut released_usernames,
                                simulated.clone(),
                            );
                            existing = Some(simulated);
                        }
                    }
                }
                if existing.is_none() {
                    let source_user_id = source_user_id.as_deref().unwrap_or_default();
                    return Ok(Err(invalid_request(format!(
                        "回滚检查点用户 '{source_user_id}' 不存在；拒绝按 email/username 匹配"
                    ))));
                }
                existing
            } else {
                let mut existing_user = None;
                if let Some(email) = email.as_deref() {
                    if let Some(user_id) = simulated_imported_user_id_by_identifier(
                        &simulated_email_owners,
                        &simulated_username_owners,
                        email,
                    ) {
                        existing_user = simulated_users_by_id.get(&user_id).cloned();
                    } else if !released_emails.contains(email)
                        && !released_usernames.contains(email)
                    {
                        if let Some(record) = self.find_user_auth_by_identifier(email).await? {
                            let simulated = simulated_imported_user_from_auth_record(&record);
                            replace_simulated_imported_user(
                                &mut simulated_users_by_id,
                                &mut simulated_email_owners,
                                &mut simulated_username_owners,
                                &mut released_emails,
                                &mut released_usernames,
                                simulated.clone(),
                            );
                            existing_user = Some(simulated);
                        }
                    }
                }
                if existing_user.is_none() {
                    if let Some(user_id) = simulated_imported_user_id_by_identifier(
                        &simulated_email_owners,
                        &simulated_username_owners,
                        &username,
                    ) {
                        existing_user = simulated_users_by_id.get(&user_id).cloned();
                    } else if !released_emails.contains(&username)
                        && !released_usernames.contains(&username)
                    {
                        if let Some(record) = self.find_user_auth_by_identifier(&username).await? {
                            let simulated = simulated_imported_user_from_auth_record(&record);
                            replace_simulated_imported_user(
                                &mut simulated_users_by_id,
                                &mut simulated_email_owners,
                                &mut simulated_username_owners,
                                &mut released_emails,
                                &mut released_usernames,
                                simulated.clone(),
                            );
                            existing_user = Some(simulated);
                        }
                    }
                }
                existing_user
            };

            let label = email.clone().unwrap_or(username.clone());
            let simulated_user = if let Some(existing) = existing_user {
                if imported_existing_user_is_protected(&existing.role, mode) {
                    continue;
                }
                match merge_mode {
                    AdminImportMergeMode::Skip => continue,
                    AdminImportMergeMode::Error => {
                        return Ok(Err(invalid_request(format!("用户 '{label}' 已存在"))));
                    }
                    AdminImportMergeMode::Overwrite => {}
                }
                if let Some(email) = email.as_deref() {
                    let taken_in_simulation = simulated_email_owners
                        .get(email)
                        .is_some_and(|owner_id| owner_id != &existing.id);
                    let taken_in_database = !released_emails.contains(email)
                        && self
                            .is_other_user_auth_email_taken(email, &existing.id)
                            .await?;
                    if taken_in_simulation || taken_in_database {
                        return Ok(Err(invalid_request(format!("邮箱已存在: {email}"))));
                    }
                }
                let username_taken_in_simulation = simulated_username_owners
                    .get(&username)
                    .is_some_and(|owner_id| owner_id != &existing.id);
                let username_taken_in_database = !released_usernames.contains(&username)
                    && self
                        .is_other_user_auth_username_taken(&username, &existing.id)
                        .await?;
                if username_taken_in_simulation || username_taken_in_database {
                    return Ok(Err(invalid_request(format!("用户名已存在: {username}"))));
                }
                SimulatedImportedUser {
                    id: existing.id,
                    email: email.clone().or(existing.email),
                    username,
                    role,
                    existed_before_import: existing.existed_before_import,
                }
            } else {
                if email.as_ref().is_some_and(|email| {
                    simulated_email_owners.contains_key(email)
                        || (!released_emails.contains(email)
                            && simulated_username_owners.contains_key(email))
                }) || simulated_username_owners.contains_key(&username)
                {
                    return Ok(Err(invalid_request(format!("用户 '{label}' 已存在"))));
                }
                SimulatedImportedUser {
                    id: format!("prevalidated-user-{index}"),
                    email,
                    username,
                    role,
                    existed_before_import: false,
                }
            };
            let user_id = simulated_user.id.clone();
            let existed_before_import = simulated_user.existed_before_import;
            replace_simulated_imported_user(
                &mut simulated_users_by_id,
                &mut simulated_email_owners,
                &mut simulated_username_owners,
                &mut released_emails,
                &mut released_usernames,
                simulated_user,
            );
            if let Some(source_user_id) = source_user_id {
                invalid!(insert_imported_id_mapping(
                    &mut imported_user_id_map,
                    source_user_id,
                    user_id.clone(),
                    "users[].id",
                ));
            }

            if existed_before_import {
                for record in self
                    .list_auth_api_key_export_records_by_user_ids(std::slice::from_ref(&user_id))
                    .await?
                    .into_iter()
                    .filter(|record| !record.is_standalone)
                {
                    let simulated = SimulatedImportedApiKey {
                        owner_id: user_id.clone(),
                        is_standalone: false,
                        target_id: record.api_key_id.clone(),
                        existed_before_import: true,
                    };
                    api_keys_by_hash
                        .entry(record.key_hash)
                        .or_insert_with(|| simulated.clone());
                    if mode.is_rollback_checkpoint() {
                        api_keys_by_hash
                            .entry(imported_api_key_tombstone(&record.api_key_id))
                            .or_insert(simulated);
                    }
                }
            }

            let imported_api_keys = match user.get("api_keys") {
                Some(Value::Array(items)) => items,
                Some(_) => return Ok(Err(invalid_request("api_keys 必须是数组"))),
                None => &empty,
            };
            for (key_index, raw_key) in imported_api_keys.iter().enumerate() {
                let key = invalid!(imported_object_field(
                    raw_key,
                    &format!("users[{index}].api_keys[{key_index}]"),
                ));
                invalid!(self.prevalidate_imported_auth_api_key(key, users_export_version, mode,));
                let source_api_key_id = invalid!(imported_optional_string(key.get("api_key_id")));
                let Some(key_material) = invalid!(self
                    .resolve_imported_system_user_api_key_material(
                        key,
                        users_export_version,
                        mode,
                    ))
                else {
                    continue;
                };
                let key_hash = key_material.key_hash;
                if !imported_api_key_hashes.insert(key_hash.clone()) {
                    return Ok(Err(invalid_request("API Key 在导入文档中重复")));
                }
                if !api_keys_by_hash.contains_key(&key_hash) {
                    if let Some(snapshot) = self
                        .app()
                        .data
                        .read_auth_api_key_snapshot_by_key_hash_strong(&key_hash, now_unix_secs)
                        .await
                        .map_err(|err| GatewayError::Internal(err.to_string()))?
                    {
                        api_keys_by_hash.insert(
                            key_hash.clone(),
                            SimulatedImportedApiKey {
                                owner_id: snapshot.user_id,
                                is_standalone: snapshot.api_key_is_standalone,
                                target_id: snapshot.api_key_id,
                                existed_before_import: true,
                            },
                        );
                    }
                }
                if let Some(existing_key) = api_keys_by_hash.get(&key_hash) {
                    if existing_key.owner_id != user_id || existing_key.is_standalone {
                        return Ok(Err(invalid_request(
                            "API Key 已存在且属于其他用户或独立余额 Key",
                        )));
                    }
                    if merge_mode == AdminImportMergeMode::Error {
                        return Ok(Err(invalid_request(format!(
                            "用户 '{label}' 的 API Key 已存在"
                        ))));
                    }
                    if merge_mode == AdminImportMergeMode::Overwrite {
                        if let Some(source_api_key_id) = source_api_key_id {
                            invalid!(insert_imported_id_mapping(
                                &mut imported_api_key_id_map,
                                source_api_key_id,
                                existing_key.target_id.clone(),
                                "api_key_id",
                            ));
                        }
                    }
                } else {
                    let target_id = format!("prevalidated-api-key-{index}-{key_index}");
                    if let Some(source_api_key_id) = source_api_key_id {
                        invalid!(insert_imported_id_mapping(
                            &mut imported_api_key_id_map,
                            source_api_key_id,
                            target_id.clone(),
                            "api_key_id",
                        ));
                    }
                    api_keys_by_hash.insert(
                        key_hash,
                        SimulatedImportedApiKey {
                            owner_id: user_id.clone(),
                            is_standalone: false,
                            target_id,
                            existed_before_import: false,
                        },
                    );
                }
            }
        }

        if let Some(standalone_owner_id) = standalone_owner_id {
            for (index, raw_key) in standalone_keys.iter().enumerate() {
                let key = invalid!(imported_object_field(
                    raw_key,
                    &format!("standalone_keys[{index}]"),
                ));
                invalid!(self.prevalidate_imported_auth_api_key(key, users_export_version, mode,));
                let source_api_key_id = invalid!(imported_optional_string(key.get("api_key_id")));
                let Some(key_material) = invalid!(self
                    .resolve_imported_system_user_api_key_material(
                        key,
                        users_export_version,
                        mode,
                    ))
                else {
                    continue;
                };
                let key_hash = key_material.key_hash;
                if !imported_api_key_hashes.insert(key_hash.clone()) {
                    return Ok(Err(invalid_request("API Key 在导入文档中重复")));
                }
                let wallet = match key.get("wallet") {
                    Some(Value::Object(map)) => Some(map),
                    Some(Value::Null) | None => None,
                    Some(_) => return Ok(Err(invalid_request("wallet 必须是对象"))),
                };
                let unlimited =
                    invalid!(imported_optional_bool(key.get("unlimited"))).unwrap_or(false);
                if let Some(wallet) = wallet {
                    invalid!(normalize_imported_wallet_target(Some(wallet), unlimited));
                }
                if !api_keys_by_hash.contains_key(&key_hash) {
                    if let Some(snapshot) = self
                        .app()
                        .data
                        .read_auth_api_key_snapshot_by_key_hash_strong(&key_hash, now_unix_secs)
                        .await
                        .map_err(|err| GatewayError::Internal(err.to_string()))?
                    {
                        api_keys_by_hash.insert(
                            key_hash.clone(),
                            SimulatedImportedApiKey {
                                owner_id: snapshot.user_id,
                                is_standalone: snapshot.api_key_is_standalone,
                                target_id: snapshot.api_key_id,
                                existed_before_import: true,
                            },
                        );
                    }
                }
                if let Some(existing_key) = api_keys_by_hash.get(&key_hash) {
                    if !existing_key.is_standalone {
                        return Ok(Err(invalid_request("独立余额 Key 已存在且属于普通用户")));
                    }
                    if merge_mode == AdminImportMergeMode::Error {
                        return Ok(Err(invalid_request("独立余额 Key 已存在")));
                    }
                    if merge_mode == AdminImportMergeMode::Overwrite {
                        if let Some(source_api_key_id) = source_api_key_id {
                            invalid!(insert_imported_id_mapping(
                                &mut imported_api_key_id_map,
                                source_api_key_id,
                                existing_key.target_id.clone(),
                                "api_key_id",
                            ));
                        }
                    }
                } else {
                    let target_id = format!("prevalidated-standalone-key-{index}");
                    if let Some(source_api_key_id) = source_api_key_id {
                        invalid!(insert_imported_id_mapping(
                            &mut imported_api_key_id_map,
                            source_api_key_id,
                            target_id.clone(),
                            "api_key_id",
                        ));
                    }
                    api_keys_by_hash.insert(
                        key_hash,
                        SimulatedImportedApiKey {
                            owner_id: standalone_owner_id.clone(),
                            is_standalone: true,
                            target_id,
                            existed_before_import: false,
                        },
                    );
                }
            }
        }

        invalid!(validate_imported_usage_aggregate_dimensions(
            &usage_aggregate_snapshot,
            &imported_user_id_map,
            &imported_api_key_id_map,
        ));

        if merge_mode == AdminImportMergeMode::Error {
            let persisted_user_target_ids = simulated_users_by_id
                .values()
                .filter(|user| user.existed_before_import)
                .map(|user| user.id.clone())
                .collect::<BTreeSet<_>>();
            let persisted_api_key_target_ids = api_keys_by_hash
                .values()
                .filter(|key| key.existed_before_import)
                .map(|key| key.target_id.clone())
                .collect::<BTreeSet<_>>();
            invalid!(
                self.prevalidate_imported_usage_aggregate_conflicts(
                    &usage_aggregate_snapshot,
                    &imported_user_id_map,
                    &imported_api_key_id_map,
                    &persisted_user_target_ids,
                    &persisted_api_key_target_ids,
                )
                .await
            );
        }

        Ok(Ok(()))
    }

    fn prevalidate_imported_auth_api_key(
        &self,
        key: &Map<String, Value>,
        users_export_version: (u32, u32),
        mode: SystemImportMode,
    ) -> Result<(), String> {
        if self
            .resolve_imported_system_user_api_key_material(key, users_export_version, mode)?
            .is_none()
        {
            return Ok(());
        }
        imported_optional_string(key.get("api_key_id"))?;
        imported_optional_string(key.get("name"))?;
        normalize_imported_user_string_list(key, "allowed_providers")?;
        normalize_imported_user_api_formats(key, "allowed_api_formats")?;
        normalize_imported_user_string_list(key, "allowed_models")?;
        normalize_imported_user_ip_rules(key)?;
        imported_optional_i32(key.get("rate_limit"), "rate_limit")?;
        let concurrent_limit =
            imported_optional_i32(key.get("concurrent_limit"), "concurrent_limit")?;
        if concurrent_limit.is_some_and(|value| value < 0) {
            return Err("concurrent_limit 必须是非负整数".to_string());
        }
        imported_optional_bool(key.get("is_active"))?;
        imported_rfc3339_to_unix_secs(key.get("expires_at"), "expires_at")?;
        imported_optional_bool(key.get("auto_delete_on_expiry"))?;
        if let Some(value) = imported_optional_u64(key.get("total_requests"), "total_requests")? {
            validate_imported_u64_storage(value, "total_requests")?;
        }
        if let Some(value) = imported_optional_u64(key.get("total_tokens"), "total_tokens")? {
            validate_imported_u64_storage(value, "total_tokens")?;
        }
        imported_optional_f64(key.get("total_cost_usd"), "total_cost_usd")?;
        imported_optional_json_object(key.get("feature_settings"), "feature_settings")
            .and_then(normalize_admin_feature_settings)?;
        Ok(())
    }

    async fn prevalidate_imported_usage_aggregate_conflicts(
        &self,
        snapshot: &AdminSystemUsageAggregateSnapshot,
        user_id_map: &BTreeMap<String, String>,
        api_key_id_map: &BTreeMap<String, String>,
        persisted_user_target_ids: &BTreeSet<String>,
        persisted_api_key_target_ids: &BTreeSet<String>,
    ) -> Result<(), String> {
        if snapshot.stats_daily.is_empty()
            && snapshot.stats_user_daily.is_empty()
            && snapshot.stats_daily_api_key.is_empty()
        {
            return Ok(());
        }

        if !self.app().data.has_backends() {
            return Ok(());
        }

        let persisted_user_ids = user_id_map
            .iter()
            .filter(|(_, target_id)| persisted_user_target_ids.contains(*target_id))
            .map(|(source_id, target_id)| (source_id.clone(), target_id.clone()))
            .collect::<BTreeMap<_, _>>();
        let persisted_api_key_ids = api_key_id_map
            .iter()
            .filter(|(_, target_id)| persisted_api_key_target_ids.contains(*target_id))
            .map(|(source_id, target_id)| (source_id.clone(), target_id.clone()))
            .collect::<BTreeMap<_, _>>();

        match self
            .import_admin_system_usage_aggregates(
                snapshot,
                &persisted_user_ids,
                &persisted_api_key_ids,
                AdminSystemUsageAggregateImportMode::ValidateError,
            )
            .await
        {
            Err(GatewayError::Client { message, .. }) => Err(message),
            Err(_) => {
                tracing::error!(
                    event_name = "admin_system_import_usage_prevalidation_error",
                    operation = "prevalidate_usage_aggregate_conflicts",
                    error_category = "repository_unavailable",
                    "admin system import usage prevalidation failed"
                );
                Err("Usage aggregate data temporarily unavailable".to_string())
            }
            Ok(_) => Ok(()),
        }
    }

    pub(crate) async fn import_admin_system_data(
        &self,
        request_body: &Bytes,
        operator_id: Option<&str>,
    ) -> Result<Result<Value, (http::StatusCode, Value)>, GatewayError> {
        self.import_admin_system_data_with_mode(
            request_body,
            operator_id,
            SystemImportMode::InteractiveUpload,
        )
        .await
    }

    pub(crate) async fn restore_admin_system_data_backup(
        &self,
        request_body: &Bytes,
        operator_id: Option<&str>,
        _authority: crate::backup::executor::BackupRestoreAuthority,
    ) -> Result<Result<Value, (http::StatusCode, Value)>, GatewayError> {
        self.import_admin_system_data_with_mode(
            request_body,
            operator_id,
            SystemImportMode::RecoveryBackup,
        )
        .await
    }

    async fn import_admin_system_data_with_mode(
        &self,
        request_body: &Bytes,
        operator_id: Option<&str>,
        mode: SystemImportMode,
    ) -> Result<Result<Value, (http::StatusCode, Value)>, GatewayError> {
        if !self.has_global_model_data_reader()
            || !self.has_global_model_data_writer()
            || !self.has_provider_catalog_data_reader()
            || !self.has_provider_catalog_data_writer()
            || !self.has_auth_user_write_capability()
            || !self.has_auth_wallet_write_capability()
            || !self.has_auth_api_key_writer()
        {
            return Ok(Err((
                http::StatusCode::SERVICE_UNAVAILABLE,
                json!({ "detail": "Admin system data unavailable" }),
            )));
        }

        let root = match serde_json::from_slice::<Value>(request_body) {
            Ok(Value::Object(map)) => map,
            _ => return Ok(Err(invalid_request("请求数据验证失败"))),
        };

        let version = root
            .get("version")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| invalid_request("version 为必填字段"));
        let version = match version {
            Ok(value) => value,
            Err(err) => return Ok(Err(err)),
        };
        if version != ADMIN_SYSTEM_DATA_EXPORT_VERSION {
            return Ok(Err(invalid_request(format!(
                "不支持的聚合数据版本: {version}，支持的版本: {ADMIN_SYSTEM_DATA_EXPORT_VERSION}"
            ))));
        }

        let merge_mode = match serde_json::from_value::<AdminImportMergeMode>(
            root.get("merge_mode").cloned().unwrap_or(Value::Null),
        ) {
            Ok(value) => value,
            Err(_) => {
                return Ok(Err(invalid_request(
                    "merge_mode 仅支持 skip / overwrite / error",
                )))
            }
        };

        let config_body =
            match build_admin_system_data_import_part_body(&root, "config_data", merge_mode) {
                Ok(value) => value,
                Err(err) => return Ok(Err(err)),
            };
        let users_body =
            match build_admin_system_data_import_part_body(&root, "user_data", merge_mode) {
                Ok(value) => value,
                Err(err) => return Ok(Err(err)),
            };

        match self
            .prevalidate_admin_system_config_import(&config_body, mode)
            .await?
        {
            Ok(()) => {}
            Err(err) => return Ok(Err(err)),
        }
        match self
            .prevalidate_admin_system_users_import(&users_body, operator_id, mode)
            .await?
        {
            Ok(()) => {}
            Err(err) => return Ok(Err(err)),
        }

        // The config and users repositories are intentionally exposed as independent write
        // handles, so this aggregate operation cannot share a database transaction across all
        // supported drivers. Capture both sides immediately before the first write. Interactive
        // imports use a redacted checkpoint. Recovery restores are authorized to
        // hold a credential-bearing checkpoint briefly in memory; otherwise a failed restore
        // could not put an overwritten secret back. The import lock held by the route serializes
        // other aggregate imports while this checkpoint is being applied.
        let checkpoint_export_mode = if mode == SystemImportMode::RecoveryBackup {
            SystemExportMode::RecoveryBackup
        } else {
            SystemExportMode::RollbackCheckpoint
        };
        let rollback_mode = if mode == SystemImportMode::RecoveryBackup {
            SystemImportMode::RecoveryRollbackCheckpoint
        } else {
            SystemImportMode::RollbackCheckpoint
        };
        let config_checkpoint = self
            .build_admin_system_config_export_payload(checkpoint_export_mode)
            .await?;
        let users_checkpoint = self
            .build_admin_system_users_export_payload(checkpoint_export_mode)
            .await?;

        let mut mutation_journal = AggregateMutationJournal::default();
        let config_result = match self
            .import_admin_system_config_with_mode(&config_body, mode, Some(&mut mutation_journal))
            .await
        {
            Ok(Ok(payload)) => payload,
            Ok(Err(original)) => {
                match self
                    .rollback_aggregate_config(&config_checkpoint, rollback_mode, &mutation_journal)
                    .await
                {
                    Ok(()) => return Ok(Err(original)),
                    Err(rollback_error) => {
                        return Err(aggregate_rollback_http_error(
                            "配置阶段",
                            &original,
                            rollback_error,
                        ));
                    }
                }
            }
            Err(error) => {
                let original_error = error.clone();
                self.rollback_aggregate_config(
                    &config_checkpoint,
                    rollback_mode,
                    &mutation_journal,
                )
                .await
                .map_err(|rollback_error| {
                    aggregate_rollback_error("配置阶段", original_error, rollback_error)
                })?;
                return Err(error);
            }
        };

        let users_result = match self
            .import_admin_system_users_with_mode(
                &users_body,
                operator_id,
                mode,
                Some(&mut mutation_journal),
            )
            .await
        {
            Ok(Ok(payload)) => payload,
            Ok(Err(original)) => {
                match self
                    .rollback_aggregate_import(
                        &config_checkpoint,
                        &users_checkpoint,
                        operator_id,
                        rollback_mode,
                        &mutation_journal,
                    )
                    .await
                {
                    Ok(()) => return Ok(Err(original)),
                    Err(rollback_error) => {
                        return Err(aggregate_rollback_http_error(
                            "用户阶段",
                            &original,
                            rollback_error,
                        ));
                    }
                }
            }
            Err(error) => {
                let original_error = error.clone();
                self.rollback_aggregate_import(
                    &config_checkpoint,
                    &users_checkpoint,
                    operator_id,
                    rollback_mode,
                    &mutation_journal,
                )
                .await
                .map_err(|rollback_error| {
                    aggregate_rollback_error("用户阶段", original_error, rollback_error)
                })?;
                return Err(error);
            }
        };

        Ok(Ok(json!({
            "message": "聚合数据导入成功",
            "config": config_result,
            "users": users_result,
        })))
    }

    /// Restore a config checkpoint using overwrite semantics. A redacted checkpoint preserves
    /// existing encrypted values; a recovery checkpoint carries the original credentials.
    async fn rollback_aggregate_config(
        &self,
        checkpoint: &Value,
        rollback_mode: SystemImportMode,
        mutation_journal: &AggregateMutationJournal,
    ) -> Result<(), GatewayError> {
        let cleanup_result = self.rollback_created_config(mutation_journal).await;
        let body = build_aggregate_rollback_body_with_options(
            checkpoint,
            true,
            cleanup_result.skip_ldap_restore,
        )?;
        let restore_result = match self
            .import_admin_system_config_with_mode(&body, rollback_mode, None)
            .await
        {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(_)) => Err(GatewayError::Internal(
                "aggregate config rollback rejected".to_string(),
            )),
            Err(error) => Err(error),
        };
        combine_rollback_results(cleanup_result.result, restore_result, "config")
    }

    async fn rollback_created_config(
        &self,
        journal: &AggregateMutationJournal,
    ) -> ConfigCleanupOutcome {
        let mut failures = Vec::new();
        let mut skip_ldap_restore = false;

        if let Some(expected) = journal.created_ldap_config.as_ref() {
            match self.delete_ldap_module_config_if_matches(expected).await {
                Ok(true) => {}
                Ok(false) => {
                    // Even if the follow-up read observes no row, another writer can create one
                    // between that read and the checkpoint restore. Skip LDAP restoration for
                    // every non-successful compare/delete result to avoid a TOCTOU overwrite.
                    skip_ldap_restore = true;
                    // A missing row means another cleanup attempt already removed it. If a row
                    // remains, however, it was changed concurrently and must not be deleted by
                    // an owner-blind rollback.
                    match self.get_ldap_module_config().await {
                        Ok(None) => {}
                        Ok(Some(_)) => failures.push(GatewayError::Internal(
                            "LDAP configuration changed during aggregate rollback".to_string(),
                        )),
                        Err(error) => failures.push(error),
                    }
                }
                Err(error) => {
                    skip_ldap_restore = true;
                    failures.push(error);
                }
            }
        }

        for (provider_id, model_id) in &journal.provider_model_ids {
            if let Err(error) = self
                .delete_admin_provider_model(provider_id, model_id)
                .await
            {
                failures.push(error);
            }
        }
        for (_, key_id) in &journal.provider_key_ids {
            if let Err(error) = self.delete_provider_catalog_key(key_id).await {
                failures.push(error);
            }
        }
        for (_, endpoint_id) in &journal.provider_endpoint_ids {
            if let Err(error) = self.delete_provider_catalog_endpoint(endpoint_id).await {
                failures.push(error);
            }
        }
        for provider_id in &journal.provider_ids {
            let endpoint_ids = journal
                .provider_endpoint_ids
                .iter()
                .filter(|(owner_id, _)| owner_id == provider_id)
                .map(|(_, id)| id.clone())
                .collect::<Vec<_>>();
            let key_ids = journal
                .provider_key_ids
                .iter()
                .filter(|(owner_id, _)| owner_id == provider_id)
                .map(|(_, id)| id.clone())
                .collect::<Vec<_>>();
            if let Err(error) = self
                .cleanup_deleted_provider_catalog_refs(provider_id, true, &endpoint_ids, &key_ids)
                .await
            {
                failures.push(error);
            }
            if let Err(error) = self
                .app()
                .delete_provider_catalog_provider(provider_id)
                .await
            {
                failures.push(error);
            }
        }
        for global_model_id in &journal.global_model_ids {
            if let Err(error) = self.delete_admin_global_model(global_model_id).await {
                failures.push(error);
            }
        }
        for provider_type in &journal.oauth_provider_types {
            match self
                .delete_oauth_provider_config_if_unlinked(provider_type)
                .await
            {
                Ok(_) => {}
                Err(error) => failures.push(error),
            }
        }
        for key in &journal.system_config_keys {
            if let Err(error) = self.delete_system_config_value(key).await {
                failures.push(error);
            }
        }

        let result = if failures.is_empty() {
            Ok(())
        } else {
            Err(GatewayError::Internal(format!(
                "aggregate config mutation cleanup failed for {} object(s)",
                failures.len()
            )))
        };
        ConfigCleanupOutcome {
            result,
            skip_ldap_restore,
        }
    }

    async fn rollback_created_users(
        &self,
        journal: &AggregateMutationJournal,
    ) -> Result<(), GatewayError> {
        let mut failures = Vec::new();
        let mut blocked_user_ids = BTreeSet::new();
        let mut blocked_api_key_ids = BTreeSet::new();

        // Remove wallets before their owning API keys/users. The owner predicate is part of the
        // delete operation, so a wallet cannot be detached and accidentally reclaimed by another
        // object between the journal lookup and compensation.
        for ((user_id, wallet_id), expected) in &journal.user_wallet_snapshots {
            match self
                .delete_wallet_if_snapshot_matches_and_unreferenced(
                    expected,
                    WalletLookupKey::UserId(user_id.as_str()),
                )
                .await
            {
                Ok(true) => {}
                Ok(false) => {
                    blocked_user_ids.insert(user_id.clone());
                    failures.push(GatewayError::Internal(format!(
                        "import rollback could not delete wallet {wallet_id} for user {user_id}"
                    )));
                }
                Err(error) => {
                    blocked_user_ids.insert(user_id.clone());
                    failures.push(error);
                }
            }
        }
        for ((api_key_id, wallet_id), expected) in &journal.api_key_wallet_snapshots {
            match self
                .delete_wallet_if_snapshot_matches_and_unreferenced(
                    expected,
                    WalletLookupKey::ApiKeyId(api_key_id.as_str()),
                )
                .await
            {
                Ok(true) => {}
                Ok(false) => {
                    blocked_api_key_ids.insert(api_key_id.clone());
                    if let Some((user_id, _)) = journal
                        .user_api_key_ids
                        .iter()
                        .find(|(_, candidate_api_key_id)| candidate_api_key_id == api_key_id)
                    {
                        // A failed API-key-wallet compensation also blocks
                        // deleting the owning user.  Otherwise the later
                        // user rollback can remove the key while leaving its
                        // funded wallet orphaned.
                        blocked_user_ids.insert(user_id.clone());
                    }
                    failures.push(GatewayError::Internal(format!(
                        "import rollback could not delete wallet {wallet_id} for API key {api_key_id}"
                    )));
                }
                Err(error) => {
                    blocked_api_key_ids.insert(api_key_id.clone());
                    if let Some((user_id, _)) = journal
                        .user_api_key_ids
                        .iter()
                        .find(|(_, candidate_api_key_id)| candidate_api_key_id == api_key_id)
                    {
                        blocked_user_ids.insert(user_id.clone());
                    }
                    failures.push(error);
                }
            }
        }
        for (user_id, api_key_id) in &journal.user_api_key_ids {
            if blocked_user_ids.contains(user_id) || blocked_api_key_ids.contains(api_key_id) {
                continue;
            }
            if let Err(error) = self.delete_user_api_key(user_id, api_key_id).await {
                failures.push(error);
            }
        }
        for api_key_id in &journal.standalone_api_key_ids {
            if blocked_api_key_ids.contains(api_key_id) {
                continue;
            }
            if let Err(error) = self.delete_standalone_api_key(api_key_id).await {
                failures.push(error);
            }
        }
        for user_id in &journal.user_ids {
            if blocked_user_ids.contains(user_id) {
                continue;
            }
            if let Err(error) = self
                .app()
                .rollback_provisional_auth_user_with_wallet(user_id, None)
                .await
            {
                failures.push(error);
            }
        }
        for group_id in &journal.user_group_ids {
            if let Err(error) = self.delete_user_group(group_id).await {
                failures.push(error);
            }
        }

        if failures.is_empty() {
            Ok(())
        } else {
            Err(GatewayError::Internal(format!(
                "aggregate user mutation cleanup failed for {} object(s)",
                failures.len()
            )))
        }
    }

    async fn rollback_existing_wallets(
        &self,
        journal: &AggregateMutationJournal,
    ) -> Result<(), GatewayError> {
        let mut failures = Vec::new();

        for ((user_id, wallet_id), mutation) in &journal.existing_user_wallets {
            let Some(after) = mutation.after.as_ref() else {
                failures.push(GatewayError::Internal(format!(
                    "import rollback has no verified post-state for existing user wallet {wallet_id} ({user_id})"
                )));
                continue;
            };
            match self
                .restore_wallet_if_snapshot_matches(
                    &mutation.before,
                    after,
                    WalletLookupKey::UserId(user_id.as_str()),
                )
                .await
            {
                Ok(true) => {}
                Ok(false) => failures.push(GatewayError::Internal(format!(
                    "import rollback wallet CAS conflict for user {user_id}, wallet {wallet_id}"
                ))),
                Err(error) => failures.push(error),
            }
        }

        for ((api_key_id, wallet_id), mutation) in &journal.existing_api_key_wallets {
            let Some(after) = mutation.after.as_ref() else {
                failures.push(GatewayError::Internal(format!(
                    "import rollback has no verified post-state for existing API-key wallet {wallet_id} ({api_key_id})"
                )));
                continue;
            };
            match self
                .restore_wallet_if_snapshot_matches(
                    &mutation.before,
                    after,
                    WalletLookupKey::ApiKeyId(api_key_id.as_str()),
                )
                .await
            {
                Ok(true) => {}
                Ok(false) => failures.push(GatewayError::Internal(format!(
                    "import rollback wallet CAS conflict for API key {api_key_id}, wallet {wallet_id}"
                ))),
                Err(error) => failures.push(error),
            }
        }

        if failures.is_empty() {
            Ok(())
        } else {
            Err(GatewayError::Internal(format!(
                "aggregate existing wallet rollback failed for {} object(s)",
                failures.len()
            )))
        }
    }

    async fn capture_existing_user_mutation(
        &self,
        journal: &mut AggregateMutationJournal,
        user: &aether_data::repository::users::StoredUserAuthRecord,
    ) -> Result<(), GatewayError> {
        if journal.existing_users.contains_key(&user.id) {
            return Ok(());
        }
        let mut before_export = self.find_export_user_by_id(&user.id).await?;
        let before_model_capability_settings = self
            .app()
            .read_user_model_capability_settings(&user.id)
            .await?;
        let before_feature_settings = before_export
            .as_ref()
            .and_then(|record| record.feature_settings.clone())
            .or(self.app().read_user_feature_settings(&user.id).await?);
        #[cfg(test)]
        if before_export.is_none() {
            before_export = Some(synthetic_rollback_export_row(
                user,
                before_model_capability_settings.clone(),
                before_feature_settings.clone(),
            )?);
        }
        let mut before_group_ids = self
            .list_user_groups_for_user(&user.id)
            .await?
            .into_iter()
            .map(|group| group.id)
            .collect::<Vec<_>>();
        before_group_ids.sort();
        before_group_ids.dedup();
        // A role/active-state update revokes every key owned by the user. Capture those keys before
        // the first user write so a later import failure can restore them through their own CAS
        // path instead of leaving an unrelated key permanently disabled.
        let existing_api_keys = self
            .list_auth_api_key_export_records_by_user_ids(std::slice::from_ref(&user.id))
            .await?;
        for record in existing_api_keys {
            if record.is_standalone || record.user_id != user.id {
                continue;
            }
            let key = (user.id.clone(), record.api_key_id.clone());
            journal
                .existing_user_api_keys
                .entry(key)
                .or_insert_with(|| ExistingApiKeyMutation {
                    before: record.clone(),
                    after: record,
                });
        }
        journal.existing_users.insert(
            user.id.clone(),
            ExistingUserMutation {
                before_auth: user.clone(),
                after_auth: user.clone(),
                before_export: before_export.clone(),
                after_export: before_export,
                before_model_capability_settings: before_model_capability_settings.clone(),
                after_model_capability_settings: before_model_capability_settings,
                before_feature_settings: before_feature_settings.clone(),
                after_feature_settings: before_feature_settings,
                before_group_ids: before_group_ids.clone(),
                after_group_ids: before_group_ids,
            },
        );
        Ok(())
    }

    /// Refresh a journal entry after each successful user mutation. Keeping the latest
    /// post-state lets compensation recover even when a later setter in the same user fails.
    async fn refresh_existing_user_mutation(
        &self,
        mutation_journal: Option<&mut AggregateMutationJournal>,
        user_id: &str,
    ) -> Result<(), GatewayError> {
        let Some(journal) = mutation_journal else {
            return Ok(());
        };
        if !journal.existing_users.contains_key(user_id) {
            return Ok(());
        }
        let Some(auth) = self.find_user_auth_by_id(user_id).await? else {
            return Ok(());
        };
        let security_state_changed = journal.existing_users.get(user_id).is_some_and(|mutation| {
            mutation.after_auth.role != auth.role || mutation.after_auth.is_active != auth.is_active
        });
        let model_capability_settings = self
            .app()
            .read_user_model_capability_settings(user_id)
            .await?;
        let mut export = self.find_export_user_by_id(user_id).await?;
        let feature_settings = export
            .as_ref()
            .and_then(|record| record.feature_settings.clone())
            .or(self.app().read_user_feature_settings(user_id).await?);
        #[cfg(test)]
        if export.is_none() {
            export = Some(synthetic_rollback_export_row(
                &auth,
                model_capability_settings.clone(),
                feature_settings.clone(),
            )?);
        }
        let mut group_ids = self
            .list_user_groups_for_user(user_id)
            .await?
            .into_iter()
            .map(|group| group.id)
            .collect::<Vec<_>>();
        group_ids.sort();
        group_ids.dedup();
        if let Some(mutation) = journal.existing_users.get_mut(user_id) {
            mutation.after_auth = auth;
            mutation.after_export = export;
            mutation.after_model_capability_settings = model_capability_settings;
            mutation.after_feature_settings = feature_settings;
            mutation.after_group_ids = group_ids;
        }
        if security_state_changed {
            // The user CAS revokes all active API keys in the same database transaction. Refresh
            // the post-state of every pre-captured key so the later key CAS can undo that exact
            // revocation while still refusing any key changed by another writer.
            for record in self
                .list_auth_api_key_export_records_by_user_ids(&[user_id.to_string()])
                .await?
            {
                if record.is_standalone || record.user_id != user_id {
                    continue;
                }
                if let Some(key_mutation) = journal
                    .existing_user_api_keys
                    .get_mut(&(user_id.to_string(), record.api_key_id.clone()))
                {
                    key_mutation.after = record;
                }
            }
        }
        Ok(())
    }

    async fn refresh_existing_api_key_mutation(
        &self,
        mutation_journal: Option<&mut AggregateMutationJournal>,
        user_id: Option<&str>,
        api_key_id: &str,
        standalone: bool,
    ) -> Result<(), GatewayError> {
        let Some(journal) = mutation_journal else {
            return Ok(());
        };
        let record = if standalone {
            self.find_auth_api_key_export_standalone_record_by_id(api_key_id)
                .await?
        } else {
            self.list_auth_api_key_export_records_by_ids(&[api_key_id.to_string()])
                .await?
                .into_iter()
                .find(|record| {
                    !record.is_standalone
                        && user_id.is_none_or(|expected| record.user_id == expected)
                })
        };
        let Some(record) = record else {
            return Ok(());
        };
        if standalone {
            if let Some(mutation) = journal.existing_standalone_api_keys.get_mut(api_key_id) {
                mutation.after = record;
            }
        } else if let Some(user_id) = user_id {
            if let Some(mutation) = journal
                .existing_user_api_keys
                .get_mut(&(user_id.to_string(), api_key_id.to_string()))
            {
                mutation.after = record;
            }
        }
        Ok(())
    }

    async fn rollback_existing_user_groups(
        &self,
        journal: &AggregateMutationJournal,
    ) -> Result<(), GatewayError> {
        let mut failures = Vec::new();
        for (group_id, mutation) in &journal.existing_user_groups {
            if mutation.before == mutation.after {
                continue;
            }
            match self
                .restore_user_group_if_matches(&mutation.after, &mutation.before)
                .await
            {
                Ok(true) => {}
                Ok(false) => {
                    tracing::warn!(
                        group_id,
                        "skipping missing or concurrently changed user group rollback"
                    );
                }
                Err(error) => failures.push(error),
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(GatewayError::Internal(format!(
                "existing user group rollback failed for {} object(s)",
                failures.len()
            )))
        }
    }

    async fn rollback_existing_users(
        &self,
        journal: &AggregateMutationJournal,
    ) -> Result<(), GatewayError> {
        let mut failures = Vec::new();
        for (user_id, mutation) in &journal.existing_users {
            let before = &mutation.before_auth;
            let after = &mutation.after_auth;
            // Passwords are intentionally excluded from the aggregate CAS because a nullable
            // password hash has its own compare-and-write operation below.
            let auth_state_changed = !before.matches_restore_state(after);
            let export_state_changed = match (&mutation.before_export, &mutation.after_export) {
                (Some(before_export), Some(after_export)) => {
                    !before_export.matches_restore_state(after_export)
                        || before_export.rate_limit != after_export.rate_limit
                        || before_export.rate_limit_mode != after_export.rate_limit_mode
                }
                (None, None) => false,
                _ => true,
            };
            let model_settings_changed = mutation.before_model_capability_settings
                != mutation.after_model_capability_settings;
            let feature_settings_changed =
                mutation.before_feature_settings != mutation.after_feature_settings;

            if auth_state_changed
                || export_state_changed
                || model_settings_changed
                || feature_settings_changed
            {
                let restore_result = match (&mutation.after_export, &mutation.before_export) {
                    (Some(expected_export), Some(restored_export)) => {
                        self.restore_local_auth_user_state_if_matches(
                            after,
                            before,
                            expected_export,
                            restored_export,
                            mutation.after_model_capability_settings.as_ref(),
                            mutation.before_model_capability_settings.clone(),
                            mutation.after_feature_settings.as_ref(),
                            mutation.before_feature_settings.clone(),
                        )
                        .await
                    }
                    _ => {
                        tracing::warn!(
                            user_id,
                            "skipping user state rollback because export snapshot is unavailable"
                        );
                        Ok(false)
                    }
                };
                match restore_result {
                    Ok(true) => {}
                    Ok(false) => {
                        tracing::warn!(
                            user_id,
                            "skipping concurrently changed user state rollback"
                        );
                    }
                    Err(error) => failures.push(error),
                }
            }

            if before.password_hash != after.password_hash {
                match self
                    .restore_local_auth_user_password_hash_if_matches(
                        user_id,
                        after.password_hash.as_deref(),
                        before.password_hash.clone(),
                        chrono::Utc::now(),
                    )
                    .await
                {
                    Ok(true) => {}
                    Ok(false) => {
                        tracing::warn!(user_id, "skipping concurrently changed user password");
                    }
                    Err(error) => failures.push(error),
                }
            }
            if mutation.before_group_ids != mutation.after_group_ids {
                match self
                    .restore_user_groups_if_matches(
                        user_id,
                        &mutation.after_group_ids,
                        &mutation.before_group_ids,
                    )
                    .await
                {
                    Ok(true) => {}
                    Ok(false) => {
                        tracing::warn!(
                            user_id,
                            "skipping concurrently changed user groups rollback"
                        );
                    }
                    Err(error) => failures.push(error),
                }
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(GatewayError::Internal(format!(
                "existing user rollback failed for {} object(s)",
                failures.len()
            )))
        }
    }

    async fn rollback_existing_api_keys(
        &self,
        journal: &AggregateMutationJournal,
    ) -> Result<(), GatewayError> {
        let mut failures = Vec::new();
        for ((user_id, api_key_id), mutation) in &journal.existing_user_api_keys {
            self.rollback_one_existing_api_key(
                Some(user_id),
                api_key_id,
                false,
                mutation,
                &mut failures,
            )
            .await?;
        }
        for (api_key_id, mutation) in &journal.existing_standalone_api_keys {
            self.rollback_one_existing_api_key(None, api_key_id, true, mutation, &mut failures)
                .await?;
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(GatewayError::Internal(format!(
                "existing API key rollback failed for {} object(s)",
                failures.len()
            )))
        }
    }

    async fn rollback_one_existing_api_key(
        &self,
        user_id: Option<&str>,
        api_key_id: &str,
        standalone: bool,
        mutation: &ExistingApiKeyMutation,
        failures: &mut Vec<GatewayError>,
    ) -> Result<(), GatewayError> {
        let _ = (user_id, standalone);
        if mutation.before == mutation.after {
            return Ok(());
        }

        match self
            .restore_api_key_if_matches(&mutation.after, &mutation.before)
            .await
        {
            Ok(true) => {}
            Ok(false) => {
                tracing::warn!(
                    api_key_id,
                    "skipping API key rollback after deletion, identity, or concurrent-state conflict"
                );
            }
            Err(error) => failures.push(error),
        }
        Ok(())
    }

    /// Compensate only objects touched by this import, then restore config. Replaying the whole
    /// user checkpoint is intentionally avoided because it could overwrite a concurrent admin
    /// change made after the import wrote a row.
    async fn rollback_aggregate_import(
        &self,
        config_checkpoint: &Value,
        _users_checkpoint: &Value,
        _operator_id: Option<&str>,
        rollback_mode: SystemImportMode,
        mutation_journal: &AggregateMutationJournal,
    ) -> Result<(), GatewayError> {
        let cleanup_result = self.rollback_created_users(mutation_journal).await;
        let existing_wallet_result = self.rollback_existing_wallets(mutation_journal).await;
        let existing_users_result = self.rollback_existing_users(mutation_journal).await;
        let existing_groups_result = self.rollback_existing_user_groups(mutation_journal).await;
        let existing_api_keys_result = self.rollback_existing_api_keys(mutation_journal).await;
        let config_result = self
            .rollback_aggregate_config(config_checkpoint, rollback_mode, mutation_journal)
            .await;
        let restore_result = combine_rollback_results(
            existing_users_result,
            existing_groups_result,
            "aggregate existing users/groups",
        );
        let restore_result = combine_rollback_results(
            existing_api_keys_result,
            restore_result,
            "aggregate existing API keys",
        );
        let restore_result = combine_rollback_results(restore_result, config_result, "aggregate");
        let restore_result =
            combine_rollback_results(existing_wallet_result, restore_result, "aggregate wallets");
        combine_rollback_results(cleanup_result, restore_result, "aggregate")
    }

    pub(crate) async fn import_admin_system_config(
        &self,
        request_body: &Bytes,
    ) -> Result<Result<Value, (http::StatusCode, Value)>, GatewayError> {
        self.import_admin_system_config_with_mode(
            request_body,
            SystemImportMode::InteractiveUpload,
            None,
        )
        .await
    }

    pub(crate) async fn restore_admin_system_config_backup(
        &self,
        request_body: &Bytes,
        _authority: crate::backup::executor::BackupRestoreAuthority,
    ) -> Result<Result<Value, (http::StatusCode, Value)>, GatewayError> {
        self.import_admin_system_config_with_mode(
            request_body,
            SystemImportMode::RecoveryBackup,
            None,
        )
        .await
    }

    async fn import_admin_system_config_with_mode(
        &self,
        request_body: &Bytes,
        mode: SystemImportMode,
        mut mutation_journal: Option<&mut AggregateMutationJournal>,
    ) -> Result<Result<Value, (http::StatusCode, Value)>, GatewayError> {
        macro_rules! invalid {
            ($expr:expr) => {
                match $expr {
                    Ok(value) => value,
                    Err(detail) => return Ok(Err(invalid_request(detail))),
                }
            };
        }
        macro_rules! routed {
            ($expr:expr) => {
                match $expr {
                    Ok(value) => value,
                    Err(err) => return Ok(Err(err)),
                }
            };
        }

        if !self.has_global_model_data_reader()
            || !self.has_global_model_data_writer()
            || !self.has_provider_catalog_data_reader()
            || !self.has_provider_catalog_data_writer()
        {
            return Ok(Err((
                http::StatusCode::SERVICE_UNAVAILABLE,
                json!({ "detail": "Admin system data unavailable" }),
            )));
        }
        match self
            .prevalidate_admin_system_config_import(request_body, mode)
            .await?
        {
            Ok(()) => {}
            Err(err) => return Ok(Err(err)),
        }
        let parsed = routed!(parse_admin_system_config_import_request(request_body));
        let source_version = parsed.request.document.version.clone();
        let root = parsed.root;
        let credentials_not_exported = invalid!(imported_config_credentials_not_exported(&root));
        let merge_mode = parsed.request.merge_mode;

        let imported_global_models = routed!(
            parse_admin_system_config_array::<ImportedGlobalModel>(&root, "global_models")
        );
        let imported_providers = routed!(parse_admin_system_config_array::<ImportedProvider>(
            &root,
            "providers"
        ));
        let imported_proxy_nodes = routed!(parse_admin_system_config_array::<ImportedProxyNode>(
            &root,
            "proxy_nodes"
        ));
        let imported_ldap = routed!(parse_admin_system_config_optional_object::<
            ImportedLdapConfig,
        >(&root, "ldap_config"));
        let imported_oauth_providers = routed!(parse_admin_system_config_array::<
            ImportedOAuthProvider,
        >(&root, "oauth_providers",));
        let imported_system_configs = routed!(parse_admin_system_config_array::<
            ImportedSystemConfig,
        >(&root, "system_configs",));

        let mut stats = AdminSystemConfigImportStats::default();

        // Proxy nodes are deployment-local resources and are intentionally not imported by the
        // Rust admin backend. Apply the external catalog selector before importing any other
        // object, and turn a non-empty exported node reference into direct mode. This keeps a
        // clean-environment restore portable and prevents a late selector validation failure from
        // leaving the rest of the document partially imported.
        let (imported_external_models_configs, mut imported_system_configs): (Vec<_>, Vec<_>) =
            imported_system_configs.into_iter().partition(|item| {
                normalize_imported_system_config_key(&item.value.key)
                    == ADMIN_EXTERNAL_MODELS_PROXY_NODE_CONFIG_KEY
            });
        // Destination-bound secrets must be applied after their destination fields.  Recovery
        // documents are user-controlled JSON and do not guarantee any ordering.
        imported_system_configs.sort_by_key(|item| {
            matches!(
                normalize_imported_system_config_key(&item.value.key).as_str(),
                "smtp_password" | "module.bark_push.device_key"
            )
        });
        let mut existing_system_config_keys = self
            .list_system_config_entries()
            .await?
            .into_iter()
            .map(|entry| normalize_imported_system_config_key(&entry.key))
            .collect::<BTreeSet<_>>();
        for imported_config_item in imported_external_models_configs {
            let (_, system_config) = imported_config_item.into_parts();
            let exists =
                existing_system_config_keys.contains(ADMIN_EXTERNAL_MODELS_PROXY_NODE_CONFIG_KEY);
            match (exists, merge_mode) {
                (true, AdminImportMergeMode::Skip) => {
                    stats.system_configs.skipped += 1;
                    continue;
                }
                (true, AdminImportMergeMode::Error) => {
                    return Ok(Err(invalid_request(format!(
                        "SystemConfig '{ADMIN_EXTERNAL_MODELS_PROXY_NODE_CONFIG_KEY}' 已存在"
                    ))));
                }
                _ => {}
            }

            let imported_proxy_node_id = match system_config.value {
                Value::Null => None,
                Value::String(value) => {
                    let value = value.trim();
                    if value.is_empty() {
                        return Ok(Err(invalid_request(
                            "external_models_proxy_node_id 不能为空",
                        )));
                    }
                    Some(value.to_string())
                }
                _ => {
                    return Ok(Err(invalid_request(
                        "external_models_proxy_node_id 必须是字符串或 null",
                    )))
                }
            };
            // A portable import must not retain a deployment-local node reference. Rollback is
            // different: it runs on the same deployment and should restore the selector when
            // that node still exists, so a failed aggregate operation does not silently switch
            // the catalog to direct mode.
            let selector = if mode.is_rollback_checkpoint() {
                match imported_proxy_node_id.as_deref() {
                    Some(node_id) if self.find_proxy_node(node_id).await?.is_some() => {
                        Some(node_id)
                    }
                    _ => None,
                }
            } else {
                None
            };
            let request_bytes = Bytes::from(
                serde_json::to_vec(&json!({ "proxy_node_id": selector }))
                    .map_err(|err| GatewayError::Internal(err.to_string()))?,
            );
            match self
                .apply_admin_external_models_config_update(&request_bytes)
                .await?
            {
                Ok(_) => {
                    if exists {
                        stats.system_configs.updated += 1;
                    } else {
                        stats.system_configs.created += 1;
                        existing_system_config_keys
                            .insert(ADMIN_EXTERNAL_MODELS_PROXY_NODE_CONFIG_KEY.to_string());
                        if let Some(journal) = mutation_journal.as_deref_mut() {
                            journal
                                .system_config_keys
                                .insert(ADMIN_EXTERNAL_MODELS_PROXY_NODE_CONFIG_KEY.to_string());
                        }
                    }
                    if !mode.is_rollback_checkpoint() {
                        if let Some(node_id) = imported_proxy_node_id {
                            stats.errors.push(format!(
                            "外部模型目录代理节点 '{node_id}' 是当前部署的本地引用；代理节点未导入，已切换为直连"
                        ));
                        }
                    }
                }
                Err((status, payload)) => return Ok(Err((status, payload))),
            }
        }

        let mut global_models_by_name = self
            .list_all_admin_global_models_for_system_transfer()
            .await?
            .into_iter()
            .map(|model| (model.name.clone(), model))
            .collect::<BTreeMap<_, _>>();

        if !imported_proxy_nodes.is_empty() {
            let empty_proxy_node_ids = imported_proxy_nodes
                .iter()
                .filter(|node| {
                    node.value
                        .id
                        .as_deref()
                        .map(str::trim)
                        .is_none_or(|value| value.is_empty())
                })
                .count();
            stats.proxy_nodes.skipped = imported_proxy_nodes.len() as u64;
            if empty_proxy_node_ids > 0 {
                stats.errors.push(format!(
                    "检测到 {empty_proxy_node_ids} 个无效 proxy_nodes 项；当前 Rust 管理后端暂不支持导入代理节点"
                ));
            } else {
                stats.errors.push(
                    "当前 Rust 管理后端暂不支持导入代理节点；仅引用这些节点(node_id)的自动连接代理配置会被清除，手动 URL 代理配置会保留"
                        .to_string(),
                );
            }
        }
        let node_id_map = BTreeMap::<String, String>::new();

        for imported_model in imported_global_models {
            let (_, model) = imported_model.into_parts();
            let name = invalid!(trim_required(&model.name, "name"));
            let display_name = invalid!(trim_required(&model.display_name, "display_name"));
            let default_price_per_request = invalid!(normalize_optional_price(
                model.default_price_per_request,
                "default_price_per_request",
            ));
            let existing_model = global_models_by_name.get(&name);
            let default_tiered_pricing = invalid!(normalize_json_object(
                prepare_imported_secret_safe_json(
                    existing_model.and_then(|model| model.default_tiered_pricing.as_ref()),
                    model.default_tiered_pricing,
                    credentials_not_exported,
                ),
                "default_tiered_pricing",
            ));
            let supported_capabilities =
                normalize_supported_capabilities(model.supported_capabilities);
            let config = invalid!(normalize_json_object(
                prepare_imported_secret_safe_json(
                    existing_model.and_then(|model| model.config.as_ref()),
                    model.config,
                    credentials_not_exported,
                ),
                "config",
            ));

            if let Some(existing) = global_models_by_name.get(&name).cloned() {
                match merge_mode {
                    AdminImportMergeMode::Skip => {
                        stats.global_models.skipped += 1;
                    }
                    AdminImportMergeMode::Error => {
                        return Ok(Err(invalid_request(format!("GlobalModel '{name}' 已存在"))));
                    }
                    AdminImportMergeMode::Overwrite => {
                        let mut record = invalid!(UpdateAdminGlobalModelRecord::new(
                            existing.id.clone(),
                            display_name,
                            model.is_active,
                            default_price_per_request,
                            default_tiered_pricing,
                            supported_capabilities,
                            config,
                        )
                        .map_err(|err| err.to_string()));
                        record.usage_count = model.usage_count;
                        let Some(updated) = self.update_admin_global_model(&record).await? else {
                            return Ok(Err(invalid_request(format!(
                                "更新 GlobalModel '{name}' 失败"
                            ))));
                        };
                        global_models_by_name.insert(name, updated);
                        stats.global_models.updated += 1;
                    }
                }
                continue;
            }

            let mut record = invalid!(CreateAdminGlobalModelRecord::new(
                Uuid::new_v4().to_string(),
                name.clone(),
                display_name,
                model.is_active,
                default_price_per_request,
                default_tiered_pricing,
                supported_capabilities,
                config,
            )
            .map_err(|err| err.to_string()));
            record.usage_count = model.usage_count;
            let Some(created) = self.create_admin_global_model(&record).await? else {
                return Ok(Err(invalid_request(format!(
                    "创建 GlobalModel '{name}' 失败"
                ))));
            };
            let created_global_model_id = created.id.clone();
            global_models_by_name.insert(name, created);
            stats.global_models.created += 1;
            if let Some(journal) = mutation_journal.as_deref_mut() {
                journal.global_model_ids.insert(created_global_model_id);
            }
        }

        let mut providers_by_name = self
            .list_provider_catalog_providers(false)
            .await?
            .into_iter()
            .map(|provider| (provider.name.clone(), provider))
            .collect::<BTreeMap<_, _>>();

        for imported_provider_item in imported_providers {
            let (raw_provider, imported_provider) = imported_provider_item.into_parts();
            let provider_name = invalid!(trim_required(&imported_provider.name, "name"));
            invalid!(
                crate::provider_transport::validate_anthropic_compatibility_profile_config(
                    imported_provider.config.as_ref(),
                )
                .map_err(|_| "无效的 Anthropic compatibility profile".to_string())
            );
            let existing_provider = providers_by_name.get(&provider_name).cloned();

            let provider = if let Some(existing) = existing_provider {
                match merge_mode {
                    AdminImportMergeMode::Skip => {
                        stats.providers.skipped += 1;
                        existing
                    }
                    AdminImportMergeMode::Error => {
                        return Ok(Err(invalid_request(format!(
                            "Provider '{provider_name}' 已存在"
                        ))));
                    }
                    AdminImportMergeMode::Overwrite => {
                        let patch =
                            match AdminProviderUpdatePatch::from_object(raw_provider.clone()) {
                                Ok(patch) => patch,
                                Err(_) => {
                                    return Ok(Err(invalid_request(format!(
                                        "Provider '{provider_name}' 配置格式无效"
                                    ))));
                                }
                            };
                        let mut updated = invalid!(
                            self.build_admin_update_provider_record(&existing, patch)
                                .await
                        );
                        updated.proxy = prepare_imported_secret_safe_proxy(
                            existing.proxy.as_ref(),
                            imported_provider.proxy.clone(),
                            credentials_not_exported,
                            &node_id_map,
                        );
                        let provider_ops_fallback_base_url =
                            imported_provider_ops_fallback_base_url(&raw_provider);
                        updated.config = invalid!(prepare_imported_provider_config(
                            self,
                            &updated.id,
                            provider_ops_fallback_base_url.as_deref(),
                            existing.config.as_ref(),
                            imported_provider.config.clone(),
                            credentials_not_exported,
                        ));
                        let Some(persisted) =
                            self.update_provider_catalog_provider(&updated).await?
                        else {
                            return Ok(Err(invalid_request(format!(
                                "更新 Provider '{provider_name}' 失败"
                            ))));
                        };
                        providers_by_name.insert(provider_name.clone(), persisted.clone());
                        stats.providers.updated += 1;
                        persisted
                    }
                }
            } else {
                let payload = match serde_json::from_value::<AdminProviderCreateRequest>(
                    Value::Object(raw_provider.clone()),
                ) {
                    Ok(payload) => payload,
                    Err(_) => {
                        return Ok(Err(invalid_request(format!(
                            "Provider '{provider_name}' 配置格式无效"
                        ))));
                    }
                };
                let (mut record, shift_existing_priorities_from) =
                    invalid!(self.build_admin_create_provider_record(payload).await);
                if let Some(enable_format_conversion) = imported_provider.enable_format_conversion {
                    record.enable_format_conversion = enable_format_conversion;
                }
                record.proxy = prepare_imported_secret_safe_proxy(
                    None,
                    imported_provider.proxy.clone(),
                    credentials_not_exported,
                    &node_id_map,
                );
                let provider_ops_fallback_base_url =
                    imported_provider_ops_fallback_base_url(&raw_provider);
                record.config = invalid!(prepare_imported_provider_config(
                    self,
                    &record.id,
                    provider_ops_fallback_base_url.as_deref(),
                    None,
                    imported_provider.config.clone(),
                    credentials_not_exported,
                ));
                let Some(created) = self
                    .create_provider_catalog_provider(&record, shift_existing_priorities_from)
                    .await?
                else {
                    return Ok(Err(invalid_request(format!(
                        "创建 Provider '{provider_name}' 失败"
                    ))));
                };
                providers_by_name.insert(provider_name.clone(), created.clone());
                stats.providers.created += 1;
                if let Some(journal) = mutation_journal.as_deref_mut() {
                    journal.provider_ids.insert(created.id.clone());
                }
                created
            };

            let imported_endpoints = routed!(parse_admin_system_config_nested_array::<
                ImportedEndpoint,
            >(&raw_provider, "endpoints"));
            let mut existing_endpoints_by_format = self
                .list_provider_catalog_endpoints_by_provider_ids(std::slice::from_ref(&provider.id))
                .await?
                .into_iter()
                .map(|endpoint| (endpoint.api_format.clone(), endpoint))
                .collect::<BTreeMap<_, _>>();

            for imported_endpoint_item in imported_endpoints {
                let (raw_endpoint, imported_endpoint) = imported_endpoint_item.into_parts();
                let normalized_api_format = invalid!(normalize_import_endpoint_format(
                    &imported_endpoint.api_format
                ));
                invalid!(
                    crate::provider_transport::validate_anthropic_compatibility_profile_config(
                        imported_endpoint.config.as_ref(),
                    )
                    .map_err(|_| "无效的 Anthropic compatibility profile".to_string())
                );
                if !fixed_provider_import_endpoint_supported(
                    &provider.provider_type,
                    &normalized_api_format,
                ) {
                    let retired = existing_endpoints_by_format.remove(&normalized_api_format);
                    if let Some(mut retired) = retired {
                        if retired.is_active {
                            retired.is_active = false;
                            retired.updated_at_unix_secs = SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .ok()
                                .map(|duration| duration.as_secs());
                            let Some(_) = self.update_provider_catalog_endpoint(&retired).await?
                            else {
                                return Ok(Err(invalid_request(format!(
                                    "停用 Provider '{provider_name}' 的已移除 Endpoint '{normalized_api_format}' 失败"
                                ))));
                            };
                            stats.endpoints.updated += 1;
                        } else {
                            stats.endpoints.skipped += 1;
                        }
                    } else {
                        stats.endpoints.skipped += 1;
                    }
                    stats.errors.push(format!(
                        "固定 Provider '{provider_name}' 不再支持 Endpoint '{normalized_api_format}'，已跳过或停用"
                    ));
                    continue;
                }
                let existing_endpoint = existing_endpoints_by_format
                    .get(&normalized_api_format)
                    .cloned();

                if let Some(existing_endpoint) = existing_endpoint {
                    match merge_mode {
                        AdminImportMergeMode::Skip => {
                            stats.endpoints.skipped += 1;
                        }
                        AdminImportMergeMode::Error => {
                            return Ok(Err(invalid_request(format!(
                                "Endpoint '{normalized_api_format}' 已存在于 Provider '{provider_name}'"
                            ))));
                        }
                        AdminImportMergeMode::Overwrite => {
                            let Some((normalized_signature, api_family, endpoint_kind)) =
                                admin_endpoint_signature_parts(&normalized_api_format)
                            else {
                                return Ok(Err(invalid_request(format!(
                                    "无效的 api_format: {}",
                                    imported_endpoint.api_format
                                ))));
                            };
                            let patch = match AdminProviderEndpointUpdatePatch::from_object(
                                raw_endpoint.clone(),
                            ) {
                                Ok(patch) => patch,
                                Err(_) => {
                                    return Ok(Err(invalid_request(
                                        "Provider Endpoint 配置格式无效",
                                    )));
                                }
                            };
                            let (fields, payload) = patch.into_parts();
                            let normalized_base_url = match payload.base_url.as_deref() {
                                Some(base_url) => {
                                    Some(invalid!(normalize_admin_base_url(base_url)))
                                }
                                None => None,
                            };
                            let update_fields =
                                admin_provider_endpoints_pure::AdminProviderEndpointUpdateFields {
                                    base_url: normalized_base_url,
                                    custom_path: payload.custom_path,
                                    header_rules: prepare_imported_secret_safe_header_rules(
                                        existing_endpoint.header_rules.as_ref(),
                                        payload.header_rules,
                                        credentials_not_exported,
                                    ),
                                    body_rules: prepare_imported_secret_safe_body_rules(
                                        existing_endpoint.body_rules.as_ref(),
                                        payload.body_rules,
                                        credentials_not_exported,
                                    ),
                                    max_retries: payload.max_retries,
                                    is_active: payload.is_active,
                                    config: prepare_imported_secret_safe_json(
                                        existing_endpoint.config.as_ref(),
                                        payload.config,
                                        credentials_not_exported,
                                    ),
                                    proxy: payload.proxy,
                                    format_acceptance_config: prepare_imported_secret_safe_json(
                                        existing_endpoint.format_acceptance_config.as_ref(),
                                        payload.format_acceptance_config,
                                        credentials_not_exported,
                                    ),
                                };
                            let mut updated = invalid!(
                                admin_provider_endpoints_pure::apply_admin_provider_endpoint_update_fields(
                                    &existing_endpoint,
                                    |field| fields.contains(field),
                                    |field| fields.is_null(field),
                                    &update_fields,
                                )
                            );
                            if fields.contains("proxy") {
                                updated.proxy = prepare_imported_secret_safe_proxy(
                                    existing_endpoint.proxy.as_ref(),
                                    imported_endpoint.proxy.clone(),
                                    credentials_not_exported,
                                    &node_id_map,
                                );
                            }
                            updated.api_format = normalized_signature.to_string();
                            updated.api_family = Some(api_family.to_string());
                            updated.endpoint_kind = Some(endpoint_kind.to_string());
                            updated.updated_at_unix_secs = SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .ok()
                                .map(|duration| duration.as_secs());
                            let Some(persisted) =
                                self.update_provider_catalog_endpoint(&updated).await?
                            else {
                                return Ok(Err(invalid_request(format!(
                                    "更新 Endpoint '{normalized_api_format}' 失败"
                                ))));
                            };
                            existing_endpoints_by_format
                                .insert(normalized_api_format.clone(), persisted);
                            stats.endpoints.updated += 1;
                        }
                    }
                    continue;
                }

                let Some((normalized_signature, api_family, endpoint_kind)) =
                    admin_endpoint_signature_parts(&normalized_api_format)
                else {
                    return Ok(Err(invalid_request(format!(
                        "无效的 api_format: {}",
                        imported_endpoint.api_format
                    ))));
                };
                let now_unix_secs = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .ok()
                    .map(|duration| duration.as_secs())
                    .unwrap_or(0);
                let mut record = invalid!(
                    admin_provider_endpoints_pure::build_admin_provider_endpoint_record(
                        Uuid::new_v4().to_string(),
                        provider.id.clone(),
                        normalized_signature.to_string(),
                        api_family.to_string(),
                        endpoint_kind.to_string(),
                        invalid!(normalize_admin_base_url(&imported_endpoint.base_url)),
                        imported_endpoint.custom_path.clone(),
                        prepare_imported_secret_safe_header_rules(
                            None,
                            imported_endpoint.header_rules.clone(),
                            credentials_not_exported,
                        ),
                        prepare_imported_secret_safe_body_rules(
                            None,
                            imported_endpoint.body_rules.clone(),
                            credentials_not_exported,
                        ),
                        imported_endpoint.max_retries.unwrap_or(2),
                        prepare_imported_secret_safe_json(
                            None,
                            imported_endpoint.config.clone(),
                            credentials_not_exported,
                        ),
                        prepare_imported_secret_safe_proxy(
                            None,
                            imported_endpoint.proxy.clone(),
                            credentials_not_exported,
                            &node_id_map,
                        ),
                        prepare_imported_secret_safe_json(
                            None,
                            imported_endpoint.format_acceptance_config.clone(),
                            credentials_not_exported,
                        ),
                        now_unix_secs,
                    )
                );
                record = record.with_health_score(1.0);
                record.is_active = imported_endpoint.is_active;
                let Some(created) = self.create_provider_catalog_endpoint(&record).await? else {
                    return Ok(Err(invalid_request(format!(
                        "创建 Endpoint '{normalized_api_format}' 失败"
                    ))));
                };
                let created_endpoint_id = created.id.clone();
                existing_endpoints_by_format.insert(normalized_api_format.clone(), created);
                stats.endpoints.created += 1;
                if let Some(journal) = mutation_journal.as_deref_mut() {
                    journal
                        .provider_endpoint_ids
                        .insert((provider.id.clone(), created_endpoint_id));
                }
            }

            let provider_endpoint_formats = existing_endpoints_by_format
                .keys()
                .cloned()
                .collect::<BTreeSet<_>>();
            let now_unix_secs = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .ok()
                .map(|duration| duration.as_secs())
                .unwrap_or(0);

            let imported_keys = routed!(parse_admin_system_config_nested_array::<
                ImportedProviderKey,
            >(&raw_provider, "api_keys"));
            let mut existing_keys = self
                .list_provider_catalog_keys_by_provider_ids(std::slice::from_ref(&provider.id))
                .await?;

            for imported_key_item in imported_keys {
                let (raw_key, imported_key) = imported_key_item.into_parts();
                let (normalized_api_formats, missing_formats) =
                    normalize_import_key_formats(&imported_key, &provider_endpoint_formats);
                if !missing_formats.is_empty() {
                    stats.errors.push(format!(
                        "Key (Provider: {provider_name}) 的 api_formats 未配置对应 Endpoint，已跳过: {:?}",
                        missing_formats
                    ));
                }
                if normalized_api_formats.is_empty() {
                    stats.keys.skipped += 1;
                    continue;
                }

                let normalized_auth_config = invalid!(normalize_import_auth_config(
                    imported_key.auth_config.clone()
                ));
                let auth_type = imported_key_auth_type(&imported_key);
                let credentials_not_exported = invalid!(
                    validate_imported_provider_key_credential_state(&imported_key)
                );
                let normalized_raw_key = normalize_import_key_raw_payload(
                    &raw_key,
                    &auth_type,
                    &normalized_api_formats,
                    normalized_auth_config.clone(),
                    credentials_not_exported,
                );
                let existing_key_index = if credentials_not_exported {
                    build_import_key_match_name(&imported_key).and_then(|target_name| {
                        existing_keys.iter().position(|existing_key| {
                            existing_key
                                .auth_type
                                .trim()
                                .eq_ignore_ascii_case(&auth_type)
                                && existing_key.name == target_name
                        })
                    })
                } else {
                    invalid!(find_imported_provider_key_index(
                        self,
                        &imported_key,
                        &auth_type,
                        normalized_auth_config.as_ref(),
                        &existing_keys,
                    ))
                };

                if credentials_not_exported && existing_key_index.is_none() {
                    stats.keys.skipped += 1;
                    continue;
                }

                if let Some(existing_index) = existing_key_index {
                    let existing_key = existing_keys[existing_index].clone();
                    let previous_codex_credential_generation = existing_key
                        .upstream_metadata
                        .as_ref()
                        .and_then(Value::as_object)
                        .and_then(|metadata| metadata.get("codex"))
                        .and_then(|codex| {
                            aether_admin::provider::quota::codex_credential_generation(Some(codex))
                        })
                        .map(ToOwned::to_owned);
                    match merge_mode {
                        AdminImportMergeMode::Skip => {
                            stats.keys.skipped += 1;
                        }
                        AdminImportMergeMode::Error => {
                            return Ok(Err(invalid_request(format!(
                                "Provider '{provider_name}' 中存在重复 Key"
                            ))));
                        }
                        AdminImportMergeMode::Overwrite => {
                            let patch = match AdminProviderKeyUpdatePatch::from_object(
                                normalized_raw_key.clone(),
                            ) {
                                Ok(patch) => patch,
                                Err(_) => {
                                    return Ok(Err(invalid_request("Provider Key 配置格式无效")));
                                }
                            };
                            let mut updated = invalid!(
                                self.build_admin_update_provider_key_record(
                                    &provider,
                                    &existing_key,
                                    patch,
                                )
                                .await
                            );
                            if credentials_not_exported {
                                updated.encrypted_api_key = existing_key.encrypted_api_key.clone();
                                updated.encrypted_auth_config =
                                    existing_key.encrypted_auth_config.clone();
                            }
                            let oauth_credentials_supplied = if auth_type == "oauth" {
                                invalid!(apply_imported_oauth_key_credentials(
                                    self,
                                    &provider.provider_type,
                                    previous_codex_credential_generation.as_deref(),
                                    &raw_key,
                                    normalized_auth_config.as_ref(),
                                    &mut updated,
                                ))
                            } else {
                                false
                            };
                            updated.proxy = prepare_imported_secret_safe_proxy(
                                existing_key.proxy.as_ref(),
                                imported_key.proxy.clone(),
                                credentials_not_exported,
                                &node_id_map,
                            );
                            updated.fingerprint = invalid!(normalize_json_object(
                                prepare_imported_secret_safe_json(
                                    existing_key.fingerprint.as_ref(),
                                    imported_key.fingerprint.clone(),
                                    credentials_not_exported,
                                ),
                                "fingerprint",
                            ));
                            let admin_update = build_provider_catalog_key_admin_cas_update(
                                &existing_key,
                                updated.clone(),
                                &provider.provider_type,
                            );
                            if !self
                                .compare_and_update_provider_catalog_key_admin_state(&admin_update)
                                .await?
                            {
                                return Ok(Err((
                                    http::StatusCode::CONFLICT,
                                    json!({
                                        "detail": format!(
                                            "Provider '{provider_name}' 的 Key 已被其他请求更新，请重试"
                                        )
                                    }),
                                )));
                            }
                            let Some(mut persisted) = self
                                .read_provider_catalog_keys_by_ids(std::slice::from_ref(
                                    &updated.id,
                                ))
                                .await?
                                .into_iter()
                                .next()
                            else {
                                return Ok(Err(invalid_request(format!(
                                    "更新 Provider '{provider_name}' 的 Key 失败"
                                ))));
                            };
                            if updated.learned_rpm_limit != existing_key.learned_rpm_limit {
                                let Some(reloaded) = self
                                    .set_provider_catalog_key_learned_rpm_limit(
                                        &updated.id,
                                        updated.learned_rpm_limit,
                                        updated.updated_at_unix_secs,
                                    )
                                    .await?
                                else {
                                    return Ok(Err(invalid_request(format!(
                                        "更新 Provider '{provider_name}' 的 Key 失败"
                                    ))));
                                };
                                persisted = reloaded;
                            }
                            if oauth_credentials_supplied {
                                let Some(reloaded) = self
                                    .reset_provider_catalog_key_recovery_state_fenced(
                                        &updated.id,
                                        updated.encrypted_auth_config.as_deref().ok_or_else(|| {
                                            GatewayError::Internal(format!(
                                                "OAuth Provider '{provider_name}' imported without auth_config"
                                            ))
                                        })?,
                                    )
                                    .await?
                                else {
                                    return Ok(Err(invalid_request(format!(
                                        "更新 Provider '{provider_name}' 的 Key 失败"
                                    ))));
                                };
                                persisted = reloaded;
                                let _ = self
                                    .app()
                                    .invalidate_local_oauth_refresh_entry(&updated.id)
                                    .await;
                                seed_imported_oauth_pool_score(
                                    self,
                                    &provider.id,
                                    &persisted,
                                    now_unix_secs,
                                )
                                .await?;
                            }
                            existing_keys[existing_index] = persisted;
                            stats.keys.updated += 1;
                        }
                    }
                    continue;
                }

                let payload = match serde_json::from_value::<AdminProviderKeyCreateRequest>(
                    Value::Object(normalized_raw_key.clone()),
                ) {
                    Ok(payload) => payload,
                    Err(_) => return Ok(Err(invalid_request("Provider Key 配置格式无效"))),
                };
                let mut record = invalid!(
                    self.build_admin_create_provider_key_record(&provider, payload)
                        .await
                );
                let oauth_credentials_supplied = if auth_type == "oauth" {
                    invalid!(apply_imported_oauth_key_credentials(
                        self,
                        &provider.provider_type,
                        None,
                        &raw_key,
                        normalized_auth_config.as_ref(),
                        &mut record,
                    ))
                } else {
                    false
                };
                record.is_active = imported_key.is_active;
                record.global_priority_by_format = invalid!(normalize_json_object(
                    imported_key.global_priority_by_format.clone(),
                    "global_priority_by_format",
                ));
                record.proxy = prepare_imported_secret_safe_proxy(
                    None,
                    imported_key.proxy.clone(),
                    credentials_not_exported,
                    &node_id_map,
                );
                record.fingerprint = invalid!(normalize_json_object(
                    prepare_imported_secret_safe_json(
                        None,
                        imported_key.fingerprint.clone(),
                        credentials_not_exported,
                    ),
                    "fingerprint",
                ));
                let Some(created) = self.create_provider_catalog_key(&record).await? else {
                    return Ok(Err(invalid_request(format!(
                        "创建 Provider '{provider_name}' 的 Key 失败"
                    ))));
                };
                // Journal the row immediately after creation.  The pool-score seed below is a
                // separate write and may fail; recording first ensures aggregate compensation
                // can still remove this key when that follow-up operation aborts the import.
                if let Some(journal) = mutation_journal.as_deref_mut() {
                    journal
                        .provider_key_ids
                        .insert((provider.id.clone(), created.id.clone()));
                }
                if oauth_credentials_supplied {
                    seed_imported_oauth_pool_score(self, &provider.id, &created, now_unix_secs)
                        .await?;
                }
                existing_keys.push(created);
                stats.keys.created += 1;
            }

            let imported_models = routed!(parse_admin_system_config_nested_array::<
                ImportedProviderModel,
            >(&raw_provider, "models"));
            let mut existing_models_by_name = self
                .list_all_admin_provider_models_for_system_transfer(&provider.id)
                .await?
                .into_iter()
                .map(|model| (model.provider_model_name.clone(), model))
                .collect::<BTreeMap<_, _>>();

            for imported_model_item in imported_models {
                let (_, imported_model) = imported_model_item.into_parts();
                let Some(global_model_name) = imported_model
                    .global_model_name
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                else {
                    stats.errors.push(format!(
                        "跳过无 global_model_name 的模型 (Provider: {provider_name})"
                    ));
                    continue;
                };
                let Some(global_model_id) = global_models_by_name
                    .get(global_model_name)
                    .map(|model| model.id.clone())
                else {
                    stats.errors.push(format!(
                        "GlobalModel '{global_model_name}' 不存在，跳过模型"
                    ));
                    continue;
                };

                let provider_model_name = invalid!(trim_required(
                    &imported_model.provider_model_name,
                    "provider_model_name"
                ));
                let existing_model = existing_models_by_name.get(&provider_model_name).cloned();

                if let Some(existing_model) = existing_model {
                    match merge_mode {
                        AdminImportMergeMode::Skip => {
                            stats.models.skipped += 1;
                        }
                        AdminImportMergeMode::Error => {
                            return Ok(Err(invalid_request(format!(
                                "Model '{provider_model_name}' 已存在于 Provider '{provider_name}'"
                            ))));
                        }
                        AdminImportMergeMode::Overwrite => {
                            let record = invalid!(build_import_provider_model_record(
                                &provider.id,
                                Some(&existing_model.id),
                                Some(&existing_model),
                                &global_model_id,
                                &imported_model,
                                credentials_not_exported,
                            ));
                            let Some(updated) = self.update_admin_provider_model(&record).await?
                            else {
                                return Ok(Err(invalid_request(format!(
                                    "更新 Provider '{provider_name}' 的模型 '{provider_model_name}' 失败"
                                ))));
                            };
                            existing_models_by_name.insert(provider_model_name, updated);
                            stats.models.updated += 1;
                        }
                    }
                    continue;
                }

                let record = invalid!(build_import_provider_model_record(
                    &provider.id,
                    None,
                    None,
                    &global_model_id,
                    &imported_model,
                    credentials_not_exported,
                ));
                let Some(created) = self.create_admin_provider_model(&record).await? else {
                    return Ok(Err(invalid_request(format!(
                        "创建 Provider '{provider_name}' 的模型 '{provider_model_name}' 失败"
                    ))));
                };
                if let Some(journal) = mutation_journal.as_deref_mut() {
                    journal
                        .provider_model_ids
                        .insert((provider.id.clone(), created.id.clone()));
                }
                existing_models_by_name.insert(provider_model_name, created);
                stats.models.created += 1;
            }
        }

        if let Some(imported_ldap_item) = imported_ldap {
            let (_, ldap_config) = imported_ldap_item.into_parts();
            if !self.has_auth_module_writer() {
                stats.ldap.skipped += 1;
                stats
                    .errors
                    .push("当前运行环境不支持写入 LDAP 配置，已跳过 ldap_config".to_string());
            } else {
                let existing = self.get_ldap_module_config().await?;
                let server_url =
                    invalid!(trim_required(&ldap_config.server_url, "LDAP 服务器地址"));
                let server_url = invalid!(normalize_ldap_transport_server_url(
                    &server_url,
                    ldap_config.use_starttls,
                )
                .ok_or_else(|| {
                    "LDAP 服务器地址必须使用 ldaps://，或在启用 StartTLS 时使用 ldap://；不得包含凭据、查询参数或片段"
                        .to_string()
                }));
                let bind_dn = invalid!(trim_required(&ldap_config.bind_dn, "绑定 DN"));
                let base_dn = invalid!(trim_required(&ldap_config.base_dn, "Base DN"));
                if !ldap_distinguished_name_is_valid(&bind_dn)
                    || !ldap_distinguished_name_is_valid(&base_dn)
                {
                    return Ok(Err(invalid_request(
                        "LDAP 绑定 DN 或 Base DN 格式无效或过长",
                    )));
                }
                let user_search_filter = invalid!(trim_required(
                    ldap_config
                        .user_search_filter
                        .as_deref()
                        .unwrap_or("(uid={username})"),
                    "搜索过滤器",
                ));
                if !ldap_search_filter_is_valid(&user_search_filter) {
                    return Ok(Err(invalid_request(
                        "LDAP 搜索过滤器格式无效，必须包含 {username} 且使用有限的括号结构",
                    )));
                }
                let username_attr = invalid!(trim_required(
                    ldap_config.username_attr.as_deref().unwrap_or("uid"),
                    "用户名属性",
                ));
                let email_attr = invalid!(trim_required(
                    ldap_config.email_attr.as_deref().unwrap_or("mail"),
                    "邮箱属性",
                ));
                let display_name_attr = invalid!(trim_required(
                    ldap_config.display_name_attr.as_deref().unwrap_or("cn"),
                    "显示名称属性",
                ));
                if [
                    username_attr.as_str(),
                    email_attr.as_str(),
                    display_name_attr.as_str(),
                ]
                .into_iter()
                .any(|attribute| !ldap_attribute_description_is_valid(attribute))
                {
                    return Ok(Err(invalid_request(
                        "LDAP 用户名、邮箱或显示名称属性格式无效",
                    )));
                }
                let connect_timeout = ldap_config.connect_timeout.unwrap_or(10);
                if !(1..=60).contains(&connect_timeout) {
                    return Ok(Err(invalid_request(
                        "LDAP connect_timeout 必须在 1 到 60 秒之间",
                    )));
                }
                let config = StoredLdapModuleConfig {
                    server_url,
                    bind_dn,
                    // Password mutation is explicit and separate from the replacement snapshot.
                    // In particular, Preserve never copies a previously read ciphertext here.
                    bind_password_encrypted: None,
                    base_dn,
                    user_search_filter: Some(user_search_filter),
                    username_attr: Some(username_attr),
                    email_attr: Some(email_attr),
                    display_name_attr: Some(display_name_attr),
                    is_enabled: ldap_config.is_enabled,
                    is_exclusive: ldap_config.is_exclusive,
                    use_starttls: ldap_config.use_starttls,
                    connect_timeout: Some(connect_timeout),
                };
                let bind_password = ldap_config
                    .bind_password
                    .as_deref()
                    .map(str::trim)
                    .map(ToOwned::to_owned);
                if bind_password
                    .as_deref()
                    .is_some_and(is_imported_redacted_secret)
                {
                    return Ok(Err(invalid_request("LDAP 脱敏占位符不能作为绑定密码导入")));
                }
                let bind_password_update = match bind_password {
                    Some(password) if password.is_empty() => LdapBindPasswordUpdate::Clear,
                    Some(password) => LdapBindPasswordUpdate::Set(routed!(self
                        .encrypt_ldap_bind_password(&config, &password)
                        .ok_or_else(|| {
                            invalid_request("LDAP 绑定密码加密失败，请检查 Rust 数据加密配置")
                        }))),
                    None => LdapBindPasswordUpdate::Preserve,
                };
                if matches!(&bind_password_update, LdapBindPasswordUpdate::Preserve) {
                    if let Some(existing) = existing.as_ref() {
                        if existing
                            .bind_password_encrypted
                            .as_deref()
                            .is_some_and(|value| !value.trim().is_empty())
                        {
                            let binding_matches = invalid!(
                                crate::handlers::shared::ldap_bind_password_binding_matches(
                                    existing, &config,
                                )
                            );
                            if !binding_matches {
                                return Ok(Err(invalid_request(
                                    "导入 LDAP 时修改了服务器、StartTLS、bind DN 或 Base DN，必须提供绑定密码",
                                )));
                            }
                        }
                    }
                }
                let will_have_password = match &bind_password_update {
                    LdapBindPasswordUpdate::Set(ciphertext) => !ciphertext.trim().is_empty(),
                    LdapBindPasswordUpdate::Clear => false,
                    LdapBindPasswordUpdate::Preserve => existing
                        .as_ref()
                        .and_then(|config| config.bind_password_encrypted.as_deref())
                        .map(str::trim)
                        .is_some_and(|value| !value.is_empty()),
                };
                if existing.is_none()
                    && !matches!(&bind_password_update, LdapBindPasswordUpdate::Set(_))
                {
                    return Ok(Err(invalid_request("首次配置 LDAP 时必须设置绑定密码")));
                }
                if ldap_config.is_exclusive && !ldap_config.is_enabled {
                    return Ok(Err(invalid_request(
                        "仅允许 LDAP 登录 需要先启用 LDAP 认证",
                    )));
                }
                if ldap_config.is_enabled && !will_have_password {
                    return Ok(Err(invalid_request("启用 LDAP 认证 需要先设置绑定密码")));
                }
                if ldap_config.is_enabled && ldap_config.is_exclusive {
                    let admin_count = self
                        .count_active_local_admin_users_with_valid_password()
                        .await?;
                    if admin_count < 1 {
                        return Ok(Err(invalid_request(
                            "启用 LDAP 独占模式前，必须至少保留 1 个有效的本地管理员账户（含有效密码）作为紧急恢复通道",
                        )));
                    }
                }
                match (existing.is_some(), merge_mode) {
                    (true, AdminImportMergeMode::Skip) => stats.ldap.skipped += 1,
                    (true, AdminImportMergeMode::Error) => {
                        return Ok(Err(invalid_request("LDAP 配置已存在")));
                    }
                    (true, AdminImportMergeMode::Overwrite) => {
                        let Some(result) = self
                            .compare_and_swap_ldap_module_config(
                                existing.as_ref(),
                                &config,
                                &bind_password_update,
                            )
                            .await?
                        else {
                            return Ok(Err(invalid_request("更新 LDAP 配置失败")));
                        };
                        if result == CompareAndSwapLdapConfigResult::Conflict {
                            return Ok(Err((
                                http::StatusCode::CONFLICT,
                                json!({
                                    "detail": "LDAP 配置已被其他请求更新，请重试"
                                }),
                            )));
                        }
                        stats.ldap.updated += 1;
                    }
                    (false, _) => {
                        let Some(result) = self
                            .compare_and_swap_ldap_module_config(
                                None,
                                &config,
                                &bind_password_update,
                            )
                            .await?
                        else {
                            return Ok(Err(invalid_request("创建 LDAP 配置失败")));
                        };
                        let CompareAndSwapLdapConfigResult::Applied(created) = result else {
                            return Ok(Err((
                                http::StatusCode::CONFLICT,
                                json!({
                                    "detail": "LDAP 配置已被其他请求创建，请重试"
                                }),
                            )));
                        };
                        // Record the exact persisted snapshot before any later phase can fail.
                        // Compensation will delete it only if no concurrent write changed it.
                        if let Some(journal) = mutation_journal.as_deref_mut() {
                            journal.created_ldap_config = Some(created);
                        }
                        stats.ldap.created += 1;
                    }
                }
            }
        }

        if !imported_oauth_providers.is_empty() {
            let imported_oauth_provider_count = imported_oauth_providers.len();
            let mut oauth_by_type = self
                .list_oauth_provider_configs()
                .await?
                .into_iter()
                .map(|provider| (provider.provider_type.clone(), provider))
                .collect::<BTreeMap<_, _>>();

            for (index, imported_oauth_item) in imported_oauth_providers.into_iter().enumerate() {
                let (_, oauth_provider) = imported_oauth_item.into_parts();
                let original_provider_type = oauth_provider.provider_type.clone();
                let original_enabled = oauth_provider.is_enabled;
                let oauth_provider = invalid!(normalize_legacy_imported_oauth_provider(
                    oauth_provider,
                    &source_version,
                ));
                let provider_type = invalid!(trim_required(
                    &oauth_provider.provider_type,
                    "provider_type",
                ));
                let existed = oauth_by_type.contains_key(&provider_type);
                if existed {
                    match merge_mode {
                        AdminImportMergeMode::Skip => {
                            stats.oauth.skipped += 1;
                            continue;
                        }
                        AdminImportMergeMode::Error => {
                            return Ok(Err(invalid_request(format!(
                                "OAuth Provider '{provider_type}' 已存在"
                            ))));
                        }
                        AdminImportMergeMode::Overwrite => {}
                    }
                }

                let display_name =
                    invalid!(trim_required(&oauth_provider.display_name, "display_name"));
                let client_id = invalid!(trim_required(&oauth_provider.client_id, "client_id"));
                let redirect_uri =
                    invalid!(trim_required(&oauth_provider.redirect_uri, "redirect_uri"));
                let frontend_callback_url = invalid!(trim_required(
                    &oauth_provider.frontend_callback_url,
                    "frontend_callback_url",
                ));
                let mut record = invalid!(build_imported_oauth_provider_record(
                    &oauth_provider,
                    EncryptedSecretUpdate::Preserve,
                ));
                record.provider_type = provider_type.clone();
                record.display_name = display_name;
                record.client_id = client_id;
                record.redirect_uri = redirect_uri;
                record.frontend_callback_url = frontend_callback_url;

                // Bind imported plaintext to the final normalized record, including all
                // endpoint and redirect fields.  Never seal using provider_type alone.
                record.client_secret_encrypted =
                    match oauth_provider.client_secret.as_deref().map(str::trim) {
                        Some(secret) if is_imported_redacted_secret(secret) => {
                            EncryptedSecretUpdate::Preserve
                        }
                        Some(secret) if !secret.is_empty() => EncryptedSecretUpdate::Set(routed!(
                            crate::handlers::shared::seal_identity_oauth_provider_client_secret(
                                self.as_ref(),
                                &record,
                                secret,
                            )
                            .map_err(|message| invalid_request(message))
                        )),
                        _ => EncryptedSecretUpdate::Preserve,
                    };

                // A redacted/omitted secret may preserve an existing value only when the
                // complete OAuth binding is unchanged.  Otherwise the old secret would be
                // replayed against a different client or endpoint after an overwrite import.
                if matches!(
                    &record.client_secret_encrypted,
                    EncryptedSecretUpdate::Preserve
                ) {
                    if let Some(existing) = oauth_by_type.get(&provider_type) {
                        if existing
                            .client_secret_encrypted
                            .as_deref()
                            .is_some_and(|value| !value.trim().is_empty())
                        {
                            let binding_matches = invalid!(
                                crate::handlers::shared::identity_oauth_provider_secret_binding_matches(
                                    existing,
                                    &record,
                                )
                            );
                            if !binding_matches {
                                return Ok(Err(invalid_request(
                                    "导入 OAuth Provider 时修改了 Client ID、端点或 redirect_uri，必须提供 client_secret",
                                )));
                            }
                        }
                    }
                }

                let Some(persisted) = self.upsert_oauth_provider_config(&record).await? else {
                    stats.oauth.skipped += (imported_oauth_provider_count - index) as u64;
                    stats.errors.push(
                        "当前运行环境不支持 OAuth Provider 配置读写，已跳过 oauth_providers"
                            .to_string(),
                    );
                    break;
                };
                oauth_by_type.insert(provider_type.clone(), persisted);
                if original_enabled && !oauth_provider.is_enabled {
                    stats.errors.push(format!(
                        "旧版 OAuth Provider '{original_provider_type}' 已安全迁移并停用，请复核域名白名单后重新启用"
                    ));
                }
                if existed {
                    stats.oauth.updated += 1;
                } else {
                    stats.oauth.created += 1;
                    if let Some(journal) = mutation_journal.as_deref_mut() {
                        journal.oauth_provider_types.insert(provider_type.clone());
                    }
                }
            }
        }

        for imported_config_item in imported_system_configs {
            let (_, system_config) = imported_config_item.into_parts();
            let ImportedSystemConfig {
                key,
                value,
                description,
            } = system_config;
            let normalized_key = normalize_imported_system_config_key(&key);
            if credentials_not_exported
                && (is_sensitive_admin_system_config_key(&normalized_key)
                    || is_interactive_export_private_system_config_key(&normalized_key))
            {
                stats.system_configs.skipped += 1;
                continue;
            }
            let exists = existing_system_config_keys.contains(&normalized_key);
            match (exists, merge_mode) {
                (true, AdminImportMergeMode::Skip) => {
                    stats.system_configs.skipped += 1;
                    continue;
                }
                (true, AdminImportMergeMode::Error) => {
                    return Ok(Err(invalid_request(format!(
                        "SystemConfig '{normalized_key}' 已存在"
                    ))));
                }
                _ => {}
            }

            let request_bytes = Bytes::from(
                serde_json::to_vec(&json!({
                    "value": value,
                    "description": description,
                }))
                .map_err(|err| GatewayError::Internal(err.to_string()))?,
            );
            let update_result =
                apply_admin_system_config_update(self, &key, &request_bytes).await?;
            match update_result {
                Ok(_) => {
                    if exists {
                        stats.system_configs.updated += 1;
                    } else {
                        stats.system_configs.created += 1;
                        existing_system_config_keys.insert(normalized_key.clone());
                        if let Some(journal) = mutation_journal.as_deref_mut() {
                            journal.system_config_keys.insert(normalized_key);
                        }
                    }
                }
                Err((status, payload)) => return Ok(Err((status, payload))),
            }
        }

        Ok(Ok(json!({
            "message": "配置导入成功",
            "stats": stats,
        })))
    }

    pub(crate) async fn import_admin_system_users(
        &self,
        request_body: &Bytes,
        operator_id: Option<&str>,
    ) -> Result<Result<Value, (http::StatusCode, Value)>, GatewayError> {
        let mut mutation_journal = AggregateMutationJournal::default();
        let result = self
            .import_admin_system_users_with_mode(
                request_body,
                operator_id,
                SystemImportMode::InteractiveUpload,
                Some(&mut mutation_journal),
            )
            .await;
        self.finish_standalone_users_import(result, &mutation_journal)
            .await
    }

    pub(crate) async fn restore_admin_system_users_backup(
        &self,
        request_body: &Bytes,
        operator_id: Option<&str>,
        _authority: crate::backup::executor::BackupRestoreAuthority,
    ) -> Result<Result<Value, (http::StatusCode, Value)>, GatewayError> {
        let mut mutation_journal = AggregateMutationJournal::default();
        let result = self
            .import_admin_system_users_with_mode(
                request_body,
                operator_id,
                SystemImportMode::RecoveryBackup,
                Some(&mut mutation_journal),
            )
            .await;
        self.finish_standalone_users_import(result, &mutation_journal)
            .await
    }

    /// Interactive and standalone recovery imports do not have the aggregate config checkpoint
    /// available to their caller. Compensate rows created by this invocation and restore existing
    /// rows only when their mutable fields still match the recorded post-state.
    async fn finish_standalone_users_import(
        &self,
        result: Result<Result<Value, (http::StatusCode, Value)>, GatewayError>,
        mutation_journal: &AggregateMutationJournal,
    ) -> Result<Result<Value, (http::StatusCode, Value)>, GatewayError> {
        match result {
            Ok(Ok(payload)) => Ok(Ok(payload)),
            Ok(Err(original)) => match self
                .rollback_standalone_users_mutations(mutation_journal)
                .await
            {
                Ok(()) => Ok(Err(original)),
                Err(rollback_error) => Err(aggregate_rollback_http_error(
                    "用户阶段",
                    &original,
                    rollback_error,
                )),
            },
            Err(original) => match self
                .rollback_standalone_users_mutations(mutation_journal)
                .await
            {
                Ok(()) => Err(original),
                Err(rollback_error) => Err(aggregate_rollback_error(
                    "用户阶段",
                    original,
                    rollback_error,
                )),
            },
        }
    }

    async fn rollback_standalone_users_mutations(
        &self,
        mutation_journal: &AggregateMutationJournal,
    ) -> Result<(), GatewayError> {
        // Run both compensations even when one fails. Newly-created rows are removed first, while
        // pre-existing wallets are restored through an owner- and snapshot-checked CAS so a
        // concurrent recharge cannot be overwritten by an import failure.
        let created_result = self.rollback_created_users(mutation_journal).await;
        let existing_wallet_result = self.rollback_existing_wallets(mutation_journal).await;
        let existing_users_result = self.rollback_existing_users(mutation_journal).await;
        let existing_groups_result = self.rollback_existing_user_groups(mutation_journal).await;
        let existing_api_keys_result = self.rollback_existing_api_keys(mutation_journal).await;
        let existing_result = combine_rollback_results(
            existing_users_result,
            existing_groups_result,
            "standalone existing users/groups",
        );
        let existing_result = combine_rollback_results(
            existing_api_keys_result,
            existing_result,
            "standalone existing API keys",
        );
        let restore_result = combine_rollback_results(
            created_result,
            existing_wallet_result,
            "standalone users wallets",
        );
        combine_rollback_results(restore_result, existing_result, "standalone users")
    }

    async fn import_admin_system_users_with_mode(
        &self,
        request_body: &Bytes,
        operator_id: Option<&str>,
        mode: SystemImportMode,
        mut mutation_journal: Option<&mut AggregateMutationJournal>,
    ) -> Result<Result<Value, (http::StatusCode, Value)>, GatewayError> {
        if !self.has_auth_user_write_capability()
            || !self.has_auth_wallet_write_capability()
            || !self.has_auth_api_key_writer()
        {
            return Ok(Err((
                http::StatusCode::SERVICE_UNAVAILABLE,
                json!({ "detail": "Admin system data unavailable" }),
            )));
        }
        match self
            .prevalidate_admin_system_users_import(request_body, operator_id, mode)
            .await?
        {
            Ok(()) => {}
            Err(err) => return Ok(Err(err)),
        }
        let root = match serde_json::from_slice::<Value>(request_body) {
            Ok(Value::Object(map)) => map,
            _ => return Ok(Err(invalid_request("请求数据验证失败"))),
        };
        let merge_mode = match serde_json::from_value::<AdminImportMergeMode>(
            root.get("merge_mode").cloned().unwrap_or(Value::Null),
        ) {
            Ok(value) => value,
            Err(_) => {
                return Ok(Err(invalid_request(
                    "merge_mode 仅支持 skip / overwrite / error",
                )));
            }
        };
        let empty = Vec::new();
        let users = match root.get("users") {
            Some(Value::Array(items)) => items,
            Some(_) => return Ok(Err(invalid_request("users 必须是数组"))),
            None => &empty,
        };
        let standalone_keys = match root.get("standalone_keys") {
            Some(Value::Array(items)) => items,
            Some(_) => return Ok(Err(invalid_request("standalone_keys 必须是数组"))),
            None => &empty,
        };
        let imported_user_groups = match root.get("user_groups") {
            Some(Value::Array(items)) => items,
            Some(_) => return Ok(Err(invalid_request("user_groups 必须是数组"))),
            None => &empty,
        };
        let standalone_owner_id = match operator_id {
            Some(candidate) => match self.find_user_auth_by_id(candidate).await? {
                Some(user) if crate::roles::is_full_admin_role(&user.role) => Some(user.id),
                _ => None,
            },
            None => None,
        };

        macro_rules! invalid_value {
            ($expr:expr) => {
                match $expr {
                    Ok(value) => value,
                    Err(detail) => return Ok(Err(invalid_request(detail))),
                }
            };
        }

        // Repository write helpers return `None` when the corresponding writer is unavailable
        // or the target row disappeared. Treat that as a failed import step so the caller's
        // mutation journal can compensate newly-created rows instead of returning a half-imported
        // success.
        macro_rules! require_persisted {
            ($expr:expr) => {
                match $expr {
                    Some(value) => value,
                    None => {
                        return Ok(Err((
                            http::StatusCode::SERVICE_UNAVAILABLE,
                            json!({ "detail": "Admin system data unavailable" }),
                        )))
                    }
                }
            };
        }

        let users_export_version = invalid_value!(
            validate_imported_system_users_export_version_for_mode(root.get("version"), mode)
        );
        let has_portable_provider_access = imported_user_groups.iter().any(|value| {
            value.as_object().is_some_and(|group| {
                group.contains_key("allowed_provider_refs")
                    || group.contains_key("provider_key_policy_refs")
            })
        });
        let (portable_providers, portable_keys) = if has_portable_provider_access {
            if !self.has_provider_catalog_data_reader() {
                return Ok(Err((
                    http::StatusCode::SERVICE_UNAVAILABLE,
                    json!({ "detail": "Provider catalog unavailable" }),
                )));
            }
            let providers = self.list_provider_catalog_providers(false).await?;
            let provider_ids = providers
                .iter()
                .map(|provider| provider.id.clone())
                .collect::<Vec<_>>();
            let keys = self
                .list_provider_catalog_keys_by_provider_ids(&provider_ids)
                .await?;
            (providers, keys)
        } else {
            (Vec::new(), Vec::new())
        };
        let mut normalized_imported_groups = Vec::with_capacity(imported_user_groups.len());
        let mut group_validation_errors = Vec::new();
        for (index, raw_group) in imported_user_groups.iter().enumerate() {
            let field_name = format!("user_groups[{index}]");
            let group = match imported_object_field(raw_group, &field_name) {
                Ok(value) => value,
                Err(detail) => {
                    group_validation_errors.push(detail);
                    continue;
                }
            };
            let (export_id, normalized_name, record) =
                match build_imported_user_group_record(group, &field_name) {
                    Ok(value) => value,
                    Err(detail) => {
                        group_validation_errors.push(detail);
                        continue;
                    }
                };
            let record = match apply_imported_portable_provider_access(
                group,
                &field_name,
                record,
                &portable_providers,
                &portable_keys,
            ) {
                Ok(value) => value,
                Err(detail) => {
                    group_validation_errors.push(detail);
                    continue;
                }
            };
            let record = match self
                .normalize_user_group_provider_access_record(record)
                .await?
            {
                Ok(value) => value,
                Err(detail) => {
                    group_validation_errors.push(format!("{field_name}: {detail}"));
                    continue;
                }
            };
            normalized_imported_groups.push((export_id, normalized_name, record));
        }
        if !group_validation_errors.is_empty() {
            return Ok(Err(invalid_request(format!(
                "用户组 Provider/Key 配置无法导入:\n- {}",
                group_validation_errors.join("\n- ")
            ))));
        }

        let supplemental_user_usage_aggregates = if mode.is_rollback_checkpoint() {
            // Rollback checkpoints must not mutate runtime usage state. In particular, do not
            // synthesize daily rows from the denormalized counters on the user records.
            Vec::new()
        } else {
            invalid_value!(build_imported_user_usage_total_aggregates(
                users,
                root.get("exported_at")
            ))
        };
        let mut stats = AdminSystemUsersImportStats::default();
        let mut imported_user_id_map = BTreeMap::<String, String>::new();
        let mut imported_api_key_id_map = BTreeMap::<String, String>::new();
        let default_group_id = self.effective_default_user_group_id().await?;
        let existing_groups = self.list_user_groups().await?;
        let mut groups_by_name = existing_groups
            .into_iter()
            .map(|group| {
                (
                    aether_data::repository::users::normalize_user_group_name(&group.name)
                        .to_ascii_lowercase(),
                    group,
                )
            })
            .collect::<BTreeMap<_, _>>();
        let mut imported_group_id_map = BTreeMap::<String, String>::new();
        let mut imported_group_name_map = BTreeMap::<String, String>::new();

        for (export_id, normalized_name, record) in normalized_imported_groups {
            if default_group_id
                .as_deref()
                .is_some_and(|group_id| export_id.as_deref() == Some(group_id))
                || normalized_name == "default"
            {
                if let Some(default_group_id) = default_group_id.as_ref() {
                    if let Some(export_id) = export_id {
                        imported_group_id_map.insert(export_id, default_group_id.clone());
                    }
                    imported_group_name_map.insert(normalized_name, default_group_id.clone());
                }
                stats.user_groups.skipped += 1;
                continue;
            }
            let existing_by_id = mode
                .is_rollback_checkpoint()
                .then(|| {
                    export_id.as_deref().and_then(|export_id| {
                        groups_by_name
                            .values()
                            .find(|group| group.id == export_id)
                            .cloned()
                    })
                })
                .flatten();
            if mode.is_rollback_checkpoint() && export_id.is_some() && existing_by_id.is_none() {
                return Ok(Err(invalid_request(format!(
                    "回滚检查点用户组 '{}' 不存在；拒绝按名称匹配",
                    export_id.as_deref().unwrap_or_default()
                ))));
            }
            if let Some(existing) =
                existing_by_id.or_else(|| groups_by_name.get(&normalized_name).cloned())
            {
                if let Some(export_id) = export_id {
                    imported_group_id_map.insert(export_id, existing.id.clone());
                }
                imported_group_name_map.insert(normalized_name.clone(), existing.id.clone());
                match merge_mode {
                    AdminImportMergeMode::Skip => {
                        stats.user_groups.skipped += 1;
                    }
                    AdminImportMergeMode::Error => {
                        return Ok(Err(invalid_request(format!(
                            "用户组 '{}' 已存在",
                            existing.name
                        ))));
                    }
                    AdminImportMergeMode::Overwrite => {
                        if let Some(journal) = mutation_journal.as_deref_mut() {
                            journal
                                .existing_user_groups
                                .entry(existing.id.clone())
                                .or_insert_with(|| ExistingUserGroupMutation {
                                    before: existing.clone(),
                                    after: existing.clone(),
                                });
                        }
                        let Some(updated) = self.update_user_group(&existing.id, record).await?
                        else {
                            return Ok(Err((
                                http::StatusCode::SERVICE_UNAVAILABLE,
                                json!({ "detail": "Admin system data unavailable" }),
                            )));
                        };
                        if let Some(journal) = mutation_journal.as_deref_mut() {
                            if let Some(mutation) =
                                journal.existing_user_groups.get_mut(&existing.id)
                            {
                                mutation.after = updated.clone();
                            }
                        }
                        groups_by_name.retain(|_, group| group.id != existing.id);
                        groups_by_name.insert(normalized_name, updated);
                        stats.user_groups.updated += 1;
                    }
                }
                continue;
            }

            let Some(created) = self.create_user_group(record).await? else {
                return Ok(Err((
                    http::StatusCode::SERVICE_UNAVAILABLE,
                    json!({ "detail": "Admin system data unavailable" }),
                )));
            };
            if let Some(journal) = mutation_journal.as_deref_mut() {
                journal.user_group_ids.insert(created.id.clone());
            }
            if let Some(export_id) = export_id {
                imported_group_id_map.insert(export_id, created.id.clone());
            }
            imported_group_name_map.insert(normalized_name.clone(), created.id.clone());
            groups_by_name.insert(normalized_name, created);
            stats.user_groups.created += 1;
        }

        for (index, raw_user) in users.iter().enumerate() {
            let user = match imported_object_field(raw_user, &format!("users[{index}]")) {
                Ok(value) => value,
                Err(detail) => return Ok(Err(invalid_request(detail))),
            };
            let source_user_id = invalid_value!(imported_optional_string(user.get("id")));
            invalid_value!(validate_rollback_user_source_id(
                mode,
                source_user_id.as_deref(),
            ));
            let Some(role) =
                invalid_value!(normalize_imported_system_user_role(user.get("role"), mode,))
            else {
                let skipped_email = invalid_value!(imported_optional_string(user.get("email")));
                let skipped_username =
                    invalid_value!(imported_optional_string(user.get("username")));
                stats.users.skipped += 1;
                stats.errors.push(format!(
                    "跳过受保护的管理员用户: {}",
                    skipped_email
                        .or(skipped_username)
                        .unwrap_or_else(|| format!("users[{index}]"))
                ));
                continue;
            };

            let email = invalid_value!(imported_optional_string(user.get("email")))
                .map(|value| value.to_ascii_lowercase());
            let email_verified =
                invalid_value!(imported_optional_bool(user.get("email_verified"))).unwrap_or(true);
            let username = invalid_value!(imported_optional_string(user.get("username")))
                .or_else(|| {
                    email.as_ref().map(|value| {
                        value
                            .split('@')
                            .next()
                            .unwrap_or(value.as_str())
                            .to_string()
                    })
                })
                .unwrap_or_else(|| format!("imported-user-{index}"));
            let password_hash = invalid_value!(resolve_imported_password_hash(
                user,
                users_export_version,
                mode,
            ));
            let allowed_providers = invalid_value!(normalize_imported_user_string_list(
                user,
                "allowed_providers"
            ));
            let allowed_api_formats = invalid_value!(normalize_imported_user_api_formats(
                user,
                "allowed_api_formats"
            ));
            let allowed_models =
                invalid_value!(normalize_imported_user_string_list(user, "allowed_models"));
            let rate_limit =
                invalid_value!(imported_optional_i32(user.get("rate_limit"), "rate_limit"));
            let allowed_providers_mode = invalid_value!(imported_user_list_policy_mode(
                user,
                "allowed_providers_mode",
                "allowed_providers",
                &allowed_providers,
            ));
            let allowed_api_formats_mode = invalid_value!(imported_user_list_policy_mode(
                user,
                "allowed_api_formats_mode",
                "allowed_api_formats",
                &allowed_api_formats,
            ));
            let allowed_models_mode = invalid_value!(imported_user_list_policy_mode(
                user,
                "allowed_models_mode",
                "allowed_models",
                &allowed_models,
            ));
            let rate_limit_mode = invalid_value!(imported_user_rate_limit_policy_mode(
                user,
                "rate_limit_mode",
                "rate_limit",
                rate_limit,
            ));
            let imported_user_group_ids = invalid_value!(resolve_imported_user_group_ids(
                user,
                &imported_group_id_map,
                &imported_group_name_map,
                &groups_by_name,
            ));
            let group_ids = if user.contains_key("group_ids") || user.contains_key("group_names") {
                let group_ids = self
                    .include_default_user_group_ids(&imported_user_group_ids)
                    .await?;
                if !group_ids.is_empty() {
                    let existing_groups = self.list_user_groups_by_ids(&group_ids).await?;
                    if existing_groups.len() != group_ids.len() {
                        return Ok(Err(invalid_request(format!(
                            "用户 '{}' 的用户组不存在",
                            email.clone().unwrap_or(username.clone())
                        ))));
                    }
                }
                Some(group_ids)
            } else {
                None
            };
            let is_active =
                invalid_value!(imported_optional_bool(user.get("is_active"))).unwrap_or(true);
            let model_capability_settings = invalid_value!(imported_optional_json_object(
                user.get("model_capability_settings"),
                "model_capability_settings"
            ));
            let feature_settings = invalid_value!(imported_optional_json_object(
                user.get("feature_settings"),
                "feature_settings"
            )
            .and_then(normalize_admin_feature_settings));
            let wallet_payload = match user.get("wallet") {
                Some(Value::Object(map)) => Some(map),
                Some(Value::Null) | None => None,
                Some(_) => return Ok(Err(invalid_request("wallet 必须是对象"))),
            };
            let wallet_target = match wallet_payload {
                Some(wallet) => Some(invalid_value!(normalize_imported_wallet_target(
                    Some(wallet),
                    false,
                ))),
                None => None,
            };

            // Checkpoint IDs are the only safe identity during compensation. Email and username
            // are mutable fields and may have been changed by the failed import itself. Refuse to
            // guess if the stable row disappeared instead of overwriting an unrelated account.
            let existing_user = if mode.is_rollback_checkpoint() {
                let source_user_id = source_user_id.as_deref().unwrap_or_default();
                let existing = self.find_user_auth_by_id(source_user_id).await?;
                if existing.is_none() {
                    return Ok(Err(invalid_request(format!(
                        "回滚检查点用户 '{source_user_id}' 不存在；拒绝按 email/username 匹配"
                    ))));
                }
                existing
            } else {
                let mut existing = if let Some(email) = email.as_deref() {
                    self.find_user_auth_by_identifier(email).await?
                } else {
                    None
                };
                if existing.is_none() {
                    existing = self.find_user_auth_by_identifier(&username).await?;
                }
                existing
            };

            let user_id = if let Some(existing) = existing_user {
                if imported_existing_user_is_protected(&existing.role, mode) {
                    stats.users.skipped += 1;
                    stats.errors.push(format!(
                        "跳过受保护的管理员用户记录: {}",
                        email.clone().unwrap_or(username.clone())
                    ));
                    continue;
                }
                match merge_mode {
                    AdminImportMergeMode::Skip => {
                        stats.users.skipped += 1;
                        continue;
                    }
                    AdminImportMergeMode::Error => {
                        return Ok(Err(invalid_request(format!(
                            "用户 '{}' 已存在",
                            email.clone().unwrap_or(username.clone())
                        ))));
                    }
                    AdminImportMergeMode::Overwrite => {
                        if let Some(journal) = mutation_journal.as_deref_mut() {
                            self.capture_existing_user_mutation(journal, &existing)
                                .await?;
                        }
                        if let Some(email) = email.as_deref() {
                            if self
                                .is_other_user_auth_email_taken(email, &existing.id)
                                .await?
                            {
                                return Ok(Err(invalid_request(format!("邮箱已存在: {email}"))));
                            }
                        }
                        if self
                            .is_other_user_auth_username_taken(&username, &existing.id)
                            .await?
                        {
                            return Ok(Err(invalid_request(format!("用户名已存在: {username}"))));
                        }
                        let email_present = email.is_some() || mode.is_rollback_checkpoint();
                        let email_verified_update = if mode.is_rollback_checkpoint() {
                            user.contains_key("email_verified")
                                .then_some(email_verified)
                        } else {
                            email.as_deref().and_then(|email| {
                                existing
                                    .email
                                    .as_deref()
                                    .is_none_or(|current| {
                                        !current.trim().eq_ignore_ascii_case(email.trim())
                                    })
                                    .then_some(email_verified)
                            })
                        };
                        let updated_profile = self
                            .update_local_auth_user_profile(
                                &existing.id,
                                email_present,
                                email.clone(),
                                email_verified_update,
                                Some(username.clone()),
                            )
                            .await?;
                        if updated_profile.is_none() {
                            return Ok(Err((
                                http::StatusCode::SERVICE_UNAVAILABLE,
                                json!({ "detail": "Admin system data unavailable" }),
                            )));
                        }
                        self.refresh_existing_user_mutation(
                            mutation_journal.as_deref_mut(),
                            &existing.id,
                        )
                        .await?;
                        if let Some(password_hash) =
                            password_hash.as_deref().filter(|value| !value.is_empty())
                        {
                            let updated_password = self
                                .reset_local_auth_user_password_and_revoke_sessions(
                                    &existing.id,
                                    password_hash.to_string(),
                                    chrono::Utc::now(),
                                )
                                .await?;
                            if !updated_password {
                                return Ok(Err((
                                    http::StatusCode::SERVICE_UNAVAILABLE,
                                    json!({ "detail": "Admin system data unavailable" }),
                                )));
                            }
                            self.refresh_existing_user_mutation(
                                mutation_journal.as_deref_mut(),
                                &existing.id,
                            )
                            .await?;
                        }
                        let updated_admin_fields = self
                            .update_local_auth_user_admin_fields(
                                &existing.id,
                                Some(role.clone()),
                                user.contains_key("allowed_providers"),
                                allowed_providers.clone(),
                                user.contains_key("allowed_api_formats"),
                                allowed_api_formats.clone(),
                                user.contains_key("allowed_models"),
                                allowed_models.clone(),
                                user.contains_key("rate_limit"),
                                rate_limit,
                                Some(is_active),
                            )
                            .await?;
                        if updated_admin_fields.is_none() {
                            return Ok(Err((
                                http::StatusCode::SERVICE_UNAVAILABLE,
                                json!({ "detail": "Admin system data unavailable" }),
                            )));
                        }
                        self.refresh_existing_user_mutation(
                            mutation_journal.as_deref_mut(),
                            &existing.id,
                        )
                        .await?;
                        if user.contains_key("email_verified") {
                            stats.errors.push(format!(
                                "用户 '{}' 的 email_verified 当前不会覆盖已有值",
                                email.clone().unwrap_or(username.clone())
                            ));
                        }
                        if user.contains_key("model_capability_settings") {
                            let updated = self
                                .update_user_model_capability_settings(
                                    &existing.id,
                                    model_capability_settings.clone(),
                                )
                                .await?;
                            if model_capability_settings.is_some() && updated.is_none() {
                                return Ok(Err((
                                    http::StatusCode::SERVICE_UNAVAILABLE,
                                    json!({ "detail": "Admin system data unavailable" }),
                                )));
                            }
                            self.refresh_existing_user_mutation(
                                mutation_journal.as_deref_mut(),
                                &existing.id,
                            )
                            .await?;
                        }
                        if user.contains_key("feature_settings") {
                            let updated = self
                                .update_user_feature_settings(
                                    &existing.id,
                                    feature_settings.clone(),
                                )
                                .await?;
                            if feature_settings.is_some() && updated.is_none() {
                                return Ok(Err((
                                    http::StatusCode::SERVICE_UNAVAILABLE,
                                    json!({ "detail": "Admin system data unavailable" }),
                                )));
                            }
                            self.refresh_existing_user_mutation(
                                mutation_journal.as_deref_mut(),
                                &existing.id,
                            )
                            .await?;
                        }
                        if allowed_providers_mode.is_some()
                            || allowed_api_formats_mode.is_some()
                            || allowed_models_mode.is_some()
                            || rate_limit_mode.is_some()
                        {
                            let updated_policy_modes = self
                                .update_local_auth_user_policy_modes(
                                    &existing.id,
                                    allowed_providers_mode.clone(),
                                    allowed_api_formats_mode.clone(),
                                    allowed_models_mode.clone(),
                                    rate_limit_mode.clone(),
                                )
                                .await?;
                            if updated_policy_modes.is_none() {
                                return Ok(Err((
                                    http::StatusCode::SERVICE_UNAVAILABLE,
                                    json!({ "detail": "Admin system data unavailable" }),
                                )));
                            }
                            self.refresh_existing_user_mutation(
                                mutation_journal.as_deref_mut(),
                                &existing.id,
                            )
                            .await?;
                        }
                        if let Some(group_ids) = group_ids.as_ref() {
                            let persisted_groups = self
                                .replace_user_groups_for_user(&existing.id, group_ids)
                                .await?;
                            if persisted_groups.len() != group_ids.len() {
                                return Ok(Err(invalid_request(format!(
                                    "用户 '{}' 的用户组未能完整写入",
                                    email.clone().unwrap_or(username.clone())
                                ))));
                            }
                            self.refresh_existing_user_mutation(
                                mutation_journal.as_deref_mut(),
                                &existing.id,
                            )
                            .await?;
                        }
                        if let Some(wallet_target) = wallet_target.as_ref() {
                            self.sync_imported_user_wallet(
                                &existing.id,
                                wallet_target,
                                &email.clone().unwrap_or(username.clone()),
                                mutation_journal.as_deref_mut(),
                            )
                            .await?;
                        }
                        stats.users.updated += 1;
                        existing.id
                    }
                }
            } else {
                let created = self
                    .create_local_auth_user_with_settings(
                        email.clone(),
                        email_verified,
                        username.clone(),
                        password_hash.unwrap_or_else(imported_password_tombstone),
                        role.clone(),
                        allowed_providers.clone(),
                        allowed_api_formats.clone(),
                        allowed_models.clone(),
                        rate_limit,
                    )
                    .await?;
                let Some(created) = created else {
                    return Ok(Err((
                        http::StatusCode::SERVICE_UNAVAILABLE,
                        json!({ "detail": "Admin system data unavailable" }),
                    )));
                };
                if let Some(journal) = mutation_journal.as_deref_mut() {
                    journal.user_ids.insert(created.id.clone());
                }
                if user.contains_key("model_capability_settings") {
                    let updated = self
                        .update_user_model_capability_settings(
                            &created.id,
                            model_capability_settings.clone(),
                        )
                        .await?;
                    if model_capability_settings.is_some() && updated.is_none() {
                        return Ok(Err((
                            http::StatusCode::SERVICE_UNAVAILABLE,
                            json!({ "detail": "Admin system data unavailable" }),
                        )));
                    }
                }
                if user.contains_key("feature_settings") {
                    let updated = self
                        .update_user_feature_settings(&created.id, feature_settings.clone())
                        .await?;
                    if feature_settings.is_some() && updated.is_none() {
                        return Ok(Err((
                            http::StatusCode::SERVICE_UNAVAILABLE,
                            json!({ "detail": "Admin system data unavailable" }),
                        )));
                    }
                }
                let created = if allowed_providers_mode.is_some()
                    || allowed_api_formats_mode.is_some()
                    || allowed_models_mode.is_some()
                    || rate_limit_mode.is_some()
                {
                    let Some(updated_policy_modes) = self
                        .update_local_auth_user_policy_modes(
                            &created.id,
                            allowed_providers_mode.clone(),
                            allowed_api_formats_mode.clone(),
                            allowed_models_mode.clone(),
                            rate_limit_mode.clone(),
                        )
                        .await?
                    else {
                        return Ok(Err((
                            http::StatusCode::SERVICE_UNAVAILABLE,
                            json!({ "detail": "Admin system data unavailable" }),
                        )));
                    };
                    updated_policy_modes
                } else {
                    created
                };
                if let Some(group_ids) = group_ids.as_ref() {
                    let persisted_groups = self
                        .replace_user_groups_for_user(&created.id, group_ids)
                        .await?;
                    if persisted_groups.len() != group_ids.len() {
                        return Ok(Err(invalid_request(format!(
                            "用户 '{}' 的用户组未能完整写入",
                            email.clone().unwrap_or(username.clone())
                        ))));
                    }
                }
                if let Some(wallet_target) = wallet_target.as_ref() {
                    self.sync_imported_user_wallet(
                        &created.id,
                        wallet_target,
                        &email.clone().unwrap_or(username.clone()),
                        mutation_journal.as_deref_mut(),
                    )
                    .await?;
                }
                stats.users.created += 1;
                created.id
            };
            if let Some(source_user_id) = source_user_id {
                imported_user_id_map.insert(source_user_id, user_id.clone());
            }

            let existing_api_keys = self
                .list_auth_api_key_export_records_by_user_ids(std::slice::from_ref(&user_id))
                .await?
                .into_iter()
                .filter(|record| !record.is_standalone)
                .collect::<Vec<_>>();
            let imported_api_keys = match user.get("api_keys") {
                Some(Value::Array(items)) => items,
                Some(_) => return Ok(Err(invalid_request("api_keys 必须是数组"))),
                None => &empty,
            };
            let mut existing_api_keys_by_hash = BTreeMap::new();
            for record in existing_api_keys {
                let api_key_id = record.api_key_id.clone();
                existing_api_keys_by_hash.insert(record.key_hash.clone(), record.clone());
                if mode.is_rollback_checkpoint() {
                    existing_api_keys_by_hash
                        .entry(imported_api_key_tombstone(&api_key_id))
                        .or_insert(record);
                }
            }

            for (key_index, raw_key) in imported_api_keys.iter().enumerate() {
                let key = match imported_object_field(
                    raw_key,
                    &format!("users[{index}].api_keys[{key_index}]"),
                ) {
                    Ok(value) => value,
                    Err(detail) => return Ok(Err(invalid_request(detail))),
                };
                let Some(key_material) = invalid_value!(self
                    .resolve_imported_system_user_api_key_material(
                        key,
                        users_export_version,
                        mode,
                    ))
                else {
                    stats.api_keys.skipped += 1;
                    stats.errors.push(format!(
                        "跳过无效 API Key: 用户 '{}'",
                        email.clone().unwrap_or(username.clone())
                    ));
                    continue;
                };
                let key_hash = key_material.key_hash;
                let key_plaintext = key_material.key_plaintext;
                let source_api_key_id =
                    invalid_value!(imported_optional_string(key.get("api_key_id")));
                let name = invalid_value!(imported_optional_string(key.get("name")));
                let allowed_providers = invalid_value!(normalize_imported_user_string_list(
                    key,
                    "allowed_providers"
                ));
                let allowed_api_formats = invalid_value!(normalize_imported_user_api_formats(
                    key,
                    "allowed_api_formats"
                ));
                let allowed_models =
                    invalid_value!(normalize_imported_user_string_list(key, "allowed_models"));
                let ip_rules = invalid_value!(normalize_imported_user_ip_rules(key));
                let imported_rate_limit =
                    invalid_value!(imported_optional_i32(key.get("rate_limit"), "rate_limit"));
                // Legacy uploads historically normalize an omitted rate limit to zero. Rollback
                // checkpoints instead preserve the nullable database value exactly.
                let rate_limit = imported_rate_limit.unwrap_or(0);
                let rate_limit_value = if mode.is_rollback_checkpoint() {
                    imported_rate_limit
                } else {
                    Some(rate_limit)
                };
                let concurrent_limit = invalid_value!(imported_optional_i32(
                    key.get("concurrent_limit"),
                    "concurrent_limit"
                ));
                if concurrent_limit.is_some_and(|value| value < 0) {
                    return Ok(Err(invalid_request("concurrent_limit 必须是非负整数")));
                }
                let force_capabilities = imported_optional_value(key.get("force_capabilities"));
                let is_active =
                    invalid_value!(imported_optional_bool(key.get("is_active"))).unwrap_or(false);
                let expires_at_unix_secs = invalid_value!(imported_rfc3339_to_unix_secs(
                    key.get("expires_at"),
                    "expires_at"
                ));
                let auto_delete_on_expiry =
                    invalid_value!(imported_optional_bool(key.get("auto_delete_on_expiry")))
                        .unwrap_or(false);
                let imported_total_requests = invalid_value!(imported_optional_u64(
                    key.get("total_requests"),
                    "total_requests"
                ));
                let total_requests = imported_total_requests.unwrap_or(0);
                let imported_total_tokens = invalid_value!(imported_optional_u64(
                    key.get("total_tokens"),
                    "total_tokens"
                ));
                let total_tokens = imported_total_tokens.unwrap_or(0);
                let imported_total_cost_usd = invalid_value!(imported_optional_f64(
                    key.get("total_cost_usd"),
                    "total_cost_usd"
                ));
                let total_cost_usd = imported_total_cost_usd.unwrap_or(0.0);
                let feature_settings = invalid_value!(imported_optional_json_object(
                    key.get("feature_settings"),
                    "feature_settings"
                )
                .and_then(normalize_admin_feature_settings));

                if let Some(existing_key) = existing_api_keys_by_hash.get(&key_hash).cloned() {
                    match merge_mode {
                        AdminImportMergeMode::Skip => {
                            stats.api_keys.skipped += 1;
                        }
                        AdminImportMergeMode::Error => {
                            return Ok(Err(invalid_request(format!(
                                "用户 '{}' 的 API Key 已存在",
                                email.clone().unwrap_or(username.clone())
                            ))));
                        }
                        AdminImportMergeMode::Overwrite => {
                            let key_encrypted = invalid_value!(self
                                .seal_imported_auth_api_key_secret(
                                    key_plaintext.as_deref(),
                                    &user_id,
                                    &existing_key.api_key_id,
                                    &key_hash,
                                    false,
                                ));
                            if let Some(journal) = mutation_journal.as_deref_mut() {
                                let key = (user_id.clone(), existing_key.api_key_id.clone());
                                journal
                                    .existing_user_api_keys
                                    .entry(key)
                                    .or_insert_with(|| ExistingApiKeyMutation {
                                        before: existing_key.clone(),
                                        after: existing_key.clone(),
                                    });
                            }
                            let updated = self
                                .update_user_api_key_basic(
                                    aether_data::repository::auth::UpdateUserApiKeyBasicRecord {
                                        user_id: user_id.clone(),
                                        api_key_id: existing_key.api_key_id.clone(),
                                        key_encrypted: key_encrypted.clone(),
                                        key_encrypted_present: key_encrypted.is_some()
                                            || mode == SystemImportMode::RecoveryRollbackCheckpoint,
                                        name: name.clone(),
                                        name_present: name.is_some()
                                            || mode.is_rollback_checkpoint(),
                                        rate_limit: rate_limit_value,
                                        rate_limit_present: true,
                                        concurrent_limit: if key.contains_key("concurrent_limit")
                                            || mode.is_rollback_checkpoint()
                                        {
                                            concurrent_limit
                                        } else {
                                            None
                                        },
                                        concurrent_limit_present: key
                                            .contains_key("concurrent_limit")
                                            || mode.is_rollback_checkpoint(),
                                        ip_rules: imported_ip_rules_present(key)
                                            .then(|| ip_rules.clone()),
                                        feature_settings: key
                                            .contains_key("feature_settings")
                                            .then(|| feature_settings.clone()),
                                    },
                                )
                                .await?;
                            if updated.is_none() {
                                return Ok(Err((
                                    http::StatusCode::SERVICE_UNAVAILABLE,
                                    json!({ "detail": "Admin system data unavailable" }),
                                )));
                            }
                            self.refresh_existing_api_key_mutation(
                                mutation_journal.as_deref_mut(),
                                Some(&user_id),
                                &existing_key.api_key_id,
                                false,
                            )
                            .await?;
                            let _ = require_persisted!(
                                self.set_user_api_key_allowed_providers(
                                    &user_id,
                                    &existing_key.api_key_id,
                                    allowed_providers.clone(),
                                )
                                .await?
                            );
                            self.refresh_existing_api_key_mutation(
                                mutation_journal.as_deref_mut(),
                                Some(&user_id),
                                &existing_key.api_key_id,
                                false,
                            )
                            .await?;
                            let _ = require_persisted!(
                                self.set_user_api_key_force_capabilities(
                                    &user_id,
                                    &existing_key.api_key_id,
                                    force_capabilities.clone(),
                                )
                                .await?
                            );
                            self.refresh_existing_api_key_mutation(
                                mutation_journal.as_deref_mut(),
                                Some(&user_id),
                                &existing_key.api_key_id,
                                false,
                            )
                            .await?;
                            let _ = require_persisted!(
                                self.set_user_api_key_active(
                                    &user_id,
                                    &existing_key.api_key_id,
                                    mode.preserves_active_state() && is_active,
                                )
                                .await?
                            );
                            self.refresh_existing_api_key_mutation(
                                mutation_journal.as_deref_mut(),
                                Some(&user_id),
                                &existing_key.api_key_id,
                                false,
                            )
                            .await?;
                            if imported_total_requests.is_some()
                                || imported_total_tokens.is_some()
                                || imported_total_cost_usd.is_some()
                            {
                                let updated_usage = self
                                    .set_api_key_usage_totals(
                                        &existing_key.api_key_id,
                                        imported_total_requests
                                            .unwrap_or(existing_key.total_requests),
                                        imported_total_tokens.unwrap_or(existing_key.total_tokens),
                                        imported_total_cost_usd
                                            .unwrap_or(existing_key.total_cost_usd),
                                    )
                                    .await?;
                                if updated_usage.is_none() {
                                    return Ok(Err((
                                        http::StatusCode::SERVICE_UNAVAILABLE,
                                        json!({ "detail": "Admin system data unavailable" }),
                                    )));
                                }
                                self.refresh_existing_api_key_mutation(
                                    mutation_journal.as_deref_mut(),
                                    Some(&user_id),
                                    &existing_key.api_key_id,
                                    false,
                                )
                                .await?;
                            }
                            if key.contains_key("allowed_api_formats")
                                || key.contains_key("allowed_models")
                                || key.contains_key("expires_at")
                                || key.contains_key("auto_delete_on_expiry")
                            {
                                stats.errors.push(format!(
                                    "用户 '{}' 的现有 API Key 仅覆盖基础字段；高级导入字段保持原值",
                                    email.clone().unwrap_or(username.clone())
                                ));
                            }
                            stats.api_keys.updated += 1;
                            if let Some(source_api_key_id) = source_api_key_id.clone() {
                                imported_api_key_id_map
                                    .insert(source_api_key_id, existing_key.api_key_id.clone());
                            }
                        }
                    }
                    continue;
                }

                let api_key_id = imported_api_key_id_for_mode(source_api_key_id.as_deref(), mode);
                let key_encrypted = invalid_value!(self.seal_imported_auth_api_key_secret(
                    key_plaintext.as_deref(),
                    &user_id,
                    &api_key_id,
                    &key_hash,
                    false,
                ));
                let created = self
                    .create_user_api_key(aether_data::repository::auth::CreateUserApiKeyRecord {
                        user_id: user_id.clone(),
                        // Preserve a checkpoint API-key ID when a missing row must be recreated;
                        // ordinary imports continue to receive fresh IDs.
                        api_key_id,
                        key_hash: key_hash.clone(),
                        key_encrypted,
                        name,
                        allowed_providers,
                        allowed_api_formats,
                        allowed_models,
                        ip_rules,
                        rate_limit,
                        concurrent_limit,
                        force_capabilities,
                        feature_settings: feature_settings.clone(),
                        is_active: mode.preserves_active_state() && is_active,
                        expires_at_unix_secs,
                        auto_delete_on_expiry,
                        total_requests,
                        total_tokens,
                        total_cost_usd,
                    })
                    .await?;
                let Some(created) = created else {
                    return Ok(Err((
                        http::StatusCode::SERVICE_UNAVAILABLE,
                        json!({ "detail": "Admin system data unavailable" }),
                    )));
                };
                let created_api_key_id = created.api_key_id.clone();
                if let Some(journal) = mutation_journal.as_deref_mut() {
                    journal
                        .user_api_key_ids
                        .insert((user_id.clone(), created_api_key_id.clone()));
                }
                existing_api_keys_by_hash.insert(key_hash, created);
                if let Some(source_api_key_id) = source_api_key_id {
                    imported_api_key_id_map.insert(source_api_key_id, created_api_key_id);
                }
                stats.api_keys.created += 1;
            }
        }

        if !standalone_keys.is_empty() {
            let Some(standalone_owner_id) = standalone_owner_id else {
                stats.standalone_keys.skipped += standalone_keys.len() as u64;
                stats
                    .errors
                    .push("无法导入独立余额 Key: 当前管理员用户记录不存在".to_string());
                if !mode.is_rollback_checkpoint() {
                    if let Some(summary) = self
                        .import_admin_system_user_usage_aggregates(
                            root.get("usage_aggregates"),
                            &supplemental_user_usage_aggregates,
                            &imported_user_id_map,
                            &imported_api_key_id_map,
                            merge_mode,
                        )
                        .await?
                    {
                        stats.usage_aggregates = Some(summary);
                    }
                }
                return Ok(Ok(json!({
                    "message": "用户数据导入成功",
                    "stats": stats,
                })));
            };

            let existing_standalone_keys = self
                .list_auth_api_key_export_standalone_records()
                .await?
                .into_iter()
                .collect::<Vec<_>>();
            let mut existing_standalone_by_hash = BTreeMap::new();
            for record in existing_standalone_keys {
                let api_key_id = record.api_key_id.clone();
                existing_standalone_by_hash.insert(record.key_hash.clone(), record.clone());
                if mode.is_rollback_checkpoint() {
                    existing_standalone_by_hash
                        .entry(imported_api_key_tombstone(&api_key_id))
                        .or_insert(record);
                }
            }

            for (index, raw_key) in standalone_keys.iter().enumerate() {
                let key = match imported_object_field(raw_key, &format!("standalone_keys[{index}]"))
                {
                    Ok(value) => value,
                    Err(detail) => return Ok(Err(invalid_request(detail))),
                };
                let Some(key_material) = invalid_value!(self
                    .resolve_imported_system_user_api_key_material(
                        key,
                        users_export_version,
                        mode,
                    ))
                else {
                    stats.standalone_keys.skipped += 1;
                    stats
                        .errors
                        .push(format!("跳过无效独立余额 Key: standalone_keys[{index}]"));
                    continue;
                };
                let key_hash = key_material.key_hash;
                let key_plaintext = key_material.key_plaintext;
                let source_api_key_id =
                    invalid_value!(imported_optional_string(key.get("api_key_id")));
                let name = invalid_value!(imported_optional_string(key.get("name")));
                let allowed_providers = invalid_value!(normalize_imported_user_string_list(
                    key,
                    "allowed_providers"
                ));
                let allowed_api_formats = invalid_value!(normalize_imported_user_api_formats(
                    key,
                    "allowed_api_formats"
                ));
                let allowed_models =
                    invalid_value!(normalize_imported_user_string_list(key, "allowed_models"));
                let ip_rules = invalid_value!(normalize_imported_user_ip_rules(key));
                let rate_limit =
                    invalid_value!(imported_optional_i32(key.get("rate_limit"), "rate_limit"))
                        .unwrap_or(0);
                let concurrent_limit = invalid_value!(imported_optional_i32(
                    key.get("concurrent_limit"),
                    "concurrent_limit"
                ));
                if concurrent_limit.is_some_and(|value| value < 0) {
                    return Ok(Err(invalid_request("concurrent_limit 必须是非负整数")));
                }
                let force_capabilities = imported_optional_value(key.get("force_capabilities"));
                let is_active =
                    invalid_value!(imported_optional_bool(key.get("is_active"))).unwrap_or(false);
                let expires_at_unix_secs = invalid_value!(imported_rfc3339_to_unix_secs(
                    key.get("expires_at"),
                    "expires_at"
                ));
                let auto_delete_on_expiry =
                    invalid_value!(imported_optional_bool(key.get("auto_delete_on_expiry")))
                        .unwrap_or(false);
                let imported_total_requests = invalid_value!(imported_optional_u64(
                    key.get("total_requests"),
                    "total_requests"
                ));
                let total_requests = imported_total_requests.unwrap_or(0);
                let imported_total_tokens = invalid_value!(imported_optional_u64(
                    key.get("total_tokens"),
                    "total_tokens"
                ));
                let total_tokens = imported_total_tokens.unwrap_or(0);
                let imported_total_cost_usd = invalid_value!(imported_optional_f64(
                    key.get("total_cost_usd"),
                    "total_cost_usd"
                ));
                let total_cost_usd = imported_total_cost_usd.unwrap_or(0.0);
                let feature_settings = invalid_value!(imported_optional_json_object(
                    key.get("feature_settings"),
                    "feature_settings"
                )
                .and_then(normalize_admin_feature_settings));
                let wallet_payload = match key.get("wallet") {
                    Some(Value::Object(map)) => Some(map),
                    Some(Value::Null) | None => None,
                    Some(_) => return Ok(Err(invalid_request("wallet 必须是对象"))),
                };
                let unlimited =
                    invalid_value!(imported_optional_bool(key.get("unlimited"))).unwrap_or(false);
                let wallet_target = match wallet_payload {
                    Some(wallet) => Some(invalid_value!(normalize_imported_wallet_target(
                        Some(wallet),
                        unlimited,
                    ))),
                    None => None,
                };

                if let Some(existing_key) = existing_standalone_by_hash.get(&key_hash).cloned() {
                    match merge_mode {
                        AdminImportMergeMode::Skip => {
                            stats.standalone_keys.skipped += 1;
                        }
                        AdminImportMergeMode::Error => {
                            return Ok(Err(invalid_request("独立余额 Key 已存在")));
                        }
                        AdminImportMergeMode::Overwrite => {
                            let key_encrypted = invalid_value!(self
                                .seal_imported_auth_api_key_secret(
                                    key_plaintext.as_deref(),
                                    &existing_key.user_id,
                                    &existing_key.api_key_id,
                                    &key_hash,
                                    true,
                                ));
                            if let Some(journal) = mutation_journal.as_deref_mut() {
                                journal
                                    .existing_standalone_api_keys
                                    .entry(existing_key.api_key_id.clone())
                                    .or_insert_with(|| ExistingApiKeyMutation {
                                        before: existing_key.clone(),
                                        after: existing_key.clone(),
                                    });
                            }
                            let updated = self
                            .update_standalone_api_key_basic(
                                aether_data::repository::auth::UpdateStandaloneApiKeyBasicRecord {
                                    api_key_id: existing_key.api_key_id.clone(),
                                    key_encrypted: key_encrypted.clone(),
                                    key_encrypted_present: key_encrypted.is_some()
                                        || mode == SystemImportMode::RecoveryRollbackCheckpoint,
                                    name: name.clone(),
                                    name_present: name.is_some(),
                                    force_capabilities: None,
                                    rate_limit_present: true,
                                    rate_limit: Some(rate_limit),
                                    concurrent_limit_present: key.contains_key("concurrent_limit"),
                                    concurrent_limit,
                                    allowed_providers: Some(allowed_providers.clone()),
                                    allowed_api_formats: Some(allowed_api_formats.clone()),
                                    allowed_models: Some(allowed_models.clone()),
                                    ip_rules: imported_ip_rules_present(key)
                                        .then(|| ip_rules.clone()),
                                    expires_at_present: false,
                                    expires_at_unix_secs: None,
                                    auto_delete_on_expiry_present: false,
                                    auto_delete_on_expiry: false,
                                },
                            )
                            .await?;
                            if updated.is_none() {
                                return Ok(Err((
                                    http::StatusCode::SERVICE_UNAVAILABLE,
                                    json!({ "detail": "Admin system data unavailable" }),
                                )));
                            }
                            self.refresh_existing_api_key_mutation(
                                mutation_journal.as_deref_mut(),
                                None,
                                &existing_key.api_key_id,
                                true,
                            )
                            .await?;
                            let _ = require_persisted!(
                                self.set_standalone_api_key_active(
                                    &existing_key.api_key_id,
                                    mode.preserves_active_state() && is_active,
                                )
                                .await?
                            );
                            self.refresh_existing_api_key_mutation(
                                mutation_journal.as_deref_mut(),
                                None,
                                &existing_key.api_key_id,
                                true,
                            )
                            .await?;
                            if key.contains_key("feature_settings") {
                                let _ = require_persisted!(
                                    self.set_standalone_api_key_feature_settings(
                                        &existing_key.api_key_id,
                                        feature_settings.clone(),
                                    )
                                    .await?
                                );
                                self.refresh_existing_api_key_mutation(
                                    mutation_journal.as_deref_mut(),
                                    None,
                                    &existing_key.api_key_id,
                                    true,
                                )
                                .await?;
                            }
                            if imported_total_requests.is_some()
                                || imported_total_tokens.is_some()
                                || imported_total_cost_usd.is_some()
                            {
                                let updated_usage = self
                                    .set_api_key_usage_totals(
                                        &existing_key.api_key_id,
                                        imported_total_requests
                                            .unwrap_or(existing_key.total_requests),
                                        imported_total_tokens.unwrap_or(existing_key.total_tokens),
                                        imported_total_cost_usd
                                            .unwrap_or(existing_key.total_cost_usd),
                                    )
                                    .await?;
                                if updated_usage.is_none() {
                                    return Ok(Err((
                                        http::StatusCode::SERVICE_UNAVAILABLE,
                                        json!({ "detail": "Admin system data unavailable" }),
                                    )));
                                }
                                self.refresh_existing_api_key_mutation(
                                    mutation_journal.as_deref_mut(),
                                    None,
                                    &existing_key.api_key_id,
                                    true,
                                )
                                .await?;
                            }
                            if key.contains_key("expires_at")
                                || key.contains_key("auto_delete_on_expiry")
                                || key.contains_key("force_capabilities")
                            {
                                stats.errors.push(
                                    "现有独立余额 Key 仅覆盖基础字段；高级导入字段保持原值"
                                        .to_string(),
                                );
                            }
                            if let Some(wallet_target) = wallet_target.as_ref() {
                                self.sync_imported_api_key_wallet(
                                    &existing_key.api_key_id,
                                    wallet_target,
                                    key.get("name")
                                        .and_then(Value::as_str)
                                        .unwrap_or("独立余额 Key"),
                                    mutation_journal.as_deref_mut(),
                                )
                                .await?;
                            }
                            stats.standalone_keys.updated += 1;
                            if let Some(source_api_key_id) = source_api_key_id.clone() {
                                imported_api_key_id_map
                                    .insert(source_api_key_id, existing_key.api_key_id.clone());
                            }
                        }
                    }
                    continue;
                }

                let api_key_id = imported_api_key_id_for_mode(source_api_key_id.as_deref(), mode);
                let key_encrypted = invalid_value!(self.seal_imported_auth_api_key_secret(
                    key_plaintext.as_deref(),
                    &standalone_owner_id,
                    &api_key_id,
                    &key_hash,
                    true,
                ));
                let created = self
                    .create_standalone_api_key(
                        aether_data::repository::auth::CreateStandaloneApiKeyRecord {
                            user_id: standalone_owner_id.clone(),
                            api_key_id,
                            key_hash: key_hash.clone(),
                            key_encrypted,
                            name,
                            allowed_providers,
                            allowed_api_formats,
                            allowed_models,
                            ip_rules,
                            rate_limit: Some(rate_limit),
                            concurrent_limit,
                            force_capabilities,
                            is_active: mode.preserves_active_state() && is_active,
                            expires_at_unix_secs,
                            auto_delete_on_expiry,
                            total_requests,
                            total_tokens,
                            total_cost_usd,
                        },
                    )
                    .await?;
                let Some(created) = created else {
                    return Ok(Err((
                        http::StatusCode::SERVICE_UNAVAILABLE,
                        json!({ "detail": "Admin system data unavailable" }),
                    )));
                };
                let created_api_key_id = created.api_key_id.clone();
                if let Some(journal) = mutation_journal.as_deref_mut() {
                    journal
                        .standalone_api_key_ids
                        .insert(created_api_key_id.clone());
                }
                if key.contains_key("feature_settings") {
                    let _ = require_persisted!(
                        self.set_standalone_api_key_feature_settings(
                            &created.api_key_id,
                            feature_settings.clone(),
                        )
                        .await?
                    );
                }
                if let Some(wallet_target) = wallet_target.as_ref() {
                    self.sync_imported_api_key_wallet(
                        &created.api_key_id,
                        wallet_target,
                        created.name.as_deref().unwrap_or("独立余额 Key"),
                        mutation_journal.as_deref_mut(),
                    )
                    .await?;
                }
                existing_standalone_by_hash.insert(key_hash, created);
                if let Some(source_api_key_id) = source_api_key_id {
                    imported_api_key_id_map.insert(source_api_key_id, created_api_key_id);
                }
                stats.standalone_keys.created += 1;
            }
        }

        if !mode.is_rollback_checkpoint() {
            if let Some(summary) = self
                .import_admin_system_user_usage_aggregates(
                    root.get("usage_aggregates"),
                    &supplemental_user_usage_aggregates,
                    &imported_user_id_map,
                    &imported_api_key_id_map,
                    merge_mode,
                )
                .await?
            {
                stats.usage_aggregates = Some(summary);
            }
        }

        Ok(Ok(json!({
            "message": "用户数据导入成功",
            "stats": stats,
        })))
    }

    async fn import_admin_system_user_usage_aggregates(
        &self,
        value: Option<&Value>,
        supplemental_user_daily: &[AdminSystemStatsUserDailyAggregate],
        user_id_map: &BTreeMap<String, String>,
        api_key_id_map: &BTreeMap<String, String>,
        merge_mode: AdminImportMergeMode,
    ) -> Result<Option<AdminSystemUsageAggregateImportSummary>, GatewayError> {
        let snapshot = build_imported_usage_aggregate_snapshot(value, supplemental_user_daily)
            .map_err(|message| GatewayError::Client {
                status: http::StatusCode::BAD_REQUEST,
                message,
            })?;
        if snapshot.stats_daily.is_empty()
            && snapshot.stats_user_daily.is_empty()
            && snapshot.stats_daily_api_key.is_empty()
        {
            return Ok(None);
        }
        self.import_admin_system_usage_aggregates(
            &snapshot,
            user_id_map,
            api_key_id_map,
            usage_aggregate_import_mode(merge_mode),
        )
        .await
        .map(Some)
    }

    async fn sync_imported_user_wallet(
        &self,
        user_id: &str,
        wallet_target: &ImportedWalletTarget,
        label: &str,
        mut mutation_journal: Option<&mut AggregateMutationJournal>,
    ) -> Result<(), GatewayError> {
        let initialized = self
            .initialize_auth_user_wallet_with_outcome(user_id, 0.0, false)
            .await?;
        let Some(initialized) = initialized else {
            return Err(GatewayError::Internal(format!(
                "failed to initialize imported wallet for {label}"
            )));
        };
        if initialized.wallet.user_id.as_deref() != Some(user_id)
            || initialized.wallet.api_key_id.is_some()
        {
            return Err(GatewayError::Internal(format!(
                "imported user wallet owner does not match {label}"
            )));
        }
        let created_wallet_id = initialized.created.then(|| initialized.wallet.id.clone());
        let existing_wallet_key =
            (!initialized.created).then(|| (user_id.to_string(), initialized.wallet.id.clone()));
        // Record only a row this invocation actually created. The repository returns this bit
        // from the same atomic operation, so a concurrent initializer's wallet is never treated
        // as import-owned and later deleted during compensation. The initial snapshot is a
        // fallback for a failure before the imported values are persisted; a later successful
        // sync replaces it with the complete post-import snapshot.
        if let Some(wallet_id) = created_wallet_id.as_ref() {
            if let Some(journal) = mutation_journal.as_deref_mut() {
                journal.user_wallet_snapshots.insert(
                    (user_id.to_string(), wallet_id.clone()),
                    initialized.wallet.clone(),
                );
            }
        }
        if let Some(key) = existing_wallet_key.as_ref() {
            if let Some(journal) = mutation_journal.as_deref_mut() {
                journal
                    .existing_user_wallets
                    .entry(key.clone())
                    .or_insert_with(|| ExistingWalletMutation {
                        before: initialized.wallet.clone(),
                        after: None,
                    });
            }
        }
        let synced = self
            .sync_wallet_snapshot(WalletOwner::User(user_id), wallet_target, label)
            .await?;
        if let Some(key) = existing_wallet_key.as_ref() {
            if let Some(journal) = mutation_journal.as_deref_mut() {
                if let Some(mutation) = journal.existing_user_wallets.get_mut(key) {
                    mutation.after = Some(synced.clone());
                }
            }
        }
        if !Self::imported_wallet_snapshot_matches_target(&synced, wallet_target) {
            return Err(GatewayError::Internal(format!(
                "persisted imported wallet snapshot changed during sync for {label}"
            )));
        }
        if let Some(wallet_id) = created_wallet_id {
            if let Some(journal) = mutation_journal {
                journal
                    .user_wallet_snapshots
                    .insert((user_id.to_string(), wallet_id), synced);
            }
        }
        Ok(())
    }

    async fn sync_imported_api_key_wallet(
        &self,
        api_key_id: &str,
        wallet_target: &ImportedWalletTarget,
        label: &str,
        mut mutation_journal: Option<&mut AggregateMutationJournal>,
    ) -> Result<(), GatewayError> {
        let initialized = self
            .initialize_auth_api_key_wallet_with_outcome(api_key_id, 0.0, false)
            .await?;
        let Some(initialized) = initialized else {
            return Err(GatewayError::Internal(format!(
                "failed to initialize imported wallet for {label}"
            )));
        };
        if initialized.wallet.api_key_id.as_deref() != Some(api_key_id)
            || initialized.wallet.user_id.is_some()
        {
            return Err(GatewayError::Internal(format!(
                "imported API-key wallet owner does not match {label}"
            )));
        }
        let created_wallet_id = initialized.created.then(|| initialized.wallet.id.clone());
        let existing_wallet_key =
            (!initialized.created).then(|| (api_key_id.to_string(), initialized.wallet.id.clone()));
        if let Some(wallet_id) = created_wallet_id.as_ref() {
            if let Some(journal) = mutation_journal.as_deref_mut() {
                journal.api_key_wallet_snapshots.insert(
                    (api_key_id.to_string(), wallet_id.clone()),
                    initialized.wallet.clone(),
                );
            }
        }
        if let Some(key) = existing_wallet_key.as_ref() {
            if let Some(journal) = mutation_journal.as_deref_mut() {
                journal
                    .existing_api_key_wallets
                    .entry(key.clone())
                    .or_insert_with(|| ExistingWalletMutation {
                        before: initialized.wallet.clone(),
                        after: None,
                    });
            }
        }
        let synced = self
            .sync_wallet_snapshot(WalletOwner::ApiKey(api_key_id), wallet_target, label)
            .await?;
        if let Some(key) = existing_wallet_key.as_ref() {
            if let Some(journal) = mutation_journal.as_deref_mut() {
                if let Some(mutation) = journal.existing_api_key_wallets.get_mut(key) {
                    mutation.after = Some(synced.clone());
                }
            }
        }
        if !Self::imported_wallet_snapshot_matches_target(&synced, wallet_target) {
            return Err(GatewayError::Internal(format!(
                "persisted imported wallet snapshot changed during sync for {label}"
            )));
        }
        if let Some(wallet_id) = created_wallet_id {
            if let Some(journal) = mutation_journal {
                journal
                    .api_key_wallet_snapshots
                    .insert((api_key_id.to_string(), wallet_id), synced);
            }
        }
        Ok(())
    }

    async fn sync_wallet_snapshot(
        &self,
        owner: WalletOwner<'_>,
        wallet_target: &ImportedWalletTarget,
        label: &str,
    ) -> Result<StoredWalletSnapshot, GatewayError> {
        let updated = match owner {
            WalletOwner::User(user_id) => {
                self.update_auth_user_wallet_snapshot(
                    user_id,
                    wallet_target.recharge_balance,
                    wallet_target.gift_balance,
                    &wallet_target.limit_mode,
                    &wallet_target.currency,
                    &wallet_target.status,
                    wallet_target.total_recharged,
                    wallet_target.total_consumed,
                    wallet_target.total_refunded,
                    wallet_target.total_adjusted,
                    wallet_target.updated_at_unix_secs,
                )
                .await?
            }
            WalletOwner::ApiKey(api_key_id) => {
                self.update_auth_api_key_wallet_snapshot(
                    api_key_id,
                    wallet_target.recharge_balance,
                    wallet_target.gift_balance,
                    &wallet_target.limit_mode,
                    &wallet_target.currency,
                    &wallet_target.status,
                    wallet_target.total_recharged,
                    wallet_target.total_consumed,
                    wallet_target.total_refunded,
                    wallet_target.total_adjusted,
                    wallet_target.updated_at_unix_secs,
                )
                .await?
            }
        };
        let Some(updated) = updated else {
            return Err(GatewayError::Internal(format!(
                "failed to persist imported wallet snapshot for {label}"
            )));
        };
        Ok(updated)
    }

    fn imported_wallet_snapshot_matches_target(
        snapshot: &StoredWalletSnapshot,
        target: &ImportedWalletTarget,
    ) -> bool {
        const AMOUNT_EPSILON_USD: f64 = 0.00000001;
        let amount_matches = |actual: f64, expected: f64| {
            actual.is_finite()
                && expected.is_finite()
                && (actual - expected).abs() <= AMOUNT_EPSILON_USD
        };
        amount_matches(snapshot.balance, target.recharge_balance)
            && amount_matches(snapshot.gift_balance, target.gift_balance)
            && snapshot.limit_mode == target.limit_mode
            && snapshot.currency == target.currency
            && snapshot.status == target.status
            && amount_matches(snapshot.total_recharged, target.total_recharged)
            && amount_matches(snapshot.total_consumed, target.total_consumed)
            && amount_matches(snapshot.total_refunded, target.total_refunded)
            && amount_matches(snapshot.total_adjusted, target.total_adjusted)
            && target
                .updated_at_unix_secs
                .is_none_or(|expected| snapshot.updated_at_unix_secs == expected)
    }

    fn resolve_imported_system_user_api_key_material(
        &self,
        key: &Map<String, Value>,
        users_export_version: (u32, u32),
        mode: SystemImportMode,
    ) -> Result<Option<ImportedApiKeyMaterial>, String> {
        let source_api_key_id = imported_optional_string(key.get("api_key_id"))?;
        let plaintext_key = imported_optional_string(key.get("key"))?;
        let key_hash = imported_optional_string(key.get("key_hash"))?;
        let key_encrypted = imported_optional_string(key.get("key_encrypted"))?;

        if users_export_version >= (1, 6) {
            if key.contains_key("key")
                || key.contains_key("key_hash")
                || key.contains_key("key_encrypted")
            {
                return Err(
                    "用户数据 1.6+ 不允许包含 key、key_hash 或 key_encrypted 凭据字段".to_string(),
                );
            }
            let credential_state = imported_optional_string(key.get("credential_state"))?;
            if credential_state.as_deref() != Some("not_exported") {
                return Err(
                    "用户数据 1.6+ API Key 必须标记 credential_state=not_exported".to_string(),
                );
            }
            let source_api_key_id = source_api_key_id
                .filter(|value| !value.is_empty())
                .ok_or_else(|| "用户数据 1.6+ API Key 必须包含 api_key_id".to_string())?;
            return Ok(Some(ImportedApiKeyMaterial {
                key_hash: imported_credential_tombstone(&format!("api-key-id:{source_api_key_id}")),
                key_plaintext: None,
            }));
        }

        if mode.restores_credentials() {
            let key_hash = key_hash
                .filter(|value| !value.is_empty())
                .ok_or_else(|| "恢复备份中的 API Key 必须包含 key_hash".to_string())?;
            if key_hash.len() != 64
                || !key_hash
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            {
                return Err("恢复备份中的 key_hash 必须是规范的小写 SHA-256 十六进制".to_string());
            }
            if plaintext_key.is_some() && key_encrypted.is_some() {
                return Err("恢复备份中的 API Key 不能同时包含 key 和 key_encrypted".to_string());
            }
            let plaintext = match (plaintext_key, key_encrypted) {
                (Some(plaintext), None) => Some(plaintext),
                (None, Some(ciphertext)) => Some(
                    self.decrypt_catalog_secret_with_fallbacks(&ciphertext)
                        .ok_or_else(|| {
                            "恢复备份中的 API Key 旧密文无法使用当前或历史数据密钥解密".to_string()
                        })?,
                ),
                (None, None) => None,
                (Some(_), Some(_)) => unreachable!(),
            };
            if let Some(plaintext) = plaintext.as_deref() {
                if hash_admin_user_api_key(plaintext) != key_hash {
                    return Err("恢复备份中的 API Key 明文与 key_hash 不匹配".to_string());
                }
            }
            return Ok(Some(ImportedApiKeyMaterial {
                key_hash,
                key_plaintext: plaintext,
            }));
        }

        let identity = source_api_key_id
            .map(|value| format!("api-key-id:{value}"))
            .or_else(|| key_hash.map(|value| format!("legacy-key-hash:{value}")))
            .or_else(|| plaintext_key.map(|value| format!("legacy-key:{value}")))
            .or_else(|| key_encrypted.map(|value| format!("legacy-key-encrypted:{value}")));
        Ok(identity.map(|identity| ImportedApiKeyMaterial {
            key_hash: imported_credential_tombstone(&identity),
            key_plaintext: None,
        }))
    }

    fn seal_imported_auth_api_key_secret(
        &self,
        plaintext: Option<&str>,
        user_id: &str,
        api_key_id: &str,
        key_hash: &str,
        is_standalone: bool,
    ) -> Result<Option<String>, String> {
        plaintext
            .map(|plaintext| {
                seal_auth_api_key_secret(
                    self.app(),
                    user_id,
                    api_key_id,
                    key_hash,
                    is_standalone,
                    plaintext,
                )
                .map_err(|_| "gateway 无法为目的 API Key 记录加密恢复凭据".to_string())
            })
            .transpose()
    }
}

#[derive(Clone, Copy)]
enum WalletOwner<'a> {
    User(&'a str),
    ApiKey(&'a str),
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use aether_data::repository::pool_scores::PostgresPoolMemberScoreRepository;
    use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
    use aether_data_contracts::repository::provider_catalog::{
        StoredProviderCatalogKey, StoredProviderCatalogProvider,
    };
    use serde_json::json;

    use super::{
        apply_imported_portable_provider_access, build_imported_user_usage_total_aggregates,
        imported_api_key_id_for_mode, imported_credential_tombstone,
        imported_existing_user_is_protected, imported_oauth_auth_config_has_credentials,
        imported_oauth_expiry_after_import, imported_optional_bool, imported_optional_f64,
        imported_optional_i32, imported_optional_u64, imported_rfc3339_to_unix_secs,
        imported_string_list_from_value, normalize_import_endpoint_format,
        normalize_import_key_formats, normalize_import_key_raw_payload,
        normalize_imported_system_user_role, normalize_imported_wallet_target,
        prepare_imported_secret_safe_body_rules, prepare_imported_secret_safe_header_rules,
        prepare_imported_secret_safe_json, resolve_imported_password_hash,
        seed_imported_oauth_pool_score, validate_imported_provider_key_credential_state,
        validate_imported_system_users_export_version,
        validate_imported_system_users_export_version_for_mode, ImportedProviderKey,
        SystemImportMode,
    };
    use crate::admin_api::AdminAppState;
    use crate::data::GatewayDataState;
    use crate::AppState;

    #[test]
    fn users_import_requires_supported_export_version() {
        assert!(validate_imported_system_users_export_version(Some(&json!("1.3"))).is_ok());
        assert!(validate_imported_system_users_export_version(Some(&json!("1.4"))).is_ok());
        assert!(validate_imported_system_users_export_version(Some(&json!("1.5"))).is_ok());
        assert!(validate_imported_system_users_export_version(Some(&json!("1.6"))).is_ok());
        assert!(validate_imported_system_users_export_version(Some(&json!("1.7"))).is_ok());
        assert_eq!(
            validate_imported_system_users_export_version(Some(&json!("2.2"))).unwrap_err(),
            "不支持的用户数据版本: 2.2，支持的版本: 1.3, 1.4, 1.5, 1.6, 1.7"
        );
        assert_eq!(
            validate_imported_system_users_export_version(Some(&json!(null))).unwrap_err(),
            "version 必须是 x.y 字符串"
        );
    }

    fn imported_group_record_for_provider_access(
    ) -> aether_data::repository::users::UpsertUserGroupRecord {
        aether_data::repository::users::UpsertUserGroupRecord {
            name: "pro".to_string(),
            description: None,
            priority: 0,
            allowed_providers: Some(vec!["source-provider".to_string()]),
            allowed_providers_mode: "specific".to_string(),
            provider_key_policies: BTreeMap::from([(
                "source-provider".to_string(),
                vec!["source-key".to_string()],
            )]),
            allowed_api_formats: None,
            allowed_api_formats_mode: "unrestricted".to_string(),
            allowed_models: None,
            allowed_models_mode: "unrestricted".to_string(),
            rate_limit: None,
            rate_limit_mode: "system".to_string(),
        }
    }

    #[test]
    fn portable_group_access_falls_back_to_unique_provider_and_key_names() {
        let provider = StoredProviderCatalogProvider::new(
            "target-provider".to_string(),
            "tiered-provider".to_string(),
            None,
            "custom".to_string(),
        )
        .expect("provider should build");
        let key = StoredProviderCatalogKey::new(
            "target-key".to_string(),
            provider.id.clone(),
            "企业 Pro".to_string(),
            "api_key".to_string(),
            None,
            true,
        )
        .expect("key should build");
        let group = json!({
            "allowed_provider_refs": [
                { "id": "source-provider", "name": "tiered-provider" }
            ],
            "provider_key_policy_refs": [{
                "provider": { "id": "source-provider", "name": "tiered-provider" },
                "keys": [{ "id": "source-key", "name": "企业 Pro" }]
            }]
        });

        let record = apply_imported_portable_provider_access(
            group.as_object().expect("group should be object"),
            "user_groups[0]",
            imported_group_record_for_provider_access(),
            std::slice::from_ref(&provider),
            std::slice::from_ref(&key),
        )
        .expect("portable references should resolve by name");

        assert_eq!(
            record.allowed_providers,
            Some(vec!["target-provider".to_string()])
        );
        assert_eq!(
            record.provider_key_policies,
            BTreeMap::from([(
                "target-provider".to_string(),
                vec!["target-key".to_string()],
            )])
        );
    }

    #[test]
    fn portable_group_access_rejects_unresolved_key_instead_of_widening_access() {
        let provider = StoredProviderCatalogProvider::new(
            "target-provider".to_string(),
            "tiered-provider".to_string(),
            None,
            "custom".to_string(),
        )
        .expect("provider should build");
        let group = json!({
            "allowed_provider_refs": [
                { "id": "source-provider", "name": "tiered-provider" }
            ],
            "provider_key_policy_refs": [{
                "provider": { "id": "source-provider", "name": "tiered-provider" },
                "keys": [{ "id": "source-key", "name": "不存在" }]
            }]
        });

        let error = apply_imported_portable_provider_access(
            group.as_object().expect("group should be object"),
            "user_groups[0]",
            imported_group_record_for_provider_access(),
            std::slice::from_ref(&provider),
            &[],
        )
        .expect_err("unresolved references must reject the import");

        assert!(error.contains("Provider Key 不存在"));
    }

    #[test]
    fn recovery_users_import_requires_v15_and_valid_bcrypt() {
        assert_eq!(
            validate_imported_system_users_export_version_for_mode(
                Some(&json!("1.5")),
                SystemImportMode::RecoveryBackup,
            ),
            Ok((1, 5)),
        );
        assert!(validate_imported_system_users_export_version_for_mode(
            Some(&json!("1.4")),
            SystemImportMode::RecoveryBackup,
        )
        .is_err());
        assert!(resolve_imported_password_hash(
            json!({ "password_hash": "attacker-controlled" })
                .as_object()
                .expect("fixture should be an object"),
            (1, 5),
            SystemImportMode::RecoveryBackup,
        )
        .is_err());
    }

    #[test]
    fn system_user_role_interactive_import_cannot_assign_admin_console_roles() {
        assert_eq!(
            normalize_imported_system_user_role(None, SystemImportMode::InteractiveUpload),
            Ok(Some("user".to_string()))
        );
        assert_eq!(
            normalize_imported_system_user_role(
                Some(&json!(" ADMIN ")),
                SystemImportMode::InteractiveUpload,
            ),
            Ok(None)
        );
        assert_eq!(
            normalize_imported_system_user_role(
                Some(&json!("audit_admin")),
                SystemImportMode::InteractiveUpload,
            ),
            Ok(None)
        );
        assert_eq!(
            normalize_imported_system_user_role(
                Some(&json!("audit_admin")),
                SystemImportMode::RollbackCheckpoint,
            ),
            Ok(None)
        );
        assert!(normalize_imported_system_user_role(
            Some(&json!("owner")),
            SystemImportMode::InteractiveUpload,
        )
        .expect_err("unknown roles must be rejected before import writes")
        .contains("不支持的用户角色"));
    }

    #[test]
    fn system_user_role_authenticated_recovery_restores_only_audit_admin() {
        for mode in [
            SystemImportMode::RecoveryBackup,
            SystemImportMode::RecoveryRollbackCheckpoint,
        ] {
            assert_eq!(
                normalize_imported_system_user_role(Some(&json!("audit_admin")), mode),
                Ok(Some("audit_admin".to_string()))
            );
            assert_eq!(
                normalize_imported_system_user_role(Some(&json!("admin")), mode),
                Ok(None)
            );
        }
    }

    #[test]
    fn system_user_role_ordinary_import_protects_existing_admin_console_users() {
        for mode in [
            SystemImportMode::InteractiveUpload,
            SystemImportMode::RollbackCheckpoint,
        ] {
            assert!(imported_existing_user_is_protected("admin", mode));
            assert!(imported_existing_user_is_protected("audit_admin", mode));
            assert!(!imported_existing_user_is_protected("user", mode));
        }

        assert!(imported_existing_user_is_protected(
            "admin",
            SystemImportMode::RecoveryBackup,
        ));
        assert!(!imported_existing_user_is_protected(
            "audit_admin",
            SystemImportMode::RecoveryBackup,
        ));
        assert!(!imported_existing_user_is_protected(
            "audit_admin",
            SystemImportMode::RecoveryRollbackCheckpoint,
        ));
    }

    #[test]
    fn rollback_checkpoint_body_forces_overwrite_and_omits_local_proxy_nodes() {
        let checkpoint = json!({
            "version": "2.3",
            "credential_state": "not_exported",
            "proxy_nodes": [{"id": "local-node"}],
            "system_configs": []
        });
        let body = super::build_aggregate_rollback_body(&checkpoint, true)
            .expect("rollback body should serialize");
        let parsed: serde_json::Value = serde_json::from_slice(&body).expect("body is JSON");

        assert_eq!(parsed["merge_mode"], json!("overwrite"));
        assert_eq!(parsed["proxy_nodes"], json!([]));
        assert_eq!(parsed["credential_state"], json!("not_exported"));
    }

    #[test]
    fn rollback_checkpoint_can_skip_ldap_without_dropping_other_config_sections() {
        let checkpoint = json!({
            "version": "2.3",
            "ldap_config": {
                "server_url": "ldaps://checkpoint.example.test",
                "bind_dn": "cn=admin,dc=example,dc=test",
                "base_dn": "dc=example,dc=test"
            },
            "system_configs": [{"key": "module.example.enabled", "value": true}],
        });
        let body = super::build_aggregate_rollback_body_with_options(&checkpoint, true, true)
            .expect("rollback body should serialize");
        let parsed: serde_json::Value = serde_json::from_slice(&body).expect("body is JSON");

        assert!(parsed.get("ldap_config").is_none());
        assert_eq!(parsed["system_configs"], checkpoint["system_configs"]);
        assert_eq!(parsed["merge_mode"], json!("overwrite"));
        assert_eq!(parsed["proxy_nodes"], json!([]));
    }

    #[test]
    fn rollback_checkpoint_body_rejects_non_object() {
        let error = super::build_aggregate_rollback_body(&json!([1, 2, 3]), false)
            .expect_err("non-object checkpoint must be rejected");
        assert!(error
            .into_message()
            .contains("checkpoint must be a JSON object"));
    }

    #[test]
    fn aggregate_users_rollback_body_excludes_all_wallet_snapshots() {
        let checkpoint = json!({
            "version": "1.5",
            "users": [{
                "id": "user-1",
                "username": "checkpoint-user",
                "request_count": 100,
                "total_tokens": 200,
                "wallet": {"balance": 10.0},
                "api_keys": [{
                    "api_key_id": "key-1",
                    "name": "user key",
                    "total_requests": 101,
                    "total_tokens": 201,
                    "total_cost_usd": 1.25,
                    "wallet": {"balance": 20.0}
                }]
            }],
            "standalone_keys": [{
                "api_key_id": "standalone-1",
                "name": "standalone key",
                "total_requests": 102,
                "total_tokens": 202,
                "total_cost_usd": 2.5,
                "wallet": {"balance": 30.0}
            }],
            "usage_aggregates": {
                "stats_daily": [{"date_unix_secs": 1}]
            }
        });

        let body = super::build_aggregate_users_rollback_body(&checkpoint)
            .expect("users rollback body should serialize");
        let parsed: serde_json::Value = serde_json::from_slice(&body).expect("body is JSON");

        assert_eq!(parsed["merge_mode"], json!("overwrite"));
        assert_eq!(parsed["users"][0]["username"], json!("checkpoint-user"));
        assert!(parsed["users"][0].get("request_count").is_none());
        assert!(parsed["users"][0].get("total_tokens").is_none());
        assert!(parsed["users"][0].get("wallet").is_none());
        assert!(parsed["users"][0]["api_keys"][0].get("wallet").is_none());
        assert!(parsed["users"][0]["api_keys"][0]
            .get("total_requests")
            .is_none());
        assert!(parsed["users"][0]["api_keys"][0]
            .get("total_tokens")
            .is_none());
        assert!(parsed["users"][0]["api_keys"][0]
            .get("total_cost_usd")
            .is_none());
        assert!(parsed["standalone_keys"][0].get("wallet").is_none());
        assert!(parsed["standalone_keys"][0].get("total_requests").is_none());
        assert!(parsed["standalone_keys"][0].get("total_tokens").is_none());
        assert!(parsed["standalone_keys"][0].get("total_cost_usd").is_none());
        assert!(parsed.get("usage_aggregates").is_none());
    }

    #[test]
    fn imported_api_key_tombstones_fit_legacy_columns_and_cannot_authenticate() {
        use sha2::{Digest, Sha256};

        let identity = "api-key-id:public-source-key-id";
        let tombstone = imported_credential_tombstone(identity);
        let normal_auth_hash = format!("{:x}", Sha256::digest(identity.as_bytes()));

        assert_eq!(tombstone.len(), 64);
        assert!(tombstone.starts_with("$aether-import-revoked$"));
        assert!(!tombstone
            .chars()
            .all(|character| character.is_ascii_hexdigit()));
        assert_ne!(tombstone, normal_auth_hash);
    }

    #[test]
    fn rollback_checkpoint_reuses_api_key_id_while_interactive_imports_rotate_it() {
        let source_id = Some("checkpoint-api-key-id");
        let rollback_id =
            imported_api_key_id_for_mode(source_id, SystemImportMode::RollbackCheckpoint);
        assert_eq!(rollback_id, "checkpoint-api-key-id");

        let interactive_id =
            imported_api_key_id_for_mode(source_id, SystemImportMode::InteractiveUpload);
        assert_ne!(interactive_id, "checkpoint-api-key-id");
        assert!(!interactive_id.is_empty());
    }

    #[test]
    fn recovery_rollback_checkpoint_keeps_credentials_and_stable_ids() {
        let mode = SystemImportMode::RecoveryRollbackCheckpoint;
        assert!(mode.restores_credentials());
        assert!(mode.preserves_active_state());
        assert!(mode.is_rollback_checkpoint());
        assert_eq!(
            imported_api_key_id_for_mode(Some("recovery-key-id"), mode),
            "recovery-key-id"
        );
        assert!(!SystemImportMode::InteractiveUpload.restores_credentials());
    }

    #[test]
    fn rollback_checkpoint_requires_stable_user_id_instead_of_identifier_guessing() {
        assert!(super::validate_rollback_user_source_id(
            SystemImportMode::RollbackCheckpoint,
            None,
        )
        .expect_err("rollback without a source ID must be rejected")
        .contains("拒绝按 email/username 猜测用户"));
        assert!(super::validate_rollback_user_source_id(
            SystemImportMode::RecoveryRollbackCheckpoint,
            None,
        )
        .is_err());
        assert!(super::validate_rollback_user_source_id(
            SystemImportMode::RollbackCheckpoint,
            Some("stable-user-id"),
        )
        .is_ok());
        assert!(
            super::validate_rollback_user_source_id(SystemImportMode::InteractiveUpload, None,)
                .is_ok()
        );
    }

    #[test]
    fn users_import_builds_supplemental_usage_aggregates_from_summary_fields() {
        let users = vec![
            json!({
                "id": "source-user-1",
                "username": "alice",
                "request_count": 12,
                "total_tokens": 3456
            }),
            json!({
                "id": "source-user-zero",
                "username": "zero",
                "request_count": 0,
                "total_tokens": 0
            }),
            json!({
                "username": "no-source-id",
                "request_count": 5,
                "total_tokens": 6
            }),
        ];

        let rows = build_imported_user_usage_total_aggregates(
            &users,
            Some(&json!("2026-05-25T12:34:56Z")),
        )
        .expect("supplemental usage aggregates should build");

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].user_id, "source-user-1");
        assert_eq!(rows[0].username.as_deref(), Some("alice"));
        assert_eq!(rows[0].total_requests, 12);
        assert_eq!(rows[0].success_requests, 12);
        assert_eq!(rows[0].input_tokens, 3456);
        assert_eq!(rows[0].date_unix_secs % 86_400, 0);
    }

    #[test]
    fn config_import_normalizes_python_cli_api_format_aliases() {
        for (raw, expected) in [
            ("openai:cli", "openai:responses"),
            ("openai:compact", "openai:responses:compact"),
            ("openai_image", "openai:image"),
            ("images", "openai:image"),
            ("/v1/images/generations", "openai:image"),
            ("/v1/images/edits", "openai:image"),
            ("claude:chat", "claude:messages"),
            ("claude:cli", "claude:messages"),
            ("gemini:chat", "gemini:generate_content"),
            ("gemini:cli", "gemini:generate_content"),
        ] {
            assert_eq!(normalize_import_endpoint_format(raw).unwrap(), expected);
        }
    }

    #[test]
    fn config_import_normalizes_key_formats_against_imported_endpoint_aliases() {
        let endpoint_formats = ["claude:messages", "openai:responses:compact"]
            .into_iter()
            .map(ToOwned::to_owned)
            .collect();
        let item = ImportedProviderKey {
            api_key: None,
            auth_type: None,
            auth_config: None,
            name: None,
            note: None,
            api_formats: Some(vec!["claude:cli".to_string(), "openai:compact".to_string()]),
            supported_endpoints: None,
            rate_multipliers: None,
            internal_priority: None,
            global_priority_by_format: None,
            auth_type_by_format: None,
            allow_auth_channel_mismatch_formats: None,
            rpm_limit: None,
            allowed_models: None,
            capabilities: None,
            cache_ttl_minutes: None,
            max_probe_interval_minutes: None,
            auto_fetch_models: None,
            locked_models: None,
            model_include_patterns: None,
            model_exclude_patterns: None,
            is_active: true,
            proxy: None,
            fingerprint: None,
            credential_state: None,
        };

        let (formats, missing) = normalize_import_key_formats(&item, &endpoint_formats);

        assert_eq!(formats, vec!["claude:messages", "openai:responses:compact"]);
        assert!(missing.is_empty());
    }

    #[test]
    fn config_import_filters_key_format_scoped_fields_to_selected_api_formats() {
        let raw_key = json!({
            "name": "test-key",
            "api_key": "sk-test",
            "api_formats": ["openai:responses", "openai:video"],
            "auth_type_by_format": {
                "openai:responses": "api_key",
                "openai:video": "bearer"
            },
            "allow_auth_channel_mismatch_formats": [
                "openai:responses",
                "openai:video"
            ]
        });
        let raw_key = raw_key.as_object().expect("key should be object");

        let payload = normalize_import_key_raw_payload(
            raw_key,
            "api_key",
            &["openai:responses".to_string()],
            None,
            false,
        );

        assert_eq!(payload["api_formats"], json!(["openai:responses"]));
        assert_eq!(
            payload["auth_type_by_format"],
            json!({ "openai:responses": "api_key" })
        );
        assert_eq!(
            payload["allow_auth_channel_mismatch_formats"],
            json!(["openai:responses"])
        );
    }

    #[test]
    fn config_import_preserves_explicit_empty_mismatch_scope_after_filtering() {
        let raw_key = json!({
            "name": "test-key",
            "api_key": "sk-test",
            "api_formats": ["openai:responses"],
            "allow_auth_channel_mismatch_formats": ["openai:video"]
        });
        let raw_key = raw_key.as_object().expect("key should be object");

        let payload = normalize_import_key_raw_payload(
            raw_key,
            "api_key",
            &["openai:responses".to_string()],
            None,
            false,
        );

        assert_eq!(payload["allow_auth_channel_mismatch_formats"], json!([]));
    }

    #[test]
    fn config_import_never_turns_redaction_markers_into_credentials() {
        let mut item = ImportedProviderKey {
            api_key: None,
            auth_type: Some("api_key".to_string()),
            auth_config: None,
            name: Some("primary".to_string()),
            note: None,
            api_formats: Some(vec!["openai:chat".to_string()]),
            supported_endpoints: None,
            rate_multipliers: None,
            internal_priority: None,
            global_priority_by_format: None,
            auth_type_by_format: None,
            allow_auth_channel_mismatch_formats: None,
            rpm_limit: None,
            allowed_models: None,
            capabilities: None,
            cache_ttl_minutes: None,
            max_probe_interval_minutes: None,
            auto_fetch_models: None,
            locked_models: None,
            model_include_patterns: None,
            model_exclude_patterns: None,
            is_active: false,
            proxy: None,
            fingerprint: None,
            credential_state: Some("not_exported".to_string()),
        };

        assert!(validate_imported_provider_key_credential_state(&item).unwrap());
        item.api_key = Some("***".to_string());
        assert!(validate_imported_provider_key_credential_state(&item).is_err());
        item.api_key = None;
        item.credential_state = Some("unknown".to_string());
        assert!(validate_imported_provider_key_credential_state(&item).is_err());
    }

    #[test]
    fn config_import_restores_existing_secret_safe_json_and_strips_new_placeholders() {
        let existing = json!({
            "credentials": {"refresh_token": "old-refresh"},
            "region": "us-east-1"
        });
        let incoming = json!({
            "credentials": "***",
            "region": "eu-west-1"
        });

        let restored =
            prepare_imported_secret_safe_json(Some(&existing), Some(incoming.clone()), true)
                .expect("existing config should restore");
        assert_eq!(
            restored["credentials"],
            json!({"refresh_token": "old-refresh"})
        );
        assert_eq!(restored["region"], "eu-west-1");

        let created = prepare_imported_secret_safe_json(None, Some(incoming), true)
            .expect("safe config should remain");
        assert!(created.get("credentials").is_none());
        assert_eq!(created["region"], "eu-west-1");
    }

    #[test]
    fn config_import_restores_matching_endpoint_rules_without_persisting_markers() {
        let existing_headers = json!([{
            "action": "set",
            "key": "Authorization",
            "value": "Bearer old-secret"
        }]);
        let incoming_headers = json!([{
            "action": "set",
            "key": "Authorization",
            "value": "***",
            "has_value": true
        }]);
        let restored_headers = prepare_imported_secret_safe_header_rules(
            Some(&existing_headers),
            Some(incoming_headers),
            true,
        )
        .expect("matching header rule should remain");
        assert_eq!(restored_headers[0]["value"], "Bearer old-secret");
        assert!(restored_headers[0].get("has_value").is_none());

        let existing_body = json!([{
            "action": "set",
            "path": "$.credentials.token",
            "value": "old-body-secret"
        }]);
        let incoming_body = json!([{
            "action": "set",
            "path": "$.credentials.token",
            "value": "***",
            "has_value": true
        }]);
        let restored_body = prepare_imported_secret_safe_body_rules(
            Some(&existing_body),
            Some(incoming_body),
            true,
        )
        .expect("matching body rule should remain");
        assert_eq!(restored_body[0]["value"], "old-body-secret");
        assert!(restored_body[0].get("has_value").is_none());
    }

    #[test]
    fn config_import_drops_unrecoverable_endpoint_rule_placeholders() {
        let incoming_headers = json!([{
            "action": "set",
            "key": "Authorization",
            "value": "***",
            "has_value": true
        }]);
        assert_eq!(
            prepare_imported_secret_safe_header_rules(None, Some(incoming_headers.clone()), true),
            Some(json!([]))
        );
        let markerless_placeholder = json!([{
            "action": "set",
            "key": "Authorization",
            "value": "***"
        }]);
        assert_eq!(
            prepare_imported_secret_safe_header_rules(None, Some(markerless_placeholder), true,),
            Some(json!([]))
        );

        let different_existing = json!([{
            "action": "set",
            "key": "X-Different",
            "value": "must-not-move"
        }]);
        assert_eq!(
            prepare_imported_secret_safe_header_rules(
                Some(&different_existing),
                Some(incoming_headers),
                true,
            ),
            Some(json!([]))
        );

        let incoming_body = json!([{
            "action": "regex_replace",
            "path": "$.credentials.token",
            "pattern": "***",
            "replacement": "***",
            "has_pattern": true,
            "has_replacement": true
        }]);
        assert_eq!(
            prepare_imported_secret_safe_body_rules(None, Some(incoming_body), true),
            Some(json!([]))
        );
    }

    #[test]
    fn oauth_import_only_treats_non_empty_secret_fields_as_credentials() {
        assert!(!imported_oauth_auth_config_has_credentials(&json!({})));
        assert!(!imported_oauth_auth_config_has_credentials(&json!({
            "provider_type": "codex",
            "expires_at": 4_102_444_800u64,
            "account_id": "acct-1",
            "refresh_token": "  "
        })));
        assert!(imported_oauth_auth_config_has_credentials(&json!({
            "provider_type": "codex",
            "refresh_token": "refresh-1"
        })));
        assert!(imported_oauth_auth_config_has_credentials(&json!({
            "session": {"sso_token": "sso-1"}
        })));
        for field in [
            "sso_rw_token",
            "ssoRwToken",
            "cf_cookies",
            "cfCookies",
            "cf_clearance",
            "cfClearance",
            "cookieHeader",
        ] {
            let mut config = serde_json::Map::new();
            config.insert(field.to_string(), json!("credential-1"));
            assert!(
                imported_oauth_auth_config_has_credentials(&serde_json::Value::Object(config)),
                "{field} is transport credential material"
            );
        }
    }

    #[test]
    fn oauth_import_expiry_tracks_the_supplied_credential_source() {
        let old_expiry = Some(1_700_000_000);
        assert_eq!(
            imported_oauth_expiry_after_import(old_expiry, false, None, true),
            None,
            "a new top-level api_key replaces the old session and clears its expiry"
        );
        assert_eq!(
            imported_oauth_expiry_after_import(old_expiry, false, None, false),
            old_expiry,
            "metadata-only imports preserve the current OAuth expiry"
        );
        assert_eq!(
            imported_oauth_expiry_after_import(
                old_expiry,
                true,
                Some(&json!({"expires_at": 4_102_444_800u64})),
                false,
            ),
            Some(4_102_444_800),
            "an explicit auth_config owns the replacement expiry"
        );
    }

    #[tokio::test]
    async fn oauth_pool_score_persistence_failure_is_propagated() {
        let mut provider = StoredProviderCatalogProvider::new(
            "provider-1".to_string(),
            "Provider One".to_string(),
            None,
            "codex".to_string(),
        )
        .expect("provider should build");
        provider.config = Some(json!({"pool_advanced": {}}));
        let key = StoredProviderCatalogKey::new(
            "key-1".to_string(),
            provider.id.clone(),
            "OAuth Key".to_string(),
            "oauth".to_string(),
            None,
            true,
        )
        .expect("key should build");
        let provider_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![provider],
            Vec::new(),
            vec![key.clone()],
        ));
        let no_writer_app = AppState::new()
            .expect("app state should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(Arc::clone(
                    &provider_repository,
                )),
            );
        seed_imported_oauth_pool_score(&AdminAppState::new(&no_writer_app), "provider-1", &key, 99)
            .await
            .expect("a disabled score writer remains an allowed no-op");

        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect_lazy("postgres://localhost/aether")
            .expect("postgres pool should build");
        let score_repository = Arc::new(PostgresPoolMemberScoreRepository::new(pool.clone()));
        pool.close().await;
        let app = AppState::new()
            .expect("app state should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(provider_repository)
                    .with_pool_score_repository_for_tests(score_repository),
            );

        let error =
            seed_imported_oauth_pool_score(&AdminAppState::new(&app), "provider-1", &key, 100)
                .await
                .expect_err("closed pool must fail OAuth score recovery");
        assert!(error
            .into_message()
            .contains("failed to recover OAuth pool score for key 'key-1'"));
    }

    #[test]
    fn import_handles_legacy_string_scalars() {
        assert_eq!(
            imported_optional_bool(Some(&json!("true"))).unwrap_err(),
            "字段必须是布尔值"
        );
        assert_eq!(
            imported_optional_i32(Some(&json!("5")), "rate_limit").unwrap_err(),
            "rate_limit 必须是整数"
        );
        assert_eq!(
            imported_optional_u64(Some(&json!("5")), "total_requests").unwrap_err(),
            "total_requests 必须是非负整数"
        );
        assert_eq!(
            imported_optional_f64(Some(&json!("1.25000000")), "total_cost_usd").unwrap(),
            Some(1.25)
        );
        assert_eq!(
            imported_optional_f64(Some(&json!("not-a-number")), "total_cost_usd").unwrap_err(),
            "total_cost_usd 必须是有限数值"
        );
    }

    #[test]
    fn import_handles_python_isoformat_timestamps() {
        assert_eq!(
            imported_rfc3339_to_unix_secs(Some(&json!("2099-01-01T00:00:00+00:00")), "expires_at")
                .unwrap(),
            Some(4_070_908_800)
        );
        assert_eq!(
            imported_rfc3339_to_unix_secs(Some(&json!("2099-01-01T00:00:00")), "expires_at")
                .unwrap(),
            Some(4_070_908_800)
        );
        assert_eq!(
            imported_rfc3339_to_unix_secs(Some(&json!("invalid")), "expires_at").unwrap_err(),
            "expires_at 必须是 RFC3339 时间"
        );
    }

    #[test]
    fn import_preserves_python_wallet_negative_recharge_balance() {
        let wallet = json!({
            "balance": -4.5,
            "recharge_balance": -5.25,
            "gift_balance": 0.75,
            "limit_mode": "finite"
        });
        let wallet = wallet.as_object().expect("wallet should be object");

        let target = normalize_imported_wallet_target(Some(wallet), false).unwrap();
        assert_eq!(target.recharge_balance, -5.25);
        assert_eq!(target.gift_balance, 0.75);
        assert_eq!(target.total_recharged, 0.0);
    }

    #[test]
    fn import_preserves_python_wallet_negative_balance_fallback() {
        let wallet = json!({
            "balance": -4.5,
            "gift_balance": 0.75,
            "limit_mode": "finite"
        });
        let wallet = wallet.as_object().expect("wallet should be object");

        let target = normalize_imported_wallet_target(Some(wallet), false).unwrap();
        assert_eq!(target.recharge_balance, -5.25);
        assert_eq!(target.gift_balance, 0.75);
    }

    #[test]
    fn import_rejects_negative_wallet_history_totals() {
        for (field, value) in [
            ("total_recharged", -1.0),
            ("total_consumed", -1.0),
            ("total_refunded", -1.0),
        ] {
            let mut wallet = serde_json::Map::new();
            wallet.insert(field.to_string(), json!(value));
            let error = normalize_imported_wallet_target(Some(&wallet), false)
                .expect_err("negative wallet history total must be rejected");
            assert!(error.contains(field), "error should identify {field}");
            assert!(
                error.contains("非负"),
                "error should require non-negative {field}"
            );
        }
    }

    #[test]
    fn import_allows_signed_wallet_adjustment_total() {
        let wallet = json!({"total_adjusted": -3.5});
        let wallet = wallet.as_object().expect("wallet should be object");
        let target = normalize_imported_wallet_target(Some(wallet), false)
            .expect("signed adjustment history should remain valid");
        assert_eq!(target.total_adjusted, -3.5);
    }

    #[test]
    fn import_rejects_legacy_string_lists() {
        assert_eq!(
            imported_string_list_from_value(Some(&json!("openai")), "allowed_providers")
                .unwrap_err(),
            "allowed_providers 必须是字符串列表"
        );
        assert_eq!(
            imported_string_list_from_value(
                Some(&json!("[\"openai:chat\"]")),
                "allowed_api_formats"
            )
            .unwrap_err(),
            "allowed_api_formats 必须是字符串列表"
        );
    }
}
