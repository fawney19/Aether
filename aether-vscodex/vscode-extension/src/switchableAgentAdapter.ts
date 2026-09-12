import {
  AgentAdapter,
  AgentEvent,
  asJsonObject,
  ControlMode,
  Disposable,
  JsonObject,
  JsonRpcId,
  JsonValue,
  Logger,
  SessionSnapshot,
} from "./protocol";

export type AgentAdapterFactory = (mode: ControlMode) => AgentAdapter | Promise<AgentAdapter>;

export interface SwitchableAgentAdapterOptions {
  initialMode: ControlMode;
  createAdapter: AgentAdapterFactory;
  /** Persist the committed mode. Persistence errors do not roll back a live adapter. */
  onModeChanged?: (mode: ControlMode, previousMode: ControlMode) => void | Promise<void>;
  logger?: Logger;
}

interface AdapterBinding {
  adapter: AgentAdapter;
  mode: ControlMode;
  generation: number;
  committed: boolean;
  bufferedEvents: AgentEvent[];
  subscription: Disposable;
}

interface ModeCapabilities extends JsonObject {
  followsVscodeRoute: boolean;
  sessionList: boolean;
  sessionSelect: boolean;
  sessionCreate: boolean;
  threadSettings: boolean;
}

/**
 * Keeps RelayHost bound to one stable AgentAdapter while atomically replacing
 * the implementation behind it when the control owner changes.
 */
export class SwitchableAgentAdapter implements AgentAdapter {
  private readonly options: SwitchableAgentAdapterOptions;
  private readonly listeners = new Set<(event: AgentEvent) => void>();
  private binding: AdapterBinding | null = null;
  private controlMode: ControlMode;
  private modeEpoch = 0;
  private startPromise: Promise<void> | null = null;
  private switchPromise: Promise<JsonValue> | null = null;
  private started = false;
  private disposed = false;

  constructor(options: SwitchableAgentAdapterOptions) {
    this.options = options;
    this.controlMode = validateControlMode(options.initialMode);
  }

  async start(): Promise<void> {
    if (this.started) return;
    if (this.disposed) throw new Error("switchable adapter has been disposed");
    if (this.startPromise) return this.startPromise;

    const operation = this.startInitialAdapter();
    this.startPromise = operation;
    try {
      await operation;
    } finally {
      if (this.startPromise === operation) this.startPromise = null;
    }
  }

  getControlMode(): ControlMode {
    return this.controlMode;
  }

  async setControlMode(params: JsonObject): Promise<JsonValue> {
    const nextMode = controlModeFromParams(params);
    this.ensureStarted();
    if (this.switchPromise) throw new Error("a control mode switch is already in progress");
    if (nextMode === this.controlMode) {
      return {
        changed: false,
        controlMode: this.controlMode,
        previousControlMode: this.controlMode,
        modeEpoch: this.modeEpoch,
      };
    }

    const operation = this.performModeSwitch(nextMode);
    this.switchPromise = operation;
    try {
      return await operation;
    } finally {
      if (this.switchPromise === operation) this.switchPromise = null;
    }
  }

  async startThread(params: JsonObject = {}): Promise<JsonValue> {
    this.assertIndependentNavigation("thread/start");
    const adapter = this.activeAdapterForMutation();
    if (!adapter.startThread) throw unsupported("thread/start", this.controlMode);
    return adapter.startThread(params);
  }

  async newSession(params: JsonObject = {}): Promise<JsonValue> {
    this.assertIndependentNavigation("session/new");
    const adapter = this.activeAdapterForMutation();
    if (adapter.newSession) return adapter.newSession(params);
    if (adapter.startThread) return adapter.startThread(params);
    throw unsupported("session/new", this.controlMode);
  }

  async startTurn(params: JsonObject): Promise<JsonValue> {
    const adapter = this.activeAdapterForMutation();
    if (!adapter.startTurn) throw unsupported("turn/start", this.controlMode);
    return adapter.startTurn(params);
  }

  async steerTurn(params: JsonObject): Promise<JsonValue> {
    const adapter = this.activeAdapterForMutation();
    if (!adapter.steerTurn) throw unsupported("turn/steer", this.controlMode);
    return adapter.steerTurn(params);
  }

  async updateThreadSettings(params: JsonObject): Promise<JsonValue> {
    const adapter = this.activeAdapterForMutation();
    if (!adapter.updateThreadSettings) throw unsupported("thread/settings/update", this.controlMode);
    return adapter.updateThreadSettings(params);
  }

  async listSessions(params: JsonObject = {}): Promise<JsonValue> {
    this.assertIndependentNavigation("session/list");
    const adapter = this.activeAdapter();
    if (!adapter.listSessions) throw unsupported("session/list", this.controlMode);
    return adapter.listSessions(params);
  }

  async selectSession(params: JsonObject): Promise<JsonValue> {
    this.assertIndependentNavigation("session/select");
    const adapter = this.activeAdapterForMutation();
    if (!adapter.selectSession) throw unsupported("session/select", this.controlMode);
    return adapter.selectSession(params);
  }

