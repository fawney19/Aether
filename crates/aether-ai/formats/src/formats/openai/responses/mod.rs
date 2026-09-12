use base64::{engine::general_purpose::STANDARD_NO_PAD, Engine as _};
use serde_json::Value;

pub mod codex;
pub(crate) mod history;
pub mod request;
pub mod response;
pub mod spec;
pub mod stream;

const TOOL_ERROR_PREFIX: &str = "[tool error]";
const AETHER_REASONING_ITEM_ID_PREFIX: &str = "rs_aether_";
const AETHER_MESSAGE_ITEM_ID_PREFIX: &str = "msg_aether_";
const GEMINI_TOOL_SIGNATURE_CARRIER_PREFIX: &str = "cpa-gemini-responses-carrier-v1:";
const MAX_GEMINI_THOUGHT_SIGNATURE_LEN: usize = 32 * 1024 * 1024;
const MAX_GEMINI_THOUGHT_SIGNATURE_ENCODED_LEN: usize =
    MAX_GEMINI_THOUGHT_SIGNATURE_LEN.div_ceil(3) * 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GeminiToolSignatureCarrierDirection {
    Next,
    Previous,
}

impl GeminiToolSignatureCarrierDirection {
    fn as_str(self) -> &'static str {
        match self {
            Self::Next => "next",
            Self::Previous => "previous",
        }
    }
}

pub(crate) fn encode_gemini_tool_signature_carrier(signature: &str) -> Option<String> {
    encode_gemini_tool_signature_carrier_with_direction(
        signature,
        GeminiToolSignatureCarrierDirection::Next,
    )
}

pub(crate) fn encode_gemini_tool_signature_carrier_with_direction(
    signature: &str,
    direction: GeminiToolSignatureCarrierDirection,
) -> Option<String> {
    (!signature.trim().is_empty() && signature.len() <= MAX_GEMINI_THOUGHT_SIGNATURE_LEN).then(
        || {
            format!(
                "{GEMINI_TOOL_SIGNATURE_CARRIER_PREFIX}{}:function:{}",
                direction.as_str(),
                STANDARD_NO_PAD.encode(signature)
            )
        },
    )
}

pub(crate) fn decode_gemini_tool_signature_carrier(
    carrier: &str,
) -> Option<(String, GeminiToolSignatureCarrierDirection)> {
    let payload = carrier.strip_prefix(GEMINI_TOOL_SIGNATURE_CARRIER_PREFIX)?;
    let (direction, encoded) = payload.split_once(":function:")?;
    let direction = match direction {
        "next" => GeminiToolSignatureCarrierDirection::Next,
        "previous" => GeminiToolSignatureCarrierDirection::Previous,
        _ => return None,
    };
    if encoded.len() > MAX_GEMINI_THOUGHT_SIGNATURE_ENCODED_LEN {
        return None;
    }
    let decoded = STANDARD_NO_PAD.decode(encoded).ok()?;
    if decoded.len() > MAX_GEMINI_THOUGHT_SIGNATURE_LEN {
        return None;
    }
    let signature = String::from_utf8(decoded).ok()?;
    (!signature.trim().is_empty() && !signature.starts_with(GEMINI_TOOL_SIGNATURE_CARRIER_PREFIX))
        .then_some((signature, direction))
}

/// Controls which provider-owned reasoning items may be replayed on a Responses request.
///
/// OpenAI reasoning references are identified by their `rs...` item IDs. DeepSeek's Responses
/// contract instead returns opaque, id-less `reasoning_text` items whose `encrypted_content`
/// must be sent back unchanged on later tool turns.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OpenAiResponsesReasoningReplayPolicy {
    #[default]
    OpenAiItemIds,
    DeepSeekOpaque,
}

/// Builds a stable, wire-compatible ID for a reasoning item synthesized by Aether.
///
/// The marker lets the outbound request sanitizer distinguish synthetic summaries from
/// provider-backed reasoning items. Synthetic items without encrypted reasoning state are useful
/// in client responses, but cannot be replayed as provider-owned reasoning state.
pub fn openai_responses_synthetic_reasoning_item_id(
    response_id: &str,
    output_index: usize,
) -> String {
    let seed = format!("{response_id}:{output_index}");
    format!(
        "{AETHER_REASONING_ITEM_ID_PREFIX}{}",
        uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_OID, seed.as_bytes()).simple()
    )
}

