use super::execution::{
    codex_agent_identity_auth_config_from_import, estimate_admin_provider_oauth_batch_import_total,
    execute_admin_provider_oauth_batch_import_for_provider_type,
};
use super::parse::{
    admin_provider_oauth_batch_contains_agent_identity,
    build_admin_provider_oauth_batch_task_state,
    parse_admin_provider_oauth_agent_identity_import_entries,
    parse_admin_provider_oauth_batch_import_request,
};
use super::progress::{
    AdminProviderOAuthBatchImportProgress, AdminProviderOAuthBatchProgressReporter,
};
use crate::handlers::admin::provider::oauth::duplicates::codex_agent_identity_account_lock_keys;
use crate::handlers::admin::provider::oauth::errors::build_internal_control_error_response;
use crate::handlers::admin::provider::oauth::state::{
    admin_provider_oauth_template, build_admin_provider_oauth_backend_unavailable_response,
    is_fixed_provider_type_for_provider_oauth,
};
use crate::handlers::admin::provider::shared::paths::{
    admin_provider_oauth_agent_identity_import_task_provider_id,
    admin_provider_oauth_batch_import_task_provider_id,
};
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::task_runtime::{
    append_event_with_logging, now_unix_secs, task_definition, update_run_status,
    upsert_run_with_logging, TASK_KEY_PROVIDER_OAUTH_BATCH_IMPORT,
};
use crate::GatewayError;
use aether_data_contracts::repository::background_tasks::{
    BackgroundTaskKind, BackgroundTaskStatus, UpsertBackgroundTaskRun,
};
use aether_runtime_state::RuntimeLockLease;
use axum::{
    body::Bytes,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::task;
use uuid::Uuid;

const PROVIDER_OAUTH_BATCH_TASK_MAX_ERROR_SAMPLES: usize = 20;
const PROVIDER_OAUTH_BATCH_IMPORT_KIND: &str = "oauth_batch";
const PROVIDER_AGENT_IDENTITY_IMPORT_KIND: &str = "agent_identity";
const PROVIDER_AGENT_IDENTITY_IMPORT_LOCK_TTL: Duration = Duration::from_secs(180);
const PROVIDER_AGENT_IDENTITY_IMPORT_LOCK_RENEW_INTERVAL: Duration = Duration::from_secs(60);

fn provider_oauth_import_kind(agent_identity_only: bool) -> &'static str {
    if agent_identity_only {
        PROVIDER_AGENT_IDENTITY_IMPORT_KIND
    } else {
        PROVIDER_OAUTH_BATCH_IMPORT_KIND
    }
}

fn codex_agent_identity_import_auth_configs(
    credentials: &str,
) -> Result<Vec<Map<String, Value>>, String> {
    let entries = parse_admin_provider_oauth_agent_identity_import_entries(credentials)?;
    entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            if entry.parse_error.is_some() {
                return Err(format!("第 {} 个条目无效", index + 1));
            }
            match codex_agent_identity_auth_config_from_import(entry) {
                Ok(Some(auth_config)) => Ok(auth_config),
                Ok(None) => Err(format!("第 {} 个条目不是 Agent Identity", index + 1)),
                Err(_) => Err(format!("第 {} 个条目无效", index + 1)),
            }
        })
        .collect()
}

fn provider_agent_identity_import_lock_key(provider_id: &str, agent_runtime_id: &str) -> String {
    let digest = Sha256::digest(format!("{provider_id}\0{agent_runtime_id}").as_bytes());
    format!("provider_oauth_agent_identity_import:{digest:x}")
}

fn provider_agent_identity_import_lock_keys(
    provider_id: &str,
    auth_configs: &[Map<String, Value>],
) -> Vec<String> {
    let mut lock_keys = Vec::new();
    for auth_config in auth_configs {
        if let Some(agent_runtime_id) = auth_config
            .get("agent_runtime_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            lock_keys.push(provider_agent_identity_import_lock_key(
                provider_id,
                agent_runtime_id,
            ));
        }
        lock_keys.extend(codex_agent_identity_account_lock_keys(
            provider_id,
            auth_config,
        ));
    }
    lock_keys.sort_unstable();
    lock_keys.dedup();
    lock_keys
}

