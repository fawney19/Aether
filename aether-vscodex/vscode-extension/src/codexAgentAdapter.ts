import { createHash } from "node:crypto";

import {
  AgentAdapter,
  AgentEvent,
  asJsonObject,
  asJsonValue,
  approvalDecisionKindForMethod,
  Disposable,
  hasApprovalDecisionField,
  isRecord,
  JsonObject,
  JsonRpcId,
  JsonRpcRequest,
  JsonValue,
  Logger,
  PendingApproval,
  SessionSnapshot,
  isJsonRpcId,
  jsonRpcIdKey,
} from "./protocol";
import { JsonlRpcClient, JsonlRpcClientOptions } from "./jsonlRpc";

const APPROVAL_METHODS = new Set([
  "item/commandExecution/requestApproval",
  "item/fileChange/requestApproval",
  "item/permissions/requestApproval",
  "applyPatchApproval",
  "execCommandApproval",
]);

const INPUT_REQUEST_METHODS = new Set([
  "item/tool/requestUserInput",
  "mcpServer/elicitation/request",
]);

// Paginated threads can contain many thousands of items. Keep hydration
// bounded so one history request cannot exhaust the extension host or exceed
// the relay frame limit, while still covering normal long-running sessions.
const HISTORY_PAGE_SIZE = 100;
const MAX_HISTORY_TURN_PAGES = 100;
const MAX_HISTORY_TURNS = HISTORY_PAGE_SIZE * MAX_HISTORY_TURN_PAGES;
const MAX_HISTORY_ITEM_PAGES = 100;

export interface CodexAgentAdapterOptions extends JsonlRpcClientOptions {
  clientName?: string;
  clientTitle?: string | null;
  clientVersion?: string;
  initializeCapabilities?: JsonObject;
  defaultCwd?: string;
  maxOutputTailChars?: number;
  approvalTimeoutMs?: number;
  /** Handle non-approval server requests (auth refresh, tool calls, etc.). */
  onServerRequest?: (request: JsonRpcRequest) => Promise<JsonValue | undefined>;
  autoRejectUnsupportedRequests?: boolean;
}

interface PendingRequest {
  request: JsonRpcRequest;
  approval?: PendingApproval;
  timer?: NodeJS.Timeout;
}

/**
 * AgentAdapter implementation backed by a child `codex app-server --stdio`.
 *
 * It deliberately does not shell out for individual tasks. All task and
 * approval operations go through the app-server JSON-RPC channel.
 */
export class CodexAgentAdapter implements AgentAdapter {
  readonly rpc: JsonlRpcClient;
  private readonly options: Required<
    Pick<CodexAgentAdapterOptions, "clientName" | "clientVersion" | "maxOutputTailChars" | "approvalTimeoutMs">
  > &
    Omit<CodexAgentAdapterOptions, "clientName" | "clientVersion" | "maxOutputTailChars" | "approvalTimeoutMs">;
  private readonly listeners = new Set<(event: AgentEvent) => void>();
  private readonly pending = new Map<string, PendingRequest>();
  private threadId: string | null = null;
  private turnId: string | null = null;
  private state = "disconnected";
  private outputTail = "";
  private messages: JsonValue[] = [];
  private sessionMetadata: JsonObject = {};
  private availableModels: JsonValue[] = [];
  private historyComplete = true;
  private sessionSwitching = false;
  private started = false;
  private readonly rpcDisposables: Disposable[];

  constructor(options: CodexAgentAdapterOptions = {}, rpc?: JsonlRpcClient) {
    this.options = {
      clientName: options.clientName ?? "codex-remote-collab",
      clientVersion: options.clientVersion ?? "0.4.0",
      maxOutputTailChars: options.maxOutputTailChars ?? 32_000,
      approvalTimeoutMs: options.approvalTimeoutMs ?? 5 * 60_000,
      ...options,
    };
    this.rpc = rpc ?? new JsonlRpcClient(options);
    this.rpcDisposables = [
      this.rpc.onNotification((message) => this.handleNotification(message.method, message.params)),
      this.rpc.onServerRequest((request) => this.handleServerRequest(request)),
      this.rpc.onExit((error) => {
        this.started = false;
        this.state = "disconnected";
        // A new app-server process cannot safely reuse ids from the dead
        // process. Clear them before publishing the terminal event so a
        // reconnect/restart cannot steer or interrupt a stale turn.
        this.threadId = null;
        this.turnId = null;
        this.outputTail = "";
        this.messages = [];
        this.sessionMetadata = {};
        this.historyComplete = true;
        this.sessionSwitching = false;
        // The child cannot receive a response after exit. Drop every pending
        // approval/input and publish an explicit expiry so the relay removes
        // its corresponding request instead of retaining a stale request id.
        this.dropPendingRequests(error?.message ?? "app-server exited");
        this.emit({ type: "connection.closed", payload: error ? { message: error.message } : {} });
      }),
    ];
  }

  async start(): Promise<void> {
    if (this.started) return;
    await this.rpc.start();
    this.state = "initializing";
    const capabilities = {
      experimentalApi: true,
      requestAttestation: false,
      ...(this.options.initializeCapabilities ?? {}),
    };
    await this.rpc.request("initialize", {
      clientInfo: {
        name: this.options.clientName,
        title: this.options.clientTitle ?? null,
        version: this.options.clientVersion,
      },
      capabilities,
    });
    this.rpc.notify("initialized");
    this.started = true;
    this.state = "idle";
    await this.refreshAvailableModels();
    this.emit({ type: "connection.opened", payload: {} });
  }

  async startThread(params: JsonObject = {}): Promise<JsonValue> {
    this.ensureStarted();
    this.ensureSessionChangeAllowed();
    const requestParams: JsonObject = { ...params };
    if (requestParams.cwd === undefined && this.options.defaultCwd) {
      requestParams.cwd = this.options.defaultCwd;
    }
    const result = await this.rpc.request("thread/start", requestParams);
    this.historyComplete = true;
    this.commitThreadResult(result, true);
    await this.publishHistorySnapshot();
    return result;
  }

  async newSession(params: JsonObject = {}): Promise<JsonValue> {
    return this.startThread(params);
  }

  async listSessions(params: JsonObject = {}): Promise<JsonValue> {
    this.ensureStarted();
    const result = await this.rpc.request("thread/list", normalizeThreadListParams(params));
    const threads = isRecord(result) && Array.isArray(result.data)
      ? result.data.filter(isRecord).map((thread) => asJsonObject(thread))
      : [];
    const sessions = threads.map((thread) => {
      const id = typeof thread.id === "string" ? thread.id : "";
      const preview = typeof thread.preview === "string" ? thread.preview : "";
      const name = typeof thread.name === "string" ? thread.name : "";
      const cwd = typeof thread.cwd === "string" ? redactText(thread.cwd) : undefined;
      const updatedAt = finiteNumber(thread.updatedAt) ?? finiteNumber(thread.createdAt);
      return {
        threadId: id,
        title: sessionTitle(name || preview, id),
        updatedAtMs: updatedAt === undefined ? null : Math.round(updatedAt * 1_000),
        ...(cwd ? { cwd } : {}),
        active: id === this.threadId,
        available: Boolean(id),
        ...(thread.status !== undefined ? { status: redactJson(thread.status) } : {}),
        ...(thread.source !== undefined ? { source: redactJson(thread.source) } : {}),
      };
    }).filter((session) => session.threadId);
    return asJsonValue({
      sessions,
      activeThreadId: this.threadId,
      nextCursor: isRecord(result) && typeof result.nextCursor === "string" ? result.nextCursor : null,
      backwardsCursor: isRecord(result) && typeof result.backwardsCursor === "string" ? result.backwardsCursor : null,
    });
  }

  async selectSession(params: JsonObject): Promise<JsonValue> {
    this.ensureStarted();
    const target = (this.stringParam(params, "threadId") ?? this.stringParam(params, "conversationId"))?.trim();
    if (!target) throw new Error("session/select requires threadId");
    this.ensureSessionChangeAllowed();
    if (this.sessionSwitching) throw new Error("a session switch is already in progress");

    const previousThreadId = this.threadId;
    const previousState = this.state;
    this.sessionSwitching = true;
    this.state = "syncing";
    this.historyComplete = false;
    this.emit({
      type: "session.switching",
      threadId: target,
      payload: { previousThreadId, targetThreadId: target },
    });
    try {
      const resumeResult = await this.resumeThreadForSelection(target);
      const hydrated = await this.ensureThreadHistory(resumeResult);
      this.commitThreadResult(resumeResult, true, hydrated);
      const switched = previousThreadId !== target;
      this.emit({
        type: "session.selected",
        threadId: target,
        payload: { previousThreadId, threadId: target, switched, available: true },
      });
      await this.publishHistorySnapshot();
      return asJsonValue({
        threadId: target,
        previousThreadId,
        switched,
        available: true,
        result: redactJson(resumeResult),
      });
    } catch (error) {
      this.state = previousState;
      this.historyComplete = true;
      throw error;
    } finally {
      this.sessionSwitching = false;
    }
  }

  async updateThreadSettings(params: JsonObject): Promise<JsonValue> {
    this.ensureStarted();
    const threadId = this.stringParam(params, "threadId") ?? this.threadId;
    if (!threadId) throw new Error("thread/settings/update requires threadId");
    if (threadId !== this.threadId) throw new Error("thread/settings/update can only target the selected thread");
    const settings = normalizeThreadSettings(params);
    const result = await this.rpc.request("thread/settings/update", { threadId, ...settings.wire });
    this.mergeThreadSettings(settings.display);
    await this.publishAuthoritativeSnapshot();
    return result;
  }

