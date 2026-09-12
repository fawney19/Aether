use aws_lc_rs::rand::SystemRandom;
use aws_lc_rs::rsa::{KeyPair, PublicKey, RsaParameters};
use aws_lc_rs::signature::{UnparsedPublicKey, RSA_PKCS1_2048_8192_SHA256, RSA_PKCS1_SHA256};
use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use thiserror::Error;

const MAX_RSA_KEY_INPUT_BYTES: usize = 64 * 1024;
const PRIVATE_KEY_PEM_LABELS: &[&str] = &["PRIVATE KEY", "RSA PRIVATE KEY"];
const PUBLIC_KEY_PEM_LABELS: &[&str] = &["PUBLIC KEY", "RSA PUBLIC KEY"];

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum RsaPkcs1Sha256Error {
    #[error("invalid RSA private key")]
    InvalidPrivateKey,
    #[error("invalid RSA public key")]
    InvalidPublicKey,
    #[error("RSA signing failed")]
    SigningFailed,
    #[error("invalid RSA signature encoding")]
    InvalidSignature,
}

fn decode_text_key_material(input: &[u8], labels: &[&str]) -> Option<Vec<u8>> {
    let text = std::str::from_utf8(input).ok()?.trim();
    if text.is_empty() {
        return None;
    }

    let encoded = if text.starts_with("-----BEGIN ") {
        labels.iter().find_map(|label| {
            let header = format!("-----BEGIN {label}-----");
            let footer = format!("-----END {label}-----");
            text.strip_prefix(&header)?.strip_suffix(&footer)
        })?
    } else {
        text
    };

    let mut compact = encoded
        .bytes()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect::<Vec<_>>();
    let decoded = (!compact.is_empty())
        .then(|| STANDARD.decode(&compact).ok())
        .flatten();
    compact.fill(0);
    decoded
}

fn parse_private_key_der(input: &[u8]) -> Result<KeyPair, RsaPkcs1Sha256Error> {
    KeyPair::from_pkcs8(input)
        .or_else(|_| KeyPair::from_der(input))
        .map_err(|_| RsaPkcs1Sha256Error::InvalidPrivateKey)
}

fn parse_private_key(input: &[u8]) -> Result<KeyPair, RsaPkcs1Sha256Error> {
    if input.is_empty() || input.len() > MAX_RSA_KEY_INPUT_BYTES {
        return Err(RsaPkcs1Sha256Error::InvalidPrivateKey);
    }
    if let Ok(key_pair) = parse_private_key_der(input) {
        return Ok(key_pair);
    }

    let mut der = decode_text_key_material(input, PRIVATE_KEY_PEM_LABELS)
        .ok_or(RsaPkcs1Sha256Error::InvalidPrivateKey)?;
    let result = parse_private_key_der(&der);
    der.fill(0);
    result
}

fn parse_public_key_der(input: &[u8]) -> Result<PublicKey, RsaPkcs1Sha256Error> {
    let public_key =
        PublicKey::from_der(input).map_err(|_| RsaPkcs1Sha256Error::InvalidPublicKey)?;
    let bits = RsaParameters::public_modulus_len(public_key.as_ref())
        .map_err(|_| RsaPkcs1Sha256Error::InvalidPublicKey)?;
    if !(2048..=8192).contains(&bits) {
        return Err(RsaPkcs1Sha256Error::InvalidPublicKey);
    }
    Ok(public_key)
}

fn parse_public_key(input: &[u8]) -> Result<PublicKey, RsaPkcs1Sha256Error> {
    if input.is_empty() || input.len() > MAX_RSA_KEY_INPUT_BYTES {
        return Err(RsaPkcs1Sha256Error::InvalidPublicKey);
    }
    if let Ok(public_key) = parse_public_key_der(input) {
        return Ok(public_key);
    }

    let der = decode_text_key_material(input, PUBLIC_KEY_PEM_LABELS)
        .ok_or(RsaPkcs1Sha256Error::InvalidPublicKey)?;
    parse_public_key_der(&der)
}

/// Signs `message` with RSASSA-PKCS1-v1_5 and SHA-256 using AWS-LC.
///
/// The private key may be PKCS#8 or PKCS#1 DER, either PEM encoded or supplied
/// as bare standard-base64 DER. Raw DER bytes are accepted as well.
pub fn rsa_pkcs1_sha256_sign(
    private_key: &[u8],
    message: &[u8],
) -> Result<Vec<u8>, RsaPkcs1Sha256Error> {
    let key_pair = parse_private_key(private_key)?;
    let mut signature = vec![0; key_pair.public_modulus_len()];
    key_pair
        .sign(
            &RSA_PKCS1_SHA256,
            &SystemRandom::new(),
            message,
            &mut signature,
        )
        .map_err(|_| RsaPkcs1Sha256Error::SigningFailed)?;
    Ok(signature)
}

/// Verifies an RSASSA-PKCS1-v1_5 SHA-256 signature using AWS-LC.
///
/// The public key may be PKCS#1 or X.509 SubjectPublicKeyInfo DER, either PEM
/// encoded or supplied as bare standard-base64 DER. Raw DER bytes are accepted
/// as well.
pub fn rsa_pkcs1_sha256_verify(
    public_key: &[u8],
    message: &[u8],
    signature: &[u8],
) -> Result<bool, RsaPkcs1Sha256Error> {
    let public_key = parse_public_key(public_key)?;
    let modulus_bits = RsaParameters::public_modulus_len(public_key.as_ref())
        .map_err(|_| RsaPkcs1Sha256Error::InvalidPublicKey)?;
    let signature_len = (modulus_bits as usize).div_ceil(8);
    if signature.len() != signature_len {
        return Err(RsaPkcs1Sha256Error::InvalidSignature);
    }
    Ok(
        UnparsedPublicKey::new(&RSA_PKCS1_2048_8192_SHA256, public_key.as_ref())
            .verify(message, signature)
            .is_ok(),
    )
}

