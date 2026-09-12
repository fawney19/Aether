"use strict";

const assert = require("node:assert/strict");
const test = require("node:test");

const { CompositeRelayTransport } = require("../vscode-extension/dist/compositeRelay.js");

class FakeRelay {
  constructor({ connectError } = {}) {
    this.connectError = connectError;
    this.frames = [];
    this.closed = false;
    this.listeners = { message: new Set(), open: new Set(), close: new Set() };
  }

  async connect() {
    if (this.connectError) throw this.connectError;
    for (const listener of this.listeners.open) listener();
  }

  send(frame) { this.frames.push(frame); }
  close() { this.closed = true; }
  onMessage(listener) { return this.add("message", listener); }
  onOpen(listener) { return this.add("open", listener); }
  onClose(listener) { return this.add("close", listener); }
  add(type, listener) {
    this.listeners[type].add(listener);
    return { dispose: () => this.listeners[type].delete(listener) };
  }
  receive(frame) { for (const listener of this.listeners.message) listener(frame); }
  disconnect(error) { for (const listener of this.listeners.close) listener(error); }
}

test("CompositeRelayTransport keeps local control available when optional cloud connect fails", async () => {
  const local = new FakeRelay();
  const cloud = new FakeRelay({ connectError: new Error("cloud offline") });
  const relay = new CompositeRelayTransport([
    { id: "local", transport: local, required: true },
    { id: "cloud", transport: cloud },
  ]);

  await relay.connect();
  assert.equal(relay.isConnected("local"), true);
  assert.equal(relay.isConnected("cloud"), false);
  relay.send({ kind: "event", type: "session.snapshot" });
  assert.equal(local.frames.length, 1);
  assert.equal(cloud.frames.length, 1, "optional transport may queue events for reconnect");
  relay.close();
  assert.equal(local.closed, true);
  assert.equal(cloud.closed, true);
});

test("CompositeRelayTransport forwards commands and reports offline only after every relay closes", async () => {
  const local = new FakeRelay();
  const cloud = new FakeRelay();
  const relay = new CompositeRelayTransport([
    { id: "local", transport: local, required: true },
    { id: "cloud", transport: cloud },
  ]);
  const messages = [];
  const closes = [];
  relay.onMessage((frame) => messages.push(frame));
  relay.onClose((error) => closes.push(error?.message));

  await relay.connect();
  assert.equal(relay.isConnected("local"), true);
  assert.equal(relay.isConnected("cloud"), true);
  cloud.receive({ kind: "command", type: "turn.start" });
  assert.equal(messages.length, 1);
  local.disconnect(new Error("local offline"));
  assert.equal(relay.isConnected("local"), false);
  assert.deepEqual(closes, []);
  cloud.disconnect(new Error("cloud offline"));
  assert.deepEqual(closes, ["cloud offline"]);
  relay.close();
});

test("CompositeRelayTransport surfaces each member reconnect for snapshot hydration", async () => {
  const local = new FakeRelay();
  const cloud = new FakeRelay();
  const relay = new CompositeRelayTransport([
    { id: "local", transport: local, required: true },
    { id: "cloud", transport: cloud },
  ]);
  let opens = 0;
  relay.onOpen(() => { opens += 1; });
  await relay.connect();
  assert.equal(opens, 2);
  cloud.disconnect(new Error("cloud offline"));
  for (const listener of cloud.listeners.open) listener();
  assert.equal(opens, 3, "cloud recovery must prompt RelayHost to publish a fresh snapshot");
  relay.close();
});

test("CompositeRelayTransport fails when the required local relay cannot connect", async () => {
  const local = new FakeRelay({ connectError: new Error("local offline") });
  const cloud = new FakeRelay();
  const relay = new CompositeRelayTransport([
    { id: "local", transport: local, required: true },
    { id: "cloud", transport: cloud },
  ]);
  await assert.rejects(relay.connect(), /local: local offline/);
  assert.equal(local.closed, true);
  assert.equal(cloud.closed, true);
});
