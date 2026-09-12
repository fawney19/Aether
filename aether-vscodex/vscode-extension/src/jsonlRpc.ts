import { ChildProcessWithoutNullStreams, spawn } from "node:child_process";
import { createInterface, Interface as ReadLineInterface } from "node:readline";

import {
  asJsonValue,
  Disposable,
  isRecord,
  JsonRpcId,
  JsonRpcNotification,
  JsonRpcRequest,
  JsonValue,
  Logger,
  jsonRpcIdKey,
} from "./protocol";

export interface JsonlRpcClientOptions {
  command?: string;
  args?: string[];
  cwd?: string;
  env?: NodeJS.ProcessEnv;
  logger?: Logger;
  /** Optional request timeout. Zero disables it, which is useful for long turns. */
  requestTimeoutMs?: number;
}

interface PendingRequest {
  method: string;
  resolve: (value: JsonValue) => void;
  reject: (reason: Error) => void;
  timer?: NodeJS.Timeout;
}

export class JsonRpcRemoteError extends Error {
  constructor(
    message: string,
    readonly code: number,
    readonly data?: JsonValue,
  ) {
    super(message);
    this.name = "JsonRpcRemoteError";
  }
}

/** Minimal newline-delimited JSON-RPC client used by `codex app-server --stdio`. */
export class JsonlRpcClient {
  private readonly options: Required<Pick<JsonlRpcClientOptions, "command" | "args" | "requestTimeoutMs">> &
    Omit<JsonlRpcClientOptions, "command" | "args" | "requestTimeoutMs">;
  private child?: ChildProcessWithoutNullStreams;
  private stdoutLines?: ReadLineInterface;
  private nextId = 1;
  private readonly pending = new Map<string, PendingRequest>();
  private readonly notificationListeners = new Set<(message: JsonRpcNotification) => void>();
  private readonly requestListeners = new Set<(message: JsonRpcRequest) => void>();
  private readonly exitListeners = new Set<(error?: Error) => void>();

  constructor(options: JsonlRpcClientOptions = {}) {
    this.options = {
      command: options.command ?? "codex",
      args: options.args ?? ["app-server", "--stdio"],
      requestTimeoutMs: options.requestTimeoutMs ?? 0,
      cwd: options.cwd,
      env: options.env,
      logger: options.logger,
    };
  }

  get running(): boolean {
    return Boolean(this.child && this.child.exitCode === null && !this.child.killed);
  }

  async start(): Promise<void> {
    if (this.running) return;

    const child = spawn(this.options.command, this.options.args, {
      cwd: this.options.cwd,
      env: { ...process.env, ...(this.options.env ?? {}) },
      stdio: ["pipe", "pipe", "pipe"],
      windowsHide: true,
    });
    this.child = child;

    this.stdoutLines = createInterface({ input: child.stdout, crlfDelay: Infinity });
    this.stdoutLines.on("line", (line) => this.handleLine(line));
    child.stderr.on("data", (chunk: Buffer) => {
      const text = redactDiagnostic(chunk.toString("utf8").trim());
      if (text) this.options.logger?.debug?.(`[app-server stderr] ${text}`);
    });
    child.once("exit", (code, signal) => {
      const expected = child.killed;
      const error = expected
        ? undefined
        : new Error(`codex app-server exited (code=${String(code)}, signal=${String(signal)})`);
      this.handleExit(error);
    });

    await new Promise<void>((resolve, reject) => {
      const onSpawn = (): void => {
        child.off("error", onError);
        resolve();
      };
      const onError = (error: Error): void => {
        child.off("spawn", onSpawn);
        const spawnError = error as NodeJS.ErrnoException;
        if (spawnError.code === "ENOENT") {
          reject(new Error(`Codex executable "${this.options.command}" was not found. Set codexRemoteCollab.codexCommand to its full path.`));
          return;
        }
        reject(error);
      };
      child.once("spawn", onSpawn);
      child.once("error", onError);
    });
  }

  request(method: string, params?: JsonValue): Promise<JsonValue> {
    if (!this.running) return Promise.reject(new Error("app-server is not running"));
    const id = this.nextId++;

    return new Promise<JsonValue>((resolve, reject) => {
      const pending: PendingRequest = { method, resolve, reject };
      if (this.options.requestTimeoutMs > 0) {
        pending.timer = setTimeout(() => {
          this.pending.delete(jsonRpcIdKey(id));
          reject(new Error(`app-server request timed out: ${method}`));
        }, this.options.requestTimeoutMs);
      }
      this.pending.set(jsonRpcIdKey(id), pending);
      try {
        this.write({ id, method, ...(params === undefined ? {} : { params }) });
      } catch (error) {
        this.pending.delete(jsonRpcIdKey(id));
        if (pending.timer) clearTimeout(pending.timer);
        reject(error instanceof Error ? error : new Error(String(error)));
      }
    });
  }

