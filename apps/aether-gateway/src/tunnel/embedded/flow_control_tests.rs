use super::*;

async fn fixture(
    window: u32,
    capacity: usize,
) -> (
    Arc<HubRouter>,
    Arc<ProxyConn>,
    Arc<LocalStream>,
    aether_runtime::BoundedQueueReceiver<Message>,
) {
    let hub = HubRouter::new(ControlPlaneClient::disabled());
    let (sender, receiver) = bounded_queue(capacity);
    let (close_tx, _) = watch::channel(false);
    let connection = Arc::new(
        ProxyConn::new(
            99,
            "flow-test".into(),
            "flow-test".into(),
            sender,
            close_tx,
            16,
            3,
        )
        .with_settings(protocol::SettingsPayload {
            initial_stream_window_bytes: window,
            min_window_update_bytes: (window / 4).max(1),
            drain_deadline_ms: 1000,
        }),
    );
    hub.register_proxy(Arc::clone(&connection));
    let stream = hub.open_local_stream("flow-test", &meta()).await.unwrap();
    (hub, connection, stream, receiver)
}

fn meta() -> protocol::RequestMeta {
    protocol::RequestMeta {
        provider_id: None,
        endpoint_id: None,
        key_id: None,
        method: "GET".into(),
        url: "https://example.com".into(),
        headers: HashMap::new(),
        stream: true,
        request_timeout_ms: None,
        stream_first_byte_timeout_ms: None,
        timeout: 30,
        follow_redirects: None,
        http1_only: false,
        transport_profile: None,
    }
}

async fn headers(hub: &Arc<HubRouter>, stream: &LocalStream) {
    let payload = serde_json::to_vec(&protocol::ResponseMeta {
        status: 200,
        headers: vec![],
    })
    .unwrap();
    let mut frame = protocol::encode_frame(
        stream.proxy_stream_id,
        protocol::RESPONSE_HEADERS,
        0,
        &payload,
    );
    hub.handle_proxy_frame(stream.proxy_conn_id, &mut frame)
        .await;
}

#[tokio::test]
async fn window_credit_is_retried_after_queue_pressure_and_cancelled_receive() {
    let (hub, _, stream, mut outbound) = fixture(128, 1).await;
    headers(&hub, &stream).await;
    assert!(stream.push_body_chunk(Bytes::from(vec![b'x'; 64])));
    let mut receiver = stream.take_body_receiver().unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(10), receiver.recv())
            .await
            .is_err()
    );
    assert_eq!(*stream.response_consumed_since_update.lock(), 64);
    outbound.recv().await.unwrap();
    let event = tokio::time::timeout(Duration::from_secs(1), receiver.recv())
        .await
        .unwrap();
    assert!(matches!(event, Some(LocalBodyEvent::Chunk(chunk)) if chunk.len() == 64));
    assert_eq!(*stream.response_consumed_since_update.lock(), 0);
    let Message::Binary(data) = outbound.recv().await.unwrap() else {
        panic!("expected binary update")
    };
    let frame = aether_contracts::tunnel::Frame::decode(data).unwrap();
    let update: protocol::WindowUpdatePayload = serde_json::from_slice(&frame.payload).unwrap();
    assert_eq!(
        frame.msg_type,
        aether_contracts::tunnel::MsgType::WindowUpdate
    );
    assert_eq!(update.delta_bytes, 64);
    hub.cancel_local_stream(stream.id, "test complete");
}

#[tokio::test]
async fn response_credit_is_not_returned_until_consumed() {
    let (hub, _, stream, mut outbound) = fixture(128, 4).await;
    outbound.recv().await.unwrap();
    let mut body = protocol::encode_frame(
        stream.proxy_stream_id,
        protocol::RESPONSE_BODY,
        0,
        &[b'x'; 128],
    );
    hub.handle_proxy_frame(99, &mut body).await;
    assert!(outbound.try_recv().is_err());
    let mut receiver = stream.take_body_receiver().unwrap();
    assert!(
        matches!(receiver.recv().await, Some(LocalBodyEvent::Chunk(chunk)) if chunk.len() == 128)
    );
    assert!(outbound.try_recv().is_ok());
    hub.cancel_local_stream(stream.id, "test complete");
}

