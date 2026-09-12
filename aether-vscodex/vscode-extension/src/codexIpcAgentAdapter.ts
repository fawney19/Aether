import { createHash, randomUUID } from "node:crypto";
import { promises as fs } from "node:fs";
import * as os from "node:os";
import * as path from "node:path";

import {
  CODEX_IPC_METHOD_VERSIONS,
  CodexIpcClient,
  CodexIpcClientOptions,
  ConversationStreamState,
  ConversationStreamEvent,
  IpcBroadcast,
  IpcSubscription,
} from "./codexIpc";
import {
  AgentAdapter,
  AgentEvent,
  AgentStatusSnapshot,
  asJsonObject,
  asJsonValue,
  Disposable,
  isJsonRpcId,
  isRecord,
  JsonObject,
  JsonRpcId,
  JsonValue,
  Logger,
  PendingApproval,
  SessionSnapshot,
  SessionListEntry,
  SessionListResult,
  SubagentSnapshot,
  jsonRpcIdKey,
} from "./protocol";

const APPROVAL_METHODS = new Set([
  "item/commandExecution/requestApproval",
  "item/fileChange/requestApproval",
  "item/permissions/requestApproval",
  "applyPatchApproval",
  "execCommandApproval",
]);
const INPUT_METHODS = new Set([
  "item/tool/requestUserInput",
  "mcpServer/elicitation/request",
]);
const TERMINAL_TURN_STATES = new Set([
  "completed",
  "complete",
  "failed",
  "cancelled",
  "canceled",
  "interrupted",
  "error",
  "done",
]);
const HISTORY_LOAD_RETRY_DELAY_MS = 50;
const VSCODE_SESSION_FOLLOW_RETRY_DELAY_MS = 250;
const VSCODE_SESSION_FOLLOW_MAX_ATTEMPTS = 3;
/** Keep the relay alive while a user opens/selects the first official panel. */
const WAITING_SESSION_DISCOVERY_DELAY_MS = 1_000;
/** A waiting poll is a fallback; route broadcasts handle the common fast path. */
const WAITING_SESSION_DISCOVERY_MAX_CANDIDATES = 8;

export interface CodexIpcAgentAdapterOptions extends Omit<CodexIpcClientOptions, "clientType" | "canHandleRequest"> {
  /** Existing conversation id. Empty/undefined enables local discovery. */
  threadId?: string;
  hostId?: string;
  autoDiscoverThread?: boolean;
  /** Workspace paths used to rank auto-discovered sessions. */
  preferredCwds?: string[];
  /** Ask the owner for older paginated history after the initial snapshot. */
  loadCompleteHistory?: boolean;
  /** Follow route changes in the official VS Code Codex panel. */
  followVscodeSession?: boolean;
  /** Coalesce the official panel's old=false/new=true route broadcasts. */
  vscodeSessionFollowDebounceMs?: number;
  ownerDiscoveryTimeoutMs?: number;
  followTimeoutMs?: number;
  maxOutputTailChars?: number;
  approvalTimeoutMs?: number;
  logger?: Logger;
  /** Invoke the existing official VS Code command for a fresh Codex panel. */
  openNewSession?: () => Promise<JsonValue>;
  /** Inject a client in tests. The adapter owns it unless disabled below. */
  client?: CodexIpcClient;
  disposeClient?: boolean;
}

interface RequestEntry {
  requestId: JsonRpcId;
  method: string;
  params: JsonObject;
  threadId?: string;
  turnId?: string;
  createdAt: number;
  expiresAt?: number;
  approval?: PendingApproval;
}

interface TurnInfo {
  id?: string;
  status: string;
  active: boolean;
  startedAt?: number;
  durationMs?: number | null;
  workedDurationMs?: number | null;
  firstTurnWorkItemStartedAtMs?: number | null;
  finalAssistantStartedAtMs?: number | null;
  completedAtMs?: number | null;
  error?: JsonValue;
  /** The raw turn record, used to classify the currently running work item. */
  raw?: Record<string, unknown>;
}

/**
 * AgentAdapter backed by the private IPC follower protocol used by the
 * official OpenAI Codex VS Code extension. It never starts or kills a codex
 * process: all work is routed to the owner of an already-open conversation.
 */
export class CodexIpcAgentAdapter implements AgentAdapter {
  private readonly options: Required<Pick<
    CodexIpcAgentAdapterOptions,
    "hostId" | "autoDiscoverThread" | "followVscodeSession" | "vscodeSessionFollowDebounceMs" | "ownerDiscoveryTimeoutMs" | "followTimeoutMs" | "maxOutputTailChars" | "approvalTimeoutMs" | "disposeClient"
  >> & CodexIpcAgentAdapterOptions;
  private readonly client: CodexIpcClient;
  private readonly listeners = new Set<(event: AgentEvent) => void>();
  private readonly subscriptions: IpcSubscription[] = [];
  private readonly pending = new Map<string, RequestEntry>();
  private readonly pendingTimers = new Map<string, NodeJS.Timeout>();
  private readonly pendingExpiryAt = new Map<string, number>();
  private readonly optimisticallyResolved = new Set<string>();
  private threadId: string | null = null;
  private ownerClientId: string | null = null;
  private revision: number | null = null;
  private conversationState: JsonObject = {};
  private turnId: string | null = null;
  private state = "disconnected";
  private status: AgentStatusSnapshot = {
    activity: "idle",
    turnStatus: "idle",
    activeFlags: [],
    startedAtMs: null,
    durationMs: null,
    workedDurationMs: null,
    elapsedMs: null,
    firstTurnWorkItemStartedAtMs: null,
    finalAssistantStartedAtMs: null,
  };
  private started = false;
  private renderedOutput = "";
  private renderedOutputLength = 0;
  private renderedOutputWasTruncated = false;
  private renderedMessageShape = "";
  private outputTail = "";
  private outputMessages: RenderedConversationMessage[] = [];
  private subagents: SubagentSnapshot[] = [];
  private renderedSubagentShape = "";
  /** Fingerprint of the display-safe metadata projection last sent to peers. */
  private renderedMetadataShape = "";
  private snapshotSeen = false;
  private historyComplete?: boolean;
  private historyLoadRequested = false;
  private historyLoadAttempts = 0;
  private historyLoadGeneration = 0;
  private historyLoadRetryTimer?: NodeJS.Timeout;
  private disposed = false;
  /** Serialize session navigation so two browser clicks cannot overlap. */
  private sessionSwitching = false;
  /** Invalidates in-flight navigation when dispose/socket close begins. */
  private sessionLifecycleGeneration = 0;
  /** IPC client that owns the official Codex panel route being mirrored. */
  private vscodeRouteClientId: string | null = null;
  /** Untrusted old=false halves; only a matching same-source true can bind. */
  private readonly vscodeRouteCandidates = new Map<string, string>();
  /** Last route that source reported as active, independent of our attachment. */
  private vscodeRouteActiveThreadId: string | null = null;
  /** Official routing emits old=false before new=true, even when the picker stays open. */
  private vscodeRouteAwaitingSelection = false;
  /** Latest route selected in that official panel, coalesced across rapid clicks. */
  private pendingVscodeThreadId: string | null = null;
  private pendingVscodeFollowAttempts = 0;
  private pendingVscodeFollowGeneration = 0;
  private vscodeRouteGeneration = 0;
  private activeVscodeSelection?: { target: string; generation: number };
  private vscodeSessionFollowTimer?: NodeJS.Timeout;
  /** Serialize attachability probes with navigation so probe cleanup cannot unfollow a newly selected thread. */
  private sessionOperationTail: Promise<void> = Promise.resolve();
  /** True while the IPC/relay host is usable but no conversation is attached. */
  private waitingForSession = false;
  private waitingDiscoveryTimer?: NodeJS.Timeout;
  private waitingDiscoveryInFlight?: Promise<void>;
  private waitingAttachPromise?: Promise<boolean>;
  private waitingAttachTarget: string | null = null;
  private queuedWaitingAttachTarget: string | null = null;
  private snapshotWaiter?: { threadId: string; ownerClientId?: string; resolve: () => void; reject: (error: Error) => void; timer: NodeJS.Timeout };
  private revisionWaiter?: { threadId: string; ownerClientId: string; revision: number; resolve: () => void; reject: (error: Error) => void; timer: NodeJS.Timeout };

  constructor(options: CodexIpcAgentAdapterOptions = {}) {
    this.options = {
      ...options,
      hostId: options.hostId ?? "local",
      autoDiscoverThread: options.autoDiscoverThread ?? true,
      followVscodeSession: options.followVscodeSession ?? true,
      vscodeSessionFollowDebounceMs: Math.max(0, options.vscodeSessionFollowDebounceMs ?? 150),
      // Owner discovery for an active local VS Code client normally returns
      // in a few milliseconds. A short default keeps stale rollout files from
      // making bridge startup look hung; callers can raise this explicitly.
      ownerDiscoveryTimeoutMs: options.ownerDiscoveryTimeoutMs ?? 2_500,
      followTimeoutMs: options.followTimeoutMs ?? 8_000,
      maxOutputTailChars: options.maxOutputTailChars ?? 32_000,
      approvalTimeoutMs: options.approvalTimeoutMs ?? 5 * 60_000,
      disposeClient: options.disposeClient ?? true,
    };
    this.client = options.client ?? new CodexIpcClient({
      ...options,
      clientType: "codex-remote-collab-follower",
      canHandleRequest: () => false,
      autoReconnect: false,
    });
    this.subscriptions.push(this.client.onBroadcast((frame) => this.handleBroadcast(frame)));
    this.subscriptions.push(this.client.onStreamEvent((event) => this.handleStreamEvent(event)));
    this.subscriptions.push(this.client.onError((error) => this.options.logger?.debug?.("Codex IPC error", error.message)));
    this.subscriptions.push(this.client.onClose((error) => this.handleClose(error)));
  }

  async start(): Promise<void> {
    if (this.started) return;
    if (this.disposed) throw new Error("Codex IPC follower is disposed");
    this.clearWaitingDiscoveryTimer();
    this.waitingForSession = false;
    this.resetHistoryLoading();
    this.renderedOutput = "";
    this.renderedOutputLength = 0;
    this.renderedOutputWasTruncated = false;
    this.renderedMessageShape = "";
    this.outputTail = "";
    this.outputMessages = [];
    this.subagents = [];
    this.renderedSubagentShape = "";
    this.renderedMetadataShape = "";
    const configured = this.options.threadId?.trim();
    await this.client.connect();
    if (configured) {
      try {
        await this.attachThread(configured);
        return;
      } catch (error) {
        // A remembered thread can belong to another Codex window (or to a
        // previous run). When auto-discovery is enabled, keep startup useful
        // by falling back to the most recent live VS Code owner.
        if (!isMissingSessionOwnerError(error)) throw error;
        if (!this.options.autoDiscoverThread) {
          this.enterWaitingForSession();
          return;
        }
        this.options.logger?.warn?.(`Configured Codex conversation ${configured} could not be followed; trying auto-discovery`, error);
        const fallback = await this.discoverThreadId(new Set([configured]));
        if (fallback) {
          try {
            await this.attachThread(fallback);
            return;
          } catch (fallbackError) {
            if (!isMissingSessionOwnerError(fallbackError)) throw fallbackError;
          }
        }
        this.enterWaitingForSession();
        return;
      }
    }

    const selectedThread = await this.discoverThreadId();
    if (!selectedThread) {
      // A fresh VS Code window can have a live IPC socket before the user has
      // opened a Codex conversation. Keep the follower (and therefore the
      // relay/WebSocket) alive so the first later panel navigation can attach
      // without restarting the bridge.
      this.enterWaitingForSession();
      return;
    }
    await this.attachThread(selectedThread);
  }

  private async attachThread(selectedThread: string, options: { fromWaiting?: boolean } = {}): Promise<void> {
    const fromWaiting = options.fromWaiting === true;
    const lifecycleGeneration = this.sessionLifecycleGeneration;
    const routeGeneration = this.vscodeRouteGeneration;
    const wasStarted = this.started;
    const wasWaiting = this.waitingForSession;
    const previousRouteClientId = this.vscodeRouteClientId;
    const previousRouteActiveThreadId = this.vscodeRouteActiveThreadId;
    const previousRouteAwaitingSelection = this.vscodeRouteAwaitingSelection;
    const owner = await this.client.findThreadOwner(selectedThread, this.options.hostId, this.options.ownerDiscoveryTimeoutMs);
    if (!owner) {
      throw new Error(`找不到会话 ${selectedThread} 的 VS Code Codex owner。请确认该会话已在官方 Codex 面板打开。`);
    }
    this.threadId = selectedThread;
    this.ownerClientId = owner;
    // An owner can be Codex Desktop while the visible VS Code webview follows
    // it, so ownerClientId is not always the route source. Bind the route
    // source on the first observed old=false/new=true navigation pair instead.
    if (!this.vscodeRouteActiveThreadId) this.vscodeRouteActiveThreadId = selectedThread;
    this.state = "syncing";
    if (!wasStarted) this.started = true;
    // A normal startup announces the connection before waiting for the first
    // stream snapshot. A waiting host already announced its IPC connection;
    // emitting a second `connection.opened` would make the browser reset its
    // connection indicator unnecessarily.
    if (!wasStarted) this.emit({ type: "connection.opened", threadId: selectedThread, payload: { mode: "attach", ownerClientId: owner } });
    // Do not expose a provisional thread as interactive while its first
    // authoritative snapshot is still in flight.
    this.waitingForSession = false;
    try {
      const waitForSnapshot = this.waitForSnapshot(selectedThread, this.options.followTimeoutMs, owner);
      void waitForSnapshot.catch(() => undefined);
      await this.client.followConversation(selectedThread, true, {
        hostId: this.options.hostId,
        targetClientIds: [owner],
      });
      await waitForSnapshot;
      this.state = this.deriveSessionState();
      this.waitingForSession = false;
      this.clearWaitingDiscoveryTimer();
      if (this.options.loadCompleteHistory !== false) void this.loadCompleteHistoryIfNeeded();
      this.options.logger?.info?.(`Attached to existing Codex conversation ${selectedThread}`);
    } catch (error) {
      // A failed follow must leave the adapter retryable and must not keep a
      // stale thread/owner that could receive a later remote command.
      this.clearSnapshotWaiter(error instanceof Error ? error : new Error(String(error)));
      try { await this.client.followConversation(selectedThread, false, { hostId: this.options.hostId, targetClientIds: [owner] }); } catch { /* best effort */ }
      const restoreWaiting = (fromWaiting || wasWaiting)
        && !this.disposed
        && lifecycleGeneration === this.sessionLifecycleGeneration;
      // A transient attach attempt made from the waiting state must not tear
      // down the IPC socket or relay. Restore the waiting projection and let
      // the discovery loop try again after the official panel is ready.
      this.started = restoreWaiting ? wasStarted : false;
      this.state = restoreWaiting ? "waiting_for_host" : "disconnected";
      this.waitingForSession = restoreWaiting;
      this.threadId = null;
      this.ownerClientId = null;
      if (!restoreWaiting) {
        this.vscodeRouteClientId = null;
        this.vscodeRouteActiveThreadId = null;
        this.vscodeRouteAwaitingSelection = false;
      } else if (this.vscodeRouteGeneration === routeGeneration) {
        this.vscodeRouteClientId = previousRouteClientId;
        this.vscodeRouteActiveThreadId = previousRouteActiveThreadId;
        this.vscodeRouteAwaitingSelection = previousRouteAwaitingSelection;
      }
      this.vscodeRouteCandidates.clear();
      this.resetConversationProjection();
      throw error;
    }
  }

  /** Attach mode deliberately has no thread creation operation. */
  async startThread(): Promise<JsonValue> {
    throw new Error("attach mode does not create a new thread; open an existing Codex conversation in VS Code");
  }

  /**
   * Open a new conversation through the already-installed VS Code Codex
   * extension. The callback is injected by the extension entrypoint; this
   * follower never starts another Codex process.
   */
  async newSession(): Promise<JsonValue> {
    // Opening the official new-session panel is also the recovery action for
    // `waiting_for_host`; it does not require an existing conversation owner.
    this.ensureStarted();
    if (!this.options.openNewSession) {
      throw new Error("当前 VS Code Codex 扩展不支持从远程打开新会话");
    }
    return asJsonValue(await this.options.openNewSession());
  }

  async startTurn(params: JsonObject): Promise<JsonValue> {
    this.ensureInteractiveReady();
    const input = extractInput(params);
    const request = pickTurnRequest(params);
    const result = await this.client.startTurn(this.threadId as string, input, {
      request,
      context: pickTurnContext(params),
      clientUserMessageId: stringValue(params.clientUserMessageId),
      ownerClientId: this.ownerClientId as string,
      timeoutMs: this.options.followTimeoutMs,
    });
    const unwrapped = unwrapFollowerResult(result);
    const nextTurn = extractTurnId(unwrapped);
    if (nextTurn) {
      this.turnId = nextTurn;
      this.state = "active";
    }
    return asJsonValue(unwrapped);
  }

  async steerTurn(params: JsonObject): Promise<JsonValue> {
    this.ensureInteractiveReady();
    const expected = stringValue(params.expectedTurnId) ?? this.turnId;
    if (!expected) throw new Error("turn/steer requires an active turn");
    const result = await this.client.steerTurn(this.threadId as string, extractInput(params), {
      clientUserMessageId: stringValue(params.clientUserMessageId) ?? randomUUID(),
      serviceTier: params.serviceTier === null || typeof params.serviceTier === "string" ? params.serviceTier : undefined,
      attachments: Array.isArray(params.attachments) ? params.attachments : [],
      additionalContext: isRecord(params.additionalContext) ? asJsonObject(params.additionalContext) : undefined,
      restoreMessage: params.restoreMessage === null || params.restoreMessage !== undefined ? asJsonValue(params.restoreMessage) : undefined,
      ownerClientId: this.ownerClientId as string,
      timeoutMs: this.options.followTimeoutMs,
    });
    this.turnId = expected;
    this.state = "active";
    return asJsonValue(unwrapFollowerResult(result));
  }

  /** Persist model/reasoning settings on the already-open official thread. */
  async updateThreadSettings(params: JsonObject): Promise<JsonValue> {
    this.ensureInteractiveReady();
    const threadSettings = pickThreadSettingsUpdate(params);
    const result = await this.client.updateThreadSettings(
      this.threadId as string,
      threadSettings,
      {
        ownerClientId: this.ownerClientId as string,
        timeoutMs: this.options.followTimeoutMs,
      },
    );
    return asJsonValue(unwrapFollowerResult(result));
  }

  /**
   * Return the local VS Code conversations that this follower can attach to.
   *
   * The official extension obtains this list from its app-server client via
   * `thread/list`. That request is intentionally not exposed by the private
   * IPC router, so the bridge uses local rollout/index metadata only to find
   * candidates. A candidate is returned after live owner discovery and, for a
   * non-active conversation, a matching follower snapshot. Closed, stale, or
   * desktop-owned rollouts are omitted instead of being shown as selectable
   * history that attach mode cannot actually open.
   */
  async listSessions(params: JsonObject = {}): Promise<JsonValue> {
    // Session discovery is useful precisely while no conversation is attached
    // (for example immediately after a fresh VS Code window opens).
    this.ensureStarted();
    const releaseSessionOperation = await this.acquireSessionOperation();
    try {
      return await this.listAttachableSessions(params);
    } finally {
      releaseSessionOperation();
    }
  }

  private async listAttachableSessions(params: JsonObject): Promise<JsonValue> {
    const limitValue = numberValue(params.limit);
    const limit = Math.max(1, Math.min(100, Number.isInteger(limitValue) ? limitValue as number : 50));
    const codexHome = resolveCodexHome(this.options);
    const [candidates, index] = await Promise.all([
      recentVscodeThreadCandidates(path.join(codexHome, "sessions"), this.options.preferredCwds ?? []),
      readSessionIndex(path.join(codexHome, "session_index.jsonl")),
    ]);
    const byId = new Map<string, Candidate>();
    for (const candidate of candidates) {
      const indexed = index.get(candidate.id);
      byId.set(candidate.id, {
        ...candidate,
        ...(indexed?.title && !candidate.title ? { title: indexed.title } : {}),
        ...(indexed?.updatedAtMs !== undefined && indexed.updatedAtMs > (candidate.updatedAtMs ?? 0)
          ? { updatedAtMs: indexed.updatedAtMs } : {}),
        ...(indexed?.cwd && !candidate.cwd ? { cwd: indexed.cwd } : {}),
      });
    }
    // A configured/current thread can be valid even while its rollout has
    // rotated away. Keep it in the picker so the active row is never lost.
    if (this.threadId && !byId.has(this.threadId)) {
      const title = stringValue(this.conversationState.title)
        ?? stringValue(this.conversationState.name)
        ?? stringValue(this.conversationState.threadTitle);
      const cwd = stringValue(this.conversationState.cwd);
      byId.set(this.threadId, {
        id: this.threadId,
        mtime: Date.now(),
        updatedAtMs: Date.now(),
        priority: 0,
        ...(title ? { title } : {}),
        ...(cwd ? { cwd } : {}),
      });
    }

    const compareCandidates = (a: Candidate, b: Candidate) => (b.updatedAtMs ?? b.mtime) - (a.updatedAtMs ?? a.mtime)
      || a.priority - b.priority;
    const ordered = [...byId.values()]
      .sort(compareCandidates)
      .slice(0, limit);
    // `limit` bounds expensive owner/snapshot probes, but a newer stale
    // rollout must never consume the only slot and hide the active attachment.
    const activeCandidate = this.threadId ? byId.get(this.threadId) : undefined;
    if (activeCandidate && !ordered.some((candidate) => candidate.id === activeCandidate.id)) {
      if (ordered.length >= limit) ordered[ordered.length - 1] = activeCandidate;
      else ordered.push(activeCandidate);
      ordered.sort(compareCandidates);
    }
    const sessions: SessionListEntry[] = [];
    // Owner discovery is deliberately bounded/concurrent: stale rollout files
    // are common, and one slow stale check must not block all other rows.
    const ownerTimeout = Math.max(250, Math.min(this.options.ownerDiscoveryTimeoutMs, 750));
    // A discovery response only proves that *some* client knows the thread.
    // Require a short, targeted snapshot probe before advertising a non-active
    // row as selectable; desktop-owned/stale threads can otherwise look live
    // and leave the browser blank after a failed switch.
    const snapshotProbeTimeout = Math.max(250, Math.min(this.options.followTimeoutMs, 750));
    for (let offset = 0; offset < ordered.length; offset += 8) {
      const batch = ordered.slice(offset, offset + 8);
      const checked = await Promise.all(batch.map(async (candidate) => {
        let owner: string | null = candidate.id === this.threadId ? this.ownerClientId : null;
        if (!owner) {
          try {
            owner = await this.client.findThreadOwner(candidate.id, this.options.hostId, ownerTimeout);
          } catch (error) {
            this.options.logger?.debug?.(`Session owner discovery failed for ${candidate.id}`, error);
          }
        }
        let available = false;
        if (owner) {
          available = candidate.id === this.threadId
            || await this.probeSessionSnapshot(candidate.id, owner, snapshotProbeTimeout);
        }
        if (!available) return null;
        const title = sanitizeSessionTitle(candidate.title) ?? `会话 ${candidate.id.slice(0, 8)}`;
        return {
          threadId: candidate.id,
          title,
          updatedAtMs: candidate.updatedAtMs ?? candidate.mtime,
          ...(candidate.cwd ? { cwd: redactText(candidate.cwd) } : {}),
          active: candidate.id === this.threadId,
          available: true,
        } satisfies SessionListEntry;
      }));
      for (const entry of checked) {
        if (entry) sessions.push(entry);
      }
      // Route navigation has priority over populating more picker rows. The
      // probes in this batch have already cleaned up their temporary follows;
      // stop here so selectSession can acquire the shared operation lock
      // instead of waiting behind dozens of stale rollout candidates.
      if (this.sessionSwitching || this.pendingVscodeThreadId || this.activeVscodeSelection) break;
    }
    const result: SessionListResult = {
      sessions,
      activeThreadId: this.threadId,
    };
    return asJsonValue(result);
  }

