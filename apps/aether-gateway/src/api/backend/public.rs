use axum::routing::get;
use axum::Router;

use crate::{handlers::proxy::proxy_request, state::AppState};

pub(crate) fn mount_public_support_routes(router: Router<AppState>) -> Router<AppState> {
    router
        .route("/v1/models", get(proxy_request))
        .route("/v1beta/models", get(proxy_request))
        .route("/v1/health", get(proxy_request))
        .route("/health", get(proxy_request))
        .route("/v1/providers", get(proxy_request))
        .route("/v1/providers/{*provider_path}", get(proxy_request))
        .route("/v1/test-connection", get(proxy_request))
        .route("/test-connection", get(proxy_request))
        .route("/api/public/site-info", get(proxy_request))
        .route("/api/public/providers", get(proxy_request))
        .route("/api/public/models", get(proxy_request))
        .route("/api/public/search/models", get(proxy_request))
        .route("/api/public/stats", get(proxy_request))
        .route("/api/public/global-models", get(proxy_request))
        .route("/api/public/health/api-formats", get(proxy_request))
        .route("/api/public/health/models", get(proxy_request))
        .route("/api/modules/auth-status", get(proxy_request))
        .route("/api/capabilities", get(proxy_request))
        .route("/api/capabilities/user-configurable", get(proxy_request))
        .route("/api/capabilities/model/{*model_path}", get(proxy_request))
        .route("/install/{*install_path}", get(proxy_request))
        .route("/install-tunnel/{*install_path}", get(proxy_request))
        .route("/i/{*install_path}", get(proxy_request))
        .route("/", get(proxy_request))
}
