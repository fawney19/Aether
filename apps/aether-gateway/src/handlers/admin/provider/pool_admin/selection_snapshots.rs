use super::build_admin_pool_error_response;
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::GatewayError;
use aether_runtime_state::{RuntimeLockLease, RuntimeState};
use axum::{body::Body, http, response::Response};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::time::sleep;
use uuid::Uuid;

pub(crate) const ADMIN_POOL_SELECTION_SNAPSHOT_ITEM_PAGE_SIZE: usize = 1_000;
pub(crate) const ADMIN_POOL_SELECTION_SNAPSHOT_MAX_TOTAL: usize = 20_000;
pub(crate) const ADMIN_POOL_SELECTION_SNAPSHOT_MAX_ACTIVE_PER_ADMIN_PROVIDER: usize = 3;

const ADMIN_POOL_SELECTION_SNAPSHOT_STATUS_READY: &str = "ready";
const ADMIN_POOL_SELECTION_SNAPSHOT_INDEX_LOCK_TTL: Duration = Duration::from_secs(5);
const ADMIN_POOL_SELECTION_SNAPSHOT_INDEX_LOCK_RETRY_DELAY: Duration = Duration::from_millis(10);
const ADMIN_POOL_SELECTION_SNAPSHOT_INDEX_LOCK_RETRIES: usize = 100;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AdminPoolSelectionSnapshotItem {
    pub(crate) ordinal: u64,
    pub(crate) key_id: String,
    pub(crate) key_updated_at_unix_secs: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct AdminPoolSelectionSnapshot {
    pub(crate) id: String,
    pub(crate) provider_id: String,
    pub(crate) created_by: Option<String>,
    pub(crate) filter_json: Option<Value>,
    pub(crate) total: usize,
    pub(crate) status: String,
    pub(crate) created_at_unix_secs: u64,
    pub(crate) expires_at_unix_secs: u64,
    pub(crate) items: Vec<AdminPoolSelectionSnapshotItem>,
}

pub(crate) struct AdminPoolSelectionSnapshotReference {
    pub(crate) snapshot_id: String,
    pub(crate) expected_total: usize,
}

fn current_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn current_admin_user_id(request_context: &AdminRequestContext<'_>) -> Option<String> {
    request_context
        .decision()
        .and_then(|decision| decision.admin_principal.as_ref())
        .map(|principal| principal.user_id.trim())
        .filter(|user_id| !user_id.is_empty())
        .map(ToOwned::to_owned)
}

pub(crate) fn admin_pool_selection_snapshot_key(snapshot_id: &str) -> String {
    format!("admin:pool:selection_snapshot:{}", snapshot_id.trim())
}

fn admin_pool_selection_snapshot_index_key(created_by: Option<&str>, provider_id: &str) -> String {
    let owner = created_by
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("anonymous");
    format!("admin:pool:selection_snapshot_index:{provider_id}:{owner}")
}

fn admin_pool_selection_snapshot_index_lock_key(
    created_by: Option<&str>,
    provider_id: &str,
) -> String {
    format!(
        "admin:pool:selection_snapshot_index_lock:{provider_id}:{}",
        created_by
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("anonymous")
    )
}

async fn acquire_admin_pool_selection_snapshot_index_lock(
    runtime: &RuntimeState,
    lock_key: &str,
) -> Result<Option<RuntimeLockLease>, GatewayError> {
    let owner = format!(
        "aether-gateway-admin-pool-selection-snapshot-{}",
        std::process::id()
    );
    for attempt in 0..ADMIN_POOL_SELECTION_SNAPSHOT_INDEX_LOCK_RETRIES {
        let lease = runtime
            .lock_try_acquire(
                lock_key,
                &owner,
                ADMIN_POOL_SELECTION_SNAPSHOT_INDEX_LOCK_TTL,
            )
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        if lease.is_some() {
            return Ok(lease);
        }
        if attempt + 1 < ADMIN_POOL_SELECTION_SNAPSHOT_INDEX_LOCK_RETRIES {
            sleep(ADMIN_POOL_SELECTION_SNAPSHOT_INDEX_LOCK_RETRY_DELAY).await;
        }
    }
    Ok(None)
}

async fn release_admin_pool_selection_snapshot_index_lock(
    runtime: &RuntimeState,
    lease: Option<RuntimeLockLease>,
) -> Result<(), GatewayError> {
    let Some(lease) = lease else {
        return Ok(());
    };
    runtime
        .lock_release(&lease)
        .await
        .map(|_| ())
        .map_err(|err| GatewayError::Internal(err.to_string()))
}

async fn update_admin_pool_selection_snapshot_index(
    runtime: &RuntimeState,
    index_key: &str,
    snapshot_id: &str,
    ttl: Duration,
) -> Result<(), GatewayError> {
    let mut snapshot_ids = match runtime
        .kv_get(index_key)
        .await
        .map_err(|err| GatewayError::Internal(err.to_string()))?
    {
        Some(raw) => serde_json::from_str::<Vec<String>>(&raw).unwrap_or_default(),
        None => Vec::new(),
    };
    snapshot_ids.retain(|id| id != snapshot_id);
    snapshot_ids.push(snapshot_id.to_string());

    let stale_snapshot_ids =
        if snapshot_ids.len() > ADMIN_POOL_SELECTION_SNAPSHOT_MAX_ACTIVE_PER_ADMIN_PROVIDER {
            let overflow = snapshot_ids
                .len()
                .saturating_sub(ADMIN_POOL_SELECTION_SNAPSHOT_MAX_ACTIVE_PER_ADMIN_PROVIDER);
            snapshot_ids.drain(0..overflow).collect::<Vec<_>>()
        } else {
            Vec::new()
        };
    if !stale_snapshot_ids.is_empty() {
        let stale_keys = stale_snapshot_ids
            .iter()
            .map(|snapshot_id| admin_pool_selection_snapshot_key(snapshot_id))
            .collect::<Vec<_>>();
        runtime
            .kv_delete_many(&stale_keys)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
    }

    let serialized_index = serde_json::to_string(&snapshot_ids)
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    runtime
        .kv_set(index_key, serialized_index, Some(ttl))
        .await
        .map_err(|err| GatewayError::Internal(err.to_string()))
}

pub(crate) async fn store_admin_pool_selection_snapshot(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    provider_id: &str,
    filter_json: Option<Value>,
    now_unix_secs: u64,
    ttl_seconds: u64,
    items: Vec<AdminPoolSelectionSnapshotItem>,
) -> Result<AdminPoolSelectionSnapshot, GatewayError> {
    if items.len() > ADMIN_POOL_SELECTION_SNAPSHOT_MAX_TOTAL {
        return Err(GatewayError::Internal(
            "pool selection snapshot exceeded max total before route validation".to_string(),
        ));
    }

    let created_by = current_admin_user_id(request_context);
    let index_key = admin_pool_selection_snapshot_index_key(created_by.as_deref(), provider_id);
    let lock_key = admin_pool_selection_snapshot_index_lock_key(created_by.as_deref(), provider_id);
    let runtime = state.app().runtime_state();
    let lock = acquire_admin_pool_selection_snapshot_index_lock(runtime, &lock_key).await?;
    if lock.is_none() {
        return Err(GatewayError::Internal(
            "pool selection snapshot index lock busy".to_string(),
        ));
    }

    let result = async {
        let snapshot_id = Uuid::new_v4().to_string();
        let expires_at_unix_secs = now_unix_secs.saturating_add(ttl_seconds);
        let snapshot = AdminPoolSelectionSnapshot {
            id: snapshot_id,
            provider_id: provider_id.to_string(),
            created_by,
            filter_json,
            total: items.len(),
            status: ADMIN_POOL_SELECTION_SNAPSHOT_STATUS_READY.to_string(),
            created_at_unix_secs: now_unix_secs,
            expires_at_unix_secs,
            items,
        };

        let serialized_snapshot = serde_json::to_string(&snapshot)
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        let ttl = Duration::from_secs(ttl_seconds);
        runtime
            .kv_set(
                &admin_pool_selection_snapshot_key(&snapshot.id),
                serialized_snapshot,
                Some(ttl),
            )
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        update_admin_pool_selection_snapshot_index(runtime, &index_key, &snapshot.id, ttl).await?;

        Ok(snapshot)
    }
    .await;

    let release_result = release_admin_pool_selection_snapshot_index_lock(runtime, lock).await;
    match (result, release_result) {
        (Ok(snapshot), Ok(())) => Ok(snapshot),
        (Err(err), _) => Err(err),
        (Ok(_), Err(err)) => Err(err),
    }
}

pub(crate) async fn read_admin_pool_selection_snapshot(
    state: &AdminAppState<'_>,
    snapshot_id: &str,
) -> Result<Option<AdminPoolSelectionSnapshot>, GatewayError> {
    let raw = state
        .app()
        .runtime_state()
        .kv_get(&admin_pool_selection_snapshot_key(snapshot_id))
        .await
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    raw.map(|raw| serde_json::from_str::<AdminPoolSelectionSnapshot>(&raw))
        .transpose()
        .map_err(|err| GatewayError::Internal(err.to_string()))
}

pub(crate) async fn validate_admin_pool_selection_snapshot_reference(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    provider_id: &str,
    reference: &AdminPoolSelectionSnapshotReference,
) -> Result<Result<AdminPoolSelectionSnapshot, Response<Body>>, GatewayError> {
    let snapshot_id = reference.snapshot_id.trim();
    if snapshot_id.is_empty() {
        return Ok(Err(build_admin_pool_error_response(
            http::StatusCode::BAD_REQUEST,
            "selection.snapshot_id should not be empty",
        )));
    }

    let Some(snapshot) = read_admin_pool_selection_snapshot(state, snapshot_id).await? else {
        return Ok(Err(build_admin_pool_error_response(
            http::StatusCode::NOT_FOUND,
            "selection snapshot 不存在或已清理",
        )));
    };

    if snapshot.provider_id != provider_id {
        return Ok(Err(build_admin_pool_error_response(
            http::StatusCode::CONFLICT,
            "selection snapshot 与当前 Provider 不匹配",
        )));
    }
    if snapshot.status.trim() != ADMIN_POOL_SELECTION_SNAPSHOT_STATUS_READY
        || snapshot.expires_at_unix_secs <= current_unix_secs()
    {
        return Ok(Err(build_admin_pool_error_response(
            http::StatusCode::CONFLICT,
            "selection snapshot 已过期，请重新选择",
        )));
    }
    if snapshot.total != reference.expected_total {
        return Ok(Err(build_admin_pool_error_response(
            http::StatusCode::CONFLICT,
            "selection snapshot 数量与确认数量不一致，请刷新后重试",
        )));
    }

    let current_admin_user_id = current_admin_user_id(request_context);
    if snapshot.created_by.as_deref() != current_admin_user_id.as_deref() {
        return Ok(Err(build_admin_pool_error_response(
            http::StatusCode::FORBIDDEN,
            "selection snapshot 不属于当前管理员",
        )));
    }

    Ok(Ok(snapshot))
}

pub(crate) async fn resolve_admin_pool_selection_snapshot_key_ids(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    provider_id: &str,
    reference: &AdminPoolSelectionSnapshotReference,
) -> Result<Result<Vec<String>, Response<Body>>, GatewayError> {
    let snapshot = match validate_admin_pool_selection_snapshot_reference(
        state,
        request_context,
        provider_id,
        reference,
    )
    .await?
    {
        Ok(snapshot) => snapshot,
        Err(response) => return Ok(Err(response)),
    };

    if snapshot.items.len() != snapshot.total {
        return Ok(Err(build_admin_pool_error_response(
            http::StatusCode::CONFLICT,
            "selection snapshot 数据不完整，请重新选择",
        )));
    }

    let mut key_ids = Vec::with_capacity(snapshot.total);
    let mut seen_key_ids = BTreeSet::new();
    for item in snapshot.items {
        if item.key_id.trim().is_empty() {
            return Ok(Err(build_admin_pool_error_response(
                http::StatusCode::CONFLICT,
                "selection snapshot 数据不完整，请重新选择",
            )));
        }
        if !seen_key_ids.insert(item.key_id.clone()) {
            return Ok(Err(build_admin_pool_error_response(
                http::StatusCode::CONFLICT,
                "selection snapshot 包含重复账号，请重新选择",
            )));
        }
        key_ids.push(item.key_id);
    }

    Ok(Ok(key_ids))
}
