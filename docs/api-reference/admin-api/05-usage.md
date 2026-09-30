# 用量查询与审计（/api/admin/usage）

> 家族：`usage_manage`（权限域 `admin:usage`）
> 代码位置：`apps/aether-gateway/src/handlers/admin/observability/usage/`

## 端点清单

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/api/admin/usage/stats` | 用量统计汇总 |
| GET | `/api/admin/usage/records` | **请求记录分页**（排查主入口） |
| GET | `/api/admin/usage/{request_id}` | 单条记录详情 |
| GET | `/api/admin/usage/{request_id}/curl` | 生成 curl 重放命令 |
| POST | `/api/admin/usage/{request_id}/replay` | 重放请求 |
| GET | `/api/admin/usage/active` | 活跃请求 |
| GET | `/api/admin/usage/heatmap` | 用量热力图 |
| GET | `/api/admin/usage/aggregation/stats` | 聚合统计 |
| GET | `/api/admin/usage/cache-affinity/*` | 缓存亲和性分析（hit-analysis / interval-timeline / ttl-analysis） |

## GET records（排查主入口）

```
GET /api/admin/usage/records?limit=20&offset=0&user_id=<id>&provider=opencode+zen&model=<模型名>&api_format=openai:responses&status=error&search=关键字
```

参数：
- `limit` / `offset`：分页
- `user_id` / `provider` / `model` / `api_format` / `status`：过滤
- `search`：关键字（用户名/request_id 等）
- `include_total`：返回 total

响应（每条 record 对应 usage 表一行）：

```json
{
  "records": [
    {
      "request_id": "uuid",
      "created_at": "2026-08-17T10:00:00Z",
      "username": "zbs",
      "api_key_name": "my-key",
      "provider_name": "<渠道名>",
      "model": "<模型名>",
      "target_model": "<模型名>",
      "api_format": "openai:responses",
      "endpoint_kind": "responses",
      "is_stream": true,
      "status_code": 400,
      "status": "error",
      "error_category": "schema_validation",
      "error_message": "Invalid schema for function ...",
      "response_time_ms": 1234,
      "first_byte_time_ms": 800,
      "input_tokens": 1200,
      "output_tokens": 300,
      "total_cost_usd": 0.0012,
      "request_cost_usd": 0.0012,
      "candidate_index": 0,
      "route_family": "openai",
      "execution_path": "local_tunnel"
    }
  ],
  "total": 4321,
  "limit": 20,
  "offset": 0
}
```

## GET 详情

```
GET /api/admin/usage/{request_id}
```

返回单条完整记录，含 `request_body` / `provider_request_body` / `response_body` 等字段（大体积字段以压缩态存储时走 usage_body_blobs）。

## GET curl / POST replay（重放验证）

```
GET  /api/admin/usage/{request_id}/curl      # 返回可直接执行的 curl 命令（含原始请求体）
POST /api/admin/usage/{request_id}/replay    # 服务端直接重放
```

> 运维技巧：排查"改了 body_rules 是否生效"时，先拿失败请求的 curl 重放，改完规则再重放对比结果（详见 ops/playbook.md 的端到端验证）。

## 直接查库（更灵活，生产常用）

```sql
select created_at, request_id, provider_name, model, api_format,
       provider_endpoint_kind, status_code, status, error_category,
       left(error_message, 200) as error, response_time_ms
from usage
where created_at > now() - interval '2 hours'
order by created_at desc limit 30;
```

请求/响应体在 `usage_body_blobs`（gzip bytea）：

```bash
# 解压（本地 python）
docker exec aether-postgres psql -U postgres -d aether -Atc \
"select encode(payload_gzip,'base64') from usage_body_blobs where request_id='<id>' and body_field='request_body';"
# base64.b64decode → gzip.decompress → json
```

路由候选链在 `request_candidates`：

```sql
select request_id, provider_name, endpoint_id, extra_data
from request_candidates where request_id='<id>' order by candidate_index;
```
