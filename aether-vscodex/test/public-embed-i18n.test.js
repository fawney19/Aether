"use strict";

const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");

const { createAetherEmbedBridge, isAetherEmbed } = require("../public/embed-bridge.js");
const i18n = require("../public/i18n.js");

function embeddedWindow() {
  const listeners = new Map();
  const posts = [];
  const parent = { postMessage: (message, origin) => posts.push({ message, origin }) };
  const bodyClasses = new Set();
  const documentElement = { dataset: {}, style: {} };
  const windowLike = {
    location: { search: "?embed=aether", origin: "https://aether.example" },
    parent,
    document: {
      body: { classList: { add: (value) => bodyClasses.add(value) } },
      documentElement,
    },
    addEventListener: (name, listener) => listeners.set(name, listener),
    removeEventListener: (name, listener) => {
      if (listeners.get(name) === listener) listeners.delete(name);
    },
  };
  return { bodyClasses, documentElement, listeners, parent, posts, windowLike };
}

test("Aether embed mode is opt-in and announces readiness only to the same-origin parent", () => {
  assert.equal(isAetherEmbed({ search: "" }), false);
  assert.equal(isAetherEmbed({ search: "?embed=other" }), false);
  assert.equal(isAetherEmbed({ search: "?embed=aether" }), true);

  const fixture = embeddedWindow();
  const bridge = createAetherEmbedBridge(fixture.windowLike);
  assert.equal(bridge.active, true);
  bridge.start();
  assert.equal(fixture.bodyClasses.has("embed-aether"), true);
  assert.deepEqual(fixture.posts, [{
    message: { v: 1, type: "aether-vscodex/ready" },
    origin: "https://aether.example",
  }]);
});

test("Aether embed bridge rejects cross-origin and non-parent messages and buffers an early connect", () => {
  const fixture = embeddedWindow();
  const bridge = createAetherEmbedBridge(fixture.windowLike);
  bridge.start();
  const dispatch = fixture.listeners.get("message");
  const connect = {
    v: 1,
    type: "aether-vscodex/connect",
    ticket: "one-time-ticket",
    wsUrl: "/api/vscodex/ws",
    locale: "en-US",
    theme: "dark",
  };

  dispatch({ origin: "https://attacker.example", source: fixture.parent, data: connect });
  dispatch({ origin: "https://aether.example", source: {}, data: connect });
  let received = null;
  bridge.on("connect", (message) => { received = message; });
  assert.equal(received, null);

  dispatch({ origin: "https://aether.example", source: fixture.parent, data: connect });
  assert.equal(received.ticket, "one-time-ticket");
  assert.equal(fixture.documentElement.dataset.theme, "dark");

  const second = embeddedWindow();
  const bufferedBridge = createAetherEmbedBridge(second.windowLike);
  bufferedBridge.start();
  second.listeners.get("message")({ origin: "https://aether.example", source: second.parent, data: connect });
  let buffered = null;
  bufferedBridge.on("connect", (message) => { buffered = message; });
  assert.equal(buffered.ticket, "one-time-ticket");
});

test("bridge ticket requests never place the ticket in a URL", () => {
  const fixture = embeddedWindow();
  const bridge = createAetherEmbedBridge(fixture.windowLike);
  bridge.start();
  bridge.requestTicket({ reason: "disconnected", deviceId: "device-1" });
  assert.deepEqual(fixture.posts.at(-1), {
    message: {
      v: 1,
      type: "aether-vscodex/request-ticket",
      reason: "disconnected",
      deviceId: "device-1",
    },
    origin: "https://aether.example",
  });
});