/// Builds a stable, wire-compatible ID for a message item synthesized by Aether.
///
/// Responses clients replay assistant message items verbatim on the next turn and
/// OpenAI requires those IDs to begin with `msg`. Upstream response IDs are not
/// guaranteed to have that prefix (Chat completion IDs and UUIDs are common), so
/// appending a suffix to the response ID is not sufficient. A deterministic UUID
/// keeps the ID stable across sync/stream projections while avoiding assumptions
/// about the upstream ID's shape or length.
pub fn openai_responses_message_item_id(response_id: &str, output_index: usize) -> String {
    let seed = format!("{response_id}:{output_index}");
    format!(
        "{AETHER_MESSAGE_ITEM_ID_PREFIX}{}",
        uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_OID, seed.as_bytes()).simple()
    )
}

/// Repairs legacy/non-OpenAI message IDs in a Responses request in place.
///
/// Aether versions before the `msg_` contract emitted IDs such as
/// `<response-id>_msg`. Clients legitimately replay those assistant items on
/// the next turn, so merely fixing newly generated responses leaves existing
/// conversations broken. Preserve already-valid provider IDs and deterministically
/// remap only message items that do not begin with `msg`.
pub fn normalize_openai_responses_message_item_ids(body: &mut Value) -> usize {
    let Some(items) = body.get_mut("input").and_then(Value::as_array_mut) else {
        return 0;
    };
    let mut repaired = 0usize;
    for (index, item) in items.iter_mut().enumerate() {
        let Some(object) = item.as_object_mut() else {
            continue;
        };
        if object.get("type").and_then(Value::as_str) != Some("message") {
            continue;
        }
        let Some(raw_id) = object.get("id") else {
            // IDs are optional for newly-authored input messages. Only repair
            // an ID that a previous response actually supplied.
            continue;
        };
        let valid = object
            .get("id")
            .and_then(Value::as_str)
            .is_some_and(|id| id.starts_with("msg"));
        if valid {
            continue;
        }
        let source_id = raw_id
            .as_str()
            .filter(|id| !id.trim().is_empty())
            .unwrap_or("missing")
            .to_string();
        object.insert(
            "id".to_string(),
            Value::String(openai_responses_message_item_id(source_id.as_str(), index)),
        );
        repaired += 1;
    }
    repaired
}

/// Removes reasoning history items that cannot be replayed against an OpenAI Responses backend.
///
/// Reasoning IDs are opaque provider references and must never be repaired by changing their
/// prefix. Foreign IDs (for example `item_...`) are therefore removed. Aether's Gemini signature
/// carriers are also removed: they are intentionally transported through the Responses
/// `encrypted_content` field so they can be restored on a later Gemini tool turn, but they are not
/// OpenAI ciphertext and must never be replayed to an OpenAI/Codex backend.
pub fn strip_incompatible_openai_responses_reasoning_items(
    body: &mut Value,
    provider_api_format: &str,
) -> usize {
    strip_incompatible_openai_responses_reasoning_items_with_policy(
        body,
        provider_api_format,
        OpenAiResponsesReasoningReplayPolicy::OpenAiItemIds,
    )
}

pub fn strip_incompatible_openai_responses_reasoning_items_with_policy(
    body: &mut Value,
    provider_api_format: &str,
    policy: OpenAiResponsesReasoningReplayPolicy,
) -> usize {
    if !aether_ai_formats::is_openai_responses_family_format(provider_api_format) {
        return 0;
    }
    // DeepSeek's id-less opaque state is valid only on the normal Responses
    // continuation contract. Both the legacy Compact endpoint and the current
    // `compaction_trigger` operation must retain the strict OpenAI item-id
    // replay rules even when the same provider key serves ordinary Responses.
    let normal_responses =
        aether_ai_formats::normalize_api_format_alias(provider_api_format) == "openai:responses";
    let compact_operation = openai_responses_request_operation(provider_api_format, body).is_some();
    let policy = if policy == OpenAiResponsesReasoningReplayPolicy::DeepSeekOpaque
        && (!normal_responses || compact_operation)
    {
        OpenAiResponsesReasoningReplayPolicy::OpenAiItemIds
    } else {
        policy
    };
    let Some(items) = body.get_mut("input").and_then(Value::as_array_mut) else {
        return 0;
    };
    let original_len = items.len();
    items.retain(|item| openai_responses_reasoning_item_is_replayable(item, policy));
    original_len.saturating_sub(items.len())
}

