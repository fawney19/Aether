"use strict";

const crypto = require("node:crypto");
const fs = require("node:fs");
const http = require("node:http");
const net = require("node:net");
const path = require("node:path");
const { spawn } = require("node:child_process");
const { URL } = require("node:url");
const { WebSocket, WebSocketServer } = require("ws");

const PACKAGE_ROOT = path.resolve(__dirname, "..");
const VUE_PUBLIC_ROOT = path.join(PACKAGE_ROOT, "web", "dist");
const PUBLIC_ROOT = fs.existsSync(path.join(VUE_PUBLIC_ROOT, "index.html"))
  ? VUE_PUBLIC_ROOT
  : path.join(PACKAGE_ROOT, "public");
const MAX_JSON_BODY = 1024 * 1024;
// Complete attached-session snapshots include the structured message/tool
// projection and can easily exceed 256 KiB. This remains bounded to protect
// the local relay while allowing long Codex conversations to hydrate.
const MAX_WS_PAYLOAD = 16 * 1024 * 1024;
const DEFAULT_EVENT_LIMIT = 2_000;
// Transcript state is represented once in the authoritative control snapshot.
// Keep replay and socket buffering smaller than an unbounded count of maximum
// sized frames so one long conversation cannot amplify into gigabytes.
const DEFAULT_EVENT_BYTE_LIMIT = 16 * 1024 * 1024;
const DEFAULT_REPLAY_BYTE_LIMIT = 2 * 1024 * 1024;
const DEFAULT_CLIENT_BUFFERED_BYTE_LIMIT = MAX_WS_PAYLOAD + 2 * 1024 * 1024;
const MAX_REPLAY_TEXT_BYTES = 64 * 1024;
const MUTATING_METHODS = new Set([
  "control/mode/set",
  "thread/start",
  "session/new",
  "thread/settings/update",
  "session/select",
  "turn/start",
  "turn/steer",
  "turn/interrupt",
]);
const ALLOWED_METHODS = new Set(["initialize", "control/mode/get", "session/list", ...MUTATING_METHODS]);
const SERVER_REQUEST_METHODS = new Set([
  "item/commandExecution/requestApproval",
  "item/fileChange/requestApproval",
  "item/permissions/requestApproval",
  "item/tool/requestUserInput",
  "mcpServer/elicitation/request",
  "applyPatchApproval",
  "execCommandApproval",
]);
const REMOTE_RESPONSE_METHODS = new Set([
  "approval.respond",
  "input.respond",
  "server.request.respond",
]);
const DEFAULT_APPROVAL_TIMEOUT_MS = 5 * 60 * 1000;

function randomToken() {
  return crypto.randomBytes(24).toString("base64url");
}

function randomId(prefix) {
  return `${prefix}_${crypto.randomBytes(9).toString("base64url")}`;
}

// JSON-RPC treats numeric and string ids as distinct values. Keep that
// distinction in in-memory maps while retaining a small compatibility bridge
// for older browser clients that stringify numeric ids before responding.
function jsonRpcIdKey(id) {
  if (typeof id === "number") {
    return `number:${Object.is(id, -0) ? "-0" : String(id)}`;
  }
  if (typeof id === "string") return `string:${id}`;
  if (id === null) return "null:";
  return `${typeof id}:${String(id)}`;
}

function isJsonRpcId(id) {
  return typeof id === "string" || typeof id === "number";
}

function findTypedMapKey(map, id, valueId = (value) => value?.appId, allowLegacyStringified = false) {
  const exact = jsonRpcIdKey(id);
  if (map.has(exact)) return exact;
  // Before protocol v1, the browser console sent every request id as text.
  // Allow that form only when it maps to one unambiguous pending id. If both
  // `1` and `"1"` are pending, the exact typed key above wins and no cross-talk
  // is possible.
  if (!allowLegacyStringified || !isJsonRpcId(id)) return exact;
  const text = String(id);
  const candidates = [];
  for (const [key, value] of map) {
    const candidateId = valueId(value);
    if (isJsonRpcId(candidateId) && String(candidateId) === text) candidates.push(key);
  }
  return candidates.length === 1 ? candidates[0] : exact;
}

function sameJsonRpcId(left, right) {
  return typeof left === typeof right && String(left) === String(right)
    && (typeof left === "string" || typeof left === "number");
}

function responseCommandId(requestId, pendingServerRequests, pendingHostCommands) {
  for (const [commandId, pending] of pendingHostCommands) {
    if (pending.kind === "server-response"
      && (sameJsonRpcId(pending.requestId, requestId) || sameJsonRpcId(pending.responseRequestId, requestId))) return commandId;
  }
  const base = `response-${String(requestId)}`;
  let oppositeKey;
  if (typeof requestId === "number") {
    oppositeKey = jsonRpcIdKey(String(requestId));
  } else if (typeof requestId === "string") {
    const numeric = Number(requestId);
    if (Number.isFinite(numeric)) oppositeKey = jsonRpcIdKey(numeric);
  }
  if (oppositeKey && pendingServerRequests.has(oppositeKey)) return `${base}-${typeof requestId}`;

  const typed = `response-${jsonRpcIdKey(requestId)}`;
  const existingBase = pendingHostCommands.get(base);
  if (!existingBase || sameJsonRpcId(existingBase.requestId, requestId)) return base;
  const existingTyped = pendingHostCommands.get(typed);
  if (!existingTyped || sameJsonRpcId(existingTyped.requestId, requestId)) return typed;
  // This is only reachable if a caller has manually occupied both stable
  // names. Keep the command id deterministic and bounded while avoiding an
  // accidental overwrite.
  return `${typed}-${crypto.createHash("sha256").update(String(requestId)).digest("hex").slice(0, 8)}`;
}

function secureEqual(left, right) {
  const a = Buffer.from(String(left || ""));
  const b = Buffer.from(String(right || ""));
  return a.length === b.length && crypto.timingSafeEqual(a, b);
}

function jsonResponse(response, statusCode, body, extraHeaders = {}) {
  const payload = Buffer.from(JSON.stringify(body));
  response.writeHead(statusCode, {
    "Content-Type": "application/json; charset=utf-8",
    "Content-Length": payload.length,
    "Cache-Control": "no-store",
    ...extraHeaders,
  });
  response.end(payload);
}

function readJson(request) {
  return new Promise((resolve, reject) => {
    let size = 0;
    const chunks = [];
    request.on("data", (chunk) => {
      size += chunk.length;
      if (size > MAX_JSON_BODY) {
        reject(Object.assign(new Error("request body is too large"), { statusCode: 413 }));
        request.destroy();
        return;
      }
      chunks.push(chunk);
    });
    request.on("end", () => {
      try {
        resolve(JSON.parse(Buffer.concat(chunks).toString("utf8") || "{}"));
      } catch {
        reject(Object.assign(new Error("invalid JSON body"), { statusCode: 400 }));
      }
    });
    request.on("error", reject);
  });
}

function redactString(value) {
  return String(value)
    .replace(/\b(sk-[A-Za-z0-9_-]{12,})\b/g, "[REDACTED_API_KEY]")
    .replace(/\b(Bearer\s+)[A-Za-z0-9._~+\/-]{12,}/gi, "$1[REDACTED]")
    .replace(/\b(gh[pousr]_[A-Za-z0-9]{20,})\b/g, "[REDACTED_GITHUB_TOKEN]")
    .replace(/([?&](?:token|key|secret)=)[^&\s]+/gi, "$1[REDACTED]");
}

function redact(value, depth = 0) {
  if (depth > 8) return "[TRUNCATED]";
  if (typeof value === "string") return redactString(value);
  if (Array.isArray(value)) return value.map((entry) => redact(entry, depth + 1));
  if (value && typeof value === "object") {
    const result = {};
    for (const [key, entry] of Object.entries(value)) {
      if (/token|authorization|cookie|private.?key|secret/i.test(key)) {
        result[key] = "[REDACTED]";
      } else {
        result[key] = redact(entry, depth + 1);
      }
    }
    return result;
  }
  return value;
}

function applyStructuredMessagesPatch(current, patch) {
  if (!Array.isArray(current) || !patch || typeof patch !== "object" || Array.isArray(patch)) return null;
  const start = Number(patch.start);
  const deleteCount = Number(patch.deleteCount);
  if (!Number.isInteger(start) || start < 0 || start > current.length
    || !Number.isInteger(deleteCount) || deleteCount < 0 || start + deleteCount > current.length
    || !Array.isArray(patch.messages)) return null;
  return [
    ...current.slice(0, start),
    ...patch.messages,
    ...current.slice(start + deleteCount),
  ];
}

function positiveByteLimit(value, fallback) {
  const parsed = Number(value);
  return Number.isFinite(parsed) && parsed > 0 ? Math.floor(parsed) : fallback;
}

function jsonByteLength(value) {
  return Buffer.byteLength(JSON.stringify(value), "utf8");
}

function boundedReplayText(value) {
  const encoded = Buffer.from(String(value), "utf8");
  if (encoded.length <= MAX_REPLAY_TEXT_BYTES) return String(value);
  // A cut through a multi-byte code point can add one replacement character,
  // which is harmless for this best-effort replay hint. The following control
  // snapshot carries the exact authoritative transcript.
  return encoded.subarray(encoded.length - MAX_REPLAY_TEXT_BYTES).toString("utf8");
}

// Every subscriber receives an authoritative control snapshot after replay.
// Keep transcript-bearing live events rich, but store only their lightweight
// form in the replay ring so streaming a long session cannot retain hundreds
// of duplicate full-history projections.
function compactTranscriptEventForReplay(event) {
  if (!event || !["session.snapshot", "output.snapshot", "output.chunk"].includes(event.type)) return event;
  const source = event.payload && typeof event.payload === "object" && !Array.isArray(event.payload)
    ? event.payload
    : {};
  // Use an allow-list rather than deleting known large fields. In particular,
  // current attach adapters send `messagesPatch` instead of `messages`, and a
  // suffix replacement can itself be nearly as large as the full transcript.
  const payload = { projectionInControlSnapshot: true };
  for (const key of [
    "threadId",
    "turnId",
    "requestId",
    "source",
    "sourceSeq",
    "stream",
    "encoding",
    "state",
    "structureChanged",
  ]) {
    const value = source[key];
    if (typeof value === "string" || typeof value === "number" || typeof value === "boolean" || value === null) {
      payload[key] = value;
    }
  }
  if (event.type === "output.chunk" && typeof source.text === "string" && source.text) {
    payload.text = boundedReplayText(source.text);
  }
  return { ...event, payload };
}

// Token usage is telemetry, not an authentication credential. The generic
// redactor intentionally treats any key containing "token" as sensitive, so
// preserve only the numeric usage projection after redacting the rest of a
// session metadata envelope.
const SAFE_USAGE_FIELDS = [
  "totalTokens",
  "total_tokens",
  "inputTokens",
  "input_tokens",
  "cachedInputTokens",
  "cached_input_tokens",
  "cacheWriteInputTokens",
  "cache_write_input_tokens",
  "outputTokens",
  "output_tokens",
  "reasoningOutputTokens",
  "reasoning_output_tokens",
];

function safeUsageNumber(value) {
  if (typeof value === "number") return Number.isFinite(value) && value >= 0 ? value : undefined;
  if (typeof value === "string" && /^\d+(?:\.\d+)?$/.test(value.trim())) {
    const number = Number(value);
    return Number.isFinite(number) && number >= 0 ? number : undefined;
  }
  return undefined;
}

function safeUsageBreakdown(value) {
  if (!value || typeof value !== "object" || Array.isArray(value)) return undefined;
  const result = {};
  for (const field of SAFE_USAGE_FIELDS) {
    const number = safeUsageNumber(value[field]);
    if (number !== undefined) result[field.replace(/_([a-z])/g, (_, letter) => letter.toUpperCase())] = number;
  }
  return Object.keys(result).length ? result : undefined;
}

function safeTokenUsage(value) {
  if (value === null) return null;
  if (!value || typeof value !== "object" || Array.isArray(value)) return undefined;
  const source = value.info && typeof value.info === "object" ? value.info
    : value.tokenUsage && typeof value.tokenUsage === "object" ? value.tokenUsage
      : value.token_usage && typeof value.token_usage === "object" ? value.token_usage : value;
  const result = {};
  const total = safeUsageBreakdown(source.total ?? source.total_token_usage ?? source.totalTokenUsage);
  const last = safeUsageBreakdown(source.last ?? source.last_token_usage ?? source.lastTokenUsage);
  const context = safeUsageNumber(source.modelContextWindow ?? source.model_context_window ?? source.contextWindow ?? source.context_window);
  if (total) result.total = total;
  if (last) result.last = last;
  if (context !== undefined) result.modelContextWindow = context;
  return Object.keys(result).length ? result : undefined;
}

function redactSessionMetadata(value) {
  const redacted = redact(value);
  if (!redacted || typeof redacted !== "object" || Array.isArray(redacted) || !value || typeof value !== "object") return redacted;
  for (const key of ["tokenUsage", "latestTokenUsageInfo"]) {
    if (!Object.prototype.hasOwnProperty.call(value, key)) continue;
    const usage = safeTokenUsage(value[key]);
    if (usage !== undefined) redacted[key] = usage;
  }
  return redacted;
}

function normalizeError(error) {
  return {
    code: error && error.code ? String(error.code) : "relay_error",
    message: redactString(error && error.message ? error.message : String(error)),
    retryable: Boolean(error && error.retryable),
  };
}

function parsePort(value, fallback) {
  const parsed = Number(value);
  return Number.isInteger(parsed) && parsed >= 0 && parsed < 65536 ? parsed : fallback;
}

