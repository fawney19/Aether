use super::payment_shared::{
    generic_payment_callback_method_allowed, payment_callback_mark_failed_response,
    payment_callback_namespaced_key, payment_callback_payload_hash,
    payment_callback_persistence_projection, payment_callback_success_response,
    NormalizedPaymentCallbackRequest,
};
use aether_data::repository::wallet::{ProcessPaymentCallbackInput, ProcessPaymentCallbackOutcome};
use axum::{body::Body, http, response::Response};

use super::{
    build_auth_error_response, build_payment_callback_processing_failed_response,
    build_payment_callback_storage_unavailable_response, AppState, GatewayPublicRequestContext,
};
use tracing::warn;

pub(super) async fn handle_payment_callback_with_wallet_repository(
    state: &AppState,
    payment_method: &str,
    request_context: &GatewayPublicRequestContext,
    payload: &NormalizedPaymentCallbackRequest,
    signature_valid: bool,
) -> Response<Body> {
    if !generic_payment_callback_method_allowed(payment_method) {
        return build_auth_error_response(
            http::StatusCode::NOT_FOUND,
            "payment callback route not found",
            false,
        );
    }
    let payment_method = payment_method.trim().to_ascii_lowercase();
    if !state.has_database_wallet_data_writer() {
        return build_payment_callback_storage_unavailable_response();
    }

    let callback_payload_hash = match payment_callback_payload_hash(&payload.payload) {
        Ok(value) => value,
        Err(_) => return build_payment_callback_processing_failed_response(),
    };
    let persisted_payload =
        payment_callback_persistence_projection(&payment_method, payload, signature_valid);
    handle_payment_callback_input_with_wallet_repository(
        state,
        request_context,
        ProcessPaymentCallbackInput {
            payment_method: payment_method.clone(),
            payment_provider: None,
            payment_channel: None,
            // Keep idempotency keys in a provider namespace. A merchant can
            // legitimately reuse an event id across separate callback routes.
            callback_key: payment_callback_namespaced_key(&payment_method, &payload.callback_key),
            order_no: payload.order_no.clone(),
            gateway_order_id: payload.gateway_order_id.clone(),
            amount_usd: payload.amount_usd,
            pay_amount: payload.pay_amount,
            pay_currency: payload.pay_currency.clone(),
            exchange_rate: payload.exchange_rate,
            payload_hash: callback_payload_hash,
            payload: persisted_payload,
            signature_valid,
        },
    )
    .await
}

pub(super) async fn handle_payment_callback_input_with_wallet_repository(
    state: &AppState,
    _request_context: &GatewayPublicRequestContext,
    input: ProcessPaymentCallbackInput,
) -> Response<Body> {
    let payment_method = input.payment_method.clone();
    let outcome = match process_payment_callback_input_with_wallet_repository(state, input).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    if !reconcile_payment_callback_referral_rewards(state, &outcome, &payment_method).await {
        // The payment credit is durable, but referral creation/crediting uses
        // its own transaction.  Do not acknowledge the webhook until that
        // obligation is either applied or durably represented: providers can
        // then replay the same callback and the idempotent payment path will
        // retry referral coordination without crediting the order twice.
        return build_payment_callback_processing_failed_response();
    }

    match outcome {
        aether_data::repository::wallet::ProcessPaymentCallbackOutcome::DuplicateProcessed {
            ..
        } => payment_callback_success_response(),
        aether_data::repository::wallet::ProcessPaymentCallbackOutcome::Failed { .. } => {
            payment_callback_mark_failed_response()
        }
        aether_data::repository::wallet::ProcessPaymentCallbackOutcome::AlreadyCredited {
            ..
        } => payment_callback_success_response(),
        aether_data::repository::wallet::ProcessPaymentCallbackOutcome::Applied { .. } => {
            payment_callback_success_response()
        }
    }
}

pub(super) async fn reconcile_payment_callback_referral_rewards(
    state: &AppState,
    outcome: &ProcessPaymentCallbackOutcome,
    payment_method: &str,
) -> bool {
    let result = match outcome {
        ProcessPaymentCallbackOutcome::Applied { order, .. } => {
            state.apply_referral_rewards_for_paid_order(order).await
        }
        ProcessPaymentCallbackOutcome::AlreadyCredited { order_id, .. }
        | ProcessPaymentCallbackOutcome::DuplicateProcessed {
            order_id: Some(order_id),
        } => {
            state
                .apply_referral_rewards_for_payment_order_id(order_id)
                .await
        }
        ProcessPaymentCallbackOutcome::DuplicateProcessed { order_id: None } => {
            // A processed callback must be linked to the credited order.  A
            // missing link is corrupted durable state, so acknowledging it
            // would permanently skip referral reconciliation.
            warn!(
                error_category = "payment_callback_missing_order_link",
                payment_method, "processed payment callback has no payment order link"
            );
            return false;
        }
        ProcessPaymentCallbackOutcome::Failed { .. } => return true,
    };
    if let Err(error) = result {
        warn!(
            error = ?error,
            error_category = "referral_reward_apply_failed",
            payment_method,
            "failed to reconcile referral rewards for credited payment order"
        );
        return false;
    }
    true
}

