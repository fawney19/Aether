"use strict";

const assert = require("node:assert/strict");
const http = require("node:http");
const path = require("node:path");
const test = require("node:test");
const { WebSocket } = require("ws");
const { CodexRelay } = require("../relay/server.js");

const fakeServer = path.join(__dirname, "..", "fixtures", "fake-app-server.cjs");

function waitFor(predicate, timeout = 5_000) {
  const started = Date.now();
  return new Promise((resolve, reject) => {
    const tick = () => {
      try {
        const result = predicate();
        if (result) return resolve(result);
      } catch (error) { return reject(error); }
      if (Date.now() - started > timeout) return reject(new Error("timed out waiting for condition"));
      setTimeout(tick, 20);
    };
    tick();
  });
}

function request(base, token, pathname, options = {}) {
  return fetch(`${base}${pathname}`, {
    ...options,
    headers: { Authorization: `Bearer ${token}`, ...(options.headers || {}) },
  });
}

function connectWs(base, token) {
  const ws = new WebSocket(base.replace(/^http/, "ws") + "/ws");
  const messages = [];
  const waiters = [];
  ws.on("message", (data) => {
    const message = JSON.parse(data.toString());
    messages.push(message);
    for (let index = waiters.length - 1; index >= 0; index -= 1) {
      if (waiters[index].predicate(message)) {
        const waiter = waiters.splice(index, 1)[0];
        waiter.resolve(message);
      }
    }
  });
  const wait = (predicate, timeout = 5_000) => new Promise((resolve, reject) => {
    const existing = messages.find(predicate);
    if (existing) return resolve(existing);
    const timer = setTimeout(() => {
      const index = waiters.findIndex((waiter) => waiter.resolve === resolve);
      if (index >= 0) waiters.splice(index, 1);
      reject(new Error("timed out waiting for websocket message"));
    }, timeout);
    waiters.push({ predicate, resolve: (message) => { clearTimeout(timer); resolve(message); } });
  });
  return new Promise((resolve, reject) => {
    ws.once("open", () => {
      ws.send(JSON.stringify({ type: "auth", token }));
      wait((message) => message.type === "auth.ok").then(() => {
        ws.send(JSON.stringify({ type: "subscribe", fromSeq: 0 }));
        resolve({ ws, wait, messages });
      }, reject);
    });
    ws.once("error", reject);
  });
}

function connectBrowserHello(base, token) {
  const ws = new WebSocket(base.replace(/^http/, "ws") + "/ws");
  const messages = [];
  const waiters = [];
  ws.on("message", (data) => {
    const message = JSON.parse(data.toString());
    messages.push(message);
    for (let index = waiters.length - 1; index >= 0; index -= 1) {
      if (waiters[index].predicate(message)) {
        const waiter = waiters.splice(index, 1)[0];
        waiter.resolve(message);
      }
    }
  });
  const wait = (predicate, timeout = 5_000) => new Promise((resolve, reject) => {
    const existing = messages.find(predicate);
    if (existing) return resolve(existing);
    const timer = setTimeout(() => {
      const index = waiters.findIndex((waiter) => waiter.resolve === resolve);
      if (index >= 0) waiters.splice(index, 1);
      reject(new Error("timed out waiting for websocket message"));
    }, timeout);
    waiters.push({ predicate, resolve: (message) => { clearTimeout(timer); resolve(message); } });
  });
  return new Promise((resolve, reject) => {
    ws.once("open", () => {
      ws.send(JSON.stringify({ v: 1, kind: "hello", clientType: "web", protocol: 1 }));
      if (token !== undefined) ws.send(JSON.stringify({ type: "auth", token }));
      wait((message) => message.type === "auth.ok").then(() => resolve({ ws, wait, messages }), reject);
    });
    ws.once("error", reject);
  });
}

function connectHost(base, token, sessionId = "host-session") {
  const ws = new WebSocket(base.replace(/^http/, "ws") + "/v1/connect");
  const messages = [];
  const waiters = [];
  ws.on("message", (data) => {
    const message = JSON.parse(data.toString());
    messages.push(message);
    for (let index = waiters.length - 1; index >= 0; index -= 1) {
      if (waiters[index].predicate(message)) {
        const waiter = waiters.splice(index, 1)[0];
        waiter.resolve(message);
      }
    }
  });
  const wait = (predicate, timeout = 5_000) => new Promise((resolve, reject) => {
    const existing = messages.find(predicate);
    if (existing) return resolve(existing);
    const timer = setTimeout(() => {
      const index = waiters.findIndex((waiter) => waiter.resolve === resolve);
      if (index >= 0) waiters.splice(index, 1);
      reject(new Error("timed out waiting for host websocket message"));
    }, timeout);
    waiters.push({ predicate, resolve: (message) => { clearTimeout(timer); resolve(message); } });
  });
  return new Promise((resolve, reject) => {
    ws.once("open", () => {
      ws.send(JSON.stringify({ v: 1, kind: "hello", clientType: "host", protocol: 1, sessionId }));
      ws.send(JSON.stringify({ v: 1, kind: "auth", accessToken: token }));
      wait((message) => message.type === "auth.ok" && message.clientType === "host").then(() => {
        resolve({ ws, wait, messages, sessionId });
      }, reject);
    });
    ws.once("error", reject);
  });
}

test("loopback relay defaults to tokenless browser and VS Code host connections", async (t) => {
  const relay = new CodexRelay({
    host: "127.0.0.1",
    port: 0,
    mode: "host",
  });
  assert.equal(relay.authRequired, false);
  await relay.start();
  t.after(() => relay.stop());
  const address = relay.address();
  const base = `http://127.0.0.1:${address.port}`;

  const health = await fetch(`${base}/api/health`);
  assert.equal(health.status, 200);
  assert.equal((await health.json()).authRequired, false);

  const state = await fetch(`${base}/api/state`);
  assert.equal(state.status, 200);
  assert.equal((await state.json()).role, "operator");

  const crossOriginWrite = await fetch(`${base}/api/command`, {
    method: "POST",
    headers: { Origin: "https://untrusted.example", "Content-Type": "application/json" },
    body: JSON.stringify({ commandId: "cross-origin", method: "turn/start", params: {} }),
  });
  assert.equal(crossOriginWrite.status, 403);

  const reboundHost = await new Promise((resolve, reject) => {
    const request = http.request({
      host: "127.0.0.1",
      port: address.port,
      path: "/api/health",
      headers: { Host: `rebound.example:${address.port}` },
    }, (response) => {
      response.resume();
      response.once("end", () => resolve(response));
    });
    request.once("error", reject);
    request.end();
  });
  assert.equal(reboundHost.statusCode, 403);

  const deceptiveHost = await new Promise((resolve, reject) => {
    const request = http.request({
      host: "127.0.0.1",
      port: address.port,
      path: "/api/health",
      headers: { Host: `127.evil:${address.port}` },
    }, (response) => {
      response.resume();
      response.once("end", () => resolve(response));
    });
    request.once("error", reject);
    request.end();
  });
  assert.equal(deceptiveHost.statusCode, 403);

  const host = await connectHost(base, undefined, "tokenless-host");
  t.after(() => host.ws.close());
  const browser = await connectWs(base, undefined);
  t.after(() => browser.ws.close());
  const browserHello = await connectBrowserHello(base);
  t.after(() => browserHello.ws.close());
  const browserStaleToken = await connectBrowserHello(base, "stale-local-token");
  t.after(() => browserStaleToken.ws.close());
  assert.equal(host.messages.find((message) => message.type === "auth.ok")?.role, "host");
  assert.equal(browser.messages.find((message) => message.type === "auth.ok")?.role, "operator");
  assert.equal(browserHello.messages.find((message) => message.type === "auth.ok")?.authRequired, false);
  assert.equal(browserStaleToken.messages.some((message) => message.type === "error"), false);
});

