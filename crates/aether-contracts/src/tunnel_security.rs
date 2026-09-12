use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use base64::Engine;
use bytes::{Buf, BufMut, Bytes, BytesMut};
use hmac::{Hmac, Mac};
use sha2::{Digest as _, Sha256};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::tunnel::{Frame, MsgType, HEADER_SIZE};

type HmacSha256 = Hmac<Sha256>;

pub const TUNNEL_SECURITY_HEADER: &str = "x-aether-tunnel-security";
pub const TUNNEL_SECURITY_SESSION_HEADER: &str = "x-aether-tunnel-security-session";
pub const TUNNEL_SECURITY_PROOF_TIMESTAMP_HEADER: &str = "x-aether-tunnel-security-proof-timestamp";
pub const TUNNEL_SECURITY_PROOF_NONCE_HEADER: &str = "x-aether-tunnel-security-proof-nonce";
pub const TUNNEL_SECURITY_PROOF_SIGNATURE_HEADER: &str = "x-aether-tunnel-security-proof-signature";
pub const TUNNEL_GENERATION_HEADER: &str = "x-aether-tunnel-generation";
pub const TUNNEL_CONTROL_PLANE_NODE_ID_HEADER: &str = "x-aether-tunnel-control-plane-node-id";
pub const TUNNEL_CONTROL_PLANE_GENERATION_HEADER: &str = "x-aether-tunnel-control-plane-generation";
pub const TUNNEL_CONTROL_PLANE_TIMESTAMP_HEADER: &str = "x-aether-tunnel-control-plane-timestamp";
pub const TUNNEL_CONTROL_PLANE_NONCE_HEADER: &str = "x-aether-tunnel-control-plane-nonce";
pub const TUNNEL_CONTROL_PLANE_SIGNATURE_HEADER: &str = "x-aether-tunnel-control-plane-signature";
pub const TUNNEL_SECURITY_NON_TLS_REQUIRED: &str = "non_tls_required";
pub const FLAG_ENCRYPTED: u8 = 0x04;

const CONTEXT: &[u8] = b"aether-tunnel-secure-v1";
const HANDSHAKE_PROOF_CONTEXT: &[u8] = b"aether-tunnel-handshake-proof-v1";
const CONTROL_PLANE_AUTH_CONTEXT: &[u8] = b"aether-tunnel-control-plane-auth-v1";
const CLIENT_TO_SERVER_LABEL: &[u8] = b"client-to-server";
const SERVER_TO_CLIENT_LABEL: &[u8] = b"server-to-client";
const CLIENT_TO_SERVER_NONCE_PREFIX: [u8; 4] = *b"c2s1";
const SERVER_TO_CLIENT_NONCE_PREFIX: [u8; 4] = *b"s2c1";
const SEQUENCE_LEN: usize = 8;
const NONCE_LEN: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunnelSecurityRole {
    Client,
    Server,
}

#[derive(Debug, thiserror::Error)]
pub enum TunnelSecurityError {
    #[error("tunnel_encryption_key must be base64-encoded 32 bytes")]
    InvalidKey,
    #[error("tunnel security session id must not be empty")]
    InvalidSession,
    #[error("secure tunnel frame is missing encrypted flag")]
    MissingEncryptedFlag,
    #[error("secure tunnel frame payload is too short")]
    PayloadTooShort,
    #[error("secure tunnel frame sequence is not the expected next value")]
    UnexpectedSequence,
    #[error("secure tunnel frame sequence space is exhausted")]
    SequenceExhausted,
    #[error("secure tunnel frame encryption failed")]
    Encrypt,
    #[error("secure tunnel frame decryption failed")]
    Decrypt,
}

pub struct SecureFrameCodec {
    seal: Aes256Gcm,
    open: Aes256Gcm,
    seal_prefix: [u8; 4],
    open_prefix: [u8; 4],
    next_sequence: AtomicU64,
    next_open_sequence: AtomicU64,
}

