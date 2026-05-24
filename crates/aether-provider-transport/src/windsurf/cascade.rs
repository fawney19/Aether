use std::fmt;

use super::proto::{
    encode_varint, get_all_fields, get_field, get_string, get_varint, parse_fields,
    write_bool_field, write_message_field, write_string_field, write_varint_field, ProtoError,
    WireType,
};
use serde_json::{json, Value};

const DEFAULT_CLIENT_VERSION: &str = "2.0.67";
const TOOL_REINFORCEMENT: &str = r#"The functions listed above are available and callable. When the user's request can be answered by calling a function, emit a <tool_call> block as described. Use this exact format: <tool_call>{"name":"...","arguments":{...}}</tool_call>"#;
const COMMUNICATION_WITH_TOOLS: &str = "You are accessed via API. When asked about your identity, describe your actual underlying model name and provider accurately. STRICTLY respond in the exact same language the user used in their latest message (Chinese -> Chinese, English -> English, Japanese -> Japanese; never switch mid-conversation). Use the functions above when relevant.";
const COMMUNICATION_NO_TOOLS: &str = "You are accessed via API. When asked about your identity, describe your actual underlying model name and provider accurately. Answer directly. STRICTLY respond in the exact same language the user used in their latest message (Chinese -> Chinese, English -> English, Japanese -> Japanese; never switch mid-conversation).";
const NO_TOOL_ADDITIONAL_PROMPT: &str = r#"CRITICAL OPERATING CONSTRAINT - READ BEFORE ANY RESPONSE:
You are being accessed as a plain chat API. You have NO tools, NO file access, NO shell, NO code execution, NO repository awareness, NO ability to list or read anything on the user's machine or any sandbox. You cannot "check", "look at", "open", "view", "inspect", "run", "glob", "grep", "list", or "edit" anything.

OUTPUT RULES:
1. Never narrate tool-like actions ("Let me check X", "I'll look at Y", "Looking at the file...", "I see in main.py...", "Based on the codebase...").
2. Never reference file paths, directory structures, line numbers, or repository contents that were not explicitly pasted into the current conversation by the user.
3. If the user asks about their code or project but hasn't pasted the relevant file content, respond: "I don't see that file in our conversation - please paste it and I'll help." Do NOT invent file contents.
4. For general questions, answer directly from your training knowledge. No preambles.
5. Match the user's language (Chinese -> Chinese, English -> English; never switch mid-conversation).

Violating these rules will produce broken output for the end user. Stay in chat-API mode at all times."#;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CascadeStep {
    pub step_type: u64,
    pub status: u64,
    pub text: String,
    pub response_text: String,
    pub modified_text: String,
    pub thinking: String,
    pub error_text: String,
    pub native_tool: Option<CascadeNativeToolStep>,
    pub usage: Option<CascadeUsage>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CascadeNativeToolStep {
    pub kind: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CascadeUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_write_tokens: u64,
    pub cache_read_tokens: u64,
    pub entry_count: u64,
}

impl CascadeUsage {
    fn has_signal(&self) -> bool {
        self.input_tokens > 0
            || self.output_tokens > 0
            || self.cache_write_tokens > 0
            || self.cache_read_tokens > 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CascadeImage {
    pub base64_data: String,
    pub mime_type: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SendCascadeMessageOptions {
    pub tool_preamble: Option<String>,
    pub images: Vec<CascadeImage>,
    pub additional_steps: Vec<Vec<u8>>,
    pub native_mode: bool,
    pub native_allowlist: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CascadeBuildError {
    message: String,
}

impl CascadeBuildError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for CascadeBuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CascadeBuildError {}

pub fn build_metadata(api_key: &str, session_id: &str) -> Vec<u8> {
    [
        write_string_field(1, "windsurf"),
        write_string_field(2, DEFAULT_CLIENT_VERSION),
        write_string_field(3, api_key),
        write_string_field(4, "en"),
        write_string_field(5, current_os_label()),
        write_string_field(7, DEFAULT_CLIENT_VERSION),
        write_string_field(8, current_arch_label()),
        write_varint_field(9, metadata_request_id(session_id)),
        write_string_field(10, session_id),
        write_string_field(12, "windsurf"),
    ]
    .concat()
}

fn metadata_request_id(seed: &str) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in seed.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash & 0x0000_ffff_ffff_ffff
}

fn current_os_label() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "linux"
    }
}

fn current_arch_label() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "x86_64"
    }
}

pub fn build_initialize_panel_state_request(
    api_key: &str,
    session_id: &str,
    trusted: bool,
) -> Vec<u8> {
    [
        write_message_field(1, &build_metadata(api_key, session_id)),
        write_bool_field(3, trusted),
    ]
    .concat()
}

pub fn build_heartbeat_request(api_key: &str, session_id: &str) -> Vec<u8> {
    write_message_field(1, &build_metadata(api_key, session_id))
}

pub fn build_get_user_status_request(api_key: &str, session_id: &str) -> Vec<u8> {
    write_message_field(1, &build_metadata(api_key, session_id))
}

pub fn build_update_panel_state_with_user_status_request(
    api_key: &str,
    session_id: &str,
    user_status_bytes: &[u8],
) -> Vec<u8> {
    let mut out = write_message_field(1, &build_metadata(api_key, session_id));
    if !user_status_bytes.is_empty() {
        out.extend(write_message_field(2, user_status_bytes));
    }
    out
}

pub fn build_add_tracked_workspace_request(workspace_path: &str) -> Vec<u8> {
    write_string_field(1, workspace_path)
}

pub fn build_update_workspace_trust_request(
    api_key: &str,
    session_id: &str,
    trusted: bool,
) -> Vec<u8> {
    [
        write_message_field(1, &build_metadata(api_key, session_id)),
        write_bool_field(2, trusted),
    ]
    .concat()
}

pub fn build_start_cascade_request(api_key: &str, session_id: &str) -> Vec<u8> {
    [
        write_message_field(1, &build_metadata(api_key, session_id)),
        write_varint_field(4, 1),
        write_varint_field(5, 1),
    ]
    .concat()
}

pub fn build_send_cascade_message_request(
    api_key: &str,
    cascade_id: &str,
    text: &str,
    model_enum: u32,
    model_uid: Option<&str>,
    session_id: &str,
) -> Result<Vec<u8>, CascadeBuildError> {
    build_send_cascade_message_request_with_options(
        api_key,
        cascade_id,
        text,
        model_enum,
        model_uid,
        session_id,
        &SendCascadeMessageOptions::default(),
    )
}

