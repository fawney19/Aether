pub fn short_request_id(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return "-".to_string();
    }

    if trimmed.chars().count() <= 12 {
        return trimmed.to_string();
    }

    if looks_like_uuid(trimmed) {
        return trimmed.chars().take(8).collect();
    }

    let prefix: String = trimmed.chars().take(6).collect();
    let suffix: String = trimmed
        .chars()
        .rev()
        .take(4)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    format!("{prefix}...{suffix}")
}

fn looks_like_uuid(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 36 {
        return false;
    }

    for (index, byte) in bytes.iter().enumerate() {
        let is_hyphen = matches!(index, 8 | 13 | 18 | 23);
        if is_hyphen {
            if *byte != b'-' {
                return false;
            }
            continue;
        }

        if !byte.is_ascii_hexdigit() {
            return false;
        }
    }

    true
}

#[cfg(test)]
mod tests {
    use super::short_request_id;

    #[test]
    fn shortens_uuid_like_request_ids_to_prefix() {
        assert_eq!(
            short_request_id("d07e1e94-41b8-409f-a18a-27993ae7ecb1"),
            "d07e1e94"
        );
    }

    #[test]
    fn shortens_long_named_request_ids_to_prefix_and_suffix() {
        assert_eq!(
            short_request_id("trace-openai-cli-stream-sync-direct-123"),
            "trace-...-123"
        );
    }

    #[test]
    fn preserves_short_request_ids() {
        assert_eq!(short_request_id("req-123"), "req-123");
    }
}
