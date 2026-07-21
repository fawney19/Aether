use std::fmt::Write as _;

use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextPayloadSummary {
    pub bytes: usize,
    pub sha256: String,
}

pub fn summarize_text_payload(text: &str) -> TextPayloadSummary {
    let digest = Sha256::digest(text.as_bytes());
    let mut sha256 = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut sha256, "{byte:02x}").expect("writing to string should not fail");
    }
    TextPayloadSummary {
        bytes: text.len(),
        sha256,
    }
}

#[cfg(test)]
mod tests {
    use super::summarize_text_payload;

    #[test]
    fn summarizes_text_payload_without_exposing_content() {
        let summary = summarize_text_payload("secret-body");
        assert_eq!(summary.bytes, 11);
        assert_eq!(
            summary.sha256,
            "7c3029502007b2beae470d090221f0a8f7708be361be4662f4b649426e5767b3"
        );
    }
}
