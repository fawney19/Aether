import { randomUUID } from "node:crypto";

import {
  AgentAdapter,
  AgentEvent,
  approvalDecisionKind,
  approvalDecisionKindForMethod,
  asJsonObject,
  asJsonValue,
  Disposable,
  hasApprovalDecisionField,
  isRecord,
  JsonObject,
  JsonRpcId,
  Logger,
  JsonValue,
  RelayActor,
  RelayCommandFrame,
  RelayEventFrame,
  RelayFrame,
  RelayTransport,
} from "./protocol";

export interface RelayHostOptions {
  adapter: AgentAdapter;
  relay: RelayTransport;
  sessionId?: string;
  actor?: RelayActor;
  /** Capabilities enforced locally even when relay authorization is bypassed. */
  capabilities?: Iterable<string>;
  logger?: Logger;
  /** Emit a handshake on transports that do not implement one themselves. */
  sendHandshake?: boolean;
}

/**
 * Maps relay commands to the app-server AgentAdapter and publishes normalized
 * adapter events. This is the policy boundary for the VS Code host.
 */
export class RelayHost {
  private readonly adapter: AgentAdapter;
  private readonly relay: RelayTransport;
  private readonly options: RelayHostOptions;
  private readonly subscriptions: Disposable[] = [];
  private readonly commandResults = new Map<string, RelayEventFrame>();
  private readonly inFlightCommands = new Set<string>();
  private readonly capabilities: Set<string>;
  private eventSeq = 0;
  private sessionId: string;
  private started = false;
  private adapterReady = false;

  constructor(options: RelayHostOptions);
  constructor(adapter: AgentAdapter, relay: RelayTransport, options?: Omit<RelayHostOptions, "adapter" | "relay">);
  constructor(
    optionsOrAdapter: RelayHostOptions | AgentAdapter,
    relayArg?: RelayTransport,
    legacyOptions: Omit<RelayHostOptions, "adapter" | "relay"> = {},
  ) {
    if (isAgentAdapter(optionsOrAdapter)) {
      this.adapter = optionsOrAdapter;
      if (!relayArg) throw new Error("RelayHost requires a relay transport");
      this.relay = relayArg;
      this.options = { ...legacyOptions, adapter: this.adapter, relay: this.relay };
    } else {
      this.options = optionsOrAdapter;
      this.adapter = optionsOrAdapter.adapter;
      this.relay = optionsOrAdapter.relay;
    }
    this.capabilities = new Set(this.options.capabilities ?? [
      "read_output",
      "send_task_input",
      "cancel_task",
      "approve_low_risk",
    ]);
    this.sessionId = this.options.sessionId ?? `sess_${randomUUID()}`;
  }

  get id(): string {
    return this.sessionId;
  }

