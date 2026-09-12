# Codex Remote Collaboration VS Code Bridge

This extension connects local and Aether relay channels to one switchable Codex
control host. **Synchronous mode** follows the conversation currently shown by
the official Codex VS Code extension through its private IPC protocol and does
not spawn a `codex` process. **Asynchronous mode** starts an independent
app-server and lets the Web UI list, resume, create, and select conversations.

The attached conversation remains visible and usable in the official Codex
panel. Remote operators can observe its output, submit a new turn or steer the
active turn, interrupt it, and answer supported approval/input requests.

The mode can be changed from the Web UI without reconnecting either relay.
Synchronous mode makes the official panel the only conversation-navigation
owner; asynchronous mode restores the browser history and new-conversation
actions. A running turn or pending request blocks mode changes.

## Requirements

- The official `openai.chatgpt` VS Code extension is installed and signed in.
- The target Codex conversation is open and owned by that extension.
- The bridge and official extension run as the same OS user. The default Unix
  socket is `$CODEX_HOME/ipc/ipc.sock`, normally `~/.codex/ipc/ipc.sock`.
- For a loopback `ws://` URL, the extension starts and owns its bundled relay
  automatically. Remote and `wss://` relay URLs remain externally hosted.

The IPC follower protocol is private and versioned, not a public OpenAI API.
An official extension update can require a compatible bridge update. Strict
stream-version checks are enabled by default so an unknown protocol fails
closed instead of being interpreted optimistically.

## Build and install

```sh
npm install
npm run check
npm run build
npx --yes @vscode/vsce package
code --install-extension codex-remote-collab-0.4.0.vsix --force
```

Run **Developer: Reload Window** after installing or replacing the VSIX.

## Configure control modes

For the local default, no separate relay command is required. The extension
starts the bundled relay on the host and port from `codexRemoteCollab.localRelayUrl`.
To run the development relay manually, disable
`codexRemoteCollab.autoStartLocalRelay` and use:

```sh
HOST=127.0.0.1 PORT=8787 CODEX_REMOTE_MODE=host npm start
```

To opt into authentication later, set `CODEX_REMOTE_AUTH=required` and the three
token variables before starting the relay.

Set the extension configuration:

```json
{
  "codexRemoteCollab.localRelayUrl": "ws://127.0.0.1:8787/v1/connect",
  "codexRemoteCollab.controlMode": "sync",
  "codexRemoteCollab.autoDiscoverThread": true,
  "codexRemoteCollab.autoStart": true
}
```

Then:

1. Open the target conversation in the official Codex panel.
2. Reload VS Code once after installing the companion extension. The local relay and bridge start automatically; no token is needed for loopback. The status item opens the Web console and is not a connect/disconnect toggle.
3. Open the relay web console; it connects automatically on localhost. The web UI uses a
   Codex-style conversation stream with a bottom composer; Enter sends and Shift+Enter
   inserts a newline. There is no separate connect/disconnect step for the local relay.

If the browser says that it is waiting for the VS Code host or the recent-session list is
empty, verify that `codexRemoteCollab.localRelayUrl` uses the same port as the relay and run
**Developer: Reload Window**. Keep `codexRemoteCollab.threadId` empty unless a specific
conversation must be pinned; an old closed ID can prevent startup until it is cleared.

When authentication is enabled, run **Codex Remote: Set Relay Token** with the
host token. It is stored in `vscode.SecretStorage`, not in settings; the browser
uses the operator or viewer token separately.

With no configured thread ID, the bridge ranks recent VS Code rollout metadata
and shows only candidates verified by live IPC owner discovery and a matching
follower snapshot. Explicit Codex Desktop tasks, closed, stale, and other
non-attachable history entries are omitted.
In synchronous mode, switching the conversation in the official Codex panel
also switches the Web projection after the new owner snapshot is ready. The
Web UI cannot list, select, or create conversations in this mode. Switch to
asynchronous mode when the browser should own conversation navigation.
To avoid ambiguity when several Codex windows are open, run **Codex Remote: Set Existing Thread ID**.
An empty value restores automatic discovery.

Useful commands:

- **Codex Remote: Start Bridge** / **Stop Bridge**
- **Codex Remote: Set Existing Thread ID**
- **Codex Remote: Set Relay Token**
- **Codex Remote: Pair with Aether**
- **Codex Remote: Configure Aether Cloud Relay**
- **Codex Remote: Send Input**
- **Codex Remote: Show Snapshot**

## Settings

