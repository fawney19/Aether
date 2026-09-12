"use strict";

const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const test = require("node:test");
const { WebSocket } = require("ws");

const { AetherVscodexCloudServer, RoomManager } = require("../cloud/server.js");

const internalToken = "test-internal-token-with-enough-entropy";

function internalFetch(base, pathname, options = {}) {
  return fetch(`${base}${pathname}`, {
    ...options,
    headers: {
      Authorization: `Bearer ${internalToken}`,
      ...(options.body ? { "Content-Type": "application/json" } : {}),
      ...(options.headers || {}),
    },
  });
}

function websocketClient(base, clientType, token, sessionId) {
  const socket = new WebSocket(`${base.replace(/^http/, "ws")}/api/vscodex/ws`);
  const messages = [];
  const waiters = [];
  const wait = (predicate, timeout = 5_000, label = "websocket frame") => new Promise((resolve, reject) => {
    const existing = messages.find(predicate);
    if (existing) return resolve(existing);
    const timer = setTimeout(() => {
      const index = waiters.findIndex((entry) => entry.resolve === resolve);
      if (index >= 0) waiters.splice(index, 1);
      reject(new Error(`timed out waiting for ${label}; received: ${JSON.stringify(messages.map((message) => ({ type: message.type, kind: message.kind, commandId: message.commandId })))}`));
    }, timeout);
    waiters.push({
      predicate,
      resolve: (message) => {
        clearTimeout(timer);
        resolve(message);
      },
    });
  });
  socket.on("message", (data) => {
    const message = JSON.parse(data.toString("utf8"));
    messages.push(message);
    for (let index = waiters.length - 1; index >= 0; index -= 1) {
      if (!waiters[index].predicate(message)) continue;
      const waiter = waiters.splice(index, 1)[0];
      waiter.resolve(message);
    }
  });
  return new Promise((resolve, reject) => {
    socket.once("open", () => {
      socket.send(JSON.stringify({ v: 1, kind: "hello", clientType, protocol: 1, ...(sessionId ? { sessionId } : {}) }));
      socket.send(JSON.stringify(clientType === "host"
        ? { v: 1, kind: "auth", accessToken: token }
        : { type: "auth", token }));
      wait((message) => message.type === "auth.ok").then(() => resolve({ socket, wait, messages }), reject);
    });
    socket.once("error", reject);
  });
}

async function pairDevice(base, userId, name) {
  const pairingResponse = await internalFetch(base, `/internal/v1/users/${encodeURIComponent(userId)}/pairings`, {
    method: "POST",
    body: JSON.stringify({ name }),
  });
  assert.equal(pairingResponse.status, 201);
  const pairing = await pairingResponse.json();
  const exchangeResponse = await fetch(`${base}/v1/pairings/exchange`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ code: pairing.code, name }),
  });
  assert.equal(exchangeResponse.status, 201);
  return exchangeResponse.json();
}

async function browserTicket(base, userId, deviceId) {
  const response = await internalFetch(base, `/internal/v1/users/${encodeURIComponent(userId)}/ws-tickets`, {
    method: "POST",
    body: JSON.stringify({ device_id: deviceId }),
  });
  assert.equal(response.status, 201);
  return response.json();
}

function exchangeAttempt(base, headers = {}) {
  return fetch(`${base}/v1/pairings/exchange`, {
    method: "POST",
    headers: { "Content-Type": "application/json", ...headers },
    body: JSON.stringify({ code: "INVALID-CODE" }),
  });
}

