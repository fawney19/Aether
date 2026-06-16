# Format Passthrough Contract

Last audited: 2026-06-03

This document defines the boundary between runtime passthrough, canonical roundtrip tests, and cross-format conversion.

## Runtime Same-Format Path

Runtime same-format provider paths must not call canonical conversion.

Current implementation:

- `crates/aether-provider-transport/src/same_format_provider/mod.rs` checks `api_format_alias_matches(client_api_format, provider_api_format)`.
- When formats match, the provider body is built by copying the parsed JSON object field-for-field.
- When formats differ, the provider body is built through `aether_ai_formats::convert_request_pure`.
- Model override, body rules, model directives, Claude Code sanitization, Gemini function-response id stripping, and stream policy are applied only after the passthrough/conversion branch in provider transport.

Important limitation:

- The current transport helper receives `body_json: &serde_json::Value`, not raw request bytes. It therefore guarantees no canonical conversion and JSON value preservation at this layer, but it cannot preserve original whitespace or object key order by itself.
- True byte-level passthrough for requests with no transport edits requires a higher-level raw-body path that can forward the original bytes directly. Until that raw-body plumbing exists, tests should assert "conversion module not called" and JSON value equivalence for this helper, not byte-for-byte serialization equivalence.

Provider schema drift does not change this rule. If OpenAI, Claude, or Gemini add a new field, same-format runtime routing must still forward it as part of the original provider body. The schema inventory and field coverage matrix are audit aids, not the runtime allowlist for same-format traffic.

## Canonical Same-Format Roundtrip

Canonical same-format roundtrip is only a test/audit mode:

```text
source format -> Canonical -> same source format
```

Required behavior:

- JSON-normalized equality, ignoring object field order and whitespace.
- Field values, array order, unknown fields, extension namespaces, and unknown enum strings must be preserved.
- This path may parse and emit; it is not the runtime path.
- Unknown provider fields are carried in provider extension namespaces and replayed when emitting the same provider format.

## Cross-Format Conversion

Cross-format conversion is strict:

```text
source format -> Canonical -> target format
```

Required behavior:

- Emit only fields valid for the target provider format.
- Map provider-specific enum values through explicit provider enum types.
- Preserve source fields only when the target has an equivalent field or documented extension passthrough.
- Fail closed with `FormatError::UnauditedField`, `FormatError::LossyConversionBlocked`, `FormatError::UnsupportedField`, `FormatError::InvalidEnumValue`, or `FormatError::InvalidTargetField` when no lossless mapping exists.
- Do not use `None` or silent omission to represent conversion failure.
- Newly added provider fields follow the same rule as other unknown fields: preserve same-format, fail closed cross-format with `UnauditedField`. A code change is required only when Aether intentionally supports a new cross-format semantic mapping.

## Pure Conversion Interface

Pure conversion lives in `crates/aether-ai-formats` and is limited to:

- parse
- emit
- provider-specific field/enum mapping
- `ConversionReport`

Pure conversion must not:

- override `model`
- add, remove, or force `stream`
- apply body rules
- apply model directives
- read the original request body to patch missing target fields
- perform provider transport policy edits

Current pure entrypoints:

- `parse_request_pure`
- `emit_request_pure`
- `convert_request_pure`
- `convert_request_pure_with_context`
- `parse_response_pure`
- `emit_response_pure`
- `convert_response_pure`

`convert_request` and `convert_response` remain legacy wrappers for existing callers that still need mapped model/report-context behavior during migration.