  async interruptTurn(params: JsonObject): Promise<JsonValue> {
    const adapter = this.activeAdapter();
    if (!adapter.interruptTurn) throw unsupported("turn/interrupt", this.controlMode);
    return adapter.interruptTurn(params);
  }

  async sendInput(text: string, params: JsonObject = {}): Promise<JsonValue> {
    return this.activeAdapterForMutation().sendInput(text, params);
  }

  async cancel(taskId?: string, params: JsonObject = {}): Promise<JsonValue> {
    return this.activeAdapter().cancel(taskId, params);
  }

  async respondApproval(
    requestId: JsonRpcId,
    decision: "allow" | "deny" | "cancel",
    reason?: string,
    response?: JsonValue,
  ): Promise<JsonValue> {
    return this.activeAdapter().respondApproval(requestId, decision, reason, response);
  }

  async denyPending(reason?: string): Promise<void> {
    await this.activeAdapter().denyPending?.(reason);
  }

  async snapshot(): Promise<SessionSnapshot> {
    const binding = this.activeBinding();
    const snapshot = await binding.adapter.snapshot();
    return this.decorateSnapshot(snapshot, binding);
  }

  onEvent(listener: (event: AgentEvent) => void): Disposable {
    this.listeners.add(listener);
    return { dispose: () => this.listeners.delete(listener) };
  }

  async dispose(): Promise<void> {
    if (this.disposed) return;
    this.disposed = true;

    const starting = this.startPromise;
    const switching = this.switchPromise;
    await starting?.catch(() => undefined);
    await switching?.catch(() => undefined);

    const binding = this.binding;
    this.binding = null;
    this.started = false;
    if (!binding) return;
    binding.committed = false;
    binding.subscription.dispose();
    await binding.adapter.dispose();
  }

  private async startInitialAdapter(): Promise<void> {
    const binding = await this.createBinding(this.controlMode, this.modeEpoch);
    try {
      await binding.adapter.start();
      if (this.disposed) throw new Error("switchable adapter was disposed while starting");
      binding.committed = true;
      this.binding = binding;
      this.started = true;
      this.flushBufferedEvents(binding);
    } catch (error) {
      await this.releaseBinding(binding);
      throw error;
    }
  }

  private async performModeSwitch(nextMode: ControlMode): Promise<JsonValue> {
    const previousBinding = this.activeBinding();
    const previousMode = this.controlMode;
    this.assertSnapshotIdle(await previousBinding.adapter.snapshot());

    const nextEpoch = this.modeEpoch + 1;
    const candidate = await this.createBinding(nextMode, nextEpoch);
    if (candidate.adapter === previousBinding.adapter) {
      candidate.subscription.dispose();
      throw new Error("adapter factory must return a distinct adapter when switching control modes");
    }
    try {
      await candidate.adapter.start();
      if (this.disposed) throw new Error("switchable adapter was disposed while switching modes");

      // VS Code can start a turn independently while the candidate boots.
      // Recheck immediately before the synchronous commit point.
      this.assertSnapshotIdle(await previousBinding.adapter.snapshot());
      const candidateSnapshot = await candidate.adapter.snapshot();
      // The candidate snapshot is an await point, so make the old adapter's
      // liveness check the final operation before committing synchronously.
      this.assertSnapshotIdle(await previousBinding.adapter.snapshot());

      candidate.committed = true;
      this.binding = candidate;
      this.controlMode = nextMode;
      this.modeEpoch = nextEpoch;
      previousBinding.committed = false;

      const result: JsonObject = {
        changed: true,
        controlMode: nextMode,
        previousControlMode: previousMode,
        modeEpoch: nextEpoch,
      };
      this.emit({ type: "control.mode.changed", payload: result });
      this.flushBufferedEvents(candidate);
      const snapshot = this.decorateSnapshot(candidateSnapshot, candidate);
      this.emit({
        type: "session.snapshot",
        threadId: snapshot.threadId ?? undefined,
        turnId: snapshot.turnId ?? undefined,
        payload: asJsonObject(snapshot),
        status: snapshot.status,
      });

      previousBinding.subscription.dispose();
      await previousBinding.adapter.dispose().catch((error) => {
        this.options.logger?.warn?.("Unable to dispose the previous control mode adapter", error);
      });
      await Promise.resolve(this.options.onModeChanged?.(nextMode, previousMode)).catch((error) => {
        this.options.logger?.warn?.("Unable to persist the committed control mode", error);
      });
      return result;
    } catch (error) {
      if (this.binding !== candidate) await this.releaseBinding(candidate);
      throw error;
    }
  }

  private async createBinding(mode: ControlMode, generation: number): Promise<AdapterBinding> {
    const adapter = await this.options.createAdapter(mode);
    if (!adapter) throw new Error(`adapter factory returned no adapter for ${mode} mode`);
    const binding: AdapterBinding = {
      adapter,
      mode,
      generation,
      committed: false,
      bufferedEvents: [],
      subscription: { dispose: () => undefined },
    };
    binding.subscription = adapter.onEvent((event) => this.receiveAdapterEvent(binding, event));
    return binding;
  }

