"use strict";

const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const os = require("node:os");
const path = require("node:path");
const test = require("node:test");

const { CodexIpcAgentAdapter } = require("../vscode-extension/dist/codexIpcAgentAdapter.js");

const THREAD_ID = "11111111-1111-4111-8111-111111111111";
const SECOND_THREAD_ID = "22222222-2222-4222-8222-222222222222";
const STALE_THREAD_ID = "33333333-3333-4333-8333-333333333333";
const THIRD_THREAD_ID = "44444444-4444-4444-8444-444444444444";
const ULID_THREAD_ID = "01a0399e790373d63b6fcde4e1f97d02";

async function waitFor(predicate, timeoutMs = 1_000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (await predicate()) return;
    await new Promise((resolve) => setTimeout(resolve, 5));
  }
  assert.fail(`condition was not met within ${timeoutMs}ms`);
}

class FakeIpcClient {
  constructor(state, emitSnapshot = true) {
    this.socketPath = "/tmp/fake-codex-ipc.sock";
    this.state = state;
    this.emitSnapshot = emitSnapshot;
    this.broadcastListeners = new Set();
    this.streamListeners = new Set();
    this.errorListeners = new Set();
    this.closeListeners = new Set();
    this.calls = [];
    this.streamStates = new Map();
  }

  subscribe(set, listener) {
    set.add(listener);
    return { dispose: () => set.delete(listener) };
  }

  onBroadcast(listener) { return this.subscribe(this.broadcastListeners, listener); }
  onStreamEvent(listener) { return this.subscribe(this.streamListeners, listener); }
  onError(listener) { return this.subscribe(this.errorListeners, listener); }
  onClose(listener) { return this.subscribe(this.closeListeners, listener); }
  getClientId() { return "follower"; }
  getConversationState(threadId) {
    const state = this.streamStates.get(threadId);
    if (!state) return undefined;
    return {
      ...state,
      conversationState: JSON.parse(JSON.stringify(state.conversationState)),
    };
  }
  async connect() { this.calls.push({ method: "connect" }); return "follower"; }
  async findThreadOwner(threadId) {
    this.calls.push({ method: "findThreadOwner", threadId });
    return threadId === THREAD_ID ? "owner" : null;
  }
  async followConversation(threadId, following, options) {
    this.calls.push({ method: "followConversation", threadId, following, options });
    if (!following) {
      this.streamStates.delete(threadId);
      return;
    }
    if (this.emitSnapshot) queueMicrotask(() => this.emitState(this.state, 1, "snapshot", threadId));
  }
  emitState(state, revision, kind = "snapshot", threadId = THREAD_ID, ownerClientId = "owner") {
    const event = {
      kind,
      conversationId: threadId,
      hostId: "local",
      ownerClientId,
      revision,
      conversationState: state,
      raw: { type: "broadcast", method: "thread-stream-state-changed", version: 11 },
    };
    this.streamStates.set(threadId, {
      conversationId: threadId,
      hostId: "local",
      ownerClientId,
      revision,
      conversationState: JSON.parse(JSON.stringify(state)),
    });
    for (const listener of this.streamListeners) listener(event);
  }
  emitFollowing(threadId, following, options = {}) {
    const frame = {
      type: "broadcast",
      method: "thread-stream-following-changed",
      version: options.version ?? 1,
      sourceClientId: options.sourceClientId ?? "owner",
      ...(options.targetClientIds ? { targetClientIds: options.targetClientIds } : {}),
      params: {
        conversationId: threadId,
        hostId: options.hostId ?? "local",
        following,
      },
    };
    for (const listener of this.broadcastListeners) listener(frame);
  }
  emitClientStatus(clientId, status, options = {}) {
    const frame = {
      type: "broadcast",
      method: "client-status-changed",
      version: options.version ?? 0,
      sourceClientId: options.sourceClientId ?? clientId,
      params: {
        clientId,
        clientType: options.clientType ?? "vscode-webview",
        status,
      },
    };
    for (const listener of this.broadcastListeners) listener(frame);
  }
  emitClose(error = new Error("fixture IPC socket closed")) {
    for (const listener of this.closeListeners) listener(error);
  }
  async startTurn(threadId, input, options) {
    this.calls.push({ method: "startTurn", threadId, input, options });
    return { turnId: "turn-new" };
  }
  async steerTurn(threadId, input, options) {
    this.calls.push({ method: "steerTurn", threadId, input, options });
    return { turnId: "turn-new" };
  }
  async updateThreadSettings(threadId, settings, options) {
    this.calls.push({ method: "updateThreadSettings", threadId, settings, options });
    return { updated: true };
  }
  async interruptTurn(threadId, options) {
    this.calls.push({ method: "interruptTurn", threadId, options });
    return { interrupted: true };
  }
  async respondCommandApproval(threadId, requestId, decision, options) {
    this.calls.push({ method: "respondCommandApproval", threadId, requestId, decision, options });
    return { accepted: true };
  }
  async respondFileApproval(threadId, requestId, decision, options) {
    this.calls.push({ method: "respondFileApproval", threadId, requestId, decision, options });
    return { accepted: true };
  }
  async respondPermissionsApproval(threadId, requestId, response, options) {
    this.calls.push({ method: "respondPermissionsApproval", threadId, requestId, response, options });
    return { accepted: true };
  }
  async respondUserInput(threadId, requestId, response, options) {
    this.calls.push({ method: "respondUserInput", threadId, requestId, response, options });
    return { accepted: true };
  }
  async respondMcpElicitation(threadId, requestId, response, options) {
    this.calls.push({ method: "respondMcpElicitation", threadId, requestId, response, options });
    return { accepted: true };
  }
  async loadCompleteHistory() { this.calls.push({ method: "loadCompleteHistory" }); }
  async dispose() { this.calls.push({ method: "dispose" }); }
}

class DiscoveryIpcClient extends FakeIpcClient {
  async findThreadOwner(threadId) {
    this.calls.push({ method: "findThreadOwner", threadId });
    return "owner";
  }
}

class MultiSessionIpcClient extends FakeIpcClient {
  constructor(states, owners = {}) {
    super(states.get(THREAD_ID));
    this.states = states;
    this.owners = owners;
  }

  async findThreadOwner(threadId) {
    this.calls.push({ method: "findThreadOwner", threadId });
    return this.owners[threadId] || null;
  }

  async followConversation(threadId, following, options) {
    this.calls.push({ method: "followConversation", threadId, following, options });
    if (!following) {
      this.streamStates.delete(threadId);
      return;
    }
    if (this.states.has(threadId)) {
      const owner = this.owners[threadId] || "owner";
      queueMicrotask(() => this.emitState(this.states.get(threadId), 1, "snapshot", threadId, owner));
    }
  }
}

/** Simulates a target owned by another Codex process that never answers follow. */
class NoSnapshotSwitchIpcClient extends MultiSessionIpcClient {
  constructor(states, owners = {}) {
    super(states, owners);
    this.targetFollowFailed = false;
  }

  async followConversation(threadId, following, options) {
    this.calls.push({ method: "followConversation", threadId, following, options });
    if (!following) {
      this.streamStates.delete(threadId);
      return;
    }
    if (threadId === SECOND_THREAD_ID) {
      this.targetFollowFailed = true;
      return;
    }
    // The old owner is deliberately silent after the failed target attach;
    // the adapter must restore from its cached stream state instead.
    if (this.targetFollowFailed && threadId === THREAD_ID) return;
    if (this.states.has(threadId)) {
      const owner = this.owners[threadId] || "owner";
      queueMicrotask(() => this.emitState(this.states.get(threadId), 1, "snapshot", threadId, owner));
    }
  }
}

class ThrowingTargetFollowIpcClient extends MultiSessionIpcClient {
  async followConversation(threadId, following, options) {
    if (threadId === SECOND_THREAD_ID && following) {
      this.calls.push({ method: "followConversation", threadId, following, options });
      throw new Error("target follow failed immediately");
    }
    return super.followConversation(threadId, following, options);
  }
}

class OwnerChangingIpcClient extends MultiSessionIpcClient {
  constructor(states, owners = {}) {
    super(states, owners);
    this.targetDiscoveryCount = 0;
  }

  async findThreadOwner(threadId) {
    this.calls.push({ method: "findThreadOwner", threadId });
    if (threadId === SECOND_THREAD_ID) {
      this.targetDiscoveryCount += 1;
      return this.targetDiscoveryCount === 1 ? "owner-b" : "owner-c";
    }
    return this.owners[threadId] || null;
  }
}

/** Delays the first target snapshot so list probing overlaps a session selection request. */
class DelayedProbeIpcClient extends MultiSessionIpcClient {
  constructor(states, owners = {}) {
    super(states, owners);
    this.targetFollowCount = 0;
  }

  async followConversation(threadId, following, options) {
    this.calls.push({ method: "followConversation", threadId, following, options });
    if (!following) {
      this.streamStates.delete(threadId);
      return;
    }
    if (!this.states.has(threadId)) return;
    const owner = this.owners[threadId] || "owner";
    if (threadId === SECOND_THREAD_ID) {
      this.targetFollowCount += 1;
      const delay = this.targetFollowCount === 1 ? 30 : 0;
      setTimeout(() => this.emitState(this.states.get(threadId), this.targetFollowCount, "snapshot", threadId, owner), delay);
      return;
    }
    queueMicrotask(() => this.emitState(this.states.get(threadId), 1, "snapshot", threadId, owner));
  }
}

/** Delays the first waiting attach so a newer official route can supersede it. */
class WaitingRouteRaceIpcClient extends MultiSessionIpcClient {
  async followConversation(threadId, following, options) {
    this.calls.push({ method: "followConversation", threadId, following, options });
    if (!following) {
      this.streamStates.delete(threadId);
      return;
    }
    if (!this.states.has(threadId)) return;
    const owner = this.owners[threadId] || "owner";
    const delay = threadId === THREAD_ID ? 30 : 0;
    setTimeout(() => this.emitState(this.states.get(threadId), 1, "snapshot", threadId, owner), delay);
  }
}

/** Holds fallback discovery so an official route can arrive while it is stale. */
class WaitingPollRouteRaceIpcClient extends WaitingRouteRaceIpcClient {
  constructor(states, owners = {}) {
    super(states, owners);
    this.delayFallbackDiscovery = false;
    this.fallbackDiscoveryStarted = false;
    this.fallbackOwner = new Promise((resolve) => {
      this.resolveFallbackOwner = resolve;
    });
  }

  async findThreadOwner(threadId) {
    this.calls.push({ method: "findThreadOwner", threadId });
    if (threadId === SECOND_THREAD_ID && this.delayFallbackDiscovery) {
      this.fallbackDiscoveryStarted = true;
      return this.fallbackOwner;
    }
    return this.owners[threadId] || null;
  }

  releaseFallbackDiscovery() {
    this.delayFallbackDiscovery = false;
    const resolve = this.resolveFallbackOwner;
    this.resolveFallbackOwner = null;
    if (resolve) resolve(this.owners[SECOND_THREAD_ID] || null);
  }
}

/** Holds the post-snapshot owner confirmation so assertions run mid-switch. */
class DelayedTargetOwnerConfirmationIpcClient extends MultiSessionIpcClient {
  constructor(states, owners = {}) {
    super(states, owners);
    this.targetDiscoveryCount = 0;
    this.targetConfirmationStarted = false;
    this.targetOwnerConfirmation = new Promise((resolve) => {
      this.resolveTargetOwnerConfirmation = resolve;
    });
  }

  async findThreadOwner(threadId) {
    this.calls.push({ method: "findThreadOwner", threadId });
    const owner = this.owners[threadId] || null;
    if (threadId !== SECOND_THREAD_ID) return owner;
    this.targetDiscoveryCount += 1;
    if (this.targetDiscoveryCount === 1) return owner;
    this.targetConfirmationStarted = true;
    return this.targetOwnerConfirmation;
  }

  releaseTargetOwnerConfirmation() {
    if (!this.resolveTargetOwnerConfirmation) return;
    const resolve = this.resolveTargetOwnerConfirmation;
    this.resolveTargetOwnerConfirmation = null;
    resolve(this.owners[SECOND_THREAD_ID] || null);
  }
}

