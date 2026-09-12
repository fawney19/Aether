use super::ADMIN_SYSTEM_DATA_EXPORT_VERSION;
use crate::handlers::admin::request::AdminAppState;
use crate::handlers::admin::system::shared::configs::is_sensitive_admin_system_config_key;
use crate::handlers::admin::system::shared::export::{
    build_admin_system_export_providers_payload, decrypt_admin_system_export_secret,
    project_admin_system_export_json, project_admin_system_export_optional_url,
    project_admin_system_export_url, ADMIN_SYSTEM_EXPORT_PAGE_LIMIT,
};
use crate::handlers::shared::{
    decrypt_or_migrate_auth_api_key_secret,
    decrypt_or_migrate_identity_oauth_provider_client_secret,
    decrypt_or_migrate_ldap_bind_password, decrypt_or_migrate_smtp_password,
    decrypt_or_migrate_system_config_secret, smtp_password_binding, system_config_bool,
    system_config_string, unix_secs_to_rfc3339,
};
use crate::GatewayError;
use aether_admin::system::{
    serialize_admin_system_users_export_wallet, AdminSystemConfigDocument, AdminSystemConfigEntry,
    AdminSystemConfigGlobalModel, AdminSystemConfigLdap, AdminSystemConfigOAuthProvider,
    AdminSystemConfigProxyNode, ADMIN_SYSTEM_CONFIG_EXPORT_VERSION,
    ADMIN_SYSTEM_USERS_EXPORT_VERSION,
};
use aether_data_contracts::repository::global_models::{
    AdminGlobalModelListQuery, AdminProviderModelListQuery, StoredAdminGlobalModel,
    StoredAdminProviderModel,
};
use chrono::Utc;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

pub(crate) const ADMIN_SYSTEM_EXPORT_CREDENTIALS_NOT_EXPORTED: &str = "not_exported";
const ADMIN_SYSTEM_USERS_RECOVERY_EXPORT_VERSION: &str = "1.5";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SystemExportMode {
    InteractiveDownload,
    RecoveryBackup,
    /// Internal checkpoint used by aggregate imports. It keeps operational enabled/disabled
    /// flags while retaining the interactive export's credential redaction guarantees.
    RollbackCheckpoint,
}

impl SystemExportMode {
    pub(crate) fn credentials_are_exported(self) -> bool {
        self == Self::RecoveryBackup
    }

    pub(crate) fn preserves_active_state(self) -> bool {
        matches!(self, Self::RecoveryBackup | Self::RollbackCheckpoint)
    }

    pub(crate) fn credential_state(self) -> Option<String> {
        (!self.credentials_are_exported())
            .then(|| ADMIN_SYSTEM_EXPORT_CREDENTIALS_NOT_EXPORTED.to_string())
    }

    fn users_export_version(self) -> &'static str {
        if self.credentials_are_exported() {
            ADMIN_SYSTEM_USERS_RECOVERY_EXPORT_VERSION
        } else {
            ADMIN_SYSTEM_USERS_EXPORT_VERSION
        }
    }
}

fn matching_export_providers<'a>(
    reference: &str,
    providers: &'a [aether_data_contracts::repository::provider_catalog::StoredProviderCatalogProvider],
) -> Vec<&'a aether_data_contracts::repository::provider_catalog::StoredProviderCatalogProvider> {
    let reference = reference.trim();
    let by_id = providers
        .iter()
        .filter(|provider| provider.id.eq_ignore_ascii_case(reference))
        .collect::<Vec<_>>();
    if !by_id.is_empty() {
        return by_id;
    }
    let by_name = providers
        .iter()
        .filter(|provider| provider.name.eq_ignore_ascii_case(reference))
        .collect::<Vec<_>>();
    if !by_name.is_empty() {
        return by_name;
    }
    providers
        .iter()
        .filter(|provider| provider.provider_type.eq_ignore_ascii_case(reference))
        .collect()
}