  async start(): Promise<void> {
    if (this.started) return;
    this.started = true;
    this.subscriptions.push(this.adapter.onEvent((event) => {
      if (event.type === "connection.opened") this.adapterReady = true;
      if (event.type === "connection.closed") this.adapterReady = false;
      this.publishAgentEvent(event);
    }));
    this.subscriptions.push(this.relay.onMessage((frame) => {
      void this.handleFrame(frame).catch((error) => {
        this.options.logger?.warn?.("Invalid relay frame", error);
        if (isRecord(frame) && typeof frame.commandId === "string") {
          this.sendCommandResult(frame.commandId, false, undefined, error instanceof Error ? error.message : String(error), typeof frame.method === "string" ? frame.method : typeof frame.type === "string" ? frame.type : undefined);
        }
      });
    }));
    if (this.relay.onClose) this.subscriptions.push(this.relay.onClose((error) => {
      // A relay disconnect must not leave an app-server request waiting for a
      // browser that can no longer answer. The adapter's local deny path is
      // deliberately fail-closed. Do not publish `connection.closed` here:
      // that event describes the app-server process, while this callback only
      // describes the outbound transport and is followed by connection.opened
      // on a successful reconnect.
      void this.adapter.denyPending?.("relay disconnected");
      this.options.logger?.debug?.("Relay transport closed", error?.message ?? "");
    }));
    if (this.relay.onOpen) this.subscriptions.push(this.relay.onOpen(() => {
      // RelayClient fires onOpen only after auth.ok. On reconnect the adapter
      // is already initialized, so the synthetic event restores relay state;
      // during initial startup the adapter event below is authoritative.
      if (this.adapterReady) {
        this.publishConnectionEvent("connection.opened");
        void this.publishSnapshot();
      }
    }));

    const configurableRelay = this.relay as RelayTransport & { setSessionId?: (sessionId: string) => void };
    configurableRelay.setSessionId?.(this.sessionId);
    try {
      await this.relay.connect();
      if (this.options.sendHandshake !== false && !transportHandlesHandshake(this.relay)) {
        this.safeSend({ v: 1, kind: "hello", clientType: "host", protocol: 1, sessionId: this.sessionId });
      }
      // Start app-server only after the relay handshake is queued/sent. This
      // keeps standalone stdout frames protocol-ordered and prevents an early
      // notification from racing the host hello.
      await this.adapter.start();
      if (!this.adapterReady) {
        this.adapterReady = true;
        this.publishConnectionEvent("connection.opened");
      }
      await this.publishSnapshot();
    } catch (error) {
      this.started = false;
      this.adapterReady = false;
      for (const subscription of this.subscriptions.splice(0)) subscription.dispose();
      this.relay.close();
      await this.adapter.dispose().catch(() => undefined);
      throw error;
    }
  }

  async stop(): Promise<void> {
    if (!this.started) return;
    this.started = false;
    this.adapterReady = false;
    this.inFlightCommands.clear();
    for (const subscription of this.subscriptions.splice(0)) subscription.dispose();
    this.relay.close();
    await this.adapter.dispose();
  }

  /** Public for unit tests and local stdin bridges. */
  async handleFrame(frame: RelayFrame): Promise<void> {
    if (!isRecord(frame)) return;
    if (frame.kind === "command" || isCommandLike(frame)) {
      await this.handleCommand(frame as unknown as RelayCommandFrame);
      return;
    }
    if (frame.kind === "event" && typeof frame.seq === "number") {
      this.safeSend({ v: 1, kind: "ack", sessionId: frame.sessionId, seq: frame.seq });
    }
  }

  private async handleCommand(frame: RelayCommandFrame): Promise<void> {
    const command = normalizeCommand(frame);
    const commandId = command.commandId;
    if (commandId) {
      const previous = this.commandResults.get(commandId);
      if (previous) {
        this.safeSend(previous);
        return;
      }
      if (this.inFlightCommands.has(commandId)) {
        this.safeSend({
          v: 1,
          kind: "event",
          // Do not call this `command.accepted`: the relay treats that event
          // as the terminal result for its pending command. A retry while the
          // original operation is running is only an informational event.
          type: "command.pending",
          id: `evt_${randomUUID()}`,
          sessionId: this.sessionId,
          seq: ++this.eventSeq,
          ts: new Date().toISOString(),
          actor: this.options.actor ?? { id: "host", role: "host" },
          payload: { commandId, duplicate: true, pending: true },
        });
        return;
      }
      this.inFlightCommands.add(commandId);
    }

    const role = frame.actor?.role ?? "operator";
    const denied = authorize(command.type, role, this.capabilities);
    if (denied) {
      // A viewer may not force a pending approval to deny (that would turn a
      // read-only role into a denial-of-service primitive). Authorized roles
      // can still be rejected by local capability/policy checks, in which
      // case denying the app-server request is the safe terminal action.
      if (role === "owner" || role === "operator" || role === "approver" || role === "host") {
        await this.denyApprovalIfNeeded(command.type, command.payload, denied);
      }
      this.sendCommandResult(commandId, false, undefined, denied, command.type);
      if (commandId) this.inFlightCommands.delete(commandId);
      return;
    }

    try {
      const result = await this.executeCommand(command.type, command.payload);
      this.sendCommandResult(commandId, true, result, undefined, command.type);
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      this.options.logger?.warn?.(`Relay command ${command.type} failed`, error);
      await this.denyApprovalIfNeeded(command.type, command.payload, message);
      this.sendCommandResult(commandId, false, undefined, message, command.type);
    } finally {
      if (commandId) this.inFlightCommands.delete(commandId);
    }
  }

