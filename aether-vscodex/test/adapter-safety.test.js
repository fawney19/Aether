"use strict";

const assert = require("node:assert/strict");
const test = require("node:test");

const { CodexAgentAdapter } = require("../vscode-extension/dist/codexAgentAdapter.js");
const { RelayHost } = require("../vscode-extension/dist/relayHost.js");

class FakeRpc {
  responses = [];
  requests = [];
  notificationListener;
  requestListener;
  exitListener;
  overrides;

  constructor(overrides = {}) {
    this.overrides = overrides;
  }

  get running() {
    return true;
  }

  async start() {}

  async request(method, params) {
    this.requests.push({ method, params });
    if (Object.prototype.hasOwnProperty.call(this.overrides, method)) {
      const override = this.overrides[method];
      return typeof override === "function" ? override(params) : override;
    }
    if (method === "initialize") return { userAgent: "test", codexHome: "/tmp/codex" };
    if (method === "thread/start") return { thread: { id: "thread-test" }, cwd: "/tmp" };
    if (method === "turn/start") return { turn: { id: "turn-test" } };
    if (method === "turn/steer") return { turn: { id: "turn-test" } };
    if (method === "turn/interrupt") return {};
    throw new Error(`unexpected request ${method}`);
  }

  notify() {}

  respond(id, result) {
    this.responses.push({ id, result });
  }

  respondError(id, code, message) {
    this.responses.push({ id, error: { code, message } });
  }

  onNotification(listener) {
    this.notificationListener = listener;
    return { dispose: () => undefined };
  }

  onServerRequest(listener) {
    this.requestListener = listener;
    return { dispose: () => undefined };
  }

  onExit(listener) {
    this.exitListener = listener;
    return { dispose: () => undefined };
  }

  close() {}

  emitRequest(request) {
    this.requestListener(request);
  }

  emitNotification(notification) {
    this.notificationListener(notification);
  }
}

class FakeRelay {
  frames = [];
  listeners = new Set();

  async connect() {}

  send(frame) {
    this.frames.push(frame);
  }

  onMessage(listener) {
    this.listeners.add(listener);
    return { dispose: () => this.listeners.delete(listener) };
  }

  close() {}
}

test("CodexAgentAdapter keeps numeric and string approval ids distinct", async () => {
  const rpc = new FakeRpc();
  const adapter = new CodexAgentAdapter({ approvalTimeoutMs: 0 }, rpc);
  await adapter.start();

  rpc.emitRequest({
    id: 1,
    method: "item/commandExecution/requestApproval",
    params: { threadId: "t", turnId: "u", itemId: "n", command: "echo number" },
  });
  rpc.emitRequest({
    id: "1",
    method: "item/commandExecution/requestApproval",
    params: { threadId: "t", turnId: "u", itemId: "s", command: "echo string" },
  });

  const snapshot = await adapter.snapshot();
  assert.deepEqual(snapshot.pendingApprovals.map((entry) => entry.requestId), [1, "1"]);
  await adapter.respondApproval(1, "deny");
  await adapter.respondApproval("1", "deny");
  assert.deepEqual(rpc.responses.map((entry) => entry.id), [1, "1"]);
  assert.equal((await adapter.snapshot()).pendingApprovals.length, 0);
  await adapter.dispose();
});

test("commandActions are included in high-risk approval classification", async () => {
  const rpc = new FakeRpc();
  const adapter = new CodexAgentAdapter({ approvalTimeoutMs: 0 }, rpc);
  await adapter.start();
  rpc.emitRequest({
    id: 2,
    method: "item/commandExecution/requestApproval",
    params: {
      threadId: "t",
      turnId: "u",
      itemId: "actions",
      command: null,
      commandActions: [{ type: "unknown", command: "sudo rm -rf /" }],
    },
  });
  const snapshot = await adapter.snapshot();
  assert.equal(snapshot.pendingApprovals[0].risk, "high");
  await adapter.respondApproval(2, "deny");
  await adapter.dispose();
});

