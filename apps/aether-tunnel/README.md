# aether-tunnel

Aether Tunnel 代理节点，部署在海外 VPS 上，通过 WebSocket 隧道为 Aether 实例中转 API 流量。

Tunnel 模式下代理节点**无需对外监听端口**，仅需出站连接到 Aether 服务器。

## 流式传输与升级注意事项

- 协议 v3 连接在 `HELLO` / `SETTINGS` 协商后才接收业务请求。实际双向流窗口取 gateway 与 agent 配置的较小值，信用更新阈值不超过该窗口的四分之一；单帧也不会超过协商窗口。
- 响应缓冲按字节限额并合并小帧，结束和错误状态独立保存。慢消费者不会阻塞同一隧道其他流的读取；超出窗口或缓冲预算的流会被明确终止，不会静默截断。
- 信用更新在消费数据后可靠入队；启用重定向重放时，进入有界重放缓存也视为请求体消费。持续无法投递关键控制帧时会关闭连接并向在途请求报告错误。
- 客户端取消会终止对应上游请求，断连会回收 session 的 writer、heartbeat 和请求任务。正常 drain 在配置期限内继续处理已有流，期限到达后终止残留任务。
- 建议先升级 gateway，再升级 agent。既有 v3 agent 已发送 `HELLO` / `SETTINGS`，可连接新 gateway；自定义 v3 节点必须完成这两步握手。协议 v1/v2 保留旧握手。与旧 gateway 混用时应保持默认窗口配置，不能依赖旧 gateway 应用新的窗口协商。
- 自动重连恢复后续请求，不会自动续传已经输出的 SSE，也不会无条件重放已经发送的请求。

## 安装

`aether-tunnel` 会根据宿主机自动选择服务管理器：
- 常规 Linux 发行版：`systemd`
- Alpine Linux：`OpenRC`

### 下载预编译二进制

