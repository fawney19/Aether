"use strict";

const crypto = require("node:crypto");
const fs = require("node:fs");
const http = require("node:http");
const net = require("node:net");
const path = require("node:path");
const { URL } = require("node:url");
const { WebSocket, WebSocketServer } = require("ws");

const { CodexRelay } = require("../relay/server.js");

const MAX_JSON_BYTES = 64 * 1024;
const MAX_WS_BYTES = 16 * 1024 * 1024;
const DEFAULT_PAIRING_TTL_MS = 10 * 60 * 1000;
const DEFAULT_TICKET_TTL_MS = 60 * 1000;
const DEFAULT_ROOM_IDLE_MS = 30 * 60 * 1000;

class DeviceStore {
  constructor(filePath) {
    this.filePath = filePath;
    this.data = { version: 1, devices: [] };
    this.load();
  }

  load() {
    try {
      const parsed = JSON.parse(fs.readFileSync(this.filePath, "utf8"));
      if (parsed?.version !== 1 || !Array.isArray(parsed.devices)) throw new Error("unsupported device store format");
      this.data = parsed;
    } catch (error) {
      if (error?.code !== "ENOENT") throw error;
      fs.mkdirSync(path.dirname(this.filePath), { recursive: true, mode: 0o700 });
      this.persist();
    }
  }

  list(userId, connectedDeviceIds = new Set()) {
    return this.data.devices
      .filter((device) => device.user_id === userId && !device.revoked_at)
      .map((device) => publicDevice(device, connectedDeviceIds.has(device.id)));
  }

  create(userId, name) {
    const id = crypto.randomUUID();
    const secret = crypto.randomBytes(32).toString("base64url");
    const salt = crypto.randomBytes(16).toString("base64url");
    const now = new Date().toISOString();
    const device = {
      id,
      user_id: userId,
      name: normalizeName(name),
      secret_salt: salt,
      secret_hash: deriveSecret(secret, salt),
      created_at: now,
      last_seen_at: null,
      revoked_at: null,
    };
    this.data.devices.push(device);
    this.persist();
    return { device: publicDevice(device, false), token: `avx1.${id}.${secret}` };
  }

  authenticate(token) {
    const parsed = parseDeviceToken(token);
    if (!parsed) return null;
    const device = this.data.devices.find((candidate) => candidate.id === parsed.id && !candidate.revoked_at);
    if (!device) return null;
    const actual = Buffer.from(deriveSecret(parsed.secret, device.secret_salt), "base64url");
    const expected = Buffer.from(device.secret_hash, "base64url");
    if (actual.length !== expected.length || !crypto.timingSafeEqual(actual, expected)) return null;
    return device;
  }

  get(userId, deviceId) {
    return this.data.devices.find((device) => device.user_id === userId && device.id === deviceId && !device.revoked_at) || null;
  }

  touch(deviceId) {
    const device = this.data.devices.find((candidate) => candidate.id === deviceId && !candidate.revoked_at);
    if (!device) return;
    device.last_seen_at = new Date().toISOString();
    this.persist();
  }

  revoke(userId, deviceId) {
    const device = this.get(userId, deviceId);
    if (!device) return false;
    device.revoked_at = new Date().toISOString();
    this.persist();
    return true;
  }

  persist() {
    fs.mkdirSync(path.dirname(this.filePath), { recursive: true, mode: 0o700 });
    const temporary = `${this.filePath}.${process.pid}.${crypto.randomBytes(4).toString("hex")}.tmp`;
    fs.writeFileSync(temporary, `${JSON.stringify(this.data, null, 2)}\n`, { mode: 0o600 });
    fs.renameSync(temporary, this.filePath);
  }
}

class EphemeralCredentials {
  constructor(options = {}) {
    this.pairingTtlMs = options.pairingTtlMs || DEFAULT_PAIRING_TTL_MS;
    this.ticketTtlMs = options.ticketTtlMs || DEFAULT_TICKET_TTL_MS;
    this.pairings = new Map();
    this.tickets = new Map();
  }

  createPairing(userId, requestedName) {
    const code = pairingCode();
    const record = {
      id: crypto.randomUUID(),
      code,
      user_id: userId,
      requested_name: normalizeName(requestedName),
      expires_at_ms: Date.now() + this.pairingTtlMs,
    };
    this.pairings.set(normalizePairingCode(code), record);
    return record;
  }

