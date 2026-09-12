//! Subscription-plan request and concurrency admission for logical WS turns.
//!
//! The HTTP Upgrade is only a transport connection. Each accepted
//! `response.create` is admitted here as one logical request, while transparent
//! provider retries reuse the permit stored on that logical turn.

use axum::extract::ws::WebSocket;
use std::time::Duration;

use crate::handlers::proxy::websocket::session::{CLOSE_INTERNAL_ERROR, CLOSE_TRY_AGAIN};
use crate::handlers::proxy::websocket::transport::{
    close_client_socket, send_responses_websocket_error,
};
use crate::plan_usage_policy::PlanUsageAdmissionError;
use crate::{AppState, GatewayError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ResponsesWebSocketPlanAdmissionError {
    status: u16,
    error_type: &'static str,
    code: &'static str,
    message: &'static str,
    close_code: u16,
    close_reason: &'static str,
}

pub(super) async fn acquire_responses_websocket_plan_admission(
    state: &AppState,
    decision: &crate::control::GatewayControlDecision,
    logical_turn_id: &str,
) -> Result<crate::plan_usage_policy::PlanUsageAdmission, PlanUsageAdmissionError> {
    crate::plan_usage_policy::check_and_acquire_plan_usage_policy_admission(
        state,
        Some(decision),
        logical_turn_id,
        crate::clock::current_unix_ms(),
    )
    .await
}

pub(super) fn responses_websocket_plan_permit_is_healthy(
    permit: Option<&aether_runtime::AdmissionPermit>,
) -> bool {
    permit.is_none_or(aether_runtime::AdmissionPermit::is_healthy)
}

pub(super) async fn send_responses_websocket_plan_admission_error(
    client_socket: &mut WebSocket,
    error: &PlanUsageAdmissionError,
) {
    let mapped = map_plan_admission_error(error);
    send_responses_websocket_error(
        client_socket,
        mapped.status,
        mapped.error_type,
        mapped.code,
        mapped.message,
    )
    .await;
}

pub(super) fn responses_websocket_plan_admission_close(
    error: &PlanUsageAdmissionError,
) -> (u16, &'static str) {
    let mapped = map_plan_admission_error(error);
    (mapped.close_code, mapped.close_reason)
}

pub(super) async fn wait_for_admission_permit_loss(
    permit: Option<&aether_runtime::AdmissionPermit>,
) {
    wait_for_admission_permit_loss_with_interval(permit, Duration::from_secs(1)).await;
}

async fn wait_for_admission_permit_loss_with_interval(
    permit: Option<&aether_runtime::AdmissionPermit>,
    health_poll_interval: Duration,
) {
    let Some(permit) = permit else {
        std::future::pending::<()>().await;
        return;
    };
    let mut health = tokio::time::interval(health_poll_interval);
    health.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        health.tick().await;
        if !permit.is_healthy() {
            return;
        }
    }
}

pub(super) async fn terminate_responses_websocket_for_plan_permit_loss(
    client_socket: &mut WebSocket,
) {
    let mapped = plan_permit_loss_error();
    send_responses_websocket_error(
        client_socket,
        mapped.status,
        mapped.error_type,
        mapped.code,
        mapped.message,
    )
    .await;
    close_client_socket(client_socket, mapped.close_code, mapped.close_reason).await;
}

