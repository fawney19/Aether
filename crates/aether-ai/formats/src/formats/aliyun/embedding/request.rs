use serde_json::{Map, Value};

use crate::formats::context::FormatContext;
use crate::formats::openai::embedding::request::{mapped_embedding_model, namespace_extensions};
use crate::protocol::canonical::{
    CanonicalEmbeddingContent, CanonicalEmbeddingInput, CanonicalRequest,
};

pub fn from(body: &Value, _ctx: &FormatContext) -> Option<CanonicalRequest> {
    from_raw(body)
}

pub fn from_raw(body_json: &Value) -> Option<CanonicalRequest> {
    let request = body_json.as_object()?;
    let model = request
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?
        .to_string();
    let contents = request
        .get("input")
        .and_then(Value::as_object)
        .and_then(|input| input.get("contents"))
        .and_then(Value::as_array)?;
    let input = contents_to_embedding_input(contents)?;
    let mut parameters = request
        .get("parameters")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let dimensions = parameters
        .remove("dimension")
        .or_else(|| request.get("dimensions").cloned())
        .and_then(|value| value.as_u64());
    let parameters = (!parameters.is_empty()).then_some(parameters);
    Some(CanonicalRequest {
        model,
        embedding: Some(crate::protocol::canonical::CanonicalEmbeddingRequest {
            input,
            encoding_format: None,
            dimensions,
            task: None,
            user: None,
            parameters,
            extensions: namespace_extensions(
                "aliyun",
                request,
                &["model", "input", "parameters", "dimensions"],
            ),
        }),
        ..CanonicalRequest::default()
    })
}

pub fn to(request: &CanonicalRequest, ctx: &FormatContext) -> Option<Value> {
    let embedding = request.embedding.as_ref()?;
    let contents = embedding_input_to_contents(&embedding.input)?;
    if contents.is_empty() {
        return None;
    }

    let mut output = Map::new();
    output.insert(
        "model".to_string(),
        Value::String(mapped_embedding_model(
            request,
            ctx.mapped_model_or(request.model.as_str()),
        )),
    );
    output.insert(
        "input".to_string(),
        Value::Object(Map::from_iter([(
            "contents".to_string(),
            Value::Array(contents),
        )])),
    );

    let mut parameters = embedding.parameters.clone().unwrap_or_default();
    if let Some(dimensions) = embedding.dimensions {
        parameters
            .entry("dimension".to_string())
            .or_insert_with(|| Value::from(dimensions));
    }
    if !parameters.is_empty() {
        output.insert("parameters".to_string(), Value::Object(parameters));
    }

    Some(Value::Object(output))
}

fn contents_to_embedding_input(contents: &[Value]) -> Option<CanonicalEmbeddingInput> {
    if contents.is_empty() {
        return None;
    }
    let parsed = contents
        .iter()
        .map(embedding_content_from_value)
        .collect::<Option<Vec<_>>>()?;
    if parsed.iter().all(|content| {
        content.image.is_none() && content.video.is_none() && content.multi_images.is_none()
    }) {
        return Some(CanonicalEmbeddingInput::StringArray(
            parsed
                .into_iter()
                .map(|content| content.text)
                .collect::<Option<Vec<_>>>()?,
        ));
    }
    Some(CanonicalEmbeddingInput::Multimodal(parsed))
}

fn embedding_content_from_value(value: &Value) -> Option<CanonicalEmbeddingContent> {
    let object = value.as_object()?;
    let content = CanonicalEmbeddingContent {
        text: object
            .get("text")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned),
        image: object
            .get("image")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned),
        video: object
            .get("video")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned),
        multi_images: match object.get("multi_images").and_then(Value::as_array) {
            Some(values) => Some(
                values
                    .iter()
                    .map(|value| {
                        value
                            .as_str()
                            .map(str::trim)
                            .filter(|value| !value.is_empty())
                            .map(ToOwned::to_owned)
                    })
                    .collect::<Option<Vec<_>>>()?,
            ),
            None => None,
        },
    };
    (!content.is_empty()).then_some(content)
}

fn embedding_input_to_contents(input: &CanonicalEmbeddingInput) -> Option<Vec<Value>> {
    match input {
        CanonicalEmbeddingInput::String(text) => {
            non_empty_text_content(text).map(|content| vec![content])
        }
        CanonicalEmbeddingInput::StringArray(items) => items
            .iter()
            .map(|text| non_empty_text_content(text))
            .collect(),
        CanonicalEmbeddingInput::Multimodal(items) => {
            items.iter().map(multimodal_content_to_value).collect()
        }
        CanonicalEmbeddingInput::TokenArray(_) | CanonicalEmbeddingInput::TokenArrayArray(_) => {
            None
        }
    }
}

fn non_empty_text_content(text: &str) -> Option<Value> {
    let text = text.trim();
    if text.is_empty() {
        None
    } else {
        Some(Value::Object(Map::from_iter([(
            "text".to_string(),
            Value::String(text.to_string()),
        )])))
    }
}

