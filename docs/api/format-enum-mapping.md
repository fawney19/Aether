# Format Enum Mapping

Status values used below:

- `native`: same semantic value exists in the target format.
- `mapped`: explicit provider-specific mapping is required.
- `blocked`: no lossless target value; conversion must fail closed.
- `preserve-same-format`: unknown/raw values are preserved only when source and target format are the same.

## OpenAI Reasoning Effort

Provider-specific types: `OpenAiChatReasoningEffort` for Chat `reasoning_effort`, and `OpenAiResponsesReasoningEffort` for Responses `reasoning.effort`. They are intentionally separate even when their current value sets overlap; a value accepted by one field is not treated as valid for the other unless that field's own enum accepts it.

| Source field | Source value | Target field | Target value | Status |
| --- | --- | --- | --- | --- |
| Chat `reasoning_effort` | `none` | Responses `reasoning.effort` | `none` | native |
| Chat `reasoning_effort` | `minimal` | Responses `reasoning.effort` | `minimal` | native when the target model supports it; blocked for GPT-5.6 |
| Chat `reasoning_effort` | `low` | Responses `reasoning.effort` | `low` | native |
| Chat `reasoning_effort` | `medium` | Responses `reasoning.effort` | `medium` | native |
| Chat `reasoning_effort` | `high` | Responses `reasoning.effort` | `high` | native |
| Chat `reasoning_effort` | `xhigh` | Responses `reasoning.effort` | `xhigh` | native |
| Chat `reasoning_effort` | `max` | Responses `reasoning.effort` | `max` | native for GPT-5.6; blocked for models that do not publish `max` |
| Responses `reasoning.effort` | `none` | Chat `reasoning_effort` | `none` | native |
| Responses `reasoning.effort` | `minimal` | Chat `reasoning_effort` | `minimal` | native when the target model supports it; blocked for GPT-5.6 |
| Responses `reasoning.effort` | `low` | Chat `reasoning_effort` | `low` | native |
| Responses `reasoning.effort` | `medium` | Chat `reasoning_effort` | `medium` | native |
| Responses `reasoning.effort` | `high` | Chat `reasoning_effort` | `high` | native |
| Responses `reasoning.effort` | `xhigh` | Chat `reasoning_effort` | `xhigh` | native |
| Responses `reasoning.effort` | `max` | Chat `reasoning_effort` | `max` | native for GPT-5.6; blocked for models that do not publish `max` |
| Responses `reasoning.summary` | any | Chat | none | blocked |
| Responses `reasoning.budget_tokens` | any | Chat | none | blocked |

Internal model directive values:

| Internal value | OpenAI Chat | OpenAI Responses | Claude output effort | Gemini thinking level | Notes |
| --- | --- | --- | --- | --- | --- |
| `none` | `none` | `none` | `low` | `low` | Budget maps to `0`. |
| `minimal` | `minimal` | `minimal` | `low` | `low` | Budget maps to `512`. |
| `low` | `low` | `low` | `low` | `low` | Budget maps to `1280`. |
| `medium` | `medium` | `medium` | `medium` | `medium` | Budget maps to `2048`. |
| `high` | `high` | `high` | `high` | `high` | Budget maps to `4096`. |
| `xhigh` | `xhigh` | `xhigh` | `xhigh` | `high` | Budget maps to `8192`. |
| `max` | `max` for GPT-5.6 | `max` for GPT-5.6 | `max` | `high` | OpenAI emission is capability-gated by the resolved model. |

GPT-5.6 (`gpt-5.6`, `gpt-5.6-sol`, `gpt-5.6-terra`, and `gpt-5.6-luna`) publishes `none`, `low`, `medium`, `high`, `xhigh`, and `max`, and does not support `minimal`. Additional non-empty effort values advertised by a model are preserved verbatim across OpenAI Chat and Responses conversion. `ultra` is a Codex client preset that resolves to `max` before transmission and is not an OpenAI wire effort. Known effort capabilities are validated against the resolved provider model, so aliases mapped to GPT-5.6 receive the GPT-5.6 contract while concrete model families keep their published constraints.