test("loopback relay hydrates attached-session snapshots larger than 256 KiB", async (t) => {
  const relay = new CodexRelay({ host: "127.0.0.1", port: 0, mode: "host" });
  await relay.start();
  t.after(() => relay.stop());
  const address = relay.address();
  const base = `http://127.0.0.1:${address.port}`;

  const host = await connectHost(base, undefined, "large-snapshot-host");
  const browser = await connectWs(base, undefined);
  t.after(() => host.ws.close());
  t.after(() => browser.ws.close());

  const historyText = "x".repeat(512 * 1024);
  host.ws.send(JSON.stringify({
    v: 1,
    kind: "event",
    type: "session.snapshot",
    id: "large-snapshot-event",
    sessionId: host.sessionId,
    seq: 1,
    ts: new Date().toISOString(),
    payload: {
      threadId: "large-thread",
      state: "idle",
      outputTail: historyText.slice(-32_000),
      messages: [{ kind: "assistant", text: historyText }],
      metadata: { title: "Large attached session", historyComplete: false },
    },
  }));

  const snapshot = await browser.wait((message) => message.kind === "event"
    && message.type === "session.snapshot"
    && message.payload?.sourceSeq === 1);
  assert.equal(snapshot.payload.messages[0].text.length, historyText.length);
  assert.equal(relay.state.messages[0].text.length, historyText.length);
  assert.equal(relay.state.sessionMetadata.title, "Large attached session");
  assert.equal(relay.state.sessionMetadata.historyComplete, false);
  const replayed = relay.events.find((event) => event.type === "session.snapshot" && event.payload?.sourceSeq === 1);
  assert.equal(replayed.payload.messages, undefined);
  assert.equal(replayed.payload.projectionInControlSnapshot, true);
  const controlSnapshot = relay.snapshot();
  assert.equal(controlSnapshot.state.messages, undefined);
  assert.equal(controlSnapshot.state.outputTail, undefined);
  assert.equal(controlSnapshot.state.subagents, undefined);
  assert.equal(controlSnapshot.state.sessionMetadata, undefined);
  assert.equal(controlSnapshot.metadata.historyComplete, false);
  const serializedControl = JSON.stringify(controlSnapshot);
  assert.ok(Buffer.byteLength(serializedControl) < historyText.length + 100_000, "history must be serialized only once");
});

test("late browser control snapshot preserves the attach waiting state", async (t) => {
  const relay = new CodexRelay({ host: "127.0.0.1", port: 0, mode: "host" });
  await relay.start();
  t.after(() => relay.stop());
  const address = relay.address();
  const base = `http://127.0.0.1:${address.port}`;

  const host = await connectHost(base, undefined, "waiting-session-host");
  t.after(() => host.ws.close());
  host.ws.send(JSON.stringify({
    v: 1,
    kind: "event",
    type: "session.snapshot",
    id: "waiting-session-snapshot",
    sessionId: host.sessionId,
    seq: 1,
    ts: new Date().toISOString(),
    payload: {
      threadId: null,
      state: "waiting_for_host",
      metadata: { waitingForSession: true, attachReady: false },
    },
  }));
  await host.wait((message) => message.kind === "ack" && message.seq === 1);

  const browser = await connectWs(base, undefined);
  t.after(() => browser.ws.close());
  const control = await browser.wait((message) => message.type === "session.snapshot"
    && message.snapshot && typeof message.snapshot === "object");
  assert.equal(control.snapshot.state.activeThreadId, null);
  assert.equal(control.snapshot.metadata.waitingForSession, true);
  assert.equal(control.snapshot.metadata.attachReady, false);
});

test("relay keeps live transcript events rich while bounding the replay ring", () => {
  const relay = new CodexRelay({
    host: "127.0.0.1",
    port: 0,
    mode: "host",
    eventByteLimit: 4_096,
  });
  const liveClient = { id: "live", role: "operator", authenticated: true, subscribed: true, capture: [] };
  relay.clients.add(liveClient);
  relay.recordEvent("output.chunk", {
    threadId: "thread-large",
    text: "delta",
    outputTail: "o".repeat(32_000),
    messages: [{ text: "m".repeat(32_000) }],
    messagesPatch: { start: 0, deleteCount: 0, messages: [{ text: "p".repeat(32_000) }] },
    subagents: [{ output: "s".repeat(32_000) }],
    raw: { transcript: "r".repeat(32_000) },
  });

  assert.equal(liveClient.capture[0].payload.messagesPatch.messages[0].text.length, 32_000);
  const replayed = relay.events[0];
  assert.equal(replayed.payload.text, "delta");
  assert.equal(replayed.payload.messages, undefined);
  assert.equal(replayed.payload.messagesPatch, undefined);
  assert.equal(replayed.payload.outputTail, undefined);
  assert.equal(replayed.payload.subagents, undefined);
  assert.equal(replayed.payload.raw, undefined);
  assert.equal(replayed.payload.projectionInControlSnapshot, true);

  relay.clients.delete(liveClient);
  for (let index = 0; index < 30; index += 1) {
    relay.recordEvent("command.pending", { commandId: `command-${index}`, note: "n".repeat(400) });
  }
  assert.ok(relay.eventBytes <= 4_096);
  assert.equal(relay.eventBytes, relay.eventSizes.reduce((total, size) => total + size, 0));
  assert.ok(relay.events.length < 30, "byte budget should prune before the count limit");
});

test("relay skips oversized replay windows and sends one authoritative snapshot", () => {
  const relay = new CodexRelay({
    host: "127.0.0.1",
    port: 0,
    mode: "host",
    eventByteLimit: 64 * 1024,
    replayByteLimit: 400,
  });
  relay.state.messages = [{ kind: "assistant", text: "authoritative history" }];
  for (let index = 0; index < 5; index += 1) {
    relay.recordEvent("command.pending", { commandId: `command-${index}`, note: "n".repeat(180) });
  }
  const client = { id: "late", role: "operator", authenticated: true, subscribed: false, capture: [] };
  relay.clients.add(client);
  relay.subscribe(client, 0);

  assert.equal(client.capture[0].type, "resync.required");
  assert.equal(client.capture[0].reason, "replay_too_large");
  assert.equal(client.capture.filter((message) => message.kind === "event").length, 0);
  const snapshot = client.capture.find((message) => message.type === "session.snapshot");
  assert.equal(snapshot.snapshot.messages[0].text, "authoritative history");
  assert.equal(snapshot.snapshot.latestSeq, relay.nextSeq);
});

test("relay socket backpressure accounts for the serialized frame size", () => {
  const relay = new CodexRelay({
    host: "127.0.0.1",
    port: 0,
    mode: "host",
    clientBufferedByteLimit: 2_048,
  });
  const accepted = {
    readyState: WebSocket.OPEN,
    bufferedAmount: 1_000,
    sent: [],
    send(value) { this.sent.push(value); },
    close(code, reason) { this.closed = { code, reason }; },
  };
  relay.sendControl({ socket: accepted }, { type: "small", text: "x".repeat(500) });
  assert.equal(accepted.sent.length, 1);
  assert.equal(accepted.closed, undefined);

  const slow = {
    readyState: WebSocket.OPEN,
    bufferedAmount: 1_900,
    sent: [],
    send(value) { this.sent.push(value); },
    close(code, reason) { this.closed = { code, reason }; },
  };
  relay.sendControl({ socket: slow }, { type: "small", text: "x".repeat(500) });
  assert.equal(slow.sent.length, 0);
  assert.equal(slow.closed.code, 1013);
});

test("loopback relay can explicitly require tokens", async (t) => {
  const relay = new CodexRelay({
    host: "127.0.0.1",
    port: 0,
    mode: "host",
    authRequired: true,
    operatorToken: "explicit-operator-token",
    viewerToken: "explicit-viewer-token",
    hostToken: "explicit-host-token",
  });
  assert.equal(relay.authRequired, true);
  await relay.start();
  t.after(() => relay.stop());
  const address = relay.address();
  const base = `http://127.0.0.1:${address.port}`;
  const health = await fetch(`${base}/api/health`);
  assert.equal((await health.json()).authRequired, true);
  assert.equal((await fetch(`${base}/api/state`)).status, 401);

  const browser = await connectWs(base, "explicit-operator-token");
  t.after(() => browser.ws.close());
  assert.equal(browser.messages.find((message) => message.type === "auth.ok")?.role, "operator");
});

