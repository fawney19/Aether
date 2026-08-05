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
            "doubao:video" => trim_doubao_video_resource_root(&plan.url)?,
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

/// Recovers the provider base URL from an Ark task URL.
///
/// This is the inverse of [`crate::doubao_video_tasks_url`]: it must strip the
/// exact path that builder appends, including the version segment. Stripping
/// less would re-add `/v3` on every follow-up request and produce `/v3/v3/...`.
fn trim_doubao_video_resource_root(url: &str) -> Option<String> {
    let base = url.split_once('?').map(|(base, _)| base).unwrap_or(url);
    let (root, suffix) = base.rsplit_once(crate::DOUBAO_VIDEO_TASKS_PATH)?;
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

#[cfg(test)]
mod tests {
    use aether_contracts::{ExecutionPlan, RequestBody};
    use serde_json::json;

    use crate::{doubao_video_tasks_url, LocalVideoTaskTransport};

    fn doubao_plan(url: &str) -> ExecutionPlan {
        ExecutionPlan {
            request_id: "req-1".to_string(),
            candidate_id: None,
            provider_name: Some("ark".to_string()),
            provider_id: "provider-1".to_string(),
            endpoint_id: "endpoint-1".to_string(),
            key_id: "key-1".to_string(),
            method: "POST".to_string(),
            url: url.to_string(),
            headers: Default::default(),
            content_type: Some("application/json".to_string()),
            content_encoding: None,
            body: RequestBody::from_json(json!({})),
            stream: false,
            client_api_format: "doubao:video".to_string(),
            provider_api_format: "doubao:video".to_string(),
            model_name: None,
            proxy: None,
            transport_profile: None,
            timeouts: None,
        }
    }

    /// Creating a task and then polling it must target the same resource.
    ///
    /// The create URL is built from the configured base, while follow-up URLs
    /// are rebuilt from a base recovered out of the create plan. If those two
    /// steps disagree the task is created successfully and then polls a 404,
    /// which surfaces only as a failed task long after the request succeeded.
    #[test]
    fn doubao_base_url_survives_a_build_parse_build_round_trip() {
        let configured_base = "https://ark.cn-beijing.volces.com/api";
        let create_url = doubao_video_tasks_url(configured_base, None);

        let transport = LocalVideoTaskTransport::from_plan(&doubao_plan(&create_url))
            .expect("doubao plan should yield a transport");
        assert_eq!(transport.upstream_base_url, configured_base);

        let poll_url = doubao_video_tasks_url(&transport.upstream_base_url, Some("cgt-1"));
        assert_eq!(
            poll_url,
            "https://ark.cn-beijing.volces.com/api/v3/contents/generations/tasks/cgt-1"
        );
    }

    #[test]
    fn doubao_transport_is_rejected_for_a_non_task_url() {
        let plan = doubao_plan("https://ark.cn-beijing.volces.com/api/v3/chat/completions");

        assert!(LocalVideoTaskTransport::from_plan(&plan).is_none());
    }
}