async fn acquire_provider_agent_identity_import_locks(
    state: &AdminAppState<'_>,
    provider_id: &str,
    lock_keys: &[String],
    task_id: &str,
) -> Result<Vec<RuntimeLockLease>, Response> {
    let owner = format!("aether-gateway-agent-identity-import-{task_id}");
    let mut leases = Vec::with_capacity(lock_keys.len());
    for lock_key in lock_keys {
        match state
            .runtime_state()
            .lock_try_acquire(
                lock_key.as_str(),
                owner.as_str(),
                PROVIDER_AGENT_IDENTITY_IMPORT_LOCK_TTL,
            )
            .await
        {
            Ok(Some(lease)) => leases.push(lease),
            Ok(None) => {
                release_provider_agent_identity_import_locks(state, leases).await;
                return Err(build_internal_control_error_response(
                    http::StatusCode::CONFLICT,
                    "其中一个 Agent Identity 正在导入或创建，请稍后重试",
                ));
            }
            Err(_) => {
                tracing::warn!(
                    provider_id = %provider_id,
                    lock_key = %lock_key,
                    "gateway Agent Identity import lock unavailable"
                );
                release_provider_agent_identity_import_locks(state, leases).await;
                return Err(build_internal_control_error_response(
                    http::StatusCode::SERVICE_UNAVAILABLE,
                    "Agent Identity 导入锁暂不可用，请稍后重试",
                ));
            }
        }
    }
    Ok(leases)
}

async fn release_provider_agent_identity_import_locks(
    state: &AdminAppState<'_>,
    leases: Vec<RuntimeLockLease>,
) {
    for lease in leases {
        match state.runtime_state().lock_release(&lease).await {
            Ok(true) => {}
            Ok(false) => tracing::warn!(
                lock_key = %lease.key,
                "gateway Agent Identity import lock was not owned during release"
            ),
            Err(_) => tracing::warn!(
                lock_key = %lease.key,
                "gateway Agent Identity import lock release failed"
            ),
        }
    }
}

async fn renew_provider_agent_identity_import_locks(
    state: &AdminAppState<'_>,
    leases: &[RuntimeLockLease],
    ttl: Duration,
) -> Result<(), String> {
    for lease in leases {
        match state.runtime_state().lock_renew(lease, ttl).await {
            Ok(true) => {}
            Ok(false) => {
                return Err(format!("Agent Identity 导入锁已失效: {}", lease.key));
            }
            Err(error) => {
                return Err(format!(
                    "Agent Identity 导入锁续租失败 ({}): {error:?}",
                    lease.key
                ));
            }
        }
    }
    Ok(())
}

struct BatchTaskProgressReporter {
    app: crate::AppState,
    task_id: String,
    provider_id: String,
    provider_type: String,
    import_kind: &'static str,
    created_at: u64,
    started_at: u64,
    error_samples: Vec<serde_json::Value>,
}

#[async_trait::async_trait]
impl AdminProviderOAuthBatchProgressReporter for BatchTaskProgressReporter {
    async fn report(&mut self, progress: AdminProviderOAuthBatchImportProgress) {
        if let Some(latest_result) = progress.latest_result.as_ref() {
            if latest_result
                .get("status")
                .and_then(serde_json::Value::as_str)
                == Some("error")
                && self.error_samples.len() < PROVIDER_OAUTH_BATCH_TASK_MAX_ERROR_SAMPLES
            {
                self.error_samples.push(latest_result.clone());
            }
        }

        let message = format!("处理中 {}/{}", progress.processed, progress.total);
        let progress_state = build_admin_provider_oauth_batch_task_state(
            &self.task_id,
            &self.provider_id,
            &self.provider_type,
            self.import_kind,
            "processing",
            progress.total,
            progress.processed,
            progress.success,
            progress.failed,
            progress.created_count,
            progress.replaced_count,
            Some(message.as_str()),
            None,
            self.error_samples.clone(),
            self.created_at,
            Some(self.started_at),
            None,
        );
        let _ = AdminAppState::new(&self.app)
            .save_provider_oauth_batch_task_payload(&self.task_id, &progress_state)
            .await;
    }
}

pub(in super::super) async fn handle_admin_provider_oauth_start_batch_import_task(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
) -> Result<Response, GatewayError> {
    handle_admin_provider_oauth_start_import_task(state, request_context, request_body, false).await
}