test("output snapshots stay redacted and interrupt clears the active turn", async () => {
  const rpc = new FakeRpc();
  const adapter = new CodexAgentAdapter({ approvalTimeoutMs: 0 }, rpc);
  await adapter.start();
  await adapter.startThread({});
  await adapter.startTurn({ text: "hello" });
  assert.equal((await adapter.snapshot()).turnId, "turn-test");

  rpc.emitNotification({
    method: "item/agentMessage/delta",
    params: { delta: "credential Bearer abcdefghijklmnop" },
  });
  const snapshot = await adapter.snapshot();
  assert.equal(snapshot.outputTail.includes("Bearer abcdefghijklmnop"), false);
  assert.match(snapshot.outputTail, /\[REDACTED\]/);

  await adapter.interruptTurn({});
  const afterInterrupt = await adapter.snapshot();
  assert.equal(afterInterrupt.turnId, null);
  assert.equal(afterInterrupt.state, "idle");
  await adapter.dispose();
});

test("async adapter lists app-server threads and exposes the model catalog", async () => {
  const rpc = new FakeRpc({
    "model/list": {
      data: [{ id: "model-1", model: "gpt-5.6-sol", displayName: "5.6 Sol", hidden: false }],
      nextCursor: null,
    },
    "thread/list": {
      data: [
        {
          id: "thread-recent",
          name: null,
          preview: "Inspect the workspace\nwith detail",
          cwd: "/tmp/workspace",
          createdAt: 1_700_000_000,
          updatedAt: 1_700_000_100,
          status: { type: "idle" },
          source: "vscode",
        },
      ],
      nextCursor: "next-page",
      backwardsCursor: null,
    },
  });
  const adapter = new CodexAgentAdapter({ approvalTimeoutMs: 0 }, rpc);
  await adapter.start();

  const result = await adapter.listSessions({ limit: 500, query: "workspace", sortKey: "invalid" });
  assert.equal(result.sessions[0].threadId, "thread-recent");
  assert.equal(result.sessions[0].title, "Inspect the workspace with detail");
  assert.equal(result.sessions[0].updatedAtMs, 1_700_000_100_000);
  assert.equal(result.nextCursor, "next-page");
  const listRequest = rpc.requests.find((entry) => entry.method === "thread/list");
  assert.deepEqual(listRequest.params, {
    limit: 100,
    sortKey: "updated_at",
    sortDirection: "desc",
    searchTerm: "workspace",
  });
  const snapshot = await adapter.snapshot();
  assert.equal(snapshot.metadata.mode, "async");
  assert.equal(snapshot.metadata.availableModels[0].model, "gpt-5.6-sol");
  await adapter.dispose();
});