impl SecureFrameCodec {
    pub fn new(
        key: &str,
        session_id: &str,
        role: TunnelSecurityRole,
    ) -> Result<Self, TunnelSecurityError> {
        let psk = decode_psk(key)?;
        let session_id = session_id.trim();
        if session_id.is_empty() {
            return Err(TunnelSecurityError::InvalidSession);
        }

        let client_to_server = derive_key(&psk, session_id.as_bytes(), CLIENT_TO_SERVER_LABEL);
        let server_to_client = derive_key(&psk, session_id.as_bytes(), SERVER_TO_CLIENT_LABEL);
        let (seal_key, open_key, seal_prefix, open_prefix) = match role {
            TunnelSecurityRole::Client => (
                client_to_server,
                server_to_client,
                CLIENT_TO_SERVER_NONCE_PREFIX,
                SERVER_TO_CLIENT_NONCE_PREFIX,
            ),
            TunnelSecurityRole::Server => (
                server_to_client,
                client_to_server,
                SERVER_TO_CLIENT_NONCE_PREFIX,
                CLIENT_TO_SERVER_NONCE_PREFIX,
            ),
        };

        Ok(Self {
            seal: Aes256Gcm::new_from_slice(&seal_key)
                .map_err(|_| TunnelSecurityError::InvalidKey)?,
            open: Aes256Gcm::new_from_slice(&open_key)
                .map_err(|_| TunnelSecurityError::InvalidKey)?,
            seal_prefix,
            open_prefix,
            next_sequence: AtomicU64::new(0),
            next_open_sequence: AtomicU64::new(0),
        })
    }

    pub fn encrypt_frame(&self, frame: Frame) -> Result<Bytes, TunnelSecurityError> {
        let sequence = self
            .next_sequence
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_add(1)
            })
            .map_err(|_| TunnelSecurityError::SequenceExhausted)?;
        let nonce_bytes = nonce_bytes(self.seal_prefix, sequence);
        let nonce = Nonce::from_slice(&nonce_bytes);
        let clear_flags = frame.flags & !FLAG_ENCRYPTED;
        let aad = frame_aad(frame.stream_id, frame.msg_type, clear_flags);
        let ciphertext = self
            .seal
            .encrypt(
                nonce,
                Payload {
                    msg: &frame.payload,
                    aad: &aad,
                },
            )
            .map_err(|_| TunnelSecurityError::Encrypt)?;

        let mut payload = BytesMut::with_capacity(SEQUENCE_LEN + ciphertext.len());
        payload.put_u64(sequence);
        payload.extend_from_slice(&ciphertext);
        Ok(Frame::new(
            frame.stream_id,
            frame.msg_type,
            clear_flags | FLAG_ENCRYPTED,
            payload.freeze(),
        )
        .encode())
    }

    pub fn decrypt_frame(&self, frame: Frame) -> Result<Frame, TunnelSecurityError> {
        if frame.flags & FLAG_ENCRYPTED == 0 {
            return Err(TunnelSecurityError::MissingEncryptedFlag);
        }
        if frame.payload.len() < SEQUENCE_LEN {
            return Err(TunnelSecurityError::PayloadTooShort);
        }

        let mut payload = frame.payload.clone();
        let sequence = payload.get_u64();
        let expected_sequence = self.next_open_sequence.load(Ordering::Relaxed);
        if sequence != expected_sequence {
            return Err(TunnelSecurityError::UnexpectedSequence);
        }
        let nonce_bytes = nonce_bytes(self.open_prefix, sequence);
        let nonce = Nonce::from_slice(&nonce_bytes);
        let clear_flags = frame.flags & !FLAG_ENCRYPTED;
        let aad = frame_aad(frame.stream_id, frame.msg_type, clear_flags);
        let plaintext = self
            .open
            .decrypt(
                nonce,
                Payload {
                    msg: &payload,
                    aad: &aad,
                },
            )
            .map_err(|_| TunnelSecurityError::Decrypt)?;
        let next_sequence = expected_sequence
            .checked_add(1)
            .ok_or(TunnelSecurityError::SequenceExhausted)?;
        self.next_open_sequence
            .compare_exchange(
                expected_sequence,
                next_sequence,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .map_err(|_| TunnelSecurityError::UnexpectedSequence)?;

        Ok(Frame::new(
            frame.stream_id,
            frame.msg_type,
            clear_flags,
            Bytes::from(plaintext),
        ))
    }
}

pub fn decode_psk(key: &str) -> Result<[u8; 32], TunnelSecurityError> {
    let key = key.trim();
    if key.len() > 44 {
        return Err(TunnelSecurityError::InvalidKey);
    }
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(key)
        .map_err(|_| TunnelSecurityError::InvalidKey)?;
    decoded
        .try_into()
        .map_err(|_| TunnelSecurityError::InvalidKey)
}

