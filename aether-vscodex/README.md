# aether-vscodex

这个项目让浏览器从本机 URL 或 Aether 云端查看、输入并处理 Codex 会话，提供两种
可随时切换的控制模式。默认的**同步模式**通过官方扩展使用的本机 IPC socket，严格
跟随 VS Code Codex 面板当前会话，不启动另一个 `codex` 进程；**异步模式**由伴随扩展
启动独立 app-server，网页可以自行列出、恢复、新建和切换会话。

同一个伴随扩展可同时连接两个互不替代的通道：本机 loopback 控制台和部署在
Aether 中的云端控制台。本机通道默认免密码且只能从本机访问；云端通道使用
Aether 登录鉴权、一次性浏览器票据和独立设备凭据；父页面不会通过协议把 Aether JWT
传给 iframe 或 Node sidecar。iframe 是随 Aether 一起发布的同源受信代码，不应被视为
隔离不受信内容的安全边界。

`vscode-extension/codex-remote-collab-0.4.0.vsix` 安装到 VS Code 后，会作为官方
`openai.chatgpt` Codex 扩展的伴随扩展，并自动托管只监听本机的 relay。开发时仍可
单独运行 `relay/server.js`。不要卸载或替换官方 Codex 扩展。

## 工作方式

```text
官方 VS Code Codex 会话
        │  本机私有 IPC（只在 VS Code 所在机器上）
        ▼
                                ┌── 本机 relay ── http://127.0.0.1:8787
VS Code aether-vscodex 扩展 ────┤
                                └── Aether gateway ── 用户/设备隔离的云端 relay
```

控制模式与传输通道是两个独立维度：切换同步/异步不会重连本地或云端 relay。本机和
Aether 控制页连接到同一台 VS Code 主机时，会看到同一个当前模式。

| 控制模式 | 会话所有者 | 网页会话导航 |
| --- | --- | --- |
| 同步 | 官方 VS Code Codex 面板 | 禁止网页自行切换；自动跟随 VS Code |
| 异步 | 扩展启动的独立 app-server | 可列出、恢复、新建和切换会话 |

浏览器的 `operator` 可以发送任务、继续/中断当前 turn，并处理 Codex 的审批、
用户输入和 MCP elicitation；`viewer` 只能查看事件和输出。远程浏览器不接触
VS Code 的 SecretStorage，也不直接连接 IPC socket。

## 前端结构

Aether 页面使用仓库既有的 Vue 3、TypeScript、Vite 和 i18n。独立控制台也提供
Vue/Vite 源码入口，但当前高保真的会话渲染与协议状态机作为兼容运行时保留，构建到
`public/` 后同时供本机 URL 和 Aether 同源 iframe 使用。这样不需要一次性重写并丢失
命令展开、滚动锚点、思考状态、Markdown、子代理、模型和权限菜单等已有行为。

界面支持 `zh-CN` 与 `en-US`。Aether 的语言和深浅色主题会通过经过来源校验的
`postMessage` 同步给 iframe；VS Code 命令与设置说明使用 `package.nls` 本地化。

## Aether 云端部署

云端模式由 Aether gateway 和独立 Node sidecar 组成。sidecar 只在 Compose 内网暴露
8788，公网的 HTTP、配对交换和 WebSocket 都经 Aether gateway：

```text
GET    /api/users/me/vscodex/devices
POST   /api/users/me/vscodex/pairings
DELETE /api/users/me/vscodex/devices/:device_id
POST   /api/users/me/vscodex/ws-tickets
POST   /api/vscodex/pair
WS     /api/vscodex/ws
```

生成至少 32 字节的内部令牌，并按 Aether 的公开 HTTPS 地址设置变量：

```sh
export AETHER_VSCODEX_INTERNAL_TOKEN="$(openssl rand -base64 32)"
export AETHER_VSCODEX_PUBLIC_WS_URL="wss://aether.example.com/api/vscodex/ws"
export AETHER_VSCODEX_ALLOWED_ORIGINS="https://aether.example.com"

docker compose \
  -f docker-compose.yml \
  -f docker-compose.local.yml \
  -f aether-vscodex/docker-compose.aether.yml \
  up -d --build
```

源码部署必须包含 `docker-compose.local.yml`，以保证 gateway、前端和 sidecar 来自同一份
checkout。使用发布镜像时可以去掉该文件，但 `APP_IMAGE` 必须固定为包含相同
`aether-vscodex` 协议版本的 Aether 镜像，不能把当前 sidecar 与旧的 `latest` gateway 混用。

首次使用源码 Compose 前先构建控制台；正式 Aether 发布流程与 Dockerfile 已自动执行
同一步骤：

```sh
npm --prefix aether-vscodex/web ci
npm --prefix aether-vscodex/web run build
```

第一阶段 sidecar 是有状态单副本：设备凭据的 scrypt 哈希保存在
`vscodex_data`，短期配对码、60 秒一次性浏览器票据和在线房间保存在内存。不要在未引入
共享连接目录前横向扩容 sidecar。

