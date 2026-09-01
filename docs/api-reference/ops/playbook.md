# 运维排查手册（已知问题库）

> 来源：`aether-gateway-ops` skill 生产排查记录（2026-08 期间）
> 排查主流程：**先查 usage → 取 trace_id 查日志 → 解 body 确认根因 → 备份 → 改配置 → 端到端验证**

## 排查流程模板

1. **拿报错/现象** → 查 usage（近 2 小时，按 status_code/error_message）定位 provider/endpoint/错误分类
2. **取 trace_id/request_id** → grep 网关日志看完整候选链（`local_sync_candidate_retry_scheduled` 逐条列出候选）
3. **看请求/响应体** → `usage_body_blobs` 解压
4. **确认根因** → 备份 → 改配置（body_rules/模型映射/enabled）→ 端到端验证 → 收尾清理临时文件

```bash
# 1. 最近请求记录
docker exec aether-postgres psql -U postgres -d aether -P pager=off -c \
"select created_at,request_id,provider_name,model,api_format,provider_endpoint_kind,status_code,status,error_category,left(error_message,200) as error,response_time_ms from usage where created_at > now() - interval '2 hours' order by created_at desc limit 30;"

# 2. 网关日志
grep -a '<trace_id>' /opt/aether/logs/aether-gateway.$(date +%F).log
grep -aE 'WARN|ERROR' /opt/aether/logs/aether-gateway.$(date +%F).log | tail -50

# 3. 候选路由（走节点还是直连）
docker exec aether-postgres psql -U postgres -d aether -P pager=off -c \
"select request_id,provider_name,endpoint_id,extra_data from request_candidates where request_id='<id>' order by candidate_index;"
```

## 已知问题库

### 1. Codex `automation_update` schema 400（已修复 2026-08-15）

- **现象**：Codex（0.147.0，wire_api=responses）任何请求报 `Invalid schema for function 'automation_update': schema must be a JSON Schema of 'type: "object"', got 'type: null'`，错误来自严格校验的上游渠道。
- **根因**：Codex 内建线程自动化工具 `automation_update` 强制附带（eager-loaded），其 JSON Schema 生成有 bug（根级 oneOf + type null）。严格校验上游（如 DeepSeek 官方）直接 400 整单拒绝。官方 issue: openai/codex#37786、#36441（0.147.0 仍存在，无关闭开关）。
- **修复**：body_rules 删除规则（body-rules 例 1/2），加到对应渠道的 responses/chat 端点（按端点加 body_rules）。
- **影响**：仅"对话里管理自动化配置"不可用；定时任务正常（本地 Codex 调度，走普通工具脚本）。
- **备份**：`provider_endpoints_backup_20260815`。

### 2. 流式首字节超时 502（tunnel 节点 bug，2026-08-15）

- **现象**：上游 200 且首字节已返回（如 13.75s），但 30s 整被掐 502，报 "upstream response body timeout"（phase=stream_read）。
- **根因**：节点 agent `aether-tunnel 0.3.12` 无流式豁免，把 `timeout_secs`（流式首字节超时配置）当整个响应体 deadline；上游 200+28 字节后无数据 → 30s 掐断。
- **修复**：tunnel v0.3.13 修复，**v0.3.16 为最终版（协议 v3，与网关 0.7.21 匹配）**；0.3.12 是协议 v2。
- **判定技巧**：`request_candidates.extra_data->'proxy'->>'node_name'` 区分走节点(local_tunnel) vs 直连(DIRECT/cached_affinity)；直连请求不受节点影响可长跑成功。
- **证据位置**：网关日志 `stream_pump_body_read_error` / `stream_execution_error_frame`；usage.error_message 精确 30.15s；节点 `journalctl -u aether-tunnel`。

### 3. 模型映射（gpt-5.4 → gpt-5.6-luna，2026-08-14）

- 停用全局模型 `is_active=false` 后，入站旧模型名靠 `config.model_mappings` 正则兜底路由：`gpt-5.6-luna` 的 config `model_mappings=["gpt5\\.4","gpt-5\\.4"]`。
- 原理：入站模型名先精确匹配全局模型名（候选 SQL 有 `COALESCE(gm.is_active,1)=1` 过滤），停用后精确匹配落空 → mapping 正则兜底。
- 管理 API：`GET/PATCH /api/admin/models/global/{id}`（PATCH 部分更新，config 整体替换）；测试 key：`POST /api/admin/api-keys`。

