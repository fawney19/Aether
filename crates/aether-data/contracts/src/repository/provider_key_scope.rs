//! Provider-scoped channel key allowlists.
//!
//! An `allowed_provider_key_ids` policy is a JSON object mapping stable
//! `provider_api_keys.provider_id` values to lists of `provider_api_keys.id`
//! values: `{ "<provider_id>": ["<key_id>", ...] }`.
//!
//! Semantics (must be preserved everywhere):
//! - `None` (SQL `NULL`) means "no key-level restriction": for every allowed
//!   provider, all active keys are usable (legacy provider-only behavior).
//! - A provider entry with a non-empty key set restricts that provider to the
//!   listed keys.
//! - A provider with no entry is unrestricted (all active keys).
//! - Empty objects / empty arrays are normalized to `None`. There is no
//!   "empty set = deny all" representation: the provider allowlist already
//!   controls provider-level deny.
//!
//! Keys in the map are stable provider IDs and stable key IDs only. Provider
//! name/type aliases are never persisted here.

use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// Stable provider ID -> set of allowed `provider_api_keys.id` values.
pub type ProviderKeyScope = BTreeMap<String, BTreeSet<String>>;

/// Parses a stored JSON value (object, stringified object, `NULL`, absent)
/// into a normalized provider key scope. Empty entries and empty maps are
/// normalized away; the result is `None` when there is no restriction.
pub fn parse_provider_key_scope(
    value: Option<Value>,
    field_name: &str,
) -> Result<Option<ProviderKeyScope>, crate::DataLayerError> {
    let Some(value) = value else {
        return Ok(None);
    };
    parse_provider_key_scope_value(&value, field_name)
}

pub fn parse_provider_key_scope_value(
    value: &Value,
    field_name: &str,
) -> Result<Option<ProviderKeyScope>, crate::DataLayerError> {
    match value {
        Value::Null => Ok(None),
        Value::Object(map) => parse_provider_key_scope_object(map, field_name),
        Value::String(raw) => {
            let raw = raw.trim();
            if raw.is_empty() || raw.eq_ignore_ascii_case("null") {
                return Ok(None);
            }
            let decoded = serde_json::from_str::<Value>(raw).map_err(|_| {
                crate::DataLayerError::UnexpectedValue(format!(
                    "{field_name} is not a valid JSON object"
                ))
            })?;
            parse_provider_key_scope_value(&decoded, field_name)
        }
        _ => Err(crate::DataLayerError::UnexpectedValue(format!(
            "{field_name} is not a JSON object"
        ))),
    }
}

fn parse_provider_key_scope_object(
    map: &serde_json::Map<String, Value>,
    field_name: &str,
) -> Result<Option<ProviderKeyScope>, crate::DataLayerError> {
    let mut scope = ProviderKeyScope::new();
    for (provider_id, value) in map {
        let provider_id = provider_id.trim();
        if provider_id.is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(format!(
                "{field_name} contains an empty provider id"
            )));
        }
        let key_ids = match value {
            Value::Array(items) => {
                parse_provider_key_scope_key_items(items, field_name, provider_id)?
            }
            Value::String(raw) => {
                let raw = raw.trim();
                if raw.is_empty() {
                    continue;
                }
                let decoded = serde_json::from_str::<Value>(raw).map_err(|_| {
                    crate::DataLayerError::UnexpectedValue(format!(
                        "{field_name}.{provider_id} is not a JSON array"
                    ))
                })?;
                match decoded {
                    Value::Array(items) => {
                        parse_provider_key_scope_key_items(&items, field_name, provider_id)?
                    }
                    Value::String(single) => {
                        let mut key_ids = BTreeSet::new();
                        let key_id = single.trim();
                        if !key_id.is_empty() {
                            key_ids.insert(key_id.to_string());
                        }
                        key_ids
                    }
                    _ => {
                        return Err(crate::DataLayerError::UnexpectedValue(format!(
                            "{field_name}.{provider_id} is not a JSON array"
                        )));
                    }
                }
            }
            _ => {
                return Err(crate::DataLayerError::UnexpectedValue(format!(
                    "{field_name}.{provider_id} is not a JSON array"
                )));
            }
        };
        if !key_ids.is_empty() {
            scope.insert(provider_id.to_string(), key_ids);
        }
    }
    Ok(normalize_provider_key_scope(Some(scope)))
}