function fixtureState(requests = []) {
  return {
    id: THREAD_ID,
    title: "fixture session",
    cwd: "/tmp/workspace",
    turns: [{
      id: "turn-old",
      status: "completed",
      items: [
        { type: "userMessage", id: "user-1", content: [{ type: "text", text: "hello" }] },
        { type: "agentMessage", id: "agent-1", text: "hi from VS Code" },
      ],
    }],
    requests,
    threadRuntimeStatus: { type: "idle" },
  };
}

test("IPC adapter follows an existing session and routes input/approval without spawning", async () => {
  const client = new FakeIpcClient(fixtureState([
    {
      id: 7,
      method: "item/commandExecution/requestApproval",
      params: { threadId: THREAD_ID, turnId: "turn-old", command: "echo safe" },
    },
    {
      id: "question-1",
      method: "item/tool/requestUserInput",
      params: { threadId: THREAD_ID, turnId: "turn-old", questions: [{ id: "choice", question: "Pick one" }] },
    },
  ]));
  const events = [];
  let newSessionCalls = 0;
  const adapter = new CodexIpcAgentAdapter({
    client,
    threadId: THREAD_ID,
    loadCompleteHistory: false,
    approvalTimeoutMs: 0,
    followTimeoutMs: 500,
    openNewSession: async () => {
      newSessionCalls += 1;
      return { opened: true, command: "chatgpt.newCodexPanel" };
    },
  });
  adapter.onEvent((event) => events.push(event));

  await adapter.start();
  const snapshot = await adapter.snapshot();
  assert.equal(snapshot.threadId, THREAD_ID);
  assert.equal(snapshot.metadata.adapter, "codex-ipc-follower");
  assert.match(snapshot.outputTail, /hi from VS Code/);
  assert.deepEqual(snapshot.pendingApprovals.map((item) => item.requestId), [7]);
  assert.equal(snapshot.pendingRequests.length, 2);
  assert.equal(client.calls.some((call) => call.method === "startProcess"), false);
  await assert.rejects(() => adapter.startThread({}), /does not create a new thread/);
  assert.deepEqual(await adapter.newSession(), { opened: true, command: "chatgpt.newCodexPanel" });
  assert.equal(newSessionCalls, 1);

  await adapter.startTurn({ text: "remote input" });
  const startCall = client.calls.find((call) => call.method === "startTurn");
  assert.deepEqual(startCall.input, "remote input");
  assert.equal(startCall.options.ownerClientId, "owner");

  await adapter.respondApproval(7, "allow");
  const approvalCall = client.calls.find((call) => call.method === "respondCommandApproval");
  assert.equal(approvalCall.decision, "accept");

  await adapter.respondApproval("question-1", "allow", undefined, { answers: { choice: ["yes"] } });
  const inputCall = client.calls.find((call) => call.method === "respondUserInput");
  assert.deepEqual(inputCall.response, { answers: { choice: { answers: ["yes"] } } });
  const outputSnapshot = events.find((event) => event.type === "output.snapshot");
  assert.ok(outputSnapshot);
  assert.deepEqual(outputSnapshot.payload.messages.map((item) => [item.role, item.kind, item.text]), [
    ["user", "user", "hello"],
    ["assistant", "assistant", "hi from VS Code"],
  ]);
  await adapter.dispose();
});

test("IPC adapter clears stale projections on close while preserving closed session identity", async () => {
  const client = new FakeIpcClient(fixtureState());
  const events = [];
  const adapter = new CodexIpcAgentAdapter({
    client,
    threadId: THREAD_ID,
    loadCompleteHistory: false,
    approvalTimeoutMs: 0,
    followTimeoutMs: 500,
  });
  adapter.onEvent((event) => events.push(event));
  await adapter.start();

  const before = await adapter.snapshot();
  assert.equal(before.threadId, THREAD_ID);
  assert.equal(before.metadata.ownerClientId, "owner");
  assert.equal(before.metadata.attachReady, true);
  assert.ok(before.messages.length > 0);
  assert.match(before.outputTail, /hi from VS Code/);
  events.length = 0;

  client.emitClose(new Error("fixture IPC connection lost"));

  const closedEvent = events.find((event) => event.type === "connection.closed");
  assert.ok(closedEvent);
  assert.equal(closedEvent.threadId, THREAD_ID);
  assert.equal(closedEvent.payload.ownerClientId, "owner");
  assert.equal(closedEvent.payload.message, "fixture IPC connection lost");

  const after = await adapter.snapshot();
  assert.equal(after.state, "disconnected");
  assert.equal(after.threadId, null);
  assert.equal(after.turnId, null);
  assert.equal(after.outputTail, "");
  assert.deepEqual(after.messages, []);
  assert.deepEqual(after.subagents, []);
  assert.equal(after.metadata.attachReady, false);
  assert.equal(Object.hasOwn(after.metadata, "ownerClientId"), false);
  assert.equal(Object.hasOwn(after.metadata, "revision"), false);
  await adapter.dispose();
});

test("IPC adapter persists model and effort through the official follower settings envelope", async () => {
  const client = new FakeIpcClient(fixtureState());
  const adapter = new CodexIpcAgentAdapter({
    client,
    threadId: THREAD_ID,
    loadCompleteHistory: false,
    approvalTimeoutMs: 0,
    followTimeoutMs: 500,
  });

  await adapter.start();
  const result = await adapter.updateThreadSettings({
    threadSettings: { model: " gpt-5.6-sol ", effort: " ultra " },
    // UI-only fields must not leak into the owner request.
    commandId: "ignored",
  });
  assert.deepEqual(result, { updated: true });
  const call = client.calls.find((entry) => entry.method === "updateThreadSettings");
  assert.equal(call.threadId, THREAD_ID);
  assert.deepEqual(call.settings, { model: "gpt-5.6-sol", effort: "ultra" });
  assert.equal(call.options.ownerClientId, "owner");
  await assert.rejects(() => adapter.updateThreadSettings({ model: "" }), /non-empty string/);
  await adapter.dispose();
});

test("IPC adapter projects official latest model and reasoning effort fields", async () => {
  const state = fixtureState();
  state.latestModel = "gpt-5.6-sol";
  state.latestReasoningEffort = "ultra";
  state.latestThreadSettings = {
    model: "gpt-5.6-sol",
    modelProvider: "aether",
    effort: "ultra",
    multiAgentMode: "explicitRequestOnly",
  };
  const client = new FakeIpcClient(state);
  const adapter = new CodexIpcAgentAdapter({
    client,
    threadId: THREAD_ID,
    loadCompleteHistory: false,
    approvalTimeoutMs: 0,
    followTimeoutMs: 500,
  });
  await adapter.start();
  const metadata = (await adapter.snapshot()).metadata;
  assert.equal(metadata.model, "gpt-5.6-sol");
  assert.equal(metadata.latestModel, "gpt-5.6-sol");
  assert.equal(metadata.effort, "ultra");
  assert.equal(metadata.latestReasoningEffort, "ultra");
  assert.equal(metadata.modelProvider, "aether");
  await adapter.dispose();
});

test("IPC adapter projects a bounded model catalog from compatible state locations", async () => {
  const state = fixtureState();
  // Exercise all of the state names used by different official extension
  // builds. The same entries should be merged rather than duplicated.
  state.availableModels = [{ model: "gpt-5.6-sol" }];
  state.models = [
    { model: "gpt-5.6-sol", description: "initial description" },
    { id: "gpt-5.6-terra", displayName: "5.6 Terra", efforts: ["low", "medium"] },
  ];
  state.modelCatalog = {
    data: [{
      id: "gpt-5.6-sol",
      model: "gpt-5.6-sol",
      displayName: "5.6 Sol",
      description: "通用 Codex 模型",
      hidden: false,
      isDefault: true,
      defaultReasoningEffort: "medium",
      supportedReasoningEfforts: [
        { reasoningEffort: "low", description: "快速" },
        { reasoningEffort: "high", description: "深入" },
      ],
      apiKey: "sk-this-must-not-cross-the-relay",
      capabilities: { internal: true },
    }],
    nextCursor: "private-cursor",
  };
  state.listModels = { data: [{ model: "gpt-5.6-terra", hidden: false }] };

  const client = new FakeIpcClient(state);
  const adapter = new CodexIpcAgentAdapter({
    client,
    threadId: THREAD_ID,
    loadCompleteHistory: false,
    approvalTimeoutMs: 0,
    followTimeoutMs: 500,
  });
  await adapter.start();

  const metadata = (await adapter.snapshot()).metadata;
  assert.ok(Array.isArray(metadata.availableModels));
  assert.deepEqual(metadata.models, metadata.availableModels);
  assert.equal(metadata.availableModels.length, 2);
  const sol = metadata.availableModels.find((entry) => entry.model === "gpt-5.6-sol");
  assert.deepEqual(sol, {
    model: "gpt-5.6-sol",
    id: "gpt-5.6-sol",
    description: "initial description",
    displayName: "5.6 Sol",
    hidden: false,
    isDefault: true,
    defaultReasoningEffort: "medium",
    supportedReasoningEfforts: [
      { reasoningEffort: "low", description: "快速" },
      { reasoningEffort: "high", description: "深入" },
    ],
  });
  const terra = metadata.availableModels.find((entry) => entry.model === "gpt-5.6-terra");
  assert.deepEqual(terra, {
    model: "gpt-5.6-terra",
    id: "gpt-5.6-terra",
    displayName: "5.6 Terra",
    supportedReasoningEfforts: [
      { reasoningEffort: "low" },
      { reasoningEffort: "medium" },
    ],
    hidden: false,
  });
  assert.equal(JSON.stringify(metadata.availableModels).includes("sk-this-must-not-cross-the-relay"), false);
  assert.equal(JSON.stringify(metadata.availableModels).includes("capabilities"), false);
  await adapter.dispose();
});

test("IPC adapter publishes a snapshot when only thread settings metadata changes", async () => {
  const state = fixtureState();
  state.latestModel = "gpt-5.6-terra";
  state.latestReasoningEffort = "medium";
  state.latestThreadSettings = { model: "gpt-5.6-terra", effort: "medium" };
  const client = new FakeIpcClient(state);
  const events = [];
  const adapter = new CodexIpcAgentAdapter({
    client,
    threadId: THREAD_ID,
    loadCompleteHistory: false,
    approvalTimeoutMs: 0,
    followTimeoutMs: 500,
  });
  adapter.onEvent((event) => events.push(event));
  await adapter.start();
  await new Promise((resolve) => setImmediate(resolve));
  events.length = 0;

  const updated = structuredClone(state);
  updated.latestModel = "gpt-5.6-sol";
  updated.latestReasoningEffort = "ultra";
  updated.latestThreadSettings = { model: "gpt-5.6-sol", effort: "ultra" };
  client.emitState(updated, 2, "patches");
  await new Promise((resolve) => setImmediate(resolve));

  const snapshots = events.filter((event) => event.type === "session.snapshot");
  assert.equal(snapshots.length, 1);
  assert.equal(snapshots[0].payload.metadata.latestModel, "gpt-5.6-sol");
  assert.equal(snapshots[0].payload.metadata.latestReasoningEffort, "ultra");
  await adapter.dispose();
});