test("non-loopback relay requires tokens by default", () => {
  const relay = new CodexRelay({
    host: "0.0.0.0",
    port: 0,
    mode: "host",
  });
  assert.equal(relay.authRequired, true);
});

test("deceptive numeric-looking hostnames never enable local no-auth", () => {
  const relay = new CodexRelay({ host: "127.evil", port: 0, mode: "host" });
  assert.equal(relay.authRequired, true);
});

test("relay defaults to host mode so bare npm start does not spawn Codex", () => {
  const relay = new CodexRelay({ host: "127.0.0.1", port: 0, authRequired: false });
  assert.equal(relay.mode, "host");
  assert.equal(relay.spawnCodex, false);
});

test("thread settings validation accepts null reasoning effort", () => {
  const relay = new CodexRelay({ host: "127.0.0.1", port: 0, mode: "host" });
  assert.equal(relay.validateCommand("thread/settings/update", {
    threadId: "thread-test",
    threadSettings: { model: "gpt-5.6-sol", effort: null },
  }), null);
  assert.match(relay.validateCommand("thread/settings/update", {
    threadId: "thread-test",
    threadSettings: { effort: 3 },
  }), /string or null/);
});

test("control mode validation only accepts sync and async", () => {
  const relay = new CodexRelay({ host: "127.0.0.1", port: 0, mode: "host" });
  assert.equal(relay.validateCommand("control/mode/get", {}), null);
  assert.equal(relay.validateCommand("control/mode/set", { mode: "sync" }), null);
  assert.equal(relay.validateCommand("control/mode/set", { mode: "async" }), null);
  assert.match(relay.validateCommand("control/mode/set", { mode: "attach" }), /sync or async/);
  assert.match(relay.validateCommand("control/mode/get", { mode: "sync" }), /does not accept/);
});

test("authenticated relay forwards commands, output, approval requests, and replay", async (t) => {
  const relay = new CodexRelay({
    host: "127.0.0.1",
    port: 0,
    mode: "embedded",
    operatorToken: "operator-test-token",
    viewerToken: "viewer-test-token",
    codexCommand: process.execPath,
    codexArgs: [fakeServer],
  });
  await relay.start();
  t.after(() => relay.stop());
  const address = relay.address();
  const base = `http://127.0.0.1:${address.port}`;
  await waitFor(() => relay.state.initialized);

  const health = await fetch(`${base}/api/health`);
  assert.equal(health.status, 200);
  const unauthorized = await fetch(`${base}/api/state`);
  assert.equal(unauthorized.status, 401);

  const viewerCommand = await request(base, "viewer-test-token", "/api/command", {
    method: "POST",
    body: JSON.stringify({ commandId: "viewer-1", method: "thread/start", params: {} }),
    headers: { "Content-Type": "application/json" },
  });
  assert.equal(viewerCommand.status, 400);
  assert.equal((await viewerCommand.json()).code, "forbidden");

  const client = await connectWs(base, "operator-test-token");
  t.after(() => client.ws.close());
  await client.wait((message) => message.type === "session.snapshot");

  client.ws.send(JSON.stringify({ type: "command", commandId: "thread-1", method: "thread/start", params: { cwd: "/tmp", sandbox: "workspace-write" } }));
  const threadResult = await client.wait((message) => message.type === "command.result" && message.payload?.commandId === "thread-1");
  assert.equal(threadResult.payload.ok, true);
  const threadId = threadResult.payload.result.thread.id;
  assert.equal(relay.state.activeThreadId, threadId);

  client.ws.send(JSON.stringify({ type: "command", commandId: "turn-1", method: "turn/start", params: { threadId, input: [{ type: "text", text: "approve this", text_elements: [] }] } }));
  await client.wait((message) => message.type === "command.result" && message.payload?.commandId === "turn-1");
  const approval = await client.wait((message) => message.kind === "event" && message.type === "approval.requested");
  assert.equal(approval.payload.requestId, 9001);
  assert.equal(approval.payload.method, "item/commandExecution/requestApproval");

  client.ws.send(JSON.stringify({ type: "respond", requestId: "9001", result: { decision: "accept" } }));
  await client.wait((message) => message.kind === "event" && message.type === "server.responded");
  await client.wait((message) => message.kind === "event" && message.type === "output.delta" && message.payload.text.includes("approval response"));
  assert.equal(relay.pendingServerRequests.size, 0);

  const events = await request(base, "viewer-test-token", "/api/events?fromSeq=0");
  const eventBody = await events.json();
  assert.ok(eventBody.latestSeq >= 1);
  assert.ok(eventBody.events.some((event) => event.type === "approval.requested"));
});