fn map_plan_admission_error(
    error: &PlanUsageAdmissionError,
) -> ResponsesWebSocketPlanAdmissionError {
    match error {
        PlanUsageAdmissionError::Rejected(_) => plan_limit_exceeded_error(),
        PlanUsageAdmissionError::Runtime(
            aether_runtime_state::RuntimeSemaphoreError::Saturated { .. },
        ) => plan_limit_exceeded_error(),
        PlanUsageAdmissionError::Runtime(
            aether_runtime_state::RuntimeSemaphoreError::Unavailable { .. },
        ) => ResponsesWebSocketPlanAdmissionError {
            status: 503,
            error_type: "server_error",
            code: "plan_usage_policy_unavailable",
            message: "Gateway could not evaluate the subscription plan limit",
            close_code: CLOSE_TRY_AGAIN,
            close_reason: "plan_usage_policy_unavailable",
        },
        PlanUsageAdmissionError::Runtime(
            aether_runtime_state::RuntimeSemaphoreError::InvalidConfiguration(_),
        )
        | PlanUsageAdmissionError::Gateway(GatewayError::Internal(_)) => {
            ResponsesWebSocketPlanAdmissionError {
                status: 500,
                error_type: "server_error",
                code: "plan_usage_policy_unavailable",
                message: "Gateway could not evaluate the subscription plan limit",
                close_code: CLOSE_INTERNAL_ERROR,
                close_reason: "plan_usage_policy_unavailable",
            }
        }
        PlanUsageAdmissionError::Gateway(GatewayError::Client { status, .. }) => {
            ResponsesWebSocketPlanAdmissionError {
                status: status.as_u16(),
                error_type: "invalid_request_error",
                code: "plan_usage_policy_unavailable",
                message: "Gateway could not evaluate the subscription plan limit",
                close_code: CLOSE_INTERNAL_ERROR,
                close_reason: "plan_usage_policy_unavailable",
            }
        }
        PlanUsageAdmissionError::Gateway(_) => ResponsesWebSocketPlanAdmissionError {
            status: 500,
            error_type: "server_error",
            code: "plan_usage_policy_unavailable",
            message: "Gateway could not evaluate the subscription plan limit",
            close_code: CLOSE_INTERNAL_ERROR,
            close_reason: "plan_usage_policy_unavailable",
        },
    }
}

const fn plan_permit_loss_error() -> ResponsesWebSocketPlanAdmissionError {
    ResponsesWebSocketPlanAdmissionError {
        status: 503,
        error_type: "server_error",
        code: "plan_usage_concurrency_unavailable",
        message: "Subscription plan concurrency admission was lost; retry this response",
        close_code: CLOSE_TRY_AGAIN,
        close_reason: "plan_usage_concurrency_unavailable",
    }
}

const fn plan_limit_exceeded_error() -> ResponsesWebSocketPlanAdmissionError {
    ResponsesWebSocketPlanAdmissionError {
        status: 429,
        error_type: "rate_limit_error",
        code: "plan_usage_limit_exceeded",
        message: "Subscription plan usage limit exceeded; retry later",
        close_code: CLOSE_TRY_AGAIN,
        close_reason: "plan_usage_limit_exceeded",
    }
}

#[cfg(test)]
mod tests {
    use super::{
        map_plan_admission_error, plan_permit_loss_error, responses_websocket_plan_admission_close,
    };
    use crate::plan_usage_policy::{PlanUsageAdmissionError, PlanUsagePolicyRejection};

    #[test]
    fn plan_limit_rejection_is_a_machine_readable_429() {
        let error = PlanUsageAdmissionError::Rejected(PlanUsagePolicyRejection {
            metric: "request_count",
            limit: 10.0,
            retry_after: 60,
            window: "rolling",
        });

        let mapped = map_plan_admission_error(&error);
        assert_eq!(mapped.status, 429);
        assert_eq!(mapped.error_type, "rate_limit_error");
        assert_eq!(mapped.code, "plan_usage_limit_exceeded");
        assert_eq!(
            responses_websocket_plan_admission_close(&error),
            (1013, "plan_usage_limit_exceeded")
        );
    }

    #[test]
    fn saturated_plan_concurrency_uses_the_same_limit_error() {
        let error = PlanUsageAdmissionError::Runtime(
            aether_runtime_state::RuntimeSemaphoreError::Saturated {
                gate: "plan_usage_concurrency",
                limit: 2,
            },
        );

        let mapped = map_plan_admission_error(&error);
        assert_eq!(mapped.status, 429);
        assert_eq!(mapped.code, "plan_usage_limit_exceeded");
    }

    #[test]
    fn lost_plan_permit_is_a_retryable_client_visible_503() {
        let mapped = plan_permit_loss_error();

        assert_eq!(mapped.status, 503);
        assert_eq!(mapped.error_type, "server_error");
        assert_eq!(mapped.code, "plan_usage_concurrency_unavailable");
        assert_eq!(mapped.close_code, 1013);
        assert_eq!(mapped.close_reason, "plan_usage_concurrency_unavailable");
    }
}
