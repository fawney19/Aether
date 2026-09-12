"use strict";

const assert = require("node:assert/strict");
const net = require("node:net");
const os = require("node:os");
const path = require("node:path");
const { mkdtempSync, rmSync } = require("node:fs");
const test = require("node:test");

const {
  CODEX_IPC_METHOD_VERSIONS,
  CodexIpcClient,
  IpcFrameDecoder,
  applyIpcPatches,
  encodeIpcFrame,
} = require("../vscode-extension/dist/codexIpc.js");

function waitFor(predicate, timeoutMs = 2_000) {
  const started = Date.now();
  return new Promise((resolve, reject) => {
    const poll = () => {
      if (predicate()) return resolve();
      if (Date.now() - started >= timeoutMs) return reject(new Error("timed out waiting for fixture"));
      setTimeout(poll, 5);
    };
    poll();
  });
}

test("private IPC framing handles split UTF-8 frames", () => {
  const message = {
    type: "broadcast",
    method: "thread-stream-following-changed",
    sourceClientId: "client-1",
    version: 1,
    params: { conversationId: "thread-1", hostId: "local", following: true, text: "中文" },
  };
  const frame = encodeIpcFrame(message);
  const decoder = new IpcFrameDecoder();
  const first = decoder.push(frame.subarray(0, 3));
  assert.deepEqual(first, []);
  const second = decoder.push(frame.subarray(3, frame.length - 1));
  assert.deepEqual(second, []);
  assert.deepEqual(decoder.push(frame.subarray(frame.length - 1)), [message]);
});

test("applyIpcPatches updates a conversation snapshot", () => {
  const initial = { turns: [{ items: [{ text: "old" }] }], status: "idle" };
  const next = applyIpcPatches(initial, [
    { op: "replace", path: ["turns", 0, "items", 0, "text"], value: "new" },
    { op: "add", path: ["turns", 0, "items", 1], value: { text: "second" } },
    { op: "replace", path: ["status"], value: "active" },
  ]);
  assert.deepEqual(next, {
    turns: [{ items: [{ text: "new" }, { text: "second" }] }],
    status: "active",
  });
});