fn multimodal_content_to_value(content: &CanonicalEmbeddingContent) -> Option<Value> {
    if content.is_empty() {
        return None;
    }
    let mut object = Map::new();
    if let Some(text) = content
        .text
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        object.insert("text".to_string(), Value::String(text.to_string()));
    }
    if let Some(image) = content
        .image
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        object.insert("image".to_string(), Value::String(image.to_string()));
    }
    if let Some(video) = content
        .video
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        object.insert("video".to_string(), Value::String(video.to_string()));
    }
    if let Some(multi_images) = content
        .multi_images
        .as_ref()
        .filter(|values| !values.is_empty() && values.iter().all(|value| !value.trim().is_empty()))
    {
        object.insert(
            "multi_images".to_string(),
            Value::Array(
                multi_images
                    .iter()
                    .map(|value| Value::String(value.trim().to_string()))
                    .collect(),
            ),
        );
    }
    if object.is_empty() {
        None
    } else {
        Some(Value::Object(object))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::{json, Map, Value};

    use super::to;
    use crate::formats::context::FormatContext;
    use crate::protocol::canonical::{
        CanonicalEmbeddingContent, CanonicalEmbeddingInput, CanonicalEmbeddingRequest,
        CanonicalRequest,
    };

    fn canonical_embedding(input: CanonicalEmbeddingInput) -> CanonicalRequest {
        CanonicalRequest {
            model: "text-embedding-3-small".to_string(),
            embedding: Some(CanonicalEmbeddingRequest {
                input,
                encoding_format: None,
                dimensions: None,
                task: None,
                user: None,
                parameters: None,
                extensions: BTreeMap::new(),
            }),
            ..CanonicalRequest::default()
        }
    }

    #[test]
    fn text_input_uses_dashscope_contents() {
        let request = canonical_embedding(CanonicalEmbeddingInput::StringArray(vec![
            "alpha".to_string(),
            "beta".to_string(),
        ]));

        let body = to(
            &request,
            &FormatContext::default().with_mapped_model("qwen3-vl-embedding"),
        )
        .expect("aliyun request");

        assert_eq!(body["model"], "qwen3-vl-embedding");
        assert_eq!(
            body["input"]["contents"],
            json!([{ "text": "alpha" }, { "text": "beta" }])
        );
    }

    #[test]
    fn multimodal_input_and_parameters_use_dashscope_contract() {
        let mut request = canonical_embedding(CanonicalEmbeddingInput::Multimodal(vec![
            CanonicalEmbeddingContent {
                text: Some("white running shoes".to_string()),
                image: None,
                video: None,
                multi_images: None,
            },
            CanonicalEmbeddingContent {
                text: None,
                image: Some("https://example.com/shoe.png".to_string()),
                video: None,
                multi_images: None,
            },
            CanonicalEmbeddingContent {
                text: None,
                image: None,
                video: None,
                multi_images: Some(vec![
                    "https://example.com/a.png".to_string(),
                    "https://example.com/b.png".to_string(),
                ]),
            },
        ]));
        let embedding = request.embedding.as_mut().expect("embedding request");
        embedding.dimensions = Some(1024);
        embedding.parameters = Some(Map::from_iter([
            ("enable_fusion".to_string(), Value::Bool(true)),
            ("res_level".to_string(), Value::from(2_u64)),
            ("max_video_frames".to_string(), Value::from(64_u64)),
        ]));

        let body = to(
            &request,
            &FormatContext::default().with_mapped_model("qwen3-vl-embedding"),
        )
        .expect("aliyun request");

        assert_eq!(
            body["input"]["contents"],
            json!([
                { "text": "white running shoes" },
                { "image": "https://example.com/shoe.png" },
                { "multi_images": ["https://example.com/a.png", "https://example.com/b.png"] }
            ])
        );
        assert_eq!(body["parameters"]["dimension"], 1024);
        assert_eq!(body["parameters"]["enable_fusion"], true);
        assert_eq!(body["parameters"]["res_level"], 2);
        assert_eq!(body["parameters"]["max_video_frames"], 64);
    }

    #[test]
    fn parameter_dimension_wins_over_openai_dimensions() {
        let mut request = canonical_embedding(CanonicalEmbeddingInput::String("alpha".to_string()));
        let embedding = request.embedding.as_mut().expect("embedding request");
        embedding.dimensions = Some(1024);
        embedding.parameters = Some(Map::from_iter([(
            "dimension".to_string(),
            Value::from(512_u64),
        )]));

        let body = to(
            &request,
            &FormatContext::default().with_mapped_model("qwen3-vl-embedding"),
        )
        .expect("aliyun request");

        assert_eq!(body["parameters"]["dimension"], 512);
    }

    #[test]
    fn token_arrays_are_not_convertible() {
        let request = canonical_embedding(CanonicalEmbeddingInput::TokenArray(vec![1, 2, 3]));
        assert!(to(
            &request,
            &FormatContext::default().with_mapped_model("qwen3-vl-embedding"),
        )
        .is_none());
    }
}
