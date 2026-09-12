use base64::Engine as _;
use hmac::{Hmac, Mac};
use sha2::{Digest as _, Sha256};

pub const INTERNAL_GATEWAY_AUTH_TIMESTAMP_HEADER: &str = "x-aether-internal-gateway-timestamp";
pub const INTERNAL_GATEWAY_AUTH_NONCE_HEADER: &str = "x-aether-internal-gateway-nonce";
pub const INTERNAL_GATEWAY_AUTH_SIGNATURE_HEADER: &str = "x-aether-internal-gateway-signature";

const INTERNAL_GATEWAY_AUTH_CONTEXT: &[u8] = b"aether-internal-gateway-auth-v1";

type HmacSha256 = Hmac<Sha256>;

pub fn sign_internal_gateway_request(
    secret: &[u8],
    method: &str,
    path_and_query: &str,
    timestamp_unix_secs: u64,
    nonce: &str,
    body: &[u8],
) -> String {
    let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC accepts keys of any size");
    update_auth_mac(
        &mut mac,
        method,
        path_and_query,
        timestamp_unix_secs,
        nonce,
        body,
    );
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
}

pub fn verify_internal_gateway_request_signature(
    secret: &[u8],
    method: &str,
    path_and_query: &str,
    timestamp_unix_secs: u64,
    nonce: &str,
    body: &[u8],
    signature: &str,
) -> bool {
    let signature = signature.trim();
    if signature.len() > 43 {
        return false;
    }
    let Ok(signature) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(signature) else {
        return false;
    };
    let Ok(mut mac) = HmacSha256::new_from_slice(secret) else {
        return false;
    };
    update_auth_mac(
        &mut mac,
        method,
        path_and_query,
        timestamp_unix_secs,
        nonce,
        body,
    );
    mac.verify_slice(&signature).is_ok()
}

fn update_auth_mac(
    mac: &mut HmacSha256,
    method: &str,
    path_and_query: &str,
    timestamp_unix_secs: u64,
    nonce: &str,
    body: &[u8],
) {
    mac.update(INTERNAL_GATEWAY_AUTH_CONTEXT);
    update_auth_field(mac, method.as_bytes());
    update_auth_field(mac, path_and_query.as_bytes());
    mac.update(&timestamp_unix_secs.to_be_bytes());
    update_auth_field(mac, nonce.as_bytes());
    mac.update(&(body.len() as u64).to_be_bytes());
    mac.update(&Sha256::digest(body));
}

fn update_auth_field(mac: &mut HmacSha256, value: &[u8]) {
    mac.update(&(value.len() as u64).to_be_bytes());
    mac.update(value);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_binds_every_security_relevant_field() {
        let secret = b"internal-gateway-test-secret-32-bytes-minimum";
        let body = br#"{"path":"/v1/models"}"#;
        let timestamp = 1_800_000_000;
        let nonce = "nonce-value-0000000000000001";
        let path = "/api/internal/gateway/resolve?mode=full";
        let signature = sign_internal_gateway_request(secret, "POST", path, timestamp, nonce, body);

        assert!(verify_internal_gateway_request_signature(
            secret, "POST", path, timestamp, nonce, body, &signature,
        ));
        assert!(!verify_internal_gateway_request_signature(
            secret, "GET", path, timestamp, nonce, body, &signature,
        ));
        assert!(!verify_internal_gateway_request_signature(
            secret,
            "POST",
            "/api/internal/gateway/resolve?mode=brief",
            timestamp,
            nonce,
            body,
            &signature,
        ));
        assert!(!verify_internal_gateway_request_signature(
            secret,
            "POST",
            path,
            timestamp + 1,
            nonce,
            body,
            &signature,
        ));
        assert!(!verify_internal_gateway_request_signature(
            secret,
            "POST",
            path,
            timestamp,
            "different-nonce-0000000000001",
            body,
            &signature,
        ));
        assert!(!verify_internal_gateway_request_signature(
            secret,
            "POST",
            path,
            timestamp,
            nonce,
            br#"{"path":"/v1/providers"}"#,
            &signature,
        ));
    }
}
