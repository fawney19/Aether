import * as http from "node:http";
import * as path from "node:path";

import { Logger } from "./protocol";

interface BundledRelay {
  start(): Promise<{ host: string; port: number }>;
  stop(): Promise<void>;
}

interface BundledRelayModule {
  CodexRelay: new (options: Record<string, unknown>) => BundledRelay;
}

export interface LocalRelayTarget {
  host: string;
  port: number;
  healthUrl: string;
  webUrl: string;
}

export interface LocalRelayControllerOptions {
  extensionPath: string;
  logger?: Logger;
  probeTimeoutMs?: number;
  relayModulePath?: string;
  loadRelayModule?: (modulePath: string) => BundledRelayModule;
  probeRelayHealth?: (url: string, timeoutMs?: number) => Promise<boolean>;
}

/**
 * Owns the loopback relay bundled with the companion extension. Remote and
 * TLS relay URLs deliberately stay outside this controller.
 */
export class LocalRelayController {
  private readonly options: LocalRelayControllerOptions;
  private relay?: BundledRelay;
  private target?: LocalRelayTarget;
  private starting?: Promise<boolean>;
  private generation = 0;

  constructor(options: LocalRelayControllerOptions) {
    this.options = options;
  }

  async ensureRunning(relayUrl: string): Promise<boolean> {
    const target = localRelayTarget(relayUrl);
    if (!target) return false;
    if ((this.relay || this.starting) && this.target?.healthUrl !== target.healthUrl) await this.stop();
    const generation = this.generation;
    this.target = target;
    const available = await this.probeHealth(target.healthUrl);
    // `stop()` may run while the health request is in flight. Do not let that
    // completed probe resurrect a relay owned by a deactivated extension.
    if (generation !== this.generation) return false;
    if (available) return false;
    if (this.relay) {
      await this.relay.stop().catch(() => undefined);
      this.relay = undefined;
    }
    if (this.starting) return this.starting;
    this.starting = this.startBundledRelay(target).finally(() => {
      this.starting = undefined;
    });
    return this.starting;
  }

  getWebUrl(relayUrl: string): string | undefined {
    return localRelayTarget(relayUrl)?.webUrl;
  }

  async stop(): Promise<void> {
    this.generation += 1;
    const starting = this.starting;
    if (starting) await starting.catch(() => undefined);
    const relay = this.relay;
    this.relay = undefined;
    this.target = undefined;
    if (relay) await relay.stop();
  }

  private async startBundledRelay(target: LocalRelayTarget): Promise<boolean> {
    const modulePath = this.options.relayModulePath
      ?? path.join(this.options.extensionPath, "dist", "local-relay", "server.js");
    let relay: BundledRelay;
    try {
      const load = this.options.loadRelayModule ?? ((value: string) => require(value) as BundledRelayModule);
      const module = load(modulePath);
      if (typeof module?.CodexRelay !== "function") throw new Error("bundled relay module is invalid");
      relay = new module.CodexRelay({
        host: target.host,
        port: target.port,
        mode: "host",
        spawnCodex: false,
        authRequired: false,
      });
      await relay.start();
    } catch (error) {
      // Another VS Code window can win the listen race after our health
      // probe. Treat that as success only when the expected relay responds.
      if (await this.probeHealth(target.healthUrl)) {
        this.options.logger?.info?.(`Using existing local relay at ${target.webUrl}`);
        return false;
      }
      throw error;
    }
    this.relay = relay;
    this.options.logger?.info?.(`Started bundled local relay at ${target.webUrl}`);
    return true;
  }

  private probeHealth(url: string): Promise<boolean> {
    const probe = this.options.probeRelayHealth ?? relayHealthAvailable;
    return probe(url, this.options.probeTimeoutMs);
  }
}

export function localRelayTarget(relayUrl: string): LocalRelayTarget | undefined {
  let url: URL;
  try {
    url = new URL(relayUrl);
  } catch {
    return undefined;
  }
  if (url.protocol !== "ws:" || !isLoopbackHostname(url.hostname)) return undefined;
  const port = Number(url.port || 80);
  if (!Number.isInteger(port) || port < 1 || port > 65_535) return undefined;
  const hostname = normalizeLoopbackHostname(url.hostname);
  const authorityHost = hostname.includes(":") ? `[${hostname}]` : hostname;
  return {
    host: hostname,
    port,
    healthUrl: `http://${authorityHost}:${port}/api/health`,
    webUrl: `http://${authorityHost}:${port}/`,
  };
}

function isLoopbackHostname(hostname: string): boolean {
  const normalized = hostname.toLowerCase().replace(/^\[|\]$/g, "");
  return normalized === "localhost" || normalized === "127.0.0.1" || normalized === "::1";
}

function normalizeLoopbackHostname(hostname: string): string {
  const normalized = hostname.toLowerCase().replace(/^\[|\]$/g, "");
  return normalized === "localhost" ? "127.0.0.1" : normalized;
}

export function relayHealthAvailable(url: string, timeoutMs = 700): Promise<boolean> {
  return new Promise((resolve) => {
    let settled = false;
    let timer: NodeJS.Timeout | undefined;
    const finish = (available: boolean): void => {
      if (settled) return;
      settled = true;
      if (timer) clearTimeout(timer);
      resolve(available);
    };
    const request = http.get(url, (response) => {
      if (response.statusCode !== 200) {
        response.resume();
        finish(false);
        return;
      }
      let body = "";
      response.setEncoding("utf8");
      response.on("data", (chunk) => {
        if (body.length <= 16_384) body += chunk;
      });
      response.on("end", () => {
        try {
          const payload = JSON.parse(body) as { ok?: unknown };
          finish(payload.ok === true);
        } catch {
          finish(false);
        }
      });
      response.on("aborted", () => finish(false));
      response.on("error", () => finish(false));
      response.on("close", () => {
        if (!response.complete) finish(false);
      });
    });
    request.setTimeout(timeoutMs, () => {
      request.destroy();
      finish(false);
    });
    request.on("error", () => finish(false));
    timer = setTimeout(() => {
      request.destroy();
      finish(false);
    }, timeoutMs);
  });
}
