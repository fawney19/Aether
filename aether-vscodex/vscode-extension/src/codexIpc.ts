/**
 * Minimal client for the private Codex desktop/VS Code coordination socket.
 *
 * This is intentionally separate from the app-server (JSONL/stdio) adapter.
 * It attaches to the already running Codex UI through the local IPC router and
 * therefore does not spawn another `codex` process. The wire protocol is
 * private and versioned by the official extension; keep this module isolated
 * so a protocol change can fail without taking down the relay bridge.
 */

import * as crypto from "node:crypto";
import * as net from "node:net";
import * as os from "node:os";
import * as path from "node:path";

import type { JsonObject, JsonValue } from "./protocol";

export const INITIALIZING_CLIENT_ID = "initializing-client";
export const DEFAULT_IPC_REQUEST_TIMEOUT_MS = 5_000;
export const DEFAULT_MAX_IPC_FRAME_BYTES = 256 * 1024 * 1024;

/** Versions shipped by openai.chatgpt 26.820.71523. */
export const CODEX_IPC_METHOD_VERSIONS = Object.freeze({
  "thread-stream-state-changed": 11,
  "thread-stream-following-changed": 1,
  "thread-stream-following-status-requested": 1,
  "ipc-connection-reset": 1,
  "thread-read-state-changed": 2,
  "thread-archived": 2,
  "thread-unarchived": 1,
  "thread-owner-discovery": 1,
  "thread-follower-start-turn": 2,
  "thread-follower-load-complete-history": 1,
  "thread-follower-compact-thread": 1,
  "thread-follower-steer-turn": 1,
  "thread-follower-interrupt-turn": 4,
  "thread-follower-update-thread-settings": 1,
  "thread-follower-edit-last-user-turn": 2,
  "thread-follower-command-approval-decision": 1,
  "thread-follower-file-approval-decision": 1,
  "thread-follower-permissions-request-approval-response": 1,
  "thread-follower-submit-user-input": 1,
  "thread-follower-submit-mcp-server-elicitation-response": 1,
  "thread-follower-set-queued-follow-ups-state": 1,
  "thread-queued-followups-changed": 1,
} as const);

export type IpcMethod = keyof typeof CODEX_IPC_METHOD_VERSIONS;
export type IpcRequestId = string | number;
export type IpcPatchPathPart = string | number;

export interface IpcRequest {
  type: "request";
  requestId: IpcRequestId;
  sourceClientId: string;
  targetClientId?: string;
  version: number;
  method: string;
  params?: JsonValue;
  timeoutMs?: number;
}

export interface IpcResponse {
  type: "response";
  requestId: IpcRequestId;
  resultType: "success" | "error";
  method?: string;
  handledByClientId?: string;
  result?: JsonValue;
  error?: string;
}

export interface IpcBroadcast {
  type: "broadcast";
  method: string;
  sourceClientId?: string;
  targetClientIds?: string[];
  version: number;
  params?: JsonValue;
}

export interface IpcClientDiscoveryRequest {
  type: "client-discovery-request";
  requestId: IpcRequestId;
  request: IpcRequest;
}

export interface IpcClientDiscoveryResponse {
  type: "client-discovery-response";
  requestId: IpcRequestId;
  response: { canHandle: boolean };
}

export type IpcMessage =
  | IpcRequest
  | IpcResponse
  | IpcBroadcast
  | IpcClientDiscoveryRequest
  | IpcClientDiscoveryResponse;

export interface IpcJsonPatch {
  op: "add" | "remove" | "replace";
  path: IpcPatchPathPart[];
  value?: JsonValue;
}

export interface ThreadStreamSnapshot {
  type: "snapshot";
  revision: number;
  conversationState: JsonObject;
}

export interface ThreadStreamPatches {
  type: "patches";
  baseRevision: number;
  revision: number;
  patches: IpcJsonPatch[];
}

export type ThreadStreamChange = ThreadStreamSnapshot | ThreadStreamPatches;

export interface ConversationStreamState {
  conversationId: string;
  hostId: string;
  ownerClientId: string;
  revision: number;
  conversationState: JsonObject;
}

export type ConversationStreamEvent =
  | (ConversationStreamState & { kind: "snapshot"; raw: IpcBroadcast })
  | (ConversationStreamState & { kind: "patches"; patches: IpcJsonPatch[]; baseRevision: number; raw: IpcBroadcast })
  | {
      kind: "desync";
      conversationId: string;
      hostId: string;
      ownerClientId: string;
      expectedRevision: number;
      receivedBaseRevision: number;
      receivedRevision: number;
      raw: IpcBroadcast;
    };

