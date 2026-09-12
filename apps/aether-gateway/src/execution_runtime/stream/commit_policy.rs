use std::time::Duration;

use serde_json::Value;

use crate::execution_runtime::MAX_STREAM_PREFETCH_BYTES;

const ANTHROPIC_PRECOMMIT_MAX_WAIT: Duration = Duration::from_millis(750);
const GEMINI_PRECOMMIT_MAX_WAIT: Duration = Duration::from_millis(750);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StreamCommitPolicy {
    ResponseHeaders,
    FirstClassifiedBody,
    FirstSseSemanticEvent {
        max_bytes: usize,
        max_wait: Duration,
    },
    FirstAnthropicSemanticEvent {
        max_bytes: usize,
        max_wait: Duration,
    },
    FirstGeminiSemanticEvent {
        max_bytes: usize,
        max_wait: Duration,
    },
}

impl StreamCommitPolicy {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn for_response(
        has_direct_finalize: bool,
        content_type: Option<&str>,
        provider_api_format: &str,
        client_api_format: &str,
        has_private_stream_normalizer: bool,
        has_local_stream_rewriter: bool,
        force_prefetch: bool,
    ) -> Self {
        if !has_direct_finalize {
            return Self::FirstClassifiedBody;
        }

        let content_type = content_type
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if content_type.contains("text/event-stream") {
            if provider_api_format.eq_ignore_ascii_case("openai:image")
                || client_api_format.eq_ignore_ascii_case("openai:image")
            {
                return if force_prefetch {
                    Self::FirstClassifiedBody
                } else {
                    Self::ResponseHeaders
                };
            }
            if provider_api_format.eq_ignore_ascii_case("claude:messages")
                && provider_api_format.eq_ignore_ascii_case(client_api_format)
                && !has_private_stream_normalizer
                && !has_local_stream_rewriter
            {
                return Self::FirstAnthropicSemanticEvent {
                    max_bytes: MAX_STREAM_PREFETCH_BYTES,
                    max_wait: ANTHROPIC_PRECOMMIT_MAX_WAIT,
                };
            }
            if provider_api_format.eq_ignore_ascii_case("gemini:generate_content") {
                return Self::FirstGeminiSemanticEvent {
                    max_bytes: MAX_STREAM_PREFETCH_BYTES,
                    max_wait: GEMINI_PRECOMMIT_MAX_WAIT,
                };
            }
            return Self::FirstSseSemanticEvent {
                max_bytes: MAX_STREAM_PREFETCH_BYTES,
                max_wait: Duration::from_secs(30),
            };
        }

        if force_prefetch {
            return Self::FirstClassifiedBody;
        }

        if has_private_stream_normalizer || has_local_stream_rewriter {
            return Self::FirstClassifiedBody;
        }

        if !provider_api_format.eq_ignore_ascii_case(client_api_format) {
            return Self::FirstClassifiedBody;
        }

        if content_type.is_empty() {
            return Self::ResponseHeaders;
        }

        if content_type.contains("json") || content_type.ends_with("+json") {
            Self::FirstClassifiedBody
        } else {
            Self::ResponseHeaders
        }
    }

    pub(super) const fn commits_on_response_headers(self) -> bool {
        matches!(self, Self::ResponseHeaders)
    }

    pub(super) const fn requires_bounded_frame_wait(self) -> bool {
        matches!(
            self,
            Self::FirstAnthropicSemanticEvent { .. }
                | Self::FirstGeminiSemanticEvent { .. }
                | Self::FirstSseSemanticEvent { .. }
        )
    }

    pub(super) const fn max_precommit_wait(self) -> Option<Duration> {
        match self {
            Self::FirstAnthropicSemanticEvent { max_wait, .. }
            | Self::FirstGeminiSemanticEvent { max_wait, .. }
            | Self::FirstSseSemanticEvent { max_wait, .. } => Some(max_wait),
            Self::ResponseHeaders | Self::FirstClassifiedBody => None,
        }
    }

    pub(super) const fn is_native_anthropic(self) -> bool {
        matches!(self, Self::FirstAnthropicSemanticEvent { .. })
    }

    pub(super) const fn is_gemini(self) -> bool {
        matches!(self, Self::FirstGeminiSemanticEvent { .. })
    }