## Tool Choice

| Canonical | OpenAI Chat | OpenAI Responses | Claude Messages | Gemini GenerateContent |
| --- | --- | --- | --- | --- |
| auto | `"auto"` | `"auto"` | `{"type":"auto"}` | unset / function calling config auto |
| none | `"none"` | `"none"` | `{"type":"none"}` | mode none |
| required | `"required"` | `"required"` | `{"type":"any"}` | mode any |
| named function | `{"type":"function","function":{"name":...}}` | `{"type":"function","name":...}` | `{"type":"tool","name":...}` | allowed function name |

Implemented guardrails:

- Claude `output_config.effort=max` maps to OpenAI `xhigh`; unknown Claude effort enums are blocked cross-format.
- Claude `tool_choice.disable_parallel_tool_use` maps inversely to OpenAI `parallel_tool_calls`.
- Gemini `allowedFunctionNames` maps to canonical named tool choice and emits back to `allowedFunctionNames`.
- Gemini `thinkingLevel` maps `low|medium|high` to OpenAI reasoning effort `low|medium|high`; unknown values are blocked cross-format.
- Responses `custom`, `web_search*`, and other built-in tools are blocked when converting to Chat unless a target raw passthrough is explicitly added.

## Tool Definition Kind

| Source | Target | Mapping |
| --- | --- | --- |
| OpenAI Chat `tools[].function.name` | Responses `tools[].name` | mapped |
| OpenAI Chat `tools[].function.parameters` | Responses `tools[].parameters` | mapped |
| OpenAI Chat `tools[].function.strict` | Responses `tools[].strict` | mapped, implemented |
| Responses `tools[].strict` | Chat `tools[].function.strict` | mapped, implemented |
| OpenAI Chat assistant `tool_calls[].id` | Responses `function_call.call_id` | mapped, implemented |
| OpenAI Chat tool `tool_call_id` | Responses `function_call_output.call_id` | mapped, implemented |
| Responses `function_call.call_id` | Chat `tool_calls[].id` | mapped, implemented |
| Responses `function_call_output.call_id` | Chat tool `tool_call_id` | mapped, implemented |
| Gemini `functionCall.id` | OpenAI/Claude canonical tool use id | mapped, implemented |
| Gemini `functionResponse.id` | OpenAI `tool_call_id` / Responses `call_id` / Claude `tool_use_id` | mapped, implemented |
| Claude `tools[].input_schema` | OpenAI `parameters` | mapped; raw same-format schema preserved |
| Gemini `functionDeclarations[].parameters` | OpenAI `parameters` | mapped; raw same-format schema preserved |

## Roles

| Canonical role | OpenAI Chat | OpenAI Responses | Claude Messages | Gemini |
| --- | --- | --- | --- | --- |
| system | `system` | `instructions` or system input item | top-level `system` | `systemInstruction` |
| developer | `developer` | `instructions` or developer input item | extension-preserved | systemInstruction extension |
| user | `user` | `message.role=user` | `user` | `user` |
| assistant | `assistant` | `message.role=assistant` / output item | `assistant` | `model` |
| tool | `tool` | `function_call_output` | `tool_result` inside user message | `functionResponse` |

Known lossy risks:

- Multiple system/developer instruction ordering needs full golden fixtures.
- Provider-specific role extensions must be preserved in same-format roundtrip and blocked cross-format if no target equivalent exists.

## Finish Reasons

| Canonical | OpenAI Chat | OpenAI Responses | Claude | Gemini |
| --- | --- | --- | --- | --- |
| stop | `stop` | completed output | `end_turn` | `STOP` |
| length | `length` | `status=incomplete`, `incomplete_details.reason=max_output_tokens` | `max_tokens` | `MAX_TOKENS` |
| tool calls | `tool_calls` | output contains `function_call` | `tool_use` | `functionCall` part, usually with `STOP` |
| content filter/safety | `content_filter` | `status=incomplete`, `incomplete_details.reason=content_filter` | `content_filtered` or refusal-compatible stops | `SAFETY`, `RECITATION`, `LANGUAGE`, `BLOCKLIST`, `PROHIBITED_CONTENT`, `SPII`, image safety/recitation stops |
| unknown | preserve-same-format | preserve-same-format | preserve-same-format | preserve-same-format |

