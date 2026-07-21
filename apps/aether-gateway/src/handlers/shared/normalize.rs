use std::collections::BTreeSet;
use std::net::IpAddr;

use serde_json::{Map, Value};

pub(crate) fn normalize_string_list(values: Option<Vec<String>>) -> Option<Vec<String>> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for value in values.into_iter().flatten() {
        let trimmed = value.trim();
        if trimmed.is_empty() || !seen.insert(trimmed.to_string()) {
            continue;
        }
        out.push(trimmed.to_string());
    }
    (!out.is_empty()).then_some(out)
}

pub(crate) fn normalize_json_object(
    value: Option<serde_json::Value>,
    field_name: &str,
) -> Result<Option<serde_json::Value>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    match value {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::Object(map) if map.is_empty() => Ok(None),
        serde_json::Value::Object(map) => Ok(Some(serde_json::Value::Object(map))),
        _ => Err(format!("{field_name} 必须是 JSON 对象")),
    }
}

pub(crate) fn normalize_json_array(
    value: Option<serde_json::Value>,
    field_name: &str,
) -> Result<Option<serde_json::Value>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    match value {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::Array(items) if items.is_empty() => Ok(None),
        serde_json::Value::Array(items) => Ok(Some(serde_json::Value::Array(items))),
        _ => Err(format!("{field_name} 必须是 JSON 数组")),
    }
}

pub(crate) fn normalize_feature_settings(value: Option<Value>) -> Result<Option<Value>, String> {
    let Some(mut value) = value else {
        return Ok(None);
    };
    match value {
        Value::Null => Ok(None),
        Value::Object(ref mut settings) => {
            normalize_chat_pii_redaction_feature_settings(settings)?;
            normalize_notification_push_service_feature_settings(settings)?;
            if settings.is_empty() {
                Ok(None)
            } else {
                Ok(Some(value))
            }
        }
        _ => Err("feature_settings 必须是对象".to_string()),
    }
}

pub(crate) fn normalize_user_self_feature_settings_update(
    value: Option<Value>,
    current: Option<Value>,
) -> Result<Option<Value>, String> {
    let mut normalized = normalize_feature_settings(value)?;
    let current_notification_push_service = current
        .and_then(|value| match value {
            Value::Object(mut settings) => settings.remove("notification_push_service"),
            _ => None,
        })
        .and_then(|value| {
            let mut wrapper = Map::new();
            wrapper.insert("notification_push_service".to_string(), value);
            normalize_notification_push_service_feature_settings(&mut wrapper)
                .ok()
                .and_then(|_| wrapper.remove("notification_push_service"))
        });

    match (&mut normalized, current_notification_push_service) {
        (Some(Value::Object(settings)), Some(value)) => {
            settings.insert("notification_push_service".to_string(), value);
        }
        (Some(Value::Object(settings)), None) => {
            settings.remove("notification_push_service");
        }
        (None, Some(value)) => {
            let mut settings = Map::new();
            settings.insert("notification_push_service".to_string(), value);
            normalized = Some(Value::Object(settings));
        }
        _ => {}
    }
    Ok(normalized)
}

pub(crate) fn normalize_ip_rules(
    values: Option<Vec<String>>,
) -> Result<Option<Vec<String>>, String> {
    let Some(values) = values else {
        return Ok(None);
    };
    let mut normalized = Vec::new();
    let mut seen = BTreeSet::new();
    for (index, raw) in values.into_iter().enumerate() {
        let rule = normalize_ip_rule(raw.trim())
            .map_err(|detail| format!("{detail}（第 {} 项）", index + 1))?;
        if seen.insert(rule.clone()) {
            normalized.push(rule);
        }
    }
    Ok((!normalized.is_empty()).then_some(normalized))
}