  consumePairing(code) {
    const key = normalizePairingCode(code);
    const record = this.pairings.get(key);
    this.pairings.delete(key);
    if (!record || record.expires_at_ms <= Date.now()) return null;
    return record;
  }

  createTicket(userId, deviceId) {
    const ticket = `avt1.${crypto.randomBytes(32).toString("base64url")}`;
    this.tickets.set(ticket, {
      user_id: userId,
      device_id: deviceId,
      expires_at_ms: Date.now() + this.ticketTtlMs,
    });
    return ticket;
  }

  consumeTicket(ticket) {
    const record = this.tickets.get(ticket);
    this.tickets.delete(ticket);
    if (!record || record.expires_at_ms <= Date.now()) return null;
    return record;
  }

  cleanup() {
    const now = Date.now();
    for (const [key, record] of this.pairings) if (record.expires_at_ms <= now) this.pairings.delete(key);
    for (const [key, record] of this.tickets) if (record.expires_at_ms <= now) this.tickets.delete(key);
  }
}

class RoomManager {
  constructor(options = {}) {
    this.rooms = new Map();
    this.pendingRooms = new Map();
    this.revokedRoomKeys = new Set();
    this.idleMs = options.idleMs || DEFAULT_ROOM_IDLE_MS;
  }

  key(userId, deviceId) {
    return `${encodeURIComponent(userId)}:${deviceId}`;
  }

  async get(userId, deviceId) {
    const key = this.key(userId, deviceId);
    if (this.revokedRoomKeys.has(key)) throw httpError(401, "device revoked");
    let room = this.rooms.get(key);
    if (!room && this.pendingRooms.has(key)) room = await this.pendingRooms.get(key);
    if (!room) {
      const creating = this.createRoom(key, userId, deviceId);
      this.pendingRooms.set(key, creating);
      try {
        room = await creating;
      } finally {
        this.pendingRooms.delete(key);
      }
    }
    if (this.revokedRoomKeys.has(key)) throw httpError(401, "device revoked");
    room.lastActiveMs = Date.now();
    return room;
  }

  async createRoom(key, userId, deviceId) {
      const hostToken = randomToken();
      const operatorToken = randomToken();
      const relay = new CodexRelay({
        host: "127.0.0.1",
        port: 0,
        mode: "host",
        authRequired: true,
        hostToken,
        operatorToken,
        viewerToken: randomToken(),
      });
      await relay.start();
      if (this.revokedRoomKeys.has(key)) {
        await relay.stop().catch(() => undefined);
        throw httpError(401, "device revoked");
      }
      const address = relay.address();
      const room = {
        key,
        userId,
        deviceId,
        relay,
        hostToken,
        operatorToken,
        baseUrl: `ws://127.0.0.1:${address.port}`,
        connections: 0,
        lastActiveMs: Date.now(),
      };
      this.rooms.set(key, room);
    return room;
  }

  connectedDeviceIds(userId) {
    return new Set([...this.rooms.values()]
      .filter((room) => room.userId === userId && room.relay.state.hostConnected)
      .map((room) => room.deviceId));
  }

  retain(room) {
    room.connections += 1;
    room.lastActiveMs = Date.now();
  }

  release(room) {
    room.connections = Math.max(0, room.connections - 1);
    room.lastActiveMs = Date.now();
  }

  async cleanup() {
    const now = Date.now();
    for (const [key, room] of this.rooms) {
      if (room.connections > 0 || now - room.lastActiveMs < this.idleMs) continue;
      this.rooms.delete(key);
      await room.relay.stop();
    }
  }

  async revoke(userId, deviceId) {
    const key = this.key(userId, deviceId);
    this.revokedRoomKeys.add(key);
    const pending = this.pendingRooms.get(key);
    if (pending) await pending.catch(() => undefined);
    const room = this.rooms.get(key);
    if (!room) return;
    this.rooms.delete(key);
    await room.relay.stop();
  }

  async stop() {
    await Promise.allSettled([...this.pendingRooms.values()]);
    this.pendingRooms.clear();
    const rooms = [...this.rooms.values()];
    this.rooms.clear();
    this.revokedRoomKeys.clear();
    await Promise.allSettled(rooms.map((room) => room.relay.stop()));
  }
}

