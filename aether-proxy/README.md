# aether-proxy

Aether Tunnel 代理节点，部署在海外 VPS 上，通过 WebSocket 隧道为 Aether 实例中转 API 流量。

Tunnel 模式下代理节点**无需对外监听端口**，仅需出站连接到 Aether 服务器。

## 安装

### Docker Compose 部署

```bash
cp .env.example .env
# 编辑 .env 填入 AETHER_PROXY_AETHER_URL 和 AETHER_PROXY_MANAGEMENT_TOKEN
docker compose up -d
```

### 下载预编译二进制

<!-- DOWNLOAD_TABLE_START -->
| Platform | Download |
|----------|----------|
| Linux x86_64 | [aether-proxy-linux-amd64.tar.gz](https://github.com/fawney19/Aether/releases/download/proxy-v0.2.5/aether-proxy-linux-amd64.tar.gz) |
| Linux ARM64 | [aether-proxy-linux-arm64.tar.gz](https://github.com/fawney19/Aether/releases/download/proxy-v0.2.5/aether-proxy-linux-arm64.tar.gz) |
| macOS x86_64 | [aether-proxy-macos-amd64.tar.gz](https://github.com/fawney19/Aether/releases/download/proxy-v0.2.5/aether-proxy-macos-amd64.tar.gz) |
| macOS ARM64 | [aether-proxy-macos-arm64.tar.gz](https://github.com/fawney19/Aether/releases/download/proxy-v0.2.5/aether-proxy-macos-arm64.tar.gz) |
| Windows x86_64 | [aether-proxy-windows-amd64.zip](https://github.com/fawney19/Aether/releases/download/proxy-v0.2.5/aether-proxy-windows-amd64.zip) |
<!-- DOWNLOAD_TABLE_END -->

## 快速开始

```bash
# 1. 首次安装配置（TUI 向导，勾选 Install Service 随系统启动服务）
sudo ./aether-proxy setup

# 2. 日常管理 (勾选 Install Service 作为系统服务的情况下)
aether-proxy status          # 看状态
aether-proxy logs            # 看日志

sudo aether-proxy start      # 启动服务
sudo aether-proxy stop       # 停止服务
sudo aether-proxy restart    # 重启服务

# 3. 重新配置（改完自动重启服务）
sudo aether-proxy setup

# 4. 彻底卸载
sudo aether-proxy uninstall
```

完成向导后, 配置自动保存到 `aether-proxy.toml`，如果启用了 Install Service，将自动注册并启动 systemd 服务。

### 直接运行

如果不需要安装为系统服务，可以直接运行。缺少必填参数时会自动进入 setup 向导：

```bash
./aether-proxy
```

## 配置

配置按以下优先级加载（高优先级覆盖低优先级）：

1. CLI 参数
2. 环境变量（`AETHER_PROXY_*`）
3. 配置文件（`aether-proxy.toml`，或通过 `AETHER_PROXY_CONFIG` 指定路径）

### 参数一览

#### 基础配置

| 参数 | 环境变量 | 默认值 | 说明 |
|------|----------|--------|------|
| `--aether-url` | `AETHER_PROXY_AETHER_URL` | **必填** | Aether 服务器地址 |
| `--management-token` | `AETHER_PROXY_MANAGEMENT_TOKEN` | **必填** | 管理员 Token（`ae_xxx` 格式） |
| `--public-ip` | `AETHER_PROXY_PUBLIC_IP` | 自动检测 | 公网 IP |
| `--node-name` | `AETHER_PROXY_NODE_NAME` | `proxy-01` | 节点名称标识 |
| `--node-region` | `AETHER_PROXY_NODE_REGION` | 自动检测 | 地区标识 |
| `--heartbeat-interval` | `AETHER_PROXY_HEARTBEAT_INTERVAL` | `30` | 心跳间隔（秒） |
| `--allowed-ports` | `AETHER_PROXY_ALLOWED_PORTS` | `80,443,8080,8443` | 允许代理的目标端口 |

#### Tunnel 连接

| 参数 | 环境变量 | 默认值 | 说明 |
|------|----------|--------|------|
| `--tunnel-connections` | `AETHER_PROXY_TUNNEL_CONNECTIONS` | `3` | 到 Aether 的连接池大小 |
| `--tunnel-max-streams` | `AETHER_PROXY_TUNNEL_MAX_STREAMS` | 自动（硬件估算） | 单连接最大并发 stream 数 |
| `--tunnel-connect-timeout-secs` | `AETHER_PROXY_TUNNEL_CONNECT_TIMEOUT_SECS` | `15` | TCP + TLS 握手超时（秒） |
| `--tunnel-tcp-keepalive-secs` | `AETHER_PROXY_TUNNEL_TCP_KEEPALIVE_SECS` | `30` | TCP keepalive 初始延迟（秒） |
| `--tunnel-tcp-nodelay` | `AETHER_PROXY_TUNNEL_TCP_NODELAY` | `true` | 禁用 Nagle 算法 |
| `--tunnel-ping-interval-secs` | `AETHER_PROXY_TUNNEL_PING_INTERVAL_SECS` | `15` | WebSocket Ping 频率（秒） |
| `--tunnel-stale-timeout-secs` | `AETHER_PROXY_TUNNEL_STALE_TIMEOUT_SECS` | `45` | 无数据断连阈值（秒） |
| `--tunnel-reconnect-base-ms` | `AETHER_PROXY_TUNNEL_RECONNECT_BASE_MS` | `500` | 指数退避基础延迟（毫秒） |
| `--tunnel-reconnect-max-ms` | `AETHER_PROXY_TUNNEL_RECONNECT_MAX_MS` | `30000` | 指数退避上限（毫秒） |

#### 上游 HTTP 请求

| 参数 | 环境变量 | 默认值 | 说明 |
|------|----------|--------|------|
| `--upstream-connect-timeout-secs` | `AETHER_PROXY_UPSTREAM_CONNECT_TIMEOUT_SECS` | `30` | 上游建连超时（秒） |
| `--upstream-pool-max-idle-per-host` | `AETHER_PROXY_UPSTREAM_POOL_MAX_IDLE_PER_HOST` | `64` | 每 Host 最大空闲连接数 |
| `--upstream-pool-idle-timeout-secs` | `AETHER_PROXY_UPSTREAM_POOL_IDLE_TIMEOUT_SECS` | `300` | 连接池空闲超时（秒） |
| `--upstream-tcp-keepalive-secs` | `AETHER_PROXY_UPSTREAM_TCP_KEEPALIVE_SECS` | `60` | TCP keepalive（秒，0 关闭） |
| `--upstream-tcp-nodelay` | `AETHER_PROXY_UPSTREAM_TCP_NODELAY` | `true` | 启用 TCP_NODELAY |

#### Aether API 客户端

| 参数 | 环境变量 | 默认值 | 说明 |
|------|----------|--------|------|
| `--aether-request-timeout-secs` | `AETHER_PROXY_AETHER_REQUEST_TIMEOUT_SECS` | `10` | 请求总超时（秒） |
| `--aether-connect-timeout-secs` | `AETHER_PROXY_AETHER_CONNECT_TIMEOUT_SECS` | `10` | 建连超时（秒） |
| `--aether-retry-max-attempts` | `AETHER_PROXY_AETHER_RETRY_MAX_ATTEMPTS` | `3` | 最大重试次数 |

#### DNS 与安全

| 参数 | 环境变量 | 默认值 | 说明 |
|------|----------|--------|------|
| `--dns-cache-ttl-secs` | `AETHER_PROXY_DNS_CACHE_TTL_SECS` | `60` | DNS 缓存 TTL（秒） |
| `--dns-cache-capacity` | `AETHER_PROXY_DNS_CACHE_CAPACITY` | `1024` | DNS 缓存容量（条目数） |

#### 日志

| 参数 | 环境变量 | 默认值 | 说明 |
|------|----------|--------|------|
| `--log-level` | `AETHER_PROXY_LOG_LEVEL` | `info` | 日志级别 |
| `--log-json` | `AETHER_PROXY_LOG_JSON` | `false` | JSON 格式日志 |

### 多服务器配置

在 `aether-proxy.toml` 中使用 `[[servers]]` 配置多个 Aether 服务器：

```toml
[[servers]]
aether_url = "https://aether-1.example.com"
management_token = "ae_xxx"
node_name = "jp-proxy-01"

[[servers]]
aether_url = "https://aether-2.example.com"
management_token = "ae_yyy"
node_name = "jp-proxy-02"
```

## 发布新版本

推送 `proxy-v*` 格式的 tag，GitHub Actions 会自动：
- 编译所有平台二进制并发布到 Releases
- 构建 Docker 镜像并推送到 GHCR 和 Docker Hub
- 更新 README 中的下载链接表格

```bash
git tag proxy-v0.2.0
git push origin proxy-v0.2.0
```