test("pairing exchange trusts a gateway client IP only with valid internal authentication", async (t) => {
  const dataDir = fs.mkdtempSync(path.join(os.tmpdir(), "aether-vscodex-rate-limit-"));
  const server = new AetherVscodexCloudServer({
    host: "127.0.0.1",
    port: 0,
    internalToken,
    publicWsUrl: "wss://aether.example/api/vscodex/ws",
    dataDir,
  });
  await server.start();
  t.after(async () => {
    await server.stop();
    fs.rmSync(dataDir, { recursive: true, force: true });
  });
  const address = server.address();
  const base = `http://127.0.0.1:${address.port}`;
  const trustedHeaders = (clientIp) => ({
    Authorization: `Bearer ${internalToken}`,
    "X-Aether-Client-IP": clientIp,
  });

  for (let attempt = 0; attempt < 10; attempt += 1) {
    assert.equal((await exchangeAttempt(base, trustedHeaders("198.51.100.10"))).status, 400);
  }
  assert.equal((await exchangeAttempt(base, trustedHeaders("198.51.100.10"))).status, 429);
  assert.equal((await exchangeAttempt(base, trustedHeaders("198.51.100.11"))).status, 400);
  assert.equal((await exchangeAttempt(base, trustedHeaders("2001:db8::10"))).status, 400);

  server.exchangeAttempts.clear();
  for (let attempt = 0; attempt < 5; attempt += 1) {
    assert.equal((await exchangeAttempt(base, { "X-Aether-Client-IP": `198.51.100.${20 + attempt}` })).status, 400);
  }
  for (let attempt = 0; attempt < 5; attempt += 1) {
    assert.equal((await exchangeAttempt(base, {
      Authorization: "Bearer invalid-internal-token",
      "X-Aether-Client-IP": `198.51.100.${30 + attempt}`,
    })).status, 400);
  }
  assert.equal((await exchangeAttempt(base, { "X-Aether-Client-IP": "198.51.100.99" })).status, 429);

  server.exchangeAttempts.clear();
  const invalidForwardedAddresses = ["proxy.internal", "198.51.100.40, 198.51.100.41"];
  for (let attempt = 0; attempt < 10; attempt += 1) {
    assert.equal((await exchangeAttempt(base, trustedHeaders(invalidForwardedAddresses[attempt % 2]))).status, 400);
  }
  assert.equal((await exchangeAttempt(base, trustedHeaders("198.51.100.42, 198.51.100.43"))).status, 429);
});

test("cloud sidecar pairs a device and isolates host/browser traffic by Aether user and device", async (t) => {
  const dataDir = fs.mkdtempSync(path.join(os.tmpdir(), "aether-vscodex-test-"));
  const server = new AetherVscodexCloudServer({
    host: "127.0.0.1",
    port: 0,
    internalToken,
    publicWsUrl: "wss://aether.example/api/vscodex/ws",
    dataDir,
    pairingTtlMs: 5_000,
    ticketTtlMs: 5_000,
  });
  await server.start();
  t.after(async () => {
    await server.stop();
    fs.rmSync(dataDir, { recursive: true, force: true });
  });
  const address = server.address();
  const base = `http://127.0.0.1:${address.port}`;

  const unauthorized = await fetch(`${base}/internal/v1/users/user-a/devices`);
  assert.equal(unauthorized.status, 401);

  const paired = await pairDevice(base, "user-a", "MacBook VS Code");
  assert.match(paired.device_token, /^avx1\./);
  const devicesResponse = await internalFetch(base, "/internal/v1/users/user-a/devices");
  assert.equal(devicesResponse.status, 200);
  const devices = await devicesResponse.json();
  assert.deepEqual(devices.devices.map((device) => ({ id: device.id, name: device.name, connected: device.connected })), [
    { id: paired.device_id, name: "MacBook VS Code", connected: false },
  ]);

  const ticket = await browserTicket(base, "user-a", paired.device_id);
  assert.equal(ticket.ws_url, "/api/vscodex/ws");
  const host = await websocketClient(base, "host", paired.device_token, "host-user-a");
  const browser = await websocketClient(base, "web", ticket.ticket);
  t.after(() => host.socket.close());
  t.after(() => browser.socket.close());
  browser.socket.send(JSON.stringify({ type: "subscribe", fromSeq: 0 }));

  host.socket.send(JSON.stringify({
    v: 1,
    kind: "event",
    type: "connection.opened",
    id: "connection-a",
    sessionId: "host-user-a",
    seq: 1,
    ts: new Date().toISOString(),
    payload: {},
  }));
  host.socket.send(JSON.stringify({
    v: 1,
    kind: "event",
    type: "session.snapshot",
    id: "snapshot-a",
    sessionId: "host-user-a",
    seq: 2,
    ts: new Date().toISOString(),
    payload: { threadId: "thread-a", state: "idle", messages: [{ kind: "assistant", text: "user-a-only" }] },
  }));
  const snapshot = await browser.wait((message) => message.kind === "event" && message.type === "session.snapshot", 5_000, "session snapshot");
  assert.equal(snapshot.payload.threadId, "thread-a");
  assert.equal(snapshot.payload.messages[0].text, "user-a-only");

  browser.socket.send(JSON.stringify({ type: "command", commandId: "cmd-a", method: "session/list", params: {} }));
  const command = await host.wait((message) => message.kind === "command" && message.commandId === "cmd-a", 5_000, "browser command");
  assert.equal(command.type, "session/list");

  const secondUser = await pairDevice(base, "user-b", "Other VS Code");
  const secondTicket = await browserTicket(base, "user-b", secondUser.device_id);
  const secondBrowser = await websocketClient(base, "web", secondTicket.ticket);
  t.after(() => secondBrowser.socket.close());
  secondBrowser.socket.send(JSON.stringify({ type: "subscribe", fromSeq: 0 }));
  await new Promise((resolve) => setTimeout(resolve, 50));
  assert.equal(secondBrowser.messages.some((message) => message.payload?.threadId === "thread-a"), false);

  const reusedTicket = new WebSocket(`${base.replace(/^http/, "ws")}/api/vscodex/ws`);
  const closed = new Promise((resolve, reject) => {
    reusedTicket.once("open", () => {
      reusedTicket.send(JSON.stringify({ v: 1, kind: "hello", clientType: "web", protocol: 1 }));
      reusedTicket.send(JSON.stringify({ type: "auth", token: ticket.ticket }));
    });
    reusedTicket.once("close", (code) => resolve(code));
    reusedTicket.once("error", reject);
  });
  assert.equal(await closed, 1008, "browser tickets are one-time credentials");
});

