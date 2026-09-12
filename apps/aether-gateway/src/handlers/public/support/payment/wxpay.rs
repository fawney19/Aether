use axum::{body::Body, http, response::Response};

use super::{
    process_payment_callback_input_with_wallet_repository,
    reconcile_payment_callback_referral_rewards, AppState, GatewayPublicRequestContext,
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
            Err(_) => {
                warn!(
                    error_category = "callback_verification_failed",
                    "wxpay notify verification failed"
                );
                return wxpay_json(http::StatusCode::BAD_REQUEST, "FAIL", "支付通知验证失败");
            }
        };
    let outcome = process_payment_callback_input_with_wallet_repository(state, input).await;
    if let Ok(outcome) = &outcome {
        if !reconcile_payment_callback_referral_rewards(state, outcome, "wxpay").await {
            return wxpay_json(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                "FAIL",
                "支付通知关联处理失败",
            );
        }
    }
    match outcome {
        Ok(aether_data::repository::wallet::ProcessPaymentCallbackOutcome::Applied { .. }) => {
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
        Ok(aether_data::repository::wallet::ProcessPaymentCallbackOutcome::Failed { .. }) => {
            warn!(
                error_category = "callback_rejected",
                path = %request_context.request_path,
                "wxpay notify processing failed"
            );
            wxpay_json(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                "FAIL",
                "支付通知处理失败",
            )
        }
        Err(response) => {
            let status = response.status();
            warn!(
                status = %status,
                path = %request_context.request_path,
                "wxpay notify storage processing failed"
            );
            wxpay_json(status, "FAIL", "支付通知处理失败")
        }
    }
}
