# OpenAI / Claude / Gemini 接口定义

生成日期：2026-06-03。

本文档整理 Aether 当前接入和转换矩阵实际涉及的三类 provider 接口面：OpenAI Chat Completions / Responses / Embeddings / Images，Claude Messages，以及 Gemini GenerateContent / Embeddings / Files / PredictLongRunning 相关接口。它不是三家公司所有管理类、训练类、账单类 API 的全集。

这是 schema inventory / audit input，不是运行时代码的字段 allowlist。Provider 官方新增字段时，同格式运行时路径仍按原始 body 透传；canonical same-format roundtrip 通过 provider extension 保留未映射字段；跨格式转换只有在存在显式语义映射时才开放，否则 fail closed。刷新本文档只用于更新审计基线和决定是否新增跨格式映射。

## 来源与范围

| Provider | 结构化来源 | 官方参考 | 本文档覆盖 |
| --- | --- | --- | --- |
| OpenAI | OpenAI OpenAPI `2.3.0` / `OpenAI API` | https://platform.openai.com/docs/api-reference | `/v1/chat/completions`, `/v1/responses`, `/v1/responses/compact`, `/v1/embeddings`, `/v1/images/*` |
| Claude / Anthropic | `anthropic-sdk-typescript` 中由 Anthropic OpenAPI 生成的 `messages.ts` | https://docs.anthropic.com/en/api/messages | `/v1/messages`, `/v1/messages/count_tokens`, Messages streaming events |
| Gemini | Google Generative Language Discovery `v1beta` / `Gemini API` | https://ai.google.dev/api | `generateContent`, `streamGenerateContent`, `embedContent`, `batchEmbedContents`, files, count tokens, predict long-running |

结构化来源 URL：

- OpenAI OpenAPI: https://app.stainless.com/api/spec/documented/openai/openapi.documented.yml
- Anthropic Messages SDK types: https://github.com/anthropics/anthropic-sdk-typescript/blob/main/src/resources/messages/messages.ts
- Gemini Discovery JSON: https://generativelanguage.googleapis.com/$discovery/rest?version=v1beta

说明：字段表中的“必填”来自官方 schema 的 `required` 或 TypeScript `?` 标记；很多接口还会受到模型、账号权限、beta header、区域、Aether provider 配置和上游版本的约束。Aether 的 `/v1/rerank` 是 OpenAI/Jina compatible 兼容面，不是 OpenAI 官方 OpenAPI 中的 endpoint；它见 `docs/api/rerank.md`。

## Aether API Format 对应关系

| Aether format | Provider 原生接口 | 请求根 schema | 响应根 schema |
| --- | --- | --- | --- |
| `openai:chat` | `POST /v1/chat/completions` | `CreateChatCompletionRequest` | `CreateChatCompletionResponse` 或 `CreateChatCompletionStreamResponse` |
| `openai:responses` | `POST /v1/responses` | `CreateResponse` | `Response` 或 `ResponseStreamEvent` |
| `openai:responses:compact` | `POST /v1/responses/compact` | `CompactResponseMethodPublicBody` | `CompactResource` |
| `openai:embedding` | `POST /v1/embeddings` | `CreateEmbeddingRequest` | `CreateEmbeddingResponse` |
| `openai:image` | `POST /v1/images/generations`, `/edits`, `/variations` | `CreateImageRequest`, `CreateImageEditRequest`, `CreateImageVariationRequest` | `ImagesResponse` 或 image stream event |
| `claude:messages` | `POST /v1/messages` | `MessageCreateParams` | `Message` 或 `RawMessageStreamEvent` |
| `gemini:generate_content` | `models/{model}:generateContent` / `:streamGenerateContent` | `GenerateContentRequest` | `GenerateContentResponse` |
| `gemini:embedding` | `models/{model}:embedContent` / `:batchEmbedContents` | `EmbedContentRequest` / `BatchEmbedContentsRequest` | `EmbedContentResponse` / `BatchEmbedContentsResponse` |

## OpenAI Endpoints

| Method | Path | Request content type | Request schema | Response schema |
| --- | --- | --- | --- | --- |
| POST | `/chat/completions` | `application/json` | `CreateChatCompletionRequest` | `CreateChatCompletionResponse` / `CreateChatCompletionStreamResponse` |
| POST | `/responses` | `application/json` | `CreateResponse` | `Response` / `ResponseStreamEvent` |
| POST | `/responses/compact` | `application/json, application/x-www-form-urlencoded` | `CompactResponseMethodPublicBody` | `CompactResource` |
| POST | `/embeddings` | `application/json` | `CreateEmbeddingRequest` | `CreateEmbeddingResponse` |
| POST | `/images/generations` | `application/json` | `CreateImageRequest` | `ImagesResponse` / `ImageGenStreamEvent` |
| POST | `/images/edits` | `multipart/form-data, application/json` | `CreateImageEditRequest` / `EditImageBodyJsonParam` | `ImagesResponse` / `ImageEditStreamEvent` |
| POST | `/images/variations` | `multipart/form-data` | `CreateImageVariationRequest` | `ImagesResponse` |

## OpenAI Schema 字段表

以下 schema 从上述 OpenAI endpoint 根 schema 递归引用得到，共 351 个。

### `AdditionalTools`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | - |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `id` | 是 | `string` | - | The unique ID of the additional tools item. |
| `role` | 是 | `MessageRole` | - | The role that provided the additional tools. |
| `tools` | 是 | `array<Tool>` | - | The additional tool definitions made available at this item. |
| `type` | 是 | `string` | `additional_tools` | The type of the item. Always additional_tools. |

### `AdditionalToolsItemParam`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | - |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `id` | 否 | `string \| null` | - | - |
| `role` | 是 | `string` | `developer` | The role that provided the additional tools. Only developer is supported. |
| `tools` | 是 | `array<Tool>` | - | A list of additional tools made available at this item. |
| `type` | 是 | `string` | `additional_tools` | The item type. Always additional_tools. |

### `Annotation`

| 项 | 值 |
| --- | --- |
| 类型 | `FileCitationBody \| UrlCitationBody \| ContainerFileCitationBody \| FilePath` |
| 说明 | An annotation that applies to a span of output text. |
| 组合 | `oneOf` |

| 变体 | 类型 | 说明 |
| --- | --- | --- |
| 1 | `FileCitationBody` | - |
| 2 | `UrlCitationBody` | - |
| 3 | `ContainerFileCitationBody` | - |
| 4 | `FilePath` | - |

### `ApplyPatchCallOutputStatus`

| 项 | 值 |
| --- | --- |
| 类型 | `string` |
| 说明 | - |

### `ApplyPatchCallOutputStatusParam`

| 项 | 值 |
| --- | --- |
| 类型 | `string` |
| 说明 | Outcome values reported for apply_patch tool call outputs. |

### `ApplyPatchCallStatus`

| 项 | 值 |
| --- | --- |
| 类型 | `string` |
| 说明 | - |

### `ApplyPatchCallStatusParam`

| 项 | 值 |
| --- | --- |
| 类型 | `string` |
| 说明 | Status values reported for apply_patch tool calls. |

### `ApplyPatchCreateFileOperation`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Instruction describing how to create a file via the apply_patch tool. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `diff` | 是 | `string` | - | Diff to apply. |
| `path` | 是 | `string` | - | Path of the file to create. |
| `type` | 是 | `string` | `create_file` | Create a new file with the provided diff. |

### `ApplyPatchCreateFileOperationParam`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Instruction for creating a new file via the apply_patch tool. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `diff` | 是 | `string` | - | Unified diff content to apply when creating the file. |
| `path` | 是 | `string` | - | Path of the file to create relative to the workspace root. |
| `type` | 是 | `string` | `create_file` | The operation type. Always create_file. |

### `ApplyPatchDeleteFileOperation`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Instruction describing how to delete a file via the apply_patch tool. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `path` | 是 | `string` | - | Path of the file to delete. |
| `type` | 是 | `string` | `delete_file` | Delete the specified file. |

### `ApplyPatchDeleteFileOperationParam`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Instruction for deleting an existing file via the apply_patch tool. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `path` | 是 | `string` | - | Path of the file to delete relative to the workspace root. |
| `type` | 是 | `string` | `delete_file` | The operation type. Always delete_file. |

### `ApplyPatchOperationParam`

| 项 | 值 |
| --- | --- |
| 类型 | `ApplyPatchCreateFileOperationParam \| ApplyPatchDeleteFileOperationParam \| ApplyPatchUpdateFileOperationParam` |
| 说明 | One of the create_file, delete_file, or update_file operations supplied to the apply_patch tool. |
| 组合 | `oneOf` |

| 变体 | 类型 | 说明 |
| --- | --- | --- |
| 1 | `ApplyPatchCreateFileOperationParam` | - |
| 2 | `ApplyPatchDeleteFileOperationParam` | - |
| 3 | `ApplyPatchUpdateFileOperationParam` | - |

### `ApplyPatchToolCall`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A tool call that applies file diffs by creating, deleting, or updating files. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `call_id` | 是 | `string` | - | The unique ID of the apply patch tool call generated by the model. |
| `created_by` | 否 | `string` | - | The ID of the entity that created this tool call. |
| `id` | 是 | `string` | - | The unique ID of the apply patch tool call. Populated when this item is returned via API. |
| `operation` | 是 | `ApplyPatchCreateFileOperation \| ApplyPatchDeleteFileOperation \| ApplyPatchUpdateFileOperation` | - | One of the create_file, delete_file, or update_file operations applied via apply_patch. |
| `status` | 是 | `ApplyPatchCallStatus` | - | The status of the apply patch tool call. One of in_progress or completed. |
| `type` | 是 | `string` | `apply_patch_call` | The type of the item. Always apply_patch_call. |

### `ApplyPatchToolCallItemParam`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A tool call representing a request to create, delete, or update files using diff patches. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `call_id` | 是 | `string` | - | The unique ID of the apply patch tool call generated by the model. |
| `id` | 否 | `string \| null` | - | - |
| `operation` | 是 | `ApplyPatchOperationParam` | - | The specific create, delete, or update instruction for the apply_patch tool call. |
| `status` | 是 | `ApplyPatchCallStatusParam` | - | The status of the apply patch tool call. One of in_progress or completed. |
| `type` | 是 | `string` | `apply_patch_call` | The type of the item. Always apply_patch_call. |

### `ApplyPatchToolCallOutput`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | The output emitted by an apply patch tool call. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `call_id` | 是 | `string` | - | The unique ID of the apply patch tool call generated by the model. |
| `created_by` | 否 | `string` | - | The ID of the entity that created this tool call output. |
| `id` | 是 | `string` | - | The unique ID of the apply patch tool call output. Populated when this item is returned via API. |
| `output` | 否 | `string \| null` | - | - |
| `status` | 是 | `ApplyPatchCallOutputStatus` | - | The status of the apply patch tool call output. One of completed or failed. |
| `type` | 是 | `string` | `apply_patch_call_output` | The type of the item. Always apply_patch_call_output. |

### `ApplyPatchToolCallOutputItemParam`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | The streamed output emitted by an apply patch tool call. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `call_id` | 是 | `string` | - | The unique ID of the apply patch tool call generated by the model. |
| `id` | 否 | `string \| null` | - | - |
| `output` | 否 | `string \| null` | - | - |
| `status` | 是 | `ApplyPatchCallOutputStatusParam` | - | The status of the apply patch tool call output. One of completed or failed. |
| `type` | 是 | `string` | `apply_patch_call_output` | The type of the item. Always apply_patch_call_output. |

### `ApplyPatchToolParam`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Allows the assistant to create, delete, or update files using unified diffs. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `type` | 是 | `string` | `apply_patch` | The type of the tool. Always apply_patch. |

### `ApplyPatchUpdateFileOperation`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Instruction describing how to update a file via the apply_patch tool. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `diff` | 是 | `string` | - | Diff to apply. |
| `path` | 是 | `string` | - | Path of the file to update. |
| `type` | 是 | `string` | `update_file` | Update an existing file with the provided diff. |

### `ApplyPatchUpdateFileOperationParam`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Instruction for updating an existing file via the apply_patch tool. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `diff` | 是 | `string` | - | Unified diff content to apply to the existing file. |
| `path` | 是 | `string` | - | Path of the file to update relative to the workspace root. |
| `type` | 是 | `string` | `update_file` | The operation type. Always update_file. |

### `ApproximateLocation`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | - |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `city` | 否 | `string \| null` | - | - |
| `country` | 否 | `string \| null` | - | - |
| `region` | 否 | `string \| null` | - | - |
| `timezone` | 否 | `string \| null` | - | - |
| `type` | 是 | `string` | `approximate` | The type of location approximation. Always approximate. |

### `AutoCodeInterpreterToolParam`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Configuration for a code interpreter container. Optionally specify the IDs of the files to run the code on. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `file_ids` | 否 | `array<string>` | - | An optional list of uploaded files to make available to your code. |
| `memory_limit` | 否 | `ContainerMemoryLimit \| null` | - | - |
| `network_policy` | 否 | `ContainerNetworkPolicyDisabledParam \| ContainerNetworkPolicyAllowlistParam` | - | Network access policy for the container. |
| `type` | 是 | `string` | `auto` | Always auto. |

### `ChatCompletionAllowedTools`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Constrains the tools available to the model to a pre-defined set. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `mode` | 是 | `string` | `auto`, `required` | Constrains the tools available to the model to a pre-defined set. auto allows the model to pick from among the allowed tools and generate a message. required requires the model to… |
| `tools` | 是 | `array<object>` | - | A list of tool definitions that the model should be allowed to call. For the Chat Completions API, the list of tool definitions might look like: |

### `ChatCompletionAllowedToolsChoice`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Constrains the tools available to the model to a pre-defined set. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `allowed_tools` | 是 | `ChatCompletionAllowedTools` | - | - |
| `type` | 是 | `string` | `allowed_tools` | Allowed tool configuration type. Always allowed_tools. |

### `ChatCompletionFunctionCallOption`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Specifying a particular function via {"name": "my_function"} forces the model to call that function. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `name` | 是 | `string` | - | The name of the function to call. |

### `ChatCompletionFunctions`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | - |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `description` | 否 | `string` | - | A description of what the function does, used by the model to choose when and how to call the function. |
| `name` | 是 | `string` | - | The name of the function to be called. Must be a-z, A-Z, 0-9, or contain underscores and dashes, with a maximum length of 64. |
| `parameters` | 否 | `FunctionParameters` | - | - |

### `ChatCompletionMessageCustomToolCall`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A call to a custom tool created by the model. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `custom` | 是 | `object` | - | The custom tool that the model called. |
| `id` | 是 | `string` | - | The ID of the tool call. |
| `type` | 是 | `string` | `custom` | The type of the tool. Always custom. |

### `ChatCompletionMessageToolCall`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A call to a function tool created by the model. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `function` | 是 | `object` | - | The function that the model called. |
| `id` | 是 | `string` | - | The ID of the tool call. |
| `type` | 是 | `string` | `function` | The type of the tool. Currently, only function is supported. |

### `ChatCompletionMessageToolCallChunk`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | - |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `function` | 否 | `object` | - | - |
| `id` | 否 | `string` | - | The ID of the tool call. |
| `index` | 是 | `integer` | - | - |
| `type` | 否 | `string` | `function` | The type of the tool. Currently, only function is supported. |

### `ChatCompletionMessageToolCalls`

| 项 | 值 |
| --- | --- |
| 类型 | `array<ChatCompletionMessageToolCall \| ChatCompletionMessageCustomToolCall>` |
| 说明 | The tool calls generated by the model, such as function calls. |

### `ChatCompletionNamedToolChoice`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Specifies a tool the model should use. Use to force the model to call a specific function. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `function` | 是 | `object` | - | - |
| `type` | 是 | `string` | `function` | For function calling, the type is always function. |

### `ChatCompletionNamedToolChoiceCustom`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Specifies a tool the model should use. Use to force the model to call a specific custom tool. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `custom` | 是 | `object` | - | - |
| `type` | 是 | `string` | `custom` | For custom tool calling, the type is always custom. |

### `ChatCompletionRequestAssistantMessage`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Messages sent by the model in response to user messages. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `audio` | 否 | `object \| null` | - | - |
| `content` | 否 | `string \| array<ChatCompletionRequestAssistantMessageContentPart> \| null` | - | - |
| `function_call` | 否 | `object \| null` | - | - |
| `name` | 否 | `string` | - | An optional name for the participant. Provides the model information to differentiate between participants of the same role. |
| `refusal` | 否 | `string \| null` | - | - |
| `role` | 是 | `string` | `assistant` | The role of the messages author, in this case assistant. |
| `tool_calls` | 否 | `ChatCompletionMessageToolCalls` | - | - |

### `ChatCompletionRequestAssistantMessageContentPart`

| 项 | 值 |
| --- | --- |
| 类型 | `ChatCompletionRequestMessageContentPartText \| ChatCompletionRequestMessageContentPartRefusal` |
| 说明 | - |
| 组合 | `oneOf` |

| 变体 | 类型 | 说明 |
| --- | --- | --- |
| 1 | `ChatCompletionRequestMessageContentPartText` | - |
| 2 | `ChatCompletionRequestMessageContentPartRefusal` | - |

### `ChatCompletionRequestDeveloperMessage`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Developer-provided instructions that the model should follow, regardless of messages sent by the user. With o1 models and newer, developer messages replace the previous system mes… |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `content` | 是 | `string \| array<ChatCompletionRequestMessageContentPartText>` | - | The contents of the developer message. |
| `name` | 否 | `string` | - | An optional name for the participant. Provides the model information to differentiate between participants of the same role. |
| `role` | 是 | `string` | `developer` | The role of the messages author, in this case developer. |

### `ChatCompletionRequestFunctionMessage`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | - |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `content` | 是 | `string \| null` | - | - |
| `name` | 是 | `string` | - | The name of the function to call. |
| `role` | 是 | `string` | `function` | The role of the messages author, in this case function. |

### `ChatCompletionRequestMessage`

| 项 | 值 |
| --- | --- |
| 类型 | `ChatCompletionRequestDeveloperMessage \| ChatCompletionRequestSystemMessage \| ChatCompletionRequestUserMessage \| ChatCompletionRequestAssistantMessage \| ChatCompletionRequestToolMessage \| ChatCompletionRequestFunctionMessage` |
| 说明 | - |
| 组合 | `oneOf` |

| 变体 | 类型 | 说明 |
| --- | --- | --- |
| 1 | `ChatCompletionRequestDeveloperMessage` | - |
| 2 | `ChatCompletionRequestSystemMessage` | - |
| 3 | `ChatCompletionRequestUserMessage` | - |
| 4 | `ChatCompletionRequestAssistantMessage` | - |
| 5 | `ChatCompletionRequestToolMessage` | - |
| 6 | `ChatCompletionRequestFunctionMessage` | - |

### `ChatCompletionRequestMessageContentPartAudio`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Learn about [audio inputs](/docs/guides/audio). |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `input_audio` | 是 | `object` | - | - |
| `type` | 是 | `string` | `input_audio` | The type of the content part. Always input_audio. |

### `ChatCompletionRequestMessageContentPartFile`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Learn about [file inputs](/docs/guides/text) for text generation. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `file` | 是 | `object` | - | - |
| `type` | 是 | `string` | `file` | The type of the content part. Always file. |

### `ChatCompletionRequestMessageContentPartImage`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Learn about [image inputs](/docs/guides/vision). |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `image_url` | 是 | `object` | - | - |
| `type` | 是 | `string` | `image_url` | The type of the content part. |

### `ChatCompletionRequestMessageContentPartRefusal`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | - |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `refusal` | 是 | `string` | - | The refusal message generated by the model. |
| `type` | 是 | `string` | `refusal` | The type of the content part. |

### `ChatCompletionRequestMessageContentPartText`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Learn about [text inputs](/docs/guides/text-generation). |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `text` | 是 | `string` | - | The text content. |
| `type` | 是 | `string` | `text` | The type of the content part. |

### `ChatCompletionRequestSystemMessage`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Developer-provided instructions that the model should follow, regardless of messages sent by the user. With o1 models and newer, use developer messages for this purpose instead. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `content` | 是 | `string \| array<ChatCompletionRequestSystemMessageContentPart>` | - | The contents of the system message. |
| `name` | 否 | `string` | - | An optional name for the participant. Provides the model information to differentiate between participants of the same role. |
| `role` | 是 | `string` | `system` | The role of the messages author, in this case system. |

### `ChatCompletionRequestSystemMessageContentPart`

| 项 | 值 |
| --- | --- |
| 类型 | `ChatCompletionRequestMessageContentPartText` |
| 说明 | - |
| 组合 | `oneOf` |

| 变体 | 类型 | 说明 |
| --- | --- | --- |
| 1 | `ChatCompletionRequestMessageContentPartText` | - |

### `ChatCompletionRequestToolMessage`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | - |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `content` | 是 | `string \| array<ChatCompletionRequestToolMessageContentPart>` | - | The contents of the tool message. |
| `role` | 是 | `string` | `tool` | The role of the messages author, in this case tool. |
| `tool_call_id` | 是 | `string` | - | Tool call that this message is responding to. |

### `ChatCompletionRequestToolMessageContentPart`

| 项 | 值 |
| --- | --- |
| 类型 | `ChatCompletionRequestMessageContentPartText` |
| 说明 | - |
| 组合 | `oneOf` |

| 变体 | 类型 | 说明 |
| --- | --- | --- |
| 1 | `ChatCompletionRequestMessageContentPartText` | - |

### `ChatCompletionRequestUserMessage`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Messages sent by an end user, containing prompts or additional context information. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `content` | 是 | `string \| array<ChatCompletionRequestUserMessageContentPart>` | - | The contents of the user message. |
| `name` | 否 | `string` | - | An optional name for the participant. Provides the model information to differentiate between participants of the same role. |
| `role` | 是 | `string` | `user` | The role of the messages author, in this case user. |

### `ChatCompletionRequestUserMessageContentPart`

| 项 | 值 |
| --- | --- |
| 类型 | `ChatCompletionRequestMessageContentPartText \| ChatCompletionRequestMessageContentPartImage \| ChatCompletionRequestMessageContentPartAudio \| ChatCompletionRequestMessageContentPartFile` |
| 说明 | - |
| 组合 | `oneOf` |

| 变体 | 类型 | 说明 |
| --- | --- | --- |
| 1 | `ChatCompletionRequestMessageContentPartText` | - |
| 2 | `ChatCompletionRequestMessageContentPartImage` | - |
| 3 | `ChatCompletionRequestMessageContentPartAudio` | - |
| 4 | `ChatCompletionRequestMessageContentPartFile` | - |

### `ChatCompletionResponseMessage`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A chat completion message generated by the model. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `annotations` | 否 | `array<object>` | - | Annotations for the message, when applicable, as when using the [web search tool](/docs/guides/tools-web-search?api-mode=chat). |
| `audio` | 否 | `object \| null` | - | - |
| `content` | 是 | `string \| null` | - | - |
| `function_call` | 否 | `object` | - | Deprecated and replaced by tool_calls. The name and arguments of a function that should be called, as generated by the model. |
| `refusal` | 是 | `string \| null` | - | - |
| `role` | 是 | `string` | `assistant` | The role of the author of this message. |
| `tool_calls` | 否 | `ChatCompletionMessageToolCalls` | - | - |

### `ChatCompletionStreamOptions`

| 项 | 值 |
| --- | --- |
| 类型 | `object \| null` |
| 说明 | - |
| 组合 | `anyOf` |

| 变体 | 类型 | 说明 |
| --- | --- | --- |
| 1 | `object` | Options for streaming response. Only set this when you set stream: true. |
| 2 | `null` | - |

### `ChatCompletionStreamResponseDelta`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A chat completion delta generated by streamed model responses. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `content` | 否 | `string \| null` | - | - |
| `function_call` | 否 | `object` | - | Deprecated and replaced by tool_calls. The name and arguments of a function that should be called, as generated by the model. |
| `refusal` | 否 | `string \| null` | - | - |
| `role` | 否 | `string` | `developer`, `system`, `user`, `assistant`, `tool` | The role of the author of this message. |
| `tool_calls` | 否 | `array<ChatCompletionMessageToolCallChunk>` | - | - |

### `ChatCompletionTokenLogprob`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | - |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `bytes` | 是 | `array<integer> \| null` | - | - |
| `logprob` | 是 | `number` | - | The log probability of this token, if it is within the top 20 most likely tokens. Otherwise, the value -9999.0 is used to signify that the token is very unlikely. |
| `token` | 是 | `string` | - | The token. |
| `top_logprobs` | 是 | `array<object>` | - | List of the most likely tokens and their log probability, at this token position. The number of entries may be fewer than the requested top_logprobs. |

### `ChatCompletionTool`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A function tool that can be used to generate a response. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `function` | 是 | `FunctionObject` | - | - |
| `type` | 是 | `string` | `function` | The type of the tool. Currently, only function is supported. |

### `ChatCompletionToolChoiceOption`

| 项 | 值 |
| --- | --- |
| 类型 | `string \| ChatCompletionAllowedToolsChoice \| ChatCompletionNamedToolChoice \| ChatCompletionNamedToolChoiceCustom` |
| 说明 | Controls which (if any) tool is called by the model. none means the model will not call any tool and instead generates a message. auto means the model can pick between generating … |
| 组合 | `oneOf` |

| 变体 | 类型 | 说明 |
| --- | --- | --- |
| 1 | `string` | none means the model will not call any tool and instead generates a message. auto means the model can pick between generating a message or calling one or more tools. required mean… |
| 2 | `ChatCompletionAllowedToolsChoice` | - |
| 3 | `ChatCompletionNamedToolChoice` | - |
| 4 | `ChatCompletionNamedToolChoiceCustom` | - |

### `ClickButtonType`

| 项 | 值 |
| --- | --- |
| 类型 | `string` |
| 说明 | - |

### `ClickParam`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A click action. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `button` | 是 | `ClickButtonType` | - | Indicates which mouse button was pressed during the click. One of left, right, wheel, back, or forward. |
| `keys` | 否 | `array<string> \| null` | - | - |
| `type` | 是 | `string` | `click` | Specifies the event type. For a click action, this property is always click. |
| `x` | 是 | `integer` | - | The x-coordinate where the click occurred. |
| `y` | 是 | `integer` | - | The y-coordinate where the click occurred. |

### `CodeInterpreterOutputImage`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | The image output from the code interpreter. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `type` | 是 | `string` | `image` | The type of the output. Always image. |
| `url` | 是 | `string(uri)` | - | The URL of the image output from the code interpreter. |

### `CodeInterpreterOutputLogs`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | The logs output from the code interpreter. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `logs` | 是 | `string` | - | The logs output from the code interpreter. |
| `type` | 是 | `string` | `logs` | The type of the output. Always logs. |

### `CodeInterpreterTool`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A tool that runs Python code to help generate a response to a prompt. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `container` | 是 | `string \| AutoCodeInterpreterToolParam` | - | The code interpreter container. Can be a container ID or an object that specifies uploaded file IDs to make available to your code, along with an optional memory_limit setting. |
| `type` | 是 | `string` | `code_interpreter` | The type of the code interpreter tool. Always code_interpreter. |

### `CodeInterpreterToolCall`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A tool call to run code. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `code` | 是 | `string \| null` | - | - |
| `container_id` | 是 | `string` | - | The ID of the container used to run the code. |
| `id` | 是 | `string` | - | The unique ID of the code interpreter tool call. |
| `outputs` | 是 | `array<CodeInterpreterOutputLogs \| CodeInterpreterOutputImage> \| null` | - | - |
| `status` | 是 | `string` | `in_progress`, `completed`, `incomplete`, `interpreting`, `failed` | The status of the code interpreter tool call. Valid values are in_progress, completed, incomplete, interpreting, and failed. |
| `type` | 是 | `string` | `code_interpreter_call` | The type of the code interpreter tool call. Always code_interpreter_call. |

### `CompactResource`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | - |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `created_at` | 是 | `integer(unixtime)` | - | Unix timestamp (in seconds) when the compacted conversation was created. |
| `id` | 是 | `string` | - | The unique identifier for the compacted response. |
| `object` | 是 | `string` | `response.compaction` | The object type. Always response.compaction. |
| `output` | 是 | `array<ItemField>` | - | The compacted list of output items. |
| `usage` | 是 | `ResponseUsage` | - | Token accounting for the compaction pass, including cached, reasoning, and total tokens. |

### `CompactResponseMethodPublicBody`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | - |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `input` | 否 | `string \| array<InputItem> \| null` | - | - |
| `instructions` | 否 | `string \| null` | - | - |
| `model` | 是 | `ModelIdsCompaction` | - | - |
| `previous_response_id` | 否 | `string \| null` | - | - |
| `prompt_cache_key` | 否 | `string \| null` | - | - |
| `prompt_cache_retention` | 否 | `PromptCacheRetentionEnum \| null` | - | - |
| `service_tier` | 否 | `ServiceTierEnum \| null` | - | - |

### `CompactionBody`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A compaction item generated by the [v1/responses/compact API](/docs/api-reference/responses/compact). |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `created_by` | 否 | `string` | - | The identifier of the actor that created the item. |
| `encrypted_content` | 是 | `string` | - | The encrypted content that was produced by compaction. |
| `id` | 是 | `string` | - | The unique ID of the compaction item. |
| `type` | 是 | `string` | `compaction` | The type of the item. Always compaction. |

### `CompactionSummaryItemParam`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A compaction item generated by the [v1/responses/compact API](/docs/api-reference/responses/compact). |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `encrypted_content` | 是 | `string` | - | The encrypted content of the compaction summary. |
| `id` | 否 | `string \| null` | - | - |
| `type` | 是 | `string` | `compaction` | The type of the item. Always compaction. |

### `CompactionTriggerItemParam`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Compacts the current context. Must be the final input item. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `type` | 是 | `string` | `compaction_trigger` | The type of the item. Always compaction_trigger. |

### `ComparisonFilter`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A filter used to compare a specified attribute key to a given value using a defined comparison operation. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `key` | 是 | `string` | - | The key to compare against the value. |
| `type` | 是 | `string` | `eq`, `ne`, `gt`, `gte`, `lt`, `lte`, `in`, `nin` | Specifies the comparison operator: eq, ne, gt, gte, lt, lte, in, nin. - eq: equals - ne: not equal - gt: greater than - gte: greater than or equal - lt: less than - lte: less than… |
| `value` | 是 | `string \| number \| boolean \| array<string \| number>` | - | The value to compare against the attribute key; supports string, number, or boolean types. |

### `CompletionUsage`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Usage statistics for the completion request. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `completion_tokens` | 是 | `integer` | - | Number of tokens in the generated completion. |
| `completion_tokens_details` | 否 | `object` | - | Breakdown of tokens used in a completion. |
| `prompt_tokens` | 是 | `integer` | - | Number of tokens in the prompt. |
| `prompt_tokens_details` | 否 | `object` | - | Breakdown of tokens used in the prompt. |
| `total_tokens` | 是 | `integer` | - | Total number of tokens used in the request (prompt + completion). |

### `CompoundFilter`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Combine multiple filters using and or or. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `filters` | 是 | `array<ComparisonFilter \| object/value>` | - | Array of filters to combine. Items can be ComparisonFilter or CompoundFilter. |
| `type` | 是 | `string` | `and`, `or` | Type of operation: and or or. |

### `ComputerAction`

| 项 | 值 |
| --- | --- |
| 类型 | `ClickParam \| DoubleClickAction \| DragParam \| KeyPressAction \| MoveParam \| ScreenshotParam \| ScrollParam \| TypeParam … (+1)` |
| 说明 | - |
| 组合 | `oneOf` |

| 变体 | 类型 | 说明 |
| --- | --- | --- |
| 1 | `ClickParam` | - |
| 2 | `DoubleClickAction` | - |
| 3 | `DragParam` | - |
| 4 | `KeyPressAction` | - |
| 5 | `MoveParam` | - |
| 6 | `ScreenshotParam` | - |
| 7 | `ScrollParam` | - |
| 8 | `TypeParam` | - |
| 9 | `WaitParam` | - |

### `ComputerActionList`

| 项 | 值 |
| --- | --- |
| 类型 | `array<ComputerAction>` |
| 说明 | Flattened batched actions for computer_use. Each action includes an type discriminator and action-specific fields. |

### `ComputerCallOutputItemParam`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | The output of a computer tool call. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `acknowledged_safety_checks` | 否 | `array<ComputerCallSafetyCheckParam> \| null` | - | - |
| `call_id` | 是 | `string` | - | The ID of the computer tool call that produced the output. |
| `id` | 否 | `string \| null` | - | - |
| `output` | 是 | `ComputerScreenshotImage` | - | - |
| `status` | 否 | `FunctionCallItemStatus \| null` | - | - |
| `type` | 是 | `string` | `computer_call_output` | The type of the computer tool call output. Always computer_call_output. |

### `ComputerCallOutputStatus`

| 项 | 值 |
| --- | --- |
| 类型 | `string` |
| 说明 | - |

### `ComputerCallSafetyCheckParam`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A pending safety check for the computer call. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `code` | 否 | `string \| null` | - | - |
| `id` | 是 | `string` | - | The ID of the pending safety check. |
| `message` | 否 | `string \| null` | - | - |

### `ComputerEnvironment`

| 项 | 值 |
| --- | --- |
| 类型 | `string` |
| 说明 | - |

### `ComputerScreenshotContent`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A screenshot of a computer. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `detail` | 是 | `ImageDetail` | - | The detail level of the screenshot image to be sent to the model. One of high, low, auto, or original. Defaults to auto. |
| `file_id` | 是 | `string \| null` | - | - |
| `image_url` | 是 | `string(uri) \| null` | - | - |
| `type` | 是 | `string` | `computer_screenshot` | Specifies the event type. For a computer screenshot, this property is always set to computer_screenshot. |

### `ComputerScreenshotImage`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A computer screenshot image used with the computer use tool. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `file_id` | 否 | `string` | - | The identifier of an uploaded file that contains the screenshot. |
| `image_url` | 否 | `string(uri)` | - | The URL of the screenshot image. |
| `type` | 是 | `string` | `computer_screenshot` | Specifies the event type. For a computer screenshot, this property is always set to computer_screenshot. |

### `ComputerTool`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A tool that controls a virtual computer. Learn more about the [computer tool](https://platform.openai.com/docs/guides/tools-computer-use). |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `type` | 是 | `string` | `computer` | The type of the computer tool. Always computer. |

### `ComputerToolCall`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A tool call to a computer use tool. See the [computer use guide](/docs/guides/tools-computer-use) for more information. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `action` | 否 | `ComputerAction` | - | - |
| `actions` | 否 | `ComputerActionList` | - | - |
| `call_id` | 是 | `string` | - | An identifier used when responding to the tool call with output. |
| `id` | 是 | `string` | - | The unique ID of the computer call. |
| `pending_safety_checks` | 是 | `array<ComputerCallSafetyCheckParam>` | - | The pending safety checks for the computer call. |
| `status` | 是 | `string` | `in_progress`, `completed`, `incomplete` | The status of the item. One of in_progress, completed, or incomplete. Populated when items are returned via API. |
| `type` | 是 | `string` | `computer_call` | The type of the computer call. Always computer_call. |

### `ComputerToolCallOutput`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | The output of a computer tool call. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `acknowledged_safety_checks` | 否 | `array<ComputerCallSafetyCheckParam>` | - | The safety checks reported by the API that have been acknowledged by the developer. |
| `call_id` | 是 | `string` | - | The ID of the computer tool call that produced the output. |
| `id` | 否 | `string` | - | The ID of the computer tool call output. |
| `output` | 是 | `ComputerScreenshotImage` | - | - |
| `status` | 否 | `string` | `in_progress`, `completed`, `incomplete` | The status of the message input. One of in_progress, completed, or incomplete. Populated when input items are returned via API. |
| `type` | 是 | `string` | `computer_call_output` | The type of the computer tool call output. Always computer_call_output. |

### `ComputerToolCallOutputResource`

| 项 | 值 |
| --- | --- |
| 类型 | `ComputerToolCallOutput & object` |
| 说明 | - |
| 组合 | `allOf` |

