"""
aether-hub local relay 端到端测试

测试流程:
1. 启动 aether-hub（绑定随机端口）
2. 用 websockets 库模拟一个 aether-proxy client 连接到 Hub
3. Mock proxy 在收到请求帧后返回固定响应帧
4. 通过 Hub 的 /local/relay/{node_id} HTTP API 发送请求
5. 验证完整链路: HTTP request -> Hub -> WS frame -> mock proxy -> WS frame -> Hub -> HTTP response

运行: uv run python tests/e2e_hub_relay.py
"""

from __future__ import annotations

import asyncio
import gzip
import json
import os
import signal
import struct
import subprocess
import sys
import time

import httpx

# ---------------------------------------------------------------------------
# Protocol constants (mirror aether-hub/src/protocol.rs)
# ---------------------------------------------------------------------------
HEADER_SIZE = 10

REQUEST_HEADERS = 0x01
REQUEST_BODY = 0x02
RESPONSE_HEADERS = 0x03
RESPONSE_BODY = 0x04
STREAM_END = 0x05
STREAM_ERROR = 0x06
PING = 0x10
PONG = 0x11
GOAWAY = 0x12

FLAG_END_STREAM = 0x01
FLAG_GZIP_COMPRESSED = 0x02


def encode_frame(stream_id: int, msg_type: int, flags: int, payload: bytes) -> bytes:
    header = struct.pack(">I", stream_id) + bytes([msg_type, flags]) + struct.pack(">I", len(payload))
    return header + payload


def parse_frame(data: bytes) -> tuple[int, int, int, bytes] | None:
    if len(data) < HEADER_SIZE:
        return None
    stream_id = struct.unpack(">I", data[0:4])[0]
    msg_type = data[4]
    flags = data[5]
    payload_len = struct.unpack(">I", data[6:10])[0]
    if len(data) < HEADER_SIZE + payload_len:
        return None
    payload = data[HEADER_SIZE : HEADER_SIZE + payload_len]
    if flags & FLAG_GZIP_COMPRESSED:
        payload = gzip.decompress(payload)
    return stream_id, msg_type, flags, payload


def encode_relay_envelope(meta: dict, body: bytes) -> bytes:
    meta_json = json.dumps(meta, ensure_ascii=False, separators=(",", ":")).encode("utf-8")
    return struct.pack("!I", len(meta_json)) + meta_json + body


# ---------------------------------------------------------------------------
# Mock aether-proxy: connects to Hub via WebSocket, handles request frames
# ---------------------------------------------------------------------------
async def mock_proxy(hub_ws_url: str, node_id: str, ready_event: asyncio.Event) -> None:
    """Simulate an aether-proxy node that echoes requests as fixed responses."""
    try:
        import websockets
    except ImportError:
        print("SKIP: websockets package not installed (uv pip install websockets)")
        sys.exit(1)

    headers = {
        "X-Node-ID": node_id,
        "X-Node-Name": f"test-{node_id}",
    }

    async with websockets.connect(
        hub_ws_url,
        additional_headers=headers,
        max_size=64 * 1024 * 1024,
    ) as ws:
        ready_event.set()
        print(f"  [mock-proxy] connected to hub as node_id={node_id}")

        request_meta: dict | None = None
        request_body: bytes = b""

        async for raw_msg in ws:
            if not isinstance(raw_msg, bytes):
                continue

            parsed = parse_frame(raw_msg)
            if parsed is None:
                continue

            stream_id, msg_type, flags, payload = parsed

            if msg_type == PING:
                await ws.send(encode_frame(0, PONG, 0, payload))
                continue

            if msg_type == REQUEST_HEADERS:
                request_meta = json.loads(payload)
                print(f"  [mock-proxy] stream={stream_id} got REQUEST_HEADERS: {request_meta.get('method')} {request_meta.get('url')}")

            elif msg_type == REQUEST_BODY:
                request_body = payload
                is_end = bool(flags & FLAG_END_STREAM)
                print(f"  [mock-proxy] stream={stream_id} got REQUEST_BODY ({len(payload)} bytes, end={is_end})")

                if is_end and request_meta:
                    # Send response: 200 OK with echoed body
                    resp_meta = {
                        "status": 200,
                        "headers": [
                            ["content-type", "application/json"],
                            ["x-test-echo", "true"],
                        ],
                    }
                    resp_meta_json = json.dumps(resp_meta, separators=(",", ":")).encode("utf-8")
                    await ws.send(encode_frame(stream_id, RESPONSE_HEADERS, 0, resp_meta_json))

                    echo_body = json.dumps({
                        "echo": True,
                        "received_method": request_meta.get("method"),
                        "received_url": request_meta.get("url"),
                        "received_body_len": len(request_body),
                    }, separators=(",", ":")).encode("utf-8")
                    await ws.send(encode_frame(stream_id, RESPONSE_BODY, 0, echo_body))
                    await ws.send(encode_frame(stream_id, STREAM_END, 0, b""))
                    print(f"  [mock-proxy] stream={stream_id} sent response (200, {len(echo_body)} bytes)")

                    request_meta = None
                    request_body = b""

            elif msg_type == STREAM_ERROR:
                error_msg = payload.decode("utf-8", errors="replace")
                print(f"  [mock-proxy] stream={stream_id} got STREAM_ERROR: {error_msg}")