test("host mode proxies browser commands and preserves one-shot approval request ids", async (t) => {
  const relay = new CodexRelay({
    host: "127.0.0.1",
    port: 0,
    mode: "host",
    operatorToken: "operator-test-token",
    viewerToken: "viewer-test-token",
    hostToken: "separate-host-token",
  });
  await relay.start();
  t.after(() => relay.stop());
  const address = relay.address();
  const base = `http://127.0.0.1:${address.port}`;

  const host = await connectHost(base, "separate-host-token", "session-from-vscode");
  const browser = await connectWs(base, "operator-test-token");
  t.after(() => host.ws.close());
  t.after(() => browser.ws.close());
  await browser.wait((message) => message.type === "session.snapshot");

  host.ws.send(JSON.stringify({
    v: 1,
    kind: "event",
    type: "connection.opened",
    id: "host-event-1",
    sessionId: host.sessionId,
    seq: 1,
    ts: new Date().toISOString(),
    payload: {},
  }));
  await browser.wait((message) => message.kind === "event" && message.type === "connection.opened");
  assert.equal(relay.state.initialized, true);

  host.ws.send(JSON.stringify({
    v: 1,
    kind: "event",
    type: "output.chunk",
    id: "host-event-2",
    sessionId: host.sessionId,
    seq: 2,
    ts: new Date().toISOString(),
    payload: { stream: "codex", text: "output from VS Code" },
  }));
  const output = await browser.wait((message) => message.kind === "event" && message.type === "output.chunk");
  assert.equal(output.payload.text, "output from VS Code");
  const ack = await host.wait((message) => message.kind === "ack" && message.seq === 2);
  assert.equal(ack.sessionId, host.sessionId);

  browser.ws.send(JSON.stringify({
    type: "command",
    commandId: "host-thread-1",
    method: "thread/start",
    params: { cwd: "/tmp", sandbox: "workspace-write" },
  }));
  const hostCommand = await host.wait((message) => message.kind === "command" && message.commandId === "host-thread-1");
  assert.equal(hostCommand.type, "thread/start");
  assert.equal(hostCommand.payload.sandbox, "workspace-write");
  host.ws.send(JSON.stringify({
    v: 1,
    kind: "event",
    type: "command.accepted",
    id: "host-result-1",
    sessionId: host.sessionId,
    seq: 3,
    ts: new Date().toISOString(),
    payload: { commandId: "host-thread-1", method: "thread/start", ok: true, result: { thread: { id: "thread-on-host" } } },
  }));
  const result = await browser.wait((message) => message.kind === "event" && message.type === "command.result" && message.payload.commandId === "host-thread-1");
  assert.equal(result.payload.ok, true);
  assert.equal(relay.state.activeThreadId, "thread-on-host");

  host.ws.send(JSON.stringify({
    v: 1,
    kind: "event",
    type: "approval.requested",
    id: "host-approval-1",
    sessionId: host.sessionId,
    seq: 4,
    ts: new Date().toISOString(),
    payload: {
      requestId: 77,
      method: "item/commandExecution/requestApproval",
      commandHash: "approval-hash-77",
      params: { command: "echo approved" },
    },
  }));
  const approval = await browser.wait((message) => message.kind === "event" && message.type === "approval.requested" && message.payload.requestId === 77);
  assert.equal(approval.payload.params.command, "echo approved");
  browser.ws.send(JSON.stringify({ type: "respond", requestId: "77", result: { decision: "accept" } }));
  const responseCommand = await host.wait((message) => message.kind === "command" && message.type === "approval.respond");
  assert.equal(responseCommand.payload.requestId, 77);
  assert.equal(responseCommand.payload.decision, "allow");
  assert.equal(responseCommand.payload.commandHash, "approval-hash-77");
  assert.deepEqual(responseCommand.payload.response, { decision: "accept" });
  host.ws.send(JSON.stringify({
    v: 1,
    kind: "event",
    type: "command.accepted",
    id: "host-response-result-1",
    sessionId: host.sessionId,
    seq: 5,
    ts: new Date().toISOString(),
    payload: { commandId: responseCommand.commandId, method: "approval.respond", ok: true, result: { accepted: true } },
  }));
  await browser.wait((message) => message.kind === "event" && message.type === "server.responded" && String(message.payload.requestId) === "77");
  assert.equal(relay.pendingServerRequests.size, 0);

  browser.ws.send(JSON.stringify({ type: "respond", requestId: "77", result: { decision: "accept" } }));
  const duplicate = await browser.wait((message) => message.type === "response.rejected" && String(message.requestId) === "77");
  assert.equal(duplicate.code, "unknown_request");

  host.ws.send(JSON.stringify({
    v: 1,
    kind: "event",
    type: "approval.requested",
    id: "host-approval-2",
    sessionId: host.sessionId,
    seq: 6,
    ts: new Date().toISOString(),
    payload: { requestId: 78, method: "item/commandExecution/requestApproval", params: { command: "echo retry" } },
  }));
  await browser.wait((message) => message.kind === "event" && message.type === "approval.requested" && message.payload.requestId === 78);
  browser.ws.send(JSON.stringify({ type: "respond", requestId: "78", result: { decision: "accept" } }));
  const rejectedCommand = await host.wait((message) => message.kind === "command" && message.type === "approval.respond" && message.payload.requestId === 78);
  assert.equal(rejectedCommand.payload.decision, "allow");
  host.ws.send(JSON.stringify({
    v: 1,
    kind: "event",
    type: "command.rejected",
    id: "host-response-rejected-1",
    sessionId: host.sessionId,
    seq: 7,
    ts: new Date().toISOString(),
    payload: { commandId: rejectedCommand.commandId, method: "approval.respond", ok: false, error: { message: "local policy denied" } },
  }));
  const responseRejected = await browser.wait((message) => message.type === "response.rejected" && String(message.requestId) === "78");
  assert.equal(responseRejected.code, "host_rejected");
  assert.equal([...relay.pendingServerRequests.values()].some((pending) => String(pending.appId) === "78"), true);
});

test("host mode forwards session list/select and publishes the selected session", async (t) => {
  const relay = new CodexRelay({
    host: "127.0.0.1",
    port: 0,
    mode: "host",
    operatorToken: "operator-test-token",
    viewerToken: "viewer-test-token",
    hostToken: "separate-host-token",
  });
  await relay.start();
  t.after(() => relay.stop());
  const address = relay.address();
  const base = `http://127.0.0.1:${address.port}`;
  const host = await connectHost(base, "separate-host-token", "session-picker-host");
  const browser = await connectWs(base, "operator-test-token");
  t.after(() => host.ws.close());
  t.after(() => browser.ws.close());

  const sendEvent = (type, seq, payload) => host.ws.send(JSON.stringify({
    v: 1,
    kind: "event",
    type,
    id: `session-picker-event-${seq}`,
    sessionId: host.sessionId,
    seq,
    ts: new Date().toISOString(),
    payload,
  }));

  sendEvent("connection.opened", 1, { mode: "attach" });
  await browser.wait((message) => message.kind === "event" && message.type === "connection.opened");
  sendEvent("session.snapshot", 2, {
    threadId: "thread-one",
    activeThreadId: "thread-one",
    title: "当前会话",
    metadata: {
      controlMode: "sync",
      modeEpoch: 0,
      capabilities: {
        followsVscodeRoute: true,
        sessionList: false,
        sessionSelect: false,
        sessionCreate: false,
        threadSettings: true,
      },
    },
  });
  await browser.wait((message) => message.kind === "event" && message.type === "session.snapshot");
  assert.equal(relay.snapshot().metadata.controlMode, "sync");
  assert.equal(relay.snapshot().metadata.capabilities.sessionSelect, false);

  browser.ws.send(JSON.stringify({
    type: "command",
    commandId: "session-list-1",
    method: "session/list",
    params: { limit: 10 },
  }));
  const listCommand = await host.wait((message) => message.kind === "command" && message.commandId === "session-list-1");
  assert.equal(listCommand.type, "session/list");
  assert.equal(listCommand.payload.limit, 10);
  sendEvent("command.result", 3, {
    commandId: "session-list-1",
    method: "session/list",
    ok: true,
    result: {
      activeThreadId: "thread-one",
      sessions: [
        { threadId: "thread-one", title: "当前会话", active: true, available: true },
        { threadId: "thread-two", title: "另一个会话", active: false, available: true },
      ],
    },
  });
  const listResult = await browser.wait((message) => message.kind === "event"
    && message.type === "command.result" && message.payload?.commandId === "session-list-1");
  assert.equal(listResult.payload.result.sessions.length, 2);

  browser.ws.send(JSON.stringify({
    type: "command",
    commandId: "session-select-1",
    method: "session/select",
    params: { threadId: "thread-two" },
  }));
  const selectCommand = await host.wait((message) => message.kind === "command" && message.commandId === "session-select-1");
  assert.equal(selectCommand.type, "session/select");
  assert.equal(selectCommand.payload.threadId, "thread-two");
  sendEvent("session.switching", 4, { previousThreadId: "thread-one", targetThreadId: "thread-two" });
  sendEvent("session.snapshot", 5, { threadId: "thread-two", activeThreadId: "thread-two", title: "另一个会话" });
  sendEvent("session.selected", 6, { threadId: "thread-two", activeThreadId: "thread-two" });
  sendEvent("command.result", 7, {
    commandId: "session-select-1",
    method: "session/select",
    ok: true,
    result: { threadId: "thread-two", previousThreadId: "thread-one", switched: true, available: true },
  });
  await browser.wait((message) => message.kind === "event" && message.type === "session.switching");
  await browser.wait((message) => message.kind === "event" && message.type === "session.selected");
  const selectResult = await browser.wait((message) => message.kind === "event"
    && message.type === "command.result" && message.payload?.commandId === "session-select-1");
  assert.equal(selectResult.payload.result.threadId, "thread-two");
  assert.equal(relay.state.activeThreadId, "thread-two");

  browser.ws.send(JSON.stringify({
    type: "command",
    commandId: "session-new-1",
    method: "session/new",
    params: {},
  }));
  const newCommand = await host.wait((message) => message.kind === "command" && message.commandId === "session-new-1");
  assert.equal(newCommand.type, "session/new");
  sendEvent("command.result", 8, {
    commandId: "session-new-1",
    method: "session/new",
    ok: true,
    result: { opened: true, command: "chatgpt.newCodexPanel" },
  });
  const newResult = await browser.wait((message) => message.kind === "event"
    && message.type === "command.result" && message.payload?.commandId === "session-new-1");
  assert.equal(newResult.payload.result.command, "chatgpt.newCodexPanel");

  browser.ws.send(JSON.stringify({
    type: "command",
    commandId: "mode-set-1",
    method: "control/mode/set",
    params: { mode: "async" },
  }));
  const modeCommand = await host.wait((message) => message.kind === "command" && message.commandId === "mode-set-1");
  assert.equal(modeCommand.type, "control/mode/set");
  assert.deepEqual(modeCommand.payload, { mode: "async" });
  sendEvent("command.result", 9, {
    commandId: "mode-set-1",
    method: "control/mode/set",
    ok: true,
    result: { mode: "async", modeEpoch: 1 },
  });
  const modeResult = await browser.wait((message) => message.kind === "event"
    && message.type === "command.result" && message.payload?.commandId === "mode-set-1");
  assert.equal(modeResult.payload.result.mode, "async");
});