pub fn build_send_cascade_message_request_with_options(
    api_key: &str,
    cascade_id: &str,
    text: &str,
    model_enum: u32,
    model_uid: Option<&str>,
    session_id: &str,
    options: &SendCascadeMessageOptions,
) -> Result<Vec<u8>, CascadeBuildError> {
    if model_enum == 0 && model_uid.unwrap_or_default().trim().is_empty() {
        return Err(CascadeBuildError::new(
            "windsurf cascade model enum or model uid is required",
        ));
    }
    let item = write_string_field(1, text);
    let mut out = [
        write_string_field(1, cascade_id),
        write_message_field(2, &item),
        write_message_field(3, &build_metadata(api_key, session_id)),
        write_message_field(5, &build_cascade_config(model_enum, model_uid, options)?),
    ]
    .concat();
    for image in &options.images {
        let base64_data = image.base64_data.trim();
        if base64_data.is_empty() {
            continue;
        }
        let mime_type = if image.mime_type.trim().is_empty() {
            "image/png"
        } else {
            image.mime_type.trim()
        };
        let image_message = [
            write_string_field(1, base64_data),
            write_string_field(2, mime_type),
        ]
        .concat();
        out.extend(write_message_field(6, &image_message));
    }
    for step in &options.additional_steps {
        if !step.is_empty() {
            out.extend(write_message_field(9, step));
        }
    }
    Ok(out)
}

fn build_cascade_config(
    model_enum: u32,
    model_uid: Option<&str>,
    options: &SendCascadeMessageOptions,
) -> Result<Vec<u8>, CascadeBuildError> {
    let model_uid = model_uid.map(str::trim).filter(|value| !value.is_empty());
    if model_enum == 0 && model_uid.is_none() {
        return Err(CascadeBuildError::new(
            "windsurf cascade config requires a model identifier",
        ));
    }

    let tool_preamble = options
        .tool_preamble
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let force_default =
        options.native_mode || (!options.images.is_empty() && tool_preamble.is_none());
    let planner_mode = if force_default { 1 } else { 3 };
    let mut conversational_config = Vec::new();
    conversational_config.extend(write_varint_field(4, planner_mode));
    if let Some(tool_preamble) = tool_preamble {
        let full_section = format!("{tool_preamble}\n\n{TOOL_REINFORCEMENT}");
        let additional_section = [
            write_varint_field(1, 1),
            write_string_field(2, &full_section),
        ]
        .concat();
        conversational_config.extend(write_message_field(12, &additional_section));
        let communication_section = [
            write_varint_field(1, 1),
            write_string_field(2, COMMUNICATION_WITH_TOOLS),
        ]
        .concat();
        conversational_config.extend(write_message_field(13, &communication_section));
    } else {
        let no_tool_section = [
            write_varint_field(1, 1),
            write_string_field(2, "No tools are available."),
        ]
        .concat();
        conversational_config.extend(write_message_field(10, &no_tool_section));
        let additional_section = [
            write_varint_field(1, 1),
            write_string_field(2, NO_TOOL_ADDITIONAL_PROMPT),
        ]
        .concat();
        conversational_config.extend(write_message_field(12, &additional_section));
        let communication_section = [
            write_varint_field(1, 1),
            write_string_field(2, COMMUNICATION_NO_TOOLS),
        ]
        .concat();
        conversational_config.extend(write_message_field(13, &communication_section));
    }

    let mut planner_config = Vec::new();
    planner_config.extend(write_message_field(2, &conversational_config));
    if let Some(model_uid) = model_uid {
        planner_config.extend(write_string_field(35, model_uid));
        planner_config.extend(write_string_field(34, model_uid));
    }
    if model_enum > 0 {
        planner_config.extend(write_message_field(
            15,
            &write_varint_field(1, u64::from(model_enum)),
        ));
        planner_config.extend(write_varint_field(1, u64::from(model_enum)));
    }
    planner_config.extend(write_varint_field(6, 32768));
    if tool_preamble.is_none() {
        let empty_section = [write_varint_field(1, 1), write_string_field(2, "")].concat();
        planner_config.extend(write_message_field(11, &empty_section));
    }
    if options.native_mode {
        planner_config.extend(write_message_field(
            13,
            &build_native_cascade_tool_config(&options.native_allowlist),
        ));
    }

    let memory_config = write_varint_field(1, 0);
    let brain_config = [
        write_varint_field(1, 1),
        write_message_field(6, &write_len_field_allow_empty(6, &[])),
    ]
    .concat();

    Ok([
        write_message_field(1, &planner_config),
        write_message_field(5, &memory_config),
        write_message_field(7, &brain_config),
    ]
    .concat())
}

fn build_native_cascade_tool_config(allowlist: &[String]) -> Vec<u8> {
    let default_tools = [
        "view_file",
        "run_command",
        "grep_search_v2",
        "find",
        "list_dir",
    ];
    let list = if allowlist.is_empty() {
        default_tools
            .into_iter()
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>()
    } else {
        allowlist
            .iter()
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>()
    };
    let contains = |name: &str| list.iter().any(|value| value == name);
    let mut out = Vec::new();
    if contains("run_command") {
        out.extend(write_len_field_allow_empty(8, &[]));
    }
    if contains("view_file") {
        out.extend(write_len_field_allow_empty(10, &[]));
    }
    if contains("list_dir") || contains("list_directory") {
        out.extend(write_len_field_allow_empty(19, &[]));
    }
    if contains("grep_search_v2") || contains("grep_search") {
        out.extend(write_len_field_allow_empty(33, &[]));
    }
    if contains("find") {
        out.extend(write_len_field_allow_empty(5, &[]));
    }
    for name in list {
        out.extend(write_string_field(32, &name));
    }
    out
}

fn write_len_field_allow_empty(field: u32, value: &[u8]) -> Vec<u8> {
    let mut out = encode_varint((u64::from(field) << 3) | WireType::Len as u64);
    out.extend(encode_varint(value.len() as u64));
    out.extend(value);
    out
}

pub fn build_get_trajectory_steps_request(cascade_id: &str, offset: u64) -> Vec<u8> {
    let mut out = write_string_field(1, cascade_id);
    if offset > 0 {
        out.extend(write_varint_field(2, offset));
    }
    out
}

