use super::attach_admin_audit_response;
use crate::LocalProviderDeleteTaskState;
use aether_admin::provider::pool as admin_provider_pool_pure;
pub(crate) use aether_admin::provider::pool::{
    admin_pool_batch_delete_task_parts, AdminPoolBatchActionRequest, AdminPoolBatchImportItem,
    AdminPoolBatchImportRequest,
};
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey,
};
use axum::{body::Body, response::Response};

pub(crate) fn admin_pool_key_proxy_value(proxy_node_id: Option<&str>) -> Option<serde_json::Value> {
    admin_provider_pool_pure::admin_pool_key_proxy_value(proxy_node_id)
}

pub(crate) fn build_admin_pool_batch_delete_task_payload(
    task: &LocalProviderDeleteTaskState,
) -> serde_json::Value {
    admin_provider_pool_pure::build_admin_pool_batch_delete_task_payload(
        &task.task_id,
        &task.provider_id,
        &task.status,
        &task.stage,
        task.total_keys,
        task.deleted_keys,
        task.total_endpoints,
        task.deleted_endpoints,
        &task.message,
    )
}

pub(crate) fn attach_admin_pool_batch_delete_task_terminal_audit(
    provider_id: &str,
    task_id: &str,
    task_status: &str,
    response: Response<Body>,
) -> Response<Body> {
    match task_status {
        "completed" => attach_admin_audit_response(
            response,
            "admin_pool_batch_delete_task_completed_viewed",
            "view_pool_batch_delete_task_terminal_state",
            "provider_key_batch_delete_task",
            &format!("{provider_id}:{task_id}"),
        ),
        "failed" => attach_admin_audit_response(
            response,
            "admin_pool_batch_delete_task_failed_viewed",
            "view_pool_batch_delete_task_terminal_state",
            "provider_key_batch_delete_task",
            &format!("{provider_id}:{task_id}"),
        ),
        _ => response,
    }
}

pub(crate) fn admin_pool_resolved_api_formats(
    endpoints: &[StoredProviderCatalogEndpoint],
    existing_keys: &[StoredProviderCatalogKey],
) -> Vec<String> {
    admin_provider_pool_pure::admin_pool_resolved_api_formats(endpoints, existing_keys)
}
