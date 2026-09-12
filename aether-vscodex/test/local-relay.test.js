const assert = require("node:assert/strict");
const http = require("node:http");
const path = require("node:path");
const test = require("node:test");

const {
  LocalRelayController,
  localRelayTarget,
  relayHealthAvailable,
} = require("../vscode-extension/dist/localRelay.js");

test("local relay target accepts only loopback ws URLs", () => {
  assert.deepEqual(localRelayTarget("ws://localhost:8898/v1/connect"), {
    host: "127.0.0.1",
    port: 8898,
    healthUrl: "http://127.0.0.1:8898/api/health",
    webUrl: "http://127.0.0.1:8898/",
  });
  assert.equal(localRelayTarget("wss://127.0.0.1:8898/v1/connect"), undefined);
  assert.equal(localRelayTarget("ws://192.168.1.10:8898/v1/connect"), undefined);
  assert.equal(localRelayTarget("not a url"), undefined);
});

test("local relay health probe recognizes a responding HTTP service", async (t) => {
  const server = http.createServer((request, response) => {
    if (request.url === "/api/health") {
      response.writeHead(200, { "content-type": "application/json" }).end(JSON.stringify({ ok: true }));
    } else if (request.url === "/aborted") {
      response.writeHead(200, { "content-type": "application/json" });
      response.write('{"ok":');
      response.destroy();
    } else if (request.url === "/drip") {
      response.writeHead(200, { "content-type": "application/json" });
      const interval = setInterval(() => response.write(" "), 10);
      response.on("close", () => clearInterval(interval));
    } else {
      response.writeHead(404).end();
    }
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  t.after(() => new Promise((resolve) => server.close(resolve)));
  const address = server.address();
  assert.equal(await relayHealthAvailable(`http://127.0.0.1:${address.port}/api/health`), true);
  assert.equal(await relayHealthAvailable(`http://127.0.0.1:${address.port}/missing`), false);
  assert.equal(await relayHealthAvailable(`http://127.0.0.1:${address.port}/aborted`, 100), false);
  const startedAt = Date.now();
  assert.equal(await relayHealthAvailable(`http://127.0.0.1:${address.port}/drip`, 50), false);
  assert.ok(Date.now() - startedAt < 500);
});

test("local relay controller starts and stops a bundled loopback relay", async () => {
  let starts = 0;
  let stops = 0;
  class FakeRelay {
    async start() { starts += 1; return { host: "127.0.0.1", port: 65534 }; }
    async stop() { stops += 1; }
  }
  const controller = new LocalRelayController({
    extensionPath: path.resolve(__dirname, "../vscode-extension"),
    probeTimeoutMs: 20,
    loadRelayModule: () => ({ CodexRelay: FakeRelay }),
  });
  assert.equal(await controller.ensureRunning("ws://127.0.0.1:65534/v1/connect"), true);
  assert.equal(starts, 1);
  await controller.stop();
  assert.equal(stops, 1);
});

test("local relay controller does not leak a relay when stopped during startup", async () => {
  let releaseStart;
  const startGate = new Promise((resolve) => { releaseStart = resolve; });
  let startEntered;
  const entered = new Promise((resolve) => { startEntered = resolve; });
  let stops = 0;
  class SlowRelay {
    async start() {
      startEntered();
      await startGate;
      return { host: "127.0.0.1", port: 65533 };
    }
    async stop() { stops += 1; }
  }
  const controller = new LocalRelayController({
    extensionPath: path.resolve(__dirname, "../vscode-extension"),
    probeTimeoutMs: 20,
    loadRelayModule: () => ({ CodexRelay: SlowRelay }),
  });
  const starting = controller.ensureRunning("ws://127.0.0.1:65533/v1/connect");
  await entered;
  const stopping = controller.stop();
  releaseStart();
  await Promise.all([starting, stopping]);
  assert.equal(stops, 1);
});

test("local relay controller does not start after stop wins an in-flight health probe", async () => {
  let resolveProbe;
  const probe = new Promise((resolve) => { resolveProbe = resolve; });
  let probeEntered;
  const entered = new Promise((resolve) => { probeEntered = resolve; });
  let starts = 0;
  class FakeRelay {
    async start() { starts += 1; return { host: "127.0.0.1", port: 65532 }; }
    async stop() {}
  }
  const controller = new LocalRelayController({
    extensionPath: path.resolve(__dirname, "../vscode-extension"),
    loadRelayModule: () => ({ CodexRelay: FakeRelay }),
    probeRelayHealth: async () => {
      probeEntered();
      return probe;
    },
  });
  const ensuring = controller.ensureRunning("ws://127.0.0.1:65532/v1/connect");
  await entered;
  await controller.stop();
  resolveProbe(false);
  assert.equal(await ensuring, false);
  assert.equal(starts, 0);
});