export interface CodexIpcClientOptions {
  /** Explicit socket path; otherwise `$CODEX_HOME/ipc/ipc.sock` or `~/.codex`. */
  socketPath?: string;
  codexHome?: string;
  homeDir?: string;
  env?: NodeJS.ProcessEnv;
  platform?: NodeJS.Platform;
  clientType?: string;
  requestTimeoutMs?: number;
  maxFrameBytes?: number;
  strictVersions?: boolean;
  /** Reconnect after a socket close and re-send all active following subscriptions. */
  autoReconnect?: boolean;
  reconnectDelayMs?: number;
  /** Optional handler for discovery requests. Default is fail-closed (`false`). */
  canHandleRequest?: (request: IpcRequest) => boolean | Promise<boolean>;
}

export interface FollowerTurnStartOptions {
  request?: JsonObject;
  context?: JsonObject;
  clientUserMessageId?: string;
  ownerClientId?: string;
  timeoutMs?: number;
}

export interface FollowerSteerOptions {
  clientUserMessageId?: string;
  serviceTier?: string | null;
  attachments?: JsonValue[];
  additionalContext?: JsonObject | null;
  restoreMessage?: JsonValue | null;
  ownerClientId?: string;
  timeoutMs?: number;
}

export interface FollowerInterruptOptions {
  mode?: "user-stop" | "system" | "descendant-cleanup" | string;
  expectedTurnId?: string | null;
  ownerClientId?: string;
  timeoutMs?: number;
}

export interface FollowOptions {
  hostId?: string;
  targetClientIds?: string[];
}

export interface RequestOptions {
  targetClientId?: string;
  timeoutMs?: number;
  version?: number;
  requestId?: IpcRequestId;
}

export interface IpcErrorOptions {
  code: string;
  response?: IpcResponse;
}

export class CodexIpcError extends Error {
  readonly code: string;
  readonly response?: IpcResponse;

  constructor(message: string, options: IpcErrorOptions) {
    super(message);
    this.name = "CodexIpcError";
    this.code = options.code;
    this.response = options.response;
  }
}

export function resolveCodexIpcSocketPath(options: {
  socketPath?: string;
  codexHome?: string;
  homeDir?: string;
  env?: NodeJS.ProcessEnv;
  platform?: NodeJS.Platform;
} = {}): string {
  if (options.socketPath?.trim()) return options.socketPath.trim();
  const platform = options.platform ?? process.platform;
  if (platform === "win32") return "\\\\.\\pipe\\codex-ipc";
  const env = options.env ?? process.env;
  const homeDir = options.homeDir ?? os.homedir();
  const configuredHome = options.codexHome?.trim() || env.CODEX_HOME?.trim() || path.join(homeDir, ".codex");
  const codexHome = configuredHome === "~"
    ? homeDir
    : configuredHome.startsWith("~/")
      ? path.join(homeDir, configuredHome.slice(2))
      : configuredHome;
  return path.join(codexHome, "ipc", "ipc.sock");
}

/** Encode one private IPC frame: uint32 little-endian byte length + UTF-8 JSON. */
export function encodeIpcFrame(message: IpcMessage, maxFrameBytes = DEFAULT_MAX_IPC_FRAME_BYTES): Buffer {
  const json = JSON.stringify(message);
  const payload = Buffer.from(json, "utf8");
  if (payload.length === 0 || payload.length > maxFrameBytes) {
    throw new RangeError(`IPC frame exceeds ${maxFrameBytes} bytes`);
  }
  const frame = Buffer.allocUnsafe(4 + payload.length);
  frame.writeUInt32LE(payload.length, 0);
  payload.copy(frame, 4);
  return frame;
}

/** Incremental decoder that accepts arbitrary TCP/Unix-socket chunk boundaries. */
export class IpcFrameDecoder {
  private buffer = Buffer.alloc(0);

  constructor(private readonly maxFrameBytes = DEFAULT_MAX_IPC_FRAME_BYTES) {}

  push(chunk: Uint8Array): IpcMessage[] {
    if (chunk.length === 0) return [];
    this.buffer = this.buffer.length === 0 ? Buffer.from(chunk) : Buffer.concat([this.buffer, chunk]);
    const messages: IpcMessage[] = [];
    while (this.buffer.length >= 4) {
      const payloadLength = this.buffer.readUInt32LE(0);
      if (payloadLength === 0 || payloadLength > this.maxFrameBytes) {
        throw new CodexIpcError(`Invalid IPC frame length (${payloadLength} bytes)`, { code: "invalid-frame-length" });
      }
      if (this.buffer.length < payloadLength + 4) break;
      const payload = this.buffer.subarray(4, payloadLength + 4).toString("utf8");
      this.buffer = this.buffer.subarray(payloadLength + 4);
      let decoded: unknown;
      try {
        decoded = JSON.parse(payload);
      } catch (error) {
        throw new CodexIpcError(`Invalid IPC JSON: ${error instanceof Error ? error.message : String(error)}`, {
          code: "invalid-json",
        });
      }
      if (!isRecord(decoded) || typeof decoded.type !== "string") {
        throw new CodexIpcError("IPC frame must be an object with a type", { code: "invalid-message" });
      }
      messages.push(decoded as unknown as IpcMessage);
    }
    return messages;
  }