fn build_portable_user_group_provider_access(
    group: &aether_data::repository::users::StoredUserGroup,
    providers: &[aether_data_contracts::repository::provider_catalog::StoredProviderCatalogProvider],
    keys: &[aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey],
) -> Result<(Vec<serde_json::Value>, Vec<serde_json::Value>), String> {
    let mut allowed_provider_refs = BTreeMap::<String, serde_json::Value>::new();
    for reference in group.allowed_providers.as_deref().unwrap_or_default() {
        let matches = matching_export_providers(reference, providers);
        if matches.is_empty() {
            return Err(format!(
                "用户组 '{}' 引用了不存在的 Provider: {}",
                group.name,
                reference.trim()
            ));
        }
        for provider in matches {
            allowed_provider_refs.insert(
                provider.id.clone(),
                json!({ "id": provider.id, "name": provider.name }),
            );
        }
    }

    let mut policy_refs = Vec::new();
    for (provider_reference, key_references) in &group.provider_key_policies {
        let provider_matches = matching_export_providers(provider_reference, providers);
        let [provider] = provider_matches.as_slice() else {
            return Err(if provider_matches.is_empty() {
                format!(
                    "用户组 '{}' 的 Key 策略引用了不存在的 Provider: {}",
                    group.name, provider_reference
                )
            } else {
                format!(
                    "用户组 '{}' 的 Key 策略 Provider 名称不唯一: {}",
                    group.name, provider_reference
                )
            });
        };
        let provider_keys = keys
            .iter()
            .filter(|key| key.provider_id == provider.id)
            .collect::<Vec<_>>();
        let mut key_refs = BTreeMap::<String, serde_json::Value>::new();
        for key_reference in key_references {
            let id_matches = provider_keys
                .iter()
                .filter(|key| key.id.eq_ignore_ascii_case(key_reference))
                .copied()
                .collect::<Vec<_>>();
            let matches = if id_matches.is_empty() {
                provider_keys
                    .iter()
                    .filter(|key| key.name.eq_ignore_ascii_case(key_reference))
                    .copied()
                    .collect::<Vec<_>>()
            } else {
                id_matches
            };
            let [key] = matches.as_slice() else {
                return Err(if matches.is_empty() {
                    format!(
                        "用户组 '{}' 的 Key 策略引用了不存在的 Key: {}",
                        group.name, key_reference
                    )
                } else {
                    format!(
                        "用户组 '{}' 的 Provider '{}' 存在重名 Key: {}",
                        group.name, provider.name, key_reference
                    )
                });
            };
            key_refs.insert(key.id.clone(), json!({ "id": key.id, "name": key.name }));
        }
        policy_refs.push(json!({
            "provider": { "id": provider.id, "name": provider.name },
            "keys": key_refs.into_values().collect::<Vec<_>>(),
        }));
    }
    policy_refs.sort_by(|left, right| {
        left.pointer("/provider/id")
            .and_then(serde_json::Value::as_str)
            .cmp(
                &right
                    .pointer("/provider/id")
                    .and_then(serde_json::Value::as_str),
            )
    });

    Ok((allowed_provider_refs.into_values().collect(), policy_refs))
}

impl<'a> AdminAppState<'a> {
    pub(crate) async fn list_all_admin_global_models_for_system_transfer(
        &self,
    ) -> Result<Vec<StoredAdminGlobalModel>, GatewayError> {
        self.list_all_admin_global_models_for_system_transfer_with_page_limit(
            ADMIN_SYSTEM_EXPORT_PAGE_LIMIT,
        )
        .await
    }

    async fn list_all_admin_global_models_for_system_transfer_with_page_limit(
        &self,
        page_limit: usize,
    ) -> Result<Vec<StoredAdminGlobalModel>, GatewayError> {
        if page_limit == 0 {
            return Err(GatewayError::Internal(
                "system transfer global-model page size must be positive".to_string(),
            ));
        }

        let first = self
            .scan_admin_global_models_for_system_transfer(page_limit)
            .await?;
        let second = self
            .scan_admin_global_models_for_system_transfer(page_limit)
            .await?;
        if first != second {
            return Err(GatewayError::Internal(
                "global-model catalog changed while building system transfer; retry".to_string(),
            ));
        }
        Ok(second)
    }

    async fn scan_admin_global_models_for_system_transfer(
        &self,
        page_limit: usize,
    ) -> Result<Vec<StoredAdminGlobalModel>, GatewayError> {
        let mut models = Vec::new();
        let mut seen_ids = BTreeSet::new();
        let mut expected_total = None;
        let mut offset = 0_usize;
        loop {
            let page = self
                .list_admin_global_models(&AdminGlobalModelListQuery {
                    offset,
                    limit: page_limit,
                    is_active: None,
                    search: None,
                })
                .await?;
            let total = *expected_total.get_or_insert(page.total);
            if page.total != total {
                return Err(GatewayError::Internal(
                    "global-model catalog changed while building system transfer; retry"
                        .to_string(),
                ));
            }
            let page_len = page.items.len();
            if page_len == 0 {
                if offset == total {
                    break;
                }
                return Err(GatewayError::Internal(
                    "global-model catalog pagination ended before the advertised total".to_string(),
                ));
            }
            for model in page.items {
                if !seen_ids.insert(model.id.clone()) {
                    return Err(GatewayError::Internal(
                        "global-model catalog changed while building system transfer; retry"
                            .to_string(),
                    ));
                }
                models.push(model);
            }
            offset = offset.checked_add(page_len).ok_or_else(|| {
                GatewayError::Internal("global-model catalog pagination overflow".to_string())
            })?;
            if offset >= total {
                if offset != total {
                    return Err(GatewayError::Internal(
                        "global-model catalog returned more rows than its advertised total"
                            .to_string(),
                    ));
                }
                break;
            }
        }
        Ok(models)
    }

