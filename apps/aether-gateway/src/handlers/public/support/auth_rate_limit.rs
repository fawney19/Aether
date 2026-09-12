use super::{build_auth_json_response, http, json, AppState, Body, Response};
use aether_runtime_state::{UsageLimitCheck, UsageLimitInput, UsageLimitRule};
use hmac::Mac;
use sha2::Sha256;
use std::net::IpAddr;
use std::time::{SystemTime, UNIX_EPOCH};

const AUTH_RATE_LIMIT_UNAVAILABLE_DETAIL: &str = "认证安全服务暂不可用，请稍后重试";
const AUTH_RATE_LIMITED_DETAIL: &str = "请求过于频繁，请稍后重试";
const AUTH_VERIFICATION_MAX_FAILURES: u32 = 5;

#[derive(Debug, Clone, Copy)]
pub(super) struct AuthRateLimitPolicy {
    action: &'static str,
    window_seconds: u64,
    ip_limit: u32,
    identity_limit: u32,
}

impl AuthRateLimitPolicy {
    const fn new(
        action: &'static str,
        window_seconds: u64,
        ip_limit: u32,
        identity_limit: u32,
    ) -> Self {
        Self {
            action,
            window_seconds,
            ip_limit,
            identity_limit,
        }
    }
}

pub(super) const AUTH_LOGIN_RATE_LIMIT: AuthRateLimitPolicy =
    AuthRateLimitPolicy::new("login", 60, 60, 10);
pub(super) const AUTH_SEND_VERIFICATION_RATE_LIMIT: AuthRateLimitPolicy =
    AuthRateLimitPolicy::new("send-verification-code", 3_600, 20, 3);
pub(super) const AUTH_REGISTER_RATE_LIMIT: AuthRateLimitPolicy =
    AuthRateLimitPolicy::new("register", 3_600, 10, 5);
pub(super) const AUTH_VERIFY_EMAIL_RATE_LIMIT: AuthRateLimitPolicy =
    AuthRateLimitPolicy::new("verify-email", 300, 30, 10);