  private async denyApprovalIfNeeded(type: string, payload: JsonObject, reason: string): Promise<void> {
    const command = canonicalCommandType(type);
    if (command !== "approval.respond" && command !== "input.respond" && command !== "server.request.respond") return;
    const requestId = payload.requestId;
    if (requestId === undefined || (typeof requestId !== "string" && typeof requestId !== "number")) return;
    try {
      await this.adapter.respondApproval(requestId, "deny", reason);
    } catch {
      // The request may already have expired or been resolved. Keep the
      // original command rejection as the observable result.
    }
  }

  private async executeCommand(type: string, payload: JsonObject): Promise<unknown> {
    switch (canonicalCommandType(type)) {
      case "control.mode.get": {
        const mode = this.adapter.getControlMode?.();
        if (mode) return { mode };
        const snapshot = await this.adapter.snapshot();
        const controlMode = snapshot.metadata?.controlMode;
        if (controlMode !== "sync" && controlMode !== "async") {
          throw new Error("adapter does not expose a control mode");
        }
        return { mode: controlMode };
      }
      case "control.mode.set":
        if (!this.adapter.setControlMode) throw new Error("adapter does not support control mode switching");
        return this.adapter.setControlMode(payload);
      case "thread.start":
        if (!this.adapter.startThread) throw new Error("adapter does not support thread/start");
        return this.adapter.startThread(payload);
      case "session.new":
        if (!this.adapter.newSession) throw new Error("adapter does not support session/new");
        return this.adapter.newSession(payload);
      case "thread.settings.update":
        if (!this.adapter.updateThreadSettings) throw new Error("adapter does not support thread/settings/update");
        return this.adapter.updateThreadSettings(payload);
      case "session.list":
        if (!this.adapter.listSessions) throw new Error("adapter does not support session/list");
        return this.adapter.listSessions(payload);
      case "session.select":
        if (!this.adapter.selectSession) throw new Error("adapter does not support session/select");
        return this.adapter.selectSession(payload);
      case "turn.start":
        if (this.adapter.startTurn) return this.adapter.startTurn(payload);
        return this.adapter.sendInput(extractCommandText(payload), payload);
      case "turn.steer":
        if (this.adapter.steerTurn) return this.adapter.steerTurn(payload);
        return this.adapter.sendInput(extractCommandText(payload), payload);
      case "turn.interrupt":
        if (this.adapter.interruptTurn) return this.adapter.interruptTurn(payload);
        return this.adapter.cancel(typeof payload.turnId === "string" ? payload.turnId : undefined, payload);
      case "task.input": {
        const text = typeof payload.text === "string" ? payload.text : typeof payload.message === "string" ? payload.message : undefined;
        if (!text) throw new Error("task.input requires payload.text");
        return this.adapter.sendInput(text, payload);
      }
      case "task.cancel": {
        const taskId = typeof payload.taskId === "string" ? payload.taskId : typeof payload.turnId === "string" ? payload.turnId : undefined;
        return this.adapter.cancel(taskId, payload);
      }
      case "approval.respond": {
        const requestId = payload.requestId;
        if (typeof requestId !== "string" && typeof requestId !== "number") throw new Error("approval.respond requires requestId");
        const requestedValue = payload.decision;
        const decision = approvalDecisionKind(requestedValue);
        if (!decision) throw new Error("decision must be a recognized allow, deny, or cancel value");
        const snapshot = await this.adapter.snapshot();
        // JSON-RPC distinguishes numeric and string ids. Keep the lookup
        // type-safe so id `1` cannot accidentally authorize response `"1"`.
        const approval = snapshot.pendingApprovals.find((item) => item.requestId === requestId);
        const method = approval?.method ?? (typeof payload.method === "string" ? payload.method : undefined);
        const response = payload.response ?? implicitApprovalResponse(requestedValue, decision, method, payload.scope);
        if (decision === "allow") {
          if (approval && typeof payload.commandHash === "string" && payload.commandHash !== approval.commandHash) {
            throw new Error("approval commandHash does not match the pending request");
          }
          if (approval?.risk === "high" && !this.capabilities.has("approve_high_risk") && !this.capabilities.has("*")) {
            throw new Error("host policy requires approve_high_risk for this approval");
          }
        }
        validateApprovalResponse(decision, response, method);
        return this.adapter.respondApproval(
          requestId,
          decision,
          typeof payload.reason === "string" ? payload.reason : undefined,
          response,
        );
      }
      case "input.respond":
      case "server.request.respond": {
        const requestId = payload.requestId;
        if (typeof requestId !== "string" && typeof requestId !== "number") throw new Error(`${type} requires requestId`);
        const response = payload.response ?? (payload.answers !== undefined ? payload.answers : undefined);
        // Tool-input requests do not carry an allow/deny field in their wire
        // response, while MCP elicitation uses `action`. Prefer an explicit
        // response action when present; otherwise honor the relay decision and
        // fail closed when a denial has no custom response.
        if (response !== undefined && !isRecord(response)) {
          throw new Error("input response must be a JSON object");
        }
        const responseDecision = isRecord(response)
          ? explicitResponseDecision(response)
          : undefined;
        const requestedDecision = payload.decision === undefined
          ? undefined
          : approvalDecisionKind(payload.decision);
        if (payload.decision !== undefined && !requestedDecision) {
          throw new Error("decision must be allow, deny, or cancel");
        }
        if (responseDecision && requestedDecision
          && responseDecision !== requestedDecision
          // RelayHost uses `decision: "allow"` as a generic envelope for
          // MCP/input responses; the nested action remains authoritative in
          // that one compatibility case.
          && requestedDecision !== "allow") {
          throw new Error(`input response implies ${responseDecision}, but decision is ${requestedDecision}`);
        }
        // For MCP, `response.action` is the actual app-server decision and is
        // authoritative even if a relay uses `decision: "allow"` as a generic
        // input-response envelope. With no custom response, an explicit relay
        // decision (or the fail-closed deny default) controls the result.
        const decision = responseDecision ?? requestedDecision ?? (response === undefined ? "deny" : "allow");
        const responseForAdapter = requestedDecision && requestedDecision !== "allow" && !responseDecision
          ? undefined
          : response;
        return this.adapter.respondApproval(
          requestId,
          decision,
          typeof payload.reason === "string" ? payload.reason : undefined,
          responseForAdapter,
        );
      }
      case "session.snapshot":
      case "snapshot":
        return this.adapter.snapshot();
      case "ping":
        return { pong: true, ts: new Date().toISOString() };
      default:
        throw new Error(`unsupported relay command: ${type}`);
    }
  }