  async startTurn(params: JsonObject): Promise<JsonValue> {
    this.ensureStarted();
    const requestParams = this.normalizeTurnParams(params);
    const result = await this.rpc.request("turn/start", requestParams);
    const turn = isRecord(result) && isRecord(result.turn) ? result.turn : undefined;
    const nextTurnId = turn && typeof turn.id === "string"
      ? turn.id
      : isRecord(result) && typeof result.turnId === "string"
        ? result.turnId
        : undefined;
    if (nextTurnId) this.turnId = nextTurnId;
    if (typeof requestParams.threadId === "string") this.threadId = requestParams.threadId;
    this.state = "active";
    return result;
  }

  async steerTurn(params: JsonObject): Promise<JsonValue> {
    this.ensureStarted();
    const requestParams = this.normalizeTurnParams(params, true);
    const result = await this.rpc.request("turn/steer", requestParams);
    const nextTurnId = isRecord(result) && typeof result.turnId === "string"
      ? result.turnId
      : isRecord(result) && isRecord(result.turn) && typeof result.turn.id === "string"
        ? result.turn.id
        : undefined;
    if (nextTurnId) this.turnId = nextTurnId;
    if (typeof requestParams.threadId === "string") this.threadId = requestParams.threadId;
    this.state = "active";
    return result;
  }

  async interruptTurn(params: JsonObject): Promise<JsonValue> {
    this.ensureStarted();
    const threadId = this.stringParam(params, "threadId") ?? this.threadId;
    const turnId = this.stringParam(params, "turnId") ?? this.turnId;
    if (!threadId || !turnId) throw new Error("turn/interrupt requires threadId and turnId");
    const result = await this.rpc.request("turn/interrupt", { threadId, turnId });
    this.state = "idle";
    // Do this eagerly rather than waiting for the asynchronous
    // `turn/completed` notification. A caller may submit the next turn as
    // soon as the interrupt response resolves.
    this.turnId = null;
    return result;
  }

  async sendInput(text: string, params: JsonObject = {}): Promise<JsonValue> {
    const body: JsonObject = { ...params, text };
    if (this.turnId) return this.steerTurn({ ...body, expectedTurnId: this.turnId });
    return this.startTurn(body);
  }

  async cancel(taskId?: string, params: JsonObject = {}): Promise<JsonValue> {
    return this.interruptTurn({ ...params, ...(taskId ? { turnId: taskId } : {}) });
  }

  async respondApproval(
    requestId: JsonRpcId,
    decision: "allow" | "deny" | "cancel",
    reason?: string,
    response?: JsonValue,
  ): Promise<JsonValue> {
    this.ensureStarted();
    const key = jsonRpcIdKey(requestId);
    const pending = this.pending.get(key);
    if (!pending) throw new Error(`unknown or already resolved approval request: ${key}`);
    const rawResponse = response === undefined
      ? this.defaultApprovalResponse(pending.request.method, pending.request.params, decision, reason)
      : asJsonValue(response);
    validateAdapterResponse(pending.request.method, rawResponse, decision);
    const result = normalizeServerResponse(pending.request.method, rawResponse);
    if (pending.timer) clearTimeout(pending.timer);
    this.pending.delete(key);
    try {
      this.rpc.respond(pending.request.id, result);
    } catch (error) {
      // Do not leave a request retryable forever when the child exits between
      // the liveness check and the JSON-RPC write.
      this.emit({
        type: pending.approval ? "approval.expired" : "input.expired",
        threadId: pending.approval?.threadId,
        turnId: pending.approval?.turnId,
        requestId,
        payload: { requestId: asJsonValue(requestId), reason: "app-server unavailable" },
      });
      throw error;
    }
    this.emit({
      type: pending.approval ? "approval.resolved" : "input.resolved",
      threadId: pending.approval?.threadId,
      turnId: pending.approval?.turnId,
      requestId,
      payload: {
        requestId: asJsonValue(requestId),
        decision,
        ...(reason ? { reason } : {}),
      },
    });
    return result;
  }

  async denyPending(reason = "relay disconnected"): Promise<void> {
    const pendingIds = [...this.pending.values()].map((entry) => entry.request.id);
    for (const requestId of pendingIds) {
      try {
        await this.respondApproval(requestId, "deny", reason);
      } catch {
        // The app-server may have resolved or exited between the snapshot and
        // this fail-closed cleanup pass.
      }
    }
  }

  async snapshot(): Promise<SessionSnapshot> {
    const status = this.statusSnapshot();
    return {
      threadId: this.threadId,
      turnId: this.turnId,
      state: this.state,
      pendingApprovals: [...this.pending.values()]
        .map((entry) => entry.approval)
        .filter((approval): approval is PendingApproval => Boolean(approval)),
      pendingRequests: [...this.pending.values()].map((entry) => ({
        requestId: entry.request.id,
        method: entry.request.method,
        params: redactJson(asJsonObject(entry.request.params)),
        ...(entry.approval?.commandHash ? { commandHash: entry.approval.commandHash } : {}),
        ...(entry.approval?.risk ? { risk: entry.approval.risk } : {}),
        ...(entry.approval?.summary ? { summary: entry.approval.summary } : {}),
        ...(entry.approval?.createdAt ? { createdAt: entry.approval.createdAt } : {}),
        ...(entry.approval?.expiresAt ? { expiresAt: entry.approval.expiresAt } : {}),
      })),
      outputTail: this.outputTail,
      messages: this.messages.map((message) => asJsonValue(message)),
      status,
      activity: status.activity,
      turnStatus: status.turnStatus,
      activeFlags: [...status.activeFlags],
      startedAtMs: status.startedAtMs,
      durationMs: status.durationMs,
      elapsedMs: status.elapsedMs,
      metadata: {
        adapter: "codex-app-server",
        mode: "async",
        started: this.started,
        historyComplete: this.historyComplete,
        ...this.sessionMetadata,
        availableModels: this.availableModels.map((model) => asJsonValue(model)),
        models: this.availableModels.map((model) => asJsonValue(model)),
      },
    };
  }

  onEvent(listener: (event: AgentEvent) => void): Disposable {
    this.listeners.add(listener);
    return { dispose: () => this.listeners.delete(listener) };
  }

  async dispose(): Promise<void> {
    for (const [key, entry] of this.pending) {
      if (entry.timer) clearTimeout(entry.timer);
      // A disconnected host must never leave a command approval hanging.
      try {
        this.rpc.respond(entry.request.id, normalizeServerResponse(
          entry.request.method,
          this.defaultApprovalResponse(entry.request.method, entry.request.params, "deny", "bridge stopped"),
        ));
      } catch {
        // The child may already have exited.
      }
      this.pending.delete(key);
    }
    for (const disposable of this.rpcDisposables) disposable.dispose();
    this.rpc.close();
    this.started = false;
    this.state = "disconnected";
    this.threadId = null;
    this.turnId = null;
    this.messages = [];
    this.outputTail = "";
    this.sessionMetadata = {};
    this.historyComplete = true;
    this.sessionSwitching = false;
  }

  private ensureStarted(): void {
    if (!this.started || !this.rpc.running) throw new Error("Codex app-server is not started");
  }

  private ensureSessionChangeAllowed(): void {
    if (this.turnId || this.pending.size) {
      throw new Error("cannot change sessions while a turn or approval is active");
    }
  }

  private async refreshAvailableModels(): Promise<void> {
    try {
      const models: JsonValue[] = [];
      let cursor: string | null = null;
      for (let page = 0; page < 10; page += 1) {
        const result = await this.rpc.request("model/list", {
          limit: 100,
          includeHidden: false,
          ...(cursor ? { cursor } : {}),
        });
        if (!isRecord(result)) break;
        if (Array.isArray(result.data)) {
          for (const model of result.data) {
            if (isRecord(model) && typeof model.model === "string") models.push(redactJson(model));
          }
        }
        cursor = typeof result.nextCursor === "string" && result.nextCursor ? result.nextCursor : null;
        if (!cursor) break;
      }
      this.availableModels = models;
    } catch (error) {
      // Older app-server builds may not expose the model catalog. Thread and
      // turn control should remain usable with a manually supplied model.
      this.options.logger?.debug?.("Unable to load app-server model catalog", error);
    }
  }

  private async resumeThreadForSelection(threadId: string): Promise<JsonValue> {
    try {
      // Paginated history is the stable protocol for newer app-server builds.
      // Request the first page in chronological order so the renderer can use
      // one consistent ordering while older pages are appended.
      return await this.rpc.request("thread/resume", {
        threadId,
        excludeTurns: true,
        initialTurnsPage: {
          limit: HISTORY_PAGE_SIZE,
          sortDirection: "asc",
          itemsView: "full",
        },
      });
    } catch (error) {
      if (isPaginationUnsupportedError(error)) {
        // Older app-server versions reject the pagination fields. Retry with
        // the legacy full-history shape before giving up.
        try {
          return await this.rpc.request("thread/resume", { threadId, excludeTurns: false });
        } catch (legacyError) {
          if (!isActiveWriterError(legacyError)) throw legacyError;
          return this.readThreadMetadata(threadId, legacyError);
        }
      }
      if (isActiveWriterError(error)) {
        // A thread currently owned by another app-server cannot be resumed by
        // this process, but its metadata and paginated history are still
        // readable. Keep the conversation view available and report the
        // writer limitation through metadata rather than showing an empty
        // session after a successful list click.
        return this.readThreadMetadata(threadId, error);
      }
      throw error;
    }
  }

  private async readThreadMetadata(threadId: string, originalError: unknown): Promise<JsonValue> {
    try {
      return await this.rpc.request("thread/read", { threadId, includeTurns: false });
    } catch (error) {
      this.options.logger?.debug?.("Unable to read a thread after resume failed", error);
      throw originalError instanceof Error ? originalError : error;
    }
  }

