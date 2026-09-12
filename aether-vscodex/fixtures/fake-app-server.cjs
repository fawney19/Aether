"use strict";

const readline = require("node:readline");

let threadNumber = 0;
let turnNumber = 0;
let activeThread = null;
let activeTurn = null;

function send(message) {
  process.stdout.write(`${JSON.stringify(message)}\n`);
}

const input = readline.createInterface({ input: process.stdin });
input.on("line", (line) => {
  let request;
  try { request = JSON.parse(line); } catch { return; }
  if (request.method === "initialize") {
    send({ id: request.id, result: { userAgent: "fake", codexHome: "/tmp/codex" } });
    send({ method: "remoteControl/status/changed", params: { status: "disabled" } });
    return;
  }
  if (request.method === "thread/start") {
    activeThread = `thread-${++threadNumber}`;
    send({ id: request.id, result: { thread: { id: activeThread }, cwd: request.params?.cwd || "/tmp" } });
    send({ method: "thread/started", params: { thread: { id: activeThread } } });
    return;
  }
  if (request.method === "turn/start") {
    activeTurn = `turn-${++turnNumber}`;
    send({ id: request.id, result: { turn: { id: activeTurn } } });
    send({ method: "turn/started", params: { threadId: request.params.threadId, turn: { id: activeTurn } } });
    const text = request.params.input?.[0]?.text || "";
    send({ method: "item/agentMessage/delta", params: { threadId: request.params.threadId, turnId: activeTurn, itemId: "item-1", delta: `echo: ${text}` } });
    if (text.includes("approve")) {
      send({ id: 9001, method: "item/commandExecution/requestApproval", params: { threadId: request.params.threadId, turnId: activeTurn, itemId: "item-2", command: "echo approval" } });
    } else {
      send({ method: "turn/completed", params: { threadId: request.params.threadId, turn: { id: activeTurn } } });
      activeTurn = null;
    }
    return;
  }
  if (request.method === "turn/steer") {
    send({ id: request.id, result: { turn: { id: activeTurn } } });
    send({ method: "item/agentMessage/delta", params: { delta: `steered: ${request.params.input?.[0]?.text || ""}` } });
    return;
  }
  if (request.method === "turn/interrupt") {
    send({ id: request.id, result: {} });
    send({ method: "turn/completed", params: { threadId: request.params.threadId, turn: { id: request.params.turnId } } });
    activeTurn = null;
    return;
  }
  if (request.id === 9001 && (request.result || request.error)) {
    send({ method: "item/agentMessage/delta", params: { delta: `approval response: ${JSON.stringify(request.result || request.error)}` } });
    send({ method: "turn/completed", params: { threadId: activeThread, turn: { id: activeTurn } } });
    activeTurn = null;
  }
});