fn openai_responses_reasoning_item_is_replayable(
    item: &Value,
    policy: OpenAiResponsesReasoningReplayPolicy,
) -> bool {
    let Some(object) = item.as_object() else {
        return true;
    };
    if object.get("type").and_then(Value::as_str) != Some("reasoning") {
        return true;
    }
    if object
        .get("encrypted_content")
        .and_then(Value::as_str)
        .is_some_and(|value| value.starts_with(GEMINI_TOOL_SIGNATURE_CARRIER_PREFIX))
    {
        return false;
    }
    if policy == OpenAiResponsesReasoningReplayPolicy::DeepSeekOpaque
        && deepseek_opaque_reasoning_item_is_replayable(object)
    {
        return true;
    }
    let Some(id) = object
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| id.starts_with("rs"))
    else {
        return false;
    };
    if !id.starts_with(AETHER_REASONING_ITEM_ID_PREFIX) {
        return true;
    }
    object
        .get("encrypted_content")
        .and_then(Value::as_str)
        .is_some_and(|encrypted_content| !encrypted_content.trim().is_empty())
}

fn deepseek_opaque_reasoning_item_is_replayable(object: &serde_json::Map<String, Value>) -> bool {
    if let Some(id) = object.get("id") {
        let id_is_empty = id.is_null() || id.as_str().is_some_and(|value| value.trim().is_empty());
        if !id_is_empty {
            return false;
        }
    }
    let has_encrypted_content = object
        .get("encrypted_content")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty());
    let has_reasoning_text =
        object
            .get("content")
            .and_then(Value::as_array)
            .is_some_and(|content| {
                content.iter().any(|part| {
                    part.get("type").and_then(Value::as_str) == Some("reasoning_text")
                        && part.get("text").is_some_and(Value::is_string)
                })
            });
    has_encrypted_content && has_reasoning_text
}

/// Semantic operation carried by an OpenAI Responses request that asks the
/// service to compact a thread. The request still uses the Responses wire
/// contract and transport endpoint.
pub const OPENAI_RESPONSES_OPERATION_COMPACT: &str = "compact";

/// Resolves the operation expressed by an OpenAI Responses wire request.
///
/// `responses_compaction_v2` is represented by a `compaction_trigger` input
/// item on the normal Responses request. The legacy Compact API format is
/// retained as the same operation for observability and scoped model mapping.
pub fn openai_responses_request_operation(api_format: &str, body: &Value) -> Option<&'static str> {
    if aether_ai_formats::is_openai_responses_compact_format(api_format) {
        return Some(OPENAI_RESPONSES_OPERATION_COMPACT);
    }
    if !aether_ai_formats::is_openai_responses_format(api_format) {
        return None;
    }

    body.get("input")
        .and_then(Value::as_array)
        .is_some_and(|items| {
            items
                .iter()
                .any(|item| item.get("type").and_then(Value::as_str) == Some("compaction_trigger"))
        })
        .then_some(OPENAI_RESPONSES_OPERATION_COMPACT)
}

