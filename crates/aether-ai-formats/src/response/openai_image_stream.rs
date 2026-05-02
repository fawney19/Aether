use base64::Engine as _;
use serde_json::Value;

use crate::contracts::OPENAI_IMAGE_SYNC_FINALIZE_REPORT_KIND;
use crate::request::standard::CODEX_OPENAI_IMAGE_DEFAULT_OUTPUT_FORMAT;
use crate::response::sse::encode_json_sse;
use crate::response::AiSurfaceFinalizeError;

#[derive(Default)]
pub struct OpenAiImageStreamState {
    buffered: Vec<u8>,
    latest_image: Option<OpenAiImageFrame>,
    emitted_partial_count: u64,
    saw_upstream_partial: bool,
    emitted_failure: bool,
}

#[derive(Clone)]
struct OpenAiImageFrame {
    b64_json: String,
}

impl OpenAiImageStreamState {
    pub fn push_chunk(
        &mut self,
        report_context: &Value,
        chunk: &[u8],
    ) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        self.buffered.extend_from_slice(chunk);
        let mut output = Vec::new();
        while let Some(block_end) = find_sse_block_end(&self.buffered) {
            let block = self.buffered.drain(..block_end).collect::<Vec<_>>();
            output.extend(self.transform_block(report_context, &block)?);
            drain_sse_separator(&mut self.buffered);
        }
        Ok(output)
    }

    pub fn finish(&mut self, report_context: &Value) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        if self.buffered.is_empty() {
            return Ok(Vec::new());
        }
        let block = std::mem::take(&mut self.buffered);
        self.transform_block(report_context, &block)
    }

    fn transform_block(
        &mut self,
        report_context: &Value,
        block: &[u8],
    ) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        let text = std::str::from_utf8(block)
            .map_err(|err| AiSurfaceFinalizeError::new(err.to_string()))?;
        let mut event_name = None::<String>;
        let mut data_lines = Vec::new();
        for raw_line in text.lines() {
            let line = raw_line.trim_end_matches('\r');
            if let Some(value) = line.strip_prefix("event:") {
                event_name = Some(value.trim().to_string());
            } else if let Some(value) = line.strip_prefix("data:") {
                data_lines.push(value.trim().to_string());
            }
        }
        let data = data_lines.join("\n");
        if data.is_empty() || data == "[DONE]" {
            return Ok(Vec::new());
        }
        let event: Value = serde_json::from_str(&data)?;
        let event_type = event
            .get("type")
            .and_then(Value::as_str)
            .or(event_name.as_deref())
            .unwrap_or_default();
        match event_type {
            "error" | "response.failed" => self.handle_failed(report_context, &event),
            "response.image_generation_call.partial_image" => {
                self.handle_image_generation_partial(report_context, &event)
            }
            "response.output_item.done" => self.handle_output_item_done(report_context, &event),
            "response.completed" => self.handle_completed(report_context, &event),
            _ => Ok(Vec::new()),
        }
    }

    fn handle_image_generation_partial(
        &mut self,
        report_context: &Value,
        event: &Value,
    ) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        if self.emitted_failure {
            return Ok(Vec::new());
        }
        if requested_partial_images(report_context) == 0 {
            return Ok(Vec::new());
        }
        let Some(result) = event
            .get("partial_image_b64")
            .or_else(|| event.get("b64_json"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return Ok(Vec::new());
        };
        let partial_image_index = event
            .get("partial_image_index")
            .or_else(|| event.get("output_index"))
            .and_then(Value::as_u64)
            .unwrap_or(self.emitted_partial_count);
        self.emitted_partial_count = self
            .emitted_partial_count
            .max(partial_image_index.saturating_add(1));
        self.saw_upstream_partial = true;
        self.latest_image = Some(OpenAiImageFrame {
            b64_json: result.to_string(),
        });

        encode_json_sse(
            Some(image_partial_event_name(report_context)),
            &serde_json::json!({
                "type": image_partial_event_name(report_context),
                "b64_json": result,
                "partial_image_index": partial_image_index,
            }),
        )
    }

    fn handle_output_item_done(
        &mut self,
        report_context: &Value,
        event: &Value,
    ) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        if self.emitted_failure {
            return Ok(Vec::new());
        }
        let Some(item) = event.get("item").and_then(Value::as_object) else {
            return Ok(Vec::new());
        };
        if item.get("type").and_then(Value::as_str) != Some("image_generation_call") {
            return Ok(Vec::new());
        }
        let Some(result) = item.get("result").and_then(Value::as_str).map(str::trim) else {
            return Ok(Vec::new());
        };
        if result.is_empty() {
            return Ok(Vec::new());
        }
        self.latest_image = Some(OpenAiImageFrame {
            b64_json: result.to_string(),
        });

        if requested_partial_images(report_context) == 0 || self.saw_upstream_partial {
            return Ok(Vec::new());
        }

        let partial_image_index = event
            .get("output_index")
            .and_then(Value::as_u64)
            .unwrap_or(self.emitted_partial_count);
        self.emitted_partial_count = partial_image_index.saturating_add(1);

        encode_json_sse(
            Some(image_partial_event_name(report_context)),
            &serde_json::json!({
                "type": image_partial_event_name(report_context),
                "b64_json": result,
                "partial_image_index": partial_image_index,
            }),
        )
    }

    fn handle_completed(
        &mut self,
        report_context: &Value,
        event: &Value,
    ) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        if self.emitted_failure {
            return Ok(Vec::new());
        }
        if self.latest_image.is_none() {
            if let Some(result) = completed_response_image_result(event) {
                self.latest_image = Some(OpenAiImageFrame {
                    b64_json: result.to_string(),
                });
            }
        }
        let Some(latest_image) = self.latest_image.clone() else {
            return Ok(Vec::new());
        };
        let usage = event
            .get("response")
            .and_then(Value::as_object)
            .and_then(|response| {
                response
                    .get("tool_usage")
                    .and_then(|value| value.get("image_gen"))
                    .cloned()
                    .or_else(|| response.get("usage").cloned())
            })
            .unwrap_or(Value::Null);

        encode_json_sse(
            Some(image_completed_event_name(report_context)),
            &serde_json::json!({
                "type": image_completed_event_name(report_context),
                "b64_json": latest_image.b64_json,
                "usage": usage,
            }),
        )
    }

    fn handle_failed(
        &mut self,
        report_context: &Value,
        event: &Value,
    ) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        if self.emitted_failure {
            return Ok(Vec::new());
        }
        self.emitted_failure = true;
        let error = image_failure_error(event);
        encode_json_sse(
            Some(image_failed_event_name(report_context)),
            &serde_json::json!({
                "type": image_failed_event_name(report_context),
                "error": error,
            }),
        )
    }
}