pub(super) async fn process_payment_callback_input_with_wallet_repository(
    state: &AppState,
    input: ProcessPaymentCallbackInput,
) -> Result<ProcessPaymentCallbackOutcome, Response<Body>> {
    if !state.has_database_wallet_data_writer() {
        return Err(build_payment_callback_storage_unavailable_response());
    }

    match state.process_payment_callback(input).await {
        Ok(Some(value)) => Ok(value),
        Ok(None) => Err(build_payment_callback_storage_unavailable_response()),
        Err(_) => {
            warn!(
                error_category = "wallet_repository_failed",
                "payment callback repository processing failed"
            );
            Err(build_payment_callback_processing_failed_response())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        handle_payment_callback_with_wallet_repository,
        reconcile_payment_callback_referral_rewards, AppState, NormalizedPaymentCallbackRequest,
    };
    use crate::control::GatewayPublicRequestContext;
    use crate::handlers::public::support::support_payment::PAYMENT_CALLBACK_STORAGE_UNAVAILABLE_DETAIL;
    use aether_data::repository::wallet::ProcessPaymentCallbackOutcome;
    use axum::body::to_bytes;
    use axum::http::{HeaderMap, Method, Uri};
    use serde_json::json;

    #[tokio::test]
    async fn payment_callback_repository_handler_returns_explicit_503_without_wallet_writer() {
        let state = AppState::new().expect("state should build");
        let request_context = GatewayPublicRequestContext::from_request_parts(
            "trace-payment-callback-wallet-writer-missing",
            &Method::POST,
            &"/api/payment/callback/manual"
                .parse::<Uri>()
                .expect("uri should parse"),
            &HeaderMap::new(),
            None,
        );
        let payload = NormalizedPaymentCallbackRequest {
            callback_key: "callback-key-1".to_string(),
            order_no: Some("order-no-1".to_string()),
            gateway_order_id: Some("gateway-order-1".to_string()),
            amount_usd: 10.0,
            pay_amount: Some(10.0),
            pay_currency: Some("USD".to_string()),
            exchange_rate: Some(1.0),
            payload: json!({ "status": "paid" }),
        };

        let response = handle_payment_callback_with_wallet_repository(
            &state,
            "manual",
            &request_context,
            &payload,
            true,
        )
        .await;

        assert_eq!(response.status(), http::StatusCode::SERVICE_UNAVAILABLE);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body should read");
        let payload: serde_json::Value =
            serde_json::from_slice(&body).expect("json body should parse");
        assert_eq!(
            payload,
            json!({ "detail": PAYMENT_CALLBACK_STORAGE_UNAVAILABLE_DETAIL })
        );
    }

    #[tokio::test]
    async fn generic_repository_entrypoint_rejects_official_payment_providers() {
        let state = AppState::new().expect("state should build");
        let request_context = GatewayPublicRequestContext::from_request_parts(
            "trace-official-payment-callback-bypass",
            &Method::POST,
            &"/api/payment/callback/alipay"
                .parse::<Uri>()
                .expect("uri should parse"),
            &HeaderMap::new(),
            None,
        );
        let payload = NormalizedPaymentCallbackRequest {
            callback_key: "callback-key-1".to_string(),
            order_no: Some("order-no-1".to_string()),
            gateway_order_id: Some("gateway-order-1".to_string()),
            amount_usd: 10.0,
            pay_amount: Some(10.0),
            pay_currency: Some("USD".to_string()),
            exchange_rate: Some(1.0),
            payload: json!({ "status": "paid" }),
        };

        for provider in ["alipay", "wxpay", "stripe", "epay"] {
            let response = handle_payment_callback_with_wallet_repository(
                &state,
                provider,
                &request_context,
                &payload,
                true,
            )
            .await;
            assert_eq!(response.status(), http::StatusCode::NOT_FOUND);
        }
    }

    #[tokio::test]
    async fn processed_callback_without_order_link_is_not_acknowledged() {
        let state = AppState::new().expect("state should build");

        assert!(
            !reconcile_payment_callback_referral_rewards(
                &state,
                &ProcessPaymentCallbackOutcome::DuplicateProcessed { order_id: None },
                "stripe",
            )
            .await
        );
    }
}