    pub(crate) async fn list_all_admin_provider_models_for_system_transfer(
        &self,
        provider_id: &str,
    ) -> Result<Vec<StoredAdminProviderModel>, GatewayError> {
        self.list_all_admin_provider_models_for_system_transfer_with_page_limit(
            provider_id,
            ADMIN_SYSTEM_EXPORT_PAGE_LIMIT,
        )
        .await
    }

    async fn list_all_admin_provider_models_for_system_transfer_with_page_limit(
        &self,
        provider_id: &str,
        page_limit: usize,
    ) -> Result<Vec<StoredAdminProviderModel>, GatewayError> {
        if page_limit == 0 {
            return Err(GatewayError::Internal(
                "system transfer provider-model page size must be positive".to_string(),
            ));
        }

        let first = self
            .scan_admin_provider_models_for_system_transfer(provider_id, page_limit)
            .await?;
        let second = self
            .scan_admin_provider_models_for_system_transfer(provider_id, page_limit)
            .await?;
        if first != second {
            return Err(GatewayError::Internal(format!(
                "provider-model catalog for '{provider_id}' changed while building system transfer; retry"
            )));
        }
        Ok(second)
    }

    async fn scan_admin_provider_models_for_system_transfer(
        &self,
        provider_id: &str,
        page_limit: usize,
    ) -> Result<Vec<StoredAdminProviderModel>, GatewayError> {
        let mut models = Vec::new();
        let mut seen_ids = BTreeSet::new();
        let mut offset = 0_usize;
        loop {
            let page = self
                .list_admin_provider_models(&AdminProviderModelListQuery {
                    provider_id: provider_id.to_string(),
                    is_active: None,
                    offset,
                    limit: page_limit,
                })
                .await?;
            let page_len = page.len();
            for model in page {
                if !seen_ids.insert(model.id.clone()) {
                    return Err(GatewayError::Internal(format!(
                        "provider-model catalog for '{provider_id}' changed while building system transfer; retry"
                    )));
                }
                models.push(model);
            }
            if page_len < page_limit {
                break;
            }
            offset = offset.checked_add(page_len).ok_or_else(|| {
                GatewayError::Internal("provider-model catalog pagination overflow".to_string())
            })?;
        }
        Ok(models)
    }