/** Parse an optional auth switch without treating an invalid value as false. */
function parseAuthRequired(value) {
  if (typeof value === "boolean") return value;
  if (value === 1) return true;
  if (value === 0) return false;
  if (typeof value !== "string" || value.trim() === "") return undefined;
  const normalized = value.trim().toLowerCase();
  if (["1", "true", "yes", "on", "required", "enabled"].includes(normalized)) return true;
  if (["0", "false", "no", "off", "none", "disabled", "local"].includes(normalized)) return false;
  return undefined;
}

function hasConfiguredToken(value) {
  return typeof value === "string" && value.length > 0;
}

/** Whether the configured listen address is restricted to this machine. */
function isLoopbackIPv4(value) {
  if (net.isIP(value) !== 4) return false;
  const octets = value.split(".").map(Number);
  return octets.length === 4 && octets[0] === 127;
}

function isLoopbackHost(host) {
  const normalized = String(host || "").trim().toLowerCase().replace(/^\[|\]$/g, "");
  if (normalized === "localhost" || normalized === "::1") return true;
  if (isLoopbackIPv4(normalized)) return true;
  return normalized.startsWith("::ffff:") && isLoopbackIPv4(normalized.slice("::ffff:".length));
}

/** Node reports IPv4 loopback peers as both 127.x and ::ffff:127.x. */
function isLoopbackAddress(address) {
  const normalized = String(address || "").trim().toLowerCase();
  if (normalized === "::1") return true;
  if (isLoopbackIPv4(normalized)) return true;
  return normalized.startsWith("::ffff:") && isLoopbackIPv4(normalized.slice("::ffff:".length));
}

function isLoopbackRequestHost(request) {
  const rawHost = String(request.headers.host || "").trim();
  if (!rawHost) return false;
  try {
    return isLoopbackHost(new URL(`http://${rawHost}`).hostname);
  } catch {
    return false;
  }
}

/** Allow browser writes only from the relay's own origin; CLI requests omit Origin. */
function isAllowedHttpOrigin(request) {
  const origin = request.headers.origin;
  if (!origin) return true;
  const requestHost = String(request.headers.host || "").trim().toLowerCase();
  if (!requestHost) return false;
  try {
    return new URL(origin).host.toLowerCase() === requestHost;
  } catch {
    return false;
  }
}

function contentType(filePath) {
  const extension = path.extname(filePath).toLowerCase();
  return (
    {
      ".html": "text/html; charset=utf-8",
      ".js": "text/javascript; charset=utf-8",
      ".css": "text/css; charset=utf-8",
      ".json": "application/json; charset=utf-8",
      ".svg": "image/svg+xml",
      ".png": "image/png",
    }[extension] || "application/octet-stream"
  );
}

function outputText(method, params) {
  if (!params || typeof params !== "object") return "";
  const candidates = [params.delta, params.text, params.output, params.chunk, params.message];
  if (params.item && typeof params.item === "object") {
    candidates.push(params.item.text, params.item.content);
  }
  const text = candidates.find((candidate) => typeof candidate === "string");
  if (text) return redactString(text);
  if (/outputDelta|agentMessage\/delta|plan\/delta|reasoning\/.+Delta/.test(method)) {
    return redactString(JSON.stringify(params));
  }
  return "";
}

function eventTypeForAppMessage(message) {
  if (message.id !== undefined && message.method) {
    if (message.method === "item/tool/requestUserInput" || message.method === "mcpServer/elicitation/request") {
      return "input.requested";
    }
    if (SERVER_REQUEST_METHODS.has(message.method)) return "approval.requested";
    return "server.requested";
  }
  if (message.id !== undefined) return "app.response";
  const method = message.method || "unknown";
  if (/outputDelta|agentMessage\/delta|plan\/delta|reasoning\/.+Delta/.test(method)) return "output.delta";
  if (method === "turn/started") return "turn.started";
  if (method === "turn/completed") return "turn.completed";
  if (method === "thread/started") return "thread.started";
  if (method === "error") return "app.error";
  return "app.notification";
}

function approvalDecisionForResult(result) {
  if (result && typeof result === "object" && !Array.isArray(result)) {
    const decision = result.decision;
    if (typeof decision === "string") {
      if (["acceptForSession", "accept", "approved", "approved_for_session", "approved_mcp_policy_amendment"].includes(decision)) return "allow";
      if (["cancel", "abort"].includes(decision)) return "cancel";
      if (["decline", "denied", "timed_out"].includes(decision)) return "deny";
    }
    // App-server v2 encodes policy amendments as tagged objects. Require one
    // known tag with its documented shape; unknown or mixed objects fail
    // closed instead of being interpreted as an approval.
    const decisionKind = approvalDecisionKind(decision);
    if (decisionKind) return decisionKind;
  }
  if (result && typeof result === "object" && !Array.isArray(result)) {
    if (result.action === "accept") return "allow";
    if (result.action === "cancel") return "cancel";
    if (result.action === "decline") return "deny";
    if (result.permissions && typeof result.permissions === "object") {
      return Object.keys(result.permissions).length ? "allow" : "deny";
    }
  }
  // A custom response is still sent to the bridge; this value is only the
  // local policy hint used by RelayHost when it needs a canonical decision.
  return "deny";
}

function approvalDecisionKind(value) {
  if (typeof value === "string") {
    if (["allow", "accept", "acceptForSession", "approved", "approved_for_session", "approved_mcp_policy_amendment"].includes(value)) return "allow";
    if (["deny", "decline", "denied", "timed_out"].includes(value)) return "deny";
    if (["cancel", "abort"].includes(value)) return "cancel";
    return undefined;
  }
  const key = knownDecisionObjectKey(value);
  if (key) return key === "denied" ? "deny" : "allow";
  return undefined;
}

function responseErrorMessage(error) {
  if (typeof error === "string") return redactString(error).slice(0, 1_000);
  if (error && typeof error === "object" && typeof error.message === "string") return redactString(error.message).slice(0, 1_000);
  return "remote response rejected";
}

function defaultServerResponse(method, reason = "request timed out") {
  if (method === "item/permissions/requestApproval") return { permissions: {}, scope: "turn" };
  if (method === "item/tool/requestUserInput") return { answers: {} };
  if (method === "mcpServer/elicitation/request") return { action: "decline", content: null, _meta: null };
  if (method === "applyPatchApproval" || method === "execCommandApproval") {
    return { decision: { denied: { rejection: reason } } };
  }
  return { decision: "decline" };
}

function normalizeServerResponseForApp(method, response) {
  response = normalizeApprovalResponseForApp(method, response);
  if (method === "item/permissions/requestApproval") {
    const source = isObjectPayload(response) ? response : {};
    const requested = isObjectPayload(source.permissions) ? source.permissions : {};
    const permissions = {};
    for (const [key, value] of Object.entries(requested)) {
      if (value !== null && value !== undefined && isObjectPayload(value)) permissions[key] = value;
    }
    const normalized = { permissions, scope: source.scope === "session" ? "session" : "turn" };
    if (typeof source.strictAutoReview === "boolean") normalized.strictAutoReview = source.strictAutoReview;
    return normalized;
  }
  if (method === "item/tool/requestUserInput") {
    if (isObjectPayload(response) && Object.prototype.hasOwnProperty.call(response, "answers")) return response;
    return { answers: isObjectPayload(response) ? response : {} };
  }
  return response;
}

const LEGACY_APPROVAL_METHODS = new Set(["applyPatchApproval", "execCommandApproval"]);

function normalizeApprovalResponseForApp(method, response) {
  const v2Approval = new Set([
    "item/commandExecution/requestApproval",
    "item/fileChange/requestApproval",
  ]);
  if (!LEGACY_APPROVAL_METHODS.has(method) && !v2Approval.has(method)) return response;
  if (!isObjectPayload(response) || !Object.prototype.hasOwnProperty.call(response, "decision")) return response;
  const decision = response.decision;
  const legacy = LEGACY_APPROVAL_METHODS.has(method);
  let normalized = decision;
  if (typeof decision === "string") {
    if (legacy) {
      if (["allow", "accept", "approved"].includes(decision)) normalized = "approved";
      else if (["acceptForSession", "approved_for_session"].includes(decision)) normalized = "approved_for_session";
      else if (["deny", "decline", "denied"].includes(decision)) {
        normalized = { denied: { rejection: "Denied remotely" } };
      } else if (["cancel", "abort"].includes(decision)) normalized = "abort";
      else if (decision === "timed_out") normalized = "timed_out";
    } else {
      if (["allow", "accept", "approved"].includes(decision)) normalized = "accept";
      else if (["acceptForSession", "approved_for_session"].includes(decision)) normalized = "acceptForSession";
      else if (["deny", "decline", "denied", "timed_out"].includes(decision)) normalized = "decline";
      else if (["cancel", "abort"].includes(decision)) normalized = "cancel";
      else if (decision === "approved_mcp_policy_amendment") normalized = "accept";
    }
  } else if (knownDecisionObjectKey(decision)) {
    const decisionKey = knownDecisionObjectKey(decision);
    if (legacy && decisionKey === "acceptWithExecpolicyAmendment") {
      const value = decision.acceptWithExecpolicyAmendment;
      normalized = { approved_execpolicy_amendment: { proposed_execpolicy_amendment: value?.execpolicy_amendment ?? value } };
    } else if (legacy && decisionKey === "applyNetworkPolicyAmendment") {
      const value = decision.applyNetworkPolicyAmendment;
      normalized = { network_policy_amendment: { network_policy_amendment: value?.network_policy_amendment ?? value } };
    } else if (!legacy && decisionKey === "approved_execpolicy_amendment") {
      const value = decision.approved_execpolicy_amendment;
      normalized = { acceptWithExecpolicyAmendment: { execpolicy_amendment: value?.proposed_execpolicy_amendment ?? value } };
    } else if (!legacy && decisionKey === "network_policy_amendment") {
      const value = decision.network_policy_amendment;
      normalized = { applyNetworkPolicyAmendment: { network_policy_amendment: value?.network_policy_amendment ?? value } };
    } else if (!legacy && decisionKey === "denied") {
      normalized = "decline";
    }
  }
  return normalized === decision ? response : { ...response, decision: normalized };
}

function isValidApprovalResponse(method, response) {
  const legacy = LEGACY_APPROVAL_METHODS.has(method);
  const v2 = method === "item/commandExecution/requestApproval" || method === "item/fileChange/requestApproval";
  if (!legacy && !v2) return true;
  if (!isObjectPayload(response) || !Object.prototype.hasOwnProperty.call(response, "decision")) return false;
  const decision = response.decision;
  if (typeof decision === "string") {
    return legacy
      ? ["approved", "approved_for_session", "approved_mcp_policy_amendment", "timed_out", "abort"].includes(decision)
      : ["accept", "acceptForSession", "decline", "cancel"].includes(decision);
  }
  if (!decision || typeof decision !== "object" || Array.isArray(decision)) return false;
  const key = knownDecisionObjectKey(decision);
  return legacy
    ? key === "approved_execpolicy_amendment" || key === "network_policy_amendment" || key === "denied"
    : key === "acceptWithExecpolicyAmendment" || key === "applyNetworkPolicyAmendment";
}

function knownDecisionObjectKey(value) {
  if (!isObjectPayload(value)) return undefined;
  const keys = Object.keys(value);
  if (keys.length !== 1) return undefined;
  const key = keys[0];
  const nested = value[key];
  if (key === "acceptWithExecpolicyAmendment") {
    return isObjectPayload(nested)
      && Object.keys(nested).every((field) => field === "execpolicy_amendment")
      && isStringArray(nested.execpolicy_amendment) ? key : undefined;
  }
  if (key === "approved_execpolicy_amendment") {
    return isObjectPayload(nested)
      && Object.keys(nested).every((field) => field === "proposed_execpolicy_amendment")
      && isStringArray(nested.proposed_execpolicy_amendment) ? key : undefined;
  }
  if (key === "applyNetworkPolicyAmendment") {
    return isObjectPayload(nested)
      && Object.keys(nested).every((field) => field === "network_policy_amendment")
      && isNetworkPolicyAmendment(nested.network_policy_amendment) ? key : undefined;
  }
  if (key === "network_policy_amendment") {
    return isObjectPayload(nested)
      && Object.keys(nested).every((field) => field === "network_policy_amendment")
      && isNetworkPolicyAmendment(nested.network_policy_amendment) ? key : undefined;
  }
  if (key === "denied") {
    return isObjectPayload(nested)
      && Object.keys(nested).every((field) => field === "rejection")
      && typeof nested.rejection === "string" ? key : undefined;
  }
  return undefined;
}

function isStringArray(value) {
  return Array.isArray(value) && value.every((entry) => typeof entry === "string");
}

function isNetworkPolicyAmendment(value) {
  return isObjectPayload(value)
    && Object.keys(value).every((field) => field === "host" || field === "action")
    && typeof value.host === "string"
    && (value.action === "allow" || value.action === "deny");
}