    pub(super) fn with_precommit_wait(mut self, wait: Duration) -> Self {
        match &mut self {
            Self::FirstAnthropicSemanticEvent { max_wait, .. }
            | Self::FirstGeminiSemanticEvent { max_wait, .. }
            | Self::FirstSseSemanticEvent { max_wait, .. } => *max_wait = wait,
            _ => {}
        }
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StreamCommitState {
    Uncommitted,
    Committed,
    Terminal,
}

#[derive(Debug, PartialEq)]
pub(super) enum StreamPrecommitObservation {
    Pending,
    Commit,
    UpstreamError { status_code: u16, body_json: Value },
}

#[derive(Debug)]
pub(super) struct StreamCommitGate {
    policy: StreamCommitPolicy,
    state: StreamCommitState,
    observed_bytes: usize,
    anthropic: AnthropicSsePrecommitInspector,
    gemini: GeminiSsePrecommitInspector,
    generic: GenericSsePrecommitInspector,
}

impl StreamCommitGate {
    pub(super) fn new(policy: StreamCommitPolicy) -> Self {
        let state = if policy.commits_on_response_headers() {
            StreamCommitState::Committed
        } else {
            StreamCommitState::Uncommitted
        };
        Self {
            policy,
            state,
            observed_bytes: 0,
            anthropic: AnthropicSsePrecommitInspector::default(),
            gemini: GeminiSsePrecommitInspector::default(),
            generic: GenericSsePrecommitInspector::default(),
        }
    }

    pub(super) const fn state(&self) -> StreamCommitState {
        self.state
    }

    pub(super) const fn is_uncommitted(&self) -> bool {
        matches!(self.state, StreamCommitState::Uncommitted)
    }

    pub(super) fn observe_provider_bytes(&mut self, chunk: &[u8]) -> StreamPrecommitObservation {
        if self.state != StreamCommitState::Uncommitted {
            return StreamPrecommitObservation::Commit;
        }

        let (max_bytes, observation) = match self.policy {
            StreamCommitPolicy::FirstAnthropicSemanticEvent { max_bytes, .. } => {
                (max_bytes, self.anthropic.observe(chunk, max_bytes))
            }
            StreamCommitPolicy::FirstGeminiSemanticEvent { max_bytes, .. } => {
                (max_bytes, self.gemini.observe(chunk, max_bytes))
            }
            StreamCommitPolicy::FirstSseSemanticEvent { max_bytes, .. } => {
                (max_bytes, self.generic.observe(chunk, max_bytes))
            }
            StreamCommitPolicy::ResponseHeaders | StreamCommitPolicy::FirstClassifiedBody => {
                return StreamPrecommitObservation::Pending;
            }
        };

        self.observed_bytes = self.observed_bytes.saturating_add(chunk.len());
        match observation {
            SemanticSseObservation::Pending => {}
            SemanticSseObservation::SemanticEvent => {
                self.state = StreamCommitState::Committed;
                return StreamPrecommitObservation::Commit;
            }
            SemanticSseObservation::Error {
                status_code,
                body_json,
            } => {
                self.state = StreamCommitState::Terminal;
                return StreamPrecommitObservation::UpstreamError {
                    status_code,
                    body_json,
                };
            }
        }

        if self.observed_bytes >= max_bytes {
            self.commit();
            StreamPrecommitObservation::Commit
        } else {
            StreamPrecommitObservation::Pending
        }
    }

    pub(super) fn commit(&mut self) {
        if self.state == StreamCommitState::Uncommitted {
            self.state = StreamCommitState::Committed;
        }
    }
}

#[derive(Debug)]
enum SemanticSseObservation {
    Pending,
    SemanticEvent,
    Error { status_code: u16, body_json: Value },
}

#[derive(Debug, Default)]
struct GenericSsePrecommitInspector {
    buffered: Vec<u8>,
}

impl GenericSsePrecommitInspector {
    fn observe(&mut self, chunk: &[u8], max_bytes: usize) -> SemanticSseObservation {
        let remaining = max_bytes.saturating_sub(self.buffered.len());
        self.buffered
            .extend_from_slice(&chunk[..chunk.len().min(remaining)]);
        while let Some((record_end, separator_len)) = find_sse_record_boundary(&self.buffered) {
            let record = self.buffered[..record_end].to_vec();
            self.buffered.drain(..record_end + separator_len);
            match classify_generic_sse_record(&record) {
                SemanticSseObservation::Pending => {}
                observation => return observation,
            }
        }
        if chunk.len() > remaining {
            SemanticSseObservation::SemanticEvent
        } else {
            SemanticSseObservation::Pending
        }
    }
}

fn classify_generic_sse_record(record: &[u8]) -> SemanticSseObservation {
    let Ok(record) = std::str::from_utf8(record) else {
        return SemanticSseObservation::SemanticEvent;
    };
    let normalized = record.replace("\r\n", "\n").replace('\r', "\n");
    let event_type = normalized
        .lines()
        .find_map(|line| line.strip_prefix("event:").map(str::trim));
    let data = normalized
        .lines()
        .filter_map(|line| line.strip_prefix("data:").map(str::trim_start))
        .collect::<Vec<_>>()
        .join("\n");
    if data.trim().is_empty() || matches!(event_type, Some("ping" | "heartbeat" | "keepalive")) {
        return SemanticSseObservation::Pending;
    }
    if data.trim() == "[DONE]" {
        return SemanticSseObservation::SemanticEvent;
    }
    let Ok(body_json) = serde_json::from_str::<Value>(data.trim()) else {
        return SemanticSseObservation::SemanticEvent;
    };
    let payload_type = body_json.get("type").and_then(Value::as_str).or(event_type);
    if payload_type.is_some_and(is_anthropic_semantic_event_type) {
        return classify_anthropic_sse_record(record.as_bytes());
    }
    let error = body_json
        .get("error")
        .filter(|value| !value.is_null())
        .or_else(|| {
            body_json
                .pointer("/response/error")
                .filter(|value| !value.is_null())
        });
    if error.is_some()
        || matches!(payload_type, Some("error" | "response.failed"))
        || body_json.get("status").and_then(Value::as_str) == Some("failed")
    {
        let failure = error
            .map(|error| serde_json::json!({ "error": error }))
            .unwrap_or_else(|| body_json.clone());
        return SemanticSseObservation::Error {
            status_code: crate::execution_runtime::submission::resolve_local_sync_error_status_code(
                200, &failure,
            ),
            body_json: failure,
        };
    }
    if matches!(
        payload_type,
        Some("ping" | "response.created" | "response.in_progress" | "response.queued")
    ) {
        return SemanticSseObservation::Pending;
    }
    if payload_type == Some("response.output_item.added")
        && matches!(
            body_json.pointer("/item/type").and_then(Value::as_str),
            Some("message" | "reasoning")
        )
        && body_json
            .pointer("/item/content")
            .and_then(Value::as_array)
            .is_none_or(Vec::is_empty)
        && body_json
            .pointer("/item/summary")
            .and_then(Value::as_array)
            .is_none_or(Vec::is_empty)
    {
        return SemanticSseObservation::Pending;
    }
    if matches!(
        payload_type,
        Some("response.content_part.added" | "response.reasoning_summary_part.added")
    ) && matches!(
        body_json.pointer("/part/type").and_then(Value::as_str),
        Some("output_text" | "summary_text" | "refusal")
    ) && !body_json
        .pointer("/part/text")
        .is_some_and(value_has_semantic_content)
        && !body_json
            .pointer("/part/refusal")
            .is_some_and(value_has_semantic_content)
    {
        return SemanticSseObservation::Pending;
    }
    if let Some(choices) = body_json.get("choices").and_then(Value::as_array) {
        let semantic = choices.iter().any(|choice| {
            choice
                .get("finish_reason")
                .is_some_and(|value| !value.is_null())
                || choice.get("text").is_some_and(value_has_semantic_content)
                || choice
                    .get("delta")
                    .or_else(|| choice.get("message"))
                    .and_then(Value::as_object)
                    .is_some_and(|delta| {
                        delta.iter().any(|(name, value)| {
                            name != "role" && value_has_semantic_content(value)
                        })
                    })
        });
        return if semantic {
            SemanticSseObservation::SemanticEvent
        } else {
            SemanticSseObservation::Pending
        };
    }
    SemanticSseObservation::SemanticEvent
}

fn value_has_semantic_content(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::String(text) => !text.is_empty(),
        Value::Array(values) => !values.is_empty(),
        Value::Object(values) => !values.is_empty(),
        _ => true,
    }
}

#[derive(Debug, Default)]
struct AnthropicSsePrecommitInspector {
    buffered: Vec<u8>,
}

impl AnthropicSsePrecommitInspector {
    fn observe(&mut self, chunk: &[u8], max_bytes: usize) -> SemanticSseObservation {
        let remaining = max_bytes.saturating_sub(self.buffered.len());
        let truncated = chunk.len() > remaining;
        self.buffered
            .extend_from_slice(&chunk[..chunk.len().min(remaining)]);

        while let Some((record_end, separator_len)) = find_sse_record_boundary(&self.buffered) {
            let record = self.buffered[..record_end].to_vec();
            self.buffered.drain(..record_end + separator_len);
            match classify_anthropic_sse_record(&record) {
                SemanticSseObservation::Pending => {}
                decision => return decision,
            }
        }

        if truncated {
            SemanticSseObservation::SemanticEvent
        } else {
            SemanticSseObservation::Pending
        }
    }
}

#[derive(Debug, Default)]
struct GeminiSsePrecommitInspector {
    buffered: Vec<u8>,
}

impl GeminiSsePrecommitInspector {
    fn observe(&mut self, chunk: &[u8], max_bytes: usize) -> SemanticSseObservation {
        let remaining = max_bytes.saturating_sub(self.buffered.len());
        let truncated = chunk.len() > remaining;
        self.buffered
            .extend_from_slice(&chunk[..chunk.len().min(remaining)]);

        while let Some((record_end, separator_len)) = find_sse_record_boundary(&self.buffered) {
            let record = self.buffered[..record_end].to_vec();
            self.buffered.drain(..record_end + separator_len);
            match classify_gemini_sse_record(&record) {
                SemanticSseObservation::Pending => {}
                decision => return decision,
            }
        }

        if truncated {
            SemanticSseObservation::SemanticEvent
        } else {
            SemanticSseObservation::Pending
        }
    }
}

pub(super) fn find_sse_record_boundary(buffer: &[u8]) -> Option<(usize, usize)> {
    let mut cursor = 0;
    while cursor < buffer.len() {
        let (line_end, line_ending_len) = next_sse_line_ending(buffer, cursor)?;
        let next_line_start = line_end + line_ending_len;
        let Some((next_line_end, next_line_ending_len)) =
            next_sse_line_ending(buffer, next_line_start)
        else {
            return None;
        };
        if next_line_end == next_line_start {
            return Some((
                line_end,
                line_ending_len.saturating_add(next_line_ending_len),
            ));
        }
        cursor = next_line_start;
    }
    None
}

fn next_sse_line_ending(buffer: &[u8], start: usize) -> Option<(usize, usize)> {
    let relative = buffer
        .get(start..)?
        .iter()
        .position(|byte| matches!(byte, b'\r' | b'\n'))?;
    let index = start + relative;
    let ending_len = if buffer[index] == b'\r' && buffer.get(index + 1) == Some(&b'\n') {
        2
    } else {
        1
    };
    Some((index, ending_len))
}

fn classify_anthropic_sse_record(record: &[u8]) -> SemanticSseObservation {
    let Ok(record) = std::str::from_utf8(record) else {
        return SemanticSseObservation::Pending;
    };
    let normalized_record = record.replace("\r\n", "\n").replace('\r', "\n");
    let mut event_type = None;
    let mut data = String::new();
    for line in normalized_record.lines() {
        if line.starts_with(':') {
            continue;
        }
        if let Some(value) = line.strip_prefix("event:") {
            let value = value.trim();
            if !value.is_empty() {
                event_type = Some(value);
            }
            continue;
        }
        if let Some(value) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(value.trim_start());
        }
    }
    if data.trim().is_empty() {
        return SemanticSseObservation::Pending;
    }