fn parse_provider_key_scope_key_items(
    items: &[Value],
    field_name: &str,
    provider_id: &str,
) -> Result<BTreeSet<String>, crate::DataLayerError> {
    let mut key_ids = BTreeSet::new();
    for item in items {
        let Some(key_id) = item.as_str() else {
            return Err(crate::DataLayerError::UnexpectedValue(format!(
                "{field_name}.{provider_id} contains a non-string key id"
            )));
        };
        let key_id = key_id.trim();
        if !key_id.is_empty() {
            key_ids.insert(key_id.to_string());
        }
    }
    Ok(key_ids)
}

/// Drops empty provider entries and returns `None` for an empty map.
pub fn normalize_provider_key_scope(scope: Option<ProviderKeyScope>) -> Option<ProviderKeyScope> {
    let Some(scope) = scope else {
        return None;
    };
    if scope.is_empty() {
        return None;
    }
    let mut normalized = ProviderKeyScope::new();
    for (provider_id, key_ids) in scope {
        if !key_ids.is_empty() {
            normalized.insert(provider_id, key_ids);
        }
    }
    (!normalized.is_empty()).then_some(normalized)
}

/// Serializes a scope for SQL JSON storage.
pub fn serialize_provider_key_scope(
    scope: Option<&ProviderKeyScope>,
    field_name: &str,
) -> Result<Option<String>, crate::DataLayerError> {
    scope
        .map(|scope| {
            let value = serde_json::to_value(scope).map_err(|err| {
                crate::DataLayerError::UnexpectedValue(format!(
                    "{field_name} contains unserializable provider key scope: {err}"
                ))
            })?;
            serde_json::to_string(&value).map_err(|err| {
                crate::DataLayerError::UnexpectedValue(format!(
                    "{field_name} contains unserializable provider key scope: {err}"
                ))
            })
        })
        .transpose()
}

/// Restricts a scope map to the given effective provider list (stable IDs).
/// Entries for providers outside the list are dropped.
pub fn restrict_provider_key_scope_to_providers(
    scope: Option<ProviderKeyScope>,
    allowed_provider_ids: &BTreeSet<String>,
) -> Option<ProviderKeyScope> {
    let Some(scope) = scope else {
        return None;
    };
    let mut restricted = ProviderKeyScope::new();
    for (provider_id, key_ids) in scope {
        if allowed_provider_ids.contains(&provider_id) {
            restricted.insert(provider_id, key_ids);
        }
    }
    normalize_provider_key_scope(Some(restricted))
}

/// Merges group-level scopes by unioning key sets per provider. A `None`
/// input contributes nothing. The result is normalized.
pub fn merge_provider_key_scopes<'a>(
    scopes: impl IntoIterator<Item = Option<ProviderKeyScope>>,
) -> Option<ProviderKeyScope> {
    let mut merged = ProviderKeyScope::new();
    for scope in scopes.into_iter().flatten() {
        for (provider_id, key_ids) in scope {
            merged.entry(provider_id).or_default().extend(key_ids);
        }
    }
    normalize_provider_key_scope(Some(merged))
}