  reset(): void {
    this.buffer = Buffer.alloc(0);
  }
}

type Listener<T> = (value: T) => void;
export interface IpcSubscription { dispose(): void; }

function subscribe<T>(set: Set<Listener<T>>, listener: Listener<T>): IpcSubscription {
  set.add(listener);
  return { dispose: () => set.delete(listener) };
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isJsonObject(value: unknown): value is JsonObject {
  return isRecord(value);
}

function requestIdKey(id: IpcRequestId): string {
  return `${typeof id}:${String(id)}`;
}

function cloneJson<T extends JsonValue>(value: T): T {
  return JSON.parse(JSON.stringify(value)) as T;
}

function versionFor(method: string, params?: JsonValue): number {
  // The official client accepts interrupt v3 when expectedTurnId is absent;
  // v4 is used when the active-turn precondition is present.
  if (method === "thread-follower-interrupt-turn"
    && (!isRecord(params) || params.expectedTurnId === undefined || params.expectedTurnId === null)) return 3;
  return CODEX_IPC_METHOD_VERSIONS[method as IpcMethod] ?? 0;
}

function textInput(text: string): JsonObject {
  return { type: "text", text, text_elements: [] };
}

function normalizeInput(input: string | JsonValue[]): JsonValue[] {
  return typeof input === "string"
    ? [textInput(input)]
    : input.map((entry) => typeof entry === "string" ? textInput(entry) : entry);
}

function hasTarget(frame: IpcBroadcast, clientId: string): boolean {
  return frame.targetClientIds == null || frame.targetClientIds.includes(clientId);
}

/** Apply the JSON patch arrays generated by Immer in the official webview. */
export function applyIpcPatches(root: JsonValue, patches: IpcJsonPatch[]): JsonValue {
  let result = cloneJson(root);
  for (const patch of patches) {
    if (!Array.isArray(patch.path)) throw new CodexIpcError("IPC patch path must be an array", { code: "invalid-patch" });
    if (patch.path.length === 0) {
      if (patch.op === "remove") throw new CodexIpcError("Removing the conversation root is unsupported", { code: "invalid-patch" });
      if (patch.value === undefined) throw new CodexIpcError("Patch value is missing", { code: "invalid-patch" });
      result = cloneJson(patch.value);
      continue;
    }

    const parentPath = patch.path.slice(0, -1);
    const key = patch.path[patch.path.length - 1];
    assertSafePatchPart(key);
    const parent = getAtPath(result, parentPath);
    if (Array.isArray(parent)) {
      const index = key === "-" ? parent.length : toArrayIndex(key);
      if (patch.op === "add") {
        if (patch.value === undefined) throw new CodexIpcError("Patch value is missing", { code: "invalid-patch" });
        parent.splice(index, 0, cloneJson(patch.value));
      } else if (patch.op === "replace") {
        if (patch.value === undefined || index < 0 || index >= parent.length) throw new CodexIpcError("Invalid array replace patch", { code: "invalid-patch" });
        parent[index] = cloneJson(patch.value);
      } else {
        if (index < 0 || index >= parent.length) throw new CodexIpcError("Invalid array remove patch", { code: "invalid-patch" });
        parent.splice(index, 1);
      }
      continue;
    }
    if (!isRecord(parent) || typeof key !== "string") {
      throw new CodexIpcError("IPC patch parent is not an object or array", { code: "invalid-patch" });
    }
    if (patch.op === "remove") {
      delete parent[key];
    } else {
      if (patch.value === undefined) throw new CodexIpcError("Patch value is missing", { code: "invalid-patch" });
      parent[key] = cloneJson(patch.value);
    }
  }
  return result;
}

function getAtPath(root: JsonValue, pathParts: IpcPatchPathPart[]): JsonValue {
  let current: JsonValue = root;
  for (const part of pathParts) {
    if (Array.isArray(current)) {
      const index = toArrayIndex(part);
      if (index < 0 || index >= current.length) throw new CodexIpcError("IPC patch path is out of bounds", { code: "invalid-patch" });
      current = current[index];
    } else if (isRecord(current) && typeof part === "string" && Object.prototype.hasOwnProperty.call(current, part)) {
      assertSafePatchPart(part);
      current = current[part];
    } else {
      throw new CodexIpcError("IPC patch path does not exist", { code: "invalid-patch" });
    }
  }
  return current;
}

function toArrayIndex(value: IpcPatchPathPart): number {
  if (typeof value === "number" && Number.isInteger(value)) return value;
  if (typeof value === "string" && /^\d+$/.test(value)) return Number(value);
  throw new CodexIpcError(`Invalid array patch index: ${String(value)}`, { code: "invalid-patch" });
}

function assertSafePatchPart(value: IpcPatchPathPart): void {
  if (value === "__proto__" || value === "prototype" || value === "constructor") {
    throw new CodexIpcError("Unsafe IPC patch path", { code: "invalid-patch" });
  }
}

export class CodexIpcClient {
  readonly socketPath: string;
  private readonly options: Required<Pick<CodexIpcClientOptions, "clientType" | "requestTimeoutMs" | "maxFrameBytes" | "strictVersions" | "autoReconnect" | "reconnectDelayMs">> & CodexIpcClientOptions;
  private socket: net.Socket | undefined;
  private decoder: IpcFrameDecoder;
  private connectPromise: Promise<string> | undefined;
  private reconnectTimer: NodeJS.Timeout | undefined;
  private disposed = false;
  private clientId = INITIALIZING_CLIENT_ID;
  private readonly pending = new Map<string, { method: string; resolve: (response: IpcResponse) => void; reject: (error: Error) => void; timer: NodeJS.Timeout }>();
  private readonly followed = new Map<string, string>();
  private readonly streams = new Map<string, ConversationStreamState>();
  private readonly messageListeners = new Set<Listener<IpcMessage>>();
  private readonly broadcastListeners = new Set<Listener<IpcBroadcast>>();
  private readonly streamListeners = new Set<Listener<ConversationStreamEvent>>();
  private readonly errorListeners = new Set<Listener<Error>>();
  private readonly closeListeners = new Set<Listener<Error | undefined>>();
  private readonly discoveryHandler?: (request: IpcRequest) => boolean | Promise<boolean>;

  constructor(options: CodexIpcClientOptions = {}) {
    this.options = {
      ...options,
      clientType: options.clientType ?? "codex-remote-collab",
      requestTimeoutMs: options.requestTimeoutMs ?? DEFAULT_IPC_REQUEST_TIMEOUT_MS,
      maxFrameBytes: options.maxFrameBytes ?? DEFAULT_MAX_IPC_FRAME_BYTES,
      strictVersions: options.strictVersions ?? true,
      autoReconnect: options.autoReconnect ?? false,
      reconnectDelayMs: options.reconnectDelayMs ?? 1_000,
    };
    this.socketPath = resolveCodexIpcSocketPath(options);
    this.decoder = new IpcFrameDecoder(this.options.maxFrameBytes);
    this.discoveryHandler = options.canHandleRequest;
  }

  getClientId(): string { return this.clientId; }

  getConversationState(conversationId: string): ConversationStreamState | undefined {
    const state = this.streams.get(conversationId);
    return state == null ? undefined : { ...state, conversationState: cloneJson(state.conversationState) };
  }

  getFollowedConversations(): ReadonlyMap<string, string> { return this.followed; }

  onMessage(listener: Listener<IpcMessage>): IpcSubscription { return subscribe(this.messageListeners, listener); }
  onBroadcast(listener: Listener<IpcBroadcast>): IpcSubscription { return subscribe(this.broadcastListeners, listener); }
  onStreamEvent(listener: Listener<ConversationStreamEvent>): IpcSubscription { return subscribe(this.streamListeners, listener); }
  onError(listener: Listener<Error>): IpcSubscription { return subscribe(this.errorListeners, listener); }
  onClose(listener: Listener<Error | undefined>): IpcSubscription { return subscribe(this.closeListeners, listener); }

  async connect(): Promise<string> {
    if (this.disposed) throw new CodexIpcError("IPC client is disposed", { code: "disposed" });
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = undefined;
    }
    if (this.socket?.writable && this.clientId !== INITIALIZING_CLIENT_ID) return this.clientId;
    if (this.connectPromise) return this.connectPromise;
    this.connectPromise = new Promise<string>((resolve, reject) => {
      const socket = net.createConnection(this.socketPath);
      this.socket = socket;
      this.decoder.reset();
      let settled = false;
      const finishError = (error: Error): void => {
        if (!settled) {
          settled = true;
          reject(error);
        }
        this.emitError(error);
      };
      socket.setNoDelay?.(true);
      socket.on("connect", () => {
        const requestId = crypto.randomUUID();
        const timer = setTimeout(() => {
          this.pending.delete(requestIdKey(requestId));
          finishError(new CodexIpcError("IPC initialize timed out", { code: "timeout" }));
          socket.destroy();
        }, this.options.requestTimeoutMs);
        this.pending.set(requestIdKey(requestId), {
          method: "initialize",
          resolve: (response) => {
            clearTimeout(timer);
            if (response.resultType !== "success" || !isRecord(response.result) || typeof response.result.clientId !== "string") {
              finishError(new CodexIpcError("IPC initialize returned an invalid response", { code: "initialize-failed", response }));
              socket.destroy();
              return;
            }
            this.clientId = response.result.clientId;
            settled = true;
            resolve(this.clientId);
            this.resubscribeAfterConnect().catch((error) => this.emitError(asError(error)));
          },
          reject: (error) => {
            clearTimeout(timer);
            finishError(error);
            socket.destroy();
          },
          timer,
        });
        this.write({
          type: "request",
          requestId,
          sourceClientId: INITIALIZING_CLIENT_ID,
          version: 0,
          method: "initialize",
          params: { clientType: this.options.clientType },
        });
      });
      socket.on("data", (chunk) => {
        try {
          for (const message of this.decoder.push(chunk)) this.handleMessage(message);
        } catch (error) {
          const normalized = asError(error);
          finishError(normalized);
          socket.destroy(normalized);
        }
      });
      socket.on("error", (error) => {
        if (!settled) finishError(error);
        else this.emitError(error);
      });
      socket.on("close", () => {
        this.handleClose();
      });
    }).finally(() => {
      this.connectPromise = undefined;
    });
    return this.connectPromise;
  }

  async followConversation(conversationId: string, following = true, options: FollowOptions = {}): Promise<void> {
    const hostId = options.hostId ?? "local";
    await this.connect();
    if (following) this.followed.set(conversationId, hostId);
    else {
      this.followed.delete(conversationId);
      this.streams.delete(conversationId);
    }
    const params: JsonObject = { conversationId, hostId, following };
    const frame: IpcBroadcast = {
      type: "broadcast",
      method: "thread-stream-following-changed",
      sourceClientId: this.clientId,
      version: CODEX_IPC_METHOD_VERSIONS["thread-stream-following-changed"],
      params,
    };
    if (options.targetClientIds) frame.targetClientIds = options.targetClientIds;
    this.write(frame);
  }

  async findThreadOwner(conversationId: string, hostId = "local", timeoutMs = this.options.requestTimeoutMs): Promise<string | null> {
    try {
      const response = await this.request("thread-owner-discovery", { conversationId, hostId }, { timeoutMs });
      return response.handledByClientId ?? null;
    } catch (error) {
      if (error instanceof CodexIpcError
        && (error.code === "no-client-found" || error.code.startsWith("no-client-found:"))) return null;
      throw error;
    }
  }

  async request(method: string, params?: JsonValue, options: RequestOptions = {}): Promise<IpcResponse> {
    await this.connect();
    const requestId = options.requestId ?? crypto.randomUUID();
    const timeoutMs = options.timeoutMs ?? this.options.requestTimeoutMs;
    const frame: IpcRequest = {
      type: "request",
      requestId,
      sourceClientId: this.clientId,
      version: options.version ?? versionFor(method, params),
      method,
      params,
    };
    if (options.targetClientId) frame.targetClientId = options.targetClientId;
    if (timeoutMs > 0) frame.timeoutMs = timeoutMs;
    return new Promise<IpcResponse>((resolve, reject) => {
      const key = requestIdKey(requestId);
      const timer = setTimeout(() => {
        this.pending.delete(key);
        reject(new CodexIpcError(`${method} timed out`, { code: "timeout" }));
      }, timeoutMs > 0 ? timeoutMs : 2 ** 31 - 1);
      this.pending.set(key, { method, resolve, reject, timer });
      try {
        this.write(frame);
      } catch (error) {
        clearTimeout(timer);
        this.pending.delete(key);
        reject(asError(error));
      }
    }).then((response) => {
      if (response.resultType === "error") {
        throw new CodexIpcError(response.error ?? `${method} failed`, { code: response.error ?? "ipc-error", response });
      }
      if (response.method != null && response.method !== method) {
        throw new CodexIpcError(`IPC response method mismatch: expected ${method}, got ${response.method}`, {
          code: "response-method-mismatch",
          response,
        });
      }
      return response;
    });
  }

  async requestFollower(method: string, conversationId: string, params: JsonObject = {}, options: RequestOptions & { ownerClientId?: string } = {}): Promise<IpcResponse> {
    const ownerClientId = options.ownerClientId ?? this.streams.get(conversationId)?.ownerClientId;
    if (!ownerClientId) throw new CodexIpcError(`No owner is known for conversation ${conversationId}`, { code: "owner-unknown" });
    // Do not allow a caller-provided params object to accidentally retarget a
    // request after the owner has been selected from the stream snapshot.
    const body: JsonObject = { ...params, conversationId };
    const { ownerClientId: _owner, ...requestOptions } = options;
    return this.request(method, body, { ...requestOptions, targetClientId: ownerClientId });
  }

  /** Send the exact private `turnStart` envelope expected by the owner. */
  async startTurn(conversationId: string, input: string | JsonValue[], options: FollowerTurnStartOptions = {}): Promise<JsonValue | undefined> {
    const request: JsonObject = {
      ...(options.request ?? {}),
      threadId: conversationId,
      input: options.request?.input ?? normalizeInput(input),
    };
    const context: JsonObject = { inheritThreadSettings: true, ...(options.context ?? {}) };
    if (options.clientUserMessageId) request.clientUserMessageId = options.clientUserMessageId;
    const response = await this.requestFollower("thread-follower-start-turn", conversationId, {
      turnStart: { request, context },
    }, {
      ownerClientId: options.ownerClientId,
      timeoutMs: options.timeoutMs,
    });
    return response.result;
  }

  async steerTurn(conversationId: string, input: string | JsonValue[], options: FollowerSteerOptions = {}): Promise<JsonValue | undefined> {
    const params: JsonObject = {
      clientUserMessageId: options.clientUserMessageId ?? crypto.randomUUID(),
      input: normalizeInput(input),
      attachments: options.attachments ?? [],
    };
    if (options.serviceTier !== undefined) params.serviceTier = options.serviceTier;
    if (options.additionalContext !== undefined) params.additionalContext = options.additionalContext;
    if (options.restoreMessage !== undefined) params.restoreMessage = options.restoreMessage;
    const response = await this.requestFollower("thread-follower-steer-turn", conversationId, params, {
      ownerClientId: options.ownerClientId,
      timeoutMs: options.timeoutMs,
    });
    return response.result;
  }

  /**
   * Persist settings for the next turn through the official conversation
   * owner.  The owner-side follower handler expects the settings nested under
   * `threadSettings`; `requestFollower` adds the conversation id to the
   * outer envelope, yielding:
   * `{ conversationId, threadSettings }`.
   */
  async updateThreadSettings(
    conversationId: string,
    threadSettings: JsonObject,
    options: RequestOptions & { ownerClientId?: string } = {},
  ): Promise<JsonValue | undefined> {
    if (!isJsonObject(threadSettings)) {
      throw new CodexIpcError("thread settings must be a JSON object", { code: "invalid-thread-settings" });
    }
    const response = await this.requestFollower(
      "thread-follower-update-thread-settings",
      conversationId,
      { threadSettings: cloneJson(threadSettings) },
      options,
    );
    return response.result;
  }

  /** Alias matching the official app-server manager method name. */
  async updateThreadSettingsForNextTurn(
    conversationId: string,
    threadSettings: JsonObject,
    options: RequestOptions & { ownerClientId?: string } = {},
  ): Promise<JsonValue | undefined> {
    return this.updateThreadSettings(conversationId, threadSettings, options);
  }

  async interruptTurn(conversationId: string, options: FollowerInterruptOptions = {}): Promise<JsonValue | undefined> {
    const params: JsonObject = { mode: options.mode ?? "user-stop" };
    if (options.expectedTurnId !== undefined && options.expectedTurnId !== null) params.expectedTurnId = options.expectedTurnId;
    const response = await this.requestFollower("thread-follower-interrupt-turn", conversationId, params, {
      ownerClientId: options.ownerClientId,
      timeoutMs: options.timeoutMs,
    });
    return response.result;
  }

  async loadCompleteHistory(conversationId: string, options: RequestOptions & { ownerClientId?: string } = {}): Promise<JsonValue | undefined> {
    const response = await this.requestFollower("thread-follower-load-complete-history", conversationId, {}, options);
    return response.result;
  }

  async respondCommandApproval(conversationId: string, requestId: IpcRequestId, decision: JsonValue, options: RequestOptions & { ownerClientId?: string } = {}): Promise<JsonValue | undefined> {
    return this.respondFollower("thread-follower-command-approval-decision", conversationId, { requestId, decision }, options);
  }

  async respondFileApproval(conversationId: string, requestId: IpcRequestId, decision: JsonValue, options: RequestOptions & { ownerClientId?: string } = {}): Promise<JsonValue | undefined> {
    return this.respondFollower("thread-follower-file-approval-decision", conversationId, { requestId, decision }, options);
  }

  async respondPermissionsApproval(conversationId: string, requestId: IpcRequestId, response: JsonValue, options: RequestOptions & { ownerClientId?: string } = {}): Promise<JsonValue | undefined> {
    return this.respondFollower("thread-follower-permissions-request-approval-response", conversationId, { requestId, response }, options);
  }

  async respondUserInput(conversationId: string, requestId: IpcRequestId, response: JsonValue, options: RequestOptions & { ownerClientId?: string } = {}): Promise<JsonValue | undefined> {
    return this.respondFollower("thread-follower-submit-user-input", conversationId, { requestId, response }, options);
  }

  async respondMcpElicitation(conversationId: string, requestId: IpcRequestId, response: JsonValue, options: RequestOptions & { ownerClientId?: string } = {}): Promise<JsonValue | undefined> {
    return this.respondFollower("thread-follower-submit-mcp-server-elicitation-response", conversationId, { requestId, response }, options);
  }

  private async respondFollower(method: string, conversationId: string, params: JsonObject, options: RequestOptions & { ownerClientId?: string }): Promise<JsonValue | undefined> {
    const response = await this.requestFollower(method, conversationId, params, options);
    return response.result;
  }

  async dispose(): Promise<void> {
    this.disposed = true;
    if (this.reconnectTimer) clearTimeout(this.reconnectTimer);
    this.reconnectTimer = undefined;
    for (const pending of this.pending.values()) {
      clearTimeout(pending.timer);
      pending.reject(new CodexIpcError("IPC client disposed", { code: "disposed" }));
    }
    this.pending.clear();
    this.socket?.destroy();
    this.socket = undefined;
    this.clientId = INITIALIZING_CLIENT_ID;
  }

  private write(message: IpcMessage): void {
    if (!this.socket?.writable) throw new CodexIpcError("IPC socket is not connected", { code: "not-connected" });
    this.socket.write(encodeIpcFrame(message, this.options.maxFrameBytes));
  }

  private handleMessage(message: IpcMessage): void {
    for (const listener of this.messageListeners) safeCall(listener, message, (error) => this.emitError(error));
    switch (message.type) {
      case "response":
        this.handleResponse(message);
        return;
      case "broadcast":
        this.handleBroadcast(message);
        return;
      case "client-discovery-request":
        this.handleDiscoveryRequest(message).catch((error) => this.emitError(asError(error)));
        return;
      case "request":
        this.handleUnexpectedRequest(message);
        return;
      case "client-discovery-response":
        // Discovery responses are consumed by the router, not by clients.
        return;
    }
  }

  private handleResponse(response: IpcResponse): void {
    const key = requestIdKey(response.requestId);
    const pending = this.pending.get(key);
    if (!pending) return;
    this.pending.delete(key);
    clearTimeout(pending.timer);
    pending.resolve(response);
  }

  private handleBroadcast(frame: IpcBroadcast): void {
    if (!hasTarget(frame, this.clientId)) return;
    for (const listener of this.broadcastListeners) safeCall(listener, frame, (error) => this.emitError(error));
    if (frame.method === "thread-stream-state-changed") {
      this.handleStreamStateBroadcast(frame);
    } else if (frame.method === "thread-stream-following-status-requested") {
      this.handleFollowingStatusRequested(frame);
    }
  }

  /** Re-announce active subscriptions when an owner reconnects or hands off. */
  private handleFollowingStatusRequested(frame: IpcBroadcast): void {
    if (!isRecord(frame.params)) return;
    if (this.options.strictVersions
      && frame.version !== CODEX_IPC_METHOD_VERSIONS["thread-stream-following-status-requested"]) {
      this.emitError(new CodexIpcError(`Unsupported thread following status version ${frame.version}`, { code: "version-mismatch" }));
      return;
    }
    const conversationId = typeof frame.params.conversationId === "string"
      ? frame.params.conversationId
      : undefined;
    const hostId = typeof frame.params.hostId === "string" ? frame.params.hostId : "local";
    const requester = frame.sourceClientId;
    if (!conversationId || !requester || requester === this.clientId) return;
    if (this.followed.get(conversationId) !== hostId) return;
    void this.followConversation(conversationId, true, {
      hostId,
      targetClientIds: [requester],
    }).catch((error) => this.emitError(asError(error)));
  }

  private handleStreamStateBroadcast(frame: IpcBroadcast): void {
    if (!isRecord(frame.params)) return;
    const conversationId = typeof frame.params.conversationId === "string" ? frame.params.conversationId : undefined;
    const hostId = typeof frame.params.hostId === "string" ? frame.params.hostId : "local";
    const change = frame.params.change;
    if (!conversationId || !isRecord(change) || typeof change.type !== "string") return;
    if (this.options.strictVersions && frame.version !== CODEX_IPC_METHOD_VERSIONS["thread-stream-state-changed"]) {
      this.emitError(new CodexIpcError(`Unsupported thread stream version ${frame.version}`, { code: "version-mismatch" }));
      return;
    }
    const ownerClientId = frame.sourceClientId ?? "";
    if (change.type === "snapshot") {
      if (typeof change.revision !== "number" || !isJsonObject(change.conversationState)) return;
      const state: ConversationStreamState = {
        conversationId,
        hostId,
        ownerClientId,
        revision: change.revision,
        conversationState: cloneJson(change.conversationState),
      };
      this.streams.set(conversationId, state);
      this.emitStream({ kind: "snapshot", ...state, raw: frame });
      return;
    }
    if (change.type !== "patches" || typeof change.baseRevision !== "number" || typeof change.revision !== "number" || !Array.isArray(change.patches)) return;
    const current = this.streams.get(conversationId);
    if (!current || current.ownerClientId !== ownerClientId || current.revision !== change.baseRevision) {
      const expectedRevision = current?.revision ?? 0;
      this.emitStream({
        kind: "desync",
        conversationId,
        hostId,
        ownerClientId,
        expectedRevision,
        receivedBaseRevision: change.baseRevision,
        receivedRevision: change.revision,
        raw: frame,
      });
      // Re-sending `following:true` is how the official follower asks the
      // owner for a fresh snapshot when a patch base revision is missed.
      if (this.followed.has(conversationId)) {
        this.followConversation(conversationId, true, { hostId }).catch((error) => this.emitError(asError(error)));
      }
      return;
    }
    try {
      const patches = change.patches as unknown as IpcJsonPatch[];
      const nextConversationState = applyIpcPatches(current.conversationState, patches);
      if (!isJsonObject(nextConversationState)) throw new CodexIpcError("Patched conversation state is not an object", { code: "invalid-patch" });
      const next: ConversationStreamState = {
        ...current,
        revision: change.revision,
        conversationState: nextConversationState,
      };
      this.streams.set(conversationId, next);
      this.emitStream({ kind: "patches", ...next, patches, baseRevision: change.baseRevision, raw: frame });
    } catch (error) {
      this.emitError(asError(error));
    }
  }

  private async handleDiscoveryRequest(message: IpcClientDiscoveryRequest): Promise<void> {
    const request = message.request;
    let canHandle = false;
    try {
      canHandle = this.discoveryHandler ? await this.discoveryHandler(request) : false;
    } catch {
      canHandle = false;
    }
    this.write({
      type: "client-discovery-response",
      requestId: message.requestId,
      response: { canHandle },
    });
  }

  private handleUnexpectedRequest(request: IpcRequest): void {
    try {
      this.write({
        type: "response",
        requestId: request.requestId,
        resultType: "error",
        error: "no-handler-for-request",
      });
    } catch (error) {
      this.emitError(asError(error));
    }
  }

  private async resubscribeAfterConnect(): Promise<void> {
    const subscriptions = [...this.followed.entries()];
    for (const [conversationId, hostId] of subscriptions) {
      this.write({
        type: "broadcast",
        method: "thread-stream-following-changed",
        sourceClientId: this.clientId,
        version: CODEX_IPC_METHOD_VERSIONS["thread-stream-following-changed"],
        params: { conversationId, hostId, following: true },
      });
    }
  }

  private handleClose(): void {
    const socket = this.socket;
    this.socket = undefined;
    this.decoder.reset();
    const closeError = new CodexIpcError("IPC socket closed", { code: "connection-closed" });
    for (const pending of this.pending.values()) {
      clearTimeout(pending.timer);
      pending.reject(closeError);
    }
    this.pending.clear();
    this.clientId = INITIALIZING_CLIENT_ID;
    for (const listener of this.closeListeners) safeCall(listener, closeError, (error) => this.emitError(error));
    if (!this.disposed && this.options.autoReconnect && socket) {
      this.reconnectTimer = setTimeout(() => {
        this.reconnectTimer = undefined;
        this.connect().catch((error) => this.emitError(asError(error)));
      }, this.options.reconnectDelayMs);
    }
  }

  private emitStream(event: ConversationStreamEvent): void {
    for (const listener of this.streamListeners) safeCall(listener, event, (error) => this.emitError(error));
  }

  private emitError(error: Error): void {
    for (const listener of this.errorListeners) safeCall(listener, error, () => undefined);
  }
}

function safeCall<T>(listener: Listener<T>, value: T, onError: (error: Error) => void): void {
  try {
    listener(value);
  } catch (error) {
    onError(asError(error));
  }
}

function asError(error: unknown): Error {
  return error instanceof Error ? error : new Error(String(error));
}
