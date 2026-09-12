import { createInterface, Interface as ReadLineInterface } from "node:readline";
import WebSocket from "ws";

import {
  Disposable,
  isRecord,
  JsonObject,
  Logger,
  RelayFrame,
  RelayHelloFrame,
  RelayTransport,
} from "./protocol";

export interface RelayClientOptions {
  url: string;
  accessToken?: string;
  sessionId?: string;
  lastSeq?: number;
  reconnect?: boolean;
  reconnectInitialMs?: number;
  reconnectMaxMs?: number;
  maxFrameBytes?: number;
  maxQueuedBytes?: number;
  logger?: Logger;
  /** Injectable constructor for tests or a browser-compatible WebSocket. */
  webSocket?: new (url: string) => unknown;
}

type SocketLike = {
  readyState?: number;
  send(data: string): void;
  close(): void;
  on?(event: string, listener: (...args: any[]) => void): void;
  addEventListener?(event: string, listener: (...args: any[]) => void): void;
};

const OPEN = 1;
// A structured Codex history snapshot is routinely larger than 256 KiB even
// though its plain-text tail is capped. Keep a bounded limit, but leave enough
// room for the message/tool projection of a long attached conversation.
export const DEFAULT_MAX_RELAY_FRAME_BYTES = 16 * 1024 * 1024;
export const DEFAULT_MAX_RELAY_QUEUE_BYTES = DEFAULT_MAX_RELAY_FRAME_BYTES + 2 * 1024 * 1024;

interface QueuedFrame {
  serialized: string;
  bytes: number;
  projectionKey?: string;
}

const QUEUED_PROJECTION_TYPES = new Set(["session.snapshot", "output.snapshot", "output.chunk"]);

function queuedProjectionKey(frame: RelayFrame): string | undefined {
  if (!isRecord(frame) || frame.kind !== "event" || typeof frame.type !== "string"
    || !QUEUED_PROJECTION_TYPES.has(frame.type)) return undefined;
  const sessionId = typeof frame.sessionId === "string" ? frame.sessionId : "default";
  return `${sessionId}:transcript`;
}

/** WebSocket relay transport with bounded reconnect and frame validation. */
export class RelayClient implements RelayTransport {
  readonly handlesHandshake = true;
  private readonly options: Required<
    Pick<RelayClientOptions, "reconnect" | "reconnectInitialMs" | "reconnectMaxMs" | "maxFrameBytes" | "maxQueuedBytes">
  > &
    Omit<RelayClientOptions, "reconnect" | "reconnectInitialMs" | "reconnectMaxMs" | "maxFrameBytes" | "maxQueuedBytes">;
  private socket?: SocketLike;
  // A WebSocket can report OPEN while its relay authentication handshake is
  // still in flight. Keep this separate from `socket` so events emitted by
  // the adapter during reconnect are queued until the relay sends auth.ok.
  private authenticatedSocket?: SocketLike;
  private connecting?: Promise<void>;
  // Incremented whenever a connection attempt is replaced or explicitly
  // closed. Late events from an older WebSocket must not mutate newer state.
  private connectionGeneration = 0;
  private reconnectTimer?: NodeJS.Timeout;
  private stopped = false;
  private retryMs: number;
  private readonly queue: QueuedFrame[] = [];
  private queueBytes = 0;
  private readonly listeners = new Set<(frame: RelayFrame) => void>();
  private readonly openListeners = new Set<() => void>();
  private readonly closeListeners = new Set<(error?: Error) => void>();

  constructor(options: RelayClientOptions) {
    const maxFrameBytes = options.maxFrameBytes ?? DEFAULT_MAX_RELAY_FRAME_BYTES;
    const defaultMaxQueuedBytes = Math.max(
      maxFrameBytes,
      Math.min(DEFAULT_MAX_RELAY_QUEUE_BYTES, maxFrameBytes * 2),
    );
    this.options = {
      ...options,
      reconnect: options.reconnect ?? true,
      reconnectInitialMs: options.reconnectInitialMs ?? 500,
      reconnectMaxMs: options.reconnectMaxMs ?? 10_000,
      maxFrameBytes,
      maxQueuedBytes: Math.max(1, Math.floor(options.maxQueuedBytes ?? defaultMaxQueuedBytes)),
    };
    this.retryMs = this.options.reconnectInitialMs;
  }

  /** Let RelayHost assign its stable session id before the first hello. */
  setSessionId(sessionId: string): void {
    this.options.sessionId = sessionId;
  }

