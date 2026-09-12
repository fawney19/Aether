import { hostname } from "node:os";

import * as vscode from "vscode";

import { CodexAgentAdapter } from "./codexAgentAdapter";
import { resolveCodexCommand } from "./codexPath";
import { CodexIpcAgentAdapter } from "./codexIpcAgentAdapter";
import { CompositeRelayTransport } from "./compositeRelay";
import { LocalRelayController, localRelayTarget } from "./localRelay";
import { AgentAdapter, ControlMode, Disposable, JsonObject, Logger } from "./protocol";
import { RelayClient } from "./relayClient";
import { RelayHost } from "./relayHost";
import { SwitchableAgentAdapter } from "./switchableAgentAdapter";

let activeHost: RelayHost | undefined;
let activeAdapter: AgentAdapter | undefined;
let activeRelay: CompositeRelayTransport | undefined;
let activeAdapterStatusSubscription: Disposable | undefined;
let statusItem: vscode.StatusBarItem | undefined;
let autoStartRetryTimer: NodeJS.Timeout | undefined;
let autoStartRetryMs = 3_000;
let localRelayController: LocalRelayController | undefined;
const t = (message: string, ...args: Array<string | number | boolean>): string => vscode.l10n.t(message, ...args);

export async function activate(context: vscode.ExtensionContext): Promise<void> {
  const output = vscode.window.createOutputChannel(t("Codex Remote Collaboration"));
  context.subscriptions.push(output);
  const logger = {
    debug: (message: string, ...args: unknown[]) => output.appendLine(`[debug] ${message} ${formatArgs(args)}`),
    info: (message: string, ...args: unknown[]) => output.appendLine(`[info] ${message} ${formatArgs(args)}`),
    warn: (message: string, ...args: unknown[]) => output.appendLine(`[warn] ${message} ${formatArgs(args)}`),
    error: (message: string, ...args: unknown[]) => output.appendLine(`[error] ${message} ${formatArgs(args)}`),
  };
  localRelayController = new LocalRelayController({ extensionPath: context.extensionPath, logger });

  statusItem = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Right, 100);
  statusItem.command = "codexRemoteCollab.openWeb";
  statusItem.text = "$(plug) Codex Remote";
  statusItem.tooltip = t("Connecting to the local Codex collaboration service");
  statusItem.show();
  context.subscriptions.push(statusItem);

  const start = async (automatic = false): Promise<void> => {
    if (!automatic && autoStartRetryTimer) {
      clearTimeout(autoStartRetryTimer);
      autoStartRetryTimer = undefined;
    }
    if (activeHost) {
      if (!automatic) vscode.window.showInformationMessage(t("The Codex remote bridge is already running."));
      return;
    }
    const configuration = vscode.workspace.getConfiguration("codexRemoteCollab");
    const relayConfiguration = resolveRelayConfiguration(configuration);
    const localRelayUrl = relayConfiguration.localUrl;
    if (!localRelayUrl) {
      vscode.window.showWarningMessage(t("Set codexRemoteCollab.localRelayUrl before starting the bridge."));
      return;
    }
    const localTarget = localRelayTarget(localRelayUrl);
    if (!localTarget) {
      vscode.window.showErrorMessage(t("codexRemoteCollab.localRelayUrl must be a loopback ws:// address."));
      return;
    }
    if (localTarget && configuration.get<boolean>("autoStartLocalRelay", true)) {
      setStatus("$(sync~spin) Codex Remote", t("Starting {0}", localTarget.webUrl), "codexRemoteCollab.openWeb");
      try {
        await localRelayController?.ensureRunning(localRelayUrl);
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error);
        setStatus("$(error) Codex Remote", t("Unable to start the local collaboration service: {0}", message), "codexRemoteCollab.openWeb");
        if (automatic) {
          logger.warn(`Automatic local relay start failed; retrying in ${autoStartRetryMs}ms`, error);
          scheduleAutoStartRetry(start);
        } else {
          vscode.window.showErrorMessage(t("Unable to start the local Codex collaboration service: {0}", message));
        }
        return;
      }
    }
    const legacyToken = await context.secrets.get("codexRemoteCollab.relayToken");
    const localToken = relayConfiguration.legacyRemote ? undefined : legacyToken;
    if (!localToken) logger.info("Using the loopback-only unauthenticated local relay");
    const initialControlMode = resolveInitialControlMode(configuration);
    let currentControlMode: ControlMode = initialControlMode;
    const createAdapter = (controlMode: ControlMode): AgentAdapter => {
      if (controlMode === "sync") {
        const configuredThreadId = configuration.get<string>("threadId", "").trim();
        const socketPath = configuration.get<string>("ipcSocketPath", "").trim();
        logger.info(`Synchronous mode enabled; following the VS Code Codex panel${configuredThreadId ? ` (initial conversation ${configuredThreadId})` : ""}`);
        return new CodexIpcAgentAdapter({
          threadId: configuredThreadId || undefined,
          socketPath: socketPath || undefined,
          hostId: configuration.get<string>("hostId", "local"),
          autoDiscoverThread: configuration.get<boolean>("autoDiscoverThread", true),
          // Synchronous mode has one navigation owner: the official panel.
          followVscodeSession: true,
          preferredCwds: workspaceRoots(),
          strictVersions: configuration.get<boolean>("ipcStrictVersions", true),
          logger,
          approvalTimeoutMs: configuration.get<number>("approvalTimeoutMs", 300_000),
          openNewSession: () => openOfficialNewSession(logger),
        });
      }

      const configuredCommand = configuration.get<string>("codexCommand", "codex");
      const command = resolveCodexCommand(configuredCommand);
      const args = configuration.get<string[]>("codexArgs", ["app-server", "--stdio"]);
      const defaultCwd = configuration.get<string>("defaultCwd", "") || firstWorkspaceRoot();
      logger.info(`Asynchronous mode enabled; using independent Codex executable: ${command}`);
      return new CodexAgentAdapter({
        command,
        args,
        defaultCwd: defaultCwd || undefined,
        logger,
        approvalTimeoutMs: configuration.get<number>("approvalTimeoutMs", 300_000),
      });
    };
    const adapter = new SwitchableAgentAdapter({
      initialMode: initialControlMode,
      createAdapter,
      logger,
      onModeChanged: async (nextMode) => {
        currentControlMode = nextMode;
        await configuration.update("controlMode", nextMode, vscode.ConfigurationTarget.Global);
        setControlModeStatus(nextMode, nextMode === "async" || Boolean((await adapter.snapshot()).threadId));
      },
    });
    const localRelay = new RelayClient({
      url: localRelayUrl,
      ...(localToken ? { accessToken: localToken } : {}),
      reconnect: configuration.get<boolean>("relayReconnect", true),
      logger,
    });
    const relayEntries = [{ id: "local", transport: localRelay, required: true }];
    const cloudRelayUrl = relayConfiguration.cloudUrl;
    const cloudToken = await context.secrets.get("codexRemoteCollab.cloudRelayToken")
      ?? (relayConfiguration.legacyRemote ? legacyToken : undefined);
    if (cloudRelayUrl && cloudToken) {
      relayEntries.push({
        id: "aether-cloud",
        transport: new RelayClient({
          url: cloudRelayUrl,
          accessToken: cloudToken,
          reconnect: configuration.get<boolean>("relayReconnect", true),
          logger,
        }),
        required: false,
      });
      logger.info(`Aether cloud relay enabled: ${cloudRelayUrl}`);
    } else if (cloudRelayUrl) {
      logger.warn("Aether cloud relay URL is configured without a device credential; cloud sync is disabled until pairing is completed");
    }
    const relay = new CompositeRelayTransport(relayEntries);
    const capabilities = ["read_output", "send_task_input", "cancel_task", "approve_low_risk"];
    if (configuration.get<boolean>("allowHighRiskApprovals", false)) capabilities.push("approve_high_risk");
    const host = new RelayHost({ adapter, relay, logger, capabilities });
    let controlReady = initialControlMode === "async";
    activeAdapterStatusSubscription?.dispose();
    const adapterStatusSubscription = adapter.onEvent((event) => {
      if (activeAdapter !== adapter) return;
      if (event.type === "control.mode.changed") {
        const changedMode = event.payload.controlMode;
        if (changedMode === "sync" || changedMode === "async") currentControlMode = changedMode;
      }
      if (event.type !== "session.snapshot") return;
      const metadata = event.payload.metadata;
      if (metadata !== null && typeof metadata === "object" && !Array.isArray(metadata)) {
        const snapshotMode = (metadata as JsonObject).controlMode;
        if (snapshotMode === "sync" || snapshotMode === "async") currentControlMode = snapshotMode;
      }
      const waiting = event.payload.state === "waiting_for_host"
        || (metadata !== null && typeof metadata === "object" && !Array.isArray(metadata)
          && (metadata as JsonObject).waitingForSession === true);
      const threadId = event.threadId
        ?? (typeof event.payload.threadId === "string" ? event.payload.threadId : undefined);
      controlReady = currentControlMode === "async" || (Boolean(threadId) && !waiting);
      setControlModeStatus(currentControlMode, controlReady);
    });
    activeAdapterStatusSubscription = adapterStatusSubscription;
    activeAdapter = adapter;
    activeRelay = relay;
    activeHost = host;
    if (configuration.get<boolean>("autoStartLocalRelay", true)) {
      localRelay.onClose(() => {
        if (activeHost !== host) return;
        setStatus("$(sync~spin) Codex Remote", t("Restoring the local collaboration service"), "codexRemoteCollab.openWeb");
        void localRelayController?.ensureRunning(localRelayUrl).catch((error) => {
          logger.warn("Unable to recover bundled local relay", error);
          setStatus("$(error) Codex Remote", t("Unable to restore the local collaboration service"), "codexRemoteCollab.openWeb");
        });
      });
      localRelay.onOpen(() => {
        if (activeHost === host) {
          setControlModeStatus(currentControlMode, controlReady);
        }
      });
    }
    try {
      await host.start();
      autoStartRetryMs = 3_000;
      const snapshot = await adapter.snapshot();
      const snapshotMode = snapshot.metadata?.controlMode;
      if (snapshotMode === "sync" || snapshotMode === "async") currentControlMode = snapshotMode;
      controlReady = currentControlMode === "async" || Boolean(snapshot.threadId);
      setControlModeStatus(currentControlMode, controlReady);
      if (!automatic && (currentControlMode === "async" || controlReady)) {
        vscode.window.showInformationMessage(currentControlMode === "sync"
          ? t("The Codex remote bridge attached to the existing VS Code Codex conversation.")
          : t("The independent Codex remote mode connected."));
      }
    } catch (error) {
      activeHost = undefined;
      activeAdapter = undefined;
      activeRelay = undefined;
      if (activeAdapterStatusSubscription === adapterStatusSubscription) {
        activeAdapterStatusSubscription.dispose();
        activeAdapterStatusSubscription = undefined;
      }
      await host.stop().catch(() => undefined);
      const message = error instanceof Error ? error.message : String(error);
      if (initialControlMode === "sync" && isAttachSessionUnavailable(message)) {
        setStatus("$(sync~spin) Codex Remote", t("Waiting for a Codex conversation to open in VS Code. It will connect automatically."), "codexRemoteCollab.openWeb");
        logger.info(`No attachable VS Code Codex session is available; retrying in ${autoStartRetryMs}ms`);
        scheduleAutoStartRetry(start);
        return;
      }
      setStatus("$(error) Codex Remote", initialControlMode === "sync" ? t("The Codex conversation is not connected") : t("The independent Codex mode is not connected"), "codexRemoteCollab.openWeb");
      if (automatic) {
        logger.warn(`Automatic bridge start failed; retrying in ${autoStartRetryMs}ms`, error);
        scheduleAutoStartRetry(start);
      } else {
        const detail = localTarget && /ECONNREFUSED|connect refused/i.test(message)
          ? t("The local collaboration service at {0} is temporarily unavailable. The extension will keep retrying.", localTarget.webUrl)
          : t("Unable to start the Codex remote bridge: {0}", message);
        vscode.window.showErrorMessage(detail);
      }
    }
  };

  const stop = async (): Promise<void> => {
    if (autoStartRetryTimer) {
      clearTimeout(autoStartRetryTimer);
      autoStartRetryTimer = undefined;
    }
    autoStartRetryMs = 3_000;
    const host = activeHost;
    activeHost = undefined;
    activeAdapter = undefined;
    activeRelay = undefined;
    activeAdapterStatusSubscription?.dispose();
    activeAdapterStatusSubscription = undefined;
    if (host) await host.stop();
    setStatus("$(plug) Codex Remote", t("Bridge paused. Click to open the web control and resume automatically."), "codexRemoteCollab.openWeb");
  };

  context.subscriptions.push(vscode.commands.registerCommand("codexRemoteCollab.openWeb", async () => {
    const localRelayUrl = resolveRelayConfiguration(vscode.workspace.getConfiguration("codexRemoteCollab")).localUrl;
    const webUrl = localRelayController?.getWebUrl(localRelayUrl);
    if (!webUrl) {
      vscode.window.showErrorMessage(t("The local collaboration URL is invalid. Check codexRemoteCollab.localRelayUrl."));
      return;
    }
    if (!activeHost) await start(false);
    if (activeHost) await vscode.env.openExternal(vscode.Uri.parse(webUrl));
  }));
  context.subscriptions.push(vscode.commands.registerCommand("codexRemoteCollab.start", start));
  context.subscriptions.push(vscode.commands.registerCommand("codexRemoteCollab.stop", stop));
  context.subscriptions.push(vscode.commands.registerCommand("codexRemoteCollab.setThreadId", async () => {
    const configuration = vscode.workspace.getConfiguration("codexRemoteCollab");
    const current = configuration.get<string>("threadId", "");
    const value = await vscode.window.showInputBox({
      prompt: t("Existing Codex conversation ID (leave blank for auto-discovery)"),
      value: current,
      ignoreFocusOut: true,
    });
    if (value === undefined) return;
    await configuration.update("threadId", value.trim(), vscode.ConfigurationTarget.Global);
    vscode.window.showInformationMessage(value.trim()
      ? t("Codex Remote will attach to {0} after the next bridge start.", value.trim())
      : t("Codex Remote will auto-discover the latest VS Code Codex conversation after the next bridge start."));
  }));
  context.subscriptions.push(vscode.commands.registerCommand("codexRemoteCollab.setRelayToken", async () => {
    const token = await vscode.window.showInputBox({ prompt: t("Relay access token (leave blank for the local relay)"), password: true, ignoreFocusOut: true });
    if (token === undefined) return;
    await context.secrets.store("codexRemoteCollab.relayToken", token);
    vscode.window.showInformationMessage(t("Relay token stored in VS Code SecretStorage."));
  }));
  context.subscriptions.push(vscode.commands.registerCommand("codexRemoteCollab.configureCloud", async () => {
    const configuration = vscode.workspace.getConfiguration("codexRemoteCollab");
    const currentUrl = configuration.get<string>("cloudRelayUrl", "");
    const url = await vscode.window.showInputBox({
      prompt: t("Aether cloud relay WebSocket URL"),
      value: currentUrl,
      placeHolder: "wss://aether.example.com/api/vscodex/ws",
      ignoreFocusOut: true,
      validateInput: validateCloudRelayUrl,
    });
    if (url === undefined) return;
    if (!url.trim()) {
      await configuration.update("cloudRelayUrl", "", vscode.ConfigurationTarget.Global);
      await context.secrets.delete("codexRemoteCollab.cloudRelayToken");
      vscode.window.showInformationMessage(t("Aether cloud connection removed. Local control remains enabled."));
      return;
    }
    const token = await vscode.window.showInputBox({
      prompt: t("Device credential from the Aether pairing flow"),
      password: true,
      ignoreFocusOut: true,
    });
    if (token === undefined) return;
    if (!token.trim()) {
      vscode.window.showWarningMessage(t("A non-empty Aether device credential is required."));
      return;
    }
    await configuration.update("cloudRelayUrl", url.trim(), vscode.ConfigurationTarget.Global);
    await context.secrets.store("codexRemoteCollab.cloudRelayToken", token.trim());
    vscode.window.showInformationMessage(t("Aether cloud connection saved. Restart the Codex Remote bridge to connect; local control remains available."));
  }));
  context.subscriptions.push(vscode.commands.registerCommand("codexRemoteCollab.pairCloud", async () => {
    const configuration = vscode.workspace.getConfiguration("codexRemoteCollab");
    const currentBaseUrl = configuration.get<string>("aetherUrl", "");
    const baseUrl = await vscode.window.showInputBox({
      prompt: t("Aether server URL"),
      value: currentBaseUrl,
      placeHolder: "https://aether.example.com",
      ignoreFocusOut: true,
      validateInput: validateAetherBaseUrl,
    });
    if (baseUrl === undefined || !baseUrl.trim()) return;
    const code = await vscode.window.showInputBox({
      prompt: t("One-time pairing code shown in Aether"),
      placeHolder: "ABCD-EFGH",
      ignoreFocusOut: true,
      validateInput: (value) => normalizePairingCode(value).length === 8 ? undefined : t("Enter the 8-character pairing code."),
    });
    if (code === undefined || !code.trim()) return;
    try {
      const normalizedBaseUrl = baseUrl.trim().replace(/\/+$/, "");
      const response = await fetch(`${normalizedBaseUrl}/api/vscodex/pair`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ code: normalizePairingCode(code), name: hostname() || "VS Code" }),
      });
      const raw = await response.text();
      let result: unknown;
      try {
        result = JSON.parse(raw);
      } catch {
        result = null;
      }
      if (!response.ok) {
        const detail = isJsonRecord(result) && typeof result.error === "string" ? result.error : `HTTP ${response.status}`;
        throw new Error(detail);
      }
      if (!isJsonRecord(result) || typeof result.device_token !== "string" || typeof result.ws_url !== "string") {
        throw new Error(t("Aether returned an invalid pairing response."));
      }
      const wsError = validateCloudRelayUrl(result.ws_url);
      if (wsError) throw new Error(wsError);
      await configuration.update("aetherUrl", normalizedBaseUrl, vscode.ConfigurationTarget.Global);
      await configuration.update("cloudRelayUrl", result.ws_url, vscode.ConfigurationTarget.Global);
      await context.secrets.store("codexRemoteCollab.cloudRelayToken", result.device_token);
      if (activeHost) await stop();
      await start(false);
      if (!activeHost) return;
      if (activeRelay?.isConnected("aether-cloud")) {
        vscode.window.showInformationMessage(t("Aether pairing completed. Local and cloud control are both active."));
      } else {
        vscode.window.showWarningMessage(t("Aether pairing was saved, but the cloud connection is currently unavailable. Local control remains active and the cloud connection will retry."));
      }
    } catch (error) {
      vscode.window.showErrorMessage(t("Unable to pair with Aether: {0}", error instanceof Error ? error.message : String(error)));
    }
  }));
  context.subscriptions.push(vscode.commands.registerCommand("codexRemoteCollab.sendInput", async () => {
    if (!activeAdapter) {
      vscode.window.showWarningMessage(t("Start the Codex remote bridge first."));
      return;
    }
    const text = await vscode.window.showInputBox({ prompt: t("Send input to the active Codex turn"), ignoreFocusOut: true });
    if (text === undefined || !text.trim()) return;
    try {
      await activeAdapter.sendInput(text);
    } catch (error) {
      vscode.window.showErrorMessage(t("Unable to send Codex input: {0}", error instanceof Error ? error.message : String(error)));
    }
  }));
  context.subscriptions.push(vscode.commands.registerCommand("codexRemoteCollab.snapshot", async () => {
    if (!activeAdapter) return vscode.window.showWarningMessage(t("Start the Codex remote bridge first."));
    const snapshot = await activeAdapter.snapshot();
    output.appendLine(JSON.stringify(snapshot));
    output.show(true);
  }));

  if (vscode.workspace.getConfiguration("codexRemoteCollab").get<boolean>("autoStart", true)) await start(true);
}