test("async adapter projects live token usage notifications into metadata and snapshots", async () => {
  const rpc = new FakeRpc({
    "model/list": { data: [], nextCursor: null },
  });
  const adapter = new CodexAgentAdapter({ approvalTimeoutMs: 0 }, rpc);
  const events = [];
  adapter.onEvent((event) => events.push(event));
  await adapter.start();
  await adapter.startThread({});

  rpc.emitNotification({
    method: "thread/tokenUsage/updated",
    params: {
      threadId: "thread-test",
      // A usage update may arrive after the turn has completed. It must not
      // make the adapter report that historical turn as active again.
      turnId: "turn-finished",
      tokenUsage: {
        total: {
          totalTokens: 1_200,
          inputTokens: 800,
          cachedInputTokens: 100,
          cacheWriteInputTokens: 20,
          outputTokens: 300,
          reasoningOutputTokens: 80,
        },
        last: {
          totalTokens: 450,
          inputTokens: 300,
          cachedInputTokens: 40,
          cacheWriteInputTokens: 10,
          outputTokens: 100,
          reasoningOutputTokens: 40,
        },
        modelContextWindow: 128_000,
      },
    },
  });

  const expected = {
    total: {
      totalTokens: 1_200,
      inputTokens: 800,
      cachedInputTokens: 100,
      cacheWriteInputTokens: 20,
      outputTokens: 300,
      reasoningOutputTokens: 80,
    },
    last: {
      totalTokens: 450,
      inputTokens: 300,
      cachedInputTokens: 40,
      cacheWriteInputTokens: 10,
      outputTokens: 100,
      reasoningOutputTokens: 40,
    },
    modelContextWindow: 128_000,
  };
  const snapshot = await adapter.snapshot();
  assert.deepEqual(snapshot.metadata.tokenUsage, expected);
  assert.deepEqual(snapshot.metadata.latestTokenUsageInfo, expected);
  assert.equal(snapshot.turnId, null);

  const usageEvent = events.find((event) => event.raw?.method === "thread/tokenUsage/updated");
  assert.ok(usageEvent);
  assert.deepEqual(usageEvent.payload.tokenUsage, expected);
  assert.deepEqual(usageEvent.payload.latestTokenUsageInfo, expected);
  // Keep the raw diagnostic envelope redacted while exposing only the safe
  // numeric projection to the browser.
  assert.equal(usageEvent.raw.params.tokenUsage, "[REDACTED]");

  rpc.emitNotification({
    method: "thread/tokenUsage/updated",
    params: {
      threadId: "thread-test",
      turnId: "turn-finished",
      tokenUsage: { total: { inputTokens: -1 } },
    },
  });
  assert.deepEqual((await adapter.snapshot()).metadata.tokenUsage, expected);
  await adapter.dispose();
});

test("async adapter resumes a thread with structured history and ignores late notifications", async () => {
  const thread = {
    id: "thread-selected",
    name: "Selected thread",
    preview: "hello",
    cwd: "/tmp/selected",
    createdAt: 1_700_000_000,
    updatedAt: 1_700_000_010,
    status: { type: "idle" },
    turns: [{
      id: "turn-history",
      status: "completed",
      startedAt: 1_700_000_001,
      completedAt: 1_700_000_004,
      durationMs: 3_000,
      items: [
        { type: "userMessage", id: "user-1", clientId: null, content: [{ type: "text", text: "hello", text_elements: [] }] },
        { type: "reasoning", id: "reason-1", summary: ["Checking files"], content: [] },
        { type: "commandExecution", id: "command-1", command: "pwd", cwd: "/tmp/selected", status: "completed", aggregatedOutput: "/tmp/selected\n", exitCode: 0, durationMs: 50, commandActions: [] },
        { type: "agentMessage", id: "agent-1", text: "Done", phase: "final_answer" },
      ],
    }],
  };
  const rpc = new FakeRpc({
    "model/list": { data: [{ id: "model-1", model: "gpt-5.6-sol" }], nextCursor: null },
    "thread/resume": {
      thread,
      model: "gpt-5.6-sol",
      modelProvider: "openai",
      serviceTier: null,
      cwd: "/tmp/selected",
      approvalPolicy: "on-request",
      approvalsReviewer: "user",
      sandbox: { type: "workspaceWrite" },
      reasoningEffort: "high",
    },
  });
  const adapter = new CodexAgentAdapter({ approvalTimeoutMs: 0 }, rpc);
  const events = [];
  adapter.onEvent((event) => events.push(event));
  await adapter.start();

  const result = await adapter.selectSession({ threadId: "thread-selected" });
  assert.equal(result.threadId, "thread-selected");
  let snapshot = await adapter.snapshot();
  assert.equal(snapshot.messages.length, 4);
  assert.deepEqual(snapshot.messages.map((message) => message.kind), ["user", "reasoning", "tool", "assistant"]);
  assert.equal(snapshot.messages[2].output, "/tmp/selected\n");
  assert.equal(snapshot.metadata.title, "Selected thread");
  assert.equal(snapshot.metadata.threadSettings.effort, "high");
  assert.equal(snapshot.metadata.historyComplete, true);
  assert.equal(snapshot.status.turnStatus, "completed");
  assert.match(snapshot.outputTail, /Done/);
  assert.ok(events.some((event) => event.type === "output.snapshot" && event.payload.historyComplete === true));
  const authoritative = events.find((event) => event.type === "session.snapshot");
  assert.equal(authoritative.payload.threadId, "thread-selected");
  assert.equal(authoritative.payload.metadata.model, "gpt-5.6-sol");
  assert.equal(authoritative.payload.messages.length, 4);

  rpc.emitNotification({
    method: "item/completed",
    params: {
      threadId: "thread-old",
      turnId: "turn-old",
      completedAtMs: Date.now(),
      item: { type: "agentMessage", id: "late-old", text: "wrong thread" },
    },
  });
  rpc.emitNotification({
    method: "item/completed",
    params: {
      threadId: "thread-selected",
      turnId: "turn-live",
      completedAtMs: Date.now(),
      item: { type: "agentMessage", id: "current-item", text: "current thread" },
    },
  });
  snapshot = await adapter.snapshot();
  assert.equal(snapshot.messages.some((message) => message.itemId === "late-old"), false);
  assert.equal(snapshot.messages.some((message) => message.itemId === "current-item"), true);
  await adapter.dispose();
});

