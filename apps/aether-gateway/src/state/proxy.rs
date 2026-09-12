use aether_contracts::{ProxySnapshot, PROXY_NODE_TUNNEL_GENERATION_EXTRA_KEY};
use aether_crypto::looks_like_python_fernet_ciphertext;
use aether_data::repository::proxy_nodes::{
    proxy_node_accepts_new_tunnels, ProxyNodeRegistrationMutation,
};
use serde_json::{json, Map, Value};
use std::future::Future;

use super::AppState;
use crate::data::GatewayDataState;
use crate::handlers::shared::{
    open_runtime_secret_payload_with_encryption_key, runtime_secret_payload_is_sealed,
    seal_runtime_secret_payload_with_encryption_key,
};
use crate::provider_transport::{GatewayProviderTransportSnapshot, TransportTunnelAffinityLookup};
use crate::GatewayError;

const TUNNEL_BASE_URL_EXTRA_KEY: &str = "tunnel_base_url";
const TUNNEL_OWNER_INSTANCE_ID_EXTRA_KEY: &str = "tunnel_owner_instance_id";
const TUNNEL_OWNER_OBSERVED_AT_EXTRA_KEY: &str = "tunnel_owner_observed_at_unix_secs";
const PROXY_NODE_PASSWORD_LEGACY_SECRET_PURPOSE: &str = "proxy-node-password";
const PROXY_TUNNEL_PSK_LEGACY_SECRET_PURPOSE: &str = "proxy-node-tunnel-psk";
const PROXY_NODE_SECRET_ENVELOPE_FAMILY_PREFIX: &str = "aether-proxy-node-secret-";
const PROXY_NODE_SECRET_V2_PREFIX: &str = "aether-proxy-node-secret-v2:";
const RUNTIME_SECRET_ENVELOPE_FAMILY_PREFIX: &str = "aether-runtime-secret-";
const PROXY_NODE_BOUND_PURPOSE_VERSION: &str = "proxy-node-secret-bound-v2";
const PROXY_NODE_PASSWORD_SCOPE: &str = "manual-proxy";
const PROXY_NODE_PASSWORD_FIELD: &str = "password";
const PROXY_TUNNEL_PSK_SCOPE: &str = "tunnel-security";
const PROXY_TUNNEL_PSK_FIELD: &str = "pre-shared-key";
const PROXY_NODE_SECRET_MIGRATION_RETRIES: usize = 8;
const PROXY_UNAVAILABLE_REASON_EXTRA_KEY: &str = "aether_proxy_unavailable_reason";