  /** Attach to another already-open VS Code Codex conversation. */
  async selectSession(params: JsonObject): Promise<JsonValue> {
    this.ensureStarted();
    const origin = stringValue(params.origin) === "vscode" ? "vscode" : "web";
    const expectedRouteGeneration = origin === "vscode"
      ? numberValue(params.vscodeRouteGeneration)
      : undefined;
    const target = (stringValue(params.threadId) ?? stringValue(params.conversationId))?.trim();
    if (!target) throw new Error("session/select requires threadId");
    if (target === this.threadId) {
      return asJsonValue({ threadId: target, previousThreadId: target, switched: false, available: true });
    }
    if (this.sessionSwitching) throw new Error("a session switch is already in progress");
    if (this.turnId || this.pending.size) {
      throw new Error("cannot switch sessions while a turn or approval is active");
    }
    const lifecycleGeneration = this.sessionLifecycleGeneration;
    this.sessionSwitching = true;
    const releaseSessionOperation = await this.acquireSessionOperation();
    const previousThreadId = this.threadId;
    const previousOwnerClientId = this.ownerClientId;
    const previousState = this.state;
    // Keep a copy of the last owner-validated old-session state before changing
    // the active conversation. It is a deterministic fallback if the target
    // cannot produce a snapshot (for example, an already-running desktop
    // writer).
    let previousConversationState: ConversationStreamState | undefined;
    let owner: string | null = null;
    let attachmentChanged = false;
    try {
      this.assertSessionSelectionCurrent(target, origin, lifecycleGeneration, expectedRouteGeneration);
      owner = await this.client.findThreadOwner(target, this.options.hostId, this.options.ownerDiscoveryTimeoutMs);
      if (!owner) throw new Error(`找不到会话 ${target} 的 VS Code Codex owner。请确认该会话已在官方 Codex 面板打开。`);
      this.assertSessionSelectionCurrent(target, origin, lifecycleGeneration, expectedRouteGeneration);
      // A local turn/approval may have appeared while owner discovery was in
      // flight. Re-check immediately before detaching the old projection.
      if (this.turnId || this.pending.size) {
        throw new Error("cannot switch sessions while a turn or approval is active");
      }
      // Rollback must use the adapter's last owner-validated projection. The
      // lower-level IPC cache sees every same-conversation snapshot before
      // this adapter can reject a stale/unknown owner, so reading that cache
      // here could resurrect content we deliberately ignored.
      previousConversationState = previousThreadId && previousOwnerClientId
        ? {
            conversationId: previousThreadId,
            hostId: this.options.hostId,
            ownerClientId: previousOwnerClientId,
            revision: this.revision ?? 0,
            conversationState: cloneObject(this.conversationState),
          }
        : undefined;
      this.emit({
        type: "session.switching",
        threadId: target,
        payload: { previousThreadId: previousThreadId ?? null, targetThreadId: target },
      });

      // Keep the old follow alive until the new owner has supplied an
      // authoritative snapshot. Events are filtered by `this.threadId`, so a
      // stale old event cannot overwrite the target projection during attach.
      this.threadId = target;
      this.ownerClientId = owner;
      this.state = "syncing";
      this.waitingForSession = false;
      this.resetConversationProjection();
      attachmentChanged = true;
      const waitForSnapshot = this.waitForSnapshot(target, this.options.followTimeoutMs, owner);
      // If follow itself fails, the catch path rejects the waiter. Attach a
      // handler immediately so that rejection can never become unhandled.
      void waitForSnapshot.catch(() => undefined);
      await this.client.followConversation(target, true, {
        hostId: this.options.hostId,
        targetClientIds: [owner],
      });
      await waitForSnapshot;
      this.assertSessionSelectionCurrent(target, origin, lifecycleGeneration, expectedRouteGeneration);
      // Revisions are scoped to an owner. A handoff can happen after the first
      // snapshot, so confirm the owner again before committing/unfollowing A.
      const confirmedOwner = await this.client.findThreadOwner(
        target,
        this.options.hostId,
        this.options.ownerDiscoveryTimeoutMs,
      );
      if (confirmedOwner !== owner) {
        throw new Error(`Codex conversation ${target} owner changed while switching`);
      }
      this.assertSessionSelectionCurrent(target, origin, lifecycleGeneration, expectedRouteGeneration);
      if (previousThreadId && previousOwnerClientId) {
        try {
          await this.client.followConversation(previousThreadId, false, {
            hostId: this.options.hostId,
            targetClientIds: [previousOwnerClientId],
          });
        } catch (error) {
          this.options.logger?.debug?.(`Unable to unfollow previous session ${previousThreadId}`, error);
        }
      }
      this.assertSessionSelectionCurrent(target, origin, lifecycleGeneration, expectedRouteGeneration);
      this.state = this.deriveSessionState();
      this.waitingForSession = false;
      this.clearWaitingDiscoveryTimer();
      if (this.options.loadCompleteHistory !== false) void this.loadCompleteHistoryIfNeeded();
      this.emit({
        type: "session.selected",
        threadId: target,
        payload: {
          threadId: target,
          activeThreadId: target,
          previousThreadId: previousThreadId ?? null,
          switched: true,
          available: true,
        },
      });
      return asJsonValue({ threadId: target, previousThreadId: previousThreadId ?? null, switched: true, available: true });
    } catch (error) {
      this.clearSnapshotWaiter(error instanceof Error ? error : new Error(String(error)));
      if (!attachmentChanged) throw error;
      const lifecycleCurrent = lifecycleGeneration === this.sessionLifecycleGeneration
        && !this.disposed
        && this.started;
      // Best-effort cleanup of the target subscription, then restore the old
      // attachment so a failed switch does not strand the bridge disconnected.
      if (lifecycleCurrent) {
        try {
          await this.client.followConversation(target, false, {
            hostId: this.options.hostId,
            ...(owner ? { targetClientIds: [owner] } : {}),
          });
        } catch { /* best effort */ }
      }
      // dispose()/onClose owns the final disconnected state. An interrupted
      // navigation must never publish rollback snapshots or reconnect after it.
      if (!lifecycleCurrent) throw error;
      this.threadId = previousThreadId;
      this.ownerClientId = previousOwnerClientId;
      this.state = previousState;
      this.waitingForSession = !previousThreadId;
      this.resetConversationProjection();
      // Move Relay/Web back before re-publishing the old snapshot. Browser
      // command errors arrive after adapter events, so relying on only the
      // command envelope would make the restored old projection look like an
      // out-of-route event and leave the early target snapshot mounted.
      if (previousThreadId) {
        this.emit({
          type: "session.selected",
          threadId: previousThreadId,
          payload: {
            threadId: previousThreadId,
            activeThreadId: previousThreadId,
            previousThreadId: target,
            targetThreadId: target,
            switched: false,
            available: true,
            failed: true,
            origin,
          },
        });
      }
      const restoredFromCache = this.restoreCachedConversationProjection(
        previousConversationState,
        previousOwnerClientId,
      );
      if (restoredFromCache && previousThreadId) {
        // Re-assert the old follow without waiting for another snapshot. The
        // cached state is already authoritative and keeps the bridge usable
        // even when the owner does not answer a duplicate follow request.
        try {
          await this.client.followConversation(previousThreadId, true, {
            hostId: this.options.hostId,
            targetClientIds: this.ownerClientId ? [this.ownerClientId] : undefined,
          });
        } catch (restoreError) {
          this.options.logger?.debug?.("Unable to re-follow previous Codex session after switch failure", restoreError);
        }
      } else if (previousThreadId && previousOwnerClientId) {
        try {
          const restoreWaiter = this.waitForSnapshot(previousThreadId, this.options.followTimeoutMs, previousOwnerClientId);
          void restoreWaiter.catch(() => undefined);
          await this.client.followConversation(previousThreadId, true, {
            hostId: this.options.hostId,
            targetClientIds: [previousOwnerClientId],
          });
          await restoreWaiter;
          this.state = this.deriveSessionState();
        } catch (restoreError) {
          this.options.logger?.warn?.("Unable to restore previous Codex session after switch failure", restoreError);
        }
      }
      if (!previousThreadId && this.started && !this.disposed) this.scheduleWaitingDiscovery();
      throw error;
    } finally {
      releaseSessionOperation();
      this.sessionSwitching = false;
    }
  }

  /** Acquire a FIFO lock shared by session probes and attachment changes. */
  private async acquireSessionOperation(): Promise<() => void> {
    const previous = this.sessionOperationTail;
    let release!: () => void;
    this.sessionOperationTail = new Promise<void>((resolve) => { release = resolve; });
    await previous;
    return release;
  }

  private assertSessionSelectionCurrent(
    target: string,
    origin: "web" | "vscode",
    lifecycleGeneration: number,
    expectedRouteGeneration?: number,
  ): void {
    if (lifecycleGeneration !== this.sessionLifecycleGeneration || this.disposed || !this.started) {
      throw new Error("session selection was cancelled because the IPC session closed");
    }
    if (origin === "vscode"
      && (expectedRouteGeneration === undefined
        || expectedRouteGeneration !== this.vscodeRouteGeneration
        || this.vscodeRouteActiveThreadId !== target)) {
      throw new Error("session selection was superseded by a newer VS Code route");
    }
  }

  async interruptTurn(params: JsonObject = {}): Promise<JsonValue> {
    this.ensureInteractiveReady();
    const expected = stringValue(params.turnId) ?? stringValue(params.expectedTurnId) ?? this.turnId;
    if (!expected) throw new Error("turn/interrupt requires an active turn");
    const mode = stringValue(params.mode) ?? "user-stop";
    const result = await this.client.interruptTurn(this.threadId as string, {
      mode,
      expectedTurnId: expected,
      ownerClientId: this.ownerClientId as string,
      timeoutMs: this.options.followTimeoutMs,
    });
    this.turnId = null;
    this.state = "idle";
    const elapsed = this.status.elapsedMs ?? this.status.durationMs ?? null;
    this.status = {
      ...this.status,
      activity: "interrupted",
      turnStatus: "interrupted",
      activeFlags: [],
      durationMs: elapsed,
      workedDurationMs: this.status.workedDurationMs ?? elapsed,
      elapsedMs: elapsed,
    };
    this.emit({ type: "task.cancelled", threadId: this.threadId ?? undefined, turnId: expected, payload: { mode } });
    return asJsonValue(unwrapFollowerResult(result));
  }

  async sendInput(text: string, params: JsonObject = {}): Promise<JsonValue> {
    const body = { ...params, text };
    return this.turnId ? this.steerTurn(body) : this.startTurn(body);
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
    return this.resolvePendingResponse(requestId, decision, reason, response, false);
  }

  private async resolvePendingResponse(
    requestId: JsonRpcId,
    decision: "allow" | "deny" | "cancel",
    reason: string | undefined,
    response: JsonValue | undefined,
    allowDuringSessionSwitch: boolean,
  ): Promise<JsonValue> {
    this.ensureAttached();
    if (this.sessionSwitching && !allowDuringSessionSwitch) {
      throw new Error("Codex session switch is still in progress");
    }
    const key = jsonRpcIdKey(requestId);
    const pending = this.pending.get(key);
    if (!pending) throw new Error(`unknown or already resolved request: ${key}`);
    const wire = this.toWireResponse(pending, decision, reason, response);
    // Remove before awaiting the owner so a repeated browser click cannot send
    // the same approval twice. If the IPC request fails, restore it for retry.
    this.pending.delete(key);
    this.clearPendingTimer(key);
    this.optimisticallyResolved.add(key);
    let result: JsonValue | undefined;
    try {
      result = await this.sendPendingResponse(pending, wire);
    } catch (error) {
      this.optimisticallyResolved.delete(key);
      this.pending.set(key, pending);
      this.schedulePendingExpiry(key, pending);
      throw error;
    }
    this.emit({
      type: pending.approval ? "approval.resolved" : "input.resolved",
      threadId: pending.threadId ?? this.threadId ?? undefined,
      turnId: pending.turnId,
      requestId,
      payload: { requestId: asJsonValue(requestId), method: pending.method, decision },
    });
    return asJsonValue(result);
  }

  async denyPending(reason = "relay disconnected"): Promise<void> {
    const entries = [...this.pending.values()];
    await Promise.all(entries.map(async (entry) => {
      try {
        await this.resolvePendingResponse(entry.requestId, "deny", reason, undefined, true);
      } catch { /* fail closed when owner is gone */ }
    }));
  }

  private async sendPendingResponse(entry: RequestEntry, wire: JsonValue): Promise<JsonValue | undefined> {
    const conversationId = this.threadId as string;
    const options = { ownerClientId: this.ownerClientId as string, timeoutMs: this.options.followTimeoutMs };
    if (entry.method === "item/commandExecution/requestApproval" || entry.method === "execCommandApproval") {
      return this.client.respondCommandApproval(conversationId, entry.requestId, wire, options);
    }
    if (entry.method === "item/fileChange/requestApproval" || entry.method === "applyPatchApproval") {
      return this.client.respondFileApproval(conversationId, entry.requestId, wire, options);
    }
    if (entry.method === "item/permissions/requestApproval") {
      return this.client.respondPermissionsApproval(conversationId, entry.requestId, wire, options);
    }
    if (entry.method === "item/tool/requestUserInput") {
      return this.client.respondUserInput(conversationId, entry.requestId, wire, options);
    }
    if (entry.method === "mcpServer/elicitation/request") {
      return this.client.respondMcpElicitation(conversationId, entry.requestId, wire, options);
    }
    throw new Error(`unsupported follower request method: ${entry.method}`);
  }

  private schedulePendingExpiry(key: string, entry: RequestEntry): void {
    if (!entry.expiresAt || this.options.approvalTimeoutMs <= 0) {
      // A later snapshot can omit an expiry that was present in an earlier
      // request record. Do not leave the old timer alive in that case.
      this.clearPendingTimer(key);
      return;
    }
    const existing = this.pendingExpiryAt.get(key);
    if (existing === entry.expiresAt && this.pendingTimers.has(key)) return;
    this.clearPendingTimer(key);
    const delay = Math.max(0, entry.expiresAt - Date.now());
    this.pendingExpiryAt.set(key, entry.expiresAt);
    this.pendingTimers.set(key, setTimeout(() => {
      this.pendingTimers.delete(key);
      this.pendingExpiryAt.delete(key);
      void this.expirePending(key);
    }, delay));
  }

  private clearPendingTimer(key: string): void {
    const timer = this.pendingTimers.get(key);
    if (timer) clearTimeout(timer);
    this.pendingTimers.delete(key);
    this.pendingExpiryAt.delete(key);
  }

  private async expirePending(key: string): Promise<void> {
    const entry = this.pending.get(key);
    if (!entry || this.disposed || !this.started) return;
    this.pending.delete(key);
    this.optimisticallyResolved.add(key);
    try {
      const wire = this.toWireResponse(entry, "deny", "approval expired");
      await this.sendPendingResponse(entry, wire);
    } catch (error) {
      this.options.logger?.debug?.(`Unable to send expiry response for ${key}`, error);
    }
    this.emit({
      type: entry.approval ? "approval.expired" : "input.expired",
      threadId: entry.threadId ?? this.threadId ?? undefined,
      turnId: entry.turnId,
      requestId: entry.requestId,
      payload: { requestId: asJsonValue(entry.requestId), method: entry.method, reason: "approval expired" },
    });
  }

  async snapshot(): Promise<SessionSnapshot> {
    const requests = [...this.pending.values()];
    return {
      threadId: this.threadId,
      turnId: this.turnId,
      state: this.state,
      status: { ...this.status, activeFlags: [...this.status.activeFlags] },
      activity: this.status.activity,
      turnStatus: this.status.turnStatus,
      activeFlags: [...this.status.activeFlags],
      startedAtMs: this.status.startedAtMs ?? null,
      durationMs: this.status.durationMs ?? null,
      workedDurationMs: this.status.workedDurationMs ?? null,
      elapsedMs: this.status.elapsedMs ?? null,
      pendingApprovals: requests.map((entry) => entry.approval).filter((entry): entry is PendingApproval => Boolean(entry)),
      pendingRequests: requests.map((entry) => ({
        requestId: entry.requestId,
        method: entry.method,
        params: redactJson(entry.params),
        ...(entry.approval?.commandHash ? { commandHash: entry.approval.commandHash } : {}),
        ...(entry.approval?.risk ? { risk: entry.approval.risk } : {}),
        ...(entry.approval?.summary ? { summary: entry.approval.summary } : {}),
        createdAt: entry.createdAt,
        ...(entry.expiresAt ? { expiresAt: entry.expiresAt } : {}),
      })),
      outputTail: this.outputTail,
      messages: asJsonValue(this.outputMessages) as JsonValue[],
      subagents: this.subagents.map((subagent) => ({ ...subagent })),
      metadata: {
        adapter: "codex-ipc-follower",
        mode: "attach",
        privateProtocol: true,
        socketPath: this.client.socketPath,
        waitingForSession: this.waitingForSession,
        attachReady: Boolean(this.threadId && this.ownerClientId && !this.waitingForSession && this.state !== "syncing"),
        ...(this.ownerClientId ? { ownerClientId: this.ownerClientId } : {}),
        ...(this.revision !== null ? { revision: this.revision } : {}),
        ...(typeof this.conversationState.cwd === "string" ? { cwd: this.conversationState.cwd } : {}),
        ...(typeof this.conversationState.title === "string" ? { title: this.conversationState.title } : {}),
        ...(typeof this.conversationState.source === "string" ? { source: this.conversationState.source } : {}),
        ...projectSessionMetadata(this.conversationState),
        activity: this.status.activity,
        turnStatus: this.status.turnStatus,
        activeFlags: asJsonValue(this.status.activeFlags),
        startedAtMs: this.status.startedAtMs ?? null,
        durationMs: this.status.durationMs ?? null,
        workedDurationMs: this.status.workedDurationMs ?? null,
        elapsedMs: this.status.elapsedMs ?? null,
        firstTurnWorkItemStartedAtMs: this.status.firstTurnWorkItemStartedAtMs ?? null,
        finalAssistantStartedAtMs: this.status.finalAssistantStartedAtMs ?? null,
        historyComplete: !hasIncompleteHistory(this.conversationState),
      },
    };
  }

  onEvent(listener: (event: AgentEvent) => void): Disposable {
    this.listeners.add(listener);
    return { dispose: () => this.listeners.delete(listener) };
  }

  async dispose(): Promise<void> {
    if (this.disposed) return;
    this.sessionLifecycleGeneration += 1;
    this.clearWaitingDiscoveryTimer();
    this.waitingForSession = false;
    this.clearSnapshotWaiter(new Error("Codex IPC follower is stopping"));
    this.clearRevisionWaiter(new Error("Codex IPC follower is stopping"));
    // While the private IPC socket is still writable, close every outstanding
    // owner request explicitly so stopping the bridge cannot strand an approval
    // dialog in the official Codex UI.
    if (this.started && this.pending.size) await this.denyPending("bridge stopped");
    this.disposed = true;
    this.started = false;
    this.state = "disconnected";
    this.clearPendingVscodeFollow();
    this.vscodeRouteActiveThreadId = null;
    this.vscodeRouteAwaitingSelection = false;
    this.vscodeRouteClientId = null;
    this.vscodeRouteCandidates.clear();
    this.activeVscodeSelection = undefined;
    this.resetHistoryLoading();
    this.clearSnapshotWaiter(new Error("Codex IPC follower disposed"));
    this.clearRevisionWaiter(new Error("Codex IPC follower disposed"));
    for (const timer of this.pendingTimers.values()) clearTimeout(timer);
    this.pendingTimers.clear();
    this.pendingExpiryAt.clear();
    if (this.threadId) {
      try {
        await this.client.followConversation(this.threadId, false, {
          hostId: this.options.hostId,
          targetClientIds: this.ownerClientId ? [this.ownerClientId] : undefined,
        });
      } catch { /* socket may already be closed */ }
    }
    for (const entry of this.pending.values()) {
      if (entry.approval) this.emit({ type: "approval.expired", threadId: entry.threadId, turnId: entry.turnId, requestId: entry.requestId, payload: { requestId: asJsonValue(entry.requestId), reason: "bridge stopped" } });
      else this.emit({ type: "input.expired", threadId: entry.threadId, turnId: entry.turnId, requestId: entry.requestId, payload: { requestId: asJsonValue(entry.requestId), reason: "bridge stopped" } });
    }
    this.pending.clear();
    this.optimisticallyResolved.clear();
    for (const subscription of this.subscriptions.splice(0)) subscription.dispose();
    if (this.options.disposeClient) await this.client.dispose();
  }

  private async discoverThreadId(excluded = new Set<string>(), maxCandidates = 64): Promise<string | undefined> {
    if (!this.options.autoDiscoverThread) return undefined;
    const codexHome = resolveCodexHome(this.options);
    const candidates = await recentVscodeThreadCandidates(path.join(codexHome, "sessions"), this.options.preferredCwds ?? []);
    if (!candidates.length) return undefined;
    // Owner discovery is the authority: a rollout file can remain on disk
    // after its VS Code owner is gone. An older conversation can still be the
    // one currently open in the official panel, so inspect enough candidates
    // to cover its bounded recent-chat view. Eight-way batches with a short
    // timeout keep the worst-case startup budget no larger than the former
    // 12-candidate, four-way scan.
    const limited = candidates.filter((candidate) => !excluded.has(candidate.id)).slice(0, Math.max(1, maxCandidates));
    const ownerTimeout = Math.max(250, Math.min(this.options.ownerDiscoveryTimeoutMs, 750));
    for (let offset = 0; offset < limited.length; offset += 8) {
      const batch = limited.slice(offset, offset + 8);
      const checks = await Promise.all(batch.map(async (candidate) => {
        try {
          const owner = await this.client.findThreadOwner(candidate.id, this.options.hostId, ownerTimeout);
          return owner ? candidate : undefined;
        } catch (error) {
          this.options.logger?.debug?.(`Thread owner discovery failed for ${candidate.id}`, error);
          return undefined;
        }
      }));
      const selected = checks.find((candidate): candidate is Candidate => Boolean(candidate));
      if (selected) {
        this.options.logger?.info?.(`Auto-discovered VS Code Codex conversation ${selected.id}`);
        return selected.id;
      }
    }
    return undefined;
  }