export async function deactivate(): Promise<void> {
  if (autoStartRetryTimer) {
    clearTimeout(autoStartRetryTimer);
    autoStartRetryTimer = undefined;
  }
  const host = activeHost;
  activeHost = undefined;
  activeAdapter = undefined;
  activeAdapterStatusSubscription?.dispose();
  activeAdapterStatusSubscription = undefined;
  if (host) await host.stop();
  const localRelay = localRelayController;
  localRelayController = undefined;
  if (localRelay) await localRelay.stop();
}

function firstWorkspaceRoot(): string | undefined {
  return vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
}

function workspaceRoots(): string[] {
  return (vscode.workspace.workspaceFolders ?? []).map((folder) => folder.uri.fsPath);
}

function setStatus(text: string, tooltip: string, command?: string): void {
  if (!statusItem) return;
  statusItem.text = text;
  statusItem.tooltip = tooltip;
  statusItem.command = command;
}

function setAttachStatus(ready: boolean): void {
  setStatus(
    ready ? "$(check) Codex Remote" : "$(sync~spin) Codex Remote",
    ready ? t("Attached to the existing Codex conversation. Click to open the web control.") : t("Waiting for a Codex conversation to open in VS Code. It will connect automatically."),
    "codexRemoteCollab.openWeb",
  );
}