  private async ensureThreadHistory(result: JsonValue): Promise<JsonObject | undefined> {
    const response = isRecord(result) ? result : {};
    const thread = extractThread(result);
    if (!thread) throw new Error("thread/resume returned no thread");
    const turns = Array.isArray(thread.turns) ? thread.turns : undefined;
    const threadId = typeof thread.id === "string" ? thread.id : undefined;
    if (!threadId) throw new Error("thread/resume returned a thread without id");

    const paginated = thread.historyMode === "paginated" || isRecord(response.initialTurnsPage);
    const hasHistoryEvidence = Boolean(
      (typeof thread.preview === "string" && thread.preview.trim())
      || ((finiteNumber(thread.updatedAt) ?? 0) > (finiteNumber(thread.createdAt) ?? 0)),
    );
    if (!paginated && turns && (turns.length > 0 || !hasHistoryEvidence)) {
      this.historyComplete = true;
      return thread;
    }

    if (paginated) {
      try {
        const initialPage = isRecord(response.initialTurnsPage)
          ? response.initialTurnsPage
          : undefined;
        const hydratedTurns = await this.loadPaginatedTurns(threadId, initialPage);
        return { ...thread, turns: hydratedTurns };
      } catch (error) {
        // Some transitional server builds advertise paginated threads but do
        // not implement one of the page methods. Fall back to the legacy read
        // endpoint so the session remains usable instead of appearing blank.
        this.options.logger?.debug?.("Paginated thread hydration failed; trying thread/read", error);
        try {
          const readResult = await this.rpc.request("thread/read", { threadId, includeTurns: true });
          const hydrated = extractThread(readResult);
          if (hydrated) {
            this.historyComplete = true;
            return hydrated;
          }
        } catch (readError) {
          this.options.logger?.debug?.("Legacy thread/read fallback failed", readError);
        }
        this.historyComplete = false;
        return { ...thread, turns: turns ?? [] };
      }
    }

    if (turns && turns.length > 0) {
      this.historyComplete = true;
      return thread;
    }

    const readResult = await this.rpc.request("thread/read", { threadId, includeTurns: true });
    const hydrated = extractThread(readResult);
    if (!hydrated) throw new Error("thread/read returned no thread");
    this.historyComplete = true;
    return hydrated;
  }

  private async loadPaginatedTurns(threadId: string, initialPage?: JsonObject): Promise<JsonObject[]> {
    const byId = new Map<string, JsonObject>();
    let page: JsonObject | undefined = initialPage;
    let cursor: string | null = null;
    let complete = true;

    for (let index = 0; index < MAX_HISTORY_TURN_PAGES; index += 1) {
      if (!page) {
        const response = await this.rpc.request("thread/turns/list", {
          threadId,
          limit: HISTORY_PAGE_SIZE,
          sortDirection: "asc",
          itemsView: "full",
          ...(cursor ? { cursor } : {}),
        });
        page = isRecord(response) ? response : {};
      }

      const pageData = Array.isArray(page.data) ? page.data : [];
      for (const value of pageData) {
        if (!isRecord(value)) continue;
        const id = typeof value.id === "string" ? value.id : `turn-${byId.size}`;
        byId.set(id, { ...value });
        if (byId.size >= MAX_HISTORY_TURNS) {
          complete = false;
          break;
        }
      }
      if (byId.size >= MAX_HISTORY_TURNS) break;
      const next = typeof page.nextCursor === "string" && page.nextCursor ? page.nextCursor : null;
      page = undefined;
      cursor = next;
      if (!cursor) break;
    }
    if (cursor) complete = false;

    const turns = sortHistoryTurns([...byId.values()]);
    await this.hydrateTurnItems(threadId, turns, (value) => {
      complete = complete && value;
    });
    this.historyComplete = complete;
    return turns;
  }

  private async hydrateTurnItems(
    threadId: string,
    turns: JsonObject[],
    markComplete: (complete: boolean) => void,
  ): Promise<void> {
    for (const turn of turns) {
      if (turn.itemsView === "full" || typeof turn.id !== "string") continue;
      const items: JsonObject[] = Array.isArray(turn.items)
        ? turn.items.filter(isRecord).map((item) => ({ ...(item as JsonObject) }))
        : [];
      const itemIds = new Set(items.map((item) => typeof item.id === "string" ? item.id : ""));
      let cursor: string | null = null;
      let complete = true;
      try {
        for (let pageIndex = 0; pageIndex < MAX_HISTORY_ITEM_PAGES; pageIndex += 1) {
          const response = await this.rpc.request("thread/items/list", {
            threadId,
            turnId: turn.id,
            limit: HISTORY_PAGE_SIZE,
            sortDirection: "asc",
            ...(cursor ? { cursor } : {}),
          });
          const page = isRecord(response) ? response : {};
          for (const entry of Array.isArray(page.data) ? page.data : []) {
            if (!isRecord(entry) || !isRecord(entry.item)) continue;
            const item = { ...entry.item };
            const id = typeof item.id === "string" ? item.id : "";
            if (!id || !itemIds.has(id)) {
              items.push(item);
              if (id) itemIds.add(id);
            }
          }
          const next = typeof page.nextCursor === "string" && page.nextCursor ? page.nextCursor : null;
          cursor = next;
          if (!cursor) break;
          if (pageIndex === MAX_HISTORY_ITEM_PAGES - 1) complete = false;
        }
      } catch (error) {
        complete = false;
        this.options.logger?.debug?.(`Unable to hydrate items for turn ${turn.id}`, error);
      }
      turn.items = items;
      turn.itemsView = "full";
      markComplete(complete);
    }
  }

  private commitThreadResult(result: JsonValue, replaceHistory: boolean, hydratedThread?: JsonObject): void {
    const thread = hydratedThread ?? extractThread(result);
    if (!thread) throw new Error("app-server thread response did not include a thread");
    const nextThreadId = typeof thread.id === "string" ? thread.id : undefined;
    if (!nextThreadId) throw new Error("app-server thread response did not include thread.id");

    this.threadId = nextThreadId;
    if (replaceHistory) {
      this.messages = projectThreadMessages(thread);
      this.outputTail = outputTailFromMessages(this.messages, this.options.maxOutputTailChars);
    }
    const activeTurn = latestActiveTurn(thread);
    this.turnId = activeTurn && typeof activeTurn.id === "string" ? activeTurn.id : null;
    this.state = this.turnId ? "active" : statusToState(thread.status);
    if (this.state === "notLoaded" || this.state === "unknown") this.state = "idle";

    const response = isRecord(result) ? result : {};
    const title = sessionTitle(
      typeof thread.name === "string" ? thread.name : typeof thread.preview === "string" ? thread.preview : "",
      nextThreadId,
    );
    const cwd = typeof response.cwd === "string"
      ? redactText(response.cwd)
      : typeof thread.cwd === "string" ? redactText(thread.cwd) : undefined;
    const model = typeof response.model === "string" ? response.model : undefined;
    const effort = response.reasoningEffort === null || typeof response.reasoningEffort === "string"
      ? response.reasoningEffort
      : undefined;
    const threadSettings: JsonObject = {
      ...(cwd ? { cwd } : {}),
      ...(model ? { model } : {}),
      ...(effort !== undefined ? { effort: asJsonValue(effort) } : {}),
      ...(response.modelProvider !== undefined ? { modelProvider: redactJson(response.modelProvider) } : {}),
      ...(response.serviceTier !== undefined ? { serviceTier: redactJson(response.serviceTier) } : {}),
      ...(response.approvalPolicy !== undefined ? { approvalPolicy: redactJson(response.approvalPolicy) } : {}),
      ...(response.approvalsReviewer !== undefined ? { approvalsReviewer: redactJson(response.approvalsReviewer) } : {}),
      ...(response.sandbox !== undefined ? { sandboxPolicy: redactJson(response.sandbox) } : {}),
    };
    this.sessionMetadata = {
      thread: threadMetadata(thread),
      title,
      ...(cwd ? { cwd } : {}),
      ...(model ? { model, latestModel: model } : {}),
      ...(effort !== undefined ? { effort: asJsonValue(effort), latestReasoningEffort: asJsonValue(effort) } : {}),
      ...(response.modelProvider !== undefined ? { modelProvider: redactJson(response.modelProvider) } : {}),
      ...(response.approvalPolicy !== undefined ? { approvalPolicy: redactJson(response.approvalPolicy) } : {}),
      ...(response.approvalsReviewer !== undefined ? { approvalsReviewer: redactJson(response.approvalsReviewer) } : {}),
      ...(response.sandbox !== undefined ? { sandboxPolicy: redactJson(response.sandbox) } : {}),
      threadSettings,
    };
  }

  private mergeThreadSettings(settings: JsonObject): void {
    const current = isRecord(this.sessionMetadata.threadSettings)
      ? this.sessionMetadata.threadSettings
      : {};
    const next = { ...current, ...redactJson(settings) as JsonObject };
    this.sessionMetadata.threadSettings = next;
    for (const key of ["model", "modelProvider", "serviceTier", "approvalPolicy", "approvalsReviewer", "sandboxPolicy", "permissions", "cwd"] as const) {
      if (settings[key] !== undefined) this.sessionMetadata[key] = redactJson(settings[key]);
    }
    if (settings.model !== undefined) this.sessionMetadata.latestModel = redactJson(settings.model);
    if (settings.effort !== undefined) {
      this.sessionMetadata.effort = redactJson(settings.effort);
      this.sessionMetadata.latestReasoningEffort = redactJson(settings.effort);
    }
  }

