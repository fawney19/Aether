use crate::handlers::admin::provider::shared::payloads::AdminProviderKeyUpdatePatch;
use serde_json::{Map, Value};
use std::collections::BTreeSet;

const BATCH_EDITABLE_KEY_FIELDS: &[&str] = &[
    "allow_auth_channel_mismatch_formats",
    "allowed_models",
    "api_formats",
    "auth_type_by_format",
    "auto_fetch_models",
    "cache_ttl_minutes",
    "capabilities",
    "concurrent_limit",
    "global_priority_by_format",
    "internal_priority",
    "is_active",
    "locked_models",
    "max_probe_interval_minutes",
    "model_exclude_patterns",
    "model_include_patterns",
    "note",
    "proxy",
    "rate_multipliers",
    "rpm_limit",
];

pub(crate) fn parse_admin_provider_key_batch_update_patch(
    value: Value,
) -> Result<Map<String, Value>, String> {
    let Value::Object(patch) = value else {
        return Err("patch 必须是 JSON 对象".to_string());
    };
    if patch.is_empty() {
        return Err("patch 至少包含一个可编辑字段".to_string());
    }

    let allowed = BATCH_EDITABLE_KEY_FIELDS
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    let unsupported = patch
        .keys()
        .filter(|field| !allowed.contains(field.as_str()))
        .cloned()
        .collect::<BTreeSet<_>>();
    if !unsupported.is_empty() {
        return Err(format!(
            "批量编辑不支持字段: {}",
            unsupported.into_iter().collect::<Vec<_>>().join(", ")
        ));
    }

    AdminProviderKeyUpdatePatch::from_object(patch.clone())
        .map_err(|_| "patch 字段类型无效".to_string())?;
    Ok(patch)
}

#[cfg(test)]
mod tests {
    use super::parse_admin_provider_key_batch_update_patch;
    use serde_json::json;

    #[test]
    fn accepts_shared_key_configuration_fields() {
        let patch = parse_admin_provider_key_batch_update_patch(json!({
            "api_formats": ["openai:responses"],
            "auto_fetch_models": true,
            "model_include_patterns": ["gpt-*"],
            "allowed_models": ["gpt-5.6-sol"],
            "rpm_limit": null
        }))
        .expect("batch patch should parse");

        assert_eq!(patch.len(), 5);
        assert_eq!(patch["auto_fetch_models"], json!(true));
    }

    #[test]
    fn rejects_identity_and_secret_fields() {
        let error = parse_admin_provider_key_batch_update_patch(json!({
            "name": "shared-name",
            "api_key": "sk-shared"
        }))
        .expect_err("identity fields must stay single-key only");

        assert_eq!(error, "批量编辑不支持字段: api_key, name");
    }

    #[test]
    fn rejects_empty_or_non_object_patch() {
        assert_eq!(
            parse_admin_provider_key_batch_update_patch(json!({}))
                .expect_err("empty patch should fail"),
            "patch 至少包含一个可编辑字段"
        );
        assert_eq!(
            parse_admin_provider_key_batch_update_patch(json!([]))
                .expect_err("array patch should fail"),
            "patch 必须是 JSON 对象"
        );
    }
}