test("host mode records normalized server.requested events for response routing", async (t) => {
  const relay = new CodexRelay({
    host: "127.0.0.1",
    port: 0,
    mode: "host",
    operatorToken: "operator-test-token",
    viewerToken: "viewer-test-token",
    hostToken: "separate-host-token",
  });
  await relay.start();
  t.after(() => relay.stop());
  const address = relay.address();
  const base = `http://127.0.0.1:${address.port}`;
  const host = await connectHost(base, "separate-host-token", "generic-request-host");
  const browser = await connectWs(base, "operator-test-token");
  t.after(() => host.ws.close());
  t.after(() => browser.ws.close());

  host.ws.send(JSON.stringify({
    v: 1,
    kind: "event",
    type: "connection.opened",
    id: "generic-ready",
    sessionId: host.sessionId,
    seq: 1,
    ts: new Date().toISOString(),
    payload: {},
  }));
  await browser.wait((message) => message.kind === "event" && message.type === "connection.opened");

  host.ws.send(JSON.stringify({
    v: 1,
    kind: "event",
    type: "server.requested",
    id: "generic-request",
    sessionId: host.sessionId,
    seq: 2,
    ts: new Date().toISOString(),
    payload: {
      requestId: "generic-1",
      method: "custom/request",
      params: { prompt: "host-only" },
    },
  }));
  const requestEvent = await browser.wait((message) => message.kind === "event" && message.type === "server.requested");
  assert.equal(requestEvent.payload.requestId, "generic-1");
  assert.equal(relay.pendingServerRequests.get("string:generic-1")?.method, "custom/request");

  browser.ws.send(JSON.stringify({ type: "respond", requestId: "generic-1", result: { accepted: true } }));
  const rejected = await browser.wait((message) => message.type === "response.rejected" && message.requestId === "generic-1");
  assert.equal(rejected.code, "unsupported_request");
  assert.equal(relay.pendingServerRequests.has("string:generic-1"), true);
});

test("host mode accepts versioned command, approval, and input response frames", async (t) => {
  const relay = new CodexRelay({
    host: "127.0.0.1",
    port: 0,
    mode: "host",
    operatorToken: "operator-test-token",
    viewerToken: "viewer-test-token",
    hostToken: "separate-host-token",
  });
  await relay.start();
  t.after(() => relay.stop());
  const address = relay.address();
  const base = `http://127.0.0.1:${address.port}`;
  const host = await connectHost(base, "separate-host-token", "versioned-host-session");
  const browser = await connectWs(base, "operator-test-token");
  t.after(() => host.ws.close());
  t.after(() => browser.ws.close());

  host.ws.send(JSON.stringify({
    v: 1,
    kind: "event",
    type: "connection.opened",
    id: "versioned-ready",
    sessionId: host.sessionId,
    seq: 1,
    ts: new Date().toISOString(),
    payload: {},
  }));
  await browser.wait((message) => message.kind === "event" && message.type === "connection.opened");

  browser.ws.send(JSON.stringify({
    v: 1,
    kind: "command",
    type: "thread.start",
    commandId: "versioned-thread-1",
    payload: { cwd: "/tmp", sandbox: "workspace-write" },
  }));
  const threadCommand = await host.wait((message) => message.kind === "command" && message.commandId === "versioned-thread-1");
  assert.equal(threadCommand.type, "thread/start");
  assert.deepEqual(threadCommand.payload, { cwd: "/tmp", sandbox: "workspace-write" });

  host.ws.send(JSON.stringify({
    v: 1,
    kind: "event",
    type: "approval.requested",
    id: "versioned-approval-request",
    sessionId: host.sessionId,
    seq: 2,
    ts: new Date().toISOString(),
    payload: {
      requestId: 91,
      method: "item/commandExecution/requestApproval",
      commandHash: "versioned-hash-91",
      params: { command: "echo versioned" },
    },
  }));
  await browser.wait((message) => message.kind === "event" && message.type === "approval.requested" && message.payload.requestId === 91);
  browser.ws.send(JSON.stringify({
    v: 1,
    kind: "command",
    type: "approval.respond",
    commandId: "browser-approval-91",
    payload: { requestId: 91, decision: "allow", response: { decision: "accept" } },
  }));
  const approvalCommand = await host.wait((message) => message.kind === "command" && message.type === "approval.respond" && message.payload.requestId === 91);
  assert.equal(approvalCommand.payload.commandHash, "versioned-hash-91");
  assert.equal(approvalCommand.payload.decision, "allow");
  assert.deepEqual(approvalCommand.payload.response, { decision: "accept" });

  host.ws.send(JSON.stringify({
    v: 1,
    kind: "event",
    type: "input.requested",
    id: "versioned-input-request",
    sessionId: host.sessionId,
    seq: 3,
    ts: new Date().toISOString(),
    payload: {
      requestId: 92,
      method: "item/tool/requestUserInput",
      params: { questions: [{ id: "choice", question: "Continue?" }] },
    },
  }));
  await browser.wait((message) => message.kind === "event" && message.type === "input.requested" && message.payload.requestId === 92);
  browser.ws.send(JSON.stringify({
    type: "input.respond",
    payload: { requestId: 92, answers: { choice: { answers: ["yes"] } } },
  }));
  const inputCommand = await host.wait((message) => message.kind === "command" && message.type === "server.request.respond" && message.payload.requestId === 92);
  assert.equal(inputCommand.payload.decision, "allow");
  assert.deepEqual(inputCommand.payload.response, { answers: { choice: { answers: ["yes"] } } });

  host.ws.send(JSON.stringify({
    v: 1,
    kind: "event",
    type: "approval.requested",
    id: "versioned-tagged-approval",
    sessionId: host.sessionId,
    seq: 4,
    ts: new Date().toISOString(),
    payload: {
      requestId: 93,
      method: "item/commandExecution/requestApproval",
      params: { command: "echo amend" },
    },
  }));
  await browser.wait((message) => message.kind === "event" && message.type === "approval.requested" && message.payload.requestId === 93);
  browser.ws.send(JSON.stringify({
    v: 1,
    kind: "command",
    type: "approval.respond",
    commandId: "browser-tagged-approval-93",
    payload: {
      requestId: 93,
      decision: "allow",
      response: { decision: { acceptWithExecpolicyAmendment: { execpolicy_amendment: ["echo"] } } },
    },
  }));
  const taggedCommand = await host.wait((message) => message.kind === "command" && message.type === "approval.respond" && message.payload.requestId === 93);
  assert.equal(taggedCommand.payload.decision, "allow");
  assert.deepEqual(taggedCommand.payload.response, {
    decision: { acceptWithExecpolicyAmendment: { execpolicy_amendment: ["echo"] } },
  });
});

