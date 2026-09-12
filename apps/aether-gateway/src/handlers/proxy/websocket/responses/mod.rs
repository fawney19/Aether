//! OpenAI Responses WebSocket protocol entry point, session engine, and adapters.
//!
//! The route is protocol-oriented. `session` bootstraps the authenticated
//! connection, `connection` owns the socket FSM, `client` and `quota` own
//! protocol/retry policy, and `lifecycle`/`turn` bridge each turn into the
//! existing usage and audit runtime. Adapters contain only provider-specific
//! connection and metadata behavior.

mod adapter;
mod adapters;
mod admission;
mod binding;
mod client;
mod connection;
mod continuation;
mod control;
mod frame;
mod lifecycle;
mod observation;
mod ownership;
mod plan_admission;
mod quota;
mod redaction;
mod relay_policy;
mod request;
mod session;
mod settlement;
mod state;
mod turn;
mod turn_state;
mod upstream;

pub(crate) use admission::ResponsesWebSocketTurnAdmission;

use std::net::SocketAddr;

use axum::body::Body;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, Response, Uri};

use crate::handlers::proxy::websocket::ingress::{
    upgrade_authenticated_ai_websocket, WebSocketIngressSpec,
};
use crate::handlers::proxy::websocket::session::RESPONSES_WEBSOCKET_SESSION_LIMITS;
use crate::{AppState, GatewayError};

pub(crate) async fn responses_websocket(
    State(state): State<AppState>,
    ConnectInfo(remote_addr): ConnectInfo<SocketAddr>,
    ws: WebSocketUpgrade,
    headers: HeaderMap,
    uri: Uri,
) -> Result<Response<Body>, GatewayError> {
    upgrade_authenticated_ai_websocket(
        state,
        remote_addr,
        ws,
        headers,
        uri,
        RESPONSES_WEBSOCKET_SESSION_LIMITS,
        RESPONSES_WEBSOCKET_INGRESS_SPEC,
        session::run_responses_websocket,
    )
    .await
}

const RESPONSES_WEBSOCKET_INGRESS_SPEC: WebSocketIngressSpec = WebSocketIngressSpec {
    route_unavailable_message: "WebSocket route is unavailable",
};