pub(super) const AUTH_VERIFICATION_STATUS_RATE_LIMIT: AuthRateLimitPolicy =
    AuthRateLimitPolicy::new("verification-status", 60, 60, 20);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AuthRateLimitCheck {
    Allowed,
    Rejected { retry_after: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AuthVerificationFailureDecision {
    Incorrect,
    Exhausted { retry_after: u64 },
}

fn unix_timestamp_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn digest_subject_with_key(
    secret: &[u8],
    action: &str,
    dimension: &str,
    subject: &str,
) -> Result<String, String> {
    let mut mac = hmac::Hmac::<Sha256>::new_from_slice(secret)
        .map_err(|_| "auth rate-limit HMAC key invalid".to_string())?;
    mac.update(b"aether-auth-rate-limit-v1\0");
    mac.update(action.as_bytes());
    mac.update(b"\0");
    mac.update(dimension.as_bytes());
    mac.update(b"\0");
    mac.update(subject.as_bytes());
    Ok(mac
        .finalize()
        .into_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn rate_limit_key_with_key(
    secret: &[u8],
    action: &str,
    dimension: &str,
    subject: &str,
    window_seconds: u64,
) -> Result<String, String> {
    let digest = digest_subject_with_key(secret, action, dimension, subject)?;
    Ok(format!(
        "auth:rate-limit:v2:{{{digest}}}:{action}:{dimension}:{}",
        window_seconds.max(1)
    ))
}

fn rate_limit_key(
    action: &str,
    dimension: &str,
    subject: &str,
    window_seconds: u64,
) -> Result<String, String> {
    let secret = super::auth_jwt_secret()?;
    rate_limit_key_with_key(
        secret.as_bytes(),
        action,
        dimension,
        subject,
        window_seconds,
    )
}

async fn consume_auth_rate_limit(
    state: &AppState,
    action: &str,
    dimension: &str,
    subject: &str,
    limit: u32,
    window_seconds: u64,
) -> Result<AuthRateLimitCheck, String> {
    let window_seconds = window_seconds.max(1);
    let now_unix_ms = unix_timestamp_millis();
    let counter_key = rate_limit_key(action, dimension, subject, window_seconds)?;
    let event_id = uuid::Uuid::new_v4().simple().to_string();
    let rule = UsageLimitRule {
        key: &counter_key,
        limit: u64::from(limit.max(1)),
        window_seconds,
        retention_seconds: window_seconds,
    };
    let result = state
        .runtime_state()
        .check_and_consume_usage_limits(UsageLimitInput {
            rules: std::slice::from_ref(&rule),
            event_id: &event_id,
            now_unix_ms,
        })
        .await
        .map_err(|err| err.to_string())?;

    Ok(match result {
        UsageLimitCheck::Allowed => AuthRateLimitCheck::Allowed,
        UsageLimitCheck::Rejected { retry_after, .. } => AuthRateLimitCheck::Rejected {
            retry_after: retry_after.max(1),
        },
    })
}

pub(super) fn build_auth_rate_limited_response(
    detail: &'static str,
    retry_after: u64,
) -> Response<Body> {
    let retry_after = retry_after.max(1);
    let mut response = build_auth_json_response(
        http::StatusCode::TOO_MANY_REQUESTS,
        json!({
            "detail": detail,
            "retry_after": retry_after,
        }),
        None,
    );
    if let Ok(value) = http::HeaderValue::from_str(&retry_after.to_string()) {
        response
            .headers_mut()
            .insert(http::header::RETRY_AFTER, value);
    }
    response
}

fn build_auth_rate_limit_unavailable_response() -> Response<Body> {
    build_auth_json_response(
        http::StatusCode::SERVICE_UNAVAILABLE,
        json!({ "detail": AUTH_RATE_LIMIT_UNAVAILABLE_DETAIL }),
        None,
    )
}

async fn enforce_auth_rate_limit(
    state: &AppState,
    policy: AuthRateLimitPolicy,
    dimension: &'static str,
    subject: &str,
    limit: u32,
) -> Result<(), Response<Body>> {
    match consume_auth_rate_limit(
        state,
        policy.action,
        dimension,
        subject,
        limit,
        policy.window_seconds,
    )
    .await
    {
        Ok(AuthRateLimitCheck::Allowed) => Ok(()),
        Ok(AuthRateLimitCheck::Rejected { retry_after }) => Err(build_auth_rate_limited_response(
            AUTH_RATE_LIMITED_DETAIL,
            retry_after,
        )),
        Err(_) => {
            tracing::warn!(
                event_name = "auth_rate_limit_check_failed",
                action = policy.action,
                dimension,
                "authentication request rejected because the rate-limit backend failed"
            );
            Err(build_auth_rate_limit_unavailable_response())
        }
    }
}

pub(super) async fn enforce_auth_ip_rate_limit(
    state: &AppState,
    policy: AuthRateLimitPolicy,
    client_ip: IpAddr,
) -> Result<(), Response<Body>> {
    enforce_auth_rate_limit(state, policy, "ip", &client_ip.to_string(), policy.ip_limit).await
}

pub(super) async fn enforce_auth_identity_rate_limit(
    state: &AppState,
    policy: AuthRateLimitPolicy,
    identity: &str,
) -> Result<(), Response<Body>> {
    enforce_auth_rate_limit(state, policy, "identity", identity, policy.identity_limit).await
}

pub(super) async fn record_auth_verification_failure(
    state: &AppState,
    email: &str,
    challenge_created_at: &str,
    challenge_code_hash: &str,
    challenge_ttl_seconds: u64,
) -> Result<AuthVerificationFailureDecision, Response<Body>> {
    let challenge_subject = format!("{email}\0{challenge_created_at}\0{challenge_code_hash}");
    match consume_auth_rate_limit(
        state,
        "verify-email-challenge",
        "challenge",
        &challenge_subject,
        AUTH_VERIFICATION_MAX_FAILURES.saturating_sub(1),
        challenge_ttl_seconds.max(1),
    )
    .await
    {
        Ok(AuthRateLimitCheck::Rejected { .. }) => Ok(AuthVerificationFailureDecision::Exhausted {
            retry_after: challenge_ttl_seconds.max(1),
        }),
        Ok(AuthRateLimitCheck::Allowed) => Ok(AuthVerificationFailureDecision::Incorrect),
        Err(_) => {
            tracing::warn!(
                event_name = "auth_verification_failure_count_failed",
                "email verification challenge rejected because the failure counter failed"
            );
            Err(build_auth_rate_limit_unavailable_response())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::Digest;

    #[test]
    fn rate_limit_keys_use_keyed_subject_digests_without_plaintext() {
        let secret = b"fixed-test-secret-with-at-least-32-bytes";
        let cases = [
            ("login", "ip", "203.0.113.42"),
            ("login", "identity", "target@example.com"),
            (
                "verify-email-challenge",
                "challenge",
                "target@example.com\0created-at\0code-hash",
            ),
        ];

        for (action, dimension, subject) in cases {
            let digest = digest_subject_with_key(secret, action, dimension, subject)
                .expect("fixed HMAC key should be accepted");
            let enumerable_digest = format!(
                "{:x}",
                Sha256::digest(format!("{dimension}\0{subject}").as_bytes())
            );
            let key = rate_limit_key_with_key(secret, action, dimension, subject, 60)
                .expect("fixed HMAC key should be accepted");

            assert_ne!(digest, enumerable_digest);
            assert!(key.contains(&format!("{{{digest}}}")));
            assert!(!key.contains(&enumerable_digest));
            for plaintext_part in subject.split('\0') {
                assert!(!key.contains(plaintext_part));
            }
        }
    }

    #[test]
    fn rate_limit_subject_digest_is_domain_separated() {
        let secret = b"fixed-test-secret-with-at-least-32-bytes";
        let subject = "same-subject";
        let login_identity = digest_subject_with_key(secret, "login", "identity", subject)
            .expect("fixed HMAC key should be accepted");
        let register_identity = digest_subject_with_key(secret, "register", "identity", subject)
            .expect("fixed HMAC key should be accepted");
        let login_ip = digest_subject_with_key(secret, "login", "ip", subject)
            .expect("fixed HMAC key should be accepted");

        assert_ne!(login_identity, register_identity);
        assert_ne!(login_identity, login_ip);
    }
}
