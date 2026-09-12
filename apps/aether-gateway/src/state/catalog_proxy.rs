use aether_admin::provider::redaction::{
    admin_json_field_has_contextual_secrets, admin_json_field_is_sensitive,
    admin_proxy_credential_field, AdminProxyCredentialField,
};
use aether_crypto::looks_like_python_fernet_ciphertext;
use aether_data_contracts::repository::provider_catalog::{
    ProviderCatalogProxyCasUpdate, StoredProviderCatalogEndpoint, StoredProviderCatalogKey,
    StoredProviderCatalogProvider,
};
use http::StatusCode;
use percent_encoding::percent_decode_str;
use serde_json::{Map, Value};
use url::Url;

use super::AppState;
use crate::handlers::shared::{
    open_runtime_secret_payload, runtime_secret_payload_is_sealed, seal_runtime_secret_payload,
};
use crate::GatewayError;

const PROVIDER_PROXY_USERNAME_PURPOSE: &str = "provider-catalog-provider-proxy-username";
const PROVIDER_PROXY_PASSWORD_PURPOSE: &str = "provider-catalog-provider-proxy-password";
const ENDPOINT_PROXY_USERNAME_PURPOSE: &str = "provider-catalog-endpoint-proxy-username";
const ENDPOINT_PROXY_PASSWORD_PURPOSE: &str = "provider-catalog-endpoint-proxy-password";
const KEY_PROXY_USERNAME_PURPOSE: &str = "provider-catalog-key-proxy-username";
const KEY_PROXY_PASSWORD_PURPOSE: &str = "provider-catalog-key-proxy-password";
const CATALOG_PROXY_SECRET_V2_PREFIX: &str = "aether-provider-catalog-proxy-secret-v2:";
const CATALOG_PROXY_BOUND_PURPOSE_VERSION: &str = "provider-catalog-proxy-credential-v2";
const CATALOG_PROXY_MIGRATION_RETRIES: usize = 8;

#[derive(Debug, Clone, Copy)]
enum CatalogProxyScope {
    Provider,
    Endpoint,
    Key,
}