test("fixture owner receives follow/start/steer/interrupt/approval requests", async () => {
  const temp = mkdtempSync(path.join(os.tmpdir(), "codex-ipc-fixture-"));
  const socketPath = path.join(temp, "ipc.sock");
  const threadId = "11111111-1111-4111-8111-111111111111";
  const ownerId = "owner-client";
  const requests = [];
  const followingBroadcasts = [];
  let fixtureSocket;
  const server = net.createServer((socket) => {
    fixtureSocket = socket;
    const decoder = new IpcFrameDecoder();
    socket.on("data", (chunk) => {
      for (const message of decoder.push(chunk)) {
        if (message.type === "request" && message.method === "initialize") {
          socket.write(encodeIpcFrame({
            type: "response",
            requestId: message.requestId,
            resultType: "success",
            method: "initialize",
            handledByClientId: "fixture-client",
            result: { clientId: "fixture-client" },
          }));
          continue;
        }
        if (message.type === "broadcast" && message.method === "thread-stream-following-changed") {
          followingBroadcasts.push(message);
          const target = message.sourceClientId;
          socket.write(encodeIpcFrame({
            type: "broadcast",
            method: "thread-stream-state-changed",
            sourceClientId: ownerId,
            targetClientIds: [target],
            version: CODEX_IPC_METHOD_VERSIONS["thread-stream-state-changed"],
            params: {
              conversationId: threadId,
              hostId: "local",
              change: {
                type: "snapshot",
                revision: 1,
                conversationState: { id: threadId, title: "fixture", turns: [], requests: [] },
              },
            },
          }));
          continue;
        }
        if (message.type === "request") {
          requests.push(message);
          socket.write(encodeIpcFrame({
            type: "response",
            requestId: message.requestId,
            resultType: "success",
            method: message.method,
            handledByClientId: ownerId,
            result: { method: message.method, ok: true },
          }));
        }
      }
    });
  });

  try {
    await new Promise((resolve, reject) => {
      server.once("error", reject);
      server.listen(socketPath, resolve);
    });
    const client = new CodexIpcClient({ socketPath, autoReconnect: false });
    const streamEvents = [];
    client.onStreamEvent((event) => streamEvents.push(event));
    await client.connect();
    await client.followConversation(threadId);
    await waitFor(() => streamEvents.some((event) => event.kind === "snapshot"));
    assert.equal(client.getConversationState(threadId).ownerClientId, ownerId);
    fixtureSocket.write(encodeIpcFrame({
      type: "broadcast",
      method: "thread-stream-following-status-requested",
      sourceClientId: ownerId,
      targetClientIds: ["fixture-client"],
      version: CODEX_IPC_METHOD_VERSIONS["thread-stream-following-status-requested"],
      params: { conversationId: threadId, hostId: "local" },
    }));
    await waitFor(() => followingBroadcasts.length >= 2);
    assert.deepEqual(followingBroadcasts[1].targetClientIds, [ownerId]);
    assert.deepEqual(followingBroadcasts[1].params, {
      conversationId: threadId,
      hostId: "local",
      following: true,
    });

    await client.startTurn(threadId, "hello", { ownerClientId: ownerId });
    await client.steerTurn(threadId, "follow-up", { ownerClientId: ownerId });
    await client.updateThreadSettings(threadId, {
      model: "gpt-5.6-sol",
      effort: "ultra",
      multiAgentMode: "explicitRequestOnly",
    }, { ownerClientId: ownerId });
    await client.interruptTurn(threadId, { mode: "user-stop", expectedTurnId: "turn-1", ownerClientId: ownerId });
    await client.respondCommandApproval(threadId, 7, "decline", { ownerClientId: ownerId });
    await client.respondFileApproval(threadId, "8", "cancel", { ownerClientId: ownerId });
    await client.respondPermissionsApproval(threadId, 9, { permissions: {}, scope: "turn" }, { ownerClientId: ownerId });
    await client.respondUserInput(threadId, 10, { answers: {} }, { ownerClientId: ownerId });
    await client.respondMcpElicitation(threadId, 11, { action: "decline", content: null, _meta: null }, { ownerClientId: ownerId });

    assert.deepEqual(requests.map((request) => request.method), [
      "thread-follower-start-turn",
      "thread-follower-steer-turn",
      "thread-follower-update-thread-settings",
      "thread-follower-interrupt-turn",
      "thread-follower-command-approval-decision",
      "thread-follower-file-approval-decision",
      "thread-follower-permissions-request-approval-response",
      "thread-follower-submit-user-input",
      "thread-follower-submit-mcp-server-elicitation-response",
    ]);
    assert.deepEqual(requests[0].params, {
      conversationId: threadId,
      turnStart: {
        request: {
          threadId,
          input: [{ type: "text", text: "hello", text_elements: [] }],
        },
        context: { inheritThreadSettings: true },
      },
    });
    assert.deepEqual(requests[2].params, {
      conversationId: threadId,
      threadSettings: {
        model: "gpt-5.6-sol",
        effort: "ultra",
        multiAgentMode: "explicitRequestOnly",
      },
    });
    assert.equal(requests[2].version, 1);
    assert.deepEqual(requests[3].params, {
      conversationId: threadId,
      mode: "user-stop",
      expectedTurnId: "turn-1",
    });
    assert.equal(requests[3].version, 4);
    assert.deepEqual(requests[4].params, { conversationId: threadId, requestId: 7, decision: "decline" });
    assert.deepEqual(requests[8].params, {
      conversationId: threadId,
      requestId: 11,
      response: { action: "decline", content: null, _meta: null },
    });
    await client.dispose();
  } finally {
    await new Promise((resolve) => server.close(resolve));
    rmSync(temp, { recursive: true, force: true });
  }
});