    let Ok(body_json) = serde_json::from_str::<Value>(data.trim()) else {
        return SemanticSseObservation::Pending;
    };
    let payload_type = body_json.get("type").and_then(Value::as_str).map(str::trim);
    if event_type == Some("error") || payload_type == Some("error") {
        return SemanticSseObservation::Error {
            status_code: anthropic_error_status_code(&body_json),
            body_json,
        };
    }

    let semantic_type = match (event_type, payload_type) {
        (Some(event_type), Some(payload_type)) if event_type == payload_type => Some(event_type),
        (None, Some(payload_type)) => Some(payload_type),
        _ => None,
    };
    let setup_only = match semantic_type {
        Some("message_start") => body_json
            .pointer("/message/content")
            .and_then(Value::as_array)
            .is_none_or(Vec::is_empty),
        Some("content_block_start") => {
            let block_type = body_json
                .pointer("/content_block/type")
                .and_then(Value::as_str);
            matches!(block_type, Some("text" | "thinking"))
                && !body_json
                    .pointer("/content_block/text")
                    .is_some_and(value_has_semantic_content)
                && !body_json
                    .pointer("/content_block/thinking")
                    .is_some_and(value_has_semantic_content)
        }
        Some("content_block_stop") => true,
        Some("message_delta") => body_json
            .pointer("/delta/stop_reason")
            .is_none_or(Value::is_null),
        _ => false,
    };
    if !setup_only && semantic_type.is_some_and(is_anthropic_semantic_event_type) {
        SemanticSseObservation::SemanticEvent
    } else {
        SemanticSseObservation::Pending
    }
}

