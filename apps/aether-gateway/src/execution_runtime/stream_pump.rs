use std::io::Error as IoError;

use aether_contracts::{
    ExecutionError, ExecutionErrorKind, ExecutionPhase, ExecutionTelemetry, StreamFrame,
    StreamFramePayload, StreamFrameType,
};
use async_stream::stream;
use axum::body::Bytes;
use base64::Engine as _;
use futures_util::{Stream, StreamExt};
use tracing::warn;

use crate::execution_runtime::ndjson::encode_stream_frame_ndjson;
use crate::execution_runtime::transport::DirectUpstreamResponse;
use crate::execution_runtime::DirectUpstreamStreamExecution;

pub(crate) fn build_direct_execution_frame_stream(
    execution: DirectUpstreamStreamExecution,
) -> impl Stream<Item = Result<Bytes, IoError>> + Send + 'static {
    stream! {
        let DirectUpstreamStreamExecution {
            request_id: _,
            candidate_id: _,
            status_code,
            headers,
            response,
            started_at,
        } = execution;

        match encode_headers_frame(status_code, headers) {
            Ok(frame) => yield Ok(frame),
            Err(err) => {
                yield Err(err);
                return;
            }
        }

        let mut upstream_bytes = 0u64;
        let mut ttfb_ms = None;
        let mut first_chunk_telemetry_emitted = false;
        match response {
            DirectUpstreamResponse::Reqwest(response) => {
                let mut bytes_stream = response.bytes_stream();
                while let Some(item) = bytes_stream.next().await {
                    match item {
                        Ok(chunk) => {
                            if ttfb_ms.is_none() {
                                ttfb_ms = Some(started_at.elapsed().as_millis() as u64);
                            }
                            if !first_chunk_telemetry_emitted {
                                match encode_telemetry_frame(ttfb_ms, ttfb_ms, upstream_bytes) {
                                    Ok(frame) => yield Ok(frame),
                                    Err(err) => {
                                        yield Err(err);
                                        return;
                                    }
                                }
                                first_chunk_telemetry_emitted = true;
                            }
                            upstream_bytes += chunk.len() as u64;
                            match encode_data_frame(&chunk) {
                                Ok(frame) => yield Ok(frame),
                                Err(err) => {
                                    yield Err(err);
                                    return;
                                }
                            }
                        }
                        Err(err) => {
                            let message = format_error_chain(&err);
                            warn!(
                                event_name = "stream_pump_body_read_error",
                                log_type = "ops",
                                status_code,
                                upstream_bytes,
                                error = %message,
                                "upstream body stream read error"
                            );
                            match encode_error_frame(status_code, message) {
                                Ok(frame) => yield Ok(frame),
                                Err(encode_err) => {
                                    yield Err(encode_err);
                                    return;
                                }
                            }
                            break;
                        }
                    }
                }
            }
            DirectUpstreamResponse::LocalTunnel(mut response) => loop {
                match response.next_chunk().await {
                    Ok(Some(chunk)) => {
                        if ttfb_ms.is_none() {
                            ttfb_ms = Some(started_at.elapsed().as_millis() as u64);
                        }
                        if !first_chunk_telemetry_emitted {
                            match encode_telemetry_frame(ttfb_ms, ttfb_ms, upstream_bytes) {
                                Ok(frame) => yield Ok(frame),
                                Err(err) => {
                                    yield Err(err);
                                    return;
                                }
                            }
                            first_chunk_telemetry_emitted = true;
                        }
                        upstream_bytes += chunk.len() as u64;
                        match encode_data_frame(&chunk) {
                            Ok(frame) => yield Ok(frame),
                            Err(err) => {
                                yield Err(err);
                                return;
                            }
                        }
                    }
                    Ok(None) => break,
                    Err(message) => {
                        warn!(
                            event_name = "stream_pump_body_read_error",
                            log_type = "ops",
                            status_code,
                            upstream_bytes,
                            error = %message,
                            "upstream body stream read error"
                        );
                        match encode_error_frame(status_code, message) {
                            Ok(frame) => yield Ok(frame),
                            Err(encode_err) => {
                                yield Err(encode_err);
                                return;
                            }
                        }
                        break;
                    }
                }
            }
        }

        match encode_telemetry_frame(
            ttfb_ms,
            Some(started_at.elapsed().as_millis() as u64),
            upstream_bytes,
        ) {
            Ok(frame) => yield Ok(frame),
            Err(err) => {
                yield Err(err);
                return;
            }
        }
        match encode_stream_frame_ndjson(&StreamFrame::eof()) {
            Ok(frame) => yield Ok(frame),
            Err(err) => yield Err(err),
        }
    }
}

