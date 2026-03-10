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
python generate_keys.py  # 生成密钥, 并将生成的密钥填入 .env

# 3. 部署 / 更新（自动执行数据库迁移）
docker compose pull && docker compose up -d

# 4. 升级前备份 (可选)
docker compose exec postgres pg_dump -U postgres aether | gzip > backup_$(date +%Y%m%d_%H%M%S).sql.gz
```

### Docker Compose（本地构建镜像）

```bash
# 1. 克隆代码
git clone https://github.com/fawney19/Aether.git
cd Aether

# 2. 配置环境变量
cp .env.example .env
python generate_keys.py  # 生成密钥, 并将生成的密钥填入 .env

# 3. 部署 / 更新（自动构建、启动、迁移）
./deploy.sh
```

### 本地开发

```bash
# 启动依赖
docker compose -f docker-compose.build.yml up -d postgres redis

# 后端
uv sync
./dev.sh

# 前端
cd frontend && npm install && npm run dev
```

## Aether Proxy (可选)

Aether Proxy 是配套的正向代理节点，部署在海外 VPS 上，为墙内的 Aether 实例中转 API 流量。或者部署在其他服务器为指定的提供商、账号、Key使用不同的节点访问。支持 TUI 向导一键配置、systemd 服务管理、TLS 加密、DNS 缓存及连接池调优。

- Docker Compose 部署或下载预编译二进制直接运行
- 通过 `aether-proxy setup` 完成交互式配置，自动注册为系统服务
- 详细文档见 [aether-proxy/README.md](aether-proxy/README.md)

## 环境变量

### 必需配置

| 变量 | 说明 |
|------|------|
| `DB_PASSWORD` | PostgreSQL 数据库密码 |
| `REDIS_PASSWORD` | Redis 密码 |
| `JWT_SECRET_KEY` | JWT 签名密钥（使用 `generate_keys.py` 生成） |
| `ENCRYPTION_KEY` | API Key 加密密钥（更换后需重新配置 Provider Key） |
| `ADMIN_EMAIL` | 初始管理员邮箱 |
| `ADMIN_USERNAME` | 初始管理员用户名 |
| `ADMIN_PASSWORD` | 初始管理员密码 |

### 可选配置

| 变量 | 默认值 | 说明 |
|------|--------|------|
| `APP_PORT` | 8084 | 应用端口 |
| `API_KEY_PREFIX` | sk | API Key 前缀 |
| `LOG_LEVEL` | INFO | 日志级别 (DEBUG/INFO/WARNING/ERROR) |
| `GUNICORN_WORKERS` | 2 | Gunicorn 工作进程数 |
| `DB_PORT` | 5432 | PostgreSQL 端口 |
| `REDIS_PORT` | 6379 | Redis 端口 |

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
# 1. 停止应用
docker compose stop app

# 2. 恢复数据库（先清空再导入）
docker compose exec -T postgres psql -U postgres -c "DROP DATABASE aether; CREATE DATABASE aether;"
gunzip < backup_xxx.sql.gz | docker compose exec -T postgres psql -U postgres -d aether

# 3. 拉取旧版本镜像并重启
#    方式一：使用具体版本 tag（如果有发布版本号）
#    将 docker-compose.yml 中 image 从 ghcr.io/fawney19/aether:latest 改为指定版本
#    方式二：使用之前记录的镜像 digest
#    将 image 改为 ghcr.io/fawney19/aether@sha256:xxxxx
docker compose up -d app
```

> 可以在升级前通过 `docker inspect ghcr.io/fawney19/aether:latest --format '{{index .RepoDigests 0}}'` 记录当前镜像 digest，方便回滚时使用。

**没有备份的情况：**

```bash
# 1. 用当前容器回退数据库迁移（回退 1 步，按需调整数字）
docker compose exec app alembic downgrade -1

# 2. 查看回退后的版本确认正确
docker compose exec app alembic current

# 3. 切回旧镜像并重启（同上方式修改 docker-compose.yml 中的 image）
docker compose up -d app
```

> 注意：没有备份的回滚依赖 alembic downgrade，如果迁移涉及不可逆的数据变更（如删除列），可能无法完全恢复数据。因此强烈建议升级前备份。

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