test("keeps numeric and string host approval ids distinct end to end", async (t) => {
  const relay = new CodexRelay({
    host: "127.0.0.1",
    port: 0,
    mode: "host",
    operatorToken: "operator-test-token",
    viewerToken: "viewer-test-token",
    hostToken: "separate-host-token",
  });
  await relay.start();
  t.after(() => relay.stop());
  const address = relay.address();
  const base = `http://127.0.0.1:${address.port}`;
  const host = await connectHost(base, "separate-host-token", "typed-id-session");
  const browser = await connectWs(base, "operator-test-token");
  t.after(() => host.ws.close());
  t.after(() => browser.ws.close());

  const sendHostEvent = (type, seq, payload, id) => host.ws.send(JSON.stringify({
    v: 1,
    kind: "event",
    type,
    id: id || `typed-${seq}`,
    sessionId: host.sessionId,
    seq,
    ts: new Date().toISOString(),
    payload,
  }));
  sendHostEvent("connection.opened", 1, {});
  await browser.wait((message) => message.kind === "event" && message.type === "connection.opened");

  sendHostEvent("approval.requested", 2, {
    requestId: 1,
    method: "item/commandExecution/requestApproval",
    params: { command: "echo numeric" },
  });
  sendHostEvent("approval.requested", 3, {
    requestId: "1",
    method: "item/commandExecution/requestApproval",
    params: { command: "echo string" },
  });
  await browser.wait((message) => message.kind === "event"
    && message.type === "approval.requested"
    && message.payload?.requestId === 1);
  await browser.wait((message) => message.kind === "event"
    && message.type === "approval.requested"
    && message.payload?.requestId === "1");
  assert.equal(relay.pendingServerRequests.size, 2);

  browser.ws.send(JSON.stringify({ type: "respond", requestId: 1, result: { decision: "accept" } }));
  browser.ws.send(JSON.stringify({ type: "respond", requestId: "1", result: { decision: "accept" } }));
  const firstCommand = await host.wait((message) => message.kind === "command"
    && message.type === "approval.respond"
    && message.payload?.requestId === 1);
  const secondCommand = await host.wait((message) => message.kind === "command"
    && message.type === "approval.respond"
    && message.payload?.requestId === "1");
  assert.notEqual(firstCommand.commandId, secondCommand.commandId);

  sendHostEvent("command.result", 4, {
    commandId: firstCommand.commandId,
    method: "approval.respond",
    ok: true,
    result: { accepted: true },
  });
  sendHostEvent("command.result", 5, {
    commandId: secondCommand.commandId,
    method: "approval.respond",
    ok: true,
    result: { accepted: true },
  });
  await browser.wait((message) => message.kind === "event"
    && message.type === "server.responded"
    && message.payload?.requestId === 1);
  await browser.wait((message) => message.kind === "event"
    && message.type === "server.responded"
    && message.payload?.requestId === "1");
  assert.equal(relay.pendingServerRequests.size, 0);
});

test("normalizes legacy approval decisions and rejects outer/inner conflicts", async (t) => {
  const relay = new CodexRelay({
    host: "127.0.0.1",
    port: 0,
    mode: "host",
    operatorToken: "operator-test-token",
    viewerToken: "viewer-test-token",
    hostToken: "separate-host-token",
  });
  await relay.start();
  t.after(() => relay.stop());
  const address = relay.address();
  const base = `http://127.0.0.1:${address.port}`;
  const host = await connectHost(base, "separate-host-token", "decision-schema-session");
  const browser = await connectWs(base, "operator-test-token");
  t.after(() => host.ws.close());
  t.after(() => browser.ws.close());
  const sendHostEvent = (type, seq, payload) => host.ws.send(JSON.stringify({
    v: 1,
    kind: "event",
    type,
    id: `decision-${seq}`,
    sessionId: host.sessionId,
    seq,
    ts: new Date().toISOString(),
    payload,
  }));
  sendHostEvent("connection.opened", 1, {});
  await browser.wait((message) => message.kind === "event" && message.type === "connection.opened");

  sendHostEvent("approval.requested", 2, {
    requestId: 201,
    method: "applyPatchApproval",
    params: { reason: "legacy patch" },
  });
  await browser.wait((message) => message.kind === "event"
    && message.type === "approval.requested"
    && message.payload?.requestId === 201);
  browser.ws.send(JSON.stringify({
    v: 1,
    kind: "command",
    type: "approval.respond",
    commandId: "legacy-approval-201",
    payload: { requestId: 201, decision: "approved" },
  }));
  const legacyCommand = await host.wait((message) => message.kind === "command"
    && message.type === "approval.respond"
    && message.payload?.requestId === 201);
  assert.deepEqual(legacyCommand.payload.response, { decision: "approved" });

  sendHostEvent("approval.requested", 3, {
    requestId: 202,
    method: "item/commandExecution/requestApproval",
    params: { command: "echo conflict" },
  });
  await browser.wait((message) => message.kind === "event"
    && message.type === "approval.requested"
    && message.payload?.requestId === 202);
  browser.ws.send(JSON.stringify({
    v: 1,
    kind: "command",
    type: "approval.respond",
    commandId: "conflicting-approval-202",
    payload: {
      requestId: 202,
      decision: "deny",
      response: { decision: "accept" },
    },
  }));
  const rejected = await browser.wait((message) => message.type === "response.rejected"
    && message.requestId === 202);
  assert.equal(rejected.code, "decision_mismatch");
  assert.equal(relay.pendingServerRequests.size, 2);

  sendHostEvent("approval.requested", 4, {
    requestId: 203,
    method: "item/commandExecution/requestApproval",
    params: { command: "echo mixed-tag" },
  });
  await browser.wait((message) => message.kind === "event"
    && message.type === "approval.requested"
    && message.payload?.requestId === 203);
  browser.ws.send(JSON.stringify({
    v: 1,
    kind: "command",
    type: "approval.respond",
    commandId: "mixed-tag-approval-203",
    payload: {
      requestId: 203,
      decision: "allow",
      response: {
        decision: {
          acceptWithExecpolicyAmendment: { execpolicy_amendment: ["echo"] },
          futurePolicyGrant: { scope: "all" },
        },
      },
    },
  }));
  const mixedRejected = await browser.wait((message) => message.type === "response.rejected"
    && message.requestId === 203);
  assert.equal(mixedRejected.code, "invalid_response");
  assert.equal(relay.pendingServerRequests.size, 3);
});

