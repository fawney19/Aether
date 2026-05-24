use axum::{body::Body, http, response::Response};

use super::{
    process_payment_callback_input_with_wallet_repository, AppState, GatewayPublicRequestContext,
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
            Err(detail) => {
                warn!(error = %detail, "alipay notify verification failed");
                return alipay_plain(http::StatusCode::OK, "fail");
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
                    "failed to apply referral rewards for alipay callback"
                );
            }
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
        Ok(aether_data::repository::wallet::ProcessPaymentCallbackOutcome::Failed {
            error,
            ..
        }) => {
            warn!(error = %error, path = %request_context.request_path, "alipay notify processing failed");
            alipay_plain(http::StatusCode::OK, "fail")
        }
        Err(response) => response,
    }
}
