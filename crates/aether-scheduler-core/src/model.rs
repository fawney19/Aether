use std::collections::BTreeSet;

use aether_data_contracts::repository::candidate_selection::{
    StoredMinimalCandidateSelectionRow, StoredProviderModelMapping,
};
use aether_data_contracts::DataLayerError;
use regex::RegexBuilder;

pub fn resolve_requested_global_model_name(
    rows: &[StoredMinimalCandidateSelectionRow],
    requested_model_name: &str,
    api_format: &str,
) -> Option<String> {
    resolve_global_model_name_by(rows, |row| row.global_model_name == requested_model_name)
        .or_else(|| {
            resolve_global_model_name_by(rows, |row| {
                row.model_provider_model_name == requested_model_name
            })
        })
        .or_else(|| {
            resolve_global_model_name_by(rows, |row| {
                row.model_provider_model_mappings
                    .as_ref()
                    .is_some_and(|mappings| {
                        mappings.iter().any(|mapping| {
                            mapping_scope_matches(mapping, api_format)
                                && mapping.name == requested_model_name
                        })
                    })
            })
        })
        .or_else(|| {
            resolve_global_model_name_by(rows, |row| {
                row.global_model_mappings.as_ref().is_some_and(|patterns| {
                    patterns
                        .iter()
                        .any(|pattern| matches_model_mapping(pattern, requested_model_name))
                })
            })
        })
}

pub fn row_supports_requested_model(
    row: &StoredMinimalCandidateSelectionRow,
    requested_model_name: &str,
    api_format: &str,
) -> bool {
    row.global_model_name == requested_model_name
        || row.model_provider_model_name == requested_model_name
        || row
            .model_provider_model_mappings
            .as_ref()
            .is_some_and(|mappings| {
                mappings.iter().any(|mapping| {
                    mapping_scope_matches(mapping, api_format)
                        && mapping.name == requested_model_name
                })
            })
        || row.global_model_mappings.as_ref().is_some_and(|patterns| {
            patterns
                .iter()
                .any(|pattern| matches_model_mapping(pattern, requested_model_name))
        })
}

fn resolve_global_model_name_by<F>(
    rows: &[StoredMinimalCandidateSelectionRow],
    matches: F,
) -> Option<String>
where
    F: Fn(&StoredMinimalCandidateSelectionRow) -> bool,
{
    let mut best_match = None::<&str>;
    for row in rows.iter().filter(|row| matches(row)) {
        let candidate = row.global_model_name.trim();
        if candidate.is_empty() {
            continue;
        }
        if best_match.is_none_or(|current| candidate < current) {
            best_match = Some(candidate);
        }
    }
    best_match.map(ToOwned::to_owned)
}

pub fn resolve_provider_model_name(
    row: &StoredMinimalCandidateSelectionRow,
    requested_model_name: &str,
    api_format: &str,
) -> Option<(String, Option<String>)> {
    let selected_provider_model_name = select_provider_model_name(row, api_format);
    let Some(key_allowed_models) = row.key_allowed_models.as_ref() else {
        return Some((selected_provider_model_name, None));
    };
    if key_allowed_models.is_empty() {
        return None;
    }

    if key_allowed_models
        .iter()
        .any(|value| value == requested_model_name)
    {
        return Some((selected_provider_model_name, None));
    }

    let mut sorted_allowed_models = key_allowed_models
        .iter()
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    sorted_allowed_models.sort_unstable();

    for &allowed_model in &sorted_allowed_models {
        if row_has_candidate_model_name(row, api_format, allowed_model) {
            let allowed_model = allowed_model.to_owned();
            return Some((allowed_model.clone(), Some(allowed_model)));
        }
    }

    let global_model_mappings = row.global_model_mappings.as_ref()?;
    for &allowed_model in &sorted_allowed_models {
        for pattern in global_model_mappings {
            if matches_model_mapping(pattern, allowed_model) {
                let allowed_model = allowed_model.to_owned();
                return Some((allowed_model.clone(), Some(allowed_model)));
            }
        }
    }

    None
}

pub fn select_provider_model_name(
    row: &StoredMinimalCandidateSelectionRow,
    api_format: &str,
) -> String {
    let Some(mappings) = row.model_provider_model_mappings.as_ref() else {
        return row.model_provider_model_name.clone();
    };

    mappings
        .iter()
        .filter(|mapping| mapping_scope_matches(mapping, api_format))
        .min_by(|left, right| {
            left.priority
                .cmp(&right.priority)
                .then(left.name.cmp(&right.name))
        })
        .map(|mapping| mapping.name.clone())
        .unwrap_or_else(|| row.model_provider_model_name.clone())
}