  private async publishHistorySnapshot(): Promise<void> {
    const snapshot = await this.snapshot();
    this.emit({
      type: "output.snapshot",
      threadId: this.threadId ?? undefined,
      turnId: this.turnId ?? undefined,
      payload: {
        stream: "codex",
        text: this.outputTail,
        messages: this.messages.map((message) => asJsonValue(message)),
        structureChanged: true,
        historyComplete: this.historyComplete,
        encoding: "utf8",
        metadata: snapshot.metadata ?? {},
        status: snapshot.status ? asJsonValue(snapshot.status) : null,
      },
    });
    await this.publishAuthoritativeSnapshot(snapshot);
  }

  private async publishAuthoritativeSnapshot(existingSnapshot?: SessionSnapshot): Promise<void> {
    const snapshot = existingSnapshot ?? await this.snapshot();
    this.emit({
      type: "session.snapshot",
      threadId: snapshot.threadId ?? undefined,
      turnId: snapshot.turnId ?? undefined,
      payload: asJsonObject(snapshot),
      status: snapshot.status,
    });
  }

  private statusSnapshot(): NonNullable<SessionSnapshot["status"]> {
    const currentMessages = this.turnId
      ? this.messages.filter((message) => isRecord(message) && message.turnId === this.turnId)
      : [];
    const startedAtValues = currentMessages
      .map((message) => isRecord(message) ? finiteNumber(message.startedAtMs) : undefined)
      .filter((value): value is number => value !== undefined);
    const durationValues = currentMessages
      .map((message) => isRecord(message) ? finiteNumber(message.durationMs) : undefined)
      .filter((value): value is number => value !== undefined);
    const startedAtMs = startedAtValues.length ? Math.min(...startedAtValues) : null;
    const durationMs = durationValues.length ? Math.max(...durationValues) : null;
    const pendingApprovals = [...this.pending.values()].some((entry) => Boolean(entry.approval));
    const pendingInput = [...this.pending.values()].some((entry) => !entry.approval);
    const latest = currentMessages.length && isRecord(currentMessages[currentMessages.length - 1])
      ? currentMessages[currentMessages.length - 1] as JsonObject
      : undefined;
    let activity = this.turnId ? "thinking" : this.state;
    if (pendingApprovals) activity = "waitingOnApproval";
    else if (pendingInput) activity = "waitingOnUserInput";
    else if (latest?.kind === "edit") activity = "editing";
    else if (latest?.itemType === "commandExecution") activity = "running";
    else if (latest?.kind === "reasoning" || latest?.kind === "plan") activity = "thinking";
    return {
      activity,
      turnStatus: this.turnId ? "inProgress" : this.state === "idle" ? "completed" : this.state,
      activeFlags: [
        ...(pendingApprovals ? ["waitingOnApproval"] : []),
        ...(pendingInput ? ["waitingOnUserInput"] : []),
      ],
      startedAtMs,
      durationMs,
      elapsedMs: this.turnId && startedAtMs !== null ? Math.max(0, Date.now() - startedAtMs) : null,
    };
  }

  private upsertItem(item: JsonObject, turnId?: string, turn?: JsonObject, lifecycle: JsonObject = {}): void {
    const projected = projectThreadItem(item, turnId, turn, lifecycle);
    const itemId = typeof projected.itemId === "string" ? projected.itemId : undefined;
    const index = itemId
      ? this.messages.findIndex((message) => isRecord(message)
        && message.itemId === itemId
        && (turnId === undefined || message.turnId === turnId))
      : -1;
    if (index >= 0) this.messages[index] = projected;
    else this.messages.push(projected);
  }

  private appendItemDelta(params: JsonObject, kind: "assistant" | "reasoning" | "plan" | "output", delta: string): void {
    const itemId = this.extractString(params, "itemId");
    const turnId = this.extractString(params, "turnId");
    if (!itemId) return;
    let index = this.messages.findIndex((message) => isRecord(message)
      && message.itemId === itemId
      && (turnId === undefined || message.turnId === turnId));
    if (index < 0) {
      const placeholder: JsonObject = {
        id: itemId,
        itemId,
        ...(turnId ? { turnId } : {}),
        itemType: kind === "output" ? "commandExecution" : kind === "assistant" ? "agentMessage" : kind,
        role: kind === "assistant" ? "assistant" : kind === "reasoning" ? "reasoning" : "tool",
        kind: kind === "output" ? "tool" : kind,
        text: "",
        status: "inProgress",
      };
      this.messages.push(placeholder);
      index = this.messages.length - 1;
    }
    const current = isRecord(this.messages[index]) ? this.messages[index] as JsonObject : {};
    if (kind === "output") current.output = `${typeof current.output === "string" ? current.output : ""}${redactText(delta)}`;
    else current.text = `${typeof current.text === "string" ? current.text : ""}${redactText(delta)}`;
    this.messages[index] = current;
  }

  private normalizeTurnParams(input: JsonObject, steering = false): JsonObject {
    const params: JsonObject = { ...input };
    const threadId = this.stringParam(params, "threadId") ?? this.threadId;
    if (!threadId) throw new Error(`${steering ? "turn/steer" : "turn/start"} requires threadId (start a thread first)`);
    params.threadId = threadId;

    const suppliedInput = params.input;
    if (typeof suppliedInput === "string") {
      params.input = [this.textInput(suppliedInput)];
    } else if (Array.isArray(suppliedInput)) {
      params.input = suppliedInput.map((item) => (typeof item === "string" ? this.textInput(item) : asJsonValue(item)));
    } else {
      const text = this.stringParam(params, "text") ?? this.stringParam(params, "message") ?? this.stringParam(params, "prompt");
      if (!text) throw new Error("turn request requires input or text");
      params.input = [this.textInput(text)];
    }
    delete params.text;
    delete params.message;
    delete params.prompt;
    if (steering) {
      const expectedTurnId = this.stringParam(params, "expectedTurnId") ?? this.turnId;
      if (!expectedTurnId) throw new Error("turn/steer requires expectedTurnId (no active turn)");
      params.expectedTurnId = expectedTurnId;
    }
    return params;
  }

  private textInput(text: string): JsonObject {
    return { type: "text", text, text_elements: [] };
  }

  private stringParam(params: JsonObject, key: string): string | undefined {
    return typeof params[key] === "string" ? (params[key] as string) : undefined;
  }