pub(in super::super) async fn handle_admin_provider_oauth_start_agent_identity_import_task(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
) -> Result<Response, GatewayError> {
    handle_admin_provider_oauth_start_import_task(state, request_context, request_body, true).await
}

async fn handle_admin_provider_oauth_start_import_task(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
    agent_identity_only: bool,
) -> Result<Response, GatewayError> {
    if !state.has_provider_catalog_data_reader() {
        return Ok(build_admin_provider_oauth_backend_unavailable_response());
    }
    let provider_id = if agent_identity_only {
        admin_provider_oauth_agent_identity_import_task_provider_id(request_context.path())
    } else {
        admin_provider_oauth_batch_import_task_provider_id(request_context.path())
    };
    let Some(provider_id) = provider_id else {
        return Ok(build_internal_control_error_response(
            http::StatusCode::NOT_FOUND,
            "Provider 不存在",
        ));
    };
    let payload = match parse_admin_provider_oauth_batch_import_request(request_body) {
        Ok(payload) => payload,
        Err(response) => return Ok(response),
    };
    if !agent_identity_only
        && admin_provider_oauth_batch_contains_agent_identity(&payload.credentials)
    {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "Agent Identity JSON 必须使用专属导入接口",
        ));
    }

    let Some(provider) = state
        .read_provider_catalog_providers_by_ids(std::slice::from_ref(&provider_id))
        .await?
        .into_iter()
        .next()
    else {
        return Ok(build_internal_control_error_response(
            http::StatusCode::NOT_FOUND,
            "Provider 不存在",
        ));
    };
    let provider_type = provider.provider_type.trim().to_ascii_lowercase();
    if agent_identity_only && provider_type != "codex" {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "仅 Codex Provider 支持导入 Agent Identity",
        ));
    }
    if !is_fixed_provider_type_for_provider_oauth(&provider_type) {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "该 Provider 不是固定类型，无法使用 provider-oauth",
        ));
    }
    if provider_type != "kiro"
        && provider_type != "windsurf"
        && admin_provider_oauth_template(&provider_type).is_none()
    {
        return Ok(build_admin_provider_oauth_backend_unavailable_response());
    }

    let agent_identity_auth_configs = if agent_identity_only {
        match codex_agent_identity_import_auth_configs(&payload.credentials) {
            Ok(auth_configs) => Some(auth_configs),
            Err(_) => {
                return Ok(build_internal_control_error_response(
                    http::StatusCode::BAD_REQUEST,
                    "该接口仅接受有效的 Agent Identity JSON",
                ));
            }
        }
    } else {
        None
    };
    let total = agent_identity_auth_configs.as_ref().map_or_else(
        || {
            estimate_admin_provider_oauth_batch_import_total(
                &provider_type,
                payload.credentials.as_str(),
            )
        },
        Vec::len,
    );
    if total == 0 {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "未找到有效的 Token 数据",
        ));
    }

    let task_id = if agent_identity_only {
        format!("agent-identity-{}", Uuid::new_v4())
    } else {
        Uuid::new_v4().to_string()
    };
    let mut agent_identity_import_leases =
        if let Some(auth_configs) = agent_identity_auth_configs.as_deref() {
            let lock_keys = provider_agent_identity_import_lock_keys(&provider_id, auth_configs);
            match acquire_provider_agent_identity_import_locks(
                state,
                &provider_id,
                &lock_keys,
                &task_id,
            )
            .await
            {
                Ok(leases) => leases,
                Err(response) => return Ok(response),
            }
        } else {
            Vec::new()
        };
    let import_kind = provider_oauth_import_kind(agent_identity_only);
    let created_at = now_unix_secs();
    let submitted_state = build_admin_provider_oauth_batch_task_state(
        &task_id,
        &provider_id,
        &provider_type,
        import_kind,
        "submitted",
        total,
        0,
        0,
        0,
        0,
        0,
        Some("任务已提交，等待执行"),
        None,
        Vec::new(),
        created_at,
        None,
        None,
    );
    if state
        .save_provider_oauth_batch_task_payload(&task_id, &submitted_state)
        .await
        .is_err()
    {
        release_provider_agent_identity_import_locks(
            state,
            std::mem::take(&mut agent_identity_import_leases),
        )
        .await;
        return Ok(build_internal_control_error_response(
            http::StatusCode::SERVICE_UNAVAILABLE,
            "provider oauth batch task redis unavailable",
        ));
    }

    if state.has_background_task_data_writer() {
        let max_attempts = task_definition(TASK_KEY_PROVIDER_OAUTH_BATCH_IMPORT)
            .map(|item| item.retry_policy.max_attempts)
            .unwrap_or(1);
        let run = UpsertBackgroundTaskRun {
            id: task_id.clone(),
            task_key: TASK_KEY_PROVIDER_OAUTH_BATCH_IMPORT.to_string(),
            kind: BackgroundTaskKind::OnDemand,
            trigger: "manual".to_string(),
            status: BackgroundTaskStatus::Queued,
            attempt: 1,
            max_attempts,
            owner_instance: Some(state.app().tunnel.local_instance_id().to_string()),
            progress_percent: 0,
            progress_message: Some("provider oauth batch import queued".to_string()),
            payload_json: Some(json!({
                "provider_id": provider_id.clone(),
                "provider_type": provider_type.clone(),
                "import_kind": import_kind,
                "total": total,
            })),
            result_json: None,
            error_message: None,
            cancel_requested: false,
            created_by: Some("admin".to_string()),
            created_at_unix_secs: created_at,
            started_at_unix_secs: None,
            finished_at_unix_secs: None,
            updated_at_unix_secs: created_at,
        };
        let _ = upsert_run_with_logging(state.app(), run).await;
        append_event_with_logging(
            state.app(),
            &task_id,
            "queued",
            "provider oauth batch import queued",
            Some(json!({
                "provider_id": provider_id.clone(),
                "provider_type": provider_type.clone(),
                "import_kind": import_kind,
                "total": total,
            })),
        )
        .await;
    }

    let task_state = state.cloned_app();
    let task_id_for_worker = task_id.clone();
    let provider_id_for_worker = provider_id.clone();
    let provider_type_for_worker = provider_type.clone();
    let proxy_node_id = payload.proxy_node_id.clone();
    let raw_credentials = payload.credentials.clone();
    let agent_identity_import_leases_for_worker = std::mem::take(&mut agent_identity_import_leases);
    task::spawn(async move {
        let started_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|duration| duration.as_secs())
            .unwrap_or(created_at);
        let processing_state = build_admin_provider_oauth_batch_task_state(
            &task_id_for_worker,
            &provider_id_for_worker,
            &provider_type_for_worker,
            import_kind,
            "processing",
            total,
            0,
            0,
            0,
            0,
            0,
            Some("任务开始执行"),
            None,
            Vec::new(),
            created_at,
            Some(started_at),
            None,
        );
        let _ = AdminAppState::new(&task_state)
            .save_provider_oauth_batch_task_payload(&task_id_for_worker, &processing_state)
            .await;

        let _ = update_run_status(
            &task_state,
            &task_id_for_worker,
            BackgroundTaskStatus::Running,
            Some(1),
            Some("provider oauth batch import started".to_string()),
            None,
            None,
            Some(started_at),
            None,
        )
        .await;
        append_event_with_logging(
            &task_state,
            &task_id_for_worker,
            "running",
            "provider oauth batch import started",
            None,
        )
        .await;

        let mut progress_reporter = BatchTaskProgressReporter {
            app: task_state.clone(),
            task_id: task_id_for_worker.clone(),
            provider_id: provider_id_for_worker.clone(),
            provider_type: provider_type_for_worker.clone(),
            import_kind,
            created_at,
            started_at,
            error_samples: Vec::new(),
        };
        let task_admin_state = AdminAppState::new(&task_state);
        let execution = execute_admin_provider_oauth_batch_import_for_provider_type(
            &task_admin_state,
            &provider_id_for_worker,
            &provider_type_for_worker,
            raw_credentials.as_str(),
            proxy_node_id.as_deref(),
            Some(&mut progress_reporter),
        );
        tokio::pin!(execution);
        let execution_result = if agent_identity_import_leases_for_worker.is_empty() {
            execution.await
        } else {
            let mut renew_timer =
                tokio::time::interval(PROVIDER_AGENT_IDENTITY_IMPORT_LOCK_RENEW_INTERVAL);
            renew_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            renew_timer.tick().await;
            loop {
                tokio::select! {
                    result = &mut execution => break result,
                    _ = renew_timer.tick() => {
                        if let Err(detail) = renew_provider_agent_identity_import_locks(
                            &task_admin_state,
                            &agent_identity_import_leases_for_worker,
                            PROVIDER_AGENT_IDENTITY_IMPORT_LOCK_TTL,
                        ).await {
                            tracing::error!(
                                provider_id = %provider_id_for_worker,
                                task_id = %task_id_for_worker,
                                detail = %detail,
                                "gateway Agent Identity import lease lost"
                            );
                            break Err(GatewayError::Internal(detail));
                        }
                    }
                }
            }
        };
        release_provider_agent_identity_import_locks(
            &task_admin_state,
            agent_identity_import_leases_for_worker,
        )
        .await;
        match execution_result {
            Ok(outcome) => {
                let finished_at = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .ok()
                    .map(|duration| duration.as_secs())
                    .unwrap_or(started_at);
                let error_samples = outcome
                    .results
                    .iter()
                    .filter(|item| {
                        item.get("status").and_then(serde_json::Value::as_str) == Some("error")
                    })
                    .take(PROVIDER_OAUTH_BATCH_TASK_MAX_ERROR_SAMPLES)
                    .cloned()
                    .collect::<Vec<_>>();
                let replaced_count = outcome
                    .results
                    .iter()
                    .filter(|item| {
                        item.get("status").and_then(serde_json::Value::as_str) == Some("success")
                            && item.get("replaced").and_then(serde_json::Value::as_bool)
                                == Some(true)
                    })
                    .count();
                let created_count = outcome.success.saturating_sub(replaced_count);
                let message = format!(
                    "导入完成：成功 {}，失败 {}",
                    outcome.success, outcome.failed
                );
                let completed_state = build_admin_provider_oauth_batch_task_state(
                    &task_id_for_worker,
                    &provider_id_for_worker,
                    &provider_type_for_worker,
                    import_kind,
                    "completed",
                    outcome.total,
                    outcome.total,
                    outcome.success,
                    outcome.failed,
                    created_count,
                    replaced_count,
                    Some(message.as_str()),
                    None,
                    error_samples,
                    created_at,
                    Some(started_at),
                    Some(finished_at),
                );
                let _ = AdminAppState::new(&task_state)
                    .save_provider_oauth_batch_task_payload(&task_id_for_worker, &completed_state)
                    .await;
                let _ = update_run_status(
                    &task_state,
                    &task_id_for_worker,
                    BackgroundTaskStatus::Succeeded,
                    Some(100),
                    Some(message),
                    Some(json!({
                        "provider_id": provider_id_for_worker,
                        "provider_type": provider_type_for_worker,
                        "import_kind": import_kind,
                        "total": outcome.total,
                        "success": outcome.success,
                        "failed": outcome.failed,
                        "created_count": created_count,
                        "replaced_count": replaced_count,
                    })),
                    None,
                    None,
                    Some(finished_at),
                )
                .await;
                append_event_with_logging(
                    &task_state,
                    &task_id_for_worker,
                    "succeeded",
                    "provider oauth batch import completed",
                    None,
                )
                .await;
            }
            Err(_) => {
                let finished_at = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .ok()
                    .map(|duration| duration.as_secs())
                    .unwrap_or(started_at);
                let failed_state = build_admin_provider_oauth_batch_task_state(
                    &task_id_for_worker,
                    &provider_id_for_worker,
                    &provider_type_for_worker,
                    import_kind,
                    "failed",
                    total,
                    0,
                    0,
                    0,
                    0,
                    0,
                    Some("导入任务执行失败"),
                    Some("provider_oauth_batch_import_failed"),
                    Vec::new(),
                    created_at,
                    Some(started_at),
                    Some(finished_at),
                );
                let _ = AdminAppState::new(&task_state)
                    .save_provider_oauth_batch_task_payload(&task_id_for_worker, &failed_state)
                    .await;
                let _ = update_run_status(
                    &task_state,
                    &task_id_for_worker,
                    BackgroundTaskStatus::Failed,
                    Some(100),
                    Some("provider oauth batch import failed".to_string()),
                    None,
                    Some("provider_oauth_batch_import_failed".to_string()),
                    None,
                    Some(finished_at),
                )
                .await;
                append_event_with_logging(
                    &task_state,
                    &task_id_for_worker,
                    "failed",
                    "provider oauth batch import failed",
                    Some(json!({
                        "error_code": "provider_oauth_batch_import_failed"
                    })),
                )
                .await;
                tracing::warn!(
                    task_id = %task_id_for_worker,
                    provider_id = %provider_id_for_worker,
                    error_category = "provider_oauth_batch_import_failed",
                    "provider oauth batch import task failed"
                );
            }
        }
    });

    let submitted_response = build_admin_provider_oauth_batch_task_state(
        &task_id,
        &provider_id,
        &provider_type,
        import_kind,
        "submitted",
        total,
        0,
        0,
        0,
        0,
        0,
        Some("任务已提交，等待执行"),
        None,
        Vec::new(),
        created_at,
        None,
        None,
    );
    Ok(Json(submitted_response).into_response())
}

