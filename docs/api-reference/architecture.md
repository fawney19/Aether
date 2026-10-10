# 架构与部署

## 一、整体拓扑

```
客户端（Codex / OpenAI SDK / Claude SDK / curl）
   │  HTTPS（Authorization: Bearer sk-xxx 或 ae-xxx）
   ▼
反向代理（可选，如 Cloudflare / Nginx / OpenResty）
   ▼
aether-app（网关容器，默认 127.0.0.1:8084）
   │
   ├── aether-postgres（postgres:15）
   ├── aether-redis（redis:7，无持久化）
   │
   ├── [出站直连] 上游 API（各渠道）
   └── [隧道转发] /api/internal/tunnel/relay/{node_id} ──► aether-tunnel 节点
```

- 网关容器：`aether-app`，监听 `127.0.0.1:8084`（建议仅本机回环，前端经反代）
- 数据目录：`/opt/aether`（日志挂载到容器 `/app/logs`）
- 日志：`/opt/aether/logs/aether-gateway.YYYY-MM-DD.log`

## 二、路由分组（router.rs 挂载顺序）

| 组 | 前缀 | 说明 |
|---|---|---|
| core | `/_gateway/health`、`/readyz`、`/.well-known/aether/frontdoor.json` | 健康/就绪/清单 |
| operational | `/_gateway/metrics`、`/_gateway/audit/*`、`/_gateway/async-tasks/*` | Prometheus、审计、任务 |
| AI | `/v1/*`、`/v1beta/*`、`/upload/*` | 客户端兼容接口 |
| public_support | `/v1/models`、`/api/public/*`、`/api/auth/*`、`/install/*`、`/` | 前端/公共 |
| oauth | `/api/oauth/*`、`/api/user/oauth/*` | OAuth |
| internal | `/api/internal/gateway/*`、`/api/internal/proxy-tunnel`、`/api/internal/tunnel/*` | 隧道/内部 |
| admin | `/api/admin/*` | 管理 API |
| catch-all | `/{*path}` → proxy_request | 兼容转发兜底 |

## 三、内部/隧道 API

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/api/internal/proxy-tunnel` | 隧道握手（WebSocket 升级） |
| POST | `/api/internal/tunnel/heartbeat` | 节点心跳 |
| POST | `/api/internal/tunnel/node-status` | 节点状态上报 |
| POST | `/api/internal/tunnel/relay/{node_id}` | 出站请求转发（节点代理） |
| ANY | `/api/internal/gateway/{*}` | 内部网关转发 |

## 四、部署组件（docker-compose.yml）

| 服务 | 镜像 | 端口 | 说明 |
|---|---|---|---|
| postgres | postgres:15 | 127.0.0.1:5432 | 主库（`pg_stat_statements`） |
| redis | redis:7-alpine | 127.0.0.1:6379 | 缓存/限流（`appendonly no`） |
| mysql | mysql:8.0 | 127.0.0.1:3306 | 可选 profile |
| app | 网关镜像 | 127.0.0.1:8084 | 主程序 |

关键环境变量：`DATABASE_URL`、`REDIS_URL`、`JWT_SECRET_KEY`（登录令牌签名）、`APP_PORT`、`AETHER_BASE_DIR`、`AETHER_GATEWAY_AUTO_PREPARE_DATABASE=true`

## 五、常用运维命令

```bash
# 日志
docker logs --tail 200 aether-app
docker exec aether-app sh -c 'grep -aE "WARN|ERROR" /app/logs/aether-gateway.$(date +%F).log | tail -50'

# 容器管理
docker restart aether-app          # 改配置一般无需重启（实时读库）

# 节点状态
ssh <节点> journalctl -u aether-tunnel --no-pager -n 50
```

## 六、格式转换架构

入站格式（openai:responses / openai:chat / claude:messages / gemini:* / jina:* / doubao:* / aliyun:*）统一经 `crates/aether-ai/formats` 转换矩阵：
1. 客户端请求 → 内部规范格式（internal）
2. 内部格式 → 渠道格式（provider format）
3. 渠道响应 → 客户端格式回写

> body_rules 在转换为 provider 格式**之后**应用（`standard_matrix.rs`）；`$item` 绑定最后一个通配符所在元素。