### 4. Provider key 管理决策（v0.7.19 验收）

- key 多选与启用/停用状态无关：disabled key 也可勾选、保存、取消；禁用/启用不得丢失 scope 引用；运行时只消费 active key；删除 key 仍需 prune 引用。

### 5. 候选列表其他端点状态（2026-08-15 观察）

- Aether 路由是**候选逐个尝试**：按 priority+health 排序，失败自动跳下一个（日志 `local_sync_candidate_retry_scheduled`）。**流式请求首个候选 400 会直接返回给客户端（不兜底）**。
- 部分上游端点可能因渠道自身问题（403/503/530/超时）失败，属渠道侧问题而非网关缺陷。
- **没有全局 body_rules 机制**（system_configs 无规则项、routing_groups 为空），规则必须按端点配置。

### 6. Codex `web_search_call` item 导致严格上游 400（已修复 2026-08-15）

- **现象**：Codex 会话执行过 web 搜索后，该会话后续所有请求报 503，usage 错误为 `Failed to deserialize the JSON body into the target type: input: invalid type: string "search", expected internally tagged enum WebSearchAction`。
- **根因**：Codex 把 `web_search_call` 项写进 input 历史（`{"type":"web_search_call","id":"...","status":"completed","action":"search"}`），其 `action` 是字符串；严格上游要求 `action` 为对象（WebSearchAction enum）→ 整单 400。会话里 2 个 web_search_call 后每个请求都带上它们，持续 400。
- **判定技巧**：对比成功/失败请求的 `usage_body_blobs.request_body` input item 类型分布（python Counter），失败请求多出 `web_search_call`。
- **修复**：body_rules 追加删除规则（body-rules 例 5）。验证：重放原失败 body（313KB，17 工具 + effort=max + stream=true）→ HTTP 200 SSE 正常。

### 7. 上游端点死链（已停用）

- **现象**：某反代端点 base_url 指向不存在的内部服务，每次候选命中白等 ~5s DNS 失败（execution_runtime_unavailable）。
- **修复**：3 个端点 `enabled=false`（端点 id 略）。恢复需先重新部署 ds2api 到同一 docker 网络。

### 8. BrowserOS neo 工具 enum 带 null → Gemini 400（已修复 2026-08-15）

- **现象**：带 browseros_neo 工具的请求在 Gemini 系渠道全部 400：`GenerateContentRequest.tools[0].function_declarations[31].parameters.properties[color].enum[9]: cannot be empty`。
- **根因**：neo MCP 服务端（browseros-claw-server.exe，127.0.0.1:9010）为可选 enum 参数生成 `enum: [..., null]`；转 Gemini function_declarations 时 null → 空串，Google 拒绝空 enum 项。
- **修复**：body_rules set 规则（body-rules 例 6），按端点格式 3 种路径，按端点格式 3 种路径加到对应端点。
- **注意**：body_rules 在转换为 provider 格式之后应用；未来 neo 服务端新增带 null enum 的属性需补规则；OpenAI 系渠道不转 Gemini 不受影响。非 Gemini 模型实测不受影响（claude-opus-4-6、grok-4.5 正常）。

## 铁律（运维强制）

1. 所有服务器操作走 ssh-skill 脚本，禁止裸 ssh/scp
2. 改配置前必须备份（见 body-rules 章节备份/回滚）
3. 只读排查优先：先查 usage/日志/表确认根因，再动手改
4. **不暴露密钥**：token/API key 不得写入对话、日志或文档；查询用 `key_prefix`、`left(...)` 截断
5. Git Bash 上传路径用 `MSYS_NO_PATHCONV=1` 前缀
6. JSON 写入用文件方式：写本地文件 → ssh_upload → `cat 文件 | docker exec -i ... psql`，避免多层 shell 转义破坏 `$` 和引号
7. **测试请求 body 必须是合法结构**：Aether `/v1/responses` 要求 `input` 为数组，字符串会 503 "没有匹配到可用的执行路径"
