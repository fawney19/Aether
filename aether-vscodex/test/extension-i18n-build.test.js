"use strict";

const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");

test("VS Code runtime strings have English and Simplified Chinese bundles", () => {
  const extensionRoot = path.join(__dirname, "..", "vscode-extension");
  const source = fs.readFileSync(path.join(extensionRoot, "src", "extension.ts"), "utf8");
  const manifest = JSON.parse(fs.readFileSync(path.join(extensionRoot, "package.json"), "utf8"));
  const english = JSON.parse(fs.readFileSync(path.join(extensionRoot, "l10n", "bundle.l10n.json"), "utf8"));
  const chinese = JSON.parse(fs.readFileSync(path.join(extensionRoot, "l10n", "bundle.l10n.zh-cn.json"), "utf8"));
  const keys = [...source.matchAll(/(?<![A-Za-z])t\("([^"]+)"/g)].map((match) => match[1]);

  assert.equal(manifest.l10n, "./l10n");
  assert.ok(keys.length > 20, "expected runtime-localized extension strings");
  for (const key of new Set(keys)) {
    assert.equal(english[key], key, `missing English source string: ${key}`);
    assert.equal(typeof chinese[key], "string", `missing zh-CN translation: ${key}`);
    assert.ok(chinese[key].length > 0, `empty zh-CN translation: ${key}`);
  }
});

test("production copy scripts require the Vue build instead of silently falling back", () => {
  const projectRoot = path.join(__dirname, "..");
  const extensionSync = fs.readFileSync(path.join(projectRoot, "vscode-extension", "scripts", "sync-local-relay.cjs"), "utf8");
  const aetherSync = fs.readFileSync(path.join(projectRoot, "..", "frontend", "scripts", "sync-vscodex.mjs"), "utf8");
  const extensionManifest = JSON.parse(fs.readFileSync(path.join(projectRoot, "vscode-extension", "package.json"), "utf8"));

  assert.match(extensionManifest.scripts["vscode:prepublish"], /build:web/);
  assert.doesNotMatch(extensionSync, /projectRoot,\s*"public"/);
  assert.doesNotMatch(aetherSync, /moduleRoot,\s*'public'/);
});
