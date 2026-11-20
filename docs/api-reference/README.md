# Aether 网关接口参考手册

基于 **fawney19/Aether main**（Rust/axum 实现）代码逐路由核对生成的完整接口文档：管理 API（34 个路由家族）、客户端兼容接口、数据库结构与运维最佳实践。

> ⚠️ 本文档不包含任何真实密钥 / token / 节点信息，示例中的 `<...>` 均为占位符。

## 章节导航

| 章节 | 说明 |
|---|---|
| [架构与部署](./architecture.md) | 组件、路由分组、compose、格式转换 |
| [鉴权机制与管理令牌](./admin-api/01-auth-and-tokens.md) | `ae-` 管理令牌、31 权限域、JWT 会话 |
| [Admin API 总览](./admin-api/00-overview.md) | 34 个路由家族、调用约定、错误格式 |
| [API 密钥管理](./admin-api/02-api-keys.md) | 独立余额 Key CRUD |
| [全局模型管理](./admin-api/03-models.md) | 全局模型与 model_mappings |
| [供应商与端点](./admin-api/04-providers-endpoints.md) | providers/endpoints、body_rules 入口 |
| [用量查询](./admin-api/05-usage.md) | usage records/curl/replay、查库 SQL |
| [其余 Admin 家族（端点级）](./admin-api/06-others.md) | 34 个家族端点表 + 示例（[families/](./admin-api/families/)） |
| [完整路由清单](./admin-api/_route-inventory.md) | 305 路径 / 365 方法×路径（代码提取） |
| [客户端 v1 API](./client-api/v1.md) | OpenAI/Claude/Gemini 兼容接口 |
| [数据库速查](./database.md) | 8 个逻辑 schema 全部表字段 |
| [body_rules 语法](./ops/body-rules.md) | 规则结构与实战示例 |
| [运维排查手册](./ops/playbook.md) | 排查流程与已知问题 |

## 文档维护

- 路由清单由代码提取：`apps/aether-gateway/src/control/route/admin/*.rs` + `handlers/admin/**/routes.rs`
- 数据库字段来源：`crates/aether-data/runtime/schema/logical/*.toml`
- 核对基线：fawney19/Aether main（2026-08）；fork `zbsdsb/Aether` 的 10 个独有提交均为内部修复，不改变管理 API 表面
