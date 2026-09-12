use std::collections::VecDeque;
use std::convert::Infallible;
use std::sync::atomic::AtomicBool;

use bytes::Bytes;
use http::{Request, Response, StatusCode};
use http_body_util::{BodyExt, Empty};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::*;

async fn within<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(3), future)
        .await
        .expect("connection test exceeded its deadline")
}

async fn tcp_pair() -> (TcpStream, TcpStream) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = TcpStream::connect(listener.local_addr().unwrap())
        .await
        .unwrap();
    let (server, _) = listener.accept().await.unwrap();
    (client, server)
}

#[tokio::test]
async fn forced_shutdown_wakes_independent_read_and_write_waiters() {
    let budget = Arc::new(HttpConnectionBudget::new(2));
    let (io, _peer) = tokio::io::duplex(1);
    let io = budget.try_admit(io).unwrap();
    let (mut reader, mut writer) = tokio::io::split(io);
    writer.write_all(b"a").await.unwrap();
    let reading = tokio::spawn(async move { reader.read_u8().await });
    let writing = tokio::spawn(async move { writer.write_all(b"b").await });
    tokio::task::yield_now().await;
    budget.force_close();
    assert_eq!(
        within(reading).await.unwrap().unwrap_err().kind(),
        io::ErrorKind::ConnectionAborted
    );
    assert_eq!(
        within(writing).await.unwrap().unwrap_err().kind(),
        io::ErrorKind::ConnectionAborted
    );
    assert_eq!(budget.snapshot().in_flight, 0);
    assert!(budget.try_admit(tokio::io::empty()).is_err());
}

#[tokio::test]
async fn forced_shutdown_before_first_poll_closes_read_write_and_flush() {
    let budget = Arc::new(HttpConnectionBudget::new(1));
    let (io, _peer) = tokio::io::duplex(16);
    let mut io = budget.try_admit(io).unwrap();
    budget.force_close();
    assert_eq!(
        io.read_u8().await.unwrap_err().kind(),
        io::ErrorKind::ConnectionAborted
    );
    assert_eq!(
        io.write_all(b"a").await.unwrap_err().kind(),
        io::ErrorKind::ConnectionAborted
    );
    assert_eq!(
        io.flush().await.unwrap_err().kind(),
        io::ErrorKind::ConnectionAborted
    );
    let buffers = [io::IoSlice::new(b"a")];
    assert_eq!(
        io.write_vectored(&buffers).await.unwrap_err().kind(),
        io::ErrorKind::ConnectionAborted
    );
    io.shutdown().await.unwrap();
    drop(io);
    assert_eq!(budget.snapshot().in_flight, 0);
}

#[test]
fn http_connection_limits_apply_to_auto_explicit_zero_and_fd_bounds() {
    for (configured, requests, websockets, fd_limit, expected) in [
        (None, 0, 0, None, 1),
        (None, 3, 5, None, 8),
        (Some(0), 3, 5, None, 8),
        (Some(9), 3, 5, None, 9),
        (Some(usize::MAX), 0, 0, None, MAX_HTTP_CONNECTIONS),
        (None, usize::MAX, usize::MAX, None, MAX_HTTP_CONNECTIONS),
        (None, 512, 512, Some(1_024), 384),
        (Some(500), 3, 5, Some(1_024), 384),
        (Some(32), 3, 5, Some(1_024), 32),
        (Some(500), 3, 5, Some(260), 2),
        (Some(500), 3, 5, Some(258), 1),
        (Some(500), 3, 5, Some(0), 1),
    ] {
        assert_eq!(
            http_connection_limit(configured, requests, websockets, fd_limit),
            expected,
        );
    }
    assert_eq!(HttpConnectionBudget::new(0).snapshot().limit, 1);
    assert_eq!(
        HttpConnectionBudget::new(usize::MAX).snapshot().limit,
        MAX_HTTP_CONNECTIONS,
    );
}