  /**
   * Keep the relay/IPC connection usable while the official panel has no
   * selected conversation yet. The first route broadcast is handled as a
   * fast path; this bounded poll covers sessions opened by older extension
   * builds that do not emit the route notification.
   */
  private enterWaitingForSession(): void {
    if (this.disposed) return;
    const wasStarted = this.started;
    const wasWaiting = this.waitingForSession;
    this.started = true;
    this.waitingForSession = true;
    this.state = "waiting_for_host";
    this.threadId = null;
    this.ownerClientId = null;
    this.vscodeRouteActiveThreadId = null;
    this.vscodeRouteClientId = null;
    this.vscodeRouteAwaitingSelection = false;
    this.vscodeRouteCandidates.clear();
    this.resetConversationProjection();
    // RelayHost normally synthesizes this event after adapter.start(). Emit it
    // here as well so the adapter can be used directly and so a transition
    // back to waiting clears any provisional target in existing browsers.
    if (!wasWaiting) {
      this.emit({ type: "connection.opened", payload: { mode: "attach", waitingForSession: true } });
      if (wasStarted) void this.emitSnapshot();
    }
    this.scheduleWaitingDiscovery();
  }

  private scheduleWaitingDiscovery(delayMs = WAITING_SESSION_DISCOVERY_DELAY_MS): void {
    if (this.disposed || !this.started || !this.waitingForSession || this.waitingDiscoveryTimer) return;
    this.waitingDiscoveryTimer = setTimeout(() => {
      this.waitingDiscoveryTimer = undefined;
      void this.runWaitingDiscovery();
    }, Math.max(0, delayMs));
  }

  private clearWaitingDiscoveryTimer(): void {
    if (this.waitingDiscoveryTimer) clearTimeout(this.waitingDiscoveryTimer);
    this.waitingDiscoveryTimer = undefined;
  }

  private async runWaitingDiscovery(): Promise<void> {
    if (this.disposed || !this.started || !this.waitingForSession || this.waitingDiscoveryInFlight) return;
    const generation = this.sessionLifecycleGeneration;
    const routeGeneration = this.vscodeRouteGeneration;
    const officialRouteHasPriority = () => this.options.followVscodeSession
      && (this.vscodeRouteGeneration !== routeGeneration
        || Boolean(this.vscodeRouteClientId && this.vscodeRouteActiveThreadId));
    const task = (async () => {
      // Polling is only a fallback for panels/builds that do not broadcast their
      // current route. Once the official panel supplies a route, never let a
      // slower filesystem/owner lookup replace it with an older recent thread.
      if (officialRouteHasPriority()) return;
      const configured = this.options.threadId?.trim();
      if (configured && await this.tryAttachWaitingThread(configured)) return;
      if (officialRouteHasPriority()) return;
      if (!this.options.autoDiscoverThread) return;
      const selected = await this.discoverThreadId(
        configured ? new Set([configured]) : new Set<string>(),
        WAITING_SESSION_DISCOVERY_MAX_CANDIDATES,
      );
      if (selected && !officialRouteHasPriority()) await this.tryAttachWaitingThread(selected);
    })();
    this.waitingDiscoveryInFlight = task;
    try {
      await task;
    } catch (error) {
      if (this.started && !this.disposed) {
        this.options.logger?.debug?.("Waiting for a VS Code Codex session", error);
      }
    } finally {
      if (this.waitingDiscoveryInFlight === task) this.waitingDiscoveryInFlight = undefined;
      // A socket close/dispose may have happened while discovery was in
      // flight. The generation check prevents a stale completion from
      // scheduling a new timer on a dead adapter.
      if (generation === this.sessionLifecycleGeneration && this.started && this.waitingForSession && !this.disposed) {
        this.scheduleWaitingDiscovery();
      }
    }
  }

  private async tryAttachWaitingThread(target: string): Promise<boolean> {
    const normalizedTarget = target.trim();
    if (!normalizedTarget || this.disposed || !this.started) return false;
    if (this.waitingAttachPromise) {
      if (this.waitingAttachTarget !== normalizedTarget) this.queuedWaitingAttachTarget = normalizedTarget;
      return this.waitingAttachTarget === normalizedTarget ? this.waitingAttachPromise : false;
    }
    if (!this.waitingForSession) return false;
    this.waitingAttachTarget = normalizedTarget;
    const task = (async () => {
      const release = await this.acquireSessionOperation();
      this.sessionSwitching = true;
      try {
        if (this.disposed || !this.started || !this.waitingForSession) return false;
        await this.attachThread(normalizedTarget, { fromWaiting: true });
        return true;
      } catch (error) {
        if (this.started && !this.disposed) {
          this.options.logger?.debug?.(`Unable to attach waiting VS Code session ${normalizedTarget}`, error);
          // attachThread restores the waiting projection for a failed
          // fromWaiting attempt. Keep the retry timer alive for the next poll.
          this.scheduleWaitingDiscovery();
        }
        return false;
      } finally {
        this.sessionSwitching = false;
        release();
      }
    })();
    this.waitingAttachPromise = task;
    let attached = false;
    try {
      attached = await task;
      return attached;
    } finally {
      if (this.waitingAttachPromise === task) this.waitingAttachPromise = undefined;
      this.waitingAttachTarget = null;
      const queued = this.queuedWaitingAttachTarget;
      this.queuedWaitingAttachTarget = null;
      if (queued && !this.disposed && this.started) {
        if (this.waitingForSession) {
          void this.tryAttachWaitingThread(queued);
        } else if (queued !== this.threadId) {
          // The official panel can move A -> B while A's first snapshot is in
          // flight. Preserve the latest route and reuse the normal deferred
          // switching path so an active turn on A is never detached early.
          this.pendingVscodeThreadId = queued;
          this.pendingVscodeFollowAttempts = 0;
          this.pendingVscodeFollowGeneration = this.vscodeRouteGeneration;
          this.schedulePendingVscodeFollow(this.options.vscodeSessionFollowDebounceMs);
        }
      }
    }
  }

  /**
   * Verify that a discovered owner can actually stream this conversation.
   * Owner discovery may return a desktop client (or a stale handoff) that
   * knows the id but cannot provide the follower snapshot needed for attach.
   * The temporary follow is isolated to a direct stream listener and is always
   * removed before returning, so probing never changes the active projection.
   */
  private async probeSessionSnapshot(conversationId: string, ownerClientId: string, timeoutMs: number): Promise<boolean> {
    let timer: NodeJS.Timeout | undefined;
    let resolveSnapshot!: (available: boolean) => void;
    const snapshot = new Promise<boolean>((resolve) => {
      resolveSnapshot = resolve;
      timer = setTimeout(() => resolve(false), timeoutMs);
    });
    const subscription = this.client.onStreamEvent((event) => {
      if (event.kind === "snapshot"
        && event.conversationId === conversationId
        && event.ownerClientId === ownerClientId) {
        resolveSnapshot(true);
      }
    });
    try {
      await this.client.followConversation(conversationId, true, {
        hostId: this.options.hostId,
        targetClientIds: [ownerClientId],
      });
      return await snapshot;
    } catch (error) {
      this.options.logger?.debug?.(`Session snapshot probe failed for ${conversationId}`, error);
      return false;
    } finally {
      if (timer) clearTimeout(timer);
      subscription.dispose();
      // A user may select this row while the probe is waiting. In that case
      // the temporary follow has become the active attachment; do not tear it
      // down from the list request's cleanup path.
      if (!(this.threadId === conversationId && this.ownerClientId === ownerClientId)) {
        try {
          await this.client.followConversation(conversationId, false, {
            hostId: this.options.hostId,
            targetClientIds: [ownerClientId],
          });
        } catch (error) {
          this.options.logger?.debug?.(`Unable to clean up session snapshot probe ${conversationId}`, error);
        }
      }
    }
  }

  /**
   * Observe the official panel's route without reading its DOM. The official
   * webview broadcasts old=false followed by new=true from one stable IPC
   * client. Requiring that pair avoids treating reconnect status replays or a
   * different Codex window's isolated true broadcast as a user navigation.
   */
  private handleBroadcast(frame: IpcBroadcast): void {
    if (!this.options.followVscodeSession || !this.started || this.disposed) return;
    if (frame.method === "client-status-changed") {
      this.handleRouteClientStatus(frame);
      return;
    }
    if (frame.method !== "thread-stream-following-changed") return;
    if (this.options.strictVersions !== false
      && frame.version !== CODEX_IPC_METHOD_VERSIONS["thread-stream-following-changed"]) return;
    // Status replies sent to a newly connected follower describe retained
    // subscriptions, not a fresh route change in the VS Code panel.
    if (frame.targetClientIds?.length) return;
    if (!isRecord(frame.params)) return;
    const conversationId = stringValue(frame.params.conversationId)?.trim();
    const hostId = stringValue(frame.params.hostId) ?? "local";
    const sourceClientId = frame.sourceClientId?.trim();
    if (!conversationId || hostId !== this.options.hostId || !sourceClientId) return;
    if (sourceClientId === this.client.getClientId()) return;

    // There is no trusted old route while the first conversation is missing.
    // Treat an untargeted `following:true` as a candidate hint, then verify it
    // through owner discovery and an authoritative snapshot before attaching.
    if (this.waitingForSession || this.waitingAttachPromise) {
      if (frame.params.following !== true) return;
      if (this.vscodeRouteClientId && sourceClientId !== this.vscodeRouteClientId) return;
      this.vscodeRouteClientId = sourceClientId;
      if (this.vscodeRouteActiveThreadId !== conversationId) {
        this.vscodeRouteActiveThreadId = conversationId;
        this.vscodeRouteGeneration += 1;
      }
      void this.tryAttachWaitingThread(conversationId);
      return;
    }

    // Bind only after one source completes old=false -> new=true. A different
    // remote follower can emit an isolated false while disposing, and that
    // must not permanently steal the trusted route source.
    if (!this.vscodeRouteClientId) {
      if (frame.params.following === false) {
        if (conversationId === this.vscodeRouteActiveThreadId) {
          this.vscodeRouteCandidates.set(sourceClientId, conversationId);
        }
        return;
      }
      if (frame.params.following !== true) return;
      if (this.vscodeRouteCandidates.get(sourceClientId) !== this.vscodeRouteActiveThreadId) return;
      this.vscodeRouteClientId = sourceClientId;
      this.vscodeRouteCandidates.clear();
      this.vscodeRouteActiveThreadId = null;
      this.vscodeRouteAwaitingSelection = true;
      this.vscodeRouteGeneration += 1;
    }
    if (sourceClientId !== this.vscodeRouteClientId) return;

    if (frame.params.following === false) {
      if (conversationId !== this.vscodeRouteActiveThreadId) return;
      this.vscodeRouteGeneration += 1;
      this.vscodeRouteActiveThreadId = null;
      this.vscodeRouteAwaitingSelection = true;
      if (this.activeVscodeSelection?.target === conversationId) {
        this.clearSnapshotWaiter(new Error(`VS Code moved away from conversation ${conversationId}`));
      }
      // A -> B -> C can happen faster than B's snapshot. Cancel the queued B
      // as soon as the official route reports that B is no longer active.
      if (this.pendingVscodeThreadId === conversationId) this.clearPendingVscodeFollow();
      return;
    }
    if (frame.params.following !== true) return;

    if (conversationId === this.vscodeRouteActiveThreadId) return;
    if (!this.vscodeRouteAwaitingSelection) {
      // Initial/reconnect following status. It may confirm the current route,
      // but an isolated true is intentionally never treated as navigation.
      if (conversationId === this.threadId) this.vscodeRouteActiveThreadId = conversationId;
      return;
    }
    this.vscodeRouteAwaitingSelection = false;
    this.vscodeRouteActiveThreadId = conversationId;
    this.vscodeRouteGeneration += 1;
    if (conversationId === this.threadId) {
      this.clearPendingVscodeFollow();
      return;
    }
    this.pendingVscodeThreadId = conversationId;
    this.pendingVscodeFollowAttempts = 0;
    this.pendingVscodeFollowGeneration = this.vscodeRouteGeneration;
    this.schedulePendingVscodeFollow(this.options.vscodeSessionFollowDebounceMs);
  }

  /**
   * The official webview gets a new IPC client id after its socket reconnects.
   * Drop only the trusted route-source binding when that client disconnects;
   * the next same-source false -> true pair can then establish the replacement.
   */
  private handleRouteClientStatus(frame: IpcBroadcast): void {
    if (this.options.strictVersions !== false && frame.version !== 0) return;
    if (!isRecord(frame.params)) return;
    const clientId = (stringValue(frame.params.clientId) ?? frame.sourceClientId)?.trim();
    const status = stringValue(frame.params.status)?.trim().toLowerCase();
    if (!clientId || status !== "disconnected") return;
    this.vscodeRouteCandidates.delete(clientId);
    if (clientId !== this.vscodeRouteClientId) return;

    this.options.logger?.debug?.(`VS Code Codex route client ${clientId} disconnected; awaiting a replacement source`);
    this.vscodeRouteClientId = null;
    this.vscodeRouteCandidates.clear();
    this.vscodeRouteAwaitingSelection = false;
    // Keep the route identity, rather than forcing it back to the currently
    // committed relay thread. A disconnect may occur between B's true signal
    // and B's snapshot; the replacement panel will later leave B with false.
    this.vscodeRouteActiveThreadId = this.activeVscodeSelection?.target
      ?? this.vscodeRouteActiveThreadId
      ?? this.threadId;
    this.vscodeRouteGeneration += 1;
    this.clearPendingVscodeFollow();
    if (this.activeVscodeSelection) {
      this.clearSnapshotWaiter(new Error("VS Code Codex route client disconnected during session selection"));
    }
  }

  private schedulePendingVscodeFollow(delayMs: number): void {
    if (!this.pendingVscodeThreadId || this.disposed || !this.started) return;
    if (this.vscodeSessionFollowTimer) clearTimeout(this.vscodeSessionFollowTimer);
    this.vscodeSessionFollowTimer = setTimeout(() => {
      this.vscodeSessionFollowTimer = undefined;
      void this.applyPendingVscodeFollow();
    }, Math.max(0, delayMs));
  }

  private async applyPendingVscodeFollow(): Promise<void> {
    const target = this.pendingVscodeThreadId;
    const routeGeneration = this.pendingVscodeFollowGeneration;
    if (!target || this.disposed || !this.started) return;
    if (this.vscodeRouteActiveThreadId !== target || routeGeneration !== this.vscodeRouteGeneration) {
      if (this.pendingVscodeThreadId === target) this.clearPendingVscodeFollow();
      return;
    }
    if (target === this.threadId) {
      this.clearPendingVscodeFollow();
      return;
    }
    // Never detach a turn or approval that is still owned by the old thread.
    // Keep only the latest official target and retry once that state is idle.
    if (this.sessionSwitching || this.turnId || this.pending.size) {
      this.schedulePendingVscodeFollow(VSCODE_SESSION_FOLLOW_RETRY_DELAY_MS);
      return;
    }

    this.pendingVscodeThreadId = null;
    this.pendingVscodeFollowGeneration = 0;
    this.activeVscodeSelection = { target, generation: routeGeneration };
    try {
      await this.selectSession({
        threadId: target,
        origin: "vscode",
        vscodeRouteGeneration: routeGeneration,
      });
      this.pendingVscodeFollowAttempts = 0;
      this.options.logger?.info?.(`Followed VS Code Codex panel to conversation ${target}`);
    } catch (error) {
      this.options.logger?.debug?.(`Unable to follow VS Code Codex panel to ${target}`, error);
      if (!this.disposed
        && this.started
        && !this.pendingVscodeThreadId
        && this.vscodeRouteActiveThreadId === target
        && this.vscodeRouteGeneration === routeGeneration
        && this.pendingVscodeFollowAttempts < VSCODE_SESSION_FOLLOW_MAX_ATTEMPTS) {
        this.pendingVscodeFollowAttempts += 1;
        this.pendingVscodeThreadId = target;
        this.pendingVscodeFollowGeneration = routeGeneration;
      }
    } finally {
      if (this.activeVscodeSelection?.target === target
        && this.activeVscodeSelection.generation === routeGeneration) {
        this.activeVscodeSelection = undefined;
      }
      if (this.pendingVscodeThreadId) {
        this.schedulePendingVscodeFollow(VSCODE_SESSION_FOLLOW_RETRY_DELAY_MS);
      }
    }
  }

  private clearPendingVscodeFollow(): void {
    if (this.vscodeSessionFollowTimer) clearTimeout(this.vscodeSessionFollowTimer);
    this.vscodeSessionFollowTimer = undefined;
    this.pendingVscodeThreadId = null;
    this.pendingVscodeFollowAttempts = 0;
    this.pendingVscodeFollowGeneration = 0;
  }

  private handleStreamEvent(event: ConversationStreamEvent): void {
    if (!this.threadId || event.conversationId !== this.threadId) return;
    // Following is targeted at the owner discovered during attach. The IPC
    // router normally filters these broadcasts, but a handoff/old socket can
    // still surface another client's event; applying it would overwrite the
    // active conversation and could satisfy a revision waiter incorrectly.
    if (this.ownerClientId && event.ownerClientId && event.ownerClientId !== this.ownerClientId) {
      this.options.logger?.debug?.(`Ignoring stream event from unexpected Codex owner ${event.ownerClientId}`);
      return;
    }
    if (event.kind === "desync") {
      this.options.logger?.warn?.(`Codex IPC stream desynchronized at revision ${event.receivedBaseRevision}; requesting snapshot`);
      this.client.followConversation(this.threadId, true, { hostId: this.options.hostId, targetClientIds: this.ownerClientId ? [this.ownerClientId] : undefined }).catch((error) => this.options.logger?.warn?.("Unable to recover IPC snapshot", error));
      return;
    }
    this.ownerClientId = event.ownerClientId || this.ownerClientId;
    this.revision = event.revision;
    if (this.revisionWaiter
      && this.revisionWaiter.threadId === event.conversationId
      && this.revisionWaiter.ownerClientId === event.ownerClientId
      && event.revision >= this.revisionWaiter.revision) {
      this.clearRevisionWaiter();
    }
    this.conversationState = cloneObject(event.conversationState);
    this.processConversationState(event.kind === "snapshot");
    if (event.kind === "snapshot" && this.options.loadCompleteHistory !== false) void this.loadCompleteHistoryIfNeeded();
    if (event.kind === "snapshot"
      && this.snapshotWaiter?.threadId === event.conversationId
      && (!this.snapshotWaiter.ownerClientId || this.snapshotWaiter.ownerClientId === event.ownerClientId)) {
      this.clearSnapshotWaiter();
    }
  }

  private processConversationState(initial: boolean): void {
    if (initial) this.snapshotSeen = true;
    const previousTurnId = this.turnId;
    const previousState = this.state;
    const previousStatus = this.status;
    const nextTurn = deriveTurn(this.conversationState);
    // Request records and runtime flags are part of the same conversation
    // snapshot. Derive status after extracting them so an approval/input wait
    // is visible immediately with the corresponding state patch.
    const nextRequests = extractRequests(this.conversationState, this.options.approvalTimeoutMs);
    this.status = deriveStatusSnapshot(this.conversationState, nextTurn, nextRequests);
    this.turnId = nextTurn.active ? nextTurn.id ?? null : null;
    this.state = nextTurn.active ? "active" : this.deriveSessionState();
    const nextHistoryComplete = !hasIncompleteHistory(this.conversationState);
    const historyChanged = this.historyComplete !== undefined && this.historyComplete !== nextHistoryComplete;
    // Settings updates are broadcast as conversation-state patches and may not
    // change output, turn status, or pending requests. Fingerprint only the
    // redacted projection exposed by `snapshot()` so those patches still reach
    // the relay without leaking opaque/private state or causing per-token
    // snapshot spam.
    const metadataShape = stableStringify(projectSessionMetadata(this.conversationState));
    const metadataChanged = metadataShape !== this.renderedMetadataShape;

    const previousPending = new Map(this.pending);
    const nextKeys = new Set(nextRequests.map((entry) => jsonRpcIdKey(entry.requestId)));
    for (const key of this.optimisticallyResolved) {
      if (!nextKeys.has(key)) this.optimisticallyResolved.delete(key);
    }
    const previousKeys = new Set(previousPending.keys());
    for (const key of this.pendingTimers.keys()) {
      if (!nextKeys.has(key)) this.clearPendingTimer(key);
    }
    this.pending.clear();
    for (const rawEntry of nextRequests) {
      const prior = previousPending.get(jsonRpcIdKey(rawEntry.requestId));
      // Some official request records omit timestamps. Keep the first-seen
      // deadline across patches instead of extending it on every output delta.
      const entry = prior
        ? { ...rawEntry, createdAt: prior.createdAt, expiresAt: prior.expiresAt ?? rawEntry.expiresAt }
        : rawEntry;
      const key = jsonRpcIdKey(entry.requestId);
      if (this.optimisticallyResolved.has(key)) continue;
      this.pending.set(key, entry);
      this.schedulePendingExpiry(key, entry);
      if (!previousKeys.has(key)) this.emitRequest(entry);
    }
    for (const key of previousKeys) {
      if (nextKeys.has(key) || this.optimisticallyResolved.has(key)) continue;
      const old = previousPending.get(key);
      if (old) this.emit({ type: old.approval ? "approval.resolved" : "input.resolved", threadId: old.threadId ?? this.threadId ?? undefined, turnId: old.turnId, requestId: old.requestId, payload: { requestId: asJsonValue(old.requestId), method: old.method } });
    }

    const rendered = renderConversationOutput(this.conversationState, this.options.maxOutputTailChars);
    const output = rendered.text;
    const messageShape = renderedMessageShape(rendered.messages);
    const messagesChanged = messageShape !== this.renderedMessageShape;
    const messagesPatch = messagesChanged
      ? renderedMessagesPatch(this.outputMessages, rendered.messages)
      : undefined;
    const subagentShape = renderedSubagentShape(rendered.subagents);
    const subagentsChanged = subagentShape !== this.renderedSubagentShape;
    if (output !== this.renderedOutput || rendered.totalLength !== this.renderedOutputLength || messagesChanged || subagentsChanged) {
      const delta = this.renderedOutput
        ? appendOnlyOutputDelta(
          this.renderedOutput,
          this.renderedOutputLength,
          output,
          rendered.totalLength,
          this.renderedOutputWasTruncated,
        )
        : undefined;
      if (!this.renderedOutput || delta === undefined || (!delta && (messagesChanged || subagentsChanged))) {
        this.emit({ type: "output.snapshot", threadId: this.threadId ?? undefined, turnId: this.turnId ?? undefined, payload: { stream: "codex", text: output, messages: asJsonValue(rendered.messages), subagents: asJsonValue(rendered.subagents), structureChanged: true, encoding: "utf8" } });
      } else {
        if (delta || subagentsChanged) this.emit({
          type: "output.chunk",
          threadId: this.threadId ?? undefined,
          turnId: this.turnId ?? undefined,
          payload: {
            stream: "codex",
            text: delta,
            // Keep the append-only field for older relays, but include the
            // authoritative projection so a browser can preserve item
            // boundaries while reasoning/commands/edits stream in.
            outputTail: output,
            ...(messagesPatch ? { messagesPatch: asJsonValue(messagesPatch) } : {}),
            subagents: asJsonValue(rendered.subagents),
            structureChanged: messagesChanged || subagentsChanged,
            encoding: "utf8",
          },
        });
      }
      this.renderedOutput = output;
      this.renderedOutputLength = rendered.totalLength;
      this.renderedOutputWasTruncated = rendered.truncated;
      this.renderedMessageShape = messageShape;
      this.renderedSubagentShape = subagentShape;
      this.outputTail = output;
      this.outputMessages = rendered.messages;
      this.subagents = rendered.subagents;
    }

    if (!initial && !previousTurnId && this.turnId) {
      this.emit({ type: "task.started", threadId: this.threadId ?? undefined, turnId: this.turnId, payload: statusPayload(this.status) });
    } else if (!initial && previousTurnId && !this.turnId) {
      const cancelled = new Set(["cancelled", "canceled", "interrupted"]).has(normalizeStatus(nextTurn.status));
      this.emit({ type: cancelled ? "task.cancelled" : "task.finished", threadId: this.threadId ?? undefined, turnId: previousTurnId, payload: statusPayload(this.status) });
    } else if (!initial && !sameStatus(previousStatus, this.status)) {
      // A turn can remain active while moving from reasoning to a command or
      // file edit, and can enter/leave an approval wait without changing its
      // id. Publish a dedicated status event so remote viewers do not have to
      // infer activity from output timing.
      this.emit({ type: "task.status", threadId: this.threadId ?? undefined, turnId: this.turnId ?? undefined, payload: statusPayload(this.status) });
    }

    this.historyComplete = nextHistoryComplete;
    this.renderedMetadataShape = metadataShape;
    if (initial || metadataChanged || historyChanged || previousTurnId !== this.turnId || previousState !== this.state || !sameStatus(previousStatus, this.status) || nextRequests.length !== previousKeys.size || subagentsChanged) {
      void this.emitSnapshot();
    }
  }