impl CatalogProxyScope {
    fn label(self) -> &'static str {
        match self {
            Self::Provider => "provider",
            Self::Endpoint => "endpoint",
            Self::Key => "key",
        }
    }

    fn username_purpose(self) -> &'static str {
        match self {
            Self::Provider => PROVIDER_PROXY_USERNAME_PURPOSE,
            Self::Endpoint => ENDPOINT_PROXY_USERNAME_PURPOSE,
            Self::Key => KEY_PROXY_USERNAME_PURPOSE,
        }
    }

    fn password_purpose(self) -> &'static str {
        match self {
            Self::Provider => PROVIDER_PROXY_PASSWORD_PURPOSE,
            Self::Endpoint => ENDPOINT_PROXY_PASSWORD_PURPOSE,
            Self::Key => KEY_PROXY_PASSWORD_PURPOSE,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CatalogProxySource {
    Incoming,
    Stored,
}

struct CatalogProxyProjection {
    runtime: Option<Value>,
    protected: Option<Value>,
    migration_required: bool,
}

struct CatalogProxyCredential {
    plaintext: String,
    protected: String,
    migration_required: bool,
}

impl AppState {
    pub(super) fn protect_provider_catalog_provider(
        &self,
        provider: &StoredProviderCatalogProvider,
    ) -> Result<StoredProviderCatalogProvider, GatewayError> {
        let mut protected = provider.clone();
        protected.proxy = self.protect_catalog_proxy(
            CatalogProxyScope::Provider,
            &provider.id,
            provider.proxy.as_ref(),
        )?;
        Ok(protected)
    }

    pub(super) fn protect_provider_catalog_endpoint(
        &self,
        endpoint: &StoredProviderCatalogEndpoint,
    ) -> Result<StoredProviderCatalogEndpoint, GatewayError> {
        let mut protected = endpoint.clone();
        protected.proxy = self.protect_catalog_proxy(
            CatalogProxyScope::Endpoint,
            &endpoint.id,
            endpoint.proxy.as_ref(),
        )?;
        Ok(protected)
    }

    pub(super) fn protect_provider_catalog_key(
        &self,
        key: &StoredProviderCatalogKey,
    ) -> Result<StoredProviderCatalogKey, GatewayError> {
        let mut protected = self.protect_provider_catalog_key_credentials(key)?;
        protected.proxy =
            self.protect_catalog_proxy(CatalogProxyScope::Key, &key.id, key.proxy.as_ref())?;
        Ok(protected)
    }

    pub(super) async fn open_provider_catalog_providers(
        &self,
        providers: Vec<StoredProviderCatalogProvider>,
    ) -> Result<Vec<StoredProviderCatalogProvider>, GatewayError> {
        let mut opened = Vec::with_capacity(providers.len());
        for provider in providers {
            opened.push(self.open_provider_catalog_provider(provider).await?);
        }
        Ok(opened)
    }

    pub(super) async fn open_provider_catalog_endpoints(
        &self,
        endpoints: Vec<StoredProviderCatalogEndpoint>,
    ) -> Result<Vec<StoredProviderCatalogEndpoint>, GatewayError> {
        let mut opened = Vec::with_capacity(endpoints.len());
        for endpoint in endpoints {
            opened.push(self.open_provider_catalog_endpoint(endpoint).await?);
        }
        Ok(opened)
    }

    pub(super) async fn open_provider_catalog_keys(
        &self,
        keys: Vec<StoredProviderCatalogKey>,
    ) -> Result<Vec<StoredProviderCatalogKey>, GatewayError> {
        let mut opened = Vec::with_capacity(keys.len());
        for key in keys {
            opened.push(self.open_provider_catalog_key(key).await?);
        }
        Ok(opened)
    }

    pub(super) async fn open_provider_catalog_provider(
        &self,
        mut provider: StoredProviderCatalogProvider,
    ) -> Result<StoredProviderCatalogProvider, GatewayError> {
        for _ in 0..CATALOG_PROXY_MIGRATION_RETRIES {
            let projection = self.project_catalog_proxy(
                CatalogProxyScope::Provider,
                &provider.id,
                provider.proxy.as_ref(),
                CatalogProxySource::Stored,
            )?;
            if !projection.migration_required {
                provider.proxy = projection.runtime;
                return Ok(provider);
            }
            self.require_catalog_proxy_migration_writer(CatalogProxyScope::Provider)?;
            let update = ProviderCatalogProxyCasUpdate {
                record_id: provider.id.clone(),
                expected_proxy: provider.proxy.clone(),
                proxy: projection.protected,
            };
            if self
                .data
                .compare_and_swap_provider_catalog_provider_proxy(&update)
                .await
                .map_err(|err| GatewayError::Internal(err.to_string()))?
            {
                provider.proxy = projection.runtime;
                return Ok(provider);
            }
            provider = self
                .data
                .list_provider_catalog_providers_by_ids(std::slice::from_ref(&provider.id))
                .await
                .map_err(|err| GatewayError::Internal(err.to_string()))?
                .into_iter()
                .next()
                .ok_or_else(|| {
                    catalog_proxy_migration_changed_error(CatalogProxyScope::Provider)
                })?;
        }
        Err(catalog_proxy_migration_unstable_error(
            CatalogProxyScope::Provider,
        ))
    }

    pub(super) async fn open_provider_catalog_endpoint(
        &self,
        mut endpoint: StoredProviderCatalogEndpoint,
    ) -> Result<StoredProviderCatalogEndpoint, GatewayError> {
        for _ in 0..CATALOG_PROXY_MIGRATION_RETRIES {
            let projection = self.project_catalog_proxy(
                CatalogProxyScope::Endpoint,
                &endpoint.id,
                endpoint.proxy.as_ref(),
                CatalogProxySource::Stored,
            )?;
            if !projection.migration_required {
                endpoint.proxy = projection.runtime;
                return Ok(endpoint);
            }
            self.require_catalog_proxy_migration_writer(CatalogProxyScope::Endpoint)?;
            let update = ProviderCatalogProxyCasUpdate {
                record_id: endpoint.id.clone(),
                expected_proxy: endpoint.proxy.clone(),
                proxy: projection.protected,
            };
            if self
                .data
                .compare_and_swap_provider_catalog_endpoint_proxy(&update)
                .await
                .map_err(|err| GatewayError::Internal(err.to_string()))?
            {
                endpoint.proxy = projection.runtime;
                return Ok(endpoint);
            }
            endpoint = self
                .data
                .list_provider_catalog_endpoints_by_ids(std::slice::from_ref(&endpoint.id))
                .await
                .map_err(|err| GatewayError::Internal(err.to_string()))?
                .into_iter()
                .next()
                .ok_or_else(|| {
                    catalog_proxy_migration_changed_error(CatalogProxyScope::Endpoint)
                })?;
        }
        Err(catalog_proxy_migration_unstable_error(
            CatalogProxyScope::Endpoint,
        ))
    }

    pub(super) async fn open_provider_catalog_key(
        &self,
        mut key: StoredProviderCatalogKey,
    ) -> Result<StoredProviderCatalogKey, GatewayError> {
        let initial_provider_id = key.provider_id.clone();
        for _ in 0..CATALOG_PROXY_MIGRATION_RETRIES {
            if !self
                .open_provider_catalog_key_credentials_once(&mut key)
                .await?
            {
                key = self
                    .data
                    .list_provider_catalog_keys_by_ids_strong(std::slice::from_ref(&key.id))
                    .await
                    .map_err(|err| GatewayError::Internal(err.to_string()))?
                    .into_iter()
                    .next()
                    .ok_or_else(|| catalog_proxy_migration_changed_error(CatalogProxyScope::Key))?;
                if key.provider_id != initial_provider_id {
                    return Err(GatewayError::Internal(
                        "provider catalog key provider binding changed during credential migration"
                            .to_string(),
                    ));
                }
                continue;
            }
            let projection = self.project_catalog_proxy(
                CatalogProxyScope::Key,
                &key.id,
                key.proxy.as_ref(),
                CatalogProxySource::Stored,
            )?;
            if !projection.migration_required {
                key.proxy = projection.runtime;
                return Ok(key);
            }
            self.require_catalog_proxy_migration_writer(CatalogProxyScope::Key)?;
            let update = ProviderCatalogProxyCasUpdate {
                record_id: key.id.clone(),
                expected_proxy: key.proxy.clone(),
                proxy: projection.protected,
            };
            if self
                .data
                .compare_and_swap_provider_catalog_key_proxy(&update)
                .await
                .map_err(|err| GatewayError::Internal(err.to_string()))?
            {
                key.proxy = projection.runtime;
                return Ok(key);
            }
            key = self
                .data
                .list_provider_catalog_keys_by_ids_strong(std::slice::from_ref(&key.id))
                .await
                .map_err(|err| GatewayError::Internal(err.to_string()))?
                .into_iter()
                .next()
                .ok_or_else(|| catalog_proxy_migration_changed_error(CatalogProxyScope::Key))?;
            if key.provider_id != initial_provider_id {
                return Err(GatewayError::Internal(
                    "provider catalog key provider binding changed during proxy migration"
                        .to_string(),
                ));
            }
        }
        Err(catalog_proxy_migration_unstable_error(
            CatalogProxyScope::Key,
        ))
    }

    pub(super) async fn open_provider_transport_snapshot_once(
        &self,
        snapshot: &mut crate::provider_transport::GatewayProviderTransportSnapshot,
    ) -> Result<bool, GatewayError> {
        let Some(mut stored_key) = self
            .data
            .list_provider_catalog_keys_by_ids_strong(std::slice::from_ref(&snapshot.key.id))
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?
            .into_iter()
            .next()
        else {
            return Ok(false);
        };
        if stored_key.provider_id != snapshot.provider.id
            || stored_key.provider_id != snapshot.key.provider_id
        {
            return Ok(false);
        }
        if !self
            .open_provider_catalog_key_credentials_once(&mut stored_key)
            .await?
        {
            return Ok(false);
        }

        let provider = self.project_catalog_proxy(
            CatalogProxyScope::Provider,
            &snapshot.provider.id,
            snapshot.provider.proxy.as_ref(),
            CatalogProxySource::Stored,
        )?;
        if provider.migration_required {
            self.require_catalog_proxy_migration_writer(CatalogProxyScope::Provider)?;
            if !self
                .data
                .compare_and_swap_provider_catalog_provider_proxy(&ProviderCatalogProxyCasUpdate {
                    record_id: snapshot.provider.id.clone(),
                    expected_proxy: snapshot.provider.proxy.clone(),
                    proxy: provider.protected,
                })
                .await
                .map_err(|err| GatewayError::Internal(err.to_string()))?
            {
                return Ok(false);
            }
        }
        snapshot.provider.proxy = provider.runtime;

        let endpoint = self.project_catalog_proxy(
            CatalogProxyScope::Endpoint,
            &snapshot.endpoint.id,
            snapshot.endpoint.proxy.as_ref(),
            CatalogProxySource::Stored,
        )?;
        if endpoint.migration_required {
            self.require_catalog_proxy_migration_writer(CatalogProxyScope::Endpoint)?;
            if !self
                .data
                .compare_and_swap_provider_catalog_endpoint_proxy(&ProviderCatalogProxyCasUpdate {
                    record_id: snapshot.endpoint.id.clone(),
                    expected_proxy: snapshot.endpoint.proxy.clone(),
                    proxy: endpoint.protected,
                })
                .await
                .map_err(|err| GatewayError::Internal(err.to_string()))?
            {
                return Ok(false);
            }
        }
        snapshot.endpoint.proxy = endpoint.runtime;

        let key = self.project_catalog_proxy(
            CatalogProxyScope::Key,
            &snapshot.key.id,
            snapshot.key.proxy.as_ref(),
            CatalogProxySource::Stored,
        )?;
        if key.migration_required {
            self.require_catalog_proxy_migration_writer(CatalogProxyScope::Key)?;
            if !self
                .data
                .compare_and_swap_provider_catalog_key_proxy(&ProviderCatalogProxyCasUpdate {
                    record_id: snapshot.key.id.clone(),
                    expected_proxy: snapshot.key.proxy.clone(),
                    proxy: key.protected,
                })
                .await
                .map_err(|err| GatewayError::Internal(err.to_string()))?
            {
                return Ok(false);
            }
        }
        snapshot.key.proxy = key.runtime;
        Ok(true)
    }

    fn require_catalog_proxy_migration_writer(
        &self,
        scope: CatalogProxyScope,
    ) -> Result<(), GatewayError> {
        if self.has_provider_catalog_data_writer() {
            Ok(())
        } else {
            Err(GatewayError::Internal(format!(
                "stored {} proxy credentials require migration but the catalog writer is unavailable",
                scope.label()
            )))
        }
    }

    fn protect_catalog_proxy(
        &self,
        scope: CatalogProxyScope,
        record_id: &str,
        proxy: Option<&Value>,
    ) -> Result<Option<Value>, GatewayError> {
        self.project_catalog_proxy(scope, record_id, proxy, CatalogProxySource::Incoming)
            .map(|projection| projection.protected)
    }

    fn project_catalog_proxy(
        &self,
        scope: CatalogProxyScope,
        record_id: &str,
        proxy: Option<&Value>,
        source: CatalogProxySource,
    ) -> Result<CatalogProxyProjection, GatewayError> {
        let Some(proxy) = proxy else {
            return Ok(CatalogProxyProjection {
                runtime: None,
                protected: None,
                migration_required: false,
            });
        };
        if proxy.is_null() {
            return Ok(CatalogProxyProjection {
                runtime: None,
                protected: None,
                migration_required: source == CatalogProxySource::Stored,
            });
        }

        let proxy_was_string = proxy.is_string();
        let mut object = match proxy {
            Value::Object(object) => object.clone(),
            Value::String(url) => {
                Map::from_iter([("url".to_string(), Value::String(url.to_string()))])
            }
            _ => return Err(catalog_proxy_representation_error(scope, source)),
        };

        let mut raw_usernames = Vec::new();
        let mut raw_passwords = Vec::new();
        let mut migration_required = scrub_catalog_proxy_sensitive_fields(
            scope,
            source,
            &mut object,
            true,
            &mut raw_usernames,
            &mut raw_passwords,
        )?;

        let mut normalized_url = None;
        let mut url_username = None;
        let mut url_password = None;
        let url_keys = object
            .keys()
            .filter(|key| matches!(compact_catalog_proxy_key(key).as_str(), "url" | "proxyurl"))
            .cloned()
            .collect::<Vec<_>>();
        for key in url_keys {
            let value = object
                .remove(&key)
                .expect("catalog proxy URL key should still exist");
            if value.is_null() {
                migration_required |= source == CatalogProxySource::Stored;
                continue;
            }
            let Some(raw_url) = value.as_str() else {
                return Err(catalog_proxy_url_error(scope, source));
            };
            let (candidate_url, username, password) =
                parse_catalog_proxy_url(scope, source, raw_url)?;
            merge_catalog_proxy_url(scope, source, &mut normalized_url, candidate_url.clone())?;
            merge_catalog_proxy_url_credential(
                scope,
                source,
                "username",
                &mut url_username,
                username,
            )?;
            merge_catalog_proxy_url_credential(
                scope,
                source,
                "password",
                &mut url_password,
                password,
            )?;
            if key != "url"
                || (candidate_url != raw_url
                    && (!proxy_was_string || stored_catalog_proxy_url_requires_cleanup(raw_url)))
            {
                migration_required |= source == CatalogProxySource::Stored;
            }
        }
        if let Some(url) = normalized_url {
            object.insert("url".to_string(), Value::String(url));
        }

        let mut explicit_username = None;
        for value in raw_usernames {
            let candidate = self.catalog_proxy_credential(
                scope,
                record_id,
                source,
                "username",
                Some(&value),
                scope.username_purpose(),
            )?;
            merge_catalog_proxy_explicit_credential(
                scope,
                source,
                "username",
                &mut explicit_username,
                candidate,
            )?;
        }
        let mut explicit_password = None;
        for value in raw_passwords {
            let candidate = self.catalog_proxy_credential(
                scope,
                record_id,
                source,
                "password",
                Some(&value),
                scope.password_purpose(),
            )?;
            merge_catalog_proxy_explicit_credential(
                scope,
                source,
                "password",
                &mut explicit_password,
                candidate,
            )?;
        }

        let username = merge_catalog_proxy_credential(
            scope,
            record_id,
            source,
            "username",
            explicit_username,
            url_username,
            self,
        )?;
        let password = merge_catalog_proxy_credential(
            scope,
            record_id,
            source,
            "password",
            explicit_password,
            url_password,
            self,
        )?;

        let mut runtime = object.clone();
        let mut protected = object;

        migration_required |= apply_catalog_proxy_credential(
            source,
            "username",
            username,
            &mut runtime,
            &mut protected,
        );
        migration_required |= apply_catalog_proxy_credential(
            source,
            "password",
            password,
            &mut runtime,
            &mut protected,
        );

        Ok(CatalogProxyProjection {
            runtime: Some(Value::Object(runtime)),
            protected: Some(Value::Object(protected)),
            migration_required,
        })
    }

    fn catalog_proxy_credential(
        &self,
        scope: CatalogProxyScope,
        record_id: &str,
        source: CatalogProxySource,
        field: &'static str,
        value: Option<&Value>,
        legacy_purpose: &'static str,
    ) -> Result<Option<CatalogProxyCredential>, GatewayError> {
        let Some(value) = value else {
            return Ok(None);
        };
        if value.is_null() {
            return Ok(Some(CatalogProxyCredential {
                plaintext: String::new(),
                protected: String::new(),
                migration_required: source == CatalogProxySource::Stored,
            }));
        }
        let Some(value) = value.as_str() else {
            return Err(catalog_proxy_credential_error(scope, source, field));
        };
        if value.contains('\0') {
            return Err(match source {
                CatalogProxySource::Incoming => {
                    catalog_proxy_credential_error(scope, source, field)
                }
                CatalogProxySource::Stored => catalog_proxy_storage_error(scope),
            });
        }
        if value.is_empty() {
            return Ok(Some(CatalogProxyCredential {
                plaintext: String::new(),
                protected: String::new(),
                migration_required: source == CatalogProxySource::Stored,
            }));
        }

        if source == CatalogProxySource::Stored && catalog_proxy_secret_is_v2(value) {
            let plaintext = open_catalog_proxy_secret_v2(self, scope, record_id, field, value)
                .ok_or_else(|| catalog_proxy_storage_error(scope))?;
            return Ok(Some(CatalogProxyCredential {
                plaintext,
                protected: value.to_string(),
                migration_required: false,
            }));
        }
        if source == CatalogProxySource::Stored && runtime_secret_payload_is_sealed(value) {
            let plaintext = open_runtime_secret_payload(self, legacy_purpose, value)
                .ok_or_else(|| catalog_proxy_storage_error(scope))?;
            let protected = seal_catalog_proxy_secret_v2(self, scope, record_id, field, &plaintext)
                .ok_or_else(|| catalog_proxy_encryption_error(scope))?;
            return Ok(Some(CatalogProxyCredential {
                plaintext,
                protected,
                migration_required: true,
            }));
        }
        if catalog_proxy_secret_is_v2(value)
            || runtime_secret_payload_is_sealed(value)
            || value.starts_with("aether-")
            || looks_like_python_fernet_ciphertext(value)
        {
            return Err(match source {
                CatalogProxySource::Incoming => {
                    catalog_proxy_credential_error(scope, source, field)
                }
                CatalogProxySource::Stored => catalog_proxy_storage_error(scope),
            });
        }

        let protected = seal_catalog_proxy_secret_v2(self, scope, record_id, field, value)
            .ok_or_else(|| catalog_proxy_encryption_error(scope))?;
        Ok(Some(CatalogProxyCredential {
            plaintext: value.to_string(),
            protected,
            migration_required: source == CatalogProxySource::Stored,
        }))
    }
}

fn catalog_proxy_secret_is_v2(value: &str) -> bool {
    value.starts_with(CATALOG_PROXY_SECRET_V2_PREFIX)
}

fn catalog_proxy_bound_purpose(scope: CatalogProxyScope, record_id: &str, field: &str) -> String {
    format!(
        "{CATALOG_PROXY_BOUND_PURPOSE_VERSION}\0scope={}\0field={field}\0record-id-bytes={}\0{record_id}",
        scope.label(),
        record_id.len()
    )
}

fn seal_catalog_proxy_secret_v2(
    state: &AppState,
    scope: CatalogProxyScope,
    record_id: &str,
    field: &str,
    plaintext: &str,
) -> Option<String> {
    if plaintext.contains('\0') {
        return None;
    }
    let purpose = catalog_proxy_bound_purpose(scope, record_id, field);
    seal_runtime_secret_payload(state, &purpose, plaintext)
        .map(|sealed| format!("{CATALOG_PROXY_SECRET_V2_PREFIX}{sealed}"))
}

fn open_catalog_proxy_secret_v2(
    state: &AppState,
    scope: CatalogProxyScope,
    record_id: &str,
    field: &str,
    stored: &str,
) -> Option<String> {
    // The distinct outer envelope is security-significant: a v2 binding failure
    // must never be retried with the unbound legacy purpose.
    let sealed = stored.strip_prefix(CATALOG_PROXY_SECRET_V2_PREFIX)?;
    let purpose = catalog_proxy_bound_purpose(scope, record_id, field);
    open_runtime_secret_payload(state, &purpose, sealed)
        .filter(|plaintext| !plaintext.contains('\0'))
}

fn scrub_catalog_proxy_sensitive_fields(
    scope: CatalogProxyScope,
    source: CatalogProxySource,
    object: &mut Map<String, Value>,
    at_root: bool,
    usernames: &mut Vec<Value>,
    passwords: &mut Vec<Value>,
) -> Result<bool, GatewayError> {
    let mut changed = false;
    let keys = object.keys().cloned().collect::<Vec<_>>();
    for key in keys {
        let compact_key = compact_catalog_proxy_key(&key);
        if compact_key == "hascredentials" {
            object.remove(&key);
            changed = true;
            continue;
        }

        if let Some(field) = admin_proxy_credential_field(&key) {
            let value = object
                .remove(&key)
                .expect("catalog proxy credential key should still exist");
            let canonical = at_root
                && key
                    == match field {
                        AdminProxyCredentialField::Username => "username",
                        AdminProxyCredentialField::Password => "password",
                    };
            changed |= !canonical || catalog_proxy_sensitive_value_is_unset(&value);
            match field {
                AdminProxyCredentialField::Username => usernames.push(value),
                AdminProxyCredentialField::Password => passwords.push(value),
            }
            continue;
        }

        let is_credential_container = matches!(
            compact_key.as_str(),
            "auth" | "proxyauth" | "credentials" | "proxycredentials"
        );
        if is_credential_container
            && object
                .get(&key)
                .is_some_and(|value| value.is_object() || value.is_array())
        {
            let nested_changed = scrub_catalog_proxy_sensitive_value(
                scope,
                source,
                object
                    .get_mut(&key)
                    .expect("catalog proxy credential container should still exist"),
                usernames,
                passwords,
            )?;
            changed |= nested_changed;
            let is_empty = object.get(&key).is_some_and(|value| match value {
                Value::Object(value) => value.is_empty(),
                Value::Array(value) => value.is_empty(),
                _ => false,
            });
            if is_empty {
                object.remove(&key);
                changed = true;
                continue;
            }
        }

        let value = object
            .get(&key)
            .expect("catalog proxy field should still exist");
        let is_root_proxy_url = at_root && matches!(compact_key.as_str(), "url" | "proxyurl");
        if !is_root_proxy_url && admin_json_field_has_contextual_secrets(&key, value) {
            return Err(catalog_proxy_unsupported_sensitive_error(
                scope, source, &key,
            ));
        }
        if admin_json_field_is_sensitive(&key, value) {
            if catalog_proxy_sensitive_value_is_unset(value) {
                object.remove(&key);
                changed = true;
                continue;
            }
            return Err(catalog_proxy_unsupported_sensitive_error(
                scope, source, &key,
            ));
        }

        changed |= scrub_catalog_proxy_sensitive_value(
            scope,
            source,
            object
                .get_mut(&key)
                .expect("catalog proxy nested field should still exist"),
            usernames,
            passwords,
        )?;
    }
    Ok(changed && source == CatalogProxySource::Stored)
}

fn scrub_catalog_proxy_sensitive_value(
    scope: CatalogProxyScope,
    source: CatalogProxySource,
    value: &mut Value,
    usernames: &mut Vec<Value>,
    passwords: &mut Vec<Value>,
) -> Result<bool, GatewayError> {
    match value {
        Value::Object(object) => {
            scrub_catalog_proxy_sensitive_fields(scope, source, object, false, usernames, passwords)
        }
        Value::Array(values) => {
            let mut changed = false;
            for value in values {
                changed |= scrub_catalog_proxy_sensitive_value(
                    scope, source, value, usernames, passwords,
                )?;
            }
            Ok(changed)
        }
        _ => Ok(false),
    }
}

fn catalog_proxy_sensitive_value_is_unset(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(value) => value.is_empty(),
        Value::Array(value) => value.is_empty(),
        Value::Object(value) => value.is_empty(),
        _ => false,
    }
}