fn encode_headers_frame(
    status_code: u16,
    headers: std::collections::BTreeMap<String, String>,
) -> Result<Bytes, IoError> {
    encode_stream_frame_ndjson(&StreamFrame {
        frame_type: StreamFrameType::Headers,
        payload: StreamFramePayload::Headers {
            status_code,
            headers,
        },
    })
}

fn encode_telemetry_frame(
    ttfb_ms: Option<u64>,
    elapsed_ms: Option<u64>,
    upstream_bytes: u64,
) -> Result<Bytes, IoError> {
    encode_stream_frame_ndjson(&StreamFrame {
        frame_type: StreamFrameType::Telemetry,
        payload: StreamFramePayload::Telemetry {
            telemetry: ExecutionTelemetry {
                ttfb_ms,
                elapsed_ms,
                upstream_bytes: Some(upstream_bytes),
            },
        },
    })
}

fn encode_data_frame(chunk: &Bytes) -> Result<Bytes, IoError> {
    encode_stream_frame_ndjson(&StreamFrame {
        frame_type: StreamFrameType::Data,
        payload: StreamFramePayload::Data {
            chunk_b64: Some(base64::engine::general_purpose::STANDARD.encode(chunk)),
            text: None,
        },
    })
}

fn encode_error_frame(status_code: u16, message: String) -> Result<Bytes, IoError> {
    encode_stream_frame_ndjson(&StreamFrame {
        frame_type: StreamFrameType::Error,
        payload: StreamFramePayload::Error {
            error: ExecutionError {
                kind: ExecutionErrorKind::Internal,
                phase: ExecutionPhase::StreamRead,
                message,
                upstream_status: Some(status_code),
                retryable: false,
                failover_recommended: false,
            },
        },
    })
}

