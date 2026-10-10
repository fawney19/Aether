use super::super::{build_router_with_state, start_server, AppState};
use crate::constants::{
    GATEWAY_HEADER, TRUSTED_ADMIN_SESSION_ID_HEADER, TRUSTED_ADMIN_USER_ID_HEADER,
    TRUSTED_ADMIN_USER_ROLE_HEADER,
};
use http::StatusCode;

#[tokio::test]
async fn admin_referral_reads_fail_explicitly_without_database() {
    let (url, handle) = start_server(build_router_with_state(AppState::new().unwrap())).await;
    for path in [
        "/api/admin/referrals/overview",
        "/api/admin/referral-rewards/reward-1",
        "/api/admin/referral-rewards?pending_reversal=true&limit=20&offset=20",
    ] {
        let response = reqwest::Client::new()
            .get(format!("{url}{path}"))
            .header(GATEWAY_HEADER, "rust-phase3b")
            .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-1")
            .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
            .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-1")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE, "{path}");
        let body: serde_json::Value = response.json().await.unwrap();
        assert!(body.get("stats").is_none());
        assert!(body["detail"].as_str().unwrap().contains("unavailable"));
    }
    handle.abort();
}

#[tokio::test]
async fn admin_referral_reads_reject_invalid_debt_filter() {
    let (url, handle) = start_server(build_router_with_state(AppState::new().unwrap())).await;
    let response = reqwest::Client::new()
        .get(format!(
            "{url}/api/admin/referral-rewards?pending_reversal=invalid"
        ))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-1")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-1")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    handle.abort();
}

#[tokio::test]
async fn admin_referral_reads_reject_non_admin_principal() {
    let (url, handle) = start_server(build_router_with_state(AppState::new().unwrap())).await;
    for path in [
        "/api/admin/referrals/overview",
        "/api/admin/referral-rewards/reward-1",
    ] {
        let response = reqwest::Client::new()
            .get(format!("{url}{path}"))
            .header(GATEWAY_HEADER, "rust-phase3b")
            .header(TRUSTED_ADMIN_USER_ID_HEADER, "user-1")
            .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "user")
            .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-1")
            .send()
            .await
            .unwrap();
        assert!(
            matches!(
                response.status(),
                StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
            ),
            "{path}: {}",
            response.status()
        );
    }
    handle.abort();
}