function setControlModeStatus(mode: ControlMode, ready: boolean): void {
  if (mode === "sync") {
    setAttachStatus(ready);
    return;
  }
  setStatus(
    ready ? "$(check) Codex Remote" : "$(sync~spin) Codex Remote",
    ready
      ? t("Independent Codex mode is connected. Click to open the web control.")
      : t("Starting the independent Codex mode."),
    "codexRemoteCollab.openWeb",
  );
}

function scheduleAutoStartRetry(start: (automatic?: boolean) => Promise<void>): void {
  if (autoStartRetryTimer) return;
  const delay = autoStartRetryMs;
  autoStartRetryMs = Math.min(autoStartRetryMs * 2, 30_000);
  autoStartRetryTimer = setTimeout(() => {
    autoStartRetryTimer = undefined;
    void start(true);
  }, delay);
}

function formatArgs(args: unknown[]): string {
  return args.length ? args.map((arg) => (typeof arg === "string" ? arg : JSON.stringify(arg))).join(" ") : "";
}

function isAttachSessionUnavailable(message: string): boolean {
  return message.includes("没有找到已打开的 VS Code Codex 会话")
    || /找不到会话\s+.+\s+的 VS Code Codex owner/.test(message);
}

function validateCloudRelayUrl(value: string): string | undefined {
  if (!value.trim()) return undefined;
  try {
    const url = new URL(value.trim());
    if (url.protocol !== "wss:" && url.protocol !== "ws:") return t("Use a ws:// or wss:// URL.");
    if (url.protocol === "ws:" && !isLoopbackHostname(url.hostname)) {
      return t("Remote Aether connections must use wss://.");
    }
    return undefined;
  } catch {
    return t("Enter a valid WebSocket URL.");
  }
}