class AetherVscodexCloudServer {
  constructor(options = {}) {
    this.host = options.host || process.env.HOST || "127.0.0.1";
    this.port = parsePort(options.port ?? process.env.PORT, 8788);
    this.internalToken = options.internalToken || process.env.AETHER_VSCODEX_INTERNAL_TOKEN || "";
    this.publicWsUrl = options.publicWsUrl || process.env.AETHER_VSCODEX_PUBLIC_WS_URL || "";
    this.allowedOrigins = normalizeOrigins(options.allowedOrigins ?? process.env.AETHER_VSCODEX_ALLOWED_ORIGINS);
    const dataDir = options.dataDir || process.env.AETHER_VSCODEX_DATA_DIR || path.join(process.cwd(), "data");
    this.store = options.store || new DeviceStore(path.join(dataDir, "devices.json"));
    this.credentials = options.credentials || new EphemeralCredentials(options);
    this.rooms = options.rooms || new RoomManager(options);
    this.exchangeAttempts = new Map();
    this.httpServer = null;
    this.wsServer = null;
    this.cleanupTimer = null;
  }

  async start() {
    if (!this.internalToken) throw new Error("AETHER_VSCODEX_INTERNAL_TOKEN is required");
    if (Buffer.byteLength(this.internalToken, "utf8") < 24) throw new Error("AETHER_VSCODEX_INTERNAL_TOKEN must contain at least 24 bytes");
    if (!this.publicWsUrl) throw new Error("AETHER_VSCODEX_PUBLIC_WS_URL is required");
    validatePublicWsUrl(this.publicWsUrl);
    if (!isLoopbackHost(this.host) && this.allowedOrigins.size === 0) {
      throw new Error("AETHER_VSCODEX_ALLOWED_ORIGINS is required when binding outside loopback");
    }
    this.httpServer = http.createServer((request, response) => {
      void this.handleHttp(request, response).catch((error) => {
        jsonResponse(response, error.statusCode || 500, { error: error.expose ? error.message : "internal server error" });
      });
    });
    this.wsServer = new WebSocketServer({ noServer: true, maxPayload: MAX_WS_BYTES });
    this.httpServer.on("upgrade", (request, socket, head) => this.handleUpgrade(request, socket, head));
    this.cleanupTimer = setInterval(() => {
      this.credentials.cleanup();
      this.cleanupExchangeAttempts();
      void this.rooms.cleanup();
    }, 30_000);
    this.cleanupTimer.unref();
    await new Promise((resolve, reject) => {
      const onError = (error) => reject(error);
      this.httpServer.once("error", onError);
      this.httpServer.listen(this.port, this.host, () => {
        this.httpServer.off("error", onError);
        resolve();
      });
    });
    return this.address();
  }

  address() {
    const address = this.httpServer.address();
    if (!address || typeof address === "string") return { host: this.host, port: this.port };
    return { host: address.address, port: address.port };
  }

  async stop() {
    if (this.cleanupTimer) clearInterval(this.cleanupTimer);
    this.cleanupTimer = null;
    if (this.wsServer) {
      for (const client of this.wsServer.clients) client.close(1001, "server shutting down");
      await new Promise((resolve) => this.wsServer.close(() => resolve()));
    }
    if (this.httpServer) await new Promise((resolve) => this.httpServer.close(() => resolve()));
    this.wsServer = null;
    this.httpServer = null;
    await this.rooms.stop();
  }