test("async adapter hydrates paginated turns and items into chronological complete history", async () => {
  const threadId = "thread-paged-history";
  const userItem = (id, text) => ({
    type: "userMessage",
    id,
    clientId: null,
    content: [{ type: "text", text, text_elements: [] }],
  });
  const assistantItem = (id, text) => ({
    type: "agentMessage",
    id,
    text,
    phase: "final_answer",
  });
  const earlyUser = userItem("early-user", "first question");
  const rpc = new FakeRpc({
    "model/list": { data: [], nextCursor: null },
    "thread/resume": {
      thread: {
        id: threadId,
        name: "Paged history",
        preview: "first question",
        cwd: "/tmp/paged",
        createdAt: 50,
        updatedAt: 350,
        historyMode: "paginated",
        status: { type: "idle" },
        turns: [],
      },
      model: "gpt-5.6-sol",
      cwd: "/tmp/paged",
      initialTurnsPage: {
        data: [{
          id: "turn-late",
          status: "completed",
          startedAt: 300,
          completedAt: 310,
          itemsView: "full",
          items: [userItem("late-user", "third question"), assistantItem("late-agent", "third answer")],
        }],
        nextCursor: "turn-page-2",
        backwardsCursor: null,
      },
    },
    "thread/turns/list": (params) => {
      if (params.cursor === "turn-page-2") {
        return {
          data: [{
            id: "turn-early",
            status: "completed",
            startedAt: 100,
            completedAt: 110,
            itemsView: "summary",
            items: [earlyUser],
          }],
          nextCursor: "turn-page-3",
          backwardsCursor: null,
        };
      }
      assert.equal(params.cursor, "turn-page-3");
      return {
        data: [{
          id: "turn-middle",
          status: "completed",
          startedAt: 200,
          completedAt: 210,
          itemsView: "full",
          items: [userItem("middle-user", "second question"), assistantItem("middle-agent", "second answer")],
        }],
        nextCursor: null,
        backwardsCursor: null,
      };
    },
    "thread/items/list": (params) => {
      assert.equal(params.turnId, "turn-early");
      return {
        data: [
          // The summary row is repeated by the full item page; hydration must
          // de-duplicate it while adding the omitted assistant response.
          { turnId: "turn-early", item: earlyUser },
          { turnId: "turn-early", item: assistantItem("early-agent", "first answer") },
        ],
        nextCursor: null,
        backwardsCursor: null,
      };
    },
  });
  const events = [];
  const adapter = new CodexAgentAdapter({ approvalTimeoutMs: 0 }, rpc);
  adapter.onEvent((event) => events.push(event));
  await adapter.start();
  await adapter.selectSession({ threadId });

  const resume = rpc.requests.find((entry) => entry.method === "thread/resume");
  assert.deepEqual(resume.params, {
    threadId,
    excludeTurns: true,
    initialTurnsPage: { limit: 100, sortDirection: "asc", itemsView: "full" },
  });
  const turnPages = rpc.requests.filter((entry) => entry.method === "thread/turns/list");
  assert.deepEqual(turnPages.map((entry) => entry.params.cursor), ["turn-page-2", "turn-page-3"]);
  assert.ok(turnPages.every((entry) => entry.params.threadId === threadId
    && entry.params.limit === 100
    && entry.params.sortDirection === "asc"
    && entry.params.itemsView === "full"));
  const itemPages = rpc.requests.filter((entry) => entry.method === "thread/items/list");
  assert.deepEqual(itemPages.map((entry) => entry.params), [{
    threadId,
    turnId: "turn-early",
    limit: 100,
    sortDirection: "asc",
  }]);
  assert.equal(rpc.requests.some((entry) => entry.method === "thread/read"), false);

  const snapshot = await adapter.snapshot();
  assert.deepEqual(snapshot.messages.map((message) => [message.turnId, message.text]), [
    ["turn-early", "first question"],
    ["turn-early", "first answer"],
    ["turn-middle", "second question"],
    ["turn-middle", "second answer"],
    ["turn-late", "third question"],
    ["turn-late", "third answer"],
  ]);
  assert.equal(snapshot.metadata.historyComplete, true);
  const outputSnapshot = events.find((event) => event.type === "output.snapshot");
  assert.equal(outputSnapshot.payload.historyComplete, true);
  assert.deepEqual(outputSnapshot.payload.messages.map((message) => message.text), [
    "first question",
    "first answer",
    "second question",
    "second answer",
    "third question",
    "third answer",
  ]);
  await adapter.dispose();
});