fn compact_catalog_proxy_key(key: &str) -> String {
    key.chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .map(|character| character.to_ascii_lowercase())
        .collect()
}

fn parse_catalog_proxy_url(
    scope: CatalogProxyScope,
    source: CatalogProxySource,
    raw_url: &str,
) -> Result<(String, Option<String>, Option<String>), GatewayError> {
    let raw_url = raw_url.trim();
    let mut parsed = Url::parse(raw_url).map_err(|_| catalog_proxy_url_error(scope, source))?;
    if !matches!(parsed.scheme(), "http" | "https" | "socks5" | "socks5h")
        || parsed.host_str().is_none()
    {
        return Err(catalog_proxy_url_error(scope, source));
    }

    let has_non_root_path = !matches!(parsed.path(), "" | "/");
    let has_disallowed_components =
        has_non_root_path || parsed.query().is_some() || parsed.fragment().is_some();
    if has_disallowed_components && source == CatalogProxySource::Incoming {
        return Err(catalog_proxy_url_error(scope, source));
    }

    let username = (!parsed.username().is_empty() || parsed.password().is_some())
        .then(|| decode_catalog_proxy_userinfo(scope, source, parsed.username()))
        .transpose()?;
    let password = parsed
        .password()
        .map(|value| decode_catalog_proxy_userinfo(scope, source, value))
        .transpose()?
        .filter(|value| !value.is_empty());
    if username.is_some() || parsed.password().is_some() {
        parsed
            .set_username("")
            .map_err(|_| catalog_proxy_url_error(scope, source))?;
        parsed
            .set_password(None)
            .map_err(|_| catalog_proxy_url_error(scope, source))?;
    }
    parsed.set_path("");
    parsed.set_query(None);
    parsed.set_fragment(None);
    Ok((parsed.to_string(), username, password))
}