  private handleNotification(method: string, rawParams: JsonValue | undefined): void {
    const params = asJsonObject(rawParams);
    const threadId = this.extractString(params, "threadId") ?? this.extractNestedString(params, "thread", "id");
    const turnId = this.extractString(params, "turnId") ?? this.extractNestedString(params, "turn", "id");
    if (threadId && this.threadId && threadId !== this.threadId) {
      this.options.logger?.debug?.(`Ignored late ${method} notification for non-selected thread ${threadId}`);
      return;
    }
    const activeTurnId = this.turnId;
    if (threadId && !this.threadId) this.threadId = threadId;
    // A late completion for an earlier turn must not overwrite a newer turn
    // that was started while the old completion notification was in flight.
    // Token usage is thread telemetry, not a lifecycle transition. It can be
    // delivered after `turn/completed`, so do not resurrect an old turn (or
    // replace a newer active turn) just because the notification carries a
    // turnId.
    if (turnId
      && method !== "thread/tokenUsage/updated"
      && (method !== "turn/completed" || !activeTurnId || activeTurnId === turnId)) {
      this.turnId = turnId;
    }

    let type = "app-server.notification";
    let payload: JsonObject = { method, params: redactJson(params) as JsonObject };
    let outputText: string | undefined;

    switch (method) {
      case "thread/started":
        type = "session.created";
        payload = { thread: redactJson(params.thread ?? params) as JsonValue };
        if (isRecord(params.thread)) {
          try {
            this.commitThreadResult({ thread: params.thread }, true);
          } catch (error) {
            this.options.logger?.debug?.("Unable to hydrate thread/started notification", error);
            this.state = "idle";
          }
        } else {
          this.state = "idle";
        }
        break;
      case "thread/name/updated": {
        const title = this.extractString(params, "threadName")?.trim();
        if (title) this.sessionMetadata.title = redactText(title);
        payload = redactJson(params) as JsonObject;
        break;
      }
      case "thread/settings/updated":
        payload = redactJson(params) as JsonObject;
        if (isRecord(params.threadSettings)) this.mergeThreadSettings(params.threadSettings);
        break;
      case "thread/tokenUsage/updated": {
        const tokenUsage = projectTokenUsage(params.tokenUsage);
        // `redactJson` treats every key containing "token" as secret. Keep
        // its redacted params for diagnostics, then add the numeric usage
        // projection that the browser usage picker and relay snapshot need.
        payload = redactJson(params) as JsonObject;
        if (tokenUsage) {
          this.sessionMetadata.tokenUsage = tokenUsage;
          // The official extension names this field latestTokenUsageInfo;
          // retain the shorter alias for existing relay/browser clients.
          this.sessionMetadata.latestTokenUsageInfo = tokenUsage;
          payload.tokenUsage = tokenUsage;
          payload.latestTokenUsageInfo = tokenUsage;
          // Persist usage through the same authoritative snapshot channel as
          // thread settings so a browser reconnect does not fall back to the
          // previous context-window value.
          void this.publishAuthoritativeSnapshot().catch((error) => {
            this.options.logger?.debug?.("Unable to publish token usage snapshot", error);
          });
        } else {
          // Do not let the redaction sentinel for malformed usage data look
          // like a real update and clear a previously valid browser value.
          delete payload.tokenUsage;
          delete payload.latestTokenUsageInfo;
        }
        break;
      }
      case "thread/status/changed":
        type = "session.state";
        payload = redactJson(params) as JsonObject;
        this.state = statusToState(params.status);
        break;
      case "thread/closed":
      case "thread/deleted":
        type = "session.closed";
        payload = redactJson(params) as JsonObject;
        this.state = "closed";
        this.threadId = null;
        this.turnId = null;
        this.messages = [];
        this.outputTail = "";
        this.sessionMetadata = {};
        this.historyComplete = true;
        break;
      case "turn/started":
        type = "task.started";
        payload = redactJson(params) as JsonObject;
        this.state = "active";
        if (isRecord(params.turn) && Array.isArray(params.turn.items)) {
          const turn = asJsonObject(params.turn);
          for (const item of params.turn.items.filter(isRecord)) this.upsertItem(asJsonObject(item), turnId, turn);
        }
        break;
      case "turn/completed": {
        const status = this.extractNestedString(params, "turn", "status");
        type = status === "interrupted" ? "task.cancelled" : "task.finished";
        payload = redactJson(params) as JsonObject;
        // Do not let a stale completion transition a newer active turn to
        // idle. Notifications are asynchronous and can arrive after the
        // caller has already started the next turn.
        if (!turnId || !activeTurnId || turnId === activeTurnId) {
          this.state = "idle";
          this.turnId = null;
        }
        if (isRecord(params.turn) && Array.isArray(params.turn.items)) {
          const turn = asJsonObject(params.turn);
          for (const item of params.turn.items.filter(isRecord)) this.upsertItem(asJsonObject(item), turnId, turn);
        }
        break;
      }
      case "item/agentMessage/delta":
        type = "output.chunk";
        outputText = this.extractString(params, "delta");
        if (outputText) this.appendItemDelta(params, "assistant", outputText);
        payload = { stream: "codex", text: redactText(outputText ?? ""), encoding: "utf8" };
        break;
      case "item/plan/delta":
        type = "output.chunk";
        outputText = this.extractString(params, "delta") ?? this.extractString(params, "text");
        if (outputText) this.appendItemDelta(params, "plan", outputText);
        payload = { stream: "reasoning", text: redactText(outputText ?? ""), encoding: "utf8" };
        break;
      case "item/reasoning/summaryTextDelta":
      case "item/reasoning/textDelta":
        type = "output.chunk";
        outputText = this.extractString(params, "delta") ?? this.extractString(params, "text");
        if (outputText) this.appendItemDelta(params, "reasoning", outputText);
        payload = { stream: "reasoning", text: redactText(outputText ?? ""), encoding: "utf8" };
        break;
      case "command/exec/outputDelta":
      case "process/outputDelta":
      case "item/commandExecution/outputDelta":
        type = "output.chunk";
        outputText = decodeOutput(params);
        if (method === "item/commandExecution/outputDelta" && outputText) {
          this.appendItemDelta(params, "output", outputText);
        }
        payload = {
          stream: outputStream(params),
          text: redactText(outputText),
          encoding: "utf8",
        };
        break;
      case "item/fileChange/outputDelta":
        type = "output.chunk";
        outputText = this.extractString(params, "delta");
        if (outputText) this.appendItemDelta(params, "output", outputText);
        payload = { stream: "codex", text: redactText(outputText ?? ""), encoding: "utf8" };
        break;
      case "item/started":
        type = "item.started";
        payload = redactJson(params) as JsonObject;
        if (isRecord(params.item)) {
          this.upsertItem(params.item, turnId, undefined, {
            ...(finiteNumber(params.startedAtMs) !== undefined ? { startedAtMs: finiteNumber(params.startedAtMs) as number } : {}),
            status: "inProgress",
          });
        }
        break;
      case "item/completed":
        type = "item.completed";
        payload = redactJson(params) as JsonObject;
        if (isRecord(params.item)) {
          this.upsertItem(params.item, turnId, undefined, {
            ...(finiteNumber(params.completedAtMs) !== undefined ? { completedAtMs: finiteNumber(params.completedAtMs) as number } : {}),
          });
        }
        break;
      case "serverRequest/resolved":
        payload = redactJson(params) as JsonObject;
        if (isJsonRpcId(params.requestId)) {
          const pending = this.pending.get(jsonRpcIdKey(params.requestId));
          type = pending?.approval ? "approval.resolved" : pending ? "input.resolved" : "approval.resolved";
          if (pending?.timer) clearTimeout(pending.timer);
          this.pending.delete(jsonRpcIdKey(params.requestId));
        } else {
          type = "approval.resolved";
        }
        break;
      case "error":
        type = "error";
        payload = redactJson(params) as JsonObject;
        break;
      case "warning":
      case "guardianWarning":
        type = "warning";
        payload = redactJson(params) as JsonObject;
        break;
      default:
        break;
    }

    // Events and snapshots must expose the same redacted view. Keeping raw
    // text in outputTail would leak credentials through `snapshot()` even
    // though the corresponding output event was redacted.
    if (outputText) this.appendOutput(outputText);
    this.emit({
      type,
      threadId,
      turnId,
      payload,
      raw: redactJson({ method, params }) as JsonValue,
    });
  }

  private handleServerRequest(request: JsonRpcRequest): void {
    const params = asJsonObject(request.params);
    const requestThreadId = this.extractString(params, "threadId") ?? this.extractString(params, "conversationId");
    if (requestThreadId && this.threadId && requestThreadId !== this.threadId) {
      // A resumed app-server can finish delivering an old request after the
      // browser has selected another thread. Never expose or retain it as an
      // approval for the selected conversation.
      try {
        if (APPROVAL_METHODS.has(request.method) || INPUT_REQUEST_METHODS.has(request.method)) {
          this.rpc.respond(request.id, normalizeServerResponse(
            request.method,
            this.defaultApprovalResponse(request.method, request.params, "deny", "thread is no longer selected"),
          ));
        } else {
          this.rpc.respondError(request.id, -32000, "thread is no longer selected");
        }
      } catch {
        // The child may have exited while the stale request was in flight.
      }
      this.options.logger?.debug?.(`Rejected stale ${request.method} request for non-selected thread ${requestThreadId}`);
      return;
    }
    if (APPROVAL_METHODS.has(request.method)) {
      const approval = this.toPendingApproval(request, params);
      const entry: PendingRequest = { request, approval };
      if (this.options.approvalTimeoutMs > 0) {
        entry.timer = setTimeout(() => this.expireApproval(request.id), this.options.approvalTimeoutMs);
        approval.expiresAt = Date.now() + this.options.approvalTimeoutMs;
      }
      this.pending.set(jsonRpcIdKey(request.id), entry);
      this.emit({
        type: "approval.requested",
        threadId: approval.threadId,
        turnId: approval.turnId,
        requestId: request.id,
        payload: {
          ...approval.payload,
          params: approval.payload,
          requestId: asJsonValue(request.id),
          method: request.method,
          action: approval.action,
          risk: approval.risk,
          summary: approval.summary,
          ...(approval.commandHash ? { commandHash: approval.commandHash } : {}),
          ...(approval.expiresAt ? { expiresAt: approval.expiresAt } : {}),
        },
        raw: redactJson(request) as JsonValue,
      });
      return;
    }

    if (INPUT_REQUEST_METHODS.has(request.method)) {
      const entry: PendingRequest = { request };
      if (this.options.approvalTimeoutMs > 0) {
        entry.timer = setTimeout(() => this.expirePendingRequest(request.id), this.options.approvalTimeoutMs);
      }
      this.pending.set(jsonRpcIdKey(request.id), entry);
      this.emit({
        type: "input.requested",
        threadId: this.extractString(params, "threadId"),
        turnId: this.extractString(params, "turnId"),
        requestId: request.id,
        payload: { requestId: asJsonValue(request.id), method: request.method, params: redactJson(params) as JsonValue },
        raw: redactJson(request) as JsonValue,
      });
      return;
    }

    this.emit({ type: "server.request", requestId: request.id, payload: { method: request.method, params: redactJson(params) as JsonValue }, raw: redactJson(request) as JsonValue });
    void this.resolveServerRequest(request);
  }

  private async resolveServerRequest(request: JsonRpcRequest): Promise<void> {
    try {
      const result = await this.options.onServerRequest?.(request);
      if (result !== undefined) {
        this.rpc.respond(request.id, result);
      } else if (this.options.autoRejectUnsupportedRequests !== false) {
        this.rpc.respondError(request.id, -32601, `Unsupported app-server request: ${request.method}`);
      }
    } catch (error) {
      this.rpc.respondError(request.id, -32000, error instanceof Error ? error.message : String(error));
    }
  }

  private toPendingApproval(request: JsonRpcRequest, params: JsonObject): PendingApproval {
    const threadId = this.extractString(params, "threadId") ?? this.extractString(params, "conversationId");
    const turnId = this.extractString(params, "turnId");
    const itemId = this.extractString(params, "itemId") ?? this.extractString(params, "callId");
    const command = this.extractString(params, "command") ?? this.extractCommand(params);
    const reason = this.extractString(params, "reason");
    const action = approvalAction(request.method);
    const risk = approvalRisk(request.method, command, params.commandActions);
    const summary = reason || command || `${action} requested by Codex`;
    return {
      requestId: request.id,
      method: request.method,
      threadId,
      turnId,
      itemId,
      action,
      risk,
      summary: redactText(summary),
      commandHash: hashJson(params),
      createdAt: Date.now(),
      payload: redactJson(params) as JsonObject,
    };
  }

  private async expireApproval(requestId: JsonRpcId): Promise<void> {
    return this.expirePendingRequest(requestId);
  }

