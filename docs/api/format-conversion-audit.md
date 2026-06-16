# Format Conversion Audit

Last audited: 2026-06-03

This audit tracks `source format -> Canonical -> target format` behavior. It is intentionally stricter than historical best-effort conversion.

Statuses:

- `native`: emitted as a target-native field without semantic change.
- `mapped`: converted through canonical/provider-specific mapping.
- `extension-preserved`: preserved in same-format canonical roundtrip or target-approved extension namespace.
- `unaudited`: rejected because the source field is not in the audited provider schema inventory for cross-format conversion.
- `unsupported`: rejected because the request/field shape is outside the supported conversion surface, independent of schema drift.
- `lossy-blocked`: conversion fails closed.
- `invalid-enum`: conversion fails closed because a provider enum value is not valid for the target mapping.

Full schema field coverage is tracked in `docs/api/format-field-coverage-matrix.md`. That matrix is generated from the schema inventory in `docs/api/provider-interface-definitions.md` by `python3 docs/api/generate_format_field_coverage.py` and gives every documented OpenAI, Claude, and Gemini schema field a handling status. “Handled” means mapped, same-format/native preserved, extension-preserved, blocked with a structured error, or explicitly marked outside the canonical conversion surface.

Provider schema refresh is not a runtime dependency. Same-format runtime paths do not use this matrix; they bypass canonical conversion. Same-format canonical roundtrip must preserve unrecognized provider fields through provider extension namespaces. Cross-format conversion is capability-based: only explicitly mapped fields are emitted, and newly discovered or unknown provider fields fail closed with `UnauditedField` until a lossless mapping is audited.

## Implemented Boundary Changes

| Area | Current behavior |
| --- | --- |
| Pure conversion API | `convert_request_pure` and response equivalents do not apply model override or stream policy. |
| Legacy conversion API | `convert_request` / `convert_response` are retained for migration and may still use legacy context behavior. |
| Same-format provider path | Bypasses canonical conversion and copies the parsed JSON object before transport edits. |
| Cross-format same-format-provider path | Uses `convert_request_pure`, then applies model/body/stream edits in transport. |
| Conversion errors | Added `UnauditedField`, `UnsupportedField`, `InvalidEnumValue`, `LossyConversionBlocked`, and `InvalidTargetField`. |
| Reporting | Added `ConversionReport` with field statuses. Runtime reports remain conversion-operation oriented; exhaustive nested schema coverage is enforced by `format-field-coverage-matrix.md`. |
| Source schema coverage | Cross-format request conversion rejects unknown source root fields before emit. Every documented schema field is covered by the field coverage matrix. |
| Schema drift handling | Official schema changes are detected by regenerating the inventory/matrix. Runtime same-format remains passthrough; cross-format unknowns return `UnauditedField` until deliberately mapped. |
| Tool schema roundtrip | Claude `input_schema` and Gemini `functionDeclarations.parameters` preserve raw same-format schema through provider-specific extensions. |
| Tool result ids | Chat `tool_call_id`, Responses `call_id`, Claude `tool_use_id`, and Gemini `functionResponse.id` are mapped through canonical tool IDs. |

## OpenAI Chat -> OpenAI Responses

| Chat field | Canonical handling | Responses output | Status |
| --- | --- | --- | --- |
| `model` | request identity | `model` | native |
| `messages` | canonical messages/instructions | `input`, `instructions` | mapped |
| `max_tokens` | generation max tokens | `max_output_tokens` | mapped |
| `max_completion_tokens` | generation max tokens | `max_output_tokens` | mapped |
| `temperature` | generation | `temperature` | native |
| `top_p` | generation | `top_p` | native |
| `top_logprobs` | generation | `top_logprobs` | native |
| `n` | generation but no Responses equivalent | none | lossy-blocked |
| `stop` | generation but no Responses equivalent | none | lossy-blocked |
| `presence_penalty` | generation but no Responses equivalent | none | lossy-blocked |
| `frequency_penalty` | generation but no Responses equivalent | none | lossy-blocked |
| `seed` | generation but no Responses equivalent | none | lossy-blocked |
| `logprobs` | generation but no Responses equivalent | none | lossy-blocked |
| `stream` | OpenAI extension | `stream` | mapped if explicit |
| `stream_options` | Chat-specific extension | none | lossy-blocked |
| `tools[].function.name` | canonical tool | `tools[].name` | mapped |
| `tools[].function.description` | canonical tool | `tools[].description` | mapped |
| `tools[].function.parameters` | canonical tool | `tools[].parameters` | mapped |
| `tools[].function.strict` | canonical tool strict | `tools[].strict` | mapped, implemented |
| assistant `tool_calls[].id` | canonical tool use id | `function_call.call_id` | mapped, implemented |
| tool message `tool_call_id` | canonical tool result id | `function_call_output.call_id` | mapped, implemented |
| `tool_choice` | canonical tool choice | `tool_choice` | mapped |
| `parallel_tool_calls` | canonical bool | `parallel_tool_calls` | native |
| `metadata` | canonical metadata | `metadata` | native |
| `response_format` | canonical response format | `text.format` | mapped |
| `reasoning_effort` | OpenAI enum | `reasoning.effort` | mapped; invalid enum blocked |
| `verbosity` | OpenAI extension | `text.verbosity` | mapped |
| `store` | OpenAI extension | `store` | extension-preserved |
| `service_tier` | OpenAI extension | `service_tier` | extension-preserved |
| `safety_identifier` | OpenAI extension | `safety_identifier` | extension-preserved |
| `prompt_cache_key` | OpenAI extension | `prompt_cache_key` | extension-preserved |
| `user` | legacy Chat user field | none | lossy-blocked |
| unknown top-level fields | source schema guard | none | unaudited |