fn stored_catalog_proxy_url_requires_cleanup(raw_url: &str) -> bool {
    let Ok(parsed) = Url::parse(raw_url.trim()) else {
        return true;
    };
    !parsed.username().is_empty()
        || parsed.password().is_some()
        || !matches!(parsed.path(), "" | "/")
        || parsed.query().is_some()
        || parsed.fragment().is_some()
}

fn merge_catalog_proxy_url(
    scope: CatalogProxyScope,
    source: CatalogProxySource,
    current: &mut Option<String>,
    candidate: String,
) -> Result<(), GatewayError> {
    if current
        .as_ref()
        .is_some_and(|current| current != &candidate)
    {
        return Err(catalog_proxy_ambiguous_url_error(scope, source));
    }
    *current = Some(candidate);
    Ok(())
}

fn decode_catalog_proxy_userinfo(
    scope: CatalogProxyScope,
    source: CatalogProxySource,
    value: &str,
) -> Result<String, GatewayError> {
    percent_decode_str(value)
        .decode_utf8()
        .map(|value| value.into_owned())
        .map_err(|_| catalog_proxy_url_error(scope, source))
}

fn merge_catalog_proxy_url_credential(
    scope: CatalogProxyScope,
    source: CatalogProxySource,
    field: &'static str,
    current: &mut Option<String>,
    candidate: Option<String>,
) -> Result<(), GatewayError> {
    let Some(candidate) = candidate else {
        return Ok(());
    };
    if current
        .as_ref()
        .is_some_and(|current| current != &candidate)
    {
        return Err(catalog_proxy_ambiguous_credential_error(
            scope, source, field,
        ));
    }
    *current = Some(candidate);
    Ok(())
}