fn image_failure_error(event: &Value) -> Value {
    let mut error = event
        .get("error")
        .or_else(|| event.get("response").and_then(|value| value.get("error")))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    if !error.contains_key("message") {
        if let Some(message) = event
            .get("message")
            .and_then(Value::as_str)
            .or_else(|| {
                event
                    .get("response")
                    .and_then(|value| value.get("error"))
                    .and_then(|value| value.get("message"))
                    .and_then(Value::as_str)
            })
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            error.insert("message".to_string(), Value::String(message.to_string()));
        }
    }
    if !error.contains_key("code") {
        if let Some(code) = event
            .get("code")
            .or_else(|| {
                event
                    .get("response")
                    .and_then(|value| value.get("error"))
                    .and_then(|value| value.get("code"))
            })
            .cloned()
        {
            error.insert("code".to_string(), code);
        }
    }
    if !error.contains_key("type") {
        let inferred_type = error
            .get("code")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .unwrap_or("upstream_error");
        error.insert("type".to_string(), Value::String(inferred_type.to_string()));
    }
    if !error.contains_key("message") {
        error.insert(
            "message".to_string(),
            Value::String("Image generation failed".to_string()),
        );
    }

    Value::Object(error)
}

fn completed_response_image_result(event: &Value) -> Option<&str> {
    event
        .get("response")
        .and_then(|value| value.get("output"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("image_generation_call"))
        .filter_map(|item| item.get("result").and_then(Value::as_str))
        .map(str::trim)
        .find(|value| !value.is_empty())
}

fn requested_partial_images(report_context: &Value) -> u64 {
    report_context
        .get("image_request")
        .and_then(|value| value.get("partial_images"))
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

fn image_partial_event_name(report_context: &Value) -> &'static str {
    if image_request_operation(report_context) == Some("edit") {
        "image_edit.partial_image"
    } else {
        "image_generation.partial_image"
    }
}

fn image_completed_event_name(report_context: &Value) -> &'static str {
    if image_request_operation(report_context) == Some("edit") {
        "image_edit.completed"
    } else {
        "image_generation.completed"
    }
}