  private sendCommandResult(commandId: string | undefined, ok: boolean, result?: unknown, error?: string, method?: string): void {
    const frame: RelayEventFrame = {
      v: 1,
      kind: "event",
      type: ok ? "command.accepted" : "command.rejected",
      id: `evt_${randomUUID()}`,
      sessionId: this.sessionId,
      seq: ++this.eventSeq,
      ts: new Date().toISOString(),
      actor: this.options.actor ?? { id: "host", role: "host" },
      payload: {
        ...(commandId ? { commandId } : {}),
        ...(method ? { method } : {}),
        ok,
        ...(ok ? { result: asJsonValue(result) } : { error: error ?? "command rejected" }),
      },
    };
    if (commandId) {
      this.commandResults.set(commandId, frame);
      if (this.commandResults.size > 1000) this.commandResults.delete(this.commandResults.keys().next().value as string);
    }
    this.safeSend(frame);
  }

  private publishAgentEvent(event: AgentEvent): void {
    if (event.threadId && this.sessionId.startsWith("sess_")) {
      // Keep a stable relay session id while exposing the app-server thread id
      // in the payload; a relay session may contain more than one thread.
    }
    const payload: JsonObject = {
      ...event.payload,
      ...(event.status ? { executionStatus: asJsonValue(event.status) } : {}),
      ...(event.threadId ? { threadId: event.threadId } : {}),
      ...(event.turnId ? { turnId: event.turnId } : {}),
      ...(event.requestId !== undefined ? { requestId: asJsonValue(event.requestId) } : {}),
      ...(event.raw !== undefined ? { raw: event.raw } : {}),
    };
    const frame: RelayEventFrame = {
      v: 1,
      kind: "event",
      type: event.type,
      id: `evt_${randomUUID()}`,
      sessionId: this.sessionId,
      seq: ++this.eventSeq,
      ts: new Date().toISOString(),
      actor: this.options.actor ?? { id: "host", role: "host" },
      payload,
      ...(event.status ? { status: { ...event.status, activeFlags: [...event.status.activeFlags] } } : {}),
    };
    try {
      this.safeSend(frame);
    } catch (error) {
      this.options.logger?.warn?.("Unable to publish relay event", error);
    }
  }

