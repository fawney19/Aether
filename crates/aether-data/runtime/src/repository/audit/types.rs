use async_trait::async_trait;

use crate::repository::auth::ResolvedAuthApiKeySnapshot;
use crate::repository::candidates::DecisionTrace;
use crate::repository::usage::StoredRequestUsageAudit;
use crate::DataLayerError;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RequestAuditBundle {
    pub request_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<StoredRequestUsageAudit>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decision_trace: Option<DecisionTrace>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_snapshot: Option<ResolvedAuthApiKeySnapshot>,
}

#[async_trait]
pub trait RequestAuditReader {
    async fn find_request_usage_audit_by_request_id(
        &self,
        request_id: &str,
    ) -> Result<Option<StoredRequestUsageAudit>, DataLayerError>;

    async fn read_request_decision_trace(
        &self,
        request_id: &str,
        attempted_only: bool,
    ) -> Result<Option<DecisionTrace>, DataLayerError>;

    async fn read_resolved_auth_api_key_snapshot(
        &self,
        user_id: &str,
        api_key_id: &str,
        now_unix_secs: u64,
    ) -> Result<Option<ResolvedAuthApiKeySnapshot>, DataLayerError>;
}

pub async fn read_request_audit_bundle(
    state: &impl RequestAuditReader,
    request_id: &str,
    attempted_only: bool,
    now_unix_secs: u64,
) -> Result<Option<RequestAuditBundle>, DataLayerError> {
    let usage = state
        .find_request_usage_audit_by_request_id(request_id)
        .await?;
    let decision_trace = state
        .read_request_decision_trace(request_id, attempted_only)
        .await?;

    let auth_snapshot = if let Some(usage) = usage.as_ref() {
        match (usage.user_id.as_deref(), usage.api_key_id.as_deref()) {
            (Some(user_id), Some(api_key_id)) => {
                state
                    .read_resolved_auth_api_key_snapshot(user_id, api_key_id, now_unix_secs)
                    .await?
            }
            _ => None,
        }
    } else {
        None
    };

    if usage.is_none() && decision_trace.is_none() && auth_snapshot.is_none() {
        return Ok(None);
    }

    Ok(Some(RequestAuditBundle {
        request_id: request_id.to_string(),
        usage,
        decision_trace,
        auth_snapshot,
    }))
}
