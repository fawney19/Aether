use serde_json::{Map, Value};

use crate::snapshot::GatewayProviderTransportSnapshot;
use crate::vertex::is_vertex_transport_context;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportRequestBodySemanticsError {
    message: &'static str,
}

impl TransportRequestBodySemanticsError {
    const fn new(message: &'static str) -> Self {
        Self { message }
    }

    pub const fn message(&self) -> &'static str {
        self.message
    }
}

impl std::fmt::Display for TransportRequestBodySemanticsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message)
    }
}

impl std::error::Error for TransportRequestBodySemanticsError {}

pub fn apply_transport_request_body_semantics(
    provider_request_body: &mut Value,
    transport: &GatewayProviderTransportSnapshot,
    provider_api_format: &str,
) -> Result<(), TransportRequestBodySemanticsError> {
    let provider_api_format = aether_ai_formats::normalize_api_format_alias(provider_api_format);
    if provider_api_format == "gemini:embedding" && is_vertex_transport_context(transport) {
        apply_vertex_gemini_embedding_body_semantics(provider_request_body)?;
    }
    Ok(())
}

fn apply_vertex_gemini_embedding_body_semantics(
    provider_request_body: &mut Value,
) -> Result<(), TransportRequestBodySemanticsError> {
    let object = provider_request_body.as_object_mut().ok_or_else(|| {
        TransportRequestBodySemanticsError::new(
            "Vertex Gemini embedding request body must be a JSON object",
        )
    })?;

    if object.contains_key("instances") {
        validate_existing_vertex_predict_body(object)?;
        object.remove("model");
        return Ok(());
    }

    let next = build_vertex_predict_body_from_gemini_embedding_object(object)?;
    *object = next;
    Ok(())
}

fn build_vertex_predict_body_from_gemini_embedding_object(
    object: &Map<String, Value>,
) -> Result<Map<String, Value>, TransportRequestBodySemanticsError> {
    if let Some(requests) = object.get("requests") {
        if object.keys().any(|key| key != "requests") {
            return Err(TransportRequestBodySemanticsError::new(
                "Vertex Gemini embedding batch body cannot mix requests with other top-level fields",
            ));
        }
        let request_items = requests.as_array().ok_or_else(|| {
            TransportRequestBodySemanticsError::new(
                "Vertex Gemini embedding requests must be an array",
            )
        })?;
        let request_objects = request_items
            .iter()
            .map(Value::as_object)
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| {
                TransportRequestBodySemanticsError::new(
                    "Vertex Gemini embedding requests must be an array of objects",
                )
            })?;
        if request_objects.is_empty() {
            return Err(TransportRequestBodySemanticsError::new(
                "Vertex Gemini embedding requests must contain at least one item",
            ));
        }
        return build_vertex_predict_body_from_gemini_embedding_items(&request_objects);
    }

    build_vertex_predict_body_from_gemini_embedding_items(&[object])
}

fn build_vertex_predict_body_from_gemini_embedding_items(
    items: &[&Map<String, Value>],
) -> Result<Map<String, Value>, TransportRequestBodySemanticsError> {
    if items.iter().any(|item| {
        item.keys().any(|key| {
            !matches!(
                key.as_str(),
                "model"
                    | "content"
                    | "taskType"
                    | "title"
                    | "outputDimensionality"
                    | "autoTruncate"
            )
        })
    }) {
        return Err(TransportRequestBodySemanticsError::new(
            "Vertex Gemini embedding body contains fields that cannot be mapped to predict instances",
        ));
    }

    let instances = items
        .iter()
        .map(|item| build_vertex_predict_instance(item))
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| {
            TransportRequestBodySemanticsError::new(
                "Vertex Gemini embedding body must contain text content parts",
            )
        })?;
    if instances.is_empty() {
        return Err(TransportRequestBodySemanticsError::new(
            "Vertex Gemini embedding body must contain at least one instance",
        ));
    }

    let mut output = Map::new();
    output.insert("instances".to_string(), Value::Array(instances));

    let mut parameters = Map::new();
    insert_shared_parameter(items, &mut parameters, "outputDimensionality")?;
    insert_shared_parameter(items, &mut parameters, "autoTruncate")?;
    if !parameters.is_empty() {
        output.insert("parameters".to_string(), Value::Object(parameters));
    }

    Ok(output)
}