| 变体 | 类型 | 说明 |
| --- | --- | --- |
| 1 | `ComputerToolCallOutput` | - |
| 2 | `object` | - |

#### allOf 展开字段

| 字段 | 必填 | 类型 | 枚举/常量 | 来源 | 说明 |
| --- | --- | --- | --- | --- | --- |
| `acknowledged_safety_checks` | 否 | `array<ComputerCallSafetyCheckParam>` | - | `ComputerToolCallOutput` | The safety checks reported by the API that have been acknowledged by the developer. |
| `call_id` | 是 | `string` | - | `ComputerToolCallOutput` | The ID of the computer tool call that produced the output. |
| `created_by` | 否 | `string` | - | `ComputerToolCallOutputResource.allOf[2]` | The identifier of the actor that created the item. |
| `id` | 是 | `string` | - | `ComputerToolCallOutput` | The ID of the computer tool call output. |
| `output` | 是 | `ComputerScreenshotImage` | - | `ComputerToolCallOutput` | - |
| `status` | 是 | `string` | `in_progress`, `completed`, `incomplete` | `ComputerToolCallOutput` | The status of the message input. One of in_progress, completed, or incomplete. Populated when input items are returned via API. |
| `type` | 是 | `string` | `computer_call_output` | `ComputerToolCallOutput` | The type of the computer tool call output. Always computer_call_output. |

### `ComputerUsePreviewTool`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A tool that controls a virtual computer. Learn more about the [computer tool](https://platform.openai.com/docs/guides/tools-computer-use). |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `display_height` | 是 | `integer` | - | The height of the computer display. |
| `display_width` | 是 | `integer` | - | The width of the computer display. |
| `environment` | 是 | `ComputerEnvironment` | - | The type of computer environment to control. |
| `type` | 是 | `string` | `computer_use_preview` | The type of the computer use tool. Always computer_use_preview. |

### `ContainerAutoParam`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | - |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `file_ids` | 否 | `array<string>` | - | An optional list of uploaded files to make available to your code. |
| `memory_limit` | 否 | `ContainerMemoryLimit \| null` | - | - |
| `network_policy` | 否 | `ContainerNetworkPolicyDisabledParam \| ContainerNetworkPolicyAllowlistParam` | - | Network access policy for the container. |
| `skills` | 否 | `array<SkillReferenceParam \| InlineSkillParam>` | - | An optional list of skills referenced by id or inline data. |
| `type` | 是 | `string` | `container_auto` | Automatically creates a container for this request |

### `ContainerFileCitationBody`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A citation for a container file used to generate a model response. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `container_id` | 是 | `string` | - | The ID of the container file. |
| `end_index` | 是 | `integer` | - | The index of the last character of the container file citation in the message. |
| `file_id` | 是 | `string` | - | The ID of the file. |
| `filename` | 是 | `string` | - | The filename of the container file cited. |
| `start_index` | 是 | `integer` | - | The index of the first character of the container file citation in the message. |
| `type` | 是 | `string` | `container_file_citation` | The type of the container file citation. Always container_file_citation. |

### `ContainerMemoryLimit`

| 项 | 值 |
| --- | --- |
| 类型 | `string` |
| 说明 | - |

### `ContainerNetworkPolicyAllowlistParam`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | - |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `allowed_domains` | 是 | `array<string>` | - | A list of allowed domains when type is allowlist. |
| `domain_secrets` | 否 | `array<ContainerNetworkPolicyDomainSecretParam>` | - | Optional domain-scoped secrets for allowlisted domains. |
| `type` | 是 | `string` | `allowlist` | Allow outbound network access only to specified domains. Always allowlist. |

### `ContainerNetworkPolicyDisabledParam`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | - |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `type` | 是 | `string` | `disabled` | Disable outbound network access. Always disabled. |

### `ContainerNetworkPolicyDomainSecretParam`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | - |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `domain` | 是 | `string` | - | The domain associated with the secret. |
| `name` | 是 | `string` | - | The name of the secret to inject for the domain. |
| `value` | 是 | `string` | - | The secret value to inject for the domain. |

### `ContainerReferenceParam`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | - |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `container_id` | 是 | `string` | - | The ID of the referenced container. |
| `type` | 是 | `string` | `container_reference` | References a container created with the /v1/containers endpoint |

### `ContainerReferenceResource`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Represents a container created with /v1/containers. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `container_id` | 是 | `string` | - | - |
| `type` | 是 | `string` | `container_reference` | The environment type. Always container_reference. |

### `ContextManagementParam`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | - |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `compact_threshold` | 否 | `integer \| null` | - | - |
| `type` | 是 | `string` | - | The context management entry type. Currently only 'compaction' is supported. |

### `Conversation-2`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | The conversation that this response belonged to. Input items and output items from this response were automatically added to this conversation. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `id` | 是 | `string` | - | The unique ID of the conversation that this response was associated with. |

### `ConversationParam`

| 项 | 值 |
| --- | --- |
| 类型 | `string \| ConversationParam-2` |
| 说明 | The conversation that this response belongs to. Items from this conversation are prepended to input_items for this response request. Input items and output items from this respons… |
| 组合 | `oneOf` |

| 变体 | 类型 | 说明 |
| --- | --- | --- |
| 1 | `string` | The unique ID of the conversation. |
| 2 | `ConversationParam-2` | - |

### `ConversationParam-2`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | The conversation that this response belongs to. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `id` | 是 | `string` | - | The unique ID of the conversation. |

### `CoordParam`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | An x/y coordinate pair, e.g. { x: 100, y: 200 }. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `x` | 是 | `integer` | - | The x-coordinate. |
| `y` | 是 | `integer` | - | The y-coordinate. |

### `CreateChatCompletionRequest`

| 项 | 值 |
| --- | --- |
| 类型 | `CreateModelResponseProperties & object` |
| 说明 | - |
| 组合 | `allOf` |

| 变体 | 类型 | 说明 |
| --- | --- | --- |
| 1 | `CreateModelResponseProperties` | - |
| 2 | `object` | - |

#### allOf 展开字段