fn merge_catalog_proxy_credential(
    scope: CatalogProxyScope,
    record_id: &str,
    source: CatalogProxySource,
    field: &'static str,
    explicit: Option<CatalogProxyCredential>,
    from_url: Option<String>,
    state: &AppState,
) -> Result<Option<CatalogProxyCredential>, GatewayError> {
    let explicit = explicit.filter(|credential| !credential.plaintext.is_empty());
    if let (Some(explicit), Some(from_url)) = (explicit.as_ref(), from_url.as_ref()) {
        if &explicit.plaintext != from_url {
            return Err(catalog_proxy_ambiguous_credential_error(
                scope, source, field,
            ));
        }
    }
    if explicit.is_some() {
        return Ok(explicit);
    }
    let Some(from_url) = from_url.filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    if from_url.contains('\0')
        || catalog_proxy_secret_is_v2(&from_url)
        || runtime_secret_payload_is_sealed(&from_url)
        || from_url.starts_with("aether-")
        || looks_like_python_fernet_ciphertext(&from_url)
    {
        return Err(match source {
            CatalogProxySource::Incoming => catalog_proxy_credential_error(scope, source, field),
            CatalogProxySource::Stored => catalog_proxy_storage_error(scope),
        });
    }
    let protected = seal_catalog_proxy_secret_v2(state, scope, record_id, field, &from_url)
        .ok_or_else(|| catalog_proxy_encryption_error(scope))?;
    Ok(Some(CatalogProxyCredential {
        plaintext: from_url,
        protected,
        migration_required: source == CatalogProxySource::Stored,
    }))
}

fn merge_catalog_proxy_explicit_credential(
    scope: CatalogProxyScope,
    source: CatalogProxySource,
    field: &'static str,
    current: &mut Option<CatalogProxyCredential>,
    candidate: Option<CatalogProxyCredential>,
) -> Result<(), GatewayError> {
    let Some(candidate) = candidate.filter(|credential| !credential.plaintext.is_empty()) else {
        return Ok(());
    };
    let Some(existing) = current.as_mut() else {
        *current = Some(candidate);
        return Ok(());
    };
    if existing.plaintext != candidate.plaintext {
        return Err(catalog_proxy_ambiguous_credential_error(
            scope, source, field,
        ));
    }
    existing.migration_required |= candidate.migration_required;
    Ok(())
}

fn apply_catalog_proxy_credential(
    source: CatalogProxySource,
    field: &'static str,
    credential: Option<CatalogProxyCredential>,
    runtime: &mut Map<String, Value>,
    protected: &mut Map<String, Value>,
) -> bool {
    let existing = runtime.contains_key(field) || protected.contains_key(field);
    let Some(credential) = credential else {
        runtime.remove(field);
        protected.remove(field);
        return existing && source == CatalogProxySource::Stored;
    };
    runtime.insert(field.to_string(), Value::String(credential.plaintext));
    protected.insert(field.to_string(), Value::String(credential.protected));
    credential.migration_required
}

fn catalog_proxy_representation_error(
    scope: CatalogProxyScope,
    source: CatalogProxySource,
) -> GatewayError {
    catalog_proxy_error(
        scope,
        source,
        "proxy must be an object, URL string, or null",
    )
}

fn catalog_proxy_url_error(scope: CatalogProxyScope, source: CatalogProxySource) -> GatewayError {
    catalog_proxy_error(
        scope,
        source,
        "proxy URL must be an http, https, socks5, or socks5h origin without path, query, or fragment",
    )
}

fn catalog_proxy_ambiguous_url_error(
    scope: CatalogProxyScope,
    source: CatalogProxySource,
) -> GatewayError {
    catalog_proxy_error(scope, source, "proxy contains conflicting URL fields")
}

fn catalog_proxy_unsupported_sensitive_error(
    scope: CatalogProxyScope,
    source: CatalogProxySource,
    field: &str,
) -> GatewayError {
    catalog_proxy_error(
        scope,
        source,
        &format!("proxy contains unsupported sensitive field {field}"),
    )
}

fn catalog_proxy_credential_error(
    scope: CatalogProxyScope,
    source: CatalogProxySource,
    field: &'static str,
) -> GatewayError {
    catalog_proxy_error(
        scope,
        source,
        &format!("proxy {field} must be gateway-managed plaintext"),
    )
}

fn catalog_proxy_ambiguous_credential_error(
    scope: CatalogProxyScope,
    source: CatalogProxySource,
    field: &'static str,
) -> GatewayError {
    catalog_proxy_error(
        scope,
        source,
        &format!("proxy URL and proxy {field} contain conflicting credentials"),
    )
}

fn catalog_proxy_error(
    scope: CatalogProxyScope,
    source: CatalogProxySource,
    detail: &str,
) -> GatewayError {
    match source {
        CatalogProxySource::Incoming => GatewayError::Client {
            status: StatusCode::BAD_REQUEST,
            message: format!("{} {detail}", scope.label()),
        },
        CatalogProxySource::Stored => catalog_proxy_storage_error(scope),
    }
}

fn catalog_proxy_storage_error(scope: CatalogProxyScope) -> GatewayError {
    GatewayError::Internal(format!(
        "stored {} proxy credentials cannot be decrypted",
        scope.label()
    ))
}

fn catalog_proxy_encryption_error(scope: CatalogProxyScope) -> GatewayError {
    GatewayError::Internal(format!(
        "{} proxy credential encryption is unavailable",
        scope.label()
    ))
}

