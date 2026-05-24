use axum::{body::Body, http, response::Response};

use super::{
    process_payment_callback_input_with_wallet_repository, AppState, GatewayPublicRequestContext,
};
use serde_json::json;
use tracing::warn;

fn wxpay_json(
    status: http::StatusCode,
    code: &'static str,
    message: impl Into<String>,
) -> Response<Body> {
    Response::builder()
        .status(status)
        .header(
            http::header::CONTENT_TYPE,
            "application/json; charset=utf-8",
        )
        .body(Body::from(
            json!({ "code": code, "message": message.into() }).to_string(),
        ))
        .expect("wxpay json response should build")
}

pub(super) async fn handle_wxpay_notify(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
    request_body: Option<&axum::body::Bytes>,
) -> Response<Body> {
    let Some(request_body) = request_body else {
        return wxpay_json(http::StatusCode::BAD_REQUEST, "FAIL", "缺少请求体");
    };
    let input =
        match crate::handlers::shared::verify_wxpay_notify_callback(state, headers, request_body)
            .await
        {
            Ok(value) => value,
            Err(detail) => {
                warn!(error = %detail, "wxpay notify verification failed");
                return wxpay_json(http::StatusCode::BAD_REQUEST, "FAIL", detail);
            }
        };
    match process_payment_callback_input_with_wallet_repository(state, input).await {
        Ok(aether_data::repository::wallet::ProcessPaymentCallbackOutcome::Applied {
            order,
            order_id,
            ..
        }) => {
            if let Err(err) = state.apply_referral_rewards_for_paid_order(&order).await {
                warn!(
                    error = ?err,
                    order_id = %order_id,
                    "failed to apply referral rewards for wxpay callback"
                );
            }
            wxpay_json(http::StatusCode::OK, "SUCCESS", "成功")
        }
        Ok(
            aether_data::repository::wallet::ProcessPaymentCallbackOutcome::AlreadyCredited {
                ..
            }
            | aether_data::repository::wallet::ProcessPaymentCallbackOutcome::DuplicateProcessed {
                ..
            },
        ) => wxpay_json(http::StatusCode::OK, "SUCCESS", "成功"),
        Ok(aether_data::repository::wallet::ProcessPaymentCallbackOutcome::Failed {
            error,
            ..
        }) => {
            warn!(error = %error, path = %request_context.request_path, "wxpay notify processing failed");
            wxpay_json(http::StatusCode::INTERNAL_SERVER_ERROR, "FAIL", error)
        }
        Err(response) => response,
    }
}