#[test]
fn http_connection_budget_drops_io_before_returning_capacity() {
    struct DropProbe {
        budget: Arc<HttpConnectionBudget>,
        observed: Arc<AtomicBool>,
    }
    impl Drop for DropProbe {
        fn drop(&mut self) {
            self.observed
                .store(self.budget.snapshot().in_flight == 1, Ordering::Relaxed);
        }
    }
    let budget = Arc::new(HttpConnectionBudget::new(1));
    let admitted_dropped = Arc::new(AtomicBool::new(false));
    let rejected_dropped = Arc::new(AtomicBool::new(false));
    let admitted = budget
        .try_admit(DropProbe {
            budget: Arc::clone(&budget),
            observed: Arc::clone(&admitted_dropped),
        })
        .unwrap();
    assert!(budget
        .try_admit(DropProbe {
            budget: Arc::clone(&budget),
            observed: Arc::clone(&rejected_dropped),
        })
        .is_err());
    assert!(rejected_dropped.load(Ordering::Relaxed));
    assert_eq!(budget.snapshot().in_flight, 1);
    drop(admitted);
    assert!(admitted_dropped.load(Ordering::Relaxed));
    assert_eq!(budget.snapshot().in_flight, 0);
    assert_eq!(budget.snapshot().high_watermark, 1);
    assert_eq!(budget.snapshot().rejected_total, 1);
    drop(budget.try_admit(()).unwrap());
    assert_eq!(budget.snapshot().in_flight, 0);
}

#[tokio::test]
async fn http_connection_budget_forwards_read_write_vectored_and_half_close() {
    let budget = Arc::new(HttpConnectionBudget::new(1));
    let (mut peer, io) = tokio::io::duplex(32);
    let vectored = io.is_write_vectored();
    let mut admitted = budget.try_admit(io).unwrap();
    assert_eq!(admitted.is_write_vectored(), vectored);
    let written = admitted
        .write_vectored(&[io::IoSlice::new(b"ab"), io::IoSlice::new(b"cd")])
        .await
        .unwrap();
    assert!((1..=4).contains(&written));
    admitted.write_all(&b"abcd"[written..]).await.unwrap();
    admitted.flush().await.unwrap();
    let mut message = [0; 4];
    peer.read_exact(&mut message).await.unwrap();
    assert_eq!(&message, b"abcd");

    admitted.shutdown().await.unwrap();
    assert_eq!(peer.read(&mut [0; 1]).await.unwrap(), 0);
    assert_eq!(budget.snapshot().in_flight, 1);
    peer.write_all(b"reply").await.unwrap();
    let mut reply = [0; 5];
    admitted.read_exact(&mut reply).await.unwrap();
    assert_eq!(&reply, b"reply");
    drop(admitted);
    assert_eq!(budget.snapshot().in_flight, 0);
}

#[tokio::test]
async fn http_connection_budget_two_tcp_listeners_share_capacity_and_recover() {
    let first_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let second_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let budget = Arc::new(HttpConnectionBudget::new(1));
    let first_client = TcpStream::connect(first_listener.local_addr().unwrap())
        .await
        .unwrap();
    let (first_io, _) = within(budget.accept(&first_listener)).await;
    let first = budget.try_admit(first_io).unwrap();

    let mut rejected_client = TcpStream::connect(second_listener.local_addr().unwrap())
        .await
        .unwrap();
    let (second_io, _) = within(budget.accept(&second_listener)).await;
    assert!(Arc::clone(&budget).try_admit(second_io).is_err());
    let rejected_read = within(rejected_client.read(&mut [0; 1])).await;
    assert!(
        matches!(rejected_read, Ok(0))
            || matches!(rejected_read, Err(ref error) if matches!(
                error.kind(), io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionAborted
            ))
    );
    assert_eq!(budget.snapshot().in_flight, 1);
    assert_eq!(budget.snapshot().rejected_total, 1);

    drop((first, first_client, rejected_client));
    let replacement_client = TcpStream::connect(second_listener.local_addr().unwrap())
        .await
        .unwrap();
    let (replacement_io, _) = within(budget.accept(&second_listener)).await;
    let replacement = budget.try_admit(replacement_io).unwrap();
    assert_eq!(budget.snapshot().in_flight, 1);
    assert_eq!(budget.snapshot().high_watermark, 1);
    drop((replacement, replacement_client));
    assert_eq!(budget.snapshot().in_flight, 0);
}

