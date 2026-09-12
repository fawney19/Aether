import { Disposable, RelayFrame, RelayTransport } from "./protocol";

export interface NamedRelayTransport {
  id: string;
  transport: RelayTransport;
  required?: boolean;
}

/**
 * Fans host events out to local and cloud relays while presenting one
 * transport lifecycle to RelayHost. A temporary cloud outage must not stop
 * the local bridge (and vice versa).
 */
export class CompositeRelayTransport implements RelayTransport {
  readonly handlesHandshake = true;
  private readonly entries: NamedRelayTransport[];
  private readonly subscriptions: Disposable[] = [];
  private readonly openEntries = new Set<string>();
  private readonly messageListeners = new Set<(frame: RelayFrame) => void>();
  private readonly openListeners = new Set<() => void>();
  private readonly closeListeners = new Set<(error?: Error) => void>();
  private started = false;
  private sessionId?: string;

  constructor(entries: NamedRelayTransport[]) {
    if (entries.length === 0) throw new Error("CompositeRelayTransport requires at least one relay");
    const ids = new Set<string>();
    for (const entry of entries) {
      if (!entry.id || ids.has(entry.id)) throw new Error(`duplicate relay id: ${entry.id || "(empty)"}`);
      ids.add(entry.id);
    }
    this.entries = [...entries];
  }

  setSessionId(sessionId: string): void {
    this.sessionId = sessionId;
    for (const { transport } of this.entries) {
      (transport as RelayTransport & { setSessionId?: (value: string) => void }).setSessionId?.(sessionId);
    }
  }

  async connect(): Promise<void> {
    if (this.started) return;
    this.started = true;
    this.bindTransports();
    if (this.sessionId) this.setSessionId(this.sessionId);

    const results = await Promise.allSettled(this.entries.map(({ transport }) => transport.connect()));
    const failures = results
      .map((result, index) => ({ result, entry: this.entries[index] }))
      .filter((item): item is { result: PromiseRejectedResult; entry: NamedRelayTransport } => item.result.status === "rejected");
    const requiredFailure = failures.find(({ entry }) => entry.required);
    const connected = results.length - failures.length;
    if (requiredFailure || connected === 0) {
      this.started = false;
      this.disposeSubscriptions();
      for (const { transport } of this.entries) transport.close();
      const detail = failures.map(({ entry, result }) => `${entry.id}: ${errorMessage(result.reason)}`).join("; ");
      throw new Error(`unable to connect relay${failures.length === 1 ? "" : "s"}: ${detail}`);
    }
  }

  send(frame: RelayFrame): void {
    const failures: string[] = [];
    for (const { id, transport } of this.entries) {
      try {
        transport.send(frame);
      } catch (error) {
        failures.push(`${id}: ${errorMessage(error)}`);
      }
    }
    if (failures.length === this.entries.length) {
      throw new Error(`all relay sends failed: ${failures.join("; ")}`);
    }
  }

  onMessage(listener: (frame: RelayFrame) => void): Disposable {
    this.messageListeners.add(listener);
    return { dispose: () => this.messageListeners.delete(listener) };
  }

  onOpen(listener: () => void): Disposable {
    this.openListeners.add(listener);
    return { dispose: () => this.openListeners.delete(listener) };
  }

  onClose(listener: (error?: Error) => void): Disposable {
    this.closeListeners.add(listener);
    return { dispose: () => this.closeListeners.delete(listener) };
  }

  isConnected(id: string): boolean {
    return this.openEntries.has(id);
  }

  close(): void {
    this.started = false;
    this.openEntries.clear();
    this.disposeSubscriptions();
    for (const { transport } of this.entries) transport.close();
  }

  private bindTransports(): void {
    for (const { id, transport } of this.entries) {
      this.subscriptions.push(transport.onMessage((frame) => {
        for (const listener of this.messageListeners) listener(frame);
      }));
      if (transport.onOpen) this.subscriptions.push(transport.onOpen(() => {
        this.openEntries.add(id);
        // RelayHost publishes an authoritative snapshot after an authenticated
        // reconnect. Surface every member reconnect so a recovered cloud relay
        // is hydrated even while the local relay remained online.
        for (const listener of this.openListeners) listener();
      }));
      if (transport.onClose) this.subscriptions.push(transport.onClose((error) => {
        const wasOpen = this.openEntries.delete(id);
        if (wasOpen && this.openEntries.size === 0) {
          for (const listener of this.closeListeners) listener(error);
        }
      }));
    }
  }

  private disposeSubscriptions(): void {
    for (const subscription of this.subscriptions.splice(0)) subscription.dispose();
  }
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