fn validate_existing_vertex_predict_body(
    object: &Map<String, Value>,
) -> Result<(), TransportRequestBodySemanticsError> {
    if object
        .keys()
        .any(|key| !matches!(key.as_str(), "model" | "instances" | "parameters"))
    {
        return Err(TransportRequestBodySemanticsError::new(
            "Vertex Gemini embedding predict body contains unsupported top-level fields",
        ));
    }
    let Some(instances) = object.get("instances").and_then(Value::as_array) else {
        return Err(TransportRequestBodySemanticsError::new(
            "Vertex Gemini embedding predict body must contain an instances array",
        ));
    };
    if instances.is_empty() {
        return Err(TransportRequestBodySemanticsError::new(
            "Vertex Gemini embedding predict body must contain at least one instance",
        ));
    }
    if object
        .get("parameters")
        .is_some_and(|parameters| !parameters.is_object())
    {
        return Err(TransportRequestBodySemanticsError::new(
            "Vertex Gemini embedding predict parameters must be an object",
        ));
    }
    Ok(())
}

fn build_vertex_predict_instance(item: &Map<String, Value>) -> Option<Value> {
    let content = gemini_embedding_content_text(item.get("content")?)?;
    let mut instance = Map::new();
    instance.insert("content".to_string(), Value::String(content));
    if let Some(task_type) = item.get("taskType") {
        instance.insert(
            "task_type".to_string(),
            Value::String(task_type.as_str()?.to_string()),
        );
    }
    if let Some(title) = item.get("title") {
        instance.insert(
            "title".to_string(),
            Value::String(title.as_str()?.to_string()),
        );
    }
    Some(Value::Object(instance))
}

fn gemini_embedding_content_text(content: &Value) -> Option<String> {
    let parts = content
        .as_object()?
        .get("parts")?
        .as_array()?
        .iter()
        .filter_map(|part| part.as_object()?.get("text")?.as_str())
        .filter(|text| !text.trim().is_empty())
        .collect::<Vec<_>>();
    if parts.is_empty() {
        return None;
    }
    Some(parts.join(""))
}

