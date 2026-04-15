use axum::{body::Body, http, response::Response};

use super::payment_gateway::{PaymentGatewayRegistry, VerifyCallbackInput};
use super::payment_shared::{
    payment_callback_payment_method_from_path, payment_callback_secret, PaymentCallbackRequest,
    PAYMENT_CALLBACK_SIGNATURE_HEADER, PAYMENT_CALLBACK_TOKEN_HEADER,
};
use super::{
    build_auth_error_response, build_payment_callback_storage_unavailable_response,
    handle_payment_callback_with_postgres, AppState, GatewayPublicRequestContext,
};

pub(super) async fn maybe_build_local_payment_callback_route_response(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
    request_body: Option<&axum::body::Bytes>,
) -> Option<Response<Body>> {
    let decision = request_context.control_decision.as_ref()?;
    if decision.route_family.as_deref() != Some("payment_callback")
        || decision.route_kind.as_deref() != Some("callback")
    {
        return None;
    }

    let Some(secret) = payment_callback_secret() else {
        return Some(build_auth_error_response(
            http::StatusCode::SERVICE_UNAVAILABLE,
            "payment callback is disabled",
            false,
        ));
    };
    let Some(provided_token) =
        crate::headers::header_value_str(headers, PAYMENT_CALLBACK_TOKEN_HEADER)
    else {
        return Some(build_auth_error_response(
            http::StatusCode::UNAUTHORIZED,
            "invalid payment callback token",
            false,
        ));
    };
    if provided_token.trim() != secret {
        return Some(build_auth_error_response(
            http::StatusCode::UNAUTHORIZED,
            "invalid payment callback token",
            false,
        ));
    }
    let Some(signature) =
        crate::headers::header_value_str(headers, PAYMENT_CALLBACK_SIGNATURE_HEADER)
    else {
        return Some(build_auth_error_response(
            http::StatusCode::UNAUTHORIZED,
            "missing payment callback signature",
            false,
        ));
    };
    let Some(request_body) = request_body else {
        return Some(build_auth_error_response(
            http::StatusCode::BAD_REQUEST,
            "缺少请求体",
            false,
        ));
    };
    let raw_payload = match serde_json::from_slice::<PaymentCallbackRequest>(request_body) {
        Ok(value) => value,
        Err(_) => {
            return Some(build_auth_error_response(
                http::StatusCode::BAD_REQUEST,
                "输入验证失败",
                false,
            ));
        }
    };
    let Some(payment_method) =
        payment_callback_payment_method_from_path(&request_context.request_path)
    else {
        return Some(build_auth_error_response(
            http::StatusCode::BAD_REQUEST,
            "payment_method is required",
            false,
        ));
    };
    let Some(adapter) = PaymentGatewayRegistry::get(&payment_method) else {
        return Some(build_auth_error_response(
            http::StatusCode::BAD_REQUEST,
            "unsupported payment_method",
            false,
        ));
    };
    let verified = match adapter.verify_callback(VerifyCallbackInput {
        secret: &secret,
        signature: &signature,
        payload: raw_payload,
    }) {
        Ok(value) => value,
        Err(err) => {
            let status = if err == "输入验证失败" {
                http::StatusCode::BAD_REQUEST
            } else {
                http::StatusCode::INTERNAL_SERVER_ERROR
            };
            return Some(build_auth_error_response(status, err, false));
        }
    };

    if state.postgres_pool().is_some() {
        return Some(
            handle_payment_callback_with_postgres(
                state,
                &payment_method,
                request_context,
                &verified.normalized_payload,
                verified.signature_valid,
            )
            .await,
        );
    }

    #[cfg(test)]
    {
        return Some(
            super::payment_test_support::handle_payment_callback_with_test_store(
                &payment_method,
                request_context,
                &verified.normalized_payload,
                verified.signature_valid,
            )
            .await,
        );
    }

    #[cfg(not(test))]
    {
        Some(build_payment_callback_storage_unavailable_response())
    }
}
