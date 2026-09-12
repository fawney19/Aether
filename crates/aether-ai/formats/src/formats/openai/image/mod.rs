use url::Url;

pub mod request;
pub mod spec;
pub mod stream;

pub(crate) const MAX_OPENAI_IMAGE_DATA_BYTES: usize = 64 * 1024 * 1024;
pub(crate) const MAX_OPENAI_IMAGE_EXTERNAL_URL_BYTES: usize = 64 * 1024;
pub(crate) const MAX_OPENAI_IMAGE_REVISED_PROMPT_BYTES: usize = 256 * 1024;

pub(crate) fn is_safe_openai_image_base64_payload(value: &str) -> bool {
    if value.is_empty()
        || value.len() > MAX_OPENAI_IMAGE_DATA_BYTES
        || value.len() % 4 == 1
        || value
            .bytes()
            .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control())
    {
        return false;
    }
    let bytes = value.as_bytes();
    let first_padding = bytes.iter().position(|byte| *byte == b'=');
    if let Some(index) = first_padding {
        let padding = bytes.len() - index;
        if padding > 2
            || bytes[index..].iter().any(|byte| *byte != b'=')
            || !bytes.len().is_multiple_of(4)
        {
            return false;
        }
    }
    bytes[..first_padding.unwrap_or(bytes.len())]
        .iter()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(*byte, b'+' | b'/'))
}

pub(crate) fn normalize_openai_image_output_format(value: &str) -> Option<&'static str> {
    let value = value.trim();
    if value.eq_ignore_ascii_case("png") {
        Some("png")
    } else if value.eq_ignore_ascii_case("jpeg") || value.eq_ignore_ascii_case("jpg") {
        Some("jpeg")
    } else if value.eq_ignore_ascii_case("webp") {
        Some("webp")
    } else {
        None
    }
}

pub(crate) fn bounded_openai_image_revised_prompt(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty() && value.len() <= MAX_OPENAI_IMAGE_REVISED_PROMPT_BYTES).then_some(value)
}

pub(crate) fn parse_safe_openai_image_data_url(value: &str) -> Option<(&'static str, &str)> {
    let (metadata, payload) = value.trim().split_once(',')?;
    let mime_type = metadata.strip_prefix("data:")?.strip_suffix(";base64")?;
    let mime_type = safe_openai_image_mime_type(mime_type.trim())?;
    (!payload.is_empty() && is_safe_openai_image_base64_payload(payload))
        .then_some((mime_type, payload))
}

pub(crate) fn safe_openai_image_mime_type(value: &str) -> Option<&'static str> {
    if value.eq_ignore_ascii_case("image/png") {
        Some("image/png")
    } else if value.eq_ignore_ascii_case("image/jpeg") || value.eq_ignore_ascii_case("image/jpg") {
        Some("image/jpeg")
    } else if value.eq_ignore_ascii_case("image/webp") {
        Some("image/webp")
    } else {
        None
    }
}

pub(crate) fn sanitize_openai_image_source_url(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty()
        || value
            .chars()
            .any(|character| character.is_ascii_control() || character.is_whitespace())
    {
        return None;
    }
    if let Some((mime_type, payload)) = parse_safe_openai_image_data_url(value) {
        return Some(format!("data:{mime_type};base64,{payload}"));
    }
    if value.len() > MAX_OPENAI_IMAGE_EXTERNAL_URL_BYTES {
        return None;
    }
    let parsed = Url::parse(value).ok()?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return None;
    }
    Some(value.to_string())
}