pub(crate) fn unavailable_proxy_snapshot(reason: &str) -> ProxySnapshot {
    let extra = Map::from_iter([(
        PROXY_UNAVAILABLE_REASON_EXTRA_KEY.to_string(),
        Value::String(reason.to_string()),
    )]);
    ProxySnapshot {
        enabled: Some(true),
        mode: Some("unavailable".to_string()),
        node_id: None,
        label: None,
        url: None,
        extra: Some(Value::Object(extra)),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProxyTunnelPskBinding {
    pub key: String,
    pub tunnel_generation: String,
}

pub(crate) async fn decrypt_or_migrate_proxy_tunnel_psk(
    data: &GatewayDataState,
    node_id: &str,
) -> Result<Option<String>, aether_data::DataLayerError> {
    Ok(decrypt_or_migrate_proxy_tunnel_psk_binding(data, node_id)
        .await?
        .map(|binding| binding.key))
}

pub(crate) async fn decrypt_or_migrate_proxy_tunnel_psk_binding(
    data: &GatewayDataState,
    node_id: &str,
) -> Result<Option<ProxyTunnelPskBinding>, aether_data::DataLayerError> {
    for _ in 0..PROXY_NODE_SECRET_MIGRATION_RETRIES {
        let Some(node) = data.find_proxy_node(node_id).await? else {
            return Ok(None);
        };
        let persistent_node_id = node.id.clone();
        let tunnel_generation = node.tunnel_generation.clone();
        let Some(observed_metadata) = node.proxy_metadata else {
            return Ok(None);
        };
        let encrypted = observed_metadata
            .pointer("/tunnel_security/encryption_key_encrypted")
            .map(|value| value.as_str().ok_or_else(proxy_tunnel_psk_storage_error))
            .transpose()?;
        let legacy = observed_metadata
            .pointer("/tunnel_security/encryption_key")
            .map(|value| value.as_str().ok_or_else(proxy_tunnel_psk_storage_error))
            .transpose()?;

        if let Some(stored) = encrypted {
            let (plaintext, replacement_ciphertext) = if proxy_node_secret_is_v2(stored) {
                (
                    open_proxy_node_secret_v2_with_encryption_key(
                        data.encryption_key(),
                        PROXY_TUNNEL_PSK_SCOPE,
                        PROXY_TUNNEL_PSK_FIELD,
                        &persistent_node_id,
                        stored,
                    )
                    .ok_or_else(proxy_tunnel_psk_storage_error)?,
                    None,
                )
            } else if runtime_secret_payload_is_sealed(stored) {
                let plaintext = open_runtime_secret_payload_with_encryption_key(
                    data.encryption_key(),
                    PROXY_TUNNEL_PSK_LEGACY_SECRET_PURPOSE,
                    stored,
                )
                .ok_or_else(proxy_tunnel_psk_storage_error)?;
                let replacement = seal_proxy_node_secret_v2_with_encryption_key(
                    data.encryption_key(),
                    PROXY_TUNNEL_PSK_SCOPE,
                    PROXY_TUNNEL_PSK_FIELD,
                    &persistent_node_id,
                    &plaintext,
                )
                .ok_or_else(proxy_tunnel_psk_migration_error)?;
                (plaintext, Some(replacement))
            } else {
                return Err(proxy_tunnel_psk_storage_error());
            };
            aether_contracts::tunnel_security::decode_psk(&plaintext)
                .map_err(|_| proxy_tunnel_psk_storage_error())?;
            if replacement_ciphertext.is_none() && legacy.is_none() {
                return Ok(Some(ProxyTunnelPskBinding {
                    key: plaintext,
                    tunnel_generation,
                }));
            }

            let replacement = proxy_metadata_with_encrypted_tunnel_psk(
                observed_metadata.clone(),
                replacement_ciphertext.unwrap_or_else(|| stored.to_string()),
            )?;
            if data
                .compare_and_set_proxy_node_metadata(
                    &persistent_node_id,
                    &observed_metadata,
                    &replacement,
                )
                .await?
            {
                return Ok(Some(ProxyTunnelPskBinding {
                    key: plaintext,
                    tunnel_generation,
                }));
            }
            continue;
        }

        let Some(legacy) = legacy else {
            return Ok(None);
        };
        if looks_like_python_fernet_ciphertext(legacy)
            || legacy.starts_with(RUNTIME_SECRET_ENVELOPE_FAMILY_PREFIX)
            || legacy.starts_with(PROXY_NODE_SECRET_ENVELOPE_FAMILY_PREFIX)
        {
            return Err(proxy_tunnel_psk_storage_error());
        }
        aether_contracts::tunnel_security::decode_psk(legacy)
            .map_err(|_| proxy_tunnel_psk_storage_error())?;
        let encrypted = seal_proxy_node_secret_v2_with_encryption_key(
            data.encryption_key(),
            PROXY_TUNNEL_PSK_SCOPE,
            PROXY_TUNNEL_PSK_FIELD,
            &persistent_node_id,
            legacy,
        )
        .ok_or_else(proxy_tunnel_psk_migration_error)?;
        let replacement =
            proxy_metadata_with_encrypted_tunnel_psk(observed_metadata.clone(), encrypted)?;
        if data
            .compare_and_set_proxy_node_metadata(
                &persistent_node_id,
                &observed_metadata,
                &replacement,
            )
            .await?
        {
            return Ok(Some(ProxyTunnelPskBinding {
                key: legacy.to_string(),
                tunnel_generation,
            }));
        }
    }

    Err(proxy_tunnel_psk_migration_error())
}

fn proxy_metadata_with_encrypted_tunnel_psk(
    mut metadata: Value,
    encrypted: String,
) -> Result<Value, aether_data::DataLayerError> {
    let tunnel_security = metadata
        .as_object_mut()
        .and_then(|metadata| metadata.get_mut("tunnel_security"))
        .and_then(Value::as_object_mut)
        .ok_or_else(proxy_tunnel_psk_storage_error)?;
    tunnel_security.remove("encryption_key");
    tunnel_security.insert(
        "encryption_key_encrypted".to_string(),
        Value::String(encrypted),
    );
    Ok(metadata)
}

fn proxy_tunnel_psk_storage_error() -> aether_data::DataLayerError {
    aether_data::DataLayerError::UnexpectedValue(
        "stored proxy tunnel security key cannot be decrypted".to_string(),
    )
}

fn proxy_tunnel_psk_migration_error() -> aether_data::DataLayerError {
    aether_data::DataLayerError::UnexpectedValue(
        "proxy tunnel security key migration did not stabilize".to_string(),
    )
}

fn proxy_node_password_error() -> GatewayError {
    GatewayError::Internal("stored proxy node password cannot be decrypted".to_string())
}

fn proxy_node_secret_is_v2(value: &str) -> bool {
    value.starts_with(PROXY_NODE_SECRET_V2_PREFIX)
}

fn proxy_node_bound_secret_purpose(scope: &str, field: &str, node_id: &str) -> String {
    format!(
        "{PROXY_NODE_BOUND_PURPOSE_VERSION}\0scope-bytes={}\0{scope}\0field-bytes={}\0{field}\0node-id-bytes={}\0{node_id}",
        scope.len(),
        field.len(),
        node_id.len(),
    )
}

fn seal_proxy_node_secret_v2_with_encryption_key(
    encryption_key: Option<&str>,
    scope: &str,
    field: &str,
    node_id: &str,
    plaintext: &str,
) -> Option<String> {
    let purpose = proxy_node_bound_secret_purpose(scope, field, node_id);
    seal_runtime_secret_payload_with_encryption_key(encryption_key, &purpose, plaintext)
        .map(|sealed| format!("{PROXY_NODE_SECRET_V2_PREFIX}{sealed}"))
}

fn open_proxy_node_secret_v2_with_encryption_key(
    encryption_key: Option<&str>,
    scope: &str,
    field: &str,
    node_id: &str,
    stored: &str,
) -> Option<String> {
    // The distinct outer envelope is security-significant. A v2 binding
    // failure must never be retried with the unbound legacy purpose.
    let sealed = stored.strip_prefix(PROXY_NODE_SECRET_V2_PREFIX)?;
    let purpose = proxy_node_bound_secret_purpose(scope, field, node_id);
    open_runtime_secret_payload_with_encryption_key(encryption_key, &purpose, sealed)
}

fn incoming_proxy_node_secret_is_ciphertext(value: &str) -> bool {
    let value = value.trim();
    value.starts_with(PROXY_NODE_SECRET_ENVELOPE_FAMILY_PREFIX)
        || value.starts_with(RUNTIME_SECRET_ENVELOPE_FAMILY_PREFIX)
        || looks_like_python_fernet_ciphertext(value)
}

impl AppState {
    pub(super) async fn register_proxy_node_with_bound_secrets(
        &self,
        mutation: &ProxyNodeRegistrationMutation,
    ) -> Result<Option<aether_data::repository::proxy_nodes::StoredProxyNode>, GatewayError> {
        let mut new_node_id = mutation
            .node_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

        for _ in 0..PROXY_NODE_SECRET_MIGRATION_RETRIES {
            let nodes = self
                .data
                .list_proxy_nodes()
                .await
                .map_err(|err| GatewayError::Internal(err.to_string()))?;
            let node_id = proxy_node_registration_identity(&nodes, mutation)
                .unwrap_or_else(|| new_node_id.clone());
            let protected = self.protect_proxy_node_registration_mutation(&node_id, mutation)?;
            match self.data.register_proxy_node(&protected).await {
                Ok(Some(node)) if node.id == node_id => return Ok(Some(node)),
                Ok(Some(_)) => {
                    return Err(GatewayError::Internal(
                        "proxy node repository changed the protected node identity".to_string(),
                    ));
                }
                Ok(None) => return Ok(None),
                Err(error) => {
                    let latest = self
                        .data
                        .list_proxy_nodes()
                        .await
                        .map_err(|err| GatewayError::Internal(err.to_string()))?;
                    let Some(latest_node_id) = proxy_node_registration_identity(&latest, mutation)
                    else {
                        return Err(GatewayError::Internal(error.to_string()));
                    };
                    if latest_node_id == node_id {
                        return Err(GatewayError::Internal(error.to_string()));
                    }
                    new_node_id = latest_node_id;
                }
            }
        }

        Err(GatewayError::Internal(
            "proxy node registration identity did not stabilize".to_string(),
        ))
    }

    pub(super) fn protect_proxy_node_registration_mutation(
        &self,
        node_id: &str,
        mutation: &ProxyNodeRegistrationMutation,
    ) -> Result<ProxyNodeRegistrationMutation, GatewayError> {
        let mut protected = mutation.clone();
        protected.node_id = Some(node_id.to_string());
        let Some(metadata) = protected.proxy_metadata.as_mut() else {
            return Ok(protected);
        };
        let Some(tunnel_security) = metadata
            .as_object_mut()
            .and_then(|metadata| metadata.get_mut("tunnel_security"))
            .and_then(Value::as_object_mut)
        else {
            return Ok(protected);
        };
        if tunnel_security.contains_key("encryption_key_encrypted") {
            return Err(GatewayError::Internal(
                "proxy tunnel security ciphertext must be created by the gateway".to_string(),
            ));
        }
        let plaintext = tunnel_security
            .remove("encryption_key")
            .map(|value| {
                value.as_str().map(ToOwned::to_owned).ok_or_else(|| {
                    GatewayError::Internal(
                        "proxy tunnel security key has an invalid stored representation"
                            .to_string(),
                    )
                })
            })
            .transpose()?;
        let Some(plaintext) = plaintext else {
            return Ok(protected);
        };
        if incoming_proxy_node_secret_is_ciphertext(&plaintext) {
            return Err(GatewayError::Internal(
                "proxy tunnel security ciphertext must be created by the gateway".to_string(),
            ));
        }
        aether_contracts::tunnel_security::decode_psk(&plaintext).map_err(|_| {
            GatewayError::Internal("proxy tunnel security key is invalid".to_string())
        })?;
        let encrypted = seal_proxy_node_secret_v2_with_encryption_key(
            self.encryption_key(),
            PROXY_TUNNEL_PSK_SCOPE,
            PROXY_TUNNEL_PSK_FIELD,
            node_id,
            plaintext.as_str(),
        )
        .ok_or_else(|| {
            GatewayError::Internal("proxy tunnel security encryption is unavailable".to_string())
        })?;
        tunnel_security.insert(
            "encryption_key_encrypted".to_string(),
            Value::String(encrypted),
        );
        Ok(protected)
    }

    pub(super) fn protect_proxy_node_password(
        &self,
        node_id: &str,
        plaintext: &str,
    ) -> Result<String, GatewayError> {
        if incoming_proxy_node_secret_is_ciphertext(plaintext) {
            return Err(GatewayError::Internal(
                "proxy node password ciphertext must be created by the gateway".to_string(),
            ));
        }
        seal_proxy_node_secret_v2_with_encryption_key(
            self.encryption_key(),
            PROXY_NODE_PASSWORD_SCOPE,
            PROXY_NODE_PASSWORD_FIELD,
            node_id,
            plaintext,
        )
        .ok_or_else(|| {
            GatewayError::Internal("proxy node password encryption is unavailable".to_string())
        })
    }

    pub(crate) async fn decrypt_proxy_node_password(
        &self,
        node_id: &str,
    ) -> Result<Option<String>, GatewayError> {
        self.decrypt_proxy_node_password_with_before_compare(node_id, || async {})
            .await
    }

    async fn decrypt_proxy_node_password_with_before_compare<BeforeCompare, CompareFuture>(
        &self,
        node_id: &str,
        before_compare: BeforeCompare,
    ) -> Result<Option<String>, GatewayError>
    where
        BeforeCompare: Fn() -> CompareFuture,
        CompareFuture: Future<Output = ()>,
    {
        for _ in 0..PROXY_NODE_SECRET_MIGRATION_RETRIES {
            let Some(node) = self.find_proxy_node(node_id).await? else {
                return Ok(None);
            };
            let persistent_node_id = node.id.clone();
            let Some(observed) = node.proxy_password else {
                return Ok(None);
            };
            if proxy_node_secret_is_v2(&observed) {
                return open_proxy_node_secret_v2_with_encryption_key(
                    self.encryption_key(),
                    PROXY_NODE_PASSWORD_SCOPE,
                    PROXY_NODE_PASSWORD_FIELD,
                    &persistent_node_id,
                    &observed,
                )
                .map(Some)
                .ok_or_else(proxy_node_password_error);
            }
            let observed_shape = observed.trim();
            if observed_shape.starts_with(PROXY_NODE_SECRET_ENVELOPE_FAMILY_PREFIX)
                || (observed_shape.starts_with(RUNTIME_SECRET_ENVELOPE_FAMILY_PREFIX)
                    && !runtime_secret_payload_is_sealed(&observed))
                || looks_like_python_fernet_ciphertext(&observed)
            {
                return Err(proxy_node_password_error());
            }

            let plaintext = if runtime_secret_payload_is_sealed(&observed) {
                open_runtime_secret_payload_with_encryption_key(
                    self.encryption_key(),
                    PROXY_NODE_PASSWORD_LEGACY_SECRET_PURPOSE,
                    &observed,
                )
                .ok_or_else(proxy_node_password_error)?
            } else {
                observed.clone()
            };
            let encrypted = seal_proxy_node_secret_v2_with_encryption_key(
                self.encryption_key(),
                PROXY_NODE_PASSWORD_SCOPE,
                PROXY_NODE_PASSWORD_FIELD,
                &persistent_node_id,
                &plaintext,
            )
            .ok_or_else(|| {
                GatewayError::Internal("proxy node password encryption is unavailable".to_string())
            })?;
            before_compare().await;
            if self
                .data
                .compare_and_set_proxy_node_password(&persistent_node_id, &observed, &encrypted)
                .await
                .map_err(|err| GatewayError::Internal(err.to_string()))?
            {
                return Ok(Some(plaintext));
            }
        }

        Err(GatewayError::Internal(
            "proxy node password migration did not stabilize".to_string(),
        ))
    }

    pub(crate) async fn read_system_proxy_node_id(&self) -> Option<String> {
        self.read_system_config_json_value("system_proxy_node_id")
            .await
            .ok()
            .flatten()
            .and_then(|value| value.as_str().map(str::trim).map(ToOwned::to_owned))
            .filter(|value| !value.is_empty())
    }

    pub(crate) async fn resolve_proxy_node_snapshot(
        &self,
        node_id: Option<&str>,
    ) -> Option<ProxySnapshot> {
        let node_id = node_id.map(str::trim).filter(|value| !value.is_empty())?;
        let node = self.find_proxy_node(node_id).await.ok().flatten()?;
        if node.status.trim() != "online" {
            return None;
        }
        if !proxy_node_accepts_new_tunnels(&node) {
            return None;
        }
        if node.tunnel_mode && node.tunnel_connected {
            let mut extra = Map::new();
            let owner = self
                .lookup_tunnel_attachment_owner(node_id)
                .await
                .ok()
                .flatten();
            if let Some(owner) = owner {
                extra.insert(
                    TUNNEL_BASE_URL_EXTRA_KEY.to_string(),
                    Value::String(owner.relay_base_url),
                );
                extra.insert(
                    TUNNEL_OWNER_INSTANCE_ID_EXTRA_KEY.to_string(),
                    Value::String(owner.gateway_instance_id),
                );
                extra.insert(
                    TUNNEL_OWNER_OBSERVED_AT_EXTRA_KEY.to_string(),
                    json!(owner.observed_at_unix_secs),
                );
            } else if !self.tunnel.has_local_proxy(node_id) {
                return None;
            }
            extra.insert(
                PROXY_NODE_TUNNEL_GENERATION_EXTRA_KEY.to_string(),
                Value::String(node.tunnel_generation.clone()),
            );
            return Some(ProxySnapshot {
                enabled: Some(true),
                mode: Some("tunnel".to_string()),
                node_id: Some(node.id),
                label: Some(node.name),
                url: None,
                extra: if extra.is_empty() {
                    None
                } else {
                    Some(Value::Object(extra))
                },
            });
        }
        if !node.is_manual {
            return None;
        }
        let proxy_password = match self.decrypt_proxy_node_password(&node.id).await {
            Ok(password) => password,
            Err(_) => return None,
        };
        let proxy_url = node
            .proxy_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())?;
        let proxy_url = proxy_url_with_node_auth(
            proxy_url,
            node.proxy_username.as_deref(),
            proxy_password.as_deref(),
        )?;
        let mut extra = Map::new();
        extra.insert(
            PROXY_NODE_TUNNEL_GENERATION_EXTRA_KEY.to_string(),
            Value::String(node.tunnel_generation.clone()),
        );
        Some(ProxySnapshot {
            enabled: Some(true),
            mode: proxy_mode_from_url(Some(&proxy_url)),
            node_id: Some(node.id),
            label: Some(node.name),
            url: Some(proxy_url),
            extra: Some(Value::Object(extra)),
        })
    }

    pub(crate) async fn resolve_system_proxy_snapshot(&self) -> Option<ProxySnapshot> {
        let node_id = self.read_system_proxy_node_id().await;
        let node_id = node_id.as_deref()?;
        self.resolve_proxy_node_snapshot(Some(node_id))
            .await
            .or_else(|| Some(unavailable_proxy_snapshot("system_proxy_node_unavailable")))
    }

    pub(crate) async fn resolve_transport_proxy_snapshot_with_tunnel_affinity(
        &self,
        transport: &GatewayProviderTransportSnapshot,
    ) -> Option<ProxySnapshot> {
        self.resolve_transport_proxy_with_source_with_tunnel_affinity(transport)
            .await
            .map(|(snapshot, _)| snapshot)
    }

    pub(crate) async fn resolve_transport_proxy_source_with_tunnel_affinity(
        &self,
        transport: &GatewayProviderTransportSnapshot,
    ) -> Option<&'static str> {
        self.resolve_transport_proxy_with_source_with_tunnel_affinity(transport)
            .await
            .map(|(_, source)| source)
    }

    pub(crate) async fn resolve_configured_proxy_snapshot_with_tunnel_affinity(
        &self,
        raw: Option<&Value>,
    ) -> Option<ProxySnapshot> {
        let object = raw?.as_object()?;
        if !proxy_enabled(object) {
            return None;
        }

        if json_field_is_explicit(object, "node_id") {
            let node_id = json_string_field(object, "node_id");
            if let Some(snapshot) = self.resolve_proxy_node_snapshot(node_id.as_deref()).await {
                return Some(snapshot);
            }
            return Some(unavailable_proxy_snapshot(
                "configured_proxy_node_unavailable",
            ));
        }

        if json_field_is_explicit(object, "url") || json_field_is_explicit(object, "proxy_url") {
            let Some(snapshot) = proxy_snapshot_from_object(object) else {
                return Some(unavailable_proxy_snapshot(
                    "configured_proxy_url_unavailable",
                ));
            };
            if snapshot.url.is_none() {
                return Some(unavailable_proxy_snapshot(
                    "configured_proxy_url_unavailable",
                ));
            }
            return Some(snapshot);
        }

        None
    }

    async fn resolve_transport_proxy_with_source_with_tunnel_affinity(
        &self,
        transport: &GatewayProviderTransportSnapshot,
    ) -> Option<(ProxySnapshot, &'static str)> {
        if let Some(snapshot) = self
            .resolve_configured_proxy_snapshot_with_tunnel_affinity(transport.key.proxy.as_ref())
            .await
        {
            return Some((snapshot, "key"));
        }
        if let Some(snapshot) = self
            .resolve_configured_proxy_snapshot_with_tunnel_affinity(
                transport.endpoint.proxy.as_ref(),
            )
            .await
        {
            return Some((snapshot, "endpoint"));
        }
        if let Some(snapshot) = self
            .resolve_configured_proxy_snapshot_with_tunnel_affinity(
                transport.provider.proxy.as_ref(),
            )
            .await
        {
            return Some((snapshot, "provider"));
        }
        self.resolve_system_proxy_snapshot()
            .await
            .map(|snapshot| (snapshot, "system"))
    }
}

