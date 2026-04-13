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
./generate_keys.sh  # 生成密钥, 并将生成的密钥填入 .env

# 3. 首次部署 / 更新
docker compose pull && docker compose up -d

# 4. 如果后续版本包含 schema 变更，再显式执行数据库迁移
docker compose run --rm app --migrate

# 5. 升级前备份 (可选)
docker compose exec postgres pg_dump -U postgres aether | gzip > backup_$(date +%Y%m%d_%H%M%S).sql.gz
```

### Docker Compose（本地构建镜像）

```bash
# 1. 克隆代码
git clone https://github.com/fawney19/Aether.git
cd Aether

# 2. 配置环境变量
cp .env.example .env
./generate_keys.sh  # 生成密钥, 并将生成的密钥填入 .env

# 3. 部署 / 更新（自动构建并启动）
git pull
./deploy.sh
```

### 本地开发

```bash
# 启动依赖
docker compose -f docker-compose.build.yml up -d postgres redis

# 数据库迁移（仅在已有数据库引入新 migration 时需要）
./dev.sh --migrate

# 后端
./dev.sh

# 前端
cd frontend && npm install && npm run dev
```

`./dev.sh` 现在只保留一种本地模式：

| 角色 | 本地地址 | 说明 |
|------|----------|------|
| Rust frontdoor | 默认 `http://localhost:8084` | `aether-gateway`，本地唯一公开入口；实际端口由 `APP_PORT` 控制 |

本地默认链路是：

```text
client -> rust frontdoor (aether-gateway) -> execution_runtime/provider transport
```

其中：

- `aether-gateway` 负责公开入口、健康检查、格式转换、本地执行 runtime，以及当前已迁到 Rust 的 frontdoor/control/background 路径。
- `./dev.sh` 不再启动 Python 宿主；未下沉到 Rust 的 legacy 路由会直接失败。
- `./dev.sh --migrate` 会复用 `.env` 里的数据库配置，显式执行一次数据库迁移后退出。
- `./dev.sh` 默认把 `AETHER_GATEWAY_VIDEO_TASK_TRUTH_SOURCE_MODE` 设为 `rust-authoritative`，避免本地还依赖 Python sync report 语义。
- 空库首次启动会自动初始化到当前 baseline。
- `aether-gateway` 默认启动不会自动应用后续 schema migration；如果数据库版本落后，服务会拒绝启动，并提示先执行 `aether-gateway --migrate`。

## Aether Proxy (可选)

Aether Proxy 是配套的正向代理节点，部署在海外 VPS 上，为墙内的 Aether 实例中转 API 流量。或者部署在其他服务器为指定的提供商、账号、Key使用不同的节点访问。支持 TUI 向导一键配置、systemd 服务管理、TLS 加密、DNS 缓存及连接池调优。

- Docker Compose 部署或下载预编译二进制直接运行
- 通过 `aether-proxy setup` 完成交互式配置，自动注册为系统服务
- 详细文档见 [apps/aether-proxy/README.md](apps/aether-proxy/README.md)

## 环境变量

部署建议直接参考对应示例文件：

- Docker Compose：根目录 [`.env.example`](.env.example)
- systemd 二进制部署：[deploy/systemd/aether-gateway.env.example](deploy/systemd/aether-gateway.env.example)

当前主链路真正要关注的是这组变量：

- `APP_PORT`：`aether-gateway` 唯一监听端口，固定绑定 `0.0.0.0:${APP_PORT}`
- `DATABASE_URL` / `REDIS_URL`：`aether-gateway` 直接读取的共享后端连接串
- `JWT_SECRET_KEY` / `ENCRYPTION_KEY`：认证和敏感数据加密所需密钥
- `API_KEY_PREFIX`：用户和管理员新建 API Key 时使用的前缀，默认 `sk`
- `PAYMENT_CALLBACK_SECRET`：支付回调公开入口的共享密钥；未配置时相关路由保持禁用
- `ADMIN_USERNAME` / `ADMIN_PASSWORD` / `ADMIN_EMAIL`：首次启动时自举首个本地管理员
- `CORS_ORIGINS` / `CORS_ALLOW_CREDENTIALS`：前端跨域来源控制；如果要跨域带登录 Cookie，`CORS_ORIGINS` 不能写 `*`
- `AETHER_GATEWAY_DEPLOYMENT_TOPOLOGY=single-node|multi-node`
- `AETHER_GATEWAY_NODE_ROLE=all|frontdoor|background`
- `RUST_LOG`：Rust 日志过滤，例如 `aether_gateway=info`、`aether_gateway=debug,sqlx=warn`
- 如果使用仓库内置的数据栈 compose，再额外配置 `DB_PASSWORD` / `REDIS_PASSWORD`

systemd 的 `.env` 必须保持简单 `KEY=VALUE` 形式，不要写 `export`、`${VAR}` 或命令替换。

## Q&A

### Q: 如何开启/关闭请求体记录？

管理员在 **系统设置** 中配置日志记录的详细程度:

| 级别 | 记录内容 |
|------|----------|
| Base | 基本请求信息 |
| Headers | Base + 请求头 |
| Full | Headers + 请求体 |

### Q: 更新出问题如何回滚？

**有备份的情况（推荐）：**

```bash
# Docker Compose:
# 1. 切回旧镜像 tag / digest
# 2. 恢复 Postgres 备份
# 3. 再启动 app

# systemd:
# 1. 把 /opt/aether/current 切回旧 release
# 2. systemctl restart aether-gateway
# 3. 如果升级包含数据库结构变更，再恢复 Postgres 备份
```

> 可以在升级前通过 `docker inspect ghcr.io/fawney19/aether:latest --format '{{index .RepoDigests 0}}'` 记录当前镜像 digest，方便回滚时使用。

**没有备份的情况：**

当前不应该再依赖旧的 `alembic downgrade` 路线。空库首次启动会自动初始化，但后续数据库升级需要显式执行 `aether-gateway --migrate`；常规服务启动不会自动应用新增 migration。如果本次发布带来了不可逆的数据结构变化，没有备份就不能保证安全回滚。因此升级前强烈建议先备份 `Postgres`。

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

[![Star History Chart](https://api.star-history.com/svg?repos=fawney19/Aether&type=Date)](https://star-history.com/#fawney19/Aether&Date)
