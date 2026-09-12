use std::sync::{Arc, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use http::{request::Parts, HeaderMap};
use serde_json::Value;
use uuid::Uuid;

use crate::ai_serving::transport::ProviderOutboundRequestContext;
use crate::client_session_affinity::codex_request_signals_from_request;

#[derive(Debug, Clone)]
pub(crate) struct CodexFingerprintContextSlot(Arc<OnceLock<ProviderOutboundRequestContext>>);

impl Default for CodexFingerprintContextSlot {
    fn default() -> Self {
        Self(Arc::new(OnceLock::new()))
    }
}

impl CodexFingerprintContextSlot {
    fn resolve(&self, headers: &HeaderMap, body_json: &Value) -> ProviderOutboundRequestContext {
        self.0
            .get_or_init(|| {
                build_codex_fingerprint_context(headers, body_json, Uuid::now_v7().to_string())
            })
            .clone()
    }
}

pub(crate) fn resolve_codex_fingerprint_context(
    parts: &Parts,
    body_json: &Value,
) -> ProviderOutboundRequestContext {
    if let Some(context) = parts
        .extensions
        .get::<ProviderOutboundRequestContext>()
        .cloned()
    {
        return context;
    }
    if let Some(slot) = parts.extensions.get::<CodexFingerprintContextSlot>() {
        return slot.resolve(&parts.headers, body_json);
    }
    build_codex_fingerprint_context(&parts.headers, body_json, Uuid::now_v7().to_string())
}

pub(crate) fn install_codex_fingerprint_context_slot(parts: &mut Parts) {
    if parts
        .extensions
        .get::<ProviderOutboundRequestContext>()
        .is_none()
        && parts
            .extensions
            .get::<CodexFingerprintContextSlot>()
            .is_none()
    {
        parts
            .extensions
            .insert(CodexFingerprintContextSlot::default());
    }
}

pub(crate) fn ensure_codex_fingerprint_context(
    parts: &mut Parts,
    body_json: &Value,
) -> ProviderOutboundRequestContext {
    let context = resolve_codex_fingerprint_context(parts, body_json);
    if parts
        .extensions
        .get::<ProviderOutboundRequestContext>()
        .is_none()
    {
        parts.extensions.remove::<CodexFingerprintContextSlot>();
        parts.extensions.insert(context.clone());
    }
    context
}

pub(crate) fn attach_codex_logical_turn_context(
    parts: &mut Parts,
    body_json: &Value,
    logical_turn_id: &str,
) -> ProviderOutboundRequestContext {
    let context =
        build_codex_fingerprint_context(&parts.headers, body_json, logical_turn_id.to_string());
    parts.extensions.remove::<CodexFingerprintContextSlot>();
    parts.extensions.insert(context.clone());
    context
}

pub(crate) fn restore_codex_logical_turn_context(
    parts: &mut Parts,
    context: &ProviderOutboundRequestContext,
) {
    parts.extensions.remove::<CodexFingerprintContextSlot>();
    parts.extensions.insert(context.clone());
}

fn build_codex_fingerprint_context(
    headers: &HeaderMap,
    body_json: &Value,
    logical_turn_id: String,
) -> ProviderOutboundRequestContext {
    let signals = codex_request_signals_from_request(headers, Some(body_json));
    let mut context = ProviderOutboundRequestContext::new(logical_turn_id, current_unix_millis());

    if let Some(turn_id) = signals.turn_id {
        context = context.with_original_turn_id(turn_id);
    }
    if let Some(session_id) = signals.thread_id.or(signals.session_id) {
        context = context.with_original_client_session_id(session_id);
    }
    if let Some(prompt_cache_key) = signals.prompt_cache_key {
        context = context.with_original_prompt_cache_key(prompt_cache_key);
    }

    context
}

fn current_unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use http::HeaderValue;
    use serde_json::json;

    use super::*;

    #[test]
    fn request_signals_are_captured_once_for_the_logical_turn() {
        let request = http::Request::builder()
            .header("thread-id", "header-thread")
            .body(())
            .expect("request should build");
        let (mut parts, _) = request.into_parts();
        let body = json!({
            "prompt_cache_key": "client-cache",
            "client_metadata": {
                "turn_id": "client-turn",
                "thread_id": "body-thread"
            }
        });

        let context = attach_codex_logical_turn_context(&mut parts, &body, "logical-turn");

        assert_eq!(context.logical_turn_id(), "logical-turn");
        assert_eq!(context.original_turn_id(), Some("client-turn"));
        assert_eq!(context.original_client_session_id(), Some("header-thread"));
        assert_eq!(context.original_prompt_cache_key(), Some("client-cache"));
        assert_eq!(
            parts.extensions.get::<ProviderOutboundRequestContext>(),
            Some(&context)
        );
    }

    #[test]
    fn restored_context_wins_over_retry_request_signals() {
        let original = ProviderOutboundRequestContext::new("logical-turn", 1234)
            .with_original_turn_id("original-turn")
            .with_original_client_session_id("original-thread")
            .with_original_prompt_cache_key("original-cache");
        let request = http::Request::builder()
            .body(())
            .expect("request should build");
        let (mut parts, _) = request.into_parts();
        parts
            .headers
            .insert("thread-id", HeaderValue::from_static("retry-thread"));
        restore_codex_logical_turn_context(&mut parts, &original);

        let resolved = resolve_codex_fingerprint_context(
            &parts,
            &json!({
                "prompt_cache_key": "retry-cache",
                "client_metadata": {"turn_id": "retry-turn"}
            }),
        );

        assert_eq!(resolved, original);
        assert_eq!(resolved.turn_started_at_unix_ms(), 1234);
    }

    #[test]
    fn generated_context_is_persisted_for_http_replanning() {
        let request = http::Request::builder()
            .header("session-id", "client-session")
            .body(())
            .expect("request should build");
        let (mut parts, _) = request.into_parts();
        let body = json!({
            "prompt_cache_key": "client-cache",
            "client_metadata": {"turn_id": "client-turn"}
        });

        let first = ensure_codex_fingerprint_context(&mut parts, &body);
        let second = resolve_codex_fingerprint_context(
            &parts,
            &json!({
                "prompt_cache_key": "retry-cache",
                "client_metadata": {"turn_id": "retry-turn"}
            }),
        );

        assert_eq!(second, first);
        assert_eq!(second.original_turn_id(), Some("client-turn"));
        assert_eq!(second.original_prompt_cache_key(), Some("client-cache"));
    }

    #[test]
    fn installed_slot_reuses_context_across_cloned_parts() {
        let request = http::Request::builder()
            .body(())
            .expect("request should build");
        let (mut parts, _) = request.into_parts();
        install_codex_fingerprint_context_slot(&mut parts);
        let cloned_parts = parts.clone();

        let first = resolve_codex_fingerprint_context(
            &parts,
            &json!({
                "prompt_cache_key": "first-cache",
                "client_metadata": {"turn_id": "first-turn"}
            }),
        );
        let second = resolve_codex_fingerprint_context(
            &cloned_parts,
            &json!({
                "prompt_cache_key": "second-cache",
                "client_metadata": {"turn_id": "second-turn"}
            }),
        );

        assert_eq!(second, first);
        assert_eq!(second.original_turn_id(), Some("first-turn"));
        assert_eq!(second.original_prompt_cache_key(), Some("first-cache"));
    }
}