  async handleHttp(request, response) {
    const requestUrl = new URL(request.url || "/", "http://sidecar.local");
    if (request.method === "GET" && requestUrl.pathname === "/healthz") {
      jsonResponse(response, 200, { ok: true, service: "aether-vscodex", mode: "single-replica" });
      return;
    }
    if (request.method === "POST" && requestUrl.pathname === "/v1/pairings/exchange") {
      this.enforceExchangeRate(request);
      const body = await readJson(request);
      const pairing = this.credentials.consumePairing(body.code);
      if (!pairing) throw httpError(400, "invalid or expired pairing code");
      const created = this.store.create(pairing.user_id, body.name || pairing.requested_name);
      jsonResponse(response, 201, {
        device_id: created.device.id,
        device_name: created.device.name,
        device_token: created.token,
        ws_url: this.publicWsUrl,
      });
      return;
    }

    const match = requestUrl.pathname.match(/^\/internal\/v1\/users\/([^/]+)\/(devices|pairings|ws-tickets)(?:\/([^/]+))?$/);
    if (!match) {
      jsonResponse(response, 404, { error: "not found" });
      return;
    }
    this.requireInternalAuth(request);
    const userId = decodeURIComponent(match[1]);
    const resource = match[2];
    const resourceId = match[3] ? decodeURIComponent(match[3]) : null;
    if (!userId || userId.length > 256) throw httpError(400, "invalid user id");

    if (request.method === "GET" && resource === "devices" && !resourceId) {
      jsonResponse(response, 200, { devices: this.store.list(userId, this.rooms.connectedDeviceIds(userId)) });
      return;
    }
    if (request.method === "POST" && resource === "pairings" && !resourceId) {
      const body = await readJson(request);
      const pairing = this.credentials.createPairing(userId, body.name);
      jsonResponse(response, 201, {
        pairing_id: pairing.id,
        code: pairing.code,
        expires_at: new Date(pairing.expires_at_ms).toISOString(),
      });
      return;
    }
    if (request.method === "DELETE" && resource === "devices" && resourceId) {
      if (!this.store.revoke(userId, resourceId)) throw httpError(404, "device not found");
      await this.rooms.revoke(userId, resourceId);
      response.writeHead(204, { "Cache-Control": "no-store" });
      response.end();
      return;
    }
    if (request.method === "POST" && resource === "ws-tickets" && !resourceId) {
      const body = await readJson(request);
      const deviceId = typeof body.device_id === "string" ? body.device_id : "";
      if (!deviceId || !this.store.get(userId, deviceId)) throw httpError(404, "device not found");
      jsonResponse(response, 201, {
        ticket: this.credentials.createTicket(userId, deviceId),
        ws_url: "/api/vscodex/ws",
        expires_in: Math.floor(this.credentials.ticketTtlMs / 1000),
      });
      return;
    }
    jsonResponse(response, 405, { error: "method not allowed" }, { Allow: allowedMethod(resource, resourceId) });
  }

  requireInternalAuth(request) {
    if (!this.hasInternalAuth(request)) throw httpError(401, "unauthorized");
  }

  hasInternalAuth(request) {
    const authorization = String(request.headers.authorization || "");
    const token = authorization.startsWith("Bearer ") ? authorization.slice(7) : "";
    return secureEqual(token, this.internalToken);
  }

  enforceExchangeRate(request) {
    const address = this.exchangeRateAddress(request);
    const now = Date.now();
    const attempts = (this.exchangeAttempts.get(address) || []).filter((time) => now - time < 60_000);
    if (attempts.length >= 10) throw httpError(429, "too many pairing attempts");
    attempts.push(now);
    this.exchangeAttempts.set(address, attempts);
  }

  exchangeRateAddress(request) {
    if (this.hasInternalAuth(request)) {
      const forwardedAddress = singleHeaderValue(request, "x-aether-client-ip")?.trim();
      if (forwardedAddress && net.isIP(forwardedAddress)) return forwardedAddress;
    }
    return request.socket.remoteAddress || "unknown";
  }

  cleanupExchangeAttempts() {
    const now = Date.now();
    for (const [address, attempts] of this.exchangeAttempts) {
      const active = attempts.filter((time) => now - time < 60_000);
      if (active.length) this.exchangeAttempts.set(address, active);
      else this.exchangeAttempts.delete(address);
    }
  }

  handleUpgrade(request, socket, head) {
    const requestUrl = new URL(request.url || "/", "http://sidecar.local");
    if (requestUrl.pathname !== "/api/vscodex/ws" && requestUrl.pathname !== "/v1/connect") {
      rejectUpgrade(socket, 404, "Not Found");
      return;
    }
    const origin = request.headers.origin;
    if (origin && this.allowedOrigins.size > 0 && !this.allowedOrigins.has(normalizeOrigin(origin))) {
      rejectUpgrade(socket, 403, "Forbidden");
      return;
    }
    this.wsServer.handleUpgrade(request, socket, head, (webSocket) => {
      this.wsServer.emit("connection", webSocket, request);
      this.handleWebSocket(webSocket, request);
    });
  }