fn format_error_chain(err: &(dyn std::error::Error + 'static)) -> String {
    let mut message = err.to_string();
    let mut source = err.source();
    while let Some(cause) = source {
        message.push_str(": ");
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    message
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::convert::Infallible;
    use std::sync::Arc;
    use std::time::Duration;

    use aether_contracts::{ExecutionPlan, ExecutionTimeouts, RequestBody};
    use async_stream::stream;
    use axum::body::{Body, Bytes};
    use axum::extract::ws::Message;
    use axum::routing::post;
    use axum::{http::header, http::HeaderValue, Router};
    use futures_util::StreamExt;
    use serde_json::Value;
    use tokio::sync::watch;

    use super::build_direct_execution_frame_stream;
    use crate::execution_runtime::transport::{
        execute_stream_plan_via_local_tunnel, DirectSyncExecutionRuntime, DirectUpstreamResponse,
    };
    use crate::tunnel::{tunnel_protocol, TunnelProxyConn};
    use crate::AppState;

    fn tunnel_proxy_snapshot(base_url: String) -> aether_contracts::ProxySnapshot {
        aether_contracts::ProxySnapshot {
            enabled: Some(true),
            mode: Some("tunnel".into()),
            node_id: Some("node-1".into()),
            label: Some("relay-node".into()),
            url: None,
            extra: Some(serde_json::json!({"tunnel_base_url": base_url})),
        }
    }

    #[tokio::test]
    async fn direct_execution_frame_stream_reports_ttfb_after_first_upstream_chunk() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("local addr should resolve");
        let app = Router::new().route(
            "/chat",
            post(|| async {
                let stream = stream! {
                    tokio::time::sleep(Duration::from_millis(25)).await;
                    yield Ok::<Bytes, Infallible>(Bytes::from_static(b"data: hello\n\n"));
                    yield Ok::<Bytes, Infallible>(Bytes::from_static(b"data: [DONE]\n\n"));
                };
                let mut response = axum::http::Response::new(Body::from_stream(stream));
                response.headers_mut().insert(
                    header::CONTENT_TYPE,
                    HeaderValue::from_static("text/event-stream"),
                );
                response
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("test server should run");
        });

        let execution = DirectSyncExecutionRuntime::new()
            .execute_stream(ExecutionPlan {
                request_id: "req-stream-ttfb-1".into(),
                candidate_id: Some("cand-stream-ttfb-1".into()),
                provider_name: Some("openai".into()),
                provider_id: "prov-1".into(),
                endpoint_id: "ep-1".into(),
                key_id: "key-1".into(),
                method: "POST".into(),
                url: format!("http://{addr}/chat"),
                headers: BTreeMap::from([("content-type".into(), "application/json".into())]),
                content_type: Some("application/json".into()),
                content_encoding: None,
                body: RequestBody::from_json(serde_json::json!({"stream": true})),
                stream: true,
                client_api_format: "openai:chat".into(),
                provider_api_format: "openai:chat".into(),
                model_name: Some("gpt-5".into()),
                proxy: None,
                tls_profile: None,
                timeouts: Some(ExecutionTimeouts {
                    connect_ms: Some(5_000),
                    total_ms: Some(5_000),
                    ..ExecutionTimeouts::default()
                }),
            })
            .await
            .expect("stream execution should succeed");

        let frame_output = build_direct_execution_frame_stream(execution)
            .map(|item| item.expect("frame should encode"))
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|bytes| String::from_utf8(bytes.to_vec()).expect("frame should be utf8"))
            .collect::<String>();

        server.abort();

        let telemetry_ttfb_ms = frame_output
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .find_map(|frame| {
                (frame.get("type").and_then(Value::as_str) == Some("telemetry")).then(|| {
                    frame
                        .get("payload")
                        .and_then(|payload| payload.get("telemetry"))
                        .and_then(|telemetry| telemetry.get("ttfb_ms"))
                        .and_then(Value::as_u64)
                })?
            });

        assert!(
            telemetry_ttfb_ms.is_some_and(|value| value > 0),
            "telemetry frame should include a measured ttfb"
        );
    }

    #[tokio::test]
    async fn direct_execution_frame_stream_emits_telemetry_before_first_data_frame() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("local addr should resolve");
        let server = tokio::spawn(async move {
            let app = Router::new().route(
                "/stream",
                post(|| async {
                    let body_stream = stream! {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                        yield Ok::<Bytes, Infallible>(Bytes::from_static(b"data: hello\n\n"));
                    };
                    (
                        [(
                            header::CONTENT_TYPE,
                            HeaderValue::from_static("text/event-stream"),
                        )],
                        Body::from_stream(body_stream),
                    )
                }),
            );
            axum::serve(listener, app)
                .await
                .expect("server should start");
        });

        let runtime = DirectSyncExecutionRuntime::new();
        let execution = runtime
            .execute_stream(ExecutionPlan {
                request_id: "req-telemetry-order".to_string(),
                candidate_id: Some("cand-telemetry-order".to_string()),
                provider_name: Some("OpenAI".to_string()),
                provider_id: "provider-1".to_string(),
                endpoint_id: "endpoint-1".to_string(),
                key_id: "key-1".to_string(),
                method: "POST".to_string(),
                url: format!("http://{addr}/stream"),
                headers: BTreeMap::new(),
                content_type: None,
                content_encoding: None,
                body: RequestBody {
                    json_body: None,
                    body_bytes_b64: None,
                    body_ref: None,
                },
                stream: true,
                client_api_format: "openai:chat".to_string(),
                provider_api_format: "openai:chat".to_string(),
                model_name: Some("gpt-5".into()),
                proxy: None,
                tls_profile: None,
                timeouts: Some(ExecutionTimeouts {
                    connect_ms: Some(5_000),
                    total_ms: Some(5_000),
                    ..ExecutionTimeouts::default()
                }),
            })
            .await
            .expect("stream execution should succeed");

        let frames = build_direct_execution_frame_stream(execution)
            .map(|item| item.expect("frame should encode"))
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|bytes| String::from_utf8(bytes.to_vec()).expect("frame should be utf8"))
            .collect::<Vec<_>>();

        server.abort();

        let frame_types = frames
            .iter()
            .map(|line| {
                serde_json::from_str::<Value>(line)
                    .expect("frame should parse")
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect::<Vec<_>>();

        let first_data_idx = frame_types
            .iter()
            .position(|kind| kind == "data")
            .expect("data frame should exist");
        let first_telemetry_idx = frame_types
            .iter()
            .position(|kind| kind == "telemetry")
            .expect("telemetry frame should exist");

        assert!(
            first_telemetry_idx < first_data_idx,
            "first telemetry frame should be emitted before the first data frame"
        );
    }

    #[tokio::test]
    async fn direct_execution_frame_stream_preserves_local_tunnel_stream_error_message() {
        let state = AppState::new().expect("app state should build");
        let tunnel_app = state.tunnel.app_state();
        let (proxy_tx, mut proxy_rx) = aether_runtime::bounded_queue(8);
        let (proxy_close_tx, _) = watch::channel(false);
        tunnel_app.hub.register_proxy(Arc::new(TunnelProxyConn::new(
            801,
            "node-1".to_string(),
            "Node 1".to_string(),
            proxy_tx,
            proxy_close_tx,
            16,
        )));

        let plan = ExecutionPlan {
            request_id: "req-local-stream-error-1".into(),
            candidate_id: Some("cand-local-stream-error-1".into()),
            provider_name: Some("openai".into()),
            provider_id: "prov-1".into(),
            endpoint_id: "ep-1".into(),
            key_id: "key-1".into(),
            method: "POST".into(),
            url: "https://example.com/chat".into(),
            headers: BTreeMap::from([("content-type".into(), "application/json".into())]),
            content_type: Some("application/json".into()),
            content_encoding: None,
            body: RequestBody::from_json(serde_json::json!({"stream": true})),
            stream: true,
            client_api_format: "openai:chat".into(),
            provider_api_format: "openai:chat".into(),
            model_name: Some("gpt-5".into()),
            proxy: Some(tunnel_proxy_snapshot("http://127.0.0.1:1".to_string())),
            tls_profile: None,
            timeouts: Some(ExecutionTimeouts {
                connect_ms: Some(5_000),
                total_ms: Some(5_000),
                ..ExecutionTimeouts::default()
            }),
        };

        let state_for_task = state.clone();
        let plan_for_task = plan.clone();
        let execution_task = tokio::spawn(async move {
            execute_stream_plan_via_local_tunnel(&state_for_task, &plan_for_task).await
        });

        let request_headers = match proxy_rx.recv().await.expect("headers frame should arrive") {
            Message::Binary(data) => data,
            other => panic!("unexpected message: {other:?}"),
        };
        let request_header = tunnel_protocol::FrameHeader::parse(&request_headers)
            .expect("request header frame should parse");
        assert_eq!(request_header.msg_type, tunnel_protocol::REQUEST_HEADERS);

        let request_body = match proxy_rx.recv().await.expect("body frame should arrive") {
            Message::Binary(data) => data,
            other => panic!("unexpected message: {other:?}"),
        };
        let request_body_header = tunnel_protocol::FrameHeader::parse(&request_body)
            .expect("request body frame should parse");
        assert_eq!(request_body_header.msg_type, tunnel_protocol::REQUEST_BODY);

        let response_meta = tunnel_protocol::ResponseMeta {
            status: 200,
            headers: vec![("content-type".to_string(), "text/event-stream".to_string())],
        };
        let response_payload =
            serde_json::to_vec(&response_meta).expect("response meta should serialize");
        let mut response_headers_frame = tunnel_protocol::encode_frame(
            request_header.stream_id,
            tunnel_protocol::RESPONSE_HEADERS,
            0,
            &response_payload,
        );
        tunnel_app
            .hub
            .handle_proxy_frame(801, &mut response_headers_frame)
            .await;

        let execution = execution_task
            .await
            .expect("execution task should complete")
            .expect("local tunnel execution should resolve")
            .expect("local tunnel execution should be available");

        let frame_task = tokio::spawn(async move {
            build_direct_execution_frame_stream(execution)
                .map(|item| item.expect("frame should encode"))
                .collect::<Vec<_>>()
                .await
                .into_iter()
                .map(|bytes| String::from_utf8(bytes.to_vec()).expect("frame should be utf8"))
                .collect::<Vec<_>>()
        });

        let mut response_body_frame = tunnel_protocol::encode_frame(
            request_header.stream_id,
            tunnel_protocol::RESPONSE_BODY,
            0,
            b"data: hello\n\n",
        );
        tunnel_app
            .hub
            .handle_proxy_frame(801, &mut response_body_frame)
            .await;

        let original_error = "proxy disconnected while forwarding upstream body";
        let mut response_error_frame =
            tunnel_protocol::encode_stream_error(request_header.stream_id, original_error);
        tunnel_app
            .hub
            .handle_proxy_frame(801, &mut response_error_frame)
            .await;

        let frames = frame_task.await.expect("frame task should complete");
        let parsed_frames = frames
            .iter()
            .map(|line| serde_json::from_str::<Value>(line).expect("frame should parse"))
            .collect::<Vec<_>>();

        assert!(
            parsed_frames
                .iter()
                .any(|frame| { frame.get("type").and_then(Value::as_str) == Some("data") }),
            "stream should contain at least one data frame before the error"
        );

        let error_message = parsed_frames
            .iter()
            .find(|frame| frame.get("type").and_then(Value::as_str) == Some("error"))
            .and_then(|frame| frame.get("payload"))
            .and_then(|payload| payload.get("error"))
            .and_then(|error| error.get("message"))
            .and_then(Value::as_str)
            .expect("error frame should include a message");

        assert_eq!(error_message, original_error);
        assert!(
            !error_message.contains("unexpected EOF during chunk size line"),
            "local tunnel path should preserve the original proxy error text"
        );
    }

    #[tokio::test]
    async fn second_local_tunnel_request_works_after_first_completes() {
        let state = AppState::new().expect("app state should build");
        let tunnel_app = state.tunnel.app_state();
        let (proxy_tx, mut proxy_rx) = aether_runtime::bounded_queue(8);
        let (proxy_close_tx, _) = watch::channel(false);
        tunnel_app.hub.register_proxy(Arc::new(TunnelProxyConn::new(
            900,
            "node-1".to_string(),
            "Node 1".to_string(),
            proxy_tx,
            proxy_close_tx,
            16,
        )));

        let plan = ExecutionPlan {
            request_id: "req-reuse-1".into(),
            candidate_id: Some("cand-reuse-1".into()),
            provider_name: Some("openai".into()),
            provider_id: "prov-1".into(),
            endpoint_id: "ep-1".into(),
            key_id: "key-1".into(),
            method: "POST".into(),
            url: "https://example.com/chat".into(),
            headers: BTreeMap::from([("content-type".into(), "application/json".into())]),
            content_type: Some("application/json".into()),
            content_encoding: None,
            body: RequestBody::from_json(serde_json::json!({"stream": true})),
            stream: true,
            client_api_format: "openai:chat".into(),
            provider_api_format: "openai:chat".into(),
            model_name: Some("gpt-5".into()),
            proxy: Some(tunnel_proxy_snapshot("http://127.0.0.1:1".to_string())),
            tls_profile: None,
            timeouts: Some(ExecutionTimeouts {
                connect_ms: Some(5_000),
                total_ms: Some(5_000),
                ..ExecutionTimeouts::default()
            }),
        };

        // --- First request ---
        let state1 = state.clone();
        let plan1 = plan.clone();
        let exec1 =
            tokio::spawn(
                async move { execute_stream_plan_via_local_tunnel(&state1, &plan1).await },
            );

        // Read request frames from proxy side
        let req1_headers = match proxy_rx.recv().await.expect("req1 headers") {
            Message::Binary(data) => data,
            other => panic!("unexpected: {other:?}"),
        };
        let req1_header =
            tunnel_protocol::FrameHeader::parse(&req1_headers).expect("req1 header parse");
        let _req1_body = proxy_rx.recv().await.expect("req1 body");

        // Simulate proxy response
        let resp_meta = serde_json::to_vec(&tunnel_protocol::ResponseMeta {
            status: 200,
            headers: vec![("content-type".to_string(), "text/event-stream".to_string())],
        })
        .unwrap();
        let mut resp_headers = tunnel_protocol::encode_frame(
            req1_header.stream_id,
            tunnel_protocol::RESPONSE_HEADERS,
            0,
            &resp_meta,
        );
        tunnel_app
            .hub
            .handle_proxy_frame(900, &mut resp_headers)
            .await;

        let execution1 = exec1
            .await
            .expect("task")
            .expect("transport")
            .expect("execution");

        // Consume the body stream fully
        let mut resp1 = match execution1.response {
            DirectUpstreamResponse::LocalTunnel(r) => r,
            _ => panic!("expected local tunnel response"),
        };

        // Send body + STREAM_END
        let mut body_frame = tunnel_protocol::encode_frame(
            req1_header.stream_id,
            tunnel_protocol::RESPONSE_BODY,
            0,
            b"data: hello\n\n",
        );
        tunnel_app
            .hub
            .handle_proxy_frame(900, &mut body_frame)
            .await;
        let mut end_frame = tunnel_protocol::encode_frame(
            req1_header.stream_id,
            tunnel_protocol::STREAM_END,
            0,
            &[],
        );
        tunnel_app.hub.handle_proxy_frame(900, &mut end_frame).await;

        // Drain the body
        while let Ok(Some(_)) = resp1.next_chunk().await {}
        drop(resp1);

        // --- Second request ---
        let state2 = state.clone();
        let plan2 = ExecutionPlan {
            request_id: "req-reuse-2".into(),
            candidate_id: Some("cand-reuse-2".into()),
            ..plan.clone()
        };
        let exec2 =
            tokio::spawn(
                async move { execute_stream_plan_via_local_tunnel(&state2, &plan2).await },
            );

        // Read second request's frames
        let req2_headers = tokio::time::timeout(Duration::from_secs(2), proxy_rx.recv())
            .await
            .expect("second request should arrive within 2s")
            .expect("req2 headers");
        let req2_data = match req2_headers {
            Message::Binary(data) => data,
            other => panic!("unexpected: {other:?}"),
        };
        let req2_header =
            tunnel_protocol::FrameHeader::parse(&req2_data).expect("req2 header parse");
        assert_eq!(req2_header.msg_type, tunnel_protocol::REQUEST_HEADERS);

        // Simulate proxy response for second request
        let mut resp2_headers = tunnel_protocol::encode_frame(
            req2_header.stream_id,
            tunnel_protocol::RESPONSE_HEADERS,
            0,
            &resp_meta,
        );
        tunnel_app
            .hub
            .handle_proxy_frame(900, &mut resp2_headers)
            .await;

        let execution2 = exec2
            .await
            .expect("task")
            .expect("transport")
            .expect("second execution should succeed");
        assert_eq!(execution2.status_code, 200);

        // Clean up
        let mut end2 = tunnel_protocol::encode_frame(
            req2_header.stream_id,
            tunnel_protocol::STREAM_END,
            0,
            &[],
        );
        tunnel_app.hub.handle_proxy_frame(900, &mut end2).await;
    }
}
