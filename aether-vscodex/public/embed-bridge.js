(function (root, factory) {
  "use strict";

  const api = factory();
  if (typeof module === "object" && module.exports) module.exports = api;
  if (!root || !root.document) return;

  const bridge = api.createAetherEmbedBridge(root);
  root.AetherVscodexEmbed = bridge;
  if (bridge.active) bridge.start();
})(typeof window === "object" ? window : undefined, function () {
  "use strict";

  const VERSION = 1;
  const PREFIX = "aether-vscodex/";
  const INBOUND_TYPES = new Set(["connect", "context", "disconnect", "error"]);

  function isAetherEmbed(locationLike) {
    try {
      return new URLSearchParams(locationLike?.search || "").get("embed") === "aether";
    } catch {
      return false;
    }
  }

  function normalizeTheme(value) {
    const theme = String(value || "").trim().toLowerCase();
    return theme === "dark" || theme === "light" ? theme : "system";
  }

  function createAetherEmbedBridge(windowLike) {
    const active = isAetherEmbed(windowLike.location);
    const listeners = new Map();
    const pending = new Map();
    let started = false;

    const emit = (name, payload) => {
      for (const listener of listeners.get(name) || []) listener(payload);
    };

    const post = (type, payload = {}) => {
      if (!active || windowLike.parent === windowLike) return false;
      windowLike.parent.postMessage({ v: VERSION, type: `${PREFIX}${type}`, ...payload }, windowLike.location.origin);
      return true;
    };

    const applyContext = (payload) => {
      if (payload.locale && windowLike.VscodexI18n?.setLocale) {
        windowLike.VscodexI18n.setLocale(payload.locale, { persist: false });
      }
      const theme = normalizeTheme(payload.theme);
      const documentElement = windowLike.document?.documentElement;
      if (documentElement) {
        if (theme === "system") delete documentElement.dataset.theme;
        else documentElement.dataset.theme = theme;
        documentElement.style.colorScheme = theme === "system" ? "" : theme;
      }
    };

    const handleMessage = (event) => {
      if (!active || event.origin !== windowLike.location.origin || event.source !== windowLike.parent) return;
      const message = event.data;
      if (!message || typeof message !== "object" || message.v !== VERSION || typeof message.type !== "string") return;
      if (!message.type.startsWith(PREFIX)) return;
      const name = message.type.slice(PREFIX.length);
      if (!INBOUND_TYPES.has(name)) return;
      if (name === "connect" || name === "context") applyContext(message);
      if (!(listeners.get(name)?.size)) pending.set(name, message);
      emit(name, message);
    };

    return {
      active,
      version: VERSION,
      start() {
        if (!active || started) return;
        started = true;
        windowLike.document.body?.classList.add("embed-aether");
        windowLike.addEventListener("message", handleMessage);
        post("ready");
      },
      stop() {
        if (!started) return;
        started = false;
        windowLike.removeEventListener("message", handleMessage);
        listeners.clear();
        pending.clear();
      },
      on(name, listener) {
        if (!INBOUND_TYPES.has(name) || typeof listener !== "function") return () => undefined;
        if (!listeners.has(name)) listeners.set(name, new Set());
        listeners.get(name).add(listener);
        if (pending.has(name)) {
          const message = pending.get(name);
          pending.delete(name);
          listener(message);
        }
        return () => listeners.get(name)?.delete(listener);
      },
      post,
      requestTicket(payload = {}) {
        return post("request-ticket", payload);
      },
      reportState(state, payload = {}) {
        return post("state", { state, ...payload });
      },
      _handleMessage: handleMessage,
    };
  }

  return { createAetherEmbedBridge, isAetherEmbed, normalizeTheme };
});