pub fn build_get_trajectory_request(cascade_id: &str) -> Vec<u8> {
    write_string_field(1, cascade_id)
}

pub fn build_get_generator_metadata_request(cascade_id: &str, offset: u64) -> Vec<u8> {
    let mut out = write_string_field(1, cascade_id);
    if offset > 0 {
        out.extend(write_varint_field(2, offset));
    }
    out
}

pub fn parse_start_cascade_response(buf: &[u8]) -> Option<String> {
    parse_fields(buf)
        .ok()
        .and_then(|fields| get_string(&fields, 1))
}

pub fn extract_user_status_bytes(buf: &[u8]) -> Option<Vec<u8>> {
    parse_fields(buf)
        .ok()
        .and_then(|fields| {
            get_field(&fields, 1, Some(WireType::Len)).map(|field| field.bytes().to_vec())
        })
        .filter(|bytes| !bytes.is_empty())
}

pub fn parse_trajectory_status(buf: &[u8]) -> Option<u64> {
    parse_fields(buf)
        .ok()
        .and_then(|fields| get_varint(&fields, 2))
}

pub fn parse_trajectory_steps(buf: &[u8]) -> Result<Vec<CascadeStep>, ProtoError> {
    let fields = parse_fields(buf)?;
    let mut out = Vec::new();
    for step in get_all_fields(&fields, 1)
        .into_iter()
        .filter(|field| field.wire_type == WireType::Len)
    {
        let step_fields = parse_fields(step.bytes())?;
        let step_type = get_varint(&step_fields, 1).unwrap_or_default();
        let status = get_varint(&step_fields, 4).unwrap_or_default();
        let mut parsed = CascadeStep {
            step_type,
            status,
            text: String::new(),
            response_text: String::new(),
            modified_text: String::new(),
            thinking: String::new(),
            error_text: String::new(),
            native_tool: parse_native_tool_step(&step_fields, step_type)?,
            usage: parse_step_usage(&step_fields)?,
        };

        if let Some(planner) = get_field(&step_fields, 20, Some(WireType::Len)) {
            let planner_fields = parse_fields(planner.bytes())?;
            parsed.response_text = get_string(&planner_fields, 1).unwrap_or_default();
            parsed.thinking = get_string(&planner_fields, 3).unwrap_or_default();
            parsed.modified_text = get_string(&planner_fields, 8).unwrap_or_default();
            parsed.text = if parsed.modified_text.is_empty() {
                parsed.response_text.clone()
            } else {
                parsed.modified_text.clone()
            };
        }

        parsed.error_text = parse_step_error_text(&step_fields)?;
        out.push(parsed);
    }
    Ok(out)
}

fn parse_step_usage(fields: &[super::proto::Field]) -> Result<Option<CascadeUsage>, ProtoError> {
    let Some(metadata) = get_field(fields, 5, Some(WireType::Len)) else {
        return Ok(None);
    };
    let metadata_fields = parse_fields(metadata.bytes())?;
    let Some(usage_field) = get_field(&metadata_fields, 9, Some(WireType::Len)) else {
        return Ok(None);
    };
    let usage_fields = parse_fields(usage_field.bytes())?;
    let usage = CascadeUsage {
        input_tokens: get_varint(&usage_fields, 2).unwrap_or_default(),
        output_tokens: get_varint(&usage_fields, 3).unwrap_or_default(),
        cache_write_tokens: get_varint(&usage_fields, 4).unwrap_or_default(),
        cache_read_tokens: get_varint(&usage_fields, 5).unwrap_or_default(),
        entry_count: 1,
    };
    Ok(usage.has_signal().then_some(usage))
}

pub fn parse_generator_metadata(buf: &[u8]) -> Result<Option<CascadeUsage>, ProtoError> {
    let fields = parse_fields(buf)?;
    let entries = get_all_fields(&fields, 1)
        .into_iter()
        .filter(|field| field.wire_type == WireType::Len)
        .collect::<Vec<_>>();
    if entries.is_empty() {
        return Ok(None);
    }

    let mut usage = CascadeUsage {
        entry_count: entries.len() as u64,
        ..CascadeUsage::default()
    };
    for entry in entries {
        let generator_fields = parse_fields(entry.bytes())?;
        let Some(chat_model) = get_field(&generator_fields, 1, Some(WireType::Len)) else {
            continue;
        };
        let chat_model_fields = parse_fields(chat_model.bytes())?;
        let Some(usage_field) = get_field(&chat_model_fields, 4, Some(WireType::Len)) else {
            continue;
        };
        let usage_fields = parse_fields(usage_field.bytes())?;
        usage.input_tokens = usage
            .input_tokens
            .saturating_add(get_varint(&usage_fields, 2).unwrap_or_default());
        usage.output_tokens = usage
            .output_tokens
            .saturating_add(get_varint(&usage_fields, 3).unwrap_or_default());
        usage.cache_write_tokens = usage
            .cache_write_tokens
            .saturating_add(get_varint(&usage_fields, 4).unwrap_or_default());
        usage.cache_read_tokens = usage
            .cache_read_tokens
            .saturating_add(get_varint(&usage_fields, 5).unwrap_or_default());
    }

    Ok(usage.has_signal().then_some(usage))
}

pub fn build_additional_step(kind: &str, args: &Value) -> Option<Vec<u8>> {
    let meta = cascade_step_meta(kind)?;
    let body = build_native_step_body(kind, args)?;
    let mut out = Vec::new();
    out.extend(write_varint_field(1, meta.type_enum));
    out.extend(write_varint_field(4, 3));
    out.extend(write_len_field_allow_empty(meta.oneof_field, &body));
    Some(out)
}

#[derive(Debug, Clone, Copy)]
struct CascadeStepMeta {
    type_enum: u64,
    oneof_field: u32,
}

fn cascade_step_meta(kind: &str) -> Option<CascadeStepMeta> {
    let (type_enum, oneof_field) = match kind {
        "grep_search" => (13, 13),
        "view_file" => (14, 14),
        "list_directory" | "list_dir" => (15, 15),
        "write_to_file" => (23, 23),
        "run_command" => (28, 28),
        "propose_code" => (32, 32),
        "find" => (34, 34),
        "read_url_content" => (40, 40),
        "search_web" => (42, 42),
        "grep_search_v2" => (105, 105),
        _ => return None,
    };
    Some(CascadeStepMeta {
        type_enum,
        oneof_field,
    })
}

