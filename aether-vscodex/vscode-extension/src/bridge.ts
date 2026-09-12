import { CodexAgentAdapter, CodexAgentAdapterOptions } from "./codexAgentAdapter";
import { RelayClient, RelayClientOptions } from "./relayClient";
import { RelayHost, RelayHostOptions } from "./relayHost";
import { AgentAdapter, Logger, RelayTransport } from "./protocol";

export interface CodexRemoteBridgeOptions {
  /** Use a supplied adapter/transport when embedding or testing. */
  adapter?: AgentAdapter;
  relay?: RelayTransport;
  adapterOptions?: CodexAgentAdapterOptions;
  relayOptions?: RelayClientOptions;
  sessionId?: string;
  capabilities?: Iterable<string>;
  logger?: Logger;
}
export interface CodexRemoteBridge {
  adapter: AgentAdapter;
  relay: RelayTransport;
  host: RelayHost;
  start(): Promise<void>;
  stop(): Promise<void>;
}

/** Construct the default outbound VS Code bridge in one call. */
export function createBridge(options: CodexRemoteBridgeOptions): CodexRemoteBridge {
  const adapter = options.adapter ?? new CodexAgentAdapter(options.adapterOptions);
  const relay = options.relay ?? (() => {
    if (!options.relayOptions) throw new Error("relayOptions are required when no relay transport is supplied");
    return new RelayClient(options.relayOptions);
  })();
  const hostOptions: RelayHostOptions = {
    adapter,
    relay,
    ...(options.sessionId ? { sessionId: options.sessionId } : {}),
    ...(options.capabilities ? { capabilities: options.capabilities } : {}),
    ...(options.logger ? { logger: options.logger } : {}),
  };
  const host = new RelayHost(hostOptions);
  return {
    adapter,
    relay,
    host,
    start: () => host.start(),
    stop: () => host.stop(),
  };
}