/// Intersects two scopes per provider (used to keep non-standalone keys at
/// the intersection of the key's own scope and the user/group scope). A
/// provider present in only one side keeps that side's keys; a provider with
/// an empty intersection is dropped (no entry = unrestricted, matching the
/// provider allowlist intersection which already handles provider denial).
pub fn intersect_provider_key_scopes(
    left: Option<&ProviderKeyScope>,
    right: Option<&ProviderKeyScope>,
) -> Option<ProviderKeyScope> {
    match (left, right) {
        (None, None) => None,
        (Some(scope), None) | (None, Some(scope)) => {
            normalize_provider_key_scope(Some(scope.clone()))
        }
        (Some(left), Some(right)) => {
            let mut merged = ProviderKeyScope::new();
            for (provider_id, left_keys) in left {
                match right.get(provider_id) {
                    Some(right_keys) => {
                        let intersection = left_keys
                            .intersection(right_keys)
                            .cloned()
                            .collect::<BTreeSet<_>>();
                        if !intersection.is_empty() {
                            merged.insert(provider_id.clone(), intersection);
                        }
                    }
                    None => {
                        merged.insert(provider_id.clone(), left_keys.clone());
                    }
                }
            }
            for (provider_id, right_keys) in right {
                if !left.contains_key(provider_id) {
                    merged.insert(provider_id.clone(), right_keys.clone());
                }
            }
            normalize_provider_key_scope(Some(merged))
        }
    }
}

/// Removes deleted key IDs from every provider entry of a scope.
pub fn remove_key_ids_from_provider_key_scope(
    scope: Option<ProviderKeyScope>,
    removed_key_ids: &BTreeSet<String>,
) -> Option<ProviderKeyScope> {
    if removed_key_ids.is_empty() {
        return normalize_provider_key_scope(scope);
    }
    let Some(scope) = scope else {
        return None;
    };
    let mut pruned = ProviderKeyScope::new();
    for (provider_id, key_ids) in scope {
        let kept = key_ids
            .difference(removed_key_ids)
            .cloned()
            .collect::<BTreeSet<_>>();
        if !kept.is_empty() {
            pruned.insert(provider_id, kept);
        }
    }
    normalize_provider_key_scope(Some(pruned))
}

/// Cross-table cleanup for deleted `provider_api_keys` rows.
///
/// `allowed_provider_key_ids` lives in both `api_keys` and `user_groups` and
/// has no foreign keys, so deleting a provider key would otherwise leave stale
/// references that silently remove all candidates for the provider. This trait
/// prunes the deleted key IDs from every policy row in both tables.
#[async_trait::async_trait]
pub trait ProviderKeyScopeCleanupRepository: Send + Sync {
    /// Removes `key_ids` from every `allowed_provider_key_ids` value in
    /// `api_keys` and `user_groups`. Returns the number of rows updated.
    async fn prune_provider_key_scope_references(
        &self,
        key_ids: &[String],
    ) -> Result<u64, crate::DataLayerError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DataLayerError;

    fn scope(items: &[(&str, &[&str])]) -> Option<ProviderKeyScope> {
        let mut map = ProviderKeyScope::new();
        for (provider_id, key_ids) in items {
            map.insert(
                provider_id.to_string(),
                key_ids.iter().map(|value| value.to_string()).collect(),
            );
        }
        normalize_provider_key_scope(Some(map))
    }

    #[test]
    fn parses_object_scope_and_normalizes_whitespace() {
        let parsed = parse_provider_key_scope(
            Some(serde_json::json!({
                " provider-1 ": [" key-a ", "key-b", " key-a "],
                "provider-2": [],
                "provider-3": ["key-c"]
            })),
            "api_keys.allowed_provider_key_ids",
        )
        .expect("scope should parse");

        assert_eq!(
            parsed,
            scope(&[
                ("provider-1", &["key-a", "key-b"]),
                ("provider-3", &["key-c"]),
            ])
        );
    }

    #[test]
    fn parses_stringified_object_scope() {
        let parsed = parse_provider_key_scope(
            Some(serde_json::json!(
                "{\"provider-1\": [\"key-a\", \"key-b\"]}"
            )),
            "user_groups.allowed_provider_key_ids",
        )
        .expect("scope should parse");

        assert_eq!(parsed, scope(&[("provider-1", &["key-a", "key-b"])]));
    }