fn native_kind_for_step_type(step_type: u64) -> Option<&'static str> {
    match step_type {
        13 => Some("grep_search"),
        14 => Some("view_file"),
        15 => Some("list_directory"),
        23 => Some("write_to_file"),
        28 => Some("run_command"),
        32 => Some("propose_code"),
        34 => Some("find"),
        40 => Some("read_url_content"),
        42 => Some("search_web"),
        105 => Some("grep_search_v2"),
        _ => None,
    }
}

fn build_native_step_body(kind: &str, args: &Value) -> Option<Vec<u8>> {
    match kind {
        "view_file" => Some(build_view_file_body(args)),
        "run_command" => Some(build_run_command_body(args)),
        "grep_search" | "grep_search_v2" => Some(build_grep_search_v2_body(args)),
        "find" => Some(build_find_body(args)),
        "list_directory" | "list_dir" => Some(build_list_directory_body(args)),
        "write_to_file" => Some(build_write_to_file_body(args)),
        "propose_code" => Some(build_propose_code_body(args)),
        "search_web" => Some(build_search_web_body(args)),
        "read_url_content" => Some(build_read_url_content_body(args)),
        _ => None,
    }
}

fn build_view_file_body(args: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    if let Some(value) = json_str(args, "absolute_path_uri") {
        out.extend(write_string_field(1, value));
    }
    if let Some(value) = json_u64(args, "offset") {
        out.extend(write_varint_field(11, value));
    }
    if let Some(value) = json_u64(args, "limit") {
        out.extend(write_varint_field(12, value));
    }
    if let Some(value) = json_str(args, "content") {
        out.extend(write_string_field(4, value));
    }
    out
}

fn build_run_command_body(args: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    if let Some(value) = json_str(args, "command_line") {
        out.extend(write_string_field(23, value));
    }
    if let Some(value) = json_str(args, "cwd") {
        out.extend(write_string_field(2, value));
    }
    if json_bool(args, "blocking") {
        out.extend(write_bool_field(11, true));
    }
    if let Some(value) = json_str(args, "stdout") {
        out.extend(write_string_field(4, value));
    }
    if let Some(value) = json_str(args, "stderr") {
        out.extend(write_string_field(5, value));
    }
    if let Some(value) = json_u64(args, "exit_code") {
        out.extend(write_varint_field(6, value));
    }
    if let Some(value) = json_str(args, "full_output") {
        let inner = write_string_field(1, value);
        out.extend(write_message_field(21, &inner));
    }
    out
}

fn build_grep_search_v2_body(args: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    for (field, key) in [
        (2, "pattern"),
        (3, "path"),
        (4, "glob"),
        (5, "output_mode"),
        (11, "type"),
        (15, "raw_output"),
    ] {
        if let Some(value) = json_str(args, key) {
            out.extend(write_string_field(field, value));
        }
    }
    for (field, key) in [
        (6, "lines_after"),
        (7, "lines_before"),
        (8, "lines_both"),
        (12, "head_limit"),
    ] {
        if let Some(value) = json_u64(args, key) {
            out.extend(write_varint_field(field, value));
        }
    }
    if json_bool(args, "case_insensitive") {
        out.extend(write_bool_field(10, true));
    }
    if json_bool(args, "multiline") {
        out.extend(write_bool_field(13, true));
    }
    out
}

fn build_find_body(args: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    if let Some(value) = json_str(args, "pattern") {
        out.extend(write_string_field(1, value));
    }
    if let Some(value) = json_str(args, "search_directory") {
        out.extend(write_string_field(10, value));
    }
    if let Some(value) = json_str(args, "raw_output") {
        out.extend(write_string_field(11, value));
    }
    out
}

fn build_list_directory_body(args: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    if let Some(value) = json_str(args, "directory_path_uri") {
        out.extend(write_string_field(1, value));
    }
    if let Some(children) = args.get("children").and_then(Value::as_array) {
        for child in children.iter().filter_map(Value::as_str) {
            out.extend(write_string_field(2, child));
        }
    }
    out
}

fn build_write_to_file_body(args: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    if let Some(value) = json_str(args, "target_file_uri") {
        out.extend(write_string_field(1, value));
    }
    if let Some(lines) = args.get("code_content").and_then(Value::as_array) {
        for line in lines.iter().filter_map(Value::as_str) {
            out.extend(write_string_field(2, line));
        }
    }
    if json_bool(args, "file_created") {
        out.extend(write_bool_field(4, true));
    }
    out
}

fn build_propose_code_body(args: &Value) -> Vec<u8> {
    let mut command = Vec::new();
    if let Some(value) = json_str(args, "instruction") {
        command.extend(write_string_field(1, value));
    }
    command.extend(write_bool_field(2, true));
    if let Some(chunks) = args.get("replacement_chunks").and_then(Value::as_array) {
        for chunk in chunks {
            let mut chunk_body = Vec::new();
            if let Some(value) = json_str(chunk, "target") {
                chunk_body.extend(write_string_field(1, value));
            }
            if let Some(value) = json_str(chunk, "replacement") {
                chunk_body.extend(write_string_field(2, value));
            }
            if json_bool(chunk, "allow_multiple") {
                chunk_body.extend(write_bool_field(3, true));
            }
            command.extend(write_message_field(9, &chunk_body));
        }
    }
    if let Some(value) = json_str(args, "target_file_uri") {
        command.extend(write_message_field(4, &write_string_field(5, value)));
    }
    let action_spec = write_message_field(1, &command);
    let mut out = write_message_field(1, &action_spec);
    if let Some(value) = json_str(args, "instruction") {
        out.extend(write_string_field(3, value));
    }
    out
}

fn build_search_web_body(args: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    if let Some(value) = json_str(args, "query") {
        out.extend(write_string_field(1, value));
    }
    if let Some(value) = json_str(args, "domain") {
        out.extend(write_string_field(3, value));
    }
    if let Some(value) = json_str(args, "summary") {
        out.extend(write_string_field(5, value));
    }
    out
}

fn build_read_url_content_body(args: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    if let Some(value) = json_str(args, "url") {
        out.extend(write_string_field(1, value));
    }
    if let Some(value) = json_str(args, "summary") {
        out.extend(write_string_field(5, value));
    }
    out
}