  handleWebSocket(socket, request) {
    let hello = null;
    let token = "";
    let upstream = null;
    let authenticating = false;
    let room = null;
    const queued = [];
    const authTimer = setTimeout(() => socket.close(1008, "authentication required"), 10_000);
    authTimer.unref();

    const connectUpstream = async () => {
      if (authenticating || upstream || !hello || !token) return;
      authenticating = true;
      let identity;
      let upstreamToken;
      if (hello.clientType === "host") {
        const device = this.store.authenticate(token);
        if (!device) throw httpError(401, "invalid device credential");
        identity = { userId: device.user_id, deviceId: device.id };
        room = await this.rooms.get(identity.userId, identity.deviceId);
        upstreamToken = room.hostToken;
        this.store.touch(device.id);
      } else {
        const ticket = this.credentials.consumeTicket(token);
        if (!ticket || !this.store.get(ticket.user_id, ticket.device_id)) throw httpError(401, "invalid or expired browser ticket");
        identity = { userId: ticket.user_id, deviceId: ticket.device_id };
        room = await this.rooms.get(identity.userId, identity.deviceId);
        upstreamToken = room.operatorToken;
      }
      this.rooms.retain(room);
      upstream = new WebSocket(`${room.baseUrl}${hello.clientType === "host" ? "/v1/connect" : "/ws"}`, {
        maxPayload: MAX_WS_BYTES,
      });
      upstream.once("open", () => {
        if (socket.readyState !== WebSocket.OPEN) {
          upstream.close();
          return;
        }
        upstream.send(JSON.stringify(hello));
        upstream.send(JSON.stringify(hello.clientType === "host"
          ? { v: 1, kind: "auth", accessToken: upstreamToken }
          : { type: "auth", token: upstreamToken }));
        for (const frame of queued.splice(0)) upstream.send(frame);
      });
      upstream.on("message", (data, isBinary) => {
        if (socket.readyState === WebSocket.OPEN) socket.send(data, { binary: isBinary });
      });
      upstream.on("close", (code, reason) => {
        if (socket.readyState === WebSocket.OPEN) socket.close(validCloseCode(code) ? code : 1011, reason.toString().slice(0, 120) || "relay closed");
      });
      upstream.on("error", () => {
        if (socket.readyState === WebSocket.OPEN) socket.close(1011, "relay unavailable");
      });
      clearTimeout(authTimer);
    };

    socket.on("message", (data, isBinary) => {
      if (isBinary) {
        socket.close(1003, "JSON text frames only");
        return;
      }
      if (upstream) {
        const text = data.toString("utf8");
        if (upstream.readyState === WebSocket.OPEN) upstream.send(text);
        else queued.push(text);
        return;
      }
      let message;
      try {
        message = JSON.parse(data.toString("utf8"));
      } catch {
        socket.close(1007, "invalid JSON");
        return;
      }
      if (message?.kind === "hello") {
        if (Number(message.protocol || 1) !== 1) {
          socket.close(1002, "unsupported protocol");
          return;
        }
        hello = {
          v: 1,
          kind: "hello",
          clientType: message.clientType === "host" ? "host" : "web",
          protocol: 1,
          ...(typeof message.sessionId === "string" ? { sessionId: message.sessionId } : {}),
          ...(Number.isFinite(Number(message.lastSeq)) ? { lastSeq: Number(message.lastSeq) } : {}),
        };
      } else if (message?.kind === "auth" || message?.type === "auth") {
        token = typeof message.accessToken === "string" ? message.accessToken : typeof message.token === "string" ? message.token : "";
      } else {
        socket.close(1002, "hello and auth required");
        return;
      }
      void connectUpstream().catch(() => socket.close(1008, "authentication failed"));
    });
    socket.on("close", () => {
      clearTimeout(authTimer);
      if (upstream && upstream.readyState < WebSocket.CLOSING) upstream.close();
      if (room) this.rooms.release(room);
    });
    socket.on("error", () => {});
  }
}

function parseDeviceToken(token) {
  const match = /^avx1\.([0-9a-f-]{36})\.([A-Za-z0-9_-]{32,})$/.exec(String(token || ""));
  return match ? { id: match[1], secret: match[2] } : null;
}

function deriveSecret(secret, salt) {
  return crypto.scryptSync(secret, Buffer.from(salt, "base64url"), 32).toString("base64url");
}

function publicDevice(device, connected) {
  return {
    id: device.id,
    name: device.name,
    connected,
    created_at: device.created_at,
    last_seen_at: device.last_seen_at,
  };
}

function normalizeName(value) {
  const name = typeof value === "string" ? value.trim().replace(/\s+/g, " ").slice(0, 80) : "";
  return name || "VS Code";
}

function pairingCode() {
  const alphabet = "ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
  const bytes = crypto.randomBytes(8);
  let result = "";
  for (let index = 0; index < 8; index += 1) result += alphabet[bytes[index] % alphabet.length];
  return `${result.slice(0, 4)}-${result.slice(4)}`;
}