  private async expirePendingRequest(requestId: JsonRpcId): Promise<void> {
    const key = jsonRpcIdKey(requestId);
    const pending = this.pending.get(key);
    if (!pending) return;
    this.pending.delete(key);
    try {
      this.rpc.respond(requestId, normalizeServerResponse(
        pending.request.method,
        this.expiredApprovalResponse(pending.request.method, pending.request.params),
      ));
    } catch {
      // The app-server may have exited while the timer was pending.
    }
    this.emit({
      type: pending.approval ? "approval.expired" : "input.expired",
      threadId: pending.approval?.threadId,
      turnId: pending.approval?.turnId,
      requestId,
      payload: { requestId: asJsonValue(requestId), reason: "approval expired" },
    });
  }

  private dropPendingRequests(reason: string): void {
    const pendingEntries = [...this.pending.values()];
    this.pending.clear();
    for (const pending of pendingEntries) {
      if (pending.timer) clearTimeout(pending.timer);
      this.emit({
        type: pending.approval ? "approval.expired" : "input.expired",
        threadId: pending.approval?.threadId,
        turnId: pending.approval?.turnId,
        requestId: pending.request.id,
        payload: { requestId: asJsonValue(pending.request.id), reason },
      });
    }
  }

  private defaultApprovalResponse(method: string, rawParams: JsonValue | undefined, decision: "allow" | "deny" | "cancel", reason?: string): JsonValue {
    const params = asJsonObject(rawParams);
    if (method === "item/permissions/requestApproval") {
      return {
        permissions: decision === "allow" ? (params.permissions ?? {}) : {},
        scope: "turn",
      };
    }
    if (method === "item/tool/requestUserInput") {
      return { answers: {} };
    }
    if (method === "mcpServer/elicitation/request") {
      return { action: decision === "allow" ? "accept" : decision === "cancel" ? "cancel" : "decline", content: null, _meta: null };
    }
    if (method === "applyPatchApproval" || method === "execCommandApproval") {
      if (decision === "allow") return { decision: "approved" };
      if (decision === "cancel") return { decision: "abort" };
      return { decision: { denied: { rejection: reason || "Denied remotely" } } };
    }
    return { decision: decision === "allow" ? "accept" : decision === "cancel" ? "cancel" : "decline" };
  }

  private expiredApprovalResponse(method: string, rawParams: JsonValue | undefined): JsonValue {
    if (method === "applyPatchApproval" || method === "execCommandApproval") {
      // Preserve the legacy app-server wire decision for an actual timeout;
      // `denied` is reserved for an explicit policy rejection.
      return { decision: "timed_out" };
    }
    return this.defaultApprovalResponse(method, rawParams, "deny", "approval expired");
  }

  private extractString(params: JsonObject, key: string): string | undefined {
    return typeof params[key] === "string" ? (params[key] as string) : undefined;
  }

  private extractNestedString(params: JsonObject, parent: string, key: string): string | undefined {
    const nested = params[parent];
    return isRecord(nested) && typeof nested[key] === "string" ? (nested[key] as string) : undefined;
  }

  private extractCommand(params: JsonObject): string | undefined {
    const command = params.command;
    if (Array.isArray(command)) return command.filter((item): item is string => typeof item === "string").join(" ");
    // Newer command-approval requests may leave `command` null while
    // providing parsed actions. Include every action command in the risk
    // input so a dangerous subcommand cannot be hidden behind command:null.
    if (Array.isArray(params.commandActions)) {
      const commands = params.commandActions
        .map((action) => isRecord(action) && typeof action.command === "string" ? action.command : undefined)
        .filter((item): item is string => Boolean(item));
      if (commands.length) return commands.join(" && ");
    }
    return undefined;
  }

  private appendOutput(text: string): void {
    // Keep this invariant at the storage boundary. New notification handlers
    // can append raw text later without creating a snapshot-only secret leak.
    const safeText = redactText(text);
    this.outputTail = `${this.outputTail}${safeText}`;
    if (this.outputTail.length > this.options.maxOutputTailChars) {
      this.outputTail = this.outputTail.slice(-this.options.maxOutputTailChars);
    }
  }

  private emit(event: AgentEvent): void {
    for (const listener of this.listeners) {
      try {
        listener(event);
      } catch (error) {
        this.options.logger?.warn?.("Agent event listener failed", error);
      }
    }
  }
}

const THREAD_SORT_KEYS = new Set(["created_at", "updated_at", "recency_at", "section_position"]);
const THREAD_SOURCE_KINDS = new Set([
  "cli", "vscode", "exec", "appServer", "subAgent", "subAgentReview",
  "subAgentCompact", "subAgentThreadSpawn", "subAgentOther", "unknown",
]);

function normalizeThreadListParams(params: JsonObject): JsonObject {
  const result: JsonObject = {};
  if (params.cursor === null || typeof params.cursor === "string") result.cursor = params.cursor;
  const limit = finiteNumber(params.limit);
  result.limit = Math.max(1, Math.min(100, Number.isInteger(limit) ? limit as number : 50));
  const sortKey = typeof params.sortKey === "string" ? params.sortKey : "updated_at";
  result.sortKey = THREAD_SORT_KEYS.has(sortKey) ? sortKey : "updated_at";
  result.sortDirection = params.sortDirection === "asc" ? "asc" : "desc";
  if (typeof params.archived === "boolean") result.archived = params.archived;
  if (params.sectionId === null || typeof params.sectionId === "string") result.sectionId = params.sectionId;
  if (typeof params.useStateDbOnly === "boolean") result.useStateDbOnly = params.useStateDbOnly;
  const searchTerm = typeof params.searchTerm === "string"
    ? params.searchTerm
    : typeof params.query === "string" ? params.query : undefined;
  if (searchTerm?.trim()) result.searchTerm = searchTerm.trim();
  if (typeof params.cwd === "string") result.cwd = params.cwd;
  else if (Array.isArray(params.cwd) && params.cwd.every((value) => typeof value === "string")) {
    result.cwd = asJsonValue(params.cwd);
  }
  if (Array.isArray(params.modelProviders) && params.modelProviders.every((value) => typeof value === "string")) {
    result.modelProviders = asJsonValue(params.modelProviders);
  }
  if (Array.isArray(params.sourceKinds)) {
    const sourceKinds = params.sourceKinds.filter((value): value is string => typeof value === "string" && THREAD_SOURCE_KINDS.has(value));
    if (sourceKinds.length) result.sourceKinds = sourceKinds;
  }
  return result;
}

function normalizeThreadSettings(params: JsonObject): { wire: JsonObject; display: JsonObject } {
  const source = isRecord(params.threadSettings) ? params.threadSettings : params;
  const wire: JsonObject = {};
  const display: JsonObject = {};
  for (const key of ["model", "cwd", "effort", "serviceTier", "summary", "personality"] as const) {
    if (!Object.prototype.hasOwnProperty.call(source, key)) continue;
    const value = source[key];
    if (value !== null && (typeof value !== "string" || !value.trim())) {
      throw new Error(`thread settings ${key} must be a non-empty string or null`);
    }
    wire[key] = typeof value === "string" ? value.trim() : null;
    display[key] = wire[key];
  }
  for (const key of ["collaborationMode", "multiAgentMode"] as const) {
    if (!Object.prototype.hasOwnProperty.call(source, key)) continue;
    const value = source[key];
    if (value !== null && typeof value !== "string" && !isRecord(value)) {
      throw new Error(`thread settings ${key} must be a string, object, or null`);
    }
    wire[key] = asJsonValue(value);
    display[key] = wire[key];
  }
  for (const key of ["approvalPolicy", "approvalsReviewer"] as const) {
    if (!Object.prototype.hasOwnProperty.call(source, key)) continue;
    const value = source[key];
    if (value !== null && typeof value !== "string") {
      throw new Error(`thread settings ${key} must be a string or null`);
    }
    wire[key] = asJsonValue(value);
    display[key] = wire[key];
  }

  const hasPermissions = Object.prototype.hasOwnProperty.call(source, "permissions");
  const hasSandboxPolicy = Object.prototype.hasOwnProperty.call(source, "sandboxPolicy");
  if (hasPermissions) {
    const value = source.permissions;
    if (value !== null && (typeof value !== "string" || !value.trim())) {
      throw new Error("thread settings permissions must be a non-empty string or null");
    }
    wire.permissions = typeof value === "string" ? value.trim() : null;
    display.permissions = wire.permissions;
  }
  if (hasSandboxPolicy) {
    const value = source.sandboxPolicy;
    if (value !== null && typeof value !== "string" && !isRecord(value)) {
      throw new Error("thread settings sandboxPolicy must be a string, object, or null");
    }
    display.sandboxPolicy = asJsonValue(value);
    if (!hasPermissions) {
      if (typeof value === "string") {
        const permission = LEGACY_SANDBOX_PERMISSIONS[value];
        if (!permission) throw new Error(`unsupported legacy sandbox policy: ${value}`);
        wire.permissions = permission;
        display.permissions = permission;
      } else {
        wire.sandboxPolicy = asJsonValue(value);
      }
    }
  }
  if (!Object.keys(wire).length) throw new Error("thread settings update requires at least one setting");
  return { wire, display };
}

const LEGACY_SANDBOX_PERMISSIONS: Record<string, string> = {
  "read-only": ":read-only",
  "workspace-write": ":workspace",
  "danger-full-access": ":danger-full-access",
};

function isActiveWriterError(error: unknown): boolean {
  const message = error instanceof Error ? error.message : String(error ?? "");
  return /active\s+writer|writer\s+lock|already\s+has\s+an\s+active\s+writer|thread\s+.*(?:locked|lock)|lock\s+.*thread/i.test(message);
}