function resolveRelayConfiguration(configuration: vscode.WorkspaceConfiguration): {
  localUrl: string;
  cloudUrl: string;
  legacyRemote: boolean;
} {
  const defaultLocalUrl = "ws://127.0.0.1:8787/v1/connect";
  const explicitLocal = inspectedValue<string>(configuration.inspect<string>("localRelayUrl"));
  const explicitCloud = inspectedValue<string>(configuration.inspect<string>("cloudRelayUrl"));
  const explicitLegacy = inspectedValue<string>(configuration.inspect<string>("relayUrl"));
  const legacyUrl = explicitLegacy?.trim() || "";
  const legacyRemote = Boolean(legacyUrl && !localRelayTarget(legacyUrl));
  const localUrl = (explicitLocal?.trim()
    || (!legacyRemote ? legacyUrl : "")
    || configuration.get<string>("localRelayUrl", defaultLocalUrl).trim()
    || defaultLocalUrl);
  const cloudUrl = explicitCloud?.trim()
    || (legacyRemote ? legacyUrl : "")
    || configuration.get<string>("cloudRelayUrl", "").trim();
  return { localUrl, cloudUrl, legacyRemote };
}

function inspectedValue<T>(inspection: ReturnType<vscode.WorkspaceConfiguration["inspect"]> | undefined): T | undefined {
  if (!inspection) return undefined;
  const values = inspection as {
    globalLanguageValue?: T;
    workspaceFolderLanguageValue?: T;
    workspaceLanguageValue?: T;
    workspaceFolderValue?: T;
    workspaceValue?: T;
    globalValue?: T;
  };
  return values.workspaceFolderLanguageValue
    ?? values.workspaceLanguageValue
    ?? values.globalLanguageValue
    ?? values.workspaceFolderValue
    ?? values.workspaceValue
    ?? values.globalValue;
}

