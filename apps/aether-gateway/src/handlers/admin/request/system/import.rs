use super::AdminAppState;
use crate::api::ai::admin_endpoint_signature_parts;
use crate::handlers::admin::provider::endpoints_admin::payloads::AdminProviderEndpointUpdatePatch;
use crate::handlers::admin::provider::shared::payloads::{
    AdminProviderCreateRequest, AdminProviderKeyCreateRequest, AdminProviderKeyUpdatePatch,
    AdminProviderUpdatePatch,
};
use crate::handlers::admin::shared::{
    normalize_json_array, normalize_json_object, normalize_string_list,
};
use crate::handlers::admin::system::shared::configs::apply_admin_system_config_update;
use crate::handlers::admin::users::{
    hash_admin_user_api_key, normalize_admin_user_api_formats, normalize_admin_user_string_list,
};
use crate::handlers::public::normalize_admin_base_url;
use crate::GatewayError;
use aether_admin::provider::endpoints as admin_provider_endpoints_pure;
use aether_admin::provider::models_write as admin_provider_models_write_pure;
use aether_admin::system::{
    normalize_admin_system_config_key, parse_admin_system_config_array,
    parse_admin_system_config_import_request, parse_admin_system_config_nested_array,
    parse_admin_system_config_optional_object, AdminImportMergeMode,
    AdminSystemConfigEndpoint as ImportedEndpoint, AdminSystemConfigEntry as ImportedSystemConfig,
    AdminSystemConfigGlobalModel as ImportedGlobalModel, AdminSystemConfigImportCounter,
    AdminSystemConfigImportStats, AdminSystemConfigLdap as ImportedLdapConfig,
    AdminSystemConfigOAuthProvider as ImportedOAuthProvider,
    AdminSystemConfigProvider as ImportedProvider,
    AdminSystemConfigProviderKey as ImportedProviderKey,
    AdminSystemConfigProviderModel as ImportedProviderModel,
    AdminSystemConfigProxyNode as ImportedProxyNode,
    ADMIN_SYSTEM_PROVIDER_OPS_SENSITIVE_CREDENTIAL_FIELDS,
};
use aether_data::repository::auth_modules::StoredLdapModuleConfig;
use aether_data::repository::oauth_providers::{
    EncryptedSecretUpdate, UpsertOAuthProviderConfigRecord,
};
use aether_data::repository::wallet::WalletLookupKey;
use aether_data_contracts::repository::global_models::{
    AdminGlobalModelListQuery, AdminProviderModelListQuery, CreateAdminGlobalModelRecord,
    UpdateAdminGlobalModelRecord, UpsertAdminProviderModelRecord,
};
use axum::{body::Bytes, http};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

const ADMIN_SYSTEM_IMPORT_MAX_SIZE_BYTES: usize = 10 * 1024 * 1024;
const MIN_ADMIN_SYSTEM_IMPORT_VERSION: (u32, u32) = (2, 2);

fn invalid_request(detail: impl Into<String>) -> (http::StatusCode, Value) {
    (
        http::StatusCode::BAD_REQUEST,
        json!({ "detail": detail.into() }),
    )
}

fn trim_required(value: &str, field_name: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("{field_name} 不能为空"));
    }
    Ok(trimmed.to_string())
}