fn classify_gemini_sse_record(record: &[u8]) -> SemanticSseObservation {
    let Ok(record) = std::str::from_utf8(record) else {
        return SemanticSseObservation::Pending;
    };
    let data = record
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .lines()
        .filter_map(|line| line.strip_prefix("data:").map(str::trim_start))
        .collect::<Vec<_>>()
        .join("\n");
    if data.trim().is_empty() {
        return SemanticSseObservation::Pending;
    }
    if data.trim() == "[DONE]" {
        return SemanticSseObservation::SemanticEvent;
    }

    let Ok(body_json) = serde_json::from_str::<Value>(data.trim()) else {
        return SemanticSseObservation::Pending;
    };
    let response = body_json.get("response").unwrap_or(&body_json);
    let Some(candidates) = response.get("candidates").and_then(Value::as_array) else {
        return SemanticSseObservation::Pending;
    };

    for candidate in candidates {
        let finish_reason = candidate
            .get("finishReason")
            .or_else(|| candidate.get("finish_reason"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty());
        if let Some(finish_reason) = finish_reason.filter(|reason| {
            matches!(
                *reason,
                "MALFORMED_FUNCTION_CALL"
                    | "UNEXPECTED_TOOL_CALL"
                    | "TOO_MANY_TOOL_CALLS"
                    | "MISSING_THOUGHT_SIGNATURE"
                    | "MALFORMED_RESPONSE"
            )
        }) {
            let message = candidate
                .get("finishMessage")
                .or_else(|| candidate.get("finish_message"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| format!("Gemini stream ended with {finish_reason}"));
            return SemanticSseObservation::Error {
                status_code: 502,
                body_json: serde_json::json!({
                    "error": {
                        "type": "upstream_gemini_finish_error",
                        "code": finish_reason,
                        "message": message,
                        "upstream_status": 200
                    }
                }),
            };
        }

        if finish_reason.is_some() {
            return SemanticSseObservation::SemanticEvent;
        }
        let Some(parts) = candidate
            .get("content")
            .and_then(|content| content.get("parts"))
            .and_then(Value::as_array)
        else {
            continue;
        };
        if parts.iter().any(gemini_part_is_client_semantic) {
            return SemanticSseObservation::SemanticEvent;
        }
    }

    SemanticSseObservation::Pending
}

fn gemini_part_is_client_semantic(part: &Value) -> bool {
    let Some(part) = part.as_object() else {
        return true;
    };
    if part
        .keys()
        .any(|key| !matches!(key.as_str(), "text" | "thought" | "thoughtSignature"))
    {
        return true;
    }
    if part.get("thought").and_then(Value::as_bool) == Some(true) {
        return part
            .get("text")
            .and_then(Value::as_str)
            .is_some_and(|text| !text.is_empty());
    }
    if part.keys().all(|key| key == "thoughtSignature") {
        return false;
    }
    if part
        .get("text")
        .and_then(Value::as_str)
        .is_some_and(|text| !text.is_empty())
    {
        return true;
    }
    false
}

fn is_anthropic_semantic_event_type(event_type: &str) -> bool {
    matches!(
        event_type,
        "message_start"
            | "content_block_start"
            | "content_block_delta"
            | "content_block_stop"
            | "message_delta"
            | "message_stop"
    )
}

pub(super) fn anthropic_error_status_code(body_json: &Value) -> u16 {
    let error_type = body_json
        .get("error")
        .and_then(|error| error.get("type"))
        .or_else(|| body_json.get("type"))
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    match error_type {
        "invalid_request_error" => 400,
        "authentication_error" => 401,
        "permission_error" => 403,
        "not_found_error" => 404,
        "request_too_large" => 413,
        "rate_limit_error" => 429,
        "overloaded_error" => 529,
        _ => 500,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn image_streams_only_prefetch_when_explicitly_requested() {
        for force_prefetch in [false, true] {
            let policy = super::StreamCommitPolicy::for_response(
                true,
                Some("text/event-stream"),
                "openai:image",
                "openai:image",
                false,
                false,
                force_prefetch,
            );
            assert_eq!(policy.commits_on_response_headers(), !force_prefetch);
            assert!(!policy.requires_bounded_frame_wait());
        }
    }

    #[test]
    fn generic_sse_waits_through_setup_and_classifies_fragmented_errors() {
        let setup = b"event: response.created\ndata: {\"type\":\"response.created\"}\n\n";
        let failure = b"event: response.failed\ndata: {\"type\":\"response.failed\",\"response\":{\"error\":{\"type\":\"server_error\",\"message\":\"capacity exhausted\"}}}\n\n";
        for split in 1..failure.len() {
            let policy = super::StreamCommitPolicy::FirstSseSemanticEvent {
                max_bytes: 4096,
                max_wait: std::time::Duration::from_secs(1),
            };
            let mut gate = super::StreamCommitGate::new(policy);
            assert_eq!(
                gate.observe_provider_bytes(setup),
                super::StreamPrecommitObservation::Pending
            );
            for control in [
                b"event: ping\ndata: keepalive\n\n".as_slice(),
                b"data: {\"type\":\"response.output_item.added\",\"item\":{\"type\":\"reasoning\",\"summary\":[]}}\n\n".as_slice(),
                b"data: {\"type\":\"response.reasoning_summary_part.added\",\"part\":{\"type\":\"summary_text\",\"text\":\"\"}}\n\n".as_slice(),
            ] {
                assert_eq!(gate.observe_provider_bytes(control), super::StreamPrecommitObservation::Pending);
            }
            assert_eq!(
                gate.observe_provider_bytes(&failure[..split]),
                super::StreamPrecommitObservation::Pending
            );
            assert!(matches!(
                gate.observe_provider_bytes(&failure[split..]),
                super::StreamPrecommitObservation::UpstreamError { .. }
            ));
        }
    }

    #[test]
    fn generic_sse_commits_on_content_or_tool_call_but_not_role() {
        for output in [
            "data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"id\":\"call-1\"}]}}]}\n\n",
        ] {
            let mut gate =
                super::StreamCommitGate::new(super::StreamCommitPolicy::FirstSseSemanticEvent {
                    max_bytes: 4096,
                    max_wait: std::time::Duration::from_secs(1),
                });
            assert_eq!(gate.observe_provider_bytes(b"data: {\"choices\":[{\"delta\":{\"role\":\"assistant\",\"content\":\"\"}}]}\n\n"), super::StreamPrecommitObservation::Pending);
            assert_eq!(
                gate.observe_provider_bytes(output.as_bytes()),
                super::StreamPrecommitObservation::Commit
            );
            assert_eq!(
                gate.observe_provider_bytes(b"data: {\"error\":{\"message\":\"late error\"}}\n\n"),
                super::StreamPrecommitObservation::Commit
            );
        }
    }

    #[test]
    fn native_anthropic_setup_does_not_hide_an_early_error() {
        let mut gate =
            super::StreamCommitGate::new(super::StreamCommitPolicy::FirstAnthropicSemanticEvent {
                max_bytes: 4096,
                max_wait: std::time::Duration::from_secs(1),
            });
        assert_eq!(gate.observe_provider_bytes(b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"content\":[]}}\n\n"), super::StreamPrecommitObservation::Pending);
        assert_eq!(gate.observe_provider_bytes(b"event: content_block_start\ndata: {\"type\":\"content_block_start\",\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n"), super::StreamPrecommitObservation::Pending);
        assert!(matches!(gate.observe_provider_bytes(b"event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\"}}\n\n"), super::StreamPrecommitObservation::UpstreamError { status_code: 529, .. }));
    }
    use std::time::Duration;

    use super::{
        anthropic_error_status_code, StreamCommitGate, StreamCommitPolicy, StreamCommitState,
        StreamPrecommitObservation,
    };

    fn native_anthropic_policy() -> StreamCommitPolicy {
        StreamCommitPolicy::FirstAnthropicSemanticEvent {
            max_bytes: 16_384,
            max_wait: Duration::from_millis(750),
        }
    }

    fn gemini_policy() -> StreamCommitPolicy {
        StreamCommitPolicy::FirstGeminiSemanticEvent {
            max_bytes: 16_384,
            max_wait: Duration::from_millis(750),
        }
    }

    #[test]
    fn policy_selects_bounded_anthropic_gate_only_for_native_same_format_sse() {
        let native = StreamCommitPolicy::for_response(
            true,
            Some("text/event-stream; charset=utf-8"),
            "claude:messages",
            "claude:messages",
            false,
            false,
            false,
        );
        assert!(native.is_native_anthropic());
        assert_eq!(
            native.max_precommit_wait(),
            Some(Duration::from_millis(750))
        );
        assert!(StreamCommitPolicy::for_response(
            true,
            Some("text/event-stream"),
            "openai:chat",
            "claude:messages",
            false,
            false,
            false,
        )
        .requires_bounded_frame_wait());
        assert!(StreamCommitPolicy::for_response(
            true,
            Some("text/event-stream"),
            "claude:messages",
            "claude:messages",
            false,
            true,
            false,
        )
        .requires_bounded_frame_wait());
    }

    #[test]
    fn policy_selects_bounded_gemini_gate_for_event_streams() {
        let policy = StreamCommitPolicy::for_response(
            true,
            Some("text/event-stream"),
            "gemini:generate_content",
            "openai:responses",
            false,
            true,
            false,
        );

        assert!(policy.is_gemini());
        assert!(policy.requires_bounded_frame_wait());
        assert_eq!(
            policy.max_precommit_wait(),
            Some(Duration::from_millis(750))
        );
    }

    #[test]
    fn gemini_gate_commits_on_first_nonempty_thought() {
        let mut gate = StreamCommitGate::new(gemini_policy());
        let thought = b"data: {\"response\":{\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"thought\":true,\"text\":\"checking\"}]}}]}}\n\n";

        assert_eq!(
            gate.observe_provider_bytes(thought),
            StreamPrecommitObservation::Commit
        );
        assert_eq!(gate.state(), StreamCommitState::Committed);
    }

    #[test]
    fn gemini_gate_commits_on_function_call_even_with_thought_marker() {
        let mut gate = StreamCommitGate::new(gemini_policy());
        let tool_call = b"data: {\"response\":{\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"thought\":true,\"functionCall\":{\"name\":\"validate\",\"args\":{}}}]}}]}}\n\n";

        assert_eq!(
            gate.observe_provider_bytes(tool_call),
            StreamPrecommitObservation::Commit
        );
        assert_eq!(gate.state(), StreamCommitState::Committed);
    }

    #[test]
    fn gemini_gate_rejects_malformed_function_call_before_commit() {
        let mut gate = StreamCommitGate::new(gemini_policy());
        let thought = b"data: {\"response\":{\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"thoughtSignature\":\"signature\",\"text\":\"\"}]}}]}}\n\n";
        let malformed = b"data: {\"response\":{\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"thoughtSignature\":\"signature\",\"text\":\"\"}]},\"finishReason\":\"MALFORMED_FUNCTION_CALL\",\"finishMessage\":\"Malformed function call: Function call is empty - no input to parse.\"}]}}\n\n";

        assert_eq!(
            gate.observe_provider_bytes(thought),
            StreamPrecommitObservation::Pending
        );
        let StreamPrecommitObservation::UpstreamError {
            status_code,
            body_json,
        } = gate.observe_provider_bytes(malformed)
        else {
            panic!("malformed Gemini function call should fail before stream commit");
        };

        assert_eq!(status_code, 502);
        assert_eq!(body_json["error"]["code"], "MALFORMED_FUNCTION_CALL");
        assert_eq!(
            body_json["error"]["message"],
            "Malformed function call: Function call is empty - no input to parse."
        );
        assert_eq!(gate.state(), StreamCommitState::Terminal);
    }

    #[test]
    fn gemini_gate_detects_malformed_function_call_across_chunk_boundaries() {
        let malformed = b"data: {\"response\":{\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"thoughtSignature\":\"signature\",\"text\":\"\"}]},\"finishReason\":\"MALFORMED_FUNCTION_CALL\",\"finishMessage\":\"empty call\"}]}}\r\n\r\n";

        for split in 1..malformed.len() {
            let mut gate = StreamCommitGate::new(gemini_policy());
            let first_observation = gate.observe_provider_bytes(&malformed[..split]);
            if !matches!(
                first_observation,
                StreamPrecommitObservation::UpstreamError {
                    status_code: 502,
                    ..
                }
            ) {
                assert_eq!(first_observation, StreamPrecommitObservation::Pending);
                assert!(matches!(
                    gate.observe_provider_bytes(&malformed[split..]),
                    StreamPrecommitObservation::UpstreamError {
                        status_code: 502,
                        ..
                    }
                ));
            }
            assert_eq!(gate.state(), StreamCommitState::Terminal);
        }
    }

    #[test]
    fn gate_detects_anthropic_error_across_every_chunk_boundary() {
        let event = b"event: error\r\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"busy\"}}\r\n\r\n";
        for split in 1..event.len() {
            let mut gate = StreamCommitGate::new(native_anthropic_policy());
            let first_observation = gate.observe_provider_bytes(&event[..split]);
            if matches!(
                first_observation,
                StreamPrecommitObservation::UpstreamError {
                    status_code: 529,
                    ..
                }
            ) {
                assert_eq!(event[split - 1], b'\r');
            } else {
                assert_eq!(first_observation, StreamPrecommitObservation::Pending);
                assert!(matches!(
                    gate.observe_provider_bytes(&event[split..]),
                    StreamPrecommitObservation::UpstreamError {
                        status_code: 529,
                        ..
                    }
                ));
            }
            assert_eq!(gate.state(), StreamCommitState::Terminal);
        }
    }

    #[test]
    fn gate_detects_cr_only_and_mixed_line_ending_errors() {
        for event in [
            "event: error\rdata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\"}}\r\r",
            "event: error\r\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\"}}\n\r",
        ] {
            for split in 1..event.len() {
                let mut gate = StreamCommitGate::new(native_anthropic_policy());
                assert_eq!(
                    gate.observe_provider_bytes(&event.as_bytes()[..split]),
                    StreamPrecommitObservation::Pending,
                    "gate committed before complete mixed-line event at split {split}",
                );
                assert!(matches!(
                    gate.observe_provider_bytes(&event.as_bytes()[split..]),
                    StreamPrecommitObservation::UpstreamError {
                        status_code: 529,
                        ..
                    }
                ));
            }
        }
    }

    #[test]
    fn unknown_and_ping_events_do_not_commit_before_anthropic_error() {
        let mut gate = StreamCommitGate::new(native_anthropic_policy());
        assert_eq!(
            gate.observe_provider_bytes(
                b"event: future_event\ndata: {\"type\":\"future_event\",\"value\":1}\n\n"
            ),
            StreamPrecommitObservation::Pending
        );
        assert_eq!(
            gate.observe_provider_bytes(b"event: ping\ndata: {\"type\":\"ping\"}\n\n"),
            StreamPrecommitObservation::Pending
        );
        assert!(matches!(
            gate.observe_provider_bytes(
                b"event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"rate_limit_error\"}}\n\n"
            ),
            StreamPrecommitObservation::UpstreamError {
                status_code: 429,
                ..
            }
        ));
    }

    #[test]
    fn first_semantic_event_commits_before_later_error_in_same_chunk() {
        let mut gate = StreamCommitGate::new(native_anthropic_policy());
        let observation = gate.observe_provider_bytes(
            concat!(
                "event: content_block_delta\n",
                "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"hello\"}}\n\n",
                "event: error\n",
                "data: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\"}}\n\n",
            )
            .as_bytes(),
        );

        assert_eq!(observation, StreamPrecommitObservation::Commit);
        assert_eq!(gate.state(), StreamCommitState::Committed);
    }

    #[test]
    fn transport_fragment_count_does_not_commit_an_incomplete_anthropic_error() {
        let policy = StreamCommitPolicy::FirstAnthropicSemanticEvent {
            max_bytes: 1024,
            max_wait: Duration::from_millis(750),
        };
        let mut gate = StreamCommitGate::new(policy);
        let event = b"event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\"}}\n\n";
        for byte in &event[..event.len() - 1] {
            assert_eq!(
                gate.observe_provider_bytes(std::slice::from_ref(byte)),
                StreamPrecommitObservation::Pending,
            );
        }
        assert!(matches!(
            gate.observe_provider_bytes(&event[event.len() - 1..]),
            StreamPrecommitObservation::UpstreamError {
                status_code: 529,
                ..
            }
        ));
    }

    #[test]
    fn anthropic_error_status_mapping_matches_messages_api_taxonomy() {
        for (error_type, status_code) in [
            ("invalid_request_error", 400),
            ("authentication_error", 401),
            ("permission_error", 403),
            ("not_found_error", 404),
            ("request_too_large", 413),
            ("rate_limit_error", 429),
            ("overloaded_error", 529),
            ("api_error", 500),
        ] {
            let body = serde_json::json!({
                "type": "error",
                "error": { "type": error_type, "message": "upstream failure" }
            });
            assert_eq!(
                anthropic_error_status_code(&body),
                status_code,
                "unexpected status for {error_type}"
            );
        }
    }
}