fn encode_tool_result_error(output: Value, is_error: bool) -> Value {
    if !is_error {
        return output;
    }
    let detail = match output {
        Value::String(text) => text,
        Value::Null => String::new(),
        value => serde_json::to_string(&value).unwrap_or_else(|_| value.to_string()),
    };
    if detail.is_empty() {
        Value::String(TOOL_ERROR_PREFIX.to_string())
    } else {
        Value::String(format!("{TOOL_ERROR_PREFIX}\n{detail}"))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        decode_gemini_tool_signature_carrier, encode_gemini_tool_signature_carrier_with_direction,
        normalize_openai_responses_message_item_ids, openai_responses_message_item_id,
        openai_responses_request_operation, openai_responses_synthetic_reasoning_item_id,
        strip_incompatible_openai_responses_reasoning_items,
        strip_incompatible_openai_responses_reasoning_items_with_policy,
        GeminiToolSignatureCarrierDirection, OpenAiResponsesReasoningReplayPolicy,
        MAX_GEMINI_THOUGHT_SIGNATURE_ENCODED_LEN, MAX_GEMINI_THOUGHT_SIGNATURE_LEN,
        OPENAI_RESPONSES_OPERATION_COMPACT,
    };

    #[test]
    fn gemini_tool_signature_carrier_roundtrips_direction_and_exact_value() {
        let signature = "  opaque-signature-with-padding==  ";
        for direction in [
            GeminiToolSignatureCarrierDirection::Next,
            GeminiToolSignatureCarrierDirection::Previous,
        ] {
            let carrier = encode_gemini_tool_signature_carrier_with_direction(signature, direction)
                .expect("signature carrier");
            assert_eq!(
                decode_gemini_tool_signature_carrier(&carrier),
                Some((signature.to_string(), direction))
            );
        }
    }

    #[test]
    fn gemini_tool_signature_carrier_rejects_nested_and_oversized_values() {
        let nested = encode_gemini_tool_signature_carrier_with_direction(
            "opaque-signature",
            GeminiToolSignatureCarrierDirection::Next,
        )
        .expect("inner carrier");
        let nested = encode_gemini_tool_signature_carrier_with_direction(
            &nested,
            GeminiToolSignatureCarrierDirection::Previous,
        )
        .expect("outer carrier");
        assert_eq!(decode_gemini_tool_signature_carrier(&nested), None);
        assert_eq!(
            encode_gemini_tool_signature_carrier_with_direction(
                &"x".repeat(MAX_GEMINI_THOUGHT_SIGNATURE_LEN + 1),
                GeminiToolSignatureCarrierDirection::Next,
            ),
            None
        );
        let oversized = format!(
            "cpa-gemini-responses-carrier-v1:next:function:{}",
            "A".repeat(MAX_GEMINI_THOUGHT_SIGNATURE_ENCODED_LEN + 1)
        );
        assert_eq!(decode_gemini_tool_signature_carrier(&oversized), None);
    }

    #[test]
    fn resolves_compaction_trigger_as_compact_operation_on_responses_transport() {
        assert_eq!(
            openai_responses_request_operation(
                "openai:responses",
                &json!({
                    "input": [
                        {"role": "user", "content": "keep working"},
                        {"type": "compaction_trigger"}
                    ]
                }),
            ),
            Some(OPENAI_RESPONSES_OPERATION_COMPACT)
        );
        assert_eq!(
            openai_responses_request_operation(
                "openai:responses",
                &json!({"input": [{"role": "user", "content": "keep working"}]}),
            ),
            None
        );
    }

    #[test]
    fn resolves_legacy_compact_contract_without_a_body_marker() {
        assert_eq!(
            openai_responses_request_operation("openai:responses:compact", &json!({})),
            Some(OPENAI_RESPONSES_OPERATION_COMPACT)
        );
    }

    #[test]
    fn synthetic_reasoning_item_ids_are_stable_and_wire_compatible() {
        let first = openai_responses_synthetic_reasoning_item_id("resp_123", 0);
        let second = openai_responses_synthetic_reasoning_item_id("resp_123", 0);
        let other = openai_responses_synthetic_reasoning_item_id("resp_123", 1);

        assert!(first.starts_with("rs_aether_"));
        assert_eq!(first, second);
        assert_ne!(first, other);
    }

    #[test]
    fn synthetic_message_item_ids_are_stable_and_start_with_msg() {
        let first = openai_responses_message_item_id("1c938e58-32a8-4d28-9c34-538d78076895", 0);
        let second = openai_responses_message_item_id("1c938e58-32a8-4d28-9c34-538d78076895", 0);
        let other = openai_responses_message_item_id("chatcmpl-123", 1);

        assert!(first.starts_with("msg_"));
        assert_eq!(first, second);
        assert_ne!(first, other);
    }

    #[test]
    fn normalizes_legacy_message_ids_but_preserves_valid_ids() {
        let mut body = json!({
            "input": [
                {"type": "message", "id": "1c938e58-32a8-4d28-9c34-538d78076895_msg", "role": "assistant"},
                {"type": "message", "id": "msg_provider_123", "role": "assistant"},
                {"type": "function_call", "id": "legacy_call"},
                {"type": "message", "role": "user"}
            ]
        });

        assert_eq!(normalize_openai_responses_message_item_ids(&mut body), 1);
        let input = body["input"].as_array().expect("input array");
        assert!(input[0]["id"]
            .as_str()
            .is_some_and(|id| id.starts_with("msg_")));
        assert_eq!(input[1]["id"], "msg_provider_123");
        assert_eq!(input[2].get("id"), Some(&json!("legacy_call")));
        assert!(input[3].get("id").is_none());
    }

    #[test]
    fn strips_foreign_and_non_replayable_synthetic_reasoning_items() {
        let portable_synthetic = openai_responses_synthetic_reasoning_item_id("resp_123", 1);
        let local_synthetic = openai_responses_synthetic_reasoning_item_id("resp_123", 2);
        let mut body = json!({
            "input": [
                {"type": "reasoning", "id": "rs_provider_123", "summary": []},
                {"type": "reasoning", "id": "item_72d3bd8d367d01977ace23f1", "summary": []},
                {"type": "reasoning", "id": "resp_123_rs_0", "summary": []},
                {"type": "reasoning", "summary": []},
                {
                    "type": "reasoning",
                    "id": portable_synthetic,
                    "summary": [],
                    "encrypted_content": "opaque"
                },
                {"type": "reasoning", "id": local_synthetic, "summary": []},
                {"type": "message", "id": "item_message_123", "role": "user", "content": "hi"}
            ]
        });

        assert_eq!(
            strip_incompatible_openai_responses_reasoning_items(&mut body, "openai:responses"),
            4
        );
        let input = body["input"].as_array().expect("input array");
        assert_eq!(input.len(), 3);
        assert_eq!(input[0]["id"], "rs_provider_123");
        assert_eq!(input[1]["encrypted_content"], "opaque");
        assert_eq!(input[2]["id"], "item_message_123");
    }

    #[test]
    fn strips_gemini_signature_carriers_before_openai_replay() {
        let gemini_item_id = openai_responses_synthetic_reasoning_item_id("resp_gemini", 0);
        let openai_item_id = openai_responses_synthetic_reasoning_item_id("resp_openai", 0);
        let carrier = encode_gemini_tool_signature_carrier_with_direction(
            "opaque-gemini-thought-signature",
            GeminiToolSignatureCarrierDirection::Next,
        )
        .expect("Gemini signature carrier");
        let mut body = json!({
            "input": [
                {
                    "type": "reasoning",
                    "id": gemini_item_id,
                    "summary": [],
                    "encrypted_content": carrier
                },
                {
                    "type": "reasoning",
                    "id": openai_item_id,
                    "summary": [],
                    "encrypted_content": "provider-encrypted-state"
                },
                {"type": "reasoning", "id": "rs_provider_123", "summary": []}
            ]
        });

        assert_eq!(
            strip_incompatible_openai_responses_reasoning_items(&mut body, "openai:responses"),
            1
        );
        let input = body["input"].as_array().expect("input array");
        assert_eq!(input.len(), 2);
        assert_eq!(input[0]["encrypted_content"], "provider-encrypted-state");
        assert_eq!(input[1]["id"], "rs_provider_123");
    }

    #[test]
    fn reasoning_item_sanitizer_is_scoped_to_responses_targets() {
        let mut body = json!({
            "input": [{"type": "reasoning", "id": "item_foreign", "summary": []}]
        });

        assert_eq!(
            strip_incompatible_openai_responses_reasoning_items(&mut body, "openai:chat"),
            0
        );
        assert_eq!(body["input"].as_array().map(Vec::len), Some(1));
    }

    #[test]
    fn deepseek_policy_preserves_idless_opaque_reasoning_text_only_for_deepseek() {
        let item = json!({
            "type": "reasoning",
            "encrypted_content": "550e8400-e29b-41d4-a716-446655440000",
            "content": [{
                "type": "reasoning_text",
                "text": "opaque provider reasoning that must be replayed"
            }]
        });
        let mut strict = json!({"input": [item.clone()]});
        let mut deepseek = json!({"input": [item]});

        assert_eq!(
            strip_incompatible_openai_responses_reasoning_items_with_policy(
                &mut strict,
                "openai:responses",
                OpenAiResponsesReasoningReplayPolicy::OpenAiItemIds,
            ),
            1
        );
        assert_eq!(
            strip_incompatible_openai_responses_reasoning_items_with_policy(
                &mut deepseek,
                "openai:responses",
                OpenAiResponsesReasoningReplayPolicy::DeepSeekOpaque,
            ),
            0
        );
        assert_eq!(deepseek["input"].as_array().map(Vec::len), Some(1));
    }

    #[test]
    fn deepseek_policy_does_not_preserve_unbound_reasoning_summaries() {
        let mut body = json!({
            "input": [
                {
                    "type": "reasoning",
                    "content": [{"type": "reasoning_text", "text": "missing state"}]
                },
                {
                    "type": "reasoning",
                    "encrypted_content": "opaque-without-reasoning-text",
                    "summary": [{"type": "summary_text", "text": "summary"}]
                }
            ]
        });

        assert_eq!(
            strip_incompatible_openai_responses_reasoning_items_with_policy(
                &mut body,
                "openai:responses",
                OpenAiResponsesReasoningReplayPolicy::DeepSeekOpaque,
            ),
            2
        );
        assert_eq!(body["input"].as_array().map(Vec::len), Some(0));
    }

    #[test]
    fn deepseek_policy_preserves_empty_reasoning_text_with_opaque_state() {
        let mut body = json!({
            "input": [{
                "type": "reasoning",
                "encrypted_content": "opaque-state",
                "content": [{"type": "reasoning_text", "text": ""}],
                "future_capability": {"preserve": true}
            }]
        });

        assert_eq!(
            strip_incompatible_openai_responses_reasoning_items_with_policy(
                &mut body,
                "openai:responses",
                OpenAiResponsesReasoningReplayPolicy::DeepSeekOpaque,
            ),
            0
        );
        assert_eq!(body["input"][0]["content"][0]["text"], "");
        assert_eq!(body["input"][0]["future_capability"]["preserve"], true);
    }

    #[test]
    fn deepseek_policy_rejects_non_string_reasoning_text() {
        let mut body = json!({
            "input": [{
                "type": "reasoning",
                "encrypted_content": "opaque-state",
                "content": [{"type": "reasoning_text", "text": {"not": "text"}}]
            }]
        });

        assert_eq!(
            strip_incompatible_openai_responses_reasoning_items_with_policy(
                &mut body,
                "openai:responses",
                OpenAiResponsesReasoningReplayPolicy::DeepSeekOpaque,
            ),
            1
        );
        assert_eq!(body["input"].as_array().map(Vec::len), Some(0));
    }

    #[test]
    fn deepseek_policy_keeps_strict_replay_for_compaction_trigger_operation() {
        let mut body = json!({
            "input": [
                {
                    "type": "reasoning",
                    "encrypted_content": "opaque-state",
                    "content": [{"type": "reasoning_text", "text": "thinking"}]
                },
                {"type": "compaction_trigger"}
            ]
        });

        assert_eq!(
            strip_incompatible_openai_responses_reasoning_items_with_policy(
                &mut body,
                "openai:responses",
                OpenAiResponsesReasoningReplayPolicy::DeepSeekOpaque,
            ),
            1
        );
        assert_eq!(body["input"], json!([{"type": "compaction_trigger"}]));
    }

    #[test]
    fn deepseek_policy_does_not_preserve_opaque_item_with_foreign_id() {
        let mut body = json!({
            "input": [{
                "type": "reasoning",
                "id": "item_provider_owned",
                "encrypted_content": "opaque-state",
                "content": [{"type": "reasoning_text", "text": "thinking"}]
            }]
        });

        assert_eq!(
            strip_incompatible_openai_responses_reasoning_items_with_policy(
                &mut body,
                "openai:responses",
                OpenAiResponsesReasoningReplayPolicy::DeepSeekOpaque,
            ),
            1
        );
        assert_eq!(body["input"].as_array().map(Vec::len), Some(0));
    }
}