class CodexRelay {
  constructor(options = {}) {
    this.host = options.host || process.env.HOST || "127.0.0.1";
    this.port = parsePort(options.port ?? process.env.PORT, 8787);
    this.eventLimit = positiveByteLimit(
      options.eventLimit ?? process.env.CODEX_REMOTE_EVENT_LIMIT,
      DEFAULT_EVENT_LIMIT,
    );
    this.eventByteLimit = positiveByteLimit(
      options.eventByteLimit ?? process.env.CODEX_REMOTE_EVENT_BYTE_LIMIT,
      DEFAULT_EVENT_BYTE_LIMIT,
    );
    this.replayByteLimit = positiveByteLimit(
      options.replayByteLimit ?? process.env.CODEX_REMOTE_REPLAY_BYTE_LIMIT,
      DEFAULT_REPLAY_BYTE_LIMIT,
    );
    this.clientBufferedByteLimit = positiveByteLimit(
      options.clientBufferedByteLimit ?? process.env.CODEX_REMOTE_CLIENT_BUFFERED_BYTE_LIMIT,
      DEFAULT_CLIENT_BUFFERED_BYTE_LIMIT,
    );
    const tokenConfigured = hasConfiguredToken(options.operatorToken)
      || hasConfiguredToken(options.viewerToken)
      || hasConfiguredToken(options.hostToken)
      || hasConfiguredToken(process.env.CODEX_REMOTE_TOKEN)
      || hasConfiguredToken(process.env.CODEX_REMOTE_VIEW_TOKEN)
      || hasConfiguredToken(process.env.CODEX_REMOTE_HOST_TOKEN);
    const explicitAuthRequired = Object.prototype.hasOwnProperty.call(options, "authRequired")
      ? parseAuthRequired(options.authRequired)
      : parseAuthRequired(process.env.CODEX_REMOTE_AUTH)
        ?? parseAuthRequired(process.env.CODEX_REMOTE_AUTH_REQUIRED);
    // A loopback relay is a local development tool by default. The moment a
    // token is configured, or the relay binds a non-loopback address, retain
    // the authenticated behavior. `authRequired` can explicitly enable auth
    // for a local relay; disabling it is intentionally limited to loopback.
    this.authRequired = explicitAuthRequired ?? (tokenConfigured || !isLoopbackHost(this.host));
    if (!this.authRequired && !isLoopbackHost(this.host)) this.authRequired = true;
    this.operatorToken = options.operatorToken || process.env.CODEX_REMOTE_TOKEN || randomToken();
    this.viewerToken = options.viewerToken || process.env.CODEX_REMOTE_VIEW_TOKEN || randomToken();
    this.hostToken = options.hostToken || process.env.CODEX_REMOTE_HOST_TOKEN || this.operatorToken;
    const configuredApprovalTimeout = Number(options.approvalTimeoutMs ?? process.env.CODEX_REMOTE_APPROVAL_TIMEOUT_MS);
    this.approvalTimeoutMs = Number.isFinite(configuredApprovalTimeout) && configuredApprovalTimeout >= 0
      ? configuredApprovalTimeout
      : DEFAULT_APPROVAL_TIMEOUT_MS;
    this.generatedOperatorToken = !options.operatorToken && !process.env.CODEX_REMOTE_TOKEN;
    this.generatedViewerToken = !options.viewerToken && !process.env.CODEX_REMOTE_VIEW_TOKEN;
    this.codexCommand = options.codexCommand || process.env.CODEX_BIN || "codex";
    this.codexArgs = options.codexArgs || this.readCodexArgs();
    this.codexCwd = options.codexCwd || process.env.CODEX_CWD || process.cwd();
    const spawnConfigured = options.spawnCodex === true
      || process.env.CODEX_SPAWN === "true"
      || process.env.CODEX_SPAWN === "1";
    // Attaching to the already-open VS Code Codex session is the safe default.
    // Keep the standalone app-server path available only when it is explicit.
    this.mode = options.mode
      || process.env.CODEX_REMOTE_MODE
      || (options.spawnCodex === false || process.env.CODEX_SPAWN === "false"
        ? "host"
        : spawnConfigured ? "embedded" : "host");
    this.spawnCodex = this.mode === "host"
      ? false
      : options.spawnCodex !== undefined
      ? options.spawnCodex !== false
      : process.env.CODEX_SPAWN !== "false";
    this.events = [];
    this.eventSizes = [];
    this.eventBytes = 0;
    this.audit = [];
    this.clients = new Set();
    // At most one outbound VS Code host is active for this MVP session. A
    // host is optional: when absent, the relay can run its embedded stdio
    // app-server. When present, browser commands are proxied to the host.
    this.hostClient = null;
    this.pendingHostCommands = new Map();
    this.pendingAppRequests = new Map();
    this.pendingServerRequests = new Map();
    this.commandResults = new Map();
    // Command idempotency is scoped to the embedded app or to one stable host
    // session. Keep the scope metadata separate so it never crosses the wire.
    this.commandResultScopes = new Map();
    this.hostCommandScope = null;
    this.nextSeq = 0;
    this.appRequestCounter = 0;
    this.appBuffer = "";
    this.appProcess = null;
    this.appGeneration = 0;
    this.appTerminalGeneration = 0;
    this.initializedResult = null;
    this.state = {
      app: this.spawnCodex ? "starting" : "waiting_for_host",
      initialized: false,
      activeThreadId: null,
      activeTurnId: null,
      cwd: this.codexCwd,
      outputTail: "",
      messages: [],
      subagents: [],
      // Keep non-sensitive session metadata available for browsers that join
      // after the adapter's original session.snapshot event was replayed.
      sessionMetadata: null,
      // Typed turn/activity projection from the attached VS Code host. Keep
      // this in the relay snapshot so a browser that connects after the last
      // event still knows whether the conversation is thinking, editing, or
      // waiting for approval.
      executionStatus: null,
      lastError: null,
      mode: this.mode,
      authRequired: this.authRequired,
      hostConnected: false,
      hostSessionId: null,
    };
  }

  readCodexArgs() {
    if (!process.env.CODEX_ARGS_JSON) return ["app-server", "--stdio"];
    try {
      const args = JSON.parse(process.env.CODEX_ARGS_JSON);
      if (!Array.isArray(args) || !args.every((entry) => typeof entry === "string")) throw new Error();
      return args;
    } catch {
      throw new Error("CODEX_ARGS_JSON must be a JSON array of strings");
    }
  }

  async start() {
    this.httpServer = http.createServer((request, response) => this.handleHttp(request, response));
    this.wsServer = new WebSocketServer({ noServer: true, maxPayload: MAX_WS_PAYLOAD });
    this.wsServer.on("connection", (socket, request) => this.handleConnection(socket, request));
    this.httpServer.on("upgrade", (request, socket, head) => this.handleUpgrade(request, socket, head));

    await new Promise((resolve, reject) => {
      const onError = (error) => reject(error);
      this.httpServer.once("error", onError);
      this.httpServer.listen(this.port, this.host, () => {
        this.httpServer.off("error", onError);
        resolve();
      });
    });

    if (this.spawnCodex) this.startCodex();
    return this.address();
  }

  address() {
    const address = this.httpServer.address();
    if (!address || typeof address === "string") return { host: this.host, port: this.port };
    return { host: address.address, port: address.port };
  }

  async stop() {
    for (const client of this.clients) client.socket.close(1001, "relay shutting down");
    this.clients.clear();
    this.hostClient = null;
    this.pendingHostCommands.clear();
    this.pendingAppRequests.clear();
    this.commandResults.clear();
    this.commandResultScopes.clear();
    this.hostCommandScope = null;
    for (const pending of this.pendingServerRequests.values()) {
      if (pending.timer) clearTimeout(pending.timer);
    }
    this.pendingServerRequests.clear();
    if (this.appProcess && !this.appProcess.killed) this.appProcess.kill("SIGTERM");
    if (this.wsServer) await new Promise((resolve) => this.wsServer.close(() => resolve()));
    if (this.httpServer) await new Promise((resolve) => this.httpServer.close(() => resolve()));
  }

  startCodex() {
    if (this.appProcess && this.appProcess.exitCode === null && !this.appProcess.killed) return;
    this.state.app = "starting";
    const child = spawn(this.codexCommand, this.codexArgs, {
      cwd: this.codexCwd,
      env: process.env,
      stdio: ["pipe", "pipe", "pipe"],
    });
    const generation = ++this.appGeneration;
    this.appProcess = child;

    child.stdout.setEncoding("utf8");
    child.stderr.setEncoding("utf8");
    child.stdout.on("data", (chunk) => {
      if (this.appProcess !== child || this.appGeneration !== generation) return;
      this.consumeAppOutput(chunk);
    });
    child.stderr.on("data", (chunk) => {
      if (this.appProcess !== child || this.appGeneration !== generation) return;
      const text = redactString(chunk).slice(0, 32_000);
      this.recordEvent("app.stderr", { stream: "stderr", text });
    });
    child.stdin.on("error", (error) => {
      if (this.appProcess !== child || this.appGeneration !== generation) return;
      this.handleAppProcessExit(child, generation, { error });
    });
    child.on("error", (error) => {
      this.handleAppProcessExit(child, generation, { error });
    });
    child.on("exit", (code, signal) => {
      this.handleAppProcessExit(child, generation, { code, signal });
    });

    this.state.app = "initializing";
    const initId = this.nextAppRequestId("initialize");
    this.pendingAppRequests.set(jsonRpcIdKey(initId), { kind: "initialize", method: "initialize" });
    try {
      this.sendToApp({
        method: "initialize",
        id: initId,
        params: {
          clientInfo: {
            name: "codex-remote-collab",
            title: "Codex Remote Collab",
            version: "0.1.0",
          },
          capabilities: {
            experimentalApi: true,
            requestAttestation: false,
          },
        },
      });
    } catch (error) {
      this.handleAppProcessExit(child, generation, { error });
    }
  }

  handleAppProcessExit(child, generation, details = {}) {
    if (this.appProcess !== child || this.appGeneration !== generation) return;
    // ChildProcess can emit both `error` and `exit`; process one terminal
    // transition so pending commands/requests are settled exactly once.
    if (this.appTerminalGeneration === generation) return;
    this.appTerminalGeneration = generation;
    this.appProcess = null;
    this.appBuffer = "";

    const previousThreadId = this.state.activeThreadId;
    const error = details.error;
    const code = details.code;
    const signal = details.signal;
    const reason = error?.message
      || `Codex app-server exited (code=${String(code)}, signal=${String(signal)})`;
    const terminalError = {
      code: error?.code ? String(error.code) : "app_exited",
      message: redactString(reason),
      retryable: true,
    };
    this.state.app = "offline";
    this.state.initialized = false;
    this.state.activeThreadId = null;
    this.state.activeTurnId = null;
    this.state.lastError = terminalError;
    // Results from a dead app-server cannot safely be reused after a restart:
    // a command may have applied side effects before the process crashed.
    this.clearCommandResults();
    this.recordEvent("app.exited", error
      ? { error: terminalError }
      : { code, signal, error: terminalError });

    const pendingCommands = [...this.pendingAppRequests.values()];
    this.pendingAppRequests.clear();
    for (const pending of pendingCommands) {
      if (!pending.commandId) continue;
      const payload = {
        commandId: pending.commandId,
        method: pending.method || null,
        ok: false,
        uncertain: true,
        retryable: true,
        error: terminalError,
      };
      this.cacheCommandResult(pending.commandId, payload, "embedded");
      const event = this.recordEvent("command.result", payload);
      if (pending.client) {
        this.sendControl(pending.client, { type: "command.result", ...payload, seq: event.seq });
      }
    }

    const pendingRequests = [...this.pendingServerRequests.values()];
    for (const pending of pendingRequests) {
      this.clearPendingServerRequest(pending.appId);
      const isInput = pending.method === "item/tool/requestUserInput"
        || pending.method === "mcpServer/elicitation/request";
      this.recordEvent(isInput ? "input.expired" : "approval.expired", {
        requestId: pending.appId,
        method: pending.method,
        reason: "Codex app-server exited",
        error: terminalError,
      }, { sessionId: previousThreadId || undefined });
    }
  }

  nextAppRequestId(label) {
    this.appRequestCounter += 1;
    return `relay-${label}-${this.appRequestCounter}`;
  }

  consumeAppOutput(chunk) {
    this.appBuffer += chunk;
    while (true) {
      const newline = this.appBuffer.indexOf("\n");
      if (newline < 0) break;
      const line = this.appBuffer.slice(0, newline).trim();
      this.appBuffer = this.appBuffer.slice(newline + 1);
      if (!line) continue;
      try {
        this.handleAppMessage(JSON.parse(line));
      } catch (error) {
        this.recordEvent("app.parse_error", {
          error: normalizeError(error),
          line: redactString(line).slice(0, 2_000),
        });
      }
    }
  }

  handleAppMessage(rawMessage) {
    const message = redact(rawMessage);
    if (message.id !== undefined && message.method) {
      const requestId = rawMessage.id;
      this.clearPendingServerRequest(requestId);
      const pending = {
        appId: requestId,
        method: message.method,
        params: message.params || {},
        createdAt: Date.now(),
      };
      this.scheduleServerRequestExpiry(requestId, pending);
      this.pendingServerRequests.set(jsonRpcIdKey(requestId), pending);
      this.recordEvent(eventTypeForAppMessage(message), {
        requestId,
        method: message.method,
        params: message.params || {},
      });
      return;
    }

    if (message.id !== undefined) {
      const key = jsonRpcIdKey(rawMessage.id);
      const pending = this.pendingAppRequests.get(key);
      this.pendingAppRequests.delete(key);

      if (pending && pending.kind === "initialize") {
        if (message.result) {
          this.initializedResult = message.result;
          this.state.app = "ready";
          this.state.initialized = true;
          // The app-server handshake is ordered: initialized follows the
          // successful initialize response. Some versions reject an early
          // notification or process it before capabilities are established.
          try {
            this.sendToApp({ method: "initialized", params: {} });
          } catch (error) {
            this.state.app = "error";
            this.state.initialized = false;
            this.state.lastError = normalizeError(error);
          }
          this.recordEvent("app.ready", { result: message.result });
        } else {
          this.state.app = "error";
          this.state.lastError = message.error || { message: "Codex initialization failed" };
          this.recordEvent("app.error", { error: this.state.lastError });
        }
        return;
      }

      if (pending && pending.method === "thread/start" && message.result?.thread?.id) {
        this.state.activeThreadId = message.result.thread.id;
        this.state.cwd = message.result.cwd || this.state.cwd;
      }
      if (pending && pending.method === "turn/start" && message.result?.turn?.id) {
        this.state.activeTurnId = message.result.turn.id;
      }

      const commandPayload = {
        commandId: pending?.commandId || null,
        method: pending?.method || null,
        ok: !message.error,
        result: message.result,
        error: message.error,
      };
      if (pending?.commandId) {
        this.cacheCommandResult(pending.commandId, commandPayload, "embedded");
      }
      const event = this.recordEvent("command.result", commandPayload);
      if (pending?.client) this.sendControl(pending.client, { ...commandPayload, type: "command.result", seq: event.seq });
      return;
    }

    this.updateStateFromNotification(message);
    this.recordEvent(eventTypeForAppMessage(message), {
      method: message.method || "unknown",
      params: message.params || {},
      text: outputText(message.method || "", message.params),
      emittedAtMs: message.emittedAtMs,
    });
  }

