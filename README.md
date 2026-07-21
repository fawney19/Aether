<p align="center">
  <img src="frontend/public/aether_adaptive.svg" width="120" height="120" alt="Aether Logo">
</p>

<h1 align="center">Aether</h1>

<p align="center">
  <strong>一站式 AI 基础设施平台</strong><br>
  支持 Claude / OpenAI / Gemini 及其 CLI 客户端的统一接入、格式转换、正/反向代理, 致力于成为用户驱动AI服务的底座
</p>
<p align="center">
  <a href="#简介">简介</a> •
  <a href="#部署">部署</a> •
  <a href="#api-文档">API 文档</a> •
  <a href="#环境变量">环境变量</a> •
  <a href="#qa">Q&A</a>
</p>


---

## 简介

Aether 是一个自托管的 AI API 网关，为团队和个人提供多租户管理、智能负载均衡、成本配额控制和健康监控能力。通过统一的 API 入口，可以无缝对接 Claude、OpenAI、Gemini 等主流 AI 服务及其 CLI 工具。

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/architecture/architecture-dark.svg">
    <source media="(prefers-color-scheme: light)" srcset="docs/architecture/architecture-light.svg">
    <img src="docs/architecture/architecture-light.svg" width="680" alt="Aether Architecture">
  </picture>
</p>

页面预览: https://fawney19.github.io/Aether/

## 部署

### Docker Compose（推荐：预构建镜像）

```bash
# 1. 克隆代码
git clone https://github.com/fawney19/Aether.git
cd Aether

# 2. 配置环境变量
cp .env.example .env
# 生成 JWT_SECRET_KEY / ENCRYPTION_KEY, 并填入 .env
./generate_keys.sh
# 编辑 .env 设置 ADMIN_PASSWORD

# 3. 首次部署 / 更新 (从以下部署形态任选其一)
# Postgres + Redis (适用于企业或多人使用)
docker compose pull && docker compose up -d
# Single Node (适用于个人用户或朋友分享)
docker compose -f docker-compose.single-node.yml pull && docker compose -f docker-compose.single-node.yml up -d
```

### 一键更新

Docker Compose 部署后，可在部署目录直接执行：

```bash
./update.sh
```

`update.sh` 会拉取最新 `app` 镜像并重建 `app` 容器，Docker named volumes、`./data` 和 `./logs` 不会被删除。Single Node 部署也可显式指定：

```bash
./update.sh --mode single-node
```

仓库自带的 Docker Compose 默认把应用日志输出到容器 `stdout/stderr`，直接用 `docker compose logs -f app` 查看，并由 Docker 轮转日志，避免正式发布镜像切换到非 root 用户后再被宿主机挂载日志目录的权限问题拖垮启动。如果你确实需要文件日志，需要在 compose 里把 `AETHER_LOG_DESTINATION` 改成 `file|both`，并额外挂载一个容器用户可写的目录到 `/opt/aether/logs`。

管理后台右上角“版本信息”会检测新版本。Docker Compose 部署只提示版本，实际更新继续执行 `./update.sh`；systemd / launchd / 二进制部署才使用后台自更新，流程是下载对应平台的 GitHub Release 包、强制校验 `SHA256SUMS`、解压到 `/opt/aether/releases/<version>`，再切换 `/opt/aether/current` 并退出进程，交给 systemd / launchd 拉起新版本。

源码或本地构建版本不会启用后台在线更新，请继续使用源码更新流程。Docker Compose 用户如果希望“容器重建后也保持镜像层面的新版本”，仍建议定期运行 `./update.sh` 拉取并重建 app 镜像。服务器访问 GitHub 需要代理时，可设置 `AETHER_UPDATE_PROXY_URL`，也兼容 `UPDATE_PROXY_URL`、`HTTPS_PROXY`、`ALL_PROXY`、`HTTP_PROXY` 以及 `NO_PROXY`。共享出口触发 GitHub API 限流时，可设置只读 `AETHER_UPDATE_GITHUB_TOKEN`，也兼容 `GITHUB_TOKEN` / `GH_TOKEN`。下载总超时默认 600 秒，连续无响应/无数据默认 30 秒，可通过 `AETHER_UPDATE_DOWNLOAD_TIMEOUT_SECS` 和 `AETHER_UPDATE_DOWNLOAD_IDLE_TIMEOUT_SECS` 调整。