## OpenAI Responses -> OpenAI Chat

| Responses field | Canonical handling | Chat output | Status |
| --- | --- | --- | --- |
| `model` | request identity | `model` | native |
| `input` | canonical messages/content/tool I/O | `messages` | mapped |
| `instructions` | canonical instruction/system | `messages` system/developer | mapped |
| `max_output_tokens` | generation max tokens | `max_completion_tokens` | mapped |
| `temperature` | generation | `temperature` | native |
| `top_p` | generation | `top_p` | native |
| `top_logprobs` | generation | `top_logprobs` | native |
| `metadata` | canonical metadata | `metadata` | native |
| `parallel_tool_calls` | canonical bool | `parallel_tool_calls` | native |
| `text.format` | canonical response format | `response_format` | mapped |
| `text.verbosity` | Responses extension | `verbosity` | mapped |
| `tools[].type=function` | canonical tool | `tools[].type=function` | mapped |
| `tools[].name` | canonical tool | `tools[].function.name` | mapped |
| `tools[].parameters` | canonical tool | `tools[].function.parameters` | mapped |
| `tools[].strict` | canonical tool strict | `tools[].function.strict` | mapped, implemented |
| `function_call.call_id` | canonical tool use id | `tool_calls[].id` | mapped, implemented |
| `function_call_output.call_id` | canonical tool result id | tool message `tool_call_id` | mapped, implemented |
| `tools[].type=custom` | raw Responses tool | none | lossy-blocked to Chat |
| `tools[].type=web_search*` | raw Responses tool | none | lossy-blocked to Chat |
| `tool_choice` | canonical tool choice | `tool_choice` | mapped |
| `reasoning.effort` | OpenAI enum | `reasoning_effort` | mapped; invalid enum blocked |
| `reasoning.summary` | Responses-only | none | lossy-blocked |
| `reasoning.budget_tokens` | Responses-only | none | lossy-blocked |
| `stream` | Responses request transport policy | none | lossy-blocked; target stream policy is transport-owned |
| `include` | Responses-only | none | lossy-blocked; legacy emitter no longer leaks |
| `previous_response_id` | Responses-only | none | lossy-blocked; legacy emitter no longer leaks |
| `truncation` | Responses-only | none | lossy-blocked |
| `prompt` | Responses-only | none | lossy-blocked |
| `conversation` | Responses-only | none | lossy-blocked |
| `background` | Responses-only | none | lossy-blocked |
| `max_tool_calls` | Responses-only | none | lossy-blocked |
| unknown top-level fields | source schema guard | none | unaudited |

## Claude Messages <-> OpenAI Chat / Responses

Claude to OpenAI Chat, Claude to OpenAI Responses, and the reverse directions are included in the field coverage matrix. Runtime strict guards cover request root fields, provider extension namespaces, thinking/cache/tool-result hazards, and target generation-field gaps. Fields without a lossless target equivalent fail closed instead of being dropped.

High-risk fields:

| Claude field | OpenAI target risk | Required status |
| --- | --- | --- |
| `system` with cache blocks | Chat/Responses system instructions | same-format preserved; cross-format `cache_control` loss is blocked |
| `thinking` | OpenAI reasoning | Claude request-level thinking config maps to OpenAI reasoning; message-level thinking blocks are blocked for Responses |
| `cache_control` | OpenAI content/tool extensions | same-format preserved; cross-format blocked when no target equivalent exists |
| `tools[].input_schema` | OpenAI tool parameters | mapped; raw same-format schema preservation implemented |
| `tool_choice.disable_parallel_tool_use` | OpenAI `parallel_tool_calls` | mapped, implemented |
| `tool_result` multi-block content | OpenAI tool output/content | same-format preserved; cross-format to Chat/Responses is lossy-blocked |
| `metadata` | OpenAI metadata | mapped when the target has metadata |
| `container`, `inference_geo`, `service_tier` | OpenAI target has no audited equivalent | lossy-blocked unless a target-approved mapping is added |