#[cfg(test)]
mod tests {
    use super::{
        acquire_provider_agent_identity_import_locks, codex_agent_identity_import_auth_configs,
        provider_agent_identity_import_lock_key, provider_agent_identity_import_lock_keys,
        release_provider_agent_identity_import_locks, renew_provider_agent_identity_import_locks,
    };
    use crate::handlers::admin::request::AdminAppState;
    use crate::AppState;
    use serde_json::json;

    fn agent_identity(runtime_id: &str) -> serde_json::Value {
        json!({
            "auth_mode": "agentIdentity",
            "agent_runtime_id": runtime_id,
            "agent_private_key": "MC4CAQAwBQYDK2VwBCIEIAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "task_id": format!("task-{runtime_id}"),
            "account_id": "account-1",
            "user_id": "user-1",
            "email": "agent@example.com"
        })
    }

    #[test]
    fn dedicated_import_accepts_root_array_and_sub2api_agent_identities() {
        let single = agent_identity("runtime-1");
        assert_eq!(
            codex_agent_identity_import_auth_configs(&single.to_string())
                .expect("single Agent Identity should parse")
                .len(),
            1
        );

        let array = json!([agent_identity("runtime-1"), agent_identity("runtime-2")]);
        assert_eq!(
            codex_agent_identity_import_auth_configs(&array.to_string())
                .expect("Agent Identity array should parse")
                .len(),
            2
        );

        let sub2api = json!({
            "type": "sub2api-data",
            "accounts": [
                {
                    "name": "agent-1@example.com",
                    "platform": "openai",
                    "credentials": agent_identity("runtime-1")
                },
                {
                    "name": "agent-2@example.com",
                    "platform": "openai",
                    "credentials": agent_identity("runtime-2")
                },
                {
                    "name": "ignored@example.com",
                    "platform": "anthropic",
                    "credentials": { "access_token": "ignored" }
                }
            ]
        });
        assert_eq!(
            codex_agent_identity_import_auth_configs(&sub2api.to_string())
                .expect("sub2api Agent Identity export should parse")
                .len(),
            2
        );
    }