pub(crate) fn parse_json_ip_rules(value: Option<&Value>) -> Result<Option<Value>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    match value {
        Value::Null => Ok(None),
        Value::Array(items) => {
            let mut values = Vec::with_capacity(items.len());
            for item in items {
                let Some(value) = item.as_str() else {
                    return Err("IP 限制规则必须是字符串数组".to_string());
                };
                values.push(value.to_string());
            }
            Ok(normalize_ip_rules(Some(values))?.map(|rules| serde_json::json!(rules)))
        }
        _ => Err("IP 限制规则必须是字符串数组".to_string()),
    }
}

pub(crate) fn ip_rules_allow(rules: Option<&[String]>, remote_ip: IpAddr) -> bool {
    let Some(rules) = rules else {
        return true;
    };
    if rules.is_empty() {
        return true;
    }

    let mut has_allow_rule = false;
    let mut matched_allow_rule = false;
    for raw in rules {
        let rule = raw.trim();
        if rule.is_empty() {
            continue;
        }
        let (deny, pattern) = match rule.strip_prefix('!') {
            Some(pattern) => (true, pattern.trim()),
            None => (false, rule),
        };
        let matched = ip_rule_pattern_matches(pattern, remote_ip);
        if deny && matched {
            return false;
        }
        if !deny {
            has_allow_rule = true;
            if matched {
                matched_allow_rule = true;
            }
        }
    }

    if has_allow_rule {
        matched_allow_rule
    } else {
        true
    }
}

pub(crate) fn json_ip_rules_allow(value: Option<&Value>, remote_ip: IpAddr) -> bool {
    let Some(value) = value else {
        return true;
    };
    if value.is_null() {
        return true;
    }
    let Some(items) = value.as_array() else {
        return false;
    };
    let mut rules = Vec::with_capacity(items.len());
    for item in items {
        let Some(rule) = item.as_str() else {
            return false;
        };
        rules.push(rule.to_string());
    }
    ip_rules_allow(Some(&rules), remote_ip)
}

fn normalize_ip_rule(raw: &str) -> Result<String, String> {
    if raw.is_empty() {
        return Err("IP 限制规则不能为空".to_string());
    }
    let (deny, pattern) = match raw.strip_prefix('!') {
        Some(pattern) => (true, pattern.trim()),
        None => (false, raw),
    };
    if pattern.is_empty() {
        return Err("IP 限制规则不能为空".to_string());
    }
    if !valid_ip_rule_pattern(pattern) {
        return Err(format!("无效的 IP 限制规则: {raw}"));
    }
    if deny {
        Ok(format!("!{pattern}"))
    } else {
        Ok(pattern.to_string())
    }
}

fn valid_ip_rule_pattern(pattern: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if pattern.parse::<IpAddr>().is_ok() {
        return true;
    }
    if valid_cidr_pattern(pattern) {
        return true;
    }
    valid_ipv4_wildcard_pattern(pattern)
}

fn valid_cidr_pattern(pattern: &str) -> bool {
    let Some((host, prefix)) = pattern.split_once('/') else {
        return false;
    };
    let Ok(ip) = host.trim().parse::<IpAddr>() else {
        return false;
    };
    let Ok(prefix) = prefix.trim().parse::<u8>() else {
        return false;
    };
    match ip {
        IpAddr::V4(_) => prefix <= 32,
        IpAddr::V6(_) => prefix <= 128,
    }
}

fn valid_ipv4_wildcard_pattern(pattern: &str) -> bool {
    if !pattern.contains('*') {
        return false;
    }
    let parts = pattern.split('.').collect::<Vec<_>>();
    parts.len() == 4
        && parts
            .iter()
            .all(|part| *part == "*" || part.parse::<u8>().is_ok())
}

