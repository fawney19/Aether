"use strict";

const assert = require("node:assert/strict");
const { EventEmitter } = require("node:events");
const test = require("node:test");

const { RelayClient } = require("../vscode-extension/dist/relayClient.js");

class FakeWebSocket extends EventEmitter {
  static instances = [];

  constructor(url) {
    super();
    this.url = url;
    this.readyState = 0;
    this.sent = [];
    FakeWebSocket.instances.push(this);
  }

  open() {
    this.readyState = 1;
    this.emit("open");
  }

  receive(frame) {
    this.emit("message", Buffer.from(JSON.stringify(frame)));
  }

  send(data) {
    this.sent.push(JSON.parse(data));
  }

  close() {
    if (this.readyState === 3) return;
    this.readyState = 3;
    this.emit("close");
  }
}

test("RelayClient queues application frames until auth.ok on initial connect and reconnect", async (t) => {
  FakeWebSocket.instances.length = 0;
  const client = new RelayClient({
    url: "ws://relay.invalid/v1/connect",
    accessToken: "host-token",
    reconnect: false,
    webSocket: FakeWebSocket,
  });
  t.after(() => client.close());

  const firstConnect = client.connect();
  const first = FakeWebSocket.instances[0];
  first.open();
  assert.deepEqual(first.sent.map((frame) => frame.kind), ["hello", "auth"]);

  client.send({ v: 1, kind: "event", type: "output.chunk", id: "event-1", sessionId: "session-1", payload: { text: "queued" } });
  assert.equal(first.sent.length, 2, "application event must not be sent before authentication");
  first.receive({ type: "auth.ok", role: "host", clientType: "host" });
  await firstConnect;
  assert.equal(first.sent.length, 3);
  assert.equal(first.sent[2].id, "event-1");

  first.close();
  const secondConnect = client.connect();
  const second = FakeWebSocket.instances[1];
  second.open();
  assert.deepEqual(second.sent.map((frame) => frame.kind), ["hello", "auth"]);

  client.send({ v: 1, kind: "event", type: "output.chunk", id: "event-2", sessionId: "session-1", payload: { text: "queued during reconnect" } });
  assert.equal(second.sent.length, 2, "reconnect window must remain auth-gated");
  second.receive({ type: "auth.ok", role: "host", clientType: "host" });
  await secondConnect;
  assert.equal(second.sent.length, 3);
  assert.equal(second.sent[2].id, "event-2");
});

test("RelayClient coalesces queued transcript projections within a byte budget", async (t) => {
  FakeWebSocket.instances.length = 0;
  const client = new RelayClient({
    url: "ws://relay.invalid/v1/connect",
    accessToken: "host-token",
    reconnect: false,
    maxFrameBytes: 4_096,
    maxQueuedBytes: 4_096,
    webSocket: FakeWebSocket,
  });
  t.after(() => client.close());

  const connecting = client.connect();
  const socket = FakeWebSocket.instances[0];
  socket.open();
  client.send({ v: 1, kind: "event", type: "approval.requested", id: "approval", sessionId: "session-1", payload: { text: "a".repeat(700) } });
  client.send({ v: 1, kind: "event", type: "output.snapshot", id: "old-projection", sessionId: "session-1", payload: { text: "x".repeat(1_200) } });
  client.send({ v: 1, kind: "event", type: "output.chunk", id: "new-projection", sessionId: "session-1", payload: { text: "y".repeat(1_200) } });
  client.send({ v: 1, kind: "event", type: "command.result", id: "command", sessionId: "session-1", payload: { text: "c".repeat(700) } });

  assert.ok(client.queueBytes <= 4_096);
  socket.receive({ type: "auth.ok", role: "host", clientType: "host" });
  await connecting;
  const queuedIds = socket.sent.slice(2).map((frame) => frame.id);
  assert.deepEqual(queuedIds, ["approval", "new-projection", "command"]);
});