    #[test]
    fn dedicated_import_rejects_mixed_and_invalid_entries() {
        let mixed = json!([
            agent_identity("runtime-1"),
            { "refresh_token": "refresh-token" }
        ]);
        assert!(codex_agent_identity_import_auth_configs(&mixed.to_string()).is_err());

        let invalid = json!({
            "auth_mode": "agentIdentity",
            "agent_runtime_id": "runtime-invalid",
            "agent_private_key": "not-a-pkcs8-key"
        });
        assert!(codex_agent_identity_import_auth_configs(&invalid.to_string()).is_err());
        assert!(codex_agent_identity_import_auth_configs("[]").is_err());
    }

    #[test]
    fn agent_identity_import_lock_key_is_scoped_to_provider_and_runtime() {
        let first = provider_agent_identity_import_lock_key("provider-a", "runtime-1");
        assert_eq!(
            first,
            provider_agent_identity_import_lock_key("provider-a", "runtime-1")
        );
        assert_ne!(
            first,
            provider_agent_identity_import_lock_key("provider-b", "runtime-1")
        );
        assert_ne!(
            first,
            provider_agent_identity_import_lock_key("provider-a", "runtime-2")
        );
    }

    #[test]
    fn agent_identity_imports_with_distinct_runtimes_share_account_lock_keys() {
        let first =
            codex_agent_identity_import_auth_configs(&agent_identity("runtime-1").to_string())
                .expect("first Agent Identity should parse");
        let second =
            codex_agent_identity_import_auth_configs(&agent_identity("runtime-2").to_string())
                .expect("second Agent Identity should parse");
        let first_keys = provider_agent_identity_import_lock_keys("provider-a", &first);
        let second_keys = provider_agent_identity_import_lock_keys("provider-a", &second);

        assert!(first_keys.iter().any(|key| second_keys.contains(key)));
        assert!(
            first_keys.contains(&provider_agent_identity_import_lock_key(
                "provider-a",
                "runtime-1"
            ))
        );
        assert!(
            second_keys.contains(&provider_agent_identity_import_lock_key(
                "provider-a",
                "runtime-2"
            ))
        );
    }