  updateStateFromNotification(message) {
    const params = message.params || {};
    if (message.method === "thread/started" && params.thread?.id) this.state.activeThreadId = params.thread.id;
    if (message.method === "turn/started" && params.turn?.id) this.state.activeTurnId = params.turn.id;
    if (message.method === "turn/completed" && (!params.turn?.id || params.turn.id === this.state.activeTurnId)) {
      this.state.activeTurnId = null;
    }
  }

  sendToApp(message) {
    if (!this.appProcess
      || this.appProcess.exitCode !== null
      || this.appProcess.killed
      || !this.appProcess.stdin
      || this.appProcess.stdin.destroyed
      || !this.appProcess.stdin.writable) {
      throw Object.assign(new Error("Codex app-server is offline"), {
        code: "app_offline",
        retryable: true,
      });
    }
    this.appProcess.stdin.write(`${JSON.stringify(message)}\n`);
  }

  recordEvent(type, payload, options = {}) {
    const event = {
      v: 1,
      kind: "event",
      id: randomId("evt"),
      seq: ++this.nextSeq,
      ts: new Date().toISOString(),
      type,
      sessionId: options.sessionId || this.state.activeThreadId,
      payload: redact(payload),
    };
    const replayEvent = compactTranscriptEventForReplay(event);
    const replayBytes = jsonByteLength(replayEvent);
    // An individual event that exceeds the entire replay budget is still
    // delivered live and represented by the authoritative state snapshot. Do
    // not let it make the in-memory ring exceed its configured hard bound.
    if (replayBytes <= this.eventByteLimit) {
      this.events.push(replayEvent);
      this.eventSizes.push(replayBytes);
      this.eventBytes += replayBytes;
    }
    while (this.events.length > this.eventLimit || this.eventBytes > this.eventByteLimit) {
      this.events.shift();
      this.eventBytes -= this.eventSizes.shift() || 0;
    }
    for (const client of this.clients) {
      if (client.authenticated && client.subscribed && client !== options.excludeClient) this.sendControl(client, event);
    }
    return event;
  }

  auditAction(client, action, details, outcome) {
    this.audit.push({
      id: randomId("audit"),
      ts: new Date().toISOString(),
      actor: client?.id || "http",
      role: client?.role || "unknown",
      action,
      details: redact(details),
      outcome,
    });
    if (this.audit.length > 500) this.audit.splice(0, this.audit.length - 500);
  }

  handleUpgrade(request, socket, head) {
    let requestUrl;
    try {
      requestUrl = new URL(request.url, `http://${request.headers.host || "localhost"}`);
    } catch {
      socket.destroy();
      return;
    }
    if (requestUrl.pathname !== "/ws" && requestUrl.pathname !== "/v1/connect") {
      socket.destroy();
      return;
    }
    if (!this.authRequired && !isLoopbackRequestHost(request)) {
      socket.write("HTTP/1.1 403 Forbidden\r\n\r\n");
      socket.destroy();
      return;
    }
    const origin = request.headers.origin;
    if (origin) {
      try {
        if (new URL(origin).host.toLowerCase() !== String(request.headers.host || "").toLowerCase()) {
          socket.write("HTTP/1.1 403 Forbidden\r\n\r\n");
          socket.destroy();
          return;
        }
      } catch {
        socket.destroy();
        return;
      }
    }
    this.wsServer.handleUpgrade(request, socket, head, (webSocket) => {
      this.wsServer.emit("connection", webSocket, request);
    });
  }

  handleConnection(socket, request) {
    const client = {
      id: randomId("client"),
      socket,
      role: null,
      authenticated: false,
      subscribed: false,
      clientType: null,
      sessionId: null,
      commandScope: null,
      lastSeq: 0,
      remoteAddress: request.socket.remoteAddress,
    };
    this.clients.add(client);
    const authTimer = setTimeout(() => {
      if (!client.authenticated) socket.close(1008, "authentication required");
    }, 10_000);
    authTimer.unref();

    socket.on("message", (data, isBinary) => {
      if (isBinary) {
        socket.close(1003, "JSON text frames only");
        return;
      }
      let message;
      try {
        message = JSON.parse(data.toString("utf8"));
      } catch {
        this.sendControl(client, { type: "error", code: "invalid_json", message: "Invalid JSON frame" });
        return;
      }
      if (!message || typeof message !== "object" || Array.isArray(message)) {
        this.sendControl(client, { type: "error", code: "invalid_frame", message: "JSON frame must be an object" });
        return;
      }
      try {
        this.handleClientMessage(client, message);
      } catch (error) {
        this.recordEvent("relay.error", { clientId: client.id, error: normalizeError(error) });
        this.sendControl(client, { type: "error", ...normalizeError(error) });
      }
    });
    socket.on("close", () => {
      clearTimeout(authTimer);
      this.clients.delete(client);
      if (this.hostClient === client) {
        this.hostClient = null;
        this.state.hostConnected = false;
        this.state.hostSessionId = null;
        this.state.app = "offline";
        this.state.initialized = false;
        this.state.activeThreadId = null;
        this.state.activeTurnId = null;
        this.state.lastError = { code: "host_disconnected", message: "VS Code host disconnected" };
        this.recordEvent("host.disconnected", { clientId: client.id }, { excludeClient: client });
        // Commands waiting on a host cannot be completed after its socket is
        // gone. Keep their ids reserved briefly so retries get a clear error.
        for (const [commandId, pending] of this.pendingHostCommands) {
          this.pendingHostCommands.delete(commandId);
          this.cacheCommandResult(commandId, {
            commandId,
            method: pending.method,
            ok: false,
            uncertain: true,
            error: { code: "host_disconnected", message: "VS Code host disconnected" },
          }, pending.commandScope || client.commandScope || this.hostCommandScope);
          if (pending.client) {
            if (pending.kind === "server-response") {
              this.sendControl(pending.client, {
                type: "response.rejected",
                requestId: pending.responseRequestId ?? pending.requestId,
                code: "host_disconnected",
                message: "VS Code host disconnected",
                retryable: true,
              });
            } else {
              this.sendControl(pending.client, {
                type: "command.result",
                commandId,
                method: pending.method,
                ok: false,
                uncertain: true,
                error: { code: "host_disconnected", message: "VS Code host disconnected" },
              });
            }
          }
        }
        for (const pending of this.pendingServerRequests.values()) {
          if (pending.source !== "host" || pending.hostClient !== client) continue;
          this.clearPendingServerRequest(pending.appId);
          this.recordEvent(
            pending.method === "item/tool/requestUserInput" || pending.method === "mcpServer/elicitation/request"
              ? "input.expired"
              : "approval.expired",
            { requestId: pending.appId, method: pending.method, reason: "VS Code host disconnected" },
            { sessionId: client.sessionId || undefined },
          );
        }
      }
      if (client.authenticated) this.recordEvent("presence.changed", { clientId: client.id, state: "offline" });
    });
    socket.on("error", () => {});
  }

  roleForToken(token) {
    if (secureEqual(token, this.operatorToken)) return "operator";
    if (secureEqual(token, this.viewerToken)) return "viewer";
    return null;
  }

  handleClientMessage(client, message) {
    if (!message || typeof message !== "object" || Array.isArray(message)) {
      this.sendControl(client, { type: "error", code: "invalid_frame", message: "JSON frame must be an object" });
      return;
    }
    if (!client.authenticated) {
      // The VS Code bridge sends a hello frame before its auth frame. Keep the
      // hello unauthenticated but remember the client kind and resume cursor.
      const localConnection = !this.authRequired && isLoopbackAddress(client.remoteAddress);
      const localNoAuthHandshake = localConnection
        && (message.kind === "hello" || message.kind === "auth" || message.type === "auth");
      let token = null;
      if (message.kind === "hello") {
        if (message.protocol !== undefined && Number(message.protocol) !== 1) {
          this.sendControl(client, { type: "error", code: "unsupported_protocol", message: "Only protocol 1 is supported" });
          client.socket.close(1002, "unsupported protocol");
          return;
        }
        client.clientType = message.clientType === "host" ? "host" : "web";
        client.sessionId = typeof message.sessionId === "string" ? message.sessionId : null;
        client.lastSeq = Number.isFinite(Number(message.lastSeq)) ? Number(message.lastSeq) : 0;
        token = typeof message.token === "string"
          ? message.token
          : typeof message.accessToken === "string"
            ? message.accessToken
            : null;
        if (!token && !localConnection) return;
      } else {
        token = message.type === "auth" && typeof message.token === "string"
          ? message.token
          : message.kind === "auth" && typeof message.accessToken === "string"
            ? message.accessToken
            : message.kind === "auth" && typeof message.token === "string"
              ? message.token
              : null;
      }
      if (!token && !localNoAuthHandshake) {
        client.socket.close(1008, "authentication required");
        return;
      }
      const isHost = client.clientType === "host";
      const role = localNoAuthHandshake
        ? (isHost ? "host" : "operator")
        : (isHost && secureEqual(token, this.hostToken) ? "operator" : this.roleForToken(token));
      if (!role || (!localNoAuthHandshake && isHost && !secureEqual(token, this.hostToken))) {
        this.auditAction(client, "authenticate", {}, "denied");
        client.socket.close(1008, "invalid token");
        return;
      }
      client.authenticated = true;
      client.clientType = client.clientType || "web";
      client.role = isHost ? "host" : role;
      if (client.clientType === "host") {
        if (this.mode === "embedded") {
          this.sendControl(client, { type: "error", code: "host_mode_disabled", message: "Start relay with CODEX_REMOTE_MODE=host (or CODEX_SPAWN=false) for a VS Code host" });
          client.socket.close(1008, "host mode disabled");
          return;
        }
        if (this.hostClient && this.hostClient !== client) {
          this.sendControl(client, { type: "error", code: "host_already_connected", message: "A VS Code host is already connected" });
          client.socket.close(1008, "host already connected");
          return;
        }
        const hostSessionId = typeof client.sessionId === "string" && client.sessionId.length > 0
          ? client.sessionId
          : null;
        const commandScope = hostSessionId ? `session:${hostSessionId}` : `connection:${client.id}`;
        // A command id is only idempotent within the same host session. A
        // reconnect with the same stable session id may reuse the cache;
        // another session must never inherit old results (including uncertain
        // disconnect results).
        if (this.hostCommandScope !== null && this.hostCommandScope !== commandScope) {
          this.clearCommandResults();
        }
        client.commandScope = commandScope;
        this.hostCommandScope = commandScope;
        if (this.state.hostSessionId && this.state.hostSessionId !== client.sessionId) {
          this.state.activeThreadId = null;
          this.state.activeTurnId = null;
        }
        this.hostClient = client;
        this.state.hostConnected = true;
        this.state.hostSessionId = client.sessionId;
        this.recordEvent("host.connected", { clientId: client.id, sessionId: client.sessionId }, { excludeClient: client });
      }
      this.auditAction(client, "authenticate", {}, "accepted");
      this.sendControl(client, {
        type: "auth.ok",
        clientId: client.id,
        role: client.role,
        clientType: client.clientType,
        protocol: 1,
        authRequired: this.authRequired,
        latestSeq: this.nextSeq,
      });
      return;
    }

    // Host frames use the versioned relay contract; browser frames use the
    // compact `type` contract. Host events are ingested and re-sequenced here
    // instead of echoed back to the host.
    if (message.kind === "hello") {
      const announcedType = message.clientType === "host" ? "host" : "web";
      if (announcedType !== client.clientType) {
        this.sendControl(client, { type: "error", code: "client_type_immutable", message: "clientType cannot change after authentication" });
        return;
      }
      client.sessionId = typeof message.sessionId === "string" ? message.sessionId : client.sessionId;
      return;
    }
    if (message.kind === "auth" || message.type === "auth") {
      return;
    }
    if (message.kind === "ack") return;
    if (message.kind === "event" && client.clientType === "host") {
      this.ingestHostEvent(client, message);
      return;
    }

    if (message.type === "subscribe") {
      this.subscribe(client, Number(message.fromSeq || 0));
      return;
    }
    if (message.type === "ping") {
      this.sendControl(client, { type: "pong", ts: new Date().toISOString(), latestSeq: this.nextSeq });
      return;
    }
    // Browser clients historically used compact `{type:"command", method,
    // params}` / `{type:"respond", requestId, result}` frames. The bridge
    // contract is versioned and uses `{kind:"command", type, payload}` (and
    // approval/input response command names). Normalize both forms at this
    // boundary; host clients are event producers and must not issue relay
    // commands back into themselves.
    if (client.clientType !== "host") {
      const command = normalizeBrowserCommand(message);
      if (command) {
        if (REMOTE_RESPONSE_METHODS.has(command.method)) {
          this.dispatchServerResponse(client, normalizeBrowserResponse(message, command.method));
        } else {
          this.dispatchCommand(client, command);
        }
        return;
      }
      const response = normalizeBrowserResponse(message);
      if (response) {
        this.dispatchServerResponse(client, response);
        return;
      }
    }
    this.sendControl(client, { type: "error", code: "unknown_frame", message: "Unknown frame type" });
  }