test("IPC adapter projects official activity, timestamps, command details, and turn duration", async () => {
  const turnStartedAtMs = Date.now() - 20_000;
  const finalAssistantStartedAtMs = turnStartedAtMs + 15_000;
  const state = fixtureState();
  state.turns[0] = {
    id: "turn-timed",
    status: "completed",
    turnStartedAtMs,
    finalAssistantStartedAtMs,
    durationMs: 18_000,
    commandExecutionStartedAtMsById: { "command-1": turnStartedAtMs + 2_000 },
    items: [
      { type: "userMessage", id: "user-timed", content: [{ type: "text", text: "**run** it" }] },
      {
        type: "commandExecution",
        id: "command-1",
        command: ["/bin/zsh", "-lc", "echo status-test"],
        commandActions: [
          { type: "unknown", command: "/bin/zsh" },
          { type: "unknown", cmd: "echo status-test" },
        ],
        cwd: "/tmp/workspace",
        shellName: "zsh",
        status: "completed",
        aggregatedOutput: "status-test",
        durationMs: 250,
        exitCode: 0,
      },
      { type: "agentMessage", id: "agent-timed", text: "done", phase: "final_answer" },
    ],
  };
  const client = new FakeIpcClient(state);
  const adapter = new CodexIpcAgentAdapter({
    client,
    threadId: THREAD_ID,
    loadCompleteHistory: false,
    approvalTimeoutMs: 0,
    followTimeoutMs: 500,
  });
  await adapter.start();
  const snapshot = await adapter.snapshot();
  assert.equal(snapshot.status.activity, "completed");
  assert.equal(snapshot.durationMs, 18_000);
  const [user, command, assistant] = snapshot.messages;
  assert.equal(user.startedAtMs, turnStartedAtMs);
  assert.equal(command.command, "echo status-test");
  assert.deepEqual(command.commandActions, [
    { type: "unknown", command: "/bin/zsh", cmd: "/bin/zsh" },
    { type: "unknown", cmd: "echo status-test", command: "echo status-test" },
  ]);
  assert.equal(command.cwd, "/tmp/workspace");
  assert.equal(command.shellName, "zsh");
  assert.equal(command.startedAtMs, turnStartedAtMs + 2_000);
  assert.equal(command.durationMs, 250);
  assert.equal(command.exitCode, 0);
  assert.equal(assistant.startedAtMs, finalAssistantStartedAtMs);
  assert.equal(assistant.durationMs, 18_000);

  const active = structuredClone(state);
  active.turns[0].status = "inProgress";
  delete active.turns[0].durationMs;
  active.turns[0].items = [{ type: "reasoning", id: "reasoning-1", summary: ["checking"] }];
  client.emitState(active, 2, "patches");
  const activeSnapshot = await adapter.snapshot();
  assert.equal(activeSnapshot.activity, "thinking");
  assert.equal(activeSnapshot.turnStatus, "inprogress");
  assert.equal(activeSnapshot.turnId, "turn-timed");
  assert.equal(activeSnapshot.messages[0].turnStatus, "inProgress");

  active.turns[0].items = [{
    type: "fileChange",
    id: "edit-1",
    status: "inProgress",
    changes: [{ path: "src/example.ts", diff: "+const remote = true;" }],
  }];
  client.emitState(active, 3, "patches");
  const editingSnapshot = await adapter.snapshot();
  assert.equal(editingSnapshot.activity, "editing");

  active.turns[0].items = [{
    type: "commandExecution",
    id: "command-active",
    status: "inProgress",
    command: "echo running",
  }];
  client.emitState(active, 4, "patches");
  const runningSnapshot = await adapter.snapshot();
  assert.equal(runningSnapshot.activity, "running");

  active.requests = [{
    id: "approval-active",
    method: "item/commandExecution/requestApproval",
    params: { threadId: THREAD_ID, turnId: "turn-timed", command: "echo approve" },
  }];
  client.emitState(active, 5, "patches");
  const waitingSnapshot = await adapter.snapshot();
  assert.equal(waitingSnapshot.activity, "waiting_approval");
  await adapter.dispose();
});

test("IPC adapter keeps command-action-only items visible and suppresses shell bootstraps", async () => {
  const state = fixtureState();
  state.turns[0] = {
    id: "turn-command-actions",
    status: "inProgress",
    items: [
      {
        // Older snapshots can expose a generic shell item while retaining
        // the official commandActions payload.
        type: "shell",
        id: "command-actions-only",
        status: "inProgress",
        aggregatedOutput: "",
        commandActions: [
          { type: "unknown", command: "/bin/zsh" },
          { type: "search", cmd: "rg --files", path: "src" },
        ],
        cwd: "/tmp/workspace",
        shellName: "zsh",
      },
      {
        type: "commandExecution",
        id: "shell-bootstrap-only",
        status: "inProgress",
        aggregatedOutput: "",
        command: "/bin/zsh",
        commandActions: [{ type: "unknown", command: "/bin/zsh" }],
      },
      {
        type: "commandExecution",
        id: "wrapped-command",
        status: "inProgress",
        aggregatedOutput: "",
        command: "/bin/zsh -lc 'printf wrapped'",
      },
      {
        type: "commandExecution",
        id: "wrapped-action",
        status: "inProgress",
        aggregatedOutput: "",
        commandActions: ["/bin/zsh", "/bin/zsh -lc 'printf action-wrapped'"],
      },
    ],
  };
  const client = new FakeIpcClient(state);
  const adapter = new CodexIpcAgentAdapter({
    client,
    threadId: THREAD_ID,
    loadCompleteHistory: false,
    approvalTimeoutMs: 0,
    followTimeoutMs: 500,
  });
  await adapter.start();

  const snapshot = await adapter.snapshot();
  assert.equal(snapshot.messages.length, 3);
  const actionsOnly = snapshot.messages.find((message) => message.itemId === "command-actions-only");
  assert.ok(actionsOnly);
  assert.equal(actionsOnly.command, "rg --files");
  assert.equal(actionsOnly.text, "rg --files");
  assert.equal(actionsOnly.cwd, "/tmp/workspace");
  assert.equal(actionsOnly.shellName, "zsh");
  const wrapped = snapshot.messages.find((message) => message.itemId === "wrapped-command");
  assert.ok(wrapped);
  assert.equal(wrapped.command, "'printf wrapped'");
  assert.equal(wrapped.text, "'printf wrapped'");
  const wrappedAction = snapshot.messages.find((message) => message.itemId === "wrapped-action");
  assert.ok(wrappedAction);
  assert.equal(wrappedAction.command, "'printf action-wrapped'");
  assert.equal(wrappedAction.text, "'printf action-wrapped'");
  assert.doesNotMatch(snapshot.outputTail, /\/bin\/zsh/);
  await adapter.dispose();
});

test("IPC adapter projects collab items, metadata envelopes, and subagent lifecycle", async () => {
  const childThreadId = "22222222-2222-4222-8222-222222222222";
  const state = fixtureState();
  state.id = THREAD_ID;
  state.turns[0] = {
    id: "turn-subagents",
    status: "inProgress",
    turnStartedAtMs: Date.now() - 2_000,
    items: [
      { type: "userMessage", id: "user-subagents", content: [{ type: "text", text: "inspect this" }] },
      {
        type: "agentMessage",
        id: "message-with-collab-metadata",
        text: "I am delegating this review",
        metadata: {
          codex_collab_agent_tool_call: {
            type: "collabAgentToolCall",
            id: "collab-spawn-1",
            tool: "spawnAgent",
            status: "inProgress",
            senderThreadId: THREAD_ID,
            receiverThreadIds: [childThreadId],
            prompt: "Inspect the tests",
            model: "gpt-5.6",
            reasoningEffort: "high",
            agentsStates: { [childThreadId]: { status: "running", message: null } },
          },
        },
      },
      {
        type: "subAgentActivity",
        id: "activity-started-1",
        kind: "started",
        agentThreadId: childThreadId,
        agentPath: "root/reviewer_agent",
      },
    ],
  };
  const client = new FakeIpcClient(state);
  const adapter = new CodexIpcAgentAdapter({
    client,
    threadId: THREAD_ID,
    loadCompleteHistory: false,
    approvalTimeoutMs: 0,
    followTimeoutMs: 500,
  });
  await adapter.start();

  let snapshot = await adapter.snapshot();
  assert.equal(snapshot.subagents.length, 1);
  assert.equal(snapshot.subagents[0].threadId, childThreadId);
  assert.equal(snapshot.subagents[0].displayName, "Reviewer agent");
  assert.equal(snapshot.subagents[0].prompt, "Inspect the tests");
  assert.equal(snapshot.subagents[0].objective, "Inspect the tests");
  assert.equal(snapshot.subagents[0].status, "working");
  assert.equal(snapshot.subagents[0].model, "gpt-5.6");
  assert.equal(snapshot.subagents[0].canInteract, true);
  assert.ok(snapshot.messages.some((message) => message.itemType === "collabAgentToolCall" && message.uiType === "multi-agent-action"));
  assert.ok(snapshot.messages.some((message) => message.itemType === "subAgentActivity" && message.uiType === "subagent-activity"));

  const completed = structuredClone(state);
  completed.turns[0].status = "completed";
  completed.turns[0].items.push({
    type: "collabAgentToolCall",
    id: "collab-wait-1",
    tool: "wait",
    status: "completed",
    senderThreadId: THREAD_ID,
    receiverThreadIds: [],
    prompt: null,
    model: null,
    reasoningEffort: null,
    agentsStates: {},
  });
  client.emitState(completed, 2, "patches");
  snapshot = await adapter.snapshot();
  assert.equal(snapshot.subagents[0].status, "done");
  await adapter.dispose();
});

test("IPC adapter discovers pending requests retained inside official turn items", async () => {
  const state = fixtureState();
  state.turns[0].status = "inProgress";
  state.turns[0].items.push({
    type: "permission-request",
    id: "permission-in-turn",
    threadId: THREAD_ID,
    turnId: "turn-old",
    summary: "需要访问工作区",
    permissions: { fileSystem: { write: true } },
  });
  const client = new FakeIpcClient(state);
  const adapter = new CodexIpcAgentAdapter({
    client,
    threadId: THREAD_ID,
    loadCompleteHistory: false,
    approvalTimeoutMs: 0,
    followTimeoutMs: 500,
  });
  await adapter.start();
  const snapshot = await adapter.snapshot();
  assert.equal(snapshot.pendingRequests.length, 1);
  assert.equal(snapshot.pendingRequests[0].requestId, "permission-in-turn");
  assert.equal(snapshot.pendingRequests[0].method, "item/permissions/requestApproval");
  assert.deepEqual(snapshot.pendingRequests[0].params.permissions, { fileSystem: { write: true } });
  assert.equal(snapshot.messages.some((message) => message && message.itemId === "permission-in-turn"), false);
  await adapter.dispose();
});

test("IPC adapter expires unanswered requests and rolls back a failed follow", async () => {
  const client = new FakeIpcClient(fixtureState([{
    id: 8,
    method: "item/commandExecution/requestApproval",
    params: { threadId: THREAD_ID, command: "echo timeout" },
  }]));
  const events = [];
  const adapter = new CodexIpcAgentAdapter({
    client,
    threadId: THREAD_ID,
    loadCompleteHistory: false,
    approvalTimeoutMs: 25,
    followTimeoutMs: 500,
  });
  adapter.onEvent((event) => events.push(event));
  await adapter.start();
  await new Promise((resolve) => setTimeout(resolve, 60));
  assert.ok(events.some((event) => event.type === "approval.expired" && event.requestId === 8));
  assert.equal((await adapter.snapshot()).pendingApprovals.length, 0);
  const expiryResponse = client.calls.find((call) => call.method === "respondCommandApproval");
  assert.equal(expiryResponse.decision, "decline");
  await adapter.dispose();

  const noSnapshotClient = new FakeIpcClient(fixtureState(), false);
  const failed = new CodexIpcAgentAdapter({
    client: noSnapshotClient,
    threadId: THREAD_ID,
    loadCompleteHistory: false,
    approvalTimeoutMs: 0,
    followTimeoutMs: 15,
  });
  await assert.rejects(() => failed.start(), /Timed out waiting for a snapshot/);
  assert.equal((await failed.snapshot()).state, "disconnected");
  assert.equal((await failed.snapshot()).threadId, null);
  await failed.dispose();
});

