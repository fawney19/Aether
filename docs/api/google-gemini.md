# Google Gemini 接口适配

Aether 使用 Gemini 原生格式接入 AI Studio 和 Vertex AI / Gemini Enterprise Agent Platform。普通生成继续使用 GenerateContent；需要原生 Interactions 的客户端使用独立的 `gemini:interactions` 格式。

## 模型与参数

- Gemini 4 Argon、Carbon、Hydrogen 和 Gemini 3.8 Flash 使用 `thinkingLevel`。跨格式请求的推理强度映射为 `low`、`medium`、`high`；Hydrogen 还支持 `xhigh`，请求的 `xhigh` 和 `max` 均映射为该级别。不再为 Gemini 4 生成 `thinkingBudget`。
- Hydrogen 使用 1,048,576 token 的统一输入与输出窗口，默认单段输出上限为 65,536 token，支持流式和非流式生成。更长输出沿用下文的原生续接方式。
- 跨格式生成请求会去掉这些模型不使用的 sampling 参数及旧的 `candidateCount`。原生请求保持原始参数，由上游执行模型级校验。
- Gemini 2.5 继续使用整数 thinking budget；既有 Gemini 3 模型的行为保持兼容。
- Gemini 3.8 / 4 的 Vertex 工具调用保留 `functionResponse.id`。历史模型仍使用其原有兼容策略。
- 带签名的文本 Part 和没有文本的 thought Part 会保留 `thoughtSignature`，并保持 Part 边界。

## Vertex 认证、地区与版本

| 接口 | API Key | Service Account | 上游路径 |
| --- | --- | --- | --- |
| GenerateContent | 支持 | 支持 | `/v1/.../publishers/google/models/{model}:{action}` |
| Gemini 4 GenerateContent | 支持 | 支持 | `/v1beta1/.../publishers/google/models/{model}:{action}` |
| Embedding | 支持 | 支持 | `/v1/.../publishers/google/models/{model}:predict` |
| Interactions | 不支持 | 支持 | `/v1beta1/projects/{project}/locations/{region}/interactions` |

Gemini 4 默认使用 `global`；显式的 `model_regions` 配置优先。其他模型沿用既有地区解析规则。

客户端 query 中的 API key 不会被转发到服务账号端点。Vertex Interactions 的可选格式、候选筛选、运行时认证与管理界面使用同一服务账号约束。

## 用量与计费

GenerateContent 的 `candidatesTokenCount` 不包含单独返回的 `thoughtsTokenCount`。标准化输出使用两者之和，`reasoning_tokens` 保留为明细；计算总量时不再重复相加。

Interactions 使用 `usage.total_input_tokens`、`total_output_tokens`、`total_thought_tokens`、`total_cached_tokens` 和 `total_tokens`；流式的 `interaction.completed` 事件使用同一映射。计费不根据可见文本长度估算推理消耗。

模态明细保留在请求记录的 `dimensions` 中，同时映射为计费规则可用的标量：

- `input_{text,image,audio,video,document}_tokens`
- `output_{text,image,audio,video,document}_tokens`
- `cache_read_{text,image,audio,video,document}_tokens`

只有上游实际返回的非负整数会进入这些维度。模态维度不会覆盖标准输入/输出 token、缓存 token 或价格。按音频、图像使用不同单价的模型，需要在模型计费规则中配置对应表达式；保留明细本身不会自动推导或替换模型价格。

## 续接与能力边界

Gemini 4 的长输出续接使用 Vertex `v1beta1`。原生 GenerateContent 请求保留 `continuationToken`；客户端负责根据 `CONTINUATION` 和上游续接 token 发起下一段，并保持模型、系统指令和工具声明一致。网关不会自动启动额外生成请求。

Gemini 4 使用隐式缓存，不支持显式 cached content。`gemini-3.8-flash` 是普通生成模型，不能作为 Live 模型使用。

Hydrogen 的 GenerateContent 原生请求保留图片、音频、视频、PDF、Office 和 OpenDocument 文件输入，以及工具调用、结构化输出、thought signature 和续接 token。模型只生成文本，不支持图片生成、语音生成或 Live API。标准价格为每百万输入 token 1.50 美元、输出 token 7.50 美元、缓存读取 token 0.075 美元；免费额度和 Batch、Flex 等折扣不改变管理端配置的标准模型价格。

此适配覆盖现有 GenerateContent、流式生成、Embedding、文件和原生 Interactions 路径。Gemini Live/Bidi、独立 cachedContents 管理以及异步批量资源管理尚无专用协议适配；它们不能通过修改模型名称获得支持。Interactions 也不转换为 OpenAI/Claude 消息格式。

## 官方参考

- [Decisions API](https://aistudio.google.com/docs/decisions-api)
- [Gemini Decision 1](https://aistudio.google.com/docs/models/gemini-decision-1)

- [Gemini 4 入门与迁移](https://docs.cloud.google.com/gemini-enterprise-agent-platform/models/start/get-started-with-gemini-4)
- [Gemini 4 Argon 开发指南](https://docs.cloud.google.com/gemini-enterprise-agent-platform/models/guides/gemini-4-argon)
- [Gemini 4 Hydrogen](https://aistudio.google.com/docs/models/gemini-4-hydrogen)
- [Gemini 4 Hydrogen 开发指南](https://docs.cloud.google.com/gemini-enterprise-agent-platform/models/guides/gemini-4-hydrogen)
- [Gemini 3.8 Flash](https://docs.cloud.google.com/gemini-enterprise-agent-platform/models/gemini/3-8-flash)
- [Interactions API](https://docs.cloud.google.com/gemini-enterprise-agent-platform/reference/models/interactions-api)
- [AI Studio 定价](https://ai.google.dev/gemini-api/docs/pricing)
- [Agent Platform 定价](https://cloud.google.com/model-garden/pricing)

## Decisions API

Gemini Decision 1 使用独立格式 `gemini:decisions`，客户端向 `POST /v1beta/decisions` 发送原生请求。端点可配置为 `https://generativelanguage.googleapis.com/v1beta`，路径为 `/decisions`。该接口目前由 Gemini Developer API 提供。

请求的 `input` 支持文本、图片、音频、视频和 PDF；`questions` 可同时包含 `noul`、`choice` 和 `score` 问题。网关只执行模型映射和已配置的请求规则，完整保留输入块、问题、选项与返回的 `answers`、概率、置信度及评分图例。该接口同步返回 JSON，不转换为聊天生成格式。

```json
{
  "model": "gemini-decision-1",
  "input": "收到的杯子破损了，需要换货。",
  "questions": {
    "damaged": {"type": "noul", "instructions": "商品是否破损？"},
    "queue": {"type": "choice", "instructions": "选择处理队列", "criteria": {"replacement": "破损换货", "billing": "账单问题"}},
    "severity": {"type": "score", "instructions": "评估损坏程度", "criteria": ["完好", "轻微损坏", "无法使用"]}
  }
}
```

用量读取 `usage.total_input_tokens`、`total_output_tokens`、`total_tokens` 和 `input_tokens_by_modality`。共享输入只按上游实际用量计算一次，不按问题数重复计费。标准价格为每百万输入 token 0.10 美元，输出 0 美元；模型价格通过现有管理配置设置。管理端支持端点配置、格式权限、原生模型测试和判断结果预览。
