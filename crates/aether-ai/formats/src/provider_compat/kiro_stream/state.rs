use std::collections::BTreeMap;

const MAX_MESSAGE_SIZE: usize = 16 * 1024 * 1024;
const MAX_BUFFER_SIZE: usize = MAX_MESSAGE_SIZE;
const MAX_ERRORS: usize = 5;

#[derive(Default)]
pub struct KiroToClaudeCliStreamState {
    decoder: EventStreamDecoder,
    state: KiroClaudeStreamState,
    started: bool,
}

#[derive(Default)]
struct KiroClaudeStreamState {
    model: String,
    thinking_enabled: bool,
    estimated_input_tokens: usize,
    cache_usage: Option<super::KiroStreamCacheUsage>,
    message_id: String,
    output_tokens: usize,
    context_input_tokens: Option<usize>,
    next_block_index: usize,
    open_blocks: BTreeMap<usize, String>,
    text_block_index: Option<usize>,
    thinking_block_index: Option<usize>,
    tool_block_indices: BTreeMap<String, usize>,
    thinking_buffer: String,
    in_thinking_block: bool,
    thinking_extracted: bool,
    strip_thinking_leading_newline: bool,
    has_tool_use: bool,
    stop_reason_override: Option<String>,
    had_error: bool,
    // 原生 reasoning 的签名可能先于正文到达；只允许原生 thinking 块消费它。
    pending_thinking_signature: Option<String>,
    // 标签式 `<thinking>` 与 Kiro 原生 reasoning 共享 Claude thinking 块，必须显式
    // 区分来源，避免标签式兼容签名误消费上游的真实签名。
    native_thinking_block_open: bool,
    // Kiro 的 meteringEvent 只表示 credit，不能套用到 token 计数。
    metering: Option<super::KiroStreamMetering>,
}

#[derive(Default)]
struct EventStreamDecoder {
    buffer: Vec<u8>,
    error_count: usize,
    stopped: bool,
}

#[derive(Default)]
struct AwsHeaders {
    values: BTreeMap<String, AwsHeaderValue>,
}

enum AwsHeaderValue {
    Ignored,
    String(String),
}

struct AwsEventFrame {
    headers: AwsHeaders,
    payload: Vec<u8>,
}

enum FrameParseError {
    Incomplete,
    Invalid(String),
}

#[path = "stream/decoder.rs"]
mod decoder;
#[path = "stream/state.rs"]
mod stream_state;

#[cfg(test)]
#[path = "stream/tests.rs"]
mod tests;
