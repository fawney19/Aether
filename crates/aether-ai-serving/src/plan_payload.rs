use aether_ai_formats::api::{
    ExecutionRuntimeAuthContext, EXECUTION_RUNTIME_STREAM_ACTION, EXECUTION_RUNTIME_SYNC_ACTION,
};

use crate::dto::{AiExecutionPlanPayload, AiStreamAttempt, AiSyncAttempt};

pub fn build_ai_sync_execution_plan_payload(
    plan_kind: &str,
    attempt: AiSyncAttempt,
    auth_context: Option<ExecutionRuntimeAuthContext>,
) -> AiExecutionPlanPayload {
    AiExecutionPlanPayload {
        action: EXECUTION_RUNTIME_SYNC_ACTION.to_string(),
        plan_kind: Some(plan_kind.to_string()),
        plan: Some(attempt.plan),
        report_kind: attempt.report_kind,
        report_context: attempt.report_context,
        auth_context,
    }
}

pub fn build_ai_stream_execution_plan_payload(
    plan_kind: &str,
    attempt: AiStreamAttempt,
    auth_context: Option<ExecutionRuntimeAuthContext>,
) -> AiExecutionPlanPayload {
    AiExecutionPlanPayload {
        action: EXECUTION_RUNTIME_STREAM_ACTION.to_string(),
        plan_kind: Some(plan_kind.to_string()),
        plan: Some(attempt.plan),
        report_kind: attempt.report_kind,
        report_context: attempt.report_context,
        auth_context,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_contracts::{ExecutionPlan, RequestBody};

    #[test]
    fn sync_plan_payload_uses_sync_action_and_attempt_report_fields() {
        let payload = build_ai_sync_execution_plan_payload(
            "openai_chat_sync",
            AiSyncAttempt {
                plan: test_plan(),
                report_kind: Some("sync_success".to_string()),
                report_context: Some(serde_json::json!({"candidate_index": 0})),
            },
            None,
        );

        assert_eq!(payload.action, EXECUTION_RUNTIME_SYNC_ACTION);
        assert_eq!(payload.plan_kind.as_deref(), Some("openai_chat_sync"));
        assert_eq!(payload.report_kind.as_deref(), Some("sync_success"));
        assert_eq!(
            payload
                .report_context
                .as_ref()
                .and_then(|value| value.get("candidate_index"))
                .and_then(serde_json::Value::as_u64),
            Some(0)
        );
        assert!(payload.plan.is_some());
    }

    #[test]
    fn stream_plan_payload_uses_stream_action() {
        let payload = build_ai_stream_execution_plan_payload(
            "openai_chat_stream",
            AiStreamAttempt {
                plan: test_plan(),
                report_kind: None,
                report_context: None,
            },
            None,
        );

        assert_eq!(payload.action, EXECUTION_RUNTIME_STREAM_ACTION);
        assert_eq!(payload.plan_kind.as_deref(), Some("openai_chat_stream"));
        assert!(payload.plan.is_some());
    }

    fn test_plan() -> ExecutionPlan {
        ExecutionPlan {
            request_id: "req_1".to_string(),
            candidate_id: None,
            provider_name: Some("provider".to_string()),
            provider_id: "provider_id".to_string(),
            endpoint_id: "endpoint_id".to_string(),
            key_id: "key_id".to_string(),
            method: "POST".to_string(),
            url: "https://example.com/v1/chat/completions".to_string(),
            headers: Default::default(),
            content_type: Some("application/json".to_string()),
            content_encoding: None,
            body: RequestBody::from_json(serde_json::json!({"model": "model"})),
            stream: false,
            client_api_format: "openai:chat".to_string(),
            provider_api_format: "openai:chat".to_string(),
            model_name: Some("model".to_string()),
            proxy: None,
            tls_profile: None,
            timeouts: None,
        }
    }
}