  private receiveAdapterEvent(binding: AdapterBinding, event: AgentEvent): void {
    if (!binding.committed) {
      binding.bufferedEvents.push(event);
      return;
    }
    if (this.binding !== binding || binding.generation !== this.modeEpoch) return;
    this.emit(this.decorateEvent(event, binding));
  }

  private flushBufferedEvents(binding: AdapterBinding): void {
    const events = binding.bufferedEvents.splice(0);
    for (const event of events) {
      if (this.binding !== binding || binding.generation !== this.modeEpoch) return;
      this.emit(this.decorateEvent(event, binding));
    }
  }

  private emit(event: AgentEvent): void {
    for (const listener of this.listeners) {
      try {
        listener(event);
      } catch (error) {
        this.options.logger?.warn?.("Switchable adapter event listener failed", error);
      }
    }
  }

  private activeBinding(): AdapterBinding {
    this.ensureStarted();
    if (!this.binding) throw new Error("switchable adapter has no active adapter");
    return this.binding;
  }

  private activeAdapter(): AgentAdapter {
    return this.activeBinding().adapter;
  }

  private activeAdapterForMutation(): AgentAdapter {
    if (this.switchPromise) throw new Error("control mode is switching; retry after it completes");
    return this.activeAdapter();
  }

  private ensureStarted(): void {
    if (this.disposed) throw new Error("switchable adapter has been disposed");
    if (!this.started || !this.binding) throw new Error("switchable adapter is not started");
  }

  private assertIndependentNavigation(operation: string): void {
    if (this.controlMode === "sync") {
      throw new Error(`${operation} is unavailable in sync mode; conversation navigation follows VS Code`);
    }
  }

  private assertSnapshotIdle(snapshot: SessionSnapshot): void {
    const pendingApprovalCount = snapshot.pendingApprovals.length;
    const pendingRequestCount = snapshot.pendingRequests?.length ?? 0;
    const state = normalizeStatus(snapshot.state);
    const turnStatus = normalizeStatus(snapshot.status?.turnStatus ?? snapshot.turnStatus ?? "");
    const activeFlags = snapshot.status?.activeFlags ?? snapshot.activeFlags ?? [];
    const hasActiveState = ACTIVE_STATUSES.has(state) || ACTIVE_STATUSES.has(turnStatus) || activeFlags.length > 0;
    if (snapshot.turnId || hasActiveState || pendingApprovalCount > 0 || pendingRequestCount > 0) {
      throw new Error("cannot switch control mode while a turn or request is active");
    }
  }

  private decorateSnapshot(snapshot: SessionSnapshot, binding: AdapterBinding): SessionSnapshot {
    return {
      ...snapshot,
      metadata: {
        ...(snapshot.metadata ?? {}),
        mode: binding.mode,
        controlMode: binding.mode,
        modeEpoch: binding.generation,
        capabilities: this.capabilities(binding),
      },
    };
  }

  private decorateEvent(event: AgentEvent, binding: AdapterBinding): AgentEvent {
    if (event.type !== "session.snapshot") return event;
    return {
      ...event,
      payload: {
        ...event.payload,
        metadata: {
          ...asJsonObject(event.payload.metadata),
          mode: binding.mode,
          controlMode: binding.mode,
          modeEpoch: binding.generation,
          capabilities: this.capabilities(binding),
        },
      },
    };
  }

  private capabilities(binding: AdapterBinding): ModeCapabilities {
    const independent = binding.mode === "async";
    return {
      followsVscodeRoute: !independent,
      sessionList: independent && typeof binding.adapter.listSessions === "function",
      sessionSelect: independent && typeof binding.adapter.selectSession === "function",
      sessionCreate: independent && (typeof binding.adapter.newSession === "function"
        || typeof binding.adapter.startThread === "function"),
      threadSettings: typeof binding.adapter.updateThreadSettings === "function",
    };
  }

  private async releaseBinding(binding: AdapterBinding): Promise<void> {
    binding.committed = false;
    binding.subscription.dispose();
    await binding.adapter.dispose().catch(() => undefined);
  }
}

function controlModeFromParams(params: JsonObject): ControlMode {
  return validateControlMode(params.mode ?? params.controlMode);
}

function validateControlMode(value: unknown): ControlMode {
  if (value === "sync" || value === "async") return value;
  throw new Error("control mode must be sync or async");
}

function unsupported(operation: string, mode: ControlMode): Error {
  return new Error(`${operation} is not supported by the ${mode} adapter`);
}

const ACTIVE_STATUSES = new Set(["active", "inprogress", "running", "starting", "thinking", "editing", "working"]);

function normalizeStatus(value: string): string {
  return value.trim().replace(/[\s_-]+/g, "").toLowerCase();
}
