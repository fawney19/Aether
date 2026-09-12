use serde_json::{json, Value};

use super::{ProviderStreamParser, StreamingStandardTerminalObserver, TerminalStreamParser};

fn full_observer(context: &Value) -> StreamingStandardTerminalObserver {
    StreamingStandardTerminalObserver {
        provider: Some(TerminalStreamParser::Standard(
            ProviderStreamParser::for_api_format(context["provider_api_format"].as_str().unwrap())
                .unwrap(),
        )),
        ..Default::default()
    }
}

// Compare every input prefix, including EOF, to catch changes in identity and terminal timing.
fn assert_summaries_match(context: &Value, events: &[Value]) {
    let structured = context["provider_api_format"]
        .as_str()
        .unwrap()
        .starts_with("openai:responses");
    for end in 0..=events.len() {
        let mut compact = StreamingStandardTerminalObserver::default();
        let mut full = full_observer(context);
        let mut via_event = StreamingStandardTerminalObserver::default();
        for (index, event) in events[..end].iter().enumerate() {
            let line = format!("data: {event}\n").into_bytes();
            compact.push_line(context, line.clone()).unwrap();
            full.push_line(context, line).unwrap();
            assert_eq!(
                compact.latest_summary(),
                full.latest_summary(),
                "prefix {index}: {event}"
            );
            if structured {
                via_event.push_event(context, event).unwrap();
                assert_eq!(via_event.latest_summary(), full.latest_summary());
            }
        }
        let expected = full.finish(context).unwrap();
        assert_eq!(compact.finish(context).unwrap(), expected, "EOF at {end}");
        if structured {
            assert_eq!(via_event.finish(context).unwrap(), expected);
        }
    }
}

fn context(format: &str) -> Value {
    json!({"provider_api_format": format, "client_api_format": format, "mapped_model": "test-model"})
}

fn completed(output: Vec<Value>) -> Value {
    json!({"type":"response.completed","response":{
        "id":"resp-final","model":"final-model","status":"completed","service_tier":" PRIORITY ",
        "output":output,"usage":{"input_tokens":11,"output_tokens":7,"total_tokens":18,
        "input_tokens_details":{"cached_tokens":0},"output_tokens_details":{"reasoning_tokens":3}}
    }})
}

#[test]
fn responses_content_snapshots_and_deltas_preserve_every_summary() {
    let events = vec![
        json!({"type":"response.output_text.delta","delta":""}),
        json!({"type":"response.output_text.delta","delta":"he","output_index":0}),
        json!({"type":"response.created","response":{"id":"late-id","model":"late-model"}}),
        json!({"type":"response.output_text.delta","delta":{"text":"hello"},"output_index":0}),
        json!({"type":"response.output_text.done","text":"hello","output_index":0}),
        json!({"type":"response.content_part.added","content_index":1,"part":{"type":"output_text","text":"another"}}),
        json!({"type":"response.content_part.done","content_index":1,"part":{"type":"output_text","text":"another part"}}),
        json!({"type":"response.refusal.delta","delta":"refuse"}),
        json!({"type":"response.refusal.done","refusal":"refused"}),
        json!({"type":"response.audio.transcript.delta","delta":"audio"}),
        json!({"type":"response.audio.transcript.done","transcript":"audio transcript"}),
        json!({"type":"response.reasoning_summary_text.delta","delta":"think","summary_index":0}),
        json!({"type":"response.reasoning_text.delta","delta":" again","summary_index":0}),
        json!({"type":"response.reasoning_summary_part.added","summary_index":1,"part":{"type":"summary_text","text":"second"}}),
        json!({"type":"response.reasoning_summary_part.done","summary_index":1,"part":{"type":"summary_text","text":"second thought"}}),
        json!({"type":"response.reasoning_summary_text.done","summary_index":0,"text":"think again"}),
        json!({"type":"response.reasoning_text.done","text":""}),
        completed(vec![
            json!({"type":"message","content":[{"type":"output_text","text":"hello"},{"type":"refusal","refusal":"refused"}]}),
            json!({"type":"reasoning","summary":[{"type":"summary_text","text":"think again"},{"type":"summary_text","text":"second thought"}]}),
        ]),
    ];
    for format in ["openai:responses", "openai:responses:compact"] {
        assert_summaries_match(&context(format), &events);
        for event in &events {
            assert_summaries_match(&context(format), std::slice::from_ref(event));
        }
    }
}