  private async loadCompleteHistoryIfNeeded(): Promise<void> {
    if (this.historyLoadRequested || this.historyLoadRetryTimer || this.historyLoadAttempts >= 2 || !this.threadId || !this.ownerClientId || !hasIncompleteHistory(this.conversationState)) return;
    this.clearHistoryLoadRetryTimer();
    // Keep the request target stable. A stream event from another owner can
    // arrive while the owner is loading history; using the mutable fields
    // below would otherwise wait for (or acknowledge) the wrong stream.
    const threadId = this.threadId;
    const ownerClientId = this.ownerClientId;
    const generation = this.historyLoadGeneration;
    this.historyLoadRequested = true;
    this.historyLoadAttempts += 1;
    try {
      const result = await this.client.loadCompleteHistory(threadId, {
        ownerClientId,
        timeoutMs: this.options.followTimeoutMs,
      });
      // The socket may close (or the owner may hand the conversation to a new
      // client) while the request is in flight. Do not install a fresh waiter
      // after dispose/close, where nobody could clear it.
      if (!this.isCurrentHistoryLoad(threadId, ownerClientId, generation)) return;
      const requestedRevision = extractRevision(result);
      // The owner acknowledges the load request before broadcasting its new
      // state. Wait for that revision so the relay's first snapshot contains
      // the complete history rather than the old paginated tail.
      if (requestedRevision !== undefined && (this.revision ?? 0) < requestedRevision) {
        await this.waitForRevision(threadId, ownerClientId, requestedRevision, this.options.followTimeoutMs);
      }
      // A session switch can resolve the old revision waiter while installing
      // a new history request. Never let that old continuation clear or retry
      // the new conversation's loading state.
      if (!this.isCurrentHistoryLoad(threadId, ownerClientId, generation)) return;
      this.historyLoadRequested = false;
      if (this.started
        && this.threadId === threadId
        && this.ownerClientId === ownerClientId
        && hasIncompleteHistory(this.conversationState)
        && this.historyLoadAttempts < 2) {
        void this.loadCompleteHistoryIfNeeded();
      }
    } catch (error) {
      // History loading is an optional read enhancement. The live tail remains
      // usable when an older extension does not implement this request.
      this.options.logger?.debug?.("Unable to load complete Codex history", error);
      if (!this.isCurrentHistoryLoad(threadId, ownerClientId, generation)) return;
      this.historyLoadRequested = false;
      if (isTransientHistoryLoadError(error)
        && this.historyLoadAttempts < 2
        && hasIncompleteHistory(this.conversationState)) {
        this.scheduleHistoryLoadRetry(threadId, ownerClientId, generation);
      }
    }
  }

  private scheduleHistoryLoadRetry(threadId: string, ownerClientId: string, generation: number): void {
    if (this.historyLoadRetryTimer || !this.isCurrentHistoryLoad(threadId, ownerClientId, generation)) return;
    this.historyLoadRetryTimer = setTimeout(() => {
      this.historyLoadRetryTimer = undefined;
      if (!this.isCurrentHistoryLoad(threadId, ownerClientId, generation)
        || this.historyLoadRequested
        || this.historyLoadAttempts >= 2
        || !hasIncompleteHistory(this.conversationState)) return;
      void this.loadCompleteHistoryIfNeeded();
    }, HISTORY_LOAD_RETRY_DELAY_MS);
  }

  private isCurrentHistoryLoad(threadId: string, ownerClientId: string, generation: number): boolean {
    return !this.disposed
      && this.started
      && this.historyLoadGeneration === generation
      && this.threadId === threadId
      && this.ownerClientId === ownerClientId;
  }

  private clearHistoryLoadRetryTimer(): void {
    if (!this.historyLoadRetryTimer) return;
    clearTimeout(this.historyLoadRetryTimer);
    this.historyLoadRetryTimer = undefined;
  }

  private resetHistoryLoading(): void {
    this.clearHistoryLoadRetryTimer();
    this.historyLoadRequested = false;
    this.historyLoadAttempts = 0;
    this.historyLoadGeneration += 1;
  }

  private emitRequest(entry: RequestEntry): void {
    const isInput = INPUT_METHODS.has(entry.method);
    this.emit({
      type: isInput ? "input.requested" : APPROVAL_METHODS.has(entry.method) ? "approval.requested" : "server.requested",
      threadId: entry.threadId ?? this.threadId ?? undefined,
      turnId: entry.turnId,
      requestId: entry.requestId,
      payload: {
        requestId: asJsonValue(entry.requestId),
        method: entry.method,
        params: redactJson(entry.params),
        ...(entry.approval ? {
          action: entry.approval.action,
          risk: entry.approval.risk,
          summary: entry.approval.summary,
          ...(entry.approval.commandHash ? { commandHash: entry.approval.commandHash } : {}),
        } : {}),
        ...(entry.expiresAt ? { expiresAt: entry.expiresAt } : {}),
      },
      raw: redactJson({ requestId: entry.requestId, method: entry.method, params: entry.params }),
    });
  }

  private async emitSnapshot(): Promise<void> {
    const snapshot = await this.snapshot();
    this.emit({ type: "session.snapshot", threadId: snapshot.threadId ?? undefined, turnId: snapshot.turnId ?? undefined, payload: asJsonObject(snapshot) });
  }

  private deriveSessionState(): string {
    const runtime = this.conversationState.threadRuntimeStatus;
    if (isRecord(runtime) && typeof runtime.type === "string") {
      const status = normalizeStatus(runtime.type);
      if (!TERMINAL_TURN_STATES.has(status) && status !== "idle" && status !== "ready") return runtime.type;
    }
    return this.snapshotSeen ? "idle" : "syncing";
  }

  private waitForSnapshot(threadId: string, timeoutMs: number, ownerClientId?: string): Promise<void> {
    this.clearSnapshotWaiter();
    return new Promise<void>((resolve, reject) => {
      const timer = setTimeout(() => {
        if (this.snapshotWaiter?.threadId === threadId
          && this.snapshotWaiter.ownerClientId === ownerClientId) this.snapshotWaiter = undefined;
        reject(new Error(`Timed out waiting for a snapshot from VS Code conversation ${threadId}`));
      }, timeoutMs);
      this.snapshotWaiter = { threadId, ownerClientId, resolve, reject, timer };
    });
  }

  /** Rehydrate the adapter's last owner-validated state after failed navigation. */
  private restoreCachedConversationProjection(
    cached: ConversationStreamState | undefined,
    fallbackOwnerClientId: string | null,
  ): boolean {
    if (!cached || !cached.conversationState) return false;
    this.ownerClientId = cached.ownerClientId || fallbackOwnerClientId;
    this.revision = cached.revision;
    this.conversationState = cloneObject(cached.conversationState);
    this.processConversationState(true);
    this.state = this.deriveSessionState();
    return true;
  }

  private waitForRevision(threadId: string, ownerClientId: string, revision: number, timeoutMs: number): Promise<void> {
    this.clearRevisionWaiter();
    if (this.threadId === threadId && this.ownerClientId === ownerClientId && (this.revision ?? 0) >= revision) return Promise.resolve();
    return new Promise<void>((resolve, reject) => {
      const timer = setTimeout(() => {
        if (this.revisionWaiter?.threadId === threadId
          && this.revisionWaiter.ownerClientId === ownerClientId
          && this.revisionWaiter.revision === revision) this.revisionWaiter = undefined;
        reject(new Error(`Timed out waiting for Codex conversation revision ${revision}`));
      }, timeoutMs);
      this.revisionWaiter = { threadId, ownerClientId, revision, resolve, reject, timer };
    });
  }

  private clearSnapshotWaiter(error?: Error): void {
    const waiter = this.snapshotWaiter;
    if (!waiter) return;
    clearTimeout(waiter.timer);
    this.snapshotWaiter = undefined;
    if (error) waiter.reject(error);
    else waiter.resolve();
  }

  private clearRevisionWaiter(error?: Error): void {
    const waiter = this.revisionWaiter;
    if (!waiter) return;
    clearTimeout(waiter.timer);
    this.revisionWaiter = undefined;
    if (error) waiter.reject(error);
    else waiter.resolve();
  }

  /** Clear all projections that belong to the previous conversation. */
  private resetConversationProjection(): void {
    this.clearRevisionWaiter();
    this.revision = null;
    this.conversationState = {};
    this.turnId = null;
    this.status = {
      activity: "idle",
      turnStatus: "idle",
      activeFlags: [],
      startedAtMs: null,
      durationMs: null,
      workedDurationMs: null,
      elapsedMs: null,
      firstTurnWorkItemStartedAtMs: null,
      finalAssistantStartedAtMs: null,
    };
    this.renderedOutput = "";
    this.renderedOutputLength = 0;
    this.renderedOutputWasTruncated = false;
    this.renderedMessageShape = "";
    this.outputTail = "";
    this.outputMessages = [];
    this.subagents = [];
    this.renderedSubagentShape = "";
    this.renderedMetadataShape = "";
    this.snapshotSeen = false;
    this.historyComplete = undefined;
    this.resetHistoryLoading();
  }

  private handleClose(error?: Error): void {
    if (!this.started || this.disposed) return;
    const closedThreadId = this.threadId;
    const closedOwnerClientId = this.ownerClientId;
    this.sessionLifecycleGeneration += 1;
    this.clearWaitingDiscoveryTimer();
    this.waitingForSession = false;
    this.started = false;
    this.state = "disconnected";
    this.clearPendingVscodeFollow();
    this.vscodeRouteActiveThreadId = null;
    this.vscodeRouteAwaitingSelection = false;
    this.vscodeRouteClientId = null;
    this.vscodeRouteCandidates.clear();
    this.activeVscodeSelection = undefined;
    this.clearSnapshotWaiter(error ?? new Error("Codex IPC socket closed"));
    this.clearRevisionWaiter(error ?? new Error("Codex IPC socket closed"));
    for (const timer of this.pendingTimers.values()) clearTimeout(timer);
    this.pendingTimers.clear();
    this.pendingExpiryAt.clear();
    const pending = [...this.pending.values()];
    this.pending.clear();
    this.optimisticallyResolved.clear();
    // Do not retain a conversation projection after the private IPC owner has
    // gone away. A later snapshot request must report a disconnected, empty
    // follower rather than stale messages that can no longer be controlled.
    this.resetConversationProjection();
    this.threadId = null;
    this.ownerClientId = null;
    this.status = {
      ...this.status,
      activity: "idle",
      turnStatus: "disconnected",
      activeFlags: [],
    };
    for (const entry of pending) {
      this.emit({
        type: entry.approval ? "approval.expired" : "input.expired",
        threadId: entry.threadId ?? closedThreadId ?? undefined,
        turnId: entry.turnId,
        requestId: entry.requestId,
        payload: { requestId: asJsonValue(entry.requestId), reason: error?.message ?? "IPC socket closed" },
      });
    }
    this.emit({
      type: "connection.closed",
      threadId: closedThreadId ?? undefined,
      payload: {
        message: error?.message ?? "IPC socket closed",
        ...(closedOwnerClientId ? { ownerClientId: closedOwnerClientId } : {}),
      },
    });
  }

  private ensureAttached(): void {
    if (!this.started) throw new Error("Codex remote bridge is not attached to a VS Code Codex session");
    if (this.waitingForSession || this.state === "waiting_for_host") {
      throw new Error("waiting_for_session: 请先在 VS Code 打开一个 Codex 会话");
    }
    if (!this.threadId || !this.ownerClientId) throw new Error("No existing Codex conversation owner is attached");
  }

  private ensureStarted(): void {
    if (!this.started) throw new Error("Codex remote bridge is not connected to the VS Code IPC host");
  }

  private ensureInteractiveReady(): void {
    this.ensureAttached();
    if (this.sessionSwitching || this.state === "syncing") throw new Error("Codex session switch is still in progress");
  }

  private emit(event: AgentEvent): void {
    // Every relay event carries the latest typed execution projection. Keep
    // the same values in payload for older consumers that only inspect the
    // untyped event envelope.
    const eventStatus = event.status ?? this.status;
    const normalized: AgentEvent = {
      ...event,
      status: { ...eventStatus, activeFlags: [...eventStatus.activeFlags] },
      payload: { ...statusPayload(eventStatus), ...event.payload },
    };
    for (const listener of this.listeners) {
      try {
        listener(normalized);
      } catch (error) {
        this.options.logger?.warn?.("Codex IPC adapter listener failed", error);
      }
    }
  }

  private toWireResponse(entry: RequestEntry, decision: "allow" | "deny" | "cancel", reason?: string, response?: JsonValue): JsonValue {
    if (entry.method === "item/permissions/requestApproval") {
      const supplied = isRecord(response) ? response : {};
      const requested = isRecord(supplied.permissions) ? supplied.permissions : decision === "allow" && isRecord(entry.params.permissions) ? entry.params.permissions : {};
      const permissions: JsonObject = {};
      for (const [key, value] of Object.entries(requested)) if (value !== null && value !== undefined) permissions[key] = asJsonValue(value);
      return { permissions, scope: supplied.scope === "session" ? "session" : "turn", ...(typeof supplied.strictAutoReview === "boolean" ? { strictAutoReview: supplied.strictAutoReview } : {}) };
    }
    if (entry.method === "item/tool/requestUserInput") {
      return normalizeUserInputResponse(response);
    }
    if (entry.method === "mcpServer/elicitation/request") {
      if (isRecord(response) && typeof response.action === "string") return response;
      return { action: decision === "allow" ? "accept" : decision === "cancel" ? "cancel" : "decline", content: null, _meta: null };
    }
    const suppliedDecision = isRecord(response) && Object.prototype.hasOwnProperty.call(response, "decision") ? response.decision : undefined;
    if (suppliedDecision !== undefined) return normalizeFollowerDecision(entry.method, suppliedDecision, decision, reason);
    if (entry.method === "applyPatchApproval" || entry.method === "execCommandApproval") {
      if (decision === "allow") return "approved";
      if (decision === "cancel") return "abort";
      return { denied: { rejection: reason || "Denied remotely" } };
    }
    return decision === "allow" ? "accept" : decision === "cancel" ? "cancel" : "decline";
  }
}

function normalizeFollowerDecision(method: string, supplied: unknown, fallback: "allow" | "deny" | "cancel", reason?: string): JsonValue {
  // The relay accepts compatibility aliases, while the private follower
  // methods use the app-server's method-specific wire vocabulary.
  if (method === "item/commandExecution/requestApproval" || method === "item/fileChange/requestApproval") {
    if (supplied === "approved" || supplied === "approved_for_session" || supplied === "approved_mcp_policy_amendment") {
      return supplied === "approved" ? "accept" : supplied === "approved_for_session" ? "acceptForSession" : "accept";
    }
    if (supplied === "denied" || supplied === "deny") return "decline";
    if (supplied === "abort") return "cancel";
    return asJsonValue(supplied);
  }
  if (method === "applyPatchApproval" || method === "execCommandApproval") {
    if (supplied === "accept") return "approved";
    if (supplied === "acceptForSession") return "approved_for_session";
    if (supplied === "decline" || supplied === "deny") return { denied: { rejection: reason || "Denied remotely" } };
    if (supplied === "cancel") return "abort";
    return asJsonValue(supplied);
  }
  // If a caller supplied a generic wrapper with no method-specific alias,
  // retain the ordinary fallback selected by the adapter.
  return asJsonValue(supplied ?? (fallback === "allow" ? "accept" : fallback === "cancel" ? "cancel" : "decline"));
}

/** Normalize the official tool-input shape: answers[id].answers is a string array. */
function normalizeUserInputResponse(response: JsonValue | undefined): JsonObject {
  const hasOuterAnswers = isRecord(response) && isRecord(response.answers);
  const source = hasOuterAnswers ? response.answers as Record<string, unknown> : isRecord(response) ? response : {};
  const answers: JsonObject = {};
  for (const [questionId, raw] of Object.entries(source)) {
    if (isRecord(raw) && Array.isArray(raw.answers)) {
      answers[questionId] = { answers: raw.answers.map(asJsonValue) };
    } else if (Array.isArray(raw)) {
      answers[questionId] = { answers: raw.map(asJsonValue) };
    } else if (typeof raw === "string") {
      answers[questionId] = { answers: [raw] };
    } else if (raw !== undefined && raw !== null) {
      answers[questionId] = { answers: [asJsonValue(raw)] };
    } else {
      answers[questionId] = { answers: [] };
    }
  }
  return { answers };
}

interface Candidate {
  id: string;
  mtime: number;
  updatedAtMs?: number;
  cwd?: string;
  title?: string;
  priority: number;
}

async function recentVscodeThreadCandidates(root: string, preferredCwds: string[] = []): Promise<Candidate[]> {
  const files: Array<{ file: string; mtime: number; id: string }> = [];
  async function visit(directory: string, depth: number): Promise<void> {
    if (depth > 3) return;
    let entries;
    try { entries = await fs.readdir(directory, { withFileTypes: true }); } catch { return; }
    await Promise.all(entries.map(async (entry) => {
      const full = path.join(directory, entry.name);
      if (entry.isDirectory()) return visit(full, depth + 1);
      if (!entry.isFile() || !entry.name.endsWith(".jsonl")) return;
      // Codex has used UUIDv7 rollouts (the current hyphenated form), compact
      // UUIDs, and 26-character ULIDs across desktop/VS Code builds. Keep the
      // suffix strict so an arbitrary prompt-like filename cannot become a
      // selectable session.
      const match = entry.name.match(/(?:^|-)((?:[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}|[0-9a-f]{32}|[0-9a-z]{26}))(?:_[^/]*)?\.jsonl$/i);
      if (!match) return;
      try { const stat = await fs.stat(full); files.push({ file: full, mtime: stat.mtimeMs, id: match[1] }); } catch { /* race */ }
    }));
  }
  await visit(root, 0);
  files.sort((a, b) => b.mtime - a.mtime);
  const result: Candidate[] = [];
  // Rollout directories can contain many old sessions. Read metadata from a
  // generous bounded set so an active VS Code session is not hidden merely
  // because desktop history was written more recently.
  for (const file of files.slice(0, 2_000)) {
    try {
      const firstLine = await readFirstLine(file.file);
      let payload: Record<string, unknown> = {};
      try {
        const record = JSON.parse(firstLine) as unknown;
        if (isRecord(record)) {
          // Current rollouts wrap metadata in `payload`; older writers put the
          // same fields on the first record itself. Accept only records that
          // actually contain session identity fields so an arbitrary event
          // record cannot become a false discovery candidate.
          const candidate = isRecord(record.payload) ? record.payload : record;
          if (["originator", "source", "thread_source", "cwd"].some((key) => Object.prototype.hasOwnProperty.call(candidate, key))) {
            payload = candidate;
          }
        }
      } catch {
        // Incomplete rollout records are ignored below. A regex fallback is
        // intentionally omitted so text inside a long prompt cannot identify
        // an unrelated session as a VS Code conversation.
      }
      const originator = stringValue(payload.originator);
      const source = stringValue(payload.source);
      const threadSource = stringValue(payload.thread_source);
      const cwd = stringValue(payload.cwd);
      const title = stringValue(payload.title)
        ?? stringValue(payload.thread_title)
        ?? stringValue(payload.threadTitle)
        ?? stringValue(payload.name)
        ?? stringValue(payload.thread_name);
      const updatedAtMs = timestampMs(payload.updated_at_ms)
        ?? timestampMs(payload.updatedAtMs)
        ?? timestampMs(payload.updated_at)
        ?? timestampMs(payload.updatedAt)
        ?? timestampMs(payload.timestamp);
      // Subagent rollouts can use the same `codex_vscode` originator as their
      // user-facing parent, but they are not conversations the operator opened.
      if (threadSource === "subagent") continue;
      // `source: vscode` is also written by Codex Desktop tasks hosted from a
      // VS Code-shaped workspace. An explicit non-VS-Code originator must win
      // so attach mode never follows a desktop task merely because it shares
      // the same IPC router. Keep compatibility with older official rollouts
      // that omitted originator altogether.
      const official = originator === "codex_vscode";
      const unattributedVscode = !originator && (source === "vscode" || threadSource === "vscode");
      const isVscode = official || unattributedVscode;
      if (!isVscode) continue;
      // Never select a rollout produced by this bridge's legacy spawn mode.
      if (originator === "codex-remote-collab") continue;
      const cwdMatch = Boolean(cwd && preferredCwds.some((candidate) => samePath(candidate, cwd)));
      const id = file.id;
      // The bridge runs inside a VS Code workspace, so an owner in that
      // workspace is a stronger signal than the rollout writer's originator.
      const priority = cwdMatch ? (official ? 0 : 1) : (official ? 2 : 3);
      if (!result.some((candidate) => candidate.id === id)) {
        result.push({
          id,
          mtime: file.mtime,
          ...(updatedAtMs !== undefined ? { updatedAtMs } : {}),
          ...(cwd ? { cwd } : {}),
          ...(title ? { title } : {}),
          priority,
        });
      }
    } catch { /* ignore incomplete/rotated rollout files */ }
  }
  result.sort((a, b) => a.priority - b.priority || b.mtime - a.mtime);
  return result;
}

/** Read enough of a rollout file to reach its first JSONL record, bounded. */
async function readFirstLine(fileName: string, maxBytes = 4 * 1024 * 1024): Promise<string> {
  const handle = await fs.open(fileName, "r");
  const parts: Buffer[] = [];
  let offset = 0;
  try {
    while (offset < maxBytes) {
      const size = Math.min(64 * 1024, maxBytes - offset);
      const chunk = Buffer.alloc(size);
      const read = await handle.read(chunk, 0, size, offset);
      if (!read.bytesRead) break;
      const piece = chunk.subarray(0, read.bytesRead);
      const newline = piece.indexOf(0x0a);
      if (newline >= 0) {
        parts.push(piece.subarray(0, newline));
        return Buffer.concat(parts).toString("utf8");
      }
      parts.push(piece);
      offset += read.bytesRead;
      if (read.bytesRead < size) break;
    }
    return Buffer.concat(parts).toString("utf8");
  } finally {
    await handle.close();
  }
}

interface SessionIndexEntry {
  title?: string;
  updatedAtMs?: number;
  cwd?: string;
}

