use axum::{body::Body, http, response::Response};

pub(super) use super::{
    build_auth_error_response, build_auth_json_response, AppState, GatewayPublicRequestContext,
};

#[path = "payment/alipay.rs"]
mod payment_alipay;
#[path = "payment/epay.rs"]
pub(super) mod payment_epay;
#[path = "payment/gateway.rs"]
pub(super) mod payment_gateway;
#[path = "payment/repository.rs"]
mod payment_repository;
#[path = "payment/route.rs"]
mod payment_route;
#[path = "payment/shared.rs"]
mod payment_shared;
#[path = "payment/stripe.rs"]
mod payment_stripe;
#[cfg(test)]
#[path = "payment/test_support.rs"]
mod payment_test_support;
#[path = "payment/wxpay.rs"]
mod payment_wxpay;

use self::payment_repository::{
    handle_payment_callback_input_with_wallet_repository,
    handle_payment_callback_with_wallet_repository,
    process_payment_callback_input_with_wallet_repository,
    reconcile_payment_callback_referral_rewards,
};
use self::payment_shared::NormalizedPaymentCallbackRequest;

const PAYMENT_CALLBACK_STORAGE_UNAVAILABLE_DETAIL: &str = "支付回调存储暂不可用";
const PAYMENT_CALLBACK_PROCESSING_FAILED_DETAIL: &str = "支付回调处理失败";

fn build_payment_callback_storage_unavailable_response() -> Response<Body> {
    // This is an intentional, stable public error.  Do not route it through
    // the generic 5xx sanitizer: payment providers use the detail to decide
    // whether a callback should be retried, and the message contains no
    // implementation detail.
    build_auth_json_response(
        http::StatusCode::SERVICE_UNAVAILABLE,
        serde_json::json!({
            "detail": PAYMENT_CALLBACK_STORAGE_UNAVAILABLE_DETAIL,
        }),
        None,
    )
}

fn build_payment_callback_processing_failed_response() -> Response<Body> {
    build_auth_json_response(
        http::StatusCode::INTERNAL_SERVER_ERROR,
        serde_json::json!({
            "detail": PAYMENT_CALLBACK_PROCESSING_FAILED_DETAIL,
        }),
        None,
    )
}

pub(super) async fn maybe_build_local_payment_callback_response(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
    request_body: Option<&axum::body::Bytes>,
) -> Option<Response<Body>> {
    payment_route::maybe_build_local_payment_callback_route_response(
        state,
        request_context,
        headers,
        request_body,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::{
        build_payment_callback_processing_failed_response,
        build_payment_callback_storage_unavailable_response,
        PAYMENT_CALLBACK_PROCESSING_FAILED_DETAIL, PAYMENT_CALLBACK_STORAGE_UNAVAILABLE_DETAIL,
    };
    use axum::body::to_bytes;
    use axum::http;
    use serde_json::json;

    #[tokio::test]
    async fn payment_callback_storage_unavailable_response_is_explicit_local_503() {
        let response = build_payment_callback_storage_unavailable_response();

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
    async fn payment_callback_processing_failure_is_a_stable_public_error() {
        let response = build_payment_callback_processing_failed_response();

        assert_eq!(response.status(), http::StatusCode::INTERNAL_SERVER_ERROR);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body should read");
        let payload: serde_json::Value =
            serde_json::from_slice(&body).expect("json body should parse");
        assert_eq!(
            payload,
            json!({ "detail": PAYMENT_CALLBACK_PROCESSING_FAILED_DETAIL })
        );
    }
}
