use super::payment_shared::{
    payment_callback_mark_failed_response, payment_callback_payload_hash,
    NormalizedPaymentCallbackRequest,
};
use aether_data::repository::wallet::{ProcessPaymentCallbackInput, ProcessPaymentCallbackOutcome};
use axum::{body::Body, http, response::Response};
use serde_json::json;

use super::super::build_auth_json_response;
use super::{
    build_auth_error_response, build_payment_callback_storage_unavailable_response, AppState,
    GatewayPublicRequestContext,
};
use tracing::warn;

pub(super) async fn handle_payment_callback_with_wallet_repository(
    state: &AppState,
    payment_method: &str,
    request_context: &GatewayPublicRequestContext,
    payload: &NormalizedPaymentCallbackRequest,
    signature_valid: bool,
) -> Response<Body> {
    if !state.has_database_wallet_data_writer() {
        return build_payment_callback_storage_unavailable_response();
    }

    let callback_payload_hash = match payment_callback_payload_hash(&payload.payload) {
        Ok(value) => value,
        Err(err) => {
            return build_auth_error_response(http::StatusCode::INTERNAL_SERVER_ERROR, err, false)
        }
    };
    handle_payment_callback_input_with_wallet_repository(
        state,
        request_context,
        ProcessPaymentCallbackInput {
            payment_method: payment_method.to_string(),
            payment_provider: None,
            payment_channel: None,
            callback_key: payload.callback_key.clone(),
            order_no: payload.order_no.clone(),
            gateway_order_id: payload.gateway_order_id.clone(),
            amount_usd: payload.amount_usd,
            pay_amount: payload.pay_amount,
            pay_currency: payload.pay_currency.clone(),
            exchange_rate: payload.exchange_rate,
            payload_hash: callback_payload_hash,
            payload: payload.payload.clone(),
            signature_valid,
        },
    )
    .await
}

pub(super) async fn handle_payment_callback_input_with_wallet_repository(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    input: ProcessPaymentCallbackInput,
) -> Response<Body> {
    let payment_method = input.payment_method.clone();
    let outcome = match process_payment_callback_input_with_wallet_repository(state, input).await {
        Ok(value) => value,
        Err(response) => return response,
    };

    match outcome {
        aether_data::repository::wallet::ProcessPaymentCallbackOutcome::DuplicateProcessed {
            order_id,
        } => build_auth_json_response(
            http::StatusCode::OK,
            json!({
                "ok": true,
                "duplicate": true,
                "credited": false,
                "order_id": order_id,
                "payment_method": payment_method,
                "request_path": request_context.request_path,
            }),
            None,
        ),
        aether_data::repository::wallet::ProcessPaymentCallbackOutcome::Failed {
            duplicate,
            error,
        } => payment_callback_mark_failed_response(
            duplicate,
            &error,
            &payment_method,
            &request_context.request_path,
        ),
        aether_data::repository::wallet::ProcessPaymentCallbackOutcome::AlreadyCredited {
            duplicate,
            order_id,
            order_no,
            wallet_id,
        } => build_auth_json_response(
            http::StatusCode::OK,
            json!({
                "ok": true,
                "duplicate": duplicate,
                "credited": false,
                "order_id": order_id,
                "order_no": order_no,
                "status": "credited",
                "wallet_id": wallet_id,
                "payment_method": payment_method,
                "request_path": request_context.request_path,
            }),
            None,
        ),
        aether_data::repository::wallet::ProcessPaymentCallbackOutcome::Applied {
            duplicate,
            order_id,
            order_no,
            wallet_id,
            order,
        } => {
            if let Err(err) = state.apply_referral_rewards_for_paid_order(&order).await {
                warn!(
                    error = ?err,
                    order_id = %order_id,
                    "failed to apply referral rewards for credited payment order"
                );
            }
            build_auth_json_response(
                http::StatusCode::OK,
                json!({
                    "ok": true,
                    "duplicate": duplicate,
                    "credited": true,
                    "order_id": order_id,
                    "order_no": order_no,
                    "status": order.status,
                    "wallet_id": wallet_id,
                    "payment_method": payment_method,
                    "request_path": request_context.request_path,
                }),
                None,
            )
        }
    }
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
        Err(err) => Err(build_auth_error_response(
            http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("payment callback failed: {err:?}"),
            false,
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        handle_payment_callback_with_wallet_repository, AppState, NormalizedPaymentCallbackRequest,
    };
    use crate::control::GatewayPublicRequestContext;
    use crate::handlers::public::support::support_payment::PAYMENT_CALLBACK_STORAGE_UNAVAILABLE_DETAIL;
    use axum::body::to_bytes;
    use axum::http::{HeaderMap, Method, Uri};
    use serde_json::json;

    #[tokio::test]
    async fn payment_callback_repository_handler_returns_explicit_503_without_wallet_writer() {
        let state = AppState::new().expect("state should build");
        let request_context = GatewayPublicRequestContext::from_request_parts(
            "trace-payment-callback-wallet-writer-missing",
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

        let response = handle_payment_callback_with_wallet_repository(
            &state,
            "alipay",
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
}