fn image_failed_event_name(report_context: &Value) -> &'static str {
    if image_request_operation(report_context) == Some("edit") {
        "image_edit.failed"
    } else {
        "image_generation.failed"
    }
}

fn image_request_operation(report_context: &Value) -> Option<&str> {
    report_context
        .get("image_request")
        .and_then(|value| value.get("operation"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn find_sse_block_end(buffer: &[u8]) -> Option<usize> {
    buffer
        .windows(2)
        .position(|window| window == b"\n\n")
        .map(|index| index + 2)
        .or_else(|| {
            buffer
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .map(|index| index + 4)
        })
}

fn drain_sse_separator(buffer: &mut Vec<u8>) {
    while matches!(buffer.first(), Some(b'\n' | b'\r')) {
        buffer.remove(0);
    }
}

pub struct OpenAiImageSyncFinalizeProduct {
    pub client_body_json: Value,
    pub provider_body_json: Value,
}

pub fn maybe_build_openai_image_sync_finalize_product(
    report_kind: &str,
    status_code: u16,
    report_context: Option<&Value>,
    body_base64: Option<&str>,
) -> Result<Option<OpenAiImageSyncFinalizeProduct>, AiSurfaceFinalizeError> {
    if report_kind != OPENAI_IMAGE_SYNC_FINALIZE_REPORT_KIND || status_code >= 400 {
        return Ok(None);
    }
    let Some(report_context) = report_context else {
        return Ok(None);
    };
    if report_context
        .get("client_api_format")
        .and_then(Value::as_str)
        .map(str::trim)
        != Some("openai:image")
    {
        return Ok(None);
    }
    let Some(body_base64) = body_base64 else {
        return Ok(None);
    };
    let default_output_format = report_context
        .get("image_request")
        .and_then(|value| value.get("output_format"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(CODEX_OPENAI_IMAGE_DEFAULT_OUTPUT_FORMAT);
    let body_bytes = base64::engine::general_purpose::STANDARD.decode(body_base64)?;
    let text = std::str::from_utf8(&body_bytes)
        .map_err(|err| AiSurfaceFinalizeError::new(err.to_string()))?;

    let mut created = None;
    let mut completed_response = None;
    let mut images = Vec::new();

    for raw_block in text.split("\n\n") {
        let block = raw_block.trim();
        if block.is_empty() {
            continue;
        }
        let data_line = block
            .lines()
            .find_map(|line| line.trim().strip_prefix("data:").map(str::trim));
        let Some(data_line) = data_line else {
            continue;
        };
        if data_line.is_empty() || data_line == "[DONE]" {
            continue;
        }
        let event: Value = serde_json::from_str(data_line)?;
        match event
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
        {
            "response.created" => {
                created = event
                    .get("response")
                    .and_then(|value| value.get("created_at"))
                    .and_then(Value::as_i64)
                    .or(created);
            }
            "response.output_item.done" => {
                let Some(item) = event.get("item").and_then(Value::as_object) else {
                    continue;
                };
                if item.get("type").and_then(Value::as_str) != Some("image_generation_call") {
                    continue;
                }
                let Some(result) = item.get("result").and_then(Value::as_str) else {
                    continue;
                };
                images.push(serde_json::json!({
                    "b64_json": result,
                    "output_format": item.get("output_format").cloned().unwrap_or(Value::String(default_output_format.to_string())),
                    "revised_prompt": item.get("revised_prompt").cloned().unwrap_or(Value::Null),
                }));
            }
            "response.completed" => {
                completed_response = event.get("response").and_then(Value::as_object).cloned();
            }
            _ => {}
        }
    }

    if images.is_empty() {
        return Ok(None);
    }

    let completed_response = completed_response.unwrap_or_default();
    let provider_usage = completed_response
        .get("tool_usage")
        .and_then(|value| value.get("image_gen"))
        .cloned()
        .or_else(|| completed_response.get("usage").cloned());
    let provider_body_json = serde_json::json!({
        "id": completed_response.get("id").cloned().unwrap_or(Value::Null),
        "object": "response",
        "model": completed_response.get("model").cloned().unwrap_or(Value::Null),
        "status": completed_response.get("status").cloned().unwrap_or(Value::String("completed".to_string())),
        "usage": provider_usage,
        "tool_usage": completed_response.get("tool_usage").cloned().unwrap_or(Value::Null),
        "output": images
            .iter()
            .map(|image| serde_json::json!({
                "type": "image_generation_call",
                "output_format": image.get("output_format").cloned().unwrap_or(Value::Null),
                "revised_prompt": image.get("revised_prompt").cloned().unwrap_or(Value::Null),
            }))
            .collect::<Vec<_>>(),
    });
    let client_images = images
        .iter()
        .map(|image| {
            let revised_prompt = image.get("revised_prompt").cloned().unwrap_or(Value::Null);
            let b64_json = image
                .get("b64_json")
                .and_then(Value::as_str)
                .unwrap_or_default();
            serde_json::json!({
                "b64_json": b64_json,
                "revised_prompt": revised_prompt,
            })
        })
        .collect::<Vec<_>>();
    let client_body_json = serde_json::json!({
        "created": created.unwrap_or_default(),
        "data": client_images,
        "usage": provider_body_json.get("usage").cloned().unwrap_or(Value::Null),
    });

    Ok(Some(OpenAiImageSyncFinalizeProduct {
        client_body_json,
        provider_body_json,
    }))
}

#[cfg(test)]
mod tests {
    use base64::Engine as _;
    use serde_json::json;

    use super::{maybe_build_openai_image_sync_finalize_product, OpenAiImageStreamState};

    fn utf8(bytes: Vec<u8>) -> String {
        String::from_utf8(bytes).expect("utf8 should decode")
    }

    #[test]
    fn emits_completed_event_for_generate() {
        let report_context = json!({
            "provider_api_format": "openai:image",
            "client_api_format": "openai:image",
            "needs_conversion": false,
            "image_request": {
                "operation": "generate"
            }
        });
        let mut rewriter = OpenAiImageStreamState::default();

        let first = rewriter
            .push_chunk(
                &report_context,
                concat!(
                    "event: response.output_item.done\n",
                    "data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"id\":\"ig_123\",\"type\":\"image_generation_call\",\"result\":\"aGVsbG8=\"}}\n\n"
                )
                .as_bytes(),
            )
            .expect("rewrite should succeed");
        assert!(first.is_empty());

        let second = rewriter
            .push_chunk(
                &report_context,
                concat!(
                    "event: response.completed\n",
                    "data: {\"type\":\"response.completed\",\"response\":{\"tool_usage\":{\"image_gen\":{\"input_tokens\":1,\"output_tokens\":2,\"total_tokens\":3}}}}\n\n"
                )
                .as_bytes(),
            )
            .expect("rewrite should succeed");
        let output_text = utf8(second);
        assert!(output_text.contains("event: image_generation.completed"));
        assert!(output_text.contains("\"type\":\"image_generation.completed\""));
        assert!(output_text.contains("\"b64_json\":\"aGVsbG8=\""));
        assert!(output_text.contains("\"input_tokens\":1"));
        assert!(!output_text.contains("data: [DONE]"));
        assert!(rewriter
            .finish(&report_context)
            .expect("finish should succeed")
            .is_empty());
    }

    #[test]
    fn maps_responses_partial_image_events() {
        let report_context = json!({
            "provider_api_format": "openai:image",
            "client_api_format": "openai:image",
            "needs_conversion": false,
            "image_request": {
                "operation": "generate",
                "partial_images": 1
            }
        });
        let mut rewriter = OpenAiImageStreamState::default();

        let partial = rewriter
            .push_chunk(
                &report_context,
                concat!(
                    "event: response.image_generation_call.partial_image\n",
                    "data: {\"type\":\"response.image_generation_call.partial_image\",\"partial_image_index\":0,\"partial_image_b64\":\"cGFydGlhbA==\"}\n\n"
                )
                .as_bytes(),
            )
            .expect("rewrite should succeed");
        let partial_text = utf8(partial);
        assert!(partial_text.contains("event: image_generation.partial_image"));
        assert!(partial_text.contains("\"type\":\"image_generation.partial_image\""));
        assert!(partial_text.contains("\"b64_json\":\"cGFydGlhbA==\""));
        assert!(partial_text.contains("\"partial_image_index\":0"));
        assert!(!partial_text.contains("response.image_generation_call.partial_image"));

        let done = rewriter
            .push_chunk(
                &report_context,
                concat!(
                    "event: response.output_item.done\n",
                    "data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"id\":\"ig_123\",\"type\":\"image_generation_call\",\"result\":\"ZmluYWw=\"}}\n\n"
                )
                .as_bytes(),
            )
            .expect("rewrite should succeed");
        assert!(done.is_empty());

        let completed = rewriter
            .push_chunk(
                &report_context,
                concat!(
                    "event: response.completed\n",
                    "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":4,\"output_tokens\":5,\"total_tokens\":9}}}\n\n"
                )
                .as_bytes(),
            )
            .expect("rewrite should succeed");
        let completed_text = utf8(completed);
        assert!(completed_text.contains("event: image_generation.completed"));
        assert!(completed_text.contains("\"type\":\"image_generation.completed\""));
        assert!(completed_text.contains("\"b64_json\":\"ZmluYWw=\""));
        assert!(completed_text.contains("\"total_tokens\":9"));
    }

    #[test]
    fn maps_upstream_error_to_generation_failed_once() {
        let report_context = json!({
            "provider_api_format": "openai:image",
            "client_api_format": "openai:image",
            "needs_conversion": false,
            "image_request": {
                "operation": "generate"
            }
        });
        let mut rewriter = OpenAiImageStreamState::default();

        let output = rewriter
            .push_chunk(
                &report_context,
                concat!(
                    "event: error\n",
                    "data: {\"type\":\"error\",\"error\":{\"type\":\"input-images\",\"code\":\"rate_limit_exceeded\",\"message\":\"Rate limit reached for gpt-image-2\",\"param\":null}}\n\n",
                    "event: response.failed\n",
                    "data: {\"type\":\"response.failed\",\"response\":{\"status\":\"failed\",\"error\":{\"code\":\"rate_limit_exceeded\",\"message\":\"Rate limit reached for gpt-image-2\"}}}\n\n"
                )
                .as_bytes(),
            )
            .expect("rewrite should succeed");
        let output_text = utf8(output);
        assert!(output_text.contains("event: image_generation.failed"));
        assert_eq!(
            output_text
                .matches("event: image_generation.failed")
                .count(),
            1
        );
        assert!(output_text.contains("\"type\":\"image_generation.failed\""));
        assert!(output_text.contains("\"type\":\"input-images\""));
        assert!(output_text.contains("\"code\":\"rate_limit_exceeded\""));
        assert!(output_text.contains("\"message\":\"Rate limit reached for gpt-image-2\""));
        assert!(!output_text.contains("response.failed"));
        assert!(rewriter
            .finish(&report_context)
            .expect("finish should succeed")
            .is_empty());
    }

    #[test]
    fn sync_finalize_product_maps_stream_response_to_client_and_provider_bodies() {
        let report_context = json!({
            "client_api_format": "openai:image",
            "provider_api_format": "openai:image",
            "image_request": {
                "operation": "generate",
                "output_format": "png"
            }
        });
        let body_base64 = base64::engine::general_purpose::STANDARD.encode(
            concat!(
                "event: response.created\n",
                "data: {\"type\":\"response.created\",\"response\":{\"created_at\":1776839946}}\n\n",
                "event: response.output_item.done\n",
                "data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"type\":\"image_generation_call\",\"output_format\":\"png\",\"revised_prompt\":\"revised history prompt\",\"result\":\"aGVsbG8=\"}}\n\n",
                "event: response.completed\n",
                "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_img_123\",\"model\":\"gpt-5.4\",\"status\":\"completed\",\"tool_usage\":{\"image_gen\":{\"input_tokens\":171,\"output_tokens\":1372,\"total_tokens\":1543}}}}\n\n"
            )
            .as_bytes(),
        );

        let product = maybe_build_openai_image_sync_finalize_product(
            "openai_image_sync_finalize",
            200,
            Some(&report_context),
            Some(&body_base64),
        )
        .expect("finalize should succeed")
        .expect("finalize should match");

        assert_eq!(product.client_body_json["created"], 1776839946);
        assert_eq!(product.client_body_json["data"][0]["b64_json"], "aGVsbG8=");
        assert_eq!(
            product.client_body_json["data"][0]["revised_prompt"],
            "revised history prompt"
        );
        assert_eq!(product.client_body_json["usage"]["input_tokens"], 171);
        assert_eq!(product.provider_body_json["id"], "resp_img_123");
        assert_eq!(
            product.provider_body_json["output"][0]["output_format"],
            "png"
        );
        assert_eq!(
            product.provider_body_json["output"][0]["revised_prompt"],
            "revised history prompt"
        );
    }
}