登录 Aether 后打开“远程控制”，生成一次性配对码。然后在 VS Code 命令面板执行
**Codex Remote: Pair with Aether**，填写 Aether 地址和配对码。插件会把设备凭据写入
VS Code SecretStorage，并同时保持本机控制台连接。

## 快速开始

前提：Node.js 20+；官方 `openai.chatgpt` VS Code 扩展已安装并登录；目标会话
已经在 VS Code 的 Codex 面板中打开。VS Code 和 relay 必须以同一个操作系统用户
运行，因为 IPC socket 是本机文件。

1. 安装依赖并构建伴随扩展：

   ```sh
   npm --prefix vscode-extension install
   npm --prefix vscode-extension run build
   ```

   本机 `ws://` 地址会由扩展自动启动 relay；loopback 模式默认不需要 token，且
   `host` 模式不会启动 `codex app-server`。

2. 安装 `vscode-extension/codex-remote-collab-0.4.0.vsix`（或在扩展目录先
   `npm run build` 再用 `npx --yes @vscode/vsce package` 打包），然后在 VS Code
   执行 **Developer: Reload Window**。

3. 在 VS Code 设置中填写：

   ```json
   {
     "codexRemoteCollab.localRelayUrl": "ws://127.0.0.1:8787/v1/connect",
     "codexRemoteCollab.controlMode": "sync",
     "codexRemoteCollab.autoDiscoverThread": true,
     "codexRemoteCollab.autoStart": true
   }
   ```

4. 执行一次 **Developer: Reload Window** 后，扩展会自动找到最近的、仍由官方
   VS Code Codex owner 持有的会话，并把已有输出同步到 relay；如果没有自动启动，
   无需手动启动或断开。右下角状态项只用于显示状态并打开 Web。需要精确指定会话时，执行
   **Codex Remote: Set Existing Thread ID**；留空则恢复自动发现。
   官方 Codex 面板切换会话时，Web 默认会在新会话快照就绪后自动跟随；正在执行或等待
   授权的旧会话会先保持附着，结束后再安全切换。

5. 浏览器打开 `http://127.0.0.1:8787`，页面会自动以本机 operator 身份连接，
   不需要输入密码。

如果页面显示“等待 VS Code 主机连接”，先确认 relay 地址与扩展设置的端口完全一致，
然后在 VS Code 执行一次 **Developer: Reload Window**。同步模式必须在官方 Codex
面板已经打开至少一个会话后才能发现 owner；通常不需要手工填写
`codexRemoteCollab.threadId`，留空会自动选择最近的可用会话。若之前填写过已经关闭的
thread ID，清空该设置后再重载窗口。

### 发布与下载插件

正式发布时不需要用户在本地编译。仓库的 `.github/workflows/release.yml` 在推送
`vX.Y.Z`、`vX.Y.Z-beta.N` 或 `vX.Y.Z-rc.N` 标签时，会在 GitHub Actions 中完成 Web
前端构建、扩展编译和 VSIX 打包，并把
`aether-vscodex-<extension-version>.vsix` 附加到对应的 GitHub Release。用户从 Release
页面下载该 VSIX，在 VS Code 的扩展视图中选择“从 VSIX 安装...”即可；安装后执行一次
**Developer: Reload Window**。

手动运行该 workflow 时，VSIX 会作为 `aether-vscodex-vsix` Actions artifact 提供下载，
但不会创建 GitHub Release。源码目录中的 VSIX 只用于本地开发验证，不是用户发布渠道。

如果命令面板提示 `command 'codexRemoteCollab.start' not found`，通常是旧版
VSIX 激活失败（旧包可能没有包含 `ws` 运行依赖）。请安装当前的
`codex-remote-collab-0.4.0.vsix` 并使用 `--force` 覆盖旧版本，然后执行一次
**Developer: Reload Window**：

```sh
code --install-extension vscode-extension/codex-remote-collab-0.4.0.vsix --force
```

也可以在 **Output → Codex Remote Collaboration** 中确认没有
`Cannot find module 'ws'`；出现该错误时，说明扩展尚未成功激活。

网页现在按官方 Codex Webview 的会话模型展示：历史和实时输出在中间消息流，用户、
助手、reasoning、命令输出分别投影为对应的消息项；助手内容支持安全的 Markdown、
代码块和复制操作，reasoning/命令活动可折叠。底部 composer 使用可编辑富文本区域，
回车发送、Shift+Enter 换行；审批和用户输入会以内嵌 card 出现在会话流中，支持风险
标记、输入控件、授权范围和明确的允许/拒绝动作。附着适配器会额外发送可选的
`messages` 角色投影，旧版 host 没有该字段时网页仍回退到纯文本快照。

页面打开后自动连接并在断线后重连，不再需要手动点击“连接”或“断开”。同步模式下
会话列表、返回历史和新建入口会被禁用，所有输入都发送到 VS Code 当前会话。这里复刻的是从本机已安装
官方 bundle 审计出的布局、状态和交互；官方 bundle 依赖 VS Code 私有 Webview API，
不能安全地直接作为 iframe 嵌入浏览器。