pub fn sign_tunnel_security_handshake(
    key: &str,
    node_id: &str,
    security_mode: &str,
    session_id: &str,
    protocol_version: u8,
    timestamp_unix_secs: u64,
    nonce: &str,
) -> Result<String, TunnelSecurityError> {
    sign_tunnel_security_handshake_for_generation(
        key,
        node_id,
        "",
        security_mode,
        session_id,
        protocol_version,
        timestamp_unix_secs,
        nonce,
    )
}

// These arguments are the versioned handshake transcript. Keep them explicit
// and ordered so existing clients and servers compute the same MAC.
#[allow(clippy::too_many_arguments)]
pub fn sign_tunnel_security_handshake_for_generation(
    key: &str,
    node_id: &str,
    tunnel_generation: &str,
    security_mode: &str,
    session_id: &str,
    protocol_version: u8,
    timestamp_unix_secs: u64,
    nonce: &str,
) -> Result<String, TunnelSecurityError> {
    let psk = decode_psk(key)?;
    let mut mac =
        <HmacSha256 as Mac>::new_from_slice(&psk).expect("HMAC accepts a 32-byte tunnel PSK");
    update_handshake_proof_mac(
        &mut mac,
        node_id,
        tunnel_generation,
        security_mode,
        session_id,
        protocol_version,
        timestamp_unix_secs,
        nonce,
    );
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes()))
}

// The verifier preserves the legacy public signature while delegating to the
// generation-aware transcript implementation.
#[allow(clippy::too_many_arguments)]
pub fn verify_tunnel_security_handshake(
    key: &str,
    node_id: &str,
    security_mode: &str,
    session_id: &str,
    protocol_version: u8,
    timestamp_unix_secs: u64,
    nonce: &str,
    signature: &str,
) -> bool {
    verify_tunnel_security_handshake_for_generation(
        key,
        node_id,
        "",
        security_mode,
        session_id,
        protocol_version,
        timestamp_unix_secs,
        nonce,
        signature,
    )
}

// This mirrors the signing API exactly; the argument order is part of the
// authenticated handshake format.
#[allow(clippy::too_many_arguments)]
pub fn verify_tunnel_security_handshake_for_generation(
    key: &str,
    node_id: &str,
    tunnel_generation: &str,
    security_mode: &str,
    session_id: &str,
    protocol_version: u8,
    timestamp_unix_secs: u64,
    nonce: &str,
    signature: &str,
) -> bool {
    let Ok(psk) = decode_psk(key) else {
        return false;
    };
    if signature.len() > 43 {
        return false;
    }
    let Ok(signature) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(signature) else {
        return false;
    };
    let Ok(mut mac) = <HmacSha256 as Mac>::new_from_slice(&psk) else {
        return false;
    };
    update_handshake_proof_mac(
        &mut mac,
        node_id,
        tunnel_generation,
        security_mode,
        session_id,
        protocol_version,
        timestamp_unix_secs,
        nonce,
    );
    mac.verify_slice(&signature).is_ok()
}

pub fn sign_tunnel_control_plane_request(
    key: &str,
    method: &str,
    path: &str,
    node_id: &str,
    timestamp_unix_secs: u64,
    nonce: &str,
    body: &[u8],
) -> Result<String, TunnelSecurityError> {
    sign_tunnel_control_plane_request_for_generation(
        key,
        method,
        path,
        node_id,
        "",
        timestamp_unix_secs,
        nonce,
        body,
    )
}

// Control-plane authentication signs these fields in this fixed order. Keep
// the public API stable instead of introducing a reordered parameter object.
#[allow(clippy::too_many_arguments)]
pub fn sign_tunnel_control_plane_request_for_generation(
    key: &str,
    method: &str,
    path: &str,
    node_id: &str,
    tunnel_generation: &str,
    timestamp_unix_secs: u64,
    nonce: &str,
    body: &[u8],
) -> Result<String, TunnelSecurityError> {
    let psk = decode_psk(key)?;
    let mut mac =
        <HmacSha256 as Mac>::new_from_slice(&psk).expect("HMAC accepts a 32-byte tunnel PSK");
    update_control_plane_auth_mac(
        &mut mac,
        method,
        path,
        node_id,
        tunnel_generation,
        timestamp_unix_secs,
        nonce,
        body,
    );
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes()))
}

