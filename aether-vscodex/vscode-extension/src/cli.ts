import { CodexAgentAdapter } from "./codexAgentAdapter";
import { RelayHost } from "./relayHost";
import { StdioRelayTransport } from "./relayClient";

/** Standalone bridge: relay frames in stdin, relay frames out on stdout. */
async function main(): Promise<void> {
  const logger = {
    debug: (message: string, ...args: unknown[]) => console.error(`[debug] ${message}`, ...args),
    info: (message: string, ...args: unknown[]) => console.error(`[info] ${message}`, ...args),
    warn: (message: string, ...args: unknown[]) => console.error(`[warn] ${message}`, ...args),
    error: (message: string, ...args: unknown[]) => console.error(`[error] ${message}`, ...args),
  };
  const command = process.env.CODEX_COMMAND || "codex";
  const args = process.env.CODEX_APP_SERVER_ARGS ? JSON.parse(process.env.CODEX_APP_SERVER_ARGS) as string[] : ["app-server", "--stdio"];
  const adapter = new CodexAgentAdapter({ command, args, defaultCwd: process.env.CODEX_WORKSPACE, logger });
  const relay = new StdioRelayTransport(process.stdin, process.stdout, logger);
  const host = new RelayHost({ adapter, relay, sendHandshake: true, logger });
  const shutdown = async (): Promise<void> => {
    await host.stop();
    process.exit(0);
  };
  process.once("SIGINT", () => void shutdown());
  process.once("SIGTERM", () => void shutdown());
  await host.start();
}

void main().catch((error) => {
  console.error(error instanceof Error ? error.stack ?? error.message : String(error));
  process.exitCode = 1;
});