test("IPC adapter stays available while waiting for the first VS Code Codex session", async (t) => {
  // An empty rollout directory represents a freshly opened VS Code window
  // whose Codex panel has not created/selected a conversation yet. Starting
  // the relay must remain possible so a later panel navigation can attach
  // without restarting the bridge.
  const codexHome = await fs.mkdtemp(path.join(os.tmpdir(), "codex-ipc-no-session-"));
  t.after(() => fs.rm(codexHome, { recursive: true, force: true }));
  const client = new FakeIpcClient(fixtureState());
  const events = [];
  const adapter = new CodexIpcAgentAdapter({
    client,
    codexHome,
    loadCompleteHistory: false,
    approvalTimeoutMs: 0,
    ownerDiscoveryTimeoutMs: 25,
    followTimeoutMs: 50,
  });
  adapter.onEvent((event) => events.push(event));

  await assert.doesNotReject(() => adapter.start());
  const snapshot = await adapter.snapshot();
  assert.equal(snapshot.threadId, null);
  assert.equal(snapshot.state, "waiting_for_host");
  assert.equal(snapshot.metadata?.waitingForSession, true);
  assert.equal(snapshot.metadata?.attachReady, false);
  assert.equal(client.calls.some((call) => call.method === "followConversation" && call.following === true), false);
  assert.ok(events.some((event) => event.type === "connection.opened" && event.threadId === undefined));

  await adapter.dispose();
});

test("IPC adapter attaches in-place when the official panel selects a session after waiting", async (t) => {
  const codexHome = await fs.mkdtemp(path.join(os.tmpdir(), "codex-ipc-route-after-wait-"));
  t.after(() => fs.rm(codexHome, { recursive: true, force: true }));
  const client = new FakeIpcClient(fixtureState());
  const events = [];
  const adapter = new CodexIpcAgentAdapter({
    client,
    codexHome,
    loadCompleteHistory: false,
    approvalTimeoutMs: 0,
    ownerDiscoveryTimeoutMs: 100,
    followTimeoutMs: 250,
  });
  adapter.onEvent((event) => events.push(event));

  await adapter.start();
  assert.equal((await adapter.snapshot()).state, "waiting_for_host");
  // The official webview broadcasts this untargeted route update when the
  // user opens a conversation. The bridge should attach over the same IPC
  // client rather than asking the user to restart the command.
  client.emitFollowing(THREAD_ID, true, { sourceClientId: "official-vscode-panel" });
  await waitFor(async () => (await adapter.snapshot()).threadId === THREAD_ID);

  const snapshot = await adapter.snapshot();
  assert.equal(snapshot.state, "idle");
  assert.equal(snapshot.metadata.waitingForSession, false);
  assert.equal(snapshot.metadata.attachReady, true);
  assert.equal(client.calls.filter((call) => call.method === "followConversation" && call.following === true).length, 1);
  assert.equal(events.filter((event) => event.type === "connection.opened").length, 1);
  assert.ok(events.some((event) => event.type === "session.snapshot" && event.threadId === THREAD_ID));
  await adapter.dispose();
});

test("IPC waiting attach follows the latest official route when selection changes mid-snapshot", async (t) => {
  const codexHome = await fs.mkdtemp(path.join(os.tmpdir(), "codex-ipc-wait-route-race-"));
  t.after(() => fs.rm(codexHome, { recursive: true, force: true }));
  const states = new Map([
    [THREAD_ID, fixtureState()],
    [SECOND_THREAD_ID, { ...fixtureState(), id: SECOND_THREAD_ID, title: "latest route" }],
  ]);
  const client = new WaitingRouteRaceIpcClient(states, {
    [THREAD_ID]: "owner-a",
    [SECOND_THREAD_ID]: "owner-b",
  });
  const adapter = new CodexIpcAgentAdapter({
    client,
    codexHome,
    loadCompleteHistory: false,
    approvalTimeoutMs: 0,
    ownerDiscoveryTimeoutMs: 100,
    followTimeoutMs: 250,
    vscodeSessionFollowDebounceMs: 0,
  });

  await adapter.start();
  client.emitFollowing(THREAD_ID, true, { sourceClientId: "official-vscode-panel" });
  await waitFor(() => client.calls.some((call) => call.method === "followConversation"
    && call.threadId === THREAD_ID && call.following === true));
  client.emitFollowing(SECOND_THREAD_ID, true, { sourceClientId: "official-vscode-panel" });
  await waitFor(async () => {
    const snapshot = await adapter.snapshot();
    return snapshot.threadId === SECOND_THREAD_ID && snapshot.metadata.attachReady === true;
  });

  const snapshot = await adapter.snapshot();
  assert.equal(snapshot.metadata.attachReady, true);
  assert.equal(snapshot.metadata.title, "latest route");
  assert.ok(client.calls.some((call) => call.method === "followConversation"
    && call.threadId === SECOND_THREAD_ID && call.following === true));
  await adapter.dispose();
});

test("IPC waiting discovery never overrides a newer official VS Code route", async (t) => {
  const codexHome = await fs.mkdtemp(path.join(os.tmpdir(), "codex-ipc-wait-poll-route-race-"));
  t.after(() => fs.rm(codexHome, { recursive: true, force: true }));
  const states = new Map([
    [THREAD_ID, fixtureState()],
    [SECOND_THREAD_ID, { ...fixtureState(), id: SECOND_THREAD_ID, title: "stale fallback" }],
  ]);
  const client = new WaitingPollRouteRaceIpcClient(states, {
    [THREAD_ID]: "owner-a",
    [SECOND_THREAD_ID]: "owner-b",
  });
  const adapter = new CodexIpcAgentAdapter({
    client,
    codexHome,
    loadCompleteHistory: false,
    approvalTimeoutMs: 0,
    ownerDiscoveryTimeoutMs: 100,
    followTimeoutMs: 250,
    vscodeSessionFollowDebounceMs: 0,
  });

  await adapter.start();
  assert.equal((await adapter.snapshot()).state, "waiting_for_host");

  const sessions = path.join(codexHome, "sessions", "2026", "08");
  await fs.mkdir(sessions, { recursive: true });
  await fs.writeFile(path.join(sessions, `rollout-${SECOND_THREAD_ID}.jsonl`), `${JSON.stringify({
    type: "session_meta",
    payload: { originator: "codex_vscode", source: "vscode", cwd: "/tmp/stale-fallback" },
  })}\n`);

  client.delayFallbackDiscovery = true;
  const fallbackDiscovery = adapter.runWaitingDiscovery();
  await waitFor(() => client.fallbackDiscoveryStarted);
  client.emitFollowing(THREAD_ID, true, { sourceClientId: "official-vscode-panel" });
  await waitFor(() => client.calls.some((call) => call.method === "followConversation"
    && call.threadId === THREAD_ID && call.following === true));
  client.releaseFallbackDiscovery();
  await fallbackDiscovery;
  await waitFor(async () => (await adapter.snapshot()).metadata.attachReady === true);

  const snapshot = await adapter.snapshot();
  assert.equal(snapshot.threadId, THREAD_ID);
  assert.equal(snapshot.metadata.title, "fixture session");
  assert.equal(client.calls.some((call) => call.method === "followConversation"
    && call.threadId === SECOND_THREAD_ID && call.following === true), false);
  await adapter.dispose();
});

test("IPC adapter waits when the configured VS Code conversation has no live owner", async () => {
  const client = new FakeIpcClient(fixtureState());
  const adapter = new CodexIpcAgentAdapter({
    client,
    threadId: STALE_THREAD_ID,
    autoDiscoverThread: false,
    loadCompleteHistory: false,
    approvalTimeoutMs: 0,
    ownerDiscoveryTimeoutMs: 25,
    followTimeoutMs: 50,
  });

  await assert.doesNotReject(() => adapter.start());
  const snapshot = await adapter.snapshot();
  assert.equal(snapshot.threadId, null);
  assert.equal(snapshot.state, "waiting_for_host");
  assert.equal(snapshot.metadata?.waitingForSession, true);
  assert.equal(snapshot.metadata?.attachReady, false);
  assert.ok(client.calls.some((call) => call.method === "findThreadOwner" && call.threadId === STALE_THREAD_ID));
  assert.equal(client.calls.some((call) => call.method === "followConversation" && call.following === true), false);

  await adapter.dispose();
});

test("IPC adapter preserves official request timestamps and streams a sliding output tail as a delta", async () => {
  const startedAtMs = Date.now() - 200;
  const initial = fixtureState([{
    id: 9,
    method: "item/commandExecution/requestApproval",
    createdAt: Date.now(),
    params: { threadId: THREAD_ID, command: "echo timestamp", startedAtMs },
  }]);
  initial.turns[0].items = [{ type: "agentMessage", id: "streaming", text: "x".repeat(40) }];
  const client = new FakeIpcClient(initial);
  const events = [];
  const adapter = new CodexIpcAgentAdapter({
    client,
    threadId: THREAD_ID,
    loadCompleteHistory: false,
    approvalTimeoutMs: 10_000,
    maxOutputTailChars: 32,
    followTimeoutMs: 500,
  });
  adapter.onEvent((event) => events.push(event));
  await adapter.start();
  const pending = (await adapter.snapshot()).pendingApprovals[0];
  assert.equal(pending.createdAt, startedAtMs);

  const next = structuredClone(initial);
  next.turns[0].items[0].text += "y";
  client.emitState(next, 2);
  const outputEvents = events.filter((event) => event.type === "output.snapshot" || event.type === "output.chunk");
  assert.equal(outputEvents.at(-1).type, "output.chunk");
  assert.equal(outputEvents.at(-1).payload.text, "y");
  assert.equal(outputEvents.at(-1).payload.messages, undefined);
  assert.equal(outputEvents.at(-1).payload.messagesPatch.start, 0);
  assert.equal(outputEvents.at(-1).payload.messagesPatch.deleteCount, 1);
  assert.equal(outputEvents.at(-1).payload.messagesPatch.messages[0].text, `${"x".repeat(40)}y`);

  // The bounded tail can remain byte-for-byte identical when a repeated
  // character arrives. It must still advance by one chunk using total length.
  const repeated = structuredClone(next);
  repeated.turns[0].items[0].text += "x";
  client.emitState(repeated, 3);
  const repeatedEvent = events.filter((event) => event.type === "output.snapshot" || event.type === "output.chunk").at(-1);
  assert.equal(repeatedEvent.type, "output.chunk");
  assert.equal(repeatedEvent.payload.text, "x");
  assert.equal(repeatedEvent.payload.messagesPatch.messages[0].text, `${"x".repeat(40)}yx`);
  await adapter.dispose();
});

test("IPC adapter normalizes epoch-second approval timestamps", async () => {
  const nowSeconds = Math.floor(Date.now() / 1000);
  const startedSeconds = nowSeconds - 2;
  const expiresSeconds = nowSeconds + 60;
  const initial = fixtureState([
    {
      id: 10,
      method: "item/commandExecution/requestApproval",
      createdAt: startedSeconds,
      expiresAt: expiresSeconds,
      params: { threadId: THREAD_ID, command: "echo outer seconds" },
    },
    {
      id: 11,
      method: "item/commandExecution/requestApproval",
      params: {
        threadId: THREAD_ID,
        command: "echo nested seconds",
        startedAt: String(startedSeconds),
        expiresAt: String(expiresSeconds),
      },
    },
  ]);
  const client = new FakeIpcClient(initial);
  const events = [];
  const adapter = new CodexIpcAgentAdapter({
    client,
    threadId: THREAD_ID,
    loadCompleteHistory: false,
    approvalTimeoutMs: 10_000,
    followTimeoutMs: 500,
  });
  adapter.onEvent((event) => events.push(event));

  await adapter.start();
  const pending = (await adapter.snapshot()).pendingApprovals;
  assert.equal(pending.length, 2);
  for (const approval of pending) {
    assert.equal(approval.createdAt, startedSeconds * 1000);
    assert.equal(approval.expiresAt, expiresSeconds * 1000);
  }
  assert.equal(events.some((event) => event.type === "approval.expired"), false);
  await adapter.dispose();
});