# ---------------------------------------------------------------------------
# Test runner
# ---------------------------------------------------------------------------
async def run_test() -> bool:
    hub_port = 18085
    hub_bind = f"127.0.0.1:{hub_port}"
    hub_binary = os.path.join(
        os.path.dirname(__file__),
        "..",
        "aether-hub",
        "target",
        "release",
        "aether-hub",
    )
    hub_binary = os.path.normpath(hub_binary)

    if not os.path.isfile(hub_binary):
        print(f"FAIL: hub binary not found at {hub_binary}")
        print("  run: cd aether-hub && cargo build --release")
        return False

    # Start aether-hub (with control plane disabled since we don't have the app running)
    print(f"[1/5] Starting aether-hub on {hub_bind} ...")
    hub_proc = subprocess.Popen(
        [
            hub_binary,
            "--bind", hub_bind,
            "--proxy-idle-timeout", "0",
            "--ping-interval", "30",
            # Use a non-existent app URL -- control plane callbacks will fail silently
            "--app-base-url", "http://127.0.0.1:19999",
        ],
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    )

    try:
        # Wait for hub to be ready
        for _ in range(30):
            await asyncio.sleep(0.2)
            try:
                async with httpx.AsyncClient() as client:
                    resp = await client.get(f"http://{hub_bind}/health", timeout=1.0)
                    if resp.status_code == 200:
                        print("  hub is healthy")
                        break
            except Exception:
                continue
        else:
            print("FAIL: hub did not start in time")
            return False

        # Check initial stats
        async with httpx.AsyncClient() as client:
            stats = (await client.get(f"http://{hub_bind}/stats")).json()
            print(f"  initial stats: {stats}")
            assert stats["proxy_connections"] == 0
            assert stats["nodes"] == 0

        # Start mock proxy
        node_id = "test-node-e2e"
        proxy_ready = asyncio.Event()
        print(f"\n[2/5] Connecting mock proxy (node_id={node_id}) ...")
        proxy_task = asyncio.create_task(
            mock_proxy(f"ws://{hub_bind}/proxy", node_id, proxy_ready)
        )

        try:
            await asyncio.wait_for(proxy_ready.wait(), timeout=5.0)
        except asyncio.TimeoutError:
            print("FAIL: mock proxy did not connect in time")
            return False

        # Give hub a moment to register
        await asyncio.sleep(0.3)

        async with httpx.AsyncClient() as client:
            stats = (await client.get(f"http://{hub_bind}/stats")).json()
            print(f"  stats after connect: {stats}")
            assert stats["proxy_connections"] == 1, f"expected 1 proxy connection, got {stats['proxy_connections']}"
            assert stats["nodes"] == 1

        # Send request through local relay
        print(f"\n[3/5] Sending request via local relay ...")
        request_body = b'{"model":"test","messages":[]}'
        envelope = encode_relay_envelope(
            {
                "method": "POST",
                "url": "https://api.example.com/v1/chat/completions",
                "headers": {
                    "content-type": "application/json",
                    "authorization": "Bearer sk-test-123",
                },
                "timeout": 30,
            },
            request_body,
        )

        async with httpx.AsyncClient() as client:
            relay_url = f"http://{hub_bind}/local/relay/{node_id}"
            resp = await client.post(
                relay_url,
                content=envelope,
                headers={"content-type": "application/vnd.aether.tunnel-envelope"},
                timeout=10.0,
            )

        print(f"  relay response: status={resp.status_code}")
        assert resp.status_code == 200, f"expected 200, got {resp.status_code}: {resp.text}"

        echo = resp.json()
        print(f"  echo body: {echo}")
        assert echo["echo"] is True
        assert echo["received_method"] == "POST"
        assert echo["received_url"] == "https://api.example.com/v1/chat/completions"
        assert echo["received_body_len"] == len(request_body)

        assert resp.headers.get("x-test-echo") == "true"

        # Verify active streams cleaned up
        print(f"\n[4/5] Verifying stream cleanup ...")
        await asyncio.sleep(0.2)
        async with httpx.AsyncClient() as client:
            stats = (await client.get(f"http://{hub_bind}/stats")).json()
            print(f"  stats after request: {stats}")
            assert stats["active_streams"] == 0, f"expected 0 active streams, got {stats['active_streams']}"

        # Test error case: request to non-existent node
        print(f"\n[5/5] Testing error cases ...")
        async with httpx.AsyncClient() as client:
            resp = await client.post(
                f"http://{hub_bind}/local/relay/non-existent-node",
                content=encode_relay_envelope(
                    {"method": "GET", "url": "https://example.com", "headers": {}, "timeout": 5},
                    b"",
                ),
                headers={"content-type": "application/vnd.aether.tunnel-envelope"},
                timeout=5.0,
            )
            assert resp.status_code == 503, f"expected 503 for missing node, got {resp.status_code}"
            assert resp.headers.get("x-aether-tunnel-error") == "connect"
            print(f"  missing node: status={resp.status_code}, error={resp.text}")

        # Test: request from non-loopback should be rejected
        # (can't easily test from non-loopback, but verify header is present for valid errors)

        # Cleanup: cancel proxy
        proxy_task.cancel()
        try:
            await proxy_task
        except asyncio.CancelledError:
            pass

        print("\n" + "=" * 50)
        print("ALL TESTS PASSED")
        print("=" * 50)
        return True

    finally:
        hub_proc.send_signal(signal.SIGTERM)
        try:
            hub_proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            hub_proc.kill()
            hub_proc.wait()
        print("\n[cleanup] hub process stopped")


if __name__ == "__main__":
    success = asyncio.run(run_test())
    sys.exit(0 if success else 1)