test("async adapter falls back to thread/read when resume omits existing history", async () => {
  const metadataThread = {
    id: "thread-paginated",
    preview: "existing conversation",
    cwd: "/tmp/project",
    createdAt: 1_700_000_000,
    updatedAt: 1_700_000_100,
    status: { type: "idle" },
    turns: [],
  };
  const rpc = new FakeRpc({
    "model/list": { data: [], nextCursor: null },
    "thread/resume": { thread: metadataThread, model: "gpt-5.6-sol", cwd: "/tmp/project" },
    "thread/read": {
      thread: {
        ...metadataThread,
        turns: [{
          id: "turn-read",
          status: "completed",
          items: [{ type: "agentMessage", id: "read-agent", text: "hydrated history" }],
        }],
      },
    },
  });
  const adapter = new CodexAgentAdapter({ approvalTimeoutMs: 0 }, rpc);
  await adapter.start();
  await adapter.selectSession({ threadId: "thread-paginated" });
  const read = rpc.requests.find((entry) => entry.method === "thread/read");
  assert.deepEqual(read.params, { threadId: "thread-paginated", includeTurns: true });
  assert.equal((await adapter.snapshot()).messages[0].text, "hydrated history");
  await adapter.dispose();
});

test("async adapter starts new sessions and sends flat durable thread settings", async () => {
  const rpc = new FakeRpc({
    "model/list": { data: [], nextCursor: null },
    "thread/start": {
      thread: { id: "thread-new", preview: "", cwd: "/tmp/new", status: { type: "idle" }, turns: [] },
      model: "gpt-5.6-sol",
      cwd: "/tmp/new",
      reasoningEffort: "medium",
    },
    "thread/settings/update": { ok: true },
  });
  const adapter = new CodexAgentAdapter({ approvalTimeoutMs: 0, defaultCwd: "/tmp/default" }, rpc);
  await adapter.start();
  await adapter.newSession({});
  await adapter.updateThreadSettings({
    threadSettings: {
      model: "gpt-5.6-terra",
      effort: "high",
      approvalPolicy: "on-request",
      approvalsReviewer: "user",
      sandboxPolicy: "workspace-write",
      permissions: ":workspace",
    },
  });
  const start = rpc.requests.find((entry) => entry.method === "thread/start");
  assert.equal(start.params.cwd, "/tmp/default");
  const update = rpc.requests.find((entry) => entry.method === "thread/settings/update");
  assert.deepEqual(update.params, {
    threadId: "thread-new",
    model: "gpt-5.6-terra",
    effort: "high",
    approvalPolicy: "on-request",
    approvalsReviewer: "user",
    permissions: ":workspace",
  });
  assert.equal(Object.prototype.hasOwnProperty.call(update.params, "sandboxPolicy"), false);
  const snapshot = await adapter.snapshot();
  assert.equal(snapshot.metadata.model, "gpt-5.6-terra");
  assert.equal(snapshot.metadata.latestReasoningEffort, "high");
  assert.equal(snapshot.metadata.sandboxPolicy, "workspace-write");
  await adapter.dispose();
});