test("IPC adapter waits for the revision returned by complete-history loading", async () => {
  const initial = fixtureState();
  initial.turnsPagination = { olderCursor: "older", hasLoadedOldest: false, isLoadingOlder: false };
  const complete = fixtureState();
  complete.turnsPagination = { olderCursor: null, hasLoadedOldest: true, isLoadingOlder: false };
  class HistoryClient extends FakeIpcClient {
    async loadCompleteHistory() {
      this.calls.push({ method: "loadCompleteHistory" });
      // The owner can acknowledge the request before the stream broadcast.
      // An unrelated owner's revision must not release our waiter.
      setTimeout(() => this.emitState(initial, 2, "snapshot", THREAD_ID, "other-owner"), 0);
      setTimeout(() => this.emitState(complete, 2, "snapshot", THREAD_ID, "owner"), 10);
      return { revision: 2 };
    }
  }
  const client = new HistoryClient(initial);
  const adapter = new CodexIpcAgentAdapter({ client, threadId: THREAD_ID, followTimeoutMs: 500, approvalTimeoutMs: 0 });
  await adapter.start();
  await new Promise((resolve) => setTimeout(resolve, 5));
  // The unrelated owner event at t=0 must not overwrite the attached stream.
  assert.equal((await adapter.snapshot()).metadata.historyComplete, false);
  await new Promise((resolve) => setTimeout(resolve, 20));
  const snapshot = await adapter.snapshot();
  assert.equal(snapshot.metadata.historyComplete, true);
  assert.equal(client.calls.filter((call) => call.method === "loadCompleteHistory").length, 1);
  await adapter.dispose();
});

test("IPC adapter retries complete-history loading once after a transient failure", async () => {
  const initial = fixtureState();
  initial.turnsPagination = { olderCursor: "older", hasLoadedOldest: false, isLoadingOlder: false };
  const complete = fixtureState();
  complete.turnsPagination = { olderCursor: null, hasLoadedOldest: true, isLoadingOlder: false };
  class RetryingHistoryClient extends FakeIpcClient {
    async loadCompleteHistory() {
      this.calls.push({ method: "loadCompleteHistory" });
      const attempts = this.calls.filter((call) => call.method === "loadCompleteHistory").length;
      if (attempts === 1) throw Object.assign(new Error("history request timed out"), { code: "timeout" });
      setTimeout(() => this.emitState(complete, 2, "snapshot", THREAD_ID, "owner"), 5);
      return { revision: 2 };
    }
  }
  const client = new RetryingHistoryClient(initial);
  const adapter = new CodexIpcAgentAdapter({ client, threadId: THREAD_ID, followTimeoutMs: 500, approvalTimeoutMs: 0 });
  await adapter.start();
  await new Promise((resolve) => setTimeout(resolve, 100));
  assert.equal(client.calls.filter((call) => call.method === "loadCompleteHistory").length, 2);
  assert.equal((await adapter.snapshot()).metadata.historyComplete, true);
  await adapter.dispose();
});

test("IPC adapter cancels a pending history retry when switching sessions", async () => {
  const initial = fixtureState();
  initial.turnsPagination = { olderCursor: "older", hasLoadedOldest: false, isLoadingOlder: false };
  const states = new Map([[THREAD_ID, initial], [SECOND_THREAD_ID, fixtureState()]]);
  class SwitchingHistoryClient extends MultiSessionIpcClient {
    async loadCompleteHistory(threadId) {
      this.calls.push({ method: "loadCompleteHistory", threadId });
      throw Object.assign(new Error("history request timed out"), { code: "timeout" });
    }
  }
  const client = new SwitchingHistoryClient(states, {
    [THREAD_ID]: "owner-a",
    [SECOND_THREAD_ID]: "owner-b",
  });
  const adapter = new CodexIpcAgentAdapter({ client, threadId: THREAD_ID, followTimeoutMs: 500, approvalTimeoutMs: 0 });
  await adapter.start();
  await new Promise((resolve) => setImmediate(resolve));
  await adapter.selectSession({ threadId: SECOND_THREAD_ID });
  await new Promise((resolve) => setTimeout(resolve, 80));
  assert.deepEqual(client.calls.filter((call) => call.method === "loadCompleteHistory").map((call) => call.threadId), [THREAD_ID]);
  assert.equal((await adapter.snapshot()).threadId, SECOND_THREAD_ID);
  await adapter.dispose();
});

test("IPC adapter cancels a pending history retry when disposed", async () => {
  const initial = fixtureState();
  initial.turnsPagination = { olderCursor: "older", hasLoadedOldest: false, isLoadingOlder: false };
  class DisposedHistoryClient extends FakeIpcClient {
    async loadCompleteHistory() {
      this.calls.push({ method: "loadCompleteHistory" });
      throw Object.assign(new Error("history request timed out"), { code: "timeout" });
    }
  }
  const client = new DisposedHistoryClient(initial);
  const adapter = new CodexIpcAgentAdapter({ client, threadId: THREAD_ID, followTimeoutMs: 500, approvalTimeoutMs: 0 });
  await adapter.start();
  await new Promise((resolve) => setImmediate(resolve));
  await adapter.dispose();
  await new Promise((resolve) => setTimeout(resolve, 80));
  assert.equal(client.calls.filter((call) => call.method === "loadCompleteHistory").length, 1);
});

test("IPC adapter reports canonical history incomplete until one complete island and all item pages", async () => {
  const state = fixtureState();
  state.turns = [];
  state.turnHistory = {
    kind: "canonical",
    history: {
      isComplete: true,
      islands: [{ entries: [] }, { entries: [] }],
      entitiesByKey: {
        "turn:1": {
          id: "turn-1",
          status: "completed",
          items: [],
          itemsPagination: { hasLoadedOldest: true },
        },
      },
    },
  };
  const client = new FakeIpcClient(state);
  const events = [];
  const adapter = new CodexIpcAgentAdapter({
    client,
    threadId: THREAD_ID,
    loadCompleteHistory: false,
    approvalTimeoutMs: 0,
    followTimeoutMs: 500,
  });
  adapter.onEvent((event) => events.push(event));
  await adapter.start();
  assert.equal((await adapter.snapshot()).metadata.historyComplete, false);

  const itemPagePending = structuredClone(state);
  itemPagePending.turnHistory.history.islands = [{ entries: [] }];
  itemPagePending.turnHistory.history.entitiesByKey["turn:1"].itemsPagination.hasLoadedOldest = false;
  client.emitState(itemPagePending, 2);
  assert.equal((await adapter.snapshot()).metadata.historyComplete, false);

  const complete = structuredClone(itemPagePending);
  complete.turnHistory.history.entitiesByKey["turn:1"].itemsPagination.hasLoadedOldest = true;
  events.length = 0;
  client.emitState(complete, 3, "patches");
  assert.equal((await adapter.snapshot()).metadata.historyComplete, true);
  await new Promise((resolve) => setImmediate(resolve));
  assert.ok(events.some((event) => event.type === "session.snapshot"));
  await adapter.dispose();
});

test("IPC auto-discovery excludes subagents and Codex Desktop tasks", async (t) => {
  const codexHome = await fs.mkdtemp(path.join(os.tmpdir(), "codex-ipc-discovery-"));
  t.after(() => fs.rm(codexHome, { recursive: true, force: true }));
  const sessions = path.join(codexHome, "sessions", "2026", "08");
  await fs.mkdir(sessions, { recursive: true });

  const preferredCwd = path.join(codexHome, "current-workspace");
  const subagentId = "22222222-2222-4222-8222-222222222222";
  const otherWorkspaceId = "33333333-3333-4333-8333-333333333333";
  const matchingDesktopId = "44444444-4444-4444-8444-444444444444";
  const matchingVscodeId = "55555555-5555-4555-8555-555555555555";
  const writeRollout = async (id, payload, mtimeSeconds) => {
    const fileName = path.join(sessions, `rollout-2026-08-28T00-00-00-${id}.jsonl`);
    await fs.writeFile(fileName, `${JSON.stringify({ type: "session_meta", payload })}\n`);
    await fs.utimes(fileName, mtimeSeconds, mtimeSeconds);
  };

  await writeRollout(subagentId, {
    originator: "codex_vscode",
    source: { subagent: { thread_spawn: {} } },
    thread_source: "subagent",
    cwd: preferredCwd,
  }, 3_000);
  await writeRollout(otherWorkspaceId, {
    originator: "codex_vscode",
    source: "vscode",
    thread_source: "user",
    cwd: path.join(codexHome, "other-workspace"),
  }, 2_000);
  await writeRollout(matchingDesktopId, {
    originator: "Codex Desktop",
    source: "vscode",
    thread_source: "user",
    cwd: preferredCwd,
  }, 4_000);
  await writeRollout(matchingVscodeId, {
    originator: "codex_vscode",
    source: "vscode",
    thread_source: "user",
    cwd: preferredCwd,
  }, 1_000);

  const client = new DiscoveryIpcClient(fixtureState());
  const adapter = new CodexIpcAgentAdapter({
    client,
    codexHome,
    preferredCwds: [preferredCwd],
    loadCompleteHistory: false,
    approvalTimeoutMs: 0,
    followTimeoutMs: 500,
  });

  await adapter.start();
  assert.equal((await adapter.snapshot()).threadId, matchingVscodeId);
  const discovered = client.calls
    .filter((call) => call.method === "findThreadOwner")
    .map((call) => call.threadId);
  assert.equal(discovered.includes(subagentId), false);
  assert.equal(discovered.includes(matchingDesktopId), false);
  await adapter.dispose();
});

test("IPC auto-discovery finds an older live conversation beyond the first twelve rollouts", async (t) => {
  const codexHome = await fs.mkdtemp(path.join(os.tmpdir(), "codex-ipc-deep-discovery-"));
  t.after(() => fs.rm(codexHome, { recursive: true, force: true }));
  const sessions = path.join(codexHome, "sessions", "2026", "08");
  await fs.mkdir(sessions, { recursive: true });
  const staleIds = Array.from({ length: 20 }, (_, index) => `aaaaaaaa-aaaa-4aaa-8aaa-${String(index).padStart(12, "0")}`);
  for (const [index, id] of staleIds.entries()) {
    await fs.writeFile(path.join(sessions, `rollout-${id}.jsonl`), `${JSON.stringify({
      type: "session_meta",
      payload: {
        originator: "codex_vscode",
        source: "vscode",
        cwd: `/tmp/stale-${index}`,
        updated_at: `2026-08-29T${String(20 - index).padStart(2, "0")}:00:00Z`,
      },
    })}\n`);
  }
  const liveRollout = path.join(sessions, `rollout-${THREAD_ID}.jsonl`);
  await fs.writeFile(liveRollout, `${JSON.stringify({
    type: "session_meta",
    payload: {
      originator: "codex_vscode",
      source: "vscode",
      cwd: "/tmp/live-old",
      updated_at: "2026-08-01T00:00:00Z",
    },
  })}\n`);
  const oldMtime = new Date("2026-08-01T00:00:00Z");
  await fs.utimes(liveRollout, oldMtime, oldMtime);

  const client = new FakeIpcClient(fixtureState());
  const adapter = new CodexIpcAgentAdapter({
    client,
    codexHome,
    loadCompleteHistory: false,
    approvalTimeoutMs: 0,
    followTimeoutMs: 500,
  });
  await adapter.start();

  assert.equal((await adapter.snapshot()).threadId, THREAD_ID);
  const discoveryCalls = client.calls.filter((call) => call.method === "findThreadOwner");
  assert.ok(discoveryCalls.length > 12);
  assert.ok(discoveryCalls.some((call) => call.threadId === THREAD_ID));
  await adapter.dispose();
});

