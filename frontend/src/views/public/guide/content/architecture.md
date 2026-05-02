# 架构说明

Aether的系统架构、请求处理流程和数据流向。

## 1. 系统概览

```mermaid
graph LR
    Client[客户端<br>SDK / CLI / Web]
    Aether[Aether<br>认证 / 路由 / 编排]
    PostgreSQL[(PostgreSQL)]
    Redis[(Redis)]
    Upstream[上游供应商<br>Claude / OpenAI / Gemini]

    Client -->|API Request| Aether
    Aether -.->|Auth / Config| PostgreSQL
    Aether <.-.>|Cache / Quota / Lock| Redis
    Aether -->|Proxy Request| Upstream
```

## 2. 核心原则

1. **API格式、端点、认证方式说明**
   提供商支持不同的格式（如 OpenAI Chat, Claude Messages），Aether在接收请求后，将统一管理路由。

2. **统一的入口模型名称**
   在内部完成多提供商、多模型名称风格的聚合映射管理。客户端只需知道统一的模型名（例如 `claude-3-opus`），Aether将根据映射规则自动寻找真正对应的上游模型名称。

3. **请求流转：格式转换**
   `多API格式兼容入口` → `格式转换` → `上游提供商` → `格式转换` → `多API格式兼容出口`。
   Aether 会尝试分析客户端的请求体格式，再将其转换为对应上游供应商要求的真正格式。

4. **请求流转：透传模式**
   `多API格式入口` → `同格式请求透传` → `上游提供商` → `同格式响应透传` → `多API格式出口`。
   如果客户端请求格式与上游目标格式一致，Aether 会直接透传（Pass-through），不进行数据解包和重整，最大化性能。
