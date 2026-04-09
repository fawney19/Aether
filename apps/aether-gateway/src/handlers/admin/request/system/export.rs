use crate::handlers::admin::request::AdminAppState;
use crate::handlers::admin::system::shared::configs::is_sensitive_admin_system_config_key;
use crate::handlers::admin::system::shared::export::{
    build_admin_system_export_providers_payload, decrypt_admin_system_export_secret,
    ADMIN_SYSTEM_CONFIG_EXPORT_VERSION, ADMIN_SYSTEM_EXPORT_PAGE_LIMIT,
};
use crate::handlers::shared::{system_config_string, unix_secs_to_rfc3339};
use crate::GatewayError;
use aether_admin::system::serialize_admin_system_users_export_wallet;
use aether_data_contracts::repository::global_models::AdminGlobalModelListQuery;
use chrono::Utc;
use serde_json::json;
use std::collections::BTreeMap;

impl<'a> AdminAppState<'a> {
    pub(crate) async fn build_admin_system_config_export_payload(
        &self,
    ) -> Result<serde_json::Value, GatewayError> {
        let global_models = self
            .list_admin_global_models(&AdminGlobalModelListQuery {
                offset: 0,
                limit: ADMIN_SYSTEM_EXPORT_PAGE_LIMIT,
                is_active: None,
                search: None,
            })
            .await?
            .items;
        let global_model_name_by_id = global_models
            .iter()
            .map(|model| (model.id.clone(), model.name.clone()))
            .collect::<BTreeMap<_, _>>();
        let global_models_data = global_models
            .iter()
            .map(|model| {
                json!({
                    "name": model.name,
                    "display_name": model.display_name,
                    "default_price_per_request": model.default_price_per_request,
                    "default_tiered_pricing": model.default_tiered_pricing,
                    "supported_capabilities": model.supported_capabilities,
                    "config": model.config,
                    "is_active": model.is_active,
                })
            })
            .collect::<Vec<_>>();
        let providers_data =
            build_admin_system_export_providers_payload(self, &global_model_name_by_id).await?;

        let ldap_data = self.get_ldap_module_config().await?.map(|config| {
            let bind_password = config
                .bind_password_encrypted
                .as_deref()
                .and_then(|ciphertext| decrypt_admin_system_export_secret(self, ciphertext))
                .unwrap_or_default();
            json!({
                "server_url": config.server_url,
                "bind_dn": config.bind_dn,
                "bind_password": bind_password,
                "base_dn": config.base_dn,
                "user_search_filter": config.user_search_filter,
                "username_attr": config.username_attr,
                "email_attr": config.email_attr,
                "display_name_attr": config.display_name_attr,
                "is_enabled": config.is_enabled,
                "is_exclusive": config.is_exclusive,
                "use_starttls": config.use_starttls,
                "connect_timeout": config.connect_timeout,
            })
        });

        let system_configs = self.list_system_config_entries().await?;
        let system_configs_data = system_configs
            .iter()
            .map(|entry| {
                let value = if is_sensitive_admin_system_config_key(&entry.key) {
                    entry
                        .value
                        .as_str()
                        .and_then(|ciphertext| decrypt_admin_system_export_secret(self, ciphertext))
                        .map(serde_json::Value::String)
                        .unwrap_or_else(|| entry.value.clone())
                } else {
                    entry.value.clone()
                };
                json!({
                    "key": entry.key,
                    "value": value,
                    "description": entry.description,
                })
            })
            .collect::<Vec<_>>();

        let oauth_providers = self.list_oauth_provider_configs().await?;
        let oauth_data = oauth_providers
            .iter()
            .map(|provider| {
                let client_secret = provider
                    .client_secret_encrypted
                    .as_deref()
                    .and_then(|ciphertext| decrypt_admin_system_export_secret(self, ciphertext))
                    .unwrap_or_default();
                json!({
                    "provider_type": provider.provider_type,
                    "display_name": provider.display_name,
                    "client_id": provider.client_id,
                    "client_secret": client_secret,
                    "authorization_url_override": provider.authorization_url_override,
                    "token_url_override": provider.token_url_override,
                    "userinfo_url_override": provider.userinfo_url_override,
                    "scopes": provider.scopes,
                    "redirect_uri": provider.redirect_uri,
                    "frontend_callback_url": provider.frontend_callback_url,
                    "attribute_mapping": provider.attribute_mapping,
                    "extra_config": provider.extra_config,
                    "is_enabled": provider.is_enabled,
                })
            })
            .collect::<Vec<_>>();

        let proxy_nodes = self.list_proxy_nodes().await?;
        let proxy_nodes_data = proxy_nodes
            .iter()
            .map(|node| {
                json!({
                    "id": node.id,
                    "name": node.name,
                    "ip": node.ip,
                    "port": node.port,
                    "region": node.region,
                    "is_manual": node.is_manual,
                    "proxy_url": node.proxy_url,
                    "proxy_username": node.proxy_username,
                    "proxy_password": node.proxy_password,
                    "tunnel_mode": node.tunnel_mode,
                    "heartbeat_interval": node.heartbeat_interval,
                    "remote_config": node.remote_config,
                    "config_version": node.config_version,
                })
            })
            .collect::<Vec<_>>();

        Ok(json!({
            "version": ADMIN_SYSTEM_CONFIG_EXPORT_VERSION,
            "exported_at": Utc::now().to_rfc3339(),
            "global_models": global_models_data,
            "providers": providers_data,
            "proxy_nodes": proxy_nodes_data,
            "ldap_config": ldap_data,
            "oauth_providers": oauth_data,
            "system_configs": system_configs_data,
        }))
    }