  private safeSend(frame: RelayFrame): void {
    try {
      this.relay.send(frame);
    } catch (error) {
      this.options.logger?.warn?.("Unable to send relay frame", error);
    }
  }

  private publishConnectionEvent(type: string, error?: Error): void {
    if (!this.started && type === "connection.closed") return;
    this.publishAgentEvent({ type, payload: error ? { message: error.message } : {} });
  }

  private async publishSnapshot(): Promise<void> {
    try {
      const snapshot = await this.adapter.snapshot();
      this.publishAgentEvent({
        type: "session.snapshot",
        threadId: snapshot.threadId ?? undefined,
        turnId: snapshot.turnId ?? undefined,
        payload: asJsonObject(snapshot),
        status: snapshot.status,
      });
    } catch (error) {
      this.options.logger?.warn?.("Unable to publish adapter snapshot", error);
    }
  }
}

interface NormalizedCommand {
  type: string;
  commandId?: string;
  payload: JsonObject;
}

function normalizeCommand(frame: RelayCommandFrame): NormalizedCommand {
  const nested = isRecord(frame.command) ? frame.command : undefined;
  const type = typeof nested?.type === "string"
    ? nested.type
    : typeof frame.method === "string"
      ? frame.method
      : frame.type === "command"
        ? ""
        : frame.type;
  const commandId = typeof frame.commandId === "string"
    ? frame.commandId
    : typeof nested?.commandId === "string"
      ? nested.commandId
      : typeof frame.id === "string"
        ? frame.id
        : undefined;
  if (!type) throw new Error("relay command has no type");

  if (isRecord(nested?.payload)) return { type, commandId, payload: asJsonObject(nested.payload) };
  if (isRecord(frame.payload)) return { type, commandId, payload: asJsonObject(frame.payload) };
  if (isRecord(frame.params)) return { type, commandId, payload: asJsonObject(frame.params) };

  const payload: JsonObject = {};
  for (const [key, value] of Object.entries(frame)) {
    if (["v", "kind", "type", "method", "params", "commandId", "id", "sessionId", "actor", "command"].includes(key)) continue;
    if (value !== undefined) payload[key] = asJsonValue(value);
  }
  return { type, commandId, payload };
}