function isPaginationUnsupportedError(error: unknown): boolean {
  if (isActiveWriterError(error)) return false;
  const code = isRecord(error) && typeof error.code === "number" ? error.code : undefined;
  if (code === -32602 || code === -32601) return true;
  const message = error instanceof Error ? error.message : String(error ?? "");
  return /unknown\s+(?:field|parameter|method)|method\s+.*not\s+found|invalid\s+(?:param(?:eter)?s?|field)|unexpected\s+(?:field|property)|unsupported\s+(?:pagination|initialTurnsPage|excludeTurns|itemsView)/i.test(message);
}

function sortHistoryTurns(turns: JsonObject[]): JsonObject[] {
  return turns
    .map((turn, index) => ({ turn, index }))
    .sort((left, right) => {
      const leftTime = [left.turn.startedAt, left.turn.createdAt, left.turn.completedAt]
        .map(finiteNumber)
        .find((value): value is number => value !== undefined);
      const rightTime = [right.turn.startedAt, right.turn.createdAt, right.turn.completedAt]
        .map(finiteNumber)
        .find((value): value is number => value !== undefined);
      if (leftTime !== undefined && rightTime !== undefined && leftTime !== rightTime) return leftTime - rightTime;
      if (leftTime !== undefined && rightTime === undefined) return -1;
      if (leftTime === undefined && rightTime !== undefined) return 1;
      const leftId = typeof left.turn.id === "string" ? left.turn.id : "";
      const rightId = typeof right.turn.id === "string" ? right.turn.id : "";
      return leftId.localeCompare(rightId) || left.index - right.index;
    })
    .map(({ turn }) => turn);
}

function extractThread(result: JsonValue): JsonObject | undefined {
  if (!isRecord(result)) return undefined;
  if (isRecord(result.thread)) return result.thread;
  return typeof result.id === "string" ? result : undefined;
}

function threadMetadata(thread: JsonObject): JsonObject {
  const result = redactJson(thread);
  if (!isRecord(result)) return {};
  return { ...result, turns: [] };
}

const TOKEN_USAGE_FIELDS = [
  "totalTokens",
  "inputTokens",
  "cachedInputTokens",
  "cacheWriteInputTokens",
  "outputTokens",
  "reasoningOutputTokens",
] as const;

/** Keep only the numeric portion of the official token-usage projection. */
function projectTokenUsage(value: unknown): JsonObject | undefined {
  if (!isRecord(value)) return undefined;
  const source = isRecord(value.info)
    ? value.info
    : isRecord(value.tokenUsage)
      ? value.tokenUsage
      : isRecord(value.token_usage)
        ? value.token_usage
        : value;
  const total = projectTokenUsageBreakdown(
    source.total
      ?? source.total_token_usage
      ?? source.totalTokenUsage,
  );
  const last = projectTokenUsageBreakdown(
    source.last
      ?? source.last_token_usage
      ?? source.lastTokenUsage,
  );
  const modelContextWindow = tokenNumber(
    source.modelContextWindow
      ?? source.model_context_window
      ?? source.contextWindow
      ?? source.context_window,
  );
  if (!total && !last && modelContextWindow === undefined) return undefined;
  return {
    ...(total ? { total } : {}),
    ...(last ? { last } : {}),
    ...(modelContextWindow !== undefined ? { modelContextWindow } : {}),
  };
}

function projectTokenUsageBreakdown(value: unknown): JsonObject | undefined {
  if (!isRecord(value)) return undefined;
  const aliases: Record<(typeof TOKEN_USAGE_FIELDS)[number], string[]> = {
    totalTokens: ["totalTokens", "total_tokens"],
    inputTokens: ["inputTokens", "input_tokens"],
    cachedInputTokens: ["cachedInputTokens", "cached_input_tokens"],
    cacheWriteInputTokens: ["cacheWriteInputTokens", "cache_write_input_tokens"],
    outputTokens: ["outputTokens", "output_tokens"],
    reasoningOutputTokens: ["reasoningOutputTokens", "reasoning_output_tokens"],
  };
  const result: JsonObject = {};
  for (const field of TOKEN_USAGE_FIELDS) {
    for (const alias of aliases[field]) {
      const number = tokenNumber(value[alias]);
      if (number === undefined) continue;
      result[field] = number;
      break;
    }
  }
  return Object.keys(result).length ? result : undefined;
}

function tokenNumber(value: unknown): number | undefined {
  if (typeof value === "number") return Number.isFinite(value) && value >= 0 ? value : undefined;
  if (typeof value !== "string" || !/^\d+(?:\.\d+)?$/.test(value.trim())) return undefined;
  const number = Number(value);
  return Number.isFinite(number) && number >= 0 ? number : undefined;
}

function latestActiveTurn(thread: JsonObject): JsonObject | undefined {
  if (!Array.isArray(thread.turns)) return undefined;
  for (let index = thread.turns.length - 1; index >= 0; index -= 1) {
    const turn = thread.turns[index];
    if (isRecord(turn) && turn.status === "inProgress") return turn;
  }
  return undefined;
}

function projectThreadMessages(thread: JsonObject): JsonValue[] {
  if (!Array.isArray(thread.turns)) return [];
  const messages: JsonValue[] = [];
  for (const turn of thread.turns) {
    if (!isRecord(turn) || !Array.isArray(turn.items)) continue;
    const turnId = typeof turn.id === "string" ? turn.id : undefined;
    for (const item of turn.items) {
      if (isRecord(item)) messages.push(projectThreadItem(item, turnId, turn));
    }
  }
  return messages;
}

function projectThreadItem(
  item: JsonObject,
  turnId?: string,
  turn?: JsonObject,
  lifecycle: JsonObject = {},
): JsonObject {
  const safeItem = redactJson(item);
  const projected: JsonObject = isRecord(safeItem) ? { ...safeItem } : {};
  const itemType = typeof item.type === "string" ? item.type : "unknown";
  const itemId = typeof item.id === "string" ? item.id : undefined;
  const turnStatus = typeof turn?.status === "string" ? turn.status : undefined;
  const startedAtMs = finiteNumber(lifecycle.startedAtMs) ?? secondsToMs(turn?.startedAt);
  const completedAtMs = finiteNumber(lifecycle.completedAtMs) ?? secondsToMs(turn?.completedAt);
  const durationMs = finiteNumber(item.durationMs) ?? finiteNumber(turn?.durationMs);
  const status = typeof lifecycle.status === "string"
    ? lifecycle.status
    : typeof item.status === "string" ? item.status : undefined;

  Object.assign(projected, {
    ...(itemId ? { id: itemId, itemId } : {}),
    ...(turnId ? { turnId } : {}),
    itemType,
    ...(status ? { status } : {}),
    ...(turnStatus ? { turnStatus } : {}),
    ...(startedAtMs !== undefined ? { startedAtMs } : {}),
    ...(completedAtMs !== undefined ? { completedAtMs } : {}),
    ...(durationMs !== undefined ? { durationMs } : {}),
  });

  switch (itemType) {
    case "userMessage":
      projected.role = "user";
      projected.kind = "user";
      projected.text = userInputText(item.content);
      break;
    case "agentMessage":
      projected.role = "assistant";
      projected.kind = "assistant";
      projected.text = typeof item.text === "string" ? redactText(item.text) : "";
      break;
    case "reasoning":
      projected.role = "reasoning";
      projected.kind = "reasoning";
      projected.text = stringArrayText(item.summary) || stringArrayText(item.content);
      break;
    case "plan":
      projected.role = "reasoning";
      projected.kind = "plan";
      projected.text = typeof item.text === "string" ? redactText(item.text) : "";
      break;
    case "commandExecution":
      projected.role = "tool";
      projected.kind = "tool";
      projected.command = typeof item.command === "string" ? redactText(item.command) : "";
      projected.text = typeof item.command === "string" ? redactText(item.command) : "";
      projected.output = typeof item.aggregatedOutput === "string" ? redactText(item.aggregatedOutput) : "";
      projected.label = "Command";
      projected.uiType = "commandExecution";
      break;
    case "fileChange": {
      projected.role = "tool";
      projected.kind = "edit";
      const paths = Array.isArray(item.changes)
        ? item.changes.filter(isRecord).map((change) => {
            const value = asJsonObject(change);
            return typeof value.path === "string" ? redactText(value.path) : "";
          }).filter(Boolean)
        : [];
      projected.text = paths.join("\n");
      projected.label = "File changes";
      projected.uiType = "fileChange";
      break;
    }
    case "collabAgentToolCall":
      projected.role = "tool";
      projected.kind = "tool";
      projected.text = typeof item.prompt === "string" ? redactText(item.prompt) : typeof item.tool === "string" ? item.tool : "";
      projected.action = typeof item.tool === "string" ? item.tool : "";
      projected.uiType = "collabAgentToolCall";
      break;
    case "subAgentActivity":
      projected.role = "tool";
      projected.kind = "tool";
      projected.text = typeof item.agentPath === "string" ? redactText(item.agentPath) : "";
      projected.activityKind = typeof item.kind === "string" ? item.kind : "";
      projected.uiType = "subAgentActivity";
      break;
    case "webSearch":
      projected.role = "tool";
      projected.kind = "tool";
      projected.text = typeof item.query === "string" ? redactText(item.query) : "";
      projected.label = "Web search";
      projected.uiType = "webSearch";
      break;
    case "imageView":
      projected.role = "tool";
      projected.kind = "tool";
      projected.text = typeof item.path === "string" ? redactText(item.path) : "";
      projected.label = "Image";
      break;
    case "contextCompaction":
      projected.role = "tool";
      projected.kind = "tool";
      projected.text = "Context compacted";
      projected.uiType = "contextCompaction";
      break;
    default:
      projected.role = "tool";
      projected.kind = "tool";
      projected.text = genericItemText(item);
      break;
  }
  return projected;
}