## Gemini GenerateContent <-> OpenAI Chat / Responses / Claude

Gemini to OpenAI Chat, Gemini to OpenAI Responses, Gemini to Claude, and reverse generation paths are included in the field coverage matrix. Gemini-only request fields are preserved same-format and blocked cross-format unless the target mapping is explicitly audited.

High-risk fields:

| Gemini field | Target risk | Required status |
| --- | --- | --- |
| `contents[].parts[].thoughtSignature` | OpenAI/Claude thinking | Chat/Claude preserve; Responses cross-format is lossy-blocked |
| `tools[].functionDeclarations` | OpenAI/Claude tool schema | mapped; raw same-format `parameters` preservation implemented |
| `toolConfig.functionCallingConfig.allowedFunctionNames` | OpenAI/Claude tool choice | single-name mapping implemented; multi-name input is lossy-blocked |
| `toolConfig.functionCallingConfig.mode` | OpenAI/Claude tool choice enum | valid enum required; invalid values fail with `InvalidEnumValue` |
| `generationConfig.thinkingConfig.thinkingLevel` | OpenAI/Claude reasoning effort | low/medium/high mapping implemented; invalid values fail closed |
| `safetySettings` | OpenAI/Claude no direct equivalent | lossy-blocked |
| `cachedContent` | OpenAI/Claude no direct equivalent | lossy-blocked |
| `codeExecution` | OpenAI/Claude tool/builtin mismatch | lossy-blocked |
| `generationConfig.responseModalities` | OpenAI/Claude modality mismatch | lossy-blocked |
| `functionResponse.id` | tool result id | conversion preserves id; Gemini upstream cleanup is transport-layer edit only |

## Embedding And Rerank

Embedding and rerank request parse/emit capability and strict target guards are implemented. Provider schema fields outside these canonical conversion surfaces are marked `not-in-conversion-surface` in the field coverage matrix instead of being left implicit.

Embedding source capability:

| Source format | Parsed request shape | Canonical fields | Status |
| --- | --- | --- | --- |
| OpenAI Embedding | `model`, `input`, `encoding_format`, `dimensions`, `user`, `parameters`, `task` | OpenAI-like embedding | mapped |
| Jina Embedding | OpenAI-like plus provider extension namespace | OpenAI-like embedding | mapped |
| Doubao Embedding | OpenAI-like `model` + text `input` | OpenAI-like embedding | mapped |
| Gemini Embedding | single `content.parts[].text` or batch `requests[]` | text input, `dimensions`, `task` | mapped |
| Aliyun Multimodal Embedding | `input.contents[]`, `parameters.dimension` | text/multimodal input, `dimensions`, `parameters` | mapped |

Embedding target guards:

| Target format | Accepted canonical fields | Blocked fields/cases | Status |
| --- | --- | --- | --- |
| OpenAI Embedding | text or token input, `encoding_format`, `dimensions`, `user` | multimodal input, `task`, generic `parameters` | lossy-blocked |
| Jina Embedding | text input, `dimensions`, `task`, `parameters` | token/multimodal input, `encoding_format`, `user` | lossy-blocked |
| Gemini Embedding | text input, `dimensions`, valid `taskType` | token/multimodal input, `encoding_format`, `user`, generic `parameters`, invalid `taskType` | lossy-blocked / invalid-enum |
| Doubao Embedding | text input, `dimensions` | token/multimodal input, `encoding_format`, `user`, `task`, generic `parameters` | lossy-blocked |
| Aliyun Multimodal Embedding | text or multimodal input, `dimensions`, `parameters` | token input, `encoding_format`, `user`, `task` | lossy-blocked |

Cross-format embedding invariants:

- Embedding formats can only convert to embedding formats.
- Unknown provider-specific embedding extension namespaces are blocked cross-format unless the namespace matches the target.
- Aliyun `parameters.dimension` maps to canonical `dimensions` and is not treated as generic `parameters`.
- Gemini batch embedding parse requires every batch item to share the same model, dimensions, and task.

Rerank first pass:

| Area | Current behavior | Status |
| --- | --- | --- |
| Source formats | OpenAI Rerank and Jina Rerank parse OpenAI-like `model`, `query`, `documents`, `top_n`, `return_documents` | mapped |
| Target formats | OpenAI Rerank and Jina Rerank emit OpenAI-like rerank bodies | mapped |
| Boundary | Rerank formats can only convert to rerank formats | lossy-blocked |
| Validation | Empty query/documents and `top_n=0` fail closed | invalid-target-field |
| Extensions | Unknown provider-specific rerank extension namespaces are blocked cross-format | unsupported |

## Sync Response Conversion