/** Read the bounded local index used by the official Codex recent-chat list. */
async function readSessionIndex(fileName: string, maxBytes = 8 * 1024 * 1024): Promise<Map<string, SessionIndexEntry>> {
  let raw: string;
  try {
    raw = await readBoundedText(fileName, maxBytes);
  } catch {
    return new Map();
  }
  const result = new Map<string, SessionIndexEntry>();
  for (const line of raw.split(/\r?\n/)) {
    if (!line.trim()) continue;
    try {
      const record = JSON.parse(line) as unknown;
      if (!isRecord(record)) continue;
      const id = stringValue(record.id) ?? stringValue(record.session_id) ?? stringValue(record.thread_id);
      if (!id) continue;
      const title = stringValue(record.thread_name)
        ?? stringValue(record.title)
        ?? stringValue(record.name)
        ?? stringValue(record.preview);
      const updatedAtMs = timestampMs(record.updated_at_ms)
        ?? timestampMs(record.updatedAtMs)
        ?? timestampMs(record.updated_at)
        ?? timestampMs(record.updatedAt)
        ?? timestampMs(record.last_updated_at);
      const cwd = stringValue(record.cwd) ?? stringValue(record.workspace) ?? stringValue(record.workspacePath);
      result.set(id, {
        ...(title ? { title } : {}),
        ...(updatedAtMs !== undefined ? { updatedAtMs } : {}),
        ...(cwd ? { cwd } : {}),
      });
    } catch {
      // A partially-written last line must not make the rest of the index
      // unavailable.
    }
  }
  return result;
}

async function readBoundedText(fileName: string, maxBytes: number): Promise<string> {
  const handle = await fs.open(fileName, "r");
  const parts: Buffer[] = [];
  let offset = 0;
  try {
    while (offset < maxBytes) {
      const size = Math.min(64 * 1024, maxBytes - offset);
      const chunk = Buffer.alloc(size);
      const read = await handle.read(chunk, 0, size, offset);
      if (!read.bytesRead) break;
      parts.push(chunk.subarray(0, read.bytesRead));
      offset += read.bytesRead;
      if (read.bytesRead < size) break;
    }
    return Buffer.concat(parts).toString("utf8");
  } finally {
    await handle.close();
  }
}

function sanitizeSessionTitle(value: string | undefined): string | undefined {
  if (!value) return undefined;
  const title = redactText(value).replace(/\s+/g, " ").trim().slice(0, 240);
  return title || undefined;
}

function samePath(left: string, right: string): boolean {
  try { return path.resolve(left) === path.resolve(right); } catch { return left === right; }
}


function resolveCodexHome(options: CodexIpcAgentAdapterOptions): string {
  const env = options.env ?? process.env;
  const configured = options.codexHome?.trim() || env.CODEX_HOME?.trim() || path.join(options.homeDir ?? os.homedir(), ".codex");
  if (configured === "~") return options.homeDir ?? os.homedir();
  if (configured.startsWith("~/")) return path.join(options.homeDir ?? os.homedir(), configured.slice(2));
  return configured;
}

function isMissingSessionOwnerError(error: unknown): boolean {
  const message = error instanceof Error ? error.message : String(error);
  return /找不到会话\s+.+\s+的 VS Code Codex owner/.test(message)
    || /no existing codex conversation owner is attached/i.test(message);
}

function extractInput(params: JsonObject): string | JsonValue[] {
  if (typeof params.text === "string") return params.text;
  if (typeof params.message === "string") return params.message;
  if (typeof params.prompt === "string") return params.prompt;
  if (typeof params.input === "string") return params.input;
  if (Array.isArray(params.input) && params.input.length) return params.input;
  throw new Error("turn request requires text or a non-empty input array");
}

function pickTurnRequest(params: JsonObject): JsonObject {
  const request: JsonObject = {};
  if (Array.isArray(params.attachments)) request.attachments = asJsonValue(params.attachments);
  // The owner inherits all existing thread settings. Only forward fields that
  // are part of the app-server turn/start request, never relay UI-only keys.
  for (const key of ["model", "serviceTier", "effort", "summary", "personality", "collaborationMode", "approvalPolicy", "approvalsReviewer", "permissions", "sandboxPolicy", "runtimeWorkspaceRoots", "cwd", "outputSchema", "multiAgentMode"]) {
    if (params[key] !== undefined) request[key] = asJsonValue(params[key]);
  }
  return request;
}

/**
 * The model picker changes durable next-turn settings, not one turn request.
 * Accept the relay-friendly flat form and the official nested form while
 * forwarding only the fields required by the picker.
 */
function pickThreadSettingsUpdate(params: JsonObject): JsonObject {
  const source = isRecord(params.threadSettings) ? params.threadSettings : params;
  const settings: JsonObject = {};

  if (Object.prototype.hasOwnProperty.call(source, "model")) {
    if (typeof source.model !== "string" || !source.model.trim()) {
      throw new Error("thread settings model must be a non-empty string");
    }
    settings.model = source.model.trim();
  }

  if (Object.prototype.hasOwnProperty.call(source, "effort")) {
    if (source.effort !== null && (typeof source.effort !== "string" || !source.effort.trim())) {
      throw new Error("thread settings effort must be a non-empty string or null");
    }
    settings.effort = typeof source.effort === "string" ? source.effort.trim() : null;
  }

  if (Object.prototype.hasOwnProperty.call(source, "multiAgentMode")) {
    if (source.multiAgentMode !== null && typeof source.multiAgentMode !== "string") {
      throw new Error("thread settings multiAgentMode must be a string or null");
    }
    settings.multiAgentMode = asJsonValue(source.multiAgentMode);
  }

  // The official permissions control updates the same durable thread
  // settings envelope as the model picker. Keep the projection explicit so a
  // browser cannot smuggle arbitrary UI state into the owner request, while
  // retaining object-valued policies used by newer app-server builds.
  for (const key of ["sandboxPolicy", "approvalPolicy"] as const) {
    if (!Object.prototype.hasOwnProperty.call(source, key)) continue;
    const value = source[key];
    if (value !== null && typeof value !== "string" && !isRecord(value)) {
      throw new Error(`thread settings ${key} must be a string, object, or null`);
    }
    settings[key] = asJsonValue(value);
  }
  if (Object.prototype.hasOwnProperty.call(source, "approvalsReviewer")) {
    const value = source.approvalsReviewer;
    if (value !== null && typeof value !== "string") {
      throw new Error("thread settings approvalsReviewer must be a string or null");
    }
    settings.approvalsReviewer = asJsonValue(value);
  }
  if (Object.prototype.hasOwnProperty.call(source, "runtimeWorkspaceRoots")) {
    const value = source.runtimeWorkspaceRoots;
    if (value !== null && (!Array.isArray(value) || !value.every((entry) => typeof entry === "string"))) {
      throw new Error("thread settings runtimeWorkspaceRoots must be an array of strings or null");
    }
    settings.runtimeWorkspaceRoots = asJsonValue(value);
  }
  if (Object.prototype.hasOwnProperty.call(source, "permissions")) {
    const value = source.permissions;
    if (value !== null && typeof value !== "string" && !isRecord(value)) {
      throw new Error("thread settings permissions must be a string, object, or null");
    }
    settings.permissions = asJsonValue(value);
  }

  if (Object.keys(settings).length === 0) {
    throw new Error("thread settings update requires model, effort, multiAgentMode, sandboxPolicy, approvalPolicy, permissions, or approvalsReviewer");
  }
  return settings;
}

function pickTurnContext(params: JsonObject): JsonObject {
  const context = isRecord(params.context) ? asJsonObject(params.context) : {};
  if (context.inheritThreadSettings === undefined) context.inheritThreadSettings = true;
  if (Array.isArray(params.commentAttachments)) context.commentAttachments = asJsonValue(params.commentAttachments);
  if (Array.isArray(params.mcpAppModelContextAttachments)) context.mcpAppModelContextAttachments = asJsonValue(params.mcpAppModelContextAttachments);
  return context;
}

function unwrapFollowerResult(value: JsonValue | undefined): JsonValue {
  if (isRecord(value) && Object.prototype.hasOwnProperty.call(value, "result")) return asJsonValue(value.result);
  return asJsonValue(value);
}

function extractRevision(value: unknown): number | undefined {
  const direct = numberValue(value);
  if (direct !== undefined) return direct;
  if (!isRecord(value)) return undefined;
  for (const key of ["revision", "streamRevision", "stateRevision"]) {
    const revision = numberValue(value[key]);
    if (revision !== undefined) return revision;
  }
  return Object.prototype.hasOwnProperty.call(value, "result") ? extractRevision(value.result) : undefined;
}

function extractTurnId(value: unknown): string | undefined {
  if (!isRecord(value)) return undefined;
  if (typeof value.turnId === "string") return value.turnId;
  if (isRecord(value.turn) && typeof value.turn.id === "string") return value.turn.id;
  if (isRecord(value.result)) return extractTurnId(value.result);
  return undefined;
}

function deriveTurn(state: JsonObject): TurnInfo {
  const candidates: TurnInfo[] = [];
  const turns = Array.isArray(state.turns) ? state.turns : [];
  for (const value of turns) if (isRecord(value)) candidates.push(turnInfo(value));
  const history = isRecord(state.turnHistory) && isRecord(state.turnHistory.history) && isRecord(state.turnHistory.history.entitiesByKey)
    ? Object.values(state.turnHistory.history.entitiesByKey) : [];
  for (const value of history) if (isRecord(value) && (value.turnId !== undefined || value.status !== undefined || value.items !== undefined)) candidates.push(turnInfo(value));
  const runtime = isRecord(state.threadRuntimeStatus) ? state.threadRuntimeStatus : undefined;
  const runtimeType = normalizeStatus(runtime?.type);
  const active = candidates.filter((entry) => entry.active).sort((a, b) => (b.startedAt ?? 0) - (a.startedAt ?? 0));
  if (active[0]) return active[0];
  const selected = candidates.sort((a, b) => (b.startedAt ?? 0) - (a.startedAt ?? 0))[0];
  if (runtime && runtimeType && runtimeType !== "idle" && runtimeType !== "ready" && typeof runtime.turnId === "string") {
    return { id: runtime.turnId, status: runtimeType, active: true };
  }
  return selected ?? { status: runtimeType || "idle", active: false };
}

function turnInfo(value: Record<string, unknown>): TurnInfo {
  const id = typeof value.id === "string" ? value.id : typeof value.turnId === "string" ? value.turnId : undefined;
  const status = normalizeStatus(typeof value.status === "string" ? value.status : isRecord(value.status) && typeof value.status.type === "string" ? value.status.type : "unknown");
  const startedAt = timestampMs(value.turnStartedAtMs)
    ?? timestampMs(value.startedAtMs)
    ?? timestampMs(value.startedAt)
    ?? timestampMs(value.createdAtMs)
    ?? timestampMs(value.createdAt);
  const durationMs = numberValue(value.durationMs) ?? numberValue(value.duration);
  const completedAtMs = timestampMs(value.completedAtMs) ?? timestampMs(value.completedAt);
  const commandStarts = isRecord(value.commandExecutionStartedAtMsById)
    ? value.commandExecutionStartedAtMsById
    : {};
  const items = Array.isArray(value.items) ? value.items.filter(isRecord) : [];
  const firstTurnWorkItemStartedAtMs = timestampMs(value.firstTurnWorkItemStartedAtMs)
    ?? timestampMs(value.firstWorkItemStartedAtMs)
    ?? timestampMs(value.firstTurnWorkItemStartedAt)
    ?? inferFirstWorkItemStartedAtMs(items, commandStarts);
  const finalAssistantStartedAtMs = timestampMs(value.finalAssistantStartedAtMs)
    ?? timestampMs(value.finalAssistantStartedAt)
    ?? inferFinalAssistantStartedAtMs(items, commandStarts);
  const active = !TERMINAL_TURN_STATES.has(status) && status !== "idle" && status !== "unknown";
  const explicitWorkedDurationMs = numberValue(value.workedDurationMs)
    ?? numberValue(value.workDurationMs)
    ?? numberValue(value.workDuration);
  const workedCompletedAtMs = finalAssistantStartedAtMs
    ?? (!active && completedAtMs !== undefined ? completedAtMs : undefined);
  const workedDurationMs = explicitWorkedDurationMs
    ?? (firstTurnWorkItemStartedAtMs !== undefined && workedCompletedAtMs !== undefined
      ? Math.max(0, workedCompletedAtMs - firstTurnWorkItemStartedAtMs)
      : undefined);
  return {
    id,
    status,
    active,
    ...(startedAt !== undefined ? { startedAt } : {}),
    ...(durationMs !== undefined ? { durationMs } : {}),
    ...(workedDurationMs !== undefined ? { workedDurationMs } : {}),
    ...(completedAtMs !== undefined ? { completedAtMs } : {}),
    ...(firstTurnWorkItemStartedAtMs !== undefined ? { firstTurnWorkItemStartedAtMs } : {}),
    ...(finalAssistantStartedAtMs !== undefined ? { finalAssistantStartedAtMs } : {}),
    ...(value.error !== undefined ? { error: asJsonValue(value.error) } : {}),
    raw: value,
  };
}

/**
 * Infer the timestamps used by the official "worked for" row when an older
 * conversation snapshot does not carry the denormalized turn fields. Persisted
 * rollout records use snake_case command metadata, so both spellings are
 * intentionally accepted here.
 */
function inferFirstWorkItemStartedAtMs(
  items: Record<string, unknown>[],
  commandStarts: Record<string, unknown>,
): number | undefined {
  for (const item of items) {
    if (isNonWorkItem(item)) continue;
    const id = stringValue(item.id);
    const timestamp = itemTimestamp(item)
      ?? (id ? timestampMs(commandStarts[id]) : undefined);
    if (timestamp !== undefined) return timestamp;
  }
  return undefined;
}

function inferFinalAssistantStartedAtMs(
  items: Record<string, unknown>[],
  commandStarts: Record<string, unknown>,
): number | undefined {
  let fallback: number | undefined;
  for (const item of items) {
    if (!isAssistantItem(item)) continue;
    const id = stringValue(item.id);
    const timestamp = itemTimestamp(item)
      ?? (id ? timestampMs(commandStarts[id]) : undefined);
    if (timestamp === undefined) continue;
    fallback = timestamp;
    const phase = normalizeStatus(item.phase);
    if (phase === "final_answer" || phase === "finalanswer") return timestamp;
  }
  return fallback;
}

function itemTimestamp(item: Record<string, unknown>): number | undefined {
  return timestampMs(item.startedAtMs)
    ?? timestampMs(item.started_at_ms)
    ?? timestampMs(item.startedAt)
    ?? timestampMs(item.started_at)
    ?? timestampMs(item.createdAtMs)
    ?? timestampMs(item.created_at_ms)
    ?? timestampMs(item.createdAt)
    ?? timestampMs(item.created_at);
}

function normalizedItemType(item: Record<string, unknown>): string {
  return String(item.type ?? item.kind ?? "").replace(/[\s/_-]+/g, "").toLowerCase();
}

function isAssistantItem(item: Record<string, unknown>): boolean {
  const type = normalizedItemType(item);
  return type === "agentmessage" || type === "assistantmessage";
}

function isNonWorkItem(item: Record<string, unknown>): boolean {
  const type = normalizedItemType(item);
  return type === "usermessage"
    || type === "steeringusermessage"
    || type === "realtimetranscript"
    || type === "worktreeinit"
    || type === "sleep";
}

/**
 * Normalize the official turn/runtime records into a stable status shape for
 * relay clients. This intentionally tolerates both raw webview state
 * (`turnStartedAtMs`, `inProgress`) and serialized history summaries
 * (`startedAt`, `completed`) used by different extension versions.
 */
function deriveStatusSnapshot(state: JsonObject, turn: TurnInfo, requests: RequestEntry[]): AgentStatusSnapshot {
  const runtime = isRecord(state.threadRuntimeStatus) ? state.threadRuntimeStatus : undefined;
  const activeFlags = Array.isArray(runtime?.activeFlags)
    ? runtime.activeFlags.filter((flag): flag is string => typeof flag === "string")
    : [];
  const activity = classifyActivity(turn, activeFlags, requests, runtime);
  const startedAtMs = turn.startedAt ?? null;
  const durationMs = turn.durationMs
    ?? (!turn.active && turn.completedAtMs !== undefined && turn.completedAtMs !== null && turn.startedAt !== undefined
      ? Math.max(0, turn.completedAtMs - turn.startedAt)
      : null);
  // The official worked-for indicator starts at the first actual work item,
  // not at the user-message/turn start, and ends when the final assistant
  // message begins. Keep this separate from the broader turn duration.
  const workStartedAtMs = turn.firstTurnWorkItemStartedAtMs ?? turn.startedAt ?? undefined;
  const workedCompletedAtMs = turn.finalAssistantStartedAtMs
    ?? (!turn.active ? turn.completedAtMs : undefined)
    ?? undefined;
  const workedDurationMs = turn.workedDurationMs
    ?? (workStartedAtMs !== undefined && workedCompletedAtMs !== undefined
      ? Math.max(0, workedCompletedAtMs - workStartedAtMs)
      : turn.active && workStartedAtMs !== undefined
        ? Math.max(0, Date.now() - workStartedAtMs)
        : null);
  const elapsedMs = turn.active && workStartedAtMs !== undefined
    ? Math.max(0, Date.now() - workStartedAtMs)
    : durationMs;
  const status: AgentStatusSnapshot = {
    activity,
    turnStatus: turn.status,
    activeFlags,
    startedAtMs,
    durationMs,
    workedDurationMs,
    elapsedMs,
    firstTurnWorkItemStartedAtMs: turn.firstTurnWorkItemStartedAtMs ?? null,
    finalAssistantStartedAtMs: turn.finalAssistantStartedAtMs ?? null,
    ...(turn.error !== undefined ? { error: turn.error } : {}),
  };
  return status;
}

function classifyActivity(
  turn: TurnInfo,
  activeFlags: string[],
  requests: RequestEntry[],
  runtime?: Record<string, unknown>,
): string {
  const flags = new Set(activeFlags.map((flag) => normalizeStatus(flag)));
  if (flags.has("waiting_on_approval") || flags.has("waiting_for_approval") || flags.has("waitingonapproval") || requests.some((entry) => Boolean(entry.approval))) {
    return "waiting_approval";
  }
  if (flags.has("waiting_on_user_input") || flags.has("waiting_for_user_input") || flags.has("waitingonuserinput") || requests.some((entry) => INPUT_METHODS.has(entry.method))) {
    return "waiting_input";
  }
  if (!turn.active) return terminalActivity(turn.status);

  const item = latestActiveWorkItem(turn.raw);
  if (item) {
    const type = normalizeStatus(item.type ?? item.kind ?? "");
    if (type === "read" || type.includes("fileread") || type.includes("readfile") || readPathsFromActions(commandActionsValue(item)).length > 0) return "reading";
    if (type.includes("filechange") || type.includes("file_change") || type.includes("patch") || type.includes("edit")) return "editing";
    if (type.includes("reasoning") || type.includes("think")) return "thinking";
    if (type.includes("command") || type.includes("exec") || type.includes("process") || type.includes("tool")) return "running";
    const phase = normalizeStatus(item.phase);
    if (phase.includes("reason") || phase.includes("think")) return "thinking";
  }

  const runtimeType = normalizeStatus(runtime?.type);
  if (runtimeType.includes("reason") || runtimeType.includes("think")) return "thinking";
  if (runtimeType.includes("edit") || runtimeType.includes("patch")) return "editing";
  return "running";
}

function latestActiveWorkItem(turn?: Record<string, unknown>): Record<string, unknown> | undefined {
  if (!turn || !Array.isArray(turn.items)) return undefined;
  for (let index = turn.items.length - 1; index >= 0; index -= 1) {
    const item = turn.items[index];
    if (!isRecord(item)) continue;
    const status = normalizeStatus(isRecord(item.status) ? item.status.type : item.status);
    if (status && status !== "unknown" && TERMINAL_TURN_STATES.has(status)) continue;
    return item;
  }
  return undefined;
}

function terminalActivity(status: string): string {
  const normalized = normalizeStatus(status);
  if (normalized === "failed" || normalized === "error") return "failed";
  if (normalized === "cancelled" || normalized === "canceled" || normalized === "interrupted") return "interrupted";
  if (normalized === "completed" || normalized === "complete" || normalized === "done") return "completed";
  return normalized === "idle" || normalized === "ready" ? "idle" : "idle";
}

function statusPayload(status: AgentStatusSnapshot): JsonObject {
  return {
    // `status` is the legacy scalar alias; `turnStatus` retains the explicit
    // name so clients can distinguish it from the coarse `activity` value.
    status: status.turnStatus,
    turnStatus: status.turnStatus,
    activity: status.activity,
    activeFlags: asJsonValue(status.activeFlags),
    startedAtMs: status.startedAtMs ?? null,
    durationMs: status.durationMs ?? null,
    workedDurationMs: status.workedDurationMs ?? null,
    elapsedMs: status.elapsedMs ?? null,
    firstTurnWorkItemStartedAtMs: status.firstTurnWorkItemStartedAtMs ?? null,
    finalAssistantStartedAtMs: status.finalAssistantStartedAtMs ?? null,
    ...(status.error !== undefined ? { error: status.error } : {}),
  };
}

function sameStatus(a: AgentStatusSnapshot, b: AgentStatusSnapshot): boolean {
  return a.activity === b.activity
    && a.turnStatus === b.turnStatus
    && JSON.stringify(a.activeFlags) === JSON.stringify(b.activeFlags)
    && a.startedAtMs === b.startedAtMs
    && a.durationMs === b.durationMs
    && a.workedDurationMs === b.workedDurationMs
    && a.firstTurnWorkItemStartedAtMs === b.firstTurnWorkItemStartedAtMs
    && a.finalAssistantStartedAtMs === b.finalAssistantStartedAtMs
    && JSON.stringify(a.error) === JSON.stringify(b.error);
}

function timestampMs(value: unknown): number | undefined {
  if (typeof value === "string") {
    const trimmed = value.trim();
    if (/^[+-]?(?:\d+(?:\.\d*)?|\.\d+)$/.test(trimmed)) {
      const number = Number(trimmed);
      if (Number.isFinite(number)) {
        return number > 0 && number < 1_000_000_000_000 ? number * 1000 : number;
      }
    }
    const parsed = Date.parse(value);
    return Number.isFinite(parsed) ? parsed : undefined;
  }
  const number = numberValue(value);
  if (number === undefined) return undefined;
  // Serialized history in older extension builds uses epoch seconds while
  // the live turn fields end in `AtMs`. Normalize both to milliseconds.
  return number > 0 && number < 1_000_000_000_000 ? number * 1000 : number;
}

