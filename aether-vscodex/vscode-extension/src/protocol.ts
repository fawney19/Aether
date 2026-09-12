/**
 * Wire types shared by the relay host and the Codex app-server adapter.
 *
 * The relay intentionally treats `payload` as JSON. Keeping this boundary
 * unopinionated lets the bridge continue working when app-server adds a new
 * notification or request before this extension is updated.
 */

export type JsonPrimitive = string | number | boolean | null;
export type JsonValue = JsonPrimitive | JsonValue[] | { [key: string]: JsonValue };
export type JsonObject = { [key: string]: JsonValue };
export type JsonRpcId = string | number;

/** Preserve the JSON-RPC id type when using it as a map key. */
export function jsonRpcIdKey(id: JsonRpcId): string {
  return `${typeof id}:${String(id)}`;
}

export function isJsonRpcId(value: unknown): value is JsonRpcId {
  return typeof value === "string" || typeof value === "number";
}

export type ApprovalDecisionKind = "allow" | "deny" | "cancel";

const LEGACY_APPROVAL_METHODS = new Set(["applyPatchApproval", "execCommandApproval"]);
const V2_APPROVAL_METHODS = new Set([
  "item/commandExecution/requestApproval",
  "item/fileChange/requestApproval",
]);

/**
 * Classify both current and legacy app-server approval decisions without
 * rewriting the wire value. Unknown tagged objects intentionally return
 * `undefined` so callers can fail closed instead of accidentally approving a
 * newly introduced response shape.
 */
export function approvalDecisionKind(value: unknown): ApprovalDecisionKind | undefined {
  if (typeof value === "string") {
    if (new Set([
      "allow",
      "accept",
      "acceptForSession",
      "approved",
      "approved_for_session",
      "approved_mcp_policy_amendment",
    ]).has(value)) return "allow";
    if (new Set(["deny", "decline", "denied", "timed_out"]).has(value)) return "deny";
    if (new Set(["cancel", "abort"]).has(value)) return "cancel";
    return undefined;
  }
  if (!isRecord(value)) return undefined;
  const keys = Object.keys(value);
  if (keys.length !== 1) return undefined;
  const key = keys[0];
  const nested = value[key];
  if (isExecpolicyAmendmentTag(key, nested) || isNetworkPolicyAmendmentTag(key, nested)) return "allow";
  if (key === "denied" && isRecord(nested) && typeof nested.rejection === "string") return "deny";
  return undefined;
}

/**
 * Classify a decision against the response schema for one app-server method.
 * The generic classifier above is intentionally useful for relay envelopes;
 * this method-aware variant prevents a v2 tagged object from being sent to a
 * legacy callback (or vice versa), while retaining compatibility aliases that
 * the relay may use in its outer `decision` field.
 */
export function approvalDecisionKindForMethod(
  value: unknown,
  method?: string,
): ApprovalDecisionKind | undefined {
  const generic = approvalDecisionKind(value);
  if (!generic || !method) return generic;

  if (LEGACY_APPROVAL_METHODS.has(method)) {
    if (typeof value === "string") {
      return new Set([
        "approved",
        "approved_for_session",
        "approved_mcp_policy_amendment",
        "timed_out",
        "abort",
      ]).has(value) ? generic : undefined;
    }
    if (!isRecord(value)) return undefined;
    const key = Object.keys(value)[0];
    return key === "approved_execpolicy_amendment"
      || key === "network_policy_amendment"
      || key === "denied" ? generic : undefined;
  }

  if (V2_APPROVAL_METHODS.has(method)) {
    if (typeof value === "string") {
      return new Set(["accept", "acceptForSession", "decline", "cancel"]).has(value)
        ? generic
        : undefined;
    }
    if (!isRecord(value)) return undefined;
    const key = Object.keys(value)[0];
    if (method === "item/fileChange/requestApproval") return undefined;
    return key === "acceptWithExecpolicyAmendment" || key === "applyNetworkPolicyAmendment"
      ? generic
      : undefined;
  }

  if (method === "mcpServer/elicitation/request") {
    return typeof value === "string" && new Set(["accept", "decline", "cancel"]).has(value)
      ? generic
      : undefined;
  }

  return generic;
}

