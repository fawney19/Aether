use aether_contracts::ExecutionPlan;
use aether_data_contracts::repository::video_tasks::{
    StoredVideoTask, VideoTaskStatus as StoredVideoTaskStatus,
};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

use crate::{
    context_text, non_empty_owned, LocalVideoTaskPersistence, LocalVideoTaskStatus,
    LocalVideoTaskTransport, LocalVideoTaskTransportBridgeInput,
};

impl LocalVideoTaskTransport {
    pub fn from_plan(plan: &ExecutionPlan) -> Option<Self> {
        let upstream_base_url = match plan.provider_api_format.as_str() {
            "openai:video" => trim_openai_video_resource_root(&plan.url)?,
            "gemini:video" => plan.url.split("/v1beta/").next()?.to_string(),
            _ => return None,
        };
        if upstream_base_url.is_empty() {
            return None;
        }
        Some(Self {
            upstream_base_url,
            provider_name: plan.provider_name.clone(),
            provider_id: plan.provider_id.clone(),
            endpoint_id: plan.endpoint_id.clone(),
            key_id: plan.key_id.clone(),
            headers: plan.headers.clone(),
            content_type: plan.content_type.clone(),
            model_name: plan.model_name.clone(),
            proxy: plan.proxy.clone(),
            transport_profile: plan.transport_profile.clone(),
            timeouts: plan.timeouts.clone(),
        })
    }

    pub fn from_bridge_input(input: LocalVideoTaskTransportBridgeInput) -> Self {
        let mut headers = BTreeMap::new();
        headers.insert(input.auth_header, input.auth_value);

        Self {
            upstream_base_url: input.upstream_base_url,
            provider_name: input.provider_name,
            provider_id: input.provider_id,
            endpoint_id: input.endpoint_id,
            key_id: input.key_id,
            headers,
            content_type: input.content_type,
            model_name: input.model_name,
            proxy: input.proxy,
            transport_profile: input.transport_profile,
            timeouts: input.timeouts,
        }
    }
}

fn trim_openai_video_resource_root(url: &str) -> Option<String> {
    let base = url.split_once('?').map(|(base, _)| base).unwrap_or(url);
    let (root, suffix) = base.rsplit_once("/videos")?;
    if !suffix.is_empty() && !suffix.starts_with('/') {
        return None;
    }
    Some(root.to_string())
}

impl LocalVideoTaskPersistence {
    pub fn from_report_context(report_context: &Map<String, Value>, plan: &ExecutionPlan) -> Self {
        Self {
            request_id: context_text(report_context, "request_id")
                .unwrap_or_else(|| plan.request_id.clone()),
            username: context_text(report_context, "username"),
            api_key_name: context_text(report_context, "api_key_name"),
            client_api_format: context_text(report_context, "client_api_format")
                .unwrap_or_else(|| plan.client_api_format.clone()),
            provider_api_format: context_text(report_context, "provider_api_format")
                .unwrap_or_else(|| plan.provider_api_format.clone()),
            original_request_body: report_context
                .get("original_request_body")
                .cloned()
                .unwrap_or_else(|| Value::Object(Map::new())),
            format_converted: report_context
                .get("format_converted")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        }
    }

    pub fn from_stored_task(task: &StoredVideoTask) -> Option<Self> {
        let client_api_format = non_empty_owned(task.client_api_format.as_ref())
            .or_else(|| non_empty_owned(task.provider_api_format.as_ref()))?;
        let provider_api_format = non_empty_owned(task.provider_api_format.as_ref())
            .or_else(|| non_empty_owned(task.client_api_format.as_ref()))?;

        Some(Self {
            request_id: task.request_id.clone(),
            username: task.username.clone(),
            api_key_name: task.api_key_name.clone(),
            client_api_format,
            provider_api_format,
            original_request_body: task
                .original_request_body
                .clone()
                .unwrap_or_else(|| Value::Object(Map::new())),
            format_converted: task.format_converted,
        })
    }
}

impl LocalVideoTaskStatus {
    pub fn as_database_status(self) -> StoredVideoTaskStatus {
        match self {
            Self::Submitted => StoredVideoTaskStatus::Submitted,
            Self::Queued => StoredVideoTaskStatus::Queued,
            Self::Processing => StoredVideoTaskStatus::Processing,
            Self::Completed => StoredVideoTaskStatus::Completed,
            Self::Failed => StoredVideoTaskStatus::Failed,
            Self::Cancelled => StoredVideoTaskStatus::Cancelled,
            Self::Expired => StoredVideoTaskStatus::Expired,
            Self::Deleted => StoredVideoTaskStatus::Deleted,
        }
    }
}