function extractRequests(state: JsonObject, approvalTimeoutMs: number): RequestEntry[] {
  const values: Array<{ id?: JsonRpcId; value: Record<string, unknown> }> = [];
  const seenRecords = new Set<Record<string, unknown>>();
  const collect = (raw: unknown, hintedId?: JsonRpcId, depth = 0): void => {
    if (depth > 6 || raw === null || raw === undefined) return;
    if (Array.isArray(raw)) {
      for (const value of raw) collect(value, undefined, depth + 1);
      return;
    }
    if (!isRecord(raw)) return;
    if (seenRecords.has(raw)) return;
    seenRecords.add(raw);
    const nested = isRecord(raw.request) ? raw.request : raw;
    const method = requestMethodOf(nested) ?? requestMethodOf(raw);
    const requestId = requestIdOf(nested) ?? requestIdOf(raw) ?? hintedId;
    if (method && isJsonRpcId(requestId)) values.push({ id: requestId, value: raw });
    // Official conversation snapshots can retain pending records inside a
    // turn item (permission-request/userInput/mcp-server-elicitation), while
    // older builds expose the same records under `requests`. Traverse only
    // protocol containers so arbitrary message content is never interpreted as
    // an approval request.
    for (const key of ["requests", "pendingRequests", "pendingApprovals", "turns", "turnHistory", "history", "items", "request"]) {
      const child = raw[key];
      if (child === undefined) continue;
      if (isRecord(child) && !Array.isArray(child)) {
        for (const [childKey, value] of Object.entries(child)) {
          collect(value, isJsonRpcId(childKey) ? childKey : undefined, depth + 1);
        }
      } else collect(child, undefined, depth + 1);
    }
  };
  collect(state);
  const result: RequestEntry[] = [];
  const seenRequests = new Set<string>();
  for (const item of values) {
    const request = isRecord(item.value.request) ? item.value.request : item.value;
    const requestId = item.id ?? requestIdOf(request);
    const method = requestMethodOf(request) ?? requestMethodOf(item.value);
    if (!isJsonRpcId(requestId) || !method) continue;
    const dedupeKey = `${jsonRpcIdKey(requestId)}\u001f${method}`;
    if (seenRequests.has(dedupeKey)) continue;
    seenRequests.add(dedupeKey);
    const params = asJsonObject(request.params ?? item.value.params ?? (request === item.value ? item.value : {}));
    // `params.startedAtMs` is the app-server's authoritative timestamp. The
    // outer fields are compatibility fallbacks for older normalized snapshots
    // and can represent when a UI record was inserted rather than when the
    // approval actually started.
    const createdAt = timestampMs(params.startedAtMs)
      ?? timestampMs(params.started_at_ms)
      ?? timestampMs(params.startedAt)
      ?? timestampMs(params.started_at)
      ?? timestampMs(request.startedAtMs)
      ?? timestampMs(request.started_at_ms)
      ?? timestampMs(request.startedAt)
      ?? timestampMs(request.started_at)
      ?? timestampMs(item.value.startedAtMs)
      ?? timestampMs(item.value.started_at_ms)
      ?? timestampMs(item.value.startedAt)
      ?? timestampMs(item.value.started_at)
      ?? timestampMs(item.value.createdAtMs)
      ?? timestampMs(item.value.created_at_ms)
      ?? timestampMs(item.value.createdAt)
      ?? timestampMs(item.value.created_at)
      ?? Date.now();
    const expiresAt = timestampMs(item.value.expiresAtMs)
      ?? timestampMs(item.value.expires_at_ms)
      ?? timestampMs(item.value.expiresAt)
      ?? timestampMs(item.value.expires_at)
      ?? timestampMs(request.expiresAtMs)
      ?? timestampMs(request.expires_at_ms)
      ?? timestampMs(request.expiresAt)
      ?? timestampMs(request.expires_at)
      ?? timestampMs(params.expiresAtMs)
      ?? timestampMs(params.expires_at_ms)
      ?? timestampMs(params.expiresAt)
      ?? timestampMs(params.expires_at)
      ?? (approvalTimeoutMs > 0 ? createdAt + approvalTimeoutMs : undefined);
    const entry: RequestEntry = {
      requestId,
      method,
      params,
      threadId: stringValue(params.threadId) ?? stringValue(params.conversationId),
      turnId: stringValue(params.turnId),
      createdAt,
      ...(expiresAt ? { expiresAt } : {}),
    };
    if (APPROVAL_METHODS.has(method)) entry.approval = toPendingApproval(entry);
    result.push(entry);
  }
  return result;
}

function requestMethodOf(value: Record<string, unknown>): string | undefined {
  if (typeof value.method === "string" && value.method) return value.method;
  const type = String(value.type ?? value.kind ?? "").replace(/[\s/_-]+/g, "").toLowerCase();
  if (type.includes("permissionrequest")) return "item/permissions/requestApproval";
  if (type.includes("commandexecutionrequest") || type === "execapproval") return "item/commandExecution/requestApproval";
  if (type.includes("filechangerequest") || type === "patchapproval") return "item/fileChange/requestApproval";
  if (type === "exec" && value.approvalRequestId !== undefined && (!isRecord(value.output) || value.output.exitCode === undefined)) return "execCommandApproval";
  if (type === "patch" && value.approvalRequestId !== undefined && value.success === undefined) return "applyPatchApproval";
  if (type.includes("userinput") && value.completed !== true) return "item/tool/requestUserInput";
  if (type.includes("mcpserverelicitation") && value.completed !== true) return "mcpServer/elicitation/request";
  return undefined;
}

function requestIdOf(value: Record<string, unknown>): JsonRpcId | undefined {
  const id = value.requestId ?? value.id;
  return isJsonRpcId(id) ? id : undefined;
}

function toPendingApproval(entry: RequestEntry): PendingApproval {
  const command = typeof entry.params.command === "string" ? entry.params.command : extractCommand(entry.params);
  const action = entry.method.includes("fileChange") || entry.method === "applyPatchApproval" ? "file.change" : entry.method.includes("permissions") ? "permissions.grant" : "command.execution";
  const risk: PendingApproval["risk"] = entry.method.includes("permissions")
    ? "high"
    : entry.method.includes("command") || entry.method === "execCommandApproval"
      ? (!command || /(?:rm\s+-rf|sudo|curl|wget|ssh|password|token|secret)/i.test(command) ? "high" : "medium")
      : "medium";
  const summary = stringValue(entry.params.reason) ?? command ?? `${action} requested by Codex`;
  return {
    requestId: entry.requestId,
    method: entry.method,
    threadId: entry.threadId,
    turnId: entry.turnId,
    itemId: stringValue(entry.params.itemId) ?? stringValue(entry.params.callId),
    action,
    risk,
    summary: redactText(summary),
    commandHash: hashJson(entry.params),
    createdAt: entry.createdAt,
    ...(entry.expiresAt ? { expiresAt: entry.expiresAt } : {}),
    payload: redactJson(entry.params) as JsonObject,
  };
}

function extractCommand(params: JsonObject): string | undefined {
  if (Array.isArray(params.command)) return params.command.filter((value): value is string => typeof value === "string").join(" ");
  const actions = params.commandActions ?? params.command_actions ?? params.parsedCmd ?? params.parsed_cmd;
  const actionList = commandActionList(actions);
  if (actionList.length) {
    const commands = actionList.map(commandActionText).filter((value): value is string => Boolean(value));
    return commands.length ? commands.join(" && ") : undefined;
  }
  return undefined;
}

interface RenderedConversationMessage {
  id?: string;
  turnId?: string;
  itemId?: string;
  role: "user" | "assistant" | "reasoning" | "tool" | "error";
  kind: "user" | "assistant" | "reasoning" | "plan" | "tool" | "edit" | "error";
  text: string;
  label?: string;
  itemType?: string;
  status?: string;
  turnStatus?: string;
  startedAtMs?: number;
  completedAtMs?: number;
  durationMs?: number;
  /** Duration of the official worked-for activity group for this turn. */
  workedDurationMs?: number;
  command?: string;
  /** Parsed command actions emitted by the official command renderer. */
  commandActions?: JsonValue[];
  cwd?: string | null;
  shellName?: string | null;
  exitCode?: number;
  phase?: string;
  breaksPreviousAdjacency?: boolean;
  /** Official collabAgentToolCall projection. */
  action?: string;
  senderThreadId?: string;
  receiverThreadIds?: string[];
  /** Official webview compatibility alias. */
  receiverThreads?: string[];
  prompt?: string | null;
  model?: string | null;
  reasoningEffort?: string | null;
  agentsStates?: JsonObject;
  /** Official subAgentActivity projection. */
  agentThreadId?: string;
  agentPath?: string;
  displayName?: string | null;
  displayStatus?: string;
  activityKind?: string;
  /** Semantic name used by the official webview converter. */
  uiType?: string;
  /** Friendly paths extracted from a parsed `read` command action. */
  readPaths?: string[];
  /** Raw tool/file output kept separate from the compact activity summary. */
  output?: string;
}

interface ItemDisplayProjection {
  outputText: string;
  projectionId?: string;
  startedAtMs?: number;
  completedAtMs?: number;
  durationMs?: number;
  workedDurationMs?: number;
  role: RenderedConversationMessage["role"];
  kind: RenderedConversationMessage["kind"];
  text: string;
  label?: string;
  itemType?: string;
  status?: string;
  command?: string;
  commandActions?: JsonValue[];
  cwd?: string | null;
  shellName?: string | null;
  action?: string;
  senderThreadId?: string;
  receiverThreadIds?: string[];
  receiverThreads?: string[];
  prompt?: string | null;
  model?: string | null;
  reasoningEffort?: string | null;
  agentsStates?: JsonObject;
  agentThreadId?: string;
  agentPath?: string;
  displayName?: string | null;
  displayStatus?: string;
  activityKind?: string;
  uiType?: string;
  readPaths?: string[];
  output?: string;
}

function renderedMessageShape(messages: RenderedConversationMessage[]): string {
  return messages.map((message, index) => [
    message.id ?? `index:${index}`,
    message.turnId ?? "",
    message.itemId ?? "",
    message.role,
    message.kind,
    message.itemType ?? "",
    message.text,
    message.label ?? "",
    message.status ?? "",
    message.turnStatus ?? "",
    message.startedAtMs ?? "",
    message.completedAtMs ?? "",
    message.durationMs ?? "",
    message.workedDurationMs ?? "",
    message.command ?? "",
    JSON.stringify(message.commandActions ?? []),
    message.cwd ?? "",
    message.shellName ?? "",
    message.exitCode ?? "",
    message.phase ?? "",
    message.action ?? "",
    message.senderThreadId ?? "",
    JSON.stringify(message.receiverThreadIds ?? []),
    message.prompt ?? "",
    message.model ?? "",
    message.reasoningEffort ?? "",
    JSON.stringify(message.agentsStates ?? {}),
    message.agentThreadId ?? "",
    message.agentPath ?? "",
    message.displayName ?? "",
    message.displayStatus ?? "",
    message.activityKind ?? "",
    message.uiType ?? "",
    JSON.stringify(message.readPaths ?? []),
    message.output ?? "",
    message.breaksPreviousAdjacency ? "break" : "",
  ].join("\u001f")).join("\u001e");
}

/**
 * Encode a suffix replacement instead of repeating the complete structured
 * history on every streaming text patch. Initial/output snapshots still carry
 * the full projection, so reconnect and late-join hydration stay lossless.
 */
function renderedMessagesPatch(
  previous: RenderedConversationMessage[],
  next: RenderedConversationMessage[],
): JsonObject | undefined {
  const sharedLength = Math.min(previous.length, next.length);
  let start = 0;
  while (start < sharedLength
    && stableStringify(asJsonValue(previous[start])) === stableStringify(asJsonValue(next[start]))) start += 1;
  if (start === previous.length && start === next.length) return undefined;
  return {
    start,
    deleteCount: previous.length - start,
    messages: asJsonValue(next.slice(start)),
  };
}

function renderedSubagentShape(subagents: SubagentSnapshot[]): string {
  return JSON.stringify(subagents);
}

function renderConversationOutput(state: JsonObject, maxChars: number): { text: string; totalLength: number; truncated: boolean; messages: RenderedConversationMessage[]; subagents: SubagentSnapshot[] } {
  const chunks: string[] = [];
  const messages: RenderedConversationMessage[] = [];
  const seen = new Map<string, number>();
  const add = (text: string, id?: string, message?: RenderedConversationMessage): void => {
    const safe = redactText(text);
    if (!safe) return;
    if (id && seen.has(id)) {
      // The same turn is commonly present in both the canonical history and
      // the active-page list. Keep the latest item text when a streaming item
      // was updated, instead of dropping the active-page update entirely.
      const position = seen.get(id) as number;
      chunks[position] = safe;
      if (message) messages[position] = { ...message, text: message.text ? redactText(message.text) : safe };
      return;
    }
    if (id) seen.set(id, chunks.length);
    chunks.push(safe);
    if (message) messages.push({ ...message, text: redactText(message.text || safe) });
  };
  const consumeTurn = (turn: Record<string, unknown>): void => {
    const turnKey = stringValue(turn.id) ?? stringValue(turn.turnId);
    const turnStatus = statusValue(turn.status);
    const turnStartedAtMs = timestampMs(turn.turnStartedAtMs)
      ?? timestampMs(turn.startedAtMs)
      ?? timestampMs(turn.startedAt)
      ?? timestampMs(turn.createdAtMs)
      ?? timestampMs(turn.createdAt);
    const turnDurationMs = numberValue(turn.durationMs) ?? numberValue(turn.duration);
    const firstTurnWorkItemStartedAtMs = timestampMs(turn.firstTurnWorkItemStartedAtMs)
      ?? timestampMs(turn.firstWorkItemStartedAtMs)
      ?? timestampMs(turn.firstTurnWorkItemStartedAt);
    const finalAssistantStartedAtMs = timestampMs(turn.finalAssistantStartedAtMs)
      ?? timestampMs(turn.finalAssistantStartedAt);
    const commandStarts = isRecord(turn.commandExecutionStartedAtMsById)
      ? turn.commandExecutionStartedAtMsById
      : {};
    const items = Array.isArray(turn.items) ? turn.items.filter(isRecord) : [];
    const inferredFirstWorkItemStartedAtMs = firstTurnWorkItemStartedAtMs
      ?? inferFirstWorkItemStartedAtMs(items, commandStarts);
    const inferredFinalAssistantStartedAtMs = finalAssistantStartedAtMs
      ?? inferFinalAssistantStartedAtMs(items, commandStarts);
    const workedCompletedAtMs = inferredFinalAssistantStartedAtMs
      ?? (!TERMINAL_TURN_STATES.has(normalizeStatus(turnStatus))
        ? undefined
        : timestampMs(turn.completedAtMs) ?? timestampMs(turn.completedAt));
    const workedDurationMs = numberValue(turn.workedDurationMs)
      ?? numberValue(turn.workDurationMs)
      ?? (inferredFirstWorkItemStartedAtMs !== undefined && workedCompletedAtMs !== undefined
        ? Math.max(0, workedCompletedAtMs - inferredFirstWorkItemStartedAtMs)
        : undefined);
    // A turn may append bookkeeping/tool records after the final assistant
    // item. Identify the final assistant from the rendered assistant records,
    // with an explicit final-answer phase taking precedence over chronology.
    const itemDisplays = items.map((item) => itemDisplayVariants(item));
    let finalAssistantIndex = -1;
    let explicitFinalAssistantIndex = -1;
    itemDisplays.forEach((displays, index) => {
      if (!displays.some((display) => display.role === "assistant")) return;
      finalAssistantIndex = index;
      const phase = typeof items[index].phase === "string"
        ? normalizeStatus(items[index].phase)
        : "";
      if (phase === "final_answer" || phase === "finalanswer") explicitFinalAssistantIndex = index;
    });
    if (explicitFinalAssistantIndex >= 0) finalAssistantIndex = explicitFinalAssistantIndex;
    items.forEach((item, index) => {
      const displays = itemDisplays[index];
      if (!displays.length) return;
      const rawItemId = typeof item.id === "string" ? item.id : undefined;
      const startedAtMs = timestampMs(item.startedAtMs)
        ?? timestampMs(item.startedAt)
        ?? (rawItemId ? timestampMs(commandStarts[rawItemId]) : undefined);
      const durationMs = numberValue(item.durationMs) ?? numberValue(item.duration);
      const completedAtMs = timestampMs(item.completedAtMs)
        ?? timestampMs(item.finishedAtMs)
        ?? timestampMs(item.completedAt)
        ?? (startedAtMs !== undefined && durationMs !== undefined ? startedAtMs + durationMs : undefined);
      const command = commandText(item);
      const phase = typeof item.phase === "string" ? item.phase : undefined;
      displays.forEach((display, displayIndex) => {
        const projectionId = display.projectionId ?? rawItemId;
        const itemId = projectionId
          ? `id:${projectionId}`
          : turnKey
            ? `turn:${turnKey}:${index}:${displayIndex}`
            : `raw:${JSON.stringify(item)}:${displayIndex}`;
        const isFinalAssistant = display.role === "assistant"
          && (phase === "final_answer" || phase === "final-answer" || index === finalAssistantIndex);
        // User items do not carry their own timestamp in several official
        // snapshots. Associate them with the turn start; likewise associate the
        // final assistant item with the turn's final-answer start and duration.
        const effectiveStartedAtMs = display.startedAtMs ?? startedAtMs
          ?? (display.role === "user" ? turnStartedAtMs : undefined)
          ?? (display.role === "reasoning" ? inferredFirstWorkItemStartedAtMs : undefined)
          ?? (isFinalAssistant ? inferredFinalAssistantStartedAtMs : undefined);
        const effectiveDurationMs = display.durationMs ?? durationMs
          ?? (isFinalAssistant ? turnDurationMs : undefined);
        const effectiveCompletedAtMs = display.completedAtMs ?? completedAtMs
          ?? (effectiveStartedAtMs !== undefined && effectiveDurationMs !== undefined
            ? effectiveStartedAtMs + effectiveDurationMs
            : undefined);
        const itemStatus = display.status ?? statusValue(item.status)
          ?? (item.completed === true ? "completed" : item.completed === false ? "in_progress" : undefined);
        const displayCommand = display.command ?? (displayIndex === 0 ? command : undefined);
        add(display.outputText, itemId, {
          id: itemId,
          ...(turnKey ? { turnId: turnKey } : {}),
          ...(projectionId ? { itemId: projectionId } : {}),
          role: display.role,
          kind: display.kind,
          text: display.text,
          ...(display.label ? { label: display.label } : {}),
          ...(display.itemType ? { itemType: display.itemType } : typeof item.type === "string" ? { itemType: item.type } : typeof item.kind === "string" ? { itemType: item.kind } : {}),
          ...(itemStatus ? { status: itemStatus } : {}),
          ...(turnStatus ? { turnStatus } : {}),
          ...(effectiveStartedAtMs !== undefined ? { startedAtMs: effectiveStartedAtMs } : {}),
          ...(effectiveCompletedAtMs !== undefined ? { completedAtMs: effectiveCompletedAtMs } : {}),
          ...(effectiveDurationMs !== undefined ? { durationMs: effectiveDurationMs } : {}),
          ...(workedDurationMs !== undefined ? { workedDurationMs } : {}),
          ...(displayCommand ? { command: displayCommand } : {}),
          ...(display.commandActions?.length ? { commandActions: display.commandActions } : {}),
          ...(display.cwd !== undefined ? { cwd: display.cwd } : {}),
          ...(display.shellName !== undefined ? { shellName: display.shellName } : {}),
          ...(numberValue(item.exitCode) !== undefined && displayIndex === 0 ? { exitCode: numberValue(item.exitCode) as number } : {}),
          ...(phase && displayIndex === 0 ? { phase } : {}),
          ...(display.action ? { action: display.action } : {}),
          ...(display.senderThreadId ? { senderThreadId: display.senderThreadId } : {}),
          ...(display.receiverThreadIds ? { receiverThreadIds: display.receiverThreadIds, receiverThreads: display.receiverThreads ?? display.receiverThreadIds } : {}),
          ...(display.prompt !== undefined ? { prompt: display.prompt } : {}),
          ...(display.model !== undefined ? { model: display.model } : {}),
          ...(display.reasoningEffort !== undefined ? { reasoningEffort: display.reasoningEffort } : {}),
          ...(display.agentsStates ? { agentsStates: display.agentsStates } : {}),
          ...(display.agentThreadId ? { agentThreadId: display.agentThreadId } : {}),
          ...(display.agentPath ? { agentPath: display.agentPath } : {}),
          ...(display.displayName !== undefined ? { displayName: display.displayName } : {}),
          ...(display.displayStatus ? { displayStatus: display.displayStatus } : {}),
          ...(display.activityKind ? { activityKind: display.activityKind } : {}),
          ...(display.uiType ? { uiType: display.uiType } : {}),
          ...(display.readPaths?.length ? { readPaths: display.readPaths } : {}),
          ...(display.output ? { output: redactText(display.output) } : {}),
          ...(item.breaksPreviousAdjacency === true ? { breaksPreviousAdjacency: true } : {}),
        });
      });
    });
  };
  // Canonical history islands carry the stable chronological order. The
  // lightweight `turns` list is usually just the active page, so append only
  // entities that are not already represented there.
  for (const turn of orderedHistoryTurns(state)) consumeTurn(turn);
  if (Array.isArray(state.turns)) for (const turn of state.turns) if (isRecord(turn)) consumeTurn(turn);
  const rendered = chunks.join("\n\n");
  return {
    text: rendered.length > maxChars ? rendered.slice(-maxChars) : rendered,
    totalLength: rendered.length,
    truncated: rendered.length > maxChars,
    messages,
    subagents: collectSubagents(state),
  };
}

/**
 * Return only newly appended text. Once the bounded output window starts
 * sliding, compare the old suffix with the new prefix so a one-character
 * stream update does not retransmit the entire 32 KB snapshot.
 */
function appendOnlyOutputDelta(
  previous: string,
  previousLength: number,
  next: string,
  nextLength: number,
  previousWasTruncated: boolean,
): string | undefined {
  // A replacement, deletion, or history prepend cannot be represented by an
  // append-only chunk. Fall back to a bounded snapshot in those cases.
  if (nextLength < previousLength) return undefined;
  if (!previousWasTruncated) return next.startsWith(previous) ? next.slice(previous.length) : undefined;
  if (!previous || !next) return undefined;
  const dropped = Math.max(0, nextLength - next.length) - Math.max(0, previousLength - previous.length);
  if (dropped < 0 || dropped > previous.length) return undefined;
  const retained = previous.slice(dropped);
  if (retained.length > next.length || next.slice(0, retained.length) !== retained) return undefined;
  // If the append is larger than the retained tail, the bounded state no
  // longer contains all newly appended text; a snapshot is the only lossless
  // representation.
  const deltaLength = nextLength - previousLength;
  if (deltaLength !== next.length - retained.length) return undefined;
  return next.slice(retained.length);
}

function orderedHistoryTurns(state: JsonObject): Record<string, unknown>[] {
  const turnHistory = isRecord(state.turnHistory) ? state.turnHistory : undefined;
  const history = turnHistory && isRecord(turnHistory.history) ? turnHistory.history : undefined;
  const entities = history && isRecord(history.entitiesByKey) ? history.entitiesByKey : undefined;
  if (!entities) return [];
  const ordered: Record<string, unknown>[] = [];
  const seen = new Set<string>();
  const add = (key: unknown): void => {
    if (typeof key !== "string" || seen.has(key)) return;
    const entity = entities[key];
    if (!isRecord(entity) || !looksLikeTurn(entity)) return;
    seen.add(key);
    ordered.push(entity);
  };
  if (history && Array.isArray(history.islands)) {
    for (const island of history.islands) if (isRecord(island) && Array.isArray(island.entries)) {
      for (const entry of island.entries) {
        if (isRecord(entry)) add(entry.key ?? entry.value);
      }
    }
  }
  // Include entities not listed by islands for forward compatibility with an
  // extension that omits island metadata in a snapshot.
  for (const [key, entity] of Object.entries(entities)) {
    if (!seen.has(key) && isRecord(entity) && looksLikeTurn(entity)) {
      seen.add(key);
      ordered.push(entity);
    }
  }
  return ordered;
}

