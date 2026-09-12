"use strict";

const assert = require("node:assert/strict");
const test = require("node:test");

const { SwitchableAgentAdapter } = require("../vscode-extension/dist/switchableAgentAdapter.js");

class FakeAdapter {
  constructor(name, options = {}) {
    this.name = name;
    this.options = options;
    this.listeners = new Set();
    this.calls = [];
    this.disposed = false;
    this.snapshotValue = options.snapshot ?? idleSnapshot(name);
  }

  async start() {
    this.calls.push(["start"]);
    this.emit({ type: "candidate.starting", payload: { name: this.name } });
    if (this.options.startGate) await this.options.startGate.promise;
    if (this.options.startError) throw this.options.startError;
    this.emit({ type: "connection.opened", payload: { name: this.name } });
  }

  async startThread(params = {}) { return this.record("startThread", params); }
  async newSession(params = {}) { return this.record("newSession", params); }
  async startTurn(params) { return this.record("startTurn", params); }
  async steerTurn(params) { return this.record("steerTurn", params); }
  async updateThreadSettings(params) { return this.record("updateThreadSettings", params); }
  async listSessions(params = {}) { return this.record("listSessions", params); }
  async selectSession(params) { return this.record("selectSession", params); }
  async interruptTurn(params) { return this.record("interruptTurn", params); }
  async sendInput(text, params = {}) { return this.record("sendInput", { text, ...params }); }
  async cancel(taskId, params = {}) { return this.record("cancel", { taskId, ...params }); }
  async respondApproval(requestId, decision, reason, response) {
    return this.record("respondApproval", { requestId, decision, reason, response });
  }
  async denyPending(reason) { this.calls.push(["denyPending", reason]); }

  async snapshot() {
    this.calls.push(["snapshot"]);
    return structuredClone(this.snapshotValue);
  }

  onEvent(listener) {
    this.listeners.add(listener);
    return { dispose: () => this.listeners.delete(listener) };
  }

  emit(event) {
    for (const listener of this.listeners) listener(event);
  }

  async dispose() {
    this.calls.push(["dispose"]);
    this.disposed = true;
  }

  record(method, params) {
    this.calls.push([method, params]);
    return { adapter: this.name, method, params };
  }
}

function idleSnapshot(name) {
  return {
    threadId: `${name}-thread`,
    turnId: null,
    state: "idle",
    pendingApprovals: [],
    pendingRequests: [],
    outputTail: "",
    metadata: { adapter: name },
  };
}