function isExecpolicyAmendmentTag(key: string, nested: unknown): boolean {
  if (!isRecord(nested)) return false;
  if (key === "acceptWithExecpolicyAmendment") {
    return isStringArray(nested.execpolicy_amendment);
  }
  if (key === "approved_execpolicy_amendment") {
    return isStringArray(nested.proposed_execpolicy_amendment);
  }
  return false;
}

function isNetworkPolicyAmendmentTag(key: string, nested: unknown): boolean {
  if (!isRecord(nested)) return false;
  if (key === "applyNetworkPolicyAmendment") {
    return isNetworkPolicyAmendment(nested.network_policy_amendment);
  }
  if (key === "network_policy_amendment") {
    return isNetworkPolicyAmendment(nested.network_policy_amendment);
  }
  return false;
}

function isStringArray(value: unknown): value is string[] {
  return Array.isArray(value) && value.every((item) => typeof item === "string");
}

function isNetworkPolicyAmendment(value: unknown): boolean {
  return isRecord(value)
    && typeof value.host === "string"
    && (value.action === "allow" || value.action === "deny");
}

/** Whether a response explicitly carries a decision/action field. */
export function hasApprovalDecisionField(value: unknown): value is Record<string, unknown> {
  return isRecord(value) && (Object.prototype.hasOwnProperty.call(value, "decision")
    || Object.prototype.hasOwnProperty.call(value, "action"));
}

export interface Disposable {
  dispose(): void;
}

export interface JsonRpcRequest {
  id: JsonRpcId;
  method: string;
  params?: JsonValue;
}

export interface JsonRpcNotification {
  method: string;
  params?: JsonValue;
}

export interface JsonRpcResponse {
  id: JsonRpcId;
  result?: JsonValue;
  error?: {
    code: number;
    message: string;
    data?: JsonValue;
  };
}

export type JsonRpcMessage = JsonRpcRequest | JsonRpcNotification | JsonRpcResponse;

export type RelayRole = "owner" | "operator" | "approver" | "viewer" | string;

export interface RelayActor {
  id?: string;
  role?: RelayRole;
}

/** A versioned relay event frame. `seq` is normally assigned by the relay. */
export interface RelayEventFrame {
  v: 1;
  kind: "event";
  type: string;
  id: string;
  sessionId: string;
  seq?: number;
  ts: string;
  actor?: RelayActor;
  payload: JsonObject;
  /** Optional typed execution projection attached by a VS Code host. */
  status?: AgentStatusSnapshot;
}

export interface RelayCommandFrame {
  v?: 1;
  kind?: "command";
  type: string;
  /** Compact relay compatibility form: `{ type: "command", method, params }`. */
  method?: string;
  params?: JsonObject;
  commandId?: string;
  id?: string;
  sessionId?: string;
  actor?: RelayActor;
  payload?: JsonObject;
  /** Some clients put the command body under `command`. */
  command?: {
    type?: string;
    commandId?: string;
    payload?: JsonObject;
    [key: string]: JsonValue | undefined;
  };
}

export interface RelayHelloFrame {
  v: 1;
  kind: "hello";
  clientType: "host" | "web" | string;
  protocol?: number;
  accessToken?: string;
  token?: string;
  lastSeq?: number;
  sessionId?: string;
  payload?: JsonObject;
}

export interface RelayAckFrame {
  v: 1;
  kind: "ack";
  sessionId: string;
  seq: number;
}

export interface RelayErrorFrame {
  v: 1;
  kind: "error";
  code: string;
  message: string;
  retryable?: boolean;
  commandId?: string;
}

export type RelayFrame =
  | RelayEventFrame
  | RelayCommandFrame
  | RelayHelloFrame
  | RelayAckFrame
  | RelayErrorFrame
  | (JsonObject & { kind?: string; v?: number });