function looksLikeTurn(value: Record<string, unknown>): boolean {
  return Array.isArray(value.items) || value.turnId !== undefined || value.status !== undefined;
}

function isTransientHistoryLoadError(error: unknown): boolean {
  const code = isRecord(error) && typeof error.code === "string" ? error.code : "";
  if (["timeout", "connection-closed", "not-connected"].includes(code) || code.startsWith("no-client-found")) return true;
  const message = error instanceof Error ? error.message : String(error ?? "");
  return /timed? out|socket (?:is )?closed|not connected|no client found/i.test(message);
}

function hasIncompleteHistory(state: JsonObject): boolean {
  const turnHistory = isRecord(state.turnHistory) ? state.turnHistory : undefined;
  const history = turnHistory && isRecord(turnHistory.history) ? turnHistory.history : undefined;
  if (turnHistory?.kind === "canonical" && !history) return true;
  const entities = history && isRecord(history.entitiesByKey) ? Object.values(history.entitiesByKey) : [];
  const turns = [
    ...entities,
    ...(Array.isArray(state.turns) ? state.turns : []),
  ];
  // Both canonical entities and the legacy turns list expose this per-turn
  // marker. The official completeness predicate only treats an explicit
  // `false` as incomplete; missing metadata is compatible with older builds.
  if (turns.some((turn) => isRecord(turn)
    && isRecord(turn.itemsPagination)
    && turn.itemsPagination.hasLoadedOldest === false)) return true;

  // Canonical history is complete only after the owner has coalesced it into
  // one island. Boundary status is deliberately not checked here: the
  // official webview uses `isComplete` and island count, and some versions
  // leave boundary objects in a non-exhausted transitional shape.
  const canonical = Boolean(history && (
    turnHistory?.kind === "canonical"
    || history.isComplete !== undefined
    || Array.isArray(history.islands)
  ));
  if (canonical) {
    return history?.isComplete !== true
      || !Array.isArray(history.islands)
      || history.islands.length !== 1;
  }

  // Legacy snapshots carry a resume marker. Avoid requesting an unsupported
  // history operation for old snapshots that expose no pagination metadata at
  // all, while respecting explicit loading/unfinished states.
  if (state.resumeState !== undefined && state.resumeState !== "resumed") return true;
  const turnsPagination = isRecord(state.turnsPagination) ? state.turnsPagination : undefined;
  return turnsPagination?.hasLoadedOldest === false;
}

function itemDisplay(rawItem: Record<string, unknown>): ItemDisplayProjection | undefined {
  const normalizedItem = normalizeOfficialItem(rawItem);
  const type = String(normalizedItem.type ?? normalizedItem.kind ?? "").replace(/[\s/_-]+/g, "").toLowerCase();

  if (type === "collabagenttoolcall") {
    const tool = stringValue(normalizedItem.tool) ?? "collabAgent";
    // `wait` is an internal synchronization action. The official webview
    // consumes it for aggregation but intentionally omits it from the
    // visible transcript.
    if (tool === "wait") return undefined;
    const status = statusValue(normalizedItem.status) ?? "inProgress";
    const receiverThreadIds = stringArray(normalizedItem.receiverThreadIds);
    const agentsStates = collabAgentStates(normalizedItem.agentsStates);
    const prompt = redactNullableString(normalizedItem.prompt);
    const model = redactNullableString(normalizedItem.model);
    const reasoningEffort = redactNullableString(normalizedItem.reasoningEffort);
    const senderThreadId = stringValue(normalizedItem.senderThreadId);
    const label = collabAgentToolLabel(tool);
    const promptText = prompt?.trim() ? `: ${redactText(prompt.trim())}` : "";
    const outputText = `${label}${promptText}`;
    return {
      outputText,
      projectionId: stringValue(normalizedItem.id),
      role: "tool",
      kind: "tool",
      text: outputText,
      label,
      itemType: "collabAgentToolCall",
      status,
      action: tool,
      ...(senderThreadId ? { senderThreadId } : {}),
      receiverThreadIds,
      receiverThreads: receiverThreadIds,
      prompt,
      model,
      reasoningEffort,
      agentsStates: redactJson(agentsStates) as JsonObject,
      uiType: "multi-agent-action",
    };
  }

  if (type === "subagentactivity") {
    const activityKind = stringValue(normalizedItem.kind) ?? "started";
    const agentThreadId = stringValue(normalizedItem.agentThreadId);
    if (!agentThreadId) return undefined;
    const agentPath = redactNullableString(normalizedItem.agentPath);
    const displayName = formatAgentPath(agentPath ?? undefined);
    const displayStatus = subagentActivityDisplayStatus(activityKind);
    const status = statusValue(normalizedItem.status)
      ?? (activityKind === "interrupted" || activityKind === "completed" ? "completed" : "inProgress");
    const label = displayName ? `子代理 · ${displayName}` : "子代理";
    const activityText = subagentActivityText(displayName, activityKind);
    return {
      outputText: activityText,
      projectionId: stringValue(normalizedItem.id),
      role: "tool",
      kind: "tool",
      text: activityText,
      label,
      itemType: "subAgentActivity",
      status,
      agentThreadId,
      ...(agentPath ? { agentPath } : {}),
      displayName,
      displayStatus,
      activityKind,
      uiType: "subagent-activity",
    };
  }

  const item = normalizedItem;
  // Pending permission/input/elicitation items are rendered by the request
  // card, not as a second transcript activity. Once the owner marks one
  // complete it may re-enter history and be displayed normally.
  const requestItem = type.includes("permissionrequest")
    || type.includes("userinput")
    || type.includes("mcpserverelicitation");
  const itemStatus = normalizeStatus(statusValue(item.status));
  const requestPending = requestItem
    && item.completed !== true
    && !TERMINAL_TURN_STATES.has(itemStatus);
  if (requestPending) return undefined;
  if (["agentmessage", "assistantmessage", "usermessage"].includes(type)) {
    const text = textFromValue(item.text) ?? textFromValue(item.content);
    if (!text) return undefined;
    if (type.startsWith("user")) return { outputText: `> ${text}`, role: "user", kind: "user", text };
    return { outputText: text, role: "assistant", kind: "assistant", text };
  }
  if (type.includes("contextcompaction")) {
    const text = textFromValue(item.summary) ?? textFromValue(item.content) ?? textFromValue(item.text) ?? "整理上下文";
    return { outputText: text, role: "reasoning", kind: "reasoning", text, label: "整理上下文" };
  }
  if (type.includes("reasoning") || type.includes("approvalreview")) {
    const text = textFromValue(item.summary) ?? textFromValue(item.content);
    return text ? { outputText: text, role: "reasoning", kind: "reasoning", text, label: "思考" } : undefined;
  }
  if (type.includes("plan") || type.includes("todo")) {
    const value = item.plan ?? item.steps ?? item.todos ?? item.content ?? item.text;
    const text = planDisplayText(value);
    return text ? { outputText: text, role: "reasoning", kind: "plan", text, label: "计划" } : undefined;
  }
  const parsedActions = commandActionsValue(item);
  const readPaths = readPathsFromActions(parsedActions);
  const directReadItem = type === "read"
    || type.includes("fileread")
    || type.includes("readfile")
    || type.includes("exploration");
  if (directReadItem || readPaths.length > 0) {
    const command = commandText(item);
    const output = textFromValue(item.aggregatedOutput)
      ?? textFromValue(item.output)
      ?? textFromValue(item.stdout)
      ?? textFromValue(item.content)
      ?? textFromValue(item.text);
    const pathSummary = readPaths.length ? readPaths.join(", ") : readPathFromItem(item);
    const summary = pathSummary ? `已读取 ${pathSummary}` : "已读取文件";
    const text = summary;
    const commandActions = projectCommandActions(parsedActions);
    const cwd = redactNullableString(item.cwd);
    const shellName = redactNullableString(item.shellName ?? item.shell);
    return {
      outputText: output && output.trim() ? `${summary}\n${output}` : text,
      role: "tool",
      kind: "tool",
      text,
      label: "已读取文件",
      itemType: typeof item.type === "string" ? item.type : "fileRead",
      ...(command ? { command } : {}),
      ...(commandActions.length ? { commandActions } : {}),
      ...(cwd !== undefined ? { cwd } : {}),
      ...(shellName !== undefined ? { shellName } : {}),
      ...(readPaths.length ? { readPaths } : {}),
      ...(output && output.trim() ? { output } : {}),
      activityKind: "read",
      uiType: "file-read",
    };
  }
  if (type.includes("filechange") || type.includes("file_change") || type.includes("patch") || type.includes("edit")) {
    const text = textFromValue(item.diff)
      ?? textFromValue(item.patch)
      ?? fileChangesText(item.changes)
      ?? textFromValue(item.output)
      ?? textFromValue(item.text);
    return text ? { outputText: text, role: "tool", kind: "edit", text, label: "文件变更" } : undefined;
  }
  const hasCommandProjection = isCommandActionValue(item.commandActions)
    || isCommandActionValue(item.command_actions)
    || isCommandActionValue(item.parsedCmd)
    || isCommandActionValue(item.parsed_cmd)
    || item.command !== undefined
    || item.commandLine !== undefined;
  if (type.includes("command") || type.includes("exec") || type.includes("process") || hasCommandProjection) {
    const command = commandText(item);
    // Some official snapshots expose commandActions before output is flushed
    // (and may leave aggregatedOutput as an empty string). Keep the command
    // visible in that state, while avoiding a bare shell bootstrap such as
    // `/bin/zsh` becoming the displayed command.
    const output = textFromValue(item.aggregatedOutput)
      ?? textFromValue(item.output)
      ?? textFromValue(item.stdout)
      ?? textFromValue(item.stderr);
    const text = output || command;
    const commandActions = projectCommandActions(parsedActions);
    const cwd = redactNullableString(item.cwd);
    const shellName = redactNullableString(item.shellName ?? item.shell);
    return text ? {
      outputText: text,
      role: "tool",
      kind: "tool",
      text,
      label: "命令输出",
      ...(commandActions.length ? { commandActions } : {}),
      ...(cwd !== undefined ? { cwd } : {}),
      ...(shellName !== undefined ? { shellName } : {}),
    } : undefined;
  }
  if (type.includes("websearch") || type.includes("mcp") || type.includes("dynamictool")
    || type.includes("imageview") || type.includes("imagegeneration") || type.includes("generatedimage")
    || type.includes("toolcall") || type.includes("permissionrequest") || type.includes("userinput")) {
    const text = textFromValue(item.output)
      ?? textFromValue(item.result)
      ?? textFromValue(item.content)
      ?? textFromValue(item.summary)
      ?? textFromValue(item.text)
      ?? textFromValue(item.query)
      ?? textFromValue(item.name);
    if (!text) return undefined;
    const label = type.includes("websearch") ? "搜索"
      : type.includes("image") ? "查看图像"
        : type.includes("permissionrequest") ? "等待授权"
          : type.includes("userinput") ? "等待输入"
        : type.includes("mcp") ? "MCP 工具"
          : "工具";
    return { outputText: text, role: "tool", kind: "tool", text, label };
  }
  const text = textFromValue(item.text) ?? textFromValue(item.output);
  return text ? { outputText: text, role: "assistant", kind: "assistant", text } : undefined;
}

/**
 * Official conversation messages can carry collaboration records in a
 * metadata envelope instead of exposing them as a top-level `type`. Normalize
 * direct item names here; metadata variants are added by
 * `itemDisplayVariants` so the parent message is retained as well.
 */
function normalizeOfficialItem(item: Record<string, unknown>): Record<string, unknown> {
  const directType = String(item.type ?? item.kind ?? "").replace(/[\s/_-]+/g, "").toLowerCase();
  if (directType === "collabagenttoolcall") {
    return { ...item, type: "collabAgentToolCall" };
  }
  if (directType === "subagentactivity") {
    return { ...item, type: "subAgentActivity" };
  }
  return item;
}

/** Return the normal item plus any collaboration records attached as metadata. */
function itemDisplayVariants(item: Record<string, unknown>): ItemDisplayProjection[] {
  const displays: ItemDisplayProjection[] = [];
  const base = itemDisplay(item);
  if (base) displays.push(base);
  const metadata = parseRecord(item.metadata);
  const candidates: Array<{ key: string; type: "collabAgentToolCall" | "subAgentActivity" }> = [
    { key: "codex_collab_agent_tool_call", type: "collabAgentToolCall" },
    { key: "codex_sub_agent_activity", type: "subAgentActivity" },
  ];
  for (const candidate of candidates) {
    const value = parseRecord(metadata?.[candidate.key]);
    if (!value) continue;
    const normalized: Record<string, unknown> = { ...value, type: candidate.type };
    // A direct item may carry a copy of its own metadata record. Do not render
    // that record twice when the ids identify the same official item.
    const normalizedId = stringValue(normalized.id);
    if (normalizedId && displays.some((display) => display.projectionId === normalizedId)) continue;
    const display = itemDisplay(normalized);
    if (display) displays.push(display);
  }
  return displays;
}

function parseRecord(value: unknown): Record<string, unknown> | undefined {
  if (isRecord(value)) return value;
  if (typeof value !== "string") return undefined;
  try {
    const parsed: unknown = JSON.parse(value);
    return isRecord(parsed) ? parsed : undefined;
  } catch {
    return undefined;
  }
}

function stringArray(value: unknown): string[] {
  return Array.isArray(value) ? value.filter((entry): entry is string => typeof entry === "string") : [];
}

function nullableString(value: unknown): string | null | undefined {
  return typeof value === "string" ? value : value === null ? null : undefined;
}

function redactNullableString(value: unknown): string | null | undefined {
  const normalized = nullableString(value);
  return normalized === undefined || normalized === null ? normalized : redactText(normalized);
}

function collabAgentStates(value: unknown): JsonObject {
  const record = parseRecord(value);
  if (!record) return {};
  const states: JsonObject = {};
  for (const [threadId, rawState] of Object.entries(record)) {
    const state = parseRecord(rawState);
    if (!state || typeof state.status !== "string") continue;
    states[threadId] = redactJson({
      status: state.status,
      ...(state.message === null || typeof state.message === "string" ? { message: state.message } : {}),
    }) as JsonValue;
  }
  return states;
}

function collabAgentToolLabel(tool: string): string {
  switch (tool) {
    case "spawnAgent": return "启动子代理";
    case "sendInput": return "向子代理发送输入";
    case "resumeAgent": return "恢复子代理";
    case "wait": return "等待子代理";
    case "closeAgent": return "关闭子代理";
    default: return "子代理操作";
  }
}

function formatAgentPath(agentPath?: string): string | null {
  if (!agentPath) return null;
  const leaf = agentPath.split("/").map((part) => part.trim()).filter((part) => part && part !== "root").at(-1);
  if (!leaf) return null;
  const normalized = leaf.replace(/[_-]+/g, " ").replace(/\s+/g, " ").trim().toLowerCase();
  return normalized ? normalized[0].toUpperCase() + normalized.slice(1) : null;
}

function subagentActivityDisplayStatus(kind: string): string {
  switch (kind) {
    case "started": return "active";
    case "interacted": return "updated";
    case "interrupted": return "interrupted";
    case "completed": return "completed";
    default: return "active";
  }
}

function subagentActivityText(displayName: string | null, kind: string): string {
  const subject = displayName ? `子代理 ${displayName}` : "子代理";
  switch (kind) {
    case "started": return `${subject} 已开始工作`;
    case "interacted": return `${subject} 正在工作`;
    case "interrupted": return `${subject} 已中断`;
    case "completed": return `${subject} 已完成`;
    default: return `${subject} ${kind}`;
  }
}

interface SubagentAccumulator extends SubagentSnapshot {
  lastEventIndex: number;
}

/** Rebuild the official subagent panel model from direct items and metadata. */
function collectSubagents(state: JsonObject): SubagentSnapshot[] {
  const agents = new Map<string, SubagentAccumulator>();
  const parentThreadId = stringValue(state.id)
    ?? stringValue(state.threadId)
    ?? (isRecord(state.thread) ? stringValue(state.thread.id) : undefined)
    ?? null;
  let eventIndex = 0;
  const ensure = (threadId: string): SubagentAccumulator => {
    const current = agents.get(threadId);
    if (current) {
      current.lastEventIndex = eventIndex;
      return current;
    }
    const created: SubagentAccumulator = {
      threadId,
      displayName: null,
      prompt: null,
      objective: null,
      status: "working",
      statusMessage: null,
      canInteract: false,
      parentThreadId,
      lastEventIndex: eventIndex,
    };
    agents.set(threadId, created);
    return created;
  };
  const consumeTurn = (turn: Record<string, unknown>): void => {
    const turnStartedAtMs = timestampMs(turn.turnStartedAtMs)
      ?? timestampMs(turn.startedAtMs)
      ?? timestampMs(turn.startedAt)
      ?? timestampMs(turn.createdAtMs)
      ?? timestampMs(turn.createdAt);
    const turnCompletedAtMs = timestampMs(turn.completedAtMs)
      ?? timestampMs(turn.completedAt)
      ?? (turnStartedAtMs !== undefined && numberValue(turn.durationMs) !== undefined
        ? turnStartedAtMs + (numberValue(turn.durationMs) as number)
        : undefined);
    for (const rawItem of Array.isArray(turn.items) ? turn.items : []) {
      if (!isRecord(rawItem)) continue;
      const itemStartedAtMs = timestampMs(rawItem.startedAtMs)
        ?? timestampMs(rawItem.startedAt)
        ?? turnStartedAtMs;
      const itemCompletedAtMs = timestampMs(rawItem.completedAtMs)
        ?? timestampMs(rawItem.finishedAtMs)
        ?? timestampMs(rawItem.completedAt)
        ?? (itemStartedAtMs !== undefined && numberValue(rawItem.durationMs) !== undefined
          ? itemStartedAtMs + (numberValue(rawItem.durationMs) as number)
          : turnCompletedAtMs);
      for (const item of officialSubagentItems(rawItem)) {
        eventIndex += 1;
        const type = String(item.type ?? item.kind ?? "").replace(/[\s/_-]+/g, "").toLowerCase();
        if (type === "subagentactivity") {
          const threadId = stringValue(item.agentThreadId);
          if (!threadId) continue;
          const agent = ensure(threadId);
          const agentPath = stringValue(item.agentPath);
          const displayName = formatAgentPath(agentPath);
          const activityKind = stringValue(item.kind) ?? "started";
          if (displayName) agent.displayName = redactText(displayName);
          if (agentPath) agent.agentPath = redactText(agentPath);
          if (agent.startedAtMs == null && itemStartedAtMs !== undefined) agent.startedAtMs = itemStartedAtMs;
          agent.statusMessage = null;
          if (activityKind === "interrupted" || activityKind === "completed") {
            agent.status = "done";
            if (itemCompletedAtMs !== undefined) agent.completedAtMs = itemCompletedAtMs;
          } else {
            agent.status = "working";
            agent.completedAtMs = null;
          }
          continue;
        }
        if (type !== "collabagenttoolcall") continue;
        const tool = stringValue(item.tool) ?? "";
        const toolStatus = statusValue(item.status) ?? "inProgress";
        const receivers = stringArray(item.receiverThreadIds ?? item.receiverThreads);
        const states = parseRecord(item.agentsStates) ?? {};
        const prompt = nullableString(item.prompt);
        const model = nullableString(item.model);

        for (const threadId of new Set([...receivers, ...Object.keys(states)])) {
          if (!threadId) continue;
          const agent = ensure(threadId);
          if (agent.startedAtMs == null && itemStartedAtMs !== undefined) agent.startedAtMs = itemStartedAtMs;
          if (tool === "spawnAgent") {
            if (prompt?.trim()) {
              agent.prompt = redactText(prompt.trim());
              agent.objective = agent.prompt;
            }
            if (model !== undefined) agent.model = model === null ? null : redactText(model);
            agent.canInteract = true;
          } else if (tool === "sendInput" || tool === "resumeAgent") {
            agent.canInteract = true;
          }
          if (toolStatus === "failed") {
            agent.status = "failed";
            if (itemCompletedAtMs !== undefined) agent.completedAtMs = itemCompletedAtMs;
          } else if (tool === "spawnAgent" || tool === "sendInput" || tool === "resumeAgent") {
            agent.status = "working";
            agent.statusMessage = null;
            agent.completedAtMs = null;
          } else if (tool === "closeAgent" && toolStatus === "completed") {
            agent.status = "done";
            if (itemCompletedAtMs !== undefined) agent.completedAtMs = itemCompletedAtMs;
          }
        }

        for (const [threadId, rawState] of Object.entries(states)) {
          const agentState = parseRecord(rawState);
          if (!agentState || typeof agentState.status !== "string") continue;
          const agent = ensure(threadId);
          agent.status = coarseSubagentStatus(agentState.status);
          if (agent.status === "waiting" || agent.status === "working") {
            agent.statusMessage = null;
            agent.completedAtMs = null;
          } else {
            const statusMessage = nullableString(agentState.message);
            agent.statusMessage = statusMessage?.trim() ? redactText(statusMessage.trim()) : null;
            if (itemCompletedAtMs !== undefined) agent.completedAtMs = itemCompletedAtMs;
          }
        }

        // A completed wait means the parent observed all currently active
        // receivers finishing, even if older state records were not patched.
        if (tool === "wait" && toolStatus === "completed") {
          for (const agent of agents.values()) {
            if (agent.status !== "waiting" && agent.status !== "working") continue;
            agent.status = "done";
            agent.statusMessage = null;
            agent.lastEventIndex = eventIndex;
            if (itemCompletedAtMs !== undefined) agent.completedAtMs = itemCompletedAtMs;
          }
        }
      }
    }
  };

  for (const turn of orderedHistoryTurns(state)) consumeTurn(turn);
  if (Array.isArray(state.turns)) for (const turn of state.turns) if (isRecord(turn)) consumeTurn(turn);

  // The official UI closes stale active rows when the parent turn is no longer
  // live. This prevents an old running state from lingering after reconnect.
  if (!deriveTurn(state).active) {
    for (const agent of agents.values()) {
      if (agent.status !== "waiting" && agent.status !== "working") continue;
      agent.status = "done";
      agent.statusMessage = null;
    }
  }

  return Array.from(agents.values(), ({ lastEventIndex: _lastEventIndex, ...agent }) => agent);
}

function officialSubagentItems(item: Record<string, unknown>): Record<string, unknown>[] {
  const result: Record<string, unknown>[] = [];
  const seen = new Set<string>();
  const add = (value: Record<string, unknown>, type: "collabAgentToolCall" | "subAgentActivity"): void => {
    const normalized: Record<string, unknown> = { ...value, type };
    const id = stringValue(normalized.id);
    const key = `${type}:${id ?? JSON.stringify(normalized)}`;
    if (seen.has(key)) return;
    seen.add(key);
    result.push(normalized);
  };
  const directType = String(item.type ?? item.kind ?? "").replace(/[\s/_-]+/g, "").toLowerCase();
  if (directType === "collabagenttoolcall") add(item, "collabAgentToolCall");
  if (directType === "subagentactivity") add(item, "subAgentActivity");
  const metadata = parseRecord(item.metadata);
  const collab = parseRecord(metadata?.codex_collab_agent_tool_call);
  if (collab) add(collab, "collabAgentToolCall");
  const activity = parseRecord(metadata?.codex_sub_agent_activity);
  if (activity) add(activity, "subAgentActivity");
  return result;
}