// Preserve the legacy verifier signature; it must feed the same transcript as
// the corresponding signing function.
#[allow(clippy::too_many_arguments)]
pub fn verify_tunnel_control_plane_request(
    key: &str,
    method: &str,
    path: &str,
    node_id: &str,
    timestamp_unix_secs: u64,
    nonce: &str,
    body: &[u8],
    signature: &str,
) -> bool {
    verify_tunnel_control_plane_request_for_generation(
        key,
        method,
        path,
        node_id,
        "",
        timestamp_unix_secs,
        nonce,
        body,
        signature,
    )
}

// The generation-aware verifier intentionally mirrors the signer field order,
// which is part of the control-plane wire contract.
#[allow(clippy::too_many_arguments)]
pub fn verify_tunnel_control_plane_request_for_generation(
    key: &str,
    method: &str,
    path: &str,
    node_id: &str,
    tunnel_generation: &str,
    timestamp_unix_secs: u64,
    nonce: &str,
    body: &[u8],
    signature: &str,
) -> bool {
    let Ok(psk) = decode_psk(key) else {
        return false;
    };
    if signature.len() > 43 {
        return false;
    }
    let Ok(signature) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(signature) else {
        return false;
    };
    let Ok(mut mac) = <HmacSha256 as Mac>::new_from_slice(&psk) else {
        return false;
    };
    update_control_plane_auth_mac(
        &mut mac,
        method,
        path,
        node_id,
        tunnel_generation,
        timestamp_unix_secs,
        nonce,
        body,
    );
    mac.verify_slice(&signature).is_ok()
}

// Keep the MAC input fields separate and visibly ordered to avoid accidental
// changes to the authenticated control-plane transcript.
#[allow(clippy::too_many_arguments)]
fn update_control_plane_auth_mac(
    mac: &mut HmacSha256,
    method: &str,
    path: &str,
    node_id: &str,
    tunnel_generation: &str,
    timestamp_unix_secs: u64,
    nonce: &str,
    body: &[u8],
) {
    mac.update(CONTROL_PLANE_AUTH_CONTEXT);
    update_handshake_proof_field(mac, method.as_bytes());
    update_handshake_proof_field(mac, path.as_bytes());
    update_handshake_proof_field(mac, node_id.as_bytes());
    update_handshake_proof_field(mac, tunnel_generation.as_bytes());
    mac.update(&timestamp_unix_secs.to_be_bytes());
    update_handshake_proof_field(mac, nonce.as_bytes());
    mac.update(&Sha256::digest(body));
}

// This helper encodes the handshake transcript in a fixed cryptographic order;
// grouping arguments into a struct would obscure that wire-level contract.
#[allow(clippy::too_many_arguments)]
fn update_handshake_proof_mac(
    mac: &mut HmacSha256,
    node_id: &str,
    tunnel_generation: &str,
    security_mode: &str,
    session_id: &str,
    protocol_version: u8,
    timestamp_unix_secs: u64,
    nonce: &str,
) {
    mac.update(HANDSHAKE_PROOF_CONTEXT);
    update_handshake_proof_field(mac, node_id.as_bytes());
    update_handshake_proof_field(mac, tunnel_generation.as_bytes());
    update_handshake_proof_field(mac, security_mode.as_bytes());
    update_handshake_proof_field(mac, session_id.as_bytes());
    mac.update(&[protocol_version]);
    mac.update(&timestamp_unix_secs.to_be_bytes());
    update_handshake_proof_field(mac, nonce.as_bytes());
}

fn update_handshake_proof_field(mac: &mut HmacSha256, value: &[u8]) {
    mac.update(&(value.len() as u64).to_be_bytes());
    mac.update(value);
}

fn derive_key(psk: &[u8; 32], session_id: &[u8], label: &[u8]) -> [u8; 32] {
    let mut mac = <HmacSha256 as Mac>::new_from_slice(psk).expect("HMAC accepts 32-byte PSK");
    mac.update(CONTEXT);
    mac.update(&[0]);
    mac.update(session_id);
    mac.update(&[0]);
    mac.update(label);
    mac.finalize().into_bytes().into()
}