/**
 * Live execution information projected from the official Codex conversation
 * state. The private IPC protocol can add new turn statuses/flags, so the
 * string fields intentionally remain open-ended for forward compatibility.
 */
export interface AgentStatusSnapshot {
  /** Coarse UI activity, for example `thinking`, `editing`, or `running`. */
  activity: string;
  /** Raw/normalized turn status (`inProgress`, `completed`, ...). */
  turnStatus: string;
  /** Runtime flags such as `waitingOnApproval` or `waitingOnUserInput`. */
  activeFlags: string[];
  startedAtMs?: number | null;
  durationMs?: number | null;
  /**
   * Time spent doing work in the official UI.  This deliberately differs
   * from `durationMs`: Codex starts the worked-for clock at the first work
   * item and stops it when the final assistant response starts.
   */
  workedDurationMs?: number | null;
  /** Elapsed wall-clock time for an active turn. */
  elapsedMs?: number | null;
  firstTurnWorkItemStartedAtMs?: number | null;
  finalAssistantStartedAtMs?: number | null;
  error?: JsonValue;
}

/** Official background-agent lifecycle values emitted by Codex v2 items. */
export type CollabAgentStatus =
  | "pendingInit"
  | "running"
  | "interrupted"
  | "completed"
  | "errored"
  | "shutdown"
  | "notFound"
  | string;

export type CollabAgentTool =
  | "spawnAgent"
  | "sendInput"
  | "resumeAgent"
  | "wait"
  | "closeAgent"
  | string;

export type CollabAgentToolCallStatus = "inProgress" | "completed" | "failed" | string;
export type SubAgentActivityKind = "started" | "interacted" | "interrupted" | "completed" | string;

/** Last known state for one receiver in a collabAgentToolCall item. */
export interface CollabAgentStateSnapshot {
  status: CollabAgentStatus;
  message?: string | null;
}

/**
 * Browser-safe projection of a background Codex subagent. The official
 * webview currently uses the four coarse statuses below; the string union is
 * deliberately open so a newer app-server status does not break the relay.
 */
export interface SubagentSnapshot {
  threadId: string;
  displayName: string | null;
  prompt: string | null;
  /** Alias used by the subagent side panel for the same prompt text. */
  objective?: string | null;
  status: "waiting" | "working" | "done" | "failed" | string;
  statusMessage: string | null;
  startedAtMs?: number | null;
  completedAtMs?: number | null;
  canInteract?: boolean;
  model?: string | null;
  agentPath?: string | null;
  parentThreadId?: string | null;
}

export interface AgentEvent {
  /** Normalized relay event name, for example `output.chunk`. */
  type: string;
  threadId?: string;
  turnId?: string;
  requestId?: JsonRpcId;
  payload: JsonObject;
  /** Original app-server notification/request, when available. */
  raw?: JsonValue;
  /** Optional typed projection of live Codex turn/runtime status. */
  status?: AgentStatusSnapshot;
}

export interface PendingApproval {
  requestId: JsonRpcId;
  method: string;
  threadId?: string;
  turnId?: string;
  itemId?: string;
  action: string;
  risk: "low" | "medium" | "high" | "unknown";
  summary: string;
  /** SHA-256 of canonicalized, unredacted app-server request params. */
  commandHash?: string;
  createdAt: number;
  expiresAt?: number;
  payload: JsonObject;
}

export interface SessionSnapshot {
  threadId: string | null;
  turnId: string | null;
  state: string;
  pendingApprovals: PendingApproval[];
  pendingRequests?: Array<{
    requestId: JsonRpcId;
    method: string;
    params?: JsonValue;
    commandHash?: string;
    risk?: string;
    summary?: string;
    createdAt?: number;
    expiresAt?: number;
  }>;
  outputTail: string;
  /** Optional role-aware projection used by the browser renderer. */
  messages?: JsonValue[];
  /** Background/inline subagents reconstructed from official collab items. */
  subagents?: SubagentSnapshot[];
  /** Live execution projection; retained alongside the legacy `state` field. */
  status?: AgentStatusSnapshot;
  /** Convenience aliases for clients that do not consume `status` yet. */
  activity?: string;
  turnStatus?: string;
  activeFlags?: string[];
  startedAtMs?: number | null;
  durationMs?: number | null;
  workedDurationMs?: number | null;
  elapsedMs?: number | null;
  metadata?: JsonObject;
}