Implemented response guardrails:

- Cross-format sync response conversion validates source finish/status enums before emitting the target response.
- Same-format canonical response roundtrip preserves raw OpenAI Chat `finish_reason`, OpenAI Responses `status`, Claude `stop_reason`, and Gemini `finishReason` values through provider extension metadata.
- OpenAI Chat unknown `choices[].finish_reason` fails with `InvalidEnumValue`.
- OpenAI Responses non-terminal `status` values (`queued`, `in_progress`, `cancelled`) are valid provider states but are blocked for sync response conversion because target sync formats cannot represent them losslessly.
- Runtime sync finalize does not fall back to legacy conversion when registry response conversion reports strict errors such as invalid enums, unsupported fields, lossy blocks, or invalid target fields.
- Stream terminal reasons now follow the same strict policy: unknown OpenAI / Claude raw finish reasons and Gemini known-but-unmappable values such as `OTHER` surface as `unsupported_finish_reason`, while OpenAI Responses `length` and `content_filter` stream finals emit `response.incomplete`.
- Gemini known but unmappable finish reasons such as `OTHER`, `MALFORMED_FUNCTION_CALL`, `UNEXPECTED_TOOL_CALL`, `MISSING_THOUGHT_SIGNATURE`, and `MALFORMED_RESPONSE` are blocked with `LossyConversionBlocked`; unknown future Gemini values fail with `InvalidEnumValue`.
- Stream finish reason guardrails are covered with provider-specific fixtures for usage, tool calls, reasoning signatures, media, and unknown payloads. Unknown provider stream events are not mapped as finish reasons; cross-format runtime conversion emits a target-format `unsupported_stream_event` error and terminates.

## Embedding Task Types

Provider-specific type: `GeminiEmbeddingTaskType`.

Gemini embedding task values are stored in canonical `embedding.task` only after
source parsing. They are emitted to Gemini as `taskType` and validated before
cross-format conversion to a Gemini target.

| Canonical task input | Gemini `taskType` output | Status |
| --- | --- | --- |
| `QUERY` | `RETRIEVAL_QUERY` | mapped alias |
| `RETRIEVAL_QUERY` | `RETRIEVAL_QUERY` | native |
| `DOCUMENT` | `RETRIEVAL_DOCUMENT` | mapped alias |
| `RETRIEVAL_DOCUMENT` | `RETRIEVAL_DOCUMENT` | native |
| `TEXT_MATCHING` | `SEMANTIC_SIMILARITY` | mapped alias |
| `SEMANTIC_SIMILARITY` | `SEMANTIC_SIMILARITY` | native |
| `CLASSIFICATION` | `CLASSIFICATION` | native |
| `CLUSTERING` | `CLUSTERING` | native |
| `QUESTION_ANSWERING` | `QUESTION_ANSWERING` | native |
| `FACT_VERIFICATION` | `FACT_VERIFICATION` | native |
| `CODE_RETRIEVAL_QUERY` | `CODE_RETRIEVAL_QUERY` | native |
| `TASK_TYPE_UNSPECIFIED` | `TASK_TYPE_UNSPECIFIED` | native |
| unknown value | none | blocked with `InvalidEnumValue` when targeting Gemini |

Cross-provider rules:

- Gemini `taskType` to OpenAI Embedding is blocked because OpenAI has no equivalent task field.
- Gemini `taskType` to Doubao/Aliyun is blocked for the same reason.
- Jina `task` may carry through canonical and emit as Jina `task`; when targeting Gemini it must match the valid Gemini task set above.