#[tokio::test]
async fn cancelled_stream_open_releases_slot_without_resetting_connection() {
    let (hub, connection, first_stream, mut outbound) = fixture(128, 1).await;
    let opening_hub = Arc::clone(&hub);
    let opening =
        tokio::spawn(async move { opening_hub.open_local_stream("flow-test", &meta()).await });
    tokio::time::timeout(Duration::from_secs(1), async {
        while hub.local_streams.len() != 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    opening.abort();
    assert!(matches!(opening.await, Err(error) if error.is_cancelled()));
    assert_eq!(connection.stream_count.load(Ordering::Relaxed), 1);
    assert_eq!(hub.local_streams.len(), 1);
    assert_eq!(hub.proxy_to_local.len(), 1);
    assert!(connection.is_available());
    outbound.recv().await.unwrap();
    assert!(outbound.try_recv().is_err());
    let next_stream = hub.open_local_stream("flow-test", &meta()).await.unwrap();
    outbound.recv().await.unwrap();
    hub.cancel_local_stream(first_stream.id, "test complete");
    outbound.recv().await.unwrap();
    hub.cancel_local_stream(next_stream.id, "test complete");
}

#[tokio::test]
async fn full_response_buffer_preserves_disconnect_error() {
    let (hub, connection, stream, mut outbound) = fixture(4 * 1024 * 1024, 512).await;
    outbound.recv().await.unwrap();
    headers(&hub, &stream).await;
    let mut receiver = stream.take_body_receiver().unwrap();
    for _ in 0..128 {
        let mut frame = protocol::encode_frame(
            stream.proxy_stream_id,
            protocol::RESPONSE_BODY,
            0,
            &vec![b'x'; 32 * 1024],
        );
        hub.handle_proxy_frame(99, &mut frame).await;
    }
    hub.unregister_proxy(connection.id, &connection.node_id);
    let mut bytes = 0;
    loop {
        match receiver.recv().await {
            Some(LocalBodyEvent::Chunk(chunk)) => bytes += chunk.len(),
            Some(LocalBodyEvent::Error(error)) => {
                assert!(error.contains("disconnected"));
                break;
            }
            event => panic!("disconnect must not become normal EOF: {event:?}"),
        }
    }
    assert_eq!(bytes, 4 * 1024 * 1024);
}

#[tokio::test]
async fn slow_stream_does_not_block_another_stream_on_the_same_connection() {
    let (hub, _, slow, mut outbound) = fixture(128, 512).await;
    outbound.recv().await.unwrap();
    let fast = hub.open_local_stream("flow-test", &meta()).await.unwrap();
    assert!(slow.push_body_chunk(Bytes::from(vec![b'x'; 128])));
    let mut overflowing = protocol::encode_frame(
        slow.proxy_stream_id,
        protocol::RESPONSE_BODY,
        0,
        b"overflow",
    );
    tokio::time::timeout(Duration::from_secs(1), async {
        hub.handle_proxy_frame(99, &mut overflowing).await;
        headers(&hub, &fast).await;
        assert_eq!(
            fast.wait_headers(Duration::from_secs(1))
                .await
                .unwrap()
                .status,
            200
        );
    })
    .await
    .expect("slow stream must not block connection reader");
    assert!(!hub.local_streams.contains_key(&slow.id));
    assert!(hub.local_streams.contains_key(&fast.id));
    hub.cancel_local_stream(fast.id, "test complete");
}

#[tokio::test]
async fn cancelling_a_stream_wakes_request_window_waiters() {
    let (_, _, stream, _) = fixture(128, 512).await;
    *stream.request_window.available.lock() = 0;
    let waiter = tokio::spawn({
        let stream = Arc::clone(&stream);
        async move {
            stream
                .acquire_request_window(1, Duration::from_secs(30))
                .await
        }
    });
    tokio::task::yield_now().await;
    stream.fail("cancelled");
    assert!(tokio::time::timeout(Duration::from_secs(1), waiter)
        .await
        .unwrap()
        .unwrap()
        .is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_headers_and_credit_updates_do_not_lose_notifications() {
    for index in 0..256 {
        let stream = Arc::new(LocalStream::new(index, "test".into(), 1, 1, 1));
        let window = Arc::new(StreamFlowWindow::new(0));
        let waiter = tokio::spawn({
            let stream = Arc::clone(&stream);
            let window = Arc::clone(&window);
            async move {
                stream.wait_headers(Duration::from_secs(1)).await.unwrap();
                window.acquire(1, Duration::from_secs(1)).await.unwrap();
            }
        });
        stream.set_response_headers(protocol::ResponseMeta {
            status: 200,
            headers: vec![],
        });
        window.add(1);
        waiter.await.unwrap();
    }
}