test("IPC adapter lists only indexed VS Code sessions with live attachable snapshots", async (t) => {
  const codexHome = await fs.mkdtemp(path.join(os.tmpdir(), "codex-ipc-session-list-"));
  t.after(() => fs.rm(codexHome, { recursive: true, force: true }));
  const sessions = path.join(codexHome, "sessions", "2026", "08");
  await fs.mkdir(sessions, { recursive: true });
  const writeRollout = async (id, payload) => {
    const fileName = path.join(sessions, `rollout-${id}.jsonl`);
    await fs.writeFile(fileName, `${JSON.stringify({ type: "session_meta", payload })}\n`);
  };
  await writeRollout(THREAD_ID, { originator: "codex_vscode", source: "vscode", cwd: "/tmp/a" });
  await writeRollout(SECOND_THREAD_ID, { originator: "codex_vscode", source: "vscode", cwd: "/tmp/b" });
  await writeRollout(STALE_THREAD_ID, { originator: "codex_vscode", source: "vscode", cwd: "/tmp/c" });
  await fs.writeFile(path.join(codexHome, "session_index.jsonl"), [
    JSON.stringify({ id: THREAD_ID, thread_name: "当前会话", updated_at: "2026-08-29T10:00:00Z" }),
    JSON.stringify({ id: SECOND_THREAD_ID, thread_name: "另一个工作区", updated_at: "2026-08-29T11:00:00Z" }),
    JSON.stringify({ id: STALE_THREAD_ID, thread_name: "已关闭会话", updated_at: "2026-08-29T12:00:00Z" }),
  ].join("\n") + "\n");

  const states = new Map([
    [THREAD_ID, fixtureState()],
    [SECOND_THREAD_ID, { ...fixtureState(), id: SECOND_THREAD_ID, title: "另一个工作区" }],
  ]);
  const client = new MultiSessionIpcClient(states, {
    [THREAD_ID]: "owner-a",
    [SECOND_THREAD_ID]: "owner-b",
  });
  const adapter = new CodexIpcAgentAdapter({
    client,
    threadId: THREAD_ID,
    codexHome,
    loadCompleteHistory: false,
    approvalTimeoutMs: 0,
    followTimeoutMs: 500,
  });
  await adapter.start();
  const result = await adapter.listSessions({ limit: 10 });
  assert.equal(result.activeThreadId, THREAD_ID);
  assert.deepEqual(result.sessions.map((entry) => entry.threadId), [SECOND_THREAD_ID, THREAD_ID]);
  assert.equal(result.sessions.find((entry) => entry.threadId === SECOND_THREAD_ID).available, true);
  assert.equal(result.sessions.find((entry) => entry.threadId === STALE_THREAD_ID), undefined);
  assert.equal(result.sessions.find((entry) => entry.threadId === SECOND_THREAD_ID).title, "另一个工作区");
  assert.ok(client.calls.some((call) => call.method === "followConversation"
    && call.threadId === SECOND_THREAD_ID && call.following === true));
  assert.ok(client.calls.some((call) => call.method === "followConversation"
    && call.threadId === SECOND_THREAD_ID && call.following === false));
  await adapter.dispose();
});

test("IPC session list keeps the active attachment when newer stale history fills the limit", async (t) => {
  const codexHome = await fs.mkdtemp(path.join(os.tmpdir(), "codex-ipc-session-limit-"));
  t.after(() => fs.rm(codexHome, { recursive: true, force: true }));
  const sessions = path.join(codexHome, "sessions", "2026", "08");
  await fs.mkdir(sessions, { recursive: true });
  const writeRollout = async (id, cwd) => {
    await fs.writeFile(path.join(sessions, `rollout-${id}.jsonl`), `${JSON.stringify({
      type: "session_meta",
      payload: { originator: "codex_vscode", source: "vscode", cwd },
    })}\n`);
  };
  await writeRollout(THREAD_ID, "/tmp/current");
  await writeRollout(STALE_THREAD_ID, "/tmp/stale");
  await fs.writeFile(path.join(codexHome, "session_index.jsonl"), [
    JSON.stringify({ id: THREAD_ID, thread_name: "当前会话", updated_at: "2026-08-28T10:00:00Z" }),
    JSON.stringify({ id: STALE_THREAD_ID, thread_name: "较新的失效记录", updated_at: "2026-08-29T12:00:00Z" }),
  ].join("\n") + "\n");

  const client = new MultiSessionIpcClient(new Map([[THREAD_ID, fixtureState()]]), { [THREAD_ID]: "owner-a" });
  const adapter = new CodexIpcAgentAdapter({
    client,
    threadId: THREAD_ID,
    codexHome,
    loadCompleteHistory: false,
    approvalTimeoutMs: 0,
    followTimeoutMs: 500,
  });
  await adapter.start();
  const result = await adapter.listSessions({ limit: 1 });
  assert.deepEqual(result.sessions.map((entry) => entry.threadId), [THREAD_ID]);
  assert.equal(result.sessions[0].active, true);
  await adapter.dispose();
});

test("IPC session list omits an owner that does not return a matching snapshot", async (t) => {
  const codexHome = await fs.mkdtemp(path.join(os.tmpdir(), "codex-ipc-session-probe-"));
  t.after(() => fs.rm(codexHome, { recursive: true, force: true }));
  const sessions = path.join(codexHome, "sessions", "2026", "08");
  await fs.mkdir(sessions, { recursive: true });
  const writeRollout = async (id, payload) => {
    const fileName = path.join(sessions, `rollout-${id}.jsonl`);
    await fs.writeFile(fileName, `${JSON.stringify({ type: "session_meta", payload })}\n`);
  };
  await writeRollout(THREAD_ID, { originator: "codex_vscode", source: "vscode", cwd: "/tmp/a" });
  await writeRollout(SECOND_THREAD_ID, { originator: "codex_vscode", source: "vscode", cwd: "/tmp/b" });
  await fs.writeFile(path.join(codexHome, "session_index.jsonl"), [
    JSON.stringify({ id: THREAD_ID, thread_name: "当前会话", updated_at: "2026-08-29T10:00:00Z" }),
    JSON.stringify({ id: SECOND_THREAD_ID, thread_name: "桌面会话", updated_at: "2026-08-29T11:00:00Z" }),
  ].join("\n") + "\n");

  const states = new Map([
    [THREAD_ID, fixtureState()],
    [SECOND_THREAD_ID, { ...fixtureState(), id: SECOND_THREAD_ID, title: "桌面会话" }],
  ]);
  const client = new NoSnapshotSwitchIpcClient(states, {
    [THREAD_ID]: "owner-a",
    [SECOND_THREAD_ID]: "desktop-owner",
  });
  const adapter = new CodexIpcAgentAdapter({
    client,
    threadId: THREAD_ID,
    codexHome,
    loadCompleteHistory: false,
    approvalTimeoutMs: 0,
    ownerDiscoveryTimeoutMs: 250,
    followTimeoutMs: 250,
  });
  await adapter.start();
  const result = await adapter.listSessions({ limit: 10 });
  assert.equal(result.sessions.find((entry) => entry.threadId === THREAD_ID).available, true);
  assert.equal(result.sessions.find((entry) => entry.threadId === SECOND_THREAD_ID), undefined);
  const targetCalls = client.calls.filter((call) => call.method === "followConversation" && call.threadId === SECOND_THREAD_ID);
  assert.equal(targetCalls.filter((call) => call.following === true).length, 1);
  assert.equal(targetCalls.filter((call) => call.following === false).length, 1);
  await adapter.dispose();
});

test("IPC adapter switches follows only after the target owner snapshot arrives", async () => {
  const states = new Map([
    [THREAD_ID, fixtureState()],
    [SECOND_THREAD_ID, { ...fixtureState(), id: SECOND_THREAD_ID, title: "目标会话", turns: [{ id: "turn-target", status: "completed", items: [{ type: "agentMessage", id: "target-message", text: "target output" }] }] }],
  ]);
  const client = new MultiSessionIpcClient(states, {
    [THREAD_ID]: "owner-a",
    [SECOND_THREAD_ID]: "owner-b",
  });
  const events = [];
  const adapter = new CodexIpcAgentAdapter({
    client,
    threadId: THREAD_ID,
    loadCompleteHistory: false,
    approvalTimeoutMs: 0,
    followTimeoutMs: 500,
  });
  adapter.onEvent((event) => events.push(event));
  await adapter.start();
  const result = await adapter.selectSession({ threadId: SECOND_THREAD_ID });
  assert.deepEqual(result, {
    threadId: SECOND_THREAD_ID,
    previousThreadId: THREAD_ID,
    switched: true,
    available: true,
  });
  assert.equal((await adapter.snapshot()).threadId, SECOND_THREAD_ID);
  assert.match((await adapter.snapshot()).outputTail, /target output/);
  assert.ok(events.some((event) => event.type === "session.switching"));
  assert.ok(events.some((event) => event.type === "session.selected"));
  const oldUnfollow = client.calls.find((call) => call.method === "followConversation" && call.threadId === THREAD_ID && call.following === false);
  assert.ok(oldUnfollow);
  await adapter.dispose();
});

test("IPC adapter follows paired route changes from the attached VS Code panel", async () => {
  const states = new Map([
    [THREAD_ID, fixtureState()],
    [SECOND_THREAD_ID, {
      ...fixtureState(),
      id: SECOND_THREAD_ID,
      title: "面板目标会话",
      turns: [{ id: "turn-target", status: "completed", items: [{ type: "agentMessage", id: "target-message", text: "panel target output" }] }],
    }],
  ]);
  const client = new MultiSessionIpcClient(states, {
    [THREAD_ID]: "panel-owner",
    [SECOND_THREAD_ID]: "target-owner",
  });
  const events = [];
  const adapter = new CodexIpcAgentAdapter({
    client,
    threadId: THREAD_ID,
    loadCompleteHistory: false,
    approvalTimeoutMs: 0,
    followTimeoutMs: 500,
    vscodeSessionFollowDebounceMs: 5,
  });
  adapter.onEvent((event) => events.push(event));
  await adapter.start();

  client.emitFollowing(THREAD_ID, false, { sourceClientId: "panel-owner" });
  client.emitFollowing(SECOND_THREAD_ID, true, { sourceClientId: "panel-owner" });
  await waitFor(async () => (await adapter.snapshot()).threadId === SECOND_THREAD_ID);

  const snapshot = await adapter.snapshot();
  assert.match(snapshot.outputTail, /panel target output/);
  assert.ok(events.some((event) => event.type === "session.switching" && event.threadId === SECOND_THREAD_ID));
  assert.ok(events.some((event) => event.type === "session.selected" && event.threadId === SECOND_THREAD_ID));
  await adapter.dispose();
});

test("IPC adapter ignores isolated, targeted, and other-client following broadcasts", async () => {
  const states = new Map([
    [THREAD_ID, fixtureState()],
    [SECOND_THREAD_ID, { ...fixtureState(), id: SECOND_THREAD_ID, title: "不应自动切换" }],
  ]);
  const client = new MultiSessionIpcClient(states, {
    [THREAD_ID]: "panel-owner",
    [SECOND_THREAD_ID]: "target-owner",
  });
  const adapter = new CodexIpcAgentAdapter({
    client,
    threadId: THREAD_ID,
    loadCompleteHistory: false,
    approvalTimeoutMs: 0,
    followTimeoutMs: 500,
    vscodeSessionFollowDebounceMs: 0,
  });
  await adapter.start();

  // Bind the route source with a same-thread leave/re-enter pair.
  client.emitFollowing(THREAD_ID, false, { sourceClientId: "panel-owner" });
  client.emitFollowing(THREAD_ID, true, { sourceClientId: "panel-owner" });
  // A reconnect/status replay is a lone true and is not a route change.
  client.emitFollowing(SECOND_THREAD_ID, true, { sourceClientId: "panel-owner" });
  // Targeted replies describe liveness to one follower, not panel navigation.
  client.emitFollowing(THREAD_ID, false, { sourceClientId: "panel-owner", targetClientIds: ["follower"] });
  client.emitFollowing(SECOND_THREAD_ID, true, { sourceClientId: "panel-owner", targetClientIds: ["follower"] });
  // Another Codex window shares the router but cannot take over this bridge.
  client.emitFollowing(THREAD_ID, false, { sourceClientId: "other-panel" });
  client.emitFollowing(SECOND_THREAD_ID, true, { sourceClientId: "other-panel" });
  await new Promise((resolve) => setTimeout(resolve, 30));

  assert.equal((await adapter.snapshot()).threadId, THREAD_ID);
  assert.equal(client.calls.some((call) => call.method === "followConversation"
    && call.threadId === SECOND_THREAD_ID && call.following === true), false);
  await adapter.dispose();
});

test("IPC adapter does not trust an isolated false from another follower", async () => {
  const states = new Map([
    [THREAD_ID, fixtureState()],
    [SECOND_THREAD_ID, { ...fixtureState(), id: SECOND_THREAD_ID, title: "真实面板目标" }],
  ]);
  const client = new MultiSessionIpcClient(states, {
    [THREAD_ID]: "owner-a",
    [SECOND_THREAD_ID]: "owner-b",
  });
  const adapter = new CodexIpcAgentAdapter({
    client,
    threadId: THREAD_ID,
    loadCompleteHistory: false,
    approvalTimeoutMs: 0,
    followTimeoutMs: 500,
    vscodeSessionFollowDebounceMs: 0,
  });
  await adapter.start();

  client.emitFollowing(THREAD_ID, false, { sourceClientId: "disposing-remote-follower" });
  client.emitFollowing(THREAD_ID, false, { sourceClientId: "real-vscode-panel" });
  client.emitFollowing(SECOND_THREAD_ID, true, { sourceClientId: "real-vscode-panel" });
  await waitFor(async () => (await adapter.snapshot()).threadId === SECOND_THREAD_ID);
  await adapter.dispose();
});