test("thread settings updates require the send_task_input capability", async () => {
  const rpc = new FakeRpc();
  const adapter = new CodexAgentAdapter({ approvalTimeoutMs: 0 }, rpc);
  await adapter.start();
  const relay = new FakeRelay();
  const host = new RelayHost({
    adapter,
    relay,
    capabilities: ["read_output"],
    sessionId: "test-session",
  });

  await host.handleFrame({
    kind: "command",
    type: "thread.settings.update",
    commandId: "settings-without-capability",
    actor: { role: "operator" },
    payload: { threadSettings: { model: "gpt-5.6-sol", effort: "high" } },
  });

  const result = relay.frames.find((frame) => frame.payload?.commandId === "settings-without-capability");
  assert.equal(result.type, "command.rejected");
  assert.match(result.payload.error, /missing capability: send_task_input/);
  await adapter.dispose();
});

test("RelayHost exposes session list as read-only and protects session selection", async () => {
  const relay = new FakeRelay();
  const calls = [];
  const adapter = {
    async start() {},
    async sendInput() { return {}; },
    async cancel() { return {}; },
    async respondApproval() { return {}; },
    async snapshot() { return { threadId: "thread-a", turnId: null, state: "idle", pendingApprovals: [], outputTail: "" }; },
    onEvent() { return { dispose() {} }; },
    async dispose() {},
    async listSessions(params) {
      calls.push({ method: "listSessions", params });
      return { sessions: [{ threadId: "thread-a", title: "A", updatedAtMs: null, active: true, available: true }], activeThreadId: "thread-a" };
    },
    async selectSession(params) {
      calls.push({ method: "selectSession", params });
      return { threadId: params.threadId, previousThreadId: "thread-a", switched: true, available: true };
    },
    async newSession(params) {
      calls.push({ method: "newSession", params });
      return { opened: true, command: "chatgpt.newCodexPanel" };
    },
    getControlMode() { return "sync"; },
    async setControlMode(params) {
      calls.push({ method: "setControlMode", params });
      return { changed: true, controlMode: params.mode, previousControlMode: "sync", modeEpoch: 1 };
    },
  };
  const host = new RelayHost({
    adapter,
    relay,
    capabilities: ["read_output", "send_task_input"],
    sessionId: "test-session",
  });

  await host.handleFrame({ kind: "command", type: "session/list", commandId: "list-1", actor: { role: "viewer" }, payload: {} });
  const listed = relay.frames.find((frame) => frame.payload?.commandId === "list-1");
  assert.equal(listed.type, "command.accepted");
  assert.equal(listed.payload.result.activeThreadId, "thread-a");
  assert.equal(calls[0].method, "listSessions");

  await host.handleFrame({ kind: "command", type: "session/select", commandId: "select-viewer", actor: { role: "viewer" }, payload: { threadId: "thread-b" } });
  const denied = relay.frames.find((frame) => frame.payload?.commandId === "select-viewer");
  assert.equal(denied.type, "command.rejected");

  await host.handleFrame({ kind: "command", type: "session/select", commandId: "select-operator", actor: { role: "operator" }, payload: { threadId: "thread-b" } });
  const selected = relay.frames.find((frame) => frame.payload?.commandId === "select-operator");
  assert.equal(selected.type, "command.accepted");
  assert.equal(selected.payload.result.threadId, "thread-b");
  assert.equal(calls.at(-1).method, "selectSession");

  await host.handleFrame({ kind: "command", type: "session/new", commandId: "new-viewer", actor: { role: "viewer" }, payload: {} });
  const deniedNew = relay.frames.find((frame) => frame.payload?.commandId === "new-viewer");
  assert.equal(deniedNew.type, "command.rejected");

  await host.handleFrame({ kind: "command", type: "session/new", commandId: "new-operator", actor: { role: "operator" }, payload: {} });
  const opened = relay.frames.find((frame) => frame.payload?.commandId === "new-operator");
  assert.equal(opened.type, "command.accepted");
  assert.equal(opened.payload.result.command, "chatgpt.newCodexPanel");
  assert.equal(calls.at(-1).method, "newSession");

  await host.handleFrame({ kind: "command", type: "control/mode/get", commandId: "mode-get-viewer", actor: { role: "viewer" }, payload: {} });
  const mode = relay.frames.find((frame) => frame.payload?.commandId === "mode-get-viewer");
  assert.equal(mode.type, "command.accepted");
  assert.equal(mode.payload.result.mode, "sync");

  await host.handleFrame({ kind: "command", type: "control/mode/set", commandId: "mode-set-viewer", actor: { role: "viewer" }, payload: { mode: "async" } });
  const deniedMode = relay.frames.find((frame) => frame.payload?.commandId === "mode-set-viewer");
  assert.equal(deniedMode.type, "command.rejected");

  await host.handleFrame({ kind: "command", type: "control/mode/set", commandId: "mode-set-operator", actor: { role: "operator" }, payload: { mode: "async" } });
  const changedMode = relay.frames.find((frame) => frame.payload?.commandId === "mode-set-operator");
  assert.equal(changedMode.type, "command.accepted");
  assert.equal(changedMode.payload.result.controlMode, "async");
  assert.equal(calls.at(-1).method, "setControlMode");
});