/** A live VS Code Codex conversation that the attach bridge has verified. */
export interface SessionListEntry {
  threadId: string;
  title: string;
  updatedAtMs: number | null;
  cwd?: string | null;
  active: boolean;
  /** True for attach-mode results; retained for wire compatibility. */
  available: boolean;
}

export interface SessionListResult {
  sessions: SessionListEntry[];
  activeThreadId: string | null;
}

/** Which owner controls conversation navigation for the remote surface. */
export type ControlMode = "sync" | "async";

export interface AgentAdapter {
  start(): Promise<void>;
  /** Switch between following VS Code and independently owned conversations. */
  setControlMode?(params: JsonObject): Promise<JsonValue>;
  /** Return the currently committed control mode without taking a snapshot. */
  getControlMode?(): ControlMode;
  /** Start a new app-server thread. */
  startThread?(params?: JsonObject): Promise<JsonValue>;
  /** Ask the official VS Code Codex extension to open a fresh conversation. */
  newSession?(params?: JsonObject): Promise<JsonValue>;
  /** Start a turn; `threadId` may be supplied in params or use the active thread. */
  startTurn?(params: JsonObject): Promise<JsonValue>;
  /** Steer the active turn. */
  steerTurn?(params: JsonObject): Promise<JsonValue>;
  /** Persist model/effort and other owner-managed settings on the thread. */
  updateThreadSettings?(params: JsonObject): Promise<JsonValue>;
  /** List verified, attachable local conversations without starting another Codex process. */
  listSessions?(params?: JsonObject): Promise<JsonValue>;
  /** Attach the follower to another already-open conversation. */
  selectSession?(params: JsonObject): Promise<JsonValue>;
  /** Interrupt a turn. */
  interruptTurn?(params: JsonObject): Promise<JsonValue>;
  /** Convenience MVP aliases. */
  sendInput(text: string, params?: JsonObject): Promise<JsonValue>;
  cancel(taskId?: string, params?: JsonObject): Promise<JsonValue>;
  respondApproval(
    requestId: JsonRpcId,
    decision: "allow" | "deny" | "cancel",
    reason?: string,
    response?: JsonValue,
  ): Promise<JsonValue>;
  /** Resolve all pending approvals/inputs with a deny response. */
  denyPending?(reason?: string): Promise<void>;
  snapshot(): Promise<SessionSnapshot>;
  onEvent(listener: (event: AgentEvent) => void): Disposable;
  dispose(): Promise<void>;
}

export interface RelayTransport {
  connect(): Promise<void>;
  send(frame: RelayFrame): void;
  onMessage(listener: (frame: RelayFrame) => void): Disposable;
  onOpen?(listener: () => void): Disposable;
  onClose?(listener: (error?: Error) => void): Disposable;
  close(): void;
}

export interface Logger {
  debug?(message: string, ...args: unknown[]): void;
  info?(message: string, ...args: unknown[]): void;
  warn?(message: string, ...args: unknown[]): void;
  error?(message: string, ...args: unknown[]): void;
}

export const noopDisposable = (): Disposable => ({ dispose: () => undefined });

export function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

export function asJsonObject(value: unknown): JsonObject {
  return isRecord(value) ? (value as JsonObject) : {};
}

export function asJsonValue(value: unknown): JsonValue {
  if (value === undefined) return null;
  if (value === null || typeof value === "string" || typeof value === "number" || typeof value === "boolean") {
    return value;
  }
  if (Array.isArray(value)) {
    return value.map(asJsonValue);
  }
  if (isRecord(value)) {
    const output: JsonObject = {};
    for (const [key, item] of Object.entries(value)) {
      if (item !== undefined) output[key] = asJsonValue(item);
    }
    return output;
  }
  return String(value);
}