  subscribe(client, fromSeq) {
    const firstAvailable = this.events.length ? this.events[0].seq : this.nextSeq + 1;
    const replayEvents = [];
    let replayBytes = 0;
    let replayTooLarge = false;
    if (fromSeq + 1 >= firstAvailable) {
      for (const event of this.events) {
        if (event.seq <= fromSeq) continue;
        const size = jsonByteLength(event);
        if (replayBytes + size > this.replayByteLimit) {
          replayTooLarge = true;
          break;
        }
        replayEvents.push(event);
        replayBytes += size;
      }
    }
    if (fromSeq + 1 < firstAvailable || replayTooLarge) {
      this.sendControl(client, {
        type: "resync.required",
        requestedFromSeq: fromSeq,
        firstAvailableSeq: firstAvailable,
        ...(replayTooLarge ? { reason: "replay_too_large" } : {}),
      });
    } else {
      for (const event of replayEvents) this.sendControl(client, event);
    }
    client.subscribed = true;
    // Other subscribers need the presence transition, while the joining
    // client receives the same fact in the clients list of its snapshot. Add
    // it first so `latestSeq` covers every authoritative state transition.
    this.recordEvent("presence.changed", { clientId: client.id, role: client.role, state: "online" }, { excludeClient: client });
    this.sendControl(client, { type: "session.snapshot", snapshot: this.snapshot() });
  }

  ingestHostEvent(client, frame) {
    const sourceType = typeof frame.type === "string" ? frame.type : "app.notification";
    const sourcePayload = frame.payload && typeof frame.payload === "object" ? frame.payload : {};
    const sourceSeq = Number.isFinite(Number(frame.seq)) ? Number(frame.seq) : undefined;
    const sessionId = client.sessionId || frame.sessionId || this.state.hostSessionId || undefined;

    const executionStatus = sourcePayload.executionStatus && typeof sourcePayload.executionStatus === "object"
      ? sourcePayload.executionStatus
      : frame.status && typeof frame.status === "object"
        ? frame.status
        : sourcePayload.status && typeof sourcePayload.status === "object"
          ? sourcePayload.status
          : null;
    if (executionStatus) this.state.executionStatus = redact(executionStatus);

    // Keep the browser snapshot useful even though the host deliberately uses
    // a normalized event vocabulary instead of raw app-server notifications.
    if (sourceType === "session.created" && sourcePayload.thread && typeof sourcePayload.thread === "object") {
      const id = sourcePayload.thread.id;
      if (typeof id === "string") this.state.activeThreadId = id;
    }
    if (sourceType === "session.snapshot") {
      const threadId = sourcePayload.threadId || sourcePayload.thread?.id;
      const turnId = sourcePayload.turnId || sourcePayload.turn?.id;
      if (typeof threadId === "string") this.state.activeThreadId = threadId;
      if (typeof turnId === "string") this.state.activeTurnId = turnId;
      if (sourcePayload.threadId === null || sourcePayload.thread === null) this.state.activeThreadId = null;
      if (sourcePayload.turnId === null || sourcePayload.turn === null) this.state.activeTurnId = null;
      this.state.app = "ready";
      this.state.initialized = true;
      this.state.lastError = null;
      if (typeof sourcePayload.outputTail === "string") this.state.outputTail = sourcePayload.outputTail;
      if (Array.isArray(sourcePayload.messages)) this.state.messages = sourcePayload.messages;
      if (sourcePayload.metadata && typeof sourcePayload.metadata === "object" && !Array.isArray(sourcePayload.metadata)) {
        const metadata = sourcePayload.metadata;
        this.state.sessionMetadata = redactSessionMetadata({
          ...(typeof metadata.title === "string" ? { title: metadata.title } : {}),
          ...(typeof metadata.name === "string" ? { name: metadata.name } : {}),
          ...(typeof metadata.cwd === "string" ? { cwd: metadata.cwd } : {}),
          ...(typeof metadata.mode === "string" ? { mode: metadata.mode } : {}),
          ...(metadata.controlMode === "sync" || metadata.controlMode === "async" ? { controlMode: metadata.controlMode } : {}),
          ...(Number.isSafeInteger(metadata.modeEpoch) && metadata.modeEpoch >= 0 ? { modeEpoch: metadata.modeEpoch } : {}),
          ...(metadata.capabilities && typeof metadata.capabilities === "object" && !Array.isArray(metadata.capabilities) ? { capabilities: metadata.capabilities } : {}),
          ...(typeof metadata.source === "string" ? { source: metadata.source } : {}),
          ...(typeof metadata.historyComplete === "boolean" ? { historyComplete: metadata.historyComplete } : {}),
          ...(typeof metadata.waitingForSession === "boolean" ? { waitingForSession: metadata.waitingForSession } : {}),
          ...(typeof metadata.attachReady === "boolean" ? { attachReady: metadata.attachReady } : {}),
          ...(typeof metadata.model === "string" ? { model: metadata.model } : {}),
          ...(typeof metadata.latestModel === "string" ? { latestModel: metadata.latestModel } : {}),
          ...(typeof metadata.effort === "string" || metadata.effort === null ? { effort: metadata.effort } : {}),
          ...(typeof metadata.latestReasoningEffort === "string" || metadata.latestReasoningEffort === null ? { latestReasoningEffort: metadata.latestReasoningEffort } : {}),
          ...(typeof metadata.modelName === "string" ? { modelName: metadata.modelName } : {}),
          ...(typeof metadata.modelProvider === "string" ? { modelProvider: metadata.modelProvider } : {}),
          ...(typeof metadata.approvalPolicy === "string" ? { approvalPolicy: metadata.approvalPolicy } : {}),
          ...(typeof metadata.approvalsReviewer === "string" ? { approvalsReviewer: metadata.approvalsReviewer } : {}),
          ...(typeof metadata.sandboxPolicy === "string" ? { sandboxPolicy: metadata.sandboxPolicy } : {}),
          ...(metadata.approvalPolicy && typeof metadata.approvalPolicy === "object" ? { approvalPolicy: metadata.approvalPolicy } : {}),
          ...(metadata.approvalsReviewer === null ? { approvalsReviewer: null } : {}),
          ...(metadata.sandboxPolicy && typeof metadata.sandboxPolicy === "object" ? { sandboxPolicy: metadata.sandboxPolicy } : {}),
          ...(typeof metadata.permissions === "string" || (metadata.permissions && typeof metadata.permissions === "object") || metadata.permissions === null ? { permissions: metadata.permissions } : {}),
          ...(typeof metadata.currentPermissions === "string" || (metadata.currentPermissions && typeof metadata.currentPermissions === "object") || metadata.currentPermissions === null ? { currentPermissions: metadata.currentPermissions } : {}),
          ...(Array.isArray(metadata.runtimeWorkspaceRoots) ? { runtimeWorkspaceRoots: metadata.runtimeWorkspaceRoots } : {}),
          ...(typeof metadata.workedDurationMs === "number" ? { workedDurationMs: metadata.workedDurationMs } : {}),
          ...(typeof metadata.firstTurnWorkItemStartedAtMs === "number" ? { firstTurnWorkItemStartedAtMs: metadata.firstTurnWorkItemStartedAtMs } : {}),
          ...(typeof metadata.finalAssistantStartedAtMs === "number" ? { finalAssistantStartedAtMs: metadata.finalAssistantStartedAtMs } : {}),
          ...(metadata.tokenUsage && typeof metadata.tokenUsage === "object" ? { tokenUsage: metadata.tokenUsage } : metadata.tokenUsage === null ? { tokenUsage: null } : {}),
          ...(metadata.latestTokenUsageInfo && typeof metadata.latestTokenUsageInfo === "object" ? { latestTokenUsageInfo: metadata.latestTokenUsageInfo } : metadata.latestTokenUsageInfo === null ? { latestTokenUsageInfo: null } : {}),
          ...(metadata.threadSettings && typeof metadata.threadSettings === "object" ? { threadSettings: metadata.threadSettings } : {}),
          ...(Array.isArray(metadata.availableModels) ? { availableModels: metadata.availableModels } : {}),
          ...(Array.isArray(metadata.models) ? { models: metadata.models } : {}),
          ...(Array.isArray(metadata.subagents) ? { subagents: metadata.subagents } : {}),
          ...(typeof metadata.parentThreadId === "string" ? { parentThreadId: metadata.parentThreadId } : {}),
          ...(typeof metadata.agentNickname === "string" ? { agentNickname: metadata.agentNickname } : {}),
          ...(typeof metadata.agentRole === "string" ? { agentRole: metadata.agentRole } : {}),
        });
      }
      if (Array.isArray(sourcePayload.subagents)) this.state.subagents = redact(sourcePayload.subagents);
      else if (Array.isArray(sourcePayload.metadata?.subagents)) this.state.subagents = redact(sourcePayload.metadata.subagents);
      for (const request of Array.isArray(sourcePayload.pendingRequests) ? sourcePayload.pendingRequests : []) {
        if (!request || typeof request !== "object" || request.requestId === undefined) continue;
        const requestId = request.requestId;
        this.clearPendingServerRequest(requestId);
        const pending = {
          appId: requestId,
          method: typeof request.method === "string" ? request.method : "server.request",
          params: request.params && typeof request.params === "object" ? request.params : {},
          ...(typeof request.risk === "string" ? { risk: request.risk } : {}),
          ...(typeof request.summary === "string" ? { summary: request.summary } : {}),
          createdAt: Number.isFinite(Number(request.createdAt)) ? Number(request.createdAt) : Date.now(),
          ...(Number.isFinite(Number(request.expiresAt)) ? { expiresAt: Number(request.expiresAt) } : {}),
          source: "host",
          hostClient: client,
          commandHash: typeof request.commandHash === "string" ? request.commandHash : undefined,
        };
        if (pending.expiresAt && pending.expiresAt <= Date.now()) continue;
        this.pendingServerRequests.set(jsonRpcIdKey(requestId), pending);
      }
    }
    if (sourceType === "session.switching") {
      const targetThreadId = sourcePayload.targetThreadId || sourcePayload.threadId;
      if (typeof targetThreadId === "string") this.state.activeThreadId = targetThreadId;
      // The old transcript belongs to the previous thread. Clear it before
      // the target's authoritative snapshot arrives so a remote picker never
      // briefly renders messages from two sessions together.
      this.state.activeTurnId = null;
      this.state.outputTail = "";
      this.state.messages = [];
      this.state.subagents = [];
      this.state.sessionMetadata = null;
      this.state.executionStatus = null;
    }
    if (sourceType === "session.selected") {
      const selectedThreadId = sourcePayload.threadId || sourcePayload.activeThreadId;
      if (typeof selectedThreadId === "string") this.state.activeThreadId = selectedThreadId;
    }
    if (sourceType === "output.snapshot") {
      if (typeof sourcePayload.text === "string") this.state.outputTail = sourcePayload.text;
      if (Array.isArray(sourcePayload.messages)) this.state.messages = sourcePayload.messages;
      if (Array.isArray(sourcePayload.subagents)) this.state.subagents = redact(sourcePayload.subagents);
      if (sourcePayload.metadata && typeof sourcePayload.metadata === "object" && !Array.isArray(sourcePayload.metadata)) {
        // Output snapshots from older hosts occasionally carry the metadata
        // projection instead of a separate session.snapshot event. Preserve
        // the safe projection so model, permission, and usage controls remain
        // available after reconnect.
        this.state.sessionMetadata = redactSessionMetadata(sourcePayload.metadata);
      }
    } else if (sourceType === "output.chunk") {
      // New attach adapters carry the complete role-aware projection alongside
      // the append-only delta. Preserve both so reconnects do not flatten
      // reasoning, tools, edits, or Markdown into one assistant transcript.
      if (typeof sourcePayload.outputTail === "string") this.state.outputTail = sourcePayload.outputTail;
      else if (typeof sourcePayload.text === "string") this.state.outputTail = `${this.state.outputTail || ""}${sourcePayload.text}`.slice(-32_000);
      if (Array.isArray(sourcePayload.messages)) this.state.messages = sourcePayload.messages;
      else {
        const patchedMessages = applyStructuredMessagesPatch(this.state.messages, sourcePayload.messagesPatch);
        if (patchedMessages) this.state.messages = patchedMessages;
      }
      if (Array.isArray(sourcePayload.subagents)) this.state.subagents = redact(sourcePayload.subagents);
      if (sourcePayload.metadata && typeof sourcePayload.metadata === "object" && !Array.isArray(sourcePayload.metadata)) {
        this.state.sessionMetadata = redactSessionMetadata(sourcePayload.metadata);
      }
    }
    if (sourceType === "task.started") {
      const id = sourcePayload.turnId || (sourcePayload.turn && sourcePayload.turn.id);
      if (typeof id === "string") this.state.activeTurnId = id;
      if (typeof sourcePayload.threadId === "string") this.state.activeThreadId = sourcePayload.threadId;
    }
    if (sourceType === "task.finished" || sourceType === "task.cancelled") {
      this.state.activeTurnId = null;
    }

    if (sourceType === "connection.opened") {
      this.state.app = "ready";
      this.state.initialized = true;
      this.state.hostConnected = true;
      this.state.lastError = null;
    } else if (sourceType === "connection.closed") {
      // A replaced host socket can have one frame already queued in the
      // transport. A stale close must not mark the newly connected host
      // offline; acknowledge it so the old bridge does not retry forever.
      if (this.hostClient !== client) {
        if (sourceSeq !== undefined) {
          this.sendControl(client, { v: 1, kind: "ack", sessionId: sessionId || "", seq: sourceSeq });
        }
        return;
      }
      this.state.app = "offline";
      this.state.initialized = false;
      this.state.activeTurnId = null;
      this.state.outputTail = "";
      this.state.messages = [];
      this.state.subagents = [];
      this.state.sessionMetadata = null;
      this.state.executionStatus = null;
      this.state.lastError = {
        code: "app_unavailable",
        message: typeof sourcePayload.message === "string"
          ? redactString(sourcePayload.message)
          : "VS Code host app-server disconnected",
        retryable: true,
      };
      // This event is emitted by the authenticated host bridge when its local
      // app-server exits. The relay socket remains usable, so clean only the
      // app-scoped pending work here; transport close has its own handler.
      this.handleHostAppUnavailable(client, sessionId, this.state.lastError.message);
    }

    // RelayHost emits normalized approval/input events and keeps the original
    // app-server request id in payload. Store it centrally so exactly one
    // browser response can be routed back to that host.
    if (sourceType === "approval.requested"
      || sourceType === "input.requested"
      || sourceType === "server.requested"
      || sourceType === "server.request") {
      const requestId = sourcePayload.requestId;
      if (requestId !== undefined) {
        const key = jsonRpcIdKey(requestId);
        this.clearPendingServerRequest(requestId);
        const pending = {
          appId: requestId,
          method: typeof sourcePayload.method === "string" ? sourcePayload.method : sourceType,
          params: sourcePayload.params || sourcePayload,
          commandHash: typeof sourcePayload.commandHash === "string" ? sourcePayload.commandHash : undefined,
          risk: typeof sourcePayload.risk === "string" ? sourcePayload.risk : undefined,
          summary: typeof sourcePayload.summary === "string" ? sourcePayload.summary : undefined,
          expiresAt: Number.isFinite(Number(sourcePayload.expiresAt)) ? Number(sourcePayload.expiresAt) : undefined,
          createdAt: Date.now(),
          source: "host",
          hostClient: client,
        };
        // The VS Code adapter owns its local approval timer. Keeping a second
        // timer in the relay would race the adapter's JSON-RPC response.
        this.pendingServerRequests.set(key, pending);
      }
    }
    if (sourceType === "approval.expired" || sourceType === "input.expired" || sourceType === "server.expired") {
      const requestId = sourcePayload.requestId;
      if (requestId !== undefined) {
        const key = jsonRpcIdKey(requestId);
        const pending = this.pendingServerRequests.get(key);
        if (pending?.source === "host" && pending.hostClient === client) {
          this.clearPendingServerRequest(pending.appId);
        }
      }
    }
    if (sourceType === "approval.resolved"
      || sourceType === "input.resolved"
      || sourceType === "server.responded"
      || sourceType === "server.resolved") {
      const requestId = sourcePayload.requestId;
      if (requestId !== undefined) this.clearPendingServerRequest(requestId);
    }

    if (sourceType === "command.accepted" || sourceType === "command.rejected" || sourceType === "command.result") {
      const commandId = sourcePayload.commandId;
      const pending = commandId ? this.pendingHostCommands.get(String(commandId)) : undefined;
      const ok = sourceType === "command.accepted" ? sourcePayload.ok !== false : sourcePayload.ok === true;
      const resultPayload = {
        commandId: commandId || null,
        method: sourcePayload.method || null,
        ok,
        result: sourcePayload.result,
        error: sourcePayload.error,
        sourceSeq,
      };
      if (resultPayload.result && typeof resultPayload.result === "object") {
        const result = resultPayload.result;
        if (result.thread && typeof result.thread.id === "string") this.state.activeThreadId = result.thread.id;
        if (typeof result.threadId === "string") this.state.activeThreadId = result.threadId;
        if (typeof result.activeThreadId === "string") this.state.activeThreadId = result.activeThreadId;
        if (typeof result.selectedThreadId === "string") this.state.activeThreadId = result.selectedThreadId;
        if (result.turn && typeof result.turn.id === "string") this.state.activeTurnId = result.turn.id;
      }
      if (commandId) {
        this.pendingHostCommands.delete(String(commandId));
        // A late terminal frame from an app that already reported
        // connection.closed must not repopulate the cache we just invalidated.
        if (pending || this.state.app !== "offline") {
          this.cacheCommandResult(
            String(commandId),
            resultPayload,
            pending?.commandScope || client.commandScope || this.hostCommandScope,
          );
        }
      }
      const event = this.recordEvent("command.result", resultPayload, { sessionId });
      if (pending?.kind === "server-response") {
        const requestId = pending.responseRequestId ?? pending.requestId;
        if (resultPayload.ok) {
          this.clearPendingServerRequest(pending.requestId);
          const responseEvent = this.recordEvent("server.responded", {
            requestId,
            method: pending.method,
            ok: true,
          }, { sessionId });
          this.sendControl(pending.client, { type: "response.accepted", requestId, seq: responseEvent.seq });
        } else {
          this.sendControl(pending.client, {
            type: "response.rejected",
            requestId,
            code: "host_rejected",
            message: responseErrorMessage(resultPayload.error || "VS Code host rejected the response"),
            retryable: true,
          });
        }
      } else if (pending?.client) {
        this.sendControl(pending.client, { type: "command.result", ...resultPayload, seq: event.seq });
      }
      return;
    }

    const payload = {
      ...sourcePayload,
      ...((sourceType === "approval.requested"
        || sourceType === "input.requested"
        || sourceType === "server.requested"
        || sourceType === "server.request") && !sourcePayload.params
        ? { params: sourcePayload }
        : {}),
      source: "vscode-host",
      ...(sourceSeq !== undefined ? { sourceSeq } : {}),
      ...(frame.raw !== undefined ? { raw: redact(frame.raw) } : {}),
    };
    const event = this.recordEvent(sourceType, payload, { sessionId });
    // RelayHost sends event frames to its own relay transport and expects an
    // ack. Acknowledge only after the frame has been accepted into our ring.
    if (sourceSeq !== undefined) {
      this.sendControl(client, { v: 1, kind: "ack", sessionId: sessionId || "", seq: sourceSeq });
    }
    return event;
  }