  async connect(): Promise<void> {
    this.stopped = false;
    if (this.socket?.readyState === OPEN && this.authenticatedSocket === this.socket) return;
    if (this.connecting) return this.connecting;

    const generation = ++this.connectionGeneration;
    let connectionPromise: Promise<void>;
    connectionPromise = new Promise<void>((resolve, reject) => {
      let settled = false;
      let authenticated = false;
      const SocketCtor = this.options.webSocket ?? WebSocket;
      let socket: SocketLike;
      try {
        socket = new SocketCtor(this.options.url) as SocketLike;
      } catch (error) {
        reject(error instanceof Error ? error : new Error(String(error)));
        return;
      }
      this.socket = socket;
      this.authenticatedSocket = undefined;

      const isCurrent = (): boolean => this.connectionGeneration === generation && this.socket === socket;

      const onOpen = (): void => {
        if (!isCurrent() || settled || authenticated) return;
        try {
          // The TCP/WebSocket open event is only a transport milestone. Do
          // not release queued commands until the relay has authenticated us.
          this.sendHello(socket);
        } catch (error) {
          if (!settled) {
            settled = true;
            reject(error instanceof Error ? error : new Error(String(error)));
          }
        }
      };
      const onMessage = (raw: unknown): void => {
        if (!isCurrent()) return;
        const data = extractMessageData(raw);
        if (Buffer.byteLength(data, "utf8") > this.options.maxFrameBytes) {
          this.options.logger?.warn?.("Ignoring oversized relay frame");
          return;
        }
        let frame: unknown;
        try {
          frame = JSON.parse(data);
        } catch {
          this.options.logger?.warn?.("Ignoring malformed relay JSON");
          return;
        }
        if (!isRecord(frame)) return;
        if (frame.type === "auth.ok" && !authenticated && !settled) {
          authenticated = true;
          settled = true;
          this.authenticatedSocket = socket;
          this.retryMs = this.options.reconnectInitialMs;
          try {
            this.flush(socket);
          } catch (error) {
            this.options.logger?.warn?.("Unable to flush relay queue after authentication", error);
          }
          for (const listener of this.openListeners) listener();
          resolve();
        } else if (frame.type === "error" && !authenticated && !settled) {
          settled = true;
          reject(new Error(typeof frame.message === "string" ? frame.message : "relay authentication failed"));
        }
        if (frame.kind === "event" && typeof frame.seq === "number") {
          this.options.lastSeq = Math.max(this.options.lastSeq ?? 0, frame.seq);
        }
        for (const listener of this.listeners) listener(frame as RelayFrame);
      };
      const onError = (raw: unknown): void => {
        if (!isCurrent()) return;
        const error = raw instanceof Error ? raw : new Error("relay websocket error");
        this.options.logger?.warn?.(error.message);
        if (!settled) {
          settled = true;
          reject(error);
        }
      };
      const onClose = (): void => {
        const current = isCurrent();
        if (current) {
          this.socket = undefined;
          if (this.authenticatedSocket === socket) this.authenticatedSocket = undefined;
        }
        const error = new Error("relay websocket closed");
        // A stale socket may still need to settle the promise returned to its
        // caller, but it must never notify the active host or schedule a
        // second reconnect loop.
        if (!current) {
          if (!settled) {
            settled = true;
            reject(error);
          }
          return;
        }
        for (const listener of this.closeListeners) listener(error);
        if (!settled) {
          settled = true;
          reject(error);
        }
        if (!this.stopped && this.options.reconnect) this.scheduleReconnect();
      };

      bindSocket(socket, onOpen, onMessage, onError, onClose);
      // A small number of test/browser WebSocket implementations can already
      // be OPEN by the time listeners are attached.
      if (socket.readyState === OPEN) queueMicrotask(onOpen);
    }).finally(() => {
      if (this.connectionGeneration === generation && this.connecting === connectionPromise) {
        this.connecting = undefined;
      }
    });
    this.connecting = connectionPromise;
    return connectionPromise;
  }

  send(frame: RelayFrame): void {
    const serialized = JSON.stringify(frame);
    const bytes = Buffer.byteLength(serialized, "utf8");
    if (bytes > this.options.maxFrameBytes) {
      throw new Error(`relay frame exceeds ${this.options.maxFrameBytes} bytes`);
    }
    if (this.socket?.readyState === OPEN && this.authenticatedSocket === this.socket) {
      this.socket.send(serialized);
      return;
    }
    this.enqueue({ serialized, bytes, projectionKey: queuedProjectionKey(frame) });
  }

  onMessage(listener: (frame: RelayFrame) => void): Disposable {
    this.listeners.add(listener);
    return { dispose: () => this.listeners.delete(listener) };
  }

  onOpen(listener: () => void): Disposable {
    this.openListeners.add(listener);
    return { dispose: () => this.openListeners.delete(listener) };
  }

  onClose(listener: (error?: Error) => void): Disposable {
    this.closeListeners.add(listener);
    return { dispose: () => this.closeListeners.delete(listener) };
  }

  close(): void {
    this.stopped = true;
    this.connectionGeneration += 1;
    if (this.reconnectTimer) clearTimeout(this.reconnectTimer);
    this.reconnectTimer = undefined;
    this.connecting = undefined;
    const socket = this.socket;
    this.socket = undefined;
    this.authenticatedSocket = undefined;
    if (socket && socket.readyState !== 3) socket.close();
    this.queue.length = 0;
    this.queueBytes = 0;
  }