| Setting | Default | Meaning |
| --- | --- | --- |
| `codexRemoteCollab.controlMode` | `sync` | `sync` follows VS Code; `async` owns an independent app-server. |
| `codexRemoteCollab.localRelayUrl` | `ws://127.0.0.1:8787/v1/connect` | Bundled loopback relay used by the local Web control. |
| `codexRemoteCollab.aetherUrl` | empty | Aether origin remembered by the pairing command. |
| `codexRemoteCollab.cloudRelayUrl` | empty | Aether WebSocket relay URL populated by pairing. |
| `codexRemoteCollab.threadId` | empty | Exact existing conversation ID; empty enables discovery. |
| `codexRemoteCollab.autoDiscoverThread` | `true` | Discover and owner-check a local VS Code session. |
| `codexRemoteCollab.followVscodeSession` | `true` | Legacy compatibility setting; synchronous mode always follows VS Code. |
| `codexRemoteCollab.ipcSocketPath` | empty | Override the local IPC socket path. |
| `codexRemoteCollab.hostId` | `local` | Owner-discovery host identifier. |
| `codexRemoteCollab.ipcStrictVersions` | `true` | Reject unsupported stream protocol versions. |
| `codexRemoteCollab.approvalTimeoutMs` | `300000` | Deny an unanswered request locally after this delay. |
| `codexRemoteCollab.allowHighRiskApprovals` | `false` | Permit remote high-risk approvals when explicitly enabled. |

`codexRemoteCollab.codexCommand`, `codexArgs`, and `defaultCwd` apply only to
asynchronous mode. The deprecated `mode=attach/spawn` values map to
`controlMode=sync/async` when no explicit control mode exists.

## Pair with Aether

The local relay stays enabled after cloud pairing. In Aether, open **Codex remote
control** and generate a one-time code. Then run **Codex Remote: Pair with Aether**
from the VS Code Command Palette, enter the Aether server URL and the code, and
the bridge will connect to both relays. The long-lived device credential is stored
only in VS Code SecretStorage. Revoke a lost or retired device from the Aether page.

## Relay behavior

The bridge sends a `hello` and, when a relay token is configured, a separate
bearer-auth frame over an outbound WebSocket. It publishes normalized events including:

- `connection.opened` / `connection.closed`
- `session.snapshot`
- `output.snapshot` / `output.chunk`
- `task.started` / `task.finished` / `task.cancelled`
- `approval.requested` / `approval.resolved` / `approval.expired`
- `input.requested` / `input.resolved` / `input.expired`

Remote commands are mapped to the existing conversation owner:

- `control/mode/set` atomically switches between `sync` and `async`.
- `session/list`, `session/select`, and `session/new` are available only in
  asynchronous mode and map to `thread/list`, `thread/resume`, and `thread/start`.
- `turn/start` starts a turn in the attached thread.
- `turn/steer` adds input to the active turn.
- `turn/interrupt` interrupts the expected active turn.
- `approval.respond`, `input.respond`, and `server.request.respond` preserve the
  original request ID and use method-specific follower responses.
- `thread/start` is deliberately rejected in synchronous mode because VS Code
  owns conversation navigation there.

The browser never connects directly to the IPC socket. Relay and host both
enforce role/capability checks; high-risk command approval remains disabled
unless the local VS Code setting opts in.

## Supported follower requests

- `item/commandExecution/requestApproval`
- `item/fileChange/requestApproval`
- `item/permissions/requestApproval`
- `item/tool/requestUserInput`
- `mcpServer/elicitation/request`
- legacy `applyPatchApproval` and `execCommandApproval`

Unanswered requests expire with a local deny. JSON-RPC numeric and string IDs
remain distinct, and a response can be submitted only once.

## Legacy mode migration

The old setting remains accepted:

```json
{
  "codexRemoteCollab.mode": "spawn",
  "codexRemoteCollab.codexCommand": "/absolute/path/to/codex",
  "codexRemoteCollab.codexArgs": ["app-server", "--stdio"]
}
```

It maps to `controlMode=async`. Prefer the new setting directly. A
`spawn codex ENOENT` error belongs only to asynchronous mode; it is not a
synchronous-mode prerequisite or a PATH problem that needs fixing for
existing-session control.

The standalone `npm run start:stdio` entry point and `createBridge()` helper
also retain the legacy app-server adapter for compatibility.

## Embedding the attach adapter

The reusable exports are in `src/index.ts`:

```ts
import {
  CodexIpcAgentAdapter,
  RelayClient,
  RelayHost,
} from "codex-remote-collab";

const adapter = new CodexIpcAgentAdapter({
  threadId: process.env.CODEX_THREAD_ID,
  autoDiscoverThread: true,
});
const relay = new RelayClient({
  url: "wss://relay.example.test/v1/connect",
  accessToken: process.env.CODEX_REMOTE_HOST_TOKEN,
});
const host = new RelayHost({ adapter, relay });
await host.start();
```

`CodexIpcClient` is exported separately for protocol fixtures and diagnostics.
Use `followConversation()` before follower mutations, and always target the
owner returned by `findThreadOwner()`.

## Troubleshooting

- **No existing session found:** open the target official Codex conversation,
  keep that VS Code window running, then retry or set its exact thread ID.
- **Owner not found:** the rollout exists on disk but no live official client
  currently owns it. Reopen the conversation in the Codex panel.
- **IPC version mismatch:** update this bridge for the installed official
  extension. Disabling strict versions is diagnostic only.
- **Relay stays at waiting for host:** confirm host mode, relay URL, and that no
  second host is already connected. If authentication is enabled, also check the
  host token.
- **Old `spawn codex ENOENT` message:** install version `0.4.0`, reload VS Code,
  and verify `codexRemoteCollab.controlMode` is `sync` unless independent
  conversations are intended.