    #[test]
    fn null_and_empty_values_normalize_to_none() {
        assert_eq!(parse_provider_key_scope(None, "f").expect("parse"), None);
        assert_eq!(
            parse_provider_key_scope(Some(Value::Null), "f").expect("parse"),
            None
        );
        assert_eq!(
            parse_provider_key_scope(Some(serde_json::json!("null")), "f").expect("parse"),
            None
        );
        assert_eq!(
            parse_provider_key_scope(Some(serde_json::json!({})), "f").expect("parse"),
            None
        );
        assert_eq!(
            parse_provider_key_scope(Some(serde_json::json!({"p": []})), "f").expect("parse"),
            None
        );
    }

    #[test]
    fn rejects_non_object_scope() {
        assert!(matches!(
            parse_provider_key_scope(Some(serde_json::json!(["a"])), "f"),
            Err(DataLayerError::UnexpectedValue(_))
        ));
        assert!(matches!(
            parse_provider_key_scope(Some(serde_json::json!({"p": 3})), "f"),
            Err(DataLayerError::UnexpectedValue(_))
        ));
        assert!(matches!(
            parse_provider_key_scope(Some(serde_json::json!({"p": ["a", 3]})), "f"),
            Err(DataLayerError::UnexpectedValue(_))
        ));
    }

    #[test]
    fn intersect_keeps_disjoint_sides_and_intersects_shared_providers() {
        let left = scope(&[("p1", &["a", "b"]), ("p2", &["x"])]);
        let right = scope(&[("p1", &["b", "c"]), ("p3", &["z"])]);

        assert_eq!(
            intersect_provider_key_scopes(left.as_ref(), right.as_ref()),
            scope(&[("p1", &["b"]), ("p2", &["x"]), ("p3", &["z"])])
        );
    }

    #[test]
    fn intersect_drops_provider_with_empty_intersection() {
        let left = scope(&[("p1", &["a"])]);
        let right = scope(&[("p1", &["b"])]);
        assert_eq!(
            intersect_provider_key_scopes(left.as_ref(), right.as_ref()),
            None
        );
    }

    #[test]
    fn merge_unions_keys_per_provider() {
        let merged = merge_provider_key_scopes([
            scope(&[("p1", &["a"])]),
            scope(&[("p1", &["b"]), ("p2", &["c"])]),
            None,
        ]);
        assert_eq!(merged, scope(&[("p1", &["a", "b"]), ("p2", &["c"])]));
    }

    #[test]
    fn restrict_filters_to_allowed_provider_ids() {
        let allowed = ["p1"].into_iter().map(str::to_string).collect();
        assert_eq!(
            restrict_provider_key_scope_to_providers(
                scope(&[("p1", &["a"]), ("p2", &["b"])]),
                &allowed,
            ),
            scope(&[("p1", &["a"])])
        );
    }

    #[test]
    fn remove_key_ids_prunes_all_provider_entries() {
        let pruned = remove_key_ids_from_provider_key_scope(
            scope(&[("p1", &["a", "b"]), ("p2", &["b", "c"])]),
            &["b"].into_iter().map(str::to_string).collect(),
        );
        assert_eq!(pruned, scope(&[("p1", &["a"]), ("p2", &["c"])]));
    }

    #[test]
    fn remove_key_ids_drops_provider_when_all_keys_removed() {
        let pruned = remove_key_ids_from_provider_key_scope(
            scope(&[("p1", &["a"])]),
            &["a"].into_iter().map(str::to_string).collect(),
        );
        assert_eq!(pruned, None);
    }

    #[test]
    fn serializes_round_trip() {
        let value = scope(&[("p1", &["a", "b"])]);
        let json = serialize_provider_key_scope(value.as_ref(), "f")
            .expect("serialize")
            .expect("some");
        assert_eq!(
            parse_provider_key_scope(serde_json::from_str(&json).ok(), "f").expect("parse"),
            value
        );
    }
}