#[test]
fn responses_tools_and_unknown_execution_fields_preserve_summaries() {
    let calls = vec![
        json!({"type":"function_call","call_id":"call-0","name":"lookup","arguments":"{\"x\":1}"}),
        json!({"type":"custom_tool_call","call_id":"call-1","name":"custom","input":"raw input"}),
        json!({"type":"shell_call","call_id":"call-2","action":{"commands":["pwd"]}}),
        json!({"type":"local_shell_call","call_id":"call-3","action":{"command":["pwd"]}}),
        json!({"type":"apply_patch_call","call_id":"call-4","operation":{"type":"update_file","path":"a","diff":"+b"}}),
        json!({"type":"computer_call","call_id":"call-5","action":{"type":"click","x":1,"y":2}}),
        json!({"type":"function_call","call_id":"bad-caller","name":"lookup","arguments":"{}","caller":{"type":"direct"}}),
        json!({"type":"function_call","call_id":"bad-namespace","namespace":42,"name":"lookup","arguments":"{}"}),
    ];
    let mut events = vec![
        json!({"type":"response.function_call_arguments.delta","output_index":0,"delta":"{"}),
        json!({"type":"response.function_call_arguments.delta","output_index":0,"call_id":"call-0","delta":"\"x\":1}"}),
        json!({"type":"response.function_call_arguments.done","output_index":0,"item":{"call_id":"call-0","name":"lookup","arguments":"{\"x\":1}"}}),
        json!({"type":"response.function_call_arguments.done","output_index":0,"namespace":{},"arguments":"{}"}),
        json!({"type":"response.custom_tool_call_input.delta","output_index":1,"delta":"raw"}),
        json!({"type":"response.custom_tool_call_input.done","output_index":1,"input":"raw input"}),
    ];
    for (index, item) in calls.iter().enumerate() {
        events.push(json!({"type":"response.output_item.added","output_index":index,"item":item}));
        events.push(json!({"type":"response.output_item.done","output_index":index,"item":item}));
    }
    for kind in [
        "function_call",
        "custom_tool_call",
        "shell_call",
        "local_shell_call",
        "apply_patch_call",
        "computer_call",
    ] {
        events.push(json!({"type":format!("response.{kind}_output.delta"),"call_id":"result","delta":"result"}));
        events.push(json!({"type":format!("response.{kind}_output.done"),"call_id":"result","output":{"ok":true}}));
        events.push(json!({"type":"response.output_item.done","item":{"type":format!("{kind}_output"),"call_id":"result","output":"result complete"}}));
    }
    events.push(completed(calls));
    assert_summaries_match(&context("openai:responses"), &events);
}

#[test]
fn responses_opaque_dedup_and_images_preserve_unknown_counts() {
    let items = vec![
        json!({"type":"future_item","id":"stable-id","payload":"first"}),
        json!({"type":"future_item","encrypted_content":"encrypted","payload":"second"}),
        json!({"type":"future_item","payload":{"no_id":true}}),
        json!({"type":"image_generation_call","result":"image data","status":"completed"}),
        json!({"type":"reasoning","summary":[],"encrypted_content":"reasoning"}),
    ];
    let mut events = Vec::new();
    for item in &items {
        events.push(json!({"type":"response.output_item.added","item":item}));
        events.push(json!({"type":"response.output_item.done","item":item}));
        events.push(json!({"type":"response.output_item.done","item":item}));
    }
    events.push(json!({"type":"response.output_item.done","item":{"missing_type":true}}));
    let mut output = items;
    output[0]["payload"] = json!("changed body, same id");
    output[1]["payload"] = json!("changed body, same encrypted content");
    output.push(json!({"type":"new_future_item","payload":"never emitted"}));
    events.push(completed(output));
    let ctx = context("openai:responses");
    assert_summaries_match(&ctx, &events);
    let mut observer = StreamingStandardTerminalObserver::default();
    for event in &events {
        observer.push_event(&ctx, event).unwrap();
    }
    assert_eq!(
        observer.finish(&ctx).unwrap().unwrap().unknown_event_count,
        2
    );
}

#[test]
fn responses_namespace_validation_keeps_existing_tool_identity() {
    let mut ctx = context("openai:responses");
    ctx["original_request_body"] = json!({"tools":[{
        "type":"namespace","name":"search","description":"Search tools","tools":[{"type":"function","name":"lookup","parameters":{"type":"object"}}]
    }]});
    let events = [
        json!({"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","call_id":"call","namespace":"search","name":"lookup","arguments":"{"}}),
        json!({"type":"response.function_call_arguments.delta","output_index":0,"delta":"}"}),
        json!({"type":"response.function_call_arguments.done","output_index":0,"namespace":"search","arguments":"{}"}),
        json!({"type":"response.function_call_arguments.done","output_index":0,"namespace":"missing","arguments":"{}"}),
        completed(vec![
            json!({"type":"function_call","call_id":"call","namespace":"search","name":"lookup","arguments":"{}"}),
        ]),
    ];
    assert_summaries_match(&ctx, &events);
    let mut observer = StreamingStandardTerminalObserver::default();
    for event in &events {
        observer.push_event(&ctx, event).unwrap();
    }
    let summary = observer.finish(&ctx).unwrap().unwrap();
    assert_eq!(summary.unknown_event_count, 1);
    assert_eq!(summary.finish_reason.as_deref(), Some("tool_calls"));
}