test("host command result cache is unavailable offline and isolated by host session", async (t) => {
  const relay = new CodexRelay({
    host: "127.0.0.1",
    port: 0,
    mode: "host",
    operatorToken: "operator-test-token",
    viewerToken: "viewer-test-token",
    hostToken: "separate-host-token",
  });
  await relay.start();
  t.after(() => relay.stop());
  const address = relay.address();
  const base = `http://127.0.0.1:${address.port}`;

  const sendReady = (host, seq, id) => host.ws.send(JSON.stringify({
    v: 1,
    kind: "event",
    type: "connection.opened",
    id,
    sessionId: host.sessionId,
    seq,
    ts: new Date().toISOString(),
    payload: {},
  }));
  const sendCommandResult = (host, seq, id, commandId, threadId) => host.ws.send(JSON.stringify({
    v: 1,
    kind: "event",
    type: "command.result",
    id,
    sessionId: host.sessionId,
    seq,
    ts: new Date().toISOString(),
    payload: {
      commandId,
      method: "thread/start",
      ok: true,
      result: { thread: { id: threadId } },
    },
  }));

  const host1 = await connectHost(base, "separate-host-token", "host-session-1");
  const browser = await connectWs(base, "operator-test-token");
  t.after(() => host1.ws.close());
  t.after(() => browser.ws.close());
  sendReady(host1, 1, "host1-ready");
  await browser.wait((message) => message.kind === "event" && message.type === "connection.opened");

  const commandId = "reused-command-id";
  browser.ws.send(JSON.stringify({
    type: "command",
    commandId,
    method: "thread/start",
    params: { cwd: "/tmp", sandbox: "workspace-write" },
  }));
  await host1.wait((message) => message.kind === "command" && message.commandId === commandId);
  sendCommandResult(host1, 2, "host1-command-result", commandId, "thread-from-session-1");
  const firstResult = await browser.wait((message) => message.kind === "event"
    && message.type === "command.result"
    && message.payload?.commandId === commandId
    && message.payload?.result?.thread?.id === "thread-from-session-1");
  assert.equal(firstResult.payload.ok, true);

  const uncertainCommandId = "uncertain-command-id";
  browser.ws.send(JSON.stringify({
    type: "command",
    commandId: uncertainCommandId,
    method: "thread/start",
    params: { cwd: "/tmp", sandbox: "workspace-write" },
  }));
  await host1.wait((message) => message.kind === "command" && message.commandId === uncertainCommandId);

  host1.ws.close();
  await waitFor(() => relay.hostClient === null);
  await browser.wait((message) => message.type === "command.result"
    && message.commandId === uncertainCommandId
    && message.uncertain === true);

  // A stale success (or uncertain disconnect result) must not be replayed
  // while no host is connected.
  browser.ws.send(JSON.stringify({
    type: "command",
    commandId,
    method: "thread/start",
    params: { cwd: "/tmp", sandbox: "workspace-write" },
  }));
  const offline = await browser.wait((message) => message.type === "command.rejected" && message.commandId === commandId);
  assert.equal(offline.code, "app_not_ready");
  browser.ws.send(JSON.stringify({
    type: "command",
    commandId: uncertainCommandId,
    method: "thread/start",
    params: { cwd: "/tmp", sandbox: "workspace-write" },
  }));
  const uncertainOffline = await browser.wait((message) => message.type === "command.rejected" && message.commandId === uncertainCommandId);
  assert.equal(uncertainOffline.code, "app_not_ready");

  // A reconnect carrying the same stable session id retains normal command
  // idempotency and returns the cached result without forwarding a command.
  const sameSessionHost = await connectHost(base, "separate-host-token", "host-session-1");
  t.after(() => sameSessionHost.ws.close());
  sendReady(sameSessionHost, 1, "same-session-ready");
  await browser.wait((message) => message.kind === "event"
    && message.type === "connection.opened"
    && message.payload?.source === "vscode-host");
  browser.ws.send(JSON.stringify({
    type: "command",
    commandId,
    method: "thread/start",
    params: { cwd: "/tmp", sandbox: "workspace-write" },
  }));
  const replay = await browser.wait((message) => message.type === "command.result"
    && message.cached === true
    && message.commandId === commandId);
  assert.equal(replay.result.thread.id, "thread-from-session-1");
  await new Promise((resolve) => setTimeout(resolve, 50));
  assert.equal(sameSessionHost.messages.some((message) => message.kind === "command" && message.commandId === commandId), false);

  browser.ws.send(JSON.stringify({
    type: "command",
    commandId: uncertainCommandId,
    method: "thread/start",
    params: { cwd: "/tmp", sandbox: "workspace-write" },
  }));
  await sameSessionHost.wait((message) => message.kind === "command" && message.commandId === uncertainCommandId);
  sendCommandResult(sameSessionHost, 2, "same-session-uncertain-result", uncertainCommandId, "thread-after-uncertain");
  const uncertainRetry = await browser.wait((message) => message.kind === "event"
    && message.type === "command.result"
    && message.payload?.commandId === uncertainCommandId
    && message.payload?.result?.thread?.id === "thread-after-uncertain");
  assert.equal(uncertainRetry.payload.ok, true);

  sameSessionHost.ws.close();
  await waitFor(() => relay.hostClient === null);

  // A different host session cannot reuse the old command id; it must receive
  // a fresh command even though the browser retries the same id.
  const host2 = await connectHost(base, "separate-host-token", "host-session-2");
  t.after(() => host2.ws.close());
  sendReady(host2, 1, "host2-ready");
  await browser.wait((message) => message.kind === "event"
    && message.type === "connection.opened"
    && message.payload?.source === "vscode-host");
  browser.ws.send(JSON.stringify({
    type: "command",
    commandId,
    method: "thread/start",
    params: { cwd: "/tmp", sandbox: "workspace-write" },
  }));
  const forwarded = await host2.wait((message) => message.kind === "command" && message.commandId === commandId);
  assert.equal(forwarded.type, "thread/start");
  sendCommandResult(host2, 2, "host2-command-result", commandId, "thread-from-session-2");
  const secondResult = await browser.wait((message) => message.kind === "event"
    && message.type === "command.result"
    && message.payload?.commandId === commandId
    && message.payload?.result?.thread?.id === "thread-from-session-2");
  assert.equal(secondResult.payload.ok, true);
});

test("rejects non-object websocket frames without taking down the relay", async (t) => {
  const relay = new CodexRelay({
    host: "127.0.0.1",
    port: 0,
    mode: "embedded",
    operatorToken: "operator-test-token",
    viewerToken: "viewer-test-token",
    codexCommand: process.execPath,
    codexArgs: [fakeServer],
  });
  await relay.start();
  t.after(() => relay.stop());
  const address = relay.address();
  const ws = new WebSocket(`ws://127.0.0.1:${address.port}/ws`);
  t.after(() => ws.close());
  const invalidFrame = new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error("timed out waiting for invalid frame response")), 5_000);
    ws.once("error", reject);
    ws.on("message", (data) => {
      const message = JSON.parse(data.toString());
      if (message.code === "invalid_frame") {
        clearTimeout(timer);
        resolve(message);
      }
    });
  });
  await new Promise((resolve, reject) => {
    ws.once("open", resolve);
    ws.once("error", reject);
  });
  ws.send("null");
  await invalidFrame;
  const health = await fetch(`http://127.0.0.1:${address.port}/api/health`);
  assert.equal(health.status, 200);
});

test("malformed percent escapes return 400 without crashing the relay", async (t) => {
  const relay = new CodexRelay({
    host: "127.0.0.1",
    port: 0,
    mode: "host",
    operatorToken: "operator-test-token",
    viewerToken: "viewer-test-token",
    hostToken: "separate-host-token",
  });
  await relay.start();
  t.after(() => relay.stop());
  const address = relay.address();
  const base = `http://127.0.0.1:${address.port}`;

  const malformed = await fetch(`${base}/%ZZ`);
  assert.equal(malformed.status, 400);
  assert.equal(await malformed.text(), "Invalid URL");

  const health = await fetch(`${base}/api/health`);
  assert.equal(health.status, 200);
});

test("cleans embedded app pending commands and approvals when the child exits", async (t) => {
  const crashServer = [
    "const readline = require('node:readline');",
    "const input = readline.createInterface({ input: process.stdin });",
    "const send = (message) => process.stdout.write(JSON.stringify(message) + '\\n');",
    "input.on('line', (line) => {",
    "  let request; try { request = JSON.parse(line); } catch { return; }",
    "  if (request.method === 'initialize') { send({ id: request.id, result: { userAgent: 'crash-test' } }); return; }",
    "  if (request.method === 'thread/start') { send({ id: request.id, result: { thread: { id: 'thread-crash' }, cwd: '/tmp' } }); return; }",
    "  if (request.method === 'turn/start') {",
    "    send({ id: 4321, method: 'item/commandExecution/requestApproval', params: { command: 'echo crash' } });",
    "    setTimeout(() => process.exit(23), 30);",
    "  }",
    "});",
  ].join("\n");
  const relay = new CodexRelay({
    host: "127.0.0.1",
    port: 0,
    mode: "embedded",
    operatorToken: "operator-test-token",
    viewerToken: "viewer-test-token",
    codexCommand: process.execPath,
    codexArgs: ["-e", crashServer],
  });
  await relay.start();
  t.after(() => relay.stop());
  const address = relay.address();
  const base = `http://127.0.0.1:${address.port}`;
  const browser = await connectWs(base, "operator-test-token");
  t.after(() => browser.ws.close());
  await browser.wait((message) => message.type === "session.snapshot");
  await waitFor(() => relay.state.initialized === true);

  browser.ws.send(JSON.stringify({
    type: "command",
    commandId: "crash-thread",
    method: "thread/start",
    params: { cwd: "/tmp", sandbox: "workspace-write" },
  }));
  const threadResult = await browser.wait((message) => message.type === "command.result"
    && message.payload?.commandId === "crash-thread");
  assert.equal(threadResult.payload.ok, true);

  browser.ws.send(JSON.stringify({
    type: "command",
    commandId: "crash-turn",
    method: "turn/start",
    params: { threadId: "thread-crash", input: [{ type: "text", text: "crash" }] },
  }));
  await browser.wait((message) => message.type === "command.accepted" && message.commandId === "crash-turn");
  const approval = await browser.wait((message) => message.kind === "event"
    && message.type === "approval.requested"
    && String(message.payload?.requestId) === "4321");
  assert.equal(approval.payload.method, "item/commandExecution/requestApproval");
  assert.equal([...relay.pendingServerRequests.values()].some((pending) => String(pending.appId) === "4321"), true);

  const uncertain = await browser.wait((message) => message.type === "command.result"
    && message.commandId === "crash-turn"
    && message.uncertain === true);
  assert.equal(uncertain.retryable, true);
  await browser.wait((message) => message.kind === "event"
    && message.type === "approval.expired"
    && String(message.payload?.requestId) === "4321");
  await waitFor(() => relay.state.app === "offline");
  assert.equal(relay.state.initialized, false);
  assert.equal(relay.pendingAppRequests.size, 0);
  assert.equal(relay.pendingServerRequests.size, 0);
  assert.equal(relay.appProcess, null);
  assert.throws(() => relay.sendToApp({ method: "ping" }), (error) => error.code === "app_offline");

  // The uncertain marker is deliberately not replayed. A retry while the app
  // is offline receives the normal readiness error instead of a duplicate.
  browser.ws.send(JSON.stringify({
    type: "command",
    commandId: "crash-turn",
    method: "turn/start",
    params: { threadId: "thread-crash", input: [{ type: "text", text: "retry" }] },
  }));
  const rejected = await browser.wait((message) => message.type === "command.rejected"
    && message.commandId === "crash-turn");
  assert.equal(rejected.code, "app_not_ready");
});