pub fn candidate_model_names(
    row: &StoredMinimalCandidateSelectionRow,
    api_format: &str,
) -> BTreeSet<String> {
    let mut names = BTreeSet::from([row.model_provider_model_name.clone()]);
    if let Some(mappings) = row.model_provider_model_mappings.as_ref() {
        for mapping in mappings {
            if mapping_scope_matches(mapping, api_format) {
                names.insert(mapping.name.clone());
            }
        }
    }
    names
}

fn mapping_scope_matches(mapping: &StoredProviderModelMapping, api_format: &str) -> bool {
    let Some(api_formats) = mapping.api_formats.as_ref() else {
        return true;
    };

    api_formats
        .iter()
        .any(|value| api_format_matches(value, api_format))
}

pub fn row_supports_required_capability(
    row: &StoredMinimalCandidateSelectionRow,
    required_capability: &str,
) -> bool {
    capabilities_support_required_capability(row.key_capabilities.as_ref(), required_capability)
}

fn capabilities_support_required_capability(
    capabilities: Option<&serde_json::Value>,
    required_capability: &str,
) -> bool {
    let required_capability = required_capability.trim();
    if required_capability.is_empty() {
        return true;
    }
    let Some(capabilities) = capabilities else {
        return false;
    };

    if let Some(object) = capabilities.as_object() {
        return object.iter().any(|(key, value)| {
            key.eq_ignore_ascii_case(required_capability)
                && match value {
                    serde_json::Value::Bool(value) => *value,
                    serde_json::Value::String(value) => value.eq_ignore_ascii_case("true"),
                    serde_json::Value::Number(value) => {
                        value.as_i64().is_some_and(|value| value > 0)
                    }
                    _ => false,
                }
        });
    }

    if let Some(items) = capabilities.as_array() {
        return items.iter().any(|value| {
            value
                .as_str()
                .is_some_and(|value| value.eq_ignore_ascii_case(required_capability))
        });
    }

    false
}

pub fn matches_model_mapping(pattern: &str, model_name: &str) -> bool {
    if pattern.eq_ignore_ascii_case(model_name) {
        return true;
    }

    let regex_pattern = format!("^(?:{pattern})$");
    let Ok(compiled) = RegexBuilder::new(&regex_pattern)
        .case_insensitive(true)
        .build()
    else {
        return false;
    };
    compiled.is_match(model_name)
}

pub fn extract_global_priority_for_format(
    raw: Option<&serde_json::Value>,
    api_format: &str,
) -> Result<Option<i32>, DataLayerError> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let Some(object) = raw.as_object() else {
        return Err(DataLayerError::UnexpectedValue(
            "provider_api_keys.global_priority_by_format is not a JSON object".to_string(),
        ));
    };

    let Some(value) = object
        .iter()
        .find(|(key, _)| api_format_matches(key, api_format))
        .map(|(_, value)| value)
    else {
        return Ok(None);
    };

    if let Some(value) = value.as_i64() {
        return i32::try_from(value).map(Some).map_err(|_| {
            DataLayerError::UnexpectedValue(format!(
                "invalid provider_api_keys.global_priority_by_format value: {value}"
            ))
        });
    }

    if let Some(value) = value.as_str() {
        let value = value.trim().parse::<i32>().map_err(|_| {
            DataLayerError::UnexpectedValue(format!(
                "invalid provider_api_keys.global_priority_by_format value: {value}"
            ))
        })?;
        return Ok(Some(value));
    }

    Err(DataLayerError::UnexpectedValue(
        "provider_api_keys.global_priority_by_format contains a non-integer value".to_string(),
    ))
}

pub fn normalize_api_format(value: &str) -> String {
    aether_ai_formats::normalize_legacy_openai_format_alias(value)
}

fn row_has_candidate_model_name(
    row: &StoredMinimalCandidateSelectionRow,
    api_format: &str,
    model_name: &str,
) -> bool {
    row.model_provider_model_name == model_name
        || row
            .model_provider_model_mappings
            .as_ref()
            .is_some_and(|mappings| {
                mappings.iter().any(|mapping| {
                    mapping_scope_matches(mapping, api_format) && mapping.name == model_name
                })
            })
}

fn api_format_matches(left: &str, right: &str) -> bool {
    normalize_api_format(left) == normalize_api_format(right)
}

#[cfg(test)]
mod tests {
    use super::matches_model_mapping;

    #[test]
    fn model_mapping_match_is_case_insensitive() {
        assert!(matches_model_mapping("gpt-4o", "GPT-4O"));
        assert!(matches_model_mapping("gpt-5(?:\\.\\d+)?", "GPT-5.1"));
    }

    #[test]
    fn model_mapping_match_is_anchored_to_full_text() {
        assert!(matches_model_mapping("gpt-4o", "gpt-4o"));
        assert!(!matches_model_mapping("gpt-4o", "gpt-4o-mini"));
    }

    #[test]
    fn invalid_model_mapping_pattern_returns_false() {
        assert!(!matches_model_mapping("([a-z", "gpt-4o"));
    }
}
