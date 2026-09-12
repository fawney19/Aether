import { accessSync, constants, Dirent, readdirSync, statSync } from "node:fs";
import { homedir } from "node:os";
import { delimiter, isAbsolute, join, sep } from "node:path";

export interface CodexPathOptions {
  /** Environment used for PATH lookup. Defaults to the extension host environment. */
  env?: NodeJS.ProcessEnv;
  /** Home directory used when looking for bundled installations. */
  homeDir?: string;
  /** Platform override for deterministic tests. */
  platform?: NodeJS.Platform;
}

/**
 * Resolve the executable used by the VS Code bridge.
 *
 * VS Code launched from Finder/Dock often receives a smaller PATH than a shell.
 * The default `codex` command therefore gets a few explicit installation
 * fallbacks, while a user-supplied command remains authoritative.
 */
export function resolveCodexCommand(configuredCommand = "codex", options: CodexPathOptions = {}): string {
  const command = configuredCommand.trim() || "codex";
  const env = options.env ?? process.env;
  const platform = options.platform ?? process.platform;
  const home = options.homeDir ?? homedir();

  if (hasPathComponent(command, platform)) {
    const resolved = executablePath(command, platform);
    if (resolved) return resolved;
    throw missingCodexError(command, platform);
  }

  const fromPath = findOnPath(command, env.PATH, platform, env.PATHEXT);
  if (fromPath) return fromPath;

  // Only the default command gets installation-specific fallbacks. A custom
  // bare command should fail loudly instead of silently running another binary.
  if (!isDefaultCommand(command, platform)) throw missingCodexError(command, platform);

  for (const candidate of bundledCandidates(home, platform)) {
    const resolved = executablePath(candidate, platform);
    if (resolved) return resolved;
  }

  throw missingCodexError(command, platform);
}

export function missingCodexError(command: string, platform: NodeJS.Platform = process.platform): Error {
  const examples = platform === "darwin"
    ? ' Set "codexRemoteCollab.codexCommand" to the full path, for example "/Applications/ChatGPT.app/Contents/Resources/codex".'
    : ' Set "codexRemoteCollab.codexCommand" to the full path of the Codex executable.';
  return new Error(`Codex executable "${command}" was not found.${examples}`);
}

function isDefaultCommand(command: string, platform: NodeJS.Platform): boolean {
  return platform === "win32" ? command.toLowerCase() === "codex" || command.toLowerCase() === "codex.exe" : command === "codex";
}

function hasPathComponent(command: string, platform: NodeJS.Platform): boolean {
  return isAbsolute(command) || command.includes(sep) || (platform === "win32" && command.includes("\\"));
}

function executablePath(candidate: string, platform: NodeJS.Platform): string | undefined {
  try {
    const info = statSync(candidate);
    if (!info.isFile()) return undefined;
    // X_OK is meaningful on POSIX; Windows still benefits from the file check.
    if (platform !== "win32") accessSync(candidate, constants.X_OK);
    return candidate;
  } catch {
    return undefined;
  }
}

function findOnPath(command: string, pathValue: string | undefined, platform: NodeJS.Platform, pathextValue?: string): string | undefined {
  if (!pathValue) return undefined;
  const extensions = platform === "win32" ? windowsExtensions(command, pathextValue) : [""];
  for (const directory of pathValue.split(delimiter)) {
    if (!directory) continue;
    for (const extension of extensions) {
      const candidate = join(directory, `${command}${extension}`);
      const resolved = executablePath(candidate, platform);
      if (resolved) return resolved;
    }
  }
  return undefined;
}

function windowsExtensions(command: string, pathextValue: string | undefined): string[] {
  if (/[.][^./\\]+$/.test(command)) return [""];
  const extensions = (pathextValue ?? ".COM;.EXE;.BAT;.CMD")
    .split(";")
    .map((value) => value.trim())
    .filter(Boolean);
  return ["", ...extensions];
}

function bundledCandidates(home: string, platform: NodeJS.Platform): string[] {
  if (platform !== "darwin") return [];

  const candidates = [
    join(home, "Applications", "ChatGPT.app", "Contents", "Resources", "codex"),
    "/Applications/ChatGPT.app/Contents/Resources/codex",
    join(home, ".local", "bin", "codex"),
    join(home, ".npm-global", "bin", "codex"),
  ];

  for (const extensionsRoot of [
    join(home, ".vscode", "extensions"),
    join(home, ".vscode-insiders", "extensions"),
  ]) {
    candidates.push(...officialExtensionCandidates(extensionsRoot));
  }
  return candidates;
}

function officialExtensionCandidates(extensionsRoot: string): string[] {
  let entries: Dirent<string>[];
  try {
    entries = readdirSync(extensionsRoot, { withFileTypes: true, encoding: "utf8" });
  } catch {
    return [];
  }

  const matches = entries
    .filter((entry) => entry.isDirectory() && entry.name.startsWith("openai.chatgpt-"))
    .map((entry) => {
      const directory = join(extensionsRoot, entry.name);
      let modified = 0;
      try {
        modified = statSync(directory).mtimeMs;
      } catch {
        // Keep an unreadable entry at the end of the deterministic sort.
      }
      return { directory, modified };
    })
    .sort((left, right) => right.modified - left.modified || right.directory.localeCompare(left.directory));

  const candidates: string[] = [];
  for (const match of matches) {
    let architectures: Dirent<string>[];
    try {
      architectures = readdirSync(join(match.directory, "bin"), { withFileTypes: true, encoding: "utf8" });
    } catch {
      continue;
    }
    for (const architecture of architectures) {
      if (architecture.isDirectory()) candidates.push(join(match.directory, "bin", architecture.name, "codex"));
    }
  }
  return candidates;
}