pub(crate) fn ip_rule_pattern_matches(pattern: &str, remote_ip: IpAddr) -> bool {
    if pattern == "*" {
        return true;
    }
    if let Ok(ip) = pattern.parse::<IpAddr>() {
        return ip == remote_ip;
    }
    if ipv4_wildcard_matches(pattern, remote_ip) {
        return true;
    }
    let Some((network, prefix)) = pattern.split_once('/') else {
        return false;
    };
    let Ok(prefix) = prefix.trim().parse::<u8>() else {
        return false;
    };
    match (network.trim().parse::<IpAddr>(), remote_ip) {
        (Ok(IpAddr::V4(network)), IpAddr::V4(remote)) if prefix <= 32 => {
            let mask = if prefix == 0 {
                0
            } else {
                u32::MAX << (32 - prefix)
            };
            (u32::from(network) & mask) == (u32::from(remote) & mask)
        }
        (Ok(IpAddr::V6(network)), IpAddr::V6(remote)) if prefix <= 128 => {
            let mask = if prefix == 0 {
                0
            } else {
                u128::MAX << (128 - prefix)
            };
            (u128::from(network) & mask) == (u128::from(remote) & mask)
        }
        _ => false,
    }
}

fn ipv4_wildcard_matches(pattern: &str, remote_ip: IpAddr) -> bool {
    let IpAddr::V4(remote_ip) = remote_ip else {
        return false;
    };
    if !valid_ipv4_wildcard_pattern(pattern) {
        return false;
    }
    pattern
        .split('.')
        .zip(remote_ip.octets())
        .all(|(pattern_part, remote_part)| {
            pattern_part == "*" || pattern_part.parse::<u8>() == Ok(remote_part)
        })
}

pub(crate) fn deserialize_optional_json_patch<'de, D>(
    deserializer: D,
) -> Result<Option<Option<Value>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    <Option<Value> as serde::Deserialize>::deserialize(deserializer).map(Some)
}

pub(crate) fn deserialize_optional_string_list_patch<'de, D>(
    deserializer: D,
) -> Result<Option<Option<Vec<String>>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    <Option<Vec<String>> as serde::Deserialize>::deserialize(deserializer).map(Some)
}

fn normalize_chat_pii_redaction_feature_settings(
    settings: &mut Map<String, Value>,
) -> Result<(), String> {
    let Some(value) = settings.get_mut("chat_pii_redaction") else {
        return Ok(());
    };
    match value {
        Value::Null => {
            settings.remove("chat_pii_redaction");
            Ok(())
        }
        Value::Object(feature) => {
            normalize_chat_pii_redaction_feature_object(feature)?;
            if feature.is_empty() {
                settings.remove("chat_pii_redaction");
            }
            Ok(())
        }
        _ => Err("chat_pii_redaction 必须是对象".to_string()),
    }
}

fn normalize_chat_pii_redaction_feature_object(
    feature: &mut Map<String, Value>,
) -> Result<(), String> {
    feature.remove("inject_model_instruction");
    for key in ["enabled"] {
        if let Some(value) = feature.get(key) {
            if !value.is_boolean() {
                return Err(format!("chat_pii_redaction.{key} 必须是布尔值"));
            }
        }
    }
    Ok(())
}

fn normalize_notification_push_service_feature_settings(
    settings: &mut Map<String, Value>,
) -> Result<(), String> {
    let Some(value) = settings.get_mut("notification_push_service") else {
        return Ok(());
    };
    match value {
        Value::Null => {
            settings.remove("notification_push_service");
            Ok(())
        }
        Value::Object(feature) => {
            normalize_notification_push_service_feature_object(feature)?;
            if feature.is_empty() {
                settings.remove("notification_push_service");
            }
            Ok(())
        }
        _ => Err("notification_push_service 必须是对象".to_string()),
    }
}