  notify(method: string, params?: JsonValue): void {
    this.write({ method, ...(params === undefined ? {} : { params }) });
  }

  respond(id: JsonRpcId, result: JsonValue): void {
    this.write({ id, result });
  }

  respondError(id: JsonRpcId, code: number, message: string, data?: JsonValue): void {
    this.write({ id, error: { code, message, ...(data === undefined ? {} : { data }) } });
  }

  onNotification(listener: (message: JsonRpcNotification) => void): Disposable {
    this.notificationListeners.add(listener);
    return { dispose: () => this.notificationListeners.delete(listener) };
  }

  onServerRequest(listener: (message: JsonRpcRequest) => void): Disposable {
    this.requestListeners.add(listener);
    return { dispose: () => this.requestListeners.delete(listener) };
  }

  onExit(listener: (error?: Error) => void): Disposable {
    this.exitListeners.add(listener);
    return { dispose: () => this.exitListeners.delete(listener) };
  }

  close(): void {
    const child = this.child;
    this.child = undefined;
    this.stdoutLines?.close();
    this.stdoutLines = undefined;
    if (child && child.exitCode === null && !child.killed) child.kill();
    this.rejectAll(new Error("app-server client closed"));
  }

  private write(message: unknown): void {
    const child = this.child;
    if (!child || child.exitCode !== null || child.killed || !child.stdin.writable) {
      throw new Error("app-server is not running");
    }
    child.stdin.write(`${JSON.stringify(message)}\n`, "utf8");
  }

  private handleLine(line: string): void {
    const trimmed = line.trim();
    if (!trimmed) return;

    let message: unknown;
    try {
      message = JSON.parse(trimmed);
    } catch (error) {
      this.options.logger?.warn?.("Ignoring malformed app-server JSON", error, trimmed.slice(0, 500));
      return;
    }
    if (!isRecord(message)) return;

    const hasId = typeof message.id === "string" || typeof message.id === "number";
    const hasMethod = typeof message.method === "string";
    if (hasId && (Object.hasOwn(message, "result") || Object.hasOwn(message, "error")) && !hasMethod) {
      this.handleResponse(message as Record<string, unknown> & { id: JsonRpcId });
      return;
    }

    if (hasMethod && hasId) {
      const request: JsonRpcRequest = {
        id: message.id as JsonRpcId,
        method: message.method as string,
        ...(message.params === undefined ? {} : { params: asJsonValue(message.params) }),
      };
      for (const listener of this.requestListeners) listener(request);
      return;
    }

    if (hasMethod) {
      const notification: JsonRpcNotification = {
        method: message.method as string,
        ...(message.params === undefined ? {} : { params: asJsonValue(message.params) }),
      };
      for (const listener of this.notificationListeners) listener(notification);
      return;
    }

    this.options.logger?.warn?.("Ignoring unknown app-server message", message);
  }

  private handleResponse(message: Record<string, unknown> & { id: JsonRpcId }): void {
    const pending = this.pending.get(jsonRpcIdKey(message.id));
    if (!pending) {
      this.options.logger?.warn?.(`Received response for unknown app-server request ${String(message.id)}`);
      return;
    }
    this.pending.delete(jsonRpcIdKey(message.id));
    if (pending.timer) clearTimeout(pending.timer);

    if (isRecord(message.error)) {
      pending.reject(
        new JsonRpcRemoteError(
          typeof message.error.message === "string" ? message.error.message : `Request failed: ${pending.method}`,
          typeof message.error.code === "number" ? message.error.code : -32000,
          message.error.data === undefined ? undefined : asJsonValue(message.error.data),
        ),
      );
      return;
    }
    pending.resolve(message.result === undefined ? null : asJsonValue(message.result));
  }

  private handleExit(error?: Error): void {
    this.child = undefined;
    this.stdoutLines?.close();
    this.stdoutLines = undefined;
    this.rejectAll(error ?? new Error("app-server exited"));
    for (const listener of this.exitListeners) listener(error);
  }

  private rejectAll(error: Error): void {
    for (const request of this.pending.values()) {
      if (request.timer) clearTimeout(request.timer);
      request.reject(error);
    }
    this.pending.clear();
  }
}

function redactDiagnostic(text: string): string {
  return text
    .replace(/Bearer\s+[A-Za-z0-9._~+\-/]+=*/gi, "Bearer [REDACTED]")
    .replace(/\b(?:sk-[A-Za-z0-9_-]{12,}|gh[pousr]_[A-Za-z0-9_]{12,})\b/g, "[REDACTED]")
    .replace(/((?:token|secret|password|api[_-]?key)\s*[:=]\s*)[^\s,;]+/gi, "$1[REDACTED]");
}
