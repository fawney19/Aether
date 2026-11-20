# Provider 查询（/api/admin/provider-query）

> 家族：`provider_query_manage` · 权限域：`admin:provider_query`
> 说明：渠道模型查询 / 测试模型 / 故障转移测试
> 鉴权：`Authorization: Bearer ae-xxx`（管理令牌）或 Web 会话 JWT。

## 端点表

| 方法 | 路径 | kind |
|---|---|---|
| POST | `/api/admin/provider-query/models` | `query_models` |
| POST | `/api/admin/provider-query/test-model` | `test_model` |
| POST | `/api/admin/provider-query/test-model-failover` | `test_model_failover` |

## 示例

```bash
curl -sS -X GET https://<网关域名>/api/admin/provider-query/models?provider_id={id} -H "Authorization: Bearer ae-xxx"
```

```bash
curl -sS -X POST https://<网关域名>/api/admin/provider-query/test-model \
  -H "Authorization: Bearer ae-xxx" -H "Content-Type: application/json" \
  -d '{"provider_id":"<id>","model":"<模型名>"}'
```