  handleHostAppUnavailable(client, sessionId, reason = "VS Code host app-server unavailable") {
    // A delayed frame from an older host socket must never tear down the
    // pending work or cache belonging to the currently authenticated host.
    if (this.hostClient !== client) return;
    const terminalError = {
      code: "app_unavailable",
      message: redactString(reason),
      retryable: true,
    };

    // A local app-server crash invalidates both completed cache entries and
    // in-flight host commands. Report in-flight commands as uncertain to the
    // originating browser, but do not cache them: a retry after recovery must
    // be explicit rather than silently replaying an unknown operation.
    this.clearCommandResults();
    for (const [commandId, pending] of [...this.pendingHostCommands]) {
      if (pending.hostClient && pending.hostClient !== client) continue;
      if (!pending.hostClient && pending.commandScope && pending.commandScope !== client.commandScope) continue;
      this.pendingHostCommands.delete(commandId);
      if (pending.kind === "server-response") {
        this.sendControl(pending.client, {
          type: "response.rejected",
          requestId: pending.responseRequestId ?? pending.requestId,
          code: terminalError.code,
          message: terminalError.message,
          retryable: true,
        });
        continue;
      }
      const payload = {
        commandId,
        method: pending.method || null,
        ok: false,
        uncertain: true,
        retryable: true,
        error: terminalError,
      };
      const event = this.recordEvent("command.result", payload, { sessionId });
      if (pending.client) this.sendControl(pending.client, { type: "command.result", ...payload, seq: event.seq });
    }

    // Host approval/input requests are owned by the adapter, so the relay does
    // not run a second expiry timer. Once the adapter reports its app process
    // unavailable, remove every request tied to this host immediately.
    for (const pending of [...this.pendingServerRequests.values()]) {
      if (pending.source !== "host" || pending.hostClient !== client) continue;
      this.clearPendingServerRequest(pending.appId);
      const isInput = pending.method === "item/tool/requestUserInput"
        || pending.method === "mcpServer/elicitation/request";
      this.recordEvent(isInput ? "input.expired" : "approval.expired", {
        requestId: pending.appId,
        method: pending.method,
        reason: terminalError.message,
        error: terminalError,
      }, { sessionId });
    }
  }

  dispatchCommand(client, message) {
    const commandId = String(message.commandId || "");
    const method = String(message.method || "");
    if (!commandId || commandId.length > 128) {
      return this.commandRejected(client, commandId, "invalid_command_id", "commandId is required");
    }
    if (!ALLOWED_METHODS.has(method)) {
      return this.commandRejected(client, commandId, "method_not_allowed", `Method ${method || "(empty)"} is not allowed`);
    }
    if (MUTATING_METHODS.has(method) && client.role !== "operator") {
      this.auditAction(client, method, { commandId }, "denied");
      return this.commandRejected(client, commandId, "forbidden", "Operator token required");
    }
    const cached = this.getCachedCommandResult(commandId);
    if (cached) {
      this.sendControl(client, { type: "command.result", ...cached, cached: true });
      return { accepted: true, cached: true };
    }
    for (const pending of this.pendingHostCommands.values()) {
      if (pending.commandId === commandId) {
        this.sendControl(client, { type: "command.accepted", commandId, method, duplicate: true });
        return { accepted: true, duplicate: true };
      }
    }
    for (const pending of this.pendingAppRequests.values()) {
      if (pending.commandId === commandId) {
        this.sendControl(client, { type: "command.accepted", commandId, method, duplicate: true });
        return { accepted: true, duplicate: true };
      }
    }

    if (method === "initialize") {
      if (!this.state.initialized) {
        return this.commandRejected(client, commandId, "app_initializing", "Codex is still initializing", true);
      }
      const result = {
        commandId,
        method,
        ok: true,
        result: this.mode === "host"
          ? { protocol: 1, mode: "host", hostConnected: this.state.hostConnected, sessionId: this.state.hostSessionId }
          : this.initializedResult,
        cachedAt: Date.now(),
      };
      this.cacheCommandResult(commandId, result, this.mode === "host" ? this.hostCommandScope : "embedded");
      this.sendControl(client, { type: "command.result", ...result, cached: true });
      return { accepted: true, cached: true };
    }

    if (!this.state.initialized) {
      return this.commandRejected(client, commandId, "app_not_ready", "Codex app-server is not ready", true);
    }
    if (!message.params || typeof message.params !== "object" || Array.isArray(message.params)) {
      return this.commandRejected(client, commandId, "invalid_params", "params must be an object");
    }

    const validationError = this.validateCommand(method, message.params);
    if (validationError) return this.commandRejected(client, commandId, "invalid_params", validationError);

    // This MVP exposes one active Codex turn per relay session. Keeping the
    // check at the relay boundary prevents two browser operators from racing
    // a turn start or steering an outdated turn id.
    const turnStartPending = [...this.pendingAppRequests.values()].some((pending) => pending.method === "turn/start")
      || [...this.pendingHostCommands.values()].some((pending) => pending.method === "turn/start");
    if (method === "turn/start" && (this.state.activeTurnId || turnStartPending)) {
      return this.commandRejected(client, commandId, "turn_active", "A Codex turn is already active", true);
    }
    if (method === "session/select" || method === "control/mode/set") {
      const pendingMethod = method === "session/select" ? "session/select" : "control/mode/set";
      const sessionSwitchPending = [...this.pendingHostCommands.values()].some((pending) => pending.method === pendingMethod)
        || [...this.pendingAppRequests.values()].some((pending) => pending.method === pendingMethod);
      if (sessionSwitchPending) {
        return this.commandRejected(client, commandId, method === "session/select" ? "session_switch_pending" : "mode_switch_pending", method === "session/select" ? "A session switch is already in progress" : "A control mode switch is already in progress", true);
      }
      if (this.state.activeTurnId || this.pendingServerRequests.size) {
        return this.commandRejected(client, commandId, method === "session/select" ? "session_busy" : "mode_busy", "The active session has a running turn or pending request", true);
      }
    }
    if (method === "turn/steer" && this.state.activeTurnId && message.params.expectedTurnId !== this.state.activeTurnId) {
      return this.commandRejected(client, commandId, "stale_turn", "expectedTurnId does not match the active turn", true);
    }
    if (method === "turn/interrupt" && this.state.activeTurnId && message.params.turnId !== this.state.activeTurnId) {
      return this.commandRejected(client, commandId, "stale_turn", "turnId does not match the active turn", true);
    }

    // A connected VS Code bridge is the source of truth for the session. The
    // relay never runs a second app-server request for the same command.
    if (this.hostClient && this.hostClient.socket.readyState === WebSocket.OPEN) {
      const hostFrame = {
        v: 1,
        kind: "command",
        type: method,
        commandId,
        sessionId: this.hostClient.sessionId || undefined,
        actor: { id: client.id || "web", role: client.role },
        payload: message.params,
      };
      this.pendingHostCommands.set(commandId, {
        commandId,
        method,
        client,
        hostClient: this.hostClient,
        commandScope: this.hostCommandScope || this.hostClient.commandScope || null,
        createdAt: Date.now(),
      });
      try {
        this.hostClient.socket.send(JSON.stringify(hostFrame));
        this.auditAction(client, method, { commandId, params: message.params, target: "vscode-host" }, "forwarded");
        this.sendControl(client, { type: "command.accepted", commandId, method, target: "vscode-host" });
        return { accepted: true, commandId, method, target: "vscode-host" };
      } catch (error) {
        this.pendingHostCommands.delete(commandId);
        return this.commandRejected(client, commandId, "host_unavailable", error.message, true);
      }
    }

    const appId = this.nextAppRequestId("command");
    try {
      this.pendingAppRequests.set(jsonRpcIdKey(appId), {
        kind: "command",
        commandId,
        method,
        client,
        createdAt: Date.now(),
      });
      this.sendToApp({ method, id: appId, params: message.params });
      this.auditAction(client, method, { commandId, params: message.params }, "forwarded");
      this.sendControl(client, { type: "command.accepted", commandId, method });
      return { accepted: true, commandId, method };
    } catch (error) {
      this.pendingAppRequests.delete(jsonRpcIdKey(appId));
      return this.commandRejected(client, commandId, error.code || "app_offline", error.message, error.retryable);
    }
  }

