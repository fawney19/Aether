use crate::handlers::admin::provider::shared::model_test_capabilities::{
    admin_provider_openai_image_normalize_options, admin_provider_openai_image_test_capability,
    AdminProviderOpenAiImageTestCapability,
};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ProviderQueryOpenAiImageTestCapability(AdminProviderOpenAiImageTestCapability);

pub(super) fn provider_query_openai_image_test_capability(
    provider_type: &str,
    provider_model: Option<&str>,
) -> ProviderQueryOpenAiImageTestCapability {
    ProviderQueryOpenAiImageTestCapability(admin_provider_openai_image_test_capability(
        provider_type,
        provider_model,
    ))
}

pub(super) fn provider_query_openai_image_normalize_options(
    provider_type: &str,
    provider_model: Option<&str>,
) -> crate::ai_serving::OpenAiImageNormalizeOptions {
    admin_provider_openai_image_normalize_options(provider_type, provider_model)
}

pub(super) fn provider_query_openai_image_requested_count(request_body: &Value) -> Option<u64> {
    request_body.get("n").and_then(|value| {
        value.as_u64().or_else(|| {
            value
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .and_then(|value| value.parse::<u64>().ok())
        })
    })
}

pub(super) fn provider_query_openai_image_normalize_failure_message(
    provider_type: &str,
    provider_model: Option<&str>,
    request_body: &Value,
) -> String {
    let capability = provider_query_openai_image_test_capability(provider_type, provider_model);
    if provider_query_openai_image_requested_count(request_body)
        .is_some_and(|value| !capability.0.supports_generation_count(value))
    {
        return format!(
            "Provider request body could not be normalized for openai:image: selected provider supports n=1..{} for generation",
            capability.0.max_generation_count
        );
    }
    "Provider request body could not be normalized for openai:image".to_string()
}