Cross-format sync response conversion now validates source stop/finish/status
enums before emitting a target body. Same-format runtime response passthrough is
still outside canonical conversion.

| Source field | Target risk | Current behavior | Status |
| --- | --- | --- | --- |
| Same-format response raw stop/status fields | canonical emitters would otherwise normalize unknown enum/status to default target stop values | raw OpenAI Chat `finish_reason`, OpenAI Responses `status`, Claude `stop_reason`/`stop_sequence`, and Gemini `finishReason` are preserved through provider extension metadata | extension-preserved |
| OpenAI Chat `choices[].finish_reason` | unknown value would otherwise emit as target normal stop | valid Chat enum required; unknown values fail with `InvalidEnumValue` | invalid-enum |
| OpenAI Responses `status` | `queued`, `in_progress`, and `cancelled` have no sync target equivalent | non-terminal valid states fail with `LossyConversionBlocked`; invalid states fail with `InvalidEnumValue` | lossy-blocked / invalid-enum |
| OpenAI Responses `incomplete_details.reason=content_filter` | previously mapped to max tokens/`length` | maps to canonical content filter and emits Chat `content_filter` / Claude `content_filtered` / Gemini `SAFETY` | mapped |
| Claude `stop_reason` | unknown value would otherwise emit as target normal stop | valid known stop enum required for cross-format conversion | invalid-enum |
| Gemini `candidates[].finishReason` | known-but-unmappable reasons would otherwise emit as target normal stop | mappable safety/max/stop reasons convert; known unmappable values such as `OTHER`, `MALFORMED_FUNCTION_CALL`, `UNEXPECTED_TOOL_CALL`, `MISSING_THOUGHT_SIGNATURE`, and `MALFORMED_RESPONSE` fail with `LossyConversionBlocked`; future unknown values fail with `InvalidEnumValue` | lossy-blocked / invalid-enum |
| Canonical `Unknown` stop reason | target emitters default to normal stop values | cross-format response conversion blocks canonical unknown stop reasons | lossy-blocked |

## Stream Conversion

Sixth batch first pass is implemented for unknown event handling and runtime
same-format boundaries. Sync response finish/status parity has a first strict
pass; stream finish-reason guardrails are implemented for unknown/unmappable
terminal reasons. Stream event schema fields are covered in the field coverage
matrix; provider-by-provider fixtures cover the runtime event behavior.

Current stream behavior:

| Area | Current behavior | Status |
| --- | --- | --- |
| Provider parsers | OpenAI Chat, OpenAI Responses, Claude, and Gemini unknown stream payloads become `CanonicalStreamEvent::UnknownEvent` | mapped |
| Cross-format stream matrix | Unknown canonical stream events emit a target-format error SSE with `unsupported_stream_event` and terminate conversion | lossy-blocked |
| Stream finish reason guard | Unknown OpenAI finish reasons, unknown Claude `stop_reason`, and Gemini known-but-unmappable `finishReason` values such as `OTHER` are preserved as raw canonical finish strings, then blocked by the matrix with `unsupported_finish_reason` | lossy-blocked |
| OpenAI Responses stream target | Canonical `length` and `content_filter` terminal reasons emit `response.incomplete` with `incomplete_details.reason=max_output_tokens` or `content_filter` instead of `response.completed` | mapped |
| Terminal observer | Unknown provider stream events increment `unknown_event_count`; OpenAI Responses failed events mark terminal error state | mapped |
| Stream -> sync aggregate | Unknown OpenAI Chat, OpenAI Responses, Claude, and Gemini stream events make the runtime finalize checked path return an error and block `body_json` fallback; legacy public aggregate helpers keep `Option` compatibility | lossy-blocked |
| Runtime strict fallback guard | `UnauditedField`, `InvalidEnumValue`, `UnsupportedField`, `LossyConversionBlocked`, and `InvalidTargetField` from registry response conversion are not allowed to fall through legacy conversion helpers | lossy-blocked |
| Runtime same-format stream | Same-format stream passthrough remains outside canonical conversion; stream policy edits are transport-layer only | native |

Stream fixture coverage:

| Provider stream | Covered fixture areas |
| --- | --- |
| OpenAI Chat | sync aggregation for text, tool call IDs/names/argument deltas, finish reason, and usage; cross-format unknown finish/event blocking |
| OpenAI Responses | text snapshot de-duplication, multi-part messages, reasoning/items, function calls, image generation calls, same-family stream sync, unknown event blocking |
| Claude Messages | thinking signatures, tool input deltas, cache/usage aggregation, media emission, unknown stop/event blocking |
| Gemini GenerateContent | text/media/signature aggregation, function calls/results, safety finish mapping, unknown parts/events, and unmappable finish reason blocking |

Matrix-level interception remains the authoritative runtime path for cross-format
unknown events; direct client emitters are covered only as provider/client
building blocks.