  validateCommand(method, params) {
    if (method === "control/mode/get") {
      if (Object.keys(params).length > 0) return "control/mode/get does not accept parameters";
    }
    if (method === "control/mode/set") {
      if (params.mode !== "sync" && params.mode !== "async") return "mode must be sync or async";
    }
    if (method === "session/list") {
      if (params.limit !== undefined && (!Number.isInteger(params.limit) || params.limit < 1 || params.limit > 100)) {
        return "limit must be an integer between 1 and 100";
      }
    }
    if (method === "session/select") {
      const threadId = params.threadId ?? params.conversationId;
      if (typeof threadId !== "string" || !threadId.trim()) return "threadId is required";
      if (threadId.length > 256) return "threadId is too long";
    }
    if (method === "thread/start") {
      const allowedSandboxes = new Set(["read-only", "workspace-write", "danger-full-access"]);
      if (params.sandbox != null && !allowedSandboxes.has(params.sandbox)) {
        return "sandbox must be read-only, workspace-write, or danger-full-access";
      }
      if (params.cwd != null && typeof params.cwd !== "string") return "cwd must be a string";
    }
    if (method === "turn/start") {
      if (typeof params.threadId !== "string" || !params.threadId) return "threadId is required";
      if (!Array.isArray(params.input) || params.input.length === 0) return "input must be a non-empty array";
    }
    if (method === "thread/settings/update") {
      if (typeof params.threadId !== "string" || !params.threadId) return "threadId is required";
      const settings = params.threadSettings ?? params.settings;
      if (!settings || typeof settings !== "object" || Array.isArray(settings)) return "threadSettings must be an object";
      if (settings.model !== undefined && typeof settings.model !== "string") return "threadSettings.model must be a string";
      // `null` is the official value for clearing a model's reasoning effort
      // (some models do not expose a selectable effort). Preserve it through
      // the relay instead of rejecting a valid next-turn update.
      if (settings.effort !== undefined && settings.effort !== null && typeof settings.effort !== "string") return "threadSettings.effort must be a string or null";
      for (const key of ["sandboxPolicy", "approvalPolicy"]) {
        const value = settings[key];
        if (value !== undefined && value !== null && typeof value !== "string" && (typeof value !== "object" || Array.isArray(value))) {
          return `threadSettings.${key} must be a string, object, or null`;
        }
      }
      if (settings.approvalsReviewer !== undefined && settings.approvalsReviewer !== null && typeof settings.approvalsReviewer !== "string") {
        return "threadSettings.approvalsReviewer must be a string or null";
      }
      if (settings.runtimeWorkspaceRoots !== undefined && settings.runtimeWorkspaceRoots !== null
        && (!Array.isArray(settings.runtimeWorkspaceRoots) || !settings.runtimeWorkspaceRoots.every((entry) => typeof entry === "string"))) {
        return "threadSettings.runtimeWorkspaceRoots must be an array of strings or null";
      }
      if (settings.permissions !== undefined && settings.permissions !== null
        && (typeof settings.permissions !== "string" && (typeof settings.permissions !== "object" || Array.isArray(settings.permissions)))) {
        return "threadSettings.permissions must be a string, object, or null";
      }
    }
    if (method === "turn/steer") {
      if (typeof params.threadId !== "string" || !params.threadId) return "threadId is required";
      if (typeof params.expectedTurnId !== "string" || !params.expectedTurnId) return "expectedTurnId is required";
      if (!Array.isArray(params.input) || params.input.length === 0) return "input must be a non-empty array";
    }
    if (method === "turn/interrupt") {
      if (typeof params.threadId !== "string" || !params.threadId) return "threadId is required";
      if (typeof params.turnId !== "string" || !params.turnId) return "turnId is required";
    }
    return null;
  }

  commandRejected(client, commandId, code, message, retryable = false) {
    const payload = { type: "command.rejected", commandId: commandId || null, code, message, retryable: Boolean(retryable) };
    this.sendControl(client, payload);
    return { accepted: false, ...payload };
  }

  scheduleServerRequestExpiry(requestId, pending) {
    if (this.approvalTimeoutMs <= 0) return;
    pending.timer = setTimeout(() => this.expireServerRequest(requestId), this.approvalTimeoutMs);
    pending.timer.unref?.();
  }

  expireServerRequest(requestId) {
    const pending = this.clearPendingServerRequest(requestId);
    if (!pending) return;
    const canonicalRequestId = pending.appId;
    const isInput = pending.method === "item/tool/requestUserInput" || pending.method === "mcpServer/elicitation/request";
    const reason = "Remote approval timed out";
    this.recordEvent(isInput ? "input.expired" : "approval.expired", {
      requestId: canonicalRequestId,
      method: pending.method,
      reason,
    }, { sessionId: pending.hostClient?.sessionId || undefined });
    this.auditAction({ id: "relay", role: "system" }, pending.method, { requestId: canonicalRequestId }, "expired");

    const result = defaultServerResponse(pending.method, reason);
    if (pending.source === "host") {
      if (!pending.hostClient || pending.hostClient.socket.readyState !== WebSocket.OPEN) return;
      const commandMethod = isInput ? "server.request.respond" : "approval.respond";
      try {
        pending.hostClient.socket.send(JSON.stringify({
          v: 1,
          kind: "command",
          type: commandMethod,
          commandId: randomId("timeout"),
          sessionId: pending.hostClient.sessionId || undefined,
          actor: { id: "relay", role: "system" },
          payload: {
            requestId: pending.appId,
            decision: "deny",
            response: result,
            reason,
          },
        }));
      } catch {
        // The local adapter also has its own expiry deny; no retry is needed.
      }
      return;
    }

    try {
      this.sendToApp({ id: pending.appId, result });
    } catch {
      // The process may have exited while the approval was pending.
    }
  }

  clearPendingServerRequest(requestId) {
    const key = jsonRpcIdKey(requestId);
    const pending = this.pendingServerRequests.get(key);
    if (pending?.timer) clearTimeout(pending.timer);
    this.pendingServerRequests.delete(key);
    return pending;
  }

  dispatchServerResponse(client, message) {
    const requestId = message.requestId ?? message.id ?? "";
    const requestKey = findTypedMapKey(this.pendingServerRequests, requestId, (value) => value?.appId, true);
    if (client.role !== "operator") {
      this.auditAction(client, "server-response", { requestId }, "denied");
      this.sendControl(client, { type: "response.rejected", requestId, code: "forbidden", message: "Operator token required" });
      return { accepted: false, code: "forbidden" };
    }
    const pending = this.pendingServerRequests.get(requestKey);
    if (!pending) {
      this.sendControl(client, { type: "response.rejected", requestId, code: "unknown_request", message: "Request is no longer pending" });
      return { accepted: false, code: "unknown_request" };
    }
    if (!("result" in message) && !("error" in message)) {
      this.sendControl(client, { type: "response.rejected", requestId, code: "invalid_response", message: "result or error is required" });
      return { accepted: false, code: "invalid_response" };
    }
    const remoteResponseAllowed = SERVER_REQUEST_METHODS.has(pending.method)
      || pending.method === "item/tool/requestUserInput"
      || pending.method === "mcpServer/elicitation/request";
    if (!remoteResponseAllowed) {
      this.sendControl(client, {
        type: "response.rejected",
        requestId,
        code: "unsupported_request",
        message: "This app-server request must be handled by the host",
      });
      return { accepted: false, code: "unsupported_request" };
    }

    const normalizedResult = Object.prototype.hasOwnProperty.call(message, "result")
      ? normalizeServerResponseForApp(pending.method, message.result)
      : undefined;
    if (Object.prototype.hasOwnProperty.call(message, "result")
      && !isValidApprovalResponse(pending.method, normalizedResult)) {
      this.sendControl(client, {
        type: "response.rejected",
        requestId,
        code: "invalid_response",
        message: "Unsupported or malformed approval decision",
      });
      return { accepted: false, code: "invalid_response" };
    }
    if (Object.prototype.hasOwnProperty.call(message, "requestedDecision")
      && (pending.method === "item/commandExecution/requestApproval"
        || pending.method === "item/fileChange/requestApproval"
        || pending.method === "applyPatchApproval"
        || pending.method === "execCommandApproval")) {
      const requested = approvalDecisionKind(message.requestedDecision);
      const actual = approvalDecisionForResult(normalizedResult);
      if (!requested || requested !== actual) {
        this.sendControl(client, {
          type: "response.rejected",
          requestId,
          code: "decision_mismatch",
          message: "Outer approval decision does not match the response",
        });
        return { accepted: false, code: "decision_mismatch" };
      }
    }

    // Host-proxy mode uses the bridge's normalized command contract. Keep the
    // original app-server request id in the payload, but do not forward an
    // arbitrary JSON-RPC response as a relay command.
    if (pending.source === "host") {
      const result = normalizedResult;
      const error = Object.prototype.hasOwnProperty.call(message, "error") ? message.error : undefined;
      const method = pending.method || "";
      const isInput = method === "item/tool/requestUserInput" || method === "mcpServer/elicitation/request";
      const commandMethod = isInput ? "server.request.respond" : "approval.respond";
      const decision = error
        ? "deny"
        : isInput
          ? "allow"
          : approvalDecisionForResult(result);
      // Generate the host command from the canonical app-server id. This
      // preserves the legacy `response-77` shape when a browser merely
      // stringified a numeric id, while still suffixing ids when both typed
      // variants are pending concurrently.
      const commandId = responseCommandId(pending.appId, this.pendingServerRequests, this.pendingHostCommands);
      const hostFrame = {
        v: 1,
        kind: "command",
        type: commandMethod,
        commandId,
        sessionId: pending.hostClient?.sessionId || undefined,
        actor: { id: client.id || "web", role: client.role },
          payload: {
            requestId: pending.appId,
            decision,
            ...(pending.commandHash ? { commandHash: pending.commandHash } : {}),
            ...(result !== undefined ? { response: result } : {}),
          ...(error !== undefined ? { reason: responseErrorMessage(error) } : {}),
        },
      };
      const existing = this.pendingHostCommands.get(commandId);
      if (existing?.kind === "server-response") {
        this.sendControl(client, { type: "response.pending", requestId, commandId });
        return { accepted: true, pending: true, requestId, commandId };
      }
      if (!pending.hostClient || pending.hostClient.socket.readyState !== WebSocket.OPEN) {
        this.clearPendingServerRequest(pending.appId);
        this.sendControl(client, { type: "response.rejected", requestId, code: "host_unavailable", message: "VS Code host is disconnected" });
        return { accepted: false, code: "host_unavailable" };
      }
      try {
        this.pendingHostCommands.set(commandId, {
          kind: "server-response",
          commandId,
          // Keep the original app-server id for exact map cleanup. The
          // browser-facing id may be a legacy stringified form of that id.
          requestId: pending.appId,
          responseRequestId: requestId,
          method: pending.method,
          client,
          commandScope: this.hostCommandScope || pending.hostClient?.commandScope || null,
          createdAt: Date.now(),
        });
        pending.hostClient.socket.send(JSON.stringify(hostFrame));
        this.auditAction(client, pending.method, { requestId, target: "vscode-host", result, error }, "forwarded");
        this.sendControl(client, { type: "response.pending", requestId, commandId });
        return { accepted: true, pending: true, requestId, commandId };
      } catch (sendError) {
        this.pendingHostCommands.delete(commandId);
        this.sendControl(client, { type: "response.rejected", requestId, code: "host_unavailable", message: sendError.message, retryable: true });
        return { accepted: false, code: "host_unavailable" };
      }
    }

    const appMessage = { id: pending.appId };
    if ("result" in message) appMessage.result = normalizedResult;
    else appMessage.error = message.error;
    try {
      this.sendToApp(appMessage);
      this.clearPendingServerRequest(pending.appId);
      this.auditAction(client, pending.method, { requestId, result: message.result, error: message.error }, "responded");
      const event = this.recordEvent("server.responded", {
        requestId,
        method: pending.method,
        ok: !message.error,
      });
      this.sendControl(client, { type: "response.accepted", requestId, seq: event.seq });
      return { accepted: true, requestId };
    } catch (error) {
      this.sendControl(client, { type: "response.rejected", requestId, ...normalizeError(error) });
      return { accepted: false, ...normalizeError(error) };
    }
  }

  cacheCommandResult(commandId, payload, sessionScope) {
    const key = String(commandId);
    const scope = this.mode === "host"
      ? (sessionScope || this.hostCommandScope || null)
      : "embedded";
    this.commandResults.set(key, { ...payload, cachedAt: Date.now() });
    this.commandResultScopes.set(key, scope);
    this.pruneCommandResults();
  }

  getCachedCommandResult(commandId) {
    const key = String(commandId);
    const cached = this.commandResults.get(key);
    if (!cached) return null;
    // A disconnect leaves the outcome unknown. Never replay that marker as a
    // completed result; remove it so a retry can be forwarded to a reconnected
    // host (or receive the normal offline error).
    if (cached.uncertain) {
      this.commandResults.delete(key);
      this.commandResultScopes.delete(key);
      return null;
    }
    const activeScope = this.mode === "host"
      ? (this.hostClient && this.state.hostConnected ? this.hostCommandScope : null)
      : "embedded";
    // Host results are never replayed while disconnected. This also prevents
    // an old result from leaking across a session-id change.
    if (!activeScope || this.commandResultScopes.get(key) !== activeScope) return null;
    return cached;
  }

