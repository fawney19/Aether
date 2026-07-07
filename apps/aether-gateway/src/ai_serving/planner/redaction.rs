use std::borrow::Cow;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde_json::Value;
use tracing::warn;

use crate::ai_serving::ExecutionRuntimeAuthContext;
use crate::privacy::{
    build_redaction_session_config, read_chat_pii_redaction_runtime_config,
    try_mask_chat_pii_request_value_with_cache_options, CachedRequestRedaction,
    ChatPiiRedactionRequestFormat, MaskChatRequestOptions, RedactionMaskError,
    RedactionSessionSlot, RedisRedactionMappingCache,
};
use crate::stage_metrics::observe_gateway_stage_ms;
use crate::{AppState, GatewayError};

pub(crate) struct ProviderRequestRedaction<'a> {
    pub(crate) body_json: Cow<'a, Value>,
    pub(crate) redacted: bool,
}

impl<'a> ProviderRequestRedaction<'a> {
    fn disabled(body_json: &'a Value) -> Self {
        Self {
            body_json: Cow::Borrowed(body_json),
            redacted: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct ChatPiiRedactionFeatureSettings {
    enabled: Option<bool>,
}

impl ChatPiiRedactionFeatureSettings {
    fn merge_from_value(&mut self, value: Option<&Value>) {
        let Some(settings) = value
            .and_then(Value::as_object)
            .and_then(|features| features.get("chat_pii_redaction"))
            .and_then(Value::as_object)
        else {
            return;
        };
        if let Some(enabled) = settings.get("enabled").and_then(Value::as_bool) {
            self.enabled = Some(enabled);
        }
    }

    fn effective_enabled(self) -> bool {
        self.enabled.unwrap_or(false)
    }
}

pub(crate) fn request_identity_response_encoding_when_redacted(
    headers: &mut std::collections::BTreeMap<String, String>,
    redacted: bool,
) {
    if redacted {
        headers.insert("accept-encoding".to_string(), "identity".to_string());
    }
}

pub(crate) async fn resolve_provider_chat_pii_redaction<'a>(
    state: &AppState,
    parts: &http::request::Parts,
    body_json: &'a Value,
    auth_context: &ExecutionRuntimeAuthContext,
    client_api_format: &str,
    candidate_id: &str,
) -> Result<ProviderRequestRedaction<'a>, GatewayError> {
    let Some(format) = ChatPiiRedactionRequestFormat::from_api_format(client_api_format) else {
        return Ok(ProviderRequestRedaction::disabled(body_json));
    };
    let Some(slot) = parts.extensions.get::<RedactionSessionSlot>() else {
        return Ok(ProviderRequestRedaction::disabled(body_json));
    };
    let request_cache_key = request_redaction_cache_key(format, body_json);
    if let Some(cached) = slot.cached_request_redaction(&request_cache_key) {
        crate::stage_metrics::record_chat_pii_redaction_request_cache_hit();
        observe_gateway_stage_ms("chat_pii_redaction_request_cache_hit", 0);
        return Ok(provider_redaction_from_cached(
            slot,
            candidate_id,
            body_json,
            cached,
        ));
    }
    crate::stage_metrics::record_chat_pii_redaction_request_cache_miss();

    let runtime_config_started_at = Instant::now();
    let runtime_config = read_chat_pii_redaction_runtime_config(state)
        .await
        .map_err(|err| {
            warn!(
                error = ?err,
                "gateway failed to read chat pii redaction runtime config"
            );
            GatewayError::Internal("chat pii redaction setup failed".to_string())
        })?;
    observe_gateway_stage_ms(
        "chat_pii_redaction_runtime_config",
        runtime_config_started_at.elapsed().as_millis() as u64,
    );
    if !runtime_config.enabled {
        slot.put_cached_request_redaction(request_cache_key, CachedRequestRedaction::unredacted());
        return Ok(ProviderRequestRedaction::disabled(body_json));
    }
    let feature_settings_started_at = Instant::now();
    let feature_settings = resolve_chat_pii_redaction_feature_settings(state, auth_context).await?;
    observe_gateway_stage_ms(
        "chat_pii_redaction_feature_settings",
        feature_settings_started_at.elapsed().as_millis() as u64,
    );
    if !feature_settings.effective_enabled() {
        slot.put_cached_request_redaction(request_cache_key, CachedRequestRedaction::unredacted());
        return Ok(ProviderRequestRedaction::disabled(body_json));
    }
    let Some(hmac_key) = state.encryption_key().map(str::as_bytes).map(Vec::from) else {
        warn!("gateway chat pii redaction is enabled but encryption key is unavailable");
        return Err(GatewayError::Internal(
            "chat pii redaction setup failed".to_string(),
        ));
    };
    let now_unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let cache = RedisRedactionMappingCache::new(state.runtime_state.as_ref());
    let mask_started_at = Instant::now();
    let masked = try_mask_chat_pii_request_value_with_cache_options(
        body_json,
        format,
        build_redaction_session_config(hmac_key, &runtime_config, now_unix_secs),
        MaskChatRequestOptions::runtime(),
        Some(&cache),
    )
    .await
    .map_err(redaction_mask_error_to_gateway_error)?;
    observe_gateway_stage_ms(
        "chat_pii_redaction_mask_body",
        mask_started_at.elapsed().as_millis() as u64,
    );
    if !masked.redacted {
        slot.put_cached_request_redaction(request_cache_key, CachedRequestRedaction::unredacted());
        return Ok(ProviderRequestRedaction {
            body_json: Cow::Borrowed(body_json),
            redacted: false,
        });
    }
    let Some(masked_body_json) = masked.body_json else {
        warn!("gateway pii redaction reported redacted without masked body");
        return Err(GatewayError::Internal(
            "chat pii redaction setup failed".to_string(),
        ));
    };
    slot.put_cached_request_redaction(
        request_cache_key,
        CachedRequestRedaction::redacted(masked_body_json.clone(), masked.session.clone()),
    );
    slot.put_for_candidate(candidate_id, masked.session);
    Ok(ProviderRequestRedaction {
        body_json: Cow::Owned(masked_body_json),
        redacted: true,
    })
}

fn request_redaction_cache_key(format: ChatPiiRedactionRequestFormat, body_json: &Value) -> String {
    format!("{format:?}:{:p}", body_json)
}

fn provider_redaction_from_cached<'a>(
    slot: &RedactionSessionSlot,
    candidate_id: &str,
    body_json: &'a Value,
    cached: CachedRequestRedaction,
) -> ProviderRequestRedaction<'a> {
    if !cached.redacted {
        return ProviderRequestRedaction::disabled(body_json);
    }
    let Some(masked_body_json) = cached.body_json else {
        return ProviderRequestRedaction::disabled(body_json);
    };
    if let Some(session) = cached.session {
        slot.put_for_candidate(candidate_id, session);
    }
    ProviderRequestRedaction {
        body_json: Cow::Owned(masked_body_json),
        redacted: true,
    }
}