    pub(crate) async fn build_admin_system_config_export_payload(
        &self,
        mode: SystemExportMode,
    ) -> Result<serde_json::Value, GatewayError> {
        let global_models = self
            .list_all_admin_global_models_for_system_transfer()
            .await?;
        let global_model_name_by_id = global_models
            .iter()
            .map(|model| (model.id.clone(), model.name.clone()))
            .collect::<BTreeMap<_, _>>();
        let global_models_data = global_models
            .iter()
            .map(|model| AdminSystemConfigGlobalModel {
                name: model.name.clone(),
                display_name: model.display_name.clone(),
                usage_count: Some(model.usage_count),
                default_price_per_request: model.default_price_per_request,
                default_tiered_pricing: project_admin_system_export_json(
                    mode,
                    model.default_tiered_pricing.as_ref(),
                ),
                supported_capabilities: model.supported_capabilities.as_ref().and_then(|value| {
                    value.as_array().map(|items| {
                        items
                            .iter()
                            .filter_map(serde_json::Value::as_str)
                            .map(ToOwned::to_owned)
                            .collect::<Vec<_>>()
                    })
                }),
                config: project_admin_system_export_json(mode, model.config.as_ref()),
                is_active: model.is_active,
            })
            .collect::<Vec<_>>();
        let providers_data =
            build_admin_system_export_providers_payload(self, &global_model_name_by_id, mode)
                .await?;

        let ldap_config = self.get_ldap_module_config().await?;
        let ldap_bind_password = if mode.credentials_are_exported() {
            match ldap_config.as_ref() {
                Some(config) => decrypt_or_migrate_ldap_bind_password(self.app(), config).await?,
                None => None,
            }
        } else {
            None
        };
        let ldap_data = ldap_config.map(|config| AdminSystemConfigLdap {
            server_url: config.server_url,
            bind_dn: config.bind_dn,
            bind_password: ldap_bind_password,
            base_dn: config.base_dn,
            user_search_filter: config.user_search_filter,
            username_attr: config.username_attr,
            email_attr: config.email_attr,
            display_name_attr: config.display_name_attr,
            is_enabled: mode.preserves_active_state() && config.is_enabled,
            is_exclusive: mode.preserves_active_state() && config.is_exclusive,
            use_starttls: config.use_starttls,
            connect_timeout: config.connect_timeout,
        });

        let system_configs = self.list_system_config_entries().await?;
        let smtp_password_binding = if mode.credentials_are_exported() {
            let host = self
                .read_system_config_json_value("smtp_host")
                .await?
                .and_then(|value| system_config_string(Some(&value)));
            let port = self
                .read_system_config_json_value("smtp_port")
                .await?
                .map(|value| crate::email_delivery::system_config_u16(Some(&value), 587))
                .unwrap_or(587);
            let user = self
                .read_system_config_json_value("smtp_user")
                .await?
                .and_then(|value| system_config_string(Some(&value)));
            let use_tls = self
                .read_system_config_json_value("smtp_use_tls")
                .await?
                .map(|value| system_config_bool(Some(&value), true))
                .unwrap_or(true);
            let use_ssl = self
                .read_system_config_json_value("smtp_use_ssl")
                .await?
                .map(|value| system_config_bool(Some(&value), false))
                .unwrap_or(false);
            host.and_then(|host| {
                smtp_password_binding(&host, port, user.as_deref(), use_tls, use_ssl)
            })
        } else {
            None
        };
        let mut system_configs_data = Vec::new();
        for entry in system_configs.iter().filter(|entry| {
            mode.credentials_are_exported()
                || !is_sensitive_admin_system_config_key(&entry.key)
                    && !is_interactive_export_private_system_config_key(&entry.key)
        }) {
            let value = if mode.credentials_are_exported()
                && is_sensitive_admin_system_config_key(&entry.key)
            {
                match entry.value.as_str() {
                    Some(stored) if !stored.trim().is_empty() => {
                        let plaintext = if entry.key.eq_ignore_ascii_case("smtp_password") {
                            let Some(binding) = smtp_password_binding.as_ref() else {
                                return Err(GatewayError::Internal(
                                    "RecoveryBackup SMTP password binding is unavailable"
                                        .to_string(),
                                ));
                            };
                            decrypt_or_migrate_smtp_password(
                                self.as_ref(),
                                binding,
                                stored.to_string(),
                            )
                            .await?
                        } else {
                            decrypt_or_migrate_system_config_secret(
                                self.as_ref(),
                                &entry.key,
                                stored.to_string(),
                            )
                            .await?
                        };
                        serde_json::Value::String(plaintext)
                    }
                    Some(_) => serde_json::Value::Null,
                    None if entry.value.is_null() => serde_json::Value::Null,
                    None => {
                        return Err(GatewayError::Internal(format!(
                            "RecoveryBackup 敏感系统配置 '{}' 不是密文字符串或 null",
                            entry.key,
                        )))
                    }
                }
            } else {
                project_admin_system_export_json(mode, Some(&entry.value))
                    .unwrap_or(serde_json::Value::Null)
            };
            system_configs_data.push(AdminSystemConfigEntry {
                key: entry.key.clone(),
                value,
                description: entry.description.clone(),
            });
        }

        let oauth_providers = self.list_oauth_provider_configs().await?;
        let mut oauth_data = Vec::with_capacity(oauth_providers.len());
        for provider in &oauth_providers {
            let client_secret = if mode.credentials_are_exported() {
                decrypt_or_migrate_identity_oauth_provider_client_secret(self.as_ref(), provider)
                    .await?
            } else {
                None
            };
            oauth_data.push(AdminSystemConfigOAuthProvider {
                provider_type: provider.provider_type.clone(),
                display_name: provider.display_name.clone(),
                client_id: provider.client_id.clone(),
                client_secret,
                authorization_url_override: project_admin_system_export_optional_url(
                    mode,
                    provider.authorization_url_override.as_deref(),
                ),
                token_url_override: project_admin_system_export_optional_url(
                    mode,
                    provider.token_url_override.as_deref(),
                ),
                userinfo_url_override: project_admin_system_export_optional_url(
                    mode,
                    provider.userinfo_url_override.as_deref(),
                ),
                scopes: provider.scopes.clone(),
                redirect_uri: project_admin_system_export_url(mode, &provider.redirect_uri),
                frontend_callback_url: project_admin_system_export_url(
                    mode,
                    &provider.frontend_callback_url,
                ),
                attribute_mapping: project_admin_system_export_json(
                    mode,
                    provider.attribute_mapping.as_ref(),
                ),
                extra_config: project_admin_system_export_json(
                    mode,
                    provider.extra_config.as_ref(),
                ),
                is_enabled: mode.preserves_active_state() && provider.is_enabled,
            });
        }

        let mut proxy_nodes = self.list_proxy_nodes().await?;
        if mode.credentials_are_exported() {
            for node in &mut proxy_nodes {
                node.proxy_password = self.app().decrypt_proxy_node_password(&node.id).await?;
            }
        }
        let proxy_nodes_data = proxy_nodes
            .iter()
            .map(|node| AdminSystemConfigProxyNode {
                id: Some(node.id.clone()),
                name: Some(node.name.clone()),
                ip: Some(node.ip.clone()),
                port: Some(node.port),
                region: node.region.clone(),
                is_manual: Some(node.is_manual),
                proxy_url: project_admin_system_export_optional_url(
                    mode,
                    node.proxy_url.as_deref(),
                ),
                proxy_username: mode
                    .credentials_are_exported()
                    .then(|| node.proxy_username.clone())
                    .flatten(),
                proxy_password: mode
                    .credentials_are_exported()
                    .then(|| node.proxy_password.clone())
                    .flatten(),
                tunnel_mode: Some(node.tunnel_mode),
                heartbeat_interval: Some(node.heartbeat_interval),
                remote_config: project_admin_system_export_json(mode, node.remote_config.as_ref()),
                config_version: Some(node.config_version),
            })
            .collect::<Vec<_>>();

        let document = AdminSystemConfigDocument {
            version: ADMIN_SYSTEM_CONFIG_EXPORT_VERSION.to_string(),
            exported_at: Utc::now().to_rfc3339(),
            credential_state: mode.credential_state(),
            global_models: global_models_data,
            providers: providers_data,
            proxy_nodes: proxy_nodes_data,
            ldap_config: ldap_data,
            oauth_providers: oauth_data,
            system_configs: system_configs_data,
        };

        serde_json::to_value(document).map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn build_admin_system_users_export_payload(
        &self,
        mode: SystemExportMode,
    ) -> Result<serde_json::Value, GatewayError> {
        let users = self.list_non_admin_export_users().await?;
        let user_ids = users.iter().map(|user| user.id.clone()).collect::<Vec<_>>();
        let user_usage_totals = self
            .app
            .summarize_usage_totals_by_user_ids(&user_ids)
            .await?
            .into_iter()
            .map(|totals| (totals.user_id.clone(), totals))
            .collect::<BTreeMap<_, _>>();
        let user_wallets = self.list_wallet_snapshots_by_user_ids(&user_ids).await?;
        let user_api_keys = self
            .list_auth_api_key_export_records_by_user_ids(&user_ids)
            .await?;
        let groups = self.list_user_groups().await?;
        let providers = self.list_provider_catalog_providers(false).await?;
        let provider_ids = providers
            .iter()
            .map(|provider| provider.id.clone())
            .collect::<Vec<_>>();
        let provider_keys = self
            .list_provider_catalog_keys_by_provider_ids(&provider_ids)
            .await?;
        let memberships = self
            .list_user_group_memberships_by_user_ids(&user_ids)
            .await?;
        let standalone_api_keys = self.list_auth_api_key_export_standalone_records().await?;
        let standalone_api_key_ids = standalone_api_keys
            .iter()
            .map(|key| key.api_key_id.clone())
            .collect::<Vec<_>>();
        let standalone_wallets = self
            .list_wallet_snapshots_by_api_key_ids(&standalone_api_key_ids)
            .await?;
        let usage_aggregates = self.export_admin_system_usage_aggregates().await?;

        let mut recovery_api_key_plaintext_by_id = BTreeMap::<String, String>::new();
        if mode.credentials_are_exported() {
            for key in user_api_keys
                .iter()
                .filter(|key| !key.is_standalone)
                .chain(standalone_api_keys.iter())
            {
                if key.key_encrypted.is_none() {
                    continue;
                }
                let plaintext = decrypt_or_migrate_auth_api_key_secret(self.app(), key).await?;
                if recovery_api_key_plaintext_by_id
                    .insert(key.api_key_id.clone(), plaintext)
                    .is_some()
                {
                    return Err(GatewayError::Internal(format!(
                        "RecoveryBackup API Key ID '{}' is not unique",
                        key.api_key_id,
                    )));
                }
            }
        }

        let wallets_by_user_id = user_wallets
            .into_iter()
            .filter_map(|wallet| wallet.user_id.clone().map(|user_id| (user_id, wallet)))
            .collect::<BTreeMap<_, _>>();
        let wallets_by_api_key_id = standalone_wallets
            .into_iter()
            .filter_map(|wallet| {
                wallet
                    .api_key_id
                    .clone()
                    .map(|api_key_id| (api_key_id, wallet))
            })
            .collect::<BTreeMap<_, _>>();

        let mut api_keys_by_user_id = BTreeMap::<
            String,
            Vec<aether_data::repository::auth::StoredAuthApiKeyExportRecord>,
        >::new();
        for key in user_api_keys.into_iter().filter(|key| !key.is_standalone) {
            api_keys_by_user_id
                .entry(key.user_id.clone())
                .or_default()
                .push(key);
        }
        let mut memberships_by_user_id = BTreeMap::<
            String,
            Vec<aether_data::repository::users::StoredUserGroupMembership>,
        >::new();
        for membership in memberships {
            memberships_by_user_id
                .entry(membership.user_id.clone())
                .or_default()
                .push(membership);
        }
        let user_groups_data = groups
            .iter()
            .map(|group| {
                let (allowed_provider_refs, provider_key_policy_refs) =
                    build_portable_user_group_provider_access(group, &providers, &provider_keys)
                        .map_err(GatewayError::Internal)?;
                Ok(json!({
                    "id": group.id.clone(),
                    "name": group.name.clone(),
                    "description": group.description.clone(),
                    "allowed_providers": group.allowed_providers.clone(),
                    "allowed_provider_refs": allowed_provider_refs,
                    "allowed_providers_mode": group.allowed_providers_mode.clone(),
                    "provider_key_policies": group.provider_key_policies.clone(),
                    "provider_key_policy_refs": provider_key_policy_refs,
                    "allowed_api_formats": group.allowed_api_formats.clone(),
                    "allowed_api_formats_mode": group.allowed_api_formats_mode.clone(),
                    "allowed_models": group.allowed_models.clone(),
                    "allowed_models_mode": group.allowed_models_mode.clone(),
                    "rate_limit": group.rate_limit,
                    "rate_limit_mode": group.rate_limit_mode.clone(),
                }))
            })
            .collect::<Result<Vec<_>, GatewayError>>()?;

        let users_data = users
            .iter()
            .map(|user| {
                let wallet = wallets_by_user_id.get(&user.id);
                let wallet_payload = serialize_admin_system_users_export_wallet(wallet);
                let memberships = memberships_by_user_id.remove(&user.id).unwrap_or_default();
                let group_ids = memberships
                    .iter()
                    .map(|membership| membership.group_id.clone())
                    .collect::<Vec<_>>();
                let group_names = memberships
                    .iter()
                    .map(|membership| membership.group_name.clone())
                    .collect::<Vec<_>>();
                let api_keys = api_keys_by_user_id.remove(&user.id).unwrap_or_default();
                let api_keys_payload = api_keys
                    .iter()
                    .map(|key| {
                        self.build_admin_system_users_export_api_key_payload(
                            key,
                            None,
                            true,
                            mode,
                            recovery_api_key_plaintext_by_id
                                .get(&key.api_key_id)
                                .map(String::as_str),
                        )
                    })
                    .collect::<Result<Vec<_>, GatewayError>>()?;
                let usage_totals = user_usage_totals.get(&user.id);

                let mut payload = json!({
                    "id": user.id.clone(),
                    "email": user.email.clone(),
                    "email_verified": user.email_verified,
                    "username": user.username.clone(),
                    "role": user.role.clone(),
                    "allowed_providers": user.allowed_providers.clone(),
                    "allowed_providers_mode": user.allowed_providers_mode.clone(),
                    "allowed_api_formats": user.allowed_api_formats.clone(),
                    "allowed_api_formats_mode": user.allowed_api_formats_mode.clone(),
                    "allowed_models": user.allowed_models.clone(),
                    "allowed_models_mode": user.allowed_models_mode.clone(),
                    "rate_limit": user.rate_limit,
                    "rate_limit_mode": user.rate_limit_mode.clone(),
                    "model_capability_settings": user.model_capability_settings.clone(),
                    "feature_settings": user.feature_settings.clone(),
                    "group_ids": group_ids,
                    "group_names": group_names,
                    "unlimited": wallet
                        .map(|entry| entry.limit_mode.eq_ignore_ascii_case("unlimited"))
                        .unwrap_or(false),
                    "wallet": wallet_payload,
                    "is_active": user.is_active,
                    "request_count": usage_totals
                        .map(|totals| totals.request_count)
                        .unwrap_or(0),
                    "total_tokens": usage_totals
                        .map(|totals| totals.total_tokens)
                        .unwrap_or(0),
                    "api_keys": api_keys_payload,
                });
                if mode.credentials_are_exported() {
                    payload["password_hash"] = json!(user.password_hash.clone());
                }
                Ok::<_, GatewayError>(payload)
            })
            .collect::<Result<Vec<_>, GatewayError>>()?;

        let standalone_keys_data = standalone_api_keys
            .iter()
            .map(|key| {
                self.build_admin_system_users_export_api_key_payload(
                    key,
                    wallets_by_api_key_id.get(&key.api_key_id),
                    false,
                    mode,
                    recovery_api_key_plaintext_by_id
                        .get(&key.api_key_id)
                        .map(String::as_str),
                )
            })
            .collect::<Result<Vec<_>, GatewayError>>()?;

        Ok(json!({
            "version": mode.users_export_version(),
            "exported_at": Utc::now().to_rfc3339(),
            "user_groups": user_groups_data,
            "users": users_data,
            "standalone_keys": standalone_keys_data,
            "usage_aggregates": usage_aggregates,
        }))
    }

    pub(crate) async fn build_admin_system_data_export_payload(
        &self,
        mode: SystemExportMode,
    ) -> Result<serde_json::Value, GatewayError> {
        let config_data = self.build_admin_system_config_export_payload(mode).await?;
        let user_data = self.build_admin_system_users_export_payload(mode).await?;

        Ok(json!({
            "version": ADMIN_SYSTEM_DATA_EXPORT_VERSION,
            "exported_at": Utc::now().to_rfc3339(),
            "config_data": config_data,
            "user_data": user_data,
        }))
    }

    fn build_admin_system_users_export_api_key_payload(
        &self,
        key: &aether_data::repository::auth::StoredAuthApiKeyExportRecord,
        wallet: Option<&aether_data::repository::wallet::StoredWalletSnapshot>,
        include_is_standalone: bool,
        mode: SystemExportMode,
        recovery_plaintext: Option<&str>,
    ) -> Result<serde_json::Value, GatewayError> {
        let mut payload = serde_json::Map::from_iter([
            ("api_key_id".to_string(), json!(key.api_key_id.clone())),
            ("name".to_string(), json!(key.name.clone())),
            (
                "allowed_providers".to_string(),
                json!(key.allowed_providers.clone()),
            ),
            (
                "allowed_api_formats".to_string(),
                json!(key.allowed_api_formats.clone()),
            ),
            (
                "allowed_models".to_string(),
                json!(key.allowed_models.clone()),
            ),
            ("ip_rules".to_string(), json!(key.ip_rules.clone())),
            ("rate_limit".to_string(), json!(key.rate_limit)),
            ("concurrent_limit".to_string(), json!(key.concurrent_limit)),
            (
                "force_capabilities".to_string(),
                json!(key.force_capabilities.clone()),
            ),
            (
                "feature_settings".to_string(),
                json!(key.feature_settings.clone()),
            ),
            (
                "is_active".to_string(),
                json!(mode.preserves_active_state() && key.is_active),
            ),
            (
                "expires_at".to_string(),
                json!(key.expires_at_unix_secs.and_then(unix_secs_to_rfc3339)),
            ),
            (
                "auto_delete_on_expiry".to_string(),
                json!(key.auto_delete_on_expiry),
            ),
            ("total_requests".to_string(), json!(key.total_requests)),
            ("total_tokens".to_string(), json!(key.total_tokens)),
            ("total_cost_usd".to_string(), json!(key.total_cost_usd)),
            (
                "wallet".to_string(),
                serialize_admin_system_users_export_wallet(wallet)
                    .unwrap_or(serde_json::Value::Null),
            ),
        ]);

        if mode.credentials_are_exported() {
            payload.insert("key_hash".to_string(), json!(key.key_hash.clone()));
            if key.key_encrypted.is_some() {
                let plaintext = recovery_plaintext.ok_or_else(|| {
                    GatewayError::Internal(format!(
                        "RecoveryBackup 无法解密或校验用户 API Key '{}'",
                        key.api_key_id,
                    ))
                })?;
                payload.insert(
                    "key".to_string(),
                    serde_json::Value::String(plaintext.to_string()),
                );
            }
        } else {
            payload.insert(
                "credential_state".to_string(),
                json!(ADMIN_SYSTEM_EXPORT_CREDENTIALS_NOT_EXPORTED),
            );
        }

        if include_is_standalone {
            payload.insert("is_standalone".to_string(), json!(key.is_standalone));
        }

        Ok(serde_json::Value::Object(payload))
    }
}

pub(crate) fn is_interactive_export_private_system_config_key(key: &str) -> bool {
    matches!(
        key.trim().to_ascii_lowercase().as_str(),
        "backup_s3_access_key_id" | "smtp_user" | "turnstile_site_key" | "backup_s3_last_slot"
    )
}

#[cfg(test)]
mod tests {
    use super::{is_interactive_export_private_system_config_key, AdminAppState, SystemExportMode};
    use aether_data::repository::global_models::InMemoryGlobalModelReadRepository;
    use aether_data_contracts::repository::global_models::{
        StoredAdminGlobalModel, StoredAdminProviderModel,
    };
    use std::sync::Arc;