test("cleans host pending work on app connection.closed and honors expiry events", async (t) => {
  const relay = new CodexRelay({
    host: "127.0.0.1",
    port: 0,
    mode: "host",
    operatorToken: "operator-test-token",
    viewerToken: "viewer-test-token",
    hostToken: "separate-host-token",
  });
  await relay.start();
  t.after(() => relay.stop());
  const address = relay.address();
  const base = `http://127.0.0.1:${address.port}`;
  const host = await connectHost(base, "separate-host-token", "cleanup-host-session");
  const browser = await connectWs(base, "operator-test-token");
  t.after(() => host.ws.close());
  t.after(() => browser.ws.close());

  const sendHostEvent = (type, seq, payload, id = `cleanup-${seq}`) => host.ws.send(JSON.stringify({
    v: 1,
    kind: "event",
    type,
    id,
    sessionId: host.sessionId,
    seq,
    ts: new Date().toISOString(),
    payload,
  }));

  sendHostEvent("connection.opened", 1, {});
  await browser.wait((message) => message.kind === "event" && message.type === "connection.opened");

  // Complete one command first so the app-unavailable transition has a cache
  // entry to invalidate as well as an in-flight command to settle.
  browser.ws.send(JSON.stringify({
    type: "command",
    commandId: "host-completed-before-exit",
    method: "thread/start",
    params: { cwd: "/tmp", sandbox: "workspace-write" },
  }));
  await host.wait((message) => message.kind === "command" && message.commandId === "host-completed-before-exit");
  sendHostEvent("command.result", 2, {
    commandId: "host-completed-before-exit",
    method: "thread/start",
    ok: true,
    result: { thread: { id: "host-thread-cleanup" } },
  });
  await browser.wait((message) => message.kind === "event"
    && message.type === "command.result"
    && message.payload?.commandId === "host-completed-before-exit");
  assert.equal(relay.commandResults.has("host-completed-before-exit"), true);

  browser.ws.send(JSON.stringify({
    type: "command",
    commandId: "host-pending-before-exit",
    method: "turn/start",
    params: { threadId: "host-thread-cleanup", input: [{ type: "text", text: "pending" }] },
  }));
  await host.wait((message) => message.kind === "command" && message.commandId === "host-pending-before-exit");
  assert.equal(relay.pendingHostCommands.has("host-pending-before-exit"), true);

  sendHostEvent("approval.requested", 3, {
    requestId: 501,
    method: "item/commandExecution/requestApproval",
    params: { command: "echo pending approval" },
  });
  await browser.wait((message) => message.kind === "event"
    && message.type === "approval.requested"
    && message.payload?.requestId === 501);
  assert.equal([...relay.pendingServerRequests.values()].some((pending) => String(pending.appId) === "501"), true);

  sendHostEvent("approval.expired", 4, {
    requestId: 501,
    method: "item/commandExecution/requestApproval",
  });
  await browser.wait((message) => message.kind === "event"
    && message.type === "approval.expired"
    && message.payload?.requestId === 501);
  assert.equal([...relay.pendingServerRequests.values()].some((pending) => String(pending.appId) === "501"), false);

  sendHostEvent("connection.closed", 5, { message: "embedded app exited" });
  await browser.wait((message) => message.kind === "event" && message.type === "connection.closed");
  const uncertain = await browser.wait((message) => message.type === "command.result"
    && message.commandId === "host-pending-before-exit"
    && message.uncertain === true);
  assert.equal(uncertain.error.code, "app_unavailable");
  await waitFor(() => relay.state.app === "offline");
  assert.equal(relay.state.initialized, false);
  assert.equal(relay.state.hostConnected, true);
  assert.equal(relay.pendingHostCommands.size, 0);
  assert.equal(relay.pendingServerRequests.size, 0);
  assert.equal(relay.commandResults.size, 0);

  // The host transport remains connected, but commands are rejected until it
  // reports a fresh connection.opened/session snapshot.
  browser.ws.send(JSON.stringify({
    type: "command",
    commandId: "host-completed-before-exit",
    method: "thread/start",
    params: { cwd: "/tmp", sandbox: "workspace-write" },
  }));
  const rejected = await browser.wait((message) => message.type === "command.rejected"
    && message.commandId === "host-completed-before-exit");
  assert.equal(rejected.code, "app_not_ready");
});

test("ignores a stale host connection.closed frame after host replacement", async (t) => {
  const relay = new CodexRelay({
    host: "127.0.0.1",
    port: 0,
    mode: "host",
    operatorToken: "operator-test-token",
    viewerToken: "viewer-test-token",
    hostToken: "separate-host-token",
  });
  await relay.start();
  t.after(() => relay.stop());
  const address = relay.address();
  const base = `http://127.0.0.1:${address.port}`;
  const firstHost = await connectHost(base, "separate-host-token", "stale-session-1");
  t.after(() => firstHost.ws.close());
  firstHost.ws.send(JSON.stringify({
    v: 1,
    kind: "event",
    type: "connection.opened",
    sessionId: firstHost.sessionId,
    seq: 1,
    payload: {},
  }));
  await waitFor(() => relay.state.app === "ready");
  const staleClient = relay.hostClient;
  firstHost.ws.close();
  await waitFor(() => relay.hostClient === null);

  const secondHost = await connectHost(base, "separate-host-token", "stale-session-2");
  t.after(() => secondHost.ws.close());
  secondHost.ws.send(JSON.stringify({
    v: 1,
    kind: "event",
    type: "connection.opened",
    sessionId: secondHost.sessionId,
    seq: 1,
    payload: {},
  }));
  await waitFor(() => relay.state.app === "ready" && relay.hostClient?.sessionId === "stale-session-2");
  const sequenceBefore = relay.nextSeq;

  relay.ingestHostEvent(staleClient, {
    v: 1,
    kind: "event",
    type: "connection.closed",
    sessionId: "stale-session-1",
    seq: 99,
    payload: { message: "late old app exit" },
  });

  assert.equal(relay.state.app, "ready");
  assert.equal(relay.state.initialized, true);
  assert.equal(relay.state.hostConnected, true);
  assert.equal(relay.state.hostSessionId, "stale-session-2");
  assert.equal(relay.nextSeq, sequenceBefore);
});