function normalizePairingCode(value) {
  return String(value || "").toUpperCase().replace(/[^A-Z2-9]/g, "");
}

function randomToken() {
  return crypto.randomBytes(32).toString("base64url");
}

function secureEqual(left, right) {
  const a = Buffer.from(String(left || ""));
  const b = Buffer.from(String(right || ""));
  return a.length === b.length && crypto.timingSafeEqual(a, b);
}

function singleHeaderValue(request, name) {
  const distinctValues = request.headersDistinct?.[name];
  if (Array.isArray(distinctValues)) return distinctValues.length === 1 ? distinctValues[0] : null;
  const value = request.headers[name];
  return typeof value === "string" ? value : null;
}

function parsePort(value, fallback) {
  const parsed = Number(value ?? fallback);
  if (!Number.isInteger(parsed) || parsed < 0 || parsed > 65535) throw new Error("invalid port");
  return parsed;
}

function normalizeOrigins(value) {
  const values = Array.isArray(value) ? value : String(value || "").split(",");
  return new Set(values.map(normalizeOrigin).filter(Boolean));
}

function normalizeOrigin(value) {
  try {
    return new URL(String(value).trim()).origin.toLowerCase();
  } catch {
    return "";
  }
}

function validatePublicWsUrl(value) {
  let url;
  try {
    url = new URL(value);
  } catch {
    throw new Error("AETHER_VSCODEX_PUBLIC_WS_URL must be an absolute WebSocket URL");
  }
  if (url.protocol !== "wss:" && !(url.protocol === "ws:" && isLoopbackHost(url.hostname))) {
    throw new Error("AETHER_VSCODEX_PUBLIC_WS_URL must use wss:// outside loopback");
  }
}

function isLoopbackHost(value) {
  const host = String(value || "").replace(/^\[|\]$/g, "").toLowerCase();
  return host === "127.0.0.1" || host === "localhost" || host === "::1";
}

function validCloseCode(code) {
  return code === 1000 || (code >= 1001 && code <= 1014 && ![1004, 1005, 1006].includes(code)) || (code >= 3000 && code <= 4999);
}

function readJson(request) {
  return new Promise((resolve, reject) => {
    let size = 0;
    const chunks = [];
    request.on("data", (chunk) => {
      size += chunk.length;
      if (size > MAX_JSON_BYTES) {
        reject(httpError(413, "request body too large"));
        request.destroy();
        return;
      }
      chunks.push(chunk);
    });
    request.on("end", () => {
      try {
        const value = JSON.parse(Buffer.concat(chunks).toString("utf8") || "{}");
        if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error();
        resolve(value);
      } catch {
        reject(httpError(400, "invalid JSON body"));
      }
    });
    request.on("error", reject);
  });
}

function jsonResponse(response, statusCode, body, extraHeaders = {}) {
  if (response.headersSent) return;
  const payload = Buffer.from(JSON.stringify(body));
  response.writeHead(statusCode, {
    "Content-Type": "application/json; charset=utf-8",
    "Content-Length": payload.length,
    "Cache-Control": "no-store",
    ...extraHeaders,
  });
  response.end(payload);
}

function rejectUpgrade(socket, status, reason) {
  socket.write(`HTTP/1.1 ${status} ${reason}\r\nConnection: close\r\n\r\n`);
  socket.destroy();
}

function httpError(statusCode, message) {
  return Object.assign(new Error(message), { statusCode, expose: statusCode < 500 });
}

function allowedMethod(resource, resourceId) {
  if (resource === "devices" && resourceId) return "DELETE";
  if (resource === "devices") return "GET";
  return "POST";
}

async function main() {
  const server = new AetherVscodexCloudServer();
  const address = await server.start();
  process.stdout.write(`Aether VS Codex sidecar listening on ${address.host}:${address.port}\n`);
  const shutdown = async () => {
    await server.stop();
    process.exit(0);
  };
  process.once("SIGINT", shutdown);
  process.once("SIGTERM", shutdown);
}

if (require.main === module) {
  main().catch((error) => {
    process.stderr.write(`${error.stack || error}\n`);
    process.exitCode = 1;
  });
}

module.exports = {
  AetherVscodexCloudServer,
  DeviceStore,
  EphemeralCredentials,
  RoomManager,
};
