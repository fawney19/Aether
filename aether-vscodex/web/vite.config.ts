/// <reference types="vitest/config" />

import { fileURLToPath, URL } from "node:url";

import vue from "@vitejs/plugin-vue";
import { defineConfig } from "vite";

const relayTarget = "http://127.0.0.1:8787";

export default defineConfig({
  // Relative assets let the same build run at the local relay root and under
  // Aether's /aether-vscodex/ static subpath.
  base: "./",
  plugins: [vue()],
  resolve: {
    alias: {
      "@": fileURLToPath(new URL("./src", import.meta.url)),
    },
  },
  server: {
    fs: {
      allow: [fileURLToPath(new URL("..", import.meta.url))],
    },
    proxy: {
      "/api": {
        target: relayTarget,
        changeOrigin: true,
      },
      "/ws": {
        target: relayTarget.replace("http", "ws"),
        changeOrigin: true,
        ws: true,
      },
    },
  },
  build: {
    outDir: "dist",
    emptyOutDir: true,
    assetsInlineLimit: 0,
  },
  test: {
    environment: "jsdom",
    include: ["tests/**/*.test.ts"],
  },
});