底部的“同步 / 异步”分段控件发送 `control/mode/set`。当前 turn 正在执行或存在待处理
授权、用户输入时，主机拒绝切换；候选适配器启动失败时保留原模式和原会话。切入异步
模式后，页面顶部会恢复会话历史、新建和选择入口；`session/list` 映射到
`thread/list`，选择会话使用 `thread/resume` 并水合完整历史，新建会话使用
`thread/start`。切回同步模式会关闭独立 app-server，并重新以 VS Code 面板为唯一
会话导航来源。

### 认证（可选）

如果以后需要保护 relay，可显式开启认证；本机流程默认不需要这些变量：

```sh
CODEX_REMOTE_AUTH=required \
CODEX_REMOTE_HOST_TOKEN='host-only-secret' \
CODEX_REMOTE_TOKEN='browser-operator-secret' \
CODEX_REMOTE_VIEW_TOKEN='browser-viewer-secret' \
CODEX_REMOTE_MODE=host npm start
```

认证开启后，Host token 填在 VS Code 扩展中，Operator/Viewer token 填在浏览器中。

## `spawn codex ENOENT` 是什么

这个错误只表示某处正在尝试启动**独立**的 `codex app-server`，但 VS Code 图形
进程的 `PATH` 找不到可执行文件。对于本项目默认的同步模式，不会调用
`spawn codex`，因此不需要通过设置 `codexCommand` 来修复它。

只有切换到异步模式（或仍使用旧版兼容设置）才需要独立可执行文件：

```json
"codexRemoteCollab.controlMode": "async"
```

扩展会优先解析 `codexRemoteCollab.codexCommand`，并可回退到官方 Codex 扩展内置的
可执行文件；`codexRemoteCollab.codexArgs` 默认是 `["app-server", "--stdio"]`。
旧 `mode=attach/spawn` 会分别迁移为 `sync/async`。

## Relay 模式

### `host`（推荐）

relay 只负责认证、事件缓存和转发；VS Code 扩展通过私有 IPC 附着官方 Codex
会话。必须先打开目标会话；本机 loopback 默认不需要 host token，只有显式开启认证时
才把 host token 提供给扩展。

### `embedded`（旧的独立进程模式）

只有显式设置 `CODEX_REMOTE_MODE=embedded` 时，relay 才会启动自己的
`codex app-server --stdio`，适合测试页面和公开 app-server 协议；它与 VS Code
当前会话无关：

```sh
CODEX_REMOTE_MODE=embedded CODEX_CWD="$PWD" npm start
```

`CODEX_BIN` 可指定独立进程的可执行文件；`CODEX_ARGS_JSON` 可覆盖其参数。不要
把这些设置误认为 attach 模式的必要配置。

## HTTP API

认证开启时，除 `/api/health` 外的 `/api/*` 都需要
`Authorization: Bearer <operator-or-viewer-token>` 或 `X-Codex-Token`；本机免认证
模式下 loopback 请求直接作为 operator 处理。

```text
GET  /api/health
GET  /api/state
GET  /api/events?fromSeq=0
POST /api/command   {"commandId":"...","method":"turn/start","params":{...}}
POST /api/respond   {"requestId":"...","result":{...}}
```

host 模式下，同步控制会拒绝 `thread/start` 和网页会话导航；异步控制会把它们转给
独立 app-server。浏览器使用 `threadId` 发送 `turn/start`、`turn/steer` 或
`turn/interrupt`。认证开启时写操作和
响应请求必须使用 operator token；本机免认证模式下 loopback operator 可直接操作。

## 私有协议和限制

- IPC follower 协议是官方 VS Code 扩展的私有、带版本号实现，不是公开 API；官方
  扩展升级后可能需要同步适配。启用 `codexRemoteCollab.ipcStrictVersions`
  时，未知 stream 版本会让连接报错而不是猜测执行。
- 自动发现只把本地 rollout 元数据当作候选，最终仍通过 IPC owner discovery
  验证；生产或多会话场景建议设置明确的 `threadId`。
- relay 默认只监听 loopback，且 loopback 默认免认证；这意味着同一台机器上能访问
  loopback 的本地进程都可能控制会话，不要把它反向代理或暴露到外部。如果开启 token
  认证，token 是 bearer secret。高风险授权默认被 host policy 拒绝，只有显式设置
  `codexRemoteCollab.allowHighRiskApprovals=true` 才允许。
- 输出会做常见 token/密码脱敏，但不能识别所有秘密；不要把凭据发送给 Codex。
- 当前 UI 控制一个 host 会话，不提供多人同时编辑或文件同步。

## 测试

根目录测试使用假的 stdio app-server，不会向真实 Codex 发送任务：

```sh
npm test
cd vscode-extension && npm run check && npm run build
```

要验证真实附着，只读地打开官方 VS Code 会话后启动 bridge；不要在验证脚本中
调用 `turn/start`，除非你确实要向该会话发送任务。
