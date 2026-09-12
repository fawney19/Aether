use crate::handlers::shared::{module_available_from_env, system_config_bool};
use crate::{AppState, GatewayError};
use serde_json::json;

#[derive(Clone, Copy)]
struct PublicAuthModuleDefinition {
    name: &'static str,
    display_name: &'static str,
    env_key: &'static str,
    default_available: bool,
}

const PUBLIC_AUTH_MODULE_DEFINITIONS: &[PublicAuthModuleDefinition] = &[
    PublicAuthModuleDefinition {
        name: "oauth",
        display_name: "OAuth 登录",
        env_key: "OAUTH_AVAILABLE",
        default_available: true,
    },
    PublicAuthModuleDefinition {
        name: "ldap",
        display_name: "LDAP 认证",
        env_key: "LDAP_AVAILABLE",
        default_available: true,
    },
];

pub(crate) fn oauth_module_config_is_valid(
    providers: &[aether_data::repository::auth_modules::StoredOAuthProviderModuleConfig],
) -> bool {
    !providers.is_empty()
        && providers.iter().all(|provider| {
            !provider.client_id.trim().is_empty()
                && provider
                    .client_secret_encrypted
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .is_some()
                && !provider.redirect_uri.trim().is_empty()
        })
}

pub(crate) fn ldap_module_config_is_valid(
    config: Option<&aether_data::repository::auth_modules::StoredLdapModuleConfig>,
) -> bool {
    crate::handlers::shared::ldap_module_config_is_valid(config)
}

pub(crate) async fn build_public_auth_modules_status_payload(
    state: &AppState,
) -> Result<serde_json::Value, GatewayError> {
    let oauth_providers = state.list_enabled_oauth_module_providers().await?;
    let ldap_config = state.get_ldap_module_config().await?;
    let oauth_active = oauth_module_config_is_valid(&oauth_providers);
    let ldap_active = ldap_module_config_is_valid(ldap_config.as_ref());

    let mut items = Vec::new();
    for module in PUBLIC_AUTH_MODULE_DEFINITIONS {
        if !module_available_from_env(module.env_key, module.default_available) {
            continue;
        }
        let enabled = state
            .read_system_config_json_value(&format!("module.{}.enabled", module.name))
            .await
            .ok()
            .flatten();
        let enabled = system_config_bool(enabled.as_ref(), false);
        let active = match module.name {
            "oauth" => enabled && oauth_active,
            "ldap" => enabled && ldap_active,
            _ => false,
        };
        items.push(json!({
            "name": module.name,
            "display_name": module.display_name,
            "active": active,
        }));
    }

    Ok(serde_json::Value::Array(items))
}