fn nonce_bytes(prefix: [u8; 4], sequence: u64) -> [u8; NONCE_LEN] {
    let mut nonce = [0_u8; NONCE_LEN];
    nonce[..4].copy_from_slice(&prefix);
    nonce[4..].copy_from_slice(&sequence.to_be_bytes());
    nonce
}

fn frame_aad(stream_id: u32, msg_type: MsgType, clear_flags: u8) -> [u8; HEADER_SIZE - 4] {
    let mut aad = [0_u8; HEADER_SIZE - 4];
    aad[..4].copy_from_slice(&stream_id.to_be_bytes());
    aad[4] = msg_type as u8;
    aad[5] = clear_flags;
    aad
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tunnel::{Frame, MsgType};

    fn test_key() -> String {
        base64::engine::general_purpose::STANDARD.encode([7_u8; 32])
    }

    #[test]
    fn secure_frame_round_trips_between_roles() {
        let client = SecureFrameCodec::new(&test_key(), "session-1", TunnelSecurityRole::Client)
            .expect("client codec");
        let server = SecureFrameCodec::new(&test_key(), "session-1", TunnelSecurityRole::Server)
            .expect("server codec");
        let frame = Frame::new(3, MsgType::RequestBody, 0, Bytes::from_static(b"secret"));

        let encrypted = client.encrypt_frame(frame).expect("encrypt");
        assert!(!encrypted.windows(b"secret".len()).any(|w| w == b"secret"));

        let wire = Frame::decode(encrypted).expect("wire frame");
        assert_ne!(wire.payload, Bytes::from_static(b"secret"));
        assert_ne!(wire.flags & FLAG_ENCRYPTED, 0);
        let decrypted = server.decrypt_frame(wire).expect("decrypt");

        assert_eq!(decrypted.stream_id, 3);
        assert_eq!(decrypted.msg_type, MsgType::RequestBody);
        assert_eq!(decrypted.flags, 0);
        assert_eq!(decrypted.payload, Bytes::from_static(b"secret"));
    }

    #[test]
    fn secure_frame_rejects_wrong_session() {
        let client = SecureFrameCodec::new(&test_key(), "session-1", TunnelSecurityRole::Client)
            .expect("client codec");
        let server = SecureFrameCodec::new(&test_key(), "session-2", TunnelSecurityRole::Server)
            .expect("server codec");
        let encrypted = client
            .encrypt_frame(Frame::new(
                1,
                MsgType::RequestBody,
                0,
                Bytes::from_static(b"secret"),
            ))
            .expect("encrypt");
        let wire = Frame::decode(encrypted).expect("wire frame");

        assert!(matches!(
            server.decrypt_frame(wire),
            Err(TunnelSecurityError::Decrypt)
        ));
    }

    #[test]
    fn secure_frame_rejects_replayed_sequence() {
        let client = SecureFrameCodec::new(&test_key(), "session-1", TunnelSecurityRole::Client)
            .expect("client codec");
        let server = SecureFrameCodec::new(&test_key(), "session-1", TunnelSecurityRole::Server)
            .expect("server codec");
        let encrypted = client
            .encrypt_frame(Frame::new(
                1,
                MsgType::RequestBody,
                0,
                Bytes::from_static(b"secret"),
            ))
            .expect("encrypt");
        let wire = Frame::decode(encrypted).expect("wire frame");

        server.decrypt_frame(wire.clone()).expect("first decrypt");
        assert!(matches!(
            server.decrypt_frame(wire),
            Err(TunnelSecurityError::UnexpectedSequence)
        ));
    }

    #[test]
    fn secure_frame_accepts_a_concurrent_sequence_only_once() {
        use std::sync::{Arc, Barrier};

        let client = SecureFrameCodec::new(&test_key(), "session-1", TunnelSecurityRole::Client)
            .expect("client codec");
        let server = Arc::new(
            SecureFrameCodec::new(&test_key(), "session-1", TunnelSecurityRole::Server)
                .expect("server codec"),
        );
        let encrypted = client
            .encrypt_frame(Frame::new(
                1,
                MsgType::RequestBody,
                0,
                Bytes::from_static(b"secret"),
            ))
            .expect("encrypt");
        let wire = Frame::decode(encrypted).expect("wire frame");
        let barrier = Arc::new(Barrier::new(3));
        let workers = (0..2)
            .map(|_| {
                let server = Arc::clone(&server);
                let barrier = Arc::clone(&barrier);
                let wire = wire.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    server.decrypt_frame(wire)
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        let results = workers
            .into_iter()
            .map(|worker| worker.join().expect("worker should not panic"))
            .collect::<Vec<_>>();

        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(result, Err(TunnelSecurityError::UnexpectedSequence)))
                .count(),
            1
        );
    }

    #[test]
    fn secure_frame_fails_closed_before_sequence_wrap() {
        let codec = SecureFrameCodec::new(&test_key(), "session-1", TunnelSecurityRole::Client)
            .expect("codec");
        codec.next_sequence.store(u64::MAX, Ordering::Relaxed);

        assert!(matches!(
            codec.encrypt_frame(Frame::new(
                1,
                MsgType::RequestBody,
                0,
                Bytes::from_static(b"secret"),
            )),
            Err(TunnelSecurityError::SequenceExhausted)
        ));
    }

    #[test]
    fn secure_frame_rejects_out_of_order_sequence_without_advancing() {
        let client = SecureFrameCodec::new(&test_key(), "session-1", TunnelSecurityRole::Client)
            .expect("client codec");
        let server = SecureFrameCodec::new(&test_key(), "session-1", TunnelSecurityRole::Server)
            .expect("server codec");
        let first = Frame::decode(
            client
                .encrypt_frame(Frame::new(
                    1,
                    MsgType::RequestBody,
                    0,
                    Bytes::from_static(b"first"),
                ))
                .expect("encrypt first"),
        )
        .expect("first wire frame");
        let second = Frame::decode(
            client
                .encrypt_frame(Frame::new(
                    1,
                    MsgType::RequestBody,
                    0,
                    Bytes::from_static(b"second"),
                ))
                .expect("encrypt second"),
        )
        .expect("second wire frame");

        assert!(matches!(
            server.decrypt_frame(second),
            Err(TunnelSecurityError::UnexpectedSequence)
        ));
        assert_eq!(
            server.decrypt_frame(first).expect("first decrypt").payload,
            Bytes::from_static(b"first")
        );
    }

    #[test]
    fn secure_frame_uses_session_in_key_derivation() {
        let session_a = "node-1:connection-a";
        let session_b = "node-1:connection-b";
        let client_a = SecureFrameCodec::new(&test_key(), session_a, TunnelSecurityRole::Client)
            .expect("client codec a");
        let client_b = SecureFrameCodec::new(&test_key(), session_b, TunnelSecurityRole::Client)
            .expect("client codec b");
        let frame = Frame::new(
            7,
            MsgType::RequestBody,
            0,
            Bytes::from_static(b"same payload"),
        );

        let encrypted_a = client_a.encrypt_frame(frame.clone()).expect("encrypt a");
        let encrypted_b = client_b.encrypt_frame(frame).expect("encrypt b");

        assert_ne!(encrypted_a, encrypted_b);
    }

    #[test]
    fn handshake_proof_round_trips_and_binds_every_field() {
        let key = test_key();
        let signature = sign_tunnel_security_handshake(
            &key,
            "node-1",
            TUNNEL_SECURITY_NON_TLS_REQUIRED,
            "0123456789abcdef0123456789abcdef",
            3,
            1_700_000_000,
            "abcdef0123456789abcdef0123456789",
        )
        .expect("sign handshake");
        assert_eq!(signature.len(), 43);
        assert!(verify_tunnel_security_handshake(
            &key,
            "node-1",
            TUNNEL_SECURITY_NON_TLS_REQUIRED,
            "0123456789abcdef0123456789abcdef",
            3,
            1_700_000_000,
            "abcdef0123456789abcdef0123456789",
            &signature,
        ));

        for valid in [
            verify_tunnel_security_handshake(
                &key,
                "node-2",
                TUNNEL_SECURITY_NON_TLS_REQUIRED,
                "0123456789abcdef0123456789abcdef",
                3,
                1_700_000_000,
                "abcdef0123456789abcdef0123456789",
                &signature,
            ),
            verify_tunnel_security_handshake(
                &key,
                "node-1",
                TUNNEL_SECURITY_NON_TLS_REQUIRED,
                "1123456789abcdef0123456789abcdef",
                3,
                1_700_000_000,
                "abcdef0123456789abcdef0123456789",
                &signature,
            ),
            verify_tunnel_security_handshake(
                &key,
                "node-1",
                TUNNEL_SECURITY_NON_TLS_REQUIRED,
                "0123456789abcdef0123456789abcdef",
                2,
                1_700_000_000,
                "abcdef0123456789abcdef0123456789",
                &signature,
            ),
            verify_tunnel_security_handshake(
                &key,
                "node-1",
                TUNNEL_SECURITY_NON_TLS_REQUIRED,
                "0123456789abcdef0123456789abcdef",
                3,
                1_700_000_001,
                "abcdef0123456789abcdef0123456789",
                &signature,
            ),
            verify_tunnel_security_handshake(
                &key,
                "node-1",
                TUNNEL_SECURITY_NON_TLS_REQUIRED,
                "0123456789abcdef0123456789abcdef",
                3,
                1_700_000_000,
                "bbcdef0123456789abcdef0123456789",
                &signature,
            ),
        ] {
            assert!(!valid, "tampered handshake field must fail verification");
        }
    }

    #[test]
    fn handshake_proof_rejects_wrong_key_and_malformed_signature() {
        let key = test_key();
        let signature = sign_tunnel_security_handshake(
            &key,
            "node-1",
            TUNNEL_SECURITY_NON_TLS_REQUIRED,
            "0123456789abcdef0123456789abcdef",
            3,
            1_700_000_000,
            "abcdef0123456789abcdef0123456789",
        )
        .expect("sign handshake");
        let wrong_key = base64::engine::general_purpose::STANDARD.encode([8_u8; 32]);
        assert!(!verify_tunnel_security_handshake(
            &wrong_key,
            "node-1",
            TUNNEL_SECURITY_NON_TLS_REQUIRED,
            "0123456789abcdef0123456789abcdef",
            3,
            1_700_000_000,
            "abcdef0123456789abcdef0123456789",
            &signature,
        ));
        assert!(!verify_tunnel_security_handshake(
            &key,
            "node-1",
            TUNNEL_SECURITY_NON_TLS_REQUIRED,
            "0123456789abcdef0123456789abcdef",
            3,
            1_700_000_000,
            "abcdef0123456789abcdef0123456789",
            "not-base64",
        ));
    }

    #[test]
    fn control_plane_proof_round_trips_and_binds_identity_route_and_body() {
        let key = test_key();
        let body = br#"{"node_id":"node-1","heartbeat_id":7}"#;
        let signature = sign_tunnel_control_plane_request(
            &key,
            "POST",
            "/api/internal/tunnel/heartbeat",
            "node-1",
            1_700_000_000,
            "nonce-0123456789abcdef",
            body,
        )
        .expect("sign control-plane request");

        assert!(verify_tunnel_control_plane_request(
            &key,
            "POST",
            "/api/internal/tunnel/heartbeat",
            "node-1",
            1_700_000_000,
            "nonce-0123456789abcdef",
            body,
            &signature,
        ));
        for valid in [
            verify_tunnel_control_plane_request(
                &key,
                "GET",
                "/api/internal/tunnel/heartbeat",
                "node-1",
                1_700_000_000,
                "nonce-0123456789abcdef",
                body,
                &signature,
            ),
            verify_tunnel_control_plane_request(
                &key,
                "POST",
                "/api/internal/tunnel/node-status",
                "node-1",
                1_700_000_000,
                "nonce-0123456789abcdef",
                body,
                &signature,
            ),
            verify_tunnel_control_plane_request(
                &key,
                "POST",
                "/api/internal/tunnel/heartbeat",
                "node-2",
                1_700_000_000,
                "nonce-0123456789abcdef",
                body,
                &signature,
            ),
            verify_tunnel_control_plane_request(
                &key,
                "POST",
                "/api/internal/tunnel/heartbeat",
                "node-1",
                1_700_000_000,
                "nonce-0123456789abcdef",
                br#"{"node_id":"node-1","heartbeat_id":8}"#,
                &signature,
            ),
        ] {
            assert!(
                !valid,
                "tampered control-plane field must fail verification"
            );
        }
    }
}
