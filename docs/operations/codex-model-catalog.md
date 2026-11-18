# Codex 官方模型目录

`GET /codex/models` 返回随 Aether 发布的完整官方 Codex 模型目录。该入口使用现有 Aether API Key 鉴权及 `openai:responses` 格式权限，响应为 `application/json`。

目录直接来自仓库中的 `apps/aether-gateway/src/handlers/public/support/models/codex_models.json`。请求不依赖上游账号在线状态或候选模型查询，也不按 Aether 模型映射裁剪或改写元数据。实际推理请求仍执行原有模型权限与路由规则。

## 客户端版本

`client_version` 是可选参数，不用于选择或过滤这份目录。以下请求返回相同 JSON：

```text
GET /codex/models
GET /codex/models?client_version=0.169.0
GET /codex/models?client_version=1.0.0
GET /codex/models?client_version=preview
```

官方卡片中的 `minimal_client_version` 等能力字段原样保留；客户端仍可自行判断其功能兼容性。更新快照不需要修改请求版本白名单。

## 配置 Codex

在已有的 provider 表中设置完整目录 URL，并保留原有推理地址与鉴权配置：

```toml
[model_providers.aether]
name = "OpenAI"
base_url = "https://aether.fawney.fun/v1"
model_catalog_url = "https://aether.fawney.fun/codex/models"
wire_api = "responses"
requires_openai_auth = false
env_key = "AETHER_API_KEY"

[features]
api_key_model_discovery = true
```

示例中的 `AETHER_API_KEY` 应由启动 Codex 的环境提供；已有其他鉴权方式时继续使用原配置。移除有效的 `model_catalog_json` 配置，否则本地目录文件会优先于远程 URL。修改客户端配置后重启客户端。

原有 `/v1/models` 普通列表与 `/v1/models?client_version=...` 的动态上游目录保持原有行为；需要固定官方快照时使用新的 `/codex/models`。

## 快照来源与更新

当前快照于 2026-11-18 从官方端点获取：

```text
https://chatgpt.com/backend-api/codex/models?client_version=0.169.0
```

快照包含 12 个模型，包括 `gpt-6.2-sol` 和 `gpt-6.2-astra`。目录保留完整 instructions、推理档位、工具配置、上下文限制及未来字段；不包含获取目录时使用的访问令牌或请求头。

更新时使用获得上游授权的凭据重新获取目录，将完整的 `{"models": [...]}` 响应替换到上述 JSON 文件，然后重新构建和发布 Aether。快照不会自动刷新；获取时使用的版本仅记录数据来源，不限制此入口接受的客户端版本。

协议依据：[官方模型目录实现说明](https://learn.chatgpt.com/docs/enterprise/roll-out-a-gateway#implement-the-catalog-endpoint)。