#[test]
fn responses_errors_zero_usage_and_event_type_lines_preserve_summaries() {
    for terminal in [
        json!({"type":"response.failed","response":{"status":"failed","error":{"message":"failed"},"usage":{"input_tokens":0,"output_tokens":0}}}),
        json!({"type":"error","error":{"type":"server_error","message":"failed"}}),
        json!({"type":"response.incomplete","response":{"status":"incomplete","incomplete_details":{"reason":"max_output_tokens"},"usage":{"input_tokens":2,"output_tokens":0}}}),
        json!({"type":"response.done","response":{"status":"completed","usage":{"input_tokens":0,"output_tokens":0,"total_tokens":0}}}),
        json!({"type":"response.completed","response":null}),
    ] {
        let ctx = context("openai:responses");
        let events = vec![
            json!({"type":"response.future"}),
            json!({"type":"ping"}),
            terminal,
        ];
        assert_summaries_match(&ctx, &events);
        let mut compact = StreamingStandardTerminalObserver::default();
        let mut full = full_observer(&ctx);
        for mut event in events {
            let kind = event.as_object_mut().unwrap().remove("type").unwrap();
            for line in [
                format!("event: {}\n", kind.as_str().unwrap()),
                format!("data: {event}\n"),
                "\n".to_string(),
            ] {
                compact.push_line(&ctx, line.as_bytes().to_vec()).unwrap();
                full.push_line(&ctx, line.into_bytes()).unwrap();
                assert_eq!(compact.latest_summary(), full.latest_summary());
            }
        }
        assert_eq!(compact.finish(&ctx).unwrap(), full.finish(&ctx).unwrap());
    }
}

#[test]
fn chat_delayed_tool_identity_and_usage_only_preserve_summaries() {
    let ctx = context("openai:chat");
    assert_summaries_match(
        &ctx,
        &[
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{"}}]}}]}),
            json!({"id":"late-id","model":"late-model","choices":[{"delta":{"tool_calls":[{"index":0,"id":"call","function":{"name":"lookup","arguments":"}"}}]}}]}),
            json!({"choices":[{"delta":{"content":"text","reasoning_content":"reason"}}],"service_tier":"priority"}),
            json!({"choices":[{"delta":{},"finish_reason":"tool_calls"}]}),
            json!({"choices":[],"usage":{"prompt_tokens":3,"completion_tokens":2,"total_tokens":5}}),
        ],
    );
    for event in [
        json!({"usage":{"prompt_tokens":0,"completion_tokens":0,"total_tokens":0}}),
        json!({"choices":[{"delta":{"tool_calls":"malformed"}}]}),
        json!({"choices":[{"delta":{"tool_calls":[null,{}, {"function":null}]}}]}),
        json!({"choices":[{"delta":{},"finish_reason":"future_reason"}]}),
        json!({"choices":[{"delta":{"future_content":true}}]}),
    ] {
        assert_summaries_match(&ctx, &[event]);
    }
}

#[test]
fn gemini_content_tools_and_errors_preserve_summaries() {
    let ctx = context("gemini:generate_content");
    let parts = vec![
        json!({"text":"text"}),
        json!({"text":"reason","thought":true,"thoughtSignature":"sig"}),
        json!({"functionCall":{"id":"call","name":"lookup","args":{"x":1}}}),
        json!({"functionResponse":{"id":"call","name":"lookup","response":{"ok":true}}}),
        json!({"inlineData":{"mimeType":"image/png","data":"aW1hZ2U="}}),
        json!({"futureContent":"unknown"}),
    ];
    let mut events = Vec::new();
    for part in &parts {
        let event = json!({"candidates":[{"content":{"parts":[part]}}]});
        events.push(event.clone());
        events.push(event.clone());
        assert_summaries_match(&ctx, &[event]);
    }
    events.push(json!({"responseId":"late-id","modelVersion":"late-model","candidates":[{"content":{"parts":parts},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":5,"candidatesTokenCount":3,"totalTokenCount":8}}));
    assert_summaries_match(&ctx, &events);
    for reason in [
        "MALFORMED_FUNCTION_CALL",
        "SAFETY",
        "MAX_TOKENS",
        "FUTURE_REASON",
    ] {
        assert_summaries_match(
            &ctx,
            &[
                json!({"response":{"candidates":[{"content":{"parts":[{"text":"partial"}]}}]}}),
                json!({"candidates":[{"content":{"parts":[]},"finishReason":reason}],"usageMetadata":{"promptTokenCount":0,"candidatesTokenCount":0}}),
            ],
        );
    }
}
