# Grok OAuth 订阅账号提供商

`grok_oauth` 用于接入通过 xAI Grok CLI OAuth 获得的订阅账号。它与既有的 `grok`
provider 不同：

| 类型 | 上游 | 凭据 |
| --- | --- | --- |
| `grok` | `https://grok.com` | 浏览器 Cookie / SSO |
| `grok_oauth` | `https://cli-chat-proxy.grok.com/v1` | xAI OAuth refresh token 与 access token |
| `custom` | 由管理员配置 | xAI API Key 或其他兼容 API Key |

## 支持范围

- 管理后台浏览器 OAuth 绑定；
- refresh token 单个导入、批量导入与自动刷新；
- OpenAI Responses 文本与流式请求；
- 通过 Aether 格式转换层兼容 Chat 请求；
- 读取 xAI CLI weekly/monthly Billing 额度，并写入结构化额度快照；
- `grok-4.5` 支持 `low`、`medium`、`high` 思考程度；请求未指定时使用上游模型默认值
  `high`，并在使用记录中保存最终实际值。

上游固定使用 Responses，不会为 `grok_oauth` 直连创建 Chat Completions 端点。图片、
视频、SSO Cookie 转换和媒体资格检查不在此 provider 的支持范围内。

Grok OAuth 默认每 30 分钟以并发 1 执行一次账号自检，从 `/v1/billing?format=credits`
和 `/v1/billing` 更新额度。管理员可以在号池高级设置中调整间隔与并发，或显式关闭账号
自检。管理页会显示额度快照的绝对更新时间；超过 60 分钟未更新时标记为“额度数据已
过期”，该时间与 OAuth Token 续期倒计时无关。

## 使用要求

1. 在管理后台创建类型为 `Grok OAuth` 的 provider。
2. 使用管理后台给出的浏览器 OAuth 链接完成授权，或导入已授权账号的 refresh token。
3. 为 key 配置可用模型与调度策略后，使用 OpenAI Responses API 发起最小文本请求。

OAuth 凭据会按 Aether 既有 OAuth key 机制加密保存。不要在 provider 名称、请求体、日志、
截图或工单中粘贴 access token、refresh token 或完整 callback URL。

## 403 行为

`grok_oauth` 的任意 403（包括响应正文包含 token 相关字样）都可能来自 xAI 风控、地区、
客户端身份或暂态限制，不能作为账号永久失效的依据。Aether 会保留账号并记录脱敏诊断；
401 仍按 OAuth 刷新/失效流程处理。

## 验证与发布

启用前必须通过仓库中的 Grok OAuth 单元、集成、前端检查和获授权测试账号 canary。部署
服务目录中的 `docs/grok-oauth-repair-sop.md` 记录完整实施步骤、验收门槛、停止条件和回滚方式。