#[tokio::test]
async fn http_connection_budget_task_cancelled_before_first_poll_returns_permit() {
    let (client, server) = tcp_pair().await;
    let budget = Arc::new(HttpConnectionBudget::new(1));
    let admitted = budget.try_admit(server).unwrap();
    let polled = Arc::new(AtomicBool::new(false));
    let polled_by_task = Arc::clone(&polled);
    let task = tokio::spawn(async move {
        let _io = admitted;
        polled_by_task.store(true, Ordering::Relaxed);
        std::future::pending::<()>().await;
    });
    task.abort();
    assert!(within(task).await.unwrap_err().is_cancelled());
    assert!(!polled.load(Ordering::Relaxed));
    assert_eq!(budget.snapshot().in_flight, 0);
    drop(client);
}

#[tokio::test]
async fn http_connection_budget_header_timeout_and_parse_failure_return_permit() {
    for malformed in [false, true] {
        let (mut client, server_io) = tcp_pair().await;
        let budget = Arc::new(HttpConnectionBudget::new(1));
        let admitted = budget.try_admit(server_io).unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let requests_seen = Arc::clone(&requests);
        let server = tokio::spawn(async move {
            let service = service_fn(move |_: Request<Incoming>| {
                requests_seen.fetch_add(1, Ordering::Relaxed);
                async { Ok::<_, Infallible>(Response::new(Empty::<Bytes>::new())) }
            });
            hyper::server::conn::http1::Builder::new()
                .timer(TokioTimer::new())
                .header_read_timeout(Duration::from_millis(20))
                .serve_connection(TokioIo::new(admitted), service)
                .await
        });
        if malformed {
            client.write_all(b"invalid request\r\n\r\n").await.unwrap();
        }
        let _connection_result = within(server).await.unwrap();
        assert_eq!(requests.load(Ordering::Relaxed), 0);
        assert_eq!(budget.snapshot().in_flight, 0);
        drop(client);
    }
}

#[tokio::test]
async fn http_connection_budget_h1_upgrade_keeps_permit_after_connection_future_finishes() {
    let (mut client, server_io) = tcp_pair().await;
    let budget = Arc::new(HttpConnectionBudget::new(1));
    let admitted = budget.try_admit(server_io).unwrap();
    let (upgrade_finished_tx, upgrade_finished_rx) = tokio::sync::oneshot::channel();
    let upgrade_finished_tx = Arc::new(std::sync::Mutex::new(Some(upgrade_finished_tx)));
    let server = tokio::spawn(async move {
        let service = service_fn(move |mut request: Request<Incoming>| {
            let on_upgrade = hyper::upgrade::on(&mut request);
            let finished = upgrade_finished_tx.lock().unwrap().take().unwrap();
            tokio::spawn(async move {
                let mut upgraded = TokioIo::new(on_upgrade.await.unwrap());
                let mut message = [0; 4];
                upgraded.read_exact(&mut message).await.unwrap();
                upgraded.write_all(&message).await.unwrap();
                assert_eq!(upgraded.read(&mut [0; 1]).await.unwrap(), 0);
                drop(upgraded);
                let _ = finished.send(());
            });
            async {
                Ok::<_, Infallible>(
                    Response::builder()
                        .status(StatusCode::SWITCHING_PROTOCOLS)
                        .header("connection", "upgrade")
                        .header("upgrade", "echo")
                        .body(Empty::<Bytes>::new())
                        .unwrap(),
                )
            }
        });
        hyper::server::conn::http1::Builder::new()
            .serve_connection(TokioIo::new(admitted), service)
            .with_upgrades()
            .await
    });
    client
        .write_all(
            b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: upgrade\r\nUpgrade: echo\r\n\r\n",
        )
        .await
        .unwrap();
    let response = within(async {
        let mut headers = Vec::new();
        while !headers.ends_with(b"\r\n\r\n") {
            assert!(headers.len() < 4096);
            headers.push(client.read_u8().await.unwrap());
        }
        headers
    })
    .await;
    assert!(response.starts_with(b"HTTP/1.1 101"));
    within(server).await.unwrap().unwrap();
    assert_eq!(budget.snapshot().in_flight, 1);
    assert!(budget.try_admit(()).is_err());
    client.write_all(b"ping").await.unwrap();
    let mut echoed = [0; 4];
    within(client.read_exact(&mut echoed)).await.unwrap();
    assert_eq!(&echoed, b"ping");
    drop(client);
    within(upgrade_finished_rx).await.unwrap();
    assert_eq!(budget.snapshot().in_flight, 0);
}