| 字段 | 必填 | 类型 | 枚举/常量 | 来源 | 说明 |
| --- | --- | --- | --- | --- | --- |
| `audio` | 否 | `object` | - | `CreateChatCompletionRequest.allOf[2]` | Parameters for audio output. Required when audio output is requested with modalities: ["audio"]. [Learn more](/docs/guides/audio). |
| `frequency_penalty` | 否 | `number` | - | `CreateChatCompletionRequest.allOf[2]` | Number between -2.0 and 2.0. Positive values penalize new tokens based on their existing frequency in the text so far, decreasing the model's likelihood to repeat the same line ve… |
| `function_call` | 否 | `string \| ChatCompletionFunctionCallOption` | - | `CreateChatCompletionRequest.allOf[2]` | Deprecated in favor of tool_choice. Controls which (if any) function is called by the model. none means the model will not call a function and instead generates a message. auto me… |
| `functions` | 否 | `array<ChatCompletionFunctions>` | - | `CreateChatCompletionRequest.allOf[2]` | Deprecated in favor of tools. A list of functions the model may generate JSON inputs for. |
| `logit_bias` | 否 | `object/map<string, integer>` | - | `CreateChatCompletionRequest.allOf[2]` | Modify the likelihood of specified tokens appearing in the completion. Accepts a JSON object that maps tokens (specified by their token ID in the tokenizer) to an associated bias … |
| `logprobs` | 否 | `boolean` | - | `CreateChatCompletionRequest.allOf[2]` | Whether to return log probabilities of the output tokens or not. If true, returns the log probabilities of each output token returned in the content of message. |
| `max_completion_tokens` | 否 | `integer` | - | `CreateChatCompletionRequest.allOf[2]` | An upper bound for the number of tokens that can be generated for a completion, including visible output tokens and [reasoning tokens](/docs/guides/reasoning). |
| `max_tokens` | 否 | `integer` | - | `CreateChatCompletionRequest.allOf[2]` | The maximum number of [tokens](/tokenizer) that can be generated in the chat completion. This value can be used to control [costs](https://openai.com/api/pricing/) for text genera… |
| `messages` | 是 | `array<ChatCompletionRequestMessage>` | - | `CreateChatCompletionRequest.allOf[2]` | A list of messages comprising the conversation so far. Depending on the [model](/docs/models) you use, different message types (modalities) are supported, like [text](/docs/guides… |
| `metadata` | 否 | `Metadata` | - | `ModelResponseProperties` | - |
| `modalities` | 否 | `ResponseModalities` | - | `CreateChatCompletionRequest.allOf[2]` | - |
| `model` | 是 | `ModelIdsShared` | - | `CreateChatCompletionRequest.allOf[2]` | Model ID used to generate the response, like gpt-4o or o3. OpenAI offers a wide range of models with different capabilities, performance characteristics, and price points. Refer t… |
| `n` | 否 | `integer` | - | `CreateChatCompletionRequest.allOf[2]` | How many chat completion choices to generate for each input message. Note that you will be charged based on the number of generated tokens across all of the choices. Keep n as 1 t… |
| `parallel_tool_calls` | 否 | `ParallelToolCalls` | - | `CreateChatCompletionRequest.allOf[2]` | - |
| `prediction` | 否 | `PredictionContent` | - | `CreateChatCompletionRequest.allOf[2]` | Configuration for a [Predicted Output](/docs/guides/predicted-outputs), which can greatly improve response times when large parts of the model response are known ahead of time. Th… |
| `presence_penalty` | 否 | `number` | - | `CreateChatCompletionRequest.allOf[2]` | Number between -2.0 and 2.0. Positive values penalize new tokens based on whether they appear in the text so far, increasing the model's likelihood to talk about new topics. |
| `prompt_cache_key` | 否 | `string` | - | `ModelResponseProperties` | Used by OpenAI to cache responses for similar requests to optimize your cache hit rates. Replaces the user field. [Learn more](/docs/guides/prompt-caching). |
| `prompt_cache_retention` | 否 | `string \| null` | - | `ModelResponseProperties` | - |
| `reasoning_effort` | 否 | `ReasoningEffort` | - | `CreateChatCompletionRequest.allOf[2]` | - |
| `response_format` | 否 | `ResponseFormatText \| ResponseFormatJsonSchema \| ResponseFormatJsonObject` | - | `CreateChatCompletionRequest.allOf[2]` | An object specifying the format that the model must output. Setting to { "type": "json_schema", "json_schema": {...} } enables Structured Outputs which ensures the model will matc… |
| `safety_identifier` | 否 | `string` | - | `ModelResponseProperties` | A stable identifier used to help detect users of your application that may be violating OpenAI's usage policies. The IDs should be a string that uniquely identifies each user, wit… |
| `seed` | 否 | `integer` | - | `CreateChatCompletionRequest.allOf[2]` | This feature is in Beta. If specified, our system will make a best effort to sample deterministically, such that repeated requests with the same seed and parameters should return … |
| `service_tier` | 否 | `ServiceTier` | - | `ModelResponseProperties` | - |
| `stop` | 否 | `StopConfiguration` | - | `CreateChatCompletionRequest.allOf[2]` | - |
| `store` | 否 | `boolean` | - | `CreateChatCompletionRequest.allOf[2]` | Whether or not to store the output of this chat completion request for use in our [model distillation](/docs/guides/distillation) or [evals](/docs/guides/evals) products. Supports… |
| `stream` | 否 | `boolean` | - | `CreateChatCompletionRequest.allOf[2]` | If set to true, the model response data will be streamed to the client as it is generated using [server-sent events](https://developer.mozilla.org/en-US/docs/Web/API/Server-sent_e… |
| `stream_options` | 否 | `ChatCompletionStreamOptions` | - | `CreateChatCompletionRequest.allOf[2]` | - |
| `temperature` | 否 | `number \| null` | - | `ModelResponseProperties` | - |
| `tool_choice` | 否 | `ChatCompletionToolChoiceOption` | - | `CreateChatCompletionRequest.allOf[2]` | - |
| `tools` | 否 | `array<ChatCompletionTool \| CustomToolChatCompletions>` | - | `CreateChatCompletionRequest.allOf[2]` | A list of tools the model may call. You can provide either [custom tools](/docs/guides/function-calling#custom-tools) or [function tools](/docs/guides/function-calling). |
| `top_logprobs` | 否 | `integer \| null` | - | `ModelResponseProperties` | - |
| `top_p` | 否 | `number \| null` | - | `ModelResponseProperties` | - |
| `user` | 否 | `string` | - | `ModelResponseProperties` | This field is being replaced by safety_identifier and prompt_cache_key. Use prompt_cache_key instead to maintain caching optimizations. A stable identifier for your end-users. Use… |
| `verbosity` | 否 | `Verbosity` | - | `CreateChatCompletionRequest.allOf[2]` | - |
| `web_search_options` | 否 | `object` | - | `CreateChatCompletionRequest.allOf[2]` | This tool searches the web for relevant results to use in a response. Learn more about the [web search tool](/docs/guides/tools-web-search?api-mode=chat). |

### `CreateChatCompletionResponse`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Represents a chat completion response returned by model, based on the provided input. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `choices` | 是 | `array<object>` | - | A list of chat completion choices. Can be more than one if n is greater than 1. |
| `created` | 是 | `integer(unixtime)` | - | The Unix timestamp (in seconds) of when the chat completion was created. |
| `id` | 是 | `string` | - | A unique identifier for the chat completion. |
| `model` | 是 | `string` | - | The model used for the chat completion. |
| `object` | 是 | `string` | `chat.completion` | The object type, which is always chat.completion. |
| `service_tier` | 否 | `ServiceTier` | - | - |
| `system_fingerprint` | 否 | `string` | - | This fingerprint represents the backend configuration that the model runs with. Can be used in conjunction with the seed request parameter to understand when backend changes have … |
| `usage` | 否 | `CompletionUsage` | - | - |

### `CreateChatCompletionStreamResponse`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Represents a streamed chunk of a chat completion response returned by the model, based on the provided input. [Learn more](/docs/guides/streaming-responses). |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `choices` | 是 | `array<object>` | - | A list of chat completion choices. Can contain more than one elements if n is greater than 1. Can also be empty for the last chunk if you set stream_options: {"include_usage": tru… |
| `created` | 是 | `integer(unixtime)` | - | The Unix timestamp (in seconds) of when the chat completion was created. Each chunk has the same timestamp. |
| `id` | 是 | `string` | - | A unique identifier for the chat completion. Each chunk has the same ID. |
| `model` | 是 | `string` | - | The model to generate the completion. |
| `object` | 是 | `string` | `chat.completion.chunk` | The object type, which is always chat.completion.chunk. |
| `service_tier` | 否 | `ServiceTier` | - | - |
| `system_fingerprint` | 否 | `string` | - | This fingerprint represents the backend configuration that the model runs with. Can be used in conjunction with the seed request parameter to understand when backend changes have … |
| `usage` | 否 | `CompletionUsage` | - | An optional field that will only be present when you set stream_options: {"include_usage": true} in your request. When present, it contains a null value **except for the last chun… |

### `CreateEmbeddingRequest`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | - |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `dimensions` | 否 | `integer` | - | The number of dimensions the resulting output embeddings should have. Only supported in text-embedding-3 and later models. |
| `encoding_format` | 否 | `string` | `float`, `base64` | The format to return the embeddings in. Can be either float or [base64](https://pypi.org/project/pybase64/). |
| `input` | 是 | `string \| array<string> \| array<integer> \| array<array<integer>>` | - | Input text to embed, encoded as a string or array of tokens. To embed multiple inputs in a single request, pass an array of strings or array of token arrays. The input must not ex… |
| `model` | 是 | `string \| string` | - | ID of the model to use. You can use the [List models](/docs/api-reference/models/list) API to see all of your available models, or see our [Model overview](/docs/models) for descr… |
| `user` | 否 | `string` | - | A unique identifier representing your end-user, which can help OpenAI to monitor and detect abuse. [Learn more](/docs/guides/safety-best-practices#end-user-ids). |

### `CreateEmbeddingResponse`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | - |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `data` | 是 | `array<Embedding>` | - | The list of embeddings generated by the model. |
| `model` | 是 | `string` | - | The name of the model used to generate the embedding. |
| `object` | 是 | `string` | `list` | The object type, which is always "list". |
| `usage` | 是 | `object` | - | The usage information for the request. |

### `CreateImageEditRequest`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | - |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `background` | 否 | `string` | `transparent`, `opaque`, `auto` | Allows to set transparency for the background of the generated image(s). This parameter is only supported for the GPT image models. Must be one of transparent, opaque or auto (def… |
| `image` | 是 | `string(binary) \| array<string(binary)>` | - | The image(s) to edit. Must be a supported image file or an array of images. For the GPT image models (gpt-image-1, gpt-image-1-mini, and gpt-image-1.5), each image should be a png… |
| `input_fidelity` | 否 | `InputFidelity \| null` | - | - |
| `mask` | 否 | `string(binary)` | - | An additional image whose fully transparent areas (e.g. where alpha is zero) indicate where image should be edited. If there are multiple images provided, the mask will be applied… |
| `model` | 否 | `string \| string` | - | The model to use for image generation. Defaults to gpt-image-1.5. |
| `n` | 否 | `integer` | - | The number of images to generate. Must be between 1 and 10. |
| `output_compression` | 否 | `integer` | - | The compression level (0-100%) for the generated images. This parameter is only supported for the GPT image models with the webp or jpeg output formats, and defaults to 100. |
| `output_format` | 否 | `string` | `png`, `jpeg`, `webp` | The format in which the generated images are returned. This parameter is only supported for the GPT image models. Must be one of png, jpeg, or webp. The default value is png. |
| `partial_images` | 否 | `PartialImages` | - | - |
| `prompt` | 是 | `string` | - | A text description of the desired image(s). The maximum length is 1000 characters for dall-e-2, and 32000 characters for the GPT image models. |
| `quality` | 否 | `string` | `standard`, `low`, `medium`, `high`, `auto` | The quality of the image that will be generated for GPT image models. Defaults to auto. |
| `response_format` | 否 | `string` | `url`, `b64_json` | The format in which the generated images are returned. Must be one of url or b64_json. URLs are only valid for 60 minutes after the image has been generated. This parameter is onl… |
| `size` | 否 | `string \| string` | - | The size of the generated images. For gpt-image-2 and gpt-image-2-2026-04-21, arbitrary resolutions are supported as WIDTHxHEIGHT strings, for example 1536x864. Width and height m… |
| `stream` | 否 | `boolean` | - | Edit the image in streaming mode. Defaults to false. See the [Image generation guide](/docs/guides/image-generation) for more information. |
| `user` | 否 | `string` | - | A unique identifier representing your end-user, which can help OpenAI to monitor and detect abuse. [Learn more](/docs/guides/safety-best-practices#end-user-ids). |

### `CreateImageRequest`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | - |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `background` | 否 | `string` | `transparent`, `opaque`, `auto` | Allows to set transparency for the background of the generated image(s). This parameter is only supported for the GPT image models. Must be one of transparent, opaque or auto (def… |
| `model` | 否 | `string \| string` | - | The model to use for image generation. One of dall-e-2, dall-e-3, or a GPT image model (gpt-image-1, gpt-image-1-mini, gpt-image-1.5). Defaults to dall-e-2 unless a parameter spec… |
| `moderation` | 否 | `string` | `low`, `auto` | Control the content-moderation level for images generated by the GPT image models. Must be either low for less restrictive filtering or auto (default value). |
| `n` | 否 | `integer` | - | The number of images to generate. Must be between 1 and 10. For dall-e-3, only n=1 is supported. |
| `output_compression` | 否 | `integer` | - | The compression level (0-100%) for the generated images. This parameter is only supported for the GPT image models with the webp or jpeg output formats, and defaults to 100. |
| `output_format` | 否 | `string` | `png`, `jpeg`, `webp` | The format in which the generated images are returned. This parameter is only supported for the GPT image models. Must be one of png, jpeg, or webp. |
| `partial_images` | 否 | `PartialImages` | - | - |
| `prompt` | 是 | `string` | - | A text description of the desired image(s). The maximum length is 32000 characters for the GPT image models, 1000 characters for dall-e-2 and 4000 characters for dall-e-3. |
| `quality` | 否 | `string` | `standard`, `hd`, `low`, `medium`, `high`, `auto` | The quality of the image that will be generated. - auto (default value) will automatically select the best quality for the given model. - high, medium and low are supported for th… |
| `response_format` | 否 | `string` | `url`, `b64_json` | The format in which generated images with dall-e-2 and dall-e-3 are returned. Must be one of url or b64_json. URLs are only valid for 60 minutes after the image has been generated… |
| `size` | 否 | `string \| string` | - | The size of the generated images. For gpt-image-2 and gpt-image-2-2026-04-21, arbitrary resolutions are supported as WIDTHxHEIGHT strings, for example 1536x864. Width and height m… |
| `stream` | 否 | `boolean` | - | Generate the image in streaming mode. Defaults to false. See the [Image generation guide](/docs/guides/image-generation) for more information. This parameter is only supported for… |
| `style` | 否 | `string` | `vivid`, `natural` | The style of the generated images. This parameter is only supported for dall-e-3. Must be one of vivid or natural. Vivid causes the model to lean towards generating hyper-real and… |
| `user` | 否 | `string` | - | A unique identifier representing your end-user, which can help OpenAI to monitor and detect abuse. [Learn more](/docs/guides/safety-best-practices#end-user-ids). |

### `CreateImageVariationRequest`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | - |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `image` | 是 | `string(binary)` | - | The image to use as the basis for the variation(s). Must be a valid PNG file, less than 4MB, and square. |
| `model` | 否 | `string \| string` | - | The model to use for image generation. Only dall-e-2 is supported at this time. |
| `n` | 否 | `integer` | - | The number of images to generate. Must be between 1 and 10. |
| `response_format` | 否 | `string` | `url`, `b64_json` | The format in which the generated images are returned. Must be one of url or b64_json. URLs are only valid for 60 minutes after the image has been generated. |
| `size` | 否 | `string` | `256x256`, `512x512`, `1024x1024` | The size of the generated images. Must be one of 256x256, 512x512, or 1024x1024. |
| `user` | 否 | `string` | - | A unique identifier representing your end-user, which can help OpenAI to monitor and detect abuse. [Learn more](/docs/guides/safety-best-practices#end-user-ids). |

### `CreateModelResponseProperties`

| 项 | 值 |
| --- | --- |
| 类型 | `ModelResponseProperties & object` |
| 说明 | - |
| 组合 | `allOf` |

| 变体 | 类型 | 说明 |
| --- | --- | --- |
| 1 | `ModelResponseProperties` | - |
| 2 | `object` | - |

#### allOf 展开字段

| 字段 | 必填 | 类型 | 枚举/常量 | 来源 | 说明 |
| --- | --- | --- | --- | --- | --- |
| `metadata` | 否 | `Metadata` | - | `ModelResponseProperties` | - |
| `prompt_cache_key` | 否 | `string` | - | `ModelResponseProperties` | Used by OpenAI to cache responses for similar requests to optimize your cache hit rates. Replaces the user field. [Learn more](/docs/guides/prompt-caching). |
| `prompt_cache_retention` | 否 | `string \| null` | - | `ModelResponseProperties` | - |
| `safety_identifier` | 否 | `string` | - | `ModelResponseProperties` | A stable identifier used to help detect users of your application that may be violating OpenAI's usage policies. The IDs should be a string that uniquely identifies each user, wit… |
| `service_tier` | 否 | `ServiceTier` | - | `ModelResponseProperties` | - |
| `temperature` | 否 | `number \| null` | - | `ModelResponseProperties` | - |
| `top_logprobs` | 否 | `integer \| null` | - | `ModelResponseProperties` | - |
| `top_p` | 否 | `number \| null` | - | `ModelResponseProperties` | - |
| `user` | 否 | `string` | - | `ModelResponseProperties` | This field is being replaced by safety_identifier and prompt_cache_key. Use prompt_cache_key instead to maintain caching optimizations. A stable identifier for your end-users. Use… |

### `CreateResponse`

| 项 | 值 |
| --- | --- |
| 类型 | `CreateModelResponseProperties & ResponseProperties & object` |
| 说明 | - |
| 组合 | `allOf` |

| 变体 | 类型 | 说明 |
| --- | --- | --- |
| 1 | `CreateModelResponseProperties` | - |
| 2 | `ResponseProperties` | - |
| 3 | `object` | - |

#### allOf 展开字段

| 字段 | 必填 | 类型 | 枚举/常量 | 来源 | 说明 |
| --- | --- | --- | --- | --- | --- |
| `background` | 否 | `boolean \| null` | - | `ResponseProperties` | - |
| `context_management` | 否 | `array<ContextManagementParam> \| null` | - | `CreateResponse.allOf[3]` | - |
| `conversation` | 否 | `ConversationParam \| null` | - | `CreateResponse.allOf[3]` | - |
| `include` | 否 | `array<IncludeEnum> \| null` | - | `CreateResponse.allOf[3]` | - |
| `input` | 否 | `InputParam` | - | `CreateResponse.allOf[3]` | - |
| `instructions` | 否 | `string \| null` | - | `CreateResponse.allOf[3]` | - |
| `max_output_tokens` | 否 | `integer \| null` | - | `CreateResponse.allOf[3]` | - |
| `max_tool_calls` | 否 | `integer \| null` | - | `ResponseProperties` | - |
| `metadata` | 否 | `Metadata` | - | `ModelResponseProperties` | - |
| `model` | 否 | `ModelIdsResponses` | - | `ResponseProperties` | Model ID used to generate the response, like gpt-4o or o3. OpenAI offers a wide range of models with different capabilities, performance characteristics, and price points. Refer t… |
| `parallel_tool_calls` | 否 | `boolean \| null` | - | `CreateResponse.allOf[3]` | - |
| `previous_response_id` | 否 | `string \| null` | - | `ResponseProperties` | - |
| `prompt` | 否 | `Prompt` | - | `ResponseProperties` | - |
| `prompt_cache_key` | 否 | `string` | - | `ModelResponseProperties` | Used by OpenAI to cache responses for similar requests to optimize your cache hit rates. Replaces the user field. [Learn more](/docs/guides/prompt-caching). |
| `prompt_cache_retention` | 否 | `string \| null` | - | `ModelResponseProperties` | - |
| `reasoning` | 否 | `Reasoning \| null` | - | `ResponseProperties` | - |
| `safety_identifier` | 否 | `string` | - | `ModelResponseProperties` | A stable identifier used to help detect users of your application that may be violating OpenAI's usage policies. The IDs should be a string that uniquely identifies each user, wit… |
| `service_tier` | 否 | `ServiceTier` | - | `ModelResponseProperties` | - |
| `store` | 否 | `boolean \| null` | - | `CreateResponse.allOf[3]` | - |
| `stream` | 否 | `boolean \| null` | - | `CreateResponse.allOf[3]` | - |
| `stream_options` | 否 | `ResponseStreamOptions` | - | `CreateResponse.allOf[3]` | - |
| `temperature` | 否 | `number \| null` | - | `ModelResponseProperties` | - |
| `text` | 否 | `ResponseTextParam` | - | `ResponseProperties` | - |
| `tool_choice` | 否 | `ToolChoiceParam` | - | `ResponseProperties` | - |
| `tools` | 否 | `ToolsArray` | - | `ResponseProperties` | - |
| `top_logprobs` | 否 | `integer \| null` | - | `ModelResponseProperties` | - |
| `top_p` | 否 | `number \| null` | - | `ModelResponseProperties` | - |
| `truncation` | 否 | `string \| null` | - | `ResponseProperties` | - |
| `user` | 否 | `string` | - | `ModelResponseProperties` | This field is being replaced by safety_identifier and prompt_cache_key. Use prompt_cache_key instead to maintain caching optimizations. A stable identifier for your end-users. Use… |

### `CustomGrammarFormatParam`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A grammar defined by the user. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `definition` | 是 | `string` | - | The grammar definition. |
| `syntax` | 是 | `GrammarSyntax1` | - | The syntax of the grammar definition. One of lark or regex. |
| `type` | 是 | `string` | `grammar` | Grammar format. Always grammar. |

### `CustomTextFormatParam`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Unconstrained free-form text. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `type` | 是 | `string` | `text` | Unconstrained text format. Always text. |

### `CustomToolCall`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A call to a custom tool created by the model. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `call_id` | 是 | `string` | - | An identifier used to map this custom tool call to a tool call output. |
| `id` | 否 | `string` | - | The unique ID of the custom tool call in the OpenAI platform. |
| `input` | 是 | `string` | - | The input for the custom tool call generated by the model. |
| `name` | 是 | `string` | - | The name of the custom tool being called. |
| `namespace` | 否 | `string` | - | The namespace of the custom tool being called. |
| `type` | 是 | `string` | `custom_tool_call` | The type of the custom tool call. Always custom_tool_call. |

### `CustomToolCallOutput`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | The output of a custom tool call from your code, being sent back to the model. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `call_id` | 是 | `string` | - | The call ID, used to map this custom tool call output to a custom tool call. |
| `id` | 否 | `string` | - | The unique ID of the custom tool call output in the OpenAI platform. |
| `output` | 是 | `string \| array<FunctionAndCustomToolCallOutput>` | - | The output from the custom tool call generated by your code. Can be a string or an list of output content. |
| `type` | 是 | `string` | `custom_tool_call_output` | The type of the custom tool call output. Always custom_tool_call_output. |

### `CustomToolCallOutputResource`

| 项 | 值 |
| --- | --- |
| 类型 | `CustomToolCallOutput & object` |
| 说明 | - |
| 组合 | `allOf` |

| 变体 | 类型 | 说明 |
| --- | --- | --- |
| 1 | `CustomToolCallOutput` | - |
| 2 | `object` | - |

#### allOf 展开字段

| 字段 | 必填 | 类型 | 枚举/常量 | 来源 | 说明 |
| --- | --- | --- | --- | --- | --- |
| `call_id` | 是 | `string` | - | `CustomToolCallOutput` | The call ID, used to map this custom tool call output to a custom tool call. |
| `created_by` | 否 | `string` | - | `CustomToolCallOutputResource.allOf[2]` | The identifier of the actor that created the item. |
| `id` | 是 | `string` | - | `CustomToolCallOutput` | The unique ID of the custom tool call output in the OpenAI platform. |
| `output` | 是 | `string \| array<FunctionAndCustomToolCallOutput>` | - | `CustomToolCallOutput` | The output from the custom tool call generated by your code. Can be a string or an list of output content. |
| `status` | 是 | `FunctionCallOutputStatusEnum` | - | `CustomToolCallOutputResource.allOf[2]` | The status of the item. One of in_progress, completed, or incomplete. Populated when items are returned via API. |
| `type` | 是 | `string` | `custom_tool_call_output` | `CustomToolCallOutput` | The type of the custom tool call output. Always custom_tool_call_output. |

### `CustomToolChatCompletions`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A custom tool that processes input using a specified format. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `custom` | 是 | `object` | - | Properties of the custom tool. |
| `type` | 是 | `string` | `custom` | The type of the custom tool. Always custom. |

### `CustomToolParam`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A custom tool that processes input using a specified format. Learn more about [custom tools](/docs/guides/function-calling#custom-tools) |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `defer_loading` | 否 | `boolean` | - | Whether this tool should be deferred and discovered via tool search. |
| `description` | 否 | `string` | - | Optional description of the custom tool, used to provide more context. |
| `format` | 否 | `CustomTextFormatParam \| CustomGrammarFormatParam` | - | The input format for the custom tool. Default is unconstrained text. |
| `name` | 是 | `string` | - | The name of the custom tool, used to identify it in tool calls. |
| `type` | 是 | `string` | `custom` | The type of the custom tool. Always custom. |

### `DetailEnum`

| 项 | 值 |
| --- | --- |
| 类型 | `string` |
| 说明 | - |

### `DoubleClickAction`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A double click action. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `keys` | 是 | `array<string> \| null` | - | - |
| `type` | 是 | `string` | `double_click` | Specifies the event type. For a double click action, this property is always set to double_click. |
| `x` | 是 | `integer` | - | The x-coordinate where the double click occurred. |
| `y` | 是 | `integer` | - | The y-coordinate where the double click occurred. |

### `DragParam`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A drag action. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `keys` | 否 | `array<string> \| null` | - | - |
| `path` | 是 | `array<CoordParam>` | - | An array of coordinates representing the path of the drag action. Coordinates will appear as an array of objects, eg |
| `type` | 是 | `string` | `drag` | Specifies the event type. For a drag action, this property is always set to drag. |

### `EasyInputMessage`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A message input to the model with a role indicating instruction following hierarchy. Instructions given with the developer or system role take precedence over instructions given w… |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `content` | 是 | `string \| InputMessageContentList` | - | Text, image, or audio input to the model, used to generate a response. Can also contain previous assistant responses. |
| `phase` | 否 | `MessagePhase \| null` | - | - |
| `role` | 是 | `string` | `user`, `assistant`, `system`, `developer` | The role of the message input. One of user, assistant, system, or developer. |
| `type` | 否 | `string` | `message` | The type of the message input. Always message. |

### `EditImageBodyJsonParam`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | JSON request body for image edits. Use images (array of ImageRefParam) instead of multipart image uploads. You can reference images via external URLs, data URLs, or uploaded file … |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `background` | 否 | `string \| null` | - | Background behavior for generated image output. |
| `images` | 是 | `array<ImageRefParam>` | - | Input image references to edit. For GPT image models, you can provide up to 16 images. |
| `input_fidelity` | 否 | `string \| null` | - | Controls fidelity to the original input image(s). |
| `mask` | 否 | `ImageRefParam` | - | - |
| `model` | 否 | `string \| string \| null` | - | The model to use for image editing. |
| `moderation` | 否 | `string \| null` | - | Moderation level for GPT image models. |
| `n` | 否 | `integer \| null` | - | The number of edited images to generate. |
| `output_compression` | 否 | `integer \| null` | - | Compression level for jpeg or webp output. |
| `output_format` | 否 | `string \| null` | - | Output image format. Supported for GPT image models. |
| `partial_images` | 否 | `PartialImages` | - | - |
| `prompt` | 是 | `string` | - | A text description of the desired image edit. |
| `quality` | 否 | `string \| null` | - | Output quality for GPT image models. |
| `size` | 否 | `string \| null` | - | Requested output image size. |
| `stream` | 否 | `boolean \| null` | - | Stream partial image results as events. |
| `user` | 否 | `string` | - | A unique identifier representing your end-user, which can help OpenAI monitor and detect abuse. |

### `Embedding`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Represents an embedding vector returned by embedding endpoint. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `embedding` | 是 | `array<number(float)>` | - | The embedding vector, which is a list of floats. The length of vector depends on the model as listed in the [embedding guide](/docs/guides/embeddings). |
| `index` | 是 | `integer` | - | The index of the embedding in the list of embeddings. |
| `object` | 是 | `string` | `embedding` | The object type, which is always "embedding". |

### `EmptyModelParam`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | - |

### `FileCitationBody`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A citation to a file. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `file_id` | 是 | `string` | - | The ID of the file. |
| `filename` | 是 | `string` | - | The filename of the file cited. |
| `index` | 是 | `integer` | - | The index of the file in the list of files. |
| `type` | 是 | `string` | `file_citation` | The type of the file citation. Always file_citation. |

### `FileDetailEnum`

| 项 | 值 |
| --- | --- |
| 类型 | `string` |
| 说明 | - |

### `FileInputDetail`

| 项 | 值 |
| --- | --- |
| 类型 | `string` |
| 说明 | - |

### `FilePath`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A path to a file. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `file_id` | 是 | `string` | - | The ID of the file. |
| `index` | 是 | `integer` | - | The index of the file in the list of files. |
| `type` | 是 | `string` | `file_path` | The type of the file path. Always file_path. |

### `FileSearchTool`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A tool that searches for relevant content from uploaded files. Learn more about the [file search tool](https://platform.openai.com/docs/guides/tools-file-search). |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `filters` | 否 | `Filters \| null` | - | - |
| `max_num_results` | 否 | `integer` | - | The maximum number of results to return. This number should be between 1 and 50 inclusive. |
| `ranking_options` | 否 | `RankingOptions` | - | Ranking options for search. |
| `type` | 是 | `string` | `file_search` | The type of the file search tool. Always file_search. |
| `vector_store_ids` | 是 | `array<string>` | - | The IDs of the vector stores to search. |

### `FileSearchToolCall`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | The results of a file search tool call. See the [file search guide](/docs/guides/tools-file-search) for more information. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `id` | 是 | `string` | - | The unique ID of the file search tool call. |
| `queries` | 是 | `array<string>` | - | The queries used to search for files. |
| `results` | 否 | `array<object> \| null` | - | - |
| `status` | 是 | `string` | `in_progress`, `searching`, `completed`, `incomplete`, `failed` | The status of the file search tool call. One of in_progress, searching, incomplete or failed, |
| `type` | 是 | `string` | `file_search_call` | The type of the file search tool call. Always file_search_call. |

### `Filters`

| 项 | 值 |
| --- | --- |
| 类型 | `ComparisonFilter \| CompoundFilter` |
| 说明 | - |
| 组合 | `anyOf` |

| 变体 | 类型 | 说明 |
| --- | --- | --- |
| 1 | `ComparisonFilter` | - |
| 2 | `CompoundFilter` | - |

### `FunctionAndCustomToolCallOutput`

| 项 | 值 |
| --- | --- |
| 类型 | `InputTextContent \| InputImageContent \| InputFileContent` |
| 说明 | - |
| 组合 | `oneOf` |

| 变体 | 类型 | 说明 |
| --- | --- | --- |
| 1 | `InputTextContent` | - |
| 2 | `InputImageContent` | - |
| 3 | `InputFileContent` | - |

### `FunctionCallItemStatus`

| 项 | 值 |
| --- | --- |
| 类型 | `string` |
| 说明 | - |

### `FunctionCallOutputItemParam`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | The output of a function tool call. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `call_id` | 是 | `string` | - | The unique ID of the function tool call generated by the model. |
| `id` | 否 | `string \| null` | - | - |
| `output` | 是 | `string \| array<InputTextContentParam \| InputImageContentParamAutoParam \| InputFileContentParam>` | - | Text, image, or file output of the function tool call. |
| `status` | 否 | `FunctionCallItemStatus \| null` | - | - |
| `type` | 是 | `string` | `function_call_output` | The type of the function tool call output. Always function_call_output. |

### `FunctionCallOutputStatusEnum`

| 项 | 值 |
| --- | --- |
| 类型 | `string` |
| 说明 | - |

### `FunctionCallStatus`

| 项 | 值 |
| --- | --- |
| 类型 | `string` |
| 说明 | - |

### `FunctionObject`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | - |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `description` | 否 | `string` | - | A description of what the function does, used by the model to choose when and how to call the function. |
| `name` | 是 | `string` | - | The name of the function to be called. Must be a-z, A-Z, 0-9, or contain underscores and dashes, with a maximum length of 64. |
| `parameters` | 否 | `FunctionParameters` | - | - |
| `strict` | 否 | `boolean \| null` | - | - |

### `FunctionParameters`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | The parameters the functions accepts, described as a JSON Schema object. See the [guide](/docs/guides/function-calling) for examples, and the [JSON Schema reference](https://json-… |

Additional properties: `任意 JSON 值`

### `FunctionShellAction`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Execute a shell command. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `commands` | 是 | `array<string>` | - | - |
| `max_output_length` | 是 | `integer \| null` | - | - |
| `timeout_ms` | 是 | `integer \| null` | - | - |

### `FunctionShellActionParam`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Commands and limits describing how to run the shell tool call. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `commands` | 是 | `array<string>` | - | Ordered shell commands for the execution environment to run. |
| `max_output_length` | 否 | `integer \| null` | - | - |
| `timeout_ms` | 否 | `integer \| null` | - | - |

### `FunctionShellCall`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A tool call that executes one or more shell commands in a managed environment. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `action` | 是 | `FunctionShellAction` | - | The shell commands and limits that describe how to run the tool call. |
| `call_id` | 是 | `string` | - | The unique ID of the shell tool call generated by the model. |
| `created_by` | 否 | `string` | - | The ID of the entity that created this tool call. |
| `environment` | 是 | `LocalEnvironmentResource \| ContainerReferenceResource \| null` | - | - |
| `id` | 是 | `string` | - | The unique ID of the shell tool call. Populated when this item is returned via API. |
| `status` | 是 | `FunctionShellCallStatus` | - | The status of the shell call. One of in_progress, completed, or incomplete. |
| `type` | 是 | `string` | `shell_call` | The type of the item. Always shell_call. |

### `FunctionShellCallItemParam`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A tool representing a request to execute one or more shell commands. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `action` | 是 | `FunctionShellActionParam` | - | The shell commands and limits that describe how to run the tool call. |
| `call_id` | 是 | `string` | - | The unique ID of the shell tool call generated by the model. |
| `environment` | 否 | `LocalEnvironmentParam \| ContainerReferenceParam \| null` | - | - |
| `id` | 否 | `string \| null` | - | - |
| `status` | 否 | `FunctionShellCallItemStatus \| null` | - | - |
| `type` | 是 | `string` | `shell_call` | The type of the item. Always shell_call. |

### `FunctionShellCallItemStatus`

| 项 | 值 |
| --- | --- |
| 类型 | `string` |
| 说明 | Status values reported for shell tool calls. |

### `FunctionShellCallOutput`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | The output of a shell tool call that was emitted. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `call_id` | 是 | `string` | - | The unique ID of the shell tool call generated by the model. |
| `created_by` | 否 | `string` | - | The identifier of the actor that created the item. |
| `id` | 是 | `string` | - | The unique ID of the shell call output. Populated when this item is returned via API. |
| `max_output_length` | 是 | `integer \| null` | - | - |
| `output` | 是 | `array<FunctionShellCallOutputContent>` | - | An array of shell call output contents |
| `status` | 是 | `FunctionShellCallOutputStatusEnum` | - | The status of the shell call output. One of in_progress, completed, or incomplete. |
| `type` | 是 | `string` | `shell_call_output` | The type of the shell call output. Always shell_call_output. |

### `FunctionShellCallOutputContent`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | The content of a shell tool call output that was emitted. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `created_by` | 否 | `string` | - | The identifier of the actor that created the item. |
| `outcome` | 是 | `FunctionShellCallOutputTimeoutOutcome \| FunctionShellCallOutputExitOutcome` | - | Represents either an exit outcome (with an exit code) or a timeout outcome for a shell call output chunk. |
| `stderr` | 是 | `string` | - | The standard error output that was captured. |
| `stdout` | 是 | `string` | - | The standard output that was captured. |

### `FunctionShellCallOutputContentParam`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Captured stdout and stderr for a portion of a shell tool call output. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `outcome` | 是 | `FunctionShellCallOutputOutcomeParam` | - | The exit or timeout outcome associated with this shell call. |
| `stderr` | 是 | `string` | - | Captured stderr output for the shell call. |
| `stdout` | 是 | `string` | - | Captured stdout output for the shell call. |

### `FunctionShellCallOutputExitOutcome`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Indicates that the shell commands finished and returned an exit code. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `exit_code` | 是 | `integer` | - | Exit code from the shell process. |
| `type` | 是 | `string` | `exit` | The outcome type. Always exit. |

### `FunctionShellCallOutputExitOutcomeParam`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Indicates that the shell commands finished and returned an exit code. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `exit_code` | 是 | `integer` | - | The exit code returned by the shell process. |
| `type` | 是 | `string` | `exit` | The outcome type. Always exit. |

### `FunctionShellCallOutputItemParam`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | The streamed output items emitted by a shell tool call. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `call_id` | 是 | `string` | - | The unique ID of the shell tool call generated by the model. |
| `id` | 否 | `string \| null` | - | - |
| `max_output_length` | 否 | `integer \| null` | - | - |
| `output` | 是 | `array<FunctionShellCallOutputContentParam>` | - | Captured chunks of stdout and stderr output, along with their associated outcomes. |
| `status` | 否 | `FunctionShellCallItemStatus \| null` | - | - |
| `type` | 是 | `string` | `shell_call_output` | The type of the item. Always shell_call_output. |

### `FunctionShellCallOutputOutcomeParam`

| 项 | 值 |
| --- | --- |
| 类型 | `FunctionShellCallOutputTimeoutOutcomeParam \| FunctionShellCallOutputExitOutcomeParam` |
| 说明 | The exit or timeout outcome associated with this shell call. |
| 组合 | `oneOf` |

| 变体 | 类型 | 说明 |
| --- | --- | --- |
| 1 | `FunctionShellCallOutputTimeoutOutcomeParam` | - |
| 2 | `FunctionShellCallOutputExitOutcomeParam` | - |

### `FunctionShellCallOutputStatusEnum`

| 项 | 值 |
| --- | --- |
| 类型 | `string` |
| 说明 | - |

### `FunctionShellCallOutputTimeoutOutcome`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Indicates that the shell call exceeded its configured time limit. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `type` | 是 | `string` | `timeout` | The outcome type. Always timeout. |

### `FunctionShellCallOutputTimeoutOutcomeParam`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Indicates that the shell call exceeded its configured time limit. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `type` | 是 | `string` | `timeout` | The outcome type. Always timeout. |

### `FunctionShellCallStatus`

| 项 | 值 |
| --- | --- |
| 类型 | `string` |
| 说明 | - |

### `FunctionShellToolParam`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A tool that allows the model to execute shell commands. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `environment` | 否 | `ContainerAutoParam \| LocalEnvironmentParam \| ContainerReferenceParam \| null` | - | - |
| `type` | 是 | `string` | `shell` | The type of the shell tool. Always shell. |

### `FunctionTool`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Defines a function in your own code the model can choose to call. Learn more about [function calling](https://platform.openai.com/docs/guides/function-calling). |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `defer_loading` | 否 | `boolean` | - | Whether this function is deferred and loaded via tool search. |
| `description` | 否 | `string \| null` | - | - |
| `name` | 是 | `string` | - | The name of the function to call. |
| `parameters` | 是 | `object/map<string, object/value> \| null` | - | - |
| `strict` | 是 | `boolean \| null` | - | - |
| `type` | 是 | `string` | `function` | The type of the function tool. Always function. |

### `FunctionToolCall`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A tool call to run a function. See the [function calling guide](/docs/guides/function-calling) for more information. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `arguments` | 是 | `string` | - | A JSON string of the arguments to pass to the function. |
| `call_id` | 是 | `string` | - | The unique ID of the function tool call generated by the model. |
| `id` | 否 | `string` | - | The unique ID of the function tool call. |
| `name` | 是 | `string` | - | The name of the function to run. |
| `namespace` | 否 | `string` | - | The namespace of the function to run. |
| `status` | 否 | `string` | `in_progress`, `completed`, `incomplete` | The status of the item. One of in_progress, completed, or incomplete. Populated when items are returned via API. |
| `type` | 是 | `string` | `function_call` | The type of the function tool call. Always function_call. |

### `FunctionToolCallOutput`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | The output of a function tool call. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `call_id` | 是 | `string` | - | The unique ID of the function tool call generated by the model. |
| `id` | 否 | `string` | - | The unique ID of the function tool call output. Populated when this item is returned via API. |
| `output` | 是 | `string \| array<FunctionAndCustomToolCallOutput>` | - | The output from the function call generated by your code. Can be a string or an list of output content. |
| `status` | 否 | `string` | `in_progress`, `completed`, `incomplete` | The status of the item. One of in_progress, completed, or incomplete. Populated when items are returned via API. |
| `type` | 是 | `string` | `function_call_output` | The type of the function tool call output. Always function_call_output. |

### `FunctionToolCallOutputResource`

| 项 | 值 |
| --- | --- |
| 类型 | `FunctionToolCallOutput & object` |
| 说明 | - |
| 组合 | `allOf` |

| 变体 | 类型 | 说明 |
| --- | --- | --- |
| 1 | `FunctionToolCallOutput` | - |
| 2 | `object` | - |

#### allOf 展开字段

| 字段 | 必填 | 类型 | 枚举/常量 | 来源 | 说明 |
| --- | --- | --- | --- | --- | --- |
| `call_id` | 是 | `string` | - | `FunctionToolCallOutput` | The unique ID of the function tool call generated by the model. |
| `created_by` | 否 | `string` | - | `FunctionToolCallOutputResource.allOf[2]` | The identifier of the actor that created the item. |
| `id` | 是 | `string` | - | `FunctionToolCallOutput` | The unique ID of the function tool call output. Populated when this item is returned via API. |
| `output` | 是 | `string \| array<FunctionAndCustomToolCallOutput>` | - | `FunctionToolCallOutput` | The output from the function call generated by your code. Can be a string or an list of output content. |
| `status` | 是 | `string` | `in_progress`, `completed`, `incomplete` | `FunctionToolCallOutput` | The status of the item. One of in_progress, completed, or incomplete. Populated when items are returned via API. |
| `type` | 是 | `string` | `function_call_output` | `FunctionToolCallOutput` | The type of the function tool call output. Always function_call_output. |

### `FunctionToolParam`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | - |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `defer_loading` | 否 | `boolean` | - | Whether this function should be deferred and discovered via tool search. |
| `description` | 否 | `string \| null` | - | - |
| `name` | 是 | `string` | - | - |
| `parameters` | 否 | `EmptyModelParam \| null` | - | - |
| `strict` | 否 | `boolean \| null` | - | - |
| `type` | 是 | `string` | `function` | - |

### `GrammarSyntax1`

| 项 | 值 |
| --- | --- |
| 类型 | `string` |
| 说明 | - |

### `HybridSearchOptions`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | - |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `embedding_weight` | 是 | `number` | - | The weight of the embedding in the reciprocal ranking fusion. |
| `text_weight` | 是 | `number` | - | The weight of the text in the reciprocal ranking fusion. |

### `Image`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Represents the content or the URL of an image generated by the OpenAI API. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `b64_json` | 否 | `string` | - | The base64-encoded JSON of the generated image. Returned by default for the GPT image models, and only present if response_format is set to b64_json for dall-e-2 and dall-e-3. |
| `revised_prompt` | 否 | `string` | - | For dall-e-3 only, the revised prompt that was used to generate the image. |
| `url` | 否 | `string(uri)` | - | When using dall-e-2 or dall-e-3, the URL of the generated image if response_format is set to url (default value). Unsupported for the GPT image models. |

### `ImageDetail`

| 项 | 值 |
| --- | --- |
| 类型 | `string` |
| 说明 | - |

### `ImageEditCompletedEvent`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Emitted when image editing has completed and the final image is available. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `b64_json` | 是 | `string` | - | Base64-encoded final edited image data, suitable for rendering as an image. |
| `background` | 是 | `string` | `transparent`, `opaque`, `auto` | The background setting for the edited image. |
| `created_at` | 是 | `integer(unixtime)` | - | The Unix timestamp when the event was created. |
| `output_format` | 是 | `string` | `png`, `webp`, `jpeg` | The output format for the edited image. |
| `quality` | 是 | `string` | `low`, `medium`, `high`, `auto` | The quality setting for the edited image. |
| `size` | 是 | `string` | `1024x1024`, `1024x1536`, `1536x1024`, `auto` | The size of the edited image. |
| `type` | 是 | `string` | `image_edit.completed` | The type of the event. Always image_edit.completed. |
| `usage` | 是 | `ImagesUsage` | - | - |

### `ImageEditPartialImageEvent`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Emitted when a partial image is available during image editing streaming. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `b64_json` | 是 | `string` | - | Base64-encoded partial image data, suitable for rendering as an image. |
| `background` | 是 | `string` | `transparent`, `opaque`, `auto` | The background setting for the requested edited image. |
| `created_at` | 是 | `integer(unixtime)` | - | The Unix timestamp when the event was created. |
| `output_format` | 是 | `string` | `png`, `webp`, `jpeg` | The output format for the requested edited image. |
| `partial_image_index` | 是 | `integer` | - | 0-based index for the partial image (streaming). |
| `quality` | 是 | `string` | `low`, `medium`, `high`, `auto` | The quality setting for the requested edited image. |
| `size` | 是 | `string` | `1024x1024`, `1024x1536`, `1536x1024`, `auto` | The size of the requested edited image. |
| `type` | 是 | `string` | `image_edit.partial_image` | The type of the event. Always image_edit.partial_image. |

### `ImageEditStreamEvent`

| 项 | 值 |
| --- | --- |
| 类型 | `ImageEditPartialImageEvent \| ImageEditCompletedEvent` |
| 说明 | - |
| 组合 | `anyOf` |

| 变体 | 类型 | 说明 |
| --- | --- | --- |
| 1 | `ImageEditPartialImageEvent` | - |
| 2 | `ImageEditCompletedEvent` | - |

### `ImageGenActionEnum`

| 项 | 值 |
| --- | --- |
| 类型 | `string` |
| 说明 | - |

### `ImageGenCompletedEvent`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Emitted when image generation has completed and the final image is available. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `b64_json` | 是 | `string` | - | Base64-encoded image data, suitable for rendering as an image. |
| `background` | 是 | `string` | `transparent`, `opaque`, `auto` | The background setting for the generated image. |
| `created_at` | 是 | `integer(unixtime)` | - | The Unix timestamp when the event was created. |
| `output_format` | 是 | `string` | `png`, `webp`, `jpeg` | The output format for the generated image. |
| `quality` | 是 | `string` | `low`, `medium`, `high`, `auto` | The quality setting for the generated image. |
| `size` | 是 | `string` | `1024x1024`, `1024x1536`, `1536x1024`, `auto` | The size of the generated image. |
| `type` | 是 | `string` | `image_generation.completed` | The type of the event. Always image_generation.completed. |
| `usage` | 是 | `ImagesUsage` | - | - |

### `ImageGenInputUsageDetails`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | The input tokens detailed information for the image generation. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `image_tokens` | 是 | `integer` | - | The number of image tokens in the input prompt. |
| `text_tokens` | 是 | `integer` | - | The number of text tokens in the input prompt. |

### `ImageGenOutputTokensDetails`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | The output token details for the image generation. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `image_tokens` | 是 | `integer` | - | The number of image output tokens generated by the model. |
| `text_tokens` | 是 | `integer` | - | The number of text output tokens generated by the model. |

### `ImageGenPartialImageEvent`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Emitted when a partial image is available during image generation streaming. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `b64_json` | 是 | `string` | - | Base64-encoded partial image data, suitable for rendering as an image. |
| `background` | 是 | `string` | `transparent`, `opaque`, `auto` | The background setting for the requested image. |
| `created_at` | 是 | `integer(unixtime)` | - | The Unix timestamp when the event was created. |
| `output_format` | 是 | `string` | `png`, `webp`, `jpeg` | The output format for the requested image. |
| `partial_image_index` | 是 | `integer` | - | 0-based index for the partial image (streaming). |
| `quality` | 是 | `string` | `low`, `medium`, `high`, `auto` | The quality setting for the requested image. |
| `size` | 是 | `string` | `1024x1024`, `1024x1536`, `1536x1024`, `auto` | The size of the requested image. |
| `type` | 是 | `string` | `image_generation.partial_image` | The type of the event. Always image_generation.partial_image. |

### `ImageGenStreamEvent`

| 项 | 值 |
| --- | --- |
| 类型 | `ImageGenPartialImageEvent \| ImageGenCompletedEvent` |
| 说明 | - |
| 组合 | `anyOf` |

| 变体 | 类型 | 说明 |
| --- | --- | --- |
| 1 | `ImageGenPartialImageEvent` | - |
| 2 | `ImageGenCompletedEvent` | - |

### `ImageGenTool`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A tool that generates images using the GPT image models. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `action` | 否 | `ImageGenActionEnum` | - | Whether to generate a new image or edit an existing image. Default: auto. |
| `background` | 否 | `string` | `transparent`, `opaque`, `auto` | Background type for the generated image. One of transparent, opaque, or auto. Default: auto. |
| `input_fidelity` | 否 | `InputFidelity \| null` | - | - |
| `input_image_mask` | 否 | `object` | - | Optional mask for inpainting. Contains image_url (string, optional) and file_id (string, optional). |
| `model` | 否 | `string \| string` | - | - |
| `moderation` | 否 | `string` | `auto`, `low` | Moderation level for the generated image. Default: auto. |
| `output_compression` | 否 | `integer` | - | Compression level for the output image. Default: 100. |
| `output_format` | 否 | `string` | `png`, `webp`, `jpeg` | The output format of the generated image. One of png, webp, or jpeg. Default: png. |
| `partial_images` | 否 | `integer` | - | Number of partial images to generate in streaming mode, from 0 (default value) to 3. |
| `quality` | 否 | `string` | `low`, `medium`, `high`, `auto` | The quality of the generated image. One of low, medium, high, or auto. Default: auto. |
| `size` | 否 | `string \| string` | - | The size of the generated images. For gpt-image-2 and gpt-image-2-2026-04-21, arbitrary resolutions are supported as WIDTHxHEIGHT strings, for example 1536x864. Width and height m… |
| `type` | 是 | `string` | `image_generation` | The type of the image generation tool. Always image_generation. |

### `ImageGenToolCall`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | An image generation request made by the model. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `id` | 是 | `string` | - | The unique ID of the image generation call. |
| `result` | 是 | `string \| null` | - | - |
| `status` | 是 | `string` | `in_progress`, `completed`, `generating`, `failed` | The status of the image generation call. |
| `type` | 是 | `string` | `image_generation_call` | The type of the image generation call. Always image_generation_call. |

### `ImageGenUsage`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | For gpt-image-1 only, the token usage information for the image generation. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `input_tokens` | 是 | `integer` | - | The number of tokens (images and text) in the input prompt. |
| `input_tokens_details` | 是 | `ImageGenInputUsageDetails` | - | - |
| `output_tokens` | 是 | `integer` | - | The number of output tokens generated by the model. |
| `output_tokens_details` | 否 | `ImageGenOutputTokensDetails` | - | - |
| `total_tokens` | 是 | `integer` | - | The total number of tokens (images and text) used for the image generation. |

### `ImageRefParam`

| 项 | 值 |
| --- | --- |
| 类型 | `object/value \| object/value` |
| 说明 | Reference an input image by either URL or uploaded file ID. Provide exactly one of image_url or file_id. |
| 组合 | `anyOf` |

| 变体 | 类型 | 说明 |
| --- | --- | --- |
| 1 | `object/value` | - |
| 2 | `object/value` | - |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `file_id` | 否 | `string` | - | The File API ID of an uploaded image to use as input. |
| `image_url` | 否 | `string(uri)` | - | A fully qualified URL or base64-encoded data URL. |

### `ImagesResponse`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | The response from the image generation endpoint. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `background` | 否 | `string` | `transparent`, `opaque` | The background parameter used for the image generation. Either transparent or opaque. |
| `created` | 是 | `integer(unixtime)` | - | The Unix timestamp (in seconds) of when the image was created. |
| `data` | 否 | `array<Image>` | - | The list of generated images. |
| `output_format` | 否 | `string` | `png`, `webp`, `jpeg` | The output format of the image generation. Either png, webp, or jpeg. |
| `quality` | 否 | `string` | `low`, `medium`, `high` | The quality of the image generated. Either low, medium, or high. |
| `size` | 否 | `string` | `1024x1024`, `1024x1536`, `1536x1024` | The size of the image generated. Either 1024x1024, 1024x1536, or 1536x1024. |
| `usage` | 否 | `ImageGenUsage` | - | - |

### `ImagesUsage`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | For the GPT image models only, the token usage information for the image generation. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `input_tokens` | 是 | `integer` | - | The number of tokens (images and text) in the input prompt. |
| `input_tokens_details` | 是 | `object` | - | The input tokens detailed information for the image generation. |
| `output_tokens` | 是 | `integer` | - | The number of image tokens in the output image. |
| `total_tokens` | 是 | `integer` | - | The total number of tokens (images and text) used for the image generation. |

### `IncludeEnum`

| 项 | 值 |
| --- | --- |
| 类型 | `string` |
| 说明 | Specify additional output data to include in the model response. Currently supported values are: - web_search_call.results: Include the search results of the web search tool call.… |

### `InlineSkillParam`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | - |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `description` | 是 | `string` | - | The description of the skill. |
| `name` | 是 | `string` | - | The name of the skill. |
| `source` | 是 | `InlineSkillSourceParam` | - | Inline skill payload |
| `type` | 是 | `string` | `inline` | Defines an inline skill for this request. |

### `InlineSkillSourceParam`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Inline skill payload |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `data` | 是 | `string` | - | Base64-encoded skill zip bundle. |
| `media_type` | 是 | `string` | `application/zip` | The media type of the inline skill payload. Must be application/zip. |
| `type` | 是 | `string` | `base64` | The type of the inline skill source. Must be base64. |

### `InputContent`

| 项 | 值 |
| --- | --- |
| 类型 | `InputTextContent \| InputImageContent \| InputFileContent` |
| 说明 | - |
| 组合 | `oneOf` |

| 变体 | 类型 | 说明 |
| --- | --- | --- |
| 1 | `InputTextContent` | - |
| 2 | `InputImageContent` | - |
| 3 | `InputFileContent` | - |

### `InputFidelity`

| 项 | 值 |
| --- | --- |
| 类型 | `string` |
| 说明 | Control how much effort the model will exert to match the style and features, especially facial features, of input images. This parameter is only supported for gpt-image-1 and gpt… |

### `InputFileContent`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A file input to the model. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `detail` | 否 | `FileInputDetail` | - | The detail level of the file to be sent to the model. Use low for the default rendering behavior, or high to render the file at higher quality. Defaults to low. |
| `file_data` | 否 | `string` | - | The content of the file to be sent to the model. |
| `file_id` | 否 | `string \| null` | - | - |
| `file_url` | 否 | `string(uri)` | - | The URL of the file to be sent to the model. |
| `filename` | 否 | `string` | - | The name of the file to be sent to the model. |
| `type` | 是 | `string` | `input_file` | The type of the input item. Always input_file. |

### `InputFileContentParam`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A file input to the model. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `detail` | 否 | `FileDetailEnum` | - | The detail level of the file to be sent to the model. Use low for the default rendering behavior, or high to render the file at higher quality. Defaults to low. |
| `file_data` | 否 | `string \| null` | - | - |
| `file_id` | 否 | `string \| null` | - | - |
| `file_url` | 否 | `string(uri) \| null` | - | - |
| `filename` | 否 | `string \| null` | - | - |
| `type` | 是 | `string` | `input_file` | The type of the input item. Always input_file. |

### `InputImageContent`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | An image input to the model. Learn about [image inputs](/docs/guides/vision). |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `detail` | 是 | `ImageDetail` | - | The detail level of the image to be sent to the model. One of high, low, auto, or original. Defaults to auto. |
| `file_id` | 否 | `string \| null` | - | - |
| `image_url` | 否 | `string(uri) \| null` | - | - |
| `type` | 是 | `string` | `input_image` | The type of the input item. Always input_image. |

### `InputImageContentParamAutoParam`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | An image input to the model. Learn about [image inputs](/docs/guides/vision) |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `detail` | 否 | `DetailEnum \| null` | - | - |
| `file_id` | 否 | `string \| null` | - | - |
| `image_url` | 否 | `string(uri) \| null` | - | - |
| `type` | 是 | `string` | `input_image` | The type of the input item. Always input_image. |

### `InputItem`

| 项 | 值 |
| --- | --- |
| 类型 | `EasyInputMessage \| Item \| CompactionTriggerItemParam \| ItemReferenceParam` |
| 说明 | - |
| 组合 | `oneOf` |

| 变体 | 类型 | 说明 |
| --- | --- | --- |
| 1 | `EasyInputMessage` | - |
| 2 | `Item` | An item representing part of the context for the response to be generated by the model. Can contain text, images, and audio inputs, as well as previous assistant responses and too… |
| 3 | `CompactionTriggerItemParam` | - |
| 4 | `ItemReferenceParam` | - |

### `InputMessage`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A message input to the model with a role indicating instruction following hierarchy. Instructions given with the developer or system role take precedence over instructions given w… |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `content` | 是 | `InputMessageContentList` | - | - |
| `role` | 是 | `string` | `user`, `system`, `developer` | The role of the message input. One of user, system, or developer. |
| `status` | 否 | `string` | `in_progress`, `completed`, `incomplete` | The status of item. One of in_progress, completed, or incomplete. Populated when items are returned via API. |
| `type` | 否 | `string` | `message` | The type of the message input. Always set to message. |

### `InputMessageContentList`

| 项 | 值 |
| --- | --- |
| 类型 | `array<InputContent>` |
| 说明 | A list of one or many input items to the model, containing different content types. |

### `InputParam`

| 项 | 值 |
| --- | --- |
| 类型 | `string \| array<InputItem>` |
| 说明 | Text, image, or file inputs to the model, used to generate a response. Learn more: - [Text inputs and outputs](/docs/guides/text) - [Image inputs](/docs/guides/images) - [File inp… |
| 组合 | `oneOf` |

| 变体 | 类型 | 说明 |
| --- | --- | --- |
| 1 | `string` | A text input to the model, equivalent to a text input with the user role. |
| 2 | `array<InputItem>` | A list of one or many input items to the model, containing different content types. |

### `InputTextContent`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A text input to the model. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `text` | 是 | `string` | - | The text input to the model. |
| `type` | 是 | `string` | `input_text` | The type of the input item. Always input_text. |

### `InputTextContentParam`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A text input to the model. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `text` | 是 | `string` | - | The text input to the model. |
| `type` | 是 | `string` | `input_text` | The type of the input item. Always input_text. |

### `Item`

| 项 | 值 |
| --- | --- |
| 类型 | `InputMessage \| OutputMessage \| FileSearchToolCall \| ComputerToolCall \| ComputerCallOutputItemParam \| WebSearchToolCall \| FunctionToolCall \| FunctionCallOutputItemParam … (+19)` |
| 说明 | Content item used to generate a response. |
| 组合 | `oneOf` |

| 变体 | 类型 | 说明 |
| --- | --- | --- |
| 1 | `InputMessage` | - |
| 2 | `OutputMessage` | - |
| 3 | `FileSearchToolCall` | - |
| 4 | `ComputerToolCall` | - |
| 5 | `ComputerCallOutputItemParam` | - |
| 6 | `WebSearchToolCall` | - |
| 7 | `FunctionToolCall` | - |
| 8 | `FunctionCallOutputItemParam` | - |
| 9 | `ToolSearchCallItemParam` | - |
| 10 | `ToolSearchOutputItemParam` | - |
| 11 | `AdditionalToolsItemParam` | - |
| 12 | `ReasoningItem` | - |
| 13 | `CompactionSummaryItemParam` | - |
| 14 | `ImageGenToolCall` | - |
| 15 | `CodeInterpreterToolCall` | - |
| 16 | `LocalShellToolCall` | - |
| 17 | `LocalShellToolCallOutput` | - |
| 18 | `FunctionShellCallItemParam` | - |
| 19 | `FunctionShellCallOutputItemParam` | - |
| 20 | `ApplyPatchToolCallItemParam` | - |
| 21 | `ApplyPatchToolCallOutputItemParam` | - |
| 22 | `MCPListTools` | - |
| 23 | `MCPApprovalRequest` | - |
| 24 | `MCPApprovalResponse` | - |
| 25 | `MCPToolCall` | - |
| 26 | `CustomToolCallOutput` | - |
| 27 | `CustomToolCall` | - |

### `ItemField`

| 项 | 值 |
| --- | --- |
| 类型 | `Message \| FunctionToolCall \| ToolSearchCall \| ToolSearchOutput \| AdditionalTools \| FunctionToolCallOutput \| FileSearchToolCall \| WebSearchToolCall … (+18)` |
| 说明 | An item representing a message, tool call, tool output, reasoning, or other response element. |
| 组合 | `oneOf` |

| 变体 | 类型 | 说明 |
| --- | --- | --- |
| 1 | `Message` | - |
| 2 | `FunctionToolCall` | - |
| 3 | `ToolSearchCall` | - |
| 4 | `ToolSearchOutput` | - |
| 5 | `AdditionalTools` | - |
| 6 | `FunctionToolCallOutput` | - |
| 7 | `FileSearchToolCall` | - |
| 8 | `WebSearchToolCall` | - |
| 9 | `ImageGenToolCall` | - |
| 10 | `ComputerToolCall` | - |
| 11 | `ComputerToolCallOutputResource` | - |
| 12 | `ReasoningItem` | - |
| 13 | `CompactionBody` | - |
| 14 | `CodeInterpreterToolCall` | - |
| 15 | `LocalShellToolCall` | - |
| 16 | `LocalShellToolCallOutput` | - |
| 17 | `FunctionShellCall` | - |
| 18 | `FunctionShellCallOutput` | - |
| 19 | `ApplyPatchToolCall` | - |
| 20 | `ApplyPatchToolCallOutput` | - |
| 21 | `MCPListTools` | - |
| 22 | `MCPApprovalRequest` | - |
| 23 | `MCPApprovalResponseResource` | - |
| 24 | `MCPToolCall` | - |
| 25 | `CustomToolCall` | - |
| 26 | `CustomToolCallOutput` | - |

### `ItemReferenceParam`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | An internal identifier for an item to reference. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `id` | 是 | `string` | - | The ID of the item to reference. |
| `type` | 否 | `string \| null` | - | - |

### `KeyPressAction`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A collection of keypresses the model would like to perform. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `keys` | 是 | `array<string>` | - | The combination of keys the model is requesting to be pressed. This is an array of strings, each representing a key. |
| `type` | 是 | `string` | `keypress` | Specifies the event type. For a keypress action, this property is always set to keypress. |

### `LocalEnvironmentParam`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | - |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `skills` | 否 | `array<LocalSkillParam>` | - | An optional list of skills. |
| `type` | 是 | `string` | `local` | Use a local computer environment. |

### `LocalEnvironmentResource`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Represents the use of a local environment to perform shell actions. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `type` | 是 | `string` | `local` | The environment type. Always local. |

### `LocalShellExecAction`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Execute a shell command on the server. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `command` | 是 | `array<string>` | - | The command to run. |
| `env` | 是 | `object/map<string, string>` | - | Environment variables to set for the command. |
| `timeout_ms` | 否 | `integer \| null` | - | - |
| `type` | 是 | `string` | `exec` | The type of the local shell action. Always exec. |
| `user` | 否 | `string \| null` | - | - |
| `working_directory` | 否 | `string \| null` | - | - |

### `LocalShellToolCall`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A tool call to run a command on the local shell. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `action` | 是 | `LocalShellExecAction` | - | - |
| `call_id` | 是 | `string` | - | The unique ID of the local shell tool call generated by the model. |
| `id` | 是 | `string` | - | The unique ID of the local shell call. |
| `status` | 是 | `string` | `in_progress`, `completed`, `incomplete` | The status of the local shell call. |
| `type` | 是 | `string` | `local_shell_call` | The type of the local shell call. Always local_shell_call. |

### `LocalShellToolCallOutput`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | The output of a local shell tool call. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `id` | 是 | `string` | - | The unique ID of the local shell tool call generated by the model. |
| `output` | 是 | `string` | - | A JSON string of the output of the local shell tool call. |
| `status` | 否 | `string \| null` | - | - |
| `type` | 是 | `string` | `local_shell_call_output` | The type of the local shell tool call output. Always local_shell_call_output. |

### `LocalShellToolParam`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A tool that allows the model to execute shell commands in a local environment. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `type` | 是 | `string` | `local_shell` | The type of the local shell tool. Always local_shell. |

### `LocalSkillParam`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | - |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `description` | 是 | `string` | - | The description of the skill. |
| `name` | 是 | `string` | - | The name of the skill. |
| `path` | 是 | `string` | - | The path to the directory containing the skill. |

### `LogProb`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | The log probability of a token. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `bytes` | 是 | `array<integer>` | - | - |
| `logprob` | 是 | `number` | - | - |
| `token` | 是 | `string` | - | - |
| `top_logprobs` | 是 | `array<TopLogProb>` | - | - |

### `MCPApprovalRequest`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A request for human approval of a tool invocation. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `arguments` | 是 | `string` | - | A JSON string of arguments for the tool. |
| `id` | 是 | `string` | - | The unique ID of the approval request. |
| `name` | 是 | `string` | - | The name of the tool to run. |
| `server_label` | 是 | `string` | - | The label of the MCP server making the request. |
| `type` | 是 | `string` | `mcp_approval_request` | The type of the item. Always mcp_approval_request. |

### `MCPApprovalResponse`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A response to an MCP approval request. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `approval_request_id` | 是 | `string` | - | The ID of the approval request being answered. |
| `approve` | 是 | `boolean` | - | Whether the request was approved. |
| `id` | 否 | `string \| null` | - | - |
| `reason` | 否 | `string \| null` | - | - |
| `type` | 是 | `string` | `mcp_approval_response` | The type of the item. Always mcp_approval_response. |

### `MCPApprovalResponseResource`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A response to an MCP approval request. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `approval_request_id` | 是 | `string` | - | The ID of the approval request being answered. |
| `approve` | 是 | `boolean` | - | Whether the request was approved. |
| `id` | 是 | `string` | - | The unique ID of the approval response |
| `reason` | 否 | `string \| null` | - | - |
| `type` | 是 | `string` | `mcp_approval_response` | The type of the item. Always mcp_approval_response. |

### `MCPListTools`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A list of tools available on an MCP server. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `error` | 否 | `string \| null` | - | - |
| `id` | 是 | `string` | - | The unique ID of the list. |
| `server_label` | 是 | `string` | - | The label of the MCP server. |
| `tools` | 是 | `array<MCPListToolsTool>` | - | The tools available on the server. |
| `type` | 是 | `string` | `mcp_list_tools` | The type of the item. Always mcp_list_tools. |

### `MCPListToolsTool`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A tool available on an MCP server. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `annotations` | 否 | `object \| null` | - | - |
| `description` | 否 | `string \| null` | - | - |
| `input_schema` | 是 | `object` | - | The JSON schema describing the tool's input. |
| `name` | 是 | `string` | - | The name of the tool. |

### `MCPTool`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Give the model access to additional tools via remote Model Context Protocol (MCP) servers. [Learn more about MCP](/docs/guides/tools-remote-mcp). |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `allowed_tools` | 否 | `array<string> \| MCPToolFilter \| null` | - | - |
| `authorization` | 否 | `string` | - | An OAuth access token that can be used with a remote MCP server, either with a custom MCP server URL or a service connector. Your application must handle the OAuth authorization f… |
| `connector_id` | 否 | `string` | `connector_dropbox`, `connector_gmail`, `connector_googlecalendar`, `connector_googledrive`, `connector_microsoftteams`, `connector_outlookcalendar`, `connector_outlookemail`, `connector_sharepoint` | Identifier for service connectors, like those available in ChatGPT. One of server_url or connector_id must be provided. Learn more about service connectors [here](/docs/guides/too… |
| `defer_loading` | 否 | `boolean` | - | Whether this MCP tool is deferred and discovered via tool search. |
| `headers` | 否 | `object/map<string, string> \| null` | - | - |
| `require_approval` | 否 | `object \| string \| null` | - | - |
| `server_description` | 否 | `string` | - | Optional description of the MCP server, used to provide more context. |
| `server_label` | 是 | `string` | - | A label for this MCP server, used to identify it in tool calls. |
| `server_url` | 否 | `string(uri)` | - | The URL for the MCP server. One of server_url or connector_id must be provided. |
| `type` | 是 | `string` | `mcp` | The type of the MCP tool. Always mcp. |

### `MCPToolCall`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | An invocation of a tool on an MCP server. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `approval_request_id` | 否 | `string \| null` | - | - |
| `arguments` | 是 | `string` | - | A JSON string of the arguments passed to the tool. |
| `error` | 否 | `string \| null` | - | - |
| `id` | 是 | `string` | - | The unique ID of the tool call. |
| `name` | 是 | `string` | - | The name of the tool that was run. |
| `output` | 否 | `string \| null` | - | - |
| `server_label` | 是 | `string` | - | The label of the MCP server running the tool. |
| `status` | 否 | `MCPToolCallStatus` | - | The status of the tool call. One of in_progress, completed, incomplete, calling, or failed. |
| `type` | 是 | `string` | `mcp_call` | The type of the item. Always mcp_call. |

### `MCPToolCallStatus`

| 项 | 值 |
| --- | --- |
| 类型 | `string` |
| 说明 | - |

### `MCPToolFilter`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A filter object to specify which tools are allowed. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `read_only` | 否 | `boolean` | - | Indicates whether or not a tool modifies data or is read-only. If an MCP server is [annotated with readOnlyHint](https://modelcontextprotocol.io/specification/2025-06-18/schema#to… |
| `tool_names` | 否 | `array<string>` | - | List of allowed tool names. |

### `Message`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A message to or from the model. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `content` | 是 | `array<InputTextContent \| OutputTextContent \| TextContent \| SummaryTextContent \| ReasoningTextContent \| RefusalContent \| InputImageContent \| ComputerScreenshotContent … (+1)>` | - | The content of the message |
| `id` | 是 | `string` | - | The unique ID of the message. |
| `phase` | 否 | `MessagePhase-2 \| null` | - | - |
| `role` | 是 | `MessageRole` | - | The role of the message. One of unknown, user, assistant, system, critic, discriminator, developer, or tool. |
| `status` | 是 | `MessageStatus` | - | The status of item. One of in_progress, completed, or incomplete. Populated when items are returned via API. |
| `type` | 是 | `string` | `message` | The type of the message. Always set to message. |

### `MessagePhase`

| 项 | 值 |
| --- | --- |
| 类型 | `string` |
| 说明 | Labels an assistant message as intermediate commentary (commentary) or the final answer (final_answer). For models like gpt-5.3-codex and beyond, when sending follow-up requests, … |

### `MessagePhase-2`

| 项 | 值 |
| --- | --- |
| 类型 | `string` |
| 说明 | - |

### `MessageRole`

| 项 | 值 |
| --- | --- |
| 类型 | `string` |
| 说明 | - |

### `MessageStatus`

| 项 | 值 |
| --- | --- |
| 类型 | `string` |
| 说明 | - |

### `Metadata`

| 项 | 值 |
| --- | --- |
| 类型 | `object/map<string, string> \| null` |
| 说明 | - |
| 组合 | `anyOf` |

| 变体 | 类型 | 说明 |
| --- | --- | --- |
| 1 | `object/map<string, string>` | Set of 16 key-value pairs that can be attached to an object. This can be useful for storing additional information about the object in a structured format, and querying for object… |
| 2 | `null` | - |

### `ModelIdsCompaction`

| 项 | 值 |
| --- | --- |
| 类型 | `ModelIdsResponses \| string \| null` |
| 说明 | Model ID used to generate the response, like gpt-5 or o3. OpenAI offers a wide range of models with different capabilities, performance characteristics, and price points. Refer to… |
| 组合 | `anyOf` |

| 变体 | 类型 | 说明 |
| --- | --- | --- |
| 1 | `ModelIdsResponses` | - |
| 2 | `string` | - |
| 3 | `null` | - |

### `ModelIdsResponses`

| 项 | 值 |
| --- | --- |
| 类型 | `ModelIdsShared \| string` |
| 说明 | - |
| 组合 | `anyOf` |

| 变体 | 类型 | 说明 |
| --- | --- | --- |
| 1 | `ModelIdsShared` | - |
| 2 | `string` | - |

### `ModelIdsShared`

| 项 | 值 |
| --- | --- |
| 类型 | `string \| string` |
| 说明 | - |
| 组合 | `anyOf` |

| 变体 | 类型 | 说明 |
| --- | --- | --- |
| 1 | `string` | - |
| 2 | `string` | - |

### `ModelResponseProperties`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | - |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `metadata` | 否 | `Metadata` | - | - |
| `prompt_cache_key` | 否 | `string` | - | Used by OpenAI to cache responses for similar requests to optimize your cache hit rates. Replaces the user field. [Learn more](/docs/guides/prompt-caching). |
| `prompt_cache_retention` | 否 | `string \| null` | - | - |
| `safety_identifier` | 否 | `string` | - | A stable identifier used to help detect users of your application that may be violating OpenAI's usage policies. The IDs should be a string that uniquely identifies each user, wit… |
| `service_tier` | 否 | `ServiceTier` | - | - |
| `temperature` | 否 | `number \| null` | - | - |
| `top_logprobs` | 否 | `integer \| null` | - | - |
| `top_p` | 否 | `number \| null` | - | - |
| `user` | 否 | `string` | - | This field is being replaced by safety_identifier and prompt_cache_key. Use prompt_cache_key instead to maintain caching optimizations. A stable identifier for your end-users. Use… |

### `MoveParam`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A mouse move action. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `keys` | 否 | `array<string> \| null` | - | - |
| `type` | 是 | `string` | `move` | Specifies the event type. For a move action, this property is always set to move. |
| `x` | 是 | `integer` | - | The x-coordinate to move to. |
| `y` | 是 | `integer` | - | The y-coordinate to move to. |

### `NamespaceToolParam`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Groups function/custom tools under a shared namespace. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `description` | 是 | `string` | - | A description of the namespace shown to the model. |
| `name` | 是 | `string` | - | The namespace name used in tool calls (for example, crm). |
| `tools` | 是 | `array<FunctionToolParam \| CustomToolParam>` | - | The function/custom tools available inside this namespace. |
| `type` | 是 | `string` | `namespace` | The type of the tool. Always namespace. |

### `OutputContent`

| 项 | 值 |
| --- | --- |
| 类型 | `OutputTextContent \| RefusalContent \| ReasoningTextContent` |
| 说明 | - |
| 组合 | `oneOf` |

| 变体 | 类型 | 说明 |
| --- | --- | --- |
| 1 | `OutputTextContent` | - |
| 2 | `RefusalContent` | - |
| 3 | `ReasoningTextContent` | - |

### `OutputItem`

| 项 | 值 |
| --- | --- |
| 类型 | `OutputMessage \| FileSearchToolCall \| FunctionToolCall \| FunctionToolCallOutputResource \| WebSearchToolCall \| ComputerToolCall \| ComputerToolCallOutputResource \| ReasoningItem … (+18)` |
| 说明 | - |
| 组合 | `oneOf` |

| 变体 | 类型 | 说明 |
| --- | --- | --- |
| 1 | `OutputMessage` | - |
| 2 | `FileSearchToolCall` | - |
| 3 | `FunctionToolCall` | - |
| 4 | `FunctionToolCallOutputResource` | - |
| 5 | `WebSearchToolCall` | - |
| 6 | `ComputerToolCall` | - |
| 7 | `ComputerToolCallOutputResource` | - |
| 8 | `ReasoningItem` | - |
| 9 | `ToolSearchCall` | - |
| 10 | `ToolSearchOutput` | - |
| 11 | `AdditionalTools` | - |
| 12 | `CompactionBody` | - |
| 13 | `ImageGenToolCall` | - |
| 14 | `CodeInterpreterToolCall` | - |
| 15 | `LocalShellToolCall` | - |
| 16 | `LocalShellToolCallOutput` | - |
| 17 | `FunctionShellCall` | - |
| 18 | `FunctionShellCallOutput` | - |
| 19 | `ApplyPatchToolCall` | - |
| 20 | `ApplyPatchToolCallOutput` | - |
| 21 | `MCPToolCall` | - |
| 22 | `MCPListTools` | - |
| 23 | `MCPApprovalRequest` | - |
| 24 | `MCPApprovalResponseResource` | - |
| 25 | `CustomToolCall` | - |
| 26 | `CustomToolCallOutputResource` | - |

### `OutputMessage`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | An output message from the model. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `content` | 是 | `array<OutputMessageContent>` | - | The content of the output message. |
| `id` | 是 | `string` | - | The unique ID of the output message. |
| `phase` | 否 | `MessagePhase \| null` | - | - |
| `role` | 是 | `string` | `assistant` | The role of the output message. Always assistant. |
| `status` | 是 | `string` | `in_progress`, `completed`, `incomplete` | The status of the message input. One of in_progress, completed, or incomplete. Populated when input items are returned via API. |
| `type` | 是 | `string` | `message` | The type of the output message. Always message. |

### `OutputMessageContent`

| 项 | 值 |
| --- | --- |
| 类型 | `OutputTextContent \| RefusalContent` |
| 说明 | - |
| 组合 | `oneOf` |

| 变体 | 类型 | 说明 |
| --- | --- | --- |
| 1 | `OutputTextContent` | - |
| 2 | `RefusalContent` | - |

### `OutputTextContent`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A text output from the model. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `annotations` | 是 | `array<Annotation>` | - | The annotations of the text output. |
| `logprobs` | 是 | `array<LogProb>` | - | - |
| `text` | 是 | `string` | - | The text output from the model. |
| `type` | 是 | `string` | `output_text` | The type of the output text. Always output_text. |

### `ParallelToolCalls`

| 项 | 值 |
| --- | --- |
| 类型 | `boolean` |
| 说明 | Whether to enable [parallel function calling](/docs/guides/function-calling#configuring-parallel-function-calling) during tool use. |

### `PartialImages`

| 项 | 值 |
| --- | --- |
| 类型 | `integer \| null` |
| 说明 | - |
| 组合 | `anyOf` |

| 变体 | 类型 | 说明 |
| --- | --- | --- |
| 1 | `integer` | The number of partial images to generate. This parameter is used for streaming responses that return partial images. Value must be between 0 and 3. When set to 0, the response wil… |
| 2 | `null` | - |

### `PredictionContent`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Static predicted output content, such as the content of a text file that is being regenerated. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `content` | 是 | `string \| array<ChatCompletionRequestMessageContentPartText>` | - | The content that should be matched when generating a model response. If generated tokens would match this content, the entire model response can be returned much more quickly. |
| `type` | 是 | `string` | `content` | The type of the predicted content you want to provide. This type is currently always content. |

### `Prompt`

| 项 | 值 |
| --- | --- |
| 类型 | `object \| null` |
| 说明 | - |
| 组合 | `anyOf` |

| 变体 | 类型 | 说明 |
| --- | --- | --- |
| 1 | `object` | Reference to a prompt template and its variables. [Learn more](/docs/guides/text?api-mode=responses#reusable-prompts). |
| 2 | `null` | - |

### `PromptCacheRetentionEnum`

| 项 | 值 |
| --- | --- |
| 类型 | `string` |
| 说明 | - |

### `RankerVersionType`

| 项 | 值 |
| --- | --- |
| 类型 | `string` |
| 说明 | - |

### `RankingOptions`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | - |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `hybrid_search` | 否 | `HybridSearchOptions` | - | Weights that control how reciprocal rank fusion balances semantic embedding matches versus sparse keyword matches when hybrid search is enabled. |
| `ranker` | 否 | `RankerVersionType` | - | The ranker to use for the file search. |
| `score_threshold` | 否 | `number` | - | The score threshold for the file search, a number between 0 and 1. Numbers closer to 1 will attempt to return only the most relevant results, but may return fewer results. |

### `Reasoning`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | **gpt-5 and o-series models only** Configuration options for [reasoning models](https://platform.openai.com/docs/guides/reasoning). |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `effort` | 否 | `ReasoningEffort` | - | - |
| `generate_summary` | 否 | `string \| null` | - | - |
| `summary` | 否 | `string \| null` | - | - |

### `ReasoningEffort`

| 项 | 值 |
| --- | --- |
| 类型 | `string \| null` |
| 说明 | - |
| 组合 | `anyOf` |

| 变体 | 类型 | 说明 |
| --- | --- | --- |
| 1 | `string` | Constrains effort on reasoning for [reasoning models](https://platform.openai.com/docs/guides/reasoning). Currently supported values are none, minimal, low, medium, high, and xhig… |
| 2 | `null` | - |

### `ReasoningItem`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A description of the chain of thought used by a reasoning model while generating a response. Be sure to include these items in your input to the Responses API for subsequent turns… |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `content` | 否 | `array<ReasoningTextContent>` | - | Reasoning text content. |
| `encrypted_content` | 否 | `string \| null` | - | - |
| `id` | 是 | `string` | - | The unique identifier of the reasoning content. |
| `status` | 否 | `string` | `in_progress`, `completed`, `incomplete` | The status of the item. One of in_progress, completed, or incomplete. Populated when items are returned via API. |
| `summary` | 是 | `array<SummaryTextContent>` | - | Reasoning summary content. |
| `type` | 是 | `string` | `reasoning` | The type of the object. Always reasoning. |

### `ReasoningTextContent`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Reasoning text from the model. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `text` | 是 | `string` | - | The reasoning text from the model. |
| `type` | 是 | `string` | `reasoning_text` | The type of the reasoning text. Always reasoning_text. |

### `RefusalContent`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A refusal from the model. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `refusal` | 是 | `string` | - | The refusal explanation from the model. |
| `type` | 是 | `string` | `refusal` | The type of the refusal. Always refusal. |

### `Response`

| 项 | 值 |
| --- | --- |
| 类型 | `ModelResponseProperties & ResponseProperties & object` |
| 说明 | - |
| 组合 | `allOf` |

| 变体 | 类型 | 说明 |
| --- | --- | --- |
| 1 | `ModelResponseProperties` | - |
| 2 | `ResponseProperties` | - |
| 3 | `object` | - |

#### allOf 展开字段

| 字段 | 必填 | 类型 | 枚举/常量 | 来源 | 说明 |
| --- | --- | --- | --- | --- | --- |
| `background` | 否 | `boolean \| null` | - | `ResponseProperties` | - |
| `completed_at` | 否 | `number(unixtime) \| null` | - | `Response.allOf[3]` | - |
| `conversation` | 否 | `Conversation-2 \| null` | - | `Response.allOf[3]` | - |
| `created_at` | 是 | `number(unixtime)` | - | `Response.allOf[3]` | Unix timestamp (in seconds) of when this Response was created. |
| `error` | 是 | `ResponseError` | - | `Response.allOf[3]` | - |
| `id` | 是 | `string` | - | `Response.allOf[3]` | Unique identifier for this Response. |
| `incomplete_details` | 是 | `object \| null` | - | `Response.allOf[3]` | - |
| `instructions` | 是 | `string \| array<InputItem> \| null` | - | `Response.allOf[3]` | - |
| `max_output_tokens` | 否 | `integer \| null` | - | `Response.allOf[3]` | - |
| `max_tool_calls` | 否 | `integer \| null` | - | `ResponseProperties` | - |
| `metadata` | 是 | `Metadata` | - | `ModelResponseProperties` | - |
| `model` | 是 | `ModelIdsResponses` | - | `ResponseProperties` | Model ID used to generate the response, like gpt-4o or o3. OpenAI offers a wide range of models with different capabilities, performance characteristics, and price points. Refer t… |
| `object` | 是 | `string` | `response` | `Response.allOf[3]` | The object type of this resource - always set to response. |
| `output` | 是 | `array<OutputItem>` | - | `Response.allOf[3]` | An array of content items generated by the model. - The length and order of items in the output array is dependent on the model's response. - Rather than accessing the first item … |
| `output_text` | 否 | `string \| null` | - | `Response.allOf[3]` | - |
| `parallel_tool_calls` | 是 | `boolean` | - | `Response.allOf[3]` | Whether to allow the model to run tool calls in parallel. |
| `previous_response_id` | 否 | `string \| null` | - | `ResponseProperties` | - |
| `prompt` | 否 | `Prompt` | - | `ResponseProperties` | - |
| `prompt_cache_key` | 否 | `string` | - | `ModelResponseProperties` | Used by OpenAI to cache responses for similar requests to optimize your cache hit rates. Replaces the user field. [Learn more](/docs/guides/prompt-caching). |
| `prompt_cache_retention` | 否 | `string \| null` | - | `ModelResponseProperties` | - |
| `reasoning` | 否 | `Reasoning \| null` | - | `ResponseProperties` | - |
| `safety_identifier` | 否 | `string` | - | `ModelResponseProperties` | A stable identifier used to help detect users of your application that may be violating OpenAI's usage policies. The IDs should be a string that uniquely identifies each user, wit… |
| `service_tier` | 否 | `ServiceTier` | - | `ModelResponseProperties` | - |
| `status` | 否 | `string` | `completed`, `failed`, `in_progress`, `cancelled`, `queued`, `incomplete` | `Response.allOf[3]` | The status of the response generation. One of completed, failed, in_progress, cancelled, queued, or incomplete. |
| `temperature` | 是 | `number \| null` | - | `ModelResponseProperties` | - |
| `text` | 否 | `ResponseTextParam` | - | `ResponseProperties` | - |
| `tool_choice` | 是 | `ToolChoiceParam` | - | `ResponseProperties` | - |
| `tools` | 是 | `ToolsArray` | - | `ResponseProperties` | - |
| `top_logprobs` | 否 | `integer \| null` | - | `ModelResponseProperties` | - |
| `top_p` | 是 | `number \| null` | - | `ModelResponseProperties` | - |
| `truncation` | 否 | `string \| null` | - | `ResponseProperties` | - |
| `usage` | 否 | `ResponseUsage` | - | `Response.allOf[3]` | - |
| `user` | 否 | `string` | - | `ModelResponseProperties` | This field is being replaced by safety_identifier and prompt_cache_key. Use prompt_cache_key instead to maintain caching optimizations. A stable identifier for your end-users. Use… |

### `ResponseAudioDeltaEvent`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Emitted when there is a partial audio response. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `delta` | 是 | `string` | - | A chunk of Base64 encoded response audio bytes. |
| `sequence_number` | 是 | `integer` | - | A sequence number for this chunk of the stream response. |
| `type` | 是 | `string` | `response.audio.delta` | The type of the event. Always response.audio.delta. |

### `ResponseAudioDoneEvent`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Emitted when the audio response is complete. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `sequence_number` | 是 | `integer` | - | The sequence number of the delta. |
| `type` | 是 | `string` | `response.audio.done` | The type of the event. Always response.audio.done. |

### `ResponseAudioTranscriptDeltaEvent`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Emitted when there is a partial transcript of audio. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `delta` | 是 | `string` | - | The partial transcript of the audio response. |
| `sequence_number` | 是 | `integer` | - | The sequence number of this event. |
| `type` | 是 | `string` | `response.audio.transcript.delta` | The type of the event. Always response.audio.transcript.delta. |

### `ResponseAudioTranscriptDoneEvent`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Emitted when the full audio transcript is completed. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `sequence_number` | 是 | `integer` | - | The sequence number of this event. |
| `type` | 是 | `string` | `response.audio.transcript.done` | The type of the event. Always response.audio.transcript.done. |

### `ResponseCodeInterpreterCallCodeDeltaEvent`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Emitted when a partial code snippet is streamed by the code interpreter. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `delta` | 是 | `string` | - | The partial code snippet being streamed by the code interpreter. |
| `item_id` | 是 | `string` | - | The unique identifier of the code interpreter tool call item. |
| `output_index` | 是 | `integer` | - | The index of the output item in the response for which the code is being streamed. |
| `sequence_number` | 是 | `integer` | - | The sequence number of this event, used to order streaming events. |
| `type` | 是 | `string` | `response.code_interpreter_call_code.delta` | The type of the event. Always response.code_interpreter_call_code.delta. |

### `ResponseCodeInterpreterCallCodeDoneEvent`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Emitted when the code snippet is finalized by the code interpreter. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `code` | 是 | `string` | - | The final code snippet output by the code interpreter. |
| `item_id` | 是 | `string` | - | The unique identifier of the code interpreter tool call item. |
| `output_index` | 是 | `integer` | - | The index of the output item in the response for which the code is finalized. |
| `sequence_number` | 是 | `integer` | - | The sequence number of this event, used to order streaming events. |
| `type` | 是 | `string` | `response.code_interpreter_call_code.done` | The type of the event. Always response.code_interpreter_call_code.done. |

### `ResponseCodeInterpreterCallCompletedEvent`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Emitted when the code interpreter call is completed. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `item_id` | 是 | `string` | - | The unique identifier of the code interpreter tool call item. |
| `output_index` | 是 | `integer` | - | The index of the output item in the response for which the code interpreter call is completed. |
| `sequence_number` | 是 | `integer` | - | The sequence number of this event, used to order streaming events. |
| `type` | 是 | `string` | `response.code_interpreter_call.completed` | The type of the event. Always response.code_interpreter_call.completed. |

### `ResponseCodeInterpreterCallInProgressEvent`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Emitted when a code interpreter call is in progress. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `item_id` | 是 | `string` | - | The unique identifier of the code interpreter tool call item. |
| `output_index` | 是 | `integer` | - | The index of the output item in the response for which the code interpreter call is in progress. |
| `sequence_number` | 是 | `integer` | - | The sequence number of this event, used to order streaming events. |
| `type` | 是 | `string` | `response.code_interpreter_call.in_progress` | The type of the event. Always response.code_interpreter_call.in_progress. |

### `ResponseCodeInterpreterCallInterpretingEvent`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Emitted when the code interpreter is actively interpreting the code snippet. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `item_id` | 是 | `string` | - | The unique identifier of the code interpreter tool call item. |
| `output_index` | 是 | `integer` | - | The index of the output item in the response for which the code interpreter is interpreting code. |
| `sequence_number` | 是 | `integer` | - | The sequence number of this event, used to order streaming events. |
| `type` | 是 | `string` | `response.code_interpreter_call.interpreting` | The type of the event. Always response.code_interpreter_call.interpreting. |

### `ResponseCompletedEvent`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Emitted when the model response is complete. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `response` | 是 | `Response` | - | Properties of the completed response. |
| `sequence_number` | 是 | `integer` | - | The sequence number for this event. |
| `type` | 是 | `string` | `response.completed` | The type of the event. Always response.completed. |

### `ResponseContentPartAddedEvent`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Emitted when a new content part is added. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `content_index` | 是 | `integer` | - | The index of the content part that was added. |
| `item_id` | 是 | `string` | - | The ID of the output item that the content part was added to. |
| `output_index` | 是 | `integer` | - | The index of the output item that the content part was added to. |
| `part` | 是 | `OutputContent` | - | The content part that was added. |
| `sequence_number` | 是 | `integer` | - | The sequence number of this event. |
| `type` | 是 | `string` | `response.content_part.added` | The type of the event. Always response.content_part.added. |

### `ResponseContentPartDoneEvent`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Emitted when a content part is done. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `content_index` | 是 | `integer` | - | The index of the content part that is done. |
| `item_id` | 是 | `string` | - | The ID of the output item that the content part was added to. |
| `output_index` | 是 | `integer` | - | The index of the output item that the content part was added to. |
| `part` | 是 | `OutputContent` | - | The content part that is done. |
| `sequence_number` | 是 | `integer` | - | The sequence number of this event. |
| `type` | 是 | `string` | `response.content_part.done` | The type of the event. Always response.content_part.done. |

### `ResponseCreatedEvent`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | An event that is emitted when a response is created. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `response` | 是 | `Response` | - | The response that was created. |
| `sequence_number` | 是 | `integer` | - | The sequence number for this event. |
| `type` | 是 | `string` | `response.created` | The type of the event. Always response.created. |

### `ResponseCustomToolCallInputDeltaEvent`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Event representing a delta (partial update) to the input of a custom tool call. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `delta` | 是 | `string` | - | The incremental input data (delta) for the custom tool call. |
| `item_id` | 是 | `string` | - | Unique identifier for the API item associated with this event. |
| `output_index` | 是 | `integer` | - | The index of the output this delta applies to. |
| `sequence_number` | 是 | `integer` | - | The sequence number of this event. |
| `type` | 是 | `string` | `response.custom_tool_call_input.delta` | The event type identifier. |

### `ResponseCustomToolCallInputDoneEvent`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Event indicating that input for a custom tool call is complete. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `input` | 是 | `string` | - | The complete input data for the custom tool call. |
| `item_id` | 是 | `string` | - | Unique identifier for the API item associated with this event. |
| `output_index` | 是 | `integer` | - | The index of the output this event applies to. |
| `sequence_number` | 是 | `integer` | - | The sequence number of this event. |
| `type` | 是 | `string` | `response.custom_tool_call_input.done` | The event type identifier. |

### `ResponseError`

| 项 | 值 |
| --- | --- |
| 类型 | `object \| null` |
| 说明 | - |
| 组合 | `anyOf` |

| 变体 | 类型 | 说明 |
| --- | --- | --- |
| 1 | `object` | An error object returned when the model fails to generate a Response. |
| 2 | `null` | - |

### `ResponseErrorCode`

| 项 | 值 |
| --- | --- |
| 类型 | `string` |
| 说明 | The error code for the response. |

### `ResponseErrorEvent`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Emitted when an error occurs. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `code` | 是 | `string \| null` | - | - |
| `message` | 是 | `string` | - | The error message. |
| `param` | 是 | `string \| null` | - | - |
| `sequence_number` | 是 | `integer` | - | The sequence number of this event. |
| `type` | 是 | `string` | `error` | The type of the event. Always error. |

### `ResponseFailedEvent`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | An event that is emitted when a response fails. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `response` | 是 | `Response` | - | The response that failed. |
| `sequence_number` | 是 | `integer` | - | The sequence number of this event. |
| `type` | 是 | `string` | `response.failed` | The type of the event. Always response.failed. |

### `ResponseFileSearchCallCompletedEvent`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Emitted when a file search call is completed (results found). |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `item_id` | 是 | `string` | - | The ID of the output item that the file search call is initiated. |
| `output_index` | 是 | `integer` | - | The index of the output item that the file search call is initiated. |
| `sequence_number` | 是 | `integer` | - | The sequence number of this event. |
| `type` | 是 | `string` | `response.file_search_call.completed` | The type of the event. Always response.file_search_call.completed. |

### `ResponseFileSearchCallInProgressEvent`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Emitted when a file search call is initiated. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `item_id` | 是 | `string` | - | The ID of the output item that the file search call is initiated. |
| `output_index` | 是 | `integer` | - | The index of the output item that the file search call is initiated. |
| `sequence_number` | 是 | `integer` | - | The sequence number of this event. |
| `type` | 是 | `string` | `response.file_search_call.in_progress` | The type of the event. Always response.file_search_call.in_progress. |

### `ResponseFileSearchCallSearchingEvent`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Emitted when a file search is currently searching. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `item_id` | 是 | `string` | - | The ID of the output item that the file search call is initiated. |
| `output_index` | 是 | `integer` | - | The index of the output item that the file search call is searching. |
| `sequence_number` | 是 | `integer` | - | The sequence number of this event. |
| `type` | 是 | `string` | `response.file_search_call.searching` | The type of the event. Always response.file_search_call.searching. |

### `ResponseFormatJsonObject`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | JSON object response format. An older method of generating JSON responses. Using json_schema is recommended for models that support it. Note that the model will not generate JSON … |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `type` | 是 | `string` | `json_object` | The type of response format being defined. Always json_object. |

### `ResponseFormatJsonSchema`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | JSON Schema response format. Used to generate structured JSON responses. Learn more about [Structured Outputs](/docs/guides/structured-outputs). |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `json_schema` | 是 | `object` | - | Structured Outputs configuration options, including a JSON Schema. |
| `type` | 是 | `string` | `json_schema` | The type of response format being defined. Always json_schema. |

### `ResponseFormatJsonSchemaSchema`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | The schema for the response format, described as a JSON Schema object. Learn how to build JSON schemas [here](https://json-schema.org/). |

Additional properties: `任意 JSON 值`

### `ResponseFormatText`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Default response format. Used to generate text responses. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `type` | 是 | `string` | `text` | The type of response format being defined. Always text. |

### `ResponseFunctionCallArgumentsDeltaEvent`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Emitted when there is a partial function-call arguments delta. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `delta` | 是 | `string` | - | The function-call arguments delta that is added. |
| `item_id` | 是 | `string` | - | The ID of the output item that the function-call arguments delta is added to. |
| `output_index` | 是 | `integer` | - | The index of the output item that the function-call arguments delta is added to. |
| `sequence_number` | 是 | `integer` | - | The sequence number of this event. |
| `type` | 是 | `string` | `response.function_call_arguments.delta` | The type of the event. Always response.function_call_arguments.delta. |

### `ResponseFunctionCallArgumentsDoneEvent`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Emitted when function-call arguments are finalized. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `arguments` | 是 | `string` | - | The function-call arguments. |
| `item_id` | 是 | `string` | - | The ID of the item. |
| `name` | 是 | `string` | - | The name of the function that was called. |
| `output_index` | 是 | `integer` | - | The index of the output item. |
| `sequence_number` | 是 | `integer` | - | The sequence number of this event. |
| `type` | 是 | `string` | `response.function_call_arguments.done` | - |

### `ResponseImageGenCallCompletedEvent`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Emitted when an image generation tool call has completed and the final image is available. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `item_id` | 是 | `string` | - | The unique identifier of the image generation item being processed. |
| `output_index` | 是 | `integer` | - | The index of the output item in the response's output array. |
| `sequence_number` | 是 | `integer` | - | The sequence number of this event. |
| `type` | 是 | `string` | `response.image_generation_call.completed` | The type of the event. Always 'response.image_generation_call.completed'. |

### `ResponseImageGenCallGeneratingEvent`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Emitted when an image generation tool call is actively generating an image (intermediate state). |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `item_id` | 是 | `string` | - | The unique identifier of the image generation item being processed. |
| `output_index` | 是 | `integer` | - | The index of the output item in the response's output array. |
| `sequence_number` | 是 | `integer` | - | The sequence number of the image generation item being processed. |
| `type` | 是 | `string` | `response.image_generation_call.generating` | The type of the event. Always 'response.image_generation_call.generating'. |

### `ResponseImageGenCallInProgressEvent`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Emitted when an image generation tool call is in progress. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `item_id` | 是 | `string` | - | The unique identifier of the image generation item being processed. |
| `output_index` | 是 | `integer` | - | The index of the output item in the response's output array. |
| `sequence_number` | 是 | `integer` | - | The sequence number of the image generation item being processed. |
| `type` | 是 | `string` | `response.image_generation_call.in_progress` | The type of the event. Always 'response.image_generation_call.in_progress'. |

### `ResponseImageGenCallPartialImageEvent`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Emitted when a partial image is available during image generation streaming. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `item_id` | 是 | `string` | - | The unique identifier of the image generation item being processed. |
| `output_index` | 是 | `integer` | - | The index of the output item in the response's output array. |
| `partial_image_b64` | 是 | `string` | - | Base64-encoded partial image data, suitable for rendering as an image. |
| `partial_image_index` | 是 | `integer` | - | 0-based index for the partial image (backend is 1-based, but this is 0-based for the user). |
| `sequence_number` | 是 | `integer` | - | The sequence number of the image generation item being processed. |
| `type` | 是 | `string` | `response.image_generation_call.partial_image` | The type of the event. Always 'response.image_generation_call.partial_image'. |

### `ResponseInProgressEvent`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Emitted when the response is in progress. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `response` | 是 | `Response` | - | The response that is in progress. |
| `sequence_number` | 是 | `integer` | - | The sequence number of this event. |
| `type` | 是 | `string` | `response.in_progress` | The type of the event. Always response.in_progress. |

### `ResponseIncompleteEvent`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | An event that is emitted when a response finishes as incomplete. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `response` | 是 | `Response` | - | The response that was incomplete. |
| `sequence_number` | 是 | `integer` | - | The sequence number of this event. |
| `type` | 是 | `string` | `response.incomplete` | The type of the event. Always response.incomplete. |

### `ResponseLogProb`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A logprob is the logarithmic probability that the model assigns to producing a particular token at a given position in the sequence. Less-negative (higher) logprob values indicate… |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `logprob` | 是 | `number` | - | The log probability of this token. |
| `token` | 是 | `string` | - | A possible text token. |
| `top_logprobs` | 否 | `array<object>` | - | The log probabilities of up to 20 of the most likely tokens. |

### `ResponseMCPCallArgumentsDeltaEvent`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Emitted when there is a delta (partial update) to the arguments of an MCP tool call. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `delta` | 是 | `string` | - | A JSON string containing the partial update to the arguments for the MCP tool call. |
| `item_id` | 是 | `string` | - | The unique identifier of the MCP tool call item being processed. |
| `output_index` | 是 | `integer` | - | The index of the output item in the response's output array. |
| `sequence_number` | 是 | `integer` | - | The sequence number of this event. |
| `type` | 是 | `string` | `response.mcp_call_arguments.delta` | The type of the event. Always 'response.mcp_call_arguments.delta'. |

### `ResponseMCPCallArgumentsDoneEvent`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Emitted when the arguments for an MCP tool call are finalized. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `arguments` | 是 | `string` | - | A JSON string containing the finalized arguments for the MCP tool call. |
| `item_id` | 是 | `string` | - | The unique identifier of the MCP tool call item being processed. |
| `output_index` | 是 | `integer` | - | The index of the output item in the response's output array. |
| `sequence_number` | 是 | `integer` | - | The sequence number of this event. |
| `type` | 是 | `string` | `response.mcp_call_arguments.done` | The type of the event. Always 'response.mcp_call_arguments.done'. |

### `ResponseMCPCallCompletedEvent`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Emitted when an MCP tool call has completed successfully. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `item_id` | 是 | `string` | - | The ID of the MCP tool call item that completed. |
| `output_index` | 是 | `integer` | - | The index of the output item that completed. |
| `sequence_number` | 是 | `integer` | - | The sequence number of this event. |
| `type` | 是 | `string` | `response.mcp_call.completed` | The type of the event. Always 'response.mcp_call.completed'. |

### `ResponseMCPCallFailedEvent`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Emitted when an MCP tool call has failed. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `item_id` | 是 | `string` | - | The ID of the MCP tool call item that failed. |
| `output_index` | 是 | `integer` | - | The index of the output item that failed. |
| `sequence_number` | 是 | `integer` | - | The sequence number of this event. |
| `type` | 是 | `string` | `response.mcp_call.failed` | The type of the event. Always 'response.mcp_call.failed'. |

### `ResponseMCPCallInProgressEvent`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Emitted when an MCP tool call is in progress. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `item_id` | 是 | `string` | - | The unique identifier of the MCP tool call item being processed. |
| `output_index` | 是 | `integer` | - | The index of the output item in the response's output array. |
| `sequence_number` | 是 | `integer` | - | The sequence number of this event. |
| `type` | 是 | `string` | `response.mcp_call.in_progress` | The type of the event. Always 'response.mcp_call.in_progress'. |

### `ResponseMCPListToolsCompletedEvent`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Emitted when the list of available MCP tools has been successfully retrieved. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `item_id` | 是 | `string` | - | The ID of the MCP tool call item that produced this output. |
| `output_index` | 是 | `integer` | - | The index of the output item that was processed. |
| `sequence_number` | 是 | `integer` | - | The sequence number of this event. |
| `type` | 是 | `string` | `response.mcp_list_tools.completed` | The type of the event. Always 'response.mcp_list_tools.completed'. |

### `ResponseMCPListToolsFailedEvent`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Emitted when the attempt to list available MCP tools has failed. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `item_id` | 是 | `string` | - | The ID of the MCP tool call item that failed. |
| `output_index` | 是 | `integer` | - | The index of the output item that failed. |
| `sequence_number` | 是 | `integer` | - | The sequence number of this event. |
| `type` | 是 | `string` | `response.mcp_list_tools.failed` | The type of the event. Always 'response.mcp_list_tools.failed'. |

### `ResponseMCPListToolsInProgressEvent`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Emitted when the system is in the process of retrieving the list of available MCP tools. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `item_id` | 是 | `string` | - | The ID of the MCP tool call item that is being processed. |
| `output_index` | 是 | `integer` | - | The index of the output item that is being processed. |
| `sequence_number` | 是 | `integer` | - | The sequence number of this event. |
| `type` | 是 | `string` | `response.mcp_list_tools.in_progress` | The type of the event. Always 'response.mcp_list_tools.in_progress'. |

### `ResponseModalities`

| 项 | 值 |
| --- | --- |
| 类型 | `array<string> \| null` |
| 说明 | - |
| 组合 | `anyOf` |

| 变体 | 类型 | 说明 |
| --- | --- | --- |
| 1 | `array<string>` | Output types that you would like the model to generate. Most models are capable of generating text, which is the default: ["text"] The gpt-4o-audio-preview model can also be used … |
| 2 | `null` | - |

### `ResponseOutputItemAddedEvent`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Emitted when a new output item is added. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `item` | 是 | `OutputItem` | - | The output item that was added. |
| `output_index` | 是 | `integer` | - | The index of the output item that was added. |
| `sequence_number` | 是 | `integer` | - | The sequence number of this event. |
| `type` | 是 | `string` | `response.output_item.added` | The type of the event. Always response.output_item.added. |

### `ResponseOutputItemDoneEvent`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Emitted when an output item is marked done. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `item` | 是 | `OutputItem` | - | The output item that was marked done. |
| `output_index` | 是 | `integer` | - | The index of the output item that was marked done. |
| `sequence_number` | 是 | `integer` | - | The sequence number of this event. |
| `type` | 是 | `string` | `response.output_item.done` | The type of the event. Always response.output_item.done. |

### `ResponseOutputTextAnnotationAddedEvent`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Emitted when an annotation is added to output text content. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `annotation` | 是 | `object` | - | The annotation object being added. (See annotation schema for details.) |
| `annotation_index` | 是 | `integer` | - | The index of the annotation within the content part. |
| `content_index` | 是 | `integer` | - | The index of the content part within the output item. |
| `item_id` | 是 | `string` | - | The unique identifier of the item to which the annotation is being added. |
| `output_index` | 是 | `integer` | - | The index of the output item in the response's output array. |
| `sequence_number` | 是 | `integer` | - | The sequence number of this event. |
| `type` | 是 | `string` | `response.output_text.annotation.added` | The type of the event. Always 'response.output_text.annotation.added'. |

### `ResponsePromptVariables`

| 项 | 值 |
| --- | --- |
| 类型 | `object/map<string, string \| InputTextContent \| InputImageContent \| InputFileContent> \| null` |
| 说明 | - |
| 组合 | `anyOf` |

| 变体 | 类型 | 说明 |
| --- | --- | --- |
| 1 | `object/map<string, string \| InputTextContent \| InputImageContent \| InputFileContent>` | Optional map of values to substitute in for variables in your prompt. The substitution values can either be strings, or other Response input types like images or files. |
| 2 | `null` | - |

### `ResponseProperties`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | - |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `background` | 否 | `boolean \| null` | - | - |
| `max_tool_calls` | 否 | `integer \| null` | - | - |
| `model` | 否 | `ModelIdsResponses` | - | Model ID used to generate the response, like gpt-4o or o3. OpenAI offers a wide range of models with different capabilities, performance characteristics, and price points. Refer t… |
| `previous_response_id` | 否 | `string \| null` | - | - |
| `prompt` | 否 | `Prompt` | - | - |
| `reasoning` | 否 | `Reasoning \| null` | - | - |
| `text` | 否 | `ResponseTextParam` | - | - |
| `tool_choice` | 否 | `ToolChoiceParam` | - | - |
| `tools` | 否 | `ToolsArray` | - | - |
| `truncation` | 否 | `string \| null` | - | - |

### `ResponseQueuedEvent`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Emitted when a response is queued and waiting to be processed. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `response` | 是 | `Response` | - | The full response object that is queued. |
| `sequence_number` | 是 | `integer` | - | The sequence number for this event. |
| `type` | 是 | `string` | `response.queued` | The type of the event. Always 'response.queued'. |

### `ResponseReasoningSummaryPartAddedEvent`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Emitted when a new reasoning summary part is added. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `item_id` | 是 | `string` | - | The ID of the item this summary part is associated with. |
| `output_index` | 是 | `integer` | - | The index of the output item this summary part is associated with. |
| `part` | 是 | `object` | - | The summary part that was added. |
| `sequence_number` | 是 | `integer` | - | The sequence number of this event. |
| `summary_index` | 是 | `integer` | - | The index of the summary part within the reasoning summary. |
| `type` | 是 | `string` | `response.reasoning_summary_part.added` | The type of the event. Always response.reasoning_summary_part.added. |

### `ResponseReasoningSummaryPartDoneEvent`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Emitted when a reasoning summary part is completed. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `item_id` | 是 | `string` | - | The ID of the item this summary part is associated with. |
| `output_index` | 是 | `integer` | - | The index of the output item this summary part is associated with. |
| `part` | 是 | `object` | - | The completed summary part. |
| `sequence_number` | 是 | `integer` | - | The sequence number of this event. |
| `summary_index` | 是 | `integer` | - | The index of the summary part within the reasoning summary. |
| `type` | 是 | `string` | `response.reasoning_summary_part.done` | The type of the event. Always response.reasoning_summary_part.done. |

### `ResponseReasoningSummaryTextDeltaEvent`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Emitted when a delta is added to a reasoning summary text. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `delta` | 是 | `string` | - | The text delta that was added to the summary. |
| `item_id` | 是 | `string` | - | The ID of the item this summary text delta is associated with. |
| `output_index` | 是 | `integer` | - | The index of the output item this summary text delta is associated with. |
| `sequence_number` | 是 | `integer` | - | The sequence number of this event. |
| `summary_index` | 是 | `integer` | - | The index of the summary part within the reasoning summary. |
| `type` | 是 | `string` | `response.reasoning_summary_text.delta` | The type of the event. Always response.reasoning_summary_text.delta. |

### `ResponseReasoningSummaryTextDoneEvent`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Emitted when a reasoning summary text is completed. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `item_id` | 是 | `string` | - | The ID of the item this summary text is associated with. |
| `output_index` | 是 | `integer` | - | The index of the output item this summary text is associated with. |
| `sequence_number` | 是 | `integer` | - | The sequence number of this event. |
| `summary_index` | 是 | `integer` | - | The index of the summary part within the reasoning summary. |
| `text` | 是 | `string` | - | The full text of the completed reasoning summary. |
| `type` | 是 | `string` | `response.reasoning_summary_text.done` | The type of the event. Always response.reasoning_summary_text.done. |

### `ResponseReasoningTextDeltaEvent`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Emitted when a delta is added to a reasoning text. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `content_index` | 是 | `integer` | - | The index of the reasoning content part this delta is associated with. |
| `delta` | 是 | `string` | - | The text delta that was added to the reasoning content. |
| `item_id` | 是 | `string` | - | The ID of the item this reasoning text delta is associated with. |
| `output_index` | 是 | `integer` | - | The index of the output item this reasoning text delta is associated with. |
| `sequence_number` | 是 | `integer` | - | The sequence number of this event. |
| `type` | 是 | `string` | `response.reasoning_text.delta` | The type of the event. Always response.reasoning_text.delta. |

### `ResponseReasoningTextDoneEvent`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Emitted when a reasoning text is completed. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `content_index` | 是 | `integer` | - | The index of the reasoning content part. |
| `item_id` | 是 | `string` | - | The ID of the item this reasoning text is associated with. |
| `output_index` | 是 | `integer` | - | The index of the output item this reasoning text is associated with. |
| `sequence_number` | 是 | `integer` | - | The sequence number of this event. |
| `text` | 是 | `string` | - | The full text of the completed reasoning content. |
| `type` | 是 | `string` | `response.reasoning_text.done` | The type of the event. Always response.reasoning_text.done. |

### `ResponseRefusalDeltaEvent`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Emitted when there is a partial refusal text. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `content_index` | 是 | `integer` | - | The index of the content part that the refusal text is added to. |
| `delta` | 是 | `string` | - | The refusal text that is added. |
| `item_id` | 是 | `string` | - | The ID of the output item that the refusal text is added to. |
| `output_index` | 是 | `integer` | - | The index of the output item that the refusal text is added to. |
| `sequence_number` | 是 | `integer` | - | The sequence number of this event. |
| `type` | 是 | `string` | `response.refusal.delta` | The type of the event. Always response.refusal.delta. |

### `ResponseRefusalDoneEvent`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Emitted when refusal text is finalized. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `content_index` | 是 | `integer` | - | The index of the content part that the refusal text is finalized. |
| `item_id` | 是 | `string` | - | The ID of the output item that the refusal text is finalized. |
| `output_index` | 是 | `integer` | - | The index of the output item that the refusal text is finalized. |
| `refusal` | 是 | `string` | - | The refusal text that is finalized. |
| `sequence_number` | 是 | `integer` | - | The sequence number of this event. |
| `type` | 是 | `string` | `response.refusal.done` | The type of the event. Always response.refusal.done. |

### `ResponseStreamEvent`

| 项 | 值 |
| --- | --- |
| 类型 | `ResponseAudioDeltaEvent \| ResponseAudioDoneEvent \| ResponseAudioTranscriptDeltaEvent \| ResponseAudioTranscriptDoneEvent \| ResponseCodeInterpreterCallCodeDeltaEvent \| ResponseCodeInterpreterCallCodeDoneEvent \| ResponseCodeInterpreterCallCompletedEvent \| ResponseCodeInterpreterCallInProgressEvent … (+45)` |
| 说明 | - |
| 组合 | `anyOf` |

| 变体 | 类型 | 说明 |
| --- | --- | --- |
| 1 | `ResponseAudioDeltaEvent` | - |
| 2 | `ResponseAudioDoneEvent` | - |
| 3 | `ResponseAudioTranscriptDeltaEvent` | - |
| 4 | `ResponseAudioTranscriptDoneEvent` | - |
| 5 | `ResponseCodeInterpreterCallCodeDeltaEvent` | - |
| 6 | `ResponseCodeInterpreterCallCodeDoneEvent` | - |
| 7 | `ResponseCodeInterpreterCallCompletedEvent` | - |
| 8 | `ResponseCodeInterpreterCallInProgressEvent` | - |
| 9 | `ResponseCodeInterpreterCallInterpretingEvent` | - |
| 10 | `ResponseCompletedEvent` | - |
| 11 | `ResponseContentPartAddedEvent` | - |
| 12 | `ResponseContentPartDoneEvent` | - |
| 13 | `ResponseCreatedEvent` | - |
| 14 | `ResponseErrorEvent` | - |
| 15 | `ResponseFileSearchCallCompletedEvent` | - |
| 16 | `ResponseFileSearchCallInProgressEvent` | - |
| 17 | `ResponseFileSearchCallSearchingEvent` | - |
| 18 | `ResponseFunctionCallArgumentsDeltaEvent` | - |
| 19 | `ResponseFunctionCallArgumentsDoneEvent` | - |
| 20 | `ResponseInProgressEvent` | - |
| 21 | `ResponseFailedEvent` | - |
| 22 | `ResponseIncompleteEvent` | - |
| 23 | `ResponseOutputItemAddedEvent` | - |
| 24 | `ResponseOutputItemDoneEvent` | - |
| 25 | `ResponseReasoningSummaryPartAddedEvent` | - |
| 26 | `ResponseReasoningSummaryPartDoneEvent` | - |
| 27 | `ResponseReasoningSummaryTextDeltaEvent` | - |
| 28 | `ResponseReasoningSummaryTextDoneEvent` | - |
| 29 | `ResponseReasoningTextDeltaEvent` | - |
| 30 | `ResponseReasoningTextDoneEvent` | - |
| 31 | `ResponseRefusalDeltaEvent` | - |
| 32 | `ResponseRefusalDoneEvent` | - |
| 33 | `ResponseTextDeltaEvent` | - |
| 34 | `ResponseTextDoneEvent` | - |
| 35 | `ResponseWebSearchCallCompletedEvent` | - |
| 36 | `ResponseWebSearchCallInProgressEvent` | - |
| 37 | `ResponseWebSearchCallSearchingEvent` | - |
| 38 | `ResponseImageGenCallCompletedEvent` | - |
| 39 | `ResponseImageGenCallGeneratingEvent` | - |
| 40 | `ResponseImageGenCallInProgressEvent` | - |
| 41 | `ResponseImageGenCallPartialImageEvent` | - |
| 42 | `ResponseMCPCallArgumentsDeltaEvent` | - |
| 43 | `ResponseMCPCallArgumentsDoneEvent` | - |
| 44 | `ResponseMCPCallCompletedEvent` | - |
| 45 | `ResponseMCPCallFailedEvent` | - |
| 46 | `ResponseMCPCallInProgressEvent` | - |
| 47 | `ResponseMCPListToolsCompletedEvent` | - |
| 48 | `ResponseMCPListToolsFailedEvent` | - |
| 49 | `ResponseMCPListToolsInProgressEvent` | - |
| 50 | `ResponseOutputTextAnnotationAddedEvent` | - |
| 51 | `ResponseQueuedEvent` | - |
| 52 | `ResponseCustomToolCallInputDeltaEvent` | - |
| 53 | `ResponseCustomToolCallInputDoneEvent` | - |

### `ResponseStreamOptions`

| 项 | 值 |
| --- | --- |
| 类型 | `object \| null` |
| 说明 | - |
| 组合 | `anyOf` |

| 变体 | 类型 | 说明 |
| --- | --- | --- |
| 1 | `object` | Options for streaming responses. Only set this when you set stream: true. |
| 2 | `null` | - |

### `ResponseTextDeltaEvent`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Emitted when there is an additional text delta. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `content_index` | 是 | `integer` | - | The index of the content part that the text delta was added to. |
| `delta` | 是 | `string` | - | The text delta that was added. |
| `item_id` | 是 | `string` | - | The ID of the output item that the text delta was added to. |
| `logprobs` | 是 | `array<ResponseLogProb>` | - | The log probabilities of the tokens in the delta. |
| `output_index` | 是 | `integer` | - | The index of the output item that the text delta was added to. |
| `sequence_number` | 是 | `integer` | - | The sequence number for this event. |
| `type` | 是 | `string` | `response.output_text.delta` | The type of the event. Always response.output_text.delta. |

### `ResponseTextDoneEvent`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Emitted when text content is finalized. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `content_index` | 是 | `integer` | - | The index of the content part that the text content is finalized. |
| `item_id` | 是 | `string` | - | The ID of the output item that the text content is finalized. |
| `logprobs` | 是 | `array<ResponseLogProb>` | - | The log probabilities of the tokens in the delta. |
| `output_index` | 是 | `integer` | - | The index of the output item that the text content is finalized. |
| `sequence_number` | 是 | `integer` | - | The sequence number for this event. |
| `text` | 是 | `string` | - | The text content that is finalized. |
| `type` | 是 | `string` | `response.output_text.done` | The type of the event. Always response.output_text.done. |

### `ResponseTextParam`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Configuration options for a text response from the model. Can be plain text or structured JSON data. Learn more: - [Text inputs and outputs](/docs/guides/text) - [Structured Outpu… |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `format` | 否 | `TextResponseFormatConfiguration` | - | - |
| `verbosity` | 否 | `Verbosity` | - | - |

### `ResponseUsage`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Represents token usage details including input tokens, output tokens, a breakdown of output tokens, and the total tokens used. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `input_tokens` | 是 | `integer` | - | The number of input tokens. |
| `input_tokens_details` | 是 | `object` | - | A detailed breakdown of the input tokens. |
| `output_tokens` | 是 | `integer` | - | The number of output tokens. |
| `output_tokens_details` | 是 | `object` | - | A detailed breakdown of the output tokens. |
| `total_tokens` | 是 | `integer` | - | The total number of tokens used. |

### `ResponseWebSearchCallCompletedEvent`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Emitted when a web search call is completed. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `item_id` | 是 | `string` | - | Unique ID for the output item associated with the web search call. |
| `output_index` | 是 | `integer` | - | The index of the output item that the web search call is associated with. |
| `sequence_number` | 是 | `integer` | - | The sequence number of the web search call being processed. |
| `type` | 是 | `string` | `response.web_search_call.completed` | The type of the event. Always response.web_search_call.completed. |

### `ResponseWebSearchCallInProgressEvent`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Emitted when a web search call is initiated. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `item_id` | 是 | `string` | - | Unique ID for the output item associated with the web search call. |
| `output_index` | 是 | `integer` | - | The index of the output item that the web search call is associated with. |
| `sequence_number` | 是 | `integer` | - | The sequence number of the web search call being processed. |
| `type` | 是 | `string` | `response.web_search_call.in_progress` | The type of the event. Always response.web_search_call.in_progress. |

### `ResponseWebSearchCallSearchingEvent`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Emitted when a web search call is executing. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `item_id` | 是 | `string` | - | Unique ID for the output item associated with the web search call. |
| `output_index` | 是 | `integer` | - | The index of the output item that the web search call is associated with. |
| `sequence_number` | 是 | `integer` | - | The sequence number of the web search call being processed. |
| `type` | 是 | `string` | `response.web_search_call.searching` | The type of the event. Always response.web_search_call.searching. |

### `ScreenshotParam`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A screenshot action. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `type` | 是 | `string` | `screenshot` | Specifies the event type. For a screenshot action, this property is always set to screenshot. |

### `ScrollParam`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A scroll action. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `keys` | 否 | `array<string> \| null` | - | - |
| `scroll_x` | 是 | `integer` | - | The horizontal scroll distance. |
| `scroll_y` | 是 | `integer` | - | The vertical scroll distance. |
| `type` | 是 | `string` | `scroll` | Specifies the event type. For a scroll action, this property is always set to scroll. |
| `x` | 是 | `integer` | - | The x-coordinate where the scroll occurred. |
| `y` | 是 | `integer` | - | The y-coordinate where the scroll occurred. |

### `SearchContentType`

| 项 | 值 |
| --- | --- |
| 类型 | `string` |
| 说明 | - |

### `SearchContextSize`

| 项 | 值 |
| --- | --- |
| 类型 | `string` |
| 说明 | - |

### `ServiceTier`

| 项 | 值 |
| --- | --- |
| 类型 | `string \| null` |
| 说明 | - |
| 组合 | `anyOf` |

| 变体 | 类型 | 说明 |
| --- | --- | --- |
| 1 | `string` | Specifies the processing type used for serving the request. - If set to 'auto', then the request will be processed with the service tier configured in the Project settings. Unless… |
| 2 | `null` | - |

### `ServiceTierEnum`

| 项 | 值 |
| --- | --- |
| 类型 | `string` |
| 说明 | - |

### `SkillReferenceParam`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | - |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `skill_id` | 是 | `string` | - | The ID of the referenced skill. |
| `type` | 是 | `string` | `skill_reference` | References a skill created with the /v1/skills endpoint. |
| `version` | 否 | `string` | - | Optional skill version. Use a positive integer or 'latest'. Omit for default. |

### `SpecificApplyPatchParam`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Forces the model to call the apply_patch tool when executing a tool call. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `type` | 是 | `string` | `apply_patch` | The tool to call. Always apply_patch. |

### `SpecificFunctionShellParam`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Forces the model to call the shell tool when a tool call is required. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `type` | 是 | `string` | `shell` | The tool to call. Always shell. |

### `StopConfiguration`

| 项 | 值 |
| --- | --- |
| 类型 | `string \| array<string>` |
| 说明 | Not supported with latest reasoning models o3 and o4-mini. Up to 4 sequences where the API will stop generating further tokens. The returned text will not contain the stop sequenc… |
| 组合 | `oneOf` |

| 变体 | 类型 | 说明 |
| --- | --- | --- |
| 1 | `string` | - |
| 2 | `array<string>` | - |

### `SummaryTextContent`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A summary text from the model. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `text` | 是 | `string` | - | A summary of the reasoning output from the model so far. |
| `type` | 是 | `string` | `summary_text` | The type of the object. Always summary_text. |

### `TextContent`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A text content. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `text` | 是 | `string` | - | - |
| `type` | 是 | `string` | `text` | - |

### `TextResponseFormatConfiguration`

| 项 | 值 |
| --- | --- |
| 类型 | `ResponseFormatText \| TextResponseFormatJsonSchema \| ResponseFormatJsonObject` |
| 说明 | An object specifying the format that the model must output. Configuring { "type": "json_schema" } enables Structured Outputs, which ensures the model will match your supplied JSON… |
| 组合 | `oneOf` |

| 变体 | 类型 | 说明 |
| --- | --- | --- |
| 1 | `ResponseFormatText` | - |
| 2 | `TextResponseFormatJsonSchema` | - |
| 3 | `ResponseFormatJsonObject` | - |

### `TextResponseFormatJsonSchema`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | JSON Schema response format. Used to generate structured JSON responses. Learn more about [Structured Outputs](/docs/guides/structured-outputs). |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `description` | 否 | `string` | - | A description of what the response format is for, used by the model to determine how to respond in the format. |
| `name` | 是 | `string` | - | The name of the response format. Must be a-z, A-Z, 0-9, or contain underscores and dashes, with a maximum length of 64. |
| `schema` | 是 | `ResponseFormatJsonSchemaSchema` | - | - |
| `strict` | 否 | `boolean \| null` | - | - |
| `type` | 是 | `string` | `json_schema` | The type of response format being defined. Always json_schema. |

### `Tool`

| 项 | 值 |
| --- | --- |
| 类型 | `FunctionTool \| FileSearchTool \| ComputerTool \| ComputerUsePreviewTool \| WebSearchTool \| MCPTool \| CodeInterpreterTool \| ImageGenTool … (+7)` |
| 说明 | A tool that can be used to generate a response. |
| 组合 | `oneOf` |

| 变体 | 类型 | 说明 |
| --- | --- | --- |
| 1 | `FunctionTool` | - |
| 2 | `FileSearchTool` | - |
| 3 | `ComputerTool` | - |
| 4 | `ComputerUsePreviewTool` | - |
| 5 | `WebSearchTool` | - |
| 6 | `MCPTool` | - |
| 7 | `CodeInterpreterTool` | - |
| 8 | `ImageGenTool` | - |
| 9 | `LocalShellToolParam` | - |
| 10 | `FunctionShellToolParam` | - |
| 11 | `CustomToolParam` | - |
| 12 | `NamespaceToolParam` | - |
| 13 | `ToolSearchToolParam` | - |
| 14 | `WebSearchPreviewTool` | - |
| 15 | `ApplyPatchToolParam` | - |

### `ToolChoiceAllowed`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Constrains the tools available to the model to a pre-defined set. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `mode` | 是 | `string` | `auto`, `required` | Constrains the tools available to the model to a pre-defined set. auto allows the model to pick from among the allowed tools and generate a message. required requires the model to… |
| `tools` | 是 | `array<object>` | - | A list of tool definitions that the model should be allowed to call. For the Responses API, the list of tool definitions might look like: |
| `type` | 是 | `string` | `allowed_tools` | Allowed tool configuration type. Always allowed_tools. |

### `ToolChoiceCustom`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Use this option to force the model to call a specific custom tool. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `name` | 是 | `string` | - | The name of the custom tool to call. |
| `type` | 是 | `string` | `custom` | For custom tool calling, the type is always custom. |

### `ToolChoiceFunction`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Use this option to force the model to call a specific function. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `name` | 是 | `string` | - | The name of the function to call. |
| `type` | 是 | `string` | `function` | For function calling, the type is always function. |

### `ToolChoiceMCP`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Use this option to force the model to call a specific tool on a remote MCP server. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `name` | 否 | `string \| null` | - | - |
| `server_label` | 是 | `string` | - | The label of the MCP server to use. |
| `type` | 是 | `string` | `mcp` | For MCP tools, the type is always mcp. |

### `ToolChoiceOptions`

| 项 | 值 |
| --- | --- |
| 类型 | `string` |
| 说明 | Controls which (if any) tool is called by the model. none means the model will not call any tool and instead generates a message. auto means the model can pick between generating … |

### `ToolChoiceParam`

| 项 | 值 |
| --- | --- |
| 类型 | `ToolChoiceOptions \| ToolChoiceAllowed \| ToolChoiceTypes \| ToolChoiceFunction \| ToolChoiceMCP \| ToolChoiceCustom \| SpecificApplyPatchParam \| SpecificFunctionShellParam` |
| 说明 | How the model should select which tool (or tools) to use when generating a response. See the tools parameter to see how to specify which tools the model can call. |
| 组合 | `oneOf` |

| 变体 | 类型 | 说明 |
| --- | --- | --- |
| 1 | `ToolChoiceOptions` | - |
| 2 | `ToolChoiceAllowed` | - |
| 3 | `ToolChoiceTypes` | - |
| 4 | `ToolChoiceFunction` | - |
| 5 | `ToolChoiceMCP` | - |
| 6 | `ToolChoiceCustom` | - |
| 7 | `SpecificApplyPatchParam` | - |
| 8 | `SpecificFunctionShellParam` | - |

### `ToolChoiceTypes`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Indicates that the model should use a built-in tool to generate a response. [Learn more about built-in tools](/docs/guides/tools). |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `type` | 是 | `string` | `file_search`, `web_search_preview`, `computer`, `computer_use_preview`, `computer_use`, `web_search_preview_2025_03_11`, `image_generation`, `code_interpreter` | The type of hosted tool the model should to use. Learn more about [built-in tools](/docs/guides/tools). Allowed values are: - file_search - web_search_preview - computer - compute… |

### `ToolSearchCall`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | - |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `arguments` | 是 | `object/value` | - | Arguments used for the tool search call. |
| `call_id` | 是 | `string \| null` | - | - |
| `created_by` | 否 | `string` | - | The identifier of the actor that created the item. |
| `execution` | 是 | `ToolSearchExecutionType` | - | Whether tool search was executed by the server or by the client. |
| `id` | 是 | `string` | - | The unique ID of the tool search call item. |
| `status` | 是 | `FunctionCallStatus` | - | The status of the tool search call item that was recorded. |
| `type` | 是 | `string` | `tool_search_call` | The type of the item. Always tool_search_call. |

### `ToolSearchCallItemParam`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | - |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `arguments` | 是 | `EmptyModelParam` | - | The arguments supplied to the tool search call. |
| `call_id` | 否 | `string \| null` | - | - |
| `execution` | 否 | `ToolSearchExecutionType` | - | Whether tool search was executed by the server or by the client. |
| `id` | 否 | `string \| null` | - | - |
| `status` | 否 | `FunctionCallItemStatus \| null` | - | - |
| `type` | 是 | `string` | `tool_search_call` | The item type. Always tool_search_call. |

### `ToolSearchExecutionType`

| 项 | 值 |
| --- | --- |
| 类型 | `string` |
| 说明 | - |

### `ToolSearchOutput`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | - |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `call_id` | 是 | `string \| null` | - | - |
| `created_by` | 否 | `string` | - | The identifier of the actor that created the item. |
| `execution` | 是 | `ToolSearchExecutionType` | - | Whether tool search was executed by the server or by the client. |
| `id` | 是 | `string` | - | The unique ID of the tool search output item. |
| `status` | 是 | `FunctionCallOutputStatusEnum` | - | The status of the tool search output item that was recorded. |
| `tools` | 是 | `array<Tool>` | - | The loaded tool definitions returned by tool search. |
| `type` | 是 | `string` | `tool_search_output` | The type of the item. Always tool_search_output. |

### `ToolSearchOutputItemParam`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | - |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `call_id` | 否 | `string \| null` | - | - |
| `execution` | 否 | `ToolSearchExecutionType` | - | Whether tool search was executed by the server or by the client. |
| `id` | 否 | `string \| null` | - | - |
| `status` | 否 | `FunctionCallItemStatus \| null` | - | - |
| `tools` | 是 | `array<Tool>` | - | The loaded tool definitions returned by the tool search output. |
| `type` | 是 | `string` | `tool_search_output` | The item type. Always tool_search_output. |

### `ToolSearchToolParam`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Hosted or BYOT tool search configuration for deferred tools. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `description` | 否 | `string \| null` | - | - |
| `execution` | 否 | `ToolSearchExecutionType` | - | Whether tool search is executed by the server or by the client. |
| `parameters` | 否 | `EmptyModelParam \| null` | - | - |
| `type` | 是 | `string` | `tool_search` | The type of the tool. Always tool_search. |

### `ToolsArray`

| 项 | 值 |
| --- | --- |
| 类型 | `array<Tool>` |
| 说明 | An array of tools the model may call while generating a response. You can specify which tool to use by setting the tool_choice parameter. We support the following categories of to… |

### `TopLogProb`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | The top log probability of a token. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `bytes` | 是 | `array<integer>` | - | - |
| `logprob` | 是 | `number` | - | - |
| `token` | 是 | `string` | - | - |

### `TypeParam`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | An action to type in text. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `text` | 是 | `string` | - | The text to type. |
| `type` | 是 | `string` | `type` | Specifies the event type. For a type action, this property is always set to type. |

### `UrlCitationBody`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A citation for a web resource used to generate a model response. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `end_index` | 是 | `integer` | - | The index of the last character of the URL citation in the message. |
| `start_index` | 是 | `integer` | - | The index of the first character of the URL citation in the message. |
| `title` | 是 | `string` | - | The title of the web resource. |
| `type` | 是 | `string` | `url_citation` | The type of the URL citation. Always url_citation. |
| `url` | 是 | `string(uri)` | - | The URL of the web resource. |

### `VectorStoreFileAttributes`

| 项 | 值 |
| --- | --- |
| 类型 | `object/map<string, string \| number \| boolean> \| null` |
| 说明 | - |
| 组合 | `anyOf` |

| 变体 | 类型 | 说明 |
| --- | --- | --- |
| 1 | `object/map<string, string \| number \| boolean>` | Set of 16 key-value pairs that can be attached to an object. This can be useful for storing additional information about the object in a structured format, and querying for object… |
| 2 | `null` | - |

### `Verbosity`

| 项 | 值 |
| --- | --- |
| 类型 | `string \| null` |
| 说明 | - |
| 组合 | `anyOf` |

| 变体 | 类型 | 说明 |
| --- | --- | --- |
| 1 | `string` | Constrains the verbosity of the model's response. Lower values will result in more concise responses, while higher values will result in more verbose responses. Currently supporte… |
| 2 | `null` | - |

### `VoiceIdsOrCustomVoice`

| 项 | 值 |
| --- | --- |
| 类型 | `VoiceIdsShared \| object` |
| 说明 | A built-in voice name or a custom voice reference. |
| 组合 | `anyOf` |

| 变体 | 类型 | 说明 |
| --- | --- | --- |
| 1 | `VoiceIdsShared` | - |
| 2 | `object` | Custom voice reference. |

### `VoiceIdsShared`

| 项 | 值 |
| --- | --- |
| 类型 | `string \| string` |
| 说明 | - |
| 组合 | `anyOf` |

| 变体 | 类型 | 说明 |
| --- | --- | --- |
| 1 | `string` | - |
| 2 | `string` | - |

### `WaitParam`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A wait action. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `type` | 是 | `string` | `wait` | Specifies the event type. For a wait action, this property is always set to wait. |

### `WebSearchActionFind`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Action type "find_in_page": Searches for a pattern within a loaded page. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `pattern` | 是 | `string` | - | The pattern or text to search for within the page. |
| `type` | 是 | `string` | `find_in_page` | The action type. |
| `url` | 是 | `string(uri)` | - | The URL of the page searched for the pattern. |

### `WebSearchActionOpenPage`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Action type "open_page" - Opens a specific URL from search results. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `type` | 是 | `string` | `open_page` | The action type. |
| `url` | 否 | `string(uri) \| null` | - | The URL opened by the model. |

### `WebSearchActionSearch`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Action type "search" - Performs a web search query. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `queries` | 否 | `array<string>` | - | The search queries. |
| `query` | 否 | `string` | - | The search query. |
| `sources` | 否 | `array<object>` | - | The sources used in the search. |
| `type` | 是 | `string` | `search` | The action type. |

### `WebSearchApproximateLocation`

| 项 | 值 |
| --- | --- |
| 类型 | `object \| null` |
| 说明 | - |
| 组合 | `anyOf` |

| 变体 | 类型 | 说明 |
| --- | --- | --- |
| 1 | `object` | The approximate location of the user. |
| 2 | `null` | - |

### `WebSearchContextSize`

| 项 | 值 |
| --- | --- |
| 类型 | `string` |
| 说明 | High level guidance for the amount of context window space to use for the search. One of low, medium, or high. medium is the default. |

### `WebSearchLocation`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Approximate location parameters for the search. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `city` | 否 | `string` | - | Free text input for the city of the user, e.g. San Francisco. |
| `country` | 否 | `string` | - | The two-letter [ISO country code](https://en.wikipedia.org/wiki/ISO_3166-1) of the user, e.g. US. |
| `region` | 否 | `string` | - | Free text input for the region of the user, e.g. California. |
| `timezone` | 否 | `string` | - | The [IANA timezone](https://timeapi.io/documentation/iana-timezones) of the user, e.g. America/Los_Angeles. |

### `WebSearchPreviewTool`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | This tool searches the web for relevant results to use in a response. Learn more about the [web search tool](https://platform.openai.com/docs/guides/tools-web-search). |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `search_content_types` | 否 | `array<SearchContentType>` | - | - |
| `search_context_size` | 否 | `SearchContextSize` | - | High level guidance for the amount of context window space to use for the search. One of low, medium, or high. medium is the default. |
| `type` | 是 | `string` | `web_search_preview`, `web_search_preview_2025_03_11` | The type of the web search tool. One of web_search_preview or web_search_preview_2025_03_11. |
| `user_location` | 否 | `ApproximateLocation \| null` | - | - |

### `WebSearchTool`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Search the Internet for sources related to the prompt. Learn more about the [web search tool](/docs/guides/tools-web-search). |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `filters` | 否 | `object \| null` | - | - |
| `search_context_size` | 否 | `string` | `low`, `medium`, `high` | High level guidance for the amount of context window space to use for the search. One of low, medium, or high. medium is the default. |
| `type` | 是 | `string` | `web_search`, `web_search_2025_08_26` | The type of the web search tool. One of web_search or web_search_2025_08_26. |
| `user_location` | 否 | `WebSearchApproximateLocation` | - | - |

### `WebSearchToolCall`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | The results of a web search tool call. See the [web search guide](/docs/guides/tools-web-search) for more information. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `action` | 是 | `WebSearchActionSearch \| WebSearchActionOpenPage \| WebSearchActionFind` | - | An object describing the specific action taken in this web search call. Includes details on how the model used the web (search, open_page, find_in_page). |
| `id` | 是 | `string` | - | The unique ID of the web search tool call. |
| `status` | 是 | `string` | `in_progress`, `searching`, `completed`, `failed` | The status of the web search tool call. |
| `type` | 是 | `string` | `web_search_call` | The type of the web search tool call. Always web_search_call. |

## Claude / Anthropic Endpoints

| Method | Path | Request schema | Response schema | 说明 |
| --- | --- | --- | --- | --- |
| POST | `/v1/messages` | `MessageCreateParams` | `Message` 或 `RawMessageStreamEvent` | 创建非流式或 SSE 流式消息 |
| POST | `/v1/messages/count_tokens` | `MessageCountTokensParams` | `MessageTokensCount` | 仅计数，不生成消息 |

## Claude / Anthropic TypeScript 字段表

以下类型从 Anthropic 官方 TypeScript SDK 的 Messages 资源类型递归引用得到，共 164 个。TypeScript union 中的 server tool 版本号是官方 SDK 暴露的字面量类型，实际可用性仍取决于 Anthropic 账号、模型和 beta 配置。

### `Base64ImageSource`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `data` | 是 | `string` |
| `media_type` | 是 | `'image/jpeg' \| 'image/png' \| 'image/gif' \| 'image/webp'` |
| `type` | 是 | `'base64'` |

### `Base64PDFSource`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `data` | 是 | `string` |
| `media_type` | 是 | `'application/pdf'` |
| `type` | 是 | `'base64'` |

### `BashCodeExecutionOutputBlock`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `file_id` | 是 | `string` |
| `type` | 是 | `'bash_code_execution_output'` |

### `BashCodeExecutionOutputBlockParam`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `file_id` | 是 | `string` |
| `type` | 是 | `'bash_code_execution_output'` |

### `BashCodeExecutionResultBlock`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `content` | 是 | `Array<BashCodeExecutionOutputBlock>` |
| `return_code` | 是 | `number` |
| `stderr` | 是 | `string` |
| `stdout` | 是 | `string` |
| `type` | 是 | `'bash_code_execution_result'` |

### `BashCodeExecutionResultBlockParam`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `content` | 是 | `Array<BashCodeExecutionOutputBlockParam>` |
| `return_code` | 是 | `number` |
| `stderr` | 是 | `string` |
| `stdout` | 是 | `string` |
| `type` | 是 | `'bash_code_execution_result'` |

### `BashCodeExecutionToolResultBlock`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `content` | 是 | `BashCodeExecutionToolResultError \| BashCodeExecutionResultBlock` |
| `tool_use_id` | 是 | `string` |
| `type` | 是 | `'bash_code_execution_tool_result'` |

### `BashCodeExecutionToolResultBlockParam`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `content` | 是 | `BashCodeExecutionToolResultErrorParam \| BashCodeExecutionResultBlockParam` |
| `tool_use_id` | 是 | `string` |
| `type` | 是 | `'bash_code_execution_tool_result'` |
| `cache_control` | 否 | `CacheControlEphemeral \| null` |

### `BashCodeExecutionToolResultError`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `error_code` | 是 | `BashCodeExecutionToolResultErrorCode` |
| `type` | 是 | `'bash_code_execution_tool_result_error'` |

### `BashCodeExecutionToolResultErrorCode`

类型别名：`\| 'invalid_tool_input' \| 'unavailable' \| 'too_many_requests' \| 'execution_time_exceeded' \| 'output_file_too_large'`

### `BashCodeExecutionToolResultErrorParam`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `error_code` | 是 | `BashCodeExecutionToolResultErrorCode` |
| `type` | 是 | `'bash_code_execution_tool_result_error'` |

### `CacheControlEphemeral`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `type` | 是 | `'ephemeral'` |
| `ttl` | 否 | `'5m' \| '1h'` |

### `CacheCreation`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `ephemeral_1h_input_tokens` | 是 | `number` |
| `ephemeral_5m_input_tokens` | 是 | `number` |

### `CitationCharLocation`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `cited_text` | 是 | `string` |
| `document_index` | 是 | `number` |
| `document_title` | 是 | `string \| null` |
| `end_char_index` | 是 | `number` |
| `file_id` | 是 | `string \| null` |
| `start_char_index` | 是 | `number` |
| `type` | 是 | `'char_location'` |

### `CitationCharLocationParam`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `cited_text` | 是 | `string` |
| `document_index` | 是 | `number` |
| `document_title` | 是 | `string \| null` |
| `end_char_index` | 是 | `number` |
| `start_char_index` | 是 | `number` |
| `type` | 是 | `'char_location'` |

### `CitationContentBlockLocation`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `cited_text` | 是 | `string` |
| `document_index` | 是 | `number` |
| `document_title` | 是 | `string \| null` |
| `end_block_index` | 是 | `number` |
| `file_id` | 是 | `string \| null` |
| `start_block_index` | 是 | `number` |
| `type` | 是 | `'content_block_location'` |

### `CitationContentBlockLocationParam`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `cited_text` | 是 | `string` |
| `document_index` | 是 | `number` |
| `document_title` | 是 | `string \| null` |
| `end_block_index` | 是 | `number` |
| `start_block_index` | 是 | `number` |
| `type` | 是 | `'content_block_location'` |

### `CitationPageLocation`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `cited_text` | 是 | `string` |
| `document_index` | 是 | `number` |
| `document_title` | 是 | `string \| null` |
| `end_page_number` | 是 | `number` |
| `file_id` | 是 | `string \| null` |
| `start_page_number` | 是 | `number` |
| `type` | 是 | `'page_location'` |

### `CitationPageLocationParam`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `cited_text` | 是 | `string` |
| `document_index` | 是 | `number` |
| `document_title` | 是 | `string \| null` |
| `end_page_number` | 是 | `number` |
| `start_page_number` | 是 | `number` |
| `type` | 是 | `'page_location'` |

### `CitationSearchResultLocationParam`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `cited_text` | 是 | `string` |
| `end_block_index` | 是 | `number` |
| `search_result_index` | 是 | `number` |
| `source` | 是 | `string` |
| `start_block_index` | 是 | `number` |
| `title` | 是 | `string \| null` |
| `type` | 是 | `'search_result_location'` |

### `CitationWebSearchResultLocationParam`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `cited_text` | 是 | `string` |
| `encrypted_index` | 是 | `string` |
| `title` | 是 | `string \| null` |
| `type` | 是 | `'web_search_result_location'` |
| `url` | 是 | `string` |

### `CitationsConfig`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `enabled` | 是 | `boolean` |

### `CitationsConfigParam`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `enabled` | 否 | `boolean` |

### `CitationsDelta`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `citation` | 是 | `\| CitationCharLocation \| CitationPageLocation \| CitationContentBlockLocation \| CitationsWebSearchResultLocation \| CitationsSearchResultLocation` |
| `type` | 是 | `'citations_delta'` |

### `CitationsSearchResultLocation`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `cited_text` | 是 | `string` |
| `end_block_index` | 是 | `number` |
| `search_result_index` | 是 | `number` |
| `source` | 是 | `string` |
| `start_block_index` | 是 | `number` |
| `title` | 是 | `string \| null` |
| `type` | 是 | `'search_result_location'` |

### `CitationsWebSearchResultLocation`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `cited_text` | 是 | `string` |
| `encrypted_index` | 是 | `string` |
| `title` | 是 | `string \| null` |
| `type` | 是 | `'web_search_result_location'` |
| `url` | 是 | `string` |

### `CodeExecutionOutputBlock`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `file_id` | 是 | `string` |
| `type` | 是 | `'code_execution_output'` |

### `CodeExecutionOutputBlockParam`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `file_id` | 是 | `string` |
| `type` | 是 | `'code_execution_output'` |

### `CodeExecutionResultBlock`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `content` | 是 | `Array<CodeExecutionOutputBlock>` |
| `return_code` | 是 | `number` |
| `stderr` | 是 | `string` |
| `stdout` | 是 | `string` |
| `type` | 是 | `'code_execution_result'` |

### `CodeExecutionResultBlockParam`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `content` | 是 | `Array<CodeExecutionOutputBlockParam>` |
| `return_code` | 是 | `number` |
| `stderr` | 是 | `string` |
| `stdout` | 是 | `string` |
| `type` | 是 | `'code_execution_result'` |

### `CodeExecutionTool20250522`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `name` | 是 | `'code_execution'` |
| `type` | 是 | `'code_execution_20250522'` |
| `allowed_callers` | 否 | `Array<'direct' \| 'code_execution_20250825' \| 'code_execution_20260120'>` |
| `cache_control` | 否 | `CacheControlEphemeral \| null` |
| `defer_loading` | 否 | `boolean` |
| `strict` | 否 | `boolean` |

### `CodeExecutionTool20250825`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `name` | 是 | `'code_execution'` |
| `type` | 是 | `'code_execution_20250825'` |
| `allowed_callers` | 否 | `Array<'direct' \| 'code_execution_20250825' \| 'code_execution_20260120'>` |
| `cache_control` | 否 | `CacheControlEphemeral \| null` |
| `defer_loading` | 否 | `boolean` |
| `strict` | 否 | `boolean` |

### `CodeExecutionTool20260120`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `name` | 是 | `'code_execution'` |
| `type` | 是 | `'code_execution_20260120'` |
| `allowed_callers` | 否 | `Array<'direct' \| 'code_execution_20250825' \| 'code_execution_20260120'>` |
| `cache_control` | 否 | `CacheControlEphemeral \| null` |
| `defer_loading` | 否 | `boolean` |
| `strict` | 否 | `boolean` |

### `CodeExecutionToolResultBlock`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `content` | 是 | `CodeExecutionToolResultBlockContent` |
| `tool_use_id` | 是 | `string` |
| `type` | 是 | `'code_execution_tool_result'` |

### `CodeExecutionToolResultBlockContent`

类型别名：`\| CodeExecutionToolResultError \| CodeExecutionResultBlock \| EncryptedCodeExecutionResultBlock`

### `CodeExecutionToolResultBlockParam`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `content` | 是 | `CodeExecutionToolResultBlockParamContent` |
| `tool_use_id` | 是 | `string` |
| `type` | 是 | `'code_execution_tool_result'` |
| `cache_control` | 否 | `CacheControlEphemeral \| null` |

### `CodeExecutionToolResultBlockParamContent`

类型别名：`\| CodeExecutionToolResultErrorParam \| CodeExecutionResultBlockParam \| EncryptedCodeExecutionResultBlockParam`

### `CodeExecutionToolResultError`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `error_code` | 是 | `CodeExecutionToolResultErrorCode` |
| `type` | 是 | `'code_execution_tool_result_error'` |

### `CodeExecutionToolResultErrorCode`

类型别名：`\| 'invalid_tool_input' \| 'unavailable' \| 'too_many_requests' \| 'execution_time_exceeded'`

### `CodeExecutionToolResultErrorParam`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `error_code` | 是 | `CodeExecutionToolResultErrorCode` |
| `type` | 是 | `'code_execution_tool_result_error'` |

### `Container`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `id` | 是 | `string` |
| `expires_at` | 是 | `string` |

### `ContainerUploadBlock`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `file_id` | 是 | `string` |
| `type` | 是 | `'container_upload'` |

### `ContainerUploadBlockParam`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `file_id` | 是 | `string` |
| `type` | 是 | `'container_upload'` |
| `cache_control` | 否 | `CacheControlEphemeral \| null` |

### `ContentBlock`

类型别名：`\| TextBlock \| ThinkingBlock \| RedactedThinkingBlock \| ToolUseBlock \| ServerToolUseBlock \| WebSearchToolResultBlock \| WebFetchToolResultBlock \| CodeExecutionToolResultBlock \| BashCodeExecutionToolResultBlock \| TextEditorCodeExecutionToolResultBlock \| ToolSearchToolResultBlock \| ContainerUploadBlock`

### `ContentBlockParam`

类型别名：`\| TextBlockParam \| ImageBlockParam \| DocumentBlockParam \| SearchResultBlockParam \| ThinkingBlockParam \| RedactedThinkingBlockParam \| ToolUseBlockParam \| ToolResultBlockParam \| ServerToolUseBlockParam \| WebSearchToolResultBlockParam \| WebFetchToolResultBlockParam \| CodeExecutionToolResultBlockParam \| BashCodeExecutionToolResultBlockParam \| TextEditorCodeExecutionToolResultBlockParam \| ToolSearchToolResultBlockParam \| ContainerUploadBlockParam \| MidConversationSystemBlockParam`

### `ContentBlockSource`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `content` | 是 | `string \| Array<ContentBlockSourceContent>` |
| `type` | 是 | `'content'` |

### `ContentBlockSourceContent`

类型别名：`TextBlockParam \| ImageBlockParam`

### `DirectCaller`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `type` | 是 | `'direct'` |

### `DocumentBlock`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `citations` | 是 | `CitationsConfig \| null` |
| `source` | 是 | `Base64PDFSource \| PlainTextSource` |
| `title` | 是 | `string \| null` |
| `type` | 是 | `'document'` |

### `DocumentBlockParam`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `source` | 是 | `Base64PDFSource \| PlainTextSource \| ContentBlockSource \| URLPDFSource` |
| `type` | 是 | `'document'` |
| `cache_control` | 否 | `CacheControlEphemeral \| null` |
| `citations` | 否 | `CitationsConfigParam \| null` |
| `context` | 否 | `string \| null` |
| `title` | 否 | `string \| null` |

### `EncryptedCodeExecutionResultBlock`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `content` | 是 | `Array<CodeExecutionOutputBlock>` |
| `encrypted_stdout` | 是 | `string` |
| `return_code` | 是 | `number` |
| `stderr` | 是 | `string` |
| `type` | 是 | `'encrypted_code_execution_result'` |

### `EncryptedCodeExecutionResultBlockParam`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `content` | 是 | `Array<CodeExecutionOutputBlockParam>` |
| `encrypted_stdout` | 是 | `string` |
| `return_code` | 是 | `number` |
| `stderr` | 是 | `string` |
| `type` | 是 | `'encrypted_code_execution_result'` |

### `ImageBlockParam`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `source` | 是 | `Base64ImageSource \| URLImageSource` |
| `type` | 是 | `'image'` |
| `cache_control` | 否 | `CacheControlEphemeral \| null` |

### `InputJSONDelta`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `partial_json` | 是 | `string` |
| `type` | 是 | `'input_json_delta'` |

### `JSONOutputFormat`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `schema` | 是 | `{ [key: string]: unknown }` |
| `type` | 是 | `'json_schema'` |

### `MemoryTool20250818`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `name` | 是 | `'memory'` |
| `type` | 是 | `'memory_20250818'` |
| `allowed_callers` | 否 | `Array<'direct' \| 'code_execution_20250825' \| 'code_execution_20260120'>` |
| `cache_control` | 否 | `CacheControlEphemeral \| null` |
| `defer_loading` | 否 | `boolean` |
| `input_examples` | 否 | `Array<{ [key: string]: unknown }>` |
| `strict` | 否 | `boolean` |

### `Message`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `id` | 是 | `string` |
| `container` | 是 | `Container \| null` |
| `content` | 是 | `Array<ContentBlock>` |
| `model` | 是 | `Model` |
| `role` | 是 | `'assistant'` |
| `stop_details` | 是 | `RefusalStopDetails \| null` |
| `stop_reason` | 是 | `StopReason \| null` |
| `stop_sequence` | 是 | `string \| null` |
| `type` | 是 | `'message'` |
| `usage` | 是 | `Usage` |

### `MessageCountTokensParams`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `messages` | 是 | `Array<MessageParam>` |
| `model` | 是 | `Model` |
| `cache_control` | 否 | `CacheControlEphemeral \| null` |
| `output_config` | 否 | `OutputConfig` |
| `system` | 否 | `string \| Array<TextBlockParam>` |
| `thinking` | 否 | `ThinkingConfigParam` |
| `tool_choice` | 否 | `ToolChoice` |
| `tools` | 否 | `Array<MessageCountTokensTool>` |

### `MessageCountTokensTool`

类型别名：`\| Tool \| ToolBash20250124 \| CodeExecutionTool20250522 \| CodeExecutionTool20250825 \| CodeExecutionTool20260120 \| MemoryTool20250818 \| ToolTextEditor20250124 \| ToolTextEditor20250429 \| ToolTextEditor20250728 \| WebSearchTool20250305 \| WebFetchTool20250910 \| WebSearchTool20260209 \| WebFetchTool20260209 \| WebFetchTool20260309 \| ToolSearchToolBm25_20251119 \| ToolSearchToolRegex20251119`

### `MessageCreateParams`

类型别名：`MessageCreateParamsNonStreaming \| MessageCreateParamsStreaming`

### `MessageCreateParamsBase`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `max_tokens` | 是 | `number` |
| `messages` | 是 | `Array<MessageParam>` |
| `model` | 是 | `Model` |
| `cache_control` | 否 | `CacheControlEphemeral \| null` |
| `container` | 否 | `string \| null` |
| `inference_geo` | 否 | `string \| null` |
| `metadata` | 否 | `Metadata` |
| `output_config` | 否 | `OutputConfig` |
| `service_tier` | 否 | `'auto' \| 'standard_only'` |
| `stop_sequences` | 否 | `Array<string>` |
| `stream` | 否 | `boolean` |
| `system` | 否 | `string \| Array<TextBlockParam>` |
| `temperature` | 否 | `number` |
| `thinking` | 否 | `ThinkingConfigParam` |
| `tool_choice` | 否 | `ToolChoice` |
| `tools` | 否 | `Array<ToolUnion>` |
| `top_k` | 否 | `number` |
| `top_p` | 否 | `number` |

### `MessageCreateParamsNonStreaming`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `stream` | 否 | `false` |

### `MessageCreateParamsStreaming`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `stream` | 是 | `true` |

### `MessageDeltaUsage`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `cache_creation_input_tokens` | 是 | `number \| null` |
| `cache_read_input_tokens` | 是 | `number \| null` |
| `input_tokens` | 是 | `number \| null` |
| `output_tokens` | 是 | `number` |
| `output_tokens_details` | 是 | `OutputTokensDetails \| null` |
| `server_tool_use` | 是 | `ServerToolUsage \| null` |

### `MessageParam`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `content` | 是 | `string \| Array<ContentBlockParam>` |
| `role` | 是 | `'user' \| 'assistant' \| 'system'` |

### `MessageTokensCount`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `input_tokens` | 是 | `number` |

### `Metadata`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `user_id` | 否 | `string \| null` |

### `MidConversationSystemBlockParam`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `content` | 是 | `Array<TextBlockParam>` |
| `type` | 是 | `'mid_conv_system'` |
| `cache_control` | 否 | `CacheControlEphemeral \| null` |

### `Model`

类型别名：`\| 'claude-opus-4-8' \| 'claude-opus-4-7' \| 'claude-mythos-preview' \| 'claude-opus-4-6' \| 'claude-sonnet-4-6' \| 'claude-haiku-4-5' \| 'claude-haiku-4-5-20251001' \| 'claude-opus-4-5' \| 'claude-opus-4-5-20251101' \| 'claude-sonnet-4-5' \| 'claude-sonnet-4-5-20250929' \| 'claude-opus-4-1' \| 'claude-opus-4-1-20250805' \| 'claude-opus-4-0' \| 'claude-opus-4-20250514' \| 'claude-sonnet-4-0' \| 'claude-sonnet-4-20250514' \| 'claude-3-haiku-20240307' \| (string & {})`

### `OutputConfig`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `effort` | 否 | `'low' \| 'medium' \| 'high' \| 'xhigh' \| 'max' \| null` |
| `format` | 否 | `JSONOutputFormat \| null` |

### `OutputTokensDetails`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `thinking_tokens` | 是 | `number` |

### `PlainTextSource`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `data` | 是 | `string` |
| `media_type` | 是 | `'text/plain'` |
| `type` | 是 | `'text'` |

### `RawContentBlockDelta`

类型别名：`\| TextDelta \| InputJSONDelta \| CitationsDelta \| ThinkingDelta \| SignatureDelta`

### `RawContentBlockDeltaEvent`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `delta` | 是 | `RawContentBlockDelta` |
| `index` | 是 | `number` |
| `type` | 是 | `'content_block_delta'` |

### `RawContentBlockStartEvent`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `content_block` | 是 | `\| TextBlock \| ThinkingBlock \| RedactedThinkingBlock \| ToolUseBlock \| ServerToolUseBlock \| WebSearchToolResultBlock \| WebFetchToolResultBlock \| CodeExecutionToolResultBlock \| BashCodeExecutionToolResultBlock \| TextEditorCodeExecutionToolResultBlock \| ToolSearchToolResultBlock \| ContainerUploadBlock` |
| `index` | 是 | `number` |
| `type` | 是 | `'content_block_start'` |

### `RawContentBlockStopEvent`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `index` | 是 | `number` |
| `type` | 是 | `'content_block_stop'` |

### `RawMessageDeltaEvent`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `delta` | 是 | `RawMessageDeltaEvent.Delta` |
| `type` | 是 | `'message_delta'` |
| `usage` | 是 | `MessageDeltaUsage` |

### `RawMessageStartEvent`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `message` | 是 | `Message` |
| `type` | 是 | `'message_start'` |

### `RawMessageStopEvent`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `type` | 是 | `'message_stop'` |

### `RawMessageStreamEvent`

类型别名：`\| RawMessageStartEvent \| RawMessageDeltaEvent \| RawMessageStopEvent \| RawContentBlockStartEvent \| RawContentBlockDeltaEvent \| RawContentBlockStopEvent`

### `RedactedThinkingBlock`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `data` | 是 | `string` |
| `type` | 是 | `'redacted_thinking'` |

### `RedactedThinkingBlockParam`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `data` | 是 | `string` |
| `type` | 是 | `'redacted_thinking'` |

### `RefusalStopDetails`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `category` | 是 | `'cyber' \| 'bio' \| null` |
| `explanation` | 是 | `string \| null` |
| `type` | 是 | `'refusal'` |

### `SearchResultBlockParam`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `content` | 是 | `Array<TextBlockParam>` |
| `source` | 是 | `string` |
| `title` | 是 | `string` |
| `type` | 是 | `'search_result'` |
| `cache_control` | 否 | `CacheControlEphemeral \| null` |
| `citations` | 否 | `CitationsConfigParam` |

### `ServerToolCaller`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `tool_id` | 是 | `string` |
| `type` | 是 | `'code_execution_20250825'` |

### `ServerToolCaller20260120`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `tool_id` | 是 | `string` |
| `type` | 是 | `'code_execution_20260120'` |

### `ServerToolUsage`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `web_fetch_requests` | 是 | `number` |
| `web_search_requests` | 是 | `number` |

### `ServerToolUseBlock`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `id` | 是 | `string` |
| `caller` | 是 | `DirectCaller \| ServerToolCaller \| ServerToolCaller20260120` |
| `input` | 是 | `unknown` |
| `name` | 是 | `\| 'web_search' \| 'web_fetch' \| 'code_execution' \| 'bash_code_execution' \| 'text_editor_code_execution' \| 'tool_search_tool_regex' \| 'tool_search_tool_bm25'` |
| `type` | 是 | `'server_tool_use'` |

### `ServerToolUseBlockParam`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `id` | 是 | `string` |
| `input` | 是 | `unknown` |
| `name` | 是 | `\| 'web_search' \| 'web_fetch' \| 'code_execution' \| 'bash_code_execution' \| 'text_editor_code_execution' \| 'tool_search_tool_regex' \| 'tool_search_tool_bm25'` |
| `type` | 是 | `'server_tool_use'` |
| `cache_control` | 否 | `CacheControlEphemeral \| null` |
| `caller` | 否 | `DirectCaller \| ServerToolCaller \| ServerToolCaller20260120` |

### `SignatureDelta`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `signature` | 是 | `string` |
| `type` | 是 | `'signature_delta'` |

### `StopReason`

类型别名：`'end_turn' \| 'max_tokens' \| 'stop_sequence' \| 'tool_use' \| 'pause_turn' \| 'refusal'`

### `TextBlock`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `citations` | 是 | `Array<TextCitation> \| null` |
| `text` | 是 | `string` |
| `type` | 是 | `'text'` |

### `TextBlockParam`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `text` | 是 | `string` |
| `type` | 是 | `'text'` |
| `cache_control` | 否 | `CacheControlEphemeral \| null` |
| `citations` | 否 | `Array<TextCitationParam> \| null` |

### `TextCitation`

类型别名：`\| CitationCharLocation \| CitationPageLocation \| CitationContentBlockLocation \| CitationsWebSearchResultLocation \| CitationsSearchResultLocation`

### `TextCitationParam`

类型别名：`\| CitationCharLocationParam \| CitationPageLocationParam \| CitationContentBlockLocationParam \| CitationWebSearchResultLocationParam \| CitationSearchResultLocationParam`

### `TextDelta`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `text` | 是 | `string` |
| `type` | 是 | `'text_delta'` |

### `TextEditorCodeExecutionCreateResultBlock`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `is_file_update` | 是 | `boolean` |
| `type` | 是 | `'text_editor_code_execution_create_result'` |

### `TextEditorCodeExecutionCreateResultBlockParam`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `is_file_update` | 是 | `boolean` |
| `type` | 是 | `'text_editor_code_execution_create_result'` |

### `TextEditorCodeExecutionStrReplaceResultBlock`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `lines` | 是 | `Array<string> \| null` |
| `new_lines` | 是 | `number \| null` |
| `new_start` | 是 | `number \| null` |
| `old_lines` | 是 | `number \| null` |
| `old_start` | 是 | `number \| null` |
| `type` | 是 | `'text_editor_code_execution_str_replace_result'` |

### `TextEditorCodeExecutionStrReplaceResultBlockParam`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `type` | 是 | `'text_editor_code_execution_str_replace_result'` |
| `lines` | 否 | `Array<string> \| null` |
| `new_lines` | 否 | `number \| null` |
| `new_start` | 否 | `number \| null` |
| `old_lines` | 否 | `number \| null` |
| `old_start` | 否 | `number \| null` |

### `TextEditorCodeExecutionToolResultBlock`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `content` | 是 | `\| TextEditorCodeExecutionToolResultError \| TextEditorCodeExecutionViewResultBlock \| TextEditorCodeExecutionCreateResultBlock \| TextEditorCodeExecutionStrReplaceResultBlock` |
| `tool_use_id` | 是 | `string` |
| `type` | 是 | `'text_editor_code_execution_tool_result'` |

### `TextEditorCodeExecutionToolResultBlockParam`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `content` | 是 | `\| TextEditorCodeExecutionToolResultErrorParam \| TextEditorCodeExecutionViewResultBlockParam \| TextEditorCodeExecutionCreateResultBlockParam \| TextEditorCodeExecutionStrReplaceResultBlockParam` |
| `tool_use_id` | 是 | `string` |
| `type` | 是 | `'text_editor_code_execution_tool_result'` |
| `cache_control` | 否 | `CacheControlEphemeral \| null` |

### `TextEditorCodeExecutionToolResultError`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `error_code` | 是 | `TextEditorCodeExecutionToolResultErrorCode` |
| `error_message` | 是 | `string \| null` |
| `type` | 是 | `'text_editor_code_execution_tool_result_error'` |

### `TextEditorCodeExecutionToolResultErrorCode`

类型别名：`\| 'invalid_tool_input' \| 'unavailable' \| 'too_many_requests' \| 'execution_time_exceeded' \| 'file_not_found'`

### `TextEditorCodeExecutionToolResultErrorParam`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `error_code` | 是 | `TextEditorCodeExecutionToolResultErrorCode` |
| `type` | 是 | `'text_editor_code_execution_tool_result_error'` |
| `error_message` | 否 | `string \| null` |

### `TextEditorCodeExecutionViewResultBlock`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `content` | 是 | `string` |
| `file_type` | 是 | `'text' \| 'image' \| 'pdf'` |
| `num_lines` | 是 | `number \| null` |
| `start_line` | 是 | `number \| null` |
| `total_lines` | 是 | `number \| null` |
| `type` | 是 | `'text_editor_code_execution_view_result'` |

### `TextEditorCodeExecutionViewResultBlockParam`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `content` | 是 | `string` |
| `file_type` | 是 | `'text' \| 'image' \| 'pdf'` |
| `type` | 是 | `'text_editor_code_execution_view_result'` |
| `num_lines` | 否 | `number \| null` |
| `start_line` | 否 | `number \| null` |
| `total_lines` | 否 | `number \| null` |

### `ThinkingBlock`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `signature` | 是 | `string` |
| `thinking` | 是 | `string` |
| `type` | 是 | `'thinking'` |

### `ThinkingBlockParam`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `signature` | 是 | `string` |
| `thinking` | 是 | `string` |
| `type` | 是 | `'thinking'` |

### `ThinkingConfigAdaptive`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `type` | 是 | `'adaptive'` |
| `display` | 否 | `'summarized' \| 'omitted' \| null` |

### `ThinkingConfigDisabled`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `type` | 是 | `'disabled'` |

### `ThinkingConfigEnabled`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `budget_tokens` | 是 | `number` |
| `type` | 是 | `'enabled'` |
| `display` | 否 | `'summarized' \| 'omitted' \| null` |

### `ThinkingConfigParam`

类型别名：`ThinkingConfigEnabled \| ThinkingConfigDisabled \| ThinkingConfigAdaptive`

### `ThinkingDelta`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `thinking` | 是 | `string` |
| `type` | 是 | `'thinking_delta'` |

### `Tool`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `input_schema` | 是 | `Tool.InputSchema` |
| `name` | 是 | `string` |
| `allowed_callers` | 否 | `Array<'direct' \| 'code_execution_20250825' \| 'code_execution_20260120'>` |
| `cache_control` | 否 | `CacheControlEphemeral \| null` |
| `defer_loading` | 否 | `boolean` |
| `description` | 否 | `string` |
| `eager_input_streaming` | 否 | `boolean \| null` |
| `input_examples` | 否 | `Array<{ [key: string]: unknown }>` |
| `strict` | 否 | `boolean` |
| `type` | 否 | `'custom' \| null` |

### `ToolBash20250124`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `name` | 是 | `'bash'` |
| `type` | 是 | `'bash_20250124'` |
| `allowed_callers` | 否 | `Array<'direct' \| 'code_execution_20250825' \| 'code_execution_20260120'>` |
| `cache_control` | 否 | `CacheControlEphemeral \| null` |
| `defer_loading` | 否 | `boolean` |
| `input_examples` | 否 | `Array<{ [key: string]: unknown }>` |
| `strict` | 否 | `boolean` |

### `ToolChoice`

类型别名：`ToolChoiceAuto \| ToolChoiceAny \| ToolChoiceTool \| ToolChoiceNone`

### `ToolChoiceAny`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `type` | 是 | `'any'` |
| `disable_parallel_tool_use` | 否 | `boolean` |

### `ToolChoiceAuto`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `type` | 是 | `'auto'` |
| `disable_parallel_tool_use` | 否 | `boolean` |

### `ToolChoiceNone`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `type` | 是 | `'none'` |

### `ToolChoiceTool`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `name` | 是 | `string` |
| `type` | 是 | `'tool'` |
| `disable_parallel_tool_use` | 否 | `boolean` |

### `ToolReferenceBlock`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `tool_name` | 是 | `string` |
| `type` | 是 | `'tool_reference'` |

### `ToolReferenceBlockParam`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `tool_name` | 是 | `string` |
| `type` | 是 | `'tool_reference'` |
| `cache_control` | 否 | `CacheControlEphemeral \| null` |

### `ToolResultBlockParam`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `tool_use_id` | 是 | `string` |
| `type` | 是 | `'tool_result'` |
| `cache_control` | 否 | `CacheControlEphemeral \| null` |
| `content` | 否 | `\| string \| Array< \| TextBlockParam \| ImageBlockParam \| SearchResultBlockParam \| DocumentBlockParam \| ToolReferenceBlockParam >` |
| `is_error` | 否 | `boolean` |

### `ToolSearchToolBm25_20251119`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `name` | 是 | `'tool_search_tool_bm25'` |
| `type` | 是 | `'tool_search_tool_bm25_20251119' \| 'tool_search_tool_bm25'` |
| `allowed_callers` | 否 | `Array<'direct' \| 'code_execution_20250825' \| 'code_execution_20260120'>` |
| `cache_control` | 否 | `CacheControlEphemeral \| null` |
| `defer_loading` | 否 | `boolean` |
| `strict` | 否 | `boolean` |

### `ToolSearchToolRegex20251119`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `name` | 是 | `'tool_search_tool_regex'` |
| `type` | 是 | `'tool_search_tool_regex_20251119' \| 'tool_search_tool_regex'` |
| `allowed_callers` | 否 | `Array<'direct' \| 'code_execution_20250825' \| 'code_execution_20260120'>` |
| `cache_control` | 否 | `CacheControlEphemeral \| null` |
| `defer_loading` | 否 | `boolean` |
| `strict` | 否 | `boolean` |

### `ToolSearchToolResultBlock`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `content` | 是 | `ToolSearchToolResultError \| ToolSearchToolSearchResultBlock` |
| `tool_use_id` | 是 | `string` |
| `type` | 是 | `'tool_search_tool_result'` |

### `ToolSearchToolResultBlockParam`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `content` | 是 | `ToolSearchToolResultErrorParam \| ToolSearchToolSearchResultBlockParam` |
| `tool_use_id` | 是 | `string` |
| `type` | 是 | `'tool_search_tool_result'` |
| `cache_control` | 否 | `CacheControlEphemeral \| null` |

### `ToolSearchToolResultError`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `error_code` | 是 | `ToolSearchToolResultErrorCode` |
| `error_message` | 是 | `string \| null` |
| `type` | 是 | `'tool_search_tool_result_error'` |

### `ToolSearchToolResultErrorCode`

类型别名：`\| 'invalid_tool_input' \| 'unavailable' \| 'too_many_requests' \| 'execution_time_exceeded'`

### `ToolSearchToolResultErrorParam`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `error_code` | 是 | `ToolSearchToolResultErrorCode` |
| `type` | 是 | `'tool_search_tool_result_error'` |

### `ToolSearchToolSearchResultBlock`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `tool_references` | 是 | `Array<ToolReferenceBlock>` |
| `type` | 是 | `'tool_search_tool_search_result'` |

### `ToolSearchToolSearchResultBlockParam`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `tool_references` | 是 | `Array<ToolReferenceBlockParam>` |
| `type` | 是 | `'tool_search_tool_search_result'` |

### `ToolTextEditor20250124`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `name` | 是 | `'str_replace_editor'` |
| `type` | 是 | `'text_editor_20250124'` |
| `allowed_callers` | 否 | `Array<'direct' \| 'code_execution_20250825' \| 'code_execution_20260120'>` |
| `cache_control` | 否 | `CacheControlEphemeral \| null` |
| `defer_loading` | 否 | `boolean` |
| `input_examples` | 否 | `Array<{ [key: string]: unknown }>` |
| `strict` | 否 | `boolean` |

### `ToolTextEditor20250429`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `name` | 是 | `'str_replace_based_edit_tool'` |
| `type` | 是 | `'text_editor_20250429'` |
| `allowed_callers` | 否 | `Array<'direct' \| 'code_execution_20250825' \| 'code_execution_20260120'>` |
| `cache_control` | 否 | `CacheControlEphemeral \| null` |
| `defer_loading` | 否 | `boolean` |
| `input_examples` | 否 | `Array<{ [key: string]: unknown }>` |
| `strict` | 否 | `boolean` |

### `ToolTextEditor20250728`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `name` | 是 | `'str_replace_based_edit_tool'` |
| `type` | 是 | `'text_editor_20250728'` |
| `allowed_callers` | 否 | `Array<'direct' \| 'code_execution_20250825' \| 'code_execution_20260120'>` |
| `cache_control` | 否 | `CacheControlEphemeral \| null` |
| `defer_loading` | 否 | `boolean` |
| `input_examples` | 否 | `Array<{ [key: string]: unknown }>` |
| `max_characters` | 否 | `number \| null` |
| `strict` | 否 | `boolean` |

### `ToolUnion`

类型别名：`\| Tool \| ToolBash20250124 \| CodeExecutionTool20250522 \| CodeExecutionTool20250825 \| CodeExecutionTool20260120 \| MemoryTool20250818 \| ToolTextEditor20250124 \| ToolTextEditor20250429 \| ToolTextEditor20250728 \| WebSearchTool20250305 \| WebFetchTool20250910 \| WebSearchTool20260209 \| WebFetchTool20260209 \| WebFetchTool20260309 \| ToolSearchToolBm25_20251119 \| ToolSearchToolRegex20251119`

### `ToolUseBlock`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `id` | 是 | `string` |
| `caller` | 是 | `DirectCaller \| ServerToolCaller \| ServerToolCaller20260120` |
| `input` | 是 | `unknown` |
| `name` | 是 | `string` |
| `type` | 是 | `'tool_use'` |

### `ToolUseBlockParam`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `id` | 是 | `string` |
| `input` | 是 | `unknown` |
| `name` | 是 | `string` |
| `type` | 是 | `'tool_use'` |
| `cache_control` | 否 | `CacheControlEphemeral \| null` |
| `caller` | 否 | `DirectCaller \| ServerToolCaller \| ServerToolCaller20260120` |

### `URLImageSource`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `type` | 是 | `'url'` |
| `url` | 是 | `string` |

### `URLPDFSource`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `type` | 是 | `'url'` |
| `url` | 是 | `string` |

### `Usage`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `cache_creation` | 是 | `CacheCreation \| null` |
| `cache_creation_input_tokens` | 是 | `number \| null` |
| `cache_read_input_tokens` | 是 | `number \| null` |
| `inference_geo` | 是 | `string \| null` |
| `input_tokens` | 是 | `number` |
| `output_tokens` | 是 | `number` |
| `output_tokens_details` | 是 | `OutputTokensDetails \| null` |
| `server_tool_use` | 是 | `ServerToolUsage \| null` |
| `service_tier` | 是 | `'standard' \| 'priority' \| 'batch' \| null` |

### `UserLocation`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `type` | 是 | `'approximate'` |
| `city` | 否 | `string \| null` |
| `country` | 否 | `string \| null` |
| `region` | 否 | `string \| null` |
| `timezone` | 否 | `string \| null` |

### `WebFetchBlock`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `content` | 是 | `DocumentBlock` |
| `retrieved_at` | 是 | `string \| null` |
| `type` | 是 | `'web_fetch_result'` |
| `url` | 是 | `string` |

### `WebFetchBlockParam`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `content` | 是 | `DocumentBlockParam` |
| `type` | 是 | `'web_fetch_result'` |
| `url` | 是 | `string` |
| `retrieved_at` | 否 | `string \| null` |

### `WebFetchTool20250910`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `name` | 是 | `'web_fetch'` |
| `type` | 是 | `'web_fetch_20250910'` |
| `allowed_callers` | 否 | `Array<'direct' \| 'code_execution_20250825' \| 'code_execution_20260120'>` |
| `allowed_domains` | 否 | `Array<string> \| null` |
| `blocked_domains` | 否 | `Array<string> \| null` |
| `cache_control` | 否 | `CacheControlEphemeral \| null` |
| `citations` | 否 | `CitationsConfigParam \| null` |
| `defer_loading` | 否 | `boolean` |
| `max_content_tokens` | 否 | `number \| null` |
| `max_uses` | 否 | `number \| null` |
| `strict` | 否 | `boolean` |

### `WebFetchTool20260209`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `name` | 是 | `'web_fetch'` |
| `type` | 是 | `'web_fetch_20260209'` |
| `allowed_callers` | 否 | `Array<'direct' \| 'code_execution_20250825' \| 'code_execution_20260120'>` |
| `allowed_domains` | 否 | `Array<string> \| null` |
| `blocked_domains` | 否 | `Array<string> \| null` |
| `cache_control` | 否 | `CacheControlEphemeral \| null` |
| `citations` | 否 | `CitationsConfigParam \| null` |
| `defer_loading` | 否 | `boolean` |
| `max_content_tokens` | 否 | `number \| null` |
| `max_uses` | 否 | `number \| null` |
| `strict` | 否 | `boolean` |

### `WebFetchTool20260309`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `name` | 是 | `'web_fetch'` |
| `type` | 是 | `'web_fetch_20260309'` |
| `allowed_callers` | 否 | `Array<'direct' \| 'code_execution_20250825' \| 'code_execution_20260120'>` |
| `allowed_domains` | 否 | `Array<string> \| null` |
| `blocked_domains` | 否 | `Array<string> \| null` |
| `cache_control` | 否 | `CacheControlEphemeral \| null` |
| `citations` | 否 | `CitationsConfigParam \| null` |
| `defer_loading` | 否 | `boolean` |
| `max_content_tokens` | 否 | `number \| null` |
| `max_uses` | 否 | `number \| null` |
| `strict` | 否 | `boolean` |
| `use_cache` | 否 | `boolean` |

### `WebFetchToolResultBlock`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `caller` | 是 | `DirectCaller \| ServerToolCaller \| ServerToolCaller20260120` |
| `content` | 是 | `WebFetchToolResultErrorBlock \| WebFetchBlock` |
| `tool_use_id` | 是 | `string` |
| `type` | 是 | `'web_fetch_tool_result'` |

### `WebFetchToolResultBlockParam`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `content` | 是 | `WebFetchToolResultErrorBlockParam \| WebFetchBlockParam` |
| `tool_use_id` | 是 | `string` |
| `type` | 是 | `'web_fetch_tool_result'` |
| `cache_control` | 否 | `CacheControlEphemeral \| null` |
| `caller` | 否 | `DirectCaller \| ServerToolCaller \| ServerToolCaller20260120` |

### `WebFetchToolResultErrorBlock`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `error_code` | 是 | `WebFetchToolResultErrorCode` |
| `type` | 是 | `'web_fetch_tool_result_error'` |

### `WebFetchToolResultErrorBlockParam`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `error_code` | 是 | `WebFetchToolResultErrorCode` |
| `type` | 是 | `'web_fetch_tool_result_error'` |

### `WebFetchToolResultErrorCode`

类型别名：`\| 'invalid_tool_input' \| 'url_too_long' \| 'url_not_allowed' \| 'url_not_in_prior_context' \| 'url_not_accessible' \| 'unsupported_content_type' \| 'too_many_requests' \| 'max_uses_exceeded' \| 'unavailable'`

### `WebSearchResultBlock`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `encrypted_content` | 是 | `string` |
| `page_age` | 是 | `string \| null` |
| `title` | 是 | `string` |
| `type` | 是 | `'web_search_result'` |
| `url` | 是 | `string` |

### `WebSearchResultBlockParam`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `encrypted_content` | 是 | `string` |
| `title` | 是 | `string` |
| `type` | 是 | `'web_search_result'` |
| `url` | 是 | `string` |
| `page_age` | 否 | `string \| null` |

### `WebSearchTool20250305`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `name` | 是 | `'web_search'` |
| `type` | 是 | `'web_search_20250305'` |
| `allowed_callers` | 否 | `Array<'direct' \| 'code_execution_20250825' \| 'code_execution_20260120'>` |
| `allowed_domains` | 否 | `Array<string> \| null` |
| `blocked_domains` | 否 | `Array<string> \| null` |
| `cache_control` | 否 | `CacheControlEphemeral \| null` |
| `defer_loading` | 否 | `boolean` |
| `max_uses` | 否 | `number \| null` |
| `strict` | 否 | `boolean` |
| `user_location` | 否 | `UserLocation \| null` |

### `WebSearchTool20260209`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `name` | 是 | `'web_search'` |
| `type` | 是 | `'web_search_20260209'` |
| `allowed_callers` | 否 | `Array<'direct' \| 'code_execution_20250825' \| 'code_execution_20260120'>` |
| `allowed_domains` | 否 | `Array<string> \| null` |
| `blocked_domains` | 否 | `Array<string> \| null` |
| `cache_control` | 否 | `CacheControlEphemeral \| null` |
| `defer_loading` | 否 | `boolean` |
| `max_uses` | 否 | `number \| null` |
| `strict` | 否 | `boolean` |
| `user_location` | 否 | `UserLocation \| null` |

### `WebSearchToolRequestError`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `error_code` | 是 | `WebSearchToolResultErrorCode` |
| `type` | 是 | `'web_search_tool_result_error'` |

### `WebSearchToolResultBlock`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `caller` | 是 | `DirectCaller \| ServerToolCaller \| ServerToolCaller20260120` |
| `content` | 是 | `WebSearchToolResultBlockContent` |
| `tool_use_id` | 是 | `string` |
| `type` | 是 | `'web_search_tool_result'` |

### `WebSearchToolResultBlockContent`

类型别名：`WebSearchToolResultError \| Array<WebSearchResultBlock>`

### `WebSearchToolResultBlockParam`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `content` | 是 | `WebSearchToolResultBlockParamContent` |
| `tool_use_id` | 是 | `string` |
| `type` | 是 | `'web_search_tool_result'` |
| `cache_control` | 否 | `CacheControlEphemeral \| null` |
| `caller` | 否 | `DirectCaller \| ServerToolCaller \| ServerToolCaller20260120` |

### `WebSearchToolResultBlockParamContent`

类型别名：`\| Array<WebSearchResultBlockParam> \| WebSearchToolRequestError`

### `WebSearchToolResultError`

| 字段 | 必填 | 类型 |
| --- | --- | --- |
| `error_code` | 是 | `WebSearchToolResultErrorCode` |
| `type` | 是 | `'web_search_tool_result_error'` |

### `WebSearchToolResultErrorCode`

类型别名：`\| 'invalid_tool_input' \| 'unavailable' \| 'max_uses_exceeded' \| 'too_many_requests' \| 'query_too_long' \| 'request_too_large'`

## Gemini Endpoints

| Method | Path | Request schema | Response schema | Aether format |
| --- | --- | --- | --- | --- |
| POST | `v1beta/{+model}:generateContent` | `GenerateContentRequest` | `GenerateContentResponse` | `gemini:generate_content` |
| POST | `v1beta/{+model}:streamGenerateContent` | `GenerateContentRequest` | `GenerateContentResponse (SSE)` | `gemini:generate_content` |
| POST | `v1beta/{+model}:embedContent` | `EmbedContentRequest` | `EmbedContentResponse` | `gemini:embedding` |
| POST | `v1beta/{+model}:batchEmbedContents` | `BatchEmbedContentsRequest` | `BatchEmbedContentsResponse` | `gemini:embedding` |
| POST | `v1beta/{+model}:countTokens` | `CountTokensRequest` | `CountTokensResponse` | `gemini:generate_content` |
| POST | `v1beta/files` | `CreateFileRequest` | `CreateFileResponse` | `gemini files` |
| GET | `v1beta/files` | - | `ListFilesResponse` | `gemini files` |
| GET | `v1beta/{+name}` | - | `File` | `gemini files` |
| DELETE | `v1beta/{+name}` | - | `Empty` | `gemini files` |
| POST | `v1beta/{+model}:predictLongRunning` | `PredictLongRunningRequest` | `Operation` | `gemini video` |

## Gemini Schema 字段表

以下 schema 从 Gemini native 接口根 schema 递归引用得到，共 97 个。

### `AttributionSourceId`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Identifier for the source contributing to this attribution. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `groundingPassage` | 否 | `GroundingPassageId` | - | Identifier for an inline passage. |
| `semanticRetrieverChunk` | 否 | `SemanticRetrieverChunk` | - | Identifier for a Chunk fetched via Semantic Retriever. |

### `AudioResponseFormat`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Configuration for audio output format. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `bitRate` | 否 | `integer(int32)` | - | Optional. Bit rate in bits per second (bps). Only applicable for compressed formats (MP3, Opus). |
| `delivery` | 否 | `string` | `DELIVERY_UNSPECIFIED`, `INLINE`, `URI` | Optional. The delivery mode for the audio output. |
| `mimeType` | 否 | `string` | `MIME_TYPE_UNSPECIFIED`, `AUDIO_MP3`, `AUDIO_OGG_OPUS`, `AUDIO_L16`, `AUDIO_WAV`, `AUDIO_ALAW`, `AUDIO_MULAW` | Optional. The MIME type of the audio output. |
| `sampleRate` | 否 | `integer(int32)` | - | Optional. Sample rate in Hz. |

### `BatchEmbedContentsRequest`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Batch request to get embeddings from the model for a list of prompts. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `requests` | 否 | `array<EmbedContentRequest>` | - | Required. Embed requests for the batch. The model in each of these requests must match the model specified BatchEmbedContentsRequest.model. |

### `BatchEmbedContentsResponse`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | The response to a BatchEmbedContentsRequest. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `embeddings` | 否 | `array<ContentEmbedding>` | - | Output only. The embeddings for each request, in the same order as provided in the batch request. |
| `usageMetadata` | 否 | `EmbeddingUsageMetadata` | - | Output only. The usage metadata for the request. |

### `Blob`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Raw media bytes. Text should not be sent as raw bytes, use the 'text' field. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `data` | 否 | `string(byte)` | - | Raw bytes for media formats. |
| `mimeType` | 否 | `string` | - | The IANA standard MIME type of the source data. Examples of supported types: - Images: image/png, image/jpeg, image/jpg, image/webp, image/heic, image/heif, image/gif, image/avif … |

### `Candidate`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A response candidate generated from the model. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `avgLogprobs` | 否 | `number(double)` | - | Output only. Average log probability score of the candidate. |
| `citationMetadata` | 否 | `CitationMetadata` | - | Output only. Citation information for model-generated candidate. This field may be populated with recitation information for any text included in the content. These are passages t… |
| `content` | 否 | `Content` | - | Output only. Generated content returned from the model. |
| `finishMessage` | 否 | `string` | - | Optional. Output only. Details the reason why the model stopped generating tokens. This is populated only when finish_reason is set. |
| `finishReason` | 否 | `string` | `FINISH_REASON_UNSPECIFIED`, `STOP`, `MAX_TOKENS`, `SAFETY`, `RECITATION`, `LANGUAGE`, `OTHER`, `BLOCKLIST`, `PROHIBITED_CONTENT`, `SPII`, `MALFORMED_FUNCTION_CALL`, `IMAGE_SAFETY`, `IMAGE_PROHIBITED_CONTENT`, `IMAGE_OTHER`, `NO_IMAGE`, `IMAGE_RECITATION`, `UNEXPECTED_TOOL_CALL`, `TOO_MANY_TOOL_CALLS`, `MISSING_THOUGHT_SIGNATURE`, `MALFORMED_RESPONSE`, `ESCALATION` | Optional. Output only. The reason why the model stopped generating tokens. If empty, the model has not stopped generating tokens. |
| `groundingAttributions` | 否 | `array<GroundingAttribution>` | - | Output only. Attribution information for sources that contributed to a grounded answer. This field is populated for GenerateAnswer calls. |
| `groundingMetadata` | 否 | `GroundingMetadata` | - | Output only. Grounding metadata for the candidate. This field is populated for GenerateContent calls. |
| `index` | 否 | `integer(int32)` | - | Output only. Index of the candidate in the list of response candidates. |
| `logprobsResult` | 否 | `LogprobsResult` | - | Output only. Log-likelihood scores for the response tokens and top tokens |
| `safetyRatings` | 否 | `array<SafetyRating>` | - | List of ratings for the safety of a response candidate. There is at most one rating per category. |
| `tokenCount` | 否 | `integer(int32)` | - | Output only. Token count for this candidate. |
| `urlContextMetadata` | 否 | `UrlContextMetadata` | - | Output only. Metadata related to url context retrieval tool. |

### `CitationMetadata`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A collection of source attributions for a piece of content. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `citationSources` | 否 | `array<CitationSource>` | - | Citations to sources for a specific response. |

### `CitationSource`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A citation to a source for a portion of a specific response. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `endIndex` | 否 | `integer(int32)` | - | Optional. End of the attributed segment, exclusive. |
| `license` | 否 | `string` | - | Optional. License for the GitHub project that is attributed as a source for segment. License info is required for code citations. |
| `startIndex` | 否 | `integer(int32)` | - | Optional. Start of segment of the response that is attributed to this source. Index indicates the start of the segment, measured in bytes. |
| `uri` | 否 | `string` | - | Optional. URI that is attributed as a source for a portion of the text. |

### `CodeExecution`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Tool that executes code generated by the model, and automatically returns the result to the model. See also ExecutableCode and CodeExecutionResult which are only generated when us… |

### `CodeExecutionResult`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Result of executing the ExecutableCode. Generated only when the CodeExecution tool is used. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `id` | 否 | `string` | - | Optional. The identifier of the ExecutableCode part this result is for. Only populated if the corresponding ExecutableCode has an id. |
| `outcome` | 否 | `string` | `OUTCOME_UNSPECIFIED`, `OUTCOME_OK`, `OUTCOME_FAILED`, `OUTCOME_DEADLINE_EXCEEDED` | Required. Outcome of the code execution. |
| `output` | 否 | `string` | - | Optional. Contains stdout when code execution is successful, stderr or other description otherwise. |

### `ComputerUse`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Computer Use tool type. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `environment` | 否 | `string` | `ENVIRONMENT_UNSPECIFIED`, `ENVIRONMENT_BROWSER` | Required. The environment being operated. |
| `excludedPredefinedFunctions` | 否 | `array<string>` | - | Optional. By default, predefined functions are included in the final model call. Some of them can be explicitly excluded from being automatically included. This can serve two purp… |

### `Content`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | The base structured datatype containing multi-part content of a message. A Content includes a role field designating the producer of the Content and a parts field containing multi… |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `parts` | 否 | `array<Part>` | - | Ordered Parts that constitute a single message. Parts may have different MIME types. |
| `role` | 否 | `string` | - | Optional. The producer of the content. Must be either 'user' or 'model'. Useful to set for multi-turn conversations, otherwise can be left blank or unset. |

### `ContentEmbedding`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A list of floats representing an embedding. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `shape` | 否 | `array<integer(int32)>` | - | This field stores the soft tokens tensor frame shape (e.g. [1, 1, 256, 2048]). |
| `values` | 否 | `array<number(float)>` | - | The embedding values. This is for 3P users only and will not be populated for 1P calls. |

### `CountTokensRequest`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Counts the number of tokens in the prompt sent to a model. Models may tokenize text differently, so each model may return a different token_count. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `contents` | 否 | `array<Content>` | - | Optional. The input given to the model as a prompt. This field is ignored when generate_content_request is set. |
| `generateContentRequest` | 否 | `GenerateContentRequest` | - | Optional. The overall input given to the Model. This includes the prompt as well as other model steering information like [system instructions](https://ai.google.dev/gemini-api/do… |

### `CountTokensResponse`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A response from CountTokens. It returns the model's token_count for the prompt. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `cacheTokensDetails` | 否 | `array<ModalityTokenCount>` | - | Output only. List of modalities that were processed in the cached content. |
| `cachedContentTokenCount` | 否 | `integer(int32)` | - | Number of tokens in the cached part of the prompt (the cached content). |
| `promptTokensDetails` | 否 | `array<ModalityTokenCount>` | - | Output only. List of modalities that were processed in the request input. |
| `totalTokens` | 否 | `integer(int32)` | - | The number of tokens that the Model tokenizes the prompt into. Always non-negative. |

### `CreateFileRequest`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Request for CreateFile. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `file` | 否 | `File` | - | Optional. Metadata for the file to create. |

### `CreateFileResponse`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Response for CreateFile. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `file` | 否 | `File` | - | Metadata for the created file. |

### `DynamicRetrievalConfig`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Describes the options to customize dynamic retrieval. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `dynamicThreshold` | 否 | `number(float)` | - | The threshold to be used in dynamic retrieval. If not set, a system default value is used. |
| `mode` | 否 | `string` | `MODE_UNSPECIFIED`, `MODE_DYNAMIC` | The mode of the predictor to be used in dynamic retrieval. |

### `EmbedContentConfig`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Configurations for the EmbedContent request. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `audioTrackExtraction` | 否 | `boolean` | - | Optional. Whether to extract audio from video content. |
| `autoTruncate` | 否 | `boolean` | - | Optional. Whether to silently truncate the input content if it's longer than the maximum sequence length. |
| `documentOcr` | 否 | `boolean` | - | Optional. Whether to enable OCR for document content. |
| `outputDimensionality` | 否 | `integer(int32)` | - | Optional. Reduced dimension for the output embedding. If set, excessive values in the output embedding are truncated from the end. |
| `taskType` | 否 | `string` | `TASK_TYPE_UNSPECIFIED`, `RETRIEVAL_QUERY`, `RETRIEVAL_DOCUMENT`, `SEMANTIC_SIMILARITY`, `CLASSIFICATION`, `CLUSTERING`, `QUESTION_ANSWERING`, `FACT_VERIFICATION`, `CODE_RETRIEVAL_QUERY` | Optional. The task type of the embedding. |
| `title` | 否 | `string` | - | Optional. The title for the text. |

### `EmbedContentRequest`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Request containing the Content for the model to embed. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `content` | 否 | `Content` | - | Required. The content to embed. Only the parts.text fields will be counted. |
| `embedContentConfig` | 否 | `EmbedContentConfig` | - | Optional. Configuration for the EmbedContent request. |
| `model` | 否 | `string` | - | Required. The model's resource name. This serves as an ID for the Model to use. This name should match a model name returned by the ListModels method. Format: models/{model} |
| `outputDimensionality` | 否 | `integer(int32)` | - | Optional. Deprecated: Please use EmbedContentConfig.output_dimensionality instead. Optional reduced dimension for the output embedding. If set, excessive values in the output embe… |
| `taskType` | 否 | `string` | `TASK_TYPE_UNSPECIFIED`, `RETRIEVAL_QUERY`, `RETRIEVAL_DOCUMENT`, `SEMANTIC_SIMILARITY`, `CLASSIFICATION`, `CLUSTERING`, `QUESTION_ANSWERING`, `FACT_VERIFICATION`, `CODE_RETRIEVAL_QUERY` | Optional. Deprecated: Please use EmbedContentConfig.task_type instead. Optional task type for which the embeddings will be used. Not supported on earlier models (models/embedding-… |
| `title` | 否 | `string` | - | Optional. Deprecated: Please use EmbedContentConfig.title instead. An optional title for the text. Only applicable when TaskType is RETRIEVAL_DOCUMENT. Note: Specifying a title fo… |

### `EmbedContentResponse`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | The response to an EmbedContentRequest. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `embedding` | 否 | `ContentEmbedding` | - | Output only. The embedding generated from the input content. |
| `usageMetadata` | 否 | `EmbeddingUsageMetadata` | - | Output only. The usage metadata for the request. |

### `EmbeddingUsageMetadata`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Metadata on the usage of the embedding request. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `promptTokenCount` | 否 | `integer(int32)` | - | Output only. Number of tokens in the prompt. |
| `promptTokenDetails` | 否 | `array<ModalityTokenCount>` | - | Output only. List of modalities that were processed in the request input. |

### `ExecutableCode`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Code generated by the model that is meant to be executed, and the result returned to the model. Only generated when using the CodeExecution tool, in which the code will be automat… |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `code` | 否 | `string` | - | Required. The code to be executed. |
| `id` | 否 | `string` | - | Optional. Unique identifier of the ExecutableCode part. The server returns the CodeExecutionResult with the matching id. |
| `language` | 否 | `string` | `LANGUAGE_UNSPECIFIED`, `PYTHON` | Required. Programming language of the code. |

### `File`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A file uploaded to the API. Next ID: 15 |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `createTime` | 否 | `string(google-datetime)` | - | Output only. The timestamp of when the File was created. |
| `displayName` | 否 | `string` | - | Optional. The human-readable display name for the File. The display name must be no more than 512 characters in length, including spaces. Example: "Welcome Image" |
| `downloadUri` | 否 | `string` | - | Output only. The download uri of the File. |
| `error` | 否 | `Status` | - | Output only. Error status if File processing failed. |
| `expirationTime` | 否 | `string(google-datetime)` | - | Output only. The timestamp of when the File will be deleted. Only set if the File is scheduled to expire. |
| `mimeType` | 否 | `string` | - | Output only. MIME type of the file. |
| `name` | 否 | `string` | - | Immutable. Identifier. The File resource name. The ID (name excluding the "files/" prefix) can contain up to 40 characters that are lowercase alphanumeric or dashes (-). The ID ca… |
| `sha256Hash` | 否 | `string(byte)` | - | Output only. SHA-256 hash of the uploaded bytes. |
| `sizeBytes` | 否 | `string(int64)` | - | Output only. Size of the file in bytes. |
| `source` | 否 | `string` | `SOURCE_UNSPECIFIED`, `UPLOADED`, `GENERATED`, `REGISTERED` | Source of the File. |
| `state` | 否 | `string` | `STATE_UNSPECIFIED`, `PROCESSING`, `ACTIVE`, `FAILED` | Output only. Processing state of the File. |
| `updateTime` | 否 | `string(google-datetime)` | - | Output only. The timestamp of when the File was last updated. |
| `uri` | 否 | `string` | - | Output only. The uri of the File. |
| `videoMetadata` | 否 | `VideoFileMetadata` | - | Output only. Metadata for a video. |

### `FileData`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | URI based data. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `fileUri` | 否 | `string` | - | Required. URI. |
| `mimeType` | 否 | `string` | - | Optional. The IANA standard MIME type of the source data. |

### `FileSearch`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | The FileSearch tool that retrieves knowledge from Semantic Retrieval corpora. Files are imported to Semantic Retrieval corpora using the ImportFile API. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `fileSearchStoreNames` | 否 | `array<string>` | - | Required. The names of the file_search_stores to retrieve from. Example: fileSearchStores/my-file-search-store-123 |
| `metadataFilter` | 否 | `string` | - | Optional. Metadata filter to apply to the semantic retrieval documents and chunks. |
| `topK` | 否 | `integer(int32)` | - | Optional. The number of semantic retrieval chunks to retrieve. |

### `FunctionCall`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A predicted FunctionCall returned from the model that contains a string representing the FunctionDeclaration.name with the arguments and their values. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `args` | 否 | `object/map<string, any>` | - | Optional. The function parameters and values in JSON object format. |
| `id` | 否 | `string` | - | Optional. Unique identifier of the function call. If populated, the client to execute the function_call and return the response with the matching id. |
| `name` | 否 | `string` | - | Required. The name of the function to call. Must be a-z, A-Z, 0-9, or contain underscores and dashes, with a maximum length of 128. |

### `FunctionCallingConfig`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Configuration for specifying function calling behavior. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `allowedFunctionNames` | 否 | `array<string>` | - | Optional. A set of function names that, when provided, limits the functions the model will call. This should only be set when the Mode is ANY or VALIDATED. Function names should m… |
| `mode` | 否 | `string` | `MODE_UNSPECIFIED`, `AUTO`, `ANY`, `NONE`, `VALIDATED` | Optional. Specifies the mode in which function calling should execute. If unspecified, the default value will be set to AUTO. |

### `FunctionDeclaration`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Structured representation of a function declaration as defined by the [OpenAPI 3.03 specification](https://spec.openapis.org/oas/v3.0.3). Included in this declaration are the func… |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `behavior` | 否 | `string` | `UNSPECIFIED`, `BLOCKING`, `NON_BLOCKING` | Optional. Specifies the function Behavior. Currently only supported by the BidiGenerateContent method. |
| `description` | 否 | `string` | - | Required. A brief description of the function. |
| `name` | 否 | `string` | - | Required. The name of the function. Must be a-z, A-Z, 0-9, or contain underscores, colons, dots, and dashes, with a maximum length of 128. |
| `parameters` | 否 | `Schema` | - | Optional. Describes the parameters to this function. Reflects the Open API 3.03 Parameter Object string Key: the name of the parameter. Parameter names are case sensitive. Schema … |
| `parametersJsonSchema` | 否 | `any` | - | Optional. Describes the parameters to the function in JSON Schema format. The schema must describe an object where the properties are the parameters to the function. For example: … |
| `response` | 否 | `Schema` | - | Optional. Describes the output from this function in JSON Schema format. Reflects the Open API 3.03 Response Object. The Schema defines the type used for the response value of the… |
| `responseJsonSchema` | 否 | `any` | - | Optional. Describes the output from this function in JSON Schema format. The value specified by the schema is the response value of the function. This field is mutually exclusive … |

### `FunctionResponse`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | The result output from a FunctionCall that contains a string representing the FunctionDeclaration.name and a structured JSON object containing any output from the function is used… |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `id` | 否 | `string` | - | Optional. The identifier of the function call this response is for. Populated by the client to match the corresponding function call id. |
| `name` | 否 | `string` | - | Required. The name of the function to call. Must be a-z, A-Z, 0-9, or contain underscores and dashes, with a maximum length of 128. |
| `parts` | 否 | `array<FunctionResponsePart>` | - | Optional. Ordered Parts that constitute a function response. Parts may have different IANA MIME types. |
| `response` | 否 | `object/map<string, any>` | - | Required. The function response in JSON object format. Callers can use any keys of their choice that fit the function's syntax to return the function output, e.g. "output", "resul… |
| `scheduling` | 否 | `string` | `SCHEDULING_UNSPECIFIED`, `SILENT`, `WHEN_IDLE`, `INTERRUPT` | Optional. Specifies how the response should be scheduled in the conversation. Only applicable to NON_BLOCKING function calls, is ignored otherwise. Defaults to WHEN_IDLE. |
| `willContinue` | 否 | `boolean` | - | Optional. Signals that function call continues, and more responses will be returned, turning the function call into a generator. Is only applicable to NON_BLOCKING function calls,… |

### `FunctionResponseBlob`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Raw media bytes for function response. Text should not be sent as raw bytes, use the 'FunctionResponse.response' field. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `data` | 否 | `string(byte)` | - | Raw bytes for media formats. |
| `mimeType` | 否 | `string` | - | The IANA standard MIME type of the source data. Examples: - image/png - image/jpeg If an unsupported MIME type is provided, an error will be returned. For a complete list of suppo… |

### `FunctionResponsePart`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A datatype containing media that is part of a FunctionResponse message. A FunctionResponsePart consists of data which has an associated datatype. A FunctionResponsePart can only c… |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `inlineData` | 否 | `FunctionResponseBlob` | - | Inline media bytes. |

### `GenerateContentRequest`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Request to generate a completion from the model. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `cachedContent` | 否 | `string` | - | Optional. The name of the content [cached](https://ai.google.dev/gemini-api/docs/caching) to use as context to serve the prediction. Format: cachedContents/{cachedContent} |
| `contents` | 否 | `array<Content>` | - | Required. The content of the current conversation with the model. For single-turn queries, this is a single instance. For multi-turn queries like [chat](https://ai.google.dev/gemi… |
| `generationConfig` | 否 | `GenerationConfig` | - | Optional. Configuration options for model generation and outputs. |
| `model` | 否 | `string` | - | Required. The name of the Model to use for generating the completion. Format: models/{model}. |
| `safetySettings` | 否 | `array<SafetySetting>` | - | Optional. A list of unique SafetySetting instances for blocking unsafe content. This will be enforced on the GenerateContentRequest.contents and GenerateContentResponse.candidates… |
| `serviceTier` | 否 | `string` | `unspecified`, `standard`, `flex`, `priority` | Optional. The service tier of the request. |
| `store` | 否 | `boolean` | - | Optional. Configures the logging behavior for a given request. If set, it takes precedence over the project-level logging config. |
| `systemInstruction` | 否 | `Content` | - | Optional. Developer set [system instruction(s)](https://ai.google.dev/gemini-api/docs/system-instructions). Currently, text only. |
| `toolConfig` | 否 | `ToolConfig` | - | Optional. Tool configuration for any Tool specified in the request. Refer to the [Function calling guide](https://ai.google.dev/gemini-api/docs/function-calling#function_calling_m… |
| `tools` | 否 | `array<Tool>` | - | Optional. A list of Tools the Model may use to generate the next response. A Tool is a piece of code that enables the system to interact with external systems to perform an action… |

### `GenerateContentResponse`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Response from the model supporting multiple candidate responses. Safety ratings and content filtering are reported for both prompt in GenerateContentResponse.prompt_feedback and f… |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `candidates` | 否 | `array<Candidate>` | - | Candidate responses from the model. |
| `modelStatus` | 否 | `ModelStatus` | - | Output only. The current model status of this model. |
| `modelVersion` | 否 | `string` | - | Output only. The model version used to generate the response. |
| `promptFeedback` | 否 | `PromptFeedback` | - | Returns the prompt's feedback related to the content filters. |
| `responseId` | 否 | `string` | - | Output only. response_id is used to identify each response. |
| `usageMetadata` | 否 | `UsageMetadata` | - | Output only. Metadata on the generation requests' token usage. |

### `GenerationConfig`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Configuration options for model generation and outputs. Not all parameters are configurable for every model. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `_responseJsonSchema` | 否 | `any` | - | Optional. Output schema of the generated response. This is an alternative to response_schema that accepts [JSON Schema](https://json-schema.org/). If set, response_schema must be … |
| `candidateCount` | 否 | `integer(int32)` | - | Optional. Number of generated responses to return. If unset, this will default to 1. Please note that this doesn't work for previous generation models (Gemini 1.0 family) |
| `enableEnhancedCivicAnswers` | 否 | `boolean` | - | Optional. Enables enhanced civic answers. It may not be available for all models. |
| `frequencyPenalty` | 否 | `number(float)` | - | Optional. Frequency penalty applied to the next token's logprobs, multiplied by the number of times each token has been seen in the respponse so far. A positive penalty will disco… |
| `imageConfig` | 否 | `ImageConfig` | - | Optional. Config for image generation. An error will be returned if this field is set for models that don't support these config options. |
| `logprobs` | 否 | `integer(int32)` | - | Optional. Only valid if response_logprobs=True. This sets the number of top logprobs, including the chosen candidate, to return at each decoding step in the Candidate.logprobs_res… |
| `maxOutputTokens` | 否 | `integer(int32)` | - | Optional. The maximum number of tokens to include in a response candidate. Note: The default value varies by model, see the Model.output_token_limit attribute of the Model returne… |
| `mediaResolution` | 否 | `string` | `MEDIA_RESOLUTION_UNSPECIFIED`, `MEDIA_RESOLUTION_LOW`, `MEDIA_RESOLUTION_MEDIUM`, `MEDIA_RESOLUTION_HIGH` | Optional. If specified, the media resolution specified will be used. |
| `presencePenalty` | 否 | `number(float)` | - | Optional. Presence penalty applied to the next token's logprobs if the token has already been seen in the response. This penalty is binary on/off and not dependant on the number o… |
| `responseFormat` | 否 | `ResponseFormatConfig` | - | Optional. Configuration for the response output format. Allows specifying output configuration per modality (text, audio, image) in a flat structure. |
| `responseJsonSchema` | 否 | `any` | - | Optional. An internal detail. Use responseJsonSchema rather than this field. |
| `responseLogprobs` | 否 | `boolean` | - | Optional. If true, export the logprobs results in response. |
| `responseMimeType` | 否 | `string` | - | Optional. MIME type of the generated candidate text. Supported MIME types are: text/plain: (default) Text output. application/json: JSON response in the response candidates. text/… |
| `responseModalities` | 否 | `array<string>` | - | Optional. The requested modalities of the response. Represents the set of modalities that the model can return, and should be expected in the response. This is an exact match to t… |
| `responseSchema` | 否 | `Schema` | - | Optional. Output schema of the generated candidate text. Schemas must be a subset of the [OpenAPI schema](https://spec.openapis.org/oas/v3.0.3#schema) and can be objects, primitiv… |
| `seed` | 否 | `integer(int32)` | - | Optional. Seed used in decoding. If not set, the request uses a randomly generated seed. |
| `speechConfig` | 否 | `SpeechConfig` | - | Optional. The speech generation config. |
| `stopSequences` | 否 | `array<string>` | - | Optional. The set of character sequences (up to 5) that will stop output generation. If specified, the API will stop at the first appearance of a stop_sequence. The stop sequence … |
| `temperature` | 否 | `number(float)` | - | Optional. Controls the randomness of the output. Note: The default value varies by model, see the Model.temperature attribute of the Model returned from the getModel function. Val… |
| `thinkingConfig` | 否 | `ThinkingConfig` | - | Optional. Config for thinking features. An error will be returned if this field is set for models that don't support thinking. |
| `topK` | 否 | `integer(int32)` | - | Optional. The maximum number of tokens to consider when sampling. Gemini models use Top-p (nucleus) sampling or a combination of Top-k and nucleus sampling. Top-k sampling conside… |
| `topP` | 否 | `number(float)` | - | Optional. The maximum cumulative probability of tokens to consider when sampling. The model uses combined Top-k and Top-p (nucleus) sampling. Tokens are sorted based on their assi… |

### `GoogleAiGenerativelanguageV1betaGroundingSupport`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Grounding support. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `confidenceScores` | 否 | `array<number(float)>` | - | Optional. Confidence score of the support references. Ranges from 0 to 1. 1 is the most confident. This list must have the same size as the grounding_chunk_indices. |
| `groundingChunkIndices` | 否 | `array<integer(int32)>` | - | Optional. A list of indices (into 'grounding_chunk' in response.candidate.grounding_metadata) specifying the citations associated with the claim. For instance [1,3,4] means that g… |
| `renderedParts` | 否 | `array<integer(int32)>` | - | Output only. Indices into the parts field of the candidate's content. These indices specify which rendered parts are associated with this support source. |
| `segment` | 否 | `GoogleAiGenerativelanguageV1betaSegment` | - | Segment of the content this support belongs to. |

### `GoogleAiGenerativelanguageV1betaSegment`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Segment of the content. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `endIndex` | 否 | `integer(int32)` | - | End index in the given Part, measured in bytes. Offset from the start of the Part, exclusive, starting at zero. |
| `partIndex` | 否 | `integer(int32)` | - | The index of a Part object within its parent Content object. |
| `startIndex` | 否 | `integer(int32)` | - | Start index in the given Part, measured in bytes. Offset from the start of the Part, inclusive, starting at zero. |
| `text` | 否 | `string` | - | The text corresponding to the segment from the response. |

### `GoogleMaps`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | The GoogleMaps Tool that provides geospatial context for the user's query. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `enableWidget` | 否 | `boolean` | - | Optional. Whether to return a widget context token in the GroundingMetadata of the response. Developers can use the widget context token to render a Google Maps widget with geospa… |

### `GoogleSearch`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | GoogleSearch tool type. Tool to support Google Search in Model. Powered by Google. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `searchTypes` | 否 | `SearchTypes` | - | Optional. The set of search types to enable. If not set, web search is enabled by default. |
| `timeRangeFilter` | 否 | `Interval` | - | Optional. Filter search results to a specific time range. If customers set a start time, they must set an end time (and vice versa). |

### `GoogleSearchRetrieval`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Tool to retrieve public web data for grounding, powered by Google. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `dynamicRetrievalConfig` | 否 | `DynamicRetrievalConfig` | - | Specifies the dynamic retrieval configuration for the given source. |

### `GroundingAttribution`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Attribution for a source that contributed to an answer. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `content` | 否 | `Content` | - | Grounding source content that makes up this attribution. |
| `sourceId` | 否 | `AttributionSourceId` | - | Output only. Identifier for the source contributing to this attribution. |

### `GroundingChunk`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A GroundingChunk represents a segment of supporting evidence that grounds the model's response. It can be a chunk from the web, a retrieved context from a file, or information fro… |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `image` | 否 | `Image` | - | Optional. Grounding chunk from image search. |
| `maps` | 否 | `Maps` | - | Optional. Grounding chunk from Google Maps. |
| `retrievedContext` | 否 | `RetrievedContext` | - | Optional. Grounding chunk from context retrieved by the file search tool. |
| `web` | 否 | `Web` | - | Grounding chunk from the web. |

### `GroundingChunkCustomMetadata`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | User provided metadata about the GroundingFact. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `key` | 否 | `string` | - | The key of the metadata. |
| `numericValue` | 否 | `number(float)` | - | Optional. The numeric value of the metadata. The expected range for this value depends on the specific key used. |
| `stringListValue` | 否 | `GroundingChunkStringList` | - | Optional. A list of string values for the metadata. |
| `stringValue` | 否 | `string` | - | Optional. The string value of the metadata. |

### `GroundingChunkStringList`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A list of string values. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `values` | 否 | `array<string>` | - | The string values of the list. |

### `GroundingMetadata`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Metadata returned to client when grounding is enabled. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `googleMapsWidgetContextToken` | 否 | `string` | - | Optional. Resource name of the Google Maps widget context token that can be used with the PlacesContextElement widget in order to render contextual data. Only populated in the cas… |
| `groundingChunks` | 否 | `array<GroundingChunk>` | - | List of supporting references retrieved from specified grounding source. When streaming, this only contains the grounding chunks that have not been included in the grounding metad… |
| `groundingSupports` | 否 | `array<GoogleAiGenerativelanguageV1betaGroundingSupport>` | - | List of grounding support. |
| `imageSearchQueries` | 否 | `array<string>` | - | Image search queries used for grounding. |
| `retrievalMetadata` | 否 | `RetrievalMetadata` | - | Metadata related to retrieval in the grounding flow. |
| `searchEntryPoint` | 否 | `SearchEntryPoint` | - | Optional. Google search entry for the following-up web searches. |
| `webSearchQueries` | 否 | `array<string>` | - | Web search queries for the following-up web search. |

### `GroundingPassageId`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Identifier for a part within a GroundingPassage. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `partIndex` | 否 | `integer(int32)` | - | Output only. Index of the part within the GenerateAnswerRequest's GroundingPassage.content. |
| `passageId` | 否 | `string` | - | Output only. ID of the passage matching the GenerateAnswerRequest's GroundingPassage.id. |

### `Image`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Chunk from image search. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `domain` | 否 | `string` | - | The root domain of the web page that the image is from, e.g. "example.com". |
| `imageUri` | 否 | `string` | - | The image asset URL. |
| `sourceUri` | 否 | `string` | - | The web page URI for attribution. |
| `title` | 否 | `string` | - | The title of the web page that the image is from. |

### `ImageConfig`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Config for image generation features. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `aspectRatio` | 否 | `string` | - | Optional. The aspect ratio of the image to generate. Supported aspect ratios: 1:1, 1:4, 4:1, 1:8, 8:1, 2:3, 3:2, 3:4, 4:3, 4:5, 5:4, 9:16, 16:9, or 21:9. If not specified, the mod… |
| `imageSize` | 否 | `string` | - | Optional. Specifies the size of generated images. Supported values are 512, 1K, 2K, 4K. If not specified, the model will use default value 1K. |

### `ImageResponseFormat`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Configuration for image output format. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `aspectRatio` | 否 | `string` | `ASPECT_RATIO_UNSPECIFIED`, `ASPECT_RATIO_ONE_BY_ONE`, `ASPECT_RATIO_TWO_BY_THREE`, `ASPECT_RATIO_THREE_BY_TWO`, `ASPECT_RATIO_THREE_BY_FOUR`, `ASPECT_RATIO_FOUR_BY_THREE`, `ASPECT_RATIO_FOUR_BY_FIVE`, `ASPECT_RATIO_FIVE_BY_FOUR`, `ASPECT_RATIO_NINE_BY_SIXTEEN`, `ASPECT_RATIO_SIXTEEN_BY_NINE`, `ASPECT_RATIO_TWENTY_ONE_BY_NINE`, `ASPECT_RATIO_ONE_BY_EIGHT`, `ASPECT_RATIO_EIGHT_BY_ONE`, `ASPECT_RATIO_ONE_BY_FOUR`, `ASPECT_RATIO_FOUR_BY_ONE` | Optional. The aspect ratio for the image output. |
| `delivery` | 否 | `string` | `DELIVERY_UNSPECIFIED`, `INLINE`, `URI` | Optional. The delivery mode for the image output. |
| `imageSize` | 否 | `string` | `IMAGE_SIZE_UNSPECIFIED`, `IMAGE_SIZE_FIVE_TWELVE`, `IMAGE_SIZE_ONE_K`, `IMAGE_SIZE_TWO_K`, `IMAGE_SIZE_FOUR_K` | Optional. The size of the image output. |
| `mimeType` | 否 | `string` | `MIME_TYPE_UNSPECIFIED`, `IMAGE_JPEG` | Optional. The MIME type of the image output. |

### `ImageSearch`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Image search for grounding and related configurations. |

### `Interval`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Represents a time interval, encoded as a Timestamp start (inclusive) and a Timestamp end (exclusive). The start must be less than or equal to the end. When the start equals the en… |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `endTime` | 否 | `string(google-datetime)` | - | Optional. Exclusive end of the interval. If specified, a Timestamp matching this interval will have to be before the end. |
| `startTime` | 否 | `string(google-datetime)` | - | Optional. Inclusive start of the interval. If specified, a Timestamp matching this interval will have to be the same or after the start. |

### `LatLng`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | An object that represents a latitude/longitude pair. This is expressed as a pair of doubles to represent degrees latitude and degrees longitude. Unless specified otherwise, this o… |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `latitude` | 否 | `number(double)` | - | The latitude in degrees. It must be in the range [-90.0, +90.0]. |
| `longitude` | 否 | `number(double)` | - | The longitude in degrees. It must be in the range [-180.0, +180.0]. |

### `ListFilesResponse`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Response for ListFiles. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `files` | 否 | `array<File>` | - | The list of Files. |
| `nextPageToken` | 否 | `string` | - | A token that can be sent as a page_token into a subsequent ListFiles call. |

### `LogprobsResult`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Logprobs Result |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `chosenCandidates` | 否 | `array<LogprobsResultCandidate>` | - | Length = total number of decoding steps. The chosen candidates may or may not be in top_candidates. |
| `logProbabilitySum` | 否 | `number(float)` | - | Sum of log probabilities for all tokens. |
| `topCandidates` | 否 | `array<TopCandidates>` | - | Length = total number of decoding steps. |

### `LogprobsResultCandidate`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Candidate for the logprobs token and score. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `logProbability` | 否 | `number(float)` | - | The candidate's log probability. |
| `token` | 否 | `string` | - | The candidate’s token string value. |
| `tokenId` | 否 | `integer(int32)` | - | The candidate’s token id value. |

### `Maps`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A grounding chunk from Google Maps. A Maps chunk corresponds to a single place. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `placeAnswerSources` | 否 | `PlaceAnswerSources` | - | Sources that provide answers about the features of a given place in Google Maps. |
| `placeId` | 否 | `string` | - | The ID of the place, in places/{place_id} format. A user can use this ID to look up that place. |
| `text` | 否 | `string` | - | Text description of the place answer. |
| `title` | 否 | `string` | - | Title of the place. |
| `uri` | 否 | `string` | - | URI reference of the place. |

### `McpServer`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A MCPServer is a server that can be called by the model to perform actions. It is a server that implements the MCP protocol. Next ID: 6 |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `name` | 否 | `string` | - | The name of the MCPServer. |
| `streamableHttpTransport` | 否 | `StreamableHttpTransport` | - | A transport that can stream HTTP requests and responses. |

### `ModalityTokenCount`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Represents token counting info for a single modality. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `modality` | 否 | `string` | `MODALITY_UNSPECIFIED`, `TEXT`, `IMAGE`, `VIDEO`, `AUDIO`, `DOCUMENT` | The modality associated with this token count. |
| `tokenCount` | 否 | `integer(int32)` | - | Number of tokens. |

### `ModelStatus`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | The status of the underlying model. This is used to indicate the stage of the underlying model and the retirement time if applicable. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `message` | 否 | `string` | - | A message explaining the model status. |
| `modelStage` | 否 | `string` | `MODEL_STAGE_UNSPECIFIED`, `UNSTABLE_EXPERIMENTAL`, `EXPERIMENTAL`, `PREVIEW`, `STABLE`, `LEGACY`, `DEPRECATED`, `RETIRED` | The stage of the underlying model. |
| `retirementTime` | 否 | `string(google-datetime)` | - | The time at which the model will be retired. |

### `MultiSpeakerVoiceConfig`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | The configuration for the multi-speaker setup. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `speakerVoiceConfigs` | 否 | `array<SpeakerVoiceConfig>` | - | Required. All the enabled speaker voices. |

### `Operation`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | This resource represents a long-running operation that is the result of a network API call. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `done` | 否 | `boolean` | - | If the value is false, it means the operation is still in progress. If true, the operation is completed, and either error or response is available. |
| `error` | 否 | `Status` | - | The error result of the operation in case of failure or cancellation. |
| `metadata` | 否 | `object/map<string, any>` | - | Service-specific metadata associated with the operation. It typically contains progress information and common metadata such as create time. Some services might not provide such m… |
| `name` | 否 | `string` | - | The server-assigned name, which is only unique within the same service that originally returns it. If you use the default HTTP mapping, the name should be a resource name ending w… |
| `response` | 否 | `object/map<string, any>` | - | The normal, successful response of the operation. If the original method returns no data on success, such as Delete, the response is google.protobuf.Empty. If the original method … |

### `Part`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A datatype containing media that is part of a multi-part Content message. A Part consists of data which has an associated datatype. A Part can only contain one of the accepted typ… |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `codeExecutionResult` | 否 | `CodeExecutionResult` | - | Result of executing the ExecutableCode. |
| `executableCode` | 否 | `ExecutableCode` | - | Code generated by the model that is meant to be executed. |
| `fileData` | 否 | `FileData` | - | URI based data. |
| `functionCall` | 否 | `FunctionCall` | - | A predicted FunctionCall returned from the model that contains a string representing the FunctionDeclaration.name with the arguments and their values. |
| `functionResponse` | 否 | `FunctionResponse` | - | The result output of a FunctionCall that contains a string representing the FunctionDeclaration.name and a structured JSON object containing any output from the function is used a… |
| `inlineData` | 否 | `Blob` | - | Inline media bytes. |
| `mediaResolution` | 否 | `MediaResolution` | - | Optional. Media resolution for the input media. |
| `partMetadata` | 否 | `object/map<string, any>` | - | Custom metadata associated with the Part. Agents using genai.Part as content representation may need to keep track of the additional information. For example it can be name of a f… |
| `text` | 否 | `string` | - | Inline text. |
| `thought` | 否 | `boolean` | - | Optional. Indicates if the part is thought from the model. |
| `thoughtSignature` | 否 | `string(byte)` | - | Optional. An opaque signature for the thought so it can be reused in subsequent requests. |
| `toolCall` | 否 | `ToolCall` | - | Server-side tool call. This field is populated when the model predicts a tool invocation that should be executed on the server. The client is expected to echo this message back to… |
| `toolResponse` | 否 | `ToolResponse` | - | The output from a server-side ToolCall execution. This field is populated by the client with the results of executing the corresponding ToolCall. |
| `videoMetadata` | 否 | `VideoMetadata` | - | Optional. Video metadata. The metadata should only be specified while the video data is presented in inline_data or file_data. |

### `PlaceAnswerSources`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Collection of sources that provide answers about the features of a given place in Google Maps. Each PlaceAnswerSources message corresponds to a specific place in Google Maps. The … |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `reviewSnippets` | 否 | `array<ReviewSnippet>` | - | Snippets of reviews that are used to generate answers about the features of a given place in Google Maps. |

### `PrebuiltVoiceConfig`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | The configuration for the prebuilt speaker to use. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `voiceName` | 否 | `string` | - | The name of the preset voice to use. |

### `PredictLongRunningRequest`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Request message for [PredictionService.PredictLongRunning]. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `instances` | 否 | `array<any>` | - | Required. The instances that are the input to the prediction call. |
| `parameters` | 否 | `any` | - | Optional. The parameters that govern the prediction call. |

### `PromptFeedback`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A set of the feedback metadata the prompt specified in GenerateContentRequest.content. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `blockReason` | 否 | `string` | `BLOCK_REASON_UNSPECIFIED`, `SAFETY`, `OTHER`, `BLOCKLIST`, `PROHIBITED_CONTENT`, `IMAGE_SAFETY` | Optional. If set, the prompt was blocked and no candidates are returned. Rephrase the prompt. |
| `safetyRatings` | 否 | `array<SafetyRating>` | - | Ratings for safety of the prompt. There is at most one rating per category. |

### `ResponseFormatConfig`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Configuration for the response output format. This is a flat object where each optional sub-field configures a specific output modality. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `audio` | 否 | `AudioResponseFormat` | - | Optional. Audio output format configuration. |
| `image` | 否 | `ImageResponseFormat` | - | Optional. Image output format configuration. |
| `text` | 否 | `TextResponseFormat` | - | Optional. Text output format configuration. |

### `RetrievalConfig`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Retrieval config. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `languageCode` | 否 | `string` | - | Optional. The language code of the user. Language code for content. Use language tags defined by [BCP47](https://www.rfc-editor.org/rfc/bcp/bcp47.txt). |
| `latLng` | 否 | `LatLng` | - | Optional. The location of the user. |

### `RetrievalMetadata`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Metadata related to retrieval in the grounding flow. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `googleSearchDynamicRetrievalScore` | 否 | `number(float)` | - | Optional. Score indicating how likely information from google search could help answer the prompt. The score is in the range [0, 1], where 0 is the least likely and 1 is the most … |

### `RetrievedContext`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Chunk from context retrieved by the file search tool. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `customMetadata` | 否 | `array<GroundingChunkCustomMetadata>` | - | Optional. User-provided metadata about the retrieved context. |
| `fileSearchStore` | 否 | `string` | - | Optional. Name of the FileSearchStore containing the document. Example: fileSearchStores/123 |
| `mediaId` | 否 | `string` | - | Optional. The media blob resource name for multimodal file search results. Format: fileSearchStores/{file_search_store_id}/media/{blob_id} |
| `pageNumber` | 否 | `integer(int32)` | - | Optional. Page number of the retrieved context, if applicable. |
| `text` | 否 | `string` | - | Optional. Text of the chunk. |
| `title` | 否 | `string` | - | Optional. Title of the document. |
| `uri` | 否 | `string` | - | Optional. URI reference of the semantic retrieval document. |

### `ReviewSnippet`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Encapsulates a snippet of a user review that answers a question about the features of a specific place in Google Maps. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `googleMapsUri` | 否 | `string` | - | A link that corresponds to the user review on Google Maps. |
| `reviewId` | 否 | `string` | - | The ID of the review snippet. |
| `title` | 否 | `string` | - | Title of the review. |

### `SafetyRating`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Safety rating for a piece of content. The safety rating contains the category of harm and the harm probability level in that category for a piece of content. Content is classified… |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `blocked` | 否 | `boolean` | - | Was this content blocked because of this rating? |
| `category` | 否 | `string` | `HARM_CATEGORY_UNSPECIFIED`, `HARM_CATEGORY_DEROGATORY`, `HARM_CATEGORY_TOXICITY`, `HARM_CATEGORY_VIOLENCE`, `HARM_CATEGORY_SEXUAL`, `HARM_CATEGORY_MEDICAL`, `HARM_CATEGORY_DANGEROUS`, `HARM_CATEGORY_HARASSMENT`, `HARM_CATEGORY_HATE_SPEECH`, `HARM_CATEGORY_SEXUALLY_EXPLICIT`, `HARM_CATEGORY_DANGEROUS_CONTENT`, `HARM_CATEGORY_CIVIC_INTEGRITY` | Required. The category for this rating. |
| `probability` | 否 | `string` | `HARM_PROBABILITY_UNSPECIFIED`, `NEGLIGIBLE`, `LOW`, `MEDIUM`, `HIGH` | Required. The probability of harm for this content. |

### `SafetySetting`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Safety setting, affecting the safety-blocking behavior. Passing a safety setting for a category changes the allowed probability that content is blocked. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `category` | 否 | `string` | `HARM_CATEGORY_UNSPECIFIED`, `HARM_CATEGORY_DEROGATORY`, `HARM_CATEGORY_TOXICITY`, `HARM_CATEGORY_VIOLENCE`, `HARM_CATEGORY_SEXUAL`, `HARM_CATEGORY_MEDICAL`, `HARM_CATEGORY_DANGEROUS`, `HARM_CATEGORY_HARASSMENT`, `HARM_CATEGORY_HATE_SPEECH`, `HARM_CATEGORY_SEXUALLY_EXPLICIT`, `HARM_CATEGORY_DANGEROUS_CONTENT`, `HARM_CATEGORY_CIVIC_INTEGRITY` | Required. The category for this setting. |
| `threshold` | 否 | `string` | `HARM_BLOCK_THRESHOLD_UNSPECIFIED`, `BLOCK_LOW_AND_ABOVE`, `BLOCK_MEDIUM_AND_ABOVE`, `BLOCK_ONLY_HIGH`, `BLOCK_NONE`, `OFF` | Required. Controls the probability threshold at which harm is blocked. |

### `Schema`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | The Schema object allows the definition of input and output data types. These types can be objects, but also primitives and arrays. Represents a select subset of an [OpenAPI 3.0 s… |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `anyOf` | 否 | `array<Schema>` | - | Optional. The value should be validated against any (one or more) of the subschemas in the list. |
| `default` | 否 | `any` | - | Optional. Default value of the field. Per JSON Schema, this field is intended for documentation generators and doesn't affect validation. Thus it's included here and ignored so th… |
| `description` | 否 | `string` | - | Optional. A brief description of the parameter. This could contain examples of use. Parameter description may be formatted as Markdown. |
| `enum` | 否 | `array<string>` | - | Optional. Possible values of the element of Type.STRING with enum format. For example we can define an Enum Direction as : {type:STRING, format:enum, enum:["EAST", NORTH", "SOUTH"… |
| `example` | 否 | `any` | - | Optional. Example of the object. Will only populated when the object is the root. |
| `format` | 否 | `string` | - | Optional. The format of the data. Any value is allowed, but most do not trigger any special functionality. |
| `items` | 否 | `Schema` | - | Optional. Schema of the elements of Type.ARRAY. |
| `maxItems` | 否 | `string(int64)` | - | Optional. Maximum number of the elements for Type.ARRAY. |
| `maxLength` | 否 | `string(int64)` | - | Optional. Maximum length of the Type.STRING |
| `maxProperties` | 否 | `string(int64)` | - | Optional. Maximum number of the properties for Type.OBJECT. |
| `maximum` | 否 | `number(double)` | - | Optional. Maximum value of the Type.INTEGER and Type.NUMBER |
| `minItems` | 否 | `string(int64)` | - | Optional. Minimum number of the elements for Type.ARRAY. |
| `minLength` | 否 | `string(int64)` | - | Optional. SCHEMA FIELDS FOR TYPE STRING Minimum length of the Type.STRING |
| `minProperties` | 否 | `string(int64)` | - | Optional. Minimum number of the properties for Type.OBJECT. |
| `minimum` | 否 | `number(double)` | - | Optional. SCHEMA FIELDS FOR TYPE INTEGER and NUMBER Minimum value of the Type.INTEGER and Type.NUMBER |
| `nullable` | 否 | `boolean` | - | Optional. Indicates if the value may be null. |
| `pattern` | 否 | `string` | - | Optional. Pattern of the Type.STRING to restrict a string to a regular expression. |
| `properties` | 否 | `object/map<string, Schema>` | - | Optional. Properties of Type.OBJECT. |
| `propertyOrdering` | 否 | `array<string>` | - | Optional. The order of the properties. Not a standard field in open api spec. Used to determine the order of the properties in the response. |
| `required` | 否 | `array<string>` | - | Optional. Required properties of Type.OBJECT. |
| `title` | 否 | `string` | - | Optional. The title of the schema. |
| `type` | 否 | `string` | `TYPE_UNSPECIFIED`, `STRING`, `NUMBER`, `INTEGER`, `BOOLEAN`, `ARRAY`, `OBJECT`, `NULL` | Required. Data type. |

### `SearchEntryPoint`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Google search entry point. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `renderedContent` | 否 | `string` | - | Optional. Web content snippet that can be embedded in a web page or an app webview. |
| `sdkBlob` | 否 | `string(byte)` | - | Optional. Base64 encoded JSON representing array of tuple. |

### `SearchTypes`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Different types of search that can be enabled on the GoogleSearch tool. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `imageSearch` | 否 | `ImageSearch` | - | Optional. Enables image search. Image bytes are returned. |
| `webSearch` | 否 | `WebSearch` | - | Optional. Enables web search. Only text results are returned. |

### `SemanticRetrieverChunk`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Identifier for a Chunk retrieved via Semantic Retriever specified in the GenerateAnswerRequest using SemanticRetrieverConfig. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `chunk` | 否 | `string` | - | Output only. Name of the Chunk containing the attributed text. Example: corpora/123/documents/abc/chunks/xyz |
| `source` | 否 | `string` | - | Output only. Name of the source matching the request's SemanticRetrieverConfig.source. Example: corpora/123 or corpora/123/documents/abc |

### `SpeakerVoiceConfig`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | The configuration for a single speaker in a multi speaker setup. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `speaker` | 否 | `string` | - | Required. The name of the speaker to use. Should be the same as in the prompt. |
| `voiceConfig` | 否 | `VoiceConfig` | - | Required. The configuration for the voice to use. |

### `SpeechConfig`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Config for speech generation and transcription. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `languageCode` | 否 | `string` | - | Optional. The IETF [BCP-47](https://www.rfc-editor.org/rfc/bcp/bcp47.txt) language code that the user configured the app to use. Used for speech recognition and synthesis. Valid v… |
| `multiSpeakerVoiceConfig` | 否 | `MultiSpeakerVoiceConfig` | - | Optional. The configuration for the multi-speaker setup. It is mutually exclusive with the voice_config field. |
| `voiceConfig` | 否 | `VoiceConfig` | - | The configuration in case of single-voice output. |

### `Status`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | The Status type defines a logical error model that is suitable for different programming environments, including REST APIs and RPC APIs. It is used by [gRPC](https://github.com/gr… |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `code` | 否 | `integer(int32)` | - | The status code, which should be an enum value of google.rpc.Code. |
| `details` | 否 | `array<object/map<string, any>>` | - | A list of messages that carry the error details. There is a common set of message types for APIs to use. |
| `message` | 否 | `string` | - | A developer-facing error message, which should be in English. Any user-facing error message should be localized and sent in the google.rpc.Status.details field, or localized by th… |

### `StreamableHttpTransport`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A transport that can stream HTTP requests and responses. Next ID: 6 |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `headers` | 否 | `object/map<string, string>` | - | Optional: Fields for authentication headers, timeouts, etc., if needed. |
| `sseReadTimeout` | 否 | `string(google-duration)` | - | Timeout for SSE read operations. |
| `terminateOnClose` | 否 | `boolean` | - | Whether to close the client session when the transport closes. |
| `timeout` | 否 | `string(google-duration)` | - | HTTP timeout for regular operations. |
| `url` | 否 | `string` | - | The full URL for the MCPServer endpoint. Example: "https://api.example.com/mcp" |

### `TextResponseFormat`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Configuration for text output format. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `mimeType` | 否 | `string` | `MIME_TYPE_UNSPECIFIED`, `APPLICATION_JSON`, `TEXT_PLAIN` | Optional. The MIME type of the text output. |
| `schema` | 否 | `any` | - | Optional. The JSON schema that the output should conform to. Only applicable when mime_type is APPLICATION_JSON. |

### `ThinkingConfig`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Config for thinking features. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `includeThoughts` | 否 | `boolean` | - | Indicates whether to include thoughts in the response. If true, thoughts are returned only when available. |
| `thinkingBudget` | 否 | `integer(int32)` | - | The number of thoughts tokens that the model should generate. |
| `thinkingLevel` | 否 | `string` | `THINKING_LEVEL_UNSPECIFIED`, `MINIMAL`, `LOW`, `MEDIUM`, `HIGH` | Optional. Controls the maximum depth of the model's internal reasoning process before it produces a response. The default value is model-dependent. Refer to the [Thinking levels g… |

### `Tool`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Tool details that the model may use to generate response. A Tool is a piece of code that enables the system to interact with external systems to perform an action, or set of actio… |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `codeExecution` | 否 | `CodeExecution` | - | Optional. Enables the model to execute code as part of generation. |
| `computerUse` | 否 | `ComputerUse` | - | Optional. Tool to support the model interacting directly with the computer. If enabled, it automatically populates computer-use specific Function Declarations. |
| `fileSearch` | 否 | `FileSearch` | - | Optional. FileSearch tool type. Tool to retrieve knowledge from Semantic Retrieval corpora. |
| `functionDeclarations` | 否 | `array<FunctionDeclaration>` | - | Optional. A list of FunctionDeclarations available to the model that can be used for function calling. The model or system does not execute the function. Instead the defined funct… |
| `googleMaps` | 否 | `GoogleMaps` | - | Optional. Tool that allows grounding the model's response with geospatial context related to the user's query. |
| `googleSearch` | 否 | `GoogleSearch` | - | Optional. GoogleSearch tool type. Tool to support Google Search in Model. Powered by Google. |
| `googleSearchRetrieval` | 否 | `GoogleSearchRetrieval` | - | Optional. Retrieval tool that is powered by Google search. |
| `mcpServers` | 否 | `array<McpServer>` | - | Optional. MCP Servers to connect to. |
| `urlContext` | 否 | `UrlContext` | - | Optional. Tool to support URL context retrieval. |

### `ToolCall`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | A predicted server-side ToolCall returned from the model. This message contains information about a tool that the model wants to invoke. The client is NOT expected to execute this… |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `args` | 否 | `object/map<string, any>` | - | Optional. The tool call arguments. Example: {"arg1" : "value1", "arg2" : "value2" , ...} |
| `id` | 否 | `string` | - | Optional. Unique identifier of the tool call. The server returns the tool response with the matching id. |
| `toolType` | 否 | `string` | `TOOL_TYPE_UNSPECIFIED`, `GOOGLE_SEARCH_WEB`, `GOOGLE_SEARCH_IMAGE`, `URL_CONTEXT`, `GOOGLE_MAPS`, `FILE_SEARCH` | Required. The type of tool that was called. |

### `ToolConfig`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | The Tool configuration containing parameters for specifying Tool use in the request. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `functionCallingConfig` | 否 | `FunctionCallingConfig` | - | Optional. Function calling config. |
| `includeServerSideToolInvocations` | 否 | `boolean` | - | Optional. If true, the API response will include the server-side tool calls and responses within the Content message. This allows clients to observe the server's tool interactions. |
| `retrievalConfig` | 否 | `RetrievalConfig` | - | Optional. Retrieval config. |

### `ToolResponse`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | The output from a server-side ToolCall execution. This message contains the results of a tool invocation that was initiated by a ToolCall from the model. The client should pass th… |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `id` | 否 | `string` | - | Optional. The identifier of the tool call this response is for. |
| `response` | 否 | `object/map<string, any>` | - | Optional. The tool response. |
| `toolType` | 否 | `string` | `TOOL_TYPE_UNSPECIFIED`, `GOOGLE_SEARCH_WEB`, `GOOGLE_SEARCH_IMAGE`, `URL_CONTEXT`, `GOOGLE_MAPS`, `FILE_SEARCH` | Required. The type of tool that was called, matching the tool_type in the corresponding ToolCall. |

### `TopCandidates`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Candidates with top log probabilities at each decoding step. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `candidates` | 否 | `array<LogprobsResultCandidate>` | - | Sorted by log probability in descending order. |

### `UrlContext`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Tool to support URL context retrieval. |

### `UrlContextMetadata`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Metadata related to url context retrieval tool. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `urlMetadata` | 否 | `array<UrlMetadata>` | - | List of url context. |

### `UrlMetadata`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Context of the a single url retrieval. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `retrievedUrl` | 否 | `string` | - | Retrieved url by the tool. |
| `urlRetrievalStatus` | 否 | `string` | `URL_RETRIEVAL_STATUS_UNSPECIFIED`, `URL_RETRIEVAL_STATUS_SUCCESS`, `URL_RETRIEVAL_STATUS_ERROR`, `URL_RETRIEVAL_STATUS_PAYWALL`, `URL_RETRIEVAL_STATUS_UNSAFE` | Status of the url retrieval. |

### `UsageMetadata`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Metadata on the generation request's token usage. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `cacheTokensDetails` | 否 | `array<ModalityTokenCount>` | - | Output only. List of modalities of the cached content in the request input. |
| `cachedContentTokenCount` | 否 | `integer(int32)` | - | Number of tokens in the cached part of the prompt (the cached content) |
| `candidatesTokenCount` | 否 | `integer(int32)` | - | Total number of tokens across all the generated response candidates. |
| `candidatesTokensDetails` | 否 | `array<ModalityTokenCount>` | - | Output only. List of modalities that were returned in the response. |
| `promptTokenCount` | 否 | `integer(int32)` | - | Number of tokens in the prompt. When cached_content is set, this is still the total effective prompt size meaning this includes the number of tokens in the cached content. |
| `promptTokensDetails` | 否 | `array<ModalityTokenCount>` | - | Output only. List of modalities that were processed in the request input. |
| `serviceTier` | 否 | `string` | `unspecified`, `standard`, `flex`, `priority` | Output only. Service tier of the request. |
| `thoughtsTokenCount` | 否 | `integer(int32)` | - | Output only. Number of tokens of thoughts for thinking models. |
| `toolUsePromptTokenCount` | 否 | `integer(int32)` | - | Output only. Number of tokens present in tool-use prompt(s). |
| `toolUsePromptTokensDetails` | 否 | `array<ModalityTokenCount>` | - | Output only. List of modalities that were processed for tool-use request inputs. |
| `totalTokenCount` | 否 | `integer(int32)` | - | Total token count for the generation request (prompt + thoughts + response candidates). |

### `VideoFileMetadata`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Metadata for a video File. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `videoDuration` | 否 | `string(google-duration)` | - | Duration of the video. |

### `VideoMetadata`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Deprecated: Use GenerateContentRequest.processing_options instead. Metadata describes the input video content. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `endOffset` | 否 | `string(google-duration)` | - | Optional. The end offset of the video. |
| `fps` | 否 | `number(double)` | - | Optional. The frame rate of the video sent to the model. If not specified, the default value will be 1.0. The fps range is (0.0, 24.0]. |
| `startOffset` | 否 | `string(google-duration)` | - | Optional. The start offset of the video. |

### `VoiceConfig`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | The configuration for the voice to use. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `prebuiltVoiceConfig` | 否 | `PrebuiltVoiceConfig` | - | The configuration for the prebuilt voice to use. |

### `Web`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Chunk from the web. |

| 字段 | 必填 | 类型 | 枚举/常量 | 说明 |
| --- | --- | --- | --- | --- |
| `title` | 否 | `string` | - | Output only. Title of the chunk. |
| `uri` | 否 | `string` | - | Output only. URI reference of the chunk. |

### `WebSearch`

| 项 | 值 |
| --- | --- |
| 类型 | `object` |
| 说明 | Standard web search for grounding and related configurations. |