function userInputText(value: unknown): string {
  if (!Array.isArray(value)) return "";
  return value.map((input) => {
    if (!isRecord(input)) return "";
    if (input.type === "text" && typeof input.text === "string") return redactText(input.text);
    if (input.type === "skill" && typeof input.name === "string") return `$${redactText(input.name)}`;
    if (input.type === "mention" && typeof input.name === "string") return `@${redactText(input.name)}`;
    if ((input.type === "image" || input.type === "audio") && typeof input.url === "string") return redactText(input.url);
    if ((input.type === "localImage" || input.type === "localAudio") && typeof input.path === "string") return redactText(input.path);
    return "";
  }).filter(Boolean).join("\n");
}

function stringArrayText(value: unknown): string {
  return Array.isArray(value)
    ? value.filter((entry): entry is string => typeof entry === "string").map(redactText).join("\n")
    : "";
}

function genericItemText(item: JsonObject): string {
  for (const key of ["text", "query", "command", "name", "tool"] as const) {
    if (typeof item[key] === "string") return redactText(item[key] as string);
  }
  if (item.output !== undefined) return redactText(stableStringify(redactJson(item.output)));
  if (item.result !== undefined) return redactText(stableStringify(redactJson(item.result)));
  return "";
}

function outputTailFromMessages(messages: JsonValue[], maxChars: number): string {
  const chunks: string[] = [];
  for (const message of messages) {
    if (!isRecord(message)) continue;
    if (typeof message.text === "string" && message.text) chunks.push(message.text);
    if (typeof message.output === "string" && message.output) chunks.push(message.output);
  }
  const output = redactText(chunks.join("\n\n"));
  return output.length > maxChars ? output.slice(-maxChars) : output;
}

function sessionTitle(value: string, threadId: string): string {
  const title = redactText(value).replace(/\s+/g, " ").trim();
  if (title) return title.slice(0, 160);
  return `Session ${threadId.slice(0, 8)}`;
}

function secondsToMs(value: unknown): number | undefined {
  const seconds = finiteNumber(value);
  return seconds === undefined ? undefined : Math.round(seconds * 1_000);
}

function finiteNumber(value: unknown): number | undefined {
  return typeof value === "number" && Number.isFinite(value) ? value : undefined;
}

/** Keep browser/embedding responses aligned with app-server response schemas. */
function normalizeServerResponse(method: string, response: JsonValue): JsonValue {
  if (method === "item/permissions/requestApproval") {
    const source = isRecord(response) ? response : {};
    const requested = isRecord(source.permissions) ? source.permissions : {};
    const permissions: JsonObject = {};
    for (const [key, value] of Object.entries(requested)) {
      // Request profiles use null to mean "not requested"; granted profiles
      // omit those fields instead of sending an invalid explicit null.
      if (value !== null && value !== undefined) permissions[key] = asJsonValue(value);
    }
    const normalized: JsonObject = {
      permissions,
      scope: source.scope === "session" ? "session" : "turn",
    };
    if (typeof source.strictAutoReview === "boolean") normalized.strictAutoReview = source.strictAutoReview;
    return normalized;
  }
  // Tool user-input responses are wrapped in an `answers` object. MCP
  // elicitation has a different schema (`action`, `content`, `_meta`) and
  // must be forwarded unchanged; wrapping it would make app-server reject
  // an otherwise valid approval response.
  if (method === "mcpServer/elicitation/request") return response;
  if (method === "item/tool/requestUserInput") {
    if (isRecord(response) && Object.prototype.hasOwnProperty.call(response, "answers")) return response;
    return { answers: isRecord(response) ? response : {} };
  }
  return response;
}

/** Validate a response immediately before it crosses the app-server boundary. */
function validateAdapterResponse(
  method: string,
  response: JsonValue,
  decision: "allow" | "deny" | "cancel",
): void {
  if (!isRecord(response)) throw new Error("app-server response must be a JSON object");

  if (method === "item/permissions/requestApproval") {
    if (!isRecord(response.permissions)
      || (response.scope !== "turn" && response.scope !== "session")
      || (response.strictAutoReview !== undefined && typeof response.strictAutoReview !== "boolean")) {
      throw new Error("invalid permissions approval response");
    }
    return;
  }

  if (method === "item/tool/requestUserInput") {
    const answers = response.answers;
    if (!isRecord(answers)) throw new Error("invalid tool input response");
    return;
  }

  if (method === "mcpServer/elicitation/request") {
    if (!Object.prototype.hasOwnProperty.call(response, "action")) {
      throw new Error("MCP elicitation response requires action");
    }
    const action = approvalDecisionKindForMethod(response.action, method);
    if (!action || action !== decision) throw new Error("MCP elicitation action conflicts with decision");
    return;
  }

  // Approval callbacks all use a `decision` field. Unknown or mixed tagged
  // objects are rejected by the method-aware classifier before write.
  if (!hasApprovalDecisionField(response) || !Object.prototype.hasOwnProperty.call(response, "decision")) {
    throw new Error("approval response requires decision");
  }
  const responseDecision = approvalDecisionKindForMethod(response.decision, method);
  if (!responseDecision) throw new Error("unsupported approval response decision");
  if (responseDecision !== decision) throw new Error(`approval response implies ${responseDecision}, but decision is ${decision}`);
}

function statusToState(status: unknown): string {
  if (typeof status === "string") return status;
  if (isRecord(status) && typeof status.type === "string") return status.type;
  return "unknown";
}

function approvalAction(method: string): string {
  switch (method) {
    case "item/commandExecution/requestApproval":
    case "execCommandApproval":
      return "command.execution";
    case "item/fileChange/requestApproval":
    case "applyPatchApproval":
      return "file.change";
    case "item/permissions/requestApproval":
      return "permissions.grant";
    default:
      return "approval";
  }
}

function approvalRisk(method: string, command?: string, commandActions?: JsonValue): PendingApproval["risk"] {
  // A permission profile can expand filesystem or network access for the
  // current turn/session, so treat it like an explicit high-impact command.
  if (method.includes("permissions")) return "high";
  if (method.includes("command") || method === "execCommandApproval") {
    const suspicious = /(?:rm\s+-rf|sudo|curl|wget|ssh|password|token|secret)/i;
    if (Array.isArray(commandActions)) {
      // `commandActions` is parsed display data, not a proof of safety. Keep
      // every request carrying it high-risk, scan all extracted commands, and
      // treat unknown/malformed actions as high-risk as well. This prevents a
      // dangerous subcommand from being hidden behind command:null.
      const actions = commandActions;
      const actionCommands = actions
        .map((action) => isRecord(action) && typeof action.command === "string" ? action.command : undefined)
        .filter((item): item is string => Boolean(item));
      const malformed = actions.some((action) => {
        if (!isRecord(action) || typeof action.command !== "string") return true;
        return action.type !== "read"
          && action.type !== "listFiles"
          && action.type !== "search"
          && action.type !== "unknown";
      });
      const combined = [command, ...actionCommands].filter((item): item is string => Boolean(item)).join(" && ");
      if (malformed || !actionCommands.length || suspicious.test(combined) || actions.some((action) => isRecord(action) && action.type === "unknown")) {
        return "high";
      }
      return "high";
    }
    if (command && suspicious.test(command)) return "high";
    // An unparseable command approval is fail-closed. A missing command can
    // otherwise be misclassified as medium and approved by the default host
    // capability policy.
    if (!command) return "high";
    return "medium";
  }
  if (method.includes("fileChange") || method === "applyPatchApproval") return "medium";
  return "unknown";
}

function outputStream(params: JsonObject): string {
  const stream = params.stream;
  if (stream === "stderr" || stream === "stdout" || stream === "codex") return stream;
  return "stdout";
}

function decodeOutput(params: JsonObject): string {
  if (typeof params.delta === "string") return params.delta;
  if (typeof params.deltaBase64 === "string") {
    try {
      return Buffer.from(params.deltaBase64, "base64").toString("utf8");
    } catch {
      return "[invalid base64 output]";
    }
  }
  return "";
}

const SECRET_KEY = /(?:token|secret|password|authorization|api[_-]?key|private[_-]?key|refresh)/i;
const SECRET_VALUE = /(?:Bearer\s+)[A-Za-z0-9._~+\-/]+=*|(?:sk-[A-Za-z0-9_-]{12,}|gh[pousr]_[A-Za-z0-9_]{12,})/g;

function redactText(text: string): string {
  return text
    .replace(SECRET_VALUE, "[REDACTED]")
    .replace(/([?&](?:token|key|secret|password|api[_-]?key)=)[^&\s]+/gi, "$1[REDACTED]")
    .replace(/((?:token|secret|password|api[_-]?key)\s*[:=]\s*)[^\s,;]+/gi, "$1[REDACTED]");
}

function redactJson(value: unknown): JsonValue {
  if (Array.isArray(value)) return value.map((item) => redactJson(item));
  if (isRecord(value)) {
    const result: JsonObject = {};
    for (const [key, child] of Object.entries(value)) {
      result[key] = SECRET_KEY.test(key) ? "[REDACTED]" : redactJson(child);
    }
    return result;
  }
  if (typeof value === "string") return redactText(value);
  return asJsonValue(value);
}

function hashJson(value: JsonValue): string {
  return createHash("sha256").update(stableStringify(value)).digest("hex");
}

function stableStringify(value: JsonValue): string {
  if (Array.isArray(value)) return `[${value.map(stableStringify).join(",")}]`;
  if (value !== null && typeof value === "object") {
    return `{${Object.keys(value).sort().map((key) => `${JSON.stringify(key)}:${stableStringify(value[key] ?? null)}`).join(",")}}`;
  }
  return JSON.stringify(value);
}