    #[tokio::test]
    async fn agent_identity_import_runtime_lock_releases_after_contention() {
        let app = AppState::new().expect("app state should build");
        let state = AdminAppState::new(&app);
        let lock_keys = vec![provider_agent_identity_import_lock_key(
            "provider-a",
            "runtime-1",
        )];
        let first = acquire_provider_agent_identity_import_locks(
            &state,
            "provider-a",
            &lock_keys,
            "agent-identity-task-1",
        )
        .await
        .expect("first lock should acquire");
        let second = acquire_provider_agent_identity_import_locks(
            &state,
            "provider-a",
            &lock_keys,
            "agent-identity-task-2",
        )
        .await
        .expect_err("second lock should be rejected");
        assert_eq!(second.status(), axum::http::StatusCode::CONFLICT);
        release_provider_agent_identity_import_locks(&state, first).await;
        let third = acquire_provider_agent_identity_import_locks(
            &state,
            "provider-a",
            &lock_keys,
            "agent-identity-task-3",
        )
        .await
        .expect("lock should be reusable after release");
        release_provider_agent_identity_import_locks(&state, third).await;
    }

    #[tokio::test]
    async fn agent_identity_import_partial_lock_failure_releases_acquired_leases() {
        let app = AppState::new().expect("app state should build");
        let state = AdminAppState::new(&app);
        let held = state
            .runtime_state()
            .lock_try_acquire("z-held", "other-task", std::time::Duration::from_secs(30))
            .await
            .expect("runtime lock should be available")
            .expect("held lock should acquire");
        let lock_keys = vec!["a-free".to_string(), "z-held".to_string()];

        let response = acquire_provider_agent_identity_import_locks(
            &state,
            "provider-a",
            &lock_keys,
            "agent-identity-task-partial",
        )
        .await
        .expect_err("second lock should cause contention");
        assert_eq!(response.status(), axum::http::StatusCode::CONFLICT);

        let free = state
            .runtime_state()
            .lock_try_acquire(
                "a-free",
                "verification-task",
                std::time::Duration::from_secs(30),
            )
            .await
            .expect("runtime lock should be available")
            .expect("partially acquired lock should have been released");
        assert!(state
            .runtime_state()
            .lock_release(&free)
            .await
            .expect("free lock should release"));
        assert!(state
            .runtime_state()
            .lock_release(&held)
            .await
            .expect("held lock should release"));
    }

    #[tokio::test]
    async fn agent_identity_import_lock_renewal_extends_all_leases() {
        let app = AppState::new().expect("app state should build");
        let state = AdminAppState::new(&app);
        let lease = state
            .runtime_state()
            .lock_try_acquire(
                "renewed-agent-lock",
                "agent-identity-task-renew",
                std::time::Duration::from_secs(1),
            )
            .await
            .expect("runtime lock should be available")
            .expect("lock should acquire");
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        renew_provider_agent_identity_import_locks(
            &state,
            std::slice::from_ref(&lease),
            std::time::Duration::from_secs(2),
        )
        .await
        .expect("lock should renew");
        tokio::time::sleep(std::time::Duration::from_millis(1_000)).await;

        let contender = state
            .runtime_state()
            .lock_try_acquire(
                "renewed-agent-lock",
                "contending-task",
                std::time::Duration::from_secs(1),
            )
            .await
            .expect("runtime lock should be available");
        assert!(contender.is_none(), "renewed lease should still be held");
        release_provider_agent_identity_import_locks(&state, vec![lease]).await;
    }
}
