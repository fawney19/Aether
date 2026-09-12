"use strict";

const assert = require("node:assert/strict");
const { chmodSync, mkdtempSync, mkdirSync, rmSync, writeFileSync } = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const test = require("node:test");

const { resolveCodexCommand } = require("../vscode-extension/dist/codexPath.js");
const { JsonlRpcClient } = require("../vscode-extension/dist/jsonlRpc.js");

function temporaryDirectory() {
  return mkdtempSync(path.join(os.tmpdir(), "codex-remote-path-"));
}

test("resolveCodexCommand finds a bare command in PATH", () => {
  const root = temporaryDirectory();
  try {
    const bin = path.join(root, "bin");
    const executable = path.join(bin, "codex-test");
    mkdirSync(bin);
    writeFileSync(executable, "#!/bin/sh\nexit 0\n");
    chmodSync(executable, 0o755);
    assert.equal(resolveCodexCommand("codex-test", { env: { PATH: bin }, platform: process.platform }), executable);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("resolveCodexCommand falls back to a per-user ChatGPT.app install", () => {
  const root = temporaryDirectory();
  try {
    const executable = path.join(root, "Applications", "ChatGPT.app", "Contents", "Resources", "codex");
    mkdirSync(path.dirname(executable), { recursive: true });
    writeFileSync(executable, "#!/bin/sh\nexit 0\n");
    chmodSync(executable, 0o755);
    assert.equal(
      resolveCodexCommand("codex", { env: { PATH: "/usr/bin:/bin" }, homeDir: root, platform: "darwin" }),
      executable,
    );
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("a missing explicit command reports a full-path setting hint", () => {
  assert.throws(
    () => resolveCodexCommand("/definitely/missing/codex", { platform: process.platform }),
    /Codex executable .* was not found.*codexRemoteCollab\.codexCommand.*full path/,
  );
});

test("JsonlRpcClient turns spawn ENOENT into an actionable error", async () => {
  const client = new JsonlRpcClient({ command: "/definitely/missing/codex", args: [] });
  await assert.rejects(() => client.start(), /Codex executable .* was not found.*codexRemoteCollab\.codexCommand/);
  client.close();
});