    pub(crate) async fn build_admin_system_users_export_payload(
        &self,
    ) -> Result<serde_json::Value, GatewayError> {
        const ADMIN_SYSTEM_USERS_EXPORT_VERSION: &str = "1.3";

        let users = self.list_non_admin_export_users().await?;
        let user_ids = users.iter().map(|user| user.id.clone()).collect::<Vec<_>>();
        let user_wallets = self.list_wallet_snapshots_by_user_ids(&user_ids).await?;
        let user_api_keys = self
            .list_auth_api_key_export_records_by_user_ids(&user_ids)
            .await?;
        let standalone_api_keys = self.list_auth_api_key_export_standalone_records().await?;
        let standalone_api_key_ids = standalone_api_keys
            .iter()
            .map(|key| key.api_key_id.clone())
            .collect::<Vec<_>>();
        let standalone_wallets = self
            .list_wallet_snapshots_by_api_key_ids(&standalone_api_key_ids)
            .await?;

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

        let users_data = users
            .iter()
            .map(|user| {
                let wallet = wallets_by_user_id.get(&user.id);
                let wallet_payload = serialize_admin_system_users_export_wallet(wallet);
                let api_keys = api_keys_by_user_id.remove(&user.id).unwrap_or_default();
                let api_keys_payload = api_keys
                    .iter()
                    .map(|key| {
                        self.build_admin_system_users_export_api_key_payload(key, None, true)
                    })
                    .collect::<Vec<_>>();

                json!({
                    "email": user.email.clone(),
                    "email_verified": user.email_verified,
                    "username": user.username.clone(),
                    "password_hash": user.password_hash.clone(),
                    "role": user.role.clone(),
                    "allowed_providers": user.allowed_providers.clone(),
                    "allowed_api_formats": user.allowed_api_formats.clone(),
                    "allowed_models": user.allowed_models.clone(),
                    "rate_limit": user.rate_limit,
                    "model_capability_settings": user.model_capability_settings.clone(),
                    "unlimited": wallet
                        .map(|entry| entry.limit_mode.eq_ignore_ascii_case("unlimited"))
                        .unwrap_or(false),
                    "wallet": wallet_payload,
                    "is_active": user.is_active,
                    "api_keys": api_keys_payload,
                })
            })
            .collect::<Vec<_>>();

        let standalone_keys_data = standalone_api_keys
            .iter()
            .map(|key| {
                self.build_admin_system_users_export_api_key_payload(
                    key,
                    wallets_by_api_key_id.get(&key.api_key_id),
                    false,
                )
            })
            .collect::<Vec<_>>();

        Ok(json!({
            "version": ADMIN_SYSTEM_USERS_EXPORT_VERSION,
            "exported_at": Utc::now().to_rfc3339(),
            "users": users_data,
            "standalone_keys": standalone_keys_data,
        }))
    }

    fn build_admin_system_users_export_api_key_payload(
        &self,
        key: &aether_data::repository::auth::StoredAuthApiKeyExportRecord,
        wallet: Option<&aether_data::repository::wallet::StoredWalletSnapshot>,
        include_is_standalone: bool,
    ) -> serde_json::Value {
        let mut payload = serde_json::Map::from_iter([
            ("key_hash".to_string(), json!(key.key_hash.clone())),
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
            ("rate_limit".to_string(), json!(key.rate_limit)),
            ("concurrent_limit".to_string(), json!(key.concurrent_limit)),
            (
                "force_capabilities".to_string(),
                json!(key.force_capabilities.clone()),
            ),
            ("is_active".to_string(), json!(key.is_active)),
            (
                "expires_at".to_string(),
                json!(key.expires_at_unix_secs.and_then(unix_secs_to_rfc3339)),
            ),
            (
                "auto_delete_on_expiry".to_string(),
                json!(key.auto_delete_on_expiry),
            ),
            ("total_requests".to_string(), json!(key.total_requests)),
            ("total_cost_usd".to_string(), json!(key.total_cost_usd)),
            (
                "wallet".to_string(),
                serialize_admin_system_users_export_wallet(wallet)
                    .unwrap_or(serde_json::Value::Null),
            ),
        ]);

        if let Some(ciphertext) = key.key_encrypted.as_deref() {
            if let Some(plaintext) = decrypt_admin_system_export_secret(self, ciphertext) {
                payload.insert("key".to_string(), serde_json::Value::String(plaintext));
            } else {
                payload.insert(
                    "key_encrypted".to_string(),
                    serde_json::Value::String(ciphertext.to_string()),
                );
            }
        }

        if include_is_standalone {
            payload.insert("is_standalone".to_string(), json!(key.is_standalone));
        }

        serde_json::Value::Object(payload)
    }
}