test("IPC adapter binds the matching route source when another unbound source emits false", async () => {
  const states = new Map([
    [THREAD_ID, fixtureState()],
    [SECOND_THREAD_ID, {
      ...fixtureState(),
      id: SECOND_THREAD_ID,
      title: "真实面板目标",
      turns: [{ id: "turn-target", status: "completed", items: [{ type: "agentMessage", id: "target-message", text: "real panel target" }] }],
    }],
  ]);
  const client = new MultiSessionIpcClient(states, {
    [THREAD_ID]: "owner-a",
    [SECOND_THREAD_ID]: "owner-b",
  });
  const adapter = new CodexIpcAgentAdapter({
    client,
    threadId: THREAD_ID,
    loadCompleteHistory: false,
    approvalTimeoutMs: 0,
    followTimeoutMs: 500,
    vscodeSessionFollowDebounceMs: 0,
  });
  await adapter.start();

  client.emitFollowing(THREAD_ID, false, { sourceClientId: "real-vscode-panel" });
  client.emitFollowing(THREAD_ID, false, { sourceClientId: "other-follower" });
  client.emitFollowing(SECOND_THREAD_ID, true, { sourceClientId: "real-vscode-panel" });
  await waitFor(async () => (await adapter.snapshot()).threadId === SECOND_THREAD_ID);

  assert.match((await adapter.snapshot()).outputTail, /real panel target/);
  await adapter.dispose();
});

test("IPC adapter lets a replacement route source take over after the bound client disconnects", async () => {
  const states = new Map([
    [THREAD_ID, fixtureState()],
    [SECOND_THREAD_ID, {
      ...fixtureState(),
      id: SECOND_THREAD_ID,
      title: "重连后的目标",
      turns: [{ id: "turn-target", status: "completed", items: [{ type: "agentMessage", id: "target-message", text: "replacement panel target" }] }],
    }],
  ]);
  const client = new MultiSessionIpcClient(states, {
    [THREAD_ID]: "owner-a",
    [SECOND_THREAD_ID]: "owner-b",
  });
  const adapter = new CodexIpcAgentAdapter({
    client,
    threadId: THREAD_ID,
    loadCompleteHistory: false,
    approvalTimeoutMs: 0,
    followTimeoutMs: 500,
    vscodeSessionFollowDebounceMs: 0,
  });
  await adapter.start();

  // First bind the official panel without changing the selected conversation.
  client.emitFollowing(THREAD_ID, false, { sourceClientId: "old-vscode-panel" });
  client.emitFollowing(THREAD_ID, true, { sourceClientId: "old-vscode-panel" });
  client.emitClientStatus("old-vscode-panel", "disconnected");

  client.emitFollowing(THREAD_ID, false, { sourceClientId: "replacement-vscode-panel" });
  client.emitFollowing(SECOND_THREAD_ID, true, { sourceClientId: "replacement-vscode-panel" });
  await waitFor(async () => (await adapter.snapshot()).threadId === SECOND_THREAD_ID);

  assert.match((await adapter.snapshot()).outputTail, /replacement panel target/);
  await adapter.dispose();
});

test("IPC adapter coalesces rapid VS Code A to B to C navigation to C", async () => {
  const states = new Map([
    [THREAD_ID, fixtureState()],
    [SECOND_THREAD_ID, { ...fixtureState(), id: SECOND_THREAD_ID, title: "中间会话" }],
    [THIRD_THREAD_ID, {
      ...fixtureState(),
      id: THIRD_THREAD_ID,
      title: "最终会话",
      turns: [{ id: "turn-c", status: "completed", items: [{ type: "agentMessage", id: "message-c", text: "final C output" }] }],
    }],
  ]);
  const client = new MultiSessionIpcClient(states, {
    [THREAD_ID]: "panel-owner",
    [SECOND_THREAD_ID]: "owner-b",
    [THIRD_THREAD_ID]: "owner-c",
  });
  const adapter = new CodexIpcAgentAdapter({
    client,
    threadId: THREAD_ID,
    loadCompleteHistory: false,
    approvalTimeoutMs: 0,
    followTimeoutMs: 500,
    vscodeSessionFollowDebounceMs: 20,
  });
  await adapter.start();

  client.emitFollowing(THREAD_ID, false, { sourceClientId: "panel-owner" });
  client.emitFollowing(SECOND_THREAD_ID, true, { sourceClientId: "panel-owner" });
  client.emitFollowing(SECOND_THREAD_ID, false, { sourceClientId: "panel-owner" });
  client.emitFollowing(THIRD_THREAD_ID, true, { sourceClientId: "panel-owner" });
  await waitFor(async () => (await adapter.snapshot()).threadId === THIRD_THREAD_ID);

  assert.match((await adapter.snapshot()).outputTail, /final C output/);
  assert.equal(client.calls.some((call) => call.method === "followConversation"
    && call.threadId === SECOND_THREAD_ID && call.following === true), false);
  await adapter.dispose();
});

test("IPC adapter cancels an in-flight B snapshot when VS Code moves on to C", async () => {
  const states = new Map([
    [THREAD_ID, fixtureState()],
    [SECOND_THREAD_ID, { ...fixtureState(), id: SECOND_THREAD_ID, title: "无快照 B" }],
    [THIRD_THREAD_ID, {
      ...fixtureState(),
      id: THIRD_THREAD_ID,
      title: "最终 C",
      turns: [{ id: "turn-c", status: "completed", items: [{ type: "agentMessage", id: "message-c", text: "C arrived" }] }],
    }],
  ]);
  const client = new NoSnapshotSwitchIpcClient(states, {
    [THREAD_ID]: "panel-owner",
    [SECOND_THREAD_ID]: "owner-b",
    [THIRD_THREAD_ID]: "owner-c",
  });
  const events = [];
  const adapter = new CodexIpcAgentAdapter({
    client,
    threadId: THREAD_ID,
    loadCompleteHistory: false,
    approvalTimeoutMs: 0,
    followTimeoutMs: 1_000,
    vscodeSessionFollowDebounceMs: 0,
  });
  adapter.onEvent((event) => events.push(event));
  await adapter.start();

  client.emitFollowing(THREAD_ID, false, { sourceClientId: "panel-owner" });
  client.emitFollowing(SECOND_THREAD_ID, true, { sourceClientId: "panel-owner" });
  await waitFor(() => events.some((event) => event.type === "session.switching" && event.threadId === SECOND_THREAD_ID));
  const movedOnAt = Date.now();
  client.emitFollowing(SECOND_THREAD_ID, false, { sourceClientId: "panel-owner" });
  client.emitFollowing(THIRD_THREAD_ID, true, { sourceClientId: "panel-owner" });
  await waitFor(async () => (await adapter.snapshot()).threadId === THIRD_THREAD_ID, 500);

  assert.ok(Date.now() - movedOnAt < 500, "C should not wait for B's 1s snapshot timeout");
  assert.equal(events.some((event) => event.type === "session.selected"
    && event.threadId === SECOND_THREAD_ID && event.payload.switched === true), false);
  assert.match((await adapter.snapshot()).outputTail, /C arrived/);
  await adapter.dispose();
});

test("IPC adapter defers the latest VS Code route while the old turn is active", async () => {
  const active = fixtureState();
  active.turns[0].status = "inProgress";
  active.threadRuntimeStatus = { type: "active" };
  const completed = structuredClone(active);
  completed.turns[0].status = "completed";
  completed.threadRuntimeStatus = { type: "idle" };
  const states = new Map([
    [THREAD_ID, active],
    [SECOND_THREAD_ID, { ...fixtureState(), id: SECOND_THREAD_ID, title: "延后目标" }],
  ]);
  const client = new MultiSessionIpcClient(states, {
    [THREAD_ID]: "panel-owner",
    [SECOND_THREAD_ID]: "target-owner",
  });
  const adapter = new CodexIpcAgentAdapter({
    client,
    threadId: THREAD_ID,
    loadCompleteHistory: false,
    approvalTimeoutMs: 0,
    followTimeoutMs: 500,
    vscodeSessionFollowDebounceMs: 0,
  });
  await adapter.start();

  client.emitFollowing(THREAD_ID, false, { sourceClientId: "panel-owner" });
  client.emitFollowing(SECOND_THREAD_ID, true, { sourceClientId: "panel-owner" });
  await new Promise((resolve) => setTimeout(resolve, 40));
  assert.equal((await adapter.snapshot()).threadId, THREAD_ID);

  client.emitState(completed, 2, "patches", THREAD_ID, "panel-owner");
  await waitFor(async () => (await adapter.snapshot()).threadId === SECOND_THREAD_ID, 1_000);
  await adapter.dispose();
});

test("IPC adapter publishes an explicit rollback when a VS Code route cannot stream", async () => {
  const states = new Map([
    [THREAD_ID, fixtureState()],
    [SECOND_THREAD_ID, { ...fixtureState(), id: SECOND_THREAD_ID, title: "无快照目标" }],
  ]);
  const client = new NoSnapshotSwitchIpcClient(states, {
    [THREAD_ID]: "panel-owner",
    [SECOND_THREAD_ID]: "target-owner",
  });
  const events = [];
  const adapter = new CodexIpcAgentAdapter({
    client,
    threadId: THREAD_ID,
    loadCompleteHistory: false,
    approvalTimeoutMs: 0,
    followTimeoutMs: 25,
    vscodeSessionFollowDebounceMs: 0,
  });
  adapter.onEvent((event) => events.push(event));
  await adapter.start();
  events.length = 0;

  client.emitFollowing(THREAD_ID, false, { sourceClientId: "panel-owner" });
  client.emitFollowing(SECOND_THREAD_ID, true, { sourceClientId: "panel-owner" });
  await waitFor(() => events.some((event) => event.type === "session.selected" && event.payload.failed === true));

  const snapshot = await adapter.snapshot();
  assert.equal(snapshot.threadId, THREAD_ID);
  assert.match(snapshot.outputTail, /hi from VS Code/);
  const rollbackIndex = events.findIndex((event) => event.type === "session.selected" && event.payload.failed === true);
  const restoredOutputIndex = events.findIndex((event, index) => index > rollbackIndex
    && event.type === "output.snapshot" && event.threadId === THREAD_ID);
  assert.ok(rollbackIndex >= 0);
  assert.ok(restoredOutputIndex > rollbackIndex);
  await adapter.dispose();
});

test("IPC adapter keeps dispose authoritative while an automatic switch is waiting", async () => {
  const states = new Map([
    [THREAD_ID, fixtureState()],
    [SECOND_THREAD_ID, { ...fixtureState(), id: SECOND_THREAD_ID, title: "等待中的目标" }],
  ]);
  const client = new NoSnapshotSwitchIpcClient(states, {
    [THREAD_ID]: "panel-owner",
    [SECOND_THREAD_ID]: "target-owner",
  });
  const events = [];
  const adapter = new CodexIpcAgentAdapter({
    client,
    threadId: THREAD_ID,
    loadCompleteHistory: false,
    approvalTimeoutMs: 0,
    followTimeoutMs: 1_000,
    vscodeSessionFollowDebounceMs: 0,
  });
  adapter.onEvent((event) => events.push(event));
  await adapter.start();
  client.emitFollowing(THREAD_ID, false, { sourceClientId: "panel-owner" });
  client.emitFollowing(SECOND_THREAD_ID, true, { sourceClientId: "panel-owner" });
  await waitFor(() => events.some((event) => event.type === "session.switching"));

  const eventCountAtDispose = events.length;
  await adapter.dispose();
  await new Promise((resolve) => setTimeout(resolve, 30));
  assert.equal((await adapter.snapshot()).state, "disconnected");
  assert.equal(events.slice(eventCountAtDispose).some((event) => [
    "session.selected",
    "output.snapshot",
    "session.snapshot",
  ].includes(event.type)), false);
});

