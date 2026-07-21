use crate::ai_serving::OpenAiImageNormalizeOptions;

const DEFAULT_IMAGE_MAX_GENERATION_COUNT: u64 = 1;
const OPENAI_IMAGE_MAX_GENERATION_COUNT: u64 = 10;
const GROK_OPENAI_IMAGE_MAX_GENERATION_COUNT: u64 = 4;

pub(crate) fn openai_image_gateway_max_generation_count() -> u64 {
    OPENAI_IMAGE_MAX_GENERATION_COUNT
}

pub(crate) fn openai_image_provider_max_generation_count(provider_type: &str) -> u64 {
    if provider_type.trim().eq_ignore_ascii_case("grok") {
        GROK_OPENAI_IMAGE_MAX_GENERATION_COUNT
    } else if matches!(
        provider_type.trim().to_ascii_lowercase().as_str(),
        "openai" | "codex"
    ) {
        OPENAI_IMAGE_MAX_GENERATION_COUNT
    } else {
        DEFAULT_IMAGE_MAX_GENERATION_COUNT
    }
}

pub(crate) fn openai_image_provider_max_generation_count_for_model(
    provider_type: &str,
    provider_model: Option<&str>,
) -> u64 {
    let provider_limit = openai_image_provider_max_generation_count(provider_type);
    provider_model.map_or(provider_limit, |model| {
        if is_dall_e_3_model(model) {
            1
        } else {
            provider_limit
        }
    })
}

pub(crate) fn openai_image_normalize_options_for_provider(
    provider_type: &str,
    provider_model: Option<&str>,
) -> OpenAiImageNormalizeOptions {
    OpenAiImageNormalizeOptions::with_max_generation_count(
        openai_image_provider_max_generation_count_for_model(provider_type, provider_model),
    )
}

fn is_dall_e_3_model(model: &str) -> bool {
    model.trim().to_ascii_lowercase().starts_with("dall-e-3")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_count_capabilities_follow_provider_and_model_contracts() {
        assert_eq!(openai_image_gateway_max_generation_count(), 10);
        assert_eq!(openai_image_provider_max_generation_count("grok"), 4);
        assert_eq!(openai_image_provider_max_generation_count("openai"), 10);
        assert_eq!(openai_image_provider_max_generation_count("codex"), 10);
        assert_eq!(openai_image_provider_max_generation_count("custom"), 1);
        assert_eq!(
            openai_image_provider_max_generation_count_for_model("openai", Some("dall-e-3")),
            1
        );
        assert_eq!(
            openai_image_normalize_options_for_provider("openai", Some("dall-e-3")),
            OpenAiImageNormalizeOptions::with_max_generation_count(1)
        );
    }
}
