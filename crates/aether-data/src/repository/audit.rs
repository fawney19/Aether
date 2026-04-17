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

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;

    use super::{read_request_audit_bundle, RequestAuditReader};
    use crate::repository::auth::{ResolvedAuthApiKeySnapshot, StoredAuthApiKeySnapshot};
    use crate::repository::candidates::{
        DecisionTrace, DecisionTraceCandidate, RequestCandidateFinalStatus, RequestCandidateStatus,
        StoredRequestCandidate,
    };
    use crate::repository::usage::StoredRequestUsageAudit;
    use crate::DataLayerError;

    #[derive(Default)]
    struct FakeRequestAuditReader {
        usage: Option<StoredRequestUsageAudit>,
        decision_trace: Option<DecisionTrace>,
        auth_snapshot: Option<ResolvedAuthApiKeySnapshot>,
        auth_snapshot_reads: AtomicUsize,
    }

    #[async_trait]
    impl RequestAuditReader for FakeRequestAuditReader {
        async fn find_request_usage_audit_by_request_id(
            &self,
            _request_id: &str,
        ) -> Result<Option<StoredRequestUsageAudit>, DataLayerError> {
            Ok(self.usage.clone())
        }

        async fn read_request_decision_trace(
            &self,
            _request_id: &str,
            _attempted_only: bool,
        ) -> Result<Option<DecisionTrace>, DataLayerError> {
            Ok(self.decision_trace.clone())
        }

        async fn read_resolved_auth_api_key_snapshot(
            &self,
            _user_id: &str,
            _api_key_id: &str,
            _now_unix_secs: u64,
        ) -> Result<Option<ResolvedAuthApiKeySnapshot>, DataLayerError> {
            self.auth_snapshot_reads.fetch_add(1, Ordering::Relaxed);
            Ok(self.auth_snapshot.clone())
        }
    }

    #[tokio::test]
    async fn read_request_audit_bundle_resolves_usage_trace_and_auth_snapshot() {
        let state = FakeRequestAuditReader {
            usage: Some(sample_usage("req-audit-1")),
            decision_trace: Some(sample_decision_trace("req-audit-1")),
            auth_snapshot: Some(sample_resolved_auth_snapshot("user-1", "api-key-1")),
            auth_snapshot_reads: AtomicUsize::new(0),
        };

        let bundle = read_request_audit_bundle(&state, "req-audit-1", true, 123)
            .await
            .expect("bundle should read")
            .expect("bundle should exist");

        assert_eq!(bundle.request_id, "req-audit-1");
        assert_eq!(
            bundle
                .usage
                .as_ref()
                .map(|usage| usage.provider_name.as_str()),
            Some("OpenAI")
        );
        assert_eq!(
            bundle
                .decision_trace
                .as_ref()
                .map(|trace| trace.total_candidates),
            Some(1)
        );
        assert_eq!(
            bundle
                .auth_snapshot
                .as_ref()
                .map(|snapshot| snapshot.api_key_id.as_str()),
            Some("api-key-1")
        );
        assert_eq!(state.auth_snapshot_reads.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn read_request_audit_bundle_returns_none_when_all_sources_are_empty() {
        let state = FakeRequestAuditReader::default();

        let bundle = read_request_audit_bundle(&state, "req-audit-empty", false, 123)
            .await
            .expect("bundle should read");

        assert!(bundle.is_none());
        assert_eq!(state.auth_snapshot_reads.load(Ordering::Relaxed), 0);
    }

    fn sample_usage(request_id: &str) -> StoredRequestUsageAudit {
        StoredRequestUsageAudit::new(
            "usage-1".to_string(),
            request_id.to_string(),
            Some("user-1".to_string()),
            Some("api-key-1".to_string()),
            Some("alice".to_string()),
            Some("default".to_string()),
            "OpenAI".to_string(),
            "gpt-4.1".to_string(),
            None,
            Some("provider-1".to_string()),
            Some("endpoint-1".to_string()),
            Some("provider-key-1".to_string()),
            Some("chat".to_string()),
            Some("openai:chat".to_string()),
            Some("openai".to_string()),
            Some("chat".to_string()),
            Some("openai:chat".to_string()),
            Some("openai".to_string()),
            Some("chat".to_string()),
            false,
            false,
            120,
            40,
            160,
            0.24,
            0.36,
            Some(200),
            None,
            None,
            Some(450),
            Some(120),
            "completed".to_string(),
            "settled".to_string(),
            100,
            101,
            Some(102),
        )
        .expect("usage should build")
    }

    fn sample_decision_trace(request_id: &str) -> DecisionTrace {
        let candidate = StoredRequestCandidate::new(
            "cand-1".to_string(),
            request_id.to_string(),
            Some("user-1".to_string()),
            Some("api-key-1".to_string()),
            Some("alice".to_string()),
            Some("default".to_string()),
            0,
            0,
            Some("provider-1".to_string()),
            Some("endpoint-1".to_string()),
            Some("provider-key-1".to_string()),
            RequestCandidateStatus::Success,
            None,
            false,
            Some(200),
            None,
            None,
            Some(37),
            None,
            None,
            None,
            100,
            Some(101),
            Some(102),
        )
        .expect("candidate should build");
        DecisionTrace {
            request_id: request_id.to_string(),
            total_candidates: 1,
            final_status: RequestCandidateFinalStatus::Success,
            total_latency_ms: 37,
            candidates: vec![DecisionTraceCandidate {
                candidate,
                provider_name: Some("OpenAI".to_string()),
                provider_website: None,
                provider_type: Some("custom".to_string()),
                provider_priority: Some(0),
                provider_keep_priority_on_conversion: Some(false),
                provider_enable_format_conversion: Some(false),
                endpoint_api_format: Some("openai:chat".to_string()),
                endpoint_api_family: Some("openai".to_string()),
                endpoint_kind: Some("chat".to_string()),
                endpoint_format_acceptance_config: None,
                provider_key_name: Some("prod".to_string()),
                provider_key_auth_type: Some("api_key".to_string()),
                provider_key_api_formats: None,
                provider_key_internal_priority: Some(10),
                provider_key_global_priority_by_format: None,
                provider_key_capabilities: None,
                provider_key_is_active: Some(true),
            }],
        }
    }

    fn sample_resolved_auth_snapshot(
        user_id: &str,
        api_key_id: &str,
    ) -> ResolvedAuthApiKeySnapshot {
        let stored = StoredAuthApiKeySnapshot::new(
            user_id.to_string(),
            "alice".to_string(),
            Some("alice@example.com".to_string()),
            "user".to_string(),
            "local".to_string(),
            true,
            false,
            Some(serde_json::json!(["openai"])),
            Some(serde_json::json!(["openai:chat"])),
            Some(serde_json::json!(["gpt-4.1"])),
            api_key_id.to_string(),
            Some("default".to_string()),
            true,
            false,
            false,
            Some(60),
            Some(5),
            Some(4_102_444_800),
            Some(serde_json::json!(["openai"])),
            Some(serde_json::json!(["openai:chat"])),
            Some(serde_json::json!(["gpt-4.1"])),
        )
        .expect("auth snapshot should build");
        ResolvedAuthApiKeySnapshot::from_stored(stored, 123)
    }
}