fn normalize_optional_price(value: Option<f64>, field_name: &str) -> Result<Option<f64>, String> {
    admin_provider_models_write_pure::normalize_optional_price(value, field_name)
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

fn encrypt_imported_provider_config(
    state: &AdminAppState<'_>,
    config: Option<Value>,
) -> Result<Option<Value>, String> {
    let Some(mut config) = normalize_json_object(config, "config")? else {
        return Ok(None);
    };
    let Some(credentials) = config
        .get_mut("provider_ops")
        .and_then(Value::as_object_mut)
        .and_then(|provider_ops| provider_ops.get_mut("connector"))
        .and_then(Value::as_object_mut)
        .and_then(|connector| connector.get_mut("credentials"))
        .and_then(Value::as_object_mut)
    else {
        return Ok(Some(config));
    };

    for field in ADMIN_SYSTEM_PROVIDER_OPS_SENSITIVE_CREDENTIAL_FIELDS {
        let Some(Value::String(raw)) = credentials.get_mut(*field) else {
            continue;
        };
        if raw.is_empty() {
            continue;
        }
        let encrypted = state
            .encrypt_catalog_secret_with_fallbacks(raw)
            .ok_or_else(|| "gateway 未配置 Provider Ops 加密密钥".to_string())?;
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
    admin_endpoint_signature_parts(value)
        .map(|(signature, _, _)| signature.to_string())
        .ok_or_else(|| format!("无效的 api_format: {value}"))
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

fn build_import_key_match_name(item: &ImportedProviderKey) -> Option<String> {
    item.name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn normalize_import_key_raw_payload(
    raw_key: &Map<String, Value>,
    auth_type: &str,
    normalized_api_formats: &[String],
    normalized_auth_config: Option<Value>,
) -> Map<String, Value> {
    let mut payload = raw_key.clone();
    if auth_type == "oauth" {
        payload.remove("api_key");
    }
    payload.insert("api_formats".to_string(), json!(normalized_api_formats));
    if let Some(auth_config) = normalized_auth_config {
        payload.insert("auth_config".to_string(), auth_config);
    } else if raw_key.contains_key("auth_config") {
        payload.insert("auth_config".to_string(), Value::Null);
    }
    payload
}

fn apply_imported_oauth_key_credentials(
    state: &AdminAppState<'_>,
    raw_key: &Map<String, Value>,
    normalized_auth_config: Option<&Value>,
    record: &mut aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey,
) -> Result<(), String> {
    if let Some(api_key_value) = raw_key.get("api_key") {
        let plaintext = match api_key_value {
            Value::String(raw) => {
                let trimmed = raw.trim();
                if trimmed.is_empty() {
                    "__placeholder__"
                } else {
                    trimmed
                }
            }
            _ => "__placeholder__",
        };
        record.encrypted_api_key = state
            .encrypt_catalog_secret_with_fallbacks(plaintext)
            .ok_or_else(|| "gateway 未配置 provider key 加密密钥".to_string())?;
    }

    if raw_key.contains_key("auth_config") {
        record.encrypted_auth_config = match normalized_auth_config {
            Some(auth_config) => {
                let plaintext =
                    serde_json::to_string(auth_config).map_err(|err| err.to_string())?;
                Some(
                    state
                        .encrypt_catalog_secret_with_fallbacks(&plaintext)
                        .ok_or_else(|| "gateway 未配置 provider key 加密密钥".to_string())?,
                )
            }
            None => None,
        };
    }

    Ok(())
}

fn build_import_provider_model_record(
    provider_id: &str,
    existing_id: Option<&str>,
    global_model_id: &str,
    item: &ImportedProviderModel,
) -> Result<UpsertAdminProviderModelRecord, String> {
    let provider_model_name = trim_required(&item.provider_model_name, "provider_model_name")?;
    let provider_model_mappings = normalize_json_array(
        item.provider_model_mappings.clone(),
        "provider_model_mappings",
    )?;
    let price_per_request = normalize_optional_price(item.price_per_request, "price_per_request")?;
    let tiered_pricing = normalize_json_object(item.tiered_pricing.clone(), "tiered_pricing")?;
    let config = normalize_json_object(item.config.clone(), "config")?;

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

#[derive(Debug, Clone, Default, serde::Serialize)]
struct AdminSystemUsersImportStats {
    users: AdminSystemConfigImportCounter,
    api_keys: AdminSystemConfigImportCounter,
    standalone_keys: AdminSystemConfigImportCounter,
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

fn validate_imported_system_export_version(version: Option<&Value>) -> Result<(), String> {
    let parsed = imported_system_export_version(version)?;
    if parsed < MIN_ADMIN_SYSTEM_IMPORT_VERSION {
        return Err(format!(
            "version {}.{} 已不再支持；仅支持 2.2+ 导出格式",
            parsed.0, parsed.1
        ));
    }
    Ok(())
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

fn imported_optional_f64(value: Option<&Value>, field_name: &str) -> Result<Option<f64>, String> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(number)) => number
            .as_f64()
            .filter(|value| value.is_finite())
            .ok_or_else(|| format!("{field_name} 必须是有限数值"))
            .map(Some),
        _ => Err(format!("{field_name} 必须是有限数值")),
    }
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

fn imported_rfc3339_to_unix_secs(
    value: Option<&Value>,
    field_name: &str,
) -> Result<Option<u64>, String> {
    let Some(value) = imported_optional_string(value)? else {
        return Ok(None);
    };
    let parsed = chrono::DateTime::parse_from_rfc3339(&value)
        .map_err(|_| format!("{field_name} 必须是 RFC3339 时间"))?;
    Ok(Some(parsed.timestamp().max(0) as u64))
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
            (total_balance - gift_balance).max(0.0)
        }
    } else {
        0.0
    }
    .max(0.0);
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
    let total_recharged = imported_optional_f64(
        wallet.and_then(|map| map.get("total_recharged")),
        "wallet.total_recharged",
    )?
    .unwrap_or(recharge_balance);
    let total_consumed = imported_optional_f64(
        wallet.and_then(|map| map.get("total_consumed")),
        "wallet.total_consumed",
    )?
    .unwrap_or(0.0);
    let total_refunded = imported_optional_f64(
        wallet.and_then(|map| map.get("total_refunded")),
        "wallet.total_refunded",
    )?
    .unwrap_or(0.0);
    let total_adjusted = imported_optional_f64(
        wallet.and_then(|map| map.get("total_adjusted")),
        "wallet.total_adjusted",
    )?
    .unwrap_or(gift_balance);
    let updated_at_unix_secs = imported_rfc3339_to_unix_secs(
        wallet.and_then(|map| map.get("updated_at")),
        "wallet.updated_at",
    )?;

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
    pub(crate) async fn import_admin_system_config(
        &self,
        request_body: &Bytes,
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
        if request_body.len() > ADMIN_SYSTEM_IMPORT_MAX_SIZE_BYTES {
            return Ok(Err(invalid_request("请求体大小不能超过 10MB")));
        }

        let parsed = routed!(parse_admin_system_config_import_request(request_body));
        let root = parsed.root;
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

        let mut global_models_by_name = self
            .list_admin_global_models(&AdminGlobalModelListQuery {
                offset: 0,
                limit: 10_000,
                is_active: None,
                search: None,
            })
            .await?
            .items
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
            let default_tiered_pricing = invalid!(normalize_json_object(
                model.default_tiered_pricing,
                "default_tiered_pricing",
            ));
            let supported_capabilities =
                normalize_supported_capabilities(model.supported_capabilities);
            let config = invalid!(normalize_json_object(model.config, "config"));

            if let Some(existing) = global_models_by_name.get(&name).cloned() {
                match merge_mode {
                    AdminImportMergeMode::Skip => {
                        stats.global_models.skipped += 1;
                    }
                    AdminImportMergeMode::Error => {
                        return Ok(Err(invalid_request(format!("GlobalModel '{name}' 已存在"))));
                    }
                    AdminImportMergeMode::Overwrite => {
                        let record = invalid!(UpdateAdminGlobalModelRecord::new(
                            existing.id.clone(),
                            display_name,
                            model.is_active,
                            default_price_per_request,
                            default_tiered_pricing,
                            supported_capabilities,
                            config,
                        )
                        .map_err(|err| err.to_string()));
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

            let record = invalid!(CreateAdminGlobalModelRecord::new(
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
            let Some(created) = self.create_admin_global_model(&record).await? else {
                return Ok(Err(invalid_request(format!(
                    "创建 GlobalModel '{name}' 失败"
                ))));
            };
            global_models_by_name.insert(name, created);
            stats.global_models.created += 1;
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
                                    ))))
                                }
                            };
                        let mut updated = invalid!(
                            self.build_admin_update_provider_record(&existing, patch)
                                .await
                        );
                        updated.proxy =
                            remap_import_proxy(imported_provider.proxy.clone(), &node_id_map);
                        updated.config = invalid!(encrypt_imported_provider_config(
                            self,
                            imported_provider.config.clone(),
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
                        ))))
                    }
                };
                let (mut record, shift_existing_priorities_from) =
                    invalid!(self.build_admin_create_provider_record(payload).await);
                if let Some(enable_format_conversion) = imported_provider.enable_format_conversion {
                    record.enable_format_conversion = enable_format_conversion;
                }
                record.proxy = remap_import_proxy(imported_provider.proxy.clone(), &node_id_map);
                record.config = invalid!(encrypt_imported_provider_config(
                    self,
                    imported_provider.config.clone(),
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
                                admin_endpoint_signature_parts(&imported_endpoint.api_format)
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
                                    )))
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
                                    header_rules: payload.header_rules,
                                    body_rules: payload.body_rules,
                                    max_retries: payload.max_retries,
                                    is_active: payload.is_active,
                                    config: payload.config,
                                    proxy: payload.proxy,
                                    format_acceptance_config: payload.format_acceptance_config,
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
                                updated.proxy = remap_import_proxy(
                                    imported_endpoint.proxy.clone(),
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
                    admin_endpoint_signature_parts(&imported_endpoint.api_format)
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
                        imported_endpoint.header_rules.clone(),
                        imported_endpoint.body_rules.clone(),
                        imported_endpoint.max_retries.unwrap_or(2),
                        imported_endpoint.config.clone(),
                        remap_import_proxy(imported_endpoint.proxy.clone(), &node_id_map),
                        imported_endpoint.format_acceptance_config.clone(),
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
                existing_endpoints_by_format.insert(normalized_api_format, created);
                stats.endpoints.created += 1;
            }

            let provider_endpoint_formats = existing_endpoints_by_format
                .keys()
                .cloned()
                .collect::<BTreeSet<_>>();

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
                let normalized_raw_key = normalize_import_key_raw_payload(
                    &raw_key,
                    &auth_type,
                    &normalized_api_formats,
                    normalized_auth_config.clone(),
                );
                let existing_key_index = if auth_type == "api_key" {
                    let target_key = imported_key
                        .api_key
                        .as_deref()
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(ToOwned::to_owned);
                    existing_keys.iter().position(|existing_key| {
                        target_key
                            .as_deref()
                            .zip(
                                self.decrypt_catalog_secret_with_fallbacks(
                                    &existing_key.encrypted_api_key,
                                )
                                .as_deref(),
                            )
                            .is_some_and(|(target, decrypted)| decrypted == target)
                    })
                } else if matches!(auth_type.as_str(), "service_account" | "vertex_ai") {
                    let target_email =
                        imported_service_account_email(normalized_auth_config.as_ref());
                    existing_keys.iter().position(|existing_key| {
                        target_email.as_deref().is_some_and(|target_email| {
                            self.parse_catalog_auth_config_json(existing_key)
                                .and_then(|config| {
                                    config
                                        .get("client_email")
                                        .and_then(Value::as_str)
                                        .map(str::trim)
                                        .filter(|value| !value.is_empty())
                                        .map(ToOwned::to_owned)
                                })
                                .as_deref()
                                == Some(target_email)
                        })
                    })
                } else {
                    build_import_key_match_name(&imported_key).and_then(|target_name| {
                        existing_keys.iter().position(|existing_key| {
                            existing_key
                                .auth_type
                                .trim()
                                .eq_ignore_ascii_case(&auth_type)
                                && existing_key.name == target_name
                        })
                    })
                };

                if let Some(existing_index) = existing_key_index {
                    let existing_key = existing_keys[existing_index].clone();
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
                                    return Ok(Err(invalid_request("Provider Key 配置格式无效")))
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
                            if auth_type == "oauth" {
                                invalid!(apply_imported_oauth_key_credentials(
                                    self,
                                    &raw_key,
                                    normalized_auth_config.as_ref(),
                                    &mut updated,
                                ));
                            }
                            updated.proxy =
                                remap_import_proxy(imported_key.proxy.clone(), &node_id_map);
                            updated.fingerprint = invalid!(normalize_json_object(
                                imported_key.fingerprint.clone(),
                                "fingerprint",
                            ));
                            let Some(persisted) =
                                self.update_provider_catalog_key(&updated).await?
                            else {
                                return Ok(Err(invalid_request(format!(
                                    "更新 Provider '{provider_name}' 的 Key 失败"
                                ))));
                            };
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
                if auth_type == "oauth" {
                    invalid!(apply_imported_oauth_key_credentials(
                        self,
                        &raw_key,
                        normalized_auth_config.as_ref(),
                        &mut record,
                    ));
                }
                record.is_active = imported_key.is_active;
                record.global_priority_by_format = invalid!(normalize_json_object(
                    imported_key.global_priority_by_format.clone(),
                    "global_priority_by_format",
                ));
                record.proxy = remap_import_proxy(imported_key.proxy.clone(), &node_id_map);
                record.fingerprint = invalid!(normalize_json_object(
                    imported_key.fingerprint.clone(),
                    "fingerprint",
                ));
                let Some(created) = self.create_provider_catalog_key(&record).await? else {
                    return Ok(Err(invalid_request(format!(
                        "创建 Provider '{provider_name}' 的 Key 失败"
                    ))));
                };
                existing_keys.push(created);
                stats.keys.created += 1;
            }

            let imported_models = routed!(parse_admin_system_config_nested_array::<
                ImportedProviderModel,
            >(&raw_provider, "models"));
            let mut existing_models_by_name = self
                .list_admin_provider_models(&AdminProviderModelListQuery {
                    provider_id: provider.id.clone(),
                    is_active: None,
                    offset: 0,
                    limit: 10_000,
                })
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
                                &global_model_id,
                                &imported_model,
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
                    &global_model_id,
                    &imported_model,
                ));
                let Some(created) = self.create_admin_provider_model(&record).await? else {
                    return Ok(Err(invalid_request(format!(
                        "创建 Provider '{provider_name}' 的模型 '{provider_model_name}' 失败"
                    ))));
                };
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
                let bind_dn = invalid!(trim_required(&ldap_config.bind_dn, "绑定 DN"));
                let base_dn = invalid!(trim_required(&ldap_config.base_dn, "Base DN"));
                let user_search_filter = invalid!(trim_required(
                    ldap_config
                        .user_search_filter
                        .as_deref()
                        .unwrap_or("(uid={username})"),
                    "搜索过滤器",
                ));
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
                let connect_timeout = ldap_config.connect_timeout.unwrap_or(10);
                if !(1..=60).contains(&connect_timeout) {
                    return Ok(Err(invalid_request(
                        "LDAP connect_timeout 必须在 1 到 60 秒之间",
                    )));
                }
                let bind_password = ldap_config
                    .bind_password
                    .as_deref()
                    .map(str::trim)
                    .map(ToOwned::to_owned);
                let will_have_password = bind_password
                    .as_deref()
                    .map(|value| !value.is_empty())
                    .unwrap_or_else(|| {
                        existing
                            .as_ref()
                            .and_then(|config| config.bind_password_encrypted.as_deref())
                            .map(str::trim)
                            .is_some_and(|value| !value.is_empty())
                    });
                if existing.is_none() && !will_have_password {
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
                let bind_password_encrypted = match bind_password {
                    Some(password) if password.is_empty() => None,
                    Some(password) => Some(routed!(self
                        .encrypt_catalog_secret_with_fallbacks(&password)
                        .ok_or_else(|| {
                            invalid_request("LDAP 绑定密码加密失败，请检查 Rust 数据加密配置")
                        }))),
                    None => existing
                        .as_ref()
                        .and_then(|config| config.bind_password_encrypted.clone()),
                };
                let config = StoredLdapModuleConfig {
                    server_url,
                    bind_dn,
                    bind_password_encrypted,
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

                match (existing.is_some(), merge_mode) {
                    (true, AdminImportMergeMode::Skip) => stats.ldap.skipped += 1,
                    (true, AdminImportMergeMode::Error) => {
                        return Ok(Err(invalid_request("LDAP 配置已存在")));
                    }
                    (true, AdminImportMergeMode::Overwrite) => {
                        let Some(_) = self.upsert_ldap_module_config(&config).await? else {
                            return Ok(Err(invalid_request("更新 LDAP 配置失败")));
                        };
                        stats.ldap.updated += 1;
                    }
                    (false, _) => {
                        let Some(_) = self.upsert_ldap_module_config(&config).await? else {
                            return Ok(Err(invalid_request("创建 LDAP 配置失败")));
                        };
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
                let client_secret_encrypted =
                    match oauth_provider.client_secret.as_deref().map(str::trim) {
                        Some(secret) if !secret.is_empty() => {
                            EncryptedSecretUpdate::Set(routed!(self
                                .encrypt_catalog_secret_with_fallbacks(secret)
                                .ok_or_else(|| {
                                    invalid_request("gateway 未配置 OAuth provider 加密密钥")
                                })))
                        }
                        _ => EncryptedSecretUpdate::Preserve,
                    };
                let record = UpsertOAuthProviderConfigRecord {
                    provider_type: provider_type.clone(),
                    display_name,
                    client_id,
                    client_secret_encrypted,
                    authorization_url_override: oauth_provider
                        .authorization_url_override
                        .map(|value| value.trim().to_string())
                        .filter(|value| !value.is_empty()),
                    token_url_override: oauth_provider
                        .token_url_override
                        .map(|value| value.trim().to_string())
                        .filter(|value| !value.is_empty()),
                    userinfo_url_override: oauth_provider
                        .userinfo_url_override
                        .map(|value| value.trim().to_string())
                        .filter(|value| !value.is_empty()),
                    scopes: normalize_string_list(oauth_provider.scopes),
                    redirect_uri,
                    frontend_callback_url,
                    attribute_mapping: invalid!(normalize_json_object(
                        oauth_provider.attribute_mapping,
                        "attribute_mapping",
                    )),
                    extra_config: invalid!(normalize_json_object(
                        oauth_provider.extra_config,
                        "extra_config",
                    )),
                    is_enabled: oauth_provider.is_enabled,
                };
                invalid!(record.validate().map_err(|err| err.to_string()));

                let Some(persisted) = self.upsert_oauth_provider_config(&record).await? else {
                    stats.oauth.skipped += (imported_oauth_provider_count - index) as u64;
                    stats.errors.push(
                        "当前运行环境不支持 OAuth Provider 配置读写，已跳过 oauth_providers"
                            .to_string(),
                    );
                    break;
                };
                oauth_by_type.insert(provider_type, persisted);
                if existed {
                    stats.oauth.updated += 1;
                } else {
                    stats.oauth.created += 1;
                }
            }
        }

        let mut existing_system_config_keys = self
            .list_system_config_entries()
            .await?
            .into_iter()
            .map(|entry| normalize_admin_system_config_key(&entry.key))
            .collect::<BTreeSet<_>>();
        for imported_config_item in imported_system_configs {
            let (_, system_config) = imported_config_item.into_parts();
            let normalized_key = normalize_admin_system_config_key(&system_config.key);
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
                    "value": system_config.value,
                    "description": system_config.description,
                }))
                .map_err(|err| GatewayError::Internal(err.to_string()))?,
            );
            match apply_admin_system_config_update(self, &system_config.key, &request_bytes).await?
            {
                Ok(_) => {
                    if exists {
                        stats.system_configs.updated += 1;
                    } else {
                        stats.system_configs.created += 1;
                        existing_system_config_keys.insert(normalized_key);
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
        if !self.has_auth_user_write_capability()
            || !self.has_auth_wallet_write_capability()
            || !self.has_auth_api_key_writer()
        {
            return Ok(Err((
                http::StatusCode::SERVICE_UNAVAILABLE,
                json!({ "detail": "Admin system data unavailable" }),
            )));
        }
        if request_body.len() > ADMIN_SYSTEM_IMPORT_MAX_SIZE_BYTES {
            return Ok(Err(invalid_request("请求体大小不能超过 10MB")));
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

        let standalone_owner_id = match operator_id {
            Some(candidate) => match self.find_user_auth_by_id(candidate).await? {
                Some(user) if user.role.eq_ignore_ascii_case("admin") => Some(user.id),
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

        invalid_value!(validate_imported_system_export_version(root.get("version")));

        let mut stats = AdminSystemUsersImportStats::default();

        for (index, raw_user) in users.iter().enumerate() {
            let user = match imported_object_field(raw_user, &format!("users[{index}]")) {
                Ok(value) => value,
                Err(detail) => return Ok(Err(invalid_request(detail))),
            };
            let role = invalid_value!(imported_optional_string(user.get("role")))
                .unwrap_or_else(|| "user".to_string())
                .to_ascii_lowercase();
            if role == "admin" {
                let skipped_email = invalid_value!(imported_optional_string(user.get("email")));
                let skipped_username =
                    invalid_value!(imported_optional_string(user.get("username")));
                stats.users.skipped += 1;
                stats.errors.push(format!(
                    "跳过管理员用户: {}",
                    skipped_email
                        .or(skipped_username)
                        .unwrap_or_else(|| format!("users[{index}]"))
                ));
                continue;
            }

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
            let password_hash = invalid_value!(imported_optional_string(user.get("password_hash")));
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
            let is_active =
                invalid_value!(imported_optional_bool(user.get("is_active"))).unwrap_or(true);
            let model_capability_settings = invalid_value!(imported_optional_json_object(
                user.get("model_capability_settings"),
                "model_capability_settings"
            ));
            let wallet_payload = match user.get("wallet") {
                Some(Value::Object(map)) => Some(map),
                Some(Value::Null) | None => None,
                Some(_) => return Ok(Err(invalid_request("wallet 必须是对象"))),
            };
            let wallet_target =
                invalid_value!(normalize_imported_wallet_target(wallet_payload, false));

            let mut existing_user = if let Some(email) = email.as_deref() {
                self.find_user_auth_by_identifier(email).await?
            } else {
                None
            };
            if existing_user.is_none() {
                existing_user = self.find_user_auth_by_identifier(&username).await?;
            }

            let user_id = if let Some(existing) = existing_user {
                if existing.role.eq_ignore_ascii_case("admin") {
                    stats.users.skipped += 1;
                    stats.errors.push(format!(
                        "跳过管理员用户记录: {}",
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
                        let updated_profile = self
                            .update_local_auth_user_profile(
                                &existing.id,
                                email.clone(),
                                Some(username.clone()),
                            )
                            .await?;
                        if updated_profile.is_none() {
                            return Ok(Err((
                                http::StatusCode::SERVICE_UNAVAILABLE,
                                json!({ "detail": "Admin system data unavailable" }),
                            )));
                        }
                        if let Some(password_hash) =
                            password_hash.as_deref().filter(|value| !value.is_empty())
                        {
                            let updated_password = self
                                .update_local_auth_user_password_hash(
                                    &existing.id,
                                    password_hash.to_string(),
                                    chrono::Utc::now(),
                                )
                                .await?;
                            if updated_password.is_none() {
                                return Ok(Err((
                                    http::StatusCode::SERVICE_UNAVAILABLE,
                                    json!({ "detail": "Admin system data unavailable" }),
                                )));
                            }
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
                        if user.contains_key("email_verified") {
                            stats.errors.push(format!(
                                "用户 '{}' 的 email_verified 当前不会覆盖已有值",
                                email.clone().unwrap_or(username.clone())
                            ));
                        }
                        if user.contains_key("model_capability_settings") {
                            let _ = self
                                .update_user_model_capability_settings(
                                    &existing.id,
                                    model_capability_settings.clone(),
                                )
                                .await?;
                        }
                        self.sync_imported_user_wallet(
                            &existing.id,
                            &wallet_target,
                            &email.clone().unwrap_or(username.clone()),
                        )
                        .await?;
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
                        password_hash.unwrap_or_default(),
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
                if user.contains_key("model_capability_settings") {
                    let _ = self
                        .update_user_model_capability_settings(
                            &created.id,
                            model_capability_settings.clone(),
                        )
                        .await?;
                }
                self.sync_imported_user_wallet(
                    &created.id,
                    &wallet_target,
                    &email.clone().unwrap_or(username.clone()),
                )
                .await?;
                stats.users.created += 1;
                created.id
            };

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
            let mut existing_api_keys_by_hash = existing_api_keys
                .into_iter()
                .map(|record| (record.key_hash.clone(), record))
                .collect::<BTreeMap<_, _>>();

            for (key_index, raw_key) in imported_api_keys.iter().enumerate() {
                let key = match imported_object_field(
                    raw_key,
                    &format!("users[{index}].api_keys[{key_index}]"),
                ) {
                    Ok(value) => value,
                    Err(detail) => return Ok(Err(invalid_request(detail))),
                };
                let Some((key_hash, key_encrypted)) =
                    invalid_value!(self.resolve_imported_system_user_api_key_material(key))
                else {
                    stats.api_keys.skipped += 1;
                    stats.errors.push(format!(
                        "跳过无效 API Key: 用户 '{}'",
                        email.clone().unwrap_or(username.clone())
                    ));
                    continue;
                };
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
                    invalid_value!(imported_optional_bool(key.get("is_active"))).unwrap_or(true);
                let expires_at_unix_secs = invalid_value!(imported_rfc3339_to_unix_secs(
                    key.get("expires_at"),
                    "expires_at"
                ));
                let auto_delete_on_expiry =
                    invalid_value!(imported_optional_bool(key.get("auto_delete_on_expiry")))
                        .unwrap_or(false);
                let total_requests = invalid_value!(imported_optional_u64(
                    key.get("total_requests"),
                    "total_requests"
                ))
                .unwrap_or(0);
                let total_cost_usd = invalid_value!(imported_optional_f64(
                    key.get("total_cost_usd"),
                    "total_cost_usd"
                ))
                .unwrap_or(0.0);

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
                            let updated = self
                                .update_user_api_key_basic(
                                    aether_data::repository::auth::UpdateUserApiKeyBasicRecord {
                                        user_id: user_id.clone(),
                                        api_key_id: existing_key.api_key_id.clone(),
                                        name: name.clone(),
                                        rate_limit: Some(rate_limit),
                                        concurrent_limit: if key.contains_key("concurrent_limit") {
                                            concurrent_limit
                                        } else {
                                            None
                                        },
                                    },
                                )
                                .await?;
                            if updated.is_none() {
                                return Ok(Err((
                                    http::StatusCode::SERVICE_UNAVAILABLE,
                                    json!({ "detail": "Admin system data unavailable" }),
                                )));
                            }
                            let _ = self
                                .set_user_api_key_allowed_providers(
                                    &user_id,
                                    &existing_key.api_key_id,
                                    allowed_providers.clone(),
                                )
                                .await?;
                            let _ = self
                                .set_user_api_key_force_capabilities(
                                    &user_id,
                                    &existing_key.api_key_id,
                                    force_capabilities.clone(),
                                )
                                .await?;
                            let _ = self
                                .set_user_api_key_active(
                                    &user_id,
                                    &existing_key.api_key_id,
                                    is_active,
                                )
                                .await?;
                            if key.contains_key("allowed_api_formats")
                                || key.contains_key("allowed_models")
                                || key.contains_key("expires_at")
                                || key.contains_key("auto_delete_on_expiry")
                                || key.contains_key("total_requests")
                                || key.contains_key("total_cost_usd")
                            {
                                stats.errors.push(format!(
                                    "用户 '{}' 的现有 API Key 仅覆盖基础字段；高级导入字段保持原值",
                                    email.clone().unwrap_or(username.clone())
                                ));
                            }
                            stats.api_keys.updated += 1;
                        }
                    }
                    continue;
                }

                let created = self
                    .create_user_api_key(aether_data::repository::auth::CreateUserApiKeyRecord {
                        user_id: user_id.clone(),
                        api_key_id: Uuid::new_v4().to_string(),
                        key_hash: key_hash.clone(),
                        key_encrypted,
                        name,
                        allowed_providers,
                        allowed_api_formats,
                        allowed_models,
                        rate_limit,
                        concurrent_limit,
                        force_capabilities,
                        is_active,
                        expires_at_unix_secs,
                        auto_delete_on_expiry,
                        total_requests,
                        total_cost_usd,
                    })
                    .await?;
                let Some(created) = created else {
                    return Ok(Err((
                        http::StatusCode::SERVICE_UNAVAILABLE,
                        json!({ "detail": "Admin system data unavailable" }),
                    )));
                };
                existing_api_keys_by_hash.insert(key_hash, created);
                stats.api_keys.created += 1;
            }
        }

        if standalone_keys.is_empty() {
            return Ok(Ok(json!({
                "message": "用户数据导入成功",
                "stats": stats,
            })));
        }

        let Some(standalone_owner_id) = standalone_owner_id else {
            stats.standalone_keys.skipped += standalone_keys.len() as u64;
            stats
                .errors
                .push("无法导入独立余额 Key: 当前管理员用户记录不存在".to_string());
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
        let mut existing_standalone_by_hash = existing_standalone_keys
            .into_iter()
            .map(|record| (record.key_hash.clone(), record))
            .collect::<BTreeMap<_, _>>();

        for (index, raw_key) in standalone_keys.iter().enumerate() {
            let key = match imported_object_field(raw_key, &format!("standalone_keys[{index}]")) {
                Ok(value) => value,
                Err(detail) => return Ok(Err(invalid_request(detail))),
            };
            let Some((key_hash, key_encrypted)) =
                invalid_value!(self.resolve_imported_system_user_api_key_material(key))
            else {
                stats.standalone_keys.skipped += 1;
                stats
                    .errors
                    .push(format!("跳过无效独立余额 Key: standalone_keys[{index}]"));
                continue;
            };
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
                invalid_value!(imported_optional_bool(key.get("is_active"))).unwrap_or(true);
            let expires_at_unix_secs = invalid_value!(imported_rfc3339_to_unix_secs(
                key.get("expires_at"),
                "expires_at"
            ));
            let auto_delete_on_expiry =
                invalid_value!(imported_optional_bool(key.get("auto_delete_on_expiry")))
                    .unwrap_or(false);
            let total_requests = invalid_value!(imported_optional_u64(
                key.get("total_requests"),
                "total_requests"
            ))
            .unwrap_or(0);
            let total_cost_usd = invalid_value!(imported_optional_f64(
                key.get("total_cost_usd"),
                "total_cost_usd"
            ))
            .unwrap_or(0.0);
            let wallet_payload = match key.get("wallet") {
                Some(Value::Object(map)) => Some(map),
                Some(Value::Null) | None => None,
                Some(_) => return Ok(Err(invalid_request("wallet 必须是对象"))),
            };
            let unlimited =
                invalid_value!(imported_optional_bool(key.get("unlimited"))).unwrap_or(false);
            let wallet_target =
                invalid_value!(normalize_imported_wallet_target(wallet_payload, unlimited));

            if let Some(existing_key) = existing_standalone_by_hash.get(&key_hash).cloned() {
                match merge_mode {
                    AdminImportMergeMode::Skip => {
                        stats.standalone_keys.skipped += 1;
                    }
                    AdminImportMergeMode::Error => {
                        return Ok(Err(invalid_request("独立余额 Key 已存在")));
                    }
                    AdminImportMergeMode::Overwrite => {
                        let updated = self
                            .update_standalone_api_key_basic(
                                aether_data::repository::auth::UpdateStandaloneApiKeyBasicRecord {
                                    api_key_id: existing_key.api_key_id.clone(),
                                    name: name.clone(),
                                    rate_limit_present: true,
                                    rate_limit: Some(rate_limit),
                                    concurrent_limit_present: key.contains_key("concurrent_limit"),
                                    concurrent_limit,
                                    allowed_providers: Some(allowed_providers.clone()),
                                    allowed_api_formats: Some(allowed_api_formats.clone()),
                                    allowed_models: Some(allowed_models.clone()),
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
                        let _ = self
                            .set_standalone_api_key_active(&existing_key.api_key_id, is_active)
                            .await?;
                        if key.contains_key("expires_at")
                            || key.contains_key("auto_delete_on_expiry")
                            || key.contains_key("force_capabilities")
                            || key.contains_key("total_requests")
                            || key.contains_key("total_cost_usd")
                        {
                            stats.errors.push(
                                "现有独立余额 Key 仅覆盖基础字段；高级导入字段保持原值".to_string(),
                            );
                        }
                        self.sync_imported_api_key_wallet(
                            &existing_key.api_key_id,
                            &wallet_target,
                            key.get("name")
                                .and_then(Value::as_str)
                                .unwrap_or("独立余额 Key"),
                        )
                        .await?;
                        stats.standalone_keys.updated += 1;
                    }
                }
                continue;
            }

            let created = self
                .create_standalone_api_key(
                    aether_data::repository::auth::CreateStandaloneApiKeyRecord {
                        user_id: standalone_owner_id.clone(),
                        api_key_id: Uuid::new_v4().to_string(),
                        key_hash: key_hash.clone(),
                        key_encrypted,
                        name,
                        allowed_providers,
                        allowed_api_formats,
                        allowed_models,
                        rate_limit: Some(rate_limit),
                        concurrent_limit,
                        force_capabilities,
                        is_active,
                        expires_at_unix_secs,
                        auto_delete_on_expiry,
                        total_requests,
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
            self.sync_imported_api_key_wallet(
                &created.api_key_id,
                &wallet_target,
                created.name.as_deref().unwrap_or("独立余额 Key"),
            )
            .await?;
            existing_standalone_by_hash.insert(key_hash, created);
            stats.standalone_keys.created += 1;
        }

        Ok(Ok(json!({
            "message": "用户数据导入成功",
            "stats": stats,
        })))
    }

    async fn sync_imported_user_wallet(
        &self,
        user_id: &str,
        wallet_target: &ImportedWalletTarget,
        label: &str,
    ) -> Result<(), GatewayError> {
        if self
            .find_wallet(WalletLookupKey::UserId(user_id))
            .await?
            .is_none()
        {
            let created = self
                .initialize_auth_user_wallet(user_id, 0.0, false)
                .await?;
            if created.is_none() {
                return Err(GatewayError::Internal(format!(
                    "failed to initialize imported wallet for {label}"
                )));
            }
        }
        self.sync_wallet_snapshot(WalletOwner::User(user_id), wallet_target, label)
            .await
    }

    async fn sync_imported_api_key_wallet(
        &self,
        api_key_id: &str,
        wallet_target: &ImportedWalletTarget,
        label: &str,
    ) -> Result<(), GatewayError> {
        if self
            .find_wallet(WalletLookupKey::ApiKeyId(api_key_id))
            .await?
            .is_none()
        {
            let created = self
                .initialize_auth_api_key_wallet(api_key_id, 0.0, false)
                .await?;
            if created.is_none() {
                return Err(GatewayError::Internal(format!(
                    "failed to initialize imported wallet for {label}"
                )));
            }
        }
        self.sync_wallet_snapshot(WalletOwner::ApiKey(api_key_id), wallet_target, label)
            .await
    }

    async fn sync_wallet_snapshot(
        &self,
        owner: WalletOwner<'_>,
        wallet_target: &ImportedWalletTarget,
        label: &str,
    ) -> Result<(), GatewayError> {
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
        if updated.is_none() {
            return Err(GatewayError::Internal(format!(
                "failed to persist imported wallet snapshot for {label}"
            )));
        }
        Ok(())
    }

    fn resolve_imported_system_user_api_key_material(
        &self,
        key: &Map<String, Value>,
    ) -> Result<Option<(String, Option<String>)>, String> {
        let plaintext_key = imported_optional_string(key.get("key"))?;
        if let Some(plaintext_key) = plaintext_key.filter(|value| !value.is_empty()) {
            return Ok(Some((
                hash_admin_user_api_key(&plaintext_key),
                self.encrypt_catalog_secret_with_fallbacks(&plaintext_key),
            )));
        }
        let key_hash = imported_optional_string(key.get("key_hash"))?;
        let key_encrypted = imported_optional_string(key.get("key_encrypted"))?;
        Ok(key_hash.map(|key_hash| (key_hash, key_encrypted)))
    }
}

#[derive(Clone, Copy)]
enum WalletOwner<'a> {
    User(&'a str),
    ApiKey(&'a str),
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        imported_optional_bool, imported_optional_f64, imported_optional_i32,
        imported_optional_u64, imported_string_list_from_value,
        validate_imported_system_export_version,
    };

    #[test]
    fn import_requires_supported_export_version() {
        assert!(validate_imported_system_export_version(Some(&json!("2.2"))).is_ok());
        assert_eq!(
            validate_imported_system_export_version(Some(&json!("2.1"))).unwrap_err(),
            "version 2.1 已不再支持；仅支持 2.2+ 导出格式"
        );
        assert_eq!(
            validate_imported_system_export_version(Some(&json!(null))).unwrap_err(),
            "version 必须是 x.y 字符串"
        );
    }

    #[test]
    fn import_rejects_legacy_string_scalars() {
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
            imported_optional_f64(Some(&json!("1.25")), "total_cost_usd").unwrap_err(),
            "total_cost_usd 必须是有限数值"
        );
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