  private sendHello(socket: SocketLike): void {
    const hello: RelayHelloFrame = {
      v: 1,
      kind: "hello",
      clientType: "host",
      protocol: 1,
      ...(this.options.sessionId ? { sessionId: this.options.sessionId } : {}),
      ...(this.options.lastSeq !== undefined ? { lastSeq: this.options.lastSeq } : {}),
    };
    socket.send(JSON.stringify(hello));
    if (this.options.accessToken) {
      // Keep authentication separate from hello so a relay can challenge the
      // host before accepting a bearer token (and so hello remains cacheable).
      socket.send(JSON.stringify({ v: 1, kind: "auth", accessToken: this.options.accessToken }));
    }
  }

  private flush(socket: SocketLike): void {
    if (socket.readyState !== OPEN || this.authenticatedSocket !== socket || this.socket !== socket) return;
    while (this.queue.length > 0) {
      const entry = this.queue.shift() as QueuedFrame;
      this.queueBytes = Math.max(0, this.queueBytes - entry.bytes);
      socket.send(entry.serialized);
    }
  }

  private enqueue(entry: QueuedFrame): void {
    // Transcript events are reconstructible: RelayHost publishes a fresh full
    // session snapshot after every authenticated reconnect. Keep only the
    // newest projection per session while preserving approval/command events.
    if (entry.projectionKey) {
      for (let index = this.queue.length - 1; index >= 0; index -= 1) {
        if (this.queue[index].projectionKey === entry.projectionKey) this.removeQueuedFrame(index);
      }
    }
    if (entry.bytes > this.options.maxQueuedBytes) {
      this.options.logger?.warn?.("Dropping relay frame that exceeds the reconnect queue byte limit");
      return;
    }
    while (this.queue.length >= 100 || this.queueBytes + entry.bytes > this.options.maxQueuedBytes) {
      const projectionIndex = this.queue.findIndex((queued) => Boolean(queued.projectionKey));
      if (projectionIndex >= 0) {
        this.removeQueuedFrame(projectionIndex);
        continue;
      }
      // Never evict an approval/command solely to retain a transcript delta;
      // the authoritative snapshot emitted after auth restores that state.
      if (entry.projectionKey) {
        this.options.logger?.debug?.("Dropping supersedable relay projection while reconnect queue is full");
        return;
      }
      this.removeQueuedFrame(0);
    }
    this.queue.push(entry);
    this.queueBytes += entry.bytes;
  }

  private removeQueuedFrame(index: number): void {
    const [removed] = this.queue.splice(index, 1);
    if (removed) this.queueBytes = Math.max(0, this.queueBytes - removed.bytes);
  }

  private scheduleReconnect(): void {
    if (this.reconnectTimer || this.stopped) return;
    const delay = this.retryMs;
    this.retryMs = Math.min(this.options.reconnectMaxMs, Math.max(this.retryMs * 2, this.options.reconnectInitialMs));
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = undefined;
      void this.connect().catch((error) => this.options.logger?.debug?.("relay reconnect failed", error));
    }, delay);
  }
}

/**
 * Line-oriented transport for local development and CI. Pipe it to a relay
 * process with `node dist/cli.js`; each line is one JSON relay frame.
 */
export class StdioRelayTransport implements RelayTransport {
  private readonly listeners = new Set<(frame: RelayFrame) => void>();
  private readonly lineReader: ReadLineInterface;
  private closed = false;

  constructor(
    private readonly input: NodeJS.ReadableStream = process.stdin,
    private readonly output: NodeJS.WritableStream = process.stdout,
    private readonly logger?: Logger,
  ) {
    this.lineReader = createInterface({ input, crlfDelay: Infinity });
    this.lineReader.on("line", (line) => {
      if (!line.trim()) return;
      try {
        const frame = JSON.parse(line);
        if (isRecord(frame)) for (const listener of this.listeners) listener(frame as RelayFrame);
      } catch (error) {
        this.logger?.warn?.("Ignoring malformed relay stdin frame", error);
      }
    });
  }

  async connect(): Promise<void> {
    this.closed = false;
  }

  send(frame: RelayFrame): void {
    if (this.closed) throw new Error("stdio relay transport is closed");
    this.output.write(`${JSON.stringify(frame)}\n`);
  }

  onMessage(listener: (frame: RelayFrame) => void): Disposable {
    this.listeners.add(listener);
    return { dispose: () => this.listeners.delete(listener) };
  }

  close(): void {
    this.closed = true;
    this.lineReader.close();
  }
}

function bindSocket(
  socket: SocketLike,
  onOpen: () => void,
  onMessage: (data: unknown) => void,
  onError: (error: unknown) => void,
  onClose: () => void,
): void {
  if (typeof socket.on === "function") {
    socket.on("open", onOpen);
    socket.on("message", onMessage);
    socket.on("error", onError);
    socket.on("close", onClose);
  } else if (typeof socket.addEventListener === "function") {
    socket.addEventListener("open", onOpen);
    socket.addEventListener("message", onMessage);
    socket.addEventListener("error", onError);
    socket.addEventListener("close", onClose);
  } else {
    onError(new Error("WebSocket implementation has no event API"));
  }
}

function extractMessageData(raw: unknown): string {
  if (typeof raw === "string") return raw;
  if (Buffer.isBuffer(raw)) return raw.toString("utf8");
  if (isRecord(raw) && "data" in raw) return extractMessageData(raw.data);
  return String(raw ?? "");
}
