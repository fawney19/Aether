use std::collections::BTreeMap;

use super::auth::{KiloFreeRequestAuth, KILO_FREE_BEARER_TOKEN};

const KILO_FREE_USER_AGENT: &str = "aether-kilo-free";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KiloFreeRequestEnvelopeSupport {
    Supported(BTreeMap<String, String>),
}

pub fn build_kilo_free_request_headers(
    _auth: &KiloFreeRequestAuth,
) -> KiloFreeRequestEnvelopeSupport {
    let headers = BTreeMap::from([
        (
            String::from("authorization"),
            format!("Bearer {}", KILO_FREE_BEARER_TOKEN),
        ),
        (
            String::from("content-type"),
            String::from("application/json"),
        ),
        (
            String::from("user-agent"),
            String::from(KILO_FREE_USER_AGENT),
        ),
    ]);

    KiloFreeRequestEnvelopeSupport::Supported(headers)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_auth() -> KiloFreeRequestAuth {
        KiloFreeRequestAuth {
            bearer_token: KILO_FREE_BEARER_TOKEN.to_string(),
        }
    }

    #[test]
    fn test_bearer_auth() {
        let result = build_kilo_free_request_headers(&sample_auth());
        match result {
            KiloFreeRequestEnvelopeSupport::Supported(headers) => {
                assert_eq!(
                    headers.get("authorization"),
                    Some(&"Bearer free".to_string())
                );
            }
        }
    }

    #[test]
    fn test_content_type() {
        let result = build_kilo_free_request_headers(&sample_auth());
        match result {
            KiloFreeRequestEnvelopeSupport::Supported(headers) => {
                assert_eq!(
                    headers.get("content-type"),
                    Some(&"application/json".to_string())
                );
            }
        }
    }

    #[test]
    fn test_user_agent() {
        let result = build_kilo_free_request_headers(&sample_auth());
        match result {
            KiloFreeRequestEnvelopeSupport::Supported(headers) => {
                assert_eq!(
                    headers.get("user-agent"),
                    Some(&"aether-kilo-free".to_string())
                );
            }
        }
    }
}
