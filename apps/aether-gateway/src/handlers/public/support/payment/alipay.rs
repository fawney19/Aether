use axum::{body::Body, http, response::Response};

use super::{
    process_payment_callback_input_with_wallet_repository,
    reconcile_payment_callback_referral_rewards, AppState, GatewayPublicRequestContext,
};
use tracing::warn;

fn alipay_plain(status: http::StatusCode, body: &'static str) -> Response<Body> {
    Response::builder()
        .status(status)
        .header(http::header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Body::from(body))
        .expect("alipay plain response should build")
}

pub(super) async fn handle_alipay_notify(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    request_body: Option<&axum::body::Bytes>,
) -> Response<Body> {
    let Some(request_body) = request_body else {
        return alipay_plain(http::StatusCode::OK, "fail");
    };
    let input =
        match crate::handlers::shared::verify_alipay_notify_callback(state, request_body).await {
            Ok(value) => value,
            Err(_) => {
                warn!(
                    error_category = "callback_verification_failed",
                    "alipay notify verification failed"
                );
                return alipay_plain(http::StatusCode::OK, "fail");
            }
        };
    let outcome = process_payment_callback_input_with_wallet_repository(state, input).await;
    if let Ok(outcome) = &outcome {
        if !reconcile_payment_callback_referral_rewards(state, outcome, "alipay").await {
            return alipay_plain(http::StatusCode::OK, "fail");
        }
    }
    match outcome {
        Ok(aether_data::repository::wallet::ProcessPaymentCallbackOutcome::Applied { .. }) => {
            alipay_plain(http::StatusCode::OK, "success")
        }
        Ok(
            aether_data::repository::wallet::ProcessPaymentCallbackOutcome::AlreadyCredited {
                ..
            }
            | aether_data::repository::wallet::ProcessPaymentCallbackOutcome::DuplicateProcessed {
                ..
            },
        ) => alipay_plain(http::StatusCode::OK, "success"),
        Ok(aether_data::repository::wallet::ProcessPaymentCallbackOutcome::Failed { .. }) => {
            warn!(
                error_category = "callback_rejected",
                path = %request_context.request_path,
                "alipay notify processing failed"
            );
            alipay_plain(http::StatusCode::OK, "fail")
        }
        Err(response) => {
            warn!(
                status = %response.status(),
                path = %request_context.request_path,
                "alipay notify storage processing failed"
            );
            alipay_plain(http::StatusCode::OK, "fail")
        }
    }
}