function canonicalCommandType(type: string): string {
  const normalized = type.trim().replace(/\//g, ".").replace(/\s+/g, ".").toLowerCase();
  if (normalized === "control.mode.get" || normalized === "controlmode.get" || normalized === "controlmodeget" || normalized === "mode.get" || normalized === "modeget") return "control.mode.get";
  if (normalized === "control.mode.set" || normalized === "controlmode.set" || normalized === "controlmodeset" || normalized === "mode.set" || normalized === "modeset") return "control.mode.set";
  if (normalized === "thread.start" || normalized === "threadstart") return "thread.start";
  if (normalized === "session.new" || normalized === "sessionnew" || normalized === "thread.new" || normalized === "threadnew") return "session.new";
  if (normalized === "thread.settings.update" || normalized === "threadsettings.update" || normalized === "threadsettingsupdate") return "thread.settings.update";
  if (normalized === "session.list" || normalized === "thread.list" || normalized === "sessionlist" || normalized === "threadlist") return "session.list";
  if (normalized === "session.select" || normalized === "session.switch" || normalized === "thread.select" || normalized === "thread.attach" || normalized === "sessionswitch" || normalized === "threadselect") return "session.select";
  if (normalized === "turn.start" || normalized === "turnstart") return "turn.start";
  if (normalized === "turn.steer" || normalized === "turnsteer") return "turn.steer";
  if (normalized === "turn.interrupt" || normalized === "turninterrupt") return "turn.interrupt";
  if (normalized === "approval.respond" || normalized === "approvalrespond") return "approval.respond";
  if (normalized === "task.input" || normalized === "taskinput") return "task.input";
  if (normalized === "task.cancel" || normalized === "taskcancel") return "task.cancel";
  if (normalized === "input.respond" || normalized === "inputrespond") return "input.respond";
  if (normalized === "server.request.respond" || normalized === "serverrequest.respond") return "server.request.respond";
  if (normalized === "session.snapshot" || normalized === "snapshot") return "session.snapshot";
  return normalized;
}

function authorize(type: string, role: string, capabilities: Set<string>): string | undefined {
  const command = canonicalCommandType(type);
  const readOnly = command === "session.snapshot" || command === "snapshot" || command === "session.list" || command === "control.mode.get" || command === "ping";
  if (readOnly) return undefined;
  if (role === "viewer") return "viewer role cannot issue control commands";
  if (role !== "owner" && role !== "operator" && role !== "approver" && role !== "host") return `role ${role} is not authorized`;
  if ((command === "approval.respond" || command === "input.respond" || command === "server.request.respond") && role !== "owner" && role !== "operator" && role !== "approver" && role !== "host") {
    return "role is not authorized to resolve approvals";
  }
  const required = command === "approval.respond" ? "approve_low_risk" : command === "task.cancel" || command === "turn.interrupt" ? "cancel_task" : command === "task.input" || command.startsWith("turn.") || command === "thread.start" || command === "thread.settings.update" || command === "session.select" || command === "session.new" || command === "control.mode.set" ? "send_task_input" : undefined;
  if (required && !capabilities.has(required) && !capabilities.has("*") && role !== "owner" && role !== "host") return `missing capability: ${required}`;
  return undefined;
}

function isCommandLike(frame: Record<string, unknown>): boolean {
  if (frame.kind === "command") return true;
  if (frame.type === "command" && typeof frame.method === "string") return true;
  if (frame.kind !== undefined) return false;
  if (typeof frame.method === "string") return true;
  if (typeof frame.commandId !== "string") return false;
  return KNOWN_COMMAND_TYPES.has(String(frame.type).trim().replace(/\//g, ".").toLowerCase());
}

const KNOWN_COMMAND_TYPES = new Set([
  "control.mode.get",
  "control.mode.set",
  "thread.start",
  "session.new",
  "thread.settings.update",
  "session.list",
  "session.select",
  "turn.start",
  "turn.steer",
  "turn.interrupt",
  "approval.respond",
  "task.input",
  "task.cancel",
  "input.respond",
  "server.request.respond",
  "session.snapshot",
  "snapshot",
  "ping",
]);

function isAgentAdapter(value: unknown): value is AgentAdapter {
  return isRecord(value) && typeof value.start === "function" && typeof value.onEvent === "function" && typeof value.sendInput === "function" && typeof value.cancel === "function" && typeof value.respondApproval === "function" && typeof value.snapshot === "function";
}

function extractCommandText(payload: JsonObject): string {
  if (typeof payload.text === "string") return payload.text;
  if (typeof payload.message === "string") return payload.message;
  if (typeof payload.prompt === "string") return payload.prompt;
  if (Array.isArray(payload.input)) {
    const first = payload.input[0];
    if (isRecord(first) && typeof first.text === "string") return first.text;
  }
  throw new Error("turn command requires text or input");
}

function validateApprovalResponse(
  decision: "allow" | "deny" | "cancel",
  response: JsonValue | undefined,
  method?: string,
): void {
  if (response === undefined) return;
  if (!isRecord(response)) throw new Error("approval response must be a JSON object");

  const hasDecision = Object.prototype.hasOwnProperty.call(response, "decision");
  const hasAction = Object.prototype.hasOwnProperty.call(response, "action");
  if (hasDecision || hasAction) {
    const decisionKind = hasDecision ? approvalDecisionKindForMethod(response.decision, method) : undefined;
    const actionKind = hasAction ? approvalDecisionKindForMethod(response.action, method) : undefined;
    if (hasDecision && !decisionKind) throw new Error("unsupported approval response decision");
    if (hasAction && !actionKind) throw new Error("unsupported approval response action");
    if (decisionKind && actionKind && decisionKind !== actionKind) {
      throw new Error("approval response decision and action conflict");
    }
    const implied = decisionKind ?? actionKind;
    if (implied && implied !== decision) {
      throw new Error(`approval response implies ${implied}, but decision is ${decision}`);
    }
    return;
  }

  // Permissions approvals intentionally carry a profile rather than a
  // decision field. Keep the profile shape narrow; malformed/unknown objects
  // must not be interpreted as an approval.
  if (method === "item/permissions/requestApproval"
    && isRecord(response.permissions)
    && (response.scope === "turn" || response.scope === "session")
    && (response.strictAutoReview === undefined || typeof response.strictAutoReview === "boolean")
    && Object.keys(response).every((key) => key === "permissions" || key === "scope" || key === "strictAutoReview")) {
    return;
  }
  throw new Error("approval response has no recognized decision or permission profile");
}

/**
 * Convert a relay's compact outer decision into a wire response only when it
 * carries a non-canonical app-server value. Canonical `allow`/`deny`/`cancel`
 * remain undefined so the adapter can choose the method-specific default.
 */
function implicitApprovalResponse(
  requestedValue: JsonValue,
  decision: "allow" | "deny" | "cancel",
  method?: string,
  scope?: JsonValue,
): JsonValue | undefined {
  // Permission approvals have a profile response, not a decision wrapper.
  // Let the adapter construct the requested turn-scoped profile by default;
  // callers that need session scope must provide the full profile explicitly.
  if (method === "item/permissions/requestApproval") return undefined;
  if (requestedValue === "allow" || requestedValue === "deny" || requestedValue === "cancel") {
    if (requestedValue === "allow" && scope === "session") {
      if (method === "applyPatchApproval" || method === "execCommandApproval") return { decision: "approved_for_session" };
      if (method === "item/commandExecution/requestApproval" || method === "item/fileChange/requestApproval") return { decision: "acceptForSession" };
    }
    return undefined;
  }
  // The generic classifier has already rejected unknown/conflicting values.
  // Preserve recognized legacy/v2 tags exactly under the app-server wrapper.
  if (decision === "allow" || decision === "deny" || decision === "cancel") {
    return { decision: requestedValue };
  }
  return undefined;
}

function explicitResponseDecision(response: Record<string, unknown>): "allow" | "deny" | "cancel" | undefined {
  if (!hasApprovalDecisionField(response)) return undefined;
  const hasDecision = Object.prototype.hasOwnProperty.call(response, "decision");
  const hasAction = Object.prototype.hasOwnProperty.call(response, "action");
  const decision = hasDecision ? approvalDecisionKind(response.decision) : undefined;
  const action = hasAction ? approvalDecisionKind(response.action) : undefined;
  if (hasDecision && !decision) throw new Error("unsupported input response decision");
  if (hasAction && !action) throw new Error("unsupported input response action");
  if (decision && action && decision !== action) throw new Error("input response decision and action conflict");
  return decision ?? action;
}

function transportHandlesHandshake(transport: RelayTransport): boolean {
  return Boolean((transport as RelayTransport & { handlesHandshake?: boolean }).handlesHandshake);
}