test("IPC adapter absorbs a snapshot waiter when target follow fails immediately", async () => {
  const states = new Map([
    [THREAD_ID, fixtureState()],
    [SECOND_THREAD_ID, { ...fixtureState(), id: SECOND_THREAD_ID, title: "立即失败" }],
  ]);
  const client = new ThrowingTargetFollowIpcClient(states, {
    [THREAD_ID]: "owner-a",
    [SECOND_THREAD_ID]: "owner-b",
  });
  const adapter = new CodexIpcAgentAdapter({
    client,
    threadId: THREAD_ID,
    loadCompleteHistory: false,
    approvalTimeoutMs: 0,
    followTimeoutMs: 500,
  });
  await adapter.start();
  await assert.rejects(() => adapter.selectSession({ threadId: SECOND_THREAD_ID }), /target follow failed immediately/);
  assert.equal((await adapter.snapshot()).threadId, THREAD_ID);
  await adapter.dispose();
});

test("IPC adapter rejects a target snapshot when owner changes before commit", async () => {
  const states = new Map([
    [THREAD_ID, fixtureState()],
    [SECOND_THREAD_ID, { ...fixtureState(), id: SECOND_THREAD_ID, title: "Owner handoff" }],
  ]);
  const client = new OwnerChangingIpcClient(states, {
    [THREAD_ID]: "owner-a",
    [SECOND_THREAD_ID]: "owner-b",
  });
  const adapter = new CodexIpcAgentAdapter({
    client,
    threadId: THREAD_ID,
    loadCompleteHistory: false,
    approvalTimeoutMs: 0,
    followTimeoutMs: 500,
  });
  await adapter.start();
  await assert.rejects(
    () => adapter.selectSession({ threadId: SECOND_THREAD_ID }),
    /owner changed while switching/,
  );
  assert.equal((await adapter.snapshot()).threadId, THREAD_ID);
  await adapter.dispose();
});

test("IPC adapter serializes list probes before selecting the same session", async (t) => {
  const codexHome = await fs.mkdtemp(path.join(os.tmpdir(), "codex-ipc-session-serialization-"));
  t.after(() => fs.rm(codexHome, { recursive: true, force: true }));
  const sessions = path.join(codexHome, "sessions", "2026", "08");
  await fs.mkdir(sessions, { recursive: true });
  for (const [id, cwd] of [[THREAD_ID, "/tmp/current"], [SECOND_THREAD_ID, "/tmp/target"]]) {
    await fs.writeFile(path.join(sessions, `rollout-${id}.jsonl`), `${JSON.stringify({
      type: "session_meta",
      payload: { originator: "codex_vscode", source: "vscode", cwd },
    })}\n`);
  }
  const states = new Map([
    [THREAD_ID, fixtureState()],
    [SECOND_THREAD_ID, { ...fixtureState(), id: SECOND_THREAD_ID, title: "目标会话" }],
  ]);
  const client = new DelayedProbeIpcClient(states, {
    [THREAD_ID]: "owner-a",
    [SECOND_THREAD_ID]: "owner-b",
  });
  const adapter = new CodexIpcAgentAdapter({
    client,
    threadId: THREAD_ID,
    codexHome,
    loadCompleteHistory: false,
    approvalTimeoutMs: 0,
    followTimeoutMs: 500,
  });
  await adapter.start();

  const listPromise = adapter.listSessions({ limit: 10 });
  await new Promise((resolve) => setTimeout(resolve, 5));
  const selectPromise = adapter.selectSession({ threadId: SECOND_THREAD_ID });
  await Promise.all([listPromise, selectPromise]);

  const targetFollowing = client.calls
    .filter((call) => call.method === "followConversation" && call.threadId === SECOND_THREAD_ID)
    .map((call) => call.following);
  assert.deepEqual(targetFollowing, [true, false, true]);
  assert.equal((await adapter.snapshot()).threadId, SECOND_THREAD_ID);
  await adapter.dispose();
});

test("IPC adapter restores the cached previous projection when target follow has no snapshot", async () => {
  const states = new Map([
    [THREAD_ID, fixtureState()],
    [SECOND_THREAD_ID, { ...fixtureState(), id: SECOND_THREAD_ID, title: "目标会话" }],
  ]);
  const client = new NoSnapshotSwitchIpcClient(states, {
    [THREAD_ID]: "owner-a",
    [SECOND_THREAD_ID]: "owner-b",
  });
  const events = [];
  const adapter = new CodexIpcAgentAdapter({
    client,
    threadId: THREAD_ID,
    loadCompleteHistory: false,
    approvalTimeoutMs: 0,
    followTimeoutMs: 25,
  });
  adapter.onEvent((event) => events.push(event));
  await adapter.start();
  const before = await adapter.snapshot();
  assert.equal(before.threadId, THREAD_ID);
  assert.match(before.outputTail, /hi from VS Code/);

  await assert.rejects(
    () => adapter.selectSession({ threadId: SECOND_THREAD_ID }),
    /Timed out waiting for a snapshot from VS Code conversation/,
  );

  const after = await adapter.snapshot();
  assert.equal(after.threadId, THREAD_ID);
  assert.equal(after.outputTail, before.outputTail);
  assert.deepEqual(after.messages, before.messages);
  assert.equal(after.state, "idle");
  assert.ok(events.some((event) => event.type === "output.snapshot" && event.threadId === THREAD_ID));
  assert.ok(events.some((event) => event.type === "session.snapshot" && event.threadId === THREAD_ID));
  await adapter.dispose();
});

test("IPC adapter restores its owner-validated projection after the IPC cache is polluted", async () => {
  const states = new Map([
    [THREAD_ID, fixtureState()],
    [SECOND_THREAD_ID, { ...fixtureState(), id: SECOND_THREAD_ID, title: "无快照目标" }],
  ]);
  const client = new NoSnapshotSwitchIpcClient(states, {
    [THREAD_ID]: "owner-a",
    [SECOND_THREAD_ID]: "owner-b",
  });
  const adapter = new CodexIpcAgentAdapter({
    client,
    threadId: THREAD_ID,
    loadCompleteHistory: false,
    approvalTimeoutMs: 0,
    followTimeoutMs: 25,
  });
  await adapter.start();
  const verified = await adapter.snapshot();

  const polluted = {
    ...fixtureState(),
    title: "来自旧 owner 的污染快照",
    turns: [{
      id: "turn-polluted",
      status: "completed",
      items: [{ type: "agentMessage", id: "polluted-message", text: "poisoned stale output" }],
    }],
  };
  client.emitState(polluted, 99, "snapshot", THREAD_ID, "old-owner");

  assert.equal(client.getConversationState(THREAD_ID).ownerClientId, "old-owner");
  assert.equal(client.getConversationState(THREAD_ID).conversationState.title, "来自旧 owner 的污染快照");
  assert.equal((await adapter.snapshot()).outputTail, verified.outputTail);

  await assert.rejects(
    () => adapter.selectSession({ threadId: SECOND_THREAD_ID }),
    /Timed out waiting for a snapshot from VS Code conversation/,
  );

  const restored = await adapter.snapshot();
  assert.equal(restored.threadId, THREAD_ID);
  assert.equal(restored.outputTail, verified.outputTail);
  assert.deepEqual(restored.messages, verified.messages);
  assert.doesNotMatch(restored.outputTail, /poisoned stale output/);
  await adapter.dispose();
});

test("IPC adapter denies target approvals on dispose and blocks ordinary commands mid-switch", async () => {
  const targetApprovalId = "target-approval";
  const targetState = {
    ...fixtureState([{
      id: targetApprovalId,
      method: "item/commandExecution/requestApproval",
      params: { threadId: SECOND_THREAD_ID, turnId: "turn-target", command: "echo target" },
    }]),
    id: SECOND_THREAD_ID,
    title: "等待 owner 确认的目标",
  };
  const states = new Map([
    [THREAD_ID, fixtureState()],
    [SECOND_THREAD_ID, targetState],
  ]);
  const client = new DelayedTargetOwnerConfirmationIpcClient(states, {
    [THREAD_ID]: "owner-a",
    [SECOND_THREAD_ID]: "owner-b",
  });
  const adapter = new CodexIpcAgentAdapter({
    client,
    threadId: THREAD_ID,
    loadCompleteHistory: false,
    approvalTimeoutMs: 0,
    followTimeoutMs: 500,
  });
  await adapter.start();

  const switching = adapter.selectSession({ threadId: SECOND_THREAD_ID });
  void switching.catch(() => undefined);
  await waitFor(() => client.targetConfirmationStarted);
  await waitFor(async () => (await adapter.snapshot()).pendingApprovals.some((entry) => entry.requestId === targetApprovalId));

  await assert.rejects(
    () => adapter.startTurn({ text: "must not be sent while switching" }),
    /session switch is still in progress/,
  );
  await assert.rejects(
    () => adapter.respondApproval(targetApprovalId, "allow"),
    /session switch is still in progress/,
  );
  assert.equal(client.calls.some((call) => call.method === "startTurn"), false);
  assert.equal(client.calls.some((call) => call.method === "respondCommandApproval"), false);

  await adapter.dispose();
  const denied = client.calls.find((call) => call.method === "respondCommandApproval"
    && call.requestId === targetApprovalId);
  assert.ok(denied);
  assert.equal(denied.threadId, SECOND_THREAD_ID);
  assert.equal(denied.decision, "decline");
  assert.equal(denied.options.ownerClientId, "owner-b");

  client.releaseTargetOwnerConfirmation();
  await assert.rejects(switching, /session selection was cancelled because the IPC session closed/);
});

test("IPC adapter refuses session switching during an active turn or pending request", async () => {
  const state = fixtureState([{ id: 12, method: "item/commandExecution/requestApproval", params: { command: "echo pending" } }]);
  const client = new MultiSessionIpcClient(new Map([[THREAD_ID, state], [SECOND_THREAD_ID, fixtureState()]]), {
    [THREAD_ID]: "owner-a",
    [SECOND_THREAD_ID]: "owner-b",
  });
  const adapter = new CodexIpcAgentAdapter({ client, threadId: THREAD_ID, loadCompleteHistory: false, approvalTimeoutMs: 0, followTimeoutMs: 500 });
  await adapter.start();
  await assert.rejects(() => adapter.selectSession({ threadId: SECOND_THREAD_ID }), /turn or approval is active/);
  await adapter.dispose();
});

test("IPC session discovery accepts current ULID rollout filenames", async (t) => {
  const codexHome = await fs.mkdtemp(path.join(os.tmpdir(), "codex-ipc-ulid-"));
  t.after(() => fs.rm(codexHome, { recursive: true, force: true }));
  const sessions = path.join(codexHome, "sessions", "2026", "08");
  await fs.mkdir(sessions, { recursive: true });
  await fs.writeFile(path.join(sessions, `rollout-2026-08-29T00-00-00-${ULID_THREAD_ID}.jsonl`), `${JSON.stringify({
    type: "session_meta",
    payload: { id: ULID_THREAD_ID, originator: "codex_vscode", source: "vscode", cwd: "/tmp/ulid" },
  })}\n`);
  await fs.writeFile(path.join(codexHome, "session_index.jsonl"), `${JSON.stringify({ id: ULID_THREAD_ID, thread_name: "ULID 会话", updated_at: "2026-08-29T12:00:00Z" })}\n`);
  const client = new DiscoveryIpcClient(fixtureState());
  const adapter = new CodexIpcAgentAdapter({
    client,
    threadId: THREAD_ID,
    codexHome,
    loadCompleteHistory: false,
    approvalTimeoutMs: 0,
    followTimeoutMs: 500,
  });
  await adapter.start();
  const result = await adapter.listSessions({ limit: 10 });
  const session = result.sessions.find((entry) => entry.threadId === ULID_THREAD_ID);
  assert.ok(session);
  assert.equal(session.title, "ULID 会话");
  assert.equal(session.available, true);
  await adapter.dispose();
});