#[cfg(test)]
mod tests {
    use aws_lc_rs::encoding::{AsDer, Pkcs8V1Der, PublicKeyX509Der};
    use aws_lc_rs::rsa::{KeyPair, KeySize};
    use aws_lc_rs::signature::KeyPair as _;
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine as _;

    use super::{rsa_pkcs1_sha256_sign, rsa_pkcs1_sha256_verify, RsaPkcs1Sha256Error};

    fn read_der_tlv<'a>(input: &mut &'a [u8], expected_tag: u8) -> &'a [u8] {
        assert_eq!(input.first().copied(), Some(expected_tag));
        let length_byte = input[1];
        let (header_len, value_len) = if length_byte & 0x80 == 0 {
            (2, usize::from(length_byte))
        } else {
            let length_bytes = usize::from(length_byte & 0x7f);
            assert!((1..=4).contains(&length_bytes));
            let value_len = input[2..2 + length_bytes]
                .iter()
                .fold(0usize, |value, byte| (value << 8) | usize::from(*byte));
            (2 + length_bytes, value_len)
        };
        let end = header_len + value_len;
        assert!(end <= input.len());
        let value = &input[header_len..end];
        *input = &input[end..];
        value
    }

    fn pkcs1_private_key_from_pkcs8(pkcs8: &[u8]) -> Vec<u8> {
        let mut input = pkcs8;
        let mut sequence = read_der_tlv(&mut input, 0x30);
        assert!(input.is_empty());
        let _version = read_der_tlv(&mut sequence, 0x02);
        let _algorithm = read_der_tlv(&mut sequence, 0x30);
        read_der_tlv(&mut sequence, 0x04).to_vec()
    }

    fn pem(label: &str, der: &[u8]) -> String {
        format!(
            "-----BEGIN {label}-----\n{}\n-----END {label}-----",
            STANDARD.encode(der)
        )
    }

    #[test]
    fn signs_and_verifies_all_supported_rsa_key_encodings() {
        let key_pair = KeyPair::generate(KeySize::Rsa2048).expect("RSA key should generate");
        let pkcs8 = AsDer::<Pkcs8V1Der<'static>>::as_der(&key_pair)
            .expect("PKCS#8 should encode")
            .as_ref()
            .to_vec();
        let pkcs1_private = pkcs1_private_key_from_pkcs8(&pkcs8);
        let pkcs1_public = key_pair.public_key().as_ref().to_vec();
        let spki_public = AsDer::<PublicKeyX509Der<'static>>::as_der(key_pair.public_key())
            .expect("SPKI should encode")
            .as_ref()
            .to_vec();
        let private_inputs = [
            pkcs8.clone(),
            pkcs1_private.clone(),
            pem("PRIVATE KEY", &pkcs8).into_bytes(),
            pem("RSA PRIVATE KEY", &pkcs1_private).into_bytes(),
            STANDARD.encode(&pkcs8).into_bytes(),
            STANDARD.encode(&pkcs1_private).into_bytes(),
        ];
        let public_inputs = [
            pkcs1_public.clone(),
            spki_public.clone(),
            pem("RSA PUBLIC KEY", &pkcs1_public).into_bytes(),
            pem("PUBLIC KEY", &spki_public).into_bytes(),
            STANDARD.encode(&pkcs1_public).into_bytes(),
            STANDARD.encode(&spki_public).into_bytes(),
        ];
        let message = b"Aether RSA-SHA256 compatibility vector";

        let expected =
            rsa_pkcs1_sha256_sign(&private_inputs[0], message).expect("PKCS#8 DER should sign");
        assert_eq!(expected.len(), 256);
        for private_key in private_inputs {
            assert_eq!(
                rsa_pkcs1_sha256_sign(&private_key, message).expect("key format should sign"),
                expected,
                "PKCS#1 v1.5 output must remain deterministic across encodings"
            );
        }
        for public_key in public_inputs {
            assert!(rsa_pkcs1_sha256_verify(&public_key, message, &expected)
                .expect("key format should verify"));
        }
        assert!(
            !rsa_pkcs1_sha256_verify(&pkcs1_public, b"tampered", &expected)
                .expect("valid key with invalid signature should return false")
        );
        assert_eq!(
            rsa_pkcs1_sha256_verify(&pkcs1_public, message, &expected[..255]),
            Err(RsaPkcs1Sha256Error::InvalidSignature)
        );
    }

    #[test]
    fn rejects_malformed_or_unsupported_rsa_keys() {
        assert_eq!(
            rsa_pkcs1_sha256_sign(b"not-a-key", b"message"),
            Err(RsaPkcs1Sha256Error::InvalidPrivateKey)
        );
        assert_eq!(
            rsa_pkcs1_sha256_verify(b"not-a-key", b"message", b"signature"),
            Err(RsaPkcs1Sha256Error::InvalidPublicKey)
        );
        assert_eq!(
            rsa_pkcs1_sha256_sign(&vec![b'A'; 64 * 1024 + 1], b"message"),
            Err(RsaPkcs1Sha256Error::InvalidPrivateKey)
        );
    }
}
