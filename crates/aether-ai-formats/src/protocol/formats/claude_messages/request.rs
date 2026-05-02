use serde_json::{json, Map, Value};

use crate::{
    protocol::canonical::{
        canonical_extension_object_mut, canonical_instructions_to_claude_system,
        canonical_messages_to_claude, canonical_openai_reasoning_effort,
        canonical_tool_choice_to_claude, canonical_tools_to_claude, claude_extensions,
        claude_generation_config, claude_messages_to_canonical, claude_parallel_tool_calls,
        claude_system_to_canonical_instructions, claude_thinking_to_canonical,
        claude_tool_choice_to_canonical, claude_tools_to_canonical,
        compact_canonical_claude_messages, insert_f64, namespace_extension_object,
        CanonicalRequest,
    },
    protocol::context::FormatContext,
    request::openai::{
        map_openai_reasoning_effort_to_claude_output,
        map_openai_reasoning_effort_to_thinking_budget,
    },
};

pub fn from(body: &Value, _ctx: &FormatContext) -> Option<CanonicalRequest> {
    from_raw(body)
}

pub fn to(request: &CanonicalRequest, ctx: &FormatContext) -> Option<Value> {
    to_raw(
        request,
        ctx.mapped_model_or(request.model.as_str()),
        ctx.upstream_is_stream,
    )
}

pub fn from_raw(body_json: &Value) -> Option<CanonicalRequest> {
    let request = body_json.as_object()?;
    let mut canonical = CanonicalRequest {
        model: request
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        ..CanonicalRequest::default()
    };

    canonical.instructions = claude_system_to_canonical_instructions(request.get("system"))?;
    let system_text = canonical
        .instructions
        .iter()
        .map(|instruction| instruction.text.as_str())
        .filter(|text| !text.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    if !system_text.is_empty() {
        canonical.system = Some(system_text);
    }
    canonical.messages = claude_messages_to_canonical(request.get("messages"))?;
    canonical.generation = claude_generation_config(request);
    let (tools, builtin_tools, web_search_options) =
        claude_tools_to_canonical(request.get("tools"))?;
    canonical.tools = tools;
    canonical.tool_choice = claude_tool_choice_to_canonical(request.get("tool_choice"));
    canonical.parallel_tool_calls = claude_parallel_tool_calls(request.get("tool_choice"));
    canonical.metadata = request.get("metadata").cloned();
    canonical.thinking = claude_thinking_to_canonical(request);

    canonical.extensions = claude_extensions(
        request,
        &[
            "model",
            "system",
            "messages",
            "max_tokens",
            "temperature",
            "top_p",
            "top_k",
            "stop",
            "stop_sequences",
            "stream",
            "tools",
            "tool_choice",
            "metadata",
            "thinking",
            "output_config",
        ],
    );
    if !builtin_tools.is_empty() {
        canonical_extension_object_mut(&mut canonical.extensions, "claude")
            .insert("builtin_tools".to_string(), Value::Array(builtin_tools));
    }
    if let Some(web_search_options) = web_search_options {
        canonical_extension_object_mut(&mut canonical.extensions, "openai")
            .insert("web_search_options".to_string(), web_search_options);
    }
    if let Some(output_config) = request.get("output_config").cloned() {
        canonical_extension_object_mut(&mut canonical.extensions, "claude")
            .insert("output_config".to_string(), output_config);
    }
    Some(canonical)
}

pub fn to_raw(
    canonical: &CanonicalRequest,
    mapped_model: &str,
    upstream_is_stream: bool,
) -> Option<Value> {
    let mut output = Map::new();
    output.insert("model".to_string(), Value::String(mapped_model.to_string()));
    output.insert(
        "messages".to_string(),
        Value::Array(compact_canonical_claude_messages(
            canonical_messages_to_claude(canonical)?,
        )),
    );
    output.insert(
        "max_tokens".to_string(),
        Value::from(canonical.generation.max_tokens.unwrap_or(1024)),
    );
    if let Some(system) = canonical_instructions_to_claude_system(&canonical.instructions) {
        output.insert("system".to_string(), system);
    } else if let Some(system) = canonical
        .system
        .as_ref()
        .filter(|value| !value.trim().is_empty())
    {
        output.insert("system".to_string(), Value::String(system.clone()));
    }
    if upstream_is_stream {
        output.insert("stream".to_string(), Value::Bool(true));
    }
    insert_f64(&mut output, "temperature", canonical.generation.temperature);
    insert_f64(&mut output, "top_p", canonical.generation.top_p);
    if let Some(top_k) = canonical.generation.top_k {
        output.insert("top_k".to_string(), Value::from(top_k));
    }
    if let Some(stop_sequences) = &canonical.generation.stop_sequences {
        output.insert(
            "stop_sequences".to_string(),
            Value::Array(stop_sequences.iter().cloned().map(Value::String).collect()),
        );
    }
    let tools = canonical_tools_to_claude(canonical);
    if !tools.is_empty() {
        output.insert("tools".to_string(), Value::Array(tools));
    }
    if let Some(tool_choice) = canonical_tool_choice_to_claude(
        canonical.tool_choice.as_ref(),
        canonical.parallel_tool_calls,
    ) {
        output.insert("tool_choice".to_string(), tool_choice);
    }
    if let Some(metadata) = canonical.metadata.clone() {
        output.insert("metadata".to_string(), metadata);
    }
    if let Some(thinking) = canonical.thinking.as_ref() {
        let openai_effort = canonical_openai_reasoning_effort(thinking);
        let budget_tokens = thinking
            .budget_tokens
            .or_else(|| openai_effort.and_then(map_openai_reasoning_effort_to_thinking_budget));
        if thinking.enabled || budget_tokens.is_some() {
            output.insert(
                "thinking".to_string(),
                json!({
                    "type": "enabled",
                    "budget_tokens": budget_tokens.unwrap_or(1024),
                }),
            );
        }
        if let Some(output_effort) =
            openai_effort.and_then(map_openai_reasoning_effort_to_claude_output)
        {
            output.insert(
                "output_config".to_string(),
                json!({
                    "effort": output_effort,
                }),
            );
        }
    }
    output.extend(namespace_extension_object(
        &canonical.extensions,
        "claude",
        &output,
    ));
    Some(Value::Object(output))
}
