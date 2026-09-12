use std::sync::Arc;
use std::time::Duration;

use axum::{body::Body, routing::get, Router};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use super::super::{serve_gateway_router, GatewayHttpLimits, HttpConnectionBudget};

async fn start(
    router: Router,
) -> (
    std::net::SocketAddr,
    CancellationToken,
    Arc<HttpConnectionBudget>,
    tokio::task::JoinHandle<Result<(), String>>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let shutdown = CancellationToken::new();
    let budget = Arc::new(HttpConnectionBudget::new(16));
    let stop = shutdown.clone();
    let shared = Arc::clone(&budget);
    let server = tokio::spawn(async move {
        serve_gateway_router(
            vec![listener],
            router,
            shared,
            GatewayHttpLimits {
                http2_max_concurrent_streams: 16,
                http_header_read_timeout_ms: 10_000,
                http_header_max_bytes: 32_768,
                http_max_headers: 100,
            },
            stop,
        )
        .await
        .map_err(|error| error.to_string())
    });
    (address, shutdown, budget, server)
}

async fn within<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(3), future)
        .await
        .expect("shutdown deadline")
}

#[tokio::test]
async fn gateway_shutdown_drains_in_flight_http1_and_http2_responses() {
    for http2 in [false, true] {
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let handler_started = Arc::clone(&started);
        let handler_release = Arc::clone(&release);
        let router = Router::new().route(
            "/",
            get(move || {
                let started = Arc::clone(&handler_started);
                let release = Arc::clone(&handler_release);
                async move {
                    started.notify_one();
                    release.notified().await;
                    "complete response"
                }
            }),
        );
        let (address, shutdown, budget, server) = start(router).await;
        let client = if http2 {
            reqwest::Client::builder().http2_prior_knowledge()
        } else {
            reqwest::Client::builder().http1_only()
        }
        .build()
        .unwrap();
        let request = tokio::spawn(async move {
            client
                .get(format!("http://{address}/"))
                .send()
                .await
                .unwrap()
                .text()
                .await
                .unwrap()
        });
        within(started.notified()).await;
        shutdown.cancel();
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!server.is_finished());
        assert!(!request.is_finished());
        assert!(TcpStream::connect(address).await.is_err());
        release.notify_one();
        assert_eq!(within(request).await.unwrap(), "complete response");
        within(server).await.unwrap().unwrap();
        assert_eq!(budget.snapshot().in_flight, 0);
    }
}

#[tokio::test]
async fn gateway_shutdown_closes_idle_protocol_detection_connections() {
    let (address, shutdown, budget, server) = start(Router::new()).await;
    let mut peer = TcpStream::connect(address).await.unwrap();
    within(async {
        while budget.snapshot().in_flight == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await;
    shutdown.cancel();
    within(server).await.unwrap().unwrap();
    assert_eq!(within(peer.read(&mut [0_u8; 1])).await.unwrap(), 0);
    assert_eq!(budget.snapshot().in_flight, 0);
}

#[tokio::test]
async fn gateway_shutdown_force_cancels_a_handler_without_socket_io() {
    struct HandlerDrop(Arc<Notify>);
    impl Drop for HandlerDrop {
        fn drop(&mut self) {
            self.0.notify_one();
        }
    }
    for http2 in [false, true] {
        let started = Arc::new(Notify::new());
        let dropped = Arc::new(Notify::new());
        let request_started = Arc::clone(&started);
        let request_dropped = Arc::clone(&dropped);
        let router = Router::new().route(
            "/",
            get(move || {
                let started = Arc::clone(&request_started);
                let dropped = Arc::clone(&request_dropped);
                async move {
                    let _guard = HandlerDrop(dropped);
                    started.notify_one();
                    std::future::pending::<&'static str>().await
                }
            }),
        );
        let (address, shutdown, budget, server) = start(router).await;
        let client = if http2 {
            reqwest::Client::builder().http2_prior_knowledge()
        } else {
            reqwest::Client::builder().http1_only()
        }
        .build()
        .unwrap();
        let request =
            tokio::spawn(async move { client.get(format!("http://{address}/")).send().await });
        within(started.notified()).await;
        shutdown.cancel();
        budget.force_close();
        within(server).await.unwrap().unwrap();
        within(dropped.notified()).await;
        assert!(within(request).await.unwrap().is_err());
        assert_eq!(budget.snapshot().in_flight, 0);
    }
}

#[tokio::test]
async fn gateway_shutdown_force_closes_upgraded_io_and_waits_for_release() {
    let upgraded_done = Arc::new(Notify::new());
    let done = Arc::clone(&upgraded_done);
    let router = Router::new().route(
        "/",
        get(move |mut request: axum::extract::Request| {
            let done = Arc::clone(&done);
            async move {
                let upgrade = hyper::upgrade::on(&mut request);
                tokio::spawn(async move {
                    let upgraded = upgrade.await.unwrap();
                    let mut io = hyper_util::rt::TokioIo::new(upgraded);
                    let error = io.read_u8().await.unwrap_err();
                    assert_eq!(error.kind(), std::io::ErrorKind::ConnectionAborted);
                    drop(io);
                    done.notify_one();
                });
                axum::http::Response::builder()
                    .status(101)
                    .header("connection", "upgrade")
                    .header("upgrade", "echo")
                    .body(Body::empty())
                    .unwrap()
            }
        }),
    );
    let (address, shutdown, budget, server) = start(router).await;
    let mut peer = TcpStream::connect(address).await.unwrap();
    peer.write_all(
        b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: upgrade\r\nUpgrade: echo\r\n\r\n",
    )
    .await
    .unwrap();
    let mut header = Vec::new();
    within(async {
        while !header.ends_with(b"\r\n\r\n") {
            header.push(peer.read_u8().await.unwrap());
        }
    })
    .await;
    assert!(header.starts_with(b"HTTP/1.1 101"));
    shutdown.cancel();
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(!server.is_finished());
    budget.force_close();
    within(upgraded_done.notified()).await;
    within(server).await.unwrap().unwrap();
    assert_eq!(budget.snapshot().in_flight, 0);
}