<!-- DOWNLOAD_TABLE_START -->
| Platform | Download |
|----------|----------|
| Linux x86_64 (GNU) | [aether-tunnel-linux-amd64.tar.gz](https://github.com/fawney19/Aether/releases/download/tunnel-v0.3.17/aether-tunnel-linux-amd64.tar.gz) |
| Linux ARM64 (GNU) | [aether-tunnel-linux-arm64.tar.gz](https://github.com/fawney19/Aether/releases/download/tunnel-v0.3.17/aether-tunnel-linux-arm64.tar.gz) |
| Linux x86_64 (musl) | [aether-tunnel-linux-musl-amd64.tar.gz](https://github.com/fawney19/Aether/releases/download/tunnel-v0.3.17/aether-tunnel-linux-musl-amd64.tar.gz) |
| Linux ARM64 (musl) | [aether-tunnel-linux-musl-arm64.tar.gz](https://github.com/fawney19/Aether/releases/download/tunnel-v0.3.17/aether-tunnel-linux-musl-arm64.tar.gz) |
| macOS x86_64 | [aether-tunnel-macos-amd64.tar.gz](https://github.com/fawney19/Aether/releases/download/tunnel-v0.3.17/aether-tunnel-macos-amd64.tar.gz) |
| macOS ARM64 | [aether-tunnel-macos-arm64.tar.gz](https://github.com/fawney19/Aether/releases/download/tunnel-v0.3.17/aether-tunnel-macos-arm64.tar.gz) |
| Windows x86_64 | [aether-tunnel-windows-amd64.zip](https://github.com/fawney19/Aether/releases/download/tunnel-v0.3.17/aether-tunnel-windows-amd64.zip) |
<!-- DOWNLOAD_TABLE_END -->

上表展示的是最新已发布版本的下载链接。从下一次 `tunnel-v*` 发布开始，表格会自动补上 `Linux x86_64 (musl)` / `Linux ARM64 (musl)` 包，供 Alpine 等 musl 系统直接使用。

## 快速开始

### 一键安装 / 添加节点

一键脚本会自动从 GitHub Releases 中筛选最新的 `tunnel-v*` tag，并按当前系统下载对应制品：Linux x86_64/ARM64（GNU 或 musl）、macOS x86_64/ARM64、Windows x86_64。仓库的通用 `latest` release 可能不是 tunnel 版本，因此脚本不会使用 `/releases/latest`。

脚本会安装/更新 `aether-tunnel` 二进制，并把新的服务器配置追加到 `aether-tunnel.toml` 的 `[[servers]]` 数组中；如果配置文件已存在，不会覆盖原有内容。检测到相同 `aether_url + node_name` 时会跳过追加。

macOS / Linux:

```bash
curl -fsSL https://raw.githubusercontent.com/fawney19/Aether/main/apps/aether-tunnel/install.sh | sh
```

Windows PowerShell:

```powershell
irm https://raw.githubusercontent.com/fawney19/Aether/main/apps/aether-tunnel/install.ps1 | iex
```

也可以用环境变量非交互式执行，适合在控制台“添加隧道节点”时生成命令：

```bash
curl -fsSL https://raw.githubusercontent.com/fawney19/Aether/main/apps/aether-tunnel/install.sh | \
  AETHER_TUNNEL_AETHER_URL="https://aether.example.com" \
  AETHER_TUNNEL_MANAGEMENT_TOKEN="ae_xxx" \
  AETHER_TUNNEL_NODE_NAME="jp-proxy-01" \
  AETHER_TUNNEL_SECURITY="off" \
  sh
```

```powershell
$env:AETHER_TUNNEL_AETHER_URL = "https://aether.example.com"
$env:AETHER_TUNNEL_MANAGEMENT_TOKEN = "ae_xxx"
$env:AETHER_TUNNEL_NODE_NAME = "jp-proxy-01"
$env:AETHER_TUNNEL_SECURITY = "off"
irm https://raw.githubusercontent.com/fawney19/Aether/main/apps/aether-tunnel/install.ps1 | iex
```

可选变量：`AETHER_TUNNEL_RELEASE_TAG` 固定安装某个 `tunnel-v*` tag，`AETHER_TUNNEL_CONFIG` 指定配置文件路径，`AETHER_TUNNEL_INSTALL_DIR` 指定二进制安装目录。

```bash
# 1. 注册 root 系统服务前，先把二进制和凭据配置放入 root 管理的路径
sudo install -o root -g root -m 0755 ./aether-tunnel /usr/local/bin/aether-tunnel
sudo install -o root -g root -m 0700 -d /etc/aether-tunnel

# 2. 首次安装配置（TUI 向导，勾选 Install Service 随系统启动服务）
sudo /usr/local/bin/aether-tunnel setup /etc/aether-tunnel/aether-tunnel.toml

# 3. 日常管理 (勾选 Install Service 作为系统服务的情况下)
aether-tunnel status          # 看状态
sudo aether-tunnel logs       # 看日志

sudo aether-tunnel start      # 启动服务
sudo aether-tunnel stop       # 停止服务
sudo aether-tunnel restart    # 重启服务

# 4. 重新配置（改完自动重启服务）
sudo aether-tunnel setup /etc/aether-tunnel/aether-tunnel.toml

# 5. 彻底卸载
sudo aether-tunnel uninstall
```

完成向导后，如果启用了 Install Service，将自动注册并启动当前系统支持的服务（`systemd` 或 `OpenRC`）。为避免 root 服务被普通用户替换二进制或凭据配置，服务模式只接受 root 所有、父目录不可由组或其他用户写入的二进制，以及权限为 `0600` 的单硬链接配置文件；直接运行模式不受此限制。

### 直接运行

如果不需要安装为系统服务，可以直接运行。缺少必填参数时会自动进入 setup 向导：

```bash
./aether-tunnel
```

### 安全更新

Linux/macOS 可运行 `sudo aether-tunnel upgrade [version]`。自更新只接受本仓库的非草稿 `tunnel-v*` / `proxy-v*` SemVer Release，下载当前平台的固定资产和同一 tag 下的 `SHA256SUMS.txt`，校验后在受保护的二进制目录内原子替换，并保留上一版本用于失败回滚。Windows 不执行进程内自更新；请重新运行上面的 PowerShell 安装脚本完成手工替换，避免二段重命名产生二进制缺失窗口。

## 配置

配置按以下优先级加载（高优先级覆盖低优先级）：

1. CLI 参数
2. 环境变量（`AETHER_TUNNEL_*`）
3. 配置文件（`aether-tunnel.toml`，或通过 `AETHER_TUNNEL_CONFIG` 指定路径）

### 参数一览

#### 基础配置

| 参数 | 环境变量 | 默认值 | 说明 |
|------|----------|--------|------|
| `--aether-url` | `AETHER_TUNNEL_AETHER_URL` | **必填** | Aether 服务器地址 |
| `--management-token` | `AETHER_TUNNEL_MANAGEMENT_TOKEN` | **必填** | 管理员 Token（`ae_xxx` 格式） |
| `--node-name` | `AETHER_TUNNEL_NODE_NAME` | **必填** | 节点名称标识 |
| `--tunnel-security` | `AETHER_TUNNEL_SECURITY` | `off` | Aether ↔ tunnel 通道安全模式；支持 `off` / `non_tls_required`；在 `[[servers]]` 中省略该字段且 `http://` 提供 key 时会自动按 `non_tls_required` 生效 |
| `--tunnel-encryption-key` | `AETHER_TUNNEL_ENCRYPTION_KEY` | 空 | secure tunnel 使用的长期 PSK（base64 32-byte），每个 `[[servers]]` 节点独立配置 |
| `--public-ip` | `AETHER_TUNNEL_PUBLIC_IP` | 自动检测 | 公网 IP |
| `--node-region` | `AETHER_TUNNEL_NODE_REGION` | 自动检测 | 地区标识 |
| `--heartbeat-interval` | `AETHER_TUNNEL_HEARTBEAT_INTERVAL` | `5` | 心跳间隔（秒） |
| `--allowed-ports` | `AETHER_TUNNEL_ALLOWED_PORTS` | `80,443,8080,8443` | 允许代理的目标端口 |
| `--allow-private-targets` | `AETHER_TUNNEL_ALLOW_PRIVATE_TARGETS` | `false` | 默认拦截 private/reserved 目标地址；仅在明确需要访问内网服务时设为 `true`，通过后仍受 `allowed_ports` 限制 |

#### Tunnel 连接

| 参数 | 环境变量 | 默认值 | 说明 |
|------|----------|--------|------|
| `--tunnel-connections` | `AETHER_TUNNEL_CONNECTIONS` | 自动（硬件估算） | 最小连接池大小；显式设置后默认固定为该值 |
| `--tunnel-connections-max` | `AETHER_TUNNEL_CONNECTIONS_MAX` | 自动（硬件估算） | 连接池自动扩容上限；大于 `tunnel_connections` 时启用 autoscale |
| `--tunnel-max-streams` | `AETHER_TUNNEL_MAX_STREAMS` | 自动（硬件估算） | 单连接最大并发 stream 数 |
| `--tunnel-profile` | `AETHER_TUNNEL_PROFILE` | `standard` | 自动连接池档位：`lite=2`、`standard=4`、`throughput=8+autoscale` |
| `--tunnel-stream-initial-window-bytes` | `AETHER_TUNNEL_STREAM_INITIAL_WINDOW_BYTES` | `4194304` | v3 stream 初始流控窗口 |
| `--tunnel-drain-deadline-ms` | `AETHER_TUNNEL_DRAIN_DEADLINE_MS` | `30000` | v3 GOAWAY/drain 优雅退出期限 |
| `--tunnel-ping-interval-ms` | `AETHER_TUNNEL_PING_INTERVAL_MS` | `10000` | WebSocket ping 周期（毫秒） |
| `--tunnel-connect-timeout-ms` | `AETHER_TUNNEL_CONNECT_TIMEOUT_MS` | `3000` | tunnel 建连超时（毫秒） |
| `--tunnel-ipv4-only` | `AETHER_TUNNEL_IPV4_ONLY` | `false` | 仅使用 IPv4 地址建立直连 WebSocket tunnel；配置 `aether_outbound_proxy_url` 时仅限制代理端点解析 |
| `--tunnel-ipv6-only` | `AETHER_TUNNEL_IPV6_ONLY` | `false` | 仅使用 IPv6 地址建立直连 WebSocket tunnel；配置 `aether_outbound_proxy_url` 时仅限制代理端点解析 |
| `--tunnel-stale-timeout-ms` | `AETHER_TUNNEL_STALE_TIMEOUT_MS` | `30000` | 无入站数据断连阈值（毫秒） |
| `--tunnel-scale-check-interval-ms` | `AETHER_TUNNEL_SCALE_CHECK_INTERVAL_MS` | `1000` | autoscale 采样周期（毫秒） |
| `--tunnel-scale-up-threshold-percent` | `AETHER_TUNNEL_SCALE_UP_THRESHOLD_PERCENT` | `50` | 单 tunnel 占用率超过该值时扩容 |
| `--tunnel-scale-down-threshold-percent` | `AETHER_TUNNEL_SCALE_DOWN_THRESHOLD_PERCENT` | `35` | 单 tunnel 占用率持续低于该值时允许缩容 |
| `--tunnel-scale-down-grace-secs` | `AETHER_TUNNEL_SCALE_DOWN_GRACE_SECS` | `15` | 低负载持续时间达到该值后才回收次级 tunnel |
| `--tunnel-tcp-keepalive-secs` | `AETHER_TUNNEL_TCP_KEEPALIVE_SECS` | `30` | TCP keepalive 初始延迟（秒） |
| `--tunnel-tcp-nodelay` | `AETHER_TUNNEL_TCP_NODELAY` | `true` | 禁用 Nagle 算法 |
| `--tunnel-reconnect-base-ms` | `AETHER_TUNNEL_RECONNECT_BASE_MS` | `50` | 指数退避基础延迟（毫秒） |
| `--tunnel-reconnect-max-ms` | `AETHER_TUNNEL_RECONNECT_MAX_MS` | `250` | 指数退避上限（毫秒） |

省略 `tunnel_connections` 时，tunnel 会按 `tunnel_profile` 和设备能力自动计算一个基线值和扩容上限：`standard` 默认至少保留 4 条常驻 tunnel；如果显式设置了 `tunnel_connections` 但没有设置 `tunnel_connections_max`，则保持固定连接池，不自动扩缩。

`tunnel_ipv4_only` / `tunnel_ipv6_only` 只能二选一。它们只改变 WebSocket tunnel 回连的 TCP 地址选择：直连 Aether 时过滤 Aether 域名的 DNS 结果；配置 `aether_outbound_proxy_url` 时过滤代理服务器端点的 DNS 结果，Host/SNI 仍使用原始 WebSocket URL。该选项不会影响 provider 上游请求；如需限制 provider 上游流量，请在 `upstream_proxy_url` 或系统网络层处理。对于 Cloudflare 等边缘 IP 会变化的域名，优先使用该选项而不是固定 `/etc/hosts`。

#### 上游 HTTP 请求

| 参数 | 环境变量 | 默认值 | 说明 |
|------|----------|--------|------|
| `--upstream-connect-timeout-secs` | `AETHER_TUNNEL_UPSTREAM_CONNECT_TIMEOUT` | `30` | 上游建连超时（秒） |
| `--upstream-pool-max-idle-per-host` | `AETHER_TUNNEL_UPSTREAM_POOL_MAX_IDLE_PER_HOST` | `64` | 每 Host 最大空闲连接数 |
| `--upstream-pool-idle-timeout-secs` | `AETHER_TUNNEL_UPSTREAM_POOL_IDLE_TIMEOUT` | `300` | 连接池空闲超时（秒） |
| `--upstream-tcp-keepalive-secs` | `AETHER_TUNNEL_UPSTREAM_TCP_KEEPALIVE` | `60` | TCP keepalive（秒，0 关闭） |
| `--upstream-tcp-nodelay` | `AETHER_TUNNEL_UPSTREAM_TCP_NODELAY` | `true` | 启用 TCP_NODELAY |
| `--upstream-proxy-url` | `AETHER_TUNNEL_UPSTREAM_PROXY_URL` | 空 | 仅 provider 上游请求使用的出口代理 |
| `--upstream-proxy-remote-dns` | `AETHER_TUNNEL_UPSTREAM_PROXY_REMOTE_DNS` | `false` | 显式信任 HTTP/SOCKS5h 代理解析供应商域名并执行目标 IP 访问控制；需重启 |

启用 `follow_redirects` 后，同源 307/308 会在请求体不超过 5 MiB 时重放。首个上游请求始终流式传输；超过重放预算时不会拒绝或截断原请求，而是将 307/308 响应原样返回给调用方。

出口代理支持 `http://`、`socks5://`、`socks5h://`。配合 WARP sidecar 时可填写：

```toml
upstream_proxy_url = "socks5h://microwarp:1080"
```

默认仍由隧道本机解析供应商域名、执行端口/IP ACL，再把已校验的 IP 交给代理；仅配置
`socks5h://` 不会跳过本地 DNS。这保留现有的防 DNS 重绑定及内网访问边界。

如果隧道本机 DNS 不可用、被污染或返回不可路由的 Fake-IP，可显式委托**受信任且配置了
目的地址访问控制的代理**解析域名。在 TOML 顶层（第一个 `[[servers]]` 之前）配置：

```toml
upstream_proxy_url = "socks5h://microwarp:1080"
upstream_proxy_remote_dns = true
```

也可启用环境变量 `AETHER_TUNNEL_UPSTREAM_PROXY_REMOTE_DNS=true`、CLI 参数
`--upstream-proxy-remote-dns` 或 setup 中的 `Proxy Remote DNS` 开关，保存后重启。
该模式仅支持 `http://` 和 `socks5h://`，不支持本地解析语义的 `socks5://`；未配置代理时
启动会报错。域名原样交给 HTTP CONNECT/SOCKS5h，HTTP Host 和 TLS SNI/证书校验仍使用
原域名，不会在失败时偷偷回退到本地 DNS。

**安全边界：**普通 HTTP CONNECT/SOCKS5 不能让隧道校验代理最终解析出的目标 IP，因此
启用该模式代表把域名目标的 IP ACL 委托给代理，而不只是换一个 DNS 服务器。隧道仍检查
端口、URL 凭据/fragment、`localhost` 和 IP 字面地址；默认继续拒绝私网/保留 IP 字面地址。
这不需要打开 `allow_private_targets`。代理本身的域名仍需本地解析；如果本地 DNS 完全
不可用，使用代理 IP 地址或修复本地解析。代理 DNS、TCP、CONNECT/SOCKS 和 TLS 握手共同
受 `upstream_connect_timeout_secs` 限制。

如果需要让 Aether 管理 API 和 WebSocket tunnel 也走代理，使用 `aether_outbound_proxy_url`。

#### Aether API 客户端

| 参数 | 环境变量 | 默认值 | 说明 |
|------|----------|--------|------|
| `--aether-request-timeout-secs` | `AETHER_TUNNEL_AETHER_REQUEST_TIMEOUT` | `10` | 请求总超时（秒） |
| `--aether-connect-timeout-secs` | `AETHER_TUNNEL_AETHER_CONNECT_TIMEOUT` | `10` | 建连超时（秒） |
| `--aether-outbound-proxy-url` | `AETHER_TUNNEL_AETHER_OUTBOUND_PROXY_URL` | 空 | Aether 注册、心跳和 WebSocket tunnel 回连使用的出口代理（默认不走代理） |
| `--aether-retry-max-attempts` | `AETHER_TUNNEL_AETHER_RETRY_MAX_ATTEMPTS` | `3` | 最大重试次数 |

#### DNS 与安全

| 参数 | 环境变量 | 默认值 | 说明 |
|------|----------|--------|------|
| `--allow-private-targets` | `AETHER_TUNNEL_ALLOW_PRIVATE_TARGETS` | `false` | 默认拦截 private/reserved 目标地址；仅在明确需要访问内网服务时设为 `true`，且仅影响重启后的进程 |
| `--dns-cache-ttl-secs` | `AETHER_TUNNEL_DNS_CACHE_TTL` | `60` | DNS 缓存 TTL（秒） |
| `--dns-cache-capacity` | `AETHER_TUNNEL_DNS_CACHE_CAPACITY` | `1024` | DNS 缓存容量（条目数） |

#### 日志

| 参数 | 环境变量 | 默认值 | 说明 |
|------|----------|--------|------|
| `--log-level` | `AETHER_TUNNEL_LOG_LEVEL` | `info` | 日志级别 |
| `--log-destination` | `AETHER_TUNNEL_LOG_DESTINATION` | `both` | 输出到 `stdout`、文件或两者同时输出 |
| `--log-dir` | `AETHER_TUNNEL_LOG_DIR` | `logs` | 文件日志目录，`file/both` 时必填 |
| `--log-rotation` | `AETHER_TUNNEL_LOG_ROTATION` | `daily` | 文件日志按小时或按天轮转 |
| `--log-retention-days` | `AETHER_TUNNEL_LOG_RETENTION_DAYS` | `7` | 文件日志保留天数 |
| `--log-max-files` | `AETHER_TUNNEL_LOG_MAX_FILES` | `30` | 文件日志最多保留文件数 |

### 日志落点

- 默认 `AETHER_TUNNEL_LOG_DESTINATION=both`，同时输出到 stdout 和 `logs/` 文件目录
- 需要只交给容器日志驱动或宿主机服务管理器时，可改成 `stdout`；setup TUI 里可用 `Save Logs to File` 开关关闭文件日志
- 文件日志固定写普通文本，并支持 `hourly/daily` 轮转；默认按天轮换、保留 7 天，最多保留 30 个文件
- 以 `systemd` 或 `OpenRC` 安装时默认会额外打开文件日志到 `/var/log/aether-tunnel`
- OpenRC 安装时，`aether-tunnel logs` 实际读取 `/var/log/aether-tunnel/current.log` 和 `/var/log/aether-tunnel/error.log`；这些文件通常需要用 `sudo aether-tunnel logs` 查看

### 隧道健康上报（Heartbeat）

tunnel 会在心跳兼容字段 `proxy_metadata` 中主动上报隧道稳定性指标，便于后端直接入库/告警：

- `proxy_metadata.tunnel_metrics`：建连尝试/成功/失败、断开次数、累计在线时长、心跳 RTT、WebSocket 收发帧与字节等。
- `proxy_metadata.recent_tunnel_errors`：最近隧道异常事件（时间戳、类别、错误摘要，环形缓冲）。

说明：仅主连接（`conn=0`）发送 heartbeat，避免多条 tunnel 重复上报同一份全局指标。

### 多服务器配置

在 `aether-tunnel.toml` 中使用 `[[servers]]` 配置 Aether 服务器。即使只有一个服务器，也必须写成一个 `[[servers]]` 条目；旧的顶层单服务器写法已不再支持。

```toml
[[servers]]
aether_url = "https://aether-1.example.com"
management_token = "ae_xxx"
node_name = "jp-proxy-01"
tunnel_security = "off"

[[servers]]
aether_url = "http://127.0.0.1:8084"
management_token = "ae_yyy"
node_name = "local-dev-proxy"
tunnel_encryption_key = "base64-32-bytes"
```

`tunnel_security = "non_tls_required"` 是本机非 TLS secure tunnel 的兼容配置面：它要求同时提供当前 `[[servers]]` 条目的 `tunnel_encryption_key`，后续握手使用 `node_name` / `X-Node-Id` 查找对应 PSK，不引入 `tunnel_encryption_key_id`。公网或局域网 `aether_url` 必须使用 HTTPS；`http://` 只允许字面量 `localhost`、`127.0.0.0/8` 或 `::1`，避免明文泄漏 `management_token`。secure tunnel 只加密注册完成后的 WebSocket tunnel frame，不保护注册请求或 bootstrap 凭据，因此不能替代 HTTPS。

如果 loopback `aether_url` 使用 `http://` 且当前 `[[servers]]` 条目提供了 `tunnel_encryption_key`，省略 `tunnel_security` 时运行时会自动按 `non_tls_required` 生效；显式配置 `tunnel_security = "off"` 会关闭该自动推断。secure tunnel 会在 WebSocket tunnel 上加密所有二进制 tunnel frame；未配置 key 或显式关闭的旧节点仍按原明文协议工作。

## 发布新版本

推送 `tunnel-v*` 格式的 tag，GitHub Actions 会自动：
- 编译所有平台二进制并发布到 Releases
- 更新 README 中的下载链接表格

```bash
git tag tunnel-v0.2.0
git push origin tunnel-v0.2.0
```