function resolveInitialControlMode(configuration: vscode.WorkspaceConfiguration): ControlMode {
  const configured = inspectedValue<ControlMode>(configuration.inspect<ControlMode>("controlMode"));
  if (configured === "sync" || configured === "async") return configured;
  const legacyMode = inspectedValue<"attach" | "spawn">(configuration.inspect<"attach" | "spawn">("mode"));
  return legacyMode === "spawn" ? "async" : "sync";
}

function validateAetherBaseUrl(value: string): string | undefined {
  if (!value.trim()) return t("Enter the Aether server URL.");
  try {
    const url = new URL(value.trim());
    if (url.username || url.password || url.search || url.hash) return t("Use the Aether origin without credentials, a query, or a fragment.");
    if (url.protocol === "https:") return undefined;
    if (url.protocol === "http:" && isLoopbackHostname(url.hostname)) return undefined;
    return t("Remote Aether servers must use https://.");
  } catch {
    return t("Enter a valid URL.");
  }
}

function normalizePairingCode(value: string): string {
  return value.toUpperCase().replace(/[^A-Z2-9]/g, "");
}

function isLoopbackHostname(value: string): boolean {
  const hostname = value.replace(/^\[|\]$/g, "").toLowerCase();
  return hostname === "127.0.0.1" || hostname === "localhost" || hostname === "::1";
}

function isJsonRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/**
 * Reuse the official extension's command registry for the header's new-chat
 * action. This keeps the remote UI attached to the same VS Code Codex
 * installation and avoids launching a second app-server process.
 */
async function openOfficialNewSession(logger: Logger): Promise<JsonObject> {
  const commands = await vscode.commands.getCommands(true);
  const command = commands.includes("chatgpt.newCodexPanel")
    ? "chatgpt.newCodexPanel"
    : commands.includes("chatgpt.newChat")
      ? "chatgpt.newChat"
      : undefined;
  if (!command) {
    throw new Error(t("The official Codex extension new-conversation command was not found. Make sure the VS Code Codex extension is enabled."));
  }
  await vscode.commands.executeCommand(command);
  logger.info?.("Opened a new official Codex conversation with " + command);
  return { opened: true, command };
}
