const fs = require("node:fs");
const path = require("node:path");

const extensionRoot = path.resolve(__dirname, "..");
const projectRoot = path.resolve(extensionRoot, "..");
const outputRoot = path.join(extensionRoot, "dist", "local-relay");
const publicRoot = path.join(extensionRoot, "dist", "public");
const vuePublicRoot = path.join(projectRoot, "web", "dist");

if (!fs.existsSync(path.join(vuePublicRoot, "index.html"))) {
  throw new Error("web/dist is missing; run npm run build:web before building the extension");
}

fs.rmSync(outputRoot, { recursive: true, force: true });
fs.rmSync(publicRoot, { recursive: true, force: true });
fs.mkdirSync(outputRoot, { recursive: true });
fs.mkdirSync(publicRoot, { recursive: true });
fs.copyFileSync(path.join(projectRoot, "relay", "server.js"), path.join(outputRoot, "server.js"));
fs.cpSync(vuePublicRoot, publicRoot, { recursive: true });
