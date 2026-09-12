use super::{
    any, build_router_with_state, build_test_auth_token, json, sample_auth_session,
    sample_auth_user, sample_auth_wallet, set_test_env_var, start_auth_gateway_with_state,
    start_server, AppState, Arc, Json, Mutex, Request, Router, StatusCode, Utc,
};
use axum::extract::ws::{Message as AxumWsMessage, WebSocketUpgrade};
use axum::response::IntoResponse;
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message as TungsteniteMessage;

#[derive(Debug, Clone, PartialEq)]
struct CapturedSidecarRequest {
    method: http::Method,
    path: String,
    authorization: Option<String>,
    client_ip: Option<String>,
    body: Option<serde_json::Value>,
}

#[test]
fn gateway_authenticates_and_proxies_vscodex_bff_routes() {
    std::thread::Builder::new()
        .name("vscodex-gateway-test".to_string())
        .stack_size(32 * 1024 * 1024)
        .spawn(|| {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .thread_stack_size(32 * 1024 * 1024)
                .build()
                .expect("test runtime should build")
                .block_on(run_vscodex_gateway_integration());
        })
        .expect("test thread should spawn")
        .join()
        .expect("test thread should complete");
}

async fn run_vscodex_gateway_integration() {
    let captured_requests = Arc::new(Mutex::new(Vec::<CapturedSidecarRequest>::new()));
    let captured_requests_for_handler = Arc::clone(&captured_requests);
    let captured_ws_handshake = Arc::new(Mutex::new(None::<(Option<String>, Option<String>)>));
    let captured_ws_handshake_for_handler = Arc::clone(&captured_ws_handshake);
    let sidecar = Router::new()
        .route(
            "/api/vscodex/ws",
            any(move |ws: WebSocketUpgrade, headers: http::HeaderMap| {
                let captured_ws_handshake = Arc::clone(&captured_ws_handshake_for_handler);
                async move {
                    let origin = headers
                        .get(http::header::ORIGIN)
                        .and_then(|value| value.to_str().ok())
                        .map(ToOwned::to_owned);
                    let authorization = headers
                        .get(http::header::AUTHORIZATION)
                        .and_then(|value| value.to_str().ok())
                        .map(ToOwned::to_owned);
                    *captured_ws_handshake
                        .lock()
                        .expect("WebSocket handshake store should lock") =
                        Some((origin, authorization));
                    ws.protocols(["vscodex.v1"])
                        .on_upgrade(|mut socket| async move {
                            while let Some(Ok(message)) = socket.next().await {
                                match message {
                                    AxumWsMessage::Text(text) => {
                                        let response = if text
                                            == r#"{"type":"auth","token":"test-auth-ok"}"# {
                                            r#"{"type":"auth.ok","role":"operator"}"#.to_string()
                                        } else {
                                            format!("echo:{text}")
                                        };
                                        if socket
                                            .send(AxumWsMessage::Text(response.into()))
                                            .await
                                            .is_err()
                                        {
                                            break;
                                        }
                                    }
                                    AxumWsMessage::Binary(bytes) => {
                                        if socket.send(AxumWsMessage::Binary(bytes)).await.is_err()
                                        {
                                            break;
                                        }
                                    }
                                    AxumWsMessage::Close(frame) => {
                                        let _ = socket.send(AxumWsMessage::Close(frame)).await;
                                        break;
                                    }
                                    AxumWsMessage::Ping(_) | AxumWsMessage::Pong(_) => {}
                                }
                            }
                        })
                }
            }),
        )
        .route(
            "/{*path}",
            any(move |request: Request| {
                let captured_requests = Arc::clone(&captured_requests_for_handler);
                async move {
                    let method = request.method().clone();
                    let path = request.uri().path().to_string();
                    let authorization = request
                        .headers()
                        .get(http::header::AUTHORIZATION)
                        .and_then(|value| value.to_str().ok())
                        .map(ToOwned::to_owned);
                    let client_ip = request
                        .headers()
                        .get("x-aether-client-ip")
                        .and_then(|value| value.to_str().ok())
                        .map(ToOwned::to_owned);
                    let body = axum::body::to_bytes(request.into_body(), 1024 * 1024)
                        .await
                        .expect("sidecar request body should be readable");
                    let body = (!body.is_empty()).then(|| {
                        serde_json::from_slice(&body).expect("sidecar request body should be JSON")
                    });
                    captured_requests
                        .lock()
                        .expect("captured request store should lock")
                        .push(CapturedSidecarRequest {
                            method: method.clone(),
                            path: path.clone(),
                            authorization,
                            client_ip,
                            body,
                        });

                    let (status, payload) = match (method, path.as_str()) {
                        (http::Method::GET, "/internal/v1/users/user-auth-1/devices") => (
                            StatusCode::OK,
                            json!({ "devices": [{ "id": "host-1", "name": "My Mac" }] }),
                        ),
                        (http::Method::POST, "/internal/v1/users/user-auth-1/pairings") => {
                            (StatusCode::CREATED, json!({ "code": "PAIR-123" }))
                        }
                        (http::Method::POST, "/internal/v1/users/user-auth-1/ws-tickets") => (
                            StatusCode::CREATED,
                            json!({
                                "ticket": "ticket-123",
                                "ws_url": "wss://aether.example/vscodex/ws"
                            }),
                        ),
                        (http::Method::DELETE, "/internal/v1/users/user-auth-1/devices/host-1") => {
                            (StatusCode::NO_CONTENT, json!({}))
                        }
                        (
                            http::Method::DELETE,
                            "/internal/v1/users/user-auth-1/devices/missing",
                        ) => (StatusCode::NOT_FOUND, json!({ "detail": "设备不存在" })),
                        (
                            http::Method::DELETE,
                            "/internal/v1/users/user-auth-1/devices/internal-denied",
                        ) => (
                            StatusCode::UNAUTHORIZED,
                            json!({ "detail": "internal token invalid" }),
                        ),
                        (
                            http::Method::DELETE,
                            "/internal/v1/users/user-auth-1/devices/redirect",
                        ) => (StatusCode::TEMPORARY_REDIRECT, json!({ "redirect": true })),
                        (
                            http::Method::DELETE,
                            "/internal/v1/users/user-auth-1/devices/empty-ok",
                        ) => return StatusCode::OK.into_response(),
                        (http::Method::POST, "/v1/pairings/exchange") => (
                            StatusCode::CREATED,
                            json!({ "device_id": "host-2", "device_token": "host-secret" }),
                        ),
                        _ => (
                            StatusCode::NOT_FOUND,
                            json!({ "detail": "unexpected path" }),
                        ),
                    };
                    let mut response = (status, Json(payload)).into_response();
                    if status == StatusCode::TEMPORARY_REDIRECT {
                        response.headers_mut().insert(
                            http::header::LOCATION,
                            "/redirect-must-not-be-followed".parse().unwrap(),
                        );
                    }
                    response
                }
            }),
        );
    let (sidecar_url, sidecar_handle) = start_server(sidecar).await;
    let _enabled = set_test_env_var("AETHER_VSCODEX_ENABLED", "true");
    let _internal_url = set_test_env_var("AETHER_VSCODEX_INTERNAL_URL", &sidecar_url);
    let _internal_token = set_test_env_var("AETHER_VSCODEX_INTERNAL_TOKEN", "sidecar-secret");

    let now = Utc::now();
    let user = sample_auth_user(now);
    let access_token = build_test_auth_token(
        "access",
        serde_json::Map::from_iter([
            ("user_id".to_string(), json!(user.id)),
            ("role".to_string(), json!(user.role)),
            (
                "created_at".to_string(),
                json!(user.created_at.map(|value| value.to_rfc3339())),
            ),
            ("session_id".to_string(), json!("session-vscodex")),
        ]),
        now + chrono::Duration::hours(1),
    );
    let (gateway_url, upstream_hits, gateway_handle, upstream_handle) =
        start_auth_gateway_with_state(
            user,
            sample_auth_wallet("user-auth-1", now),
            [sample_auth_session(
                "user-auth-1",
                "session-vscodex",
                "browser-device-vscodex",
                "refresh-vscodex",
                now,
            )],
        )
        .await;
    let client = reqwest::Client::new();

    let unauthenticated = client
        .get(format!("{gateway_url}/api/users/me/vscodex/devices"))
        .send()
        .await
        .expect("unauthenticated request should complete");
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);
    assert!(captured_requests
        .lock()
        .expect("captured request store should lock")
        .is_empty());

    let devices = client
        .get(format!("{gateway_url}/api/users/me/vscodex/devices"))
        .bearer_auth(&access_token)
        .header("x-client-device-id", "browser-device-vscodex")
        .send()
        .await
        .expect("devices request should complete");
    assert_eq!(devices.status(), StatusCode::OK);
    let devices_payload: serde_json::Value =
        devices.json().await.expect("devices body should be JSON");
    assert_eq!(devices_payload["devices"][0]["id"], "host-1");

    let pairing = client
        .post(format!("{gateway_url}/api/users/me/vscodex/pairings"))
        .bearer_auth(&access_token)
        .header("x-client-device-id", "browser-device-vscodex")
        .json(&json!({ "name": "My Mac", "user_id": "attacker" }))
        .send()
        .await
        .expect("pairing request should complete");
    assert_eq!(pairing.status(), StatusCode::CREATED);
    assert_eq!(
        pairing
            .headers()
            .get(http::header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok()),
        Some("no-store")
    );
    let pairing_payload: serde_json::Value =
        pairing.json().await.expect("pairing body should be JSON");
    assert_eq!(pairing_payload["code"], "PAIR-123");

    let ticket = client
        .post(format!("{gateway_url}/api/users/me/vscodex/ws-tickets"))
        .bearer_auth(&access_token)
        .header("x-client-device-id", "browser-device-vscodex")
        .json(&json!({ "device_id": "host-1", "user_id": "attacker" }))
        .send()
        .await
        .expect("ticket request should complete");
    assert_eq!(ticket.status(), StatusCode::CREATED);
    assert_eq!(
        ticket
            .headers()
            .get(http::header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok()),
        Some("no-store")
    );
    let ticket_payload: serde_json::Value =
        ticket.json().await.expect("ticket body should be JSON");
    assert_eq!(ticket_payload["ticket"], "ticket-123");
    assert_eq!(ticket_payload["ws_url"], "wss://aether.example/vscodex/ws");

    let deleted = client
        .delete(format!("{gateway_url}/api/users/me/vscodex/devices/host-1"))
        .bearer_auth(&access_token)
        .header("x-client-device-id", "browser-device-vscodex")
        .send()
        .await
        .expect("delete request should complete");
    assert_eq!(deleted.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        deleted
            .headers()
            .get(http::header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok()),
        Some("no-store")
    );

    let missing = client
        .delete(format!(
            "{gateway_url}/api/users/me/vscodex/devices/missing"
        ))
        .bearer_auth(&access_token)
        .header("x-client-device-id", "browser-device-vscodex")
        .send()
        .await
        .expect("missing device request should complete");
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    let missing_payload: serde_json::Value =
        missing.json().await.expect("missing body should be JSON");
    assert_eq!(missing_payload["detail"], "设备不存在");

    let internal_denied = client
        .delete(format!(
            "{gateway_url}/api/users/me/vscodex/devices/internal-denied"
        ))
        .bearer_auth(&access_token)
        .header("x-client-device-id", "browser-device-vscodex")
        .send()
        .await
        .expect("internal auth failure request should complete");
    assert_eq!(internal_denied.status(), StatusCode::BAD_GATEWAY);
    let internal_denied_payload: serde_json::Value = internal_denied
        .json()
        .await
        .expect("internal auth failure body should be JSON");
    assert_eq!(
        internal_denied_payload["detail"],
        "服务暂不可用，请稍后重试"
    );

    let redirected = client
        .delete(format!(
            "{gateway_url}/api/users/me/vscodex/devices/redirect"
        ))
        .bearer_auth(&access_token)
        .header("x-client-device-id", "browser-device-vscodex")
        .send()
        .await
        .expect("redirecting sidecar request should complete");
    assert_eq!(redirected.status(), StatusCode::BAD_GATEWAY);

    let empty_success = client
        .delete(format!(
            "{gateway_url}/api/users/me/vscodex/devices/empty-ok"
        ))
        .bearer_auth(&access_token)
        .header("x-client-device-id", "browser-device-vscodex")
        .send()
        .await
        .expect("empty sidecar success should complete");
    assert_eq!(empty_success.status(), StatusCode::BAD_GATEWAY);

    let pairing_exchange = client
        .post(format!("{gateway_url}/api/vscodex/pair"))
        .header("x-aether-client-ip", "203.0.113.99")
        .json(&json!({
            "code": "PAIR-123",
            "name": "Office Mac",
            "user_id": "attacker",
            "device_token": "stolen"
        }))
        .send()
        .await
        .expect("public pairing exchange should complete");
    assert_eq!(pairing_exchange.status(), StatusCode::CREATED);
    assert_eq!(
        pairing_exchange
            .headers()
            .get(http::header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok()),
        Some("no-store")
    );
    let pairing_exchange_payload: serde_json::Value = pairing_exchange
        .json()
        .await
        .expect("pairing exchange body should be JSON");
    assert_eq!(pairing_exchange_payload["device_id"], "host-2");
    assert_eq!(pairing_exchange_payload["device_token"], "host-secret");

    let mut websocket_request = format!("{gateway_url}/api/vscodex/ws")
        .replace("http://", "ws://")
        .into_client_request()
        .expect("WebSocket request should build");
    websocket_request.headers_mut().insert(
        http::header::ORIGIN,
        "https://aether.example".parse().unwrap(),
    );
    websocket_request.headers_mut().insert(
        http::header::AUTHORIZATION,
        "Bearer browser-aether-jwt".parse().unwrap(),
    );
    websocket_request.headers_mut().insert(
        http::header::SEC_WEBSOCKET_PROTOCOL,
        "vscodex.v1".parse().unwrap(),
    );
    let (mut websocket, websocket_response) = tokio_tungstenite::connect_async(websocket_request)
        .await
        .expect("gateway WebSocket should connect");
    assert_eq!(
        websocket_response
            .headers()
            .get(http::header::SEC_WEBSOCKET_PROTOCOL)
            .and_then(|value| value.to_str().ok()),
        Some("vscodex.v1")
    );
    websocket
        .send(TungsteniteMessage::Text(
            "{\"type\":\"auth\",\"ticket\":\"one-time-ticket\"}".into(),
        ))
        .await
        .expect("ticket frame should send");
    let echoed = websocket
        .next()
        .await
        .expect("echoed frame should arrive")
        .expect("echoed frame should be valid");
    assert_eq!(
        echoed,
        TungsteniteMessage::Text("echo:{\"type\":\"auth\",\"ticket\":\"one-time-ticket\"}".into())
    );
    websocket.close(None).await.expect("WebSocket should close");
    assert_eq!(
        captured_ws_handshake
            .lock()
            .expect("WebSocket handshake store should lock")
            .clone(),
        Some((Some("https://aether.example".to_string()), None))
    );

    let limited_gateway = build_router_with_state(
        AppState::new()
            .expect("limited gateway state should build")
            .with_request_concurrency_limit(1),
    );
    let (limited_gateway_url, limited_gateway_handle) = start_server(limited_gateway).await;
    let limited_ws_url =
        format!("{limited_gateway_url}/api/vscodex/ws").replace("http://", "ws://");
    let limited_ws_request = || {
        let mut request = limited_ws_url
            .as_str()
            .into_client_request()
            .expect("limited WebSocket request should build");
        request
            .headers_mut()
            .insert("x-real-ip", "198.51.100.50".parse().unwrap());
        request
    };
    let mut held_websockets = Vec::new();
    for index in 0..16 {
        let (websocket, _) = tokio_tungstenite::connect_async(limited_ws_request())
            .await
            .unwrap_or_else(|err| panic!("limited WebSocket {index} should connect: {err}"));
        held_websockets.push(websocket);
    }
    let per_ip_limit_error = tokio_tungstenite::connect_async(limited_ws_request())
        .await
        .expect_err("seventeenth WebSocket from one IP should be rejected");
    match per_ip_limit_error {
        tokio_tungstenite::tungstenite::Error::Http(response) => {
            assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
            assert_eq!(
                response
                    .headers()
                    .get(http::header::RETRY_AFTER)
                    .and_then(|value| value.to_str().ok()),
                Some("1")
            );
        }
        other => panic!("expected HTTP per-IP limit rejection, got {other:?}"),
    }

    held_websockets[0]
        .send(TungsteniteMessage::Text(
            r#"{"type":"auth","token":"test-auth-ok"}"#.into(),
        ))
        .await
        .expect("test authentication frame should send");
    let auth_ok =
        tokio::time::timeout(std::time::Duration::from_secs(1), held_websockets[0].next())
            .await
            .expect("test authentication response should arrive in time")
            .expect("test authentication response should contain a frame")
            .expect("test authentication response should be valid");
    assert_eq!(
        auth_ok,
        TungsteniteMessage::Text(r#"{"type":"auth.ok","role":"operator"}"#.into())
    );
    tokio::time::sleep(std::time::Duration::from_millis(25)).await;

    let (mut replacement_websocket, _) = tokio_tungstenite::connect_async(limited_ws_request())
        .await
        .expect("sidecar auth success should release one pending per-IP slot");
    replacement_websocket
        .close(None)
        .await
        .expect("replacement WebSocket should close");
    for mut websocket in held_websockets {
        websocket
            .close(None)
            .await
            .expect("held WebSocket should close");
    }
    limited_gateway_handle.abort();

    let blacklisted_gateway = build_router_with_state(
        AppState::new()
            .expect("blacklisted gateway state should build")
            .with_admin_security_blacklist_for_tests([(
                "127.0.0.1".to_string(),
                "blocked".to_string(),
            )]),
    );
    let (blacklisted_gateway_url, blacklisted_gateway_handle) =
        start_server(blacklisted_gateway).await;
    let blacklisted_ws_url =
        format!("{blacklisted_gateway_url}/api/vscodex/ws").replace("http://", "ws://");
    let blacklisted_error = tokio_tungstenite::connect_async(&blacklisted_ws_url)
        .await
        .expect_err("blacklisted WebSocket should be rejected");
    match blacklisted_error {
        tokio_tungstenite::tungstenite::Error::Http(response) => {
            assert_eq!(response.status(), StatusCode::FORBIDDEN)
        }
        other => panic!("expected HTTP blacklist rejection, got {other:?}"),
    }
    blacklisted_gateway_handle.abort();

    let requests = captured_requests
        .lock()
        .expect("captured request store should lock")
        .clone();
    assert_eq!(requests.len(), 9);
    assert!(requests
        .iter()
        .all(|request| request.authorization.as_deref() == Some("Bearer sidecar-secret")));
    assert!(requests[..8]
        .iter()
        .all(|request| request.path.starts_with("/internal/v1/users/user-auth-1/")));
    assert_eq!(requests[8].path, "/v1/pairings/exchange");
    assert_eq!(requests[1].body, Some(json!({ "name": "My Mac" })));
    assert_eq!(requests[2].body, Some(json!({ "device_id": "host-1" })));
    assert_eq!(
        requests[8].body,
        Some(json!({ "code": "PAIR-123", "name": "Office Mac" }))
    );
    assert_eq!(requests[8].client_ip.as_deref(), Some("127.0.0.1"));
    assert!(requests[..8]
        .iter()
        .all(|request| request.client_ip.is_none()));

    let _disabled = set_test_env_var("AETHER_VSCODEX_ENABLED", "false");
    let disabled = client
        .get(format!("{gateway_url}/api/users/me/vscodex/devices"))
        .bearer_auth(&access_token)
        .header("x-client-device-id", "browser-device-vscodex")
        .send()
        .await
        .expect("disabled feature request should complete");
    assert_eq!(disabled.status(), StatusCode::SERVICE_UNAVAILABLE);
    let disabled_payload: serde_json::Value =
        disabled.json().await.expect("disabled body should be JSON");
    assert_eq!(disabled_payload["detail"], "服务暂不可用，请稍后重试");
    assert_eq!(
        captured_requests
            .lock()
            .expect("captured request store should lock")
            .len(),
        9
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
    sidecar_handle.abort();
}