fn normalize_notification_push_service_feature_object(
    feature: &mut Map<String, Value>,
) -> Result<(), String> {
    for key in ["enabled"] {
        if let Some(value) = feature.get(key) {
            if !value.is_boolean() {
                return Err(format!("notification_push_service.{key} 必须是布尔值"));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        ip_rules_allow, json_ip_rules_allow, normalize_feature_settings, normalize_ip_rules,
        normalize_user_self_feature_settings_update, parse_json_ip_rules,
    };
    use serde_json::json;
    use std::net::{IpAddr, Ipv4Addr};

    fn v4(a: u8, b: u8, c: u8, d: u8) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(a, b, c, d))
    }

    #[test]
    fn normalize_ip_rules_accepts_ip_cidr_wildcard_and_deny_rules() {
        let rules = normalize_ip_rules(Some(vec![
            " 203.0.113.10 ".to_string(),
            "10.0.0.0/24".to_string(),
            "192.168.*.*".to_string(),
            "! 10.0.0.13 ".to_string(),
            "203.0.113.10".to_string(),
        ]))
        .expect("valid IP rules should normalize");

        assert_eq!(
            rules,
            Some(vec![
                "203.0.113.10".to_string(),
                "10.0.0.0/24".to_string(),
                "192.168.*.*".to_string(),
                "!10.0.0.13".to_string(),
            ]),
        );
    }

    #[test]
    fn normalize_feature_settings_accepts_notification_push_service_permission() {
        let normalized = normalize_feature_settings(Some(json!({
            "notification_push_service": {"enabled": true}
        })))
        .expect("feature settings should normalize")
        .expect("feature settings should remain set");

        assert_eq!(
            normalized["notification_push_service"]["enabled"],
            json!(true)
        );
    }

    #[test]
    fn user_self_feature_update_preserves_notification_push_permission() {
        let normalized = normalize_user_self_feature_settings_update(
            Some(json!({
                "chat_pii_redaction": {"enabled": true},
                "notification_push_service": {"enabled": false}
            })),
            Some(json!({
                "notification_push_service": {"enabled": true}
            })),
        )
        .expect("feature settings should normalize")
        .expect("feature settings should remain set");

        assert_eq!(
            normalized["notification_push_service"]["enabled"],
            json!(true)
        );
        assert_eq!(normalized["chat_pii_redaction"]["enabled"], json!(true));
    }

    #[test]
    fn ip_rules_allow_applies_allow_rules_and_deny_overrides() {
        let rules = vec![
            "10.0.0.0/24".to_string(),
            "192.168.*.*".to_string(),
            "!10.0.0.13".to_string(),
        ];

        assert!(ip_rules_allow(Some(&rules), v4(10, 0, 0, 12)));
        assert!(ip_rules_allow(Some(&rules), v4(192, 168, 2, 3)));
        assert!(!ip_rules_allow(Some(&rules), v4(10, 0, 0, 13)));
        assert!(!ip_rules_allow(Some(&rules), v4(203, 0, 113, 10)));
    }

    #[test]
    fn ip_rules_allow_defaults_to_allow_when_only_deny_rules_exist() {
        let rules = vec!["!10.0.*.*".to_string()];

        assert!(!ip_rules_allow(Some(&rules), v4(10, 0, 0, 13)));
        assert!(ip_rules_allow(Some(&rules), v4(203, 0, 113, 10)));
    }

    #[test]
    fn parse_json_ip_rules_normalizes_empty_and_string_arrays() {
        assert_eq!(
            parse_json_ip_rules(Some(&json!([" 203.0.113.10 ", "!10.0.0.13"])))
                .expect("valid JSON IP rules should parse"),
            Some(json!(["203.0.113.10", "!10.0.0.13"])),
        );
        assert_eq!(
            parse_json_ip_rules(Some(&json!([]))).expect("empty rules should parse"),
            None,
        );
    }

    #[test]
    fn json_ip_rules_allow_rejects_invalid_stored_shape() {
        assert!(!json_ip_rules_allow(
            Some(&json!({"bad": true})),
            v4(10, 0, 0, 1)
        ));
        assert!(!json_ip_rules_allow(Some(&json!([123])), v4(10, 0, 0, 1)));
    }
}