  clearCommandResults() {
    this.commandResults.clear();
    this.commandResultScopes.clear();
  }

  pruneCommandResults() {
    const cutoff = Date.now() - 15 * 60 * 1000;
    for (const [key, value] of this.commandResults) {
      if (value.cachedAt < cutoff) {
        this.commandResults.delete(key);
        this.commandResultScopes.delete(key);
      }
    }
    while (this.commandResults.size > 1_000) {
      const [firstKey] = this.commandResults.keys();
      this.commandResults.delete(firstKey);
      this.commandResultScopes.delete(firstKey);
    }
  }

  sendControl(client, message) {
    if (client.capture) client.capture.push(message);
    if (!client.socket || client.socket.readyState !== WebSocket.OPEN) return;
    const serialized = JSON.stringify(message);
    const frameBytes = Buffer.byteLength(serialized, "utf8");
    if (frameBytes > MAX_WS_PAYLOAD) {
      client.socket.close(1009, "relay frame is too large");
      return;
    }
    const bufferedBytes = Number(client.socket.bufferedAmount) || 0;
    if (bufferedBytes + frameBytes > this.clientBufferedByteLimit) {
      client.socket.close(1013, "client is too slow");
      return;
    }
    client.socket.send(serialized);
  }

  snapshot() {
    // These projections used to be serialized both inside `state` and again
    // at the top level, nearly doubling every long-history control frame.
    // Keep the compact lifecycle state nested and one authoritative transcript
    // projection at the stable top-level protocol fields.
    const {
      outputTail,
      messages,
      subagents,
      sessionMetadata,
      executionStatus,
      ...state
    } = this.state;
    return {
      protocol: 1,
      latestSeq: this.nextSeq,
      state,
      clients: [...this.clients]
        .filter((client) => client.authenticated)
        .map((client) => ({ id: client.id, role: client.role })),
      pendingRequests: [...this.pendingServerRequests.values()].map((request) => ({
        requestId: request.appId,
        method: request.method,
        params: request.params,
        ...(request.commandHash ? { commandHash: request.commandHash } : {}),
        ...(request.risk ? { risk: request.risk } : {}),
        ...(request.summary ? { summary: request.summary } : {}),
        ...(request.expiresAt ? { expiresAt: request.expiresAt } : {}),
        createdAt: request.createdAt,
      })),
      outputTail: outputTail || "",
      messages: Array.isArray(messages) ? messages : [],
      subagents: Array.isArray(subagents) ? subagents : [],
      ...(sessionMetadata ? { metadata: sessionMetadata } : {}),
      status: executionStatus,
      executionStatus,
    };
  }

  tokenFromRequest(request) {
    const authorization = request.headers.authorization || "";
    if (/^Bearer\s+/i.test(authorization)) return authorization.replace(/^Bearer\s+/i, "");
    return request.headers["x-codex-token"] || "";
  }

  authenticateHttp(request) {
    const role = this.roleForToken(this.tokenFromRequest(request));
    if (role) return role;
    if (!this.authRequired && isLoopbackAddress(request.socket?.remoteAddress)) return "operator";
    return null;
  }

  async handleHttp(request, response) {
    const base = `http://${request.headers.host || "localhost"}`;
    let requestUrl;
    try {
      requestUrl = new URL(request.url, base);
    } catch {
      jsonResponse(response, 400, { error: "invalid_url" });
      return;
    }

    if (!this.authRequired && !isLoopbackRequestHost(request)) {
      jsonResponse(response, 403, { error: "loopback_host_required" });
      return;
    }

    if (request.method === "GET" && requestUrl.pathname === "/api/health") {
      jsonResponse(response, this.state.app === "offline" ? 503 : 200, {
        ok: this.state.app !== "offline",
        app: this.state.app,
        initialized: this.state.initialized,
        authRequired: this.authRequired,
        latestSeq: this.nextSeq,
      });
      return;
    }

    if (requestUrl.pathname.startsWith("/api/")) {
      const role = this.authenticateHttp(request);
      if (!role) {
        jsonResponse(response, 401, { error: "unauthorized" }, { "WWW-Authenticate": "Bearer" });
        return;
      }

      if (request.method === "POST" && !isAllowedHttpOrigin(request)) {
        jsonResponse(response, 403, { error: "origin_not_allowed" });
        return;
      }

      if (request.method === "GET" && requestUrl.pathname === "/api/state") {
        jsonResponse(response, 200, { role, ...this.snapshot(), audit: this.audit.slice(-50) });
        return;
      }
      if (request.method === "GET" && requestUrl.pathname === "/api/events") {
        const fromSeq = Number(requestUrl.searchParams.get("fromSeq") || 0);
        jsonResponse(response, 200, {
          latestSeq: this.nextSeq,
          events: this.events.filter((event) => event.seq > fromSeq),
        });
        return;
      }
      if (request.method === "POST" && requestUrl.pathname === "/api/command") {
        try {
          const body = await readJson(request);
          const client = { id: "http", role, authenticated: true, capture: [] };
          const result = this.dispatchCommand(client, { type: "command", ...body });
          jsonResponse(response, result.accepted ? 202 : 400, { ...result, messages: client.capture });
        } catch (error) {
          jsonResponse(response, error.statusCode || 400, { error: normalizeError(error) });
        }
        return;
      }
      if (request.method === "POST" && requestUrl.pathname === "/api/respond") {
        try {
          const body = await readJson(request);
          const client = { id: "http", role, authenticated: true, capture: [] };
          const result = this.dispatchServerResponse(client, { type: "respond", ...body });
          jsonResponse(response, result.accepted ? 202 : 400, { ...result, messages: client.capture });
        } catch (error) {
          jsonResponse(response, error.statusCode || 400, { error: normalizeError(error) });
        }
        return;
      }
      jsonResponse(response, 404, { error: "not_found" });
      return;
    }

    this.serveStatic(request, response, requestUrl.pathname);
  }

  serveStatic(request, response, pathname) {
    if (request.method !== "GET" && request.method !== "HEAD") {
      response.writeHead(405, { Allow: "GET, HEAD" });
      response.end();
      return;
    }
    let relativePath;
    try {
      relativePath = pathname === "/" ? "index.html" : decodeURIComponent(pathname).replace(/^\/+/, "");
    } catch {
      // A malformed percent escape must be an ordinary client error, not an
      // uncaught exception from the HTTP request handler.
      response.writeHead(400, { "Content-Type": "text/plain; charset=utf-8", "Cache-Control": "no-store" });
      response.end("Invalid URL");
      return;
    }
    const filePath = path.resolve(PUBLIC_ROOT, relativePath);
    if (!filePath.startsWith(`${PUBLIC_ROOT}${path.sep}`) && filePath !== path.join(PUBLIC_ROOT, "index.html")) {
      response.writeHead(403);
      response.end("Forbidden");
      return;
    }
    fs.stat(filePath, (error, stat) => {
      if (error || !stat.isFile()) {
        response.writeHead(404, { "Content-Type": "text/plain; charset=utf-8" });
        response.end("Not found");
        return;
      }
      response.writeHead(200, {
        "Content-Type": contentType(filePath),
        "Content-Length": stat.size,
        "Cache-Control": "no-store",
        "X-Content-Type-Options": "nosniff",
        "Referrer-Policy": "no-referrer",
        "Content-Security-Policy": "default-src 'self'; connect-src 'self' ws: wss:; img-src 'self' data:; style-src 'self'; script-src 'self'; base-uri 'none'; frame-ancestors 'self'",
      });
      if (request.method === "HEAD") response.end();
      else fs.createReadStream(filePath).pipe(response);
    });
  }
}

function isObjectPayload(value) {
  return Boolean(value && typeof value === "object" && !Array.isArray(value));
}

function firstObject(...values) {
  return values.find((value) => isObjectPayload(value)) || null;
}

function commandMethodFromFrame(frame) {
  const nested = isObjectPayload(frame.command) ? frame.command : null;
  if (typeof frame.method === "string" && frame.method.trim()) return frame.method.trim();
  if (typeof nested?.type === "string" && nested.type.trim()) return nested.type.trim();
  if (typeof frame.kind === "string" && (frame.kind === "command" || frame.kind === "response") && typeof frame.type === "string") {
    if (frame.type !== "command" && frame.type !== "respond" && frame.type !== "server-response") return frame.type.trim();
  }
  if (typeof frame.type === "string" && frame.type !== "command" && frame.type !== "respond" && frame.type !== "server-response") {
    return frame.type.trim();
  }
  return "";
}

function normalizeWireMethod(method) {
  const value = String(method || "").trim();
  const aliases = {
    "control.mode.get": "control/mode/get",
    "controlmode.get": "control/mode/get",
    "mode.get": "control/mode/get",
    "control.mode.set": "control/mode/set",
    "controlmode.set": "control/mode/set",
    "mode.set": "control/mode/set",
    "thread.start": "thread/start",
    "session.new": "session/new",
    "sessionnew": "session/new",
    "thread.new": "session/new",
    "threadnew": "session/new",
    "session/new": "session/new",
    "thread/new": "session/new",
    "thread.settings.update": "thread/settings/update",
    "threadsettings.update": "thread/settings/update",
    "session.list": "session/list",
    "thread.list": "session/list",
    "session.select": "session/select",
    "session.switch": "session/select",
    "thread.select": "session/select",
    "thread.attach": "session/select",
    "turn.start": "turn/start",
    "turn.steer": "turn/steer",
    "turn.interrupt": "turn/interrupt",
  };
  return aliases[value.toLowerCase()] || value;
}

function commandIdFromFrame(frame) {
  const nested = isObjectPayload(frame.command) ? frame.command : null;
  const value = frame.commandId ?? nested?.commandId ?? (typeof frame.id === "string" ? frame.id : undefined);
  return value === undefined || value === null ? "" : String(value);
}

function normalizeBrowserCommand(frame) {
  const method = normalizeWireMethod(commandMethodFromFrame(frame));
  const type = frame.type;
  const hasCommandEnvelope = frame.kind === "command"
    || type === "command"
    || (frame.kind === undefined && (ALLOWED_METHODS.has(method) || REMOTE_RESPONSE_METHODS.has(method)));
  if (!hasCommandEnvelope || !method) return null;

  const nested = isObjectPayload(frame.command) ? frame.command : null;
  const params = firstObject(frame.params, frame.payload, nested?.params, nested?.payload) || {};
  return {
    type: "command",
    commandId: commandIdFromFrame(frame),
    method,
    params,
  };
}

function approvalWireDecision(decision) {
  // Keep the caller's decision intact until dispatch knows the target
  // app-server method. Legacy approval methods use `approved`/`abort`, while
  // v2 methods use `accept`/`cancel`; method-aware normalization handles the
  // conversion without discarding amendment tags.
  return decision;
}

function normalizeBrowserResponse(frame, hintedMethod) {
  const type = typeof frame.type === "string" ? frame.type : "";
  const method = hintedMethod || commandMethodFromFrame(frame);
  const isLegacy = type === "respond" || type === "server-response";
  const isResponse = isLegacy
    || frame.kind === "response"
    || REMOTE_RESPONSE_METHODS.has(method);
  if (!isResponse) return null;

  const nested = isObjectPayload(frame.command) ? frame.command : null;
  const payload = firstObject(frame.payload, frame.params, nested?.payload, nested?.params) || (isLegacy ? {} : frame);
  const requestId = frame.requestId ?? payload.requestId ?? (typeof frame.id === "number" || typeof frame.id === "string" ? frame.id : "");
    const response = { type: "respond", requestId };
  if (payload.decision !== undefined) response.requestedDecision = payload.decision;

  if (Object.prototype.hasOwnProperty.call(frame, "result")) {
    response.result = frame.result;
  } else if (Object.prototype.hasOwnProperty.call(frame, "error")) {
    response.error = frame.error;
  } else if (Object.prototype.hasOwnProperty.call(payload, "result")) {
    response.result = payload.result;
  } else if (Object.prototype.hasOwnProperty.call(payload, "error")) {
    response.error = payload.error;
  } else if (payload.response !== undefined) {
    response.result = payload.response;
  } else if (method === "input.respond" || method === "server.request.respond") {
    if (payload.answers !== undefined) response.result = { answers: payload.answers };
    else {
      const custom = {};
      for (const [key, value] of Object.entries(payload)) {
        if (!["v", "kind", "type", "method", "commandId", "id", "sessionId", "actor", "requestId", "reason", "params", "payload", "command"].includes(key)) custom[key] = value;
      }
      if (Object.keys(custom).length) response.result = custom;
    }
  } else if (payload.decision !== undefined) {
    response.result = { decision: approvalWireDecision(payload.decision) };
  }
  return response;
}

async function main() {
  const relay = new CodexRelay();
  const address = await relay.start();
  const displayHost = address.host === "::" || address.host === "0.0.0.0" ? "127.0.0.1" : address.host;
  process.stdout.write(`Codex Remote Collab: http://${displayHost}:${address.port}\n`);
  if (relay.authRequired) {
    process.stdout.write(`Host token:     ${relay.hostToken}\n`);
    process.stdout.write(`Operator token: ${relay.operatorToken}\n`);
    process.stdout.write(`Viewer token:   ${relay.viewerToken}\n`);
    process.stdout.write("Keep these tokens private. Use TLS before exposing this relay outside a trusted network.\n");
  } else {
    process.stdout.write("Authentication: disabled for loopback connections (set CODEX_REMOTE_AUTH=required to enable tokens).\n");
  }

  const shutdown = async () => {
    await relay.stop();
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
  ALLOWED_METHODS,
  CodexRelay,
  SERVER_REQUEST_METHODS,
  redact,
};