test("approval decision conflicts and unknown tagged objects fail closed", async () => {
  const rpc = new FakeRpc();
  const adapter = new CodexAgentAdapter({ approvalTimeoutMs: 0 }, rpc);
  await adapter.start();
  const relay = new FakeRelay();
  const host = new RelayHost({
    adapter,
    relay,
    capabilities: ["read_output", "send_task_input", "cancel_task", "approve_low_risk"],
    sessionId: "test-session",
  });

  rpc.emitRequest({
    id: 3,
    method: "execCommandApproval",
    params: { conversationId: "thread-test", callId: "call-3", command: ["echo", "safe"] },
  });
  await host.handleFrame({
    kind: "command",
    type: "approval.respond",
    commandId: "conflicting-response",
    actor: { role: "operator" },
    payload: {
      requestId: 3,
      decision: "deny",
      response: { decision: "approved_mcp_policy_amendment" },
    },
  });
  assert.deepEqual(rpc.responses[0], {
    id: 3,
    result: { decision: { denied: { rejection: "approval response implies allow, but decision is deny" } } },
  });

  rpc.emitRequest({
    id: 4,
    method: "item/commandExecution/requestApproval",
    params: { threadId: "thread-test", turnId: "turn-test", itemId: "item-4", command: "echo safe" },
  });
  await host.handleFrame({
    kind: "command",
    type: "approval.respond",
    commandId: "unknown-tagged-response",
    actor: { role: "operator" },
    payload: {
      requestId: 4,
      decision: "allow",
      response: { decision: { futurePolicyGrant: { scope: "all" } } },
    },
  });
  assert.deepEqual(rpc.responses[1], {
    id: 4,
    result: { decision: "decline" },
  });
  await adapter.dispose();
});