test("locale dictionary covers static shell and core dynamic status text", () => {
  assert.equal(i18n.translate("设置", "en-US"), "Settings");
  assert.equal(i18n.translate("中文", "en-US"), "Chinese");
  assert.equal(i18n.translate("正在思考", "en-US"), "Thinking");
  assert.equal(i18n.translate("已读取这些内容 · 4 个文件", "en-US"), "Read these items · 4 files");
  assert.equal(i18n.translate("用时 3分45秒", "en-US"), "Worked for 3m45s");
  assert.equal(i18n.translate("修改权限，当前为需要时询问", "en-US"), "Change permissions. Current: Ask when needed");
  assert.equal(i18n.translate("模型设置更新失败：timeout", "en-US"), "Unable to update model settings: timeout");
  assert.equal(i18n.translate("请求 #17 已发送，等待 VS Code 主机确认", "en-US"), "Request #17 sent; waiting for the VS Code host");
  assert.equal(i18n.translate("无法读取 notes.md", "en-US"), "Unable to read notes.md");
  assert.equal(i18n.translate("命令: timed out", "en-US"), "Command: timed out");
  assert.equal(i18n.translate("命令: timed out（执行状态未知，请等待主机恢复）", "en-US"), "Command: timed out (execution status unknown; wait for the host to recover)");
  assert.equal(i18n.translate("子代理 失败", "en-US"), "Subagent failed");
  assert.equal(i18n.translate("已在 2秒 内运行 echo hi", "en-US"), "Ran echo hi in 2s");
  assert.equal(i18n.translate("命令运行失败 · echo hi · 2秒", "en-US"), "Command failed · echo hi · 2s");
  assert.equal(i18n.translate("命令运行失败 · echo hi", "en-US"), "Command failed · echo hi");
  assert.equal(i18n.translate("已停止 echo hi · 2秒", "en-US"), "Stopped echo hi · 2s");
  assert.equal(i18n.translate("文件变更 · 失败", "en-US"), "File changes · Failed");
  assert.equal(i18n.translate("文件变更 · 已中断", "en-US"), "File changes · Interrupted");
  assert.equal(i18n.translate("命令 · echo hi", "en-US"), "Command · echo hi");
  assert.equal(i18n.translate("命令 · 设置", "en-US"), "Command · 设置");
  assert.equal(i18n.translate("正在读取 设置", "en-US"), "Reading 设置");
  assert.equal(i18n.translate("已在 2秒 内运行 设置", "en-US"), "Ran 设置 in 2s");
  assert.equal(i18n.translate("正在切换到「设置」…", "en-US"), "Switching to “设置”...");
  assert.equal(i18n.translate("你停止了工作", "en-US"), "You stopped working");
  assert.equal(i18n.translate("工具失败", "en-US"), "Tool failed");
  assert.equal(i18n.translate("正在搜索", "en-US"), "Searching");
  assert.equal(i18n.translate("已工具 · 2秒", "en-US"), "Tool completed · 2s");
  assert.equal(i18n.translate("当前模型 5.6 Sol 标准，切换模型", "en-US"), "Current model: 5.6 Sol Medium. Change model");
  assert.equal(i18n.translate("编辑了文件", "en-US"), "Edited files");
  assert.equal(i18n.translate("编辑了文件 · 2秒", "en-US"), "Edited files · 2s");
  assert.equal(i18n.translate("已完成计划", "en-US"), "Completed plan");
  assert.equal(i18n.translate("已完成计划 · 2秒", "en-US"), "Completed plan · 2s");
  assert.equal(i18n.translate("…（文件已截断）", "en-US"), "... (file truncated)");
  assert.equal(i18n.translate("事件窗口已过期，请以当前快照为准", "en-US"), "The event window expired; the current snapshot is authoritative");
  assert.equal(i18n.translate("控制模式", "en-US"), "Control mode");
  assert.equal(i18n.translate("同步模式跟随 VS Code 当前会话", "en-US"), "Sync mode follows the current VS Code conversation");
  assert.equal(i18n.translate("异步模式可独立管理会话", "en-US"), "Async mode manages conversations independently");
  assert.equal(i18n.translate("当前任务或请求完成后才能切换控制模式", "en-US"), "The control mode can be changed after the current task or request finishes");
  assert.equal(i18n.translate("Settings", "zh-CN"), "设置");
  assert.equal(i18n.normalizeLocale("zh-Hans"), "zh-CN");
  assert.equal(i18n.normalizeLocale("en-GB"), "en-US");
});