fn parse_native_tool_step(
    fields: &[super::proto::Field],
    step_type: u64,
) -> Result<Option<CascadeNativeToolStep>, ProtoError> {
    let Some(kind) = native_kind_for_step_type(step_type) else {
        return Ok(None);
    };
    let Some(meta) = cascade_step_meta(kind) else {
        return Ok(None);
    };
    let Some(body) = get_field(fields, meta.oneof_field, Some(WireType::Len)) else {
        return Ok(None);
    };
    let body_fields = parse_fields(body.bytes())?;
    let arguments = decode_native_tool_arguments(kind, &body_fields)?;
    Ok(Some(CascadeNativeToolStep {
        kind: kind.to_string(),
        arguments,
    }))
}

fn decode_native_tool_arguments(
    kind: &str,
    fields: &[super::proto::Field],
) -> Result<Value, ProtoError> {
    let value = match kind {
        "view_file" => json!({
            "absolute_path_uri": get_string(fields, 1).unwrap_or_default(),
            "offset": get_varint(fields, 11).unwrap_or_default(),
            "limit": get_varint(fields, 12).unwrap_or_default(),
            "content": get_string(fields, 4).unwrap_or_default(),
            "raw_content": get_string(fields, 9).unwrap_or_default(),
        }),
        "run_command" => {
            let mut full_output = String::new();
            if let Some(combined) = get_field(fields, 21, Some(WireType::Len)) {
                let combined_fields = parse_fields(combined.bytes())?;
                full_output = get_string(&combined_fields, 1).unwrap_or_default();
            }
            json!({
                "command_line": get_string(fields, 23).or_else(|| get_string(fields, 1)).unwrap_or_default(),
                "proposed_command_line": get_string(fields, 25).unwrap_or_default(),
                "cwd": get_string(fields, 2).unwrap_or_default(),
                "stdout": get_string(fields, 4).unwrap_or_default(),
                "stderr": get_string(fields, 5).unwrap_or_default(),
                "exit_code": get_varint(fields, 6).unwrap_or_default(),
                "full_output": full_output,
            })
        }
        "grep_search" | "grep_search_v2" => json!({
            "pattern": get_string(fields, 2).unwrap_or_default(),
            "path": get_string(fields, 3).unwrap_or_default(),
            "glob": get_string(fields, 4).unwrap_or_default(),
            "output_mode": get_string(fields, 5).unwrap_or_default(),
            "lines_after": get_varint(fields, 6).unwrap_or_default(),
            "lines_before": get_varint(fields, 7).unwrap_or_default(),
            "lines_both": get_varint(fields, 8).unwrap_or_default(),
            "case_insensitive": get_varint(fields, 10).unwrap_or_default() != 0,
            "type": get_string(fields, 11).unwrap_or_default(),
            "head_limit": get_varint(fields, 12).unwrap_or_default(),
            "multiline": get_varint(fields, 13).unwrap_or_default() != 0,
            "raw_output": get_string(fields, 15).unwrap_or_default(),
        }),
        "find" => json!({
            "pattern": get_string(fields, 1).unwrap_or_default(),
            "search_directory": get_string(fields, 10).unwrap_or_default(),
            "raw_output": get_string(fields, 11).unwrap_or_default(),
        }),
        "list_directory" | "list_dir" => {
            let children = get_all_fields(fields, 2)
                .into_iter()
                .filter(|field| field.wire_type == WireType::Len)
                .filter_map(|field| String::from_utf8(field.bytes().to_vec()).ok())
                .collect::<Vec<_>>();
            json!({
                "directory_path_uri": get_string(fields, 1).unwrap_or_default(),
                "children": children,
            })
        }
        "write_to_file" => {
            let code_content = get_all_fields(fields, 2)
                .into_iter()
                .filter(|field| field.wire_type == WireType::Len)
                .filter_map(|field| String::from_utf8(field.bytes().to_vec()).ok())
                .collect::<Vec<_>>();
            json!({
                "target_file_uri": get_string(fields, 1).unwrap_or_default(),
                "code_content": code_content,
                "file_created": get_varint(fields, 4).unwrap_or_default() != 0,
            })
        }
        "propose_code" => decode_propose_code_arguments(fields)?,
        "search_web" => json!({
            "query": get_string(fields, 1).unwrap_or_default(),
            "domain": get_string(fields, 3).unwrap_or_default(),
            "summary": get_string(fields, 5).unwrap_or_default(),
        }),
        "read_url_content" => json!({
            "url": get_string(fields, 1).unwrap_or_default(),
            "summary": get_string(fields, 5).unwrap_or_default(),
        }),
        _ => json!({}),
    };
    Ok(value)
}

fn decode_propose_code_arguments(fields: &[super::proto::Field]) -> Result<Value, ProtoError> {
    let mut target_file_uri = String::new();
    let mut instruction = String::new();
    let mut replacement_chunks = Vec::new();
    if let Some(action_spec) = get_field(fields, 1, Some(WireType::Len)) {
        let action_fields = parse_fields(action_spec.bytes())?;
        if let Some(command) = get_field(&action_fields, 1, Some(WireType::Len)) {
            let command_fields = parse_fields(command.bytes())?;
            instruction = get_string(&command_fields, 1).unwrap_or_default();
            if let Some(file_target) = get_field(&command_fields, 4, Some(WireType::Len)) {
                let target_fields = parse_fields(file_target.bytes())?;
                target_file_uri = get_string(&target_fields, 5)
                    .or_else(|| get_string(&target_fields, 1))
                    .unwrap_or_default();
            }
            for chunk in get_all_fields(&command_fields, 9)
                .into_iter()
                .filter(|field| field.wire_type == WireType::Len)
            {
                let chunk_fields = parse_fields(chunk.bytes())?;
                replacement_chunks.push(json!({
                    "target": get_string(&chunk_fields, 1).unwrap_or_default(),
                    "replacement": get_string(&chunk_fields, 2).unwrap_or_default(),
                    "allow_multiple": get_varint(&chunk_fields, 3).unwrap_or_default() != 0,
                }));
            }
        }
    }
    Ok(json!({
        "target_file_uri": target_file_uri,
        "replacement_chunks": replacement_chunks,
        "instruction": instruction,
    }))
}

fn json_str<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

fn json_u64(value: &Value, key: &str) -> Option<u64> {
    value.get(key).and_then(Value::as_u64)
}