标准 Docker Compose 使用 Docker named volumes 存放 Postgres/Redis/MySQL 数据；Single Node 使用部署目录下的 `./data` 存放 SQLite 数据。

如果是本地源码构建镜像的部署，继续使用：

```bash
./deploy.sh
```

如果要在本机联调“管理后台在线更新”本身，可启动仓库内置的 release-layout 测试环境：

```bash
docker compose -f docker-compose.release-local.yml up -d --build
```

这套环境会用当前源码构建一个本地测试镜像，但编译为 `release` 类型，并默认伪装成 `v0.7.0`，这样后台会按正式发布版逻辑开放“立即更新”。默认监听 `http://127.0.0.1:18085`，数据目录使用 `./data-release-local`；日志默认走 `docker logs`，不会影响你正在跑的源码构建容器。

如果这套容器在 `prepare-update` 时访问 GitHub 失败，而你本机是通过代理出网，请在 `.env` 里把 `AETHER_UPDATE_PROXY_URL` 写成宿主机地址，例如 `http://host.docker.internal:7890`；容器内的 `127.0.0.1` 指向容器自身，不是宿主机。

如果想重置这套联调环境（包括 `/opt/aether/current` 和已下载的历史版本），执行：

```bash
docker compose -f docker-compose.release-local.yml down -v
```

可选变量：

- `AETHER_RELEASE_LOCAL_VERSION`：本地联调镜像对外声明的当前版本，默认 `v0.7.0`
- `AETHER_RELEASE_LOCAL_PORT`：本地联调端口，默认 `18085`
- `LOCAL_RELEASE_APP_IMAGE`：本地联调镜像名，默认 `aether-app:release-local`

### 一键安装（默认 Single Node：Linux systemd / macOS launchd + SQLite）

```bash
git clone https://github.com/fawney19/Aether.git
cd Aether
curl -fsSL https://raw.githubusercontent.com/fawney19/Aether/main/install.sh | sudo bash
```

## 本地开发

依赖 Docker、Rust toolchain、Node.js 和 make。

```bash
make dev
```

`make dev` 会同时启动后端 `aether-gateway` 和前端 `frontend` 的 Vite dev server。需要单独启动时可使用 `make dev-backend` 或 `make dev-frontend`。
Postgres / Redis 本地依赖未就绪时，`make dev` 会自动执行 `docker compose up -d postgres redis`。

## Aether Tunnel (可选)

Aether Tunnel 是配套的正向代理节点，部署在海外 VPS 上，为墙内的 Aether 实例中转 API 流量。

- Docker Compose 部署或下载预编译二进制直接运行
- 提供 macOS/Linux 与 Windows 一键脚本，自动下载最新 `tunnel-v*` 制品并向现有 `aether-tunnel.toml` 追加 `[[servers]]`
- 通过 `aether-tunnel setup` 完成交互式配置，自动注册为系统服务
- 详细文档见 [apps/aether-tunnel/README.md](apps/aether-tunnel/README.md)

## API 文档

- Embeddings: [OpenAI compatible `POST /v1/embeddings`](docs/api/embeddings.md)
- Rerank: [OpenAI/Jina compatible `POST /v1/rerank`](docs/api/rerank.md)

## 环境变量