test("device revocation closes its room and blocks future host authentication", async (t) => {
  const dataDir = fs.mkdtempSync(path.join(os.tmpdir(), "aether-vscodex-revoke-"));
  const server = new AetherVscodexCloudServer({
    host: "127.0.0.1",
    port: 0,
    internalToken,
    publicWsUrl: "wss://aether.example/api/vscodex/ws",
    dataDir,
  });
  await server.start();
  t.after(async () => {
    await server.stop();
    fs.rmSync(dataDir, { recursive: true, force: true });
  });
  const address = server.address();
  const base = `http://127.0.0.1:${address.port}`;
  const paired = await pairDevice(base, "user-a", "Revoked device");
  const host = await websocketClient(base, "host", paired.device_token, "revoked-host");

  const response = await internalFetch(base, `/internal/v1/users/user-a/devices/${paired.device_id}`, { method: "DELETE" });
  assert.equal(response.status, 204);
  await new Promise((resolve) => host.socket.once("close", resolve));

  const rejected = new WebSocket(`${base.replace(/^http/, "ws")}/api/vscodex/ws`);
  const closed = new Promise((resolve, reject) => {
    rejected.once("open", () => {
      rejected.send(JSON.stringify({ v: 1, kind: "hello", clientType: "host", protocol: 1, sessionId: "retry" }));
      rejected.send(JSON.stringify({ v: 1, kind: "auth", accessToken: paired.device_token }));
    });
    rejected.once("close", (code) => resolve(code));
    rejected.once("error", reject);
  });
  assert.equal(await closed, 1008);
});

test("room revocation wins a concurrent room creation", async () => {
  const rooms = new RoomManager();
  let releaseCreation;
  const creationGate = new Promise((resolve) => { releaseCreation = resolve; });
  let stopped = false;
  const room = {
    key: rooms.key("user-a", "device-a"),
    userId: "user-a",
    deviceId: "device-a",
    relay: { stop: async () => { stopped = true; } },
    connections: 0,
    lastActiveMs: Date.now(),
  };
  rooms.createRoom = async (key) => {
    await creationGate;
    rooms.rooms.set(key, room);
    return room;
  };

  const pendingGet = rooms.get("user-a", "device-a");
  await new Promise((resolve) => setImmediate(resolve));
  const pendingRevoke = rooms.revoke("user-a", "device-a");
  releaseCreation();

  await assert.rejects(pendingGet, /device revoked/);
  await pendingRevoke;
  assert.equal(stopped, true);
  assert.equal(rooms.rooms.has(room.key), false);
  await assert.rejects(rooms.get("user-a", "device-a"), /device revoked/);
});