    #[test]
    fn export_modes_keep_interactive_and_recovery_credentials_separate() {
        assert!(!SystemExportMode::InteractiveDownload.credentials_are_exported());
        assert_eq!(
            SystemExportMode::InteractiveDownload.users_export_version(),
            "1.7"
        );
        assert_eq!(
            SystemExportMode::RecoveryBackup.users_export_version(),
            "1.5"
        );
        assert!(SystemExportMode::RecoveryBackup.credentials_are_exported());
        assert!(!SystemExportMode::InteractiveDownload.preserves_active_state());
        assert!(SystemExportMode::RollbackCheckpoint.preserves_active_state());
        assert!(!SystemExportMode::RollbackCheckpoint.credentials_are_exported());
    }

    #[test]
    fn interactive_system_export_omits_credential_companion_fields() {
        for key in [
            "backup_s3_access_key_id",
            "smtp_user",
            "turnstile_site_key",
            "backup_s3_last_slot",
        ] {
            assert!(is_interactive_export_private_system_config_key(key));
        }
        assert!(!is_interactive_export_private_system_config_key(
            "site_name"
        ));
    }

    #[tokio::test]
    async fn system_transfer_model_queries_read_every_page() {
        let global_models = (0..3)
            .map(|index| {
                StoredAdminGlobalModel::new(
                    format!("global-{index}"),
                    format!("model-{index}"),
                    format!("Model {index}"),
                    true,
                    None,
                    None,
                    None,
                    None,
                    0,
                    0,
                    0,
                    Some(index),
                    None,
                )
                .expect("test global model should be valid")
            })
            .collect::<Vec<_>>();
        let provider_models = (0..3)
            .map(|index| StoredAdminProviderModel {
                id: format!("provider-model-{index}"),
                provider_id: "provider-1".to_string(),
                global_model_id: format!("global-{index}"),
                provider_model_name: format!("upstream-model-{index}"),
                provider_model_mappings: None,
                price_per_request: None,
                tiered_pricing: None,
                supports_vision: None,
                supports_function_calling: None,
                supports_streaming: None,
                supports_extended_thinking: None,
                supports_image_generation: None,
                is_active: true,
                is_available: true,
                config: None,
                created_at_unix_ms: Some(index),
                updated_at_unix_secs: None,
                global_model_name: Some(format!("model-{index}")),
                global_model_display_name: Some(format!("Model {index}")),
                global_model_default_price_per_request: None,
                global_model_default_tiered_pricing: None,
                global_model_supported_capabilities: None,
                global_model_config: None,
            })
            .collect::<Vec<_>>();
        let repository = Arc::new(
            InMemoryGlobalModelReadRepository::seed(Vec::new())
                .with_admin_global_models(global_models)
                .with_admin_provider_models(provider_models),
        );
        let app = crate::AppState::new()
            .expect("test app state should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::disabled()
                    .with_global_model_repository_for_tests(repository),
            );
        let state = AdminAppState::new(&app);

        let global_models = state
            .list_all_admin_global_models_for_system_transfer_with_page_limit(2)
            .await
            .expect("all global-model pages should load");
        let provider_models = state
            .list_all_admin_provider_models_for_system_transfer_with_page_limit("provider-1", 2)
            .await
            .expect("all provider-model pages should load");

        assert_eq!(global_models.len(), 3);
        assert_eq!(provider_models.len(), 3);
        assert_eq!(
            global_models
                .iter()
                .map(|model| model.id.as_str())
                .collect::<Vec<_>>(),
            vec!["global-0", "global-1", "global-2"]
        );
        assert_eq!(
            provider_models
                .iter()
                .map(|model| model.id.as_str())
                .collect::<Vec<_>>(),
            vec!["provider-model-2", "provider-model-1", "provider-model-0"]
        );
    }
}