fn insert_shared_parameter(
    items: &[&Map<String, Value>],
    parameters: &mut Map<String, Value>,
    key: &str,
) -> Result<(), TransportRequestBodySemanticsError> {
    let mut value: Option<Value> = None;
    for item in items {
        let Some(next) = item.get(key) else {
            continue;
        };
        match &value {
            Some(current) if current != next => {
                return Err(TransportRequestBodySemanticsError::new(
                    "Vertex Gemini embedding batch items must use the same shared parameters",
                ));
            }
            None => value = Some(next.clone()),
            _ => {}
        }
    }
    if let Some(value) = value {
        parameters.insert(key.to_string(), value);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::apply_transport_request_body_semantics;
    use crate::snapshot::{
        GatewayProviderTransportEndpoint, GatewayProviderTransportKey,
        GatewayProviderTransportProvider, GatewayProviderTransportSnapshot,
    };

    fn sample_transport(provider_type: &str, base_url: &str) -> GatewayProviderTransportSnapshot {
        GatewayProviderTransportSnapshot {
            provider: GatewayProviderTransportProvider {
                id: "provider-1".to_string(),
                name: "provider".to_string(),
                provider_type: provider_type.to_string(),
                website: None,
                is_active: true,
                keep_priority_on_conversion: false,
                enable_format_conversion: true,
                concurrent_limit: None,
                max_retries: None,
                proxy: None,
                request_timeout_secs: None,
                stream_first_byte_timeout_secs: None,
                config: None,
            },
            endpoint: GatewayProviderTransportEndpoint {
                id: "endpoint-1".to_string(),
                provider_id: "provider-1".to_string(),
                api_format: "gemini:embedding".to_string(),
                api_family: Some("gemini".to_string()),
                endpoint_kind: Some("embedding".to_string()),
                is_active: true,
                base_url: base_url.to_string(),
                header_rules: None,
                body_rules: None,
                max_retries: None,
                custom_path: None,
                config: None,
                format_acceptance_config: None,
                proxy: None,
            },
            key: GatewayProviderTransportKey {
                id: "key-1".to_string(),
                provider_id: "provider-1".to_string(),
                name: "key".to_string(),
                auth_type: "api_key".to_string(),
                is_active: true,
                api_formats: Some(vec!["gemini:embedding".to_string()]),
                auth_type_by_format: None,
                allow_auth_channel_mismatch_formats: None,
                allowed_models: None,
                capabilities: None,
                rate_multipliers: None,
                global_priority_by_format: None,
                expires_at_unix_secs: None,
                proxy: None,
                fingerprint: None,
                upstream_metadata: None,
                decrypted_api_key: "secret".to_string(),
                decrypted_auth_config: None,
            },
        }
    }

    #[test]
    fn vertex_gemini_embedding_single_body_uses_predict_contract() {
        let transport = sample_transport("vertex_ai", "https://aiplatform.googleapis.com");
        let mut body = json!({
            "model": "gemini-embedding-2",
            "content": {"parts": [{"text": "hello"}]},
            "taskType": "RETRIEVAL_QUERY",
            "outputDimensionality": 768
        });

        apply_transport_request_body_semantics(&mut body, &transport, "gemini:embedding")
            .expect("body semantics should apply");

        assert!(body.get("model").is_none());
        assert!(body.get("content").is_none());
        assert_eq!(body["instances"][0]["content"], "hello");
        assert_eq!(body["instances"][0]["task_type"], "RETRIEVAL_QUERY");
        assert_eq!(body["parameters"]["outputDimensionality"], 768);
    }

    #[test]
    fn gemini_api_embedding_single_body_keeps_model_for_developer_api() {
        let transport =
            sample_transport("gemini", "https://generativelanguage.googleapis.com/v1beta");
        let mut body = json!({
            "model": "gemini-embedding-2",
            "content": {"parts": [{"text": "hello"}]}
        });

        apply_transport_request_body_semantics(&mut body, &transport, "gemini:embedding")
            .expect("developer API body should pass through");

        assert_eq!(body["model"], "gemini-embedding-2");
    }

    #[test]
    fn vertex_gemini_embedding_batch_body_uses_predict_instances() {
        let transport = sample_transport("vertex_ai", "https://aiplatform.googleapis.com");
        let mut body = json!({
            "requests": [
                {
                    "model": "models/gemini-embedding-2",
                    "content": {"parts": [{"text": "hello"}]}
                },
                {
                    "model": "models/gemini-embedding-2",
                    "content": {"parts": [{"text": "world"}]}
                }
            ]
        });

        apply_transport_request_body_semantics(&mut body, &transport, "gemini:embedding")
            .expect("batch body semantics should apply");

        assert!(body.get("requests").is_none());
        assert_eq!(body["instances"][0]["content"], "hello");
        assert_eq!(body["instances"][1]["content"], "world");
    }

    #[test]
    fn vertex_gemini_embedding_existing_predict_body_removes_duplicate_model() {
        let transport = sample_transport("vertex_ai", "https://aiplatform.googleapis.com");
        let mut body = json!({
            "model": "gemini-embedding-2",
            "instances": [
                {"content": "hello", "task_type": "RETRIEVAL_QUERY"}
            ],
            "parameters": {
                "outputDimensionality": 768
            }
        });

        apply_transport_request_body_semantics(&mut body, &transport, "gemini:embedding")
            .expect("existing predict body should be accepted");

        assert!(body.get("model").is_none());
        assert_eq!(body["instances"][0]["content"], "hello");
        assert_eq!(body["parameters"]["outputDimensionality"], 768);
    }

    #[test]
    fn vertex_gemini_embedding_existing_predict_body_rejects_unconsumed_fields() {
        let transport = sample_transport("vertex_ai", "https://aiplatform.googleapis.com");
        let mut body = json!({
            "model": "gemini-embedding-2",
            "instances": [
                {"content": "hello"}
            ],
            "input": "this field would not be consumed by Vertex predict"
        });

        let error =
            apply_transport_request_body_semantics(&mut body, &transport, "gemini:embedding")
                .expect_err("predict body must not carry unconsumed OpenAI fields");

        assert!(error.message().contains("unsupported top-level fields"));
        assert!(body.get("model").is_some());
    }

    #[test]
    fn vertex_gemini_embedding_rejects_unconverted_openai_body() {
        let transport = sample_transport("vertex_ai", "https://aiplatform.googleapis.com");
        let mut body = json!({
            "model": "gemini-embedding-2",
            "input": "hello"
        });

        let error =
            apply_transport_request_body_semantics(&mut body, &transport, "gemini:embedding")
                .expect_err("OpenAI embedding body must not be sent to Vertex native predict");

        assert!(error.message().contains("cannot be mapped"));
        assert!(body.get("input").is_some());
    }
}