async fn resolve_chat_pii_redaction_feature_settings(
    state: &AppState,
    auth_context: &ExecutionRuntimeAuthContext,
) -> Result<ChatPiiRedactionFeatureSettings, GatewayError> {
    let user_settings_fut = state.read_user_feature_settings(&auth_context.user_id);
    let key_settings_fut = state.read_auth_api_key_feature_settings(
        &auth_context.user_id,
        &auth_context.api_key_id,
        auth_context.api_key_is_standalone,
    );
    let (user_settings, key_settings) = tokio::try_join!(user_settings_fut, key_settings_fut)
        .map_err(|err| {
            warn!(
                error = ?err,
                "gateway failed to read chat pii redaction feature settings"
            );
            GatewayError::Internal("chat pii redaction setup failed".to_string())
        })?;

    let mut settings = ChatPiiRedactionFeatureSettings::default();
    settings.merge_from_value(user_settings.as_ref());
    settings.merge_from_value(key_settings.as_ref());
    Ok(settings)
}

fn redaction_mask_error_to_gateway_error(error: RedactionMaskError) -> GatewayError {
    match error {
        RedactionMaskError::Limit(limit) => GatewayError::Client {
            status: limit.client_status(),
            message: limit.safe_message().to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::ChatPiiRedactionFeatureSettings;

    #[test]
    fn chat_pii_redaction_feature_settings_only_control_enablement() {
        let mut settings = ChatPiiRedactionFeatureSettings::default();
        settings.merge_from_value(Some(&json!({
            "chat_pii_redaction": {
                "enabled": true
            }
        })));

        assert!(settings.effective_enabled());
    }
}
