use axum::{body::Body, http, response::Response};

pub(super) use super::{build_auth_error_response, AppState, GatewayPublicRequestContext};

#[path = "payment/gateway.rs"]
pub(super) mod payment_gateway;
#[path = "payment/postgres.rs"]
mod payment_postgres;
#[path = "payment/route.rs"]
mod payment_route;
#[path = "payment/shared.rs"]
mod payment_shared;
#[cfg(test)]
#[path = "payment/test_support.rs"]
mod payment_test_support;

use self::payment_postgres::handle_payment_callback_with_postgres;
use self::payment_shared::NormalizedPaymentCallbackRequest;

const PAYMENT_CALLBACK_STORAGE_UNAVAILABLE_DETAIL: &str = "支付回调存储暂不可用";

fn build_payment_callback_storage_unavailable_response() -> Response<Body> {
    build_auth_error_response(
        http::StatusCode::SERVICE_UNAVAILABLE,
        PAYMENT_CALLBACK_STORAGE_UNAVAILABLE_DETAIL,
        false,
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
        build_payment_callback_storage_unavailable_response,
        PAYMENT_CALLBACK_STORAGE_UNAVAILABLE_DETAIL,
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
}
