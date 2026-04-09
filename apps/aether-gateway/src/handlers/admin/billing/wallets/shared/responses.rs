use super::requests::ADMIN_WALLETS_DATA_UNAVAILABLE_DETAIL;
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

pub(in super::super) fn build_admin_wallets_data_unavailable_response() -> Response<Body> {
    (
        http::StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({ "detail": ADMIN_WALLETS_DATA_UNAVAILABLE_DETAIL })),
    )
        .into_response()
}

pub(in super::super) fn build_admin_wallets_bad_request_response(
    detail: impl Into<String>,
) -> Response<Body> {
    (
        http::StatusCode::BAD_REQUEST,
        Json(json!({ "detail": detail.into() })),
    )
        .into_response()
}

pub(in super::super) fn build_admin_wallet_not_found_response() -> Response<Body> {
    (
        http::StatusCode::NOT_FOUND,
        Json(json!({ "detail": "Wallet not found" })),
    )
        .into_response()
}

pub(in super::super) fn build_admin_wallet_refund_not_found_response() -> Response<Body> {
    (
        http::StatusCode::NOT_FOUND,
        Json(json!({ "detail": "Refund request not found" })),
    )
        .into_response()
}