test("renderer-owned dynamic labels have English fallbacks without translating host values", () => {
  assert.equal(i18n.translate("命令 · echo hi", "en-US"), "Command · echo hi");
  assert.equal(i18n.translate("你停止了工作", "en-US"), "You stopped working");
  assert.equal(i18n.translate("工具失败", "en-US"), "Tool failed");
  assert.equal(i18n.translate("正在搜索", "en-US"), "Searching");
  assert.equal(i18n.translate("当前模型 5.6 Sol 标准，切换模型", "en-US"), "Current model: 5.6 Sol Medium. Change model");

  const app = fs.readFileSync(path.join(__dirname, "..", "public", "app.js"), "utf8");
  // Command/path/title values are appended after a locale-specific prefix;
  // they are never passed through the translator as a whole.
  assert.match(app, /uiWithRaw\("正在运行 ", "Running ",/);
  assert.match(app, /uiWithRaw\("已读取 ", "Read ",/);
  assert.match(app, /uiLocale\(\) === "en-US" \? `Switching to/);
});

test("public shell uses relative assets and embedded startup skips the health probe", () => {
  const publicRoot = path.join(__dirname, "..", "public");
  const html = fs.readFileSync(path.join(publicRoot, "index.html"), "utf8");
  const app = fs.readFileSync(path.join(publicRoot, "app.js"), "utf8");
  assert.match(html, /href="\.\/style\.css"/);
  assert.match(html, /src="\.\/embed-bridge\.js"/);
  assert.match(html, /src="\.\/i18n\.js"/);
  assert.match(html, /src="\.\/app\.js"/);
  assert.match(app, /if \(embeddedInAether\)[\s\S]+else \{[\s\S]+fetch\("\.\/api\/health"/);
  assert.doesNotMatch(app, /ticket=.*state\.embedTicket/);
  assert.match(app, /empty\.textContent = t\(activity\.status === "inProgress" \? "正在读取文件" : "读取完成"\)/);
  assert.match(app, /outputContent\.textContent = t\("无输出"\)/);
  assert.match(app, /button\.title = t\(title\)/);
  assert.match(app, /activity\.action === "spawnAgent" \? t\("启动子代理"\)/);
  assert.match(app, /return t\("需要远程确认或输入"\)/);
  assert.match(app, /questionPrompt === undefined \|\| questionPrompt === null \? t\("请输入"\)/);
  assert.match(app, /checkbox\.setAttribute\("aria-label", t\(checkbox\.checked \? "已完成" : "未完成"\)\)/);
  assert.match(app, /window\.addEventListener\("aether-vscodex:locale", \(\) => \{[\s\S]+state\.activities\.values\(\)[\s\S]+renderRequests\(\)/);
});

test("control mode is snapshot-authoritative and gates independent session actions", () => {
  const publicRoot = path.join(__dirname, "..", "public");
  const html = fs.readFileSync(path.join(publicRoot, "index.html"), "utf8");
  const app = fs.readFileSync(path.join(publicRoot, "app.js"), "utf8");

  assert.match(html, /id="controlModeSwitch"[\s\S]+data-control-mode="sync"[\s\S]+data-control-mode="async"/);
  assert.match(app, /command\("control\/mode\/set", \{ mode \}\)/);
  assert.match(app, /applyControlModeSnapshot\(payload\.metadata\)/);
  assert.match(app, /const controlMetadata = \{[\s\S]+snapshot\.metadata[\s\S]+appState\.sessionMetadata[\s\S]+applyControlModeSnapshot\(controlMetadata\)/);
  assert.match(app, /sessionList: source\.sessionList === true/);
  assert.match(app, /Boolean\(state\.sessionListCommandId\)/);
  assert.match(app, /mode_switch_pending.*return "正在切换控制模式"/);
  assert.match(app, /mode_busy\|cannot switch control mode.*return "当前任务或请求完成后才能切换控制模式"/);
  assert.match(app, /setConversationStatus\(sessionErrorMessage\(message, "控制模式切换失败"\), "warning"\)/);
  assert.match(app, /if \(!sessionControlAllowed\("sessionList"\)\) return;/);
  assert.match(app, /if \(!sessionControlAllowed\("sessionSelect"\)\)/);
  assert.match(app, /if \(!sessionControlAllowed\("sessionCreate"\)\)/);
  assert.match(app, /sessionPickerButton\.disabled = !listAllowed/);
});
