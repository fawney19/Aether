# body_rules 语法与实战规则

> 来源：`aether-gateway-ops` skill + 代码 `apps/aether-gateway/src/handlers/admin/provider/endpoints_admin/payloads.rs`（字段：`body_rules` / `header_rules`）
> 作用时机：请求在**转换为 provider 格式之后**应用（`standard_matrix.rs`）；即规则作用于"即将发给上游的 body"。

## 一、规则结构

规则是 JSON 数组，每个元素：

```json
{
  "path": "JSON路径（支持 tools[*] 通配）",
  "action": "drop | set",
  "value": "set 时的值（任意 JSON）",
  "condition": {
    "op": "eq | starts_with | not_exists | ...",
    "path": "$item.xxx（引用数组元素）",
    "value": "比较值",
    "source": "original（可选，用原始 body 值）"
  }
}
```

- 单条 condition 可直接 `op/path/value`；多条用 `{"all":[...]}` 组合
- 可多层 `all` 组合
- `$item` 绑定**最后一个通配符**所在元素（`rules.rs::get_item_prefix_from_concrete`）
- 递归展开只命中已存在的 key，不会为无关工具建字段

## 二、实战规则

### 例 1：删除指定工具（automation_update，responses 格式）

```json
[
  {"path": "tools[*]", "action": "drop", "condition": {"op": "eq", "path": "$item.name", "value": "automation_update"}},
  {"path": "tools[*]", "action": "drop", "condition": {"op": "eq", "path": "$item.name", "value": "codex_app__automation_update"}}
]
```

### 例 2：chat 格式删除工具（同时匹配转换前/后两种结构）

```json
[
  {"path": "tools[*]", "action": "drop", "condition": {"op": "eq", "path": "$item.name", "value": "automation_update"}},
  {"path": "tools[*]", "action": "drop", "condition": {"op": "eq", "path": "$item.function.name", "value": "automation_update"}}
]
```

### 例 3：条件删命名空间工具

```json
[
  {"path": "tools[*]", "action": "drop", "condition": {"all": [
    {"op": "eq", "path": "$item.type", "value": "namespace"},
    {"op": "eq", "path": "$item.name", "value": "image_gen"}
  ]}}
]
```

### 例 4：deepseek-v4 去 thinking + 限 max_completion_tokens（chat 格式）

```json
[
  {"path": "thinking", "action": "drop", "condition": {"op": "starts_with", "path": "model", "value": "deepseek-v4", "source": "original"}},
  {"path": "max_completion_tokens", "value": 32768, "action": "set", "condition": {"op": "starts_with", "path": "model", "value": "deepseek-v4", "source": "original"}}
]
```

### 例 5：删 input 里的 web_search_call item（responses）

```json
[
  {"path": "input[*]", "action": "drop", "condition": {"op": "eq", "path": "$item.type", "value": "web_search_call"}}
]
```

> `input[*]` 通配对 chat 格式同样有效（转换前删除）。

### 例 6：修 BrowserOS neo 工具 enum 含 null（按端点格式 3 种路径）

responses（平铺）：
```json
{"path": "tools[*].parameters.properties.color.enum", "value": ["grey","blue","orange"], "action": "set", "condition": {"op": "eq", "path": "$item.name", "value": "browseros_neo"}}
```

chat（包装）：
```json
{"path": "tools[*].function.parameters.properties.color.enum", "value": ["grey","blue","orange"], "action": "set", "condition": {"op": "eq", "path": "$item.function.name", "value": "browseros_neo"}}
```

claude:messages（input_schema）：
```json
{"path": "tools[*].input_schema.properties.color.enum", "value": ["grey","blue","orange"], "action": "set", "condition": {"op": "eq", "path": "$item.name", "value": "browseros_neo"}}
```

## 三、修改方式（API）

```
PATCH /api/admin/endpoints/{id}
{ "body_rules": [ ...完整规则数组... ] }
```

⚠️ 规则是**整体替换**：改前先 GET 现有 body_rules，全量带上再提交。

## 四、备份与回滚（改任何端点前先做）

```sql
-- 备份（幂等）
CREATE TABLE IF NOT EXISTS provider_endpoints_backup_<日期> AS
SELECT * FROM provider_endpoints WHERE id='<endpoint_id>';

-- 或增量补备份
INSERT INTO provider_endpoints_backup_<日期>
SELECT * FROM provider_endpoints
WHERE id IN ('<id1>','<id2>')
  AND NOT EXISTS (SELECT 1 FROM provider_endpoints_backup_<日期> b WHERE b.id = provider_endpoints.id);

-- 回滚
UPDATE provider_endpoints e
SET body_rules = b.body_rules, header_rules = b.header_rules, updated_at = now()
FROM provider_endpoints_backup_<日期> b
WHERE e.id = b.id AND e.id = '<endpoint_id>';
```

> 自动化备份表 `provider_endpoints_backup_20260815` 是历史备份（勿删）。

## 五、端到端验证

```bash
# input 必须是数组！模型名要真实存在
cat > /tmp/req.json << 'EOF'
{"model":"<模型名>","input":[{"role":"user","content":[{"type":"input_text","text":"hi"}]}],"tools":[{"type":"function","name":"automation_update","parameters":{"type":null}}]}
EOF
curl -sS -w '\nHTTP_CODE:%{http_code} TIME:%{time_total}s\n' --max-time 60 https://<网关域名>/v1/responses \
  -H "Authorization: Bearer <用户key>" -H "Content-Type: application/json" --data @/tmp/req.json
```

- 修复生效标志：HTTP 200 且响应体 `tools` 数组里没有 automation_update；正常工具（如 shell_command）保留
- 验证走哪个端点：查 usage 的 `provider_name` + `provider_endpoint_kind`