#[tokio::test]
async fn http_connection_budget_h2_parallel_streams_share_one_socket_permit() {
    let (client_io, server_io) = tcp_pair().await;
    let budget = Arc::new(HttpConnectionBudget::new(1));
    let admitted = budget.try_admit(server_io).unwrap();
    let requests = Arc::new(AtomicUsize::new(0));
    let concurrent = Arc::new(tokio::sync::Barrier::new(3));
    let server_requests = Arc::clone(&requests);
    let server_concurrent = Arc::clone(&concurrent);
    let server = tokio::spawn(async move {
        let service = service_fn(move |_: Request<Incoming>| {
            server_requests.fetch_add(1, Ordering::Relaxed);
            let concurrent = Arc::clone(&server_concurrent);
            async move {
                concurrent.wait().await;
                Ok::<_, Infallible>(Response::new(Empty::<Bytes>::new()))
            }
        });
        hyper::server::conn::http2::Builder::new(TokioExecutor::new())
            .max_concurrent_streams(2)
            .serve_connection(TokioIo::new(admitted), service)
            .await
    });
    let (sender, connection) = within(
        hyper::client::conn::http2::Builder::new(TokioExecutor::new())
            .handshake::<_, Empty<Bytes>>(TokioIo::new(client_io)),
    )
    .await
    .unwrap();
    let client_driver = tokio::spawn(connection);
    let mut requests_in_flight = tokio::task::JoinSet::new();
    for path in ["first", "second"] {
        let mut sender = sender.clone();
        requests_in_flight.spawn(async move {
            let response = sender
                .send_request(
                    Request::builder()
                        .uri(format!("http://localhost/{path}"))
                        .body(Empty::<Bytes>::new())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            response.into_body().collect().await.unwrap();
        });
    }
    within(concurrent.wait()).await;
    assert_eq!(requests.load(Ordering::Relaxed), 2);
    assert_eq!(budget.snapshot().in_flight, 1);
    assert_eq!(budget.snapshot().high_watermark, 1);
    within(async {
        while let Some(result) = requests_in_flight.join_next().await {
            result.unwrap();
        }
    })
    .await;
    assert_eq!(budget.snapshot().in_flight, 1);
    drop(sender);
    client_driver.abort();
    let _ = within(client_driver).await;
    let _ = within(server).await.unwrap();
    assert_eq!(budget.snapshot().in_flight, 0);
}

#[tokio::test(start_paused = true)]
async fn http_connection_accept_retries_peer_errors_and_backs_off_resource_errors() {
    let budget = HttpConnectionBudget::new(1);
    let mut attempts = VecDeque::from([
        Err(io::Error::from(io::ErrorKind::ConnectionAborted)),
        Err(io::Error::from(io::ErrorKind::ConnectionReset)),
        Err(io::Error::other("injected file descriptor exhaustion")),
        Err(io::Error::other("injected temporary accept failure")),
        Ok(42),
    ]);
    let started = tokio::time::Instant::now();
    let accepted = budget
        .accept_with(|| std::future::ready(attempts.pop_front().unwrap()))
        .await;
    assert_eq!(accepted, 42);
    assert!(attempts.is_empty());
    assert_eq!(started.elapsed(), Duration::from_secs(2));
    assert_eq!(budget.snapshot().accept_errors_total, 4);
    assert_eq!(budget.snapshot().in_flight, 0);
    assert_eq!(budget.snapshot().rejected_total, 0);
}
