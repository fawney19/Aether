use std::sync::{Arc, Mutex};

use aether_crypto::{encrypt_python_fernet_plaintext, DEVELOPMENT_ENCRYPTION_KEY};
use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
use aether_data::repository::proxy_nodes::{InMemoryProxyNodeRepository, StoredProxyNode};
use aether_data::repository::video_tasks::InMemoryVideoTaskRepository;
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
};
use aether_data_contracts::repository::video_tasks::{
    UpsertVideoTask, VideoTaskLookupKey, VideoTaskReadRepository, VideoTaskWriteRepository,
};
use axum::body::{to_bytes, Body, Bytes};
use axum::response::Response;
use axum::routing::any;
use axum::{extract::Request, Json, Router};
use http::header::{HeaderName, HeaderValue};
use http::StatusCode;
use serde_json::json;

use crate::constants::{
    CONTROL_EXECUTED_HEADER, CONTROL_EXECUTE_FALLBACK_HEADER, EXECUTION_PATH_HEADER,
    TRACE_ID_HEADER,
};

use super::{
    build_router, build_router_with_state, build_state_with_execution_runtime_override,
    start_server, AppState, VideoTaskTruthSourceMode,
};

mod data_read;
mod gemini_sync_create;
mod gemini_sync_task;
mod openai_sync_create;
mod openai_sync_task;
mod registry_poller;
mod routing;
mod stream;

/// Seed online manual proxy nodes for video execution fixtures.
///
/// Production resolution intentionally fails closed when a provider refers to
/// an unregistered node.  Tests that exercise a configured node therefore need
/// the same deployment-state record; the loopback URL is never contacted when
/// the execution-runtime override is active.
pub(super) fn video_proxy_node_repository<I, S>(node_ids: I) -> Arc<InMemoryProxyNodeRepository>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let nodes = node_ids.into_iter().map(|node_id| {
        let node_id = node_id.as_ref();
        StoredProxyNode::new(
            node_id.to_string(),
            format!("video-test-{node_id}"),
            "127.0.0.1".to_string(),
            1,
            true,
            "online".to_string(),
            30,
            0,
            0,
            0,
            0,
            0,
            false,
            false,
            1,
        )
        .expect("video test proxy node should build")
        .with_manual_proxy_fields(Some("http://127.0.0.1:1".to_string()), None, None)
        .with_tunnel_generation(format!("video-test-generation-{node_id}"))
    });
    Arc::new(InMemoryProxyNodeRepository::seed(nodes))
}

/// Build a provider catalog row set for tasks whose sensitive snapshot fields
/// have been removed by the persistence boundary.  The credential is sealed
/// with the record-bound v2 envelope used by production, so a read-only test
/// state can reconstruct transport without relying on a migration writer.
pub(super) fn video_provider_catalog_repository(
    provider_id: &str,
    provider_type: &str,
    endpoint_id: &str,
    api_format: &str,
    endpoint_base_url: &str,
    key_id: &str,
    upstream_api_key: &str,
) -> Arc<InMemoryProviderCatalogReadRepository> {
    fn seal_bound_credential(
        provider_id: &str,
        key_id: &str,
        field: &str,
        plaintext: &str,
    ) -> String {
        let purpose = format!(
            "provider-catalog-credential-bound-v2\0provider-id-bytes={}\0{provider_id}\0key-id-bytes={}\0{key_id}\0field={field}",
            provider_id.len(),
            key_id.len(),
        );
        let protected = format!("{purpose}\0{plaintext}");
        let ciphertext = encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, &protected)
            .expect("provider test credential should encrypt");
        format!("aether-provider-catalog-credential-v2:aether-runtime-secret-v1:{ciphertext}")
    }

    let provider = StoredProviderCatalogProvider::new(
        provider_id.to_string(),
        format!("video-{provider_type}"),
        Some("https://example.com".to_string()),
        provider_type.to_string(),
    )
    .expect("provider should build")
    .with_transport_fields(
        true,
        false,
        false,
        None,
        Some(2),
        None,
        Some(20.0),
        None,
        None,
    );
    let endpoint = StoredProviderCatalogEndpoint::new(
        endpoint_id.to_string(),
        provider_id.to_string(),
        api_format.to_string(),
        Some(provider_type.to_string()),
        Some("video".to_string()),
        true,
    )
    .expect("endpoint should build")
    .with_transport_fields(
        endpoint_base_url.to_string(),
        None,
        None,
        Some(2),
        None,
        None,
        None,
        None,
    )
    .expect("endpoint transport should build");
    let key = StoredProviderCatalogKey::new(
        key_id.to_string(),
        provider_id.to_string(),
        "prod".to_string(),
        "api_key".to_string(),
        None,
        true,
    )
    .expect("key should build")
    .with_transport_fields(
        Some(serde_json::json!([api_format])),
        seal_bound_credential(provider_id, key_id, "api-key", upstream_api_key),
        None,
        None,
        Some(serde_json::json!({api_format: 1})),
        None,
        None,
        None,
        None,
    )
    .expect("key transport should build");

    Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![endpoint],
        vec![key],
    ))
}