fn proxy_node_registration_identity(
    nodes: &[aether_data::repository::proxy_nodes::StoredProxyNode],
    mutation: &ProxyNodeRegistrationMutation,
) -> Option<String> {
    nodes
        .iter()
        .filter(|node| !node.is_manual && node.ip == mutation.ip && node.port == mutation.port)
        .min_by(|left, right| {
            left.created_at_unix_ms
                .unwrap_or(u64::MAX)
                .cmp(&right.created_at_unix_ms.unwrap_or(u64::MAX))
                .then(left.id.cmp(&right.id))
        })
        .map(|node| node.id.clone())
}

fn proxy_enabled(object: &Map<String, Value>) -> bool {
    object
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(true)
}

fn proxy_snapshot_from_object(object: &Map<String, Value>) -> Option<ProxySnapshot> {
    let mode = json_string_field(object, "mode");
    let node_id = json_string_field(object, "node_id");
    let label = json_string_field(object, "label");
    let url = json_string_field(object, "url")
        .or_else(|| json_string_field(object, "proxy_url"))
        .and_then(|proxy_url| {
            proxy_url_with_node_auth(
                &proxy_url,
                json_proxy_credential_field(object, "username"),
                json_proxy_credential_field(object, "password"),
            )
        });

    if node_id.is_none() && url.is_none() {
        return None;
    }

    let mut extra = Map::new();
    for (key, value) in object {
        if matches!(
            key.as_str(),
            "enabled"
                | "mode"
                | "node_id"
                | "label"
                | "url"
                | "proxy_url"
                | "username"
                | "password"
        ) {
            continue;
        }
        extra.insert(key.clone(), value.clone());
    }

    Some(ProxySnapshot {
        enabled: object.get("enabled").and_then(Value::as_bool),
        mode,
        node_id,
        label,
        url,
        extra: if extra.is_empty() {
            None
        } else {
            Some(Value::Object(extra))
        },
    })
}