function coarseSubagentStatus(status: string): "waiting" | "working" | "done" | "failed" {
  switch (normalizeStatus(status)) {
    case "pendinginit": return "waiting";
    case "running": return "working";
    case "completed":
    case "interrupted":
    case "shutdown": return "done";
    case "errored":
    case "notfound": return "failed";
    default: return "working";
  }
}

function planDisplayText(value: unknown): string | undefined {
  if (typeof value === "string") return value;
  if (!Array.isArray(value)) {
    if (isRecord(value)) return planDisplayText(value.plan ?? value.steps ?? value.todos ?? value.text ?? value.content);
    return textFromValue(value);
  }
  const lines = value.map((entry) => {
    if (!isRecord(entry)) return textFromValue(entry);
    const status = statusValue(entry.status) ?? "pending";
    const marker = ["completed", "complete", "done", "success", "succeeded"].includes(status.toLowerCase()) ? "[x]" : "[ ]";
    const label = stringValue(entry.step) ?? stringValue(entry.text) ?? stringValue(entry.title) ?? stringValue(entry.description);
    return label ? `${marker} ${label}` : undefined;
  }).filter((line): line is string => Boolean(line));
  return lines.length ? lines.join("\n") : undefined;
}

function itemDisplayText(item: Record<string, unknown>): string | undefined {
  return itemDisplay(item)?.outputText;
}

/**
 * Keep the command action shape used by the official webview while making it
 * safe to send over the relay. Older snapshots use `command`, whereas the
 * webview's normalized action uses `cmd`; expose both aliases so either
 * renderer can consume the projection without losing the original fields.
 */
function projectCommandActions(value: unknown): JsonValue[] {
  const actions = commandActionList(value);
  if (!actions.length) return [];
  const projected: JsonValue[] = [];
  for (const rawAction of actions) {
    const action = redactJson(rawAction);
    if (isRecord(action)) {
      const command = stringValue(action.command) ?? stringValue(action.cmd);
      if (command && action.command === undefined) action.command = command;
      if (command && action.cmd === undefined) action.cmd = command;
      if (Object.keys(action).length > 0) projected.push(action as JsonObject);
      continue;
    }
    if (typeof action === "string" && action.trim()) projected.push(action);
  }
  return projected;
}

function commandActionText(value: unknown): string | undefined {
  if (typeof value === "string") return commandValue(value);
  if (!isRecord(value)) return undefined;
  const command = stringValue(value.command) ?? stringValue(value.cmd);
  return command ? commandValue(command) : undefined;
}

const SHELL_BOOTSTRAP = /^(?:.*[/\\])?(?:bash|cmd(?:\.exe)?|fish|powershell(?:\.exe)?|pwsh(?:\.exe)?|sh|zsh)(?:\s|$)/i;

function isShellBootstrapCommand(value: string): boolean {
  return SHELL_BOOTSTRAP.test(value.trim());
}

function commandValue(value: unknown): string | undefined {
  if (typeof value === "string") {
    const command = value.trim();
    if (!command) return undefined;
    // A few IPC versions put the shell wrapper and the user command in one
    // string instead of an argv array. Strip only the wrapper here; the
    // frontend remains responsible for presentation-level quote cleanup.
    const wrapped = command.match(/^(?:.*[/\\])?(?:bash|cmd(?:\.exe)?|fish|powershell(?:\.exe)?|pwsh(?:\.exe)?|sh|zsh)\s+-(?:l?c|c?l)\s+([\s\S]+)$/i);
    return (wrapped?.[1] ?? command).trim() || undefined;
  }
  if (!Array.isArray(value)) return undefined;
  const parts = value.filter((part): part is string => typeof part === "string").map((part) => part.trim()).filter(Boolean);
  if (!parts.length) return undefined;
  // The IPC snapshot may preserve the process argv (`zsh -lc <command>`)
  // rather than the command string shown by the official disclosure.
  if (parts.length >= 3 && SHELL_BOOTSTRAP.test(parts[0]) && /^-(?:l?c|c?l)$/i.test(parts[1])) {
    return parts.slice(2).join(" ").trim() || undefined;
  }
  return parts.join(" ").trim() || undefined;
}

function commandText(item: Record<string, unknown>): string | undefined {
  // The official renderer walks actions backwards and displays the last
  // non-bootstrap command. This matters when the first action is just the
  // shell wrapper used to launch the process.
  const actions = commandActionsValue(item);
  const actionList = commandActionList(actions);
  for (let index = actionList.length - 1; index >= 0; index -= 1) {
    const candidate = commandActionText(actionList[index]);
    if (candidate && !isShellBootstrapCommand(candidate)) return candidate;
  }
  const command = commandValue(item.command) ?? stringValue(item.commandLine)?.trim();
  if (!command || isShellBootstrapCommand(command)) return undefined;
  return command;
}

/** Read the parsed command-action field across live and persisted schemas. */
function commandActionsValue(item: Record<string, unknown>): unknown {
  return item.commandActions
    ?? item.command_actions
    ?? item.parsedCmd
    ?? item.parsed_cmd;
}

/** Normalize live/persisted command actions, which may be a single object. */
function commandActionList(value: unknown): unknown[] {
  if (Array.isArray(value)) return value;
  return isRecord(value) ? [value] : [];
}

function isCommandActionValue(value: unknown): boolean {
  return Array.isArray(value) || isRecord(value);
}

function readPathsFromActions(value: unknown): string[] {
  const actions = commandActionList(value);
  if (!actions.length) return [];
  const paths: string[] = [];
  const seen = new Set<string>();
  for (const raw of actions) {
    if (!isRecord(raw)) continue;
    const type = normalizeStatus(raw.type);
    if (type !== "read") continue;
    const path = stringValue(raw.path) ?? stringValue(raw.filePath) ?? stringValue(raw.file_path) ?? stringValue(raw.name);
    if (!path) continue;
    const safe = redactText(path.trim());
    if (!safe || seen.has(safe)) continue;
    seen.add(safe);
    paths.push(safe);
  }
  return paths.slice(0, 128);
}

function readPathFromItem(item: Record<string, unknown>): string | undefined {
  const value = item.path ?? item.filePath ?? item.file_path ?? item.file ?? item.name;
  return typeof value === "string" && value.trim() ? redactText(value.trim()) : undefined;
}

function fileChangesText(value: unknown): string | undefined {
  if (!Array.isArray(value)) return textFromValue(value);
  const changes = value.map((change) => {
    if (!isRecord(change)) return textFromValue(change);
    const file = stringValue(change.path) ?? stringValue(change.filePath) ?? stringValue(change.file) ?? stringValue(change.name);
    const kind = statusValue(change.kind ?? change.type ?? change.status);
    const diff = textFromValue(change.diff) ?? textFromValue(change.patch) ?? textFromValue(change.output) ?? textFromValue(change.text);
    const heading = [kind ? `[${kind}]` : undefined, file].filter(Boolean).join(" ");
    return [heading, diff].filter(Boolean).join("\n");
  }).filter((part): part is string => Boolean(part));
  return changes.length ? changes.join("\n\n") : undefined;
}

function statusValue(value: unknown): string | undefined {
  if (typeof value === "string") return value;
  return isRecord(value) && typeof value.type === "string" ? value.type : undefined;
}

function textFromValue(value: unknown): string | undefined {
  if (typeof value === "string") return value;
  if (Array.isArray(value)) {
    const parts = value.map(textFromValue).filter((part): part is string => Boolean(part));
    return parts.length ? parts.join("\n") : undefined;
  }
  if (isRecord(value)) {
    for (const key of ["text", "value", "output", "stdout", "stderr", "delta", "summary"]) {
      const text = textFromValue(value[key]);
      if (text) return text;
    }
  }
  return undefined;
}

const MODEL_CATALOG_ROOT_KEYS = ["availableModels", "models", "modelCatalog", "listModels"] as const;
const MODEL_CATALOG_CONTAINER_KEYS = new Set<string>([
  "data",
  "items",
  "models",
  "availableModels",
  "modelCatalog",
  "listModels",
]);
const MODEL_CATALOG_META_KEYS = new Set<string>([
  "cursor",
  "nextCursor",
  "next_cursor",
  "hasMore",
  "has_more",
  "total",
  "name",
  "label",
  "description",
  "provider",
  "status",
  "type",
  "message",
  "error",
]);
const MODEL_CATALOG_MAX_ENTRIES = 256;
const MODEL_CATALOG_MAX_TEXT = 512;
const MODEL_CATALOG_MAX_MODEL = 256;
const MODEL_CATALOG_MAX_EFFORTS = 32;
const MODEL_CATALOG_MAX_SCANNED = 4_096;

/**
 * Keep the model directory useful to the browser without forwarding opaque
 * provider records (which may contain credentials, URLs, or internal flags).
 * The official `model/list` response is normally `{ data: Model[] }`, but
 * older extension builds have exposed the same data under several state keys.
 */
function projectAvailableModels(state: JsonObject): JsonValue[] {
  const sources: unknown[] = [];
  const addSources = (value: unknown): void => {
    if (!isRecord(value)) return;
    for (const key of MODEL_CATALOG_ROOT_KEYS) {
      if (value[key] !== undefined) sources.push(value[key]);
    }
  };
  addSources(state);
  for (const key of ["thread", "metadata", "conversation", "session", "latestThreadSettings", "threadSettings", "settings"]) {
    addSources(state[key]);
  }

  const projected: JsonObject[] = [];
  const byModel = new Map<string, JsonObject>();
  const visited = new Set<object>();
  let scanned = 0;

  const add = (value: unknown, fallbackModel?: string): void => {
    const item = projectAvailableModel(value, fallbackModel);
    if (!item) return;
    const model = stringValue(item.model);
    if (!model) return;
    const key = model.toLowerCase();
    const existing = byModel.get(key);
    if (!existing) {
      byModel.set(key, item);
      projected.push(item);
      return;
    }
    // A state patch can first expose a bare model id and later provide the
    // catalog details. Fill only absent fields so explicit false/null values
    // from the first projection are not accidentally overwritten.
    for (const [field, fieldValue] of Object.entries(item)) {
      if (existing[field] === undefined || (Array.isArray(existing[field]) && (existing[field] as unknown[]).length === 0)) {
        existing[field] = fieldValue;
      }
    }
  };

  const collect = (value: unknown, fallbackModel?: string, depth = 0): void => {
    if (projected.length >= MODEL_CATALOG_MAX_ENTRIES || scanned >= MODEL_CATALOG_MAX_SCANNED || depth > 6 || value === undefined || value === null) return;
    scanned += 1;
    if (typeof value === "string") {
      // Strings at the root are model ids. A string under a map key is a
      // display label, so retain the key as the canonical id in that case.
      if (fallbackModel && isPlausibleModelMapKey(fallbackModel)) add({ model: fallbackModel, displayName: value });
      else add(value);
      return;
    }
    if (Array.isArray(value)) {
      for (const entry of value) collect(entry, undefined, depth + 1);
      return;
    }
    if (!isRecord(value)) return;
    if (visited.has(value)) return;
    visited.add(value);

    let hasContainer = false;
    for (const key of MODEL_CATALOG_CONTAINER_KEYS) {
      if (value[key] === undefined) continue;
      hasContainer = true;
      collect(value[key], undefined, depth + 1);
    }

    const strongIdentity = modelCatalogText(value.model, MODEL_CATALOG_MAX_MODEL)
      ?? modelCatalogText(value.id, MODEL_CATALOG_MAX_MODEL)
      ?? modelCatalogText(value.slug, MODEL_CATALOG_MAX_MODEL);
    const directModel = modelCatalogIdentity(value);
    const isModelEntry = Boolean(
      (directModel && (!hasContainer || strongIdentity))
      || (fallbackModel && isPlausibleModelMapKey(fallbackModel)),
    );
    if (isModelEntry) add(value, fallbackModel);
    // Once a record has an identity, its scalar fields are model properties,
    // not additional map entries (for example `displayName: "Sol"`).
    if (isModelEntry && !hasContainer) return;

    // A map-shaped catalog (`{ "gpt-5": { displayName: ... } }`) is used by
    // a few pre-model/list extension builds. Ignore pagination metadata and
    // known envelopes while walking those entries.
    for (const [key, child] of Object.entries(value)) {
      if (MODEL_CATALOG_CONTAINER_KEYS.has(key) || MODEL_CATALOG_META_KEYS.has(key)) continue;
      if (!isPlausibleModelMapKey(key)) continue;
      if (hasContainer && !isRecord(child) && !Array.isArray(child)) continue;
      if (isRecord(child) || Array.isArray(child)) collect(child, key, depth + 1);
      else if (typeof child === "string" && isPlausibleModelMapKey(key)) collect(child, key, depth + 1);
    }
  };

  for (const source of sources) collect(source);
  return projected;
}

function isPlausibleModelMapKey(value: string): boolean {
  const key = value.trim();
  return Boolean(key)
    && key.length <= MODEL_CATALOG_MAX_MODEL
    && !MODEL_CATALOG_CONTAINER_KEYS.has(key)
    && !MODEL_CATALOG_META_KEYS.has(key)
    && !/(?:token|secret|password|authorization|api[_-]?key|private[_-]?key|refresh)/i.test(key);
}

function modelCatalogText(value: unknown, maxLength = MODEL_CATALOG_MAX_TEXT): string | undefined {
  if (typeof value !== "string") return undefined;
  const text = redactText(value.trim()).slice(0, maxLength).trim();
  return text || undefined;
}

function modelCatalogStrongIdentity(value: Record<string, unknown>): string | undefined {
  return modelCatalogText(value.model, MODEL_CATALOG_MAX_MODEL)
    ?? modelCatalogText(value.id, MODEL_CATALOG_MAX_MODEL)
    ?? modelCatalogText(value.slug, MODEL_CATALOG_MAX_MODEL);
}

function modelCatalogIdentity(value: Record<string, unknown>): string | undefined {
  return modelCatalogStrongIdentity(value)
    ?? modelCatalogText(value.name, MODEL_CATALOG_MAX_MODEL);
}

function projectAvailableModel(value: unknown, fallbackModel?: string): JsonObject | undefined {
  if (typeof value === "string") {
    const model = modelCatalogText(fallbackModel ?? value, MODEL_CATALOG_MAX_MODEL);
    return model ? { model } : undefined;
  }
  if (!isRecord(value)) return undefined;
  // For map-shaped catalogs the key is the canonical model id and `name` is
  // commonly only a human-readable label. Prefer explicit model/id/slug,
  // then the map key, and use `name` as a legacy fallback for standalone rows.
  const model = modelCatalogStrongIdentity(value)
    ?? (fallbackModel && isPlausibleModelMapKey(fallbackModel) ? modelCatalogText(fallbackModel, MODEL_CATALOG_MAX_MODEL) : undefined)
    ?? modelCatalogText(value.name, MODEL_CATALOG_MAX_MODEL);
  if (!model) return undefined;

  const result: JsonObject = { model };
  const id = modelCatalogText(value.id, MODEL_CATALOG_MAX_MODEL);
  if (id) result.id = id;
  const displayName = modelCatalogText(value.displayName ?? value.label ?? (fallbackModel ? value.name : undefined));
  if (displayName) result.displayName = displayName;
  const description = modelCatalogText(value.description);
  if (description) result.description = description;
  const specialty = modelCatalogText(value.modelSpecialty);
  if (specialty) result.modelSpecialty = specialty;
  for (const key of ["hidden", "isDefault"] as const) {
    if (typeof value[key] === "boolean") result[key] = value[key];
  }
  const upgrade = modelCatalogText(value.upgrade, MODEL_CATALOG_MAX_MODEL);
  if (upgrade) result.upgrade = upgrade;
  const defaultEffort = modelCatalogText(value.defaultReasoningEffort ?? value.defaultEffort, 64);
  if (defaultEffort) result.defaultReasoningEffort = defaultEffort;
  else if (value.defaultReasoningEffort === null || value.defaultEffort === null) result.defaultReasoningEffort = null;

  const rawEfforts = Array.isArray(value.supportedReasoningEfforts)
    ? value.supportedReasoningEfforts
    : Array.isArray(value.reasoningEfforts)
      ? value.reasoningEfforts
      : Array.isArray(value.efforts) ? value.efforts : undefined;
  const efforts = projectReasoningEfforts(rawEfforts);
  if (efforts) result.supportedReasoningEfforts = efforts;
  return result;
}

function projectReasoningEfforts(value: unknown[] | undefined): JsonValue[] | undefined {
  if (!value) return undefined;
  const seen = new Set<string>();
  const projected: JsonValue[] = [];
  for (const entry of value.slice(0, MODEL_CATALOG_MAX_EFFORTS)) {
    const effort = typeof entry === "string"
      ? modelCatalogText(entry, 64)
      : isRecord(entry)
        ? modelCatalogText(entry.reasoningEffort ?? entry.effort, 64)
        : undefined;
    if (!effort) continue;
    const key = effort.toLowerCase();
    if (seen.has(key)) continue;
    seen.add(key);
    const description = isRecord(entry) ? modelCatalogText(entry.description) : undefined;
    projected.push({
      reasoningEffort: effort,
      ...(description ? { description } : {}),
    });
  }
  return projected.length ? projected : undefined;
}

const TOKEN_USAGE_FIELDS = [
  "totalTokens",
  "inputTokens",
  "cachedInputTokens",
  "cacheWriteInputTokens",
  "outputTokens",
  "reasoningOutputTokens",
] as const;

/** Project the official thread/tokenUsage payload without forwarding limits or account data. */
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
  const contextWindow = tokenNumber(
    source.modelContextWindow
      ?? source.model_context_window
      ?? source.contextWindow
      ?? source.context_window,
  );
  if (!total && !last && contextWindow === undefined) return undefined;
  return {
    ...(total ? { total } : {}),
    ...(last ? { last } : {}),
    ...(contextWindow !== undefined ? { modelContextWindow: contextWindow } : {}),
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

/** Project display-safe thread settings from the opaque IPC state. */
function projectSessionMetadata(state: JsonObject): JsonObject {
  const candidates: unknown[] = [
    state.latestThreadSettings,
    state.threadSettings,
    state.settings,
    isRecord(state.thread) ? state.thread.latestThreadSettings : undefined,
    isRecord(state.thread) ? state.thread.settings : undefined,
  ];
  // Merge compatibility locations from oldest to newest so a partial
  // `latestThreadSettings` record can still inherit provider/permission
  // fields exposed by older state shapes, while the official latest record
  // wins when it contains the same key.
  const settings: Record<string, unknown> = {};
  for (const candidate of candidates.slice().reverse()) {
    if (isRecord(candidate)) Object.assign(settings, candidate);
  }
  const result: JsonObject = {};
  const latestModel = settings.model ?? state.latestModel ?? state.model;
  const latestReasoningEffort = settings.effort !== undefined
    ? settings.effort
    : Object.prototype.hasOwnProperty.call(state, "latestReasoningEffort")
      ? state.latestReasoningEffort
      : state.effort;
  const values: Record<string, unknown> = {
    model: latestModel,
    latestModel,
    modelProvider: settings.modelProvider ?? state.modelProvider,
    approvalPolicy: settings.approvalPolicy ?? state.approvalPolicy,
    approvalsReviewer: settings.approvalsReviewer ?? state.approvalsReviewer,
    sandboxPolicy: settings.sandboxPolicy ?? settings.sandbox ?? state.sandboxPolicy ?? state.sandbox,
    permissions: settings.permissions ?? state.permissions,
    currentPermissions: settings.currentPermissions ?? state.currentPermissions,
    runtimeWorkspaceRoots: settings.runtimeWorkspaceRoots ?? state.runtimeWorkspaceRoots,
    cwd: settings.cwd ?? state.cwd,
    effort: latestReasoningEffort,
    latestReasoningEffort,
    summary: settings.summary ?? state.summary,
  };
  for (const [key, value] of Object.entries(values)) {
    if (value !== undefined && value !== null) result[key] = asJsonValue(value);
  }
  // Preserve an explicit null effort: the official state uses null when a
  // model has no selectable reasoning level, and omission would make the
  // browser retain a stale prior value.
  if (latestReasoningEffort === null) {
    result.effort = null;
    result.latestReasoningEffort = null;
  }
  const tokenUsageSource = [
    state.latestTokenUsageInfo,
    state.tokenUsage,
    state.token_usage,
    settings.latestTokenUsageInfo,
    settings.tokenUsage,
    settings.token_usage,
  ].find((candidate) => candidate !== undefined);
  const tokenUsage = projectTokenUsage(tokenUsageSource);
  if (tokenUsage) {
    // Keep both names: `latestTokenUsageInfo` is the official state key while
    // `tokenUsage` is easier for relay/browser clients to consume.
    result.tokenUsage = tokenUsage;
    result.latestTokenUsageInfo = tokenUsage;
  } else if (tokenUsageSource === null) {
    result.tokenUsage = null;
    result.latestTokenUsageInfo = null;
  }
  if (!result.title && isRecord(state.thread)) {
    const title = state.thread.name ?? state.thread.title ?? state.thread.preview;
    if (title !== undefined && title !== null) result.title = asJsonValue(title);
  }
  const availableModels = projectAvailableModels(state);
  if (availableModels.length) {
    // `availableModels` is the current relay field; `models` preserves the
    // name used by older browser clients and by the app-server response.
    result.availableModels = availableModels;
    result.models = availableModels;
  }
  return result;
}

function stringValue(value: unknown): string | undefined { return typeof value === "string" ? value : undefined; }
function numberValue(value: unknown): number | undefined { return typeof value === "number" && Number.isFinite(value) ? value : undefined; }
function normalizeStatus(value: unknown): string { return typeof value === "string" ? value.replace(/[- ]/g, "_").toLowerCase() : "unknown"; }
function cloneObject(value: JsonObject): JsonObject { return JSON.parse(JSON.stringify(value)) as JsonObject; }

const SECRET_KEY = /(?:token|secret|password|authorization|api[_-]?key|private[_-]?key|refresh)/i;
const SECRET_VALUE = /(?:Bearer\s+)[A-Za-z0-9._~+\-/]+=*|(?:sk-[A-Za-z0-9_-]{12,}|gh[pousr]_[A-Za-z0-9]{12,})/g;
function redactText(text: string): string {
  return text.replace(SECRET_VALUE, "[REDACTED]").replace(/([?&](?:token|key|secret|password|api[_-]?key)=)[^&\s]+/gi, "$1[REDACTED]").replace(/((?:token|secret|password|api[_-]?key)\s*[:=]\s*)[^\s,;]+/gi, "$1[REDACTED]");
}
function redactJson(value: unknown): JsonValue {
  if (Array.isArray(value)) return value.map((item) => redactJson(item));
  if (isRecord(value)) {
    const result: JsonObject = {};
    for (const [key, child] of Object.entries(value)) result[key] = SECRET_KEY.test(key) ? "[REDACTED]" : redactJson(child);
    return result;
  }
  return typeof value === "string" ? redactText(value) : asJsonValue(value);
}
function hashJson(value: JsonValue): string { return createHash("sha256").update(stableStringify(value)).digest("hex"); }
function stableStringify(value: JsonValue): string {
  if (Array.isArray(value)) return `[${value.map(stableStringify).join(",")}]`;
  if (value !== null && typeof value === "object") return `{${Object.keys(value).sort().map((key) => `${JSON.stringify(key)}:${stableStringify(value[key] ?? null)}`).join(",")}}`;
  return JSON.stringify(value);
}