- `APP_PORT`：`aether-gateway` 唯一监听端口，固定绑定 `0.0.0.0:${APP_PORT}`
- `DATABASE_URL`：数据库连接串；SQLite 例如 `sqlite:///opt/aether/data/aether.db`，Postgres 例如 `postgresql://postgres:aether@postgres:5432/aether`
- `AETHER_GATEWAY_DATA_POSTGRES_MIN_CONNECTIONS` / `AETHER_GATEWAY_DATA_POSTGRES_MAX_CONNECTIONS`：数据库连接池手动覆盖值；未配置时会自动推导，SQLite 固定 `1/1`，Postgres/MySQL 按 CPU 核心数计算并默认封顶 `100`
- `AETHER_GATEWAY_MAX_IN_FLIGHT_REQUESTS`：单实例请求并发上限；未配置时按 CPU 自动推导（基础范围 `512-65536`），低文件描述符预算时会进一步下调
- `AETHER_GATEWAY_REQUEST_BODY_BUFFER_BUDGET_MB`：单实例同时读取和解压请求体的加权内存预算，默认 `256MB`
- `AETHER_GATEWAY_REQUEST_BODY_READ_TIMEOUT_MS`：请求体完整读取超时，默认 `120000ms`
- `AETHER_MAX_REQUEST_BODY_MB`：单请求解压后的最大请求体，默认 `64MB`
- `AETHER_MAX_INTERNAL_BUFFERED_BODY_MB`：heartbeat、管理探测等内部必须整包读取的响应体上限，默认 `128MB`
- `AETHER_TUNNEL_NODE_STATUS_QUEUE_CAPACITY`：隧道节点状态上报队列容量，默认 `1024`；满载时拒绝新事件，避免控制面故障导致无界内存增长
- `AETHER_GATEWAY_SECURITY_CACHE_TTL_MS`：IP 黑白名单本地缓存时间，默认 `1000ms`，写操作会主动失效相关缓存
- `AETHER_MAX_REDACTED_SYNC_RESPONSE_BODY_MB`：启用 PII 恢复时同步响应允许缓冲的最大大小，默认 `64MB`
- `REDIS_URL`：Redis 连接串；仅 Postgres + Redis 的 Docker Compose 部署需要配置
- `AETHER_RUNTIME_BACKEND=memory|redis`：运行时缓存/协调后端。SQLite 默认用 `memory`，不会连接 Redis
- `AETHER_GATEWAY_AUTO_PREPARE_DATABASE`：常规启动前自动执行挂起的 schema migration 和 backfill；仓库自带的 `docker-compose.yml` 默认开启
- `JWT_SECRET_KEY` / `ENCRYPTION_KEY`：认证和敏感数据加密所需密钥
- `API_KEY_PREFIX`：用户和管理员新建 API Key 时使用的前缀，默认 `sk`
- `ADMIN_USERNAME` / `ADMIN_PASSWORD` / `ADMIN_EMAIL`：首次启动时自举首个本地管理员；`install.sh` 会提示输入管理员密码
- `CORS_ORIGINS` / `CORS_ALLOW_CREDENTIALS`：前端跨域来源控制；如果要跨域带登录 Cookie，`CORS_ORIGINS` 不能写 `*`
- `RUST_LOG`：Rust 日志过滤，例如 `aether_gateway=info`、`aether_gateway=debug,sqlx=warn`
- Docker Compose 的 `DB_PASSWORD` / `REDIS_PASSWORD` 默认使用 `aether`

---

## 许可证

本项目采用 [Aether 非商业开源许可证](LICENSE)。允许个人学习、教育研究、非盈利组织及企业内部非盈利性质的使用；禁止用于盈利目的。商业使用请联系获取商业许可。

## 联系作者

<p align="center">
  <img src="docs/author/qq_qrcode.jpg" width="200" alt="QQ二维码">
  &nbsp;&nbsp;&nbsp;&nbsp;
  <img src="docs/author/qrcode_1770574997172.jpg" width="200" alt="QQ群二维码">
</p>

## Star History

[![Star History Chart](https://api.star-history.com/svg?repos=fawney19/Aether&type=date&legend=top-left)](https://www.star-history.com/?repos=fawney19%2FAether&type=date&legend=top-left)