test("RelayClient evicts reconstructible projections before queued control events", async (t) => {
  FakeWebSocket.instances.length = 0;
  const client = new RelayClient({
    url: "ws://relay.invalid/v1/connect",
    accessToken: "host-token",
    reconnect: false,
    maxFrameBytes: 4_096,
    maxQueuedBytes: 2_500,
    webSocket: FakeWebSocket,
  });
  t.after(() => client.close());

  const connecting = client.connect();
  const socket = FakeWebSocket.instances[0];
  socket.open();
  client.send({ v: 1, kind: "event", type: "approval.requested", id: "approval", sessionId: "session-1", payload: { text: "a".repeat(850) } });
  client.send({ v: 1, kind: "event", type: "output.chunk", id: "projection", sessionId: "session-1", payload: { text: "x".repeat(900) } });
  client.send({ v: 1, kind: "event", type: "command.result", id: "command", sessionId: "session-1", payload: { text: "c".repeat(850) } });

  assert.ok(client.queueBytes <= 2_500);
  socket.receive({ type: "auth.ok", role: "host", clientType: "host" });
  await connecting;
  const queuedIds = socket.sent.slice(2).map((frame) => frame.id);
  assert.deepEqual(queuedIds, ["approval", "command"]);
});

test("RelayClient supports a tokenless local handshake", async (t) => {
  FakeWebSocket.instances.length = 0;
  const client = new RelayClient({
    url: "ws://127.0.0.1:8787/v1/connect",
    reconnect: false,
    webSocket: FakeWebSocket,
  });
  t.after(() => client.close());

  const connecting = client.connect();
  const socket = FakeWebSocket.instances[0];
  socket.open();
  assert.deepEqual(socket.sent.map((frame) => frame.kind), ["hello"]);
  socket.receive({ type: "auth.ok", role: "host", clientType: "host", authRequired: false });
  await connecting;

  client.send({ v: 1, kind: "event", type: "connection.opened", id: "event-local", sessionId: "session-local", payload: {} });
  assert.equal(socket.sent.length, 2);
  assert.equal(socket.sent[1].type, "connection.opened");
});

test("RelayClient accepts structured history snapshots larger than the old 256 KiB limit", async (t) => {
  FakeWebSocket.instances.length = 0;
  const client = new RelayClient({
    url: "ws://127.0.0.1:8787/v1/connect",
    reconnect: false,
    webSocket: FakeWebSocket,
  });
  t.after(() => client.close());

  const connecting = client.connect();
  const socket = FakeWebSocket.instances[0];
  socket.open();
  socket.receive({ type: "auth.ok", role: "host", clientType: "host", authRequired: false });
  await connecting;

  const historyText = "x".repeat(512 * 1024);
  assert.doesNotThrow(() => client.send({
    v: 1,
    kind: "event",
    type: "session.snapshot",
    id: "large-history-snapshot",
    sessionId: "session-local",
    payload: { threadId: "large-thread", messages: [{ kind: "assistant", text: historyText }] },
  }));
  assert.equal(socket.sent.at(-1).payload.messages[0].text.length, historyText.length);
});

test("RelayClient ignores late events from a replaced socket", async (t) => {
  FakeWebSocket.instances.length = 0;
  const client = new RelayClient({
    url: "ws://relay.invalid/v1/connect",
    accessToken: "host-token",
    reconnect: false,
    webSocket: FakeWebSocket,
  });
  t.after(() => client.close());

  const firstConnect = client.connect();
  const first = FakeWebSocket.instances[0];
  first.open();
  client.close();

  const secondConnect = client.connect();
  const second = FakeWebSocket.instances[1];
  second.open();

  // Simulate a delayed event from the old socket after the replacement.
  first.open();
  first.receive({ type: "auth.ok", role: "host", clientType: "host" });
  assert.equal(second.sent.length, 2, "late auth must not authenticate or flush the new socket");

  client.send({ v: 1, kind: "event", type: "output.chunk", id: "event-after-replace", sessionId: "session-1", payload: { text: "queued" } });
  second.receive({ type: "auth.ok", role: "host", clientType: "host" });
  await secondConnect;
  assert.equal(second.sent[2].id, "event-after-replace");
  await assert.rejects(firstConnect);
});