function deferred() {
  let resolve;
  let reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

test("sync mode decorates snapshots and enforces VS Code-owned navigation", async () => {
  const sync = new FakeAdapter("sync");
  const adapter = new SwitchableAgentAdapter({ initialMode: "sync", createAdapter: () => sync });
  await adapter.start();

  const snapshot = await adapter.snapshot();
  assert.equal(snapshot.metadata.adapter, "sync");
  assert.equal(snapshot.metadata.mode, "sync");
  assert.equal(snapshot.metadata.controlMode, "sync");
  assert.equal(snapshot.metadata.modeEpoch, 0);
  assert.deepEqual(snapshot.metadata.capabilities, {
    followsVscodeRoute: true,
    sessionList: false,
    sessionSelect: false,
    sessionCreate: false,
    threadSettings: true,
  });

  await assert.rejects(adapter.listSessions(), /unavailable in sync mode/);
  await assert.rejects(adapter.selectSession({ threadId: "other" }), /unavailable in sync mode/);
  await assert.rejects(adapter.newSession(), /unavailable in sync mode/);
  await assert.rejects(adapter.startThread(), /unavailable in sync mode/);
  assert.equal((await adapter.sendInput("hello")).adapter, "sync");
  assert.equal((await adapter.updateThreadSettings({ model: "codex" })).adapter, "sync");
  await adapter.dispose();
});

test("async mode proxies the complete AgentAdapter surface", async () => {
  const independent = new FakeAdapter("async");
  const adapter = new SwitchableAgentAdapter({ initialMode: "async", createAdapter: () => independent });
  await adapter.start();

  await adapter.startThread({ cwd: "/workspace" });
  await adapter.newSession({ model: "codex" });
  await adapter.startTurn({ text: "start" });
  await adapter.steerTurn({ text: "steer" });
  await adapter.updateThreadSettings({ effort: "high" });
  await adapter.listSessions({ limit: 10 });
  await adapter.selectSession({ threadId: "thread-2" });
  await adapter.interruptTurn({ turnId: "turn-1" });
  await adapter.sendInput("input", { source: "web" });
  await adapter.cancel("turn-2", { reason: "user" });
  await adapter.respondApproval(7, "allow", "approved", { decision: "accept" });
  await adapter.denyPending("offline");

  assert.deepEqual(
    independent.calls.map(([method]) => method).filter((method) => !["start", "snapshot", "dispose"].includes(method)),
    [
      "startThread",
      "newSession",
      "startTurn",
      "steerTurn",
      "updateThreadSettings",
      "listSessions",
      "selectSession",
      "interruptTurn",
      "sendInput",
      "cancel",
      "respondApproval",
      "denyPending",
    ],
  );
  await adapter.dispose();
});

test("session/new falls back to thread/start for a minimal async adapter", async () => {
  const independent = new FakeAdapter("async");
  independent.newSession = undefined;
  const adapter = new SwitchableAgentAdapter({ initialMode: "async", createAdapter: () => independent });
  await adapter.start();

  const result = await adapter.newSession({ cwd: "/workspace" });
  assert.equal(result.method, "startThread");
  assert.equal((await adapter.snapshot()).metadata.capabilities.sessionCreate, true);
  await adapter.dispose();
});

test("mode switch commits atomically, buffers candidate events, and isolates the old generation", async () => {
  const sync = new FakeAdapter("sync");
  const gate = deferred();
  const asyncAdapter = new FakeAdapter("async", { startGate: gate });
  const adapter = new SwitchableAgentAdapter({
    initialMode: "sync",
    createAdapter: (mode) => mode === "sync" ? sync : asyncAdapter,
  });
  const events = [];
  adapter.onEvent((event) => events.push(`${event.type}:${event.payload.name ?? event.payload.controlMode ?? ""}`));
  await adapter.start();
  events.length = 0;

  const switching = adapter.setControlMode({ mode: "async" });
  await Promise.resolve();
  sync.emit({ type: "old.while-current", payload: { name: "sync" } });
  assert.deepEqual(events, ["old.while-current:sync"]);
  await assert.rejects(adapter.sendInput("racing input"), /mode is switching/);
  gate.resolve();

  const result = await switching;
  assert.deepEqual(result, {
    changed: true,
    controlMode: "async",
    previousControlMode: "sync",
    modeEpoch: 1,
  });
  assert.equal(sync.disposed, true);
  assert.equal(adapter.getControlMode(), "async");
  assert.ok(events.indexOf("control.mode.changed:async") < events.indexOf("candidate.starting:async"));
  assert.ok(events.includes("connection.opened:async"));

  sync.emit({ type: "old.after-commit", payload: { name: "sync" } });
  asyncAdapter.emit({ type: "new.after-commit", payload: { name: "async" } });
  assert.equal(events.includes("old.after-commit:sync"), false);
  assert.equal(events.includes("new.after-commit:async"), true);

  const snapshot = await adapter.snapshot();
  assert.equal(snapshot.metadata.modeEpoch, 1);
  assert.deepEqual(snapshot.metadata.capabilities, {
    followsVscodeRoute: false,
    sessionList: true,
    sessionSelect: true,
    sessionCreate: true,
    threadSettings: true,
  });
  assert.equal((await adapter.listSessions()).adapter, "async");
  assert.equal((await adapter.newSession()).adapter, "async");
  await adapter.dispose();
});

test("delegate snapshot events always carry authoritative mode metadata", async () => {
  const sync = new FakeAdapter("sync");
  const asyncAdapter = new FakeAdapter("async");
  const adapter = new SwitchableAgentAdapter({
    initialMode: "sync",
    createAdapter: (mode) => mode === "sync" ? sync : asyncAdapter,
  });
  const snapshots = [];
  adapter.onEvent((event) => {
    if (event.type === "session.snapshot") snapshots.push(event.payload);
  });
  await adapter.start();

  sync.emit({
    type: "session.snapshot",
    threadId: "sync-thread-2",
    payload: { threadId: "sync-thread-2", metadata: { adapter: "sync", route: "/thread/2" } },
  });
  assert.deepEqual(snapshots.at(-1).metadata, {
    adapter: "sync",
    route: "/thread/2",
    mode: "sync",
    controlMode: "sync",
    modeEpoch: 0,
    capabilities: {
      followsVscodeRoute: true,
      sessionList: false,
      sessionSelect: false,
      sessionCreate: false,
      threadSettings: true,
    },
  });

  await adapter.setControlMode({ mode: "async" });
  snapshots.length = 0;
  asyncAdapter.emit({
    type: "session.snapshot",
    threadId: "async-thread-2",
    payload: { threadId: "async-thread-2", metadata: { adapter: "async", title: "Second" } },
  });
  assert.equal(snapshots.length, 1);
  assert.equal(snapshots[0].metadata.adapter, "async");
  assert.equal(snapshots[0].metadata.title, "Second");
  assert.equal(snapshots[0].metadata.controlMode, "async");
  assert.equal(snapshots[0].metadata.modeEpoch, 1);
  assert.equal(snapshots[0].metadata.capabilities.followsVscodeRoute, false);
  assert.equal(snapshots[0].metadata.capabilities.sessionSelect, true);
  await adapter.dispose();
});

test("active turns and pending requests prevent a mode switch", async (t) => {
  const cases = [
    ["active turn", { ...idleSnapshot("sync"), turnId: "turn-1", state: "active" }],
    ["active state before a turn id arrives", { ...idleSnapshot("sync"), state: "in_progress" }],
    ["active runtime flag", { ...idleSnapshot("sync"), activeFlags: ["thinking"] }],
    ["pending approval", {
      ...idleSnapshot("sync"),
      pendingApprovals: [{ requestId: 1, method: "approval", action: "run", risk: "low", summary: "run", createdAt: 1, payload: {} }],
    }],
    ["pending input", {
      ...idleSnapshot("sync"),
      pendingRequests: [{ requestId: "input-1", method: "item/tool/requestUserInput" }],
    }],
  ];

  for (const [name, snapshot] of cases) {
    await t.test(name, async () => {
      const sync = new FakeAdapter("sync", { snapshot });
      let factoryCalls = 0;
      const adapter = new SwitchableAgentAdapter({
        initialMode: "sync",
        createAdapter: (mode) => {
          factoryCalls += 1;
          return mode === "sync" ? sync : new FakeAdapter("async");
        },
      });
      await adapter.start();
      await assert.rejects(adapter.setControlMode({ mode: "async" }), /turn or request is active/);
      assert.equal(factoryCalls, 1, "busy checks happen before creating a second adapter");
      assert.equal(adapter.getControlMode(), "sync");
      await adapter.dispose();
    });
  }
});

test("candidate startup failure leaves the old adapter authoritative", async () => {
  const sync = new FakeAdapter("sync");
  const failed = new FakeAdapter("async", { startError: new Error("candidate failed") });
  const adapter = new SwitchableAgentAdapter({
    initialMode: "sync",
    createAdapter: (mode) => mode === "sync" ? sync : failed,
  });
  const events = [];
  adapter.onEvent((event) => events.push(event.type));
  await adapter.start();
  events.length = 0;

  await assert.rejects(adapter.setControlMode({ controlMode: "async" }), /candidate failed/);
  assert.equal(adapter.getControlMode(), "sync");
  assert.equal(failed.disposed, true);
  assert.equal(sync.disposed, false);
  assert.equal(events.includes("candidate.starting"), false, "failed candidate events stay private");
  assert.equal((await adapter.sendInput("still attached")).adapter, "sync");
  assert.equal((await adapter.snapshot()).metadata.modeEpoch, 0);
  await adapter.dispose();
});

test("a mode factory cannot reuse the currently active adapter instance", async () => {
  const shared = new FakeAdapter("shared");
  const adapter = new SwitchableAgentAdapter({ initialMode: "sync", createAdapter: () => shared });
  await adapter.start();

  await assert.rejects(adapter.setControlMode({ mode: "async" }), /must return a distinct adapter/);
  assert.equal(adapter.getControlMode(), "sync");
  assert.equal(shared.disposed, false);
  assert.equal((await adapter.sendInput("still live")).adapter, "shared");
  await adapter.dispose();
});

test("listener failures cannot turn a committed switch into a rejected command", async () => {
  const sync = new FakeAdapter("sync");
  const asyncAdapter = new FakeAdapter("async");
  const adapter = new SwitchableAgentAdapter({
    initialMode: "sync",
    createAdapter: (mode) => mode === "sync" ? sync : asyncAdapter,
  });
  adapter.onEvent(() => { throw new Error("consumer failed"); });
  await adapter.start();

  const result = await adapter.setControlMode({ mode: "async" });
  assert.equal(result.changed, true);
  assert.equal(adapter.getControlMode(), "async");
  assert.equal(sync.disposed, true);
  await adapter.dispose();
});

test("a turn that appears while the candidate starts aborts before commit", async () => {
  const sync = new FakeAdapter("sync");
  const gate = deferred();
  const candidate = new FakeAdapter("async", { startGate: gate });
  const adapter = new SwitchableAgentAdapter({
    initialMode: "sync",
    createAdapter: (mode) => mode === "sync" ? sync : candidate,
  });
  await adapter.start();

  const switching = adapter.setControlMode({ mode: "async" });
  await Promise.resolve();
  sync.snapshotValue.turnId = "turn-race";
  sync.snapshotValue.state = "active";
  gate.resolve();

  await assert.rejects(switching, /turn or request is active/);
  assert.equal(adapter.getControlMode(), "sync");
  assert.equal(candidate.disposed, true);
  assert.equal(sync.disposed, false);
  sync.snapshotValue.turnId = null;
  sync.snapshotValue.state = "idle";
  await adapter.dispose();
});

test("control mode validation and idempotent switches are explicit", async () => {
  const sync = new FakeAdapter("sync");
  const adapter = new SwitchableAgentAdapter({ initialMode: "sync", createAdapter: () => sync });
  await adapter.start();

  await assert.rejects(adapter.setControlMode({ mode: "attach" }), /must be sync or async/);
  assert.deepEqual(await adapter.setControlMode({ mode: "sync" }), {
    changed: false,
    controlMode: "sync",
    previousControlMode: "sync",
    modeEpoch: 0,
  });
  await adapter.dispose();
});