fn catalog_proxy_migration_changed_error(scope: CatalogProxyScope) -> GatewayError {
    GatewayError::Internal(format!(
        "stored {} proxy changed during credential migration",
        scope.label()
    ))
}

fn catalog_proxy_migration_unstable_error(scope: CatalogProxyScope) -> GatewayError {
    GatewayError::Internal(format!(
        "stored {} proxy credential migration did not stabilize",
        scope.label()
    ))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use aether_crypto::DEVELOPMENT_ENCRYPTION_KEY;
    use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
    use aether_data_contracts::repository::provider_catalog::{
        ProviderCatalogReadRepository, StoredProviderCatalogEndpoint, StoredProviderCatalogKey,
        StoredProviderCatalogProvider,
    };
    use serde_json::{json, Value};

    use super::{
        catalog_proxy_secret_is_v2, open_catalog_proxy_secret_v2, seal_catalog_proxy_secret_v2,
        CatalogProxyScope, ENDPOINT_PROXY_PASSWORD_PURPOSE, PROVIDER_PROXY_PASSWORD_PURPOSE,
    };
    use crate::data::GatewayDataState;
    use crate::handlers::shared::seal_runtime_secret_payload;
    use crate::{AppState, GatewayError};

    fn sample_provider(id: &str, proxy: Option<Value>) -> StoredProviderCatalogProvider {
        let mut provider = StoredProviderCatalogProvider::new(
            id.to_string(),
            format!("Provider {id}"),
            Some("https://example.test".to_string()),
            "openai".to_string(),
        )
        .expect("provider should build");
        provider.proxy = proxy;
        provider
    }

    fn sample_endpoint(proxy: Option<Value>) -> StoredProviderCatalogEndpoint {
        StoredProviderCatalogEndpoint::new(
            "endpoint-1".to_string(),
            "provider-1".to_string(),
            "openai:chat".to_string(),
            Some("openai".to_string()),
            Some("chat".to_string()),
            true,
        )
        .expect("endpoint should build")
        .with_transport_fields(
            "https://api.example.test/v1".to_string(),
            None,
            None,
            None,
            None,
            None,
            None,
            proxy,
        )
        .expect("endpoint transport should build")
    }

    fn sample_key(proxy: Option<Value>) -> StoredProviderCatalogKey {
        StoredProviderCatalogKey::new(
            "key-1".to_string(),
            "provider-1".to_string(),
            "Key 1".to_string(),
            "api_key".to_string(),
            None,
            true,
        )
        .expect("key should build")
        .with_transport_fields(
            None,
            None::<String>,
            None,
            None,
            None,
            None,
            None,
            proxy,
            None,
        )
        .expect("key transport should build")
    }

    fn state_with_repository(repository: Arc<InMemoryProviderCatalogReadRepository>) -> AppState {
        AppState::new()
            .expect("test state should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(repository)
                    .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            )
    }

    fn encryption_state() -> AppState {
        AppState::new()
            .expect("test state should build")
            .with_data_state_for_tests(
                GatewayDataState::disabled()
                    .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            )
    }

    #[tokio::test]
    async fn provider_proxy_credentials_are_sealed_before_repository_write() {
        let repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            Vec::new(),
            Vec::new(),
            Vec::new(),
        ));
        let state = state_with_repository(Arc::clone(&repository));
        let provider = sample_provider(
            "provider-create",
            Some(json!({
                "url": "http://alice%40example.test:p%3Ass@proxy.example.test:8080",
                "enabled": true
            })),
        );

        let created = state
            .create_provider_catalog_provider(&provider, None)
            .await
            .expect("provider create should succeed")
            .expect("provider writer should exist");
        assert_eq!(
            created.proxy.as_ref().unwrap()["username"],
            "alice@example.test"
        );
        assert_eq!(created.proxy.as_ref().unwrap()["password"], "p:ss");
        assert_eq!(
            created.proxy.as_ref().unwrap()["url"],
            "http://proxy.example.test:8080/"
        );

        let stored = repository
            .list_providers_by_ids(&["provider-create".to_string()])
            .await
            .expect("stored provider should load")
            .pop()
            .expect("stored provider should exist");
        let serialized = serde_json::to_string(&stored.proxy).expect("proxy should serialize");
        assert!(!serialized.contains("alice@example.test"));
        assert!(!serialized.contains("p:ss"));
        let stored_proxy = stored.proxy.as_ref().expect("stored proxy should exist");
        assert!(stored_proxy["username"]
            .as_str()
            .is_some_and(catalog_proxy_secret_is_v2));
        assert!(stored_proxy["password"]
            .as_str()
            .is_some_and(catalog_proxy_secret_is_v2));
        assert_eq!(stored_proxy["url"], "http://proxy.example.test:8080/");
    }

    #[tokio::test]
    async fn legacy_proxy_url_userinfo_is_migrated_with_field_level_cas() {
        let provider = sample_provider(
            "provider-legacy",
            Some(Value::String(
                "http://legacy-user:legacy-pass@proxy.example.test:8080".to_string(),
            )),
        );
        let repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![provider],
            Vec::new(),
            Vec::new(),
        ));
        let state = state_with_repository(Arc::clone(&repository));

        let opened = state
            .list_provider_catalog_providers(false)
            .await
            .expect("legacy provider should migrate")
            .pop()
            .expect("legacy provider should exist");
        assert_eq!(opened.proxy.as_ref().unwrap()["username"], "legacy-user");
        assert_eq!(opened.proxy.as_ref().unwrap()["password"], "legacy-pass");

        let stored = repository
            .list_providers_by_ids(&["provider-legacy".to_string()])
            .await
            .expect("migrated provider should load")
            .pop()
            .expect("migrated provider should exist");
        let stored_proxy = stored.proxy.as_ref().expect("stored proxy should exist");
        assert_eq!(stored_proxy["url"], "http://proxy.example.test:8080/");
        assert!(stored_proxy["username"]
            .as_str()
            .is_some_and(catalog_proxy_secret_is_v2));
        assert!(stored_proxy["password"]
            .as_str()
            .is_some_and(catalog_proxy_secret_is_v2));
    }

    #[tokio::test]
    async fn legacy_unbound_proxy_ciphertext_is_migrated_to_record_bound_v2_with_cas() {
        let encryptor = encryption_state();
        let legacy_password = seal_runtime_secret_payload(
            &encryptor,
            PROVIDER_PROXY_PASSWORD_PURPOSE,
            "legacy-unbound-password",
        )
        .expect("legacy password should seal");
        let provider = sample_provider(
            "provider-legacy-envelope",
            Some(json!({
                "url": "http://proxy.example.test:8080",
                "password": legacy_password
            })),
        );
        let repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![provider],
            Vec::new(),
            Vec::new(),
        ));
        let state = state_with_repository(Arc::clone(&repository));

        let opened = state
            .list_provider_catalog_providers(false)
            .await
            .expect("legacy ciphertext should migrate")
            .pop()
            .expect("legacy provider should exist");
        assert_eq!(
            opened.proxy.as_ref().unwrap()["password"],
            "legacy-unbound-password"
        );

        let stored = repository
            .list_providers_by_ids(&["provider-legacy-envelope".to_string()])
            .await
            .expect("migrated provider should load")
            .pop()
            .expect("migrated provider should exist");
        let migrated = stored.proxy.as_ref().unwrap()["password"]
            .as_str()
            .expect("migrated password should be a string");
        assert!(catalog_proxy_secret_is_v2(migrated));
        assert_eq!(
            open_catalog_proxy_secret_v2(
                &state,
                CatalogProxyScope::Provider,
                "provider-legacy-envelope",
                "password",
                migrated,
            )
            .as_deref(),
            Some("legacy-unbound-password")
        );
    }

    #[tokio::test]
    async fn record_bound_proxy_ciphertext_copied_to_another_record_fails_closed() {
        let state = encryption_state();
        let protected = state
            .protect_provider_catalog_provider(&sample_provider(
                "provider-source",
                Some(json!({
                    "url": "http://proxy.example.test:8080",
                    "username": "bound-user",
                    "password": "bound-password"
                })),
            ))
            .expect("source proxy should seal");
        let copied = sample_provider("provider-target", protected.proxy.clone());

        let error = state
            .open_provider_catalog_provider(copied)
            .await
            .expect_err("cross-record ciphertext copy must fail closed");
        assert!(matches!(error, GatewayError::Internal(_)));
    }

    #[test]
    fn provider_endpoint_and_key_proxy_credentials_use_distinct_purposes() {
        let state = encryption_state();
        let proxy = Some(json!({
            "url": "socks5h://proxy.example.test:1080",
            "username": "purpose-user",
            "password": "purpose-password"
        }));
        let provider = state
            .protect_provider_catalog_provider(&sample_provider("provider-1", proxy.clone()))
            .expect("provider proxy should seal");
        let endpoint = state
            .protect_provider_catalog_endpoint(&sample_endpoint(proxy.clone()))
            .expect("endpoint proxy should seal");
        let key = state
            .protect_provider_catalog_key(&sample_key(proxy))
            .expect("key proxy should seal");

        let provider_password = provider.proxy.as_ref().unwrap()["password"]
            .as_str()
            .unwrap();
        let endpoint_password = endpoint.proxy.as_ref().unwrap()["password"]
            .as_str()
            .unwrap();
        let key_password = key.proxy.as_ref().unwrap()["password"].as_str().unwrap();
        assert_eq!(
            open_catalog_proxy_secret_v2(
                &state,
                CatalogProxyScope::Provider,
                "provider-1",
                "password",
                provider_password,
            )
            .as_deref(),
            Some("purpose-password")
        );
        assert!(open_catalog_proxy_secret_v2(
            &state,
            CatalogProxyScope::Endpoint,
            "endpoint-1",
            "password",
            provider_password,
        )
        .is_none());
        assert_eq!(
            open_catalog_proxy_secret_v2(
                &state,
                CatalogProxyScope::Endpoint,
                "endpoint-1",
                "password",
                endpoint_password,
            )
            .as_deref(),
            Some("purpose-password")
        );
        assert_eq!(
            open_catalog_proxy_secret_v2(
                &state,
                CatalogProxyScope::Key,
                "key-1",
                "password",
                key_password,
            )
            .as_deref(),
            Some("purpose-password")
        );
    }

    #[tokio::test]
    async fn proxy_url_string_uses_runtime_compatible_object_shape() {
        let state = encryption_state();
        let protected = state
            .protect_provider_catalog_provider(&sample_provider(
                "provider-string",
                Some(Value::String("http://proxy.example.test:8080".to_string())),
            ))
            .expect("proxy string should normalize");

        assert_eq!(
            protected.proxy,
            Some(json!({"url": "http://proxy.example.test:8080/"}))
        );

        let opened = state
            .open_provider_catalog_provider(sample_provider(
                "provider-legacy-string",
                Some(Value::String("http://proxy.example.test:8080".to_string())),
            ))
            .await
            .expect("credential-free legacy proxy string should open without a writer");
        assert_eq!(
            opened.proxy,
            Some(json!({"url": "http://proxy.example.test:8080/"}))
        );
    }

    #[tokio::test]
    async fn proxy_credentials_preserve_significant_whitespace() {
        let state = encryption_state();
        let protected = state
            .protect_provider_catalog_provider(&sample_provider(
                "provider-spaces",
                Some(json!({
                    "url": "http://proxy.example.test:8080",
                    "username": " alice ",
                    "password": " pass phrase "
                })),
            ))
            .expect("proxy credentials should seal");
        let stored = serde_json::to_string(&protected.proxy).expect("proxy should serialize");
        assert!(!stored.contains(" alice "));
        assert!(!stored.contains(" pass phrase "));

        let opened = state
            .open_provider_catalog_provider(protected)
            .await
            .expect("sealed proxy credentials should open");
        assert_eq!(opened.proxy.as_ref().unwrap()["username"], " alice ");
        assert_eq!(opened.proxy.as_ref().unwrap()["password"], " pass phrase ");
    }

    #[tokio::test]
    async fn proxy_credential_aliases_are_migrated_to_encrypted_canonical_fields() {
        let state = encryption_state();
        let protected = state
            .protect_provider_catalog_provider(&sample_provider(
                "provider-aliases",
                Some(json!({
                    "proxy_url": "socks5h://proxy.example.test:1080",
                    "proxy_auth": {
                        "proxy_user": "alias-user",
                        "proxy_passphrase": "alias-password"
                    },
                    "region": "test"
                })),
            ))
            .expect("supported proxy credential aliases should normalize");
        let stored_proxy = protected.proxy.as_ref().expect("proxy should exist");
        assert_eq!(stored_proxy["url"], "socks5h://proxy.example.test:1080");
        assert_eq!(stored_proxy["region"], "test");
        assert!(stored_proxy.get("proxy_url").is_none());
        assert!(stored_proxy.get("proxy_auth").is_none());
        assert!(stored_proxy["username"]
            .as_str()
            .is_some_and(catalog_proxy_secret_is_v2));
        assert!(stored_proxy["password"]
            .as_str()
            .is_some_and(catalog_proxy_secret_is_v2));
        let serialized = stored_proxy.to_string();
        assert!(!serialized.contains("alias-user"));
        assert!(!serialized.contains("alias-password"));

        let opened = state
            .open_provider_catalog_provider(protected)
            .await
            .expect("canonical proxy credentials should open");
        assert_eq!(opened.proxy.as_ref().unwrap()["username"], "alias-user");
        assert_eq!(opened.proxy.as_ref().unwrap()["password"], "alias-password");
    }

    #[test]
    fn unsupported_proxy_sensitive_fields_fail_closed() {
        let state = encryption_state();
        for (id, proxy) in [
            (
                "provider-token",
                json!({
                    "url": "http://proxy.example.test:8080",
                    "token": "must-not-persist"
                }),
            ),
            (
                "provider-nested-secret",
                json!({
                    "url": "http://proxy.example.test:8080",
                    "options": {"clientSecret": "must-not-enter-extra"}
                }),
            ),
            (
                "provider-header-map",
                json!({
                    "url": "http://proxy.example.test:8080",
                    "headers": {"x-custom-auth": "must-not-enter-extra"}
                }),
            ),
            (
                "provider-nested-url",
                json!({
                    "url": "http://proxy.example.test:8080",
                    "options": {
                        "health_url": "https://alice:secret@health.example.test/?token=secret"
                    }
                }),
            ),
        ] {
            let error = state
                .protect_provider_catalog_provider(&sample_provider(id, Some(proxy)))
                .expect_err("unsupported sensitive proxy fields must be rejected");
            assert!(matches!(error, GatewayError::Client { .. }));
        }
    }

    #[test]
    fn incoming_proxy_url_is_origin_only_and_scheme_allowlisted() {
        let state = encryption_state();
        for (id, url) in [
            ("provider-ftp", "ftp://proxy.example.test:21"),
            ("provider-path", "http://proxy.example.test:8080/path"),
            (
                "provider-query",
                "http://proxy.example.test:8080/?token=secret",
            ),
            (
                "provider-fragment",
                "socks5://proxy.example.test:1080/#secret",
            ),
        ] {
            let error = state
                .protect_provider_catalog_provider(&sample_provider(id, Some(json!({"url": url}))))
                .expect_err("non-origin proxy URL must be rejected");
            assert!(matches!(error, GatewayError::Client { .. }));
        }
    }

    #[tokio::test]
    async fn stored_proxy_url_components_are_removed_with_cas_migration() {
        let provider = sample_provider(
            "provider-url-cleanup",
            Some(json!({
                "url": "http://legacy-user:legacy-pass@proxy.example.test:8080/path?token=secret#fragment"
            })),
        );
        let repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![provider],
            Vec::new(),
            Vec::new(),
        ));
        let state = state_with_repository(Arc::clone(&repository));

        let opened = state
            .list_provider_catalog_providers(false)
            .await
            .expect("legacy proxy URL should migrate")
            .pop()
            .expect("provider should exist");
        assert_eq!(
            opened.proxy.as_ref().unwrap()["url"],
            "http://proxy.example.test:8080/"
        );
        assert_eq!(opened.proxy.as_ref().unwrap()["username"], "legacy-user");
        assert_eq!(opened.proxy.as_ref().unwrap()["password"], "legacy-pass");

        let stored = repository
            .list_providers_by_ids(&["provider-url-cleanup".to_string()])
            .await
            .expect("migrated provider should load")
            .pop()
            .expect("migrated provider should exist");
        let serialized = stored.proxy.expect("stored proxy should exist").to_string();
        for secret in ["legacy-user", "legacy-pass", "token=secret", "fragment"] {
            assert!(!serialized.contains(secret));
        }
    }

    #[tokio::test]
    async fn password_only_proxy_credentials_remain_compatible() {
        let state = encryption_state();
        let protected = state
            .protect_provider_catalog_provider(&sample_provider(
                "provider-password-only",
                Some(json!({
                    "url": "http://proxy.example.test:8080",
                    "password": "legacy-password"
                })),
            ))
            .expect("password-only proxy should seal");
        assert!(protected.proxy.as_ref().unwrap().get("username").is_none());
        assert!(!protected
            .proxy
            .as_ref()
            .unwrap()
            .to_string()
            .contains("legacy-password"));

        let opened = state
            .open_provider_catalog_provider(protected)
            .await
            .expect("password-only proxy should open");
        assert!(opened.proxy.as_ref().unwrap().get("username").is_none());
        assert_eq!(
            opened.proxy.as_ref().unwrap()["password"],
            "legacy-password"
        );

        let protected_from_url = state
            .protect_provider_catalog_provider(&sample_provider(
                "provider-password-only-url",
                Some(Value::String(
                    "http://:url-password@proxy.example.test:8080".to_string(),
                )),
            ))
            .expect("password-only URL userinfo should normalize");
        let stored_proxy = protected_from_url.proxy.as_ref().unwrap();
        assert_eq!(stored_proxy["url"], "http://proxy.example.test:8080/");
        assert!(stored_proxy.get("username").is_none());
        assert!(stored_proxy["password"]
            .as_str()
            .is_some_and(catalog_proxy_secret_is_v2));
        assert!(!stored_proxy.to_string().contains("url-password"));
    }

    #[tokio::test]
    async fn damaged_or_wrong_purpose_stored_proxy_ciphertext_fails_closed() {
        let encryptor = encryption_state();
        let wrong_purpose = seal_runtime_secret_payload(
            &encryptor,
            ENDPOINT_PROXY_PASSWORD_PURPOSE,
            "must-not-open-as-provider-password",
        )
        .expect("test ciphertext should seal");
        let providers = vec![
            sample_provider(
                "provider-wrong-purpose",
                Some(json!({
                    "url": "http://proxy.example.test:8080",
                    "username": "alice",
                    "password": wrong_purpose
                })),
            ),
            sample_provider(
                "provider-damaged",
                Some(json!({
                    "url": "http://proxy.example.test:8080",
                    "username": "alice",
                    "password": "aether-runtime-secret-v1:not-a-fernet-token"
                })),
            ),
            sample_provider(
                "provider-foreign-envelope",
                Some(json!({
                    "url": "http://proxy.example.test:8080",
                    "username": "alice",
                    "password": "aether-payment-gateway-secret-v2:foreign"
                })),
            ),
        ];
        let repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            providers,
            Vec::new(),
            Vec::new(),
        ));
        let state = state_with_repository(repository);

        for provider_id in [
            "provider-wrong-purpose",
            "provider-damaged",
            "provider-foreign-envelope",
        ] {
            let error = state
                .read_provider_catalog_providers_by_ids(&[provider_id.to_string()])
                .await
                .expect_err("invalid stored ciphertext must fail closed");
            assert!(matches!(&error, GatewayError::Internal(_)));
            assert!(!error.into_message().contains("must-not-open"));
        }
    }

    #[tokio::test]
    async fn incoming_proxy_ciphertext_and_ambiguous_userinfo_are_rejected() {
        let repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            Vec::new(),
            Vec::new(),
            Vec::new(),
        ));
        let state = state_with_repository(Arc::clone(&repository));
        let forged = sample_provider(
            "provider-forged",
            Some(json!({
                "url": "http://proxy.example.test:8080",
                "username": "alice",
                "password": "aether-runtime-secret-v1:not-client-controlled"
            })),
        );
        let ambiguous = sample_provider(
            "provider-ambiguous",
            Some(json!({
                "url": "http://url-user:url-pass@proxy.example.test:8080",
                "username": "different-user",
                "password": "url-pass"
            })),
        );
        let foreign_envelope = sample_provider(
            "provider-foreign-envelope",
            Some(json!({
                "url": "http://proxy.example.test:8080",
                "username": "alice",
                "password": "aether-payment-gateway-secret-v2:foreign"
            })),
        );

        for provider in [&forged, &foreign_envelope, &ambiguous] {
            let error = state
                .create_provider_catalog_provider(provider, None)
                .await
                .expect_err("untrusted proxy credential representation must be rejected");
            assert!(matches!(&error, GatewayError::Client { .. }));
        }
        assert!(repository
            .list_providers(false)
            .await
            .expect("repository should remain readable")
            .is_empty());
    }

    #[test]
    fn provider_username_envelope_is_bound_separately_from_password() {
        let state = encryption_state();
        let sealed = seal_catalog_proxy_secret_v2(
            &state,
            CatalogProxyScope::Provider,
            "provider-field-binding",
            "username",
            "separate-user",
        )
        .expect("username should seal");
        assert!(open_catalog_proxy_secret_v2(
            &state,
            CatalogProxyScope::Provider,
            "provider-field-binding",
            "password",
            &sealed,
        )
        .is_none());
    }
}
