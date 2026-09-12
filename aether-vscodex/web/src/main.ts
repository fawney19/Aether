import { createApp } from "vue";

import appRuntimeUrl from "../../public/app.js?url";
import embedBridgeUrl from "../../public/embed-bridge.js?url";
import i18nRuntimeUrl from "../../public/i18n.js?url";
import "../../public/style.css";
import App from "./App.vue";
import { installRequestTemplate } from "./runtime/request-template";

type RuntimeAsset = {
  id: string;
  url: string;
};

const runtimeAssets: RuntimeAsset[] = [
  { id: "vscodex-i18n-runtime", url: i18nRuntimeUrl },
  { id: "vscodex-embed-bridge", url: embedBridgeUrl },
  { id: "vscodex-compat-runtime", url: appRuntimeUrl },
];

function loadRuntimeAsset(asset: RuntimeAsset): Promise<void> {
  const existing = document.getElementById(asset.id) as HTMLScriptElement | null;
  if (existing?.dataset.loaded === "true") return Promise.resolve();

  return new Promise((resolve, reject) => {
    const script = existing ?? document.createElement("script");
    script.id = asset.id;
    script.async = false;
    script.src = asset.url;
    script.addEventListener("load", () => {
      script.dataset.loaded = "true";
      resolve();
    }, { once: true });
    script.addEventListener("error", () => reject(new Error(`Unable to load ${asset.id}`)), { once: true });
    if (!existing) document.body.append(script);
  });
}

async function startCompatibilityRuntime(): Promise<void> {
  for (const asset of runtimeAssets) await loadRuntimeAsset(asset);
}

createApp(App).mount("#app");
installRequestTemplate();

void startCompatibilityRuntime().catch((error: unknown) => {
  const message = error instanceof Error ? error.message : String(error);
  const status = document.getElementById("appState");
  if (status) status.textContent = message;
  document.body.dataset.runtimeError = "true";
  console.error("Failed to start the Codex compatibility runtime", error);
});