fn json_bool(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn parse_step_error_text(fields: &[super::proto::Field]) -> Result<String, ProtoError> {
    if let Some(error_message) = get_field(fields, 24, Some(WireType::Len)) {
        let message_fields = parse_fields(error_message.bytes())?;
        if let Some(inner) = get_field(&message_fields, 3, Some(WireType::Len)) {
            let value = read_error_details(inner.bytes())?;
            if !value.is_empty() {
                return Ok(value);
            }
        }
    }
    if let Some(error) = get_field(fields, 31, Some(WireType::Len)) {
        return read_error_details(error.bytes());
    }
    Ok(String::new())
}

fn read_error_details(buf: &[u8]) -> Result<String, ProtoError> {
    let fields = parse_fields(buf)?;
    for number in [1, 2, 3] {
        if let Some(value) = get_string(&fields, number) {
            let value = value.trim();
            if !value.is_empty() {
                return Ok(value
                    .lines()
                    .next()
                    .unwrap_or(value)
                    .chars()
                    .take(300)
                    .collect());
            }
        }
    }
    Ok(String::new())
}

pub fn grpc_frame(payload: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(5 + payload.len());
    frame.push(0);
    frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    frame.extend_from_slice(payload);
    frame
}

pub fn extract_grpc_frames(buf: &[u8]) -> Vec<Vec<u8>> {
    let mut frames = Vec::new();
    let mut offset = 0usize;
    while offset + 5 <= buf.len() {
        let compressed = buf[offset];
        let len = u32::from_be_bytes([
            buf[offset + 1],
            buf[offset + 2],
            buf[offset + 3],
            buf[offset + 4],
        ]) as usize;
        if compressed != 0 || offset + 5 + len > buf.len() {
            break;
        }
        frames.push(buf[offset + 5..offset + 5 + len].to_vec());
        offset += 5 + len;
    }
    frames
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::windsurf::proto::{
        get_all_fields, get_field, get_string, get_varint, parse_fields, WireType,
    };

    #[test]
    fn builds_start_cascade_request_with_metadata_and_source_fields() {
        let request = build_start_cascade_request("api-key", "session-1");
        let fields = parse_fields(&request).expect("start request should parse");
        let metadata = get_field(&fields, 1, Some(WireType::Len)).expect("metadata field");
        let metadata_fields = parse_fields(metadata.bytes()).expect("metadata should parse");

        assert_eq!(get_string(&metadata_fields, 1).as_deref(), Some("windsurf"));
        assert_eq!(get_string(&metadata_fields, 3).as_deref(), Some("api-key"));
        assert_eq!(
            get_string(&metadata_fields, 10).as_deref(),
            Some("session-1")
        );
        assert_eq!(get_varint(&fields, 4), Some(1));
        assert_eq!(get_varint(&fields, 5), Some(1));
    }

    #[test]
    fn builds_send_user_message_with_model_uid_in_cascade_config() {
        let request = build_send_cascade_message_request(
            "api-key",
            "cascade-1",
            "hello",
            0,
            Some("gpt-5-5-low"),
            "session-1",
        )
        .expect("request should build");
        let fields = parse_fields(&request).expect("send request should parse");

        assert_eq!(get_string(&fields, 1).as_deref(), Some("cascade-1"));
        let item = get_field(&fields, 2, Some(WireType::Len)).expect("item field");
        let item_fields = parse_fields(item.bytes()).expect("item should parse");
        assert_eq!(get_string(&item_fields, 1).as_deref(), Some("hello"));

        let config = get_field(&fields, 5, Some(WireType::Len)).expect("config field");
        let config_fields = parse_fields(config.bytes()).expect("config should parse");
        let planner = get_field(&config_fields, 1, Some(WireType::Len)).expect("planner config");
        let planner_fields = parse_fields(planner.bytes()).expect("planner should parse");

        assert_eq!(
            get_string(&planner_fields, 35).as_deref(),
            Some("gpt-5-5-low")
        );
        assert_eq!(
            get_string(&planner_fields, 34).as_deref(),
            Some("gpt-5-5-low")
        );
        assert_eq!(get_varint(&planner_fields, 6), Some(32768));
        assert!(
            get_field(&config_fields, 5, Some(WireType::Len)).is_some(),
            "memory_config should be present"
        );
    }

    #[test]
    fn builds_no_tool_prompt_overrides_like_windsurfapi() {
        let request = build_send_cascade_message_request(
            "api-key",
            "cascade-1",
            "hello",
            0,
            Some("gpt-5-5-low"),
            "session-1",
        )
        .expect("request should build");

        let conversational_fields = conversational_fields_from_send_request(&request);
        assert_eq!(get_varint(&conversational_fields, 4), Some(3));

        let tool_section = section_override_string(&conversational_fields, 10);
        assert_eq!(tool_section.as_deref(), Some("No tools are available."));

        let additional = section_override_string(&conversational_fields, 12)
            .expect("additional instructions should be present");
        assert!(additional.contains("CRITICAL OPERATING CONSTRAINT"));
        assert!(additional.contains("NO file access"));

        let communication = section_override_string(&conversational_fields, 13)
            .expect("communication section should be present");
        assert!(communication.contains("Answer directly"));
        assert!(communication.contains("same language"));
    }

    #[test]
    fn builds_images_as_repeated_field_six() {
        let request = build_send_cascade_message_request_with_options(
            "api-key",
            "cascade-1",
            "describe",
            123,
            Some("MODEL_TEST"),
            "session-1",
            &SendCascadeMessageOptions {
                images: vec![
                    CascadeImage {
                        base64_data: "aaa".to_string(),
                        mime_type: "image/jpeg".to_string(),
                    },
                    CascadeImage {
                        base64_data: "bbb".to_string(),
                        mime_type: String::new(),
                    },
                ],
                ..SendCascadeMessageOptions::default()
            },
        )
        .expect("request should build");

        let fields = parse_fields(&request).expect("send request should parse");
        let images = get_all_fields(&fields, 6);
        assert_eq!(images.len(), 2);
        let first = parse_fields(images[0].bytes()).expect("image should parse");
        assert_eq!(get_string(&first, 1).as_deref(), Some("aaa"));
        assert_eq!(get_string(&first, 2).as_deref(), Some("image/jpeg"));
        let second = parse_fields(images[1].bytes()).expect("image should parse");
        assert_eq!(get_string(&second, 1).as_deref(), Some("bbb"));
        assert_eq!(get_string(&second, 2).as_deref(), Some("image/png"));

        let conversational_fields = conversational_fields_from_send_request(&request);
        assert_eq!(
            get_varint(&conversational_fields, 4),
            Some(1),
            "vision requests without tool emulation use DEFAULT planner mode"
        );
    }

    #[test]
    fn writes_additional_steps_to_field_nine() {
        let step_a = crate::windsurf::proto::write_string_field(1, "view_file");
        let step_b = crate::windsurf::proto::write_string_field(1, "run_command");
        let request = build_send_cascade_message_request_with_options(
            "api-key",
            "cascade-1",
            "continue",
            123,
            Some("MODEL_TEST"),
            "session-1",
            &SendCascadeMessageOptions {
                additional_steps: vec![step_a.clone(), Vec::new(), step_b.clone()],
                ..SendCascadeMessageOptions::default()
            },
        )
        .expect("request should build");

        let fields = parse_fields(&request).expect("send request should parse");
        let steps = get_all_fields(&fields, 9);
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].bytes(), step_a.as_slice());
        assert_eq!(steps[1].bytes(), step_b.as_slice());
    }

    #[test]
    fn builds_native_tool_steps_with_windsurfapi_field_numbers() {
        let search = build_additional_step(
            "search_web",
            &serde_json::json!({"query": "rust news", "domain": "example.com"}),
        )
        .expect("search_web should encode");
        let search_fields = parse_fields(&search).expect("search step should parse");
        assert_eq!(get_varint(&search_fields, 1), Some(42));
        assert_eq!(get_varint(&search_fields, 4), Some(3));
        let search_body = get_field(&search_fields, 42, Some(WireType::Len)).expect("search body");
        let search_body_fields = parse_fields(search_body.bytes()).expect("body should parse");
        assert_eq!(
            get_string(&search_body_fields, 1).as_deref(),
            Some("rust news")
        );
        assert_eq!(
            get_string(&search_body_fields, 3).as_deref(),
            Some("example.com")
        );

        let fetch = build_additional_step(
            "read_url_content",
            &serde_json::json!({"url": "https://example.com", "summary": "ok"}),
        )
        .expect("read_url_content should encode");
        let fetch_fields = parse_fields(&fetch).expect("fetch step should parse");
        assert_eq!(get_varint(&fetch_fields, 1), Some(40));
        let fetch_body = get_field(&fetch_fields, 40, Some(WireType::Len)).expect("fetch body");
        let fetch_body_fields = parse_fields(fetch_body.bytes()).expect("body should parse");
        assert_eq!(
            get_string(&fetch_body_fields, 1).as_deref(),
            Some("https://example.com")
        );
        assert_eq!(get_string(&fetch_body_fields, 5).as_deref(), Some("ok"));
    }

    #[test]
    fn parses_native_tool_step_arguments() {
        let step = build_additional_step(
            "run_command",
            &serde_json::json!({
                "command_line": "pwd",
                "cwd": "/tmp",
                "stdout": "/tmp\n",
                "exit_code": 0
            }),
        )
        .expect("run_command should encode");
        let response = crate::windsurf::proto::write_message_field(1, &step);
        let steps = parse_trajectory_steps(&response).expect("steps should parse");

        let native = steps[0].native_tool.as_ref().expect("native tool");
        assert_eq!(native.kind, "run_command");
        assert_eq!(native.arguments["command_line"], "pwd");
        assert_eq!(native.arguments["cwd"], "/tmp");
        assert_eq!(native.arguments["stdout"], "/tmp\n");
        assert_eq!(native.arguments["exit_code"], 0);
    }

    #[test]
    fn parse_trajectory_steps_extracts_step_usage() {
        let model_usage = [
            crate::windsurf::proto::write_varint_field(2, 11),
            crate::windsurf::proto::write_varint_field(3, 22),
            crate::windsurf::proto::write_varint_field(4, 33),
            crate::windsurf::proto::write_varint_field(5, 44),
        ]
        .concat();
        let step_metadata = crate::windsurf::proto::write_message_field(9, &model_usage);
        let step = [
            crate::windsurf::proto::write_varint_field(1, 15),
            crate::windsurf::proto::write_varint_field(4, 3),
            crate::windsurf::proto::write_message_field(5, &step_metadata),
        ]
        .concat();
        let response = crate::windsurf::proto::write_message_field(1, &step);

        let steps = parse_trajectory_steps(&response).expect("steps should parse");
        let usage = steps[0].usage.expect("step usage should parse");

        assert_eq!(usage.input_tokens, 11);
        assert_eq!(usage.output_tokens, 22);
        assert_eq!(usage.cache_write_tokens, 33);
        assert_eq!(usage.cache_read_tokens, 44);
        assert_eq!(usage.entry_count, 1);
    }

    #[test]
    fn parses_generator_metadata_usage_like_windsurfapi() {
        let usage_a = [
            crate::windsurf::proto::write_varint_field(2, 10),
            crate::windsurf::proto::write_varint_field(3, 20),
            crate::windsurf::proto::write_varint_field(4, 30),
            crate::windsurf::proto::write_varint_field(5, 40),
        ]
        .concat();
        let chat_model_a = crate::windsurf::proto::write_message_field(4, &usage_a);
        let entry_a = crate::windsurf::proto::write_message_field(1, &chat_model_a);
        let usage_b = [
            crate::windsurf::proto::write_varint_field(2, 1),
            crate::windsurf::proto::write_varint_field(3, 2),
            crate::windsurf::proto::write_varint_field(4, 3),
            crate::windsurf::proto::write_varint_field(5, 4),
        ]
        .concat();
        let chat_model_b = crate::windsurf::proto::write_message_field(4, &usage_b);
        let entry_b = crate::windsurf::proto::write_message_field(1, &chat_model_b);
        let response = [
            crate::windsurf::proto::write_message_field(1, &entry_a),
            crate::windsurf::proto::write_message_field(1, &entry_b),
        ]
        .concat();

        let usage = parse_generator_metadata(&response)
            .expect("metadata should parse")
            .expect("usage should be present");

        assert_eq!(usage.input_tokens, 11);
        assert_eq!(usage.output_tokens, 22);
        assert_eq!(usage.cache_write_tokens, 33);
        assert_eq!(usage.cache_read_tokens, 44);
        assert_eq!(usage.entry_count, 2);
    }

    #[test]
    fn builds_generator_metadata_request_with_offset() {
        let request = build_get_generator_metadata_request("cascade-1", 7);
        let fields = parse_fields(&request).expect("request should parse");

        assert_eq!(get_string(&fields, 1).as_deref(), Some("cascade-1"));
        assert_eq!(get_varint(&fields, 2), Some(7));
    }

    #[test]
    fn native_mode_switches_to_default_and_adds_tool_config() {
        let request = build_send_cascade_message_request_with_options(
            "api-key",
            "cascade-1",
            "hello",
            123,
            Some("MODEL_TEST"),
            "session-1",
            &SendCascadeMessageOptions {
                native_mode: true,
                native_allowlist: vec!["view_file".to_string(), "run_command".to_string()],
                ..SendCascadeMessageOptions::default()
            },
        )
        .expect("request should build");

        let planner_fields = planner_fields_from_send_request(&request);
        let tool_config = get_field(&planner_fields, 13, Some(WireType::Len))
            .expect("native mode should include tool config");
        let tool_fields = parse_fields(tool_config.bytes()).expect("tool config should parse");
        let allowlist = get_all_fields(&tool_fields, 32)
            .into_iter()
            .filter_map(|field| String::from_utf8(field.bytes().to_vec()).ok())
            .collect::<Vec<_>>();
        assert!(allowlist.contains(&"view_file".to_string()));
        assert!(allowlist.contains(&"run_command".to_string()));
        assert!(get_field(&tool_fields, 10, Some(WireType::Len)).is_some());
        assert!(get_field(&tool_fields, 8, Some(WireType::Len)).is_some());

        let conversational_fields = conversational_fields_from_send_request(&request);
        assert_eq!(get_varint(&conversational_fields, 4), Some(1));
    }

    #[test]
    fn tool_preamble_uses_additional_and_communication_sections_only() {
        let request = build_send_cascade_message_request_with_options(
            "api-key",
            "cascade-1",
            "hello",
            123,
            Some("MODEL_TEST"),
            "session-1",
            &SendCascadeMessageOptions {
                tool_preamble: Some("Tool definitions here.".to_string()),
                ..SendCascadeMessageOptions::default()
            },
        )
        .expect("request should build");

        let conversational_fields = conversational_fields_from_send_request(&request);
        assert_eq!(get_varint(&conversational_fields, 4), Some(3));
        assert!(
            get_field(&conversational_fields, 10, Some(WireType::Len)).is_none(),
            "tool preamble should not be duplicated into field 10"
        );
        let additional = section_override_string(&conversational_fields, 12)
            .expect("additional instructions should be present");
        assert!(additional.contains("Tool definitions here."));
        assert!(additional.contains("<tool_call>"));
        let communication = section_override_string(&conversational_fields, 13)
            .expect("communication section should be present");
        assert!(communication.contains("Use the functions above when relevant."));
    }

    #[test]
    fn builds_user_status_panel_sync_requests() {
        let request = build_get_user_status_request("api-key", "session-1");
        let fields = parse_fields(&request).expect("status request should parse");
        let metadata = get_field(&fields, 1, Some(WireType::Len)).expect("metadata field");
        let metadata_fields = parse_fields(metadata.bytes()).expect("metadata should parse");
        assert_eq!(get_string(&metadata_fields, 3).as_deref(), Some("api-key"));
        assert_eq!(
            get_string(&metadata_fields, 10).as_deref(),
            Some("session-1")
        );

        let user_status = crate::windsurf::proto::write_string_field(7, "user@example.com");
        let update =
            build_update_panel_state_with_user_status_request("api-key", "session-1", &user_status);
        let update_fields = parse_fields(&update).expect("panel update should parse");
        assert!(get_field(&update_fields, 1, Some(WireType::Len)).is_some());
        let embedded = get_field(&update_fields, 2, Some(WireType::Len))
            .expect("user status bytes should be embedded");
        assert_eq!(embedded.bytes(), user_status.as_slice());

        let response = crate::windsurf::proto::write_message_field(1, &user_status);
        assert_eq!(
            extract_user_status_bytes(&response).as_deref(),
            Some(user_status.as_slice())
        );
    }

    #[test]
    fn parses_start_status_and_planner_response_steps() {
        let start_response = crate::windsurf::proto::write_string_field(1, "cascade-1");
        assert_eq!(
            parse_start_cascade_response(&start_response).as_deref(),
            Some("cascade-1")
        );

        let status_response = crate::windsurf::proto::write_varint_field(2, 1);
        assert_eq!(parse_trajectory_status(&status_response), Some(1));

        let planner_response = [
            crate::windsurf::proto::write_string_field(1, "hello"),
            crate::windsurf::proto::write_string_field(3, "thinking"),
        ]
        .concat();
        let step = [
            crate::windsurf::proto::write_varint_field(1, 15),
            crate::windsurf::proto::write_varint_field(4, 3),
            crate::windsurf::proto::write_message_field(20, &planner_response),
        ]
        .concat();
        let steps_response = crate::windsurf::proto::write_message_field(1, &step);
        let steps = parse_trajectory_steps(&steps_response).expect("steps should parse");

        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].step_type, 15);
        assert_eq!(steps[0].status, 3);
        assert_eq!(steps[0].text, "hello");
        assert_eq!(steps[0].thinking, "thinking");
    }

    #[test]
    fn rejects_send_message_without_model_identifier() {
        let err = build_send_cascade_message_request(
            "api-key",
            "cascade-1",
            "hello",
            0,
            None,
            "session-1",
        )
        .expect_err("missing model should fail");

        assert!(err.to_string().contains("model"));
    }

    fn planner_fields_from_send_request(request: &[u8]) -> Vec<crate::windsurf::proto::Field> {
        let fields = parse_fields(request).expect("send request should parse");
        let config = get_field(&fields, 5, Some(WireType::Len)).expect("config field");
        let config_fields = parse_fields(config.bytes()).expect("config should parse");
        let planner = get_field(&config_fields, 1, Some(WireType::Len)).expect("planner config");
        parse_fields(planner.bytes()).expect("planner should parse")
    }

    fn conversational_fields_from_send_request(
        request: &[u8],
    ) -> Vec<crate::windsurf::proto::Field> {
        let planner_fields = planner_fields_from_send_request(request);
        let conversational =
            get_field(&planner_fields, 2, Some(WireType::Len)).expect("conversational config");
        parse_fields(conversational.bytes()).expect("conversational config should parse")
    }

    fn section_override_string(
        fields: &[crate::windsurf::proto::Field],
        number: u32,
    ) -> Option<String> {
        let section = get_field(fields, number, Some(WireType::Len))?;
        let section_fields = parse_fields(section.bytes()).ok()?;
        get_string(&section_fields, 2)
    }
}