fn json_field_is_explicit(object: &Map<String, Value>, key: &str) -> bool {
    object.get(key).is_some_and(|value| !value.is_null())
}

fn json_string_field(object: &Map<String, Value>, key: &str) -> Option<String> {
    object
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn json_proxy_credential_field<'a>(object: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    object
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

fn proxy_mode_from_url(proxy_url: Option<&str>) -> Option<String> {
    let proxy_url = proxy_url?.trim();
    if proxy_url.is_empty() {
        return None;
    }
    let scheme = url::Url::parse(proxy_url)
        .ok()
        .map(|value| value.scheme().to_ascii_lowercase())
        .unwrap_or_default();
    if scheme.starts_with("socks") {
        Some("socks".to_string())
    } else {
        Some("http".to_string())
    }
}

fn proxy_url_with_node_auth(
    proxy_url: &str,
    username: Option<&str>,
    password: Option<&str>,
) -> Option<String> {
    let username = username.filter(|value| !value.is_empty());
    let password = password.filter(|value| !value.is_empty());
    let mut parsed = url::Url::parse(proxy_url).ok()?;
    if !matches!(parsed.scheme(), "http" | "https" | "socks5" | "socks5h")
        || parsed.host_str().is_none()
    {
        return None;
    }
    if username.is_none() && password.is_none() {
        return Some(parsed.to_string());
    }
    let username = username.unwrap_or("");
    if parsed.set_username(username).is_err() {
        return None;
    }
    if parsed.set_password(password).is_err() {
        return None;
    }
    Some(parsed.to_string())
}

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };

    use aether_crypto::{encrypt_python_fernet_plaintext, DEVELOPMENT_ENCRYPTION_KEY};
    use aether_data::repository::proxy_nodes::{
        InMemoryProxyNodeRepository, ProxyNodeManualCreateMutation, ProxyNodeManualUpdateMutation,
        ProxyNodeReadRepository, ProxyNodeRegistrationMutation, ProxyNodeWriteRepository,
        StoredProxyNode,
    };
    use aether_provider_transport::snapshot::{
        GatewayProviderTransportEndpoint, GatewayProviderTransportKey,
        GatewayProviderTransportProvider, GatewayProviderTransportSnapshot,
    };
    use serde_json::json;
    use tokio::sync::Barrier;

    use super::{
        decrypt_or_migrate_proxy_tunnel_psk, open_proxy_node_secret_v2_with_encryption_key,
        proxy_node_bound_secret_purpose, proxy_node_secret_is_v2, proxy_url_with_node_auth,
        seal_proxy_node_secret_v2_with_encryption_key, PROXY_NODE_PASSWORD_FIELD,
        PROXY_NODE_PASSWORD_LEGACY_SECRET_PURPOSE, PROXY_NODE_PASSWORD_SCOPE,
        PROXY_TUNNEL_PSK_FIELD, PROXY_TUNNEL_PSK_LEGACY_SECRET_PURPOSE, PROXY_TUNNEL_PSK_SCOPE,
        PROXY_UNAVAILABLE_REASON_EXTRA_KEY,
    };
    use crate::handlers::shared::seal_runtime_secret_payload_with_encryption_key;
    use crate::{data::GatewayDataState, AppState};

    const VALID_TUNNEL_PSK: &str = "BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=";
    const ROTATED_TUNNEL_PSK: &str = "CAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAg=";

    #[test]
    fn proxy_node_bound_secret_purpose_has_unambiguous_component_lengths() {
        assert_ne!(
            proxy_node_bound_secret_purpose("a", "bc", "node"),
            proxy_node_bound_secret_purpose("ab", "c", "node"),
        );
        assert_ne!(
            proxy_node_bound_secret_purpose("scope", "field", "a\0b"),
            proxy_node_bound_secret_purpose("scope", "field\0a", "b"),
        );
    }

    #[test]
    fn proxy_url_with_node_auth_omits_empty_password_separator() {
        assert_eq!(
            proxy_url_with_node_auth("socks5://proxy.example:1080", Some("alice"), None).as_deref(),
            Some("socks5://alice@proxy.example:1080")
        );
        assert_eq!(
            proxy_url_with_node_auth("http://proxy.example:8080", None, None).as_deref(),
            Some("http://proxy.example:8080/")
        );
        assert_eq!(
            proxy_url_with_node_auth("http://proxy.example:8080", None, Some("secret")).as_deref(),
            Some("http://:secret@proxy.example:8080/")
        );
    }

    #[tokio::test]
    async fn manual_proxy_credentials_fail_closed_when_url_cannot_accept_auth() {
        let mut malformed = sample_manual_node("manual-malformed-auth-url");
        malformed.proxy_url = Some("not a proxy url".to_string());
        malformed.proxy_username = Some("alice".to_string());

        let mut unsupported = sample_manual_node("manual-unsupported-auth-url");
        unsupported.proxy_url = Some("mailto:proxy@example.com".to_string());
        unsupported.proxy_username = Some("alice".to_string());

        let mut password_only = sample_manual_node("manual-password-only");
        password_only.proxy_username = None;
        password_only.proxy_password = Some("legacy-password".to_string());

        let repository = Arc::new(InMemoryProxyNodeRepository::seed([
            malformed,
            unsupported,
            password_only,
        ]));
        let state = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(
                GatewayDataState::with_proxy_node_repository_for_tests(repository)
                    .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            );

        for node_id in ["manual-malformed-auth-url", "manual-unsupported-auth-url"] {
            assert!(
                state
                    .resolve_proxy_node_snapshot(Some(node_id))
                    .await
                    .is_none(),
                "configured credentials must not fall back to an unauthenticated URL: {node_id}"
            );
        }
        let password_only = state
            .resolve_proxy_node_snapshot(Some("manual-password-only"))
            .await
            .expect("legacy password-only proxy should remain usable");
        assert_eq!(
            password_only.url.as_deref(),
            Some("http://:legacy-password@proxy.example:8080/")
        );
    }

    #[tokio::test]
    async fn manual_proxy_password_is_encrypted_before_repository_write() {
        let repository = Arc::new(InMemoryProxyNodeRepository::default());
        let state = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(
                GatewayDataState::with_proxy_node_repository_for_tests(Arc::clone(&repository))
                    .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            );

        let created = state
            .create_manual_proxy_node(&ProxyNodeManualCreateMutation {
                node_id: None,
                name: "manual-proxy".to_string(),
                ip: "127.0.0.1".to_string(),
                port: 8080,
                region: None,
                proxy_url: "http://proxy.example:8080".to_string(),
                proxy_username: Some("alice".to_string()),
                proxy_password: Some("manual-password-marker".to_string()),
                registered_by: None,
            })
            .await
            .expect("manual proxy create should succeed")
            .expect("manual proxy should be stored");
        let stored = repository
            .find_proxy_node(&created.id)
            .await
            .expect("stored proxy should read")
            .expect("stored proxy should exist")
            .proxy_password
            .expect("stored proxy password should exist");

        assert!(proxy_node_secret_is_v2(&stored));
        assert!(!stored.contains("manual-password-marker"));
        assert_eq!(
            state
                .decrypt_proxy_node_password(&created.id)
                .await
                .expect("stored password should decrypt")
                .as_deref(),
            Some("manual-password-marker")
        );
    }

    #[tokio::test]
    async fn manual_proxy_duplicate_node_id_fails_without_overwriting_bound_password() {
        let repository = Arc::new(InMemoryProxyNodeRepository::default());
        let state = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(
                GatewayDataState::with_proxy_node_repository_for_tests(Arc::clone(&repository))
                    .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            );
        let mut first = sample_manual_create_mutation(
            "manual-duplicate-first",
            "127.0.0.41",
            8141,
            "first-password",
        );
        first.node_id = Some("fixed-manual-node-id".to_string());
        let created = state
            .create_manual_proxy_node(&first)
            .await
            .expect("first proxy should create")
            .expect("first proxy should persist");
        assert_eq!(created.id, "fixed-manual-node-id");

        let mut duplicate = sample_manual_create_mutation(
            "manual-duplicate-second",
            "127.0.0.42",
            8142,
            "replacement-password",
        );
        duplicate.node_id = Some("fixed-manual-node-id".to_string());
        assert!(state.create_manual_proxy_node(&duplicate).await.is_err());
        assert_eq!(
            state
                .decrypt_proxy_node_password("fixed-manual-node-id")
                .await
                .expect("original password should remain readable")
                .as_deref(),
            Some("first-password")
        );
        let nodes = repository
            .list_proxy_nodes()
            .await
            .expect("repository should list");
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].name, "manual-duplicate-first");
    }

    #[tokio::test]
    async fn administrator_password_rotation_wins_legacy_migration_race() {
        let mut node = sample_manual_node("manual-race");
        node.proxy_password = Some("legacy-password".to_string());
        let repository = Arc::new(InMemoryProxyNodeRepository::seed([node]));
        let state = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(
                GatewayDataState::with_proxy_node_repository_for_tests(Arc::clone(&repository))
                    .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            );
        let reached_compare = Arc::new(Barrier::new(2));
        let resume_compare = Arc::new(Barrier::new(2));
        let first_compare = Arc::new(AtomicBool::new(true));

        let migration = state.decrypt_proxy_node_password_with_before_compare("manual-race", {
            let reached_compare = Arc::clone(&reached_compare);
            let resume_compare = Arc::clone(&resume_compare);
            let first_compare = Arc::clone(&first_compare);
            move || {
                let reached_compare = Arc::clone(&reached_compare);
                let resume_compare = Arc::clone(&resume_compare);
                let first_compare = Arc::clone(&first_compare);
                async move {
                    if first_compare.swap(false, Ordering::SeqCst) {
                        reached_compare.wait().await;
                        resume_compare.wait().await;
                    }
                }
            }
        });
        let rotation = async {
            reached_compare.wait().await;
            state
                .update_manual_proxy_node(&ProxyNodeManualUpdateMutation {
                    node_id: "manual-race".to_string(),
                    name: None,
                    ip: None,
                    port: None,
                    region: None,
                    proxy_url: None,
                    proxy_username: None,
                    proxy_password: Some("rotated-password".to_string()),
                })
                .await
                .expect("administrator rotation should persist")
                .expect("manual proxy should exist");
            resume_compare.wait().await;
        };

        let (migrated, ()) = tokio::join!(migration, rotation);
        assert_eq!(
            migrated
                .expect("migration should retry the rotated value")
                .as_deref(),
            Some("rotated-password")
        );
        let stored = repository
            .find_proxy_node("manual-race")
            .await
            .expect("stored proxy should read")
            .expect("stored proxy should exist")
            .proxy_password
            .expect("stored password should exist");
        assert!(proxy_node_secret_is_v2(&stored));
        assert!(!stored.contains("legacy-password"));
    }

    #[tokio::test]
    async fn damaged_proxy_password_ciphertext_fails_closed() {
        let mut node = sample_manual_node("manual-damaged");
        node.proxy_password = Some("aether-runtime-secret-v1:not-a-fernet-token".to_string());
        let repository = Arc::new(InMemoryProxyNodeRepository::seed([node]));
        let state = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(
                GatewayDataState::with_proxy_node_repository_for_tests(repository)
                    .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            );

        assert!(state
            .decrypt_proxy_node_password("manual-damaged")
            .await
            .is_err());
        assert!(state
            .resolve_proxy_node_snapshot(Some("manual-damaged"))
            .await
            .is_none());
    }

    #[tokio::test]
    async fn proxy_node_password_v2_rejects_cross_node_and_cross_field_copy() {
        let repository = Arc::new(InMemoryProxyNodeRepository::default());
        let state = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(
                GatewayDataState::with_proxy_node_repository_for_tests(Arc::clone(&repository))
                    .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            );
        let first = state
            .create_manual_proxy_node(&sample_manual_create_mutation(
                "manual-bound-first",
                "127.0.0.11",
                8111,
                "first-password",
            ))
            .await
            .expect("first proxy should create")
            .expect("first proxy should exist");
        let second = state
            .create_manual_proxy_node(&sample_manual_create_mutation(
                "manual-bound-second",
                "127.0.0.12",
                8112,
                "second-password",
            ))
            .await
            .expect("second proxy should create")
            .expect("second proxy should exist");
        let first_ciphertext = repository
            .find_proxy_node(&first.id)
            .await
            .expect("first proxy should read")
            .and_then(|node| node.proxy_password)
            .expect("first ciphertext should exist");
        let second_ciphertext = repository
            .find_proxy_node(&second.id)
            .await
            .expect("second proxy should read")
            .and_then(|node| node.proxy_password)
            .expect("second ciphertext should exist");

        assert!(proxy_node_secret_is_v2(&first_ciphertext));
        assert!(open_proxy_node_secret_v2_with_encryption_key(
            Some(DEVELOPMENT_ENCRYPTION_KEY),
            PROXY_TUNNEL_PSK_SCOPE,
            PROXY_TUNNEL_PSK_FIELD,
            &first.id,
            &first_ciphertext,
        )
        .is_none());
        assert!(repository
            .compare_and_set_proxy_password(&second.id, &second_ciphertext, &first_ciphertext)
            .await
            .expect("ciphertext copy should persist for the adversarial fixture"));
        assert_eq!(
            state
                .decrypt_proxy_node_password(&first.id)
                .await
                .expect("original ciphertext should open")
                .as_deref(),
            Some("first-password")
        );
        assert!(state.decrypt_proxy_node_password(&second.id).await.is_err());
    }

    #[tokio::test]
    async fn proxy_node_password_legacy_runtime_envelope_migrates_to_bound_v2() {
        let legacy = seal_runtime_secret_payload_with_encryption_key(
            Some(DEVELOPMENT_ENCRYPTION_KEY),
            PROXY_NODE_PASSWORD_LEGACY_SECRET_PURPOSE,
            "legacy-runtime-password",
        )
        .expect("legacy password should seal");
        let mut node = sample_manual_node("manual-runtime-legacy");
        node.proxy_password = Some(legacy);
        let repository = Arc::new(InMemoryProxyNodeRepository::seed([node]));
        let state = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(
                GatewayDataState::with_proxy_node_repository_for_tests(Arc::clone(&repository))
                    .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            );

        assert_eq!(
            state
                .decrypt_proxy_node_password("manual-runtime-legacy")
                .await
                .expect("legacy password should migrate")
                .as_deref(),
            Some("legacy-runtime-password")
        );
        let migrated = repository
            .find_proxy_node("manual-runtime-legacy")
            .await
            .expect("migrated proxy should read")
            .and_then(|node| node.proxy_password)
            .expect("migrated password should exist");
        assert!(proxy_node_secret_is_v2(&migrated));
    }

    #[tokio::test]
    async fn tunnel_psk_registration_and_legacy_migration_store_only_ciphertext() {
        let repository = Arc::new(InMemoryProxyNodeRepository::default());
        let data = GatewayDataState::with_proxy_node_repository_for_tests(Arc::clone(&repository))
            .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY);
        let state = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(data.clone());
        let registered = state
            .register_proxy_node(&ProxyNodeRegistrationMutation {
                node_id: None,
                name: "tunnel-new".to_string(),
                ip: "127.0.0.1".to_string(),
                port: 0,
                region: None,
                heartbeat_interval: 30,
                active_connections: None,
                total_requests: None,
                avg_latency_ms: None,
                hardware_info: None,
                estimated_max_concurrency: None,
                proxy_metadata: Some(json!({
                    "tunnel_security": {
                        "mode": "non_tls_required",
                        "encryption_key": VALID_TUNNEL_PSK,
                    }
                })),
                proxy_version: None,
                registered_by: None,
                tunnel_mode: true,
            })
            .await
            .expect("tunnel registration should succeed")
            .expect("tunnel should be stored");
        let stored = repository
            .find_proxy_node(&registered.id)
            .await
            .expect("stored tunnel should read")
            .expect("stored tunnel should exist")
            .proxy_metadata
            .expect("stored tunnel metadata should exist");
        assert!(stored.pointer("/tunnel_security/encryption_key").is_none());
        let ciphertext = stored
            .pointer("/tunnel_security/encryption_key_encrypted")
            .and_then(serde_json::Value::as_str)
            .expect("encrypted tunnel key should exist");
        assert!(!ciphertext.contains(VALID_TUNNEL_PSK));
        assert_eq!(
            open_proxy_node_secret_v2_with_encryption_key(
                Some(DEVELOPMENT_ENCRYPTION_KEY),
                PROXY_TUNNEL_PSK_SCOPE,
                PROXY_TUNNEL_PSK_FIELD,
                &registered.id,
                ciphertext,
            )
            .as_deref(),
            Some(VALID_TUNNEL_PSK)
        );

        let mut legacy = sample_tunnel_node("tunnel-legacy");
        legacy.proxy_metadata = Some(json!({
            "version": "1.0.0",
            "tunnel_security": {
                "mode": "non_tls_required",
                "encryption_key": VALID_TUNNEL_PSK,
            }
        }));
        let legacy_repository = Arc::new(InMemoryProxyNodeRepository::seed([legacy]));
        let legacy_data =
            GatewayDataState::with_proxy_node_repository_for_tests(Arc::clone(&legacy_repository))
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY);
        assert_eq!(
            decrypt_or_migrate_proxy_tunnel_psk(&legacy_data, "tunnel-legacy")
                .await
                .expect("legacy tunnel key should migrate")
                .as_deref(),
            Some(VALID_TUNNEL_PSK)
        );
        let migrated = legacy_repository
            .find_proxy_node("tunnel-legacy")
            .await
            .expect("migrated tunnel should read")
            .expect("migrated tunnel should exist")
            .proxy_metadata
            .expect("migrated metadata should exist");
        assert!(migrated
            .pointer("/tunnel_security/encryption_key")
            .is_none());
        assert!(migrated
            .pointer("/tunnel_security/encryption_key_encrypted")
            .and_then(serde_json::Value::as_str)
            .is_some_and(proxy_node_secret_is_v2));
    }

    #[tokio::test]
    async fn damaged_tunnel_psk_ciphertext_fails_closed() {
        let mut node = sample_tunnel_node("tunnel-damaged");
        node.proxy_metadata = Some(json!({
            "tunnel_security": {
                "mode": "non_tls_required",
                "encryption_key_encrypted": "aether-runtime-secret-v1:not-a-fernet-token",
            }
        }));
        let repository = Arc::new(InMemoryProxyNodeRepository::seed([node]));
        let data = GatewayDataState::with_proxy_node_repository_for_tests(repository)
            .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY);

        assert!(decrypt_or_migrate_proxy_tunnel_psk(&data, "tunnel-damaged")
            .await
            .is_err());
    }

    #[tokio::test]
    async fn proxy_tunnel_psk_v2_rejects_cross_node_and_cross_field_copy() {
        let first_ciphertext = seal_proxy_node_secret_v2_with_encryption_key(
            Some(DEVELOPMENT_ENCRYPTION_KEY),
            PROXY_TUNNEL_PSK_SCOPE,
            PROXY_TUNNEL_PSK_FIELD,
            "tunnel-bound-first",
            VALID_TUNNEL_PSK,
        )
        .expect("first tunnel key should seal");
        let mut first = sample_tunnel_node("tunnel-bound-first");
        first.proxy_metadata = Some(json!({
            "tunnel_security": {
                "mode": "non_tls_required",
                "encryption_key_encrypted": first_ciphertext,
            }
        }));
        let mut second = sample_tunnel_node("tunnel-bound-second");
        second.proxy_metadata = first.proxy_metadata.clone();
        let repository = Arc::new(InMemoryProxyNodeRepository::seed([first, second]));
        let data = GatewayDataState::with_proxy_node_repository_for_tests(repository)
            .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY);

        assert_eq!(
            decrypt_or_migrate_proxy_tunnel_psk(&data, "tunnel-bound-first")
                .await
                .expect("original tunnel key should open")
                .as_deref(),
            Some(VALID_TUNNEL_PSK)
        );
        assert!(
            decrypt_or_migrate_proxy_tunnel_psk(&data, "tunnel-bound-second")
                .await
                .is_err()
        );
        let copied = data
            .find_proxy_node("tunnel-bound-first")
            .await
            .expect("first tunnel should read")
            .and_then(|node| node.proxy_metadata)
            .and_then(|metadata| {
                metadata
                    .pointer("/tunnel_security/encryption_key_encrypted")
                    .and_then(serde_json::Value::as_str)
                    .map(ToOwned::to_owned)
            })
            .expect("tunnel ciphertext should exist");
        assert!(open_proxy_node_secret_v2_with_encryption_key(
            Some(DEVELOPMENT_ENCRYPTION_KEY),
            PROXY_NODE_PASSWORD_SCOPE,
            PROXY_NODE_PASSWORD_FIELD,
            "tunnel-bound-first",
            &copied,
        )
        .is_none());
    }

    #[tokio::test]
    async fn wrong_binding_v2_tunnel_psk_never_falls_back_to_legacy_plaintext() {
        let wrong_binding = seal_proxy_node_secret_v2_with_encryption_key(
            Some(DEVELOPMENT_ENCRYPTION_KEY),
            PROXY_TUNNEL_PSK_SCOPE,
            PROXY_TUNNEL_PSK_FIELD,
            "different-node",
            VALID_TUNNEL_PSK,
        )
        .expect("wrong-binding fixture should seal");
        let mut node = sample_tunnel_node("tunnel-no-v2-fallback");
        node.proxy_metadata = Some(json!({
            "tunnel_security": {
                "mode": "non_tls_required",
                "encryption_key_encrypted": wrong_binding,
                "encryption_key": VALID_TUNNEL_PSK,
            }
        }));
        let repository = Arc::new(InMemoryProxyNodeRepository::seed([node]));
        let data = GatewayDataState::with_proxy_node_repository_for_tests(repository)
            .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY);

        assert!(
            decrypt_or_migrate_proxy_tunnel_psk(&data, "tunnel-no-v2-fallback")
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn proxy_tunnel_psk_legacy_runtime_envelope_migrates_to_bound_v2() {
        let legacy = seal_runtime_secret_payload_with_encryption_key(
            Some(DEVELOPMENT_ENCRYPTION_KEY),
            PROXY_TUNNEL_PSK_LEGACY_SECRET_PURPOSE,
            VALID_TUNNEL_PSK,
        )
        .expect("legacy tunnel key should seal");
        let mut node = sample_tunnel_node("tunnel-runtime-legacy");
        node.proxy_metadata = Some(json!({
            "tunnel_security": {
                "mode": "non_tls_required",
                "encryption_key_encrypted": legacy,
            }
        }));
        let repository = Arc::new(InMemoryProxyNodeRepository::seed([node]));
        let data = GatewayDataState::with_proxy_node_repository_for_tests(Arc::clone(&repository))
            .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY);

        assert_eq!(
            decrypt_or_migrate_proxy_tunnel_psk(&data, "tunnel-runtime-legacy")
                .await
                .expect("legacy tunnel key should migrate")
                .as_deref(),
            Some(VALID_TUNNEL_PSK)
        );
        let migrated = repository
            .find_proxy_node("tunnel-runtime-legacy")
            .await
            .expect("migrated tunnel should read")
            .and_then(|node| node.proxy_metadata)
            .and_then(|metadata| {
                metadata
                    .pointer("/tunnel_security/encryption_key_encrypted")
                    .and_then(serde_json::Value::as_str)
                    .map(ToOwned::to_owned)
            })
            .expect("migrated tunnel ciphertext should exist");
        assert!(proxy_node_secret_is_v2(&migrated));
    }

    #[tokio::test]
    async fn incoming_proxy_node_ciphertext_is_rejected_before_repository_write() {
        let repository = Arc::new(InMemoryProxyNodeRepository::default());
        let state = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(
                GatewayDataState::with_proxy_node_repository_for_tests(Arc::clone(&repository))
                    .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            );
        let password_ciphertext = seal_proxy_node_secret_v2_with_encryption_key(
            Some(DEVELOPMENT_ENCRYPTION_KEY),
            PROXY_NODE_PASSWORD_SCOPE,
            PROXY_NODE_PASSWORD_FIELD,
            "attacker-selected-id",
            "copied-password",
        )
        .expect("password fixture should seal");
        assert!(state
            .create_manual_proxy_node(&sample_manual_create_mutation(
                "manual-ciphertext-input",
                "127.0.0.21",
                8121,
                &password_ciphertext,
            ))
            .await
            .is_err());
        let legacy_runtime_ciphertext = seal_runtime_secret_payload_with_encryption_key(
            Some(DEVELOPMENT_ENCRYPTION_KEY),
            PROXY_NODE_PASSWORD_LEGACY_SECRET_PURPOSE,
            "legacy-copied-password",
        )
        .expect("legacy password fixture should seal");
        let fernet_ciphertext =
            encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, "fernet-copied-password")
                .expect("fernet password fixture should seal");
        for (name, ip, port, ciphertext) in [
            (
                "manual-v1-ciphertext-input",
                "127.0.0.23",
                8123,
                legacy_runtime_ciphertext.as_str(),
            ),
            (
                "manual-fernet-ciphertext-input",
                "127.0.0.24",
                8124,
                fernet_ciphertext.as_str(),
            ),
        ] {
            assert!(state
                .create_manual_proxy_node(&sample_manual_create_mutation(
                    name, ip, port, ciphertext,
                ))
                .await
                .is_err());
        }

        let tunnel_ciphertext = seal_proxy_node_secret_v2_with_encryption_key(
            Some(DEVELOPMENT_ENCRYPTION_KEY),
            PROXY_TUNNEL_PSK_SCOPE,
            PROXY_TUNNEL_PSK_FIELD,
            "attacker-selected-id",
            VALID_TUNNEL_PSK,
        )
        .expect("tunnel fixture should seal");
        assert!(state
            .register_proxy_node(&sample_tunnel_registration_mutation(
                "tunnel-ciphertext-input",
                "127.0.0.22",
                8122,
                &tunnel_ciphertext,
            ))
            .await
            .is_err());
        assert!(repository
            .list_proxy_nodes()
            .await
            .expect("repository should list")
            .is_empty());
    }

    #[tokio::test]
    async fn tunnel_reregistration_keeps_node_identity_and_rebinds_rotated_psk() {
        let repository = Arc::new(InMemoryProxyNodeRepository::default());
        let data = GatewayDataState::with_proxy_node_repository_for_tests(Arc::clone(&repository))
            .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY);
        let state = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(data.clone());
        let first = state
            .register_proxy_node(&sample_tunnel_registration_mutation(
                "tunnel-stable-first",
                "127.0.0.31",
                8131,
                VALID_TUNNEL_PSK,
            ))
            .await
            .expect("first registration should succeed")
            .expect("first registration should persist");
        let rotated = state
            .register_proxy_node(&sample_tunnel_registration_mutation(
                "tunnel-stable-rotated",
                "127.0.0.31",
                8131,
                ROTATED_TUNNEL_PSK,
            ))
            .await
            .expect("rotated registration should succeed")
            .expect("rotated registration should persist");

        assert_eq!(rotated.id, first.id);
        assert_eq!(
            decrypt_or_migrate_proxy_tunnel_psk(&data, &first.id)
                .await
                .expect("rotated key should open")
                .as_deref(),
            Some(ROTATED_TUNNEL_PSK)
        );
    }

    #[tokio::test]
    async fn resolve_proxy_node_snapshot_rejects_unroutable_tunnel_node() {
        let repository = Arc::new(InMemoryProxyNodeRepository::seed(vec![sample_tunnel_node(
            "proxy-node-stale",
        )]));
        let state = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(GatewayDataState::with_proxy_node_repository_for_tests(
                repository,
            ));

        let snapshot = state
            .resolve_proxy_node_snapshot(Some("proxy-node-stale"))
            .await;

        assert_eq!(snapshot, None);
    }

    #[tokio::test]
    async fn resolve_proxy_node_snapshot_keeps_tunnel_node_with_owner_hint() {
        let node = sample_tunnel_node("proxy-node-owned");
        let tunnel_generation = node.tunnel_generation.clone();
        let repository = Arc::new(InMemoryProxyNodeRepository::seed(vec![node]));
        let state = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(
                GatewayDataState::with_proxy_node_repository_for_tests(repository)
                    .with_system_config_values_for_tests(vec![(
                        "tunnel.attachments.proxy-node-owned".to_string(),
                        json!({
                            "gateway_instance_id": "gateway-owner",
                            "relay_base_url": "http://gateway-owner.internal",
                            "tunnel_generation": tunnel_generation,
                            "conn_count": 1,
                            "observed_at_unix_secs": 4_102_444_800u64,
                        }),
                    )]),
            );

        let snapshot = state
            .resolve_proxy_node_snapshot(Some("proxy-node-owned"))
            .await
            .expect("owned tunnel snapshot should resolve");

        assert_eq!(snapshot.mode.as_deref(), Some("tunnel"));
        assert_eq!(snapshot.node_id.as_deref(), Some("proxy-node-owned"));
        assert_eq!(
            snapshot
                .extra
                .as_ref()
                .and_then(|extra| extra.get("tunnel_base_url"))
                .and_then(serde_json::Value::as_str),
            Some("http://gateway-owner.internal")
        );
    }

    #[tokio::test]
    async fn resolve_configured_proxy_snapshot_blocks_unroutable_stored_tunnel_reference() {
        let repository = Arc::new(InMemoryProxyNodeRepository::seed(vec![sample_tunnel_node(
            "proxy-node-stale",
        )]));
        let state = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(GatewayDataState::with_proxy_node_repository_for_tests(
                repository,
            ));

        let snapshot = state
            .resolve_configured_proxy_snapshot_with_tunnel_affinity(Some(&json!({
                "node_id": "proxy-node-stale",
                "enabled": true,
            })))
            .await
            .expect("explicit unavailable proxy must remain represented");

        assert_eq!(snapshot.enabled, Some(true));
        assert_eq!(snapshot.mode.as_deref(), Some("unavailable"));
        assert!(snapshot.url.is_none());
        assert_eq!(
            snapshot
                .extra
                .as_ref()
                .and_then(|extra| extra.get(PROXY_UNAVAILABLE_REASON_EXTRA_KEY))
                .and_then(serde_json::Value::as_str),
            Some("configured_proxy_node_unavailable")
        );
    }

    #[tokio::test]
    async fn explicit_stored_node_does_not_fall_back_to_inline_url_when_unroutable() {
        let repository = Arc::new(InMemoryProxyNodeRepository::seed(vec![sample_tunnel_node(
            "proxy-node-stale",
        )]));
        let state = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(GatewayDataState::with_proxy_node_repository_for_tests(
                repository,
            ));

        let snapshot = state
            .resolve_configured_proxy_snapshot_with_tunnel_affinity(Some(&json!({
                "node_id": "proxy-node-stale",
                "url": "http://proxy.example:8080",
                "enabled": true,
            })))
            .await
            .expect("explicit unavailable proxy must remain represented");

        assert_eq!(snapshot.mode.as_deref(), Some("unavailable"));
        assert!(snapshot.node_id.is_none());
        assert!(snapshot.url.is_none());
    }

    #[tokio::test]
    async fn malformed_explicit_inline_proxy_remains_fail_closed() {
        let state = AppState::new().expect("state should build");
        let snapshot = state
            .resolve_configured_proxy_snapshot_with_tunnel_affinity(Some(&json!({
                "enabled": true,
                "url": "   ",
            })))
            .await
            .expect("malformed explicit proxy must remain represented");

        assert_eq!(snapshot.mode.as_deref(), Some("unavailable"));
        assert!(snapshot.url.is_none());
    }

    #[tokio::test]
    async fn inline_proxy_credentials_are_injected_without_copying_them_to_extra() {
        let state = AppState::new().expect("state should build");
        let snapshot = state
            .resolve_configured_proxy_snapshot_with_tunnel_affinity(Some(&json!({
                "enabled": true,
                "url": "http://proxy.example:8080",
                "username": " alice ",
                "password": " p:ss ",
                "region": "test",
            })))
            .await
            .expect("authenticated inline proxy should resolve");

        assert_eq!(
            snapshot.url.as_deref(),
            Some("http://%20alice%20:%20p%3Ass%20@proxy.example:8080/")
        );
        assert_eq!(snapshot.extra, Some(json!({"region": "test"})));
    }

    #[tokio::test]
    async fn unavailable_key_proxy_blocks_endpoint_and_provider_fallbacks() {
        let repository = Arc::new(InMemoryProxyNodeRepository::seed([sample_tunnel_node(
            "proxy-node-stale",
        )]));
        let state = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(GatewayDataState::with_proxy_node_repository_for_tests(
                repository,
            ));
        let transport = sample_transport(
            Some(json!({"enabled": true, "node_id": "proxy-node-stale"})),
            Some(json!({"enabled": true, "url": "http://endpoint-proxy:8080"})),
            Some(json!({"enabled": true, "url": "http://provider-proxy:8080"})),
        );

        let snapshot = state
            .resolve_transport_proxy_snapshot_with_tunnel_affinity(&transport)
            .await
            .expect("unavailable key proxy must block fallback");
        assert_eq!(snapshot.mode.as_deref(), Some("unavailable"));
        assert!(snapshot.url.is_none());
        assert_eq!(
            state
                .resolve_transport_proxy_source_with_tunnel_affinity(&transport)
                .await,
            Some("key")
        );

        let transport = sample_transport(
            Some(json!({"enabled": false, "node_id": "proxy-node-stale"})),
            Some(json!({"enabled": true, "url": "http://endpoint-proxy:8080"})),
            None,
        );
        let snapshot = state
            .resolve_transport_proxy_snapshot_with_tunnel_affinity(&transport)
            .await
            .expect("disabled key proxy should allow endpoint resolution");
        assert_eq!(snapshot.url.as_deref(), Some("http://endpoint-proxy:8080/"));
    }

    #[tokio::test]
    async fn unavailable_system_proxy_node_does_not_become_direct_transport() {
        let state = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(
                GatewayDataState::disabled().with_system_config_values_for_tests([(
                    "system_proxy_node_id".to_string(),
                    json!("missing-system-proxy"),
                )]),
            );

        let snapshot = state
            .resolve_system_proxy_snapshot()
            .await
            .expect("configured unavailable system proxy must remain represented");
        assert_eq!(snapshot.mode.as_deref(), Some("unavailable"));
        assert!(snapshot.url.is_none());
    }

    fn sample_manual_create_mutation(
        name: &str,
        ip: &str,
        port: i32,
        password: &str,
    ) -> ProxyNodeManualCreateMutation {
        ProxyNodeManualCreateMutation {
            node_id: None,
            name: name.to_string(),
            ip: ip.to_string(),
            port,
            region: None,
            proxy_url: "http://proxy.example:8080".to_string(),
            proxy_username: Some("alice".to_string()),
            proxy_password: Some(password.to_string()),
            registered_by: None,
        }
    }

    fn sample_tunnel_registration_mutation(
        name: &str,
        ip: &str,
        port: i32,
        psk: &str,
    ) -> ProxyNodeRegistrationMutation {
        ProxyNodeRegistrationMutation {
            node_id: None,
            name: name.to_string(),
            ip: ip.to_string(),
            port,
            region: None,
            heartbeat_interval: 30,
            active_connections: None,
            total_requests: None,
            avg_latency_ms: None,
            hardware_info: None,
            estimated_max_concurrency: None,
            proxy_metadata: Some(json!({
                "tunnel_security": {
                    "mode": "non_tls_required",
                    "encryption_key": psk,
                }
            })),
            proxy_version: None,
            registered_by: None,
            tunnel_mode: true,
        }
    }

    fn sample_tunnel_node(id: &str) -> StoredProxyNode {
        StoredProxyNode::new(
            id.to_string(),
            id.to_string(),
            "127.0.0.1".to_string(),
            0,
            false,
            "online".to_string(),
            15,
            1,
            0,
            0,
            0,
            0,
            true,
            true,
            1,
        )
        .expect("sample tunnel node should build")
    }

    fn sample_manual_node(id: &str) -> StoredProxyNode {
        StoredProxyNode::new(
            id.to_string(),
            id.to_string(),
            "127.0.0.1".to_string(),
            8080,
            true,
            "online".to_string(),
            0,
            0,
            0,
            0,
            0,
            0,
            false,
            false,
            0,
        )
        .expect("sample manual node should build")
        .with_manual_proxy_fields(
            Some("http://proxy.example:8080".to_string()),
            Some("alice".to_string()),
            None,
        )
    }

    fn sample_transport(
        key_proxy: Option<serde_json::Value>,
        endpoint_proxy: Option<serde_json::Value>,
        provider_proxy: Option<serde_json::Value>,
    ) -> GatewayProviderTransportSnapshot {
        GatewayProviderTransportSnapshot {
            provider: GatewayProviderTransportProvider {
                id: "provider-1".to_string(),
                name: "provider".to_string(),
                provider_type: "openai".to_string(),
                website: None,
                is_active: true,
                keep_priority_on_conversion: false,
                enable_format_conversion: false,
                concurrent_limit: None,
                max_retries: None,
                proxy: provider_proxy,
                request_timeout_secs: None,
                stream_first_byte_timeout_secs: None,
                config: None,
            },
            endpoint: GatewayProviderTransportEndpoint {
                id: "endpoint-1".to_string(),
                provider_id: "provider-1".to_string(),
                api_format: "openai:chat_completions".to_string(),
                api_family: None,
                endpoint_kind: None,
                is_active: true,
                base_url: "https://api.example.test".to_string(),
                header_rules: None,
                body_rules: None,
                max_retries: None,
                custom_path: None,
                config: None,
                format_acceptance_config: None,
                proxy: endpoint_proxy,
            },
            key: GatewayProviderTransportKey {
                id: "key-1".to_string(),
                provider_id: "provider-1".to_string(),
                name: "key".to_string(),
                auth_type: "api_key".to_string(),
                is_active: true,
                api_formats: None,
                auth_type_by_format: None,
                allow_auth_channel_mismatch_formats: None,
                allowed_models: None,
                capabilities: None,
                rate_multipliers: None,
                global_priority_by_format: None,
                expires_at_unix_secs: None,
                proxy: key_proxy,
                fingerprint: None,
                upstream_metadata: None,
                decrypted_api_key: "test-key".to_string(),
                decrypted_auth_config: None,
            },
        }
    }
}
